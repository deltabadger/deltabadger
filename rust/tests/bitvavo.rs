//! The Bitvavo trading adapter (venue::bitvavo, plan VB slice A3) over recorded bodies: its signer against honeymaker's own
//! requests (the `Prehash` scheme KuCoin shares, tests/kucoin.rs pinning KuCoin's), its rules against Exchanges::Bitvavo::ERRORS,
//! #parse_order_data and the sizing wire recorded from Rails, and its reads, placement, polls, the gone-order recovery from
//! the trades, the recovery by client order id (it can only FIND an order) and cancel. Rails-free.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::placement;
use deltabadger::engine::venue_rules::BITVAVO;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::Pairs;
use deltabadger::venue::bitvavo::{self, BitvavoVenue, ABSENCE_UNPROVEN, URL};
use deltabadger::venue::http::{HttpRequest, ScriptedTransport};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Side, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/bitvavo_signing.json")).unwrap() }
fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/bitvavo_vectors.json")).unwrap() }
fn btc_eur() -> Ticker {
    Ticker { id: 1, ticker: "BTC-EUR".into(), base_code: "BTC".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(),
             exchange_name: "Bitvavo".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 2, price_decimals: 0,
             minimum_base_size: bd("0.0001"), minimum_quote_size: bd("5"), trading_enabled: true, available: true, crypto: true }
}
fn venue(script: Value) -> (ScriptedTransport, BitvavoVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    let mut p = Pairs::default();
    for code in ["BTC", "EUR"] { p.assets.insert(code.into(), code.into()); }
    (t.clone(), BitvavoVenue::new(t, URL).with_pairs(p))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn fail(status: u16, body: Value) -> Value { json!([{ "status": status, "body": body }]) }
fn lower(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
    h.sort();
    h
}
const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str) -> NewOrder {
    NewOrder { pair: "BTC-EUR".into(), side: Side::Buy, kind, volume: volume.into(), quote_volume: quote, cl_ord_id: CL.into(),
               deadline: "2026-09-01T10:00:10Z".parse().unwrap(), day: false }
}
fn gone() -> Value { fail(404, json!({ "errorCode": 240, "error": "No order found. Please be aware that simultaneously updating the same order may return this error." })) }

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

/// Honeymaker::Clients::Bitvavo#signed_headers, byte for byte under a frozen #timestamp_ms: GETs with and without a query, an
/// escaped one, POSTs (the JSON body as sent), a DELETE (its query signed, where KuCoin's signs the path alone), a public read.
#[test]
fn every_recorded_honeymaker_request_is_reproduced() {
    let cases = fixture()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 20);
    for c in cases {
        let method: &'static str = match c["method"].as_str().unwrap() { "GET" => "GET", "POST" => "POST", "DELETE" => "DELETE", m => panic!("{m}") };
        let query: Vec<(&'static str, String)> = c["query"].as_str().map(|q| {
            reqwest::Url::parse(&format!("http://x/?{q}")).unwrap().query_pairs().map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned())).collect()
        }).unwrap_or_default();
        let body = c["body"].as_str().map(|b| serde_json::from_str::<Value>(b).unwrap());
        let r = HttpRequest { method, base: URL.into(), path: c["path"].as_str().unwrap().into(), query, body, not_after: None };
        assert_eq!(deltabadger::venue::binance::build_query(&r.query), c["query"].as_str().unwrap_or(""), "Faraday::Utils.build_query: {c}");
        assert_eq!(r.body.as_ref().map(Value::to_string).as_deref(), c["body"].as_str(), "the body honeymaker sends: {c}");
        let h = bitvavo::headers(&r, c["api_key"].as_str().unwrap(), c["api_secret"].as_str().unwrap(), c["timestamp"].as_i64().unwrap());
        let want = lower(&c["headers"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect::<Vec<_>>());
        assert_eq!(lower(&h), want, "{c}");
    }
}

/// engine::venue_rules::BITVAVO is Exchanges::Bitvavo::ERRORS, and Exchange#ambiguous_placement_error? as it reads them.
#[test]
fn the_rules_are_rails_errors() {
    let f = fixture();
    let list = |v: &Value| v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    for (kind, patterns) in BITVAVO.kinds {
        assert_eq!(patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(), list(&f["errors"][kind]), "{kind}");
    }
    let mut transient = list(&f["network_transient"]);
    transient.extend(list(&f["errors"]["transient"]));
    assert_eq!(BITVAVO.transient_text.iter().map(|s| s.to_string()).collect::<Vec<_>>(), transient, "Exchange#transient_error?");
    let safe = list(&f["errors"]["placement_safe_transient"]);
    for m in list(&f["errors"]["transient"]) {
        let body = format!("{{{m},\"error\":\"x\"}}");
        assert_eq!(BITVAVO.add_outcome_unknown(std::slice::from_ref(&body)), !safe.contains(&m), "{m}");
    }
    for m in safe.iter().chain(list(&f["placement_safe"]).iter()) { assert!(placement::safe_rejection(std::slice::from_ref(m)), "{m}"); }
    assert!(BITVAVO.reports_missing && !BITVAVO.young_missing_fails && !BITVAVO.absence_provable && !BITVAVO.follow_up_strict);
}

/// #parse_order_data (V3), recorded over 10,000 generated GET /v2/order answers.
#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let v = vectors();
    let cases = v["bitvavo_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 10_000);
    let s = |o: &Option<BigDec>| o.as_ref().map(BigDec::to_s_f);
    for c in cases {
        let id = format!("{}-{}", c["raw"]["market"].as_str().unwrap(), c["raw"]["orderId"].as_str().unwrap());
        let got = bitvavo::parse_order(&id, &c["raw"]).unwrap();
        let status = match got.status { OrderStatus::Open => "open", OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", OrderStatus::Unknown => "unknown", OrderStatus::Failed => "failed" };
        assert_eq!(status, c["status"], "{c}");
        assert_eq!(s(&got.price).as_deref(), c["price"].as_str(), "price {c}");
        assert_eq!((s(&got.amount).as_deref(), s(&got.quote_amount).as_deref()), (c["amount"].as_str(), c["quote_amount"].as_str()), "{c}");
        assert_eq!((got.amount_exec.to_s_f(), s(&got.quote_amount_exec)), (c["amount_exec"].as_str().unwrap().to_string(), c["quote_amount_exec"].as_str().map(str::to_string)), "{c}");
        assert_eq!((got.sell, got.limit), (c["side"] == "sell", c["order_type"] == "limit_order"), "{c}");
    }
}

/// Bot::OrderSetter's sizing under Exchanges::Bitvavo#minimum_amount_logic and #set_market_order / #set_limit_order's wire,
/// through amount::size, limit_refusal, to_order and the adapter's own POST body.
#[tokio::test(flavor = "current_thread")]
async fn every_recorded_rails_sizing_and_wire_is_reproduced() {
    let (_d, o, s) = common::install();
    use common::seed::{self, BotSpec};
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let v = vectors();
    let cases = v["bitvavo_sizing"].as_array().unwrap();
    assert_eq!(cases.len(), 10_657);
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut quote, mut refused) = (0, 0);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let shape = &v["bitvavo_tickers"][c["ticker"].as_u64().unwrap() as usize];
        let int = |k: &str| shape[k].as_str().unwrap().parse::<i64>().unwrap();
        let t = Ticker { base_decimals: int("base_decimals"), quote_decimals: int("quote_decimals"), price_decimals: int("price_decimals"),
                         minimum_base_size: bd(shape["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(shape["minimum_quote_size"].as_str().unwrap()), ..btc_eur() };
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &BITVAVO).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!((plan.price.to_s_f(), plan.amount.to_s_f()), (c["price"].as_str().unwrap().to_string(), c["amount"].as_str().unwrap().to_string()), "{c}");
        assert_eq!((plan.quote_type, below), (c["amount_type"] == "quote", c["below_minimum"].as_bool().unwrap()), "{c}");
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        let order = plan.to_order(CL.into(), deadline, BITVAVO.wire).unwrap();
        let (t, venue) = venue(json!({ "POST /v2/order": ok(json!({ "orderId": "x", "market": "BTC-EUR" })) }));
        assert_eq!(venue.add_order(&order).await, Ok("BTC-EUR-x".into()));
        let mut sent = t.requests()[0].body.clone().unwrap();
        assert_eq!((sent["clientOrderId"].take(), sent["operatorId"].take()), (json!(CL), json!(1)), "{c}");
        let w = &c["wire"];
        let mut want = json!({ "market": "BTC-EUR", "side": "buy", "orderType": w["order_type"] });
        for (rails, ours) in [("amount", "amount"), ("amount_quote", "amountQuote"), ("price", "price")] {
            if let Some(x) = w.get(rails) { want[ours] = x.clone(); }
        }
        if w.get("amount_quote").is_some() { quote += 1; }
        let sent: serde_json::Map<String, Value> = sent.as_object().unwrap().iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(Value::Object(sent), want, "{c}");
    }
    assert_eq!((quote, refused), (1_862, 1_977), "both market wires and the limit refusal are exercised");
}

// ---- reads

#[tokio::test(flavor = "current_thread")]
async fn prices_and_balances_read_as_rails_reads_them() {
    let book = json!({ "market": "BTC-EUR", "bid": "64000", "bidSize": "1.5", "ask": "64001", "askSize": "0.2" });
    let (t, v) = venue(json!({ "GET /v2/ticker/book": ok(book), "GET /v2/ticker/price": ok(json!({ "market": "BTC-EUR", "price": "64000.5" })),
                               "GET /v2/balance": ok(json!([{ "symbol": "BTC", "available": "0.5", "inOrder": "0.1" }, { "symbol": "EUR", "available": "100.25", "inOrder": "0" }])) }));
    assert_eq!(v.price(&btc_eur(), PriceSide::Ask).await, Ok(bd("64001")));
    assert_eq!(v.price(&btc_eur(), PriceSide::Bid).await, Ok(bd("64000")));
    assert_eq!(v.price(&btc_eur(), PriceSide::Last).await, Ok(bd("64000.5")));
    assert_eq!(v.balance("EUR", true).await, Ok(bd("100.25")));
    assert_eq!(v.free_base_balance(&btc_eur()).await, Ok(bd("0.5")));
    let keys: Vec<String> = t.requests().iter().map(deltabadger::venue::http::request_key).collect();
    assert_eq!(keys[..3], ["GET /v2/ticker/book?market=BTC-EUR", "GET /v2/ticker/book?market=BTC-EUR", "GET /v2/ticker/price?market=BTC-EUR"]);
    // A zero price raises "Wrong … price", a missing field is Rails' KeyError, an error body is the venue's.
    let (_, v) = venue(json!({ "GET /v2/ticker/book": ok(json!({ "bid": "0", "bidSize": "1", "ask": "0", "askSize": "1" })), "GET /v2/ticker/price": ok(json!({ "market": "BTC-EUR" })) }));
    assert_eq!(v.price(&btc_eur(), PriceSide::Bid).await, Err(VenueError::Rejected(vec!["Wrong bid price for BTC-EUR: 0.0".into()])));
    assert_eq!(v.price(&btc_eur(), PriceSide::Last).await, Err(VenueError::Ambiguous("Key path price not found".into())));
    let body = json!({ "errorCode": 305, "error": "No active API key found." });
    let (_, v) = venue(json!({ "GET /v2/balance": fail(403, body.clone()) }));
    let Err(VenueError::Rejected(e)) = v.balance("EUR", true).await else { panic!() };
    assert!(BITVAVO.is_invalid_key(&e), "{e:?}");
    let (_, v) = venue(json!({ "GET /v2/ticker/price": ok(json!([{ "market": "BTC-EUR", "price": "64000" }, { "market": "ETH-EUR", "price": "0" }, { "market": "X-EUR", "price": null }])) }));
    assert_eq!(v.tickers_prices(&[]).await, Ok(vec![("BTC-EUR".into(), json!("64000.0"))]));
}

// ---- placement

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_as_rails_maps_them() {
    let err = |status: u16, code: i64, error: &str| fail(status, json!({ "errorCode": code, "error": error }));
    let place = |reply: Value| async move { venue(json!({ "POST /v2/order": reply })).1.add_order(&order(OrderKind::Market, true, "60")).await };
    assert_eq!(place(ok(json!({ "orderId": "abc-1", "market": "BTC-EUR" }))).await, Ok("BTC-EUR-abc-1".into()));
    assert_eq!(place(ok(json!({ "market": "BTC-EUR" }))).await, Err(VenueError::Ambiguous(bitvavo::NO_ORDER_ID.into())));
    let Err(VenueError::Rejected(e)) = place(err(400, 216, "You do not have sufficient balance to complete this operation.")).await else { panic!() };
    assert_eq!(BITVAVO.failure_kind(&e), Some("insufficient_funds"));
    let Err(VenueError::Rejected(e)) = place(err(403, 304, "outside the access window")).await else { panic!() };
    assert!(placement::safe_rejection(&e), "304: no row, the run rescheduled");
    let Err(VenueError::Rejected(e)) = place(err(400, 109, "timed out")).await else { panic!() };
    assert!(BITVAVO.add_outcome_unknown(&e), "109 may have placed the order");
    assert!(matches!(place(fail(503, json!("<html>upstream</html>"))).await, Err(VenueError::Ambiguous(_))));
    assert!(matches!(place(json!([{ "network": "post_send", "message": "Net::ReadTimeout" }])).await, Err(VenueError::Ambiguous(_))));
    assert!(matches!(place(json!([{ "network": "pre_send", "message": "refused" }])).await, Err(VenueError::Rejected(_))));
    // A limit order: base amount and price, the intent id as clientOrderId, operatorId 1.
    let (t, v) = venue(json!({ "POST /v2/order": ok(json!({ "orderId": "o1", "market": "BTC-EUR" })) }));
    v.add_order(&order(OrderKind::Limit { price: "63840".into() }, false, "0.00093984")).await.unwrap();
    assert_eq!(t.requests()[0].body.as_ref().unwrap().to_string(),
               format!(r#"{{"market":"BTC-EUR","side":"buy","orderType":"limit","amount":"0.00093984","price":"63840","clientOrderId":"{CL}","operatorId":1}}"#));
}

// ---- polls, the gone-order recovery, cancel

fn trade(id: &str, order: &str, amount: &str, price: &str, at_ms: i64, side: &str) -> Value {
    json!({ "id": id, "orderId": order, "timestamp": at_ms, "market": "BTC-EUR", "side": side, "amount": amount, "price": price, "fee": "0.1", "feeCurrency": "EUR" })
}

#[tokio::test(flavor = "current_thread")]
async fn a_gone_order_is_missing_and_its_fills_come_from_every_trade_page() {
    let since: DateTime<Utc> = "2026-09-01T09:00:00Z".parse().unwrap();
    let start = bitvavo::ruby_ms(since);
    // A full page (1000) of other orders' trades, then the page before its oldest id holding the order's two fills.
    let full: Vec<Value> = (0..1000).map(|i| trade(&format!("p{i}"), "other", "1", "1", start + 10_000_000 - i, "buy")).collect();
    let second = json!([trade("f2", "abc", "0.0003", "64010", start + 2000, "buy"), trade("f1", "abc", "0.0006", "64000", start + 1000, "buy"), trade("old", "abc", "9", "1", start - 1, "buy")]);
    let (t, v) = venue(json!({ "GET /v2/order": gone(), "GET /v2/trades": [{ "status": 200, "body": full }, { "status": 200, "body": second }] }));
    assert_eq!(v.orders(&["BTC-EUR-abc".into()]).await, Ok(vec![]), "240: left out, for the trades recovery");
    let fills = v.fills_from_trades(&["BTC-EUR-abc".into()], since).await.unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!((fills[0].status, fills[0].amount_exec.to_s_f(), fills[0].quote_amount_exec.as_ref().map(BigDec::to_s_f)), (OrderStatus::Closed, "0.0009".into(), Some("57.603".into())));
    assert_eq!(fills[0].price, bd("57.603").div(&bd("0.0009")), "the average, unrounded");
    let keys: Vec<String> = t.requests().iter().skip(1).map(deltabadger::venue::http::request_key).collect();
    assert_eq!(keys, ["GET /v2/trades?limit=1000&market=BTC-EUR", "GET /v2/trades?limit=1000&market=BTC-EUR&tradeIdTo=p999"]);
    // Only Bitvavo's own 240 is gone; a gateway 404 fails the batch.
    let (_, v) = venue(json!({ "GET /v2/order": fail(404, json!("<html>Not Found</html>")) }));
    assert!(matches!(v.orders(&["BTC-EUR-abc".into()]).await, Err(VenueError::Rejected(_))));
    // A full page with nothing new fails, never loops.
    let (_, v) = venue(json!({ "GET /v2/trades": [{ "status": 200, "body": (0..1000).map(|i| trade(&format!("p{i}"), "x", "1", "1", start + 1000 + i, "buy")).collect::<Vec<_>>() }] }));
    let Err(VenueError::Rejected(e)) = v.fills_from_trades(&["BTC-EUR-abc".into()], since).await else { panic!() };
    assert_eq!(e, ["Bitvavo trades on BTC-EUR returned a full page with nothing new"]);
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_sends_the_market_order_id_and_operator() {
    let (t, v) = venue(json!({ "DELETE /v2/order": ok(json!({ "orderId": "a-b" })) }));
    assert_eq!(v.cancel("BTC-EUR-a-b").await, Ok(()));
    assert_eq!(deltabadger::venue::http::request_key(&t.requests()[0]), "DELETE /v2/order?market=BTC-EUR&operatorId=1&orderId=a-b");
}

/// Recovery (plan VB §1.5) asks for the clientOrderId the intent sent: a hit naming it is the order; a 240 is never absence.
#[tokio::test(flavor = "current_thread")]
async fn the_client_order_lookup_finds_an_order_and_never_proves_one_absent() {
    let since: DateTime<Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    let found = json!({ "orderId": "o-9", "market": "BTC-EUR", "clientOrderId": CL, "status": "filled", "side": "buy", "orderType": "market",
                        "amountQuote": "60", "filledAmount": "0.00093", "filledAmountQuote": "59.52" });
    let (t, v) = venue(json!({ "GET /v2/order": ok(found) }));
    let o = v.order_by_client_id(CL, "BTC-EUR", since).await.unwrap().unwrap();
    assert_eq!((o.txid.as_str(), o.status), ("BTC-EUR-o-9", OrderStatus::Closed));
    assert_eq!(deltabadger::venue::http::request_key(&t.requests()[0]), format!("GET /v2/order?clientOrderId={CL}&market=BTC-EUR"));
    let (_, v) = venue(json!({ "GET /v2/order": gone() }));
    assert_eq!(v.order_by_client_id(CL, "BTC-EUR", since).await, Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())));
    let (_, v) = venue(json!({ "GET /v2/order": ok(json!({ "orderId": "o-1", "market": "BTC-EUR", "clientOrderId": "another" })) }));
    assert!(matches!(v.order_by_client_id(CL, "BTC-EUR", since).await, Err(VenueError::Rejected(_))));
}
