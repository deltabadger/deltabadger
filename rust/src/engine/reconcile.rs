//! D-KUCOIN-HF-RETENTION: what a KuCoin HF row still owes, and the one way it settles.
//!
//! - **Obligation** (`rust_kucoin_unresolved:<tx>`): a row whose state cannot be told (unreadable, unknown, lower totals
//!   than already reported, fewer than its known fills, contradicting its settlement). While one stands, its row's bot
//!   places nothing (`barrier`, read where every intent is born). A positive read that applies, or a settlement, ends it.
//! - **Receipt** (`rust_kucoin_evidence:<tx>`): the facts a row settled on (status, exact dealt totals, fees when known,
//!   the operator's evidence). A row settles once: the same facts again are a no-op, other facts are a conflict.
//!
//! Every terminal state applied to a row with an HF record goes through `admit` (polling, recovery, `reconcile-order`).
use super::model;
use super::EngineError;
use crate::ruby::BigDec;
use crate::venue::{OrderState, OrderStatus};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const UNRESOLVED: &str = "rust_kucoin_unresolved:";
pub const RECEIPT: &str = "rust_kucoin_evidence:";

fn data(e: String) -> EngineError { EngineError::Data(e) }

/// The row's open obligation, if any.
pub fn obligation(c: &Connection, tx_id: i64) -> Result<Option<Value>, EngineError> {
    Ok(crate::app_config::get_plain(c, &format!("{UNRESOLVED}{tx_id}")).map_err(data)?.and_then(|s| serde_json::from_str(&s).ok()))
}

pub(crate) fn mark(c: &Connection, tx_id: i64, why: &str, now: DateTime<Utc>) -> Result<(), EngineError> {
    if obligation(c, tx_id)?.is_some_and(|o| o["why"] == why) { return Ok(()); }
    crate::app_config::set_plain(c, &format!("{UNRESOLVED}{tx_id}"), &json!({ "why": why, "at": crate::codec::format_time(now) }).to_string(), now).map_err(data)
}

pub(crate) fn clear(c: &Connection, tx_id: i64) -> Result<(), EngineError> {
    crate::app_config::delete(c, &format!("{UNRESOLVED}{tx_id}")).map_err(data)
}

/// The first row of `bot_id` that holds its spending: an open obligation, or an adopted row whose route is unproven
/// (`ORIGIN`). Owned through the row, so a Merge or Split that moves the row moves it. (transaction id, why)
pub fn barrier(c: &Connection, bot_id: i64) -> Result<Option<(i64, String)>, EngineError> {
    let row = |prefix: &str, filter: &str| -> Result<Option<(i64, String)>, EngineError> {
        // A key range, so the scan touches only these rows (app_configs.key is unique).
        let sql = format!("SELECT t.id, k.value FROM app_configs k JOIN transactions t ON t.id = CAST(substr(k.key, ?2) AS INTEGER) \
                           WHERE k.key >= ?1 AND k.key < ?4 AND t.bot_id = ?3 {filter} ORDER BY t.id LIMIT 1");
        // Every prefix ends in ':'; ';' is the next character, so [prefix, upper) is exactly the prefix's keys.
        let upper = format!("{};", &prefix[..prefix.len() - 1]);
        Ok(c.query_row(&sql, params![prefix, prefix.len() as i64 + 1, bot_id, upper], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).optional()?
            .map(|(id, v)| (id, v.and_then(|v| serde_json::from_str::<Value>(&v).ok()).and_then(|v| v["why"].as_str().map(str::to_string)).unwrap_or_default())))
    };
    if let Some(found) = row(UNRESOLVED, "")? { return Ok(Some(found)); }
    Ok(row(ORIGIN, "AND json_extract(k.value, '$.origin') = 'unproven'")?
        .map(|(id, _)| (id, "it was placed before this engine took over and is found on neither KuCoin route".into())))
}

/// D-KUCOIN-HF-RETENTION (§5): an adopted row's route, `{"origin": "classic" | "unproven"}`. An HF origin is the HF record.
pub const ORIGIN: &str = "rust_kucoin_origin:";

/// The dealt totals settled on, as exact decimals, and the status ("closed" | "cancelled").
#[derive(Clone, Debug, PartialEq)]
pub struct Facts { pub status: &'static str, pub size: BigDec, pub funds: BigDec }

impl Facts {
    pub fn of(s: &OrderState) -> Option<Self> {
        let status = match s.status { OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", _ => return None };
        Some(Self { status, size: s.amount_exec.clone(), funds: s.quote_amount_exec.clone().unwrap_or_else(BigDec::zero) })
    }
    fn json(&self) -> Value { json!({ "status": self.status, "deal_size": self.size.to_s_f(), "deal_funds": self.funds.to_s_f() }) }
}

/// The row's settlement: its receipt, or, for a terminal row settled before receipts existed, its status with its record's
/// exact totals (`implicit`).
pub struct Settled { pub facts: Facts, pub receipt: Option<Value> }

pub fn receipt(c: &Connection, tx_id: i64) -> Result<Option<Value>, EngineError> {
    Ok(crate::app_config::get_plain(c, &format!("{RECEIPT}{tx_id}")).map_err(data)?.and_then(|s| serde_json::from_str(&s).ok()))
}

fn decimal(v: &Value, key: &str, tx_id: i64) -> Result<BigDec, EngineError> {
    v[key].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(|| data(format!("unreadable {key} for transaction {tx_id}")))
}

pub fn settled(c: &Connection, tx_id: i64, record: &Value) -> Result<Option<Settled>, EngineError> {
    if let Some(r) = receipt(c, tx_id)? {
        let status = match r["status"].as_str() { Some("closed") => "closed", Some("cancelled") => "cancelled", _ => return Err(data(format!("unreadable receipt of transaction {tx_id}"))) };
        return Ok(Some(Settled { facts: Facts { status, size: decimal(&r, "deal_size", tx_id)?, funds: decimal(&r, "deal_funds", tx_id)? }, receipt: Some(r) }));
    }
    let external: Option<i64> = c.query_row("SELECT external_status FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0))?;
    let status = match external { Some(2) => "closed", Some(3) => "cancelled", _ => return Ok(None) };
    Ok(Some(Settled { facts: Facts { status, size: decimal(record, "deal_size", tx_id)?, funds: decimal(record, "deal_funds", tx_id)? }, receipt: None }))
}

pub(crate) fn write_receipt(c: &Connection, tx_id: i64, facts: &Facts, source: &str, extra: Value, now: DateTime<Utc>) -> Result<(), EngineError> {
    let mut r = facts.json();
    r["v"] = json!(1);
    r["source"] = json!(source);
    r["at"] = json!(crate::codec::format_time(now));
    r["fees"] = Value::Null;
    if let Value::Object(extra) = extra { for (k, v) in extra { r[k] = v; } }
    crate::app_config::set_plain(c, &format!("{RECEIPT}{tx_id}"), &r.to_string(), now).map_err(data)
}

/// What the ledger already holds of order `order_id`: its API fills (visible, and withheld where HF-3's table exists), the
/// owner's and the row's exchange's only, each id once. Σ size, Σ funds, Σ |fee| per currency.
#[derive(Debug, PartialEq)]
pub struct Known { pub size: BigDec, pub funds: BigDec, pub fees: std::collections::BTreeMap<String, BigDec> }

pub fn known_fills(c: &Connection, user_id: i64, exchange_id: i64, order_id: &str) -> Result<Known, EngineError> {
    let mut rows: Vec<(String, String)> = vec![];
    let mut s = c.prepare_cached("SELECT tx_id, raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id IS NOT NULL \
                                  AND json_valid(raw_data) AND json_extract(raw_data, '$.orderId') = ?3 AND json_extract(raw_data, '$.createdAt') IS NOT NULL")?;
    rows.extend(s.query_map(params![user_id, exchange_id, order_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?);
    let withheld: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kucoin_withheld_fills')", [], |r| r.get(0))?;
    if withheld {
        let mut s = c.prepare_cached("SELECT tx_id, entry FROM kucoin_withheld_fills WHERE user_id = ?1 AND exchange_id = ?2 \
                                      AND json_valid(entry) AND json_extract(entry, '$.orderId') = ?3")?;
        rows.extend(s.query_map(params![user_id, exchange_id, order_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut known = Known { size: BigDec::zero(), funds: BigDec::zero(), fees: Default::default() };
    for (id, raw) in rows {
        if !seen.insert(id.clone()) { continue; }
        let raw: Value = serde_json::from_str(&raw).map_err(|e| data(format!("fill {id}: {e}")))?;
        if raw["family"] != "ClassicHf" { continue; }
        let n = |k: &str| raw[k].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(|| data(format!("fill {id}: unreadable {k}")));
        known.size = &known.size + &n("size")?;
        known.funds = &known.funds + &n("funds")?;
        // The ledger's fee: its absolute value (kucoin_ledger::fill).
        let fee = n("fee")?;
        let fee = if fee < BigDec::zero() { &BigDec::zero() - &fee } else { fee };
        if let Some(currency) = raw["feeCurrency"].as_str() {
            let sum = known.fees.entry(currency.to_string()).or_insert_with(BigDec::zero);
            *sum = &*sum + &fee;
        }
    }
    Ok(known)
}

/// The owner and exchange `known_fills` reads for row `tx_id`: the row's bot's user, the row's own exchange.
pub(crate) fn owner(c: &Connection, tx_id: i64) -> Result<(i64, i64), EngineError> {
    Ok(c.query_row("SELECT b.user_id, t.exchange_id FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.id = ?1", [tx_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// What `admit` decided for one HF answer, in the caller's write transaction.
pub(crate) enum Admit {
    /// Apply it; `terminal`: it settles the row (the caller writes the receipt after its writes).
    Proceed { terminal: bool },
    /// Nothing more to write (a replay, or an obligation now stands).
    Done,
}

/// The gate before an HF row takes an answer (`polling::apply_order_in`). A settled row takes only its own facts again
/// (a no-op that ends an obligation; for a row settled before receipts, its receipt and what it still owed). An unsettled
/// row takes no unknown state, no lower totals than reported, nor fewer than its known fills: each is an obligation.
/// A terminal answer ends the row's reversal cancel here, before its settlement credits the reserve.
pub(crate) fn admit(c: &Connection, bot_id: i64, tx_id: i64, external_id: &str, record: &Value, s: &OrderState, now: DateTime<Utc>) -> Result<Admit, EngineError> {
    let facts = Facts::of(s);
    if let Some(settled) = settled(c, tx_id, record)? {
        if facts.as_ref() == Some(&settled.facts) {
            if settled.receipt.is_none() {
                discharge(c, bot_id, tx_id)?;
                write_receipt(c, tx_id, &settled.facts, "api", Value::Null, now)?;
            }
            clear(c, tx_id)?;
        } else {
            mark(c, tx_id, "KuCoin's answer contradicts the order's settlement", now)?;
        }
        return Ok(Admit::Done);
    }
    if matches!(s.status, OrderStatus::Unknown | OrderStatus::Failed) {
        mark(c, tx_id, "its state cannot be told yet", now)?;
        return Ok(Admit::Done);
    }
    let funds = s.quote_amount_exec.clone().unwrap_or_else(BigDec::zero);
    if s.amount_exec < decimal(record, "deal_size", tx_id)? || funds < decimal(record, "deal_funds", tx_id)? {
        mark(c, tx_id, "KuCoin answered lower dealt totals than it already reported", now)?;
        return Ok(Admit::Done);
    }
    let (user, exchange) = owner(c, tx_id)?;
    let known = known_fills(c, user, exchange, external_id)?;
    if s.amount_exec < known.size || funds < known.funds {
        mark(c, tx_id, "KuCoin answered less than the fills already recorded for it", now)?;
        return Ok(Admit::Done);
    }
    let terminal = facts.is_some();
    if terminal { super::reversal::drop_cancel(c, bot_id, tx_id)?; }
    Ok(Admit::Proceed { terminal })
}

/// A row settled before receipts: what it still owes once its settlement is confirmed (the reversal's cancel entry, a
/// pending reserve).
fn discharge(c: &Connection, bot_id: i64, tx_id: i64) -> Result<(), EngineError> {
    super::reversal::drop_cancel(c, bot_id, tx_id)?;
    super::reversal::settle(c, bot_id, tx_id).map(|_| ())
}

/// An HF row a read could not tell (excluded, or a failed single read), in its own fenced write: an obligation, unless the
/// row is already settled (then a missing remote answer owes nothing).
pub(crate) fn unread(c: &Connection, bot_id: i64, tx_id: i64, why: &str, version: &Option<model::CredentialVersion>, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::credential_write(c, version, |tx| {
        super::polling::require_owner(tx, bot_id, tx_id)?;
        if let Some(record) = crate::venue::kucoin::order_record(tx, tx_id).map_err(data)? {
            if record["family"] == "ClassicHf" && settled(tx, tx_id, &record)?.is_some() { return Ok(()); }
        }
        mark(tx, tx_id, why, now)
    })
}
