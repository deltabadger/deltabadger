//! The engine running a bot whose starting time is on, against Rails' answers (fixtures/start_time_vectors.json): the
//! amount it owes around the delayed first run, and Bot::Startable#disable_starting_time!'s writes after it.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::{amount, model};
use deltabadger::ruby::BigDec;
use rusqlite::Connection;
use serde_json::{json, Value};

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/start_time_vectors.json")).unwrap() }
fn time(s: &Value) -> DateTime<Utc> { s.as_str().unwrap().parse().unwrap() }
fn dec(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

/// The bot exactly as Bot::Lifecycle#start left it in Rails.
fn spec(v: &Value) -> BotSpec {
    let started = &v["started"];
    let mut settings = json!({ "interval": v["interval"], "quote_amount": 60.0 });
    for (k, x) in v["settings"].as_object().unwrap().iter().chain(started["settings"].as_object().unwrap()) { settings[k] = x.clone(); }
    BotSpec { status: started["status"].as_i64().unwrap(), started_at: started["started_at"].as_str().map(str::to_owned),
              settings_changed_at: started["settings_changed_at"].as_str().map(str::to_owned), settings, transient: started["transient_data"].clone() }
}

fn stored(c: &Connection, id: i64) -> (Value, Value, String) {
    c.query_row("SELECT settings, transient_data, settings_changed_at FROM bots WHERE id = ?1", [id],
                |r| Ok((serde_json::from_str(&r.get::<_, String>(0)?).unwrap(), serde_json::from_str(&r.get::<_, String>(1)?).unwrap(), r.get(2)?))).unwrap()
}

#[test]
fn the_amount_and_the_disable_match_rails() {
    let (mut reads, mut disables) = (0, 0);
    for v in vectors()["schedule"].as_array().unwrap() {
        let (_d, o, s) = common::install();
        let c = &o.primary;
        let id = seed::insert_bot(c, &s, &spec(v));
        let pending = |at: DateTime<Utc>| amount::pending_quote_amount(c, &model::load_bot(c, id).unwrap(), at.timestamp_micros()).unwrap();
        for read in v["reads"].as_array().unwrap() {
            assert_eq!(pending(time(&read["now"])), dec(read["pending"].as_str().unwrap()), "{read}");
            reads += 1;
        }
        let Some(bought) = v["bought"].as_str() else { continue };
        let t0: DateTime<Utc> = deltabadger::codec::parse_time(v["started"]["started_at"].as_str().unwrap()).unwrap();
        // The first run's closed buy, as the recorder wrote it.
        c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, quote_amount, quote_amount_exec, \
                   amount_exec, price, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
                   VALUES (?1, ?2, 'OSTART-1', 0, 2, 0, 0, '60', ?3, '0.001', '50000', 'BTC', 'EUR', ?4, ?5, ?6, 60, 'REGULAR', '[]', ?7, ?7)",
                  rusqlite::params![id, s.exchange_id, bought, s.btc, s.quote, v["interval"].as_str(),
                                    deltabadger::codec::format_time(t0 + chrono::Duration::milliseconds(500))]).unwrap();
        amount::disable_starting_time(c, id, time(&v["disabled_at"])).unwrap();
        let (settings, transient, changed) = stored(c, id);
        let rails = &v["disabled"];
        assert_eq!(settings["start_time_enabled"], json!(false), "{v}");
        assert_eq!(transient, rails["transient_data"], "the carry and its marker, as Rails stores them: {v}");
        assert_eq!(changed, rails["settings_changed_at"].as_str().unwrap(), "the window restarts");
        assert_eq!(pending(time(&v["second"]["now"])), dec(v["second"]["pending"].as_str().unwrap()), "the next run's amount: {v}");
        // A second call is a no-op: the rule is off.
        amount::disable_starting_time(c, id, time(&v["second"]["now"])).unwrap();
        assert_eq!(stored(c, id).2, changed);
        disables += 1;
    }
    assert_eq!((reads, disables), (108, 18));
}
