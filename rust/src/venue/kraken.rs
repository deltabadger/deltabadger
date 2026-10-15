//! Exchanges::Kraken + Honeymaker::Clients::Kraken for one-asset buys, over a Transport (spec E1). Rails is the oracle; each
//! method names the Ruby it ports. The engine's FakeVenue reads the same raw bodies with the parsers below.
//!
//! Where the code lives (spec E §1, settled by E1's dependency test): a module of this crate on the shared transport. Linking
//! honeymaker's core crate turns on serde_json/arbitrary_precision crate-wide, which changes how Ruby-parity JSON is held
//! (tests/sync_vectors.rs: three tests fail with it on), so its reviewed logic is ported here instead.
//!
//! Signing (`private_headers`) happens inside the transport at send time (ReqwestTransport with `Auth::Kraken`), so the nonce
//! is minted when the request leaves. The adapter builds the request without it: what ScriptedTransport records is what the
//! Rails harness records (AddOrder params minus nonce).
use super::http::{self, DecodeError, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, Venue, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, KRAKEN};
use crate::ruby::BigDec;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256, Sha512};

/// Honeymaker::Clients::Kraken::URL.
pub const URL: &str = "https://api.kraken.com";
pub const UNREADABLE: &str = "Kraken: unreadable response";
/// KRAKEN's throttle text (engine::venue_rules), named on a read answered with HTTP 429/418 so VenueRules::is_throttle sees it.
const HTTP_THROTTLE: &str = "EAPI:Rate limit exceeded";
/// Honeymaker::Clients::Kraken#closed_orders_from_trades's `max_pages`.
const TRADES_MAX_PAGES: u32 = 20;
/// Why a lookup by client order id never ends in "absent" (spec E §2.6): Kraken's `cl_ord_id` filter, `count` and echo on
/// Open/ClosedOrders are not confirmed by a recorded live answer. Until they are, the intent stays and the operator settles it
/// with `deltabadger resolve-placement`.
pub const ABSENCE_UNPROVEN: &str = "Kraken: no order with this client order id was found, and the lookup is not confirmed live, so absence is not proven";

const ASSET_MAP: [(&str, &str); 10] = [("ZUSD", "USD"), ("ZEUR", "EUR"), ("ZGBP", "GBP"), ("ZJPY", "JPY"), ("ZCHF", "CHF"),
    ("ZCAD", "CAD"), ("ZAUD", "AUD"), ("XXBT", "XBT"), ("XETH", "ETH"), ("XXDG", "XDG")];

// ---- signing: Honeymaker::Clients::Kraken#private_headers / #nonce, byte for byte (tests/fixtures/kraken_signing.json)

/// URI.encode_www_form: keep A-Z a-z 0-9 * - . _, space → +, everything else %XX (uppercase).
pub fn www_form(pairs: &[(String, String)]) -> String {
    let enc = |s: &str| s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => (b as char).to_string(),
        b' ' => "+".into(),
        _ => format!("%{b:02X}"),
    }).collect::<String>();
    pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

/// Base64.decode64 = String#unpack1("m"): ruby/pack.c's lenient branch (bytes outside the alphabet are skipped; '=' in the
/// third or fourth slot ends the input). Ported from honeymaker's core (crates/honeymaker/src/encode.rs).
fn ruby_decode64(input: &[u8]) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let table = |c: u8| T.iter().position(|&x| x == c).map_or(-1, |p| p as i32);
    let n = input.len();
    let at = |i: usize| if i < n { input[i] } else { 0 };
    let mut out = Vec::with_capacity(n * 3 / 4 + 3);
    let (mut s, mut a, mut b, mut c) = (0usize, -1i32, -1i32, 0i32);
    // Each slot skips bytes outside the alphabet; the third and fourth also stop at '='.
    let next = |s: &mut usize, stop_at_pad: bool| -> i32 {
        loop {
            let v = table(at(*s));
            if v != -1 || *s >= n || (stop_at_pad && at(*s) == b'=') { return v; }
            *s += 1;
        }
    };
    while s < n {
        b = -1;
        c = -1;
        a = next(&mut s, false);
        if s >= n { break; }
        s += 1;
        b = next(&mut s, false);
        if s >= n { break; }
        s += 1;
        c = next(&mut s, true);
        if at(s) == b'=' || s >= n { break; }
        s += 1;
        let d = next(&mut s, true);
        if at(s) == b'=' || s >= n { break; }
        s += 1;
        out.extend_from_slice(&[((a << 2) | (b >> 4)) as u8, ((b << 4) | (c >> 2)) as u8, ((c << 6) | d) as u8]);
        a = -1;
    }
    if a != -1 && b != -1 {
        out.push(((a << 2) | (b >> 4)) as u8);
        if c != -1 { out.push(((b << 4) | (c >> 2)) as u8); }
    }
    out
}

pub fn public_headers() -> Vec<(String, String)> {
    vec![("Accept".into(), "application/json".into()), ("Content-Type".into(), "application/json".into()), ("User-Agent".into(), "Honeymaker Ruby".into())]
}

/// The headers for a private POST whose form body is `body` (nonce first): API-Sign = base64(HMAC-SHA512(decode64(secret),
/// path + SHA256(nonce + body))). With no key or no secret the gem sends its public headers (Kraken refuses the call).
pub fn private_headers(path: &str, body: &str, key: &str, secret: &str) -> Vec<(String, String)> {
    if key.is_empty() || secret.is_empty() { return public_headers(); }
    let nonce = body.split('&').find_map(|kv| kv.strip_prefix("nonce=")).unwrap_or("");
    let mut message = path.as_bytes().to_vec();
    message.extend_from_slice(&Sha256::digest(format!("{nonce}{body}").as_bytes()));
    let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(&ruby_decode64(secret.as_bytes())) else { return public_headers() };
    mac.update(&message);
    vec![("API-Key".into(), key.into()), ("API-Sign".into(), base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())),
         ("Accept".into(), "application/json".into()), ("Content-Type".into(), "application/x-www-form-urlencoded".into()),
         ("User-Agent".into(), "Honeymaker Ruby".into())]
}

/// Honeymaker::Clients::Kraken#nonce: microseconds, max(now, last + 1), per API key, process-wide. One engine process owns an
/// install (the whole-install lock) and Rails is stopped meanwhile, so no other writer shares the key's sequence.
pub fn next_nonce(key: &str) -> u64 {
    static LAST: std::sync::Mutex<Option<std::collections::HashMap<[u8; 32], u64>>> = std::sync::Mutex::new(None);
    let id: [u8; 32] = Sha256::digest(key.as_bytes()).into();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
    let mut g = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let last = g.get_or_insert_with(Default::default).entry(id).or_insert(0);
    *last = now.max(*last + 1);
    *last
}

/// The wire form of a private call: `nonce` first, then the body's fields in order (the gem's hash order).
pub(crate) fn private_form(r: &HttpRequest, key: &str) -> String {
    let mut pairs = vec![("nonce".to_string(), next_nonce(key).to_string())];
    pairs.extend(r.body.as_ref().and_then(Value::as_object).into_iter().flatten()
        .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))));
    www_form(&pairs)
}

// ---- parsing: Exchanges::Kraken#parse_order_data and friends

/// A Kraken decimal: absent or null is None (Ruby's nil). Anything present must be a finite decimal within BigDec's bounds,
/// or the whole answer is unreadable — never a zero price, fill or balance. DIVERGES from Ruby on purpose (ruby::json_to_d):
/// Rails' `.to_d` reads "garbage" as 0 and "NaN" as NaN.
fn dec(v: &Value) -> Result<Option<BigDec>, VenueError> { crate::ruby::json_to_d(v).map_err(|_| unreadable()) }
pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }

/// `Ok(())` for a clean body; `Rejected` for a non-empty `error`; `Ambiguous` when `error` is missing or not an array, or a
/// success body has no `result` (honeymaker 0.12.3: unreadable).
pub(crate) fn check(body: &Value) -> Result<(), VenueError> {
    let Some(e) = body["error"].as_array() else { return Err(unreadable()) };
    if !e.is_empty() { return Err(VenueError::Rejected(e.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())); }
    if body.get("result").is_none() { return Err(unreadable()); }
    Ok(())
}

/// `parse_order_data` raises on an unknown status or order type; here that is an error, never a guess.
pub(crate) fn parse_order(txid: &str, o: &Value) -> Result<OrderState, VenueError> {
    let status = match o["status"].as_str() {
        Some("open") => OrderStatus::Open, Some("closed") => OrderStatus::Closed,
        Some("canceled") | Some("expired") => OrderStatus::Cancelled, Some("pending") => OrderStatus::Unknown,
        other => return Err(VenueError::Ambiguous(format!("Unknown Kraken order status: {}", other.unwrap_or("")))),
    };
    if !matches!(o["descr"]["ordertype"].as_str(), Some("market") | Some("limit")) {
        return Err(VenueError::Ambiguous(format!("Unknown Kraken order type: {}", o["descr"]["ordertype"].as_str().unwrap_or(""))));
    }
    let viqc = o["oflags"].as_str().is_some_and(|f| f.split(',').any(|x| x == "viqc"));
    let limit = o["descr"]["ordertype"] == "limit";
    // Kraken's parse_venue_number requires every order number; absent fills never mean zero.
    let required = |value: &Value| dec(value)?.ok_or_else(unreadable);
    let vol = Some(required(&o["vol"])?);
    let mut price = required(&o["price"])?;
    if price.is_zero() && limit { price = required(&o["descr"]["price"])?; }
    Ok(OrderState {
        asset_class: None,
        txid: txid.into(),
        status,
        price: (!price.is_zero()).then_some(price), amount: if viqc { None } else { vol.clone() }, quote_amount: if viqc { vol } else { None },
        amount_exec: required(&o["vol_exec"])?, quote_amount_exec: Some(required(&o["cost"])?), limit,
        sell: o["descr"]["type"] == "sell",
        pair: o["descr"]["pair"].as_str().map(str::to_string),
    })
}

/// Exchanges::Kraken#get_ask_price / #get_last_price over a checked Ticker body: the first result entry's `a[0]` / `c[0]`. A
/// zero book raises "Wrong … price", naming the pair (kraken.rb:225, :255).
pub(crate) fn ticker_price(body: &Value, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
    let Some((_, t)) = body["result"].as_object().and_then(|m| m.iter().next()) else {
        return Err(VenueError::Rejected(vec![format!("Failed to get Kraken {} ticker information", ticker.ticker)]));
    };
    let (key, label) = match side { PriceSide::Ask => ("a", "ask"), PriceSide::Last => ("c", "last") };
    let p = dec(&t[key][0])?.unwrap_or_else(BigDec::zero);
    if p.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.ticker, p.to_s_f())])); }
    Ok(p)
}

/// Exchanges::Kraken#get_balances for one asset: balance − hold_trade under the gem's ASSET_MAP. Rails assigns per row, so
/// with several rows for one asset (ZEUR, EUR.HOLD) the last in body order wins; absent is 0.
pub(crate) fn free_balance(body: &Value, asset_symbol: &str) -> Result<BigDec, VenueError> {
    let mut free = BigDec::zero();
    for (code, b) in body["result"].as_object().into_iter().flatten() {
        let head = code.split('.').next().unwrap_or(code);
        let name = ASSET_MAP.iter().find(|(k, _)| *k == head).map_or(head, |(_, v)| *v);
        if name == asset_symbol {
            free = &dec(&b["balance"])?.unwrap_or_else(BigDec::zero) - &dec(&b["hold_trade"])?.unwrap_or_else(BigDec::zero);
        }
    }
    Ok(free)
}

/// Honeymaker::Clients::Kraken#aggregate_trades per wanted order, as Exchanges::Kraken#order_data_from_trade_aggregate maps
/// it: closed, VWAP price, amounts unknown. The aggregate names its first trade's pair (Rails looks it up as a ticker).
pub(crate) fn aggregate_trades(trades: &[Value], txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
    let mut by: Vec<(String, BigDec, BigDec, bool, bool, Option<String>)> = vec![];
    for trade in trades {
        let Some(o) = trade["ordertxid"].as_str().filter(|o| txids.iter().any(|t| t == o)) else { continue };
        // Both amounts are required, as in parse_order (the gem raises converting a missing one): a malformed trade makes the
        // answer unreadable, so the order stays waiting, never a closed zero fill that frees its money.
        let required = |value: &Value| dec(value)?.ok_or_else(unreadable);
        let (vol, cost) = (required(&trade["vol"])?, required(&trade["cost"])?);
        match by.iter_mut().find(|(t, ..)| t == o) {
            Some(e) => { e.1 = &e.1 + &vol; e.2 = &e.2 + &cost; }
            None => by.push((o.to_string(), vol, cost, trade["ordertype"] == "limit", trade["type"] == "sell", trade["pair"].as_str().map(str::to_string))),
        }
    }
    Ok(by.into_iter().map(|(txid, vol, cost, limit, sell, pair)| OrderState {
        asset_class: None,
        txid, status: OrderStatus::Closed, price: cost.div(&vol), amount: None, quote_amount: None, amount_exec: vol, quote_amount_exec: Some(cost), limit, sell, pair,
    }).collect())
}

// ---- the adapter

#[derive(Clone)]
pub struct KrakenVenue<T: Transport> { transport: T, base: String }

impl<T: Transport> KrakenVenue<T> {
    pub fn new(transport: T) -> Self { Self { transport, base: URL.into() } }
    /// Another host: local test servers only.
    #[doc(hidden)]
    pub fn with_base(mut self, base: &str) -> Self { self.base = base.into(); self }

    fn public(&self, path: &str, query: Vec<(&'static str, String)>) -> HttpRequest {
        HttpRequest { method: "GET", base: self.base.clone(), path: path.into(), query, body: None, not_after: None }
    }
    /// A private call: its fields as a JSON object of strings, in the gem's order. The transport adds the nonce and signs.
    fn private(&self, path: &str, fields: &[(&str, String)]) -> HttpRequest {
        let body: Map<String, Value> = fields.iter().map(|(k, v)| (k.to_string(), Value::String(v.clone()))).collect();
        HttpRequest { method: "POST", base: self.base.clone(), path: path.into(), query: vec![], body: Some(Value::Object(body)), not_after: None }
    }

    /// One call as honeymaker's with_rescue + Exchanges::Kraken read it, then the body's `error`/`result` check. A transport
    /// failure that sent nothing is Transient; one that may have sent is Transient on a read and Ambiguous on a placement
    /// (Exchange#ambiguous_placement_error?). A non-2xx answer is Faraday's raise_error: the body (or its status) refused,
    /// except that on a placement only a 4xx is definitive. A 2xx that is not a readable JSON object is unreadable.
    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        let redact = |m: String| self.transport.redact_diagnostic(&m);
        let resp = match self.transport.send(r).await {
            Ok(resp) => resp,
            Err(TransportError::Permanent(m)) => return Err(VenueError::Rejected(vec![redact(m)])),
            Err(TransportError::NotSent(m)) => return Err(VenueError::Transient(redact(m))),
            Err(TransportError::MaybeSent(m)) if placement => return Err(VenueError::Ambiguous(redact(m))),
            Err(TransportError::MaybeSent(m)) => return Err(VenueError::Transient(redact(m))),
        };
        if !(200..300).contains(&resp.status) {
            // Spec E §2.10, a listed divergence: HTTP 429/418 on a read is a throttle (polling's RateLimited wait); Rails fails
            // it as a generic error. Never on a placement, where the answer below stays definitive.
            if !placement && matches!(resp.status, 429 | 418) {
                return Err(VenueError::Rejected(vec![format!("the server responded with status {} ({HTTP_THROTTLE})", resp.status)]));
            }
            let message = if resp.body.trim().is_empty() { format!("the server responded with status {}", resp.status) } else { resp.body.clone() };
            return Err(if placement && !(400..500).contains(&resp.status) { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) });
        }
        let body = match http::decode_json(&resp.body) {
            Ok(v) if v.is_object() => v,
            Ok(_) | Err(DecodeError::NotJson | DecodeError::OutOfRange(_)) => return Err(unreadable()),
        };
        check(&body)?;
        Ok(body)
    }
}

impl<T: Transport> Venue for KrakenVenue<T> {
    fn rules(&self) -> &'static VenueRules { &KRAKEN }

    /// Exchanges::Kraken#get_ticker_information: public Ticker by the ticker's own pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let body = self.call(&self.public("/0/public/Ticker", vec![("pair", ticker.ticker.clone())]), false).await?;
        ticker_price(&body, ticker, side)
    }

    /// #set_market_order / #set_limit_order through Clients::Kraken#add_order: the gem's fields in its order, plus Rust's
    /// `cl_ord_id` and `deadline` (RFC 3339, UTC, milliseconds; Kraken drops the order after it). Sent at most once.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let (ordertype, price) = match &o.kind { OrderKind::Market => ("market", None), OrderKind::Limit { price } => ("limit", Some(price.clone())) };
        let mut fields = vec![("ordertype", ordertype.to_string()), ("type", "buy".into()), ("volume", o.volume.clone()), ("pair", o.pair.clone()),
                              ("cl_ord_id", o.cl_ord_id.clone())];
        if let Some(p) = price { fields.push(("price", p)); }
        if o.quote_volume { fields.push(("oflags", "viqc".into())); }
        fields.push(("deadline", o.deadline.to_rfc3339_opts(SecondsFormat::Millis, true)));
        // The absolute bound on the send, as for every venue: `deadline` is `at` + the send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.private("/0/private/AddOrder", &fields) };
        let body = self.call(&r, true).await?;
        // Accepted with no id: Exchanges::Kraken's `unacknowledged` (kraken.rb:782), which may have placed the order.
        body["result"]["txid"][0].as_str().filter(|t| !t.is_empty()).map(str::to_string)
            .ok_or_else(|| VenueError::Ambiguous(format!("Failed to set Kraken {ordertype} order (order_id is nil)")))
    }

    /// #get_orders over one slice (the engine slices by 50): QueryOrders with the ids joined; ids Kraken omits are absent.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        if txids.is_empty() { return Ok(vec![]); }
        let body = self.call(&self.private("/0/private/QueryOrders", &[("txid", txids.join(",")), ("consolidate_taker", "true".into())]), false).await?;
        txids.iter().filter_map(|t| body["result"].get(t).map(|raw| parse_order(t, raw))).collect()
    }

    /// OpenOrders, then ClosedOrders since `since`, each filtered by `cl_ord_id`; a row whose echo matches is the order.
    /// Finding nothing never proves absence (ABSENCE_UNPROVEN).
    async fn order_by_client_id(&self, cl_ord_id: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        // ponytail: one ClosedOrders page; with the filter honoured one page answers. A miss stays unproven either way.
        for (path, key, fields) in [("/0/private/OpenOrders", "open", vec![("cl_ord_id", cl_ord_id.to_string())]),
                                    ("/0/private/ClosedOrders", "closed", vec![("cl_ord_id", cl_ord_id.to_string()), ("start", since.timestamp().to_string())])] {
            let body = self.call(&self.private(path, &fields), false).await?;
            for (txid, raw) in body["result"][key].as_object().into_iter().flatten() {
                if raw["cl_ord_id"] == cl_ord_id { return parse_order(txid, raw).map(Some); }
            }
        }
        Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into()))
    }

    /// Exchanges::Kraken#recover_missing_from_trades → Clients::Kraken#closed_orders_from_trades: TradesHistory from `since`
    /// (the engine passes created_at − 1 h), paged by `ofs` until `count` or 20 pages, a trade served twice counted once.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        if txids.is_empty() { return Ok(vec![]); }
        let (mut trades, mut seen, mut offset) = (vec![], std::collections::HashSet::new(), 0usize);
        for _ in 0..TRADES_MAX_PAGES {
            let body = self.call(&self.private("/0/private/TradesHistory", &[("start", since.timestamp().to_string()), ("ofs", offset.to_string())]), false).await?;
            let page = body["result"]["trades"].as_object().cloned().unwrap_or_default();
            if page.is_empty() { break; }
            offset += page.len();
            for (id, t) in page { if seen.insert(id) { trades.push(t); } }
            // `count.to_i`: an Integer, a numeric String, or else 0.
            let count = match &body["result"]["count"] { Value::Number(n) => n.as_u64().unwrap_or(0), Value::String(s) => s.trim().parse().unwrap_or(0), _ => 0 };
            if offset as u64 >= count { break; }
        }
        aggregate_trades(&trades, txids)
    }

    /// Bot::Fundable's free balance: BalanceEx's balance − hold_trade.
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        let body = self.call(&self.private("/0/private/BalanceEx", &[]), false).await?;
        free_balance(&body, asset_symbol)
    }
}
