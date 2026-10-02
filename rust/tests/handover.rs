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
        &px, deltabadger::engine::venue_rules::KRAKEN.minimum_logic).unwrap() else { panic!() };
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
        .env("RAILS_ENV", "development").env_remove("DATABASE_URL").env("APP_ROOT_URL", "http://localhost:3000")
        .env("PRIMARY_DATABASE_URL", format!("sqlite3:{}", dir.join("production.sqlite3").display()))
        .env("QUEUE_DATABASE_URL", format!("sqlite3:{}", dir.join("production_queue.sqlite3").display()))
        .env("CACHE_DATABASE_URL", format!("sqlite3:{}/cache.sqlite3", scratch.path().display()))
        .env("CABLE_DATABASE_URL", format!("sqlite3:{}/cable.sqlite3", scratch.path().display()))
        .env("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", keys.primary_key).env("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", keys.key_derivation_salt)
        .output().expect("bin/rails runs");
    (out.status.success() && String::from_utf8_lossy(&out.stdout).contains("booted"), String::from_utf8_lossy(&out.stderr).into_owned())
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
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60), &deltabadger::ruby::BigDec::from_i64(64_000), ALPACA.minimum_logic).unwrap() else { panic!() };
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
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60), &deltabadger::ruby::BigDec::from_i64(64_000), ALPACA.minimum_logic).unwrap() else { panic!() };
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
        &px, deltabadger::engine::venue_rules::KRAKEN.minimum_logic).unwrap() else { panic!() };
    let intent = placement::begin(&o.primary, &bot, &plan, &deltabadger::engine::FixedClock(now())).unwrap(); // sent; its reply lost
    // Another plain cryptocurrency on the same venue.
    o.primary.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) \
                       VALUES ('ethereum', 'ETH', 'Ethereum', 'Cryptocurrency', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", []).unwrap();
    let eth = o.primary.last_insert_rowid();
    o.primary.execute(
        "INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
         minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
         VALUES (?1, 'ETHEUR', 'ETH', 'EUR', ?2, ?3, 8, 5, 2, '0.002', '0.5', 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
        rusqlite::params![s.exchange_id, eth, s.quote]).unwrap();
    // The web saves the bot onto ETH while its BTC order is unresolved.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json(?1)) WHERE id = ?2",
               rusqlite::params![json!({ eth.to_string(): 1.0 }).to_string(), b]).unwrap();
    assert!(eligibility::check_install(&tx).unwrap().problems.is_empty(), "eligible, as far as check_install sees");
    let refused = eligibility::guard(&tx, &seed::cipher(), b).unwrap_err();
    let line = format!("bot {b}: an order is still being reconciled; its asset, exchange and quote cannot change until it settles");
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
