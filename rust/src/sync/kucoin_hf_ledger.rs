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
//! - **UTA** (D-KUCOIN-UTA-LEDGER, plan UTA r1.8a): the same unit over `GET /api/ua/v2/unified/order/execution`,
//!   account-wide (`tradeType=SPOT`, no symbol, so no `missing:` observations), `pageSize=200`, sides `BUY|SELL` and ns
//!   times mapped onto the classic fill (`funds` = `value`, `createdAt` = `executionTime / 10^6`), raw `"family":"Uta"`.
//!   KuCoin states no retention for it; the HF one (7 d) is assumed, so a longer one only reads less, and shows as gap.
//! - **UTA backfill** (r2.3, r3, r4): within `[now − 365 d, the unit's reach)`, what no completed UTA read covers
//!   (`uta_read`) is read back one 7-day window a run, newest first, whatever the account's mode now and whatever an
//!   export covers; the one-trade rule keeps each trade once. A window keeps its bounds and cursor until it ends on an
//!   empty page; a failed one waits a day (`RETRY`) while older ones go on. It certifies nothing (a silent clamp can't be
//!   told from completeness), and its failures are logged only. The 365 days are our cap, one KuCoin export's span.
//! - **The end.** Complete: `hf_api` (`uta_api`) when certifiable, only from the start of the family's current mode
//!   episode and never for a family the account is not in now (r2.1); the attempt closed. Failed or abandoned (the unit's start left
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
use crate::venue::kucoin_account::AccountMode;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const FILLS_PATH: &str = "/api/v1/hf/fills";
pub const UTA_FILLS_PATH: &str = "/api/ua/v2/unified/order/execution";
pub const SYMBOLS_PATH: &str = "/api/v2/symbols";
/// KuCoin keeps HF fills this long, and answers at most this range (manifest: MAX_INTERVAL).
pub const RETENTION: Duration = Duration::days(7);
pub const MARGIN: Duration = Duration::hours(6);
const OVERLAP: Duration = Duration::hours(25);

/// ponytail: one symbol's pages; past it the unit fails, never reads as complete.
pub const MAX_PAGES: usize = 1_000;
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// One family's fills API: what a unit reads, how it pages, and its coverage sources.
#[derive(Clone, Copy)]
pub(super) struct Route {
    pub mode: AccountMode, name: &'static str, path: &'static str, progress: &'static str, api: &'static str, attempt: &'static str, open: &'static str,
    /// Where a completed walk is recorded as read (UTA only: the backfill's complement).
    read: Option<&'static str>,
    /// Symbol by symbol (HF), else one account-wide walk (UTA).
    by_symbol: bool, limit: (&'static str, &'static str),
    /// A page item checked strictly and mapped onto the classic fill's raw, or None.
    item: fn(&Value, &str, Span) -> Option<Value>,
}
pub(super) const HF: Route = Route { mode: AccountMode::ClassicHf, name: "KuCoin HF fills", path: FILLS_PATH, progress: "rust_sync.kucoin_hf", api: hf::HF_API, attempt: hf::HF_ATTEMPT,
                                     open: hf::HF_ATTEMPT_OPEN, read: None, by_symbol: true, limit: ("limit", "100"), item: hf_item };
pub(super) const UTA: Route = Route { mode: AccountMode::Uta, name: "KuCoin UTA fills", path: UTA_FILLS_PATH, progress: "rust_sync.kucoin_uta", api: hf::UTA_API,
                                      attempt: hf::UTA_ATTEMPT, open: hf::UTA_ATTEMPT_OPEN, read: Some(hf::UTA_READ), by_symbol: false, limit: ("pageSize", "200"), item: uta_item };
impl Route {
    pub(super) fn of(mode: AccountMode) -> Option<Self> { [HF, UTA].into_iter().find(|r| r.mode == mode) }
    fn progress_key(&self, key_id: i64) -> String { format!("{}:{key_id}", self.progress) }
}

fn decimal(item: &Value, k: &str) -> Option<BigDec> { item[k].as_str().and_then(|s| BigDec::parse(s).ok()) }
fn positive(item: &Value, k: &str) -> bool { decimal(item, k).is_some_and(|d| d.is_positive()) }

/// An HF fill: the classic fill's own fields.
fn hf_item(item: &Value, symbol: &str, (start, end): Span) -> Option<Value> {
    let at = item["createdAt"].as_i64();
    let ok = item["symbol"] == symbol && matches!(item["side"].as_str(), Some("buy" | "sell")) && (item["tradeId"].is_u64() || item["tradeId"].is_string())
        && positive(item, "size") && positive(item, "funds") && decimal(item, "fee").is_some() && item["feeCurrency"].is_string()
        && at.is_some_and(|a| a >= start.timestamp_millis() && a < end.timestamp_millis());
    let mut raw = item.clone();
    raw["family"] = json!("ClassicHf");
    ok.then_some(raw)
}

/// A UTA execution (r1.1): SPOT only, a pair symbol, `BUY|SELL` exactly, the ns time within the unit.
fn uta_item(item: &Value, _: &str, (start, end): Span) -> Option<Value> {
    let at = item["executionTime"].as_u64().map(|ns| (ns / 1_000_000) as i64);
    let side = match item["side"].as_str() { Some("BUY") => "buy", Some("SELL") => "sell", _ => return None };
    let ok = item["tradeType"] == "SPOT" && item["symbol"].as_str().is_some_and(|s| s.contains('-')) && (item["tradeId"].is_u64() || item["tradeId"].is_string())
        && positive(item, "size") && positive(item, "value") && decimal(item, "fee").is_some() && item["feeCurrency"].is_string()
        && at.is_some_and(|a| a >= start.timestamp_millis() && a < end.timestamp_millis());
    let mut raw = item.clone();
    (raw["side"], raw["funds"], raw["createdAt"], raw["family"]) = (json!(side), item["value"].clone(), json!(at), json!("Uta"));
    ok.then_some(raw)
}

/// How the HF unit of one run ended.
#[derive(Debug, PartialEq)]
pub(super) enum Ended { Complete, Paused, Failed(String) }

/// The unit's progress: its bounds, symbols and where it stands.
#[derive(Clone, Debug)]
struct Unit { start: DateTime<Utc>, end: DateTime<Utc>, symbols: Option<Vec<String>>, index: usize, last_id: Option<u64>, pages: usize }

impl Unit {
    fn json(&self, stamp: &Value) -> Value {
        json!({ "credential": stamp, "start": sql_time(self.start), "end": sql_time(self.end), "symbols": self.symbols, "index": self.index,
                "last_id": self.last_id.map(|i| i.to_string()), "pages": self.pages })
    }
}

fn save(c: &Connection, route: &Route, key_id: i64, unit: &Unit, stamp: &Value, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![route.progress_key(key_id), unit.json(stamp).to_string(), sql_time(now)])?;
    Ok(())
}

/// The open attempt (the latest), the latest certified end, and the progress record when it is this credential's.
type Loaded = (Option<Span>, Option<DateTime<Utc>>, Option<Unit>);
fn load(c: &Connection, route: &Route, key: &Key, stamp: &Value) -> Result<Loaded, SyncError> {
    let open = hf::spans(c, key, route.open)?.into_iter().map(|(_, s)| s).max();
    let certified = hf::spans(c, key, route.api)?.into_iter().map(|(_, s)| s.1).max();
    let raw: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [route.progress_key(key.id)], |r| r.get(0)).optional()?.flatten();
    let unit = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()).and_then(|v| {
        if !crate::engine::model::stamp_current_for(&v["credential"], stamp).is_fresh() { return None; }
        let t = |k: &str| v[k].as_str().and_then(|s| crate::codec::parse_time(s).ok());
        Some(Unit { start: t("start")?, end: t("end")?, symbols: serde_json::from_value(v["symbols"].clone()).ok().flatten(), index: v["index"].as_u64()? as usize,
                    last_id: v["last_id"].as_str().and_then(|s| s.parse().ok()), pages: v["pages"].as_u64().unwrap_or(0) as usize })
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

/// The attempt closed (`hf_attempt`), what it certifies (`hf_api`, r2.1's clip of `span`) when `certify`, the progress
/// record gone and the gap refreshed: completion, failure and abandonment alike.
fn close(c: &crate::engine::model::FencedTransaction<'_>, route: &Route, key: &Key, span: Span, certify: bool, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("DELETE FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source = ?3 AND start_at = ?4 AND end_at = ?5",
              params![key.user_id, key.exchange_id, route.open, sql_time(span.0), sql_time(span.1)])?;
    hf::add(c, key, route.attempt, span, now)?;
    if certify { if let Some(certified) = hf::certifiable(c, key, route.mode, span)? { hf::add(c, key, route.api, certified, now)?; } }
    c.execute("DELETE FROM app_configs WHERE key = ?1", [route.progress_key(key.id)])?;
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

/// One page's items, each checked strictly (`route.item`) and mapped as the classic fill is (identity `symbol-tradeId`,
/// exact gross amounts, the fee's absolute value, raw orderId and family); and the next `lastId`. None: the explicit
/// empty page that ends the walk.
fn page(route: &Route, data: &Value, symbol: &str, span: Span, sent: Option<u64>) -> Result<Option<(Vec<Entry>, u64)>, String> {
    let name = route.name;
    let items = data["items"].as_array().ok_or(format!("{name}: items is not a list"))?;
    if items.is_empty() { return Ok(None); }
    let last = data["lastId"].as_u64().ok_or(format!("{name}: a page without an integer lastId"))?;
    if Some(last) == sent { return Err(format!("{name}: lastId repeats the one sent")); }
    let mut entries: Vec<Entry> = vec![];
    for item in items {
        let raw = (route.item)(item, symbol, span).ok_or(format!("{name}: a malformed fill"))?;
        let e = fill(Raw::from_value(raw.clone())).map_err(|(why, _)| format!("{name}: {why}"))?;
        // The same id twice in one page: identical is one fill, any difference fails it.
        let same = |a: &Entry| ["side", "size", "funds", "fee", "feeCurrency", "createdAt"].iter().all(|k| a.raw.value[k] == raw[*k]);
        match entries.iter().find(|a| a.tx_id == e.tx_id) { Some(a) if same(a) => continue, Some(_) => return Err(format!("{name}: one id with two different fills in a page")), None => entries.push(e) }
    }
    Ok(Some((entries, last)))
}

/// The page's commit: its rows (an id already stored with a different fill fails the unit, nothing written), the
/// one-trade rule over their seconds, and the cursor past it.
#[allow(clippy::too_many_arguments)]
async fn publish(db: &Db, version: &CredentialVersion, route: Route, key: &Arc<Key>, entries: Vec<Entry>, unit: Unit, stamp: Value, now: DateTime<Utc>) -> Result<Result<(), String>, SyncError> {
    let k = key.clone();
    commit_for(db, version, move |c| {
        if let Err(why) = store_page(c, &k, entries, now)? { return Ok(Err(why)); }
        save(c, &route, k.id, &unit, &stamp, now)?;
        Ok(Ok(()))
    }).await
}

/// A page's rows (an id already stored with a different fill: nothing written) and the one-trade rule over their seconds.
fn store_page(c: &crate::engine::model::FencedTransaction<'_>, k: &Key, entries: Vec<Entry>, now: DateTime<Utc>) -> Result<Result<(), String>, SyncError> {
    for e in &entries { if let Some(Err(why)) = hf::conflict(c, k, e)? { return Ok(Err(why)); } }
    if !entries.is_empty() {
        let seconds = hf::seconds_of(&entries);
        let batch = Arc::new(entries);
        let p = Progress::new(ledger::asset_rule(c, k.exchange_id, &k.exchange_type)?, ledger::stored_merged(c, k)?, batch.clone());
        ledger::store(c, k, 0..batch.len(), None, p, now)?;
        hf::flush(c, k, seconds, now)?;
    }
    Ok(Ok(()))
}

/// One family's unit of one ledger run, within half the run's deadline.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run<T: Transport>(db: &Db, version: &CredentialVersion, route: Route, venue: &KucoinVenue<T>, key: &Key, clock: &dyn Clock, wait: std::time::Duration,
                                      run: &super::continuation::Run<'_>) -> Result<Ended, SyncError> {
    let budget = run.deadline / 2;
    let key = Arc::new(key.clone());
    let stamp = version.cache_stamp();
    let now = clock.now();
    let (open, certified, recorded) = { let (k, s) = (key.clone(), stamp.clone()); phase(db, move |c| load(c, &route, &k, &s)).await? };
    let end = second(now);
    let within = |start: DateTime<Utc>, at: DateTime<Utc>| at - RETENTION < start;
    // Establish: the open attempt while it is within retention, else a fresh unit (an expired one closed first).
    let (span, mut unit) = match open {
        Some(span) if within(span.0, now) => (span, recorded.filter(|u| (u.start, u.end) == span).unwrap_or(Unit { start: span.0, end: span.1, symbols: None, index: 0, last_id: None, pages: 0 })),
        _ => {
            let start = [Some(end - RETENTION + MARGIN), certified.map(|c| c - OVERLAP)].into_iter().flatten().max().expect("a start");
            let span = (start, end);
            let unit = Unit { start, end, symbols: None, index: 0, last_id: None, pages: 0 };
            let (k, u, s) = (key.clone(), unit.clone(), stamp.clone());
            commit_for(db, version, move |c| {
                if let Some(old) = open { close(c, &route, &k, old, false, now)?; }
                hf::add(c, &k, route.open, span, now)?;
                save(c, &route, k.id, &u, &s, now)?;
                hf::refresh_gap(c, &k, now)
            }).await?;
            (span, unit)
        }
    };
    let fail = |text: String| {
        let k = key.clone();
        async move {
            commit_for(db, version, move |c| close(c, &route, &k, span, false, now)).await?;
            Ok::<_, SyncError>(Ended::Failed(text))
        }
    };
    // Symbols, once per unit; an account-wide route walks once.
    let symbols = match unit.symbols.clone() {
        Some(symbols) => symbols,
        None if !route.by_symbol => vec![String::new()],
        None => {
            let data = match read(venue, wait, SYMBOLS_PATH, vec![]).await { Ok(d) => d, Err(text) => return fail(text).await };
            let Some(list) = data.as_array().and_then(|l| l.iter().map(|s| s["symbol"].as_str().map(str::to_string)).collect::<Option<Vec<_>>>()) else {
                return fail("KuCoin symbols: unreadable list".into()).await;
            };
            unit.symbols = Some(list.clone());
            let (k, u, s) = (key.clone(), unit.clone(), stamp.clone());
            let listed = list.clone();
            commit_for(db, version, move |c| { episodes(c, &k, &listed, span.0, now)?; save(c, &route, k.id, &u, &s, now) }).await?;
            list
        }
    };
    let reserve = crate::venue::http::TOTAL_TIMEOUT * (1 + THROTTLE_RETRIES as u32) + wait * THROTTLE_RETRIES as u32 + super::continuation::FINALIZE_FLOOR;
    let mut first = true;
    while unit.index < symbols.len() {
        if clock.now() - RETENTION >= span.0 {
            return fail(format!("the {} unit left KuCoin's retention before it finished; what it did not read shows as a gap", route.name)).await;
        }
        if !first && run.stopwatch.elapsed() + reserve >= budget { return Ok(Ended::Paused); }
        first = false;
        let symbol = &symbols[unit.index];
        let first_param = if route.by_symbol { ("symbol", symbol.clone()) } else { ("tradeType", "SPOT".to_string()) };
        let mut query = vec![first_param, ("startAt", span.0.timestamp_millis().to_string()), ("endAt", (span.1.timestamp_millis() - 1).to_string()), (route.limit.0, route.limit.1.to_string())];
        if let Some(id) = unit.last_id { query.push(("lastId", id.to_string())); }
        let data = match read(venue, wait, route.path, query).await { Ok(d) => d, Err(text) => return fail(text).await };
        let (entries, next) = match page(&route, &data, symbol, span, unit.last_id) {
            Err(text) => return fail(text).await,
            Ok(None) => (vec![], Unit { index: unit.index + 1, last_id: None, pages: 0, ..unit.clone() }),
            // The symbol's pages, counted across runs (the progress keeps the count).
            Ok(Some(_)) if unit.pages >= MAX_PAGES => return fail(format!("Too many pages of KuCoin {}{}", route.path, if route.by_symbol { format!(" for {symbol}") } else { String::new() })).await,
            Ok(Some((entries, last))) => (entries, Unit { last_id: Some(last), pages: unit.pages + 1, ..unit.clone() }),
        };
        if let Err(text) = publish(db, version, route, &key, entries, next.clone(), stamp.clone(), now).await? { return fail(text).await; }
        unit = next;
    }
    // Complete: the mode read again (r2.1: certified only if it is still this family's; the reading is recorded), and
    // absence checked again against the rows as they are now (r4.2).
    let ended = venue.account_mode_now().await.ok();
    let (k, at) = (key.clone(), clock.now());
    commit_for(db, version, move |c| {
        if let Some(read) = route.read { hf::add(c, &k, read, span, now)?; }
        if let Some(mode) = ended { hf::observe_mode(c, &k, mode, at)?; }
        let certify = ended == Some(route.mode) && (!route.by_symbol || episodes(c, &k, &symbols, span.0, now)?);
        close(c, &route, &k, span, certify, now)
    }).await?;
    Ok(Ended::Complete)
}

// ---- UTA backfill ----------------------------------------------------------------------------------------------------

/// Our cap on how far back the backfill reads (r3.2): one KuCoin export's span, not a documented limit.
pub const BACKFILL_FLOOR: Duration = Duration::days(365);
/// How long a failed backfill window waits before its next try (r4.1).
const RETRY: Duration = Duration::hours(24);

fn backfill_key(key_id: i64) -> String { format!("rust_sync.kucoin_uta_backfill:{key_id}") }

/// A backfill window: frozen bounds, where its walk stands, and when a failed one may be tried again.
#[derive(Clone, Debug)]
struct Window { start: DateTime<Utc>, end: DateTime<Utc>, last_id: Option<u64>, pages: usize, retry_after: Option<DateTime<Utc>> }

fn windows_json(windows: &[Window], stamp: &Value) -> Value {
    json!({ "credential": stamp, "windows": windows.iter().map(|w| json!({ "start": sql_time(w.start), "end": sql_time(w.end), "last_id": w.last_id.map(|i| i.to_string()),
                                                                            "pages": w.pages, "retry_after": w.retry_after.map(sql_time) })).collect::<Vec<_>>() })
}

/// The windows in progress or waiting, when the record is this credential's. A first record is saved at once, so every
/// later credential finds one. Another credential's record names no windows, and what was read is forgotten too: another
/// key may name another account, whose executions the earlier reads never asked for.
fn load_windows(c: &crate::engine::model::FencedTransaction<'_>, key: &Key, stamp: &Value, now: DateTime<Utc>) -> Result<Vec<Window>, SyncError> {
    let raw: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [backfill_key(key.id)], |r| r.get(0)).optional()?.flatten();
    let Some(v) = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()) else { save_windows(c, key.id, &[], stamp, now)?; return Ok(vec![]) };
    if !crate::engine::model::stamp_current_for(&v["credential"], stamp).is_fresh() {
        hf::remove(c, key, hf::UTA_READ)?;
        save_windows(c, key.id, &[], stamp, now)?;
        return Ok(vec![]);
    }
    let t = |w: &Value, k: &str| w[k].as_str().and_then(|s| crate::codec::parse_time(s).ok());
    Ok(v["windows"].as_array().into_iter().flatten().filter_map(|w| Some(Window { start: t(w, "start")?, end: t(w, "end")?, last_id: w["last_id"].as_str().and_then(|s| s.parse().ok()),
                                                                                  pages: w["pages"].as_u64().unwrap_or(0) as usize, retry_after: t(w, "retry_after") })).collect())
}

fn save_windows(c: &Connection, key_id: i64, windows: &[Window], stamp: &Value, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![backfill_key(key_id), windows_json(windows, stamp).to_string(), sql_time(now)])?;
    Ok(())
}

/// The window this run reads: the newest of a recorded one that is due and the newest unread slice, at most 7 days,
/// of `[floor, top)` that neither a completed read nor a recorded window covers. None: nothing left.
fn pick(windows: &mut Vec<Window>, read: &[Span], (floor, top): Span, now: DateTime<Utc>) -> Option<usize> {
    windows.retain(|w| w.end > floor);
    let due = windows.iter().enumerate().filter(|(_, w)| w.retry_after.is_none_or(|t| t <= now)).max_by_key(|(_, w)| w.end).map(|(i, _)| i);
    let held: Vec<Span> = read.iter().copied().chain(windows.iter().map(|w| (w.start, w.end))).collect();
    let fresh = hf::subtract((floor, top), &held).last().map(|&(a, b)| Window { start: a.max(b - RETENTION), end: b, last_id: None, pages: 0, retry_after: None });
    match (due, fresh) {
        (Some(i), Some(f)) if f.end <= windows[i].end => Some(i),
        (_, Some(f)) => { windows.push(f); Some(windows.len() - 1) }
        (due, None) => due,
    }
}

/// One run's UTA backfill window (r3.3, r4), within half the run's deadline; never the job's failure.
pub(super) async fn backfill<T: Transport>(db: &Db, version: &CredentialVersion, venue: &KucoinVenue<T>, key: &Key, clock: &dyn Clock, wait: std::time::Duration,
                                           run: &super::continuation::Run<'_>) -> Result<(), SyncError> {
    let route = UTA;
    let key = Arc::new(key.clone());
    let stamp = version.cache_stamp();
    let now = clock.now();
    let end = second(now);
    let bounds = (end - BACKFILL_FLOOR, end - RETENTION + MARGIN);
    let (mut windows, done) = { let (k, s) = (key.clone(), stamp.clone()); commit_for(db, version, move |c| Ok((load_windows(c, &k, &s, now)?, hf::spans(c, &k, hf::UTA_READ)?))).await? };
    let done: Vec<Span> = done.into_iter().map(|(_, s)| s).collect();
    let Some(index) = pick(&mut windows, &done, bounds, now) else { return Ok(()) };
    windows[index].retry_after = None;
    let budget = run.deadline / 2;
    let reserve = crate::venue::http::TOTAL_TIMEOUT * (1 + THROTTLE_RETRIES as u32) + wait * THROTTLE_RETRIES as u32 + super::continuation::FINALIZE_FLOOR;
    loop {
        // Paused (or never begun, when the units took the half): the window's progress is saved with its last page; the
        // next run resumes it.
        if run.stopwatch.elapsed() + reserve >= budget { return Ok(()); }
        let w = windows[index].clone();
        let mut query = vec![("tradeType", "SPOT".to_string()), ("startAt", w.start.timestamp_millis().to_string()), ("endAt", (w.end.timestamp_millis() - 1).to_string()),
                             (route.limit.0, route.limit.1.to_string())];
        if let Some(id) = w.last_id { query.push(("lastId", id.to_string())); }
        let outcome = match read(venue, wait, route.path, query).await {
            Err(text) => Err(text),
            Ok(data) => match page(&route, &data, "", (w.start, w.end), w.last_id) {
                Ok(Some(_)) if w.pages >= MAX_PAGES => Err(format!("Too many pages of KuCoin {}", route.path)),
                other => other,
            },
        };
        let (k, s) = (key.clone(), stamp.clone());
        match outcome {
            Ok(None) => {
                windows.remove(index);
                let rest = windows.clone();
                return commit_for(db, version, move |c| { hf::add(c, &k, hf::UTA_READ, (w.start, w.end), now)?; save_windows(c, k.id, &rest, &s, now) }).await;
            }
            Ok(Some((entries, last))) => {
                windows[index] = Window { last_id: Some(last), pages: w.pages + 1, ..w.clone() };
                let saved = windows.clone();
                match commit_for(db, version, move |c| {
                    if let Err(why) = store_page(c, &k, entries, now)? { return Ok(Err(why)); }
                    save_windows(c, k.id, &saved, &s, now)?;
                    Ok(Ok(()))
                }).await? {
                    Ok(()) => {}
                    Err(text) => { windows[index] = w; return retry_later(db, version, &key, windows, index, &stamp, now, text).await; }
                }
            }
            Err(text) => return retry_later(db, version, &key, windows, index, &stamp, now, text).await,
        }
    }
}

/// A failed window waits a day, its cursor kept; the failure is logged only (the backfill certifies nothing).
#[allow(clippy::too_many_arguments)]
async fn retry_later(db: &Db, version: &CredentialVersion, key: &Arc<Key>, mut windows: Vec<Window>, index: usize, stamp: &Value, now: DateTime<Utc>, text: String) -> Result<(), SyncError> {
    crate::engine::log(&format!("[kucoin] UTA backfill of api key {} for {} to {} failed; it tries again in a day: {text}", key.id, sql_time(windows[index].start), sql_time(windows[index].end)));
    windows[index].retry_after = Some(now + RETRY);
    let (k, s) = (key.clone(), stamp.clone());
    commit_for(db, version, move |c| save_windows(c, k.id, &windows, &s, now)).await
}
