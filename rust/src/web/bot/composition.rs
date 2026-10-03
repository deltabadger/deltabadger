//! Manual composition's after-save callback, inside the settings writer's transaction.
use super::Bot;
use crate::{codec, ruby::BigDec, web::{format::Num, WebError}};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

pub fn reconcile(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<(), WebError> {
    if bot.text("weighting") == Some("market_cap") { return Ok(()); }
    let weights = bot.allocations();
    if weights.len() > 1000 { return Err(super::start::history_error()); }
    let matched: Vec<_> = weights.iter().filter_map(|(asset, weight)| {
        bot.tickers.iter().find(|ticker| ticker.base_asset_id == *asset && ticker.tradable())
            .map(|ticker| (*asset, ticker.id, *weight))
    }).collect();
    let total = super::float_sum(matched.iter().map(|(_,_,weight)| *weight));
    // Rails returns a failed derivation; the after-save callback does not abort the save.
    if total <= 0.0 { return Ok(()); }
    if !total.is_finite() { return Err(super::start::history_error()); }
    let at = codec::format_time(now);
    let mut statement = c.prepare("SELECT id, asset_id FROM bot_index_assets WHERE bot_id=?1 AND in_index=1 LIMIT 1001")?;
    let rows = statement.query_map([bot.id], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    if rows.len() > 1000 { return Err(super::start::history_error()); }
    for (id, asset) in rows {
        if !matched.iter().any(|(member,_,_)| *member == asset) {
            // update_all deliberately does not touch updated_at or current_allocation.
            if c.execute("UPDATE bot_index_assets SET in_index=0, exited_at=?1 WHERE id=?2 AND bot_id=?3", (&at,id,bot.id))? != 1 {
                return Err(super::data("composition member disappeared".into()));
            }
        }
    }
    for (asset, ticker, weight) in matched {
        let weight = BigDec::from_f64(weight / total).map_err(|_| super::start::history_error())?.round(6);
        let existing = c.query_row("SELECT id,ticker_id,target_allocation,in_index,entered_at,exited_at FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", (bot.id,asset), |r|
            Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,super::Stored>(2)?.0,r.get::<_,Option<bool>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?))).optional()?;
        if let Some((id,old_ticker,old_weight,in_index,entered,exited)) = existing {
            let dirty = old_ticker != ticker || old_weight.as_ref() != Some(&weight) || in_index != Some(true) || entered.is_none() || exited.is_some();
            if dirty && c.execute("UPDATE bot_index_assets SET ticker_id=?1,target_allocation=?2,in_index=1,entered_at=COALESCE(entered_at,?3),exited_at=NULL,updated_at=?3 WHERE id=?4 AND bot_id=?5",
                (ticker,Num::Dec(weight).to_f(),&at,id,bot.id))? != 1 { return Err(super::data("composition member disappeared".into())); }
        } else {
            c.execute("INSERT INTO bot_index_assets(bot_id,asset_id,ticker_id,target_allocation,in_index,entered_at,created_at,updated_at) VALUES(?1,?2,?3,?4,1,?5,?5,?5)",
                (bot.id,asset,ticker,Num::Dec(weight).to_f(),&at))?;
        }
    }
    Ok(())
}
