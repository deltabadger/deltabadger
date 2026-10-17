//! AccountTransaction::SyncJob for a KuCoin key (plan K6): Exchanges::Kucoin#get_ledger (fills in weekly windows, SUCCESS
//! deposits and withdrawals), then AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports them. Read
//! only: signed GETs. Rows, tx_ids (`<symbol>-<tradeId>`, the transfer's `id` else its `walletTxId`), fees and raw_data
//! are Rails' (rust/tests/sync_parity.rs, the `kucoin-ledger-` scenarios).
//!
//! As Rails reads it: a fills range answering more than 50,000 rows is halved down to an hour, then the sync fails; a
//! throttled page (429000) is asked again up to 6 times, 10 s apart; any other failed read fails the whole sync, nothing is
//! stored and the watermark stays. Inner (main ↔ trade) transfers are not read: they change no holdings.
//!
//! Where it reads more strictly than Rails (as binance_ledger does): an unreadable answer or amount fails the sync (Rails
//! stores a garbage amount as 0), and a walk past 10,000 pages fails rather than running on.
//!
//! Nothing schedules it yet: KuCoin is not in `LEDGER_VENUES` (that, with its balances, comes with the venue's wiring).
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, text, Fail};
use super::ledger::{self, Outcome, Progress};
use super::{capture_bound_version, load_key_of, phase, record_sync_error, sql_time, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::http::Transport;
use crate::venue::kucoin::{KucoinVenue, KUCOIN, URL};
use chrono::{DateTime, Duration, Utc};
use rusqlite::params;
use serde_json::Value;
use std::sync::Arc;

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
/// ponytail: Rails has no page cap; one stops a walk a malformed totalPage would never end.
const MAX_PAGES: i64 = 10_000;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;
/// What one run may read in all; past it the sync fails, nothing stored.
pub const MAX_ENTRIES: usize = 500_000;
const BATCH: usize = 100;

/// `#to_i` of a JSON count (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_default(), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
fn to_s(v: &Value) -> Option<String> { match v { Value::Null => None, Value::String(s) => Some(s.clone()), other => Some(other.to_string()) } }
/// Exchange#throttled_error? for KuCoin (ERRORS[:throttle]).
fn throttled(text: &str) -> bool { ["429000", "Too Many Requests"].iter().any(|p| text.contains(p)) }

/// A page that could not be read: why, and whether it was KuCoin's range cap (read_fills halves the range on that).
struct PageError { fail: Fail, over_cap: bool }
impl From<Fail> for PageError { fn from(fail: Fail) -> Self { Self { fail, over_cap: false } } }

/// Exchanges::Kucoin#each_page: currentPage 1.. to totalPage, a throttled answer asked again, every other failure (or an
/// envelope without the success code: "Failed to get KuCoin history: <code> <msg>") the end of the read.
async fn each_page<T: Transport>(venue: &KucoinVenue<T>, wait: std::time::Duration, path: &str, query: &[(&'static str, String)], capped: bool,
                                 each: &mut dyn FnMut(Raw) -> Result<(), Fail>) -> Result<(), PageError> {
    let (mut page, mut retried) = (1i64, 0usize);
    loop {
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
        let raw = match read {
            Ok(raw) => raw,
            Err((text, false)) if throttled(&text) && retried < THROTTLE_RETRIES => {
                retried += 1;
                if !wait.is_zero() { tokio::time::sleep(wait).await; }
                continue;
            }
            Err(fail) => return Err(fail.into()),
        };
        retried = 0;
        let data = &raw.value["data"];
        if capped && page == 1 && to_i(&data["totalNum"]) > RANGE_CAP {
            return Err(PageError { fail: (format!("KuCoin has more than {RANGE_CAP} fills in one range"), false), over_cap: true });
        }
        if let Some(items) = raw.object("data").map_err(|why| (format!("KuCoin history: {why}"), true))?.as_ref().and_then(|d| d.member("items").cloned()) {
            if !matches!(&items, super::wire::Node::Scalar(t) if t == "null") {
                for item in objects(&items, "KuCoin history item")? { each(item)?; }
            }
        }
        if page >= to_i(&data["totalPage"]) { return Ok(()); }
        page += 1;
        if page > MAX_PAGES { return Err((format!("Too many pages of KuCoin {path}"), false).into()); }
    }
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

fn push(entries: &mut Vec<Entry>, e: Entry) -> Result<(), Fail> {
    if entries.len() >= MAX_ENTRIES { return Err((format!("more than {MAX_ENTRIES} KuCoin ledger rows in one run"), false)); }
    entries.push(e);
    Ok(())
}

/// Exchanges::Kucoin#get_ledger from `start` (the watermark less the overlap; None is a full sync) to now.
async fn fetch<T: Transport>(venue: &KucoinVenue<T>, wait: std::time::Duration, start: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<Vec<Entry>, Fail> {
    let mut entries: Vec<Entry> = vec![];
    let now_ms = now.timestamp() * 1000;
    let mut from = start.map_or(HISTORY_FROM_MS, |s| s.timestamp() * 1000);
    while from < now_ms {
        let to = (from + WINDOW_MS).min(now_ms);
        // #read_fills: a range over the cap is halved, the earlier half first, until it is an hour.
        let mut ranges = vec![(from, to)];
        while let Some((a, b)) = ranges.pop() {
            let mut read = |raw: Raw| -> Result<(), Fail> { push(&mut entries, fill(raw)?) };
            match each_page(venue, wait, FILLS_PATH, &[("startAt", a.to_string()), ("endAt", b.to_string())], true, &mut read).await {
                Ok(()) => {}
                Err(e) if e.over_cap && b - a > MIN_RANGE_MS => { let m = a + (b - a) / 2; ranges.push((m, b)); ranges.push((a, m)); }
                Err(e) => return Err(e.fail),
            }
        }
        from = to;
    }
    let since: Vec<(&'static str, String)> = start.map(|s| ("startAt", s.timestamp_millis().to_string())).into_iter().collect();
    for (path, kind) in [(DEPOSITS_PATH, DEPOSIT), (WITHDRAWALS_PATH, WITHDRAWAL)] {
        let mut q = since.clone();
        q.push(("status", "SUCCESS".into()));
        let mut read = |raw: Raw| -> Result<(), Fail> { push(&mut entries, transfer(kind, raw)?) };
        each_page(venue, wait, path, &q, false, &mut read).await.map_err(|e| e.fail)?;
    }
    Ok(entries)
}

/// One ledger sync of one KuCoin key, as AccountTransaction::SyncJob#perform runs it. Idempotent. `wait` is the throttle
/// wait (THROTTLE_WAIT; the parity harness passes zero, as Rails' harness stubs its sleep).
/// `Ok(Err(failure))`: nothing fetched is stored, the watermark is untouched, the key's `last_sync_error` holds why.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, wait: std::time::Duration) -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let key = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[KUCOIN])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        Ok((key, produced))
    }).await?;
    let (key, produced) = key;
    let watermark = if produced { key.last_synced_at } else { None };
    let user_id = key.user_id;
    let start = watermark.map(|w| w.min(now) - Duration::hours(OVERLAP_HOURS));
    let venue = KucoinVenue::new(transport, URL);
    let entries = match fetch(&venue, wait, start, now).await {
        Ok(entries) => entries,
        Err((text, raised)) => {
            let creds = credentials.clone();
            let error = super::commit_for(db, &version, move |c| record_sync_error(c, key_id, &text, &creds)).await?;
            if !raised { ledger::link_transfers_for(db, user_id, Some(&version), clock.now()).await?; }
            return Ok(Err(Failure { error, raised }));
        }
    };
    let raw_watermark = key.last_synced_at.map(|t| t.timestamp_micros());
    let max_seen = entries.iter().filter_map(|e| e.transacted_at).max();
    let key = Arc::new(key);
    let entries = Arc::new(entries);
    let mut progress = {
        let (key, batch) = (key.clone(), entries.clone());
        phase(db, move |c| Ok(Progress::new(ledger::asset_rule(c, key.exchange_id, &key.exchange_type)?, ledger::stored_merged(c, &key)?, batch))).await?
    };
    let mut from_entry = 0;
    while from_entry < entries.len() {
        let (key, to) = (key.clone(), entries.len().min(from_entry + BATCH));
        let at = from_entry;
        progress = super::commit_for(db, &version, move |c| ledger::store(c, &key, at..to, None, progress, now)).await?;
        from_entry = to;
    }
    ledger::resolve_recent(db, &version, &key, progress.assets.clone(), now).await?;
    let mut outcome = progress.out;
    // The newest row read, held back by a row that could not be saved, never past this sync's start; else the last one.
    let watermark = [max_seen, progress.min_skipped].into_iter().flatten().min().or(key.last_synced_at).map(|w| w.min(now));
    let changed = watermark.map(|w| w.timestamp_micros()) != raw_watermark || key.last_sync_error.is_some();
    let producer = version.clone();
    super::commit_for(db, &version, move |c| {
        super::cache::record_ledger(c, key_id, &producer, now)?;
        if changed { c.execute("UPDATE api_keys SET last_synced_at = ?1, last_sync_error = NULL, updated_at = ?2 WHERE id = ?3", params![watermark.map(sql_time), sql_time(now), key_id])?; }
        Ok(())
    }).await?;
    outcome.watermark = watermark;
    outcome.complete = true;
    outcome.linked = ledger::link_transfers_for(db, user_id, Some(&version), clock.now()).await?;
    Ok(Ok(outcome))
}
