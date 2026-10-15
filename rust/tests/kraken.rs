//! The Kraken adapter (venue::kraken) over recorded bodies, its signer against the legacy gem's own output, and the
//! per-venue dispatch and proxy (venue::live). Rails-free: no install, no network beyond local wiremock servers.
use deltabadger::crypto::Credentials;
use deltabadger::engine::model::Ticker;
use deltabadger::engine::venue_rules::KRAKEN;
use deltabadger::ruby::BigDec;
use deltabadger::venue::http::{self, ReqwestTransport, ScriptedTransport};
use deltabadger::venue::kraken::{self, KrakenVenue};
use deltabadger::venue::live::LiveVenue;
use deltabadger::venue::{alpaca, NewOrder, OrderKind, OrderStatus, PriceSide, Venue, VenueError, VenueFactory};
use serde_json::{json, Value};

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn xbt_eur() -> Ticker {
    Ticker { id: 1, ticker: "XBTEUR".into(), base_code: "XBT".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(),
             exchange_name: "Kraken".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 2, price_decimals: 1,
             minimum_base_size: bd("0.0001"), minimum_quote_size: bd("0.5"), trading_enabled: true, available: true, crypto: true }
}
fn at(s: &str) -> chrono::DateTime<chrono::Utc> { s.parse().unwrap() }
fn venue(script: Value) -> (ScriptedTransport, KrakenVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), KrakenVenue::new(t))
}
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn order(kind: OrderKind, quote: bool) -> NewOrder {
    NewOrder { pair: "XBTEUR".into(), kind, volume: "10.0".into(), quote_volume: quote, cl_ord_id: "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01".into(),
               deadline: at("2026-09-01T10:00:10.123456Z"), day: false }
}
fn raw_order(status: &str, cl: &str) -> Value {
    json!({ "status": status, "cl_ord_id": cl, "oflags": "fciq", "vol": "0.001", "vol_exec": "0.001", "cost": "50.0", "price": "50000.0",
            "descr": { "pair": "XBTEUR", "type": "buy", "ordertype": "market", "price": "0" } })
}

// ---- signing: byte-identical to Honeymaker::Clients::Kraken#private_headers (honeymaker a0cc074, crates/honeymaker/tests/vectors)

#[test]
fn every_recorded_gem_signature_is_reproduced() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/kraken_signing.json")).unwrap();
    assert_eq!(cases.len(), 120);
    for case in &cases {
        let pairs: Vec<(String, String)> = case["pairs"].as_array().unwrap().iter()
            .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into())).collect();
        let body = kraken::www_form(&pairs);
        assert_eq!(body, case["body"], "body for {case}");
        let mut got = kraken::private_headers(case["path"].as_str().unwrap(), &body, case["api_key"].as_str().unwrap(), case["api_secret"].as_str().unwrap());
        got.sort();
        let mut want: Vec<(String, String)> = case["headers"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().into())).collect();
        want.sort();
        assert_eq!(got, want, "headers for {case}");
    }
}

#[test]
fn nonces_strictly_increase_per_key() {
    let mut last = kraken::next_nonce("nonce-key");
    for _ in 0..10_000 { let n = kraken::next_nonce("nonce-key"); assert!(n > last, "{n} <= {last}"); last = n; }
}

// ---- requests and answers, as Exchanges::Kraken + Honeymaker::Clients::Kraken send and read them

#[tokio::test(flavor = "current_thread")]
async fn a_market_buy_in_quote_posts_the_gems_add_order_fields_plus_client_id_and_deadline() {
    let (t, v) = venue(json!({ "POST /0/private/AddOrder": ok(json!({ "error": [], "result": { "txid": ["OABC-1"] } })) }));
    assert_eq!(v.add_order(&order(OrderKind::Market, true)).await, Ok("OABC-1".into()));
    let r = &t.requests()[0];
    assert_eq!((r.method, r.path.as_str()), ("POST", "/0/private/AddOrder"));
    // Field order is the gem's hash order (nonce, added by the transport, comes first on the wire).
    assert_eq!(r.body.as_ref().unwrap().to_string(),
        r#"{"ordertype":"market","type":"buy","volume":"10.0","pair":"XBTEUR","cl_ord_id":"5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01","oflags":"viqc","deadline":"2026-09-01T10:00:10.123Z"}"#);
    assert!(r.not_after.is_some(), "a placement carries its send bound");
}

#[tokio::test(flavor = "current_thread")]
async fn a_limit_buy_sends_its_price_and_no_oflags() {
    let (t, v) = venue(json!({ "POST /0/private/AddOrder": ok(json!({ "error": [], "result": { "txid": ["OL-1"] } })) }));
    v.add_order(&order(OrderKind::Limit { price: "49999.9".into() }, false)).await.unwrap();
    assert_eq!(t.requests()[0].body.as_ref().unwrap().to_string(),
        r#"{"ordertype":"limit","type":"buy","volume":"10.0","pair":"XBTEUR","cl_ord_id":"5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01","price":"49999.9","deadline":"2026-09-01T10:00:10.123Z"}"#);
}

#[tokio::test(flavor = "current_thread")]
async fn placement_answers_map_to_rails_ambiguity_rules() {
    let cases: Vec<(Value, Result<String, VenueError>)> = vec![
        // Kraken's own refusal: Rejected with its error array (the engine applies VenueRules::add_outcome_unknown).
        (ok(json!({ "error": ["EOrder:Insufficient funds"] })), Err(VenueError::Rejected(vec!["EOrder:Insufficient funds".into()]))),
        (ok(json!({ "error": ["EService:Unavailable"] })), Err(VenueError::Rejected(vec!["EService:Unavailable".into()]))),
        // Accepted with no id (Exchanges::Kraken's `unacknowledged`): may be placed.
        (ok(json!({ "error": [], "result": { "txid": [] } })), Err(VenueError::Ambiguous("Failed to set Kraken market order (order_id is nil)".into()))),
        (ok(json!("<html>maintenance</html>")), Err(VenueError::Ambiguous("Kraken: unreadable response".into()))),
        (ok(json!({ "result": { "txid": ["O1"] } })), Err(VenueError::Ambiguous("Kraken: unreadable response".into()))),
        (json!([{ "status": 502, "body": "bad gateway" }]), Err(VenueError::Ambiguous("bad gateway".into()))),
        (json!([{ "status": 429, "body": "" }]), Err(VenueError::Rejected(vec!["the server responded with status 429".into()]))),
        (json!([{ "network": "post_send", "message": "read timeout" }]), Err(VenueError::Ambiguous("read timeout".into()))),
        (json!([{ "network": "pre_send", "message": "refused" }]), Err(VenueError::Transient("refused".into()))),
        (json!([{ "network": "permanent", "message": "certificate" }]), Err(VenueError::Rejected(vec!["certificate".into()]))),
    ];
    for (reply, want) in cases {
        let (_, v) = venue(json!({ "POST /0/private/AddOrder": reply }));
        assert_eq!(v.add_order(&order(OrderKind::Market, true)).await, want, "{reply}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prices_read_the_first_ticker_entry_and_refuse_a_zero_book() {
    let body = |a: &str, c: &str| ok(json!({ "error": [], "result": { "XXBTZEUR": { "a": [a, "1", "1.000"], "b": ["1", "1", "1.000"], "c": [c, "0.1"] } } }));
    let (t, v) = venue(json!({ "GET /0/public/Ticker": body("50001.5", "50000.1") }));
    assert_eq!(v.price(&xbt_eur(), PriceSide::Ask).await, Ok(bd("50001.5")));
    assert_eq!(v.price(&xbt_eur(), PriceSide::Last).await, Ok(bd("50000.1")));
    assert_eq!(http::request_key(&t.requests()[0]), "GET /0/public/Ticker?pair=XBTEUR");
    let (_, v) = venue(json!({ "GET /0/public/Ticker": body("0", "0.0") }));
    assert_eq!(v.price(&xbt_eur(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Wrong ask price for XBTEUR: 0.0".into()])));
    let (_, v) = venue(json!({ "GET /0/public/Ticker": ok(json!({ "error": [], "result": {} })) }));
    assert_eq!(v.price(&xbt_eur(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Failed to get Kraken XBTEUR ticker information".into()])));
    let (_, v) = venue(json!({ "GET /0/public/Ticker": body("NaN", "1") }));
    assert!(matches!(v.price(&xbt_eur(), PriceSide::Ask).await, Err(VenueError::Ambiguous(_))), "an unreadable price is never a price");
}

#[tokio::test(flavor = "current_thread")]
async fn orders_query_the_ids_joined_and_parse_as_rails() {
    let (t, v) = venue(json!({ "POST /0/private/QueryOrders": ok(json!({ "error": [], "result": { "O1": raw_order("closed", "x"), "OTHER": raw_order("open", "y") } })) }));
    let got = v.orders(&["O1".into(), "O2".into()]).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].txid.as_str(), got[0].status, got[0].amount_exec.clone(), got[0].quote_amount_exec.clone().unwrap()), ("O1", OrderStatus::Closed, bd("0.001"), bd("50.0")));
    assert_eq!(t.requests()[0].body.as_ref().unwrap().to_string(), r#"{"txid":"O1,O2","consolidate_taker":"true"}"#);
    let (t, v) = venue(json!({}));
    assert_eq!(v.orders(&[]).await, Ok(vec![]), "no ids, no request");
    assert!(t.requests().is_empty());
    let (_, v) = venue(json!({ "POST /0/private/QueryOrders": ok(json!({ "error": [], "result": { "O1": raw_order("weird", "x") } })) }));
    assert!(matches!(v.orders(&["O1".into()]).await, Err(VenueError::Ambiguous(m)) if m.contains("Unknown Kraken order status")));
}

#[tokio::test(flavor = "current_thread")]
async fn a_client_order_id_is_found_open_or_closed() {
    let cl = "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01";
    let since = at("2026-09-01T09:00:00Z");
    let (t, v) = venue(json!({ "POST /0/private/OpenOrders": ok(json!({ "error": [], "result": { "open": { "OOPEN": raw_order("open", cl) } } })) }));
    assert_eq!(v.order_by_client_id(cl, "XBTEUR", since).await.unwrap().map(|o| o.txid), Some("OOPEN".into()));
    assert_eq!(t.requests()[0].body.as_ref().unwrap().to_string(), format!(r#"{{"cl_ord_id":"{cl}"}}"#));
    let (t, v) = venue(json!({ "POST /0/private/OpenOrders": ok(json!({ "error": [], "result": { "open": {} } })),
                                "POST /0/private/ClosedOrders": ok(json!({ "error": [], "result": { "closed": { "OC": raw_order("closed", cl) }, "count": 1 } })) }));
    assert_eq!(v.order_by_client_id(cl, "XBTEUR", since).await.unwrap().map(|o| o.txid), Some("OC".into()));
    assert_eq!(t.requests()[1].body.as_ref().unwrap().to_string(), format!(r#"{{"cl_ord_id":"{cl}","start":"{}"}}"#, since.timestamp()));
}

/// Kraken's `cl_ord_id` filter, `count` and echo on Open/ClosedOrders are not confirmed by a recorded live answer, so no
/// answer proves an order absent: a lookup that finds nothing is incomplete, the intent stays, and
/// `deltabadger resolve-placement` settles it.
#[tokio::test(flavor = "current_thread")]
async fn a_lookup_that_finds_nothing_never_proves_absence() {
    let (_, v) = venue(json!({ "POST /0/private/OpenOrders": ok(json!({ "error": [], "result": { "open": { "OX": raw_order("open", "another") } } })),
                                "POST /0/private/ClosedOrders": ok(json!({ "error": [], "result": { "closed": {}, "count": 0 } })) }));
    match v.order_by_client_id("5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01", "XBTEUR", at("2026-09-01T09:00:00Z")).await {
        Err(VenueError::Ambiguous(m)) => assert!(m.contains("absence"), "{m}"),
        other => panic!("absence must stay unproven: {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fills_are_recovered_from_every_trades_page_since_the_bound() {
    let trade = |o: &str, vol: &str, cost: &str| json!({ "ordertxid": o, "pair": "XBTEUR", "type": "buy", "ordertype": "market", "vol": vol, "cost": cost });
    let (t, v) = venue(json!({ "POST /0/private/TradesHistory": [
        { "status": 200, "body": { "error": [], "result": { "trades": { "T1": trade("O1", "0.001", "50"), "T2": trade("O9", "1", "1") }, "count": 3 } } },
        { "status": 200, "body": { "error": [], "result": { "trades": { "T2": trade("O9", "1", "1"), "T3": trade("O1", "0.002", "100") }, "count": 3 } } } ] }));
    let since = at("2026-09-01T09:00:00Z");
    let got = v.fills_from_trades(&["O1".into()], since).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].status, got[0].amount_exec.clone(), got[0].quote_amount_exec.clone().unwrap(), got[0].price.clone()), (OrderStatus::Closed, bd("0.003"), bd("150"), Some(bd("50000"))));
    let bodies: Vec<String> = t.requests().iter().map(|r| r.body.as_ref().unwrap().to_string()).collect();
    assert_eq!(bodies, vec![format!(r#"{{"start":"{0}","ofs":"0"}}"#, since.timestamp()), format!(r#"{{"start":"{0}","ofs":"2"}}"#, since.timestamp())]);
}

#[tokio::test(flavor = "current_thread")]
async fn the_free_balance_is_balance_less_hold_trade_under_the_gems_asset_map() {
    let (_, v) = venue(json!({ "POST /0/private/BalanceEx": ok(json!({ "error": [], "result": { "ZEUR": { "balance": "100.5", "hold_trade": "20.25" }, "XXBT": { "balance": "1", "hold_trade": "0" } } })) }));
    assert_eq!(v.balance("EUR", true).await, Ok(bd("80.25")));
    assert_eq!(v.balance("XBT", true).await, Ok(bd("1")));
    assert_eq!(v.balance("USD", true).await, Ok(BigDec::zero()));
}

// ---- the real transport: what reaches the wire

#[tokio::test(flavor = "current_thread")]
async fn the_real_transport_signs_private_calls_and_sends_public_ones_unsigned() {
    use wiremock::matchers::{body_string_contains, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let secret = "dGVzdF9zZWNyZXRfa2V5XzEyMzQ1Njc4OTAxMjM0NTY=";
    Mock::given(method("POST")).and(path("/0/private/BalanceEx")).and(header("API-Key", "KEY")).and(header_exists("API-Sign"))
        .and(header("content-type", "application/x-www-form-urlencoded")).and(header("user-agent", "Honeymaker Ruby")).and(body_string_contains("nonce="))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"error":[],"result":{"ZEUR":{"balance":"5","hold_trade":"1"}}}"#)).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/0/public/Ticker")).and(header("user-agent", "Honeymaker Ruby"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"error":[],"result":{"X":{"a":["2","1","1"],"b":["1","1","1"],"c":["1.5","1"]}}}"#)).expect(1).mount(&server).await;
    let v = KrakenVenue::new(ReqwestTransport::new(http::client(), "KEY".into(), secret.into()).kraken()).with_base(&server.uri());
    assert_eq!(v.balance("EUR", true).await, Ok(bd("4")));
    assert_eq!(v.price(&xbt_eur(), PriceSide::Ask).await, Ok(bd("2")));
    // The signature verifies against the body that was sent.
    let sent = &server.received_requests().await.unwrap()[0];
    let body = String::from_utf8(sent.body.clone()).unwrap();
    assert!(body.starts_with("nonce=") && !body.contains('&'), "{body}");
    let want = kraken::private_headers("/0/private/BalanceEx", &body, "KEY", secret).into_iter().find(|(k, _)| k == "API-Sign").unwrap().1;
    assert_eq!(sent.headers.get("API-Sign").unwrap().to_str().unwrap(), want);
    assert!(!server.received_requests().await.unwrap()[1].headers.contains_key("API-Sign"));
    server.verify().await;
}

// ---- per-venue proxy: PROXY_<VENUE> / AppConfig proxy_<venue>, never a direct connection when one is configured

#[test]
fn the_app_config_proxy_wins_over_the_environment_and_blanks_are_absent() {
    // The one ExchangeProxy.for every proxied venue shares (Binance's too).
    let env = |k: &str| (k == "PROXY_KRAKEN").then(|| "http://e:2".to_string());
    assert_eq!(http::proxy_for("kraken", Some("http://a:1".into()), &env), Some("http://a:1".into()));
    assert_eq!(http::proxy_for("kraken", Some("  ".into()), &env), Some("http://e:2".into()));
    assert_eq!(http::proxy_for("kraken", None, &|_| Some("\t".into())), None);
    assert_eq!(http::proxy_for("kraken", None, &|_| None), None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_configured_proxy_carries_every_request_and_a_dead_one_never_falls_back_to_direct() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let proxy = MockServer::start().await;
    let direct = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string(r#"{"error":[],"result":{"X":{"a":["3","1","1"],"b":["1","1","1"],"c":["1","1"]}}}"#)).expect(1).mount(&proxy).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&direct).await;
    let via = |p: &str| KrakenVenue::new(ReqwestTransport::new(http::client_via(p).unwrap(), String::new(), String::new()).kraken()).with_base(&direct.uri());
    assert_eq!(via(&proxy.uri()).price(&xbt_eur(), PriceSide::Ask).await, Ok(bd("3")), "through the proxy");
    match via("http://127.0.0.1:9").price(&xbt_eur(), PriceSide::Ask).await {
        Err(VenueError::Transient(_)) => {}
        other => panic!("a dead proxy is a failure that sent nothing: {other:?}"),
    }
    assert!(http::client_via("::not a url::").is_err(), "an unusable proxy is refused, never ignored");
    proxy.verify().await;
    direct.verify().await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_proxy_changed_in_app_config_is_used_by_the_next_client_without_a_restart() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let ticker = r#"{"error":[],"result":{"X":{"a":["3","1","1"],"b":["1","1","1"],"c":["1","1"]}}}"#;
    let (a, b, direct) = (MockServer::start().await, MockServer::start().await, MockServer::start().await);
    for server in [&a, &b, &direct] {
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_string(ticker)).expect(1).mount(server).await;
    }
    let dir = tempfile::tempdir().unwrap();
    let c = rusqlite::Connection::open(dir.path().join("primary.sqlite3")).unwrap();
    c.execute_batch("CREATE TABLE app_configs (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT NOT NULL, value TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)").unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap());
    let factory = alpaca::LiveFactory::for_install(&c, &cipher);
    let price = |f: &alpaca::LiveFactory| {
        let LiveVenue::Kraken(k) = f.for_bot("Exchanges::Kraken", None) else { panic!("a Kraken bot gets the Kraken venue") };
        let k = k.with_base(&direct.uri());
        async move { k.price(&xbt_eur(), PriceSide::Ask).await }
    };
    let now = chrono::Utc::now();
    deltabadger::app_config::set(&c, &cipher, "proxy_kraken", &a.uri(), now).unwrap();
    assert_eq!(price(&factory).await, Ok(bd("3")), "the proxy set after start");
    deltabadger::app_config::set(&c, &cipher, "proxy_kraken", &b.uri(), now).unwrap();
    assert_eq!(price(&factory).await, Ok(bd("3")), "the changed proxy");
    deltabadger::app_config::set(&c, &cipher, "proxy_kraken", " ", now).unwrap();
    assert_eq!(price(&factory).await, Ok(bd("3")), "a blank proxy is none (ExchangeProxy.for's .presence): direct");
    for server in [&a, &b, &direct] { server.verify().await; }
}

#[tokio::test(flavor = "current_thread")]
async fn the_factory_routes_each_bot_to_its_venue_and_an_unported_venue_sends_nothing() {
    let creds = Some(Credentials { redaction_values: vec![], key: "K".into(), secret: "S".into(), passphrase: None });
    let f = alpaca::LiveFactory::new();
    assert!(matches!(f.for_bot("Exchanges::Alpaca", creds.clone()), LiveVenue::Alpaca(_)));
    let kraken = f.for_bot("Exchanges::Kraken", creds.clone());
    assert!(std::ptr::eq(kraken.rules(), &KRAKEN), "a Kraken bot gets Kraken's rules");
    // Every venue states the key an order goes under, so recovery never asks another key's account (spec E2 T6).
    assert_eq!(kraken.placement_identity().unwrap()["host"], kraken::URL);
    assert_eq!(kraken.placement_identity().unwrap()["key_fingerprint"].as_str().map(str::len), Some(64));
    // A factory that has no install to read proxy_kraken from cannot know whether one is configured: it never goes direct.
    match kraken.price(&xbt_eur(), PriceSide::Ask).await {
        Err(VenueError::Transient(m)) => assert!(m.contains("proxy"), "{m}"),
        other => panic!("{other:?}"),
    }
    match f.for_bot("Exchanges::Coinbase", creds).price(&xbt_eur(), PriceSide::Ask).await {
        Err(VenueError::Transient(m)) => assert!(m.contains("Exchanges::Coinbase is not connected"), "{m}"),
        other => panic!("{other:?}"),
    }
}


// ---- review fixes: malformed recovered fills, the relabelled log line, read-side HTTP throttling

#[tokio::test(flavor = "current_thread")]
async fn a_recovered_trade_without_readable_amounts_leaves_the_order_unresolved() {
    // QueryOrders omits O1 (engine::polling then asks TradesHistory); the trade names O1 with no readable vol/cost. It must be
    // an unreadable answer (the poll fails, the order stays waiting), never a closed zero fill that frees its money.
    let shapes: Vec<Value> = vec![
        json!({ "ordertxid": "O1", "type": "buy", "ordertype": "market", "vol": null, "cost": null }),
        json!({ "ordertxid": "O1", "type": "buy", "ordertype": "market" }),
        json!({ "ordertxid": "O1", "type": "buy", "ordertype": "market", "vol": "0.001" }),
        json!({ "ordertxid": "O1", "type": "buy", "ordertype": "market", "cost": "60" }),
        json!({ "ordertxid": "O1", "type": "buy", "ordertype": "market", "vol": "garbage", "cost": "60" }),
        json!({ "ordertxid": "O1", "type": "buy", "ordertype": "market", "vol": "0.001", "cost": "" }),
    ];
    for trade in shapes {
        let (_, v) = venue(json!({
            "POST /0/private/QueryOrders": ok(json!({ "error": [], "result": {} })),
            "POST /0/private/TradesHistory": ok(json!({ "error": [], "result": { "count": 1, "trades": { "T1": trade.clone() } } })) }));
        assert_eq!(v.orders(&["O1".into()]).await, Ok(vec![]), "QueryOrders omits the order");
        assert_eq!(v.fills_from_trades(&["O1".into()], at("2026-09-01T09:00:00Z")).await, Err(VenueError::Ambiguous(kraken::UNREADABLE.into())), "{trade}");
    }
}

#[test]
fn the_relabelled_log_line_is_redacted_as_a_whole() {
    let t = ReqwestTransport::new(http::client(), "KEY".into(), "SECRET".into()).with_sensitive_values(vec!["[Kraken] HTTP 500".into()]).kraken();
    let line = t.failure_line(&http::HttpResponse { status: 500, body: String::new() });
    assert!(!line.contains("[Kraken] HTTP 500"), "{line}");
    assert_eq!(line, deltabadger::crypto::VENUE_TEXT_REDACTED);
    let plain = ReqwestTransport::new(http::client(), "KEY".into(), "SECRET".into()).kraken();
    assert_eq!(plain.failure_line(&http::HttpResponse { status: 500, body: String::new() }), "[Kraken] HTTP 500; venue response diagnostic omitted");
}

#[tokio::test(flavor = "current_thread")]
async fn an_http_throttle_on_a_read_is_rate_limited_and_on_a_placement_stays_definitive() {
    for status in [429, 418] {
        let throttled = json!([{ "status": status, "body": "" }]);
        let (_, v) = venue(json!({ "POST /0/private/QueryOrders": throttled.clone(), "POST /0/private/BalanceEx": throttled.clone(),
                                   "GET /0/public/Ticker": throttled.clone(), "POST /0/private/TradesHistory": throttled.clone() }));
        let reads = [v.orders(&["O1".into()]).await.map(|_| ()), v.balance("EUR", true).await.map(|_| ()),
                     v.price(&xbt_eur(), PriceSide::Ask).await.map(|_| ()), v.fills_from_trades(&["O1".into()], at("2026-09-01T09:00:00Z")).await.map(|_| ())];
        for read in reads {
            let Err(VenueError::Rejected(errs)) = read else { panic!("HTTP {status} on a read: {read:?}") };
            assert!(KRAKEN.is_throttle(&errs), "HTTP {status} on a read must be RateLimited: {errs:?}");
        }
        let (_, v) = venue(json!({ "POST /0/private/AddOrder": throttled }));
        let Err(VenueError::Rejected(errs)) = v.add_order(&order(OrderKind::Market, true)).await else { panic!("HTTP {status} on a placement is definitive") };
        assert!(!KRAKEN.is_throttle(&errs), "never RateLimited on a placement: {errs:?}");
    }
}
