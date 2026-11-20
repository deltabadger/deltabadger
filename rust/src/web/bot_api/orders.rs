//! app/services/bot_api/orders: MarketBuy, MarketSell, LimitBuy, LimitSell (with BotOrder for an order through a signal
//! bot) and Cancel, up to their first venue call (PLAN-A9 slice O1). MCP and REST both call these and format the `Res`.
//! Every refusal 2.x makes before a venue call is made here in 2.x's order, on the database alone; a dry run (MCP's
//! paper trading) answers here too. An order through a signal bot that passes every check is queued for the engine
//! (engine::api_orders, D-ORDER-QUEUED) and waited for (`wait`); so is a standalone one (slice O4: api_orders::
//! place_standalone, D-ORDER-AMBIGUOUS, D-ORDER-KRAKEN-QUOTE-SELL, D-ORDER-STANDALONE-LOG); a cancel that passes them is
//! sent by `send_cancel` (slice O2), the bot page's one bounded call (web::bot::cancel), on a logged standalone order's
//! stored route where there is one.
//!
//! Named divergences: D-ORDER-AMOUNT-VALIDATION (a standalone amount, price or amount_type 2.x hands to the venue is
//! refused with BotOrder's texts), D-ORDER-IBKR (a venue this build does not trade passes 2.x's checks, then answers
//! `not_ported`), D-ORDER-DRY-NO-READ (a standalone dry run reads no price) and D-ORDER-DRY-CANCEL (a dry-run cancel
//! answers its success text; 2.x's Exchange::Dryable raises). D-ORDER-MARKET-PRICE (REST): a market order's `price`,
//! which OrdersController permits and the market services do not take (2.x raises and leaves the key in progress), is
//! ignored. D-ORDER-CANCEL-TRANSPORT (a cancel the venue never answers is `cancel_failed` with the transport's message;
//! 2.x raises), D-ORDER-KUCOIN-FOREIGN (a bare order id on a KuCoin HF or UTA account is refused unsent) and A4's
//! D-CANCEL-REREAD-ALWAYS (a cancelled bot row is re-read by the engine whatever the venue answered, on either path) and
//! D-ORDER-ID-SAFE (a bare order id that is not a plain path segment is refused unsent).
use super::{present, refused, Domain, Res};
use crate::engine::model::{self, Ticker};
use crate::web::{bot::mcp_input::number, App, WebError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { MarketBuy, MarketSell, LimitBuy, LimitSell }

impl Kind {
    pub fn tool(name: &str) -> Option<Self> {
        Some(match name { "market_buy" => Self::MarketBuy, "market_sell" => Self::MarketSell, "limit_buy" => Self::LimitBuy, "limit_sell" => Self::LimitSell, _ => return None })
    }
    pub fn buy(self) -> bool { matches!(self, Self::MarketBuy | Self::LimitBuy) }
    pub fn limit(self) -> bool { matches!(self, Self::LimitBuy | Self::LimitSell) }
    fn side(self) -> &'static str { if self.buy() { "buy" } else { "sell" } }
    /// A buy spends quote, a sell sells base (BotOrder::DEFAULT_AMOUNT_TYPE and the services' `effective_type`).
    fn default_amount_type(self) -> &'static str { if self.buy() { "quote" } else { "base" } }
}

/// The surface the order came through: MCP's dates are in the caller's zone (ActionMCP::Current), REST's in UTC.
#[derive(Clone, Copy)]
pub struct Caller { pub user: i64, pub dry_run: bool, pub mcp: bool }

fn failure(status: Domain, code: &'static str, message: impl Into<String>) -> Res { Res::failure(status, code, message.into()) }
fn not_ported(what: &str) -> Res { failure(Domain::NotPorted, "not_ported", refused(what)) }
/// D-ORDER-IBKR.
const IBKR: &str = "orders through MCP or the REST API on this exchange";

/// `value.to_s` as 2.x writes a parameter: text as it is, a Float as Float#to_s.
pub(crate) fn to_s(v: &Value) -> String {
    match v { Value::Number(n) if n.is_f64() => crate::v2::float_to_s(n.as_f64().unwrap_or_default()), _ => crate::web::mcp::tools::str_value(v) }
}
fn arg<'a>(args: &'a Map<String, Value>, key: &str) -> &'a Value { args.get(key).unwrap_or(&Value::Null) }

/// Lookup.find_exchange: by name, case-insensitively, retired venues left out. (id, name, type)
fn find_exchange(c: &Connection, name: &Value) -> Result<Option<(i64, String, String)>, WebError> {
    if !present(name) { return Ok(None) }
    crate::web::mcp::reads::exchange(c, &to_s(name))
}
fn exchange_not_found(c: &Connection, name: &Value) -> Result<Res, WebError> {
    Ok(failure(Domain::NotFound, "exchange_not_found", format!("Exchange '{}' not found. Available: {}", to_s(name), crate::web::mcp::tradeable(c)?.join(", "))))
}
/// #710 (PLAN-710-6 §4.2): the account a write naming `exchange` acts on, by its posted `account_id`: Err is the answer
/// (a 404 for an account not the user's live one there, the 422 naming them when 2+ are live and none is named).
pub(crate) fn picked(c: &Connection, user: i64, exchange: i64, name: &str, posted: &Value) -> Result<Result<crate::identity::Account, Res>, WebError> {
    let posted = if present(posted) { match account_id(posted) { Some(id) => Some(id), None => return Ok(Err(account_not_found())) } } else { None };
    Ok(match crate::identity::pick(c, user, exchange, posted)? {
        crate::identity::Pick::One(account) => Ok(account),
        crate::identity::Pick::NotFound => Err(account_not_found()),
        crate::identity::Pick::Ambiguous(list) => Err(failure(Domain::ValidationFailed, "account_required", crate::identity::Pick::required(name, &list))),
    })
}
/// A posted `account_id` (#710): a whole integer, or a string of digits; anything else (`"12oops"`, `1.5`) names no account.
pub(crate) fn account_id(v: &Value) -> Option<i64> {
    match v { Value::Number(n) => n.as_i64(), Value::String(s) => { let t = s.trim(); if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) { t.parse().ok() } else { None } }, _ => None }
}
pub(crate) fn account_not_found() -> Res { failure(Domain::NotFound, "account_not_found", "Account not found.") }
/// Lookup.find_api_key: the user's correct trading key.
/// Whether `account` has a correct trading key (#710: the account an order is placed on or cancelled through).
fn has_correct_key(c: &Connection, account: &crate::identity::Account) -> Result<bool, WebError> {
    Ok(c.query_row(&format!("SELECT 1 FROM api_keys k WHERE {} AND k.key_type=0 AND k.status=1 LIMIT 1", account.keys("k")), [], |_| Ok(())).optional()?.is_some())
}
fn api_key_missing(exchange: &str) -> Res {
    failure(Domain::PermissionDenied, "api_key_missing", format!("No valid API key found for {exchange}. Please add an API key in Settings."))
}
/// Lookup.find_ticker: exactly one of the venue's tickers with these asset symbols.
fn find_ticker(c: &Connection, exchange: i64, base: &str, quote: &str) -> Result<Option<Ticker>, WebError> {
    let ids = c.prepare("SELECT t.id FROM tickers t JOIN assets b ON b.id=t.base_asset_id JOIN assets q ON q.id=t.quote_asset_id \
                         WHERE t.exchange_id=?1 AND b.symbol=?2 AND q.symbol=?3 LIMIT 2")?
        .query_map(rusqlite::params![exchange, base, quote], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    match ids.as_slice() { [id] => Ok(model::ticker_by_id(c, exchange, *id)?), _ => Ok(None) }
}
/// Lookup.untradable_refusal.
fn untradable(exchange: &str, ticker: &Ticker) -> Option<Res> {
    (!(ticker.available && ticker.trading_enabled)).then(|| failure(Domain::Conflict, "ticker_not_tradable",
        format!("Trading pair {}/{} is not tradable on {exchange}.", ticker.base_symbol, ticker.quote_symbol)))
}
/// Lookup.wash_sale_refusal: a live lock on the ticker's base asset while the rule is on. The date is the last locked day,
/// in the zone 2.x reads the deadline in.
fn wash_refusal(c: &Connection, now: chrono::DateTime<chrono::Utc>, who: Caller, ticker: &Ticker) -> Result<Option<Res>, WebError> {
    if crate::engine::wash::days(c, who.user).map_err(|e| WebError::Config(format!("{e:?}")))? == 0 { return Ok(None) }
    let until: Option<String> = c.query_row("SELECT buy_locked_until FROM wash_sale_locks WHERE user_id=?1 AND asset_id=?2 AND buy_locked_until > ?3 LIMIT 1",
        rusqlite::params![who.user, ticker.base_asset_id, crate::codec::format_time(now)], |r| r.get(0)).optional()?;
    let Some(until) = until else { return Ok(None) };
    // `(buy_locked_until - 1.day).to_date`: one calendar day before the deadline's date in that zone (a DST change
    // makes it differ from 24 hours before).
    let at = crate::codec::parse_time(&until).map_err(|e| WebError::Config(format!("wash lock deadline: {e:?}")))?;
    let date = if who.mcp {
        let zone: String = c.query_row("SELECT time_zone FROM users WHERE id=?1", [who.user], |r| r.get(0))?;
        crate::web::timezone::local(at, &zone).date_naive()
    } else { at.date_naive() };
    let day = date.pred_opt().ok_or_else(|| WebError::Config("wash lock deadline out of range".into()))?;
    Ok(Some(failure(Domain::Conflict, "wash_sale_locked", format!("{} is locked until {day} after a sale at a loss. Change this in account settings.", ticker.base_code))))
}
/// Whether this build places orders on the venue at all (D-ORDER-IBKR).
fn trades(kind: &str) -> bool { crate::venue::registry::row(kind).is_some_and(|r| r.trades) }

/// What `place` came to: an answer now, or an order the engine places (the row is committed; wake the engine, then `wait`).
pub enum Placed { Done(Res), Queued(Queued) }

/// An order queued for the engine, with what its answer repeats of the request.
/// `shown`: the account's label (#710), which the refusals name; `exchange` (the answer's field) stays the bare name.
pub struct Queued { pub id: i64, bot_id: Option<i64>, exchange: String, shown: String, pair: String, base: String, side: &'static str, amount: Value, amount_type: String,
                    /// A standalone order's: "market" or "limit", and a limit's price as sent (Null otherwise).
                    order_type: &'static str, price: Value }

/// MarketBuy/MarketSell/LimitBuy/LimitSell#call. `args`: the service's keyword arguments as the surface hands them over
/// (a missing key is nil); `bot_id`: None only for nil (REST hands over `params[:bot_id].to_s`, MCP the number).
pub fn place(c: &Connection, app: &App, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: Option<&Value>) -> Result<Placed, WebError> {
    place_at(c, app.now(), who, kind, args, bot_id)
}

/// `place` at `now` (also the grid-orders entry, parity::standalone_orders, at the scenario's time).
pub fn place_at(c: &Connection, now: chrono::DateTime<chrono::Utc>, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: Option<&Value>) -> Result<Placed, WebError> {
    Ok(match bot_id {
        Some(bot_id) if !kind.limit() => return through_bot(c, now, who, kind, args, bot_id),
        _ => standalone(c, now, who, kind, args, bot_id)?,
    })
}

fn standalone(c: &Connection, now: chrono::DateTime<chrono::Utc>, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: Option<&Value>) -> Result<Placed, WebError> {
    standalone_checked(c, now, who, kind, args, bot_id)
}

fn standalone_checked(c: &Connection, now: chrono::DateTime<chrono::Utc>, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: Option<&Value>) -> Result<Placed, WebError> {
    let done = |res: Res| Ok(Placed::Done(res));
    if let Some(bot_id) = bot_id {
        // Sent at all, however malformed: a limit order is refused, a market order goes through the bot.
        let _ = bot_id;
        return done(failure(Domain::ValidationFailed, "bot_limit_orders_unsupported", "A bot takes market orders only: a signal bot has no schedule on which a resting \
            order would be confirmed. Leave bot_id out to place the limit order on the account."));
    }
    let required: &[&str] = if kind.limit() { &["exchange_name", "base_asset", "quote_asset", "amount", "price"] } else { &["exchange_name", "base_asset", "quote_asset", "amount"] };
    let missing: Vec<&str> = required.iter().copied().filter(|k| !present(arg(args, k))).collect();
    if !missing.is_empty() {
        return done(failure(Domain::ValidationFailed, "missing_required_parameter", format!("Missing required parameter(s): {}.", missing.join(", "))));
    }
    let Some((exchange, name, venue)) = find_exchange(c, arg(args, "exchange_name"))? else { return exchange_not_found(c, arg(args, "exchange_name")).map(Placed::Done) };
    // #710: the posted account, or the exchange's while it has one live account (§4.3).
    let account = match picked(c, who.user, exchange, &name, arg(args, "account_id"))? { Ok(account) => account, Err(res) => return done(res) };
    let shown = crate::identity::label_or(c, account.id, &name)?.plain();
    if !has_correct_key(c, &account)? { return done(api_key_missing(&shown)) }
    let (base, quote) = (to_s(arg(args, "base_asset")).to_uppercase(), to_s(arg(args, "quote_asset")).to_uppercase());
    let Some(ticker) = find_ticker(c, exchange, &base, &quote)? else {
        return done(failure(Domain::NotFound, "pair_not_found", format!("Trading pair {base}/{quote} not found on {shown}.")));
    };
    if let Some(refusal) = untradable(&shown, &ticker) { return done(refusal) }
    if kind.buy() { if let Some(refusal) = wash_refusal(c, now, who, &ticker)? { return done(refusal) } }
    // D-ORDER-AMOUNT-VALIDATION: where 2.x hands these to the venue (or raises), BotOrder's refusals.
    if !number(arg(args, "amount")).is_some_and(|n| n.is_positive()) { return done(invalid_number("amount")) }
    let Some(amount_type) = amount_type(kind, args) else { return done(invalid_amount_type()) };
    if kind.limit() && !number(arg(args, "price")).is_some_and(|n| n.is_positive()) { return done(invalid_number("price")) }
    if !trades(&venue) { return done(not_ported(IBKR)) }
    let queued = Queued { id: 0, bot_id: None, exchange: name, shown, pair: format!("{base}/{quote}"), base: base.clone(), side: kind.side(), amount: arg(args, "amount").clone(),
                          amount_type, order_type: if kind.limit() { "limit" } else { "market" }, price: if kind.limit() { arg(args, "price").clone() } else { Value::Null } };
    // Exchange::Dryable#create_dry_order's answer; D-ORDER-DRY-NO-READ: no price is read for it.
    if who.dry_run { return done(placed(&queued, true, &format!("dry-order-{}", &uuid::Uuid::new_v4().to_string()[10..]))) }
    // PLAN-A9 O4: queued for the engine with the account's venue and pair as found here, and the key's version.
    let (Some(amount), price) = (number(arg(args, "amount")), if kind.limit() { number(arg(args, "price")) } else { None }) else { return done(invalid_number("amount")) };
    let supplied: Map<String, Value> = [("base_asset", json!(base)), ("quote_asset", json!(quote))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let side = if kind.buy() { crate::venue::Side::Buy } else { crate::venue::Side::Sell };
    let standalone = crate::engine::api_orders::Standalone { account, exchange_id: exchange, ticker_id: ticker.id, limit: kind.limit(), price, mcp: who.mcp };
    let new = crate::engine::api_orders::New::Standalone { user_id: who.user, side, amount: &amount, in_base: queued.amount_type == "base", supplied: &supplied, standalone };
    let id = crate::engine::api_orders::insert(c, &new, now).map_err(|e| WebError::Config(format!("{e:?}")))?;
    Ok(Placed::Queued(Queued { id, ..queued }))
}

/// MarketBuy/MarketSell/LimitBuy/LimitSell's success: `upstream` is `{order_id:}` on every venue (PLAN-A9 O4 r4 §1: a
/// String everywhere).
fn placed(q: &Queued, dry_run: bool, order_id: &str) -> Res {
    let mut data = Map::new();
    data.insert("dry_run".into(), json!(dry_run));
    data.insert("exchange".into(), json!(q.exchange));
    data.insert("pair".into(), json!(q.pair));
    data.insert("side".into(), json!(q.side));
    data.insert("order_type".into(), json!(q.order_type));
    data.insert("amount".into(), q.amount.clone());
    data.insert("amount_type".into(), json!(q.amount_type));
    if q.order_type == "limit" { data.insert("price".into(), q.price.clone()); }
    data.insert("upstream".into(), json!({ "order_id": order_id }));
    Res::success(Value::Object(data))
}

/// The standalone request's checks again, in the engine's claim (PLAN-A9 O4 §2.3 step 6): the key (Lookup.find_api_key),
/// the pair by its id (gone: `pair_not_found` with the pair the caller named), its tradability, and for a buy the wash-sale
/// lock dated in the request's surface's zone (r1 §5). 2.x's codes and texts.
pub fn standalone_late_refusal(c: &Connection, now: chrono::DateTime<chrono::Utc>, request: &crate::engine::api_orders::Request) -> Result<Option<Res>, WebError> {
    let Some(s) = &request.standalone else { return Err(WebError::Config("a bot request has no standalone refusal".into())) };
    let name: String = c.query_row("SELECT coalesce(name, '') FROM exchanges WHERE id=?1", [s.exchange_id], |r| r.get(0)).optional()?.unwrap_or_default();
    let name = crate::identity::label_or(c, s.account.id, &name)?.plain();
    if !has_correct_key(c, &s.account)? { return Ok(Some(api_key_missing(&name))) }
    let Some(ticker) = model::ticker_by_id(c, s.exchange_id, s.ticker_id).map_err(|e| WebError::Config(format!("{e:?}")))? else {
        let named = |k: &str| request.supplied.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        return Ok(Some(failure(Domain::NotFound, "pair_not_found", format!("Trading pair {}/{} not found on {name}.", named("base_asset"), named("quote_asset")))));
    };
    if let Some(refusal) = untradable(&name, &ticker) { return Ok(Some(refusal)) }
    if request.side == crate::venue::Side::Buy { return wash_refusal(c, now, Caller { user: request.user_id, dry_run: false, mcp: s.mcp }, &ticker) }
    Ok(None)
}

fn invalid_number(field: &str) -> Res { failure(Domain::ValidationFailed, "invalid_number", format!("{field} must be a number greater than 0.")) }
/// `amount_type.presence || DEFAULT_AMOUNT_TYPE.fetch(side)`, then one of the two strings exactly (BotOrder::AMOUNT_TYPES).
fn amount_type(kind: Kind, args: &Map<String, Value>) -> Option<String> {
    let given = arg(args, "amount_type");
    if !present(given) { return Some(kind.default_amount_type().to_string()) }
    given.as_str().filter(|s| matches!(*s, "quote" | "base")).map(str::to_string)
}
fn invalid_amount_type() -> Res { failure(Domain::ValidationFailed, "invalid_amount_type", "amount_type must be 'quote' or 'base'.") }

/// BotApi::Number.integer.
pub(crate) fn integer(v: &Value) -> Option<i64> {
    if let Some(f) = v.as_f64().filter(|_| v.is_f64()) { if !(f.is_finite() && f == f.floor()) { return None } }
    let n = number(v)?;
    let text = n.to_s_f();
    text.strip_suffix(".0").and_then(|whole| whole.parse().ok())
}

/// The bot BotOrder trades through: what its refusals read.
/// `shown`: the bot's account's label (#710), which the refusals name; `exchange` is the bare name the pair check compares.
struct Through { bot: model::Bot, exchange: String, shown: String, venue: String, ticker: Option<Ticker> }
fn shown(c: &Connection, bot: &model::Bot) -> Result<String, WebError> { model::exchange_name(c, bot).map_err(|e| WebError::Config(format!("{e:?}"))) }

/// `@user.bots.not_deleted.find_by(id:)`, with what BotOrder's refusals read.
fn find_bot(c: &Connection, user: i64, id: i64) -> Result<Option<Through>, WebError> {
    let found: Option<(String, String)> = c.query_row("SELECT coalesce(e.name, ''), coalesce(e.type, '') FROM bots b LEFT JOIN exchanges e ON e.id=b.exchange_id \
                                                       WHERE b.id=?1 AND b.user_id=?2 AND b.status!=?3",
        rusqlite::params![id, user, crate::enums::BotStatus::Deleted as i64], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((exchange, venue)) = found else { return Ok(None) };
    let bot = model::load_bot(c, id).map_err(|e| WebError::Config(format!("{e:?}")))?;
    let ticker = if bot.bot_type == model::SIGNAL_TYPE { model::ticker_for(c, &bot).map_err(|e| WebError::Config(format!("{e:?}")))? } else { None };
    Ok(Some(Through { shown: shown(c, &bot)?, bot, exchange, venue, ticker }))
}
fn bot_not_found() -> Res { failure(Domain::NotFound, "bot_not_found", "Bot not found.") }

/// BotOrder#call, up to its first venue call (`market_refusal`); a live order is queued for the engine.
/// Also the grid-signal `api` axis' entry (parity::api_orders), at the scenario's time.
pub fn through_bot(c: &Connection, now: chrono::DateTime<chrono::Utc>, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: &Value) -> Result<Placed, WebError> {
    let done = |res: Res| Ok(Placed::Done(res));
    let Some(amount) = number(arg(args, "amount")).filter(|n| n.is_positive()) else { return done(invalid_number("amount")) };
    let Some(amount_type) = amount_type(kind, args) else { return done(invalid_amount_type()) };
    let Some(id) = integer(bot_id).filter(|id| *id > 0) else { return done(failure(Domain::ValidationFailed, "invalid_number", "bot_id must be a whole number.")) };
    let Some(through) = find_bot(c, who.user, id)? else { return done(bot_not_found()) };
    if let Some(refusal) = bot_refusal(c, &through, args)? { return done(refusal) }
    if let Some(refusal) = venue_refusal(c, now, who, kind, &through)? { return done(refusal) }
    if !trades(&through.venue) { return done(not_ported(IBKR)) }
    let (base, quote) = pair(c, &through)?;
    let mut queued = Queued { id: 0, bot_id: Some(id), exchange: through.exchange.clone(), shown: through.shown.clone(), pair: format!("{base}/{quote}"), base, side: kind.side(), amount: arg(args, "amount").clone(), amount_type,
                              order_type: "market", price: Value::Null };
    if who.dry_run { return done(submitted(&queued, true, None, None)) }
    // What validation saw goes with the request: the engine re-checks the bot against the supplied fields, and places
    // nothing under another key (PLAN-A9 r1 §4, r2 §1).
    let supplied: Map<String, Value> = ["exchange_name", "base_asset", "quote_asset"].into_iter()
        .map(|k| (k.to_string(), Some(arg(args, k)).filter(|v| present(v)).map_or(Value::Null, |v| json!(to_s(v))))).collect();
    let side = if kind.buy() { crate::venue::Side::Buy } else { crate::venue::Side::Sell };
    let new = crate::engine::api_orders::New::Bot { user_id: who.user, bot: &through.bot, side, amount: &amount, in_base: queued.amount_type == "base", supplied: &supplied };
    queued.id = crate::engine::api_orders::insert(c, &new, now).map_err(|e| WebError::Config(format!("{e:?}")))?;
    Ok(Placed::Queued(queued))
}

/// BotOrder#success.
fn submitted(q: &Queued, dry_run: bool, transaction_id: Option<i64>, order_id: Option<&str>) -> Res {
    Res::success(json!({
        "dry_run": dry_run, "bot_id": q.bot_id, "transaction_id": transaction_id, "order_id": order_id, "exchange": q.exchange, "pair": q.pair,
        "side": q.side, "order_type": "market", "amount": q.amount, "amount_type": q.amount_type,
    }))
}

/// BotOrder's late `bot_refusal`, on `bot` as the engine read it to size the order (`supplied`: the caller's pair fields).
pub fn late_refusal(c: &Connection, bot: &model::Bot, request: &crate::engine::api_orders::Request) -> Result<Option<Res>, WebError> {
    if bot.user_id != request.user_id || bot.status == crate::enums::BotStatus::Deleted { return Ok(Some(bot_not_found())) }
    let (exchange, venue): (String, String) = c.query_row("SELECT coalesce(name, ''), coalesce(type, '') FROM exchanges WHERE id=?1", [bot.exchange_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?.unwrap_or_default();
    let ticker = if bot.bot_type == model::SIGNAL_TYPE { model::ticker_for(c, bot).map_err(|e| WebError::Config(format!("{e:?}")))? } else { None };
    bot_refusal(c, &Through { bot: bot.clone(), exchange, shown: shown(c, bot)?, venue, ticker }, &request.supplied)
}

/// 2.x's status for each refusal code the engine answers with (`Outcome::Refused`).
const REFUSALS: &[(&str, Domain)] = &[
    ("bot_not_found", Domain::NotFound), ("bot_not_orderable", Domain::ValidationFailed), ("bot_not_running", Domain::Conflict),
    ("placement_unresolved", Domain::Conflict), ("pair_not_found", Domain::NotFound), ("bot_pair_mismatch", Domain::ValidationFailed),
    ("ticker_not_tradable", Domain::Conflict), ("market_closed", Domain::Conflict), ("exchange_unavailable", Domain::UpstreamFailed),
    ("api_key_missing", Domain::PermissionDenied), ("wash_sale_locked", Domain::Conflict),
];

/// BotOrder#venue_busy.
fn venue_busy(exchange: &str) -> Res { failure(Domain::Conflict, "venue_busy", format!("{exchange} is busy with another order from your bots. Nothing was placed; try again.")) }
pub(crate) fn ambiguous(errors: &str, exchange: &str) -> Res {
    failure(Domain::UpstreamFailed, "placement_ambiguous", format!("The order may or may not have been placed: {errors}. Check {exchange} before sending it again."))
}

/// BotOrder#answer: what the engine's outcome tells the caller (PLAN-A9 §2.4).
pub fn answer(q: &Queued, outcome: crate::engine::api_orders::Outcome) -> Res {
    use crate::engine::api_orders::Outcome;
    let sentence = |errors: &[String]| crate::v2::to_sentence(errors);
    match outcome {
        Outcome::Submitted { transaction_id, order_id, pair } => {
            let mut res = submitted(q, false, transaction_id, Some(&order_id));
            if let Some(pair) = pair { res.data["pair"] = json!(pair); }
            res
        }
        Outcome::Skipped => failure(Domain::ValidationFailed, "below_minimum_amount", format!("The order is below {}'s minimum for {}.", q.shown, q.pair)),
        Outcome::Unresolved => placement_unresolved(q.bot_id.unwrap_or_default(), &q.shown),
        Outcome::Locked => failure(Domain::Conflict, "wash_sale_locked", format!("{} is locked after a sale at a loss.", q.base)),
        Outcome::Ambiguous { errors } => ambiguous(&sentence(&errors), &q.shown),
        // r3 §2: a standalone failure is MarketBuy's `errors.join(', ')`; a bot order's is BotOrder's to_sentence.
        Outcome::Failed { errors } if q.bot_id.is_none() => failure(Domain::UpstreamFailed, "order_failed", format!("Order failed: {}", errors.join(", "))),
        Outcome::Failed { errors } => failure(Domain::UpstreamFailed, "order_failed", format!("Order failed: {}", sentence(&errors))),
        Outcome::Placed { order_id } => placed(q, false, &order_id),
        Outcome::Refused { code, message } => match REFUSALS.iter().find(|(c, _)| *c == code) {
            Some((code, status)) => failure(*status, code, message),
            None => failure(Domain::UpstreamFailed, "order_failed", format!("Order failed: {message}")),
        },
        // D-ORDER-API-CAP.
        Outcome::CapReached { left } => failure(Domain::Conflict, "quote_amount_limit_reached",
            format!("Bot {} has {left} {} left of its spending limit.", q.bot_id.unwrap_or_default(), q.pair.split_once('/').map_or("", |(_, quote)| quote))),
        Outcome::Busy => venue_busy(&q.shown),
        Outcome::Unknown => ambiguous("the engine restarted while it was placing it", &q.shown),
    }
}

/// What a caller is told when its request's answer could not be read (a database error while waiting): it may have been placed.
pub fn unanswered(q: &Queued) -> Res { ambiguous("its answer could not be read", &q.shown) }

/// How long a taken request may go unanswered before the caller is told it may have been placed (D-ORDER-WAIT-CAP): past
/// the last instant a send may start, plus two whole venue calls (the -1021 one more attempt) and a margin.
pub fn answer_cap() -> std::time::Duration {
    std::time::Duration::from_secs(crate::engine::api_orders::SEND_SECONDS as u64) + 2 * crate::venue::http::TOTAL_TIMEOUT + std::time::Duration::from_secs(5)
}

/// BotOrder's wait for the venue's lease, as a wait for the engine (D-ORDER-QUEUED): polled every LEASE_POLL. A request
/// no pass took within LEASE_WAIT is withdrawn (`venue_busy`, nothing placed); one taken and unanswered at the cap is
/// `placement_ambiguous`. With no engine attached it is withdrawn at once.
pub async fn wait(app: &App, q: &Queued) -> Result<Res, WebError> {
    use crate::engine::api_orders;
    let start = tokio::time::Instant::now();
    let window = start + std::time::Duration::from_secs(api_orders::WINDOW_SECONDS as u64);
    let cap = start + answer_cap();
    let id = q.id;
    let engine = app.engine_attached();
    loop {
        let now = tokio::time::Instant::now();
        let withdraw = !engine || now >= window;
        let (state, outcome) = app.db(move |c| {
            if withdraw && api_orders::withdraw(c, id).map_err(|e| WebError::Config(format!("{e:?}")))? {
                api_orders::forget(c, id).map_err(|e| WebError::Config(format!("{e:?}")))?;
                return Ok(("withdrawn".to_string(), None));
            }
            let found = api_orders::state(c, id).map_err(|e| WebError::Config(format!("{e:?}")))?;
            if found.as_ref().is_some_and(|(state, _)| state == "done") { api_orders::forget(c, id).map_err(|e| WebError::Config(format!("{e:?}")))?; }
            found.ok_or_else(|| WebError::Config(format!("api order {id} vanished")))
        }).await?;
        match (state.as_str(), outcome) {
            ("done", Some(outcome)) => return Ok(answer(q, outcome)),
            ("withdrawn", _) => return Ok(venue_busy(&q.shown)),
            _ if now >= cap => return Ok(ambiguous("the exchange did not answer in time", &q.shown)),
            _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
}

/// The bot's pair as asset symbols (BotOrder#base, #quote).
fn pair(c: &Connection, t: &Through) -> Result<(String, String), WebError> {
    let symbol = |id: Option<i64>| -> Result<String, WebError> {
        Ok(match id { Some(id) => c.query_row("SELECT coalesce(symbol, '') FROM assets WHERE id=?1", [id], |r| r.get(0)).optional()?.unwrap_or_default(), None => String::new() })
    };
    Ok((symbol(t.bot.asset_ids().first().copied())?, symbol(t.bot.quote_asset_id())?))
}

/// BotOrder#bot_refusal (the bot was found).
fn bot_refusal(c: &Connection, t: &Through, args: &Map<String, Value>) -> Result<Option<Res>, WebError> {
    let id = t.bot.id;
    if t.bot.bot_type != model::SIGNAL_TYPE {
        return Ok(Some(failure(Domain::ValidationFailed, "bot_not_orderable", format!("Bot {id} is not a signal bot. Only a signal bot takes orders; every other type places its own."))));
    }
    if !crate::enums::BOT_WORKING.contains(&t.bot.status) {
        return Ok(Some(failure(Domain::Conflict, "bot_not_running", format!("Bot {id} is not running. Start it to place orders through it."))));
    }
    // Bots::Signal#placement_unresolved?: the marker only (an intent this build keeps in flight is the engine's to wait on).
    if t.bot.transient.get(crate::engine::signal::MARKER_KEY).is_some_and(Value::is_object) { return Ok(Some(placement_unresolved(id, &t.shown))) }
    let Some(ticker) = &t.ticker else { return Ok(Some(failure(Domain::NotFound, "pair_not_found", format!("Bot {id} has no tradable pair on its exchange.")))) };
    // BotOrder#pair_mismatch: a field that was sent must agree, case-insensitively.
    let (base, quote) = pair(c, t)?;
    let wrong: Vec<&str> = [("exchange_name", &t.exchange), ("base_asset", &base), ("quote_asset", &quote)].into_iter()
        .filter(|(key, actual)| { let sent = arg(args, key); present(sent) && to_s(sent).to_lowercase() != actual.to_lowercase() })
        .map(|(key, _)| key).collect();
    if !wrong.is_empty() {
        return Ok(Some(failure(Domain::ValidationFailed, "bot_pair_mismatch", format!("Bot {id} trades {base}/{quote} on {}; {} say otherwise. Leave them out to use the bot's pair.", t.shown, wrong.join(", ")))));
    }
    Ok(untradable(&t.shown, ticker))
}

fn placement_unresolved(id: i64, exchange: &str) -> Res {
    failure(Domain::Conflict, "placement_unresolved", format!("An earlier order from bot {id} may or may not have been placed; check {exchange} and resolve it on the bot page."))
}

/// BotOrder#venue_refusal: a buy's wash-sale lock, then the stored key (pending is asked first).
fn venue_refusal(c: &Connection, now: chrono::DateTime<chrono::Utc>, who: Caller, kind: Kind, t: &Through) -> Result<Option<Res>, WebError> {
    if kind.buy() { if let Some(ticker) = &t.ticker { if let Some(refusal) = wash_refusal(c, now, who, ticker)? { return Ok(Some(refusal)) } } }
    let status: Option<i64> = c.query_row(&format!("SELECT status FROM api_keys k WHERE {} AND key_type=0 ORDER BY id LIMIT 1", crate::identity::bot_keys(c, t.bot.id, "k")?), [], |r| r.get(0)).optional()?;
    Ok(match status {
        Some(s) if s == crate::enums::ApiKeyStatus::PendingActivation as i64 => Some(failure(Domain::Conflict, "api_key_pending", format!("The API key for {} is still pending activation.", t.shown))),
        Some(s) if s == crate::enums::ApiKeyStatus::Correct as i64 => None,
        _ => Some(api_key_missing(&t.shown)),
    })
}

/// A cancel's outcome before the venue: 2.x's answer, or the one cancel to send (`send_cancel`).
pub enum Cancel { Done(Res), Send(CancelJob) }
/// What a cancel past every check sends, and its success answer.
pub struct CancelJob { target: Target, answer: Value }
impl CancelJob {
    /// The account the cancel is sent through (None: a bot row's, sent through its bot's venue).
    pub fn account(&self) -> Option<crate::identity::Account> { match &self.target { Target::Log { account, .. } | Target::Key { account, .. } => Some(*account), Target::Row { .. } => None } }
}
enum Target {
    /// Transaction#cancel: the row's order on its bot's venue, then the engine's re-read.
    Row { bot: i64, tx: i64, external: String },
    /// `exchange.cancel_order(order_id:)` on the user's key; `row`: the user's bot row with that order id (bot, tx), whose
    /// stored order (OrderRef::load) is cancelled and re-read (r3).
    Key { account: crate::identity::Account, external: String, row: Option<(i64, i64)> },
    /// A standalone order this build placed (the retained log, D-ORDER-STANDALONE-LOG): sent on its stored route, any
    /// KuCoin family (`record`), with no re-read (there is no row).
    Log { account: crate::identity::Account, order: crate::venue::OrderRef },
}

fn cancel_failed(why: &str) -> Res { failure(Domain::UpstreamFailed, "cancel_failed", format!("Cancel failed: {why}")) }

/// BotApi::Orders::Cancel#call, up to its venue call.
pub fn cancel(c: &Connection, who: Caller, order_id: &Value, exchange_name: &Value, account_id: &Value) -> Result<Cancel, WebError> {
    if !present(order_id) { return Ok(Cancel::Done(failure(Domain::ValidationFailed, "missing_required_parameter", "Missing required parameter(s): order_id."))) }
    let text = to_s(order_id);
    // `\A\d+\z`, then `@user.transactions.submitted.open.find_by(id: to_i)`: a bot's row, whatever the bot's status.
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        // (id, bot, external id, "base/quote", side, exchange name, exchange type)
        type Row = (i64, i64, String, String, String, String, String);
        let row: Option<Row> = match text.parse::<i64>() {
            Ok(id) => c.query_row(&format!("SELECT t.id, t.bot_id, coalesce({}, ''), coalesce(t.base, '') || '/' || coalesce(t.quote, ''), \
                                   CASE t.side WHEN 0 THEN 'buy' WHEN 1 THEN 'sell' ELSE '' END, coalesce(e.name, ''), coalesce(e.type, '') FROM transactions t \
                                   JOIN bots b ON b.id=t.bot_id LEFT JOIN exchanges e ON e.id=t.exchange_id WHERE b.user_id=?1 AND t.id=?2 AND t.status=0 AND t.external_status=1", crate::venue_ids::order_id(c, "t")?),
                [who.user, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).optional()?,
            Err(_) => None,
        };
        if let Some((id, bot, external, pair, side, exchange, venue)) = row {
            // Transaction#cancel refuses an imported row before asking the venue.
            if external.starts_with("imported_") {
                return Ok(Cancel::Done(cancel_failed(&crate::web::i18n::text("en", "bot.messages.cannot_cancel_imported_order", &[]))));
            }
            // D-CANCEL-NO-EXTERNAL-ID, as the bot page: a row with no order id has nothing to name, so nothing is sent.
            if external.is_empty() { return Ok(Cancel::Done(cancel_failed("the order has no exchange order id"))) }
            if !trades(&venue) { return Ok(Cancel::Done(not_ported(IBKR))) }
            let answer = json!({ "dry_run": who.dry_run, "id": id, "exchange": exchange, "pair": pair, "side": side, "cancelled": true });
            if who.dry_run { return Ok(Cancel::Done(Res::success(answer))) }
            return Ok(Cancel::Send(CancelJob { target: Target::Row { bot, tx: id, external }, answer }));
        }
    }
    if !present(exchange_name) {
        return Ok(Cancel::Done(failure(Domain::ValidationFailed, "exchange_name_required", "Exchange name is required when cancelling by exchange order ID.")));
    }
    let Some((exchange, name, venue)) = find_exchange(c, exchange_name)? else { return exchange_not_found(c, exchange_name).map(Cancel::Done) };
    // #710 (§4.3): the account the venue id is looked up in; with 2+ live accounts one must be named, even when only one
    // of them holds the id.
    let picked = match picked(c, who.user, exchange, &name, account_id)? { Ok(account) => account, Err(res) => return Ok(Cancel::Done(res)) };
    // r3: a stored order beats a bare id. The user's bot rows first (the latest with this venue and order id); then the
    // retained standalone order log (PLAN-A9 O4); then the bare id. #710: each is cancelled through the account it was
    // placed on (a bare id through the exchange's number 1), whose key is the one checked.
    let row: Option<(i64, i64)> = c.query_row(&format!("SELECT t.bot_id, t.id FROM transactions t JOIN bots b ON b.id=t.bot_id \
                                               WHERE b.user_id=?1 AND t.exchange_id=?2 AND {o}=?3 AND substr({o}, 1, 9) != 'imported_' AND {mine} ORDER BY t.id DESC LIMIT 1",
                                               o = crate::venue_ids::order_id(c, "t")?, mine = picked.orders(c, "t")?),
        rusqlite::params![who.user, exchange, text], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let log = if row.is_none() { crate::engine::api_orders::log_order(c, who.user, exchange, &text, picked.id).map_err(|e| WebError::Config(format!("{e:?}")))?
                                     .filter(|(account, _, _)| account.id == picked.id || picked.id.is_none()) } else { None };
    let account = match (&row, &log) {
        (Some((_, tx)), _) => crate::identity::Account::of_order(c, *tx)?,
        (None, Some((account, _, _))) => Some(*account),
        (None, None) => Some(picked),
    };
    let Some(account) = account else { return Ok(Cancel::Done(api_key_missing(&crate::identity::label_or(c, picked.id, &name)?.plain()))) };
    if !has_correct_key(c, &account)? { return Ok(Cancel::Done(api_key_missing(&crate::identity::label_or(c, account.id, &name)?.plain()))) }
    if !trades(&venue) { return Ok(Cancel::Done(not_ported(IBKR))) }
    let answer = json!({ "dry_run": who.dry_run, "external_id": order_id, "exchange": name, "cancelled": true });
    if who.dry_run { return Ok(Cancel::Done(Res::success(answer))) }
    if let Some((_, identity, snapshot)) = log {
        // r1 §4: the route from what was stored at the claim, never from the ticker row (it may be gone or renamed since).
        let mut intent = identity.unwrap_or_else(|| json!({}));
        for key in ["symbol", "side", "limit"] { intent[key] = snapshot[key].clone(); }
        let order = crate::venue::OrderRef { external_id: text, record: crate::venue::kucoin::record_of(&intent) };
        return Ok(Cancel::Send(CancelJob { target: Target::Log { account, order }, answer }));
    }
    // D-ORDER-ID-SAFE: a bare id becomes a path segment on the venue (`DELETE /v2/orders/<id>`), so one that could leave it
    // (`../positions/X` would be Alpaca's position liquidation) is refused and nothing is sent. Every venue's order ids are
    // letters, digits, '-' and '_'. 2.x interpolates it.
    if row.is_none() && !(text.len() <= 128 && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')) {
        return Ok(Cancel::Done(failure(Domain::ValidationFailed, "invalid_order_id", "order_id must be an exchange order id: letters, digits, '-' and '_'.")));
    }
    Ok(Cancel::Send(CancelJob { target: Target::Key { account, external: text, row }, answer }))
}

/// The one bounded cancel a `CancelJob` names, on a connection of its own (no lock held while the venue answers), and its
/// answer. A refusal is 2.x's `cancel_failed` with the venue's errors; no answer at all (a transport failure, the timeout)
/// is the same failure with the transport's message (D-ORDER-CANCEL-TRANSPORT: 2.x raises).
pub async fn send_cancel(app: &App, job: CancelJob) -> Result<Res, WebError> {
    use crate::web::bot::cancel::{self as page, Sent};
    page::on_own_connection(app, move |c, app, runtime| {
        let factory = page::live_factory(c, app);
        let sent = match job.target {
            Target::Row { bot, tx, external } => page::send_for_row(c, app, &factory, runtime, &model::load_bot(c, bot)?, tx, &external)?,
            Target::Log { account, order } => {
                let venue = crate::engine::run::venue_for_key(c, &factory, &app.cipher, account)?;
                page::send(runtime, &venue, &order)
            }
            Target::Key { account, external, row } => {
                let venue = crate::engine::run::venue_for_key(c, &factory, &app.cipher, account)?;
                match row {
                    Some((bot, tx)) => {
                        let order = crate::venue::OrderRef::load(c, tx, &external)?;
                        let sent = page::send(runtime, &venue, &order);
                        page::leave_reread(c, app, bot, tx)?;
                        sent
                    }
                    None => {
                        if let Some(why) = foreign_refusal(runtime, &venue, &external) { return Ok(cancel_failed(&why)) }
                        page::send(runtime, &venue, &crate::venue::OrderRef { external_id: external, record: None })
                    }
                }
            }
        };
        Ok(match sent {
            Sent::Cancelled => Res::success(job.answer),
            Sent::Refused { errors, .. } => cancel_failed(&errors.join(", ")),
            Sent::Unanswered(why) => cancel_failed(&why),
        })
    }).await
}

/// D-ORDER-KUCOIN-FOREIGN: an order id with no stored route is sent on KuCoin's classic route, the only one 2.x has. On
/// an HF or UTA account (the key's resolved mode) that route cannot reach the order, so nothing is sent; nor when the mode
/// cannot be read. None on a classic account and on every other venue.
fn foreign_refusal<V: crate::venue::Venue>(runtime: &tokio::runtime::Handle, venue: &V, external: &str) -> Option<String> {
    use crate::venue::kucoin_account::AccountMode;
    match runtime.block_on(tokio::time::timeout(crate::venue::http::TOTAL_TIMEOUT, venue.kucoin_mode())) {
        Ok(None | Some(Ok(AccountMode::ClassicLf))) => None,
        Ok(Some(Ok(AccountMode::ClassicHf | AccountMode::Uta))) => Some(format!("Order {external} was not placed through Deltabadger; cancel it on KuCoin.")),
        Ok(Some(Err(e))) => Some(e.message()),
        Err(_) => Some(format!("the exchange did not answer within {} seconds", crate::venue::http::TOTAL_TIMEOUT.as_secs())),
    }
}

/// The MCP tools' texts (MarketBuyTool, MarketSellTool, LimitBuyTool, LimitSellTool, CancelOrderTool): `[DRY RUN] ` before
/// every answer under paper trading, a refusal as the tool's error. `args` are the tool's arguments as MCP cast them.
pub fn mcp_text(name: &str, args: &Value, dry_run: bool, res: &Res) -> (String, bool) {
    let prefix = if dry_run { "[DRY RUN] " } else { "" };
    if let Some(code) = res.code {
        let message = if code == "exchange_name_required" && name == "cancel_order" {
            "Exchange name is required when cancelling by exchange order ID. Use: cancel_order(order_id: '...', exchange_name: 'Alpaca')".to_string()
        } else { res.message.clone() };
        return (format!("{prefix}{message}"), true);
    }
    let d = &res.data;
    let s = |key: &str| crate::web::mcp::tools::str_value(&d[key]);
    // MCP casts a `number` property to a Float, so a whole amount prints as `10.0`.
    let shown = |key: &str| match &args[key] { Value::Number(n) => crate::v2::float_to_s(n.as_f64().unwrap_or_default()), v => to_s(v) };
    if name == "cancel_order" {
        return (if d["id"].is_null() { format!("{prefix}Order {} cancellation submitted on {}.", s("external_id"), s("exchange")) }
            else { format!("{prefix}Order #{} ({} {} on {}) cancellation submitted.", d["id"], s("side").to_uppercase(), s("pair"), s("exchange")) }, false);
    }
    let Some(kind) = Kind::tool(name) else { return ("An unexpected error occurred.".into(), true) };
    // The upstream answer as 2.x's Hash#inspect writes it (only a dry run's `{order_id: "…"}` reaches here).
    let upstream = d["upstream"].as_object().map(|m| format!("{{{}}}", m.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", "))).unwrap_or_default();
    if kind.limit() {
        return (format!("{prefix}Limit {} order placed on {}: {} @ {}. {upstream}", kind.side(), s("exchange"), s("pair"), shown("price")), false);
    }
    let (base, quote) = s("pair").split_once('/').map(|(b, q)| (b.to_string(), q.to_string())).unwrap_or_default();
    let currency = if kind.buy() { if s("amount_type") == "quote" { quote } else { base } } else if s("amount_type") == "base" { base } else { quote };
    let (placed, detail) = if d["bot_id"].is_null() { (format!("placed on {}", s("exchange")), upstream) }
        else if d["dry_run"] == true { (format!("placed by bot {} on {}", d["bot_id"], s("exchange")), "Nothing was placed or recorded.".to_string()) }
        else { (format!("placed by bot {} on {}", d["bot_id"], s("exchange")), format!("Order {}, transaction {}.", s("order_id"), s("transaction_id"))) };
    (format!("{prefix}Market {} order {placed}: {} {currency} of {}. {detail}", kind.side(), shown("amount"), s("pair")), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_is_bot_api_number_integer() {
        for (v, want) in [(json!(12.0), Some(12)), (json!(12.5), None), (json!("12"), Some(12)), (json!("12.0"), Some(12)), (json!("12abc"), None),
                          (json!(" 7 "), Some(7)), (json!("1e2"), None), (json!(-3.0), None), (json!("true"), None), (json!(1.0000000000000002), None)] {
            assert_eq!(integer(&v), want, "{v}");
        }
    }
}
