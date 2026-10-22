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
    let b = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_drop_limited", json!(true)));
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
    let registry = store::Registry { migrations: &common::as_migrations(&list), tables: common::PROBE_TABLES };
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
    assert_eq!(count(&c, "SELECT count(*) FROM rust_schema_migrations"), 1, "Rust's record untouched");
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
fn stale_catalog_refuses_rails_takeover_but_allows_an_engine_owned_restart() -> Result<(), Box<dyn std::error::Error>> {
    use deltabadger::engine::{eligibility, staleness};
    let (dir, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&paths, now()).map_err(|e| format!("{e:?}"))?;
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2020-01-01 00:00:00'", [])?;
    assert!(matches!(handover::take_over(&lock, &o, &seed::cipher(), "test", now()), Err(EngineError::Ineligible(p)) if p.iter().any(|m| m.contains("reference data stale"))));
    assert!(lease::read(&o.primary, &seed::cipher()).map_err(|e| format!("{e:?}"))?.is_none());
    lease::claim(&lock, &o.primary, &seed::cipher(), "test", now()).map_err(|e| format!("{e:?}"))?;
    staleness::seed(&o.primary, now()).map_err(|e| format!("{e:?}"))?;
    let takeover = handover::take_over(&lock, &o, &seed::cipher(), "test", now()).map_err(|e| format!("{e:?}"))?;
    assert!(matches!(takeover.claim, Claim::AfterCrash));
    assert_eq!(takeover.eligible, vec![bot]);
    assert!(staleness::stale(&o.primary, &model::load_bot(&o.primary, bot).map_err(|e| format!("{e:?}"))?, now()).map_err(|e| format!("{e:?}"))?.is_some(), "restart cannot make stale data fresh");
    assert!(!eligibility::check_install_at(&o.primary, now()).map_err(|e| format!("{e:?}"))?.problems.is_empty(), "check continues reporting staleness");
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.price_drop_limited', 1)", [])?;
    assert!(matches!(handover::take_over(&lock, &o, &seed::cipher(), "test", now()), Err(EngineError::Ineligible(_))), "restart still refuses unsupported work");
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
