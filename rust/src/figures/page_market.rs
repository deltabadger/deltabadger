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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const PRICE_TTL: i64 = 60;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 512;
/// What the cache keeps in all: entries and heads together (the bound every answer is admitted under, `Cache::admit`).
pub const CACHE_BYTES: usize = 32 * 1024 * 1024;
/// One candle series as it accumulates, page by page: past it the series is a settled failure (a venue paging without end).
pub const SERIES_BYTES: usize = MAX_BYTES;
/// Every series in flight in one fill, together.
pub const FILL_BYTES: usize = CACHE_BYTES;
/// Exchanges::Kucoin::MAX_CANDLE_PAGES: the most windows one KuCoin series reads.
pub const MAX_CANDLE_PAGES: usize = 1000;
const UNAVAILABLE: &str = "Market data unavailable";
const KUCOIN_CANDLES: &str = "/api/v1/market/candles";

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
pub struct Cache { entries: RefCell<BTreeMap<String, Entry>>, heads: RefCell<BTreeMap<String, Head>>,
    /// Answers the cache had no room for (`admit`), by key, until when: settled failures for every pass on this cache until
    /// the next fill asks again (`clear_refusals`). Bounded by MAX_ENTRIES.
    refused: RefCell<BTreeMap<String, i64>>, stamp: Option<At>, origin:Option<Producer>, rendered: Rendered, bytes: Cell<usize>,
    /// The install's held conversions, which kept figures are checked against as they are shown (`fx_current`).
    fx: Option<Arc<FxStore>> }

/// What an account's figures are made under (figures-specific provenance): its Alpaca key's version, or no usable Alpaca
/// key. `NoAlpaca` names the account and the version of the key row it has that cannot be read (none: no key at all), so
/// that adding, replacing or removing a key changes it, and checking it needs no decryption (`is_current`).
#[derive(Clone, Debug)]
pub enum Producer { Alpaca(crate::engine::model::CredentialVersion), NoAlpaca { user: i64, key: Option<crate::engine::model::CredentialVersion> } }
/// Two key versions made the same answers: both absent, or the same version.
fn same_key(a: Option<&crate::engine::model::CredentialVersion>, b: Option<&crate::engine::model::CredentialVersion>) -> bool {
    match (a, b) { (None, None) => true, (Some(a), Some(b)) => crate::engine::model::current_for(Some(a), Some(b)).is_fresh(), _ => false }
}
impl Producer {
    /// The Alpaca key market data is read with, if any.
    pub fn alpaca(&self) -> Option<&crate::engine::model::CredentialVersion> { match self { Producer::Alpaca(v) => Some(v), Producer::NoAlpaca { .. } => None } }
    /// The same producer: one key version, or the same account without a usable key in the same state.
    pub fn same(&self, other: &Producer) -> bool {
        match (self, other) {
            (Producer::Alpaca(a), Producer::Alpaca(b)) => same_key(Some(a), Some(b)),
            (Producer::NoAlpaca { user: a, key: x }, Producer::NoAlpaca { user: b, key: y }) => a == b && same_key(x.as_ref(), y.as_ref()),
            _ => false,
        }
    }
    /// Whether the account is still as this producer found it, read in the caller's transaction: its key unchanged, or still
    /// no Alpaca key (or the same unreadable one).
    pub fn is_current(&self, c: &rusqlite::Connection) -> Result<bool, crate::engine::EngineError> {
        match self {
            Producer::Alpaca(version) => crate::engine::model::credential_is_current(c, version),
            Producer::NoAlpaca { user, key } => {
                let id: Option<i64> = rusqlite::OptionalExtension::optional(c.query_row(ALPACA_KEY, [user], |r| r.get(0)))?;
                Ok(match (id, key) { (None, None) => true, (Some(id), Some(version)) => id == version.id && crate::engine::model::credential_is_current(c, version)?, _ => false })
            }
        }
    }
}
/// The account's Alpaca trading key, as the figures pick it (`loading::read_info`).
pub const ALPACA_KEY: &str = "SELECT k.id FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0 AND e.type='Exchanges::Alpaca' ORDER BY k.id LIMIT 1";

const ALPACA: &str = "https://data.alpaca.markets";
/// A request's cache key: its path and query, behind its host for any venue but Alpaca's market data (whose keys predate
/// other venues and stay as they were).
fn key(request: &HttpRequest) -> String {
    let query = format!("{}?{}", request.path, request.query.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
    if request.base == ALPACA { query } else { format!("{} {query}", request.base) }
}
fn request(path: &str, query: Vec<(&'static str, String)>) -> HttpRequest {
    HttpRequest { method: "GET", base: ALPACA.into(), path: path.into(), query, body: None, not_after: None }
}

pub const CANDLE_TTL: i64 = 30 * 86_400;
/// A series' timeframe in seconds, from Alpaca's `timeframe` or KuCoin's `type`.
fn seconds(request: &HttpRequest) -> i64 {
    match request.query.iter().find(|(k,_)| *k == "timeframe" || *k == "type").map(|(_,v)| v.as_str()) {
        Some("1Min" | "1min") => 60, Some("5Min" | "5min") => 300, Some("15Min" | "15min") => 900, Some("30Min" | "30min") => 1800,
        Some("1Hour" | "1hour") => 3600, Some("4Hour" | "4hour") => 14400, Some("1Week" | "1week") => 604800, Some("1Month") => 2592000, _ => 86400,
    }
}
/// Whether a demand is a candle series (kept as a head) rather than one answer: Alpaca's bars, or a KuCoin series. A KuCoin
/// series is never sent as it is: its key names the series (symbol, since, type), and its pages are derived from it
/// (`kucoin_pages`), so the head is found again by the next fill whatever `now` is.
fn is_series(request: &HttpRequest) -> bool { request.path.ends_with("/bars") || request.path == KUCOIN_CANDLES }
fn param<'r>(request: &'r HttpRequest, name: &str) -> &'r str { request.query.iter().find(|(k,_)| *k == name).map_or("", |(_,v)| v.as_str()) }
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
    // KuCoin's allTickers: every row's symbol and last price, the rest of each row dropped (`kucoin::all_tickers` reads them).
    if path == ALL_TICKERS {
        let Some(Value::Array(rows)) = body.get_mut("data").and_then(|data| data.get_mut("ticker")) else { return body };
        let kept: Vec<Value> = std::mem::take(rows).into_iter().map(|mut row| serde_json::json!({ "symbol": row.get_mut("symbol").map_or(Value::Null, Value::take), "last": row.get_mut("last").map_or(Value::Null, Value::take) })).collect();
        return serde_json::json!({ "data": { "ticker": kept } });
    }
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
/// `held`: a series' bytes stay counted against the fill's bound (`SeriesBytes`) until the fill admits or drops it: the field is only ever dropped, at the end of its admission.
struct Answered<'a> { key: String, value: Fetch<Answer>, size: usize, head: Option<(Arc<[Bar]>, usize)>, _held: Option<SeriesBytes<'a>> }
async fn answer<'a>(wire: &impl Transport, request: HttpRequest, old: Option<Arc<[Bar]>>, now: i64, compute: &Arc<tokio::sync::Semaphore>, in_flight: &'a AtomicUsize) -> Answered<'a> {
    let key = key(&request);
    if is_series(&request) {
        let mut held = SeriesBytes { series: 0, in_flight };
        // The entry shares the head's series; its bytes are counted once, on the head.
        let answered = candles(wire, request, old, now, compute, &mut held).await;
        let held = Some(held);
        return match answered {
            Ok(Some((bars, size))) => Answered { key, value: Ok(Answer::Bars(bars.clone())), size: 0, head: Some((bars, size)), _held: held },
            Ok(None) => Answered { key, value: Ok(Answer::Bars(Arc::from(Vec::new()))), size: 0, head: None, _held: held },
            Err(failure) => Answered { key, value: Err(failure), size: 0, head: None, _held: held },
        };
    }
    let path = request.path.clone();
    let answered = match send(wire, &request).await {
        Ok(body) => cpu(compute, move || decode(&body).map(|value| { let value = prices_only(&path, value); (size(&value), value) })).await,
        Err(failure) => Err(failure),
    };
    match answered {
        Ok((size, value)) => Answered { key, value: Ok(Answer::Json(Arc::new(value))), size, head: None, _held: None },
        Err(failure) => Answered { key, value: Err(failure), size: 0, head: None, _held: None },
    }
}
/// A candle series (CandleSeriesCache#refresh): the tail from the last stored bar (AT it: the overlap tells a rewritten
/// history from a continuing one), merged onto it, or the whole series again when the venue rewrote the overlapping bar.
/// With its size; `None` for a series with no closed bar, which leaves no head. A merged series past SERIES_BYTES is a
/// settled failure, and the stored head is left as it was.
async fn candles(wire: &impl Transport, request: HttpRequest, old: Option<Arc<[Bar]>>, now: i64, compute: &Arc<tokio::sync::Semaphore>, held: &mut SeriesBytes<'_>) -> Fetch<Option<(Arc<[Bar]>, usize)>> {
    let tail = pages(wire, &request, old.as_ref().and_then(|old| old.last()), now, compute, held).await?;
    // The merge copies the stored series: its bytes count too, for as long as the copy lives.
    if let Some(old) = &old { held.take(old.iter().map(Bar::size).sum())?; }
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
        None => pages(wire, &request, None, now, compute, held).await?,
    };
    cpu(compute, move || {
        let mut bars = unsorted;
        // Stable sort preserves the first occurrence, as Ruby's uniq-before-sort does.
        bars.sort_by_key(|bar| bar.at);
        bars.dedup_by_key(|bar| bar.at);
        let size = bars.iter().map(Bar::size).sum::<usize>();
        if size > SERIES_BYTES { return Err(Failure::Failed(UNAVAILABLE.into())); }
        Ok((!bars.is_empty()).then(|| (Arc::from(bars), size)))
    }).await
}
/// The bytes one series holds from its first page until the fill admits or drops it: counted against SERIES_BYTES for the
/// series and FILL_BYTES for every series of the fill in flight, as each page is decoded and as the merge copies the stored
/// series; given back to the fill's count when the reservation is dropped (`Answered::held`).
struct SeriesBytes<'a> { series: usize, in_flight: &'a AtomicUsize }
impl SeriesBytes<'_> {
    fn take(&mut self, size: usize) -> Fetch<()> {
        self.series = self.series.saturating_add(size);
        let fill = self.in_flight.fetch_add(size, Ordering::SeqCst).saturating_add(size);
        if self.series > SERIES_BYTES || fill > FILL_BYTES { return Err(Failure::Failed(UNAVAILABLE.into())); }
        Ok(())
    }
}
impl Drop for SeriesBytes<'_> { fn drop(&mut self) { self.in_flight.fetch_sub(self.series, Ordering::SeqCst); } }
/// The closed candles of every page of a series, from its start or from `tail` (the last stored bar), under the bounds
/// (`SeriesBytes`): a series that crosses one is a settled failure, and nothing of it is kept.
async fn pages(wire: &impl Transport, request: &HttpRequest, tail: Option<&Bar>, now: i64, compute: &Arc<tokio::sync::Semaphore>, held: &mut SeriesBytes<'_>) -> Fetch<Vec<Bar>> {
    if request.path == KUCOIN_CANDLES { return kucoin_pages(wire, request, tail.map(|bar| bar.at), now, compute, held).await; }
    // Alpaca answers a long range oldest first, a page at a time, each naming the next (`next_page_token`), as
    // Clients::Alpaca reads them; the token joins the query in its sorted place. A tail starts at the last bar's own text.
    let mut request = request.clone();
    if let Some(last) = tail {
        for (name, value) in &mut request.query { if *name == "start" { *value = last.t.to_string(); } }
    }
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
        held.take(bars.iter().map(Bar::size).sum())?;
        out.extend(bars);
        match next { Some(next) => token = Some(next), None => return Ok(out) }
    }
}
/// Exchanges::Kucoin#get_candles: windows of `startAt` .. `endAt` = min(start + 1500 timeframes, now), in seconds. A page
/// with candles moves the start to its last candle + 1 s, an empty one to `endAt` + 1 s (windows before the listing are
/// skipped, not the end); the walk ends at the window that reaches now, or after MAX_CANDLE_PAGES windows, with what it
/// read. Any failed window fails the series.
// ponytail: Rails compares `end_at >= now` with the clock's fraction; here both are whole seconds, so a window ending within
// the same second as a fractional `now` ends the walk one window early. Carry nanoseconds if a recording ever shows it.
async fn kucoin_pages(wire: &impl Transport, series: &HttpRequest, tail: Option<i64>, now: i64, compute: &Arc<tokio::sync::Semaphore>, held: &mut SeriesBytes<'_>) -> Fetch<Vec<Bar>> {
    let timeframe = seconds(series);
    let mut start: i64 = match tail { Some(at) => at, None => param(series, "since").parse().map_err(|_| Failure::Failed(UNAVAILABLE.into()))? };
    let mut out = vec![];
    for _ in 0..MAX_CANDLE_PAGES {
        let end = start.saturating_add(timeframe.saturating_mul(1500)).min(now);
        let page = HttpRequest { method: "GET", base: series.base.clone(), path: KUCOIN_CANDLES.into(), body: None, not_after: None,
            query: vec![("endAt", end.to_string()), ("startAt", start.to_string()), ("symbol", param(series, "symbol").to_string()), ("type", param(series, "type").to_string())] };
        let body = send(wire, &page).await?;
        let (bars, last) = cpu(compute, move || kucoin_rows(decode(&body)?, timeframe, now)).await?;
        held.take(bars.iter().map(Bar::size).sum())?;
        out.extend(bars);
        if end >= now { break; }
        start = match last { Some(last) => last.saturating_add(1), None => end.saturating_add(1) };
    }
    Ok(out)
}
/// One KuCoin window: `data` rows of `[time, open, close, high, low, volume]` strings, read in time order. The closed ones,
/// and the last row's time (closed or not), which is where the next window starts. No `data` is an empty window, as Rails'
/// `data.present?` reads it.
fn kucoin_rows(mut body: Value, timeframe: i64, now: i64) -> Fetch<(Vec<Bar>, Option<i64>)> {
    let unreadable = || Failure::Failed(UNAVAILABLE.into());
    if !body.is_object() { return Err(unreadable()); }
    let rows = match body.get_mut("data").map(Value::take) {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(rows)) => rows,
        Some(Value::String(text)) if text.is_empty() => vec![],
        Some(_) => return Err(unreadable()),
    };
    let mut read = vec![];
    for row in rows {
        let Value::Array(mut row) = row else { return Err(unreadable()) };
        let at = match row.first() { Some(Value::String(t)) => t.parse::<i64>().ok(), Some(Value::Number(n)) => n.as_i64(), _ => None }.ok_or_else(unreadable)?;
        read.push((at, row.get_mut(1).map_or(Value::Null, Value::take)));
    }
    read.sort_by_key(|(at, _)| *at);
    let last = read.last().map(|(at, _)| *at);
    let mut out = vec![];
    for (at, open) in read {
        if at.saturating_add(timeframe) <= now {
            Dec::to_d(&open)?;
            out.push(Bar { at, t: at.to_string().into(), o: open });
        }
    }
    Ok((out, last))
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
    pub fn producer(&self)->&Option<Producer> { &self.origin }
    /// What the cache's figures are made under: recorded by every fill, one that made no request too.
    pub fn produced_by(&mut self, producer: Producer) { self.origin = Some(producer); }
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
    /// Expired entries and heads go first, oldest first: what `admit` makes room with before it refuses anything.
    fn evict_expired(&self, now: i64) {
        let mut entries: Vec<(i64, String)> = self.entries.borrow().iter().filter(|(_, e)| e.until <= now).map(|(k, e)| (e.until, k.clone())).collect();
        entries.sort();
        for (_, key) in entries { self.remove_entry(&key); }
        let mut heads: Vec<(i64, String)> = self.heads.borrow().iter().filter(|(_, h)| h.until <= now).map(|(k, h)| (h.until, k.clone())).collect();
        heads.sort();
        for (_, key) in heads { self.remove_head(&key); }
    }
    /// The one admission rule for every answer the cache keeps (an entry, and a series' head with it): a replacement reuses
    /// its key's slot and counts only its size difference; a new key needs a free slot (MAX_ENTRIES, entries and heads
    /// apart); and the cache after it stays within CACHE_BYTES. Nothing kept is removed to make room but what has expired.
    fn admit(&self, key: &str, entry: usize, head: Option<usize>, now: i64) -> bool {
        self.evict_expired(now);
        let (entries, heads) = (self.entries.borrow(), self.heads.borrow());
        let old_entry = entries.get(key).map(|e| e.size);
        let old_head = heads.get(key).map(|h| h.size);
        if old_entry.is_none() && entries.len() >= MAX_ENTRIES { return false; }
        if head.is_some() && old_head.is_none() && heads.len() >= MAX_ENTRIES { return false; }
        // A head is replaced only by a head: an answer with none leaves the stored series as it is.
        let replaced = old_entry.unwrap_or(0) + head.map_or(0, |_| old_head.unwrap_or(0));
        self.bytes().saturating_sub(replaced).saturating_add(entry).saturating_add(head.unwrap_or(0)) <= CACHE_BYTES
    }
    /// An answer refused for room: a settled failure for every pass on this cache (`Reader::get`) until the next fill.
    fn refuse(&self, key: String, now: i64) {
        let mut refused = self.refused.borrow_mut();
        refused.retain(|_, until| *until > now);
        // ponytail: past MAX_ENTRIES refusals one goes unrecorded and is asked for again by the next pass of this fill.
        if refused.len() < MAX_ENTRIES || refused.contains_key(&key) { refused.insert(key, now.saturating_add(PRICE_TTL)); }
    }
    /// A new fill asks again for what an earlier one had no room for.
    pub fn clear_refusals(&self) { self.refused.borrow_mut().clear(); }
    /// Every demand answered, eight requests at a time, with no CPU work on the runtime's thread (`cpu`). Demands past
    /// MAX_ENTRIES are refused (`refuse`) for this fill, so the holdings that were admitted keep their figures.
    pub async fn fill_bounded(&mut self, wire: &impl Transport, mut demands: Vec<HttpRequest>, now: i64, compute: &Arc<tokio::sync::Semaphore>) {
        use futures_util::stream::{self, StreamExt};
        self.rendered = Rendered::default();
        // A wire with an Alpaca key makes its answers under it. One without makes them for an account with no usable key
        // (what the fill recorded, `produced_by`), never under a key it does not carry.
        self.origin = match wire.producer() { Some(version) => Some(Producer::Alpaca(version)), None => self.origin.take().filter(|p| matches!(p, Producer::NoAlpaca { .. })) };
        let producer = wire.producer();
        if demands.len() > MAX_ENTRIES { for request in demands.split_off(MAX_ENTRIES) { self.refuse(key(&request), now); } }
        // What each candle demand starts from: its stored head, unless another key's head or an expired one.
        let jobs: Vec<(HttpRequest, Option<Arc<[Bar]>>)> = demands.into_iter().map(|request| {
            let cache_key = key(&request);
            // A head made with no key is current for a fill with no key (the same account's: `Service` keys caches by identity).
            if self.heads.borrow().get(&cache_key).is_some_and(|h|(h.origin.is_some()||wire.producer().is_some())&&!crate::engine::model::current_for(h.origin.as_ref(),wire.producer().as_ref()).is_fresh()) { self.remove_head(&cache_key); }
            let old = is_series(&request).then(|| self.heads.borrow().get(&cache_key).filter(|h| h.until > now).map(|h| h.bars.clone())).flatten();
            (request, old)
        }).collect();
        let in_flight = AtomicUsize::new(0);
        let mut answers = stream::iter(jobs).map(|(request, old)| answer(wire, request, old, now, compute, &in_flight)).buffer_unordered(8);
        while let Some(answer) = answers.next().await {
            if !self.admit(&answer.key, answer.size, answer.head.as_ref().map(|(_, size)| *size), now) { self.refuse(answer.key, now); continue; }
            self.refused.borrow_mut().remove(&answer.key);
            if let Some((bars, size)) = answer.head {
                self.remove_head(&answer.key);
                self.heads.borrow_mut().insert(answer.key.clone(), Head { until: now.saturating_add(CANDLE_TTL), bars, origin: producer.clone(), size });
                self.add(size);
            }
            self.remove_entry(&answer.key);
            self.entries.borrow_mut().insert(answer.key, Entry { until: now.saturating_add(PRICE_TTL), value: answer.value, origin: producer.clone(), size: answer.size });
            self.add(answer.size);
        }
    }
    /// `fill_bounded` under a permit of its own: for a caller with no figures service (a test, a harness).
    pub async fn fill(&mut self, wire: &impl Transport, demands: Vec<HttpRequest>, now: i64) {
        self.fill_bounded(wire, demands, now, &Arc::new(tokio::sync::Semaphore::new(1))).await
    }
}

pub struct Reader<'a> { cache: &'a Cache, now: i64, demands: RefCell<BTreeMap<String, HttpRequest>>, symbols: Option<Vec<String>>, current:Option<&'a rusqlite::Connection>,
    fx: Option<FxSnapshot>, fx_demands: RefCell<std::collections::BTreeSet<(String, String)>>, fx_used: RefCell<BTreeMap<String, u64>> }
impl<'a> Reader<'a> {
    pub fn new(cache: &'a Cache, now: i64) -> Self { Self { cache, now, demands: RefCell::default(), symbols: None, current:None, fx: None, fx_demands: RefCell::default(), fx_used: RefCell::default() } }
    /// Conversions answered from the held ones (`MarketData::held_rate`); one not held is a demand (`fx_demands`).
    pub fn with_fx(mut self, fx: FxSnapshot) -> Self { self.fx = Some(fx); self }
    /// The conversions this reader was asked for and does not hold: (from, to).
    pub fn fx_demands(&self) -> Vec<(String, String)> { self.fx_demands.borrow().iter().cloned().collect() }
    /// The held conversions it answered with.
    pub fn fx_deps(&self) -> FxDeps { FxDeps { identity: self.fx.as_ref().map(|fx| fx.identity.clone()).unwrap_or_default(), pairs: self.fx_used.borrow().clone() } }
    pub fn with_current(mut self,c:&'a rusqlite::Connection)->Self { self.current=Some(c);self }
    /// Whether this pass still waits on reads: market data or a conversion it asked for and does not hold. A settled failure
    /// (an error answer, a venue this reader cannot read) is not pending: it stays with its holding or bot, as Rails rescues
    /// it there, and never fails the rest of the account's figures.
    pub fn pending(&self) -> bool { !self.demands.borrow().is_empty() || !self.fx_demands.borrow().is_empty() }
    pub fn with_symbols(mut self, symbols: Vec<String>) -> Self { self.symbols = Some(symbols); self }
    pub fn demands(&self) -> Vec<HttpRequest> { self.demands.borrow().values().cloned().collect() }
    fn get(&self, request: HttpRequest) -> Fetch<Answer> {
        let fresh=|origin:&Option<crate::engine::model::CredentialVersion>|->Result<bool,Failure> {
            let Some(c)=self.current else { return Ok(true) }; // The unbound numerical harness reads no stored credentials.
            // An answer is the cache's producer's (its key, or no key), and that producer is still the account's.
            let Some(producer)=self.cache.origin.as_ref() else { return Ok(false) };
            if !same_key(origin.as_ref(),producer.alpaca()){return Ok(false)}
            producer.is_current(c).map_err(|_|Failure::Failed(UNAVAILABLE.into()))
        };
        let key = key(&request);
        // An answer the cache had no room for is settled for this cache: asked again only by the next fill.
        if self.cache.refused.borrow().get(&key).is_some_and(|until| *until > self.now) { return Err(Failure::Failed(UNAVAILABLE.into())); }
        if let Some(entry)=self.cache.entries.borrow().get(&key){fresh(&entry.origin)?;}
        if let Some(head)=self.cache.heads.borrow().get(&key){fresh(&head.origin)?;}
        if self.cache.entries.borrow().get(&key).is_some_and(|e|!fresh(&e.origin).unwrap_or(false)){let gone=self.cache.entries.borrow_mut().remove(&key);if let Some(e)=gone{self.cache.sub(e.size);}}
        if self.cache.heads.borrow().get(&key).is_some_and(|h|!fresh(&h.origin).unwrap_or(false)){let gone=self.cache.heads.borrow_mut().remove(&key);if let Some(h)=gone{self.cache.sub(h.size);}}
        if is_series(&request) {
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

/// Whether the figures read this venue's market data at all. A venue they do not read yet has no prices or candles here,
/// which is no failure of the venue's: its bots show what is known without a price, and no note that it did not answer.
pub fn reads(venue: &Venue) -> bool { crate::venue::registry::row(&venue.exchange_type).is_some_and(|r| r.figures.is_some()) }
/// Whether the figures read this venue's candles.
fn reads_candles(venue: &Venue) -> bool { crate::venue::registry::row(&venue.exchange_type).and_then(|r| r.figures).is_some_and(|f| f.candles()) }
/// Exchanges::Kucoin#get_candles' interval names, for the timeframes the chart asks for (`chart::timeframe`). Rails' 3 days
/// and 1 month are rebuilt from 1day/1week candles; the chart never asks for them, so they have no series here.
fn kucoin_type(timeframe: i64) -> Option<&'static str> {
    Some(match timeframe { 60 => "1min", 300 => "5min", 900 => "15min", 1800 => "30min", 3600 => "1hour", 14400 => "4hour", 86_400 => "1day", 604_800 => "1week", _ => return None })
}
const ALL_TICKERS: &str = "/api/v1/market/allTickers";

impl MarketData for Reader<'_> {
    fn prices(&self, venue: &Venue, symbols: &[String]) -> Fetch<Vec<(String, Member<Dec>)>> {
        if !reads(venue) { return Err(Failure::Failed(UNAVAILABLE.into())); }
        if venue.exchange_type == crate::venue::kucoin::KUCOIN {
            // Exchanges::Kucoin#get_tickers_prices: the whole market, its symbols ignored, one read for every KuCoin bot.
            let request = HttpRequest { method: "GET", base: crate::venue::kucoin::URL.into(), path: ALL_TICKERS.into(), query: vec![], body: None, not_after: None };
            return match self.get(request)? {
                Answer::Json(body) => crate::venue::kucoin::all_tickers(&body).map(|rows| rows.into_iter().map(|(symbol, price)| (symbol, Dec::venue_number(&Value::String(price.to_s_f())))).collect()).map_err(|_| Failure::Failed(UNAVAILABLE.into())),
                Answer::Bars(_) => Err(Failure::Failed(UNAVAILABLE.into())),
            };
        }
        // The account's names make one shared request per kind (`with_symbols`). A failed one fails only a caller that
        // asks for a symbol of its kind, as Rails asks per bot (Exchanges::Alpaca#get_tickers_prices with the bot's own
        // symbols): a stock basket never sees the crypto quote fail.
        let asks = |crypto: bool| symbols.iter().any(|s| s.contains('/') == crypto);
        let symbols = self.symbols.as_deref().unwrap_or(symbols);
        let mut prices = vec![];
        let mut failed = None;
        for crypto in [false, true] {
            let mut names: Vec<&str> = symbols.iter().filter(|s| s.contains('/') == crypto).map(String::as_str).collect();
            names.sort_unstable(); names.dedup();
            if names.is_empty() { continue; }
            let path = if crypto { "/v1beta3/crypto/us/latest/trades" } else { "/v2/stocks/snapshots" };
            match self.get(request(path, vec![("symbols", names.join(","))])) {
                Err(_) | Ok(Answer::Bars(_)) if !asks(crypto) => {}
                Err(error) => failed = Some(error),
                Ok(Answer::Bars(_)) => failed = Some(Failure::Failed(UNAVAILABLE.into())),
                Ok(Answer::Json(body)) => {
                    let entries = if crypto { &body["trades"] } else { &*body };
                    let Some(entries) = entries.as_object() else { if asks(crypto) { failed = Some(Failure::Failed(UNAVAILABLE.into())); } continue; };
                    for (code, item) in entries {
                        let price = if crypto { &item["p"] } else { &item["latestTrade"]["p"] };
                        // Owner ruling: absent trades carry no price. A reported numeric zero remains a price.
                        if !price.is_null() { prices.push((code.clone(), Dec::venue_number(price))); }
                    }
                }
            }
        }
        match failed { Some(error) => Err(error), None => Ok(prices) }
    }
    fn candles(&self, venue: &Venue, ticker: &Ticker, since: At, timeframe: i64, restated: bool) -> Fetch<Vec<(At, Dec)>> {
        // A venue this reader cannot read has no candles, as it has no prices (`prices`): that holding goes without a
        // grid, and the pass is not marked failed, which would fail every bot's figures.
        if !reads_candles(venue) { return Err(Failure::Failed(UNAVAILABLE.into())); }
        if venue.exchange_type == crate::venue::kucoin::KUCOIN {
            // CandleSeriesCache's series for (ticker, since, timeframe): KuCoin has one history, so `restated` names the same
            // series. The key is the series, not a page: its pages move with now (`kucoin_pages`), the key does not.
            let Some(name) = kucoin_type(timeframe) else { return Err(Failure::Failed(UNAVAILABLE.into())) };
            let since = since.0.div_euclid(1_000_000_000);
            let series = HttpRequest { method: "GET", base: crate::venue::kucoin::URL.into(), path: KUCOIN_CANDLES.into(), body: None, not_after: None,
                query: vec![("since", since.to_string()), ("symbol", ticker.ticker.clone()), ("type", name.into())] };
            let Answer::Bars(bars) = self.get(series)? else { return Err(Failure::Failed(UNAVAILABLE.into())) };
            return bars.iter().map(|bar| {
                let at = chrono::DateTime::from_timestamp(bar.at, 0).and_then(At::from_utc).ok_or_else(|| Failure::Failed(UNAVAILABLE.into()))?;
                Ok((at, Dec::to_d(&bar.o)?))
            }).collect();
        }
        let result = (|| {
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

    // ---- KuCoin series (Exchanges::Kucoin#get_candles, CandleSeriesCache) and the cache's bounds

    /// Answers KuCoin windows from a function of (startAt, endAt), and logs every one asked for.
    struct Kucoin<F: Fn(i64, i64) -> HttpResponse> { answer: F, asked: RefCell<Vec<(i64, i64)>>, producer: CredentialVersion }
    impl<F: Fn(i64, i64) -> HttpResponse> Transport for Kucoin<F> {
        fn producer(&self)->Option<CredentialVersion>{Some(self.producer.clone())}
        async fn send(&self,r:&HttpRequest)->Result<HttpResponse,TransportError>{
            assert_eq!((r.base.as_str(), r.path.as_str()), (crate::venue::kucoin::URL, KUCOIN_CANDLES));
            let names: Vec<&str> = r.query.iter().map(|(k,_)|*k).collect();
            assert_eq!(names, ["endAt","startAt","symbol","type"], "Faraday's sorted parameters");
            let (start, end) = (param(r,"startAt").parse().unwrap(), param(r,"endAt").parse().unwrap());
            self.asked.borrow_mut().push((start, end));
            Ok((self.answer)(start, end))
        }
    }
    fn rows(times: impl IntoIterator<Item = i64>, open: impl Fn(i64) -> String) -> HttpResponse {
        let data: Vec<Value> = times.into_iter().map(|t| serde_json::json!([t.to_string(), open(t), "1", "1", "1", "1"])).collect();
        HttpResponse { status: 200, body: serde_json::json!({"code":"200000","data":data}).to_string() }
    }
    fn kucoin<F: Fn(i64, i64) -> HttpResponse>(answer: F) -> Kucoin<F> {
        Kucoin { answer, asked: RefCell::default(), producer: model::credential_version_by_id(&db(),1).unwrap().unwrap() }
    }
    const H: i64 = 3600;
    const SINCE: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z
    fn series(since: i64, kind: &str) -> HttpRequest {
        HttpRequest { method: "GET", base: crate::venue::kucoin::URL.into(), path: KUCOIN_CANDLES.into(), body: None, not_after: None,
                      query: vec![("since", since.to_string()), ("symbol", "BTC-USDT".into()), ("type", kind.into())] }
    }
    fn opens(cache: &Cache, key: &str) -> Vec<(i64, String)> {
        cache.heads.borrow()[key].bars.iter().map(|bar| (bar.at, bar.o.as_str().unwrap().to_string())).collect()
    }
    fn hourly(start: i64, end: i64) -> impl Iterator<Item = i64> { (0..).map(move |i| start - start.rem_euclid(H) + i * H).skip_while(move |t| *t < start).take_while(move |t| *t <= end) }

    #[tokio::test]
    async fn a_kucoin_series_walks_its_windows_as_rails_does(){
        // Windows of 1500 hours: candles, an empty one (a gap in the listing), candles, then the window that reaches now.
        let now = SINCE + 3500 * H + 1800;
        let listed = SINCE + 1700 * H;
        let wire = kucoin(|start, end| rows(hourly(start, end).filter(|t| *t < SINCE + 100 * H || *t >= listed), |t| format!("{}.5", (t - SINCE) / H)));
        let mut cache = Cache::default();
        let req = series(SINCE, "1hour");
        cache.fill(&wire, vec![req.clone()], now).await;
        let last_of_first = SINCE + 99 * H;
        assert_eq!(*wire.asked.borrow(), vec![
            (SINCE, SINCE + 1500 * H),                          // candles: the next window starts past the last one
            (last_of_first + 1, last_of_first + 1 + 1500 * H),  // empty: the next starts past its end
            (last_of_first + 2 + 1500 * H, last_of_first + 2 + 3000 * H),
            (SINCE + 3099 * H + 1, now),                        // reaches now: the walk ends
        ]);
        let bars = opens(&cache, &key(&req));
        assert_eq!(bars.first().unwrap(), &(SINCE, "0.5".to_string()));
        // The last candle opened at now - 30 min is not closed, and is left out; the one before it is the last.
        assert_eq!(bars.last().unwrap().0, SINCE + 3499 * H);
        assert_eq!(bars.len(), 100 + 1800, "the first 100 hours, then from the listing to the last closed candle");
    }

    #[tokio::test]
    async fn a_kept_kucoin_series_is_reused_then_extended_from_its_last_candle(){
        let now = SINCE + 10 * H;
        let wire = kucoin(|start, end| rows(hourly(start, end), |t| format!("{}", 100 + (t - SINCE) / H)));
        let mut cache = Cache::default();
        let req = series(SINCE, "1hour");
        cache.fill(&wire, vec![req.clone()], now).await;
        assert_eq!(*wire.asked.borrow(), vec![(SINCE, now)]);
        let key = key(&req);
        assert_eq!(opens(&cache, &key).last().unwrap().0, SINCE + 9 * H);
        // Before the next candle is due the head is the answer, with nothing asked.
        let reader = Reader::new(&cache, now + 61);
        assert!(reader.get(req.clone()).is_ok() && reader.demands().is_empty());
        // Past it: a tail from the last stored candle (AT it), merged on.
        let later = SINCE + 13 * H + 5;
        let reader = Reader::new(&cache, later);
        assert!(reader.get(req.clone()).is_err() && reader.demands().len() == 1, "the series is asked for again");
        cache.fill(&wire, reader.demands(), later).await;
        assert_eq!(wire.asked.borrow()[1], (SINCE + 9 * H, later));
        let bars = opens(&cache, &key);
        assert_eq!(bars.iter().map(|b| b.0).collect::<Vec<_>>(), (0..13).map(|i| SINCE + i * H).collect::<Vec<_>>());
        assert_eq!(cache.bytes(), recount(&cache));
    }

    #[tokio::test]
    async fn a_moved_overlap_rebuilds_the_kucoin_series_from_its_start(){
        let now = SINCE + 10 * H;
        let moved = std::cell::Cell::new(false);
        let wire = kucoin(|start, end| rows(hourly(start, end), |t| if moved.get() { format!("{}", 2 * (100 + (t - SINCE) / H)) } else { format!("{}", 100 + (t - SINCE) / H) }));
        let mut cache = Cache::default();
        let req = series(SINCE, "1hour");
        cache.fill(&wire, vec![req.clone()], now).await;
        moved.set(true);
        let later = SINCE + 13 * H;
        cache.fill(&wire, vec![req.clone()], later).await;
        assert_eq!(wire.asked.borrow()[1..], [(SINCE + 9 * H, later), (SINCE, later)], "the tail, then the whole series again");
        assert_eq!(opens(&cache, &key(&req))[0], (SINCE, "200".to_string()));
        // A bar that moved by 0.1% or less is rounding: the tail is merged.
        let mut cache = Cache::default();
        let small = kucoin(|start, end| rows(hourly(start, end), |t| if start > SINCE && t == SINCE + 9 * H { "109.1".into() } else { format!("{}", 100 + (t - SINCE) / H) }));
        cache.fill(&small, vec![req.clone()], now).await;
        cache.fill(&small, vec![req.clone()], later).await;
        assert_eq!(small.asked.borrow().len(), 2);
        assert_eq!(opens(&cache, &key(&req))[9], (SINCE + 9 * H, "109".to_string()), "the stored bar stands");
    }

    #[tokio::test]
    async fn a_failed_later_window_fails_the_series_and_keeps_the_stored_one(){
        let now = SINCE + 2000 * H;
        let failing = std::cell::Cell::new(false);
        let wire = kucoin(|start, end| if failing.get() && start > SINCE { HttpResponse { status: 500, body: "{}".into() } } else { rows(hourly(start, end), |_| "1".into()) });
        let mut cache = Cache::default();
        let req = series(SINCE, "1hour");
        let key = key(&req);
        cache.fill(&wire, vec![req.clone()], now).await;
        let kept = opens(&cache, &key);
        failing.set(true);
        // A cold walk whose second window fails leaves no series.
        let mut cold = Cache::default();
        cold.fill(&wire, vec![req.clone()], now).await;
        assert!(matches!(cold.entries.borrow()[&key].value, Err(Failure::Failed(_))) && cold.heads.borrow().is_empty());
        // A tail that fails keeps the stored series.
        let later = now + 3 * H;
        cache.fill(&wire, vec![req.clone()], later).await;
        assert!(matches!(cache.entries.borrow()[&key].value, Err(Failure::Failed(_))), "a settled failure: the holding has no grid");
        assert_eq!(opens(&cache, &key), kept, "the stored series is left as it was");
    }

    #[tokio::test]
    async fn a_kucoin_answer_without_candles_is_an_empty_window_and_an_unreadable_one_fails(){
        let now = SINCE + 10 * H;
        for (body, ok) in [(r#"{"code":"400100","msg":"bad"}"#, true), (r#"{"code":"200000","data":[]}"#, true), (r#"{"data":"x"}"#, false), ("[]", false), (r#"{"data":[["abc","1"]]}"#, false)] {
            let wire = kucoin(|_, _| HttpResponse { status: 200, body: body.into() });
            let mut cache = Cache::default();
            cache.fill(&wire, vec![series(SINCE, "1hour")], now).await;
            let value = &cache.entries.borrow()[&key(&series(SINCE, "1hour"))].value;
            assert_eq!(value.is_ok(), ok, "{body}");
            assert!(cache.heads.borrow().is_empty());
        }
    }

    #[tokio::test]
    async fn a_venue_paging_without_end_crosses_the_series_bound_and_keeps_the_previous_head(){
        // Every window answers 200 one-minute candles and never reaches now.
        let now = SINCE + 60 * 1500 * 2000;
        let wire = kucoin(|start, _| rows((0..200).map(|i| start + i * 60), |_| "1.5".into()));
        let mut cache = Cache::default();
        let req = series(SINCE, "1min");
        let key = key(&req);
        let old: Arc<[Bar]> = Arc::from(vec![Bar { at: SINCE, t: SINCE.to_string().into(), o: serde_json::json!("1") }]);
        cache.heads.borrow_mut().insert(key.clone(), Head { until: now + CANDLE_TTL, bars: old.clone(), origin: Some(wire.producer.clone()), size: old[0].size() });
        cache.add(old[0].size());
        // The tail from it never ends; the series bound stops it, and nothing of it is kept.
        cache.fill(&wire, vec![req], now).await;
        assert!(wire.asked.borrow().len() < MAX_CANDLE_PAGES, "stopped by the bound, {} windows", wire.asked.borrow().len());
        assert!(matches!(cache.entries.borrow()[&key].value, Err(Failure::Failed(_))));
        assert!(Arc::ptr_eq(&cache.heads.borrow()[&key].bars, &old), "the previous head stays");
        assert_eq!(cache.bytes(), recount(&cache));
    }

    #[test]
    fn every_series_in_flight_counts_against_one_fill_bound(){
        let in_flight = AtomicUsize::new(0);
        let near = SERIES_BYTES - 1;
        let mut series: Vec<SeriesBytes> = (0..4).map(|_| SeriesBytes { series: 0, in_flight: &in_flight }).collect();
        for s in &mut series { assert!(s.take(near).is_ok(), "each series alone is within its bound"); }
        let mut fifth = SeriesBytes { series: 0, in_flight: &in_flight };
        assert!(fifth.take(near).is_err(), "the fill's bound is shared");
        drop(fifth);
        drop(series.pop());
        let mut again = SeriesBytes { series: 0, in_flight: &in_flight };
        assert!(again.take(near).is_ok(), "a finished series gives its bytes back");
        assert!(again.take(2).is_err(), "and one series past its own bound fails");
    }

    #[tokio::test]
    async fn a_tail_that_makes_the_series_too_large_is_refused_and_the_old_head_stays(){
        let now = SINCE + 10 * H;
        let wire = kucoin(|start, end| rows(hourly(start, end), |_| "1".into()));
        let mut cache = Cache::default();
        let req = series(SINCE, "1hour");
        let key = key(&req);
        // A stored series just under the bound, its last candle long ago.
        let unit = Bar { at: 0, t: "0".into(), o: serde_json::json!("1") }.size();
        let big: Vec<Bar> = (0..(SERIES_BYTES / unit) as i64).map(|i| Bar { at: SINCE - 10_000_000 + i, t: "0".into(), o: serde_json::json!("1") }).collect();
        let size: usize = big.iter().map(Bar::size).sum();
        let big: Arc<[Bar]> = Arc::from(big);
        cache.heads.borrow_mut().insert(key.clone(), Head { until: now + CANDLE_TTL, bars: big.clone(), origin: Some(wire.producer.clone()), size });
        cache.add(size);
        let other = request("/v2/stocks/snapshots", vec![("symbols","AAA".into())]);
        cache.entries.borrow_mut().insert(key_of(&other), Entry { until: now + PRICE_TTL, value: Ok(Answer::Json(Arc::new(serde_json::json!({})))), origin: None, size: 2 });
        cache.add(2);
        cache.fill(&wire, vec![req], now).await;
        assert!(matches!(cache.entries.borrow()[&key].value, Err(Failure::Failed(_))));
        assert!(Arc::ptr_eq(&cache.heads.borrow()[&key].bars, &big), "the old head stays");
        assert!(cache.entries.borrow().contains_key(&key_of(&other)), "other answers are untouched");
        assert_eq!(cache.bytes(), recount(&cache));
    }
    fn key_of(r: &HttpRequest) -> String { key(r) }

    #[tokio::test]
    async fn a_full_cache_refuses_what_has_no_room_without_clearing_anything(){
        let c=db();let producer=model::credential_version_by_id(&c,1).unwrap().unwrap();
        let wire=BoundWire{producer,requests:RefCell::default()};
        let now=chrono::DateTime::parse_from_rfc3339("2026-01-02T12:00:00Z").unwrap().timestamp();
        let bars=|symbol:&str|request(&format!("/v2/stocks/{symbol}/bars"),vec![("start","2025-01-01T00:00:00Z".into()),("timeframe","1Day".into())]);
        // A cache near its total: one kept answer takes nearly all of it.
        let mut cache=Cache::default();
        let held=request("/v2/stocks/snapshots",vec![("symbols","HELD".into())]);
        cache.entries.borrow_mut().insert(key(&held),Entry{until:now+PRICE_TTL,value:Ok(Answer::Json(Arc::new(serde_json::json!({})))),origin:None,size:CACHE_BYTES-10});
        cache.add(CACHE_BYTES-10);
        cache.fill(&wire,vec![bars("NEW")],now).await;
        assert!(cache.heads.borrow().is_empty() && cache.entries.borrow().contains_key(&key(&held)), "nothing cleared, the new series refused");
        let reader=Reader::new(&cache,now);
        assert!(reader.get(bars("NEW")).is_err() && reader.demands().is_empty(), "settled for this cache, not asked again");
        // A render-only pass on a clone (another locale) reads it as settled too.
        let copy=cache.clone();
        let reader=Reader::new(&copy,now);
        assert!(reader.get(bars("NEW")).is_err() && reader.demands().is_empty());
        // The next fill asks again.
        cache.clear_refusals();
        let reader=Reader::new(&cache,now);
        assert!(reader.get(bars("NEW")).is_err() && reader.demands().len()==1);

        // More demands than MAX_ENTRIES: the admitted ones are kept, the rest refused, nothing cleared.
        let mut cache=Cache::default();
        let many:Vec<HttpRequest>=(0..MAX_ENTRIES+2).map(|i|request("/v2/stocks/snapshots",vec![("symbols",format!("S{i}"))])).collect();
        cache.fill(&wire,many.clone(),now).await;
        assert_eq!(cache.entries.borrow().len(),MAX_ENTRIES);
        let reader=Reader::new(&cache,now);
        assert!(reader.get(many[0].clone()).is_ok());
        assert!(reader.get(many[MAX_ENTRIES+1].clone()).is_err() && reader.demands().is_empty(), "refused, not demanded again in this fill");
        // A replacement of a kept key at full count is admitted.
        cache.fill(&wire,vec![many[3].clone()],now+1).await;
        assert_eq!(cache.entries.borrow()[&key(&many[3])].until,now+1+PRICE_TTL);
        // A new key at full count is refused while every entry is current; once they expire it is admitted.
        let late=request("/v2/stocks/snapshots",vec![("symbols","LATE".into())]);
        cache.fill(&wire,vec![late.clone()],now+2).await;
        assert!(!cache.entries.borrow().contains_key(&key(&late)));
        cache.fill(&wire,vec![late.clone()],now+PRICE_TTL+5).await;
        assert!(cache.entries.borrow().contains_key(&key(&late)));
        assert_eq!(cache.bytes(),recount(&cache));
    }
}
