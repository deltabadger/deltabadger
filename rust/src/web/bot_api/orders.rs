//! app/services/bot_api/orders: MarketBuy, MarketSell, LimitBuy, LimitSell (with BotOrder for an order through a signal
//! bot) and Cancel, up to their first venue call (PLAN-A9 slice O1). MCP and REST both call these and format the `Res`.
//! Every refusal Rails makes before a venue call is made here in Rails' order, on the database alone; a dry run (MCP's
//! paper trading) answers here too. An order that passes every check is `not_ported` until the engine places it (O3, O4);
//! a cancel that passes them is sent by `send_cancel` (slice O2), the bot page's one bounded call (web::bot::cancel).
//!
//! Named divergences: D-ORDER-AMOUNT-VALIDATION (a standalone amount, price or amount_type Rails hands to the venue is
//! refused with BotOrder's texts), D-ORDER-IBKR (a venue this build does not trade passes Rails' checks, then answers
//! `not_ported`), D-ORDER-DRY-NO-READ (a standalone dry run reads no price) and D-ORDER-DRY-CANCEL (a dry-run cancel
//! answers its success text; Rails' Exchange::Dryable raises). D-ORDER-MARKET-PRICE (REST): a market order's `price`,
//! which OrdersController permits and the market services do not take (Rails raises and leaves the key in progress), is
//! ignored. D-ORDER-CANCEL-TRANSPORT (a cancel the venue never answers is `cancel_failed` with the transport's message;
//! Rails raises), D-ORDER-KUCOIN-FOREIGN (a bare order id on a KuCoin HF or UTA account is refused unsent) and A4's
//! D-CANCEL-REREAD-ALWAYS (a cancelled bot row is re-read by the engine whatever the venue answered, on either path).
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
const PLACING: &str = "orders placed through MCP or the REST API";
/// D-ORDER-IBKR.
const IBKR: &str = "orders through MCP or the REST API on this exchange";

/// `value.to_s` as Ruby writes a parameter: text as it is, a Float as Float#to_s.
pub(crate) fn to_s(v: &Value) -> String {
    match v { Value::Number(n) if n.is_f64() => crate::ruby::float_to_s(n.as_f64().unwrap_or_default()), _ => crate::web::mcp::tools::str_value(v) }
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
/// Lookup.find_api_key: the user's correct trading key.
fn has_correct_key(c: &Connection, user: i64, exchange: i64) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 AND status=1 LIMIT 1", [user, exchange], |_| Ok(())).optional()?.is_some())
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
/// in the zone Rails reads the deadline in.
fn wash_refusal(c: &Connection, app: &App, who: Caller, ticker: &Ticker) -> Result<Option<Res>, WebError> {
    if crate::engine::wash::days(c, who.user).map_err(|e| WebError::Config(format!("{e:?}")))? == 0 { return Ok(None) }
    let until: Option<String> = c.query_row("SELECT buy_locked_until FROM wash_sale_locks WHERE user_id=?1 AND asset_id=?2 AND buy_locked_until > ?3 LIMIT 1",
        rusqlite::params![who.user, ticker.base_asset_id, crate::codec::format_time(app.now())], |r| r.get(0)).optional()?;
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

/// MarketBuy/MarketSell/LimitBuy/LimitSell#call. `args`: the service's keyword arguments as the surface hands them over
/// (a missing key is nil); `bot_id`: None only for nil (REST hands over `params[:bot_id].to_s`, MCP the number).
pub fn place(c: &Connection, app: &App, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: Option<&Value>) -> Result<Res, WebError> {
    if let Some(bot_id) = bot_id {
        // Sent at all, however malformed: a limit order is refused, a market order goes through the bot.
        if kind.limit() {
            return Ok(failure(Domain::ValidationFailed, "bot_limit_orders_unsupported", "A bot takes market orders only: a signal bot has no schedule on which a resting \
                order would be confirmed. Leave bot_id out to place the limit order on the account."));
        }
        return through_bot(c, app, who, kind, args, bot_id);
    }
    let required: &[&str] = if kind.limit() { &["exchange_name", "base_asset", "quote_asset", "amount", "price"] } else { &["exchange_name", "base_asset", "quote_asset", "amount"] };
    let missing: Vec<&str> = required.iter().copied().filter(|k| !present(arg(args, k))).collect();
    if !missing.is_empty() {
        return Ok(failure(Domain::ValidationFailed, "missing_required_parameter", format!("Missing required parameter(s): {}.", missing.join(", "))));
    }
    let Some((exchange, name, venue)) = find_exchange(c, arg(args, "exchange_name"))? else { return exchange_not_found(c, arg(args, "exchange_name")) };
    if !has_correct_key(c, who.user, exchange)? { return Ok(api_key_missing(&name)) }
    let (base, quote) = (to_s(arg(args, "base_asset")).to_uppercase(), to_s(arg(args, "quote_asset")).to_uppercase());
    let Some(ticker) = find_ticker(c, exchange, &base, &quote)? else {
        return Ok(failure(Domain::NotFound, "pair_not_found", format!("Trading pair {base}/{quote} not found on {name}.")));
    };
    if let Some(refusal) = untradable(&name, &ticker) { return Ok(refusal) }
    if kind.buy() { if let Some(refusal) = wash_refusal(c, app, who, &ticker)? { return Ok(refusal) } }
    // D-ORDER-AMOUNT-VALIDATION: where Rails hands these to the venue (or raises), BotOrder's refusals.
    if !number(arg(args, "amount")).is_some_and(|n| n.is_positive()) { return Ok(invalid_number("amount")) }
    let Some(amount_type) = amount_type(kind, args) else { return Ok(invalid_amount_type()) };
    if kind.limit() && !number(arg(args, "price")).is_some_and(|n| n.is_positive()) { return Ok(invalid_number("price")) }
    if !trades(&venue) { return Ok(not_ported(IBKR)) }
    if !who.dry_run { return Ok(not_ported(PLACING)) }
    // Exchange::Dryable#create_dry_order's answer; D-ORDER-DRY-NO-READ: no price is read for it.
    let mut data = Map::new();
    data.insert("dry_run".into(), json!(true));
    data.insert("exchange".into(), json!(name));
    data.insert("pair".into(), json!(format!("{base}/{quote}")));
    data.insert("side".into(), json!(kind.side()));
    data.insert("order_type".into(), json!(if kind.limit() { "limit" } else { "market" }));
    data.insert("amount".into(), arg(args, "amount").clone());
    data.insert("amount_type".into(), json!(amount_type));
    if kind.limit() { data.insert("price".into(), arg(args, "price").clone()); }
    data.insert("upstream".into(), json!({ "order_id": format!("dry-order-{}", &uuid::Uuid::new_v4().to_string()[10..]) }));
    Ok(Res::success(Value::Object(data)))
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
fn integer(v: &Value) -> Option<i64> {
    if let Some(f) = v.as_f64().filter(|_| v.is_f64()) { if !(f.is_finite() && f == f.floor()) { return None } }
    let n = number(v)?;
    let text = n.to_s_f();
    text.strip_suffix(".0").and_then(|whole| whole.parse().ok())
}

/// The bot BotOrder trades through: what its refusals read.
struct Through { bot: model::Bot, exchange: String, venue: String, ticker: Option<Ticker> }

/// BotOrder#call, up to its first venue call (`market_refusal`).
fn through_bot(c: &Connection, app: &App, who: Caller, kind: Kind, args: &Map<String, Value>, bot_id: &Value) -> Result<Res, WebError> {
    if !number(arg(args, "amount")).is_some_and(|n| n.is_positive()) { return Ok(invalid_number("amount")) }
    let Some(amount_type) = amount_type(kind, args) else { return Ok(invalid_amount_type()) };
    let Some(id) = integer(bot_id).filter(|id| *id > 0) else { return Ok(failure(Domain::ValidationFailed, "invalid_number", "bot_id must be a whole number.")) };
    // `@user.bots.not_deleted.find_by(id:)`
    let found: Option<(String, String)> = c.query_row("SELECT coalesce(e.name, ''), coalesce(e.type, '') FROM bots b LEFT JOIN exchanges e ON e.id=b.exchange_id \
                                                       WHERE b.id=?1 AND b.user_id=?2 AND b.status!=?3",
        rusqlite::params![id, who.user, crate::enums::BotStatus::Deleted as i64], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((exchange, venue)) = found else { return Ok(failure(Domain::NotFound, "bot_not_found", "Bot not found.")) };
    let bot = model::load_bot(c, id).map_err(|e| WebError::Config(format!("{e:?}")))?;
    let ticker = if bot.bot_type == model::SIGNAL_TYPE { model::ticker_for(c, &bot).map_err(|e| WebError::Config(format!("{e:?}")))? } else { None };
    let through = Through { bot, exchange, venue, ticker };
    if let Some(refusal) = bot_refusal(c, &through, args)? { return Ok(refusal) }
    if let Some(refusal) = venue_refusal(c, app, who, kind, &through)? { return Ok(refusal) }
    if !trades(&through.venue) { return Ok(not_ported(IBKR)) }
    if !who.dry_run { return Ok(not_ported(PLACING)) }
    let (base, quote) = pair(c, &through)?;
    Ok(Res::success(json!({
        "dry_run": true, "bot_id": id, "transaction_id": null, "order_id": null, "exchange": through.exchange, "pair": format!("{base}/{quote}"),
        "side": kind.side(), "order_type": "market", "amount": arg(args, "amount"), "amount_type": amount_type,
    })))
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
    if t.bot.transient.get(crate::engine::signal::MARKER_KEY).is_some_and(Value::is_object) { return Ok(Some(placement_unresolved(id, &t.exchange))) }
    let Some(ticker) = &t.ticker else { return Ok(Some(failure(Domain::NotFound, "pair_not_found", format!("Bot {id} has no tradable pair on its exchange.")))) };
    // BotOrder#pair_mismatch: a field that was sent must agree, case-insensitively.
    let (base, quote) = pair(c, t)?;
    let wrong: Vec<&str> = [("exchange_name", &t.exchange), ("base_asset", &base), ("quote_asset", &quote)].into_iter()
        .filter(|(key, actual)| { let sent = arg(args, key); present(sent) && to_s(sent).to_lowercase() != actual.to_lowercase() })
        .map(|(key, _)| key).collect();
    if !wrong.is_empty() {
        return Ok(Some(failure(Domain::ValidationFailed, "bot_pair_mismatch", format!("Bot {id} trades {base}/{quote} on {}; {} say otherwise. Leave them out to use the bot's pair.", t.exchange, wrong.join(", ")))));
    }
    Ok(untradable(&t.exchange, ticker))
}

fn placement_unresolved(id: i64, exchange: &str) -> Res {
    failure(Domain::Conflict, "placement_unresolved", format!("An earlier order from bot {id} may or may not have been placed; check {exchange} and resolve it on the bot page."))
}

/// BotOrder#venue_refusal: a buy's wash-sale lock, then the stored key (pending is asked first).
fn venue_refusal(c: &Connection, app: &App, who: Caller, kind: Kind, t: &Through) -> Result<Option<Res>, WebError> {
    if kind.buy() { if let Some(ticker) = &t.ticker { if let Some(refusal) = wash_refusal(c, app, who, ticker)? { return Ok(Some(refusal)) } } }
    let status: Option<i64> = c.query_row("SELECT status FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 LIMIT 1", [who.user, t.bot.exchange_id], |r| r.get(0)).optional()?;
    Ok(match status {
        Some(s) if s == crate::enums::ApiKeyStatus::PendingActivation as i64 => Some(failure(Domain::Conflict, "api_key_pending", format!("The API key for {} is still pending activation.", t.exchange))),
        Some(s) if s == crate::enums::ApiKeyStatus::Correct as i64 => None,
        _ => Some(api_key_missing(&t.exchange)),
    })
}

/// A cancel's outcome before the venue: Rails' answer, or the one cancel to send (`send_cancel`).
pub enum Cancel { Done(Res), Send(CancelJob) }
/// What a cancel past every check sends, and its success answer.
pub struct CancelJob { target: Target, answer: Value }
enum Target {
    /// Transaction#cancel: the row's order on its bot's venue, then the engine's re-read.
    Row { bot: i64, tx: i64, external: String },
    /// `exchange.cancel_order(order_id:)` on the user's key; `row`: the user's bot row with that order id (bot, tx), whose
    /// stored order (OrderRef::load) is cancelled and re-read (r3).
    Key { user: i64, exchange: i64, external: String, row: Option<(i64, i64)> },
}

fn cancel_failed(why: &str) -> Res { failure(Domain::UpstreamFailed, "cancel_failed", format!("Cancel failed: {why}")) }

/// BotApi::Orders::Cancel#call, up to its venue call.
pub fn cancel(c: &Connection, who: Caller, order_id: &Value, exchange_name: &Value) -> Result<Cancel, WebError> {
    if !present(order_id) { return Ok(Cancel::Done(failure(Domain::ValidationFailed, "missing_required_parameter", "Missing required parameter(s): order_id."))) }
    let text = to_s(order_id);
    // `\A\d+\z`, then `@user.transactions.submitted.open.find_by(id: to_i)`: a bot's row, whatever the bot's status.
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        // (id, bot, external id, "base/quote", side, exchange name, exchange type)
        type Row = (i64, i64, String, String, String, String, String);
        let row: Option<Row> = match text.parse::<i64>() {
            Ok(id) => c.query_row("SELECT t.id, t.bot_id, coalesce(t.external_id, ''), coalesce(t.base, '') || '/' || coalesce(t.quote, ''), \
                                   CASE t.side WHEN 0 THEN 'buy' WHEN 1 THEN 'sell' ELSE '' END, coalesce(e.name, ''), coalesce(e.type, '') FROM transactions t \
                                   JOIN bots b ON b.id=t.bot_id LEFT JOIN exchanges e ON e.id=t.exchange_id WHERE b.user_id=?1 AND t.id=?2 AND t.status=0 AND t.external_status=1",
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
    if !has_correct_key(c, who.user, exchange)? { return Ok(Cancel::Done(api_key_missing(&name))) }
    if !trades(&venue) { return Ok(Cancel::Done(not_ported(IBKR))) }
    let answer = json!({ "dry_run": who.dry_run, "external_id": order_id, "exchange": name, "cancelled": true });
    if who.dry_run { return Ok(Cancel::Done(Res::success(answer))) }
    // r3: a stored order beats a bare id. The user's bot rows first (the latest with this venue and order id); then the
    // retained standalone order log, which slice O4 adds (until then there is none to look in); then the bare id.
    let row: Option<(i64, i64)> = c.query_row("SELECT t.bot_id, t.id FROM transactions t JOIN bots b ON b.id=t.bot_id \
                                               WHERE b.user_id=?1 AND t.exchange_id=?2 AND t.external_id=?3 AND substr(t.external_id, 1, 9) != 'imported_' ORDER BY t.id DESC LIMIT 1",
        rusqlite::params![who.user, exchange, text], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    Ok(Cancel::Send(CancelJob { target: Target::Key { user: who.user, exchange, external: text, row }, answer }))
}

/// The one bounded cancel a `CancelJob` names, on a connection of its own (no lock held while the venue answers), and its
/// answer. A refusal is Rails' `cancel_failed` with the venue's errors; no answer at all (a transport failure, the timeout)
/// is the same failure with the transport's message (D-ORDER-CANCEL-TRANSPORT: Rails raises).
pub async fn send_cancel(app: &App, job: CancelJob) -> Result<Res, WebError> {
    use crate::web::bot::cancel::{self as page, Sent};
    page::on_own_connection(app, move |c, app, runtime| {
        let factory = page::live_factory(c, app);
        let sent = match job.target {
            Target::Row { bot, tx, external } => page::send_for_row(c, app, &factory, runtime, &model::load_bot(c, bot)?, tx, &external)?,
            Target::Key { user, exchange, external, row } => {
                let venue = crate::engine::run::venue_for_key(c, &factory, &app.cipher, user, exchange)?;
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

/// D-ORDER-KUCOIN-FOREIGN: an order id with no stored route is sent on KuCoin's classic route, the only one Rails has. On
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
    let shown = |key: &str| match &args[key] { Value::Number(n) => crate::ruby::float_to_s(n.as_f64().unwrap_or_default()), v => to_s(v) };
    if name == "cancel_order" {
        return (if d["id"].is_null() { format!("{prefix}Order {} cancellation submitted on {}.", s("external_id"), s("exchange")) }
            else { format!("{prefix}Order #{} ({} {} on {}) cancellation submitted.", d["id"], s("side").to_uppercase(), s("pair"), s("exchange")) }, false);
    }
    let Some(kind) = Kind::tool(name) else { return ("An unexpected error occurred.".into(), true) };
    // The upstream answer as Ruby's Hash#inspect writes it (only a dry run's `{order_id: "…"}` reaches here).
    let upstream = d["upstream"].as_object().map(|m| format!("{{{}}}", m.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", "))).unwrap_or_default();
    if kind.limit() {
        return (format!("{prefix}Limit {} order placed on {}: {} @ {}. {upstream}", kind.side(), s("exchange"), s("pair"), shown("price")), false);
    }
    let (base, quote) = s("pair").split_once('/').map(|(b, q)| (b.to_string(), q.to_string())).unwrap_or_default();
    let currency = if kind.buy() { if s("amount_type") == "quote" { quote } else { base } } else if s("amount_type") == "base" { base } else { quote };
    let (placed, detail) = if d["bot_id"].is_null() { (format!("placed on {}", s("exchange")), upstream) }
        else { (format!("placed by bot {} on {}", d["bot_id"], s("exchange")), "Nothing was placed or recorded.".to_string()) };
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
