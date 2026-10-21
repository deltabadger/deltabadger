//! Honeymaker::Clients::Hyperliquid (with hyperliquid-rb 0.1.1 for orders and cancels) and Exchanges::Hyperliquid for the
//! engine's spot orders (plan VB slice B1): prices, balances, placement, polls, cancel, the gone-order recovery from the
//! fills and the recovery by cloid. Rails is the oracle (post-audit, AUDIT-hyperliquid H1-H8); each method names the Ruby it
//! ports. `venue::live` builds it for every Hyperliquid bot.
//!
//! Every read is a POST to /info whose JSON body names its `type` and the account's address (the API key). An order or a
//! cancel is a POST to /exchange whose body here is `{action}`: the transport (`sign::Wire::Hyperliquid`) mints the nonce
//! from the process-wide monotonic source (Honeymaker::Clients::Hyperliquid.next_nonce, `signers::hyperliquid::NONCE`) and
//! signs the action with the agent key at send time, as honeymaker's `build_exchange` wrapper drives hyperliquid-rb's
//! `post_action` (no vault, no expiry, through ExchangeProxy.for('hyperliquid') like every other request).
//!
//! Hyperliquid spot has no market order. A market plan reaches the adapter crossed (`OrderKind::Crossed`, D-679, plan VB
//! §1.5): a GTC limit at the price the engine persisted in the intent, its volume as sized (Rails' #set_crossing_order
//! re-reads #market_price_for; GTC, so a miss rests and is polled like any limit order).
//!
//! Where it differs on purpose (plan VB §5, named in tests/parity.rs and tests/hyperliquid.rs):
//! - the order carries a cloid (`0x` + the intent id's 32 hex digits, D9; Rails sends none) and recovery asks orderStatus
//!   for it, which can only FIND an order (absence is never concluded);
//! - the asset id comes from one spotMeta read (hyperliquid-rb's Info also reads meta and perpDexs, which name no spot
//!   pair); a failed read before the order sends nothing;
//! - a cancel addresses the pair orderStatus names for the order (Rails reads it from the order's row, H4), and is a
//!   success only when Hyperliquid says so (Rails takes any HTTP 200 as cancelled, an `err` answer included:
//!   D-HL-CANCEL-REFUSED);
//! - a placement that may have reached Hyperliquid (a 5xx, a lost reply, an answer that is not JSON, or whose status is
//!   neither "ok" nor "err", or an "ok" without statuses or an oid) keeps the intent: only an answer that proves the refusal
//!   (`err`, a status error, a 4xx) clears it; a number that is no decimal is refused where Rails reads it as 0.
use super::binance::Pairs;
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, HYPERLIQUID as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub const URL: &str = "https://api.hyperliquid.xyz";
pub const HYPERLIQUID: &str = "Exchanges::Hyperliquid";
pub const INFO: &str = "/info";
pub const EXCHANGE: &str = "/exchange";
pub const UNREADABLE: &str = "Hyperliquid: unreadable response";
/// Exchanges::Hyperliquid::TIME_PAGE_LIMITS[:user_fills_by_time], and TIME_PAGE_LIMIT for every other time-ranged read.
pub const FILLS_PAGE: usize = 2000;
pub const TIME_PAGE: usize = 500;
/// Honeymaker::Clients::Hyperliquid::FILLS_PAGES: the cap on one order's cost walk.
const FILLS_PAGES: usize = 25;
/// ponytail: Rails' read_by_time has no page cap; the venue serves at most the 10000 most recent fills, so this many pages
/// only stops a cursor that never ends.
const MAX_TIME_PAGES: usize = 10_000;
pub const NO_STATUSES: &str = "Hyperliquid order failed: no statuses returned";
pub const NO_OID: &str = "Hyperliquid order failed: no order ID returned";

#[derive(Clone)]
pub struct HyperliquidVenue<T: Transport> { transport: T, base: String, user: String, pairs: Pairs, identity: Option<Value> }

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }
/// `x.to_d` / `BigDecimal((x || "0").to_s)` of a field that may be absent; present and unreadable refuses the whole answer.
fn or_zero(v: &Value) -> Result<BigDec, VenueError> { if v.is_null() { Ok(BigDec::zero()) } else { required(v) } }
/// `#to_s` of a scalar, as Ruby interpolates it (nil is "").
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }

/// Ruby's String#inspect.
fn ruby_string(s: &str) -> String {
    let mut out = String::from("\"");
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"), '\n' => out.push_str("\\n"), '\t' => out.push_str("\\t"), '\r' => out.push_str("\\r"),
            '\x1b' => out.push_str("\\e"), '\x07' => out.push_str("\\a"), '\x08' => out.push_str("\\b"), '\x0b' => out.push_str("\\v"), '\x0c' => out.push_str("\\f"),
            '#' if matches!(chars.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Ruby's #inspect (and #to_s) of what JSON.parse made of `v`, as "Hyperliquid order failed: #{response}" interpolates it.
pub fn inspect(v: &Value) -> String {
    match v {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => if n.is_f64() { crate::ruby::float_to_s(n.as_f64().unwrap_or_default()) } else { n.to_string() },
        Value::String(s) => ruby_string(s),
        Value::Array(a) => format!("[{}]", a.iter().map(inspect).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!("{{{}}}", m.iter().map(|(k, v)| format!("{} => {}", ruby_string(k), inspect(v))).collect::<Vec<_>>().join(", ")),
    }
}

/// Exchanges::Hyperliquid#parse_order_id: "<base>-<oid>", the oid what follows the LAST dash.
pub fn split_order_id(order_id: &str) -> (&str, &str) { order_id.rsplit_once('-').unwrap_or(("", order_id)) }

/// The cloid an intent is sent with (Cloid#to_raw): `0x` and 16 bytes of hex, the intent's UUID without its dashes.
pub fn cloid(cl_ord_id: &str) -> Option<String> {
    let hex: String = cl_ord_id.chars().filter(|c| *c != '-').collect::<String>().to_lowercase();
    (hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("0x{hex}"))
}

/// Honeymaker::Clients::Hyperliquid#parse_order_status, suffix-aware: the whole cancel and reject family is cancelled; a
/// triggered order rests.
pub fn order_status(status: &str) -> OrderStatus {
    match status {
        "filled" => OrderStatus::Closed,
        "open" | "triggered" => OrderStatus::Open,
        s if s.to_lowercase().contains("cancel") || s.to_lowercase().contains("reject") => OrderStatus::Cancelled,
        _ => OrderStatus::Unknown,
    }
}

/// `fill['sz'].to_d` and `fill['px'].to_d` summed as #order_data_from_fills / #order_status' cost sum them.
fn fills_sums(fills: &[Value]) -> Result<(BigDec, BigDec), VenueError> {
    let (mut size, mut quote) = (BigDec::zero(), BigDec::zero());
    for f in fills {
        let sz = required(&f["sz"])?;
        quote = &quote + &(&required(&f["px"])? * &sz);
        size = &size + &sz;
    }
    Ok((size, quote))
}

/// Exchanges::Hyperliquid#order_data_from_fills: the executed size and cost of the order's fills, their ratio the price
/// (none without a size), the first fill's side and pair, a limit order, closed.
pub fn order_from_fills(order_id: &str, fills: &[Value]) -> Result<OrderState, VenueError> {
    let (amount, quote) = fills_sums(fills)?;
    let price = if amount.is_positive() { quote.div(&amount) } else { None };
    Ok(OrderState { txid: order_id.into(), status: OrderStatus::Closed, price, amount: None, quote_amount: None, amount_exec: amount, quote_amount_exec: Some(quote),
                    limit: true, sell: fills[0]["side"] != "B", pair: fills[0]["coin"].as_str().map(str::to_string), asset_class: None })
}

/// The top of one side of an l2Book answer (`levels` = [bids, asks]): its `px`, or None for a side with no orders.
fn top(body: &Value, side: usize) -> Result<Option<BigDec>, VenueError> {
    match body["levels"].get(side).and_then(|levels| levels.get(0)) {
        None | Some(Value::Null) => Ok(None),
        Some(level) => or_zero(&level["px"]).map(Some),
    }
}

impl<T: Transport> HyperliquidVenue<T> {
    /// `user` is the account's address, the key Rails names every read with (`api_key.key`).
    pub fn new(transport: T, base: &str, user: &str) -> Self { Self { transport, base: base.into(), user: user.into(), pairs: Pairs::default(), identity: None } }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// The exchange's tickers (`Pairs::load`): pair names to base and quote, venue codes to assets.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    fn post(&self, path: &str, body: Value) -> HttpRequest {
        HttpRequest { method: "POST", base: self.base.clone(), path: path.into(), query: vec![], body: Some(body), not_after: None }
    }

    /// A POST to /info with this body (the key check's one read).
    pub fn info_request(&self, body: Value) -> HttpRequest { self.post(INFO, body) }

    /// Honeymaker::Clients::Hyperliquid#post_info's body: its `type` first, then the request's fields in honeymaker's order.
    pub fn info_body(kind: &str, fields: Vec<(&str, Value)>) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("type".into(), kind.into());
        for (k, v) in fields { body.insert(k.into(), v); }
        Value::Object(body)
    }

    /// One /info read as honeymaker's #with_rescue returns it: a transport failure as the shared split, a non-2xx answer its
    /// body (Faraday's message when empty), a read's 401 `Unauthorized`; a 2xx that is not JSON is unreadable.
    async fn info(&self, kind: &str, fields: Vec<(&str, Value)>) -> Result<Value, VenueError> {
        let r = self.post(INFO, Self::info_body(kind, fields));
        let resp = self.transport.send(&r).await.map_err(|e| http::transport_failure(e, false, |m| self.transport.redact_diagnostic(&m)))?;
        if !(200..300).contains(&resp.status) {
            let message = if resp.body.is_empty() { format!("the server responded with status {} for POST {}", resp.status, r.url()) } else { resp.body };
            return Err(if resp.status == 401 { VenueError::Unauthorized(vec![message]) } else { VenueError::Rejected(vec![message]) });
        }
        http::decode_json(&resp.body).map_err(|_| unreadable())
    }

    /// One /info read for the ledger and balance syncs: the 2xx body of at most `limit` bytes, unparsed (`Ok`), else the
    /// failure's text (the answer's body, or Faraday's or the transport's message).
    pub async fn read_text(&self, kind: &str, fields: Vec<(&str, Value)>, limit: usize) -> Result<String, String> {
        let r = self.post(INFO, Self::info_body(kind, fields));
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for POST {}", resp.status, r.url())),
        }
    }

    /// One /exchange action through hyperliquid-rb's Transport#handle_response as honeymaker's #with_status_rescue returns it.
    /// An HTTP 200 is the parsed answer (not JSON: unreadable, which on a placement may have placed the order). A 4xx is
    /// ClientError "HTTP <status>: <error>" (a refusal: nothing reached the matching engine), a 5xx ServerError (on a placement
    /// the order may have been taken), any other status "Unexpected HTTP <status>". A transport failure is the shared split.
    async fn action(&self, action: Value, placement: bool, not_after: Option<DateTime<Utc>>) -> Result<Value, VenueError> {
        let r = HttpRequest { not_after, ..self.post(EXCHANGE, json!({ "action": action })) };
        let resp = self.transport.send(&r).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))?;
        if resp.status == 200 { return http::decode_json(&resp.body).map_err(|_| unreadable()); }
        // Transport#error_message: a JSON object's `error` (else the object), a JSON string as it is, else the body.
        let error = match http::decode_json(&resp.body) {
            Ok(Value::Object(m)) => m.get("error").filter(|e| !e.is_null()).map_or_else(|| inspect(&Value::Object(m.clone())), to_s),
            Ok(Value::String(s)) => s,
            Ok(other) => inspect(&other),
            Err(_) => resp.body.clone(),
        };
        let message = match resp.status { s @ 400..=599 => format!("HTTP {s}: {error}"), s => format!("Unexpected HTTP {s}") };
        Err(if placement && !(400..500).contains(&resp.status) { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) })
    }

    /// Info#name_to_asset for a spot pair: 10000 + the spotMeta universe entry's own `index` (never its position: the universe
    /// is sparse), found by its name ("@107", "PURR/USDC") or by its tokens' "BASE/QUOTE". Read before anything is sent: a
    /// failed read sends nothing, and a pair Hyperliquid does not list is hyperliquid-rb's "Unknown coin name".
    async fn asset(&self, coin: &str) -> Result<u64, VenueError> {
        let meta = self.info("spotMeta", vec![]).await.map_err(|e| VenueError::Transient(match e {
            VenueError::Rejected(m) | VenueError::Unauthorized(m) => m.join(", "), VenueError::Ambiguous(m) | VenueError::Transient(m) => m }))?;
        let tokens = meta["tokens"].as_array().cloned().unwrap_or_default();
        let name = |i: &Value| i.as_u64().and_then(|i| tokens.get(i as usize)).and_then(|t| t["name"].as_str()).map(str::to_string);
        for entry in meta["universe"].as_array().ok_or_else(|| VenueError::Transient(UNREADABLE.into()))? {
            let pair = entry["tokens"].as_array().and_then(|t| Some(format!("{}/{}", name(t.first()?)?, name(t.get(1)?)?)));
            if entry["name"].as_str() == Some(coin) || pair.as_deref() == Some(coin) {
                return entry["index"].as_u64().map(|i| 10_000 + i).ok_or_else(|| VenueError::Transient(UNREADABLE.into()));
            }
        }
        // hyperliquid-rb's Error, labelled by honeymaker's #with_rescue with its class.
        Err(VenueError::Rejected(vec![format!("Hyperliquid::Error: Unknown coin name: {coin}")]))
    }

    /// Clients::Hyperliquid#spot_balances as #get_balances reads it: one row per coin held, free = total - hold.
    async fn free(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let body = self.info("spotClearinghouseState", vec![("user", self.user.clone().into())]).await?;
        let rows = match &body["balances"] { Value::Null => vec![], Value::Array(rows) => rows.clone(), _ => return Err(unreadable()) };
        let mut free = BigDec::zero();
        for row in rows {
            if !row["coin"].as_str().is_some_and(&pick) { continue; }
            free = &or_zero(&row["total"])? - &or_zero(&row["hold"])?;
        }
        Ok(free)
    }

    /// Exchanges::Hyperliquid#read_by_time: every row from `start_ms` on, oldest first. Each page starts at the newest time of
    /// the one before, so rows sharing that millisecond are read again; identical rows are counted, a row kept as often as the
    /// page that holds it most; a page shorter than `limit` ends the walk, and a full page that brings nothing new fails.
    pub async fn read_by_time(&self, kind: &str, method: &str, start_ms: i64, limit: usize) -> Result<Vec<Value>, VenueError> {
        let mut rows: Vec<(Value, usize)> = vec![];
        let mut from = start_ms;
        for _ in 0..MAX_TIME_PAGES {
            let page = self.info(kind, vec![("user", self.user.clone().into()), ("startTime", from.into())]).await?;
            let page = match page { Value::Array(page) => page, Value::Null => vec![], _ => return Err(unreadable()) };
            let mut fresh = 0;
            for (row, count) in tally(&page) {
                match rows.iter_mut().find(|(r, _)| *r == row) {
                    Some(slot) if slot.1 >= count => continue,
                    Some(slot) => slot.1 = count,
                    None => rows.push((row, count)),
                }
                fresh += 1;
            }
            if page.len() < limit { return Ok(rows.into_iter().flat_map(|(row, count)| std::iter::repeat_n(row, count)).collect()); }
            if fresh == 0 { return Err(VenueError::Rejected(vec![format!("Hyperliquid {method} returned a full page with nothing new")])); }
            from = page.iter().map(|r| time_ms(&r["time"])).max().unwrap_or(from);
        }
        Err(VenueError::Rejected(vec![format!("Too many pages of Hyperliquid {kind}")]))
    }

    /// Clients::Hyperliquid#order_fills: this order's fills from its timestamp on, page after page (each from the newest time
    /// it saw, repeats dropped by tid) until they cover its executed size, a page brings nothing new, or FILLS_PAGES.
    async fn order_fills(&self, oid: &Value, mut start: i64, size: &BigDec) -> Result<Vec<Value>, VenueError> {
        let mut seen: Vec<String> = vec![];
        let mut matched: Vec<Value> = vec![];
        for _ in 0..FILLS_PAGES {
            let page = self.info("userFillsByTime", vec![("user", self.user.clone().into()), ("startTime", start.into())]).await?;
            let page = match page { Value::Array(page) => page, Value::Null => vec![], _ => return Err(unreadable()) };
            let id = |f: &Value| if f["tid"].is_null() { f.to_string() } else { to_s(&f["tid"]) };
            let fresh: Vec<Value> = page.into_iter().filter(|f| !seen.contains(&id(f))).collect();
            if fresh.is_empty() { break; }
            seen.extend(fresh.iter().map(id));
            matched.extend(fresh.iter().filter(|f| to_s(&f["oid"]) == to_s(oid)).cloned());
            if &fills_sums(&matched)?.0 >= size { break; }
            start = fresh.iter().map(|f| time_ms(&f["time"])).max().unwrap_or(start);
        }
        Ok(matched)
    }

    /// Clients::Hyperliquid#order_status as Exchanges::Hyperliquid#build_order_data reads it: None for unknownOid. The answer
    /// is NESTED (`order.order` holds the order, `order.status` its status) and carries no fills: the ordered size is origSz,
    /// the executed one origSz - sz, and its cost the order's own fills (`order_fills`, only when something executed; a failed
    /// read fails), the limit price × executed when the fills no longer hold it. The stored id is the one asked for.
    async fn get_order(&self, order_id: &str, oid: Value) -> Result<Option<OrderState>, VenueError> {
        let raw = self.info("orderStatus", vec![("user", self.user.clone().into()), ("oid", oid.clone())]).await?;
        if raw["status"] == "unknownOid" { return Ok(None); }
        let order = &raw["order"]["order"];
        let limit_price = or_zero(&order["limitPx"])?;
        let ordered = or_zero(&order["origSz"])?;
        let executed = (&ordered - &or_zero(&order["sz"])?).max(BigDec::zero());
        let (mut price, mut quote) = (limit_price.clone(), BigDec::zero());
        if executed.is_positive() {
            let matched = match &order["timestamp"] { Value::Null => vec![], t => self.order_fills(&order["oid"], time_ms(t), &executed).await? };
            let cost = fills_sums(&matched)?.1;
            quote = if cost.is_positive() { cost } else { &limit_price * &executed };
            price = quote.div(&executed).ok_or_else(unreadable)?;
        }
        Ok(Some(OrderState { txid: order_id.into(), status: order_status(raw["order"]["status"].as_str().unwrap_or_default()), price: (!price.is_zero()).then_some(price),
                             amount: Some(ordered), quote_amount: None, amount_exec: executed, quote_amount_exec: Some(quote), limit: true, sell: order["side"] != "B",
                             pair: order["coin"].as_str().map(str::to_string), asset_class: None }))
    }
}

/// `row['time'].to_i`.
fn time_ms(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }

/// `page.tally`, in the page's order: each distinct row (Ruby's Hash#eql?, which serde's map equality matches) and how often.
pub fn tally(page: &[Value]) -> Vec<(Value, usize)> {
    let mut out: Vec<(Value, usize)> = vec![];
    for row in page { match out.iter_mut().find(|(r, _)| r == row) { Some(slot) => slot.1 += 1, None => out.push((row.clone(), 1)) } }
    out
}

impl<T: Transport> super::Venue for HyperliquidVenue<T> {
    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> { self.identity.clone() }

    /// #get_ask_price / #get_bid_price through #get_l2_book (a side with no orders is "Hyperliquid has no asks for …", H3),
    /// and #get_last_price (allMids' entry for the pair, "No price available for …" when it has none). A zero price raises
    /// "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let name = &ticker.ticker;
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.info("l2Book", vec![("coin", name.clone().into())]).await?;
                let (index, label, plural) = if side == PriceSide::Bid { (0, "bid", "bids") } else { (1, "ask", "asks") };
                match top(&body, index)? {
                    Some(price) => (price, label),
                    None => return Err(VenueError::Rejected(vec![format!("Hyperliquid has no {plural} for {name}")])),
                }
            }
            PriceSide::Last => {
                let mids = self.info("allMids", vec![]).await?;
                match &mids[name.as_str()] {
                    Value::Null => return Err(VenueError::Rejected(vec![format!("No price available for {name}")])),
                    v => match required(v)? { p if p.is_zero() => return Err(VenueError::Rejected(vec![format!("No price available for {name}")])), p => (p, "last") },
                }
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {name}: {}", price.to_s_f())])); }
        Ok(price)
    }

    /// #set_crossing_order / #set_limit_order through Clients::Hyperliquid#order → Exchange#order: one GTC limit order
    /// (`{a, b, p, s, r: false, t: {limit: {tif: "Gtc"}}}`, p and s through float_to_wire), a crossed (market) order at its
    /// persisted price, plus `c`, the cloid (D9). Sent at most once. "ok" with the order resting or filled is its oid, stored
    /// as "<base>-<oid>"; an `err` answer or a status error is the refusal Rails writes ("Hyperliquid order failed: …").
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let price = match &o.kind {
            OrderKind::Crossed { price } | OrderKind::Limit { price } => price,
            // A crossing venue's market plan always carries its crossed price (placement::send_to_venue refuses one without).
            OrderKind::Market => return Err(VenueError::Transient("Hyperliquid has no market order and no crossed price was prepared; not sent".into())),
        };
        // Exchanges::Hyperliquid#minimum_amount_logic is :base_and_quote_in_base: an order is always a base amount.
        if o.quote_volume { return Err(VenueError::Transient("Hyperliquid takes no quote-sized order; not sent".into())); }
        let Some(base) = self.pairs.by_symbol.get(&o.pair).map(|p| p.base.clone()) else { return Err(VenueError::Transient(format!("Hyperliquid pair {} is not listed here; not sent", o.pair))) };
        let Some(cloid) = cloid(&o.cl_ord_id) else { return Err(VenueError::Transient(format!("client order id {} is no cloid; not sent", o.cl_ord_id))) };
        // Utils.float_to_wire raises on a value a double cannot carry: a refused order, never re-rounded (plan VB §1.5).
        let wire = |v: &str| BigDec::parse(v).map_err(|_| format!("unreadable {v}")).and_then(|d| super::signers::hyperliquid::float_to_wire(&d)).map_err(|m| VenueError::Rejected(vec![m]));
        let (p, s) = (wire(price)?, wire(&o.volume)?);
        let asset = self.asset(&o.pair).await?;
        let order = json!({ "a": asset, "b": o.side == super::Side::Buy, "p": p, "s": s, "r": false, "t": { "limit": { "tif": "Gtc" } }, "c": cloid });
        let action = json!({ "type": "order", "orders": [order], "grouping": "na" });
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let not_after = o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s");
        let response = self.action(action, true, Some(not_after)).await?;
        if !response.is_object() { return Err(unreadable()); }
        // Only Hyperliquid's own `err` proves the order was refused; an answer with any other status says nothing about it.
        if response["status"] == "err" { return Err(VenueError::Rejected(vec![format!("Hyperliquid order failed: {}", inspect(&response))])); }
        if response["status"] != "ok" { return Err(VenueError::Ambiguous(format!("Hyperliquid order failed: {}", inspect(&response)))); }
        // From here the venue said "ok": a missing acknowledgement is not a rejection.
        let status = match &response["response"]["data"]["statuses"] { Value::Array(s) if !s.is_empty() => s[0].clone(), _ => return Err(VenueError::Ambiguous(NO_STATUSES.into())) };
        if !matches!(&status["error"], Value::Null) && !crate::ruby::blank(&to_s(&status["error"])) {
            return Err(VenueError::Rejected(vec![format!("Hyperliquid order failed: {}", to_s(&status["error"]))]));
        }
        let oid = [&status["resting"]["oid"], &status["filled"]["oid"]].into_iter().find(|v| !v.is_null()).map(to_s);
        oid.map(|oid| format!("{base}-{oid}")).ok_or_else(|| VenueError::Ambiguous(NO_OID.into()))
    }

    /// #get_orders: #get_order per id (orderStatus by its oid, `oid.to_i`); an unknownOid is left out (`missing`, which
    /// polling settles from the fills, `fills_from_trades`); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids {
            if let Some(o) = self.get_order(id, crate::ruby::to_i(split_order_id(id).1).into()).await? { out.push(o); }
        }
        Ok(out)
    }

    /// Recovery (plan VB §1.5) may only FIND the order: orderStatus by the cloid the intent sent. An order naming that cloid
    /// and pair is authoritative; unknownOid proves nothing (the intent waits).
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let Some(cloid) = cloid(cl_ord_id) else { return Ok(None) };
        let Some(base) = self.pairs.by_symbol.get(symbol).map(|p| p.base.clone()) else { return Ok(None) };
        let raw = self.info("orderStatus", vec![("user", self.user.clone().into()), ("oid", cloid.clone().into())]).await?;
        if raw["status"] == "unknownOid" { return Ok(None); }
        let order = &raw["order"]["order"];
        if order["cloid"].as_str() != Some(cloid.as_str()) || order["coin"].as_str() != Some(symbol) {
            return Err(VenueError::Rejected(vec![format!("Hyperliquid answered for cloid {cloid} on {symbol} with another order")]));
        }
        let oid = order["oid"].clone();
        if oid.is_null() { return Err(unreadable()); }
        self.get_order(&format!("{base}-{}", to_s(&oid)), oid).await
    }

    /// Exchanges::Hyperliquid#recover_order_from_fills for each order #get_orders named missing: every fill from `since` (the
    /// oldest missing row, less an hour) on, through `read_by_time`, keyed on its oid. A fill books the order closed; no fill
    /// leaves it missing (Bot::StaleOrderResolver decides); a failed read fails.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        if txids.is_empty() { return Ok(vec![]); }
        let fills = self.read_by_time("userFillsByTime", "user_fills_by_time", crate::venue::bitvavo::ruby_ms(since), FILLS_PAGE).await?;
        let mut out = vec![];
        for id in txids {
            let oid = split_order_id(id).1;
            let mine: Vec<Value> = fills.iter().filter(|f| to_s(&f["oid"]) == oid).cloned().collect();
            if !mine.is_empty() { out.push(order_from_fills(id, &mine)?); }
        }
        Ok(out)
    }

    /// #get_balances' free part (total - hold) of the quote asset: each row's coin mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.free(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.free(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.free(|code| code == ticker.quote_code).await }

    /// Exchanges::Hyperliquid#get_tickers_prices: allMids, the listed pairs' mids (Rails caches the answer for a minute; this
    /// reads fresh). Each price is its decimal's `to_s('F')`; a mid that is no decimal is left out (as Binance's and KuCoin's).
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let mids = self.info("allMids", vec![]).await?;
        let mids = mids.as_object().ok_or_else(unreadable)?;
        Ok(mids.iter().filter(|(pair, _)| self.pairs.by_symbol.contains_key(*pair))
            .filter_map(|(pair, v)| crate::ruby::json_to_d(v).ok().flatten().map(|p| (pair.clone(), Value::String(p.to_s_f())))).collect())
    }

    /// #cancel_order through Clients::Hyperliquid#cancel → Exchange#cancel: `{type: cancel, cancels: [{a, o}]}` for the pair
    /// orderStatus names for the order (H4). A success only when Hyperliquid answers "ok" with the cancel's status "success"
    /// (D-HL-CANCEL-REFUSED: Rails takes any HTTP 200 as cancelled).
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let oid = crate::ruby::to_i(split_order_id(txid).1);
        let raw = self.info("orderStatus", vec![("user", self.user.clone().into()), ("oid", oid.into())]).await?;
        let Some(coin) = raw["order"]["order"]["coin"].as_str() else { return Err(VenueError::Rejected(vec![format!("Hyperliquid does not know order {txid}")])) };
        let asset = self.asset(coin).await?;
        let response = self.action(json!({ "type": "cancel", "cancels": [{ "a": asset, "o": oid }] }), false, None).await?;
        if response["status"] == "ok" && response["response"]["data"]["statuses"][0] == "success" { return Ok(()); }
        Err(VenueError::Rejected(vec![format!("Hyperliquid cancel failed: {}", inspect(&response))]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_cloids_statuses_and_inspect_read_as_rails_does() {
        assert_eq!(split_order_id("PURR-123"), ("PURR", "123"));
        assert_eq!(split_order_id("A-B-9"), ("A-B", "9"), "the oid follows the last dash");
        assert_eq!(cloid("0b1d2c3e-4f50-6172-8394-a5b6c7d8e9f0").as_deref(), Some("0x0b1d2c3e4f5061728394a5b6c7d8e9f0"));
        assert_eq!(cloid("not-a-uuid"), None);
        for (s, want) in [("filled", OrderStatus::Closed), ("open", OrderStatus::Open), ("triggered", OrderStatus::Open), ("marginCanceled", OrderStatus::Cancelled),
                          ("rejected", OrderStatus::Cancelled), ("scheduledCancel", OrderStatus::Cancelled), ("odd", OrderStatus::Unknown)] {
            assert_eq!(order_status(s), want, "{s}");
        }
        assert_eq!(inspect(&json!({"status": "err", "response": "User or API Wallet 0x1 does not exist.", "n": 1, "f": 1.5, "b": true, "z": null, "a": [1, {}]})),
                   r#"{"status" => "err", "response" => "User or API Wallet 0x1 does not exist.", "n" => 1, "f" => 1.5, "b" => true, "z" => nil, "a" => [1, {}]}"#);
        assert_eq!(ruby_string("\u{1}a\"\\#{x}é\u{7f}\u{1b}"), r#""\u0001a\"\\\#{x}é\u007F\e""#);
    }

    #[test]
    fn a_tally_counts_identical_rows_in_page_order() {
        let (a, b) = (json!({"time": 1, "x": 1}), json!({"x": 1, "time": 1}));
        assert_eq!(tally(&[a.clone(), json!({"time": 2}), b]), vec![(a, 2), (json!({"time": 2}), 1)], "key order is no difference");
    }
}
