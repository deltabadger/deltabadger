//! AccountTransaction::SyncJob for a KuCoin key (plan K6): Exchanges::Kucoin#get_ledger (fills in weekly windows, SUCCESS
//! deposits and withdrawals), then AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports them. Read
//! only: signed GETs. Rows, tx_ids (`<symbol>-<tradeId>`, the transfer's `id` else its `walletTxId`), fees and raw_data
//! are Rails' (rust/tests/sync_parity.rs, the `kucoin-ledger-` scenarios).
//!
//! As Rails reads it: a fills range answering more than 50,000 rows is halved down to an hour, then the sync fails; a
//! throttled page (429000) is asked again up to 6 times, 10 s apart; any other failed read fails the whole sync, nothing
//! it read is stored and the watermark stays. Inner (main ↔ trade) transfers are not read: they change no holdings.
//!
//! Where it differs on purpose (plan VB A1): an import longer than the job's deadline goes on over several runs
//! (sync::continuation), each storing what it read and resuming from where the last stopped; the walk ends where the
//! import's first run began. A short import reads and stores exactly what it did in one run.
//!
//! Where it reads more strictly than Rails (as binance_ledger does): an unreadable answer or amount fails the sync (Rails
//! stores a garbage amount as 0), and a walk past 10,000 pages fails rather than running on.
//!
//! Scheduled as every ledger job is (sync::jobs::LedgerSync, KuCoin's registry row has a `ledger`), through ExchangeProxy.for('kucoin').
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, text, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::{capture_bound_version, load_key_of, phase, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::http::Transport;
use crate::venue::kucoin::{KucoinVenue, KUCOIN, URL};
use chrono::Duration;
use serde_json::Value;

pub const FILLS_PATH: &str = "/api/v1/fills";
pub const DEPOSITS_PATH: &str = "/api/v1/deposits";
pub const WITHDRAWALS_PATH: &str = "/api/v1/withdrawals";
/// Exchanges::Kucoin::FILLS_HISTORY_FROM: KuCoin's v2 data starts on 2019-02-18.
pub const HISTORY_FROM_MS: i64 = 1_550_448_000_000;
/// Exchanges::Kucoin::FILLS_WINDOW.
const WINDOW_MS: i64 = 7 * 86_400_000;
/// Exchanges::Kucoin::FILLS_RANGE_CAP and FILLS_MIN_RANGE.
const RANGE_CAP: i64 = 50_000;
const MIN_RANGE_MS: i64 = 3_600_000;
/// Exchanges::Kucoin::LEDGER_THROTTLE_RETRIES and LEDGER_THROTTLE_WAIT.
const THROTTLE_RETRIES: usize = 6;
pub const THROTTLE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
pub const OVERLAP_HOURS: i64 = 25;
const PAGE_SIZE: &str = "500";
const PAGE_ROWS: usize = 500;
/// ponytail: Rails has no page cap; one stops a walk a malformed totalPage would never end.
const MAX_PAGES: i64 = 10_000;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;

/// `#to_i` of a JSON count (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_default(), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
fn to_s(v: &Value) -> Option<String> { match v { Value::Null => None, Value::String(s) => Some(s.clone()), other => Some(other.to_string()) } }
/// Exchange#throttled_error? for KuCoin (ERRORS[:throttle]).
fn throttled(text: &str) -> bool { ["429000", "Too Many Requests"].iter().any(|p| text.contains(p)) }

/// A page that could not be read: why, and whether it was KuCoin's range cap (read_fills halves the range on that).
struct PageError { fail: Fail, over_cap: bool }
impl From<Fail> for PageError { fn from(fail: Fail) -> Self { Self { fail, over_cap: false } } }

/// One page of Exchanges::Kucoin#each_page: a throttled answer asked again (up to THROTTLE_RETRIES, `wait` apart), every
/// other failure (or an envelope without the success code: "Failed to get KuCoin history: <code> <msg>") the end of the
/// read. `capped`: page 1 of a fills range naming more than RANGE_CAP rows is KuCoin's cap (read_fills halves on it).
/// The rows, and whether this was the range's last page (`totalPage`).
async fn read_page<T: Transport>(venue: &KucoinVenue<T>, wait: std::time::Duration, path: &str, query: &[(&'static str, String)], capped: bool, page: i64,
                                 each: &mut dyn FnMut(Raw) -> Result<(), Fail>) -> Result<bool, PageError> {
    let mut retried = 0usize;
    let raw = loop {
        let mut q = query.to_vec();
        q.extend([("currentPage", page.to_string()), ("pageSize", PAGE_SIZE.to_string())]);
        let read = match venue.read_text(path, q, MAX_PAGE_BYTES).await {
            Err(text) => Err((text, false)),
            Ok(body) => {
                let raw = Raw::from_node(&page_of(&body, "KuCoin history", MAX_PAGE_NODES)?).map_err(|_| ("unreadable KuCoin history".to_string(), true))?;
                match to_s(&raw.value["code"]).as_deref() {
                    Some("200000") => Ok(raw),
                    _ => {
                        let detail: Vec<String> = [&raw.value["code"], &raw.value["msg"]].into_iter().filter_map(to_s).collect();
                        Err((format!("Failed to get KuCoin history: {}", detail.join(" ")), false))
                    }
                }
            }
        };
        match read {
            Ok(raw) => break raw,
            Err((text, false)) if throttled(&text) && retried < THROTTLE_RETRIES => {
                retried += 1;
                if !wait.is_zero() { tokio::time::sleep(wait).await; }
            }
            Err(fail) => return Err(fail.into()),
        }
    };
    let data = &raw.value["data"];
    if capped && page == 1 && to_i(&data["totalNum"]) > RANGE_CAP {
        return Err(PageError { fail: (format!("KuCoin has more than {RANGE_CAP} fills in one range"), false), over_cap: true });
    }
    if let Some(items) = raw.object("data").map_err(|why| (format!("KuCoin history: {why}"), true))?.as_ref().and_then(|d| d.member("items").cloned()) {
        if !matches!(&items, super::wire::Node::Scalar(t) if t == "null") {
            for item in objects(&items, "KuCoin history item")? { each(item)?; }
        }
    }
    Ok(page >= to_i(&data["totalPage"]))
}

/// One fill as #get_ledger maps it: a buy or a sell of the symbol's base for its quote, the fee's absolute value.
fn fill(raw: Raw) -> Result<Entry, Fail> {
    let symbol = raw.value["symbol"].as_str().ok_or_else(|| ("unreadable fill symbol".to_string(), true))?.to_string();
    let mut parts = symbol.split('-');
    let (base, quote) = (parts.next().unwrap_or_default().to_string(), parts.next().map(str::to_string));
    let at = ruby_time_at_ms(to_i(&raw.value["createdAt"])).ok_or_else(|| ("unreadable fill createdAt".to_string(), true))?;
    let side = raw.value["side"] == "buy";
    let (size, funds) = (amount(&raw, "size", "fill")?, amount(&raw, "funds", "fill")?);
    let fee = raw.number("fee").map_err(|why| (format!("fill: {why}"), true))?.map(|f| if f < BigDec::zero() { &BigDec::zero() - &f } else { f });
    let mut e = entry(if side { BUY } else { SELL }, &base, size, &raw, at);
    e.quote_currency = quote;
    e.quote_amount = Some(funds);
    e.fee_currency = raw.value["feeCurrency"].as_str().map(str::to_string);
    e.fee_amount = fee;
    e.tx_id = Some(format!("{symbol}-{}", text(raw.value.get("tradeId")).unwrap_or_default()));
    Ok(e)
}

/// One deposit or withdrawal: keyed by its record id, else its walletTxId (the on-chain hash a batch shares).
fn transfer(entry_type: i64, raw: Raw) -> Result<Entry, Fail> {
    let currency = raw.value["currency"].as_str().ok_or_else(|| ("unreadable transfer currency".to_string(), true))?.to_string();
    let at = ruby_time_at_ms(to_i(&raw.value["createdAt"])).ok_or_else(|| ("unreadable transfer createdAt".to_string(), true))?;
    let mut e = entry(entry_type, &currency, amount(&raw, "amount", "transfer")?, &raw, at);
    e.fee_currency = Some(currency);
    e.fee_amount = raw.number("fee").map_err(|why| (format!("transfer: {why}"), true))?;
    let present = |k: &str| text(raw.value.get(k)).filter(|s| !crate::ruby::blank(s));
    e.tx_id = present("id").or_else(|| present("walletTxId"));
    Ok(e)
}

/// Which part of Exchanges::Kucoin#get_ledger the walk is in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Fills, Deposits, Withdrawals, Done }

impl Source {
    fn name(self) -> &'static str { match self { Self::Fills => "fills", Self::Deposits => "deposits", Self::Withdrawals => "withdrawals", Self::Done => "done" } }
    fn named(s: &str) -> Option<Self> { [Self::Fills, Self::Deposits, Self::Withdrawals, Self::Done].into_iter().find(|v| v.name() == s) }
}

/// Exchanges::Kucoin#get_ledger from `start` (the watermark less the overlap; None is a full sync) to `end` (the import's
/// first run), a page at a time: the fills in weekly windows [`window_start`, `window_end`), a range over KuCoin's cap
/// halved down to an hour (`ranges`: the window's ranges still to read, the one being read last), then SUCCESS deposits
/// and withdrawals since `start`. `page` is the next currentPage of the range or source being read.
///
/// A fills range is bounded at both ends, so its pages hold the same rows in every run. The transfers are not (Rails
/// sends no `endAt`) and come newest first: a transfer arriving between two runs would push rows already stored onto
/// the next page, and one without an id or a hash would be stored twice. So a transfer source's first page records
/// `pin`, just past its newest row, and every page of that source a later run reads asks `endAt` = `pin`: the rows stay
/// where the first page found them, and what arrives after it is the next import's. Pages read in the run that read the
/// first one send what Rails sends.
struct Walk<'a, T: Transport> {
    venue: &'a KucoinVenue<T>, wait: std::time::Duration, start: Option<chrono::DateTime<chrono::Utc>>, end: i64,
    source: Source, window_start: i64, window_end: i64, ranges: Vec<(i64, i64)>, page: i64,
    /// The transfer source's `endAt`, recorded by its first page (`pin`) and sent from the next run on (`end_at`).
    pin: Option<i64>, end_at: Option<i64>,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a KucoinVenue<T>, wait: std::time::Duration, begin: continuation::Begin<'_>) -> Self {
        let (start, end) = (begin.start, begin.end.timestamp() * 1000);
        let mut walk = Self { venue, wait, start, end, source: Source::Fills, window_start: 0, window_end: 0, ranges: vec![], page: 1, pin: None, end_at: None };
        match begin.cursor.and_then(|c| Some((c, Source::named(c["source"].as_str()?)?))) {
            Some((c, source)) => {
                walk.source = source;
                walk.window_start = c["window_start"].as_i64().unwrap_or_default();
                walk.window_end = c["window_end"].as_i64().unwrap_or_default();
                walk.ranges = c["ranges"].as_array().into_iter().flatten().filter_map(|r| Some((r[0].as_i64()?, r[1].as_i64()?))).collect();
                walk.page = c["page"].as_i64().unwrap_or(1);
                (walk.pin, walk.end_at) = (c["pin"].as_i64(), c["pin"].as_i64());
            }
            None => walk.window(begin.start.map_or(HISTORY_FROM_MS, |s| s.timestamp() * 1000)),
        }
        walk
    }
    /// The weekly window from `from`, or the transfers once the fills reach the end.
    fn window(&mut self, from: i64) {
        self.page = 1;
        if from < self.end {
            (self.window_start, self.window_end) = (from, (from + WINDOW_MS).min(self.end));
            self.ranges = vec![(self.window_start, self.window_end)];
        } else {
            self.source = Source::Deposits;
        }
    }
    fn next_page(&mut self, last: bool) -> Result<(), Fail> {
        if !last {
            self.page += 1;
            let path = match self.source { Source::Fills => FILLS_PATH, Source::Deposits => DEPOSITS_PATH, _ => WITHDRAWALS_PATH };
            if self.page > MAX_PAGES { return Err((format!("Too many pages of KuCoin {path}"), false)); }
            return Ok(());
        }
        (self.page, self.pin, self.end_at) = (1, None, None);
        match self.source {
            Source::Fills => { self.ranges.pop(); if self.ranges.is_empty() { self.window(self.window_end); } }
            Source::Deposits => self.source = Source::Withdrawals,
            _ => self.source = Source::Done,
        }
        Ok(())
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value {
        serde_json::json!({ "source": self.source.name(), "window_start": self.window_start, "window_end": self.window_end,
                            "ranges": self.ranges.iter().map(|(a, b)| serde_json::json!([a, b])).collect::<Vec<_>>(), "page": self.page, "pin": self.pin })
    }
    fn page_rows(&self) -> usize { PAGE_ROWS }
    /// Every try at the transport's whole budget, and the throttle waits between them.
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT * (1 + THROTTLE_RETRIES as u32) + self.wait * THROTTLE_RETRIES as u32 }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        let mut rows = vec![];
        match self.source {
            Source::Fills => {
                let (a, b) = *self.ranges.last().expect("a fills window has a range");
                let mut read = |raw: Raw| -> Result<(), Fail> { rows.push(fill(raw)?); Ok(()) };
                match read_page(self.venue, self.wait, FILLS_PATH, &[("startAt", a.to_string()), ("endAt", b.to_string())], true, self.page, &mut read).await {
                    Ok(last) => self.next_page(last)?,
                    // #read_fills: a range over the cap is halved, the earlier half first, until it is an hour.
                    Err(e) if e.over_cap && b - a > MIN_RANGE_MS => { let m = a + (b - a) / 2; self.ranges.pop(); self.ranges.push((m, b)); self.ranges.push((a, m)); self.page = 1; }
                    Err(e) => return Err(e.fail),
                }
            }
            Source::Deposits | Source::Withdrawals => {
                let (path, kind) = if self.source == Source::Deposits { (DEPOSITS_PATH, DEPOSIT) } else { (WITHDRAWALS_PATH, WITHDRAWAL) };
                let mut q: Vec<(&'static str, String)> = self.start.map(|s| ("startAt", s.timestamp_millis().to_string())).into_iter().collect();
                q.extend(self.end_at.map(|e| ("endAt", e.to_string())));
                q.push(("status", "SUCCESS".into()));
                let mut newest = None;
                let mut read = |raw: Raw| -> Result<(), Fail> { newest = newest.max(Some(to_i(&raw.value["createdAt"]))); rows.push(transfer(kind, raw)?); Ok(()) };
                let last = read_page(self.venue, self.wait, path, &q, false, self.page, &mut read).await.map_err(|e| e.fail)?;
                if self.page == 1 { self.pin = newest.map(|n| n + 1); }
                self.next_page(last)?;
            }
            Source::Done => {}
        }
        Ok(rows)
    }
}

/// One ledger sync of one KuCoin key, as AccountTransaction::SyncJob#perform runs it, held to the job's deadline
/// (sync::continuation): a long import goes on over several runs. Idempotent. `wait` is the throttle wait (THROTTLE_WAIT;
/// the parity harness passes zero, as Rails' harness stubs its sleep).
/// `Ok(Err(failure))`: a fresh import stores nothing it fetched and its watermark is untouched; a resumed one keeps what
/// earlier runs stored and resumes at the next sync. The key's `last_sync_error` holds why.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, wait: std::time::Duration) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, wait, &run).await
}

/// `sync` within `run`'s bounds (a test states a short deadline, a scripted stopwatch or a short chain).
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, wait: std::time::Duration, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[KUCOIN])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        Ok((key, produced))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = KucoinVenue::new(transport, URL);
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, wait, begin)).await
}
