//! AccountTransaction::SyncJob for a Hyperliquid key (plan VB slice B1): Exchanges::Hyperliquid#get_ledger (post-audit
//! H5-H7) on the venue continuation (sync::continuation), then AccountTransactionSync#store! and TransferMatcher.run! as
//! ledger.rs ports them. Read only: POSTs to /info naming the account's address (the API key). Rows, tx_ids (a fill's `tid`,
//! an update's `hash`), fees and raw_data are Rails' (rust/tests/sync_parity.rs, the `hyperliquid-ledger-` scenarios).
//!
//! As Rails reads it (#read_by_time): every fill from userFillsByTime, then every update from userNonFundingLedgerUpdates,
//! each from `startTime` = the watermark less 25 hours in ms (0 on a first sync). The next page starts at the newest `time`
//! of the one before, so the rows of that millisecond are read again; identical rows (by content, Hash#eql?) are counted,
//! a row kept as often as the page that holds it most; a page shorter than the read's limit (2000 fills, 500 updates) ends
//! the read; a full page that brings nothing new fails ("Hyperliquid <method> returned a full page with nothing new"); any
//! failed read fails the sync. A fill is a spot one when its coin is "@N" or names a pair ("PURR/USDC"): its base and
//! quote from the exchange's tickers (the raw coin and USDC when unlisted), its fee in feeToken only when positive. An
//! update is a spot deposit, withdrawal or airdrop by its delta's type (#spot_transfer_entry); every other kind is no row.
//!
//! Across runs (plan VB §1.6): the cursor holds the read, the next page's `startTime` and the counts of the rows at that
//! millisecond (the only rows a later page can serve again), so a resumed walk neither doubles a boundary row nor loses a
//! duplicate. Rows are handed to the store as each page first proves them (a later page holding more copies hands the
//! difference); the store dedupes as one store over Rails' whole fetch does.
//!
//! Where it reads more strictly than Rails (as binance_ledger does): an unreadable amount fails the sync (Rails stores a
//! garbage amount as 0), and an answer that is not a list fails it (Rails' `Array(...)` walks a Hash's pairs and raises).
use super::activities::{Entry, Raw, AIRDROP, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::{capture_bound_version, load_key_of, phase, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::http::Transport;
use crate::venue::hyperliquid::{HyperliquidVenue, FILLS_PAGE, HYPERLIQUID, TIME_PAGE, URL};
use chrono::Duration;
use serde_json::{json, Value};

pub const OVERLAP_HOURS: i64 = 25;
pub const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 800_000;
/// ponytail: Rails' read_by_time has no page cap; the venue serves at most the 10000 most recent fills, so this many pages
/// only stops a cursor that never ends.
const MAX_PAGES: usize = 100_000;

/// `#to_i` of a JSON value (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
/// `#to_s` of a scalar, as Ruby interpolates it (nil is "").
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
/// Ruby truthiness of a decoded value.
fn truthy(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) }
/// `x.presence`: a string that is not blank.
fn present(v: &Value) -> Option<String> { Some(to_s(v)).filter(|s| !v.is_null() && !crate::ruby::blank(s)) }
/// `Time.at(row['time'].to_i / 1000.0).utc`.
fn at(raw: &Raw, what: &str) -> Result<chrono::DateTime<chrono::Utc>, Fail> {
    ruby_time_at_ms(to_i(&raw.value["time"])).ok_or_else(|| (format!("unreadable {what} time"), true))
}

/// `tickers.pluck(:ticker, :base, :quote).to_h`: a pair name to its base and quote, a later ticker winning.
pub struct Pairs(Vec<(String, String, String)>);

impl Pairs {
    pub fn load(c: &rusqlite::Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let mut s = c.prepare("SELECT ticker, base, quote FROM tickers WHERE exchange_id = ?1 ORDER BY id")?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(Self(rows))
    }
    fn of(&self, coin: &str) -> Option<(&str, &str)> { self.0.iter().rev().find(|(t, _, _)| t == coin).map(|(_, b, q)| (b.as_str(), q.as_str())) }
}

/// One spot fill as #get_ledger maps it (None for a perp fill: its coin is neither "@N" nor a pair name).
fn fill(raw: &Raw, pairs: &Pairs) -> Result<Option<Entry>, Fail> {
    let coin = to_s(&raw.value["coin"]);
    if !(coin.starts_with('@') || coin.contains('/')) { return Ok(None); }
    let (base, quote) = match pairs.of(&coin) { Some((b, q)) => (b.to_string(), Some(q.to_string())), None => (coin.clone(), None) };
    let size = amount(raw, "sz", "fill")?;
    let price = raw.decimal("px").map_err(|why| (format!("fill: {why}"), true))?;
    let fee = raw.decimal("fee").map_err(|why| (format!("fill: {why}"), true))?;
    let mut e = entry(if raw.value["side"] == "B" { BUY } else { SELL }, &base, size.clone(), raw, at(raw, "fill")?);
    e.quote_amount = Some(&price * &size);
    if fee.is_positive() {
        e.fee_currency = Some(present(&raw.value["feeToken"]).or_else(|| quote.clone()).unwrap_or_else(|| "USDC".into()));
        e.fee_amount = Some(fee);
    }
    e.quote_currency = Some(quote.unwrap_or_else(|| "USDC".into()));
    e.tx_id = Some(to_s(&raw.value["tid"]));
    Ok(Some(e))
}

/// #spot_transfer_entry: one update as a deposit into or a withdrawal out of the spot balance (or a spotGenesis airdrop);
/// every other kind, and a send that is both or neither, is no row. An all-zero hash keys on nothing.
fn update(raw: &Raw, address: &str) -> Result<Option<Entry>, Fail> {
    // `update['delta'] || {}`.
    let delta_raw = raw.object("delta").map_err(|why| (format!("update: {why}"), true))?.unwrap_or_else(|| Raw::from_value(json!({})));
    let delta = &delta_raw.value;
    let mine = |field: &str| to_s(&delta[field]).eq_ignore_ascii_case(address);
    let (mut token, mut amount_key) = (delta["token"].clone(), "amount");
    let kind = match delta["type"].as_str() {
        Some("spotTransfer") => Some(if mine("user") { WITHDRAWAL } else { DEPOSIT }),
        Some("send") => {
            let out = mine("user") && delta["sourceDex"] == "spot";
            let into = mine("destination") && delta["destinationDex"] == "spot";
            if out && !into { Some(WITHDRAWAL) } else if into && !out { Some(DEPOSIT) } else { None }
        }
        Some("accountClassTransfer") => { token = json!("USDC"); amount_key = "usdc"; Some(if truthy(&delta["toPerp"]) { WITHDRAWAL } else { DEPOSIT }) }
        Some("cStakingTransfer") => Some(if truthy(&delta["isDeposit"]) { WITHDRAWAL } else { DEPOSIT }),
        Some("spotGenesis") => Some(AIRDROP),
        _ => None,
    };
    let Some(kind) = kind else { return Ok(None) };
    // `amount.to_d` of the delta's amount (or usdc): read strictly, as every other venue's amounts.
    let base_amount = amount(&delta_raw, amount_key, "update")?;
    let fee = if kind == WITHDRAWAL { delta_raw.decimal("fee").map_err(|why| (format!("update: {why}"), true))? } else { BigDec::zero() };
    let mut e = entry(kind, "", base_amount, raw, at(raw, "update")?);
    e.base_currency = if token.is_null() { None } else { Some(to_s(&token)) };
    if fee.is_positive() {
        e.fee_currency = Some(present(&delta["feeToken"]).unwrap_or_else(|| "USDC".into()));
        e.fee_amount = Some(fee);
    }
    let hash = to_s(&raw.value["hash"]);
    let zero = hash.strip_prefix("0x").is_some_and(|rest| rest.chars().all(|c| c == '0'));
    e.tx_id = if zero { None } else { Some(hash) };
    e.description = delta["type"].as_str().map(str::to_string);
    Ok(Some(e))
}

/// Which read of Exchanges::Hyperliquid#get_ledger the walk is on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Fills, Updates, Done }

impl Source {
    fn name(self) -> &'static str { match self { Self::Fills => "fills", Self::Updates => "updates", Self::Done => "done" } }
    fn named(s: &str) -> Option<Self> { [Self::Fills, Self::Updates, Self::Done].into_iter().find(|v| v.name() == s) }
    /// The info `type` and the honeymaker method #read_by_time names in its failure.
    fn kind(self) -> (&'static str, &'static str) {
        if self == Self::Fills { ("userFillsByTime", "user_fills_by_time") } else { ("userNonFundingLedgerUpdates", "user_non_funding_ledger_updates") }
    }
    /// TIME_PAGE_LIMITS.fetch(method, TIME_PAGE_LIMIT).
    fn limit(self) -> usize { if self == Self::Fills { FILLS_PAGE } else { TIME_PAGE } }
}

/// Exchanges::Hyperliquid#get_ledger from `start` (ms), a page at a time: `from` is the next page's `startTime`, `counts`
/// the most copies of each row at that millisecond any page held so far (#read_by_time's `rows`, for the rows a later page
/// can serve again).
struct Walk<'a, T: Transport> {
    venue: &'a HyperliquidVenue<T>, pairs: &'a Pairs, address: String, start: i64,
    source: Source, from: i64, counts: Vec<(String, usize)>, pages: usize,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a HyperliquidVenue<T>, pairs: &'a Pairs, address: &str, begin: continuation::Begin<'_>) -> Self {
        // `(start_time.to_f * 1000).to_i`, or 0.
        let start = begin.start.map_or(0, |s| (s.timestamp() as f64 * 1000.0 + f64::from(s.timestamp_subsec_nanos()) / 1e6) as i64);
        let mut walk = Self { venue, pairs, address: address.into(), start, source: Source::Fills, from: start, counts: vec![], pages: 0 };
        if let Some((c, source)) = begin.cursor.and_then(|c| Some((c, Source::named(c["source"].as_str()?)?))) {
            walk.source = source;
            walk.from = c["from"].as_i64().unwrap_or(start);
            walk.counts = c["counts"].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_u64()? as usize))).collect();
        }
        walk
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value { json!({ "source": self.source.name(), "from": self.from, "counts": self.counts.iter().map(|(k, n)| json!([k, n])).collect::<Vec<_>>() }) }
    fn page_rows(&self) -> usize { self.source.limit() }
    /// One try at the transport's whole budget: Rails paces none of these reads.
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        if self.source == Source::Done { return Ok(vec![]); }
        if self.pages >= MAX_PAGES { return Err(("Too many pages of Hyperliquid history".into(), false)); }
        self.pages += 1;
        let (kind, method) = self.source.kind();
        let text = self.venue.read_text(kind, vec![("user", self.address.clone().into()), ("startTime", self.from.into())], MAX_PAGE_BYTES).await.map_err(|text| (text, false))?;
        let node = page_of(&text, "Hyperliquid ledger", MAX_PAGE_NODES)?;
        // `Array(result.data)`: null is an empty page.
        let page = if matches!(&node, super::wire::Node::Scalar(t) if t == "null") { vec![] } else { objects(&node, "Hyperliquid ledger row")? };
        // `page.tally`, in the page's order (Ruby's Hash#eql?: key order is no difference).
        let mut tally: Vec<(String, usize, &Raw)> = vec![];
        // A row older than `startTime` was read on an earlier page (the venue serves from startTime on): #read_by_time's `rows`
        // already holds it, so it is never new again.
        for raw in page.iter().filter(|r| to_i(&r.value["time"]) >= self.from) {
            let key = raw.eql_key();
            match tally.iter_mut().find(|(k, _, _)| *k == key) { Some(slot) => slot.1 += 1, None => tally.push((key, 1, raw)) }
        }
        let (mut rows, mut fresh) = (vec![], 0usize);
        for (key, count, raw) in &tally {
            let before = self.counts.iter().find(|(k, _)| k == key).map_or(0, |(_, n)| *n);
            if before >= *count { continue; }
            fresh += 1;
            let mapped = if self.source == Source::Fills { fill(raw, self.pairs)? } else { update(raw, &self.address)? };
            if let Some(e) = mapped { rows.extend(std::iter::repeat_n(e, count - before)); }
        }
        if page.len() < self.source.limit() {
            (self.source, self.from, self.counts) = (if self.source == Source::Fills { Source::Updates } else { Source::Done }, self.start, vec![]);
            return Ok(rows);
        }
        if fresh == 0 { return Err((format!("Hyperliquid {method} returned a full page with nothing new"), false)); }
        let next = page.iter().map(|r| to_i(&r.value["time"])).max().unwrap_or(self.from);
        // The rows a later page can serve again are those at `next`: their most copies so far (the earlier ones too while
        // the walk has not moved past that millisecond).
        let mut counts = if next == self.from { std::mem::take(&mut self.counts) } else { vec![] };
        for (key, count, _) in tally.into_iter().filter(|(_, _, r)| to_i(&r.value["time"]) == next) {
            match counts.iter_mut().find(|(k, _)| *k == key) { Some(slot) => slot.1 = slot.1.max(count), None => counts.push((key, count)) }
        }
        (self.from, self.counts) = (next, counts);
        Ok(rows)
    }
}

/// One ledger sync of one Hyperliquid key, as AccountTransaction::SyncJob#perform runs it, held to the job's deadline
/// (sync::continuation): a long import goes on over several runs. Idempotent.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, &run).await
}

/// `sync` within `run`'s bounds (a test states a short deadline, a scripted stopwatch or a short chain).
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced, pairs) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[HYPERLIQUID])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        let pairs = Pairs::load(c, key.exchange_id)?;
        Ok((key, produced, pairs))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = HyperliquidVenue::new(transport, URL, &credentials.key);
    let address = credentials.key.clone();
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, &pairs, &address, begin)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fill_is_spot_by_its_coin_and_pays_a_positive_fee_in_its_token() {
        let pairs = Pairs(vec![("@107".into(), "HYPE".into(), "USDC".into()), ("@232".into(), "HYPE".into(), "USDH".into())]);
        let raw = |v: Value| Raw::from_value(v);
        let buy = fill(&raw(json!({"coin": "@232", "side": "B", "px": "40", "sz": "2", "fee": "0.01", "feeToken": "HYPE", "tid": 7, "time": 1_700_000_000_123i64})), &pairs).unwrap().unwrap();
        assert_eq!((buy.entry_type, buy.base_currency.as_deref(), buy.quote_currency.as_deref(), buy.fee_currency.as_deref(), buy.tx_id.as_deref()),
                   (BUY, Some("HYPE"), Some("USDH"), Some("HYPE"), Some("7")));
        assert_eq!(buy.quote_amount.unwrap().to_s_f(), "80.0");
        let rebate = fill(&raw(json!({"coin": "@999", "side": "A", "px": "1", "sz": "3", "fee": "-0.1", "tid": 8, "time": 1})), &pairs).unwrap().unwrap();
        assert_eq!((rebate.entry_type, rebate.base_currency.as_deref(), rebate.quote_currency.as_deref(), rebate.fee_amount), (SELL, Some("@999"), Some("USDC"), None));
        assert!(fill(&raw(json!({"coin": "BTC", "side": "B", "px": "1", "sz": "1", "fee": "0", "time": 1})), &pairs).unwrap().is_none(), "a perp fill");
    }

    #[test]
    fn an_update_is_a_spot_transfer_by_its_type() {
        let me = "0xAbC";
        let u = |delta: Value, hash: &str| update(&Raw::from_value(json!({"time": 5, "hash": hash, "delta": delta})), me).unwrap();
        let w = u(json!({"type": "spotTransfer", "token": "PURR", "amount": "5", "user": "0xabc", "destination": "0x1", "fee": "1", "feeToken": ""}), "0x12").unwrap();
        assert_eq!((w.entry_type, w.base_currency.as_deref(), w.fee_currency.as_deref(), w.tx_id.as_deref(), w.description.as_deref()),
                   (WITHDRAWAL, Some("PURR"), Some("USDC"), Some("0x12"), Some("spotTransfer")));
        let c = u(json!({"type": "accountClassTransfer", "usdc": "10", "toPerp": false}), "0x000").unwrap();
        assert_eq!((c.entry_type, c.base_currency.as_deref(), c.base_amount.to_s_f(), c.tx_id), (DEPOSIT, Some("USDC"), "10.0".into(), None));
        assert!(u(json!({"type": "send", "token": "X", "amount": "1", "user": "0xabc", "destination": "0xabc", "sourceDex": "spot", "destinationDex": "spot"}), "0x1").is_none());
        assert_eq!(u(json!({"type": "spotGenesis", "token": "X", "amount": "1"}), "").unwrap().tx_id.as_deref(), Some(""), "no hash is an empty id, as Rails keys it");
        assert!(u(json!({"type": "deposit", "usdc": "1"}), "0x1").is_none());
    }
}
