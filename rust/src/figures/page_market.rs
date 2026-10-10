//! Raw market responses are shared; bounded decimals are made and dropped only on the computing thread.
use super::at::At;
use super::db::Ticker;
use super::dec::Dec;
use super::market::{Failure, Fetch, MarketData, Member, Quoted, Venue};
use super::num::Num;
use crate::venue::http::{HttpRequest, Transport, TransportError};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

pub const PRICE_TTL: i64 = 60;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 512;
const UNAVAILABLE: &str = "Market data unavailable";

#[derive(Clone)]
struct Entry { until: i64, value: Fetch<Value>, origin:Option<crate::engine::model::CredentialVersion> }
#[derive(Clone)]
struct Head { until: i64, bars: Vec<Value>, origin:Option<crate::engine::model::CredentialVersion> }
#[derive(Clone, Default)]
pub struct Cache { entries: RefCell<BTreeMap<String, Entry>>, heads: RefCell<BTreeMap<String, Head>>, stamp: Option<At>, origin:Option<crate::engine::model::CredentialVersion> }

fn key(request: &HttpRequest) -> String {
    format!("{}?{}", request.path, request.query.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"))
}
fn request(path: &str, query: Vec<(&'static str, String)>) -> HttpRequest {
    HttpRequest { method: "GET", base: "https://data.alpaca.markets".into(), path: path.into(), query, body: None, not_after: None }
}

pub const CANDLE_TTL: i64 = 30 * 86_400;
fn seconds(request: &HttpRequest) -> i64 {
    match request.query.iter().find(|(k,_)| *k == "timeframe").map(|(_,v)| v.as_str()) {
        Some("1Min") => 60, Some("5Min") => 300, Some("15Min") => 900, Some("30Min") => 1800,
        Some("1Hour") => 3600, Some("4Hour") => 14400, Some("1Week") => 604800, Some("1Month") => 2592000, _ => 86400,
    }
}
fn bar_time(bar: &Value) -> Option<i64> {
    bar["t"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp())
}
fn bar_price(bar: &Value) -> Fetch<Dec> {
    let price = bar.get("o").filter(|p| !p.is_null()).ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
    Ok(Dec::to_d(price)?)
}
async fn fetch(wire: &impl Transport, request: &HttpRequest) -> Fetch<Value> {
    match wire.send_limited(request, MAX_BYTES).await {
        Ok(reply) if (200..300).contains(&reply.status) => crate::venue::http::decode_json(&reply.body)
            .map_err(|_| Failure::Failed(UNAVAILABLE.into())),
        Ok(_) | Err(TransportError::Permanent(_)) => Err(Failure::Failed(UNAVAILABLE.into())),
        Err(_) => Err(Failure::Raised(UNAVAILABLE.into())),
    }
}
fn closed(body: Value, request: &HttpRequest, now: i64) -> Fetch<Vec<Value>> {
    let bars = if request.path.starts_with("/v1beta3/") {
        let symbol = request.query.iter().find(|(k,_)| *k == "symbols").map_or("", |(_,v)| v);
        body["bars"][symbol].as_array()
    } else { body["bars"].as_array() };
    let Some(bars) = bars else { return Err(Failure::Failed(UNAVAILABLE.into())); };
    let mut out = vec![];
    for bar in bars {
        let time = bar_time(bar).ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
        if time.saturating_add(seconds(request)) <= now {
            // Validate even overlapping bars before a merge can discard them.
            bar_price(bar)?;
            out.push(bar.clone());
        }
    }
    Ok(out)
}
fn rewritten(old: &Value, tail: &[Value], adjusted: bool) -> Fetch<bool> {
    let Some(overlap) = tail.iter().find(|bar| bar_time(bar) == bar_time(old)) else { return Ok(adjusted) };
    // Decimals are local to this call and never survive an await or move to another thread.
    let before = bar_price(old)?;
    let after = bar_price(overlap)?;
    if before.is_zero() || after.is_zero() { return Ok(false); }
    let ratio = (&after - &before)?.div(&before)?;
    Ok(ratio > Dec::to_d(&serde_json::json!("0.001"))? || ratio < Dec::to_d(&serde_json::json!("-0.001"))?)
}
impl Cache {
    /// Bound the retained wire representation, independently of the decimal budget.
    pub fn bytes(&self) -> usize {
        self.entries.borrow().values().map(|e| e.value.as_ref().map_or(0,|v|v.to_string().len())).sum::<usize>()
            + self.heads.borrow().values().map(|h|h.bars.iter().map(|v|v.to_string().len()).sum::<usize>()).sum::<usize>()
    }
    pub fn producer(&self)->&Option<crate::engine::model::CredentialVersion> { &self.origin }
    pub fn stamp(&self) -> Option<At> { self.stamp }
    pub fn set_stamp(&mut self, at: At) { self.stamp = Some(at); }
    pub async fn fill(&mut self, wire: &impl Transport, demands: Vec<HttpRequest>, now: i64) {
        self.origin=wire.producer();
        for request in demands {
            let cache_key = key(&request);
            let is_bars = request.path.ends_with("/bars");
            let value = if is_bars { self.fill_candles(wire, &request, now).await } else { fetch(wire, &request).await };
            if self.entries.borrow().len() >= MAX_ENTRIES { self.entries.borrow_mut().retain(|_, entry| entry.until > now); }
            // This process is a cache, not a second database. Eviction only causes a new read.
            if self.entries.borrow().len() >= MAX_ENTRIES { self.entries.borrow_mut().clear(); }
            self.entries.borrow_mut().insert(cache_key, Entry { until: now.saturating_add(PRICE_TTL), value, origin:wire.producer() });
            if self.bytes() > 32 * 1024 * 1024 { self.entries.borrow_mut().clear(); self.heads.borrow_mut().clear(); }
        }
    }
    async fn fill_candles(&mut self, wire: &impl Transport, request: &HttpRequest, now: i64) -> Fetch<Value> {
        let cache_key = key(request);
        if self.heads.borrow().get(&cache_key).is_some_and(|h|!crate::engine::model::current_for(h.origin.as_ref(),wire.producer().as_ref()).is_fresh()) { self.heads.borrow_mut().remove(&cache_key); }
        let old = self.heads.borrow().get(&cache_key).filter(|h| h.until > now).map(|h| h.bars.clone()).unwrap_or_default();
        let mut tail_request = request.clone();
        if let Some(last) = old.last() {
            let start = last["t"].as_str().ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
            for (name, value) in &mut tail_request.query { if *name == "start" { *value = start.to_string(); } }
        }
        let tail = closed(fetch(wire, &tail_request).await?, request, now)?;
        let adjusted = request.query.iter().any(|(k,v)| *k == "adjustment" && v == "split");
        let rebuild = old.last().map(|last| rewritten(last, &tail, adjusted)).transpose()?.unwrap_or(false);
        let mut bars = if rebuild {
            closed(fetch(wire, request).await?, request, now)?
        } else {
            let last = old.last().and_then(bar_time);
            old.into_iter().chain(tail.into_iter().filter(|bar| last.is_none_or(|last| bar_time(bar).is_some_and(|t| t > last)))).collect()
        };
        // Stable sort preserves the first occurrence, as Ruby's uniq-before-sort does.
        bars.sort_by_key(bar_time);
        bars.dedup_by_key(|bar| bar_time(bar));
        if !bars.is_empty() {
            if self.heads.borrow().len() >= MAX_ENTRIES { self.heads.borrow_mut().clear(); }
            self.heads.borrow_mut().insert(cache_key, Head { until: now.saturating_add(CANDLE_TTL), bars: bars.clone(), origin:wire.producer() });
        }
        Ok(Value::Array(bars))
    }
}

pub struct Reader<'a> { cache: &'a Cache, now: i64, demands: RefCell<BTreeMap<String, HttpRequest>>, symbols: Option<Vec<String>>, failed: Cell<bool>, current:Option<&'a rusqlite::Connection> }
impl<'a> Reader<'a> {
    pub fn new(cache: &'a Cache, now: i64) -> Self { Self { cache, now, demands: RefCell::default(), symbols: None, failed: Cell::new(false), current:None } }
    pub fn with_current(mut self,c:&'a rusqlite::Connection)->Self { self.current=Some(c);self }
    pub fn failed(&self) -> bool { self.failed.get() }
    pub fn with_symbols(mut self, symbols: Vec<String>) -> Self { self.symbols = Some(symbols); self }
    pub fn demands(&self) -> Vec<HttpRequest> { self.demands.borrow().values().cloned().collect() }
    fn get(&self, request: HttpRequest) -> Fetch<Value> {
        let fresh=|origin:&Option<crate::engine::model::CredentialVersion>|->Result<bool,Failure> {
            let Some(c)=self.current else { return Ok(true) }; // The unbound numerical harness reads no stored credentials.
            if !crate::engine::model::current_for(origin.as_ref(),self.cache.origin.as_ref()).is_fresh(){return Ok(false)}
            match origin {Some(origin)=>crate::engine::model::credential_is_current(c,origin).map_err(|_|Failure::Failed(UNAVAILABLE.into())),None=>Ok(false)}
        };
        let key = key(&request);
        if let Some(entry)=self.cache.entries.borrow().get(&key){fresh(&entry.origin)?;}
        if let Some(head)=self.cache.heads.borrow().get(&key){fresh(&head.origin)?;}
        if self.cache.entries.borrow().get(&key).is_some_and(|e|!fresh(&e.origin).unwrap_or(false)){self.cache.entries.borrow_mut().remove(&key);}
        if self.cache.heads.borrow().get(&key).is_some_and(|h|!fresh(&h.origin).unwrap_or(false)){self.cache.heads.borrow_mut().remove(&key);}
        if request.path.ends_with("/bars") {
            if let Some(entry) = self.cache.entries.borrow().get(&key).filter(|entry| fresh(&entry.origin).unwrap_or(false) && entry.until > self.now) {
                return entry.value.clone();
            }
            if let Some(head) = self.cache.heads.borrow().get(&key).filter(|head| fresh(&head.origin).unwrap_or(false) && head.until > self.now && head.bars.last().and_then(bar_time).is_some_and(|t| self.now < t.saturating_add(2 * seconds(&request)))) {
                return Ok(Value::Array(head.bars.clone()));
            }
        } else if let Some(entry) = self.cache.entries.borrow().get(&key).filter(|entry| fresh(&entry.origin).unwrap_or(false) && entry.until > self.now) { return entry.value.clone(); }
        self.demands.borrow_mut().insert(key, request);
        Err(Failure::Failed(UNAVAILABLE.into()))
    }
}

impl MarketData for Reader<'_> {
    fn prices(&self, venue: &Venue, symbols: &[String]) -> Fetch<Vec<(String, Member<Dec>)>> {
        if venue.exchange_type != "Exchanges::Alpaca" { return Err(Failure::Failed(UNAVAILABLE.into())); }
        let symbols = self.symbols.as_deref().unwrap_or(symbols);
        let mut prices = vec![];
        let mut failed = None;
        for crypto in [false, true] {
            let mut names: Vec<&str> = symbols.iter().filter(|s| s.contains('/') == crypto).map(String::as_str).collect();
            names.sort_unstable(); names.dedup();
            if names.is_empty() { continue; }
            let path = if crypto { "/v1beta3/crypto/us/latest/trades" } else { "/v2/stocks/snapshots" };
            match self.get(request(path, vec![("symbols", names.join(","))])) {
                Err(error) => failed = Some(error),
                Ok(body) => {
                    let entries = if crypto { &body["trades"] } else { &body };
                    let Some(entries) = entries.as_object() else { failed = Some(Failure::Failed(UNAVAILABLE.into())); continue; };
                    for (code, item) in entries {
                        let price = if crypto { &item["p"] } else { &item["latestTrade"]["p"] };
                        // Owner ruling: absent trades carry no price. A reported numeric zero remains a price.
                        if !price.is_null() { prices.push((code.clone(), Dec::to_d(price))); }
                    }
                }
            }
        }
        match failed { Some(error) => { self.failed.set(true); Err(error) }, None => Ok(prices) }
    }
    fn candles(&self, venue: &Venue, ticker: &Ticker, since: At, timeframe: i64, restated: bool) -> Fetch<Vec<(At, Dec)>> {
        let result = (|| {
            if venue.exchange_type != "Exchanges::Alpaca" { return Err(Failure::Failed(UNAVAILABLE.into())); }
            let start = since.utc().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            let name = match timeframe { 60 => "1Min", 300 => "5Min", 900 => "15Min", 1800 => "30Min", 3600 => "1Hour", 14400 => "4Hour", 604800 => "1Week", 2592000 => "1Month", _ => "1Day" };
            let crypto = ticker.base_category.as_deref() == Some("Cryptocurrency");
            let (path, query) = if crypto {
                ("/v1beta3/crypto/us/bars".to_string(), vec![("start", start), ("symbols", ticker.ticker.clone()), ("timeframe", name.into())])
            } else {
                let code: String = form_urlencoded::byte_serialize(ticker.base.as_bytes()).collect();
                let mut query = vec![];
                if restated { query.push(("adjustment", "split".into())); }
                query.extend([("limit", "10000".into()), ("start", start), ("timeframe", name.into())]);
                (format!("/v2/stocks/{code}/bars"), query)
            };
            let body = self.get(request(&path, query))?;
            let bars = body.as_array().ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
            bars.iter().map(|bar| {
                let at = bar["t"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).and_then(|t| At::from_utc(t.to_utc()))
                    .ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
                Ok((at, bar_price(bar)?))
            }).collect()
        })();
        if result.is_err() { self.failed.set(true); }
        result
    }
    fn exchange_rates(&self) -> Fetch<Vec<(String, Member<Num>)>> { Err(Failure::Failed("Currency conversion unavailable".into())) }
    fn coin_price(&self, _: &str, _: &str) -> Fetch<Quoted> { Err(Failure::Failed("Currency conversion unavailable".into())) }
}

#[cfg(test)]
mod r4_cache_tests {
    use super::*;
    use crate::engine::model::{self,CredentialVersion};
    use crate::venue::http::HttpResponse;
    struct BoundWire { producer:CredentialVersion,requests:RefCell<Vec<HttpRequest>> }
    impl Transport for BoundWire {
        fn producer(&self)->Option<CredentialVersion>{Some(self.producer.clone())}
        async fn send(&self,r:&HttpRequest)->Result<HttpResponse,TransportError>{
            self.requests.borrow_mut().push(r.clone());
            Ok(HttpResponse{status:200,body:serde_json::json!({"bars":[{"t":"2026-01-01T00:00:00Z","o":100}]}).to_string()})
        }
    }
    fn db()->rusqlite::Connection {
        let c=rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys(id INTEGER PRIMARY KEY,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT); INSERT INTO api_keys(id,key,secret)VALUES(1,'ciphertext-A','ciphertext-secret');").unwrap();c
    }
    #[tokio::test]
    async fn r4_page_entries_and_heads_drop_after_rotation_on_every_read(){
        let c=db();let producer=model::credential_version_by_id(&c,1).unwrap().unwrap();
        let wire=BoundWire{producer,requests:RefCell::default()};
        let req=request("/v2/stocks/AAPL/bars",vec![("start","2025-01-01T00:00:00Z".into()),("timeframe","1Day".into())]);
        let now=chrono::DateTime::parse_from_rfc3339("2026-01-02T12:00:00Z").unwrap().timestamp();
        let mut cache=Cache::default();cache.fill(&wire,vec![req.clone()],now).await;
        assert!(Reader::new(&cache,now).with_current(&c).get(req.clone()).is_ok(),"R4 unchanged cache reads normally");
        c.execute("UPDATE api_keys SET key='ciphertext-B' WHERE id=1",[]).unwrap();
        assert!(Reader::new(&cache,now).with_current(&c).get(req.clone()).is_err(),"R4 A page value cannot be shown for B");
        assert!(cache.entries.borrow().is_empty()&&cache.heads.borrow().is_empty(),"R4 stale entries and candle heads must be physically dropped");
        // Returning to A cannot resurrect a dropped value.
        c.execute("UPDATE api_keys SET key='ciphertext-A' WHERE id=1",[]).unwrap();
        assert!(Reader::new(&cache,now).with_current(&c).get(req).is_err(),"R4 stale cache cannot resurrect");
    }
    #[tokio::test]
    async fn r4_candle_head_rotation_starts_full_b_series(){
        let c=db();let a=BoundWire{producer:model::credential_version_by_id(&c,1).unwrap().unwrap(),requests:RefCell::default()};
        let req=request("/v2/stocks/AAPL/bars",vec![("start","2025-01-01T00:00:00Z".into()),("timeframe","1Day".into())]);
        let now=chrono::DateTime::parse_from_rfc3339("2026-01-02T12:00:00Z").unwrap().timestamp();
        let mut cache=Cache::default();cache.fill(&a,vec![req.clone()],now).await;
        c.execute("UPDATE api_keys SET key='ciphertext-B' WHERE id=1",[]).unwrap();
        let b=BoundWire{producer:model::credential_version_by_id(&c,1).unwrap().unwrap(),requests:RefCell::default()};
        cache.fill(&b,vec![req.clone()],now+61).await;
        assert_eq!(b.requests.borrow()[0].query,req.query,"R4 B must fetch a full series rather than A's cached tail");
        assert!(Reader::new(&cache,now+61).with_current(&c).get(req).is_ok());
    }
}
