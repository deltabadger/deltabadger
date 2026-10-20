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
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &p, &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
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
    assert!(matches!(placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap(), Sent::Accepted(ref t) if t == "OTX-1"));
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
    assert!(matches!(placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap(), Sent::Accepted(_)));
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
    assert!(matches!(placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap(), Sent::Ambiguous(_)));
    assert!(reload(&o, &bot).rust_placement().is_some());
    let refused = FakeVenue::new().next_add(AddOutcome::NotSent("refused".into())); // the fake books a duplicate cl_ord_id, so a fresh venue
    assert!(matches!(placement::send(&o.primary, &refused, &intent, &FixedClock(t0())).await.unwrap(), Sent::NotSent(_)));
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
    let Sizing::Place(plan) = amount::size(&bot, &eth_ticker, &BigDec::from_i64(18), &BigDec::from_i64(2500), &deltabadger::engine::venue_rules::ALPACA).unwrap() else { panic!() };
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

#[tokio::test(flavor = "current_thread")]
async fn ownership_stop_in_the_dispatch_gap_sends_nothing() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().next_add(AddOutcome::Accept("should-not-leave".into()));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET status=2 WHERE id=?1",[bot.id]).unwrap();
    assert!(matches!(placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap(), Sent::Fenced));
    assert!(reload(&o, &bot).rust_placement().is_none(), "the unsent intent is cleared in the fencing transaction");
    assert!(venue.sent().is_empty(), "STOP committed before this $60 order was handed to the venue");
}

#[tokio::test(flavor = "current_thread")]
async fn the_real_kraken_adapter_is_fenced_and_recorded_like_any_venue() {
    use deltabadger::venue::{http::ScriptedTransport, kraken::KrakenVenue};
    let (_d, o, bot, plan) = setup();
    let script = json!({ "POST /0/private/AddOrder": [{ "status": 200, "body": { "error": [], "result": { "txid": ["OTX-K"] } } }] });
    let transport = ScriptedTransport::from_script(&script);
    let venue = KrakenVenue::new(transport.clone());
    // A STOP between the intent and the send: the real adapter is never called.
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET status=2 WHERE id=?1", [bot.id]).unwrap();
    assert!(matches!(placement::send_recorded(&o.primary, &venue, &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Fenced));
    assert!(transport.requests().is_empty(), "a fenced Kraken order never reaches the transport");
    // The owning run's send goes out and is recorded with that run's identity.
    let (_d, o, bot, plan) = setup();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert!(matches!(placement::send_recorded(&o.primary, &venue, &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Accepted(id) if id == "OTX-K"));
    assert_eq!(transport.requests().len(), 1);
    let identity: Option<String> = o.primary.query_row("SELECT run_identity FROM transactions WHERE external_id='OTX-K'", [], |r| r.get(0)).unwrap();
    assert!(identity.is_some(), "the accepted Kraken order carries the run that sent it");
    assert!(reload(&o, &bot).rust_placement().is_none());
}

struct StopDuringDispatch {
    inner: FakeVenue,
    db: String,
    bot_id: i64,
    stopper: std::cell::RefCell<Option<std::thread::JoinHandle<()>>>,
}
impl deltabadger::venue::Venue for StopDuringDispatch {
    fn rules(&self) -> &'static deltabadger::engine::venue_rules::VenueRules { self.inner.rules() }
    async fn price(&self, t: &model::Ticker, s: deltabadger::venue::PriceSide) -> Result<BigDec, deltabadger::venue::VenueError> { self.inner.price(t,s).await }
    async fn add_order(&self, o: &deltabadger::venue::NewOrder) -> Result<String,deltabadger::venue::VenueError> {
        let (started_tx,started_rx)=std::sync::mpsc::channel();
        let (done_tx,done_rx)=std::sync::mpsc::channel();
        let db=self.db.clone(); let id=self.bot_id;
        *self.stopper.borrow_mut()=Some(std::thread::spawn(move || {
            let c=rusqlite::Connection::open(db).unwrap();
            c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            started_tx.send(()).unwrap();
            c.execute("UPDATE bots SET status=2, transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1",[id]).unwrap();
            let _=done_tx.send(());
        }));
        started_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(6)); // reply exceeds production busy_timeout
        done_rx.recv_timeout(std::time::Duration::from_millis(100)).expect("STOP must commit before the slow reply");
        self.inner.add_order(o).await
    }
    async fn orders(&self, ids: &[String]) -> Result<Vec<deltabadger::venue::OrderState>,deltabadger::venue::VenueError> { self.inner.orders(ids).await }
    async fn order_by_client_id(&self, id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<deltabadger::venue::OrderState>,deltabadger::venue::VenueError> { self.inner.order_by_client_id(id,symbol,since).await }
    async fn fills_from_trades(&self, ids: &[String], since: DateTime<Utc>) -> Result<Vec<deltabadger::venue::OrderState>,deltabadger::venue::VenueError> { self.inner.fills_from_trades(ids,since).await }
    async fn balance(&self, asset: &str, crypto: bool) -> Result<BigDec,deltabadger::venue::VenueError> { self.inner.balance(asset,crypto).await }
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_stop_commits_during_slow_dispatch_and_accepted_order_is_recorded() {
    let (_d,o,bot,plan)=setup();
    let v=StopDuringDispatch { inner:FakeVenue::new().next_add(AddOutcome::Accept("transmitted".into())),db:o.primary.path().unwrap().into(),bot_id:bot.id,stopper:std::cell::RefCell::new(None) };
    let intent=placement::begin(&o.primary,&bot,&plan,&FixedClock(t0())).unwrap();
    assert!(matches!(placement::send_recorded(&o.primary, &v,&bot,&intent,&FixedClock(t0())).await.unwrap(),Sent::Accepted(_)));
    v.stopper.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(reload(&o,&bot).status,deltabadger::enums::BotStatus::Stopped);
    assert_eq!(count(&o,"SELECT count(*) FROM transactions WHERE external_id='transmitted'"),1);
    assert!(reload(&o,&bot).rust_placement().is_none());
    let identity: String = o.primary.query_row("SELECT run_identity FROM transactions WHERE external_id='transmitted'", [], |r|r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&identity).unwrap()[0], 0, "attribute the order to the run that sent it");
}

#[tokio::test(flavor = "current_thread")]
async fn divergence_same_timestamp_restart_fences_the_old_order() {
    assert_eq!(common::ownership_case("same_timestamp_restart")["rails_fenced"], false);
    let (_d,o,bot,plan)=setup();
    let v=FakeVenue::new().next_add(AddOutcome::Accept("old-run".into()));
    let intent=placement::begin(&o.primary,&bot,&plan,&FixedClock(t0())).unwrap();
    // Resume preserves started_at and last_action_job_at, even when the clock has not advanced.
    o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1",[bot.id]).unwrap();
    assert!(matches!(placement::send_recorded(&o.primary, &v,&bot,&intent,&FixedClock(t0())).await.unwrap(), Sent::Fenced));
    assert!(reload(&o,&bot).rust_placement().is_none(), "a known-unsent order must not delay the resumed run");
    assert!(v.sent().is_empty(), "a resumed run is a new owner even at identical timestamps");
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_recovery_does_not_defer_a_restarted_run() {
    let (_d,o,bot,plan)=setup();
    placement::begin(&o.primary,&bot,&plan,&FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET status=4, transient_data=json_set(transient_data,'$.run_generation',1,'$.last_action_job_at','2026-09-30T12:00:01.000Z') WHERE id=?1",[bot.id]).unwrap();
    assert!(matches!(placement::recover(&o.primary,&FakeVenue::new(),&reload(&o,&bot),&FixedClock(t0()+Duration::seconds(70))).await.unwrap(),Recovery::NotPlaced));
    let current=reload(&o,&bot);
    assert!(current.rust_placement().is_none(),"the old intent was conclusively resolved");
    assert!(current.rust_defer_until_us().unwrap().is_none(),"an old intent cannot defer the new run");
    assert_eq!(current.status,deltabadger::enums::BotStatus::Executing);
}

#[test]
fn ownership_resuming_at_the_same_timestamp_invalidates_a_persisted_wait() {
    let (_d,o,bot,_)=setup();
    placement::defer_to_next_checkpoint(&o.primary,&bot,t0()).unwrap();
    let previous=bot.schedule_key().unwrap();
    o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1",[bot.id]).unwrap();
    assert_ne!(reload(&o,&bot).schedule_key().unwrap(),previous,"a deferral belongs to one lifecycle generation");
}

#[test]
fn accepted_order_is_a_fact_even_if_credentials_rotate_before_recording() {
    let (_d, o, bot, plan) = setup();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE api_keys SET key='rotated' WHERE user_id=?1", [bot.user_id]).unwrap();
    placement::record_accepted(&o.primary, &bot, &intent, "accepted-old-credentials").unwrap();
    assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE external_id='accepted-old-credentials'"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_fenced_cleanup_preserves_a_different_or_dispatched_intent() {
    for dispatched in [false, true] {
        let (_d, o, bot, plan) = setup();
        let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
        o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1", [bot.id]).unwrap();
        if dispatched {
            o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement.dispatched',json('true')) WHERE id=?1", [bot.id]).unwrap();
        } else {
            o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement.cl_ord_id','new-owner') WHERE id=?1", [bot.id]).unwrap();
        }
        let before = reload(&o, &bot).rust_placement();
        let venue = FakeVenue::new();
        assert!(matches!(placement::send_recorded(&o.primary, &venue, &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Fenced));
        assert_eq!(reload(&o, &bot).rust_placement(), before);
        assert!(venue.sent().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_rechecks_generation_after_waiting_for_the_dispatch_write_lock() {
    let (_d, o, bot, plan) = setup();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    let path = o.primary.path().unwrap().to_owned();
    let id = bot.id;
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let stopper = std::thread::spawn(move || {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1", [id]).unwrap();
        held_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        c.execute_batch("COMMIT").unwrap();
    });
    held_rx.recv().unwrap();
    let venue = FakeVenue::new();
    assert!(matches!(placement::send_recorded(&o.primary, &venue, &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Fenced));
    stopper.join().unwrap();
    assert!(venue.sent().is_empty(), "the check must see STOP committed during the lock wait");
    assert!(reload(&o, &bot).rust_placement().is_none());
}

#[test]
fn divergence_stale_tick_completion_cannot_write_over_a_new_run() {
    let rails = common::ownership_case("stale_tick_completion");
    assert_eq!(rails["rails_write_succeeded"], true);
    assert_eq!(rails["rails_status"], "waiting");
    let (_d, o, bot, _) = setup();
    o.primary.execute("UPDATE bots SET status=4, transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1", [bot.id]).unwrap();
    assert!(matches!(model::transition_owned(&o.primary, &bot, deltabadger::enums::BotStatus::Waiting, t0()), Err(deltabadger::engine::EngineError::RunFenced)));
    assert_eq!(reload(&o, &bot).status, deltabadger::enums::BotStatus::Executing);
}

#[tokio::test(flavor = "current_thread")]
async fn a_binance_minus_1021_writes_no_row_clears_the_intent_and_a_plain_refusal_writes_a_failed_row() {
    use deltabadger::venue::{binance::BinanceVenue, http::ScriptedTransport};
    for (msg, rows) in [("Timestamp for this request is outside of the recvWindow.", 0),
                        ("Timestamp for this request was 1000ms ahead of the server's time.", 0),
                        ("Account has insufficient balance for requested action.", 1)] {
        let (_d, o, bot, plan) = setup();
        let t = ScriptedTransport::default();
        t.reply("POST /api/v3/order", 400, json!({ "code": -1021, "msg": msg }));
        let venue = BinanceVenue::new(t.clone(), "https://api.binance.com", "s");
        let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
        assert!(matches!(placement::send_recorded(&o.primary, &venue, &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Rejected(_)), "{msg}");
        assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE status = 1"), rows, "{msg}");
        assert!(reload(&o, &bot).rust_placement().is_none(), "{msg}: a definitive answer clears the intent");
        assert_eq!(t.requests().len(), 1, "sent once");
    }
}

fn binance_setup() -> (tempfile::TempDir, store::Opened, model::Bot, amount::OrderPlan) {
    let (d, o, bot, _) = setup();
    o.primary.execute_batch("UPDATE exchanges SET type = 'Exchanges::Binance', name = 'Binance'; UPDATE tickers SET ticker = 'BTCEUR', base = 'BTC'").unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::BINANCE).unwrap() else { panic!() };
    (d, o, bot, plan)
}
fn binance(t: &deltabadger::venue::http::ScriptedTransport, key: &str) -> deltabadger::venue::binance::BinanceVenue<deltabadger::venue::http::ScriptedTransport> {
    let creds = deltabadger::crypto::Credentials { key: key.into(), secret: "s".into(), passphrase: None, redaction_values: vec![] };
    deltabadger::venue::binance::BinanceVenue::new(t.clone(), "https://api.binance.com", "s").with_identity(&creds)
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_asks_only_under_the_key_and_host_the_order_was_sent_with_and_for_the_symbol_it_was_sent_for() {
    use deltabadger::venue::http::ScriptedTransport;
    let (_d, o, bot, plan) = binance_setup();
    let lost = ScriptedTransport::default();
    lost.network("POST /api/v3/order", "post_send", "read timeout");
    // As the tick merges the sending venue's identity into the intent (begin_with_credentials).
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    let id = deltabadger::venue::Venue::placement_identity(&binance(&lost, "KEY-A")).unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.host', ?1, '$.rust_placement.key_fingerprint', ?2) WHERE id = ?3",
                      rusqlite::params![id["host"].as_str(), id["key_fingerprint"].as_str(), bot.id]).unwrap();
    let raw = reload(&o, &bot).rust_placement().unwrap();
    assert_eq!((raw["symbol"].as_str(), raw["host"].as_str()), (Some("BTCEUR"), Some("https://api.binance.com")), "{raw}");
    assert!(matches!(placement::send_recorded(&o.primary, &binance(&lost, "KEY-A"), &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Ambiguous(_)));
    // The pair is renamed after the send: recovery still asks for the symbol the order was sent for.
    o.primary.execute("UPDATE tickers SET ticker = 'RENAMEDEUR'", []).unwrap();
    let later = FixedClock(t0() + Duration::hours(1));
    let replaced = ScriptedTransport::default();
    assert!(matches!(placement::recover(&o.primary, &binance(&replaced, "KEY-B"), &reload(&o, &bot), &later).await.unwrap(), Recovery::Pending));
    assert!(replaced.requests().is_empty(), "another key's account is never asked");
    let elsewhere = ScriptedTransport::default();
    let other_host = deltabadger::venue::binance::BinanceVenue::new(elsewhere.clone(), "https://api.binance.us", "s")
        .with_identity(&deltabadger::crypto::Credentials { key: "KEY-A".into(), secret: "s".into(), passphrase: None, redaction_values: vec![] });
    assert!(matches!(placement::recover(&o.primary, &other_host, &reload(&o, &bot), &later).await.unwrap(), Recovery::Pending));
    assert!(elsewhere.requests().is_empty(), "another host is never asked");
    let same = ScriptedTransport::default();
    same.reply("GET /api/v3/order", 400, json!({ "code": -2013, "msg": "Order does not exist." }));
    assert!(matches!(placement::recover(&o.primary, &binance(&same, "KEY-A"), &reload(&o, &bot), &later).await.unwrap(), Recovery::Pending),
            "-2013 does not prove absence until a recorded answer confirms it");
    let asked = &same.requests()[0];
    assert!(asked.query.iter().any(|(k, v)| *k == "symbol" && v == "BTCEUR") && asked.query.iter().any(|(k, v)| *k == "origClientOrderId" && *v == intent.cl_ord_id), "{asked:?}");
    assert!(reload(&o, &bot).rust_placement().is_some(), "the intent stays");
}

// ---- 6a: a sell's intent records its side, and recovery only accepts an answer on that side

fn sell_plan(o: &store::Opened, bot: &model::Bot) -> amount::OrderPlan {
    let ticker = model::ticker_for(&o.primary, bot).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size_sell(bot, &ticker, &BigDec::parse("0.0012").unwrap(), &BigDec::from_i64(50_000)).unwrap() else { panic!() };
    plan
}
fn closed(kind: &str) -> serde_json::Value {
    json!({ "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012", "cost": "60", "oflags": "", "descr": { "ordertype": "market", "price": "0", "type": kind } })
}

#[tokio::test(flavor = "current_thread")]
async fn a_recovered_sell_is_written_as_a_sell() {
    let (_d, o, bot, _) = setup();
    let plan = sell_plan(&o, &bot);
    let venue = FakeVenue::new().next_add(AddOutcome::AmbiguousPlaced("OTX-S".into())).order("OTX-S", closed("sell"));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    assert_eq!(reload(&o, &bot).rust_placement().unwrap()["side"], "sell");
    assert!(matches!(placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap(), Sent::Ambiguous(_)));
    assert_eq!(venue.sent()[0].side, deltabadger::venue::Side::Sell);
    let later = FixedClock(t0() + Duration::hours(2));
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &later).await.unwrap(), Recovery::Recorded(_)));
    assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE side = 1 AND external_id = 'OTX-S'"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_on_the_other_side_is_not_the_intents_order_and_the_intent_stays() {
    let (_d, o, bot, _) = setup();
    let plan = sell_plan(&o, &bot);
    let venue = FakeVenue::new().next_add(AddOutcome::AmbiguousPlaced("OTX-B".into())).order("OTX-B", closed("buy"));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap();
    let later = FixedClock(t0() + Duration::hours(2));
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &later).await.unwrap(), Recovery::Pending));
    assert!(reload(&o, &bot).rust_placement().is_some());
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_intent_from_before_selling_names_no_side_and_is_a_buy() {
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().next_add(AddOutcome::AmbiguousPlaced("OTX-1".into())).order("OTX-1", closed("buy"));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.side') WHERE id = ?1", [bot.id]).unwrap();
    let later = FixedClock(t0() + Duration::hours(2));
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &later).await.unwrap(), Recovery::Recorded(_)));
    assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE side = 0"), 1);
    // The same old intent answered with a sell stays unresolved: a missing side never reads as a sell.
    let (_d, o, bot, plan) = setup();
    let venue = FakeVenue::new().next_add(AddOutcome::AmbiguousPlaced("OTX-1".into())).order("OTX-1", closed("sell"));
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    placement::send(&o.primary, &venue, &intent, &FixedClock(t0())).await.unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.side') WHERE id = ?1", [bot.id]).unwrap();
    assert!(matches!(placement::recover(&o.primary, &venue, &reload(&o, &bot), &later).await.unwrap(), Recovery::Pending));
}
