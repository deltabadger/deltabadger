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
    assert_eq!(one::<i64>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots"), 1, "persisted with the fill");
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Scheduled);
    drop(o);
    let lock = deltabadger::lease::lock(&paths, at("2026-09-08T10:05:00Z")).unwrap();
    let mut e = deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, ScriptedFactory(t.clone()), seed::cipher(), lock);
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-08T10:05:00Z"))).await.unwrap();
    let (status, key, pending): (i64, String, Option<i64>) = e.primary.query_row(
        "SELECT status, stop_message_key, json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots WHERE id = ?1", [id],
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
    assert!(one::<Option<i64>>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots").is_none());
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
        "SELECT status, stop_message_key, json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((status, key.as_str(), pending), (BotStatus::Stopped as i64, tick::AMOUNT_SPENT, None));
}

/// A stop an earlier tick counted but could not run (its database error outlasted the tick) lands before the next tick sizes
/// anything: Rails' Bot::StopJob ran long before that checkpoint, which therefore writes no skipped row.
#[tokio::test(flavor = "current_thread")]
async fn a_stop_left_pending_by_an_earlier_tick_lands_before_the_next_tick_sizes() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &limited(json!(60.005)).transient("rust_amount_limit_stops_pending", json!(1)));
    seed::insert_tx(&o.primary, &s, id, &closed("OC", "60"));
    let t = script(json!({}));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Skipped), "{out:?}");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), 0, "the 0.005 left is never sized");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'order_skipped'"), 0);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 1);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
    assert!(one::<Option<i64>>(&o, "SELECT json_extract(transient_data, '$.rust_amount_limit_stops_pending') FROM bots").is_none());
}
