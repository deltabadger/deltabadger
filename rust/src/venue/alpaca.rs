//! Exchanges::Alpaca + Clients::Alpaca for one-asset crypto buys, over a Transport. Rails is the oracle; each method
//! names the Ruby it ports. Market data uses the bot's own key, as Rails' market_data_client does.
use super::http::{self, DecodeError, HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError};
use super::VenueFactory;
use crate::crypto::{Cipher, Credentials};
use crate::engine::{eligibility, model, EngineError};
use rusqlite::Connection;
use super::{ClockAnswer, NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, Venue, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, ALPACA};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub const TRADING_URL: &str = "https://api.alpaca.markets";
pub const PAPER_TRADING_URL: &str = "https://paper-api.alpaca.markets";
pub const DATA_URL: &str = "https://data.alpaca.markets";

/// Where each Alpaca mode goes. Production is `Hosts::alpaca()`; a test points them at local servers, and `Urls::pick`
/// still makes the choice.
#[derive(Clone, Debug, PartialEq)]
pub struct Hosts { pub paper: String, pub live: String, pub data: String }

impl Hosts {
    pub fn alpaca() -> Self { Self { paper: PAPER_TRADING_URL.into(), live: TRADING_URL.into(), data: DATA_URL.into() } }
    /// One local server for all three (a test that does not tell the modes apart).
    pub fn all(url: &str) -> Self { Self { paper: url.into(), live: url.into(), data: url.into() } }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Urls { pub trading: String, pub data: String }

impl Urls {
    /// Exchanges::Alpaca#paper_mode? and Clients::Alpaca: the live trading host for exactly "live" (nil and anything else
    /// is paper); market data on its own host whatever the mode. The only place a mode becomes a host.
    pub fn pick(passphrase: Option<&str>, hosts: &Hosts) -> Self {
        Self { trading: if passphrase == Some("live") { hosts.live.clone() } else { hosts.paper.clone() }, data: hosts.data.clone() }
    }
    pub fn for_passphrase(passphrase: Option<&str>) -> Self { Self::pick(passphrase, &Hosts::alpaca()) }
}

/// `value.to_d` on a JSON-parsed Ruby value: nil → 0, Integer exact, Float#to_d, a decimal String.
/// DIVERGES from Ruby on purpose (ruby::json_to_d): "garbage".to_d is 0 and "NaN".to_d is NaN in Ruby; here anything
/// that is not a finite decimal within BigDec's bounds is `Err` with the raw value quoted, so it can never become a zero
/// price, quantity, fill or balance. Each caller turns it into that call's unreadable-answer error.
pub fn ruby_to_d(v: &Value) -> Result<BigDec, String> { ruby_opt_to_d(v).map(|d| d.unwrap_or_else(BigDec::zero)) }

/// `value&.to_d`: nil stays nil.
pub fn ruby_opt_to_d(v: &Value) -> Result<Option<BigDec>, String> { crate::ruby::json_to_d(v).map_err(|_| crate::ruby::raw(v)) }

/// Clients::Alpaca#with_rescue's message for a failed request: "HTTP <status>" for an HTML body, else the JSON body's
/// `message`, else the raw body, else Faraday's own "the server responded with status …" (raise_error, empty body).
pub fn error_message(r: &HttpRequest, resp: &HttpResponse) -> String {
    let body = resp.body.as_str();
    if body.trim().is_empty() { return format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()); }
    let lower = body.to_ascii_lowercase();
    if lower.match_indices('<').any(|(i, _)| lower[i + 1..].trim_start().starts_with("html")) { return format!("HTTP {}", resp.status); }
    // Through the venue decode like every body: one with an out-of-range number is quoted raw, never parsed into a message.
    match http::decode_json(body).ok().as_ref().and_then(|v| v.get("message")) {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(v) if !v.is_null() && !v.is_string() => v.to_string(),
        _ => body.to_string(),
    }
}

/// What an unreadable 2xx answer reports: the out-of-range number, or Rails' message for a body that is not JSON.
fn unreadable(e: DecodeError, r: &HttpRequest, resp: &HttpResponse) -> String {
    match e { DecodeError::OutOfRange(m) => m, DecodeError::NotJson => error_message(r, resp) }
}

/// Alpaca's own "order not found" answer: code 40410000 and a message starting "order not found".
/// ABSENCE RULE: only this envelope proves an order absent. Every other 404 (an empty object, a gateway's route error,
/// HTML, the right code with another message) proves nothing and stays Pending, so the intent stays (fail closed). The live check (Task 11) asserts this envelope against the real paper API;
/// if Alpaca's real envelope differs, this matcher follows the recording, and until then every lookup stays Pending.
pub fn alpaca_not_found(body: &str) -> bool {
    // An envelope with an out-of-range number anywhere is unreadable (http::decode_json), so it never proves absence.
    http::decode_json(body).is_ok_and(|v| v["code"] == 40_410_000 && v["message"].as_str().is_some_and(|m| m.starts_with("order not found")))
}

/// Exchanges::Alpaca#parse_order_status.
fn status(s: Option<&str>) -> OrderStatus {
    match s {
        Some(
            "new" | "accepted" | "pending_new" | "partially_filled" | "held" | "accepted_for_bidding" | "pending_cancel"
            | "pending_replace" | "done_for_day" | "stopped" | "suspended" | "calculated",
        ) => OrderStatus::Open,
        Some("filled") => OrderStatus::Closed,
        Some("canceled" | "expired" | "replaced") => OrderStatus::Cancelled,
        Some("rejected") => OrderStatus::Failed,
        _ => OrderStatus::Unknown,
    }
}

/// Extract only identity strings while preserving all other fields as raw JSON. No order number is decoded here.
/// Wrong-typed or absent identity fields cannot identify an untracked order. Complete stored IDs need neither field.
pub fn order_identity(raw: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut identity: std::collections::HashMap<String, Box<serde_json::value::RawValue>> = serde_json::from_str(raw).unwrap_or_default();
    let string = |value: Option<Box<serde_json::value::RawValue>>| value.and_then(|v| serde_json::from_str::<String>(v.get()).ok());
    (string(identity.remove("id")), string(identity.remove("symbol")), string(identity.remove("asset_class")))
}

/// Exchanges::Alpaca#parse_order_data. `id` is the id asked for (#get_orders keys its answer by it). Fills are gross.
/// `Err` names the first number it cannot read: the answer is unreadable, never a zero fill.
pub fn parse_order(id: &str, o: &Value) -> Result<OrderState, String> {
    let d = |field: &str| ruby_opt_to_d(&o[field]).map_err(|raw| format!("Alpaca order {id}: unreadable {field} {raw}"));
    let required = |field: &str| d(field)?.ok_or_else(|| format!("Alpaca order {id}: unreadable {field} null"));
    let filled_qty = required("filled_qty")?;
    let filled_avg_price = d("filled_avg_price")?;
    if filled_qty.is_positive() && filled_avg_price.is_none() { required("filled_avg_price")?; }
    // Rails parses every supplied number, even when a positive fill price takes precedence.
    let notional = d("notional")?;
    let qty = d("qty")?;
    let limit_price = d("limit_price")?;
    // `filled_avg_price.positive? ? it : (limit_price || 0)`: an unfilled market order reports price 0, and
    // Transaction#update_with_order_data writes it (0 is not nil, so `.compact` keeps it).
    let price = match &filled_avg_price {
        Some(p) if p.is_positive() => p.clone(),
        _ => limit_price.unwrap_or_else(BigDec::zero),
    };
    Ok(OrderState {
        asset_class: o["asset_class"].as_str().map(str::to_string),
        txid: id.to_string(), status: status(o["status"].as_str()), price: Some(price),
        amount: qty, quote_amount: notional,
        quote_amount_exec: Some(&filled_qty * &filled_avg_price.unwrap_or_else(BigDec::zero)), amount_exec: filled_qty,
        limit: o["type"] == "limit", sell: o["side"] == "sell", pair: o["symbol"].as_str().map(str::to_string),
    })
}

/// Read-side presentation retains absence; placement/polling keep parse_order's persisted zero semantics.
pub fn parse_read_order(id: &str, o: &Value) -> Result<OrderState, String> {
    let mut order = parse_order(id, o)?;
    let filled = ruby_opt_to_d(&o["filled_avg_price"]).map_err(|_| "Unreadable order price".to_string())?;
    let limit = ruby_opt_to_d(&o["limit_price"]).map_err(|_| "Unreadable order price".to_string())?;
    order.price = filled.filter(BigDec::is_positive).or(limit);
    Ok(order)
}

/// An answer `AlpacaVenue::read` fetched and did not parse.
#[derive(Clone, Debug)]
pub struct Body { request: HttpRequest, response: HttpResponse }

impl Body {
    /// The body of a 2xx answer; else Rails' message for the failed request, as `get` gives it.
    pub fn text(&self) -> Result<&str, VenueError> {
        if (200..300).contains(&self.response.status) { Ok(&self.response.body) } else { Err(self.unreadable()) }
    }
    /// What Clients::Alpaca#with_rescue reports for an answer it could not read.
    pub fn unreadable(&self) -> VenueError { VenueError::Rejected(vec![error_message(&self.request, &self.response)]) }
}

/// An authenticated sync handle and every raw reply retain the original ciphertext producer.
pub struct Captured<'a,T:Transport> { venue:&'a AlpacaVenue<T>, origin:crate::engine::model::CredentialVersion }
impl<'a,T:Transport> Captured<'a,T> {
    pub(crate) fn new(venue:&'a AlpacaVenue<T>,origin:crate::engine::model::CredentialVersion)->Self { Self{venue,origin} }
    pub fn origin(&self)->&crate::engine::model::CredentialVersion { &self.origin }
    pub async fn read(&self,data:bool,path:&str,query:Vec<(&'static str,String)>,limit:usize)->crate::engine::model::Produced<Result<Body,VenueError>> {
        let origin=Some(self.origin.clone());
        crate::engine::model::Produced::new(self.venue.read(data,path,query,limit).await,origin)
    }
}
#[derive(Clone)]
pub struct AlpacaVenue<T: Transport> { transport: T, urls: Urls, identity: Option<Value> }

/// What an intent records of the key it is sent under (`placement::begin_with_credentials`), and what recovery compares
/// before it asks anything: the mode (which trading host) and a fingerprint of the public key id. Never the secret.
pub fn placement_identity(credentials: &Credentials) -> Value {
    use sha2::{Digest, Sha256};
    json!({ "alpaca_live": credentials.passphrase.as_deref() == Some("live"), "key_fingerprint": hex::encode(Sha256::digest(credentials.key.as_bytes())) })
}

impl<T: Transport> AlpacaVenue<T> {
    pub fn new(transport: T, urls: Urls) -> Self { Self { transport, urls, identity: None } }
    pub fn urls(&self) -> &Urls { &self.urls }
    /// The key this venue sends with, as `placement_identity` states it (the factory sets it from the same credentials
    /// that picked the host).
    pub fn with_identity(mut self, credentials: &Credentials) -> Self { self.identity = Some(placement_identity(credentials)); self }
    fn redact_error(&self,error:VenueError)->VenueError{
        match error{
            VenueError::Rejected(messages)=>VenueError::Rejected(messages.into_iter().map(|m|self.transport.redact_diagnostic(&m)).collect()),
            VenueError::Unauthorized(messages)=>VenueError::Unauthorized(messages.into_iter().map(|m|self.transport.redact_diagnostic(&m)).collect()),
            VenueError::Ambiguous(message)=>VenueError::Ambiguous(self.transport.redact_diagnostic(&message)),
            VenueError::Transient(message)=>VenueError::Transient(self.transport.redact_diagnostic(&message)),
        }
    }

    fn request(&self, method: &'static str, data_host: bool, path: String, query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        HttpRequest { method, base: if data_host { self.urls.data.clone() } else { self.urls.trading.clone() }, path, query, body, not_after: None }
    }

    /// One authenticated GET on the trading host, or the market-data host, for the tracker's syncs (activities, account,
    /// positions, stock snapshots): the answer of at most `limit` body bytes, unparsed. The caller reads it off the
    /// runtime thread (`Body::text`). `Err` is a transport failure, as `get` maps it; an answer over the limit is one.
    pub async fn read(&self, data_host: bool, path: &str, query: Vec<(&'static str, String)>, limit: usize) -> Result<Body, VenueError> {
        let request = self.request("GET", data_host, path.into(), query, None);
        match self.transport.send_limited(&request, limit).await {
            Err(TransportError::Permanent(m)) => Err(VenueError::Rejected(vec![m])),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            Ok(response) => Ok(Body { request, response }),
        }
    }

    /// Position identities are read before any numeric conversion. Keep transport and structural
    /// failures strict; individual unidentified rows are excluded by the shared position reader.
    async fn raw_positions(&self) -> Result<Vec<crate::sync::balances::Position>, VenueError> {
        let body = self.read(false, "/v2/positions", vec![], crate::sync::balances::MAX_LIST_BYTES).await?;
        match crate::sync::parsed(body, crate::sync::balances::positions).await {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(why)) | Err((why, false)) => Err(VenueError::Rejected(vec![why])),
            Err((why, true)) => Err(VenueError::Transient(why)),
        }
    }

    /// A read as Clients::Alpaca#with_rescue answers it: a 2xx body that parses; else Rejected with Rails' message (an
    /// HTTP failure, a 3xx, an unreadable body, or a permanent transport failure is a Failure Result); a transient transport
    /// failure is Transient (Client.network_failure raises it).
    async fn get(&self, r: HttpRequest) -> Result<Value, VenueError> {
        match self.transport.send(&r).await {
            Err(TransportError::Permanent(m)) => Err(VenueError::Rejected(vec![m])),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            Ok(resp) if (200..300).contains(&resp.status) => http::decode_json(&resp.body).map_err(|e| VenueError::Rejected(vec![unreadable(e, &r, &resp)])),
            Ok(resp) if resp.status == 401 => Err(VenueError::Unauthorized(vec![error_message(&r, &resp)])),
            Ok(resp) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
        }
    }
}

impl<T: Transport> AlpacaVenue<T> {

    async fn positions_diagnostic_source(&self) -> Result<std::collections::HashMap<String, BigDec>, VenueError> {
        let bad = || VenueError::Rejected(vec!["unreadable venue positions".into()]);
        let rows = self.raw_positions().await?;
        let mut out = std::collections::HashMap::new();
        for (_category, symbol, raw) in rows {
            if symbol.is_empty() || symbol.len() > 100 { return Err(bad()); }
            let row = http::decode_json(&raw.text()).map_err(|_| bad())?;
            let qty = ruby_opt_to_d(&row["qty"]).map_err(|_| bad())?.ok_or_else(bad)?;
            if qty < BigDec::zero() || out.insert(symbol.to_string(), qty).is_some() { return Err(bad()); }
        }
        Ok(out)
    }

    async fn clock_diagnostic_source(&self) -> Result<ClockAnswer, VenueError> {
        let r = self.request("GET", false, "/v2/clock".into(), vec![], None);
        match self.transport.send(&r).await {
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            Err(TransportError::Permanent(m)) => Ok(ClockAnswer::Failed { status: None, message: m }),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(ClockAnswer::Body(resp.body)),
            Ok(resp) => Ok(ClockAnswer::Failed { status: Some(resp.status), message: error_message(&r, &resp) }),
        }
    }

    /// Exchanges::Alpaca#get_ask_price / #get_bid_price / #get_last_price. Crypto: the latest quote's `ap` / `bp` / trade's
    /// `p` by pair in the `symbols` query. A stock or ETF: Clients::Alpaca#get_latest_quote / #get_latest_trade, the BASE in
    /// the path, `quote.ap` / `quote.bp` / `trade.p`, no feed parameter. A zero or missing price is Rails' "Wrong … price" raise.
    async fn price_diagnostic_source(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let label = match side { PriceSide::Ask => "ask", PriceSide::Bid => "bid", PriceSide::Last => "last" };
        let raw = if ticker.crypto {
            let (path, key, field) = match side {
                PriceSide::Ask => ("/v1beta3/crypto/us/latest/quotes", "quotes", "ap"),
                PriceSide::Bid => ("/v1beta3/crypto/us/latest/quotes", "quotes", "bp"),
                PriceSide::Last => ("/v1beta3/crypto/us/latest/trades", "trades", "p"),
            };
            let body = self.get(self.request("GET", true, path.into(), vec![("symbols", ticker.ticker.clone())], None)).await?;
            body[key][ticker.ticker.as_str()][field].clone()
        } else {
            let (path, key, field) = match side { PriceSide::Ask => ("quotes", "quote", "ap"), PriceSide::Bid => ("quotes", "quote", "bp"), PriceSide::Last => ("trades", "trade", "p") };
            let body = self.get(self.request("GET", true, format!("/v2/stocks/{}/{path}/latest", ticker.base_code), vec![], None)).await?;
            body[key][field].clone()
        };
        // An unreadable price is an unreadable answer (Rejected, as `get` reads one): the tick retries and places nothing.
        // parse_venue_number(nil) is BigDecimal(""), an ArgumentError: a missing price is unreadable, never zero.
        let price = ruby_opt_to_d(&raw)
            .map_err(|raw| VenueError::Rejected(vec![format!("unreadable {label} price for {}: {raw}", ticker.base_code)]))?
            .ok_or_else(|| VenueError::Rejected(vec![r#"invalid value for BigDecimal(): """#.to_string()]))?;
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.base_code, price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Alpaca#create_order: crypto is always gtc, nils compacted,
    /// Rails' key order, plus Rust's client_order_id (Rails sends none). A 4xx or a permanent transport failure (a TLS
    /// failure while connecting: nothing sent) is a definitive refusal (Rails' failed row); a 3xx, a 5xx, a lost reply, or
    /// a 2xx without a readable id may have placed the order.
    async fn add_order_diagnostic_source(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut body = json!({ "symbol": o.pair, "side": o.side.as_str(), "type": "market", "time_in_force": if o.day { "day" } else { "gtc" } });
        match &o.kind {
            OrderKind::Market if o.quote_volume => { body["notional"] = json!(o.volume); }
            OrderKind::Market => { body["qty"] = json!(o.volume); }
            OrderKind::Limit { price } => { body["type"] = json!("limit"); body["qty"] = json!(o.volume); body["limit_price"] = json!(price); }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("Alpaca")),
        }
        body["client_order_id"] = json!(o.cl_ord_id);
        // The absolute bound: `deadline` is `at` + the send window (SEND_WINDOW_SECONDS == DEADLINE_SECONDS), plus the budget.
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(super::http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.request("POST", false, "/v2/orders".into(), vec![], Some(body)) };
        match self.transport.send(&r).await {
            Err(TransportError::Permanent(m)) => Err(VenueError::Rejected(vec![m])),
            Err(TransportError::NotSent(m)) => Err(VenueError::Transient(m)),
            Err(TransportError::MaybeSent(m)) => Err(VenueError::Ambiguous(m)),
            Ok(resp) if (200..300).contains(&resp.status) => {
                let v = match http::decode_json(&resp.body) {
                    Err(DecodeError::OutOfRange(m)) => return Err(VenueError::Ambiguous(format!("Alpaca accepted the order with an unreadable answer: {m}"))),
                    v => v.unwrap_or(Value::Null),
                };
                let id = v["id"].as_str().filter(|s| !s.is_empty()).map(str::to_string)
                    .ok_or_else(|| VenueError::Ambiguous(format!("Alpaca accepted the order without a readable id: {}", error_message(&r, &resp))))?;
                // A number in the answer it cannot read (a fill of "NaN") makes the whole answer unreadable: the order is
                // resolved by its client order id, never recorded from an answer that may carry a wrong fill. Placement
                // only needs the accepted id; required fill fields belong to the subsequent poll, not this response.
                for field in ["filled_qty", "filled_avg_price", "notional", "qty", "limit_price"] {
                    ruby_opt_to_d(&v[field]).map_err(|raw| VenueError::Ambiguous(format!("Alpaca accepted the order with an unreadable answer: Alpaca order {id}: unreadable {field} {raw}")))?;
                }
                Ok(id)
            }
            Ok(resp) if (400..500).contains(&resp.status) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
            Ok(resp) => Err(VenueError::Ambiguous(error_message(&r, &resp))),
        }
    }

    /// #get_orders: identity exclusion precedes all status/number parsing, as in Rails.
    async fn orders_identified_diagnostic_source<F>(&self, ids: &[String], mut identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        let mut out = Vec::with_capacity(ids.len());
        let mut skipped = vec![];
        for id in ids {
            let request = self.request("GET", false, format!("/v2/orders/{id}"), vec![], None);
            let response = match self.transport.send(&request).await {
                Err(TransportError::Permanent(m)) => return Err(VenueError::Rejected(vec![m])),
                Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => return Err(VenueError::Transient(m)),
                Ok(response) if (200..300).contains(&response.status) => response,
                Ok(response) => return Err(VenueError::Rejected(vec![error_message(&request, &response)])),
            };
            // Validate JSON syntax without converting numeric tokens; identified rows still pass the strict decoder below.
            let raw: Box<serde_json::value::RawValue> = serde_json::from_str(&response.body)
                .map_err(|_| VenueError::Rejected(vec![unreadable(DecodeError::NotJson, &request, &response)]))?;
            let (_, pair, class) = order_identity(raw.get());
            if !identify(id, pair.as_deref(), class.as_deref())? {
                eprintln!("Alpaca order skipped: unsupported class or unmapped/ambiguous order identity");
                skipped.push(id.clone());
                continue;
            }
            let body = http::decode_json(raw.get()).map_err(|e| VenueError::Rejected(vec![unreadable(e, &request, &response)]))?;
            out.push(parse_order(id, &body).map_err(|e| VenueError::Rejected(vec![e]))?);
        }
        Ok((out, skipped))
    }

    /// GET /v2/orders:by_client_order_id: one complete answer. Only Alpaca's own not-found envelope proves absence; any
    /// other 404 (an empty object, a gateway's route error, HTML) and an answer about another client order id prove nothing.
    async fn order_by_client_id_diagnostic_source(&self, cl_ord_id: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let r = self.request("GET", false, "/v2/orders:by_client_order_id".into(), vec![("client_order_id", cl_ord_id.to_string())], None);
        match self.transport.send(&r).await {
            Err(TransportError::Permanent(m)) => Err(VenueError::Rejected(vec![m])),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            // ABSENCE RULE: only Alpaca's own not-found envelope (code 40410000, message starting "order not found") proves
            // the order absent; every other 404 falls through to the last arm and stays Pending.
            Ok(resp) if resp.status == 404 && alpaca_not_found(&resp.body) => Ok(None),
            Ok(resp) if (200..300).contains(&resp.status) => {
                let v: Value = http::decode_json(&resp.body).map_err(|e| VenueError::Rejected(vec![unreadable(e, &r, &resp)]))?;
                match (v["id"].as_str(), v["client_order_id"].as_str()) {
                    (Some(id), Some(cl)) if cl == cl_ord_id => parse_order(id, &v).map(Some).map_err(|e| VenueError::Rejected(vec![e])),
                    _ => Err(VenueError::Rejected(vec![format!("Alpaca answered for client order id {cl_ord_id} with {}", resp.body)])),
                }
            }
            Ok(resp) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
        }
    }

    /// Rails has no trade or activity fallback for Alpaca orders (the activities endpoint feeds only the tracker).
    async fn fills_from_trades_diagnostic_source(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }

    /// #get_balances (account, then positions; either failure is the answer) and #spendable_balance for an all-crypto
    /// bot: non_marginable_buying_power, else cash (`&.to_d`, so only an absent field falls back).
    async fn balance_diagnostic_source(&self, asset_symbol: &str, all_crypto: bool) -> Result<BigDec, VenueError> {
        let account = self.get(self.request("GET", false, "/v2/account".into(), vec![], None)).await?;
        let positions = self.raw_positions().await?;
        for _position in positions {
            eprintln!("Alpaca position excluded from cash funding: only the account cash/buying power funds USD orders");
        }
        // ponytail: eligibility admits only USD-quoted Alpaca bots; another quote would be a position lookup ("BTCUSD").
        if asset_symbol != "USD" { return Ok(BigDec::zero()); }
        // An unreadable balance is never a zero (which would read as low funds). Clients::Alpaca would raise on it, so it is
        // Transient: retried when nothing was placed, rescheduled without a replay when something was (Bot::ActionJob).
        let free = match ruby_opt_to_d(&account[if all_crypto { "non_marginable_buying_power" } else { "buying_power" }]) {
            Ok(Some(d)) => Ok(d),
            Ok(None) => ruby_to_d(&account["cash"]),
            Err(raw) => Err(raw),
        };
        free.map_err(|raw| VenueError::Transient(format!("Alpaca account: unreadable balance {raw}")))
    }
}

impl<T: Transport> AlpacaVenue<T> {
    /// #get_balances(asset_ids: [asset_id]) as Bot::Rebalancer#live_free_balance reads one asset's :free. The account, then
    /// the positions (either failure is the answer); an account with no cash figure, a supported position with no symbol,
    /// or ambiguous USD cash fails it. A position names an asset by class + symbol through the catalog; its free is
    /// `qty_available` (shares on open sell orders are locked), the one USD cash asset's is `cash`, never buying power.
    /// Absent is 0. Rails skips a position naming several assets; here one that may be this asset is an error, never a 0
    /// that would read as no holdings.
    async fn free_of_diagnostic_source(&self, asset_id: i64, catalog: crate::sync::balances::Catalog) -> Result<BigDec, VenueError> {
        let account = self.get(self.request("GET", false, "/v2/account".into(), vec![], None)).await?;
        let mut positions = self.raw_positions().await?;
        let cash = ruby_opt_to_d(&account["cash"]).map_err(|raw| VenueError::Rejected(vec![format!("Alpaca account: unreadable cash {raw}")]))?
            .ok_or_else(|| VenueError::Rejected(vec!["the account has no cash figure".into()]))?;
        let names = |class: &str, symbol: &str| catalog.candidates(class, symbol).is_some_and(|ids| ids.contains(&asset_id));
        if positions.iter().any(|(class, symbol, _)| names(class, symbol) && catalog.candidates(class, symbol).is_some_and(|ids| ids.len() > 1)) {
            return Err(VenueError::Rejected(vec!["an Alpaca position names this asset ambiguously".into()]));
        }
        // Rails reads quantities only for the asset asked for: another position's gap is not this asset's answer.
        positions.retain(|(class, symbol, _)| names(class, symbol));
        let held = crate::sync::balances::complete_balances(&catalog, cash, positions).map_err(|why| VenueError::Rejected(vec![why]))?;
        Ok(held.into_iter().find(|(id, _, _)| *id == asset_id).map_or_else(BigDec::zero, |(_, free, _)| free))
    }
}

impl<T: Transport> Venue for AlpacaVenue<T> {
    async fn free_base_identified<F>(&self, t: &Ticker, catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> {
        self.free_of_diagnostic_source(t.base_asset_id, catalog()?).await.map_err(|error| self.redact_error(error))
    }
    /// Settled cash (`account.cash`), as Bot::Rebalancer reads the quote; `balance` keeps buying power for buys.
    async fn settled_quote_balance<F>(&self, t: &Ticker, catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> {
        self.free_of_diagnostic_source(t.quote_asset_id, catalog()?).await.map_err(|error| self.redact_error(error))
    }

    /// Exchanges::Alpaca#get_tickers_prices: stocks by their snapshots' latest trade, then crypto pairs (a `/` in the code) by
    /// their latest trade, each list sorted; the first failed request is the answer.
    async fn tickers_prices(&self, symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        let mut out = vec![];
        for crypto in [false, true] {
            let mut names: Vec<&str> = symbols.iter().filter(|s| s.contains('/') == crypto).map(String::as_str).collect();
            if names.is_empty() { continue; }
            names.sort_unstable();
            let path = if crypto { "/v1beta3/crypto/us/latest/trades" } else { "/v2/stocks/snapshots" };
            let body = self.get(self.request("GET", true, path.into(), vec![("symbols", names.join(","))], None)).await.map_err(|e| self.redact_error(e))?;
            let entries = if crypto { body.get("trades").cloned().unwrap_or_else(|| json!({})) } else { body };
            for (code, item) in entries.as_object().into_iter().flatten() {
                out.push((code.clone(), if crypto { item["p"].clone() } else { item["latestTrade"]["p"].clone() }));
            }
        }
        Ok(out)
    }
    fn rules(&self)-> &'static VenueRules{&ALPACA}
    fn placement_identity(&self)->Option<Value>{self.identity.clone()}
    async fn positions(&self)->Result<std::collections::HashMap<String, BigDec>,VenueError>{self.positions_diagnostic_source().await.map_err(|error|self.redact_error(error))}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{self.clock_diagnostic_source().await.map_err(|error|self.redact_error(error)) .map(|answer|match answer{ClockAnswer::Failed{status,message}=>ClockAnswer::Failed{status,message:self.transport.redact_diagnostic(&message)},other=>other})}
    async fn price(&self, ticker: &Ticker, side: PriceSide)->Result<BigDec,VenueError>{self.price_diagnostic_source(ticker,side).await.map_err(|error|self.redact_error(error))}
    async fn add_order(&self, o: &NewOrder)->Result<String,VenueError>{self.add_order_diagnostic_source(o).await.map_err(|error|self.redact_error(error))}
    async fn orders(&self, ids: &[String])->Result<Vec<OrderState>,VenueError>{self.orders_identified_diagnostic_source(ids, |_,_,_|Ok(true)).await.map(|(orders,_)|orders).map_err(|error|self.redact_error(error))}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError>
    where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.orders_identified_diagnostic_source(ids,identify).await.map_err(|error|self.redact_error(error))}
    async fn order_by_client_id(&self, cl_ord_id: &str, _symbol: &str, since: DateTime<Utc>)->Result<Option<OrderState>,VenueError>{self.order_by_client_id_diagnostic_source(cl_ord_id,since).await.map_err(|error|self.redact_error(error))}
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{self.fills_from_trades_diagnostic_source(txids,since).await.map_err(|error|self.redact_error(error))}
    async fn balance(&self, asset_symbol: &str, all_crypto: bool)->Result<BigDec,VenueError>{self.balance_diagnostic_source(asset_symbol,all_crypto).await.map_err(|error|self.redact_error(error))}
    /// #cancel_order through Clients::Alpaca#cancel_order: DELETE /v2/orders/<id> on the trading host.
    /// Alpaca answers 204 with no body; any 2xx is success, as with_rescue returns it unparsed.
    async fn cancel(&self, txid: &str)->Result<(),VenueError>{
        let r = self.request("DELETE", false, format!("/v2/orders/{txid}"), vec![], None);
        let answer = match self.transport.send(&r).await {
            Err(TransportError::Permanent(m)) => Err(VenueError::Rejected(vec![m])),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(()),
            Ok(resp) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
        };
        answer.map_err(|error|self.redact_error(error))
    }
}

/// The real venues by exchange type (venue::live::LiveVenue), as Rails' Exchange subclasses: Alpaca (paper or live, as
/// the key says), Kraken and Binance (each through its proxy, ExchangeProxy.for, read each time a venue is built). A venue with no Rust
/// code yet gets a venue that sends nothing; `run`'s preflight refuses such bots before it claims the install, so this is
/// the second line. A passphrase that does not decrypt never reaches here: `model::credentials_for` fails that bot's tick.
#[derive(Clone)]
pub struct LiveFactory { client: reqwest::Client, hosts: Hosts, clock: std::sync::Arc<dyn crate::engine::Clock + Send + Sync>,
    settings: Option<(std::path::PathBuf, Cipher)> }

/// What a factory with no install to read `proxy_<venue>` from answers for a proxied venue: it cannot know whether a proxy
/// is configured, so it never connects directly.
const SETTINGS_UNREAD: &str = "this process has no install to read the venue's proxy setting from (LiveFactory::for_install)";

impl LiveFactory {
    /// Share the deadline producer clock with every real HTTP venue handle.
    pub fn with_clock(mut self,clock:std::sync::Arc<dyn crate::engine::Clock + Send + Sync>)->Self{self.clock=clock;self}
    pub fn new() -> Self { Self { client: http::client(), hosts: Hosts::alpaca(), clock: std::sync::Arc::new(crate::engine::SystemClock), settings: None } }
    /// The engine's factory: each venue it builds reads its proxy from this install's database (`c`'s file) at that moment.
    pub fn for_install(c: &Connection, cipher: &Cipher) -> Self {
        Self { settings: c.path().filter(|p| !p.is_empty()).map(|p| (p.into(), cipher.clone())), ..Self::new() }
    }
    /// A venue's egress now: its client and proxy URL (venue::live::egress), from a fresh read of the install's settings, and
    /// `read` of the same install database (the exchange's tickers) in that one open.
    fn egress_and<R>(&self, name: &str, read: impl FnOnce(&Connection) -> Result<R, String>) -> Result<((reqwest::Client, Option<String>), R), String> {
        let (path, cipher) = self.settings.as_ref().ok_or(SETTINGS_UNREAD)?;
        let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|_| format!("the {name} proxy setting is unreadable"))?;
        let egress = super::live::egress(&c, cipher, &|k| std::env::var(k).ok(), name, &self.client)?;
        Ok((egress, read(&c)?))
    }
    /// Local Alpaca hosts for native production-adapter tests; `Urls::pick` still chooses between them.
    #[doc(hidden)] pub fn with_hosts(hosts:Hosts)->Self{Self{hosts,..Self::new()}}
}

impl Default for LiveFactory {
    fn default() -> Self { Self::new() }
}

impl LiveFactory {
    /// An Alpaca venue for this key (the bot's, or a sync job's). Its host and its identity come from the same
    /// credentials it signs with; no key is Rails' unsaved fallback, paper (`paper_mode?(nil)`).
    pub fn alpaca(&self, credentials: Option<Credentials>, sensitive: Vec<String>) -> AlpacaVenue<ReqwestTransport> {
        let urls = Urls::pick(credentials.as_ref().and_then(|c| c.passphrase.as_deref()), &self.hosts);
        let identity = credentials.as_ref().map(placement_identity);
        let transport = match credentials {
            Some(c) => ReqwestTransport::new(self.client.clone(), c.key, c.secret).with_sensitive_values(c.redaction_values),
            // Rails' unsaved fallback key: Alpaca answers 401 and the tick fails, per bot.
            None => ReqwestTransport::new(self.client.clone(), String::new(), String::new()),
        };
        AlpacaVenue { transport: transport.with_clock(self.clock.clone()).with_sensitive_values(sensitive), urls, identity }
    }
}

impl VenueFactory for LiveFactory {
    type V = super::live::LiveVenue;
    fn for_bot(&self, exchange_type: &str, credentials: Option<Credentials>) -> Self::V {
        self.for_bot_with_redaction(exchange_type,credentials,Vec::new())
    }
    fn for_bot_with_redaction(&self,exchange_type:&str,credentials:Option<Credentials>,sensitive:Vec<String>)->Self::V{
        use super::live::LiveVenue;
        let refused = |why: String| ReqwestTransport::refused(&format!("{exchange_type} is not connected in this build{why}"));
        let unconnected = || LiveVenue::Alpaca(AlpacaVenue::new(refused(String::new()), Urls::for_passphrase(None)));
        let Some(row) = super::registry::row(exchange_type).filter(|r| r.trades) else { return unconnected() };
        if row.exchange_type == crate::sync::ALPACA { return LiveVenue::Alpaca(self.alpaca(credentials, sensitive)); }
        let identity = credentials.clone();
        // Honeymaker.client(<name_id>, proxy: ExchangeProxy.for(<name_id>)), read at each build, and where the adapter reads
        // them the exchange's tickers (#parse_order_data, #parse_trade_data, #asset_from_symbol).
        let pairs = |c: &Connection| -> Result<super::binance::Pairs, String> {
            if !row.needs_pairs { return Ok(super::binance::Pairs::default()); }
            let id: i64 = c.query_row("SELECT id FROM exchanges WHERE type = ?1", [exchange_type], |r| r.get(0))
                .map_err(|_| format!("the {} exchange row is unreadable", row.name))?;
            super::binance::Pairs::load(c, id).map_err(|_| format!("the {} tickers are unreadable", row.name))
        };
        let (key, secret, passphrase, values) = credentials.map(|c| (c.key, c.secret, c.passphrase.unwrap_or_default(), c.redaction_values)).unwrap_or_default();
        let (transport, pairs) = match self.egress_and(row.name_id, pairs) {
            Err(why) => (refused(format!(": {why}")), super::binance::Pairs::default()),
            Ok(((client, proxy), pairs)) => {
                let transport = match exchange_type {
                    "Exchanges::Kraken" => ReqwestTransport::new(client, key, secret.clone()).kraken(),
                    super::kucoin::KUCOIN => ReqwestTransport::kucoin(client, key, secret.clone(), passphrase),
                    super::bitvavo::BITVAVO => ReqwestTransport::bitvavo(client, key, secret.clone()),
                    super::gemini::GEMINI => ReqwestTransport::gemini(client, key, secret.clone()),
                    super::bitget::BITGET => ReqwestTransport::bitget(client, key, secret.clone(), passphrase),
                    _ => ReqwestTransport::binance(client, key, secret.clone()),
                };
                (transport.with_sensitive_values(values.into_iter().chain(proxy).collect()).with_clock(self.clock.clone()), pairs)
            }
        };
        let transport = transport.with_sensitive_values(sensitive);
        match exchange_type {
            "Exchanges::Kraken" => {
                let venue = super::kraken::KrakenVenue::new(transport);
                LiveVenue::Kraken(match &identity { Some(c) => venue.with_identity(c), None => venue })
            }
            super::binance::BINANCE | super::binance::BINANCE_US => {
                let rules = row.rules.expect("a trading row has its rules");
                let venue = super::binance::BinanceVenue::new(transport, row.base, &secret).with_pairs(pairs).with_rules(rules);
                LiveVenue::Binance(match &identity { Some(c) => venue.with_identity(c), None => venue })
            }
            super::kucoin::KUCOIN => {
                let venue = super::kucoin::KucoinVenue::new(transport, row.base).with_pairs(pairs);
                LiveVenue::Kucoin(match &identity { Some(c) => venue.with_identity(c), None => venue })
            }
            super::bitvavo::BITVAVO => {
                let venue = super::bitvavo::BitvavoVenue::new(transport, row.base).with_pairs(pairs);
                LiveVenue::Bitvavo(match &identity { Some(c) => venue.with_identity(c), None => venue })
            }
            super::gemini::GEMINI => {
                let venue = super::gemini::GeminiVenue::new(transport, row.base).with_pairs(pairs);
                LiveVenue::Gemini(match &identity { Some(c) => venue.with_identity(c), None => venue })
            }
            super::bitget::BITGET => {
                let venue = super::bitget::BitgetVenue::new(transport, row.base).with_pairs(pairs);
                LiveVenue::Bitget(match &identity { Some(c) => venue.with_identity(c), None => venue })
            }
            _ => unconnected(),
        }
    }
}

/// What `deltabadger run` refuses before it claims anything: anything `check` refuses, and then every bot the engine will
/// call a venue for — each eligible bot, and each bot of ANY status with an outstanding order or a placement intent (the
/// loop polls and reconciles those too) — that is on a venue this build cannot reach, has no key, has a key this
/// SECRET_KEY_BASE cannot decrypt.
pub fn preflight(c: &Connection, cipher: &Cipher) -> Result<Vec<i64>, Vec<String>> {
    let report = eligibility::check_install(c).map_err(|e| vec![format!("{e:?}")])?;
    let mut problems = report.problems.clone();
    problems.extend(report.unreadable.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})")));
    preflight_after(c, cipher, report.eligible, problems)
}

/// `preflight` for the bots a `check_install` just found eligible, with the problems it found: the guard has its own
/// report in hand, and does not read the whole install a second time.
pub fn preflight_after(c: &Connection, cipher: &Cipher, mut ids: Vec<i64>, mut problems: Vec<String>) -> Result<Vec<i64>, Vec<String>> {
    let mut s = c.prepare(
        "SELECT bot_id FROM transactions WHERE status = 0 AND external_status IN (0, 1) AND bot_id IS NOT NULL \
         UNION SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL ORDER BY 1")
        .map_err(|e| vec![format!("{e:?}")])?;
    let owing = s.query_map([], |r| r.get::<_, i64>(0)).and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).map_err(|e| vec![format!("{e:?}")])?;
    for id in owing { if !ids.contains(&id) { ids.push(id); } }
    for id in &ids {
        match bot_problem(c, cipher, *id) {
            Ok(None) => {}
            Ok(Some(p)) => problems.push(format!("bot {id}: {p}")),
            Err(e) => problems.push(format!("bot {id}: {e:?} (is SECRET_KEY_BASE this instance's own?)")),
        }
    }
    if problems.is_empty() { Ok(ids) } else { Err(problems) }
}

fn bot_problem(c: &Connection, cipher: &Cipher, id: i64) -> Result<Option<String>, EngineError> {
    let bot = model::load_bot(c, id)?;
    let exchange = model::exchange_type(c, &bot)?;
    let Some(rules) = crate::engine::venue_rules::for_exchange(&exchange) else {
        return Ok(Some(format!("{exchange} is not connected in this build (only Kraken, Binance, Binance.US, KuCoin, Bitvavo, Gemini, Bitget and Alpaca)")));
    };
    Ok(match model::credentials_for(c, cipher, &bot)? {
        None => Some(format!("no {} API key", rules.name)),
        Some(_) => None,
    })
}
