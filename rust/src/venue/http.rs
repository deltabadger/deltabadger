//! The HTTP layer beneath the in-app venue clients (Alpaca): reqwest for real requests, and a scripted transport that
//! replays recorded bodies (tests and the parity harness). A transport failure says whether the request can have
//! reached the venue; the placement protocol depends on that split.
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

/// Client::OPTIONS: open 5 s, read 30 s, write 10 s. reqwest has no write timeout, so the whole request is capped at
/// their sum: nothing this client sends can reach the venue later than TOTAL_TIMEOUT after the send began.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    pub method: &'static str,
    /// Scheme and host, e.g. "https://paper-api.alpaca.markets".
    pub base: String,
    pub path: String,
    pub query: Vec<(&'static str, String)>,
    pub body: Option<Value>,
    /// The latest moment this request may still be in flight (the intent's `at` + send window + request budget), carried
    /// from the order intent so a process suspended after the freshness check cannot start a fresh 45 s request on resume.
    /// `None` for reads. ScriptedTransport ignores it (its replies are instantaneous).
    pub not_after: Option<DateTime<Utc>>,
}

impl HttpRequest {
    /// The URL as Faraday sends and prints it: query values form-encoded, so "BTC/USD" is "BTC%2FUSD".
    pub fn url(&self) -> reqwest::Url {
        let u = format!("{}{}", self.base, self.path);
        let parsed = if self.query.is_empty() { reqwest::Url::parse(&u) } else { reqwest::Url::parse_with_params(&u, &self.query) };
        parsed.unwrap_or_else(|e| panic!("{u} is not a URL: {e}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse { pub status: u16, pub body: String }

#[derive(Clone, Debug, PartialEq)]
pub enum TransportError {
    /// Provably never transmitted: refused, DNS, connect timeout, TLS handshake (Client.pre_transmission?).
    NotSent(String),
    /// May have reached the venue: a timeout or a lost connection after the request was written.
    MaybeSent(String),
    /// A failure Client.network_failure returns as a Failure instead of raising (it is not retried): a TLS certificate or
    /// protocol failure, or a local permission refusal. Both happen while connecting, so nothing was sent.
    Permanent(String),
}

pub trait Transport {
    async fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    /// A bare number outside the venue caps: serde would turn it into 0.0 (1e-350), ±Inf or an error (1e400), or round it.
    OutOfRange(String),
    /// Not JSON at all.
    NotJson,
}

/// Every venue response body is decoded here, never with serde_json directly. serde_json reads a bare JSON number as an
/// f64, so `1e-350` would arrive as 0.0 before any cap could see it. The raw text is scanned first: every bare number
/// (string contents skipped, escapes honoured) must be within the venue caps (ruby::VENUE_MAX_EXPONENT for the effective
/// exponent, mantissa included; ruby::VENUE_MAX_DIGITS significant digits), or the whole answer is unreadable.
pub fn decode_json(body: &str) -> Result<Value, DecodeError> {
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' { i += if b[i] == b'\\' { 2 } else { 1 }; }
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                while i < b.len() && matches!(b[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') { i += 1; }
                number_in_caps(&body[start..i])?;
            }
            _ => i += 1,
        }
    }
    serde_json::from_str(body).map_err(|_| DecodeError::NotJson)
}

/// Over borrowed slices only: a hostile token of any length costs no allocation, and its diagnostic quotes a short prefix.
fn number_in_caps(token: &str) -> Result<(), DecodeError> {
    use crate::ruby::{VENUE_MAX_DIGITS, VENUE_MAX_EXPONENT};
    // The scanner only takes ASCII bytes into a token, so any byte index is a char boundary.
    let bad = || DecodeError::OutOfRange(format!("unreadable number {}{} ({} characters) in the venue's answer",
        &token[..token.len().min(32)], if token.len() > 32 { "…" } else { "" }, token.len()));
    let t = token.strip_prefix('-').unwrap_or(token);
    let (mantissa, exp) = t.split_once(['e', 'E']).unwrap_or((t, "0"));
    let exp = i128::from(exp.parse::<i64>().map_err(|_| bad())?);
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = || int.bytes().chain(frac.bytes());
    if int.is_empty() || !digits().all(|d| d.is_ascii_digit()) { return Err(bad()); }
    let Some(first) = digits().position(|d| d != b'0') else { return Ok(()) }; // a true zero
    let last = int.len() + frac.len() - 1 - digits().rev().position(|d| d != b'0').unwrap_or(0);
    let exponent = int.len() as i128 - 1 - first as i128 + exp; // the most significant digit's power of ten
    if (last - first + 1) as i64 > VENUE_MAX_DIGITS || exponent.abs() > i128::from(VENUE_MAX_EXPONENT) { return Err(bad()); }
    Ok(())
}

pub fn client() -> reqwest::Client { client_with(CONNECT_TIMEOUT, READ_TIMEOUT, TOTAL_TIMEOUT) }

pub fn client_with(connect: Duration, read: Duration, total: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .read_timeout(read)
        .timeout(total)
        // Drop idle pooled connections after 5 s: a server has likely closed an older one, and an order POST written to a
        // dead socket would fail as a spurious MaybeSent and cost a needless recovery wait.
        .pool_idle_timeout(Duration::from_secs(5))
        // Rails' effective User-Agent: Clients::Alpaca replaces Faraday's headers, so Net::HTTP sends "Ruby".
        .user_agent("Ruby")
        // Rails reaches Alpaca directly (there is no PROXY_ALPACA); never pick up HTTP(S)_PROXY from the environment.
        .no_proxy()
        // Faraday has no follow-redirects middleware here: a 3xx is an answer, never a second request to another host
        // (a followed 307/308 would re-send the order elsewhere).
        .redirect(reqwest::redirect::Policy::none())
        // One call, one request: a transport retry could re-send an order and blur "never sent" into "sent twice".
        .retry(reqwest::retry::never())
        .build()
        .expect("a static reqwest configuration builds")
}

#[derive(Clone)]
pub struct ReqwestTransport { client: reqwest::Client, key: String, secret: String, refused: Option<String> }

impl ReqwestTransport {
    /// `key`/`secret` as Clients::Alpaca sends them (`@api_key.to_s`: a missing key is an empty header).
    pub fn new(client: reqwest::Client, key: String, secret: String) -> Self { Self { client, key, secret, refused: None } }
    /// Answers every request with NotSent and sends nothing (a venue this build must not reach).
    pub fn refused(reason: &str) -> Self { Self { client: client(), key: String::new(), secret: String::new(), refused: Some(reason.into()) } }
}

impl Transport for ReqwestTransport {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if let Some(reason) = &self.refused { return Err(TransportError::NotSent(reason.clone())); }
        let method = reqwest::Method::from_bytes(r.method.as_bytes()).expect("a static method name");
        let mut b = self.client.request(method, r.url())
            .header("APCA-API-KEY-ID", &self.key)
            .header("APCA-API-SECRET-KEY", &self.secret)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        if let Some(body) = &r.body { b = b.body(body.to_string()); }
        if let Some(not_after) = r.not_after {
            // Read immediately before the send: what is left of the absolute bound becomes this request's whole timeout.
            // LISTED RESIDUAL: this bounds when a send may start and how long reqwest keeps it alive, not when the bytes leave.
            // A process frozen mid-write of the order request (SIGSTOP, a VM pause) and resumed later can deliver it after
            // the absence window (placement::recover_since) has already declared it never placed. Escape hatch: the operator
            // checks the Alpaca paper dashboard and settles the intent with `deltabadger resolve-placement`.
            // LISTED RESIDUAL: bytes delivered later than the 20-minute absence margin (VenueRules::absence_margin_secs): a
            // kernel configured with a longer retransmission window than Linux's defaults, or a path (proxy, middlebox) that
            // buffers longer than TCP does. Escape hatch: `deltabadger resolve-placement`; before resolving "not placed" the
            // operator checks Alpaca's order list for the intent's client_order_id.
            match (not_after - Utc::now()).to_std().ok().filter(|left| !left.is_zero()) {
                None => return Err(TransportError::NotSent(format!("past the send bound {}; not sent", not_after.to_rfc3339()))),
                Some(left) => b = b.timeout(left.min(TOTAL_TIMEOUT)),
            }
        }
        let resp = b.send().await.map_err(classify)?;
        let status = resp.status().as_u16();
        // The status line has arrived: the request was sent, so losing the body leaves the outcome open.
        let body = resp.text().await.map_err(|e| TransportError::MaybeSent(describe(&e)))?;
        Ok(HttpResponse { status, body })
    }
}

/// Client.network_failure's split, then Client.pre_transmission?:
/// - Permanent: only while connecting (`is_connect`, so provably before any request byte), a TLS failure other than the
///   peer closing the connection (rustls reports certificate and protocol errors as io::ErrorKind::InvalidData) or a
///   permission refusal (EACCES/EPERM → PermissionDenied). Rails returns these as a Failure: not retried, and for a
///   placement a failed row. Outside connect they stay MaybeSent: on Linux sendmsg returns EPERM when netfilter drops a
///   packet on an established connection, possibly after request bytes went out, and a failed row would read as "absent".
/// - NotSent: `is_connect` (DNS, refusal, a connect timeout): no request byte was written.
/// - MaybeSent: anything else, as Rails rules for unknown provenance.
fn classify(e: reqwest::Error) -> TransportError {
    let m = describe(&e);
    let lower = m.to_ascii_lowercase();
    let eof = lower.contains("unexpected eof") || lower.contains("end of file");
    let mut io_kind = None;
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        // reqwest 0.13 wraps the rustls InvalidData io::Error inside another io::Error whose own source() skips it.
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            let io = io.get_ref().and_then(|i| i.downcast_ref::<std::io::Error>()).unwrap_or(io);
            if matches!(io.kind(), std::io::ErrorKind::InvalidData | std::io::ErrorKind::PermissionDenied) { io_kind = Some(io.kind()); break; }
        }
        source = std::error::Error::source(s);
    }
    match transport_kind(io_kind, eof, e.is_connect()) {
        Kind::Permanent => TransportError::Permanent(m),
        Kind::NotSent => TransportError::NotSent(m),
        Kind::MaybeSent => TransportError::MaybeSent(m),
    }
}

#[doc(hidden)]
#[derive(Debug, PartialEq)]
pub enum Kind { Permanent, NotSent, MaybeSent }

/// The pure decision behind `classify` (reqwest::Error cannot be built in a test). `io_kind` is the innermost io error
/// kind in the source chain, `eof` whether the message says the peer closed the stream.
#[doc(hidden)]
pub fn transport_kind(io_kind: Option<std::io::ErrorKind>, eof: bool, is_connect: bool) -> Kind {
    use std::io::ErrorKind::{InvalidData, PermissionDenied};
    let permanent = match io_kind { Some(InvalidData) => !eof, Some(PermissionDenied) => true, _ => false };
    if is_connect { if permanent { Kind::Permanent } else { Kind::NotSent } } else { Kind::MaybeSent }
}

fn describe(e: &reqwest::Error) -> String {
    let mut m = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source { m.push_str(": "); m.push_str(&s.to_string()); source = std::error::Error::source(s); }
    m
}

/// Recorded answers per "METHOD path", the same script the Rails harness serves beneath Clients::Alpaca. A reply is
/// {"status", "body"} (a JSON string body is sent as raw text) or {"network": "pre_send"|"post_send"|"permanent", "message"}.
/// The last reply for a key repeats; an unscripted call panics, as the Rails harness raises.
#[derive(Clone, Default)]
pub struct ScriptedTransport { s: Rc<RefCell<Scripted>> }

#[derive(Default)]
struct Scripted { replies: HashMap<String, VecDeque<Value>>, requests: Vec<HttpRequest> }

impl ScriptedTransport {
    pub fn from_script(script: &Value) -> Self {
        let t = Self::default();
        for (key, replies) in script.as_object().into_iter().flatten() {
            t.s.borrow_mut().replies.insert(key.clone(), replies.as_array().cloned().unwrap_or_default().into());
        }
        t
    }
    pub fn reply(&self, key: &str, status: u16, body: Value) -> &Self { self.push(key, serde_json::json!({ "status": status, "body": body })) }
    pub fn network(&self, key: &str, kind: &str, message: &str) -> &Self { self.push(key, serde_json::json!({ "network": kind, "message": message })) }
    fn push(&self, key: &str, reply: Value) -> &Self { self.s.borrow_mut().replies.entry(key.into()).or_default().push_back(reply); self }
    pub fn requests(&self) -> Vec<HttpRequest> { self.s.borrow().requests.clone() }
    /// The order bodies POSTed, without Rust's client_order_id: what parity compares with Rails' wire.
    pub fn posted_orders(&self) -> Vec<Value> {
        self.s.borrow().requests.iter().filter(|r| r.method == "POST" && r.path == "/v2/orders").filter_map(|r| r.body.clone())
            .map(|mut b| { if let Value::Object(m) = &mut b { m.remove("client_order_id"); } b }).collect()
    }
}

impl Transport for ScriptedTransport {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let key = format!("{} {}", r.method, r.path);
        let reply = {
            let mut s = self.s.borrow_mut();
            s.requests.push(r.clone());
            let q = s.replies.get_mut(&key).filter(|q| !q.is_empty()).unwrap_or_else(|| panic!("unscripted Alpaca call {key}"));
            if q.len() > 1 { q.pop_front().expect("non-empty") } else { q[0].clone() }
        };
        let message = reply["message"].as_str().unwrap_or_default().to_string();
        match reply["network"].as_str() {
            Some("pre_send") => Err(TransportError::NotSent(message)),
            Some("permanent") => Err(TransportError::Permanent(message)),
            Some("post_send") => Err(TransportError::MaybeSent(message)),
            Some(other) => panic!("unknown network kind {other:?} scripted for {key}"),
            None => Ok(HttpResponse {
                status: reply["status"].as_u64().unwrap_or_else(|| panic!("scripted reply for {key} has no status: {reply}")) as u16,
                body: match &reply["body"] { Value::String(s) => s.clone(), other => other.to_string() },
            }),
        }
    }
}
