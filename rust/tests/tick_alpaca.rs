mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::placement::{self, Recovery};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{model, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::ruby::BigDec;
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

const PRE_SEND: &str = "Faraday::ConnectionFailed: Connection refused - connect(2) for \"paper-api.alpaca.markets\" port 443";
const POST_SEND: &str = "Faraday::TimeoutError: Net::ReadTimeout";

fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn script(over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": 64321.5, "bp": 64300.25 } } })),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": { "BTC/USD": { "p": 64310.75 } } })),
        "POST /v2/orders": ok(json!({ "id": "OTX-1", "status": "pending_new" })),
        "GET /v2/account": ok(json!({ "cash": "100000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
fn setup(spec: BotSpec) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded) {
    let (d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &spec);
    (d, o, id, s)
}
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
async fn tick_until_settled(o: &store::Opened, v: &AlpacaVenue<ScriptedTransport>, id: i64, start: DateTime<Utc>) -> TickOutcome {
    let (mut now, mut attempts) = (start, Attempts::default());
    loop {
        match tick::tick(&o.primary, v, id, &FixedClock(now), &mut attempts).await.unwrap() {
            TickOutcome::RetryAfter(d) => now += Duration::from_std(d).unwrap(),
            other => return other,
        }
    }
}
fn weekly() -> BotSpec { BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
const T0: &str = "2026-09-01T10:00:00.5Z";

#[tokio::test(flavor = "current_thread")]
async fn a_market_tick_sends_a_notional_gtc_buy_with_a_client_order_id() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({}));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(t.posted_orders(), vec![json!({ "symbol": "BTC/USD", "side": "buy", "type": "market", "time_in_force": "gtc", "notional": "60.00" })]);
    let sent = t.requests().into_iter().find(|r| r.method == "POST").unwrap();
    assert_eq!(sent.body.unwrap()["client_order_id"].as_str().map(str::len), Some(36), "Rust's own UUID (Rails sends none)");
    assert_eq!(one::<String>(&o, "SELECT external_id FROM transactions"), "OTX-1");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_limit_tick_sends_qty_and_limit_price_floored_and_formatted_as_rails() {
    let (_d, o, id, _) = setup(weekly().with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)));
    let t = script(json!({}));
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    // 64310.75 × 0.9975 = 64149.973125 → 64149.97; 60.00 / 64149.97 → 0.000935308 at 9 base decimals.
    assert_eq!(t.posted_orders(), vec![json!({ "symbol": "BTC/USD", "side": "buy", "type": "limit", "time_in_force": "gtc", "qty": "0.000935308", "limit_price": "64149.97" })]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_balance_transport_failure_after_a_placement_reschedules_without_retrying() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "GET /v2/account": [{ "network": "post_send", "message": POST_SEND }] }));
    let mut attempts = Attempts::default();
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut attempts).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    assert_eq!(t.posted_orders().len(), 1, "a retry would buy twice");
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!((b.status, b.last_failure_kind().as_deref()), (BotStatus::Retrying, Some("transient")));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event LIKE 'execution_%'"), 0, "Rails writes no activity row here");
    assert_eq!(attempts.transient, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_same_failure_before_any_placement_retries() {
    let (_d, o, id, _) = setup(BotSpec::weekly(0.4, "2026-09-01 10:00:00")); // under the 1 USD minimum: a skipped row, nothing placed
    let t = script(json!({ "GET /v2/account": [{ "network": "post_send", "message": POST_SEND }] }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::RetryAfter(_)), "{out:?}");
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_price_transport_failure_reaches_retry_on_with_the_clients_own_message() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "GET /v1beta3/crypto/us/latest/quotes": [{ "network": "pre_send", "message": PRE_SEND }] }));
    assert!(matches!(tick_until_settled(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled));
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_retrying'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap()["error"], PRE_SEND, "no \"No price for\" prefix: the composition re-raises the client's error");
}

#[tokio::test(flavor = "current_thread")]
async fn a_zero_ask_is_rails_wrong_price_error_naming_the_base() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": 0 } } })) }));
    assert!(matches!(tick_until_settled(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled));
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_retrying'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap()["error"], "No price for BTC: Wrong ask price for BTC: 0.0");
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn the_sweep_ignores_a_rejected_order_and_the_tick_goes_on() {
    let (_d, o, id, s) = setup(weekly());
    seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(0), external_id: Some("OREJ".into()), order_type: 0, amount: None,
        quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-08-25 10:00:01".into() });
    let t = script(json!({ "GET /v2/orders/OREJ": ok(json!({ "id": "OREJ", "status": "rejected", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "60", "qty": null, "filled_qty": "0", "filled_avg_price": null, "limit_price": null })) }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(one::<i64>(&o, "SELECT external_status FROM transactions WHERE external_id = 'OREJ'"), 0, "still unknown: Rails' polls have no branch for :failed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_partially_filled_order_fails_every_tick_before_placement() {
    let (_d, o, id, s) = setup(weekly());
    seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(1), external_id: Some("OPART".into()), order_type: 1, amount: Some("0.000935"),
        quote_amount: None, price: Some("64150"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-08-25 10:00:01".into() });
    let t = script(json!({ "GET /v2/orders/OPART": ok(json!({ "id": "OPART", "status": "partially_filled", "symbol": "BTC/USD", "type": "limit", "side": "buy",
        "notional": null, "qty": "0.000935", "filled_qty": "0.0004", "filled_avg_price": "64150", "limit_price": "64150" })) }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    assert!(t.posted_orders().is_empty(), "the sweep raises before anything is placed");
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap()["error"], "Order OPART status is unknown.");
    assert_eq!(one::<f64>(&o, "SELECT amount_exec FROM transactions WHERE external_id = 'OPART'"), 0.0, "the row is not updated");
}

#[tokio::test(flavor = "current_thread")]
async fn a_poll_answered_with_an_http_error_is_a_general_failure() {
    let (_d, o, id, s) = setup(weekly());
    seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(0), external_id: Some("OERR".into()), order_type: 0, amount: None,
        quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-08-25 10:00:01".into() });
    let t = script(json!({ "GET /v2/orders/OERR": [{ "status": 429, "body": { "code": 42910000, "message": "rate limit exceeded" } }] }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "not throttled, not retried: {out:?}");
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap()["error"], "Failed to fetch orders OERR. Result: [\"rate limit exceeded\"]");
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_is_absent_only_after_20_minutes_and_never_sent_twice() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
    let v = venue(&t);
    let t0 = at(T0);
    assert!(matches!(tick::tick(&o.primary, &v, id, &FixedClock(t0), &mut Attempts::default()).await.unwrap(), TickOutcome::AwaitingReconciliation));
    t.reply("GET /v2/orders:by_client_order_id", 404, json!({ "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" })); // Alpaca's own envelope
    let bot = || model::load_bot(&o.primary, id).unwrap();
    assert!(matches!(placement::recover(&o.primary, &v, &bot(), &FixedClock(t0 + Duration::seconds(1199))).await.unwrap(), Recovery::Pending),
            "inside the 20-minute margin (Linux tcp_retries2 gives a still-owned socket ~924 s)");
    assert!(matches!(placement::recover(&o.primary, &v, &bot(), &FixedClock(t0 + Duration::seconds(1200))).await.unwrap(), Recovery::NotPlaced));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0);
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'placement_ambiguous' ORDER BY id DESC LIMIT 1");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap()["error"], "the order never reached Alpaca");
    assert_eq!(t.posted_orders().len(), 1, "never sent twice");
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_whose_order_landed_is_recorded_with_its_fill() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
    let v = venue(&t);
    let t0 = at(T0);
    tick::tick(&o.primary, &v, id, &FixedClock(t0), &mut Attempts::default()).await.unwrap();
    let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    t.reply("GET /v2/orders:by_client_order_id", 200, json!({ "id": "OTX-9", "client_order_id": cl, "status": "filled", "symbol": "BTC/USD", "type": "market",
        "side": "buy", "notional": "60", "qty": null, "filled_qty": "0.000932719", "filled_avg_price": "64328.1", "limit_price": null }));
    let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), &FixedClock(t0 + Duration::seconds(5))).await.unwrap();
    assert!(matches!(recovered, Recovery::Recorded(_)), "{recovered:?}");
    let (ext, status, exec): (String, i64, f64) = o.primary.query_row("SELECT external_id, external_status, quote_amount_exec FROM transactions", [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((ext.as_str(), status), ("OTX-9", 2));
    assert_eq!(exec.to_bits(), (&BigDec::parse("0.000932719").unwrap() * &BigDec::parse("64328.1").unwrap()).round(18).to_f().to_bits());
    assert_eq!(t.posted_orders().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_certificate_failure_reading_the_balance_is_not_low_and_the_tick_succeeds() {
    // Rails: Client.network_failure returns a Failure for an SSL cause, and Bot::Fundable reads a Failure as "not low".
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "GET /v2/account": [{ "network": "permanent", "message": "Faraday::SSLError: certificate verify failed" }] }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bots WHERE last_end_of_funds_notification IS NOT NULL"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_funds_check_spends_non_marginable_buying_power_not_cash() {
    // Buffer: 60 USD a week = 60 / 604800 × 259200 ≈ 25.71 USD.
    for (cash, nmbp, notified) in [("100000", "1", true), ("1", "100000", false)] {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "GET /v2/account": ok(json!({ "cash": cash, "buying_power": "200000", "non_marginable_buying_power": nmbp })) }));
        tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
        assert_eq!(one::<bool>(&o, "SELECT last_end_of_funds_notification IS NOT NULL FROM bots"), notified, "cash {cash}, non-marginable {nmbp}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_retry_within_five_seconds_reuses_the_cached_price() {
    use deltabadger::engine::tick::{PriceCache, TickContext};
    let (_d, o, id, _) = setup(weekly().with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)));
    let t = script(json!({
        "GET /v1beta3/crypto/us/latest/trades": [{ "status": 200, "body": { "trades": { "BTC/USD": { "p": 64310.75 } } } },
                                                 { "status": 200, "body": { "trades": { "BTC/USD": { "p": 70000 } } } }],
        "POST /v2/orders": [{ "network": "pre_send", "message": PRE_SEND }, { "status": 200, "body": { "id": "OTX-1", "status": "new" } }] }));
    let prices = PriceCache::default();
    let cx = TickContext { prices: &prices, process_start: at(T0), stopping: &|| false };
    let (mut now, mut attempts) = (at(T0), Attempts::default());
    loop {
        match tick::tick_recovering(&o.primary, &venue(&t), id, &FixedClock(now), &mut attempts, &mut None, &cx).await.unwrap() {
            TickOutcome::RetryAfter(d) => now += Duration::from_std(d).unwrap(), // 3 s: inside the cache's 5 s
            other => { assert!(matches!(other, TickOutcome::Done { placed: true }), "{other:?}"); break; }
        }
    }
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/trades")).count(), 1, "the retry read the cache, as Rails' does");
    assert_eq!(t.posted_orders()[1]["limit_price"], "64149.97", "the second POST uses the cached 64310.75, not 70000");
}

#[tokio::test(flavor = "current_thread")]
async fn an_insufficient_funds_rejection_stamps_the_shared_daily_budget() {
    let (_d, o, a, s) = setup(weekly());
    let b = seed::insert_bot(&o.primary, &s, &weekly());
    let rejected = || script(json!({ "POST /v2/orders": [{ "status": 403, "body": { "code": 40310000, "message": "insufficient buying power" } }] }));
    let stamp = |id: i64| -> Option<String> { o.primary.query_row("SELECT last_end_of_funds_notification FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    tick::tick(&o.primary, &venue(&rejected()), a, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let first = stamp(a).expect("Bot::Failable stamps it when it notifies");
    tick::tick(&o.primary, &venue(&rejected()), a, &FixedClock(at(T0) + Duration::hours(1)), &mut Attempts::default()).await.unwrap();
    assert_eq!(stamp(a), Some(first.clone()), "at most once a day");
    tick::tick(&o.primary, &venue(&rejected()), b, &FixedClock(at(T0) + Duration::hours(2)), &mut Attempts::default()).await.unwrap();
    assert_eq!(stamp(b), None, "the budget is per user and quote asset: bot a's notice covers bot b");
    tick::tick(&o.primary, &venue(&rejected()), a, &FixedClock(at(T0) + Duration::hours(25)), &mut Attempts::default()).await.unwrap();
    assert_ne!(stamp(a), Some(first), "a day later it notifies again");
}

#[tokio::test(flavor = "current_thread")]
async fn a_send_resumed_past_its_bound_never_reaches_alpaca() {
    // The process passes the freshness check, then is suspended; it resumes 60 s later in real time.
    use deltabadger::engine::{amount, placement::Sent, venue_rules::ALPACA};
    use deltabadger::venue::http::{client, ReqwestTransport};
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let (_d, o, id, _) = setup(weekly());
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
    let v = AlpacaVenue::new(ReqwestTransport::new(client(), "k".into(), "s".into()), Urls { trading: server.uri(), data: server.uri() });
    let bot = model::load_bot(&o.primary, id).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(64_000), ALPACA.minimum_logic).unwrap() else { panic!() };
    let at_ = chrono::Utc::now() - Duration::seconds(60);
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(at_)).unwrap();
    assert!(matches!(placement::send(&v, &intent, &FixedClock(at_)).await, Sent::NotSent(_)), "the freshness check passed; the bound did not");
}

#[tokio::test(flavor = "current_thread")]
async fn a_restarted_process_trusts_an_alpaca_absence_only_a_full_window_after_it_started() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }],
                           "GET /v2/orders:by_client_order_id": [{ "status": 404, "body": { "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" } }] }));
    let v = venue(&t);
    let t0 = at(T0);
    tick::tick(&o.primary, &v, id, &FixedClock(t0), &mut Attempts::default()).await.unwrap(); // the intent stays
    let restart = t0 + Duration::seconds(600); // killed after the send, restarted ten minutes later
    let bot = || model::load_bot(&o.primary, id).unwrap();
    assert!(matches!(placement::recover_since(&o.primary, &v, &bot(), &FixedClock(restart), restart).await.unwrap(), Recovery::Pending),
            "past at + 20 min, but this process has not watched a full margin");
    assert!(matches!(placement::recover_since(&o.primary, &v, &bot(), &FixedClock(restart + Duration::seconds(1199)), restart).await.unwrap(), Recovery::Pending));
    assert!(matches!(placement::recover_since(&o.primary, &v, &bot(), &FixedClock(restart + Duration::seconds(1200)), restart).await.unwrap(), Recovery::NotPlaced));
}

#[tokio::test(flavor = "current_thread")]
async fn a_five_minute_smart_bot_waits_on_its_unresolved_intent_and_never_places_twice() {
    // Hourly 60 in smart chunks of 5: a checkpoint every 5 minutes, each one blocked by the ambiguous first send.
    let (_d, o, id, _) = setup(weekly().with("interval", json!("hour")).with("smart_intervaled", json!(true)).with("smart_interval_quote_amount", json!(5.0)));
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, { "status": 200, "body": { "id": "OTX-2", "status": "pending_new" } }],
                           "GET /v2/orders:by_client_order_id": [{ "status": 404, "body": { "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" } }] }));
    let v = venue(&t);
    let t0 = at(T0);
    assert!(matches!(tick::tick(&o.primary, &v, id, &FixedClock(t0), &mut Attempts::default()).await.unwrap(), TickOutcome::AwaitingReconciliation));
    for k in 1..4 {
        let out = tick::tick(&o.primary, &v, id, &FixedClock(t0 + Duration::minutes(5 * k)), &mut Attempts::default()).await.unwrap();
        assert!(matches!(out, TickOutcome::AwaitingReconciliation), "checkpoint +{} min: {out:?}", 5 * k);
    }
    assert!(matches!(tick::tick(&o.primary, &v, id, &FixedClock(t0 + Duration::seconds(1199)), &mut Attempts::default()).await.unwrap(), TickOutcome::AwaitingReconciliation));
    assert_eq!(t.posted_orders().len(), 1, "no second placement while the first may still land");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_some());
    // At the margin the absence is proven and that same tick buys again, once.
    tick::tick(&o.primary, &v, id, &FixedClock(t0 + Duration::seconds(1200)), &mut Attempts::default()).await.unwrap();
    assert_eq!(t.posted_orders().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn the_follow_up_poll_raises_as_rails_job_does() {
    use deltabadger::engine::polling::{self, PollFailure};
    let (_d, o, id, s) = setup(weekly());
    let tx = seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(1), external_id: Some("OPOLL".into()), order_type: 1, amount: Some("0.000935"),
        quote_amount: None, price: Some("64150"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-01 10:00:01".into() });
    let partial = script(json!({ "GET /v2/orders/OPOLL": ok(json!({ "id": "OPOLL", "status": "partially_filled", "symbol": "BTC/USD", "type": "limit", "side": "buy",
        "notional": null, "qty": "0.000935", "filled_qty": "0.0004", "filled_avg_price": "64150", "limit_price": "64150" })) }));
    let now = at("2026-09-01T10:00:06Z");
    assert_eq!(polling::follow_up(&o.primary, &venue(&partial), id, tx, now).await, Err(PollFailure::General("Order OPOLL status is unknown.".into())));
    assert_eq!(one::<f64>(&o, "SELECT amount_exec FROM transactions WHERE external_id = 'OPOLL'"), 0.0, "the row is not updated");
    let failing = script(json!({ "GET /v2/orders/OPOLL": [{ "status": 500, "body": { "message": "internal server error" } }] }));
    assert_eq!(polling::follow_up(&o.primary, &venue(&failing), id, tx, now).await,
               Err(PollFailure::General(format!("Failed to fetch order {tx}. Result: [\"internal server error\"]"))), "Rails names the transaction id");
}

// The engine's own process_start wiring (run.rs, step_bot and reconcile_idle): a restarted `run` must not trust an
// Alpaca 404 until a full margin after ITS start, even when the intent is far older than the margin.
struct ScriptedFactory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for ScriptedFactory {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(&self.0) }
}

async fn a_restarted_engine_keeps_an_old_intent(status: Option<i64>) {
    let dir = common::rails_install();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &weekly());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }],
                           "GET /v2/orders:by_client_order_id": [{ "status": 404, "body": { "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" } }] }));
    let t0 = at(T0);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(t0), &mut Attempts::default()).await.unwrap(); // the ambiguous send leaves the intent
    if let Some(st) = status { o.primary.execute("UPDATE bots SET status = ?1 WHERE id = ?2", rusqlite::params![st, id]).unwrap(); }
    assert_eq!(t.posted_orders().len(), 1);

    // A new process, 30 min after the intent: its own first step is its start.
    let mut e = deltabadger::engine::run::Engine::new(o.primary, ScriptedFactory(t.clone()), seed::cipher(), lock);
    deltabadger::engine::run::step(&mut e, &FixedClock(t0 + Duration::minutes(30))).await.unwrap();
    assert!(t.requests().iter().any(|r| r.path.contains("by_client_order_id")), "the lookup must have run");
    assert!(model::load_bot(&e.primary, id).unwrap().rust_placement().is_some(), "20 min have not passed since this process started");
    assert_eq!(t.posted_orders().len(), 1, "no second POST");
}

#[tokio::test(flavor = "current_thread")]
async fn a_restarted_engine_keeps_a_running_bots_old_intent_until_a_margin_after_its_own_start() { a_restarted_engine_keeps_an_old_intent(None).await }

#[tokio::test(flavor = "current_thread")]
async fn a_restarted_engine_keeps_a_stopped_bots_old_intent_until_a_margin_after_its_own_start() { a_restarted_engine_keeps_an_old_intent(Some(2)).await }

// A venue number this build cannot read (non-finite, garbage, out of BigDec's range) is an error, never zero. Ruby's
// String#to_d gives 0 for garbage and NaN/Infinity for those words; the engine refuses both on purpose.
const UNREADABLE: [&str; 4] = ["NaN", "Infinity", "garbage", "-Infinity"];
/// Outside BigDec's own range, or outside the venue-number caps (exponent ±40, 64 significant digits).
const OUT_OF_RANGE: [&str; 4] = ["1e-1000000000", "1e41", "1e-41", "1.00000000000000000000000000000000000000000000000000000000000000001"];
fn every_bad() -> impl Iterator<Item = &'static str> { UNREADABLE.into_iter().chain(OUT_OF_RANGE) }

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_price_places_nothing_and_retries() {
    for bad in UNREADABLE {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": bad } } })) }));
        assert!(matches!(tick_until_settled(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled), "{bad}");
        assert!(t.posted_orders().is_empty(), "{bad}: nothing is placed");
        assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/quotes")).count(), 4, "{bad}: transient, retried as retry_on does");
        let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_retrying'");
        let error = serde_json::from_str::<Value>(&details).unwrap()["error"].as_str().unwrap().to_string();
        assert!(error.starts_with("No price for BTC: ") && error.contains("unreadable") && error.contains(bad), "{bad}: {error}");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_balance_after_a_placement_reschedules_without_a_second_buy_or_a_funds_notice() {
    for bad in UNREADABLE {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "GET /v2/account": ok(json!({ "cash": "100000", "non_marginable_buying_power": bad })) }));
        let mut attempts = Attempts::default();
        let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut attempts).await.unwrap();
        assert!(matches!(out, TickOutcome::Rescheduled), "{bad}: {out:?}");
        assert_eq!(t.posted_orders().len(), 1, "{bad}: a retry would buy twice");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bots WHERE last_end_of_funds_notification IS NOT NULL"), 0, "{bad}: not read as a zero balance");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_balance_before_any_placement_retries() {
    let (_d, o, id, _) = setup(BotSpec::weekly(0.4, "2026-09-01 10:00:00")); // under the minimum: nothing placed
    let t = script(json!({ "GET /v2/account": ok(json!({ "cash": "NaN" })) }));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::RetryAfter(_)), "{out:?}");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bots WHERE last_end_of_funds_notification IS NOT NULL"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_placement_answer_with_an_unreadable_fill_is_ambiguous_and_records_no_fill() {
    for field in ["filled_qty", "filled_avg_price"] {
        for bad in every_bad() {
            let (_d, o, id, _) = setup(weekly());
            let mut answer = json!({ "id": "OTX-1", "status": "filled", "filled_qty": "0.000932719", "filled_avg_price": "64328.1" });
            answer[field] = json!(bad);
            let t = script(json!({ "POST /v2/orders": ok(answer) }));
            let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
            assert!(matches!(out, TickOutcome::AwaitingReconciliation), "{field}={bad}: {out:?}");
            assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0, "{field}={bad}: no row, so no zero fill");
            assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_some(), "{field}={bad}: the intent stays");
            assert_eq!(t.posted_orders().len(), 1);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_recovery_lookup_with_an_unreadable_fill_stays_pending() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
    let v = venue(&t);
    let t0 = at(T0);
    tick::tick(&o.primary, &v, id, &FixedClock(t0), &mut Attempts::default()).await.unwrap();
    let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    t.reply("GET /v2/orders:by_client_order_id", 200, json!({ "id": "OTX-9", "client_order_id": cl, "status": "filled", "symbol": "BTC/USD", "type": "market",
        "side": "buy", "notional": "60", "qty": null, "filled_qty": "NaN", "filled_avg_price": "64328.1", "limit_price": null }));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let recovered = placement::recover(&o.primary, &v, &bot, &FixedClock(t0 + Duration::seconds(1300))).await.unwrap();
    assert!(matches!(recovered, Recovery::Pending), "{recovered:?}");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0, "no zero fill recorded");
    assert_eq!(t.posted_orders().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_poll_with_an_unreadable_fill_records_no_zero_fill() {
    use deltabadger::engine::polling;
    for (field, bad) in [("filled_qty", "NaN"), ("filled_avg_price", "NaN"), ("filled_qty", "Infinity"), ("filled_avg_price", "garbage"),
                         ("filled_qty", "1e-1000000000"), ("filled_qty", "1e350"), ("filled_avg_price", "1e41")] {
        let (_d, o, id, s) = setup(weekly());
        let tx = seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(0), external_id: Some("OFILL".into()), order_type: 0, amount: None,
            quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() });
        let mut answer = json!({ "id": "OFILL", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "60", "qty": null,
            "filled_qty": "0.000932719", "filled_avg_price": "64328.1", "limit_price": null });
        answer[field] = json!(bad);
        let t = script(json!({ "GET /v2/orders/OFILL": ok(answer) }));
        let now = at("2026-09-01T10:00:06Z");
        assert!(polling::follow_up(&o.primary, &venue(&t), id, tx, now).await.is_err(), "{field}={bad}");
        let (ext, exec): (i64, Option<f64>) = o.primary.query_row("SELECT external_status, quote_amount_exec FROM transactions WHERE id = ?1", [tx], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((ext, exec), (0, None), "{field}={bad}: the row is untouched");
        // The sweep in front of the next tick fails the same way, so nothing new is bought on top of it.
        let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0) + Duration::days(7)), &mut Attempts::default()).await.unwrap();
        assert!(matches!(out, TickOutcome::Rescheduled), "{field}={bad}: the sweep's general failure: {out:?}");
        let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
        assert!(details.contains("unreadable"), "{field}={bad}: {details}");
        assert!(t.posted_orders().is_empty(), "{field}={bad}");
        assert_eq!(one::<Option<f64>>(&o, "SELECT quote_amount_exec FROM transactions"), None);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_price_outside_bigdecs_range_places_nothing_and_retries() {
    for bad in ["1e-1000000000", "1e1000000000", "1e-401", "1e300", "1e-300", "1e41"] {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": bad } } })) }));
        assert!(matches!(tick_until_settled(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled), "{bad}");
        assert!(t.posted_orders().is_empty(), "{bad}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_ticker_precision_out_of_range_fails_the_tick_without_an_order() {
    // Eligibility refuses this install; a tick that runs anyway must still fail cleanly, not size with it.
    let (_d, o, id, _) = setup(weekly());
    o.primary.execute("UPDATE tickers SET price_decimals = 1000000000", []).unwrap();
    let t = script(json!({}));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    assert!(t.posted_orders().is_empty());
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
    assert!(details.contains("price_decimals 1000000000"), "{details}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_recovery_lookup_with_an_out_of_range_fill_stays_pending() {
    for bad in OUT_OF_RANGE {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
        let v = venue(&t);
        tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
        let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
        t.reply("GET /v2/orders:by_client_order_id", 200, json!({ "id": "OTX-9", "client_order_id": cl, "status": "filled", "symbol": "BTC/USD", "type": "market",
            "side": "buy", "notional": "60", "qty": null, "filled_qty": bad, "filled_avg_price": "64328.1", "limit_price": null }));
        let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), &FixedClock(at(T0) + Duration::seconds(1300))).await.unwrap();
        assert!(matches!(recovered, Recovery::Pending), "{bad}: {recovered:?}");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0, "{bad}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_plan_whose_intent_would_not_read_back_is_refused_before_anything_is_written_or_sent() {
    use deltabadger::engine::{amount, venue_rules::ALPACA};
    let (_d, o, id, _) = setup(weekly());
    let bot = model::load_bot(&o.primary, id).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    // A 1e300 ask (venue caps refuse it now; the check must not rely on them): the amount, 6e-299, is 301 characters written out.
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::parse("1e300").unwrap(), ALPACA.minimum_logic).unwrap() else { panic!() };
    let err = placement::begin(&o.primary, &bot, &plan, &FixedClock(at(T0))).unwrap_err();
    assert!(format!("{err:?}").contains("read back"), "{err:?}");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none(), "no intent that recovery could not read");
}

#[tokio::test(flavor = "current_thread")]
async fn a_tick_whose_order_would_not_read_back_places_nothing() {
    // 1e300 USD a week sizes a notional of 301 digits: the intent could not be read back, so nothing is committed or sent.
    let (_d, o, id, _) = setup(BotSpec::weekly(1e300, "2026-09-01 10:00:00"));
    let t = script(json!({}));
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    assert!(t.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0);
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
    assert!(details.contains("read back"), "{details}");
}

// A bare JSON number serde would turn into 0.0 (1e-350), ±Inf or an error (1e400), or a 70-digit float: the whole raw
// body is unreadable before it is parsed, so none of them is ever a zero price, fill or balance.
const RAW_BAD: [&str; 3] = ["1e-350", "1e400", "1234567890123456789012345678901234567890123456789012345678901234567890"];
fn raw(body: String) -> Value { json!([{ "status": 200, "body": body }]) }

#[tokio::test(flavor = "current_thread")]
async fn a_raw_price_body_with_an_out_of_range_number_places_nothing() {
    for bad in RAW_BAD {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "GET /v1beta3/crypto/us/latest/quotes": raw(format!(r#"{{"quotes":{{"BTC/USD":{{"ap":{bad},"bp":64300.25}}}}}}"#)) }));
        assert!(matches!(tick_until_settled(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled), "{bad}");
        assert!(t.posted_orders().is_empty(), "{bad}");
        let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_retrying'");
        assert!(details.contains("unreadable"), "{bad}: {details}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_raw_placement_answer_with_an_out_of_range_fill_is_ambiguous() {
    for bad in RAW_BAD {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "POST /v2/orders": raw(format!(r#"{{"id":"OTX-1","status":"filled","filled_qty":{bad},"filled_avg_price":"64328.1"}}"#)) }));
        let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
        assert!(matches!(out, TickOutcome::AwaitingReconciliation), "{bad}: {out:?}");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0, "{bad}: no row, no zero fill");
        assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_some(), "{bad}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_raw_poll_or_lookup_with_an_out_of_range_fill_records_no_zero_fill() {
    use deltabadger::engine::polling;
    for bad in RAW_BAD {
        let (_d, o, id, s) = setup(weekly());
        let tx = seed::insert_tx(&o.primary, &s, id, &TxSpec { status: 0, external_status: Some(0), external_id: Some("OFILL".into()), order_type: 0, amount: None,
            quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() });
        let t = script(json!({ "GET /v2/orders/OFILL": raw(format!(r#"{{"id":"OFILL","status":"filled","type":"market","side":"buy","notional":"60","filled_qty":{bad},"filled_avg_price":"64328.1"}}"#)) }));
        assert!(polling::follow_up(&o.primary, &venue(&t), id, tx, at("2026-09-01T10:00:06Z")).await.is_err(), "{bad}");
        let (ext, exec): (i64, Option<f64>) = o.primary.query_row("SELECT external_status, amount_exec FROM transactions WHERE id = ?1", [tx], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((ext, exec), (0, None), "{bad}: the row is untouched");
    }
    for bad in RAW_BAD {
        let (_d, o, id, _) = setup(weekly());
        let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }] }));
        let v = venue(&t);
        tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
        let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
        t.reply("GET /v2/orders:by_client_order_id", 200, json!(format!(r#"{{"id":"OTX-9","client_order_id":"{cl}","status":"filled","type":"market","side":"buy","notional":"60","filled_qty":{bad},"filled_avg_price":"64328.1"}}"#)));
        let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), &FixedClock(at(T0) + Duration::seconds(1300))).await.unwrap();
        assert!(matches!(recovered, Recovery::Pending), "{bad}: {recovered:?}");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0, "{bad}: no zero fill recorded");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_not_found_answer_with_an_out_of_range_number_proves_nothing() {
    let (_d, o, id, _) = setup(weekly());
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }],
        "GET /v2/orders:by_client_order_id": [{ "status": 404, "body": r#"{"code":40410000,"message":"order not found","x":1e-350}"# }] }));
    let v = venue(&t);
    tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let recovered = placement::recover(&o.primary, &v, &model::load_bot(&o.primary, id).unwrap(), &FixedClock(at(T0) + Duration::seconds(1300))).await.unwrap();
    assert!(matches!(recovered, Recovery::Pending), "{recovered:?}");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_some(), "the intent stays");
}
