//! A DCA basket (Bots::DcaMultiAsset with one or more members): its holdings, composition and buy split, ported from the
//! narrowed Bot::Composition::Measurable ledger walk, Bot::Composition::Allocatable and Bot::Composition::OrderSetter.
//! Only what an eligible bot can contain is ported; eligibility.rs refuses the rest (sells, non-REGULAR or imported rows,
//! rows without an asset, merged history, splits, market-cap weights). Exited members (in_index = false) are admitted as
//! holdings only: the walk counts them, the split never sees them.
use super::model::Bot;
use super::EngineError;
use crate::enums::TxExternalStatus;
use crate::ruby::{from_sql, BigDec};
use rusqlite::Connection;
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
