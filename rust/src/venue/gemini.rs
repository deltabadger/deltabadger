//! Honeymaker::Clients::Gemini and Exchanges::Gemini for the engine's spot orders (plan VB slice B3): prices, balances,
//! placement, polls, cancel and recovery by client order id. Rails is the oracle; each method names the Ruby it ports.
//!
//! Signing (Honeymaker::Clients::Gemini#post_signed, `signers::gemini`): every private call is an empty-body POST whose
//! parameters travel base64'd in X-GEMINI-PAYLOAD with `request` and a millisecond `nonce`, HMAC-SHA384'd. The transport
//! (`sign::Wire::Gemini`) signs at send time, with SPEC-E §2.9's process-wide nonce `max(now_ms, last + 1)` (honeymaker
//! sends the plain clock: every nonce Rust sends is one Rails could have sent, and two calls in one ms never collide). The
//! request's params are its JSON `body`, in the caller's order; nothing is sent as a body.
//!
//! Gemini has no market order. A market plan reaches the adapter crossed (`OrderKind::Crossed`, D-679, plan VB §1.5): an
//! `exchange limit` with `immediate-or-cancel` at the price the engine persisted in the intent, its volume as sized. There
//! is no second price read here, and nothing is resized (Rails' #set_market_order re-reads #market_price_for).
use super::binance::Pairs;
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, GEMINI as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

pub const URL: &str = "https://api.gemini.com";
pub const GEMINI: &str = "Exchanges::Gemini";
pub const UNREADABLE: &str = "Gemini: unreadable response";
/// Rails stores `raw["order_id"].to_s` (an empty id when the answer names none); here an answer without one may have
/// placed the order: Ambiguous, as on Binance and KuCoin.
pub const NO_ORDER_ID: &str = "Gemini answered the order with no order id";
/// The reason Gemini gives an order it does not know (HTTP 404), Exchanges::Gemini#get_order's not-found test.
pub const ORDER_NOT_FOUND: &str = "OrderNotFound";
pub const BALANCES: &str = "/v1/balances";
pub const ORDER_STATUS: &str = "/v1/order/status";
pub const ORDER_NEW: &str = "/v1/order/new";
pub const ORDER_CANCEL: &str = "/v1/order/cancel";
pub const PRICE_FEED: &str = "/v1/pricefeed";
/// The whole market's prices in one body: its read stops at this many bytes, never a price from a cut body.
pub const PRICE_FEED_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct GeminiVenue<T: Transport> { transport: T, base: String, pairs: Pairs, identity: Option<Value> }

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }

fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }
/// `x.to_d` of a field that may be absent (`nil.to_d` is 0); present and unreadable refuses the whole answer.
fn or_zero(v: &Value) -> Result<BigDec, VenueError> { if v.is_null() { Ok(BigDec::zero()) } else { required(v) } }
/// Ruby's truthiness of a JSON value: only null (or absent) and false are false.
fn truthy(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) }
fn text(v: &Value) -> Result<&str, VenueError> { v.as_str().ok_or_else(unreadable) }

/// Exchange#response_field(result, 'reason') of a failure: the first error's JSON `reason`.
pub fn reason(messages: &[String]) -> Option<String> {
    let body: Value = serde_json::from_str(messages.first()?).ok()?;
    body.get("reason").and_then(Value::as_str).map(str::to_string)
}

/// Exchanges::Gemini#parse_order_data and #parse_order_status. The price is `avg_execution_price`, else the order's own
/// `price`, a zero one none; the executed quote is executed × that price (0 without one). Cancelled whatever it filled; live
/// with something remaining is open; not live with something executed is closed; anything else unknown.
pub fn parse_order(order_id: &str, o: &Value) -> Result<OrderState, VenueError> {
    let dig = |k: &str| super::dig_or_raise(o, k);
    let symbol = text(dig("symbol")?)?;
    // #parse_order_type: only `market` and `exchange market` are market orders; Gemini's crossed IOC is `exchange limit`.
    let limit = !matches!(text(dig("type")?)?.to_lowercase().as_str(), "market" | "exchange market");
    let mut price = required(dig("avg_execution_price")?)?;
    if price.is_zero() { price = required(dig("price")?)?; }
    let price = (!price.is_zero()).then_some(price);
    let amount = required(dig("original_amount")?)?;
    let sell = text(dig("side")?)?.eq_ignore_ascii_case("sell");
    let amount_exec = required(dig("executed_amount")?)?;
    let quote_amount_exec = price.as_ref().map_or_else(BigDec::zero, |p| &amount_exec * p);
    let (live, remaining) = (truthy(&o["is_live"]), or_zero(&o["remaining_amount"])?);
    let status = if truthy(&o["is_cancelled"]) { OrderStatus::Cancelled } else if live && remaining.is_positive() { OrderStatus::Open }
                 else if !live && amount_exec.is_positive() { OrderStatus::Closed } else { OrderStatus::Unknown };
    Ok(OrderState { txid: order_id.into(), status, price, amount: Some(amount), quote_amount: None, amount_exec, quote_amount_exec: Some(quote_amount_exec),
                    limit, sell, pair: Some(symbol.into()), asset_class: None })
}

/// Exchanges::Gemini#get_tickers_prices over /v1/pricefeed: every entry's `dig_or_raise`d `pair`, downcased, and its
/// `price`, a later entry for a pair replacing an earlier one. DIVERGES on purpose as Binance and KuCoin do (ruby::json_to_d):
/// a price that is no decimal is left out, where Rails reads it as 0.
pub fn price_feed(body: &Value) -> Result<Vec<(String, BigDec)>, VenueError> {
    let mut out: Vec<(String, BigDec)> = vec![];
    for entry in body.as_array().ok_or_else(unreadable)? {
        let pair = text(super::dig_or_raise(entry, "pair")?)?.to_lowercase();
        let price = super::dig_or_raise(entry, "price")?;
        let Some(price) = crate::ruby::json_to_d(price).ok().flatten() else { continue };
        out.retain(|(p, _)| *p != pair);
        out.push((pair, price));
    }
    Ok(out)
}

impl<T: Transport> GeminiVenue<T> {
    pub fn new(transport: T, base: &str) -> Self { Self { transport, base: base.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// The exchange's tickers (`Pairs::load`): venue codes to assets.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    /// #post_signed: an empty-body POST, its params (nils compacted) in the payload the transport signs.
    pub fn private(&self, path: &str, params: Vec<(&'static str, Value)>) -> HttpRequest {
        let body: Map<String, Value> = params.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.to_string(), v)).collect();
        HttpRequest { method: "POST", base: self.base.clone(), path: path.into(), query: vec![], body: Some(Value::Object(body)), not_after: None }
    }
    /// #get_public.
    fn public(&self, path: String) -> HttpRequest { HttpRequest { method: "GET", base: self.base.clone(), path, query: vec![], body: None, not_after: None } }

    /// One call as honeymaker's #with_rescue returns it and Exchange#ambiguous_placement_error? reads it for a placement. A
    /// transport failure is the shared split (`http::transport_failure`). A non-2xx answer is its body (Faraday's message
    /// when empty); a read's 401 is `Unauthorized`; on a placement only a 4xx, or a placement-safe InvalidNonce whatever its
    /// status, is definitive, anything else may have placed the order. A 2xx that is not JSON is unreadable.
    async fn call(&self, r: &HttpRequest, placement: bool, limit: usize) -> Result<Value, VenueError> {
        http::decode_json(&self.answer(r, placement, limit).await?).map_err(|_| unreadable())
    }

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

    async fn post(&self, path: &str, params: Vec<(&'static str, Value)>) -> Result<Value, VenueError> {
        self.call(&self.private(path, params), false, http::MAX_BODY).await
    }

    /// One signed POST as honeymaker's #with_rescue returns it to the ledger and balance syncs: the 2xx body of at most
    /// `limit` bytes, unparsed (`Ok`), else the failure's text: the answer's body, or Faraday's (or the transport's) message.
    pub async fn read_text(&self, path: &str, params: Vec<(&'static str, Value)>, limit: usize) -> Result<String, String> {
        let r = self.private(path, params);
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for POST {}", resp.status, r.url())),
        }
    }

    /// Clients::Gemini#get_raw_balances as Exchanges::Gemini#get_balances reads it: no data is "Failed to get Gemini
    /// balances"; every row's `currency` must be there.
    async fn balances(&self) -> Result<Vec<Value>, VenueError> {
        let body = self.post(BALANCES, vec![]).await?;
        if body.is_null() { return Err(VenueError::Rejected(vec!["Failed to get Gemini balances".into()])); }
        let rows = body.as_array().ok_or_else(unreadable)?;
        rows.iter().map(|row| { text(super::dig_or_raise(row, "currency")?)?; Ok(row.clone()) }).collect()
    }

    /// `available` of the last row whose upcased currency the predicate picks (absent is 0); `amount` must be readable too,
    /// as #get_balances digs it for the locked part.
    async fn available(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let mut free = BigDec::zero();
        for row in self.balances().await? {
            if !pick(&row["currency"].as_str().unwrap_or_default().to_uppercase()) { continue; }
            free = required(super::dig_or_raise(&row, "available")?)?;
            required(super::dig_or_raise(&row, "amount")?)?;
        }
        Ok(free)
    }

    /// Clients::Gemini#order_status as Exchanges::Gemini#get_order reads it: None for an order Gemini answers OrderNotFound.
    async fn get_order(&self, order_id: &str) -> Result<Option<Value>, VenueError> {
        match self.post(ORDER_STATUS, vec![("order_id", Value::String(order_id.into()))]).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m)) if reason(&m).as_deref() == Some(ORDER_NOT_FOUND) => Ok(None),
            other => other.map(Some),
        }
    }
}

impl<T: Transport> super::Venue for GeminiVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    /// #get_ask_price / #get_bid_price through #get_bid_ask_price (pubticker: `bid` and `ask` both dug), and #get_last_price
    /// (the same read's `last`). A zero price raises "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let name = &ticker.ticker;
        let body = self.call(&self.public(format!("/v1/pubticker/{name}")), false, http::MAX_BODY).await?;
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let (bid, ask) = (required(super::dig_or_raise(&body, "bid")?)?, required(super::dig_or_raise(&body, "ask")?)?);
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => (required(super::dig_or_raise(&body, "last")?)?, "last"),
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {name}: {}", price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Gemini#new_order: an `exchange limit` order with the gem's fields
    /// in its order (symbol, amount, price, side, type, client_order_id, options). A crossed (market) order is
    /// `immediate-or-cancel` at its persisted price; a limit order rests (`options: []`). `client_order_id` is the intent id
    /// (spec E §2.6; Rails sends none). Sent at most once; the stored id is the raw order_id.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let (price, options) = match &o.kind {
            OrderKind::Crossed { price } => (price, vec![Value::from("immediate-or-cancel")]),
            OrderKind::Limit { price } => (price, vec![]),
            // A crossing venue's market plan always carries its crossed price (placement::send_to_venue refuses one without).
            OrderKind::Market => return Err(VenueError::Transient("Gemini has no market order and no crossed price was prepared; not sent".into())),
        };
        // Exchanges::Gemini#minimum_amount_logic is :base: an order is always a base amount.
        if o.quote_volume { return Err(VenueError::Transient("Gemini takes no quote-sized order; not sent".into())); }
        let params = vec![("symbol", o.pair.clone().into()), ("amount", o.volume.clone().into()), ("price", price.clone().into()), ("side", o.side.as_str().into()),
                          ("type", "exchange limit".into()), ("client_order_id", o.cl_ord_id.clone().into()), ("options", Value::Array(options))];
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")), ..self.private(ORDER_NEW, params) };
        let answer = self.call(&r, true, http::MAX_BODY).await?;
        match &answer["order_id"] {
            Value::Number(n) => Ok(n.to_string()),
            Value::String(id) if !id.is_empty() => Ok(id.clone()),
            _ => Err(VenueError::Ambiguous(NO_ORDER_ID.into())),
        }
    }

    /// #get_orders: #get_order per id; an order Gemini answers OrderNotFound is left out (`missing`, for
    /// Bot::StaleOrderResolver); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids {
            if let Some(raw) = self.get_order(id).await? { out.push(parse_order(id, &raw)?); }
        }
        Ok(out)
    }

    /// #get_order (Bot::FetchAndUpdateOrderJob): a gone order fails with Rails' "Gemini no longer knows order <id>".
    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.get_order(txid).await? {
            Some(raw) => Ok((vec![parse_order(txid, &raw)?], vec![])),
            None => Err(VenueError::Rejected(vec![format!("Gemini no longer knows order {txid}")])),
        }
    }

    /// Recovery may only FIND the order (plan VB §1.5: `absence_provable` false): /v1/order/status by `client_order_id`, an
    /// order (or a list of them) naming this client order id and symbol. Anything else proves nothing: the intent waits,
    /// for a later lookup or `deltabadger resolve-placement`. Rails has no lookup (it sends no client order id).
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let body = match self.post(ORDER_STATUS, vec![("client_order_id", Value::String(cl_ord_id.into()))]).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m)) if reason(&m).as_deref() == Some(ORDER_NOT_FOUND) => return Ok(None),
            other => other?,
        };
        let orders = match &body { Value::Array(list) => list.iter().collect::<Vec<_>>(), one => vec![one] };
        let mine = |o: &&Value| o["client_order_id"].as_str() == Some(cl_ord_id) && o["symbol"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(symbol));
        match orders.into_iter().find(mine) {
            Some(o) => {
                let id = match &o["order_id"] { Value::Number(n) => n.to_string(), Value::String(s) if !s.is_empty() => s.clone(), _ => return Err(unreadable()) };
                parse_order(&id, o).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Exchanges::Gemini reads no fills for a gone order: it stays missing, for Bot::StaleOrderResolver.
    async fn fills_from_trades(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }

    /// #get_balances' `available` for the quote asset: each row's currency mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.available(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.available(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.available(|code| code == ticker.quote_code).await }

    /// Exchanges::Gemini#get_tickers_prices (`price_feed`). Rails caches the answer for a minute; this reads fresh. A 2xx body
    /// that is not JSON is honeymaker's Failure (its text). Each price is its decimal's `to_s('F')`.
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let text = self.answer(&self.public(PRICE_FEED.into()), false, PRICE_FEED_LIMIT).await?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        Ok(price_feed(&body)?.into_iter().map(|(pair, price)| (pair, Value::String(price.to_s_f()))).collect())
    }

    /// #cancel_order through Clients::Gemini#cancel_order: any 2xx answer is the cancel, as Rails takes it.
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        self.post(ORDER_CANCEL, vec![("order_id", Value::String(txid.into()))]).await.map(|_| ())
    }
}
