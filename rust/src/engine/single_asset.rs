//! PLAN-A23: a legacy single-asset bot (Bots::DcaSingleAsset), and a basket a stale single-asset save clobbered, becomes a
//! one-asset basket at every start, inside the start's own transaction (set_aside::apply), with the writes of Rails'
//! Bot::SingleToComposition#convert! (D-SINGLE-AT-START). No Rails process or job outlives the claim, so Rails' two
//! quiescence skips (executing, a job in flight) have nothing to wait for (D-SINGLE-QUIET-AT-CLAIM); a working bot on a
//! ticker its venue no longer trades converts (D-SINGLE-UNTRADEABLE-CONVERTS), and so does one whose condition watches
//! another ticker (D-SINGLE-CONDITION-CONVERTS). What can't be converted stays as it is, and eligibility names it to its
//! owner (`Skip`, D-SINGLE-KEPT-NAMED).
use super::model::{self, Bot};
use super::EngineError;
use chrono::{DateTime, Utc};
use rusqlite::{params, types::Value as Sql, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

pub const SINGLE: &str = "Bots::DcaSingleAsset";
pub const MULTI: &str = "Bots::DcaMultiAsset";

/// Bot::SingleToComposition.clobbered: the pair's key is back AND the basket's allocations are gone.
const CLOBBERED: &str = "type = 'Bots::DcaMultiAsset' \
    AND CASE WHEN json_valid(settings) THEN json_extract(settings, '$.base_asset_id') END IS NOT NULL \
    AND CASE WHEN json_valid(settings) THEN json_type(settings, '$.allocations') END IS NOT 'object'";

/// Why a legacy row is not converted (Rails' preflight reasons that survive a takeover, plan §4).
#[derive(Debug, Clone, PartialEq)]
pub enum Skip {
    /// missing assets, missing exchange, missing asset rows; settings that aren't an object or ids that aren't ids.
    Settings,
    /// missing ticker: no ticker row for the pair on its exchange.
    Market,
    /// A history the basket would read differently: its owner's words for what (S-HISTORY).
    History(&'static str),
    /// unexpected composition rows: a membership of another asset.
    Memberships,
    /// Convertible now (its ticker came back while the engine ran): the next start converts it.
    NextStart,
}

impl Skip {
    /// The operator's words (check, logs).
    pub fn text(&self) -> &'static str {
        match self {
            Self::Settings => "its settings can't be read", Self::Market => "no ticker for its pair",
            Self::History(what) => what, Self::Memberships => "a membership of another asset",
            Self::NextStart => "converts at the next start",
        }
    }
}

/// What `convert!` writes with: the pair's base asset and its ticker row.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion { pub base: i64, pub ticker: i64, entered: Option<DateTime<Utc>> }

/// A row the conversion is for: the single-asset class, or a basket in the clobbered shape.
pub fn legacy(bot: &Bot) -> bool {
    bot.bot_type == SINGLE
        || (bot.bot_type == MULTI && bot.settings.get("base_asset_id").is_some_and(|v| !v.is_null())
            && !bot.settings.get("allocations").is_some_and(Value::is_object))
}

/// Why a legacy row is kept, as of now: `Skip::NextStart` when nothing holds it back any more.
pub fn skip(c: &Connection, bot: &Bot) -> Result<Skip, EngineError> {
    Ok(preflight(c, bot, bot.bot_type == MULTI)?.err().unwrap_or(Skip::NextStart))
}

/// Every single-asset row by id, then every clobbered basket (`repair_clobbered!`): the ids converted. A row this build
/// can't read (model::load_bot, a stored time) is left as it is, so the start's check names it unreadable.
pub fn convert_all(tx: &Connection, now: DateTime<Utc>) -> Result<Vec<i64>, EngineError> {
    let mut converted = vec![];
    for (from, filter) in [(SINGLE, "type = 'Bots::DcaSingleAsset'"), (MULTI, CLOBBERED)] {
        let mut s = tx.prepare(&format!("SELECT id FROM bots WHERE {filter} ORDER BY id"))?;
        let ids = s.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            let Ok(bot) = model::load_bot(tx, id) else { continue };
            if bot.validate_times().is_err() { continue; }
            if let Ok(plan) = preflight(tx, &bot, from == MULTI)? {
                if convert(tx, &bot, from, plan, now)? { converted.push(id); }
            }
        }
    }
    Ok(converted)
}

/// Bot::SingleToComposition.preflight, less what no longer holds a row back after the claim (executing, a job in flight, an
/// untradeable ticker, a condition on another ticker). Data never errors here: what Ruby can't read is `Skip::Settings`.
pub fn preflight(c: &Connection, bot: &Bot, repair: bool) -> Result<Result<Conversion, Skip>, EngineError> {
    let empty = Map::new();
    let settings = bot.settings.as_object().unwrap_or(&empty);
    let (Some(base), Some(quote)) = (to_i(settings.get("base_asset_id")), to_i(settings.get("quote_asset_id"))) else { return Ok(Err(Skip::Settings)) };
    if base == 0 || quote == 0 { return Ok(Err(Skip::Settings)); }
    let (exchange, created): (Option<i64>, Option<String>) = c.query_row("SELECT exchange_id, created_at FROM bots WHERE id = ?1", [bot.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    // A membership enters at the bot's creation (`entered_at ||= row.created_at`): one this build can't read is kept.
    let entered = match created.as_deref().map(crate::codec::parse_time) { None => None, Some(Ok(t)) => Some(t), Some(Err(_)) => return Ok(Err(Skip::Settings)) };
    let Some(exchange) = exchange else { return Ok(Err(Skip::Settings)) };
    let assets: i64 = c.query_row("SELECT count(*) FROM assets WHERE id IN (?1, ?2)", [base, quote], |r| r.get(0))?;
    if assets != 2 { return Ok(Err(Skip::Settings)); }
    // No availability filter: the membership hangs on the ticker row whatever its state.
    let ticker: Option<i64> = c.query_row("SELECT id FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2 AND quote_asset_id = ?3 ORDER BY id LIMIT 1",
                                          [exchange, base, quote], |r| r.get(0)).optional()?;
    let Some(ticker) = ticker else { return Ok(Err(Skip::Market)) };
    // Each query names the bot (?1) and the base (?2).
    let exists = |sql: &str| c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {sql})"), params![bot.id, base], |r| r.get::<_, bool>(0));
    if !repair && exists("bot_index_assets WHERE bot_id = ?1 AND asset_id != ?2")? { return Ok(Err(Skip::Memberships)); }
    // history_divergence: the histories the two walks read differently (Transaction.statuses submitted, sides sell, closed).
    if exists("transactions WHERE bot_id = ?1 AND status = 0 AND (base_asset_id IS NULL OR base_asset_id != ?2)")? { return Ok(Err(Skip::History("orders of another asset"))); }
    if exists("transactions WHERE bot_id = ?1 AND status = 0 AND transaction_type != 'REGULAR' AND ?2 = ?2")? { return Ok(Err(Skip::History("rebalancing, liquidation or redeploy orders"))); }
    if exists("transactions WHERE bot_id = ?1 AND status = 0 AND side = 1 AND (quote_amount_exec IS NULL OR quote_amount_exec <= 0) \
               AND COALESCE(amount_exec, CASE WHEN external_status = 2 THEN amount END) > 0 AND ?2 = ?2")? {
        return Ok(Err(Skip::History("a sale whose proceeds weren't reported")));
    }
    Ok(Ok(Conversion { base, ticker, entered }))
}

/// Ruby's `to_i` on a settings value; None where Ruby raises (true, an array, a hash).
fn to_i(v: Option<&Value>) -> Option<i64> {
    match v {
        None | Some(Value::Null) => Some(0),
        Some(Value::String(s)) => Some(crate::ruby::to_i(s)),
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.abs() < 9.2e18).map(|f| f.trunc() as i64)),
        _ => None,
    }
}

/// ActiveSupport's `blank?`.
fn blank(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null | Value::Bool(false)) => true,
        Some(Value::String(s)) => s.chars().all(char::is_whitespace),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        _ => false,
    }
}

/// Bot::SingleToComposition.convert!: the type, the settings, then the memberships, on the row still of type `from`.
/// settings_changed_at is untouched (update_columns). false: the row is no longer of that type.
fn convert(tx: &Connection, bot: &Bot, from: &str, plan: Conversion, now: DateTime<Utc>) -> Result<bool, EngineError> {
    // Hash#except keeps the order; Hash#[]= replaces a key in place and appends a new one.
    let mut settings: Map<String, Value> = bot.settings.as_object().cloned().unwrap_or_default().into_iter().filter(|(k, _)| k != "base_asset_id").collect();
    settings.insert("allocations".into(), json!({ plan.base.to_string(): 1.0 }));
    // The single-asset bot read a blank sell sentence as a base amount, the basket reads it as quote.
    if blank(settings.get("sell_denomination")) { settings.insert("sell_denomination".into(), json!("base")); }
    let at = crate::codec::format_time(now);
    let n = tx.execute("UPDATE bots SET type = ?1, settings = ?2, updated_at = ?3 WHERE id = ?4 AND type = ?5",
                       params![MULTI, crate::web::bot::mcp_input::encode(&Value::Object(settings)), at, bot.id, from])?;
    if n == 0 { return Ok(false); }
    tx.execute("UPDATE bot_index_assets SET in_index = 0, exited_at = ?1, updated_at = ?1 WHERE bot_id = ?2 AND in_index = 1 AND asset_id != ?3",
               params![at, bot.id, plan.base])?;
    // find_or_initialize_by, then save! only when a value changed as ActiveRecord compares typed values.
    let entered = plan.entered.map(crate::codec::format_time);
    let row = tx.query_row("SELECT id, ticker_id, target_allocation, in_index, entered_at, exited_at FROM bot_index_assets WHERE bot_id = ?1 AND asset_id = ?2 ORDER BY id LIMIT 1",
                           params![bot.id, plan.base], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Sql>(1)?, r.get::<_, Sql>(2)?, r.get::<_, Sql>(3)?, r.get::<_, Sql>(4)?, r.get::<_, Sql>(5)?)))
        .optional()?;
    let Some((id, ticker, target, in_index, entered_at, exited_at)) = row else {
        tx.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, exited_at, created_at, updated_at) \
                    VALUES (?1, ?2, ?3, '1.0', 1, ?4, NULL, ?5, ?5)", params![bot.id, plan.base, plan.ticker, entered, at])?;
        return Ok(true);
    };
    let mut set: Vec<(&str, Sql)> = vec![];
    let integer = |v: &Sql| match v { Sql::Integer(i) => Some(*i), Sql::Real(f) => Some(f.trunc() as i64), Sql::Text(s) => Some(crate::ruby::to_i(s)), _ => None };
    if integer(&ticker) != Some(plan.ticker) { set.push(("ticker_id", Sql::Integer(plan.ticker))); }
    let one = match &target { Sql::Integer(i) => *i == 1, Sql::Real(f) => *f == 1.0, Sql::Text(s) => s.trim().parse::<f64>() == Ok(1.0), _ => false };
    if !one { set.push(("target_allocation", Sql::Text("1.0".into()))); }
    // ActiveModel::Type::Boolean: nil and "" are nil, FALSE_VALUES are false, anything else true.
    let truthy = match &in_index { Sql::Null => false, Sql::Integer(i) => *i != 0, Sql::Real(f) => *f != 0.0,
                                   Sql::Text(s) => !["", "0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&s.as_str()), Sql::Blob(_) => true };
    if !truthy { set.push(("in_index", Sql::Integer(1))); }
    if entered_at == Sql::Null { if let Some(t) = entered { set.push(("entered_at", Sql::Text(t))); } }
    if exited_at != Sql::Null { set.push(("exited_at", Sql::Null)); }
    if set.is_empty() { return Ok(true); }
    set.push(("updated_at", Sql::Text(at)));
    let columns = set.iter().enumerate().map(|(i, (k, _))| format!("{k} = ?{}", i + 2)).collect::<Vec<_>>().join(", ");
    let mut values = vec![Sql::Integer(id)];
    values.extend(set.into_iter().map(|(_, v)| v));
    tx.execute(&format!("UPDATE bot_index_assets SET {columns} WHERE id = ?1"), rusqlite::params_from_iter(values))?;
    Ok(true)
}

/// Q4: the sentence a kept legacy bot of `user`'s answers its page and its Start with, or None (not theirs, deleted, not
/// legacy, or a row this build can't read).
pub fn kept_sentence(c: &Connection, user: i64, id: i64) -> Result<Option<String>, EngineError> {
    let owned: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id = ?1 AND user_id = ?2 AND status != 3)", [id, user], |r| r.get(0))?;
    if !owned { return Ok(None); }
    let Ok(bot) = model::load_bot(c, id) else { return Ok(None) };
    if !legacy(&bot) { return Ok(None); }
    Ok(Some(super::set_aside::legacy_sentence(c, id, &skip(c, &bot)?)?))
}
