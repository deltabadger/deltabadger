mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::placement::{self, OperatorResolution, Recovery, Sent};
use deltabadger::engine::{model, FixedClock, SteppingClock};
use deltabadger::ruby::BigDec;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{AddOutcome, FakeVenue};
use serde_json::json;

fn t0() -> DateTime<Utc> { "2026-09-30T12:00:00Z".parse().unwrap() }
fn setup() -> (tempfile::TempDir, store::Opened, model::Bot, amount::OrderPlan) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let p = BigDec::from_i64(50_000);
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &p, deltabadger::engine::venue_rules::KRAKEN.minimum_logic) else { panic!() };
    (dir, o, bot, plan)
}
fn count(o: &store::Opened, sql: &str) -> i64 { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
fn reload(o: &store::Opened, b: &model::Bot) -> model::Bot { model::load_bot(&o.primary, b.id).unwrap() }

#[tokio::test(flavor = "current_thread")]
async fn the_intent_is_committed_with_a_deadline_from_the_moment_of_commit() {
    let (_d, o, bot, plan) = setup();
    let clock = SteppingClock::new(t0(), Duration::seconds(7)); // time passes between reads
    let _ = deltabadger::engine::Clock::now(&clock); // e.g. the tick read the clock earlier
    let intent = placement::begin(&o.primary, &bot, &plan, &clock).unwrap();
    assert_eq!(intent.at, t0() + Duration::seconds(7), "read at commit, not earlier in the tick");
    assert_eq!(intent.deadline, intent.at + Duration::seconds(10));
    assert_eq!(reload(&o, &bot).rust_placement().unwrap()["cl_ord_id"], intent.cl_ord_id.as_str());
    assert!(placement::begin(&o.primary, &reload(&o, &bot), &plan, &clock).is_err(), "a second intent is refused while one exists");
}

#[tokio::test(flavor = "current_thread")]
async fn a_crash_after_kraken_accepted_is_recorded_exactly_once_with_its_fill_and_original_time() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().next_add(AddOutcome::Accept("OTX-1".into())).order("OTX-1", json!({
        "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012", "cost": "60", "oflags": "", "descr": { "ordertype": "market", "price": "0" } }));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(matches!(placement::send(&venue, &intent, &FixedClock(t0())).await, Sent::Accepted(ref t) if t == "OTX-1"));
    // crash here: the reply never reached the database
    let later = FixedClock(t0() + Duration::hours(2));
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &later).await.unwrap(), Recovery::Recorded(_)));
    let (ext, created): (i64, String) = o.primary.query_row("SELECT external_status, created_at FROM transactions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((ext, created.as_str()), (2, "2026-09-30 12:00:00"), "closed in the same commit, dated when it was placed");
    assert!(reload(&o, &bot).rust_placement().is_none());
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &later).await.unwrap(), Recovery::NoIntent));
    assert_eq!((venue.sent().len(), count(&o, "SELECT count(*) FROM transactions")), (1, 1), "never a second order or row");
}

#[tokio::test(flavor = "current_thread")]
async fn a_crash_before_the_send_is_not_placed_only_after_deadline_plus_a_minute() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    let at = |s: i64| FixedClock(t0() + Duration::seconds(s));
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &at(69)).await.unwrap(), Recovery::Pending));
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &at(70)).await.unwrap(), Recovery::NotPlaced));
    assert!(reload(&o, &bot).rust_placement().is_none());
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 0);
    assert_eq!(count(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'placement_ambiguous'"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_incomplete_lookup_keeps_the_intent_however_late_it_is() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().lookup_fails(1);
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &FixedClock(t0() + Duration::days(3))).await.unwrap(), Recovery::Pending));
    assert!(reload(&o, &bot).rust_placement().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejection_writes_a_failed_row_with_krakens_errors_and_clears_the_intent() {
    let (_d, o, bot, plan) = setup();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(placement::record_rejected(&o.primary, &bot, &intent, &["EOrder:Insufficient funds".to_string()]).unwrap());
    let (status, errors): (i64, String) = o.primary.query_row("SELECT status, error_messages FROM transactions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((status, errors.as_str()), (1, "[\"EOrder:Insufficient funds\"]"));
    assert!(reload(&o, &bot).rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn an_operator_decision_resolves_an_intent_kraken_cannot_answer_for() {
    let (_d, o, bot, plan) = setup();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::Placed("OTX-HUMAN".into()), t0()).unwrap();
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 1);
    assert!(reload(&o, &bot).rust_placement().is_none());
    let next: DateTime<Utc> = "2026-10-06T10:00:00Z".parse().unwrap();
    assert_eq!(reload(&o, &bot).rust_defer_until_us().unwrap(), Some(next.timestamp_micros()), "the wait for the next checkpoint is persisted");
    assert!(placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::NotPlaced, t0()).is_err(), "nothing left to resolve");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stale_snapshot_cannot_record_an_order_twice() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().next_add(AddOutcome::Accept("OTX-1".into()));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(matches!(placement::send(&venue, &intent, &FixedClock(t0())).await, Sent::Accepted(_)));
    let stale = reload(&o, &bot); // still carries the intent
    placement::record_accepted(&o.primary, &stale, &intent, "OTX-1").unwrap();
    let later = FixedClock(t0() + Duration::hours(2));
    assert!(matches!(placement::recover(&o.primary, &venue, &stale, &later).await.unwrap(), Recovery::NoIntent));
    assert!(placement::record_accepted(&o.primary, &stale, &intent, "OTX-1").is_err());
    assert!(placement::record_rejected(&o.primary, &stale, &intent, &["EOrder:x".to_string()]).is_err());
    assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE external_id = 'OTX-1'"), 1);
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_operator_cannot_record_a_blank_or_already_recorded_order_id() {
    let (_d, o, bot, plan) = setup();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    for blank in ["", "  "] {
        assert!(placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::Placed(blank.into()), t0()).is_err());
    }
    o.primary.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, created_at, updated_at) SELECT id, exchange_id, 'OTX-DUP', 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00' FROM bots WHERE id = ?1", [bot.id]).unwrap();
    assert!(placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::Placed("OTX-DUP".into()), t0()).is_err());
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 1);
    assert!(reload(&o, &bot).rust_placement().is_some(), "nothing was written");
}

#[tokio::test(flavor = "current_thread")]
async fn an_intent_missing_a_boolean_is_unreadable_not_false() {
    let (_d, o, bot, plan) = setup();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.limit') WHERE id = ?1", [bot.id]).unwrap();
    let venue = FakeVenue::new();
    assert!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &FixedClock(t0() + Duration::days(1))).await.is_err());
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.limit', 'false') WHERE id = ?1", [bot.id]).unwrap();
    assert!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &FixedClock(t0() + Duration::days(1))).await.is_err());
    assert!(reload(&o, &bot).rust_placement().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn a_placement_safe_rejection_writes_no_row_and_clears_the_intent() {
    let (_d, o, bot, plan) = setup();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    let msg = format!("EAPI:{}", placement::PLACEMENT_SAFE_TRANSIENT_ERRORS[0]);
    assert!(!placement::record_rejected(&o.primary, &bot, &intent, &[msg]).unwrap());
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 0);
    assert!(reload(&o, &bot).rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn an_ambiguous_send_keeps_the_intent_and_a_not_sent_one_can_drop_it() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().next_add(AddOutcome::AmbiguousPlaced("OTX-9".into()));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(matches!(placement::send(&venue, &intent, &FixedClock(t0())).await, Sent::Ambiguous(_)));
    assert!(reload(&o, &bot).rust_placement().is_some());
    let refused = FakeVenue::new().next_add(AddOutcome::NotSent("refused".into())); // the fake books a duplicate cl_ord_id, so a fresh venue
    assert!(matches!(placement::send(&refused, &intent, &FixedClock(t0())).await, Sent::NotSent(_)));
    placement::drop_intent(&o.primary, bot.id).unwrap();
    assert!(reload(&o, &bot).rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_basket_legs_intent_names_its_own_ticker_and_is_recorded_on_it() {
    use common::scripted::{ok, script, venue};
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").weights(&[(s.btc, 0.7), (eth, 0.3)]));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let eth_ticker = model::ticker_for_asset(&o.primary, &bot, eth).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size(&bot, &eth_ticker, &BigDec::from_i64(18), &BigDec::from_i64(2500), deltabadger::engine::venue_rules::ALPACA.minimum_logic) else { panic!() };
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    let stored = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap();
    assert_eq!((stored["ticker_id"].as_i64(), stored["base_asset_id"].as_i64()), (Some(eth_ticker.id), Some(eth)), "the leg's own pair, not the first member's");
    let t = script(json!({ "GET /v2/orders:by_client_order_id": [ok(json!({ "id": "OTX-E", "client_order_id": intent.cl_ord_id, "status": "filled",
        "symbol": "ETH/USD", "type": "market", "side": "buy", "notional": "18", "qty": null, "filled_qty": "0.0072", "filled_avg_price": "2500", "limit_price": null }))] }));
    let recovered = placement::recover(&o.primary, &venue(&t), &model::load_bot(&o.primary, id).unwrap(), &FixedClock(t0() + Duration::seconds(5))).await.unwrap();
    assert!(matches!(recovered, Recovery::Recorded(_)), "{recovered:?}");
    let (asset, base, exec): (i64, String, f64) = o.primary.query_row(
        "SELECT base_asset_id, base, quote_amount_exec FROM transactions WHERE external_id = 'OTX-E'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((asset, base.as_str(), exec), (eth, "ETH", 18.0));
}

#[test]
fn a_legacy_intent_without_base_asset_id_still_resolves() {
    let (_d, o, bot, plan) = setup();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.base_asset_id') WHERE id = ?1", [bot.id]).unwrap();
    placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::NotPlaced, t0()).unwrap();
    assert!(reload(&o, &bot).rust_placement().is_none());
}

#[test]
fn an_intent_naming_another_venues_ticker_is_unreadable() {
    let (_d, o, bot, plan) = setup();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.ticker_id', 999999) WHERE id = ?1", [bot.id]).unwrap();
    match placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::NotPlaced, t0()) {
        Err(deltabadger::engine::EngineError::Data(m)) => assert!(m.contains("rust_placement"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_intent_without_its_snapshot_counts_as_stranded_and_begin_records_one() {
    let (_d, o, bot, plan) = setup();
    placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(placement::stranded(&o.primary).unwrap().is_empty(), "begin records what the order was sent under");
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.allocations') WHERE id = ?1", [bot.id]).unwrap();
    assert_eq!(placement::stranded(&o.primary).unwrap(), vec![bot.id], "no snapshot: fail closed");
}
