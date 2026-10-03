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
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("interval", json!("day")).with("allocations", Value::Object(allocations));
    // The recorder's limit settings: `with_basket(…, settings: { limit_ordered: true, limit_order_pcnt_distance: 0.0025 })`.
    if case["limit"] == true { spec = spec.with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)); }
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
    assert_eq!(cases.len(), 43);
    for (i, case) in cases.iter().enumerate() {
        let (_d, o, id, ids) = build(case);
        o.primary.execute("UPDATE exchanges SET name = ?1", [case["exchange"].as_str().unwrap()]).unwrap();
        let bot = model::load_bot(&o.primary, id).unwrap();
        // bot.save!'s after_save refresh_composition, with every member tradable.
        basket::refresh_composition(&o.primary, &bot, at("2026-09-01T10:00:00Z")).unwrap().expect("the save derives");
        let stamps = |sql: &str| -> Vec<(i64, Option<String>, Option<String>)> {
            let mut s = o.primary.prepare(sql).unwrap();
            s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
        };
        let first: HashMap<i64, Option<String>> = stamps("SELECT asset_id, entered_at, exited_at FROM bot_index_assets WHERE bot_id = ?1")
            .into_iter().map(|(a, e, _)| (a, e)).collect();
        let mut failure = Value::Null;
        let untradable: Vec<&str> = case["untradable"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        if !untradable.is_empty() {
            for sym in &untradable { o.primary.execute("UPDATE tickers SET trading_enabled = 0 WHERE base_asset_id = ?1", [ids[*sym]]).unwrap(); }
            if let Err(m) = basket::refresh_composition(&o.primary, &bot, at("2026-09-01T11:00:00Z")).unwrap() { failure = json!(m); }
        }
        let readd: Vec<&str> = case["readd"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        if !readd.is_empty() {
            for sym in &readd { o.primary.execute("UPDATE tickers SET trading_enabled = 1 WHERE base_asset_id = ?1", [ids[*sym]]).unwrap(); }
            basket::refresh_composition(&o.primary, &bot, at("2026-09-01T12:00:00Z")).unwrap().expect("a re-add derives");
        }
        assert_eq!(failure, case["failure"], "case {i}");
        let got: Vec<Value> = stamps("SELECT asset_id, entered_at, exited_at FROM bot_index_assets WHERE bot_id = ?1 ORDER BY id").into_iter()
            .map(|(a, e, x)| json!([symbol_of(&ids, a), first[&a] == e, x.is_none()])).collect();
        assert_eq!(Value::Array(got), case["stamps"], "case {i} entered/exited: {case}");
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

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

#[test]
fn every_recorded_rails_split_is_reproduced() {
    use deltabadger::engine::amount;
    let cases = common::vectors()["basket_splits"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 55);
    for (i, case) in cases.iter().enumerate() {
        let (_d, o, id, _ids) = build(case);
        let bot = model::load_bot(&o.primary, id).unwrap();
        basket::refresh_composition(&o.primary, &bot, at("2026-09-01T10:00:00Z")).unwrap().expect("the save derives");
        let side = if case["limit"] == true { "last" } else { "ask" };
        let priced: Vec<basket::Priced> = basket::members(&o.primary, &bot).unwrap().into_iter().map(|m| {
            let reference = bd(case["prices"][&m.ticker.base_code][side].as_str().unwrap());
            let price = amount::order_price(&bot, &m.ticker, &reference);
            basket::Priced { member: m, reference, price }
        }).collect();
        let got = basket::split(&priced, &basket::holdings(&o.primary, &bot).unwrap(), &basket::reserved(&o.primary, &bot).unwrap(), &bd(case["x"].as_str().unwrap()));
        match (got, &case["orders"]) {
            (Ok(legs), Value::Array(want)) => {
                let price_of = |id: i64| priced.iter().find(|p| p.member.ticker.id == id).unwrap().price.clone();
                let got: Vec<Value> = legs.iter().map(|l| {
                    let p = price_of(l.ticker.id);
                    json!([l.ticker.base_code, p.to_s_f(), l.quote.div(&p).unwrap().to_s_f(), l.quote.to_s_f()])
                }).collect();
                assert_eq!(&got, want, "case {i}: {case}");
            }
            (Err(decimals), Value::Null) => assert_eq!(case["failure"], format!("limit price rounds to zero at {decimals} decimals"), "case {i}"),
            (got, want) => panic!("case {i}: Rust {:?}, Rails {want}", got.map(|legs| legs.len())),
        }
    }
}

/// The cap's remainder and its decision, with Rails' own numerics. Each case is a one-asset bot with the recorded cap (an
/// Integer or a Float in the settings JSON) switched on before its rows.
#[test]
fn every_recorded_rails_amount_cap_is_reproduced() {
    use deltabadger::engine::amount;
    use deltabadger::ruby::Num;
    let cases = common::vectors()["amount_caps"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 40);
    for (i, case) in cases.iter().enumerate() {
        let (_d, o, id, _ids) = build(case);
        o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', json(?1)), \
                           transient_data = json_set(transient_data, '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = ?2",
                          rusqlite::params![case["cap"].to_string(), id]).unwrap();
        let bot = model::load_bot(&o.primary, id).unwrap();
        let got = amount::quote_amount_available_num(&o.primary, &bot).unwrap().unwrap();
        let want = &case["available"];
        let same = match (&got, want["class"].as_str().unwrap()) {
            (Num::Float(f), "Float") => format!("{:016x}", f.to_bits()) == want["f"].as_str().unwrap(),
            (Num::Dec(d), "BigDecimal") => d.to_s_f() == want["d"].as_str().unwrap(),
            (Num::Int(n), "Integer") => n.to_string() == want["i"].as_str().unwrap(),
            _ => false,
        };
        assert!(same, "case {i}: Rust {got:?}, Rails {want}: {case}");
        assert_eq!(amount::quote_amount_limit_reached(&o.primary, &bot).unwrap(), case["reached"] == true, "case {i}: {case}");
    }
}
