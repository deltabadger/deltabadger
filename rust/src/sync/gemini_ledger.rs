//! AccountTransaction::SyncJob for a Gemini key (plan VB slice B3): Exchanges::Gemini#get_ledger (every trade from
//! /v1/mytrades, then every Deposit and Withdrawal from /v2/transfers, each walked forward by time), then
//! AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports them. Signed POSTs that only read. Rows,
//! tx_ids (a trade's `tid`, a transfer's `eid`), fees and raw_data are Rails' (rust/tests/sync_parity.rs, the
//! `gemini-ledger-` scenarios).
//!
//! As Rails reads it (#each_ledger_row): a page of at most 500 trades (50 transfers) from `timestamp` = the watermark less
//! 25 hours, or 0 on a first sync; the next page from the newest `timestampms` on that page, not one past it, so the rows
//! of that millisecond are read again and their ids drop the repeats; a short page ends the source; a full page with
//! nothing new fails ("Gemini ledger paging stuck at <ms>"); an answer that is not a list fails ("Gemini answered a ledger
//! read with <Class>"). Gemini allows one transfers request per 5 s, so those pages are 5 s apart. Any failed read fails
//! the sync: nothing it read is stored and the watermark stays. A trade's pair comes from the exchange's tickers (M4), a
//! pair no longer listed split at the longest quote they know.
//!
//! Where it differs on purpose (plan VB §1.6): an import longer than the job's deadline goes on over several runs
//! (sync::continuation), each storing what it read and resuming from where the last stopped (the source, the next page's
//! `timestamp`, and the ids already read at that millisecond, which is what Rails' per-source `seen` set decides there).
//! Where it reads more strictly than Rails (as binance_ledger does): an unreadable amount fails the sync (Rails stores a
//! garbage amount as 0).
//!
//! Scheduled as every ledger job is (sync::jobs::LedgerSync, Gemini's registry row has a `ledger`), through
//! ExchangeProxy.for('gemini').
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, text, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::{capture_bound_version, load_key_of, phase, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::venue::gemini::{GeminiVenue, GEMINI, URL};
use crate::venue::http::Transport;
use chrono::Duration;
use serde_json::Value;

pub const TRADES_PATH: &str = "/v1/mytrades";
pub const TRANSFERS_PATH: &str = "/v2/transfers";
/// Exchanges::Gemini::TRADES_PAGE and TRANSFERS_PAGE: the endpoints' maximums.
pub const TRADES_PAGE: usize = 500;
pub const TRANSFERS_PAGE: usize = 50;
/// Exchanges::Gemini::TRANSFERS_REQUEST_INTERVAL.
pub const TRANSFERS_PAUSE: std::time::Duration = std::time::Duration::from_secs(5);
pub const OVERLAP_HOURS: i64 = 25;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;

/// `row['timestampms'].to_i`: a number, or a numeric string; anything else is 0.
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }

/// Ruby's class name of a parsed JSON value, as #each_ledger_row names an answer that is not an Array.
fn class_of(v: &Value) -> &'static str {
    match v { Value::Object(_) => "Hash", Value::String(_) => "String", Value::Null => "NilClass", Value::Bool(true) => "TrueClass", Value::Bool(false) => "FalseClass",
              Value::Number(n) if n.is_f64() => "Float", Value::Number(_) => "Integer", Value::Array(_) => "Array" }
}

/// The exchange's tickers as #ledger_pair reads them: `ticker.downcase` to [base, quote], a later ticker winning.
pub struct Pairs(Vec<(String, String, String)>);

impl Pairs {
    pub fn load(c: &rusqlite::Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let mut s = c.prepare("SELECT ticker, base, quote FROM tickers WHERE exchange_id = ?1 ORDER BY id")?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?.to_lowercase(), r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(Self(rows))
    }
    /// #ledger_pair: the listed pair, else the symbol split at the longest quote the tickers know, else [symbol, nil].
    pub fn of(&self, symbol: &str) -> (String, Option<String>) {
        if let Some((_, base, quote)) = self.0.iter().rev().find(|(t, _, _)| *t == symbol.to_lowercase()) { return (base.clone(), Some(quote.clone())); }
        let symbol = symbol.to_uppercase();
        // `values.map(&:last).uniq.sort_by { -length }`: the quotes of the deduplicated pairs, longest first.
        let mut quotes: Vec<&str> = vec![];
        for (ticker, _, quote) in &self.0 {
            if self.0.iter().rev().find(|(t, _, _)| t == ticker).is_some_and(|(_, _, q)| q == quote) && !quotes.contains(&quote.as_str()) { quotes.push(quote); }
        }
        quotes.sort_by_key(|q| std::cmp::Reverse(q.len()));
        match quotes.into_iter().find(|q| symbol.ends_with(q) && symbol.len() > q.len()) {
            Some(q) => (symbol[..symbol.len() - q.len()].to_string(), Some(q.to_string())),
            None => (symbol, None),
        }
    }
}

/// One trade as #get_ledger maps it: a buy or a sell of the pair's base, quoted at price × amount, the fee in its own
/// currency (the quote when it names none), keyed by `tid`.
fn trade(raw: Raw, pairs: &Pairs) -> Result<Entry, Fail> {
    let symbol = raw.value["symbol"].as_str().ok_or_else(|| ("unreadable trade symbol".to_string(), true))?;
    let (base, quote) = pairs.of(symbol);
    let at = ruby_time_at_ms(to_i(&raw.value["timestampms"])).ok_or_else(|| ("unreadable trade timestampms".to_string(), true))?;
    let (size, price) = (amount(&raw, "amount", "trade")?, raw.decimal("price").map_err(|why| (format!("trade: {why}"), true))?);
    let fee = raw.decimal("fee_amount").map_err(|why| (format!("trade: {why}"), true))?;
    let mut e = entry(if raw.value["type"] == "Buy" { BUY } else { SELL }, &base, size.clone(), &raw, at);
    e.quote_amount = Some(&price * &size);
    e.fee_currency = raw.value["fee_currency"].as_str().map(str::to_uppercase).or_else(|| quote.clone());
    e.quote_currency = quote;
    e.fee_amount = Some(fee);
    e.tx_id = Some(text(raw.value.get("tid")).unwrap_or_default());
    Ok(e)
}

/// One Deposit or Withdrawal (any other type is no entry): the fee, when positive, in `feeCurrency` (else the transfer's
/// currency), keyed by `eid`.
fn transfer(raw: Raw) -> Result<Option<Entry>, Fail> {
    let kind = match raw.value["type"].as_str() { Some("Deposit") => DEPOSIT, Some("Withdrawal") => WITHDRAWAL, _ => return Ok(None) };
    let currency = raw.value["currency"].as_str().map(str::to_uppercase);
    let at = ruby_time_at_ms(to_i(&raw.value["timestampms"])).ok_or_else(|| ("unreadable transfer timestampms".to_string(), true))?;
    let fee = raw.decimal("feeAmount").map_err(|why| (format!("transfer: {why}"), true))?;
    let mut e = entry(kind, "", amount(&raw, "amount", "transfer")?, &raw, at);
    e.base_currency = currency.clone();
    if fee.is_positive() {
        e.fee_currency = raw.value["feeCurrency"].as_str().map(str::to_uppercase).or(currency);
        e.fee_amount = Some(fee);
    }
    e.tx_id = Some(text(raw.value.get("eid")).unwrap_or_default());
    Ok(Some(e))
}

/// Which list of Exchanges::Gemini#get_ledger the walk reads.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Trades, Transfers, Done }

impl Source {
    fn name(self) -> &'static str { match self { Self::Trades => "trades", Self::Transfers => "transfers", Self::Done => "done" } }
    fn named(s: &str) -> Option<Self> { [Self::Trades, Self::Transfers, Self::Done].into_iter().find(|v| v.name() == s) }
}

/// Exchanges::Gemini#get_ledger from `start` (the watermark less the overlap; None reads from 0), a page at a time:
/// `from` is the next page's `timestamp`, `seen` the ids already read at that millisecond (#each_ledger_row's `seen`:
/// rows before it never come back), `paged` whether a page of this source was read already (the transfers pause).
struct Walk<'a, T: Transport> {
    venue: &'a GeminiVenue<T>, pairs: &'a Pairs, pause: std::time::Duration, start: i64,
    source: Source, from: i64, seen: Vec<String>, paged: bool,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a GeminiVenue<T>, pairs: &'a Pairs, pause: std::time::Duration, begin: continuation::Begin<'_>) -> Self {
        // `(start_time.to_f * 1000).to_i`, or 0.
        let start = begin.start.map_or(0, |s| (s.timestamp() as f64 * 1000.0 + f64::from(s.timestamp_subsec_nanos()) / 1e6) as i64);
        let mut walk = Self { venue, pairs, pause, start, source: Source::Trades, from: start, seen: vec![], paged: false };
        if let Some((c, source)) = begin.cursor.and_then(|c| Some((c, Source::named(c["source"].as_str()?)?))) {
            walk.source = source;
            walk.from = c["from"].as_i64().unwrap_or(start);
            walk.seen = c["seen"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
            walk.paged = c["paged"].as_bool().unwrap_or(false);
        }
        walk
    }
    fn limit(&self) -> usize { if self.source == Source::Trades { TRADES_PAGE } else { TRANSFERS_PAGE } }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value { serde_json::json!({ "source": self.source.name(), "from": self.from, "seen": self.seen, "paged": self.paged }) }
    fn page_rows(&self) -> usize { self.limit() }
    /// One try at the transport's whole budget, and the transfers' pause before it.
    fn request_reserve(&self) -> std::time::Duration {
        crate::venue::http::TOTAL_TIMEOUT + if self.source == Source::Transfers { self.pause } else { std::time::Duration::ZERO }
    }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        let (path, id, params): (&str, &str, Vec<(&'static str, Value)>) = match self.source {
            Source::Trades => (TRADES_PATH, "tid", vec![("limit_trades", Value::from(TRADES_PAGE)), ("timestamp", Value::from(self.from))]),
            Source::Transfers => {
                // `sleep(pause)` after every full page: the next page of this source waits it out.
                if self.paged && !self.pause.is_zero() { tokio::time::sleep(self.pause).await; }
                (TRANSFERS_PATH, "eid", vec![("timestamp", Value::from(self.from)), ("limit_transfers", Value::from(TRANSFERS_PAGE))])
            }
            Source::Done => return Ok(vec![]),
        };
        let body = self.venue.read_text(path, params, MAX_PAGE_BYTES).await.map_err(|text| (text, false))?;
        let node = page_of(&body, "Gemini ledger", MAX_PAGE_NODES)?;
        if !matches!(node, super::wire::Node::Array(_)) {
            let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            return Err((format!("Gemini answered a ledger read with {}", class_of(&value)), false));
        }
        let page = objects(&node, "Gemini ledger row")?;
        let full = page.len() >= self.limit();
        let mut rows = vec![];
        let (mut newest, mut fresh) = (None::<i64>, 0usize);
        let mut at_newest: Vec<(i64, String)> = vec![];
        for raw in page {
            let key = text(raw.value.get(id)).unwrap_or_default();
            let ms = to_i(&raw.value["timestampms"]);
            newest = newest.max(Some(ms));
            at_newest.push((ms, key.clone()));
            // `seen.add?(row[id])`: a row met before (at the page's first millisecond) is not yielded again.
            if self.seen.contains(&key) || at_newest[..at_newest.len() - 1].iter().any(|(_, k)| *k == key) { continue; }
            fresh += 1;
            match self.source {
                Source::Trades => rows.push(trade(raw, self.pairs)?),
                _ => if let Some(e) = transfer(raw)? { rows.push(e) },
            }
        }
        self.paged = true;
        if !full {
            (self.source, self.from, self.seen, self.paged) = (if self.source == Source::Trades { Source::Transfers } else { Source::Done }, self.start, vec![], false);
            return Ok(rows);
        }
        if fresh == 0 { return Err((format!("Gemini ledger paging stuck at {}", self.from), false)); }
        let next = newest.unwrap_or(self.from);
        // The ids read at the next page's millisecond: the ones it will serve again. (The ids of the millisecond this page
        // began at stay seen too while the walk has not moved past it.)
        let mut seen: Vec<String> = if next == self.from { std::mem::take(&mut self.seen) } else { vec![] };
        for (_, key) in at_newest.into_iter().filter(|(ms, _)| *ms == next) { if !seen.contains(&key) { seen.push(key); } }
        (self.from, self.seen) = (next, seen);
        Ok(rows)
    }
}

/// One ledger sync of one Gemini key, as AccountTransaction::SyncJob#perform runs it, held to the job's deadline
/// (sync::continuation). `pause` is the transfers' pacing (TRANSFERS_PAUSE; the parity harness passes zero, as Rails'
/// harness stubs its sleep). `Ok(Err(failure))`: a fresh import stores nothing it fetched and its watermark is untouched.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pause: std::time::Duration) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, pause, &run).await
}

/// `sync` within `run`'s bounds (a test states a short deadline, a scripted stopwatch or a short chain).
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pause: std::time::Duration, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced, pairs) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[GEMINI])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        let pairs = Pairs::load(c, key.exchange_id)?;
        Ok((key, produced, pairs))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = GeminiVenue::new(transport, URL);
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, &pairs, pause, begin)).await
}

#[cfg(test)]
mod tests {
    use super::Pairs;

    #[test]
    fn a_pair_is_the_listed_one_else_split_at_the_longest_known_quote() {
        let pairs = Pairs(vec![("btcusd".into(), "BTC".into(), "USD".into()), ("ethgusd".into(), "ETH".into(), "GUSD".into())]);
        assert_eq!(pairs.of("BTCUSD"), ("BTC".into(), Some("USD".into())));
        assert_eq!(pairs.of("dogeusd"), ("DOGE".into(), Some("USD".into())), "M4: never DOG/EUSD");
        assert_eq!(pairs.of("LINKGUSD"), ("LINK".into(), Some("GUSD".into())), "the longest quote first");
        assert_eq!(pairs.of("USD"), ("USD".into(), None), "a symbol no longer than a quote is not split");
        assert_eq!(pairs.of("xyzeur"), ("XYZEUR".into(), None));
    }
}
