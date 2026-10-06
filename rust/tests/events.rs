//! What the engine announces after a tick: each order row it recorded (for the ledger sync) and each spent out-of-funds
//! budget (the mail service's wake). The funds marker itself is pinned in tests/tick.rs.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::FixedClock;
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{AddOutcome, FakeFactory, FakeVenue};
use rusqlite::Connection;
use tokio::sync::mpsc::UnboundedReceiver;

fn at(s: &str) -> FixedClock { FixedClock(s.parse::<DateTime<Utc>>().unwrap()) }
fn drained(rx: &mut UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> { std::iter::from_fn(|| rx.try_recv().ok()).collect() }
fn rows(c: &Connection) -> Vec<(i64, i64, i64)> {
    c.prepare("SELECT bot_id, id, status FROM transactions ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
/// 1 EUR free after each buy: under three days of spend, so Bot::Fundable spends the (user, EUR) budget on the first bot.
fn low_funds() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "1", "0") }

/// A Kraken install with `n` weekly bots of one user, all due at 2026-09-01 10:00:00.
fn engine(n: usize, v: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, Vec<i64>, seed::Seeded) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-09-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let ids = (0..n).map(|_| seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).collect();
    (dir, Engine::new(o.primary, FakeFactory(v), seed::cipher(), lock), ids, s)
}

#[tokio::test(flavor = "current_thread")]
async fn each_recorded_row_and_each_spent_funds_budget_is_announced_once_after_its_tick() {
    let (_d, mut e, bots, s) = engine(2, low_funds());
    let mut rx = e.subscribe();
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let r = rows(&e.primary);
    assert_eq!(r.len(), 2);
    assert_eq!(drained(&mut rx), vec![
        EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: r[0].1 },
        EngineEvent::FundsLow { bot_id: bots[0], user_id: s.user_id, quote_asset_id: Some(s.quote) },
        EngineEvent::OrderRecorded { bot_id: bots[1], transaction_id: r[1].1 },
    ], "the second bot finds the budget of the same user and quote spent, as Rails does");
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // follow-up polls only: no row, no stamp
    assert_eq!(drained(&mut rx), vec![]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_tick_failing_for_funds_announces_its_failed_row_and_the_spent_budget() {
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0")
        .next_add(AddOutcome::Reject(vec!["EOrder:Insufficient funds".into()]));
    let (_d, mut e, bots, s) = engine(1, v);
    let mut rx = e.subscribe();
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let r = rows(&e.primary);
    assert_eq!(r[0].2, 1, "a failed row, as Rails writes for a definitive rejection");
    assert_eq!(drained(&mut rx), vec![
        EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: r[0].1 },
        EngineEvent::FundsLow { bot_id: bots[0], user_id: s.user_id, quote_asset_id: Some(s.quote) },
    ]);
}

#[tokio::test(flavor = "current_thread")]
async fn every_subscriber_hears_every_event() {
    let (_d, mut e, bots, _s) = engine(1, low_funds());
    let (mut a, mut b) = (e.subscribe(), e.subscribe());
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let heard = drained(&mut a);
    assert!(matches!(heard.first(), Some(EngineEvent::OrderRecorded { bot_id, .. }) if *bot_id == bots[0]));
    assert_eq!(drained(&mut b), heard);
}

/// Kraken's answer for the fake's first accepted order, filled.
fn filled() -> serde_json::Value {
    serde_json::json!({ "status": "closed", "price": "50000", "vol": "60", "vol_exec": "0.0012", "cost": "60", "oflags": "viqc",
                        "descr": { "type": "buy", "ordertype": "market", "price": "0" } })
}

#[tokio::test(flavor = "current_thread")]
async fn a_fill_the_follow_up_poll_writes_is_announced_once() {
    let (_d, mut e, bots, _s) = engine(1, low_funds().order("OFAKE-1", filled()));
    let mut rx = e.subscribe_to(EngineEvent::is_order);
    let mut mail = e.subscribe(); // as mail and the scheduler subscribe
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let tx = rows(&e.primary)[0].1;
    assert!(drained(&mut rx).contains(&EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: tx }));
    assert!(drained(&mut mail).contains(&EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: tx }));
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // the follow-up poll finds the fill
    assert_eq!(drained(&mut rx), vec![EngineEvent::OrderUpdated { bot_id: bots[0] }]);
    assert_eq!(drained(&mut mail), vec![], "an order update reached a subscriber that did not ask for it");
    run::step(&mut e, &at("2026-09-01T10:01:05.5Z")).await.unwrap(); // nothing waits on the venue any more
    assert_eq!(drained(&mut rx), vec![]);
}

#[tokio::test(flavor = "current_thread")]
async fn every_tick_and_every_poll_announce_their_bot_once() {
    let open = serde_json::json!({ "status": "open", "price": "0", "vol": "60", "vol_exec": "0", "cost": "0", "oflags": "viqc",
                                   "descr": { "type": "buy", "ordertype": "market", "price": "0" } });
    let answer = |order: serde_json::Value| serde_json::json!({ "error": [], "result": { "OFAKE-1": order } });
    // QueryOrders answers the follow-up poll, the next tick's sweep, the second order's poll, and then every query the same.
    let v = FakeVenue::from_script(&serde_json::json!({ "http": { "/0/private/QueryOrders": [answer(open.clone()), answer(open.clone()), answer(open), answer(filled())] } }))
        .ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "1", "0");
    let (_d, mut e, bots, _s) = engine(1, v);
    let mut rx = e.subscribe_to(EngineEvent::is_order);
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let tx = rows(&e.primary)[0].1;
    drained(&mut rx);
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // the poll writes `open` over `unknown`
    assert_eq!(drained(&mut rx), vec![EngineEvent::OrderUpdated { bot_id: bots[0] }]);
    let updates = |heard: Vec<EngineEvent>| heard.into_iter().filter(|event| matches!(event, EngineEvent::OrderUpdated { .. })).collect::<Vec<_>>();
    // The sweep finds it as it was, and a second order is placed: the tick is announced all the same, and so is the
    // second order's poll, with nothing read to decide.
    run::step(&mut e, &at("2026-09-08T10:00:00.5Z")).await.unwrap();
    assert_eq!(updates(drained(&mut rx)), vec![EngineEvent::OrderUpdated { bot_id: bots[0] }; 2], "one overdue poll and one tick in this pass");
    run::step(&mut e, &at("2026-09-08T10:00:05.5Z")).await.unwrap();
    assert_eq!(updates(drained(&mut rx)), vec![EngineEvent::OrderUpdated { bot_id: bots[0] }], "the second order's poll");
    run::step(&mut e, &at("2026-09-15T10:00:00.5Z")).await.unwrap(); // the sweep before the third order finds the first filled
    assert_eq!(updates(drained(&mut rx)), vec![EngineEvent::OrderUpdated { bot_id: bots[0] }; 2], "one overdue poll and one tick, however many rows they wrote");
    let filled: i64 = e.primary.query_row("SELECT external_status FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    assert_eq!(filled, 2);
}

/// A tick that fails after its sweep committed a fill still announces the fill: the order has left the waiting set, and
/// no later sweep would find it again. Here the fill is committed by the sweep, then the tick's next order and its failure
/// handling are refused by triggers armed at the price read that follows the sweep, so the tick ends in an error.
#[tokio::test(flavor = "current_thread")]
async fn a_fill_committed_before_the_tick_fails_is_announced() {
    use std::{cell::RefCell, rc::Rc};
    let open = serde_json::json!({ "status": "open", "price": "0", "vol": "60", "vol_exec": "0", "cost": "0", "oflags": "viqc",
                                   "descr": { "type": "buy", "ordertype": "market", "price": "0" } });
    let answer = |order: serde_json::Value| serde_json::json!({ "error": [], "result": { "OFAKE-1": order } });
    let database: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::default();
    let armed = database.clone();
    // QueryOrders answers `open` to the follow-up poll and the fill to the next tick's sweep (and every query after it).
    let v = FakeVenue::from_script(&serde_json::json!({ "http": { "/0/private/QueryOrders": [answer(open), answer(filled())] } }))
        .ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "1", "0")
        .on_price(move || {
            let Some(path) = armed.borrow().clone() else { return };
            let c = Connection::open(path).unwrap();
            if c.query_row("SELECT count(*) FROM transactions WHERE external_status = 2", [], |r| r.get::<_, i64>(0)).unwrap() > 0 {
                c.execute_batch("CREATE TRIGGER IF NOT EXISTS refuse_order BEFORE INSERT ON transactions BEGIN SELECT RAISE(ABORT, 'insert refused'); END;
                                 CREATE TRIGGER IF NOT EXISTS refuse_status BEFORE UPDATE OF status ON bots BEGIN SELECT RAISE(ABORT, 'status write refused'); END;").unwrap();
            }
        });
    let (d, mut e, bots, _s) = engine(1, v);
    *database.borrow_mut() = Some(d.path().join("production.sqlite3"));
    let mut rx = e.subscribe_to(EngineEvent::is_order);
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let tx = rows(&e.primary)[0].1;
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // the follow-up poll writes `open`
    drained(&mut rx);
    let _ = run::step(&mut e, &at("2026-09-08T10:00:00.5Z")).await; // the engine survives a failed tick
    let armed: i64 = e.primary.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'refuse_%'", [], |r| r.get(0)).unwrap();
    assert_eq!(armed, 2, "the tick went past its sweep to a price read");
    assert_eq!(rows(&e.primary).len(), 1, "no second order was recorded");
    let filled: i64 = e.primary.query_row("SELECT external_status FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    assert_eq!(filled, 2, "the sweep committed the fill before the tick failed");
    assert!(drained(&mut rx).contains(&EngineEvent::OrderUpdated { bot_id: bots[0] }), "the committed fill was not announced");
}

/// A tick whose placement committed an order and then failed (its funds write and its failure handling are refused by
/// triggers armed while AddOrder awaits its reply) still announces the order and schedules its follow-up poll, which
/// records the fill on time: the order is in no later sweep's waiting set.
#[tokio::test(flavor = "current_thread")]
async fn an_order_placed_before_the_tick_fails_is_announced_and_polled() {
    use std::{cell::RefCell, rc::Rc};
    let database: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::default();
    let armed = database.clone();
    let v = low_funds().order("OFAKE-1", filled()).on_add(move || {
        let Some(path) = armed.borrow().clone() else { return };
        Connection::open(path).unwrap().execute_batch(
            "CREATE TRIGGER refuse_funds BEFORE UPDATE OF last_end_of_funds_notification ON bots BEGIN SELECT RAISE(ABORT, 'funds write refused'); END;
             CREATE TRIGGER refuse_status BEFORE UPDATE OF status ON bots BEGIN SELECT RAISE(ABORT, 'status write refused'); END;").unwrap();
    });
    let (d, mut e, bots, _s) = engine(1, v);
    *database.borrow_mut() = Some(d.path().join("production.sqlite3"));
    let mut rx = e.subscribe_to(EngineEvent::is_order);
    let _ = run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await; // the engine survives a failed tick
    let placed = rows(&e.primary);
    assert_eq!(placed.len(), 1, "the placement committed its order");
    let tx = placed[0].1;
    let funds: Option<String> = e.primary.query_row("SELECT last_end_of_funds_notification FROM bots WHERE id = ?1", [bots[0]], |r| r.get(0)).unwrap();
    assert_eq!(funds, None, "the tick failed at its funds write, after the placement");
    let heard = drained(&mut rx);
    assert!(heard.contains(&EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: tx }), "the placed order was not announced: {heard:?}");
    assert!(heard.contains(&EngineEvent::OrderUpdated { bot_id: bots[0] }), "the failed tick was not announced: {heard:?}");
    e.primary.execute_batch("DROP TRIGGER refuse_funds; DROP TRIGGER refuse_status;").unwrap();
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // its follow-up poll, on time
    let filled: i64 = e.primary.query_row("SELECT external_status FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    assert_eq!(filled, 2, "the follow-up poll recorded the fill");
    assert_eq!(drained(&mut rx), vec![EngineEvent::OrderUpdated { bot_id: bots[0] }]);
}

/// A sweep that writes 10,000 orders of one bot (each found abandoned: older than the stale window, unknown to the venue)
/// enqueues one OrderUpdated, the tick's, not 10,000: the queue's size when the step returns, before anything reads it, is
/// its peak.
#[tokio::test(flavor = "current_thread")]
async fn a_sweep_that_writes_ten_thousand_orders_enqueues_one_update() {
    let (_d, mut e, bots, _s) = engine(1, low_funds());
    let mut rx = e.subscribe_to(EngineEvent::is_order);
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // the placed order's follow-up poll
    let placed = rows(&e.primary)[0].1;
    e.primary.execute_batch(&format!("CREATE TEMP TABLE old AS SELECT * FROM transactions WHERE id = {placed};
        UPDATE old SET id = NULL, external_id = NULL, status = 0, external_status = 0, created_at = '2026-08-01' || substr(created_at, 11);")).unwrap();
    e.primary.execute_batch(&format!("BEGIN; {} COMMIT;", "INSERT INTO transactions SELECT * FROM old;".repeat(10_000))).unwrap();
    drained(&mut rx);
    run::step(&mut e, &at("2026-09-08T10:00:00.5Z")).await.unwrap(); // its sweep finds all 10,000 abandoned
    let abandoned: i64 = e.primary.query_row("SELECT count(*) FROM transactions WHERE external_status = 4", [], |r| r.get(0)).unwrap();
    assert_eq!(abandoned, 10_000, "the sweep wrote every old order");
    let peak = rx.len();
    let heard = drained(&mut rx);
    assert_eq!(peak, heard.len());
    let updates = heard.iter().filter(|event| matches!(event, EngineEvent::OrderUpdated { .. })).count();
    assert_eq!(updates, 1, "one update for the bot, not one per order");
    assert!(peak <= 2, "the queue's peak was {peak} events: {heard:?}");
    let _ = bots;
}

/// Sustained order updates reach only the subscriber that asked for them: the queues of mail and the scheduler (which
/// subscribe as before) hold none of them, and still hear every kind they heard.
#[test]
fn order_updates_are_queued_only_for_subscribers_that_ask_for_them() {
    use deltabadger::engine::events::EngineEvents;
    let mut events = EngineEvents::default();
    let (mut mail, mut scheduler) = (events.subscribe(), events.subscribe());
    let mut figures = events.subscribe_to(EngineEvent::is_order);
    for n in 0..100_000 { events.send(EngineEvent::OrderUpdated { bot_id: n }); }
    events.send(EngineEvent::OrderRecorded { bot_id: 1, transaction_id: 7 });
    events.send(EngineEvent::FundsLow { bot_id: 1, user_id: 1, quote_asset_id: None });
    for rx in [&mut mail, &mut scheduler] {
        assert_eq!(drained(rx), vec![EngineEvent::OrderRecorded { bot_id: 1, transaction_id: 7 }, EngineEvent::FundsLow { bot_id: 1, user_id: 1, quote_asset_id: None }]);
    }
    let heard = drained(&mut figures);
    assert_eq!(heard.len(), 100_001, "the figures service hears every order event");
    assert!(heard.iter().all(EngineEvent::is_order));
}
