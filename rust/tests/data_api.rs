//! Clients::MarketData in Rust — the provider's resolution, with_rescue's outcomes, the live request's shape.
mod common;
use common::seed;
use deltabadger::app_config;
use deltabadger::jobs::data_api::{self, ApiError, Config, DataApi, PriceSource};
use std::collections::BTreeMap;
use deltabadger::store::{self, Paths};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn db() -> (tempfile::TempDir, rusqlite::Connection) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o.primary)
}
fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
}
fn scripted(t: &ScriptedTransport) -> DataApi<ScriptedTransport> {
    DataApi::new(Config { url: "http://data-api:3000".into(), token: "tok".into() }, t.clone(), t.clone())
}

#[test]
fn the_provider_and_its_credentials_resolve_as_market_data_settings_does() {
    let (_d, c) = db();
    let k = seed::cipher();
    let t = "2026-10-02T10:00:00Z".parse().unwrap();
    assert_eq!(data_api::config(&env(&[]), &c, &k).unwrap(), None, "no provider: the CoinGecko path, not ported");
    assert_eq!(data_api::config(&env(&[("MARKET_DATA_URL", " ")]), &c, &k).unwrap(), None, "a blank variable is not present?");
    let e = env(&[("MARKET_DATA_URL", "http://data-api:3000"), ("MARKET_DATA_TOKEN", "tok")]);
    assert_eq!(data_api::config(&e, &c, &k).unwrap(), Some(Config { url: "http://data-api:3000".into(), token: "tok".into() }));
    app_config::set(&c, &k, "market_data_url", "https://data.example", t).unwrap();
    app_config::set(&c, &k, "market_data_token", "", t).unwrap();
    assert_eq!(data_api::config(&e, &c, &k).unwrap(), Some(Config { url: "https://data.example".into(), token: "".into() }),
               "a row wins over the environment, even a blank one");
    app_config::set(&c, &k, "market_data_provider", "deltabadger", t).unwrap();
    assert!(data_api::config(&env(&[]), &c, &k).unwrap().is_some(), "the database can name the provider");
}

#[tokio::test(flavor = "current_thread")]
async fn with_rescue_outcomes_become_transient_failed_and_rate_limited() {
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/indices", 200, json!({ "data": [] }))
        .reply("GET /api/v2/indices", 502, json!("<html><body>Bad gateway</body></html>"))
        .reply("GET /api/v2/indices", 200, json!("not json"))
        .reply("GET /api/v2/indices", 404, json!({ "error": "Invalid exchange: x" }));
    let api = scripted(&t);
    assert_eq!(api.indices().await.unwrap(), json!({ "data": [] }));
    assert_eq!(api.indices().await.unwrap_err(), ApiError::Failed { status: Some(502), message: "HTTP 502".into() });
    assert_eq!(api.indices().await.unwrap_err(), ApiError::Failed { status: Some(200), message: "Unreadable response (HTTP 200)".into() });
    assert_eq!(api.indices().await.unwrap_err().message(), r#"{"error":"Invalid exchange: x"}"#, "the raw body, as Faraday hands it over");

    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/listings?venue_scheme=alpaca_exchange", 429, json!({ "error": "slow down" }))
        .network("GET /api/v2/listings", "post_send", "Faraday::TimeoutError: Net::ReadTimeout");
    let api = scripted(&t);
    assert!(api.alpaca_listings().await.unwrap_err().rate_limited(), "a key with its query is answered first");
    assert_eq!(api.alpaca_crypto_listings().await.unwrap_err(), ApiError::Transient("Faraday::TimeoutError: Net::ReadTimeout".into()),
               "Client.network_failure raises TransientNetworkError");
    assert_eq!(t.requests()[0].query, vec![("venue_scheme", "alpaca_exchange".to_string())]);
    assert_eq!(api.public_url(), "https://data.deltabadger.com", "the docker alias is rewritten for browsers");
}

#[tokio::test(flavor = "current_thread")]
async fn the_live_request_carries_the_bearer_token_and_faradays_sorted_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/prefix/api/v2/assets")).and(query_param("type", "stock,etf")).and(query_param("include", "identifiers"))
        .and(header("Authorization", "Bearer tok")).and(header("Accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "external_id": "AAPL.US" }] })))
        .expect(1).mount(&server).await;
    let api = DataApi::live(Config { url: format!("{}/prefix/", server.uri()), token: "tok".into() });
    assert_eq!(api.stocks().await.unwrap()["data"][0]["external_id"], "AAPL.US");
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent[0].url.query(), Some("include=identifiers&type=stock%2Cetf"), "Faraday sorts and form-encodes the params");
}

#[tokio::test(flavor = "current_thread")]
async fn prices_are_looked_up_by_external_id_as_market_data_get_prices_does() {
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/prices", 200, json!({ "data": { "bitcoin": { "usd": 64000.5 }, "USD.FOREX": { "usd": 1.0 }, "delisted": { "usd": null },
                                                          "texty": { "usd": "2.5" } } }))
        .reply("GET /api/v1/prices", 502, json!({ "error": "Failed to fetch prices" }));
    let api = scripted(&t);
    let source: &dyn PriceSource = &api; // the other track's view of the client
    assert_eq!(source.prices(&[], "usd").await.unwrap(), BTreeMap::new(), "no id, no request");
    assert!(t.requests().is_empty());
    let ids: Vec<String> = ["bitcoin", "USD.FOREX", "bitcoin", "delisted", "missing", "texty"].map(String::from).to_vec();
    assert_eq!(source.prices(&ids, "usd").await.unwrap(),
               BTreeMap::from([("USD.FOREX".to_string(), 1.0), ("bitcoin".to_string(), 64000.5), ("texty".to_string(), 2.5)]),
               "an id data-api does not price is absent; a String price is String#to_f");
    assert_eq!(t.requests()[0].query, vec![("coin_ids", "bitcoin,USD.FOREX,delisted,missing,texty".to_string()), ("vs_currencies", "usd".to_string())],
               "uniq'd in order, comma-joined, as Clients::MarketData#get_prices sends them");
    assert_eq!(source.prices(&ids, "usd").await.unwrap_err(), ApiError::Failed { status: Some(502), message: r#"{"error":"Failed to fetch prices"}"#.into() },
               "with_rescue's failure; Rails' caller would then try CoinGecko (not ported)");
}
