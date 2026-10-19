mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, not_found, ok, script, venue, POST_SEND};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{amount, model, placement, polling, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::ruby::BigDec;
use deltabadger::store;
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
const STAMP: &str = "2026-09-01T10:00:00.000Z";
const IN: &str = "2026-09-01 10:00:01"; // inside the stamp's window
const T0: &str = "2026-09-01T10:00:00.5Z";
/// A weekly 60 USD BTC bot whose cap was switched on at STAMP.
fn limited(limit: Value) -> BotSpec {
    BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", limit)
        .transient("quote_amount_limit_enabled_at", json!(STAMP))
}
#[allow(clippy::too_many_arguments)]
fn tx(status: i64, ext: Option<i64>, id: Option<&str>, order_type: i64, amount: Option<&'static str>, quote: Option<&'static str>,
      price: Option<&'static str>, qexec: Option<&'static str>, aexec: Option<&'static str>, created: &str) -> TxSpec {
    TxSpec { status, external_status: ext, external_id: id.map(str::to_string), order_type, amount, quote_amount: quote, price,
             quote_amount_exec: qexec, amount_exec: aexec, created_at: created.into() }
}
fn closed(id: &str, quote: &'static str) -> TxSpec { tx(0, Some(2), Some(id), 0, None, Some(quote), Some("64000"), Some(quote), Some("0.0009375"), IN) }
fn available(o: &store::Opened, id: i64) -> Option<BigDec> { amount::quote_amount_available(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap() }
fn filled(id: &str, quote_exec_qty: &str) -> Value {
    ok(json!({ "id": id, "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "60", "qty": null,
               "filled_qty": quote_exec_qty, "filled_avg_price": "64000", "limit_price": null }))
}

#[test]
fn the_tally_counts_each_bucket_as_rails_does() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(500.0)));
    for t in [
        closed("OC", "60"),                                                                                 // closed: its executed quote, 60
        tx(0, Some(1), Some("OO"), 1, Some("0.001"), None, Some("64150"), Some("0"), Some("0"), IN),         // open limit: amount × price, 64.15
        tx(0, Some(0), Some("OU"), 0, None, Some("60"), Some("64000"), None, None, IN),                     // unknown market: its submitted quote, 60
        tx(0, Some(3), Some("OX"), 0, None, Some("60"), Some("64000"), Some("20"), Some("0.0003125"), IN),   // cancelled partly filled: 20
        tx(0, Some(4), Some("OA"), 0, None, Some("60"), Some("64000"), None, None, IN),                     // abandoned, never filled: 0
        tx(1, None, None, 0, None, Some("60"), Some("64000"), Some("0"), Some("0"), IN),                    // failed: not submitted
        tx(2, None, None, 0, None, Some("0.4"), Some("64000"), Some("0"), Some("0"), IN),                   // skipped: not submitted
        tx(0, Some(2), Some("OB"), 0, None, Some("100"), Some("64000"), Some("100"), Some("0.0015625"), "2026-09-01 09:59:59"), // before the stamp
    ] { seed::insert_tx(&o.primary, &s, id, &t); }
    assert_eq!(available(&o, id), Some(bd("295.85")), "500 − (60 + 64.15 + 60 + 20)");
}

/// B2-1: a closed NULL quote with known quantity/price normalizes to its exact cost.
/// Rails #503 returns nil; normalized engine accounting must still credit both $60 fills.
#[test]
fn a_closed_buy_with_reconstructable_cost_counts_toward_the_cap() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(50.0)));
    seed::insert_tx(&o.primary, &s, id, &closed("OC", "60"));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(2), Some("ON"), 0, None, Some("60"), Some("64000"), None, Some("0.0009375"), IN));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(amount::quote_amount_limit_reached(&o.primary, &bot).unwrap(), "B2-1 normalized spend is 120, over the 50 cap");
    assert_eq!(amount::quote_amount_available(&o.primary, &bot).unwrap(),Some(bd("0")));
}

#[test]
fn an_unset_stamp_counts_nothing_an_absent_limit_reads_1000_and_off_is_no_cap() {
    let (_d, o, s) = common::install_alpaca();
    let no_stamp = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(50.0)));
    seed::insert_tx(&o.primary, &s, no_stamp, &closed("OC1", "60"));
    assert_eq!(available(&o, no_stamp), Some(bd("50")), "created_at >= NULL matches nothing");
    let absent = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true))
        .transient("quote_amount_limit_enabled_at", json!(STAMP)));
    assert_eq!(available(&o, absent), Some(bd("1000")), "after_initialize: quote_amount_limit ||= 1000");
    let off = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limit", json!(50.0)));
    assert_eq!(available(&o, off), None);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_intent_counts_as_spent_and_no_tick_sizes_past_it() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }], "GET /v2/orders:by_client_order_id": [not_found()] }));
    let v = venue(&t);
    assert!(matches!(tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap(), TickOutcome::AwaitingReconciliation));
    // The order may have landed, so its whole quote counts until it is settled. Rails, with no row, would say 100.
    assert_eq!(available(&o, id), Some(bd("40")));
    // No tick sizes while the intent is unresolved, so the term never binds a sizing; pinned anyway.
    let out = tick::tick(&o.primary, &v, id, &FixedClock(at(T0) + Duration::minutes(5)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::AwaitingReconciliation), "{out:?}");
    assert_eq!(t.posted_orders().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_cap_cuts_the_order_to_what_is_left_and_nothing_left_is_no_order() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    seed::insert_tx(&o.primary, &s, id, &closed("OC", "60"));
    let t = script(json!({ "GET /v2/orders/OTX-1": [ok(json!({ "id": "OTX-1", "status": "accepted", "symbol": "BTC/USD", "type": "market", "side": "buy",
        "notional": "40", "qty": null, "filled_qty": "0", "filled_avg_price": null, "limit_price": null }))] }));
    let v = venue(&t);
    // Two weeks owed (120) − 60 invested = 60; 100 − 60 = 40 left under the cap.
    tick::tick(&o.primary, &v, id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert_eq!(t.posted_orders()[0]["notional"], "40.00");
    let (rows, logs): (i64, i64) = (one(&o, "SELECT count(*) FROM transactions"), one(&o, "SELECT count(*) FROM bot_activity_logs"));
    // A week on: 80 owed, but 60 closed + 40 waiting spend the cap. No order, no row, no log (set_orders' amount.zero?).
    let out = tick::tick(&o.primary, &v, id, &FixedClock(at("2026-09-15T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: false }), "{out:?}");
    assert_eq!(t.posted_orders().len(), 1);
    assert_eq!((one::<i64>(&o, "SELECT count(*) FROM transactions"), one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs")), (rows, logs));
}

#[tokio::test(flavor = "current_thread")]
async fn a_remainder_under_the_venue_minimum_is_skipped_every_tick_and_the_bot_keeps_running() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.4)));
    seed::insert_tx(&o.primary, &s, id, &closed("OC", "60"));
    let t = script(json!({}));
    for (week, rows) in [("2026-09-08T10:00:00.5Z", 1), ("2026-09-15T10:00:00.5Z", 2)] {
        let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(week)), &mut Attempts::default()).await.unwrap();
        assert!(matches!(out, TickOutcome::Done { placed: false }), "{week}: {out:?}");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), rows, "{week}: 0.40 left, under the 1 USD minimum");
        assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Scheduled, "0.40 is over the 0.01 precision floor: not reached");
    }
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_fill_that_spends_the_cap_stops_the_bot_as_bot_stop_job_does() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
    let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-P"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P", "0.0009375")] }));
    polling::follow_up(&o.primary, &venue(&t), id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
    let (status, stopped_at, key): (i64, String, String) = o.primary.query_row("SELECT status, stopped_at, stop_message_key FROM bots WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((status, stopped_at.as_str(), key.as_str()), (BotStatus::Stopped as i64, "2026-09-01 10:00:06", tick::AMOUNT_SPENT));
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'stopped'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap(), json!({ "stop_message_key": "bot.settings.extra_amount_limit.amount_spent" }));
}

#[tokio::test(flavor = "current_thread")]
async fn a_fill_short_of_the_cap_does_not_stop_the_bot() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-P"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P", "0.0009375")] }));
    polling::follow_up(&o.primary, &venue(&t), id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Scheduled);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stopped_bot_is_stopped_again_as_bot_stop_job_does() {
    for (status, stopped_again) in [(2, true), (7, false)] {
        let (_d, o, s) = common::install_alpaca();
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..limited(json!(60.0)) });
        o.primary.execute("UPDATE bots SET stopped_at = '2026-08-01 00:00:00', stop_message_key = 'bot.status.stopped_by_user' WHERE id = ?1", [id]).unwrap();
        let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-P"), 0, None, Some("60"), Some("64000"), None, None, IN));
        let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P", "0.0009375")] }));
        polling::follow_up(&o.primary, &venue(&t), id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
        let (stopped_at, key): (String, String) = o.primary.query_row("SELECT stopped_at, stop_message_key FROM bots WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        if stopped_again {
            assert_eq!((stopped_at.as_str(), key.as_str()), ("2026-09-01 10:00:06", tick::AMOUNT_SPENT), "Bot::Lifecycle#stop stops a stopped bot again");
        } else {
            assert_eq!((stopped_at.as_str(), key.as_str()), ("2026-08-01 00:00:00", "bot.status.stopped_by_user"), "archived: Bot::Lifecycle#stop returns early");
        }
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), stopped_again as i64);
    }
}

/// Bot::QuoteAmountLimitable#handle_quote_amount_limit_update mails stopped_by_amount_limit from the fill's own callback,
/// whatever the stop then does (an archived bot is mailed too), and only when the fill spends the cap: the mail is owed in
/// the fill's transaction, on a follow-up poll and on a swept fill alike.
#[tokio::test(flavor = "current_thread")]
async fn a_fill_that_spends_the_cap_owes_the_amount_limit_mail_in_its_own_transaction() {
    let limit_mail = |o: &store::Opened| one::<Option<String>>(o, "SELECT json_extract(transient_data, '$.rust_limit_mail_pending.stamped_at') FROM bots");
    for (status, cap, sweep, owed) in [(1, 60.0, false, true), (7, 60.0, false, true), (1, 100.0, false, false), (1, 60.0, true, true)] {
        let (_d, o, s) = common::install_alpaca();
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..limited(json!(cap)) });
        let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-P"), 0, None, Some("60"), Some("64000"), None, None, IN));
        let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P", "0.0009375")] }));
        if sweep {
            polling::sweep(&o.primary, &venue(&t), &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T10:00:06Z")).await.unwrap();
        } else {
            polling::follow_up(&o.primary, &venue(&t), id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
        }
        let expected = owed.then(|| "2026-09-01T10:00:06.000Z".to_string());
        assert_eq!(limit_mail(&o), expected, "status {status}, cap {cap}, sweep {sweep}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_recovered_order_is_counted_once() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
    let v = venue(&t);
    tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap(); // 60 sent, reply lost
    let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    t.reply("GET /v2/orders:by_client_order_id", 200, json!({ "id": "OTX-9", "client_order_id": cl, "status": "filled", "symbol": "BTC/USD", "type": "market",
        "side": "buy", "notional": "60", "qty": null, "filled_qty": "0.0009375", "filled_avg_price": "64000", "limit_price": null }));
    let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), &FixedClock(at(T0) + Duration::seconds(5))).await.unwrap();
    assert!(matches!(recovered, placement::Recovery::Recorded(_)), "{recovered:?}");
    // 100 − 60 = 40 left. Counted twice (its row and its intent) it would read 0 and stop the bot.
    assert_eq!(available(&o, id), Some(bd("40")));
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Retrying);
}

#[tokio::test(flavor = "current_thread")]
async fn a_swept_fill_that_spends_the_cap_stops_the_bot_after_the_tick_as_bot_stop_job_does() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.005)));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-S"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-S": [filled("OTX-S", "0.0009375")] }));
    // A week on: 120 owed − 60 = 60, cut to the 0.005 the cap leaves.
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: false }), "{out:?}");
    // Rails' Bot::StopJob runs after the run whose sweep committed the fill: that run still sizes the 0.005 left (under the
    // venue minimum: a skipped row and an order_skipped warning), and only then is the bot stopped.
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), 1);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'order_skipped'"), 1);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
    assert_eq!(one::<String>(&o, "SELECT stop_message_key FROM bots"), tick::AMOUNT_SPENT);
}

// The stop a swept fill owes is persisted with the fill, so a crash before the tick ends replays it.
struct ScriptedFactory(deltabadger::venue::http::ScriptedTransport);
impl deltabadger::venue::VenueFactory for ScriptedFactory {
    type V = deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(&self.0) }
}

#[tokio::test(flavor = "current_thread")]
async fn a_crash_between_a_swept_fill_and_its_stop_replays_the_stop_at_the_next_start() {
    let dir = common::rails_install();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let o = store::open(&paths).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-S"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-S": [filled("OTX-S", "0.0009375")] }));
    // The tick stamps its run, its sweep commits the fill, then the process dies before the tick's end runs the stop. The
    // stamp makes the bot not due at the restart, so only the replay can stop it.
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.last_action_job_at', '2026-09-08T10:00:00.500Z') WHERE id = ?1", [id]).unwrap();
    polling::sweep(&o.primary, &venue(&t), &model::load_bot(&o.primary, id).unwrap(), at("2026-09-08T10:00:00.5Z")).await.unwrap();
    assert_eq!(one::<i64>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots"), 1, "persisted with the fill");
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Scheduled);
    drop(o);
    let lock = deltabadger::lease::lock(&paths, at("2026-09-08T10:05:00Z")).unwrap();
    let mut e = deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, ScriptedFactory(t.clone()), seed::cipher(), lock);
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-08T10:05:00Z"))).await.unwrap();
    let (status, key, pending): (i64, String, Option<i64>) = e.primary.query_row(
        "SELECT status, stop_message_key, json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((status, key.as_str(), pending), (BotStatus::Stopped as i64, tick::AMOUNT_SPENT, None));
    let stopped: i64 = e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'", [], |r| r.get(0)).unwrap();
    assert_eq!(stopped, 1);
    assert!(t.posted_orders().is_empty(), "a stopped bot places nothing");
}

#[tokio::test(flavor = "current_thread")]
async fn two_swept_fills_that_spend_the_cap_stop_the_bot_twice_as_two_stop_jobs_do() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-A"), 0, None, Some("40"), Some("64000"), None, None, IN));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-B"), 0, None, Some("30"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-A": [filled("OTX-A", "0.000625")], "GET /v2/orders/OTX-B": [filled("OTX-B", "0.00046875")] }));
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    // Each fill's after_commit finds the cap spent and enqueues its own Bot::StopJob; each writes a `stopped` log.
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 2);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
    assert!(one::<Option<i64>>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_basket_whose_swept_fill_spends_the_cap_places_no_leg_and_stops_once_after_the_tick() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.005)).weights(&[(s.btc, 0.5), (eth, 0.5)]));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-S"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-S": [filled("OTX-S", "0.0009375")] }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: false }), "{out:?}");
    assert!(t.posted_orders().is_empty(), "the 0.005 the cap leaves buys no leg");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 1);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
}

/// Accepted divergence: an unresolved leg's intent counts as spent. A fill that brings the tally to the cap with that intent
/// stops the bot, and when the leg later proves never placed the bot stays stopped below its cap. Rails has no row for the
/// leg, so its fill leaves the cap unreached and it keeps buying.
#[tokio::test(flavor = "current_thread")]
async fn a_stop_that_counted_an_intent_later_proved_never_placed_stays_a_stop() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)).weights(&[(s.btc, 0.5), (eth, 0.5)]));
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "network": "post_send", "message": POST_SEND }],
                           "GET /v2/orders/OTX-1": [filled("OTX-1", "0.00046875")], "GET /v2/orders:by_client_order_id": [not_found()] }));
    let v = venue(&t);
    let out = tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::AwaitingReconciliation), "{out:?}");
    let leg: i64 = one(&o, "SELECT id FROM transactions WHERE external_id = 'OTX-1'");
    // Leg 1's 30 filled plus leg 2's unresolved 30 reach the 60 cap.
    polling::follow_up(&o.primary, &v, id, leg, at(T0) + Duration::seconds(6)).await.unwrap();
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
    assert_eq!(one::<String>(&o, "SELECT stop_message_key FROM bots"), tick::AMOUNT_SPENT);
    // Leg 2 never reached the venue: the intent goes, the stop stays, 30 of the cap is left unspent.
    let out = tick::tick(&o.primary, &v, id, &FixedClock(at(T0) + Duration::minutes(21)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Skipped), "{out:?}");
    // A stopped tick writes nothing. The engine's independent idle reconciliation settles its
    // already-transmitted order (run::reconcile_idle), without granting that tick a new run.
    let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(),
        &FixedClock(at(T0) + Duration::minutes(21))).await.unwrap();
    assert!(matches!(recovered, placement::Recovery::NotPlaced), "{recovered:?}");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none(), "settled as not placed");
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped, "the stop is not undone");
    assert_eq!(available(&o, id), Some(bd("30")));
    assert_eq!(t.posted_orders().len(), 2);
}

/// A tick that fails with a plain database error after its sweep spent the cap still runs the stop the sweep counted, as
/// Rails' Bot::StopJob runs whatever the run's own outcome.
#[tokio::test(flavor = "current_thread")]
async fn a_swept_stop_lands_when_the_tick_then_fails_on_the_database() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.005)).transient("last_failure_kind", json!("insufficient_funds")));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-S"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-S": [filled("OTX-S", "0.0009375")] }));
    // Clearing the failure state fails as a busy database would.
    o.primary.execute_batch("CREATE TEMP TRIGGER busy BEFORE UPDATE OF transient_data ON bots \
        WHEN json_extract(OLD.transient_data, '$.last_failure_kind') IS NOT NULL AND json_extract(NEW.transient_data, '$.last_failure_kind') IS NULL \
        BEGIN SELECT RAISE(ABORT, 'database is locked'); END").unwrap();
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await;
    assert!(out.is_err(), "{out:?}");
    let (status, key, pending): (i64, String, Option<i64>) = o.primary.query_row(
        "SELECT status, stop_message_key, json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((status, key.as_str(), pending), (BotStatus::Stopped as i64, tick::AMOUNT_SPENT, None));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 1);
}

/// A stop an earlier tick counted but could not run (its database error outlasted the tick) lands before the next tick sizes
/// anything: Rails' Bot::StopJob ran long before that checkpoint, which therefore writes no skipped row.
#[tokio::test(flavor = "current_thread")]
async fn a_stop_left_pending_by_an_earlier_tick_lands_before_the_next_tick_sizes() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.005)));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-S"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-S": [filled("OTX-S", "0.0009375")] }));
    // The earlier tick's sweep counted the stop; its end never ran it.
    polling::sweep(&o.primary, &venue(&t), &model::load_bot(&o.primary, id).unwrap(), at("2026-09-08T10:00:00.5Z")).await.unwrap();
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Skipped), "{out:?}");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), 0, "the 0.005 left is never sized");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'order_skipped'"), 0);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 1);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
    assert!(one::<Option<i64>>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots").is_none());
}

/// One swept fill that spent the 60 cap, its stop counted and not yet run (the tick then ended in an error that skipped it).
async fn counted_stop() -> (tempfile::TempDir, store::Opened, i64) {
    let (d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
    seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-S"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-S": [filled("OTX-S", "0.0009375")] }));
    polling::sweep(&o.primary, &venue(&t), &model::load_bot(&o.primary, id).unwrap(), at("2026-09-08T10:00:00.5Z")).await.unwrap();
    assert_eq!(one::<i64>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots"), 1);
    (d, o, id)
}
fn stops(o: &store::Opened) -> i64 { one(o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'") }
fn pending(o: &store::Opened) -> Option<String> { one(o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots") }

/// Rails' Bot::StopJob ran right after the fill, long before the user started the bot afresh; a stop counted before that
/// start is not run on the restarted bot.
#[tokio::test(flavor = "current_thread")]
async fn a_counted_stop_is_discarded_after_a_fresh_start() {
    let (_d, o, id) = counted_stop().await;
    o.primary.execute("UPDATE bots SET started_at = '2026-09-08 11:00:00' WHERE id = ?1", [id]).unwrap();
    let out = tick::tick(&o.primary, &venue(&script(json!({}))), id, &FixedClock(at("2026-09-08T11:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { .. }), "{out:?}");
    assert_eq!((model::load_bot(&o.primary, id).unwrap().status, stops(&o), pending(&o)), (BotStatus::Scheduled, 0, None));
}

/// A stop counted under a 60 cap is not run once the user raised the cap: the cause is gone.
#[tokio::test(flavor = "current_thread")]
async fn a_counted_stop_is_discarded_after_the_cap_is_raised() {
    let (_d, o, id) = counted_stop().await;
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limit', 200.0) WHERE id = ?1", [id]).unwrap();
    let t = script(json!({}));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:01Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!((model::load_bot(&o.primary, id).unwrap().status, stops(&o), pending(&o)), (BotStatus::Scheduled, 0, None));
    assert_eq!(t.posted_orders()[0]["notional"], "60.00", "120 owed − 60 invested, 140 left under the raised cap");
}

#[tokio::test(flavor = "current_thread")]
async fn a_counted_stop_on_an_unchanged_bot_stops_it_exactly_once() {
    let (_d, o, id) = counted_stop().await;
    tick::tick(&o.primary, &venue(&script(json!({}))), id, &FixedClock(at("2026-09-08T10:00:01Z")), &mut Attempts::default()).await.unwrap();
    assert_eq!((model::load_bot(&o.primary, id).unwrap().status, stops(&o), pending(&o)), (BotStatus::Stopped, 1, None));
    tick::tick(&o.primary, &venue(&script(json!({}))), id, &FixedClock(at("2026-09-08T10:00:02Z")), &mut Attempts::default()).await.unwrap();
    assert_eq!(stops(&o), 1, "nothing left to run twice");
}

/// After a lease or store error the engine no longer writes: the counted stop stays for the next start or the handback.
#[tokio::test(flavor = "current_thread")]
async fn a_lease_or_store_error_leaves_the_counted_stop_unrun() {
    for err in [deltabadger::engine::EngineError::Lease(deltabadger::lease::LeaseError::Locked),
                deltabadger::engine::EngineError::Store(store::StoreError::Behind { missing: vec![] })] {
        let (_d, o, id) = counted_stop().await;
        assert!(tick::end_of_tick(&o.primary, id, at("2026-09-08T10:00:01Z"), Err(err)).is_err());
        assert_eq!((model::load_bot(&o.primary, id).unwrap().status, stops(&o)), (BotStatus::Scheduled, 0));
        assert_eq!(one::<i64>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots"), 1);
    }
}

/// A lost reply, then the recovery finds the order filled and its fill spends the cap: the bot is stopped in the recovery's
/// transaction, and the stopped_by_amount_limit mail is owed there too, as on the poll and the sweep.
#[tokio::test(flavor = "current_thread")]
async fn a_recovered_fill_that_spends_the_cap_owes_the_limit_mail_with_its_stop() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
    let v = venue(&t);
    tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap(); // 60 sent, reply lost
    let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    t.reply("GET /v2/orders:by_client_order_id", 200, json!({ "id": "OTX-9", "client_order_id": cl, "status": "filled", "symbol": "BTC/USD", "type": "market",
        "side": "buy", "notional": "60", "qty": null, "filled_qty": "0.0009375", "filled_avg_price": "64000", "limit_price": null }));
    let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), &FixedClock(at(T0) + Duration::seconds(5))).await.unwrap();
    assert!(matches!(recovered, placement::Recovery::Recorded(_)), "{recovered:?}");
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.status, BotStatus::Stopped);
    assert!(b.transient.get(deltabadger::engine::notice::LIMIT).is_some(), "the limit mail is owed: {}", b.transient);
}

// ---- 6a: the buy cap is direction-aware (quote_amount_limitable.rb:106; placement and intents count buys only)

#[tokio::test(flavor = "current_thread")]
async fn a_late_buy_fill_after_a_flip_into_selling_does_not_stop_the_seller() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)).with("direction", json!("selling")));
    let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-P"), 0, None, Some("60"), Some("64000"), None, None, IN));
    let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P", "0.0009375")] }));
    polling::follow_up(&o.primary, &venue(&t), id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Scheduled);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 0);
}

#[test]
fn an_unresolved_sell_intent_is_not_counted_as_buy_spending() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    let set = |side: Value| o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json(?1)) WHERE id = ?2",
        rusqlite::params![json!({ "quote_amount": "60", "side": side }).to_string(), id]).unwrap();
    set(json!("sell"));
    assert_eq!(available(&o, id), Some(bd("100")), "a sell spends none of the buy cap");
    assert_eq!(amount::exact_cap_available(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap(), Some(bd("100")));
    set(json!("buy"));
    assert_eq!(available(&o, id), Some(bd("40")));
    set(Value::Null);
    assert_eq!(available(&o, id), Some(bd("40")), "an intent from before selling names no side: a buy");
}

#[test]
fn an_exhausted_buy_cap_never_trims_a_sell_at_placement() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
    seed::insert_tx(&o.primary, &s, id, &closed("OTX-1", "60"));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(amount::exact_cap_available(&o.primary, &bot).unwrap(), Some(BigDec::zero()), "the buy cap is spent");
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size_sell(&bot, &ticker, &bd("0.001"), &bd("64000")).unwrap() else { panic!() };
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(at(T0))).unwrap();
    assert_eq!((intent.plan.volume.clone(), intent.plan.quote_amount.clone()), (plan.volume.clone(), plan.quote_amount.clone()), "untrimmed");
}

// ---- 6b T1: a REBALANCE order is not a contribution (Bot::OrderCreator#order_identity_values; Rails scopes the carry, the
// spend cap and the pending-buy spending to 'REGULAR').

fn buy_plan(o: &store::Opened, bot: &model::Bot, kind: amount::TxKind) -> amount::OrderPlan {
    let ticker = model::ticker_for(&o.primary, bot).unwrap().unwrap();
    let amount::Sizing::Place(mut plan) = amount::size(bot, &ticker, &bd("60"), &bd("64000"), deltabadger::engine::venue_rules::ALPACA.minimum_logic).unwrap() else { panic!() };
    plan.kind = kind;
    plan
}

#[test]
fn an_unresolved_rebalance_buy_intent_is_not_counted_as_buy_spending() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    let set = |kind: &str| o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json(?1)) WHERE id = ?2",
        rusqlite::params![json!({ "quote_amount": "60", "side": "buy", "transaction_type": kind }).to_string(), id]).unwrap();
    set("REBALANCE");
    assert_eq!(available(&o, id), Some(bd("100")), "an owed rebalance buy spends none of the DCA cap");
    assert_eq!(amount::exact_cap_available(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap(), Some(bd("100")));
    set("REGULAR");
    assert_eq!(available(&o, id), Some(bd("40")));
}

#[test]
fn a_stopped_bot_with_an_exhausted_dca_cap_still_sizes_an_owed_rebalance_buy() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: BotStatus::Stopped as i64, ..limited(json!(60.0)) });
    seed::insert_tx(&o.primary, &s, id, &closed("OTX-1", "60"));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(amount::exact_cap_available(&o.primary, &bot).unwrap(), Some(BigDec::zero()), "the DCA cap is spent");
    let rebalance = buy_plan(&o, &bot, amount::TxKind::Rebalance);
    let amount::CapGuard::Place(placed) = amount::guard_exact_cap(&o.primary, &bot, &rebalance).unwrap() else { panic!("a rebalance buy is not the DCA cap's") };
    assert_eq!((placed.volume, placed.quote_amount, placed.kind), (rebalance.volume.clone(), rebalance.quote_amount.clone(), amount::TxKind::Rebalance));
    assert!(!matches!(amount::guard_exact_cap(&o.primary, &bot, &buy_plan(&o, &bot, amount::TxKind::Regular)), Ok(amount::CapGuard::Place(_))),
        "a REGULAR buy is still refused by the spent cap");
}

#[test]
fn a_rebalance_row_is_written_as_rebalance_and_its_fill_owes_the_carry_and_the_cap_nothing() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(100.0)));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let now = at("2026-09-01T10:00:01Z");
    let owed = amount::pending_quote_amount(&o.primary, &bot, now.timestamp_micros()).unwrap();
    for (kind, external_id) in [(amount::TxKind::Rebalance, "OTX-R"), (amount::TxKind::Regular, "OTX-G")] {
        let tx = o.primary.unchecked_transaction().unwrap();
        let row = amount::write_accepted_order(&tx, &bot, &buy_plan(&o, &bot, kind), external_id, now, None).unwrap();
        tx.commit().unwrap();
        o.primary.execute("UPDATE transactions SET external_status = 2, quote_amount_exec = 60, amount_exec = 0.0009375 WHERE id = ?1", [row]).unwrap();
        let t: String = o.primary.query_row("SELECT transaction_type FROM transactions WHERE id = ?1", [row], |r| r.get(0)).unwrap();
        assert_eq!(t, kind.as_str());
        let bot = model::load_bot(&o.primary, id).unwrap();
        let (left, pending) = (available(&o, id), amount::pending_quote_amount(&o.primary, &bot, now.timestamp_micros()).unwrap());
        if kind == amount::TxKind::Rebalance {
            assert_eq!((left, pending), (Some(bd("100")), owed.clone()), "a rebalance fill moves neither the cap nor the carry");
        } else {
            assert_eq!(left, Some(bd("40")));
            assert!(pending < owed, "a REGULAR fill is a contribution");
        }
    }
}

/// Transaction's after_commit runs handle_quote_amount_limit_update for any BUY whose executed quote changed, a REBALANCE leg
/// included; the re-check itself counts REGULAR rows only.
#[tokio::test(flavor = "current_thread")]
async fn a_rebalance_buy_fill_rechecks_the_cap_as_any_buy_fill_does() {
    for (regular_spent, stopped) in [(false, false), (true, true)] {
        let (_d, o, s) = common::install_alpaca();
        let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.0)));
        if regular_spent { seed::insert_tx(&o.primary, &s, id, &closed("OTX-1", "60")); }
        let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-P"), 0, None, Some("60"), Some("64000"), None, None, IN));
        o.primary.execute("UPDATE transactions SET transaction_type = 'REBALANCE' WHERE id = ?1", [order]).unwrap();
        let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P", "0.0009375")] }));
        polling::follow_up(&o.primary, &venue(&t), id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
        assert_eq!(model::load_bot(&o.primary, id).unwrap().status == BotStatus::Stopped, stopped, "REGULAR spent: {regular_spent}");
    }
}

// ---- plan CB T2: the quote cap on Kraken, Binance and KuCoin. The exact-cap guard runs for every REGULAR buy; a reduced
// order is checked against the venue's whole minimum (Exchange#minimum_amount_logic) in its own denomination, at its own price.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Form { Quote, Base, Limit }
const CRYPTO: [(&str, &str); 3] = [("Exchanges::Kraken", "XBTEUR"), ("Exchanges::Binance", "BTCEUR"), ("Exchanges::Kucoin", "BTC-EUR")];
fn leak(s: &str) -> &'static str { Box::leak(s.to_string().into_boxed_str()) }
/// The order price each form is sized at: quote-sized at 30000 (the 5 EUR floor binds, in quote), base-sized at 100000 (the
/// 0.0001 BTC floor binds, in base), a limit buy at 30000 (distance 0, so the limit price is the reference).
fn form_price(form: Form) -> &'static str { if form == Form::Base { "100000" } else { "30000" } }

/// A weekly BTC bot on `venue`, capped at 100 EUR with `spent` of it filled, on a pair of 8 base and 2 quote decimals whose
/// floors are 0.0001 BTC and 5 EUR.
fn crypto_capped(venue: &str, pair: &str, spent: &str, form: Form) -> (tempfile::TempDir, store::Opened, model::Bot) {
    let (d, o, s) = common::install();
    o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
    o.primary.execute("UPDATE tickers SET ticker = ?1, base_decimals = 8, quote_decimals = 2, price_decimals = 1, minimum_base_size = '0.0001', minimum_quote_size = '5'", [pair]).unwrap();
    let mut spec = limited(json!(100));
    if form == Form::Limit { spec = spec.with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0)); }
    let id = seed::insert_bot(&o.primary, &s, &spec);
    seed::insert_tx(&o.primary, &s, id, &closed("OC-1", leak(spent)));
    let bot = model::load_bot(&o.primary, id).unwrap();
    (d, o, bot)
}

fn sized(o: &store::Opened, bot: &model::Bot, venue: &str, x: &str, form: Form) -> amount::OrderPlan {
    let ticker = model::ticker_for(&o.primary, bot).unwrap().unwrap();
    let logic = deltabadger::engine::venue_rules::for_exchange(venue).unwrap().minimum_logic;
    let amount::Sizing::Place(plan) = amount::size(bot, &ticker, &bd(x), &bd(form_price(form)), logic).unwrap() else { panic!("{venue} {form:?} {x}: sized to place") };
    assert_eq!(plan.quote_type, form == Form::Quote, "{venue} {form:?}: Rails sizes this form");
    plan
}

/// The 9 cells (3 venues × quote-sized MARKET, base-sized MARKET, LIMIT), each: a trim within 4 increments, a trim landing
/// exactly on the venue minimum (placed), and one increment below it (a skipped row).
#[test]
fn the_exact_cap_guard_trims_every_crypto_order_form_and_checks_the_venues_whole_minimum() {
    for (venue, pair) in CRYPTO {
        let notional_limit = venue != "Exchanges::Kraken"; // Binance and KuCoin limit buys: :base_and_quote_in_base; Kraken's: :base
        for form in [Form::Quote, Form::Base, Form::Limit] {
            // (case, spent, requested quote, Some(guarded volume, its cost) placed / None skipped)
            let cells: Vec<(&str, &str, &str, Option<(&str, &str)>)> = match form {
                Form::Quote => vec![("trim", "80", "20.03", Some(("20", "20"))), ("minimum", "95", "5.02", Some(("5", "5"))), ("below", "95.01", "5.02", None)],
                // 0.00009999 BTC costs 9.999, above the 5 EUR quote floor: only the base floor skips it.
                Form::Base => vec![("trim", "80", "20.003", Some(("0.0002", "20"))), ("minimum", "90", "10.002", Some(("0.0001", "10"))), ("below", "90.001", "10.002", None)],
                Form::Limit if notional_limit => vec![("trim", "80", "20.0009", Some(("0.00066666", "19.9998"))),
                    ("minimum", "94.9999", "5.0007", Some(("0.00016667", "5.0001"))), ("below", "95", "5.0007", None)],
                // Kraken's limit floor is the base one: 0.0001 BTC at 3 EUR is placed although it is under the 5 EUR quote floor.
                Form::Limit => vec![("trim", "80", "20.0009", Some(("0.00066666", "19.9998"))), ("minimum", "97", "3.0006", Some(("0.0001", "3"))),
                    ("below", "97.0003", "3.0006", None)],
            };
            for (case, spent, x, want) in cells {
                let (_d, o, bot) = crypto_capped(venue, pair, spent, form);
                let plan = sized(&o, &bot, venue, x, form);
                let exact = amount::exact_cap_available(&o.primary, &bot).unwrap().unwrap();
                let label = format!("{venue} {form:?} {case}");
                match (amount::guard_exact_cap(&o.primary, &bot, &plan).unwrap(), want) {
                    (amount::CapGuard::Place(g), Some((volume, cost))) => {
                        assert_eq!((g.volume.clone(), g.quote_amount.clone()), (bd(volume), bd(cost)), "{label}");
                        assert!(g.quote_amount <= exact, "{label}: within the exact cap");
                        // The denomination is kept: a quote-sized market buy stays in quote, the others store base units.
                        assert_eq!((g.quote_type, g.limit, g.price.clone()), (form == Form::Quote, form == Form::Limit, plan.price.clone()), "{label}");
                        assert!(g.volume < plan.volume, "{label}: reduced");
                    }
                    (amount::CapGuard::BelowMinimum(g), None) => assert!(g.quote_amount <= exact, "{label}"),
                    (_, want) => panic!("{label}: expected {want:?}"),
                }
                // Placement runs the same guard on every venue: a skipped trim writes no intent.
                let begun = placement::begin(&o.primary, &bot, &plan, &FixedClock(at(T0)));
                assert_eq!(begun.is_ok(), want.is_some(), "{label}: {:?}", begun.as_ref().err());
                assert_eq!(model::load_bot(&o.primary, bot.id).unwrap().rust_placement().is_some(), want.is_some(), "{label}");
            }
        }
    }
}

/// The trimmed intent of each cell, on each venue's own wire: its reply lost, then recovered by its client order id. The
/// intent reads back (Intent::from_json) in its guarded denomination, and the row is the guarded order's.
#[tokio::test(flavor = "current_thread")]
async fn a_trimmed_intent_is_sent_and_recovered_in_its_own_denomination_on_each_crypto_wire() {
    use deltabadger::engine::venue_rules::WireFormat;
    use deltabadger::venue::http::ScriptedTransport;
    for (venue, pair) in CRYPTO {
        for (form, spent, x) in [(Form::Quote, "80", "20.03"), (Form::Base, "80", "20.003"), (Form::Limit, "80", "20.0009")] {
            let (_d, o, bot) = crypto_capped(venue, pair, spent, form);
            let plan = sized(&o, &bot, venue, x, form);
            let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(at(T0))).unwrap();
            let label = format!("{venue} {form:?}");
            let stored = model::load_bot(&o.primary, bot.id).unwrap().rust_placement().unwrap().clone();
            assert_eq!((stored["quote_type"].as_bool(), bd(stored["volume"].as_str().unwrap())), (Some(form == Form::Quote), intent.plan.volume.clone()), "{label}");
            let wire = intent.plan.to_order(intent.cl_ord_id.clone(), intent.deadline, WireFormat::Kraken).unwrap();
            let want_volume = match form { Form::Quote => "20", Form::Base => "0.0002", Form::Limit => "0.00066666" };
            assert_eq!((bd(&wire.volume), wire.quote_volume), (bd(want_volume), form == Form::Quote), "{label}: the volume sent");
            let cl = intent.cl_ord_id.clone();
            let (kind, price) = if form == Form::Limit { ("limit", "30000") } else { ("market", "0") };
            let clock = FixedClock(at(T0) + Duration::seconds(5));
            let bot = model::load_bot(&o.primary, bot.id).unwrap();
            let recovered = match venue {
                "Exchanges::Kraken" => {
                    let raw = json!({ "status": "open", "cl_ord_id": cl, "oflags": if form == Form::Quote { "viqc" } else { "" }, "vol": wire.volume, "vol_exec": "0",
                                      "cost": "0", "price": "0", "descr": { "pair": pair, "type": "buy", "ordertype": kind, "price": price } });
                    let t = ScriptedTransport::from_script(&json!({ "POST /0/private/OpenOrders": [{ "status": 200, "body": { "error": [], "result": { "open": { "OTX-R": raw } } } }] }));
                    placement::recover(&o.primary, &deltabadger::venue::kraken::KrakenVenue::new(t), &bot, &clock).await.unwrap()
                }
                "Exchanges::Binance" => {
                    let (qty, quote) = if form == Form::Quote { ("0.00000000", wire.volume.as_str()) } else { (wire.volume.as_str(), "0.00000000") };
                    let raw = json!({ "symbol": pair, "orderId": 28457, "orderListId": -1, "clientOrderId": cl, "price": if form == Form::Limit { "30000.00000000" } else { "0.00000000" },
                                      "origQty": qty, "executedQty": "0.00000000", "cummulativeQuoteQty": "0.00000000", "status": "NEW", "timeInForce": "GTC",
                                      "type": kind.to_uppercase(), "side": "BUY", "origQuoteOrderQty": quote });
                    let t = ScriptedTransport::from_script(&json!({ "GET /api/v3/order": [{ "status": 200, "body": raw }] }));
                    let mut pairs = deltabadger::venue::binance::Pairs::default();
                    pairs.by_symbol.insert(pair.into(), deltabadger::venue::binance::Pair { base: "BTC".into(), quote: "EUR".into(), price_decimals: 1 });
                    let v = deltabadger::venue::binance::BinanceVenue::new(t, "https://api.binance.com", "s").with_pairs(pairs);
                    placement::recover(&o.primary, &v, &bot, &clock).await.unwrap()
                }
                _ => {
                    let (size, funds) = if form == Form::Quote { ("0", wire.volume.as_str()) } else { (wire.volume.as_str(), "0") };
                    let raw = json!({ "id": "KC-R", "symbol": pair, "opType": "DEAL", "type": kind, "side": "buy", "price": price, "size": size, "funds": funds,
                                      "dealFunds": "0", "dealSize": "0", "fee": "0", "feeCurrency": "EUR", "clientOid": cl, "isActive": true, "cancelExist": false, "createdAt": 1788000000000u64 });
                    let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/order/client-order/{cl}"): [{ "status": 200, "body": { "code": "200000", "data": raw } }] }));
                    placement::recover(&o.primary, &deltabadger::venue::kucoin::KucoinVenue::new(t, deltabadger::venue::kucoin::URL), &bot, &clock).await.unwrap()
                }
            };
            assert!(matches!(recovered, placement::Recovery::Recorded(_)), "{label}: {recovered:?}");
            assert!(model::load_bot(&o.primary, bot.id).unwrap().rust_placement().is_none(), "{label}");
            // The row as the recovered order reads (#parse_order_data): the guarded quote for a quote-sized buy, else the guarded base.
            let (quote, amount, order_type): (f64, f64, i64) = o.primary.query_row("SELECT quote_amount, amount, order_type FROM transactions WHERE external_id NOT LIKE 'OC-%'",
                [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            let sized = if form == Form::Quote { quote } else { amount };
            assert_eq!((sized, order_type), (want_volume.parse::<f64>().unwrap(), i64::from(form == Form::Limit)), "{label}: the guarded order's row");
        }
    }
}

/// The stop on a fill (Bot::QuoteAmountLimitable#handle_quote_amount_limit_update) is venue-neutral: a Kraken fill recovered
/// from TradesHistory and a KuCoin fill recovered from /fills after #get_order named it missing stop the bot and owe the mail,
/// as a polled fill does; the same late fill on a bot flipped into selling stops nothing (`return if selling?`).
#[tokio::test(flavor = "current_thread")]
async fn a_fill_recovered_from_kraken_trades_or_kucoin_fills_that_spends_the_cap_stops_the_bot() {
    use deltabadger::venue::http::ScriptedTransport;
    for venue in ["Exchanges::Kraken", "Exchanges::Kucoin"] {
        for selling in [false, true] {
            let (_d, o, s) = common::install();
            o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
            let mut spec = limited(json!(60));
            if selling { spec = spec.with("direction", json!("selling")); }
            let id = seed::insert_bot(&o.primary, &s, &spec);
            let order = seed::insert_tx(&o.primary, &s, id, &tx(0, Some(0), Some("OTX-7"), 0, None, Some("60"), Some("50000"), None, None, IN));
            let now = at("2026-09-01T10:00:06Z");
            if venue == "Exchanges::Kraken" {
                let v = deltabadger::venue::fake::FakeVenue::from_script(&json!({ "http": { "/0/private/TradesHistory": [{ "error": [], "result": { "count": 1, "trades": {
                    "T1": { "ordertxid": "OTX-7", "vol": "0.0012", "cost": "60", "fee": "0.156", "type": "buy", "ordertype": "market" } } } }] } }));
                polling::follow_up(&o.primary, &v, id, order, now).await.unwrap();
                assert!(v.calls("/0/private/TradesHistory") > 0, "recovered from TradesHistory");
            } else {
                let fill = json!({ "symbol": "BTC-EUR", "tradeId": "t1", "orderId": "OTX-7", "side": "buy", "type": "market", "price": "50000", "size": "0.0012",
                                   "funds": "60", "fee": "0.06", "feeCurrency": "EUR", "createdAt": 1788220801000u64 });
                let t = ScriptedTransport::from_script(&json!({
                    "GET /api/v1/orders/OTX-7": [{ "status": 404, "body": { "code": "400100", "msg": "order not exist." } }],
                    "GET /api/v1/fills": [{ "status": 200, "body": { "code": "200000", "data": { "currentPage": 1, "pageSize": 500, "totalNum": 1, "totalPage": 1, "items": [fill] } } }] }));
                // KuCoin's follow-up asks for its one order only (a forgotten one is its error); the tick's sweep recovers it.
                let v = deltabadger::venue::kucoin::KucoinVenue::new(t.clone(), deltabadger::venue::kucoin::URL);
                polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), now).await.unwrap();
                assert!(t.requests().iter().any(|r| r.path == "/api/v1/fills"), "recovered from the fills");
            }
            let (ext, qexec): (i64, f64) = o.primary.query_row("SELECT external_status, quote_amount_exec FROM transactions WHERE id = ?1", [order], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert_eq!((ext, qexec), (2, 60.0), "{venue} selling {selling}: the fill is recorded");
            let b = model::load_bot(&o.primary, id).unwrap();
            if selling {
                assert_eq!(b.status, BotStatus::Scheduled, "{venue}: a late buy after the flip does not stop the seller");
                assert!(b.transient.get(deltabadger::engine::notice::LIMIT).is_none(), "{venue}");
                assert!(b.pending_amount_limit_stops().unwrap().is_none(), "{venue}");
            } else if venue == "Exchanges::Kraken" {
                let key: String = one(&o, "SELECT stop_message_key FROM bots");
                assert_eq!((b.status, key.as_str()), (BotStatus::Stopped, tick::AMOUNT_SPENT), "{venue}: the follow-up's stop lands with the fill");
                assert!(b.transient.get(deltabadger::engine::notice::LIMIT).is_some(), "{venue}: the limit mail is owed");
            } else {
                // The sweep's stop runs after the tick (Bot::StopJob after Bot::ActionJob): counted with the fill.
                assert_eq!(one::<i64>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending.count') FROM bots"), 1, "{venue}");
                assert!(b.transient.get(deltabadger::engine::notice::LIMIT).is_some(), "{venue}: the limit mail is owed");
            }
        }
    }
}
