//! The ⇄ reversal (plan 6a T7, Revision r4): the web's one transaction, the engine's cancels, the placement barrier and the
//! exactly-once reserve settlement.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{model, placement, reversal, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{FakeFactory, FakeVenue};
use deltabadger::web::bot::write::{self, Reversal};
use serde_json::{json, Value};

fn at(s: &str) -> FixedClock { FixedClock(t(s)) }
fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn engine(spec: BotSpec, venue: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, i64, seed::Seeded) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-09-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &spec);
    (dir, Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock), id, s)
}
fn priced() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0").balance_body("XXBT", "1", "0") }
/// A weekly 60 EUR buyer that ticked at its first checkpoint, with that buy still open.
fn buyer() -> BotSpec { BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_action_job_at", json!("2026-09-01T10:00:01.000Z")) }
fn open_buy(c: &rusqlite::Connection, s: &seed::Seeded, bot: i64, ext: &str) -> i64 {
    seed::insert_tx(c, s, bot, &TxSpec { status: 0, external_status: Some(1), external_id: Some(ext.into()), order_type: 1, amount: Some("0.0012"),
        quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() })
}
fn reverse(c: &rusqlite::Connection, s: &seed::Seeded, id: i64, now: &str) -> Reversal { write::reverse(c, &seed::cipher(), s.user_id, id, t(now)).unwrap() }
fn bot(c: &rusqlite::Connection, id: i64) -> model::Bot { model::load_bot(c, id).unwrap() }
fn kraken_cancelled(exec: &str, cost: &str) -> Value {
    json!({ "status": "canceled", "price": "50000", "vol": "0.0012", "vol_exec": exec, "cost": cost, "oflags": "", "descr": { "type": "buy", "ordertype": "limit", "price": "50000", "pair": "XBTEUR" } })
}

#[tokio::test(flavor = "current_thread")]
async fn three_states_and_no_trade_right_after_the_flip() {
    let v = priced();
    let (_d, mut e, id, s) = engine(buyer().with("sell_amount", json!(0.001)), v.clone());
    let generation = bot(&e.primary, id).run_generation();
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"), Reversal::Reversed));
    let b = bot(&e.primary, id);
    assert_eq!((b.settings["direction"].as_str(), b.settings["sell_denomination"].as_str()), (Some("selling"), Some("base")));
    assert_eq!(b.run_generation(), generation + 1, "an in-flight tick's writes are fenced");
    assert_eq!(b.status, BotStatus::Scheduled);
    assert_eq!(b.started_at_us().unwrap(), Some(t("2026-09-03T12:00:00Z").timestamp_micros()));
    assert!(b.rust_defer_until_us().unwrap() > Some(t("2026-09-03T12:00:00Z").timestamp_micros()), "it waits for its next checkpoint: {:?}", b.transient);
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap();
    assert!(v.sent().is_empty(), "re-anchored at now, the bot would otherwise sell at once");
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:02Z"), Reversal::Reversed));
    assert_eq!(bot(&e.primary, id).settings["sell_denomination"], "quote");
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:03Z"), Reversal::Reversed));
    let b = bot(&e.primary, id);
    assert_eq!((b.settings["direction"].as_str(), b.settings["sell_denomination"].as_str()), (Some("buying"), Some("base")));
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancelled_buy_returns_its_unspent_reserve_once() {
    let v = priced();
    let (_d, mut e, id, s) = engine(buyer(), v.clone());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"), Reversal::Reversed));
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![tx], vec![tx]), "registered in Rails' own key, its cancel left for the engine");
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "0.0", "the waiting buy counts at its reserve when captured");
    let v = v.order("OBUY", kraken_cancelled("0.0004", "20"));
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap();
    assert_eq!(v.calls("/0/private/CancelOrder"), 1);
    assert_eq!(reversal::work(&bot(&e.primary, id)).1, Vec::<i64>::new(), "the cancel outcome is recorded");
    run::step(&mut e, &at("2026-09-03T12:00:07Z")).await.unwrap(); // the follow-up poll reads it cancelled, 20 EUR filled
    let b = bot(&e.primary, id);
    assert_eq!(b.transient["missed_quote_amount"], "40.0", "60 reserved − 20 filled returns to the carry");
    assert!(b.transient.get("flip_reserve_pending").is_none());
    let tx_conn = &e.primary;
    assert!(!model::locked(tx_conn, |c| reversal::settle(c, id, tx)).unwrap(), "a second settle credits nothing");
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "40.0");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_cancel_is_logged_and_restores_nothing() {
    let v = priced().order("OBUY", kraken_cancelled("0", "0"));
    let (_d, mut e, id, s) = engine(buyer(), v.clone());
    open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    push_cancel(&v, json!({ "error": ["EOrder:Unknown order"] }));
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap();
    let logged: i64 = e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'order_cancel_failed' AND level = 1", [id], |r| r.get(0)).unwrap();
    assert_eq!(logged, 1);
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![], vec![]), "a buy whose cancel failed may still be live: never registered");
    run::step(&mut e, &at("2026-09-03T12:10:00Z")).await.unwrap();
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "0.0");
}
fn push_cancel(v: &FakeVenue, body: Value) { v.script("/0/private/CancelOrder", body); }

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_repeated_after_a_crash_reads_the_order_before_calling_it_failed() {
    let v = priced().order("OBUY", kraken_cancelled("0", "0"));
    let (_d, mut e, id, s) = engine(buyer(), v.clone());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    // The first attempt reached Kraken and the process died before recording it.
    e.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_reversal_cancels[0].sent', json('true')) WHERE id = ?1", [id]).unwrap();
    push_cancel(&v, json!({ "error": ["EOrder:Unknown order"] }));
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap();
    let logged: i64 = e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event = 'order_cancel_failed'", [], |r| r.get(0)).unwrap();
    assert_eq!(logged, 0, "Kraken reports it cancelled: the first attempt did it");
    run::step(&mut e, &at("2026-09-03T12:00:07Z")).await.unwrap();
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "60.0", "nothing filled: the whole reserve returns");
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![], vec![]));
    let _ = tx;
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_reversals_register_a_reserve_once() {
    let (_d, e, id, s) = engine(buyer(), priced());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"); // buying → selling N base
    reverse(&e.primary, &s, id, "2026-09-03T12:00:01Z"); // → selling for N quote
    reverse(&e.primary, &s, id, "2026-09-03T12:00:02Z"); // → buying
    reverse(&e.primary, &s, id, "2026-09-03T12:00:03Z"); // → selling again, the buy still waiting
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![tx], vec![tx]));
}

#[tokio::test(flavor = "current_thread")]
async fn a_tick_or_an_unresolved_order_defers_the_reversal() {
    let (_d, e, id, s) = engine(buyer(), priced());
    let before: String = e.primary.query_row("SELECT settings || transient_data || status FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    e.primary.execute("UPDATE bots SET status = 4 WHERE id = ?1", [id]).unwrap();
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"), Reversal::InProgress), "a tick holds the bot");
    e.primary.execute("UPDATE bots SET status = 1, transient_data = json_set(transient_data, '$.rust_placement', json('{\"cl_ord_id\":\"x\"}')) WHERE id = ?1", [id]).unwrap();
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"), Reversal::InProgress), "an unresolved order");
    e.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement') WHERE id = ?1", [id]).unwrap();
    let after: String = e.primary.query_row("SELECT settings || transient_data || status FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(before, after, "nothing changed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_tick_sized_before_the_reversal_is_fenced_and_the_barrier_holds_placement() {
    let (_d, e, id, s) = engine(buyer(), priced());
    open_buy(&e.primary, &s, id, "OBUY");
    let sized = bot(&e.primary, id);
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    assert!(model::require_run(&e.primary, &sized).is_err(), "the bumped generation fences a tick that claimed the old run");
    let d = |x: &str| deltabadger::ruby::BigDec::parse(x).unwrap();
    let plan = deltabadger::engine::amount::OrderPlan { ticker: model::ticker_for(&e.primary, &sized).unwrap().unwrap(), side: deltabadger::venue::Side::Buy, limit: false,
        price: d("50000"), amount: d("0.0012"), quote_amount: d("60"), quote_type: true, volume: d("60"), kind: deltabadger::engine::amount::TxKind::Regular };
    assert!(placement::begin(&e.primary, &bot(&e.primary, id), &plan, &at("2026-09-03T12:00:00Z")).is_err(), "no intent while a captured cancel is unrecorded");
    assert!(bot(&e.primary, id).rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn the_reserve_settles_after_a_stop_and_after_a_restart_whose_polls_ran_out() {
    let v = priced();
    let (dir, mut e, id, s) = engine(buyer(), v.clone());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    e.primary.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); // STOP
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap(); // the cancel still goes out
    assert_eq!(v.calls("/0/private/CancelOrder"), 1);
    // The follow-up chain ran out while the venue still said open; later the row is final (a sweep), and the engine restarts.
    e.primary.execute("UPDATE transactions SET external_status = 3, quote_amount_exec = 15, amount_exec = 0.0003 WHERE id = ?1", [tx]).unwrap();
    drop(e);
    let p = Paths::from_env(&|_| None, dir.path());
    let mut e = Engine::new(store::open(&p).unwrap().primary, FakeFactory(v), seed::cipher(), lease::lock(&p, t("2026-09-01T00:00:00Z")).unwrap());
    run::step(&mut e, &at("2026-09-03T13:00:00Z")).await.unwrap();
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "45.0", "the startup scan settles it");
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![], vec![]));
}

#[tokio::test(flavor = "current_thread")]
async fn handback_waits_for_unrecorded_cancels() {
    let (dir, e, id, s) = engine(buyer(), priced());
    open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    drop(e);
    let p = Paths::from_env(&|_| None, dir.path());
    let o = store::open(&p).unwrap();
    let lock = lease::lock(&p, t("2026-09-01T00:00:00Z")).unwrap();
    let r = deltabadger::engine::handover::hand_back(&lock, &o, &FakeFactory(priced()), &seed::cipher(), &at("2026-09-03T12:00:01Z")).await;
    assert!(matches!(r, Err(deltabadger::engine::EngineError::Unresolved(ref ids)) if ids == &vec![id]), "{r:?}");
}

/// A reserve still waiting for its buy's final state is Rust's to settle: a Rails without its reader would never credit it,
/// so handback waits for it even after the cancel is recorded.
#[tokio::test(flavor = "current_thread")]
async fn handback_waits_for_a_reserve_still_waiting_for_its_buy() {
    let (dir, mut e, id, s) = engine(buyer(), priced());
    open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap(); // the cancel goes out and is recorded; the buy is not final
    assert_eq!(reversal::work(&bot(&e.primary, id)).1, Vec::<i64>::new(), "no cancel left unrecorded");
    drop(e);
    let p = Paths::from_env(&|_| None, dir.path());
    let o = store::open(&p).unwrap();
    let lock = lease::lock(&p, t("2026-09-01T00:00:00Z")).unwrap();
    let r = deltabadger::engine::handover::hand_back(&lock, &o, &FakeFactory(priced()), &seed::cipher(), &at("2026-09-03T12:00:02Z")).await;
    assert!(matches!(r, Err(deltabadger::engine::EngineError::Unresolved(ref ids)) if ids == &vec![id]), "{r:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_selling_bots_frozen_carry_survives_a_settings_edit_and_the_reversal_back_to_buying() {
    let spec = buyer().with("direction", json!("selling")).with("sell_denomination", json!("quote")).with("sell_quote_amount", json!(30))
        .transient("missed_quote_amount", json!("5.0"));
    let (_d, e, id, s) = engine(spec, priced());
    // A settings edit while selling (the sell sentence), which moves the carry window.
    e.primary.execute("UPDATE bots SET settings = json_set(settings, '$.sell_quote_amount', 40), settings_changed_at = '2026-09-03 11:00:00' WHERE id = ?1", [id]).unwrap();
    assert!(matches!(reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"), Reversal::Reversed));
    let b = bot(&e.primary, id);
    assert_eq!(b.settings["direction"], "buying");
    assert_eq!(b.transient["missed_quote_amount"], "5.0", "the frozen buy carry, not two days of buying");
}

#[tokio::test(flavor = "current_thread")]
async fn a_due_bot_held_by_the_barrier_trades_on_the_next_pass_once_its_cancels_are_recorded() {
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("XBT", "1", "0");
    let (_d, mut e, id, s) = engine(buyer().with("sell_amount", json!(0.001)), v.clone());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z"); // → selling 0.001 BTC weekly, next checkpoint 2026-09-10 12:00
    // A pass whose cancel work cannot be recorded (an entry this build cannot read ahead of the real one).
    let real = bot(&e.primary, id).transient[reversal::CANCELS].clone();
    let blocked = json!([{ "id": "unreadable" }, real[0].clone()]);
    let set = |c: &rusqlite::Connection, value: &Value| c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_reversal_cancels', json(?1)) WHERE id = ?2",
        rusqlite::params![value.to_string(), id]).unwrap();
    set(&e.primary, &blocked);
    run::step(&mut e, &at("2026-09-10T12:00:01Z")).await.unwrap();
    assert!(v.sent().is_empty(), "the barrier holds the due sell");
    assert_eq!(reversal::work(&bot(&e.primary, id)).1, vec![tx]);
    // The next pass records the cancel, and the bot sells then, not a week later.
    set(&e.primary, &real);
    run::step(&mut e, &at("2026-09-10T12:01:00Z")).await.unwrap();
    assert_eq!(reversal::work(&bot(&e.primary, id)).1, Vec::<i64>::new());
    assert_eq!(v.sent().len(), 1, "the sell runs on the pass after the cancels are recorded");
    assert_eq!(v.sent()[0].side, deltabadger::venue::Side::Sell);
}

#[tokio::test(flavor = "current_thread")]
async fn a_final_buy_is_not_settled_before_its_cancel_outcome_is_recorded() {
    let v = priced();
    let (_d, mut e, id, s) = engine(buyer(), v.clone());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    reverse(&e.primary, &s, id, "2026-09-03T12:00:00Z");
    // The row reads final (cancelled, 20 EUR filled) while this pass cannot record the cancel: the startup scan must wait.
    e.primary.execute("UPDATE transactions SET external_status = 3, quote_amount_exec = 20, amount_exec = 0.0004 WHERE id = ?1", [tx]).unwrap();
    let real = bot(&e.primary, id).transient[reversal::CANCELS].clone();
    let set = |c: &rusqlite::Connection, value: &Value| c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_reversal_cancels', json(?1)) WHERE id = ?2",
        rusqlite::params![value.to_string(), id]).unwrap();
    set(&e.primary, &json!([{ "id": "unreadable" }, real[0].clone()]));
    run::step(&mut e, &at("2026-09-03T12:00:01Z")).await.unwrap();
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "0.0", "nothing settles before the cancel outcome");
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![tx], vec![tx]));
    // The cancel then fails: the buy may still be live, so nothing ever returns for it.
    set(&e.primary, &real);
    push_cancel(&v, json!({ "error": ["EOrder:Unknown order"] }));
    run::step(&mut e, &at("2026-09-03T12:00:02Z")).await.unwrap();
    run::step(&mut e, &at("2026-09-03T12:20:00Z")).await.unwrap();
    assert_eq!(bot(&e.primary, id).transient["missed_quote_amount"], "0.0");
    assert_eq!(reversal::work(&bot(&e.primary, id)), (vec![], vec![]));
}

#[tokio::test(flavor = "current_thread")]
async fn a_buy_whose_cancel_is_recorded_is_not_registered_again_by_later_reversals() {
    let (_d, mut e, id, s) = engine(buyer(), priced());
    let tx = open_buy(&e.primary, &s, id, "OBUY");
    // Each reversal captures the still-waiting buy again; each pass records that cancel before the next reversal.
    for (i, now) in ["2026-09-03T12:00:00Z", "2026-09-03T12:00:02Z", "2026-09-03T12:00:04Z", "2026-09-03T12:00:06Z"].iter().enumerate() {
        reverse(&e.primary, &s, id, now);
        if i < 3 { run::step(&mut e, &at(&format!("2026-09-03T12:00:0{}Z", 2 * i + 1))).await.unwrap(); }
    }
    assert_eq!(bot(&e.primary, id).settings["direction"], "selling");
    assert_eq!(reversal::work(&bot(&e.primary, id)).0, vec![tx], "registered once across two buy → sell reversals");
}
