mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{model, EngineError, FixedClock};
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
