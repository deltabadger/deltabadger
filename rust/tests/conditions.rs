//! Slice 7a: the price rule's comparison against Rails' own (condition_vectors.json), the carry window it opens, and the
//! check's writes fenced against a reversal that lands while its price read is out.
mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::conditions::{self, Checked};
use deltabadger::engine::model::{self, PriceRule};
use deltabadger::engine::tick::PriceCache;
use deltabadger::engine::{accounting, EngineError};
use deltabadger::enums::BotStatus;
use deltabadger::ruby::{BigDec, Num};
use deltabadger::venue::fake::FakeVenue;
use serde_json::{json, Value};

fn num(v: &Value) -> Num {
    if let Some(i) = v["int"].as_i64() { return Num::Int(i); }
    Num::Float(f64::from_bits(u64::from_str_radix(v["float"].as_str().unwrap(), 16).unwrap()))
}

#[test]
fn every_recorded_rails_price_comparison_is_reproduced() {
    use sha2::{Digest, Sha256};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/condition_vectors.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in fixture["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}: script/rust/repin");
    }
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1200);
    for c in cases {
        let rule = PriceRule { prefix: "price_limit", limit: num(&c["limit"]), lower: num(&c["lower"]), upper: num(&c["upper"]), after: false,
                               value_condition: c["condition"].as_str().map(str::to_string), ticker_id: None, action: "pause".into(), met_at_us: None };
        let price = BigDec::parse(c["price"].as_str().unwrap()).unwrap();
        assert_eq!(conditions::price_met(&rule, &price).unwrap(), c["met"].as_bool().unwrap(), "{c}");
    }
}

/// The carry window opens at the condition (price_limitable.rb:106-115): nothing is owed while the rule is unmet, and once met
/// only the intervals since it was met; while selling the frozen carry stands whatever the rule.
#[test]
fn the_carry_window_opens_where_the_condition_was_met() {
    let (_d, o, s) = common::install();
    let rule = |b: BotSpec| b.with("price_limited", json!(true)).with("price_limit_value_condition", json!("below"));
    let pending = |spec: BotSpec, at: &str| {
        let id = seed::insert_bot(&o.primary, &s, &spec);
        accounting::pending_quote_amount(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at.parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_micros()).unwrap()
    };
    let d = |t: &str| BigDec::parse(t).unwrap();
    let base = || BotSpec::weekly(60.0, "2026-09-01 10:00:00");
    assert_eq!(pending(base(), "2026-09-08T10:00:01Z"), d("120"), "no rule: two intervals");
    assert_eq!(pending(rule(base()).transient("missed_quote_amount", json!("30")), "2026-09-08T10:00:01Z"), d("0"), "paused: nothing owed, carry included");
    assert_eq!(pending(rule(base()).transient("price_limit_condition_met_at", json!("2026-08-31T10:00:00.000Z")), "2026-09-08T10:00:01Z"), d("120"), "met before the start");
    assert_eq!(pending(rule(base()).transient("price_limit_condition_met_at", json!("2026-09-03T10:00:00.000Z")), "2026-09-08T10:00:01Z"), d("60"), "met mid-interval: from then");
    assert_eq!(pending(rule(base()).transient("price_limit_condition_met_at", json!("2026-09-08T10:00:00.500Z")).transient("missed_quote_amount", json!("30")),
                       "2026-09-08T10:00:01Z"), d("30"), "met off the grid: only the carry");
    let selling = rule(base()).with("direction", json!("selling")).with("sell_price_limited", json!(true)).transient("missed_quote_amount", json!("37.5"));
    assert_eq!(pending(selling, "2026-09-08T10:00:01Z"), d("37.5"), "selling: the buy carry stays frozen");
}

fn kraken_body(last: &str) -> Value {
    json!({ "error": [], "result": { "XXBTZEUR": { "a": ["50000.2", "1", "1.000"], "b": ["49990.1", "1", "1.000"], "c": [last, "0.001"], "v": ["12.5", "30.1"],
            "p": [last, last], "t": [100, 250], "l": [last, last], "h": [last, last], "o": last } } })
}

/// A bot resting on its price rule, as a paused tick leaves it.
pub fn paused(o: &deltabadger::store::Opened, s: &seed::Seeded, limit: f64) -> i64 {
    let id = seed::insert_bot(&o.primary, s, &BotSpec { status: BotStatus::Waiting as i64, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
        .with("price_limited", json!(true)).with("price_limit", json!(limit)).with("price_limit_value_condition", json!("below"))
        .transient("last_action_job_at", json!("2026-09-02T10:00:00.000Z")));
    model::log_activity(&o.primary, id, "limit_paused", model::Level::Info, json!({ "limit_type": "price" }), "2026-09-02T10:00:00.001Z".parse().unwrap()).unwrap();
    id
}

/// Rails flips a paused bot (reversible.rb:188-191) and cancels its check; a check whose price read is out when the flip lands
/// writes nothing: no met_at over the reversed row, no re-arm.
#[tokio::test(flavor = "current_thread")]
async fn a_reversal_during_the_checks_price_read_fences_its_writes() {
    let (d, o, s) = common::install();
    let id = paused(&o, &s, 60_000.0);
    let path = d.path().join("production.sqlite3");
    let owner = s.user_id;
    let reversed = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = reversed.clone();
    let venue = FakeVenue::new().on_price(move || {
        let c = rusqlite::Connection::open(&path).unwrap();
        let r = deltabadger::web::bot::write::reverse(&c, &seed::cipher(), owner, id, "2026-09-02T10:03:00Z".parse().unwrap()).unwrap();
        flag.set(matches!(r, deltabadger::web::bot::write::Reversal::Reversed));
    });
    venue.script("/0/public/Ticker", kraken_body("49995.3"));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let version = model::credential_version(&o.primary, &bot).unwrap();
    let out = conditions::check(&o.primary, &venue, &bot, "2026-09-02T10:03:00Z".parse().unwrap(), &PriceCache::default(), &version).await;
    assert!(reversed.get(), "the paused bot reversed");
    assert!(matches!(out, Err(EngineError::RunFenced)), "{out:?}");
    let after = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(after.status, BotStatus::Scheduled, "the flip's own schedule");
    assert!(after.selling(), "reversed");
    assert!(after.transient.get("price_limit_condition_met_at").is_none(), "no stamp over the reversed row: {}", after.transient);
    assert!(after.rust_defer().unwrap().is_some_and(|(t, _)| t > "2026-09-02T10:03:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_micros()),
            "the flip's next checkpoint, never a run-now");
}

/// The check alone: unmet waits for the end of the minute, a failed read a minute, a zero price raises (Repair), met re-arms.
#[tokio::test(flavor = "current_thread")]
async fn the_check_answers_as_the_check_job_does() {
    let (_d, o, s) = common::install();
    let at: chrono::DateTime<chrono::Utc> = "2026-09-02T10:00:20.5Z".parse().unwrap();
    for (body, expected) in [(kraken_body("60100"), Ok(Checked::Next("2026-09-02T10:00:59.999999Z".parse().unwrap()))),
                             (json!({ "error": ["EService:Unavailable"] }), Ok(Checked::Next("2026-09-02T10:01:20.5Z".parse().unwrap()))),
                             (json!({ "error": ["EAPI:Invalid key"] }), Ok(Checked::Next("2026-09-02T10:01:20.5Z".parse().unwrap()))),
                             (kraken_body("0"), Err("Wrong last price")),
                             (kraken_body("49995.3"), Ok(Checked::Rearmed))] {
        let id = paused(&o, &s, 60_000.0);
        let venue = FakeVenue::new();
        venue.script("/0/public/Ticker", body);
        let bot = model::load_bot(&o.primary, id).unwrap();
        let version = model::credential_version(&o.primary, &bot).unwrap();
        let out = conditions::check(&o.primary, &venue, &bot, at, &PriceCache::default(), &version).await;
        match (out, expected) {
            (Ok(got), Ok(want)) => assert_eq!(got, want),
            (Err(EngineError::Data(m)), Err(want)) => assert!(m.contains(want), "{m}"),
            (got, want) => panic!("{got:?} != {want:?}"),
        }
        let after = model::load_bot(&o.primary, id).unwrap();
        let rearmed = after.status == BotStatus::Scheduled;
        assert_eq!(rearmed, after.rust_defer().unwrap().is_some(), "a re-arm commits its run-now with it");
    }
}
