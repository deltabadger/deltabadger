mod common;
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::alpaca::{self, AlpacaVenue, Urls, DATA_URL, PAPER_TRADING_URL};
use deltabadger::venue::http::{client, ReqwestTransport, ScriptedTransport};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Venue, VenueError};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn btc_usd() -> Ticker {
    Ticker { id: 1, ticker: "BTC/USD".into(), base_code: "BTC".into(), quote_code: "USD".into(), base_symbol: "BTC".into(), quote_symbol: "USD".into(), exchange_name: "Alpaca".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: 9, quote_decimals: 2, price_decimals: 2,
             minimum_base_size: bd("0.000027"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true, }
}
fn venue(script: Value) -> (ScriptedTransport, AlpacaVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn status(code: u16, body: Value) -> Value { json!([{ "status": code, "body": body }]) }
fn market(volume: &str) -> NewOrder {
    // A current deadline: the real transport refuses a POST past deadline + 45 s (its send bound).
    NewOrder { pair: "BTC/USD".into(), kind: OrderKind::Market, volume: volume.into(), quote_volume: true, cl_ord_id: "c-1".into(),
               deadline: chrono::Utc::now() + chrono::Duration::seconds(10), day: false, }
}

#[test]
fn paper_whatever_the_passphrase_before_3_0() {
    for p in [Some("live"), None, Some("paper"), Some("Live"), Some("live "), Some("")] {
        let u = Urls::for_passphrase(p);
        assert_eq!((u.trading.as_str(), u.data.as_str()), (PAPER_TRADING_URL, DATA_URL), "{p:?}");
        assert!(![&u.trading, &u.data].iter().any(|h| h.contains("//api.alpaca.markets")), "never the live host: {p:?}");
    }
}

#[test]
fn every_recorded_rails_order_parse_is_reproduced() {
    let cases = common::vectors()["alpaca_orders"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 39);
    for c in cases {
        let (body, want) = (&c[0], &c[1]);
        let s = alpaca::parse_order("O1", body).unwrap();
        let status = match s.status { OrderStatus::Open => "open", OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled",
                                      OrderStatus::Failed => "failed", OrderStatus::Unknown => "unknown" };
        assert_eq!(status, want["status"], "status {c}");
        let d = |x: &Option<BigDec>| x.as_ref().map(BigDec::to_s_f);
        assert_eq!(d(&s.price).as_deref(), want["price"].as_str(), "price {c}");
        assert_eq!(d(&s.amount).as_deref(), want["amount"].as_str(), "amount {c}");
        assert_eq!(d(&s.quote_amount).as_deref(), want["quote_amount"].as_str(), "quote_amount {c}");
        assert_eq!(s.amount_exec.to_s_f(), want["amount_exec"], "amount_exec {c}");
        assert_eq!(s.quote_amount_exec.to_s_f(), want["quote_amount_exec"], "quote_amount_exec {c}");
        assert_eq!(s.limit, want["order_type"] == "limit_order", "order_type {c}");
        assert_eq!(s.sell, want["side"] == "sell", "side {c}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn one_side_per_call_and_a_zero_or_missing_price_is_rails_error() {
    let (t, v) = venue(json!({
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": 64321.5, "bp": 64300.25 } } })),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": { "BTC/USD": { "p": 64310.75 } } })) }));
    assert_eq!(v.price(&btc_usd(), PriceSide::Ask).await, Ok(bd("64321.5")));
    assert_eq!(v.price(&btc_usd(), PriceSide::Last).await, Ok(bd("64310.75")));
    let r = &t.requests()[0];
    assert_eq!((r.base.as_str(), r.query.clone()), (DATA_URL, vec![("symbols", "BTC/USD".to_string())]));
    for body in [json!({ "quotes": { "BTC/USD": { "ap": 0 } } }), json!({ "quotes": {} }), json!({})] {
        let (_, v) = venue(json!({ "GET /v1beta3/crypto/us/latest/quotes": ok(body.clone()) }));
        let message = if body["quotes"]["BTC/USD"]["ap"] == 0 { "Wrong ask price for BTC: 0.0" } else { r#"invalid value for BigDecimal(): """# };
        assert_eq!(v.price(&btc_usd(), PriceSide::Ask).await, Err(VenueError::Rejected(vec![message.into()])));
    }
    let (_, v) = venue(json!({ "GET /v1beta3/crypto/us/latest/trades": status(401, json!("<html><body>401 Authorization Required</body></html>")) }));
    assert_eq!(v.price(&btc_usd(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["HTTP 401".into()])));
    let (_, v) = venue(json!({ "GET /v1beta3/crypto/us/latest/quotes": [{ "network": "pre_send", "message": "Faraday::ConnectionFailed: x" }] }));
    assert_eq!(v.price(&btc_usd(), PriceSide::Ask).await, Err(VenueError::Transient("Faraday::ConnectionFailed: x".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn a_market_buy_is_notional_and_a_limit_buy_is_qty_and_limit_price_each_with_a_client_order_id() {
    let (t, v) = venue(json!({ "POST /v2/orders": ok(json!({ "id": "OTX-1", "status": "pending_new" })) }));
    assert_eq!(v.add_order(&market("60.00")).await, Ok("OTX-1".into()));
    let limit = NewOrder { kind: OrderKind::Limit { price: "64149.97".into() }, volume: "0.000935308".into(), quote_volume: false, ..market("") };
    v.add_order(&limit).await.unwrap();
    let bodies: Vec<Value> = t.requests().into_iter().map(|r| r.body.unwrap()).collect();
    assert_eq!(bodies[0], json!({ "symbol": "BTC/USD", "side": "buy", "type": "market", "time_in_force": "gtc", "notional": "60.00", "client_order_id": "c-1" }));
    assert_eq!(bodies[1], json!({ "symbol": "BTC/USD", "side": "buy", "type": "limit", "time_in_force": "gtc", "qty": "0.000935308", "limit_price": "64149.97", "client_order_id": "c-1" }));
    assert_eq!(bodies[0].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["symbol", "side", "type", "time_in_force", "notional", "client_order_id"], "Rails' key order");
    assert_eq!(t.requests()[0].base, PAPER_TRADING_URL);
}

#[tokio::test(flavor = "current_thread")]
async fn a_refusal_carries_rails_message() {
    let cases = [
        (403, json!({ "buying_power": "0", "code": 40310000, "cost_basis": "60", "message": "insufficient buying power" }), "insufficient buying power"),
        (401, json!({ "code": 40110000, "message": "unauthorized." }), "unauthorized."),
        (429, json!({ "code": 42910000, "message": "rate limit exceeded" }), "rate limit exceeded"),
        (422, json!("qty must be >= 0.000027"), "qty must be >= 0.000027"),
        (422, json!({ "code": 42210000 }), r#"{"code":42210000}"#),
        (403, json!("<HTML><body>Forbidden</body></HTML>"), "HTTP 403"),
    ];
    for (code, body, message) in cases {
        let (_, v) = venue(json!({ "POST /v2/orders": status(code, body) }));
        assert_eq!(v.add_order(&market("60.00")).await, Err(VenueError::Rejected(vec![message.into()])), "{code} {message}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_post_that_may_have_landed_is_ambiguous() {
    let cases = [
        (status(500, json!({ "code": 50010000, "message": "internal server error" })), "internal server error".to_string()),
        (status(502, json!("")), format!("the server responded with status 502 for POST {PAPER_TRADING_URL}/v2/orders")),
        (json!([{ "network": "post_send", "message": "Faraday::TimeoutError: Net::ReadTimeout" }]), "Faraday::TimeoutError: Net::ReadTimeout".to_string()),
    ];
    for (reply, message) in cases {
        let (_, v) = venue(json!({ "POST /v2/orders": reply }));
        assert_eq!(v.add_order(&market("60.00")).await, Err(VenueError::Ambiguous(message)));
    }
    for reply in [ok(json!("upstream connect error")), ok(json!({ "status": "accepted" }))] {
        let (_, v) = venue(json!({ "POST /v2/orders": reply }));
        assert!(matches!(v.add_order(&market("60.00")).await, Err(VenueError::Ambiguous(_))), "a 2xx without a readable id");
    }
    let (_, v) = venue(json!({ "POST /v2/orders": [{ "network": "pre_send", "message": "Faraday::ConnectionFailed: refused" }] }));
    assert_eq!(v.add_order(&market("60.00")).await, Err(VenueError::Transient("Faraday::ConnectionFailed: refused".into())), "never sent: retryable");
    let (_, v) = venue(json!({ "POST /v2/orders": status(307, json!("")) }));
    assert!(matches!(v.add_order(&market("60.00")).await, Err(VenueError::Ambiguous(_))), "a redirected order is not followed and not settled");
}

#[tokio::test(flavor = "current_thread")]
async fn permanent_transport_failures_are_failures_as_client_network_failure_returns_them() {
    let cert = || json!([{ "network": "permanent", "message": "Faraday::SSLError: certificate verify failed" }]);
    let (_, v) = venue(json!({ "POST /v2/orders": cert() }));
    assert_eq!(v.add_order(&market("60.00")).await, Err(VenueError::Rejected(vec!["Faraday::SSLError: certificate verify failed".into()])), "Rails writes a failed row");
    let (_, v) = venue(json!({ "GET /v2/account": cert() }));
    assert_eq!(v.balance("USD", true).await, Err(VenueError::Rejected(vec!["Faraday::SSLError: certificate verify failed".into()])), "a Failure: Fundable reads it as not low");
    let (_, v) = venue(json!({ "GET /v2/orders/A": status(301, json!("")) }));
    assert!(matches!(v.orders(&["A".into()]).await, Err(VenueError::Rejected(_))), "a 3xx read is a definitive non-success");
}

#[tokio::test(flavor = "current_thread")]
async fn orders_are_fetched_one_by_one_and_the_first_failure_is_the_answer() {
    let (t, v) = venue(json!({
        "GET /v2/orders/A": ok(json!({ "id": "A", "status": "filled", "type": "market", "side": "buy", "notional": "60", "qty": null, "filled_qty": "0.000932719", "filled_avg_price": "64328.1", "limit_price": null })),
        "GET /v2/orders/B": status(500, json!({ "message": "internal server error" })),
        "GET /v2/orders/C": ok(json!({ "id": "C", "status": "new" })) }));
    assert_eq!(v.orders(&["A".into(), "B".into(), "C".into()]).await, Err(VenueError::Rejected(vec!["internal server error".into()])));
    assert_eq!(t.requests().iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/v2/orders/A", "/v2/orders/B"], "C is never asked");
    let a = v.orders(&["A".into()]).await.unwrap().remove(0);
    assert_eq!((a.status, a.amount_exec.clone(), a.quote_amount_exec.clone()), (OrderStatus::Closed, bd("0.000932719"), &bd("0.000932719") * &bd("64328.1")));
}

#[tokio::test(flavor = "current_thread")]
async fn only_alpacas_own_404_proves_absence() {
    let found = json!({ "id": "OTX-9", "client_order_id": "c-1", "status": "filled", "type": "market", "side": "buy", "notional": "60", "qty": null, "filled_qty": "0.001", "filled_avg_price": "60000", "limit_price": null });
    let since = "2026-09-01T09:00:00Z".parse().unwrap();
    let (t, v) = venue(json!({ "GET /v2/orders:by_client_order_id": ok(found) }));
    assert_eq!(v.order_by_client_id("c-1", since).await.unwrap().map(|o| o.txid), Some("OTX-9".into()));
    assert_eq!(t.requests()[0].query, vec![("client_order_id", "c-1".to_string())]);
    assert!(v.order_by_client_id("c-2", since).await.is_err(), "an answer about another client order id proves nothing");
    let (_, v) = venue(json!({ "GET /v2/orders:by_client_order_id": status(404, json!({ "code": 40410000, "message": "order not found for c-1" })) }));
    assert_eq!(v.order_by_client_id("c-1", since).await, Ok(None));
    for reply in [status(404, json!("<html>Not Found</html>")), status(404, json!({})), status(404, json!({ "message": "route not found" })),
                  status(404, json!({ "code": 40410000, "message": "route not found" })), status(500, json!({ "message": "internal server error" })),
                  json!([{ "network": "post_send", "message": "timeout" }])] {
        let (_, v) = venue(json!({ "GET /v2/orders:by_client_order_id": reply }));
        assert!(v.order_by_client_id("c-1", since).await.is_err(), "incomplete: the intent must stay");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_spendable_balance_is_non_marginable_buying_power_else_cash() {
    let positions = ok(json!([{ "symbol": "BTCUSD", "qty": "0.5" }]));
    let (t, v) = venue(json!({ "GET /v2/account": ok(json!({ "cash": "100", "buying_power": "400", "non_marginable_buying_power": "80" })), "GET /v2/positions": positions.clone() }));
    assert_eq!(v.balance("USD", true).await, Ok(bd("80")));
    assert_eq!(t.requests().iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/v2/account", "/v2/positions"]);
    let (_, v) = venue(json!({ "GET /v2/account": ok(json!({ "cash": "100", "buying_power": "400" })), "GET /v2/positions": positions }));
    assert_eq!(v.balance("USD", true).await, Ok(bd("100")), "an absent field falls back to cash");
    let (_, v) = venue(json!({ "GET /v2/account": ok(json!({ "cash": "100" })), "GET /v2/positions": status(500, json!({ "message": "internal server error" })) }));
    assert_eq!(v.balance("USD", true).await, Err(VenueError::Rejected(vec!["internal server error".into()])), "#get_balances returns the positions failure");
}

#[tokio::test(flavor = "current_thread")]
async fn there_is_no_trade_fallback() {
    let (t, v) = venue(json!({}));
    assert_eq!(v.fills_from_trades(&["A".into()], "2026-09-01T09:00:00Z".parse().unwrap()).await, Ok(vec![]));
    assert!(t.requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn the_real_transport_speaks_to_alpaca_shaped_endpoints() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).and(query_param("symbols", "BTC/USD"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"quotes":{"BTC/USD":{"ap":64321.5}}}"#)).mount(&server).await;
    Mock::given(method("POST")).and(path("/v2/orders"))
        .and(body_json(json!({ "symbol": "BTC/USD", "side": "buy", "type": "market", "time_in_force": "gtc", "notional": "60.00", "client_order_id": "c-1" })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"OTX-1","status":"pending_new"}"#)).expect(1).mount(&server).await;
    let v = AlpacaVenue::new(ReqwestTransport::new(client(), "PKTEST".into(), "s".into()), Urls { trading: server.uri(), data: server.uri() });
    assert_eq!(v.price(&btc_usd(), PriceSide::Ask).await, Ok(bd("64321.5")));
    assert_eq!(v.add_order(&market("60.00")).await, Ok("OTX-1".into()));
}

// Paper only before 3.0: the factory never connects a live key or another exchange, and preflight refuses both before run claims.
#[tokio::test(flavor = "current_thread")]
async fn live_factory_connects_paper_only() {
    use deltabadger::crypto::Credentials;
    use deltabadger::venue::VenueFactory;
    let f = alpaca::LiveFactory::new();
    let creds = |p: Option<&str>| Some(Credentials { key: "PK".into(), secret: "s".into(), passphrase: p.map(str::to_string) });
    for p in [Some("paper"), None, Some("Live"), Some("live ")] {
        assert_eq!(f.for_bot("Exchanges::Alpaca", creds(p)).urls().trading, PAPER_TRADING_URL, "{p:?}");
    }
    let live = f.for_bot("Exchanges::Alpaca", creds(Some("live")));
    assert_eq!(live.urls().trading, PAPER_TRADING_URL, "never the live host");
    match live.price(&btc_usd(), PriceSide::Ask).await {
        Err(VenueError::Transient(m)) => assert_eq!(m, alpaca::LIVE_REFUSED),
        other => panic!("a live key sends nothing: {other:?}"),
    }
    match f.for_bot("Exchanges::Kraken", creds(None)).price(&btc_usd(), PriceSide::Ask).await {
        Err(VenueError::Transient(m)) => assert!(m.contains("Exchanges::Kraken is not connected"), "{m}"),
        other => panic!("another exchange sends nothing: {other:?}"),
    }
}

#[test]
fn preflight_refuses_a_live_key_and_an_undecryptable_passphrase() {
    use common::seed::{self, BotSpec};
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    assert_eq!(alpaca::preflight(&o.primary, &seed::cipher()), Ok(vec![id]));
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")]).unwrap();
    let e = alpaca::preflight(&o.primary, &seed::cipher()).unwrap_err();
    assert!(e.iter().any(|p| p == &format!("bot {id}: {}", alpaca::LIVE_REFUSED)), "{e:?}");
    // Only the passphrase is unreadable: it must never default to paper.
    let other = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-instance").unwrap());
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [other.encrypt("paper")]).unwrap();
    let e = alpaca::preflight(&o.primary, &seed::cipher()).unwrap_err();
    assert!(e.iter().any(|p| p.starts_with(&format!("bot {id}:")) && p.contains("api key unreadable")), "{e:?}");
}

fn aapl() -> Ticker {
    Ticker { id: 3, ticker: "AAPL".into(), base_code: "AAPL".into(), base_symbol: "AAPL".into(), crypto: false, base_decimals: 9, quote_decimals: 2,
             price_decimals: 2, minimum_base_size: bd("0.000000001"), minimum_quote_size: bd("1"), ..btc_usd() }
}
#[tokio::test(flavor = "current_thread")]
async fn a_stock_is_priced_by_its_base_in_the_path_with_no_feed() {
    let (t, v) = venue(json!({
        "GET /v2/stocks/AAPL/quotes/latest": ok(json!({ "symbol": "AAPL", "quote": { "ap": 187.43, "bp": 187.4, "t": "2026-09-01T14:00:00Z" } })),
        "GET /v2/stocks/AAPL/trades/latest": ok(json!({ "symbol": "AAPL", "trade": { "p": 187.41, "t": "2026-09-01T14:00:00Z" } })),
    }));
    assert_eq!(v.price(&aapl(), PriceSide::Ask).await.unwrap(), bd("187.43"));
    assert_eq!(v.price(&aapl(), PriceSide::Last).await.unwrap(), bd("187.41"));
    for r in t.requests() {
        assert_eq!(r.base, DATA_URL, "the market data host");
        assert!(r.query.is_empty(), "Rails sends no feed and no symbols parameter: {:?}", r.query);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_zero_or_missing_stock_quote_is_rails_error() {
    let (_t, v) = venue(json!({ "GET /v2/stocks/AAPL/quotes/latest": ok(json!({ "quote": { "ap": 0, "bp": 0 } })),
                                "GET /v2/stocks/AAPL/trades/latest": ok(json!({ "trade": null })) }));
    assert_eq!(v.price(&aapl(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Wrong ask price for AAPL: 0.0".into()])));
    assert_eq!(v.price(&aapl(), PriceSide::Last).await, Err(VenueError::Rejected(vec![r#"invalid value for BigDecimal(): """#.into()])));
}

#[tokio::test(flavor = "current_thread")]
async fn a_stock_buy_is_a_day_order_and_a_crypto_buy_stays_gtc() {
    let (t, v) = venue(json!({ "POST /v2/orders": ok(json!({ "id": "O1", "status": "accepted" })) }));
    v.add_order(&NewOrder { pair: "AAPL".into(), day: true, ..market("60.00") }).await.unwrap();
    v.add_order(&market("60.00")).await.unwrap();
    let tif: Vec<Value> = t.posted_orders().iter().map(|b| b["time_in_force"].clone()).collect();
    assert_eq!(tif, vec![json!("day"), json!("gtc")]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stock_bot_spends_buying_power_and_a_crypto_bot_non_marginable_buying_power() {
    let (_t, v) = venue(json!({ "GET /v2/account": ok(json!({ "cash": "10", "buying_power": "200", "non_marginable_buying_power": "100" })),
                                "GET /v2/positions": ok(json!([])) }));
    assert_eq!(v.balance("USD", false).await.unwrap(), bd("200"));
    assert_eq!(v.balance("USD", true).await.unwrap(), bd("100"));
    let (_t, v) = venue(json!({ "GET /v2/account": ok(json!({ "cash": "10" })), "GET /v2/positions": ok(json!([])) }));
    assert_eq!(v.balance("USD", false).await.unwrap(), bd("10"), "an absent field falls back to cash (`balance[key] || balance[:free]`)");
}
