//! The Gemini trading adapter (venue::gemini) over recorded bodies, and the D-679 placement contract it is the first
//! consumer of (plan VB §1.5): a market buy sized from a quote budget at the crossed price it is sent at, a market sell's
//! base sent at the crossed bid, both prepared once and persisted in the intent, never re-read or resized at the send.
//! Rails-free.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::amount::{self, Origin, Sizing};
use deltabadger::engine::model;
use deltabadger::engine::placement::{self, OperatorResolution, Recovery, Sent};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::venue_rules::{GEMINI, KRAKEN};
use deltabadger::engine::FixedClock;
use deltabadger::ruby::BigDec;
use deltabadger::store::{self, Paths};
use deltabadger::venue::binance::Pairs;
use deltabadger::venue::gemini::{self, GeminiVenue, URL};
use deltabadger::venue::http::{self, ScriptedTransport};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Side, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn t0() -> DateTime<Utc> { "2026-09-01T10:00:00.5Z".parse().unwrap() }
fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn err(status: u16, reason: &str, message: &str) -> Value { json!({ "status": status, "body": { "result": "error", "reason": reason, "message": message } }) }
fn pubticker(bid: &str, ask: &str, last: &str) -> Value { ok(json!({ "bid": bid, "ask": ask, "last": last, "volume": { "BTC": "1", "EUR": "1", "timestamp": 1 } })) }
fn balances(eur: &str, btc: &str) -> Value {
    ok(json!([{ "type": "exchange", "currency": "BTC", "amount": btc, "available": btc, "availableForWithdrawal": btc },
              { "type": "exchange", "currency": "EUR", "amount": eur, "available": eur, "availableForWithdrawal": eur }]))
}
fn order(id: &str, live: bool, cancelled: bool, executed: &str, original: &str, avg: &str) -> Value {
    json!({ "order_id": id, "id": id, "symbol": "btceur", "exchange": "gemini", "avg_execution_price": avg, "side": "buy", "type": "exchange limit",
            "timestamp": "1788000000", "timestampms": 1788000000000u64, "is_live": live, "is_cancelled": cancelled, "executed_amount": executed,
            "remaining_amount": (&bd(original) - &bd(executed)).to_s_f(), "original_amount": original, "price": "64640.11", "options": ["immediate-or-cancel"] })
}

/// A Gemini install: the seeded Kraken one renamed, its pair `btceur` at Gemini's precisions (8 base, 2 price decimals).
fn install(spec: BotSpec) -> (tempfile::TempDir, store::Opened, i64) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute_batch("UPDATE exchanges SET type = 'Exchanges::Gemini', name = 'Gemini'; \
                             UPDATE tickers SET ticker = 'btceur', base = 'BTC', quote = 'EUR', base_decimals = 8, quote_decimals = 2, price_decimals = 2, \
                             minimum_base_size = '0.00001', minimum_quote_size = '0'").unwrap();
    let id = seed::insert_bot(&o.primary, &s, &spec);
    (dir, o, id)
}
fn venue(o: &store::Opened, t: &ScriptedTransport) -> GeminiVenue<ScriptedTransport> {
    let id: i64 = o.primary.query_row("SELECT id FROM exchanges", [], |r| r.get(0)).unwrap();
    let creds = deltabadger::crypto::Credentials { key: "KEY-A".into(), secret: "s".into(), passphrase: None, redaction_values: vec![] };
    GeminiVenue::new(t.clone(), URL).with_pairs(Pairs::load(&o.primary, id).unwrap()).with_identity(&creds)
}
fn bot(o: &store::Opened, id: i64) -> model::Bot { model::load_bot(&o.primary, id).unwrap() }
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
fn placed(t: &ScriptedTransport) -> Vec<Value> { t.requests().into_iter().filter(|r| r.path == gemini::ORDER_NEW).filter_map(|r| r.body).collect() }
fn reads(t: &ScriptedTransport) -> usize { t.requests().iter().filter(|r| r.path.starts_with("/v1/pubticker/")).count() }

// ---- the wire: every private call an empty-body POST, its params signed in the payload

#[tokio::test(flavor = "current_thread")]
async fn the_transport_sends_an_empty_body_with_the_params_signed_in_the_payload() {
    use base64::Engine;
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_string(r#"{"order_id":"77"}"#)).mount(&server).await;
    let at: DateTime<Utc> = "2026-09-20T02:30:00.750Z".parse().unwrap();
    let t = http::ReqwestTransport::gemini(http::client(), "KEY".into(), "SECRET".into()).with_clock(std::sync::Arc::new(FixedClock(at)));
    let v = GeminiVenue::new(t, &server.uri());
    let o = NewOrder { pair: "btcusd".into(), side: Side::Buy, kind: OrderKind::Crossed { price: "64640.11".into() }, volume: "0.00092821".into(),
                       quote_volume: false, cl_ord_id: "cl-1".into(), deadline: at + Duration::seconds(10), day: false };
    assert_eq!(v.add_order(&o).await, Ok("77".into()));
    assert_eq!(v.add_order(&o).await, Ok("77".into()));
    let got = server.received_requests().await.unwrap();
    let mut nonces = vec![];
    for g in &got {
        assert!(g.body.is_empty(), "nothing travels in the body");
        let header = |k: &str| g.headers.get(k).unwrap().to_str().unwrap().to_string();
        let payload: Value = serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(header("X-GEMINI-PAYLOAD")).unwrap()).unwrap();
        assert_eq!(payload, json!({ "symbol": "btcusd", "amount": "0.00092821", "price": "64640.11", "side": "buy", "type": "exchange limit",
                                    "client_order_id": "cl-1", "options": ["immediate-or-cancel"], "request": "/v1/order/new", "nonce": payload["nonce"] }));
        let params: Vec<(&str, Value)> = payload.as_object().unwrap().iter().filter(|(k, _)| !["request", "nonce"].contains(&k.as_str())).map(|(k, v)| (k.as_str(), v.clone())).collect();
        let nonce: u64 = payload["nonce"].as_str().unwrap().parse().unwrap();
        let want = deltabadger::venue::signers::gemini::headers("KEY", "SECRET", "/v1/order/new", &params, nonce);
        for (k, v) in want { assert_eq!(header(k), v, "{k}"); }
        nonces.push(nonce);
    }
    // SPEC-E §2.9: two calls in one ms of the transport's clock never share a nonce, and none is behind the clock.
    assert!(nonces[0] >= deltabadger::venue::binance::timestamp_ms(at) as u64 && nonces[1] > nonces[0], "{nonces:?}");
}

#[test]
fn the_gemini_log_line_is_labelled_gemini() {
    let t = http::ReqwestTransport::gemini(http::client(), "KEY".into(), "SECRET".into());
    assert_eq!(t.failure_line(&http::HttpResponse { status: 400, body: r#"{"result":"error"}"#.into() }), "[Gemini] HTTP 400; venue response diagnostic omitted");
}

// ---- reads (Exchanges::Gemini#get_bid_ask_price / #get_last_price / #get_balances / #get_tickers_prices)

fn ticker() -> model::Ticker {
    model::Ticker { id: 1, ticker: "btceur".into(), base_code: "BTC".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(),
                    exchange_name: "Gemini".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 2, price_decimals: 2,
                    minimum_base_size: bd("0.00001"), minimum_quote_size: bd("0"), trading_enabled: true, available: true, crypto: true }
}
fn bare(script: Value) -> (ScriptedTransport, GeminiVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    let mut p = Pairs::default();
    for code in ["BTC", "EUR"] { p.assets.insert(code.into(), code.into()); }
    (t.clone(), GeminiVenue::new(t, URL).with_pairs(p))
}

#[tokio::test(flavor = "current_thread")]
async fn prices_read_the_pubticker_and_refuse_a_zero_or_a_missing_side() {
    let (_, v) = bare(json!({ "GET /v1/pubticker/btceur": [pubticker("64000.01", "64000.11", "64000.05")] }));
    for (side, want) in [(PriceSide::Bid, "64000.01"), (PriceSide::Ask, "64000.11"), (PriceSide::Last, "64000.05")] { assert_eq!(v.price(&ticker(), side).await, Ok(bd(want))); }
    let (_, v) = bare(json!({ "GET /v1/pubticker/btceur": [pubticker("0", "0", "0")] }));
    assert_eq!(v.price(&ticker(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Wrong ask price for btceur: 0.0".into()])));
    let (_, v) = bare(json!({ "GET /v1/pubticker/btceur": [ok(json!({ "ask": "1", "last": "1" }))] }));
    assert_eq!(v.price(&ticker(), PriceSide::Ask).await, Err(VenueError::Ambiguous("Key path bid not found".into())), "#get_bid_ask_price digs both sides");
    let (_, v) = bare(json!({ "GET /v1/pubticker/btceur": [ok(json!({ "bid": "x", "ask": "1", "last": "1" }))] }));
    assert_eq!(v.price(&ticker(), PriceSide::Ask).await, Err(VenueError::Ambiguous(gemini::UNREADABLE.into())), "an unreadable number is never a price");
}

#[tokio::test(flavor = "current_thread")]
async fn balances_are_each_rows_available_amount_by_its_upcased_currency() {
    let (_, v) = bare(json!({ "POST /v1/balances": [ok(json!([{ "currency": "btc", "amount": "0.5", "available": "0.4" }, { "currency": "EUR", "amount": "10", "available": "9" }]))] }));
    assert_eq!(v.free_base_balance(&ticker()).await, Ok(bd("0.4")));
    assert_eq!(v.balance("EUR", true).await, Ok(bd("9")));
    let (_, v) = bare(json!({ "POST /v1/balances": [ok(Value::Null)] }));
    assert_eq!(v.balance("EUR", true).await, Err(VenueError::Rejected(vec!["Failed to get Gemini balances".into()])));
    let (_, v) = bare(json!({ "POST /v1/balances": [err(400, "InvalidApiKey", "Invalid API key")] }));
    assert!(matches!(v.balance("EUR", true).await, Err(VenueError::Rejected(m)) if GEMINI.is_invalid_key(&m)));
    let (_, v) = bare(json!({ "POST /v1/balances": [ok(json!([{ "currency": "EUR", "available": "9" }]))] }));
    assert_eq!(v.balance("EUR", true).await, Err(VenueError::Ambiguous("Key path amount not found".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn the_price_feed_is_every_pair_downcased() {
    let (_, v) = bare(json!({ "GET /v1/pricefeed": [ok(json!([{ "pair": "BTCUSD", "price": "64000.1", "percentChange24h": "0.01" }, { "pair": "ETHUSD", "price": "garbage" }]))] }));
    assert_eq!(v.tickers_prices(&[]).await, Ok(vec![("btcusd".into(), json!("64000.1"))]));
}

// ---- placement (Exchanges::Gemini#set_market_order / #set_limit_order) and its answers

fn new_order(kind: OrderKind) -> NewOrder {
    NewOrder { pair: "btceur".into(), side: Side::Sell, kind, volume: "0.004".into(), quote_volume: false, cl_ord_id: "cl-9".into(), deadline: t0() + Duration::seconds(10), day: false }
}

#[tokio::test(flavor = "current_thread")]
async fn a_crossed_order_is_an_ioc_limit_at_its_price_and_a_limit_order_rests() {
    let (t, v) = bare(json!({ "POST /v1/order/new": [ok(json!({ "order_id": 106817811 }))] }));
    assert_eq!(v.add_order(&new_order(OrderKind::Crossed { price: "1.21".into() })).await, Ok("106817811".into()));
    assert_eq!(v.add_order(&new_order(OrderKind::Limit { price: "1.25".into() })).await, Ok("106817811".into()));
    let sent = placed(&t);
    assert_eq!(sent[0], json!({ "symbol": "btceur", "amount": "0.004", "price": "1.21", "side": "sell", "type": "exchange limit", "client_order_id": "cl-9", "options": ["immediate-or-cancel"] }));
    assert_eq!(sent[1]["options"], json!([]));
    // Gemini has no market order, and takes none sized in quote: neither is ever sent.
    assert!(matches!(v.add_order(&new_order(OrderKind::Market)).await, Err(VenueError::Transient(_))));
    assert!(matches!(v.add_order(&NewOrder { quote_volume: true, ..new_order(OrderKind::Crossed { price: "1.21".into() }) }).await, Err(VenueError::Transient(_))));
    assert_eq!(placed(&t).len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_to_rails_ambiguity_rules() {
    let crossed = new_order(OrderKind::Crossed { price: "1.21".into() });
    for (reply, want) in [
        (err(400, "InsufficientFunds", "insufficient funds"), "rejected"),
        (err(403, "MissingRole", "lacks the role"), "rejected"),
        (err(400, "InvalidNonce", "Nonce has not increased"), "rejected"),
        (err(500, "System", "Internal error"), "ambiguous"),
        (err(503, "Maintenance", "Down"), "ambiguous"),
        (json!({ "status": 502, "body": "<html>bad gateway</html>" }), "ambiguous"),
        (ok(json!({ "symbol": "btceur" })), "ambiguous"),
        (json!({ "status": 200, "body": "<html>upstream</html>" }), "ambiguous"),
        (json!({ "network": "post_send", "message": "Net::ReadTimeout" }), "ambiguous"),
        (json!({ "network": "pre_send:api.gemini.com", "message": "Connection refused" }), "rejected"),
    ] {
        let (_, v) = bare(json!({ "POST /v1/order/new": [reply.clone()] }));
        let got = v.add_order(&crossed).await;
        assert_eq!(match got { Err(VenueError::Rejected(_)) => "rejected", Err(VenueError::Ambiguous(_)) => "ambiguous", other => panic!("{reply}: {other:?}") }, want, "{reply}");
    }
    // InvalidNonce is refused before the order reaches the book: placement-safe (M2), no failed row.
    assert!(placement::safe_rejection(&[r#"{"result":"error","reason":"InvalidNonce","message":"x"}"#.into()]));
    assert_eq!(GEMINI.failure_kind(&[r#"{"result":"error","reason":"System"}"#.into()]), Some("transient"));
    assert_eq!(GEMINI.failure_kind(&[r#"{"result":"error","reason":"MissingRole"}"#.into()]), Some("permission_denied"));
    assert!(GEMINI.add_outcome_unknown(&[r#"{"result":"error","reason":"Maintenance","message":"x"}"#.into()]));
}

// ---- polls (Exchanges::Gemini#get_order / #get_orders / #parse_order_data)

#[test]
fn every_documented_status_combination_maps_as_parse_order_status() {
    let st = |live, cancelled, executed: &str, original: &str| gemini::parse_order("1", &order("1", live, cancelled, executed, original, "0")).unwrap().status;
    assert_eq!(st(false, true, "0.0004", "0.00093"), OrderStatus::Cancelled, "an IOC that partly filled is cancelled with its fill");
    assert_eq!(st(true, false, "0", "0.00093"), OrderStatus::Open);
    assert_eq!(st(true, false, "0.00093", "0.00093"), OrderStatus::Unknown, "live with nothing remaining");
    assert_eq!(st(false, false, "0.00093", "0.00093"), OrderStatus::Closed);
    assert_eq!(st(false, false, "0", "0.00093"), OrderStatus::Unknown);
    let o = gemini::parse_order("7", &order("7", false, false, "0.0005", "0.0005", "64000.5")).unwrap();
    assert_eq!((o.price, o.amount, o.amount_exec.clone(), o.quote_amount_exec, o.limit, o.sell), (Some(bd("64000.5")), Some(bd("0.0005")), bd("0.0005"), Some(bd("32.00025")), true, false));
    let o = gemini::parse_order("7", &order("7", true, false, "0", "0.0005", "0")).unwrap();
    assert_eq!((o.price, o.quote_amount_exec), (Some(bd("64640.11")), Some(bd("0"))), "no execution: the order's own price, nothing executed");
    let mut m = order("7", false, false, "1", "1", "1");
    m["type"] = json!("exchange market");
    assert!(!gemini::parse_order("7", &m).unwrap().limit);
}

#[tokio::test(flavor = "current_thread")]
async fn an_order_gemini_does_not_know_is_missing_and_any_other_failure_fails_the_batch() {
    let gone = err(404, "OrderNotFound", "Order 5 not found");
    let (_, v) = bare(json!({ "POST /v1/order/status": [ok(order("4", false, false, "0.1", "0.1", "1")), gone] }));
    let got = v.orders(&["4".into(), "5".into()]).await.unwrap();
    assert_eq!(got.iter().map(|o| o.txid.as_str()).collect::<Vec<_>>(), ["4"], "5 is missing");
    let (_, v) = bare(json!({ "POST /v1/order/status": [err(404, "OrderNotFound", "x")] }));
    assert_eq!(v.order_identified("5", |_, _, _| Ok(true)).await, Err(VenueError::Rejected(vec!["Gemini no longer knows order 5".into()])));
    let (_, v) = bare(json!({ "POST /v1/order/status": [err(500, "System", "x")] }));
    assert!(v.orders(&["4".into()]).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_posts_the_order_id() {
    let (t, v) = bare(json!({ "POST /v1/order/cancel": [ok(order("4", false, true, "0", "0.1", "0"))] }));
    assert_eq!(v.cancel("4").await, Ok(()));
    assert_eq!(t.requests()[0].body, Some(json!({ "order_id": "4" })));
}

// ---- D-679 (plan VB §1.5): the crossed price, prepared once and persisted

fn gemini_script(bid: &str, ask: &str) -> Value {
    json!({ "GET /v1/pubticker/btceur": [pubticker(bid, ask, "64000.05")], "POST /v1/order/new": [ok(json!({ "order_id": "7612345678" }))],
            "POST /v1/balances": [balances("100000", "0.5")] })
}

#[tokio::test(flavor = "current_thread")]
async fn a_market_buy_is_sized_at_the_crossed_ask_it_is_sent_at_and_spends_within_its_budget() {
    let (_d, o, id) = install(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let t = ScriptedTransport::from_script(&gemini_script("64000.01", "64000.11"));
    assert!(matches!(tick::tick(&o.primary, &venue(&o, &t), id, &FixedClock(t0()), &mut Attempts::default()).await.unwrap(), TickOutcome::Done { placed: true }));
    let sent = placed(&t);
    // adjust(adjust(64000.11) × 1.01) = 64640.11; floor(60 / 64640.11, 8) = 0.00092821 (Rails sends 0.00093749, sized at the raw ask).
    assert_eq!((sent[0]["price"].as_str(), sent[0]["amount"].as_str(), &sent[0]["options"]), (Some("64640.11"), Some("0.00092821"), &json!(["immediate-or-cancel"])));
    assert!(&bd("0.00092821") * &bd("64640.11") <= bd("60"), "a fill at the cross spends at most the budget");
    assert_eq!(reads(&t), 1, "one price read: the crossed price is the sizing read's, never read again at placement");
    let (price, amount, quote): (f64, f64, f64) = o.primary.query_row("SELECT price, amount, quote_amount FROM transactions", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((price, quote), (64640.11, 60.0));
    assert!((amount - 60.0 / 64640.11).abs() < 1e-12, "the row's base is the budget at the price it was sent at");
}

#[tokio::test(flavor = "current_thread")]
async fn below_the_minimum_at_the_crossed_ask_is_the_dca_legs_skipped_row() {
    let (_d, o, id) = install(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE tickers SET minimum_base_size = '0.00093'", []).unwrap(); // 0.00093749 at the ask, 0.00092821 at the cross
    let t = ScriptedTransport::from_script(&gemini_script("64000.01", "64000.11"));
    assert!(matches!(tick::tick(&o.primary, &venue(&o, &t), id, &FixedClock(t0()), &mut Attempts::default()).await.unwrap(), TickOutcome::Done { placed: false }));
    assert!(placed(&t).is_empty());
    assert_eq!((one::<i64>(&o, "SELECT status FROM transactions"), one::<String>(&o, "SELECT event FROM bot_activity_logs")), (2, "order_skipped".into()));
}

/// The plan the tick prepares for a 60 EUR market buy at the ask 64000.11 (D-679): sized at the crossed 64640.11.
fn crossed_plan(o: &store::Opened, b: &model::Bot) -> amount::OrderPlan {
    let ticker = model::ticker_for(&o.primary, b).unwrap().unwrap();
    let crossed = GEMINI.market_price(&bd("64000.11"), &ticker, Side::Buy).unwrap();
    let Sizing::Place(plan) = amount::size(b, &ticker, &bd("60"), &crossed, &GEMINI).unwrap() else { panic!() };
    let price = plan.price.clone();
    plan.with_wire_price(&GEMINI, &price, true).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn the_intent_persists_origin_price_wire_price_and_volume_and_the_send_reads_nothing_again() {
    let (_d, o, id) = install(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let b = bot(&o, id);
    let intent = placement::begin(&o.primary, &b, &crossed_plan(&o, &b), &FixedClock(t0())).unwrap();
    let raw = bot(&o, id).rust_placement().unwrap();
    assert_eq!((raw["origin"].as_str(), raw["price"].as_str(), raw["wire_price"].as_str(), raw["volume"].as_str()),
               (Some("quote_budget"), Some("64640.11"), Some("64640.11"), Some("0.00092821")));
    // The book moved after preparation: nothing is read again, the persisted price and volume go out unchanged.
    let t = ScriptedTransport::from_script(&json!({ "POST /v1/order/new": [ok(json!({ "order_id": "88" }))] })); // no pubticker scripted: a read would panic
    assert!(matches!(placement::send_recorded(&o.primary, &venue(&o, &t), &b, &intent, &FixedClock(t0())).await.unwrap(), Sent::Accepted(_)));
    assert_eq!((placed(&t)[0]["price"].as_str(), placed(&t)[0]["amount"].as_str()), (Some("64640.11"), Some("0.00092821")));
}

#[tokio::test(flavor = "current_thread")]
async fn after_a_crash_between_preparation_and_send_the_intent_reads_back_whole_and_nothing_is_re_derived() {
    let (_d, o, id) = install(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let b = bot(&o, id);
    placement::begin(&o.primary, &b, &crossed_plan(&o, &b), &FixedClock(t0())).unwrap();
    // The process died before the send. Gemini's lookup can only find an order, so the never-sent intent waits.
    let t = ScriptedTransport::from_script(&json!({ "POST /v1/order/status": [err(404, "OrderNotFound", "x")] }));
    for days in [1, 31] {
        let at = FixedClock(t0() + Duration::days(days));
        assert!(matches!(placement::recover_since(&o.primary, &venue(&o, &t), &bot(&o, id), &at, t0()).await.unwrap(), Recovery::Pending), "+{days}d: absence is never concluded");
    }
    assert_eq!(t.requests().iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), [gemini::ORDER_STATUS, gemini::ORDER_STATUS], "no price is read, nothing is sent");
    // The operator says it was placed: the row is the intent's own plan, read back, at the crossed price.
    placement::resolve_by_operator(&o.primary, id, OperatorResolution::Placed("99".into()), t0() + Duration::days(31)).unwrap();
    let (price, amount): (f64, f64) = o.primary.query_row("SELECT price, amount FROM transactions WHERE external_id = '99'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(price, 64640.11);
    assert!((amount - 60.0 / 64640.11).abs() < 1e-12);
}

#[tokio::test(flavor = "current_thread")]
async fn an_ambiguous_send_keeps_the_intent_and_the_client_order_id_lookup_finds_it() {
    let (_d, o, id) = install(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let b = bot(&o, id);
    let intent = placement::begin(&o.primary, &b, &crossed_plan(&o, &b), &FixedClock(t0())).unwrap();
    let v = venue(&o, &ScriptedTransport::default());
    let identity = v.placement_identity().unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.host', ?1, '$.rust_placement.key_fingerprint', ?2) WHERE id = ?3",
                      rusqlite::params![identity["host"].as_str(), identity["key_fingerprint"].as_str(), id]).unwrap();
    let t = ScriptedTransport::from_script(&json!({ "POST /v1/order/new": [{ "network": "post_send", "message": "Net::ReadTimeout" }] }));
    assert!(matches!(placement::send_recorded(&o.primary, &venue(&o, &t), &b, &intent, &FixedClock(t0())).await.unwrap(), Sent::Ambiguous(_)));
    assert!(bot(&o, id).rust_placement().is_some(), "the intent stays");
    let mut found = order("7612345679", false, false, "0.00092821", "0.00092821", "64000.12");
    found["client_order_id"] = json!("$client_order_id");
    let t = ScriptedTransport::from_script(&json!({ "POST /v1/order/status": [ok(found)] }));
    assert!(matches!(placement::recover_since(&o.primary, &venue(&o, &t), &bot(&o, id), &FixedClock(t0() + Duration::seconds(30)), t0()).await.unwrap(), Recovery::Recorded(_)));
    assert_eq!(t.requests()[0].body, Some(json!({ "client_order_id": intent.cl_ord_id })), "looked up by the intent id it was sent with");
    assert_eq!(one::<String>(&o, "SELECT external_id FROM transactions"), "7612345679");
    assert!(bot(&o, id).rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_crossing_venues_market_plan_without_a_crossed_price_is_never_sent() {
    let (_d, o, id) = install(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let b = bot(&o, id);
    let mut plan = crossed_plan(&o, &b);
    plan.wire_price = None;
    let intent = placement::begin(&o.primary, &b, &plan, &FixedClock(t0())).unwrap();
    let t = ScriptedTransport::default();
    assert!(matches!(placement::send_recorded(&o.primary, &venue(&o, &t), &b, &intent, &FixedClock(t0())).await.unwrap(), Sent::NotSent(m) if m.contains("no crossed price")));
    assert!(t.requests().is_empty(), "never sent uncrossed");
}

#[tokio::test(flavor = "current_thread")]
async fn the_exact_cap_guard_bounds_the_spend_at_the_crossed_price() {
    // 0.00092821 at 64640.11 costs 59.99999; at the raw ask it would cost 59.41 and pass a 59.998 cap untouched. At the
    // crossed price the guard trims it to 0.00092818, under a 0.00092819 minimum: the leg's skipped outcome.
    let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(59.998))
        .transient("quote_amount_limit_enabled_at", json!("2026-09-01T09:00:00.000Z"));
    let (_d, o, id) = install(spec);
    o.primary.execute("UPDATE tickers SET minimum_base_size = '0.00092819'", []).unwrap();
    let b = bot(&o, id);
    let err = placement::begin(&o.primary, &b, &crossed_plan(&o, &b), &FixedClock(t0())).unwrap_err();
    assert!(format!("{err:?}").contains("below minimum after exact cap guard"), "{err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_market_sell_keeps_its_base_and_goes_out_at_the_crossed_bid() {
    let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("direction", json!("selling")).with("sell_interval", json!("week"))
        .with("sell_denomination", json!("base")).with("sell_amount", json!(0.01));
    let (_d, o, id) = install(spec);
    let t = ScriptedTransport::from_script(&json!({ "GET /v1/pubticker/btceur": [pubticker("1.231", "1.24", "1.235")], "POST /v1/order/new": [ok(json!({ "order_id": "5" }))],
                                                    "POST /v1/balances": [balances("100000", "0.004")] }));
    assert!(matches!(tick::tick(&o.primary, &venue(&o, &t), id, &FixedClock(t0()), &mut Attempts::default()).await.unwrap(), TickOutcome::Done { placed: true }));
    let sent = placed(&t);
    // Rails' base clamped to the free 0.004; sent at adjust(adjust(1.231) × 0.99) = adjust(1.2177) = 1.21, the price Rails'
    // #set_crossing_order re-reads. The row keeps Rails' raw-bid price and valuation.
    assert_eq!((sent[0]["side"].as_str(), sent[0]["amount"].as_str(), sent[0]["price"].as_str()), (Some("sell"), Some("0.004"), Some("1.21")));
    assert_eq!(reads(&t), 1, "one price read");
    let (price, amount, quote): (f64, f64, f64) = o.primary.query_row("SELECT price, amount, quote_amount FROM transactions", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((price, amount), (1.231, 0.004));
    assert!((quote - 0.004 * 1.231).abs() < 1e-12);
}

#[test]
fn a_base_plan_and_every_venue_without_a_cross_are_untouched() {
    let t = ticker();
    let plan = amount::OrderPlan { ticker: t.clone(), side: Side::Sell, limit: false, price: bd("100"), amount: bd("1"), quote_amount: bd("100"), quote_type: false,
                                   volume: bd("1"), kind: amount::TxKind::Regular, origin: Origin::Base, wire_price: None };
    // No cross: no wire price, a Market order, the base as it is.
    let kraken = plan.clone().with_wire_price(&KRAKEN, &bd("100"), false).unwrap();
    assert!(kraken.wire_price.is_none());
    assert_eq!(kraken.to_order("c".into(), t0(), KRAKEN.wire).unwrap().kind, OrderKind::Market);
    // A Gemini sell of 1 base at bid 100: still 1 base (never 1.0101), at the crossed 99.
    let gemini = plan.clone().with_wire_price(&GEMINI, &bd("100"), false).unwrap();
    let o = gemini.to_order("c".into(), t0(), GEMINI.wire).unwrap();
    assert_eq!((o.kind, o.volume.as_str()), (OrderKind::Crossed { price: "99.0".into() }, "1.0"), "BigDecimal#to_s('F')");
    // A limit plan is never crossed.
    let limit = amount::OrderPlan { limit: true, ..plan }.with_wire_price(&GEMINI, &bd("100"), false).unwrap();
    assert!(limit.wire_price.is_none());
}
