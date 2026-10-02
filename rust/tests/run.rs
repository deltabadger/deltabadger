mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::placement;
use deltabadger::engine::venue_rules::KRAKEN;
use deltabadger::engine::{model, EngineError, FixedClock};
use deltabadger::ruby::BigDec;
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{AddOutcome, FakeFactory, FakeVenue};
use serde_json::json;

fn at(s: &str) -> FixedClock { FixedClock(s.parse::<DateTime<Utc>>().unwrap()) }
fn us(s: &str) -> i64 { s.parse::<DateTime<Utc>>().unwrap().timestamp_micros() }
fn engine(spec: BotSpec, venue: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, i64, seed::Seeded) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-09-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &spec);
    (dir, Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock), id, s)
}
fn priced() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0") }

#[tokio::test(flavor = "current_thread")]
async fn a_bot_that_never_ticked_ticks_now_and_then_just_after_its_next_checkpoint() {
    let v = priced();
    let (_d, mut e, _, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    let wake = run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    assert_eq!(v.sent().len(), 1);
    assert!(wake <= us("2026-09-01T10:00:05.5Z"), "the new order is polled 5 s later");
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap();
    assert!(run::step(&mut e, &at("2026-09-01T10:01:00Z")).await.unwrap() <= us("2026-09-01T10:02:00Z"), "idle: re-read at least every minute");
    run::step(&mut e, &at("2026-09-08T10:00:00Z")).await.unwrap();
    assert_eq!(v.sent().len(), 1, "exactly on the checkpoint is not yet after it");
    run::step(&mut e, &at("2026-09-08T10:00:00.001Z")).await.unwrap();
    assert_eq!(v.sent().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn three_weeks_of_downtime_is_one_order_for_three_weeks() {
    let v = priced();
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap();
    e.primary.execute("UPDATE transactions SET external_status = 2, quote_amount_exec = 60", []).unwrap();
    run::step(&mut e, &at("2026-09-22T10:00:01Z")).await.unwrap();
    assert_eq!(v.sent().len(), 2);
    assert_eq!(v.sent()[1].volume, "0.0036", "180 EUR at 50000: three weeks in one order");
    assert_eq!(model::load_bot(&e.primary, id).unwrap().status, deltabadger::enums::BotStatus::Scheduled);
}

#[tokio::test(flavor = "current_thread")]
async fn a_restart_polls_every_outstanding_order_and_ticks_a_retrying_bot_at_once() {
    let v = priced().order("OSTOPPED", json!({ "status": "closed", "price": "50000", "vol": "60", "vol_exec": "0.0012", "cost": "60",
                                                "oflags": "viqc", "descr": { "ordertype": "market", "price": "0" } }));
    let (_d, mut e, retrying, s) = engine(BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }, v.clone());
    e.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.last_action_job_at', '2026-09-01T10:00:01.000Z') WHERE id = ?1", [retrying]).unwrap();
    let stopped = seed::insert_bot(&e.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    seed::insert_tx(&e.primary, &s, stopped, &TxSpec { status: 0, external_status: Some(0), external_id: Some("OSTOPPED".into()), order_type: 0,
        amount: None, quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:02".into() });
    run::step(&mut e, &at("2026-09-02T09:00:00Z")).await.unwrap();
    let ext: i64 = e.primary.query_row("SELECT external_status FROM transactions WHERE external_id = 'OSTOPPED'", [], |r| r.get(0)).unwrap();
    assert_eq!(ext, 2, "an order of a stopped bot is polled at start, as Rails' adoption does");
    assert_eq!(v.sent().len(), 1, "the retrying bot ran at once instead of waiting a whole interval");
}

#[tokio::test(flavor = "current_thread")]
async fn a_follow_up_poll_survives_a_stop_and_retries_a_transient_failure() {
    let v = FakeVenue::from_script(&json!({ "http": {
        "/0/public/Ticker": [{ "error": [], "result": { "XXBTZEUR": { "a": ["50000.0", "1", "1.000"], "b": ["49990.1", "1", "1.000"], "c": ["49995.0", "0.001"], "v": ["12.5", "30.1"], "p": ["49995.0", "49995.0"], "t": [100, 250], "l": ["49995.0", "49995.0"], "h": ["49995.0", "49995.0"], "o": "49995.0" } } }],
        "/0/private/BalanceEx": [{ "error": [], "result": { "ZEUR": { "balance": "100000", "hold_trade": "0" } } }],
        "/0/private/AddOrder": [{ "error": [], "result": { "txid": ["OTX-F"] } }],
        "/0/private/QueryOrders": [{ "error": ["EService:Unavailable"] }, { "error": [], "result": { "OTX-F": {
            "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012", "cost": "60", "oflags": "", "descr": { "type": "buy", "ordertype": "market", "price": "0" } } } }]
    }}));
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap();
    e.primary.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); // stopped before the poll
    run::step(&mut e, &at("2026-09-01T10:00:06Z")).await.unwrap(); // poll fails transiently → retry in 3 s
    run::step(&mut e, &at("2026-09-01T10:00:09Z")).await.unwrap(); // retried: closed
    let ext: i64 = e.primary.query_row("SELECT external_status FROM transactions WHERE external_id = 'OTX-F'", [], |r| r.get(0)).unwrap();
    assert_eq!(ext, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stopped_bots_unresolved_placement_is_still_reconciled() {
    let v = priced().next_add(AddOutcome::AmbiguousPlaced("OTX-A".into()));
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap(); // the reply is lost: the intent stays
    assert!(model::load_bot(&e.primary, id).unwrap().rust_placement().is_some());
    e.primary.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); // the user stops it
    run::step(&mut e, &at("2026-09-01T10:00:40Z")).await.unwrap();
    assert!(model::load_bot(&e.primary, id).unwrap().rust_placement().is_none(), "settled although stopped");
    let ext: String = e.primary.query_row("SELECT external_id FROM transactions WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(ext, "OTX-A");
}

#[tokio::test(flavor = "current_thread")]
async fn one_bots_broken_data_does_not_stop_the_others() {
    let v = priced();
    let (_d, mut e, broken, s) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    let healthy = seed::insert_bot(&e.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    e.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.missed_quote_amount', 'not a number') WHERE id = ?1", [broken]).unwrap();
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap();
    assert_eq!(v.sent().len(), 1, "bot {healthy} still traded");
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_made_ineligible_while_running_stops_the_engine() {
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), priced());
    e.primary.execute("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true')) WHERE id = ?1", [id]).unwrap();
    assert!(matches!(run::step(&mut e, &at("2026-09-01T10:00:01Z")).await, Err(EngineError::Ineligible(_))));
}

#[tokio::test(flavor = "current_thread")]
async fn sub_millisecond_anchor_ticks_once_per_checkpoint() {
    let v = priced();
    let (_d, mut e, _, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00.123456"), v.clone());
    run::step(&mut e, &at("2026-09-01T10:00:00.1235Z")).await.unwrap();
    assert_eq!(v.sent().len(), 1);
    let stamp = |e: &Engine<FakeFactory>| -> String { e.primary.query_row("SELECT json_extract(transient_data, '$.last_action_job_at') FROM bots", [], |r| r.get(0)).unwrap() };
    let first = stamp(&e);
    e.primary.execute("UPDATE transactions SET external_status = 2, quote_amount_exec = 60", []).unwrap();
    run::step(&mut e, &at("2026-09-01T10:00:05.2Z")).await.unwrap();
    assert_eq!(v.sent().len(), 1);
    assert_eq!(stamp(&e), first, "the same checkpoint is not ticked again");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stale_retry_entry_does_not_spin_the_loop() {
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), priced());
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap();
    e.primary.execute("UPDATE transactions SET external_status = 2, quote_amount_exec = 60", []).unwrap();
    run::step(&mut e, &at("2026-09-01T10:00:10Z")).await.unwrap();
    e.inject_stale_retry(id, us("2026-09-01T10:00:00Z"));
    let now = us("2026-09-01T11:00:00Z");
    let wake = run::step(&mut e, &at("2026-09-01T11:00:00Z")).await.unwrap();
    assert!(wake > now + 1_000_000, "wake {wake} must not be now+1us");
}

#[tokio::test(flavor = "current_thread")]
async fn a_key_that_does_not_decrypt_fails_the_tick_and_never_stops_the_bot() {
    let v = priced();
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    let foreign = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-install").unwrap());
    e.primary.execute("UPDATE api_keys SET key = ?1, secret = ?2", [foreign.encrypt("test-key"), foreign.encrypt("dGVzdC1zZWNyZXQ=")]).unwrap();
    for t in ["2026-09-01T10:00:01Z", "2026-09-08T10:00:01Z", "2026-09-15T10:00:01Z"] { run::step(&mut e, &at(t)).await.unwrap(); }
    assert!(v.sent().is_empty(), "no AddOrder without a readable key");
    assert_eq!(model::load_bot(&e.primary, id).unwrap().status, deltabadger::enums::BotStatus::Scheduled, "never stopped as invalid_key");
}

#[tokio::test(flavor = "current_thread")]
async fn a_working_bot_eligibility_cannot_read_is_not_ticked() {
    let v = priced();
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    e.primary.execute("UPDATE users SET wash_sale_enabled = 'garbage'", []).unwrap(); // bot_reasons cannot read it
    let report = deltabadger::engine::eligibility::check_install(&e.primary).unwrap();
    assert!(report.unreadable.iter().any(|(i, _)| *i == id), "{:?}", report.unreadable);
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap();
    assert!(v.sent().is_empty(), "an unreadable bot is skipped, not traded");
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_left_executing_ticks_at_its_next_checkpoint() {
    for stuck in [4, 6] {
        let v = priced();
        let (_d, mut e, id, _) = engine(BotSpec { status: stuck, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }, v.clone());
        e.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.last_action_job_at', '2026-09-01T10:00:01.000Z') WHERE id = ?1", [id]).unwrap();
        run::step(&mut e, &at("2026-09-08T10:00:01Z")).await.unwrap();
        assert_eq!(v.sent().len(), 1, "status {stuck}: ticked at the next checkpoint");
        assert_eq!(model::load_bot(&e.primary, id).unwrap().status, deltabadger::enums::BotStatus::Scheduled, "status {stuck}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_order_recovered_in_a_working_bots_tick_gets_its_follow_up_poll() {
    let v = priced().next_add(AddOutcome::AmbiguousPlaced("OTX-R".into()));
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap(); // reply lost: the intent stays
    run::step(&mut e, &at("2026-09-01T10:00:40Z")).await.unwrap(); // the working bot's tick finds the order open by cl_ord_id
    let ext = |e: &Engine<FakeFactory>| e.primary.query_row("SELECT external_status FROM transactions WHERE external_id = 'OTX-R'", [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(ext(&e), 1, "recovered open");
    assert!(model::load_bot(&e.primary, id).unwrap().rust_placement().is_none());
    v.order("OTX-R", json!({ "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012", "cost": "60", "oflags": "", "descr": { "type": "buy", "ordertype": "market", "price": "0" } }));
    e.primary.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); // stopped: only the queued poll can settle it
    let wake = run::step(&mut e, &at("2026-09-01T10:00:41Z")).await.unwrap();
    assert!(wake <= us("2026-09-01T10:00:45Z"), "wake pulled forward to the poll");
    assert_eq!(ext(&e), 1, "not before its +5 s");
    run::step(&mut e, &at("2026-09-01T10:00:46Z")).await.unwrap();
    assert_eq!(ext(&e), 2, "the follow-up poll recorded the fill");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_request_lets_the_tick_in_hand_finish_and_starts_nothing_new() {
    let v = priced();
    let (dir, e, first, s) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    let second = seed::insert_bot(&e.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let stop = e.stop_handle();
    let _ = v.clone().on_add(move || stop.request()); // the stop lands while the first bot's AddOrder awaits its reply
    let r = run::run(e, &at("2026-09-01T10:00:00.5Z")).await;
    assert!(matches!(r, Err(EngineError::Stopped)), "{r:?}");
    assert_eq!(v.sent().len(), 1, "the second due bot is not ticked");
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let rows: Vec<(i64, i64)> = c.prepare("SELECT id, status FROM bots ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, vec![(first, 1), (second, 1)], "the first tick finished back to scheduled; the second never started");
    assert_eq!(c.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_during_recovery_starts_no_new_placement() {
    let v = priced();
    let (dir, e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v.clone());
    // An intent left by a crash before its send; at 10:05 the lookup proves it absent and the bot would tick next.
    let bot = model::load_bot(&e.primary, id).unwrap();
    let ticker = model::ticker_for(&e.primary, &bot).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), KRAKEN.minimum_logic) else { panic!() };
    placement::begin(&e.primary, &bot, &plan, &at("2026-09-01T10:00:00.5Z")).unwrap();
    let stop = e.stop_handle();
    let _ = v.clone().on_lookup(move || stop.request()); // SIGTERM lands while the venue answers the lookup
    let r = run::run(e, &at("2026-09-01T10:05:00Z")).await;
    assert!(matches!(r, Err(EngineError::Stopped)), "{r:?}");
    assert!(v.sent().is_empty(), "recovery finished, but no new order was placed after the stop");
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let intent: Option<String> = c.query_row("SELECT json_extract(transient_data, '$.rust_placement') FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(intent.is_none(), "the intent was settled as not placed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_made_due_and_notified_is_ticked_without_waiting_for_the_idle_cap() {
    let v = priced();
    let (dir, e, id, _) = engine(BotSpec::weekly(60.0, "2099-01-01 00:00:00"), v.clone()); // not due for decades
    let (wake, stop) = (e.wake_handle(), e.stop_handle());
    let db = dir.path().join("production.sqlite3");
    let driver = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await; // the loop is asleep on its 60 s idle cap by now
        let started = deltabadger::codec::format_time(chrono::Utc::now() - chrono::Duration::seconds(1));
        rusqlite::Connection::open(&db).unwrap().execute("UPDATE bots SET started_at = ?1 WHERE id = ?2", rusqlite::params![started, id]).unwrap();
        wake.notify_one(); // what the web UI does after starting a bot
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while v.sent().is_empty() && std::time::Instant::now() < deadline { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
        stop.request();
    };
    let t0 = std::time::Instant::now();
    let (r, ()) = tokio::join!(run::run(e, &deltabadger::engine::SystemClock), driver);
    assert!(matches!(r, Err(EngineError::Stopped)), "{r:?}");
    assert_eq!(v.sent().len(), 1, "ticked on the notify");
    assert!(t0.elapsed() < std::time::Duration::from_secs(30), "well inside the 60 s idle cap: {:?}", t0.elapsed());
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_stopped_while_retrying_and_started_again_retries_from_its_first_attempt() {
    let v = FakeVenue::new(); // no Ticker scripted: every price read fails transiently
    let (_d, mut e, id, _) = engine(BotSpec::weekly(60.0, "2026-09-01 10:00:00"), v);
    assert!(run::step(&mut e, &at("2026-09-01T10:00:01Z")).await.unwrap() <= us("2026-09-01T10:00:04Z"), "first failure: 3 s");
    assert!(run::step(&mut e, &at("2026-09-01T10:00:04Z")).await.unwrap() <= us("2026-09-01T10:00:22Z"), "second: 18 s");
    // The web stops it (Lifecycle#stop) and wakes the engine, which passes once; then the web starts it fresh.
    e.primary.execute("UPDATE bots SET status = 2, stopped_at = '2026-09-01 10:00:05', stop_message_key = NULL WHERE id = ?1", [id]).unwrap();
    run::step(&mut e, &at("2026-09-01T10:00:05Z")).await.unwrap();
    e.primary.execute("UPDATE bots SET status = 1, transient_data = json_remove(transient_data, '$.last_action_job_at') WHERE id = ?1", [id]).unwrap();
    let wake = run::step(&mut e, &at("2026-09-01T10:00:06Z")).await.unwrap();
    assert_eq!(model::load_bot(&e.primary, id).unwrap().status, deltabadger::enums::BotStatus::Retrying);
    assert!(wake <= us("2026-09-01T10:00:09Z"), "a fresh job: its first failure waits 3 s, not the third attempt's 83 s");
}
