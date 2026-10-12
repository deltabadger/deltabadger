//! Honeymaker::Clients::Binance (and ::BinanceUs) for the tracker's reads: signed GETs only. This client places no order,
//! cancels nothing and withdraws nothing; it has no method that sends anything but a GET.
//! Rails is the oracle: Exchanges::Binance#get_balances through honeymaker's #account_information and #with_rescue.
use super::http::{HttpRequest, Transport, TransportError};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const URL: &str = "https://api.binance.com";
pub const US_URL: &str = "https://api.binance.us";
pub const BINANCE: &str = "Exchanges::Binance";
pub const BINANCE_US: &str = "Exchanges::BinanceUs";
/// Exchanges::Binance::READ_RECV_WINDOW: signed reads use Binance's maximum.
pub const READ_RECV_WINDOW: &str = "60000";
/// Exchanges::Binance::ERRORS[:invalid_key]: the venue's words that condemn a key (Exchange#condemning_invalid_key_error?).
pub const INVALID_KEY: [&str; 3] = ["API-key format invalid.", "Invalid API-key, IP, or permissions for action.", "Invalid Api-Key ID."];

/// The venue an exchange type reads from, and ExchangeProxy's name for it.
pub fn host(exchange_type: &str) -> Option<(&'static str, &'static str)> {
    match exchange_type { BINANCE => Some((URL, "binance")), BINANCE_US => Some((US_URL, "binance_us")), _ => None }
}

/// Faraday::Utils.escape: everything but `[a-zA-Z0-9 .~_-]` percent-encoded (upper-case hex), then a space as "+".
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'~' | b'_' | b'-' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Faraday::Utils.build_query (FlatParamsEncoder): the pairs sorted by key, each escaped.
pub fn build_query(params: &[(&'static str, String)]) -> String {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    sorted.iter().map(|(k, v)| format!("{}={}", escape(k), escape(v))).collect::<Vec<_>>().join("&")
}

/// Client#hmac_sha256: lower-case hex.
pub fn sign(secret: &str, query: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(query.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Client#timestamp_ms: `(Time.now.utc.to_f * 1_000).to_i`, in Ruby's Float arithmetic.
pub fn timestamp_ms(now: DateTime<Utc>) -> i64 {
    let seconds = now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1e9;
    (seconds * 1000.0) as i64
}

#[derive(Clone)]
pub struct BinanceVenue<T: Transport> { transport: T, base: String, secret: String }

impl<T: Transport> BinanceVenue<T> {
    pub fn new(transport: T, base: &str, secret: &str) -> Self { Self { transport, base: base.into(), secret: secret.into() } }

    /// One signed GET as honeymaker sends it: the call's params (nils compacted) and `timestamp`, signed over
    /// `build_query` of exactly those, and the signature added; Faraday then sends every pair sorted by key.
    /// `Ok`: the body of a 2xx answer of at most `limit` bytes, unparsed. `Err`: #with_rescue's text for the failure: the
    /// answer's body when it has one, else Faraday's own message (a transport failure included: honeymaker returns it,
    /// it never raises).
    pub async fn signed_get(&self, path: &str, mut params: Vec<(&'static str, String)>, now: DateTime<Utc>, limit: usize) -> Result<String, String> {
        params.push(("timestamp", timestamp_ms(now).to_string()));
        let signature = sign(&self.secret, &build_query(&params));
        params.push(("signature", signature));
        params.sort_by(|a, b| a.0.cmp(b.0));
        let request = HttpRequest { method: "GET", base: self.base.clone(), path: path.into(), query: params, body: None, not_after: None };
        match self.transport.send_limited(&request, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, request.url())),
        }
    }
}

/// Exchanges::Binance#parse_error_message, as #get_balances applies it to a failure: the JSON body's `msg` when present,
/// else the failure's own text.
pub fn error_message(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text).ok().as_ref().and_then(|v| v.as_object()).and_then(|o| o.get("msg")) {
        Some(serde_json::Value::String(s)) if !crate::ruby::blank(s) => s.clone(),
        Some(v) if !v.is_null() && !v.is_string() && v != &serde_json::Value::Bool(false) => v.to_string(),
        _ => text.to_string(),
    }
}

/// Exchange#condemning_invalid_key_error? for Binance and Binance.US: the failure names one of the venue's own words.
pub fn condemns(text: &str) -> bool { INVALID_KEY.iter().any(|m| text.contains(m)) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_as_honeymaker_does() {
        // Binance's own documented example (SIGNED endpoint security, HMAC): this secret over this query.
        let secret = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
        let query = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        assert_eq!(sign(secret, query), "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71");
        assert_eq!(build_query(&[("timestamp", "1".into()), ("omitZeroBalances", "true".into()), ("recvWindow", "60000".into())]),
                   "omitZeroBalances=true&recvWindow=60000&timestamp=1");
        assert_eq!(build_query(&[("a", "B C/~é".into())]), "a=B+C%2F~%C3%A9");
    }

    #[test]
    fn the_timestamp_is_rubys_float_milliseconds() {
        let at: DateTime<Utc> = "2026-09-20T02:30:00.750000Z".parse().unwrap();
        assert_eq!(timestamp_ms(at), 1_789_871_400_750);
    }

    #[test]
    fn the_proxy_is_resolved_as_exchange_proxy_does() {
        use crate::venue::http::proxy_for;
        let env = |k: &str| (k == "PROXY_BINANCE").then(|| "http://proxy:8080".to_string());
        assert_eq!(proxy_for("binance", None, &env).as_deref(), Some("http://proxy:8080"));
        assert_eq!(proxy_for("binance", Some("http://app:1".into()), &env).as_deref(), Some("http://app:1"), "AppConfig wins");
        assert_eq!(proxy_for("binance", Some("  ".into()), &env).as_deref(), Some("http://proxy:8080"), "a blank AppConfig is none");
        assert_eq!(proxy_for("binance_us", None, &env), None);
        let us = |k: &str| (k == "PROXY_BINANCE_US").then(|| "http://us:1".to_string());
        assert_eq!(proxy_for("binance_us", None, &us).as_deref(), Some("http://us:1"));
        assert_eq!(proxy_for("binance", None, &|_| Some(String::new())), None);
    }

    #[test]
    fn a_failure_reads_the_venues_msg() {
        assert_eq!(error_message(r#"{"code":-2015,"msg":"Invalid API-key, IP, or permissions for action."}"#), "Invalid API-key, IP, or permissions for action.");
        assert_eq!(error_message("<html>bad gateway</html>"), "<html>bad gateway</html>");
        assert_eq!(error_message(r#"{"code":-1}"#), r#"{"code":-1}"#);
        assert!(condemns("Invalid API-key, IP, or permissions for action.") && !condemns("Timestamp for this request is outside of the recvWindow."));
    }
}
