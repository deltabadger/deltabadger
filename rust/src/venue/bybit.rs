//! Bybit V5 spot, ported from Exchanges::Bybit and honeymaker. D-BYBIT-AMBIG keeps an intent on server errors.
use chrono::{DateTime, Duration, Utc};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::collections::HashMap;
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, BYBIT as RULES};
use crate::ruby::BigDec;
use super::binance::{build_query, Pairs};
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, Side, VenueError};
pub const BYBIT: &str = "Exchanges::Bybit";
pub const URL: &str = "https://api.bybit.com";
pub const ASSETS_PATH: &str = "/v5/account/wallet-balance";
pub const PLACE_PATH: &str = "/v5/order/create";
pub const CANCEL_PATH: &str = "/v5/order/cancel";
pub const REALTIME_PATH: &str = "/v5/order/realtime";
pub const HISTORY_PATH: &str = "/v5/order/history";
pub const TICKERS_PATH: &str = "/v5/market/tickers";
pub const ORDERBOOK_PATH: &str = "/v5/market/orderbook";
pub const NO_CODE: &str = "Bybit answered the order with no result code";
pub const NO_ORDER_ID: &str = "Bybit answered the order with no order id";
pub fn headers(r: &HttpRequest, key: &str, secret: &str, timestamp_ms: i64) -> Vec<(String, String)> {
    if r.path.starts_with("/v5/market/") || key.is_empty() || secret.is_empty() { return super::sign::json_headers(); }
    super::sign::prehash(&super::sign::BYBIT, r, &build_query(&r.query), key, secret, "", timestamp_ms)
}
pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous("Bybit: unreadable response".into()) }
pub(crate) fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
fn order_id(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !crate::ruby::blank(s) => Some(s.clone()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        _ => None,
    }
}
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }
fn decimal(v: &Value) -> Result<BigDec, VenueError> { if v.is_null() || v.as_str() == Some("") { Ok(BigDec::zero()) } else { required(v) } }
pub(crate) fn failure_text(text: &str) -> String {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("retMsg").or_else(|| v.get("msg")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)).unwrap_or_else(|| text.into())
}
pub fn free_balance(coin: &Value) -> Result<BigDec, VenueError> {
    if coin.get("walletBalance").is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|s| !crate::ruby::blank(s))) {
        return Ok((&(&decimal(&coin["walletBalance"])? - &decimal(&coin["locked"])?) - &decimal(&coin["spotBorrow"])?).max(BigDec::zero()));
    }
    let old = coin.get("availableToWithdraw").filter(|v| !v.is_null() && v.as_str().is_none_or(|s| !crate::ruby::blank(s))).unwrap_or(&coin["free"]);
    decimal(old)
}
pub fn parse_order(id: &str, raw: &Value) -> Result<OrderState, VenueError> {
    let dig = |k| super::dig_or_raise(raw, k);
    let limit = match dig("orderType")?.as_str() { Some("Limit") => true, Some("Market") => false, _ => return Err(VenueError::Rejected(vec![format!("Unknown Bybit order type: {}", to_s(&raw["orderType"]))])) };
    let mut price = decimal(dig("avgPrice")?)?;
    if price.is_zero() { price = decimal(dig("price")?)?; }
    let amount = decimal(dig("qty")?)?;
    let status = match dig("orderStatus")?.as_str() {
        Some("Created" | "Untriggered") => OrderStatus::Unknown,
        Some("New" | "PartiallyFilled") => OrderStatus::Open,
        Some("Filled") => OrderStatus::Closed,
        Some("Cancelled" | "PartiallyFilledCanceled" | "Expired" | "Deactivated") => OrderStatus::Cancelled,
        Some("Rejected") => OrderStatus::Failed,
        _ => return Err(VenueError::Rejected(vec![format!("Unknown Bybit order status: {}", to_s(&raw["orderStatus"]))])),
    };
    Ok(OrderState { txid: id.into(), status, price: (!price.is_zero()).then_some(price), amount: (!amount.is_zero()).then_some(amount), quote_amount: None,
        amount_exec: decimal(dig("cumExecQty")?)?, quote_amount_exec: Some(decimal(dig("cumExecValue")?)?), limit,
        sell: dig("side")?.as_str().ok_or_else(unreadable)?.eq_ignore_ascii_case("sell"), pair: Some(to_s(dig("symbol")?)), asset_class: None })
}
#[derive(Clone)]
pub struct BybitVenue<T: Transport> {
    transport: T, base: String, pairs: Pairs, identity: Option<Value>, placed: HashMap<String, DateTime<Utc>>, now: DateTime<Utc>,
    history_db: Option<std::path::PathBuf>,
}
impl<T: Transport> BybitVenue<T> {
    pub fn new(transport: T, base: &str) -> Self { Self { transport, base: base.into(), pairs: Pairs::default(), identity: None, placed: HashMap::new(), now: Utc::now(), history_db: None } }
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    pub fn with_history(mut self, placed: HashMap<String, DateTime<Utc>>, now: DateTime<Utc>) -> Self { self.placed = placed; self.now = now; self }
    /// Keep only the install's path: the history fallback opens it for one order after realtime misses.
    pub fn with_history_db(mut self, path: std::path::PathBuf, now: DateTime<Utc>) -> Self { self.history_db = Some(path); self.now = now; self }
    async fn placement_time(&self, id: &str) -> Result<Option<DateTime<Utc>>, VenueError> {
        let Some(path) = self.history_db.clone() else { return Ok(self.placed.get(id).copied()) };
        let id = id.to_owned();
        let unreadable = || VenueError::Transient("Bybit order placement time is unreadable".into());
        // Like the factory's settings reads, use a fresh connection to this install, without creating a missing DB.
        // SQLite's open, query and timestamp parsing all stay off the async thread; no connection survives the read.
        tokio::task::spawn_blocking(move || {
            let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| unreadable())?;
            let at: Option<String> = c.query_row(
                "SELECT created_at FROM transactions WHERE exchange_id = (SELECT id FROM exchanges WHERE type = ?1) AND external_id = ?2",
                rusqlite::params![BYBIT, id], |r| r.get(0)).optional().map_err(|_| unreadable())?;
            at.map(|at| crate::codec::parse_time(&at).map_err(|_| unreadable())).transpose()
        }).await.map_err(|_| unreadable())?
    }
    pub fn request(&self, method: &'static str, path: &str, mut query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        query.sort_by(|a,b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path: path.into(), query, body, not_after: None }
    }
    pub async fn read_text(&self, path: &str, query: Vec<(&'static str, String)>, limit: usize) -> Result<String, String> {
        let r = self.request("GET", path, query, None);
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, r.url())),
        }
    }
    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        let resp = self.transport.send_limited(r, http::MAX_BODY).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))?;
        let body = http::decode_json(&resp.body).ok();
        let code = body.as_ref().and_then(|v| v.get("retCode")).filter(|v| v.as_i64().is_some() || v.as_str().is_some_and(|s| s.parse::<i64>().is_ok())).map(to_s).unwrap_or_default();
        let message = failure_text(&resp.body);
        if placement && (code == "10000" || code == "10016") { return Err(VenueError::Ambiguous(message)); }
        if !(200..300).contains(&resp.status) {
            let message = if message.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { message };
            if !placement && resp.status == 401 { return Err(VenueError::Unauthorized(vec![message])); }
            // A code-less response, an intermediary failure, or a server error cannot prove rejection.
            let rejected = (400..500).contains(&resp.status) && !code.is_empty() && code != "0";
            return Err(if placement && !rejected { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) });
        }
        let body = body.ok_or_else(unreadable)?;
        if code.is_empty() { return Err(if placement { VenueError::Ambiguous(NO_CODE.into()) } else { unreadable() }); }
        if code != "0" {
            let message = to_s(&body["retMsg"]);
            // Rails find_order retries history for Honeymaker's exact missing-order error, too.
            // HTTP failures returned above remain failed reads, even when their JSON contains this text.
            if r.path == REALTIME_PATH && message == "Order not found" { return Ok(Value::Null); }
            return Err(VenueError::Rejected(vec![message]));
        }
        Ok(body)
    }
    async fn get(&self, path: &str, query: Vec<(&'static str, String)>) -> Result<Value, VenueError> { self.call(&self.request("GET", path, query, None), false).await }
    async fn lookup(&self, query: Vec<(&'static str, String)>, placed: Option<DateTime<Utc>>, id: &str) -> Result<Option<Value>, VenueError> {
        let first = |body: Value| -> Result<Option<Value>, VenueError> {
            match &body["result"]["list"] { Value::Null => Ok(None), Value::Array(a) => Ok(a.first().cloned()), _ => Err(unreadable()) }
        };
        if let Some(o) = first(self.get(REALTIME_PATH, query.clone()).await?)? { return Ok(Some(o)); }
        let placed = match placed { Some(at) => Some(at), None => self.placement_time(id).await? };
        let Some(placed) = placed else { return Err(VenueError::Rejected(vec![format!("Bybit order {id} is not in realtime and its placement time is unknown")])); };
        let mut start = placed - Duration::hours(1);
        let until = (start + Duration::days(28)).min(self.now);
        while start < until {
            let end = (start + Duration::days(7)).min(until);
            let mut q = query.clone();
            q.extend([("startTime", super::bitvavo::ruby_ms(start).to_string()), ("endTime", super::bitvavo::ruby_ms(end).to_string())]);
            if let Some(o) = first(self.get(HISTORY_PATH, q).await?)? { return Ok(Some(o)); }
            start = end;
        }
        Ok(None)
    }
    async fn find(&self, id: &str) -> Result<Option<Value>, VenueError> {
        self.lookup(vec![("category", "spot".into()), ("orderId", id.into())], None, id).await
    }
    async fn available(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let b = self.get(ASSETS_PATH, vec![("accountType", "UNIFIED".into())]).await?;
        let mut free = BigDec::zero();
        for account in b["result"]["list"].as_array().ok_or_else(unreadable)? {
            for coin in account["coin"].as_array().ok_or_else(unreadable)? {
                if coin["coin"].as_str().is_some_and(&pick) { free = free_balance(coin)?; }
            }
        }
        Ok(free)
    }
}
impl<T: Transport> super::Venue for BybitVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }
    async fn price(&self, t: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let mut q = vec![("category", "spot".into()), ("symbol", t.ticker.clone())];
        let (p, label) = if side == PriceSide::Last {
            let b = self.get(TICKERS_PATH, q).await?;
            (decimal(super::dig_or_raise(&b["result"]["list"][0], "lastPrice")?)?, "last")
        } else {
            q.push(("limit", "1".into()));
            let b = self.get(ORDERBOOK_PATH, q).await?;
            let result = super::dig_or_raise(&b, "result")?;
            let bid = super::dig_or_raise(result, "b")?.get(0).and_then(Value::as_array).ok_or_else(unreadable)?;
            let ask = super::dig_or_raise(result, "a")?.get(0).and_then(Value::as_array).ok_or_else(unreadable)?;
            for level in [bid, ask] { decimal(level.get(1).ok_or_else(unreadable)?)?; }
            if side == PriceSide::Bid { (decimal(bid.first().ok_or_else(unreadable)?)?, "bid") } else { (decimal(ask.first().ok_or_else(unreadable)?)?, "ask") }
        };
        if p.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", t.ticker, p.to_s_f())])); }
        Ok(p)
    }
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        if o.cl_ord_id.len() > 36 { return Err(VenueError::Rejected(vec!["Bybit orderLinkId exceeds 36 characters".into()])); }
        let mut body = serde_json::Map::new();
        for (k,v) in [("category", "spot"), ("symbol", o.pair.as_str()), ("side", if o.side == Side::Buy { "Buy" } else { "Sell" }),
                      ("orderType", if matches!(o.kind, OrderKind::Market) { "Market" } else { "Limit" }), ("qty", o.volume.as_str())] { body.insert(k.into(), v.into()); }
        match &o.kind {
            OrderKind::Market => { body.insert("marketUnit".into(), if o.quote_volume { "quoteCoin" } else { "baseCoin" }.into()); }
            OrderKind::Limit { price } => {
                if o.quote_volume { return Err(VenueError::Transient("Bybit limit quantity must be in base; not sent".into())); }
                if !BigDec::parse(price).is_ok_and(|p| p.is_positive()) { return Err(VenueError::Rejected(vec![format!("Invalid limit price for {}", o.pair)])); }
                body.insert("price".into(), price.clone().into()); body.insert("timeInForce".into(), "GTC".into());
            }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("Bybit")),
        }
        body.insert("orderLinkId".into(), o.cl_ord_id.clone().into());
        let r = HttpRequest { not_after: Some(o.deadline + Duration::from_std(http::TOTAL_TIMEOUT).expect("45 seconds")), ..self.request("POST", PLACE_PATH, vec![], Some(body.into())) };
        let b = self.call(&r, true).await?;
        order_id(&b["result"]["orderId"]).ok_or_else(|| VenueError::Ambiguous(NO_ORDER_ID.into()))
    }
    async fn orders(&self, ids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in ids { if let Some(raw) = self.find(id).await? { out.push(parse_order(id, &raw)?); } }
        Ok(out)
    }
    async fn order_identified<F>(&self, id: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.find(id).await? { Some(raw) => Ok((vec![parse_order(id, &raw)?], vec![])), None => Err(VenueError::Rejected(vec![format!("Bybit no longer knows order {id}")])) }
    }
    async fn order_by_client_id(&self, id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let Some(raw) = self.lookup(vec![("category", "spot".into()), ("symbol", symbol.into()), ("orderLinkId", id.into())], Some(since), id).await? else { return Ok(None) };
        if raw["orderLinkId"].as_str() != Some(id) || raw["symbol"].as_str() != Some(symbol) { return Ok(None); }
        let ext = order_id(&raw["orderId"]).ok_or_else(unreadable)?;
        parse_order(&ext, &raw).map(Some)
    }
    async fn fills_from_trades(&self, _ids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }
    async fn balance(&self, asset: &str, _all_crypto: bool) -> Result<BigDec, VenueError> { self.available(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset)).await }
    async fn free_base_balance(&self, t: &Ticker) -> Result<BigDec, VenueError> { self.available(|code| code == t.base_code).await }
    async fn settled_quote_balance<F>(&self, t: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.available(|code| code == t.quote_code).await }
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let b = self.get(TICKERS_PATH, vec![("category", "spot".into())]).await?;
        let mut out = vec![];
        for row in b["result"]["list"].as_array().ok_or_else(unreadable)? {
            let symbol = to_s(super::dig_or_raise(row, "symbol")?);
            let p = decimal(super::dig_or_raise(row, "lastPrice")?)?;
            out.retain(|(s,_)| s != &symbol); out.push((symbol, Value::String(p.to_s_f())));
        }
        Ok(out)
    }
    async fn cancel(&self, id: &str) -> Result<(), VenueError> {
        let raw = self.find(id).await?.ok_or_else(|| VenueError::Rejected(vec![format!("Bybit no longer knows order {id}")]))?;
        let symbol = to_s(&raw["symbol"]);
        if crate::ruby::blank(&symbol) { return Err(VenueError::Rejected(vec!["Failed to find Bybit order for cancellation".into()])); }
        self.call(&self.request("POST", CANCEL_PATH, vec![], Some(json!({"category":"spot","symbol":symbol,"orderId":id}))), false).await.map(|_| ())
    }
}
