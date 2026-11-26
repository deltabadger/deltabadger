//! PLAN index-as-ingredient §3.2 (slice 4b): a stored index bot (`Bots::DcaIndex`) becomes a basket that follows its index
//! (`Bots::DcaMultiAsset` with one `index_groups` element at weight 1.0) at every start, inside the start's own transaction
//! (set_aside::apply, after single_asset::convert_all). The settings and transient data are edited in place by SQLite's JSON
//! functions, so every untouched value keeps its stored text; `settings_changed_at`, `run_generation` and `bot_index_assets`
//! are untouched. The group's `rust_bot_group_members` rows are seeded from the `in_index` members, weights as stored.
//! A row that can't be carried over exactly stays `Bots::DcaIndex` (the legacy reader runs it as today) and is converted at a
//! later start (`Kept`, named by `check`).
//! Named divergences: D-INDEX-ONE-CLASS (a working stored seller, set aside today, converts and buys, as 2.70.21 ran it),
//! D-INDEX-RETIRED-PAIR (a converted retired composed index renormalises its surviving pairs), D-INDEX-DISTINCT-SEATS (a
//! ranking naming an asset twice seats distinct assets).
use super::model::{self, Bot};
use super::{basket, composition, index, rebalance, single_asset, EngineError};
use crate::enums::{BotStatus, BOT_WORKING};
use chrono::{DateTime, Utc};
use rusqlite::{params, types::{Value as Sql, ValueRef}, Connection};
use serde_json::{Map, Value};

/// A stored index bot's index keys: they move, raw, into its one `index_groups` element.
const KEYS: [&str; 7] = ["index_type", "index_category_id", "index_name", "index_name_prefix", "num_coins", "hold_all", "allocation_flattening"];

/// Why a stored index bot is not converted at this start. It runs as today and converts at a start after the cause goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// An unresolved placement intent (`rust_placement`): its snapshot holds the class and settings it was sent under.
    Intent,
    /// `rebalance_pending`.
    Rebalance,
    /// The row doesn't load, a stored time doesn't parse, settings or transient data aren't a JSON object or name a key
    /// twice (serde reads the last, SQLite's JSON functions the first), or a member row's ids don't decode.
    Unreadable,
    /// A `direction` other than absent, `buying` or `selling`: refused today, so it stays refused.
    Direction,
    /// Assets stored in `allocations` beside the index: the legacy reader reads their tickers (the quote cap's precision).
    Allocations,
    /// The converted row would be past `draft::loadable`'s bounds (a row editable today stays editable).
    Bounds,
    /// An `in_index` member's target can't be carried as a group weight exactly.
    Targets,
}

impl Kept {
    /// `check`'s words.
    pub fn text(self) -> &'static str {
        match self {
            Self::Intent => "its order isn't settled", Self::Rebalance => "a rebalance is in progress",
            Self::Unreadable => "its data can't be read", Self::Bounds => "its settings would be too large",
            Self::Direction => "its direction can't be read", Self::Allocations => "it has assets stored beside its index",
            Self::Targets => "its targets can't be carried over exactly",
        }
    }
}

/// A converted bot. `selling`: it was working and stored as selling, which an index bot never did; it now buys.
/// `rebalances`: stored as selling with rebalancing on, working or stopped; it now rebalances, as 2.x ran it (2.x ignores an
/// index bot's direction; this engine's legacy reader held its rebalance). Both D-INDEX-ONE-CLASS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted { pub id: i64, pub selling: bool, pub rebalances: bool }

/// What a pass converted, and what it kept with why.
pub type Pass = (Vec<Converted>, Vec<(i64, Kept)>);
/// A group row: (asset, ticker, weight).
type Seat = (i64, i64, f64);

/// Every stored index bot not deleted, by id: the ones converted and the ones kept with why.
pub fn convert_all(tx: &Connection, now: DateTime<Utc>) -> Result<Pass, EngineError> {
    let mut s = tx.prepare("SELECT id FROM bots WHERE type = 'Bots::DcaIndex' AND status <> ?1 ORDER BY id")?;
    let ids = s.query_map([BotStatus::Deleted as i64], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    let (mut converted, mut kept) = (vec![], vec![]);
    for id in ids {
        match convert(tx, id, now)? { Ok(c) => converted.push(c), Err(why) => kept.push((id, why)) }
    }
    Ok((converted, kept))
}

fn set(v: Option<&Value>) -> bool { v.is_some_and(|v| !v.is_null()) }

fn convert(tx: &Connection, id: i64, now: DateTime<Utc>) -> Result<Result<Converted, Kept>, EngineError> {
    let Some(bot) = single_asset::readable(model::load_bot(tx, id))? else { return Ok(Err(Kept::Unreadable)) };
    if bot.validate_times().is_err() { return Ok(Err(Kept::Unreadable)); }
    let (Some(settings), Some(transient)) = (bot.settings.as_object(), bot.transient.as_object()) else { return Ok(Err(Kept::Unreadable)) };
    if set(transient.get("rust_placement")) { return Ok(Err(Kept::Intent)); }
    if rebalance::is_pending(&bot.transient) { return Ok(Err(Kept::Rebalance)); }
    if !matches!(settings.get("direction"), None | Some(Value::Null)) && !["buying", "selling"].contains(&settings.get("direction").and_then(Value::as_str).unwrap_or("")) {
        return Ok(Err(Kept::Direction));
    }
    if settings.get("allocations").is_some_and(|a| !a.is_null() && a.as_object().is_none_or(|a| !a.is_empty())) { return Ok(Err(Kept::Allocations)); }
    // A key named twice: serde (the legacy reader) keeps the last, SQLite's `->`, json_set and json_remove the first.
    let (keys, transient_keys): (usize, usize) = tx.query_row("SELECT (SELECT count(*) FROM json_each(settings)), (SELECT count(*) FROM json_each(transient_data)) FROM bots WHERE id = ?1",
                                                              [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if keys != settings.len() || transient_keys != transient.len() { return Ok(Err(Kept::Unreadable)); }
    let members = match members(tx, &bot)? { Ok(m) => m, Err(why) => return Ok(Err(why)) };

    // The sell side as composition::reset_sell_side leaves it, as SQLite edits: a key it changed is set to its new value
    // (false, null or "quote", nothing numeric), a key it removed is removed.
    let external = composition::external_id(Some(settings));
    let (mut sold, mut latched) = (settings.clone(), transient.clone());
    composition::reset_sell_side(&mut sold, &mut latched);
    let mut sets: Vec<(String, Value)> = sold.iter().filter(|(k, v)| settings.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    sets.extend([("allocations".to_string(), Value::Object(Map::new())), ("weighting".into(), Value::from("manual"))]);
    let mut removed: Vec<String> = settings.keys().filter(|k| !sold.contains_key(*k)).cloned().collect();
    let moved: Vec<&str> = KEYS.into_iter().filter(|k| settings.contains_key(*k)).collect();
    removed.extend(moved.iter().map(|k| k.to_string()));
    removed.extend(["base_asset_ids", "direction", "sell_interval"].map(String::from));
    let mut unlatched: Vec<String> = transient.keys().filter(|k| !latched.contains_key(*k)).cloned().collect();
    unlatched.extend([index::COMPOSED_KEY, composition::COMPOSED_OF_KEY].map(String::from));

    // json_remove(json_set(settings, '$.index_groups', json_array(json_object(<moved>, 'weight', 1.0)), <sets>), <removed>).
    let mut args: Vec<Sql> = vec![];
    let mut arg = |v: Sql| { args.push(v); format!("?{}", args.len()) };
    let mut group: Vec<String> = moved.iter().map(|k| format!("'{k}', settings -> {}", arg(Sql::Text(format!("$.{k}"))))).collect();
    group.push("'weight', 1.0".into());
    let mut expr = format!("json_set(settings, '$.index_groups', json_array(json_object({}))", group.join(", "));
    for (k, v) in &sets { expr += &format!(", {}, json({})", arg(Sql::Text(format!("$.{k}"))), arg(Sql::Text(v.to_string()))); }
    expr += ")";
    let mut settings_expr = format!("json_remove({expr}");
    for k in &removed { settings_expr += &format!(", {}", arg(Sql::Text(format!("$.{k}")))); }
    settings_expr += ")";
    let mut transient_expr = "json_remove(transient_data".to_string();
    for k in &unlatched { transient_expr += &format!(", {}", arg(Sql::Text(format!("$.{k}")))); }
    transient_expr += ")";
    let at = arg(Sql::Integer(id));
    let (new_settings, new_transient): (String, String) = tx.query_row(&format!("SELECT {settings_expr}, {transient_expr} FROM bots WHERE id = {at}"),
        rusqlite::params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?;
    // Headroom for the first grouped refresh's `rust_composed_of` (one group): a row editable now stays editable after it. Its
    // longest form: a v4 uuid, a sha256 hex digest, the widest ranking id, the canonical ranked_at (index::recorded_ranked_at),
    // the widest f64 serde_json writes.
    let worst = serde_json::json!({ "rev": "00000000-0000-0000-0000-000000000000", "digest": "0".repeat(64),
        "ranked": { external: [i64::MIN, "0000-01-01T00:00:00.000000Z"] }, "shares": { "assets": {}, "groups": { external: -1.2345678901234567e-300 } } });
    let refreshed: String = tx.query_row(&format!("SELECT json_set(?1, '$.{}', json(?2))", composition::COMPOSED_OF_KEY), params![new_transient, worst.to_string()], |r| r.get(0))?;
    if crate::web::bot::draft::loadable(&new_settings, &refreshed).is_none() { return Ok(Err(Kept::Bounds)); }

    let n = tx.execute("UPDATE bots SET type = 'Bots::DcaMultiAsset', settings = ?1, transient_data = ?2, updated_at = ?3 WHERE id = ?4 AND type = 'Bots::DcaIndex'",
                       params![new_settings, new_transient, crate::codec::format_time(now), id])?;
    if n == 0 { return Err(EngineError::Data(format!("bot {id}: no longer an index bot"))); }
    // The group's members, always: every in_index member keeps its seat at the first derivation (index::incumbents). Composed
    // (they may stand for a retired index) only when this engine composed them from this very index.
    let composed = transient.get(index::COMPOSED_KEY).and_then(Value::as_str) == Some(external);
    tx.execute("DELETE FROM rust_bot_group_members WHERE bot_id = ?1", [id])?;
    for (asset, ticker, weight) in members {
        tx.execute("INSERT INTO rust_bot_group_members (bot_id, index_external_id, asset_id, ticker_id, composed, weight) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                   params![id, external, asset, ticker, composed, weight])?;
    }
    let working = BOT_WORKING.contains(&bot.status);
    // What the scheduler's rebalance fire evaluates (run::rebalance_candidate) and rebalance::run then acts on.
    let rebalances = bot.selling() && crate::web::bot::cast_boolean(settings.get("rebalance_enabled")) && super::run::rebalance_candidate(tx, id)?;
    Ok(Ok(Converted { id, selling: working && bot.selling(), rebalances }))
}

/// The in_index members as group rows (asset, ticker, weight), each weight the stored target as is. `Unreadable`: a row's ids
/// don't decode (that bot is kept, the pass goes on); `Targets`: a target can't be carried exactly (unreadable, NULL, or a
/// weight `save_member` would write back differently).
fn members(c: &Connection, bot: &Bot) -> Result<Result<Vec<Seat>, Kept>, EngineError> {
    let mut s = c.prepare("SELECT asset_id, ticker_id, target_allocation FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1 ORDER BY asset_id")?;
    // The ids as borrowed: only an integer is one (a text id, invalid UTF-8 included, is never decoded).
    let id = |v: ValueRef<'_>| match v { ValueRef::Integer(i) => Some(i), _ => None };
    let rows = s.query_map([bot.id], |r| Ok((id(r.get_ref(0)?), id(r.get_ref(1)?), basket::target_from_sql(r.get_ref(2)?))))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for (asset, ticker, target) in rows {
        let (Some(asset), Some(ticker)) = (asset, ticker) else { return Ok(Err(Kept::Unreadable)) };
        let Ok(Some(target)) = target else { return Ok(Err(Kept::Targets)) };
        let weight = target.to_f();
        if crate::v2::decimal_column(weight, 10, 6).ok() != Some(target) { return Ok(Err(Kept::Targets)); }
        out.push((asset, ticker, weight));
    }
    Ok(Ok(out))
}

/// `check`'s line for a bot `convert_all` converts.
pub fn would_convert(c: &Connection, bot: &Converted) -> Result<String, EngineError> {
    let will = match (bot.selling, bot.rebalances) { (true, true) => "buy and rebalance", (true, false) => "buy", (false, true) => "rebalance", (false, false) => "" };
    let note = if will.is_empty() { String::new() } else { format!(", and it will {will}: it is stored as selling, which an index bot never did") };
    Ok(format!("{} will be converted into a basket that follows its index at the next start{note}", super::set_aside::name(c, bot.id)?))
}

/// `check`'s line for a bot it keeps.
pub fn would_keep(c: &Connection, id: i64, why: Kept) -> Result<String, EngineError> {
    Ok(format!("{} stays an index bot for now: {}", super::set_aside::name(c, id)?, why.text()))
}

/// The start log's line for a converted bot.
pub fn converted(c: &Connection, id: i64) -> Result<String, EngineError> {
    Ok(format!("converted {} into a basket that follows its index", super::set_aside::name(c, id)?))
}
