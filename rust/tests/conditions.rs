//! Slice 7a: the price rule's comparison against Rails' own (condition_vectors.json), the carry window it opens, and the
//! check's writes fenced against a reversal that lands while its price read is out.
mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::conditions::{self, Checked};
use deltabadger::engine::model::{self, PriceRule};
use deltabadger::engine::tick::PriceCache;
use deltabadger::engine::{accounting, Clock, EngineError, FixedClock};
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
    let out = conditions::check(&o.primary, &venue, &bot, &FixedClock("2026-09-02T10:03:00Z".parse().unwrap()), &PriceCache::default(), &version).await;
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
        let out = conditions::check(&o.primary, &venue, &bot, &FixedClock(at), &PriceCache::default(), &version).await;
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

/// A clock the test moves while the venue answers.
struct Moving(std::rc::Rc<std::cell::Cell<chrono::DateTime<chrono::Utc>>>);
impl Clock for Moving { fn now(&self) -> chrono::DateTime<chrono::Utc> { self.0.get() } }

/// Rails stamps `Time.current` after get_last_price answers: a read that takes the clock across a minute (or an interval
/// checkpoint) stamps met_at, and schedules the next check, at the time it answered, never the time it was sent.
#[tokio::test(flavor = "current_thread")]
async fn the_checks_writes_take_the_time_the_price_arrived() {
    let (_d, o, s) = common::install();
    let sent: chrono::DateTime<chrono::Utc> = "2026-09-02T10:00:59.9Z".parse().unwrap();
    let answered: chrono::DateTime<chrono::Utc> = "2026-09-02T10:01:02.25Z".parse().unwrap();
    for (last, met) in [("49995.3", true), ("60100", false)] {
        let id = paused(&o, &s, 60_000.0);
        let time = std::rc::Rc::new(std::cell::Cell::new(sent));
        let moved = time.clone();
        let venue = FakeVenue::new().on_price(move || moved.set(answered));
        venue.script("/0/public/Ticker", kraken_body(last));
        let bot = model::load_bot(&o.primary, id).unwrap();
        let version = model::credential_version(&o.primary, &bot).unwrap();
        let out = conditions::check(&o.primary, &venue, &bot, &Moving(time), &PriceCache::default(), &version).await.unwrap();
        let after = model::load_bot(&o.primary, id).unwrap();
        if met {
            assert_eq!(out, Checked::Rearmed);
            assert_eq!(after.transient["price_limit_condition_met_at"], json!("2026-09-02T10:01:02.250Z"), "{}", after.transient);
        } else {
            assert_eq!(out, Checked::Next("2026-09-02T10:01:59.999999Z".parse().unwrap()), "the end of the minute the price arrived in");
        }
    }
}

// ---- Slice 7s: the price rule's status line (Bot::BroadcastPriceLimitInfoUpdateJob) --------------------------------------

/// A request as the web leaves it (broadcasts::info_update's statement).
fn want(c: &rusqlite::Connection, id: i64) { assert!(conditions::want_info(c, id, None, conditions::PRICE, "en").unwrap()); }

/// A stopped basket with its buy-side price rule (below `limit`).
fn stopped_rule(o: &deltabadger::store::Opened, s: &seed::Seeded, limit: f64, f: impl FnOnce(BotSpec) -> BotSpec) -> i64 {
    seed::insert_bot(&o.primary, s, &f(BotSpec { status: BotStatus::Stopped as i64, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
        .with("price_limited", json!(true)).with("price_limit", json!(limit)).with("price_limit_value_condition", json!("below"))))
}

async fn serve(o: &deltabadger::store::Opened, venue: &FakeVenue, at: &str, prices: &PriceCache) -> Vec<Result<Option<Value>, EngineError>> {
    let mut out = vec![];
    for claim in conditions::info_claims(&o.primary, conditions::PRICE).unwrap() {
        let version = model::credential_version(&o.primary, &claim.bot).unwrap();
        out.push(conditions::info(&o.primary, venue, &claim, &FixedClock(at.parse().unwrap()), prices, &version).await);
    }
    out
}

/// Q1: the info job evaluates the rule with its writes on a stopped bot, stores the info for 20 s and serves the request;
/// a request 10 s on still evaluates (here: unmet, met_at cleared, the carry captured) but shows the kept info, expiry included.
#[tokio::test(flavor = "current_thread")]
async fn the_status_line_evaluates_with_rails_writes_and_keeps_its_info_for_20_seconds() {
    let (_d, o, s) = common::install();
    let id = stopped_rule(&o, &s, 60_000.0, |b| b);
    want(&o.primary, id);
    let venue = FakeVenue::new();
    venue.script("/0/public/Ticker", kraken_body("49995.3"));
    let first = serve(&o, &venue, "2026-09-02T10:00:00Z", &PriceCache::default()).await;
    let info = first[0].as_ref().unwrap().clone().expect("an info");
    assert_eq!((info["base"].clone(), info["condition_met"].clone()), (json!("BTC"), json!(true)), "{info}");
    let after = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(after.transient["price_limit_condition_met_at"], json!("2026-09-02T10:00:00.000Z"), "the page view stamps met_at");
    assert!(after.transient.get(conditions::INFO_WANTED_KEY).is_none(), "served: {}", after.transient);
    assert_eq!(after.status, BotStatus::Stopped);

    want(&o.primary, id);
    let venue = FakeVenue::new();
    venue.script("/0/public/Ticker", kraken_body("60100"));
    let second = serve(&o, &venue, "2026-09-02T10:00:10Z", &PriceCache::default()).await;
    assert_eq!(second[0].as_ref().unwrap().as_ref(), Some(&info), "the kept info, its expiry too");
    let after = model::load_bot(&o.primary, id).unwrap();
    assert!(after.transient["price_limit_condition_met_at"].is_null(), "still evaluated: met_at cleared {}", after.transient);
    assert_eq!(after.transient["missed_quote_amount_was_set"], json!(true), "the capture landed (the window moved)");

    want(&o.primary, id);
    let third = serve(&o, &venue, "2026-09-02T10:00:25Z", &PriceCache::default()).await;
    assert_eq!(third[0].as_ref().unwrap().as_ref().map(|i| i["condition_met"].clone()), Some(json!(false)), "expired: computed again");
}

/// accountable.rb:119-127 (r3): a clear that leaves the carry window where it was — another enabled rule's met_at is nil, so
/// the decorated started_at was nil already — takes the capture back: the carry as stored and the marker nil. Without the
/// other rule the capture lands.
#[tokio::test(flavor = "current_thread")]
async fn a_clear_that_leaves_the_window_unmoved_restores_the_carry_and_clears_the_marker() {
    let (_d, o, s) = common::install();
    let stored = |b: BotSpec| b.transient("price_limit_condition_met_at", json!("2026-09-01T12:00:00.000Z")).transient("missed_quote_amount", json!("30"))
        .transient("missed_quote_amount_was_set", json!(true));
    let held = stopped_rule(&o, &s, 40_000.0, |b| stored(b).with("price_drop_limited", json!(true)).with("price_drop_limit", json!(0.2)));
    let alone = stopped_rule(&o, &s, 40_000.0, stored);
    for id in [held, alone] { want(&o.primary, id); }
    let venue = FakeVenue::new();
    venue.script("/0/public/Ticker", kraken_body("49995.3"));
    let out = serve(&o, &venue, "2026-09-02T10:00:00Z", &PriceCache::default()).await;
    assert!(out.iter().all(|r| r.as_ref().is_ok_and(|i| i.as_ref().is_some_and(|i| i["condition_met"] == json!(false)))), "{out:?}");
    let held = model::load_bot(&o.primary, held).unwrap();
    assert!(held.transient["price_limit_condition_met_at"].is_null());
    assert_eq!(held.transient["missed_quote_amount"], json!("30"), "the carry as stored: {}", held.transient);
    assert!(held.transient.get("missed_quote_amount_was_set").is_some_and(Value::is_null), "the marker nil: {}", held.transient);
    let alone = model::load_bot(&o.primary, alone).unwrap();
    assert!(alone.transient["price_limit_condition_met_at"].is_null());
    assert_eq!(alone.transient["missed_quote_amount_was_set"], json!(true), "the capture lands: {}", alone.transient);
}

/// The claim is a version: a request that arrives while the price read is out survives the serve, and a settings change
/// during the read discards the run (no met_at, no info) and keeps the request.
#[tokio::test(flavor = "current_thread")]
async fn a_newer_request_survives_and_a_settings_change_discards_the_run() {
    let (d, o, s) = common::install();
    let id = stopped_rule(&o, &s, 60_000.0, |b| b);
    want(&o.primary, id);
    let path = d.path().join("production.sqlite3");
    let again = path.clone();
    let venue = FakeVenue::new().on_price(move || want(&rusqlite::Connection::open(&again).unwrap(), id));
    venue.script("/0/public/Ticker", kraken_body("49995.3"));
    let out = serve(&o, &venue, "2026-09-02T10:00:00Z", &PriceCache::default()).await;
    assert!(out[0].as_ref().is_ok_and(Option::is_some), "{out:?}");
    let after = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(after.transient[conditions::INFO_WANTED_KEY]["price_limit"]["version"], json!(2), "the newer request stays: {}", after.transient);

    let other = stopped_rule(&o, &s, 60_000.0, |b| b);
    o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_condition_info_wanted') WHERE id = ?1", [id]).unwrap();
    want(&o.primary, other);
    let edit = path.clone();
    let venue = FakeVenue::new().on_price(move || {
        rusqlite::Connection::open(&edit).unwrap().execute("UPDATE bots SET settings = json_set(settings, '$.price_limit', 1000) WHERE id = ?1", [other]).unwrap();
    });
    venue.script("/0/public/Ticker", kraken_body("49995.3"));
    let out = serve(&o, &venue, "2026-09-02T10:00:00Z", &PriceCache::default()).await;
    assert!(matches!(out[0], Err(EngineError::RunFenced)), "{out:?}");
    let after = model::load_bot(&o.primary, other).unwrap();
    assert!(after.transient.get("price_limit_condition_met_at").is_none(), "no stamp for a rule that changed: {}", after.transient);
    assert!(after.transient.get(conditions::INFO_KEY).is_none());
    assert!(after.transient.get(conditions::INFO_WANTED_KEY).is_some(), "the request stays for the bot as it is now");
}

/// A failed read leaves no info and no write, as Rails broadcasts nothing; the request is served. An index bot (no
/// PriceLimitable) gets nothing either.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_read_or_an_index_bot_leaves_no_info() {
    let (_d, o, s) = common::install();
    let failing = stopped_rule(&o, &s, 60_000.0, |b| b.transient("price_limit_condition_met_at", json!("2026-09-01T12:00:00.000Z")));
    let index = stopped_rule(&o, &s, 60_000.0, |b| b);
    o.primary.execute("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = ?1", [index]).unwrap();
    for id in [failing, index] { want(&o.primary, id); }
    let venue = FakeVenue::new();
    venue.script("/0/public/Ticker", json!({ "error": ["EService:Unavailable"] }));
    let out = serve(&o, &venue, "2026-09-02T10:00:00Z", &PriceCache::default()).await;
    assert!(out.iter().all(|r| matches!(r, Ok(None))), "{out:?}");
    for id in [failing, index] {
        let after = model::load_bot(&o.primary, id).unwrap();
        assert!(after.transient.get(conditions::INFO_KEY).is_none() && after.transient.get(conditions::INFO_WANTED_KEY).is_none(), "{}", after.transient);
    }
    assert_eq!(model::load_bot(&o.primary, failing).unwrap().transient["price_limit_condition_met_at"], json!("2026-09-01T12:00:00.000Z"), "nothing written");
}
