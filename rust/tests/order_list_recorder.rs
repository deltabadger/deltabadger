//! The order-list recorder (rust/examples/record_order_lists, PLAN-A28-fixtures.md): the scrubber on a hand-written body
//! per exchange, the consistent mapping, the leak check, the allow-list and the permission reads, and the record flow
//! over a scripted transport. No live call anywhere.
#[path = "../examples/record_order_lists/recorder.rs"]
mod recorder;
#[path = "../examples/record_order_lists/uta.rs"]
mod uta;

use deltabadger::venue::http::ScriptedTransport;
use recorder::*;
use serde_json::{json, Value};

const DAYS: i64 = 100;
fn scrubber() -> Scrubber { Scrubber::new(b"test salt".to_vec(), DAYS) }

fn endpoint(venue: &str, file: &str) -> &'static Endpoint { ENDPOINTS.iter().find(|e| e.venue == venue && e.file == file).unwrap() }

/// Same keys in the same order (Kraken's id keys excepted: same count), same JSON types, same string lengths.
fn same_shape(a: &Value, b: &Value, dynamic: bool) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(x.len(), y.len());
            for ((ka, va), (kb, vb)) in x.iter().zip(y) {
                if !dynamic { assert_eq!(ka, kb); }
                same_shape(va, vb, ka == "open" || ka == "closed");
            }
        }
        (Value::Array(x), Value::Array(y)) => { assert_eq!(x.len(), y.len()); for (p, q) in x.iter().zip(y) { same_shape(p, q, false); } }
        // A nested-JSON string (Bitget's feeDetail) is re-serialized: serde's number spelling may change its length.
        (Value::String(x), Value::String(y)) if x.starts_with('{') => assert!(y.starts_with('{')),
        (Value::String(x), Value::String(y)) => assert_eq!(x.len(), y.len(), "{x} vs {y}"),
        (Value::Number(x), Value::Number(y)) => assert_eq!(x.is_f64(), y.is_f64(), "{x} vs {y}"),
        _ => assert_eq!(std::mem::discriminant(a), std::mem::discriminant(b)),
    }
}

/// (venue, list endpoint, body, planted originals that must not survive, kept (pointer, value) pairs, shifted (pointer, expected))
type Body = (&'static str, &'static str, Value, Vec<&'static str>, Vec<(&'static str, Value)>, Vec<(&'static str, Value)>);
fn bodies() -> Vec<Body> {
    vec![
        ("kraken", "closed_orders",
         json!({"error": [], "result": {"closed": {
             "OQCLML-BW3P3-BUCMWZ": {"refid": null, "userref": 912345, "status": "closed", "opentm": 1700000000.1234, "closetm": 1700000100.5,
                 "descr": {"pair": "XBTUSD", "type": "buy", "ordertype": "limit", "price": "30123.4", "order": "buy 0.00123000 XBTUSD @ limit 30123.4"},
                 "vol": "0.00123000", "vol_exec": "0.00123000", "cost": "37.05", "fee": "0.09", "price": "30123.4", "misc": "", "oflags": "fciq"},
             "OB5VMB-B4U2U-DK2WRW": {"status": "canceled", "opentm": 1700000200, "descr": {"pair": "XBTUSD", "type": "sell"}, "vol": "0.00200000", "vol_exec": "0.00000000", "cost": "0.00000"}
         }, "count": 2}}),
         vec!["OQCLML-BW3P3-BUCMWZ", "OB5VMB-B4U2U-DK2WRW", "912345", "30123.4", "0.00123000", "37.05", "0.00200000"],
         vec![("/result/count", json!(2))], vec![]),
        ("kucoin", "hf_orders_done",
         json!({"code": "200000", "data": {"lastId": 2312411231i64, "items": [
             {"id": "68a3e81d2a090d00070575f7", "clientOid": "5c52e11203aa677f33e493fb", "symbol": "BTC-USDT", "side": "buy", "type": "limit",
              "price": "100000", "size": "0.00001", "dealSize": "0", "dealFunds": "0", "fee": "0", "feeCurrency": "USDT", "createdAt": 1755572253338i64, "active": false},
             {"id": "68a3e81d2a090d00070575f8", "clientOid": "5c52e11203aa677f33e493fc", "symbol": "BTC-USDT", "side": "sell", "type": "market",
              "price": "0", "size": "0.00042", "dealSize": "0.00042", "dealFunds": "41.88", "fee": "0.04188", "feeCurrency": "USDT", "createdAt": 1755572263485i64, "active": false}
         ]}}),
         vec!["68a3e81d2a090d00070575f7", "5c52e11203aa677f33e493fb", "2312411231", "0.00001", "0.00042", "41.88", "0.04188", "100000"],
         vec![("/code", json!("200000")), ("/data/items/0/symbol", json!("BTC-USDT")), ("/data/items/0/dealSize", json!("0"))],
         vec![("/data/items/0/createdAt", json!(1755572253338i64 - DAYS * 86_400_000))]),
        ("coinbase", "historical_batch",
         json!({"orders": [
             {"order_id": "0000-000000-000000", "client_order_id": "11111111-2222-3333-4444-555555555555", "product_id": "BTC-USD", "side": "BUY", "status": "FILLED",
              "filled_size": "0.0012", "average_filled_price": "65123.45", "total_fees": "0.47", "created_time": "2024-03-05T10:11:12.123456Z", "user_id": "a1b2c3d4-e5f6"},
             {"order_id": "abcdef12-3456-7890-abcd-ef1234567890", "client_order_id": "c0ffee00-1234", "product_id": "BTC-USD", "side": "SELL", "status": "CANCELLED",
              "filled_size": "0", "average_filled_price": "0", "total_fees": "0", "created_time": "2024-03-05T11:00:00Z", "user_id": "a1b2c3d4-e5f6"}
         ], "has_next": true, "cursor": "789123456", "sequence": "0"}),
         vec!["0000-000000-000000", "abcdef12-3456-7890-abcd-ef1234567890", "11111111-2222-3333-4444-555555555555", "a1b2c3d4-e5f6", "65123.45", "0.0012", "789123456"],
         vec![("/has_next", json!(true)), ("/orders/0/status", json!("FILLED"))],
         vec![("/orders/0/created_time", json!("2023-11-26T10:11:12.123456Z"))]),
        ("alpaca", "orders",
         json!([{"id": "61e69015-8549-4bfd-b9c3-01e75843f47d", "client_order_id": "eb9e2aaa-f71a-4f51-b5b4-52a6c565dad4", "symbol": "AAPL", "qty": "1.5",
                 "filled_qty": "1.5", "filled_avg_price": "189.32", "side": "buy", "status": "filled", "submitted_at": "2024-03-05T14:30:00.123456789Z", "replaced_by": null},
                {"id": "61e69015-8549-4bfd-b9c3-01e75843f47e", "client_order_id": "eb9e2aaa-f71a-4f51-b5b4-52a6c565dad5", "symbol": "AAPL", "qty": "2",
                 "filled_qty": "0", "filled_avg_price": null, "side": "buy", "status": "canceled", "submitted_at": "2024-03-05T15:00:00Z", "replaced_by": null}]),
         vec!["61e69015-8549-4bfd-b9c3-01e75843f47d", "eb9e2aaa-f71a-4f51-b5b4-52a6c565dad4", "189.32"],
         vec![("/1/filled_qty", json!("0")), ("/0/status", json!("filled"))],
         vec![("/0/submitted_at", json!("2023-11-26T14:30:00.123456789Z"))]),
        ("bitget", "history_orders",
         json!({"code": "00000", "msg": "success", "requestTime": 1695808949356i64, "data": [
             {"userId": "8672173294", "symbol": "BTCUSDT", "orderId": "1098394041234567890", "clientOid": "1098394041234567891", "price": "26000.5", "size": "0.001",
              "orderType": "limit", "side": "buy", "status": "filled", "baseVolume": "0.001", "quoteVolume": "26.0005", "enterPointSource": "API",
              "feeDetail": "{\"newFees\":{\"c\":0,\"d\":0,\"deduction\":false,\"r\":-0.0012,\"t\":-0.0012,\"totalDeductionFee\":0}}", "cancelReason": "", "cTime": "1695808949356", "uTime": "1695808949999"},
             {"userId": "8672173294", "symbol": "BTCUSDT", "orderId": "1098394041234567892", "clientOid": "x-1", "price": "1", "size": "1", "orderType": "limit",
              "side": "sell", "status": "cancelled", "baseVolume": "0", "quoteVolume": "0", "cancelReason": "", "cTime": "1695808950000", "uTime": "1695808950001"}
         ]}),
         vec!["8672173294", "1098394041234567890", "1098394041234567891", "26000.5", "26.0005", "0.0012"],
         vec![("/code", json!("00000")), ("/data/0/cancelReason", json!("")), ("/data/0/enterPointSource", json!("API"))],
         vec![("/data/0/cTime", json!((1695808949356i64 - DAYS * 86_400_000).to_string()))]),
        ("mexc", "all_orders",
         json!([{"symbol": "MXUSDT", "orderId": "C02__413849518122422272012", "orderListId": -1, "clientOrderId": "", "price": "3.0101", "origQty": "5.12",
                 "executedQty": "5.12", "cummulativeQuoteQty": "15.411712", "status": "FILLED", "type": "LIMIT", "side": "BUY", "time": 1700000000123i64, "updateTime": 1700000000999i64},
                {"symbol": "MXUSDT", "orderId": "C02__413849518122422272013", "orderListId": -1, "clientOrderId": "", "price": "2.5", "origQty": "4", "executedQty": "0",
                 "cummulativeQuoteQty": "0", "status": "CANCELED", "type": "LIMIT", "side": "SELL", "time": 1700000001000i64, "updateTime": null}]),
         vec!["C02__413849518122422272012", "3.0101", "15.411712"],
         vec![("/0/status", json!("FILLED")), ("/1/executedQty", json!("0"))],
         vec![("/0/time", json!(1700000000123i64 - DAYS * 86_400_000))]),
        ("gemini", "orders_history",
         json!([{"order_id": "73797746498585286", "id": "73797746498585286", "client_order_id": "my-order-77", "symbol": "btcusd", "exchange": "gemini",
                 "avg_execution_price": "64123.10", "side": "buy", "type": "exchange limit", "timestamp": "1700000000", "timestampms": 1700000000123i64,
                 "is_live": false, "is_cancelled": false, "executed_amount": "0.0015", "remaining_amount": "0", "original_amount": "0.0015", "price": "64200.00", "trades": []},
                {"order_id": "73797746498585299", "id": "73797746498585299", "symbol": "btcusd", "exchange": "gemini", "avg_execution_price": "0.00", "side": "sell",
                 "type": "exchange limit", "timestamp": "1700000100", "timestampms": 1700000100000i64, "is_live": false, "is_cancelled": true,
                 "executed_amount": "0", "remaining_amount": "0.002", "original_amount": "0.002", "price": "99000.00"}]),
         vec!["73797746498585286", "my-order-77", "64123.10", "0.0015", "64200.00"],
         vec![("/0/exchange", json!("gemini")), ("/1/avg_execution_price", json!("0.00")), ("/1/is_cancelled", json!(true))],
         vec![("/0/timestamp", json!((1700000000i64 - DAYS * 86_400).to_string()))]),
        ("bitvavo", "orders",
         json!([{"orderId": "1be6d0df-d5dc-4b53-a250-3376f3b393e6", "clientOrderId": "2be7d0df-d5dc-4b53-a250-3376f3b393e6", "market": "BTC-EUR", "created": 1542621155181i64,
                 "updated": 1542621155181i64, "status": "filled", "side": "buy", "orderType": "limit", "amount": "0.1", "amountRemaining": "0", "price": "3500.5",
                 "filledAmount": "0.1", "filledAmountQuote": "350.05", "feePaid": "0.87", "feeCurrency": "EUR", "fills": [{"id": "371c6bd3-d06d-4573-9f15-18697cd210e5", "amount": "0.1", "price": "3500.5"}]},
                {"orderId": "1be6d0df-d5dc-4b53-a250-3376f3b393e7", "market": "BTC-EUR", "created": 1542621155999i64, "status": "canceled", "side": "sell",
                 "orderType": "limit", "amount": "0.2", "amountRemaining": "0.2", "price": "9999", "filledAmount": "0", "fills": []}]),
         vec!["1be6d0df-d5dc-4b53-a250-3376f3b393e6", "371c6bd3-d06d-4573-9f15-18697cd210e5", "3500.5", "350.05"],
         vec![("/0/market", json!("BTC-EUR")), ("/0/feeCurrency", json!("EUR"))], vec![]),
        ("bingx", "history_orders",
         json!({"code": 0, "msg": "", "debugMsg": "", "data": {"orders": [
             {"symbol": "BTC-USDT", "orderId": 1731111111111111111i64, "price": "61234.5", "origQty": "0.0002", "executedQty": "0.0002", "cummulativeQuoteQty": "12.2469",
              "status": "FILLED", "type": "LIMIT", "side": "BUY", "time": 1700000000123i64, "updateTime": 1700000000999i64, "fee": -0.0122469, "clientOrderID": ""},
             {"symbol": "BTC-USDT", "orderId": 1731111111111111112i64, "price": "1", "origQty": "1", "executedQty": "0", "status": "CANCELED", "type": "LIMIT",
              "side": "SELL", "time": 1700000001000i64, "updateTime": 1700000001000i64, "fee": 0, "clientOrderID": ""}]}}),
         vec!["1731111111111111111", "61234.5", "12.2469", "0.0122469"],
         vec![("/code", json!(0)), ("/data/orders/1/fee", json!(0))], vec![]),
        ("bitrue", "all_orders",
         json!([{"symbol": "XRPUSDT", "orderId": 379521234, "clientOrderId": "abc123def456", "price": "0.6123", "origQty": "20.5", "executedQty": "20.5",
                 "cummulativeQuoteQty": "12.55215", "status": "FILLED", "type": "LIMIT", "side": "BUY", "time": 1700000000123i64, "updateTime": 1700000000999i64},
                {"symbol": "XRPUSDT", "orderId": 379521299, "clientOrderId": "abc123def457", "price": "0.9", "origQty": "11", "executedQty": "0.00000000",
                 "status": "CANCELED", "type": "LIMIT", "side": "SELL", "time": 1700000001000i64, "updateTime": 1700000001000i64}]),
         vec!["379521234", "abc123def456", "0.6123", "12.55215"],
         vec![("/1/executedQty", json!("0.00000000"))], vec![]),
        ("hyperliquid", "historical_orders",
         json!([{"order": {"coin": "BTC", "side": "B", "limitPx": "61234.0", "sz": "0.0", "oid": 9123456789i64, "timestamp": 1700000000123i64, "origSz": "0.00042",
                           "cloid": "0x1234567890abcdef1234567890abcdef", "orderType": "Limit", "tif": "Gtc"}, "status": "filled", "statusTimestamp": 1700000000999i64},
                {"order": {"coin": "BTC", "side": "A", "limitPx": "99000.0", "sz": "0.001", "oid": 9123456790i64, "timestamp": 1700000100000i64, "origSz": "0.001",
                           "cloid": null, "orderType": "Limit", "tif": "Gtc"}, "status": "canceled", "statusTimestamp": 1700000100500i64}]),
         vec!["9123456789", "61234.0", "0.00042", "0x1234567890abcdef1234567890abcdef"],
         vec![("/0/order/coin", json!("BTC")), ("/0/order/sz", json!("0.0")), ("/0/status", json!("filled"))],
         vec![("/0/statusTimestamp", json!(1700000000999i64 - DAYS * 86_400_000))]),
    ]
}

#[test]
fn every_exchange_body_is_scrubbed_with_its_shape_kept() {
    for (venue, file, body, planted, kept, shifted) in bodies() {
        let e = endpoint(venue, file);
        assert_eq!(orders(e, &body), Ok(2), "{venue}");
        let mut sc = scrubber();
        let out = sc.scrub(venue, file, body.clone()).unwrap();
        same_shape(&body, &out, false);
        assert_eq!(orders(e, &out), Ok(2), "{venue}: the envelope still reads as a success after the scrub");
        let text = out.to_string();
        for p in planted { assert!(!text.contains(p), "{venue}: {p} survived: {text}"); }
        for (ptr, v) in kept { assert_eq!(out.pointer(ptr), Some(&v), "{venue} {ptr}"); }
        for (ptr, v) in shifted { assert_eq!(out.pointer(ptr), Some(&v), "{venue} {ptr}"); }
        sc.leak_check(file, &out, &[]).unwrap();
        assert!(!sc.report.is_empty());
    }
}

#[test]
fn kraken_float_seconds_shift_as_text_and_keys_are_replaced() {
    let (_, _, body, ..) = bodies().remove(0);
    let out = scrubber().scrub("kraken", "closed_orders", body).unwrap();
    let closed = out["result"]["closed"].as_object().unwrap();
    assert!(!closed.contains_key("OQCLML-BW3P3-BUCMWZ"));
    let first = closed.values().next().unwrap();
    assert_eq!(first["opentm"].to_string(), format!("{}.1234", 1_700_000_000 - DAYS * 86_400));
    assert_eq!(first["descr"]["pair"], "XBTUSD");
    assert_ne!(first["descr"]["order"], "buy 0.00123000 XBTUSD @ limit 30123.4", "free text is never kept");
}

#[test]
fn the_same_id_maps_to_the_same_fake_across_files_and_as_a_map_key() {
    let mut sc = scrubber();
    let a = sc.scrub("kucoin", "a.json", json!({"code": "200000", "data": {"items": [{"id": "68a3e81d2a090d00070575f7", "price": "123456"}]}})).unwrap();
    let b = sc.scrub("kucoin", "b.json", json!({"code": "200000", "data": [{"orderId": "68a3e81d2a090d00070575f7", "amount": 123456}]})).unwrap();
    assert_eq!(a["data"]["items"][0]["id"], b["data"][0]["orderId"]);
    assert_eq!(a["data"]["items"][0]["price"].as_str().unwrap(), b["data"][0]["amount"].to_string(), "a number and a string with the same digits share a fake");
    let mut sc = scrubber();
    let v = sc.scrub("kraken", "ledger.json", json!({"error": [], "result": {"refid": "OQCLML-BW3P3-BUCMWZ"}})).unwrap();
    let k = sc.scrub("kraken", "closed.json", json!({"error": [], "result": {"closed": {"OQCLML-BW3P3-BUCMWZ": {"status": "closed"}}}})).unwrap();
    let key = k["result"]["closed"].as_object().unwrap().keys().next().unwrap().clone();
    assert_eq!(v["result"]["refid"], json!(key));
    assert!(sc.report.contains_key("closed.json /result/closed/*"), "{:?}", sc.report.keys());
}

#[test]
fn hex_ids_stay_hex_and_every_replacement_differs() {
    let mut sc = scrubber();
    let f = sc.fake("68a3e81d2a090d00070575f7", false, "/id").unwrap();
    assert!(f.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) && f.len() == 24, "{f}");
    for s in ["1", "a", "Z", "9"] { assert_ne!(sc.fake(s, false, "/x").unwrap(), s); }
    let cloid = sc.fake("0x1234567890abcdef1234567890abcdef", false, "/cloid").unwrap();
    assert!(cloid.starts_with("0x") && cloid.len() == 34 && cloid[2..].chars().all(|c| c.is_ascii_hexdigit()), "{cloid}");
    let out = sc.scrub("bitget", "x", json!({"code": "00000", "data": [{"cancelReason": "", "dash": "-", "one": "1", "zero": "0.000", "n0": 0, "id": "0000-0000", "small": "0.00123"}]})).unwrap();
    assert_ne!(out["data"][0]["id"], "0000-0000", "an all-zero id is no amount");
    assert!(out["data"][0]["small"].as_str().unwrap().starts_with("0.00"), "a decimal keeps its magnitude");
    assert_eq!(out["data"][0]["cancelReason"], "");
    assert_eq!(out["data"][0]["dash"], "-");
    assert_ne!(out["data"][0]["one"], "1");
    assert_eq!(out["data"][0]["zero"], "0.000");
    assert_eq!(out["data"][0]["n0"], 0);
}

#[test]
fn a_time_named_field_that_is_no_time_is_replaced_not_shifted() {
    let mut sc = scrubber();
    let out = sc.scrub("mexc", "x", json!([{"time": "OQCLML12345", "updateTime": 12345, "timestamp": "2024-01-01T00:00:00Z extra 12345"}])).unwrap();
    assert_ne!(out[0]["time"], "OQCLML12345");
    let shifted_wrongly = json!(12345 - DAYS * 86_400);
    assert_ne!(out[0]["updateTime"], shifted_wrongly);
    assert_ne!(out[0]["updateTime"], 12345);
    assert!(!out[0]["timestamp"].as_str().unwrap().contains("12345"));
}

#[test]
fn free_text_and_long_kept_values_are_replaced() {
    let mut sc = scrubber();
    let out = sc.scrub("bingx", "x", json!({"code": 0, "msg": "order 1731111111111111111 not found", "data": {"orders": [], "symbol": "ID-1234567"}})).unwrap();
    assert!(!out["msg"].as_str().unwrap().contains("1731111111111111111"));
    assert_ne!(out["data"]["symbol"], "ID-1234567", "a kept field fails the shape check on a 5+ digit run");
}

#[test]
fn numbers_keep_their_type_and_exponent() {
    let mut sc = scrubber();
    let out = sc.scrub("mexc", "x", json!([{"a": 1.5e-7, "b": 12345678901234567u64, "c": 2.50}])).unwrap();
    assert!(out[0]["a"].is_f64() && out[0]["a"].to_string().contains('e'), "{}", out[0]["a"]);
    assert_ne!(out[0]["a"], json!(1.5e-7));
    assert!(out[0]["b"].is_u64() && out[0]["b"].to_string().len() == 17);
    assert!(out[0]["c"].is_f64(), "trailing zeros are serde's canonical spelling, named in the plan");
    assert!(long_integer(r#"{"id": 123456789012345678901}"#));
    assert!(!long_integer(r#"{"id": "123456789012345678901", "n": 1234567890123456789}"#));
}

#[test]
fn an_unknown_dynamic_key_aborts() {
    let err = scrubber().scrub("bitget", "x", json!({"code": "00000", "data": {"1098394041234567890": {"price": "1"}}})).unwrap_err();
    assert!(err.contains("unknown dynamic key") && !err.contains("1098394041234567890"), "{err}");
}

#[test]
fn the_leak_check_catches_a_planted_original_and_a_key_without_naming_them() {
    let mut sc = scrubber();
    sc.scrub("alpaca", "x", json!([{"id": "61e69015-8549-4bfd-b9c3-01e75843f47d"}])).unwrap();
    let err = sc.leak_check("x", &json!([{"note": "was 61e69015-8549-4bfd-b9c3-01e75843f47d"}]), &[]).unwrap_err();
    assert!(err.contains("/[]/note") && !err.contains("61e69015"), "{err}");
    let err = sc.leak_check("x", &json!({"nested": "{\"k\":\"SECRETKEY123\"}"}), &["SECRETKEY123"]).unwrap_err();
    assert!(!err.contains("SECRETKEY123"), "{err}");
    let err = sc.leak_check("x", &json!({"SECRETKEY123": 1}), &["SECRETKEY123"]).unwrap_err();
    assert!(err.contains("(a key)") && !err.contains("SECRETKEY123"), "{err}");
}

#[test]
fn the_allow_list_refuses_every_write_and_allows_every_read() {
    for (venue, method, path) in [
        ("kraken", "POST", "/0/private/AddOrder"), ("kraken", "POST", "/0/private/Withdraw"), ("gemini", "POST", "/v1/order/new"),
        ("mexc", "POST", "/api/v3/order"), ("mexc", "DELETE", "/api/v3/openOrders"), ("hyperliquid", "POST", "/exchange"),
        ("coinbase", "POST", "/api/v3/brokerage/orders"), ("alpaca", "POST", "/v2/orders"), ("kucoin", "POST", "/api/v1/hf/orders"),
        ("bitvavo", "POST", "/v2/order"), ("binance", "GET", "/api/v3/allOrders"),
    ] { assert!(!allowed(venue, method, path), "{venue} {method} {path}"); }
    for e in ENDPOINTS {
        assert!(allowed(e.venue, e.method, e.path));
        assert!(e.method == "GET" || ["kraken", "gemini", "hyperliquid"].contains(&e.venue), "{} {}: a POST read outside the three venues whose reads are POSTs", e.venue, e.file);
    }
    assert_eq!(ENDPOINTS.len(), 23, "the PLAN-A28-3 §8 Q1 list, with A28-3c-3a's userFillsByTime");
}

#[test]
fn permission_reads_refuse_unless_read_only() {
    use Permission::*;
    for (venue, status, body, want) in [
        ("coinbase", 200, r#"{"can_view":true,"can_trade":false,"can_transfer":false}"#, ReadOnly),
        ("coinbase", 200, r#"{"can_view":true,"can_trade":true,"can_transfer":false}"#, CanTrade),
        ("coinbase", 200, r#"{"can_view":true,"can_trade":false}"#, Unreadable),
        ("coinbase", 200, r#"{"can_trade":null,"can_transfer":false}"#, Unreadable),
        ("coinbase", 401, r#"{"can_trade":false,"can_transfer":false}"#, Unreadable),
        ("gemini", 200, r#"{"isAuditor":true,"isFundManager":false,"isTrader":false}"#, ReadOnly),
        ("gemini", 200, r#"{"isAuditor":false,"isFundManager":true,"isTrader":false}"#, CanTrade),
        ("gemini", 200, r#"{"isAuditor":true,"isFundManager":"false","isTrader":false}"#, Unreadable),
        ("kucoin", 200, r#"{"code":"200000","data":{"permission":"General"}}"#, ReadOnly),
        ("kucoin", 200, r#"{"code":"200000","data":{"permission":"General,Spot"}}"#, CanTrade),
        ("kucoin", 200, r#"{"code":"200000","data":{"permission":null}}"#, Unreadable),
        ("kucoin", 200, r#"{"code":"400003","data":{"permission":"General"}}"#, Unreadable),
        ("kucoin", 200, "not json", Unreadable),
        ("kraken", 200, "{}", Unreadable),
    ] { assert_eq!(permission(venue, status, body), want, "{venue} {body}"); }
}

#[test]
fn keys_are_never_arguments() {
    assert!(refuse_key_args(&["kraken".into(), "--key".into(), "abc".into()]).is_err());
    assert!(refuse_key_args(&["kraken".into(), "--api-secret=abc".into()]).is_err());
    assert!(refuse_key_args(&["kraken".into(), "--since".into(), "2024-01-01T00:00:00Z".into()]).is_ok());
}

fn window() -> Window {
    Window { since: "2024-03-01T00:00:00Z".parse().unwrap(), until: "2024-03-10T00:00:00Z".parse().unwrap(), symbol: None }
}
const KEY: &str = "PKTESTKEY0123456789";
fn keys() -> Keys { Keys { key: KEY.into(), secret: "TESTSECRETabcdefghijkl".into(), passphrase: String::new() } }

fn alpaca_order(n: u8) -> Value {
    json!({"id": format!("61e69015-8549-4bfd-b9c3-01e75843f4{n:02}"), "symbol": "AAPL", "qty": "1.5", "status": "filled", "submitted_at": "2024-03-05T14:30:00Z", "echo": KEY})
}

#[tokio::test(flavor = "current_thread")]
async fn a_scripted_run_writes_the_set_with_no_key_and_shifted_windows() {
    let dir = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1), alpaca_order(2), alpaca_order(3)]));
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1)]));
    t.reply("GET /v2/orders", 200, json!([]));
    let report = record(&t, "alpaca", "https://paper-api.alpaca.markets", &window(), &keys(), &mut scrubber(), dir.path()).await.unwrap();
    assert!(report.contains("orders.json /[]/id: replaced"), "{report}");
    let sent = t.requests();
    assert_eq!(sent.len(), 3);
    assert!(sent[1].query.contains(&("limit", "1".into())));
    assert!(sent[2].query.contains(&("until", "2024-02-29T00:00:00Z".into())));
    let venue = dir.path().join("alpaca");
    let mut names: Vec<String> = std::fs::read_dir(&venue).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["manifest.json", "orders.empty.json", "orders.json", "orders.page.json"]);
    for n in &names {
        let text = std::fs::read_to_string(venue.join(n)).unwrap();
        assert!(!text.contains(KEY) && !text.contains("61e69015-8549"), "{n}: {text}");
    }
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(venue.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["files"][0]["params"], json!({"status": "all", "after": "2023-11-22T00:00:00Z", "until": "2023-12-01T00:00:00Z"}));
    assert_eq!(manifest["exchange"], "alpaca");
    assert_eq!(manifest["files"][0]["orders"], 3);
}

#[tokio::test(flavor = "current_thread")]
async fn a_rerun_replaces_the_whole_set() {
    let dir = tempfile::tempdir().unwrap();
    let venue = dir.path().join("alpaca");
    std::fs::create_dir_all(&venue).unwrap();
    std::fs::write(venue.join("orders.stale.json"), "{}").unwrap();
    std::fs::write(venue.join("notes.txt"), "kept").unwrap();
    let t = ScriptedTransport::default();
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1), alpaca_order(2)]));
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1)]));
    t.reply("GET /v2/orders", 200, json!([]));
    record(&t, "alpaca", "https://x", &window(), &keys(), &mut scrubber(), dir.path()).await.unwrap();
    assert!(!venue.join("orders.stale.json").exists());
    assert!(venue.join("notes.txt").exists() && venue.join("orders.json").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_last_request_or_a_case_that_shows_nothing_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1), alpaca_order(2)]));
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1)]));
    t.reply("GET /v2/orders", 500, json!({"message": "internal"}));
    let err = record(&t, "alpaca", "https://x", &window(), &keys(), &mut scrubber(), dir.path()).await.unwrap_err();
    assert!(err.contains("HTTP 500") && err.contains("nothing written"), "{err}");
    assert!(!dir.path().join("alpaca").exists());
    let t = ScriptedTransport::default();
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1), alpaca_order(2)]));
    t.reply("GET /v2/orders", 200, json!([alpaca_order(1), alpaca_order(2)]));
    let err = record(&t, "alpaca", "https://x", &window(), &keys(), &mut scrubber(), dir.path()).await.unwrap_err();
    assert!(err.contains("page case"), "{err}");
    assert!(!dir.path().join("alpaca").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn a_key_that_can_trade_is_refused_before_any_list_read() {
    let dir = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply("POST /v1/roles", 200, json!({"isAuditor": false, "isFundManager": false, "isTrader": true}));
    let w = Window { symbol: Some("btcusd".into()), ..window() };
    let err = record(&t, "gemini", "https://x", &w, &keys(), &mut scrubber(), dir.path()).await.unwrap_err();
    assert!(err.contains("can trade"), "{err}");
    assert_eq!(t.requests().len(), 1);
}

#[test]
fn query_signed_requests_carry_the_clients_signature_and_never_the_secret() {
    let w = Window { symbol: Some("MXUSDT".into()), ..window() };
    let now = "2024-03-10T00:00:00Z".parse().unwrap();
    let (r, named) = request(endpoint("mexc", "all_orders"), Case::Page, &w, &keys(), "https://api.mexc.com", now);
    let names: Vec<&str> = r.query.iter().map(|(k, _)| *k).collect();
    assert_eq!(names, ["endTime", "limit", "recvWindow", "signature", "startTime", "symbol", "timestamp"], "MexcVenue::request sorts after signing");
    assert!(!format!("{r:?}").contains("TESTSECRET"));
    assert!(named.iter().all(|(k, _)| !["signature", "timestamp", "recvWindow"].contains(k)));
    let (r, _) = request(endpoint("hyperliquid", "historical_orders"), Case::Full, &w, &keys(), "https://x", now);
    assert_eq!(r.body, Some(json!({"type": "historicalOrders", "user": KEY})));
    // userFillsByTime: the type, the address, then the window's start as a number (no end, no other param).
    let (r, named) = request(endpoint("hyperliquid", "user_fills_by_time"), Case::Full, &w, &keys(), "https://x", now);
    assert_eq!(r.body, Some(json!({"type": "userFillsByTime", "user": KEY, "startTime": w.since.timestamp_millis()})));
    assert_eq!(named.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["type", "user", "startTime"]);
}

#[test]
fn a_hyperliquid_spot_coin_named_by_its_index_is_kept() {
    let mut sc = Scrubber::new(b"salt".to_vec(), DAYS);
    let body = sc.scrub("hyperliquid", "historical_orders.json", json!([{"order": {"coin": "@107", "oid": 9123456789i64}, "status": "filled"}])).unwrap();
    assert_eq!(body[0]["order"]["coin"], json!("@107"));
    assert_ne!(body[0]["order"]["oid"], json!(9123456789i64));
}

#[test]
fn many_near_equal_values_scrub_without_a_fake_meeting_a_real_one() {
    // 1500 prices one tick apart (the Assistance Fund's answer): a fake made early must not equal or contain one met later.
    let rows: Vec<Value> = (0..1500).map(|i| json!({"order": {"limitPx": format!("79.{i:03}"), "sz": format!("1{i:03}.3"), "oid": 9_000_000_000i64 + i}})).collect();
    let mut sc = scrubber();
    sc.reserve(&Value::Array(rows.clone()));
    let body = sc.scrub("hyperliquid", "historical_orders.json", Value::Array(rows)).unwrap();
    sc.leak_check("historical_orders.json", &body, &[]).unwrap();
}

#[test]
fn a_kept_zero_bars_no_fake() {
    // Gemini's shape: a zero average beside a small amount whose fake keeps the same leading zeros.
    let body = json!([{"avg_execution_price": "0.00", "original_amount": "0.0015", "executed_amount": "0.0015"}]);
    let mut sc = scrubber();
    sc.reserve(&body);
    let out = sc.scrub("gemini", "orders_history.json", body).unwrap();
    assert_eq!(out[0]["avg_execution_price"], json!("0.00"));
    sc.leak_check("orders_history.json", &out, &[]).unwrap();
}

// ---- kucoin-uta capture mode (PLAN-841 U5-0, r1.1–r5) ----

const ORDER: &str = "68a3e81d2a090d00070575f7";
const FACTOR: u32 = 3;

fn uta_window() -> Window {
    Window { since: "2024-03-05T00:00:00Z".parse().unwrap(), until: "2024-03-08T00:00:00Z".parse().unwrap(), symbol: Some("BTC-USDT".into()) }
}
fn kc_keys() -> Keys { Keys { key: KEY.into(), secret: "TESTSECRETabcdefghijkl".into(), passphrase: "TESTPASSPHRASE01".into() } }
fn ok_data(data: Value) -> Value { json!({"code": "200000", "data": data}) }
fn refusal() -> Value { json!({"code": "400100", "msg": "This API is not supported in unified account mode"}) }
fn hf_fill() -> Value {
    json!({"id": 2312411231i64, "orderId": "6721a1f3e2b5d00007f4b123", "tradeId": 11116472408833i64, "symbol": "BTC-USDT", "side": "buy", "liquidity": "taker",
           "type": "market", "price": "67012.3", "size": "0.0001", "funds": "6.70123", "fee": "0.00670123", "feeRate": "0.001", "feeCurrency": "USDT", "createdAt": 1709712000123i64})
}
fn execution(trade: &str, size: &str, value: &str, fee: &str) -> Value {
    json!({"tradeId": trade, "orderId": ORDER, "symbol": "BTC-USDT", "side": "BUY", "tradeType": "SPOT", "liquidityRole": "TAKER", "price": "67000.1",
           "size": size, "value": value, "fee": fee, "feeCurrency": "USDT", "executionTime": 1709712000123000000i64})
}
fn detail() -> Value {
    ok_data(json!({"orderId": ORDER, "clientOid": "5c52e11203aa677f33e493fb", "symbol": "BTC-USDT", "side": "BUY", "orderType": "MARKET", "tradeType": "SPOT",
                   "status": 3, "size": "28.14", "sizeUnit": "QUOTECCY", "filledSize": "0.00042", "price": "0", "avgPrice": "67000.1",
                   "orderTime": 1709712000100000000i64, "updatedTime": 1709712000200000000i64}))
}

/// One phase's answers, in the order `uta::reads` sends them. `switched`: the classic routes refuse; HF still lists the
/// pre-switch fill.
fn script(t: &ScriptedTransport, phase: uta::Phase) {
    let before = phase == uta::Phase::Before;
    t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General"})));
    t.reply("GET /api/ua/v2/account/mode", 200, ok_data(json!({"selfAccountMode": if before { "CLASSIC" } else { "UNIFIED" }})));
    let classic = |t: &ScriptedTransport, path: &str, body: Value| { if before { t.reply(path, 200, body); } else { t.reply(path, 400, refusal()); } };
    classic(t, "GET /api/v1/hf/accounts/opened", ok_data(json!(true)));
    classic(t, "GET /api/v1/accounts", ok_data(json!([{"id": "5bd6e9286d99522a52e458de", "currency": "USDT", "type": "trade_hf", "balance": "12.345", "available": "12.345", "holds": "0"}])));
    t.reply("GET /api/v1/hf/fills", 200, ok_data(json!({"items": [hf_fill()], "lastId": 2312411231i64})));
    t.reply("GET /api/v1/hf/fills", 200, ok_data(json!({"items": [], "lastId": 2312411231i64})));
    for _ in 0..2 { classic(t, "GET /api/v1/fills", ok_data(json!({"currentPage": 1, "pageSize": 500, "totalNum": 0, "totalPage": 0, "items": []}))); }
    if before { return; }
    t.reply("GET /api/v1/orders", 400, refusal());
    t.reply("GET /api/v1/hf/orders/done", 400, refusal());
    t.reply("GET /api/ua/v2/unified/account/balance", 200, ok_data(json!({"accountType": "UNIFIED", "accounts": [{"currencies": [
        {"currency": "USDT", "balance": "20.5", "available": "20.5", "hold": "0", "liability": "0", "equity": "20.5"}]}]})));
    let page = |items: Vec<Value>, last: u64| ok_data(json!({"items": items, "lastId": last}));
    t.reply("GET /api/ua/v2/unified/order/execution", 200, page(vec![], 0));
    t.reply("GET /api/ua/v2/unified/order/history", 200, page(vec![], 0));
    if phase != uta::Phase::Traded { return; }
    t.reply("GET /api/ua/v2/unified/order/detail", 200, detail());
    // A trade on two pages, the second page repeating one fill: each tradeId once, Σ size = filledSize.
    let (e1, e2, e3) = (execution("T1001", "0.0002", "13.40002", "0.01340002"), execution("T1002", "0.0001", "6.70001", "0.00670001"),
                        execution("T1003", "0.00012", "8.040012", "0.008040012"));
    t.reply("GET /api/ua/v2/unified/order/execution", 200, page(vec![e1, e2.clone()], 9007199254740993));
    t.reply("GET /api/ua/v2/unified/order/execution", 200, page(vec![e2, e3], 9007199254740995));
    t.reply("GET /api/ua/v2/unified/order/execution", 200, page(vec![], 9007199254740995));
}

fn read_json(path: &std::path::Path) -> Value { serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap() }
fn uta_scrubber() -> Scrubber { Scrubber::new(b"test salt".to_vec(), DAYS) }

#[test]
fn uta_amounts_scale_exactly_with_their_decimals() {
    assert_eq!(scale("0.00042", 3).as_deref(), Some("0.00126"));
    assert_eq!(scale("100", 7).as_deref(), Some("700"));
    assert_eq!(scale("-1.5", 2).as_deref(), Some("-3.0"));
    assert_eq!(scale("0.00000000", 9).as_deref(), Some("0.00000000"));
    assert_eq!(scale("1e5", 2), None);
}

#[tokio::test(flavor = "current_thread")]
async fn uta_traded_phase_records_every_page_and_the_scrubbed_set_reads_as_the_adapter_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    script(&t, uta::Phase::Traded);
    let mut sc = uta_scrubber();
    let report = uta::record_uta(&t, "https://x", uta::Phase::Traded, &uta_window(), &[ORDER.into()], &kc_keys(), &mut sc, FACTOR, dir.path(), None).await.unwrap();
    assert!(report.contains("refused (recorded as the answer): hf_accounts_opened.json, accounts.json, fills_symbol.1.json"), "{report}");
    assert!(!report.contains("cannot read"), "{report}");
    let set = dir.path().join("kucoin_uta/traded");
    let files: Vec<Value> = ["order_1_detail.json", "order_1_executions.1.json", "order_1_executions.2.json", "order_1_executions.3.json"].iter().map(|f| read_json(&set.join(f))).collect();
    // r2.2, r3.1: the contract check the adapter runs; the totals are the raw ones times the factor.
    let d = &files[0]["data"];
    let want = deltabadger::venue::kucoin::HfOrder { id: d["orderId"].as_str().unwrap().into(), symbol: "BTC-USDT".into(), sell: false, limit: false };
    assert_ne!(want.id, ORDER);
    let (state, fees) = deltabadger::venue::kucoin::uta_evidence(&files, &want).unwrap();
    assert_eq!(state.amount_exec.to_s_f(), "0.00126");
    assert_eq!(state.quote_amount_exec.unwrap().to_s_f(), "84.420126");
    assert_eq!(fees["USDT"].to_s_f(), "0.084420126");
    // Kept: the enums and the price; shifted: ns times; replaced: ids.
    let e = &files[1]["data"]["items"][0];
    assert_eq!((e["tradeType"].as_str(), e["side"].as_str(), e["liquidityRole"].as_str(), e["price"].as_str()), (Some("SPOT"), Some("BUY"), Some("TAKER"), Some("67000.1")));
    assert_eq!(e["executionTime"], json!(1709712000123000000i64 - DAYS * 86_400_000_000_000));
    assert_eq!(d["orderTime"], json!(1709712000100000000i64 - DAYS * 86_400_000_000_000));
    assert_eq!((d["status"].as_u64(), d["sizeUnit"].as_str(), d["avgPrice"].as_str()), (Some(3), Some("QUOTECCY"), Some("67000.1")));
    assert_ne!(e["tradeId"], "T1001");
    // r2.3: each page's request carries the page before's (scrubbed) cursor, so a replay walks to the empty page.
    let manifest = read_json(&set.join("manifest.json"));
    let entry = |f: &str| manifest["files"].as_array().unwrap().iter().find(|e| e["file"] == f).unwrap().clone();
    assert_eq!(entry("order_1_executions.2.json")["params"]["lastId"].as_str().unwrap(), files[1]["data"]["lastId"].to_string());
    assert_eq!(entry("order_1_executions.3.json")["params"]["lastId"].as_str().unwrap(), files[2]["data"]["lastId"].to_string());
    assert_eq!(entry("order_1_detail.json")["params"]["orderId"], d["orderId"]);
    assert_eq!(entry("executions.1.json")["params"]["startAt"], json!((1709596800000i64 - DAYS * 86_400_000).to_string()));
    // r1.1: a refusal is that route's recorded answer, its code and message kept.
    assert_eq!(entry("accounts.json")["refused"], true);
    assert_eq!(read_json(&set.join("accounts.json")), refusal());
    for f in std::fs::read_dir(&set).unwrap() {
        let text = std::fs::read_to_string(f.unwrap().path()).unwrap();
        assert!(!text.contains(ORDER) && !text.contains("T1001") && !text.contains(KEY) && !text.contains("5c52e11203aa"), "{text}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn uta_phases_share_one_scrub_through_the_state_file() {
    let (dir, keys_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let state = keys_dir.path().join("kucoin-uta.state.json");
    let (mut sc, factor) = uta::load_state(&state, true).unwrap();
    assert!((2..=19).contains(&factor));
    let t = ScriptedTransport::default();
    script(&t, uta::Phase::Before);
    uta::record_uta(&t, "https://x", uta::Phase::Before, &uta_window(), &[], &kc_keys(), &mut sc, factor, dir.path(), Some(&state)).await.unwrap();
    assert!(uta::load_state(&keys_dir.path().join("none.json"), false).err().unwrap().contains("before phase first"));
    let (mut sc, again) = uta::load_state(&state, false).unwrap();
    assert_eq!(again, factor);
    let t = ScriptedTransport::default();
    script(&t, uta::Phase::Switched);
    uta::record_uta(&t, "https://x", uta::Phase::Switched, &uta_window(), &[], &kc_keys(), &mut sc, factor, dir.path(), None).await.unwrap();
    let fill = |phase: &str| read_json(&dir.path().join(format!("kucoin_uta/{phase}/hf_fills.1.json")))["data"]["items"][0].clone();
    let (a, b) = (fill("before"), fill("switched"));
    assert_eq!(a, b, "the same fill keeps the same scrubbed ids, time and amounts in both phases");
    assert_ne!(a["orderId"], hf_fill()["orderId"]);
    assert_eq!(a["size"], json!(scale("0.0001", factor).unwrap()));
    assert!(uta::state_outside(&dir.path().join("x.json"), dir.path()).is_err());
    assert!(uta::state_outside(&state, dir.path()).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn uta_aborts_on_a_throttle_a_server_error_the_wrong_mode_or_a_trading_key() {
    let run = |t: ScriptedTransport, phase: uta::Phase| async move {
        let dir = tempfile::tempdir().unwrap();
        let r = uta::record_uta(&t, "https://x", phase, &uta_window(), &[], &kc_keys(), &mut uta_scrubber(), FACTOR, dir.path(), None).await;
        assert!(!dir.path().join("kucoin_uta").exists());
        r.unwrap_err()
    };
    // A throttle on a route that may refuse is still no answer.
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General"})));
    t.reply("GET /api/ua/v2/account/mode", 200, ok_data(json!({"selfAccountMode": "UNIFIED"})));
    t.reply("GET /api/v1/hf/accounts/opened", 429, json!({"code": "429000", "msg": "Too Many Requests"}));
    assert!(run(t, uta::Phase::Switched).await.contains("HTTP 429"));
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General"})));
    t.reply("GET /api/ua/v2/account/mode", 500, json!({"code": "500000", "msg": "internal"}));
    assert!(run(t, uta::Phase::Switched).await.contains("HTTP 500"));
    // The before phase on an account already switched.
    let t = ScriptedTransport::default();
    script(&t, uta::Phase::Switched);
    assert!(run(t, uta::Phase::Before).await.contains("mode"));
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General,Unified"})));
    assert!(run(t, uta::Phase::Switched).await.contains("can trade"));
    // A failed signature is the probe failing, not the route's refusal; so is an answer with no code.
    for (status, body) in [(401, json!({"code": "400005", "msg": "Invalid KC-API-SIGN"})), (400, json!({"code": "400005", "msg": "Invalid KC-API-SIGN"})),
                           (400, json!({"msg": "x"})), (400, json!({"code": "110188", "msg": "Too many requests"})), (200, json!({"code": "166115", "msg": "System busy"})), (200, json!({"code": "999999", "msg": "Server busy, please try again"}))] {
        let t = ScriptedTransport::default();
        t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General"})));
        t.reply("GET /api/ua/v2/account/mode", 200, ok_data(json!({"selfAccountMode": "UNIFIED"})));
        t.reply("GET /api/v1/hf/accounts/opened", status, body);
        assert!(run(t, uta::Phase::Switched).await.contains("nothing written"));
    }
    // A page without an item list is never the walk's end.
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General"})));
    t.reply("GET /api/ua/v2/account/mode", 200, ok_data(json!({"selfAccountMode": "CLASSIC"})));
    t.reply("GET /api/v1/hf/accounts/opened", 200, ok_data(json!(true)));
    t.reply("GET /api/v1/accounts", 200, ok_data(json!([])));
    t.reply("GET /api/v1/hf/fills", 200, ok_data(json!({"lastId": 1})));
    assert!(run(t, uta::Phase::Before).await.contains("no item list"));
}

#[tokio::test(flavor = "current_thread")]
async fn uta_an_order_answered_for_another_id_is_named_not_passed() {
    let dir = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    script(&t, uta::Phase::Traded);
    let other = "68a3e81d2a090d0007aaaaaa";
    let report = uta::record_uta(&t, "https://x", uta::Phase::Traded, &uta_window(), &[other.into()], &kc_keys(), &mut uta_scrubber(), FACTOR, dir.path(), None).await.unwrap();
    assert!(report.contains("order 1: the adapter cannot read"), "{report}");
}

#[test]
fn uta_a_message_with_digits_or_an_address_is_replaced() {
    let mut sc = uta_scrubber();
    sc.extra = Some(uta::extra(FACTOR));
    let out = sc.scrub("kucoin", "x.json", json!({"code": "400100", "msg": "account 1234 user@x.io is not supported"})).unwrap();
    assert_ne!(out["msg"], "account 1234 user@x.io is not supported");
    let out = sc.scrub("kucoin", "x.json", refusal()).unwrap();
    assert_eq!(out, refusal());
    assert!(sc.scrub("kucoin", "x.json", json!({"code": "200000", "data": {"size": "1e-5"}})).unwrap_err().contains("no plain decimal"));
    assert!(sc.scrub("kucoin", "x.json", json!({"code": "200000", "data": {"balance": 0.5}})).unwrap_err().contains("fractional JSON number"));
    assert_eq!(sc.scrub("kucoin", "x.json", json!({"code": "400100", "msg": "账户 张三"})).unwrap()["msg"], "(scrubbed)");
    assert_eq!(sc.scrub("kucoin", "x.json", json!({"code": "200000", "data": {"items": [{"remark": "张三"}]}})).unwrap()["data"]["items"][0]["remark"], "(scrubbed)");
    assert!(sc.scrub("kucoin", "x.json", json!({"code": "200000", "data": {"balance": 9000000000000000001u64}})).unwrap_err().contains("exact integer range"));
}

#[tokio::test(flavor = "current_thread")]
async fn uta_classic_pages_walk_to_total_page_and_the_known_fill_must_be_on_the_symbol() {
    let base = |t: &ScriptedTransport| {
        t.reply("GET /api/v1/user/api-key", 200, ok_data(json!({"permission": "General"})));
        t.reply("GET /api/ua/v2/account/mode", 200, ok_data(json!({"selfAccountMode": "CLASSIC"})));
        t.reply("GET /api/v1/hf/accounts/opened", 200, ok_data(json!(false)));
        t.reply("GET /api/v1/accounts", 200, ok_data(json!([])));
        t.reply("GET /api/v1/hf/fills", 200, ok_data(json!({"items": [], "lastId": 0})));
    };
    let classic = |items: Vec<Value>, page: u64| ok_data(json!({"currentPage": page, "pageSize": 500, "totalNum": 2, "totalPage": "2", "items": items}));
    let dir = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    base(&t);
    // A string totalPage of 2, its first page empty: both pages are read, as the ledger reads them.
    t.reply("GET /api/v1/fills", 200, classic(vec![], 1));
    t.reply("GET /api/v1/fills", 200, classic(vec![hf_fill()], 2));
    for _ in 0..2 { t.reply("GET /api/v1/fills", 200, classic(vec![], 1)); }
    uta::record_uta(&t, "https://x", uta::Phase::Before, &uta_window(), &[], &kc_keys(), &mut uta_scrubber(), FACTOR, dir.path(), None).await.unwrap();
    assert!(dir.path().join("kucoin_uta/before/fills_symbol.2.json").exists());
    assert_eq!(t.requests().iter().filter(|r| r.path == "/api/v1/fills").count(), 4);
    // A fill on another symbol is no known fill.
    let t = ScriptedTransport::default();
    base(&t);
    let mut other = hf_fill();
    other["symbol"] = json!("ETH-USDT");
    t.reply("GET /api/v1/fills", 200, ok_data(json!({"currentPage": 1, "totalPage": 1, "items": [other]})));
    t.reply("GET /api/v1/fills", 200, ok_data(json!({"currentPage": 1, "totalPage": 1, "items": []})));
    let err = uta::record_uta(&t, "https://x", uta::Phase::Before, &uta_window(), &[], &kc_keys(), &mut uta_scrubber(), FACTOR, tempfile::tempdir().unwrap().path(), None).await.unwrap_err();
    assert!(err.contains("no fill on --symbol"), "{err}");
}

#[test]
fn uta_export_is_scrubbed_with_the_session_state_and_still_reads() {
    let csv = "UID,Account Type,Currency,Side,Amount,Fee,Time(UTC+08:00),Remark,Type\n\
               123456789,Unified Account,USDT,Withdrawal,6.70123,0,2024-03-06 16:00:00,,Spot\n\
               123456789,Unified Account,BTC,Deposit,0.0001,0.0000001,2024-03-06 16:00:00,,Spot\n";
    let mut sc = uta_scrubber();
    let out = uta::scrub_csv(csv, &mut sc, FACTOR, &[]).unwrap();
    assert!(!out.contains("123456789"), "{out}");
    assert!(out.contains("\"Unified Account\",\"USDT\",\"Withdrawal\",\"20.10369\",\"0\",\"2023-11-27 16:00:00\",\"\",\"Spot\""), "{out}");
    let rows = deltabadger::tracker::import::kucoin::read(&deltabadger::tracker::import::csv::Table::parse(&out).unwrap()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].fee.to_s_f(), "0.0000003");
    // A worded remark that repeats the side is kept, not barred as an original.
    let csv = "UID,Account Type,Currency,Side,Amount,Fee,Time(UTC),Remark,Type\n123456789,Funding Account,USDT,Deposit,10,0,2024-03-06 16:00:00,Deposit,Deposit\n";
    assert!(uta::scrub_csv(csv, &mut uta_scrubber(), FACTOR, &[]).unwrap().contains("\"Deposit\",\"Deposit\""));
    // A file without the export's header row (its first row would be copied as is) is refused.
    let email = "UID,Account Type,Currency,Side,Amount,Fee,Time(UTC),Remark,Type\n1,Funding Account,USDT,Deposit,10,0,2024-03-06 16:00:00,alice@example.com,Deposit\n";
    assert!(!uta::scrub_csv(email, &mut uta_scrubber(), FACTOR, &[]).unwrap().contains("alice@example.com"));
    let name = email.replace("alice@example.com", "张三 123");
    let out = uta::scrub_csv(&name, &mut uta_scrubber(), FACTOR, &[]).unwrap();
    assert!(!out.contains("张三") && out.contains("(scrubbed)"), "{out}");
    for odd in ["-", "0"] { assert!(uta::scrub_csv(&email.replace("alice@example.com", odd), &mut uta_scrubber(), FACTOR, &[]).is_ok(), "{odd}"); }
    let person = uta::scrub_csv(&email.replace("alice@example.com", "Alice Smith"), &mut uta_scrubber(), FACTOR, &[]).unwrap();
    assert!(!person.contains("Alice Smith"), "{person}");
    // A credential in a column kept as is (here Type) is caught by the leak check.
    let pass = email.replace(",Deposit\n", ",Hunter2pass\n");
    assert!(uta::scrub_csv(&pass, &mut uta_scrubber(), FACTOR, &["Hunter2pass"]).unwrap_err().contains("credential"));
    for bad in ["123456789,Unified Account,USDT\n", "RECORDER_KEY=abc\nRECORDER_SECRET=def\n",
                "UID,Account Type,Currency,Side,Amount,Fee,Time(123456789),Remark,Type\n"] {
        assert!(uta::scrub_csv(bad, &mut uta_scrubber(), FACTOR, &[]).unwrap_err().contains("not a KuCoin Account History export"));
    }
}
