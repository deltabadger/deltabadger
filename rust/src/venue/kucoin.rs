//! Honeymaker::Clients::Kucoin and Exchanges::Kucoin for the engine's spot orders (plan K, slices K1-K3): prices, balances,
//! placement, polls, cancel and recovery by client order id. Rails is the oracle; each method names the Ruby it ports. The
//! classic (LF) endpoints, as Rails runs them (plan K ruling O1); `venue::live` builds it for every KuCoin bot.
//!
//! Signing (Honeymaker::Clients::Kucoin#signed_headers): KC-API-SIGN = base64(HMAC-SHA256(secret, timestamp + METHOD + path +
//! `?query` (a GET with params) | the JSON body (a POST) | nothing (a DELETE))), the passphrase likewise HMAC'd, key version 2.
//! The transport (`http::ReqwestTransport::kucoin`) signs at send time over exactly the query and body bytes it sends.
use super::binance::{build_query, Pairs};
use super::http::{self, HttpRequest, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::candles::{self, Candle, Timeframe};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, KUCOIN as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const URL: &str = "https://api.kucoin.com";
pub const KUCOIN: &str = "Exchanges::Kucoin";
/// Clients::Kucoin#get_klines' path.
pub const CANDLES: &str = "/api/v1/market/candles";
/// Exchanges::Kucoin#get_candles' kline type for each timeframe: 3 days are daily and a month weekly, built.
pub fn kline_type(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::Minute => "1min", Timeframe::FiveMinutes => "5min", Timeframe::FifteenMinutes => "15min", Timeframe::ThirtyMinutes => "30min",
        Timeframe::Hour => "1hour", Timeframe::FourHours => "4hour", Timeframe::Day | Timeframe::ThreeDays => "1day",
        Timeframe::Week | Timeframe::Month => "1week",
    }
}
pub const UNREADABLE: &str = "KuCoin: unreadable response";
/// Exchanges::Kucoin#placement_result (R0b): a 200000 answer with no orderId took the order, or may have.
pub const NO_ORDER_ID: &str = "KuCoin answered the order with no order id";
/// Plan K r4: on KuCoin absence is never concluded. The exact client-order lookup only sees active orders and the done-orders
/// scan only what KuCoin still keeps, so a miss in both proves nothing; the intent waits for `deltabadger resolve-placement`.
/// A divergence: Rails sends a fresh random clientOid and has no lookup at all.
pub const ABSENCE_UNPROVEN: &str = "KuCoin: neither the client order lookup nor the done-orders scan found this order, and KuCoin cannot prove it absent; settle it with `deltabadger resolve-placement`";
/// allTickers is the whole market in one body (about 1 MB): its read stops at this many bytes, never a price from a cut body.
pub const ALL_TICKERS_LIMIT: usize = 4 * 1024 * 1024;
/// Exchanges::Kucoin::ORDER_NOT_FOUND_MESSAGES: a forgotten order is told by its message only (its code, 400100, is generic).
pub const ORDER_NOT_FOUND_MESSAGES: [&str; 5] = ["the order does not exist", "order not exist", "order_not_exist", "ordernotexist", "order not found"];
/// Exchanges::Kucoin::FILLS_WINDOW: `/api/v1/fills` (and the order lists) serve at most a 7-day range.
const WINDOW_MS: i64 = 7 * 86_400_000;
/// Exchanges::Kucoin::FILLS_RANGE_CAP: KuCoin answers at most 50,000 rows for one range.
const RANGE_CAP: i64 = 50_000;
const PAGE_SIZE: &str = "500";
/// ponytail: a page walk stops at 100 pages (50,000 rows, the range cap) and fails, never reads as complete.
const MAX_PAGES: i64 = 100;

/// Honeymaker's #get_public paths: sent with its unauthenticated headers.
pub fn is_public(path: &str) -> bool { path.starts_with("/api/v1/market/") || path.starts_with("/api/v2/symbols") }

/// The query as it is signed: classic as honeymaker signs it and Faraday sends it (Faraday::Utils.build_query: sorted,
/// escaped); an HF route as sent, in order and unescaped.
pub fn query_text(r: &HttpRequest) -> String {
    if hf(&r.path) { r.query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&") } else { build_query(&r.query) }
}

/// D-KUCOIN-HF-ORDERS: an HF route's request. Its parameters are signed as sent (in order, unescaped; the URL encodes
/// them separately), and a DELETE signs its query (`sign::KUCOIN_HF`).
pub fn hf(path: &str) -> bool { path.starts_with("/api/v1/hf/") }

/// The body bytes: #post_signed signs `body.compact.to_json` and Faraday's JSON middleware sends the same; the fields keep
/// the order the client built them in (serde_json's preserve_order).
pub fn body_text(r: &HttpRequest) -> Option<String> { r.body.as_ref().map(Value::to_string) }

/// The headers honeymaker sends with `r` at `timestamp_ms`: #signed_headers (`sign::KUCOIN`), or #unauthenticated_headers on
/// a public read.
pub fn headers(r: &HttpRequest, key: &str, secret: &str, passphrase: &str, timestamp_ms: i64) -> Vec<(String, String)> {
    if is_public(&r.path) { return super::sign::json_headers(); }
    let scheme = if hf(&r.path) { &super::sign::KUCOIN_HF } else { &super::sign::KUCOIN };
    super::sign::prehash(scheme, r, &query_text(r), key, secret, passphrase, timestamp_ms)
}

/// D-KUCOIN-HF-ORDERS: the HF order route.
const HF_ORDERS: &str = "/api/v1/hf/orders";
/// HF placement codes that prove nothing was placed: authentication and permission (400003–400007), invalid parameters
/// (400100, unless a duplicate clientOid), insufficient balance (200004), rate limit (429000), and 400002's safe rejection.
const HF_REFUSALS: [&str; 9] = ["400002", "400003", "400004", "400005", "400006", "400007", "400100", "200004", "429000"];

#[derive(Clone)]
pub struct KucoinVenue<T: Transport> { transport: T, base: String, pairs: Pairs, identity: Option<Value>, account: Option<AccountContext>, missing_context: bool, hf: bool }

/// Which order API an account's new orders use: the prepared detection's (ClassicHf: HF; anything else: classic).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family { Classic, Hf }

#[derive(Clone)]
struct AccountContext {
    version: crate::engine::model::CredentialVersion,
    resolver: super::kucoin_account::Resolver,
    prepared: std::sync::Arc<std::sync::Mutex<Option<super::kucoin_account::Detection>>>,
}

pub(crate) fn unreadable() -> VenueError { VenueError::Ambiguous(UNREADABLE.into()) }

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, VenueError> { super::json::field(v, key, unreadable) }
fn required(v: &Value) -> Result<BigDec, VenueError> { super::json::required(v, unreadable) }
/// `x&.to_d`: null or absent is None.
fn optional(v: &Value) -> Result<Option<BigDec>, VenueError> { if v.is_null() { Ok(None) } else { required(v).map(Some) } }
/// Ruby's truthiness of a JSON value: only null (or absent) and false are false.
fn truthy(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) }
/// `#to_s` of a scalar, as Ruby interpolates it.
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), other => other.to_string() } }
/// `#to_i` of a JSON page count (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_default(), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
/// The envelope's code as `to_s` reads it; an answer without one is unreadable.
fn code(body: &Value) -> Result<String, VenueError> { match body.get("code") { Some(c @ (Value::String(_) | Value::Number(_))) => Ok(to_s(c)), _ => Err(unreadable()) } }
const OK: &str = "200000";

/// Client#api_error: "KuCoin API error <code>: <msg>", the parts that are present.
pub fn api_error(body: &Value) -> String {
    let msg = Some(&body["msg"]).filter(|m| !m.is_null()).unwrap_or(&body["message"]);
    let detail: Vec<String> = [&body["code"], msg].into_iter().filter(|v| !v.is_null()).map(to_s).filter(|s| !s.is_empty()).collect();
    if detail.is_empty() { "KuCoin API error".into() } else { format!("KuCoin API error {}", detail.join(": ")) }
}
/// Exchanges::Kucoin's `result.data.values_at('code', 'msg').compact.join(' ')`.
fn code_and_msg(body: &Value) -> String { [&body["code"], &body["msg"]].into_iter().filter(|v| !v.is_null()).map(to_s).collect::<Vec<_>>().join(" ") }

/// Exchanges::Kucoin#order_not_found?.
pub fn not_found(messages: &[String]) -> bool {
    let joined = messages.join(" ").to_lowercase();
    ORDER_NOT_FOUND_MESSAGES.iter().any(|m| joined.contains(m))
}

/// Exchanges::Kucoin#parse_order_type: `market` or `limit` (any case), else Rails raises.
fn limit_type(v: &Value) -> Result<bool, VenueError> {
    let t = v.as_str().ok_or_else(unreadable)?;
    match t.to_lowercase().as_str() {
        "market" => Ok(false), "limit" => Ok(true),
        _ => Err(VenueError::Rejected(vec![format!("Unknown KuCoin order type: {t}")])),
    }
}

/// Exchanges::Kucoin#parse_order_data and #parse_order_status. A cancelled order keeps what it dealt; "done" with nothing
/// dealt is Unknown, never Closed (a market order's terminal zero fill carries `zero_fill_at`, D-KUCOIN-ZERO-FILL-FUNDS). The price is dealFunds / dealSize (rounded half up to the pair's tick) when both are
/// positive, else the order's own price; a zero price is none.
pub fn parse_order(order_id: &str, o: &Value, pairs: &Pairs) -> Result<OrderState, VenueError> {
    let symbol = field(o, "symbol")?.as_str().ok_or_else(unreadable)?;
    let limit = limit_type(field(o, "type")?)?;
    let (deal_funds, deal_size) = (optional(&o["dealFunds"])?, optional(&o["dealSize"])?);
    let mut price = match (&deal_funds, &deal_size) {
        (Some(f), Some(s)) if f.is_positive() && s.is_positive() => f.div(s).ok_or_else(unreadable)?,
        _ => optional(&o["price"])?.unwrap_or_else(BigDec::zero),
    };
    if price.is_positive() { if let Some(p) = pairs.by_symbol.get(symbol) { price = price.round(p.price_decimals); } }
    let sell = field(o, "side")?.as_str().ok_or_else(unreadable)?.eq_ignore_ascii_case("sell");
    let amount_exec = deal_size.clone().unwrap_or_else(BigDec::zero);
    let status = if truthy(&o["cancelExist"]) { OrderStatus::Cancelled } else if truthy(&o["isActive"]) { OrderStatus::Open }
                 else if amount_exec.is_positive() { OrderStatus::Closed } else { OrderStatus::Unknown };
    // D-KUCOIN-ZERO-FILL-FUNDS: only explicit terminal evidence on a market order (both flags JSON false, both dealt amounts
    // present and exactly 0, a readable createdAt) is kept for the poll to settle (OrderState::settled); all else as Rails.
    let zero = |v: &Option<BigDec>| v.as_ref().is_some_and(BigDec::is_zero);
    let zero_fill_at = (status == OrderStatus::Unknown && !limit && o["isActive"] == Value::Bool(false) && o["cancelExist"] == Value::Bool(false)
                        && zero(&deal_size) && zero(&deal_funds))
        .then(|| o["createdAt"].as_i64().and_then(DateTime::from_timestamp_millis)).flatten();
    Ok(OrderState { zero_fill_at, txid: order_id.into(), status, price: (!price.is_zero()).then_some(price), amount: optional(&o["size"])?, quote_amount: optional(&o["funds"])?,
                    amount_exec, quote_amount_exec: Some(deal_funds.unwrap_or_else(BigDec::zero)), limit, sell, pair: Some(symbol.into()), asset_class: None })
}

/// D-KUCOIN-HF-ORDERS: a stored HF order as its record names it. Every answer must be this order before its state is
/// read: its id, symbol, side and type.
#[derive(Clone, Debug, PartialEq)]
pub struct HfOrder { pub id: String, pub symbol: String, pub sell: bool, pub limit: bool }

/// D-KUCOIN-HF-ORDERS: an HF order's record (`rust_kucoin_order:<transaction id>`), written with its row from the intent
/// it was sent for: family, the exact wire symbol, side, type and the sending key (`host`, `key_fingerprint`), and the
/// last applied exact dealt totals. A row without one is a classic row (every row before HF-2).
pub const ORDER_RECORD: &str = "rust_kucoin_order:";

/// Writes the record of the row the bot's unresolved intent just became, when that intent was sent on an HF account.
pub fn record_order(c: &rusqlite::Connection, tx_id: i64, intent: &Value, now: DateTime<Utc>) -> Result<(), String> {
    if intent["kucoin_account_mode"] != "ClassicHf" { return Ok(()); }
    let record = serde_json::json!({"family": "ClassicHf", "symbol": intent["symbol"], "sell": intent["side"] == "sell",
        "limit": intent["limit"], "host": intent["host"], "key_fingerprint": intent["key_fingerprint"], "deal_size": "0", "deal_funds": "0"});
    crate::app_config::set_plain(c, &format!("{ORDER_RECORD}{tx_id}"), &record.to_string(), now)
}

/// The row's HF order record, if it has one.
pub fn order_record(c: &rusqlite::Connection, tx_id: i64) -> Result<Option<Value>, String> {
    Ok(crate::app_config::get_plain(c, &format!("{ORDER_RECORD}{tx_id}"))?.and_then(|s| serde_json::from_str(&s).ok()))
}

/// D-KUCOIN-HF-ORDERS: an HF row's dealt totals never go down. False (nothing applied) when either is lower than the
/// record's last applied exact totals; else the record keeps these, in the caller's transaction. Exact decimals, never the
/// row's f64 columns. A row without a record (classic) is always true.
pub fn advance_totals(c: &rusqlite::Connection, tx_id: i64, size: &BigDec, funds: Option<&BigDec>, now: DateTime<Utc>) -> Result<bool, String> {
    let Some(mut record) = order_record(c, tx_id)? else { return Ok(true) };
    let prior = |key: &str| record[key].as_str().and_then(|v| BigDec::parse(v).ok()).ok_or_else(|| format!("unreadable {key} in {ORDER_RECORD}{tx_id}"));
    let funds = funds.cloned().unwrap_or_else(BigDec::zero);
    let (old_size, old_funds) = (prior("deal_size")?, prior("deal_funds")?);
    if *size < old_size || funds < old_funds { return Ok(false); }
    if *size != old_size || funds != old_funds {
        record["deal_size"] = size.to_s_f().into();
        record["deal_funds"] = funds.to_s_f().into();
        crate::app_config::set_plain(c, &format!("{ORDER_RECORD}{tx_id}"), &record.to_string(), now)?;
    }
    Ok(true)
}

/// D-KUCOIN-HF-ORDERS: an HF order answer (`GET /api/v1/hf/orders/…`). Never reads `isActive`. Unreadable (Ambiguous, the
/// row waits) unless it is `want`, its `active`, `inOrderBook` and `cancelExist` are JSON booleans, and `dealSize` and
/// `dealFunds` are decimals, both ≥ 0, positive together. Then: inactive yet in the book is unreadable; active is Open;
/// cancelled is Cancelled with what it dealt; inactive with a fill is Closed; inactive with nothing dealt is Unknown, a
/// market order's carrying `zero_fill_at` (createdAt) for the delayed settle (D-KUCOIN-ZERO-FILL-FUNDS). The price as
/// `parse_order`'s.
pub fn parse_hf_order(o: &Value, want: &HfOrder, pairs: &Pairs) -> Result<OrderState, VenueError> {
    let text = |key: &str| o[key].as_str().ok_or_else(unreadable);
    let flag = |key: &str| o[key].as_bool().ok_or_else(unreadable);
    let side = text("side")?.to_lowercase();
    if text("id")? != want.id || text("symbol")? != want.symbol || side != if want.sell { "sell" } else { "buy" }
        || limit_type(field(o, "type")?).map_err(|_| unreadable())? != want.limit {
        return Err(unreadable());
    }
    let (active, in_book, cancelled) = (flag("active")?, flag("inOrderBook")?, flag("cancelExist")?);
    let (size, funds) = (required(field(o, "dealSize")?)?, required(field(o, "dealFunds")?)?);
    if size < BigDec::zero() || funds < BigDec::zero() || size.is_positive() != funds.is_positive() || (!active && in_book) { return Err(unreadable()); }
    let status = if active { OrderStatus::Open } else if cancelled { OrderStatus::Cancelled }
                 else if size.is_positive() { OrderStatus::Closed } else { OrderStatus::Unknown };
    let zero_fill_at = (status == OrderStatus::Unknown && !want.limit).then(|| o["createdAt"].as_i64().and_then(DateTime::from_timestamp_millis)).flatten();
    let mut price = if size.is_positive() { funds.div(&size).ok_or_else(unreadable)? } else { optional(&o["price"])?.unwrap_or_else(BigDec::zero) };
    if price.is_positive() { if let Some(p) = pairs.by_symbol.get(&want.symbol) { price = price.round(p.price_decimals); } }
    Ok(OrderState { zero_fill_at, txid: want.id.clone(), status, price: (!price.is_zero()).then_some(price), amount: optional(&o["size"])?, quote_amount: optional(&o["funds"])?,
                    amount_exec: size, quote_amount_exec: Some(funds), limit: want.limit, sell: want.sell, pair: Some(want.symbol.clone()), asset_class: None })
}

/// The prices of an allTickers answer, as Exchanges::Kucoin#get_tickers_prices reads them: every `data.ticker` row by its
/// `dig_or_raise`d `symbol`, its `last` kept only when positive, a later row for a symbol replacing an earlier one. No
/// `data.ticker` is Rails' "Failed to get KuCoin tickers". Shared by the engine (`tickers_prices`) and the figures.
pub fn all_tickers(body: &Value) -> Result<Vec<(String, BigDec)>, VenueError> {
    let failed = || VenueError::Rejected(vec!["Failed to get KuCoin tickers".into()]);
    // Hash#dig: a `data` that is not a hash (or a body that is not one) raises.
    let data = match body.as_object().ok_or_else(unreadable)?.get("data") { None | Some(Value::Null) => return Err(failed()), Some(d) => d };
    let rows = match data.as_object().ok_or_else(unreadable)?.get("ticker") { None | Some(Value::Null) => return Err(failed()), Some(t) => t };
    let mut out: Vec<(String, BigDec)> = vec![];
    for row in rows.as_array().ok_or_else(unreadable)? {
        let symbol = super::dig_or_raise(row, "symbol")?.as_str().ok_or_else(unreadable)?.to_string();
        let price = match &row["last"] {
            Value::Null => None,
            v @ (Value::String(_) | Value::Number(_)) => crate::ruby::json_to_d(v).ok().flatten(),
            _ => return Err(unreadable()),
        };
        // `prices_hash[symbol] = price if price.positive?`: a later positive price for the same symbol replaces it.
        if let Some(p) = price.filter(|p| *p > BigDec::zero()) {
            out.retain(|(s, _)| *s != symbol);
            out.push((symbol, p));
        }
    }
    Ok(out)
}

impl<T: Transport> KucoinVenue<T> {
    /// Classic wire adapter, also used by the unchanged Rails replay. Runtime construction attaches captured context.
    pub fn new(transport: T, base: &str) -> Self { Self { transport, base: base.into(), pairs: Pairs::default(), identity: None, account: None, missing_context: false, hf: false } }
    pub fn with_account_context(mut self, version: crate::engine::model::CredentialVersion, resolver: super::kucoin_account::Resolver) -> Self {
        self.account = Some(AccountContext { version, resolver, prepared: Default::default() });
        self
    }
    pub(crate) fn with_missing_account_context(mut self) -> Self { self.missing_context = true; self }
    /// D-KUCOIN-HF-ORDERS: admits a detected ClassicHf account (tests only, until HF-5 connects HF and removes this).
    #[doc(hidden)]
    pub fn allow_hf(mut self) -> Self { self.hf = true; self }
    /// The modes this venue trades on: classic always, HF only when admitted.
    fn admit(&self, mode: super::kucoin_account::AccountMode) -> Result<(), VenueError> {
        if self.hf && mode == super::kucoin_account::AccountMode::ClassicHf { Ok(()) } else { mode.require_classic() }
    }
    /// The account's mode as the shared Resolver detects it for this credential, admitted or not: the history and balance
    /// reads route on it (D-KUCOIN-HF-LEDGER); `prepare` keeps the trading gate.
    pub async fn account_mode(&self) -> Result<super::kucoin_account::AccountMode, VenueError> {
        if self.missing_context { return Err(super::kucoin_account::refusal("credential provenance missing")); }
        let Some(account) = &self.account else { return Ok(super::kucoin_account::AccountMode::ClassicLf) };
        Ok(account.resolver.resolve(&self.transport, &self.base, &account.version).await.value?.mode)
    }
    /// The order API of new orders and balances: the prepared detection's. Unprepared (the Rails replay): classic.
    pub fn family(&self) -> Family {
        let prepared = self.account.as_ref().and_then(|a| *a.prepared.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if prepared.is_some_and(|d| d.mode == super::kucoin_account::AccountMode::ClassicHf) { Family::Hf } else { Family::Classic }
    }
    fn prepared_for_send(&self) -> Result<(), VenueError> {
        if self.missing_context { return Err(super::kucoin_account::refusal("credential provenance missing")); }
        if let Some(account) = &self.account {
            let prepared = *account.prepared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match prepared {
                Some(d) if account.resolver.is_current(&self.transport, &self.base, &account.version, d) => self.admit(d.mode)?,
                _ => return Err(super::kucoin_account::refusal("account preparation missing, expired or invalidated")),
            }
        }
        Ok(())
    }
    /// The key this venue sends with (`venue::key_identity`), recorded in each intent and compared before a recovery asks.
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self { self.identity = Some(super::key_identity(&self.base, credentials)); self }
    /// Another host: local test servers only.
    #[doc(hidden)]
    pub fn with_base(mut self, base: &str) -> Self { self.base = base.into(); self }
    /// The exchange's tickers (`Pairs::load`): venue codes to assets, and each pair's price decimals.
    pub fn with_pairs(mut self, pairs: Pairs) -> Self { self.pairs = pairs; self }

    fn request(&self, method: &'static str, path: String, mut query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        query.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest { method, base: self.base.clone(), path, query, body, not_after: None }
    }

    /// One call as honeymaker's #with_rescue returns it and Exchange#ambiguous_placement_error? reads it for a placement. A
    /// transport failure that sent nothing is Transient (Rejected on a placement: definitive); one that may have sent is
    /// Transient on a read and Ambiguous on a placement. A non-2xx answer is its body (Faraday's message when empty); on a
    /// placement only a 4xx, or a placement-safe 400002 whatever its status, is definitive. A 2xx that is not JSON is unreadable.
    async fn call(&self, r: &HttpRequest, placement: bool, limit: usize) -> Result<Value, VenueError> {
        http::decode_json(&self.answer(r, placement, limit).await?).map_err(|_| unreadable())
    }

    /// One send as honeymaker's #with_rescue reads it: the 2xx body's text, or the failure.
    async fn answer(&self, r: &HttpRequest, placement: bool, limit: usize) -> Result<String, VenueError> {
        let resp = self.transport.send_limited(r, limit).await.map_err(|e| http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m)))?;
        if !(200..300).contains(&resp.status) {
            let message = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { resp.body };
            // Honeymaker attaches the status (data: { status: }) and Exchange#raise_on_invalid_key! reads a 401 as a rejected key.
            if !placement && resp.status == 401 { return Err(VenueError::Unauthorized(vec![message])); }
            let definitive = (400..500).contains(&resp.status) || crate::engine::placement::safe_rejection(std::slice::from_ref(&message));
            return Err(if placement && !definitive { VenueError::Ambiguous(message) } else { VenueError::Rejected(vec![message]) });
        }
        Ok(resp.body)
    }

    async fn get(&self, path: String, query: Vec<(&'static str, String)>, limit: usize) -> Result<Value, VenueError> {
        self.call(&self.request("GET", path, query, None), false, limit).await
    }

    /// One signed GET as honeymaker's #with_rescue returns it to Exchanges::Kucoin's ledger reads: the 2xx body of at most
    /// `limit` bytes, unparsed (`Ok`), else the failure's text: the answer's body, or Faraday's (or the transport's) message.
    pub async fn read_text(&self, path: &str, query: Vec<(&'static str, String)>, limit: usize) -> Result<String, String> {
        let r = self.request("GET", path.into(), query, None);
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, r.url())),
        }
    }

    /// `read_text` with the query sent in the order given: an HF route signs its parameters as sent (D-KUCOIN-HF-ORDERS).
    pub async fn read_text_in_order(&self, path: &str, query: Vec<(&'static str, String)>, limit: usize) -> Result<String, String> {
        let r = HttpRequest { method: "GET", base: self.base.clone(), path: path.into(), query, body: None, not_after: None };
        match self.transport.send_limited(&r, limit).await {
            Err(TransportError::Permanent(m) | TransportError::NotSent(m) | TransportError::MaybeSent(m)) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!("the server responded with status {} for GET {}", resp.status, r.url())),
        }
    }

    /// Clients::Kucoin#get_accounts(type: 'trade') as Exchanges::Kucoin#get_balances reads it: only the success code means
    /// balances, else "Failed to get KuCoin balances: <code> <msg>".
    async fn accounts(&self) -> Result<Vec<Value>, VenueError> {
        super::Venue::prepare(self).await?;
        // D-KUCOIN-HF-ORDERS: an HF account's funds are its trade_hf account's.
        let kind = if self.family() == Family::Hf { "trade_hf" } else { "trade" };
        let body = self.get("/api/v1/accounts".into(), vec![("type", kind.into())], http::MAX_BODY).await?;
        if body["data"].is_null() || code(&body).ok().as_deref() != Some(OK) {
            return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin balances: {}", code_and_msg(&body))]));
        }
        let rows = body["data"].as_array().ok_or_else(unreadable)?;
        rows.iter().map(|row| { field(row, "currency")?; Ok(row.clone()) }).collect()
    }

    /// `available` of the last row the predicate picks (absent is 0). `locked`, or before KuCoin's 2026-08-27 rename `holds`,
    /// must be readable too, as #get_balances digs for it.
    async fn available(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let mut free = BigDec::zero();
        for row in self.accounts().await? {
            if !pick(row["currency"].as_str().ok_or_else(unreadable)?) { continue; }
            free = required(field(&row, "available")?)?;
            match row.get("locked") { Some(Value::Null) => {}, Some(l) => { required(l)?; } None => { required(field(&row, "holds")?)?; } }
        }
        Ok(free)
    }

    /// Clients::Kucoin#get_order as Exchanges::Kucoin#get_order reads it: None for an order KuCoin no longer knows (by its
    /// message only, or the gem's "Order not found" for a success envelope without one).
    async fn get_order(&self, order_id: &str) -> Result<Option<Value>, VenueError> {
        let body = match self.get(format!("/api/v1/orders/{order_id}"), vec![], http::MAX_BODY).await {
            Err(VenueError::Rejected(m) | VenueError::Unauthorized(m) | VenueError::Coded { errors: m, .. }) if not_found(&m) => return Ok(None),
            other => other?,
        };
        if code(&body)? != OK {
            let message = vec![api_error(&body)];
            return if not_found(&message) { Ok(None) } else { Err(VenueError::Rejected(message)) };
        }
        Ok(Some(body["data"].clone()).filter(|d| !d.is_null()))
    }

    /// Exchanges::Kucoin#each_page: currentPage 1.. to totalPage, every page on the success code ("Failed to get KuCoin
    /// history: <code> <msg>" else), each item handed to `each`. `capped`: a first page naming more than RANGE_CAP rows fails.
    /// ponytail: Rails waits a throttled page out (6 × 10 s) and halves an over-cap fills range down to an hour; here both
    /// fail the read, and the caller's poll is retried later, so nothing is ever read as complete.
    async fn each_page(&self, path: &str, query: &[(&'static str, String)], capped: bool, mut each: impl FnMut(&Value)) -> Result<(), VenueError> {
        for page in 1..=MAX_PAGES {
            let mut q = query.to_vec();
            q.extend([("currentPage", page.to_string()), ("pageSize", PAGE_SIZE.into())]);
            let body = self.get(path.into(), q, http::MAX_BODY).await?;
            if code(&body).ok().as_deref() != Some(OK) {
                return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin history: {}", code_and_msg(&body))]));
            }
            let data = &body["data"];
            if capped && page == 1 && to_i(&data["totalNum"]) > RANGE_CAP {
                return Err(VenueError::Rejected(vec![format!("KuCoin has more than {RANGE_CAP} fills in one range")]));
            }
            match &data["items"] { Value::Null => {}, Value::Array(items) => items.iter().for_each(&mut each), _ => return Err(unreadable()) }
            if page >= to_i(&data["totalPage"]) { return Ok(()); }
        }
        Err(VenueError::Rejected(vec![format!("Too many pages of KuCoin {path}")]))
    }

    /// Weekly windows [from, to) as Exchanges::Kucoin walks them, each read to its end.
    async fn windows(&self, path: &str, query: &[(&'static str, String)], from_ms: i64, to_ms: i64, capped: bool, mut each: impl FnMut(&Value)) -> Result<(), VenueError> {
        let mut start = from_ms;
        while start < to_ms {
            let end = (start + WINDOW_MS).min(to_ms);
            let mut q = query.to_vec();
            q.extend([("startAt", start.to_string()), ("endAt", end.to_string())]);
            self.each_page(path, &q, capped, &mut each).await?;
            start = end;
        }
        Ok(())
    }

    /// D-KUCOIN-HF-ORDERS: an HF placement's answer, its status and envelope read together. Accepted only on 200000 with
    /// an orderId and no other clientOid. Refused only by a code that proves nothing was placed (HF_REFUSALS, 400100
    /// unless it names a duplicate clientOid, HTTP 429), and 400002 as the safe rejection it is. A transport that sent
    /// nothing is Transient (NotSent: no row). Every other answer (5xx, 500000/230005, an unknown or missing code, a
    /// duplicate clientOid, unreadable) is Ambiguous: the intent stays and no replacement is sent.
    async fn hf_placed(&self, r: &HttpRequest, cl_ord_id: &str) -> Result<String, VenueError> {
        let resp = match self.transport.send_limited(r, http::MAX_BODY).await {
            Err(http::TransportError::NotSent(m)) => return Err(VenueError::Transient(self.transport.redact_diagnostic(&m))),
            Err(e) => return Err(http::transport_failure(e, true, |m| self.transport.redact_diagnostic(&m))),
            Ok(resp) => resp,
        };
        let body = http::decode_json(&resp.body).ok();
        let code = body.as_ref().and_then(|b| code(b).ok());
        let refused = |b: &Value| VenueError::Rejected(vec![api_error(b)]);
        match (body.as_ref(), code.as_deref()) {
            (Some(b), Some(OK)) if (200..300).contains(&resp.status) => {
                let other = b["data"]["clientOid"].as_str().is_some_and(|c| c != cl_ord_id);
                match &b["data"]["orderId"] { Value::String(id) if !id.is_empty() && !other => Ok(id.clone()), _ => Err(VenueError::Ambiguous(NO_ORDER_ID.into())) }
            }
            (Some(b), Some("400100")) if b["msg"].as_str().is_some_and(|m| m.to_lowercase().contains("clientoid")) => Err(VenueError::Ambiguous(api_error(b))),
            // 400002 is the safe rejection whatever its text: the engine's own marker carries it (placement::safe_rejection).
            (Some(b), Some("400002")) if resp.status < 500 => Err(VenueError::Rejected(vec!["Invalid KC-API-TIMESTAMP".into(), api_error(b)])),
            // A server error is never a refusal, whatever code it carries.
            (Some(b), Some(c)) if HF_REFUSALS.contains(&c) && resp.status < 500 => Err(refused(b)),
            (_, _) if resp.status == 429 => Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&resp.body)])),
            (Some(b), _) => Err(VenueError::Ambiguous(api_error(b))),
            (None, _) => Err(VenueError::Ambiguous(format!("KuCoin answered HTTP {} with an unreadable body", resp.status))),
        }
    }

    /// D-KUCOIN-HF-ORDERS: the stored HF order a ref names, when this venue may ask about it: HF admitted, and the ref's
    /// recorded host and key are this venue's own (never another account's order, never redirected by the current mode).
    fn hf_ref(&self, r: &super::OrderRef) -> Option<Result<HfOrder, ()>> {
        let record = r.record.as_ref().filter(|rec| rec["family"] == "ClassicHf")?;
        let mine = self.hf && record["host"].as_str() == Some(self.base.as_str())
            && self.identity.as_ref().is_some_and(|i| i["key_fingerprint"] == record["key_fingerprint"]);
        let order = (|| Some(HfOrder { id: r.external_id.clone(), symbol: record["symbol"].as_str()?.into(), sell: record["sell"].as_bool()?, limit: record["limit"].as_bool()? }))();
        Some(order.filter(|_| mine).ok_or(()))
    }

    /// D-KUCOIN-HF-ORDERS: one stored HF order, `GET /api/v1/hf/orders/{id}?symbol=`. A failure worth retrying (transport,
    /// throttle, transient text) is its error; any other answer that cannot be told (not found, refused, unreadable) is
    /// None, and the row waits (an excluded id: never missing, never abandoned).
    async fn hf_order(&self, want: &HfOrder) -> Result<Option<OrderState>, VenueError> {
        let r = HttpRequest { method: "GET", base: self.base.clone(), path: format!("{HF_ORDERS}/{}", want.id), query: vec![("symbol", want.symbol.clone())], body: None, not_after: None };
        match self.call(&r, false, http::MAX_BODY).await {
            Ok(body) if code(&body).ok().as_deref() == Some(OK) => Ok(parse_hf_order(&body["data"], want, &self.pairs).ok()),
            Ok(_) => Ok(None),
            Err(e @ VenueError::Transient(_)) => Err(e),
            Err(VenueError::Rejected(m)) if RULES.is_throttle(&m) || RULES.is_transient(&m) => Err(VenueError::Rejected(m)),
            Err(_) => Ok(None),
        }
    }

    /// D-KUCOIN-HF-ORDERS: recovery's exact lookup, `GET /api/v1/hf/orders/client-order/{clientOid}?symbol=`. Only an
    /// answer naming this clientOid and symbol is the order (then read as `parse_hf_order` reads it); a miss or any failure
    /// is never absence (ABSENCE_UNPROVEN): the intent stays. There is no done-orders scan on HF.
    async fn hf_by_client_id(&self, cl_ord_id: &str, symbol: &str) -> Result<Option<OrderState>, VenueError> {
        let r = HttpRequest { method: "GET", base: self.base.clone(), path: format!("{HF_ORDERS}/client-order/{cl_ord_id}"), query: vec![("symbol", symbol.into())], body: None, not_after: None };
        let found = match self.call(&r, false, http::MAX_BODY).await {
            Ok(b) if code(&b).ok().as_deref() == Some(OK) && b["data"]["clientOid"].as_str() == Some(cl_ord_id) && b["data"]["symbol"].as_str() == Some(symbol) => b["data"].clone(),
            _ => return Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())),
        };
        let want = HfOrder { id: found["id"].as_str().filter(|s| !s.is_empty()).ok_or_else(unreadable)?.into(), symbol: symbol.into(),
                             sell: found["side"].as_str().is_some_and(|s| s.eq_ignore_ascii_case("sell")), limit: limit_type(field(&found, "type")?).map_err(|_| unreadable())? };
        parse_hf_order(&found, &want, &self.pairs).map(Some)
    }

    /// The classic clientOid lookup (`order_by_client_id` on a classic account): the client-order route, then the done-orders
    /// windows. A miss is never absence.
    async fn classic_by_client_id(&self, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        let mine = |o: &Value| o["clientOid"].as_str() == Some(cl_ord_id) && o["symbol"].as_str() == Some(symbol);
        let found = |o: &Value| -> Result<OrderState, VenueError> { parse_order(o["id"].as_str().filter(|s| !s.is_empty()).ok_or_else(unreadable)?, o, &self.pairs) };
        if let Ok(body) = self.get(format!("/api/v1/order/client-order/{cl_ord_id}"), vec![], http::MAX_BODY).await {
            if code(&body).ok().as_deref() == Some(OK) && mine(&body["data"]) { return found(&body["data"]).map(Some); }
        }
        let mut hit = None;
        let query = [("status", "done".to_string()), ("symbol", symbol.to_string())];
        self.windows("/api/v1/orders", &query, since.timestamp_millis(), self.transport.now().timestamp_millis(), false, |o| {
            if hit.is_none() && mine(o) { hit = Some(o.clone()); }
        }).await?;
        match hit { Some(o) => found(&o).map(Some), None => Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())) }
    }

    /// Exchanges::Kucoin#order_data_from_fills: the summed sizes and funds, the price funds / size at the tick, closed.
    fn order_from_fills(&self, order_id: &str, fills: &[(String, Value)]) -> Result<OrderState, VenueError> {
        let (mut amount, mut quote) = (BigDec::zero(), BigDec::zero());
        for (_, f) in fills { amount = &amount + &required(field(f, "size")?)?; quote = &quote + &required(field(f, "funds")?)?; }
        let first = &fills[0].1;
        let symbol = first["symbol"].as_str().ok_or_else(unreadable)?;
        let mut price = if amount.is_positive() { quote.div(&amount) } else { None };
        if let (Some(p), Some(pair)) = (&mut price, self.pairs.by_symbol.get(symbol)) { *p = p.round(pair.price_decimals); }
        Ok(OrderState { zero_fill_at: None, txid: order_id.into(), status: OrderStatus::Closed, price, amount: None, quote_amount: None, amount_exec: amount,
                        quote_amount_exec: Some(quote), limit: limit_type(&Value::String(to_s(&first["type"])))?,
                        sell: to_s(&first["side"]).eq_ignore_ascii_case("sell"), pair: Some(symbol.into()), asset_class: None })
    }
}

impl<T: Transport> super::Venue for KucoinVenue<T> {
    fn producer(&self) -> Option<crate::engine::model::CredentialVersion> { self.account.as_ref().map(|a| a.version.clone()) }
    async fn prepare(&self) -> Result<Option<crate::engine::model::CredentialVersion>, VenueError> {
        if self.missing_context { return Err(super::kucoin_account::refusal("credential provenance missing")); }
        if let Some(account) = &self.account {
            let detected = account.resolver.resolve(&self.transport, &self.base, &account.version).await.value?;
            let mut prepared = account.prepared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if prepared.is_some_and(|old| old.mode != detected.mode) {
                return Err(super::kucoin_account::refusal("account mode changed since preparation"));
            }
            self.admit(detected.mode)?;
            *prepared = Some(detected);
        }
        Ok(self.producer())
    }
    async fn validate_preparation(&self, identity: Option<&Value>) -> Result<(), VenueError> {
        self.prepare().await?;
        // The intent's family must be the prepared one: an order is sent only on the account family it was sized for.
        let prepared = self.account.as_ref().and_then(|a| *a.prepared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).map(|d| d.mode.as_str());
        if self.account.is_some() && (prepared.is_none() || identity.and_then(|i| i["kucoin_account_mode"].as_str()) != prepared) {
            return Err(super::kucoin_account::refusal("intent has no matching prepared account mode"));
        }
        self.prepared_for_send()
    }
    /// Exchanges::Kucoin#get_candles: windows of 1500 periods from the start (`startAt`/`endAt` in whole seconds, a month's
    /// 1500 calendar months), each ending at now at the latest, at most MAX_CANDLE_PAGES of them (then what was read, as
    /// Rails' cap). Each window's candles in time order; the next window from a second after its latest, or a second after
    /// its end when it had none; the walk ends at the window reaching now. Deduped by time (the first kept) and sorted; 3
    /// days built from daily candles and 1 month from weekly. Rows are [time, open, close, high, low, volume, …]. DIVERGES
    /// (D-COND-CANDLE-ERROR-ENVELOPE): a code other than 200000 is the venue's refusal (Client#api_error), where Rails reads
    /// its missing `data` as no candles.
    async fn candles(&self, ticker: &Ticker, start_at: DateTime<Utc>, timeframe: Timeframe) -> Result<Vec<Candle>, VenueError> {
        let window = |from: DateTime<Utc>| match timeframe {
            Timeframe::Month => from.checked_add_months(chrono::Months::new(1500)),
            _ => from.checked_add_signed(chrono::Duration::seconds(1500 * timeframe.seconds())),
        };
        let (mut start, mut out) = (start_at, Vec::<Candle>::new());
        for _ in 0..candles::MAX_CANDLE_PAGES {
            let now = self.transport.now();
            let end = window(start).map_or(now, |e| e.min(now));
            let r = self.request("GET", CANDLES.into(), vec![("symbol", ticker.ticker.clone()), ("type", kline_type(timeframe).into()),
                                                         ("startAt", start.timestamp().to_string()), ("endAt", end.timestamp().to_string())], None);
            let body = candles::get(&self.transport, &r, str::to_string, None, unreadable).await?;
            if !body.is_object() { return Err(unreadable()); }
            if body["code"] != OK { return Err(VenueError::Rejected(vec![api_error(&body)])); }
            let mut page = match &body["data"] {
                Value::Null => vec![],
                Value::Array(rows) => rows.iter().map(|row| {
                    let at = row.get(0).and_then(candles::to_i).and_then(|s| DateTime::from_timestamp(s, 0));
                    at.and_then(|at| candles::fields(at, row, [1, 3, 4, 2, 5])).ok_or_else(unreadable)
                }).collect::<Result<Vec<_>, _>>()?,
                _ => return Err(unreadable()),
            };
            // ponytail: sorted by time; Rails sorts by the raw value, which differs only for times of different lengths.
            page.sort_by_key(|c| c.at);
            let latest = page.last().map(|c| c.at);
            out.extend(page);
            if end >= now { break; }
            start = latest.unwrap_or(end) + chrono::Duration::seconds(1);
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|c| seen.insert(c.at));
        out.sort_by_key(|c| c.at);
        Ok(if matches!(timeframe, Timeframe::ThreeDays | Timeframe::Month) { candles::build(&out, timeframe) } else { out })
    }

    fn rules(&self) -> &'static VenueRules { &RULES }
    fn placement_identity(&self) -> Option<Value> {
        let mut identity = self.identity.clone();
        if let Some(account) = &self.account {
            if let Some(d) = *account.prepared.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
                identity.get_or_insert_with(|| serde_json::json!({}))["kucoin_account_mode"] = d.mode.as_str().into();
            }
        }
        identity
    }

    /// #get_ask_price / #get_bid_price through #get_bid_ask_price (level2_20: the first bid and ask, price and size), and
    /// #get_last_price (allTickers, the symbol's `last`; null is 0). A zero price raises "Wrong … price", naming the pair.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let name = &ticker.ticker;
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let body = self.get("/api/v1/market/orderbook/level2_20".into(), vec![("symbol", name.clone())], http::MAX_BODY).await?;
                let data = &body["data"];
                if data.is_null() { return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin order book for {name}")])); }
                let level = |book: &str, i: usize| required(&data[book][0][i]);
                let (bid, ask) = (level("bids", 0)?, level("asks", 0)?);
                level("bids", 1)?;
                level("asks", 1)?;
                if side == PriceSide::Bid { (bid, "bid") } else { (ask, "ask") }
            }
            PriceSide::Last => {
                let body = self.get("/api/v1/market/allTickers".into(), vec![], ALL_TICKERS_LIMIT).await?;
                let Some(rows) = body["data"]["ticker"].as_array() else { return Err(VenueError::Rejected(vec!["Failed to get KuCoin last price".into()])) };
                let row = rows.iter().find(|t| t["symbol"].as_str() == Some(name))
                    .ok_or_else(|| VenueError::Rejected(vec![format!("Ticker {name} not found on KuCoin")]))?;
                (optional(&row["last"])?.unwrap_or_else(BigDec::zero), "last")
            }
        };
        if price.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {name}: {}", price.to_s_f())])); }
        Ok(price)
    }

    /// #set_market_order / #set_limit_order through Clients::Kucoin#place_order: POST /api/v1/orders with clientOid, side,
    /// symbol, type, then a market order's `size` or `funds`, or a limit order's `size` and `price`. `clientOid` is the
    /// intent id (spec E §2.6; Rails sends a fresh random UUID). Sent at most once. The stored id is the raw orderId. An
    /// error envelope is honeymaker's api_error text (the placement layer reads 500000/230005 as unknown, 400002 as safe);
    /// a success without an orderId, or an answer without a code, may have placed the order: Ambiguous.
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        self.prepared_for_send()?;
        let mut body = serde_json::Map::new();
        for (k, v) in [("clientOid", o.cl_ord_id.as_str()), ("side", o.side.as_str()), ("symbol", o.pair.as_str())] { body.insert(k.into(), v.into()); }
        match &o.kind {
            OrderKind::Market => {
                body.insert("type".into(), "market".into());
                body.insert(if o.quote_volume { "funds" } else { "size" }.into(), o.volume.clone().into());
            }
            OrderKind::Limit { price } => {
                body.insert("type".into(), "limit".into());
                body.insert("size".into(), o.volume.clone().into());
                body.insert("price".into(), price.clone().into());
            }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("KuCoin")),
        }
        // The absolute bound on the send, as for every venue: the intent's send window, plus the request budget.
        let hf = self.family() == Family::Hf;
        let r = HttpRequest { not_after: Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s")),
                              ..self.request("POST", if hf { HF_ORDERS } else { "/api/v1/orders" }.into(), vec![], Some(Value::Object(body))) };
        if hf { return self.hf_placed(&r, &o.cl_ord_id).await; }
        let answer = self.call(&r, true, http::MAX_BODY).await?;
        if code(&answer)? != OK { return Err(VenueError::Rejected(vec![api_error(&answer)])); }
        match &answer["data"]["orderId"] {
            Value::String(id) if !id.is_empty() => Ok(id.clone()),
            _ => Err(VenueError::Ambiguous(NO_ORDER_ID.into())),
        }
    }

    /// #get_orders: #get_order per id; an order KuCoin no longer knows is left out (`missing`, which polling settles from
    /// its fills, `fills_from_trades`); any other failure fails the batch.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for id in txids {
            if let Some(raw) = self.get_order(id).await? { out.push(parse_order(id, &raw, &self.pairs)?); }
        }
        Ok(out)
    }

    /// #get_order (Bot::FetchAndUpdateOrderJob). A forgotten order fails here: the follow-up has no order row to date a fills
    /// read from, so the row waits for the sweep, which reads its fills (`fills_from_trades`) as #order_from_fills does.
    async fn order_identified<F>(&self, txid: &str, _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        match self.get_order(txid).await? {
            Some(raw) => Ok((vec![parse_order(txid, &raw, &self.pairs)?], vec![])),
            None => Err(VenueError::Rejected(vec![format!("KuCoin no longer knows order {txid}")])),
        }
    }

    /// Recovery (plan K r4) may only FIND the order: the exact lookup `GET /api/v1/order/client-order/{clientOid}` (active
    /// orders only; a hit naming this clientOid and symbol is authoritative), then `GET /api/v1/orders?status=done` over weekly
    /// windows from `since` to now, every page, matching clientOid. Neither finding it is never absence (ABSENCE_UNPROVEN);
    /// a failed scan page is its own error.
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        if self.family() == Family::Hf { return self.hf_by_client_id(cl_ord_id, symbol).await; }
        self.classic_by_client_id(cl_ord_id, symbol, since).await
    }

    /// D-KUCOIN-HF-RETENTION: an intent sent on an HF account is asked on the HF clientOid route, whatever the account's mode
    /// is now (the caller has checked the intent's host and key are this venue's); one sent classic, on the classic route.
    async fn recovery_with(&self, intent: &Value, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        match intent["kucoin_account_mode"].as_str() {
            Some("ClassicHf") if self.hf => self.hf_by_client_id(cl_ord_id, symbol).await,
            Some("ClassicHf") => Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())),
            Some(_) => self.classic_by_client_id(cl_ord_id, symbol, since).await,
            None => super::Venue::order_by_client_id(self, cl_ord_id, symbol, since).await,
        }
    }
    /// Exchanges::Kucoin#order_from_fills for each order #get_orders named missing (R0c): its fills by orderId in weekly windows
    /// from `since` (the oldest missing row, less an hour) to now, each fill once by tradeId. A fill books the order closed;
    /// no fill leaves it missing (Bot::StaleOrderResolver decides); a failed read fails, so nothing is written off.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        let now_ms = self.transport.now().timestamp() * 1000;
        for id in txids {
            let mut fills: Vec<(String, Value)> = vec![];
            self.windows("/api/v1/fills", &[("orderId", id.clone())], since.timestamp() * 1000, now_ms, true, |f| {
                if to_s(&f["orderId"]) != *id { return; }
                let trade = to_s(&f["tradeId"]);
                match fills.iter_mut().find(|(t, _)| *t == trade) { Some(slot) => slot.1 = f.clone(), None => fills.push((trade, f.clone())) }
            }).await?;
            if !fills.is_empty() { out.push(self.order_from_fills(id, &fills)?); }
        }
        Ok(out)
    }

    /// #get_balances' `available` for the quote asset: each trade-account row's currency mapped to its asset (#asset_from_symbol).
    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.available(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol)).await
    }

    /// The base's available amount for a seller: the trade-account row of the ticker's base code.
    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> { self.available(|code| code == ticker.base_code).await }

    async fn settled_quote_balance<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { self.available(|code| code == ticker.quote_code).await }

    /// Exchanges::Kucoin#get_tickers_prices: the allTickers read `price(Last)` makes, every `data.ticker` row by its
    /// `dig_or_raise`d `symbol`, the price `last&.to_d || 0` kept only when positive. No `data.ticker` is Rails' "Failed to
    /// get KuCoin tickers". DIVERGES on purpose as Binance does (ruby::json_to_d, Q6): a `last` that is no decimal is left
    /// out, where String#to_d reads "12x" as 12. Rails caches the answer for a minute; this reads fresh. Each price is its
    /// decimal's `to_s('F')`.
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
        // A 2xx body that is not JSON is honeymaker's Failure (its text), never a raise.
        let text = self.answer(&self.request("GET", "/api/v1/market/allTickers".into(), vec![], None), false, ALL_TICKERS_LIMIT).await?;
        let body = match http::decode_json(&text) {
            Err(http::DecodeError::NotJson) => return Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&text)])),
            other => other.map_err(|_| unreadable())?,
        };
        Ok(all_tickers(&body)?.into_iter().map(|(symbol, price)| (symbol, Value::String(price.to_s_f()))).collect())
    }

    /// #cancel_order through Clients::Kucoin#cancel_order: DELETE /api/v1/orders/{id}. A divergence: Rails takes any 2xx as a
    /// cancel; here only the success code is, an error envelope is the venue's refusal.
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let body = self.call(&self.request("DELETE", format!("/api/v1/orders/{txid}"), vec![], None), false, http::MAX_BODY).await?;
        if code(&body)? == OK { Ok(()) } else { Err(VenueError::Rejected(vec![api_error(&body)])) }
    }

    /// D-KUCOIN-HF-ORDERS: classic rows as `orders_identified`; each HF row on its own route, by its record. An HF row this
    /// venue may not ask about, or whose answer cannot be told, is excluded: present, never missing or abandoned.
    async fn orders_with<F>(&self, refs: &[super::OrderRef], identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        let classic: Vec<String> = refs.iter().filter(|r| self.hf_ref(r).is_none() && !r.adopted()).map(|r| r.external_id.clone()).collect();
        let (mut found, mut excluded) = if classic.is_empty() { (vec![], vec![]) } else { super::Venue::orders_identified(self, &classic, identify).await? };
        for r in refs {
            // D-KUCOIN-HF-RETENTION: an adopted row whose route is unproven is read on neither route: it waits, excluded.
            if r.adopted() { excluded.push(r.external_id.clone()); continue; }
            match self.hf_ref(r) {
                None => {}
                // A bulk read never fails the sweep for one HF row: it waits, excluded.
                Some(Ok(want)) => match self.hf_order(&want).await { Ok(Some(s)) => found.push(s), Ok(None) | Err(_) => excluded.push(r.external_id.clone()) },
                Some(Err(())) => excluded.push(r.external_id.clone()),
            }
        }
        Ok((found, excluded))
    }
    async fn order_with<F>(&self, r: &super::OrderRef, identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        if r.adopted() { return Ok((vec![], vec![r.external_id.clone()])); }
        match self.hf_ref(r) {
            None => super::Venue::order_identified(self, &r.external_id, identify).await,
            // One order's read keeps a retryable failure, so the follow-up is retried, never dropped.
            Some(Ok(want)) => match self.hf_order(&want).await? { Some(s) => Ok((vec![s], vec![])), None => Ok((vec![], vec![r.external_id.clone()])) },
            Some(Err(())) => Ok((vec![], vec![r.external_id.clone()])),
        }
    }
    /// D-KUCOIN-HF-ORDERS: an HF row's cancel, `DELETE /api/v1/hf/orders/{id}?symbol=`. Accepted only on 200000 naming
    /// this order; an error envelope is the venue's refusal; anything else (lost, 5xx, unreadable, another id) is
    /// Ambiguous. An accepted cancel is a request, never a terminal state: the row's poll settles it.
    async fn cancel_with(&self, r: &super::OrderRef) -> Result<(), VenueError> {
        // D-KUCOIN-HF-RETENTION: never cancelled on a route its order is not proven to be on.
        if r.adopted() { return Err(VenueError::Ambiguous("KuCoin order route unproven".into())); }
        let want = match self.hf_ref(r) {
            None => return super::Venue::cancel(self, &r.external_id).await,
            Some(Ok(want)) => want,
            Some(Err(())) => return Err(VenueError::Ambiguous("KuCoin HF order record does not match this venue".into())),
        };
        let req = HttpRequest { method: "DELETE", base: self.base.clone(), path: format!("{HF_ORDERS}/{}", want.id), query: vec![("symbol", want.symbol.clone())], body: None, not_after: None };
        let resp = match self.transport.send_limited(&req, http::MAX_BODY).await {
            Ok(resp) => resp,
            Err(e) => return Err(VenueError::Ambiguous(self.transport.redact_diagnostic(&format!("{e:?}")))),
        };
        let body = http::decode_json(&resp.body).ok();
        match (resp.status, body.as_ref().and_then(|b| code(b).ok()).as_deref()) {
            (200..=299, Some(OK)) if body.as_ref().is_some_and(|b| b["data"]["orderId"].as_str() == Some(want.id.as_str())) => Ok(()),
            (400..=499, Some(c)) if c != OK => Err(VenueError::Rejected(vec![api_error(body.as_ref().expect("a code was read"))])),
            _ => Err(VenueError::Ambiguous(format!("KuCoin's answer to cancelling order {} cannot be told (HTTP {})", want.id, resp.status))),
        }
    }
    /// D-KUCOIN-HF-RETENTION: an adopted row's route. The classic order route always, the HF one on an HF account (when HF is
    /// admitted), each read for this id on the venue's own ticker for the row's pair, with its side and type: exactly one
    /// positive answer proves that route. A miss, an unreadable answer, both, or no ticker prove nothing; a failure worth
    /// retrying is its error (the row waits, excluded, either way).
    async fn origin_with(&self, r: &super::OrderRef) -> Result<super::Origin, VenueError> {
        use super::Origin;
        let Some(rec) = r.record.as_ref().filter(|_| r.adopted()) else { return Ok(Origin::Unproven) };
        let Some(symbol) = rec["symbol"].as_str() else { return Ok(Origin::Unproven) };
        let want = HfOrder { id: r.external_id.clone(), symbol: symbol.into(), sell: rec["sell"] == true, limit: rec["limit"] == true };
        let names = |o: &Value| o["id"].as_str() == Some(want.id.as_str()) && o["symbol"].as_str() == Some(symbol)
            && o["side"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(if want.sell { "sell" } else { "buy" }))
            && limit_type(&o["type"]).ok() == Some(want.limit);
        // The account decides which routes are asked: an HF account needs both (and HF admitted), a classic one the classic
        // route; a detection failure or UTA proves nothing.
        let hf_account = match self.account_mode().await {
            Ok(super::kucoin_account::AccountMode::ClassicLf) => false,
            Ok(super::kucoin_account::AccountMode::ClassicHf) if self.hf => true,
            _ => return Ok(Origin::Unproven),
        };
        let classic = match self.get_order(&want.id).await {
            Ok(found) => found.as_ref().is_some_and(names),
            Err(e @ VenueError::Transient(_)) => return Err(e),
            Err(VenueError::Rejected(m)) if RULES.is_throttle(&m) || RULES.is_transient(&m) => return Err(VenueError::Rejected(m)),
            Err(_) => false,
        };
        let hf = hf_account && self.hf_order(&want).await?.is_some();
        Ok(match (classic, hf) {
            (true, false) => Origin::Classic,
            (false, true) => Origin::Hf(serde_json::json!({ "family": "ClassicHf", "symbol": symbol, "sell": want.sell, "limit": want.limit, "host": self.base,
                "key_fingerprint": self.identity.as_ref().map_or(Value::Null, |i| i["key_fingerprint"].clone()), "deal_size": "0", "deal_funds": "0" })),
            _ => Origin::Unproven,
        })
    }

}
