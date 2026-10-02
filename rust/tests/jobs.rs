//! Plan 2f: the in-process scheduler (rust/src/jobs): durable state, schedules, the runner.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed;
use deltabadger::app_config;
use deltabadger::jobs::state::{self, JobState};
use deltabadger::store::{self, Paths};
use rusqlite::Connection;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn db() -> (tempfile::TempDir, Connection) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o.primary)
}

#[test]
fn job_state_is_one_plain_app_configs_row_per_job_that_reads_without_the_keys() {
    let (_d, c) = db();
    let t = at("2026-10-02T10:15:03Z");
    assert_eq!(state::read(&c, "sync_x", None).unwrap(), JobState::default(), "a job that never ran here");
    state::record_error(&c, "sync_x", None, t, &"e".repeat(300)).unwrap();
    let s = state::read(&c, "sync_x", None).unwrap();
    assert!(s.failing());
    assert_eq!(s.last_error.as_deref().map(str::len), Some(state::ERROR_LIMIT));
    state::record_success(&c, "sync_x", None, t + Duration::minutes(1)).unwrap();
    let s = state::read(&c, "sync_x", None).unwrap();
    assert!(!s.failing(), "a success after the error");
    assert!(s.last_error.is_some(), "the last error stays visible");
    assert!(s.describe().starts_with("last success 2026-10-02T10:16:03Z"), "{}", s.describe());
    let raw: String = c.query_row("SELECT value FROM app_configs WHERE key = 'rust_job.sync_x'", [], |r| r.get(0)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).expect("plain JSON, no envelope");
    assert_eq!(v["last_success_at"], "2026-10-02T10:16:03.000Z");
    assert_eq!(state::all(&c).unwrap().into_iter().map(|(n, _)| n).collect::<Vec<_>>(), vec!["sync_x".to_string()]);
}

#[test]
fn app_config_set_is_a_no_op_for_an_unchanged_value_and_encrypts_a_changed_one() {
    let (_d, c) = db();
    let cipher = seed::cipher();
    let updated = |c: &Connection| -> String { c.query_row("SELECT updated_at FROM app_configs WHERE key = 'k'", [], |r| r.get(0)).unwrap() };
    app_config::set(&c, &cipher, "k", "31", at("2026-10-02T10:15:00Z")).unwrap();
    app_config::set(&c, &cipher, "k", "31", at("2026-10-03T10:15:00Z")).unwrap();
    assert_eq!(updated(&c), "2026-10-02 10:15:00", "AppConfig.set of the same value saves nothing");
    app_config::set(&c, &cipher, "k", "32", at("2026-10-03T10:15:00Z")).unwrap();
    assert_eq!(updated(&c), "2026-10-03 10:15:00");
    let raw: String = c.query_row("SELECT value FROM app_configs WHERE key = 'k'", [], |r| r.get(0)).unwrap();
    assert!(raw.contains("\"p\":"), "stored as Rails' encryption envelope: {raw}");
    assert_eq!(app_config::get(&c, &cipher, "k").unwrap().as_deref(), Some("32"));
    assert!(app_config::exists(&c, "k").unwrap());
    assert!(!app_config::exists(&c, "missing").unwrap());
}
