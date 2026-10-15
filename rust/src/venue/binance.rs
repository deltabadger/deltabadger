//! Honeymaker::Clients::Binance (and ::BinanceUs) for the tracker's reads (signed GETs), and Exchanges::Binance for the
//! engine's one-asset spot buys (spec E2): prices, balance, placement, polls and the fill rebuild. Rails is the oracle; each
//! method names the Ruby it ports. Binance.US trades nowhere here: only the tracker reads it.
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderState, PriceSide};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, BINANCE as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const URL: &str = "https://api.binance.com";
pub const US_URL: &str = "https://api.binance.us";
pub const BINANCE: &str = "Exchanges::Binance";
pub const BINANCE_US: &str = "Exchanges::BinanceUs";
/// Exchanges::Binance::READ_RECV_WINDOW: signed reads use Binance's maximum.
pub const READ_RECV_WINDOW: &str = "60000";
/// Honeymaker::Clients::Binance#new_order's default recvWindow, which Exchanges::Binance keeps for a placement: a stale order
/// is rejected (-1021), never executed late.
pub const PLACE_RECV_WINDOW: &str = "5000";
pub const UNREADABLE: &str = "Binance: unreadable response";
/// Named on a read answered with HTTP 429/418 (spec E §2.10, a listed divergence: Rails fails it as a generic error), so
/// VenueRules::is_throttle sees it: BINANCE's only throttle text.
/// Why a lookup by client order id never ends in "absent" (spec E §2.6): `GET /api/v3/order?origClientOrderId` answering
/// -2013 for an order that does not exist is not confirmed by a recorded live (or testnet) answer. Until it is, the intent
/// stays and the operator settles it with `deltabadger resolve-placement`.
pub const ABSENCE_UNPROVEN: &str = "Binance: no order with this client order id was found, and the lookup is not confirmed live, so absence is not proven";
pub const HTTP_THROTTLE: &str = "Binance: request rate limit exceeded";
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

/// What Exchanges::Binance reads from the exchange's tickers table while it trades: per venue symbol, its base and quote codes
/// and price decimals (#parse_order_data / #parse_trade_data `tickers.find_by(ticker:)`), and each venue code's asset symbol
/// (#asset_from_symbol: available tickers only, the first ticker naming a code wins).
#[derive(Clone, Debug, Default)]
pub struct Pairs { pub by_symbol: std::collections::HashMap<String, Pair>, pub assets: std::collections::HashMap<String, String> }

#[derive(Clone, Debug, PartialEq)]
pub struct Pair { pub base: String, pub quote: String, pub price_decimals: u8 }

impl Pairs {
    /// The exchange's tickers in id order, as the factory loads them for each venue it builds.
    pub fn load(c: &rusqlite::Connection, exchange_id: i64) -> Result<Self, String> {
        let mut s = c.prepare("SELECT t.ticker, t.base, t.quote, t.price_decimals, t.available, ba.symbol, qa.symbol FROM tickers t \
                               LEFT JOIN assets ba ON ba.id = t.base_asset_id LEFT JOIN assets qa ON qa.id = t.quote_asset_id \
                               WHERE t.exchange_id = ?1 ORDER BY t.id").map_err(|e| e.to_string())?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?,
                                                      r.get::<_, Option<bool>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?)))
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).map_err(|e| e.to_string())?;
        let mut pairs = Self::default();
        for (symbol, base, quote, decimals, available, base_asset, quote_asset) in rows {
            let price_decimals = u8::try_from(decimals).map_err(|_| format!("ticker {symbol}: price_decimals {decimals}"))?;
            if available == Some(true) {
                if let Some(a) = base_asset { pairs.assets.entry(base.clone()).or_insert(a); }
                if let Some(a) = quote_asset { pairs.assets.entry(quote.clone()).or_insert(a); }
            }
            pairs.by_symbol.entry(symbol).or_insert(Pair { base, quote, price_decimals });
        }
        Ok(pairs)
    }
}

#[derive(Clone)]
pub struct BinanceVenue<T: Transport> { transport: T, base: String, secret: String, pairs: Pairs }

impl<T: Transport> BinanceVenue<T> {
    pub fn new(transport: T, base: &str, secret: &str) -> Self { Self { transport, base: base.into(), secret: secret.into(), pairs: Pairs::default() } }
    /// The exchange's tickers (`Pairs::load`), which the trading methods read.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    /// One signed request as honeymaker builds it, on GET, POST and DELETE alike: the call's params (nils compacted) and
    /// `timestamp`, signed over `build_query` of exactly those, and the signature added; Faraday then sends every pair in the
    /// query, sorted by key (a POST has no body).
    pub fn signed(&self, method: &'static str, path: &str, mut params: Vec<(&'static str, String)>, now: DateTime<Utc>) -> HttpRequest {
        params.push(("timestamp", timestamp_ms(now).to_string()));
        let signature = sign(&self.secret, &build_query(&params));
        params.push(("signature", signature));
        self.request(method, path, params)
    }

    fn request(&self, method: &'static str, path: &str, mut params: Vec<(&'static str, String)>) -> HttpRequest {
        params.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path: path.into(), query: params, body: None, not_after: None }
    }

    /// One signed GET (`signed`). `Ok`: the body of a 2xx answer of at most `limit` bytes, unparsed. `Err`: #with_rescue's
    /// text for the failure: the answer's body when it has one, else Faraday's own message (a transport failure included:
    /// honeymaker returns it, it never raises).
    pub async fn signed_get(&self, path: &str, params: Vec<(&'static str, String)>, now: DateTime<Utc>, limit: usize) -> Result<String, String> {
        self.get(self.signed("GET", path, params, now), limit).await
    }

    /// An unsigned GET (honeymaker's #exchange_information): the params sorted, no key, no signature.
    pub async fn public_get(&self, path: &str, params: Vec<(&'static str, String)>, limit: usize) -> Result<String, String> { self.get(self.request("GET", path, params), limit).await }

    async fn get(&self, request: HttpRequest, limit: usize) -> Result<String, String> {
        match self.transport.send_limited(&request, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, request.url())),
        }
    }

    /// A signed trading call, stamped with the transport's clock just before it is sent (`Transport::now`).
    fn signed_now(&self, method: &'static str, path: &str, params: Vec<(&'static str, String)>) -> HttpRequest {
        self.signed(method, path, params, self.transport.now())
    }

    /// One call as honeymaker's #with_rescue and Exchanges::Binance read it (Exchange#ambiguous_placement_error? for a
    /// placement). A transport failure that sent nothing is Transient; one that may have sent is Transient on a read and
    /// Ambiguous on a placement. A non-2xx answer is Faraday's raise_error, reported as #parse_error_message reads it (the
    /// body's `msg`, else the body, else Faraday's message); on a placement only a 4xx, or a -1021 rejection
    /// (Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS) whatever its status, is definitive. HTTP 429/418 on a read is a throttle
    /// (`HTTP_THROTTLE`). A 2xx that is not JSON, or holds a number out of range, is unreadable: never a parsed zero.
    pub(crate) async fn call(&self, r: &HttpRequest, placement: bool) -> Result<serde_json::Value, super::VenueError> {
        use super::VenueError;
        let redact = |m: String| self.transport.redact_diagnostic(&m);
        let resp = match self.transport.send(r).await {
            Ok(resp) => resp,
            Err(TransportError::Permanent(m)) => return Err(VenueError::Rejected(vec![redact(m)])),
            Err(TransportError::NotSent(m)) => return Err(VenueError::Transient(redact(m))),
            Err(TransportError::MaybeSent(m)) if placement => return Err(VenueError::Ambiguous(redact(m))),
            Err(TransportError::MaybeSent(m)) => return Err(VenueError::Transient(redact(m))),
        };
        if !(200..300).contains(&resp.status) {
            if !placement && matches!(resp.status, 429 | 418) {
                return Err(VenueError::Rejected(vec![format!("the server responded with status {} ({HTTP_THROTTLE})", resp.status)]));
            }
            let message = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) }
                          else { error_message(&resp.body) };
            let definitive = (400..500).contains(&resp.status) || crate::engine::placement::safe_rejection(std::slice::from_ref(&message));
            return Err(if placement && !definitive { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) });
        }
        http::decode_json(&resp.body).map_err(|_| unreadable())
    }
}

pub(crate) fn unreadable() -> super::VenueError { super::VenueError::Ambiguous(UNREADABLE.into()) }

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

/// A Binance decimal string or number: present and readable, or the whole answer is unreadable (never a zero). DIVERGES from
/// Ruby on purpose (ruby::json_to_d): Rails' `.to_d` reads "garbage" as 0.
fn required(v: &serde_json::Value) -> Result<BigDec, super::VenueError> {
    crate::ruby::json_to_d(v).ok().flatten().ok_or_else(unreadable)
}

impl<T: Transport> super::Venue for BinanceVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }

    /// Exchanges::Binance#get_ask_price through #get_bid_ask_price (bookTicker: all four of bidPrice, bidQty, askPrice and
    /// askQty are dug for), and #get_last_price (ticker/price). A failure is #parse_error_message's text; a zero price raises
    /// "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, super::VenueError> {
        let symbol = vec![("symbol", ticker.ticker.clone())];
        let (price, label) = match side {
            PriceSide::Ask => {
                let body = self.call(&self.request("GET", "/api/v3/ticker/bookTicker", symbol), false).await?;
                for field in ["bidPrice", "bidQty", "askQty"] { required(&body[field])?; }
                (required(&body["askPrice"])?, "ask")
            }
            PriceSide::Last => (required(&self.call(&self.request("GET", "/api/v3/ticker/price", symbol), false).await?["price"])?, "last"),
        };
        if price.is_zero() { return Err(super::VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.ticker, price.to_s_f())])); }
        Ok(price)
    }

    async fn add_order(&self, _o: &NewOrder) -> Result<String, super::VenueError> { Err(super::VenueError::Transient("Binance placement is not ported yet".into())) }
    async fn orders(&self, _txids: &[String]) -> Result<Vec<OrderState>, super::VenueError> { Err(super::VenueError::Transient("Binance polls are not ported yet".into())) }
    async fn order_by_client_id(&self, _id: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, super::VenueError> { Err(super::VenueError::Ambiguous(ABSENCE_UNPROVEN.into())) }
    /// Rails' trade fallback is inside #get_orders (the fill rebuild); a missing order is never looked up in trades.
    async fn fills_from_trades(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, super::VenueError> { Ok(vec![]) }

    /// Exchanges::Binance#get_balances for the quote asset: GET /api/v3/account (omitZeroBalances, the read recvWindow), each
    /// row's venue code mapped to its asset (#asset_from_symbol), `free`; the last row naming the asset wins, absent is 0.
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, super::VenueError> {
        let r = self.signed_now("GET", "/api/v3/account", vec![("omitZeroBalances", "true".into()), ("recvWindow", READ_RECV_WINDOW.into())]);
        let body = self.call(&r, false).await?;
        let rows = body["balances"].as_array().ok_or_else(unreadable)?;
        let mut free = BigDec::zero();
        for row in rows {
            let Some(code) = row["asset"].as_str() else { continue };
            if self.pairs.assets.get(code).map(String::as_str) != Some(asset_symbol) { continue; }
            required(&row["locked"])?;
            free = required(&row["free"])?;
        }
        Ok(free)
    }
}

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
