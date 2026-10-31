//! BotApi::Bots::Create, CreateIndex and CreateSignal (CreateSupport#save_and_start): a bot made and started in one call,
//! from MCP's create tools and REST's `POST /api/v1/bots`. The checks run in Rails' order with its codes and words; the
//! bot is then built and inserted as the wizard saves it (wizard::create, wizard::index_create, signal_write) and started
//! as the bot page starts it (write::lifecycle_in, signal_write::start_created), all in ONE write transaction: Rails'
//! insert is the start's own save (`valid?(:start) && save`), so a refused create leaves nothing (no row, no log, no wake).
//! D-API-CREATE-REFUSED: a create the engine would not run is refused, nothing written. D-API-CREATE-HYPERLIQUID-LIMIT:
//! a Hyperliquid bot gets the wizard's limit orders (Rails' API create leaves them off and its start then refuses it).
use super::{action_params::ActionParams, draft::FieldError, mcp_input, signal_write, write::{self, Action, Outcome, Prepared, Step}, For};
use crate::engine::{eligibility, model};
use crate::ruby::BigDec;
use crate::web::bot_api::{Domain, Res};
use crate::web::layout::Ctx;
use crate::web::mcp::tools::str_value;
use crate::web::{bots, wizard, WebError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// Which service a call names (the create tool, or REST's `type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Dca, Index, Signal }

impl Kind {
    pub fn tool(name: &str) -> Option<Self> {
        Some(match name { "create_bot" => Self::Dca, "create_index_bot" => Self::Index, "create_signal_bot" => Self::Signal, _ => return None })
    }
    /// The parameters the service takes (BotsController's permit lists, the tools' properties).
    pub fn params(self) -> &'static [&'static str] {
        match self {
            Self::Dca => &["exchange_name", "base_asset", "second_base_asset", "quote_asset", "quote_amount", "interval", "allocation", "label", "start_at", "assets", "weighting"],
            Self::Index => &["exchange_name", "quote_asset", "quote_amount", "interval", "index", "num_coins", "allocation_flattening", "label", "start_at"],
            Self::Signal => &["exchange_name", "base_asset", "quote_asset", "label"],
        }
    }
}

const INTERVALS: [&str; 4] = ["hour", "day", "week", "month"];
const WEIGHTINGS: [&str; 2] = ["manual", "market_cap"];
/// Bots::DcaMultiAsset::MAX_ASSETS.
const MAX_ASSETS: usize = 100;
/// Index::TOP_COINS_EXTERNAL_ID.
const TOP_COINS: &str = "top-coins";
/// Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS.
const MINIMUM_SUPPORTED_COINS: i64 = 3;

fn fail(status: Domain, code: &'static str, message: String) -> Res { Res::failure(status, code, message) }
fn invalid(code: &'static str, message: String) -> Res { fail(Domain::ValidationFailed, code, message) }

/// A parameter as the service holds it (absent is nil).
fn get<'a>(p: &'a Map<String, Value>, key: &str) -> &'a Value { p.get(key).unwrap_or(&Value::Null) }
fn present(v: &Value) -> bool { crate::web::bot_api::present(v) }
/// `"#{value}"`.
fn text(v: &Value) -> String { str_value(v) }

/// BotApi::Number.within(value, low..high).
fn within(v: &Value, low: i64, high: i64) -> Option<BigDec> {
    mcp_input::number(v).filter(|n| n >= &BigDec::from_i64(low) && n <= &BigDec::from_i64(high))
}

/// BotApi::Number.integer: a whole number however it arrived (a Float only when it is integral).
fn integer(v: &Value) -> Option<i64> {
    if let Some(f) = v.as_f64().filter(|_| v.is_f64()) { if !(f.is_finite() && f == f.floor()) { return None; } }
    let n = mcp_input::number(v)?;
    (n.floor(0) == n).then(|| n.to_s_f().split('.').next().and_then(|w| w.parse().ok())).flatten()
}

/// CreateSupport#missing_required.
fn missing(p: &Map<String, Value>, keys: &[&str]) -> Option<Res> {
    let absent: Vec<&str> = keys.iter().copied().filter(|k| !present(get(p, k))).collect();
    (!absent.is_empty()).then(|| invalid("missing_required_parameter", format!("Missing required parameter(s): {}.", absent.join(", "))))
}

fn interval_ok(v: &Value) -> bool { v.as_str().is_some_and(|i| INTERVALS.contains(&i)) }

fn invalid_interval(v: &Value) -> Res {
    invalid("invalid_interval", format!("Invalid interval '{}'. Must be one of: {}", text(v), INTERVALS.join(", ")))
}

/// A tradeable venue: Exchange.tradeable (available, not retired).
struct Venue { id: i64, name: String, class: String }

/// CreateSupport#find_exchange: `LOWER(name) = name.downcase`, the first by id.
fn find_exchange(c: &Connection, name: &Value) -> Result<Option<Venue>, WebError> {
    Ok(c.query_row("SELECT id, name, type FROM exchanges WHERE available = 1 AND type NOT IN ('Exchanges::Bitmart') AND LOWER(name) = ?1 ORDER BY id LIMIT 1",
                   [text(name).to_lowercase()], |r| Ok(Venue { id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), class: r.get::<_, Option<String>>(2)?.unwrap_or_default() })).optional()?)
}

fn exchange_not_found(c: &Connection, name: &Value) -> Result<Res, WebError> {
    Ok(fail(Domain::NotFound, "exchange_not_found", format!("Exchange '{}' not found. Available: {}", text(name), crate::web::mcp::tradeable(c)?.join(", "))))
}

/// CreateSupport#trading_key: a correct trading key of the user's on the venue.
fn has_trading_key(c: &Connection, user: i64, venue: &Venue) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0 AND status = ?3)",
                   (user, venue.id, crate::enums::ApiKeyStatus::Correct as i64), |r| r.get(0))?)
}

fn api_key_missing(venue: &Venue) -> Res {
    fail(Domain::PermissionDenied, "api_key_missing", format!("No valid API key found for {}. Please add an API key in Settings.", venue.name))
}

/// CreateSupport#positive_number.
fn positive(v: &Value, name: &str) -> Result<BigDec, Res> {
    mcp_input::number(v).filter(BigDec::is_positive).ok_or_else(|| invalid("invalid_number", format!("{name} must be a number greater than 0.")))
}

/// CreateSupport#find_pair: exactly one ticker of these symbols on the venue, and that one available.
fn find_pair(c: &Connection, venue: &Venue, base: &str, quote: &str) -> Result<Option<(i64, i64)>, WebError> {
    let mut s = c.prepare("SELECT t.base_asset_id, t.quote_asset_id, t.available FROM tickers t JOIN assets b ON b.id = t.base_asset_id \
                           JOIN assets q ON q.id = t.quote_asset_id WHERE t.exchange_id = ?1 AND b.symbol = ?2 AND q.symbol = ?3 LIMIT 2")?;
    let rows: Vec<(i64, i64, bool)> = s.query_map((venue.id, base.to_uppercase(), quote.to_uppercase()), |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<bool>>(2)?.unwrap_or(false))))?
        .collect::<Result<_, _>>()?;
    Ok(match rows.as_slice() { [(b, q, true)] => Some((*b, *q)), _ => None })
}

fn pair_not_found(venue: &Venue, symbol: &str, quote: &Value) -> Res {
    fail(Domain::NotFound, "pair_not_found", format!("Trading pair {}/{} not found on {}.", symbol.to_uppercase(), text(quote).to_uppercase(), venue.name))
}

/// One basket entry: its symbol, and its weight when one was given (`None`: none given).
type Entry = (String, Option<BigDec>);

/// Ruby's `String#split(',')`: the empty strings at the end are dropped.
fn ruby_split(s: &str, on: char) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split(on).collect();
    while parts.last().is_some_and(|p| p.is_empty()) { parts.pop(); }
    parts
}

/// Create#normalize_asset: a hash's symbol (stripped, upcased) and, when the key is there, a weight in 0..100.
fn normalize(symbol: &Value, allocation: Option<&Value>) -> Option<Entry> {
    let symbol = mcp_input::strip(&text(symbol)).to_uppercase();
    if crate::ruby::blank(&symbol) { return None; }
    match allocation { None => Some((symbol, None)), Some(v) => within(v, 0, 100).map(|a| (symbol, Some(a))) }
}

/// Create#basket_entries: `None` is the 422 (unparseable, malformed, empty, duplicated or past MAX_ASSETS).
fn basket_entries(p: &Map<String, Value>) -> Option<Vec<Entry>> {
    let (assets, second) = (get(p, "assets"), get(p, "second_base_asset"));
    let entries: Vec<Option<Entry>> = if present(assets) {
        match assets {
            Value::String(s) => ruby_split(s, ',').into_iter().map(|e| {
                let mut parts = mcp_input::strip(e).splitn(2, ':');
                let symbol = json!(parts.next().unwrap_or(""));
                match parts.next() { None => normalize(&symbol, None), Some(a) => normalize(&symbol, Some(&json!(a))) }
            }).collect(),
            Value::Array(items) => items.iter().map(|e| e.as_object().and_then(|h| normalize(h.get("symbol").unwrap_or(&Value::Null), h.get("allocation")))).collect(),
            // Array(hash) is its [key, value] pairs, none of them a hash; Array(scalar) is [scalar].
            _ => vec![None],
        }
    } else if present(second) {
        // The legacy pair keeps its symbols as sent (Create#basket_entries builds these two hashes itself).
        let allocation = get(p, "allocation");
        let first = if present(allocation) { within(allocation, 0, 100)? } else { BigDec::from_i64(50) };
        let rest = BigDec::from_i64(100).checked_add(&first.checked_mul(&BigDec::from_i64(-1)).ok()?).ok()?;
        vec![Some((text(get(p, "base_asset")), Some(first))), Some((text(second), Some(rest)))]
    } else {
        vec![normalize(get(p, "base_asset"), None)]
    };
    let entries: Vec<Entry> = entries.into_iter().collect::<Option<_>>()?;
    if entries.is_empty() || entries.len() > MAX_ASSETS { return None; }
    let mut seen: Vec<&str> = vec![];
    for (symbol, _) in &entries { if seen.contains(&symbol.as_str()) { return None; } seen.push(symbol); }
    Some(entries)
}

/// Create#basket_weights: fractions summing to 1, equal when none was given (or market_cap derives them), else every given
/// percentage over 100, which must all be there and add up to 100 within 0.1. Floats at the model boundary.
fn basket_weights(entries: &[Entry], market_cap: bool) -> Option<Vec<f64>> {
    let given: Vec<&BigDec> = entries.iter().filter_map(|(_, a)| a.as_ref()).collect();
    if market_cap || given.is_empty() { return Some(vec![1.0 / entries.len() as f64; entries.len()]); }
    if given.len() != entries.len() { return None; }
    let mut sum = BigDec::zero();
    for g in &given { sum = sum.checked_add(g).ok()?; }
    let off = sum.checked_add(&BigDec::from_i64(-100)).ok()?;
    let tenth = BigDec::parse("0.1").ok()?;
    let below = off.checked_mul(&BigDec::from_i64(-1)).ok()?;
    if off > tenth || below > tenth { return None; }
    given.iter().map(|g| g.div(&BigDec::from_i64(100)).map(|w| w.to_f())).collect()
}

/// What a create came to inside its transaction: the answer, and whether there is a write to commit.
struct Made { res: Res, commit: Option<Prepared<()>> }

fn refused(res: Res) -> Made { Made { res, commit: None } }

/// The service: checks, build, insert and start in one write transaction, committed only when the bot started.
pub fn create(c: &Connection, ctx: &Ctx, user: i64, kind: Kind, p: &Map<String, Value>) -> Result<Res, WebError> {
    let tx = model::immediate(c)?;
    let made = match kind { Kind::Dca => dca(&tx, ctx, user, p)?, Kind::Index => index(&tx, ctx, user, p)?, Kind::Signal => signal(&tx, ctx, user, p)? };
    let Some(prepared) = made.commit else { return Ok(made.res) }; // dropped: rolled back
    tx.commit()?;
    ctx.app.wake_engine();
    for (stream, html) in prepared.broadcasts { ctx.app.hub.broadcast(&stream, &html); }
    if let Some(id) = prepared.after_scheduling { super::action_view::broadcast_after_scheduling(ctx.clone(), id); }
    Ok(made.res)
}

/// The full-messages refusal of a model that would not start (CreateSupport's `bot_invalid`).
fn bot_invalid(errors: &[FieldError]) -> Res {
    invalid("bot_invalid", format!("Failed to create bot: {}", mcp_input::full_messages(errors)))
}

/// Bot::Startable#schedule_start_at: a `start_at` that was sent (blank included) switches the date mode on, read in the
/// account's zone; one that does not parse is nil, which the start's validation then refuses.
fn schedule(c: &Connection, user: i64, settings: &mut Map<String, Value>, start_at: Option<&Value>, now: chrono::DateTime<chrono::Utc>) -> Result<(), WebError> {
    let Some(value) = start_at.filter(|v| !v.is_null()) else { return Ok(()) };
    let zone: String = c.query_row("SELECT time_zone FROM users WHERE id = ?1", [user], |r| r.get(0))?;
    settings.insert("start_time_enabled".into(), json!(true));
    settings.insert("start_time_mode".into(), json!("date"));
    let at = if present(value) { crate::codec::parse_form_time(&json!(text(value)), &zone, now).unwrap_or(Value::Null) } else { Value::Null };
    settings.insert("start_at".into(), at);
    Ok(())
}

/// The start of a just-inserted DCA or index bot, as the bot page's Start runs it, on `tx`. `Ok(Err(res))`: refused.
fn start(tx: &Connection, ctx: &Ctx, user: i64, id: i64, create_errors: Vec<FieldError>) -> Result<Result<Prepared<()>, Res>, WebError> {
    let (mut seen, mut refusal): (Vec<FieldError>, Option<String>) = (vec![], None);
    let step = write::lifecycle_in(tx, ctx, user, id, Action::Start, &ActionParams::from_service(Map::new()), |_, _, view| {
        seen = view.draft.as_ref().map(|d| d.added.clone()).unwrap_or_else(|| view.errors.clone());
        refusal = view.errors.last().map(|e| e.message.clone());
        Ok(Prepared { response: (), broadcasts: vec![], after_scheduling: None })
    })?;
    // valid?(:start) on the new record: the start's errors as they were added, then the new record's own (create context).
    let mut errors = seen;
    for e in create_errors { if !errors.iter().any(|x| x.field == e.field && x.message == e.message) { errors.push(e); } }
    Ok(match step {
        Step::Commit(prepared) if errors.is_empty() => Ok(prepared),
        Step::Commit(_) => Err(bot_invalid(&errors)),
        Step::Done(Outcome::Invalid(_, _)) => Err(bot_invalid(&errors)),
        Step::Done(Outcome::GuardRefused(_)) => Err(fail(Domain::Conflict, "write_refused", refusal.unwrap_or_default())),
        Step::Done(Outcome::Unported(reason)) => Err(fail(Domain::NotPorted, "not_ported", crate::web::bot_api::refused(reason))),
        Step::Done(Outcome::Missing | Outcome::NoChange(_) | Outcome::Committed(_)) => return Err(WebError::Config("a create's start found no new bot".into())),
    })
}

/// Bot::Accountable on a NEW record (CreateSupport#save_and_start calls set_missed_quote_amount before the start): the
/// carry is captured on the insert, so the bot owes 0 (0.0 when the start is delayed: Lifecycle#start captures it again,
/// as a Float). The wizard's bot is saved first and started later, where the start finds no change to capture and
/// stores nil; the rows are Rails' own (pages_api.rb, api_create_*).
fn new_record_carry(tx: &Connection, id: i64, now: chrono::DateTime<chrono::Utc>) -> Result<bool, WebError> {
    let started: Option<String> = tx.query_row("SELECT started_at FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
    let delayed = match started { Some(at) => crate::codec::parse_time(&at).map_err(|e| WebError::Config(format!("a new bot's started_at: {e:?}")))? > now, None => false };
    tx.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.missed_quote_amount', json(?2)) WHERE id = ?1", (id, if delayed { "0.0" } else { "0" }))?;
    Ok(delayed)
}

/// Bot::BroadcastAfterScheduledActionJob, which a delayed first start enqueues: the new bot's status and progress bars, as
/// its owner sees them. A delayed start is scheduled by this write, so they go out at once (as the web's Start sends them).
fn scheduled_bars(tx: &Connection, ctx: &Ctx, user: i64, id: i64) -> Result<Vec<(String, String)>, WebError> {
    let owner = ctx.user().cloned().ok_or_else(|| WebError::Config("a create has no owner".into()))?;
    let heard = super::live::owner_ctx(&ctx.app, owner);
    let Some(bot) = super::Bot::find(tx, user, id, For::Page, heard.locale)? else { return Ok(vec![]) };
    let (_, configured) = bots::market_data(tx, &ctx.app)?;
    let state = super::status::broadcast(tx, &heard, &bot, configured)?;
    Ok(super::action_view::bar_broadcasts(&format!("user_{user}:bot_updates"), &bot, &state).to_vec())
}

/// A started bot this build's pages could not serve (the check the web's Start makes): refused, nothing kept.
fn unservable(tx: &Connection, ctx: &Ctx, id: i64) -> Result<Option<Res>, WebError> {
    let (provider, _) = bots::market_data(tx, &ctx.app)?;
    Ok(super::refusal(tx, id, provider, For::Settings)?.map(|reason| fail(Domain::NotPorted, "not_ported", crate::web::bot_api::refused(reason))))
}

fn started_at(tx: &Connection, id: i64) -> Result<Value, WebError> {
    let at: Option<String> = tx.query_row("SELECT started_at FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
    let Some(at) = at else { return Ok(Value::Null) };
    let at = crate::codec::parse_time(&at).map_err(|e| WebError::Config(format!("a new bot's started_at: {e:?}")))?;
    Ok(json!(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)))
}

fn row(tx: &Connection, id: i64) -> Result<(String, String, String, Map<String, Value>), WebError> {
    let (label, kind, status, settings): (Option<String>, String, i64, String) =
        tx.query_row("SELECT label, type, status, settings FROM bots WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    let status = crate::web::mcp::tools::STATUSES.get(status as usize).copied().unwrap_or("").to_string();
    Ok((label.unwrap_or_default(), kind, status, serde_json::from_str(&settings).unwrap_or_default()))
}

/// BotApi::Bots::Create.
fn dca(tx: &Connection, ctx: &Ctx, user: i64, p: &Map<String, Value>) -> Result<Made, WebError> {
    let basket = present(get(p, "assets")) || present(get(p, "second_base_asset"));
    let required: &[&str] = if basket { &["exchange_name", "quote_asset", "quote_amount", "interval"] } else { &["exchange_name", "quote_asset", "quote_amount", "interval", "base_asset"] };
    if let Some(res) = missing(p, required) { return Ok(refused(res)); }
    if !interval_ok(get(p, "interval")) { return Ok(refused(invalid_interval(get(p, "interval")))); }
    let weighting = if present(get(p, "weighting")) { get(p, "weighting").clone() } else { json!("manual") };
    if !weighting.as_str().is_some_and(|w| WEIGHTINGS.contains(&w)) {
        return Ok(refused(invalid("invalid_weighting", format!("weighting must be one of: {}", WEIGHTINGS.join(", ")))));
    }
    let market_cap = weighting == "market_cap";
    let allocation = get(p, "allocation");
    if present(get(p, "second_base_asset")) && present(allocation) && within(allocation, 0, 100).is_none() {
        return Ok(refused(invalid("invalid_allocation", format!("Invalid allocation '{}'. Must be a percentage between 0 and 100.", text(allocation)))));
    }
    let Some(venue) = find_exchange(tx, get(p, "exchange_name"))? else { return Ok(refused(exchange_not_found(tx, get(p, "exchange_name"))?)) };
    if !has_trading_key(tx, user, &venue)? { return Ok(refused(api_key_missing(&venue))); }
    let amount = match positive(get(p, "quote_amount"), "quote_amount") { Ok(a) => a, Err(res) => return Ok(refused(res)) };
    let Some(entries) = basket_entries(p) else {
        return Ok(refused(invalid("invalid_basket", format!("assets must list 1 to {MAX_ASSETS} symbols, e.g. 'BTC:60,ETH:40' or 'BTC,ETH'."))));
    };
    let quote = get(p, "quote_asset");
    let mut pairs = vec![];
    for (symbol, _) in &entries {
        match find_pair(tx, &venue, symbol, &text(quote))? { Some(pair) => pairs.push(pair), None => return Ok(refused(pair_not_found(&venue, symbol, quote))) }
    }
    let Some(weights) = basket_weights(&entries, market_cap) else {
        return Ok(refused(invalid("allocations_unbalanced", "Either give every asset a weight summing to 100, or none.".into())));
    };
    let members: Vec<i64> = pairs.iter().map(|(base, _)| *base).collect();
    let allocations: Map<String, Value> = members.iter().zip(&weights).map(|(id, w)| (id.to_string(), json!(w))).collect();
    if market_cap {
        let mut unique = members.clone();
        unique.dedup();
        let capped: i64 = tx.query_row("SELECT COUNT(*) FROM assets WHERE id IN (SELECT value FROM json_each(?1)) AND market_cap >= 1", [json!(unique).to_string()], |r| r.get(0))?;
        if capped != unique.len() as i64 {
            let symbols: Vec<&str> = entries.iter().map(|(s, _)| s.as_str()).collect();
            return Ok(refused(invalid("market_cap_unavailable", format!("Market-cap weighting needs a market cap for every asset; not every one of {} has one. Give weights instead.", symbols.join(", ")))));
        }
    }
    let settings = json!({"quote_asset_id": pairs[0].1, "quote_amount": amount.to_f(), "interval": get(p, "interval"), "weighting": weighting,
                          "allocations": allocations, "base_asset_ids": members});
    let mut config = json!({"exchange_id": venue.id, "settings": settings});
    // D-API-CREATE-HYPERLIQUID-LIMIT: the wizard's lock, which Rails' API create skips.
    wizard::create::lock_limit_orders(tx, &mut config)?;
    let mut built = wizard::create::build(tx, &config, "en")?;
    if present(get(p, "label")) { built.label = text(get(p, "label")); }
    schedule(tx, user, &mut built.settings, p.get("start_at"), ctx.now)?;
    let create_errors: Vec<FieldError> = wizard::create::validate(tx, &built, "en")?.into_iter().map(|message| FieldError { field: "allocations".into(), message }).collect();
    let id = wizard::create::insert(tx, user, built, ctx.now)?;
    let mut prepared = match start(tx, ctx, user, id, create_errors)? { Ok(p) => p, Err(res) => return Ok(refused(res)) };
    if let Some(res) = unservable(tx, ctx, id)? { return Ok(refused(res)); }
    if new_record_carry(tx, id, ctx.now)? { prepared.broadcasts.extend(scheduled_bars(tx, ctx, user, id)?); }
    let (label, kind, status, stored) = row(tx, id)?;
    let symbols: Vec<&str> = entries.iter().map(|(s, _)| s.as_str()).collect();
    let data = json!({"id": id, "label": label, "type": kind, "status": status, "exchange": venue.name,
                      "pair": format!("{}/{}", symbols.join("+"), text(quote).to_uppercase()), "quote_asset": text(quote).to_uppercase(),
                      "quote_amount": stored.get("quote_amount"), "interval": stored.get("interval"), "started_at": started_at(tx, id)?});
    Ok(Made { res: Res { status: Domain::Created, ..Res::success(data) }, commit: Some(prepared) })
}

/// Index.for_picker.available_on_exchange(exchange).find_by(external_id:): the picker's order, data-api's indices only
/// when it is the provider.
fn find_index(tx: &Connection, ctx: &Ctx, venue: &Venue, external_id: &str) -> Result<Option<(String, Option<String>)>, WebError> {
    let (deltabadger, _) = bots::market_data(tx, &ctx.app)?;
    Ok(tx.query_row("SELECT external_id, name FROM indices WHERE external_id = ?1 AND json_extract(available_exchanges, ?2) IS NOT NULL \
                     AND (?3 OR source IS NOT 'deltabadger') \
                     ORDER BY CASE source WHEN 'deltabadger' THEN 0 WHEN 'internal' THEN 1 ELSE 2 END, weight DESC, market_cap DESC LIMIT 1",
                    (external_id, format!("$.\"{}\"", venue.class), deltabadger), |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
}

/// BotApi::Bots::CreateIndex.
fn index(tx: &Connection, ctx: &Ctx, user: i64, p: &Map<String, Value>) -> Result<Made, WebError> {
    if let Some(res) = missing(p, &["exchange_name", "quote_asset", "quote_amount", "interval"]) { return Ok(refused(res)); }
    if !interval_ok(get(p, "interval")) { return Ok(refused(invalid_interval(get(p, "interval")))); }
    if !bots::market_data(tx, &ctx.app)?.1 {
        return Ok(refused(invalid("market_data_not_configured", "Market data provider is not configured. Set up CoinGecko or Deltabadger market data in Settings first.".into())));
    }
    let Some(venue) = find_exchange(tx, get(p, "exchange_name"))? else { return Ok(refused(exchange_not_found(tx, get(p, "exchange_name"))?)) };
    if !has_trading_key(tx, user, &venue)? { return Ok(refused(api_key_missing(&venue))); }
    let asked = if present(get(p, "index")) { text(get(p, "index")) } else { TOP_COINS.to_string() };
    let Some((external_id, name)) = find_index(tx, ctx, &venue, &asked)? else {
        return Ok(refused(fail(Domain::NotFound, "index_not_found", format!("Index '{asked}' not found or not available on {}. Use 'list_indices' with exchange_name to see what is.", venue.name))));
    };
    let amount = match positive(get(p, "quote_amount"), "quote_amount") { Ok(a) => a, Err(res) => return Ok(refused(res)) };
    let flattening = if present(get(p, "allocation_flattening")) {
        match mcp_input::number(get(p, "allocation_flattening")).filter(|n| n <= &BigDec::one()) { Some(f) => f, None => return Ok(refused(invalid("invalid_number", "allocation_flattening must be a number between 0 and 1.".into()))) }
    } else { BigDec::zero() };
    let count = if present(get(p, "num_coins")) {
        match integer(get(p, "num_coins")) { Some(n) => Some(n), None => return Ok(refused(invalid("invalid_number", "num_coins must be a whole number.".into()))) }
    } else { None };
    // Index#bot_settings.
    let top = external_id == TOP_COINS;
    let mut settings = json!({"interval": get(p, "interval"), "allocation_flattening": flattening.to_f()});
    let index_settings = if top { json!({"index_type": "top", "index_category_id": null, "index_name": null, "index_name_prefix": null}) }
        else { json!({"index_type": "category", "index_category_id": external_id, "index_name": name, "index_name_prefix": wizard::index_create::count_named(Some(&external_id)).map(|(prefix, _)| prefix)}) };
    for (k, v) in index_settings.as_object().into_iter().flatten() { settings[k] = v.clone(); }
    let Some(quote) = wizard::index::quote_asset(tx, venue.id, &venue.class, settings.as_object().unwrap_or(&Map::new()), &text(get(p, "quote_asset")).to_uppercase())? else {
        return Ok(refused(fail(Domain::NotFound, "quote_asset_not_found", format!("Fewer than {MINIMUM_SUPPORTED_COINS} members of '{asked}' trade against {} on {}. Try another quote asset or exchange.", text(get(p, "quote_asset")).to_uppercase(), venue.name))));
    };
    // `bots.new(settings:)` runs after_initialize before the quote and the amount are assigned: no Smart Intervals amount
    // is derived (it needs both), and the assigned keys follow the defaults. The save's callbacks (the count's clamp, the
    // label) then run on the assigned settings: the second build, whose defaults are all already set but that one.
    let mut config = json!({"exchange_id": venue.id, "settings": settings});
    wizard::create::lock_limit_orders(tx, &mut config)?;
    let initialized = wizard::index_create::build(tx, &config, "en")?;
    let mut settings = initialized.settings;
    let derived = settings.get("smart_interval_quote_amount").is_some_and(|v| !v.is_null());
    if !derived { settings.shift_remove("smart_interval_quote_amount"); }
    settings.insert("quote_asset_id".into(), json!(quote));
    settings.insert("quote_amount".into(), json!(amount.to_f()));
    if let Some(n) = count {
        settings.insert("num_coins".into(), json!(n));
        let max = initialized.bounded.unwrap_or(wizard::index_create::MAX_COINS);
        settings.insert("hold_all".into(), json!(n >= max));
    }
    let mut built = wizard::index_create::build(tx, &json!({"exchange_id": venue.id, "settings": settings}), "en")?;
    if !derived { built.settings.shift_remove("smart_interval_quote_amount"); }
    if present(get(p, "label")) { built.label = text(get(p, "label")); }
    schedule(tx, user, &mut built.settings, p.get("start_at"), ctx.now)?;
    let create_errors: Vec<FieldError> = wizard::index_create::validate(&built, "en").into_iter().map(|message| FieldError { field: "num_coins".into(), message }).collect();
    let id = wizard::index_create::insert(tx, user, built, ctx.now)?;
    let mut prepared = match start(tx, ctx, user, id, create_errors)? { Ok(p) => p, Err(res) => return Ok(refused(res)) };
    if let Some(res) = unservable(tx, ctx, id)? { return Ok(refused(res)); }
    if new_record_carry(tx, id, ctx.now)? { prepared.broadcasts.extend(scheduled_bars(tx, ctx, user, id)?); }
    // The start's save writes the Smart Intervals amount the bot reads (the default a saved bot's settings page shows); a new
    // record's after_initialize found no quote to derive it from, so Rails stores none.
    if !derived { tx.execute("UPDATE bots SET settings = json_remove(settings, '$.smart_interval_quote_amount') WHERE id = ?1", [id])?; }
    let (label, kind, status, stored) = row(tx, id)?;
    let data = json!({"id": id, "label": label, "type": kind, "status": status, "exchange": venue.name, "index": asked,
                      "num_coins": stored.get("num_coins"), "allocation_flattening": stored.get("allocation_flattening"),
                      "quote_asset": text(get(p, "quote_asset")).to_uppercase(), "quote_amount": stored.get("quote_amount"),
                      "interval": stored.get("interval"), "started_at": started_at(tx, id)?});
    Ok(Made { res: Res { status: Domain::Created, ..Res::success(data) }, commit: Some(prepared) })
}

/// BotApi::Bots::CreateSignal: one pair, no schedule, no webhook rule.
fn signal(tx: &Connection, ctx: &Ctx, user: i64, p: &Map<String, Value>) -> Result<Made, WebError> {
    if let Some(res) = missing(p, &["exchange_name", "base_asset", "quote_asset"]) { return Ok(refused(res)); }
    let Some(venue) = find_exchange(tx, get(p, "exchange_name"))? else { return Ok(refused(exchange_not_found(tx, get(p, "exchange_name"))?)) };
    if !has_trading_key(tx, user, &venue)? { return Ok(refused(api_key_missing(&venue))); }
    let Some((base, quote)) = find_pair(tx, &venue, &text(get(p, "base_asset")), &text(get(p, "quote_asset")))? else {
        return Ok(refused(pair_not_found(&venue, &text(get(p, "base_asset")), get(p, "quote_asset"))));
    };
    let label = present(get(p, "label")).then(|| text(get(p, "label")));
    let id = signal_write::insert_created(tx, user, venue.id, &json!({"base_asset_id": base, "quote_asset_id": quote}), label.as_deref(), &[], "en", ctx.now)?;
    let broadcasts = match signal_write::start_created(tx, ctx, user, id)? {
        signal_write::Started::Yes(broadcasts) => broadcasts,
        signal_write::Started::Invalid(errors) => return Ok(refused(bot_invalid(&errors))),
        signal_write::Started::Refused(message) => return Ok(refused(fail(Domain::Conflict, "write_refused", message))),
    };
    if let Some(reason) = signal_write::refused_after(tx, &ctx.app, id)? { return Ok(refused(fail(Domain::NotPorted, "not_ported", crate::web::bot_api::refused(reason)))); }
    if let Err(refusal) = eligibility::guard(tx, &ctx.app.cipher, Some(id)) {
        return Ok(refused(fail(Domain::Conflict, "write_refused", crate::web::bot_api::refused(&refusal.reason()))));
    }
    let (label, kind, status, _) = row(tx, id)?;
    let symbols: (String, String) = tx.query_row("SELECT (SELECT symbol FROM assets WHERE id = ?1), (SELECT symbol FROM assets WHERE id = ?2)", (base, quote), |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
    let data = json!({"id": id, "label": label, "type": kind, "status": status, "exchange": venue.name,
                      "pair": format!("{}/{}", symbols.0, symbols.1), "started_at": started_at(tx, id)?});
    Ok(Made { res: Res { status: Domain::Created, ..Res::success(data) }, commit: Some(Prepared { response: (), broadcasts, after_scheduling: None }) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baskets_parse_as_rails_does() {
        let p = |v: Value| v.as_object().unwrap().clone();
        let symbols = |v: Value| basket_entries(&p(v)).map(|e| e.into_iter().map(|(s, a)| (s, a.map(|a| a.to_s_f()))).collect::<Vec<_>>());
        assert_eq!(symbols(json!({"assets": "AAPL:60,msft:40"})), Some(vec![("AAPL".into(), Some("60".into())), ("MSFT".into(), Some("40".into()))]));
        assert_eq!(symbols(json!({"assets": "AAPL,MSFT,"})).map(|e| e.len()), Some(2), "a trailing comma is dropped, as String#split drops it");
        for refused in [json!({"assets": ","}), json!({"assets": "AAPL:"}), json!({"assets": "AAPL,aapl"}), json!({"assets": "AAPL,,MSFT"}), json!({"assets": [1, 2]})] {
            assert_eq!(symbols(refused.clone()), None, "{refused}");
        }
        assert_eq!(symbols(json!({"base_asset": "AAPL", "second_base_asset": "MSFT", "allocation": 70})), Some(vec![("AAPL".into(), Some("70".into())), ("MSFT".into(), Some("30".into()))]));
        let many = (1..=101).map(|i| format!("S{i}")).collect::<Vec<_>>().join(",");
        assert_eq!(symbols(json!({"assets": many})), None);
    }

    #[test]
    fn weights_are_floats_summing_to_one() {
        let e = |a: &[Option<&str>]| a.iter().enumerate().map(|(i, w)| (format!("S{i}"), w.map(|w| BigDec::parse(w).unwrap()))).collect::<Vec<_>>();
        assert_eq!(basket_weights(&e(&[None, None, None]), false), Some(vec![1.0 / 3.0; 3]));
        assert_eq!(basket_weights(&e(&[Some("60"), Some("39.95")]), false), Some(vec![0.6, 0.3995]));
        assert_eq!(basket_weights(&e(&[Some("60"), Some("30")]), false), None);
        assert_eq!(basket_weights(&e(&[Some("60"), None]), false), None);
        assert_eq!(basket_weights(&e(&[Some("90"), Some("10")]), true), Some(vec![0.5, 0.5]));
    }

    #[test]
    fn integers_as_bot_api_number_reads_them() {
        assert_eq!(integer(&json!(3.0)), Some(3));
        assert_eq!(integer(&json!("3")), Some(3));
        assert_eq!(integer(&json!(2.5)), None);
        assert_eq!(integer(&json!(1.0000000000000002)), None);
    }
}
