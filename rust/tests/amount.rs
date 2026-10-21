mod common;
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::venue_rules::ALPACA;
use deltabadger::ruby::BigDec;
use deltabadger::venue::OrderKind;
use serde_json::json;

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn us(t: &str) -> i64 { t.parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_micros() }
use common::install;
fn tx(status: i64, ext: i64, created: &str, q: Option<&'static str>, qexec: Option<&'static str>, a: Option<&'static str>, p: Option<&'static str>) -> TxSpec {
    TxSpec { status, external_status: Some(ext), external_id: Some(format!("O-{created}")), order_type: 0, amount: a, quote_amount: q, price: p,
             quote_amount_exec: qexec, amount_exec: None, created_at: created.into() }
}

#[test]
fn a_late_tick_owes_every_missed_interval_minus_what_was_invested() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    seed::insert_tx(&o.primary, &s, id, &tx(0, 2, "2026-09-01 10:00:01", Some("60"), Some("60"), None, None));
    let bot = model::load_bot(&o.primary, id).unwrap();
    // just past the 4th checkpoint: 4 intervals × 60 − 60 invested
    assert_eq!(amount::pending_quote_amount(&o.primary, &bot, us("2026-09-22T10:00:00.5Z")).unwrap(), bd("180"));
}

#[test]
fn waiting_orders_count_at_their_requested_value_and_old_rows_do_not_count() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("missed_quote_amount", json!("12.5")));
    seed::insert_tx(&o.primary, &s, id, &tx(0, 1, "2026-09-01 10:00:01", None, None, Some("0.001"), Some("50000")));
    seed::insert_tx(&o.primary, &s, id, &tx(0, 2, "2026-08-01 10:00:01", Some("999"), Some("999"), None, None));
    seed::insert_tx(&o.primary, &s, id, &tx(1, 0, "2026-09-01 10:00:02", Some("60"), Some("0"), None, None));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(amount::pending_quote_amount(&o.primary, &bot, us("2026-09-01T10:00:05Z")).unwrap(), bd("22.5"));
}

#[test]
fn smart_intervals_split_the_amount_and_the_interval() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!(20.0)));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(amount::pending_quote_amount(&o.primary, &bot, us("2026-09-06T10:00:00Z")).unwrap(), bd("60"));
}

#[test]
fn never_negative() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    seed::insert_tx(&o.primary, &s, id, &tx(0, 2, "2026-09-01 10:00:01", Some("500"), Some("500"), None, None));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(amount::pending_quote_amount(&o.primary, &bot, us("2026-09-02T10:00:00Z")).unwrap().is_zero());
}

fn sizing_ticker(t: &serde_json::Value) -> Ticker {
    let n = |k: &str| t[k].as_str().unwrap().parse::<i64>().unwrap();
    Ticker { id: 1, ticker: "XBTEUR".into(), base_code: "XBT".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(), exchange_name: "Kraken".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: n("base_decimals"), quote_decimals: n("quote_decimals"), price_decimals: n("price_decimals"),
             minimum_base_size: bd(t["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(t["minimum_quote_size"].as_str().unwrap()),
             trading_enabled: true, available: true, crypto: true, }
}

#[test]
fn every_recorded_rails_sizing_is_reproduced() {
    let (_d, o, s) = install();
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let cases = common::vectors()["sizing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 105);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let p = bd(c["last_or_ask"].as_str().unwrap());
        let sizing = amount::size(bot, &sizing_ticker(&c["ticker"]), &bd(c["x"].as_str().unwrap()), &p, &deltabadger::engine::venue_rules::KRAKEN).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!(plan.price.to_s_f(), c["price"].as_str().unwrap(), "price {c}");
        assert_eq!(plan.amount.to_s_f(), c["amount"].as_str().unwrap(), "unrounded amount (Ruby's division) {c}");
        assert_eq!(plan.quote_type, c["amount_type"] == "quote", "amount_type {c}");
        assert_eq!(below, c["below_minimum"].as_bool().unwrap(), "below_minimum {c}");
        assert_eq!(plan.volume.to_s_f(), c["volume"].as_str().unwrap(), "volume {c}");
    }
}

#[test]
fn a_limit_price_that_floors_to_zero_is_refused() {
    let (_d, o, s) = install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)))).unwrap();
    let t = sizing_ticker(&json!({"base_decimals": "0", "quote_decimals": "0", "price_decimals": "0", "minimum_base_size": "1", "minimum_quote_size": "5"}));
    let p = bd("0.9");
    assert!(matches!(amount::size(&bot, &t, &bd("60"), &p, &deltabadger::engine::venue_rules::KRAKEN).unwrap(), Sizing::ZeroPrice { decimals: 0 }));
}

#[test]
fn rows_bind_decimals_as_rails_does() {
    let (_d, o, s) = install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let t = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let p = bd("50000.2");
    let Sizing::BelowMinimum(plan) = amount::size(&bot, &t, &bd("0.4"), &p, &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
    assert_eq!(plan.log_details()["amount"], "0.000007999968000127999488002047991808", "Ruby's 31 digits, unrounded");
    let id = {
        let tx=o.primary.unchecked_transaction().unwrap();
        let fenced=deltabadger::engine::model::check_credential_result(&tx,&None).unwrap();
        let id = amount::write_order_row(&fenced, &bot, &plan, amount::RowKind::Skipped, "2026-09-01T10:00:01Z".parse().unwrap()).unwrap();
        tx.commit().unwrap();id
    };
    let (amount, created): (f64, String) = o.primary.query_row("SELECT amount, created_at FROM transactions WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(amount.to_bits(), bd("0.000007999968000127999488002047991808").round(18).to_f().to_bits());
    assert_eq!(created, "2026-09-01 10:00:01");
}

#[test]
fn every_recorded_rails_alpaca_sizing_and_wire_is_reproduced() {
    let (_d, o, s) = common::install_alpaca();
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let cases = common::vectors()["alpaca_sizing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 180);
    let mut refused = 0;
    let deadline = "2026-09-01T10:00:10Z".parse().unwrap();
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let t = Ticker { ticker: "BTC/USD".into(), base_code: "BTC".into(), quote_code: "USD".into(), quote_symbol: "USD".into(), exchange_name: "Alpaca".into(), ..sizing_ticker(&c["ticker"]) };
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &ALPACA).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!(plan.price.to_s_f(), c["price"].as_str().unwrap(), "price {c}");
        assert_eq!(plan.amount.to_s_f(), c["amount"].as_str().unwrap(), "unrounded amount {c}");
        assert_eq!(c["amount_type"], "quote", "Alpaca sizes every buy in quote {c}");
        assert!(plan.quote_type, "{c}");
        assert_eq!(below, c["below_minimum"].as_bool().unwrap(), "below_minimum (quote-only) {c}");
        // Exchange#limit_buy refuses a quantity under minimum_base_size (or zero): nothing reaches the wire.
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        let order = plan.to_order("cl".into(), deadline, ALPACA.wire).unwrap();
        let w = &c["wire"];
        assert_eq!((w["symbol"].as_str(), w["side"].as_str(), w["time_in_force"].as_str()), (Some("BTC/USD"), Some("buy"), Some("gtc")), "{c}");
        assert_eq!(order.pair, w["symbol"].as_str().unwrap(), "pair {c}");
        match order.kind {
            OrderKind::Market => {
                assert_eq!(w["type"], "market", "{c}");
                assert_eq!(order.volume, w["notional"].as_str().unwrap(), "notional {c}");
                assert!(bd(&order.volume) <= plan.volume, "R1c: the notional never rounds up {c}");
                assert!(order.quote_volume);
            }
            OrderKind::Limit { price } => {
                assert_eq!(w["type"], "limit", "{c}");
                assert_eq!(order.volume, w["qty"].as_str().unwrap(), "qty {c}");
                assert_eq!(price, w["limit_price"].as_str().unwrap(), "limit_price {c}");
                assert!(bd(&price) <= plan.price && bd(&order.volume).checked_mul(&bd(&price)).unwrap() <= plan.volume, "R1c: price and qty never round up {c}");
                assert!(!order.quote_volume);
            }
            OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
        }
    }
    assert_eq!(refused, 34, "limit quantities under minimum_base_size Rails refuses");
}

#[test]
fn wire_decimal_floors_and_pads_in_decimal_as_rails_sends_it() {
    // Exchanges::Alpaca#wire_decimal, recorded in Ruby 4.0.7.
    assert_eq!(amount::wire_decimal(&bd("812037646.065271585"), 9), "812037646.065271585");
    assert_eq!(amount::wire_decimal(&bd("123456789012.123456789"), 9), "123456789012.123456789");
    assert_eq!(amount::wire_decimal(&bd("0.00265"), 4), "0.0026");
    assert_eq!(amount::wire_decimal(&bd("0.5"), 4), "0.5000");
    assert_eq!(amount::wire_decimal(&bd("5"), 2), "5.00");
    assert_eq!(amount::wire_decimal(&bd("5.99"), 0), "5");
    assert_eq!(amount::wire_decimal(&bd("0"), 2), "0.00");
}

#[test]
fn a_cancelled_or_abandoned_buy_counts_what_it_filled_and_the_rest_is_owed_again() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("missed_quote_amount", json!("30")));
    seed::insert_tx(&o.primary, &s, id, &tx(0, 3, "2026-09-01 10:00:01", Some("90"), Some("39.96"), None, None)); // expired part-filled
    seed::insert_tx(&o.primary, &s, id, &tx(0, 4, "2026-09-01 10:00:02", Some("60"), None, None, None));           // abandoned, never filled
    seed::insert_tx(&o.primary, &s, id, &tx(0, 3, "2026-08-31 10:00:00", Some("60"), Some("60"), None, None));     // before the window
    let bot = model::load_bot(&o.primary, id).unwrap();
    // Rails (#448): two weeks owed (120) + the carry (30) − the 39.96 that filled. The unfilled 50.04 is owed again, and no
    // fill ever consumed the carry. The old arithmetic said 150 here.
    assert_eq!(amount::pending_quote_amount(&o.primary, &bot, us("2026-09-08T10:00:00.5Z")).unwrap(), bd("110.04"));
}

/// One recorded carry row, bound as Rails binds it: decimals as the Float BigDecimal#to_f gives, absent keys NULL.
fn insert_carry_row(c: &rusqlite::Connection, s: &seed::Seeded, bot_id: i64, row: &serde_json::Value) {
    let text = |k: &str| row.get(k).and_then(serde_json::Value::as_str).map(str::to_string);
    let real = |k: &str| row.get(k).and_then(serde_json::Value::as_str).map(|v| v.parse::<f64>().unwrap_or_else(|_| panic!("{k} {v}")));
    let int = |k: &str| row.get(k).and_then(serde_json::Value::as_i64);
    c.execute(
        "INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
         amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, \
         error_messages, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'BTC', 'USD', ?13, ?14, 'day', 60, ?15, '[]', ?16, ?16)",
        rusqlite::params![bot_id, s.exchange_id, text("external_id"), int("status"), int("external_status"), int("side"), int("order_type"),
                          real("amount"), real("quote_amount"), real("price"), real("amount_exec"), real("quote_amount_exec"), s.btc, s.quote,
                          text("transaction_type"), text("created_at")]).unwrap();
}

#[test]
fn every_recorded_rails_carry_is_reproduced() {
    let cases = common::vectors()["carry"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 36);
    for (i, c) in cases.iter().enumerate() {
        let (_d, o, s) = common::install_alpaca();
        let spec = BotSpec { status: 1, started_at: Some(c["started_at"].as_str().unwrap().into()),
                             settings_changed_at: c["settings_changed_at"].as_str().map(str::to_string),
                             settings: c["settings"].clone(), transient: json!({ "missed_quote_amount": c["carry"] }) };
        let id = seed::insert_bot(&o.primary, &s, &spec);
        for r in c["rows"].as_array().unwrap() { insert_carry_row(&o.primary, &s, id, r); }
        let bot = model::load_bot(&o.primary, id).unwrap();
        let got = amount::pending_quote_amount(&o.primary, &bot, us(c["now"].as_str().unwrap())).unwrap();
        assert_eq!(got, bd(c["pending"].as_str().unwrap()), "case {i}: {c}");
    }
}

/// Bot::Lifecycle#start(start_fresh: false): run now, or wait for the next checkpoint, as Rails decided each recorded case
/// (from the Bot::ActionJob it enqueued). A continue start never takes a delayed first run, whatever the start time says.
#[test]
fn every_recorded_rails_continue_start_decision_is_reproduced() {
    let cases = common::vectors()["continue_start"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 7);
    for (i, c) in cases.iter().enumerate() {
        let (_d, o, s) = common::install_alpaca();
        let mut spec = BotSpec { status: 1, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }.with("interval", json!("day"))
            .transient("quote_amount_limit_enabled_at", json!("2026-08-01T00:00:00.000Z"));
        for (k, v) in c["settings"].as_object().unwrap() { spec = spec.with(k, v.clone()); }
        if !c["last_action_job_at"].is_null() { spec = spec.transient("last_action_job_at", c["last_action_job_at"].clone()); }
        let id = seed::insert_bot(&o.primary, &s, &spec);
        for r in c["rows"].as_array().unwrap() { insert_carry_row(&o.primary, &s, id, r); }
        let bot = model::load_bot(&o.primary, id).unwrap();
        let now = amount::continue_runs_now(&o.primary, &bot, us(c["now"].as_str().unwrap())).unwrap();
        assert_eq!(if now { "now" } else { "checkpoint" }, c["decision"].as_str().unwrap(), "case {i}: {c}");
    }
}

#[test]
fn every_recorded_rails_alpaca_stock_wire_is_reproduced() {
    let (_d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let spec = || BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(aapl, 1.0)]);
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &spec())).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &spec()
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let t = model::ticker_for(&o.primary, &market).unwrap().unwrap();
    assert!(!t.crypto, "a Stock asset's ticker is not crypto");
    let cases = common::vectors()["alpaca_stock_sizing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 36);
    let deadline = "2026-09-01T14:00:10Z".parse().unwrap();
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let (plan, below) = match amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &ALPACA).unwrap() {
            Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}"),
        };
        assert_eq!(plan.price.to_s_f(), c["price"].as_str().unwrap(), "price {c}");
        assert_eq!(below, c["below_minimum"].as_bool().unwrap(), "below_minimum {c}");
        assert_eq!(plan.limit_refusal().unwrap(), None, "{c}");
        let order = plan.to_order("cl".into(), deadline, ALPACA.wire).unwrap();
        let w = &c["wire"];
        assert_eq!(w["time_in_force"], "day", "Rails sends a stock as a day order {c}");
        assert!(order.day, "{c}");
        assert_eq!(order.pair, w["symbol"].as_str().unwrap(), "the bare symbol {c}");
        match order.kind {
            OrderKind::Market => assert_eq!(order.volume, w["notional"].as_str().unwrap(), "notional {c}"),
            OrderKind::Limit { price } => {
                assert_eq!(order.volume, w["qty"].as_str().unwrap(), "qty {c}");
                assert_eq!(price, w["limit_price"].as_str().unwrap(), "limit_price floored to 2 decimals {c}");
            }
            OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
        }
    }
}

#[test]
fn an_alpaca_limit_quantity_under_the_base_minimum_is_refused_before_sending() {
    // Exchange#limit_buy: 1 USD floors to 0.0000 BTC at 64160.69 (4 decimals), and to 0.00002 BTC at 50000, under 0.000027.
    for (min, decimals, price, message) in [
        ("0.0001", "4", "64160.69", "Limit order of 0.0 BTC is below the minimum of 0.0001 BTC for BTC/USD"),
        ("0.000027", "9", "50000", "Limit order of 0.00002 BTC is below the minimum of 0.000027 BTC for BTC/USD"),
    ] {
        let t = Ticker { ticker: "BTC/USD".into(), base_code: "BTC".into(), quote_code: "USD".into(), quote_symbol: "USD".into(), exchange_name: "Alpaca".into(),
            ..sizing_ticker(&json!({"base_decimals": decimals, "quote_decimals": "2", "price_decimals": "2", "minimum_base_size": min, "minimum_quote_size": "1"})) };
        let plan = amount::OrderPlan { ticker: t, side: deltabadger::venue::Side::Buy, limit: true, price: bd(price), amount: bd("1").div(&bd(price)).unwrap(), quote_amount: bd("1"),
            quote_type: true, volume: bd("1"), kind: amount::TxKind::Regular,
            origin: amount::Origin::QuoteBudget, wire_price: None };
        assert_eq!(plan.limit_refusal().unwrap().as_deref(), Some(message));
    }
}

#[test]
fn every_recorded_rails_binance_sizing_and_wire_is_reproduced() {
    use deltabadger::engine::venue_rules::BINANCE;
    let (_d, o, s) = install();
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/binance_vectors.json")).unwrap();
    let cases = vectors["binance_sizing"].as_array().unwrap();
    assert_eq!(cases.len(), 10_593);
    let deadline = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut quote, mut refused) = (0, 0);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let shape = &vectors["binance_tickers"][c["ticker"].as_u64().unwrap() as usize];
        let t = Ticker { ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), quote_symbol: "USDT".into(), exchange_name: "Binance".into(), ..sizing_ticker(shape) };
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &BINANCE).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!(plan.price.to_s_f(), c["price"].as_str().unwrap(), "price {c}");
        assert_eq!(plan.amount.to_s_f(), c["amount"].as_str().unwrap(), "unrounded amount {c}");
        assert_eq!(plan.quote_type, c["amount_type"] == "quote", "amount_type {c}");
        assert_eq!(below, c["below_minimum"].as_bool().unwrap(), "below_minimum {c}");
        // Exchange#limit_buy refuses a quantity under minimum_base_size (or zero): nothing reaches the wire.
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        let order = plan.to_order("cl".into(), deadline, BINANCE.wire).unwrap();
        let w = &c["wire"];
        assert_eq!((w["symbol"].as_str(), w["side"].as_str(), order.pair.as_str()), (Some("BTCUSDT"), Some("BUY"), "BTCUSDT"), "{c}");
        match order.kind {
            OrderKind::Market => {
                assert_eq!((w["type"].as_str(), w["self_trade_prevention_mode"].as_str()), (Some("MARKET"), Some("EXPIRE_MAKER")), "{c}");
                let (sent, absent) = if order.quote_volume { quote += 1; ("quote_order_qty", "quantity") } else { ("quantity", "quote_order_qty") };
                assert_eq!(Some(order.volume.as_str()), w[sent].as_str(), "{sent} {c}");
                assert!(w.get(absent).is_none(), "{c}");
            }
            OrderKind::Limit { price } => {
                assert_eq!((w["type"].as_str(), w["time_in_force"].as_str()), (Some("LIMIT"), Some("GTC")), "{c}");
                assert!(w.get("self_trade_prevention_mode").is_none() && !order.quote_volume, "{c}");
                assert_eq!((Some(price.as_str()), Some(order.volume.as_str())), (w["price"].as_str(), w["quantity"].as_str()), "price and quantity {c}");
            }
            OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
        }
    }
    assert_eq!((quote, refused), (2_317, 1_785), "both market wires and the limit refusal are exercised");
}

/// Exchanges::Kucoin's sizing (Bot::OrderSetter under :base_or_quote / :base_and_quote_in_base) and #set_market_order /
/// #set_limit_order's wire, replayed through amount::size, limit_refusal and to_order (script/rust/record_venue_vectors.rb kucoin).
#[test]
fn every_recorded_rails_kucoin_sizing_and_wire_is_reproduced() {
    use deltabadger::engine::venue_rules::KUCOIN;
    let (_d, o, s) = install();
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/kucoin_vectors.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (file, recorded) in vectors["sources"].as_object().unwrap() {
        use sha2::{Digest, Sha256};
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap())), recorded.as_str().unwrap(), "{file} changed: re-record with script/rust/repin");
    }
    let cases = vectors["kucoin_sizing"].as_array().unwrap();
    assert_eq!(cases.len(), 10_471);
    let deadline = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut quote, mut refused) = (0, 0);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let shape = &vectors["kucoin_tickers"][c["ticker"].as_u64().unwrap() as usize];
        let t = Ticker { ticker: "BTC-USDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), quote_symbol: "USDT".into(), exchange_name: "KuCoin".into(), ..sizing_ticker(shape) };
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &KUCOIN).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!(plan.price.to_s_f(), c["price"].as_str().unwrap(), "price {c}");
        assert_eq!(plan.amount.to_s_f(), c["amount"].as_str().unwrap(), "unrounded amount {c}");
        assert_eq!(plan.quote_type, c["amount_type"] == "quote", "amount_type {c}");
        assert_eq!(below, c["below_minimum"].as_bool().unwrap(), "below_minimum {c}");
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        let order = plan.to_order("cl".into(), deadline, KUCOIN.wire).unwrap();
        let w = &c["wire"];
        assert_eq!((w["symbol"].as_str(), w["side"].as_str(), order.pair.as_str()), (Some("BTC-USDT"), Some("buy"), "BTC-USDT"), "{c}");
        match order.kind {
            OrderKind::Market => {
                assert_eq!(w["type"].as_str(), Some("market"), "{c}");
                let (sent, absent) = if order.quote_volume { quote += 1; ("funds", "size") } else { ("size", "funds") };
                assert_eq!(Some(order.volume.as_str()), w[sent].as_str(), "{sent} {c}");
                assert!(w.get(absent).is_none() && w.get("price").is_none(), "{c}");
            }
            OrderKind::Limit { price } => {
                assert_eq!(w["type"].as_str(), Some("limit"), "{c}");
                assert!(!order.quote_volume && w.get("funds").is_none(), "{c}");
                assert_eq!((Some(price.as_str()), Some(order.volume.as_str())), (w["price"].as_str(), w["size"].as_str()), "price and size {c}");
            }
            OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
        }
    }
    assert_eq!((quote, refused), (2_177, 1_824), "both market wires and the limit refusal are exercised");
}

#[test]
fn the_binance_port_is_the_ruby_that_was_recorded() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/binance_vectors.json")).unwrap();
    for (file, recorded) in vectors["sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-record with script/rust/repin and re-check the Binance port");
    }
}

// ---- 6a: a sell's price and size (Bot::OrderSetter#order_price(:sell), #calculate_best_amount_info for side: :sell)

#[test]
fn a_sell_is_priced_above_the_last_trade_and_sized_in_base_under_base_and_quote_in_base() {
    use deltabadger::venue::Side;
    let (_d, o, s) = install();
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.01)))).unwrap();
    let t = sizing_ticker(&json!({"base_decimals": "5", "quote_decimals": "2", "price_decimals": "2", "minimum_base_size": "0.00001", "minimum_quote_size": "5"}));
    let last = bd("60000.004");
    assert_eq!(amount::side_price(&limit, &t, &last, Side::Sell, &deltabadger::engine::venue_rules::KRAKEN).unwrap(), bd("60600"), "(1 + d), floored");
    assert_eq!(amount::side_price(&limit, &t, &last, Side::Buy, &deltabadger::engine::venue_rules::KRAKEN).unwrap(), bd("59400"), "(1 - d), floored");
    assert_eq!(amount::side_price(&market, &t, &last, Side::Sell, &deltabadger::engine::venue_rules::KRAKEN).unwrap(), last, "a market sell is the bid itself");
    let price = bd("60600");
    // ceil(5 / 60600, 5) = 0.00009 is the floor; the amount is floored to 5 decimals against it.
    let size = |a: &str| amount::size_sell(&limit, &t, &bd(a), &price).unwrap();
    let Sizing::Place(p) = size("0.000123456") else { panic!() };
    assert_eq!((p.side, p.volume.to_s_f(), p.quote_type, p.limit), (Side::Sell, "0.00012".to_string(), false, true));
    assert_eq!(p.quote_amount, &bd("0.000123456") * &price, "quote_amount is amount × price");
    assert!(matches!(size("0.00009999"), Sizing::Place(_)), "0.00009 is not below 0.00009");
    assert!(matches!(size("0.000089"), Sizing::BelowMinimum(_)));
    assert!(matches!(size("0.000001"), Sizing::BelowMinimum(_)), "positive, floors to zero: a skipped row, not order_ignored");
    assert!(matches!(size("0"), Sizing::Ignored(_)));
    // Whatever the venue's buy logic, a sell's wire is a base quantity.
    for wire in [deltabadger::engine::venue_rules::KRAKEN.wire, ALPACA.wire, deltabadger::engine::venue_rules::BINANCE.wire] {
        let Sizing::Place(m) = amount::size_sell(&market, &t, &bd("0.5"), &price).unwrap() else { panic!() };
        let order = m.to_order("cl".into(), "2026-09-01T10:00:10Z".parse().unwrap(), wire).unwrap();
        assert_eq!((order.side, order.quote_volume, order.volume.as_str()), (Side::Sell, false, if wire == ALPACA.wire { "0.50000" } else { "0.5" }));
    }
}

/// Plan 6a T8: Rails' one-asset sell on Kraken, Binance and Alpaca — order_price(:sell), `[base || quote / price, free].min`,
/// calculate_best_amount_info(side: :sell) and the market_sell / limit_sell wire — replayed through side_price, size_sell,
/// limit_refusal and to_order (script/rust/record_sell_sizing_vectors.rb).
#[test]
fn every_recorded_rails_sell_sizing_and_wire_is_reproduced() {
    use deltabadger::engine::venue_rules::{ALPACA, BINANCE, KRAKEN};
    use deltabadger::venue::Side;
    let (_d, o, s) = install();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/sell_sizing_vectors.json")).unwrap();
    let bot = |d: &serde_json::Value| {
        let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00");
        let spec = if d.is_null() { spec } else { spec.with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", d.clone()) };
        model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &spec)).unwrap()
    };
    let bots: Vec<(serde_json::Value, model::Bot)> = [json!(null), json!(0.0025), json!(0.01), json!(0.001), json!(0.1234)].into_iter().map(|d| { let b = bot(&d); (d, b) }).collect();
    let deadline = "2026-09-01T10:00:10Z".parse().unwrap();
    for (venue, rules, pair, base, quote, expected) in [("kraken", &KRAKEN, "XXBTZEUR", "XBT", "EUR", (2_102, 790, 509)),
                                                        ("binance", &BINANCE, "BTCUSDT", "BTC", "USDT", (2_130, 810, 485)),
                                                        ("alpaca", &ALPACA, "BTC/USD", "BTC", "USD", (2_127, 858, 535))] {
        let cases = vectors[format!("{venue}_sell")].as_array().unwrap();
        let (mut below_n, mut refused_n) = (0, 0);
        for c in cases {
            let bot = &bots.iter().find(|(d, _)| *d == c["d"]).unwrap().1;
            let t = Ticker { ticker: pair.into(), base_code: base.into(), quote_code: quote.into(), quote_symbol: quote.into(),
                             ..sizing_ticker(&vectors["tickers"][c["t"].as_u64().unwrap() as usize]) };
            let price = amount::side_price(bot, &t, &bd(c["reference"].as_str().unwrap()), Side::Sell, rules).unwrap();
            assert_eq!(price.to_s_f(), c["price"].as_str().unwrap(), "price {c}");
            // tick::sell's `base || quote / price`, then `[amount, free].min`.
            let wanted = bd(c["wanted"].as_str().unwrap());
            let wanted = if c["denomination"] == "quote" { wanted.div(&price).unwrap() } else { wanted };
            let free = bd(c["free"].as_str().unwrap());
            let amount = if free < wanted { free } else { wanted };
            let (plan, below) = match amount::size_sell(bot, &t, &amount, &price).unwrap() {
                Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
            assert_eq!((plan.side, plan.quote_type), (Side::Sell, false), "{c}");
            assert_eq!(plan.amount.to_s_f(), c["amount"].as_str().unwrap(), "amount {c}");
            assert_eq!(plan.quote_amount.to_s_f(), c["quote_amount"].as_str().unwrap(), "quote_amount {c}");
            assert_eq!(below, c["below_minimum"].as_bool().unwrap(), "below_minimum {c}");
            if below { below_n += 1; }
            let refusal = plan.limit_refusal().unwrap();
            assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
            if refusal.is_some() { refused_n += 1; continue; }
            let order = plan.to_order("cl".into(), deadline, rules.wire).unwrap();
            let w = &c["wire"];
            assert!(order.side == Side::Sell && !order.quote_volume, "{c}");
            if venue == "kraken" {
                assert_eq!((w["type"].as_str(), w["pair"].as_str(), w["oflags"].clone()), (Some("sell"), Some(pair), json!([])), "{c}");
                assert_eq!(w["volume"].as_str(), Some(order.volume.as_str()), "volume {c}");
                match &order.kind {
                    OrderKind::Market => assert_eq!((w["ordertype"].as_str(), w.get("price")), (Some("market"), None), "{c}"),
                    OrderKind::Limit { price } => assert_eq!((w["ordertype"].as_str(), w["price"].as_str()), (Some("limit"), Some(price.as_str())), "{c}"),
                    OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
                }
            } else if venue == "alpaca" {
                assert_eq!((w["symbol"].as_str(), w["side"].as_str(), w["time_in_force"].as_str()), (Some(pair), Some("sell"), Some("gtc")), "{c}");
                assert_eq!(w["qty"].as_str(), Some(order.volume.as_str()), "qty {c}");
                assert!(w.get("notional").is_none(), "{c}");
                match &order.kind {
                    OrderKind::Market => assert_eq!((w["type"].as_str(), w.get("limit_price")), (Some("market"), None), "{c}"),
                    OrderKind::Limit { price } => assert_eq!((w["type"].as_str(), w["limit_price"].as_str()), (Some("limit"), Some(price.as_str())), "{c}"),
                    OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
                }
            } else {
                assert_eq!((w["symbol"].as_str(), w["side"].as_str()), (Some(pair), Some("SELL")), "{c}");
                assert_eq!(w["quantity"].as_str(), Some(order.volume.as_str()), "quantity {c}");
                assert!(w.get("quote_order_qty").is_none(), "{c}");
                match &order.kind {
                    OrderKind::Market => assert_eq!((w["type"].as_str(), w["self_trade_prevention_mode"].as_str()), (Some("MARKET"), Some("EXPIRE_MAKER")), "{c}"),
                    OrderKind::Limit { price } => assert_eq!((w["type"].as_str(), w["time_in_force"].as_str(), w["price"].as_str()), (Some("LIMIT"), Some("GTC"), Some(price.as_str())), "{c}"),
                    OrderKind::Crossed { .. } => panic!("no venue in these vectors crosses the book"),
                }
            }
        }
        assert_eq!((cases.len(), below_n, refused_n), expected, "{venue}: every case replayed, both floors and the limit refusal exercised");
    }
}

#[test]
fn the_sell_sizing_port_is_the_ruby_that_was_recorded() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/sell_sizing_vectors.json")).unwrap();
    for (file, recorded) in vectors["sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-record with script/rust/repin and re-check the sell port");
    }
}
