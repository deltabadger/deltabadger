mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::{handover, model, placement, EngineError};
use deltabadger::lease::{self, Claim};
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{FakeFactory, FakeVenue};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::process::Command;

fn now() -> DateTime<Utc> { "2026-09-30T12:00:00Z".parse().unwrap() }
fn job(q: &rusqlite::Connection, class: &str, gid: &str) {
    q.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, priority, created_at, updated_at) VALUES ('kraken', ?1, ?2, 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00')",
              rusqlite::params![class, json!({ "job_class": class, "arguments": [{ "_aj_globalid": gid }] }).to_string()]).unwrap();
    let id = q.last_insert_rowid();
    q.execute("INSERT INTO solid_queue_scheduled_executions (job_id, queue_name, priority, scheduled_at, created_at) VALUES (?1, 'kraken', 0, '2026-10-07 10:00:00', '2026-09-30 11:00:00')", [id]).unwrap();
}
fn count(c: &rusqlite::Connection, sql: &str) -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() }

#[test]
fn takeover_claims_the_install_removes_only_its_bots_rails_jobs_and_unsticks_executing() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let mine = seed::insert_bot(&o.primary, &s, &BotSpec { status: 4, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let crashed_after_waiting = seed::insert_bot(&o.primary, &s, &BotSpec { status: 6, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    job(&o.queue, "Bot::ActionJob", &format!("gid://deltabadger/Bots::DcaMultiAsset/{mine}"));
    job(&o.queue, "AccountTransaction::SyncJob", "gid://deltabadger/ApiKey/1");
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert!(matches!(t.claim, Claim::FromRails));
    assert_eq!((t.deleted_jobs, t.normalised), (1, 2));
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 1, "only the bot's job goes");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_scheduled_executions"), 1, "the bot job's execution cascades; the other job's stays");
    for id in [mine, crashed_after_waiting] {
        assert_eq!(model::load_bot(&o.primary, id).unwrap().status, deltabadger::enums::BotStatus::Scheduled, "a tick interrupted after executing or waiting resumes");
    }
}

#[test]
fn an_ineligible_install_is_refused_before_anything_is_written() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limited", json!(true)));
    job(&o.queue, "Bot::ActionJob", &format!("gid://deltabadger/Bots::DcaMultiAsset/{b}"));
    assert!(matches!(handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()), Err(EngineError::Ineligible(_))));
    assert!(lease::read(&o.primary, &seed::cipher()).unwrap().is_none(), "no claim");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 1, "Rails' job untouched");
}

#[test]
fn a_takeover_repeated_after_a_failure_following_the_claim_finishes_the_job() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec { status: 4, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    // The state a crash right after the claim leaves: lease is Rust's, job still queued, bot still executing.
    deltabadger::lease::claim(&l, &o.primary, &seed::cipher(), "0.2.0", now()).unwrap();
    job(&o.queue, "Bot::ActionJob", &format!("gid://deltabadger/Bots::DcaMultiAsset/{b}"));
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert!(matches!(t.claim, Claim::AfterCrash));
    assert_eq!((t.deleted_jobs, t.normalised), (1, 1));
    let again = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((again.deleted_jobs, again.normalised), (0, 0), "idempotent");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs WHERE class_name LIKE 'Bot::%'"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_order_blocks_handback_and_rails_keeps_refusing() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let bot = model::load_bot(&o.primary, b).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let px = deltabadger::ruby::BigDec::from_i64(50_000);
    let deltabadger::engine::amount::Sizing::Place(plan) = deltabadger::engine::amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60),
        &px, deltabadger::engine::venue_rules::KRAKEN.minimum_logic) else { panic!() };
    placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap();
    let factory = FakeFactory(FakeVenue::new().lookup_fails(1));
    assert!(matches!(handover::hand_back(&l, &o, &factory, &seed::cipher(), &deltabadger::engine::FixedClock(now())).await, Err(EngineError::Unresolved(ref v)) if v == &vec![b]));
    assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust", "still Rust's: Rails must refuse");
    drop((l, o));
    let (booted, stderr) = rails_boot(dir.path());
    assert!(!booted && stderr.contains("still owns this data"), "Rails refuses because the lease says rust: {stderr}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_completed_handback_is_adopted_by_rails() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap(), 1);
    assert_eq!(model::load_bot(&o.primary, b).unwrap().status, deltabadger::enums::BotStatus::Scheduled);
    drop((l, o));
    assert!(rails_boot(dir.path()).0, "Rails starts after a completed handback");
    let q = rusqlite::Connection::open(dir.path().join("production_queue.sqlite3")).unwrap();
    assert_eq!(count(&q, "SELECT count(*) FROM solid_queue_jobs WHERE class_name = 'Bot::RepairOrphanedBotsJob'"), 1, "adopted: repair enqueued");
}

/// Boots the real Rails app (development env, not test, so the engine-lease initializers run) against this
/// install's files, with the test cipher's keys, and reports whether it started.
fn rails_boot(dir: &std::path::Path) -> (bool, String) {
    let keys = deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let out = Command::new(root.join("bin/rails")).current_dir(root).args(["runner", "puts :booted"])
        .env("RAILS_ENV", "development").env_remove("DATABASE_URL")
        .env("PRIMARY_DATABASE_URL", format!("sqlite3:{}", dir.join("production.sqlite3").display()))
        .env("QUEUE_DATABASE_URL", format!("sqlite3:{}", dir.join("production_queue.sqlite3").display()))
        .env("CACHE_DATABASE_URL", format!("sqlite3:{}/cache.sqlite3", scratch.path().display()))
        .env("CABLE_DATABASE_URL", format!("sqlite3:{}/cable.sqlite3", scratch.path().display()))
        .env("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", keys.primary_key).env("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", keys.key_derivation_salt)
        .output().expect("bin/rails runs");
    (out.status.success() && String::from_utf8_lossy(&out.stdout).contains("booted"), String::from_utf8_lossy(&out.stderr).into_owned())
}
