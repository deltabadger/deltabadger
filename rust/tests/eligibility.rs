mod common;
use common::install;
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::eligibility;
use serde_json::json;

fn plain() -> BotSpec { BotSpec::weekly(60.0, "2026-09-01 10:00:00") }

#[test]
fn the_slice_is_eligible_with_its_two_options() {
    let (_d, o, s) = install();
    let a = seed::insert_bot(&o.primary, &s, &plain());
    let b = seed::insert_bot(&o.primary, &s, &plain().with("limit_ordered", json!(true)).with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!(20.0)));
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![a, b]);
}

fn closed(at: &str) -> TxSpec {
    TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("O{at}")), order_type: 0, amount: Some("0.002"), quote_amount: Some("100"),
             price: Some("50000"), quote_amount_exec: Some("100"), amount_exec: Some("0.002"), created_at: at.into() }
}

/// 6a: a one-asset seller on Kraken or Binance runs, and a history holding its sells is read by the same normalized walk.
#[test]
fn a_one_asset_seller_with_sells_in_its_history_is_eligible_on_kraken_and_binance() {
    for venue in ["Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"] {
        let (_d, o, s) = install();
        o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
        let id = seed::insert_bot(&o.primary, &s, &plain().with("direction", json!("selling")).with("sell_denomination", json!("base")).with("sell_amount", json!(0.001)));
        seed::insert_tx(&o.primary, &s, id, &closed("2026-09-02 10:00:00"));
        let sold = seed::insert_tx(&o.primary, &s, id, &closed("2026-09-03 10:00:00"));
        o.primary.execute("UPDATE transactions SET side = 1, amount = '0.0005', amount_exec = '0.0005', quote_amount = '30', quote_amount_exec = '30', price = '60000' WHERE id = ?1", [sold]).unwrap();
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.is_empty(), "{venue}: {:?}", r.problems);
        assert_eq!(r.eligible, vec![id], "{venue}");
        let bot = deltabadger::engine::model::load_bot(&o.primary, id).unwrap();
        let walk = deltabadger::engine::basket::walk(&o.primary, &bot, chrono::Utc::now()).unwrap();
        assert_eq!(walk.amounts[&s.btc].to_s_f(), "0.0015", "{venue}: bought 0.002, sold 0.0005");
    }
}

/// 6a Alpaca: its free base is qty_available through the position identity, so a one-asset seller runs there too.
#[test]
fn a_one_asset_seller_with_sells_in_its_history_is_eligible_on_alpaca() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &plain().with("direction", json!("selling")).with("sell_denomination", json!("base")).with("sell_amount", json!(0.001)));
    seed::insert_tx(&o.primary, &s, id, &closed("2026-09-02 10:00:00"));
    let sold = seed::insert_tx(&o.primary, &s, id, &closed("2026-09-03 10:00:00"));
    o.primary.execute("UPDATE transactions SET side = 1, amount = '0.0005', amount_exec = '0.0005', quote_amount = '30', quote_amount_exec = '30', price = '60000' WHERE id = ?1", [sold]).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
}

/// 6b: a wider Alpaca basket sells (the ranked DCA-out), rows a merge brought from another venue included: live_free_balance
/// caps what it sells here at its own units.
#[test]
fn a_two_asset_seller_with_foreign_rows_is_eligible_on_alpaca() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &plain().with("direction", json!("selling")).with("sell_denomination", json!("quote")).with("sell_quote_amount", json!(50.0))
        .weights(&[(s.btc, 0.5), (eth, 0.5)]));
    seed::insert_tx(&o.primary, &s, id, &closed("2026-09-02 10:00:00"));
    let foreign = seed::insert_tx(&o.primary, &s, id, &closed("2026-09-03 10:00:00"));
    o.primary.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '2026-01-01', '2026-01-01')", []).unwrap();
    o.primary.execute("UPDATE transactions SET exchange_id = last_insert_rowid() WHERE id = ?1", [foreign]).unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.merged_history_until_id', ?1) WHERE id = ?2", rusqlite::params![foreign, id]).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
}

type Make = Box<dyn Fn(&rusqlite::Connection, &seed::Seeded) -> i64>;

#[test]
fn each_thing_outside_the_slice_is_refused_with_its_reason() {
    let cases: Vec<(&str, Make)> = vec![
        ("price_drop_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("price_drop_limited", json!(true))))),
        ("indicator_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("indicator_limited", json!(true))))),
        // Lifting the starting time left the suffix rule whole: any other `*_limited`, `*_ordered`, `*_intervaled` refuses.
        ("stop_ordered", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("stop_ordered", json!(true))))),
        ("volume_intervaled", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("volume_intervaled", json!("1"))))),
        ("base_amount_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("base_amount_limited", json!(true))))),
        // A starting time runs only as a fresh start leaves it (start_at = started_at); this one's start_at is elsewhere.
        ("start_time_enabled", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("start_time_enabled", json!(true)).with("start_at", json!("2020-01-01T00:00:00Z"))))),
        ("rebalance_threshold [1] is not a number", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("rebalance_enabled", json!(true)).with("rebalance_threshold", json!([1]))))),
        ("direction \"sideways\"", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("direction", json!("sideways"))))),
        ("sell_interval \"fortnight\"", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("direction", json!("selling")).with("sell_interval", json!("fortnight"))))),
        ("sell_amount is negative", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("direction", json!("selling")).with("sell_amount", json!(-1))))),
        ("sell cadence under a second", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("direction", json!("selling")).with("sell_denomination", json!("base"))
            .with("sell_interval", json!("hour")).with("sell_amount", json!(1)).with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!(20.0)).with("smart_interval_base_amount", json!(0.0001))))),
        ("selling with 1 order(s) placed on another venue", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain().with("direction", json!("selling")));
            seed::insert_tx(c, s, id, &closed("2026-09-02 10:00:00"));
            c.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Binance', 'Binance', '2026-01-01', '2026-01-01')", []).unwrap();
            c.execute("UPDATE transactions SET exchange_id = last_insert_rowid() WHERE bot_id = ?1", [id]).unwrap(); id })),
        ("liquidation_pending", Box::new(|c, s| seed::insert_bot(c, s, &plain().transient("liquidation_pending", json!({"state": "x"}))))),
        ("wash_sale", Box::new(|c, s| { c.execute("UPDATE users SET wash_sale_enabled = 1", []).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("category", Box::new(|c, s| { c.execute("UPDATE assets SET category = 'stock' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("instrument", Box::new(|c, s| { c.execute("UPDATE assets SET instrument_type = 'tokenized' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("type", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain()); c.execute("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = ?1", [id]).unwrap(); id })),
        ("smart interval amount", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("smart_intervaled", json!(true))))),
        ("smart interval amount", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!("20"))))),
        // A precision outside 0..=40 is a data problem: refused here, never used as a rounding scale.
        ("price_decimals 1000000000", Box::new(|c, s| { c.execute("UPDATE tickers SET price_decimals = 1000000000", []).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("base_decimals -1", Box::new(|c, s| { c.execute("UPDATE tickers SET base_decimals = -1", []).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("quote_decimals 41", Box::new(|c, s| { c.execute("UPDATE tickers SET quote_decimals = 41", []).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("LIQUIDATION/REDEPLOY", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain());
            c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                       VALUES (?1, ?2, 'OLIQ', 0, 1, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01 00:00:00', '2026-09-01 00:00:00')", rusqlite::params![id, s.exchange_id]).unwrap(); id })),
        ("LIQUIDATION order", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain());
            c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                       VALUES (?1, ?2, 'OLIQ2', 0, 4, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01 00:00:00', '2026-09-01 00:00:00')", rusqlite::params![id, s.exchange_id]).unwrap(); id })),
    ];
    for (reason, make) in cases {
        let (_d, o, s) = install();
        let id = make(&o.primary, &s);
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains(reason)), "{reason}: {:?}", r.problems);
        assert!(r.eligible.is_empty(), "{reason}");
    }
}

#[test]
fn an_active_rule_refuses_the_install() {
    let (_d, o, s) = install();
    seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("INSERT INTO rules (type, status, user_id, settings, created_at, updated_at) VALUES ('Rules::Withdrawal', 1, ?1, '{}', '2026-01-01', '2026-01-01')", [s.user_id]).unwrap();
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains("rule")));
}

#[test]
fn a_stopped_bot_outside_the_slice_refuses_only_when_it_has_pending_work() {
    let (_d, o, s) = install();
    let quiet = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain().with("price_drop_limited", json!(true)) });
    assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty(), "a quiet stopped bot is Rails' business later, not now");
    seed::insert_tx(&o.primary, &s, quiet, &TxSpec { status: 0, external_status: Some(1), external_id: Some("OOPEN".into()), order_type: 1,
        amount: Some("0.001"), quote_amount: None, price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-20 10:00:00".into() });
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains(&format!("bot {quiet}")) && p.contains("outstanding order")));
    // A stopped Kraken rebalancer is this engine's (plan CB T4): no Rails work in it.
    let rebalancer = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain().with("rebalance_enabled", json!(true)) });
    assert!(!eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains(&format!("bot {rebalancer}"))));
    for status in [3, 7] { // deleted, archived: their orders are still at the venue
        let gone = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..plain().with("price_drop_limited", json!(true)) });
        seed::insert_tx(&o.primary, &s, gone, &TxSpec { status: 0, external_status: Some(0), external_id: Some(format!("OGONE{status}")), order_type: 0,
            amount: None, quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-20 10:00:00".into() });
        assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains(&format!("bot {gone}"))), "status {status}");
    }
}

#[test]
fn a_missing_user_row_never_passes() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("PRAGMA foreign_keys = OFF", []).unwrap();
    o.primary.execute("DELETE FROM users", []).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.eligible.is_empty());
    assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("user not found")) || r.unreadable.iter().any(|(i, _)| *i == id), "{:?}", r.problems);
}

#[test]
fn index_assets_refuse_the_bot() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("INSERT INTO bot_index_assets (asset_id, bot_id, ticker_id, created_at, updated_at) VALUES (?1, ?2, ?3, '2026-01-01', '2026-01-01')", [s.quote, id, s.ticker_id]).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("index assets present (1)")), "{:?}", r.problems);
    assert!(r.eligible.is_empty());
}

#[test]
fn a_retrying_rule_refuses_the_install() {
    let (_d, o, s) = install();
    seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("INSERT INTO rules (type, status, user_id, settings, created_at, updated_at) VALUES ('Rules::Withdrawal', 5, ?1, '{}', '2026-01-01', '2026-01-01')", [s.user_id]).unwrap();
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains("rule")));
}

#[test]
fn an_index_asset_row_for_the_bots_own_asset_is_tolerated() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("INSERT INTO bot_index_assets (asset_id, bot_id, ticker_id, created_at, updated_at) VALUES (?1, ?2, ?3, '2026-01-01', '2026-01-01')", [s.btc, id, s.ticker_id]).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
}

#[test]
fn a_limit_bot_whose_distance_is_not_a_number_is_refused() {
    for garbage in [json!("abc"), json!(true), json!([1])] {
        let (_d, o, s) = install();
        let id = seed::insert_bot(&o.primary, &s, &plain().with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", garbage.clone()));
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("limit_order_pcnt_distance")), "{garbage}: {:?}", r.problems);
    }
    let (_d, o, s) = install();
    let blank = seed::insert_bot(&o.primary, &s, &plain().with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!("")));
    assert_eq!(eligibility::check_install(&o.primary).unwrap().eligible, vec![blank], "blank is Rails' 0.001 default");
}

#[test]
fn a_working_bot_that_was_never_started_is_refused() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { started_at: None, ..plain() });
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("started_at")), "{:?}", r.problems);
    assert!(r.eligible.is_empty());
}

fn refused(c: &rusqlite::Connection, id: i64) -> Option<String> {
    eligibility::check_install(c).unwrap().problems.into_iter().find(|p| p.starts_with(&format!("bot {id} ")))
}

#[test]
fn an_unreadable_rebalance_threshold_refuses_only_where_evaluate_rebalancers_would_pick_the_bot() {
    // Bot::EvaluateRebalancersJob#candidates: DcaIndex/DcaMultiAsset, not deleted or archived. A stopped bot still rebalances.
    for (status, ty, refuses) in [(2, "Bots::DcaMultiAsset", true), (0, "Bots::DcaMultiAsset", true), (2, "Bots::DcaIndex", true),
                                  (3, "Bots::DcaMultiAsset", false), (7, "Bots::DcaMultiAsset", false), (2, "Bots::DcaSingleAsset", false)] {
        let (_d, o, s) = install();
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..plain().with("rebalance_enabled", json!(true)).with("rebalance_threshold", json!([1])) });
        o.primary.execute("UPDATE bots SET type = ?1 WHERE id = ?2", rusqlite::params![ty, id]).unwrap();
        assert_eq!(refused(&o.primary, id).is_some(), refuses, "status {status} {ty}: {:?}", refused(&o.primary, id));
    }
}

#[test]
fn an_abandoned_liquidation_the_user_accounted_for_no_longer_refuses() {
    // Bot::LiquidationState#unresolved_liquidation_orders skips the ids in transient liquidation_resolved_orders.
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain() });
    let abandon = |ext: &str| -> i64 {
        o.primary.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                           VALUES (?1, ?2, ?3, 0, 4, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01 00:00:00', '2026-09-01 00:00:00')", rusqlite::params![id, s.exchange_id, ext]).unwrap();
        o.primary.last_insert_rowid()
    };
    let first = abandon("OAB1");
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.liquidation_resolved_orders', json_array(?1)) WHERE id = ?2", [first, id]).unwrap();
    assert_eq!(refused(&o.primary, id), None, "an attested order is accounted for");
    abandon("OAB2");
    assert!(refused(&o.primary, id).is_some_and(|p| p.contains("1 unresolved abandoned LIQUIDATION")), "{:?}", refused(&o.primary, id));
}

#[test]
fn the_refusal_names_the_bots_real_status() {
    for (status, label) in [(0, "created"), (2, "stopped"), (3, "deleted"), (7, "archived")] {
        let (_d, o, s) = install();
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..plain().with("price_drop_limited", json!(true)) });
        seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(1), external_id: Some("OOPEN".into()), order_type: 1,
            amount: Some("0.001"), quote_amount: None, price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-20 10:00:00".into() });
        let p = refused(&o.primary, id).expect("an outstanding order refuses");
        assert!(p.starts_with(&format!("bot {id} ({label}) ")), "{p}");
    }
}

#[test]
fn the_alpaca_crypto_slice_is_eligible_with_its_two_options() {
    let (_d, o, s) = common::install_alpaca();
    let a = seed::insert_bot(&o.primary, &s, &plain());
    let b = seed::insert_bot(&o.primary, &s, &plain().with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)));
    let c = seed::insert_bot(&o.primary, &s, &plain().with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!(20.0)));
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![a, b, c]);
}

#[test]
fn malformed_asset_categories_incomplete_indices_and_other_quotes_are_refused() {
    let cases: Vec<(&str, Make)> = vec![
        ("category stock", Box::new(|c, s| { c.execute("UPDATE assets SET category = 'stock' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("category etf", Box::new(|c, s| { c.execute("UPDATE assets SET category = 'etf' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("instrument tokenized", Box::new(|c, s| { c.execute("UPDATE assets SET instrument_type = 'tokenized' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("no ticker for the asset", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("allocations", json!({ s.btc.to_string(): 0.5, s.quote.to_string(): 0.5 }))))),
        ("index_type None", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain()); c.execute("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = ?1", [id]).unwrap(); id })),
        ("quote USDT (Alpaca: only USD)", Box::new(|c, s| { c.execute("UPDATE assets SET symbol = 'USDT' WHERE id = ?1", [s.quote]).unwrap(); seed::insert_bot(c, s, &plain()) })),
    ];
    for (reason, make) in cases {
        let (_d, o, s) = common::install_alpaca();
        let id = make(&o.primary, &s);
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains(reason)), "{reason}: {:?}", r.problems);
        assert!(r.eligible.is_empty(), "{reason}");
    }
}

#[test]
fn any_other_exchange_is_refused() {
    let (_d, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Bybit', name = 'Bybit'", []).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("a bot on an exchange other than Alpaca, Kraken, Binance, Binance.US, KuCoin, Bitvavo, Gemini, Bitget, MEXC, Hyperliquid, Bybit, Bitrue and Coinbase")), "{:?}", r.problems);
}

#[test]
fn the_guard_refuses_a_write_that_makes_the_install_ineligible_and_names_the_bot() {
    use deltabadger::engine::model;
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    // A write the engine can run: the guard says commit.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount', 70) WHERE id = ?1", [id]).unwrap();
    assert!(eligibility::guard(&tx, &seed::cipher(), Some(id)).is_ok());
    tx.commit().unwrap();
    // One it cannot: refused in check's words, naming the bot. The caller rolls back.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET settings = json_set(settings, '$.price_drop_limited', json('true')) WHERE id = ?1", [id]).unwrap();
    let refused = eligibility::guard(&tx, &seed::cipher(), Some(id)).unwrap_err();
    let line = format!("bot {id} (scheduled): price_drop_limited");
    assert!(matches!(&refused, eligibility::Refusal::Ineligible(p) if p == &vec![line.clone()]), "{refused:?}");
    assert_eq!(refused.reason(), line);
    assert_eq!(refused.message(), format!("this install uses things only the full app runs:\n{line}"));
    drop(tx); // rolled back
    let limited: Option<i64> = o.primary.query_row("SELECT json_extract(settings, '$.price_drop_limited') FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(limited, None, "nothing of the refused write remains");
}

#[test]
fn a_report_refuses_problems_first_then_unreadable_rows_in_checks_words() {
    let unreadable = vec![(7, "Data(\"x\")".to_string())];
    let r = eligibility::Report { eligible: vec![1], problems: vec![], unreadable: unreadable.clone(), notes: vec![] };
    let refused = r.refusal().unwrap_err();
    assert_eq!(refused.message(), format!("unreadable bot rows: {unreadable:?}"), "check's own format");
    assert_eq!(refused.reason(), "bot 7: unreadable (Data(\"x\"))");
    let r = eligibility::Report { eligible: vec![], problems: vec!["bot 3 (stopped): x".into()], unreadable, notes: vec![] };
    assert!(matches!(r.refusal(), Err(eligibility::Refusal::Ineligible(_))), "problems come first, as check reports them");
    let r = eligibility::Report { eligible: vec![1, 2], problems: vec![], unreadable: vec![], notes: vec![] };
    assert_eq!(r.refusal().unwrap(), vec![1, 2]);
}

/// The guard refuses what this build cannot trade, in `preflight`'s words, inside the write: a start with no key (Kraken,
/// Alpaca). Each refused start rolls back; a Kraken start with its key, and an Alpaca start with a live key, go through.
#[test]
fn the_guard_refuses_a_start_this_build_cannot_trade_and_the_start_rolls_back() {
    use deltabadger::engine::model;
    // Lifecycle#start as the web writes it (only from created or stopped), guarded before commit.
    let start = |c: &rusqlite::Connection, id: i64| -> Result<(), eligibility::Refusal> {
        let tx = model::immediate(c).unwrap();
        tx.execute("UPDATE bots SET status = 1 WHERE id = ?1 AND status IN (0, 2)", [id]).unwrap();
        eligibility::guard(&tx, &seed::cipher(), Some(id))?; // Err: `tx` is dropped, rolled back
        tx.commit().unwrap();
        Ok(())
    };
    let status = |c: &rusqlite::Connection, id: i64| -> i64 { c.query_row("SELECT status FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    let untradable = |r: &eligibility::Refusal, line: &str| matches!(r, eligibility::Refusal::Untradable(p) if p == &vec![line.to_string()]);

    // A Kraken bot with its key starts like in Rails (no live gate); without a key the start is refused and rolls back.
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain() });
    assert!(start(&o.primary, id).is_ok(), "a Kraken bot with a key starts");
    assert_eq!(status(&o.primary, id), 1);
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain() });
    o.primary.execute("DELETE FROM api_keys", []).unwrap();
    let refused = start(&o.primary, id).unwrap_err();
    assert!(untradable(&refused, &format!("bot {id}: no Kraken API key")), "{refused:?}");
    assert_eq!(status(&o.primary, id), 2, "rolled back");

    // Alpaca with a live key starts as in Rails; with no key it is refused.
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain() });
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")]).unwrap();
    assert!(start(&o.primary, id).is_ok(), "a live key starts");
    o.primary.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap();
    o.primary.execute("DELETE FROM api_keys", []).unwrap();
    let refused = start(&o.primary, id).unwrap_err();
    assert!(untradable(&refused, &format!("bot {id}: no Alpaca API key")), "{refused:?}");
    assert_eq!(status(&o.primary, id), 2, "rolled back");
}

/// When the check itself fails, the 422's reason is generic: the internal error is logged, never shown.
#[test]
fn a_guard_that_fails_itself_keeps_its_internal_error_out_of_the_reason() {
    use deltabadger::engine::model;
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("ALTER TABLE bots RENAME TO bots_gone", []).unwrap(); // the check cannot read its table
    let refused = eligibility::guard(&tx, &seed::cipher(), Some(id)).unwrap_err();
    assert!(matches!(refused, eligibility::Refusal::Failed(_)), "{refused:?}");
    assert_eq!(refused.reason(), "the check could not be completed");
    let reason = refused.reason();
    assert!(!reason.contains("bots") && !reason.contains("Sqlite"), "{reason}");
}

#[test]
fn an_alpaca_crypto_basket_is_eligible() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &plain().weights(&[(s.btc, 0.5), (eth, 0.3), (sol, 0.2)]));
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
}

/// A crypto member beside the seeded BTC/EUR on the install's venue, on its own SYMBOLEUR pair. Returns its asset id.
fn venue_member(c: &rusqlite::Connection, s: &seed::Seeded, symbol: &str) -> i64 {
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')",
              rusqlite::params![symbol.to_lowercase(), symbol]).unwrap();
    let asset = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
               VALUES (?1, ?2 || 'EUR', ?2, 'EUR', ?3, ?4, 8, 5, 2, '0.002', '0.5', 1, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.exchange_id, symbol, asset, s.quote]).unwrap();
    asset
}

/// P1-T0: Kraken and Binance buy baskets of two and three members, each member on its own pair.
#[test]
fn a_kraken_or_binance_basket_is_eligible() {
    for venue in ["Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"] {
        for members in [2, 3] {
            let (_d, o, s) = install();
            o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
            let (eth, sol) = (venue_member(&o.primary, &s, "ETH"), venue_member(&o.primary, &s, "SOL"));
            let weights: &[(i64, f64)] = if members == 2 { &[(s.btc, 0.7), (eth, 0.3)] } else { &[(s.btc, 0.5), (eth, 0.3), (sol, 0.2)] };
            let id = seed::insert_bot(&o.primary, &s, &plain().weights(weights).with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)));
            let r = eligibility::check_install(&o.primary).unwrap();
            assert!(r.problems.is_empty(), "{venue} {members}: {:?}", r.problems);
            assert_eq!(r.eligible, vec![id], "{venue} {members}");
        }
    }
}

/// Plan CB T2: the quote amount limit is venue-neutral (Bot::QuoteAmountLimitable): a capped basket is admitted on each crypto venue.
#[test]
fn a_crypto_basket_with_a_quote_amount_limit_is_admitted() {
    for venue in ["Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"] {
        let (_d, o, s) = install();
        o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
        let eth = venue_member(&o.primary, &s, "ETH");
        let id = seed::insert_bot(&o.primary, &s, &plain().weights(&[(s.btc, 0.5), (eth, 0.5)]).with("quote_amount_limited", json!(true))
            .with("quote_amount_limit", json!(500.0)).transient("quote_amount_limit_enabled_at", json!("2026-01-01T00:00:00.000Z")));
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.is_empty(), "{venue}: {:?}", r.problems);
        assert_eq!(r.eligible, vec![id], "{venue}");
        // An unreadable cap still refuses, on every venue.
        o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limit', 'bad')", []).unwrap();
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.iter().any(|p| p.contains("quote_amount_limit")), "{venue}: {:?}", r.problems);
    }
}

/// What a Kraken, Binance or KuCoin basket still may not do in this engine: an index bot that sells.
#[test]
fn a_kraken_or_binance_basket_outside_the_slice_is_refused_with_its_reason() {
    type Case = Box<dyn Fn(&rusqlite::Connection, &seed::Seeded, i64) -> i64>;
    for venue in ["Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"] {
        let cases: Vec<(String, Case)> = vec![
            ("an index bot that sells".into(), Box::new(move |c, s, _| { let id = seed::index_bot(c, s, "layer-1", 3, 0.0, false);
                c.execute("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling') WHERE id = ?1", [id]).unwrap(); id })),
        ];
        for (reason, make) in cases {
            let (_d, o, s) = install();
            o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
            let eth = venue_member(&o.primary, &s, "ETH");
            let id = make(&o.primary, &s, eth);
            let r = eligibility::check_install(&o.primary).unwrap();
            assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains(&reason)), "{venue} {reason}: {:?}", r.problems);
            assert!(r.eligible.is_empty(), "{venue} {reason}");
        }
    }
}

/// Plan CB T5: market-cap weights are derived from the assets table (Bots::DcaMultiAsset#apply_market_cap_weights), on every
/// venue the same way: a market-cap basket is admitted on Alpaca, Kraken, Binance and KuCoin.
#[test]
fn a_market_cap_basket_is_admitted_on_every_venue() {
    for venue in ["Exchanges::Alpaca", "Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"] {
        let (_d, o, s) = if venue == "Exchanges::Alpaca" { common::install_alpaca() } else { install() };
        if venue != "Exchanges::Alpaca" { o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap(); }
        let eth = if venue == "Exchanges::Alpaca" { seed::add_eth_sol(&o.primary, &s).0 } else { venue_member(&o.primary, &s, "ETH") };
        let id = seed::insert_bot(&o.primary, &s, &plain().weights(&[(s.btc, 0.7), (eth, 0.3)]).with("weighting", json!("market_cap")));
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.is_empty(), "{venue}: {:?}", r.problems);
        assert_eq!(r.eligible, vec![id], "{venue}");
    }
}

/// Plan CB T3: a basket of three that sells is admitted on Kraken, Binance and KuCoin (the ranked DCA-out is venue-neutral).
#[test]
fn a_wider_crypto_basket_that_sells_is_admitted() {
    for venue in ["Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"] {
        let (_d, o, s) = install();
        o.primary.execute("UPDATE exchanges SET type = ?1", [venue]).unwrap();
        let (eth, sol) = (venue_member(&o.primary, &s, "ETH"), venue_member(&o.primary, &s, "SOL"));
        let id = seed::insert_bot(&o.primary, &s, &plain().weights(&[(s.btc, 0.5), (eth, 0.3), (sol, 0.2)]).with("direction", json!("selling"))
            .with("sell_denomination", json!("quote")).with("sell_quote_amount", json!(100.0)));
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.is_empty(), "{venue}: {:?}", r.problems);
        assert_eq!(r.eligible, vec![id], "{venue}");
    }
}

/// Plan CI: a Top coins bot at EUR on Kraken, and a category bot quoted in BTC on Binance, are admitted on a data-api install,
/// with or without their `indices` row (a missing row is the tick's "Index not found", never an install refusal).
#[test]
fn a_crypto_index_bot_on_kraken_or_binance_is_admitted() {
    let (_d, o, s) = install();
    let top = seed::index_bot(&o.primary, &s, "ignored", 3, 0.5, false);
    o.primary.execute("UPDATE bots SET settings = json_remove(json_set(settings, '$.index_type', 'top'), '$.index_category_id') WHERE id = ?1", [top]).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![top]);

    let (_d, o, s) = install();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Binance', name = 'Binance'", []).unwrap();
    let eth = venue_member(&o.primary, &s, "ETH");
    o.primary.execute("UPDATE tickers SET ticker = 'ETHBTC', quote = 'BTC', quote_asset_id = ?1 WHERE base_asset_id = ?2", [s.btc, eth]).unwrap();
    let id = seed::index_bot(&o.primary, &s, "layer-1", 3, 0.0, false);
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_asset_id', ?1) WHERE id = ?2", [s.btc, id]).unwrap();
    seed::insert_index(&o.primary, "layer-1", &["bitcoin", "ethereum"], &json!({}));
    o.primary.execute("UPDATE indices SET source = 'coingecko'", []).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
}

/// Owner ruling O6 (2026-10-18): KuCoin admits Top coins and category index bots under the Kraken/Binance rule, at USDT and USDC.
#[test]
fn a_crypto_index_bot_on_kucoin_is_admitted_at_usdt_and_usdc() {
    for (quote, kind) in [("USDT", "top"), ("USDC", "category")] {
        let (_d, o, s) = install();
        o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Kucoin', name = 'KuCoin'", []).unwrap();
        o.primary.execute("UPDATE assets SET symbol = ?1, external_id = lower(?1), category = 'Cryptocurrency' WHERE id = ?2", rusqlite::params![quote, s.quote]).unwrap();
        o.primary.execute("UPDATE tickers SET ticker = 'BTC-' || ?1, quote = ?1", [quote]).unwrap();
        let id = seed::index_bot(&o.primary, &s, "layer-1", 3, 0.0, false);
        if kind == "top" {
            o.primary.execute("UPDATE bots SET settings = json_remove(json_set(settings, '$.index_type', 'top'), '$.index_category_id') WHERE id = ?1", [id]).unwrap();
        }
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.is_empty(), "{quote} {kind}: {:?}", r.problems);
        assert_eq!(r.eligible, vec![id], "{quote} {kind}");
    }
}

/// One history row of `bot`: closed, of `side`, `kind`, `asset`, with this external id.
fn history_row(c: &rusqlite::Connection, s: &seed::Seeded, bot: i64, side: i64, kind: &str, asset: Option<i64>, ext: &str) {
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, base_asset_id, transaction_type, \
               error_messages, bot_interval, bot_quote_amount, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 2, ?4, 0, ?5, ?6, '[]', 'week', 60, '2026-09-01 00:00:00', '2026-09-01 00:00:00')",
              rusqlite::params![bot, s.exchange_id, ext, side, asset, kind]).unwrap();
}

#[test]
fn a_basket_outside_what_the_engine_ports_is_refused_with_its_reason() {
    type Case = Box<dyn Fn(&rusqlite::Connection, &seed::Seeded, i64) -> i64>;
    let half = |s: &seed::Seeded, eth: i64| plain().weights(&[(s.btc, 0.5), (eth, 0.5)]);
    let cases: Vec<(&str, Case)> = vec![
        ("not 1", Box::new(|c, s, eth| seed::insert_bot(c, s, &plain().weights(&[(s.btc, 0.5), (eth, 0.3)])))),
        ("a weight this build does not read", Box::new(|c, s, eth| seed::insert_bot(c, s, &plain().with("allocations", json!({ s.btc.to_string(): "0.5", eth.to_string(): 0.5 }))))),
        ("101 assets (at most 100)", Box::new(|c, s, _| {
            let many: serde_json::Map<String, serde_json::Value> = (0..101).map(|i| ((5000 + i).to_string(), json!(1.0 / 101.0))).collect();
            seed::insert_bot(c, s, &plain().with("allocations", serde_json::Value::Object(many)))
        })),
        // An Alpaca basket's REBALANCE rows are read since 6b (tests/rebalance.rs); a liquidation is still not.
        ("1 LIQUIDATION/REDEPLOY row(s)", Box::new(move |c, s, eth| { let id = seed::insert_bot(c, s, &half(s, eth)); history_row(c, s, id, 1, "LIQUIDATION", Some(s.btc), "OLIQ"); id })),
        ("1 imported row(s)", Box::new(move |c, s, eth| { let id = seed::insert_bot(c, s, &half(s, eth)); history_row(c, s, id, 0, "REGULAR", Some(s.btc), "imported_1"); id })),
        ("1 order(s) recorded without base_asset_id", Box::new(move |c, s, eth| { let id = seed::insert_bot(c, s, &half(s, eth)); history_row(c, s, id, 0, "REGULAR", None, "ONOASSET"); id })),
        ("asset {btc} listed more than once", Box::new(|c, s, _| seed::insert_bot(c, s, &plain().with("allocations", json!({ s.btc.to_string(): 0.5, format!("0{}", s.btc): 0.5 }))))),
        ("1 split(s) recorded for its assets", Box::new(move |c, s, eth| {
            c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, raw_data, transacted_at, created_at, updated_at) \
                       VALUES (?1, ?2, 15, 'BTC', 0, '{\"corporate_action\":\"split\"}', '2026-09-01', '2026-09-01', '2026-09-01')", rusqlite::params![s.user_id, s.exchange_id]).unwrap();
            seed::insert_bot(c, s, &half(s, eth))
        })),
    ];
    for (reason, make) in cases {
        let (_d, o, s) = common::install_alpaca();
        let (eth, _) = seed::add_eth_sol(&o.primary, &s);
        let id = make(&o.primary, &s, eth);
        let reason = reason.replace("{btc}", &s.btc.to_string());
        let reason = reason.as_str();
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains(reason)), "{reason}: {:?}", r.problems);
        assert!(r.eligible.is_empty(), "{reason}");
    }
}

/// An exited member (in_index = false) is a holding only and never refuses the bot, whether settings still
/// list it (a delisting the tick marked) or not (a member the user removed). Its units are REGULAR buys with their asset.
#[test]
fn an_exited_member_that_holds_units_is_eligible() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &plain().weights(&[(s.btc, 0.5), (eth, 0.5)]));
    for (asset, in_index) in [(s.btc, 1), (eth, 0), (sol, 0)] {
        o.primary.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, exited_at, created_at, updated_at) \
                           VALUES (?1, ?2, (SELECT id FROM tickers WHERE base_asset_id = ?2), 0.5, ?3, CASE WHEN ?3 = 0 THEN '2026-09-01' END, '2026-09-01', '2026-09-01')",
                          rusqlite::params![id, asset, in_index]).unwrap();
    }
    history_row(&o.primary, &s, id, 0, "REGULAR", Some(eth), "OETH");
    history_row(&o.primary, &s, id, 0, "REGULAR", Some(sol), "OSOL");
    // These former members really hold one unit each; NULL fills are unreadable under R1.
    o.primary.execute("UPDATE transactions SET amount_exec=1, quote_amount_exec=10, price=10 WHERE bot_id=?1", [id]).unwrap();
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.unreadable.is_empty(), "{:?}", r.unreadable);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![id]);
}

#[test]
fn an_amount_limited_alpaca_bot_is_eligible_unless_its_limit_or_stamp_is_unreadable() {
    let limited = || plain().with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(100.0))
        .transient("quote_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z"));
    let (_d, o, s) = common::install_alpaca();
    let a = seed::insert_bot(&o.primary, &s, &limited());
    let b = seed::insert_bot(&o.primary, &s, &plain().with("quote_amount_limited", json!(true))); // no limit (1000), no stamp (counts nothing)
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.eligible, vec![a, b]);
    for (reason, spec) in [("quote_amount_limit \"100\" is not a number", limited().with("quote_amount_limit", json!("100"))),
                           ("quote_amount_limit_enabled_at", limited().transient("quote_amount_limit_enabled_at", json!("yesterday")))] {
        let (_d, o, s) = common::install_alpaca();
        let id = seed::insert_bot(&o.primary, &s, &spec);
        let r = eligibility::check_install(&o.primary).unwrap();
        if reason=="quote_amount_limit_enabled_at" {
            assert!(r.unreadable.iter().any(|(bad,e)|*bad==id && e.contains("Time(") && e.contains("yesterday")),"{:?}",r.unreadable);
            assert!(r.refusal().is_err());
        } else { assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains(reason)), "{reason}: {:?}", r.problems); }
    }
}

#[test]
fn check_and_the_takeover_refuse_an_install_whose_alpaca_tickers_are_stale() {
    use deltabadger::engine::{handover, EngineError};
    use deltabadger::{lease, store};
    let now: chrono::DateTime<chrono::Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    let dir = common::rails_install();
    let p = store::Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, now).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 09:00:01'", []).unwrap(); // 48h 59m 59s: inside the 49 h bound
    assert_eq!(eligibility::check_install_at(&o.primary, now).unwrap().eligible, vec![id]);
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 08:00:00'", []).unwrap(); // 50 h
    let r = eligibility::check_install_at(&o.primary, now).unwrap();
    assert!(r.eligible.is_empty());
    let p = r.problems.iter().find(|p| p.contains(&format!("bot {id}"))).expect("the bot is named");
    assert!(p.contains("Alpaca crypto tickers") && p.contains("exchange_assets.updated_at") && p.contains("50h 0m old") && p.contains("49h bound"), "{p}");
    assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty(), "the running engine's pass refuses the tick, not the install");
    match handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", now) {
        Err(EngineError::Ineligible(problems)) => assert!(problems.iter().any(|m| m.contains("Alpaca crypto tickers")), "{problems:?}"),
        other => panic!("the takeover must refuse a stale install: {:?}", other.map(|t| t.eligible)),
    }
}

/// Ticker::TechnicallyAnalyzable writes an all-time high with `update!` (technically_analyzable.rb:125),
/// moving tickers.updated_at with no catalog sync. That write must not make a stale catalog read fresh.
#[test]
fn an_all_time_high_write_cannot_make_a_stale_catalog_read_fresh() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let now: chrono::DateTime<chrono::Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 08:00:00'", []).unwrap(); // the last sync, 50 h ago
    o.primary.execute("UPDATE tickers SET ath = 70000, ath_updated_at = '2026-09-01 09:59:00', updated_at = '2026-09-01 09:59:00'", []).unwrap();
    let r = eligibility::check_install_at(&o.primary, now).unwrap();
    assert!(r.eligible.is_empty() && r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("50h 0m old")), "{:?}", r.problems);
}

/// With no sync stamp (no app_configs alpaca_crypto_listings_last_good_count: a self-hosted install, or
/// one that never synced) the verdict is unknown: noted, not refused.
#[test]
fn an_install_with_no_catalog_stamp_is_noted_not_refused() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("DELETE FROM app_configs WHERE key = 'alpaca_crypto_listings_last_good_count'", []).unwrap();
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2020-01-01 00:00:00'", []).unwrap();
    let r = eligibility::check_install_at(&o.primary, "2026-09-01T10:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(r.eligible, vec![id]);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(r.notes.iter().any(|n| n.contains(&format!("bot {id}")) && n.contains("reference data unknown") && n.contains("not refused")), "{:?}", r.notes);
    assert!(r.notes.iter().all(|n| !n.contains("Plan ")), "{:?}", r.notes);
}

#[test]
fn kraken_has_no_staleness_bound() {
    let (_d, o, s) = install(); // Kraken tickers stamped 2026-01-01, eight months before `now`
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let r = eligibility::check_install_at(&o.primary, "2026-09-01T10:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(r.eligible, vec![id]);
    assert!(r.notes.is_empty(), "Kraken is not measured at all, not measured as unknown: {:?}", r.notes);
}

/// The bound is inclusive: a catalog exactly 49 h old is fresh, one second more is stale.
#[test]
fn the_49_hour_bound_is_inclusive() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let now: chrono::DateTime<chrono::Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 09:00:00'", []).unwrap(); // exactly 49 h
    assert_eq!(eligibility::check_install_at(&o.primary, now).unwrap().eligible, vec![id]);
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 08:59:59'", []).unwrap(); // 49 h 0 m 1 s
    let r = eligibility::check_install_at(&o.primary, now).unwrap();
    assert!(r.eligible.is_empty() && r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("49h 0m old")), "{:?}", r.problems);
}

/// What `check`, `serve` and the takeover print names what is not supported, never an internal plan.
#[test]
fn refusals_and_notes_name_no_internal_plan() {
    let mut lines = vec![];
    let kraken: Vec<Make> = vec![
        Box::new(|c, s| { let id = seed::index_bot(c, s, "layer-1", 3, 0.0, false);
            c.execute("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling') WHERE id = ?1", [id]).unwrap(); id }),
    ];
    for make in kraken {
        let (_d, o, s) = install();
        make(&o.primary, &s);
        lines.extend(eligibility::check_install(&o.primary).unwrap().problems);
    }
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    seed::insert_bot(&o.primary, &s, &plain().weights(&[(s.btc, 0.5), (eth, 0.5)]).with("direction", json!("sideways")));
    o.primary.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, raw_data, transacted_at, created_at, updated_at) \
                       VALUES (?1, ?2, 15, 'BTC', 0, '{\"corporate_action\":\"split\"}', '2026-09-01', '2026-09-01', '2026-09-01')", rusqlite::params![s.user_id, s.exchange_id]).unwrap();
    o.primary.execute("DELETE FROM app_configs WHERE key = 'alpaca_crypto_listings_last_good_count'", []).unwrap();
    let r = eligibility::check_install_at(&o.primary, "2026-09-01T10:00:00Z".parse().unwrap()).unwrap();
    lines.extend(r.problems);
    lines.extend(r.notes);
    for needle in ["an index bot that sells", "direction", "split(s)"] {
        assert!(lines.iter().any(|l| l.contains(needle)), "{needle}: {lines:?}");
    }
    assert!(lines.iter().all(|l| !l.contains("Plan ")), "{lines:?}");
}

/// An intent written by an earlier engine build, before intents recorded what they were sent under, gets that snapshot at
/// takeover, from the bot's row: that build had no web UI and let nothing else write, so the row is what the order was
/// sent under. From then on the stranded rule holds for that bot like any other, stopped or not.
#[test]
fn a_legacy_intent_gets_its_snapshot_at_takeover_and_a_stopped_bot_stays_frozen() {
    use deltabadger::engine::{amount, handover, model, placement, FixedClock};
    use deltabadger::ruby::BigDec;
    let now: chrono::DateTime<chrono::Utc> = "2026-09-30T12:00:00Z".parse().unwrap();
    let (d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let bot = model::load_bot(&o.primary, id).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(64_000),
        &deltabadger::engine::venue_rules::ALPACA).unwrap() else { panic!("sized") };
    placement::begin(&o.primary, &bot, &plan, &FixedClock(now)).unwrap();
    // As the earlier build wrote it: no exchange, quote or allocations recorded.
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.exchange_id', \
                       '$.rust_placement.quote_asset_id', '$.rust_placement.allocations') WHERE id = ?1", [id]).unwrap();
    let l = deltabadger::lease::lock(&deltabadger::store::Paths::from_env(&|_| None, d.path()), now).unwrap();
    handover::take_over(&l, &o, &seed::cipher(), "0.2.0", now).unwrap();
    let v = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap();
    assert_eq!((v["exchange_id"].as_i64(), v["quote_asset_id"].as_i64()), (Some(bot.exchange_id), bot.quote_asset_id()), "backfilled from the row");
    assert_eq!(Some(&v["allocations"]), bot.settings.get("allocations"));
    let reconciling = format!("bot {id}: an order is still being reconciled; its composition, asset, exchange and quote cannot change until it settles");
    let reweigh = |tx: &rusqlite::Transaction| tx.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json(?1)) WHERE id = ?2",
        rusqlite::params![json!({ s.btc.to_string(): 0.9995 }).to_string(), id]).unwrap();
    // A stop and the weight change in one write: refused.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); // Lifecycle#stop, as the web writes it
    reweigh(&tx);
    let refused = eligibility::guard(&tx, &seed::cipher(), Some(id)).unwrap_err();
    assert!(matches!(&refused, eligibility::Refusal::Reconciling(_)), "{refused:?}");
    assert_eq!(refused.reason(), reconciling, "what the 422 carries");
    drop(tx); // rolled back
    // A plain stop: goes through.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap();
    assert!(eligibility::guard(&tx, &seed::cipher(), Some(id)).is_ok(), "a stop goes through");
    tx.commit().unwrap();
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, deltabadger::enums::BotStatus::Stopped);
    // The weight change on the stopped bot: still refused while the order is unresolved.
    let tx = model::immediate(&o.primary).unwrap();
    reweigh(&tx);
    assert_eq!(eligibility::guard(&tx, &seed::cipher(), Some(id)).unwrap_err().reason(), reconciling);
}

#[test]
fn alpaca_stocks_and_etfs_are_admitted_as_data_api_imports_them() {
    let (_d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let (qqqm, _) = seed::add_alpaca_stock(&o.primary, &s, "QQQM");
    o.primary.execute("UPDATE assets SET instrument_type = 'etf' WHERE id = ?1", [qqqm]).unwrap();
    for w in [vec![(aapl, 1.0)], vec![(qqqm, 1.0)], vec![(aapl, 0.5), (s.btc, 0.5)]] {
        let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&w));
        let bot = model::load_bot(&o.primary, id).unwrap();
        assert_eq!(eligibility::bot_reasons(&o.primary, &bot).unwrap(), Vec::<String>::new(), "{w:?}");
    }
}

#[test]
fn a_legacy_untyped_or_tokenized_stock_is_refused() {
    let (_d, o, s) = common::install_alpaca();
    let (legacy, _) = seed::add_alpaca_stock(&o.primary, &s, "IBIT");
    o.primary.execute("UPDATE assets SET external_id = 'alpaca_0f4c9e2a', instrument_type = NULL WHERE id = ?1", [legacy]).unwrap();
    let (untyped, _) = seed::add_alpaca_stock(&o.primary, &s, "MSFT");
    o.primary.execute("UPDATE assets SET instrument_type = NULL WHERE id = ?1", [untyped]).unwrap();
    let (tokenized, _) = seed::add_alpaca_stock(&o.primary, &s, "TSLAX");
    o.primary.execute("UPDATE assets SET category = 'Cryptocurrency', instrument_type = 'tokenized_stock' WHERE id = ?1", [tokenized]).unwrap();
    for (asset, why) in [(legacy, "legacy alpaca_<uuid>"), (untyped, "no instrument_type"), (tokenized, "tokenized")] {
        let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(asset, 1.0)]));
        let reasons = eligibility::bot_reasons(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap();
        assert!(reasons.iter().any(|r| r.starts_with("asset category")), "{why}: {reasons:?}");
    }
}

#[test]
fn a_stock_on_kraken_is_refused() {
    let (_d, o, s) = common::install();
    o.primary.execute("UPDATE assets SET category = 'Stock', instrument_type = 'stock' WHERE id = ?1", [s.btc]).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let reasons = eligibility::bot_reasons(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap();
    assert!(reasons.iter().any(|r| r.contains("stocks and ETFs on Alpaca")), "{reasons:?}");
}

use deltabadger::engine::model;

/// Slice 7a: a price rule with the `pause` action runs, on either side; its flip actions (7c), a value Rails cannot compare,
/// and the three other rules (7b) stay refused.
#[test]
fn a_price_rule_with_the_pause_action_is_eligible() {
    let (_d, o, s) = install();
    let reasons = |spec: BotSpec| eligibility::bot_reasons(&o.primary, &deltabadger::engine::model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &spec)).unwrap()).unwrap();
    let rule = || plain().with("price_limited", json!(true)).with("price_limit", json!(50000.0));
    assert!(reasons(rule()).is_empty());
    assert!(reasons(rule().with("price_limit_action", json!("pause"))).is_empty());
    assert!(reasons(plain().with("sell_price_limited", json!(true)).with("sell_price_limit_action", json!(""))).is_empty(), "blank reads as pause");
    assert!(reasons(rule().with("price_limited", json!(false))).is_empty(), "off");
    assert_eq!(reasons(rule().with("price_limit_action", json!("start_selling"))), vec!["price_limited with the start_selling action (the flip actions come with slice 7c)"]);
    assert_eq!(reasons(plain().with("sell_price_limited", json!(true)).with("sell_price_limit_action", json!("start_buying"))),
               vec!["sell_price_limited with the start_buying action (the flip actions come with slice 7c)"]);
    assert_eq!(reasons(rule().with("price_limit", json!("50000"))), vec![r#"price_limit "50000" is not a number"#]);
    assert_eq!(reasons(rule().with("price_limited", json!("1"))), vec!["price_limited"], "a flag Rails reads as off, but set: refused");
    for other in ["price_drop_limited", "moving_average_limited", "indicator_limited"] {
        assert_eq!(reasons(plain().with(other, json!(true))), vec![other.to_string()]);
    }
}

/// P1-T1: the one admission rule the engine and the bot pages share. Every venue takes buying baskets of any size and index
/// bots; a basket of any size sells on every venue (6a, 6b, plan CB T3); an index bot never sells (Rails has none).
#[test]
fn one_admission_rule_for_engine_and_pages() {
    use eligibility::admission;
    for venue in ["Exchanges::Alpaca", "Exchanges::Kraken", "Exchanges::Binance", "Exchanges::BinanceUs", "Exchanges::Kucoin", "Exchanges::Bitvavo", "Exchanges::Gemini", "Exchanges::Bitget", "Exchanges::Mexc", "Exchanges::Hyperliquid", "Exchanges::Bybit", "Exchanges::Bitrue"] {
        assert_eq!(admission(Some(venue), false, false), None, "{venue} buyer");
        assert_eq!(admission(Some(venue), false, true), None, "{venue} seller of any size");
        assert_eq!(admission(Some(venue), true, true), Some("an index bot that sells"), "{venue} index seller");
        assert_eq!(admission(Some(venue), true, false), None, "{venue} index");
    }
    for venue in [Some("Exchanges::Bingx"), None] {
        assert_eq!(admission(venue, false, false), Some("a bot on an exchange other than Alpaca, Kraken, Binance, Binance.US, KuCoin, Bitvavo, Gemini, Bitget, MEXC, Hyperliquid, Bybit, Bitrue and Coinbase"));
    }
}
