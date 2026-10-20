//! The KuCoin trading adapter (venue::kucoin) over recorded bodies: its signer and posted bytes against honeymaker's own
//! requests, its reads, placement answers mapped as Exchange#ambiguous_placement_error? maps them, order polls, cancel and
//! recovery by client order id (plan K r4: recovery only ever FINDS an order; absence is never concluded). Rails-free.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::placement::{self, OperatorResolution, Recovery, Sent};
use deltabadger::engine::venue_rules::KUCOIN;
use deltabadger::engine::FixedClock;
use deltabadger::ruby::BigDec;
use deltabadger::store::{self, Paths};
use deltabadger::venue::binance::{Pair, Pairs};
use deltabadger::venue::http::{self, HttpRequest, ScriptedTransport};
use deltabadger::venue::kucoin::{self, KucoinVenue, ABSENCE_UNPROVEN, URL};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Side, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn btc_usdt() -> Ticker {
    Ticker { id: 1, ticker: "BTC-USDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
             exchange_name: "KuCoin".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 6, price_decimals: 1,
             minimum_base_size: bd("0.00001"), minimum_quote_size: bd("0.1"), trading_enabled: true, available: true, crypto: true }
}
fn pairs() -> Pairs {
    let mut p = Pairs::default();
    p.by_symbol.insert("BTC-USDT".into(), Pair { base: "BTC".into(), quote: "USDT".into(), price_decimals: 1 });
    for (code, asset) in [("BTC", "BTC"), ("USDT", "USDT"), ("ETH", "ETH")] { p.assets.insert(code.into(), asset.into()); }
    p
}
fn venue(script: Value) -> (ScriptedTransport, KucoinVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), KucoinVenue::new(t, URL).with_pairs(pairs()))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn r200(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn fail(status: u16, body: Value) -> Value { json!([{ "status": status, "body": body }]) }
fn env(data: Value) -> Value { ok(json!({ "code": "200000", "data": data })) }
fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/kucoin_signing.json")).unwrap() }
fn lower(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
    h.sort();
    h
}
fn recorded_headers(v: &Value) -> Vec<(String, String)> {
    lower(&v.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect::<Vec<_>>())
}
fn query_of(r: &HttpRequest) -> Vec<(String, String)> { r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() }
fn pairs_of(v: &[(&str, &str)]) -> Vec<(String, String)> { v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }

// ---- the pins: the fixture is Rails' and honeymaker's, recorded from the files it names

#[test]
fn the_fixture_is_recorded_from_the_current_rails_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (file, recorded) in fixture()["sources"].as_object().unwrap() {
        use sha2::Digest;
        let now = hex::encode(sha2::Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(recorded.as_str().unwrap(), now, "{file} changed: re-record with script/rust/repin");
    }
}

// ---- K1: signing, byte-identical to Honeymaker::Clients::Kucoin#signed_headers under a frozen #timestamp_ms

#[test]
fn every_recorded_honeymaker_signed_request_is_reproduced() {
    let cases = fixture()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 14);
    for c in cases {
        let method: &'static str = match c["method"].as_str().unwrap() { "GET" => "GET", "POST" => "POST", "DELETE" => "DELETE", m => panic!("{m}") };
        let query: Vec<(&'static str, String)> = c["query"].as_str().map(|q| {
            reqwest::Url::parse(&format!("http://x/?{q}")).unwrap().query_pairs().map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned())).collect()
        }).unwrap_or_default();
        let body = c["body"].as_str().map(|b| serde_json::from_str::<Value>(b).unwrap());
        let r = HttpRequest { method, base: URL.into(), path: c["path"].as_str().unwrap().into(), query, body, not_after: None };
        assert_eq!(kucoin::query_text(&r), c["query"].as_str().unwrap_or(""), "Faraday::Utils.build_query: {c}");
        assert_eq!(kucoin::body_text(&r).as_deref(), c["body"].as_str(), "the POST body is the bytes honeymaker sends: {c}");
        let h = kucoin::headers(&r, c["api_key"].as_str().unwrap(), c["api_secret"].as_str().unwrap(), c["passphrase"].as_str().unwrap(), c["timestamp"].as_i64().unwrap());
        assert_eq!(lower(&h), recorded_headers(&c["headers"]), "{c}");
    }
}

#[test]
fn a_public_read_carries_no_key() {
    let r = HttpRequest { method: "GET", base: URL.into(), path: "/api/v1/market/allTickers".into(), query: vec![], body: None, not_after: None };
    let h = lower(&kucoin::headers(&r, "k", "s", "p", 1));
    assert_eq!(h, pairs_of(&[("accept", "application/json"), ("content-type", "application/json")]), "honeymaker's unauthenticated_headers");
}

#[tokio::test(flavor = "current_thread")]
async fn the_transport_signs_and_sends_the_same_bytes() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_string(r#"{"code":"200000","data":{"orderId":"X1"}}"#)).mount(&server).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string(r#"{"code":"200000","data":[]}"#)).mount(&server).await;
    let at: DateTime<Utc> = "2026-09-20T02:30:00.750000Z".parse().unwrap();
    let t = http::ReqwestTransport::kucoin(http::client(), "KEY".into(), "SECRET".into(), "PASS".into()).with_clock(std::sync::Arc::new(FixedClock(at)));
    let v = KucoinVenue::new(t, &server.uri()).with_pairs(pairs());
    let o = order(OrderKind::Limit { price: "64150.5".into() }, false, "0.00093", Side::Buy);
    assert_eq!(v.add_order(&NewOrder { deadline: at + Duration::seconds(10), ..o }).await, Ok("X1".into()));
    assert_eq!(v.balance("USDT", true).await, Ok(BigDec::zero()));
    let got = server.received_requests().await.unwrap();
    assert_eq!(got.len(), 2);
    for g in &got {
        let r = HttpRequest { method: if g.method.as_str() == "POST" { "POST" } else { "GET" }, base: server.uri(), path: g.url.path().into(),
                              query: g.url.query_pairs().map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned())).collect(),
                              body: (!g.body.is_empty()).then(|| serde_json::from_slice(&g.body).unwrap()), not_after: None };
        assert_eq!(g.url.query().unwrap_or(""), kucoin::query_text(&r), "the query sent is the one signed");
        if let Some(b) = kucoin::body_text(&r) { assert_eq!(String::from_utf8(g.body.clone()).unwrap(), b, "the body sent is the one signed"); }
        let want = kucoin::headers(&r, "KEY", "SECRET", "PASS", deltabadger::venue::binance::timestamp_ms(at));
        for (k, v) in want {
            assert_eq!(g.headers.get(k.as_str()).map(|h| h.to_str().unwrap().to_string()), Some(v), "{k} on {} {}", g.method, g.url);
        }
    }
}

// ---- K1: reads (Exchanges::Kucoin#get_ask_price / #get_bid_price / #get_last_price / #get_balances)

fn book(bid: &str, ask: &str) -> Value { env(json!({ "time": 1789871400750u64, "sequence": "1", "bids": [[bid, "1.5"], ["63999", "2"]], "asks": [[ask, "0.3"]] })) }
fn tickers(last: &str) -> Value { env(json!({ "time": 1789871400750u64, "ticker": [{ "symbol": "ETH-USDT", "last": "3000" }, { "symbol": "BTC-USDT", "last": last }] })) }

#[tokio::test(flavor = "current_thread")]
async fn prices_read_the_books_first_level_and_the_tickers_last_and_refuse_a_zero() {
    let (t, v) = venue(json!({ "GET /api/v1/market/orderbook/level2_20": book("64000.1", "64000.2"), "GET /api/v1/market/allTickers": tickers("64000.05") }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Ok(bd("64000.2")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Ok(bd("64000.1")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Ok(bd("64000.05")));
    let keys: Vec<String> = t.requests().iter().map(http::request_key).collect();
    assert_eq!(keys, ["GET /api/v1/market/orderbook/level2_20?symbol=BTC-USDT", "GET /api/v1/market/orderbook/level2_20?symbol=BTC-USDT", "GET /api/v1/market/allTickers"]);
    let (_, v) = venue(json!({ "GET /api/v1/market/orderbook/level2_20": book("0", "0.0"), "GET /api/v1/market/allTickers": tickers("0") }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Wrong ask price for BTC-USDT: 0.0".into()])));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Err(VenueError::Rejected(vec!["Wrong bid price for BTC-USDT: 0.0".into()])));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["Wrong last price for BTC-USDT: 0.0".into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_or_unreadable_price_is_never_a_price() {
    let (_, v) = venue(json!({ "GET /api/v1/market/orderbook/level2_20": ok(json!({ "code": "400100", "msg": "symbol not exists" })),
                               "GET /api/v1/market/allTickers": ok(json!({ "code": "400100", "msg": "x" })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Failed to get KuCoin order book for BTC-USDT".into()])));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["Failed to get KuCoin last price".into()])));
    let (_, v) = venue(json!({ "GET /api/v1/market/allTickers": env(json!({ "ticker": [{ "symbol": "ETH-USDT", "last": "3000" }] })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["Ticker BTC-USDT not found on KuCoin".into()])));
    for body in [env(json!({ "bids": [], "asks": [] })), env(json!({ "bids": [["NaN", "1"]], "asks": [["1", "1"]] })), ok(json!("<html>maintenance</html>"))] {
        let (_, v) = venue(json!({ "GET /api/v1/market/orderbook/level2_20": body.clone() }));
        assert!(matches!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Ambiguous(_))), "{body}");
    }
    let (_, v) = venue(json!({ "GET /api/v1/market/orderbook/level2_20": fail(400, json!({ "code": "400100", "msg": "bad" })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Rejected(vec![r#"{"code":"400100","msg":"bad"}"#.into()])), "honeymaker's failure is the body");
    // allTickers is the whole market: its read has a stated limit, and a body over it is never read as a price.
    let huge = format!(r#"{{"code":"200000","data":{{"ticker":[{{"symbol":"BTC-USDT","last":"1","pad":"{}"}}]}}}}"#, "x".repeat(kucoin::ALL_TICKERS_LIMIT));
    let (_, v) = venue(json!({ "GET /api/v1/market/allTickers": ok(json!(huge)) }));
    assert!(v.price(&btc_usdt(), PriceSide::Last).await.is_err());
}

fn accounts(rows: Value) -> Value { env(rows) }

#[tokio::test(flavor = "current_thread")]
async fn the_balance_is_the_trade_accounts_available_amount() {
    let rows = json!([{ "id": "a", "currency": "BTC", "type": "trade", "balance": "0.6", "available": "0.5", "holds": "0.1" },
                      { "id": "b", "currency": "USDT", "type": "trade", "balance": "1300", "available": "1234.5", "locked": "65.5" }]);
    let (t, v) = venue(json!({ "GET /api/v1/accounts": accounts(rows) }));
    assert_eq!(v.balance("USDT", true).await, Ok(bd("1234.5")));
    assert_eq!(query_of(&t.requests()[0]), pairs_of(&[("type", "trade")]));
    assert_eq!(v.balance("EUR", true).await, Ok(BigDec::zero()), "a code no ticker maps is no balance");
    assert_eq!(v.free_base_balance(&btc_usdt()).await, Ok(bd("0.5")), "available, never balance");
    let no_catalog = || -> Result<deltabadger::sync::balances::Catalog, VenueError> { panic!("KuCoin names assets by code") };
    assert_eq!(v.settled_quote_balance(&btc_usdt(), no_catalog).await, Ok(bd("1234.5")));
    let (_, v) = venue(json!({ "GET /api/v1/accounts": accounts(json!([{ "currency": "USDT", "available": "12", "holds": "0" }])) }));
    assert_eq!(v.free_base_balance(&btc_usdt()).await, Ok(BigDec::zero()), "absent is no holdings");
    for row in [json!({ "currency": "USDT", "available": "garbage", "holds": "0" }), json!({ "currency": "USDT", "available": "1" })] {
        let (_, v) = venue(json!({ "GET /api/v1/accounts": accounts(json!([row.clone()])) }));
        assert!(matches!(v.balance("USDT", true).await, Err(VenueError::Ambiguous(_))), "unreadable is never 0: {row}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn only_the_success_code_means_balances() {
    let (_, v) = venue(json!({ "GET /api/v1/accounts": ok(json!({ "code": "400003", "msg": "KC-API-KEY not exists", "data": [] })) }));
    let Err(VenueError::Rejected(e)) = v.balance("USDT", true).await else { panic!() };
    assert_eq!(e, ["Failed to get KuCoin balances: 400003 KC-API-KEY not exists"]);
    assert_eq!(KUCOIN.failure_kind(&e), Some("invalid_key"), "the venue's reason reaches the key check");
    let (_, v) = venue(json!({ "GET /api/v1/accounts": ok(json!({ "code": "200000" })) }));
    assert_eq!(v.balance("USDT", true).await, Err(VenueError::Rejected(vec!["Failed to get KuCoin balances: 200000".into()])));
    let (_, v) = venue(json!({ "GET /api/v1/accounts": fail(429, json!({ "code": "429000", "msg": "Too Many Requests" })) }));
    let Err(VenueError::Rejected(e)) = v.balance("USDT", true).await else { panic!() };
    assert!(KUCOIN.is_throttle(&e), "{e:?}");
}

// ---- K1: the rules table, pinned against Rails' lists

#[test]
fn the_rules_are_exchanges_kucoins() {
    let f = fixture();
    let rails = &f["errors"];
    let list = |v: &Value| v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect::<Vec<_>>();
    for (kind, patterns) in KUCOIN.kinds {
        let want = if rails.get(kind).is_some() { list(&rails[kind]) } else { vec![] };
        assert_eq!(patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(), want, "{kind}");
    }
    let mut transient = list(&f["network_transient"]);
    transient.extend(list(&rails["transient"]));
    assert_eq!(KUCOIN.transient_text.iter().map(|s| s.to_string()).collect::<Vec<_>>(), transient, "Exchange#transient_error?");
    // r5: every transient answer Rails' #ambiguous_placement_error? reads as unknown is unknown here, the placement-safe ones excepted.
    let safe = list(&rails["placement_safe_transient"]);
    for m in list(&rails["transient"]) {
        assert_eq!(KUCOIN.add_outcome_unknown(std::slice::from_ref(&m)), !safe.contains(&m), "{m}");
    }
    for m in ["KuCoin API error 500000: Internal Server Error", "KuCoin API error 230005: The system is busy, please try again later.",
              "KuCoin API error 500000: anything", "KuCoin API error 230005"] {
        assert!(KUCOIN.add_outcome_unknown(&[m.into()]), "{m}");
    }
    for m in safe.iter().chain(list(&f["placement_safe"]).iter()) { assert!(placement::safe_rejection(std::slice::from_ref(m)), "{m}"); }
    assert_eq!((KUCOIN.minimum_logic.market, KUCOIN.minimum_logic.limit),
               (deltabadger::engine::venue_rules::AmountLogic::BaseOrQuote, deltabadger::engine::venue_rules::AmountLogic::BaseAndQuoteInBase));
    assert!(KUCOIN.reports_missing && KUCOIN.follow_up_strict && !KUCOIN.deadline_sent && !KUCOIN.absence_provable);
}

// ---- K2: placement (Exchanges::Kucoin#set_market_order / #set_limit_order through Clients::Kucoin#place_order)

const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str, side: Side) -> NewOrder {
    NewOrder { pair: "BTC-USDT".into(), side, kind, volume: volume.into(), quote_volume: quote, cl_ord_id: CL.into(),
               deadline: "2026-09-01T10:00:10Z".parse().unwrap(), day: false }
}
fn accepted() -> Value { env(json!({ "orderId": "6716f0a2c1b6d10007a3b1c5" })) }

#[tokio::test(flavor = "current_thread")]
async fn each_order_posts_the_bytes_rails_posts_with_the_intent_id_as_client_oid() {
    let f = fixture();
    let key = f["api_key"].as_array().unwrap().clone();
    let orders = f["orders"].as_array().unwrap().clone();
    assert_eq!(orders.len(), 8);
    for o in orders {
        let rails: Value = serde_json::from_str(o["body"].as_str().unwrap()).unwrap();
        let side = if o["side"] == "sell" { Side::Sell } else { Side::Buy };
        let new = match rails["type"].as_str().unwrap() {
            "market" if rails.get("funds").is_some() => order(OrderKind::Market, true, rails["funds"].as_str().unwrap(), side),
            "market" => order(OrderKind::Market, false, rails["size"].as_str().unwrap(), side),
            _ => order(OrderKind::Limit { price: rails["price"].as_str().unwrap().into() }, false, rails["size"].as_str().unwrap(), side),
        };
        let (t, v) = venue(json!({ "POST /api/v1/orders": accepted() }));
        assert_eq!(v.add_order(&new).await, Ok("6716f0a2c1b6d10007a3b1c5".into()), "the stored id is the raw orderId");
        let r = &t.requests()[0];
        assert_eq!((r.method, r.path.as_str(), r.query.is_empty(), r.not_after.is_some()), ("POST", "/api/v1/orders", true, true), "{o}");
        assert_eq!(kucoin::body_text(r).as_deref(), o["body"].as_str(), "{o}");
        let h = kucoin::headers(r, key[0].as_str().unwrap(), key[1].as_str().unwrap(), key[2].as_str().unwrap(), o["timestamp"].as_i64().unwrap());
        assert_eq!(lower(&h), recorded_headers(&o["headers"]), "signed over exactly those bytes: {o}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_to_rails_ambiguity_rules() {
    let unknown = |e: &VenueError| matches!(e, VenueError::Rejected(m) if KUCOIN.add_outcome_unknown(m));
    let rejected = |m: &str| Err::<String, _>(VenueError::Rejected(vec![m.into()]));
    let no_id = Err::<String, _>(VenueError::Ambiguous(kucoin::NO_ORDER_ID.into()));
    let unreadable = Err::<String, _>(VenueError::Ambiguous(kucoin::UNREADABLE.into()));
    let ts = r#"{"code":"400002","msg":"Invalid KC-API-TIMESTAMP"}"#;
    // (reply, the venue's answer, whether placement reads it as unknown (r5), whether it is placement-safe)
    let cases: Vec<(Value, Result<String, VenueError>, bool, bool)> = vec![
        (accepted(), Ok("6716f0a2c1b6d10007a3b1c5".into()), false, false),
        // 200000 without an id took the order, or may have (R0b): never a placed order with no id.
        (env(json!({})), no_id.clone(), false, false),
        (env(Value::Null), no_id.clone(), false, false),
        (env(json!({ "orderId": "" })), no_id.clone(), false, false),
        (env(json!({ "orderId": 5 })), no_id, false, false),
        // An HTTP-200 envelope with a refusal: honeymaker's api_error text, definitive...
        (ok(json!({ "code": "200004", "msg": "Balance insufficient!" })), rejected("KuCoin API error 200004: Balance insufficient!"), false, false),
        (ok(json!({ "code": "400100", "msg": "Order size below the minimum requirement." })), rejected("KuCoin API error 400100: Order size below the minimum requirement."), false, false),
        // ...except the venue's own outage and busy answers (r5), whatever their status.
        (ok(json!({ "code": "500000", "msg": "Internal Server Error" })), rejected("KuCoin API error 500000: Internal Server Error"), true, false),
        (ok(json!({ "code": "230005", "msg": "The system is busy, please try again later." })), rejected("KuCoin API error 230005: The system is busy, please try again later."), true, false),
        (ok(json!({ "code": "500000", "msg": "Something else" })), rejected("KuCoin API error 500000: Something else"), true, false),
        // 400002 is a pre-trade rejection: safe, in an envelope, a 4xx or a 5xx.
        (ok(json!({ "code": "400002", "msg": "Invalid KC-API-TIMESTAMP" })), rejected("KuCoin API error 400002: Invalid KC-API-TIMESTAMP"), false, true),
        (fail(400, json!(ts)), rejected(ts), false, true),
        (fail(503, json!(ts)), rejected(ts), false, true),
        // A 429 is definitive (§2.7).
        (fail(429, json!({ "code": "429000", "msg": "Too Many Requests" })), rejected(r#"{"code":"429000","msg":"Too Many Requests"}"#), false, false),
        (fail(400, json!({ "code": "400100", "msg": "bad" })), rejected(r#"{"code":"400100","msg":"bad"}"#), false, false),
        (fail(400, json!({ "code": "500000", "msg": "Internal Server Error" })), rejected(r#"{"code":"500000","msg":"Internal Server Error"}"#), true, false),
        // May have been placed: a 5xx, an unreadable 2xx, a lost reply.
        (fail(500, json!({ "code": "500000", "msg": "Internal Server Error" })), Err(VenueError::Ambiguous(r#"{"code":"500000","msg":"Internal Server Error"}"#.into())), false, false),
        (fail(502, json!("<html>bad gateway</html>")), Err(VenueError::Ambiguous("<html>bad gateway</html>".into())), false, false),
        (ok(json!("<html>maintenance</html>")), unreadable.clone(), false, false),
        (ok(json!({ "data": { "orderId": "x" } })), unreadable, false, false),
        (json!([{ "network": "post_send", "message": "read timeout" }]), Err(VenueError::Ambiguous("read timeout".into())), false, false),
        // Provably never sent: definitive.
        (json!([{ "network": "pre_send", "message": "refused" }]), rejected("refused"), false, false),
        (json!([{ "network": "permanent", "message": "certificate" }]), rejected("certificate"), false, false),
    ];
    for (reply, want, is_unknown, safe) in cases {
        let (_, v) = venue(json!({ "POST /api/v1/orders": reply }));
        let got = v.add_order(&order(OrderKind::Market, true, "60", Side::Buy)).await;
        assert_eq!(got, want, "{reply}");
        if let Err(e) = &got {
            assert_eq!(unknown(e), is_unknown, "{reply}");
            assert_eq!(matches!(e, VenueError::Rejected(m) if placement::safe_rejection(m)), safe, "{reply}");
        }
    }
}

// ---- K2 in the placement protocol: 400002, a refusal, and r5's unknown envelopes

fn t0() -> DateTime<Utc> { "2026-09-30T12:00:00Z".parse().unwrap() }
fn setup() -> (tempfile::TempDir, store::Opened, model::Bot, amount::OrderPlan) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute_batch("UPDATE exchanges SET type = 'Exchanges::Kucoin', name = 'KuCoin'; UPDATE tickers SET ticker = 'BTC-EUR', base = 'BTC'").unwrap();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), KUCOIN.minimum_logic).unwrap() else { panic!() };
    (dir, o, bot, plan)
}
fn count(o: &store::Opened, sql: &str) -> i64 { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
fn reload(o: &store::Opened, b: &model::Bot) -> model::Bot { model::load_bot(&o.primary, b.id).unwrap() }
fn kv(t: &ScriptedTransport) -> KucoinVenue<ScriptedTransport> {
    let creds = deltabadger::crypto::Credentials { key: "KEY-A".into(), secret: "s".into(), passphrase: Some("p".into()), redaction_values: vec![] };
    KucoinVenue::new(t.clone(), URL).with_identity(&creds)
}
fn posts(t: &ScriptedTransport) -> usize { t.requests().iter().filter(|r| r.method == "POST").count() }

#[tokio::test(flavor = "current_thread")]
async fn a_400002_writes_no_row_and_a_refusal_writes_a_failed_row() {
    for (reply, rows) in [(json!({ "code": "400002", "msg": "Invalid KC-API-TIMESTAMP" }), 0), (json!({ "code": "200004", "msg": "Balance insufficient!" }), 1)] {
        let (_d, o, bot, plan) = setup();
        let t = ScriptedTransport::default();
        t.reply("POST /api/v1/orders", 200, reply.clone());
        let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
        assert_eq!(intent.cl_ord_id.len(), 36, "the intent id is the clientOid (≤ 40 characters)");
        assert!(matches!(placement::send_recorded(&o.primary, &kv(&t), &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Rejected(_)), "{reply}");
        assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE status = 1"), rows, "{reply}");
        assert!(reload(&o, &bot).rust_placement().is_none(), "{reply}: a definitive answer clears the intent");
        let body: Value = serde_json::from_str(&kucoin::body_text(&t.requests()[0]).unwrap()).unwrap();
        assert_eq!((body["clientOid"].as_str(), body["symbol"].as_str(), body["type"].as_str()), (Some(intent.cl_ord_id.as_str()), Some("BTC-EUR"), Some("market")));
    }
}

/// An intent whose placement reply was lost (or answered 500000 / 230005): the ambiguous send, recorded as the tick records it.
async fn ambiguous_send(reply: Value) -> (tempfile::TempDir, store::Opened, model::Bot, placement::Intent) {
    let (d, o, bot, plan) = setup();
    let t = ScriptedTransport::from_script(&json!({ "POST /api/v1/orders": [reply] }));
    let id = kv(&t).placement_identity().unwrap();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0())).unwrap();
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.host', ?1, '$.rust_placement.key_fingerprint', ?2) WHERE id = ?3",
                      rusqlite::params![id["host"].as_str(), id["key_fingerprint"].as_str(), bot.id]).unwrap();
    assert!(matches!(placement::send_recorded(&o.primary, &kv(&t), &bot, &intent, &FixedClock(t0())).await.unwrap(), Sent::Ambiguous(_)), "{reply}");
    assert!(reload(&o, &bot).rust_placement().is_some());
    assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 0);
    (d, o, bot, intent)
}

fn raw_order(id: &str, cl: &str, active: bool, cancel: bool, deal_size: &str, deal_funds: &str) -> Value {
    json!({ "id": id, "symbol": "BTC-EUR", "opType": "DEAL", "type": "market", "side": "buy", "price": "0", "size": "0", "funds": "60",
            "dealFunds": deal_funds, "dealSize": deal_size, "fee": "0.06", "feeCurrency": "EUR", "clientOid": cl,
            "isActive": active, "cancelExist": cancel, "createdAt": 1790000000000u64 })
}
fn not_found() -> Value { json!({ "status": 404, "body": { "code": "400100", "msg": "order not exist." } }) }
fn done_page(items: Value) -> Value { r200(json!({ "code": "200000", "data": { "currentPage": 1, "pageSize": 500, "totalNum": 1, "totalPage": 1, "items": items } })) }

#[tokio::test(flavor = "current_thread")]
async fn r5_unknown_envelopes_keep_the_intent_across_ticks_and_a_restart_and_never_place_twice() {
    for reply in [r200(json!({ "code": "500000", "msg": "Internal Server Error" })), r200(json!({ "code": "230005", "msg": "The system is busy, please try again later." }))] {
        let (_d, o, bot, intent) = ambiguous_send(reply.clone()).await;
        let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/order/client-order/".to_string() + &intent.cl_ord_id: [not_found()],
                                                        "GET /api/v1/orders": [done_page(json!([]))], "POST /api/v1/orders": [r200(json!({ "code": "200000", "data": { "orderId": "SECOND" } }))] }));
        for (days, start) in [(1, t0()), (30, t0()), (31, t0() + Duration::days(31))] {
            let at = FixedClock(t0() + Duration::days(days));
            assert!(matches!(placement::recover_since(&o.primary, &kv(&t), &reload(&o, &bot), &at, start).await.unwrap(), Recovery::Pending), "{reply} +{days}d");
            assert!(matches!(deltabadger::engine::tick::tick(&o.primary, &kv(&t), bot.id, &at, &mut Default::default()).await.unwrap(),
                             deltabadger::engine::tick::TickOutcome::AwaitingReconciliation), "the bot halts on its unresolved order");
        }
        assert_eq!(posts(&t), 0, "nothing is placed while the order is unresolved");
        assert!(reload(&o, &bot).rust_placement().is_some(), "{reply}: the 1,200 s clearing never applies to KuCoin");
        assert_eq!(count(&o, "SELECT count(*) FROM transactions"), 0);
    }
}

// ---- K3: polls (Exchanges::Kucoin#get_order / #get_orders / #parse_order_data)

#[tokio::test(flavor = "current_thread")]
async fn every_documented_status_combination_maps_as_parse_order_status() {
    use OrderStatus::*;
    // (cancelExist, isActive, dealSize) → status; null and absent read as false, as Ruby's truthiness does.
    let cases = [
        (json!(true), json!(true), "0", Cancelled), (json!(true), json!(false), "0", Cancelled), (json!(true), json!(false), "0.001", Cancelled),
        (json!(true), json!(true), "0.001", Cancelled), (json!(true), Value::Null, "0", Cancelled),
        (json!(false), json!(true), "0", Open), (json!(false), json!(true), "0.0005", Open), (Value::Null, json!(true), "0", Open),
        (json!(false), json!(false), "0.001", Closed), (Value::Null, Value::Null, "0.001", Closed),
        (json!(false), json!(false), "0", Unknown), (json!(false), json!(false), "0.000", Unknown), (Value::Null, Value::Null, "0", Unknown),
    ];
    for (cancel, active, deal, want) in cases {
        let mut raw = raw_order("O1", "x", false, false, deal, if bd(deal).is_zero() { "0" } else { "64.05" });
        raw["cancelExist"] = cancel.clone();
        raw["isActive"] = active.clone();
        raw["symbol"] = json!("BTC-USDT");
        let (t, v) = venue(json!({ "GET /api/v1/orders/O1": env(raw.clone()) }));
        let got = v.orders(&["O1".into()]).await.unwrap();
        assert_eq!((got.len(), got[0].status, got[0].txid.as_str()), (1, want, "O1"), "{raw}");
        assert_eq!((t.requests()[0].method, t.requests()[0].path.as_str(), t.requests()[0].query.is_empty()), ("GET", "/api/v1/orders/O1", true));
    }
    let (_, v) = venue(json!({ "GET /api/v1/orders/O1": env(json!(null)) }));
    assert_eq!(v.orders(&["O1".into()]).await, Ok(vec![]), "honeymaker's \"Order not found\": missing");
}

#[tokio::test(flavor = "current_thread")]
async fn an_orders_figures_are_read_as_parse_order_data_reads_them() {
    // A partial fill that was cancelled keeps its executed amounts; the price is dealFunds / dealSize at the tick, half up.
    let mut raw = raw_order("O1", "x", false, true, "0.001", "64.05");
    raw["symbol"] = json!("BTC-USDT");
    raw["type"] = json!("limit");
    raw["side"] = json!("SELL");
    raw["price"] = json!("64100");
    raw["size"] = json!("0.002");
    let (_, v) = venue(json!({ "GET /api/v1/orders/O1": env(raw.clone()) }));
    let got = v.orders(&["O1".into()]).await.unwrap().remove(0);
    assert_eq!((got.status, got.price.clone(), got.amount.clone(), got.quote_amount.clone(), got.amount_exec.clone(), got.quote_amount_exec.clone(), got.limit, got.sell, got.pair.as_deref()),
               (OrderStatus::Cancelled, Some(bd("64050")), Some(bd("0.002")), Some(bd("60")), bd("0.001"), Some(bd("64.05")), true, true, Some("BTC-USDT")));
    // Nothing dealt: the order's own price; a zero price is none.
    let mut open = raw.clone();
    open["dealSize"] = json!("0");
    open["dealFunds"] = json!("0");
    open["cancelExist"] = json!(false);
    open["isActive"] = json!(true);
    let (_, v) = venue(json!({ "GET /api/v1/orders/O1": env(open.clone()) }));
    assert_eq!(v.orders(&["O1".into()]).await.unwrap()[0].price, Some(bd("64100")));
    open["price"] = json!("0");
    let (_, v) = venue(json!({ "GET /api/v1/orders/O1": env(open.clone()) }));
    assert_eq!(v.orders(&["O1".into()]).await.unwrap()[0].price, None);
    open["type"] = json!("stop");
    let (_, v) = venue(json!({ "GET /api/v1/orders/O1": env(open.clone()) }));
    assert_eq!(v.orders(&["O1".into()]).await, Err(VenueError::Rejected(vec!["Unknown KuCoin order type: stop".into()])));
    open["type"] = json!("limit");
    open["dealSize"] = json!("garbage");
    let (_, v) = venue(json!({ "GET /api/v1/orders/O1": env(open) }));
    assert!(matches!(v.orders(&["O1".into()]).await, Err(VenueError::Ambiguous(_))), "an unreadable amount is never a zero");
}

#[tokio::test(flavor = "current_thread")]
async fn a_forgotten_order_is_missing_by_its_message_only() {
    for msg in ["The order does not exist.", "order not exist.", "order_not_exist", "orderNotExist", "Order not found"] {
        let (_, v) = venue(json!({ "GET /api/v1/orders/GONE": fail(404, json!({ "code": "400100", "msg": msg })), "GET /api/v1/orders/O1": env(raw_order("O1", "x", true, false, "0", "0")) }));
        let got = v.orders(&["GONE".into(), "O1".into()]).await.unwrap();
        assert_eq!(got.iter().map(|o| o.txid.as_str()).collect::<Vec<_>>(), ["O1"], "{msg}: missing, the batch still answers");
        let (_, v) = venue(json!({ "GET /api/v1/orders/GONE": ok(json!({ "code": "400100", "msg": msg })) }));
        assert_eq!(v.orders(&["GONE".into()]).await, Ok(vec![]), "{msg} in an HTTP-200 envelope");
    }
    // Never by code: 400100 and 404000 with another message fail the batch, which is retried.
    for reply in [fail(400, json!({ "code": "400100", "msg": "Parameter error" })), fail(404, json!({ "code": "404000", "msg": "Url Not Found" }))] {
        let (_, v) = venue(json!({ "GET /api/v1/orders/GONE": reply.clone() }));
        assert!(matches!(v.orders(&["GONE".into()]).await, Err(VenueError::Rejected(_))), "{reply}");
    }
    // The follow-up asks for one order: a forgotten one is an error, so the row waits for the sweep's fill check.
    let (_, v) = venue(json!({ "GET /api/v1/orders/GONE": fail(404, json!({ "code": "400100", "msg": "order not exist." })) }));
    assert_eq!(v.order_identified("GONE", |_, _, _| Ok(true)).await, Err(VenueError::Rejected(vec!["KuCoin no longer knows order GONE".into()])));
}

fn fill(trade: &str, order: &str, size: &str, funds: &str) -> Value {
    json!({ "symbol": "BTC-USDT", "tradeId": trade, "orderId": order, "side": "buy", "type": "market", "price": "64000", "size": size, "funds": funds,
            "fee": "0.064", "feeCurrency": "USDT", "createdAt": 1790000000000u64 })
}
fn fills_page(items: Value, page: u32, pages: u32, total: u32) -> Value {
    r200(json!({ "code": "200000", "data": { "currentPage": page, "pageSize": 500, "totalNum": total, "totalPage": pages, "items": items } }))
}

#[tokio::test(flavor = "current_thread")]
async fn a_forgotten_order_is_settled_from_its_fills_never_written_off() {
    // R0c, as Exchanges::Kucoin#order_from_fills: weekly windows from `since` to now, every page, each fill once by tradeId.
    let since = Utc::now() - Duration::days(10);
    let (t, v) = venue(json!({ "GET /api/v1/fills": [fills_page(json!([fill("1", "GONE", "0.0006", "38.4")]), 1, 2, 2), fills_page(json!([fill("2", "GONE", "0.0004", "25.62"), fill("9", "OTHER", "1", "1")]), 2, 2, 2),
                                                     fills_page(json!([fill("2", "GONE", "0.0004", "25.62")]), 1, 1, 1)] }));
    let got = v.fills_from_trades(&["GONE".into()], since).await.unwrap();
    assert_eq!(got.len(), 1);
    let s = &got[0];
    assert_eq!((s.txid.as_str(), s.status, s.amount_exec.clone(), s.quote_amount_exec.clone(), s.price.clone(), s.amount.clone(), s.sell, s.limit),
               ("GONE", OrderStatus::Closed, bd("0.001"), Some(bd("64.02")), Some(bd("64020")), None, false, false));
    let asked: Vec<Vec<(String, String)>> = t.requests().iter().map(query_of).collect();
    assert_eq!(asked.len(), 3, "two pages of the first week, one of the rest");
    let start = since.timestamp() * 1000;
    let week = start + 7 * 86_400_000;
    assert_eq!(asked[0], pairs_of(&[("currentPage", "1"), ("endAt", &week.to_string()), ("orderId", "GONE"), ("pageSize", "500"), ("startAt", &start.to_string())]));
    assert_eq!(asked[1][0], ("currentPage".into(), "2".into()));
    assert_eq!(asked[2].iter().find(|(k, _)| k == "startAt").unwrap().1, week.to_string());
    // No fill: nothing recovered, and the stale-order rule decides as in Rails.
    let (_, v) = venue(json!({ "GET /api/v1/fills": [fills_page(json!([]), 1, 0, 0)] }));
    assert_eq!(v.fills_from_trades(&["GONE".into()], since).await, Ok(vec![]));
    // A page that failed, an error envelope, or a range over KuCoin's cap: never "no fill".
    for reply in [fail(500, json!("oops")), ok(json!({ "code": "400003", "msg": "KC-API-KEY not exists" })), json!([fills_page(json!([]), 1, 101, 50_001)])] {
        let (_, v) = venue(json!({ "GET /api/v1/fills": reply.clone() }));
        assert!(v.fills_from_trades(&["GONE".into()], since).await.is_err(), "{reply}");
    }
}

// ---- K3: cancel (Exchanges::Kucoin#cancel_order: DELETE, the path signed alone)

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_deletes_the_order_by_its_id() {
    let (t, v) = venue(json!({ "DELETE /api/v1/orders/O1": env(json!({ "cancelledOrderIds": ["O1"] })) }));
    assert_eq!(v.cancel("O1").await, Ok(()));
    let r = &t.requests()[0];
    assert_eq!((r.method, r.path.as_str(), r.query.is_empty(), r.body.is_none()), ("DELETE", "/api/v1/orders/O1", true, true));
    let (_, v) = venue(json!({ "DELETE /api/v1/orders/O1": fail(400, json!({ "code": "400100", "msg": "order_not_exist_or_not_allow_to_cancel" })) }));
    assert_eq!(v.cancel("O1").await, Err(VenueError::Rejected(vec![r#"{"code":"400100","msg":"order_not_exist_or_not_allow_to_cancel"}"#.into()])));
    let (_, v) = venue(json!({ "DELETE /api/v1/orders/O1": ok(json!({ "code": "400100", "msg": "order_not_exist_or_not_allow_to_cancel" })) }));
    assert_eq!(v.cancel("O1").await, Err(VenueError::Rejected(vec!["KuCoin API error 400100: order_not_exist_or_not_allow_to_cancel".into()])),
               "an error envelope is no cancel");
}

// ---- K3: recovery by client order id (r4): the exact lookup, then the done-orders scan; neither ever proves absence

#[tokio::test(flavor = "current_thread")]
async fn a_lookup_finds_an_open_order_by_its_client_oid_and_never_concludes_absence() {
    let since: DateTime<Utc> = Utc::now() - Duration::hours(2);
    let (t, v) = venue(json!({ format!("GET /api/v1/order/client-order/{CL}"): env(raw_order("O1", CL, true, false, "0", "0")) }));
    let got = v.order_by_client_id(CL, "BTC-EUR", since).await.unwrap().unwrap();
    assert_eq!((got.txid.as_str(), got.status), ("O1", OrderStatus::Open));
    assert_eq!(t.requests().len(), 1, "an exact hit is authoritative");
    // Another client id in the answer is not this order.
    let (_, v) = venue(json!({ format!("GET /api/v1/order/client-order/{CL}"): env(raw_order("O1", "other", true, false, "0", "0")), "GET /api/v1/orders": [done_page(json!([]))] }));
    assert_eq!(v.order_by_client_id(CL, "BTC-EUR", since).await, Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())));
    // Neither finds it: never Ok(None).
    for exact in [not_found(), r200(json!({ "code": "200000", "data": null })), json!({ "network": "post_send", "message": "read timeout" })] {
        let (t, v) = venue(json!({ format!("GET /api/v1/order/client-order/{CL}"): [exact.clone()], "GET /api/v1/orders": [done_page(json!([raw_order("O9", "other", false, false, "1", "1")]))] }));
        assert_eq!(v.order_by_client_id(CL, "BTC-EUR", since).await, Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())), "{exact}");
        let scan = t.requests().into_iter().find(|r| r.path == "/api/v1/orders").expect("the done scan runs");
        let q = query_of(&scan);
        for (k, want) in [("status", "done"), ("symbol", "BTC-EUR"), ("currentPage", "1"), ("pageSize", "500")] {
            assert!(q.iter().any(|(qk, qv)| qk == k && qv == want), "{k}: {q:?}");
        }
        assert!(q.iter().any(|(k, v)| k == "startAt" && *v == (since.timestamp_millis()).to_string()), "{q:?}");
    }
    // A failed done page: an error, never absence.
    let (_, v) = venue(json!({ format!("GET /api/v1/order/client-order/{CL}"): [not_found()], "GET /api/v1/orders": fail(500, json!("oops")) }));
    assert!(v.order_by_client_id(CL, "BTC-EUR", since).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn the_done_scan_finds_a_filled_order_whose_reply_was_lost() {
    let since: DateTime<Utc> = Utc::now() - Duration::days(9);
    let page2 = r200(json!({ "code": "200000", "data": { "currentPage": 2, "pageSize": 500, "totalNum": 2, "totalPage": 2, "items": [raw_order("O7", CL, false, false, "0.0012", "60")] } }));
    let page1 = r200(json!({ "code": "200000", "data": { "currentPage": 1, "pageSize": 500, "totalNum": 2, "totalPage": 2, "items": [raw_order("O6", "other", false, false, "1", "1")] } }));
    let (t, v) = venue(json!({ format!("GET /api/v1/order/client-order/{CL}"): [not_found()], "GET /api/v1/orders": [done_page(json!([])), page1, page2] }));
    let got = v.order_by_client_id(CL, "BTC-EUR", since).await.unwrap().unwrap();
    assert_eq!((got.txid.as_str(), got.status, got.amount_exec.clone()), ("O7", OrderStatus::Closed, bd("0.0012")));
    let windows: Vec<String> = t.requests().iter().filter(|r| r.path == "/api/v1/orders").map(|r| query_of(r).into_iter().find(|(k, _)| k == "startAt").unwrap().1).collect();
    let start = since.timestamp_millis();
    assert_eq!(windows, [start.to_string(), (start + 7 * 86_400_000).to_string(), (start + 7 * 86_400_000).to_string()], "weekly windows, each paged to its end");
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_is_recorded_once_from_the_done_scan_or_the_exact_lookup() {
    for found_by_scan in [true, false] {
        let (_d, o, bot, intent) = ambiguous_send(json!({ "network": "post_send", "message": "read timeout" })).await;
        let cl = intent.cl_ord_id.clone();
        let filled = raw_order("O7", &cl, false, false, "0.0012", "60");
        let script = if found_by_scan {
            json!({ format!("GET /api/v1/order/client-order/{cl}"): [not_found()], "GET /api/v1/orders": [done_page(json!([filled]))] })
        } else {
            json!({ format!("GET /api/v1/order/client-order/{cl}"): env(raw_order("O7", &cl, true, false, "0", "0")) })
        };
        let t = ScriptedTransport::from_script(&script);
        let later = FixedClock(Utc::now());
        assert!(matches!(placement::recover(&o.primary, &kv(&t), &reload(&o, &bot), &later).await.unwrap(), Recovery::Recorded(_)), "scan {found_by_scan}");
        let (ext, status): (String, i64) = o.primary.query_row("SELECT external_id, external_status FROM transactions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((ext.as_str(), status), ("O7", if found_by_scan { 2 } else { 1 }), "closed with its fill, or open");
        assert!(matches!(placement::recover(&o.primary, &kv(&t), &reload(&o, &bot), &later).await.unwrap(), Recovery::NoIntent));
        assert_eq!((posts(&t), count(&o, "SELECT count(*) FROM transactions")), (0, 1), "never a second order or row");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_order_neither_lookup_finds_halts_the_bot_until_the_operator_resolves_it() {
    for resolution in [OperatorResolution::NotPlaced, OperatorResolution::Placed("6716f0a2c1b6d10007a3b1c5".into())] {
        let (_d, o, bot, intent) = ambiguous_send(json!({ "network": "post_send", "message": "read timeout" })).await;
        let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/order/client-order/{}", intent.cl_ord_id): [not_found()],
                                                        "GET /api/v1/orders": [done_page(json!([]))] }));
        for (hours, start) in [(1, t0()), (48, t0()), (24 * 40, t0() + Duration::days(39))] {
            let at = FixedClock(t0() + Duration::hours(hours));
            assert!(matches!(placement::recover_since(&o.primary, &kv(&t), &reload(&o, &bot), &at, start).await.unwrap(), Recovery::Pending), "+{hours}h");
            assert!(matches!(deltabadger::engine::tick::tick(&o.primary, &kv(&t), bot.id, &at, &mut Default::default()).await.unwrap(),
                             deltabadger::engine::tick::TickOutcome::AwaitingReconciliation));
        }
        assert_eq!(posts(&t), 0);
        assert!(reload(&o, &bot).rust_placement().is_some(), "kept across ticks and a restart");
        let placed = matches!(resolution, OperatorResolution::Placed(_));
        placement::resolve_by_operator(&o.primary, bot.id, resolution, t0() + Duration::days(41)).unwrap();
        assert!(reload(&o, &bot).rust_placement().is_none());
        assert_eq!(count(&o, "SELECT count(*) FROM transactions WHERE external_id = '6716f0a2c1b6d10007a3b1c5'"), i64::from(placed));
        assert_eq!(count(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'placement_ambiguous'"), 1, "one record of the operator's decision");
    }
}

// ---- K5: #parse_order_data, recorded (script/rust/record_venue_vectors.rb kucoin)

#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let v: Value = serde_json::from_str(include_str!("fixtures/kucoin_vectors.json")).unwrap();
    let mut p = Pairs::default();
    p.by_symbol.insert("BTC-USDT".into(), Pair { base: "BTC".into(), quote: "USDT".into(), price_decimals: 1 });
    p.by_symbol.insert("ETH-BTC".into(), Pair { base: "ETH".into(), quote: "BTC".into(), price_decimals: 6 });
    let cases = v["kucoin_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 10_000);
    let s = |o: &Option<BigDec>| o.as_ref().map(BigDec::to_s_f);
    for c in cases {
        let got = kucoin::parse_order(c["raw"]["id"].as_str().unwrap(), &c["raw"], &p).unwrap();
        let status = match got.status { OrderStatus::Open => "open", OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", OrderStatus::Unknown => "unknown", OrderStatus::Failed => "failed" };
        assert_eq!(status, c["status"], "{c}");
        assert_eq!(s(&got.price).as_deref(), c["price"].as_str(), "price {c}");
        assert_eq!((s(&got.amount).as_deref(), s(&got.quote_amount).as_deref()), (c["amount"].as_str(), c["quote_amount"].as_str()), "{c}");
        assert_eq!((got.amount_exec.to_s_f(), s(&got.quote_amount_exec)), (c["amount_exec"].as_str().unwrap().to_string(), c["quote_amount_exec"].as_str().map(str::to_string)), "{c}");
        assert_eq!((got.sell, got.limit), (c["side"] == "sell", c["order_type"] == "limit_order"), "{c}");
    }
}

// ---- K4: wiring

/// A 401 on a read carries its status (plan CI T3): the index probe and the tick read it as a rejected key, as
/// Exchange#raise_on_invalid_key! does with honeymaker's `data: { status: 401 }`. A placement's 401 stays a definitive refusal.
#[tokio::test(flavor = "current_thread")]
async fn a_401_read_is_unauthorized_and_a_401_placement_is_rejected() {
    let body = json!({ "code": "400003", "msg": "KC-API-KEY not exists" });
    let (_, v) = venue(json!({ "GET /api/v1/market/orderbook/level2_20": fail(401, body.clone()), "GET /api/v1/accounts": fail(401, json!("")) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Unauthorized(vec![body.to_string()])));
    let Err(VenueError::Unauthorized(e)) = v.free_base_balance(&btc_usdt()).await else { panic!("an empty 401 is Unauthorized too") };
    assert_eq!(e, ["the server responded with status 401 for GET https://api.kucoin.com/api/v1/accounts?type=trade"]);
    let (_, v) = venue(json!({ "POST /api/v1/orders": fail(401, body.clone()) }));
    assert_eq!(v.add_order(&order(OrderKind::Market, true, "60", Side::Buy)).await, Err(VenueError::Rejected(vec![body.to_string()])));
}

/// The factory builds every KuCoin bot's venue through ExchangeProxy.for('kucoin'), read at each build, signing with the
/// key's passphrase; with no install to read the proxy setting from, nothing is sent.
#[tokio::test(flavor = "current_thread")]
async fn the_factory_builds_kucoin_through_its_proxy_with_the_keys_passphrase_per_build() {
    use deltabadger::venue::live::LiveVenue;
    use deltabadger::venue::{alpaca::LiveFactory, VenueFactory};
    use wiremock::{matchers::{header, method}, Mock, MockServer, ResponseTemplate};
    let (key, secret, passphrase) = ("kc-key-0123456789abcdef", "kc-secret-0123456789abcdef0123456789", "kc-passphrase-01");
    let probe = HttpRequest { method: "GET", base: URL.into(), path: "/api/v1/accounts".into(), query: vec![], body: None, not_after: None };
    let signed = kucoin::headers(&probe, key, secret, passphrase, 1).into_iter().find(|(h, _)| h == "KC-API-PASSPHRASE").unwrap().1;
    let rows = r#"{"code":"200000","data":[{"currency":"BTC","available":"0.25","holds":"0"}]}"#;
    let (a, b, direct) = (MockServer::start().await, MockServer::start().await, MockServer::start().await);
    for server in [&a, &b] {
        Mock::given(method("GET")).and(header("KC-API-PASSPHRASE", signed.as_str())).and(header("KC-API-KEY", key))
            .respond_with(ResponseTemplate::new(200).set_body_string(rows)).expect(1).mount(server).await;
    }
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string(rows)).expect(0).mount(&direct).await;
    let dir = tempfile::tempdir().unwrap();
    let c = rusqlite::Connection::open(dir.path().join("primary.sqlite3")).unwrap();
    c.execute_batch("CREATE TABLE app_configs (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT NOT NULL, value TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
        CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT);
        CREATE TABLE assets (id INTEGER PRIMARY KEY, symbol TEXT);
        CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, ticker TEXT, base TEXT, quote TEXT, price_decimals INTEGER, available BOOLEAN, base_asset_id INTEGER, quote_asset_id INTEGER);
        INSERT INTO exchanges VALUES (9, 'Exchanges::Kucoin');").unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap());
    let factory = LiveFactory::for_install(&c, &cipher);
    let creds = Some(deltabadger::crypto::Credentials { redaction_values: vec![], key: key.into(), secret: secret.into(), passphrase: Some(passphrase.into()) });
    for proxy in [&a, &b] {
        deltabadger::app_config::set(&c, &cipher, "proxy_kucoin", &proxy.uri(), Utc::now()).unwrap();
        let LiveVenue::Kucoin(v) = factory.for_bot("Exchanges::Kucoin", creds.clone()) else { panic!("a KuCoin bot gets the KuCoin venue") };
        assert!(std::ptr::eq(v.rules(), &KUCOIN));
        assert_eq!(v.placement_identity().unwrap()["host"], URL);
        assert_eq!(v.with_base(&direct.uri()).free_base_balance(&btc_usdt()).await, Ok(bd("0.25")), "the proxy read at this build");
    }
    for server in [&a, &b, &direct] { server.verify().await; }
    let LiveVenue::Kucoin(v) = LiveFactory::new().for_bot("Exchanges::Kucoin", creds) else { panic!() };
    match v.free_base_balance(&btc_usdt()).await { Err(VenueError::Transient(m)) => assert!(m.contains("proxy"), "{m}"), other => panic!("{other:?}") }
}

#[test]
fn the_kucoin_log_line_is_labelled_kucoin() {
    let t = http::ReqwestTransport::kucoin(http::client(), "KEY".into(), "SECRET".into(), "PASS".into());
    assert_eq!(t.failure_line(&http::HttpResponse { status: 400, body: r#"{"code":"400100","msg":"x"}"#.into() }), "[KuCoin] HTTP 400; venue response diagnostic omitted");
}

/// The engine reads the stored passphrase of a KuCoin key, as Rails' ApiKey#passphrase gives it to the KuCoin client: an
/// empty one signs every order "400004 Invalid KC-API-PASSPHRASE". (The factory test above hands its credentials in directly.)
#[test]
fn the_engine_loads_a_kucoin_keys_stored_passphrase() {
    let (_d, o, s) = common::install();
    let c = &o.primary;
    let cipher = seed::cipher();
    c.execute("UPDATE exchanges SET type = 'Exchanges::Kucoin', name = 'KuCoin'", []).unwrap();
    c.execute("UPDATE api_keys SET passphrase = ?1", [cipher.encrypt("kc-passphrase-01")]).unwrap();
    let id = seed::insert_bot(c, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00"));
    let bot = deltabadger::engine::model::load_bot(c, id).unwrap();
    let creds = deltabadger::engine::model::credentials_for(c, &cipher, &bot).unwrap().expect("the key");
    assert_eq!(creds.passphrase.as_deref(), Some("kc-passphrase-01"));
    // A venue whose client takes none reads none, even with a value stored.
    c.execute("UPDATE exchanges SET type = 'Exchanges::Kraken', name = 'Kraken'", []).unwrap();
    assert_eq!(deltabadger::engine::model::credentials_for(c, &cipher, &bot).unwrap().expect("the key").passphrase, None);
}
