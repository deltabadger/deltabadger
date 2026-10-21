//! Honeymaker::Clients::Bitvavo and Exchanges::Bitvavo for the engine's spot orders (plan VB slice A3): prices, balances,
//! placement, polls, cancel, the gone-order recovery from the trades and the recovery by client order id. Rails is the
//! oracle (post-audit, AUDIT-bitvavo V1-V7); each method names the Ruby it ports. `venue::live` builds it for every
//! Bitvavo bot.
//!
//! Signing (Honeymaker::Clients::Bitvavo#signed_headers): BITVAVO-ACCESS-SIGNATURE = hex(HMAC-SHA256(secret, timestamp +
//! METHOD + path + `?query` (a GET or a DELETE with params) | the JSON body (a POST))), `sign::BITVAVO`. The transport
//! (`http::ReqwestTransport::bitvavo`) signs at send time over exactly the query and body bytes it sends.
//!
//! Where it differs on purpose (plan VB §5, named in tests/parity.rs): the order carries `clientOrderId` = the intent id
//! (D9; Rails sends none) and recovery asks for it (`order_by_client_id`), which can only FIND an order (absence is never
//! concluded, ABSENCE_UNPROVEN); a placement that may have reached Bitvavo (a lost reply, a 5xx, an answer without an
//! orderId) keeps the intent where Rails writes a failed (or a "BTC-EUR-") row; a number that is no decimal is refused
//! where Rails reads it as 0.
use super::binance::{build_query, Pairs};
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, BITVAVO as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const URL: &str = "https://api.bitvavo.com";
pub const BITVAVO: &str = "Exchanges::Bitvavo";
pub const UNREADABLE: &str = "Bitvavo: unreadable response";
/// Exchanges::Bitvavo::KEY_CHECK_ORDER_NOT_FOUND: errorCode 240, Bitvavo's order-gone signal (#bitvavo_not_found?).
pub const ORDER_NOT_FOUND: i64 = 240;
/// Honeymaker::Clients::Bitvavo::DEFAULT_OPERATOR_ID: mandatory on every order create and cancel (errorCode 203).
pub const OPERATOR_ID: i64 = 1;
/// Exchanges::Bitvavo::TRADES_PAGE_LIMIT: /v2/trades answers at most 1000 a call.
pub const TRADES_PAGE: usize = 1000;
pub const ORDER_PATH: &str = "/v2/order";
pub const TRADES_PATH: &str = "/v2/trades";
pub const BALANCE_PATH: &str = "/v2/balance";
pub const MARKETS_PATH: &str = "/v2/markets";
/// A 200 to the order without an orderId: Rails stores the order as "<market>-"; Bitvavo took the order, or may have.
pub const NO_ORDER_ID: &str = "Bitvavo answered the order with no order id";
/// Plan VB §1.5: on Bitvavo absence is never concluded. The client-order lookup answers 240 for an order it does not hold,
/// which proves nothing about one sent moments ago; the intent waits for `deltabadger resolve-placement`. A divergence:
/// Rails sends no clientOrderId and has no lookup at all.
pub const ABSENCE_UNPROVEN: &str = "Bitvavo: the client order lookup did not find this order, and Bitvavo cannot prove it absent; settle it with `deltabadger resolve-placement`";
/// The trades walk's ceiling: 100,000 pages of 1000 (ponytail: Rails has none; a misbehaving cursor fails, never loops).
const MAX_TRADE_PAGES: usize = 100_000;

/// Honeymaker's #get_public paths: sent with its unauthenticated headers.
pub fn is_public(path: &str) -> bool {
    [MARKETS_PATH, "/v2/ticker/", "/v2/assets"].iter().any(|p| path.starts_with(p)) || path.ends_with("/candles")
}

/// The headers honeymaker sends with `r` at `timestamp_ms`: #signed_headers (`sign::BITVAVO`), or #unauthenticated_headers on a
/// public read.
pub fn headers(r: &HttpRequest, key: &str, secret: &str, timestamp_ms: i64) -> Vec<(String, String)> {
    if is_public(&r.path) { return super::sign::json_headers(); }
    super::sign::prehash(&super::sign::BITVAVO, r, &build_query(&r.query), key, secret, "", timestamp_ms)
}

/// Ruby's `(time.to_f * 1000).to_i`, as Exchanges::Bitvavo turns a time into its milliseconds: Time#to_f of whole
/// nanoseconds is their Float over a billion (time.c `rb_time_unmagnify_to_float`), so 02:00:00.250 is ...249.
pub fn ruby_ms(t: DateTime<Utc>) -> i64 {
    let nanos = i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos());
    let seconds = if nanos % 1_000_000_000 == 0 { (nanos / 1_000_000_000) as f64 } else { nanos as f64 / 1e9 };
    (seconds * 1000.0) as i64
}

/// Exchanges::Bitvavo#split_order_id: "BTC-EUR-abc-123" → ("BTC-EUR", "abc-123") (the market has one dash, the id may too).
pub fn split_order_id(order_id: &str) -> (String, String) {
    let parts: Vec<&str> = order_id.split('-').collect();
    (parts.iter().take(2).copied().collect::<Vec<_>>().join("-"), parts.iter().skip(2).copied().collect::<Vec<_>>().join("-"))
}

/// Exchange#response_field(result, 'errorCode') of a failure's text: the JSON object's `errorCode`, an Integer only.
pub fn error_code(text: &str) -> Option<i64> {
    serde_json::from_str::<Value>(text).ok()?.get("errorCode")?.as_i64()
}

/// #bitvavo_not_found?: Bitvavo's own 240 in the body, never a bare 404 a gateway or a wrong path also answers with.
pub fn not_found(messages: &[String]) -> bool { messages.first().and_then(|m| error_code(m)) == Some(ORDER_NOT_FOUND) }

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, VenueError> { super::json::field(v, key, unreadable) }
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }
/// `x.to_d` of a present value; null or absent is 0 (nil.to_d).
fn decimal(v: &Value) -> Result<BigDec, VenueError> { if v.is_null() { Ok(BigDec::zero()) } else { required(v) } }
/// `#to_s` of a scalar, as Ruby interpolates it (nil is "").
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
fn nonzero(d: BigDec) -> Option<BigDec> { (!d.is_zero()).then_some(d) }

/// Exchanges::Bitvavo#parse_order_data (V3): the price is filledAmountQuote / filledAmount, unrounded, whenever something
/// filled, else the order's own price; a zero price, amount or amountQuote is none. ORDER_STATUS_MAP; an order type or a
/// status it does not list is refused, as Rails raises.
pub fn parse_order(order_id: &str, o: &Value) -> Result<OrderState, VenueError> {
    let market = field(o, "market")?.as_str().ok_or_else(unreadable)?;
    let limit = match field(o, "orderType")?.as_str() {
        Some("market") => false, Some("limit") => true,
        other => return Err(VenueError::Rejected(vec![format!("Unknown Bitvavo order type: {}", other.unwrap_or_default())])),
    };
    let sell = field(o, "side")?.as_str().ok_or_else(unreadable)?.eq_ignore_ascii_case("sell");
    let (amount_exec, quote_exec) = (decimal(&o["filledAmount"])?, decimal(&o["filledAmountQuote"])?);
    let mut price = decimal(&o["price"])?;
    if quote_exec.is_positive() && amount_exec.is_positive() { price = quote_exec.div(&amount_exec).ok_or_else(unreadable)?; }
    let status = match field(o, "status")?.as_str() {
        Some("new" | "partiallyFilled") => OrderStatus::Open,
        Some("filled") => OrderStatus::Closed,
        Some("canceled" | "cancelled" | "expired") => OrderStatus::Cancelled,
        Some("rejected") => OrderStatus::Failed,
        other => return Err(VenueError::Rejected(vec![format!("Unknown Bitvavo order status: {}", other.unwrap_or_default())])),
    };
    Ok(OrderState { txid: order_id.into(), status, price: nonzero(price), amount: nonzero(decimal(&o["amount"])?), quote_amount: nonzero(decimal(&o["amountQuote"])?),
                    amount_exec, quote_amount_exec: Some(quote_exec), limit, sell, pair: Some(market.into()), asset_class: None })
}

/// The prices of a GET /v2/ticker/price answer, as Exchanges::Bitvavo#get_tickers_prices reads them: every row by its
/// `dig_or_raise`d `market`, its `price` kept only when positive, a later row for a market replacing an earlier one.
/// DIVERGES on purpose as KuCoin's does: a price that is no decimal is left out (String#to_d reads "12x" as 12).
pub fn ticker_prices(body: &Value) -> Result<Vec<(String, BigDec)>, VenueError> {
    let mut out: Vec<(String, BigDec)> = vec![];
    for row in body.as_array().ok_or_else(unreadable)? {
        let market = super::dig_or_raise(row, "market")?.as_str().ok_or_else(unreadable)?.to_string();
        let price = match &row["price"] {
            Value::Null => None,
            v @ (Value::String(_) | Value::Number(_)) => crate::ruby::json_to_d(v).ok().flatten(),
            _ => return Err(unreadable()),
        };
        if let Some(p) = price.filter(BigDec::is_positive) {
            out.retain(|(m, _)| *m != market);
            out.push((market, p));
        }
    }
    Ok(out)
}

/// Exchanges::Bitvavo#order_data_from_trades: the summed amounts and amount × price, the price their ratio (unrounded),
/// the first trade's side, a limit order (trades carry no order type), closed.
pub fn order_from_trades(order_id: &str, market: &str, trades: &[Value]) -> Result<OrderState, VenueError> {
    let (mut amount, mut quote) = (BigDec::zero(), BigDec::zero());
    for t in trades {
        let a = decimal(&t["amount"])?;
        quote = &quote + &(&a * &decimal(&t["price"])?);
        amount = &amount + &a;
    }
    let price = if amount.is_positive() { quote.div(&amount) } else { None };
    Ok(OrderState { txid: order_id.into(), status: OrderStatus::Closed, price, amount: None, quote_amount: None, amount_exec: amount,
                    quote_amount_exec: Some(quote), limit: true, sell: to_s(&trades[0]["side"]).eq_ignore_ascii_case("sell"),
                    pair: Some(market.into()), asset_class: None })
}

#[derive(Clone)]
pub struct BitvavoVenue<T: Transport> { transport: T, base: String, pairs: Pairs, identity: Option<Value> }

impl<T: Transport> BitvavoVenue<T> {
    pub fn new(transport: T, base: &str) -> Self { Self { transport, base: base.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// The exchange's tickers (`Pairs::load`): venue codes to assets.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }
    pub fn transport(&self) -> &T { &self.transport }

    pub fn request(&self, method: &'static str, path: &str, mut query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        query.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path: path.into(), query, body, not_after: None }
    }

    /// One send as honeymaker's #with_rescue reads it: the 2xx body's text, or the failure. A transport failure that sent
    /// nothing is Transient (Rejected on a placement: definitive); one that may have sent is Transient on a read and
    /// Ambiguous on a placement. A non-2xx answer is its body (Faraday's message when empty); on a placement only a 4xx, or
    /// a placement-safe one (errorCode 304) whatever its status, is definitive; a 401 on a read carries its status.
    async fn answer(&self, r: &HttpRequest, placement: bool) -> Result<String, VenueError> {
        let resp = self.transport.send(r).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))?;
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
        http::decode_json(&self.answer(r, placement).await?).map_err(|_| unreadable())
    }

    async fn get(&self, path: &str, query: Vec<(&'static str, String)>) -> Result<Value, VenueError> { self.call(&self.request("GET", path, query, None), false).await }

    /// One signed GET as honeymaker's #with_rescue returns it to Exchanges::Bitvavo's ledger and balance reads: the 2xx body of
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

    /// Clients::Bitvavo#get_raw_balance as #get_balances reads it: one row per symbol held.
    async fn balances(&self) -> Result<Vec<Value>, VenueError> {
        let body = self.get(BALANCE_PATH, vec![]).await?;
        body.as_array().cloned().ok_or_else(unreadable)
    }

    /// `available` of the last row the predicate picks (absent is 0); `inOrder` must be readable too, as #get_balances digs for it.
    async fn available(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let mut free = BigDec::zero();
        for row in self.balances().await? {
            if !row["symbol"].as_str().is_some_and(&pick) { continue; }
            free = required(super::dig_or_raise(&row, "available")?)?;
            required(super::dig_or_raise(&row, "inOrder")?)?;
        }
        Ok(free)
    }

    /// Clients::Bitvavo#get_order as Exchanges::Bitvavo#get_order reads it: None for an order Bitvavo no longer knows (240).
    async fn get_order(&self, order_id: &str) -> Result<Option<OrderState>, VenueError> {
        let (market, ext) = split_order_id(order_id);
        match self.get(ORDER_PATH, vec![("market", market), ("orderId", ext)]).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m)) if not_found(&m) => Ok(None),
            other => parse_order(order_id, &other?).map(Some),
        }
    }

    /// Exchanges::Bitvavo#read_trades: every trade on `market` from `start_ms` on (all of them without one). Newest first, at
    /// most 1000 a call, each page before the oldest id of the one before (`tradeIdTo`, no start/end: those are capped at
    /// 24 h), until a short page or one reaching past `start_ms`; repeats dropped by id; a full page with nothing new fails.
    /// `pause` after every request (the ledger's pacing; recovery is unpaced).
    pub async fn read_trades(&self, market: &str, start_ms: Option<i64>, pause: std::time::Duration) -> Result<Vec<Value>, VenueError> {
        let mut trades: Vec<Value> = vec![];
        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TRADE_PAGES {
            let mut q = vec![("market", market.to_string()), ("limit", TRADES_PAGE.to_string())];
            q.extend(cursor.clone().map(|c| ("tradeIdTo", c)));
            let page = self.get(TRADES_PATH, q).await;
            if !pause.is_zero() { tokio::time::sleep(pause).await; }
            let page = page?;
            let page = page.as_array().ok_or_else(unreadable)?;
            let mut fresh = 0;
            for t in page { if seen.insert(to_s(&t["id"])) { trades.push(t.clone()); fresh += 1; } }
            let oldest = page.iter().map(|t| crate::ruby::to_i(&to_s(&t["timestamp"]))).min();
            if page.len() < TRADES_PAGE || start_ms.is_some_and(|s| oldest.is_some_and(|o| o < s)) {
                return Ok(trades.into_iter().filter(|t| start_ms.is_none_or(|s| crate::ruby::to_i(&to_s(&t["timestamp"])) >= s)).collect());
            }
            if fresh == 0 { return Err(VenueError::Rejected(vec![format!("Bitvavo trades on {market} returned a full page with nothing new")])); }
            cursor = Some(to_s(&page[page.len() - 1]["id"]));
        }
        Err(VenueError::Rejected(vec![format!("Too many pages of Bitvavo {market} trades")]))
    }
}

impl<T: Transport> super::Venue for BitvavoVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    /// #get_ask_price / #get_bid_price through #get_bid_ask_price (GET /v2/ticker/book: bid, bidSize, ask and askSize, each
    /// `dig_or_raise`d), and #get_last_price (GET /v2/ticker/price, its `price`). A zero price raises "Wrong … price".
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let name = &ticker.ticker;
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.get("/v2/ticker/book", vec![("market", name.clone())]).await?;
                let level = |k: &str| super::dig_or_raise(&body, k).and_then(required);
                let bid = level("bid")?;
                level("bidSize")?;
                let ask = level("ask")?;
                level("askSize")?;
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => {
                let body = self.get("/v2/ticker/price", vec![("market", name.clone())]).await?;
                (required(super::dig_or_raise(&body, "price")?)?, "last")
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {name}: {}", price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Bitvavo#place_order: POST /v2/order with market, side, orderType,
    /// then a market order's `amount` or `amountQuote`, or a limit order's `amount` and `price`, then `clientOrderId` (the
    /// intent id; Rails sends none) and operatorId 1. Sent at most once. The stored id is "<market>-<orderId>". A refusal is
    /// Bitvavo's body; an answer without an orderId may have placed the order: Ambiguous.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut body = serde_json::Map::new();
        for (k, v) in [("market", o.pair.as_str()), ("side", o.side.as_str())] { body.insert(k.into(), v.into()); }
        match &o.kind {
            OrderKind::Market => {
                body.insert("orderType".into(), "market".into());
                body.insert(if o.quote_volume { "amountQuote" } else { "amount" }.into(), o.volume.clone().into());
            }
            OrderKind::Limit { price } => {
                body.insert("orderType".into(), "limit".into());
                body.insert("amount".into(), o.volume.clone().into());
                body.insert("price".into(), price.clone().into());
            }
        }
        body.insert("clientOrderId".into(), o.cl_ord_id.clone().into());
        body.insert("operatorId".into(), OPERATOR_ID.into());
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.request("POST", ORDER_PATH, vec![], Some(Value::Object(body))) };
        let answer = self.call(&r, true).await?;
        let id = to_s(&answer["orderId"]);
        if id.is_empty() { return Err(VenueError::Ambiguous(NO_ORDER_ID.into())); }
        let market = answer["market"].as_str().unwrap_or(&o.pair);
        Ok(format!("{market}-{id}"))
    }

    /// #get_orders: #get_order per id; an order Bitvavo no longer knows (240) is left out (`missing`, which polling settles
    /// from the trades, `fills_from_trades`); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids { if let Some(o) = self.get_order(id).await? { out.push(o); } }
        Ok(out)
    }

    /// Recovery (plan VB §1.5) may only FIND the order: GET /v2/order with the market and the clientOrderId the intent sent;
    /// a hit naming them is authoritative. A 240 is never absence (ABSENCE_UNPROVEN).
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let body = match self.get(ORDER_PATH, vec![("market", symbol.to_string()), ("clientOrderId", cl_ord_id.to_string())]).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m)) if not_found(&m) => return Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())),
            other => other?,
        };
        if body["clientOrderId"].as_str() != Some(cl_ord_id) || body["market"].as_str() != Some(symbol) {
            return Err(VenueError::Rejected(vec![format!("Bitvavo answered for client order id {cl_ord_id} on {symbol} with another order")]));
        }
        let id = to_s(&body["orderId"]);
        if id.is_empty() { return Err(unreadable()); }
        parse_order(&format!("{symbol}-{id}"), &body).map(Some)
    }

    /// Exchanges::Bitvavo#recover_order_from_trades for each order #get_orders named missing: every trade on its market from
    /// `since` (the oldest missing row, less an hour) on, through `read_trades`, keyed on its orderId. A fill books the order
    /// closed (#order_data_from_trades); no fill leaves it missing (Bot::StaleOrderResolver decides); a failed read fails.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids {
            let (market, ext) = split_order_id(id);
            let trades: Vec<Value> = self.read_trades(&market, Some(ruby_ms(since)), std::time::Duration::ZERO).await?
                .into_iter().filter(|t| to_s(&t["orderId"]) == ext).collect();
            if !trades.is_empty() { out.push(order_from_trades(id, &market, &trades)?); }
        }
        Ok(out)
    }

    /// #get_balances' `available` for the quote asset: each row's symbol mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.available(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.available(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.available(|code| code == ticker.quote_code).await }

    /// Exchanges::Bitvavo#get_tickers_prices: GET /v2/ticker/price for every market (`ticker_prices`). A 2xx body that is not
    /// JSON is honeymaker's Failure (its text). Rails caches the answer for a minute; this reads fresh.
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let text = self.answer(&self.request("GET", "/v2/ticker/price", vec![], None), false).await?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        Ok(ticker_prices(&body)?.into_iter().map(|(market, price)| (market, Value::String(price.to_s_f()))).collect())
    }

    /// #cancel_order through Clients::Bitvavo#cancel_order: DELETE /v2/order with the market, orderId and operatorId 1; any
    /// 2xx is a cancel, as Rails reads it.
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let (market, ext) = split_order_id(txid);
        self.answer(&self.request("DELETE", ORDER_PATH, vec![("market", market), ("orderId", ext), ("operatorId", OPERATOR_ID.to_string())], None), false).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_order_id_splits_after_the_market() {
        assert_eq!(split_order_id("BTC-EUR-abc-123"), ("BTC-EUR".to_string(), "abc-123".to_string()));
        assert_eq!(split_order_id("BTC-EUR-"), ("BTC-EUR".to_string(), String::new()));
        assert_eq!(error_code(r#"{"errorCode":240,"error":"No order found."}"#), Some(240));
        assert_eq!(error_code(r#"{"errorCode":"240"}"#), None, "a String never equals Rails' Integer");
        assert_eq!(error_code("<html>404</html>"), None);
        assert_eq!(ruby_ms("2026-09-20T02:00:00.250000Z".parse().unwrap()), 1_789_869_600_249, "as Ruby's Float");
        assert_eq!(ruby_ms("2026-09-15T00:00:00Z".parse().unwrap()), 1_789_430_400_000);
    }
}
