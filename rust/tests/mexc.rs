//! The MEXC trading adapter (venue::mexc, plan VB slice A5) over recorded bodies: its signer against honeymaker's own requests
//! (the `QueryHmacHex` scheme Binance shares, tests/binance_trade.rs pinning Binance's), its rules against
//! Exchanges::Mexc::ERRORS, #parse_order_data (`parse_binance_shaped`) and the sizing wire recorded from Rails, and its
//! reads, placement, per-order polls, the recovery by client order id (it can only FIND an order) and cancel. Rails-free.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::placement;
use deltabadger::engine::venue_rules::MEXC;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::{Pair, Pairs};
use deltabadger::venue::http::{self, HttpRequest, ScriptedTransport};
use deltabadger::venue::mexc::{self, MexcVenue, ABSENCE_UNPROVEN, URL};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Side, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/mexc_signing.json")).unwrap() }
fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/mexc_vectors.json")).unwrap() }
fn btc_usdt() -> Ticker {
    Ticker { id: 1, ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
             exchange_name: "MEXC".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 6, quote_decimals: 2, price_decimals: 2,
             minimum_base_size: bd("0.000001"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true }
}
fn pairs() -> Pairs {
    let mut p = Pairs::default();
    p.by_symbol.insert("BTCUSDT".into(), Pair { base: "BTC".into(), quote: "USDT".into(), price_decimals: 2 });
    p.by_symbol.insert("PEPEUSDT".into(), Pair { base: "PEPE".into(), quote: "USDT".into(), price_decimals: 10 });
    for code in ["BTC", "USDT", "PEPE"] { p.assets.insert(code.into(), code.into()); }
    p
}
fn venue(script: Value) -> (ScriptedTransport, MexcVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), MexcVenue::new(t, URL, "s3cr3t").with_pairs(pairs()))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn r200(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn fail(status: u16, code: i64, msg: &str) -> Value { json!([{ "status": status, "body": { "code": code, "msg": msg } }]) }
/// The query a request sent, without what changes per send.
fn sent(r: &HttpRequest) -> Vec<(String, String)> {
    r.query.iter().filter(|(k, _)| !matches!(*k, "timestamp" | "signature")).map(|(k, v)| (k.to_string(), v.clone())).collect()
}
fn pairs_of(v: &[(&str, &str)]) -> Vec<(String, String)> { v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str) -> NewOrder {
    NewOrder { pair: "BTCUSDT".into(), side: Side::Buy, kind, volume: volume.into(), quote_volume: quote, cl_ord_id: CL.into(),
               deadline: "2026-09-01T10:00:10Z".parse().unwrap(), day: false }
}
fn raw_order(id: &str, status: &str, kind: &str, executed: &str, cumulative: &str) -> Value {
    json!({ "symbol": "BTCUSDT", "orderId": id, "orderListId": -1, "clientOrderId": CL, "price": if kind == "LIMIT" { "64150" } else { "0" },
            "origQty": if kind == "LIMIT" { "0.00093" } else { "0" }, "executedQty": executed, "cummulativeQuoteQty": cumulative, "status": status,
            "type": kind, "side": "BUY", "origQuoteOrderQty": if kind == "LIMIT" { Value::Null } else { json!("60") } })
}
fn gone() -> Value { fail(400, -2013, "Order does not exist.") }

#[test]
fn the_fixtures_are_recorded_from_the_current_rails_sources() {
    use sha2::Digest;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for f in [fixture(), vectors()] {
        for (file, recorded) in f["sources"].as_object().unwrap() {
            let now = hex::encode(sha2::Sha256::digest(std::fs::read(root.join(file)).unwrap()));
            assert_eq!(recorded.as_str().unwrap(), now, "{file} changed: re-record with script/rust/repin");
        }
    }
}

/// Honeymaker::Clients::Mexc's signed requests, byte for byte under a frozen #timestamp_ms: GETs, POSTs and DELETEs with
/// every param in the query (an escaped one included), X-MEXC-APIKEY on every request of a keyed client, a public read.
#[test]
fn every_recorded_honeymaker_request_is_reproduced() {
    let cases = fixture()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 24);
    for c in cases {
        let method: &'static str = match c["method"].as_str().unwrap() { "GET" => "GET", "POST" => "POST", "DELETE" => "DELETE", m => panic!("{m}") };
        let query = c["query"].as_str().unwrap_or("");
        let params: Vec<(&'static str, String)> = reqwest::Url::parse(&format!("http://x/?{query}")).unwrap().query_pairs()
            .filter(|(k, _)| k != "timestamp" && k != "signature").map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned())).collect();
        let v = MexcVenue::new(ScriptedTransport::default(), URL, c["api_secret"].as_str().unwrap());
        // A moment half a millisecond in: Ruby's Float milliseconds of it are the recorded ones.
        let public = c["call"].as_str().unwrap().ends_with("public");
        let r = if public { v.request(method, c["path"].as_str().unwrap(), params) }
                else { v.signed(method, c["path"].as_str().unwrap(), params, DateTime::from_timestamp_micros(c["timestamp"].as_i64().unwrap() * 1000 + 500).unwrap()) };
        // The query as ReqwestTransport::mexc sends it (`sign::request`): Faraday::Utils.build_query, the text signed.
        assert_eq!(deltabadger::venue::binance::build_query(&r.query), c["query"].as_str().unwrap_or(""), "{c}");
        assert!(r.body.is_none() && c["body"].as_str().is_none_or(str::is_empty), "honeymaker sends every param in the query, a POST too: {c}");
        // Faraday's header names are case-insensitive (it keeps "X-mexc-apikey").
        let h: std::collections::BTreeMap<String, Value> = c["headers"].as_object().unwrap().iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .filter(|(k, _)| k != "content-length").collect(); // Faraday's adapter, and reqwest, count the empty body themselves
        let want: std::collections::BTreeMap<String, Value> = [("x-mexc-apikey", c["api_key"].clone()), ("accept", json!("application/json")), ("content-type", json!("application/json"))]
            .into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(h, want, "the key header and the JSON ones only, as ReqwestTransport::mexc sends them: {c}");
    }
}

/// engine::venue_rules::MEXC is Exchanges::Mexc::ERRORS, and Exchange#ambiguous_placement_error? as it reads them.
#[test]
fn the_rules_are_rails_errors() {
    let f = fixture();
    let list = |v: &Value| v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    for (kind, patterns) in MEXC.kinds {
        assert_eq!(patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(), list(&f["errors"][kind]), "{kind}");
    }
    let mut transient = list(&f["network_transient"]);
    transient.extend(list(&f["errors"]["transient"]));
    assert_eq!(MEXC.transient_text.iter().map(|s| s.to_string()).collect::<Vec<_>>(), transient, "Exchange#transient_error?");
    let safe = list(&f["placement_safe"]);
    for m in list(&f["errors"]["transient"]) {
        assert_eq!(MEXC.add_outcome_unknown(std::slice::from_ref(&m)), !safe.iter().any(|s| m.contains(s.as_str())), "{m}");
        assert_eq!(placement::safe_rejection(std::slice::from_ref(&m)), safe.iter().any(|s| m.contains(s.as_str())), "{m}");
    }
    assert_eq!(f["order_gone_codes"], json!(mexc::ORDER_GONE_CODES));
    assert!(MEXC.reports_missing && MEXC.young_missing_fails && !MEXC.absence_provable && MEXC.follow_up_strict);
}

/// #parse_order_data (X1, X2), recorded over 10,000 generated GET /api/v3/order answers.
#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let v = vectors();
    let cases = v["mexc_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 10_000);
    let s = |o: &Option<BigDec>| o.as_ref().map(BigDec::to_s_f);
    let (mut averaged, mut rounded) = (0, 0);
    for c in cases {
        let id = format!("{}-{}", c["raw"]["symbol"].as_str().unwrap(), c["raw"]["orderId"].as_str().unwrap());
        let got = mexc::parse_order(&id, &c["raw"], &pairs()).unwrap();
        let status = match got.status { OrderStatus::Open => "open", OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", OrderStatus::Unknown => "unknown", OrderStatus::Failed => "failed" };
        assert_eq!(status, c["status"], "{c}");
        assert_eq!(s(&got.price).as_deref(), c["price"].as_str(), "price {c}");
        assert_eq!((s(&got.amount).as_deref(), s(&got.quote_amount).as_deref()), (c["amount"].as_str(), c["quote_amount"].as_str()), "{c}");
        assert_eq!((got.amount_exec.to_s_f(), s(&got.quote_amount_exec)), (c["amount_exec"].as_str().unwrap().to_string(), c["quote_amount_exec"].as_str().map(str::to_string)), "{c}");
        assert_eq!((got.sell, got.limit), (c["side"] == "sell", c["order_type"] == "limit_order"), "{c}");
        let filled = bd(c["raw"]["executedQty"].as_str().unwrap()).is_positive() && bd(c["raw"]["cummulativeQuoteQty"].as_str().unwrap()).is_positive();
        if filled && c["raw"]["price"] != "0" { averaged += 1; }
        if filled && c["raw"]["symbol"] != "NEWUSDT" { rounded += 1; }
    }
    assert!(averaged > 1000 && rounded > 1000, "X2's average over a non-zero price, and the tick rounding, are exercised: {averaged} {rounded}");
    // An unlisted status raises in Rails (#parse_order_status): refused, never read as some status.
    let raw = raw_order("C02__1", "PENDING_CANCEL", "MARKET", "0", "0");
    assert_eq!(mexc::parse_order("BTCUSDT-C02__1", &raw, &pairs()), Err(VenueError::Rejected(vec!["Unknown MEXC order status: PENDING_CANCEL".into()])));
}

/// Bot::OrderSetter's sizing under Exchanges::Mexc#minimum_amount_logic and #set_market_order / #set_limit_order's wire (a
/// zero base minimum among the pairs: the quote minimum alone), through amount::size, limit_refusal, to_order and the
/// adapter's own POST query.
#[tokio::test(flavor = "current_thread")]
async fn every_recorded_rails_sizing_and_wire_is_reproduced() {
    let (_d, o, s) = common::install();
    use common::seed::{self, BotSpec};
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let v = vectors();
    let cases = v["mexc_sizing"].as_array().unwrap();
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut quote, mut refused, mut zero_base) = (0, 0, 0);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let shape = &v["mexc_tickers"][c["ticker"].as_u64().unwrap() as usize];
        let int = |k: &str| shape[k].as_str().unwrap().parse::<i64>().unwrap();
        let t = Ticker { base_decimals: int("base_decimals"), quote_decimals: int("quote_decimals"), price_decimals: int("price_decimals"),
                         minimum_base_size: bd(shape["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(shape["minimum_quote_size"].as_str().unwrap()), ..btc_usdt() };
        if t.minimum_base_size.is_zero() { zero_base += 1; }
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &MEXC).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!((plan.price.to_s_f(), plan.amount.to_s_f()), (c["price"].as_str().unwrap().to_string(), c["amount"].as_str().unwrap().to_string()), "{c}");
        assert_eq!((plan.quote_type, below), (c["amount_type"] == "quote", c["below_minimum"].as_bool().unwrap()), "{c}");
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        let order = plan.to_order(CL.into(), deadline, MEXC.wire).unwrap();
        let (t, venue) = venue(json!({ "POST /api/v3/order": ok(json!({ "symbol": "BTCUSDT", "orderId": "C02__1" })) }));
        assert_eq!(venue.add_order(&order).await, Ok("BTCUSDT-C02__1".into()));
        let mut ours: serde_json::Map<String, Value> = sent(&t.requests()[0]).into_iter().map(|(k, v)| (k, json!(v))).collect();
        assert_eq!((ours.remove("newClientOrderId"), ours.remove("recvWindow")), (Some(json!(CL)), Some(json!("5000"))), "{c}");
        let mut want = serde_json::Map::new();
        for (rails, field) in c["wire"].as_object().unwrap() {
            let camel = match rails.as_str() { "quote_order_qty" => "quoteOrderQty", "time_in_force" => "timeInForce", other => other };
            want.insert(camel.to_string(), field.clone());
        }
        if want.contains_key("quoteOrderQty") { quote += 1; }
        assert_eq!(Value::Object(ours), Value::Object(want), "{c}");
    }
    assert!(quote > 1000 && refused > 500 && zero_base > 3000, "both market wires, the limit refusal and a zero base minimum are exercised: {quote} {refused} {zero_base}");
}

// ---- reads

#[tokio::test(flavor = "current_thread")]
async fn prices_and_balances_read_as_rails_reads_them() {
    let book = json!({ "symbol": "BTCUSDT", "bidPrice": "64000.10", "bidQty": "1.5", "askPrice": "64000.11", "askQty": "0.2" });
    let (t, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": ok(book), "GET /api/v3/ticker/price": ok(json!({ "symbol": "BTCUSDT", "price": "64000.05" })),
                               "GET /api/v3/account": ok(json!({ "canTrade": true, "balances": [{ "asset": "BTC", "free": "0.5", "locked": "0.1" }, { "asset": "USDT", "free": "100.25", "locked": "0" }] })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Ok(bd("64000.11")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Ok(bd("64000.10")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Ok(bd("64000.05")));
    assert_eq!(v.balance("USDT", true).await, Ok(bd("100.25")));
    assert_eq!(v.free_base_balance(&btc_usdt()).await, Ok(bd("0.5")));
    let keys: Vec<String> = t.requests().iter().map(http::request_key).collect();
    assert_eq!(keys[..3], ["GET /api/v3/ticker/bookTicker?symbol=BTCUSDT", "GET /api/v3/ticker/bookTicker?symbol=BTCUSDT", "GET /api/v3/ticker/price?symbol=BTCUSDT"]);
    assert_eq!(sent(&t.requests()[3]), pairs_of(&[("recvWindow", "5000")]), "the gem's recvWindow, and no omitZeroBalances");
    let (_, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": ok(json!({ "bidPrice": "0", "bidQty": "1", "askPrice": "0", "askQty": "1" })), "GET /api/v3/ticker/price": ok(json!({ "symbol": "BTCUSDT" })) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Err(VenueError::Rejected(vec!["Wrong bid price for BTCUSDT: 0.0".into()])));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Err(VenueError::Ambiguous("Key path price not found".into())));
    let (_, v) = venue(json!({ "GET /api/v3/account": fail(400, 10072, "Api key info invalid") }));
    let Err(VenueError::Rejected(e)) = v.balance("USDT", true).await else { panic!() };
    assert!(MEXC.is_invalid_key(&e), "#parse_error_message's msg: {e:?}");
    let (_, v) = venue(json!({ "GET /api/v3/ticker/price": ok(json!([{ "symbol": "BTCUSDT", "price": "64000" }, { "symbol": "ETHUSDT", "price": "0" }, { "symbol": "BTCUSDT", "price": "64001" }])) }));
    assert_eq!(v.tickers_prices(&[]).await, Ok(vec![("ETHUSDT".into(), json!("0.0")), ("BTCUSDT".into(), json!("64001.0"))]));
}

// ---- placement

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_as_rails_maps_them() {
    let place = |reply: Value| async move { venue(json!({ "POST /api/v3/order": reply })).1.add_order(&order(OrderKind::Market, true, "60")).await };
    assert_eq!(place(ok(json!({ "symbol": "BTCUSDT", "orderId": "C02__443776347957968896" }))).await, Ok("BTCUSDT-C02__443776347957968896".into()));
    assert!(matches!(place(ok(json!({ "symbol": "BTCUSDT" }))).await, Err(VenueError::Ambiguous(_))), "no orderId: it may have been placed");
    let Err(VenueError::Rejected(e)) = place(fail(400, 10101, "Insufficient balance")).await else { panic!() };
    assert_eq!(MEXC.failure_kind(&e), Some("insufficient_funds"));
    let Err(VenueError::Rejected(e)) = place(fail(400, 700_003, "Timestamp for this request is outside of the recvWindow.")).await else { panic!() };
    assert!(placement::safe_rejection(&e), "700003: no row, the run rescheduled");
    let Err(VenueError::Rejected(e)) = place(fail(400, 503, "service not available, please try again")).await else { panic!() };
    assert!(MEXC.add_outcome_unknown(&e), "MEXC's own try-again may have placed the order");
    assert!(matches!(place(fail(503, 503, "service not available")).await, Err(VenueError::Ambiguous(_))));
    assert!(matches!(place(json!([{ "network": "post_send", "message": "Net::ReadTimeout" }])).await, Err(VenueError::Ambiguous(_))));
    assert!(matches!(place(json!([{ "network": "pre_send", "message": "refused" }])).await, Err(VenueError::Rejected(_))));
    // A limit order: GTC price and quantity, the intent id as newClientOrderId, the gem's recvWindow; every field in the query.
    let (t, v) = venue(json!({ "POST /api/v3/order": ok(json!({ "orderId": "o1" })) }));
    v.add_order(&order(OrderKind::Limit { price: "63840".into() }, false, "0.000939")).await.unwrap();
    assert!(t.requests()[0].body.is_none());
    assert_eq!(sent(&t.requests()[0]), pairs_of(&[("newClientOrderId", CL), ("price", "63840"), ("quantity", "0.000939"), ("recvWindow", "5000"), ("side", "BUY"),
                                                  ("symbol", "BTCUSDT"), ("timeInForce", "GTC"), ("type", "LIMIT")]));
    // MEXC has no cross (D-679 is Gemini's and Hyperliquid's): a crossed plan is never sent.
    let (t, v) = venue(json!({ "POST /api/v3/order": ok(json!({ "orderId": "o1" })) }));
    assert!(matches!(v.add_order(&order(OrderKind::Crossed { price: "64000".into() }, false, "0.000939")).await, Err(VenueError::Transient(_))));
    assert!(t.requests().is_empty());
}

// ---- polls, cancel, recovery

#[tokio::test(flavor = "current_thread")]
async fn each_order_is_asked_alone_and_a_gone_one_is_missing() {
    let (t, v) = venue(json!({ "GET /api/v3/order": [r200(raw_order("C02__5", "FILLED", "MARKET", "0.001", "64.0004")), gone()[0].clone(),
                                                     r200(raw_order("C02__7", "PARTIALLY_CANCELED", "LIMIT", "0.0004", "25.66"))] }));
    let got = v.orders(&["BTCUSDT-C02__5".into(), "BTCUSDT-C02__6".into(), "BTCUSDT-C02__7".into()]).await.unwrap();
    let ids: Vec<&str> = got.iter().map(|o| o.txid.as_str()).collect();
    assert_eq!(ids, ["BTCUSDT-C02__5", "BTCUSDT-C02__7"], "-2013: left out, for Bot::StaleOrderResolver");
    assert_eq!((got[0].status, got[0].price.clone()), (OrderStatus::Closed, Some(bd("64000.4"))), "the average, rounded half up to the tick");
    assert_eq!((got[1].status, got[1].price.clone(), got[1].amount_exec.clone()), (OrderStatus::Cancelled, Some(bd("64150")), bd("0.0004")),
               "X1: terminal with its fill; X2: priced at its fills (25.66 / 0.0004), not its limit");
    assert_eq!(sent(&t.requests()[1]), pairs_of(&[("orderId", "C02__6"), ("recvWindow", "5000"), ("symbol", "BTCUSDT")]));
    // Only MEXC's own codes are gone; anything else fails the batch.
    let (_, v) = venue(json!({ "GET /api/v3/order": [{ "status": 404, "body": "<html>Not Found</html>" }] }));
    assert!(matches!(v.orders(&["BTCUSDT-C02__1".into()]).await, Err(VenueError::Rejected(_))));
    let (_, v) = venue(json!({ "GET /api/v3/order": fail(400, -2011, "Unknown order sent.") }));
    assert_eq!(v.orders(&["BTCUSDT-C02__1".into()]).await, Ok(vec![]));
    let (_, v) = venue(json!({ "GET /api/v3/order": gone() }));
    assert_eq!(v.order_identified("BTCUSDT-C02__1", |_, _, _| Ok(true)).await, Err(VenueError::Rejected(vec!["MEXC no longer knows order BTCUSDT-C02__1".into()])));
    assert_eq!(v.fills_from_trades(&["BTCUSDT-C02__1".into()], Utc::now()).await, Ok(vec![]), "Rails reads no trades for an order");
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_sends_the_symbol_and_order_id() {
    let (t, v) = venue(json!({ "DELETE /api/v3/order": ok(json!({ "orderId": "C02__1" })) }));
    assert_eq!(v.cancel("BTCUSDT-C02__1").await, Ok(()));
    assert_eq!(sent(&t.requests()[0]), pairs_of(&[("orderId", "C02__1"), ("recvWindow", "5000"), ("symbol", "BTCUSDT")]));
}

/// Recovery (plan VB §1.5) asks for the newClientOrderId the intent sent: a hit naming it is the order; -2013 is never absence.
#[tokio::test(flavor = "current_thread")]
async fn the_client_order_lookup_finds_an_order_and_never_proves_one_absent() {
    let since: DateTime<Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    let (t, v) = venue(json!({ "GET /api/v3/order": ok(raw_order("C02__9", "FILLED", "MARKET", "0.00093", "59.52")) }));
    let o = v.order_by_client_id(CL, "BTCUSDT", since).await.unwrap().unwrap();
    assert_eq!((o.txid.as_str(), o.status), ("BTCUSDT-C02__9", OrderStatus::Closed));
    assert_eq!(sent(&t.requests()[0]), pairs_of(&[("origClientOrderId", CL), ("recvWindow", "5000"), ("symbol", "BTCUSDT")]));
    let (_, v) = venue(json!({ "GET /api/v3/order": gone() }));
    assert_eq!(v.order_by_client_id(CL, "BTCUSDT", since).await, Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())));
    let (_, v) = venue(json!({ "GET /api/v3/order": ok(json!({ "symbol": "BTCUSDT", "orderId": "C02__1", "clientOrderId": "another" })) }));
    assert!(matches!(v.order_by_client_id(CL, "BTCUSDT", since).await, Err(VenueError::Rejected(_))));
}
