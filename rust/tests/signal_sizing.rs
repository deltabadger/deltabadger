//! Plan 8a T4 / r2 §R6: a signal bot's sizing replayed against Rails' own answers (script/rust/record_signal_vectors.rb):
//! #signal_size (a webhook rule), #api_order_data (an API caller's amount), #calculate_best_amount_info on either, and
//! #venue_takes?, on Kraken, Binance, KuCoin and Alpaca, exact on BigDecimal.
use deltabadger::engine::amount::{self, TxKind};
use deltabadger::engine::model::{SignalRule, Ticker};
use deltabadger::engine::{signal, venue_rules};
use deltabadger::ruby::BigDec;
use deltabadger::venue::Side;

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

fn ticker(t: &serde_json::Value) -> Ticker {
    let n = |k: &str| t[k].as_str().unwrap().parse::<i64>().unwrap();
    Ticker { id: 1, ticker: "BTCX".into(), base_code: "BTC".into(), quote_code: "X".into(), base_symbol: "BTC".into(), quote_symbol: "X".into(), exchange_name: "X".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: n("base_decimals"), quote_decimals: n("quote_decimals"), price_decimals: n("price_decimals"),
             minimum_base_size: bd(t["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(t["minimum_quote_size"].as_str().unwrap()),
             trading_enabled: true, available: true, crypto: true }
}

#[test]
fn every_recorded_rails_signal_sizing_is_reproduced() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/signal_sizing.json")).unwrap();
    for (venue, exchange, expected_below) in [("kraken", "Exchanges::Kraken", 110), ("binance", "Exchanges::Binance", 125), ("kucoin", "Exchanges::Kucoin", 107), ("alpaca", "Exchanges::Alpaca", 98)] {
        let logic = venue_rules::for_exchange(exchange).unwrap().minimum_logic;
        let cases = vectors[venue].as_array().unwrap();
        let mut below_n = 0;
        for c in cases {
            let t = ticker(&vectors["tickers"][c["t"].as_u64().unwrap() as usize]);
            let side = if c["side"] == "buy" { Side::Buy } else { Side::Sell };
            let price = bd(c["price"].as_str().unwrap());
            let amount = bd(c["amount"].as_str().unwrap());
            let (base, quote) = if c["kind"] == "signal" {
                let rule = SignalRule { id: 1, bot_id: 1, side, percentage: c["amount_type"] == "percentage", amount, enabled: true, last_triggered_at: None };
                let held = bd(c["balance"].as_str().unwrap());
                let held = (side == Side::Sell || rule.percentage).then_some(&held);
                signal::size(&rule, &price, held).unwrap()
            } else {
                let in_base = c["amount_type"] == "base";
                assert_eq!(signal::venue_takes(side, in_base, logic), c["venue_takes"].as_bool().unwrap(), "venue_takes {venue} {c}");
                signal::api_order_data(&amount, in_base, &price).unwrap()
            };
            assert_eq!((base.to_s_f(), quote.to_s_f()), (c["base"].as_str().unwrap().to_string(), c["quote"].as_str().unwrap().to_string()), "size {venue} {c}");
            let (plan, below) = amount::size_order(&t, side, &price, &base, &quote, logic, TxKind::Regular).unwrap();
            let info = &c["amount_info"];
            assert_eq!(if plan.quote_type { "quote" } else { "base" }, info["amount_type"].as_str().unwrap(), "amount_type {venue} {c}");
            assert_eq!((if plan.quote_type { &plan.quote_amount } else { &plan.amount }).to_s_f(), info["amount"].as_str().unwrap(), "amount {venue} {c}");
            assert_eq!(below.to_string(), info["below_minimum_amount"].as_str().unwrap(), "below_minimum {venue} {c}");
            assert_eq!(plan.kind, TxKind::Regular);
            if below { below_n += 1; }
        }
        assert_eq!((cases.len(), below_n), (300, expected_below), "{venue}: every case replayed, the floor exercised");
    }
}

/// r2 §R6: where the venue takes the caller's denomination, `quote_type` and `volume` change together, the volume floored at
/// that side's decimals, so the two can never disagree.
#[test]
fn a_denomination_switch_rewrites_the_volume_with_it() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/signal_sizing.json")).unwrap();
    let t = ticker(&vectors["tickers"][0]); // base 5, quote 8 decimals
    let (plan, _) = amount::size_order(&t, Side::Buy, &bd("64000.11"), &bd("0.0015625"), &bd("100.000001234"), venue_rules::KRAKEN.minimum_logic, TxKind::Regular).unwrap();
    let quote = plan.in_denomination(&bd("100.000001234"), true).unwrap();
    assert_eq!((quote.quote_type, quote.volume.to_s_f()), (true, "100.00000123".to_string()));
    let base = plan.in_denomination(&bd("0.0015625"), false).unwrap();
    assert_eq!((base.quote_type, base.volume.to_s_f()), (false, "0.00156".to_string()));
    assert_eq!((base.amount.clone(), base.quote_amount.clone(), base.price.clone()), (plan.amount.clone(), plan.quote_amount.clone(), plan.price.clone()), "only what is sent changes");
}

#[test]
fn the_signal_sizing_port_is_the_ruby_that_was_recorded() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("fixtures/signal_sizing.json")).unwrap();
    for (file, recorded) in vectors["sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-record with script/rust/repin and re-check the signal port");
    }
}
