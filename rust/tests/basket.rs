mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::{basket, model};
use deltabadger::ruby::BigDec;
use serde_json::{json, Value};
use std::collections::HashMap;

/// A recorded case's bot as script/rust/record_vectors.rb's `with_basket` builds it: its V-prefixed members on Alpaca with the
/// recorded precision, allocations in the recorded order, and the recorded rows.
fn build(case: &Value) -> (tempfile::TempDir, deltabadger::store::Opened, i64, HashMap<String, i64>) {
    let (d, o, s) = common::install_alpaca();
    let mut ids = HashMap::new();
    for sym in case["weights"].as_object().unwrap().keys() {
        ids.insert(sym.clone(), seed::add_alpaca_crypto(&o.primary, &s, sym, &case["pairs"][sym]).0);
    }
    let allocations: serde_json::Map<String, Value> = case["weights"].as_object().unwrap().iter().map(|(sym, w)| (ids[sym].to_string(), w.clone())).collect();
    let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("interval", json!("day")).with("allocations", Value::Object(allocations));
    let id = seed::insert_bot(&o.primary, &s, &spec);
    for row in case["rows"].as_array().into_iter().flatten() { seed::insert_row(&o.primary, &s, id, ids[row["asset"].as_str().unwrap()], row); }
    (d, o, id, ids)
}

fn by_symbol(ids: &HashMap<String, i64>, m: HashMap<i64, BigDec>) -> Value {
    Value::Object(m.into_iter().map(|(asset, v)| (ids.iter().find(|(_, id)| **id == asset).unwrap().0.clone(), json!(v.to_s_f()))).collect())
}

#[test]
fn every_recorded_rails_ledger_is_reproduced() {
    let cases = common::vectors()["basket_ledgers"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 45);
    for (i, case) in cases.iter().enumerate() {
        let (_d, o, id, ids) = build(case);
        let bot = model::load_bot(&o.primary, id).unwrap();
        assert_eq!(by_symbol(&ids, basket::holdings(&o.primary, &bot).unwrap()), case["holdings"], "case {i} holdings: {case}");
        assert_eq!(by_symbol(&ids, basket::reserved(&o.primary, &bot).unwrap()), case["reserved"], "case {i} reserved: {case}");
    }
}

#[test]
fn a_row_the_port_does_not_walk_fails_instead_of_reading_as_nothing() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, price, amount_exec, quote_amount_exec, \
                       base_asset_id, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                       VALUES (?1, ?2, 'OSELL', 0, 2, 1, 0, 64000, 0.001, 64, ?3, 'REGULAR', '[]', 'week', 60, '2026-09-01', '2026-09-01')",
                      rusqlite::params![id, s.exchange_id, s.btc]).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(matches!(basket::holdings(&o.primary, &bot), Err(deltabadger::engine::EngineError::Data(_))), "eligibility refuses it; met anyway, the tick fails");
}

fn at(s: &str) -> chrono::DateTime<chrono::Utc> { s.parse().unwrap() }

fn symbol_of(ids: &HashMap<String, i64>, asset: i64) -> String { ids.iter().find(|(_, id)| **id == asset).unwrap().0.clone() }

#[test]
fn every_recorded_rails_composition_is_reproduced() {
    let cases = common::vectors()["basket_compositions"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 36);
    for (i, case) in cases.iter().enumerate() {
        let (_d, o, id, ids) = build(case);
        o.primary.execute("UPDATE exchanges SET name = ?1", [case["exchange"].as_str().unwrap()]).unwrap();
        let bot = model::load_bot(&o.primary, id).unwrap();
        // bot.save!'s after_save refresh_composition, with every member tradable.
        basket::refresh_composition(&o.primary, &bot, at("2026-09-01T10:00:00Z")).unwrap().expect("the save derives");
        let mut failure = Value::Null;
        let untradable: Vec<&str> = case["untradable"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        if !untradable.is_empty() {
            for sym in &untradable { o.primary.execute("UPDATE tickers SET trading_enabled = 0 WHERE base_asset_id = ?1", [ids[*sym]]).unwrap(); }
            if let Err(m) = basket::refresh_composition(&o.primary, &bot, at("2026-09-01T11:00:00Z")).unwrap() { failure = json!(m); }
        }
        assert_eq!(failure, case["failure"], "case {i}");
        let mut s = o.primary.prepare("SELECT asset_id, target_allocation, in_index FROM bot_index_assets WHERE bot_id = ?1 ORDER BY id").unwrap();
        let rows: Vec<Value> = s.query_map([id], |r| {
            let target = basket::target_from_sql(r.get_ref(1)?).unwrap();
            Ok(json!([symbol_of(&ids, r.get(0)?), target.map(|t| t.to_s_f()), r.get::<_, Option<bool>>(2)?]))
        }).unwrap().map(Result::unwrap).collect();
        assert_eq!(Value::Array(rows), case["index_rows"], "case {i} rows: {case}");
        let members: Vec<Value> = basket::members(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap().into_iter()
            .map(|m| json!([symbol_of(&ids, m.asset_id), format!("{:016x}", m.weight.to_bits())])).collect();
        assert_eq!(Value::Array(members), case["members"], "case {i} members: {case}");
    }
}

#[test]
fn an_unchanged_composition_writes_nothing() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").weights(&[(s.btc, 0.7), (eth, 0.3)]));
    let bot = model::load_bot(&o.primary, id).unwrap();
    basket::refresh_composition(&o.primary, &bot, at("2026-09-01T10:00:00Z")).unwrap().unwrap();
    let snapshot = || -> String { o.primary.query_row("SELECT json_group_array(json_array(id, target_allocation, in_index, entered_at, exited_at, updated_at)) FROM bot_index_assets", [], |r| r.get(0)).unwrap() };
    let before = snapshot();
    basket::refresh_composition(&o.primary, &bot, at("2026-09-02T10:00:00Z")).unwrap().unwrap();
    assert_eq!(snapshot(), before, "ActiveRecord saves nothing unchanged, so updated_at stays");
}

/// Another bot's rows and this bot's resting sell count for neither reading; a former member's fills still count as held.
#[test]
fn only_this_bots_buys_are_read_and_a_former_members_fills_stay_held() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let mine = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let other = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let closed = |amount: &str| json!({ "external_id": format!("C{amount}"), "external_status": 2, "price": "64000", "amount_exec": amount, "quote_amount_exec": "64", "created_at": "2026-09-01 10:00:00" });
    let resting = |amount: &str| json!({ "external_id": format!("R{amount}"), "external_status": 1, "order_type": 1, "price": "64000", "amount": amount, "amount_exec": "0", "created_at": "2026-09-01 10:00:00" });
    seed::insert_row(&o.primary, &s, mine, s.btc, &closed("0.001"));
    seed::insert_row(&o.primary, &s, mine, eth, &closed("0.5")); // ETH is not in the bot's allocations: an exited member
    seed::insert_row(&o.primary, &s, mine, s.btc, &resting("0.002"));
    seed::insert_row(&o.primary, &s, other, s.btc, &closed("7"));
    seed::insert_row(&o.primary, &s, other, s.btc, &resting("9"));
    let bot = model::load_bot(&o.primary, mine).unwrap();
    let held = |m: HashMap<i64, BigDec>| { let mut v: Vec<(i64, String)> = m.into_iter().map(|(k, v)| (k, v.to_s_f())).collect(); v.sort(); v };
    let mut want = vec![(s.btc, "0.001".to_string()), (eth, "0.5".to_string())];
    want.sort();
    assert_eq!(held(basket::holdings(&o.primary, &bot).unwrap()), want);
    assert_eq!(held(basket::reserved(&o.primary, &bot).unwrap()), vec![(s.btc, "0.002".to_string())]);

    let mut sell = resting("0.003");
    sell["side"] = json!(1);
    seed::insert_row(&o.primary, &s, mine, s.btc, &sell);
    assert_eq!(held(basket::reserved(&o.primary, &bot).unwrap()), vec![(s.btc, "0.002".to_string())], "a resting sell reserves nothing to buy");
}
