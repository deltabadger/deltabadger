mod common;
use chrono::{Duration, TimeZone, Utc};
use deltabadger::crypto::{Cipher, EncryptionKeys};
use deltabadger::lease::{self, Claim, LeaseError};
use deltabadger::store::{self, Paths};
use std::fs::OpenOptions;

fn t0() -> chrono::DateTime<Utc> { Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap() }
fn cipher() -> Cipher { Cipher::new(&EncryptionKeys::resolve(&|_| None, "lease-test-secret").unwrap()) }
fn paths(dir: &std::path::Path) -> Paths { Paths::from_env(&|_| None, dir) }

fn rails_heartbeat(p: &Paths, at: chrono::DateTime<Utc>) {
    rusqlite::Connection::open(&p.queue).unwrap().execute(
        "INSERT INTO solid_queue_processes (kind, last_heartbeat_at, pid, hostname, created_at, name) VALUES ('Worker', ?1, 1, 'h', ?1, 'w')",
        [deltabadger::codec::format_time(at)]).unwrap();
}

#[test]
fn the_lock_is_taken_before_any_database_exists_and_creates_none() {
    let dir = tempfile::tempdir().unwrap();
    let _l = lease::lock(&paths(dir.path()), t0()).unwrap();
    assert!(!dir.path().join("production.sqlite3").exists());
    assert!(!dir.path().join("production_queue.sqlite3").exists());
}

#[test]
fn a_second_rust_process_is_locked_out() {
    let dir = tempfile::tempdir().unwrap();
    let _l = lease::lock(&paths(dir.path()), t0()).unwrap();
    assert!(matches!(lease::lock(&paths(dir.path()), t0()), Err(LeaseError::Locked)));
}

#[test]
fn a_rails_process_holding_the_shared_lock_blocks_rust() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let rails = OpenOptions::new().write(true).create(true).truncate(false).open(p.lock_file()).unwrap();
    rails.try_lock_shared().unwrap(); // what config/initializers/00_engine_lock.rb holds for its lifetime
    assert!(matches!(lease::lock(&p, t0()), Err(LeaseError::Locked)));
    drop(rails);
    assert!(lease::lock(&p, t0()).is_ok());
}

#[test]
fn a_live_pre_floor_rails_blocks_until_its_heartbeat_is_stale() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    rails_heartbeat(&p, t0());
    assert!(matches!(lease::lock(&p, t0() + Duration::seconds(119)), Err(LeaseError::RailsAlive { seconds_ago: 119 })));
    assert!(lease::lock(&p, t0() + Duration::seconds(121)).is_ok());
}

#[test]
fn claim_records_rust_ownership_encrypted_and_names_where_it_came_from() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    let l = lease::lock(&p, t0()).unwrap();
    let o = store::open(&p).unwrap();
    assert!(matches!(lease::claim(&l, &o.primary, &cipher(), "0.1.0", t0()).unwrap(), Claim::FromRails));
    let raw: String = o.primary.query_row("SELECT value FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get(0)).unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(envelope["p"].is_string() && envelope["h"]["iv"].is_string(), "stored as a Rails envelope");
    assert_eq!(lease::read(&o.primary, &cipher()).unwrap().unwrap()["engine"], "rust");
    assert!(matches!(lease::claim(&l, &o.primary, &cipher(), "0.1.0", t0()).unwrap(), Claim::AfterCrash));
}

#[test]
fn hand_back_is_recorded_and_the_next_claim_sees_it() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    let l = lease::lock(&p, t0()).unwrap();
    let o = store::open(&p).unwrap();
    lease::claim(&l, &o.primary, &cipher(), "0.1.0", t0()).unwrap();
    lease::hand_back(&l, &o.primary, &cipher(), t0()).unwrap();
    let v = lease::read(&o.primary, &cipher()).unwrap().unwrap();
    assert_eq!((v["engine"].as_str(), v["released_by"].as_str(), v["handed_back"].as_bool()), (Some("none"), Some("rust"), Some(true)));
    assert!(matches!(lease::claim(&l, &o.primary, &cipher(), "0.1.0", t0()).unwrap(), Claim::AfterHandback));
}

#[test]
fn an_unreadable_row_is_refused_not_overwritten() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    let l = lease::lock(&p, t0()).unwrap();
    let o = store::open(&p).unwrap();
    let foreign = Cipher::new(&EncryptionKeys::resolve(&|_| None, "another-install").unwrap());
    lease::claim(&l, &o.primary, &foreign, "0.1.0", t0()).unwrap();
    assert!(matches!(lease::claim(&l, &o.primary, &cipher(), "0.1.0", t0()), Err(LeaseError::Unreadable)));
    assert!(lease::read(&o.primary, &foreign).unwrap().is_some(), "left as it was");
}
