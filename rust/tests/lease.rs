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
    drop(common::rails_install()); // finish the one bin/rails spawn first: a child forked meanwhile inherits the fd below
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

/// PLAN-B2 R1/r1.13 for `run` and `serve`, on a fake clock: the wait is what the closure is asked for, never slept.
/// `during` runs inside the wait, as something else would while the process sleeps.
fn lock_waiting(p: &Paths, start: chrono::DateTime<Utc>, during: impl FnOnce(chrono::DateTime<Utc>))
    -> (Result<lease::EngineLock, LeaseError>, Vec<(i64, u64)>) {
    let (clock, waits) = (std::cell::Cell::new(start), std::cell::RefCell::new(vec![]));
    let result = tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(lease::lock_waiting_once(p, || clock.get(), |ago, wait| {
        waits.borrow_mut().push((ago, wait));
        clock.set(clock.get() + Duration::seconds(wait as i64));
        during(clock.get());
        async {}
    }));
    (result, waits.into_inner())
}

#[test]
fn a_just_stopped_rails_is_waited_out_once_then_checked_again() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    rails_heartbeat(&p, t0());
    // 30 s old: waits 120 - 30 + 1 = 91 s, then takes the lock.
    let (taken, waits) = lock_waiting(&p, t0() + Duration::seconds(30), |_| {});
    assert!(taken.is_ok(), "{:?}", taken.err());
    assert_eq!(waits, vec![(30, 91)]);
    drop(taken);

    // Refreshed during the wait: refused after it, with no second wait.
    let (taken, waits) = lock_waiting(&p, t0() + Duration::seconds(30), |now| rails_heartbeat(&p, now - Duration::seconds(5)));
    assert!(matches!(taken, Err(LeaseError::RailsAlive { seconds_ago: 5 })), "{:?}", taken.err());
    assert_eq!(waits, vec![(30, 91)]);
}

#[test]
fn a_heartbeat_in_the_future_is_refused_at_once() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    rails_heartbeat(&p, t0() + Duration::seconds(10));
    let (taken, waits) = lock_waiting(&p, t0(), |_| {});
    assert!(matches!(taken, Err(LeaseError::RailsAlive { seconds_ago: -10 })), "{:?}", taken.err());
    assert!(waits.is_empty());
}

#[test]
fn a_rails_that_takes_the_lock_during_the_wait_is_refused_as_locked() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    rails_heartbeat(&p, t0());
    let rails = std::cell::RefCell::new(None);
    let (taken, waits) = lock_waiting(&p, t0() + Duration::seconds(100), |_| {
        let f = OpenOptions::new().write(true).open(p.lock_file()).unwrap();
        f.try_lock_shared().expect("the lock is not held during the wait");
        *rails.borrow_mut() = Some(f);
    });
    assert!(matches!(taken, Err(LeaseError::Locked)), "{:?}", taken.err());
    assert_eq!(waits, vec![(100, 21)]);
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

#[test]
fn a_clone_of_the_lock_keeps_the_install_locked_until_the_last_one_drops() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    let first = lease::lock(&p, t0()).unwrap();
    let held = first.clone(); // what `serve` keeps after the engine has taken (and dropped) the first
    drop(first);
    assert!(matches!(lease::lock(&p, t0()), Err(LeaseError::Locked)), "the clone still holds it");
    drop(held);
    assert!(lease::lock(&p, t0()).is_ok(), "released with the last clone");
}

#[test]
fn a_claim_stamps_a_new_ownership_and_a_restart_reopens_nothing() {
    use rusqlite::OptionalExtension;
    let dir = common::rails_install();
    let p = paths(dir.path());
    let l = lease::lock(&p, t0()).unwrap();
    let o = store::open(&p).unwrap();
    let taken = |o: &store::Opened| o.primary.query_row("SELECT value FROM app_configs WHERE key = ?1", [lease::TAKEN_OVER_AT], |r| r.get::<_, String>(0)).optional().unwrap();
    let at = |s: &str| s.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
    lease::claim(&l, &o.primary, &cipher(), "0.1.0", at("2026-10-01T08:00:00Z")).unwrap();
    assert_eq!(taken(&o).as_deref(), Some("2026-10-01T08:00:00.000Z"), "from Rails: stamped with the claim");
    lease::claim(&l, &o.primary, &cipher(), "0.1.0", at("2026-10-01T09:00:00Z")).unwrap();
    assert_eq!(taken(&o).as_deref(), Some("2026-10-01T08:00:00.000Z"), "a restart after a crash continues this ownership");
    // A claim by a build without the stamp crashed: the lease says rust, the window opened by the last handback is still open.
    o.primary.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, '2026-10-01T10:00:00.000Z', '2026-10-01 10:00:00', '2026-10-01 10:00:00')",
                      [lease::HANDED_BACK_AT]).unwrap();
    assert!(matches!(lease::claim(&l, &o.primary, &cipher(), "0.1.0", at("2026-10-01T11:00:00Z")).unwrap(), Claim::AfterCrash));
    assert_eq!(taken(&o).as_deref(), Some("2026-10-01T11:00:00.000Z"), "the restart closes the open window");
}
