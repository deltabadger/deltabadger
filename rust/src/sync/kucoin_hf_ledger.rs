//! D-KUCOIN-HF-LEDGER (plan HF-3 §2): a KuCoin HF account's fills, which classic `/api/v1/fills` never returns, read
//! from `GET /api/v1/hf/fills` symbol by symbol and published page by page into the ledger, before the classic walk.
//!
//! - **The unit.** `[start, end)` in whole seconds: `end` the run's clock, `start` the later of `end − RETENTION + MARGIN`
//!   and the last certified unit's end less 25 h (failed, abandoned and uncertifiable units never move it). It is
//!   established in one commit (`hf_attempt_open`, the progress record) before any page; while it stays within KuCoin's
//!   retention a later run resumes it, with the same bounds whatever the progress record says.
//! - **Symbols.** `GET /api/v2/symbols`, every listed symbol, read once per unit and frozen in its progress. A pair the
//!   user traded that KuCoin no longer lists gets a `missing:<symbol>` observation; a unit certifies only if each open
//!   observation is at or before its start.
//! - **Pages.** `symbol, startAt, endAt, limit=100`, then the answer's `lastId` (an exact integer) until an explicit
//!   empty page. One fenced commit per page: the rows, the one-trade rule over their seconds, and the cursor.
//! - **The end.** Complete: `hf_api` when certifiable, the attempt closed. Failed or abandoned (the unit's start left
//!   KuCoin's retention before it finished): the attempt closed, nothing certified, and the error. What was published
//!   stays; what was not read is the gap.
use super::activities::{Entry, Raw};
use super::kucoin_hf::{self as hf, second, Span};
use super::kucoin_ledger::{fill, THROTTLE_RETRIES};
use super::ledger::{self, Progress};
use super::{commit_for, phase, sql_time, Key, SyncError};
use crate::engine::model::CredentialVersion;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::http::Transport;
use crate::venue::kucoin::KucoinVenue;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const FILLS_PATH: &str = "/api/v1/hf/fills";
pub const SYMBOLS_PATH: &str = "/api/v2/symbols";
/// KuCoin keeps HF fills this long, and answers at most this range (manifest: MAX_INTERVAL).
pub const RETENTION: Duration = Duration::days(7);
pub const MARGIN: Duration = Duration::hours(6);
const OVERLAP: Duration = Duration::hours(25);
const LIMIT: &str = "100";
/// ponytail: one symbol's pages; past it the unit fails, never reads as complete.
pub const MAX_PAGES: usize = 1_000;
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

fn progress_key(key_id: i64) -> String { format!("rust_sync.kucoin_hf:{key_id}") }

/// How the HF unit of one run ended.
#[derive(Debug, PartialEq)]
pub(super) enum Ended { Complete, Paused, Failed(String) }

/// The unit's progress: its bounds, symbols and where it stands.
#[derive(Clone, Debug)]
struct Unit { start: DateTime<Utc>, end: DateTime<Utc>, symbols: Option<Vec<String>>, index: usize, last_id: Option<u64> }

impl Unit {
    fn json(&self, stamp: &Value) -> Value {
        json!({ "credential": stamp, "start": sql_time(self.start), "end": sql_time(self.end), "symbols": self.symbols, "index": self.index,
                "last_id": self.last_id.map(|i| i.to_string()) })
    }
}

fn save(c: &Connection, key_id: i64, unit: &Unit, stamp: &Value, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![progress_key(key_id), unit.json(stamp).to_string(), sql_time(now)])?;
    Ok(())
}

/// The open attempt (the latest), the latest certified end, and the progress record when it is this credential's and
/// these bounds'.
/// The open attempt, the latest certified end, and the progress record.
type Loaded = (Option<Span>, Option<DateTime<Utc>>, Option<Unit>);
fn load(c: &Connection, key: &Key, stamp: &Value) -> Result<Loaded, SyncError> {
    let open = hf::spans(c, key, hf::HF_ATTEMPT_OPEN)?.into_iter().map(|(_, s)| s).max();
    let certified = hf::spans(c, key, hf::HF_API)?.into_iter().map(|(_, s)| s.1).max();
    let raw: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [progress_key(key.id)], |r| r.get(0)).optional()?.flatten();
    let unit = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()).and_then(|v| {
        if !crate::engine::model::stamp_current_for(&v["credential"], stamp).is_fresh() { return None; }
        let t = |k: &str| v[k].as_str().and_then(|s| crate::codec::parse_time(s).ok());
        Some(Unit { start: t("start")?, end: t("end")?, symbols: serde_json::from_value(v["symbols"].clone()).ok().flatten(), index: v["index"].as_u64()? as usize,
                    last_id: v["last_id"].as_str().and_then(|s| s.parse().ok()) })
    });
    Ok((open, certified, unit))
}

/// The pairs the user's history trades: its KuCoin BUY/SELL rows, visible or withheld.
fn historical_pairs(c: &Connection, key: &Key) -> Result<BTreeSet<String>, SyncError> {
    let mut s = c.prepare("SELECT DISTINCT base_currency || '-' || quote_currency FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND entry_type IN (0, 1) \
                           AND base_currency IS NOT NULL AND quote_currency IS NOT NULL \
                           UNION SELECT DISTINCT json_extract(entry, '$.symbol') FROM kucoin_withheld_fills WHERE user_id = ?1 AND exchange_id = ?2")?;
    let pairs: Vec<Option<String>> = s.query_map(params![key.user_id, key.exchange_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(pairs.into_iter().flatten().collect())
}

/// Absence episodes against the unit's symbols: a historical pair missing from them gets an observation (now) unless
/// it has one; a listed pair loses its own. Whether the unit is certifiable: every open observation at or before `start`.
fn episodes(c: &Connection, key: &Key, symbols: &[String], start: DateTime<Utc>, now: DateTime<Utc>) -> Result<bool, SyncError> {
    let listed: BTreeSet<&str> = symbols.iter().map(String::as_str).collect();
    let held: Vec<(String, Span)> = hf::spans(c, key, hf::MISSING)?;
    for (source, _) in &held {
        if listed.contains(&source[hf::MISSING.len()..]) { hf::remove(c, key, source)?; }
    }
    for pair in historical_pairs(c, key)? {
        let source = format!("{}{pair}", hf::MISSING);
        if !listed.contains(pair.as_str()) && !held.iter().any(|(s, _)| *s == source) { hf::add(c, key, &source, (second(now), second(now)), now)?; }
    }
    Ok(hf::spans(c, key, hf::MISSING)?.iter().all(|(_, (at, _))| *at <= start))
}

/// The attempt closed (`hf_attempt`), certified (`hf_api`) when `certify`, the progress record gone and the gap
/// refreshed: completion, failure and abandonment alike.
fn close(c: &crate::engine::model::FencedTransaction<'_>, key: &Key, span: Span, certify: bool, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("DELETE FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source = ?3 AND start_at = ?4 AND end_at = ?5",
              params![key.user_id, key.exchange_id, hf::HF_ATTEMPT_OPEN, sql_time(span.0), sql_time(span.1)])?;
    hf::add(c, key, hf::HF_ATTEMPT, span, now)?;
    if certify { hf::add(c, key, hf::HF_API, span, now)?; }
    c.execute("DELETE FROM app_configs WHERE key = ?1", [progress_key(key.id)])?;
    hf::refresh_gap(c, key, now)
}

fn throttled(text: &str) -> bool { ["429000", "Too Many Requests"].iter().any(|p| text.contains(p)) }

/// One signed read in order, a throttle asked again as the classic walk asks it; the success envelope's `data`.
async fn read<T: Transport>(venue: &KucoinVenue<T>, wait: std::time::Duration, path: &str, query: Vec<(&'static str, String)>) -> Result<Value, String> {
    let mut retried = 0;
    loop {
        let answer = venue.read_text_in_order(path, query.clone(), MAX_PAGE_BYTES).await
            .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|_| format!("unreadable KuCoin answer from {path}")))
            .and_then(|body| if body["code"] == "200000" { Ok(body["data"].clone()) } else { Err(format!("KuCoin {path}: {} {}", body["code"], body["msg"])) });
        match answer {
            Err(text) if throttled(&text) && retried < THROTTLE_RETRIES => { retried += 1; if !wait.is_zero() { tokio::time::sleep(wait).await; } }
            other => return other,
        }
    }
}

/// One page's items, each checked strictly and mapped as the classic fill is (identity `symbol-tradeId`, exact gross
/// amounts, the fee's absolute value, raw orderId), its raw marked `"family":"ClassicHf"`; and the next `lastId`. None:
/// the explicit empty page that ends the symbol.
fn page(data: &Value, symbol: &str, (start, end): Span, sent: Option<u64>) -> Result<Option<(Vec<Entry>, u64)>, String> {
    let items = data["items"].as_array().ok_or("KuCoin HF fills: items is not a list")?;
    if items.is_empty() { return Ok(None); }
    let last = data["lastId"].as_u64().ok_or("KuCoin HF fills: a page without an integer lastId")?;
    if Some(last) == sent { return Err("KuCoin HF fills: lastId repeats the one sent".into()); }
    let (from, to) = (start.timestamp_millis(), end.timestamp_millis());
    let mut entries: Vec<Entry> = vec![];
    for item in items {
        let decimal = |k: &str| item[k].as_str().and_then(|s| BigDec::parse(s).ok());
        let positive = |k: &str| decimal(k).is_some_and(|d| d.is_positive());
        let at = item["createdAt"].as_i64();
        let ok = item["symbol"] == symbol && matches!(item["side"].as_str(), Some("buy" | "sell")) && (item["tradeId"].is_u64() || item["tradeId"].is_string())
            && positive("size") && positive("funds") && decimal("fee").is_some() && item["feeCurrency"].is_string() && at.is_some_and(|a| a >= from && a < to);
        if !ok { return Err("KuCoin HF fills: a malformed fill".into()); }
        let mut raw = item.clone();
        raw["family"] = json!("ClassicHf");
        let e = fill(Raw::from_value(raw)).map_err(|(why, _)| format!("KuCoin HF fills: {why}"))?;
        // The same id twice in one page: identical is one fill, any difference fails it.
        let same = |a: &Entry| ["side", "size", "funds", "fee", "feeCurrency", "createdAt"].iter().all(|k| a.raw.value[k] == item[*k]);
        match entries.iter().find(|a| a.tx_id == e.tx_id) { Some(a) if same(a) => continue, Some(_) => return Err("KuCoin HF fills: one id with two different fills in a page".into()), None => entries.push(e) }
    }
    Ok(Some((entries, last)))
}

/// The page's commit: its rows (an id already stored with a different fill fails the unit, nothing written), the
/// one-trade rule over their seconds, and the cursor past it.
async fn publish(db: &Db, version: &CredentialVersion, key: &Arc<Key>, entries: Vec<Entry>, unit: Unit, stamp: Value, now: DateTime<Utc>) -> Result<Result<(), String>, SyncError> {
    let k = key.clone();
    commit_for(db, version, move |c| {
        for e in &entries { if let Some(Err(why)) = hf::conflict(c, &k, e)? { return Ok(Err(why)); } }
        if !entries.is_empty() {
            let seconds = hf::seconds_of(&entries);
            let batch = Arc::new(entries);
            let p = Progress::new(ledger::asset_rule(c, k.exchange_id, &k.exchange_type)?, ledger::stored_merged(c, &k)?, batch.clone());
            ledger::store(c, &k, 0..batch.len(), None, p, now)?;
            hf::flush(c, &k, seconds, now)?;
        }
        save(c, k.id, &unit, &stamp, now)?;
        Ok(Ok(()))
    }).await
}

/// The HF unit of one ledger run, within half the run's deadline.
pub(super) async fn run<T: Transport>(db: &Db, version: &CredentialVersion, venue: &KucoinVenue<T>, key: &Key, clock: &dyn Clock, wait: std::time::Duration,
                                      run: &super::continuation::Run<'_>) -> Result<Ended, SyncError> {
    let budget = run.deadline / 2;
    let key = Arc::new(key.clone());
    let stamp = version.cache_stamp();
    let now = clock.now();
    let (open, certified, recorded) = { let (k, s) = (key.clone(), stamp.clone()); phase(db, move |c| load(c, &k, &s)).await? };
    let end = second(now);
    let within = |start: DateTime<Utc>, at: DateTime<Utc>| at - RETENTION < start;
    // Establish: the open attempt while it is within retention, else a fresh unit (an expired one closed first).
    let (span, mut unit) = match open {
        Some(span) if within(span.0, now) => (span, recorded.filter(|u| (u.start, u.end) == span).unwrap_or(Unit { start: span.0, end: span.1, symbols: None, index: 0, last_id: None })),
        _ => {
            let start = [Some(end - RETENTION + MARGIN), certified.map(|c| c - OVERLAP)].into_iter().flatten().max().expect("a start");
            let span = (start, end);
            let unit = Unit { start, end, symbols: None, index: 0, last_id: None };
            let (k, u, s) = (key.clone(), unit.clone(), stamp.clone());
            commit_for(db, version, move |c| {
                if let Some(old) = open { close(c, &k, old, false, now)?; }
                hf::add(c, &k, hf::HF_ATTEMPT_OPEN, span, now)?;
                save(c, k.id, &u, &s, now)?;
                hf::refresh_gap(c, &k, now)
            }).await?;
            (span, unit)
        }
    };
    let fail = |text: String| {
        let k = key.clone();
        async move {
            commit_for(db, version, move |c| close(c, &k, span, false, now)).await?;
            Ok::<_, SyncError>(Ended::Failed(text))
        }
    };
    // Symbols, once per unit.
    let symbols = match unit.symbols.clone() {
        Some(symbols) => symbols,
        None => {
            let data = match read(venue, wait, SYMBOLS_PATH, vec![]).await { Ok(d) => d, Err(text) => return fail(text).await };
            let Some(list) = data.as_array().and_then(|l| l.iter().map(|s| s["symbol"].as_str().map(str::to_string)).collect::<Option<Vec<_>>>()) else {
                return fail("KuCoin symbols: unreadable list".into()).await;
            };
            unit.symbols = Some(list.clone());
            let (k, u, s) = (key.clone(), unit.clone(), stamp.clone());
            let listed = list.clone();
            commit_for(db, version, move |c| { episodes(c, &k, &listed, span.0, now)?; save(c, k.id, &u, &s, now) }).await?;
            list
        }
    };
    let reserve = crate::venue::http::TOTAL_TIMEOUT * (1 + THROTTLE_RETRIES as u32) + wait * THROTTLE_RETRIES as u32 + super::continuation::FINALIZE_FLOOR;
    let mut first = true;
    let mut pages = 0;
    while unit.index < symbols.len() {
        if clock.now() - RETENTION >= span.0 {
            return fail("the KuCoin HF unit left KuCoin's retention before it finished; what it did not read shows as a gap".into()).await;
        }
        if !first && run.stopwatch.elapsed() + reserve >= budget { return Ok(Ended::Paused); }
        first = false;
        let symbol = &symbols[unit.index];
        let mut query = vec![("symbol", symbol.clone()), ("startAt", span.0.timestamp_millis().to_string()), ("endAt", (span.1.timestamp_millis() - 1).to_string()), ("limit", LIMIT.to_string())];
        if let Some(id) = unit.last_id { query.push(("lastId", id.to_string())); }
        let data = match read(venue, wait, FILLS_PATH, query).await { Ok(d) => d, Err(text) => return fail(text).await };
        let (entries, next) = match page(&data, symbol, span, unit.last_id) {
            Err(text) => return fail(text).await,
            Ok(None) => { pages = 0; (vec![], Unit { index: unit.index + 1, last_id: None, ..unit.clone() }) }
            Ok(Some((entries, last))) => {
                pages += 1;
                if pages > MAX_PAGES { return fail(format!("Too many pages of KuCoin {FILLS_PATH} for {symbol}")).await; }
                (entries, Unit { last_id: Some(last), ..unit.clone() })
            }
        };
        if let Err(text) = publish(db, version, &key, entries, next.clone(), stamp.clone(), now).await? { return fail(text).await; }
        unit = next;
    }
    // Complete: absence checked again against the rows as they are now (r4.2).
    let k = key.clone();
    commit_for(db, version, move |c| {
        let certify = episodes(c, &k, &symbols, span.0, now)?;
        close(c, &k, span, certify, now)
    }).await?;
    Ok(Ended::Complete)
}
