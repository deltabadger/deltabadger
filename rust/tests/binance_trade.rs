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
/// One reply of a sequence (`[r200(a), r200(b)]`: a, then b).
fn r200(body: Value) -> Value { json!({ "status": 200, "body": body }) }
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
    // A read's 401 carries its status (Exchange#invalid_key_error?), with the text Rejected carries.
    assert_eq!(v.balance("USDT", true).await, Err(VenueError::Unauthorized(vec!["Invalid API-key, IP, or permissions for action.".into()])));
}

// ---- placement: Exchanges::Binance#set_market_order / #set_limit_order, Exchange#ambiguous_placement_error?

const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str) -> NewOrder {
    NewOrder { pair: "BTCUSDT".into(), side: deltabadger::venue::Side::Buy, kind, volume: volume.into(), quote_volume: quote, cl_ord_id: CL.into(),
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
        // Provably never sent: honeymaker's Failure, a definitive refusal (Rails writes a failed row and does not retry).
        (json!([{ "network": "pre_send", "message": "refused" }]), Err(VenueError::Rejected(vec!["refused".into()]))),
        (json!([{ "network": "permanent", "message": "certificate" }]), Err(VenueError::Rejected(vec!["certificate".into()]))),
    ];
    for (reply, want) in cases {
        let (_, v) = venue(json!({ "POST /api/v3/order": reply }));
        assert_eq!(v.add_order(&order(OrderKind::Market, true, "60")).await, want, "{reply}");
    }
    assert!(deltabadger::engine::placement::safe_rejection(&[m1021.into()]) && deltabadger::engine::placement::safe_rejection(&[ahead.into()]));
}

// ---- polls: Exchanges::Binance#get_orders / #get_order, #parse_order_data, the fill rebuild from myTrades

#[test]
fn every_recorded_rails_trade_parse_and_aggregate_is_reproduced() {
    use deltabadger::venue::binance::{aggregate, parse_trade};
    let cases = vectors()["binance_fills"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 10_000);
    let pairs = pairs();
    let (mut base, mut quote, mut fallback) = (0, 0, 0);
    for c in &cases {
        let symbol = c["symbol"].as_str().unwrap();
        let trades: Vec<_> = c["trades"].as_array().unwrap().iter().map(|t| {
            let raw = json!({ "symbol": symbol, "id": t[0], "orderId": 28457, "price": t[1], "qty": t[2], "quoteQty": t[3], "commission": t[4],
                              "commissionAsset": t[5], "isBuyer": t[6], "isMaker": t[7] });
            if t[5] == "BTC" && symbol == "BTCUSDT" || t[5] == "ETH" { base += 1; } else if t[5] == "USDT" || t[5] == "BTC" { quote += 1; }
            if symbol == "RNDRUSDT" { fallback += 1; }
            parse_trade(&raw, &pairs).unwrap()
        }).collect();
        for (t, want) in trades.iter().zip(c["net"].as_array().unwrap()) {
            assert_eq!((t.amount_exec.to_s_f(), t.quote_amount_exec.to_s_f()), (want[0].as_str().unwrap().to_string(), want[1].as_str().unwrap().to_string()), "{c}");
        }
        let fill = aggregate(&trades, pairs.by_symbol.get(symbol)).unwrap();
        let want = &c["aggregate"];
        assert_eq!((fill.price.to_s_f(), fill.amount_exec.to_s_f(), fill.quote_amount_exec.to_s_f()),
                   (want[0].as_str().unwrap().into(), want[1].as_str().unwrap().into(), want[2].as_str().unwrap().into()), "{c}");
    }
    assert!(base > 1000 && quote > 1000 && fallback > 1000, "every netting branch is exercised: {base} {quote} {fallback}");
    assert_eq!(cases[0]["aggregate"][0], "1.0", "1.00, 1.01, 1.00 at two decimals: the weighted price rounds half up to the tick");
}

fn raw_order(id: i64, status: &str, kind: &str, executed: &str, cumulative: &str) -> Value {
    json!({ "symbol": "BTCUSDT", "orderId": id, "orderListId": -1, "clientOrderId": "x", "price": if kind == "LIMIT" { "64150.00000000" } else { "0.00000000" },
            "origQty": if kind == "LIMIT" { "0.00093000" } else { "0.00000000" }, "executedQty": executed, "cummulativeQuoteQty": cumulative,
            "status": status, "timeInForce": "GTC", "type": kind, "side": "BUY", "origQuoteOrderQty": if kind == "LIMIT" { "0.00000000" } else { "60.00000000" } })
}
fn trade(id: i64, order: i64, price: &str, qty: &str, commission: &str, asset: &str) -> Value {
    json!({ "symbol": "BTCUSDT", "id": id, "orderId": order, "orderListId": -1, "price": price, "qty": qty,
            "quoteQty": (&bd(price) * &bd(qty)).to_s_f(), "commission": commission, "commissionAsset": asset, "time": 1789871400750u64,
            "isBuyer": true, "isMaker": false, "isBestMatch": true })
}

#[tokio::test(flavor = "current_thread")]
async fn statuses_map_as_order_status_map_and_an_unexecuted_order_keeps_its_own_figures() {
    use deltabadger::venue::OrderStatus::*;
    for (status, want) in [("NEW", Open), ("PENDING_NEW", Open), ("PARTIALLY_FILLED", Open), ("FILLED", Closed), ("CANCELED", Cancelled),
                           ("EXPIRED", Cancelled), ("EXPIRED_IN_MATCH", Cancelled), ("REJECTED", Failed), ("PENDING_CANCEL", Unknown), ("MYSTERY", Unknown)] {
        let (t, v) = venue(json!({ "GET /api/v3/allOrders": ok(json!([raw_order(28457, status, "LIMIT", "0.00000000", "0.00000000")])) }));
        let got = v.orders(&["BTCUSDT-28457".into()]).await.unwrap();
        assert_eq!((got.len(), got[0].status, got[0].txid.as_str()), (1, want, "BTCUSDT-28457"), "{status}");
        assert_eq!((got[0].price.clone(), got[0].amount.clone(), got[0].quote_amount.clone(), got[0].amount_exec.clone(), got[0].quote_amount_exec.clone(), got[0].limit),
                   (Some(bd("64150")), Some(bd("0.00093")), None, BigDec::zero(), Some(BigDec::zero()), true), "{status}");
        assert_eq!(t.requests().len(), 1, "no trades are read for an order that executed nothing");
        assert_eq!(sent(&t.requests()[0]), pairs_of(&[("limit", "1000"), ("orderId", "28457"), ("recvWindow", "60000"), ("symbol", "BTCUSDT")]));
    }
    // A historical order: nothing executed and a negative cumulative quote, which Rails reads as nil (never written).
    let (_, v) = venue(json!({ "GET /api/v3/allOrders": ok(json!([raw_order(1, "CANCELED", "MARKET", "0.00000000", "-1.00000000")])) }));
    let got = v.orders(&["BTCUSDT-1".into()]).await.unwrap();
    assert_eq!((got[0].quote_amount_exec.clone(), got[0].amount_exec.clone(), got[0].quote_amount.clone(), got[0].price.clone()), (None, BigDec::zero(), Some(bd("60")), None));
    let (_, v) = venue(json!({ "GET /api/v3/allOrders": ok(json!([raw_order(1, "NEW", "STOP_LOSS", "0", "0")])) }));
    assert_eq!(v.orders(&["BTCUSDT-1".into()]).await, Err(VenueError::Rejected(vec!["Unknown Binance order type: STOP_LOSS".into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn an_executed_order_is_rebuilt_from_all_its_trades_net_of_commission() {
    let (t, v) = venue(json!({
        "GET /api/v3/allOrders": ok(json!([raw_order(5, "FILLED", "MARKET", "0.00100000", "64.00000000"), raw_order(6, "NEW", "LIMIT", "0", "0"),
                                           raw_order(7, "PARTIALLY_FILLED", "LIMIT", "0.00040000", "25.66000000")])),
        "GET /api/v3/myTrades": [r200(json!([trade(1, 5, "64000.00", "0.0006", "0.0384", "USDT"), trade(2, 5, "64001.00", "0.0004", "0.0001", "BNB")])),
                                 r200(json!([trade(3, 7, "64150.00", "0.0004", "0.0000004", "BTC")]))] }));
    let got = v.orders(&["BTCUSDT-5".into(), "BTCUSDT-7".into(), "BTCUSDT-99".into()]).await.unwrap();
    let ids: Vec<&str> = got.iter().map(|o| o.txid.as_str()).collect();
    assert_eq!(ids, ["BTCUSDT-5", "BTCUSDT-7"], "an id allOrders did not return is simply absent; one not asked for is ignored");
    assert_eq!((got[0].price.clone(), got[0].amount_exec.clone(), got[0].quote_amount_exec.clone()), (Some(bd("64000.4")), bd("0.001"), Some(bd("64.0388"))),
               "quote commission added to what was spent, BNB ignored, price weighted by qty and rounded to the tick");
    assert_eq!((got[1].price.clone(), got[1].amount_exec.clone(), got[1].quote_amount_exec.clone()), (Some(bd("64150")), bd("0.0003996"), Some(bd("25.66"))),
               "base commission taken from what was received");
    let trades: Vec<_> = t.requests().into_iter().filter(|r| r.path == "/api/v3/myTrades").collect();
    assert_eq!(trades.len(), 2, "one walk per executed order");
    assert_eq!(sent(&trades[0]), pairs_of(&[("fromId", "0"), ("limit", "1000"), ("orderId", "5"), ("recvWindow", "60000"), ("symbol", "BTCUSDT")]));
}

#[tokio::test(flavor = "current_thread")]
async fn a_fill_whose_trades_do_not_add_up_yet_is_rails_failure_and_the_order_waits() {
    for page in [json!([]), json!([trade(1, 5, "64000.00", "0.0006", "0", "USDT")])] {
        let (_, v) = venue(json!({ "GET /api/v3/allOrders": ok(json!([raw_order(5, "FILLED", "MARKET", "0.00100000", "64.00000000")])), "GET /api/v3/myTrades": ok(page.clone()) }));
        assert_eq!(v.orders(&["BTCUSDT-5".into()]).await,
                   Err(VenueError::Rejected(vec!["Binance trades of order BTCUSDT-5 do not add up to its executed quantity yet".into()])), "{page}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn trades_are_paged_by_id_and_a_trade_served_twice_counts_once() {
    // A full page of 1000, then the next page from the highest id + 1; Binance repeats id 1000 (same millisecond) on it.
    let first: Vec<Value> = (1..=1000).map(|i| trade(i, 5, "64000.00", "0.000001", "0", "BNB")).collect();
    let second = json!([trade(1000, 5, "64000.00", "0.000001", "0", "BNB"), trade(1001, 5, "64010.00", "0.000001", "0", "BNB")]);
    let (t, v) = venue(json!({ "GET /api/v3/allOrders": ok(json!([raw_order(5, "FILLED", "MARKET", "0.00100100", "64.07")])),
                               "GET /api/v3/myTrades": [r200(json!(first)), r200(second)] }));
    let got = v.orders(&["BTCUSDT-5".into()]).await.unwrap();
    assert_eq!(got[0].amount_exec, bd("0.001001"), "1001 distinct trades");
    let from: Vec<String> = t.requests().iter().filter(|r| r.path == "/api/v3/myTrades")
        .map(|r| r.query.iter().find(|(k, _)| *k == "fromId").unwrap().1.clone()).collect();
    assert_eq!(from, ["0", "1001"]);
    let full: Vec<Value> = (1..=1000).map(|i| trade(i, 5, "1", "1", "0", "BNB")).collect();
    let (_, v) = venue(json!({ "GET /api/v3/allOrders": ok(json!([raw_order(5, "FILLED", "MARKET", "1", "1")])), "GET /api/v3/myTrades": ok(json!(full)) }));
    assert_eq!(v.orders(&["BTCUSDT-5".into()]).await, Err(VenueError::Rejected(vec!["Too many pages of Binance BTCUSDT trades".into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn all_orders_walks_forward_from_the_smallest_id_past_each_full_page() {
    let page: Vec<Value> = (100..1100).map(|i| raw_order(i, "CANCELED", "LIMIT", "0", "0")).collect();
    let (t, v) = venue(json!({ "GET /api/v3/allOrders": [r200(json!(page)), r200(json!([raw_order(5000, "NEW", "LIMIT", "0", "0")]))] }));
    let got = v.orders(&["BTCUSDT-5000".into(), "BTCUSDT-150".into(), "ETHBTC-150".into()]).await;
    let walks: Vec<Vec<(String, String)>> = t.requests().iter().map(sent).collect();
    assert_eq!(walks[0], pairs_of(&[("limit", "1000"), ("orderId", "150"), ("recvWindow", "60000"), ("symbol", "BTCUSDT")]));
    assert_eq!(walks[1], pairs_of(&[("limit", "1000"), ("orderId", "5000"), ("recvWindow", "60000"), ("symbol", "BTCUSDT")]),
               "the next page starts at max(smallest id still wanted, last id + 1)");
    assert_eq!(walks[2], pairs_of(&[("limit", "1000"), ("orderId", "150"), ("recvWindow", "60000"), ("symbol", "ETHBTC")]), "each symbol on its own");
    let ids: Vec<String> = got.unwrap().into_iter().map(|o| o.txid).collect();
    assert_eq!(ids, ["BTCUSDT-150", "BTCUSDT-5000"], "the ETHBTC walk is answered with BTCUSDT's last page (the script repeats it): nothing of it");
}

#[tokio::test(flavor = "current_thread")]
async fn the_follow_up_asks_get_order_by_symbol_and_order_id() {
    let (t, v) = venue(json!({ "GET /api/v3/order": ok(raw_order(28457, "NEW", "LIMIT", "0", "0")) }));
    let (got, skipped) = v.order_identified("BTCUSDT-28457", |_, _, _| Ok(true)).await.unwrap();
    assert_eq!((got.len(), got[0].txid.as_str(), skipped.len()), (1, "BTCUSDT-28457", 0));
    assert_eq!(sent(&t.requests()[0]), pairs_of(&[("orderId", "28457"), ("recvWindow", "60000"), ("symbol", "BTCUSDT")]));
    let (_, v) = venue(json!({ "GET /api/v3/order": fail(400, json!({ "code": -2013, "msg": "Order does not exist." })) }));
    assert_eq!(v.order_identified("BTCUSDT-1", |_, _, _| Ok(true)).await, Err(VenueError::Rejected(vec!["Order does not exist.".into()])));
    let (_, v) = venue(json!({ "GET /api/v3/order": fail(418, json!({ "code": -1003, "msg": "Way too much request weight used; IP banned until 1789871400750." })) }));
    let Err(VenueError::Rejected(e)) = v.order_identified("BTCUSDT-1", |_, _, _| Ok(true)).await else { panic!() };
    assert!(deltabadger::engine::venue_rules::BINANCE.is_throttle(&e), "a 418 on a read waits as a throttle: {e:?}");
}

// ---- recovery by client order id, and the factory

#[tokio::test(flavor = "current_thread")]
async fn a_lookup_by_client_id_names_the_intents_symbol_and_only_its_own_answer_counts() {
    use deltabadger::venue::binance::ABSENCE_UNPROVEN;
    let since = "2026-09-01T09:00:00Z".parse().unwrap();
    let mut found = raw_order(28457, "FILLED", "MARKET", "0.00100000", "64.00000000");
    found["clientOrderId"] = json!(CL);
    let (t, v) = venue(json!({ "GET /api/v3/order": ok(found.clone()), "GET /api/v3/myTrades": ok(json!([trade(1, 28457, "64000.00", "0.001", "0.064", "USDT")])) }));
    let got = v.order_by_client_id(CL, "BTCUSDT", since).await.unwrap().unwrap();
    assert_eq!((got.txid.as_str(), got.quote_amount_exec.clone()), ("BTCUSDT-28457", Some(bd("64.064"))), "read as #get_order reads it");
    assert_eq!(sent(&t.requests()[0]), pairs_of(&[("origClientOrderId", CL), ("recvWindow", "60000"), ("symbol", "BTCUSDT")]));
    for (body, why) in [(json!({ "symbol": "ETHUSDT", "clientOrderId": CL }), "another symbol"), (json!({ "symbol": "BTCUSDT", "clientOrderId": "other" }), "another client id")] {
        let mut other = found.clone();
        for (k, val) in body.as_object().unwrap() { other[k] = val.clone(); }
        let (_, v) = venue(json!({ "GET /api/v3/order": ok(other) }));
        assert!(matches!(v.order_by_client_id(CL, "BTCUSDT", since).await, Err(VenueError::Rejected(_))), "{why}: an incomplete answer");
    }
    let (_, v) = venue(json!({ "GET /api/v3/order": fail(400, json!({ "code": -2013, "msg": "Order does not exist." })) }));
    assert_eq!(v.order_by_client_id(CL, "BTCUSDT", since).await, Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())), "absence is not proven yet");
    let (_, v) = venue(json!({ "GET /api/v3/order": [{ "network": "post_send", "message": "read timeout" }] }));
    assert!(v.order_by_client_id(CL, "BTCUSDT", since).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn the_factory_builds_binance_through_its_proxy_with_the_exchanges_tickers_per_build() {
    use deltabadger::venue::live::LiveVenue;
    use deltabadger::venue::{alpaca::LiveFactory, VenueFactory};
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let book = r#"{"symbol":"BTCUSDT","bidPrice":"1","bidQty":"1","askPrice":"3","askQty":"1"}"#;
    let (a, b, direct) = (MockServer::start().await, MockServer::start().await, MockServer::start().await);
    for server in [&a, &b] { Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string(book)).expect(1).mount(server).await; }
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string(book)).expect(0).mount(&direct).await;
    let dir = tempfile::tempdir().unwrap();
    let c = rusqlite::Connection::open(dir.path().join("primary.sqlite3")).unwrap();
    c.execute_batch("CREATE TABLE app_configs (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT NOT NULL, value TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
        CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT);
        CREATE TABLE assets (id INTEGER PRIMARY KEY, symbol TEXT);
        CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, ticker TEXT, base TEXT, quote TEXT, price_decimals INTEGER, available BOOLEAN, base_asset_id INTEGER, quote_asset_id INTEGER);
        INSERT INTO exchanges VALUES (7, 'Exchanges::Binance');").unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap());
    let factory = LiveFactory::for_install(&c, &cipher);
    // Realistic key material: the transport scrubs every credential value out of each answer it reads.
    let creds = Some(deltabadger::crypto::Credentials { redaction_values: vec![], key: "vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A".into(),
                                                        secret: "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j".into(), passphrase: None });
    let now = chrono::Utc::now();
    for proxy in [&a, &b] {
        deltabadger::app_config::set(&c, &cipher, "proxy_binance", &proxy.uri(), now).unwrap();
        let LiveVenue::Binance(v) = factory.for_bot("Exchanges::Binance", creds.clone()) else { panic!("a Binance bot gets the Binance venue") };
        assert!(std::ptr::eq(v.rules(), &deltabadger::engine::venue_rules::BINANCE));
        assert_eq!(v.placement_identity().unwrap()["host"], "https://api.binance.com");
        // The proxy set at this build carries the request; the host itself is never reached directly.
        assert_eq!(v.with_base(&direct.uri()).price(&btc_usdt(), PriceSide::Ask).await, Ok(bd("3")), "the proxy read at this build");
    }
    for server in [&a, &b, &direct] { server.verify().await; }
    // No install to read proxy_binance from: refused, never direct.
    let LiveVenue::Binance(v) = LiveFactory::new().for_bot("Exchanges::Binance", creds) else { panic!() };
    match v.price(&btc_usdt(), PriceSide::Ask).await { Err(VenueError::Transient(m)) => assert!(m.contains("proxy"), "{m}"), other => panic!("{other:?}") }
}

#[test]
fn the_binance_log_line_is_labelled_binance() {
    let t = http::ReqwestTransport::binance(http::client(), "KEY".into(), "SECRET".into());
    assert_eq!(t.failure_line(&http::HttpResponse { status: 400, body: r#"{"code":-2010,"msg":"x"}"#.into() }), "[Binance] HTTP 400; venue response diagnostic omitted");
}

// ---- 6a: the sell side (Exchanges::Binance#get_bid_price, #market_sell / #limit_sell: `side.to_s.upcase`)

#[tokio::test(flavor = "current_thread")]
async fn a_sell_posts_side_sell_and_the_bid_is_the_books_bid_price() {
    let cases = [
        (order(OrderKind::Market, false, "0.00123"), vec![("newClientOrderId", CL), ("quantity", "0.00123"), ("recvWindow", "5000"),
            ("selfTradePreventionMode", "EXPIRE_MAKER"), ("side", "SELL"), ("symbol", "BTCUSDT"), ("type", "MARKET")]),
        (order(OrderKind::Limit { price: "64150.5".into() }, false, "0.00093"), vec![("newClientOrderId", CL), ("price", "64150.5"),
            ("quantity", "0.00093"), ("recvWindow", "5000"), ("side", "SELL"), ("symbol", "BTCUSDT"), ("timeInForce", "GTC"), ("type", "LIMIT")]),
    ];
    for (o, want) in cases {
        let (t, v) = venue(json!({ "POST /api/v3/order": accepted() }));
        v.add_order(&NewOrder { side: deltabadger::venue::Side::Sell, ..o }).await.unwrap();
        assert_eq!(sent(&t.requests()[0]), pairs_of(&want));
    }
    let book = |bid: &str| ok(json!({ "symbol": "BTCUSDT", "bidPrice": bid, "bidQty": "1.5", "askPrice": "64000.11", "askQty": "0.2" }));
    let (_, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": book("64000.10") }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Ok(bd("64000.10")));
    let (_, v) = venue(json!({ "GET /api/v3/ticker/bookTicker": book("0.00000000") }));
    assert_eq!(v.price(&btc_usdt(), PriceSide::Bid).await, Err(VenueError::Rejected(vec!["Wrong bid price for BTCUSDT: 0.0".into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn a_sellers_free_base_is_the_row_of_the_tickers_base_code_and_never_reads_unreadable_as_zero() {
    let body = json!({ "balances": [{ "asset": "BTC", "free": "0.5", "locked": "0.1" }, { "asset": "USDT", "free": "12", "locked": "0" }] });
    let (_, v) = venue(json!({ "GET /api/v3/account": ok(body) }));
    assert_eq!(v.free_base_balance(&btc_usdt()).await, Ok(bd("0.5")), "free, never free + locked");
    let no_catalog = || -> Result<deltabadger::sync::balances::Catalog, VenueError> { panic!("Binance names assets by code") };
    assert_eq!(v.settled_quote_balance(&btc_usdt(), no_catalog).await, Ok(bd("12")), "the quote's own row");
    let (_, v) = venue(json!({ "GET /api/v3/account": ok(json!({ "balances": [{ "asset": "USDT", "free": "12", "locked": "0" }] })) }));
    assert_eq!(v.free_base_balance(&btc_usdt()).await, Ok(BigDec::zero()), "absent is no holdings");
    let (_, v) = venue(json!({ "GET /api/v3/account": ok(json!({ "balances": [{ "asset": "BTC", "free": "garbage", "locked": "0" }] })) }));
    assert!(matches!(v.free_base_balance(&btc_usdt()).await, Err(VenueError::Ambiguous(_))), "unreadable is never 0");
    let (_, v) = venue(json!({ "GET /api/v3/account": ok(json!({ "nope": [] })) }));
    assert!(v.free_base_balance(&btc_usdt()).await.is_err(), "a body without balances is unreadable");
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_deletes_the_order_by_symbol_and_order_id() {
    let (t, v) = venue(json!({ "DELETE /api/v3/order": ok(json!({ "symbol": "BTCUSDT", "orderId": 28457, "status": "CANCELED" })) }));
    assert_eq!(v.cancel("BTCUSDT-28457").await, Ok(()));
    let r = &t.requests()[0];
    assert_eq!((r.method, r.path.as_str()), ("DELETE", "/api/v3/order"));
    assert_eq!(sent(r), pairs_of(&[("orderId", "28457"), ("recvWindow", "5000"), ("symbol", "BTCUSDT")]));
    let (_, v) = venue(json!({ "DELETE /api/v3/order": fail(400, json!({ "code": -2011, "msg": "Unknown order sent." })) }));
    assert_eq!(v.cancel("BTCUSDT-28457").await, Err(VenueError::Rejected(vec!["Unknown order sent.".into()])));
}
