//! Slice 6b: threshold rebalancing (Bot::Rebalancer, Bot::RebalanceJob, Bot::EvaluateRebalancersJob) on Alpaca.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, not_found, ok, POST_SEND, PRE_SEND};
use common::seed::{self, BotSpec};
use deltabadger::engine::rebalance::{self, Outcome};
use deltabadger::engine::tick::{PriceCache, TickContext};
use deltabadger::engine::{eligibility, model, FixedClock};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const T0: &str = "2026-09-01T12:30:00Z";
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }

/// A 50/50 BTC/ETH basket, rebalancing on, holding 0.01 BTC (640 USD) and 0.04 ETH (100 USD): BTC sits 36.5 points over.
fn swap_bot(spec: BotSpec) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded, i64) {
    let (d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &spec.with("rebalance_enabled", json!(true)).weights(&[(s.btc, 0.5), (eth, 0.5)]));
    deltabadger::engine::basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    for (asset, qty, price, quote, n) in [(s.btc, "0.01", "64000", "640", 1), (eth, "0.04", "2500", "100", 2)] {
        seed::insert_row(&o.primary, &s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty, "amount_exec": qty,
            "quote_amount": quote, "quote_amount_exec": quote, "price": price, "created_at": "2026-09-01 10:00:01" }));
    }
    (d, o, id, s, eth)
}
fn stopped() -> BotSpec { BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") } }

fn position(sym: &str, qty: &str) -> Value { json!({ "symbol": sym, "asset_class": "crypto", "qty": qty, "qty_available": qty, "side": "long" }) }
#[allow(clippy::too_many_arguments)]
fn order(id: &str, status: &str, sym: &str, side: &str, qty: Option<&str>, notional: Option<&str>, filled: &str, avg: Option<&str>) -> Value {
    ok(json!({ "id": id, "status": status, "symbol": sym, "type": "market", "side": side, "notional": notional, "qty": qty,
               "filled_qty": filled, "filled_avg_price": avg, "limit_price": null }))
}
/// Prices at one level for bid, ask and last; the account's positions and settled cash; orders accepted as OTX-1, OTX-2…
fn script(over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v1beta3/crypto/us/latest/quotes": [ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 }, "ETH/USD": { "ap": 2500, "bp": 2500 }, "SOL/USD": { "ap": 150, "bp": 150 } } }))],
        "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 }, "ETH/USD": { "p": 2500 }, "SOL/USD": { "p": 150 } } }))],
        "POST /v2/orders": [accepted("OTX-1"), accepted("OTX-2"), accepted("OTX-3")],
        "GET /v2/account": [ok(json!({ "cash": "100000", "non_marginable_buying_power": "100000" }))],
        "GET /v2/positions": [ok(json!([position("BTCUSD", "0.01"), position("ETHUSD", "0.04")]))],
    });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }

async fn evaluate(o: &store::Opened, t: &ScriptedTransport, id: i64, when: DateTime<Utc>) -> Outcome {
    let prices = PriceCache::default();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let cx = TickContext { credential_version: model::credential_version(&o.primary, &bot).unwrap(), prices: &prices,
                           process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {} };
    rebalance::run(&o.primary, &venue(t), id, &FixedClock(when), &cx).await.unwrap()
}
fn pending(o: &store::Opened, id: i64) -> Value { model::load_bot(&o.primary, id).unwrap().transient.get("rebalance_pending").cloned().unwrap_or(Value::Null) }
fn posted(t: &ScriptedTransport) -> Vec<(String, String, String)> {
    t.posted_orders().iter().map(|o| (o["symbol"].as_str().unwrap().into(), o["side"].as_str().unwrap().into(),
                                       o["qty"].as_str().or(o["notional"].as_str()).unwrap().into())).collect()
}
fn events(o: &store::Opened) -> Vec<String> {
    o.primary.prepare("SELECT event FROM bot_activity_logs ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
fn rebalance_rows(o: &store::Opened) -> Vec<(i64, Option<i64>, i64, Option<String>)> {
    o.primary.prepare("SELECT status, external_status, side, external_id FROM transactions WHERE transaction_type = 'REBALANCE' ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}
/// The venue's answer for the bot's unresolved order, by the client order id its intent recorded.
fn landed(o: &store::Opened, id: i64, t: &ScriptedTransport, found: &Value) {
    let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    let mut body = found["body"].clone();
    body["client_order_id"] = json!(cl);
    t.reply("GET /v2/orders:by_client_order_id", 200, body);
}
fn sold_btc(filled: &str, quote_avg: Option<&str>, status: &str) -> Value { order("OTX-1", status, "BTC/USD", "sell", Some("0.00421875"), None, filled, quote_avg) }

#[tokio::test(flavor = "current_thread")]
async fn a_stopped_bot_sells_the_overweight_member_then_buys_the_underweight_one_on_later_polls() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({
        "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
        "GET /v2/orders/OTX-2": [order("OTX-2", "filled", "ETH/USD", "buy", None, Some("270"), "0.108", Some("2500"))],
    }));
    // 740 USD at 50/50: BTC is 270 over its 370, sold at the bid.
    assert!(matches!(evaluate(&o, &t, id, at(T0)).await, Outcome::Placed(_)));
    assert_eq!(posted(&t), vec![("BTC/USD".into(), "sell".into(), "0.004218750".into())]);
    let sell: i64 = one(&o, "SELECT id FROM transactions WHERE external_id = 'OTX-1'");
    assert_eq!(pending(&o, id), json!({ "phase": "selling", "sell_transaction_id": sell, "buy_transaction_id": null, "remaining_quote_amount": null, "buy_attempted": false }));
    assert_eq!(events(&o), vec!["rebalance_sell_placed"]);
    // The next fire: the sell filled, its 270 USD go to ETH, the most underweight.
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    assert_eq!(posted(&t)[1], ("ETH/USD".into(), "buy".into(), "270.00".into()));
    let buy: i64 = one(&o, "SELECT id FROM transactions WHERE external_id = 'OTX-2'");
    assert_eq!(pending(&o, id), json!({ "phase": "buying", "sell_transaction_id": sell, "buy_transaction_id": buy, "remaining_quote_amount": "270.0", "buy_attempted": true }));
    // And the one after: the buy filled the whole 270, the swap is done.
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(8)).await, Outcome::Skipped("completed"));
    assert_eq!(pending(&o, id), Value::Null);
    assert_eq!(rebalance_rows(&o), vec![(0, Some(2), 1, Some("OTX-1".into())), (0, Some(2), 0, Some("OTX-2".into()))]);
    assert_eq!(one::<i64>(&o, "SELECT status FROM bots"), 2, "still stopped");
    // Balanced now: nothing is due.
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(12)).await, Outcome::Skipped("not_due"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_drift_at_or_under_the_threshold_is_not_due() {
    let (_d, o, id, _, _) = swap_bot(stopped().with("rebalance_threshold", json!(0.365)));
    assert_eq!(evaluate(&o, &script(json!({})), id, at(T0)).await, Outcome::Skipped("not_due"), "640 / 740 − 0.5 = 0.3648… is under 0.365");
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.rebalance_threshold', 0.36)", []).unwrap();
    assert!(matches!(evaluate(&o, &script(json!({})), id, at(T0)).await, Outcome::Placed(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_on_the_sell_is_resolved_by_its_client_order_id_never_halted() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let found = order("OTX-9", "filled", "BTC/USD", "sell", Some("0.00421875"), None, "0.00421875", Some("64000"));
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, accepted("OTX-2")],
                           "GET /v2/orders/OTX-9": [found.clone()] }));
    assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("awaiting_reconciliation"));
    landed(&o, id, &t, &found);
    // The intent and its phase landed together: a crash here leaves both.
    let b = model::load_bot(&o.primary, id).unwrap();
    assert!(b.rust_placement().is_some());
    assert_eq!(pending(&o, id)["phase"], "selling");
    assert_eq!(rebalance_rows(&o), vec![]);
    // The next fire finds the order by its client id: recorded, carried forward, then resumed in the same evaluation.
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    assert_eq!(rebalance_rows(&o).iter().map(|r| r.3.clone().unwrap()).collect::<Vec<_>>(), vec!["OTX-9", "OTX-2"]);
    assert_eq!(pending(&o, id)["phase"], "buying");
    assert!(!events(&o).contains(&"rebalance_ambiguous".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_the_venue_never_saw_undoes_the_sell_phase() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, accepted("OTX-2")], "GET /v2/orders:by_client_order_id": [not_found()] }));
    evaluate(&o, &t, id, at(T0)).await;
    // Past Alpaca's absence margin the 404 is proof: nothing was sold, the phase goes, and the swap starts afresh.
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    assert_eq!(rebalance_rows(&o).len(), 1);
    assert_eq!(pending(&o, id)["phase"], "selling");
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_on_the_buy_keeps_what_is_owed() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "network": "post_send", "message": POST_SEND }, accepted("OTX-3")],
                           "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
                           "GET /v2/orders:by_client_order_id": [not_found()] }));
    evaluate(&o, &t, id, at(T0)).await;
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Skipped("awaiting_reconciliation"));
    assert_eq!(pending(&o, id)["buy_attempted"], true, "marked with the intent");
    // Proven absent: the attempt mark goes, the 270 stay owed, and the buy is placed again.
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(8)).await, Outcome::Placed(_)));
    assert_eq!(posted(&t).len(), 3);
    assert_eq!(pending(&o, id)["remaining_quote_amount"], "270.0");
}

#[tokio::test(flavor = "current_thread")]
async fn a_partly_filled_then_cancelled_sell_buys_with_what_it_sold() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.002", Some("64000"), "canceled")] }));
    evaluate(&o, &t, id, at(T0)).await;
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    assert_eq!(posted(&t)[1], ("ETH/USD".into(), "buy".into(), "128.00".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn a_closed_sell_without_proceeds_waits_for_them() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", None, "filled")] }));
    evaluate(&o, &t, id, at(T0)).await;
    // Closed, executed, with neither a quote figure nor a price (Rails' #realized_proceeds is nil); the venue's answer stays unreadable.
    o.primary.execute("UPDATE transactions SET external_status = 2, amount_exec = 0.00421875, price = NULL, quote_amount_exec = NULL WHERE external_id = 'OTX-1'", []).unwrap();
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Skipped("proceeds_unknown"));
    assert_eq!(pending(&o, id)["phase"], "selling");
    assert_eq!(posted(&t).len(), 1);
    // A restart before the figures arrive: the walk cannot value that sell, yet the bot is admitted awaiting them…
    let report = eligibility::check_install(&o.primary).unwrap();
    assert_eq!((report.problems, report.unreadable), (vec![], vec![]), "bot {id} is stopped: admitted, not listed as working");
    // …and the next fire reads them and buys.
    let late = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")], "POST /v2/orders": [accepted("OTX-2")] }));
    assert!(matches!(evaluate(&o, &late, id, at(T0) + Duration::hours(8)).await, Outcome::Placed(_)));
    assert_eq!(posted(&late)[0].1, "buy");
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancelled_sell_that_sold_nothing_ends_the_swap() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0", None, "canceled")] }));
    evaluate(&o, &t, id, at(T0)).await;
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Skipped("nothing_sold"));
    assert_eq!(pending(&o, id), Value::Null);
}

#[tokio::test(flavor = "current_thread")]
async fn a_partly_filled_buy_owes_only_the_remainder() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
                           "GET /v2/orders/OTX-2": [order("OTX-2", "canceled", "ETH/USD", "buy", None, Some("270"), "0.04", Some("2500"))] }));
    evaluate(&o, &t, id, at(T0)).await;
    evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await;
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(8)).await, Outcome::Placed(_)));
    assert_eq!(posted(&t)[2].0, "ETH/USD");
    assert_eq!(pending(&o, id)["remaining_quote_amount"], "170.0", "270 owed − 100 filled");
}

#[tokio::test(flavor = "current_thread")]
async fn the_owed_buy_is_capped_at_the_settled_cash() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
                           "GET /v2/account": [ok(json!({ "cash": "200", "non_marginable_buying_power": "100000" }))] }));
    evaluate(&o, &t, id, at(T0)).await;
    evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await;
    assert_eq!(posted(&t)[1], ("ETH/USD".into(), "buy".into(), "200.00".into()), "never the DCA leg's cash");
}

#[tokio::test(flavor = "current_thread")]
async fn an_owed_buy_under_the_floor_is_dust_said_once() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
                           "GET /v2/account": [ok(json!({ "cash": "0.5", "non_marginable_buying_power": "100000" }))] }));
    evaluate(&o, &t, id, at(T0)).await;
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Skipped("below_minimum"));
    assert_eq!(pending(&o, id), Value::Null);
    assert_eq!(events(&o), vec!["rebalance_sell_placed", "rebalance_dust"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_sell_under_the_floor_is_silent_but_flagged() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    o.primary.execute("UPDATE tickers SET minimum_quote_size = '500' WHERE ticker = 'BTC/USD'", []).unwrap();
    let t = script(json!({}));
    assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("below_minimum"));
    assert!(t.posted_orders().is_empty());
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.transient["rebalance_below_minimum_at"], "2026-09-01T12:30:00Z");
    assert!(b.transient.get("rebalance_pending").is_none() && b.rust_placement().is_none());
    assert!(events(&o).is_empty() && rebalance_rows(&o).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_definitive_rejection_halts_the_swap_without_a_row() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "POST /v2/orders": [{ "status": 403, "body": { "code": 40310000, "message": "insufficient balance for BTC" } }] }));
    assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("ambiguous"));
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.transient["rebalance_pending"]["phase"], "ambiguous");
    assert_eq!(b.transient["rust_rebalance_halted"], true);
    assert!(b.rust_placement().is_none());
    assert!(rebalance_rows(&o).is_empty());
    assert_eq!(events(&o), vec!["rebalance_ambiguous"]);
    // Terminal until the user resolves it: nothing more is asked.
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Skipped("ambiguous"));
    assert_eq!(t.posted_orders().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_send_that_never_left_undoes_the_phase_on_either_leg() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "POST /v2/orders": [{ "network": "pre_send", "message": PRE_SEND }] }));
    assert!(matches!(evaluate(&o, &t, id, at(T0)).await, Outcome::Failed(_)));
    assert_eq!(pending(&o, id), Value::Null, "the sell owes nothing");
    assert!(rebalance_rows(&o).is_empty());
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "network": "pre_send", "message": PRE_SEND }],
                           "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")] }));
    evaluate(&o, &t, id, at(T0)).await;
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Failed(_)));
    let p = pending(&o, id);
    assert_eq!((p["phase"].as_str(), p["buy_attempted"].as_bool(), p["remaining_quote_amount"].as_str()), (Some("buying"), Some(false), Some("270.0")));
}

#[tokio::test(flavor = "current_thread")]
async fn a_resting_buy_of_the_asset_on_the_account_blocks_its_sale() {
    let (_d, o, id, s, _) = swap_bot(stopped());
    // Another bot of the same account rests a BTC buy, recorded by name before rows stored their asset.
    let other = seed::insert_bot(&o.primary, &s, &stopped());
    o.primary.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, base, quote, transaction_type, error_messages, created_at, updated_at) \
                       VALUES (?1, ?2, 'OREST', 0, 1, 0, 1, 'BTC', 'USD', 'REGULAR', '[]', '2026-09-01 11:00:00', '2026-09-01 11:00:00')", rusqlite::params![other, s.exchange_id]).unwrap();
    let t = script(json!({}));
    assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("open_buy"));
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_with_the_rule_off_still_finishes_its_owed_buy() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")] }));
    evaluate(&o, &t, id, at(T0)).await;
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.rebalance_enabled', json('false'))", []).unwrap();
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    assert_eq!(posted(&t)[1].0, "ETH/USD");
}

#[tokio::test(flavor = "current_thread")]
async fn the_dca_tick_stands_down_between_the_legs() {
    let (_d, o, id, _, _) = swap_bot(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let t = script(json!({}));
    assert!(matches!(evaluate(&o, &t, id, at(T0)).await, Outcome::Placed(_)));
    let out = deltabadger::engine::tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:01Z")), &mut Default::default()).await.unwrap();
    assert!(matches!(out, deltabadger::engine::tick::TickOutcome::Done { placed: false }), "{out:?}");
    assert_eq!(t.posted_orders().len(), 1, "the sale's proceeds are the swap's");
}

// ---- T8: who the engine admits ----

#[test]
fn an_alpaca_rebalancing_bot_is_eligible_in_every_phase_this_engine_resumes() {
    let (_d, o, id, _, _) = swap_bot(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let check = || eligibility::check_install(&o.primary).unwrap();
    assert_eq!((check().eligible, check().problems), (vec![id], vec![]));
    for phase in ["selling", "buying"] {
        o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json(?1))",
                          [json!({ "phase": phase, "sell_transaction_id": null, "buy_transaction_id": null, "remaining_quote_amount": null, "buy_attempted": false }).to_string()]).unwrap();
        assert!(check().problems.is_empty(), "{phase}: {:?}", check().problems);
    }
    // A waiting REBALANCE row (the swap's resting sell) is read, not refused.
    o.primary.execute("UPDATE transactions SET external_status = 1, transaction_type = 'REBALANCE', side = 1 WHERE external_id = 'OHELD-1'", []).unwrap();
    assert!(check().problems.is_empty(), "{:?}", check().problems);
    // Rails' own ambiguous halt is Rails' to resolve; this engine's is admitted (idle).
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending.phase', 'ambiguous')", []).unwrap();
    assert!(check().problems.iter().any(|p| p.contains("ambiguous")), "{:?}", check().problems);
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_rebalance_halted', json('true'))", []).unwrap();
    assert!(check().problems.is_empty(), "{:?}", check().problems);
    // An abandoned REBALANCE leg stays refused (its fill is unknown).
    o.primary.execute("UPDATE transactions SET external_status = 4 WHERE external_id = 'OHELD-1'", []).unwrap();
    assert!(check().problems.iter().any(|p| p.contains("abandoned REBALANCE")), "{:?}", check().problems);
}

#[test]
fn rebalancing_off_alpaca_is_still_refused() {
    let dir = common::rails_install();
    let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("rebalance_enabled", json!(true)));
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains("rebalance_enabled")));
}

// ---- T7: the clock ----

struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(&self.0) }
}

#[tokio::test(flavor = "current_thread")]
async fn the_engine_rebalances_a_stopped_bot_at_each_fire_and_only_then() {
    let (d, o, id, _, _) = swap_bot(stopped());
    drop(o);
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    // The sell rests: its follow-up poll finds it open.
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0", None, "new")] }));
    let mut e = deltabadger::engine::run::Engine::new(o.primary, Factory(t.clone()), seed::cipher(), lock);
    // Started at 12:00: the first fire is 12:30, not the 08:30 one already past.
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T12:00:00Z"))).await.unwrap();
    assert!(t.posted_orders().is_empty());
    let wake = deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T12:29:00Z"))).await.unwrap();
    assert!(wake <= at("2026-09-01T12:30:00Z").timestamp_micros());
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T12:30:00Z"))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1);
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T12:31:00Z"))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1, "once per fire");
    let _ = id;
}

#[tokio::test(flavor = "current_thread")]
async fn a_reversal_awaiting_its_cancels_bars_the_swap() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_reversal_cancels', json('[{\"id\": 1, \"registered\": false, \"sent\": false}]'))", []).unwrap();
    assert!(deltabadger::engine::reversal::barred(&model::load_bot(&o.primary, id).unwrap()));
    let t = script(json!({}));
    assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("reversal_pending"));
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_handback_settles_a_lost_swap_reply_into_rails_own_pending_state() {
    let (d, o, id, _, _) = swap_bot(stopped());
    drop(o);
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    let o = store::open(&paths).unwrap();
    deltabadger::engine::handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at(T0)).unwrap();
    let found = order("OTX-9", "new", "BTC/USD", "sell", Some("0.00421875"), None, "0", None);
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
    assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("awaiting_reconciliation"));
    landed(&o, id, &t, &found);
    // The handback finds the order and settles it with its phase, in one transaction: Rails reads a swap it can resume.
    deltabadger::engine::handover::hand_back(&lock, &o, &Factory(t.clone()), &seed::cipher(), &FixedClock(at(T0) + Duration::minutes(1))).await.unwrap();
    let sell: i64 = one(&o, "SELECT id FROM transactions WHERE external_id = 'OTX-9'");
    assert_eq!(pending(&o, id), json!({ "phase": "selling", "sell_transaction_id": sell, "buy_transaction_id": null, "remaining_quote_amount": null, "buy_attempted": false }));
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
    assert_eq!(one::<String>(&o, "SELECT transaction_type FROM transactions WHERE id = (SELECT max(id) FROM transactions)"), "REBALANCE");
}
