//! A DCA basket (Bots::DcaMultiAsset with one or more members): its holdings, composition and buy split, ported from the
//! narrowed Bot::Composition::Measurable ledger walk, Bot::Composition::Allocatable and Bot::Composition::OrderSetter.
//! Only what an eligible bot can contain is ported; eligibility.rs refuses the rest (sells, non-REGULAR or imported rows,
//! rows without an asset, merged history, splits, market-cap weights). Exited members (in_index = false) are admitted as
//! holdings only: the walk counts them, the split never sees them.
use super::model::{self, Bot, Ticker};
use super::EngineError;
use crate::codec::format_time;
use crate::enums::TxExternalStatus;
use crate::ruby::{decimal_column, float_sum, from_sql, BigDec};
use chrono::{DateTime, Utc};
use rusqlite::types::{Value as Sql, ValueRef};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

/// Base amount held, by asset id: `metrics(force: true)[:asset_breakdown][key_for(asset_id)][:amount]` for what an eligible
/// bot holds. Every submitted row of the bot, whatever its external status, goes through Transaction.confirmed_exec_amounts
/// (a closed row without executions reads its requested ones). A row whose price, executed quote or executed base is blank,
/// or whose executed quote or base is zero, adds nothing (measurable.rb:153-154: Alpaca reports a zero quote before it
/// knows the average price). Every other row is a REGULAR buy and adds its executed base amount
/// (Bot::RebalanceAccounting#apply_regular_buy). An asset with nothing applied is absent, as in Rails.
pub fn holdings(c: &Connection, bot: &Bot) -> Result<HashMap<i64, BigDec>, EngineError> {
    let mut s = c.prepare(
        "SELECT base_asset_id, external_status, price, amount, amount_exec, quote_amount_exec, side, transaction_type \
         FROM transactions WHERE bot_id = ?1 AND status = 0 ORDER BY created_at, id")?;
    let mut rows = s.query([bot.id])?;
    let mut out: HashMap<i64, BigDec> = HashMap::new();
    while let Some(r) = rows.next()? {
        let (asset, side, kind): (Option<i64>, Option<i64>, String) = (r.get(0)?, r.get(6)?, r.get(7)?);
        // Eligibility refuses all three. Met anyway, the walk this build ports would be wrong for the bot, so its tick fails.
        let Some(asset) = asset.filter(|_| side == Some(0) && kind == "REGULAR") else {
            return Err(EngineError::Data(format!("bot {}: a sell, a non-REGULAR row or a row without an asset is outside the ported ledger walk", bot.id)));
        };
        let dec = |i: usize| -> Result<Option<BigDec>, EngineError> { from_sql(r.get_ref(i)?).map_err(data) };
        let (price, amount, mut amount_exec, mut quote_exec) = (dec(2)?, dec(3)?, dec(4)?, dec(5)?);
        if r.get::<_, Option<i64>>(1)? == Some(TxExternalStatus::Closed as i64) {
            if quote_exec.is_none() { if let (Some(p), Some(a)) = (&price, &amount) { quote_exec = Some(p * a); } }
            if amount_exec.is_none() { amount_exec = amount.clone(); }
        }
        let (Some(_), Some(q), Some(a)) = (&price, &quote_exec, &amount_exec) else { continue };
        if q.is_zero() || a.is_zero() { continue; }
        let held = out.remove(&asset).unwrap_or_else(BigDec::zero);
        out.insert(asset, &held + a);
    }
    Ok(out)
}

/// Bot::Composition::OrderSetter#reserved_waiting_amounts(:buy): the unexecuted remainder (`amount.to_d - amount_exec.to_d`,
/// blank reading 0) of every waiting buy, by asset. A resting order counts as held, so its member is not bought twice.
pub fn reserved(c: &Connection, bot: &Bot) -> Result<HashMap<i64, BigDec>, EngineError> {
    let mut s = c.prepare("SELECT base_asset_id, amount, amount_exec FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1) AND side = 0")?;
    let mut rows = s.query([bot.id])?;
    let mut out: HashMap<i64, BigDec> = HashMap::new();
    while let Some(r) = rows.next()? {
        let dec = |i: usize| -> Result<BigDec, EngineError> { Ok(from_sql(r.get_ref(i)?).map_err(data)?.unwrap_or_else(BigDec::zero)) };
        let remainder = &dec(1)? - &dec(2)?;
        if !remainder.is_positive() { continue; }
        // Rails matches an asset-less row by name or stands the tick down (order_setter.rb:174-175); eligibility refuses such rows.
        let asset = r.get::<_, Option<i64>>(0)?.ok_or_else(|| EngineError::Data(format!("bot {}: a resting order without base_asset_id", bot.id)))?;
        let held = out.remove(&asset).unwrap_or_else(BigDec::zero);
        out.insert(asset, &held + &remainder);
    }
    Ok(out)
}

/// bot_index_assets.target_allocation as ActiveRecord reads it back: ActiveModel::Type::Decimal(precision 10, scale 6). A REAL
/// goes through the Float cast; an INTEGER or TEXT through BigDecimal, then round(6).
pub fn target_from_sql(v: ValueRef<'_>) -> Result<Option<BigDec>, EngineError> {
    match v {
        ValueRef::Real(f) => decimal_column(f, 10, 6).map(Some).map_err(data),
        other => Ok(from_sql(other).map_err(data)?.map(|d| d.round(6))),
    }
}

/// Bots::DcaMultiAsset#refresh_composition, on every tick before anything is sized: derive_composition, then
/// update_bot_index_assets. Err(message) is Rails' Failure, which fails the tick; nothing is written then.
pub fn refresh_composition(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Result<(), String>, EngineError> {
    let allocations = bot.allocations().ok_or_else(|| EngineError::Data(format!("bot {}: allocations this build does not read", bot.id)))?;
    // derive_composition: in settings order, a member counts only with an available, trading-enabled ticker at the quote.
    let mut matched: Vec<(i64, i64, f64)> = vec![];
    for (asset_id, weight) in allocations {
        let ticker: Option<i64> = c.query_row(
            "SELECT id FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2 AND quote_asset_id = ?3 AND available = 1 AND trading_enabled = 1",
            params![bot.exchange_id, asset_id, bot.quote_asset_id()], |r| r.get(0)).optional()?;
        if let Some(ticker_id) = ticker { matched.push((asset_id, ticker_id, weight)); }
    }
    // `matched.sum { weight }` is Ruby's compensated Float sum; each weight is renormalised in Float (`weight / total`).
    let total = float_sum(&matched.iter().map(|(_, _, w)| *w).collect::<Vec<_>>());
    if total <= 0.0 { return Ok(Err(format!("None of the portfolio's weighted assets trade on {}", model::exchange_name(c, bot)?))); }
    let kept: Vec<i64> = matched.iter().map(|(asset, _, _)| *asset).collect();
    // Rails' `transaction do`: nested in the caller's transaction if there is one.
    model::locked(c, |tx| {
        // A member that dropped out keeps its row as a holding: update_all, so updated_at stays.
        tx.execute("UPDATE bot_index_assets SET in_index = 0, exited_at = ?1 WHERE bot_id = ?2 AND in_index = 1 AND asset_id NOT IN (SELECT value FROM json_each(?3))",
                   params![format_time(now), bot.id, serde_json::to_string(&kept).expect("ids serialise")])?;
        for (asset_id, ticker_id, weight) in matched { save_member(tx, bot.id, asset_id, ticker_id, weight / total, now)?; }
        Ok(())
    })?;
    Ok(Ok(()))
}

/// A member's row: id, ticker_id, target_allocation (as read back), in_index, entered_at, exited_at.
type MemberRow = (i64, i64, Result<Option<BigDec>, EngineError>, Option<bool>, Option<String>, Option<String>);

/// Allocatable#save_member: find or initialize the member's row and save it. ActiveRecord writes only the attributes that
/// changed, and moves updated_at only then; a new row is inserted with created_at, updated_at and entered_at at now.
fn save_member(c: &Connection, bot_id: i64, asset_id: i64, ticker_id: i64, weight: f64, now: DateTime<Utc>) -> Result<(), EngineError> {
    let target = decimal_column(weight, 10, 6).map_err(data)?;
    let now_text = format_time(now);
    let row: Option<MemberRow> = c.query_row(
        "SELECT id, ticker_id, target_allocation, in_index, entered_at, exited_at FROM bot_index_assets WHERE bot_id = ?1 AND asset_id = ?2",
        params![bot_id, asset_id],
        |r| Ok((r.get(0)?, r.get(1)?, target_from_sql(r.get_ref(2)?), r.get(3)?, r.get(4)?, r.get(5)?))).optional()?;
    let Some((id, old_ticker, old_target, in_index, entered_at, exited_at)) = row else {
        c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5, ?5)", params![bot_id, asset_id, ticker_id, target.to_f(), now_text])?;
        return Ok(());
    };
    let mut sets: Vec<(&str, Sql)> = vec![];
    if old_ticker != ticker_id { sets.push(("ticker_id", Sql::Integer(ticker_id))); }
    if old_target? != Some(target.clone()) { sets.push(("target_allocation", Sql::Real(target.to_f()))); }
    if in_index != Some(true) { sets.push(("in_index", Sql::Integer(1))); }
    if entered_at.is_none() { sets.push(("entered_at", Sql::Text(now_text.clone()))); }
    if exited_at.is_some() { sets.push(("exited_at", Sql::Null)); }
    if sets.is_empty() { return Ok(()); }
    let assignments: Vec<String> = sets.iter().enumerate().map(|(i, (col, _))| format!("{col} = ?{}", i + 1)).collect();
    let mut values: Vec<Sql> = sets.into_iter().map(|(_, v)| v).collect();
    values.push(Sql::Text(now_text));
    values.push(Sql::Integer(id));
    c.execute(&format!("UPDATE bot_index_assets SET {}, updated_at = ?{} WHERE id = ?{}", assignments.join(", "), values.len() - 1, values.len()),
              rusqlite::params_from_iter(values))?;
    Ok(())
}

/// A member as get_orders_data takes it: its ticker (bot_index_assets.ticker_id) and its weight, already `.to_f`.
#[derive(Clone, Debug)]
pub struct Member { pub asset_id: i64, pub ticker: Ticker, pub weight: f64 }

/// Bot::Composition::Allocatable#buyable_allocations with no wash-sale lock (refused by eligibility): the in_index rows by
/// target_allocation DESC, ties by id (Rails' order is undefined there), re-weighted to `t / total` in BigDecimal only
/// when the total is positive and not exactly 1 (thirds stored as 0.333333 are), then `.to_f` (order_setter.rb:406).
pub fn members(c: &Connection, bot: &Bot) -> Result<Vec<Member>, EngineError> {
    let mut s = c.prepare("SELECT asset_id, ticker_id, target_allocation FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1 ORDER BY target_allocation DESC, id")?;
    let rows = s.query_map([bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, target_from_sql(r.get_ref(2)?))))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut weights = vec![];
    for (asset_id, ticker_id, target) in rows { weights.push((asset_id, ticker_id, target?.unwrap_or_else(BigDec::zero))); }
    let total = weights.iter().fold(BigDec::zero(), |acc, (_, _, t)| &acc + t);
    let renormalise = total.is_positive() && total != BigDec::one();
    let mut out = vec![];
    for (asset_id, ticker_id, t) in weights {
        let weight = if renormalise { t.div(&total).expect("the total is positive").to_f() } else { t.to_f() };
        let ticker = model::ticker_by_id(c, bot.exchange_id, ticker_id)?
            .ok_or_else(|| EngineError::Data(format!("bot {}: member ticker {ticker_id} is not on this venue", bot.id)))?;
        out.push(Member { asset_id, ticker, weight });
    }
    Ok(out)
}

/// The pairs of every allocation, in settings order, whatever their availability: what a parity copy must script prices for.
pub fn member_pairs(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let mut out = vec![];
    for (asset_id, _) in bot.allocations().unwrap_or_default() {
        if let Some(t) = model::ticker_for_asset(c, bot, asset_id)? { out.push(t.ticker); }
    }
    Ok(out)
}
