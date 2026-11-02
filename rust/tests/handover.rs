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
fn takeover_claims_the_install_removes_its_bots_rails_jobs_drops_the_rest_and_unsticks_executing() {
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
    // D-UPGRADE-RAILS-WORK-DROPPED: the sync has nothing left to finish (the scheduler runs its own), so it goes, counted.
    assert_eq!((t.deleted_jobs, t.dropped_jobs, t.normalised), (2, 1, 2));
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0, "the bot's job and the dropped sync go");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_scheduled_executions"), 0, "their executions cascade");
    assert_eq!(deltabadger::app_config::get_plain(&o.primary, handover::QUEUE_DONE_KEY).unwrap(), None, "the set empties once the rows are gone");
    for id in [mine, crashed_after_waiting] {
        assert_eq!(model::load_bot(&o.primary, id).unwrap().status, deltabadger::enums::BotStatus::Scheduled, "a tick interrupted after executing or waiting resumes");
    }
}

/// PLAN-B2 R3 (D-UPGRADE-SET-ASIDE): a working bot this build refuses is stopped with its reasons, its Rails job goes as
/// for every bot the engine now owns, and the install is taken over.
#[test]
fn an_ineligible_working_bot_is_set_aside_and_the_install_is_taken_over() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("future_price_drop_limited", json!(true)));
    job(&o.queue, "Bot::ActionJob", &format!("gid://deltabadger/Bots::DcaMultiAsset/{b}"));
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(t.set_aside.iter().map(|a| a.id).collect::<Vec<_>>(), vec![b]);
    assert!(t.eligible.is_empty());
    let bot = model::load_bot(&o.primary, b).unwrap();
    assert_eq!(bot.status, deltabadger::enums::BotStatus::Stopped);
    assert!(deltabadger::engine::set_aside::reasons(bot.transient.get(deltabadger::engine::set_aside::KEY)).is_some_and(|r| r.len() == 1), "{:?}", bot.transient);
    assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0, "the stopped bot's Rails job goes");
}

/// PLAN-B2 R4, r1.4: an ineligible bot with work in flight cannot be set aside (row 26): the takeover refuses with C6, and
/// its transaction rolls back: nothing is stopped, claimed or deleted.
#[test]
fn an_ineligible_bot_with_work_in_flight_is_a_hard_stop_and_nothing_is_written() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let healthy = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2099-01-01 00:00:00"));
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("future_price_drop_limited", json!(true)));
    seed::insert_row(&o.primary, &s, b, s.btc, &json!({"external_id":"WAIT", "external_status":0, "side":0,
        "amount":"0.001", "quote_amount":"60", "amount_exec":"0", "quote_amount_exec":"0", "created_at":"2026-09-01 10:00:01"}));
    job(&o.queue, "Bot::ActionJob", &format!("gid://deltabadger/Bots::DcaMultiAsset/{b}"));
    let dump = |c: &rusqlite::Connection| c.query_row("SELECT group_concat(id || status || transient_data || coalesce(stop_message_key, ''), ';') FROM bots", [], |r| r.get::<_, String>(0)).unwrap();
    let before = (dump(&o.primary), count(&o.primary, "SELECT count(*) FROM bot_activity_logs"));
    match handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()) {
        Err(EngineError::HardStop(message)) => {
            assert!(message.starts_with("Deltabadger didn't start") && message.contains(&format!("Bot {b}: It has open orders.")) && !message.contains(&format!("Bot {healthy}")), "{message}");
            assert!(message.ends_with("Nothing was bought or sold. Contact support."), "{message}");
        }
        other => panic!("{:?}", other.map(|t| t.eligible)),
    }
    assert_eq!((dump(&o.primary), count(&o.primary, "SELECT count(*) FROM bot_activity_logs")), before, "the stop rolled back");
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
        &px, &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
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
fn rails_boot(dir: &std::path::Path) -> (bool, String) { rails_boot_running(dir, "") }

/// `rails_boot`, running `script` (Ruby) in the booted app.
fn rails_boot_running(dir: &std::path::Path, script: &str) -> (bool, String) {
    let scratch = tempfile::tempdir().unwrap();
    let file = scratch.path().join("script.rb"); // a file, so a raise shows its backtrace
    std::fs::write(&file, format!("{script}\nputs :booted\n")).unwrap();
    let out = rails_on(dir, scratch.path(), &["runner", file.to_str().unwrap()]).output().expect("bin/rails runs");
    (out.status.success() && String::from_utf8_lossy(&out.stdout).contains("booted"), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// `bin/rails <args>` in the development env against this install's files (cache and cable in `scratch`), with the
/// test cipher's keys. A schema dump goes to `scratch`, never to db/.
fn rails_on(dir: &std::path::Path, scratch: &std::path::Path, args: &[&str]) -> Command {
    let keys = deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut command = Command::new(root.join("bin/rails"));
    command.current_dir(root).args(args).env("SCHEMA", scratch.join("schema.rb"))
        .env("RAILS_ENV", "development").env_remove("DATABASE_URL").env("APP_ROOT_URL", "http://localhost:3000")
        .env("PRIMARY_DATABASE_URL", format!("sqlite3:{}", dir.join("production.sqlite3").display()))
        .env("QUEUE_DATABASE_URL", format!("sqlite3:{}", dir.join("production_queue.sqlite3").display()))
        .env("CACHE_DATABASE_URL", format!("sqlite3:{}/cache.sqlite3", scratch.display()))
        .env("CABLE_DATABASE_URL", format!("sqlite3:{}/cable.sqlite3", scratch.display()))
        .env("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", keys.primary_key).env("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", keys.key_derivation_salt);
    command
}

/// Rails runs on an install Rust migrated: it ignores rust_schema_migrations and Rust's tables in db:migrate,
/// db:migrate:status and a boot after a handback, and leaves them as they were.
#[tokio::test(flavor = "current_thread")]
async fn rails_migrates_and_adopts_a_handback_on_an_install_rust_migrated() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let list = common::probe_migrations();
    // This build's migrations (rails_install applied them) and the probe after them.
    let migrations: Vec<store::Migration> = store::REGISTRY.migrations.iter().copied().chain(common::as_migrations(&list)).collect();
    let tables: Vec<(&str, store::TableContract)> = store::REGISTRY.tables.iter().chain(common::PROBE_TABLES)
        .map(|(v, t)| (*v, store::TableContract { name: t.name, columns: t.columns, unique_indexes: t.unique_indexes })).collect();
    let registry = store::Registry { migrations: &migrations, tables: &tables };
    // Opened before the probe migrates: this build's own registry does not know it, so `open` would refuse after.
    let o = store::open(&p).unwrap();
    assert_eq!(store::migrate_with(&p, &l, &registry).unwrap(), vec![common::PROBE.to_string()]);
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap();
    drop((l, o));
    let scratch = tempfile::tempdir().unwrap();
    for task in ["db:migrate", "db:migrate:status:primary"] {
        let out = rails_on(dir.path(), scratch.path(), &[task]).output().expect("bin/rails runs");
        assert!(out.status.success(), "{task}: {}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains(" down ") && !stdout.contains("NO FILE"), "{task}: {stdout}");
    }
    assert!(rails_boot(dir.path()).0, "Rails starts after a completed handback");
    let c = rusqlite::Connection::open(&p.primary).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM rust_schema_migrations"), store::REGISTRY.migrations.len() as i64 + 1, "Rust's record untouched");
    assert_eq!(count(&c, "SELECT count(*) FROM sqlite_master WHERE name = 'rust_migration_probe'"), 1);
    assert_eq!(store::check_with(&p, &registry).unwrap().pending_rust, Vec::<String>::new());
}

#[tokio::test(flavor = "current_thread")]
async fn wrong_secret_handback_writes_nothing() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let row = || -> String { o.primary.query_row("SELECT value || updated_at FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get(0)).unwrap() };
    let bots = || -> String { o.primary.query_row("SELECT group_concat(status || updated_at) FROM bots", [], |r| r.get(0)).unwrap() };
    let (row_before, bots_before) = (row(), bots());
    let wrong = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-instance").unwrap());
    let r = handover::hand_back(&l, &o, &FakeFactory(FakeVenue::new()), &wrong, &deltabadger::engine::FixedClock(now())).await;
    assert!(matches!(r, Err(EngineError::Lease(lease::LeaseError::Unreadable))), "{r:?}");
    assert_eq!((row(), bots()), (row_before, bots_before), "no intents, a wrong secret: nothing written");
    assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust");
}

#[derive(Clone)]
struct ScriptedAlpaca(deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>);
impl deltabadger::venue::VenueFactory for ScriptedAlpaca {
    type V = deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>;
    fn for_bot(&self, _exchange_type: &str, _credentials: Option<deltabadger::crypto::Credentials>) -> Self::V { self.0.clone() }
}

async fn handback_after_lookups(lookups: serde_json::Value) -> (Result<usize, EngineError>, usize) {
    use deltabadger::engine::{amount, venue_rules::ALPACA};
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60), &deltabadger::ruby::BigDec::from_i64(64_000), &ALPACA).unwrap() else { panic!() };
    placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap();
    let t = deltabadger::venue::http::ScriptedTransport::from_script(&serde_json::json!({ "GET /v2/orders:by_client_order_id": lookups }));
    let factory = ScriptedAlpaca(deltabadger::venue::alpaca::AlpacaVenue::new(t.clone(), deltabadger::venue::alpaca::Urls::for_passphrase(Some("paper"))));
    let later = deltabadger::engine::FixedClock(now() + chrono::Duration::seconds(1300)); // the process started at `now()`: a full 1200 s window ago
    let r = handover::hand_back_retrying(&l, &o, &factory, &seed::cipher(), &later, now(), std::time::Duration::from_millis(10)).await;
    (r, t.requests().len())
}

#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_handback_waits_once_then_settles_or_exits_3() {
    let not_found = serde_json::json!({ "status": 404, "body": { "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" } });
    let failing = serde_json::json!({ "status": 500, "body": { "message": "internal server error" } });
    let (r, lookups) = handback_after_lookups(serde_json::json!([failing, not_found])).await;
    assert!(matches!(r, Ok(1)), "{r:?}");
    assert_eq!((lookups, handover::handback_exit_code(&r)), (2, 0), "unresolved, waited, retried, resolved");
    let (r, lookups) = handback_after_lookups(serde_json::json!([failing])).await;
    assert!(matches!(r, Err(EngineError::Unresolved(_))), "{r:?}");
    assert_eq!((lookups, handover::handback_exit_code(&r)), (2, 3), "still unaccounted for after one retry: exit 3, nothing written");
}

/// Wall time that follows tokio's (paused) clock, so a test can watch what the CLI's handback does across its wait.
struct TokioClock { base: DateTime<Utc>, start: tokio::time::Instant }
impl deltabadger::engine::Clock for TokioClock {
    fn now(&self) -> DateTime<Utc> { self.base + chrono::Duration::from_std(self.start.elapsed()).unwrap() }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_cli_handback_trusts_no_absence_before_a_full_margin_after_its_own_start_and_waits_exactly_that() {
    use deltabadger::engine::{amount, venue_rules::ALPACA};
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60), &deltabadger::ruby::BigDec::from_i64(64_000), &ALPACA).unwrap() else { panic!() };
    // The intent is far older than the margin: only this process's own (fresh) start can hold the absence back.
    placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now() - chrono::Duration::seconds(2000))).unwrap();
    let not_found = json!({ "status": 404, "body": { "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" } });
    let t = deltabadger::venue::http::ScriptedTransport::from_script(&json!({ "GET /v2/orders:by_client_order_id": [not_found] }));
    let factory = ScriptedAlpaca(deltabadger::venue::alpaca::AlpacaVenue::new(t.clone(), deltabadger::venue::alpaca::Urls::for_passphrase(Some("paper"))));
    let clock = TokioClock { base: now(), start: tokio::time::Instant::now() };
    let r = handover::hand_back_cli(&l, &o, &factory, &seed::cipher(), &clock).await;
    assert!(matches!(r, Ok(1)), "{r:?}");
    assert_eq!(t.requests().len(), 2, "the first not-found came from a process started < 1200 s ago and settled nothing");
    assert_eq!(clock.start.elapsed(), std::time::Duration::from_secs(ALPACA.absence_margin_secs as u64 + 1), "waited exactly the margin + 1 s");
    let intent: Option<String> = o.primary.query_row("SELECT json_extract(transient_data, '$.rust_placement') FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(intent.is_none(), "settled as not placed by the second, trustworthy lookup");
}

/// While a bot's order is unresolved, a write that changes what its intent matches on (here the
/// asset, so the ticker) is refused by the guard, although `check_install` alone would accept it; so the handback can
/// still find the order and settle it.
#[tokio::test(flavor = "current_thread")]
async fn a_settings_write_cannot_strand_an_unresolved_order_and_handback_settles_it() {
    use deltabadger::engine::eligibility::{self, Refusal};
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
        &px, &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
    let intent = placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap(); // sent; its reply lost
    // The web changes the bot's composition (BTC's weight) while its BTC order is unresolved.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json(?1)) WHERE id = ?2",
               rusqlite::params![json!({ s.btc.to_string(): 0.9995 }).to_string(), b]).unwrap();
    assert!(eligibility::check_install(&tx).unwrap().problems.is_empty(), "eligible, as far as check_install sees");
    let refused = eligibility::guard(&tx, &seed::cipher(), Some(b)).unwrap_err();
    let line = format!("bot {b}: an order is still being reconciled; its composition, asset, exchange and quote cannot change until it settles");
    assert!(matches!(&refused, Refusal::Reconciling(lines) if lines == &vec![line.clone()]), "{refused:?}");
    assert_eq!(refused.reason(), line, "what the 422 carries");
    drop(tx); // rolled back
    // The venue has the order under the intent's client order id: the handback finds it, records it and settles.
    let closed = json!({ "error": [], "result": { "closed": { "OTX-H": { "cl_ord_id": intent.cl_ord_id, "status": "closed", "price": "50000",
        "vol": "60", "vol_exec": "0.0012", "cost": "60", "oflags": "viqc", "descr": { "type": "buy", "ordertype": "market", "price": "0" } } } } });
    let factory = FakeFactory(FakeVenue::from_script(&json!({ "http": { "/0/private/ClosedOrders": [closed] } })));
    assert_eq!(handover::hand_back(&l, &o, &factory, &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap(), 1);
    assert!(model::load_bot(&o.primary, b).unwrap().rust_placement().is_none(), "the order is settled");
    assert_eq!(count(&o.primary, "SELECT count(*) FROM transactions WHERE external_id = 'OTX-H'"), 1, "and recorded");
}

/// A swept fill spent the cap, then the engine died before its tick's end ran the stop; the operator hands back instead of
/// restarting. The handback runs the pending stop, as the engine's next start would have.
#[tokio::test(flavor = "current_thread")]
async fn a_crash_then_handback_lands_the_pending_amount_limit_stop() { crash_then_handback(false).await }

/// The same, but the cap was raised before the handback: the stop's cause is gone, so the handback discards it and hands the
/// bot back scheduled.
#[tokio::test(flavor = "current_thread")]
async fn a_crash_then_cap_raise_then_handback_discards_the_pending_amount_limit_stop() { crash_then_handback(true).await }

async fn crash_then_handback(raised: bool) {
    use common::scripted::{ok, script, venue};
    let at: DateTime<Utc> = "2026-09-08T10:00:00.5Z".parse().unwrap();
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, at).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true))
        .with("quote_amount_limit", json!(60.0)).transient("quote_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z")));
    seed::insert_tx(&o.primary, &s, b, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("OTX-S".into()), order_type: 0,
        amount: None, quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() });
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", at).unwrap();
    let t = script(json!({ "GET /v2/orders/OTX-S": [ok(json!({ "id": "OTX-S", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy",
        "notional": "60", "qty": null, "filled_qty": "0.0009375", "filled_avg_price": "64000", "limit_price": null }))] }));
    // The tick's sweep commits the fill and counts the stop; the engine dies before the tick's end.
    deltabadger::engine::polling::sweep(&o.primary, &venue(&t), &model::load_bot(&o.primary, b).unwrap(), at).await.unwrap();
    assert_eq!(count(&o.primary, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots"), 1);
    if raised {
        o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limit', 200.0) WHERE id = ?1", [b]).unwrap();
    }
    assert_eq!(handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(at)).await.unwrap(),
               raised as usize, "a stopped bot is not handed back as scheduled");
    let (status, key, pending): (i64, Option<String>, Option<String>) = o.primary.query_row(
        "SELECT status, stop_message_key, json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots WHERE id = ?1", [b],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    if raised {
        assert_eq!((status, key, pending), (deltabadger::enums::BotStatus::Scheduled as i64, None, None));
    } else {
        assert_eq!((status, key.as_deref(), pending), (deltabadger::enums::BotStatus::Stopped as i64, Some(deltabadger::engine::tick::AMOUNT_SPENT), None));
    }
    assert_eq!(count(&o.primary, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), !raised as i64);
}

/// A settled order deferred the next buy to the next weekly checkpoint, then the engine died and the operator handed back.
/// The handback removes the engine's key; Rails' own adoption re-arms the bot at next_interval_checkpoint_at, which is that
/// checkpoint, so Rails does not buy the remainder early either.
#[tokio::test(flavor = "current_thread")]
async fn a_deferral_crash_then_handback_then_rails_tick_buys_nothing_before_the_next_checkpoint() {
    let at: DateTime<Utc> = "2026-09-01T10:30:00Z".parse().unwrap();
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, at).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    o.primary.execute("UPDATE bots SET label = 'Bitcoin' WHERE id = ?1", [b]).unwrap(); // Rails loads it: Labelable needs one
    placement::defer_to_next_checkpoint(&o.primary, &model::load_bot(&o.primary, b).unwrap(), at).unwrap();
    assert_eq!(model::load_bot(&o.primary, b).unwrap().rust_defer_until_us().unwrap(), Some("2026-09-08T10:00:00Z".parse::<DateTime<Utc>>().unwrap().timestamp_micros()));
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", at).unwrap();
    assert_eq!(handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(at)).await.unwrap(), 1);
    let bot = model::load_bot(&o.primary, b).unwrap();
    assert_eq!(bot.status, deltabadger::enums::BotStatus::Scheduled);
    assert!(bot.transient.get("rust_defer_until").is_none(), "the engine's key does not reach Rails");
    drop((l, o));
    // Rails boots, adopts the handback, and its repair sweep (run here at the handback's time) re-arms the bot.
    let (booted, err) = rails_boot_running(dir.path(),
        "require 'active_support/testing/time_helpers'\ninclude ActiveSupport::Testing::TimeHelpers\ntravel_to(Time.utc(2026, 9, 1, 10, 30)) { Bot::RepairOrphanedBotsJob.perform_now }");
    assert!(booted, "{err}");
    let q = rusqlite::Connection::open(dir.path().join("production_queue.sqlite3")).unwrap();
    let at: String = q.query_row("SELECT e.scheduled_at FROM solid_queue_scheduled_executions e JOIN solid_queue_jobs j ON j.id = e.job_id \
                                  WHERE j.class_name = 'Bot::ActionJob'", [], |r| r.get(0)).unwrap();
    assert!(at.starts_with("2026-09-08 10:00:00"), "Rails' first run is the deferred checkpoint, not earlier: {at}");
}

/// The web continued a bot and the engine never passed before the handback: the request is the engine's own key, so it goes.
#[tokio::test(flavor = "current_thread")]
async fn a_handback_removes_an_unhandled_continue_request() {
    let at: DateTime<Utc> = "2026-09-08T12:00:00Z".parse().unwrap();
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, at).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .transient("rust_continue_start", json!({ "requested_at": "2026-09-08T11:59:59Z" })));
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", at).unwrap();
    assert_eq!(handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(at)).await.unwrap(), 1);
    let bot = model::load_bot(&o.primary, b).unwrap();
    assert!(bot.transient.get("rust_continue_start").is_none(), "the engine's key does not reach Rails");
    assert_eq!(bot.status, deltabadger::enums::BotStatus::Scheduled);
}


#[test]
fn a_stale_catalog_is_noted_at_a_rails_takeover_and_at_an_engine_owned_restart() -> Result<(), Box<dyn std::error::Error>> {
    use deltabadger::engine::{eligibility, staleness};
    let (dir, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&paths, now()).map_err(|e| format!("{e:?}"))?;
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2020-01-01 00:00:00'", [])?;
    // D-UPGRADE-FRESHNESS-AT-TICK: the takeover from Rails admits it; the tick refuses the bot (below).
    let first = handover::take_over(&lock, &o, &seed::cipher(), "test", now()).map_err(|e| format!("{e:?}"))?;
    assert_eq!(first.eligible, vec![bot]);
    let takeover = handover::take_over(&lock, &o, &seed::cipher(), "test", now()).map_err(|e| format!("{e:?}"))?;
    assert!(matches!(takeover.claim, Claim::AfterCrash));
    assert_eq!(takeover.eligible, vec![bot]);
    assert!(staleness::stale(&o.primary, &model::load_bot(&o.primary, bot).map_err(|e| format!("{e:?}"))?, now()).map_err(|e| format!("{e:?}"))?.is_some(), "restart cannot make stale data fresh");
    assert!(eligibility::check_install_at(&o.primary, now()).map_err(|e| format!("{e:?}"))?.notes.iter().any(|n| n.contains("reference data stale")), "check continues reporting staleness");
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.future_price_drop_limited', 1)", [])?;
    // PLAN-B2 R3: every start, not only the first: a restart sets a newly refused bot aside instead of refusing the install.
    let restart = handover::take_over(&lock, &o, &seed::cipher(), "test", now()).map_err(|e| format!("{e:?}"))?;
    assert_eq!((restart.set_aside.iter().map(|a| a.id).collect::<Vec<_>>(), restart.eligible), (vec![bot], vec![]));
    assert_eq!(model::load_bot(&o.primary, bot).map_err(|e| format!("{e:?}"))?.status, deltabadger::enums::BotStatus::Stopped);
    Ok(())
}

#[test]
fn an_index_bots_rails_jobs_are_removed_at_takeover() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    seed::insert_index(&o.primary, "nasdaq-100", &["AAPL.US"], &serde_json::json!({ "AAPL.US": 3e12 }));
    let id = seed::index_bot(&o.primary, &s, "nasdaq-100", 1, 0.0, false);
    seed::fresh_stock_jobs(&o.primary, now());
    job(&o.queue, "Bot::ActionJob", &format!("gid://deltabadger/Bots::DcaIndex/{id}"));
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(t.deleted_jobs, 1, "the GlobalID names the bot's own class");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0);
}

/// Slice 7a: a bot resting on its price rule is handed over as it rests. Takeover drops Rails' check chain and leaves it
/// `waiting` (the engine checks it); handback leaves it `waiting`, and Rails' adoption restarts its check chain, where
/// `scheduled` would trade it at the next checkpoint whatever the price.
#[tokio::test(flavor = "current_thread")]
async fn a_price_paused_bot_is_handed_over_paused_and_rails_restarts_its_check() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec { status: 6, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
        .with("price_limited", json!(true)).with("price_limit", json!(50000.0)).transient("last_action_job_at", json!("2026-09-30T11:00:00.000Z")));
    model::log_activity(&o.primary, b, "limit_paused", model::Level::Info, json!({ "limit_type": "price" }), "2026-09-30T11:00:00.001Z".parse().unwrap()).unwrap();
    job(&o.queue, "Bot::PriceLimitCheckJob", &format!("gid://deltabadger/Bots::DcaMultiAsset/{b}"));
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((t.deleted_jobs, t.normalised), (1, 0), "Rails' check chain goes; the pause is not a cut-short tick");
    assert_eq!(model::load_bot(&o.primary, b).unwrap().status, deltabadger::enums::BotStatus::Waiting);
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_condition_check_dead', json('true')) WHERE id = ?1", [b]).unwrap();
    assert_eq!(handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap(), 0);
    let bot = model::load_bot(&o.primary, b).unwrap();
    assert_eq!(bot.status, deltabadger::enums::BotStatus::Waiting, "still paused");
    assert!(bot.transient.get("rust_condition_check_dead").is_none(), "the engine's own marker goes");
    o.primary.execute("UPDATE bots SET updated_at = '2026-09-30 11:00:00' WHERE id = ?1", [b]).unwrap(); // past Repair's grace
    drop((l, o));
    let (booted, err) = rails_boot_running(dir.path(), &format!(
        "Bot::RepairOrphanedBotsJob.perform_now\nraise 'no check chain' unless Bot.find({b}).pending_limit_check_job?\nraise 'not waiting' unless Bot.find({b}).waiting?"));
    assert!(booted, "{err}");
}

/// Plan CI T3b: a Bot::ResyncIndexCompositionJob Rails has not finished (ready, scheduled or claimed; a failed one is over)
/// becomes its bot's `rust_index_resync` marker at takeover, for a created or stopped index bot too, never for an archived
/// one; a takeover repeated after a crash between the marker and the delete marks again. At handback the markers stay: a
/// Rust reclaim finds them, and Rails' adoption enqueues one job per marked bot however many handbacks came first.
#[tokio::test(flavor = "current_thread")]
async fn rails_pending_resyncs_become_markers_at_takeover_and_rails_adopts_them_once_after_handbacks() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let index = |status: i64| { let id = seed::index_bot(&o.primary, &s, "layer-1", 3, 0.0, false);
                                o.primary.execute("UPDATE bots SET status = ?1 WHERE id = ?2", [status, id]).unwrap(); id };
    let (created, stopped, archived) = (index(0), index(2), index(7));
    let gid = |id: i64| format!("gid://deltabadger/Bots::DcaIndex/{id}");
    let resync = |id: i64| { o.queue.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, priority, created_at, updated_at) VALUES ('default', 'Bot::ResyncIndexCompositionJob', ?1, 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00')",
                                             [json!({ "job_class": "Bot::ResyncIndexCompositionJob", "arguments": [{ "_aj_globalid": gid(id) }] }).to_string()]).unwrap();
                             o.queue.last_insert_rowid() };
    resync(created); // ready
    let failed = resync(created);
    o.queue.execute("INSERT INTO solid_queue_failed_executions (job_id, error, created_at) VALUES (?1, 'boom', '2026-09-30 11:00:00')", [failed]).unwrap();
    job(&o.queue, "Bot::ResyncIndexCompositionJob", &gid(stopped)); // scheduled (a retry)
    let claimed = resync(stopped);
    o.queue.execute("INSERT INTO solid_queue_claimed_executions (job_id, process_id, created_at) VALUES (?1, 1, '2026-09-30 11:00:00')", [claimed]).unwrap();
    resync(archived);
    let n = |id: i64| o.primary.query_row("SELECT json_extract(transient_data, '$.rust_index_resync.n') FROM bots WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0)).unwrap();
    let rows = |o: &store::Opened| count(&o.queue, "SELECT count(*) FROM solid_queue_jobs WHERE class_name = 'Bot::ResyncIndexCompositionJob'");
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(t.deleted_jobs, 3);
    assert_eq!((n(created), n(stopped), n(archived)), (Some(1), Some(2), None), "one mark per pending row");
    assert_eq!(rows(&o), 2, "the failed row and the archived bot's stay");
    resync(stopped); // the delete was lost to a crash: the repeated takeover marks again and deletes it
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((n(stopped), rows(&o)), (Some(3), 2));

    handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap();
    assert_eq!((n(created), n(stopped)), (Some(1), Some(3)), "kept for Rails");
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap(); // a Rust reclaim before Rails booted
    assert_eq!((n(created), n(stopped)), (Some(1), Some(3)), "found intact");
    handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap();
    drop((l, o));
    let (booted, err) = rails_boot(dir.path());
    assert!(booted, "{err}");
    let q = rusqlite::Connection::open(dir.path().join("production_queue.sqlite3")).unwrap();
    let mut s = q.prepare("SELECT arguments FROM solid_queue_jobs WHERE class_name = 'Bot::ResyncIndexCompositionJob' AND finished_at IS NULL \
                           AND id NOT IN (SELECT job_id FROM solid_queue_failed_executions) ORDER BY id").unwrap();
    let enqueued: Vec<String> = s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap)
        .map(|a| serde_json::from_str::<serde_json::Value>(&a).unwrap()["arguments"][0]["_aj_globalid"].as_str().unwrap().to_string()).collect();
    assert_eq!(enqueued, [gid(archived), gid(created), gid(stopped)], "the archived bot's own Rails row, then one per marked bot");
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM bots WHERE json_type(transient_data, '$.rust_index_resync') IS NOT NULL"), 0, "adopted: the key goes");
}

// ---- Plan 8d: a signal bot's webhook calls across the handover (T2 takeover import, T3 handback) ----------------------

fn signal_jobs_fixture() -> serde_json::Value {
    use sha2::{Digest, Sha256};
    let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/signal_jobs.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in fixture["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}: script/rust/repin");
    }
    fixture
}

/// `row` as an INSERT into `table`, its keys the columns.
fn insert_row(c: &rusqlite::Connection, table: &str, row: &serde_json::Map<String, serde_json::Value>) {
    let cols: Vec<&String> = row.keys().collect();
    let sql = format!("INSERT INTO {table} ({}) VALUES ({})", cols.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
                      (1..=cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", "));
    let values: Vec<rusqlite::types::Value> = row.values().map(|v| match v {
        serde_json::Value::Null => rusqlite::types::Value::Null,
        serde_json::Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
        serde_json::Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }).collect();
    c.execute(&sql, rusqlite::params_from_iter(values)).unwrap();
}

/// Rails' recorded Bot::SignalJob `name` in the queue, with its execution row, as script/rust/record_signal_jobs.rb left it.
fn queue_signal_job(q: &rusqlite::Connection, fixture: &serde_json::Value, name: &str) {
    let job = &fixture["jobs"][name];
    insert_row(q, "solid_queue_jobs", job["row"].as_object().unwrap());
    let id = q.last_insert_rowid();
    if let Some(execution) = job["execution"].as_object() {
        let mut row = execution.clone();
        row.insert("job_id".into(), json!(id));
        insert_row(q, &format!("solid_queue_{}_executions", job["state"].as_str().unwrap()), &row);
    }
}

/// An install holding the recording's signal bot 1 (Kraken BTC/EUR, working) with its rules 1 to 5; rule 6 and bot 2 are
/// gone, as the recording left them. The jobs `names` are queued.
fn signal_install(names: &[&str]) -> (tempfile::TempDir, deltabadger::lease::EngineLock, store::Opened, serde_json::Value, seed::Seeded) {
    let fixture = signal_jobs_fixture();
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute("INSERT INTO bots (id, type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
                       VALUES (1, 'Bots::Signal', 1, ?1, ?2, ?3, '{}', '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
                      rusqlite::params![s.exchange_id, s.user_id, json!({ "base_asset_id": s.btc, "quote_asset_id": s.quote }).to_string()]).unwrap();
    for rule in 1..=5 {
        o.primary.execute("INSERT INTO bot_signals (id, bot_id, direction, amount_type, amount, enabled, token, created_at, updated_at) \
                           VALUES (?1, 1, 0, 0, '100', 1, ?2, '2026-08-25 09:00:00', '2026-08-25 09:00:00')", rusqlite::params![rule, format!("tok-{rule}")]).unwrap();
    }
    for name in names { queue_signal_job(&o.queue, &fixture, name); }
    (dir, l, o, fixture, s)
}

fn signal_calls_of(o: &store::Opened) -> Vec<model::SignalCall> { model::load_bot(&o.primary, 1).unwrap().signal_calls().unwrap() }
fn import_set(o: &store::Opened) -> Option<String> { deltabadger::app_config::get_plain(&o.primary, handover::SIGNAL_IMPORT_KEY).unwrap() }
fn job_ids(q: &rusqlite::Connection) -> Vec<String> {
    let mut s = q.prepare("SELECT active_job_id FROM solid_queue_jobs ORDER BY id").unwrap();
    s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
fn job_id(fixture: &serde_json::Value, name: &str) -> String { fixture["jobs"][name]["job_id"].as_str().unwrap().to_string() }
fn imported(fixture: &serde_json::Value, name: &str) -> model::SignalCall {
    let job = &fixture["jobs"][name];
    model::SignalCall { source: format!("rails:{}", job["job_id"].as_str().unwrap()), signal_id: job["signal_id"].as_i64().unwrap(),
                        triggered_at: job["triggered_at"].as_str().unwrap().parse().unwrap() }
}

/// §R1: every waiting job becomes its bot's call, in queue order, with the triggered_at Rails deserializes; a job whose rule
/// or bot is gone becomes none (discard_on); a finished one stays Rails' record. A failed one may have placed its order: it
/// becomes the bot's marker (r3.1, D-UPGRADE-CLAIMED-SIGNAL). The calls commit with the claim, then the rows go, then the
/// import set empties.
#[test]
fn rails_signal_jobs_become_the_engines_calls_and_leave_the_queue() {
    let all = ["ready", "blocked", "scheduled", "finished", "failed", "rule_removed", "bot_removed"];
    let (_dir, l, o, fixture, _) = signal_install(&all);
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((t.imported_calls, t.deleted_jobs, t.eligible.clone()), (3, 6, vec![1]));
    assert_eq!(signal_calls_of(&o), ["ready", "blocked", "scheduled"].map(|n| imported(&fixture, n)).to_vec());
    assert_eq!(signal_calls_of(&o)[0].triggered_at, "2026-09-30T11:58:01.123456Z".parse::<DateTime<Utc>>().unwrap(), "the TimeWithZone, to the microsecond");
    assert_eq!(job_ids(&o.queue), [job_id(&fixture, "finished")], "Rails' record of the finished job stays");
    assert_eq!(marker_of(&o)["id"], format!("rails-job:{}", job_id(&fixture, "failed")), "the failed job is the bot's marker");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_ready_executions") + count(&o.queue, "SELECT count(*) FROM solid_queue_blocked_executions")
               + count(&o.queue, "SELECT count(*) FROM solid_queue_scheduled_executions"), 0, "executions cascade");
    assert_eq!(import_set(&o), None, "the set empties once the rows are gone");
    // Repeated starts import nothing again.
    for _ in 0..3 {
        let again = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
        assert_eq!((again.imported_calls, again.deleted_jobs), (0, 0));
    }
    assert_eq!(signal_calls_of(&o).len(), 3);
}

/// §R1: a crash after the import committed and before the rows went (or before the set emptied) is finished by the next
/// start, importing nothing twice; meanwhile the engine runs no call.
#[tokio::test(flavor = "current_thread")]
async fn a_crash_between_the_import_and_the_delete_imports_nothing_twice_and_runs_nothing_meanwhile() {
    let (_dir, l, o, fixture, _) = signal_install(&["ready", "blocked", "rule_removed"]);
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    // The state step 1 leaves: the calls and the set committed, Rails' rows still queued.
    for name in ["ready", "blocked", "rule_removed"] { queue_signal_job(&o.queue, &fixture, name); }
    let ids: Vec<String> = ["ready", "blocked", "rule_removed"].iter().map(|n| job_id(&fixture, n)).collect();
    deltabadger::app_config::set_plain(&o.primary, handover::SIGNAL_IMPORT_KEY, &json!(ids).to_string(), now()).unwrap();
    // No call runs while the set holds an id.
    let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3").balance_body("ZEUR", "10000", "0");
    let at = deltabadger::engine::FixedClock("2026-09-30T11:59:00Z".parse().unwrap());
    {
        let primary = rusqlite::Connection::open(_dir.path().join("production.sqlite3")).unwrap();
        let mut e = deltabadger::engine::run::Engine::new(primary, FakeFactory(venue.clone()), seed::cipher(), l.clone());
        deltabadger::engine::run::step(&mut e, &at).await.unwrap();
    }
    assert!(venue.sent().is_empty(), "nothing placed while the import is unfinished");
    assert_eq!(signal_calls_of(&o).len(), 2);
    // The next start finishes steps 2 and 3, and imports nothing again.
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((t.imported_calls, t.deleted_jobs), (0, 3));
    assert_eq!((signal_calls_of(&o), job_ids(&o.queue), import_set(&o)), (vec![imported(&fixture, "ready"), imported(&fixture, "blocked")], vec![], None));
}

/// §R1: a call that ran (consumed) before its queue row was deleted is not imported again: its id is still in the set.
#[test]
fn a_call_that_ran_before_its_row_went_is_not_imported_again() {
    let (_dir, l, o, fixture, _) = signal_install(&["ready"]);
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    model::remove_signal_call(&o.primary, 1, &imported(&fixture, "ready").source).unwrap();
    queue_signal_job(&o.queue, &fixture, "ready");
    deltabadger::app_config::set_plain(&o.primary, handover::SIGNAL_IMPORT_KEY, &json!([job_id(&fixture, "ready")]).to_string(), now()).unwrap();
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((t.imported_calls, t.deleted_jobs), (0, 1));
    assert!(signal_calls_of(&o).is_empty() && job_ids(&o.queue).is_empty() && import_set(&o).is_none());
}

/// r1.2, r4.2 (D-UPGRADE-CLAIMED-SIGNAL): a claimed and a failed job of one bot may each have placed an order. Neither is
/// placed again: one marker (the first job's, with its rule's side) blocks the bot until the owner confirms, and each job
/// leaves a line (C3). Nothing is a call.
#[test]
fn claimed_and_failed_signal_jobs_become_one_marker_and_a_line_each() {
    let (_dir, l, o, fixture, _) = signal_install(&["ready", "blocked"]);
    fail_signal_job(&o.queue, &fixture, "ready", true);
    fail_signal_job(&o.queue, &fixture, "blocked", false);
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((t.imported_calls, t.deleted_jobs), (0, 2));
    assert!(signal_calls_of(&o).is_empty(), "never a call");
    let marker = marker_of(&o);
    assert_eq!((marker["id"].as_str().unwrap(), marker["side"].as_str()), (format!("rails-job:{}", job_id(&fixture, "ready")).as_str(), Some("buy")));
    assert_eq!(bot_events(&o.primary, 1), [handover::UPGRADE_SIGNAL_UNRESOLVED, handover::UPGRADE_SIGNAL_UNRESOLVED]);
    assert!(deltabadger::engine::signal::blocked(&model::load_bot(&o.primary, 1).unwrap()));
    assert!(job_ids(&o.queue).is_empty());
    // A start after a crash between the commit and the delete: the rows are back, the set names them. Nothing twice.
    for name in ["ready", "blocked"] { queue_signal_job(&o.queue, &fixture, name); }
    let rows: Vec<i64> = { let mut s = o.queue.prepare("SELECT id FROM solid_queue_jobs ORDER BY id").unwrap(); s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
    deltabadger::app_config::set_plain(&o.primary, handover::QUEUE_DONE_KEY, &json!(rows).to_string(), now()).unwrap();
    let again = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((again.imported_calls, again.deleted_jobs), (0, 2));
    assert_eq!(bot_events(&o.primary, 1).len(), 2, "no line twice");
    assert!(signal_calls_of(&o).is_empty() && job_ids(&o.queue).is_empty());
}

/// §R10's takeover case: Rails' marker of an ambiguous send and one queued job. The imported call places nothing and
/// expires at 300 s + 1 µs with its line.
#[tokio::test(flavor = "current_thread")]
async fn an_imported_call_behind_rails_marker_places_nothing_and_expires() {
    let (dir, l, o, fixture, _) = signal_install(&["ready"]);
    o.primary.execute("UPDATE bots SET transient_data = ?1", [json!({ "signal_placement_unresolved": { "id": "marker-1", "at": "2026-09-30T11:57:00Z", "side": "buy", "error": "timeout" } }).to_string()]).unwrap();
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3").balance_body("ZEUR", "10000", "0");
    let triggered = imported(&fixture, "ready").triggered_at;
    let mut e = deltabadger::engine::run::Engine::new(rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap(), FakeFactory(venue.clone()), seed::cipher(), l.clone());
    let wake = deltabadger::engine::run::step(&mut e, &deltabadger::engine::FixedClock(triggered + chrono::Duration::seconds(300))).await.unwrap();
    assert!(venue.sent().is_empty() && signal_calls_of(&o).len() == 1, "held by the marker");
    assert_eq!(wake, (triggered + chrono::Duration::seconds(300) + chrono::Duration::microseconds(1)).timestamp_micros(), "the loop wakes at the expiry");
    deltabadger::engine::run::step(&mut e, &deltabadger::engine::FixedClock(triggered + chrono::Duration::seconds(300) + chrono::Duration::microseconds(1))).await.unwrap();
    assert!(venue.sent().is_empty() && signal_calls_of(&o).is_empty());
    let event: String = o.primary.query_row("SELECT event FROM bot_activity_logs ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(event, "signal_expired");
}

/// A signal install the engine owns, with one call triggered at `triggered`.
fn signal_install_with_call(triggered: DateTime<Utc>) -> (tempfile::TempDir, deltabadger::lease::EngineLock, store::Opened, seed::Seeded) {
    let (dir, l, o, _, s) = signal_install(&[]);
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    model::append_signal_call(&o.primary, 1, &model::SignalCall { source: "hook:1".into(), signal_id: 1, triggered_at: triggered }).unwrap();
    (dir, l, o, s)
}
fn events(o: &store::Opened) -> Vec<String> {
    let mut s = o.primary.prepare("SELECT event FROM bot_activity_logs ORDER BY id").unwrap();
    s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// §R8': a call is still valid at exactly 300 s and expires 1 µs later. A valid one refuses the handback with when it
/// expires, writing nothing; an expired one is logged `signal_expired` and removed, and the handback completes.
#[tokio::test(flavor = "current_thread")]
async fn the_handback_refuses_while_a_call_is_valid_and_expires_it_after() {
    for (age_us, valid) in [(0, true), (299_999_000, true), (300_000_000, true), (300_000_001, false), (301_000_000, false)] {
        let triggered = now() - chrono::Duration::microseconds(age_us);
        let (_dir, l, o, _) = signal_install_with_call(triggered);
        let r = handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await;
        if valid {
            let Err(EngineError::SignalCallsWaiting(until)) = r else { panic!("{age_us}: {r:?}") };
            assert_eq!(until, triggered + chrono::Duration::seconds(300) + chrono::Duration::microseconds(1), "{age_us}");
            assert_eq!((signal_calls_of(&o).len(), events(&o).len()), (1, 0), "{age_us}: nothing written");
            assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust", "{age_us}: still Rust's");
        } else {
            assert!(r.is_ok(), "{age_us}: {r:?}");
            assert_eq!((signal_calls_of(&o).len(), events(&o)), (0, vec!["signal_expired".to_string()]), "{age_us}");
        }
    }
}

/// §R8: `hand_back_retrying` waits until the last call's expiry + 1 s, then expires it; a restart during the wait finds
/// the call still there and waits for it again; an intent left by a call then gets the existing (longer) wait.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_cli_handback_waits_out_a_valid_call_then_the_intent_wait_follows() {
    let (_dir, l, o, s) = signal_install_with_call(now());
    let clock = TokioClock { base: now(), start: tokio::time::Instant::now() };
    // A restart during the wait: the first process is gone after 100 s, the call is still there.
    assert!(tokio::time::timeout(std::time::Duration::from_secs(100),
        handover::hand_back_retrying(&l, &o, &FakeFactory::default(), &seed::cipher(), &clock, now(), std::time::Duration::from_secs(10))).await.is_err());
    assert_eq!((signal_calls_of(&o).len(), events(&o).len()), (1, 0));
    // An order unaccounted for at the first lookup: its intent's own wait follows the call's.
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let bot = model::load_bot(&o.primary, b).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let deltabadger::engine::amount::Sizing::Place(plan) = deltabadger::engine::amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60),
        &deltabadger::ruby::BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
    placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap();
    let factory = FakeFactory(FakeVenue::new().lookup_fails(1));
    let r = handover::hand_back_retrying(&l, &o, &factory, &seed::cipher(), &clock, now(), std::time::Duration::from_secs(10)).await;
    let waited = clock.start.elapsed();
    assert!(waited >= std::time::Duration::from_micros(301_000_001 + 10_000_000), "the call's wait (to 301 s), then the intent's 10 s: {waited:?} {r:?}");
    assert_eq!(events(&o)[0], "signal_expired");
    assert!(signal_calls_of(&o).is_empty());
}

/// An index bot (top 3) on the seeded Kraken venue, admitted at takeover, and the install taken over.
fn taken_over_with_index() -> (tempfile::TempDir, deltabadger::lease::EngineLock, store::Opened, seed::Seeded, i64) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let index = seed::index_bot(&o.primary, &s, "ignored", 3, 0.0, false);
    o.primary.execute("UPDATE bots SET settings = json_remove(json_set(settings, '$.index_type', 'top'), '$.index_category_id') WHERE id = ?1", [index]).unwrap();
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    (dir, l, o, s, index)
}
fn snapshot_bots(o: &store::Opened) -> Vec<(i64, i64, String, String)> {
    o.primary.prepare("SELECT id, status, settings, transient_data FROM bots ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}

/// D-HANDBACK-RUST-CONDITIONS: Rails' DcaIndex never reads a spending cap or a trigger rule, so while an index bot has one
/// on that this engine runs, the handback is refused by bot and switch, before anything is handed back: the lease stays
/// Rust's, a signal bot's waiting call is not consumed, and the switches and met_at stay as they are. Off, it hands back.
#[tokio::test(flavor = "current_thread")]
async fn a_handback_is_refused_while_an_index_bot_runs_a_rule_rails_never_reads() {
    let (_d, l, o, s, index) = taken_over_with_index();
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', 100, '$.price_limited', json('true'), \
                       '$.sell_price_limited', json('true')), transient_data = json_set(transient_data, '$.price_limit_condition_met_at', '2026-09-29T10:00:00.000Z') WHERE id = ?1", [index]).unwrap();
    let signal = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE bots SET type = 'Bots::Signal' WHERE id = ?1", [signal]).unwrap();
    model::append_signal_call(&o.primary, signal, &model::SignalCall { source: "hook:x".into(), signal_id: 1, triggered_at: now() - chrono::Duration::days(1) }).unwrap();
    let before = snapshot_bots(&o);
    match handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await {
        Err(EngineError::Ineligible(problems)) => {
            assert_eq!(problems.len(), 2, "{problems:?}: the cap and the buy price rule; the sell side is inert on an index bot");
            assert!(problems.iter().all(|p| p.starts_with(&format!("bot {index}: "))), "{problems:?}");
            assert!(problems.iter().any(|p| p.contains("quote_amount_limited")) && problems.iter().any(|p| p.contains("price_limited")), "{problems:?}");
        }
        other => panic!("expected the refusal, got {other:?}"),
    }
    assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust", "still Rust's");
    assert_eq!(snapshot_bots(&o), before, "switches, met_at and the waiting call as they were");
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('false'), '$.price_limited', json('false')) WHERE id = ?1", [index]).unwrap();
    o.primary.execute("DELETE FROM bots WHERE id = ?1", [signal]).unwrap();
    handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap();
}

/// The second check, inside the ownership transaction: a cap switched on after the first check (here while the handback
/// settles another bot's order) still refuses the handback, and nothing of that transaction lands.
#[tokio::test(flavor = "current_thread")]
async fn a_cap_switched_on_during_the_handback_is_caught_inside_the_ownership_transaction() {
    let (dir, l, o, s, index) = taken_over_with_index();
    let b = seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let bot = model::load_bot(&o.primary, b).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let deltabadger::engine::amount::Sizing::Place(plan) = deltabadger::engine::amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60),
        &deltabadger::ruby::BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
    let intent = placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap();
    let status = model::load_bot(&o.primary, b).unwrap().status;
    let db = dir.path().join("production.sqlite3");
    let switch_on = move || {
        rusqlite::Connection::open(&db).unwrap()
            .execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true')) WHERE id = ?1", [index]).unwrap();
    };
    // The venue has the order under the intent's id: the handback settles it, and meanwhile the cap is switched on.
    let closed = json!({ "error": [], "result": { "closed": { "OTX-H": { "cl_ord_id": intent.cl_ord_id, "status": "closed", "price": "50000",
        "vol": "60", "vol_exec": "0.0012", "cost": "60", "oflags": "viqc", "descr": { "type": "buy", "ordertype": "market", "price": "0" } } } } });
    let venue = FakeVenue::from_script(&json!({ "http": { "/0/private/ClosedOrders": [closed] } })).on_lookup(switch_on.clone()).on_query(switch_on);
    let r = handover::hand_back(&l, &o, &FakeFactory(venue), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await;
    assert!(matches!(r, Err(EngineError::Ineligible(ref p)) if p.len() == 1 && p[0].contains(&format!("bot {index}: quote_amount_limited"))), "{r:?}");
    assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust", "the lease never moved");
    assert_eq!(model::load_bot(&o.primary, b).unwrap().status, status, "the transaction's status writes rolled back");
}

/// D-KUCOIN-HF-RETENTION (R08): an HF order record written after the first check (here while the handback settles another
/// bot's order) refuses inside the ownership transaction; nothing of that transaction lands.
#[tokio::test(flavor = "current_thread")]
async fn an_hf_order_recorded_during_the_handback_is_caught_inside_the_ownership_transaction() {
    let (dir, l, o, s, index) = taken_over_with_index();
    let row = seed::insert_row(&o.primary, &s, index, s.btc, &json!({"external_id": "HF1", "external_status": 2, "created_at": "2026-09-01 10:00:00"}));
    let b = seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let bot = model::load_bot(&o.primary, b).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let deltabadger::engine::amount::Sizing::Place(plan) = deltabadger::engine::amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60),
        &deltabadger::ruby::BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
    let intent = placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap();
    let db = dir.path().join("production.sqlite3");
    let record = move || {
        rusqlite::Connection::open(&db).unwrap().execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, '{\"family\":\"ClassicHf\"}', '2026-09-01', '2026-09-01') ON CONFLICT(key) DO NOTHING",
            [format!("{}{row}", deltabadger::venue::kucoin::ORDER_RECORD)]).unwrap();
    };
    let closed = json!({ "error": [], "result": { "closed": { "OTX-H": { "cl_ord_id": intent.cl_ord_id, "status": "closed", "price": "50000",
        "vol": "60", "vol_exec": "0.0012", "cost": "60", "oflags": "viqc", "descr": { "type": "buy", "ordertype": "market", "price": "0" } } } } });
    let venue = FakeVenue::from_script(&json!({ "http": { "/0/private/ClosedOrders": [closed] } })).on_lookup(record.clone()).on_query(record);
    let r = handover::hand_back(&l, &o, &FakeFactory(venue), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await;
    assert!(matches!(r, Err(EngineError::Ineligible(ref p)) if p.iter().any(|m| m == &format!("bot {index}: KuCoin HF orders are Rust-only"))), "{r:?}");
    assert_eq!(lease::read(&o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust", "the lease never moved");
}

/// r1.5, r4.3 (D-UPGRADE-COMMAND-DROPPED): a waiting command on any bot is deleted with one line (C4); nothing moves money.
#[test]
fn waiting_composition_jobs_on_any_bot_are_dropped_with_a_line() {
    for class in ["LiquidateExited", "Redeploy", "DeclineRedeploy", "ResolveLiquidation", "ResolveRedeploy"] {
        for status in [deltabadger::enums::BotStatus::Stopped, deltabadger::enums::BotStatus::Archived] {
            let dir=common::rails_install();
            let p=Paths::from_env(&|_|None,dir.path());
            let l=lease::lock(&p,now()).unwrap();
            let o=store::open(&p).unwrap();
            let s=seed::seed_kraken(&o.primary,&seed::cipher());
            let id=seed::insert_bot(&o.primary,&s,&BotSpec{status:status as i64,..BotSpec::weekly(60.0,"2026-09-01 10:00:00")});
            let class=format!("Bot::{class}Job");
            job(&o.queue,&class,&format!("gid://deltabadger/Bots::DcaMultiAsset/{id}"));
            let t=handover::take_over(&l,&o,&seed::cipher(),"test",now()).unwrap();
            assert_eq!((t.deleted_jobs, t.dropped_jobs), (1, 0), "{class} {status:?}");
            assert_eq!(bot_events(&o.primary, id), [handover::UPGRADE_COMMAND_DROPPED], "{class} {status:?}");
            assert_eq!(count(&o.queue,"SELECT count(*) FROM solid_queue_jobs"),0);
            assert_eq!(count(&o.primary,&format!("SELECT count(*) FROM transactions WHERE bot_id = {id}")),0, "nothing placed");
        }
    }
}

#[test]
fn finished_or_failed_composition_jobs_do_not_block_takeover() {
    for failed in [false,true] {
        let dir=common::rails_install();
        let p=Paths::from_env(&|_|None,dir.path());
        let l=lease::lock(&p,now()).unwrap();
        let o=store::open(&p).unwrap();
        let s=seed::seed_kraken(&o.primary,&seed::cipher());
        let id=seed::insert_bot(&o.primary,&s,&BotSpec {status:2,..BotSpec::weekly(60.0,"2026-09-01 10:00:00")});
        job(&o.queue,"Bot::ResolveLiquidationJob",&format!("gid://deltabadger/Bots::DcaMultiAsset/{id}"));
        if failed {
            o.queue.execute("INSERT INTO solid_queue_failed_executions(job_id,error,created_at) SELECT id,'failed','2026-09-30 11:00:00' FROM solid_queue_jobs",[]).unwrap();
        } else {
            o.queue.execute("UPDATE solid_queue_jobs SET finished_at='2026-09-30 11:00:00'",[]).unwrap();
        }
        assert!(handover::take_over(&l,&o,&seed::cipher(),"test",now()).is_ok());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rust_withdrawal_outcomes_and_consumed_evaluations_survive_handback_takeover() {
    let dir=common::rails_install();let p=Paths::from_env(&|_|None,dir.path());
    let lock=lease::lock(&p,now()).unwrap();let o=store::open(&p).unwrap();
    let s=seed::seed_kraken(&o.primary,&seed::cipher());
    o.primary.execute("INSERT INTO rules(type,status,user_id,exchange_id,asset_id,settings,created_at,updated_at) VALUES('Rules::Withdrawal',2,?1,?2,?3,'{}',?4,?4)",rusqlite::params![s.user_id,s.exchange_id,s.btc,deltabadger::codec::format_time(now())]).unwrap();
    let rule=o.primary.last_insert_rowid();
    let tx=model::immediate(&o.primary).unwrap();
    deltabadger::engine::rules::log(&tx,rule,Some("expired-intent"),2,"Withdrawal failed: the request was not sent in time",&json!({}),now()).unwrap();
    tx.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at,consumed_at,outcome) VALUES(?1,'start:consumed',?2,?2,'skipped')",rusqlite::params![rule,deltabadger::codec::format_time(now())]).unwrap();tx.commit().unwrap();
    handover::take_over(&lock,&o,&seed::cipher(),"test",now()).unwrap();
    handover::hand_back(&lock,&o,&FakeFactory::default(),&seed::cipher(),&deltabadger::engine::FixedClock(now())).await.unwrap();
    handover::take_over(&lock,&o,&seed::cipher(),"test",now()).unwrap();
    assert_eq!(count(&o.primary,"SELECT count(*) FROM rust_rails_attempt_acks"),0);
    assert_eq!(count(&o.primary,"SELECT count(*) FROM rule_logs"),1);
}

#[test]
fn withdrawal_cli_migrates_and_seeds_before_any_takeover() {
    let dir=common::rails_install_unmigrated();let p=Paths::from_env(&|_|None,dir.path());
    let c=rusqlite::Connection::open(&p.primary).unwrap();let s=seed::seed_kraken(&c,&seed::cipher());
    c.execute("INSERT INTO rules(type,status,user_id,exchange_id,asset_id,settings,created_at,updated_at) VALUES('Rules::Withdrawal',2,?1,?2,?3,'{}',?4,?4)",rusqlite::params![s.user_id,s.exchange_id,s.btc,deltabadger::codec::format_time(now())]).unwrap();
    let rule=c.last_insert_rowid();
    c.execute("INSERT INTO rule_logs(rule_id,status,message,details,created_at) VALUES(?1,1,'Withdrew 0.9 BTC','{\"withdrawal_id\":\"X\",\"amount\":\"0.9\"}',?2)",rusqlite::params![rule,deltabadger::codec::format_time(now())]).unwrap();
    c.execute("INSERT INTO rule_logs(rule_id,status,message,details,created_at) VALUES(?1,2,'Unexpected error: unreadable','{}',?2)",rusqlite::params![rule,deltabadger::codec::format_time(now())]).unwrap();let log=c.last_insert_rowid().to_string();
    let run=|args:&[&str]|{
        Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("resolve-withdrawal").args(args)
            .env("STORAGE_DIR",dir.path()).env("SECRET_KEY_BASE","engine-test-secret")
            .env_remove("DATABASE_PATH").env_remove("QUEUE_DATABASE_PATH")
            .env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT").env_remove("ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL")
            .output().unwrap()
    };
    let duplicate=run(&["--rails",&log,"--sent","X","--amount","0.9"]);
    assert!(!duplicate.status.success());assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already recorded"));
    assert_eq!(count(&c,"SELECT count(*) FROM rust_withdrawal_ids"),1);assert_eq!(count(&c,"SELECT count(*) FROM rust_rails_attempt_acks"),0);
    let listed=run(&["--rails-list",&rule.to_string()]);assert!(listed.status.success(),"{}",String::from_utf8_lossy(&listed.stderr));assert!(String::from_utf8_lossy(&listed.stdout).contains(&format!("Rails attempt {log}")));
    let resolved=run(&["--rails",&log,"--sent","new-id","--amount","0.9"]);assert!(resolved.status.success(),"{}",String::from_utf8_lossy(&resolved.stderr));
    assert_eq!(count(&c,"SELECT count(*) FROM rust_rails_attempt_acks"),1);assert_eq!(count(&c,"SELECT count(*) FROM rust_rule_log_origins"),1);
    assert!(lease::read(&c,&seed::cipher()).unwrap().is_none(),"resolution does not take ownership");
}

/// D-KUCOIN-HF-LEDGER: a KuCoin history the HF migration marked is Rust's (Rails reads neither HF fills nor the one-trade
/// rule), through a later ClassicLf too: the marker alone refuses the handback, before and inside its transaction.
#[tokio::test(flavor = "current_thread")]
async fn an_hf_enabled_kucoin_history_refuses_the_handback_and_a_classic_only_one_hands_back() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    o.primary.execute("INSERT INTO kucoin_coverage (user_id, exchange_id, source, start_at, end_at, updated_at) VALUES (1, 9, 'hf_enabled', '2026-09-30 12:00:00', '2026-09-30 12:00:00', '2026-09-30 12:00:00')", []).unwrap();
    let refused = handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await;
    assert!(matches!(refused, Err(EngineError::Ineligible(ref p)) if p == &["KuCoin HF history is Rust-only".to_string()]), "{refused:?}");
    o.primary.execute("DELETE FROM kucoin_coverage", []).unwrap();
    assert_eq!(handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap(), 1);
}

/// The Ruby files under `dir`, recursively.
fn ruby_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { ruby_files(&path, out) } else if path.extension().is_some_and(|e| e == "rb") { out.push(path) }
    }
}

/// PLAN-B2 r3.2, r4.5: every job class 2.x can leave in its queue has an outcome in handover::RAILS_WORK, and the table
/// names no class the repository lacks. A class added under app/jobs without an outcome fails here, not at a user's start.
#[test]
fn every_rails_job_class_has_an_outcome() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = std::collections::BTreeSet::new();
    // Every `class … < …` under app/jobs, by its full name: the `module`/`class` headers it is indented under. Error
    // classes (`< StandardError`) are not jobs.
    let header = regex::Regex::new(r"^(\s*)(module|class) ([A-Z][A-Za-z0-9_:]*)(\s*<\s*([A-Za-z0-9_:]+))?").unwrap();
    let mut files = vec![];
    ruby_files(&repo.join("app/jobs"), &mut files);
    for file in &files {
        let mut open: Vec<(usize, String)> = vec![];
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let Some(m) = header.captures(line) else { continue };
            let indent = m[1].len();
            open.retain(|(i, _)| *i < indent);
            let name = open.iter().map(|(_, n)| n.as_str()).chain([&m[3]]).collect::<Vec<_>>().join("::");
            if &m[2] == "class" && m.get(5).is_some_and(|s| s.as_str() != "StandardError") { found.insert(name); }
            open.push((indent, m[3].to_string()));
        }
    }
    assert!(found.contains("Tracker::LedgerJob::Retry") && found.contains("Bot::ActionJob"), "the class reader found {found:?}");
    let classes = found.clone();
    // Every constant `perform_later` receiver in app/ and lib/ (`X.perform_later`, `X.set(…).perform_later`). A relative
    // name (Retry inside Tracker::LedgerJob) is the class under app/jobs it ends.
    let receiver = regex::Regex::new(r"([A-Z][A-Za-z0-9_]*(?:::[A-Z][A-Za-z0-9_]*)*)(?:\.set\([^)]*\))?\.perform_later").unwrap();
    let mut files = vec![];
    ruby_files(&repo.join("app"), &mut files);
    ruby_files(&repo.join("lib"), &mut files);
    let mut receivers = 0;
    for file in &files {
        for m in receiver.captures_iter(&std::fs::read_to_string(file).unwrap()) {
            let name = &m[1];
            receivers += 1;
            found.insert(classes.iter().find(|c| *c == name || c.ends_with(&format!("::{name}"))).cloned().unwrap_or_else(|| name.to_string()));
        }
    }
    assert!(receivers > 40, "perform_later receivers read: {receivers}");
    // config/recurring.yml: each `class:`, and SolidQueue::RecurringJob for a `command:` task. The mailer's job.
    let recurring = std::fs::read_to_string(repo.join("config/recurring.yml")).unwrap();
    for line in recurring.lines().map(str::trim) {
        if let Some(class) = line.strip_prefix("class:") { found.insert(class.trim().trim_matches('"').to_string()); }
        if line.starts_with("command:") { found.insert("SolidQueue::RecurringJob".into()); }
    }
    assert!(found.contains("SolidQueue::RecurringJob"), "recurring.yml has a command: task");
    found.insert("ActionMailer::MailDeliveryJob".into());

    let table: std::collections::BTreeSet<String> = handover::RAILS_WORK.iter().map(|(c, _)| c.to_string()).collect();
    assert_eq!(table.len(), handover::RAILS_WORK.len(), "a class listed twice");
    let missing: Vec<_> = found.difference(&table).collect();
    assert!(missing.is_empty(), "Rails job classes without an outcome in handover::RAILS_WORK: {missing:?}");
    let stale: Vec<_> = table.difference(&found).collect();
    assert!(stale.is_empty(), "handover::RAILS_WORK names classes the repository no longer has: {stale:?}");
    assert_eq!(handover::rails_work("Some::FutureJob"), handover::RailsWork::Dropped, "an unknown class is dropped, never a hard stop");
}

// ---- PLAN-B2 B2-6a: Rails' queued money work at a takeover ---------------------------------------------------------------

/// A queue row of `class` with ActiveJob's serialized `arguments` (Q5), in execution state `exec` (ready, scheduled,
/// blocked, claimed, failed). Returns its row id.
fn queued(q: &rusqlite::Connection, class: &str, arguments: serde_json::Value, exec: &str) -> i64 {
    q.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, active_job_id, priority, created_at, updated_at) VALUES ('default', ?1, ?2, ?3, 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00')",
              rusqlite::params![class, json!({ "job_class": class, "arguments": arguments }).to_string(), uuid::Uuid::new_v4().to_string()]).unwrap();
    let id = q.last_insert_rowid();
    set_exec(q, id, exec);
    id
}

fn set_exec(q: &rusqlite::Connection, id: i64, exec: &str) {
    for table in ["ready", "scheduled", "blocked", "claimed", "failed"] { q.execute(&format!("DELETE FROM solid_queue_{table}_executions WHERE job_id = ?1"), [id]).unwrap(); }
    let sql = match exec {
        "ready" => "INSERT INTO solid_queue_ready_executions (job_id, queue_name, priority, created_at) VALUES (?1, 'default', 0, '2026-09-30 11:00:00')",
        "scheduled" => "INSERT INTO solid_queue_scheduled_executions (job_id, queue_name, priority, scheduled_at, created_at) VALUES (?1, 'default', 0, '2026-10-07 10:00:00', '2026-09-30 11:00:00')",
        "blocked" => "INSERT INTO solid_queue_blocked_executions (job_id, queue_name, priority, concurrency_key, expires_at, created_at) VALUES (?1, 'default', 0, 'k', '2026-09-30 12:00:00', '2026-09-30 11:00:00')",
        "claimed" => "INSERT INTO solid_queue_claimed_executions (job_id, process_id, created_at) VALUES (?1, 7, '2026-09-30 11:00:00')",
        "failed" => "INSERT INTO solid_queue_failed_executions (job_id, error, created_at) VALUES (?1, 'pruned', '2026-09-30 11:00:00')",
        other => panic!("{other}"),
    };
    q.execute(sql, [id]).unwrap();
}

/// The recording's job `name`, now claimed (a dead process held it) or failed (Solid Queue pruned that claim).
fn fail_signal_job(q: &rusqlite::Connection, fixture: &serde_json::Value, name: &str, claimed: bool) {
    let id: i64 = q.query_row("SELECT id FROM solid_queue_jobs WHERE active_job_id = ?1", [job_id(fixture, name)], |r| r.get(0)).unwrap();
    set_exec(q, id, if claimed { "claimed" } else { "failed" });
}

fn marker_of(o: &store::Opened) -> serde_json::Value {
    model::load_bot(&o.primary, 1).unwrap().transient.get(deltabadger::engine::signal::MARKER_KEY).cloned().unwrap_or(serde_json::Value::Null)
}

fn bot_events(c: &rusqlite::Connection, bot: i64) -> Vec<String> {
    let mut s = c.prepare("SELECT event FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap();
    s.query_map([bot], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// The bot's status and stop_message_key, as stored.
fn stopped_with(c: &rusqlite::Connection, bot: i64) -> (i64, Option<String>) {
    c.query_row("SELECT status, stop_message_key FROM bots WHERE id = ?1", [bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
}

fn gid(bot: i64) -> serde_json::Value { json!({ "_aj_globalid": format!("gid://deltabadger/Bots::DcaMultiAsset/{bot}") }) }

/// An install with one working Kraken DCA bot.
fn dca_install() -> (tempfile::TempDir, deltabadger::lease::EngineLock, store::Opened, seed::Seeded, i64) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = lease::lock(&p, now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    (dir, l, o, s, b)
}

fn hard_stop(r: Result<handover::Takeover, EngineError>) -> String {
    match r { Err(EngineError::HardStop(m)) => m, Err(e) => panic!("not a hard stop: {e:?}"), Ok(_) => panic!("taken over") }
}

/// r3.2, r4.5: one queue holding a row of every class in handover::RAILS_WORK (FetchAndCreateOrderJob apart: a hard stop of
/// its own) gets each row's outcome: money work handled, wakes journalled, the rest dropped and counted.
#[test]
fn a_queue_with_every_rails_job_class_gets_each_rows_outcome() {
    use handover::RailsWork;
    let (_dir, l, o, s, b) = dca_install();
    let mut rows = vec![];
    for (class, work) in handover::RAILS_WORK {
        if *work == RailsWork::FetchOrder { continue; }
        let args = match *class {
            "Bot::StopJob" => json!([gid(b), { "stop_message_key": "bot.settings.extra_amount_limit.amount_spent", "_aj_ruby2_keywords": ["stop_message_key"] }]),
            "Rule::ExecuteJob" => json!([{ "_aj_globalid": "gid://deltabadger/Rules::Withdrawal/99" }]),
            "ApiKeyValidatorJob" => json!([s.api_key_id]),
            "Ibkr::GenerateConnectionKeysJob" => json!([999]), // no such key: Rails' job returns
            _ if matches!(work, RailsWork::Wake(_)) => json!([s.user_id]),
            _ => json!([gid(b)]),
        };
        rows.push((*class, *work, queued(&o.queue, class, args, "ready")));
    }
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let left: Vec<String> = { let mut s = o.queue.prepare("SELECT class_name FROM solid_queue_jobs ORDER BY id").unwrap(); s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
    for (class, work, _) in &rows {
        let kept = matches!(work, RailsWork::Resync);
        assert_eq!(left.iter().any(|c| c == class), kept, "{class} ({work:?}): only a resync of no index bot stays");
    }
    let dropped = rows.iter().filter(|(_, w, _)| matches!(w, RailsWork::Dropped | RailsWork::IbkrKeys)).count();
    assert_eq!(wake_journal(&o), [(TRACKER_LEDGER.to_string(), s.user_id.to_string()), (PORTFOLIO_BACKFILL.to_string(), s.user_id.to_string()),
                                  (API_KEY_VALIDATOR.to_string(), s.api_key_id.to_string())]);
    // The waiting signal job names a basket, not a signal bot: Rails' discard, through the import set.
    assert_eq!(t.dropped_jobs, dropped);
    assert_eq!(stopped_with(&o.primary, b), (2, Some("bot.settings.extra_amount_limit.amount_spent".into())), "Bot::StopJob ran");
    assert_eq!(bot_events(&o.primary, b).iter().filter(|e| *e == handover::UPGRADE_COMMAND_DROPPED).count(), 5, "one line per command");
    assert!(!bot_events(&o.primary, b).iter().any(|e| e == handover::UPGRADE_ORDER_UNRECORDED), "a waiting ActionJob is no ambiguity");
}

/// r3.1 (D-UPGRADE-RAILS-AMBIGUITY-INHERITED): Rails' own ambiguity (placement_ambiguous, the successor scheduled) is
/// inherited as Rails leaves it: the successor goes, the bot gets one line (C12), and it trades again.
#[tokio::test(flavor = "current_thread")]
async fn a_completed_ambiguous_action_job_is_inherited_with_a_line_and_trading_resumes() {
    let (dir, l, o, _s, b) = dca_install();
    model::log_activity(&o.primary, b, "placement_ambiguous", model::Level::Warning, json!({ "error": "timeout" }), now() - chrono::Duration::hours(1)).unwrap();
    let done = queued(&o.queue, "Bot::ActionJob", json!([gid(b)]), "ready");
    o.queue.execute("DELETE FROM solid_queue_ready_executions WHERE job_id = ?1", [done]).unwrap();
    o.queue.execute("UPDATE solid_queue_jobs SET finished_at = '2026-09-30 11:00:00' WHERE id = ?1", [done]).unwrap();
    queued(&o.queue, "Bot::ActionJob", json!([gid(b)]), "scheduled");
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!((t.deleted_jobs, t.eligible.clone()), (1, vec![b]));
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs WHERE finished_at IS NULL"), 0, "the successor goes");
    assert_eq!(bot_events(&o.primary, b), ["placement_ambiguous", handover::UPGRADE_ORDER_UNRECORDED]);
    // A restart (lease rust) adds nothing.
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(bot_events(&o.primary, b).len(), 2);
    let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0");
    let mut e = deltabadger::engine::run::Engine::new(rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap(), FakeFactory(venue.clone()), seed::cipher(), l.clone());
    deltabadger::engine::run::step(&mut e, &deltabadger::engine::FixedClock(now())).await.unwrap();
    assert_eq!(venue.sent().len(), 1, "trading resumes");
}

/// r3.1: a claimed or failed ActionJob may have sent an order nothing records: deleted, one line (C12), once.
#[test]
fn a_claimed_or_failed_action_job_leaves_one_line_once() {
    for exec in ["claimed", "failed"] {
        let (_dir, l, o, _s, b) = dca_install();
        let row = queued(&o.queue, "Bot::ActionJob", json!([gid(b)]), exec);
        handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
        assert_eq!(bot_events(&o.primary, b), [handover::UPGRADE_ORDER_UNRECORDED], "{exec}");
        // Crash between the commit and the delete: the row is back, the set names it.
        o.queue.execute("INSERT INTO solid_queue_jobs (id, queue_name, class_name, arguments, priority, created_at, updated_at) VALUES (?1, 'default', 'Bot::ActionJob', ?2, 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00')",
                        rusqlite::params![row, json!({ "arguments": [gid(b)] }).to_string()]).unwrap();
        set_exec(&o.queue, row, exec);
        deltabadger::app_config::set_plain(&o.primary, handover::QUEUE_DONE_KEY, &json!([row]).to_string(), now()).unwrap();
        let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
        assert_eq!((t.deleted_jobs, bot_events(&o.primary, b).len()), (1, 1), "{exec}: deleted, no second line");
        assert_eq!(deltabadger::app_config::get_plain(&o.primary, handover::QUEUE_DONE_KEY).unwrap(), None);
    }
}

/// r5-3: a claimed or failed RebalanceJob is behind Rails' own barrier (rebalance_pending): the row goes, the barrier stays,
/// so no leg is placed again until the pending swap is settled; and no C12 line.
#[test]
fn a_claimed_or_failed_rebalance_job_keeps_rails_barrier() {
    for exec in ["claimed", "failed"] {
        let (_dir, l, o, _s, b) = dca_install();
        let pending = json!({ "id": 41, "sell_symbol": "BTC" });
        o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json(?1)) WHERE id = ?2", rusqlite::params![pending.to_string(), b]).unwrap();
        queued(&o.queue, "Bot::RebalanceJob", json!([gid(b)]), exec);
        let _ = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now());
        assert_eq!(model::load_bot(&o.primary, b).unwrap().transient["rebalance_pending"], pending, "{exec}: the barrier stays");
        assert!(!bot_events(&o.primary, b).contains(&handover::UPGRADE_ORDER_UNRECORDED.to_string()), "{exec}");
        assert_eq!(count(&o.primary, &format!("SELECT count(*) FROM transactions WHERE bot_id = {b}")), 0);
    }
}

/// r1.3: Bot::StopJob's amount-limit stop happens in the claim transaction, with its key, once.
#[test]
fn a_queued_stop_job_stops_the_bot_with_its_key_in_the_claim() {
    let (_dir, l, o, _s, b) = dca_install();
    queued(&o.queue, "Bot::StopJob", json!([gid(b), { "stop_message_key": "bot.settings.extra_amount_limit.amount_spent", "_aj_ruby2_keywords": ["stop_message_key"] }]), "scheduled");
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(stopped_with(&o.primary, b), (2, Some("bot.settings.extra_amount_limit.amount_spent".into())));
    assert!(t.eligible.is_empty() && t.set_aside.is_empty(), "stopped before the check: neither run nor set aside");
    assert_eq!(bot_events(&o.primary, b), ["stopped"]);
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0);
}

/// r3.1 (failed-only: Solid Queue pruned the dead claim) → the marker.
#[test]
fn a_failed_only_signal_job_becomes_the_marker() {
    let (_dir, l, o, fixture, _) = signal_install(&["scheduled"]);
    fail_signal_job(&o.queue, &fixture, "scheduled", false);
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(marker_of(&o)["id"], format!("rails-job:{}", job_id(&fixture, "scheduled")));
    assert!(signal_calls_of(&o).is_empty() && job_ids(&o.queue).is_empty());
}

/// r6 fork A: a claimed or failed job on a bot that already holds a marker (Rails') or an unresolved intent (this
/// engine's) is a hard stop naming the bot: the existing marker is never overwritten, and nothing is written.
#[test]
fn a_failed_signal_job_beside_a_marker_or_an_intent_is_a_hard_stop() {
    for existing in [json!({ "signal_placement_unresolved": { "id": "rails-1", "at": "2026-09-30T11:00:00Z", "side": "buy", "error": "timeout" } }),
                     json!({ "rust_placement": { "cl_ord_id": "cl-1" } })] {
        let (_dir, l, o, fixture, _) = signal_install(&["ready"]);
        o.primary.execute("UPDATE bots SET transient_data = ?1 WHERE id = 1", [existing.to_string()]).unwrap();
        fail_signal_job(&o.queue, &fixture, "ready", false);
        let message = hard_stop(handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()));
        assert!(message.contains("Bot 1") && message.contains("webhook call"), "{message}");
        assert_eq!(model::load_bot(&o.primary, 1).unwrap().transient, existing, "kept as it was");
        assert!(lease::read(&o.primary, &seed::cipher()).unwrap().is_none() && bot_events(&o.primary, 1).is_empty());
        assert_eq!(job_ids(&o.queue).len(), 1);
    }
}

/// r4.2: a claimed job whose rule was deleted is still a marker (never the discard path), without a side.
#[test]
fn a_claimed_signal_job_whose_rule_is_gone_is_a_marker_without_side() {
    let (_dir, l, o, fixture, _) = signal_install(&["rule_removed"]);
    fail_signal_job(&o.queue, &fixture, "rule_removed", true);
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let marker = marker_of(&o);
    assert_eq!(marker["id"], format!("rails-job:{}", job_id(&fixture, "rule_removed")));
    assert!(marker.get("side").is_none(), "{marker}");
    assert_eq!(bot_events(&o.primary, 1), [handover::UPGRADE_SIGNAL_UNRESOLVED]);
}

/// r5-4: a claimed job whose bot can't be read is a hard stop naming the queue row.
#[test]
fn a_claimed_signal_job_on_an_unreadable_bot_is_a_hard_stop_naming_the_row() {
    let (_dir, l, o, _fixture, _) = signal_install(&[]);
    o.primary.execute("UPDATE bots SET transient_data = '{not json' WHERE id = 1", []).unwrap();
    let row = queued(&o.queue, "Bot::SignalJob", json!([{ "_aj_globalid": "gid://deltabadger/Bots::Signal/1" }, { "_aj_globalid": "gid://deltabadger/BotSignal/1" }]), "claimed");
    let message = hard_stop(handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()));
    assert!(message.contains(&format!("queue row {row}")), "{message}");
    assert!(lease::read(&o.primary, &seed::cipher()).unwrap().is_none());
}

/// R6 (D-UPGRADE-UNREAD-SIGNAL): a waiting job this build can't read is deleted, never placed, with a start-log line.
#[test]
fn an_unreadable_waiting_signal_job_is_deleted_and_logged() {
    let (_dir, l, o, _fixture, _) = signal_install(&[]);
    let row = queued(&o.queue, "Bot::SignalJob", json!([{ "_aj_globalid": "gid://deltabadger/Bots::Signal/1" }, "not a rule"]), "ready");
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert!(t.log.iter().any(|line| line.contains(&format!("queue row {row}"))), "{:?}", t.log);
    assert!(signal_calls_of(&o).is_empty() && marker_of(&o).is_null() && job_ids(&o.queue).is_empty());
}

/// r4.3: a claimed LiquidateExitedJob whose `placing` marker survived becomes the ambiguous halt (nothing resumes past it);
/// without a marker (claimed before any leg) it is the C4 line. Nothing is placed either way.
#[test]
fn a_claimed_liquidation_halts_on_its_marker_or_leaves_a_line() {
    for marker in [true, false] {
        let (_dir, l, o, _s, b) = dca_install();
        if marker {
            o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.liquidation_pending', json('{\"id\":\"n1\",\"state\":\"placing\",\"symbol\":\"BTC\"}')) WHERE id = ?1", [b]).unwrap();
        }
        queued(&o.queue, "Bot::LiquidateExitedJob", json!([gid(b)]), "claimed");
        let _ = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now());
        let bot = model::load_bot(&o.primary, b).unwrap();
        if marker {
            assert_eq!(bot.transient["liquidation_pending"]["state"], "ambiguous");
            assert_eq!(bot_events(&o.primary, b), ["liquidation_ambiguous"]);
        } else {
            assert_eq!(bot_events(&o.primary, b), [handover::UPGRADE_COMMAND_DROPPED]);
        }
        assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0);
        assert_eq!(count(&o.primary, &format!("SELECT count(*) FROM transactions WHERE bot_id = {b}")), 0, "nothing placed");
    }
}

/// r6 fork A: an unfinished FetchAndCreateOrderJob holds an order Rails placed and never recorded: a hard stop naming the
/// bot and the order id, and nothing is written.
#[test]
fn a_fetch_and_create_order_job_is_a_hard_stop_naming_the_order() {
    let (_dir, l, o, _s, b) = dca_install();
    queued(&o.queue, "Bot::FetchAndCreateOrderJob", json!([gid(b), "OQCLML-BW3P3-BUCMWZ"]), "ready");
    let message = hard_stop(handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()));
    assert!(message.contains(&format!("Bot {b}")) && message.contains("OQCLML-BW3P3-BUCMWZ"), "{message}");
    assert!(lease::read(&o.primary, &seed::cipher()).unwrap().is_none());
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 1);
    assert!(bot_events(&o.primary, b).is_empty());
    assert_eq!(deltabadger::app_config::get_plain(&o.primary, handover::QUEUE_DONE_KEY).unwrap(), None);
}

/// r4.7: a failed fan-out is acknowledged and removed; its children are judged on their own; a rule job this build can't
/// read is a hard stop naming its queue row.
#[test]
fn rule_jobs_a_failed_fanout_is_acknowledged_its_children_judged_and_an_unreadable_one_stops() {
    let (_dir, l, o, _s, _b) = dca_install();
    queued(&o.queue, "Rule::EvaluateAllJob", json!([]), "failed");
    let child = queued(&o.queue, "Rule::ExecuteJob", json!([{ "_aj_globalid": "gid://deltabadger/Rules::Withdrawal/5" }]), "claimed");
    let child_job: String = o.queue.query_row("SELECT active_job_id FROM solid_queue_jobs WHERE id = ?1", [child], |r| r.get(0)).unwrap();
    let Err(EngineError::Ineligible(lines)) = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()) else { panic!("refused") };
    assert_eq!(lines.len(), 1, "the child only: {lines:?}");
    assert!(lines[0].contains(&format!("resolve-withdrawal --rails-job {child_job}")), "{lines:?}");
    o.queue.execute("DELETE FROM solid_queue_jobs WHERE id = ?1", [child]).unwrap();
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(count(&o.primary, "SELECT count(*) FROM rust_rails_job_acks WHERE resolution = 'skip'"), 1, "the fan-out is acknowledged");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0, "and removed");
    let row = queued(&o.queue, "Rule::ExecuteJob", json!(["no rule here"]), "ready");
    let message = hard_stop(handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()));
    assert!(message.contains(&format!("support must remove queue row {row}")), "{message}");
}

/// C3, C4, C12 as the activity feed prints them (web::i18n's Rust-only texts), in every locale (English for now).
#[test]
fn the_takeovers_lines_read_in_plain_words() {
    use deltabadger::web::i18n::{text, Arg};
    for locale in ["en", "de"] {
        let c3 = text(locale, &format!("bot_activity.events.{}", handover::UPGRADE_SIGNAL_UNRESOLVED), &[("call", Arg::Text("buy at 2026-09-30 11:58:01 UTC"))]);
        assert!(c3.starts_with("A webhook call (buy at 2026-09-30 11:58:01 UTC) may or may not have placed an order"), "{c3}");
        let c4 = text(locale, &format!("bot_activity.events.{}", handover::UPGRADE_COMMAND_DROPPED), &[("action", Arg::Text("redeploy"))]);
        assert_eq!(c4, "Your request to redeploy didn't run before the update. Request it again.");
        let c12 = text(locale, &format!("bot_activity.events.{}", handover::UPGRADE_ORDER_UNRECORDED), &[]);
        assert!(c12.ends_with("Check your exchange.") && !c12.contains("2."), "{c12}");
    }
}

/// Codex B2-6a P1: a handback after a crash between the claim and the queue delete finishes the queue first, so Rails is
/// never handed a row the takeover already acted on (a StopJob run twice, a dropped command run after all).
#[tokio::test(flavor = "current_thread")]
async fn a_handback_after_a_crash_before_the_queue_delete_hands_rails_none_of_those_rows() {
    let (_dir, l, o, _s, b) = dca_install();
    let row = queued(&o.queue, "Bot::RedeployJob", json!([gid(b)]), "ready");
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    o.queue.execute("INSERT INTO solid_queue_jobs (id, queue_name, class_name, arguments, priority, created_at, updated_at) VALUES (?1, 'default', 'Bot::RedeployJob', ?2, 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00')",
                    rusqlite::params![row, json!({ "arguments": [gid(b)] }).to_string()]).unwrap();
    set_exec(&o.queue, row, "ready");
    deltabadger::app_config::set_plain(&o.primary, handover::QUEUE_DONE_KEY, &json!([row]).to_string(), now()).unwrap();
    handover::hand_back(&l, &o, &FakeFactory::default(), &seed::cipher(), &deltabadger::engine::FixedClock(now())).await.unwrap();
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs WHERE class_name = 'Bot::RedeployJob'"), 0, "the dropped command never reaches Rails");
    assert_eq!(deltabadger::app_config::get_plain(&o.primary, handover::QUEUE_DONE_KEY).unwrap(), None);
}

/// Codex B2-6a P1: a soft-deleted bot's exchange order is still on the exchange, and Rails' job would still record it: a
/// hard stop, as for any bot. Only a bot whose row is gone has nothing to record into.
#[test]
fn a_fetch_and_create_order_job_of_a_deleted_bot_is_still_a_hard_stop() {
    let (_dir, l, o, _s, b) = dca_install();
    o.primary.execute("UPDATE bots SET status = ?1 WHERE id = ?2", rusqlite::params![deltabadger::enums::BotStatus::Deleted as i64, b]).unwrap();
    queued(&o.queue, "Bot::FetchAndCreateOrderJob", json!([gid(b), "OQCLML-BW3P3-BUCMWZ"]), "scheduled");
    let message = hard_stop(handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()));
    assert!(message.contains("OQCLML-BW3P3-BUCMWZ"), "{message}");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 1);
}

/// Codex B2-6a P2: Rails' own ambiguity is read before the takeover writes its lines (a dropped command's here), so a
/// takeover line never hides it.
#[test]
fn rails_last_ambiguity_still_gets_its_line_beside_a_dropped_command() {
    let (_dir, l, o, _s, b) = dca_install();
    model::log_activity(&o.primary, b, "placement_ambiguous", model::Level::Warning, json!({ "error": "timeout" }), now() - chrono::Duration::hours(1)).unwrap();
    queued(&o.queue, "Bot::RedeployJob", json!([gid(b)]), "ready");
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let lines = bot_events(&o.primary, b);
    assert!(lines.contains(&handover::UPGRADE_COMMAND_DROPPED.to_string()) && lines.contains(&handover::UPGRADE_ORDER_UNRECORDED.to_string()), "{lines:?}");
}

// ---- PLAN-B2 B2-6b: Rails' wakes, IBKR keys -----------------------------------------------------------------------------

use deltabadger::sync::jobs::API_KEY_VALIDATOR;
use deltabadger::tracker::jobs::{PORTFOLIO_BACKFILL, TRACKER_LEDGER};

/// The wake journal's (job, scope) pairs, in order.
fn wake_journal(o: &store::Opened) -> Vec<(String, String)> {
    let Some(text) = deltabadger::app_config::get_plain(&o.primary, handover::RAILS_WAKES_KEY).unwrap() else { return vec![] };
    serde_json::from_str::<Vec<serde_json::Value>>(&text).unwrap().iter().map(|w| (w["job"].as_str().unwrap().to_string(), w["scope"].as_str().unwrap().to_string())).collect()
}

/// r3.3, r4.4, r4.5: every unfinished wake row (waiting, claimed or failed) becomes one journal entry per job and scope
/// (LedgerJob and ::Retry are one), and leaves the queue; a row naming nothing there is dropped and logged. The entry is
/// woken at every start until its job has run since the takeover (D-UPGRADE-RAILS-WAKES), whatever the user's keys (here only a Kraken key, which
/// no tracker sync reads); then it goes. A user or key gone since is dropped at the start.
#[test]
fn rails_wake_rows_become_one_journal_entry_each_woken_until_their_job_runs() {
    let (_dir, l, o, s, _b) = dca_install();
    let (user, key) = (s.user_id, s.api_key_id);
    for (class, args, exec) in [("Tracker::LedgerJob", json!([user]), "ready"), ("Tracker::LedgerJob::Retry", json!([user]), "failed"),
                                ("PortfolioSnapshot::BackfillJob", json!([user, null]), "claimed"), ("ApiKeyValidatorJob", json!([key]), "scheduled")] {
        queued(&o.queue, class, args, exec);
    }
    let gone = queued(&o.queue, "Tracker::LedgerJob", json!([999]), "ready");
    let unread = queued(&o.queue, "PortfolioSnapshot::BackfillJob", json!([gid(1)]), "ready");
    let t = handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs"), 0, "every row leaves the queue");
    assert_eq!(t.dropped_jobs, 2);
    assert!(t.log.contains(&format!("dropped Tracker::LedgerJob (queue row {gone}): it names no user this install has")), "{:?}", t.log);
    assert!(t.log.contains(&format!("dropped PortfolioSnapshot::BackfillJob (queue row {unread}): it names no user this install has")), "{:?}", t.log);
    let all = vec![(TRACKER_LEDGER.to_string(), user.to_string()), (PORTFOLIO_BACKFILL.to_string(), user.to_string()), (API_KEY_VALIDATOR.to_string(), key.to_string())];
    assert_eq!(wake_journal(&o), all);
    // A restart before any run (or between a wake and its run) wakes every entry again.
    for _ in 0..2 { assert_eq!(handover::due_rails_wakes(&o.primary, now()).unwrap(), all); }
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    assert_eq!(wake_journal(&o), all, "a restart imports nothing again");
    // A run before the takeover doesn't count, and a failed one leaves its entry; a run since it does.
    deltabadger::jobs::state::record_run(&o.primary, TRACKER_LEDGER, Some(&user.to_string()), now() - chrono::Duration::hours(1)).unwrap();
    deltabadger::jobs::state::record_error(&o.primary, PORTFOLIO_BACKFILL, Some(&user.to_string()), now() + chrono::Duration::hours(1), "no prices").unwrap();
    assert_eq!(handover::due_rails_wakes(&o.primary, now()).unwrap(), all);
    deltabadger::jobs::state::record_success(&o.primary, TRACKER_LEDGER, Some(&user.to_string()), now()).unwrap();
    assert_eq!(handover::due_rails_wakes(&o.primary, now()).unwrap(), all[1..]);
    assert_eq!(wake_journal(&o), all[1..]);
    // The key is gone (Rails' job would fail on it): dropped.
    o.primary.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
    assert_eq!(handover::due_rails_wakes(&o.primary, now()).unwrap(), all[1..2]);
}

/// r4.4: a pending ApiKeyValidatorJob for a Kraken key is run by the scheduler as the binary builds it (the resolver
/// registers the key's validator on the wake): the key is validated and the entry goes at the next start.
#[tokio::test(flavor = "current_thread")]
async fn a_pending_validator_job_for_a_kraken_key_validates_it() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let (dir, l, o, s, _b) = dca_install();
    o.primary.execute("UPDATE api_keys SET status = 0 WHERE id = ?1", [s.api_key_id]).unwrap();
    queued(&o.queue, "ApiKeyValidatorJob", json!([s.api_key_id]), "failed");
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/0/private/GetApiKeyInfo")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"error": [], "result": {"permissions":
        ["query-funds", "query-open-trades", "query-closed-trades", "modify-trades", "close-trades", "query-ledger"]}}))).expect(1).mount(&server).await;
    let db = || rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let wakes = handover::due_rails_wakes(&db(), chrono::Utc::now()).unwrap();
    assert_eq!(wakes, [(API_KEY_VALIDATOR.to_string(), s.api_key_id.to_string())]);
    let api = std::rc::Rc::new(None::<deltabadger::jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>);
    let scheduler = deltabadger::jobs::Scheduler::new(db(), seed::cipher(), vec![], None)
        .with_resolver(deltabadger::jobs::resolve::all(Egress(server.uri()), api, deltabadger::tracker::jobs::system_wall()));
    for (job, scope) in &wakes { scheduler.wakers().wake(job, Some(scope), None); }
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let key = s.api_key_id;
    let (ended, ()) = tokio::join!(scheduler.run(stopped, &deltabadger::engine::SystemClock), async {
        for _ in 0..100 {
            if deltabadger::jobs::state::read(&db(), API_KEY_VALIDATOR, Some(&key.to_string())).unwrap().last_run_at.is_some() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        stop.send(true).unwrap();
    });
    ended.unwrap();
    server.verify().await;
    assert_eq!(db().query_row("SELECT status FROM api_keys WHERE id = ?1", [key], |r| r.get::<_, i64>(0)).unwrap(), 1, "validated");
    assert!(handover::due_rails_wakes(&db(), chrono::Utc::now()).unwrap().is_empty());
    assert_eq!(deltabadger::app_config::get_plain(&db(), handover::RAILS_WAKES_KEY).unwrap(), None, "the entry is gone");
}

/// Every venue's key check at the test's server.
#[derive(Clone)]
struct Egress(String);
impl deltabadger::sync::jobs::Connect for Egress {
    type T = deltabadger::venue::http::ReqwestTransport;
    fn connect(&self, c: &deltabadger::crypto::Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(), c.key.clone(), c.secret.clone()),
                                                     deltabadger::venue::alpaca::Urls { trading: self.0.clone(), data: self.0.clone() })
    }
    fn key_check_egress(&self, _: &str, _: Option<String>) -> Result<(String, reqwest::Client), String> { Ok((self.0.clone(), reqwest::Client::builder().no_proxy().build().unwrap())) }
}

/// A wake row Rails queued after an earlier takeover (a handback, Rails ran again) is new work: its entry takes the new
/// takeover's time, so a run before it does not count, though the old entry was still journalled.
#[test]
fn a_wake_row_queued_after_an_earlier_takeover_is_owed_a_new_run() {
    let (_dir, l, o, s, _b) = dca_install();
    let scope = s.user_id.to_string();
    queued(&o.queue, "Tracker::LedgerJob", json!([s.user_id]), "ready");
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now()).unwrap();
    deltabadger::jobs::state::record_success(&o.primary, TRACKER_LEDGER, Some(&scope), now() + chrono::Duration::minutes(1)).unwrap();
    queued(&o.queue, "Tracker::LedgerJob", json!([s.user_id]), "ready");
    let later = now() + chrono::Duration::hours(1);
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", later).unwrap();
    assert_eq!(handover::due_rails_wakes(&o.primary, later).unwrap(), [(TRACKER_LEDGER.to_string(), scope)]);
}
