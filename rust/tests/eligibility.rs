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

type Make = Box<dyn Fn(&rusqlite::Connection, &seed::Seeded) -> i64>;

#[test]
fn each_thing_outside_the_slice_is_refused_with_its_reason() {
    let cases: Vec<(&str, Make)> = vec![
        ("allocations", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("allocations", json!({ s.btc.to_string(): 0.5, s.quote.to_string(): 0.5 }))))),
        ("price_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("price_limited", json!(true))))),
        ("indicator_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("indicator_limited", json!(true))))),
        ("quote_amount_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("quote_amount_limited", json!(true))))),
        ("start_time_enabled", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("start_time_enabled", json!(true))))),
        ("rebalance_enabled", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("rebalance_enabled", json!(true))))),
        ("direction", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("direction", json!("selling"))))),
        ("liquidation_pending", Box::new(|c, s| seed::insert_bot(c, s, &plain().transient("liquidation_pending", json!({"state": "x"}))))),
        ("wash_sale", Box::new(|c, s| { c.execute("UPDATE users SET wash_sale_enabled = 1", []).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("category", Box::new(|c, s| { c.execute("UPDATE assets SET category = 'stock' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("instrument", Box::new(|c, s| { c.execute("UPDATE assets SET instrument_type = 'tokenized' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("type", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain()); c.execute("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = ?1", [id]).unwrap(); id })),
        ("smart interval amount", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("smart_intervaled", json!(true))))),
        ("smart interval amount", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!("20"))))),
        ("LIQUIDATION/REDEPLOY", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain());
            c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                       VALUES (?1, ?2, 'OLIQ', 0, 1, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01', '2026-09-01')", rusqlite::params![id, s.exchange_id]).unwrap(); id })),
        ("LIQUIDATION order", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain());
            c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                       VALUES (?1, ?2, 'OLIQ2', 0, 4, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01', '2026-09-01')", rusqlite::params![id, s.exchange_id]).unwrap(); id })),
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
    let quiet = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain().with("price_limited", json!(true)) });
    assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty(), "a quiet stopped bot is Rails' business later, not now");
    seed::insert_tx(&o.primary, &s, quiet, &TxSpec { status: 0, external_status: Some(1), external_id: Some("OOPEN".into()), order_type: 1,
        amount: Some("0.001"), quote_amount: None, price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-20 10:00:00".into() });
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains(&format!("bot {quiet}")) && p.contains("outstanding order")));
    let rebalancer = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain().with("rebalance_enabled", json!(true)) });
    assert!(eligibility::check_install(&o.primary).unwrap().problems.iter().any(|p| p.contains(&format!("bot {rebalancer}"))));
    for status in [3, 7] { // deleted, archived: their orders are still at the venue
        let gone = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..plain().with("price_limited", json!(true)) });
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
fn rebalance_enabled_refuses_only_where_evaluate_rebalancers_would_pick_the_bot() {
    // Bot::EvaluateRebalancersJob#candidates: DcaIndex/DcaMultiAsset, not deleted or archived. A stopped bot still rebalances.
    for (status, ty, refuses) in [(2, "Bots::DcaMultiAsset", true), (0, "Bots::DcaMultiAsset", true), (2, "Bots::DcaIndex", true),
                                  (3, "Bots::DcaMultiAsset", false), (7, "Bots::DcaMultiAsset", false), (2, "Bots::DcaSingleAsset", false)] {
        let (_d, o, s) = install();
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..plain().with("rebalance_enabled", json!(true)) });
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
                           VALUES (?1, ?2, ?3, 0, 4, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01', '2026-09-01')", rusqlite::params![id, s.exchange_id, ext]).unwrap();
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
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status, ..plain().with("price_limited", json!(true)) });
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
fn alpaca_stocks_etfs_baskets_index_bots_amount_limits_and_other_quotes_are_refused() {
    // The owner's production instance is exactly the first six (spec amendment): it is not the canary.
    let cases: Vec<(&str, Make)> = vec![
        ("category stock", Box::new(|c, s| { c.execute("UPDATE assets SET category = 'stock' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("category etf", Box::new(|c, s| { c.execute("UPDATE assets SET category = 'etf' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("instrument tokenized", Box::new(|c, s| { c.execute("UPDATE assets SET instrument_type = 'tokenized' WHERE id = ?1", [s.btc]).unwrap(); seed::insert_bot(c, s, &plain()) })),
        ("allocations: 2", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("allocations", json!({ s.btc.to_string(): 0.5, s.quote.to_string(): 0.5 }))))),
        ("type Bots::DcaIndex", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain()); c.execute("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = ?1", [id]).unwrap(); id })),
        ("quote_amount_limited", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("quote_amount_limited", json!(true))))),
        ("direction", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("direction", json!("selling"))))),
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
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Binance', name = 'Binance'", []).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &plain());
    let r = eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.iter().any(|p| p.contains(&format!("bot {id}")) && p.contains("exchange Exchanges::Binance (only Kraken and Alpaca)")), "{:?}", r.problems);
}

#[test]
fn the_guard_refuses_a_write_that_makes_the_install_ineligible_and_names_the_bot() {
    use deltabadger::engine::model;
    let (_d, o, s) = common::install_alpaca(); // this build trades Alpaca paper only: the guard checks that too
    let id = seed::insert_bot(&o.primary, &s, &plain());
    // A write the engine can run: the guard says commit.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount', 70) WHERE id = ?1", [id]).unwrap();
    assert!(eligibility::guard(&tx, &seed::cipher()).is_ok());
    tx.commit().unwrap();
    // One it cannot: refused in check's words, naming the bot. The caller rolls back.
    let tx = model::immediate(&o.primary).unwrap();
    tx.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true')) WHERE id = ?1", [id]).unwrap();
    let refused = eligibility::guard(&tx, &seed::cipher()).unwrap_err();
    let line = format!("bot {id} (scheduled): quote_amount_limited");
    assert!(matches!(&refused, eligibility::Refusal::Ineligible(p) if p == &vec![line.clone()]), "{refused:?}");
    assert_eq!(refused.reason(), line);
    assert_eq!(refused.message(), format!("this install uses things only the full app runs:\n{line}"));
    drop(tx); // rolled back
    let limited: Option<i64> = o.primary.query_row("SELECT json_extract(settings, '$.quote_amount_limited') FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(limited, None, "nothing of the refused write remains");
}

#[test]
fn a_report_refuses_problems_first_then_unreadable_rows_in_checks_words() {
    let unreadable = vec![(7, "Data(\"x\")".to_string())];
    let r = eligibility::Report { eligible: vec![1], problems: vec![], unreadable: unreadable.clone() };
    let refused = r.refusal().unwrap_err();
    assert_eq!(refused.message(), format!("unreadable bot rows: {unreadable:?}"), "check's own format");
    assert_eq!(refused.reason(), "bot 7: unreadable (Data(\"x\"))");
    let r = eligibility::Report { eligible: vec![], problems: vec!["bot 3 (stopped): x".into()], unreadable };
    assert!(matches!(r.refusal(), Err(eligibility::Refusal::Ineligible(_))), "problems come first, as check reports them");
    let r = eligibility::Report { eligible: vec![1, 2], problems: vec![], unreadable: vec![] };
    assert_eq!(r.refusal().unwrap(), vec![1, 2]);
}
/// Codex round 1 (P1): the guard refuses what this build cannot trade, in `preflight`'s words, inside the write: the
/// start of an idle Kraken bot, and starts on Alpaca with a live key or with no key. Each start rolls back.
#[test]
fn the_guard_refuses_a_start_this_build_cannot_trade_and_the_start_rolls_back() {
    use deltabadger::engine::model;
    // Lifecycle#start as the web writes it (only from created or stopped), guarded before commit.
    let start = |c: &rusqlite::Connection, id: i64| -> Result<(), eligibility::Refusal> {
        let tx = model::immediate(c).unwrap();
        tx.execute("UPDATE bots SET status = 1 WHERE id = ?1 AND status IN (0, 2)", [id]).unwrap();
        eligibility::guard(&tx, &seed::cipher())?; // Err: `tx` is dropped, rolled back
        tx.commit().unwrap();
        Ok(())
    };
    let status = |c: &rusqlite::Connection, id: i64| -> i64 { c.query_row("SELECT status FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    let untradable = |r: &eligibility::Refusal, line: &str| matches!(r, eligibility::Refusal::Untradable(p) if p == &vec![line.to_string()]);

    // A stopped Kraken bot is not traded, so the install is fine until the start.
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain() });
    assert!(eligibility::guard(&o.primary, &seed::cipher()).is_ok(), "an idle Kraken bot is not traded");
    let refused = start(&o.primary, id).unwrap_err();
    assert!(untradable(&refused, &format!("bot {id}: Exchanges::Kraken is not connected in this build (Alpaca paper only)")), "{refused:?}");
    assert_eq!(status(&o.primary, id), 2, "rolled back");

    // Alpaca with a live key, then with no key.
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..plain() });
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")]).unwrap();
    let refused = start(&o.primary, id).unwrap_err();
    assert!(untradable(&refused, &format!("bot {id}: {}", deltabadger::venue::alpaca::LIVE_REFUSED)), "{refused:?}");
    assert_eq!(status(&o.primary, id), 2, "rolled back");
    o.primary.execute("DELETE FROM api_keys", []).unwrap();
    let refused = start(&o.primary, id).unwrap_err();
    assert!(untradable(&refused, &format!("bot {id}: no Alpaca API key")), "{refused:?}");
    assert_eq!(status(&o.primary, id), 2, "rolled back");
}
