//! D-BINANCE-ORDER-LINK (AUDIT #15): a Binance or Binance.US myTrades fill names its bot order. Rails' matcher looks
//! the order up by the fill's whole id ("SYMBOL-orderId-id") or a snake-case `order_id` the fill never has, so no
//! Binance fill ever linked. The link is display-only: the tracker row's bot cell is its one reader.
//!
//! A row whose raw carries `orderId` is a fill, and this resolver is its only path: the generic lookups never see it.
//! New fills link as they are stored (`ledger::bot_order`); stored history links through `OrderLinks`, woken at start.

use super::activities::{BUY, SELL, SWAP_IN, SWAP_OUT};
use crate::venue::binance::{BINANCE, BINANCE_US};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub const JOB: &str = "binance_order_links";
const BATCH: i64 = 500;

pub fn venue(exchange_type: &str) -> bool { [BINANCE, BINANCE_US].contains(&exchange_type) }

/// None: not a fill (no `orderId`), the generic matcher's row. Some(None): a fill naming no order this can prove.
/// Some(Some(id)): the order's external id, "SYMBOL-orderId", when the whole tx_id is "SYMBOL-orderId-id" with an
/// optional "-in"/"-out" leg suffix, and raw `orderId` and `id` are those same decimal numbers.
pub fn fill_order(tx_id: Option<&str>, raw: &Value) -> Option<Option<String>> {
    let order = raw.get("orderId")?;
    Some((|| {
        let tx_id = tx_id?;
        let body = tx_id.strip_suffix("-in").or_else(|| tx_id.strip_suffix("-out")).unwrap_or(tx_id);
        let [symbol, order_id, fill_id] = body.split('-').collect::<Vec<_>>()[..] else { return None };
        let symbol_ok = !symbol.is_empty() && symbol.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        (symbol_ok && decimal(order)? == order_id && decimal(raw.get("id")?)? == fill_id).then(|| format!("{symbol}-{order_id}"))
    })())
}

/// An id as canonical decimal digits: a non-negative integer, or a string of digits without a leading zero.
fn decimal(v: &Value) -> Option<String> {
    let s = match v { Value::Number(n) => n.as_u64()?.to_string(), Value::String(s) => s.clone(), _ => return None };
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))).then_some(s)
}

/// The submitted order of the owner's bots on this exchange with that external id (unique per exchange), or no link.
pub fn order(c: &Connection, owner: i64, exchange_id: i64, external_id: &str) -> rusqlite::Result<Option<i64>> {
    c.prepare_cached("SELECT id FROM transactions WHERE external_id = ?1 AND exchange_id = ?2 AND status = 0 \
                      AND bot_id IN (SELECT id FROM bots WHERE user_id = ?3)")?
        .query_row(params![external_id, exchange_id, owner], |r| r.get(0)).optional()
}

/// One pass over every stored Binance/Binance.US trade row with no link, by id. The cursor moves past every row it
/// reads, linked or not, so rows that never link don't hold it. Returns the rows linked.
pub fn link_stored(c: &Connection, cipher: &crate::crypto::Cipher, now: chrono::DateTime<chrono::Utc>) -> Result<usize, String> {
    let (mut cursor, mut linked) = (0, 0);
    while let Some((last, n)) = link_batch(c, cipher, cursor, now)? { cursor = last; linked += n; }
    Ok(linked)
}

/// One batch of `link_stored`: the 500 unlinked trade rows after `cursor`, in one immediate transaction (a concurrent
/// writer can't invalidate its read before its writes). Only `transaction_id` and `updated_at` are written, and only
/// where the link is still NULL. None: no rows left, or Rust no longer holds the instance. Some: (last id read, linked).
pub fn link_batch(c: &Connection, cipher: &crate::crypto::Cipher, cursor: i64, now: chrono::DateTime<chrono::Utc>) -> Result<Option<(i64, usize)>, String> {
    let tx = crate::engine::model::immediate_retry_busy(c).map_err(|e| format!("{e:?}"))?;
    // The job runs only while Rust holds the instance; a handback mid-pass stops it before the next write.
    if !crate::lease::read(&tx, cipher).map_err(|_| "unreadable ownership".to_string())?.is_some_and(|v| v["engine"] == "rust") { return Ok(None) }
    let rows: Vec<(i64, i64, i64, Option<String>, String)> = {
        let mut s = tx.prepare_cached("SELECT t.id, t.user_id, t.exchange_id, t.tx_id, t.raw_data FROM account_transactions t \
            JOIN exchanges e ON e.id = t.exchange_id WHERE t.id > ?1 AND t.transaction_id IS NULL AND e.type IN (?2, ?3) \
            AND t.entry_type IN (?4, ?5, ?6, ?7) ORDER BY t.id LIMIT ?8").map_err(|e| e.to_string())?;
        let rows = s.query_map(params![cursor, BINANCE, BINANCE_US, BUY, SELL, SWAP_IN, SWAP_OUT, BATCH],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default())))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    let Some(&(last, ..)) = rows.last() else { return Ok(None) };
    let mut linked = 0;
    for (id, owner, exchange, tx_id, raw) in rows {
        let Ok(raw) = serde_json::from_str::<Value>(&raw) else { continue };
        let Some(Some(external)) = fill_order(tx_id.as_deref(), &raw) else { continue };
        let Some(order) = order(&tx, owner, exchange, &external).map_err(|e| e.to_string())? else { continue };
        linked += tx.execute("UPDATE account_transactions SET transaction_id = ?1, updated_at = ?2 WHERE id = ?3 AND transaction_id IS NULL",
                             params![order, super::sql_time(now), id]).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Some((last, linked)))
}

/// The stored-history pass as a job: woken at every start, so an order recorded after its fill links at the next one.
/// ponytail: rescans every unlinked Binance trade row per start; a high-water mark if one tenant's history makes it slow.
pub struct OrderLinks;
impl crate::jobs::Job for OrderLinks {
    fn spec(&self) -> crate::jobs::Spec {
        use crate::jobs::{schedule::Jitter, Retry, Spec};
        Spec { name: JOB, scope: None, schedule: None, jitter: Jitter::NONE, retry: Retry::Polynomial { attempts: 5 }, deadline: super::jobs::LEDGER_DEADLINE }
    }
    fn run<'a>(&'a self, cx: crate::jobs::Cx<'a>, _: Vec<crate::jobs::Wake>) -> crate::jobs::JobFuture<'a> {
        Box::pin(async move {
            let now = cx.clock.now();
            let (mut cursor, mut linked) = (0, 0);
            // One database unit per batch: a stop or the deadline lands between batches, and other writers get a turn.
            loop {
                match cx.db.run(move |c, cipher| link_batch(c, cipher, cursor, now)).await {
                    Ok(Some((last, n))) => { cursor = last; linked += n; tokio::time::sleep(super::WRITE_GAP).await; }
                    Ok(None) => break,
                    Err(e) => return crate::jobs::Outcome::Transient(e),
                }
            }
            if linked == 0 { return crate::jobs::Outcome::NothingNew }
            crate::engine::log(&format!("[Binance] linked {linked} stored fills to their bot orders"));
            crate::jobs::Outcome::Done
        })
    }
}
