//! Each job's last success and last error, durable across restarts, read by `deltabadger check` and by the engine's
//! staleness refusal. Rails owns the schema until 3.0 (spec §2), and Plan 2 rules "no new tables or columns", so the state
//! is one `app_configs` row per job and scope: key `rust_job.<name>` (or `rust_job.<name>:<scope>` for a job registered
//! once per scope, such as a ledger sync per API key), value plain JSON
//! `{"last_run_at":"…Z","last_success_at":"…Z","last_error_at":"…Z","last_error":"…","rails_at":"…Z","incomplete_since":"…Z"}`.
//! The scheduler writes the run fields, one job at a time; a job writes `incomplete_since` before its first write unit
//! (`mark_incomplete`); `rails_at` is written once, at the takeover (engine::staleness::seed).
use crate::app_config;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};

pub const PREFIX: &str = "rust_job.";
/// As ApiKey's SYNC_ERROR_LIMIT (app/models/api_key.rb:83-87): a stored error is at most 200 characters.
pub const ERROR_LIMIT: usize = 200;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobState {
    /// The last run that ended well, whether or not it refreshed anything (the schedule's catch-up reads it).
    pub last_run_at: Option<DateTime<Utc>>,
    /// The last run that refreshed its data completely: an import that published every unit of a non-empty payload
    /// (Codex round 2). A source's freshness reads only this.
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// The newest stamp of the data this job refreshes, as Rails left it when this engine took the install over and
    /// before any run of its own (Plan 2f P1-1): the freshness baseline until the job's first success here.
    pub rails_at: Option<DateTime<Utc>>,
    /// Set by a job before its first write unit, cleared only with `last_success_at` (Codex round 2): while it is set, some
    /// of the source's rows may carry this engine's stamps from an import that did not finish.
    pub incomplete_since: Option<DateTime<Utc>>,
}

impl JobState {
    /// The latest run ended in an error (an error after the last success, or errors only). `None < Some`.
    pub fn failing(&self) -> bool { self.last_error_at > self.last_success_at }

    /// Venue text stays in the stored state, while CLI diagnostics omit its free text.
    pub fn describe_venue(&self)->String { let mut public=self.clone(); if public.last_error.is_some(){public.last_error=Some("venue diagnostic omitted".into())} public.describe() }

    /// One line, for `check` and the staleness refusal.
    pub fn describe(&self) -> String {
        let t = |at: Option<DateTime<Utc>>| at.map_or_else(|| "never".to_string(), |t| t.to_rfc3339_opts(SecondsFormat::Secs, true));
        match &self.last_error {
            Some(e) if self.failing() => format!("last success {}, failing since {}: {e}", t(self.last_success_at), t(self.last_error_at)),
            _ => format!("last success {}", t(self.last_success_at)),
        }
    }
}

/// `rust_job.<job>`, or `rust_job.<job>:<scope>`.
pub fn key(job: &str, scope: Option<&str>) -> String {
    match scope { Some(s) => format!("{PREFIX}{job}:{s}"), None => format!("{PREFIX}{job}") }
}

fn time(v: &Value) -> Option<DateTime<Utc>> { v.as_str()?.parse().ok() }
fn text(t: DateTime<Utc>) -> String { t.to_rfc3339_opts(SecondsFormat::Millis, true) }

/// The row, if this job (and scope) has one. An unreadable value reads as an empty state.
pub fn find(c: &Connection, job: &str, scope: Option<&str>) -> Result<Option<JobState>, String> {
    if !app_config::exists(c, &key(job, scope))? { return Ok(None); }
    let v: Value = app_config::get_plain(c, &key(job, scope))?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    Ok(Some(JobState { last_run_at: time(&v["last_run_at"]), last_success_at: time(&v["last_success_at"]), last_error_at: time(&v["last_error_at"]),
                       last_error: v["last_error"].as_str().map(String::from), rails_at: time(&v["rails_at"]),
                       incomplete_since: time(&v["incomplete_since"]) }))
}

/// A missing row is a job that never ran here.
pub fn read(c: &Connection, job: &str, scope: Option<&str>) -> Result<JobState, String> { Ok(find(c, job, scope)?.unwrap_or_default()) }

fn write(c: &Connection, job: &str, scope: Option<&str>, s: &JobState, now: DateTime<Utc>) -> Result<(), String> {
    let v = json!({ "last_run_at": s.last_run_at.map(text), "last_success_at": s.last_success_at.map(text),
                    "last_error_at": s.last_error_at.map(text), "last_error": s.last_error, "rails_at": s.rails_at.map(text),
                    "incomplete_since": s.incomplete_since.map(text) });
    app_config::set_plain(c, &key(job, scope), &v.to_string(), now)
}

/// A run that refreshed its data completely: it ran, it succeeded, and nothing it wrote is left incomplete.
pub fn record_success(c: &Connection, job: &str, scope: Option<&str>, at: DateTime<Utc>) -> Result<(), String> {
    let mut s = read(c, job, scope)?;
    (s.last_run_at, s.last_success_at, s.incomplete_since) = (Some(at), Some(at), None);
    write(c, job, scope, &s, at)
}

/// A run that ended well but refreshed nothing (an empty payload, a venue it does not serve): the source keeps its age.
pub fn record_run(c: &Connection, job: &str, scope: Option<&str>, at: DateTime<Utc>) -> Result<(), String> {
    let mut s = read(c, job, scope)?;
    s.last_run_at = Some(at);
    write(c, job, scope, &s, at)
}

/// Before an import's first write unit: from now until a complete success, this engine's stamps on the source's rows may
/// be partial. An earlier mark is kept (the source has been incomplete since then).
pub fn mark_incomplete(c: &Connection, job: &str, scope: Option<&str>, at: DateTime<Utc>) -> Result<(), String> {
    let mut s = read(c, job, scope)?;
    if s.incomplete_since.is_some() { return Ok(()); }
    s.incomplete_since = Some(at);
    write(c, job, scope, &s, at)
}

pub fn record_error(c: &Connection, job: &str, scope: Option<&str>, at: DateTime<Utc>, message: &str) -> Result<(), String> {
    let mut s = read(c, job, scope)?;
    s.last_error_at = Some(at);
    s.last_error = Some(message.chars().take(ERROR_LIMIT).collect());
    write(c, job, scope, &s, at)
}

/// Creates the row with Rails' baseline (`rails_at`) when the job has none yet; an existing row is left alone (P1-1: once
/// this engine has run the job, only its completed runs count). True when it created the row.
pub fn seed(c: &Connection, job: &str, rails_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<bool, String> {
    if find(c, job, None)?.is_some() { return Ok(false); }
    write(c, job, None, &JobState { rails_at, ..JobState::default() }, now)?;
    Ok(true)
}

/// Every job's state, whichever plan's job wrote it: `(<job> or <job>:<scope>, state)`, by key.
pub fn all(c: &Connection) -> Result<Vec<(String, JobState)>, String> {
    let mut s = c.prepare("SELECT key FROM app_configs WHERE substr(key, 1, ?1) = ?2 ORDER BY key").map_err(|e| e.to_string())?;
    let keys = s.query_map((PREFIX.len() as i64, PREFIX), |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    keys.into_iter().map(|k| {
        let name = k[PREFIX.len()..].to_string();
        let (job, scope) = match name.split_once(':') { Some((j, s)) => (j.to_string(), Some(s.to_string())), None => (name.clone(), None) };
        read(c, &job, scope.as_deref()).map(|st| (name, st))
    }).collect()
}
