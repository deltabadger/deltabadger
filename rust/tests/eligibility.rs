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
