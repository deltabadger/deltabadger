//! Raw market responses are shared; bounded decimals are made and dropped only on the computing thread.
use super::at::At;
use super::db::Ticker;
use super::dec::Dec;
use super::market::{Failure, Fetch, MarketData, Member, Quoted, Venue};
use super::num::Num;
use crate::venue::http::{HttpRequest, Transport, TransportError};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;

pub const PRICE_TTL: i64 = 60;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 512;
const UNAVAILABLE: &str = "Market data unavailable";

#[derive(Clone)]
struct Entry { until: i64, value: Fetch<Value> }
#[derive(Clone)]
struct Head { until: i64, bars: Vec<Value> }
#[derive(Clone, Default)]
pub struct Cache { entries: BTreeMap<String, Entry>, heads: BTreeMap<String, Head> }

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
        if time.saturating_add(seconds(request)) <= now { out.push(bar.clone()); }
    }
    Ok(out)
}
fn rewritten(old: &Value, tail: &[Value], adjusted: bool) -> Fetch<bool> {
    let Some(overlap) = tail.iter().find(|bar| bar_time(bar) == bar_time(old)) else { return Ok(adjusted) };
    // Decimals are local to this call and never survive an await or move to another thread.
    let before = Dec::to_d(&old["o"])?;
    let after = Dec::to_d(&overlap["o"])?;
    if before.is_zero() || after.is_zero() { return Ok(false); }
    let ratio = (&after - &before)?.div(&before)?;
    Ok(ratio > Dec::to_d(&serde_json::json!("0.001"))? || ratio < Dec::to_d(&serde_json::json!("-0.001"))?)
}
impl Cache {
    pub async fn fill(&mut self, wire: &impl Transport, demands: Vec<HttpRequest>, now: i64) {
        for request in demands {
            let cache_key = key(&request);
            let is_bars = request.path.ends_with("/bars");
            let value = if is_bars { self.fill_candles(wire, &request, now).await } else { fetch(wire, &request).await };
            if self.entries.len() >= MAX_ENTRIES { self.entries.retain(|_, entry| entry.until > now); }
            // This process is a cache, not a second database. Eviction only causes a new read.
            if self.entries.len() >= MAX_ENTRIES { self.entries.clear(); }
            self.entries.insert(cache_key, Entry { until: now.saturating_add(PRICE_TTL), value });
        }
    }
    async fn fill_candles(&mut self, wire: &impl Transport, request: &HttpRequest, now: i64) -> Fetch<Value> {
        let cache_key = key(request);
        let old = self.heads.get(&cache_key).filter(|h| h.until > now).map(|h| h.bars.clone()).unwrap_or_default();
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
            if self.heads.len() >= MAX_ENTRIES { self.heads.clear(); }
            self.heads.insert(cache_key, Head { until: now.saturating_add(CANDLE_TTL), bars: bars.clone() });
        }
        Ok(Value::Array(bars))
    }
}

pub struct Reader<'a> { cache: &'a Cache, now: i64, demands: RefCell<BTreeMap<String, HttpRequest>> }
impl<'a> Reader<'a> {
    pub fn new(cache: &'a Cache, now: i64) -> Self { Self { cache, now, demands: RefCell::default() } }
    pub fn demands(&self) -> Vec<HttpRequest> { self.demands.borrow().values().cloned().collect() }
    fn get(&self, request: HttpRequest) -> Fetch<Value> {
        let key = key(&request);
        if request.path.ends_with("/bars") {
            if let Some(entry) = self.cache.entries.get(&key).filter(|entry| entry.until > self.now && entry.value.as_ref().map_or(true, |v| v.as_array().is_some_and(Vec::is_empty))) {
                return entry.value.clone();
            }
            if let Some(head) = self.cache.heads.get(&key).filter(|head| head.until > self.now && head.bars.last().and_then(bar_time).is_some_and(|t| self.now < t.saturating_add(2 * seconds(&request)))) {
                return Ok(Value::Array(head.bars.clone()));
            }
        } else if let Some(entry) = self.cache.entries.get(&key).filter(|entry| entry.until > self.now) { return entry.value.clone(); }
        self.demands.borrow_mut().insert(key, request);
        Err(Failure::Failed(UNAVAILABLE.into()))
    }
}

impl MarketData for Reader<'_> {
    fn prices(&self, venue: &Venue, symbols: &[String]) -> Fetch<Vec<(String, Member<Dec>)>> {
        if venue.exchange_type != "Exchanges::Alpaca" { return Err(Failure::Failed(UNAVAILABLE.into())); }
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
        match failed { Some(error) => Err(error), None => Ok(prices) }
    }
    fn candles(&self, venue: &Venue, ticker: &Ticker, since: At, timeframe: i64, restated: bool) -> Fetch<Vec<(At, Dec)>> {
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
            Ok((at, Dec::to_d(&bar["o"])?))
        }).collect()
    }
    fn exchange_rates(&self) -> Fetch<Vec<(String, Member<Num>)>> { Err(Failure::Failed("Currency conversion unavailable".into())) }
    fn coin_price(&self, _: &str, _: &str) -> Fetch<Quoted> { Err(Failure::Failed("Currency conversion unavailable".into())) }
}
