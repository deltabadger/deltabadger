//! A venue ledger import that one run does not finish (plan VB §1.6, the D2 rules): the walk is a sequence of sources
//! (KuCoin: fills in weekly windows, deposits, withdrawals) read a page at a time, and a run stops before the job's
//! deadline (`LEDGER_DEADLINE`) could cut it, stores what it read, records where it stopped and wakes itself. The run
//! that reaches the end writes the watermark. Alpaca's own import (`ledger::Import`) keeps its policy.
//!
//! - **Admission.** The first page of a run is always read (otherwise nothing ever progresses). Every later page is read
//!   only while `elapsed + request + page + finalize < deadline`: the request's worst case (the transport's whole budget
//!   per try and the venue's throttle waits), the page's worst-case rows at the measured store cost, and the rows read
//!   but not stored yet plus the finishing step. Each term is a conservative estimate, never a guarantee: correctness
//!   rests on replay, below, not on the estimate holding.
//! - **DB cost** is measured end to end, around each `db.run` (queue, connection, write lock and commit included): the
//!   cost of a unit is twice the longest of the last `UNIT_WINDOW` units, never below `UNIT_FLOOR`, plus `WRITE_GAP`. The
//!   window persists in the record, so a run's estimate starts from what earlier runs met.
//! - **Checkpoints.** What a run read is stored after its reads, in units of at most `BATCH` rows; the cursor (and what
//!   the record carries) is written in the same transaction as the last unit of each page that holds rows, past the
//!   empty pages after it. Pages are stored only while their units fit before the deadline (the first always).
//! - **Replay.** The runner may still cut a run at an await. The cursor only moves with the last unit of a page, rows are
//!   stored by their identity (`ledger::store`'s duplicate rules: a tx_id, or content counted one for one) and the
//!   watermark moves only on completion, so a cut run replays from its last checkpoint, re-reading at most what it had
//!   not committed and storing nothing twice.
//! - **Progress.** A run always stores its first page, so a run that ends by itself has progressed. A run cut before its
//!   first checkpoint has not: each run of a recorded import is counted before it reads, and one that starts after `STALLS`
//!   runs in a row checkpointed nothing (a first page that alone outlasts the deadline: a venue defect to look at) fails
//!   with a sync error instead of reading, and the count starts over. A cut run never wakes another. (A fresh import cut
//!   before its first checkpoint has no record: the next sync starts it again, as Rails' single read would.)
//! - **Chains.** A run that leaves more to read wakes the next one, at most `CHAIN_RUNS` in a row. Past that the key's
//!   `last_sync_error` says the import is still running and the next scheduled sync (or order event) resumes from the
//!   cursor. Nothing ever restarts an import from scratch, except a new credential (the record is discarded, #511) or a
//!   handback that moved the watermark.
use super::activities::Entry;
use super::ledger::{self, Outcome, Progress};
use super::{phase, record_sync_error, sql_time, Failure, Key, SyncError, WRITE_GAP};
use crate::crypto::Credentials;
use crate::engine::model::CredentialVersion;
use crate::jobs::Db;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

/// Runs one chain may take back to back: at most a day of runs at the 3600 s deadline.
pub const CHAIN_RUNS: usize = 24;
/// Runs in a row that may checkpoint nothing before the next one fails at its start.
pub const STALLS: usize = 2;
pub const UNIT_FLOOR: Duration = Duration::from_secs(2);
pub const PAGE_FLOOR: Duration = Duration::from_secs(10);
pub const FINALIZE_FLOOR: Duration = Duration::from_secs(60);
pub const UNIT_WINDOW: usize = 8;
/// Rows per write unit, as the stores of every ledger sync.
pub const BATCH: usize = 100;
/// The most rows one run reads before it stores them (the reads' memory).
pub const MAX_RUN_ROWS: usize = 500_000;

const STALLED: &str = "the ledger import completed no page in its last runs; it continues from where it stopped at the next sync";

/// How long the run has been going: the wall clock in production, a scripted one in tests. `unit_begun` and `unit_done`
/// bracket each timed database unit (a test's scripted delay, or its cut).
pub trait Stopwatch { fn elapsed(&self) -> Duration; fn unit_begun(&self) {} fn unit_done(&self) {} }
pub struct Wall(std::time::Instant);
impl Wall { pub fn start() -> Self { Self(std::time::Instant::now()) } }
impl Stopwatch for Wall { fn elapsed(&self) -> Duration { self.0.elapsed() } }

/// What bounds one run.
pub struct Run<'a> { pub deadline: Duration, pub chain_runs: usize, pub stopwatch: &'a dyn Stopwatch }

/// One venue's walk, a page at a time. `step` reads the next page (its requests, a throttle's retries included) and
/// moves the cursor past it; `Ok(rows)` may be empty (an empty window, a range halved).
pub(super) trait Walk {
    fn done(&self) -> bool;
    fn cursor(&self) -> Value;
    /// The most rows the next page can hold.
    fn page_rows(&self) -> usize;
    /// The longest the next page's requests can take: every try at the transport's whole budget, and every wait.
    fn request_reserve(&self) -> Duration;
    async fn step(&mut self) -> Result<Vec<Entry>, (String, bool)>;
}

/// Where a walk begins: a fresh import from `start` (None: the venue's whole history) to `end`, or a recorded cursor.
pub(super) struct Begin<'a> { pub start: Option<DateTime<Utc>>, pub end: DateTime<Utc>, pub cursor: Option<&'a Value> }

/// The import's record: `app_configs` row `rust_sync.venue_ledger:<api_key_id>`, plain JSON. Rails never reads it.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    credential_origin: Value,
    /// The key's `last_synced_at` when the import began (microseconds): a record whose key moved since is ignored.
    watermark: Option<i64>,
    start: Option<DateTime<Utc>>,
    /// When the import's first run began: the walk's end, and what the watermark may not pass.
    started: DateTime<Utc>,
    cursor: Value,
    runs: usize,
    chain: usize,
    stalls: usize,
    max_seen: Option<DateTime<Utc>>,
    min_skipped: Option<DateTime<Utc>>,
    /// The last units' end-to-end times, in milliseconds.
    units: Vec<u64>,
    /// The id-less rows this import stored, and how many id-less entries of each identity it read (`Progress::stored`
    /// and `repeats` for those rows): rows an earlier run of the import stored are no evidence against a later entry.
    idless_stored: Vec<i64>,
    idless_repeats: Vec<(ledger::EventKey, usize)>,
}

fn record_key(key_id: i64) -> String { format!("rust_sync.venue_ledger:{key_id}") }
fn micros(t: Option<DateTime<Utc>>) -> Option<i64> { t.map(|t| t.timestamp_micros()) }
fn time(v: &Value) -> Option<DateTime<Utc>> { v.as_i64().and_then(DateTime::<Utc>::from_timestamp_micros) }

fn event_json(k: &ledger::EventKey) -> Value { json!([k.0, k.1, k.2.to_s_f(), micros(k.3)]) }
fn event_of(v: &Value) -> Option<ledger::EventKey> {
    Some((v[0].as_i64()?, v[1].as_str().map(str::to_string), crate::ruby::BigDec::parse(v[2].as_str()?).ok()?, time(&v[3])))
}

impl Record {
    fn json(&self) -> Value {
        json!({ "credential_origin": self.credential_origin, "watermark": self.watermark, "start": micros(self.start), "started": self.started.timestamp_micros(),
                "cursor": self.cursor, "runs": self.runs, "chain": self.chain, "stalls": self.stalls, "max_seen": micros(self.max_seen),
                "min_skipped": micros(self.min_skipped), "units": self.units, "idless_stored": self.idless_stored,
                "idless_repeats": self.idless_repeats.iter().map(|(k, n)| json!([event_json(k), n])).collect::<Vec<_>>() })
    }
}

/// The key's record when it still describes this key: the same credential and the same watermark (a handback that moved
/// it leaves the record behind). Anything else is ignored, and replaced or removed by this import.
fn load(c: &Connection, key: &Key, version: &CredentialVersion) -> Result<Option<Record>, SyncError> {
    let raw: Option<Option<String>> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [record_key(key.id)], |r| r.get(0)).optional()?;
    let Some(v) = raw.flatten().and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { return Ok(None) };
    let Some(started) = time(&v["started"]) else { return Ok(None) };
    if !crate::engine::model::stamp_current_for(&v["credential_origin"], &version.cache_stamp()).is_fresh() || v["watermark"].as_i64() != micros(key.last_synced_at) { return Ok(None); }
    let count = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    Ok(Some(Record {
        credential_origin: v["credential_origin"].clone(), watermark: v["watermark"].as_i64(), start: time(&v["start"]), started, cursor: v["cursor"].clone(),
        runs: count("runs"), chain: count("chain"), stalls: count("stalls"), max_seen: time(&v["max_seen"]), min_skipped: time(&v["min_skipped"]),
        units: v["units"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect(),
        idless_stored: v["idless_stored"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect(),
        idless_repeats: v["idless_repeats"].as_array().into_iter().flatten().filter_map(|p| Some((event_of(&p[0])?, p[1].as_u64()? as usize))).collect(),
    }))
}

fn save(c: &Connection, key_id: i64, record: &Record, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![record_key(key_id), record.json().to_string(), sql_time(now)])?;
    Ok(())
}

/// What admission and the store phase know of the database's cost.
struct Budget<'a> { run: &'a Run<'a>, units: VecDeque<Duration> }

impl Budget<'_> {
    fn unit(&self) -> Duration { self.units.iter().max().map_or(UNIT_FLOOR, |m| (*m * 2).max(UNIT_FLOOR)) + WRITE_GAP }
    fn rows(&self, units: usize) -> Duration { self.unit() * units as u32 }
    fn fits(&self, cost: Duration) -> bool { self.run.stopwatch.elapsed() + cost < self.run.deadline }
    async fn timed<T>(&mut self, unit: impl std::future::Future<Output = T>) -> T {
        let at = self.run.stopwatch.elapsed();
        self.run.stopwatch.unit_begun();
        let out = unit.await;
        self.run.stopwatch.unit_done();
        if self.units.len() == UNIT_WINDOW { self.units.pop_front(); }
        self.units.push_back(self.run.stopwatch.elapsed().saturating_sub(at));
        out
    }
}

fn units_of(rows: usize) -> usize { rows.div_ceil(BATCH).max(1) }

/// Pages read and not stored yet, as the units they are stored in: a page with rows, and the empty pages after it (their
/// cursor rides its checkpoint); empty pages before any rows make a unit of their own only when nothing follows them.
#[derive(Default)]
struct Read { segments: Vec<(Vec<Entry>, Value)>, units: usize }
impl Read {
    fn push(&mut self, rows: Vec<Entry>, cursor: Value) {
        match self.segments.last_mut() {
            Some((_, at)) if rows.is_empty() => *at = cursor,
            Some((last, at)) if last.is_empty() => { *last = rows; *at = cursor; }
            _ => self.segments.push((rows, cursor)),
        }
        self.units = self.segments.iter().map(|(r, _)| units_of(r.len())).sum();
    }
    fn rows(&self) -> usize { self.segments.iter().map(|(r, _)| r.len()).sum() }
}

/// One run of a venue's ledger import for `key` (already loaded and fenced by the caller's `version`). `watermark` is the
/// key's watermark as this credential produced it (None: read the whole history); `overlap` how far before it a fresh
/// import starts; `make` builds the walk.
#[allow(clippy::too_many_arguments)]
pub(super) async fn sync<W: Walk>(db: &Db, version: &CredentialVersion, key: Key, watermark: Option<DateTime<Utc>>, overlap: chrono::Duration, credentials: &Credentials,
                                  now: DateTime<Utc>, run: &Run<'_>, make: impl FnOnce(Begin<'_>) -> W) -> Result<Result<Outcome, Failure>, SyncError> {
    let key_id = key.id;
    let user_id = key.user_id;
    let raw_watermark = micros(key.last_synced_at);
    let resumed = { let (k, v) = (key.clone(), version.clone()); phase(db, move |c| load(c, &k, &v)).await? };
    let fresh = resumed.is_none();
    let mut record = resumed.unwrap_or_else(|| Record {
        credential_origin: version.cache_stamp(), watermark: raw_watermark, start: watermark.map(|w| w.min(now) - overlap), started: now, cursor: Value::Null,
        runs: 0, chain: 0, stalls: 0, max_seen: None, min_skipped: None, units: vec![], idless_stored: vec![], idless_repeats: vec![],
    });
    let fail = |text: String, raised: bool, record: Option<Record>| {
        let creds = credentials.clone();
        async move {
            let error = super::commit_for(db, version, move |c| {
                if let Some(r) = &record { save(c, key_id, r, now)?; }
                record_sync_error(c, key_id, &text, &creds)
            }).await?;
            if !raised { ledger::link_transfers_for(db, user_id, Some(version), now).await?; }
            Ok::<_, SyncError>(Err(Failure { error, raised }))
        }
    };
    if !fresh {
        // Counted before anything is read, so a run the deadline cuts counts too.
        if record.stalls >= STALLS { return fail(STALLED.into(), false, Some(Record { stalls: 0, chain: 0, ..record })).await; }
        record = Record { runs: record.runs + 1, chain: record.chain + 1, stalls: record.stalls + 1, ..record };
        let counted = record.clone();
        super::commit_for(db, version, move |c| save(c, key_id, &counted, now)).await?;
    } else {
        record.runs = 1;
        record.chain = 1;
    }
    let mut walk = make(Begin { start: record.start, end: record.started, cursor: (!fresh).then_some(&record.cursor) });
    let mut budget = Budget { run, units: record.units.iter().map(|ms| Duration::from_millis(*ms)).collect() };

    // Read, page by page, while the next page and everything read so far still fit.
    let mut read = Read::default();
    let mut first = true;
    while !walk.done() {
        let cost = walk.request_reserve() + budget.rows(units_of(walk.page_rows())).max(PAGE_FLOOR) + (budget.rows(read.units + 1)).max(FINALIZE_FLOOR);
        if !first && (!budget.fits(cost) || read.rows() + walk.page_rows() > MAX_RUN_ROWS) { break; }
        first = false;
        match walk.step().await {
            Ok(rows) => read.push(rows, walk.cursor()),
            // A fresh import fails as Rails' single read does: nothing it read is stored. A resumed one keeps its record.
            Err((text, raised)) => return fail(text, raised, (!fresh).then_some(record)).await,
        }
    }
    let complete = walk.done();

    // Store, a page (with the empty pages after it) at a time, each checkpointed in its last unit.
    let key = Arc::new(key);
    let mut progress = {
        let k = key.clone();
        phase(db, move |c| Ok(Progress::new(ledger::asset_rule(c, k.exchange_id, &k.exchange_type)?, ledger::stored_merged(c, &k)?, Arc::new(vec![])))).await?
    };
    progress.stored.extend(record.idless_stored.iter().copied());
    progress.repeats.extend(record.idless_repeats.iter().cloned());
    let segments = read.segments.len();
    let mut stored_segments = 0;
    for (i, (rows, cursor)) in read.segments.into_iter().enumerate() {
        let last = i + 1 == segments;
        // A complete run's last pages, when they hold no rows, need no checkpoint: the finishing unit removes the record.
        if rows.is_empty() && last && complete { stored_segments += 1; break; }
        if stored_segments > 0 && !budget.fits(budget.rows(units_of(rows.len())) + FINALIZE_FLOOR) { break; }
        let max_seen = rows.iter().filter_map(|e| e.transacted_at).max();
        let idless: Vec<ledger::EventKey> = rows.iter().filter(|e| e.tx_id.as_deref().is_none_or(crate::ruby::blank))
            .map(|e| (e.entry_type, e.base_currency.clone(), e.base_amount.clone(), e.transacted_at)).collect();
        let batch = Arc::new(rows);
        progress = progress.over(batch.clone());
        let before: HashSet<i64> = progress.stored.clone();
        let mut from = 0;
        loop {
            let to = batch.len().min(from + BATCH);
            let (k, p) = (key.clone(), progress);
            let checkpoint = (to == batch.len()).then(|| {
                let mut r = record.clone();
                r.cursor = cursor.clone();
                r.stalls = 0;
                r.max_seen = [r.max_seen, max_seen].into_iter().flatten().max();
                r.units = budget.units.iter().map(|d| d.as_millis() as u64).collect();
                for e in &idless {
                    match r.idless_repeats.iter_mut().find(|(k, _)| k == e) { Some((_, n)) => *n += 1, None => r.idless_repeats.push((e.clone(), 1)) }
                }
                r
            });
            let before = before.clone();
            let (p, r) = budget.timed(super::commit_for(db, version, move |c| {
                let p = ledger::store(c, &k, from..to, None, p, now)?;
                let Some(mut r) = checkpoint else { return Ok((p, None)) };
                r.min_skipped = [r.min_skipped, p.min_skipped].into_iter().flatten().min();
                let added: Vec<i64> = p.stored.difference(&before).copied().collect();
                if !added.is_empty() {
                    let mut s = c.prepare(&format!("SELECT id FROM account_transactions WHERE tx_id IS NULL AND id IN ({})", added.iter().map(i64::to_string).collect::<Vec<_>>().join(",")))?;
                    for id in s.query_map([], |row| row.get::<_, i64>(0))? { r.idless_stored.push(id?); }
                }
                save(c, key_id, &r, now)?;
                Ok((p, Some(r)))
            })).await?;
            progress = p;
            if let Some(r) = r { record = r; }
            from = to;
            if from >= batch.len() { break; }
        }
        stored_segments += 1;
    }
    ledger::resolve_recent(db, version, &key, progress.assets.clone(), now).await?;
    let mut outcome = progress.out;
    let finished = complete && stored_segments == segments;
    if !finished {
        if record.chain >= run.chain_runs {
            let text = format!("the ledger import is still running after {} runs; it continues from where it stopped at the next sync", record.runs);
            return fail(text, false, Some(Record { chain: 0, ..record })).await;
        }
        crate::engine::log(&format!("[{}] ledger import of api key {key_id} is not complete after {} runs; the next run continues",
                                    crate::tracker::rows::name_id(&key.exchange_type), record.runs));
        return Ok(Ok(outcome));
    }
    // The newest row read, held back by a row that could not be saved, never past the import's start; else the last one.
    let min_skipped = [record.min_skipped, progress.min_skipped].into_iter().flatten().min();
    let max_seen = record.max_seen;
    let watermark = [max_seen, min_skipped].into_iter().flatten().min().or(key.last_synced_at).map(|w| w.min(record.started));
    let changed = micros(watermark) != raw_watermark || key.last_sync_error.is_some();
    let producer = version.clone();
    super::commit_for(db, version, move |c| {
        super::cache::record_ledger(c, key_id, &producer, now)?;
        if changed { c.execute("UPDATE api_keys SET last_synced_at = ?1, last_sync_error = NULL, updated_at = ?2 WHERE id = ?3", params![watermark.map(sql_time), sql_time(now), key_id])?; }
        c.execute("DELETE FROM app_configs WHERE key = ?1", [record_key(key_id)])?;
        Ok(())
    }).await?;
    outcome.watermark = watermark;
    outcome.complete = true;
    outcome.linked = ledger::link_transfers_for(db, user_id, Some(version), now).await?;
    Ok(Ok(outcome))
}
