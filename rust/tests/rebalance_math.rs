//! Replays script/rust/record_rebalance_math.rb: Rails' composition_targets, drift, due check, extreme entries and sell
//! ranking over live valuations with Float and String Alpaca prices, decimal(10,6) weights, ties and exact thresholds.
use deltabadger::engine::basket::target_from_sql;
use deltabadger::engine::model::Ticker;
use deltabadger::engine::rebalance::{self, Direction, Entry, Member};
use deltabadger::figures::dec::Dec;
use deltabadger::figures::num::Num;
use deltabadger::figures::walk::{AssetValue, Breakdown, Metrics};
use deltabadger::ruby::BigDec;
use rusqlite::types::ValueRef;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

fn ticker(asset_id: i64, sym: &str) -> Ticker {
    Ticker { id: asset_id, ticker: format!("{sym}/USD"), base_code: sym.into(), quote_code: "USD".into(), base_symbol: sym.into(), quote_symbol: "USD".into(),
             exchange_name: "Alpaca".into(), base_asset_id: asset_id, quote_asset_id: 0, base_decimals: 9, quote_decimals: 2, price_decimals: 9,
             minimum_base_size: bd("0.000000001"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true }
}

fn raw_target(v: &Value) -> Option<BigDec> {
    match v {
        Value::Null => target_from_sql(ValueRef::Null),
        Value::Number(n) if n.is_f64() => target_from_sql(ValueRef::Real(n.as_f64().unwrap())),
        Value::Number(n) => target_from_sql(ValueRef::Integer(n.as_i64().unwrap())),
        Value::String(s) => target_from_sql(ValueRef::Text(s.as_bytes())),
        other => panic!("target {other}"),
    }.unwrap()
}

/// The case's inputs as figures::live leaves them: every holding in the breakdown, keyed by its asset; a value for each
/// positive holding of a member with a ticker and a trade (amount × the price as the venue sent it).
fn inputs(c: &Value) -> (Metrics, Vec<Member>, Vec<i64>) {
    let id = |sym: &str| c["asset_ids"][sym].as_i64().unwrap();
    let members: Vec<Member> = c["members"].as_array().unwrap().iter().filter(|m| m["in_index"] == true).map(|m| {
        let sym = m["sym"].as_str().unwrap();
        Member { asset_id: id(sym), symbol: sym.into(), target: raw_target(&m["raw"]), ticker: (m["tradeable"] == true).then(|| ticker(id(sym), sym)) }
    }).collect();
    let tradeable = |sym: &str| c["members"].as_array().unwrap().iter().any(|m| m["sym"] == sym && m["tradeable"] == true);
    let mut data = Metrics::empty();
    data.prices_stale = c["expected"]["stale"] == true;
    for (sym, amount) in c["holdings"].as_object().unwrap() {
        let amount = Num::Dec(Dec::parse(amount.as_str().unwrap()).unwrap());
        let zero = Num::Int(0);
        data.asset_breakdown.push((sym.clone(), Breakdown { amount: amount.clone(), quote_invested: zero.clone(), tax_basis: zero.clone(), tax_units: zero.clone(), tax_cost_unknown: false }));
        data.key_assets.push((sym.clone(), Some(id(sym))));
        // A Float price is recorded as its Float#to_s (Rails' JSON encoder would drop its 17th digit): sent here as the JSON
        // number Alpaca sent.
        let price = match &c["prices"][sym] { Value::Object(f) => json!(f["float"].as_str().unwrap().parse::<f64>().unwrap()), p => p.clone() };
        let price = &price;
        if data.prices_stale || !amount.is_positive() || !tradeable(sym) || price.is_null() { continue; }
        let price = Num::Dec(Dec::venue_number(price).unwrap()); // as page_market reads Alpaca's trade
        data.asset_values.push((sym.clone(), AssetValue { amount: amount.clone(), quote_invested: zero.clone(), current_value: amount.mul(&price).unwrap(),
            current_price: price, avg_price: zero.clone(), pnl_percentage: zero.clone(), tax_basis: zero.clone(), harvestable: false }));
    }
    let locked = c["locked"].as_array().map(|l| l.iter().map(|s| id(s.as_str().unwrap())).collect()).unwrap_or_default();
    (data, members, locked)
}

fn row(e: &Entry) -> Value {
    json!({ "asset_id": e.asset_id, "symbol": e.symbol, "amount": e.amount.to_s_f(), "value": e.value.to_s_f(), "target": e.target.to_s_f() })
}

#[test]
fn every_recorded_rails_rebalance_valuation_and_ranking_is_reproduced() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/rebalance_math.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let (mut targeted, mut due, mut ties, mut sold, mut bought) = (0, 0, 0, 0, 0);
    for (n, c) in cases.iter().enumerate() {
        let e = &c["expected"];
        let (data, members, locked) = inputs(c);
        let entries = rebalance::composition_targets(&data, &members, &locked).unwrap();
        let got = entries.as_ref().map(|es| es.iter().map(row).collect::<Vec<_>>());
        assert_eq!(got.map_or(Value::Null, Value::from), e["targets"], "case {n} targets: {c}");
        let drift = rebalance::drift(entries.as_deref());
        assert_eq!(drift.as_ref().map(BigDec::to_s_f).map_or(Value::Null, Value::from), e["drift"], "case {n} drift: {c}");
        let is_due = rebalance::due(&c["settings"], &json!({}), false, drift.as_ref()).unwrap();
        assert_eq!(Value::from(is_due), e["due"], "case {n} due: {c}");
        for (dir, key) in [(Direction::Max, "max"), (Direction::Min, "min")] {
            let (i, total) = rebalance::extreme_entry(entries.as_deref(), dir);
            let asset = i.map(|i| entries.as_ref().unwrap()[i].asset_id);
            assert_eq!(json!([asset, total.to_s_f()]), e[key], "case {n} {key}: {c}");
        }
        let withdrawal = bd(c["withdrawal"].as_str().unwrap_or("0"));
        let ranking = entries.as_ref().map(|es| rebalance::sell_ranking(es, &withdrawal).into_iter().map(|i| es[i].asset_id).collect::<Vec<_>>());
        assert_eq!(ranking.map_or(Value::Null, |r| json!(r)), e["ranking"], "case {n} ranking: {c}");
        // The legs' order data at the case's venue reads: price nil or not positive, or nothing to trade, is no order.
        let read = |side: &str, sym: &str| c[side][sym].as_str().map(bd);
        let sell = rebalance::sell_target(entries.as_deref()).and_then(|(i, excess)| {
            let entry = &entries.as_ref().unwrap()[i];
            let sym = &entry.ticker.as_ref().unwrap().base_code;
            let price = read("bids", sym).filter(BigDec::is_positive)?;
            let amount = rebalance::least([excess.div(&price).unwrap(), read("free", sym).unwrap_or_else(BigDec::zero)]).unwrap();
            amount.is_positive().then(|| json!([entry.asset_id, amount.to_s_f(), (&amount * &price).to_s_f(), price.to_s_f()]))
        });
        assert_eq!(sell.unwrap_or(Value::Null), e["sell"], "case {n} sell: {c}");
        let owed = bd(c["owed"].as_str().unwrap_or("0"));
        let buy = if !owed.is_positive() { None } else { rebalance::buy_target(entries.as_deref(), &owed).and_then(|(i, shortfall)| {
            let entry = &entries.as_ref().unwrap()[i];
            let price = read("asks", &entry.ticker.as_ref().unwrap().base_code).filter(BigDec::is_positive)?;
            let spend = rebalance::least([owed.clone(), shortfall, read("free", "USD").unwrap_or_else(BigDec::zero)]).unwrap();
            spend.is_positive().then(|| json!([entry.asset_id, spend.div(&price).unwrap().to_s_f(), spend.to_s_f(), price.to_s_f()]))
        }) };
        if buy.is_some() { bought += 1; }
        if e["sell"] != Value::Null { sold += 1; }
        assert_eq!(buy.unwrap_or(Value::Null), e["buy"], "case {n} buy: {c}");
        if let Some(es) = &entries {
            targeted += 1;
            let keys: Vec<_> = es.iter().map(|x| rebalance::deviation(x, &es.iter().fold(BigDec::zero(), |a, x| &a + &x.value))).collect();
            if keys.iter().enumerate().any(|(i, k)| keys[..i].iter().any(|j| j.0 == k.0)) { ties += 1; }
        }
        if is_due { due += 1; }
    }
    // The grid holds what it says it holds.
    assert!(cases.len() >= 10_000 && targeted > 5_000 && due > 1_000 && ties > 200 && sold > 1_000 && bought > 1_000,
            "{} cases, {targeted} targeted, {due} due, {ties} with tied deviations, {sold} sells, {bought} buys", cases.len());
}

#[test]
fn the_rebalance_port_is_the_ruby_that_was_recorded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let fixture: Value = serde_json::from_str(include_str!("fixtures/rebalance_math.json")).unwrap();
    for (file, recorded) in fixture["sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-record with script/rust/repin and re-check the rebalance port");
    }
}

// ---- 6b T5: the pending state in Rails' exact shape ----

#[test]
fn the_pending_state_round_trips_in_rails_shape() {
    use deltabadger::engine::rebalance::{pending, Pending};
    let p = Pending { phase: "buying".into(), sell_transaction_id: Some(12), buy_transaction_id: None, remaining_quote_amount: Some(BigDec::parse("99").unwrap()), buy_attempted: true };
    // set_rebalance_pending!: `remaining_quote_amount&.to_d&.to_s('F')`, `buy_attempted.present?`.
    assert_eq!(p.to_json(), json!({ "phase": "buying", "sell_transaction_id": 12, "buy_transaction_id": null, "remaining_quote_amount": "99.0", "buy_attempted": true }));
    assert_eq!(pending(&json!({ "rebalance_pending": p.to_json() })).unwrap(), Some(p));
    assert_eq!(pending(&json!({ "rebalance_pending": {} })).unwrap(), None, "an empty Hash is not present?");
    assert_eq!(pending(&json!({ "rebalance_pending": "selling" })).unwrap(), None, "only a Hash is a pending state");
    let rails = json!({ "rebalance_pending": { "phase": "selling", "sell_transaction_id": "5", "remaining_quote_amount": null } });
    let read = pending(&rails).unwrap().unwrap();
    assert_eq!((read.sell_transaction_id, read.remaining_quote_amount, read.buy_attempted), (Some(5), None, false));
    assert!(pending(&json!({ "rebalance_pending": { "phase": "selling", "sell_transaction_id": [1] } })).is_err(), "never guessed");
}
