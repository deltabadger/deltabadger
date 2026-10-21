//! Bybit history on the continuation kit: 729 days, seven-day spot windows, then 29-day transfer windows.
//! D-BYBIT-STUCK refuses a repeated cursor; Rails would loop. Cursors and id-less dedup survive checkpoints.
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, text, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::{capture_bound_version, load_key_of, phase, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::venue::bybit::{BybitVenue, BYBIT, URL};
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

pub const FILLS_PATH: &str = "/v5/execution/list";
pub const DEPOSITS_PATH: &str = "/v5/asset/deposit/query-record";
pub const INTERNAL_PATH: &str = "/v5/asset/deposit/query-internal-record";
pub const WITHDRAWALS_PATH: &str = "/v5/asset/withdraw/query-record";
// Default pages are at most 100 executions or 50 transfers. Reserve for 100 rows.
// A first import is at least 105 execution + 3 × 26 transfer pages (183); cursor pages add to this, not windows.
pub const PAGE: usize = 100;
pub const HISTORY_DAYS: i64 = 729;
pub const TRADE_DAYS: i64 = 7;
pub const TRANSFER_DAYS: i64 = 29;
pub const OVERLAP_HOURS: i64 = 25;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;
const MAX_PAGES: usize = 100_000;

fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
fn ms(t: DateTime<Utc>) -> i64 { crate::venue::bitvavo::ruby_ms(t) }
fn abs(d: crate::ruby::BigDec) -> crate::ruby::BigDec { if d < crate::ruby::BigDec::zero() { &crate::ruby::BigDec::zero() - &d } else { d } }

struct Tickers(Vec<(String, String, String)>);
impl Tickers {
    fn load(c: &rusqlite::Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let mut s = c.prepare("SELECT ticker, base, quote FROM tickers WHERE exchange_id = ?1 ORDER BY id")?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(Self(rows))
    }
    fn pair(&self, symbol: Option<&str>) -> (Option<String>, Option<String>) {
        match self.0.iter().find(|(t, _, _)| Some(t.as_str()) == symbol) {
            Some((_, base, quote)) => (Some(base.clone()), Some(quote.clone())),
            None => (symbol.map(str::to_string), symbol.map(str::to_string)),
        }
    }
}

fn fill(raw: Raw, tickers: &Tickers) -> Result<Entry, Fail> {
    let (base, quote) = tickers.pair(raw.value["symbol"].as_str());
    let at = ruby_time_at_ms(to_i(&raw.value["execTime"])).ok_or_else(|| ("unreadable fill execTime".into(), true))?;
    let mut e = entry(if raw.value["side"] == "Buy" { BUY } else { SELL }, "", amount(&raw, "execQty", "fill")?, &raw, at);
    e.base_currency = base; e.quote_currency = quote;
    e.quote_amount = Some(amount(&raw, "execValue", "fill")?);
    e.fee_currency = text(raw.value.get("feeCurrency"));
    e.fee_amount = Some(abs(amount(&raw, "execFee", "fill")?));
    e.tx_id = text(raw.value.get("execId"));
    Ok(e)
}
fn transfer(source: Source, raw: Raw) -> Result<Option<Entry>, Fail> {
    let success = match source { Source::Deposits => [3, 70012].contains(&to_i(&raw.value["status"])), Source::Internal => to_i(&raw.value["status"]) == 2,
                                 _ => raw.value["status"] == "success" };
    if !success { return Ok(None); }
    let field = match source { Source::Deposits => "successAt", Source::Internal => "createdTime", _ => "updateTime" };
    let mut at = to_i(&raw.value[field]);
    if source == Source::Internal && at < 1_000_000_000_000 { at = at.checked_mul(1000).ok_or_else(|| ("unreadable internal transfer time".into(), true))?; }
    let at = ruby_time_at_ms(at).ok_or_else(|| ("unreadable transfer time".into(), true))?;
    let kind = if source == Source::Withdrawals { WITHDRAWAL } else { DEPOSIT };
    let mut e = entry(kind, "", amount(&raw, "amount", "transfer")?, &raw, at);
    e.base_currency = text(raw.value.get("coin"));
    if kind == WITHDRAWAL { e.fee_currency = e.base_currency.clone(); e.fee_amount = Some(amount(&raw, "withdrawFee", "withdrawal")?); }
    if source == Source::Internal { e.description = Some("Internal transfer".into()); }
    e.tx_id = text(raw.value.get("txID"));
    Ok(Some(e))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Fills, Deposits, Internal, Withdrawals, Done }

impl Source {
    fn name(self) -> &'static str { match self { Self::Fills => "fills", Self::Deposits => "deposits", Self::Internal => "internal", Self::Withdrawals => "withdrawals", Self::Done => "done" } }
    fn named(s: &str) -> Option<Self> { [Self::Fills, Self::Deposits, Self::Internal, Self::Withdrawals, Self::Done].into_iter().find(|v| v.name() == s) }
    fn path(self) -> &'static str { match self { Self::Fills => FILLS_PATH, Self::Deposits => DEPOSITS_PATH, Self::Internal => INTERNAL_PATH, _ => WITHDRAWALS_PATH } }
}

struct Walk<'a, T: Transport> {
    venue: &'a BybitVenue<T>, tickers: &'a Tickers, now_ms: i64, transfers_ms: i64,
    source: Source, window: i64, before: Option<String>, seen: Vec<String>, bare: Vec<String>, pages: usize,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a BybitVenue<T>, tickers: &'a Tickers, begin: continuation::Begin<'_>) -> Self {
        let now_ms = ms(begin.end);
        let reach = begin.end - Duration::days(HISTORY_DAYS);
        let fills_ms = ms(begin.start.map_or(reach, |s| s.max(reach)));
        let transfers_ms = fills_ms;
        let mut walk = Self { venue, tickers, now_ms, transfers_ms, source: Source::Fills, window: fills_ms, before: None, seen: vec![], bare: vec![], pages: 0 };
        if let Some((c, source)) = begin.cursor.and_then(|c| Some((c, Source::named(c["source"].as_str()?)?))) {
            walk.source = source;
            walk.window = c["window"].as_i64().unwrap_or(fills_ms);
            walk.before = c["before"].as_str().map(str::to_string);
            walk.seen = c["seen"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
            walk.bare = c["bare"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        }
        walk.settle();
        walk
    }

    fn window_end(&self) -> i64 { (self.window + if self.source == Source::Fills { TRADE_DAYS } else { TRANSFER_DAYS } * 86_400_000).min(self.now_ms) }

    fn settle(&mut self) {
        if self.source == Source::Fills && self.window >= self.now_ms { (self.source, self.window) = (Source::Deposits, self.transfers_ms); }
        if matches!(self.source, Source::Deposits | Source::Internal | Source::Withdrawals) && self.window >= self.now_ms { self.source = Source::Done; }
    }

    fn next_read(&mut self) {
        self.before = None;
        self.seen.clear();
        match self.source {
            Source::Fills => self.window = self.window_end(),
            Source::Deposits => self.source = Source::Internal,
            Source::Internal => self.source = Source::Withdrawals,
            Source::Withdrawals => (self.source, self.window) = (Source::Deposits, self.window_end()),
            Source::Done => {}
        }
        self.settle();
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value { json!({ "source": self.source.name(), "window": self.window, "before": self.before, "seen": self.seen, "bare": self.bare }) }
    fn page_rows(&self) -> usize { PAGE }
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT * 6 + std::time::Duration::from_secs(10) }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        if self.source == Source::Done { return Ok(vec![]); }
        if self.pages >= MAX_PAGES { return Err(("Too many pages of Bybit history".into(), false)); }
        self.pages += 1;
        let mut query = vec![("startTime", self.window.to_string()), ("endTime", self.window_end().to_string())];
        if self.source == Source::Fills { query.push(("category", "spot".into())); }
        if self.source == Source::Withdrawals { query.push(("withdrawType", "2".into())); }
        query.extend(self.before.clone().map(|b| ("cursor", b)));
        let optional = self.source != Source::Fills;
        let mut retries = 0;
        let body = loop {
            let text = self.venue.read_text(self.source.path(), query.clone(), MAX_PAGE_BYTES).await.map_err(|e| (e, false))?;
            let body = Raw::from_node(&page_of(&text, "Bybit ledger", MAX_PAGE_NODES)?).map_err(|e| (e, true))?;
            if to_i(&body.value["retCode"]) == 10006 && retries < 5 { retries += 1; tokio::time::sleep(std::time::Duration::from_secs(2)).await; continue; }
            break body;
        };
        let code = to_s(&body.value["retCode"]);
        if optional && to_i(&body.value["retCode"]) == 10005 { self.next_read(); return Ok(vec![]); }
        if code != "0" { return Err((format!("Bybit ledger read failed: {code} {}", to_s(&body.value["retMsg"])), false)); }
        let result = body.object("result").map_err(|e| (e, true))?;
        let page = match result.as_ref().and_then(|r| r.member(if self.source == Source::Fills { "list" } else { "rows" })) {
            None => vec![], Some(super::wire::Node::Scalar(t)) if t == "null" => vec![], Some(node) => objects(node, "Bybit ledger row")?,
        };
        let mut rows = vec![];
        for raw in &page {
            let entry = if self.source == Source::Fills { Some(fill(raw.clone(), self.tickers)?) } else { transfer(self.source, raw.clone())? };
            if let Some(e) = entry {
                if e.tx_id.as_deref().is_none_or(crate::ruby::blank) {
                    let key = format!("{}:{}", e.entry_type, raw.eql_key());
                    if self.bare.contains(&key) { continue; }
                    self.bare.push(key);
                }
                rows.push(e);
            }
        }
        let next = result.as_ref().and_then(|r| r.value.get("nextPageCursor")).filter(|v| !v.is_null()).map(to_s).filter(|s| !crate::ruby::blank(s));
        if page.is_empty() || next.is_none() { self.next_read(); return Ok(rows); }
        if next == self.before || next.as_ref().is_some_and(|n| self.seen.contains(n)) { return Err((format!("Bybit {} paging stuck at cursor {}", self.source.name(), self.before.clone().unwrap_or_default()), false)); }
        self.seen.extend(next.clone());
        self.before = next;
        Ok(rows)
    }
}

pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, &run).await
}

pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced, tickers) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[BYBIT])?;
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        let tickers = Tickers::load(c, key.exchange_id)?;
        Ok((key, produced, tickers))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = BybitVenue::new(transport, URL);
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, &tickers, begin)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::continuation::Walk as _;

    #[tokio::test(start_paused = true)]
    async fn throttle_retries_five_times_at_two_seconds_without_moving_the_cursor() {
        for recovered in [false, true] {
            let throttle = json!({"status":200,"body":{"retCode":10006,"retMsg":"Too many visits"}});
            let mut replies = vec![throttle; 6];
            if recovered { replies[5] = json!({"status":200,"body":{"retCode":0,"result":{"list":[]}}}); }
            let transport = crate::venue::http::ScriptedTransport::from_script(&json!({format!("GET {FILLS_PATH}"):replies}));
            let venue = BybitVenue::new(transport.clone(), URL);
            let tickers = Tickers(vec![]);
            let end = "2026-09-20T02:00:00Z".parse().unwrap();
            let mut walk = Walk::new(&venue, &tickers, continuation::Begin { start:None, end, cursor:None });
            let cursor = walk.cursor();
            assert_eq!(walk.request_reserve(), std::time::Duration::from_secs(280));
            let start = tokio::time::Instant::now();
            let result = walk.step().await;
            assert_eq!(result.is_ok(), recovered);
            assert_eq!(tokio::time::Instant::now() - start, std::time::Duration::from_secs(10));
            let requests = transport.requests();
            assert_eq!(requests.len(), 6);
            assert!(requests.iter().all(|r| r == &requests[0]));
            if !recovered { assert_eq!(walk.cursor(), cursor); }
        }
    }


    #[test]
    fn a_fill_names_its_listed_pair_else_its_symbol_twice() {
        let t = Tickers(vec![("BTCUSDT".into(), "BTC".into(), "USDT".into()), ("BTCUSDT".into(), "XBT".into(), "USDT".into())]);
        assert_eq!(t.pair(Some("BTCUSDT")), (Some("BTC".into()), Some("USDT".into())), "the first ticker");
        assert_eq!(t.pair(Some("NEWUSDT")), (Some("NEWUSDT".into()), Some("NEWUSDT".into())));
        assert_eq!(t.pair(None), (None, None));
    }
}
