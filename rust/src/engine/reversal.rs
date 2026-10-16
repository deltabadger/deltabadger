//! Bot::Reversible's ⇄ flip, split as plan Revision r4 rules. The web's one write transaction (web::bot::write::reverse)
//! rotates the bot and records the work; it never calls a venue. The engine then cancels each captured order
//! (`cancel_captured`), and credits a cancelled buy's unspent reserve to the carry exactly once, when the order's final
//! state is known (`settle`). The pending credits are kept in Rails' own representation (`flip_reserve_pending`, PR #597),
//! so a handback leaves Rails able to finish them; the cancels are Rust's alone, and a handback waits for them.
use super::model::{self, Level};
use super::{accounting, EngineError};
use crate::enums::TxExternalStatus;
use crate::ruby::{BigDec, Num};
use crate::venue::{Attributed, OrderStatus, Venue};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

/// Bot::Reversible::FLIP_RESERVE_PENDING: ids of buys a flip cancelled whose unspent reserve is credited once final.
pub const RESERVE_PENDING: &str = "flip_reserve_pending";
/// Rust-only: the orders a reversal captured whose cancel outcome is not recorded yet, as
/// `[{"id": transaction id, "registered": this reversal put it in RESERVE_PENDING, "sent": a cancel may have reached the venue}]`.
pub const CANCELS: &str = "rust_reversal_cancels";

fn ids(v: Option<&Value>) -> Vec<i64> { v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() }
fn entries(bot: &model::Bot) -> Vec<Value> { bot.transient.get(CANCELS).and_then(Value::as_array).cloned().unwrap_or_default() }

/// Writes both keys; an empty one goes, as Rails' `.presence` + `compact` drops an empty pending list.
fn write(c: &Connection, bot_id: i64, pending: &[i64], cancels: &[Value]) -> Result<(), EngineError> {
    for (key, value) in [(RESERVE_PENDING, json!(pending)), (CANCELS, json!(cancels))] {
        let path = format!("$.{key}");
        if value.as_array().is_some_and(Vec::is_empty) {
            c.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?1) WHERE id = ?2", params![path, bot_id])?;
        } else {
            c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3", params![path, value.to_string(), bot_id])?;
        }
    }
    Ok(())
}

/// In the reverse's own transaction: every captured order is to be cancelled, and, when the flip leaves the buy side, every
/// captured buy is registered for its reserve (`register_flip_reserves!`: a union, so a repeated reversal never registers an
/// order twice). An order still awaiting an earlier reversal's cancel keeps that entry.
pub fn record(c: &Connection, bot_id: i64, captured: &[(i64, bool)], leaving_buy: bool) -> Result<(), EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    let mut pending = ids(bot.transient.get(RESERVE_PENDING));
    let mut cancels = entries(&bot);
    for &(id, buy) in captured {
        if cancels.iter().any(|e| e["id"] == id) { continue; }
        let registered = leaving_buy && buy && !pending.contains(&id);
        if registered { pending.push(id); }
        cancels.push(json!({ "id": id, "registered": registered, "sent": false }));
    }
    write(c, bot_id, &pending, &cancels)
}

/// Bots holding a cancel the engine has not recorded yet, whatever their status: a STOP does not withdraw a reversal's cancels.
/// The bots holding a reserve a reversal registered whose buy has not reached its final state yet.
pub fn pending_settlements(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare("SELECT id FROM bots WHERE json_array_length(transient_data, ?1) > 0 ORDER BY id")?;
    let rows = s.query_map([format!("$.{RESERVE_PENDING}")], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(rows)
}

pub fn unrecorded(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare("SELECT id FROM bots WHERE json_array_length(transient_data, ?1) > 0 ORDER BY id")?;
    let rows = s.query_map([format!("$.{CANCELS}")], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(rows)
}

/// Whether this bot may not place: an order a reversal captured has no recorded cancel outcome (the placement barrier,
/// checked inside the placement intent's own transaction).
pub fn barred(bot: &model::Bot) -> bool { !entries(bot).is_empty() }

enum Outcome { Cancelled, Failed }

/// Bot::Reversible#cancel_unfilled_orders for one bot's captured orders, best effort as there: a cancel the venue refuses is
/// logged as `order_cancel_failed` and the flip goes on. Each entry is marked `sent` (committed) before its cancel goes out,
/// so after a crash a repeated cancel that the venue refuses is checked against the order itself: one the venue reports
/// cancelled was cancelled by the first attempt. Returns the orders cancelled, for their follow-up poll.
pub async fn cancel_captured<V: Venue + Attributed>(c: &Connection, venue: &V, bot_id: i64, now: DateTime<Utc>) -> Result<Vec<i64>, EngineError> {
    let mut cancelled = vec![];
    for entry in entries(&model::load_bot(c, bot_id)?) {
        let Some(id) = entry["id"].as_i64() else { return Err(EngineError::Data(format!("bot {bot_id}: unreadable {CANCELS} entry {entry}"))) };
        let external: Option<String> = c.query_row("SELECT external_id FROM transactions WHERE id = ?1", [id], |r| r.get(0)).unwrap_or(None);
        let was_sent = entry["sent"] == true;
        let outcome = match external {
            None => Outcome::Failed, // no order to cancel: nothing is restored for it
            Some(ext) => {
                if !was_sent { mark_sent(c, bot_id, id)?; }
                match venue.cancel(&ext).await {
                    Ok(()) => Outcome::Cancelled,
                    Err(e) if was_sent => match venue.order_result(&ext, |_, _, _| Ok(true)).await.value {
                        Ok((found, _)) if found.iter().any(|o| o.txid == ext && o.status == OrderStatus::Cancelled) => Outcome::Cancelled,
                        _ => { super::log(&format!("[engine] bot {bot_id}: could not cancel order {ext} while reversing: {e:?}")); Outcome::Failed }
                    },
                    Err(e) => { super::log(&format!("[engine] bot {bot_id}: could not cancel order {ext} while reversing: {e:?}")); Outcome::Failed }
                }
            }
        };
        let ext: String = c.query_row("SELECT coalesce(external_id, '') FROM transactions WHERE id = ?1", [id], |r| r.get(0)).unwrap_or_default();
        model::credential_write(c, &venue.producer(), |tx| {
            let bot = model::load_bot(tx, bot_id)?;
            let mut cancels = entries(&bot);
            cancels.retain(|e| e["id"] != id);
            let mut pending = ids(bot.transient.get(RESERVE_PENDING));
            if let Outcome::Failed = outcome {
                // Rails never registers a buy whose cancel failed: it may still be live on the exchange.
                if entry["registered"] == true { pending.retain(|p| *p != id); }
                tx.execute("INSERT INTO bot_activity_logs (bot_id, event, level, message, details, created_at) VALUES (?1, 'order_cancel_failed', ?2, ?3, ?4, ?5)",
                    params![bot_id, Level::Warning as i64, format!("Could not cancel order {ext} while reversing — it may still be open on the exchange."),
                            json!({ "order_id": ext }).to_string(), crate::codec::format_time(now)])?;
            }
            write(tx, bot_id, &pending, &cancels)?;
            if let Outcome::Cancelled = outcome { settle(tx, bot_id, id)?; }
            Ok(())
        })?;
        if let Outcome::Cancelled = outcome { cancelled.push(id); }
    }
    Ok(cancelled)
}

fn mark_sent(c: &Connection, bot_id: i64, id: i64) -> Result<(), EngineError> {
    model::locked(c, |tx| {
        let mut cancels = entries(&model::load_bot(tx, bot_id)?);
        for e in &mut cancels { if e["id"] == id { e["sent"] = json!(true); } }
        tx.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3", params![format!("$.{CANCELS}"), json!(cancels).to_string(), bot_id])?;
        Ok(())
    })
}

/// Bot::Reversible#final_buy_remainder: what a stopped buy left unspent, or None while it is not final (or its fill is not known).
fn final_buy_remainder(row: &crate::figures::fill::StoredOrder) -> Option<BigDec> {
    if !matches!(row.external_status, Some(s) if s == TxExternalStatus::Closed as i64 || s == TxExternalStatus::Cancelled as i64) { return None; }
    let exec = row.amount_exec.clone().unwrap_or_else(BigDec::zero);
    if row.quote_amount_exec.is_none() && row.price.is_none() && exec.is_positive() { return None; }
    let price = row.price.clone().unwrap_or_else(BigDec::zero);
    let filled = row.quote_amount_exec.clone().unwrap_or_else(BigDec::zero).max(&exec * &price);
    // `quote_amount || (amount * price)`: Ruby raises on a nil there; such a row is never settled.
    let requested = match (&row.quote_amount, &row.amount, &row.price) { (Some(q), _, _) => q.clone(), (None, Some(a), Some(p)) => a * p, _ => return None };
    Some((&requested - &filled).max(BigDec::zero()))
}

/// Bot::Reversible#settle_flip_reserve!, in the caller's write transaction: a registered buy whose final state is known
/// returns its unspent reserve to the carry, capped at effective_quote_amount, and leaves the list in the same write, so a
/// repeated poll credits nothing. An abandoned order's fill is unknown: it leaves the list and nothing returns. Not before
/// the reversal's cancel of it is recorded: a cancel that then fails takes it off the list instead.
pub fn settle(c: &Connection, bot_id: i64, tx_id: i64) -> Result<bool, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    let mut pending = ids(bot.transient.get(RESERVE_PENDING));
    if !pending.contains(&tx_id) || entries(&bot).iter().any(|e| e["id"] == tx_id) { return Ok(false); }
    let row = crate::figures::fill::stored_order(c, tx_id)?;
    let remainder = final_buy_remainder(&row);
    if remainder.is_none() && row.external_status != Some(TxExternalStatus::Abandoned as i64) { return Ok(false); }
    let stored = accounting::carry_num(bot.transient.get("missed_quote_amount"))?.to_dec().map_err(EngineError::Arithmetic)?;
    let carry = match remainder.filter(BigDec::is_positive) {
        Some(r) => accounting::minimum(Num::Dec(&stored + &r), bot.effective_quote_amount_num()?)?,
        None => Num::Dec(stored),
    };
    pending.retain(|p| *p != tx_id);
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.missed_quote_amount', json(?1)) WHERE id = ?2",
              params![accounting::serialized(carry)?.to_string(), bot_id])?;
    write(c, bot_id, &pending, &entries(&bot))?;
    Ok(true)
}

/// The startup and periodic scan: every registered reserve whose order is already final is settled now; those whose order
/// still waits are returned (bot, transaction) to be polled again, so a settlement survives a STOP, further rotations and a
/// follow-up chain that ran out of attempts.
pub fn scan(c: &Connection) -> Result<Vec<(i64, i64)>, EngineError> {
    let mut s = c.prepare("SELECT id FROM bots WHERE json_array_length(transient_data, ?1) > 0 ORDER BY id")?;
    let bots = s.query_map([format!("$.{RESERVE_PENDING}")], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    drop(s);
    let mut waiting = vec![];
    for bot_id in bots {
        for tx_id in ids(model::load_bot(c, bot_id)?.transient.get(RESERVE_PENDING)) {
            if model::locked(c, |tx| settle(tx, bot_id, tx_id))? { continue; }
            let open: bool = c.query_row("SELECT status = 0 AND external_status IN (0, 1) FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0)).unwrap_or(false);
            if open { waiting.push((bot_id, tx_id)); }
        }
    }
    Ok(waiting)
}

/// Tests only: a reversal's recorded work, as (pending reserve ids, unrecorded cancel ids).
pub fn work(bot: &model::Bot) -> (Vec<i64>, Vec<i64>) {
    (ids(bot.transient.get(RESERVE_PENDING)), entries(bot).iter().filter_map(|e| e["id"].as_i64()).collect())
}
