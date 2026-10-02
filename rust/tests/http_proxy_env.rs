//! Its own test binary on purpose: it sets process env (HTTP(S)_PROXY), which is a data race against any test running in
//! parallel in the same process. Alone in this binary, nothing else reads the env while it runs.
use deltabadger::venue::http::*;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(base: &str, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<serde_json::Value>) -> HttpRequest {
    HttpRequest { method, base: base.into(), path: path.into(), query, body, not_after: None }
}
fn transport() -> ReqwestTransport { ReqwestTransport::new(client(), "PKTEST".into(), "s3cret".into()) }

#[tokio::test(flavor = "current_thread")]
async fn a_read_carries_rails_headers_and_encodes_the_pair_as_faraday_does() {
    // Rails reaches Alpaca directly: a proxy in the environment must not be picked up (every test here uses no_proxy).
    std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");
    std::env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).and(query_param("symbols", "BTC/USD"))
        .and(header("APCA-API-KEY-ID", "PKTEST")).and(header("APCA-API-SECRET-KEY", "s3cret")).and(header("user-agent", "Ruby"))
        .and(header("accept", "application/json")).and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"quotes":{}}"#)).expect(1).mount(&server).await;
    let r = req(&server.uri(), "GET", "/v1beta3/crypto/us/latest/quotes", vec![("symbols", "BTC/USD".into())], None);
    assert_eq!(r.url().as_str(), format!("{}/v1beta3/crypto/us/latest/quotes?symbols=BTC%2FUSD", server.uri()));
    assert_eq!(transport().send(&r).await.unwrap(), HttpResponse { status: 200, body: r#"{"quotes":{}}"#.into() });
}
