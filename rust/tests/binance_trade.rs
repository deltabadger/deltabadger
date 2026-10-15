//! The Binance trading adapter (venue::binance) over recorded bodies, its signer against honeymaker's own output, and its
//! answers mapped as Exchanges::Binance and Exchange#ambiguous_placement_error? map them. Rails-free: no install, no network.
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::{BinanceVenue, Pair, Pairs};
use deltabadger::venue::http::{self, ScriptedTransport};
use deltabadger::venue::{NewOrder, OrderKind, PriceSide, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn btc_usdt() -> Ticker {
    Ticker { id: 1, ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
             exchange_name: "Binance".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 5, quote_decimals: 8, price_decimals: 2,
             minimum_base_size: bd("0.00001"), minimum_quote_size: bd("5"), trading_enabled: true, available: true, crypto: true }
}
fn pairs() -> Pairs {
    let mut p = Pairs::default();
    p.by_symbol.insert("BTCUSDT".into(), Pair { base: "BTC".into(), quote: "USDT".into(), price_decimals: 2 });
    p.by_symbol.insert("ETHBTC".into(), Pair { base: "ETH".into(), quote: "BTC".into(), price_decimals: 5 });
    for (code, asset) in [("BTC", "BTC"), ("USDT", "USDT"), ("ETH", "ETH")] { p.assets.insert(code.into(), asset.into()); }
    p
}
fn venue(script: Value) -> (ScriptedTransport, BinanceVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), BinanceVenue::new(t, "https://api.binance.com", "s3cr3t").with_pairs(pairs()))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn fail(status: u16, body: Value) -> Value { json!([{ "status": status, "body": body }]) }
/// The query a request sent, without what changes per send.
fn sent(r: &http::HttpRequest) -> Vec<(String, String)> {
    r.query.iter().filter(|(k, _)| !matches!(*k, "timestamp" | "signature")).map(|(k, v)| (k.to_string(), v.clone())).collect()
}
fn pairs_of(v: &[(&str, &str)]) -> Vec<(String, String)> { v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/binance_vectors.json")).unwrap() }
fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for ch in snake.chars() { if ch == '_' { up = true } else if up { out.extend(ch.to_uppercase()); up = false } else { out.push(ch) } }
    out
}

// ---- signing: byte-identical to Honeymaker::Clients::Binance#sign_params under a frozen #timestamp_ms

#[test]
fn every_recorded_honeymaker_signed_query_is_reproduced() {
    let cases = vectors()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 32);
    for c in cases {
        let v = BinanceVenue::new(ScriptedTransport::default(), "https://api.binance.com", c["api_secret"].as_str().unwrap());
        let mut params: Vec<(&'static str, String)> = c["args"].as_object().unwrap().iter()
            .map(|(k, v)| (&*Box::leak(camel(k).into_boxed_str()), v.as_str().unwrap().to_string())).collect();
        if !params.iter().any(|(k, _)| *k == "recvWindow") { params.push(("recvWindow", "5000".into())); } // the gem's default
        let ms = c["timestamp"].as_i64().unwrap();
        let now = chrono::DateTime::from_timestamp_millis(ms).unwrap();
        let method = if c["method"] == "POST" { "POST" } else { "GET" };
        let r = v.signed(method, c["path"].as_str().unwrap(), params, now);
        assert_eq!((r.method, r.path.as_str()), (method, c["path"].as_str().unwrap()), "{c}");
        assert_eq!(r.url().query(), c["query"].as_str(), "{c}");
        assert!(r.body.is_none() && c["body"].is_null(), "honeymaker sends every param in the query, a POST too: {c}");
    }
}

// ---- prices and balance: Exchanges::Binance#get_ask_price / #get_last_price / #get_balances

#[tokio::test(flavor = "current_thread")]
async fn prices_read_the_book_ask_and_the_last_trade_and_refuse_a_zero() {
    let book = |ask: &str| ok(json!({ "symbol": "BTCUSDT", "bidPrice": "64000.10", "bidQty": "1.5", "askPrice": ask, "askQty": "0.2" }));
    let (t, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": book("64000.11"), "GET /api/v3/ticker/price": ok(json!({ "symbol": "BTCUSDT", "price": "64000.05" })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Ok(bd("64000.11")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Ok(bd("64000.05")));
    let keys: Vec<String> = t.requests().iter().map(http::request_key).collect();
    assert_eq!(keys, ["GET /api/v3/ticker/bookTicker?symbol=BTCUSDT", "GET /api/v3/ticker/price?symbol=BTCUSDT"], "unsigned public reads");
    let (_, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": book("0.00000000"), "GET /api/v3/ticker/price": ok(json!({ "price": "0" })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Wrong ask price for BTCUSDT: 0.0".into()])));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["Wrong last price for BTCUSDT: 0.0".into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_price_reads_the_venues_msg_and_an_unreadable_one_is_never_a_price() {
    let (_, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": fail(400, json!({ "code": -1121, "msg": "Invalid symbol." })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Invalid symbol.".into()])));
    for body in [json!({ "bidPrice": "1", "bidQty": "1", "askPrice": "NaN", "askQty": "1" }), json!({ "bidPrice": "1", "askPrice": "2", "askQty": "1" }),
                 json!("<html>maintenance</html>"), json!([])] {
        let (_, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": ok(body.clone()) }));
        assert!(matches!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Ambiguous(_))), "{body}");
    }
    // Spec E §2.10: HTTP 429/418 on a read is a throttle.
    let (_, v) = venue(json!({ "GET /api/v3/ticker/price": fail(429, json!({ "code": -1003, "msg": "Too much request weight used." })) }));
    let Err(VenueError::Rejected(e)) = v.price(&btc_usdt(), PriceSide::Last).await else { panic!() };
    assert!(deltabadger::engine::venue_rules::BINANCE.is_throttle(&e), "{e:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn the_balance_is_the_free_amount_of_the_quote_assets_venue_code() {
    let body = json!({ "makerCommission": 10, "balances": [
        { "asset": "BTC", "free": "0.5", "locked": "0" }, { "asset": "USDT", "free": "1234.5678", "locked": "10" },
        { "asset": "LDUSDT", "free": "99", "locked": "0" } ] });
    let (t, v) = venue(json!({ "GET /api/v3/account": ok(body) }));
    assert_eq!(v.balance("USDT", true).await, Ok(bd("1234.5678")));
    assert_eq!(sent(&t.requests()[0]), pairs_of(&[("omitZeroBalances", "true"), ("recvWindow", "60000")]));
    assert!(t.requests()[0].query.iter().any(|(k, _)| *k == "signature"));
    assert_eq!(v.balance("EUR", true).await, Ok(BigDec::zero()), "a code no ticker maps is no balance");
    let (_, v) = venue(json!({ "GET /api/v3/account": ok(json!({ "balances": [{ "asset": "USDT", "free": "garbage", "locked": "0" }] })) }));
    assert!(matches!(v.balance("USDT", true).await, Err(VenueError::Ambiguous(_))), "an unreadable balance is never a zero");
    let (_, v) = venue(json!({ "GET /api/v3/account": fail(401, json!({ "code": -2015, "msg": "Invalid API-key, IP, or permissions for action." })) }));
    assert_eq!(v.balance("USDT", true).await, Err(VenueError::Rejected(vec!["Invalid API-key, IP, or permissions for action.".into()])));
}

// ---- placement: Exchanges::Binance#set_market_order / #set_limit_order, Exchange#ambiguous_placement_error?

const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str) -> NewOrder {
    NewOrder { pair: "BTCUSDT".into(), kind, volume: volume.into(), quote_volume: quote, cl_ord_id: CL.into(),
               deadline: "2026-09-01T10:00:10Z".parse().unwrap(), day: false }
}
fn accepted() -> Value { ok(json!({ "symbol": "BTCUSDT", "orderId": 28457, "orderListId": -1, "clientOrderId": CL, "transactTime": 1789871400750u64 })) }

#[tokio::test(flavor = "current_thread")]
async fn each_buy_posts_the_fields_rails_sends_plus_the_client_order_id() {
    let cases = [
        (order(OrderKind::Market, true, "60.5"), vec![("newClientOrderId", CL), ("quoteOrderQty", "60.5"), ("recvWindow", "5000"),
            ("selfTradePreventionMode", "EXPIRE_MAKER"), ("side", "BUY"), ("symbol", "BTCUSDT"), ("type", "MARKET")]),
        (order(OrderKind::Market, false, "0.00123"), vec![("newClientOrderId", CL), ("quantity", "0.00123"), ("recvWindow", "5000"),
            ("selfTradePreventionMode", "EXPIRE_MAKER"), ("side", "BUY"), ("symbol", "BTCUSDT"), ("type", "MARKET")]),
        (order(OrderKind::Limit { price: "64150.5".into() }, false, "0.00093"), vec![("newClientOrderId", CL), ("price", "64150.5"),
            ("quantity", "0.00093"), ("recvWindow", "5000"), ("side", "BUY"), ("symbol", "BTCUSDT"), ("timeInForce", "GTC"), ("type", "LIMIT")]),
    ];
    for (o, want) in cases {
        let (t, v) = venue(json!({ "POST /api/v3/order": accepted() }));
        assert_eq!(v.add_order(&o).await, Ok("BTCUSDT-28457".into()), "the stored id is SYMBOL-orderId");
        let r = &t.requests()[0];
        assert_eq!((r.method, r.path.as_str(), r.body.is_none()), ("POST", "/api/v3/order", true));
        assert_eq!(sent(r), pairs_of(&want), "{o:?}");
        assert!(r.query.iter().any(|(k, _)| *k == "signature") && r.not_after.is_some(), "signed, with its send bound");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_to_rails_ambiguity_rules() {
    let m1021 = "Timestamp for this request is outside of the recvWindow.";
    let ahead = "Timestamp for this request was 1000ms ahead of the server's time.";
    let cases: Vec<(Value, Result<String, VenueError>)> = vec![
        (accepted(), Ok("BTCUSDT-28457".into())),
        (ok(json!({ "symbol": "BTCUSDT", "orderId": "28458" })), Ok("BTCUSDT-28458".into())),
        // A definitive refusal: the venue's msg (Exchanges::Binance#parse_error_message).
        (fail(400, json!({ "code": -2010, "msg": "Account has insufficient balance for requested action." })),
         Err(VenueError::Rejected(vec!["Account has insufficient balance for requested action.".into()]))),
        (fail(401, json!({ "code": -2015, "msg": "Invalid API-key, IP, or permissions for action." })),
         Err(VenueError::Rejected(vec!["Invalid API-key, IP, or permissions for action.".into()]))),
        // -1021 is placement-safe (definitive, never ambiguous), in both of Rails' wordings, even on a 5xx.
        (fail(400, json!({ "code": -1021, "msg": m1021 })), Err(VenueError::Rejected(vec![m1021.into()]))),
        (fail(400, json!({ "code": -1021, "msg": ahead })), Err(VenueError::Rejected(vec![ahead.into()]))),
        (fail(503, json!({ "code": -1021, "msg": m1021 })), Err(VenueError::Rejected(vec![m1021.into()]))),
        // A 429 on placement is a definitive refusal (Rails' 4xx rule), never the read throttle.
        (fail(429, json!({ "code": -1015, "msg": "Too many new orders." })), Err(VenueError::Rejected(vec!["Too many new orders.".into()]))),
        // May have been placed: a 5xx, an unreadable 2xx, a 2xx with no orderId, a lost reply.
        (fail(503, json!({ "code": -1001, "msg": "Internal error; unable to process your request. Please try again." })),
         Err(VenueError::Ambiguous("Internal error; unable to process your request. Please try again.".into()))),
        (fail(502, json!("<html>bad gateway</html>")), Err(VenueError::Ambiguous("<html>bad gateway</html>".into()))),
        (ok(json!("<html>maintenance</html>")), Err(VenueError::Ambiguous("Binance: unreadable response".into()))),
        (ok(json!(r#"{"symbol":"BTCUSDT","orderId":1e400}"#)), Err(VenueError::Ambiguous("Binance: unreadable response".into()))),
        (ok(json!({ "symbol": "BTCUSDT" })), Err(VenueError::Ambiguous("Failed to set Binance market order (order_id is BTCUSDT-)".into()))),
        (ok(json!({ "symbol": "BTCUSDT", "orderId": null })), Err(VenueError::Ambiguous("Failed to set Binance market order (order_id is BTCUSDT-)".into()))),
        (json!([{ "network": "post_send", "message": "read timeout" }]), Err(VenueError::Ambiguous("read timeout".into()))),
        (json!([{ "network": "pre_send", "message": "refused" }]), Err(VenueError::Transient("refused".into()))),
        (json!([{ "network": "permanent", "message": "certificate" }]), Err(VenueError::Rejected(vec!["certificate".into()]))),
    ];
    for (reply, want) in cases {
        let (_, v) = venue(json!({ "POST /api/v3/order": reply }));
        assert_eq!(v.add_order(&order(OrderKind::Market, true, "60")).await, want, "{reply}");
    }
    assert!(deltabadger::engine::placement::safe_rejection(&[m1021.into()]) && deltabadger::engine::placement::safe_rejection(&[ahead.into()]));
}
