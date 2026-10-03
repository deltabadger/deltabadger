mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::engine::run::{self, Engine};
use deltabadger::crypto::Credentials;
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{model, FixedClock};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

/// Tuesday 2026-09-01, 10:00:00.5 EDT: the market is open.
const T0: &str = "2026-09-01T14:00:00.5Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn clock_body(open: bool, next_open: &str, next_close: &str) -> Value {
    json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": open, "next_open": next_open, "next_close": next_close })
}
/// Open, with a close far ahead: any test date reads it as open. Tests of the clock itself script their own.
fn open_clock() -> Value { ok(clock_body(true, "2099-01-02T09:30:00-05:00", "2099-01-01T16:00:00-05:00")) }
fn script(over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v2/clock": open_clock(),
        "GET /v2/stocks/AAPL/quotes/latest": ok(json!({ "quote": { "ap": 187.43, "bp": 187.4 } })),
        "GET /v2/stocks/AAPL/trades/latest": ok(json!({ "trade": { "p": 187.41 } })),
        "POST /v2/orders": ok(json!({ "id": "OTX-1", "status": "accepted" })),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
fn weekly() -> BotSpec { BotSpec::weekly(60.0, "2026-09-01 14:00:00") }
fn limit(spec: BotSpec) -> BotSpec { spec.with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)) }
/// An Alpaca install with AAPL beside BTC/USD, and a bot over `spec` buying AAPL alone: (dir, store, bot, seed, AAPL asset).
fn setup(spec: BotSpec) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded, i64) {
    let (d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let id = seed::insert_bot(&o.primary, &s, &spec.weights(&[(aapl, 1.0)]));
    (d, o, id, s, aapl)
}
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
async fn tick_at(o: &store::Opened, t: &ScriptedTransport, id: i64, now: DateTime<Utc>) -> TickOutcome {
    seed::fresh_stock_jobs(&o.primary, now);
    tick::tick(&o.primary, &venue(t), id, &FixedClock(now), &mut Attempts::default()).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_stock_market_tick_sends_a_day_notional_buy() {
    let (_d, o, id, _, _) = setup(weekly());
    let t = script(json!({}));
    let out = tick_at(&o, &t, id, at(T0)).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(t.posted_orders(), vec![json!({ "symbol": "AAPL", "side": "buy", "type": "market", "time_in_force": "day", "notional": "60.00" })]);
    assert!(t.requests().iter().any(|r| r.path == "/v2/stocks/AAPL/quotes/latest"), "a market buy reads the ask");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stock_limit_tick_sends_a_day_qty_buy_below_the_last_trade() {
    let (_d, o, id, _, _) = setup(limit(weekly()));
    let t = script(json!({}));
    tick_at(&o, &t, id, at(T0)).await;
    // 187.41 × (1 − 0.0025) = 186.941475, floored to price_decimals 2; qty = 60 / 186.94 floored to base_decimals 9.
    assert_eq!(t.posted_orders(), vec![json!({ "symbol": "AAPL", "side": "buy", "type": "limit", "time_in_force": "day",
                                               "qty": "0.320958596", "limit_price": "186.94" })]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_expired_part_filled_day_order_counts_its_fill_and_is_not_bought_again() {
    // Last week's limit order expired at the close with 0.213333333 filled at 187.5 (39.9999999375 USD). Rule "A" (A8): the
    // fill is spent and only the unfilled part is owed again, so this week's order is 120 − 39.9999999375 = 80.0000000625,
    // sent as 80.00 / 186.94. Before rule "A" (missed_quote_amount 0) the fill counted nowhere and 120 was re-bought.
    let (_d, o, id, s, aapl) = setup(limit(weekly()));
    seed::insert_stock_tx(&o.primary, &s, id, aapl, "AAPL", &TxSpec {
        status: 0, external_status: Some(1), external_id: Some("OEXP-1".into()), order_type: 1, amount: Some("0.32"), quote_amount: None,
        price: Some("187.5"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-01 14:00:01".into() });
    let t = script(json!({ "GET /v2/orders/OEXP-1": ok(json!({ "id": "OEXP-1", "symbol": "AAPL", "status": "expired", "type": "limit", "side": "buy",
        "qty": "0.32", "notional": null, "filled_qty": "0.213333333", "filled_avg_price": "187.5", "limit_price": "187.5" })) }));
    let out = tick_at(&o, &t, id, at("2026-09-08T14:00:01Z")).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(one::<i64>(&o, "SELECT external_status FROM transactions WHERE external_id = 'OEXP-1'"), 3, "expired reads as cancelled");
    assert_eq!(t.posted_orders()[0]["qty"], "0.427944795", "80.00 / 186.94: the filled part is not bought again");
}

#[tokio::test(flavor = "current_thread")]
async fn the_composition_decides_all_crypto() {
    let (_d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let bot = |w: &[(i64, f64)]| model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &weekly().weights(w))).unwrap();
    assert!(model::all_crypto(&o.primary, &bot(&[(s.btc, 1.0)])).unwrap(), "a crypto bot");
    assert!(!model::all_crypto(&o.primary, &bot(&[(aapl, 1.0)])).unwrap(), "a stock bot");
    assert!(!model::all_crypto(&o.primary, &bot(&[(s.btc, 0.5), (aapl, 0.5)])).unwrap(), "a mixed basket");
    o.primary.execute("UPDATE tickers SET trading_enabled = 0 WHERE base_asset_id = ?1", [s.btc]).unwrap();
    assert!(model::all_crypto(&o.primary, &bot(&[(s.btc, 1.0)])).unwrap(), "an untradable crypto ticker keeps its bot crypto (2b's ruling)");
}

/// Closed at T0 (a holiday): the next open is Wednesday 09:30 EDT.
fn closed_clock() -> Value { ok(clock_body(false, "2026-09-02T09:30:00-04:00", "2026-09-02T16:00:00-04:00")) }
fn daily() -> BotSpec { BotSpec { settings: json!({ "interval": "day", "quote_amount": 60.0 }), ..weekly() } }
fn clock_reads(t: &ScriptedTransport) -> usize { t.requests().iter().filter(|r| r.path == "/v2/clock").count() }
fn events(o: &store::Opened) -> Vec<String> {
    let mut s = o.primary.prepare("SELECT event FROM bot_activity_logs ORDER BY id").unwrap();
    s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
/// Ticks with Rails' retries replayed (retry_on's waits), until an outcome other than RetryAfter.
async fn tick_until_settled(o: &store::Opened, t: &ScriptedTransport, id: i64, start: DateTime<Utc>) -> TickOutcome {
    seed::fresh_stock_jobs(&o.primary, start);
    let (mut now, mut attempts) = (start, Attempts::default());
    loop {
        match tick::tick(&o.primary, &venue(t), id, &FixedClock(now), &mut attempts).await.unwrap() {
            TickOutcome::RetryAfter(d) => now += Duration::from_std(d).unwrap(),
            other => return other,
        }
    }
}
struct Scripted(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Scripted {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<Credentials>) -> Self::V { venue(&self.0) }
}
#[tokio::test(flavor = "current_thread")]
async fn a_closed_market_parks_the_bot_until_next_open_and_writes_nothing_else() {
    let (_d, o, id, _, _) = setup(weekly());
    let t = script(json!({ "GET /v2/clock": closed_clock() }));
    let untouched: String = one(&o, "SELECT updated_at FROM bots");
    let out = tick_at(&o, &t, id, at(T0)).await;
    assert!(matches!(out, TickOutcome::MarketClosed { until } if until == at("2026-09-02T13:30:00Z")), "{out:?}");
    assert_eq!(events(&o), vec!["market_closed"]);
    assert_eq!(one::<String>(&o, "SELECT details FROM bot_activity_logs"), r#"{"next_market_open_at":"2026-09-02T09:30:00.000-04:00"}"#,
               "Time.parse(next_open).as_json: the clock's own offset, three fraction digits");
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(bot.transient["waiting_for_market_open"], json!(true));
    assert!(bot.last_action_job_at_us().unwrap().is_none(), "no last_action_job_at: the bot stays due");
    assert_eq!(bot.status, BotStatus::Scheduled, "no status change");
    assert!(t.requests().iter().all(|r| r.path == "/v2/clock"), "no sweep, no price, no balance");
    let parked: String = one(&o, "SELECT updated_at FROM bots");
    assert_ne!(parked, untouched, "update!(waiting_for_market_open: true) saved");
    tick_at(&o, &t, id, at("2026-09-01T14:05:00Z")).await;
    assert_eq!(one::<String>(&o, "SELECT updated_at FROM bots"), parked, "already true: store_accessor writes nothing, update! saves nothing");
    assert_eq!(events(&o), vec!["market_closed", "market_closed"]);
}

#[tokio::test(flavor = "current_thread")]
async fn the_first_open_tick_clears_the_flag_and_buys_every_skipped_interval() {
    let (_d, o, id, _, _) = setup(daily());
    let t = script(json!({ "GET /v2/clock": [closed_clock()[0].clone(), ok(clock_body(true, "2026-09-04T09:30:00-04:00", "2026-09-03T16:00:00-04:00"))[0].clone()] }));
    tick_at(&o, &t, id, at(T0)).await;
    let out = tick_at(&o, &t, id, at("2026-09-03T14:00:01Z")).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(t.posted_orders()[0]["notional"], "180.00", "Tuesday, Wednesday and Thursday: three intervals through the interval count");
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(bot.transient.get("waiting_for_market_open").is_none(), "ActionJob Fix C compacts the cleared flag");
}

#[tokio::test(flavor = "current_thread")]
async fn a_clock_that_cannot_be_trusted_places_nothing_and_retries() {
    let cases = [
        ("5xx", json!([{ "status": 503, "body": { "code": 50310000, "message": "service unavailable" } }])),
        ("html 502", json!([{ "status": 502, "body": "<html><body>Bad Gateway</body></html>" }])),
        ("a 2xx that is not JSON", json!([{ "status": 200, "body": "upstream connect error" }])),
        ("no is_open", ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "next_open": "2026-09-02T09:30:00-04:00", "next_close": "2026-09-01T16:00:00-04:00" }))),
        ("an unreadable next_open", ok(clock_body(false, "tomorrow", "2026-09-01T16:00:00-04:00"))),
        ("certificate", json!([{ "network": "permanent", "message": "Faraday::SSLError: certificate verify failed" }])),
        ("open, past its next_close", ok(clock_body(true, "2026-09-02T09:30:00-04:00", "2026-09-01T09:59:00-04:00"))),
        ("closed, past its next_open", ok(clock_body(false, "2026-09-01T09:30:00-04:00", "2026-09-01T16:00:00-04:00"))),
    ];
    for (name, reply) in cases {
        let (_d, o, id, _, _) = setup(weekly());
        let t = script(json!({ "GET /v2/clock": reply }));
        let out = tick_until_settled(&o, &t, id, at(T0)).await;
        assert!(matches!(out, TickOutcome::Rescheduled), "{name}: {out:?}");
        assert!(t.posted_orders().is_empty(), "{name}: nothing placed (Rails would have read the market as open)");
        assert_eq!(clock_reads(&t), 4, "{name}: retry_on's four attempts");
        assert!(t.requests().iter().all(|r| r.path == "/v2/clock"), "{name}: nothing else was asked");
        assert_eq!(events(&o), vec!["execution_retrying"], "{name}");
        let bot = model::load_bot(&o.primary, id).unwrap();
        assert_eq!((bot.status, bot.last_failure_kind()), (BotStatus::Retrying, Some("transient".into())), "{name}");
        assert!(bot.last_action_job_at_us().unwrap().is_none(), "{name}: written only after the gate");
        assert!(bot.transient.get("waiting_for_market_open").is_none(), "{name}: not parked");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_clock_timeout_retries_as_rails_does() {
    let (_d, o, id, _, _) = setup(weekly());
    let t = script(json!({ "GET /v2/clock": [{ "network": "post_send", "message": "Faraday::TimeoutError: Net::ReadTimeout" }] }));
    assert!(matches!(tick_until_settled(&o, &t, id, at(T0)).await, TickOutcome::Rescheduled));
    let details: Value = serde_json::from_str(&one::<String>(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_retrying'")).unwrap();
    assert_eq!(details, json!({ "error": "Faraday::TimeoutError: Net::ReadTimeout", "transient_exhausted": true }));
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejected_key_on_the_clock_fails_the_tick_as_invalid_key() {
    for (reply, error, kind) in [
        (json!([{ "status": 401, "body": { "code": 40110000, "message": "unauthorized." } }]), "Alpaca rejected the API key: unauthorized.", json!("invalid_key")),
        (json!([{ "status": 401, "body": "<html>401 Authorization Required</html>" }]), "Alpaca rejected the API key: HTTP 401", Value::Null),
    ] {
        let (_d, o, id, _, _) = setup(weekly());
        let t = script(json!({ "GET /v2/clock": reply }));
        assert!(matches!(tick_at(&o, &t, id, at(T0)).await, TickOutcome::Rescheduled));
        let details: Value = serde_json::from_str(&one::<String>(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'")).unwrap();
        assert_eq!((details["error"].as_str(), &details["kind"]), (Some(error), &kind), "Exchange#raise_on_invalid_key!: a StandardError");
        assert!(t.posted_orders().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_parked_bot_is_not_ticked_before_next_open_and_buys_at_it() {
    let dir = common::rails_install();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    seed::insert_bot(&o.primary, &s, &weekly().weights(&[(aapl, 1.0)]));
    let t = script(json!({ "GET /v2/clock": [closed_clock()[0].clone(), ok(clock_body(true, "2026-09-03T09:30:00-04:00", "2026-09-02T16:00:00-04:00"))[0].clone()] }));
    seed::fresh_stock_jobs(&o.primary, at(T0));
    let mut e = Engine::new(o.primary, Scripted(t.clone()), seed::cipher(), lock);
    let wake = run::step(&mut e, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(clock_reads(&t), 1);
    assert!(wake <= at("2026-09-02T13:30:00Z").timestamp_micros(), "the loop wakes by next_open");
    run::step(&mut e, &FixedClock(at("2026-09-01T18:00:00Z"))).await.unwrap();
    assert_eq!(clock_reads(&t), 1, "parked: still due, but not ticked before next_open");
    run::step(&mut e, &FixedClock(at("2026-09-02T13:30:00.5Z"))).await.unwrap();
    assert_eq!(clock_reads(&t), 2);
    assert_eq!(t.posted_orders()[0]["notional"], "60.00", "the open places the week's buy");
}

use deltabadger::enums::BotStatus;

#[tokio::test(flavor = "current_thread")]
async fn clock_reschedule_survives_restart_until_the_rails_checkpoint() {
    let (dir, o, id, _, _) = setup(weekly());
    let failed = script(json!({ "GET /v2/clock": [{ "network": "post_send", "message": "Faraday::TimeoutError: Net::ReadTimeout" }] }));
    assert!(matches!(tick_until_settled(&o, &failed, id, at(T0)).await, TickOutcome::Rescheduled));
    assert_eq!(clock_reads(&failed), 4);
    drop(o);
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T15:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    let t = script(json!({}));
    let mut e = Engine::new(o.primary, Scripted(t.clone()), seed::cipher(), lock);
    run::step(&mut e, &FixedClock(at("2026-09-01T15:00:00Z"))).await.unwrap();
    assert!(t.posted_orders().is_empty(), "a recovered clock must still wait for Rails' next checkpoint");
    seed::fresh_stock_jobs(&e.primary, at("2026-09-08T14:00:00Z"));
    run::step(&mut e, &FixedClock(at("2026-09-08T14:00:00Z"))).await.unwrap();
    assert!(t.posted_orders().is_empty(), "checkpoints are strictly after their instant");
    run::step(&mut e, &FixedClock(at("2026-09-08T14:00:00.5Z"))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_weekend_stock_wait_is_discarded_when_restarted_as_crypto() {
    let (dir, o, id, s, _) = setup(weekly());
    let now = at("2026-09-05T14:00:00Z");
    seed::fresh_stock_jobs(&o.primary, now);
    let lock = deltabadger::lease::lock(&store::Paths::from_env(&|_| None, dir.path()), now).unwrap();
    let t = script(json!({
        "GET /v2/clock": ok(clock_body(false, "2026-09-08T09:30:00-04:00", "2026-09-08T16:00:00-04:00")),
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({"quotes":{"BTC/USD":{"ap":60000,"bp":60000}}}))
    }));
    let mut e = Engine::new(o.primary, Scripted(t.clone()), seed::cipher(), lock);
    run::step(&mut e, &FixedClock(now)).await.unwrap();
    assert_eq!(clock_reads(&t), 1);
    assert!(t.posted_orders().is_empty());
    e.primary.execute("UPDATE bots SET status=2, stopped_at='2026-09-05 14:01:00' WHERE id=?1", [id]).unwrap();
    run::step(&mut e, &FixedClock(now + Duration::minutes(1))).await.unwrap();
    e.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)), status=1, started_at='2026-09-05 14:02:00' WHERE id=?2",
        rusqlite::params![json!({s.btc.to_string():1.0}).to_string(),id]).unwrap();
    run::step(&mut e, &FixedClock(now + Duration::minutes(2) + Duration::milliseconds(500))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1, "Rails buys crypto over the weekend");
    assert_eq!(t.posted_orders()[0]["symbol"], "BTC/USD");
    assert_eq!(clock_reads(&t), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_parked_wait_is_invalidated_by_each_lifecycle_or_composition_change() {
    for change in ["stop", "start", "composition", "crypto"] {
        let (dir, o, id, s, _) = setup(weekly());
        let now = at(T0);
        seed::fresh_stock_jobs(&o.primary, now);
        let lock = deltabadger::lease::lock(&store::Paths::from_env(&|_| None, dir.path()), now).unwrap();
        let t = script(json!({
            "GET /v2/clock": [closed_clock()[0].clone(), open_clock()[0].clone()],
            "GET /v1beta3/crypto/us/latest/quotes": ok(json!({"quotes":{"BTC/USD":{"ap":60000,"bp":60000}}}))
        }));
        let mut e = Engine::new(o.primary, Scripted(t.clone()), seed::cipher(), lock);
        run::step(&mut e, &FixedClock(now)).await.unwrap();
        assert_eq!(clock_reads(&t), 1);
        match change {
            // Both actions between passes: the loop never sees status=stopped.
            "stop" => { e.primary.execute("UPDATE bots SET stopped_at='2026-09-01 14:01:00' WHERE id=?1", [id]).unwrap(); }
            "start" => { e.primary.execute("UPDATE bots SET started_at='2026-09-01 14:01:00' WHERE id=?1", [id]).unwrap(); }
            "composition" => { e.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)) WHERE id=?2",
                rusqlite::params![json!({s.btc.to_string():0.5, model::load_bot(&e.primary,id).unwrap().asset_ids()[0].to_string():0.5}).to_string(),id]).unwrap(); }
            "crypto" => { e.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)) WHERE id=?2",
                rusqlite::params![json!({s.btc.to_string():1.0}).to_string(),id]).unwrap(); }
            _ => unreachable!(),
        }
        run::step(&mut e, &FixedClock(now + Duration::minutes(2))).await.unwrap();
        assert!(!t.posted_orders().is_empty(), "{change}: the old wait is discarded");
        assert_eq!(clock_reads(&t), if change == "crypto" {1} else {2}, "{change}");
    }
}
