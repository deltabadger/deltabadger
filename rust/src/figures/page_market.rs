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
use std::sync::{Arc, Mutex};

pub const PRICE_TTL: i64 = 60;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 512;
const UNAVAILABLE: &str = "Market data unavailable";

/// A closed candle as the figures read it: its time (`bar_time`), the venue's own text of it (the next tail's `start`), and
/// its open exactly as the venue sent it, a number or a string, read by `Dec::to_d` as the answer itself is.
#[derive(Clone, Debug)]
struct Bar { at: i64, t: Box<str>, o: Value }
impl Bar {
    /// What it takes in memory, for the retained-bytes bound.
    fn size(&self) -> usize { std::mem::size_of::<Bar>() + self.t.len() + self.o.as_str().map_or(0, str::len) }
}
/// A kept answer: a price answer as `Reader::prices` reads it (`prices_only`), or a candle series.
#[derive(Clone)]
enum Answer { Json(Arc<Value>), Bars(Arc<[Bar]>) }
// Responses are shared, not copied: every page load clones the account's cache (`Service::begin`). A candle series is
// held once: its answer's entry and its head share it.
#[derive(Clone)]
struct Entry { until: i64, value: Fetch<Answer>, origin:Option<crate::engine::model::CredentialVersion>, size: usize }
#[derive(Clone)]
struct Head { until: i64, bars: Arc<[Bar]>, origin:Option<crate::engine::model::CredentialVersion>, size: usize }
/// What was rendered from one cache as it stands, by the caller's key: shared by every clone of it until a fill or a new
/// stamp changes it (both start an empty one).
type Rendered = Arc<Mutex<Vec<(String, Arc<Value>)>>>;
#[derive(Clone, Default)]
/// `bytes`: what the retained responses take, kept as they are inserted, replaced and removed.
pub struct Cache { entries: RefCell<BTreeMap<String, Entry>>, heads: RefCell<BTreeMap<String, Head>>, stamp: Option<At>, origin:Option<crate::engine::model::CredentialVersion>, rendered: Rendered, bytes: Cell<usize>,
    /// The install's held conversions, which kept figures are checked against as they are shown (`fx_current`).
    fx: Option<Arc<FxStore>> }

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
/// A response's body; decoding it is CPU work (`decode`).
async fn send(wire: &impl Transport, request: &HttpRequest) -> Fetch<String> {
    match wire.send_limited(request, MAX_BYTES).await {
        Ok(reply) if (200..300).contains(&reply.status) => Ok(reply.body),
        Ok(_) | Err(TransportError::Permanent(_)) => Err(Failure::Failed(UNAVAILABLE.into())),
        Err(_) => Err(Failure::Raised(UNAVAILABLE.into())),
    }
}
fn decode(body: &str) -> Fetch<Value> { crate::venue::http::decode_json(body).map_err(|_| Failure::Failed(UNAVAILABLE.into())) }
/// CPU work of a fill: decoding, the closed bars and their merge, and what they take. Off the runtime's thread, under the
/// figures' one permit for CPU work (`Service::compute`), which the worker holds until its closure returns: a fill that
/// stops waiting (its deadline) leaves the worker to finish, and the next CPU work waits for it.
async fn cpu<T: Send + 'static>(compute: &Arc<tokio::sync::Semaphore>, work: impl FnOnce() -> Fetch<T> + Send + 'static) -> Fetch<T> {
    let Ok(permit) = compute.clone().acquire_owned().await else { return Err(Failure::Failed(UNAVAILABLE.into())) };
    tokio::task::spawn_blocking(move || { let _permit = permit; work() }).await.unwrap_or_else(|_| Err(Failure::Failed(UNAVAILABLE.into())))
}
fn size(value: &Value) -> usize { value.to_string().len() }
/// A price answer cut to what `Reader::prices` reads of it: each symbol's price, as sent, under every symbol the answer
/// names, in its order. An answer whose symbols are not an object is kept whole, so that it fails there as it did. The
/// snapshot of an index's whole venue listing is thousands of symbols of five quotes each; a price is all a figure reads.
fn prices_only(path: &str, mut body: Value) -> Value {
    let crypto = path.ends_with("/latest/trades");
    if !crypto && !path.ends_with("/snapshots") { return body; }
    let symbols = if crypto { body.get_mut("trades") } else { Some(&mut body) };
    let Some(Value::Object(symbols)) = symbols else { return body };
    let kept: serde_json::Map<String, Value> = std::mem::take(symbols).into_iter().map(|(code, mut item)| {
        let price = if crypto { item.get_mut("p") } else { item.get_mut("latestTrade").and_then(|trade| trade.get_mut("p")) }.map_or(Value::Null, Value::take);
        (code, if crypto { serde_json::json!({ "p": price }) } else { serde_json::json!({ "latestTrade": { "p": price } }) })
    }).collect();
    if crypto { serde_json::json!({ "trades": kept }) } else { Value::Object(kept) }
}
/// One demand's answer, ready to insert: the entry's value and size, and the candle head it leaves, if any.
struct Answered { key: String, value: Fetch<Answer>, size: usize, head: Option<(Arc<[Bar]>, usize)> }
async fn answer(wire: &impl Transport, request: HttpRequest, old: Option<Arc<[Bar]>>, now: i64, compute: &Arc<tokio::sync::Semaphore>) -> Answered {
    let key = key(&request);
    if request.path.ends_with("/bars") {
        // The entry shares the head's series; its bytes are counted once, on the head.
        return match candles(wire, request, old, now, compute).await {
            Ok(Some((bars, size))) => Answered { key, value: Ok(Answer::Bars(bars.clone())), size: 0, head: Some((bars, size)) },
            Ok(None) => Answered { key, value: Ok(Answer::Bars(Arc::from(Vec::new()))), size: 0, head: None },
            Err(failure) => Answered { key, value: Err(failure), size: 0, head: None },
        };
    }
    let path = request.path.clone();
    let answered = match send(wire, &request).await {
        Ok(body) => cpu(compute, move || decode(&body).map(|value| { let value = prices_only(&path, value); (size(&value), value) })).await,
        Err(failure) => Err(failure),
    };
    match answered {
        Ok((size, value)) => Answered { key, value: Ok(Answer::Json(Arc::new(value))), size, head: None },
        Err(failure) => Answered { key, value: Err(failure), size: 0, head: None },
    }
}
/// A candle series: the tail since the last stored bar, merged onto it, or the whole series again when the venue rewrote
/// the overlapping bar (a split). With its size; `None` for a series with no closed bar, which leaves no head.
async fn candles(wire: &impl Transport, request: HttpRequest, old: Option<Arc<[Bar]>>, now: i64, compute: &Arc<tokio::sync::Semaphore>) -> Fetch<Option<(Arc<[Bar]>, usize)>> {
    let mut tail_request = request.clone();
    if let Some(last) = old.as_ref().and_then(|old| old.last()) {
        for (name, value) in &mut tail_request.query { if *name == "start" { *value = last.t.to_string(); } }
    }
    let tail = pages(wire, &tail_request, now, compute).await?;
    let merging = request.clone();
    let merged = cpu(compute, move || {
        let adjusted = merging.query.iter().any(|(k,v)| *k == "adjustment" && v == "split");
        let old: &[Bar] = old.as_deref().unwrap_or_default();
        if old.last().map(|last| rewritten(last, &tail, adjusted)).transpose()?.unwrap_or(false) { return Ok(None); }
        let last = old.last().map(|bar| bar.at);
        Ok(Some(old.iter().cloned().chain(tail.into_iter().filter(|bar| last.is_none_or(|last| bar.at > last))).collect::<Vec<_>>()))
    }).await?;
    let unsorted = match merged {
        Some(bars) => bars,
        None => pages(wire, &request, now, compute).await?,
    };
    cpu(compute, move || {
        let mut bars = unsorted;
        // Stable sort preserves the first occurrence, as Ruby's uniq-before-sort does.
        bars.sort_by_key(|bar| bar.at);
        bars.dedup_by_key(|bar| bar.at);
        let size = bars.iter().map(Bar::size).sum::<usize>();
        Ok((!bars.is_empty()).then(|| (Arc::from(bars), size)))
    }).await
}
/// The closed candles of every page of a bars answer. Alpaca answers a long range oldest first, a page at a time, each
/// naming the next (`next_page_token`), as Clients::Alpaca reads them; the token joins the query in its sorted place.
async fn pages(wire: &impl Transport, request: &HttpRequest, now: i64, compute: &Arc<tokio::sync::Semaphore>) -> Fetch<Vec<Bar>> {
    let mut out = vec![];
    let mut token: Option<String> = None;
    loop {
        let mut page = request.clone();
        if let Some(token) = token.take() {
            let at = page.query.iter().position(|(k,_)| *k > "page_token").unwrap_or(page.query.len());
            page.query.insert(at, ("page_token", token));
        }
        let body = send(wire, &page).await?;
        let reading = request.clone();
        let (bars, next) = cpu(compute, move || {
            let value = decode(&body)?;
            let next = value.get("next_page_token").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
            Ok((closed(value, &reading, now)?, next))
        }).await?;
        out.extend(bars);
        match next { Some(next) => token = Some(next), None => return Ok(out) }
    }
}
/// The answer's closed candles, each checked as a price, taken out of the answer: nothing of it is kept but these.
fn closed(mut body: Value, request: &HttpRequest, now: i64) -> Fetch<Vec<Bar>> {
    let bars = if request.path.starts_with("/v1beta3/") {
        let symbol = request.query.iter().find(|(k,_)| *k == "symbols").map_or("", |(_,v)| v);
        body.get_mut("bars").and_then(|bars| bars.get_mut(symbol))
    } else { body.get_mut("bars") };
    let Some(Value::Array(bars)) = bars.map(Value::take) else { return Err(Failure::Failed(UNAVAILABLE.into())); };
    let mut out = vec![];
    for mut bar in bars {
        let time = bar_time(&bar).ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
        if time.saturating_add(seconds(request)) <= now {
            // Validate even overlapping bars before a merge can discard them.
            bar_price(&bar)?;
            let t = bar["t"].as_str().unwrap_or_default().into(); // allow-swallow: bar_time read it as text above
            out.push(Bar { at: time, t, o: bar["o"].take() });
        }
    }
    Ok(out)
}
fn rewritten(old: &Bar, tail: &[Bar], adjusted: bool) -> Fetch<bool> {
    let Some(overlap) = tail.iter().find(|bar| bar.at == old.at) else { return Ok(adjusted) };
    // Decimals are local to this call and never survive an await or move to another thread.
    let before = Dec::to_d(&old.o)?;
    let after = Dec::to_d(&overlap.o)?;
    if before.is_zero() || after.is_zero() { return Ok(false); }
    let ratio = (&after - &before)?.div(&before)?;
    Ok(ratio > Dec::to_d(&serde_json::json!("0.001"))? || ratio < Dec::to_d(&serde_json::json!("-0.001"))?)
}
impl Cache {
    /// Bound the retained wire representation, independently of the decimal budget.
    pub fn bytes(&self) -> usize { self.bytes.get() }
    fn add(&self, size: usize) { self.bytes.set(self.bytes.get().saturating_add(size)); }
    fn sub(&self, size: usize) { self.bytes.set(self.bytes.get().saturating_sub(size)); }
    fn remove_entry(&self, key: &str) { if let Some(entry) = self.entries.borrow_mut().remove(key) { self.sub(entry.size); } }
    fn remove_head(&self, key: &str) { if let Some(head) = self.heads.borrow_mut().remove(key) { self.sub(head.size); } }
    fn clear_entries(&self) { let gone: usize = self.entries.borrow().values().map(|e| e.size).sum(); self.entries.borrow_mut().clear(); self.sub(gone); }
    fn clear_heads(&self) { let gone: usize = self.heads.borrow().values().map(|h| h.size).sum(); self.heads.borrow_mut().clear(); self.sub(gone); }
    pub fn producer(&self)->&Option<crate::engine::model::CredentialVersion> { &self.origin }
    pub fn stamp(&self) -> Option<At> { self.stamp }
    pub fn with_fx(mut self, fx: Arc<FxStore>) -> Self { self.fx = Some(fx); self }
    /// Whether the conversions kept figures read (`deps`) still hold at `now` for the provider configured now. Figures that
    /// read none hold without a store.
    pub fn fx_current(&self, deps: &FxDeps, identity: &str, now: i64) -> bool {
        deps.pairs.is_empty() || self.fx.as_ref().is_some_and(|fx| fx.current(deps, identity, now))
    }
    pub fn set_stamp(&mut self, at: At) { self.stamp = Some(at); self.rendered = Rendered::default(); }
    /// What `keep` kept under `key` for this cache.
    pub fn kept(&self, key: &str) -> Option<Arc<Value>> {
        self.rendered.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }
    /// Keeps what was rendered from this cache under `key`, for every clone of it. A handful of keys (a locale and a
    /// path prefix each, with or without a form token) per cache; past that the oldest goes.
    pub fn keep(&self, key: String, value: Arc<Value>) {
        let mut rendered = self.rendered.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        rendered.retain(|(k, _)| *k != key);
        if rendered.len() >= 8 { rendered.remove(0); }
        rendered.push((key, value));
    }
    /// Every demand answered, eight requests at a time, with no CPU work on the runtime's thread (`cpu`).
    pub async fn fill_bounded(&mut self, wire: &impl Transport, demands: Vec<HttpRequest>, now: i64, compute: &Arc<tokio::sync::Semaphore>) {
        use futures_util::stream::{self, StreamExt};
        self.rendered = Rendered::default();
        self.origin=wire.producer();
        let producer = wire.producer();
        // What each candle demand starts from: its stored head, unless another key's head or an expired one.
        let jobs: Vec<(HttpRequest, Option<Arc<[Bar]>>)> = demands.into_iter().map(|request| {
            let cache_key = key(&request);
            if self.heads.borrow().get(&cache_key).is_some_and(|h|!crate::engine::model::current_for(h.origin.as_ref(),wire.producer().as_ref()).is_fresh()) { self.remove_head(&cache_key); }
            let old = request.path.ends_with("/bars").then(|| self.heads.borrow().get(&cache_key).filter(|h| h.until > now).map(|h| h.bars.clone())).flatten();
            (request, old)
        }).collect();
        let mut answers = stream::iter(jobs).map(|(request, old)| answer(wire, request, old, now, compute)).buffer_unordered(8);
        while let Some(answer) = answers.next().await {
            if let Some((bars, size)) = answer.head {
                if self.heads.borrow().len() >= MAX_ENTRIES { self.clear_heads(); }
                self.remove_head(&answer.key);
                self.heads.borrow_mut().insert(answer.key.clone(), Head { until: now.saturating_add(CANDLE_TTL), bars, origin: producer.clone(), size });
                self.add(size);
            }
            if self.entries.borrow().len() >= MAX_ENTRIES {
                let expired: Vec<String> = self.entries.borrow().iter().filter(|(_, entry)| entry.until <= now).map(|(k, _)| k.clone()).collect();
                for key in expired { self.remove_entry(&key); }
            }
            // This process is a cache, not a second database. Eviction only causes a new read.
            if self.entries.borrow().len() >= MAX_ENTRIES { self.clear_entries(); }
            self.remove_entry(&answer.key);
            self.entries.borrow_mut().insert(answer.key, Entry { until: now.saturating_add(PRICE_TTL), value: answer.value, origin: producer.clone(), size: answer.size });
            self.add(answer.size);
            if self.bytes() > 32 * 1024 * 1024 { self.clear_entries(); self.clear_heads(); }
        }
    }
    /// `fill_bounded` under a permit of its own: for a caller with no figures service (a test, a harness).
    pub async fn fill(&mut self, wire: &impl Transport, demands: Vec<HttpRequest>, now: i64) {
        self.fill_bounded(wire, demands, now, &Arc::new(tokio::sync::Semaphore::new(1))).await
    }
}

pub struct Reader<'a> { cache: &'a Cache, now: i64, demands: RefCell<BTreeMap<String, HttpRequest>>, symbols: Option<Vec<String>>, failed: Cell<bool>, current:Option<&'a rusqlite::Connection>,
    fx: Option<FxSnapshot>, fx_demands: RefCell<std::collections::BTreeSet<(String, String)>>, fx_used: RefCell<BTreeMap<String, u64>> }
impl<'a> Reader<'a> {
    pub fn new(cache: &'a Cache, now: i64) -> Self { Self { cache, now, demands: RefCell::default(), symbols: None, failed: Cell::new(false), current:None, fx: None, fx_demands: RefCell::default(), fx_used: RefCell::default() } }
    /// Conversions answered from the held ones (`MarketData::held_rate`); one not held is a demand (`fx_demands`).
    pub fn with_fx(mut self, fx: FxSnapshot) -> Self { self.fx = Some(fx); self }
    /// The conversions this reader was asked for and does not hold: (from, to).
    pub fn fx_demands(&self) -> Vec<(String, String)> { self.fx_demands.borrow().iter().cloned().collect() }
    /// The held conversions it answered with.
    pub fn fx_deps(&self) -> FxDeps { FxDeps { identity: self.fx.as_ref().map(|fx| fx.identity.clone()).unwrap_or_default(), pairs: self.fx_used.borrow().clone() } }
    pub fn with_current(mut self,c:&'a rusqlite::Connection)->Self { self.current=Some(c);self }
    pub fn failed(&self) -> bool { self.failed.get() }
    pub fn with_symbols(mut self, symbols: Vec<String>) -> Self { self.symbols = Some(symbols); self }
    pub fn demands(&self) -> Vec<HttpRequest> { self.demands.borrow().values().cloned().collect() }
    fn get(&self, request: HttpRequest) -> Fetch<Answer> {
        let fresh=|origin:&Option<crate::engine::model::CredentialVersion>|->Result<bool,Failure> {
            let Some(c)=self.current else { return Ok(true) }; // The unbound numerical harness reads no stored credentials.
            if !crate::engine::model::current_for(origin.as_ref(),self.cache.origin.as_ref()).is_fresh(){return Ok(false)}
            match origin {Some(origin)=>crate::engine::model::credential_is_current(c,origin).map_err(|_|Failure::Failed(UNAVAILABLE.into())),None=>Ok(false)}
        };
        let key = key(&request);
        if let Some(entry)=self.cache.entries.borrow().get(&key){fresh(&entry.origin)?;}
        if let Some(head)=self.cache.heads.borrow().get(&key){fresh(&head.origin)?;}
        if self.cache.entries.borrow().get(&key).is_some_and(|e|!fresh(&e.origin).unwrap_or(false)){let gone=self.cache.entries.borrow_mut().remove(&key);if let Some(e)=gone{self.cache.sub(e.size);}}
        if self.cache.heads.borrow().get(&key).is_some_and(|h|!fresh(&h.origin).unwrap_or(false)){let gone=self.cache.heads.borrow_mut().remove(&key);if let Some(h)=gone{self.cache.sub(h.size);}}
        if request.path.ends_with("/bars") {
            if let Some(entry) = self.cache.entries.borrow().get(&key).filter(|entry| fresh(&entry.origin).unwrap_or(false) && entry.until > self.now) {
                return entry.value.clone();
            }
            if let Some(head) = self.cache.heads.borrow().get(&key).filter(|head| fresh(&head.origin).unwrap_or(false) && head.until > self.now && head.bars.last().is_some_and(|bar| self.now < bar.at.saturating_add(2 * seconds(&request)))) {
                return Ok(Answer::Bars(head.bars.clone()));
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
                Ok(Answer::Bars(_)) => failed = Some(Failure::Failed(UNAVAILABLE.into())),
                Ok(Answer::Json(body)) => {
                    let entries = if crypto { &body["trades"] } else { &*body };
                    let Some(entries) = entries.as_object() else { failed = Some(Failure::Failed(UNAVAILABLE.into())); continue; };
                    for (code, item) in entries {
                        let price = if crypto { &item["p"] } else { &item["latestTrade"]["p"] };
                        // Owner ruling: absent trades carry no price. A reported numeric zero remains a price.
                        if !price.is_null() { prices.push((code.clone(), Dec::venue_number(price))); }
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
                ("/v1beta3/crypto/us/bars".to_string(), vec![("limit", "10000".into()), ("start", start), ("symbols", ticker.ticker.clone()), ("timeframe", name.into())])
            } else {
                let code: String = form_urlencoded::byte_serialize(ticker.base.as_bytes()).collect();
                let mut query = vec![];
                if restated { query.push(("adjustment", "split".into())); }
                query.extend([("limit", "10000".into()), ("start", start), ("timeframe", name.into())]);
                (format!("/v2/stocks/{code}/bars"), query)
            };
            let Answer::Bars(bars) = self.get(request(&path, query))? else { return Err(Failure::Failed(UNAVAILABLE.into())) };
            bars.iter().map(|bar| {
                let at = chrono::DateTime::parse_from_rfc3339(&bar.t).ok().and_then(|t| At::from_utc(t.to_utc()))
                    .ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
                Ok((at, Dec::to_d(&bar.o)?))
            }).collect()
        })();
        if result.is_err() { self.failed.set(true); }
        result
    }
    // Raw provider answers never reach a page pass: every conversion is a held one (`held_rate`), fetched by the fill.
    fn exchange_rates(&self) -> Fetch<Vec<(String, Member<Num>)>> { Err(Failure::Failed("Currency conversion unavailable".into())) }
    fn coin_price(&self, _: &str, _: &str) -> Fetch<Quoted> { Err(Failure::Failed("Currency conversion unavailable".into())) }
    fn held_rate(&self, from: &str, to: &str) -> Option<Fetch<Quoted>> {
        let fx = self.fx.as_ref()?;
        let key = (from.to_string(), to.to_string());
        match fx.held.get(&key) {
            Some(held) => {
                self.fx_used.borrow_mut().insert(pair_name(from, to), held.generation);
                Some(held.value.clone().and_then(|rate| rate.quoted()))
            }
            None => {
                // Not held, or no longer: the fill asks the provider, and this pass is not kept.
                self.fx_demands.borrow_mut().insert(key);
                self.failed.set(true);
                Some(Err(Failure::Failed(UNAVAILABLE.into())))
            }
        }
    }
}

/// A conversion's answer as held: no decimal crosses a thread (`service`), so a rate is kept as the value it was made from.
#[derive(Clone, Debug, PartialEq)]
pub enum HeldRate { Float(f64), Decimal(String) }
impl HeldRate {
    pub fn of(quoted: &Quoted) -> Fetch<HeldRate> {
        match quoted {
            Quoted::Num(Num::Float(f)) => Ok(HeldRate::Float(*f)),
            Quoted::Num(n) => Ok(HeldRate::Decimal(n.to_d()?.to_s_f())),
            Quoted::Text(text) => Ok(HeldRate::Decimal(Dec::strict(text)?.to_s_f())),
        }
    }
    fn quoted(&self) -> Fetch<Quoted> {
        Ok(match self { HeldRate::Float(f) => Quoted::Num(Num::Float(*f)), HeldRate::Decimal(text) => Quoted::Num(Num::Dec(Dec::strict(text)?)) })
    }
}
/// One held conversion: its answer, until when (Unix seconds), and which answer it is (`generation`, never reused).
#[derive(Clone, Debug)]
pub struct Held { pub value: Fetch<HeldRate>, pub until: i64, pub generation: u64 }
fn pair_name(from: &str, to: &str) -> String { format!("{from}->{to}") }
/// Utilities::Currency's Rails.cache entries (`exchange_rate_FROM_to_TO`), per market-data provider (`identity`: its
/// fingerprint, or "none"): every conversion the pages and the tracker made, success or failure, until it expires.
/// In-process: it starts empty when the server starts.
#[derive(Default)]
pub struct FxStore { held: Mutex<BTreeMap<(String, String, String), Held>>, generation: std::sync::atomic::AtomicU64 }
/// What one pass may read of the store: the unexpired conversions of one provider, as they stood when it began.
#[derive(Clone, Default)]
pub struct FxSnapshot { identity: String, held: BTreeMap<(String, String), Held> }
/// The conversions a rendering read: the provider, and each pair's generation. It holds while each is still the one held
/// and unexpired, for the provider now configured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FxDeps { pub identity: String, pub pairs: BTreeMap<String, u64> }
impl FxDeps {
    pub fn to_json(&self) -> Value { serde_json::json!({"identity": self.identity, "pairs": self.pairs}) }
    /// None: not a record this build wrote, which holds nothing.
    pub fn from_json(v: &Value) -> Option<FxDeps> {
        let pairs = v["pairs"].as_object()?.iter().map(|(k, g)| g.as_u64().map(|g| (k.clone(), g))).collect::<Option<_>>()?;
        Some(FxDeps { identity: v["identity"].as_str()?.to_string(), pairs })
    }
}
impl FxStore {
    const CAPACITY: usize = 256;
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String, String), Held>> { self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }
    pub fn get(&self, identity: &str, from: &str, to: &str, now: i64) -> Option<Held> {
        self.lock().get(&(identity.to_string(), from.to_string(), to.to_string())).filter(|held| held.until > now).cloned()
    }
    pub fn put(&self, identity: &str, from: &str, to: &str, value: Fetch<HeldRate>, until: i64, now: i64) {
        let generation = self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst).wrapping_add(1);
        let mut held = self.lock();
        if held.len() >= Self::CAPACITY { held.retain(|_, h| h.until > now); }
        if held.len() >= Self::CAPACITY { held.clear(); }
        held.insert((identity.to_string(), from.to_string(), to.to_string()), Held { value, until, generation });
    }
    pub fn snapshot(&self, identity: &str, now: i64) -> FxSnapshot {
        let held = self.lock().iter().filter(|((id, _, _), h)| id == identity && h.until > now).map(|((_, from, to), h)| ((from.clone(), to.clone()), h.clone())).collect();
        FxSnapshot { identity: identity.to_string(), held }
    }
    /// Whether what a rendering read still holds at `now` for the provider configured now (`identity`).
    pub fn current(&self, deps: &FxDeps, identity: &str, now: i64) -> bool {
        if deps.pairs.is_empty() { return true; }
        if deps.identity != identity { return false; }
        let held = self.lock();
        deps.pairs.iter().all(|(name, generation)| {
            let Some((from, to)) = name.split_once("->") else { return false };
            held.get(&(identity.to_string(), from.to_string(), to.to_string())).is_some_and(|h| h.generation == *generation && h.until > now)
        })
    }
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
    /// What the retained responses take, recounted from them, as `bytes` was once computed on every call.
    fn recount(cache:&Cache)->usize {
        cache.entries.borrow().values().map(|e| match &e.value { Ok(Answer::Json(v))=>v.to_string().len(), _=>0 }).sum::<usize>()
            + cache.heads.borrow().values().map(|h|h.bars.iter().map(Bar::size).sum::<usize>()).sum::<usize>()
    }
    #[tokio::test]
    async fn bytes_total_matches_a_full_recount(){
        let c=db();let producer=model::credential_version_by_id(&c,1).unwrap().unwrap();
        let wire=BoundWire{producer,requests:RefCell::default()};
        let now=chrono::DateTime::parse_from_rfc3339("2026-01-02T12:00:00Z").unwrap().timestamp();
        let bars=|symbol:&str|request(&format!("/v2/stocks/{symbol}/bars"),vec![("start","2025-01-01T00:00:00Z".into()),("timeframe","1Day".into())]);
        let mut cache=Cache::default();
        // New entries and heads, then the same ones replaced, then a price that fails.
        cache.fill(&wire,vec![bars("AAA"),bars("BBB")],now).await;
        assert_eq!(cache.bytes(),recount(&cache));
        assert!(cache.bytes()>0);
        cache.fill(&wire,vec![bars("AAA"),request("/v2/stocks/snapshots",vec![("symbols","AAA".into())])],now+61).await;
        assert_eq!(cache.bytes(),recount(&cache));
        // Entries dropped by a credential rotation.
        c.execute("UPDATE api_keys SET key='ciphertext-B' WHERE id=1",[]).unwrap();
        assert!(Reader::new(&cache,now+61).with_current(&c).get(bars("AAA")).is_err());
        assert_eq!(cache.bytes(),recount(&cache));
        // Expired entries swept, past the entry bound.
        c.execute("UPDATE api_keys SET key='ciphertext-A' WHERE id=1",[]).unwrap();
        let many:Vec<HttpRequest>=(0..MAX_ENTRIES+3).map(|i|bars(&format!("S{i}"))).collect();
        cache.fill(&wire,many,now+200).await;
        assert_eq!(cache.bytes(),recount(&cache));
    }
    /// A series is held once: its answer's entry and the head the next tail starts from are one allocation, and a merged
    /// tail keeps each bar's open as the venue sent it.
    #[tokio::test]
    async fn a_series_is_held_once_and_keeps_its_opens(){
        struct Tail(RefCell<u8>,CredentialVersion);
        impl Transport for Tail {
            fn producer(&self)->Option<CredentialVersion>{Some(self.1.clone())}
            async fn send(&self,_:&HttpRequest)->Result<HttpResponse,TransportError>{
                *self.0.borrow_mut()+=1;
                let bars=if *self.0.borrow()==1 { serde_json::json!([{"t":"2026-01-01T00:00:00Z","o":"10.50"},{"t":"2026-01-02T00:00:00Z","o":11}]) }
                    else { serde_json::json!([{"t":"2026-01-02T00:00:00Z","o":11},{"t":"2026-01-03T00:00:00Z","o":"12.000"}]) };
                Ok(HttpResponse{status:200,body:serde_json::json!({"bars":bars}).to_string()})
            }
        }
        let req=request("/v2/stocks/AAPL/bars",vec![("start","2025-01-01T00:00:00Z".into()),("timeframe","1Day".into())]);
        let key=key(&req);let day=86_400;let start=chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().timestamp();
        let wire=Tail(RefCell::new(0),model::credential_version_by_id(&db(),1).unwrap().unwrap());let mut cache=Cache::default();
        let shared=|cache:&Cache|match &cache.entries.borrow()[&key].value { Ok(Answer::Bars(bars))=>Arc::ptr_eq(bars,&cache.heads.borrow()[&key].bars), _=>false };
        cache.fill(&wire,vec![req.clone()],start+3*day).await;
        assert!(shared(&cache),"the entry and the head hold one series");
        cache.fill(&wire,vec![req.clone()],start+4*day).await;
        assert!(shared(&cache));
        let opens:Vec<Value>=cache.heads.borrow()[&key].bars.iter().map(|bar|bar.o.clone()).collect();
        assert_eq!(opens,vec![serde_json::json!("10.50"),serde_json::json!(11),serde_json::json!("12.000")]);
        assert_eq!(cache.bytes(),recount(&cache));
    }
    /// Alpaca answers a long range a page at a time, oldest first; every page is read, as Clients::Alpaca reads them.
    #[tokio::test]
    async fn every_page_of_a_series_is_read(){
        struct Paged(RefCell<Vec<HttpRequest>>,CredentialVersion);
        impl Transport for Paged {
            fn producer(&self)->Option<CredentialVersion>{Some(self.1.clone())}
            async fn send(&self,r:&HttpRequest)->Result<HttpResponse,TransportError>{
                self.0.borrow_mut().push(r.clone());
                let body=if r.query.iter().any(|(k,_)|*k=="page_token") { serde_json::json!({"bars":{"BTC/USD":[{"t":"2026-01-03T00:00:00Z","o":3}]},"next_page_token":null}) }
                    else { serde_json::json!({"bars":{"BTC/USD":[{"t":"2026-01-01T00:00:00Z","o":1},{"t":"2026-01-02T00:00:00Z","o":2}]},"next_page_token":"p2"}) };
                Ok(HttpResponse{status:200,body:body.to_string()})
            }
        }
        let req=request("/v1beta3/crypto/us/bars",vec![("limit","10000".into()),("start","2025-01-01T00:00:00Z".into()),("symbols","BTC/USD".into()),("timeframe","1Day".into())]);
        let now=chrono::DateTime::parse_from_rfc3339("2026-01-05T00:00:00Z").unwrap().timestamp();
        let wire=Paged(RefCell::default(),model::credential_version_by_id(&db(),1).unwrap().unwrap());
        let mut cache=Cache::default();cache.fill(&wire,vec![req.clone()],now).await;
        let opens:Vec<Value>=cache.heads.borrow()[&key(&req)].bars.iter().map(|bar|bar.o.clone()).collect();
        assert_eq!(opens,vec![serde_json::json!(1),serde_json::json!(2),serde_json::json!(3)]);
        let second:Vec<&str>=wire.0.borrow()[1].query.iter().map(|(k,_)|*k).collect();
        assert_eq!(second,vec!["limit","page_token","start","symbols","timeframe"],"the token joins the query in its sorted place");
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
