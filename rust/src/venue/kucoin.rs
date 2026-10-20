//! Honeymaker::Clients::Kucoin and Exchanges::Kucoin for the engine's spot orders (plan K, slices K1-K3): prices, balances,
//! placement, polls, cancel and recovery by client order id. Rails is the oracle; each method names the Ruby it ports. The
//! classic (LF) endpoints, as Rails runs them (plan K ruling O1); `venue::live` builds it for every KuCoin bot.
//!
//! Signing (Honeymaker::Clients::Kucoin#signed_headers): KC-API-SIGN = base64(HMAC-SHA256(secret, timestamp + METHOD + path +
//! `?query` (a GET with params) | the JSON body (a POST) | nothing (a DELETE))), the passphrase likewise HMAC'd, key version 2.
//! The transport (`http::ReqwestTransport::kucoin`) signs at send time over exactly the query and body bytes it sends.
use super::binance::{build_query, Pairs};
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, KUCOIN as RULES};
use crate::ruby::BigDec;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

pub const URL: &str = "https://api.kucoin.com";
pub const KUCOIN: &str = "Exchanges::Kucoin";
pub const UNREADABLE: &str = "KuCoin: unreadable response";
/// Exchanges::Kucoin#placement_result (R0b): a 200000 answer with no orderId took the order, or may have.
pub const NO_ORDER_ID: &str = "KuCoin answered the order with no order id";
/// Plan K r4: on KuCoin absence is never concluded. The exact client-order lookup only sees active orders and the done-orders
/// scan only what KuCoin still keeps, so a miss in both proves nothing; the intent waits for `deltabadger resolve-placement`.
/// A divergence: Rails sends a fresh random clientOid and has no lookup at all.
pub const ABSENCE_UNPROVEN: &str = "KuCoin: neither the client order lookup nor the done-orders scan found this order, and KuCoin cannot prove it absent; settle it with `deltabadger resolve-placement`";
/// allTickers is the whole market in one body (about 1 MB): its read stops at this many bytes, never a price from a cut body.
pub const ALL_TICKERS_LIMIT: usize = 4 * 1024 * 1024;
/// Exchanges::Kucoin::ORDER_NOT_FOUND_MESSAGES: a forgotten order is told by its message only (its code, 400100, is generic).
pub const ORDER_NOT_FOUND_MESSAGES: [&str; 5] = ["the order does not exist", "order not exist", "order_not_exist", "ordernotexist", "order not found"];
/// Exchanges::Kucoin::FILLS_WINDOW: `/api/v1/fills` (and the order lists) serve at most a 7-day range.
const WINDOW_MS: i64 = 7 * 86_400_000;
/// Exchanges::Kucoin::FILLS_RANGE_CAP: KuCoin answers at most 50,000 rows for one range.
const RANGE_CAP: i64 = 50_000;
const PAGE_SIZE: &str = "500";
/// ponytail: a page walk stops at 100 pages (50,000 rows, the range cap) and fails, never reads as complete.
const MAX_PAGES: i64 = 100;

/// Honeymaker's #get_public paths: sent with its unauthenticated headers.
pub fn is_public(path: &str) -> bool { path.starts_with("/api/v1/market/") || path.starts_with("/api/v2/symbols") }

/// The query as honeymaker signs it and Faraday sends it (Faraday::Utils.build_query: sorted, escaped).
pub fn query_text(r: &HttpRequest) -> String { build_query(&r.query) }

/// The body bytes: #post_signed signs `body.compact.to_json` and Faraday's JSON middleware sends the same; the fields keep
/// the order the client built them in (serde_json's preserve_order).
pub fn body_text(r: &HttpRequest) -> Option<String> { r.body.as_ref().map(Value::to_string) }

fn b64_hmac(secret: &str, message: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(message.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// The headers honeymaker sends with `r` at `timestamp_ms`: #signed_headers, or #unauthenticated_headers on a public read.
pub fn headers(r: &HttpRequest, key: &str, secret: &str, passphrase: &str, timestamp_ms: i64) -> Vec<(String, String)> {
    let mut out = vec![("Accept".to_string(), "application/json".to_string()), ("Content-Type".to_string(), "application/json".to_string())];
    if is_public(&r.path) { return out; }
    let ts = timestamp_ms.to_string();
    let query = query_text(r);
    let tail = match r.method {
        "GET" if !query.is_empty() => format!("?{query}"),
        "POST" => body_text(r).unwrap_or_default(),
        _ => String::new(), // #delete_signed signs the path alone
    };
    let sign = b64_hmac(secret, &format!("{ts}{}{}{tail}", r.method, r.path));
    let mut signed = vec![("KC-API-KEY".to_string(), key.to_string()), ("KC-API-SIGN".to_string(), sign), ("KC-API-TIMESTAMP".to_string(), ts),
                          ("KC-API-PASSPHRASE".to_string(), b64_hmac(secret, passphrase)), ("KC-API-KEY-VERSION".to_string(), "2".to_string())];
    signed.append(&mut out);
    signed
}

#[derive(Clone)]
pub struct KucoinVenue<T: Transport> { transport: T, base: String, pairs: Pairs, identity: Option<Value> }

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }

/// Utilities::Hash.dig_or_raise: absent or null is unreadable.
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, VenueError> { v.get(key).filter(|x| !x.is_null()).ok_or_else(unreadable) }

/// A decimal string or number: present and readable, or the answer is unreadable (never a zero; Ruby's `.to_d` reads
/// "garbage" as 0, a listed divergence as on Binance).
fn required(v: &Value) -> Result<BigDec, VenueError> { crate::ruby::json_to_d(v).ok().flatten().ok_or_else(unreadable) }
/// `x&.to_d`: null or absent is None.
fn optional(v: &Value) -> Result<Option<BigDec>, VenueError> { if v.is_null() { Ok(None) } else { required(v).map(Some) } }
/// Ruby's truthiness of a JSON value: only null (or absent) and false are false.
fn truthy(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) }
/// `#to_s` of a scalar, as Ruby interpolates it.
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), other => other.to_string() } }
/// `#to_i` of a JSON page count (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_default(), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
/// The envelope's code as `to_s` reads it; an answer without one is unreadable.
fn code(body: &Value) -> Result<String, VenueError> { match body.get("code") { Some(c @ (Value::String(_) | Value::Number(_))) => Ok(to_s(c)), _ => Err(unreadable()) } }
const OK: &str = "200000";

/// Client#api_error: "KuCoin API error <code>: <msg>", the parts that are present.
pub fn api_error(body: &Value) -> String {
    let msg = Some(&body["msg"]).filter(|m| !m.is_null()).unwrap_or(&body["message"]);
    let detail: Vec<String> = [&body["code"], msg].into_iter().filter(|v| !v.is_null()).map(to_s).filter(|s| !s.is_empty()).collect();
    if detail.is_empty() { "KuCoin API error".into() } else { format!("KuCoin API error {}", detail.join(": ")) }
}
/// Exchanges::Kucoin's `result.data.values_at('code', 'msg').compact.join(' ')`.
fn code_and_msg(body: &Value) -> String { [&body["code"], &body["msg"]].into_iter().filter(|v| !v.is_null()).map(to_s).collect::<Vec<_>>().join(" ") }

/// Exchanges::Kucoin#order_not_found?.
pub fn not_found(messages: &[String]) -> bool {
    let joined = messages.join(" ").to_lowercase();
    ORDER_NOT_FOUND_MESSAGES.iter().any(|m| joined.contains(m))
}

/// Exchanges::Kucoin#parse_order_type: `market` or `limit` (any case), else Rails raises.
fn limit_type(v: &Value) -> Result<bool, VenueError> {
    let t = v.as_str().ok_or_else(unreadable)?;
    match t.to_lowercase().as_str() {
        "market" => Ok(false), "limit" => Ok(true),
        _ => Err(VenueError::Rejected(vec![format!("Unknown KuCoin order type: {t}")])),
    }
}

/// Exchanges::Kucoin#parse_order_data and #parse_order_status. A cancelled order keeps what it dealt; "done" with nothing
/// dealt is Unknown, never Closed. The price is dealFunds / dealSize (rounded half up to the pair's tick) when both are
/// positive, else the order's own price; a zero price is none.
pub fn parse_order(order_id: &str, o: &Value, pairs: &Pairs) -> Result<OrderState, VenueError> {
    let symbol = field(o, "symbol")?.as_str().ok_or_else(unreadable)?;
    let limit = limit_type(field(o, "type")?)?;
    let (deal_funds, deal_size) = (optional(&o["dealFunds"])?, optional(&o["dealSize"])?);
    let mut price = match (&deal_funds, &deal_size) {
        (Some(f), Some(s)) if f.is_positive() && s.is_positive() => f.div(s).ok_or_else(unreadable)?,
        _ => optional(&o["price"])?.unwrap_or_else(BigDec::zero),
    };
    if price.is_positive() { if let Some(p) = pairs.by_symbol.get(symbol) { price = price.round(p.price_decimals); } }
    let sell = field(o, "side")?.as_str().ok_or_else(unreadable)?.eq_ignore_ascii_case("sell");
    let amount_exec = deal_size.unwrap_or_else(BigDec::zero);
    let status = if truthy(&o["cancelExist"]) { OrderStatus::Cancelled } else if truthy(&o["isActive"]) { OrderStatus::Open }
                 else if amount_exec.is_positive() { OrderStatus::Closed } else { OrderStatus::Unknown };
    Ok(OrderState { txid: order_id.into(), status, price: (!price.is_zero()).then_some(price), amount: optional(&o["size"])?, quote_amount: optional(&o["funds"])?,
                    amount_exec, quote_amount_exec: Some(deal_funds.unwrap_or_else(BigDec::zero)), limit, sell, pair: Some(symbol.into()), asset_class: None })
}

/// The prices of an allTickers answer, as Exchanges::Kucoin#get_tickers_prices reads them: every `data.ticker` row by its
/// `dig_or_raise`d `symbol`, its `last` kept only when positive, a later row for a symbol replacing an earlier one. No
/// `data.ticker` is Rails' "Failed to get KuCoin tickers". Shared by the engine (`tickers_prices`) and the figures.
pub fn all_tickers(body: &Value) -> Result<Vec<(String, BigDec)>, VenueError> {
    let failed = || VenueError::Rejected(vec!["Failed to get KuCoin tickers".into()]);
    // Hash#dig: a `data` that is not a hash (or a body that is not one) raises.
    let data = match body.as_object().ok_or_else(unreadable)?.get("data") { None | Some(Value::Null) => return Err(failed()), Some(d) => d };
    let rows = match data.as_object().ok_or_else(unreadable)?.get("ticker") { None | Some(Value::Null) => return Err(failed()), Some(t) => t };
    let mut out: Vec<(String, BigDec)> = vec![];
    for row in rows.as_array().ok_or_else(unreadable)? {
        let symbol = super::dig_or_raise(row, "symbol")?.as_str().ok_or_else(unreadable)?.to_string();
        let price = match &row["last"] {
            Value::Null => None,
            v @ (Value::String(_) | Value::Number(_)) => crate::ruby::json_to_d(v).ok().flatten(),
            _ => return Err(unreadable()),
        };
        // `prices_hash[symbol] = price if price.positive?`: a later positive price for the same symbol replaces it.
        if let Some(p) = price.filter(|p| *p > BigDec::zero()) {
            out.retain(|(s, _)| *s != symbol);
            out.push((symbol, p));
        }
    }
    Ok(out)
}

impl<T: Transport> KucoinVenue<T> {
    pub fn new(transport: T, base: &str) -> Self { Self { transport, base: base.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// Another host: local test servers only.
    #[doc(hidden)]
    pub fn with_base(mut self, base: &str) -> Self { self.base = base.into(); self }
    /// The exchange's tickers (`Pairs::load`): venue codes to assets, and each pair's price decimals.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    fn request(&self, method: &'static str, path: String, mut query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        query.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path, query, body, not_after: None }
    }

    /// One call as honeymaker's #with_rescue returns it and Exchange#ambiguous_placement_error? reads it for a placement. A
    /// transport failure that sent nothing is Transient (Rejected on a placement: definitive); one that may have sent is
    /// Transient on a read and Ambiguous on a placement. A non-2xx answer is its body (Faraday's message when empty); on a
    /// placement only a 4xx, or a placement-safe 400002 whatever its status, is definitive. A 2xx that is not JSON is unreadable.
    async fn call(&self, r: &HttpRequest, placement: bool, limit: usize) -> Result<Value, VenueError> {
        http::decode_json(&self.answer(r, placement, limit).await?).map_err(|_| unreadable())
    }

    /// One send as honeymaker's #with_rescue reads it: the 2xx body's text, or the failure.
    async fn answer(&self, r: &HttpRequest, placement: bool, limit: usize) -> Result<String, VenueError> {
        let redact = |m: String| self.transport.redact_diagnostic(&m);
        let resp = match self.transport.send_limited(r, limit).await {
            Ok(resp) => resp,
            Err(TransportError::Permanent(m)) => return Err(VenueError::Rejected(vec![redact(m)])),
            Err(TransportError::NotSent(m)) if placement => return Err(VenueError::Rejected(vec![redact(m)])),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) if !placement => return Err(VenueError::Transient(redact(m))),
            Err(TransportError::MaybeSent(m) | TransportError::NotSent(m)) => return Err(VenueError::Ambiguous(redact(m))),
        };
        if !(200..300).contains(&resp.status) {
            let message = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { resp.body };
            // Honeymaker attaches the status (data: { status: }) and Exchange#raise_on_invalid_key! reads a 401 as a rejected key.
            if !placement && resp.status == 401 { return Err(VenueError::Unauthorized(vec![message])); }
            let definitive = (400..500).contains(&resp.status) || crate::engine::placement::safe_rejection(std::slice::from_ref(&message));
            return Err(if placement && !definitive { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) });
        }
        Ok(resp.body)
    }

    async fn get(&self, path: String, query: Vec<(&'static str, String)>, limit: usize) -> Result<Value, VenueError> {
        self.call(&self.request("GET", path, query, None), false, limit).await
    }

    /// One signed GET as honeymaker's #with_rescue returns it to Exchanges::Kucoin's ledger reads: the 2xx body of at most
    /// `limit` bytes, unparsed (`Ok`), else the failure's text: the answer's body, or Faraday's (or the transport's) message.
    pub async fn read_text(&self, path: &str, query: Vec<(&'static str, String)>, limit: usize) -> Result<String, String> {
        let r = self.request("GET", path.into(), query, None);
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, r.url())),
        }
    }

    /// Clients::Kucoin#get_accounts(type: 'trade') as Exchanges::Kucoin#get_balances reads it: only the success code means
    /// balances, else "Failed to get KuCoin balances: <code> <msg>".
    async fn accounts(&self) -> Result<Vec<Value>, VenueError> {
        let body = self.get("/api/v1/accounts".into(), vec![("type", "trade".into())], http::MAX_BODY).await?;
        if body["data"].is_null() || code(&body).ok().as_deref() != Some(OK) {
            return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin balances: {}", code_and_msg(&body))]));
        }
        let rows = body["data"].as_array().ok_or_else(unreadable)?;
        rows.iter().map(|row| { field(row, "currency")?; Ok(row.clone()) }).collect()
    }

    /// `available` of the last row the predicate picks (absent is 0). `locked`, or before KuCoin's 2026-08-27 rename `holds`,
    /// must be readable too, as #get_balances digs for it.
    async fn available(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let mut free = BigDec::zero();
        for row in self.accounts().await? {
            if !pick(row["currency"].as_str().ok_or_else(unreadable)?) { continue; }
            free = required(field(&row, "available")?)?;
            match row.get("locked") { Some(Value::Null) => {}, Some(l) => { required(l)?; } None => { required(field(&row, "holds")?)?; } }
        }
        Ok(free)
    }

    /// Clients::Kucoin#get_order as Exchanges::Kucoin#get_order reads it: None for an order KuCoin no longer knows (by its
    /// message only, or the gem's "Order not found" for a success envelope without one).
    async fn get_order(&self, order_id: &str) -> Result<Option<Value>, VenueError> {
        let body = match self.get(format!("/api/v1/orders/{order_id}"), vec![], http::MAX_BODY).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m)) if not_found(&m) => return Ok(None),
            other => other?,
        };
        if code(&body)? != OK {
            let message = vec![api_error(&body)];
            return if not_found(&message) { Ok(None) } else { Err(VenueError::Rejected(message)) };
        }
        Ok(Some(body["data"].clone()).filter(|d| !d.is_null()))
    }

    /// Exchanges::Kucoin#each_page: currentPage 1.. to totalPage, every page on the success code ("Failed to get KuCoin
    /// history: <code> <msg>" else), each item handed to `each`. `capped`: a first page naming more than RANGE_CAP rows fails.
    /// ponytail: Rails waits a throttled page out (6 × 10 s) and halves an over-cap fills range down to an hour; here both
    /// fail the read, and the caller's poll is retried later, so nothing is ever read as complete.
    async fn each_page(&self, path: &str, query: &[(&'static str, String)], capped: bool, mut each: impl FnMut(&Value)) -> Result<(), VenueError> {
        for page in 1..=MAX_PAGES {
            let mut q = query.to_vec();
            q.extend([("currentPage", page.to_string()), ("pageSize", PAGE_SIZE.into())]);
            let body = self.get(path.into(), q, http::MAX_BODY).await?;
            if code(&body).ok().as_deref() != Some(OK) {
                return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin history: {}", code_and_msg(&body))]));
            }
            let data = &body["data"];
            if capped && page == 1 && to_i(&data["totalNum"]) > RANGE_CAP {
                return Err(VenueError::Rejected(vec![format!("KuCoin has more than {RANGE_CAP} fills in one range")]));
            }
            match &data["items"] { Value::Null => {}, Value::Array(items) => items.iter().for_each(&mut each), _ => return Err(unreadable()) }
            if page >= to_i(&data["totalPage"]) { return Ok(()); }
        }
        Err(VenueError::Rejected(vec![format!("Too many pages of KuCoin {path}")]))
    }

    /// Weekly windows [from, to) as Exchanges::Kucoin walks them, each read to its end.
    async fn windows(&self, path: &str, query: &[(&'static str, String)], from_ms: i64, to_ms: i64, capped: bool, mut each: impl FnMut(&Value)) -> Result<(), VenueError> {
        let mut start = from_ms;
        while start < to_ms {
            let end = (start + WINDOW_MS).min(to_ms);
            let mut q = query.to_vec();
            q.extend([("startAt", start.to_string()), ("endAt", end.to_string())]);
            self.each_page(path, &q, capped, &mut each).await?;
            start = end;
        }
        Ok(())
    }

    /// Exchanges::Kucoin#order_data_from_fills: the summed sizes and funds, the price funds / size at the tick, closed.
    fn order_from_fills(&self, order_id: &str, fills: &[(String, Value)]) -> Result<OrderState, VenueError> {
        let (mut amount, mut quote) = (BigDec::zero(), BigDec::zero());
        for (_, f) in fills { amount = &amount + &required(field(f, "size")?)?; quote = &quote + &required(field(f, "funds")?)?; }
        let first = &fills[0].1;
        let symbol = first["symbol"].as_str().ok_or_else(unreadable)?;
        let mut price = if amount.is_positive() { quote.div(&amount) } else { None };
        if let (Some(p), Some(pair)) = (&mut price, self.pairs.by_symbol.get(symbol)) { *p = p.round(pair.price_decimals); }
        Ok(OrderState { txid: order_id.into(), status: OrderStatus::Closed, price, amount: None, quote_amount: None, amount_exec: amount,
                        quote_amount_exec: Some(quote), limit: limit_type(&Value::String(to_s(&first["type"])))?,
                        sell: to_s(&first["side"]).eq_ignore_ascii_case("sell"), pair: Some(symbol.into()), asset_class: None })
    }
}

impl<T: Transport> super::Venue for KucoinVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    /// #get_ask_price / #get_bid_price through #get_bid_ask_price (level2_20: the first bid and ask, price and size), and
    /// #get_last_price (allTickers, the symbol's `last`; null is 0). A zero price raises "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let name = &ticker.ticker;
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.get("/api/v1/market/orderbook/level2_20".into(), vec![("symbol", name.clone())], http::MAX_BODY).await?;
                let data = &body["data"];
                if data.is_null() { return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin order book for {name}")])); }
                let level = |book: &str, i: usize| required(&data[book][0][i]);
                let (bid, ask) = (level("bids", 0)?, level("asks", 0)?);
                level("bids", 1)?;
                level("asks", 1)?;
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => {
                let body = self.get("/api/v1/market/allTickers".into(), vec![], ALL_TICKERS_LIMIT).await?;
                let Some(rows) = body["data"]["ticker"].as_array() else { return Err(VenueError::Rejected(vec!["Failed to get KuCoin last price".into()])) };
                let row = rows.iter().find(|t| t["symbol"].as_str() == Some(name))
                    .ok_or_else(|| VenueError::Rejected(vec![format!("Ticker {name} not found on KuCoin")]))?;
                (optional(&row["last"])?.unwrap_or_else(BigDec::zero), "last")
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {name}: {}", price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Kucoin#place_order: POST /api/v1/orders with clientOid, side,
    /// symbol, type, then a market order's `size` or `funds`, or a limit order's `size` and `price`. `clientOid` is the
    /// intent id (spec E §2.6; Rails sends a fresh random UUID). Sent at most once. The stored id is the raw orderId. An
    /// error envelope is honeymaker's api_error text (the placement layer reads 500000/230005 as unknown, 400002 as safe);
    /// a success without an orderId, or an answer without a code, may have placed the order: Ambiguous.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut body = serde_json::Map::new();
        for (k, v) in [("clientOid", o.cl_ord_id.as_str()), ("side", o.side.as_str()), ("symbol", o.pair.as_str())] { body.insert(k.into(), v.into()); }
        match &o.kind {
            OrderKind::Market => {
                body.insert("type".into(), "market".into());
                body.insert(if o.quote_volume { "funds" } else { "size" }.into(), o.volume.clone().into());
            }
            OrderKind::Limit { price } => {
                body.insert("type".into(), "limit".into());
                body.insert("size".into(), o.volume.clone().into());
                body.insert("price".into(), price.clone().into());
            }
        }
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.request("POST", "/api/v1/orders".into(), vec![], Some(Value::Object(body))) };
        let answer = self.call(&r, true, http::MAX_BODY).await?;
        if code(&answer)? != OK { return Err(VenueError::Rejected(vec![api_error(&answer)])); }
        match &answer["data"]["orderId"] {
            Value::String(id) if !id.is_empty() => Ok(id.clone()),
            _ => Err(VenueError::Ambiguous(NO_ORDER_ID.into())),
        }
    }

    /// #get_orders: #get_order per id; an order KuCoin no longer knows is left out (`missing`, which polling settles from
    /// its fills, `fills_from_trades`); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids {
            if let Some(raw) = self.get_order(id).await? { out.push(parse_order(id, &raw, &self.pairs)?); }
        }
        Ok(out)
    }

    /// #get_order (Bot::FetchAndUpdateOrderJob). A forgotten order fails here: the follow-up has no order row to date a fills
    /// read from, so the row waits for the sweep, which reads its fills (`fills_from_trades`) as #order_from_fills does.
    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.get_order(txid).await? {
            Some(raw) => Ok((vec![parse_order(txid, &raw, &self.pairs)?], vec![])),
            None => Err(VenueError::Rejected(vec![format!("KuCoin no longer knows order {txid}")])),
        }
    }

    /// Recovery (plan K r4) may only FIND the order: the exact lookup `GET /api/v1/order/client-order/{clientOid}` (active
    /// orders only; a hit naming this clientOid and symbol is authoritative), then `GET /api/v1/orders?status=done` over weekly
    /// windows from `since` to now, every page, matching clientOid. Neither finding it is never absence (ABSENCE_UNPROVEN);
    /// a failed scan page is its own error.
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let mine = |o: &Value| o["clientOid"].as_str() == Some(cl_ord_id) && o["symbol"].as_str() == Some(symbol);
        let found = |o: &Value| -> Result<OrderState, VenueError> { parse_order(o["id"].as_str().filter(|s| !s.is_empty()).ok_or_else(unreadable)?, o, &self.pairs) };
        if let Ok(body) = self.get(format!("/api/v1/order/client-order/{cl_ord_id}"), vec![], http::MAX_BODY).await {
            if code(&body).ok().as_deref() == Some(OK) && mine(&body["data"]) { return found(&body["data"]).map(Some); }
        }
        let mut hit = None;
        let query = [("status", "done".to_string()), ("symbol", symbol.to_string())];
        self.windows("/api/v1/orders", &query, since.timestamp_millis(), self.transport.now().timestamp_millis(), false, |o| {
            if hit.is_none() && mine(o) { hit = Some(o.clone()); }
        }).await?;
        match hit { Some(o) => found(&o).map(Some), None => Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())) }
    }

    /// Exchanges::Kucoin#order_from_fills for each order #get_orders named missing (R0c): its fills by orderId in weekly windows
    /// from `since` (the oldest missing row, less an hour) to now, each fill once by tradeId. A fill books the order closed;
    /// no fill leaves it missing (Bot::StaleOrderResolver decides); a failed read fails, so nothing is written off.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        let now_ms = self.transport.now().timestamp() * 1000;
        for id in txids {
            let mut fills: Vec<(String, Value)> = vec![];
            self.windows("/api/v1/fills", &[("orderId", id.clone())], since.timestamp() * 1000, now_ms, true, |f| {
                if to_s(&f["orderId"]) != *id { return; }
                let trade = to_s(&f["tradeId"]);
                match fills.iter_mut().find(|(t, _)| *t == trade) { Some(slot) => slot.1 = f.clone(), None => fills.push((trade, f.clone())) }
            }).await?;
            if !fills.is_empty() { out.push(self.order_from_fills(id, &fills)?); }
        }
        Ok(out)
    }

    /// #get_balances' `available` for the quote asset: each trade-account row's currency mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.available(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    /// The base's available amount for a seller: the trade-account row of the ticker's base code.
    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.available(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.available(|code| code == ticker.quote_code).await }

    /// Exchanges::Kucoin#get_tickers_prices: the allTickers read `price(Last)` makes, every `data.ticker` row by its
    /// `dig_or_raise`d `symbol`, the price `last&.to_d || 0` kept only when positive. No `data.ticker` is Rails' "Failed to
    /// get KuCoin tickers". DIVERGES on purpose as Binance does (ruby::json_to_d, Q6): a `last` that is no decimal is left
    /// out, where String#to_d reads "12x" as 12. Rails caches the answer for a minute; this reads fresh. Each price is its
    /// decimal's `to_s('F')`.
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        // A 2xx body that is not JSON is honeymaker's Failure (its text), never a raise.
        let text = self.answer(&self.request("GET", "/api/v1/market/allTickers".into(), vec![], None), false, ALL_TICKERS_LIMIT).await?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        Ok(all_tickers(&body)?.into_iter().map(|(symbol, price)| (symbol, Value::String(price.to_s_f()))).collect())
    }

    /// #cancel_order through Clients::Kucoin#cancel_order: DELETE /api/v1/orders/{id}. A divergence: Rails takes any 2xx as a
    /// cancel; here only the success code is, an error envelope is the venue's refusal.
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let body = self.call(&self.request("DELETE", format!("/api/v1/orders/{txid}"), vec![], None), false, http::MAX_BODY).await?;
        if code(&body)? == OK { Ok(()) } else { Err(VenueError::Rejected(vec![api_error(&body)])) }
    }
}
