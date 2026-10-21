//! The Bitget trading adapter (venue::bitget, plan VB slice A4) over recorded bodies: its signer against honeymaker's own
//! requests (the `Prehash` scheme KuCoin and Bitvavo share, base64 with the passphrase as it is), its rules against
//! Exchanges::Bitget::ERRORS, #parse_order_data, the sizing wire and both sides of #set_market_order / #set_limit_order
//! recorded from Rails, and its reads, placement, polls, the recovery by clientOid (it can only FIND an order) and cancel.
//! Rails-free.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::amount::{self, OrderPlan, Origin, Sizing, TxKind};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::placement;
use deltabadger::engine::venue_rules::BITGET;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::Pairs;
use deltabadger::venue::bitget::{self, BitgetVenue, URL};
use deltabadger::venue::http::{HttpRequest, ScriptedTransport};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Side, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/bitget_signing.json")).unwrap() }
fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/bitget_vectors.json")).unwrap() }
fn btc_usdt() -> Ticker {
    Ticker { id: 1, ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
             exchange_name: "Bitget".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 6, quote_decimals: 8, price_decimals: 2,
             minimum_base_size: bd("0"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true }
}
fn venue(script: Value) -> (ScriptedTransport, BitgetVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    let mut p = Pairs::default();
    for code in ["BTC", "USDT"] { p.assets.insert(code.into(), code.into()); }
    (t.clone(), BitgetVenue::new(t, URL).with_pairs(p))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn fail(status: u16, body: Value) -> Value { json!([{ "status": status, "body": body }]) }
fn envelope(data: Value) -> Value { json!({ "code": "00000", "msg": "success", "requestTime": 1, "data": data }) }
fn err(code: &str, msg: &str) -> Value { json!({ "code": code, "msg": msg, "requestTime": 1, "data": null }) }
fn lower(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
    h.sort();
    h
}
const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str, side: Side) -> NewOrder {
    NewOrder { pair: "BTCUSDT".into(), side, kind, volume: volume.into(), quote_volume: quote, cl_ord_id: CL.into(),
               deadline: "2026-09-01T10:00:10Z".parse().unwrap(), day: false }
}

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

/// Honeymaker::Clients::Bitget#signed_headers, byte for byte under a frozen #timestamp_ms: GETs with and without a query, an
/// escaped one, POSTs (the JSON body as sent, nils compacted), and the public reads (no signature).
#[test]
fn every_recorded_honeymaker_request_is_reproduced() {
    let cases = fixture()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 22);
    for c in cases {
        let method: &'static str = match c["method"].as_str().unwrap() { "GET" => "GET", "POST" => "POST", m => panic!("{m}") };
        let query: Vec<(&'static str, String)> = c["query"].as_str().map(|q| {
            reqwest::Url::parse(&format!("http://x/?{q}")).unwrap().query_pairs().map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned())).collect()
        }).unwrap_or_default();
        let body = c["body"].as_str().filter(|b| !b.is_empty()).map(|b| serde_json::from_str::<Value>(b).unwrap());
        let r = HttpRequest { method, base: URL.into(), path: c["path"].as_str().unwrap().into(), query, body, not_after: None };
        assert_eq!(deltabadger::venue::binance::build_query(&r.query), c["query"].as_str().unwrap_or(""), "Faraday::Utils.build_query: {c}");
        assert_eq!(r.body.as_ref().map(Value::to_string).as_deref(), c["body"].as_str().filter(|b| !b.is_empty()), "the body honeymaker sends: {c}");
        let h = bitget::headers(&r, c["api_key"].as_str().unwrap(), c["api_secret"].as_str().unwrap(), c["passphrase"].as_str().unwrap(), c["timestamp"].as_i64().unwrap());
        let want = lower(&c["headers"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect::<Vec<_>>());
        assert_eq!(lower(&h), want, "{c}");
    }
}

/// engine::venue_rules::BITGET is Exchanges::Bitget::ERRORS, and Exchange#ambiguous_placement_error? as it reads them.
#[test]
fn the_rules_are_rails_errors() {
    let f = fixture();
    let list = |v: &Value| v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    for (kind, patterns) in BITGET.kinds {
        assert_eq!(patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(), list(&f["errors"][kind]), "{kind}");
    }
    let mut transient = list(&f["network_transient"]);
    transient.extend(list(&f["errors"]["transient"]));
    assert_eq!(BITGET.transient_text.iter().map(|s| s.to_string()).collect::<Vec<_>>(), transient, "Exchange#transient_error?");
    let safe = list(&f["errors"]["placement_safe_transient"]);
    for m in list(&f["errors"]["transient"]) {
        let body = format!(r#"{{"code":"40000","msg":"{m}"}}"#);
        assert_eq!(BITGET.add_outcome_unknown(std::slice::from_ref(&body)), !safe.contains(&m), "{m}");
    }
    for m in safe.iter().chain(list(&f["placement_safe"]).iter()) { assert!(placement::safe_rejection(std::slice::from_ref(m)), "{m}"); }
    assert!(BITGET.reports_missing && BITGET.young_missing_fails && !BITGET.absence_provable && BITGET.follow_up_strict);
}

/// #parse_order_data (G2), recorded over 10,000 generated orderInfo answers.
#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let v = vectors();
    let cases = v["bitget_parsed"].as_array().unwrap();
    assert_eq!(cases.len(), 10_000);
    let s = |o: &Option<BigDec>| o.as_ref().map(BigDec::to_s_f);
    for c in cases {
        let id = format!("{}-{}", c["raw"]["symbol"].as_str().unwrap(), c["raw"]["orderId"].as_str().unwrap());
        let got = bitget::parse_order(&id, &c["raw"]).unwrap();
        let status = match got.status { OrderStatus::Open => "open", OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", OrderStatus::Unknown => "unknown", OrderStatus::Failed => "failed" };
        assert_eq!(status, c["status"], "{c}");
        assert_eq!(s(&got.price).as_deref(), c["price"].as_str(), "price {c}");
        assert_eq!((s(&got.amount).as_deref(), s(&got.quote_amount).as_deref()), (c["amount"].as_str(), c["quote_amount"].as_str()), "{c}");
        assert_eq!((got.amount_exec.to_s_f(), s(&got.quote_amount_exec)), (c["amount_exec"].as_str().unwrap().to_string(), c["quote_amount_exec"].as_str().map(str::to_string)), "{c}");
        assert_eq!((got.sell, got.limit), (c["side"] == "sell", c["order_type"] == "limit_order"), "{c}");
    }
}

/// The ticker of a recorded case's precision shape.
fn shaped(v: &Value, index: &Value) -> Ticker {
    let shape = &v["bitget_tickers"][index.as_u64().unwrap() as usize];
    let int = |k: &str| shape[k].as_str().unwrap().parse::<i64>().unwrap();
    Ticker { base_decimals: int("base_decimals"), quote_decimals: int("quote_decimals"), price_decimals: int("price_decimals"),
             minimum_base_size: bd(shape["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(shape["minimum_quote_size"].as_str().unwrap()), ..btc_usdt() }
}

/// What Rails handed honeymaker's #place_order, as the adapter's POST body names it (no clientOid).
fn want_body(w: &Value) -> Value {
    let mut want = json!({ "symbol": w["symbol"], "side": w["side"], "orderType": w["order_type"], "size": w["size"], "force": w["force"] });
    if let Some(p) = w.get("price") { want["price"] = p.clone(); }
    want
}

async fn sent_body(order: &NewOrder) -> Result<Value, VenueError> {
    let (t, venue) = venue(json!({ "POST /api/v2/spot/trade/place-order": ok(envelope(json!({ "orderId": "1234567890", "clientOid": CL }))) }));
    assert_eq!(venue.add_order(order).await?, "BTCUSDT-1234567890");
    let mut sent = t.requests()[0].body.clone().unwrap();
    assert_eq!(sent["clientOid"].take(), json!(CL));
    sent.as_object_mut().unwrap().shift_remove("clientOid");
    Ok(sent)
}

/// Bot::OrderSetter's sizing under Exchanges::Bitget#minimum_amount_logic (market :base_or_quote, limit
/// :base_and_quote_in_base) and #set_market_order / #set_limit_order's wire, through amount::size, limit_refusal, to_order
/// (`WireFormat::Bitget`: a base market buy turned into quote coins at the ask it was sized at) and the adapter's POST body.
#[tokio::test(flavor = "current_thread")]
async fn every_recorded_rails_sizing_and_wire_is_reproduced() {
    let (_d, o, s) = common::install();
    use common::seed::{self, BotSpec};
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let v = vectors();
    let cases = v["bitget_sizing"].as_array().unwrap();
    assert_eq!(cases.len(), 10_637);
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut converted, mut refused) = (0, 0);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let t = shaped(&v, &c["ticker"]);
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &BITGET).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!((plan.price.to_s_f(), plan.amount.to_s_f()), (c["price"].as_str().unwrap().to_string(), c["amount"].as_str().unwrap().to_string()), "{c}");
        assert_eq!((plan.quote_type, below), (c["amount_type"] == "quote", c["below_minimum"].as_bool().unwrap()), "{c}");
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        if !plan.limit && !plan.quote_type { converted += 1; }
        let order = plan.to_order(CL.into(), deadline, BITGET.wire).unwrap();
        assert_eq!(sent_body(&order).await.unwrap(), want_body(&c["wire"]), "{c}");
    }
    assert!(converted > 100 && refused > 100, "the base market buy and the limit refusal are exercised: {converted}, {refused}");
}

/// Both sides of #set_market_order (G1) and #set_limit_order (#532) over 4,000 generated orders: the plan the engine
/// persists (its unfloored amount, the touch it was sized at), through to_order and the adapter. Listed divergence
/// D-BITGET-ZERO-TOUCH: a quote-sized sell converted at a bid that floors to zero is not sent; Rails sends size "Infinity".
/// (Rails never sizes a sell in quote: Bot::OrderSetter and Bots::Signal::OrderSetter#venue_takes? both sell in base.)
#[tokio::test(flavor = "current_thread")]
async fn both_sides_of_every_recorded_rails_order_are_reproduced() {
    let v = vectors();
    let cases = v["bitget_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 4_000);
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut zero_touch, mut invalid_price) = (0, 0);
    for c in cases {
        let t = shaped(&v, &c["ticker"]);
        let (side, limit, quote) = (if c["side"] == "sell" { Side::Sell } else { Side::Buy }, c["order_type"] == "limit", c["amount_type"] == "quote");
        let raw = bd(c["amount"].as_str().unwrap());
        let price = bd(c[if limit { "price" } else if side == Side::Sell { "bid" } else { "ask" }].as_str().unwrap());
        let volume = raw.floor(u8::try_from(if quote { t.quote_decimals } else { t.base_decimals }).unwrap());
        let plan = OrderPlan { ticker: t.clone(), side, limit, price, amount: raw.clone(), quote_amount: raw, quote_type: quote, volume, kind: TxKind::Regular,
                               origin: if side == Side::Buy { Origin::QuoteBudget } else { Origin::Base }, wire_price: None };
        if c["wire"]["size"] == "Infinity" {
            assert!(plan.to_order(CL.into(), deadline, BITGET.wire).is_err(), "D-BITGET-ZERO-TOUCH {c}");
            zero_touch += 1;
            continue;
        }
        let order = plan.to_order(CL.into(), deadline, BITGET.wire).unwrap();
        match c["refused"].as_str() {
            Some(text) => {
                assert_eq!(sent_body(&order).await, Err(VenueError::Rejected(vec![text.into()])), "{c}");
                invalid_price += 1;
            }
            None => assert_eq!(sent_body(&order).await.unwrap(), want_body(&c["wire"]), "{c}"),
        }
    }
    assert_eq!((zero_touch, invalid_price), (28, 194), "a recorded case was added or dropped");
}

// ---- reads

#[tokio::test(flavor = "current_thread")]
async fn prices_and_balances_read_as_rails_reads_them() {
    let book = envelope(json!({ "asks": [["64001.37", "0.2"]], "bids": [["64000", "1.5"]], "ts": "1" }));
    let assets = envelope(json!([{ "coin": "BTC", "available": "0.5", "frozen": "0.1", "locked": "0" }, { "coin": "USDT", "available": "100.25", "frozen": "0", "locked": "0" }]));
    let (t, v) = venue(json!({ "GET /api/v2/spot/market/orderbook": ok(book), "GET /api/v2/spot/market/tickers": ok(envelope(json!([{ "symbol": "BTCUSDT", "lastPr": "64000.5" }]))),
                               "GET /api/v2/spot/account/assets": ok(assets) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Ok(bd("64001.37")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Ok(bd("64000")));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Ok(bd("64000.5")));
    assert_eq!(v.balance("USDT", true).await, Ok(bd("100.25")));
    assert_eq!(v.free_base_balance(&btc_usdt()).await, Ok(bd("0.5")));
    let keys: Vec<String> = t.requests().iter().map(deltabadger::venue::http::request_key).collect();
    assert_eq!(keys[..3], ["GET /api/v2/spot/market/orderbook?limit=1&symbol=BTCUSDT", "GET /api/v2/spot/market/orderbook?limit=1&symbol=BTCUSDT",
                           "GET /api/v2/spot/market/tickers?symbol=BTCUSDT"]);
    // A zero price raises "Wrong … price"; no book is Rails' Failure; an error envelope on the assets is "Failed to get …".
    let (_, v) = venue(json!({ "GET /api/v2/spot/market/orderbook": ok(envelope(json!({ "asks": [["0", "1"]], "bids": [["0", "1"]] }))),
                               "GET /api/v2/spot/market/tickers": ok(envelope(json!([]))) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Err(VenueError::Rejected(vec!["Wrong bid price for BTCUSDT: 0.0".into()])));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["Failed to get Bitget last price for BTCUSDT".into()])));
    let (_, v) = venue(json!({ "GET /api/v2/spot/market/orderbook": ok(envelope(Value::Null)) }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Failed to get Bitget order book for BTCUSDT".into()])));
    let (_, v) = venue(json!({ "GET /api/v2/spot/account/assets": ok(json!({ "code": "40015", "msg": "System is abnormal", "data": [] })) }));
    assert_eq!(v.balance("USDT", true).await, Err(VenueError::Rejected(vec!["Failed to get Bitget balances: 40015 System is abnormal".into()])));
    let (_, v) = venue(json!({ "GET /api/v2/spot/account/assets": fail(400, err("40037", "Apikey does not exist")) }));
    let Err(VenueError::Rejected(e)) = v.balance("USDT", true).await else { panic!() };
    assert!(BITGET.is_invalid_key(&e), "{e:?}");
    // Every row of the whole market, a zero kept as Rails keeps it; a price that is no decimal left out.
    let (_, v) = venue(json!({ "GET /api/v2/spot/market/tickers": ok(envelope(json!([{ "symbol": "BTCUSDT", "lastPr": "64000" }, { "symbol": "ETHUSDT", "lastPr": "0" },
                                                                                     { "symbol": "XUSDT", "lastPr": "12x" }]))) }));
    assert_eq!(v.tickers_prices(&[]).await, Ok(vec![("BTCUSDT".into(), json!("64000.0")), ("ETHUSDT".into(), json!("0.0"))]));
    let (_, v) = venue(json!({ "GET /api/v2/spot/market/tickers": ok(json!({ "code": "00000" })) }));
    assert_eq!(v.tickers_prices(&[]).await, Err(VenueError::Rejected(vec!["Failed to get Bitget tickers".into()])));
}

// ---- placement

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_as_rails_maps_them() {
    let place = |reply: Value| async move { venue(json!({ "POST /api/v2/spot/trade/place-order": reply })).1.add_order(&order(OrderKind::Market, true, "60", Side::Buy)).await };
    assert_eq!(place(ok(envelope(json!({ "orderId": "1001", "clientOid": CL })))).await, Ok("BTCUSDT-1001".into()));
    assert_eq!(place(ok(envelope(json!({})))).await, Err(VenueError::Ambiguous(bitget::NO_ORDER_ID.into())));
    // An answer without a code proves nothing about the order: the intent stays.
    for silent in [json!({}), json!(null), json!({ "code": null }), json!({ "data": { "orderId": "1001" } })] {
        assert_eq!(place(ok(silent)).await, Err(VenueError::Ambiguous(bitget::NO_CODE.into())));
    }
    let Err(VenueError::Rejected(e)) = place(fail(400, err("43012", "Insufficient balance"))).await else { panic!() };
    assert_eq!(BITGET.failure_kind(&e), Some("insufficient_funds"));
    // An error envelope inside HTTP 200 is honeymaker's api_error text.
    assert_eq!(place(ok(err("43012", "Insufficient balance"))).await, Err(VenueError::Rejected(vec!["Bitget API error 43012: Insufficient balance".into()])));
    let Err(VenueError::Rejected(e)) = place(fail(400, err("40008", "Request timestamp expired"))).await else { panic!() };
    assert!(placement::safe_rejection(&e), "40008: no row, the run rescheduled");
    let Err(VenueError::Rejected(e)) = place(fail(400, err("40010", "Request timed out"))).await else { panic!() };
    assert!(BITGET.add_outcome_unknown(&e), "40010 may have placed the order");
    assert!(matches!(place(fail(503, json!("<html>upstream</html>"))).await, Err(VenueError::Ambiguous(_))));
    assert!(matches!(place(json!([{ "network": "post_send", "message": "Net::ReadTimeout" }])).await, Err(VenueError::Ambiguous(_))));
    assert!(matches!(place(json!([{ "network": "pre_send", "message": "refused" }])).await, Err(VenueError::Rejected(_))));
    // The body's fields in honeymaker's order, then the intent id as clientOid.
    let (t, v) = venue(json!({ "POST /api/v2/spot/trade/place-order": ok(envelope(json!({ "orderId": "o1" }))) }));
    v.add_order(&order(OrderKind::Limit { price: "63840.0".into() }, false, "0.000939", Side::Buy)).await.unwrap();
    assert_eq!(t.requests()[0].body.as_ref().unwrap().to_string(),
               format!(r#"{{"symbol":"BTCUSDT","side":"buy","orderType":"limit","size":"0.000939","price":"63840.0","force":"gtc","clientOid":"{CL}"}}"#));
    // A market order is sent in its side's unit only (WireFormat::Bitget converted it): anything else is not sent.
    assert!(matches!(v.add_order(&order(OrderKind::Market, false, "0.001", Side::Buy)).await, Err(VenueError::Transient(_))));
    assert!(matches!(v.add_order(&order(OrderKind::Market, true, "60", Side::Sell)).await, Err(VenueError::Transient(_))));
    assert_eq!(t.requests().len(), 1);
}

// ---- polls, cancel, recovery

#[tokio::test(flavor = "current_thread")]
async fn a_gone_order_is_missing_and_any_other_failure_fails_the_batch() {
    let filled = envelope(json!([{ "symbol": "BTCUSDT", "orderId": "7", "orderType": "market", "side": "buy", "status": "filled", "priceAvg": "64000.5",
                                    "size": "60", "baseVolume": "0.00093749", "quoteVolume": "59.9999" }]));
    let (t, v) = venue(json!({ "GET /api/v2/spot/trade/orderInfo?orderId=7": ok(filled), "GET /api/v2/spot/trade/orderInfo?orderId=8": fail(400, err("40109", "gone")),
                               "GET /api/v2/spot/trade/orderInfo?orderId=9": ok(envelope(json!([]))), "GET /api/v2/spot/trade/orderInfo?orderId=10": ok(err("43001", "x")) }));
    let ids: Vec<String> = ["BTCUSDT-7", "BTCUSDT-8", "BTCUSDT-9", "BTCUSDT-10"].map(String::from).to_vec();
    let got = v.orders(&ids).await.unwrap();
    assert_eq!(got.len(), 1, "40109, an empty list and 43001 are missing");
    assert_eq!((got[0].txid.as_str(), got[0].status, got[0].quote_amount.as_ref().map(BigDec::to_s_f), got[0].amount.clone()),
               ("BTCUSDT-7", OrderStatus::Closed, Some("60.0".to_string()), None), "a market buy's size is its quote amount (G2)");
    assert_eq!(t.requests().len(), 4);
    let (_, v) = venue(json!({ "GET /api/v2/spot/trade/orderInfo": fail(400, err("40015", "System is abnormal")) }));
    assert!(matches!(v.orders(&["BTCUSDT-1".into()]).await, Err(VenueError::Rejected(_))));
    let (_, v) = venue(json!({ "GET /api/v2/spot/trade/orderInfo": fail(400, err("40109", "gone")) }));
    assert_eq!(v.order_identified("BTCUSDT-1", |_, _, _| Ok(true)).await, Err(VenueError::Rejected(vec!["Bitget no longer knows order BTCUSDT-1".into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_posts_the_symbol_and_order_id_and_looks_a_legacy_id_up() {
    let (t, v) = venue(json!({ "POST /api/v2/spot/trade/cancel-order": ok(err("43001", "x")) }));
    assert_eq!(v.cancel("BTCUSDT-123").await, Ok(()), "any 2xx is the cancel, as Rails takes it");
    assert_eq!(t.requests()[0].body.as_ref().unwrap().to_string(), r#"{"symbol":"BTCUSDT","orderId":"123"}"#);
    let (t, v) = venue(json!({ "GET /api/v2/spot/trade/orderInfo": ok(envelope(json!([{ "symbol": "ETHUSDT", "orderId": "55" }]))),
                               "POST /api/v2/spot/trade/cancel-order": ok(envelope(json!({ "orderId": "55" }))) }));
    assert_eq!(v.cancel("55").await, Ok(()));
    assert_eq!(t.requests()[1].body.as_ref().unwrap().to_string(), r#"{"symbol":"ETHUSDT","orderId":"55"}"#);
}

/// Recovery (plan VB §1.5) asks for the clientOid the intent sent: a hit naming it and the symbol is the order; a gone or
/// empty answer is never absence.
#[tokio::test(flavor = "current_thread")]
async fn the_client_order_lookup_finds_an_order_and_never_proves_one_absent() {
    let since: DateTime<Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    let found = envelope(json!([{ "orderId": "99", "symbol": "BTCUSDT", "clientOid": CL, "status": "filled", "side": "buy", "orderType": "market",
                                   "size": "60", "priceAvg": "64000", "baseVolume": "0.0009375", "quoteVolume": "60" }]));
    let (t, v) = venue(json!({ "GET /api/v2/spot/trade/orderInfo": ok(found) }));
    let o = v.order_by_client_id(CL, "BTCUSDT", since).await.unwrap().unwrap();
    assert_eq!((o.txid.as_str(), o.status), ("BTCUSDT-99", OrderStatus::Closed));
    assert_eq!(deltabadger::venue::http::request_key(&t.requests()[0]), format!("GET /api/v2/spot/trade/orderInfo?clientOid={CL}"));
    for answer in [ok(envelope(json!([]))), fail(400, err("40109", "gone")), ok(envelope(json!([{ "orderId": "1", "symbol": "BTCUSDT", "clientOid": "another" }])))] {
        let (_, v) = venue(json!({ "GET /api/v2/spot/trade/orderInfo": answer }));
        assert_eq!(v.order_by_client_id(CL, "BTCUSDT", since).await, Ok(None));
    }
}

/// The exact quote cap (amount::guard_exact_cap) bounds what a base market buy on Bitget spends: it goes out in quote coins,
/// its unfloored base at the floored ask (WireFormat::Bitget), so the guard measures that quote, and a reduced order is a
/// quote order of what the cap leaves, never more.
#[test]
fn the_exact_cap_bounds_the_quote_a_base_market_buy_is_sent_for() {
    use common::seed::{self, BotSpec};
    let (_d, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Bitget', name = 'Bitget' WHERE id = ?1", [s.exchange_id]).unwrap();
    let capped = |limit: f64| {
        let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(limit))
            .transient("quote_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z"));
        model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &spec)).unwrap()
    };
    let t = Ticker { minimum_quote_size: bd("0"), ..btc_usdt() };
    let bot = capped(1000.0);
    let Sizing::Place(plan) = amount::size(&bot, &t, &bd("60"), &bd("64001.37"), &BITGET).unwrap() else { panic!() };
    assert!(!plan.quote_type, "no quote minimum: the base wins");
    let sent = |plan: &OrderPlan| plan.to_order(CL.into(), "2026-09-01T10:00:10Z".parse().unwrap(), BITGET.wire).unwrap();
    let amount::CapGuard::Place(same) = amount::guard_exact_cap(&o.primary, &bot, &plan).unwrap() else { panic!() };
    assert_eq!(sent(&same), sent(&plan), "under the cap: as sized");
    let whole = BigDec::parse(&sent(&plan).volume).unwrap();
    assert!(whole > bd("59.96"), "the unfloored base at the ask: {whole:?}");
    let bot = capped(59.97);
    let amount::CapGuard::Place(reduced) = amount::guard_exact_cap(&o.primary, &bot, &plan).unwrap() else { panic!() };
    let order = sent(&reduced);
    assert_eq!((order.quote_volume, order.volume.as_str()), (true, "59.97"), "a quote order of the cap");
}
