mod common;
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::ruby::BigDec;
use deltabadger::venue::Prices;
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
    Ticker { id: 1, ticker: "XBTEUR".into(), base_code: "XBT".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(), exchange_name: "Kraken".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: n("base_decimals"), quote_decimals: n("quote_decimals"), price_decimals: n("price_decimals"),
             minimum_base_size: bd(t["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(t["minimum_quote_size"].as_str().unwrap()),
             trading_enabled: true, available: true }
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
        let sizing = amount::size(bot, &sizing_ticker(&c["ticker"]), &bd(c["x"].as_str().unwrap()), &Prices { bid: p.clone(), ask: p.clone(), last: p });
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
    assert!(matches!(amount::size(&bot, &t, &bd("60"), &Prices { bid: p.clone(), ask: p.clone(), last: p }), Sizing::ZeroPrice { decimals: 0 }));
}

#[test]
fn rows_bind_decimals_as_rails_does() {
    let (_d, o, s) = install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let t = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let p = bd("50000.2");
    let Sizing::BelowMinimum(plan) = amount::size(&bot, &t, &bd("0.4"), &Prices { bid: p.clone(), ask: p.clone(), last: p }) else { panic!() };
    assert_eq!(plan.log_details()["amount"], "0.000007999968000127999488002047991808", "Ruby's 31 digits, unrounded");
    let id = amount::write_order_row(&o.primary, &bot, &plan, amount::RowKind::Skipped, "2026-09-01T10:00:01Z".parse().unwrap()).unwrap();
    let (amount, created): (f64, String) = o.primary.query_row("SELECT amount, created_at FROM transactions WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(amount.to_bits(), bd("0.000007999968000127999488002047991808").round(18).to_f().to_bits());
    assert_eq!(created, "2026-09-01 10:00:01");
}
