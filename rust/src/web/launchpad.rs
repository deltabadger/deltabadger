//! The platform claim: Clients::Launchpad#claim (POST <LAUNCHPAD_URL>/api/claim, 5 s to connect, 10 s per read) and
//! Platform::RedeemClaim's reading of the answer. Stricter than Rails: HTTP errors use fixed messages, never
//! upstream bodies or request URLs, which may contain credentials. Claim diagnostics are never logged.
use serde_json::{Map, Value};
use std::time::Duration;

pub const INVALID_CLAIM: &str = "That claim code is invalid or has expired.";
pub const CLAIM_FAILED: &str = "Deltabadger could not complete the claim. Please try again.";
pub const INVALID_MARKET_DATA: &str = "Deltabadger returned incomplete market data configuration. Please try again.";
pub const RETRYABLE: &str = "Unable to reach Deltabadger. Please try again.";
pub const AMBIGUOUS: &str = "Deltabadger may already have used this one-time code. Generate a fresh code from the dashboard and try again.";
/// `ENV.fetch('LAUNCHPAD_URL', 'https://deltabadger.com')`.
pub const DEFAULT_URL: &str = "https://deltabadger.com";
/// The largest answer read. A claim answer is a few hundred bytes.
const MAX_ANSWER: usize = 1024 * 1024;

/// A redeemed claim: what RedeemClaim writes, and the subscriber's identity for the form.
#[derive(Debug, PartialEq)]
pub struct Claim {
    pub market_url: String,
    pub market_token: String,
    /// `proxy_<key downcased>` => value, in the answer's order.
    pub proxies: Vec<(String, String)>,
    /// platform_identity_attributes: name and email when the identity is an object (an object without them is `None`s).
    pub identity: (Option<String>, Option<String>),
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    /// Result::Failure: the sentence the page shows.
    Message(String),
    /// An answer Rails raises on inside its transaction (a `proxies` that is not an object): a 500.
    Malformed,
}

fn message(text: impl Into<String>) -> Failure { Failure::Message(text.into()) }

/// ActiveSupport's `present?` for a JSON value.
fn present(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !crate::ruby::blank(s),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// ActiveModel's string cast, as an AppConfig value or a user attribute receives a JSON value.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(true) => Some("t".into()),
        Value::Bool(false) => Some("f".into()),
        other => Some(other.to_string()),
    }
}

/// How Ruby names the receiver of a NoMethodError (`undefined method 'x' for …`).
pub(crate) fn receiver(value: &Value) -> &'static str {
    match value {
        Value::Null => "nil",
        Value::Bool(true) => "true",
        Value::Bool(false) => "false",
        Value::String(_) => "an instance of String",
        Value::Array(_) => "an instance of Array",
        Value::Number(n) if n.is_f64() => "an instance of Float",
        Value::Number(_) => "an instance of Integer",
        Value::Object(_) => "an instance of Hash",
    }
}

/// The claim URL: Faraday joins `api/claim` onto the base URL's path.
pub fn url(base: &str) -> String {
    format!("{}/api/claim", base.trim_end_matches('/'))
}

/// One answer as Clients::Launchpad and RedeemClaim read it. `content_type` is the answer's Content-Type.
pub fn read(status: u16, content_type: Option<&str>, body: &str) -> Result<Claim, Failure> {
    // Error bodies can echo a claim token or credentials, even when they are JSON or plain text.
    if (400..600).contains(&status) {
        return Err(message(if status == 401 { INVALID_CLAIM } else { CLAIM_FAILED }));
    }
    // Faraday's JSON response middleware parses a `…json` content type only, and a blank body as nil.
    let json = content_type.and_then(|t| t.split(';').next()).is_some_and(|t| t.trim().to_ascii_lowercase().ends_with("json"));
    let value = if !json {
        Value::String(body.to_string())
    } else if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).map_err(|_| message(format!("Unreadable response (HTTP {status})")))?
    };
    let Value::Object(payload) = value else {
        return Err(message(format!("undefined method 'deep_symbolize_keys' for {}", receiver(&value))));
    };
    redeem(&payload)
}

/// Platform::RedeemClaim#call on a payload that is an object.
fn redeem(payload: &Map<String, Value>) -> Result<Claim, Failure> {
    let market = payload.get("market_data").and_then(Value::as_object);
    let field = |key: &str| market.and_then(|m| m.get(key)).filter(|v| present(v)).and_then(text);
    let (Some(market_url), Some(market_token)) = (field("url"), field("token")) else { return Err(message(INVALID_MARKET_DATA)) };
    let proxies = match payload.get("proxies") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Vec::new(),
        Some(Value::Object(proxies)) => proxies.iter().map(|(key, value)| (format!("proxy_{}", key.to_lowercase()), text(value).unwrap_or_default())).collect(),
        Some(_) => return Err(Failure::Malformed),
    };
    let identity = match payload.get("identity") {
        Some(Value::Object(identity)) => (identity.get("name").and_then(text), identity.get("email").and_then(text)),
        _ => (None, None),
    };
    Ok(Claim { market_url, market_token, proxies, identity })
}

/// POST {"code": code} to the claim URL and read the answer. A failure to connect (nothing was sent) asks to try
/// again; any other transport failure may have spent the one-time code.
pub async fn claim(base: &str, code: Value) -> Result<Claim, Failure> {
    let url = url(base);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(10))
        // Faraday has no total deadline; this one only bounds a server that trickles.
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| message(RETRYABLE))?;
    let transport = |e: reqwest::Error| message(if e.is_connect() { RETRYABLE } else { AMBIGUOUS });
    let mut response = client.post(&url).header(reqwest::header::CONTENT_TYPE, "application/json").body(serde_json::json!({ "code": code }).to_string()).send().await.map_err(transport)?;
    let status = response.status().as_u16();
    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_ANSWER {
            return Err(message(format!("Unreadable response (HTTP {status})")));
        }
    }
    read(status, content_type.as_deref(), &String::from_utf8_lossy(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    const JSON: Option<&str> = Some("application/json; charset=utf-8");

    #[test]
    fn answers_read_as_rails_reads_them() {
        let ok = r#"{"identity":{"name":"Owner","email":"o@example.com"},"market_data":{"url":"https://d.example","token":"t"},"proxies":{"Binance":"http://p","n":1}}"#;
        assert_eq!(read(200, JSON, ok), Ok(Claim { market_url: "https://d.example".into(), market_token: "t".into(),
            proxies: vec![("proxy_binance".into(), "http://p".into()), ("proxy_n".into(), "1".into())], identity: (Some("Owner".into()), Some("o@example.com".into())) }));
        let failed = |status, content_type, body: &str| match read(status, content_type, body) { Err(Failure::Message(m)) => m, other => panic!("{other:?}") };
        assert_eq!(failed(401, JSON, "{}"), INVALID_CLAIM);
        // Deliberately stricter than Rails: errors never expose an upstream body or request URL.
        for (status, content_type, body) in [(500, JSON, r#"{"error":"credential-sentinel"}"#),
            (502, Some("text/html"), "<HTML>credential-sentinel</HTML>"), (503, None, " ")] {
            assert_eq!(failed(status, content_type, body), "Deltabadger could not complete the claim. Please try again.");
        }
        assert_eq!(failed(200, JSON, "hello"), "Unreadable response (HTTP 200)");
        assert_eq!(failed(200, JSON, "[]"), "undefined method 'deep_symbolize_keys' for an instance of Array");
        assert_eq!(failed(200, JSON, ""), "undefined method 'deep_symbolize_keys' for nil");
        assert_eq!(failed(200, Some("text/plain"), "{}"), "undefined method 'deep_symbolize_keys' for an instance of String");
        assert_eq!(failed(200, JSON, r#"{"market_data":{"url":"u","token":" "}}"#), INVALID_MARKET_DATA);
        assert_eq!(read(200, JSON, r#"{"market_data":{"url":"u","token":"t"},"proxies":"x"}"#), Err(Failure::Malformed));
        assert_eq!(url("http://h:1/"), "http://h:1/api/claim");
    }
}
