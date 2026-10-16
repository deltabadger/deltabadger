//! One tick of an eligible bot: Bot::ActionJob#perform around DcaMultiAsset#execute_action (with the
//! Fundable and LimitOrderable decorators), and the failure handling of ActionJob's rescues.
use crate::venue::Attributed;
use super::amount::{self, RowKind, Sizing};
use super::venue_rules::VenueRules;
use super::model::{self, Level};
use super::notice;
use super::{splits, index};
use super::clock::{self as market, Gate};
use super::placement::{self, Recovery, Sent};
use super::polling::{self, PollFailure};
use super::{basket, staleness, Clock, EngineError};
use chrono::{DateTime, Utc};
use crate::codec::format_time;
use crate::enums::BotStatus;
use crate::ruby::{iso8601_ms, to_sentence, BigDec};
use crate::venue::{PriceSide, Venue, VenueError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::time::Duration;

pub const MAX_ATTEMPTS: u32 = 4;
const BLOCKING_KINDS: [&str; 3] = ["invalid_key", "permission_denied", "restricted"];
const THREE_DAYS: f64 = 259_200.0;
/// The marker that makes the out-of-funds mail durable: transient_data.rust_funds_mail_pending, written with the stamp.
pub const FUNDS_MAIL_PENDING: &str = "rust_funds_mail_pending";

/// The out-of-funds budget stamp and the mail it owes, in ONE statement: both land or neither does. `touch`:
/// Bot::Fundable stamps through `update!` (updated_at moves), Bot::Failable through `update_column` (it does not).
fn stamp_funds_low(c: &model::FencedTransaction<'_>, bot: &model::Bot, now: chrono::DateTime<chrono::Utc>, touch: bool) -> Result<(), EngineError> {
    let marker = json!({ "quote_asset": bot.quote_asset_id(), "stamped_at": iso8601_ms(now) }).to_string();
    let updated_at = if touch { ", updated_at = ?1" } else { "" };
    c.execute(&format!("UPDATE bots SET last_end_of_funds_notification = ?1{updated_at}, \
                        transient_data = json_set(transient_data, '$.{FUNDS_MAIL_PENDING}', json(?3)) WHERE id = ?2"),
              params![format_time(now), bot.id, marker])?;
    Ok(())
}

/// ActiveJob's exception_executions, one counter per retry_on handler.
#[derive(Debug, Default, Clone, Copy)]
pub struct Attempts { pub transient: u32, pub rate: u32, pub open: u32 }

#[derive(Debug)]
pub enum TickOutcome {
    /// The market is closed (Bot::ActionJob's closed path): `waiting_for_market_open` and `market_closed` were written, nothing
    /// else. The bot stays due and `run` holds it until `until` (the clock's next_open).
    MarketClosed { until: DateTime<Utc>, producer: Option<model::CredentialVersion> },

    Skipped, Done { placed: bool }, RetryAfter(Duration), Rescheduled, Stopped, AwaitingReconciliation,
    /// Reference data only a Rails job refreshes is past its bound (staleness.rs): nothing was read from the venue, nothing
    /// written, and the bot stays due.
    Stale { source: &'static str, message: String },
}

/// `:polynomially_longer` with retry_jitter 0 (executions**4 + 2 s), or BotJob::RATE_LIMIT_WAIT (15 s × executions).
pub fn retry_wait(executions: u32, rate_limited: bool) -> Duration {
    Duration::from_secs(if rate_limited { 15 * executions as u64 } else { (executions as u64).pow(4) + 2 })
}

/// stop_message_key of the amount-limit stop (config/locales/bot.en.yml: "The whole amount has been invested.").
pub const AMOUNT_SPENT: &str = "bot.settings.extra_amount_limit.amount_spent";

/// Bot::ActionJob#stop_for_blocking_failure: only a still-working bot is stopped.
pub fn stop(c: &Connection, bot_id: i64, stop_message_key: &str, now: DateTime<Utc>) -> Result<(), EngineError> {
    stop_owing_mail(c, bot_id, stop_message_key, None, now).map(|_| ())
}

/// `stop`, and in the same statement the marker of the mail Rails sends with this stop (engine::notice): the mail is owed
/// only if this call is the one that stopped the bot. Returns whether it was (false: a stop, archive or delete got there first).
/// The stop (with its marker) and its activity-log row are one transaction on every path: the caller's when it has one,
/// else this call's own. A log row that cannot be written leaves the bot working and nothing owed, so a retry writes all three.
pub fn stop_owing_mail(c: &Connection, bot_id: i64, stop_message_key: &str, mail: Option<(&str, Value)>, now: DateTime<Utc>) -> Result<bool, EngineError> {
    stop_if(c, bot_id, stop_message_key, mail, &format!("status IN ({})", model::working_list()), now)
}

/// The one stop write: status, stopped_at, stop_message_key and the optional mail marker where `condition` holds, then its
/// `stopped` log, in one transaction. Returns whether it stopped the bot.
fn stop_if(c: &Connection, bot_id: i64, stop_message_key: &str, mail: Option<(&str, Value)>, condition: &str, now: DateTime<Utc>) -> Result<bool, EngineError> {
    if c.is_autocommit() {
        let tx = model::immediate_retry_busy(c)?;
        let stopped = stop_if(&tx, bot_id, stop_message_key, mail, condition, now)?;
        tx.commit()?;
        return Ok(stopped);
    }
    let (path, marker) = match &mail { Some((key, marker)) => (format!("$.{key}"), marker.to_string()), None => ("$".into(), String::new()) };
    model::locked(c, |c| {
        // STOP invalidates even a same-timestamp run; preserve every unrelated transient key.
        let n = c.execute(&format!("UPDATE bots SET status = ?1, stopped_at = ?2, stop_message_key = ?3, updated_at = ?2, \
                                    transient_data = json_set(CASE WHEN ?5 = '$' THEN transient_data ELSE json_set(transient_data, ?5, json(?6)) END, '$.run_generation', coalesce(json_extract(transient_data, '$.run_generation'),0)+1) \
                                    WHERE id = ?4 AND {condition}"),
                  params![BotStatus::Stopped as i64, format_time(now), stop_message_key, bot_id, path, marker])?;
        if n == 0 { return Ok(false); }
        model::log_activity(c, bot_id, "stopped", Level::Info, json!({ "stop_message_key": stop_message_key }), now)?;
        Ok(true)
    })
}

/// The stop Bot::QuoteAmountLimitable#handle_quote_amount_limit_update enqueues (Bot::StopJob → Bot::Lifecycle#stop,
/// lifecycle.rb:91-115): every status but archived and deleted, so a stopped bot is stopped again (a new stopped_at and a
/// `stopped` log). It owes no mail itself: Rails mails stopped_by_amount_limit from the fill callback, whatever the stop then
/// does, so the marker (notice::LIMIT) is written where the stop is counted (polling::apply_committed).
pub fn stop_for_amount_limit(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    stop_if(c, bot_id, AMOUNT_SPENT, None, &format!("status NOT IN ({}, {})", BotStatus::Deleted as i64, BotStatus::Archived as i64), now).map(|_| ())
}

/// The amount-limit stops a sweep counted (polling::apply_committed), one per qualifying fill callback, as Rails runs one
/// Bot::StopJob per callback: Bot::Lifecycle#stop writes a `stopped` log every time it succeeds, a stopped bot included
/// (lifecycle.rb:91-115). Each stop consumes one count in its own transaction, so a crash replays exactly the rest: at the end
/// of the tick that swept, at the next start (run::step), or at the handback.
/// A count that no longer applies (Bot::pending_amount_limit_stops) is discarded and logged, never run.
pub fn run_pending_amount_limit_stops(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    drain_amount_limit_stops(c, bot_id, now, None)
}

fn drain_amount_limit_stops(c: &Connection, bot_id: i64, now: DateTime<Utc>, owner: Option<&model::Bot>) -> Result<(), EngineError> {
    let mut stopped_here = false;
    let mut expected_run = owner.map(model::Bot::run_identity).transpose()?;
    loop {
        let tx = model::immediate(c)?;
        if let Some(expected_run) = &expected_run {
            let current = model::load_bot(&tx, bot_id)?;
            if &current.run_identity()? != expected_run
                || (!stopped_here && !crate::enums::BOT_WORKING.contains(&current.status)) { return Ok(()); }
        }
        let Some((n, current)) = model::load_bot(&tx, bot_id)?.pending_amount_limit_stops()? else { return Ok(()) };
        let remove = "UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_amount_limit_stops_pending') WHERE id = ?1";
        if !current || n <= 0 {
            // Counted before the bot was started (afresh or continued) or its limit changed: Rails' Bot::StopJob ran before either.
            tx.execute(remove, [bot_id])?;
            tx.commit()?;
            if !current { super::log(&format!("[engine] bot {bot_id}: {n} amount-limit stop(s) counted before a start or a limit change; discarded")); }
            return Ok(());
        }
        stop_for_amount_limit(&tx, bot_id, now)?;
        stopped_here = true;
        // Our own STOP advances the generation. Continue only from that identity, so a
        // concurrent restart still fences the remaining callbacks between transactions.
        if expected_run.is_some() { expected_run = Some(model::load_bot(&tx, bot_id)?.run_identity()?); }
        if n == 1 {
            tx.execute(remove, [bot_id])?;
        } else {
            tx.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_amount_limit_stops_pending.count', ?1) WHERE id = ?2", params![n - 1, bot_id])?;
        }
        tx.commit()?;
    }
}

enum Fail {
    CredentialsChanged,
    Transient(String),
    RateLimited(String),
    General { message: String, errors: Vec<String>, failed_row: bool },
    PlacementSafe(String),
    Ambiguous(String),
    /// A transport failure after this run already placed an order (Bot::ActionJob's TransientNetworkError rescue with a
    /// submitted row since action_started_at): never retried, because the replay would place a second order.
    TransientAfterPlacement,
}

type PriceKey = (i64, i64, PriceSide);

/// Exchange#get_{ask,last}_price's Rails.cache entry (exchanges/alpaca.rb:273-335, exchanges/kraken.rb:218-260): kept
/// 5 s from when it was written, keyed by exchange, ticker and side; only a usable (non-zero) price is stored. It lives
/// in the engine (and across a parity scenario's retries), never in a venue instance, so it survives venue recreation.
type CachedPrice = (chrono::DateTime<chrono::Utc>, model::Produced<BigDec>);
#[derive(Default)]
pub struct PriceCache(std::cell::RefCell<std::collections::HashMap<PriceKey, CachedPrice>>);

impl PriceCache {
    pub fn get(&self, key: PriceKey, now: chrono::DateTime<chrono::Utc>, version: &Option<model::CredentialVersion>) -> Option<BigDec> {
        let mut entries=self.0.borrow_mut();
        let stale=entries.get(&key).is_some_and(|(at,price)|
            now>=*at+chrono::Duration::seconds(5) || !price.current_for(version).is_fresh());
        if stale { entries.remove(&key); }
        entries.get(&key).map(|(_,price)|price.value.clone())
    }

    #[doc(hidden)]
    pub fn put(&self,key:PriceKey,now:DateTime<Utc>,price:BigDec,version:&Option<model::CredentialVersion>){self.put_result(key,now,model::Produced::new(price,version.clone()))}
    pub fn put_result(&self,key:PriceKey,now:DateTime<Utc>,price:model::Produced<BigDec>){self.0.borrow_mut().insert(key,(now,price));}

}

/// What a tick borrows from the engine that runs it.
pub struct TickContext<'a> {
    pub credential_version: Option<model::CredentialVersion>,
    pub prices: &'a PriceCache,
    pub below_minimum: &'a dyn Fn(i64, Vec<i64>),
    /// When this process started: Alpaca absence is trusted only a full margin (20 min) after it (placement::recover_since).
    pub process_start: chrono::DateTime<chrono::Utc>,
    /// A stop was requested (the engine's Shutdown); checked between recovery and execution.
    pub stopping: &'a dyn Fn() -> bool,
}

fn record_failure(c: &model::FencedTransaction<'_>, bot_id: i64, kind: Option<&str>) -> Result<(), EngineError> {
    model::record_failure_origin(c,bot_id)?;
    model::merge_transient_compact(c, bot_id, &[("last_failure_kind", kind.map_or(Value::Null,Value::from))])
}

/// Which of Bot::ActionJob's failing exits is recording the failure: it decides the mail.
#[derive(Clone, Copy, PartialEq)]
enum Exit {
    /// `rescue StandardError`, not blocking: notify_recoverable (end_of_funds for a buy refused for funds, else notify_about_error).
    Recoverable,
    /// `rescue StandardError`, the second blocking failure in a row: always notifies; the mail is stopped_by_error, owed by the stop itself.
    Blocking,
    /// EXHAUSTION_HANDLER after a rate limit: notify_about_error whatever the kind.
    Exhausted,
}

/// Bot::Failable#notify_about_failure? and #record_failure!(kind, notified:), with the marker of the mail Rails then sends
/// (engine::notice), written with the budget it spends.
/// - A buy refused for funds shares Bot::Fundable's budget: one mail a day per user and quote asset, stamped on
///   bots.last_end_of_funds_notification with update_column (updated_at does not move). The engine only buys.
/// - Every other kind has its own day in transient_data.failure_notifications[kind] (`unknown` for no kind): an ISO 8601
///   time without fraction, never cleared by a success.
fn record_notified_failure(c: &model::FencedTransaction<'_>, bot_id: i64, kind: Option<&str>, error: &str, exit: Exit, clock: &dyn Clock) -> Result<(), EngineError> {
    let now = clock.now();
    let bot = model::load_bot(c, bot_id)?;
    // Bot::Failable#shares_end_of_funds_budget?: never while selling, whose "insufficient" is base, not quote: the error
    // mail on the kind's own budget (action_job.rb#notify_recoverable).
    let shared = kind == Some("insufficient_funds") && !bot.selling();
    let key = kind.unwrap_or("unknown");
    let budget = bot.transient.get("failure_notifications").and_then(Value::as_object).cloned().unwrap_or_default();
    let notify = exit == Exit::Blocking || if shared {
        !notified_in_last_day(c, &bot, clock)?
    } else {
        // `notified_at.blank? || Time.zone.parse(notified_at) < 1.day.ago`. A value this cannot read counts as open:
        // one mail too many, never one too few.
        match budget.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(at) => crate::codec::parse_time(at).map_err(|_|EngineError::Data("unreadable stored timestamp".into()))? < now - chrono::Duration::days(1),
            None => true,
        }
    };
    let mut values = vec![("last_failure_kind", kind.map_or(Value::Null,Value::from))];
    if notify && shared && exit == Exit::Recoverable {
        stamp_funds_low(c, &bot, now, false)?;
    } else if notify && shared {
        c.execute("UPDATE bots SET last_end_of_funds_notification = ?1 WHERE id = ?2", params![format_time(now), bot_id])?;
    } else if notify {
        let mut budget = budget;
        budget.insert(key.to_string(), json!(now.format("%Y-%m-%dT%H:%M:%SZ").to_string()));
        values.push(("failure_notifications", Value::Object(budget)));
    }
    if notify && (exit == Exit::Exhausted || (exit == Exit::Recoverable && !shared)) {
        values.push((notice::ERROR, notice::error_marker(bot.transient.get(notice::ERROR), key, error, now)));
    }
    model::record_failure_origin(c,bot_id)?;
    model::merge_transient_compact(c, bot_id, &values)
}

pub async fn tick<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts) -> Result<TickOutcome, EngineError> {
    let mut bot = model::load_bot(c, bot_id)?;
    let prices = PriceCache::default();
    let cx = TickContext { credential_version: model::credential_version(c, &bot)?, prices: &prices, process_start: chrono::DateTime::<chrono::Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {} };
    tick_recovering_for_run(c, venue, &mut bot, clock, attempts, &mut None, &cx).await
}

/// `tick`, also reporting the transaction a persisted intent was settled into this tick (its row carries the intent's
/// earlier `created_at`, so the caller cannot find it by time and must queue its follow-up poll itself).
pub async fn tick_recovering<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>, cx: &TickContext<'_>) -> Result<TickOutcome, EngineError> {
    let mut bot = model::load_bot(c, bot_id)?;
    tick_recovering_for_run(c, venue, &mut bot, clock, attempts, recovered, cx).await
}

/// The engine passes the snapshot it selected BEFORE constructing the venue or doing other work.
/// Only our own timestamp/counter write advances this snapshot; it never adopts a restarted row.
#[allow(clippy::too_many_arguments)]
pub async fn tick_recovering_for_run<V: Venue>(c: &Connection, venue: &V, bot: &mut model::Bot, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>, cx: &TickContext<'_>) -> Result<TickOutcome, EngineError> {
    let bot_id = bot.id;
    bot.validate_times()?;
    crate::figures::fill::validate_row_times(c,bot_id).map_err(|e|EngineError::Data(format!("{e:?}")))?;
    let handle=crate::venue::Handle::new(venue,cx.credential_version.clone());
    let venue=&handle;
    drain_amount_limit_stops(c, bot_id, clock.now(), Some(bot))?;
    let outcome = match tick_inner(c, venue, bot, clock, attempts, recovered, cx).await {
        Err(EngineError::CredentialsChanged) => retry_credentials(c, bot, clock.now(), attempts),
        other => other,
    };
    let outcome = match outcome {
        Err(EngineError::RunFenced) => { *attempts = Attempts::default(); return Ok(TickOutcome::Skipped); }
        other => other,
    };
    if let Err(EngineError::Lease(_) | EngineError::Store(_)) = outcome { return outcome; }
    let drain = drain_amount_limit_stops(c, bot_id, clock.now(), Some(bot));
    finish_tick(bot_id, outcome, drain)
}

/// The Bot::StopJobs a tick's sweep enqueued run after the run, whatever its outcome (polling::apply_committed counted
/// them), unless the engine itself can no longer write. When the tick and the stops both fail, both errors are reported:
/// an error that ends the engine wins, and the other is logged.
pub fn end_of_tick(c: &Connection, bot_id: i64, now: DateTime<Utc>, outcome: Result<TickOutcome, EngineError>) -> Result<TickOutcome, EngineError> {
    if let Err(EngineError::Lease(_) | EngineError::Store(_)) = outcome { return outcome; }
    finish_tick(bot_id, outcome, run_pending_amount_limit_stops(c, bot_id, now))
}

fn finish_tick(bot_id: i64, outcome: Result<TickOutcome, EngineError>, drain: Result<(), EngineError>) -> Result<TickOutcome, EngineError> {
    match (outcome, drain) {
        (outcome, Ok(())) => outcome,
        (Ok(_), Err(drain)) => Err(drain),
        (Err(tick), Err(drain @ (EngineError::Lease(_) | EngineError::Store(_)))) => {
            super::log(&format!("[engine] bot {bot_id}: the tick failed: {tick:?}"));
            Err(drain)
        }
        (Err(tick), Err(drain)) => {
            super::log(&format!("[engine] bot {bot_id}: the amount-limit stops after a failed tick also failed: {drain:?}"));
            Err(tick)
        }
    }
}

async fn tick_inner<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &mut model::Bot, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>, cx: &TickContext<'_>) -> Result<TickOutcome, EngineError> {
    let bot_id = bot.id;
    model::require_run(c, bot)?;
    // An intent is settled whatever the status: a bot stopped after an ambiguous send still owns that order.
    let checkpoint_after_recovery = placement::pending_requires_checkpoint(bot)?;
    let settled = match placement::recover_since(c, venue, bot, clock, cx.process_start).await? {
        Recovery::Pending => return Ok(TickOutcome::AwaitingReconciliation),
        Recovery::Recorded(tx) => { *recovered = Some(tx); true }
        Recovery::NotPlaced => true,
        Recovery::NoIntent => false,
    };
    // A stop requested while the venue answered the lookup: the intent is settled; nothing new starts.
    if (cx.stopping)() { return Ok(TickOutcome::Skipped); }
    model::require_run(c, bot)?;
    if !matches!(bot.status, BotStatus::Scheduled | BotStatus::Retrying) { return Ok(TickOutcome::Skipped); }
    // The settled intent was a failed run's order, and Bot::ActionJob runs the next order of a failed run at
    // next_interval_checkpoint_at (action_job.rb:193, :272, :325-328). So the tick ends here, for Placed and NotPlaced alike,
    // one asset or a basket's leg k: what was not bought is owed again at the next checkpoint, as in Rails.
    if settled {
        *attempts = Attempts::default();
        return Ok(if checkpoint_after_recovery { TickOutcome::Rescheduled } else { TickOutcome::Skipped });
    }
    // A reversal's cancel not recorded yet (run::run_reversals failed this pass): the run is not claimed, so the bot stays due
    // and trades on the engine's next pass once the cancels are recorded, on schedule as Rails would.
    if super::reversal::barred(bot) {
        super::log(&format!("[engine] bot {bot_id}: waits for a reversal's cancels to be recorded; ticks on the next pass"));
        return Ok(TickOutcome::Skipped);
    }
    // Decide nothing from reference data past its bound.
    if let Some(s) = staleness::stale(c, bot, clock.now())? { return Ok(TickOutcome::Stale { source: s.source, message: s.message }); }

    // Bot::ActionJob#perform asks the market before it writes anything (action_job.rb:91-100): on a venue with market hours,
    // for a bot that is not all crypto (Exchanges::Alpaca#market_open?). The clock fails closed (clock.rs).
    if venue.rules().market_hours && !model::all_crypto(c, bot)? {
        let clock_result=venue.clock_result().await;
        let clock_origin=clock_result.origin().clone();
        match market::gate(clock_result.value, &model::exchange_name(c, bot)?, clock.now()) {
            Gate::Open => {}
            Gate::Closed { next_open, details } => { *attempts = Attempts::default(); return model::run_credential_write(c, bot, &clock_origin, |tx| park(tx, bot, next_open, details, clock.now())); }
            Gate::Retry(m) => return handle_failure(c, bot, Fail::Transient(m), clock, attempts, venue.rules(), &cx.credential_version),
            Gate::InvalidKey(m) => {
                let fail = Fail::General { errors: vec![m.clone()], message: m, failed_row: false };
                return handle_failure(c, bot, fail, clock, attempts, venue.rules(), &cx.credential_version);
            }
        }
    }

    let now = clock.now();
    let next_tick = bot.tick_generation().checked_add(1).ok_or_else(||EngineError::Data("tick generation overflow".into()))?;
    // What the claim replaces, so a tick that must wait for a ledger refresh can hand the checkpoint back (unclaim).
    let claimed_over = model::run_credential_write(c, bot, &cx.credential_version, |tx| {
        let prior = model::load_bot(tx, bot_id)?;
        if bot.transient.get("waiting_for_market_open").is_some_and(|v| !v.is_null() && *v != Value::Bool(false)) {
            model::merge_transient_compact(tx, bot_id, &[("waiting_for_market_open", Value::Null)])?;
        }
        let writes = vec![("last_action_job_at", json!(iso8601_ms(now))), ("tick_generation", json!(next_tick))];
        placement::remove_wait(tx, Some(bot_id))?;
        model::update_transient(tx, bot_id, &writes, now)?;
        Ok(Claimed { status: prior.status, last_action_job_at: prior.transient.get("last_action_job_at").cloned(), defer: prior.transient.get("rust_defer_until").cloned() })
    })?;
    bot.transient["last_action_job_at"] = json!(iso8601_ms(now));
    bot.transient["tick_generation"] = json!(next_tick);

    // Anything that goes wrong inside execute_action fails this bot's tick the way a StandardError does in
    // Rails (retrying, execution_failed, next checkpoint) — it never leaves the bot `executing`.
    let executed = match execute(c, venue, bot, clock, cx).await {
        Ok(r) => r,
        Err(e @ (EngineError::Lease(_) | EngineError::Store(_) | EngineError::CredentialsChanged | EngineError::RunFenced)) => return Err(e),
        Err(e) => Err(Fail::General { message: format!("{e:?}"), errors: vec![], failed_row: false }),
    };
    let outcome = match executed {
        Ok(Ran { fenced: true, .. }) => { *attempts = Attempts::default(); TickOutcome::Skipped }
        Ok(Ran { ledger_stale: Some(stale), .. }) => unclaim(c, bot, claimed_over, *stale, clock.now(), &cx.credential_version)?,
        Ok(Ran { placed, .. }) => {
            *attempts = Attempts::default();
            // clear_failure_state!, then back to scheduled unless stopped meanwhile.
            model::run_credential_write(c, bot, &cx.credential_version, |tx| {
                if model::load_bot(tx, bot_id)?.last_failure_kind().is_some() { record_failure(tx, bot_id, None)?; }
                // The starting time only places the first run; it turns off before the transition, as in action_job.rb:139.
                amount::disable_starting_time(tx, bot_id, clock.now())?;
                Ok(if model::transition_working(tx, bot_id, BotStatus::Scheduled, clock.now())? { TickOutcome::Done { placed } } else { TickOutcome::Skipped })
            })?
        }
        Err(fail) => {
            handle_failure(c, bot, fail, clock, attempts, venue.rules(), &cx.credential_version)?
        }
    };
    Ok(outcome)
}

/// DcaMultiAsset#execute_action with the sweep in front and Fundable behind. Ok(placed) = success.
async fn execute<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Result<Ran, Fail>, EngineError> {
    let bot_id = bot.id;
    model::require_run(c, bot)?;
    // Bot::Rebalanceable's first stand-down: a swap in flight owes its buy the proceeds of its sell, so the DCA leg places
    // nothing, reads nothing and sweeps nothing (rebalanceable.rb:64-68). Success: the contribution is carried by the interval
    // count.
    if super::rebalance::is_pending(&bot.transient) {
        let now = clock.now();
        model::run_write(c, bot, |tx| model::log_activity(tx, bot_id, "dca_skipped_rebalance_pending", Level::Info, json!({}), now))?;
        return Ok(Ok(Ran { placed: false, fenced: false, ledger_stale: None }));
    }
    // Bot::Rebalanceable's stand-down, the outermost decorator of execute_action (rebalanceable.rb:75-82): a restatement the
    // market has not priced yet, or a split the bot cannot size. No sweep, no order, no balance read; success, so the bot goes
    // back to scheduled, and the contribution is carried by the interval count.
    if splits::untrusted(c, bot, clock.now())? {
        let now = clock.now();
        model::run_write(c, bot, |tx| model::log_activity(tx, bot_id, "dca_skipped_restatement", Level::Info, json!({}), now))?;
        return Ok(Ok(Ran { placed: false, fenced: false, ledger_stale: None }));
    }

    if let Err(f) = polling::sweep(c, venue, bot, clock.now()).await {
        return Ok(Err(match f {
            PollFailure::CredentialsChanged => Fail::CredentialsChanged,
            PollFailure::RateLimited(m) => Fail::RateLimited(m),
            PollFailure::Transient(m) => Fail::Transient(m),
            PollFailure::General(m) => Fail::General { errors: vec![m.clone()], message: m, failed_row: false },
        }));
    }
    model::require_run(c, bot)?;
    let reconciled = splits::snapshot(c, bot)?;
    if let Some(reason) = splits::refusal(c, venue, bot, clock.now()).await? {
        model::run_credential_write(c, bot, &cx.credential_version, |tx| {
            model::merge_transient_compact(tx, bot_id, &[("rust_split_hold", json!({"reason":reason,"at":iso8601_ms(clock.now())}))])?;
            model::log_activity(tx, bot_id, "dca_skipped_restatement", Level::Info, json!({"reason":reason}), clock.now())
        })?;
        super::log(&format!("[engine] bot {bot_id}: split reconciliation required: {reason}; refresh the ledger and correct its split records before restarting; no trade is required"));
        return Ok(Ok(Ran { placed: false, fenced: false, ledger_stale: None }));
    }
    if splits::snapshot(c, bot)? != reconciled {
        let now = clock.now();
        model::run_write(c, bot, |tx| model::log_activity(tx, bot_id, "dca_skipped_restatement", Level::Info, json!({"reason":"split generation or held assets changed during reconciliation"}), now))?;
        return Ok(Ok(Ran { placed: false, fenced: false, ledger_stale: None }));
    }
    model::run_write(c, bot, |tx| { tx.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.rust_split_hold') WHERE id=?1 AND json_extract(transient_data,'$.rust_split_hold') IS NOT NULL", [bot_id])?; Ok(()) })?;
    // Statusable#transition_working!: every status write of the tick moves only a still-working bot, so a stop that
    // lands meanwhile (during the sweep, or while AddOrder awaits its reply) wins. Stopped before executing, nothing
    // is placed and Fundable is skipped.
    let first_tick = own_rows(c, bot)?.is_empty();
    model::transition_owned(c, bot, BotStatus::Executing, clock.now())?;
    let mut claimed = bot.clone();
    claimed.status = BotStatus::Executing;
    let bot = &claimed;
    let (mut placed, mut fenced) = (false, false);
    {
        // Bots::DcaMultiAsset#refresh_composition: the members are re-derived and written before anything is sized; with no
        // tradable weighted member the tick fails with no order row.
        // The composition this tick buys: an index bot derives it from data-api's ranking with price probes (index.rs), a basket
        // from its settings (basket.rs). A Failure fails the tick with no order row; a probe's transport failure retries.
        let refreshed = if bot.bot_type == "Bots::DcaIndex" {
            index::refresh_composition_captured(c, venue, bot, clock, cx.prices, &cx.credential_version).await?
        } else {
            let now = clock.now();
            model::run_write(c, bot, |tx| basket::refresh_composition(tx, bot, now))?.map_err(index::Refusal::Failure)
        };
        match refreshed {
            Ok(()) => {}
            Err(index::Refusal::Failure(m)) => return Ok(Err(Fail::General { message: m, errors: vec![], failed_row: false })),
            Err(index::Refusal::Transient(m)) => return Ok(Err(Fail::Transient(m))),
        }
        // Selling: what the sell sentence says, never pending_quote_amount (the frozen buy carry would place a buy).
        let sale = bot.selling().then(|| bot.sell_amounts());
        let mut x = match &sale {
            Some((base, quote)) => base.clone().unwrap_or_else(|| quote.clone()),
            None => amount::pending_quote_amount(c, bot, clock.now().timestamp_micros())?,
        };
        // validate_orders_amount!: a negative amount raises a plain StandardError.
        if x < BigDec::zero() { let m = "Orders quote_amount must be positive".to_string(); return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false })); }
        // Bot::QuoteAmountLimitable decorates pending_quote_amount: `[super, available].min`. The cap applies to the carry,
        // so the last order is cut to what is left, never skipped for exceeding it; nothing left is no order, no row, no log.
        if sale.is_none() { if let Some(available) = amount::quote_amount_available(c, bot)? { if available < x { x = available; } } }
        if !x.is_zero() {
            let mut legs = Legs::default();
            let bought = match sale {
                Some((base, quote)) => sell(c, venue, bot, base, &quote, clock, cx, &mut legs, &reconciled).await,
                None => buy(c, venue, bot, &x, clock, cx, &mut legs, &reconciled).await,
            };
            // set_orders' `ensure record_skipped_orders!`: on every way out of the loop, a raise included.
            let bought = match bought {
                Err(e @ (EngineError::CredentialsChanged | EngineError::RunFenced)) => return Err(e),
                other => other,
            };
            // A tick that waits for a ledger refresh sizes every leg again, so its below-minimum legs are reported then.
            if legs.ledger_stale.is_some() && !legs.fenced {
                return Ok(Ok(Ran { placed: legs.placed, fenced: false, ledger_stale: legs.ledger_stale }));
            }
            if !legs.skipped.is_empty() {
                model::run_credential_write(c, bot, &cx.credential_version, |tx| record_skipped(tx, bot, &legs.skipped, legs.placed, clock.now()))?;
            }
            (placed, fenced) = (legs.placed, legs.fenced);
            if let Err(f) = bought? { return Ok(Err(f)); }
        }
        // A stop that landed while AddOrder awaited its reply stays; the orders already sent stand and are still polled.
        // Fenced, the run is no longer this tick's: no :waiting over a restarted run's claim, no below-minimum notice.
        if fenced { return Err(EngineError::RunFenced); }
        model::transition_owned(c, bot, BotStatus::Waiting, clock.now())?;
        if first_tick && !fenced {
            model::run_write(c, bot, |tx| {
                let rows = own_rows(tx, bot)?;
                if !rows.is_empty() && rows.iter().all(|(_, status)| *status == 2) {
                    (cx.below_minimum)(bot_id, rows.into_iter().map(|(id, _)| id).collect());
                }
                Ok(())
            })?;
        }
    }
    // Bot::Fundable#funds_are_low?: the quote asset's balance, whatever the member tickers (an index bot has no allocations),
    // spent as Exchanges::Alpaca#spendable_balance picks it. A failed balance Result means "not low"; only a transport
    // failure raises.
    // While selling the user spends base, not quote: no quote-buffer check and no balance read (fundable.rb:10-12).
    if bot.selling() { return Ok(Ok(Ran { placed, fenced, ledger_stale: None })); }
    model::require_run(c, bot)?;
    let funds=venue.balance_result(&model::quote_symbol(c, bot)?, model::all_crypto(c, bot)?).await;
    model::require_run(c, bot)?;
    let funds_origin=funds.origin().clone();
    match funds.value {
        Ok(free) => {
            let interval_seconds = match bot.interval().map(|i| i.as_str()) { Some("hour") => 3_600.0, Some("day") => 86_400.0, Some("week") => 604_800.0, _ => 2_629_746.0 };
            let buffer = super::accounting::balance_buffer(bot, interval_seconds, THREE_DAYS)?;
            if free < buffer && !notified_in_last_day(c, bot, clock)? {
                // `update!(last_end_of_funds_notification:)` then notify_end_of_funds.
                model::run_credential_write(c, bot, &funds_origin, |tx| stamp_funds_low(tx, bot, clock.now(), true))?;
            }
        }
        // Clients::Alpaca raises a transport failure out of Bot::Fundable's balance read. Bot::ActionJob then refuses to
        // replay a run that already placed an order (retrying, rescheduled), and retries one that placed nothing.
        Err(VenueError::Transient(m)) if venue.rules().transport_raises => {
            return Ok(Err(if placed { Fail::TransientAfterPlacement } else { Fail::Transient(m) }));
        }
        // honeymaker's with_rescue turns a network failure into a Failure result: "not low", the tick succeeds.
        Err(VenueError::Rejected(_) | VenueError::Transient(_)) => {}
        // An unreadable body raises a plain StandardError in Rails: execution_failed, no retry.
        Err(VenueError::Ambiguous(m)) => return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false })),
    }
    Ok(Ok(Ran { placed, fenced, ledger_stale: None }))
}

/// What a tick's execute_action did. A fenced tick skips Fundable and all completion writes. `ledger_stale`: a leg was
/// refused because the account ledger stopped being fresh after it was sized; the tick hands its checkpoint back (unclaim).
struct Ran { placed: bool, fenced: bool, ledger_stale: Option<Box<staleness::Stale>> }

/// What a basket buy did before it returned, for the skipped-leg report that runs on every way out.
#[derive(Default)]
struct Legs { placed: bool, fenced: bool, skipped: Vec<amount::OrderPlan>, ledger_stale: Option<Box<staleness::Stale>> }

/// The values a tick's claim replaced.
struct Claimed { status: BotStatus, last_action_job_at: Option<Value>, defer: Option<Value> }

/// A leg was refused because the account ledger needs a complete refresh after the basket was sized: typically another bot's
/// order on the same key woke `ledger_sync` while this tick was running (sync::jobs::LedgerSync::wants). Rails has no such
/// fence and would have bought, so this tick must not consume the checkpoint: last_action_job_at and the wait go back to what
/// the claim found, the bot goes back to the status it had, and the outcome is `Stale`, as when the tick-start check
/// refuses. The bot stays due. The scheduler wakes the engine when a ledger sync records a success (jobs::Scheduler::record),
/// and the next tick re-sizes from the refreshed ledger; what this tick placed counts as spent in pending_quote_amount, so
/// nothing is bought twice. While the sync is still running, or if it failed, the tick-start check refuses (Stale) and
/// rechecks every run::STALE_RECHECK_US, until a sync completes.
fn unclaim(c: &Connection, bot: &mut model::Bot, claimed: Claimed, stale: staleness::Stale, now: DateTime<Utc>, version: &Option<model::CredentialVersion>) -> Result<TickOutcome, EngineError> {
    let back = model::run_credential_write(c, bot, version, |tx| {
        for (key, value) in [("last_action_job_at", &claimed.last_action_job_at), ("rust_defer_until", &claimed.defer)] {
            let path = format!("$.{key}");
            match value {
                Some(v) => tx.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3", params![path, v.to_string(), bot.id])?,
                None => tx.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?1) WHERE id = ?2", params![path, bot.id])?,
            };
        }
        model::transition_working(tx, bot.id, claimed.status, now)
    })?;
    match &claimed.last_action_job_at { Some(v) => bot.transient["last_action_job_at"] = v.clone(), None => { bot.transient.as_object_mut().map(|t| t.remove("last_action_job_at")); } }
    Ok(if back { TickOutcome::Stale { source: stale.source, message: stale.message } } else { TickOutcome::Skipped })
}

/// Bot::Composition::OrderSetter#set_orders(side: :buy) over get_orders_data: holdings, every member's price before any
/// order (Step 1), the split, then the legs in member order. Each leg is settled (row written, intent cleared) before the next
/// is sent, and the first leg that is not accepted ends the loop (order_setter.rb:80); legs never sent stay owed through
/// pending_quote_amount. A price failure can only come before the first leg.
#[allow(clippy::too_many_arguments)]
async fn buy<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, x: &BigDec, clock: &dyn Clock, cx: &TickContext<'_>, legs: &mut Legs, reconciled: &splits::Snapshot) -> Result<Result<(), Fail>, EngineError> {
    let composition = placement::composition_snapshot(c, bot)?;
    let members = basket::members(c, bot)?;
    if members.is_empty() {
        let m = "No assets in composition".to_string();
        return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
    }
    let holdings = model::locked(c, |tx| {
        if splits::snapshot(tx, bot)? != *reconciled { return Ok(None); }
        Ok(Some(basket::walk(tx, bot, clock.now())?.amounts))
    })?;
    let Some(holdings) = holdings else { return Ok(Ok(())); };
    let reserved = basket::reserved(c, bot)?;
    // Bot::OrderSetter#reference_price: the last trade for a limit buy, the ask for a market buy.
    let side = if bot.limit_distance().is_some() { PriceSide::Last } else { PriceSide::Ask };
    let mut priced = vec![];
    for member in members {
        let reference = match reference(venue, bot, &member.ticker, side, &member.ticker.base_symbol, clock, cx).await {
            Ok(p) => p,
            Err(f) => return Ok(Err(f)),
        };
        let price = amount::order_price(bot, &member.ticker, &reference)?;
        priced.push(basket::Priced { member, reference, price });
    }
    let split = match basket::split(&priced, &holdings, &reserved, x) {
        Ok(split) => split,
        Err(decimals) => {
            let m = format!("limit price rounds to zero at {decimals} decimals");
            return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
        }
    };
    let tickers: Vec<_> = priced.iter().map(|p| &p.member.ticker).collect();
    for leg in split {
        match amount::size(bot, &leg.ticker, &leg.quote, &leg.reference, venue.rules().minimum_logic)? {
            Sizing::Nothing => {}
            Sizing::Ignored(plan) => { let now = clock.now(); model::run_write(c, bot, |tx| model::log_activity(tx, bot.id, "order_ignored", Level::Info, plan.log_details(), now))?; }
            Sizing::BelowMinimum(plan) => legs.skipped.push(plan), // reported once, at the end (record_skipped)
            Sizing::ZeroPrice { decimals } => {
                let m = format!("limit price rounds to zero at {decimals} decimals");
                return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
            }
            Sizing::Place(plan) => match place(c, venue, bot, &plan, &tickers, &composition, reconciled, clock, cx, legs).await? {
                Placed::Next => {}
                Placed::Stop => break,
                Placed::Failed(f) => return Ok(Err(f)),
            },
        }
    }
    Ok(Ok(()))
}

/// What placing one sized order did to the leg loop.
enum Placed { Next, Stop, Failed(Fail) }

/// One sized order through the intent and the venue: set_orders' create_order and its result handling, for a buy leg or a sell.
#[allow(clippy::too_many_arguments)]
async fn place<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, plan: &amount::OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &splits::Snapshot, clock: &dyn Clock, cx: &TickContext<'_>, legs: &mut Legs) -> Result<Placed, EngineError> {
    // A stop or a composition edit always wins over a tick in progress: an order already sent stands, and nothing
    // sized before the change is placed (the fence is in the intent's own transaction).
    let intent = match placement::begin_with_credentials(c, bot, plan, tickers, composition, reconciled, &cx.credential_version, venue.placement_identity(), clock)? {
        placement::Begun::Intent(intent) => intent,
        placement::Begun::Changed => return Ok(Placed::Stop),
        placement::Begun::Fenced => { legs.fenced = true; return Ok(Placed::Stop); }
        placement::Begun::CredentialsChanged => return Ok(Placed::Failed(Fail::CredentialsChanged)),
        placement::Begun::BelowMinimum(skipped) => { legs.skipped.push(*skipped); return Ok(Placed::Next); }
        placement::Begun::LedgerStale(stale) => { legs.ledger_stale = Some(stale); return Ok(Placed::Stop); }
    };
    Ok(match placement::send_recorded(c, venue, bot, &intent, clock).await? {
        Sent::Accepted(_) => { legs.placed = true; Placed::Next }
        Sent::Fenced => { legs.fenced = true; Placed::Stop }
        Sent::Rejected(errs) => {
            let row = !placement::safe_rejection(&errs);
            let m = to_sentence(&errs);
            Placed::Failed(if row { Fail::General { message: m, errors: errs, failed_row: true } } else { Fail::PlacementSafe(m) })
        }
        Sent::Ambiguous(m) => Placed::Failed(Fail::Ambiguous(m)), // the intent stays; later legs wait for the next checkpoint
        // Nothing reached the venue. After a placed leg, Bot::ActionJob's already-placed guard refuses the replay
        // (action_job.rb:223-235); before one, retry_on replays the whole tick.
        Sent::NotSent(m) => Placed::Failed(if legs.placed { Fail::TransientAfterPlacement } else { Fail::Transient(m) }),
    })
}

/// Bot::OrderSetter#reference_price through Exchange's 5-second price cache: the venue's "Wrong … price" (a zero book) and
/// a transport failure both fail the tick as the composition rescues them ("No price for …").
/// `named`: how the failure names the asset (the buy leg its symbol, the sell leg `ticker.base`, the venue's own code).
async fn reference<V: Venue + Attributed>(venue: &V, bot: &model::Bot, ticker: &model::Ticker, side: PriceSide, named: &str, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<BigDec, Fail> {
    let key = (bot.exchange_id, ticker.id, side);
    let fetched = match cx.prices.get(key, clock.now(), &cx.credential_version) {
        Some(hit) => Ok(hit),
        // Only a usable price is stored (the venue returns a zero book as an error).
        None => { let result=venue.price_result(ticker,side).await; if let Ok(price)=&result.value { cx.prices.put_result(key,clock.now(),result.clone().map(|_|price.clone())); } result.value },
    };
    match fetched {
        Ok(p) => Ok(p),
        // A zero book is the venue's own "Wrong … price" error, which the composition rescues into "No price for …".
        Err(VenueError::Rejected(e)) => Err(Fail::Transient(format!("No price for {named}: {}", to_sentence(&e)))),
        // An in-app client raises the transport failure itself (Client.network_failure); the composition re-raises it
        // unwrapped, so it reaches retry_on with its own message.
        Err(VenueError::Transient(m)) if venue.rules().transport_raises => Err(Fail::Transient(m)),
        Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => Err(Fail::Transient(format!("No price for {named}: {m}"))),
    }
}

/// Bot::Rebalancer#live_free_balance(asset_id) for one of `ticker`'s assets: the venue's free balance as #get_balances reads
/// it (the quote is settled cash, the base its free units through the venue's asset identity). A base this bot also bought
/// on another venue (a merge) is capped at #local_units: the venue may sell in its name only what it bought there. `Err`
/// is the venue's failed read, never an empty balance.
/// Exchange#market_price_for(ticker:, side:): the bid for a sell, the ask for a buy, through the same 5-second price cache as
/// every other price read, then Ticker#adjusted_price (floored to price_decimals). None for a failed read or a price that is
/// not positive (Bot::Rebalancer#side_price's nil).
pub(crate) async fn market_price_for<V: Venue + Attributed>(venue: &V, bot: &model::Bot, ticker: &model::Ticker, side: crate::venue::Side, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Option<BigDec>, EngineError> {
    let price_side = if side == crate::venue::Side::Sell { PriceSide::Bid } else { PriceSide::Ask };
    let Ok(price) = reference(venue, bot, ticker, price_side, &ticker.base_code, clock, cx).await else { return Ok(None) };
    let (_, _, price_decimals) = ticker.scales().map_err(EngineError::Data)?;
    Ok(Some(price.floor(price_decimals)).filter(BigDec::is_positive))
}

pub async fn live_free_balance<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, ticker: &model::Ticker, asset_id: i64) -> Result<Result<BigDec, VenueError>, EngineError> {
    let catalog = || crate::sync::balances::catalog_for(c, bot.user_id, bot.exchange_id).map_err(|e| VenueError::Rejected(vec![e.0]));
    let read = if asset_id == ticker.quote_asset_id { venue.settled_quote_result(ticker, catalog).await } else { venue.free_base_result(ticker, catalog).await };
    let free = match read.value { Ok(free) => free, Err(e) => return Ok(Err(e)) };
    if Some(asset_id) == bot.quote_asset_id() || !foreign_rows(c, bot, asset_id)? { return Ok(Ok(free)); }
    let local = local_units(c, bot, asset_id)?;
    Ok(Ok(if local < free { local } else { free }))
}

/// Bot::Rebalancer#foreign_rows?: a submitted row of this asset placed on another venue (only a merge writes one).
fn foreign_rows(c: &Connection, bot: &model::Bot, asset_id: i64) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM transactions WHERE bot_id = ?1 AND status = 0 AND base_asset_id = ?2 AND exchange_id != ?3)",
                   params![bot.id, asset_id, bot.exchange_id], |r| r.get(0))?)
}

/// Bot::Rebalancer::UNSPLITTABLE_CATEGORIES: unit counts no corporate action restates.
const UNSPLITTABLE_CATEGORIES: [&str; 4] = ["Cryptocurrency", "Currency", "Fiat", "Stablecoin"];

/// Bot::Rebalancer#local_units: executed buys minus executed sells of the asset on this bot's own venue, each read by
/// Transaction.confirmed_exec_amounts (a closed row with no executed figure counts its ordered amount), never below 0. An
/// asset that could have split (a share, a tokenized one) has none: a sum of fills is not what is held.
fn local_units(c: &Connection, bot: &model::Bot, asset_id: i64) -> Result<BigDec, EngineError> {
    let category: Option<String> = c.query_row("SELECT category FROM assets WHERE id = ?1", [asset_id], |r| r.get(0)).optional()?.flatten();
    if !category.is_some_and(|k| UNSPLITTABLE_CATEGORIES.contains(&k.as_str())) { return Ok(BigDec::zero()); }
    let dec = |v: rusqlite::types::ValueRef<'_>| crate::ruby::from_sql(v).map_err(|e| EngineError::Data(format!("{e:?}")));
    let mut s = c.prepare("SELECT side, external_status, amount, amount_exec FROM transactions WHERE bot_id = ?1 AND status = 0 AND exchange_id = ?2 AND base_asset_id = ?3")?;
    let mut rows = s.query(params![bot.id, bot.exchange_id, asset_id])?;
    let mut net = BigDec::zero();
    while let Some(r) = rows.next()? {
        let (side, external_status): (Option<i64>, Option<i64>) = (r.get(0)?, r.get(1)?);
        let (amount, amount_exec) = (dec(r.get_ref(2)?)?, dec(r.get_ref(3)?)?);
        let executed = if external_status == Some(2) { amount_exec.or(amount) } else { amount_exec }.unwrap_or_else(BigDec::zero);
        net = if side == Some(1) { &net - &executed } else { &net + &executed };
    }
    Ok(net.max(BigDec::zero()))
}

/// A failed live_free_balance as Bot::Rebalancer raises it: throttled, transient, else a plain error.
fn balance_failure<V: Venue>(venue: &V, bot: &model::Bot, e: VenueError) -> Fail {
    match e {
        VenueError::Rejected(e) if venue.rules().is_throttle(&e) => Fail::RateLimited(to_sentence(&e)),
        VenueError::Transient(m) => Fail::Transient(m),
        // Exchange#transient_failure?'s text rules (Kraken's HTTP-200 "EGeneral:Internal error").
        VenueError::Rejected(e) if venue.rules().is_transient(&e) => Fail::Transient(to_sentence(&e)),
        VenueError::Rejected(e) => {
            let m = format!("Failed to read balance for bot {}: {}", bot.id, to_sentence(&e));
            Fail::General { errors: vec![m.clone()], message: m, failed_row: false }
        }
        VenueError::Ambiguous(m) => {
            let m = format!("Failed to read balance for bot {}: {m}", bot.id);
            Fail::General { errors: vec![m.clone()], message: m, failed_row: false }
        }
    }
}

/// Bot::Composition::OrderSetter#one_asset_sell_orders_data, then set_orders for its one order. `base`: "sell N base" (the
/// per-tick base), else `quote` is sold at the order price. Up to the whole free wallet (owner ruling Q2): the venue's free
/// base, never 0 for an unreadable answer. Nothing free is a skip with a log line only (`sell_skipped no_holdings`).
#[allow(clippy::too_many_arguments)]
async fn sell<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, base: Option<BigDec>, quote: &BigDec, clock: &dyn Clock, cx: &TickContext<'_>, legs: &mut Legs, reconciled: &splits::Snapshot) -> Result<Result<(), Fail>, EngineError> {
    let composition = placement::composition_snapshot(c, bot)?;
    // Bots::DcaMultiAsset#one_asset?: the configured members, not the tradeable ones.
    if !bot.settings.as_object().is_some_and(super::schedule::one_asset) {
        // Bots::DcaMultiAsset#sell_denomination is 'quote' for a wider basket (sell_amounts reads it so): base is None here.
        return basket_sell(c, venue, bot, quote, &composition, clock, cx, legs, reconciled).await;
    }
    let Some(member) = basket::members(c, bot)?.into_iter().next() else {
        let m = "No assets in composition".to_string();
        return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
    };
    let ticker = member.ticker;
    // Bot::OrderSetter#reference_price(ticker, :sell): the last trade for a limit sell, the bid for a market sell.
    let reference = match reference(venue, bot, &ticker, if bot.limit_distance().is_some() { PriceSide::Last } else { PriceSide::Bid }, &ticker.base_code, clock, cx).await {
        Ok(p) => p,
        Err(f) => return Ok(Err(f)),
    };
    let price = amount::side_price(bot, &ticker, &reference, crate::venue::Side::Sell)?;
    let wanted = match base {
        Some(base) => base,
        // `quote / price`: BigDecimal#/ by zero raises ZeroDivisionError, a plain StandardError.
        None => match quote.div(&price) {
            Some(q) => q,
            None => { let m = "divided by 0".to_string(); return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false })); }
        },
    };
    let free = match live_free_balance(c, venue, bot, &ticker, ticker.base_asset_id).await? {
        Ok(free) => free,
        Err(e) => return Ok(Err(balance_failure(venue, bot, e))),
    };
    model::require_run(c, bot)?;
    if !free.is_positive() {
        super::log(&format!("[engine] bot {}: set_orders composition event=sell_skipped reason=no_holdings", bot.id));
        return Ok(Ok(()));
    }
    let amount = if free < wanted { free } else { wanted };
    match amount::size_sell(bot, &ticker, &amount, &price)? {
        Sizing::Nothing => {}
        Sizing::Ignored(plan) => { let now = clock.now(); model::run_write(c, bot, |tx| model::log_activity(tx, bot.id, "order_ignored", Level::Info, plan.log_details(), now))?; }
        Sizing::BelowMinimum(plan) => legs.skipped.push(plan),
        Sizing::ZeroPrice { decimals } => {
            let m = format!("limit price rounds to zero at {decimals} decimals");
            return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
        }
        Sizing::Place(plan) => if let Placed::Failed(f) = place(c, venue, bot, &plan, &[&ticker], &composition, reconciled, clock, cx, legs).await? { return Ok(Err(f)); },
    }
    Ok(Ok(()))
}

/// Bot::Composition::OrderSetter#get_sell_orders_data(withdrawal), then set_orders over its orders. The withdrawal comes out
/// of the member furthest above its share of what is left after it, valued fresh (composition_targets, force) net of its
/// own resting sells; each order is `min(remaining / price, held net of resting sells)`, under the venue floor it hands
/// over to the next-ranked member, then capped at live_free_balance and floored again. Every order is sized before any is
/// sent. Nothing cleared a floor: the first member's attempt is reported once (a skipped row). No holdings is a logged
/// skip; holdings with no valuation retry before anything is placed.
#[allow(clippy::too_many_arguments)]
async fn basket_sell<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &model::Bot, withdrawal: &BigDec, composition: &Value, clock: &dyn Clock, cx: &TickContext<'_>, legs: &mut Legs, reconciled: &splits::Snapshot) -> Result<Result<(), Fail>, EngineError> {
    let general = |m: String| Fail::General { errors: vec![m.clone()], message: m, failed_row: false };
    let (walked, entries) = match super::rebalance::value(c, venue, bot, clock.now()).await? {
        Ok(valued) => valued,
        Err(super::rebalance::ValueFail::Transient(m)) => return Ok(Err(Fail::Transient(m))),
        Err(super::rebalance::ValueFail::Raised(m)) => return Ok(Err(general(m))),
    };
    let dec = |n: &crate::figures::num::Num| n.to_d().map_err(|e| EngineError::Data(format!("{e:?}"))).and_then(|d| BigDec::parse(&d.to_s_f()).map_err(|e| EngineError::Data(format!("{e:?}"))));
    // holdings.dig(key_for(asset_id, fresh), :amount).to_d
    let held_of = |asset: i64| -> Result<BigDec, EngineError> {
        match walked.key_for(asset).and_then(|k| walked.asset_breakdown.iter().find(|(x, _)| x == k)) { Some((_, b)) => dec(&b.amount), None => Ok(BigDec::zero()) }
    };
    if !walked.asset_breakdown.iter().any(|(_, b)| b.amount.is_positive()) {
        super::log(&format!("[engine] bot {}: set_orders composition event=sell_skipped reason=no_holdings", bot.id));
        return Ok(Ok(()));
    }
    let Some(mut entries) = entries else { return Ok(Err(Fail::Transient(format!("No valuation for bot {}'s holdings", bot.id)))) };
    let resting = basket::reserved_sells(c, bot)?;
    let (zero, one) = (BigDec::zero(), BigDec::one());
    for e in &mut entries {
        let pending = resting.get(&e.asset_id).unwrap_or(&zero);
        if !(pending.is_positive() && e.amount.is_positive()) { continue; }
        let share = pending.div(&e.amount).expect("the amount is positive");
        e.value = &e.value * &(&one - &(if share < one { share } else { one.clone() }));
    }
    let side = if bot.limit_distance().is_some() { PriceSide::Last } else { PriceSide::Bid };
    let below = |s: Sizing| matches!(s, Sizing::BelowMinimum(_));
    let mut remaining = withdrawal.clone();
    let (mut orders, mut below_floor) = (vec![], None);
    for i in super::rebalance::sell_ranking(&entries, withdrawal) {
        if !remaining.is_positive() { break; }
        let ticker = entries[i].ticker.clone().expect("a ranked entry is tradeable");
        let held = &held_of(ticker.base_asset_id)? - resting.get(&ticker.base_asset_id).unwrap_or(&zero);
        if !held.is_positive() { continue; }
        let reference = match reference(venue, bot, &ticker, side, &ticker.base_code, clock, cx).await { Ok(p) => p, Err(f) => return Ok(Err(f)) };
        let price = amount::side_price(bot, &ticker, &reference, crate::venue::Side::Sell)?;
        // `remaining / price`: BigDecimal#/ by zero raises ZeroDivisionError, a plain StandardError.
        let Some(wanted) = remaining.div(&price) else { return Ok(Err(general("divided by 0".into()))) };
        let amount = if held < wanted { held } else { wanted };
        if below(amount::size_sell(bot, &ticker, &amount, &price)?) { below_floor.get_or_insert((ticker, amount, price)); continue; }
        let free = match live_free_balance(c, venue, bot, &ticker, ticker.base_asset_id).await? { Ok(free) => free, Err(e) => return Ok(Err(balance_failure(venue, bot, e))) };
        model::require_run(c, bot)?;
        if !free.is_positive() { continue; }
        let amount = if free < amount { free } else { amount };
        if below(amount::size_sell(bot, &ticker, &amount, &price)?) { below_floor.get_or_insert((ticker, amount, price)); continue; }
        remaining = &remaining - &(&amount * &price);
        orders.push((ticker, amount, price));
    }
    if orders.is_empty() { orders.extend(below_floor); }
    let tickers: Vec<&model::Ticker> = entries.iter().filter_map(|e| e.ticker.as_ref()).collect();
    for (ticker, amount, price) in orders {
        match amount::size_sell(bot, &ticker, &amount, &price)? {
            Sizing::Nothing => {}
            Sizing::Ignored(plan) => { let now = clock.now(); model::run_write(c, bot, |tx| model::log_activity(tx, bot.id, "order_ignored", Level::Info, plan.log_details(), now))?; }
            Sizing::BelowMinimum(plan) => legs.skipped.push(plan),
            Sizing::ZeroPrice { decimals } => return Ok(Err(general(format!("limit price rounds to zero at {decimals} decimals")))),
            Sizing::Place(plan) => match place(c, venue, bot, &plan, &tickers, composition, reconciled, clock, cx, legs).await? {
                Placed::Next => {}
                Placed::Stop => break,
                Placed::Failed(f) => return Ok(Err(f)),
            },
        }
    }
    Ok(Ok(()))
}

/// Bot::Composition::OrderSetter#record_skipped_orders!: legs under the venue minimum are reported once. If anything was
/// placed, one `orders_below_minimum` line. Otherwise each gets an `order_skipped` warning and a `skipped` row.
fn record_skipped(c: &model::FencedTransaction<'_>, bot: &model::Bot, skipped: &[amount::OrderPlan], placed_any: bool, now: DateTime<Utc>) -> Result<(), EngineError> {
    if skipped.is_empty() { return Ok(()); }
    if placed_any {
        let bases: Vec<&str> = skipped.iter().map(|p| p.ticker.base_code.as_str()).collect();
        return model::log_activity(c, bot.id, "orders_below_minimum", Level::Info, json!({ "count": skipped.len(), "bases": bases.join(", ") }), now);
    }
    for plan in skipped {
        model::log_activity(c, bot.id, "order_skipped", Level::Warning, plan.log_details(), now)?;
        amount::write_order_row(c, bot, plan, RowKind::Skipped, now)?;
    }
    Ok(())
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

fn retry_credentials(c: &Connection, bot: &model::Bot, now: DateTime<Utc>, attempts: &mut Attempts) -> Result<TickOutcome, EngineError> {
    model::run_write(c, bot, |tx| {
        model::transition_owned(tx, bot, BotStatus::Scheduled, now)?;
        placement::run_now(tx, bot, now)?;
        Ok(())
    })?;
    *attempts = Attempts::default();
    Ok(TickOutcome::Skipped)
}
fn handle_failure(c: &Connection, bot: &model::Bot, fail: Fail, clock: &dyn Clock, attempts: &mut Attempts, rules: &VenueRules, version: &Option<model::CredentialVersion>) -> Result<TickOutcome, EngineError> {
    if matches!(fail, Fail::CredentialsChanged) { return retry_credentials(c, bot, clock.now(), attempts); }
    model::run_credential_write(c, bot, version, |tx| {
        let bot_id = bot.id;
        let outcome = handle_failure_inner(tx, bot_id, fail, clock, attempts, rules)?;
        if matches!(outcome, TickOutcome::Rescheduled) {
            placement::defer_failure(tx, &model::load_bot(tx, bot_id)?, clock.now())?;
        }
        Ok(outcome)
    })
}

fn handle_failure_inner(c: &model::FencedTransaction<'_>, bot_id: i64, fail: Fail, clock: &dyn Clock, attempts: &mut Attempts, rules: &VenueRules) -> Result<TickOutcome, EngineError> {
    let now = clock.now();
    if !model::transition_working(c, bot_id, BotStatus::Retrying, now)? { return Ok(TickOutcome::Skipped); }
    match fail {
        Fail::CredentialsChanged => Ok(TickOutcome::Skipped),
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
            // One transaction: the budget, the mail it owes and the log line, or none of them.
            let tx = c;
            record_notified_failure(tx, bot_id, kind, &m, Exit::Exhausted, clock)?;
            model::log_activity(tx, bot_id, "execution_failed", Level::Error, json!({ "error": m, "kind": kind, "rate_limited_exhausted": true }), now)?;
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
        Fail::TransientAfterPlacement => {
            *attempts = Attempts::default();
            record_failure(c, bot_id, Some("transient"))?;
            Ok(TickOutcome::Rescheduled)
        }
        Fail::General { message, errors, failed_row } => {
            *attempts = Attempts::default();
            let kind = rules.failure_kind(if errors.is_empty() { std::slice::from_ref(&message) } else { &errors });
            // One transaction: the budget, the stop where there is one, the mail owed and the log lines, or none of them.
            // A failure in the middle leaves the bot as the tick found it: nothing spent, nothing owed, not stopped.
            let tx = c;
            let prior=model::load_bot(tx,bot_id)?;
            let state=model::Produced::new((),model::CredentialVersion::from_stamp(&prior.transient["rust_failure_origin"]));
            let previous=if state.current_for(&tx.producer_version()).is_fresh(){prior.last_failure_kind()}else{None};
            let blocking = kind.is_some_and(|k| BLOCKING_KINDS.contains(&k) && previous.as_deref() == Some(k));
            record_notified_failure(tx, bot_id, kind, &message, if blocking { Exit::Blocking } else { Exit::Recoverable }, clock)?;
            if !failed_row {
                model::log_activity(tx, bot_id, "execution_failed", Level::Error, json!({ "error": message, "kind": kind }), now)?;
            }
            if let (true, Some(kind)) = (blocking, kind) {
                // stop_for_blocking_failure: Bot::StopJob, then notify_stopped_by_error.
                stop_owing_mail(tx, bot_id, &format!("bot.status.stopped_by_error.{kind}"), Some((notice::STOPPED, notice::stopped_marker(&message, now))), now)?;
                    return Ok(TickOutcome::Stopped);
            }
            Ok(TickOutcome::Rescheduled)
        }
    }
}

/// Bot::ActionJob's closed path (action_job.rb:93-100): `update!(waiting_for_market_open: true)`, which saves nothing (and
/// moves no updated_at) when the key already holds true; `market_closed` with next_market_open_at; the next run at next_open.
/// No sweep, no last_action_job_at, no status change, no price.
fn park(c: &model::FencedTransaction<'_>, bot: &model::Bot, next_open: DateTime<Utc>, details: String, now: DateTime<Utc>) -> Result<TickOutcome, EngineError> {
    if bot.transient.get("waiting_for_market_open") != Some(&json!(true)) {
        model::update_transient(c, bot.id, &[("waiting_for_market_open", json!(true))], now)?;
    }
    model::log_activity(c, bot.id, "market_closed", Level::Info, json!({ "next_market_open_at": details }), now)?;
    Ok(TickOutcome::MarketClosed { until: next_open, producer:c.producer_version() })
}

/// Bot::Composition::OrderSetter#own_transactions: IDs strictly after the inherited cutoff.
/// Capture emptiness before execution, then require all own rows to be skipped before notifying.
fn own_rows(c: &Connection, bot: &model::Bot) -> Result<Vec<(i64,i64)>, EngineError> {
    let cutoff = model::merged_history_cutoff(c, bot)?;
    let mut s = c.prepare("SELECT id,status FROM transactions WHERE bot_id=?1 AND (?2 IS NULL OR id>?2) ORDER BY id")?;
    let rows = s.query_map(params![bot.id,cutoff], |r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    Ok(rows)
}
