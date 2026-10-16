//! Honeymaker::Clients::Binance (and ::BinanceUs) for the tracker's reads (signed GETs), and Exchanges::Binance for the
//! engine's one-asset spot buys (spec E2): prices, balance, placement, polls and the fill rebuild. Rails is the oracle; each
//! method names the Ruby it ports. Binance.US trades nowhere here: only the tracker reads it.
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, PriceSide};
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
pub struct BinanceVenue<T: Transport> { transport: T, base: String, secret: String, pairs: Pairs, identity: Option<serde_json::Value> }

impl<T: Transport> BinanceVenue<T> {
    pub fn new(transport: T, base: &str, secret: &str) -> Self { Self { transport, base: base.into(), secret: secret.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// Another host: local test servers and the ignored testnet recorder only.
    #[doc(hidden)]
    pub fn with_base(mut self, base: &str) -> Self { self.base = base.into(); self }
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

    fn parse_order(&self, order_id: &str, o: &serde_json::Value) -> Result<Order, super::VenueError> { parse_order(order_id, o, &self.pairs) }

    /// #get_order / #get_orders after parsing: an order that executed nothing keeps the order's own figures; one that did is
    /// rebuilt from all its trades (#get_aggregated_trades_for_orders): price and net amounts.
    async fn settle(&self, order: Order, order_id: &str) -> Result<OrderState, super::VenueError> {
        let Order { mut state, executed } = order;
        if executed.is_zero() { return Ok(state); }
        let fill = self.fill(order_id, &executed).await?;
        state.price = Some(fill.price);
        state.amount_exec = fill.amount_exec;
        state.quote_amount_exec = Some(fill.quote_amount_exec);
        Ok(state)
    }

    /// #get_aggregated_trades_for_orders for one order: myTrades by symbol and orderId, paged by trade id from 0 (without one
    /// Binance answers the latest trades), each trade once by its id, 100 pages at most; complete only when the gross
    /// quantities add up to executedQty, else Rails' failure (the order stays waiting).
    async fn fill(&self, order_id: &str, executed: &BigDec) -> Result<Fill, super::VenueError> {
        let mut parts = order_id.split('-');
        let (symbol, ext) = (parts.next().unwrap_or_default(), crate::ruby::to_i(parts.next().unwrap_or_default()));
        let mut trades: Vec<(i64, serde_json::Value)> = vec![];
        let mut from_id = 0i64;
        let mut exhausted = true;
        for _ in 0..PAGES {
            let r = self.signed_now("GET", "/api/v3/myTrades", vec![("symbol", symbol.to_string()), ("orderId", ext.to_string()),
                                    ("fromId", from_id.to_string()), ("limit", PAGE.to_string()), ("recvWindow", READ_RECV_WINDOW.into())]);
            let body = self.call(&r, false).await?;
            let page = body.as_array().ok_or_else(unreadable)?;
            for raw in page {
                let id = field(raw, "id")?.as_i64().ok_or_else(unreadable)?;
                match trades.iter_mut().find(|(t, _)| *t == id) { Some(slot) => slot.1 = raw.clone(), None => trades.push((id, raw.clone())) }
            }
            if page.len() < PAGE { exhausted = false; break; }
            from_id = trades.iter().map(|(id, _)| *id).max().unwrap_or_default() + 1;
        }
        if exhausted { return Err(super::VenueError::Rejected(vec![format!("Too many pages of Binance {symbol} trades")])); }
        let parsed = trades.iter().map(|(_, raw)| parse_trade(raw, &self.pairs)).collect::<Result<Vec<_>, _>>()?;
        if parsed.is_empty() || parsed.iter().fold(BigDec::zero(), |a, t| &a + &t.amount) != *executed {
            return Err(super::VenueError::Rejected(vec![format!("Binance trades of order {order_id} do not add up to its executed quantity yet")]));
        }
        aggregate(&parsed, self.pairs.by_symbol.get(symbol))
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
            // Honeymaker returns a transport failure as a Failure (it never raises): on a placement that provably sent nothing
            // it is a definitive refusal, a failed row as Rails writes one (Exchange#ambiguous_placement_error?: pre-transmission).
            Err(TransportError::NotSent(m)) if placement => return Err(VenueError::Rejected(vec![redact(m)])),
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

/// #get_orders / #get_aggregated_trades_for_orders: a page's size, and the most pages one walk reads.
const PAGE: usize = 1000;
const PAGES: usize = 100;

/// One order as #parse_order_data reads it, before its fill is settled (`BinanceVenue::settle`).
#[derive(Clone, Debug, PartialEq)]
pub struct Order { pub state: OrderState, pub executed: BigDec }

/// One trade as #parse_trade_data reads it: gross, then net of a commission in the base (buy −, sell +) or the quote
/// (buy +, sell −); a commission in a third asset (BNB) is ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Trade { pub price: BigDec, pub amount: BigDec, pub quote_amount: BigDec, pub amount_exec: BigDec, pub quote_amount_exec: BigDec }

/// What #aggregate_trades gives the order: the qty-weighted price (rounded half up to the pair's price decimals when the
/// pair is known) and the summed net amounts.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill { pub price: BigDec, pub amount_exec: BigDec, pub quote_amount_exec: BigDec }

/// Utilities::Hash.dig_or_raise: absent or null is unreadable.
fn field<'a>(v: &'a serde_json::Value, key: &str) -> Result<&'a serde_json::Value, super::VenueError> {
    v.get(key).filter(|x| !x.is_null()).ok_or_else(unreadable)
}

/// Exchanges::Binance#parse_order_data. ORDER_STATUS_MAP; a status it does not list is Unknown (Rails raises; the poll
/// decides, spec E §2.4). An order type other than MARKET or LIMIT is refused, as Rails raises.
pub fn parse_order(order_id: &str, o: &serde_json::Value, pairs: &Pairs) -> Result<Order, super::VenueError> {
    use super::OrderStatus;
    let symbol = field(o, "symbol")?.as_str().ok_or_else(unreadable)?;
    let limit = match field(o, "type")?.as_str() {
        Some("MARKET") => false, Some("LIMIT") => true,
        other => return Err(super::VenueError::Rejected(vec![format!("Unknown Binance order type: {}", other.unwrap_or_default())])),
    };
    let nonzero = |d: BigDec| (!d.is_zero()).then_some(d);
    let amount = nonzero(required(field(o, "origQty")?)?);
    let quote_amount = nonzero(required(field(o, "origQuoteOrderQty")?)?);
    let sell = field(o, "side")?.as_str().ok_or_else(unreadable)?.eq_ignore_ascii_case("sell");
    let executed = required(field(o, "executedQty")?)?;
    // "for some historical orders" the cumulative quote is negative: nil, never written.
    let quote_executed = Some(required(field(o, "cummulativeQuoteQty")?)?).filter(|q| *q >= BigDec::zero());
    let status = match field(o, "status")?.as_str() {
        Some("NEW" | "PENDING_NEW" | "PARTIALLY_FILLED") => OrderStatus::Open,
        Some("FILLED") => OrderStatus::Closed,
        Some("CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH") => OrderStatus::Cancelled,
        Some("REJECTED") => OrderStatus::Failed,
        _ => OrderStatus::Unknown, // PENDING_CANCEL, and anything else
    };
    let mut price = required(field(o, "price")?)?;
    if price.is_zero() && executed.is_positive() {
        if let Some(q) = quote_executed.as_ref().filter(|q| q.is_positive()) {
            price = q.div(&executed).expect("executed is positive");
            if let Some(pair) = pairs.by_symbol.get(symbol) { price = price.round(pair.price_decimals); }
        }
    }
    let state = OrderState { txid: order_id.into(), status, price: nonzero(price), amount, quote_amount, amount_exec: executed.clone(),
                             quote_amount_exec: quote_executed, limit, sell, pair: Some(symbol.into()), asset_class: None };
    Ok(Order { state, executed })
}

/// Exchanges::Binance#parse_trade_data. The commission's asset is matched against the pair's base and quote, or, for a symbol
/// the exchange no longer lists (a renamed pair), against the symbol's start and end.
pub fn parse_trade(t: &serde_json::Value, pairs: &Pairs) -> Result<Trade, super::VenueError> {
    let symbol = field(t, "symbol")?.as_str().ok_or_else(unreadable)?;
    field(t, "orderId")?;
    let (price, amount, quote_amount, commission) =
        (required(field(t, "price")?)?, required(field(t, "qty")?)?, required(field(t, "quoteQty")?)?, required(field(t, "commission")?)?);
    let asset = field(t, "commissionAsset")?.as_str().ok_or_else(unreadable)?;
    let buy = *field(t, "isBuyer")? == serde_json::Value::Bool(true);
    field(t, "isMaker")?;
    let (in_base, in_quote) = match pairs.by_symbol.get(symbol) {
        Some(p) => (asset == p.base, asset == p.quote),
        None => (symbol.starts_with(asset), symbol.ends_with(asset)),
    };
    let amount_exec = if in_base { if buy { &amount - &commission } else { &amount + &commission } } else { amount.clone() };
    let quote_amount_exec = if in_quote { if buy { &quote_amount + &commission } else { &quote_amount - &commission } } else { quote_amount.clone() };
    Ok(Trade { price, amount, quote_amount, amount_exec, quote_amount_exec })
}

/// Exchanges::Binance#aggregate_trades over one order's trades (never empty: the caller refuses that first).
/// Utilities::Math.weighted_average divides the BigDecimal Σ price×qty by Σ qty `.to_f`: a Float divisor, as Ruby converts it
/// (`BigDec::from_f64`, Float#to_d).
pub fn aggregate(trades: &[Trade], pair: Option<&Pair>) -> Result<Fill, super::VenueError> {
    let total = |f: fn(&Trade) -> &BigDec| trades.iter().fold(BigDec::zero(), |a, t| &a + f(t));
    let weighted = trades.iter().fold(BigDec::zero(), |a, t| &a + &(&t.price * &t.amount));
    let divisor = BigDec::from_f64(total(|t| &t.amount).to_f()).map_err(|_| unreadable())?;
    let mut price = weighted.div(&divisor).ok_or_else(unreadable)?;
    if let Some(p) = pair { price = price.round(p.price_decimals); }
    Ok(Fill { price, amount_exec: total(|t| &t.amount_exec), quote_amount_exec: total(|t| &t.quote_amount_exec) })
}

/// A Binance decimal string or number: present and readable, or the whole answer is unreadable (never a zero). DIVERGES from
/// Ruby on purpose (ruby::json_to_d): Rails' `.to_d` reads "garbage" as 0.
fn required(v: &serde_json::Value) -> Result<BigDec, super::VenueError> {
    crate::ruby::json_to_d(v).ok().flatten().ok_or_else(unreadable)
}

impl<T: Transport> BinanceVenue<T> {
    /// #get_balances' `free` for one Binance asset code (never free + locked). Absent is 0; unreadable is an error.
    async fn free_of(&self, code: &str) -> Result<BigDec, super::VenueError> {
        let r = self.signed_now("GET", "/api/v3/account", vec![("omitZeroBalances", "true".into()), ("recvWindow", READ_RECV_WINDOW.into())]);
        let body = self.call(&r, false).await?;
        let mut free = BigDec::zero();
        for row in body["balances"].as_array().ok_or_else(unreadable)? {
            if row["asset"].as_str() != Some(code) { continue; }
            required(&row["locked"])?;
            free = required(&row["free"])?;
        }
        Ok(free)
    }
}

impl<T: Transport> super::Venue for BinanceVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<serde_json::Value> { self.identity.clone() }

    /// Exchanges::Binance#get_ask_price / #get_bid_price through #get_bid_ask_price (bookTicker: all four of bidPrice, bidQty, askPrice and
    /// askQty are dug for), and #get_last_price (ticker/price). A failure is #parse_error_message's text; a zero price raises
    /// "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, super::VenueError> {
        let symbol = vec![("symbol", ticker.ticker.clone())];
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.call(&self.request("GET", "/api/v3/ticker/bookTicker", symbol), false).await?;
                for field in ["bidPrice", "bidQty", "askPrice", "askQty"] { required(&body[field])?; }
                if side == PriceSide::Bid { (required(&body["bidPrice"])?, "bid") } else { (required(&body["askPrice"])?, "ask") }
            }
            PriceSide::Last => (required(&self.call(&self.request("GET", "/api/v3/ticker/price", symbol), false).await?["price"])?, "last"),
        };
        if price.is_zero() { return Err(super::VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.ticker, price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Binance#new_order: POST /api/v3/order, every field in the query.
    /// A market buy is `quoteOrderQty` or `quantity` with selfTradePreventionMode EXPIRE_MAKER; a limit buy GTC `price` and
    /// `quantity` (Rails sends no STP there). Plus Rust's `newClientOrderId` (the intent id; Rails sends none) and the gem's
    /// default recvWindow. Sent at most once. The stored id is "SYMBOL-<orderId>" (ids repeat across symbols); an answer
    /// without an orderId (Rails would store "SYMBOL-") may have placed the order: Ambiguous.
    async fn add_order(&self, o: &NewOrder) -> Result<String, super::VenueError> {
        let mut params = vec![("symbol", o.pair.clone()), ("side", o.side.as_str().to_ascii_uppercase())];
        let kind = match &o.kind {
            OrderKind::Market => {
                params.extend([("type", "MARKET".into()), (if o.quote_volume { "quoteOrderQty" } else { "quantity" }, o.volume.clone()),
                               ("selfTradePreventionMode", "EXPIRE_MAKER".into())]);
                "market"
            }
            OrderKind::Limit { price } => {
                params.extend([("type", "LIMIT".into()), ("timeInForce", "GTC".into()), ("price", price.clone()), ("quantity", o.volume.clone())]);
                "limit"
            }
        };
        params.extend([("newClientOrderId", o.cl_ord_id.clone()), ("recvWindow", PLACE_RECV_WINDOW.into())]);
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.signed_now("POST", "/api/v3/order", params) };
        let body = self.call(&r, true).await?;
        let id = match &body["orderId"] {
            serde_json::Value::Number(n) if n.is_u64() || n.is_i64() => n.to_string(),
            serde_json::Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err(super::VenueError::Ambiguous(format!("Failed to set Binance {kind} order (order_id is {}-)", o.pair))),
        };
        Ok(format!("{}-{id}", o.pair))
    }
    /// #get_orders: per symbol (the stored "SYMBOL-<orderId>"), allOrders from the smallest id up, 1000 a page, a full page
    /// moving past its last id, a short one ending the walk (100 pages at most); each order with an execution is rebuilt from
    /// its trades. An id the venue did not return is simply absent (#get_orders answers `missing: []`).
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, super::VenueError> {
        let mut by_symbol: Vec<(&str, Vec<i64>)> = vec![];
        for id in txids {
            let symbol = id.split('-').next().unwrap_or_default();
            let ext = crate::ruby::to_i(id.rsplit('-').next().unwrap_or_default());
            match by_symbol.iter_mut().find(|(s, _)| *s == symbol) { Some((_, v)) => v.push(ext), None => by_symbol.push((symbol, vec![ext])) }
        }
        let mut found: Vec<(Order, String)> = vec![];
        for (symbol, mut wanted) in by_symbol {
            let mut from_id = wanted.iter().copied().min().unwrap_or_default();
            let mut exhausted = true;
            for _ in 0..PAGES {
                let r = self.signed_now("GET", "/api/v3/allOrders", vec![("symbol", symbol.to_string()), ("orderId", from_id.to_string()),
                                        ("limit", PAGE.to_string()), ("recvWindow", READ_RECV_WINDOW.into())]);
                let body = self.call(&r, false).await?;
                let page = body.as_array().ok_or_else(unreadable)?;
                let mut last = None;
                for raw in page {
                    let ext = raw["orderId"].as_i64().ok_or_else(unreadable)?;
                    last = last.max(Some(ext));
                    if !wanted.contains(&ext) { continue; }
                    let id = format!("{symbol}-{ext}");
                    found.push((self.parse_order(&id, raw)?, id));
                    wanted.retain(|w| *w != ext);
                }
                if wanted.is_empty() || page.len() < PAGE { exhausted = false; break; }
                from_id = wanted.iter().copied().min().unwrap_or_default().max(last.unwrap_or_default() + 1);
            }
            if exhausted { return Err(super::VenueError::Rejected(vec![format!("Too many pages of Binance {symbol} orders")])); }
        }
        // Rails rebuilds after every symbol's walk, in the order the orders were found.
        let mut out = vec![];
        for (order, id) in found { out.push(self.settle(order, &id).await?); }
        Ok(out)
    }

    /// #get_order (Bot::FetchAndUpdateOrderJob): GET /api/v3/order by symbol and orderId, rebuilt from its trades when it
    /// executed. Binance states no identity to check: the callback is Alpaca's.
    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), super::VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, super::VenueError> {
        let mut parts = txid.split('-');
        let (symbol, ext) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        let r = self.signed_now("GET", "/api/v3/order", vec![("symbol", symbol.to_string()), ("orderId", ext.to_string()), ("recvWindow", READ_RECV_WINDOW.into())]);
        let body = self.call(&r, false).await?;
        let order = self.parse_order(txid, &body)?;
        Ok((vec![self.settle(order, txid).await?], vec![]))
    }
    /// GET /api/v3/order by the intent's symbol and origClientOrderId (spec E §2.6), read with the read recvWindow. A found
    /// order is the intent's only when the answer names that symbol and that client order id; it is then read as #get_order
    /// reads one (its fill rebuilt from its trades). Any other answer, -2013 "Order does not exist." included, is incomplete:
    /// that answer for an absent order is not yet confirmed by a recorded live answer (ABSENCE_UNPROVEN).
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, super::VenueError> {
        let r = self.signed_now("GET", "/api/v3/order", vec![("symbol", symbol.to_string()), ("origClientOrderId", cl_ord_id.to_string()),
                                                          ("recvWindow", READ_RECV_WINDOW.into())]);
        let body = match self.call(&r, false).await {
            Ok(body) => body,
            Err(super::VenueError::Rejected(e)) if e.iter().any(|m| m == "Order does not exist.") => return Err(super::VenueError::Ambiguous(ABSENCE_UNPROVEN.into())),
            Err(e) => return Err(e),
        };
        let ext = match &body["orderId"] { serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(), _ => return Err(unreadable()) };
        if body["symbol"].as_str() != Some(symbol) || body["clientOrderId"].as_str() != Some(cl_ord_id) {
            return Err(super::VenueError::Rejected(vec![format!("Binance answered for client order id {cl_ord_id} on {symbol} with another order")]));
        }
        let id = format!("{symbol}-{ext}");
        let order = self.parse_order(&id, &body)?;
        self.settle(order, &id).await.map(Some)
    }
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

    /// The base's free balance for a seller: the same account read, the row whose venue code is the ticker's base code (the
    /// code #asset_from_symbol resolves to the ticker's base asset). The last such row wins, absent is 0.
    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, super::VenueError> { self.free_of(&ticker.base_code).await }

    /// #cancel_order through Clients::Binance#cancel_order: DELETE /api/v3/order by the stored "SYMBOL-<orderId>", with the
    /// gem's default recvWindow; a failure is #parse_error_message's text.
    async fn cancel(&self, txid: &str) -> Result<(), super::VenueError> {
        let mut parts = txid.split('-');
        let (symbol, ext) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        let r = self.signed_now("DELETE", "/api/v3/order", vec![("symbol", symbol.to_string()), ("orderId", ext.to_string()), ("recvWindow", PLACE_RECV_WINDOW.into())]);
        self.call(&r, false).await.map(|_| ())
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
