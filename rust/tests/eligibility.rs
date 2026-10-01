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
        ("allocations", Box::new(|c, s| seed::insert_bot(c, s, &plain().with("allocations", json!({ s.btc.to_string(): 0.5, s.eur.to_string(): 0.5 }))))),
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
                       VALUES (?1, ?2, 'OLIQ', 0, 1, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01', '2026-09-01')", rusqlite::params![id, s.kraken_id]).unwrap(); id })),
        ("LIQUIDATION order", Box::new(|c, s| { let id = seed::insert_bot(c, s, &plain());
            c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, transaction_type, error_messages, bot_interval, bot_quote_amount, created_at, updated_at) \
                       VALUES (?1, ?2, 'OLIQ2', 0, 4, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01', '2026-09-01')", rusqlite::params![id, s.kraken_id]).unwrap(); id })),
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
    o.primary.execute("INSERT INTO bot_index_assets (asset_id, bot_id, ticker_id, created_at, updated_at) VALUES (?1, ?2, ?3, '2026-01-01', '2026-01-01')", [s.eur, id, s.ticker_id]).unwrap();
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
                           VALUES (?1, ?2, ?3, 0, 4, 1, 0, 'LIQUIDATION', '[]', 'week', 60, '2026-09-01', '2026-09-01')", rusqlite::params![id, s.kraken_id, ext]).unwrap();
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
