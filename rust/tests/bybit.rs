//! Bybit V5 safety and Rails parity (plan VB A8).
use deltabadger::venue::bybit::{self, BybitVenue, URL};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::{NewOrder, OrderKind, Side, Venue, VenueError};
use serde_json::{json, Value};
const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order() -> NewOrder {
    NewOrder { pair: "BTCUSDT".into(), side: Side::Buy, kind: OrderKind::Market, volume: "60.0".into(), quote_volume: true,
        cl_ord_id: CL.into(), deadline: "2026-09-01T10:00:10Z".parse().unwrap(), day: false }
}
fn venue(script: Value) -> (ScriptedTransport, BybitVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), BybitVenue::new(t, URL))
}
#[tokio::test]
async fn placement_requires_a_proven_result_and_preserves_the_uuid() {
    for body in [json!({}), json!(null), json!({"retCode":null}), json!({"retCode":0,"result":{}}), json!({"retCode":0,"result":{"orderId":"  "}}),
                 json!({"retCode":0,"result":{"orderId":{}}}), json!({"retCode":0,"result":{"orderId":false}}), json!({"retCode":0,"result":{"orderId":[]}}),
                 json!({"retCode":10000,"retMsg":"Server Timeout"}), json!({"retCode":10016,"retMsg":"Server error."})] {
        let (_, v) = venue(json!({"POST /v5/order/create":[{"status":200,"body":body}]}));
        assert!(matches!(v.add_order(&order()).await, Err(VenueError::Ambiguous(_))), "{body}");
    }
    let (t, v) = venue(json!({"POST /v5/order/create":[{"status":200,"body":{"retCode":0,"result":{"orderId":"raw-id"}}}]}));
    assert_eq!(v.add_order(&order()).await.unwrap(), "raw-id");
    let body = t.requests()[0].body.clone().unwrap();
    let (_, numeric) = venue(json!({"POST /v5/order/create":[{"status":200,"body":{"retCode":0,"result":{"orderId":123}}}]}));
    assert_eq!(numeric.add_order(&order()).await.unwrap(), "123");
    assert_eq!(CL.len(), 36);
    assert_eq!(body, json!({"category":"spot","symbol":"BTCUSDT","side":"Buy","orderType":"Market","qty":"60.0","marketUnit":"quoteCoin","orderLinkId":CL}));
    let (_, v) = venue(json!({"POST /v5/order/create":[{"status":200,"body":{"retCode":170131,"retMsg":"Insufficient balance"}}]}));
    assert_eq!(v.add_order(&order()).await, Err(VenueError::Rejected(vec!["Insufficient balance".into()])));
}
#[tokio::test]
async fn an_unknown_placement_time_never_proves_an_order_missing() {
    let (t, v) = venue(json!({"GET /v5/order/realtime":[{"status":200,"body":{"retCode":0,"result":{"list":[]}}}]}));
    assert!(matches!(v.orders(&["old-id".into()]).await, Err(VenueError::Rejected(_))));
    assert_eq!(t.requests().len(), 1);
}
#[test]
fn balances_use_wallet_less_locked_and_borrow_with_legacy_fallback() {
    for (coin, want) in [(json!({"walletBalance":"10","locked":"3","spotBorrow":"2","availableToWithdraw":"99"}), "5.0"),
                         (json!({"walletBalance":"1","locked":"3"}), "0.0"),
                         (json!({"walletBalance":"","availableToWithdraw":"2"}), "2.0"),
                         (json!({"free":"4"}), "4.0"), (json!({}), "0.0")] {
        assert_eq!(bybit::free_balance(&coin).unwrap().to_s_f(), want);
    }
}

use deltabadger::venue::http::HttpRequest;
use deltabadger::venue::OrderStatus;
use deltabadger::ruby::BigDec;
fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/bybit_signing.json")).unwrap() }
fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/bybit_vectors.json")).unwrap() }
fn lower(h: &[(String, String)]) -> Vec<(String, String)> { let mut h: Vec<_> = h.iter().map(|(k,v)| (k.to_ascii_lowercase(),v.clone())).collect(); h.sort(); h }
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

/// Honeymaker::Clients::Bybit#signed_headers, byte for byte under a frozen #timestamp_ms: GETs with and without a query, an
/// escaped one, POSTs (the JSON body as sent, nils compacted), and the public reads (no signature).
#[test]
fn every_recorded_honeymaker_request_is_reproduced() {
    let cases = fixture()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 28);
    for c in cases {
        let method: &'static str = match c["method"].as_str().unwrap() { "GET" => "GET", "POST" => "POST", m => panic!("{m}") };
        let query: Vec<(&'static str, String)> = c["query"].as_str().map(|q| {
            reqwest::Url::parse(&format!("http://x/?{q}")).unwrap().query_pairs().map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned())).collect()
        }).unwrap_or_default();
        let body = c["body"].as_str().filter(|b| !b.is_empty()).map(|b| serde_json::from_str::<Value>(b).unwrap());
        let r = HttpRequest { method, base: URL.into(), path: c["path"].as_str().unwrap().into(), query, body, not_after: None };
        assert_eq!(deltabadger::venue::binance::build_query(&r.query), c["query"].as_str().unwrap_or(""), "Faraday::Utils.build_query: {c}");
        assert_eq!(r.body.as_ref().map(Value::to_string).as_deref(), c["body"].as_str().filter(|b| !b.is_empty()), "the body honeymaker sends: {c}");
        let h = bybit::headers(&r, c["api_key"].as_str().unwrap(), c["api_secret"].as_str().unwrap(), c["timestamp"].as_i64().unwrap());
        let want = lower(&c["headers"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect::<Vec<_>>());
        assert_eq!(lower(&h), want, "{c}");
    }
}

/// #parse_order_data recorded over 10,000 generated V5 order answers.
#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let v = vectors();
    let cases = v["bybit_parsed"].as_array().unwrap();
    assert_eq!(cases.len(), 10_000);
    let s = |o: &Option<BigDec>| o.as_ref().map(BigDec::to_s_f);
    for c in cases {
        let id = c["raw"]["orderId"].as_str().unwrap().to_string();
        let got = bybit::parse_order(&id, &c["raw"]).unwrap();
        let status = match got.status { OrderStatus::Open => "open", OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", OrderStatus::Unknown => "unknown", OrderStatus::Failed => "failed" };
        assert_eq!(status, c["status"], "{c}");
        assert_eq!(s(&got.price).as_deref(), c["price"].as_str(), "price {c}");
        assert_eq!((s(&got.amount).as_deref(), s(&got.quote_amount).as_deref()), (c["amount"].as_str(), c["quote_amount"].as_str()), "{c}");
        assert_eq!((got.amount_exec.to_s_f(), s(&got.quote_amount_exec)), (c["amount_exec"].as_str().unwrap().to_string(), c["quote_amount_exec"].as_str().map(str::to_string)), "{c}");
        assert_eq!((got.sell, got.limit), (c["side"] == "sell", c["order_type"] == "limit_order"), "{c}");
    }
}


mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::amount::{self, OrderPlan, Origin, Sizing, TxKind};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::venue_rules::BYBIT;
fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn btc_usdt() -> Ticker {
    Ticker { id: 1, ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
             exchange_name: "Bybit".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 6, quote_decimals: 8, price_decimals: 2,
             minimum_base_size: bd("0"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true }
}
/// The ticker of a recorded case's precision shape.
fn shaped(v: &Value, index: &Value) -> Ticker {
    let shape = &v["bybit_tickers"][index.as_u64().unwrap() as usize];
    let int = |k: &str| shape[k].as_str().unwrap().parse::<i64>().unwrap();
    Ticker { base_decimals: int("base_decimals"), quote_decimals: int("quote_decimals"), price_decimals: int("price_decimals"),
             minimum_base_size: bd(shape["minimum_base_size"].as_str().unwrap()), minimum_quote_size: bd(shape["minimum_quote_size"].as_str().unwrap()), ..btc_usdt() }
}

/// What Rails handed honeymaker's #create_order, as the adapter's POST body names it (no orderLinkId).
fn want_body(w: &Value) -> Value {
    let mut want = json!({"category":w["category"],"symbol":w["symbol"],"side":w["side"],"orderType":w["order_type"],"qty":w["qty"]});
    for (wire, key) in [("price","price"),("timeInForce","time_in_force"),("marketUnit","market_unit")] { if let Some(v) = w.get(key) { want[wire] = v.clone(); } }
    want
}
async fn sent_body(order: &NewOrder) -> Result<Value, VenueError> {
    let (t,v) = venue(json!({"POST /v5/order/create":[{"status":200,"body":{"retCode":0,"result":{"orderId":"123"}}}]}));
    assert_eq!(v.add_order(order).await?, "123");
    let mut sent = t.requests()[0].body.clone().unwrap();
    assert_eq!(sent["orderLinkId"], json!(CL)); sent.as_object_mut().unwrap().shift_remove("orderLinkId"); Ok(sent)
}

/// Bot::OrderSetter's sizing under Exchanges::Bybit#minimum_amount_logic (market :base_or_quote, limit
/// :base_and_quote_in_base) and #set_market_order / #set_limit_order's wire, through amount::size, limit_refusal, to_order
/// (`WireFormat::Bybit`: a market order keeps its unit) and the adapter's POST body.
#[tokio::test(flavor = "current_thread")]
async fn every_recorded_rails_sizing_and_wire_is_reproduced() {
    let (_d, o, s) = common::install();
    use common::seed::{self, BotSpec};
    let market = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let limit = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)))).unwrap();
    let v = vectors();
    let cases = v["bybit_sizing"].as_array().unwrap();
    assert_eq!(cases.len(), 5_129);
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let mut refused = 0;
    for c in cases {
        let bot = if c["order_type"] == "limit_order" { &limit } else { &market };
        let t = shaped(&v, &c["ticker"]);
        let sizing = amount::size(bot, &t, &bd(c["x"].as_str().unwrap()), &bd(c["last_or_ask"].as_str().unwrap()), &BYBIT).unwrap();
        let (plan, below) = match sizing { Sizing::Place(p) => (p, false), Sizing::BelowMinimum(p) => (p, true), other => panic!("{c}: {other:?}") };
        assert_eq!((plan.price.to_s_f(), plan.amount.to_s_f()), (c["price"].as_str().unwrap().to_string(), c["amount"].as_str().unwrap().to_string()), "{c}");
        assert_eq!((plan.quote_type, below), (c["amount_type"] == "quote", c["below_minimum"].as_bool().unwrap()), "{c}");
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() { refused += 1; continue; }
        let order = plan.to_order(CL.into(), deadline, BYBIT.wire).unwrap();
        assert_eq!(sent_body(&order).await.unwrap(), want_body(&c["wire"]), "{c}");
    }
    assert!(refused > 100, "the limit refusal is exercised: {refused}");
}

/// Both sides and units, with quote limits converted at the adjusted limit price before the intent.
#[tokio::test(flavor = "current_thread")]
async fn both_sides_of_every_recorded_rails_order_are_reproduced() {
    let v = vectors();
    let cases = v["bybit_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 4_000);
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let mut invalid_price = 0;
    for c in cases {
        let t = shaped(&v, &c["ticker"]);
        let (side, limit, quote) = (if c["side"] == "sell" { Side::Sell } else { Side::Buy }, c["order_type"] == "limit", c["amount_type"] == "quote");
        let raw = bd(c["amount"].as_str().unwrap());
        let price = bd(c[if limit { "price" } else if side == Side::Sell { "bid" } else { "ask" }].as_str().unwrap());
        let volume = raw.floor(u8::try_from(if quote { t.quote_decimals } else { t.base_decimals }).unwrap());
        let plan = OrderPlan { ticker: t.clone(), side, limit, price, amount: raw.clone(), quote_amount: raw, quote_type: quote, volume, kind: TxKind::Regular,
                               origin: if side == Side::Buy { Origin::QuoteBudget } else { Origin::Base }, wire_price: None };
        let order = plan.to_order(CL.into(), deadline, BYBIT.wire).unwrap();
        match c["refused"].as_str() {
            Some(text) => {
                assert_eq!(sent_body(&order).await, Err(VenueError::Rejected(vec![text.into()])), "{c}");
                invalid_price += 1;
            }
            None => assert_eq!(sent_body(&order).await.unwrap(), want_body(&c["wire"]), "{c}"),
        }
    }
    assert!(invalid_price > 100, "invalid prices refused");
}


fn raw_order(id: &str) -> Value { json!({"orderId":id,"orderLinkId":CL,"symbol":"BTCUSDT","orderType":"Market","side":"Buy","qty":"0.001","avgPrice":"60000","price":"0","cumExecQty":"0.001","cumExecValue":"60","orderStatus":"Filled"}) }
fn reply(rows: Value) -> Value { json!({"status":200,"body":{"retCode":0,"result":{"list":rows}}}) }
#[tokio::test]
async fn the_factory_builds_bybit_without_reading_transactions() {
    use deltabadger::venue::{alpaca::LiveFactory, VenueFactory};
    let dir = tempfile::tempdir().unwrap();
    let c = rusqlite::Connection::open(dir.path().join("primary.sqlite3")).unwrap();
    c.execute_batch("CREATE TABLE app_configs (key TEXT, value TEXT);
        CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT);
        CREATE TABLE assets (id INTEGER PRIMARY KEY, symbol TEXT);
        CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, ticker TEXT, base TEXT, quote TEXT, price_decimals INTEGER, available BOOLEAN, base_asset_id INTEGER, quote_asset_id INTEGER);
        INSERT INTO exchanges VALUES (9, 'Exchanges::Bybit');").unwrap();
    // There is deliberately no transactions table. An expired send bound proves the real transport was built,
    // without opening a socket; a factory that tried to read transactions would instead refuse the transport.
    let factory = LiveFactory::for_install(&c, &common::seed::cipher())
        .with_clock(std::sync::Arc::new(deltabadger::engine::FixedClock("2026-09-02T00:00:00Z".parse().unwrap())));
    let v = factory.for_bot_with_redaction(bybit::BYBIT, None, vec![]);
    match v.add_order(&order()).await {
        Err(VenueError::Rejected(m)) => assert!(m[0].starts_with("past the send bound"), "{m:?}"),
        other => panic!("expected an expired send bound, got {other:?}"),
    }
}

#[tokio::test]
async fn history_fallback_reads_only_the_missing_orders_current_placement_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.sqlite3");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT UNIQUE);
        CREATE TABLE transactions (id INTEGER PRIMARY KEY, exchange_id INTEGER, external_id TEXT, created_at TEXT);
        CREATE UNIQUE INDEX index_transactions_on_exchange_id_and_external_id ON transactions(exchange_id, external_id);
        INSERT INTO exchanges VALUES (9, 'Exchanges::Bybit'), (10, 'Exchanges::Other');").unwrap();
    let at: DateTime<Utc> = "2026-08-01T00:00:00Z".parse().unwrap();
    let (t, v) = venue(json!({"GET /v5/order/realtime":[reply(json!([]))],
        "GET /v5/order/history":[reply(json!([])), reply(json!([raw_order("old-id")]))]}));
    let v = v.with_history_db(path, at + chrono::Duration::days(40));
    // The placement is recorded after the handle is built. Neither another exchange's same id nor an unrelated
    // historical order's malformed timestamp may affect this order's fallback.
    c.execute_batch("INSERT INTO transactions VALUES (1, 10, 'old-id', 'unreadable'),
        (2, 9, 'unrelated-id', 'unreadable'), (3, 9, 'old-id', '2026-08-01 00:00:00');").unwrap();
    assert_eq!(v.orders(&["old-id".into()]).await.unwrap(), vec![bybit::parse_order("old-id", &raw_order("old-id")).unwrap()]);
    let requests = t.requests();
    assert_eq!(requests.len(), 3);
    for (n, r) in requests.iter().skip(1).enumerate() {
        assert_eq!(r.path, bybit::HISTORY_PATH);
        let start = at - chrono::Duration::hours(1) + chrono::Duration::days(7 * n as i64);
        let q = |key| r.query.iter().find(|(k,_)| *k == key).unwrap().1.as_str();
        assert_eq!(q("orderId"), "old-id");
        assert_eq!(q("startTime"), start.timestamp_millis().to_string());
        assert_eq!(q("endTime"), (start + chrono::Duration::days(7)).timestamp_millis().to_string());
    }
    assert_eq!(v.orders(&["unknown-id".into()]).await,
        Err(VenueError::Rejected(vec!["Bybit order unknown-id is not in realtime and its placement time is unknown".into()])));
    assert_eq!(t.requests().len(), 4, "an unknown placement cannot start a history walk");
}

#[tokio::test]
async fn realtime_and_client_recovery_do_not_read_placement_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sqlite3");
    let at: DateTime<Utc> = "2026-08-01T00:00:00Z".parse().unwrap();
    let (t, v) = venue(json!({"GET /v5/order/realtime":[reply(json!([raw_order("old-id")])), reply(json!([]))],
        "GET /v5/order/history":[reply(json!([raw_order("old-id")]))]}));
    let v = v.with_history_db(path.clone(), at + chrono::Duration::days(40));
    assert_eq!(v.orders(&["old-id".into()]).await.unwrap()[0].txid, "old-id");
    assert_eq!(t.requests().len(), 1);
    assert_eq!(v.order_by_client_id(CL, "BTCUSDT", at).await.unwrap().unwrap().txid, "old-id");
    assert_eq!(t.requests().len(), 3, "client recovery already has the placement time");
    assert!(matches!(v.orders(&["old-id".into()]).await, Err(VenueError::Transient(_))));
    assert_eq!(t.requests().len(), 4, "an unreadable placement prevents the history request");
    assert!(!path.exists(), "the read must not create a missing instance database");
}

#[tokio::test]
async fn history_walk_and_client_recovery_find_raw_ids_and_cancel_uses_the_symbol() {
    let at: DateTime<Utc> = "2026-08-01T00:00:00Z".parse().unwrap();
    let end = at + chrono::Duration::days(40);
    let (t,v) = venue(json!({"GET /v5/order/realtime":[{"status":200,"body":{"retCode":110001,"retMsg":"Order not found"}}], "GET /v5/order/history":[reply(json!([])),reply(json!([raw_order("old-id")]))],
        "POST /v5/order/cancel":[{"status":200,"body":{"retCode":0,"result":{}}}]}));
    let v = v.with_history([("old-id".into(),at)].into(), end);
    let orders=v.orders(&["old-id".into()]).await.unwrap();
    assert_eq!(orders[0].txid,"old-id");
    let requests=t.requests();
    assert_eq!(requests.len(),3);
    let q=|n:usize,key| requests[n].query.iter().find(|(k,_)| *k==key).unwrap().1.parse::<i64>().unwrap();
    assert_eq!(q(1,"endTime")-q(1,"startTime"),7*86_400_000);
    assert_eq!(q(1,"endTime"),q(2,"startTime"));
    assert_eq!(v.order_by_client_id(CL,"BTCUSDT",at).await.unwrap().unwrap().txid,"old-id");
    v.cancel("old-id").await.unwrap();
    assert_eq!(t.requests().last().unwrap().body,Some(json!({"category":"spot","symbol":"BTCUSDT","orderId":"old-id"})));
    let (_,missing)=venue(json!({"GET /v5/order/realtime":[reply(json!([]))],"GET /v5/order/history":[reply(json!([]))]}));
    let missing=missing.with_history([("old-id".into(),at)].into(),end);
    assert!(missing.orders(&["old-id".into()]).await.unwrap().is_empty());
    assert!(missing.order_by_client_id(CL,"BTCUSDT",at).await.unwrap().is_none());
    let (t,failed)=venue(json!({"GET /v5/order/realtime":[{"status":400,"body":{"retCode":110001,"retMsg":"Order not found"}}]}));
    let failed=failed.with_history([("old-id".into(),at)].into(),end);
    assert_eq!(failed.orders(&["old-id".into()]).await,Err(VenueError::Rejected(vec!["Order not found".into()])));
    assert_eq!(t.requests().len(),1,"an HTTP failure stays a failed read");
    let mut unusable=raw_order("old-id"); unusable["orderId"]=json!({});
    let (_,invalid)=venue(json!({"GET /v5/order/realtime":[reply(json!([unusable]))]}));
    assert!(matches!(invalid.order_by_client_id(CL,"BTCUSDT",at).await,Err(VenueError::Ambiguous(_))));
}
#[tokio::test]
async fn lost_or_unreadable_placement_replies_are_never_rejected() {
    for status in [200, 400, 401, 429, 500, 503] {
        for body in [json!("broken"),json!({}),json!({"retCode":0})] {
            let (_,v)=venue(json!({"POST /v5/order/create":[{"status":status,"body":body}]}));
            assert!(matches!(v.add_order(&order()).await,Err(VenueError::Ambiguous(_))),"{status} {body}");
        }
    }
    for network in ["post_send","pre_send"] {
        let (_,v)=venue(json!({"POST /v5/order/create":[{"network":network,"message":"transport failed"}]}));
        let result=v.add_order(&order()).await;
        if network=="post_send" { assert!(matches!(result,Err(VenueError::Ambiguous(_)))); }
        // The shared transport contract proves that NotSent could not create an order.
        else { assert_eq!(result,Err(VenueError::Rejected(vec!["transport failed".into()]))); }
    }
}
#[test]
fn rules_match_the_recorded_rails_error_patterns() {
    let f=fixture();
    for (kind,patterns) in BYBIT.kinds {
        let expected: Vec<&str>=f["errors"][kind].as_array().into_iter().flatten().map(|v|v.as_str().unwrap()).collect();
        assert_eq!(patterns,expected);
    }
    let transient: Vec<&str> = f["network_transient"].as_array().unwrap().iter().chain(f["errors"]["transient"].as_array().unwrap()).map(|v| v.as_str().unwrap()).collect();
    assert_eq!(BYBIT.transient_text, transient);
    for message in f["placement_safe"].as_array().unwrap() {
        assert!(deltabadger::engine::placement::safe_rejection(&[message.as_str().unwrap().into()]));
    }
    assert!(BYBIT.reports_missing && BYBIT.young_missing_fails && !BYBIT.absence_provable);
}

#[tokio::test]
async fn prices_and_trading_balances_use_the_rails_v5_fields() {
    use deltabadger::venue::{PriceSide, binance::Pairs};
    let (t,v)=venue(json!({
        "GET /v5/market/tickers":[reply(json!([{"symbol":"BTCUSDT","lastPrice":"64000.5"}]))],
        "GET /v5/market/orderbook":[{"status":200,"body":{"retCode":0,"result":{"b":[["64000","2"]],"a":[["64001.37","1"]]}}}],
        "GET /v5/account/wallet-balance":[reply(json!([{"coin":[{"coin":"USDT","walletBalance":"100","locked":"10","spotBorrow":"5","availableToWithdraw":"0"},{"coin":"BTC","walletBalance":"1","locked":"0.1"}]}]))]
    }));
    let mut pairs=Pairs::default(); pairs.assets.insert("USDT".into(),"USDT".into());
    let v=v.with_pairs(pairs);
    assert_eq!(v.price(&btc_usdt(),PriceSide::Last).await.unwrap(),bd("64000.5"));
    assert_eq!(v.price(&btc_usdt(),PriceSide::Bid).await.unwrap(),bd("64000"));
    assert_eq!(v.price(&btc_usdt(),PriceSide::Ask).await.unwrap(),bd("64001.37"));
    assert_eq!(v.balance("USDT",true).await.unwrap(),bd("85"));
    assert_eq!(v.free_base_balance(&btc_usdt()).await.unwrap(),bd("0.9"));
    assert_eq!(v.tickers_prices(&[]).await.unwrap(),vec![("BTCUSDT".into(),json!("64000.5"))]);
    assert_eq!(t.requests()[3].query,vec![("accountType","UNIFIED".into())]);
}
