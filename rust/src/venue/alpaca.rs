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

pub const PAPER_TRADING_URL: &str = "https://paper-api.alpaca.markets";
pub const DATA_URL: &str = "https://data.alpaca.markets";

#[derive(Clone, Debug, PartialEq)]
pub struct Urls { pub trading: String, pub data: String }

impl Urls {
    /// Exchanges::Alpaca#paper_mode? picks the live host for exactly "live". Before 3.0 this build is paper only: every
    /// passphrase gets the paper host, and the live host is not in the binary (LiveFactory and preflight refuse "live").
    pub fn for_passphrase(_passphrase: Option<&str>) -> Self {
        Self { trading: PAPER_TRADING_URL.into(), data: DATA_URL.into() }
    }
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
        quote_amount_exec: &filled_qty * &filled_avg_price.unwrap_or_else(BigDec::zero), amount_exec: filled_qty,
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

#[derive(Clone)]
pub struct AlpacaVenue<T: Transport> { transport: T, urls: Urls }

impl<T: Transport> AlpacaVenue<T> {
    pub fn new(transport: T, urls: Urls) -> Self { Self { transport, urls } }
    pub fn urls(&self) -> &Urls { &self.urls }

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
            Ok(resp) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
        }
    }
}

impl<T: Transport> Venue for AlpacaVenue<T> {
    fn rules(&self) -> &'static VenueRules { &ALPACA }

    /// Exchanges::Alpaca#get_ask_price / #get_last_price. Crypto: the latest quote's `ap` / trade's `p` by pair in the
    /// `symbols` query. A stock or ETF: Clients::Alpaca#get_latest_quote / #get_latest_trade, the BASE in the path, `quote.ap`
    /// / `trade.p`, no feed parameter (clients/alpaca.rb:172-192). A zero or missing price is Rails' "Wrong … price" raise.
    /// Clients::Alpaca#get_clock on the trading host, with the bot's key.
    async fn positions(&self) -> Result<std::collections::HashMap<String, BigDec>, VenueError> {
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

    async fn clock(&self) -> Result<ClockAnswer, VenueError> {
        let r = self.request("GET", false, "/v2/clock".into(), vec![], None);
        match self.transport.send(&r).await {
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            Err(TransportError::Permanent(m)) => Ok(ClockAnswer::Failed { status: None, message: m }),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(ClockAnswer::Body(resp.body)),
            Ok(resp) => Ok(ClockAnswer::Failed { status: Some(resp.status), message: error_message(&r, &resp) }),
        }
    }

    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let label = match side { PriceSide::Ask => "ask", PriceSide::Last => "last" };
        let raw = if ticker.crypto {
            let (path, key, field) = match side {
                PriceSide::Ask => ("/v1beta3/crypto/us/latest/quotes", "quotes", "ap"),
                PriceSide::Last => ("/v1beta3/crypto/us/latest/trades", "trades", "p"),
            };
            let body = self.get(self.request("GET", true, path.into(), vec![("symbols", ticker.ticker.clone())], None)).await?;
            body[key][ticker.ticker.as_str()][field].clone()
        } else {
            let (path, key, field) = match side { PriceSide::Ask => ("quotes", "quote", "ap"), PriceSide::Last => ("trades", "trade", "p") };
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
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut body = json!({ "symbol": o.pair, "side": "buy", "type": "market", "time_in_force": if o.day { "day" } else { "gtc" } });
        match &o.kind {
            OrderKind::Market if o.quote_volume => { body["notional"] = json!(o.volume); }
            OrderKind::Market => { body["qty"] = json!(o.volume); }
            OrderKind::Limit { price } => { body["type"] = json!("limit"); body["qty"] = json!(o.volume); body["limit_price"] = json!(price); }
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

    /// Without transaction context every requested order retains strict parsing (e.g. placement recovery).
    async fn orders(&self, ids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        self.orders_identified(ids, |_, _, _| Ok(true)).await.map(|(orders, _)| orders)
    }

    /// #get_orders: identity exclusion precedes all status/number parsing, as in Rails.
    async fn orders_identified<F>(&self, ids: &[String], mut identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
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
    async fn order_by_client_id(&self, cl_ord_id: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
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
    async fn fills_from_trades(&self, _txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { Ok(vec![]) }

    /// #get_balances (account, then positions; either failure is the answer) and #spendable_balance for an all-crypto
    /// bot: non_marginable_buying_power, else cash (`&.to_d`, so only an absent field falls back).
    async fn balance(&self, asset_symbol: &str, all_crypto: bool) -> Result<BigDec, VenueError> {
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

/// Before 3.0 the engine trades Alpaca paper only.
pub const LIVE_REFUSED: &str = "live Alpaca trading is not enabled in this build (paper only before 3.0)";

/// The real venues by exchange type. This build connects Alpaca paper only; anything else gets a venue that sends nothing
/// (`run`'s preflight refuses such bots before it claims the install, so this is the second line). A passphrase that does
/// not decrypt never reaches here: `model::credentials_for` fails that bot's tick (and preflight refuses it).
#[derive(Clone)]
pub struct LiveFactory { client: reqwest::Client }

impl LiveFactory {
    pub fn new() -> Self { Self { client: http::client() } }
}

impl Default for LiveFactory {
    fn default() -> Self { Self::new() }
}

impl VenueFactory for LiveFactory {
    type V = AlpacaVenue<ReqwestTransport>;
    fn for_bot(&self, exchange_type: &str, credentials: Option<Credentials>) -> Self::V {
        let live = credentials.as_ref().is_some_and(|c| c.passphrase.as_deref() == Some("live"));
        let transport = match (exchange_type, credentials) {
            ("Exchanges::Alpaca", _) if live => ReqwestTransport::refused(LIVE_REFUSED),
            ("Exchanges::Alpaca", Some(c)) => ReqwestTransport::new(self.client.clone(), c.key, c.secret),
            // Rails' unsaved fallback key: Alpaca answers 401 and the tick fails, per bot.
            ("Exchanges::Alpaca", None) => ReqwestTransport::new(self.client.clone(), String::new(), String::new()),
            (other, _) => ReqwestTransport::refused(&format!("{other} is not connected in this build")),
        };
        // Every key that gets this far is paper (anything but exactly "live"), so the live host is never even named.
        AlpacaVenue::new(transport, Urls::for_passphrase(None))
    }
}

/// What `deltabadger run` refuses before it claims anything: anything `check` refuses, and then every bot the engine will
/// call a venue for — each eligible bot, and each bot of ANY status with an outstanding order or a placement intent (the
/// loop polls and reconciles those too) — that is on a venue this build cannot reach, has no key, has a key this
/// SECRET_KEY_BASE cannot decrypt, or has a live key.
pub fn preflight(c: &Connection, cipher: &Cipher) -> Result<Vec<i64>, Vec<String>> {
    let report = eligibility::check_install(c).map_err(|e| vec![format!("{e:?}")])?;
    let mut problems = report.problems.clone();
    problems.extend(report.unreadable.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})")));
    let mut ids = report.eligible.clone();
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
    if exchange != ALPACA.exchange_type { return Ok(Some(format!("{exchange} is not connected in this build (Alpaca paper only)"))); }
    Ok(match model::credentials_for(c, cipher, &bot)? {
        None => Some("no Alpaca API key".into()),
        Some(k) if k.passphrase.as_deref() == Some("live") => Some(LIVE_REFUSED.into()),
        Some(_) => None,
    })
}
