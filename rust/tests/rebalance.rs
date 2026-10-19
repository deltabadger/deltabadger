//! Slice 6b: threshold rebalancing (Bot::Rebalancer, Bot::RebalanceJob, Bot::EvaluateRebalancersJob) on Alpaca, and (plan CB T4) on Kraken, Binance and KuCoin.
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

async fn evaluate(o: &store::Opened, t: &ScriptedTransport, id: i64, when: DateTime<Utc>) -> Outcome { evaluate_on(o, &venue(t), id, when).await }
async fn evaluate_on<V: deltabadger::venue::Venue>(o: &store::Opened, v: &V, id: i64, when: DateTime<Utc>) -> Outcome {
    let prices = PriceCache::default();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let cx = TickContext { credential_version: model::credential_version(&o.primary, &bot).unwrap(), prices: &prices,
                           process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    rebalance::run(&o.primary, v, id, &FixedClock(when), &cx).await.unwrap()
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

/// The buy's twin: closed, executed, its price and quote figure unknown. Before the fix the walk refused it and the bot was
/// unreadable at the install, so no fire ever re-read the row (Rails' #resume_buying! refreshes it first).
#[tokio::test(flavor = "current_thread")]
async fn a_closed_buy_without_its_fill_figures_waits_for_them() {
    let (_d, o, id, _, _) = swap_bot(stopped());
    let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
                           "GET /v2/orders/OTX-2": [order("OTX-2", "filled", "ETH/USD", "buy", None, Some("270"), "0.108", None)] }));
    evaluate(&o, &t, id, at(T0)).await;
    assert!(matches!(evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    o.primary.execute("UPDATE transactions SET external_status = 2, amount_exec = 0.108, price = NULL, quote_amount_exec = NULL WHERE external_id = 'OTX-2'", []).unwrap();
    assert_eq!(evaluate(&o, &t, id, at(T0) + Duration::hours(8)).await, Outcome::Skipped("fill_unknown"));
    assert_eq!(pending(&o, id)["phase"], "buying");
    let report = eligibility::check_install(&o.primary).unwrap();
    assert_eq!((report.problems, report.unreadable), (vec![], vec![]), "admitted awaiting the buy's figures");
    let late = script(json!({ "GET /v2/orders/OTX-2": [order("OTX-2", "filled", "ETH/USD", "buy", None, Some("270"), "0.108", Some("2500"))] }));
    assert_eq!(evaluate(&o, &late, id, at(T0) + Duration::hours(12)).await, Outcome::Skipped("completed"));
    assert_eq!(pending(&o, id), Value::Null);
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

// ---- a stop or a buy⇄sell reverse racing the swap (another connection: the web, the MCP) ----

use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
/// The scripted venue, with `sql` committed by another connection the moment `path` is asked (as tests/placement.rs's
/// StopDuringDispatch commits a STOP while the order is in flight).
struct During { script: ScriptedTransport, db: rusqlite::Connection, path: &'static str, sql: String }
impl Transport for During {
    async fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if req.path == self.path { self.db.execute_batch(&self.sql).unwrap(); }
        self.script.send(req).await
    }
}
fn during(o: &store::Opened, t: &ScriptedTransport, path: &'static str, sql: String) -> AlpacaVenue<During> {
    AlpacaVenue::new(During { script: t.clone(), db: rusqlite::Connection::open(o.primary.path().unwrap()).unwrap(), path, sql }, Urls::for_passphrase(Some("paper")))
}
/// The web's STOP of a working bot, and a reverse to selling that captured a resting order whose cancel is not recorded yet.
fn stop(id: i64) -> String { format!("UPDATE bots SET status = 2, transient_data = json_set(transient_data, '$.run_generation', 1) WHERE id = {id}") }
fn reverse(id: i64) -> String {
    format!("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling'), \
             transient_data = json_set(transient_data, '$.rust_reversal_cancels', json('[{{\"id\": 1, \"registered\": false, \"sent\": false}}]')) WHERE id = {id}")
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_or_a_reverse_while_the_swaps_sell_is_in_flight_still_records_it() {
    for (name, sql) in [("stop", stop as fn(i64) -> String), ("reverse", reverse)] {
        let (_d, o, id, _, _) = swap_bot(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        let t = script(json!({ "GET /v2/orders/OTX-1": [sold_btc("0", None, "new")] }));
        // The dispatch mark is committed before the request leaves; the other write lands while the reply is on its way.
        let out = evaluate_on(&o, &during(&o, &t, "/v2/orders", sql(id)), id, at(T0)).await;
        assert!(matches!(out, Outcome::Placed(_)), "{name}: {out:?}");
        let sell: i64 = one(&o, "SELECT id FROM transactions WHERE external_id = 'OTX-1'");
        assert_eq!(rebalance_rows(&o), vec![(0, Some(0), 1, Some("OTX-1".into()))], "{name}: the sold order is a fact");
        assert_eq!(pending(&o, id), json!({ "phase": "selling", "sell_transaction_id": sell, "buy_transaction_id": null, "remaining_quote_amount": null, "buy_attempted": false }), "{name}");
        assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none(), "{name}");
        // The next fire: a stopped bot resumes its swap (Rails rebalances stopped bots); a reversal's barrier holds it.
        let next = evaluate(&o, &t, id, at(T0) + Duration::hours(4)).await;
        assert_eq!(next, Outcome::Skipped(if name == "stop" { "waiting_for_sell" } else { "reversal_pending" }), "{name}");
        assert_eq!(t.posted_orders().len(), 1, "{name}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_reverse_while_the_swap_is_sized_sends_nothing_and_leaves_no_phase_but_a_stop_does_not_hold_it() {
    // Positions are the last read before the intent (the sell's free-balance cap).
    let (_d, o, id, _, _) = swap_bot(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let t = script(json!({}));
    assert_eq!(evaluate_on(&o, &during(&o, &t, "/v2/positions", reverse(id)), id, at(T0)).await, Outcome::Skipped("changed"));
    assert!(t.posted_orders().is_empty());
    assert_eq!(pending(&o, id), Value::Null);
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
    let (_d, o, id, _, _) = swap_bot(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let t = script(json!({}));
    assert!(matches!(evaluate_on(&o, &during(&o, &t, "/v2/positions", stop(id)), id, at(T0)).await, Outcome::Placed(_)));
    assert_eq!(t.posted_orders().len(), 1);
    assert_eq!(pending(&o, id)["phase"], "selling");
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
fn a_stopped_rebalancing_bot_outside_the_slice_is_refused() {
    // Rebalancing runs on stopped bots, so a stopped one is this engine's work: whatever else it carries must be supported.
    let (_d, o, _, _, _) = swap_bot(stopped().with("weighting", json!("market_cap")));
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains("weighting market_cap")));
    // Sliders off 100% are not such a thing: Rails' rebalance_targets reads them as no targets (the grid's `unbalanced`).
    let (_d, o, _, _, _) = swap_bot(stopped());
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json((SELECT json_group_object(key, 0.6) FROM json_each(bots.settings, '$.allocations'))))", []).unwrap();
    assert_eq!(eligibility::check_install(&o.primary).unwrap().problems, Vec::<String>::new());
}

#[test]
fn rebalancing_on_kraken_is_admitted() {
    // Plan CB T4: Bot::Rebalancer names no venue; a Kraken rebalancer is this engine's.
    let dir = common::rails_install();
    let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("rebalance_enabled", json!(true)));
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
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
async fn sliders_saved_off_100_mid_swap_still_let_the_owed_buy_land_and_settle() {
    // Rails' rebalance_targets: `rebalance_pending? || allocations_balanced?`. The owed buy is placed, polled and settled
    // with the sliders at 120%; its outstanding row must not turn the stopped basket into an install-wide refusal.
    let (d, o, id, _, _) = swap_bot(stopped());
    drop(o);
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    let t = script(json!({
        "GET /v2/orders/OTX-1": [sold_btc("0.00421875", Some("64000"), "filled")],
        "GET /v2/orders/OTX-2": [order("OTX-2", "filled", "ETH/USD", "buy", None, Some("270"), "0.108", Some("2500"))],
    }));
    let primary = store::open(&paths).unwrap().primary;
    let mut e = deltabadger::engine::run::Engine::new(o.primary, Factory(t.clone()), seed::cipher(), lock);
    macro_rules! step { ($s:expr) => { deltabadger::engine::run::step(&mut e, &FixedClock(at($s))) } }
    step!("2026-09-01T12:00:00Z").await.unwrap();
    step!("2026-09-01T12:30:00Z").await.unwrap();
    assert_eq!(t.posted_orders().len(), 1);
    primary.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json((SELECT json_group_object(key, 0.6) FROM json_each(bots.settings, '$.allocations'))))", []).unwrap();
    step!("2026-09-01T12:31:00Z").await.unwrap();
    step!("2026-09-01T16:30:00Z").await.unwrap();
    assert_eq!(posted(&t)[1], ("ETH/USD".into(), "buy".into(), "270.00".into()));
    assert_eq!(eligibility::check_install(&primary).unwrap().problems, Vec::<String>::new());
    step!("2026-09-01T16:31:00Z").await.unwrap();
    assert_eq!(primary.query_row("SELECT external_status FROM transactions WHERE external_id = 'OTX-2'", [], |r| r.get::<_, i64>(0)).unwrap(), 2, "the owed buy was polled");
    step!("2026-09-01T20:30:00Z").await.unwrap();
    assert_eq!(model::load_bot(&primary, id).unwrap().transient.get("rebalance_pending").cloned().unwrap_or(Value::Null), Value::Null, "the swap completed");
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

// ---- an allocation edit racing the swap's reads: the pass decides from what it read before its first await ----

/// A stopped 50/50 AAA/BBB stock basket holding 9 AAA and 1 BBB at 100: AAA sits 40 points over (sell 4). At 90/10 it is
/// on target. An open clock, both positions, every quote at 100.
fn stock_swap() -> (tempfile::TempDir, store::Opened, i64, [i64; 2], ScriptedTransport) {
    let (d, o, s) = common::install_alpaca();
    let (aaa, _) = seed::add_alpaca_stock(&o.primary, &s, "AAA");
    let (bbb, _) = seed::add_alpaca_stock(&o.primary, &s, "BBB");
    o.primary.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) \
                       SELECT exchange_id, base_asset_id, 1, created_at, updated_at FROM tickers WHERE ticker IN ('AAA', 'BBB')", []).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &stopped().with("rebalance_enabled", json!(true)).weights(&[(aaa, 0.5), (bbb, 0.5)]));
    deltabadger::engine::basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    for (asset, sym, qty, quote) in [(aaa, "AAA", "9", "900"), (bbb, "BBB", "1", "100")] {
        seed::insert_stock_tx(&o.primary, &s, id, asset, sym, &seed::TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("held-{sym}")), order_type: 0,
            amount: Some(qty), quote_amount: Some(quote), price: Some("100"), amount_exec: Some(qty), quote_amount_exec: Some(quote), created_at: "2026-09-01 10:00:01".into() });
    }
    seed::fresh_stock_jobs(&o.primary, at(T0));
    let mut m = json!({
        "GET /v2/clock": [ok(json!({ "timestamp": "2026-09-01T08:30:00-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" }))],
        "POST /v2/orders": [accepted("OTX-1")],
        "GET /v2/account": [ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" }))],
        "GET /v2/positions": [ok(json!([{ "symbol": "AAA", "asset_class": "us_equity", "qty": "9", "qty_available": "9" },
                                        { "symbol": "BBB", "asset_class": "us_equity", "qty": "1", "qty_available": "1" }]))],
        "GET /v2/stocks/snapshots": [ok(json!({ "AAA": { "latestTrade": { "p": 100 } }, "BBB": { "latestTrade": { "p": 100 } } }))],
        "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": {} }))],
    });
    for sym in ["AAA", "BBB"] { m[format!("GET /v2/stocks/{sym}/quotes/latest")] = json!([ok(json!({ "quote": { "ap": 100, "bp": 100 } }))]); }
    (d, o, id, [aaa, bbb], ScriptedTransport::from_script(&m))
}
/// The web's save of a 90/10 split: the settings and the member rows it derives.
fn edit_to_90_10(id: i64, [aaa, bbb]: [i64; 2]) -> String {
    format!("UPDATE bots SET settings = json_set(settings, '$.allocations', json('{{\"{aaa}\": 0.9, \"{bbb}\": 0.1}}')), settings_changed_at = '2026-09-01 12:29:59' WHERE id = {id}; \
             UPDATE bot_index_assets SET target_allocation = CASE asset_id WHEN {aaa} THEN 0.9 ELSE 0.1 END WHERE bot_id = {id};")
}
fn targets(o: &store::Opened, id: i64) -> Vec<f64> {
    o.primary.prepare("SELECT target_allocation FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1 ORDER BY id").unwrap()
        .query_map([id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// Codex P1: the 50/50 → 90/10 edit lands while /v2/clock is asked. The composition refresh wrote the old 50/50 back over
/// the new members, the fence re-read the new settings and passed, and the old 50/50 sell went out though 90/10 is on
/// target. Then a positions read and a price read: each places nothing and leaves the edit as the user saved it.
#[tokio::test(flavor = "current_thread")]
async fn an_allocation_edit_during_any_of_the_swaps_reads_places_nothing_and_keeps_the_edit() {
    let (_d, o, id, _, t) = stock_swap();
    assert!(matches!(evaluate(&o, &t, id, at(T0)).await, Outcome::Placed(_)), "unedited, the 50/50 sells AAA");
    assert_eq!(posted(&t), vec![("AAA".into(), "sell".into(), "4.000000000".into())]);
    for path in ["/v2/clock", "/v2/positions", "/v2/stocks/snapshots", "/v2/stocks/AAA/quotes/latest"] {
        let (_d, o, id, assets, t) = stock_swap();
        let out = evaluate_on(&o, &during(&o, &t, path, edit_to_90_10(id, assets)), id, at(T0)).await;
        assert!(!matches!(out, Outcome::Placed(_)), "{path}: {out:?}");
        assert!(t.posted_orders().is_empty(), "{path}");
        assert_eq!(pending(&o, id), Value::Null, "{path}");
        assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none(), "{path}");
        let bot = model::load_bot(&o.primary, id).unwrap();
        assert_eq!(bot.settings["allocations"], json!({ assets[0].to_string(): 0.9, assets[1].to_string(): 0.1 }), "{path}");
        assert_eq!(targets(&o, id), vec![0.9, 0.1], "{path}: the members the edit wrote");
        // The next fire decides from the edit: on target, nothing is due.
        assert_eq!(evaluate(&o, &t, id, at(T0)).await, Outcome::Skipped("not_due"), "{path}");
    }
}

// ---- plan CB T4: rebalancing on Kraken, Binance and KuCoin ----

/// A stopped 50/50 BTC/ETH basket on a crypto venue (quote EUR, as the seed has it), rebalancing on, holding 0.01 BTC (500)
/// and 0.04 ETH (100) at 50,000 and 2,500: BTC sits 33 points over, and the swap sells 0.004 BTC for 200.
fn crypto_swap_bot(venue: &str, transient: Value) -> (tempfile::TempDir, i64, i64) {
    let (d, o, s) = common::install();
    let (class, name, btc, eth) = match venue {
        "binance" => ("Exchanges::Binance", "Binance", "BTCEUR", "ETHEUR"), "kucoin" => ("Exchanges::Kucoin", "KuCoin", "BTC-EUR", "ETH-EUR"),
        _ => ("Exchanges::Kraken", "Kraken", "XBTEUR", "ETHEUR"),
    };
    o.primary.execute("UPDATE exchanges SET type = ?1, name = ?2", [class, name]).unwrap();
    if venue != "kraken" { o.primary.execute("UPDATE tickers SET ticker = ?1, base = 'BTC'", [btc]).unwrap(); }
    o.primary.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('ethereum', 'ETH', 'ETH', 'Cryptocurrency', '2026-01-01', '2026-01-01')", []).unwrap();
    let eth_id = o.primary.last_insert_rowid();
    o.primary.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                       minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
                       VALUES (?1, ?2, 'ETH', 'EUR', ?3, ?4, 8, 5, 2, '0.0001', '0.5', 1, 1, '2026-01-01', '2026-01-01')",
                      rusqlite::params![s.exchange_id, eth, eth_id, s.quote]).unwrap();
    let mut spec = stopped().with("rebalance_enabled", json!(true)).weights(&[(s.btc, 0.5), (eth_id, 0.5)]);
    spec.transient = transient;
    let id = seed::insert_bot(&o.primary, &s, &spec);
    deltabadger::engine::basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    for (asset, qty, price, quote, n) in [(s.btc, "0.01", "50000", "500", 1), (eth_id, "0.04", "2500", "100", 2)] {
        seed::insert_row(&o.primary, &s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty, "amount_exec": qty,
            "quote_amount": quote, "quote_amount_exec": quote, "price": price, "created_at": "2026-09-01 10:00:01" }));
    }
    let exchange = s.exchange_id;
    drop(o);
    (d, id, exchange)
}

/// One venue's answers for a swap: both members at one price for bid, ask and last (the bulk read and each book), the free
/// balances, and `over` replacing whole keys.
fn crypto_script(venue: &str, over: Value) -> ScriptedTransport {
    let mut m = match venue {
        "binance" => {
            let book = |sym: &str, p: &str| ok(json!({ "symbol": sym, "bidPrice": p, "bidQty": "1", "askPrice": p, "askQty": "1" }));
            json!({ "GET /api/v3/ticker/price": [ok(json!([{ "symbol": "BTCEUR", "price": "50000" }, { "symbol": "ETHEUR", "price": "2500" }]))],
                    "GET /api/v3/ticker/bookTicker?symbol=BTCEUR": [book("BTCEUR", "50000")], "GET /api/v3/ticker/bookTicker?symbol=ETHEUR": [book("ETHEUR", "2500")],
                    "GET /api/v3/account": [ok(json!({ "balances": [{ "asset": "BTC", "free": "0.01", "locked": "0" }, { "asset": "ETH", "free": "0.04", "locked": "0" },
                                                                     { "asset": "EUR", "free": "100000", "locked": "0" }] }))] })
        }
        "kucoin" => {
            let book = |p: &str| ok(json!({ "code": "200000", "data": { "time": 1, "sequence": "1", "bids": [[p, "1"]], "asks": [[p, "1"]] } }));
            json!({ "GET /api/v1/market/allTickers": [ok(json!({ "code": "200000", "data": { "time": 1, "ticker": [{ "symbol": "BTC-EUR", "last": "50000" }, { "symbol": "ETH-EUR", "last": "2500" }] } }))],
                    "GET /api/v1/market/orderbook/level2_20?symbol=BTC-EUR": [book("50000")], "GET /api/v1/market/orderbook/level2_20?symbol=ETH-EUR": [book("2500")],
                    "GET /api/v1/accounts": [ok(json!({ "code": "200000", "data": [
                        { "id": "b", "currency": "BTC", "type": "trade", "balance": "0.01", "available": "0.01", "holds": "0" },
                        { "id": "e", "currency": "ETH", "type": "trade", "balance": "0.04", "available": "0.04", "holds": "0" },
                        { "id": "q", "currency": "EUR", "type": "trade", "balance": "100000", "available": "100000", "holds": "0" }] }))] })
        }
        _ => {
            let entry = |p: &str| json!({ "a": [p, "1", "1.000"], "b": [p, "1", "1.000"], "c": [p, "0.1"] });
            json!({ "GET /0/public/Ticker": [ok(json!({ "error": [], "result": { "XXBTZEUR": entry("50000"), "ETHEUR": entry("2500") } }))],
                    "GET /0/public/Ticker?pair=XBTEUR": [ok(json!({ "error": [], "result": { "XXBTZEUR": entry("50000") } }))],
                    "GET /0/public/Ticker?pair=ETHEUR": [ok(json!({ "error": [], "result": { "ETHEUR": entry("2500") } }))],
                    "POST /0/private/BalanceEx": [ok(json!({ "error": [], "result": { "XXBT": { "balance": "0.01", "hold_trade": "0" },
                        "XETH": { "balance": "0.04", "hold_trade": "0" }, "ZEUR": { "balance": "100000", "hold_trade": "0" } } }))],
                    "POST /0/private/AddOrder": [ok(json!({ "error": [], "result": { "txid": ["OTX-1"] } })), ok(json!({ "error": [], "result": { "txid": ["OTX-2"] } }))] })
        }
    };
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}

/// The venue over a script, with the install's pairs (Binance and KuCoin read them).
enum Crypto { Kraken(deltabadger::venue::kraken::KrakenVenue<ScriptedTransport>), Binance(deltabadger::venue::binance::BinanceVenue<ScriptedTransport>),
              Kucoin(deltabadger::venue::kucoin::KucoinVenue<ScriptedTransport>) }
fn crypto_venue(venue: &str, o: &store::Opened, exchange: i64, t: &ScriptedTransport) -> Crypto {
    use deltabadger::venue::{binance, kucoin};
    match venue {
        "binance" => Crypto::Binance(binance::BinanceVenue::new(t.clone(), binance::URL, "s").with_pairs(binance::Pairs::load(&o.primary, exchange).unwrap())),
        "kucoin" => Crypto::Kucoin(kucoin::KucoinVenue::new(t.clone(), kucoin::URL).with_pairs(binance::Pairs::load(&o.primary, exchange).unwrap())),
        _ => Crypto::Kraken(deltabadger::venue::kraken::KrakenVenue::new(t.clone())),
    }
}
async fn crypto_fire(venue: &str, o: &store::Opened, exchange: i64, t: &ScriptedTransport, id: i64, when: DateTime<Utc>) -> Outcome {
    match crypto_venue(venue, o, exchange, t) { Crypto::Kraken(v) => evaluate_on(o, &v, id, when).await, Crypto::Binance(v) => evaluate_on(o, &v, id, when).await,
                                               Crypto::Kucoin(v) => evaluate_on(o, &v, id, when).await }
}
/// The orders a script's venue was asked to place, as (pair, side, quote amount or base volume).
fn crypto_posts(venue: &str, t: &ScriptedTransport) -> Vec<(String, String, String)> {
    t.requests().into_iter().filter(|r| r.method == "POST" && ["/0/private/AddOrder", "/api/v3/order", "/api/v1/orders"].contains(&r.path.as_str())).map(|r| {
        let q = |k: &str| r.query.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone()).unwrap_or_default();
        let b = r.body.clone().unwrap_or_default();
        let s = |k: &str| b[k].as_str().unwrap_or_default().to_string();
        match venue {
            "binance" => (q("symbol"), q("side").to_lowercase(), if q("quoteOrderQty").is_empty() { q("quantity") } else { q("quoteOrderQty") }),
            "kucoin" => (s("symbol"), s("side"), if s("funds").is_empty() { s("size") } else { s("funds") }),
            _ => (s("pair"), s("type"), s("volume")),
        }
    }).collect()
}

/// Rails' halt resolution (Bots::RebalanceResolutionsController, R0's Bot::Rebalanceable#resolve_rebalance_halt!) on the
/// handed-back install, once the controller's refresh has found no rebalance order still waiting.
fn rails_resolves_the_halt(dir: &std::path::Path, bot_id: i64) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let boot = tempfile::tempdir().unwrap();
    let mut cmd = std::process::Command::new(root.join("bin/rails"));
    cmd.current_dir(root).args(["runner", &format!("Bot.find({bot_id}).resolve_rebalance_halt!")]).env_remove("DATABASE_URL")
        .env("APP_ROOT_URL", "http://localhost:3000").env("SECRET_KEY_BASE", "engine-test-secret")
        .env("PRIMARY_DATABASE_URL", format!("sqlite3:{}/production.sqlite3", dir.display()))
        .env("QUEUE_DATABASE_URL", format!("sqlite3:{}/production_queue.sqlite3", dir.display()));
    for db in ["cache", "cable"] { cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", boot.path().display())); }
    let run = cmd.output().expect("bin/rails runs");
    assert!(run.status.success(), "the Rails resolution failed:\n{}", String::from_utf8_lossy(&run.stderr));
}

#[tokio::test(flavor = "current_thread")]
async fn a_stopped_kraken_basket_sells_the_overweight_member_then_buys_with_its_gross_proceeds() {
    let (d, id, exchange) = crypto_swap_bot("kraken", json!({}));
    let o = store::open(&store::Paths::from_env(&|_| None, d.path())).unwrap();
    let t = crypto_script("kraken", json!({}));
    assert!(matches!(crypto_fire("kraken", &o, exchange, &t, id, at(T0)).await, Outcome::Placed(_)));
    assert_eq!(crypto_posts("kraken", &t), vec![("XBTEUR".into(), "sell".into(), "0.004".into())]);
    // Kraken's `cost` is gross: the 0.5 fee is not taken off what the buy is owed (Q1, as Rails has it).
    let sold = ok(json!({ "error": [], "result": { "OTX-1": { "status": "closed", "vol": "0.004", "vol_exec": "0.004", "cost": "200", "fee": "0.5", "price": "50000",
                          "oflags": "", "descr": { "pair": "XBTEUR", "type": "sell", "ordertype": "market", "price": "0" } } } }));
    let t = crypto_script("kraken", json!({ "POST /0/private/QueryOrders": [sold], "POST /0/private/AddOrder": [ok(json!({ "error": [], "result": { "txid": ["OTX-2"] } }))] }));
    assert!(matches!(crypto_fire("kraken", &o, exchange, &t, id, at(T0) + Duration::hours(4)).await, Outcome::Placed(_)));
    assert_eq!(crypto_posts("kraken", &t), vec![("ETHEUR".into(), "buy".into(), "0.08".into())], "200 of ETH at 2,500");
    assert_eq!(pending(&o, id)["remaining_quote_amount"], "200.0");
}

/// The venue's answer for the recovered order: the sell filled whole (0.004 BTC for 200), the buy cancelled after 80 of the
/// 200 owed (0.032 ETH).
fn recovered_read(venue: &str, pair: &str, ext: &str, leg: &str) -> Value {
    let (side, filled, quote, price) = if leg == "sell" { ("SELL", "0.004", "200", "50000") } else { ("BUY", "0.032", "80", "2500") };
    if venue == "binance" {
        json!({ "GET /api/v3/order": [ok(json!({ "symbol": pair, "orderId": 28457, "orderListId": -1, "clientOrderId": "x", "price": "0", "origQty": "0",
                                                 "executedQty": filled, "cummulativeQuoteQty": quote, "status": if leg == "sell" { "FILLED" } else { "CANCELED" },
                                                 "timeInForce": "GTC", "type": "MARKET", "side": side, "origQuoteOrderQty": "0", "time": 1 }))],
                "GET /api/v3/myTrades": [ok(json!([{ "symbol": pair, "id": 1, "orderId": 28457, "orderListId": -1, "price": price, "qty": filled, "quoteQty": quote,
                                                    "commission": "0", "commissionAsset": "BNB", "time": 1, "isBuyer": leg == "buy", "isMaker": false, "isBestMatch": true }]))] })
    } else {
        json!({ format!("GET /api/v1/orders/{ext}"): [ok(json!({ "code": "200000", "data": { "id": ext, "symbol": pair, "opType": "DEAL", "type": "market",
            "side": side.to_lowercase(), "price": "0", "size": "0", "funds": "0", "dealSize": filled, "dealFunds": quote, "fee": "0", "feeCurrency": "EUR",
            "clientOid": "x", "isActive": false, "cancelExist": leg == "buy", "cancelReason": "", "createdAt": 1 } }))] })
    }
}

/// Binance and KuCoin cannot prove an unfound order absent. A swap whose placement reply was lost halts once the absence
/// window has passed; the operator settles the intent (`deltabadger resolve-placement`), the engine hands back, the user
/// resolves the halt in Rails (R0), and the next fire after a re-takeover follows Rails' resolution: a recovered sell's
/// proceeds are bought once, a recovered buy's fill comes off what is owed, and the lost order is never sent again.
#[tokio::test(flavor = "current_thread")]
async fn an_unfound_crypto_swap_order_halts_and_its_resolution_never_submits_it_twice() {
    use deltabadger::engine::placement::{resolve_by_operator, OperatorResolution};
    for venue in ["binance", "kucoin"] {
        for (leg, placed) in [("sell", true), ("sell", false), ("buy", true)] {
            let label = format!("{venue} {leg} placed={placed}");
            let owed = json!({ "rebalance_pending": { "phase": "buying", "sell_transaction_id": null, "buy_transaction_id": null,
                                                      "remaining_quote_amount": "200.0", "buy_attempted": false } });
            let (d, id, exchange) = crypto_swap_bot(venue, if leg == "buy" { owed } else { json!({}) });
            let paths = store::Paths::from_env(&|_| None, d.path());
            let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
            let o = store::open(&paths).unwrap();
            deltabadger::engine::handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at(T0)).unwrap();
            let place = if venue == "binance" { "POST /api/v3/order" } else { "POST /api/v1/orders" };
            let lost = crypto_script(venue, json!({ place: [{ "network": "post_send", "message": POST_SEND }] }));
            crypto_fire(venue, &o, exchange, &lost, id, at(T0)).await;
            assert_eq!(crypto_posts(venue, &lost).len(), 1, "{label}");
            assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_some(), "{label}: the intent is kept");
            // A day on, the venue still cannot find it: the swap halts, nothing is sent.
            let unfound = crypto_script(venue, json!({ "GET /api/v3/order": [{ "status": 400, "body": { "code": -2013, "msg": "Order does not exist." } }],
                "GET /api/v1/order/client-order/".to_string() + model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap():
                    [{ "status": 404, "body": { "code": "400100", "msg": "order not exist." } }],
                "GET /api/v1/orders": [ok(json!({ "code": "200000", "data": { "currentPage": 1, "pageSize": 500, "totalNum": 0, "totalPage": 0, "items": [] } }))] }));
            assert_eq!(crypto_fire(venue, &o, exchange, &unfound, id, at(T0) + Duration::days(1)).await, Outcome::Skipped("ambiguous"), "{label}");
            assert!(crypto_posts(venue, &unfound).is_empty(), "{label}");
            assert_eq!(pending(&o, id)["phase"], "ambiguous", "{label}");
            // The operator checked the venue.
            let (pair, ext) = match (venue, leg) {
                ("binance", "sell") => ("BTCEUR", "BTCEUR-28457".to_string()), ("binance", _) => ("ETHEUR", "ETHEUR-28457".to_string()),
                (_, "sell") => ("BTC-EUR", "6716f0a2c1b6d10007a3b1c1".to_string()), _ => ("ETH-EUR", "6716f0a2c1b6d10007a3b1c1".to_string()),
            };
            let resolution = if placed { OperatorResolution::Placed(ext.clone()) } else { OperatorResolution::NotPlaced };
            resolve_by_operator(&o.primary, id, resolution, at(T0) + Duration::days(1)).unwrap();
            let row: Option<i64> = o.primary.query_row("SELECT id FROM transactions WHERE external_id = ?1", [&ext], |r| r.get(0)).ok();
            assert_eq!(row.is_some(), placed, "{label}");
            if let Some(row) = row {
                let key = if leg == "sell" { "sell_transaction_id" } else { "buy_transaction_id" };
                assert_eq!(pending(&o, id)[key], row, "{label}: the halt names the recovered order");
                // The engine's follow-up poll reads it settled: the sell filled whole (200), the buy 80 of the 200 owed.
                let read = recovered_read(venue, pair, &ext, leg);
                let poll = crypto_script(venue, read);
                match crypto_venue(venue, &o, exchange, &poll) {
                    Crypto::Binance(v) => { deltabadger::engine::polling::follow_up(&o.primary, &v, id, row, at(T0) + Duration::days(1)).await.unwrap(); }
                    Crypto::Kucoin(v) => { deltabadger::engine::polling::follow_up(&o.primary, &v, id, row, at(T0) + Duration::days(1)).await.unwrap(); }
                    Crypto::Kraken(_) => unreachable!(),
                }
            }
            deltabadger::engine::handover::hand_back(&lock, &o, &Factory(script(json!({}))), &seed::cipher(), &FixedClock(at(T0) + Duration::days(1))).await.unwrap();
            drop(o);
            drop(lock);
            rails_resolves_the_halt(d.path(), id);
            let lock = deltabadger::lease::lock(&paths, at(T0) + Duration::days(2)).unwrap();
            let o = store::open(&paths).unwrap();
            deltabadger::engine::handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at(T0) + Duration::days(2)).unwrap();
            let after = pending(&o, id);
            // The next fire: one further order at most, and never the lost one again.
            let mut over = json!({ place: [ok(if venue == "binance" { json!({ "symbol": "ETHEUR", "orderId": 30001, "orderListId": -1, "transactTime": 1 }) }
                                              else { json!({ "code": "200000", "data": { "orderId": "6716f0a2c1b6d10007a3b1d9" } }) })] });
            if placed {
                // resume_* refreshes the recovered order first: it reads as the poll left it.
                let read = recovered_read(venue, pair, &ext, leg);
                for (k, v) in read.as_object().unwrap() { over[k] = v.clone(); }
            }
            let reads = crypto_script(venue, over);
            crypto_fire(venue, &o, exchange, &reads, id, at(T0) + Duration::days(2)).await;
            let posts = crypto_posts(venue, &reads);
            // A buy's size in the quote: its quote amount, or its base volume at ETH's 2,500.
            let spent = |p: &(String, String, String)| { let n: f64 = p.2.parse().unwrap(); if n < 1.0 { n * 2500.0 } else { n } };
            match (leg, placed) {
                // R0 put the recovered sell back: its 200 of proceeds are owed, and bought once.
                ("sell", true) => {
                    assert_eq!(after["phase"], "selling", "{label}: {after}");
                    assert_eq!(posts.len(), 1, "{label}: {posts:?}");
                    assert_eq!((posts[0].1.as_str(), spent(&posts[0])), ("buy", 200.0), "{label}: {posts:?}");
                }
                // Attested never placed: Rails cleared the swap; the fire decides afresh (a new sell, not the lost one).
                ("sell", false) => {
                    assert_eq!(after, Value::Null, "{label}");
                    assert!(posts.len() <= 1 && posts.iter().all(|p| p.1 == "sell"), "{label}: {posts:?}");
                }
                // R0 kept the recovered buy: 80 of the 200 filled, so only the 120 still owed is bought.
                _ => {
                    assert_eq!((after["phase"].as_str(), after["buy_attempted"].as_bool()), (Some("buying"), Some(true)), "{label}: {after}");
                    assert_eq!(posts.len(), 1, "{label}: {posts:?}");
                    assert_eq!((posts[0].1.as_str(), spent(&posts[0])), ("buy", 120.0), "{label}: the remainder, not the 200 again: {posts:?}");
                }
            }
        }
    }
}

/// Bot::Rebalancer#waiting_buy_blocks_sell? reads every bot on the account: another Kraken basket's resting BTC buy (on the
/// same key) stands the swap's BTC sale down.
#[tokio::test(flavor = "current_thread")]
async fn a_resting_buy_of_another_bot_on_the_kraken_key_blocks_the_sale() {
    let (d, id, exchange) = crypto_swap_bot("kraken", json!({}));
    let o = store::open(&store::Paths::from_env(&|_| None, d.path())).unwrap();
    o.primary.execute_batch(&format!("INSERT INTO bots (user_id, exchange_id, type, status, settings, transient_data, created_at, updated_at) \
        SELECT user_id, exchange_id, type, 2, json_remove(settings, '$.rebalance_enabled'), '{{}}', created_at, updated_at FROM bots WHERE id = {id}")).unwrap();
    let other = o.primary.last_insert_rowid();
    o.primary.execute(&format!("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, price, base, quote, \
        base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
        SELECT {other}, exchange_id, 'OREST-9', 0, 1, 0, 1, 0.001, 45000, base, quote, base_asset_id, quote_asset_id, 'day', 60, 'REGULAR', '[]', created_at, created_at \
        FROM transactions WHERE bot_id = {id} AND external_id = 'OHELD-1'"), []).unwrap();
    let open = ok(json!({ "error": [], "result": { "OREST-9": { "status": "open", "vol": "0.001", "vol_exec": "0", "cost": "0", "price": "0", "oflags": "",
                                                               "descr": { "pair": "XBTEUR", "type": "buy", "ordertype": "limit", "price": "45000" } } } }));
    let t = crypto_script("kraken", json!({ "POST /0/private/QueryOrders": [open] }));
    assert_eq!(crypto_fire("kraken", &o, exchange, &t, id, at(T0)).await, Outcome::Skipped("open_buy"));
    assert!(crypto_posts("kraken", &t).is_empty());
    assert_eq!(pending(&o, id), Value::Null);
}

/// Plan CB T4: a crypto rebalancer is admitted in every phase this engine resumes, a Rails-written ambiguous halt stays
/// Rails' to resolve, and a swap Rails left mid-way is taken over and handed back in Rails' own shape.
#[tokio::test(flavor = "current_thread")]
async fn a_crypto_rebalancer_is_admitted_mid_swap_and_handed_back_as_rails_left_it() {
    for venue in ["kraken", "binance", "kucoin"] {
        let (d, id, _) = crypto_swap_bot(venue, json!({}));
        let paths = store::Paths::from_env(&|_| None, d.path());
        let o = store::open(&paths).unwrap();
        let check = || eligibility::check_install(&o.primary).unwrap();
        assert!(check().problems.is_empty(), "{venue}: {:?}", check().problems);
        // A resting REBALANCE sell, and the swap Rails left selling on it.
        o.primary.execute("UPDATE transactions SET external_status = 1, transaction_type = 'REBALANCE', side = 1 WHERE external_id = 'OHELD-1'", []).unwrap();
        let sell: i64 = one(&o, "SELECT id FROM transactions WHERE external_id = 'OHELD-1'");
        let swap = json!({ "phase": "selling", "sell_transaction_id": sell, "buy_transaction_id": null, "remaining_quote_amount": null, "buy_attempted": false });
        o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json(?1))", [swap.to_string()]).unwrap();
        assert!(check().problems.is_empty(), "{venue}: {:?}", check().problems);
        drop(o);
        let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
        let o = store::open(&paths).unwrap();
        deltabadger::engine::handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at(T0)).unwrap();
        deltabadger::engine::handover::hand_back(&lock, &o, &Factory(script(json!({}))), &seed::cipher(), &FixedClock(at(T0))).await.unwrap();
        assert_eq!(pending(&o, id), swap, "{venue}: Rails reads back the swap it left");
        // Rails' own ambiguous halt is Rails' to resolve.
        o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending.phase', 'ambiguous')", []).unwrap();
        assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains("ambiguous")), "{venue}");
    }
}
