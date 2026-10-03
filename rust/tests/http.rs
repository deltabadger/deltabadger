use deltabadger::engine::venue_rules::ALPACA;
use deltabadger::venue::http::*;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(base: &str, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<serde_json::Value>) -> HttpRequest {
    HttpRequest { method, base: base.into(), path: path.into(), query, body, not_after: None }
}
fn transport() -> ReqwestTransport { ReqwestTransport::new(client(), "PKTEST".into(), "s3cret".into()) }

#[tokio::test(flavor = "current_thread")]
async fn a_post_sends_its_json_body_and_any_status_comes_back_with_its_body() {
    let server = MockServer::start().await;
    let body = json!({ "symbol": "BTC/USD", "side": "buy", "type": "market", "time_in_force": "gtc", "notional": "60.00", "client_order_id": "c-1" });
    Mock::given(method("POST")).and(path("/v2/orders")).and(body_json(&body))
        .respond_with(ResponseTemplate::new(403).set_body_string(r#"{"code":40310000,"message":"insufficient buying power"}"#)).expect(1).mount(&server).await;
    let out = transport().send(&req(&server.uri(), "POST", "/v2/orders", vec![], Some(body))).await.unwrap();
    assert_eq!(out, HttpResponse { status: 403, body: r#"{"code":40310000,"message":"insufficient buying power"}"#.into() });
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_connection_was_never_sent() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(); // bound, then closed again
    let r = req(&format!("http://127.0.0.1:{port}"), "POST", "/v2/orders", vec![], Some(json!({})));
    assert!(matches!(transport().send(&r).await, Err(TransportError::NotSent(_))));
}

#[tokio::test(flavor = "current_thread")]
async fn a_connection_lost_after_the_request_may_have_been_sent() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let _ = std::io::Read::read(&mut socket, &mut buf); // the request arrives, then the connection drops unanswered
    });
    let r = req(&base, "POST", "/v2/orders", vec![], Some(json!({ "symbol": "BTC/USD" })));
    assert!(matches!(transport().send(&r).await, Err(TransportError::MaybeSent(_))));
}

#[tokio::test(flavor = "current_thread")]
async fn a_reply_slower_than_the_budget_may_have_been_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2))).mount(&server).await;
    let t = ReqwestTransport::new(client_with(Duration::from_secs(5), Duration::from_millis(200), Duration::from_secs(1)), "k".into(), "s".into());
    assert!(matches!(t.send(&req(&server.uri(), "POST", "/v2/orders", vec![], Some(json!({})))).await, Err(TransportError::MaybeSent(_))));
}

#[tokio::test(flavor = "current_thread")]
async fn a_redirect_is_an_answer_and_never_reaches_a_second_endpoint() {
    let elsewhere = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"ELSEWHERE"}"#)).expect(0).mount(&elsewhere).await;
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v2/orders"))
        .respond_with(ResponseTemplate::new(307).insert_header("Location", format!("{}/v2/orders", elsewhere.uri()).as_str())).expect(1).mount(&server).await;
    let out = transport().send(&req(&server.uri(), "POST", "/v2/orders", vec![], Some(json!({ "symbol": "BTC/USD" })))).await.unwrap();
    assert_eq!(out.status, 307, "returned as it came; the order is not re-sent to the Location");
}

#[tokio::test(flavor = "current_thread")]
async fn a_tls_protocol_failure_is_permanent_as_rails_ssl_errors_are() {
    // TLS to a plain-HTTP server: the handshake fails on a protocol error, as a certificate failure does, before any request.
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
    let base = server.uri().replace("http://", "https://");
    let out = transport().send(&req(&base, "GET", "/v2/account", vec![], None)).await;
    assert!(matches!(out, Err(TransportError::Permanent(_))), "{out:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_past_its_send_bound_is_never_sent_and_a_near_bound_shortens_the_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3))).expect(1).mount(&server).await;
    let late = HttpRequest { not_after: Some(chrono::Utc::now() - chrono::Duration::seconds(1)), ..req(&server.uri(), "POST", "/v2/orders", vec![], Some(json!({}))) };
    assert!(matches!(transport().send(&late).await, Err(TransportError::NotSent(_))), "resumed past its bound: nothing sent");
    let near = HttpRequest { not_after: Some(chrono::Utc::now() + chrono::Duration::milliseconds(500)), ..late };
    let t0 = std::time::Instant::now();
    assert!(matches!(transport().send(&near).await, Err(TransportError::MaybeSent(_))));
    assert!(t0.elapsed() < Duration::from_secs(2), "the request gave up at its bound, not after 45 s: {:?}", t0.elapsed());
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_transport_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
    let out = ReqwestTransport::refused("live Alpaca trading is not enabled").send(&req(&server.uri(), "POST", "/v2/orders", vec![], Some(json!({})))).await;
    assert_eq!(out, Err(TransportError::NotSent("live Alpaca trading is not enabled".into())));
}

#[test]
fn the_absence_window_is_the_send_window_plus_the_whole_request_budget() {
    assert_eq!(ALPACA.reach_within_secs, deltabadger::engine::placement::SEND_WINDOW_SECONDS + TOTAL_TIMEOUT.as_secs() as i64, "send window + the whole request budget");
    assert_eq!(TOTAL_TIMEOUT, CONNECT_TIMEOUT + Duration::from_secs(10) + READ_TIMEOUT, "Client::OPTIONS: open 5 + write 10 + read 30");
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_replays_in_order_repeats_the_last_and_strips_the_client_order_id() {
    let t = ScriptedTransport::from_script(&json!({ "POST /v2/orders": [
        { "status": 200, "body": { "id": "A" } },
        { "network": "post_send", "message": "Faraday::TimeoutError: Net::ReadTimeout" }] }));
    let post = req("https://paper-api.alpaca.markets", "POST", "/v2/orders", vec![], Some(json!({ "symbol": "BTC/USD", "client_order_id": "c-1" })));
    assert_eq!(t.send(&post).await.unwrap().body, r#"{"id":"A"}"#);
    assert_eq!(t.send(&post).await, Err(TransportError::MaybeSent("Faraday::TimeoutError: Net::ReadTimeout".into())));
    assert_eq!(t.send(&post).await, Err(TransportError::MaybeSent("Faraday::TimeoutError: Net::ReadTimeout".into())), "the last reply repeats");
    t.network("GET /v2/positions", "permanent", "Faraday::SSLError: certificate verify failed");
    assert_eq!(t.send(&req("https://paper-api.alpaca.markets", "GET", "/v2/positions", vec![], None)).await,
               Err(TransportError::Permanent("Faraday::SSLError: certificate verify failed".into())));
    assert_eq!(t.posted_orders(), vec![json!({ "symbol": "BTC/USD" }); 3]);
    t.reply("GET /v2/account", 200, json!("not json"));
    assert_eq!(t.send(&req("https://paper-api.alpaca.markets", "GET", "/v2/account", vec![], None)).await.unwrap().body, "not json", "a string body is raw text");
}

#[tokio::test(flavor = "current_thread")]
#[should_panic(expected = "unscripted Alpaca call GET /v2/clock")]
async fn an_unscripted_call_is_loud() {
    let _ = ScriptedTransport::default().send(&req("https://paper-api.alpaca.markets", "GET", "/v2/clock", vec![], None)).await;
}

#[test]
fn transport_kind_gates_permanent_on_connect() {
    use std::io::ErrorKind::{InvalidData, PermissionDenied};
    assert_eq!(transport_kind(Some(PermissionDenied), false, true), Kind::Permanent);
    assert_eq!(transport_kind(Some(PermissionDenied), false, false), Kind::MaybeSent);
    assert_eq!(transport_kind(Some(InvalidData), false, true), Kind::Permanent);
    assert_eq!(transport_kind(Some(InvalidData), false, false), Kind::MaybeSent);
    assert_eq!(transport_kind(Some(InvalidData), true, true), Kind::NotSent); // peer closed: not a TLS verdict
    assert_eq!(transport_kind(None, false, true), Kind::NotSent);
    assert_eq!(transport_kind(None, false, false), Kind::MaybeSent); // timeout after connect
}

#[test]
#[should_panic(expected = "unknown network kind")]
fn a_misspelt_network_kind_fails_loudly() {
    let t = ScriptedTransport::default();
    t.network("GET /x", "postsend", "boom");
    let r = req("http://x", "GET", "/x", vec![], None);
    let _ = futures_lite_block(t.send(&r));
}

#[test]
#[should_panic(expected = "has no status")]
fn a_reply_without_a_status_fails_loudly() {
    let t = ScriptedTransport::from_script(&json!({ "GET /x": [{ "body": "{}" }] }));
    let r = req("http://x", "GET", "/x", vec![], None);
    let _ = futures_lite_block(t.send(&r));
}

fn futures_lite_block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
}

#[tokio::test(flavor = "current_thread")]
async fn a_scripted_lookup_answers_for_the_client_order_id_asked_for() {
    use deltabadger::venue::Venue;
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders:by_client_order_id": [{ "status": 200, "body": {
        "id": "OTX-L", "client_order_id": "$client_order_id", "status": "filled", "symbol": "ETH/USD", "type": "market", "side": "buy",
        "notional": "36", "qty": null, "filled_qty": "0.0144", "filled_avg_price": "2500", "limit_price": null } }] }));
    let v = deltabadger::venue::alpaca::AlpacaVenue::new(t, deltabadger::venue::alpaca::Urls::for_passphrase(Some("paper")));
    let found = v.order_by_client_id("9b1d2c3e-1111-4000-8000-000000000001", "2026-09-01T10:00:00Z".parse().unwrap()).await;
    assert_eq!(found.unwrap().expect("found").txid, "OTX-L", "a recorded answer cannot know the engine's UUID");
}
