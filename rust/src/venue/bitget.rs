//! Honeymaker::Clients::Bitget and Exchanges::Bitget for the engine's spot orders (plan VB slice A4): prices, balances,
//! placement, polls, cancel and the recovery by client order id. The classic v2 spot API, as Rails runs it (owner ruling:
//! no UTA/v3). Rails is the oracle (post-audit, AUDIT-bitget G1-G7, merged in #608); each method names the Ruby it ports.
//! `venue::live` builds it for every Bitget bot.
//!
//! Signing (Honeymaker::Clients::Bitget#signed_headers): ACCESS-SIGN = base64(HMAC-SHA256(secret, timestamp + METHOD + path +
//! `?query` (a GET with params) | the JSON body (a POST))), the passphrase sent as it is, `locale: en-US` (`sign::BITGET`). The
//! transport (`http::ReqwestTransport::bitget`) signs at send time over exactly the query and body bytes it sends.
//!
//! A market order has one `size`, in quote coins for a buy and base coins for a sell (G1). An amount in the other unit is
//! converted before the intent is written (`engine::venue_rules::WireFormat::Bitget`): at the touch the order was sized at,
//! floored to the tick, which is what Rails' #market_price_for re-reads through its 5-second price cache; floored to that
//! unit's decimals. Nothing is re-read or resized here.
//!
//! Where it differs on purpose (plan VB §5, named in tests/parity.rs): the order carries `clientOid` = the intent id (D9;
//! Rails sends none) and recovery asks for it (`order_by_client_id`), which can only FIND an order (absence is never
//! concluded); a placement that may have reached Bitget (a lost reply, a 5xx, an unreadable answer, a success without an
//! orderId) keeps the intent where Rails writes a failed (or a "BTCUSDT-") row; a number that is no decimal is refused where
//! Rails reads it as 0.
use super::binance::{build_query, Pairs};
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, Side, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, BITGET as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const URL: &str = "https://api.bitget.com";
pub const BITGET: &str = "Exchanges::Bitget";
pub const UNREADABLE: &str = "Bitget: unreadable response";
pub const ORDERBOOK_PATH: &str = "/api/v2/spot/market/orderbook";
pub const TICKERS_PATH: &str = "/api/v2/spot/market/tickers";
pub const ASSETS_PATH: &str = "/api/v2/spot/account/assets";
pub const PLACE_PATH: &str = "/api/v2/spot/trade/place-order";
pub const ORDER_INFO_PATH: &str = "/api/v2/spot/trade/orderInfo";
pub const CANCEL_PATH: &str = "/api/v2/spot/trade/cancel-order";
/// The success code of every Bitget envelope.
pub const OK: &str = "00000";
/// Exchanges::Bitget::ORDER_GONE_CODES: orderInfo for an order Bitget no longer has.
pub const ORDER_GONE_CODES: [&str; 2] = ["40109", "43001"];
/// A 00000 answer to the order without an orderId: Rails stores "<symbol>-"; Bitget took the order, or may have.
pub const NO_ORDER_ID: &str = "Bitget answered the order with no order id";
pub const NO_CODE: &str = "Bitget answered the order with no result code";
/// The whole market's tickers in one body: its read stops at this many bytes, never a price from a cut body.
pub const TICKERS_LIMIT: usize = 4 * 1024 * 1024;

/// Honeymaker's #get_public paths: sent with its unauthenticated headers.
pub fn is_public(path: &str) -> bool { path.starts_with("/api/v2/spot/public/") || path.starts_with("/api/v2/spot/market/") }

/// The headers honeymaker sends with `r` at `timestamp_ms`: #signed_headers (`sign::BITGET`), or #unauthenticated_headers on a
/// public read.
pub fn headers(r: &HttpRequest, key: &str, secret: &str, passphrase: &str, timestamp_ms: i64) -> Vec<(String, String)> {
    if is_public(&r.path) { return super::sign::json_headers(); }
    super::sign::prehash(&super::sign::BITGET, r, &build_query(&r.query), key, secret, passphrase, timestamp_ms)
}

/// Exchanges::Bitget#parse_order_id: "BTCUSDT-123" → (Some("BTCUSDT"), "123"); a legacy id without a dash names no symbol.
pub fn split_order_id(order_id: &str) -> (Option<String>, String) {
    match order_id.split_once('-') { Some((symbol, id)) => (Some(symbol.into()), id.into()), None => (None, order_id.into()) }
}

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }
/// `x&.to_d`: null or absent is None (honeymaker's BigDecimal() has refused anything that is no decimal already).
fn optional(v: &Value) -> Result<Option<BigDec>, VenueError> { if v.is_null() { Ok(None) } else { required(v).map(Some) } }
/// `#to_s` of a scalar, as Ruby interpolates it (nil is "").
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
/// Honeymaker's `result.data["code"] == "00000"`: a String only.
fn ok(body: &Value) -> bool { body.get("code").and_then(Value::as_str) == Some(OK) }

/// Client#api_error: "Bitget API error <code>: <msg>", the parts that are present (`msg`, else `message`).
pub fn api_error(body: &Value) -> String {
    let msg = Some(&body["msg"]).filter(|m| !m.is_null()).unwrap_or(&body["message"]);
    let detail: Vec<String> = [&body["code"], msg].into_iter().filter(|v| !v.is_null()).map(to_s).filter(|s| !s.is_empty()).collect();
    if detail.is_empty() { "Bitget API error".into() } else { format!("Bitget API error {}", detail.join(": ")) }
}

/// Exchanges::Bitget#bitget_envelope of a failure's text: the JSON body's `code` and `msg`, else no code and the text itself.
pub fn envelope(text: &str) -> (Option<Value>, Option<Value>) {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(o)) => (o.get("code").cloned().filter(|v| !v.is_null()), o.get("msg").cloned().filter(|v| !v.is_null())),
        _ => (None, Some(Value::String(text.into()))),
    }
}

/// Exchanges::Bitget#order_gone?: the gem's "Order not found" (an empty list), or 40109 / 43001 as the body's String code or
/// in honeymaker's "API error <code>" text.
pub fn gone(messages: &[String]) -> bool {
    let message = messages.join(" ");
    if message.contains("Order not found") { return true; }
    let code = match messages.first().map(|m| envelope(m).0) {
        Some(Some(code)) => code.as_str().map(str::to_string),
        _ => message.split_once("API error ").map(|(_, rest)| rest.chars().take_while(char::is_ascii_digit).collect::<String>()).filter(|c| !c.is_empty()),
    };
    code.is_some_and(|c| ORDER_GONE_CODES.contains(&c.as_str()))
}

/// Exchanges::Bitget#parse_order_data, after honeymaker's #normalize_order (whose BigDecimal() refuses a number that is no
/// decimal, as here). The price is `priceAvg` when present, else `price`, a zero one none. `size` is quote coins on a market
/// buy and base coins otherwise (G2). ORDER_STATUS_MAP; an order type or a status it does not list is refused, as Rails raises.
pub fn parse_order(order_id: &str, o: &Value) -> Result<OrderState, VenueError> {
    let dig = |k: &str| super::dig_or_raise(o, k);
    let symbol = dig("symbol")?.as_str().ok_or_else(unreadable)?;
    let order_type = dig("orderType")?.as_str().ok_or_else(unreadable)?;
    let limit = match order_type.to_lowercase().as_str() {
        "market" => false, "limit" => true,
        _ => return Err(VenueError::Rejected(vec![format!("Unknown Bitget order type: {order_type}")])),
    };
    let price = match optional(&o["priceAvg"])? { Some(p) => p, None => optional(&o["price"])?.unwrap_or_else(BigDec::zero) };
    let side = dig("side")?.as_str().ok_or_else(unreadable)?.to_lowercase();
    let size = optional(&o["size"])?;
    optional(&o["quoteSize"])?; // read by #normalize_order
    let (amount, quote_amount) = if !limit && side == "buy" { (None, size) } else { (size, None) };
    let amount_exec = optional(&o["baseVolume"])?.unwrap_or_else(BigDec::zero);
    let quote_amount_exec = optional(&o["quoteVolume"])?.unwrap_or_else(BigDec::zero);
    let status = match dig("status")?.as_str() {
        Some("init" | "new") => OrderStatus::Unknown,
        Some("partially_filled" | "live") => OrderStatus::Open,
        Some("filled") => OrderStatus::Closed,
        Some("cancelled") => OrderStatus::Cancelled,
        other => return Err(VenueError::Rejected(vec![format!("Unknown Bitget order status: {}", other.unwrap_or_default())])),
    };
    Ok(OrderState { txid: order_id.into(), status, price: (!price.is_zero()).then_some(price), amount, quote_amount, amount_exec,
                    quote_amount_exec: Some(quote_amount_exec), limit, sell: side == "sell", pair: Some(symbol.into()), asset_class: None })
}

/// The prices of a GET /api/v2/spot/market/tickers answer, as Exchanges::Bitget#get_tickers_prices reads them: no `data` is
/// "Failed to get Bitget tickers"; every row by its `dig_or_raise`d `symbol` and `lastPr`, a later row for a symbol replacing
/// an earlier one (a zero price kept, as Rails keeps it). DIVERGES on purpose as KuCoin's does: a price that is no decimal is
/// left out (String#to_d reads "12x" as 12).
pub fn ticker_prices(body: &Value) -> Result<Vec<(String, BigDec)>, VenueError> {
    let data = match body.as_object().ok_or_else(unreadable)?.get("data") {
        None | Some(Value::Null) => return Err(VenueError::Rejected(vec!["Failed to get Bitget tickers".into()])),
        Some(d) => d,
    };
    let mut out: Vec<(String, BigDec)> = vec![];
    for row in data.as_array().ok_or_else(unreadable)? {
        let symbol = super::dig_or_raise(row, "symbol")?.as_str().ok_or_else(unreadable)?.to_string();
        let price = match super::dig_or_raise(row, "lastPr")? {
            v @ (Value::String(_) | Value::Number(_)) => crate::ruby::json_to_d(v).ok().flatten(),
            _ => return Err(unreadable()),
        };
        if let Some(p) = price {
            out.retain(|(s, _)| *s != symbol);
            out.push((symbol, p));
        }
    }
    Ok(out)
}

#[derive(Clone)]
pub struct BitgetVenue<T: Transport> { transport: T, base: String, pairs: Pairs, identity: Option<Value> }

impl<T: Transport> BitgetVenue<T> {
    pub fn new(transport: T, base: &str) -> Self { Self { transport, base: base.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// The exchange's tickers (`Pairs::load`): venue codes to assets.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    pub fn request(&self, method: &'static str, path: &str, mut query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        query.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path: path.into(), query, body, not_after: None }
    }

    /// One send as honeymaker's #with_rescue reads it: the 2xx body's text, or the failure. A transport failure is the shared
    /// split (`http::transport_failure`). A non-2xx answer is its body (Faraday's message when empty); a read's 401 is
    /// `Unauthorized`; on a placement only a 4xx (Bitget's business errors are HTTP 400), or a placement-safe 40008 whatever
    /// its status, is definitive.
    async fn answer(&self, r: &HttpRequest, placement: bool, limit: usize) -> Result<String, VenueError> {
        let resp = self.transport.send_limited(r, limit).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))?;
        if !(200..300).contains(&resp.status) {
            let message = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { resp.body };
            if !placement && resp.status == 401 { return Err(VenueError::Unauthorized(vec![message])); }
            let definitive = (400..500).contains(&resp.status) || crate::engine::placement::safe_rejection(std::slice::from_ref(&message));
            return Err(if placement && !definitive { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) });
        }
        Ok(resp.body)
    }

    /// `answer`, decoded: a 2xx that is not JSON is unreadable.
    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        http::decode_json(&self.answer(r, placement, http::MAX_BODY).await?).map_err(|_| unreadable())
    }

    async fn get(&self, path: &str, query: Vec<(&'static str, String)>) -> Result<Value, VenueError> { self.call(&self.request("GET", path, query, None), false).await }

    /// One signed GET as honeymaker's #with_rescue returns it to Exchanges::Bitget's ledger and balance reads: the 2xx body of
    /// at most `limit` bytes, unparsed (`Ok`), else the failure's text: the answer's body, or Faraday's (or the transport's) message.
    pub async fn read_text(&self, path: &str, query: Vec<(&'static str, String)>, limit: usize) -> Result<String, String> {
        let r = self.request("GET", path, query, None);
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, r.url())),
        }
    }

    /// #get_bid_ask_price: GET orderbook?symbol=<pair>&limit=1, its first bid and ask. No `data` is "Failed to get Bitget
    /// order book for <pair>"; a missing level is unreadable (Rails raises).
    async fn book(&self, symbol: &str) -> Result<(BigDec, BigDec), VenueError> {
        let body = self.get(ORDERBOOK_PATH, vec![("symbol", symbol.into()), ("limit", "1".into())]).await?;
        let data = &body["data"];
        if data.is_null() { return Err(VenueError::Rejected(vec![format!("Failed to get Bitget order book for {symbol}")])); }
        let level = |book: &str| -> Result<BigDec, VenueError> {
            let level = data[book][0].as_array().ok_or_else(unreadable)?;
            optional(level.get(1).unwrap_or(&Value::Null))?;
            Ok(optional(level.first().unwrap_or(&Value::Null))?.unwrap_or_else(BigDec::zero))
        };
        Ok((level("bids")?, level("asks")?))
    }

    /// Clients::Bitget#get_account_assets as Exchanges::Bitget#get_balances reads it: only the success code with `data` means
    /// balances, else "Failed to get Bitget balances: <code> <msg>"; every row's `coin` must be there.
    async fn assets(&self) -> Result<Vec<Value>, VenueError> {
        let body = self.get(ASSETS_PATH, vec![]).await?;
        if body["data"].is_null() || to_s(&body["code"]) != OK {
            let detail: Vec<String> = [&body["code"], &body["msg"]].into_iter().filter(|v| !v.is_null()).map(to_s).collect();
            return Err(VenueError::Rejected(vec![format!("Failed to get Bitget balances: {}", detail.join(" "))]));
        }
        let rows = body["data"].as_array().ok_or_else(unreadable)?;
        rows.iter().map(|row| { super::dig_or_raise(row, "coin")?.as_str().ok_or_else(unreadable)?; Ok(row.clone()) }).collect()
    }

    /// `available` of the last row the predicate picks (absent is 0); `frozen` must be readable too, as #get_balances digs it.
    async fn available(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let mut free = BigDec::zero();
        for row in self.assets().await? {
            if !pick(row["coin"].as_str().unwrap_or_default()) { continue; }
            free = required(super::dig_or_raise(&row, "available")?)?;
            required(super::dig_or_raise(&row, "frozen")?)?;
        }
        Ok(free)
    }

    /// Clients::Bitget#get_order as Exchanges::Bitget#get_order reads it: the order, or None for one Bitget no longer has
    /// (#order_gone?). An error envelope is honeymaker's api_error text; a list's first order is the order.
    async fn order_info(&self, query: Vec<(&'static str, String)>) -> Result<Option<Value>, VenueError> {
        let body = match self.get(ORDER_INFO_PATH, query).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m)) if gone(&m) => return Ok(None),
            other => other?,
        };
        if !ok(&body) {
            let message = vec![api_error(&body)];
            return if gone(&message) { Ok(None) } else { Err(VenueError::Rejected(message)) };
        }
        let raw = match &body["data"] { Value::Array(list) => list.first().cloned().unwrap_or(Value::Null), other => other.clone() };
        match raw { Value::Null | Value::Bool(false) => Ok(None), Value::Object(_) => Ok(Some(raw)), _ => Err(unreadable()) }
    }

    async fn get_order(&self, order_id: &str) -> Result<Option<Value>, VenueError> {
        self.order_info(vec![("orderId", split_order_id(order_id).1)]).await
    }
}

impl<T: Transport> super::Venue for BitgetVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    /// #get_ask_price / #get_bid_price through #get_bid_ask_price, and #get_last_price (GET tickers?symbol=<pair>: the first
    /// row's `lastPr`; no row is "Failed to get Bitget last price for <pair>"). A zero price raises "Wrong … price".
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let name = &ticker.ticker;
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let (bid, ask) = self.book(name).await?;
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => {
                let body = self.get(TICKERS_PATH, vec![("symbol", name.clone())]).await?;
                let first = match &body["data"] {
                    Value::Array(rows) if !rows.is_empty() => &rows[0],
                    Value::Null | Value::Array(_) => return Err(VenueError::Rejected(vec![format!("Failed to get Bitget last price for {name}")])),
                    _ => return Err(unreadable()),
                };
                (required(super::dig_or_raise(first, "lastPr")?)?, "last")
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {name}: {}", price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Bitget#place_order: POST place-order with symbol, side, orderType,
    /// size, (a limit's) price, force (`ioc` market, `gtc` limit), then clientOid (the intent id; Rails sends none). A market
    /// order's size is in the unit its side takes (G1) and a limit order's in base (#532), as `WireFormat::Bitget` converted
    /// it. Sent at most once. The stored id is "<symbol>-<orderId>". An error envelope is honeymaker's api_error text; a 00000 answer without an orderId, or an answer
    /// without a code, may have placed the order: Ambiguous.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        // `WireFormat::Bitget` sizes the order in its unit (a market buy in quote, everything else in base) before the intent.
        let in_unit = o.quote_volume == (o.side == Side::Buy && o.kind == OrderKind::Market);
        if !in_unit { return Err(VenueError::Transient(format!("a Bitget {} order sized in the other unit; not sent", o.side.as_str()))); }
        let (order_type, force, price) = match &o.kind {
            OrderKind::Market => ("market", "ioc", None),
            OrderKind::Limit { price } => {
                if !BigDec::parse(price).is_ok_and(|p| p.is_positive()) { return Err(VenueError::Rejected(vec![format!("Invalid limit price for {}", o.pair)])); }
                ("limit", "gtc", Some(price.clone()))
            }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("Bitget")),
        };
        let mut body = serde_json::Map::new();
        for (k, v) in [("symbol", o.pair.as_str()), ("side", o.side.as_str()), ("orderType", order_type)] { body.insert(k.into(), v.into()); }
        body.insert("size".into(), o.volume.clone().into());
        if let Some(p) = price { body.insert("price".into(), p.into()); }
        body.insert("force".into(), force.into());
        body.insert("clientOid".into(), o.cl_ord_id.clone().into());
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.request("POST", PLACE_PATH, vec![], Some(Value::Object(body))) };
        let answer = self.call(&r, true).await?;
        // Only an answer that names an error code rejects the order; one without a code ({}, null) says nothing about
        // whether it landed, so the intent stays for recovery by clientOid.
        if answer.get("code").is_none_or(|c| to_s(c).is_empty()) { return Err(VenueError::Ambiguous(NO_CODE.into())); }
        if !ok(&answer) { return Err(VenueError::Rejected(vec![api_error(&answer)])); }
        let id = to_s(&answer["data"]["orderId"]);
        if id.is_empty() { return Err(VenueError::Ambiguous(NO_ORDER_ID.into())); }
        Ok(format!("{}-{id}", o.pair))
    }

    /// #get_orders: #get_order per id; an order Bitget no longer has (G5) is left out (`missing`, for
    /// Bot::StaleOrderResolver); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids {
            if let Some(raw) = self.get_order(id).await? { out.push(parse_order(id, &raw)?); }
        }
        Ok(out)
    }

    /// #get_order (Bot::FetchAndUpdateOrderJob): a gone order fails with Rails' "Bitget no longer knows order <id>".
    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.get_order(txid).await? {
            Some(raw) => Ok((vec![parse_order(txid, &raw)?], vec![])),
            None => Err(VenueError::Rejected(vec![format!("Bitget no longer knows order {txid}")])),
        }
    }

    /// Recovery may only FIND the order (plan VB §1.5: `absence_provable` false): orderInfo by `clientOid`, an order naming
    /// this client order id and symbol. Anything else proves nothing: the intent waits, for a later lookup or
    /// `deltabadger resolve-placement`. Rails has no lookup (it sends no clientOid).
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let Some(o) = self.order_info(vec![("clientOid", cl_ord_id.to_string())]).await? else { return Ok(None) };
        if o["clientOid"].as_str() != Some(cl_ord_id) || o["symbol"].as_str() != Some(symbol) { return Ok(None); }
        let id = to_s(&o["orderId"]);
        if id.is_empty() { return Err(unreadable()); }
        parse_order(&format!("{symbol}-{id}"), &o).map(Some)
    }

    /// Exchanges::Bitget reads no fills for a gone order: it stays missing, for Bot::StaleOrderResolver.
    async fn fills_from_trades(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }

    /// #get_balances' `available` for the quote asset: each row's coin mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.available(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.available(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.available(|code| code == ticker.quote_code).await }

    /// Exchanges::Bitget#get_tickers_prices (`ticker_prices`). Rails caches the answer for a minute; this reads fresh. A 2xx
    /// body that is not JSON is honeymaker's Failure (its text). Each price is its decimal's `to_s('F')`.
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let text = self.answer(&self.request("GET", TICKERS_PATH, vec![], None), false, TICKERS_LIMIT).await?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        Ok(ticker_prices(&body)?.into_iter().map(|(symbol, price)| (symbol, Value::String(price.to_s_f()))).collect())
    }

    /// #cancel_order through Clients::Bitget#cancel_order: POST cancel-order with the symbol and orderId (a legacy id without
    /// its symbol is looked up first); any 2xx is the cancel, as Rails takes it.
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let (symbol, ext) = split_order_id(txid);
        let symbol = match symbol {
            Some(s) => s,
            None => {
                let raw = self.get_order(txid).await?.ok_or_else(|| VenueError::Rejected(vec!["Order not found".into()]))?;
                to_s(&raw["symbol"])
            }
        };
        let body = serde_json::json!({ "symbol": symbol, "orderId": ext });
        self.answer(&self.request("POST", CANCEL_PATH, vec![], Some(body)), false, http::MAX_BODY).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_order_id_splits_at_its_first_dash_and_a_gone_order_is_told_by_its_code() {
        assert_eq!(split_order_id("BTCUSDT-1234"), (Some("BTCUSDT".to_string()), "1234".to_string()));
        assert_eq!(split_order_id("1234"), (None, "1234".to_string()));
        assert!(gone(&[r#"{"code":"40109","msg":"The data of the order cannot be found"}"#.into()]));
        assert!(gone(&["Bitget API error 43001: 订单不存在".into()]));
        assert!(gone(&["Order not found".into()]));
        assert!(!gone(&[r#"{"code":43001,"msg":"x"}"#.into()]), "an Integer code never equals Rails' String");
        assert!(!gone(&["Bitget API error 43012: Insufficient balance".into()]));
        assert_eq!(api_error(&serde_json::json!({ "code": "43012", "msg": "Insufficient balance" })), "Bitget API error 43012: Insufficient balance");
    }
}
