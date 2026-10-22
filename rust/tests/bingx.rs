//! The BINGX trading adapter (venue::bingx, plan VB slice A7) over recorded bodies: its signer against honeymaker's own requests
//! (the `QueryHmacHex` scheme Binance shares, tests/binance_trade.rs pinning Binance's), its rules against
//! Exchanges::Bingx::ERRORS, #parse_order_data (`parse_binance_shaped`) and the sizing wire recorded from Rails, and its
//! reads, placement, per-order polls, the recovery by client order id (it can only FIND an order) and cancel. Rails-free.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::amount::{self, Sizing};
use deltabadger::engine::model::{self, Ticker};

use deltabadger::engine::venue_rules::BINGX;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::{Pair, Pairs};
use deltabadger::venue::bingx::{self, BingxVenue, URL};
use deltabadger::venue::http::{HttpRequest, ScriptedTransport};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, Side, Venue, VenueError};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec {
    BigDec::parse(s).unwrap()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/bingx_signing.json")).unwrap()
}
fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/bingx_vectors.json")).unwrap()
}
fn btc_usdt() -> Ticker {
    Ticker {
        id: 1,
        ticker: "BTC-USDT".into(),
        base_code: "BTC".into(),
        quote_code: "USDT".into(),
        base_symbol: "BTC".into(),
        quote_symbol: "USDT".into(),
        exchange_name: "BingX".into(),
        base_asset_id: 1,
        quote_asset_id: 2,
        base_decimals: 6,
        quote_decimals: 2,
        price_decimals: 2,
        minimum_base_size: bd("0.000001"),
        minimum_quote_size: bd("1"),
        trading_enabled: true,
        available: true,
        crypto: true,
    }
}
fn pairs() -> Pairs {
    let mut p = Pairs::default();
    p.by_symbol.insert(
        "BTC-USDT".into(),
        Pair {
            base: "BTC".into(),
            quote: "USDT".into(),
            price_decimals: 2,
        },
    );
    p.by_symbol.insert(
        "PEPE-USDT".into(),
        Pair {
            base: "PEPE".into(),
            quote: "USDT".into(),
            price_decimals: 10,
        },
    );
    for code in ["BTC", "USDT", "PEPE"] {
        p.assets.insert(code.into(), code.into());
    }
    p
}
fn venue(script: Value) -> (ScriptedTransport, BingxVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (
        t.clone(),
        BingxVenue::new(t, URL, "s3cr3t").with_pairs(pairs()),
    )
}
fn ok(body: Value) -> Value {
    json!([{ "status": 200, "body": {"code":0,"data":body} }])
}
/// The query a request sent, without what changes per send.
fn sent(r: &HttpRequest) -> Vec<(String, String)> {
    r.query
        .iter()
        .filter(|(k, _)| !matches!(*k, "timestamp" | "signature"))
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}
fn pairs_of(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
const CL: &str = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
const WIRE_CL: &str = "5f1c1f3e2b8f4c4e9a590d9c3f1b6a01";
fn order(kind: OrderKind, quote: bool, volume: &str) -> NewOrder {
    NewOrder {
        pair: "BTC-USDT".into(),
        side: Side::Buy,
        kind,
        volume: volume.into(),
        quote_volume: quote,
        cl_ord_id: CL.into(),
        deadline: "2026-09-01T10:00:10Z".parse().unwrap(),
        day: false,
    }
}
fn raw_order(id: &str, status: &str, kind: &str, executed: &str, cumulative: &str) -> Value {
    json!({ "symbol": "BTC-USDT", "orderId": id, "orderListId": -1, "clientOrderID": WIRE_CL, "price": if kind == "LIMIT" { "64150" } else { "0" },
            "origQty": if kind == "LIMIT" { "0.00093" } else { "0" }, "executedQty": executed, "cummulativeQuoteQty": cumulative, "status": status,
            "type": kind, "side": "BUY", "origQuoteOrderQty": if kind == "LIMIT" { Value::Null } else { json!("60") } })
}

#[test]
fn the_fixtures_are_recorded_from_the_current_rails_sources() {
    use sha2::Digest;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for f in [fixture(), vectors()] {
        for (file, recorded) in f["sources"].as_object().unwrap() {
            let now = hex::encode(sha2::Sha256::digest(
                std::fs::read(root.join(file)).unwrap(),
            ));
            assert_eq!(
                recorded.as_str().unwrap(),
                now,
                "{file} changed: re-record with script/rust/repin"
            );
        }
    }
}

/// Honeymaker::Clients::BingX's signed requests, byte for byte under a frozen #timestamp_ms: GETs and POSTs with
/// every param in the query (an escaped one included), X-BX-APIKEY on every request of a keyed client, a public read.
#[test]
fn every_recorded_honeymaker_request_is_reproduced() {
    let cases = fixture()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 24);
    for c in cases {
        let method: &'static str = match c["method"].as_str().unwrap() {
            "GET" => "GET",
            "POST" => "POST",
            "DELETE" => "DELETE",
            m => panic!("{m}"),
        };
        let query = c["query"].as_str().unwrap_or("");
        let params: Vec<(&'static str, String)> =
            reqwest::Url::parse(&format!("http://x/?{query}"))
                .unwrap()
                .query_pairs()
                .filter(|(k, _)| k != "timestamp" && k != "signature")
                .map(|(k, v)| (&*Box::leak(k.into_owned().into_boxed_str()), v.into_owned()))
                .collect();
        let v = BingxVenue::new(
            ScriptedTransport::default(),
            URL,
            c["api_secret"].as_str().unwrap(),
        );
        // A moment half a millisecond in: Ruby's Float milliseconds of it are the recorded ones.
        let public = c["call"].as_str().unwrap().ends_with("public");
        let r = if public {
            v.request(method, c["path"].as_str().unwrap(), params)
        } else {
            v.signed(
                method,
                c["path"].as_str().unwrap(),
                params,
                DateTime::from_timestamp_micros(c["timestamp"].as_i64().unwrap() * 1000 + 500)
                    .unwrap(),
            )
        };
        // The query as ReqwestTransport::bingx sends it (`sign::request`): Faraday::Utils.build_query, the text signed.
        assert_eq!(
            deltabadger::venue::binance::build_query(&r.query),
            c["query"].as_str().unwrap_or(""),
            "{c}"
        );
        assert!(
            r.body.is_none() && c["body"].as_str().is_none_or(str::is_empty),
            "honeymaker sends every param in the query, a POST too: {c}"
        );
        // Faraday's header names are case-insensitive (it keeps "X-bingx-apikey").
        let h: std::collections::BTreeMap<String, Value> = c["headers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .filter(|(k, _)| k != "content-length")
            .collect(); // Faraday's adapter, and reqwest, count the empty body themselves
        let want: std::collections::BTreeMap<String, Value> = if public {
            Default::default()
        } else {
            let mut h =
                std::collections::BTreeMap::from([("x-bx-apikey".into(), c["api_key"].clone())]);
            if method == "POST" {
                h.insert("content-type".into(), json!("application/json"));
            }
            h
        };
        assert_eq!(
            h, want,
            "the key header and the JSON ones only, as ReqwestTransport::bingx sends them: {c}"
        );
    }
}

/// engine::venue_rules::BINGX is Exchanges::Bingx::ERRORS, and Exchange#ambiguous_placement_error? as it reads them.
#[test]
fn the_rules_are_rails_errors() {
    let f = fixture();
    let list = |v: &Value| {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for (kind, patterns) in BINGX.kinds {
        assert_eq!(
            patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            list(&f["errors"][kind]),
            "{kind}"
        );
    }
    let mut transient = list(&f["network_transient"]);
    transient.extend(list(&f["errors"]["transient"]));
    assert_eq!(
        BINGX
            .transient_text
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        transient,
        "Exchange#transient_error?"
    );
    assert!(
        BINGX.reports_missing
            && BINGX.young_missing_fails
            && !BINGX.absence_provable
            && BINGX.follow_up_strict
    );
}

/// #parse_order_data (B1), recorded over 10,000 generated GET /openApi/spot/v1/trade/query answers.
#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let v = vectors();
    let cases = v["bingx_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 10_000);
    let s = |o: &Option<BigDec>| o.as_ref().map(BigDec::to_s_f);
    let (mut averaged, mut rounded) = (0, 0);
    for c in cases {
        let id = format!(
            "{}-{}",
            c["raw"]["symbol"].as_str().unwrap(),
            c["raw"]["orderId"].as_str().unwrap()
        );
        let got = bingx::parse_order(&id, &c["raw"], &pairs()).unwrap();
        let status = match got.status {
            OrderStatus::Open => "open",
            OrderStatus::Closed => "closed",
            OrderStatus::Cancelled => "cancelled",
            OrderStatus::Unknown => "unknown",
            OrderStatus::Failed => "failed",
        };
        assert_eq!(status, c["status"], "{c}");
        assert_eq!(s(&got.price).as_deref(), c["price"].as_str(), "price {c}");
        assert_eq!(
            (s(&got.amount).as_deref(), s(&got.quote_amount).as_deref()),
            (c["amount"].as_str(), c["quote_amount"].as_str()),
            "{c}"
        );
        assert_eq!(
            (got.amount_exec.to_s_f(), s(&got.quote_amount_exec)),
            (
                c["amount_exec"].as_str().unwrap().to_string(),
                c["quote_amount_exec"].as_str().map(str::to_string)
            ),
            "{c}"
        );
        assert_eq!(
            (got.sell, got.limit),
            (c["side"] == "sell", c["order_type"] == "limit_order"),
            "{c}"
        );
        let filled = bd(c["raw"]["executedQty"].as_str().unwrap()).is_positive()
            && bd(c["raw"]["cummulativeQuoteQty"].as_str().unwrap()).is_positive();
        if filled && c["raw"]["price"] != "0" {
            averaged += 1;
        }
        if filled && c["raw"]["symbol"] != "NEW-USDT" {
            rounded += 1;
        }
    }
    assert!(averaged > 1000 && rounded > 1000, "B1's average over a non-zero price, and the tick rounding, are exercised: {averaged} {rounded}");
    // An unlisted status raises in Rails (#parse_order_status): refused, never read as some status.
    let raw = raw_order("C02__1", "PENDING_CANCEL", "MARKET", "0", "0");
    assert_eq!(
        bingx::parse_order("BTC-USDT-C02__1", &raw, &pairs()),
        Err(VenueError::Rejected(vec![
            "Unknown BingX order status: PENDING_CANCEL".into()
        ]))
    );
}

/// Bot::OrderSetter's sizing under Exchanges::Bingx#minimum_amount_logic and #set_market_order / #set_limit_order's wire (a
/// zero base minimum among the pairs: the quote minimum alone), through amount::size, limit_refusal, to_order and the
/// adapter's own POST query.
#[tokio::test(flavor = "current_thread")]
async fn every_recorded_rails_sizing_and_wire_is_reproduced() {
    let (_d, o, s) = common::install();
    use common::seed::{self, BotSpec};
    let market = model::load_bot(
        &o.primary,
        seed::insert_bot(
            &o.primary,
            &s,
            &BotSpec::weekly(60.0, "2026-09-01 10:00:00"),
        ),
    )
    .unwrap();
    let limit = model::load_bot(
        &o.primary,
        seed::insert_bot(
            &o.primary,
            &s,
            &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
                .with("limit_ordered", json!(true))
                .with("limit_order_pcnt_distance", json!(0.0025)),
        ),
    )
    .unwrap();
    let v = vectors();
    let cases = v["bingx_sizing"].as_array().unwrap();
    let deadline: DateTime<Utc> = "2026-09-01T10:00:10Z".parse().unwrap();
    let (mut quote, mut refused, mut zero_base) = (0, 0, 0);
    for c in cases {
        let bot = if c["order_type"] == "limit_order" {
            &limit
        } else {
            &market
        };
        let shape = &v["bingx_tickers"][c["ticker"].as_u64().unwrap() as usize];
        let int = |k: &str| shape[k].as_str().unwrap().parse::<i64>().unwrap();
        let t = Ticker {
            base_decimals: int("base_decimals"),
            quote_decimals: int("quote_decimals"),
            price_decimals: int("price_decimals"),
            minimum_base_size: bd(shape["minimum_base_size"].as_str().unwrap()),
            minimum_quote_size: bd(shape["minimum_quote_size"].as_str().unwrap()),
            ..btc_usdt()
        };
        if t.minimum_base_size.is_zero() {
            zero_base += 1;
        }
        let sizing = amount::size(
            bot,
            &t,
            &bd(c["x"].as_str().unwrap()),
            &bd(c["last_or_ask"].as_str().unwrap()),
            &BINGX,
        )
        .unwrap();
        let (plan, below) = match sizing {
            Sizing::Place(p) => (p, false),
            Sizing::BelowMinimum(p) => (p, true),
            other => panic!("{c}: {other:?}"),
        };
        assert_eq!(
            (plan.price.to_s_f(), plan.amount.to_s_f()),
            (
                c["price"].as_str().unwrap().to_string(),
                c["amount"].as_str().unwrap().to_string()
            ),
            "{c}"
        );
        assert_eq!(
            (plan.quote_type, below),
            (
                c["amount_type"] == "quote",
                c["below_minimum"].as_bool().unwrap()
            ),
            "{c}"
        );
        let refusal = plan.limit_refusal().unwrap();
        assert_eq!(refusal.as_deref(), c["refused"].as_str(), "refusal {c}");
        if refusal.is_some() {
            refused += 1;
            continue;
        }
        let order = plan.to_order(CL.into(), deadline, BINGX.wire).unwrap();
        let (t, venue) = venue(
            json!({ "POST /openApi/spot/v1/trade/order": ok(json!({ "symbol": "BTC-USDT", "orderId": "C02__1" })) }),
        );
        assert_eq!(venue.add_order(&order).await, Ok("BTC-USDT-C02__1".into()));
        let mut ours: serde_json::Map<String, Value> = sent(&t.requests()[0])
            .into_iter()
            .map(|(k, v)| (k, json!(v)))
            .collect();
        assert_eq!(ours.remove("newClientOrderId"), Some(json!(WIRE_CL)), "{c}");
        let mut want = serde_json::Map::new();
        for (rails, field) in c["wire"].as_object().unwrap() {
            let camel = match rails.as_str() {
                "quote_order_qty" => "quoteOrderQty",
                "time_in_force" => "timeInForce",
                other => other,
            };
            want.insert(camel.to_string(), field.clone());
        }
        if want.contains_key("quoteOrderQty") {
            quote += 1;
        }
        assert_eq!(Value::Object(ours), Value::Object(want), "{c}");
    }
    assert!(quote > 1000 && refused > 500 && zero_base > 3000, "both market wires, the limit refusal and a zero base minimum are exercised: {quote} {refused} {zero_base}");
}

#[tokio::test(flavor = "current_thread")]
async fn ambiguous_envelopes_and_server_failures_keep_placement_unsettled() {
    for (status, body) in [
        (200, json!({})),
        (200, json!({"data":{"orderId":"1"}})),
        (200, json!({"code":0,"data":{}})),
        (200, json!({"code":100500,"msg":"Internal system error"})),
        (503, json!({"code":100202,"msg":"Insufficient assets"})),
        (200, json!({"code":123456,"msg":"unknown"})),
    ] {
        let (_, v) =
            venue(json!({"POST /openApi/spot/v1/trade/order":[{"status":status,"body":body}]}));
        assert!(matches!(
            v.add_order(&order(OrderKind::Market, true, "60")).await,
            Err(VenueError::Ambiguous(_))
        ));
    }
    let (_, v) = venue(
        json!({"POST /openApi/spot/v1/trade/order":[{"status":200,"body":{"code":100202,"msg":"Insufficient assets"}}]}),
    );
    assert!(matches!(
        v.add_order(&order(OrderKind::Market, true, "60")).await,
        Err(VenueError::Rejected(_))
    ));
}
#[tokio::test(flavor = "current_thread")]
async fn dashed_ids_cancel_and_recovery_use_bingx_fields() {
    let (t, v) = venue(json!({"POST /openApi/spot/v1/trade/cancel":ok(json!({})),
        "GET /openApi/spot/v1/trade/query":ok(raw_order("9","FILLED","MARKET","1","60"))}));
    v.cancel("BTC-USDT-9").await.unwrap();
    assert_eq!(
        sent(&t.requests()[0]),
        pairs_of(&[("orderId", "9"), ("symbol", "BTC-USDT")])
    );
    let o = v
        .order_by_client_id(CL, "BTC-USDT", Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(o.txid, "BTC-USDT-9");
    assert_eq!(
        sent(&t.requests()[1]),
        pairs_of(&[("clientOrderID", WIRE_CL), ("symbol", "BTC-USDT")])
    );
}
#[tokio::test(flavor = "current_thread")]
async fn missing_requires_both_the_code_and_order_message() {
    for code in [100404, 100400] {
        for msg in [
            "Order does not exist",
            "Order not exist",
            "path does not exist",
        ] {
            let (_, v) = venue(
                json!({"GET /openApi/spot/v1/trade/query":[{"status":200,"body":{"code":code,"msg":msg}}]}),
            );
            let result = v.orders(&["BTC-USDT-9".into()]).await;
            assert_eq!(result.is_ok(), msg.starts_with("Order"));
            assert!(v
                .order_by_client_id(CL, "BTC-USDT", Utc::now())
                .await
                .is_err());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn public_prices_and_account_balances_follow_the_bingx_envelope() {
    use deltabadger::venue::PriceSide;
    let (_, v) = venue(json!({
        "GET /openApi/spot/v1/market/depth": ok(json!({"bids":[["63999","1"]],"asks":[["64000","2"]]})),
        "GET /openApi/spot/v2/market/ticker": ok(json!([{"symbol":"BTC-USDT","lastPrice":"64001"}])),
        "GET /openApi/spot/v1/account/balance": ok(json!({"balances":[{"asset":"BTC","free":"0.5","locked":"0.1"},{"asset":"USDT","free":"80","locked":"20"}]}))
    }));
    assert_eq!(
        v.price(&btc_usdt(), PriceSide::Ask).await.unwrap(),
        bd("64000")
    );
    assert_eq!(
        v.price(&btc_usdt(), PriceSide::Bid).await.unwrap(),
        bd("63999")
    );
    assert_eq!(
        v.price(&btc_usdt(), PriceSide::Last).await.unwrap(),
        bd("64001")
    );
    assert_eq!(v.balance("USDT", true).await.unwrap(), bd("80"));
    assert_eq!(v.free_base_balance(&btc_usdt()).await.unwrap(), bd("0.5"));
    assert_eq!(
        v.tickers_prices(&[]).await.unwrap(),
        vec![("BTC-USDT".into(), json!("64001.0"))]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn market_sizing_preserves_rails_s7_for_both_sides() {
    for side in [Side::Buy, Side::Sell] {
        for quote in [true, false] {
            let (t, v) = venue(
                json!({"POST /openApi/spot/v1/trade/order":ok(json!({"data":{"orderId":"7"}}))}),
            );
            let mut o = order(OrderKind::Market, quote, "1.25");
            o.side = side;
            assert_eq!(v.add_order(&o).await.unwrap(), "BTC-USDT-7");
            let r = &t.requests()[0];
            assert!(r.body.is_none());
            assert!(r.query.contains(&(
                if quote { "quoteOrderQty" } else { "quantity" },
                "1.25".into()
            )));
            assert!(!r
                .query
                .iter()
                .any(|(k, _)| *k == if quote { "quantity" } else { "quoteOrderQty" }));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn client_id_character_and_length_limits_are_checked_before_sending() {
    for id in [
        "x".repeat(41),
        "legacy-id".into(),
        String::new(),
        "é".into(),
    ] {
        let (t, v) = venue(json!({}));
        let mut o = order(OrderKind::Market, true, "60");
        o.cl_ord_id = id;
        assert!(matches!(
            v.add_order(&o).await,
            Err(VenueError::Rejected(_))
        ));
        assert!(matches!(
            v.order_by_client_id(&o.cl_ord_id, &o.pair, Utc::now())
                .await,
            Err(VenueError::Rejected(_))
        ));
        assert!(t.requests().is_empty());
    }
    let (t, v) = venue(json!({"POST /openApi/spot/v1/trade/order":ok(json!({"orderId":"7"}))}));
    let mut o = order(OrderKind::Market, true, "60");
    o.cl_ord_id = "a_".repeat(20);
    v.add_order(&o).await.unwrap();
    assert!(t.requests()[0]
        .query
        .contains(&("newClientOrderId", o.cl_ord_id)));
}
