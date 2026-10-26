//! Shared composition-operation state and periodic liquidation settlement.
use super::{model::{self, Bot, Level}, amount::TxKind, EngineError};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};

pub const SELLING_TTL_SECONDS: i64 = 15 * 60;

fn present(bot: &Bot, key: &str) -> bool {
    bot.transient.get(key).is_some_and(|v| match v {
        Value::Null | Value::Bool(false) => false,
        Value::Object(o) => !o.is_empty(), Value::Array(a) => !a.is_empty(),
        Value::String(s) => !s.trim().is_empty(), _ => true,
    })
}

pub fn in_flight(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<bool, EngineError> {
    busy(c, bot, now, true)
}

/// Split excludes redeploy, leaving it to its modal. The other blockers are shared.
pub fn busy(c: &Connection, bot: &Bot, now: DateTime<Utc>, redeploy: bool) -> Result<bool, EngineError> {
    Ok(present(bot, "rebalance_pending") || liquidation_in_flight(c, bot, now)?
        || (redeploy && redeploy_in_flight(c, bot)?))
}

pub fn liquidation_in_flight(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let request = &bot.transient["liquidation_selling_since"];
    // Rust requests remain owned until compare-cleared, including taken requests after a crash.
    // Rails markers have no holdings; only their page spinner's 15-minute TTL applies.
    let selling = request.is_object() && (request.get("holdings").is_some()
        || request["at"].as_i64().or_else(|| request["at"].as_str()?.parse().ok()).unwrap_or(0) > now.timestamp() - SELLING_TTL_SECONDS);
    let abandoned = unresolved_liquidations(c, bot)?;
    Ok(present(bot, "liquidation_pending") || selling || waiting(c, bot.id, "LIQUIDATION")? || abandoned)
}

pub(crate) fn unresolved_liquidations(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    let resolved: Vec<i64> = match bot.transient.get("liquidation_resolved_orders") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())).collect(),
        _ => vec![],
    };
    let mut s = c.prepare("SELECT id FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND external_status=4")?;
    let abandoned = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().any(|id| !resolved.contains(&id));
    Ok(abandoned)
}

pub fn redeploy_in_flight(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    Ok(present(bot, "redeploy_pending") || present(bot, "rust_redeploy_request") || waiting(c, bot.id, "REDEPLOY")?)
}

fn waiting(c: &Connection, id: i64, kind: &str) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND transaction_type=?2 AND status=0 AND external_status IN (0,1))", (id, kind), |r| r.get(0))?)
}

pub(crate) fn name(kind: TxKind) -> Option<&'static str> {
    match kind { TxKind::Regular | TxKind::Rebalance => None, TxKind::Liquidation => Some("liquidation"), TxKind::Redeploy => Some("redeploy") }
}

pub(crate) fn start(c: &Connection, bot_id: i64, kind: TxKind, id: &str, symbol: Option<&str>) -> Result<(), EngineError> {
    let Some(name) = name(kind) else { return Ok(()) };
    let mut pending = json!({"id":id,"state":"placing"});
    if let Some(symbol) = symbol { pending["symbol"] = json!(symbol); }
    model::merge_transient_compact(c, bot_id, &[(&format!("{name}_pending"), pending)])
}

/// Idempotent promotion, preserving the generation nonce and the liquidation's symbol.
pub(crate) fn halt(c: &Connection, bot_id: i64, kind: TxKind, reason: &str, details: Value, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let Some(name) = name(kind) else { return Ok(false) };
    let bot = model::load_bot(c, bot_id)?;
    let key = format!("{name}_pending");
    let Some(mut pending) = bot.transient.get(&key).filter(|p| p.is_object()).cloned() else { return Ok(false) };
    if pending["state"] == "ambiguous" { return Ok(false); }
    pending["state"] = json!("ambiguous");
    model::merge_transient_compact(c, bot_id, &[(&key, pending)])?;
    let mut details = details;
    details["reason"] = json!(reason);
    model::log_activity(c, bot_id, &format!("{name}_ambiguous"), Level::Error, details, now)?;
    Ok(true)
}

/// A Rails-only placing marker survived its worker. Unlike selling_since, it has no timestamp or TTL
/// (LiquidationState#promote_stale_liquidation_placement!); the engine's exclusive ownership permits promotion.
pub fn promote_orphans(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::locked(c, |c| {
        for kind in [TxKind::Liquidation, TxKind::Redeploy] { promote_orphan(c, bot_id, kind, now)?; }
        Ok(())
    })
}

pub(crate) fn promote_orphan(c: &Connection, bot_id: i64, kind: TxKind, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::locked(c, |c| {
        let bot = model::load_bot(c, bot_id)?;
        if bot.rust_placement().is_some() { return Ok(()); }
        let Some(name) = name(kind) else { return Ok(()) };
        if bot.transient[format!("{name}_pending")]["state"] == "placing" {
            halt(c, bot_id, kind, "placement intent survived its worker", json!({}), now)?;
        }
        Ok(())
    })
}


/// The watched ids belong to this sweep; a terminal row is not announced twice.
pub(crate) fn waiting_liquidations(c: &Connection, bot: Option<i64>) -> Result<Vec<(i64,i64)>, EngineError> {
    let mut s=c.prepare("SELECT bot_id,id FROM transactions WHERE transaction_type='LIQUIDATION' AND status=0 AND external_status IN (0,1) AND (?1 IS NULL OR bot_id=?1) ORDER BY id")?;
    let rows=s.query_map([bot],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
    Ok(rows)
}
pub(crate) fn halt_abandoned(c: &Connection, watched: &[(i64,i64)], now: DateTime<Utc>) -> Result<(), EngineError> {
    let mut bots: std::collections::BTreeMap<i64,Vec<String>>=Default::default();
    for (bot,id) in watched {
        use rusqlite::OptionalExtension;
        let base:Option<String>=c.query_row("SELECT base FROM transactions WHERE id=?1 AND bot_id=?2 AND external_status=4",(*id,*bot),|r|r.get(0)).optional()?.flatten();
        if let Some(base)=base {
            let names=bots.entry(*bot).or_default();
            if !names.contains(&base) { names.push(base); }
        }
    }
    for (bot,names) in bots { model::log_activity(c,bot,"liquidation_ambiguous",Level::Error,json!({"base":names.join(", "),"reason":"exchange stopped reporting the order"}),now)?; }
    Ok(())
}
#[derive(Default)]
pub(crate) struct Scan { pub waiting: Vec<(i64,i64)>, pub late: Vec<(i64,i64)> }
pub(crate) fn scan(c: &Connection, now: DateTime<Utc>) -> Result<Scan,EngineError> {
    let waiting=waiting_liquidations(c,None)?;
    // ponytail: re-read late proceeds for seven days after the terminal update; widen if a venue reports them later.
    // Rails has no closed_at column: updated_at records the poll that closed/cancelled the row.
    let mut s=c.prepare("SELECT bot_id,id FROM transactions WHERE transaction_type='LIQUIDATION' AND status=0 AND external_status IN (2,3) AND amount_exec>0 AND COALESCE(quote_amount_exec,0)=0 AND updated_at>=?1 ORDER BY id")?;
    let late=s.query_map([crate::codec::format_time(now-chrono::Duration::days(7))],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
    Ok(Scan {waiting,late})
}
