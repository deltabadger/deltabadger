//! Bot::FetchAndUpdateOpenOrdersJob (the strict sweep before a tick), Bot::FetchAndUpdateOrderJob (the
//! lenient poll after a placement), and Transaction#update_with_order_data.
use super::venue_rules::KRAKEN;
use super::model::{self, Level};
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::enums::TxExternalStatus;
use crate::ruby::{from_sql, inspect, to_sentence, to_sql, BigDec};
use crate::venue::{OrderState, OrderStatus, Venue, VenueError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Value as Sql;
use rusqlite::{params, Connection};
use serde_json::json;

pub const STALE_AFTER_DAYS: i64 = 14;

#[derive(Debug, PartialEq)]
pub enum PollFailure { RateLimited(String), Transient(String), General(String) }

struct Row {
    id: i64, created_at_us: i64, side: Option<i64>, transaction_type: String, external_status: Option<i64>,
    price: Option<BigDec>, amount: Option<BigDec>, quote_amount: Option<BigDec>, amount_exec: Option<BigDec>, quote_amount_exec: Option<BigDec>,
    order_type: Option<i64>, base: Option<String>, quote: Option<String>, base_asset_id: Option<i64>, quote_asset_id: Option<i64>,
}

/// An unreadable created_at fails the row (and so the bot's tick); it must never read as the epoch.
fn created_us(s: &str) -> Result<i64, EngineError> {
    parse_time(s).map(|t| t.timestamp_micros()).map_err(|e| EngineError::Data(format!("{e:?}")))
}

fn load(c: &Connection, id: i64) -> Result<Row, EngineError> {
    let row = c.query_row(
        "SELECT id, created_at, side, transaction_type, external_status, price, amount, quote_amount, amount_exec, quote_amount_exec, \
         order_type, base, quote, base_asset_id, quote_asset_id FROM transactions WHERE id = ?1", [id],
        |r| {
            let dec = |i: usize| from_sql(r.get_ref(i)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")));
            Ok(Row { id: r.get(0)?,
                     created_at_us: created_us(&r.get::<_, String>(1)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")))?,
                     side: r.get(2)?, transaction_type: r.get(3)?, external_status: r.get(4)?,
                     price: dec(5)?, amount: dec(6)?, quote_amount: dec(7)?, amount_exec: dec(8)?, quote_amount_exec: dec(9)?,
                     order_type: r.get(10)?, base: r.get(11)?, quote: r.get(12)?, base_asset_id: r.get(13)?, quote_asset_id: r.get(14)? })
        })?;
    Ok(row)
}

pub fn apply_in(c: &Connection, bot_id: i64, tx_id: i64, s: &OrderState, update_missed: bool, now: DateTime<Utc>) -> Result<(), EngineError> {
    let status = match s.status {
        OrderStatus::Open => TxExternalStatus::Open, OrderStatus::Closed => TxExternalStatus::Closed,
        OrderStatus::Cancelled => TxExternalStatus::Cancelled, OrderStatus::Unknown => return Ok(()),
    };
    let row = load(c, tx_id)?;
    let bot = model::load_bot(c, bot_id)?;
    let ticker = model::ticker_for(c, &bot)?;
    let previous_quote_amount_exec = row.quote_amount_exec.clone().unwrap_or_else(BigDec::zero);

    // update_with_order_data(...).compact under ActiveRecord's dirty check: only changed attributes are written.
    let mut sets: Vec<(&str, Sql)> = vec![];
    let mut decimal = |col: &'static str, old: &Option<BigDec>, new: Option<&BigDec>| {
        if let Some(new) = new { if old.as_ref() != Some(&new.round(18)) { sets.push((col, Sql::Real(to_sql(new)))); } }
    };
    decimal("price", &row.price, s.price.as_ref());
    decimal("amount", &row.amount, s.amount.as_ref());
    decimal("quote_amount", &row.quote_amount, s.quote_amount.as_ref());
    decimal("amount_exec", &row.amount_exec, Some(&s.amount_exec));
    decimal("quote_amount_exec", &row.quote_amount_exec, Some(&s.quote_amount_exec));
    if row.external_status != Some(status as i64) { sets.push(("external_status", Sql::Integer(status as i64))); }
    let side = if s.sell { 1 } else { 0 }; // parse_order_data: descr.type, as the exchange reports it
    if row.side != Some(side) { sets.push(("side", Sql::Integer(side))); }
    let order_type = if s.limit { 1 } else { 0 };
    if row.order_type != Some(order_type) { sets.push(("order_type", Sql::Integer(order_type))); }
    if let Some(t) = &ticker {
        // base/quote are snapshots: only filled when blank (update_with_order_data's `presence ||`).
        if row.base.as_deref().unwrap_or("").is_empty() { sets.push(("base", Sql::Text(t.base_symbol.clone()))); }
        if row.quote.as_deref().unwrap_or("").is_empty() { sets.push(("quote", Sql::Text(t.quote_symbol.clone()))); }
        if row.base_asset_id.is_none() { sets.push(("base_asset_id", Sql::Integer(t.base_asset_id))); }
        if row.quote_asset_id.is_none() { sets.push(("quote_asset_id", Sql::Integer(t.quote_asset_id))); }
    }
    if !sets.is_empty() {
        let assignments: Vec<String> = sets.iter().enumerate().map(|(i, (col, _))| format!("{col} = ?{}", i + 1)).collect();
        let sql = format!("UPDATE transactions SET {}, status = 0, updated_at = ?{} WHERE id = ?{}", assignments.join(", "), sets.len() + 1, sets.len() + 2);
        let mut values: Vec<Sql> = sets.into_iter().map(|(_, v)| v).collect();
        values.push(Sql::Text(format_time(now)));
        values.push(Sql::Integer(row.id));
        c.execute(&sql, rusqlite::params_from_iter(values))?;
    }

    let in_window = bot.calc_since_us().is_some_and(|since| row.created_at_us >= since);
    if update_missed && row.transaction_type == "REGULAR" && side == 0 && in_window { // `order.buy?` after the update
        let missed = &bot.missed_quote_amount()? - &(&s.quote_amount_exec - &previous_quote_amount_exec);
        // [0, x].max: Ruby returns the Integer 0 unless x is strictly greater.
        let value = if missed.is_positive() { json!(missed.to_s_f()) } else { json!(0) };
        // Rails assigns a BigDecimal against the stored String, so it always writes; only Integer 0 over Integer 0 is a no-op.
        if !(value == json!(0) && bot.transient.get("missed_quote_amount") == Some(&json!(0))) {
            model::update_transient(c, bot_id, &[("missed_quote_amount", value)], now)?;
        }
    }
    Ok(())
}

fn classify(e: VenueError, ids: &[String]) -> PollFailure {
    match e {
        VenueError::Rejected(errs) if KRAKEN.is_throttle(&errs) => PollFailure::RateLimited(to_sentence(&errs)),
        VenueError::Rejected(errs) if KRAKEN.is_transient(&errs) => PollFailure::Transient(to_sentence(&errs)),
        VenueError::Rejected(errs) => PollFailure::General(format!("Failed to fetch orders {}. Result: {}", to_sentence(ids), inspect(&errs))),
        VenueError::Transient(m) | VenueError::Ambiguous(m) => PollFailure::Transient(m),
    }
}

fn waiting_ids(c: &Connection, bot: &model::Bot) -> Result<Vec<(i64, String, i64)>, EngineError> {
    let mut s = c.prepare(
        "SELECT id, external_id, created_at FROM transactions WHERE bot_id = ?1 AND exchange_id = ?2 AND status = 0 AND external_status IN (0, 1) \
         AND (external_id IS NULL OR external_id NOT LIKE 'imported_%') ORDER BY id")?;
    let rows = s.query_map(params![bot.id, bot.exchange_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, String>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|(id, ext, at)| Ok((id, ext, created_us(&at)?))).collect()
}

fn apply_committed(c: &Connection, bot_id: i64, tx_id: i64, s: &OrderState, now: DateTime<Utc>) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    apply_in(&tx, bot_id, tx_id, s, true, now)?;
    tx.commit()?;
    Ok(())
}

/// Bot::FetchAndUpdateOpenOrdersJob for one bot. Errors carry Rails' messages; orders updated before an
/// error stay updated, as each Rails update! commits on its own.
pub async fn sweep<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, now: DateTime<Utc>) -> Result<(), PollFailure> {
    poll(c, venue, bot, now, true).await
}

/// Both Rails polls go through Exchanges::Kraken's QueryOrders → TradesHistory → StaleOrderResolver path, over
/// the given waiting rows. `strict` (the sweep) fails on `unknown` like FetchAndUpdateOpenOrdersJob; the follow-up skips it.
async fn poll<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, now: DateTime<Utc>, strict: bool) -> Result<(), PollFailure> {
    let db = |e: EngineError| PollFailure::General(format!("{e:?}"));
    poll_rows(c, venue, bot, waiting_ids(c, bot).map_err(db)?, now, strict).await
}

async fn poll_rows<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, rows: Vec<(i64, String, i64)>, now: DateTime<Utc>, strict: bool) -> Result<(), PollFailure> {
    let db = |e: EngineError| PollFailure::General(format!("{e:?}"));
    if rows.is_empty() { return Ok(()); }
    let ids: Vec<String> = rows.iter().map(|(_, e, _)| e.clone()).collect();
    let mut found: Vec<OrderState> = vec![];
    for batch in ids.chunks(50) {
        found.extend(venue.orders(batch).await.map_err(|e| classify(e, &ids))?);
    }
    let missing: Vec<String> = ids.iter().filter(|id| !found.iter().any(|o| &&o.txid == id)).cloned().collect();
    if !missing.is_empty() {
        let since = rows.iter().filter(|(_, e, _)| missing.contains(e)).map(|(_, _, at)| *at).min().unwrap() - 3_600_000_000;
        let fills = venue.fills_from_trades(&missing, DateTime::from_timestamp_micros(since).unwrap()).await.map_err(|e| classify(e, &ids))?;
        found.extend(fills.into_iter().map(|f| OrderState { status: OrderStatus::Closed, ..f }));
    }
    // Missing after both endpoints: Bot::StaleOrderResolver (Kraken is authoritative, so young ones just wait).
    for (id, ext, created) in &rows {
        if found.iter().any(|o| &o.txid == ext) { continue; }
        if *created < (now - Duration::days(STALE_AFTER_DAYS)).timestamp_micros() {
            c.execute("UPDATE transactions SET external_status = ?1, updated_at = ?2 WHERE id = ?3",
                      params![TxExternalStatus::Abandoned as i64, format_time(now), id]).map_err(|e| db(e.into()))?;
            model::log_activity(c, bot.id, "order_abandoned", Level::Info, json!({ "order_id": ext }), now).map_err(db)?;
        }
    }
    for state in &found {
        let Some((id, _, _)) = rows.iter().find(|(_, e, _)| e == &state.txid) else { continue };
        if state.status == OrderStatus::Unknown {
            if strict { return Err(PollFailure::General(format!("Order {} status is unknown.", state.txid))); }
            continue;
        }
        apply_committed(c, bot.id, *id, state, now).map_err(db)?;
    }
    Ok(())
}

/// Bot::FetchAndUpdateOrderJob for ONE order (enqueued per order at placement, and at engine start for every
/// outstanding order), for a bot in ANY status: the same QueryOrders → TradesHistory → stale path, lenient on
/// `unknown`. One order's failure never holds back another's fill. The caller retries Transient/RateLimited
/// failures as that job's retry_on does (3 and 4 attempts); others end the chain. A row no longer waiting is done.
pub async fn follow_up<V: Venue>(c: &Connection, venue: &V, bot_id: i64, tx_id: i64, now: DateTime<Utc>) -> Result<(), PollFailure> {
    let db = |e: EngineError| PollFailure::General(format!("{e:?}"));
    let bot = model::load_bot(c, bot_id).map_err(db)?;
    let rows: Vec<_> = waiting_ids(c, &bot).map_err(db)?.into_iter().filter(|(id, _, _)| *id == tx_id).collect();
    poll_rows(c, venue, &bot, rows, now, false).await
}
