//! Bitrue spot orders (A6), ported from Exchanges::Bitrue and honeymaker 0.12.5.
//! Reuses QueryHmacHex, parse_binance_shaped and poll_each. T1/T2/T3/T4/T5/T6/T8 are Rails parity.
//! D9 sends a client id; D-BITRUE-AMBIG retains unproven placements; D-BITRUE-NUM refuses unreadable numbers.
//! Official spot docs name no client-id length limit (checked 2026-10-21); the reversible 32-hex UUID is used
//! consistently for send and lookup. Missing lookup results never prove absence.
use super::binance::{error_message, Pairs, Shape};
use super::http::{self, HttpRequest, HttpResponse, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, BITRUE as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const URL: &str = "https://openapi.bitrue.com";
pub const BITRUE: &str = "Exchanges::Bitrue";
pub const UNREADABLE: &str = "Bitrue: unreadable response";
pub const RECV_WINDOW: &str = "5000";
pub const ORDER_GONE_CODE: i64 = -2013;
pub const ORDER_PATH: &str = "/api/v1/order";
pub const ACCOUNT_PATH: &str = "/api/v1/account";
pub const ABSENCE_UNPROVEN: &str = "Bitrue: the client order lookup did not find this order, and Bitrue cannot prove it absent; settle it with `deltabadger resolve-placement`";

pub const SHAPE: Shape = Shape {
    statuses: &[("NEW", OrderStatus::Open), ("PARTIALLY_FILLED", OrderStatus::Open), ("FILLED", OrderStatus::Closed), ("CANCELED", OrderStatus::Cancelled),
                ("PENDING_CREATE", OrderStatus::Open), ("PENDING_CANCEL", OrderStatus::Unknown), ("EXPIRED", OrderStatus::Cancelled), ("REJECTED", OrderStatus::Failed)],
    unlisted: None, average_whenever_filled: true, quote_optional: true,
};

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }

fn client_order_id(id: &str) -> Result<String, VenueError> { super::client_order_id_32(id, "Bitrue") }

pub fn error_code(text: &str) -> Option<f64> { serde_json::from_str::<Value>(text).ok()?.get("code")?.as_f64() }

fn gone(body: &str) -> bool {
    serde_json::from_str::<Value>(body).ok().is_some_and(|v| match &v["code"] {
        Value::String(s) => crate::ruby::to_i(s) == ORDER_GONE_CODE,
        Value::Number(n) => n.as_f64().is_some_and(|n| n as i64 == ORDER_GONE_CODE),
        _ => false,
    })
}

pub fn parse_order(order_id: &str, o: &Value, pairs: &Pairs) -> Result<OrderState, VenueError> {
    super::binance::parse_binance_shaped(RULES.name, order_id, o, pairs, &SHAPE).map(|o| o.state)
}

fn split(order_id: &str) -> (String, Option<String>) {
    let mut parts = order_id.split('-');
    (parts.next().unwrap_or_default().to_string(), parts.next().filter(|s| !s.is_empty()).map(str::to_string))
}

#[derive(Clone)]
pub struct BitrueVenue<T: Transport> { transport: T, base: String, secret: String, pairs: Pairs, identity: Option<Value> }

impl<T: Transport> BitrueVenue<T> {
    pub fn new(transport: T, base: &str, secret: &str) -> Self { Self { transport, base: base.into(), secret: secret.into(), pairs: Pairs::default(), identity: None } }
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    pub fn signed(&self, method: &'static str, path: &str, params: Vec<(&'static str, String)>, now: DateTime<Utc>) -> HttpRequest {
        self.request(method, path, super::sign::BITRUE_QUERY.sign(&self.secret, params, now))
    }

    pub fn request(&self, method: &'static str, path: &str, mut params: Vec<(&'static str, String)>) -> HttpRequest {
        params.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path: path.into(), query: params, body: None, not_after: None }
    }

    fn signed_now(&self, method: &'static str, path: &str, mut params: Vec<(&'static str, String)>) -> HttpRequest {
        params.push(("recvWindow", RECV_WINDOW.into()));
        self.signed(method, path, params, self.transport.now())
    }

    pub async fn read_text(&self, r: &HttpRequest, limit: usize) -> Result<String, String> {
        match self.transport.send_limited(r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for {} {}", resp.status, r.method, r.url())),
        }
    }

    async fn send(&self, r: &HttpRequest, placement: bool) -> Result<HttpResponse, VenueError> {
        self.transport.send(r).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))
    }

    fn classify(&self, r: &HttpRequest, resp: HttpResponse, placement: bool) -> Result<String, VenueError> {
        if (200..300).contains(&resp.status) { return Ok(resp.body); }
        let message = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { error_message(&resp.body) };
        if !placement && resp.status == 401 { return Err(VenueError::Unauthorized(vec![message])); }
        if !placement && resp.status == 429 { return Err(VenueError::Rejected(vec!["Too Many Requests".into()])); }
        // D-BITRUE-AMBIG: only a documented pre-trade refusal proves non-placement. A 5xx never does.
        let errors = std::slice::from_ref(&message);
        let code = error_code(&resp.body);
        let definitive = (400..500).contains(&resp.status) && resp.status != 429 &&
            (["recvWindow IllegalArgumentException", "timestamp IllegalArgumentException"].iter().any(|p| message.contains(p)) ||
             code.is_some_and(|c| [-1013., -1022., -1102., -1121., -2010., -2017., -2014., -2015.].contains(&c))) &&
            !RULES.add_outcome_unknown(errors);
        Err(if placement && !definitive { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) })
    }

    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        let resp = self.send(r, placement).await?;
        http::decode_json(&self.classify(r, resp, placement)?).map_err(|_| unreadable())
    }

    async fn lookup(&self, r: &HttpRequest) -> Result<Option<Value>, VenueError> {
        let resp = self.send(r, false).await?;
        if gone(&resp.body) { return Ok(None); }
        http::decode_json(&self.classify(r, resp, false)?).map(Some).map_err(|_| unreadable())
    }

    async fn get_order(&self, order_id: &str) -> Result<Option<OrderState>, VenueError> {
        let (symbol, ext) = split(order_id);
        let mut params = vec![("symbol", symbol)];
        params.extend(ext.map(|e| ("orderId", e)));
        match self.lookup(&self.signed_now("GET", ORDER_PATH, params)).await? {
            Some(body) => parse_order(order_id, &body, &self.pairs).map(Some),
            None => Ok(None),
        }
    }

    async fn free(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let body = self.call(&self.signed_now("GET", ACCOUNT_PATH, vec![]), false).await?;
        let mut free = BigDec::zero();
        for row in super::dig_or_raise(&body, "balances")?.as_array().ok_or_else(unreadable)? {
            if !row["asset"].as_str().is_some_and(|code| pick(&code.to_uppercase())) { continue; }
            free = required(super::dig_or_raise(row, "free")?)?;
            required(super::dig_or_raise(row, "locked")?)?;
        }
        Ok(free)
    }
}

impl<T: Transport> super::Venue for BitrueVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let symbol = vec![("symbol", ticker.ticker.clone())];
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.call(&self.request("GET", "/api/v1/ticker/bookTicker", symbol), false).await?;
                let level = |k: &str| super::dig_or_raise(&body, k).and_then(required);
                let bid = level("bidPrice")?;
                level("bidQty")?;
                let ask = level("askPrice")?;
                level("askQty")?;
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => {
                let body = self.call(&self.request("GET", "/api/v1/ticker/price", symbol), false).await?;
                (required(super::dig_or_raise(&body, "price")?)?, "last")
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.ticker, price.to_s_f())])); }
        Ok(price)
    }

    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        if o.quote_volume { return Err(VenueError::Transient("Bitrue: quantity must be sized in base before placement; not sent".into())); }
        let mut params = vec![("symbol", o.pair.clone()), ("side", o.side.as_str().to_ascii_uppercase())];
        let kind = match &o.kind {
            OrderKind::Market => {
                params.extend([("type", "MARKET".into()), ("quantity", o.volume.clone())]);
                "market"
            }
            OrderKind::Limit { price } => {
                params.extend([("type", "LIMIT".into()), ("timeInForce", "GTC".into()), ("price", price.clone()), ("quantity", o.volume.clone())]);
                "limit"
            }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("Bitrue")),
        };
        params.push(("newClientOrderId", client_order_id(&o.cl_ord_id)?));
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")), ..self.signed_now("POST", ORDER_PATH, params) };
        let body = self.call(&r, true).await?;
        let id = match &body["orderId"] {
            Value::Number(n) if n.is_u64() || n.is_i64() => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err(VenueError::Ambiguous(format!("Failed to set {} {kind} order (order_id is {}-)", RULES.name, o.pair))),
        };
        Ok(format!("{}-{id}", o.pair))
    }

    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        Ok(super::poll_each(txids, |id| async move { self.get_order(&id).await }).await?.0)
    }

    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.get_order(txid).await? {
            Some(o) => Ok((vec![o], vec![])),
            None => Err(VenueError::Rejected(vec![format!("{} no longer knows order {txid}", RULES.name)])),
        }
    }

    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let cl_ord_id = client_order_id(cl_ord_id)?;
        let r = self.signed_now("GET", ORDER_PATH, vec![("symbol", symbol.to_string()), ("origClientOrderId", cl_ord_id.to_string())]);
        let Some(body) = self.lookup(&r).await? else { return Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())) };
        let ext = match &body["orderId"] {
            Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err(unreadable()),
        };
        if body["symbol"].as_str() != Some(symbol) || body["clientOrderId"].as_str() != Some(cl_ord_id.as_str()) {
            return Err(VenueError::Rejected(vec![format!("{} answered for client order id {cl_ord_id} on {symbol} with another order", RULES.name)]));
        }
        parse_order(&format!("{symbol}-{ext}"), &body, &self.pairs).map(Some)
    }

    async fn fills_from_trades(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }

    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.free(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.free(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.free(|code| code == ticker.quote_code).await }

    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let r = self.request("GET", "/api/v1/ticker/24hr", vec![]);
        let resp = self.send(&r, false).await?;
        let text = self.classify(&r, resp, false)?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        let mut out: Vec<(String, Value)> = vec![];
        for row in body.as_array().ok_or_else(unreadable)? {
            let symbol = super::dig_or_raise(row, "symbol")?.as_str().ok_or_else(unreadable)?.to_string();
            let price = match &row["lastPrice"] {
                v @ (Value::String(_) | Value::Number(_)) => crate::ruby::json_to_d(v).ok().flatten(),
                Value::Null => None,
                _ => return Err(unreadable()),
            };
            if let Some(p) = price.filter(|p| p.is_positive()) {
                match out.iter_mut().find(|(s, _)| *s == symbol) { Some((_, old)) => *old = Value::String(p.to_s_f()), None => out.push((symbol, Value::String(p.to_s_f()))) }
            }
        }
        Ok(out)
    }

    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let (symbol, ext) = split(txid);
        let mut params = vec![("symbol", symbol)];
        params.extend(ext.map(|e| ("orderId", e)));
        self.call(&self.signed_now("DELETE", ORDER_PATH, params), false).await.map(|_| ())
    }
}

