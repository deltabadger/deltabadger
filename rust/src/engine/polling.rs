//! Bot::FetchAndUpdateOpenOrdersJob (the strict sweep before a tick), Bot::FetchAndUpdateOrderJob (the
//! lenient poll after a placement), and Transaction#update_with_order_data.
use crate::venue::Attributed;
use super::venue_rules::VenueRules;
use super::model::{self, Level};
use super::amount_limit::{self, Cap};
use super::{amount, tick, EngineError};
use crate::codec::{format_time, parse_time};
use crate::enums::TxExternalStatus;
use crate::ruby::{inspect, to_sentence, to_sql, BigDec};
use crate::venue::{OrderState, OrderStatus, Venue, VenueError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Value as Sql;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

pub const STALE_AFTER_DAYS: i64 = 14;

#[derive(Clone, Copy, PartialEq)]
enum PollMode { Sweep, FollowUp, Refresh, Terminal }

#[derive(Debug, PartialEq)]
pub enum PollFailure { CredentialsChanged, RateLimited(String), Transient(String), General(String), OwnerChanged(i64),
    /// D-KUCOIN-HF-RETENTION: (transaction, why) the bot's first unresolved HF obligation (reconcile::barrier).
    AwaitingReconciliation(i64, String) }

fn poll_error(e: EngineError) -> PollFailure {
    match e { EngineError::CredentialsChanged => PollFailure::CredentialsChanged, EngineError::OwnerChanged(owner) => PollFailure::OwnerChanged(owner), other => PollFailure::General(format!("{other:?}")) }
}

/// A follow-up poll that did not fail: the order is settled, or the venue still holds it open (Rails' retry_on
/// OrderStillOpen: asked again, whatever bot placed it).
///
/// `SettlesAt` (D-KUCOIN-ZERO-FILL-FUNDS): the venue answered a zero fill too young to be final (OrderState::settles_at); asked
/// again, and once more just past that time if the retries end before it.
///
/// `OwnerChanged` (PLAN-dashboard-bot-actions slice F): a Merge or Split moved the row to the bot given while this poll ran.
/// Nothing was written; the poll obligation stays, under that bot.
#[derive(Debug, PartialEq)]
pub enum FollowUp { Done, StillOpen, SettlesAt(DateTime<Utc>), OwnerChanged(i64) }

/// An unreadable created_at fails the row (and so the bot's tick); it must never read as the epoch.
fn created_us(s: &str) -> Result<i64, EngineError> {
    parse_time(s).map(|t| t.timestamp_micros()).map_err(|e| EngineError::Data(format!("{e:?}")))
}

/// The bot that owns the row now, when it is not `bot_id`: a Merge or Split moved it (PLAN-dashboard-bot-actions slice F).
fn moved_to(c: &Connection, bot_id: i64, tx_id: i64) -> Result<Option<i64>, EngineError> {
    let owner: Option<Option<i64>> = c.query_row("SELECT bot_id FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0)).optional()?;
    Ok(owner.flatten().filter(|owner| *owner != bot_id))
}

/// The fence of every poll completion: inside its transaction, before any write or callback, the row must still be the polled
/// bot's. A row moved meanwhile is never written for its old owner.
pub(crate) fn require_owner(c: &Connection, bot_id: i64, tx_id: i64) -> Result<(), EngineError> {
    match moved_to(c, bot_id, tx_id)? { Some(owner) => Err(EngineError::OwnerChanged(owner)), None => Ok(()) }
}

/// Transaction#update_with_order_data for one row. `_update_missed` is Rails' `update_missed_quote_amount:` keyword, inert
/// since Rails' fill-credit fix (#448): a fill is credited once, by its own row (amount::pending_quote_amount), and a poll
/// never moves the carry. Kept, as Rails keeps the keyword, until Rails removes it. Returns whether this fill spent the
/// amount limit (Transaction's after_commit → Bot::QuoteAmountLimitable#handle_quote_amount_limit_update); the caller stops
/// the bot when Rails' Bot::StopJob would run (apply_committed, placement::recover_since, tick::tick_recovering).
pub fn apply_in(c: &model::FencedTransaction<'_>, bot_id: i64, tx_id: i64, s: &OrderState, _update_missed: bool, now: DateTime<Utc>) -> Result<Option<Cap>, EngineError> {
    apply_order_in(c, bot_id, tx_id, s, now, false, None)
}

/// `apply_in` of a settlement an operator proved (`reconcile-order`): its receipt carries `source` and `extra`.
pub(crate) fn apply_evidenced_in(c: &model::FencedTransaction<'_>, bot_id: i64, tx_id: i64, s: &OrderState, source: &str, extra: serde_json::Value, now: DateTime<Utc>) -> Result<Option<Cap>, EngineError> {
    apply_order_in(c, bot_id, tx_id, s, now, false, Some((source, extra)))
}

/// D-KUCOIN-HF-RETENTION: a row with an HF record takes an answer only through `reconcile::admit`; one that applies ends
/// its obligation, and a terminal one leaves its receipt, in the same transaction.
fn apply_order_in(c: &model::FencedTransaction<'_>, bot_id: i64, tx_id: i64, s: &OrderState, now: DateTime<Utc>, execution_only: bool, evidence: Option<(&str, serde_json::Value)>) -> Result<Option<Cap>, EngineError> {
    require_owner(c, bot_id, tx_id)?;
    let record = crate::venue::kucoin::order_record(c, tx_id).map_err(EngineError::Data)?.filter(|r| r["family"] == "ClassicHf");
    let settles = match &record {
        Some(record) => match super::reconcile::admit(c, bot_id, tx_id, &s.txid, record, s, now)? {
            super::reconcile::Admit::Done => return Ok(None),
            super::reconcile::Admit::Proceed { terminal } => Some(terminal),
        },
        None => None,
    };
    if !crate::venue::kucoin::advance_totals(c, tx_id, &s.amount_exec, s.quote_amount_exec.as_ref(), now).map_err(EngineError::Data)? {
        super::log(&format!("[engine] bot {bot_id}: KuCoin answered transaction {tx_id} with lower dealt totals than it already reported; the row waits"));
        return Ok(None);
    }
    let cap = apply_body(c, bot_id, tx_id, s, now, execution_only)?;
    if let Some(terminal) = settles {
        super::reconcile::clear(c, tx_id)?;
        if let Some(facts) = super::reconcile::Facts::of(s).filter(|_| terminal) {
            let (source, extra) = evidence.unwrap_or(("api", serde_json::Value::Null));
            super::reconcile::write_receipt(c, tx_id, &facts, source, extra, now)?;
        }
    }
    Ok(cap)
}

fn apply_body(c: &model::FencedTransaction<'_>, bot_id: i64, tx_id: i64, s: &OrderState, now: DateTime<Utc>, execution_only: bool) -> Result<Option<Cap>, EngineError> {
    let row = crate::figures::fill::stored_order(c, tx_id)?;
    let bot = model::load_bot(c, bot_id)?;
    // Transaction#update_with_order_data fills blank asset fields from `order_data[:ticker]`: the venue's ticker for THIS
    // order's pair (Exchanges::*#parse_order_data `tickers.find_by(ticker:)`), never the bot's first member.
    let ticker = if execution_only || (row.base_asset_id.is_some() && row.quote_asset_id.is_some()) {
        None
    } else if model::exchange_type(c, &bot)? == "Exchanges::Alpaca" {
        let ticker = model::alpaca_order_ticker(c, bot.exchange_id, s.pair.as_deref(), s.asset_class.as_deref())?;
        if ticker.is_none() {
            eprintln!("Alpaca order skipped: unsupported class or unmapped/ambiguous order identity");
            return Ok(None);
        }
        ticker
    } else {
        match &s.pair { Some(pair) => model::ticker_for_pair(c, bot.exchange_id, pair)?, None => None }
    };

    let status = if execution_only {
        // Re-read under the write lock: only positive execution data enriches a terminal row.
        // Missing, unknown or open answers never reopen it or erase its recorded execution.
        if !matches!(s.status, OrderStatus::Closed | OrderStatus::Cancelled) { return Ok(None); }
        match row.external_status {
            Some(2) => TxExternalStatus::Closed, Some(3) => TxExternalStatus::Cancelled, _ => return Ok(None),
        }
    } else {
        match s.status {
            OrderStatus::Open => TxExternalStatus::Open, OrderStatus::Closed => TxExternalStatus::Closed,
            OrderStatus::Cancelled => TxExternalStatus::Cancelled, OrderStatus::Unknown | OrderStatus::Failed => return Ok(None),
        }
    };

    // update_with_order_data(...).compact under ActiveRecord's dirty check: only changed attributes are written.
    let mut sets: Vec<(&str, Sql)> = vec![];
    let mut decimal = |col: &'static str, old: &Option<BigDec>, new: Option<&BigDec>| {
        if let Some(new) = new { if old.as_ref() != Some(&new.round(18)) { sets.push((col, Sql::Real(to_sql(new)))); } }
    };
    if !execution_only {
        decimal("price", &row.price, s.price.as_ref());
        decimal("amount", &row.amount, s.amount.as_ref());
        decimal("quote_amount", &row.quote_amount, s.quote_amount.as_ref());
    }
    if !execution_only || s.amount_exec.is_positive() { decimal("amount_exec", &row.amount_exec, Some(&s.amount_exec)); }
    decimal("quote_amount_exec", &row.quote_amount_exec, s.quote_amount_exec.as_ref().filter(|n| !execution_only || n.is_positive()));
    if !execution_only && row.external_status != Some(status as i64) { sets.push(("external_status", Sql::Integer(status as i64))); }
    let side = if execution_only { row.side.unwrap_or(0) } else { i64::from(s.sell) }; // parse_order_data: descr.type, as the exchange reports it
    if !execution_only && row.side != Some(side) { sets.push(("side", Sql::Integer(side))); }
    let order_type = if s.limit { 1 } else { 0 };
    if !execution_only && row.order_type != Some(order_type) { sets.push(("order_type", Sql::Integer(order_type))); }
    if let Some(t) = &ticker {
        // base/quote are snapshots: only filled when blank (update_with_order_data's `presence ||`).
        if row.base.as_deref().unwrap_or("").is_empty() { sets.push(("base", Sql::Text(t.base_symbol.clone()))); }
        if row.quote.as_deref().unwrap_or("").is_empty() { sets.push(("quote", Sql::Text(t.quote_symbol.clone()))); }
        if row.base_asset_id.is_none() { sets.push(("base_asset_id", Sql::Integer(t.base_asset_id))); }
        if row.quote_asset_id.is_none() { sets.push(("quote_asset_id", Sql::Integer(t.quote_asset_id))); }
    }
    let exec_changed = sets.iter().any(|(col, _)| *col == "quote_amount_exec");
    let changed = |cols: &[&str]| sets.iter().any(|(col, _)| cols.contains(col));
    let executed = s.amount_exec.is_positive() || (execution_only && row.amount_exec.as_ref().is_some_and(BigDec::is_positive));
    let terminal = matches!(status, TxExternalStatus::Closed | TxExternalStatus::Cancelled);
    // Transaction#base_cap_relevant_change? (the base cap's trigger, below).
    let base_changed = (changed(&["amount_exec"]) && s.amount_exec.is_positive()) || (changed(&["external_status"]) && status == TxExternalStatus::Closed);
    // Transaction's after_save :reconcile_wash_sale (transaction.rb:67-72): a sell closed or cancelled whose status, executed
    // figures or asset this write changed. Bot::WashSaleGuard is a basket's and an index bot's only.
    let wash = side == 1 && terminal && changed(&["external_status", "amount_exec", "quote_amount_exec", "base_asset_id"])
        && matches!(bot.bot_type.as_str(), "Bots::DcaMultiAsset" | "Bots::DcaIndex");
    // Its after_commit (transaction.rb:50-56): a closed sell, or a cancelled one that filled, whose status or executed
    // figures changed, wakes the account's ledger sync (EngineEvent::SaleSettled), for every bot type.
    let sold = side == 1 && (status == TxExternalStatus::Closed || (status == TxExternalStatus::Cancelled && executed))
        && changed(&["external_status", "amount_exec", "quote_amount_exec"]);
    // D-KUCOIN-ZERO-FILL-FUNDS: a buy the venue ended with nothing dealt is its out-of-funds answer. The settlement that moves
    // the row from a non-terminal status (once per row: a failed row is never polled again) fails it with the venue's own
    // insufficient-funds error, and the bot with it as a placement rejected with that error fails it. A sell stays Closed.
    let refused = if s.closed_empty() && !s.sell && matches!(row.external_status, None | Some(0) | Some(1)) {
        let rules = super::venue_rules::for_exchange(&model::exchange_type(c, &bot)?).ok_or_else(|| EngineError::Data("no venue rules for the bot's exchange".into()))?;
        let errors = vec![crate::venue::ZERO_FILL_REFUSAL.to_string()];
        sets.push(("error_messages", Sql::Text(json!(errors).to_string())));
        Some((rules, errors))
    } else { None };
    if !sets.is_empty() {
        let assignments: Vec<String> = sets.iter().enumerate().map(|(i, (col, _))| format!("{col} = ?{}", i + 1)).collect();
        let row_status = if refused.is_some() { crate::enums::TxStatus::Failed as i64 } else { 0 };
        let sql = format!("UPDATE transactions SET {}, status = {row_status}, updated_at = ?{} WHERE id = ?{}", assignments.join(", "), sets.len() + 1, sets.len() + 2);
        let mut values: Vec<Sql> = sets.into_iter().map(|(_, v)| v).collect();
        values.push(Sql::Text(format_time(now)));
        values.push(Sql::Integer(row.id));
        c.execute(&sql, rusqlite::params_from_iter(values))?;
    }

    if wash { super::wash::reconcile_fill(c, &bot, tx_id, now)?; }
    if sold { note_sale(c, bot_id)?; }
    // D-IBKR-SELLCAP: a closed or cancelled order's sell reservation is released with the write that settles it.
    if terminal { crate::venue::ibkr_reservations::release_settled(c, now)?; }
    // Transaction's after_save → Bot::Reversible#settle_flip_reserve!: a buy a reversal cancelled returns its reserve once final.
    super::reversal::settle(c, bot_id, tx_id)?;
    if let Some((rules, errors)) = refused { tick::order_refused(c, bot_id, errors, rules, now)?; }
    // Transaction's after_commit (transaction.rb:31-33) → Bot::QuoteAmountLimitable#handle_quote_amount_limit_update: a buy
    // whose executed quote changed to a positive value re-checks the cap; reached, Rails enqueues Bot::StopJob, which runs after
    // the job that committed the fill.
    // `return if selling?` (quote_amount_limitable.rb:106): a buy that still fills after a flip never stops the seller.
    // D-SIG-CAP: a spent cap never stops a signal bot (its sells keep working; buy calls get the cap line).
    if exec_changed && side == 0 && !bot.selling() && bot.bot_type != model::SIGNAL_TYPE && s.quote_amount_exec.as_ref().is_some_and(BigDec::is_positive) && bot.quote_amount_limited() && amount::quote_amount_limit_reached(c, &bot)? {
        return Ok(Some(Cap::Quote));
    }
    // Its sell-side mirror (transaction.rb:34-36 → Bot::BaseAmountLimitable#handle_base_amount_limit_update): a sell of any
    // kind whose executed base changed to a positive value, or that was newly closed (base_cap_relevant_change?), re-checks
    // the base cap while the bot sells its one asset; reached, Bot::StopJob(amount_sold) and the base cap's mail.
    let base_hit = side == 1 && base_changed && bot.base_amount_limited() && super::accounting::base_amount_limit_reached(c, &bot)?;
    // A rotation's deferred D-6D-2 decision (amount_limit::ROTATION_PENDING) waits for this order's final state.
    amount_limit::settle_rotation(c, bot_id, tx_id, now, base_hit)?;
    Ok(base_hit.then_some(Cap::Base))
}

/// A terminal sell fill, noted in the fill's own transaction on this connection: a connection-local temp table, so the note
/// rolls back with the fill and nothing of it reaches the database file. The engine announces each bot noted once its writes
/// committed (`settled_sales`, run::step), as Rails' after_commit enqueues the sync.
fn note_sale(c: &Connection, bot_id: i64) -> Result<(), EngineError> {
    c.execute_batch(SALES_TABLE)?;
    c.execute("INSERT INTO temp.rust_sales_settled (bot_id) VALUES (?1)", [bot_id])?;
    Ok(())
}
const SALES_TABLE: &str = "CREATE TEMP TABLE IF NOT EXISTS rust_sales_settled (bot_id INTEGER NOT NULL)";

/// The bots whose sales `note_sale` noted since the last call, each once, and the notes cleared.
pub fn settled_sales(c: &Connection) -> Result<Vec<i64>, EngineError> {
    c.execute_batch(SALES_TABLE)?;
    let mut s = c.prepare("SELECT DISTINCT bot_id FROM temp.rust_sales_settled ORDER BY bot_id")?;
    let bots = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    c.execute("DELETE FROM temp.rust_sales_settled", [])?;
    Ok(bots)
}

fn classify(e: VenueError, ids: &[String], rules: &VenueRules) -> PollFailure {
    match e {
        VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. } if rules.is_throttle(&errs) => PollFailure::RateLimited(to_sentence(&errs)),
        VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. } if rules.is_transient(&errs) => PollFailure::Transient(to_sentence(&errs)),
        VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. } => PollFailure::General(format!("Failed to fetch orders {}. Result: {}", to_sentence(ids), inspect(&errs))),
        VenueError::Transient(m) | VenueError::Ambiguous(m) => PollFailure::Transient(m),
    }
}

pub(crate) fn waiting_ids(c: &Connection, bot: &model::Bot) -> Result<Vec<(i64, String, i64)>, EngineError> {
    // A row whose status Rails never writes may be a working order: unresolved, never skipped.
    let unknown: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND (status IS NULL OR status NOT IN (0, 1, 2))", [bot.id], |r| r.get(0))?;
    if unknown > 0 { return Err(EngineError::Data(format!("bot {}: {unknown} order row(s) with a status Rails never writes are unresolved", bot.id))); }
    let mut s = c.prepare(
        "SELECT id, external_id, created_at FROM transactions WHERE bot_id = ?1 AND exchange_id = ?2 AND status = 0 AND external_status IN (0, 1) \
         AND (external_id IS NULL OR external_id NOT LIKE 'imported_%') ORDER BY id")?;
    let rows = s.query_map(params![bot.id, bot.exchange_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, String>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|(id, ext, at)| Ok((id, ext, created_us(&at)?))).collect()
}

/// One fill, committed. A fill that spends the amount limit enqueues one Rails Bot::StopJob per qualifying callback
/// (quote_amount_limitable.rb:111), and mails stopped_by_amount_limit: its marker (notice::LIMIT; the base cap's, notice::BASE_LIMIT) is written in the fill's
/// transaction. Rails mails once per callback; the marker is one key, so two such fills before a send owe one mail. Outside a sweep (a follow-up poll: that job runs right after FetchAndUpdateOrderJob, which
/// has nothing left to do) the stop lands in the fill's own transaction. Without it (the tick's sweep: the job runs after the
/// whole Bot::ActionJob) one pending stop is counted in `transient_data.rust_amount_limit_stops_pending`, by `json_set` in the
/// fill's own transaction, so a crash before the tick ends loses nothing: tick::run_pending_amount_limit_stops runs them at
/// the tick's end, at the next start (run::step), or at the handback.
/// notify_stopped_by_amount_limit runs in the fill callback that spends the cap, whatever the stop then does: the mail
/// (notice::LIMIT) is owed in that fill's own transaction, on every path that applies a fill (here and placement's recovery).
fn apply_committed(c: &Connection, bot_id: i64, tx_id: i64, s: &OrderState, now: DateTime<Utc>, mode: PollMode, version: &Option<model::CredentialVersion>) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    let fenced=model::check_credential_result(&tx, version)?;
    if let Some(cap) = apply_order_in(&fenced, bot_id, tx_id, s, now, mode == PollMode::Terminal, None)? {
        amount_limit::owe_mail(&tx, bot_id, cap, now)?;
        if mode != PollMode::Sweep {
            tick::stop_for_amount_limit(&tx, bot_id, cap, now)?;
        } else {
            // Counted under the bot's start and that cap's limit (Bot::cap_key), with the cap's `kind`; a stale count from
            // before a restart, or one of the other cap, restarts at 1.
            let bot = model::load_bot(&tx, bot_id)?;
            let n = match bot.pending_amount_limit_stops()? { Some((n, true)) if bot.pending_amount_limit_cap() == cap => n + 1, _ => 1 };
            tx.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_amount_limit_stops_pending', json(?1)) WHERE id = ?2",
                       params![serde_json::json!({ "count": n, "key": bot.cap_key(cap)?, "kind": cap.kind() }).to_string(), bot_id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Bot::FetchAndUpdateOpenOrdersJob for one bot. Errors carry Rails' messages; orders updated before an
/// error stay updated, as each Rails update! commits on its own.
pub async fn sweep<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, now: DateTime<Utc>) -> Result<(), PollFailure> {
    let db = |e: EngineError| poll_error(e);
    let handle=crate::venue::Handle::for_bot(venue,c,bot).map_err(db)?;
    let venue=&handle;
    poll_rows(c, venue, bot, waiting_ids(c, bot).map_err(db)?, now, PollMode::Sweep).await?;
    // D-KUCOIN-HF-RETENTION: a row this bot cannot tell holds its spending (reconcile::barrier); the tick waits.
    match super::reconcile::barrier(c, bot.id).map_err(db)? {
        Some((tx_id, why)) => Err(PollFailure::AwaitingReconciliation(tx_id, why)),
        None => Ok(()),
    }
}

/// Read only identity columns before the venue parses status or money. Complete stored IDs win.
fn order_identified(c: &Connection, bot: &model::Bot, tx_id: i64, pair: Option<&str>, class: Option<&str>) -> Result<bool, EngineError> {
    let stored: bool = c.query_row("SELECT base_asset_id IS NOT NULL AND quote_asset_id IS NOT NULL FROM transactions WHERE id=?1", [tx_id], |r| r.get(0))?;
    if stored || model::exchange_type(c, bot)? != "Exchanges::Alpaca" { return Ok(true); }
    Ok(model::alpaca_order_ticker(c, bot.exchange_id, pair, class)?.is_some())
}

/// Both Rails polls go through Exchanges::Kraken's QueryOrders → TradesHistory → StaleOrderResolver path, over
/// the given rows. The sweep fails on `unknown` like FetchAndUpdateOpenOrdersJob; the follow-up skips it.
/// A terminal refresh uses the same lookups, but never abandons a missing order.
/// Returns when the earliest zero-fill answer it skipped becomes final (OrderState::settles_at).
async fn poll_rows<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, rows: Vec<(i64, String, i64)>, now: DateTime<Utc>, mode: PollMode) -> Result<Option<DateTime<Utc>>, PollFailure> {
    let strict = mode == PollMode::Sweep;
    let db = |e: EngineError| poll_error(e);
    if rows.is_empty() { return Ok(None); }
    let version = venue.producer();
    let ids: Vec<String> = rows.iter().map(|(_, e, _)| e.clone()).collect();
    let mut found: Vec<OrderState> = vec![];
    let mut skipped = vec![];
    // Each row with what its venue recorded at acceptance (D-KUCOIN-HF-ORDERS: a KuCoin HF order's route).
    let refs = rows.iter().map(|(id, ext, _)| crate::venue::OrderRef::load(c, *id, ext)).collect::<Result<Vec<_>, _>>().map_err(db)?;
    for batch in refs.chunks(50) {
        let (orders, excluded) = venue.orders_result(batch, |ext, pair, class| {
            let Some((id, _, _)) = rows.iter().find(|(_, e, _)| e == ext) else { return Ok(false); };
            order_identified(c, bot, *id, pair, class).map_err(|_| VenueError::Rejected(vec!["Order identity could not be read".into()]))
        }).await.value.map_err(|e| classify(e, &ids, venue.rules()))?;
        found.extend(orders.into_iter().map(|o| o.settled(now)));
        skipped.extend(excluded);
    }
    // D-KUCOIN-HF-RETENTION: an HF row the venue could not tell owes until a read applies (reconcile::unread).
    for (r, (id, _, _)) in refs.iter().zip(&rows) {
        if r.hf() && skipped.contains(&r.external_id) && !found.iter().any(|o| o.txid == r.external_id) {
            super::reconcile::unread(c, bot.id, *id, "KuCoin could not tell this order's state", &version, now).map_err(db)?;
        }
    }
    // A venue that never names missing ids (VenueRules::reports_missing false: Binance) leaves an unreported row waiting:
    // no trade recovery, no abandonment.
    let reports = venue.rules().reports_missing;
    let missing: Vec<String> = ids.iter().filter(|id| !skipped.contains(id) && !found.iter().any(|o| &&o.txid == id)).cloned().collect();
    if reports && !missing.is_empty() {
        let since = rows.iter().filter(|(_, e, _)| missing.contains(e)).map(|(_, _, at)| *at).min()
            .and_then(|at| at.checked_sub(3_600_000_000)).ok_or_else(|| db(super::schedule::time_range_error()))?;
        let fills = venue.fills_result(&missing, DateTime::from_timestamp_micros(since).ok_or_else(|| db(super::schedule::time_range_error()))?).await.value.map_err(|e| classify(e, &ids, venue.rules()))?;
        for f in fills {
            // Inconclusive (Exchanges::Kraken#recover_missing_from_trades): the sweep leaves it out of missing; get_order fails
            // without not_found, so the follow-up raises as Bot::FetchAndUpdateOrderJob does.
            if f.status == OrderStatus::Unknown {
                if mode == PollMode::FollowUp {
                    let tx_id = rows.iter().find(|(_, e, _)| e == &f.txid).map_or(0, |r| r.0);
                    return Err(PollFailure::General(format!("Failed to fetch order {tx_id}. Result: {}", inspect(&[format!("Kraken could not tell whether order {} filled", f.txid)]))));
                }
                skipped.push(f.txid);
            } else {
                found.push(OrderState { status: OrderStatus::Closed, ..f });
            }
        }
    }
    // Missing after both endpoints: Bot::StaleOrderResolver (Kraken is authoritative, so young ones just wait).
    let mut young = vec![];
    for (id, ext, created) in &rows {
        if mode == PollMode::Terminal || !reports || skipped.contains(ext) || found.iter().any(|o| &o.txid == ext) { continue; }
        if *created >= now.checked_sub_signed(Duration::days(STALE_AFTER_DAYS)).ok_or_else(|| db(super::schedule::time_range_error()))?.timestamp_micros() {
            young.push(ext.clone());
        } else {
            model::credential_write(c, &version, |tx| {
                require_owner(tx, bot.id, *id)?;
                tx.execute("UPDATE transactions SET external_status = ?1, updated_at = ?2 WHERE id = ?3",
                      params![TxExternalStatus::Abandoned as i64, format_time(now), id])?;
                model::log_activity(tx, bot.id, "order_abandoned", Level::Info, json!({ "order_id": ext }), now)?;
                super::composition::halt_abandoned_redeploy(tx, bot.id, *id)?;
                super::reversal::settle(tx, bot.id, *id).map(|_| ())
            }).map_err(db)?;
        }
    }
    let mut settles_at: Option<DateTime<Utc>> = None;
    for state in &found {
        let Some((id, _, _)) = rows.iter().find(|(_, e, _)| e == &state.txid) else { continue };
        if state.status == OrderStatus::Unknown {
            // An HF row's unknown state is its obligation (reconcile::admit), never a failed sweep.
            if refs.iter().any(|r| r.hf() && r.external_id == state.txid) {
                apply_committed(c, bot.id, *id, state, now, mode, &version).map_err(db)?;
                settles_at = settles_at.into_iter().chain(state.settles_at()).min();
                continue;
            }
            if strict { return Err(PollFailure::General(format!("Order {} status is unknown.", state.txid))); }
            settles_at = settles_at.into_iter().chain(state.settles_at()).min();
            continue;
        }
        apply_committed(c, bot.id, *id, state, now, mode, &version).map_err(db)?;
    }
    // FetchAndUpdateOpenOrdersJob, after the progress above: a young order a venue without a fill fallback of its own
    // (Exchange#authoritative_missing_orders? false) left out may hide a fill, so the sweep fails loudly.
    if strict && !young.is_empty() && venue.rules().young_missing_fails {
        return Err(PollFailure::General(format!("Exchange omitted recent order(s): {}", to_sentence(&young))));
    }
    Ok(settles_at)
}

/// Bot::FetchAndUpdateOrderJob for ONE order (enqueued per order at placement, and at engine start for every
/// outstanding order), for a bot in ANY status: the same QueryOrders → TradesHistory → stale path, lenient on
/// `unknown`. One order's failure never holds back another's fill. The caller retries Transient/RateLimited
/// failures as that job's retry_on does (3 and 4 attempts); others end the chain. A row no longer waiting is done.
pub async fn follow_up<V: Venue>(c: &Connection, venue: &V, bot_id: i64, tx_id: i64, now: DateTime<Utc>) -> Result<FollowUp, PollFailure> {
    let db = |e: EngineError| poll_error(e);
    // A row moved to another bot (Merge, Split) is that bot's to poll, before the venue is asked or after it answered: the
    // obligation is handed on, whatever this poll's outcome, and never dropped as Done.
    if let Some(owner) = moved_to(c, bot_id, tx_id).map_err(db)? { return Ok(FollowUp::OwnerChanged(owner)); }
    let bot = model::load_bot(c, bot_id).map_err(db)?;
    let handle=crate::venue::Handle::for_bot(venue,c,&bot).map_err(db)?;
    let venue=&handle;
    let rows: Vec<_> = waiting_ids(c, &bot).map_err(db)?.into_iter().filter(|(id, _, _)| *id == tx_id).collect();
    let polled = if venue.rules().follow_up_strict { follow_up_strict(c, venue, &bot, rows, tx_id, now, PollMode::FollowUp).await } else { poll_rows(c, venue, &bot, rows, now, PollMode::FollowUp).await };
    if let Some(owner) = moved_to(c, bot_id, tx_id).map_err(db)? { return Ok(FollowUp::OwnerChanged(owner)); }
    let settles_at = polled?;
    if let Some(at) = settles_at { return Ok(FollowUp::SettlesAt(at)); }
    // ponytail: the lenient path skips an `unknown` answer, so a row already open from an earlier poll reads as still open
    // here; bounded by the caller's 8 attempts and read-only, so a few extra polls at worst.
    let open: bool = c.query_row("SELECT status = 0 AND external_status = 1 FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0))
        .map_err(|e| db(e.into()))?;
    Ok(if open { FollowUp::StillOpen } else { FollowUp::Done })
}

/// Bot::FetchAndUpdateOrderJob through Exchange#get_order, as Rails runs it for Alpaca and Binance: one order (Venue::order_identified); a throttle or transient
/// failure is retried by the caller (retry_on); any other failure raises "Failed to fetch order <transaction id>"; an
/// unknown status raises. A raise changes no row, as in Rails. In the follow-up chain a zero-fill answer
/// too young to be final is no raise: its time is returned for the caller to ask again (D-KUCOIN-ZERO-FILL-FUNDS).
async fn follow_up_strict<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, rows: Vec<(i64, String, i64)>, tx_id: i64, now: DateTime<Utc>, mode: PollMode) -> Result<Option<DateTime<Utc>>, PollFailure> {
    let Some((id, ext, _)) = rows.into_iter().next() else { return Ok(None) };
    let rules = venue.rules();
    let version = venue.producer();
    let order = crate::venue::OrderRef::load(c, id, &ext).map_err(poll_error)?;
    let answer = venue.order_result(&order, |_, pair, class| {
        order_identified(c, bot, id, pair, class).map_err(|_| VenueError::Rejected(vec!["Order identity could not be read".into()]))
    }).await.value;
    // D-KUCOIN-HF-RETENTION: an HF row this read could not tell owes before the failure is returned (and retried).
    if order.hf() && !matches!(&answer, Ok((found, _)) if !found.is_empty()) {
        super::reconcile::unread(c, bot.id, id, "KuCoin could not tell this order's state", &version, now).map_err(poll_error)?;
    }
    let state = match answer {
        Ok((_, skipped)) if skipped.contains(&ext) => return Err(PollFailure::General(format!("Failed to fetch order {tx_id}. Result: unsupported class or unmapped/ambiguous order identity"))),
        Ok((mut found, _)) if !found.is_empty() => found.remove(0).settled(now),
        Ok(_) if mode == PollMode::Terminal => return Ok(None),
        Ok(_) => return Err(PollFailure::General(format!("Failed to fetch order {tx_id}. Result: []"))),
        Err(VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. }) if rules.is_throttle(&errs) => return Err(PollFailure::RateLimited(to_sentence(&errs))),
        Err(VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. }) if rules.is_transient(&errs) => return Err(PollFailure::Transient(to_sentence(&errs))),
        Err(VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. }) => return Err(PollFailure::General(format!("Failed to fetch order {tx_id}. Result: {}", inspect(&errs)))),
        Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => return Err(PollFailure::Transient(m)),
    };
    // An HF row's unknown state is its obligation (reconcile::admit), written before the follow-up waits or fails.
    if order.hf() && state.status == OrderStatus::Unknown { apply_committed(c, bot.id, id, &state, now, mode, &version).map_err(poll_error)?; }
    if let Some(at) = state.settles_at().filter(|_| mode == PollMode::FollowUp) { return Ok(Some(at)); }
    if state.status == OrderStatus::Unknown {
        return if mode == PollMode::Terminal { Ok(None) } else { Err(PollFailure::General(format!("Order {ext} status is unknown."))) };
    }
    apply_committed(c, bot.id, id, &state, now, mode, &version).map_err(poll_error).map(|()| None)
}

/// Bot::FetchAndUpdateOrderJob for one row the caller names, whatever its external status (Bot::Rebalancer#refresh_pending_order!
/// re-fetches the pending transaction even once closed: a closed sell may still lack its proceeds). Only this venue's rows
/// are asked (the job skips a row placed elsewhere); a row not submitted is left alone.
pub async fn refresh<V: Venue>(c: &Connection, venue: &V, bot_id: i64, tx_id: i64, now: DateTime<Utc>) -> Result<(), PollFailure> {
    refresh_row(c, venue, bot_id, tx_id, now, false).await
}

/// Late proceeds for an already-terminal liquidation. Keep its status and recorded execution
/// when the venue has nothing new, and apply only positive execution data from a terminal answer.
pub async fn refresh_terminal<V: Venue>(c: &Connection, venue: &V, bot_id: i64, tx_id: i64, now: DateTime<Utc>) -> Result<(), PollFailure> {
    refresh_row(c, venue, bot_id, tx_id, now, true).await
}

async fn refresh_row<V: Venue>(c: &Connection, venue: &V, bot_id: i64, tx_id: i64, now: DateTime<Utc>, execution_only: bool) -> Result<(), PollFailure> {
    let db = |e: EngineError| poll_error(e);
    let bot = model::load_bot(c, bot_id).map_err(db)?;
    let handle = crate::venue::Handle::for_bot(venue, c, &bot).map_err(db)?;
    let venue = &handle;
    let mut s = c.prepare("SELECT id, external_id, created_at, external_status FROM transactions WHERE id = ?1 AND bot_id = ?2 AND exchange_id = ?3 AND status = 0 \
                           AND external_id IS NOT NULL AND external_id NOT LIKE 'imported_%'").map_err(|e| db(e.into()))?;
    let rows = s.query_map(params![tx_id, bot.id, bot.exchange_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?)))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).map_err(|e| db(e.into()))?;
    let terminal = rows.first().is_some_and(|(_, _, _, status)| matches!(status, Some(2) | Some(3)));
    let rows = rows.into_iter().map(|(id, ext, at, _)| Ok((id, ext, created_us(&at)?))).collect::<Result<Vec<_>, EngineError>>().map_err(db)?;
    if execution_only && !terminal { return Ok(()); }
    if venue.rules().follow_up_strict {
        follow_up_strict(c, venue, &bot, rows, tx_id, now, if execution_only { PollMode::Terminal } else { PollMode::Refresh }).await.map(|_| ())
    } else {
        poll_rows(c, venue, &bot, rows, now, if execution_only { PollMode::Terminal } else { PollMode::FollowUp }).await.map(|_| ())
    }
}
