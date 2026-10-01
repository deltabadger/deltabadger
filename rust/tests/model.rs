mod common;
use common::install;
use common::seed::{self, BotSpec};
use deltabadger::engine::model::{self, Level};
use deltabadger::enums::BotStatus;
use serde_json::json;

fn now() -> chrono::DateTime<chrono::Utc> { "2026-09-30T12:00:00.123456Z".parse().unwrap() }

#[test]
fn a_bot_reads_back_as_rails_stored_it() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00.5")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025))
        .transient("missed_quote_amount", json!("12.5")));
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.status, BotStatus::Scheduled);
    assert_eq!(b.quote_amount(), Some(60.0));
    assert_eq!(b.limit_distance().unwrap().to_s_f(), "0.0025");
    assert_eq!(b.missed_quote_amount().unwrap().to_s_f(), "12.5");
    assert_eq!(b.asset_ids(), vec![s.btc]);
    assert_eq!(model::ticker_for(&o.primary, &b).unwrap().unwrap().ticker, "XBTEUR");
}

#[test]
fn settings_are_read_with_rails_readers_and_defaults() {
    let (_d, o, s) = install();
    let read = |spec: BotSpec| model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &spec)).unwrap();
    let base = || BotSpec::weekly(60.0, "2026-09-01 10:00:00");
    assert_eq!(read(base().with("limit_ordered", json!(true))).limit_distance().unwrap().to_s_f(), "0.001", "the ||= 0.001 default");
    assert_eq!(read(base().with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!("0.0025"))).limit_distance().unwrap().to_s_f(), "0.0025");
    assert!(read(base().with("limit_ordered", json!("true"))).limit_distance().is_none(), "limit_ordered? is `== true`");
    assert!(read(base().with("smart_intervaled", json!(1)).with("smart_interval_quote_amount", json!(20.0))).smart_quote_amount().is_none());
    assert_eq!(read(base().with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!(20.0))).smart_quote_amount(), Some(20.0));
    assert!(read(base().with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!("20"))).smart_quote_amount().is_none(),
            "a string amount raises TypeError in Rails: never a number here");
}

#[test]
fn credentials_follow_rails_api_key_lookup() {
    let (_d, o, s) = install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    o.primary.execute("UPDATE api_keys SET status = 2", []).unwrap(); // incorrect: Rails still uses it
    let c = model::credentials_for(&o.primary, &seed::cipher(), &bot).unwrap().unwrap();
    assert_eq!(c.key, "test-key");
    o.primary.execute("DELETE FROM api_keys", []).unwrap();
    assert!(model::credentials_for(&o.primary, &seed::cipher(), &bot).unwrap().is_none());
}

#[test]
fn missed_amount_blank_is_zero_and_a_number_goes_through_float_to_d() {
    let (_d, o, s) = install();
    let blank = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let num = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("missed_quote_amount", json!(0.1)));
    assert!(model::load_bot(&o.primary, blank).unwrap().missed_quote_amount().unwrap().is_zero());
    assert_eq!(model::load_bot(&o.primary, num).unwrap().missed_quote_amount().unwrap().to_s_f(), "0.1");
}

#[test]
fn update_transient_keeps_nulls_and_merge_compact_drops_them() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("keep", json!(1)));
    model::update_transient(&o.primary, id, &[("waiting_for_market_open", json!(null)), ("last_action_job_at", json!("x"))], now()).unwrap();
    let b = model::load_bot(&o.primary, id).unwrap();
    assert!(b.transient.as_object().unwrap().contains_key("waiting_for_market_open"));
    model::merge_transient_compact(&o.primary, id, &[("last_failure_kind", json!(null))]).unwrap();
    let b = model::load_bot(&o.primary, id).unwrap();
    assert!(!b.transient.as_object().unwrap().contains_key("waiting_for_market_open"), "compact drops every null key");
    assert_eq!(b.transient["keep"], 1);
    let updated: String = o.primary.query_row("SELECT updated_at FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(updated, "2026-09-30 12:00:00.123456", "merge_compact leaves updated_at where update_transient put it");
}

#[test]
fn transition_working_only_moves_a_working_bot() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    assert!(model::transition_working(&o.primary, id, BotStatus::Scheduled, now()).unwrap());
    model::update_status(&o.primary, id, BotStatus::Stopped, now()).unwrap();
    assert!(!model::transition_working(&o.primary, id, BotStatus::Scheduled, now()).unwrap());
}

#[test]
fn activity_logs_are_written_like_log_activity() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    model::log_activity(&o.primary, id, "order_skipped", Level::Warning, json!({"base": "BTC"}), now()).unwrap();
    let (event, level, details, created): (String, i64, String, String) = o.primary.query_row(
        "SELECT event, level, details, created_at FROM bot_activity_logs WHERE bot_id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!((event.as_str(), level, created.as_str()), ("order_skipped", 1, "2026-09-30 12:00:00.123456"));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&details).unwrap(), json!({"base": "BTC"}));
}
