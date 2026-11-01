//! PLAN-conditions-every-bot, CE-0: D-CAP-REMAINDER-STOP acceptance (i)–(vi) on a basket and an index bot, and the cap/carry
//! arithmetic of an index bot. Every money value comes from fixture F (tests/fixtures/conditions_every_bot_expected.json).
mod common;
use chrono::{DateTime, Utc};
use common::scripted::{script, venue};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{accounting, model, notice, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::store;
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const T0: &str = "2026-09-01T10:00:00.5Z"; // the first tick of a weekly bot started 2026-09-01 10:00: 60 owed
fn f(case: &str) -> Value { common::conditions_fixture()[case].clone() }
fn strings(v: &Value) -> Vec<String> { v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect() }
fn cents(s: &str) -> String { format!("{:.2}", s.parse::<f64>().unwrap()) }

/// What one tick of a capped bot did: the outcome, the notionals sent, the skipped rows' quotes, whether it stopped with the
/// amount-limit key and its mail, and the `stopped` lines.
struct Ticked { out: TickOutcome, placed: Vec<String>, skipped: Vec<String>, status: BotStatus, key: Option<String>, mail: bool, stopped_lines: i64 }

fn read(o: &store::Opened, id: i64, out: TickOutcome, t: &deltabadger::venue::http::ScriptedTransport) -> Ticked {
    let placed = t.posted_orders().iter().map(|b| b["notional"].as_str().unwrap_or_default().to_string()).collect();
    let skipped = o.primary.prepare("SELECT CAST(quote_amount AS TEXT) FROM transactions WHERE bot_id = ?1 AND status = 2 ORDER BY id").unwrap()
        .query_map([id], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let key: Option<String> = o.primary.query_row("SELECT stop_message_key FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    let stopped_lines = o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'stopped'", [id], |r| r.get(0)).unwrap();
    Ticked { out, placed, skipped, status: bot.status, key, mail: bot.transient.get(notice::LIMIT).is_some(), stopped_lines }
}

/// The plan's checks for one case: the legs F names, and the stop exactly when F says so.
fn assert_case(name: &str, case: &Value, got: &Ticked) {
    assert_eq!(got.placed, strings(&case["placed"]), "{name}: placed");
    if let Some(want) = case.get("skipped") { assert_eq!(got.skipped, strings(want), "{name}: skipped"); }
    if let Some(want) = case.get("skipped_cents") { assert_eq!(got.skipped.iter().map(|s| cents(s)).collect::<Vec<_>>(), strings(want), "{name}: skipped"); }
    if case["stopped"] == json!(true) {
        assert!(matches!(got.out, TickOutcome::Skipped), "{name}: {:?}", got.out);
        assert_eq!((got.status, got.key.as_deref(), got.mail, got.stopped_lines), (BotStatus::Stopped, Some(tick::AMOUNT_SPENT), true, 1), "{name}: the amount-limit stop");
    } else {
        assert!(matches!(got.out, TickOutcome::Done { .. }), "{name}: {:?}", got.out);
        assert_eq!((got.status, got.mail, got.stopped_lines), (BotStatus::Scheduled, false, 0), "{name}: no stop");
    }
}

/// A weekly Alpaca basket over BTC, ETH, SOL (the first `weights.len()`), every pair's quote minimum `minimum`, and a cap.
async fn basket_case(name: &str) {
    let case = f(name);
    let (_d, o, s) = common::install_alpaca();
    let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
    let weights: Vec<(i64, f64)> = case["weights"].as_array().unwrap().iter().zip([s.btc, eth, sol]).map(|(w, a)| (a, w.as_f64().unwrap())).collect();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(case["quote_amount"].as_f64().unwrap(), "2026-09-01 10:00:00").weights(&weights)
        .with("quote_amount_limited", json!(true)).with("quote_amount_limit", case["cap"].clone())
        .transient("quote_amount_limit_enabled_at", json!("2026-08-01T00:00:00.000Z")));
    o.primary.execute("UPDATE tickers SET minimum_quote_size = ?1, minimum_base_size = '0.000000001'", [case["minimum"].as_str().unwrap()]).unwrap();
    let t = script(json!({}));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let got = read(&o, id, out, &t);
    assert_case(name, &case, &got);
    if case["stopped"] == json!(true) {
        // The next checkpoint sends nothing: the bot is stopped.
        let next = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:00.5Z")), &mut Attempts::default()).await.unwrap();
        assert!(matches!(next, TickOutcome::Skipped), "{name}: {next:?}");
        assert_eq!(t.posted_orders().len(), got.placed.len(), "{name}: nothing more sent");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn i_a_remainder_under_every_members_minimum_stops_at_the_first_binding_tick() { basket_case("i_under_every_minimum").await }

#[tokio::test(flavor = "current_thread")]
async fn ii_three_equal_legs_of_four_under_a_five_minimum_stop() { basket_case("ii_three_by_four").await }

#[tokio::test(flavor = "current_thread")]
async fn iii_one_placeable_leg_is_placed_and_nothing_stops() { basket_case("iii_one_leg_placeable").await }

#[tokio::test(flavor = "current_thread")]
async fn iv_a_cap_that_does_not_bind_never_stops_on_dust_legs() { basket_case("iv_not_binding").await }

#[tokio::test(flavor = "current_thread")]
async fn v_the_minimum_plus_one_increment_is_placed_one_under_stops() {
    basket_case("v_boundary_above").await;
    basket_case("v_boundary_below").await;
}

/// The index bot's universe (index.rs): Alpaca stocks AAA..EEE of a data-api category, `caps` each.
fn index_universe(caps: [f64; 5], num_coins: i64) -> (tempfile::TempDir, store::Opened, seed::Seeded, i64, Vec<(i64, i64)>) {
    let (d, o, s) = common::install_alpaca();
    let members: Vec<(i64, i64)> = ["AAA", "BBB", "CCC", "DDD", "EEE"].iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    let weights: serde_json::Map<String, Value> = ["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"].iter().zip(caps).map(|(k, c)| (k.to_string(), json!(c))).collect();
    seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"], &Value::Object(weights));
    let id = seed::index_bot(&o.primary, &s, "nasdaq-100", num_coins, 0.0, false);
    seed::fresh_stock_jobs(&o.primary, at("2026-09-01T14:00:00.5Z"));
    (d, o, s, id, members)
}
fn stock_script() -> deltabadger::venue::http::ScriptedTransport {
    let ok = |b: Value| json!([{ "status": 200, "body": b }]);
    let mut m = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": (1..=5).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    });
    for sym in ["AAA", "BBB", "CCC", "DDD", "EEE"] { m[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": 100, "bp": 100 } })); }
    deltabadger::venue::http::ScriptedTransport::from_script(&m)
}

/// (vi) the index version of (ii): five equal members, 20 left, five legs of 4 under a 5 minimum.
#[tokio::test(flavor = "current_thread")]
async fn vi_an_index_bot_stops_when_no_leg_of_its_remainder_is_placeable() {
    let case = f("vi_index_five_by_four");
    let (_d, o, _s, id, _) = index_universe([1e12; 5], 5);
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', ?1), \
                       transient_data = json_set(transient_data, '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = ?2", (case["cap"].as_i64().unwrap(), id)).unwrap();
    o.primary.execute("UPDATE tickers SET minimum_quote_size = ?1", [case["minimum"].as_str().unwrap()]).unwrap();
    let t = stock_script();
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-01T14:00:00.5Z")), &mut Attempts::default()).await.unwrap();
    assert_case("vi", &case, &read(&o, id, out, &t));
}

/// F's Codex row: carry 20 + one interval of 10 owed in the window an after-latched price rule opened; a 95 buy before that
/// window but after the cap's start counts toward the cap of 100 only. Pending 30, remainder 5, the tick's amount 5.
#[test]
fn an_index_bots_cap_and_carry_meet_as_f_computes() {
    let case = f("index_carry_cap");
    let (_d, o, s, id, members) = index_universe([5e12, 3e12, 2e12, 1e12, 5e11], 3);
    o.primary.execute("UPDATE bots SET started_at = '2026-08-25 14:00:00', settings = json_set(settings, '$.quote_amount', ?1, '$.quote_amount_limited', json('true'), \
                       '$.quote_amount_limit', ?2, '$.price_limited', json('true'), '$.price_limit_timing_condition', 'after', '$.price_limit_in_ticker_id', ?3), \
                       transient_data = json_set(transient_data, '$.missed_quote_amount', ?4, '$.price_limit_condition_met_at', '2026-09-01T13:00:00.000Z', \
                       '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = ?5",
                      (case["quote_amount"].as_i64().unwrap(), case["cap"].as_i64().unwrap(), members[0].1, case["carry"].as_str().unwrap(), id)).unwrap();
    assert_eq!(case["spent_before_window"], json!("95"));
    seed::insert_stock_tx(&o.primary, &s, id, members[0].0, "AAA", &TxSpec { status: 0, external_status: Some(2), external_id: Some("O-95".into()), order_type: 0,
        amount: Some("0.95"), quote_amount: Some("95"), price: Some("100"),
        quote_amount_exec: Some("95"), amount_exec: Some("0.95"), created_at: "2026-08-28 10:00:00".into() });
    let bot = model::load_bot(&o.primary, id).unwrap();
    let now = at("2026-09-01T14:00:00.5Z").timestamp_micros();
    let pending = accounting::pending_quote_amount(&o.primary, &bot, now).unwrap();
    let left = accounting::quote_amount_available(&o.primary, &bot).unwrap().unwrap();
    let num = |k: &str| deltabadger::ruby::BigDec::parse(case[k].as_str().unwrap()).unwrap();
    assert_eq!((&pending, &left), (&num("pending"), &num("remainder")));
    assert_eq!(pending.min(left), num("total"));
}

/// Codex P2: a cap raised or switched off while the tick awaits its price read is re-read in the stop's own transaction. A
/// Kraken basket over BTC, cap 0.4 (binding, under the 5 EUR minimum); `sql` is what the web commits during the read.
async fn cap_changed_during_the_price_read(sql: &'static str) -> Ticked {
    let dir = common::baseline_install();
    let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(0.4))
        .transient("quote_amount_limit_enabled_at", json!("2026-08-01T00:00:00.000Z")));
    o.primary.execute("UPDATE tickers SET minimum_quote_size = '5'", []).unwrap();
    let db = o.primary.path().unwrap().to_string();
    let v = deltabadger::venue::fake::FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0")
        .on_price(move || { if !sql.is_empty() { rusqlite::Connection::open(&db).unwrap().execute(sql, [id]).unwrap(); } });
    let out = tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(v.sent().is_empty(), "nothing placeable at the remainder read before the change");
    let bot = model::load_bot(&o.primary, id).unwrap();
    let key: Option<String> = o.primary.query_row("SELECT stop_message_key FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    let stopped_lines = o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'stopped'", [id], |r| r.get(0)).unwrap();
    Ticked { out, placed: vec![], skipped: vec![], status: bot.status, key, mail: bot.transient.get(notice::LIMIT).is_some(), stopped_lines }
}

#[tokio::test(flavor = "current_thread")]
async fn an_unchanged_cap_stops_after_the_price_read() {
    let got = cap_changed_during_the_price_read("").await;
    assert_eq!((got.status, got.key.as_deref(), got.mail, got.stopped_lines), (BotStatus::Stopped, Some(tick::AMOUNT_SPENT), true, 1));
}

#[tokio::test(flavor = "current_thread")]
async fn a_cap_raised_during_the_price_read_does_not_stop_the_bot() {
    let got = cap_changed_during_the_price_read("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limit', 1000) WHERE id = ?1").await;
    assert_eq!((got.status, got.key, got.mail, got.stopped_lines), (BotStatus::Scheduled, None, false, 0), "{:?}", got.out);
}

#[tokio::test(flavor = "current_thread")]
async fn a_cap_disabled_during_the_price_read_does_not_stop_the_bot() {
    let got = cap_changed_during_the_price_read("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('false')) WHERE id = ?1").await;
    assert_eq!((got.status, got.key, got.mail, got.stopped_lines), (BotStatus::Scheduled, None, false, 0), "{:?}", got.out);
}

/// Codex P2: an index bot with the cap on and no limit stored reads Rails' default 1000 in the engine, and the web's preview
/// and carry read the same, not "uncapped".
#[test]
fn an_index_bots_cap_with_no_limit_defaults_to_1000_in_the_engine_and_the_web() {
    for stored in [None, Some(json!(null)), Some(json!(false))] {
        let (_d, o, s, id, _) = index_universe([1e12; 5], 5);
        o.primary.execute("UPDATE bots SET settings = json_remove(json_set(settings, '$.quote_amount_limited', json('true')), '$.quote_amount_limit'), \
                           transient_data = json_set(transient_data, '$.missed_quote_amount', '5000', '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = ?1", [id]).unwrap();
        if let Some(v) = &stored { o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limit', json(?1)) WHERE id = ?2", (v.to_string(), id)).unwrap(); }
        let now = at("2026-09-01T14:00:00.5Z");
        let bot = model::load_bot(&o.primary, id).unwrap();
        let engine = accounting::quote_amount_available(&o.primary, &bot).unwrap().unwrap();
        let engine_pending = accounting::pending_quote_amount(&o.primary, &bot, now.timestamp_micros()).unwrap().min(engine.clone());
        assert_eq!(engine.to_s_f(), "1000.0", "{stored:?}: engine");
        let web = deltabadger::web::bot::Bot::find(&o.primary, s.user_id, id, deltabadger::web::bot::For::Page, "en").unwrap().unwrap();
        let preview = deltabadger::web::bot::start::amount_limit(&o.primary, &web).unwrap().expect("the web reads the cap as on");
        assert_eq!(preview.left.unwrap().to_d().unwrap(), engine, "{stored:?}: preview");
        assert_eq!(deltabadger::web::bot::write::pending(&o.primary, &web, now).unwrap().to_d().unwrap(), engine_pending, "{stored:?}: carry");
    }
}
