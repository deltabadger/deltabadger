//! Plan 8a: a signal bot's webhook calls through the production path (run::step with the fake Kraken and a fake clock),
//! with the test seam that admits signal bots until slice 8d does: one order per call, the call consumed with its intent,
//! the barrier, the marker this engine raises and only its own recovery lifts, the crash windows, the expiry.
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::signal::{self, Step};
use deltabadger::engine::{model, placement, FixedClock};
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{AddOutcome, FakeFactory, FakeVenue};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

const T0: &str = "2026-09-01T14:00:01Z";
fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn at(s: &str) -> FixedClock { FixedClock(t(s)) }
fn plus(s: &str, secs: f64) -> FixedClock { FixedClock(t(s) + chrono::Duration::microseconds((secs * 1e6) as i64)) }

struct Setup { _dir: tempfile::TempDir, e: Engine<FakeFactory>, bot: i64, rule: i64, venue: FakeVenue }

/// A started Bots::Signal on Kraken (BTC/EUR) with one rule, as BotApi::Bots::CreateSignal leaves it, under an engine that
/// admits it.
fn setup(direction: i64, percentage: bool, amount: &str, venue: FakeVenue) -> Setup {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, t("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
                       VALUES ('Bots::Signal', 1, ?1, ?2, ?3, '{}', '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
                      params![s.exchange_id, s.user_id, json!({ "base_asset_id": s.btc, "quote_asset_id": s.quote }).to_string()]).unwrap();
    let bot = o.primary.last_insert_rowid();
    o.primary.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, created_at, updated_at) \
                       VALUES (?1, ?2, ?3, ?4, 1, 'tok-1', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
                      params![bot, direction, i64::from(percentage), amount]).unwrap();
    let rule = o.primary.last_insert_rowid();
    let mut e = Engine::new(o.primary, FakeFactory(venue.clone()), seed::cipher(), lock);
    e.admit_signal_bots_for_tests();
    Setup { _dir: dir, e, bot, rule, venue }
}

fn priced() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3").balance_body("ZEUR", "10000", "0") }
fn buy() -> Setup { setup(0, false, "100", priced()) }

fn call(c: &Connection, bot: i64, rule: i64, n: usize, triggered: &str) {
    model::append_signal_call(c, bot, &model::SignalCall { source: format!("hook:{n}"), signal_id: rule, triggered_at: t(triggered) }).unwrap();
}
fn calls(c: &Connection, bot: i64) -> usize { model::load_bot(c, bot).unwrap().signal_calls().unwrap().len() }
fn transient(c: &Connection, bot: i64) -> Value { model::load_bot(c, bot).unwrap().transient }
fn rows(c: &Connection) -> Vec<(i64, Option<String>, String, f64)> {
    let mut s = c.prepare("SELECT status, external_id, bot_interval, bot_quote_amount FROM transactions ORDER BY id").unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}
fn lines(c: &Connection) -> Vec<(String, Value)> {
    let mut s = c.prepare("SELECT event, details FROM bot_activity_logs ORDER BY id").unwrap();
    s.query_map([], |r| Ok((r.get::<_, String>(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap()))).unwrap().map(Result::unwrap).collect()
}
fn status(c: &Connection, bot: i64) -> i64 { c.query_row("SELECT status FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap() }

#[test]
fn a_call_expires_strictly_after_max_age() {
    let triggered = t(T0);
    assert!(!signal::expired(triggered, triggered + chrono::Duration::seconds(300)), "exactly 300 s old still runs");
    assert!(signal::expired(triggered, triggered + chrono::Duration::seconds(300) + chrono::Duration::microseconds(1)));
    let c = model::SignalCall { source: "hook:x".into(), signal_id: 1, triggered_at: triggered };
    assert!(signal::expired(triggered, signal::expires_at(&c)) && !signal::expired(triggered, signal::expires_at(&c) - chrono::Duration::microseconds(1)));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_call_places_one_order_consumed_with_its_intent_and_the_bot_stays_scheduled() {
    let mut s = buy();
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    let wake = run::step(&mut s.e, &at(T0)).await.unwrap();
    assert_eq!(s.venue.sent().len(), 1);
    assert_eq!((s.venue.sent()[0].volume.as_str(), s.venue.sent()[0].quote_volume), ("0.00199999", false), "Kraken buys 100 EUR in base at the ask");
    assert_eq!(calls(&s.e.primary, s.bot), 0);
    assert!(transient(&s.e.primary, s.bot).get("rust_placement").is_none());
    // Bot::OrderCreator#order_identity_values: the schedule's two columns keep their defaults.
    assert_eq!(rows(&s.e.primary), vec![(0, Some("OFAKE-1".into()), String::new(), 0.0)]);
    assert_eq!(status(&s.e.primary, s.bot), 1, "never executing or retrying");
    assert!(wake <= plus(T0, 5.0).0.timestamp_micros(), "the order is polled 5 s later");
    // FetchAndUpdateOrderJob's OrderStillOpen: an order the venue still holds open is asked again.
    run::step(&mut s.e, &plus(T0, 5.0)).await.unwrap();
    let asked = s.venue.calls("/0/private/QueryOrders") + s.venue.calls("/0/private/ClosedOrders");
    assert!(asked >= 1, "the follow-up asked the venue");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_follow_up_poll_fills_the_row_and_touches_no_carry_and_no_limit() {
    let v = priced().order("OTX-1", json!({ "status": "closed", "price": "50000.2", "vol": "0.00199999", "vol_exec": "0.00199999", "cost": "99.99", "oflags": "",
                                             "descr": { "ordertype": "market", "price": "0", "type": "buy" } }))
                    .next_add(AddOutcome::Accept("OTX-1".into()));
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    let before: (String, String) = s.e.primary.query_row("SELECT settings, transient_data FROM bots WHERE id = ?1", [s.bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    run::step(&mut s.e, &plus(T0, 5.0)).await.unwrap();
    let (ext, exec): (i64, f64) = s.e.primary.query_row("SELECT external_status, amount_exec FROM transactions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(ext == 2 && exec > 0.0, "closed, with its fill: {ext} {exec}");
    let after: (String, String) = s.e.primary.query_row("SELECT settings, transient_data FROM bots WHERE id = ?1", [s.bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(before, after, "a signal bot's fill writes no carry, no amount-limit stop and no mail marker");
    assert_eq!(status(&s.e.primary, s.bot), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn two_calls_run_one_per_pass_oldest_first() {
    let mut s = buy();
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    call(&s.e.primary, s.bot, s.rule, 2, "2026-09-01T14:00:31Z");
    let wake = run::step(&mut s.e, &at("2026-09-01T14:00:32Z")).await.unwrap();
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (1, 1), "one call per bot per pass");
    assert!(wake <= plus("2026-09-01T14:00:32Z", 0.001).0.timestamp_micros(), "the next call runs at once");
    run::step(&mut s.e, &at("2026-09-01T14:00:32Z")).await.unwrap();
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (2, 0), "two calls, two orders, never one order twice");
    assert_eq!(rows(&s.e.primary).len(), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_ambiguous_send_raises_the_marker_and_the_next_call_waits_then_expires() {
    let v = priced().next_add(AddOutcome::AmbiguousNotPlaced("Net::ReadTimeout".into())).lookup_fails(1_000);
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    let td = transient(&s.e.primary, s.bot);
    let intent = td["rust_placement"]["cl_ord_id"].as_str().unwrap().to_string();
    assert_eq!(td["signal_placement_unresolved"], json!({ "id": intent, "at": "2026-09-01T14:00:01Z", "side": "buy", "error": "Net::ReadTimeout" }),
               "R8a's marker, its id the intent's cl_ord_id");
    assert_eq!(lines(&s.e.primary).iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>(), ["placement_ambiguous"]);
    assert!(rows(&s.e.primary).is_empty(), "no row: its fate is unknown");
    // A second call: the barrier holds it, and the loop wakes when it expires.
    call(&s.e.primary, s.bot, s.rule, 2, "2026-09-01T14:01:00Z");
    let wake = run::step(&mut s.e, &at("2026-09-01T14:01:00Z")).await.unwrap();
    assert!(wake <= t("2026-09-01T14:06:00Z").timestamp_micros() + 1, "woken no later than its expiry");
    run::step(&mut s.e, &at("2026-09-01T14:06:00Z")).await.unwrap();
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (1, 1), "exactly 300 s old: still waiting");
    run::step(&mut s.e, &plus("2026-09-01T14:06:00Z", 0.000001)).await.unwrap();
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (1, 0));
    assert_eq!(lines(&s.e.primary).last().unwrap(), &("signal_expired".to_string(), json!({ "signal_id": s.rule })));
    assert_eq!(transient(&s.e.primary, s.bot)["signal_placement_unresolved"]["id"], json!(intent), "the marker stays");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn ambiguous_then_recovered_writes_the_row_clears_the_marker_and_defers_nothing() {
    let v = priced().next_add(AddOutcome::AmbiguousPlaced("OLOST-1".into()));
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    assert!(transient(&s.e.primary, s.bot).get("signal_placement_unresolved").is_some());
    // The reconcile pass: a signal bot is working but never ticks, so its intent is looked up here.
    run::step(&mut s.e, &plus(T0, 31.0)).await.unwrap();
    let td = transient(&s.e.primary, s.bot);
    assert!(td.get("rust_placement").is_none() && td.get("signal_placement_unresolved").is_none() && td.get("rust_defer_until").is_none(), "{td}");
    assert_eq!(rows(&s.e.primary), vec![(0, Some("OLOST-1".into()), String::new(), 0.0)]);
    assert_eq!(s.venue.sent().len(), 1);
    // The bot places again.
    call(&s.e.primary, s.bot, s.rule, 2, "2026-09-01T14:01:00Z");
    run::step(&mut s.e, &at("2026-09-01T14:01:00Z")).await.unwrap();
    assert_eq!(s.venue.sent().len(), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn ambiguous_then_proven_absent_clears_its_own_marker_only() {
    let v = priced().next_add(AddOutcome::AmbiguousNotPlaced("Net::ReadTimeout".into()));
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    // Kraken drops an order after its deadline (10 s): absence is provable from 60 s after it.
    run::step(&mut s.e, &plus(T0, 71.0)).await.unwrap();
    let td = transient(&s.e.primary, s.bot);
    assert!(td.get("rust_placement").is_none() && td.get("signal_placement_unresolved").is_none() && td.get("rust_defer_until").is_none(), "{td}");
    assert_eq!(lines(&s.e.primary).last().unwrap().1["resolution"], json!("not_placed"));
    // A marker this engine did not raise (Rails', or a newer one) is never lifted by another order's recovery.
    let tx = model::immediate(&s.e.primary).unwrap();
    model::merge_transient_compact(&tx, s.bot, &[(signal::MARKER_KEY, json!({ "id": "rails-uuid", "at": "2026-09-01T13:00:00Z", "side": "buy", "error": "x" }))]).unwrap();
    signal::clear_marker(&tx, s.bot, "another-cl-ord-id").unwrap();
    tx.commit().unwrap();
    assert_eq!(transient(&s.e.primary, s.bot)["signal_placement_unresolved"]["id"], json!("rails-uuid"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_stop_between_sizing_and_the_claim_places_nothing_and_logs_signal_ignored() {
    let s = buy();
    let path = s._dir.path().join("production.sqlite3");
    let v = s.venue.clone().on_price(move || { Connection::open(&path).unwrap().execute("UPDATE bots SET status = 2", []).unwrap(); });
    let Setup { _dir, mut e, bot, rule, .. } = s;
    call(&e.primary, bot, rule, 1, T0);
    run::step(&mut e, &at(T0)).await.unwrap();
    assert!(v.sent().is_empty());
    assert_eq!((calls(&e.primary, bot), lines(&e.primary)), (0, vec![("signal_ignored".to_string(), json!({ "signal_id": rule }))]));
    assert!(rows(&e.primary).is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn credentials_replaced_mid_call_place_nothing_and_the_call_runs_again() {
    let s = buy();
    let path = s._dir.path().join("production.sqlite3");
    let cipher = seed::cipher();
    let rotated = std::cell::Cell::new(false);
    let v = s.venue.clone().on_price(move || {
        if rotated.replace(true) { return; }
        Connection::open(&path).unwrap().execute("UPDATE api_keys SET key = ?1", [cipher.encrypt("rotated-key")]).unwrap();
    });
    let Setup { _dir, mut e, bot, rule, .. } = s;
    call(&e.primary, bot, rule, 1, T0);
    run::step(&mut e, &at(T0)).await.unwrap();
    assert!(v.sent().is_empty(), "sized under the old key: nothing sent");
    assert_eq!(calls(&e.primary, bot), 1, "the call stays");
    run::step(&mut e, &plus(T0, 1.0)).await.unwrap();
    assert_eq!((v.sent().len(), calls(&e.primary, bot)), (1, 0), "placed once under the new key");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_placement_safe_rejection_is_placed_once_more_and_a_second_rejection_writes_the_row() {
    let safe = || AddOutcome::Reject(vec!["Timestamp for this request is outside of the recvWindow.".into()]);
    let v = priced().next_add(safe()).next_add(safe());
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    assert_eq!(s.venue.sent().len(), 2, "once more, never a third time");
    assert_ne!(s.venue.sent()[0].cl_ord_id, s.venue.sent()[1].cl_ord_id, "under a new intent");
    assert_eq!(rows(&s.e.primary).iter().map(|r| r.0).collect::<Vec<_>>(), [1]);
    assert!(transient(&s.e.primary, s.bot)["rust_error_mail_pending"]["signal"]["error"].as_str().unwrap().contains("recvWindow"), "the first failure mails");
    // A safe rejection then an acceptance: one order recorded.
    let v = priced().next_add(safe()).next_add(AddOutcome::Accept("OTX-2".into()));
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    assert_eq!((s.venue.sent().len(), rows(&s.e.primary)), (2, vec![(0, Some("OTX-2".into()), String::new(), 0.0)]));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_crash_before_the_intent_runs_the_call_again_and_places_once() {
    let v = priced().latency(std::time::Duration::from_secs(10));
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    // The process dies while the price is read: nothing was written.
    assert!(tokio::time::timeout(std::time::Duration::from_secs(1), run::step(&mut s.e, &at(T0))).await.is_err());
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (0, 1));
    run::step(&mut s.e, &plus(T0, 2.0)).await.unwrap();
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot), rows(&s.e.primary).len()), (1, 0, 1));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_crash_after_the_intent_commit_never_reruns_the_call_and_recovery_settles_it() {
    let s = buy();
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    // begin_signal committed (the call consumed with it), then the process died before the send.
    let bot = model::load_bot(&s.e.primary, s.bot).unwrap();
    let ticker = model::ticker_for(&s.e.primary, &bot).unwrap().unwrap();
    let price = deltabadger::ruby::BigDec::parse("50000.2").unwrap();
    let (plan, _) = deltabadger::engine::amount::size_order(&ticker, deltabadger::venue::Side::Buy, &price, &price.div(&price).unwrap(), &price,
        deltabadger::engine::venue_rules::KRAKEN.minimum_logic, deltabadger::engine::amount::TxKind::Regular).unwrap();
    let the_call = bot.signal_calls().unwrap().remove(0);
    let version = model::credential_version(&s.e.primary, &bot).unwrap();
    assert!(matches!(placement::begin_signal(&s.e.primary, &bot, &plan, Some(&the_call), &version, None, &at(T0)).unwrap(), placement::SignalClaim::Intent(_)));
    assert_eq!(calls(&s.e.primary, s.bot), 0, "consumed in the intent's transaction");
    // A restarted engine reconciles the intent (never sent: absent once provable) and places nothing.
    let Setup { _dir, e, venue, bot: id, .. } = s;
    let (primary, factory, cipher, lock) = (e.primary, e.factory, e.cipher, e.lock);
    let mut e = Engine::new(primary, factory, cipher, lock);
    e.admit_signal_bots_for_tests();
    run::step(&mut e, &at(T0)).await.unwrap();
    run::step(&mut e, &plus(T0, 71.0)).await.unwrap();
    assert!(venue.sent().is_empty());
    assert!(transient(&e.primary, id).get("rust_placement").is_none());
    assert!(rows(&e.primary).is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_crash_after_the_send_leaves_recovery_in_charge_and_sends_nothing_more() {
    let gate = std::rc::Rc::new(tokio::sync::Notify::new());
    let v = priced().hold_add(gate);
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    // AddOrder went out; the process dies before its reply.
    assert!(tokio::time::timeout(std::time::Duration::from_secs(1), run::step(&mut s.e, &at(T0))).await.is_err());
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (1, 0));
    let td = transient(&s.e.primary, s.bot);
    assert_eq!(td["rust_placement"]["dispatched"], json!(true), "the intent stays for recovery");
    let Setup { _dir, e, venue, bot: id, .. } = s;
    let (primary, cipher, lock) = (e.primary, e.cipher, e.lock);
    let mut e = Engine::new(primary, FakeFactory(FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3")), cipher, lock);
    e.admit_signal_bots_for_tests();
    run::step(&mut e, &plus(T0, 71.0)).await.unwrap();
    assert!(transient(&e.primary, id).get("rust_placement").is_none(), "settled by its recovery");
    assert_eq!(venue.sent().len(), 1, "nothing sent again");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_restart_with_an_open_intent_reconciles_it() {
    let v = priced().next_add(AddOutcome::AmbiguousPlaced("OLOST-9".into()));
    let mut s = setup(0, false, "100", v);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &at(T0)).await.unwrap();
    let Setup { _dir, e, venue, bot, .. } = s;
    let (primary, factory, cipher, lock) = (e.primary, e.factory, e.cipher, e.lock);
    let mut e = Engine::new(primary, factory, cipher, lock);
    e.admit_signal_bots_for_tests();
    run::step(&mut e, &plus(T0, 5.0)).await.unwrap();
    assert_eq!(rows(&e.primary), vec![(0, Some("OLOST-9".into()), String::new(), 0.0)]);
    assert!(transient(&e.primary, bot).get("signal_placement_unresolved").is_none());
    assert_eq!(venue.sent().len(), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_refusals_consume_the_call_with_their_line_and_a_removed_rule_with_none() {
    for (setup_sql, line) in [("UPDATE bots SET status = 2", Some("signal_ignored")), ("UPDATE bot_signals SET enabled = 0", Some("signal_ignored")),
                              ("UPDATE api_keys SET status = 3", Some("signal_api_key_pending")), ("DELETE FROM bot_signals", None)] {
        let mut s = buy();
        call(&s.e.primary, s.bot, s.rule, 1, T0);
        s.e.primary.execute_batch(setup_sql).unwrap();
        run::step(&mut s.e, &at(T0)).await.unwrap();
        assert!(s.venue.sent().is_empty(), "{setup_sql}");
        assert_eq!(calls(&s.e.primary, s.bot), 0, "{setup_sql}");
        let expected: Vec<(String, Value)> = line.map(|l| (l.to_string(), json!({ "signal_id": s.rule }))).into_iter().collect();
        assert_eq!(lines(&s.e.primary), expected, "{setup_sql}");
    }
    // Expired before it ran.
    let mut s = buy();
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    run::step(&mut s.e, &plus(T0, 300.000001)).await.unwrap();
    assert_eq!((s.venue.sent().len(), lines(&s.e.primary)), (0, vec![("signal_expired".to_string(), json!({ "signal_id": s.rule }))]));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn without_the_seam_the_install_still_refuses_a_working_signal_bot() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, t("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
                       VALUES ('Bots::Signal', 1, ?1, ?2, ?3, '{}', '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
                      params![s.exchange_id, s.user_id, json!({ "base_asset_id": s.btc, "quote_asset_id": s.quote }).to_string()]).unwrap();
    let mut e = Engine::new(o.primary, FakeFactory(priced()), seed::cipher(), lock);
    assert!(matches!(run::step(&mut e, &at(T0)).await, Err(deltabadger::engine::EngineError::Ineligible(_))), "admission is slice 8d's");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_next_reports_idle_waiting_and_ran() {
    let s = buy();
    let prices = deltabadger::engine::tick::PriceCache::default();
    let bot = model::load_bot(&s.e.primary, s.bot).unwrap();
    let cx = deltabadger::engine::tick::TickContext { credential_version: model::credential_version(&s.e.primary, &bot).unwrap(), prices: &prices,
        process_start: t(T0), stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    assert_eq!(signal::run_next(&s.e.primary, &s.venue, s.bot, &at(T0), &cx).await.unwrap(), Step::Idle);
    call(&s.e.primary, s.bot, s.rule, 1, T0);
    s.e.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.signal_placement_unresolved', json('{\"id\":\"m\"}'))", []).unwrap();
    assert_eq!(signal::run_next(&s.e.primary, &s.venue, s.bot, &at(T0), &cx).await.unwrap(), Step::Waiting(t(T0) + chrono::Duration::seconds(300) + chrono::Duration::microseconds(1)));
    s.e.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.signal_placement_unresolved')", []).unwrap();
    assert_eq!(signal::run_next(&s.e.primary, &s.venue, s.bot, &at(T0), &cx).await.unwrap(), Step::Ran);
}

/// Plan 8b: the hook's transaction (web::hook::receive, with the guard that 8d will pass) claims the rule as Active Record
/// writes it and leaves the engine the call with the claim's time; the engine places it once.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_webhook_call_the_hook_accepts_is_placed_once_by_the_engine() {
    use deltabadger::web::hook::{self, Outcome};
    let mut s = buy();
    let now = t("2026-09-01T14:00:01.123456789Z");
    let updated: String = s.e.primary.query_row("SELECT updated_at FROM bots WHERE id = ?1", [s.bot], |r| r.get(0)).unwrap();
    assert_eq!(hook::receive(&s.e.primary, "tok-1", now, |_, _| Ok(())).unwrap(), Outcome::Accepted);
    let claimed: String = s.e.primary.query_row("SELECT last_triggered_at FROM bot_signals WHERE id = ?1", [s.rule], |r| r.get(0)).unwrap();
    assert_eq!(claimed, "2026-09-01 14:00:01.123456");
    let waiting = model::load_bot(&s.e.primary, s.bot).unwrap().signal_calls().unwrap();
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].source.starts_with("hook:") && waiting[0].signal_id == s.rule && waiting[0].triggered_at == t("2026-09-01T14:00:01.123456Z"), "{waiting:?}");
    assert_eq!(s.e.primary.query_row("SELECT updated_at FROM bots WHERE id = ?1", [s.bot], |r| r.get::<_, String>(0)).unwrap(), updated, "the call leaves updated_at alone");
    // The same instant and 29.999 s later are the cooldown; nothing more is queued.
    for later in [0, 29_999_999] {
        assert_eq!(hook::receive(&s.e.primary, "tok-1", now + chrono::Duration::microseconds(later), |_, _| Ok(())).unwrap(), Outcome::Ignored("cooldown"));
    }
    run::step(&mut s.e, &FixedClock(now + chrono::Duration::seconds(1))).await.unwrap();
    assert_eq!((s.venue.sent().len(), calls(&s.e.primary, s.bot)), (1, 0));
}

/// A call the guard refuses rolls its claim back with it: nothing written, so a retry is not a cooldown.
#[test]
fn a_webhook_call_the_guard_refuses_writes_nothing() {
    use deltabadger::web::hook::{self, Outcome};
    let s = buy();
    let refused = hook::receive(&s.e.primary, "tok-1", t(T0), |_, _| Err("not admitted".into())).unwrap();
    assert_eq!(refused, Outcome::Refused("not admitted".into()));
    let claimed: Option<String> = s.e.primary.query_row("SELECT last_triggered_at FROM bot_signals WHERE id = ?1", [s.rule], |r| r.get(0)).unwrap();
    assert_eq!((claimed, calls(&s.e.primary, s.bot)), (None, 0));
    assert!(s.e.primary.is_autocommit());
    assert_eq!(hook::receive(&s.e.primary, "nope", t(T0), |_, _| Ok(())).unwrap(), Outcome::NotFound);
}
