//! Bot::FetchAndUpdateOpenOrdersJob (the strict sweep before a tick), Bot::FetchAndUpdateOrderJob (the
//! lenient poll after a placement), and Transaction#update_with_order_data.
use super::venue_rules::VenueRules;
use super::model::{self, Level};
use super::{amount, tick, EngineError};
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
    id: i64, side: Option<i64>, external_status: Option<i64>,
    price: Option<BigDec>, amount: Option<BigDec>, quote_amount: Option<BigDec>, amount_exec: Option<BigDec>, quote_amount_exec: Option<BigDec>,
    order_type: Option<i64>, base: Option<String>, quote: Option<String>, base_asset_id: Option<i64>, quote_asset_id: Option<i64>,
}

/// An unreadable created_at fails the row (and so the bot's tick); it must never read as the epoch.
fn created_us(s: &str) -> Result<i64, EngineError> {
    parse_time(s).map(|t| t.timestamp_micros()).map_err(|e| EngineError::Data(format!("{e:?}")))
}

fn load(c: &Connection, id: i64) -> Result<Row, EngineError> {
    let row = c.query_row(
        "SELECT id, side, external_status, price, amount, quote_amount, amount_exec, quote_amount_exec, \
         order_type, base, quote, base_asset_id, quote_asset_id FROM transactions WHERE id = ?1", [id],
        |r| {
            let dec = |i: usize| from_sql(r.get_ref(i)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")));
            Ok(Row { id: r.get(0)?, side: r.get(1)?, external_status: r.get(2)?,
                     price: dec(3)?, amount: dec(4)?, quote_amount: dec(5)?, amount_exec: dec(6)?, quote_amount_exec: dec(7)?,
                     order_type: r.get(8)?, base: r.get(9)?, quote: r.get(10)?, base_asset_id: r.get(11)?, quote_asset_id: r.get(12)? })
        })?;
    Ok(row)
}

/// Transaction#update_with_order_data for one row. `_update_missed` is Rails' `update_missed_quote_amount:` keyword, inert
/// since Rails' fill-credit fix (#448): a fill is credited once, by its own row (amount::pending_quote_amount), and a poll
/// never moves the carry. Kept, as Rails keeps the keyword, until Rails removes it. Returns whether this fill spent the
/// amount limit (Transaction's after_commit → Bot::QuoteAmountLimitable#handle_quote_amount_limit_update); the caller stops
/// the bot when Rails' Bot::StopJob would run (apply_committed, placement::recover_since, tick::tick_recovering).
pub fn apply_in(c: &Connection, bot_id: i64, tx_id: i64, s: &OrderState, _update_missed: bool, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let status = match s.status {
        OrderStatus::Open => TxExternalStatus::Open, OrderStatus::Closed => TxExternalStatus::Closed,
        OrderStatus::Cancelled => TxExternalStatus::Cancelled, OrderStatus::Unknown | OrderStatus::Failed => return Ok(false),
    };
    let row = load(c, tx_id)?;
    let bot = model::load_bot(c, bot_id)?;
    // Transaction#update_with_order_data fills blank asset fields from `order_data[:ticker]`: the venue's ticker for THIS
    // order's pair (Exchanges::*#parse_order_data `tickers.find_by(ticker:)`), never the bot's first member.
    let ticker = match &s.pair { Some(pair) => model::ticker_for_pair(c, bot.exchange_id, pair)?, None => None };

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
    let exec_changed = sets.iter().any(|(col, _)| *col == "quote_amount_exec");
    if !sets.is_empty() {
        let assignments: Vec<String> = sets.iter().enumerate().map(|(i, (col, _))| format!("{col} = ?{}", i + 1)).collect();
        let sql = format!("UPDATE transactions SET {}, status = 0, updated_at = ?{} WHERE id = ?{}", assignments.join(", "), sets.len() + 1, sets.len() + 2);
        let mut values: Vec<Sql> = sets.into_iter().map(|(_, v)| v).collect();
        values.push(Sql::Text(format_time(now)));
        values.push(Sql::Integer(row.id));
        c.execute(&sql, rusqlite::params_from_iter(values))?;
    }

    // Transaction's after_commit (transaction.rb:31-33) → Bot::QuoteAmountLimitable#handle_quote_amount_limit_update: a buy
    // whose executed quote changed to a positive value re-checks the cap; reached, Rails enqueues Bot::StopJob, which runs after
    // the job that committed the fill.
    Ok(exec_changed && side == 0 && s.quote_amount_exec.is_positive() && bot.quote_amount_limited() && amount::quote_amount_limit_reached(c, &bot)?)
}

fn classify(e: VenueError, ids: &[String], rules: &VenueRules) -> PollFailure {
    match e {
        VenueError::Rejected(errs) if rules.is_throttle(&errs) => PollFailure::RateLimited(to_sentence(&errs)),
        VenueError::Rejected(errs) if rules.is_transient(&errs) => PollFailure::Transient(to_sentence(&errs)),
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

/// One fill, committed. A fill that spends the amount limit enqueues one Rails Bot::StopJob per qualifying callback
/// (quote_amount_limitable.rb:104). With `stop_now` (a follow-up poll: that job runs right after FetchAndUpdateOrderJob, which
/// has nothing left to do) the stop lands in the fill's own transaction. Without it (the tick's sweep: the job runs after the
/// whole Bot::ActionJob) one pending stop is counted in `transient_data.rust_amount_limit_stops_pending`, by `json_set` in the
/// fill's own transaction, so a crash before the tick ends loses nothing: tick::run_pending_amount_limit_stops runs them at
/// the tick's end, at the next start (run::step), or at the handback.
fn apply_committed(c: &Connection, bot_id: i64, tx_id: i64, s: &OrderState, now: DateTime<Utc>, stop_now: bool) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    if apply_in(&tx, bot_id, tx_id, s, true, now)? {
        if stop_now {
            tick::stop_for_amount_limit(&tx, bot_id, now)?;
        } else {
            tx.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_amount_limit_stops_pending', \
                        coalesce(json_extract(transient_data, '$.rust_amount_limit_stops_pending'), 0) + 1) WHERE id = ?1", [bot_id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Bot::FetchAndUpdateOpenOrdersJob for one bot. Errors carry Rails' messages; orders updated before an
/// error stay updated, as each Rails update! commits on its own.
pub async fn sweep<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, now: DateTime<Utc>) -> Result<(), PollFailure> {
    let db = |e: EngineError| PollFailure::General(format!("{e:?}"));
    poll_rows(c, venue, bot, waiting_ids(c, bot).map_err(db)?, now, true).await
}

/// Both Rails polls go through Exchanges::Kraken's QueryOrders → TradesHistory → StaleOrderResolver path, over
/// the given waiting rows. `strict` (the sweep) fails on `unknown` like FetchAndUpdateOpenOrdersJob; the follow-up skips it.
async fn poll_rows<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, rows: Vec<(i64, String, i64)>, now: DateTime<Utc>, strict: bool) -> Result<(), PollFailure> {
    let db = |e: EngineError| PollFailure::General(format!("{e:?}"));
    if rows.is_empty() { return Ok(()); }
    let ids: Vec<String> = rows.iter().map(|(_, e, _)| e.clone()).collect();
    let mut found: Vec<OrderState> = vec![];
    for batch in ids.chunks(50) {
        found.extend(venue.orders(batch).await.map_err(|e| classify(e, &ids, venue.rules()))?);
    }
    let missing: Vec<String> = ids.iter().filter(|id| !found.iter().any(|o| &&o.txid == id)).cloned().collect();
    if !missing.is_empty() {
        let since = rows.iter().filter(|(_, e, _)| missing.contains(e)).map(|(_, _, at)| *at).min().unwrap() - 3_600_000_000;
        let fills = venue.fills_from_trades(&missing, DateTime::from_timestamp_micros(since).unwrap()).await.map_err(|e| classify(e, &ids, venue.rules()))?;
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
        apply_committed(c, bot.id, *id, state, now, !strict).map_err(db)?;
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
    if venue.rules().follow_up_strict { return follow_up_strict(c, venue, &bot, rows, tx_id, now).await; }
    poll_rows(c, venue, &bot, rows, now, false).await
}

/// Bot::FetchAndUpdateOrderJob through Exchange#get_order, as Rails runs it for Alpaca: one GET; a throttle or transient
/// failure is retried by the caller (retry_on); any other failure raises "Failed to fetch order <transaction id>"; an
/// unknown status (partially_filled included) raises. A raise changes no row, as in Rails.
async fn follow_up_strict<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, rows: Vec<(i64, String, i64)>, tx_id: i64, now: DateTime<Utc>) -> Result<(), PollFailure> {
    let Some((id, ext, _)) = rows.into_iter().next() else { return Ok(()) };
    let rules = venue.rules();
    let state = match venue.orders(std::slice::from_ref(&ext)).await {
        Ok(mut found) if !found.is_empty() => found.remove(0),
        Ok(_) => return Err(PollFailure::General(format!("Failed to fetch order {tx_id}. Result: []"))),
        Err(VenueError::Rejected(errs)) if rules.is_throttle(&errs) => return Err(PollFailure::RateLimited(to_sentence(&errs))),
        Err(VenueError::Rejected(errs)) if rules.is_transient(&errs) => return Err(PollFailure::Transient(to_sentence(&errs))),
        Err(VenueError::Rejected(errs)) => return Err(PollFailure::General(format!("Failed to fetch order {tx_id}. Result: {}", inspect(&errs)))),
        Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => return Err(PollFailure::Transient(m)),
    };
    if state.status == OrderStatus::Unknown { return Err(PollFailure::General(format!("Order {ext} status is unknown."))); }
    apply_committed(c, bot.id, id, &state, now, true).map_err(|e| PollFailure::General(format!("{e:?}")))
}
