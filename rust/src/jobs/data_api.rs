//! Clients::MarketData (app/models/clients/market_data.rb) over a Transport, with Client#with_rescue's outcomes
//! (app/models/client.rb:175-182, :244-264): the market-data service ("data-api") hosted instances read their catalogue from.
use crate::app_config;
use crate::crypto::Cipher;
use crate::venue::http::{self, HttpRequest, HttpResponse, Transport, TransportError};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Clients::MarketData::BULK_READ_TIMEOUT (:28-38): the two bulk stock pulls (~12 MB each) get 60 s per read, not 30.
pub const BULK_READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Faraday has no total cap; reqwest needs one. ponytail: 10 min, far above a healthy 12 MB pull; widen it if a slow link
/// ever needs more.
const BULK_TOTAL: Duration = Duration::from_secs(600);
/// The 29-scenario reference grid's largest serialized body is 266,652 bytes (stock assets).
/// Real bulk stock pulls are approximately 12 MB; 32 MiB leaves over 2.5x headroom for those too.
pub const MAX_REFERENCE_BODY: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Config { pub url: String, pub token: String }

/// MarketDataSettings.deltabadger? and AppConfig.market_data_url / _token (app/models/market_data_settings.rb:7-11, :38-48;
/// app/models/app_config.rb:269-289):
/// - the provider is deltabadger when MARKET_DATA_URL is present in the environment, or when app_configs names it;
/// - the URL and token come from app_configs when a row exists, even a blank one, else from the environment.
///
/// None: another provider, whose sync this build does not port.
pub fn config(env: &dyn Fn(&str) -> Option<String>, c: &Connection, cipher: &Cipher) -> Result<Option<Config>, String> {
    crate::engine::provider::bind(c,cipher,env).map_err(|_| "index configuration reader unavailable".to_string())?;
    let env_url = env("MARKET_DATA_URL").filter(|v| !v.trim().is_empty());
    let deltabadger = env_url.is_some() || app_config::get(c, cipher, "market_data_provider")?.as_deref() == Some("deltabadger");
    if !deltabadger { return Ok(None); }
    let pick = |key: &str, var: &str| -> Result<String, String> {
        if app_config::exists(c, key)? { Ok(app_config::get(c, cipher, key)?.unwrap_or_default()) } else { Ok(env(var).unwrap_or_default()) }
    };
    Ok(Some(Config { url: pick("market_data_url", "MARKET_DATA_URL")?, token: pick("market_data_token", "MARKET_DATA_TOKEN")? }))
}

#[derive(Debug, PartialEq)]
pub enum ApiError {
    /// Client.network_failure raised Client::TransientNetworkError: the request got no answer.
    Transient(String),
    /// A Result::Failure: an HTTP error (`status`), an unreadable body, or a permanent transport failure (no status).
    Failed { status: Option<u16>, message: String },
}

impl ApiError {
    pub fn message(&self) -> String {
        match self { Self::Transient(m) | Self::Failed { message: m, .. } => m.clone() }
    }
    /// MarketData.rate_limited_failure? (market_data.rb:423-426): a 429, which the stock and Alpaca listing syncs raise as
    /// Client::RateLimitedError.
    pub fn rate_limited(&self) -> bool { matches!(self, Self::Failed { status: Some(429), .. }) }
}

/// The live transport: Clients::MarketData's connection, with a bearer token and JSON (:118-132).
pub struct ApiTransport { client: reqwest::Client, token: String, base: Result<reqwest::Url, String> }

impl ApiTransport {
    fn new(client: reqwest::Client, token: &str, base: Result<reqwest::Url, String>) -> Self { Self { client, token: token.into(), base } }
}

impl Transport for ApiTransport {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.send_limited(r, MAX_REFERENCE_BODY).await
    }

    async fn send_limited(&self, r: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> {
        let mut url = self.base.as_ref().map_err(|e| TransportError::Permanent(e.clone()))?.clone();
        url.set_path(&format!("{}{}", url.path().trim_end_matches('/'), r.path));
        if !r.query.is_empty() { url.query_pairs_mut().extend_pairs(&r.query); }
        let resp = self.client.get(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .send().await.map_err(http::classify)?;
        http::read_limited(resp, limit.min(MAX_REFERENCE_BODY)).await
    }
}

pub struct DataApi<T: Transport> { pub config: Config, normal: T, bulk: T }

impl DataApi<ApiTransport> {
    pub fn live(config: Config) -> Self {
        // Parse external configuration once, retaining the error as a value so each job records its own failure.
        let base = reqwest::Url::parse(&config.url).map_err(|e| format!("invalid market-data URL: {e}"))
            .and_then(|url| if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
                Ok(url)
            } else { Err("market-data URL must have an HTTP(S) host".into()) });
        let normal = ApiTransport::new(http::client(), &config.token, base.clone());
        let bulk = ApiTransport::new(http::client_with(http::CONNECT_TIMEOUT, BULK_READ_TIMEOUT, BULK_TOTAL), &config.token, base);
        Self { config, normal, bulk }
    }
}

impl<T: Transport> DataApi<T> {
    pub fn new(config: Config, normal: T, bulk: T) -> Self { Self { config, normal, bulk } }

    /// MarketDataSettings.deltabadger_public_url (market_data_settings.rb:60-70): logo paths are served to browsers, so the
    /// docker alias `data-api` becomes the public host.
    pub fn configuration(&self) -> &Config { &self.config }

    pub fn public_url(&self) -> String {
        match reqwest::Url::parse(&self.config.url) {
            Ok(u) if u.host_str() == Some("data-api") => "https://data.deltabadger.com".into(),
            _ => self.config.url.clone(),
        }
    }

    /// One GET as Clients::MarketData sends it, read as with_rescue reads it. The body is parsed on the blocking pool:
    /// a 12 MB payload must not hold the runtime thread.
    pub async fn get(&self, path: &str, query: &[(&'static str, &str)], bulk: bool) -> Result<Value, ApiError> {
        let req = HttpRequest {
            method: "GET", base: self.config.url.trim_end_matches('/').to_string(), path: path.into(),
            query: query.iter().map(|(k, v)| (*k, v.to_string())).collect(), body: None, not_after: None,
        };
        let transport = if bulk { &self.bulk } else { &self.normal };
        let resp = match transport.send_limited(&req, MAX_REFERENCE_BODY).await {
            Ok(r) => r,
            Err(TransportError::Permanent(m)) => return Err(ApiError::Failed { status: None, message: m }),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => return Err(ApiError::Transient(m)),
        };
        if resp.status >= 400 { return Err(ApiError::Failed { status: Some(resp.status), message: failure_message(&req, &resp) }); }
        if resp.body.trim().is_empty() { return Ok(Value::Null); } // Faraday's JSON middleware reads an empty body as nil
        let status = resp.status;
        tokio::task::spawn_blocking(move || serde_json::from_str::<Value>(&resp.body))
            .await
            .map_err(|e| ApiError::Failed { status: None, message: format!("the blocking pool lost the parse: {e}") })?
            .map_err(|_| ApiError::Failed { status: Some(status), message: format!("Unreadable response (HTTP {status})") })
    }

    pub async fn assets(&self) -> Result<Value, ApiError> { self.get("/api/v1/assets", &[], false).await }
    pub async fn indices(&self) -> Result<Value, ApiError> { self.get("/api/v2/indices", &[], false).await }
    pub async fn tickers(&self, exchange: &str) -> Result<Value, ApiError> { self.get(&format!("/api/v1/tickers/{exchange}"), &[], false).await }
    /// Faraday sorts the params, so the query is given in that order.
    pub async fn stocks(&self) -> Result<Value, ApiError> { self.get("/api/v2/assets", &[("include", "identifiers"), ("type", "stock,etf")], true).await }
    pub async fn alpaca_listings(&self) -> Result<Value, ApiError> { self.get("/api/v2/listings", &[("venue_scheme", "alpaca_exchange")], true).await }
    pub async fn alpaca_crypto_listings(&self) -> Result<Value, ApiError> { self.get("/api/v2/listings", &[("venue", "alpaca_crypto")], true).await }

    /// MarketData.get_prices on the deltabadger provider (market_data.rb:170-182) through Clients::MarketData#get_prices
    /// (clients/market_data.rb:81-88): `GET api/v1/prices?coin_ids=<ids, comma-joined>&vs_currencies=<currency>`, the
    /// normal 30 s read. The ids are uniq'd in order first, and no id is no request (`Result::Success.new({})`). A price is
    /// `dig('data', id, currency)` read with `to_f`; an id data-api does not price (absent, or nil) is absent from the map.
    /// A failed request is with_rescue's outcome: Rails then falls back to CoinGecko, which is not ported.
    pub async fn prices(&self, external_ids: &[String], currency: &str) -> Result<BTreeMap<String, f64>, ApiError> {
        let mut ids: Vec<&str> = vec![];
        for id in external_ids { if !ids.contains(&id.as_str()) { ids.push(id); } }
        if ids.is_empty() { return Ok(BTreeMap::new()); }
        let body = self.get("/api/v1/prices", &[("coin_ids", &ids.join(",")), ("vs_currencies", currency)], false).await?;
        Ok(ids.into_iter().filter_map(|id| to_f(&body["data"][id][currency]).map(|p| (id.to_string(), p))).collect())
    }
}

/// `price.to_f if price`: nil and false are no price; a number is itself; a String is String#to_f. ponytail: the longest
/// leading prefix that parses as a float, else 0.0 (Ruby's rule up to exotic forms like "1_000"); data-api sends numbers.
fn to_f(v: &Value) -> Option<f64> {
    match v {
        Value::Null | Value::Bool(false) => None,
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = s.trim_start();
            Some((1..=t.len()).rev().filter(|&n| t.is_char_boundary(n)).find_map(|n| t[..n].parse::<f64>().ok()).unwrap_or(0.0))
        }
        _ => None,
    }
}

/// The price lookup the ledger and balance syncs (the other track) and any 2f job use. `DataApi` implements it; a
/// caller takes a `&dyn PriceSource`, so it never depends on the client's transport type.
pub trait PriceSource {
    /// Prices in `currency` by external id; ids data-api does not price are absent. Err: the request failed.
    fn prices<'a>(&'a self, external_ids: &'a [String], currency: &'a str) -> PriceFuture<'a>;
}

/// Not Send: polled on the runtime thread, as every job is.
pub type PriceFuture<'a> = Pin<Box<dyn Future<Output = Result<BTreeMap<String, f64>, ApiError>> + 'a>>;

impl<T: Transport> PriceSource for DataApi<T> {
    fn prices<'a>(&'a self, external_ids: &'a [String], currency: &'a str) -> PriceFuture<'a> {
        Box::pin(DataApi::prices(self, external_ids, currency))
    }
}

/// with_rescue's message for an HTTP error: "HTTP <status>" for an HTML body, else the raw body, else Faraday's own.
fn failure_message(r: &HttpRequest, resp: &HttpResponse) -> String {
    let body = resp.body.as_str();
    if body.trim().is_empty() {
        let raw = format!("{}{}", r.base, r.path);
        let url = reqwest::Url::parse_with_params(&raw, &r.query).map(|u| u.to_string()).unwrap_or(raw);
        return format!("the server responded with status {} for {} {url}", resp.status, r.method);
    }
    let lower = body.to_ascii_lowercase();
    if lower.match_indices('<').any(|(i, _)| lower[i + 1..].trim_start().starts_with("html")) { return format!("HTTP {}", resp.status); }
    body.to_string()
}


impl<T: Transport> PriceSource for Option<DataApi<T>> {
    fn prices<'a>(&'a self, ids: &'a [String], currency: &'a str) -> PriceFuture<'a> {
        match self {
            Some(api) => PriceSource::prices(api, ids, currency),
            None => Box::pin(async { Err(ApiError::Failed { status: None, message: "No market data provider available for prices".into() }) }),
        }
    }
}
