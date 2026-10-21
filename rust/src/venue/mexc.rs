//! Honeymaker::Clients::Mexc and Exchanges::Mexc for the engine's spot orders (plan VB slice A5): prices, balances,
//! placement, per-order polls, cancel and the recovery by client order id. Rails is the oracle (post-audit, AUDIT-mexc
//! X1-X12); each method names the Ruby it ports. `venue::live` builds it for every MEXC bot.
//!
//! MEXC is a Binance API clone: the same signing (`sign::QueryHmacHex`, the key in X-MEXC-APIKEY), every param in the
//! query on every method, Binance-shaped orders (`binance::parse_binance_shaped`, MEXC's status map and price rule) and
//! the stored id "SYMBOL-<orderId>". It differs in what it reads: one GET per order (`venue::poll_each`), an order MEXC
//! no longer knows (-2013/-2011) listed missing; no fill rebuild from trades; the gem's default recvWindow everywhere.
//!
//! Where it differs on purpose (plan VB §5, named in tests/parity.rs): the order carries `newClientOrderId` = the intent id
//! (D9; Rails sends none) and recovery asks for it (`order_by_client_id`), which can only FIND an order (absence is never
//! concluded, ABSENCE_UNPROVEN); a placement that may have reached MEXC (a lost reply, a 5xx, an unreadable answer, one
//! without an orderId, "service not available") keeps the intent where Rails writes a failed (or a "BTCUSDT-") row; a
//! number that is no decimal is refused where Rails reads it as 0.
use super::binance::{error_message, Pairs, Shape};
use super::http::{self, HttpRequest, HttpResponse, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, MEXC as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const URL: &str = "https://api.mexc.com";
pub const MEXC: &str = "Exchanges::Mexc";
pub const UNREADABLE: &str = "MEXC: unreadable response";
/// Honeymaker::Clients::Mexc's default recvWindow, which every Exchanges::Mexc call keeps.
pub const RECV_WINDOW: &str = "5000";
/// Exchanges::Mexc::ORDER_GONE_CODES: -2013 "Order does not exist.", -2011 "Unknown order sent".
pub const ORDER_GONE_CODES: [i64; 2] = [-2013, -2011];
pub const ORDER_PATH: &str = "/api/v3/order";
pub const ACCOUNT_PATH: &str = "/api/v3/account";
/// Plan VB §1.5: on MEXC absence is never concluded. The lookup answers -2013 for an order it does not hold, which proves
/// nothing about one sent moments ago; the intent waits for `deltabadger resolve-placement`. A divergence: Rails sends no
/// newClientOrderId and has no lookup at all.
pub const ABSENCE_UNPROVEN: &str = "MEXC: the client order lookup did not find this order, and MEXC cannot prove it absent; settle it with `deltabadger resolve-placement`";

/// Exchanges::Mexc::ORDER_STATUS_MAP (X1: PARTIALLY_CANCELED is terminal); any other status raises, as Rails'
/// #parse_order_status does. The fills' average whenever something filled (X2); origQuoteOrderQty may be absent.
pub const SHAPE: Shape = Shape {
    statuses: &[("NEW", OrderStatus::Open), ("PARTIALLY_FILLED", OrderStatus::Open), ("FILLED", OrderStatus::Closed), ("CANCELED", OrderStatus::Cancelled),
                ("PARTIALLY_CANCELED", OrderStatus::Cancelled), ("EXPIRED", OrderStatus::Cancelled), ("REJECTED", OrderStatus::Failed)],
    unlisted: None, average_whenever_filled: true, quote_optional: true,
};

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }

/// Exchange#response_field(result, 'code') of a failure's text, as a number (Ruby's `include?` compares 2013.0 with 2013 as
/// equal; a String never equals an Integer).
pub fn error_code(text: &str) -> Option<f64> { serde_json::from_str::<Value>(text).ok()?.get("code")?.as_f64() }

/// The failure's body names an order MEXC no longer knows (Exchanges::Mexc#get_order's ORDER_GONE_CODES).
fn gone(body: &str) -> bool { error_code(body).is_some_and(|c| ORDER_GONE_CODES.iter().any(|g| c == *g as f64)) }

/// Exchanges::Mexc#parse_order_data (`binance::parse_binance_shaped` with MEXC's shape).
pub fn parse_order(order_id: &str, o: &Value, pairs: &Pairs) -> Result<OrderState, VenueError> {
    super::binance::parse_binance_shaped(RULES.name, order_id, o, pairs, &SHAPE).map(|o| o.state)
}

/// `order_id.split('-')` into the symbol and MEXC's own id ("BTCUSDT-" has none: honeymaker then sends no orderId).
fn split(order_id: &str) -> (String, Option<String>) {
    let mut parts = order_id.split('-');
    (parts.next().unwrap_or_default().to_string(), parts.next().filter(|s| !s.is_empty()).map(str::to_string))
}

#[derive(Clone)]
pub struct MexcVenue<T: Transport> { transport: T, base: String, secret: String, pairs: Pairs, identity: Option<Value> }

impl<T: Transport> MexcVenue<T> {
    pub fn new(transport: T, base: &str, secret: &str) -> Self { Self { transport, base: base.into(), secret: secret.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// The exchange's tickers (`Pairs::load`), which the trading methods read.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    /// One signed request as honeymaker builds it (`sign::MEXC_QUERY`), every pair in the query, sorted by key.
    pub fn signed(&self, method: &'static str, path: &str, params: Vec<(&'static str, String)>, now: DateTime<Utc>) -> HttpRequest {
        self.request(method, path, super::sign::MEXC_QUERY.sign(&self.secret, params, now))
    }

    /// An unsigned request (honeymaker's #get_public): the params sorted, no signature.
    pub fn request(&self, method: &'static str, path: &str, mut params: Vec<(&'static str, String)>) -> HttpRequest {
        params.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path: path.into(), query: params, body: None, not_after: None }
    }

    fn signed_now(&self, method: &'static str, path: &str, mut params: Vec<(&'static str, String)>) -> HttpRequest {
        params.push(("recvWindow", RECV_WINDOW.into()));
        self.signed(method, path, params, self.transport.now())
    }

    /// One send as honeymaker's #with_rescue reads it: the 2xx body of at most `limit` bytes (`Ok`), else the failure's
    /// text: the answer's body, or Faraday's (or the transport's) message. The ledger's and the balance sync's reads.
    pub async fn read_text(&self, r: &HttpRequest, limit: usize) -> Result<String, String> {
        match self.transport.send_limited(r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for {} {}", resp.status, r.method, r.url())),
        }
    }

    /// One send, its transport failure classified (`http::transport_failure`).
    async fn send(&self, r: &HttpRequest, placement: bool) -> Result<HttpResponse, VenueError> {
        self.transport.send(r).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))
    }

    /// An answer as Exchanges::Mexc reads it (Exchange#ambiguous_placement_error? for a placement): a non-2xx is
    /// #parse_error_message's text (the body's `msg`, else the body, else Faraday's message); a 401 on a read carries its
    /// status; on a placement only a 4xx, or a -1021 rejection whatever its status, is definitive.
    fn classify(&self, r: &HttpRequest, resp: HttpResponse, placement: bool) -> Result<String, VenueError> {
        if (200..300).contains(&resp.status) { return Ok(resp.body); }
        let message = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { error_message(&resp.body) };
        if !placement && resp.status == 401 { return Err(VenueError::Unauthorized(vec![message])); }
        let definitive = (400..500).contains(&resp.status) || crate::engine::placement::safe_rejection(std::slice::from_ref(&message));
        Err(if placement && !definitive { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) })
    }

    /// `classify`d and decoded: a 2xx that is not JSON is unreadable.
    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        let resp = self.send(r, placement).await?;
        http::decode_json(&self.classify(r, resp, placement)?).map_err(|_| unreadable())
    }

    /// One order read whose answer may name an order MEXC no longer knows: `None` then.
    async fn lookup(&self, r: &HttpRequest) -> Result<Option<Value>, VenueError> {
        let resp = self.send(r, false).await?;
        if !(200..300).contains(&resp.status) && gone(&resp.body) { return Ok(None); }
        http::decode_json(&self.classify(r, resp, false)?).map(Some).map_err(|_| unreadable())
    }

    /// Exchanges::Mexc#get_order: GET /api/v3/order by symbol and orderId; None for an order MEXC no longer knows.
    async fn get_order(&self, order_id: &str) -> Result<Option<OrderState>, VenueError> {
        let (symbol, ext) = split(order_id);
        let mut params = vec![("symbol", symbol)];
        params.extend(ext.map(|e| ("orderId", e)));
        match self.lookup(&self.signed_now("GET", ORDER_PATH, params)).await? {
            Some(body) => parse_order(order_id, &body, &self.pairs).map(Some),
            None => Ok(None),
        }
    }

    /// GET /api/v3/account's `balances`, each row's free (and its locked, which must be readable too) for the codes `pick`
    /// accepts; the last such row wins, absent is 0.
    async fn free(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let body = self.call(&self.signed_now("GET", ACCOUNT_PATH, vec![]), false).await?;
        let mut free = BigDec::zero();
        for row in super::dig_or_raise(&body, "balances")?.as_array().ok_or_else(unreadable)? {
            if !row["asset"].as_str().is_some_and(&pick) { continue; }
            free = required(super::dig_or_raise(row, "free")?)?;
            required(super::dig_or_raise(row, "locked")?)?;
        }
        Ok(free)
    }
}

impl<T: Transport> super::Venue for MexcVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    /// #get_ask_price / #get_bid_price through #get_bid_ask_price (bookTicker: bidPrice, bidQty, askPrice and askQty, each
    /// `dig_or_raise`d), and #get_last_price (ticker/price). A zero price raises "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let symbol = vec![("symbol", ticker.ticker.clone())];
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.call(&self.request("GET", "/api/v3/ticker/bookTicker", symbol), false).await?;
                let level = |k: &str| super::dig_or_raise(&body, k).and_then(required);
                let bid = level("bidPrice")?;
                level("bidQty")?;
                let ask = level("askPrice")?;
                level("askQty")?;
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => {
                let body = self.call(&self.request("GET", "/api/v3/ticker/price", symbol), false).await?;
                (required(super::dig_or_raise(&body, "price")?)?, "last")
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.ticker, price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Mexc#new_order: POST /api/v3/order, every field in the query. A
    /// market order is `quoteOrderQty` or `quantity`; a limit order GTC `price` and `quantity`. Plus Rust's
    /// `newClientOrderId` (the intent id; Rails sends none) and the gem's recvWindow. Sent at most once. The stored id is
    /// "SYMBOL-<orderId>"; an answer without an orderId (Rails would store "SYMBOL-") may have placed the order: Ambiguous.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut params = vec![("symbol", o.pair.clone()), ("side", o.side.as_str().to_ascii_uppercase())];
        let kind = match &o.kind {
            OrderKind::Market => {
                params.extend([("type", "MARKET".into()), (if o.quote_volume { "quoteOrderQty" } else { "quantity" }, o.volume.clone())]);
                "market"
            }
            OrderKind::Limit { price } => {
                params.extend([("type", "LIMIT".into()), ("timeInForce", "GTC".into()), ("price", price.clone()), ("quantity", o.volume.clone())]);
                "limit"
            }
        };
        params.push(("newClientOrderId", o.cl_ord_id.clone()));
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")), ..self.signed_now("POST", ORDER_PATH, params) };
        let body = self.call(&r, true).await?;
        let id = match &body["orderId"] {
            Value::Number(n) if n.is_u64() || n.is_i64() => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err(VenueError::Ambiguous(format!("Failed to set {} {kind} order (order_id is {}-)", RULES.name, o.pair))),
        };
        Ok(format!("{}-{id}", o.pair))
    }

    /// #get_orders (X5): #get_order per id; an order MEXC no longer knows is left out (`missing`, which polling hands to
    /// Bot::StaleOrderResolver); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        Ok(super::poll_each(txids, |id| async move { self.get_order(&id).await }).await?.0)
    }

    /// #get_order (Bot::FetchAndUpdateOrderJob): a gone order is #get_order's failure.
    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.get_order(txid).await? {
            Some(o) => Ok((vec![o], vec![])),
            None => Err(VenueError::Rejected(vec![format!("{} no longer knows order {txid}", RULES.name)])),
        }
    }

    /// Recovery (plan VB §1.5) may only FIND the order: GET /api/v3/order by the intent's symbol and origClientOrderId; a hit
    /// naming them is the order. -2013/-2011 is never absence (ABSENCE_UNPROVEN).
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let r = self.signed_now("GET", ORDER_PATH, vec![("symbol", symbol.to_string()), ("origClientOrderId", cl_ord_id.to_string())]);
        let Some(body) = self.lookup(&r).await? else { return Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())) };
        let ext = match &body["orderId"] {
            Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err(unreadable()),
        };
        if body["symbol"].as_str() != Some(symbol) || body["clientOrderId"].as_str() != Some(cl_ord_id) {
            return Err(VenueError::Rejected(vec![format!("{} answered for client order id {cl_ord_id} on {symbol} with another order", RULES.name)]));
        }
        parse_order(&format!("{symbol}-{ext}"), &body, &self.pairs).map(Some)
    }

    /// Rails reads no trades for an order; a missing one is never looked up in trades.
    async fn fills_from_trades(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }

    /// #get_balances' `free` for the quote asset: each row's venue code mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.free(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.free(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.free(|code| code == ticker.quote_code).await }

    /// Exchanges::Mexc#get_tickers_prices: /api/v3/ticker/price for every symbol, each `dig_or_raise`d `symbol` and `price`
    /// (a zero kept, a later row for a symbol replacing an earlier one). A 2xx body that is not JSON is honeymaker's Failure
    /// (its text). DIVERGES on purpose as Binance's does: a price that is no decimal is left out. Rails caches the answer for
    /// a minute; this reads fresh.
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let r = self.request("GET", "/api/v3/ticker/price", vec![]);
        let resp = self.send(&r, false).await?;
        let text = self.classify(&r, resp, false)?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        let mut out: Vec<(String, Value)> = vec![];
        for row in body.as_array().ok_or_else(unreadable)? {
            let symbol = super::dig_or_raise(row, "symbol")?.as_str().ok_or_else(unreadable)?.to_string();
            let price = match super::dig_or_raise(row, "price")? {
                v @ (Value::String(_) | Value::Number(_)) => crate::ruby::json_to_d(v).ok().flatten(),
                _ => return Err(unreadable()),
            };
            out.retain(|(s, _)| *s != symbol);
            if let Some(p) = price { out.push((symbol, Value::String(p.to_s_f()))); }
        }
        Ok(out)
    }

    /// #cancel_order through Clients::Mexc#cancel_order: DELETE /api/v3/order by the stored "SYMBOL-<orderId>"; a JSON 2xx is
    /// a cancel, as Rails reads it.
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let (symbol, ext) = split(txid);
        let mut params = vec![("symbol", symbol)];
        params.extend(ext.map(|e| ("orderId", e)));
        self.call(&self.signed_now("DELETE", ORDER_PATH, params), false).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_order_id_splits_as_ruby_splits_it() {
        assert_eq!(split("BTCUSDT-C02__443776347957968896"), ("BTCUSDT".to_string(), Some("C02__443776347957968896".to_string())));
        assert_eq!(split("BTCUSDT-"), ("BTCUSDT".to_string(), None));
        assert_eq!(split("A-B-C"), ("A".to_string(), Some("B".to_string())));
        assert!(gone(r#"{"code":-2013,"msg":"Order does not exist."}"#) && gone(r#"{"code":-2011.0}"#));
        assert!(!gone(r#"{"code":"-2013"}"#) && !gone("<html>404</html>"));
    }
}
