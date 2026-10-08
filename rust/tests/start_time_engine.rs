//! The engine running a bot whose starting time is on, against Rails' answers (fixtures/start_time_vectors.json): the
//! amount it owes around the delayed first run, Bot::Startable#disable_starting_time!'s writes after it, the run's timing,
//! and which start-time states it refuses.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{amount, eligibility, model, FixedClock};
use deltabadger::lease;
use deltabadger::ruby::BigDec;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{FakeFactory, FakeVenue};
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

fn engine(spec: &BotSpec, venue: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, i64) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-01-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, spec);
    (dir, Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock), id)
}

/// Nothing before the start; at the first pass after it, one order for the amount Rails owes then; the rule then off, and
/// the next order at the next checkpoint, not before.
#[tokio::test(flavor = "current_thread")]
async fn the_first_run_waits_for_the_start_and_then_turns_the_rule_off() {
    let all = vectors();
    for v in all["schedule"].as_array().unwrap().iter().filter(|v| v["bought"].is_null()) {
        let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0");
        let (_d, mut e, id) = engine(&spec(v), venue.clone());
        let t0 = time(&v["wait_until"]);
        let us = |d: i64| FixedClock(t0 + chrono::Duration::microseconds(d));
        assert!(eligibility::check_install(&e.primary).unwrap().eligible.contains(&id), "{v}");
        let wake = run::step(&mut e, &us(-1)).await.unwrap();
        assert!(venue.sent().is_empty(), "never before the start: {v}");
        let (settings, transient, _) = stored(&e.primary, id);
        assert!(settings["start_time_enabled"] == json!(true) && transient.get("last_action_job_at").is_none_or(Value::is_null),
                "no run at all before the start (a run would also turn the rule off): {v}");
        assert!(wake > t0.timestamp_micros() && wake <= t0.timestamp_micros() + 5_000_000, "it wakes just after the start: {v}");
        let first = &v["reads"][3]; // T0 + 0.5 s
        run::step(&mut e, &FixedClock(time(&first["now"]))).await.unwrap();
        let owed = dec(first["pending"].as_str().unwrap());
        if owed == BigDec::zero() {
            // Rails' month clamp: a start on the 31st owes nothing at its first run (see the vectors), and buys nothing.
            assert!(venue.sent().is_empty(), "{v}");
        } else {
            assert_eq!(venue.sent().len(), 1, "{v}");
            let order = &venue.sent()[0];
            let quote = match (&order.kind, order.quote_volume) {
                (_, true) => dec(&order.volume),
                (deltabadger::venue::OrderKind::Limit { price }, false) => &dec(&order.volume) * &dec(price),
                (_, false) => &dec(&order.volume) * &dec("50000.0"), // a market buy sized in base at the ask
            };
            assert!(&owed - &quote < dec("0.001") && quote <= owed, "the order is the amount Rails owes: {quote:?} for {owed:?}");
        }
        assert_eq!(stored(&e.primary, id).0["start_time_enabled"], json!(false), "the run turns the rule off: {v}");
        let next = time(&first["next"]);
        run::step(&mut e, &FixedClock(next - chrono::Duration::seconds(1))).await.unwrap();
        assert!(venue.sent().len() <= 1, "nothing more before the next checkpoint: {v}");
    }
}

#[test]
fn only_a_fresh_starts_starting_time_runs() {
    let v = &vectors()["schedule"][6]; // Warsaw, every day at 09:30
    let refused = |edit: &str| {
        let (_d, o, s) = common::install();
        let id = seed::insert_bot(&o.primary, &s, &spec(v));
        o.primary.execute(edit, [id]).unwrap();
        let r = eligibility::check_install(&o.primary).unwrap();
        (r.eligible.contains(&id), r.problems.join("; "))
    };
    assert!(refused("UPDATE bots SET id = id WHERE id = ?1").0, "the state Lifecycle#start leaves runs");
    let (ok, why) = refused("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_continue_start', json('{\"requested_at\":\"2026-09-10T05:00:00Z\"}')) WHERE id = ?1");
    assert!(!ok && why.contains("continue"), "{why}");
    let (ok, why) = refused("UPDATE bots SET settings = json_set(settings, '$.start_at', '2026-09-12T07:30:00Z') WHERE id = ?1");
    assert!(!ok && why.contains("start_at"), "a start_at moved after the start: {why}");
    let (ok, why) = refused("UPDATE bots SET settings = json_remove(settings, '$.start_at') WHERE id = ?1");
    assert!(!ok && why.contains("start_at"), "{why}");
    let (ok, why) = refused("UPDATE bots SET settings = json_set(settings, '$.start_at', 'garbage') WHERE id = ?1");
    assert!(!ok && why.contains("start_at"), "{why}");
    // Off, or stopped: nothing to refuse.
    assert!(refused("UPDATE bots SET settings = json_set(settings, '$.start_time_enabled', json('false'), '$.start_at', 'garbage') WHERE id = ?1").0);
    let (_d, o, s) = common::install();
    let id = seed::insert_bot(&o.primary, &s, &spec(v));
    o.primary.execute("UPDATE bots SET status = 2, settings = json_remove(settings, '$.start_at') WHERE id = ?1", [id]).unwrap();
    assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty());
}
