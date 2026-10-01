//! One tick of an eligible bot: Bot::ActionJob#perform around DcaMultiAsset#execute_action (with the
//! Fundable and LimitOrderable decorators), and the failure handling of ActionJob's rescues.
use super::amount::{self, RowKind, Sizing};
use super::venue_rules::VenueRules;
use super::model::{self, Level};
use super::placement::{self, Recovery, Sent};
use super::polling::{self, PollFailure};
use super::{Clock, EngineError};
use crate::codec::format_time;
use crate::enums::BotStatus;
use crate::ruby::{iso8601_ms, to_sentence, BigDec};
use crate::venue::{PriceSide, Venue, VenueError};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::time::Duration;

pub const MAX_ATTEMPTS: u32 = 4;
const BLOCKING_KINDS: [&str; 3] = ["invalid_key", "permission_denied", "restricted"];
const THREE_DAYS: f64 = 259_200.0;

/// ActiveJob's exception_executions, one counter per retry_on handler.
#[derive(Debug, Default, Clone, Copy)]
pub struct Attempts { pub transient: u32, pub rate: u32 }

#[derive(Debug)]
pub enum TickOutcome { Skipped, Done { placed: bool }, RetryAfter(Duration), Rescheduled, Stopped, AwaitingReconciliation }

/// `:polynomially_longer` with retry_jitter 0 (executions**4 + 2 s), or BotJob::RATE_LIMIT_WAIT (15 s × executions).
pub fn retry_wait(executions: u32, rate_limited: bool) -> Duration {
    Duration::from_secs(if rate_limited { 15 * executions as u64 } else { (executions as u64).pow(4) + 2 })
}

pub fn stop(c: &Connection, bot_id: i64, stop_message_key: &str, now: chrono::DateTime<chrono::Utc>) -> Result<(), EngineError> {
    let n = c.execute(&format!("UPDATE bots SET status = ?1, stopped_at = ?2, stop_message_key = ?3, updated_at = ?2 WHERE id = ?4 AND status IN ({})", model::working_list()),
              params![BotStatus::Stopped as i64, format_time(now), stop_message_key, bot_id])?;
    if n == 0 { return Ok(()); }
    model::log_activity(c, bot_id, "stopped", Level::Info, json!({ "stop_message_key": stop_message_key }), now)
}

enum Fail {
    Transient(String),
    RateLimited(String),
    General { message: String, errors: Vec<String>, failed_row: bool },
    PlacementSafe(String),
    Ambiguous(String),
}

fn record_failure(c: &Connection, bot_id: i64, kind: Option<&str>) -> Result<(), EngineError> {
    model::merge_transient_compact(c, bot_id, &[("last_failure_kind", kind.map(Value::from).unwrap_or(Value::Null))])
}

pub async fn tick<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts) -> Result<TickOutcome, EngineError> {
    tick_recovering(c, venue, bot_id, clock, attempts, &mut None).await
}

/// `tick`, also reporting the transaction a persisted intent was settled into this tick (its row carries the intent's
/// earlier `created_at`, so the caller cannot find it by time and must queue its follow-up poll itself).
pub async fn tick_recovering<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>) -> Result<TickOutcome, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    // An intent is settled whatever the status: a bot stopped after an ambiguous send still owns that order.
    match placement::recover(c, venue, &bot, clock).await? {
        Recovery::Pending => return Ok(TickOutcome::AwaitingReconciliation),
        Recovery::Recorded(tx) => *recovered = Some(tx),
        Recovery::NoIntent | Recovery::NotPlaced => {}
    }
    let bot = model::load_bot(c, bot_id)?;
    if !matches!(bot.status, BotStatus::Scheduled | BotStatus::Retrying) { return Ok(TickOutcome::Skipped); }

    let now = clock.now();
    // Rails' store_accessor writes a key only when the value changes, so `waiting_for_market_open: nil` touches the
    // key only where it holds something non-null; an absent key stays absent.
    let mut writes = vec![("last_action_job_at", json!(iso8601_ms(now)))];
    if bot.transient.get("waiting_for_market_open").is_some_and(|v| !v.is_null()) { writes.push(("waiting_for_market_open", Value::Null)); }
    model::update_transient(c, bot_id, &writes, now)?;

    // Anything that goes wrong inside execute_action fails this bot's tick the way a StandardError does in
    // Rails (retrying, execution_failed, next checkpoint) — it never leaves the bot `executing`.
    let executed = match execute(c, venue, bot_id, clock).await {
        Ok(r) => r,
        Err(e @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(e),
        Err(e) => Err(Fail::General { message: format!("{e:?}"), errors: vec![], failed_row: false }),
    };
    match executed {
        Ok(placed) => {
            *attempts = Attempts::default();
            // clear_failure_state!, then back to scheduled unless stopped meanwhile.
            if model::load_bot(c, bot_id)?.last_failure_kind().is_some() { record_failure(c, bot_id, None)?; }
            Ok(if model::transition_working(c, bot_id, BotStatus::Scheduled, clock.now())? { TickOutcome::Done { placed } } else { TickOutcome::Skipped })
        }
        Err(fail) => handle_failure(c, bot_id, fail, clock, attempts, venue.rules()),
    }
}

/// DcaMultiAsset#execute_action with the sweep in front and Fundable behind. Ok(placed) = success.
async fn execute<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock) -> Result<Result<bool, Fail>, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    if let Err(f) = polling::sweep(c, venue, &bot, clock.now()).await {
        return Ok(Err(match f {
            PollFailure::RateLimited(m) => Fail::RateLimited(m),
            PollFailure::Transient(m) => Fail::Transient(m),
            PollFailure::General(m) => Fail::General { errors: vec![m.clone()], message: m, failed_row: false },
        }));
    }
    // Statusable#transition_working!: every status write of the tick moves only a still-working bot, so a stop that
    // lands meanwhile (during the sweep, or while AddOrder awaits its reply) wins. Stopped before executing, nothing
    // is placed, but Fundable still reads the balance as it wraps execute_action.
    let working = model::transition_working(c, bot_id, BotStatus::Executing, clock.now())?;
    let bot = model::load_bot(c, bot_id)?;
    let ticker_row = model::ticker_for(c, &bot)?;
    let mut placed = false;
    if working {
        // Bots::DcaMultiAsset#refresh_composition → derive_composition: only an available, trading-enabled ticker
        // counts. It runs before anything is sized, so an untradable pair fails the tick with no order row.
        let ticker = match &ticker_row {
            Some(t) if t.available && t.trading_enabled => t,
            _ => {
                let m = format!("None of the portfolio's weighted assets trade on {}", model::exchange_name(c, &bot)?);
                return Ok(Err(Fail::General { message: m, errors: vec![], failed_row: false }));
            }
        };
        let x = amount::pending_quote_amount(c, &bot, clock.now().timestamp_micros())?;
        if !x.is_zero() {
            // Bot::OrderSetter#reference_price: the last trade for a limit buy, the ask for a market buy. A zero book is the
            // venue's own "Wrong … price" error, which Rails' composition rescues into "No price for …".
            let side = if bot.limit_distance().is_some() { PriceSide::Last } else { PriceSide::Ask };
            let reference = match venue.price(ticker, side).await {
                Ok(p) => p,
                Err(VenueError::Rejected(e)) => return Ok(Err(Fail::Transient(format!("No price for {}: {}", ticker.base_symbol, to_sentence(&e))))),
                Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => return Ok(Err(Fail::Transient(format!("No price for {}: {m}", ticker.base_symbol)))),
            };
            match amount::size(&bot, ticker, &x, &reference, venue.rules().minimum_logic) {
                Sizing::Nothing => {}
                Sizing::Ignored(plan) => model::log_activity(c, bot_id, "order_ignored", Level::Info, plan.log_details(), clock.now())?,
                Sizing::BelowMinimum(plan) => {
                    model::log_activity(c, bot_id, "order_skipped", Level::Warning, plan.log_details(), clock.now())?;
                    amount::write_order_row(c, &bot, &plan, RowKind::Skipped, clock.now())?;
                }
                Sizing::ZeroPrice { decimals } => {
                    let m = format!("limit price rounds to zero at {decimals} decimals");
                    return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
                }
                Sizing::Place(plan) => {
                    let intent = placement::begin(c, &bot, &plan, clock)?;
                    match placement::send(venue, &intent, clock).await {
                        Sent::Accepted(txid) => { placement::record_accepted(c, &bot, &intent, &txid)?; placed = true; }
                        Sent::Rejected(errs) => {
                            let row = placement::record_rejected(c, &bot, &intent, &errs)?;
                            let m = to_sentence(&errs);
                            return Ok(Err(if row { Fail::General { message: m, errors: errs, failed_row: true } } else { Fail::PlacementSafe(m) }));
                        }
                        Sent::Ambiguous(m) => return Ok(Err(Fail::Ambiguous(m))), // the intent stays
                        Sent::NotSent(m) => { placement::drop_intent(c, bot_id)?; return Ok(Err(Fail::Transient(m))); }
                    }
                }
            }
        }
        // A stop that landed while AddOrder awaited its reply stays; the order already sent stands and is still polled.
        model::transition_working(c, bot_id, BotStatus::Waiting, clock.now())?;
    }
    let Some(ticker) = ticker_row else { return Ok(Ok(placed)) }; // stopped, and no pair to read a balance for

    // Bot::Fundable: a failed balance Result means "not low"; only a transport failure raises.
    match venue.balance(&ticker.quote_symbol).await {
        Ok(free) => {
            let interval_seconds = match bot.interval().map(|i| i.as_str()) { Some("hour") => 3_600.0, Some("day") => 86_400.0, Some("week") => 604_800.0, _ => 2_629_746.0 };
            let buffer = BigDec::from_f64(bot.quote_amount().unwrap_or_default() / interval_seconds * THREE_DAYS).map_err(|e| EngineError::Data(format!("{e:?}")))?;
            if free < buffer && !notified_in_last_day(c, &bot, clock)? {
                let now = format_time(clock.now());
                c.execute("UPDATE bots SET last_end_of_funds_notification = ?1, updated_at = ?1 WHERE id = ?2", params![now, bot_id])?;
            }
        }
        // honeymaker's with_rescue turns a network failure into a Failure result: "not low", the tick succeeds.
        Err(VenueError::Rejected(_) | VenueError::Transient(_)) => {}
        // An unreadable body raises a plain StandardError in Rails: execution_failed, no retry.
        Err(VenueError::Ambiguous(m)) => return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false })),
    }
    Ok(Ok(placed))
}

/// Bot::Fundable#notified_in_last_day?
fn notified_in_last_day(c: &Connection, bot: &model::Bot, clock: &dyn Clock) -> Result<bool, EngineError> {
    let since = format_time(clock.now() - chrono::Duration::days(1));
    let n: i64 = c.query_row(
        "SELECT count(*) FROM bots WHERE user_id = ?1 AND json_extract(settings, '$.quote_asset_id') = ?2 \
         AND last_end_of_funds_notification IS NOT NULL AND last_end_of_funds_notification > ?3",
        params![bot.user_id, bot.quote_asset_id(), since], |r| r.get(0))?;
    Ok(n > 0)
}

fn handle_failure(c: &Connection, bot_id: i64, fail: Fail, clock: &dyn Clock, attempts: &mut Attempts, rules: &VenueRules) -> Result<TickOutcome, EngineError> {
    let now = clock.now();
    if !model::transition_working(c, bot_id, BotStatus::Retrying, now)? { return Ok(TickOutcome::Skipped); }
    match fail {
        // `rescue TransientNetworkError, RateLimitedError`: :transient is recorded, then retry_on counts per handler.
        Fail::Transient(m) => {
            record_failure(c, bot_id, Some("transient"))?;
            attempts.transient += 1;
            if attempts.transient < MAX_ATTEMPTS { return Ok(TickOutcome::RetryAfter(retry_wait(attempts.transient, false))); }
            *attempts = Attempts::default();
            model::log_activity(c, bot_id, "execution_retrying", Level::Info, json!({ "error": m, "transient_exhausted": true }), now)?;
            Ok(TickOutcome::Rescheduled)
        }
        Fail::RateLimited(m) => {
            record_failure(c, bot_id, Some("transient"))?;
            attempts.rate += 1;
            if attempts.rate < MAX_ATTEMPTS { return Ok(TickOutcome::RetryAfter(retry_wait(attempts.rate, true))); }
            *attempts = Attempts::default();
            let kind = rules.failure_kind(std::slice::from_ref(&m));
            record_failure(c, bot_id, kind)?;
            model::log_activity(c, bot_id, "execution_failed", Level::Error, json!({ "error": m, "kind": kind, "rate_limited_exhausted": true }), now)?;
            Ok(TickOutcome::Rescheduled)
        }
        Fail::PlacementSafe(m) => {
            *attempts = Attempts::default(); // the retry chain ends here: the next run is a fresh job
            record_failure(c, bot_id, Some("transient"))?;
            model::log_activity(c, bot_id, "execution_retrying", Level::Info, json!({ "error": m, "placement_transient": true }), now)?;
            Ok(TickOutcome::Rescheduled)
        }
        Fail::Ambiguous(m) => {
            *attempts = Attempts::default();
            record_failure(c, bot_id, None)?;
            model::log_activity(c, bot_id, "placement_ambiguous", Level::Warning, json!({ "error": m }), now)?;
            Ok(TickOutcome::AwaitingReconciliation)
        }
        Fail::General { message, errors, failed_row } => {
            *attempts = Attempts::default();
            let kind = rules.failure_kind(if errors.is_empty() { std::slice::from_ref(&message) } else { &errors });
            let previous = model::load_bot(c, bot_id)?.last_failure_kind();
            let blocking = kind.is_some_and(|k| BLOCKING_KINDS.contains(&k) && previous.as_deref() == Some(k));
            record_failure(c, bot_id, kind)?;
            if !failed_row {
                model::log_activity(c, bot_id, "execution_failed", Level::Error, json!({ "error": message, "kind": kind }), now)?;
            }
            if blocking {
                stop(c, bot_id, &format!("bot.status.stopped_by_error.{}", kind.unwrap()), now)?;
                return Ok(TickOutcome::Stopped);
            }
            Ok(TickOutcome::Rescheduled)
        }
    }
}
