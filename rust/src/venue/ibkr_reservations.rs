//! IBKR sell reservations (Ibkr plan I4b, D-IBKR-SELLCAP). A margin account can short: IBKR fills a sell of more shares
//! than are held. Rails has no cap. Here a sell intent reserves what it sells, in the intent's own transaction, and is
//! placed only while the position covers it beside this install's other unsettled sells of the same asset:
//!
//!   available = position − Σ unreleased (quantity − coalesce(amount_exec, 0)) on (account_ref, the asset's conids)
//!
//! The position is read under the login's lock (`PlacementHold`), which the order polls also take, so a fill is either in
//! the position read or still counted by its reservation. Every conid `ibkr_conids` maps to the asset counts: a reservation
//! made before the listing's conid was replaced still holds. A reservation is linked to its order row when that row is
//! written (`link`), released once the row is closed or cancelled, or once its intent is gone without a row (nothing was
//! placed); an abandoned order keeps it. Only our own orders are counted: trading outside Deltabadger can still short.
use super::ibkr::IBKR;
use super::{PlacementHold, Side};
use crate::codec::format_time;
use crate::engine::amount::OrderPlan;
use crate::engine::model::{self, Bot};
use crate::engine::EngineError;
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// An IBKR intent written without its venue's hold: never placed.
pub const NO_HOLD: &str = "an IBKR order needs its account and position read under the login's lock; not placed";

fn migrated(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT count(*) = 1 FROM sqlite_master WHERE type = 'table' AND name = 'ibkr_reservations'", [], |r| r.get(0))
}

/// Inside the intent's transaction, for bot `bot`'s order `plan` (intent `cl_ord_id`): why it is not placed, or None. A
/// sell that is placed has its reservation written. Nothing for a bot on another venue.
pub fn reserve(c: &Connection, bot: &Bot, plan: &OrderPlan, cl_ord_id: &str, hold: Option<&PlacementHold>, now: DateTime<Utc>) -> Result<Option<String>, EngineError> {
    if model::exchange_type(c, bot)? != IBKR { return Ok(None) }
    let Some(hold) = hold else { return Ok(Some(NO_HOLD.into())) };
    if let Some(why) = super::ibkr_account::placement_refusal(c, bot.id, &hold.account_ref)? { return Ok(Some(why)) }
    if plan.ticker.ticker.trim().parse::<i64>().ok() != Some(hold.conid) {
        return Ok(Some(format!("the order's contract {} is not the one its position was read for", plan.ticker.ticker)));
    }
    let listed: bool = c.query_row("SELECT count(*) > 0 FROM ibkr_conids WHERE conid = ?1 AND asset_id = ?2", params![hold.conid, plan.ticker.base_asset_id], |r| r.get(0))?;
    if !listed { return Ok(Some(format!("IBKR contract {} is not the catalogue's listing of this asset", hold.conid))) }
    if plan.side == Side::Buy { return Ok(None) }
    let Some(position) = &hold.position else { return Ok(Some(NO_HOLD.into())) };
    release_settled(c, now)?;
    let available = position - &outstanding(c, &hold.account_ref, plan.ticker.base_asset_id, hold.conid)?;
    if plan.volume > available {
        return Ok(Some(format!("selling {} shares of IBKR contract {} would exceed the {} held and not already being sold",
                               shares(&plan.volume), hold.conid, shares(&available.max(BigDec::zero())))));
    }
    let key = match super::ibkr_account::ibkr_key(c, bot.id)? { Some(Some(key)) => key, _ => return Ok(Some(NO_HOLD.into())) };
    c.execute("INSERT INTO ibkr_reservations (api_key_id, account_ref, conid, bot_id, cl_ord_id, transaction_id, quantity, created_at, released_at) \
               VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, NULL)",
              params![key, hold.account_ref, hold.conid, bot.id, cl_ord_id, shares(&plan.volume), format_time(now)])?;
    Ok(None)
}

/// A share count as text: "10", "2.5".
pub fn shares(d: &BigDec) -> String { let t = d.to_s_f(); t.strip_suffix(".0").map(str::to_string).unwrap_or(t) }

/// What the unreleased reservations of `account_ref` on `asset_id`'s conids (and `conid`) still hold.
fn outstanding(c: &Connection, account_ref: &str, asset_id: i64, conid: i64) -> Result<BigDec, EngineError> {
    let mut s = c.prepare("SELECT r.quantity, CAST(t.amount_exec AS TEXT) FROM ibkr_reservations r LEFT JOIN transactions t ON t.id = r.transaction_id \
                           WHERE r.released_at IS NULL AND r.account_ref = ?1 AND (r.conid = ?3 OR r.conid IN (SELECT conid FROM ibkr_conids WHERE asset_id = ?2))")?;
    let rows = s.query_map(params![account_ref, asset_id, conid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
    let bad = |v: &str| EngineError::Data(format!("an IBKR reservation reads {v}"));
    let mut total = BigDec::zero();
    for row in rows {
        let (quantity, exec) = row?;
        let quantity = BigDec::parse(&quantity).map_err(|_| bad(&quantity))?;
        let exec = match exec { Some(e) => BigDec::parse(&e).map_err(|_| bad(&e))?, None => BigDec::zero() };
        total = &total + &(&quantity - &exec).max(BigDec::zero());
    }
    Ok(total)
}

/// Releases every reservation whose order row is closed or cancelled, and every one whose intent is gone without a row.
pub fn release_settled(c: &Connection, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    if !migrated(c)? { return Ok(0) }
    c.execute("UPDATE ibkr_reservations SET released_at = ?1 WHERE released_at IS NULL AND ( \
                 (transaction_id IS NOT NULL AND (SELECT external_status FROM transactions WHERE id = ibkr_reservations.transaction_id) IN (2, 3)) \
                 OR (transaction_id IS NULL AND NOT EXISTS (SELECT 1 FROM bots b WHERE b.id = ibkr_reservations.bot_id \
                     AND json_extract(b.transient_data, '$.rust_placement.cl_ord_id') = ibkr_reservations.cl_ord_id)))", [format_time(now)])
}

/// Bot `bot_id`'s order row `transaction_id`, written for its unresolved intent: that intent's reservation names it. Called
/// before the intent is removed, in the same transaction (amount::write_accepted_order).
pub fn link(c: &Connection, bot_id: i64, transaction_id: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE ibkr_reservations SET transaction_id = ?2 WHERE bot_id = ?1 AND transaction_id IS NULL AND released_at IS NULL \
               AND cl_ord_id = (SELECT json_extract(transient_data, '$.rust_placement.cl_ord_id') FROM bots WHERE id = ?1)", params![bot_id, transaction_id]).map(|_| ())
}

/// D-IBKR-HANDBACK: Rails would trade IBKR bots without the account binding, the reservations or the recovery by cOID, so
/// the handback is refused while any IBKR bot that is not deleted, open IBKR order, unresolved IBKR intent or unreleased
/// reservation exists. One line per finding.
pub fn handback_refusals(c: &Connection, now: DateTime<Utc>) -> rusqlite::Result<Vec<String>> {
    let mut out = vec![];
    let mut each = |sql: &str, line: &dyn Fn(i64) -> String| -> rusqlite::Result<()> {
        let mut s = c.prepare(sql)?;
        for id in s.query_map([IBKR], |r| r.get::<_, i64>(0))? { out.push(line(id?)); }
        Ok(())
    };
    each("SELECT b.id FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE e.type = ?1 AND b.status <> 3 ORDER BY b.id",
         &|id| format!("bot {id}: an IBKR bot; only this engine trades IBKR, so delete it before handing back"))?;
    each("SELECT t.id FROM transactions t JOIN exchanges e ON e.id = t.exchange_id WHERE e.type = ?1 AND t.status = 0 AND t.external_status IN (0, 1) ORDER BY t.id",
         &|id| format!("transaction {id}: an open IBKR order"))?;
    each("SELECT b.id FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE e.type = ?1 AND json_extract(b.transient_data, '$.rust_placement') IS NOT NULL ORDER BY b.id",
         &|id| format!("bot {id}: an unresolved IBKR order"))?;
    if migrated(c)? {
        release_settled(c, now)?;
        each("SELECT id FROM ibkr_reservations WHERE released_at IS NULL AND ?1 IS NOT NULL ORDER BY id",
             &|id| format!("IBKR reservation {id}: shares held for an order that has not settled"))?;
    }
    Ok(out)
}
