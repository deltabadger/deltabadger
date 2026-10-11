mod common;
use common::{install, install_alpaca};
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
fn update_transient_keeps_nulls_and_merge_compact_drops_only_owned_keys() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("keep", json!(1)));
    model::update_transient(&o.primary, id, &[("waiting_for_market_open", json!(null)), ("last_action_job_at", json!("x"))], now()).unwrap();
    let b = model::load_bot(&o.primary, id).unwrap();
    assert!(b.transient.as_object().unwrap().contains_key("waiting_for_market_open"));
    model::merge_transient_compact(&o.primary, id, &[("last_failure_kind", json!(null))]).unwrap();
    let b = model::load_bot(&o.primary, id).unwrap();
    assert!(b.transient.as_object().unwrap().contains_key("waiting_for_market_open"), "R5 compact preserves other keys");
    assert!(!b.transient.as_object().unwrap().contains_key("last_failure_kind"));
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

#[test]
fn a_key_that_does_not_decrypt_is_an_error_not_a_missing_key() {
    let (_d, o, s) = install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let foreign = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-install").unwrap());
    o.primary.execute("UPDATE api_keys SET key = ?1", [foreign.encrypt("test-key")]).unwrap();
    let err = model::credentials_for(&o.primary, &seed::cipher(), &bot).unwrap_err();
    assert!(matches!(&err, deltabadger::engine::EngineError::Data(m) if m.contains("api key unreadable")), "{err:?}");
}

#[test]
fn an_unparseable_last_action_job_at_is_a_data_error() {
    let (_d, o, s) = install();
    let read = |v: serde_json::Value| model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_action_job_at", v))).unwrap().last_action_job_at_us();
    assert!(matches!(read(json!("yesterday")), Err(deltabadger::engine::EngineError::Data(_))));
    assert!(matches!(read(json!(12)), Err(deltabadger::engine::EngineError::Data(_))));
    assert_eq!(read(json!(null)).unwrap(), None);
    assert_eq!(read(json!("2026-09-01T10:00:00.500Z")).unwrap(), Some("2026-09-01T10:00:00.5Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_micros()));
}

#[test]
fn a_blank_limit_distance_is_rails_default_and_garbage_is_unreadable() {
    let (_d, o, s) = install();
    let read = |v: serde_json::Value| model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", v))).unwrap().limit_distance().map(|d| d.to_s_f());
    for blank in [json!(""), json!("   "), json!(null), json!(false), json!([]), json!({})] {
        assert_eq!(read(blank.clone()).as_deref(), Some("0.001"), "{blank}: blank? → BigDecimal('0.001')");
    }
    for garbage in [json!("abc"), json!(true), json!([1]), json!({"a": 1})] {
        assert_eq!(read(garbage.clone()), None, "{garbage}: not a distance");
    }
    assert_eq!(read(json!(0)).as_deref(), Some("0.0"), "a real 0 stays 0");
}

#[test]
fn an_alpaca_key_carries_its_mode_and_a_ticker_its_venue_code() {
    let (_d, o, s) = install_alpaca();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let c = model::credentials_for(&o.primary, &seed::cipher(), &bot).unwrap().unwrap();
    assert_eq!((c.key.as_str(), c.secret.as_str(), c.passphrase.as_deref()), ("PKTEST", "paper-secret", Some("paper")));
    let t = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    assert_eq!((t.ticker.as_str(), t.base_code.as_str(), t.base_symbol.as_str(), t.quote_symbol.as_str()), ("BTC/USD", "BTC", "BTC", "USD"));

    let (_d2, o2, s2) = install();
    let kraken_bot = model::load_bot(&o2.primary, seed::insert_bot(&o2.primary, &s2, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    assert_eq!(model::credentials_for(&o2.primary, &seed::cipher(), &kraken_bot).unwrap().unwrap().passphrase, None, "a Kraken key has none");
    assert_eq!(model::ticker_for(&o2.primary, &kraken_bot).unwrap().unwrap().base_code, "XBT");
}

#[test]
fn a_kraken_key_with_an_unreadable_unused_passphrase_still_loads() {
    let (_d, o, s) = install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let foreign = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-install").unwrap());
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [foreign.encrypt("whatever")]).unwrap(); // decrypts under no key we hold
    let c = model::credentials_for(&o.primary, &seed::cipher(), &bot).unwrap().unwrap();
    assert_eq!((c.key.as_str(), c.passphrase), ("test-key", None), "Kraken never decrypts the passphrase, as merged");
}

#[test]
fn transient_writes_touch_only_their_own_keys() {
    let (_d, o, s) = install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    // Another writer's keys, in its order and its exact text (a number serde_json would re-render, a null).
    o.primary.execute(r#"UPDATE bots SET transient_data = '{"zeta":1.50,"big":12345678901234567890123,"alpha":null,"web":"x"}' WHERE id = ?1"#, [id]).unwrap();
    let raw = |o: &deltabadger::store::Opened| -> String { o.primary.query_row("SELECT transient_data FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    model::update_transient(&o.primary, id, &[("last_action_job_at", json!("2026-09-30T12:00:00.123Z")), ("waiting_for_market_open", json!(null))], now()).unwrap();
    assert_eq!(raw(&o), r#"{"zeta":1.50,"big":12345678901234567890123,"alpha":null,"web":"x","last_action_job_at":"2026-09-30T12:00:00.123Z","waiting_for_market_open":null}"#,
               "store_accessor stores the null; every other key is untouched, byte for byte");
    model::merge_transient_compact(&o.primary, id, &[("last_failure_kind", json!("transient"))]).unwrap();
    assert_eq!(raw(&o), r#"{"zeta":1.50,"big":12345678901234567890123,"alpha":null,"web":"x","last_action_job_at":"2026-09-30T12:00:00.123Z","waiting_for_market_open":null,"last_failure_kind":"transient"}"#,
               "R5 compacts only supplied keys; every other byte stays");
}

#[test]
fn r9c_damaged_timestamp_loading_is_lazy_and_stopping_preserves_evidence() {
    let (_d, o, s) = install();
    for field in ["last_action_job_at", "quote_amount_limit_enabled_at", "started_at", "settings_changed_at"] {
        let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        if matches!(field, "started_at" | "settings_changed_at") {
            o.primary.execute(&format!("UPDATE bots SET {field}='garbage' WHERE id=?1"), [id]).unwrap();
        } else {
            o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,'garbage') WHERE id=?2", (format!("$.{field}"), id)).unwrap();
        }
        let bot = model::load_bot(&o.primary, id).expect("loading does not read timestamps");
        assert_eq!(bot.quote_amount(), Some(60.0));
        if field == "last_action_job_at" { assert!(bot.last_action_job_at_us().is_err()); }
        if field == "quote_amount_limit_enabled_at" { assert!(bot.quote_amount_limit_enabled_at_us().is_err()); }
        model::update_status(&o.primary, id, BotStatus::Stopped, now()).unwrap();
        let stopped = model::load_bot(&o.primary, id).unwrap();
        assert_eq!(stopped.status, BotStatus::Stopped);
        assert_eq!(stopped.transient, bot.transient);
    }
}
