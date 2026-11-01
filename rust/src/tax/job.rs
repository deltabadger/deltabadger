//! Tax::GenerateReportJob: where a report and its refusal live, the request rows (`rust_tax_reports`), and the run.
//!
//! D-TAX-QUEUE: Rails' Solid Queue job is a `rust_tax_reports` row. The slot is Rails' concurrency key
//! `tax_report_<user>_<scope>`: at most one `queued` or `running` row per (user, scope), so an INSERT that meets the
//! partial unique index is Rails' discard on conflict (`Generating.accepted?` false), and `Generating.for?(user)` is a
//! crypto-scope row in either state. A request runs once: a run ends `ready`, `refused` or `failed`, and that row stays as
//! the request's outcome until the next request for its (user, scope) is accepted (which deletes it in the same
//! transaction). Rails' 10-minute lease is the row's lifetime: a run past the job's deadline, or one whose process died,
//! ends `failed` (Rails' killed worker leaves no answer either).
//! A Rails defect fixed here, not copied: `Generating.accepted?` answers true for a job Solid Queue discarded on the
//! conflict (ActiveJob sets `successfully_enqueued` after the adapter returns and the discarded job has no
//! `provider_job_id`), so Rails' web request made while a report runs deletes a true refusal and shows progress for a run
//! that never happens. The INSERT's outcome is the verdict here: not accepted, the refusal comes back.
//!
//! Every touch of a (user, scope)'s report files (a create from the web, REST or MCP, the run's publish, both downloads,
//! the status, the reconcile, the recovery) holds `files(user, scope)`, one in-process lock: web and jobs are one
//! process. Staging names are the request's own (`<path>.stale.<request>` for a create's step aside,
//! `<path>.tmp.<request>` for the run's output), and the run publishes by committing its row's outcome, then renaming
//! the file into place, both under the lock, so no reader sees the step between them.
//!
//! D-TAX-BUDGET: the run is held to the tracker's walk budget (`tracker::jobs::WALK`); past it the run ends as
//! `report_failed`, where Rails would run on.
use crate::jobs::data_api::DataApi;
use crate::jobs::{Cx, Db, Job, JobFuture, Outcome, Retry, Spec, Wake};
use crate::tracker::fx;
use crate::tracker::jobs::{fetching, message, metered, Allowance};
use crate::venue::http::Transport;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Condvar, Mutex, PoisonError};

/// The job's name (what a create wakes).
pub const JOB: &str = "tax_report";
/// Tax::GenerateReportJob::SCOPES.
pub const SCOPES: [&str; 2] = ["crypto", "broker"];
/// Rails' `duration: 10.minutes`, the slot's lease: a run past it ends `failed`.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);

/// `AppPaths.tmp`: APP_TMP_DIR, else the install's `tmp` (Rails.root/tmp; the process runs from Rails.root).
pub fn tmp_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("APP_TMP_DIR").filter(|v| !v.trim().is_empty()).map_or_else(|| PathBuf::from("tmp"), PathBuf::from)
}

/// Tax::GenerateReportJob.report_country.
pub fn report_country(country: &str) -> String { country.chars().filter(char::is_ascii_alphabetic).collect::<String>().to_uppercase() }

/// A scope Rails knows, else crypto (`SCOPES.include?(report_scope.to_s)`).
pub fn scope_of(scope: &str) -> &'static str { if scope == "broker" { "broker" } else { "crypto" } }

/// Tax::GenerateReportJob.report_path: the one place a report's file name is built, sanitised here.
pub fn report_path(tmp: &Path, user: i64, country: &str, year: i64, scope: &str) -> PathBuf {
    tmp.join("tax_reports").join(format!("{user}_{}_{year}_{}.csv", report_country(country), scope_of(scope)))
}

/// Tax::GenerateReportJob.refusal_path: beside the CSV, never at its path.
pub fn refusal_path(tmp: &Path, user: i64, country: &str, year: i64, scope: &str) -> PathBuf {
    with_suffix(&report_path(tmp, user, country, year, scope), ".refused.json")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf { PathBuf::from(format!("{}{suffix}", path.display())) }

/// Tax::GenerateReportJob.refusal: the parsed file, or None (absent, unreadable or not JSON).
pub fn refusal(tmp: &Path, user: i64, country: &str, year: i64, scope: &str) -> Option<Value> {
    let bytes = std::fs::read(refusal_path(tmp, user, country, year, scope)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A refusal's `symbols` joined as `refusal['symbols'].join(', ')` writes them.
pub fn symbols(refusal: &Value) -> String {
    refusal["symbols"].as_array().into_iter().flatten().map(|s| match s { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }).collect::<Vec<_>>().join(", ")
}

/// Tax::GenerateReportJob.refusal_message, in `locale` (text, not yet escaped).
pub fn refusal_message(locale: &str, refusal: &Value) -> String {
    let key = if refusal["reason"] == "excess_return_of_capital" { "tracker.tax_report.refused_excess_return_of_capital" } else { "tracker.tax_report.refused_tokenized" };
    crate::web::i18n::text(locale, key, &[("symbols", crate::web::i18n::Arg::Text(&symbols(refusal)))])
}

// ---- the file lock ---------------------------------------------------------------------------------------------------

static HELD: Mutex<Vec<(i64, &'static str)>> = Mutex::new(Vec::new());
static FREED: Condvar = Condvar::new();

/// `files(user, scope)` held: dropped, the next waiter goes.
pub struct Files { key: (i64, &'static str) }

impl Drop for Files {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        held.retain(|k| *k != self.key);
        FREED.notify_all();
    }
}

/// The one lock over a (user, scope)'s report files. Taken before any SQLite transaction, never inside one, so a
/// holder never waits on a writer that waits on it. Blocking: take it on a blocking thread (`App::db`, `Db::run`).
pub fn files(user: i64, scope: &str) -> Files {
    let key = (user, scope_of(scope));
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    while held.contains(&key) { held = FREED.wait(held).unwrap_or_else(PoisonError::into_inner); }
    held.push(key);
    Files { key }
}

// ---- the request rows ------------------------------------------------------------------------------------------------

/// One `rust_tax_reports` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: i64, pub request: String, pub user_id: i64, pub scope: String, pub country: String, pub year: i64,
    pub stablecoin_as_fiat: bool, pub locale: String, pub state: String,
}

const COLUMNS: &str = "id, request, user_id, scope, country, year, stablecoin_as_fiat, locale, state";

fn read(r: &rusqlite::Row) -> rusqlite::Result<Request> {
    Ok(Request { id: r.get(0)?, request: r.get(1)?, user_id: r.get(2)?, scope: r.get(3)?, country: r.get(4)?, year: r.get(5)?,
                 stablecoin_as_fiat: r.get(6)?, locale: r.get(7)?, state: r.get(8)? })
}

/// A new request's token: 32 lowercase hex characters.
pub fn token() -> String { hex::encode(rand::random::<[u8; 16]>()) }

fn stamp(t: DateTime<Utc>) -> String { crate::codec::format_time(t) }

/// The (user, scope)'s queued or running request.
pub fn active(c: &Connection, user: i64, scope: &str) -> rusqlite::Result<Option<Request>> {
    c.query_row(&format!("SELECT {COLUMNS} FROM rust_tax_reports WHERE user_id = ?1 AND scope = ?2 AND state IN ('queued', 'running')"),
                rusqlite::params![user, scope_of(scope)], read).optional()
}

/// `Generating.for?(user)`: a crypto report queued or running for the account.
pub fn generating(c: &Connection, user: i64) -> rusqlite::Result<bool> { Ok(active(c, user, "crypto")?.is_some()) }

pub fn by_request(c: &Connection, request: &str) -> rusqlite::Result<Option<Request>> {
    c.query_row(&format!("SELECT {COLUMNS} FROM rust_tax_reports WHERE request = ?1"), [request], read).optional()
}

/// What a create asks for, as Rails hands it to `perform_later`.
pub struct Ask<'a> { pub user: i64, pub scope: &'a str, pub country: &'a str, pub year: i64, pub stablecoin_as_fiat: bool, pub locale: &'a str }

/// `perform_later` and the queue's verdict, inside the caller's write transaction: the slot's earlier outcome is
/// replaced by this request, or nothing is written when the slot is taken (false).
pub fn enqueue(tx: &Connection, ask: &Ask, request: &str, now: DateTime<Utc>) -> rusqlite::Result<bool> {
    if active(tx, ask.user, ask.scope)?.is_some() { return Ok(false); }
    tx.execute("DELETE FROM rust_tax_reports WHERE user_id = ?1 AND scope = ?2", rusqlite::params![ask.user, scope_of(ask.scope)])?;
    tx.execute("INSERT INTO rust_tax_reports (request, user_id, scope, country, year, stablecoin_as_fiat, locale, state, created_at) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8)",
               rusqlite::params![request, ask.user, scope_of(ask.scope), ask.country, ask.year, ask.stablecoin_as_fiat, ask.locale, stamp(now)])?;
    Ok(true)
}

/// A file stepped aside under the request's own name, put back or dropped once the create's transaction has ended.
pub struct Aside { from: PathBuf, to: PathBuf }

impl Aside {
    /// `File.rename(path, "#{path}.stale")` when the file exists.
    pub fn step(path: &Path, request: &str) -> std::io::Result<Option<Aside>> {
        if !path.exists() { return Ok(None); }
        let to = with_suffix(path, &format!(".stale.{request}"));
        std::fs::rename(path, &to)?;
        Ok(Some(Aside { from: path.to_path_buf(), to }))
    }
    pub fn restore(self) -> std::io::Result<()> { if self.to.exists() { std::fs::rename(&self.to, &self.from)?; } Ok(()) }
    pub fn drop_stale(self) -> std::io::Result<()> { match std::fs::remove_file(&self.to) { Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e), _ => Ok(()) } }
}

fn remove(path: &Path) -> Result<(), String> {
    // Absent (or under no directory at all) is removed.
    match std::fs::remove_file(path) { Err(e) if path.exists() => Err(format!("cannot remove a tax report file: {e}")), _ => Ok(()) }
}

// ---- the run ---------------------------------------------------------------------------------------------------------

/// What a panel is told: the partial and what it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Panel { Progress(u32), Ready, Refused(Value), Failed }

/// The broadcast that replaces a request's panel (`broadcast_replace_to "user_<id>", :tax_report`).
pub fn broadcast(db: &Db, r: &Request, panel: &Panel) {
    let html = crate::web::tracker::report::panel(&r.locale, r, panel);
    db.notifications.tax_report(r.user_id, &crate::web::turbo::stream("replace", &format!("tax-report-{}", r.request), &html));
}

/// The terminal panel of a finished request, read from its row and its files (None while it is queued or running, or
/// when a refused request's file is gone).
pub fn terminal(tmp: &Path, r: &Request) -> Option<Panel> {
    match r.state.as_str() {
        "ready" => Some(Panel::Ready),
        "failed" => Some(Panel::Failed),
        "refused" => refusal(tmp, r.user_id, &r.country, r.year, &r.scope).map(Panel::Refused),
        _ => None,
    }
}

/// `tax_report`: one request per run, the oldest queued; woken by every accepted create and at start-up, and again by
/// itself while requests wait.
pub struct TaxReport<T: Transport> { pub api: Rc<Option<DataApi<T>>>, pub ecb: Option<fx::Source>, pub tmp: PathBuf }

impl<T: Transport> Job for TaxReport<T> {
    fn spec(&self) -> Spec {
        // A little past the run's own deadline: the run ends itself `failed` first (`perform_within`).
        Spec { name: JOB, scope: None, schedule: None, jitter: crate::jobs::schedule::Jitter::NONE, retry: Retry::None, deadline: DEADLINE + std::time::Duration::from_secs(60) }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            if let Err(e) = recover(&cx.db, &self.tmp).await { return Outcome::Failed(e); }
            let now = cx.clock.now();
            let taken = cx.db.run(move |c, _| take(c, now).map_err(|e| e.to_string())).await;
            let r = match taken { Ok(Some(r)) => r, Ok(None) => return Outcome::NothingNew, Err(e) => return Outcome::Failed(e) };
            let outcome = match tokio::time::timeout(DEADLINE, perform(&cx, (*self.api).as_ref(), self.ecb.as_ref(), &r)).await {
                Ok(outcome) => outcome,
                Err(_) => Err(format!("dropped past its {DEADLINE:?} deadline")),
            };
            // A publish that could not write its file fails the request as a raised run does (Rails' rescue broadcasts
            // report_failed); one that failed after its commit is finished by the next run's recovery.
            let outcome = match outcome { Ok(done) => publish(&cx.db, &self.tmp, &r, done).await, Err(e) => Err(e) };
            let ended = match outcome {
                Ok(()) => Ok(()),
                Err(e) => { crate::engine::log(&format!("[tax] report {} for user {} failed: {e}", r.id, r.user_id)); fail(&cx.db, &self.tmp, &r).await }
            };
            // The next request waits for its own run.
            let more = cx.db.run(|c, _| c.query_row("SELECT EXISTS(SELECT 1 FROM rust_tax_reports WHERE state = 'queued')", [], |x| x.get::<_, bool>(0)).map_err(|e| e.to_string())).await;
            if more.unwrap_or(false) { cx.wakers.wake(JOB, None, None); }
            match ended { Ok(()) => Outcome::Done, Err(e) => Outcome::Failed(e) }
        })
    }
}

/// The oldest queued request, taken (`running`) in one write transaction.
fn take(c: &Connection, now: DateTime<Utc>) -> rusqlite::Result<Option<Request>> {
    let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
    let found = tx.query_row(&format!("SELECT {COLUMNS} FROM rust_tax_reports WHERE state = 'queued' ORDER BY id LIMIT 1"), [], read).optional()?;
    if let Some(r) = &found { tx.execute("UPDATE rust_tax_reports SET state = 'running', started_at = ?2 WHERE id = ?1", rusqlite::params![r.id, stamp(now)])?; }
    tx.commit()?;
    Ok(found.map(|r| Request { state: "running".into(), ..r }))
}

/// `perform` up to the file: the ECB rates, the report's reads, then its enrichment on a blocking thread whose price
/// fetches are made on this one (`tracker::jobs::fetching`), the progress broadcast each time the percent changes.
async fn perform<T: Transport>(cx: &Cx<'_>, api: Option<&DataApi<T>>, ecb: Option<&fx::Source>, r: &Request) -> Result<crate::tax::Outcome, String> {
    // D-TAX-NOT-PORTED-GATE: nothing is queued for a report this build cannot compute; a row that is anyway fails.
    if r.scope != "crypto" || !crate::tax::jurisdictions::ported(&r.country) { return Err(format!("the {} report for {} is not ported", r.scope, r.country)); }
    let today = cx.clock.now().date_naive();
    fx::ensure_loaded(&cx.db, ecb, today).await;
    let mut allowance = Allowance::run();
    let (user, country, year) = (r.user_id, r.country.clone(), i32::try_from(r.year).map_err(|_| "a report year out of range".to_string())?);
    let prepared = metered(&cx.db, &mut allowance, move |c, _| crate::tax::prepare(c, user, &country, year)).await?;
    let report = match prepared { crate::tax::Prepared::Done(outcome) => return Ok(outcome), crate::tax::Prepared::Report(report) => report };
    let (db, request) = (cx.db.clone(), r.clone());
    fetching(&cx.db, api, &mut allowance, move |fetcher| {
        let mut last = 0;
        report.compute(fetcher, today, &mut |percent| {
            if percent != last { last = percent; broadcast(&db, &request, &Panel::Progress(percent)); }
        })
    }).await?.map_err(message)
}

/// The row's outcome committed, then the file renamed into place, under the file lock; then the broadcast. A row that is
/// no longer this run's (`running`) publishes nothing.
async fn publish(db: &Db, tmp: &Path, r: &Request, done: crate::tax::Outcome) -> Result<(), String> {
    let path = report_path(tmp, r.user_id, &r.country, r.year, &r.scope);
    let refused = refusal_path(tmp, r.user_id, &r.country, r.year, &r.scope);
    let (state, body, target, other, panel) = match done {
        crate::tax::Outcome::Csv(csv) => ("ready", csv, path, refused, Panel::Ready),
        crate::tax::Outcome::Refused { reason, symbols } => {
            let refusal = serde_json::json!({ "reason": reason, "symbols": symbols });
            // `JSON.generate`: compact, `/` unescaped, non-ASCII as it is (serde_json writes the same).
            ("refused", refusal.to_string(), refused, path, Panel::Refused(refusal))
        }
    };
    let staged = with_suffix(&target, &format!(".tmp.{}", r.request));
    if let Some(dir) = target.parent() { std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the tax report directory: {e}"))?; }
    std::fs::write(&staged, body).map_err(|e| format!("cannot write the tax report: {e}"))?;
    let (row, published) = (r.clone(), staged.clone());
    let kept = db.run(move |c, _| {
        let _files = files(row.user_id, &row.scope);
        let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let moved = tx.execute("UPDATE rust_tax_reports SET state = ?2, finished_at = ?3 WHERE id = ?1 AND state = 'running'",
                               rusqlite::params![row.id, state, stamp(Utc::now())]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        if moved == 0 { remove(&published)?; return Ok(false); }
        // Publishing and clearing the other kind's file are one step (a stale CSV beside a refusal, or the reverse). The
        // other goes first: a crash between the two leaves `.tmp.<request>` for recovery to finish, never both files.
        remove(&other)?;
        std::fs::rename(&published, &target).map_err(|e| format!("cannot publish the tax report: {e}"))?;
        Ok(true)
    }).await?;
    if kept { broadcast(db, &Request { state: state.into(), ..r.clone() }, &panel); }
    Ok(())
}

/// A run that raised: the row `failed`, its output removed, the failed panel.
async fn fail(db: &Db, tmp: &Path, r: &Request) -> Result<(), String> {
    let (row, tmp) = (r.clone(), tmp.to_path_buf());
    let moved = db.run(move |c, _| {
        let _files = files(row.user_id, &row.scope);
        for target in [report_path(&tmp, row.user_id, &row.country, row.year, &row.scope), refusal_path(&tmp, row.user_id, &row.country, row.year, &row.scope)] {
            remove(&with_suffix(&target, &format!(".tmp.{}", row.request)))?;
        }
        c.execute("UPDATE rust_tax_reports SET state = 'failed', finished_at = ?2 WHERE id = ?1 AND state = 'running'", rusqlite::params![row.id, stamp(Utc::now())])
            .map_err(|e| e.to_string())
    }).await?;
    if moved > 0 { broadcast(db, &Request { state: "failed".into(), ..r.clone() }, &Panel::Failed); }
    Ok(())
}

/// The scheduler's deadline path (`Db::deadline_completion`): every running request ends `failed` and its panel is told.
/// Its output file, if any, is removed by the next run's recovery.
pub async fn deadline(db: &Db) {
    let failed = db.run(|c, _| {
        let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let rows = {
            let mut s = tx.prepare(&format!("SELECT {COLUMNS} FROM rust_tax_reports WHERE state = 'running'")).map_err(|e| e.to_string())?;
            let rows = s.query_map([], read).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
            rows
        };
        tx.execute("UPDATE rust_tax_reports SET state = 'failed', finished_at = ?1 WHERE state = 'running'", [stamp(Utc::now())]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(rows)
    }).await;
    match failed {
        Ok(rows) => for r in rows { broadcast(db, &Request { state: "failed".into(), ..r }, &Panel::Failed); },
        Err(e) => crate::engine::log(&format!("[tax] deadline completion failed: {e}")),
    }
}

/// What a crash can leave, set right before each run (no run is in flight then: the job is registered once, the
/// scheduler runs one job at a time, and the engine lock keeps one process per install). Each step under the files'
/// lock, so a create that is mid-way finishes first. Idempotent.
/// - a `running` row: its process died mid-run. Its output is removed, the row `failed`.
/// - a `ready`/`refused` row with its `.tmp.<request>`: it died between commit and rename; the rename is finished.
/// - any other `.tmp.<request>`: removed.
/// - a `.stale.<request>` with a row: the create committed and died before removing it; removed. With none: the create
///   rolled back or died before committing; renamed back where its file is absent, else removed.
pub async fn recover(db: &Db, tmp: &Path) -> Result<(), String> {
    let tmp = tmp.to_path_buf();
    let failed = db.run(move |c, _| {
        let mut failed = vec![];
        let running = {
            let mut s = c.prepare(&format!("SELECT {COLUMNS} FROM rust_tax_reports WHERE state = 'running'")).map_err(|e| e.to_string())?;
            let rows = s.query_map([], read).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
            rows
        };
        for r in running {
            let _files = files(r.user_id, &r.scope);
            for target in [report_path(&tmp, r.user_id, &r.country, r.year, &r.scope), refusal_path(&tmp, r.user_id, &r.country, r.year, &r.scope)] {
                remove(&with_suffix(&target, &format!(".tmp.{}", r.request)))?;
            }
            let moved = c.execute("UPDATE rust_tax_reports SET state = 'failed', finished_at = ?2 WHERE id = ?1 AND state = 'running'", rusqlite::params![r.id, stamp(Utc::now())])
                .map_err(|e| e.to_string())?;
            if moved > 0 { failed.push(Request { state: "failed".into(), ..r }); }
        }
        let dir = tmp.join("tax_reports");
        let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(failed) };
        let mut staged: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.contains(".stale.") || n.contains(".tmp.")).collect();
        staged.sort();
        for name in staged {
            let Some((kind, target, request)) = parse(&name) else { continue };
            let Some((user, scope)) = owner(target) else { continue };
            let _files = files(user, scope);
            let path = dir.join(&name);
            if !path.exists() { continue; }
            let row = by_request(c, request).map_err(|e| e.to_string())?;
            let target = dir.join(target);
            match (kind, row.as_ref().map(|r| r.state.as_str())) {
                ("tmp", Some(state @ ("ready" | "refused"))) => {
                    let r = row.as_ref().expect("a row");
                    let own = if state == "ready" { report_path(&tmp, r.user_id, &r.country, r.year, &r.scope) } else { refusal_path(&tmp, r.user_id, &r.country, r.year, &r.scope) };
                    if own == target {
                        std::fs::rename(&path, &target).map_err(|e| format!("cannot finish a tax report's publish: {e}"))?;
                        let other = if state == "ready" { refusal_path(&tmp, r.user_id, &r.country, r.year, &r.scope) } else { report_path(&tmp, r.user_id, &r.country, r.year, &r.scope) };
                        remove(&other)?;
                    } else { remove(&path)?; }
                }
                ("tmp", _) => remove(&path)?,
                (_, Some(_)) => remove(&path)?,
                (_, None) if !target.exists() => std::fs::rename(&path, &target).map_err(|e| format!("cannot restore a tax report: {e}"))?,
                (_, None) => remove(&path)?,
            }
        }
        Ok(failed)
    }).await?;
    for r in failed { broadcast(db, &r, &Panel::Failed); }
    Ok(())
}

/// `<target>.<stale|tmp>.<request>` → (kind, target, request).
fn parse(name: &str) -> Option<(&'static str, &str, &str)> {
    for kind in ["stale", "tmp"] {
        if let Some(i) = name.rfind(&format!(".{kind}.")) {
            let request = &name[i + kind.len() + 2..];
            if request.len() == 32 && request.bytes().all(|b| b.is_ascii_hexdigit()) { return Some((kind, &name[..i], request)); }
        }
    }
    None
}

/// `<user>_<country>_<year>_<scope>.csv[.refused.json]` → (user, scope).
fn owner(target: &str) -> Option<(i64, &'static str)> {
    let base = target.strip_suffix(".refused.json").unwrap_or(target).strip_suffix(".csv")?;
    let mut parts = base.split('_');
    let user = parts.next()?.parse().ok()?;
    let scope = parts.nth(2)?;
    SCOPES.contains(&scope).then(|| (user, scope_of(scope)))
}

/// Construct the job for the scheduler.
pub fn job<T: Transport + 'static>(api: Rc<Option<DataApi<T>>>, ecb: Option<fx::Source>, tmp: PathBuf) -> Box<dyn Job> {
    Box::new(TaxReport { api, ecb, tmp })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_names_parse_back_to_their_slot() {
        let req = "0123456789abcdef0123456789abcdef";
        assert_eq!(parse(&format!("7_US_2025_crypto.csv.stale.{req}")), Some(("stale", "7_US_2025_crypto.csv", req)));
        assert_eq!(parse(&format!("7_US_2025_crypto.csv.refused.json.tmp.{req}")), Some(("tmp", "7_US_2025_crypto.csv.refused.json", req)));
        assert_eq!(parse("7_US_2025_crypto.csv.tmp"), None);
        assert_eq!(owner("7_US_2025_crypto.csv.refused.json"), Some((7, "crypto")));
        assert_eq!(owner("7_DE_2025_broker.csv"), Some((7, "broker")));
        assert_eq!(owner("x_DE_2025_broker.csv"), None);
    }

    #[test]
    fn paths_are_sanitised_as_rails_does() {
        let tmp = Path::new("/t");
        assert_eq!(report_path(tmp, 3, "u.s/../", 2025, "nope"), PathBuf::from("/t/tax_reports/3_US_2025_crypto.csv"));
        assert_eq!(refusal_path(tmp, 3, "DE", 2024, "broker"), PathBuf::from("/t/tax_reports/3_DE_2024_broker.csv.refused.json"));
    }
}
