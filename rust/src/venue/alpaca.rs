//! Exchanges::Alpaca + Clients::Alpaca for one-asset crypto buys, over a Transport. Rails is the oracle; each method
//! names the Ruby it ports. Market data uses the bot's own key, as Rails' market_data_client does.
use super::http::{self, HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError};
use super::VenueFactory;
use crate::crypto::{Cipher, Credentials};
use crate::engine::{eligibility, model, EngineError};
use rusqlite::Connection;
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, Venue, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, ALPACA};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub const TRADING_URL: &str = "https://api.alpaca.markets";
pub const PAPER_TRADING_URL: &str = "https://paper-api.alpaca.markets";
pub const DATA_URL: &str = "https://data.alpaca.markets";

#[derive(Clone, Debug, PartialEq)]
pub struct Urls { pub trading: String, pub data: String }

impl Urls {
    /// Exchanges::Alpaca#paper_mode?: anything but exactly "live" (nil included) is paper. The data host never changes.
    pub fn for_passphrase(passphrase: Option<&str>) -> Self {
        Self { trading: if passphrase == Some("live") { TRADING_URL } else { PAPER_TRADING_URL }.into(), data: DATA_URL.into() }
    }
}

/// `value.to_d` on a JSON-parsed Ruby value: nil → 0, Integer exact, Float#to_d, String#to_d.
/// ponytail: String#to_d reads the numeric prefix of garbage ("12abc" → 12); this reads garbage as 0. Alpaca sends clean decimals.
pub fn ruby_to_d(v: &Value) -> BigDec {
    match v {
        Value::Number(n) => n.as_i64().map(BigDec::from_i64).or_else(|| n.as_f64().and_then(|f| BigDec::from_f64(f).ok())).unwrap_or_else(BigDec::zero),
        Value::String(s) => BigDec::parse(s).unwrap_or_else(|_| BigDec::zero()),
        _ => BigDec::zero(),
    }
}

/// `value&.to_d`: nil stays nil.
pub fn ruby_opt_to_d(v: &Value) -> Option<BigDec> { (!v.is_null()).then(|| ruby_to_d(v)) }

/// Clients::Alpaca#with_rescue's message for a failed request: "HTTP <status>" for an HTML body, else the JSON body's
/// `message`, else the raw body, else Faraday's own "the server responded with status …" (raise_error, empty body).
pub fn error_message(r: &HttpRequest, resp: &HttpResponse) -> String {
    let body = resp.body.as_str();
    if body.trim().is_empty() { return format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()); }
    let lower = body.to_ascii_lowercase();
    if lower.match_indices('<').any(|(i, _)| lower[i + 1..].trim_start().starts_with("html")) { return format!("HTTP {}", resp.status); }
    match serde_json::from_str::<Value>(body).ok().as_ref().and_then(|v| v.get("message")) {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(v) if !v.is_null() && !v.is_string() => v.to_string(),
        _ => body.to_string(),
    }
}

/// Alpaca's own "order not found" answer: code 40410000 and a message starting "order not found".
/// ABSENCE RULE: only this envelope proves an order absent. Every other 404 (an empty object, a gateway's route error,
/// HTML, the right code with another message) proves nothing and stays Pending, so the intent stays (fail closed). The live check (Task 11) asserts this envelope against the real paper API;
/// if Alpaca's real envelope differs, this matcher follows the recording, and until then every lookup stays Pending.
pub fn alpaca_not_found(body: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|v| v["code"] == 40_410_000 && v["message"].as_str().is_some_and(|m| m.starts_with("order not found")))
}

/// Exchanges::Alpaca#parse_order_status.
fn status(s: Option<&str>) -> OrderStatus {
    match s {
        Some("new" | "accepted" | "pending_new") => OrderStatus::Open,
        Some("filled") => OrderStatus::Closed,
        Some("canceled" | "expired" | "replaced") => OrderStatus::Cancelled,
        Some("rejected") => OrderStatus::Failed,
        _ => OrderStatus::Unknown,
    }
}

/// Exchanges::Alpaca#parse_order_data. `id` is the id asked for (#get_orders keys its answer by it). Fills are gross.
pub fn parse_order(id: &str, o: &Value) -> OrderState {
    let filled_qty = ruby_to_d(&o["filled_qty"]);
    let filled_avg_price = ruby_opt_to_d(&o["filled_avg_price"]);
    // `filled_avg_price.positive? ? it : (limit_price || 0)`: an unfilled market order reports price 0, and
    // Transaction#update_with_order_data writes it (0 is not nil, so `.compact` keeps it).
    let price = match &filled_avg_price {
        Some(p) if p.is_positive() => p.clone(),
        _ => ruby_opt_to_d(&o["limit_price"]).unwrap_or_else(BigDec::zero),
    };
    OrderState {
        txid: id.to_string(), status: status(o["status"].as_str()), price: Some(price),
        amount: ruby_opt_to_d(&o["qty"]), quote_amount: ruby_opt_to_d(&o["notional"]),
        quote_amount_exec: &filled_qty * &filled_avg_price.unwrap_or_else(BigDec::zero), amount_exec: filled_qty,
        limit: o["type"] == "limit", sell: o["side"] == "sell",
    }
}

#[derive(Clone)]
pub struct AlpacaVenue<T: Transport> { transport: T, urls: Urls }

impl<T: Transport> AlpacaVenue<T> {
    pub fn new(transport: T, urls: Urls) -> Self { Self { transport, urls } }
    pub fn urls(&self) -> &Urls { &self.urls }

    fn request(&self, method: &'static str, data_host: bool, path: String, query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        HttpRequest { method, base: if data_host { self.urls.data.clone() } else { self.urls.trading.clone() }, path, query, body, not_after: None }
    }

    /// A read as Clients::Alpaca#with_rescue answers it: a 2xx body that parses; else Rejected with Rails' message (an
    /// HTTP failure, a 3xx, an unreadable body, or a permanent transport failure is a Failure Result); a transient transport
    /// failure is Transient (Client.network_failure raises it).
    async fn get(&self, r: HttpRequest) -> Result<Value, VenueError> {
        match self.transport.send(&r).await {
            Err(TransportError::Permanent(m)) => Err(VenueError::Rejected(vec![m])),
            Err(TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(VenueError::Transient(m)),
            Ok(resp) if (200..300).contains(&resp.status) => serde_json::from_str(&resp.body).map_err(|_| VenueError::Rejected(vec![error_message(&r, &resp)])),
            Ok(resp) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
        }
    }
}

impl<T: Transport> Venue for AlpacaVenue<T> {
    fn rules(&self) -> &'static VenueRules { &ALPACA }

    /// Exchanges::Alpaca#get_ask_price / #get_last_price for a crypto ticker: the latest quote's `ap`, the latest trade's `p`.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let (path, key, field, label) = match side {
            PriceSide::Ask => ("/v1beta3/crypto/us/latest/quotes", "quotes", "ap", "ask"),
            PriceSide::Last => ("/v1beta3/crypto/us/latest/trades", "trades", "p", "last"),
        };
        let body = self.get(self.request("GET", true, path.into(), vec![("symbols", ticker.ticker.clone())], None)).await?;
        let price = ruby_to_d(&body[key][ticker.ticker.as_str()][field]);
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.base_code, price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Alpaca#create_order: crypto is always gtc, nils compacted,
    /// Rails' key order, plus Rust's client_order_id (Rails sends none). A 4xx or a permanent transport failure (a TLS
    /// failure while connecting: nothing sent) is a definitive refusal (Rails' failed row); a 3xx, a 5xx, a lost reply, or
    /// a 2xx without a readable id may have placed the order.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut body = json!({ "symbol": o.pair, "side": "buy", "type": "market", "time_in_force": "gtc" });
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
                let id = serde_json::from_str::<Value>(&resp.body).ok().and_then(|v| v["id"].as_str().filter(|s| !s.is_empty()).map(str::to_string));
                id.ok_or_else(|| VenueError::Ambiguous(format!("Alpaca accepted the order without a readable id: {}", error_message(&r, &resp))))
            }
            Ok(resp) if (400..500).contains(&resp.status) => Err(VenueError::Rejected(vec![error_message(&r, &resp)])),
            Ok(resp) => Err(VenueError::Ambiguous(error_message(&r, &resp))),
        }
    }

    /// #get_orders: one GET per id; the first failure is the answer; nothing is ever reported missing.
    async fn orders(&self, ids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let body = self.get(self.request("GET", false, format!("/v2/orders/{id}"), vec![], None)).await?;
            out.push(parse_order(id, &body));
        }
        Ok(out)
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
                let v: Value = serde_json::from_str(&resp.body).map_err(|_| VenueError::Rejected(vec![error_message(&r, &resp)]))?;
                match (v["id"].as_str(), v["client_order_id"].as_str()) {
                    (Some(id), Some(cl)) if cl == cl_ord_id => Ok(Some(parse_order(id, &v))),
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
    async fn balance(&self, asset_symbol: &str) -> Result<BigDec, VenueError> {
        let account = self.get(self.request("GET", false, "/v2/account".into(), vec![], None)).await?;
        self.get(self.request("GET", false, "/v2/positions".into(), vec![], None)).await?;
        // ponytail: eligibility admits only USD-quoted Alpaca bots; another quote would be a position lookup ("BTCUSD").
        if asset_symbol != "USD" { return Ok(BigDec::zero()); }
        Ok(ruby_opt_to_d(&account["non_marginable_buying_power"]).unwrap_or_else(|| ruby_to_d(&account["cash"])))
    }
}

/// Before 3.0 the engine trades Alpaca paper only (the amendment's rollout rule).
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
