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
use rusqlite::{params, Connection};
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
    let (path, marker) = match &mail { Some((key, marker)) => (format!("$.{key}"), marker.to_string()), None => ("$".into(), String::new()) };
    model::locked(c, |c| {
        // Without a mail the CASE leaves transient_data exactly as stored.
        let n = c.execute(&format!("UPDATE bots SET status = ?1, stopped_at = ?2, stop_message_key = ?3, updated_at = ?2, \
                                    transient_data = CASE WHEN ?5 = '$' THEN transient_data ELSE json_set(transient_data, ?5, json(?6)) END \
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
    loop {
        let tx = model::immediate(c)?;
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
    let shared = kind == Some("insufficient_funds");
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
    let prices = PriceCache::default();
    let cx = TickContext { credential_version: model::credential_version(c, &model::load_bot(c, bot_id)?)?, prices: &prices, process_start: chrono::DateTime::<chrono::Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {} };
    tick_recovering(c, venue, bot_id, clock, attempts, &mut None, &cx).await
}

/// `tick`, also reporting the transaction a persisted intent was settled into this tick (its row carries the intent's
/// earlier `created_at`, so the caller cannot find it by time and must queue its follow-up poll itself).
pub async fn tick_recovering<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>, cx: &TickContext<'_>) -> Result<TickOutcome, EngineError> {
    // R9 refuses malformed stored times before polling, bookkeeping or any persisted write.
    model::load_bot(c,bot_id)?.validate_times()?;
    crate::figures::fill::validate_row_times(c,bot_id).map_err(|e|EngineError::Data(format!("{e:?}")))?;
    let handle=crate::venue::Handle::new(venue,cx.credential_version.clone());
    let venue=&handle;
    // A stop an earlier tick counted but could not run (an error outlasted it): Rails' Bot::StopJob ran long before now.
    run_pending_amount_limit_stops(c, bot_id, clock.now())?;
    let outcome = match tick_inner(c, venue, bot_id, clock, attempts, recovered, cx).await {
        Err(EngineError::CredentialsChanged) => retry_credentials(c, bot_id, clock.now(), attempts),
        other => other,
    };
    end_of_tick(c, bot_id, clock.now(), outcome)
}

/// The Bot::StopJobs a tick's sweep enqueued run after the run, whatever its outcome (polling::apply_committed counted
/// them), unless the engine itself can no longer write. When the tick and the stops both fail, both errors are reported:
/// an error that ends the engine wins, and the other is logged.
pub fn end_of_tick(c: &Connection, bot_id: i64, now: DateTime<Utc>, outcome: Result<TickOutcome, EngineError>) -> Result<TickOutcome, EngineError> {
    if let Err(EngineError::Lease(_) | EngineError::Store(_)) = outcome { return outcome; }
    match (outcome, run_pending_amount_limit_stops(c, bot_id, now)) {
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

async fn tick_inner<V: Venue + Attributed>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>, cx: &TickContext<'_>) -> Result<TickOutcome, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    // An intent is settled whatever the status: a bot stopped after an ambiguous send still owns that order.
    let settled = match placement::recover_since(c, venue, &bot, clock, cx.process_start).await? {
        Recovery::Pending => return Ok(TickOutcome::AwaitingReconciliation),
        Recovery::Recorded(tx) => { *recovered = Some(tx); true }
        Recovery::NotPlaced => true,
        Recovery::NoIntent => false,
    };
    // A stop requested while the venue answered the lookup: the intent is settled; nothing new starts.
    if (cx.stopping)() { return Ok(TickOutcome::Skipped); }
    let bot = model::load_bot(c, bot_id)?;
    if !matches!(bot.status, BotStatus::Scheduled | BotStatus::Retrying) { return Ok(TickOutcome::Skipped); }
    // The settled intent was a failed run's order, and Bot::ActionJob runs the next order of a failed run at
    // next_interval_checkpoint_at (action_job.rb:193, :272, :325-328). So the tick ends here, for Placed and NotPlaced alike,
    // one asset or a basket's leg k: what was not bought is owed again at the next checkpoint, as in Rails.
    if settled {
        *attempts = Attempts::default();
        return Ok(TickOutcome::Rescheduled);
    }
    // Decide nothing from reference data past its bound.
    if let Some(s) = staleness::stale(c, &bot, clock.now())? { return Ok(TickOutcome::Stale { source: s.source, message: s.message }); }

    // Bot::ActionJob#perform asks the market before it writes anything (action_job.rb:91-100): on a venue with market hours,
    // for a bot that is not all crypto (Exchanges::Alpaca#market_open?). The clock fails closed (clock.rs).
    if venue.rules().market_hours && !model::all_crypto(c, &bot)? {
        let clock_result=venue.clock_result().await;
        let clock_origin=clock_result.origin().clone();
        match market::gate(clock_result.value, &model::exchange_name(c, &bot)?, clock.now()) {
            Gate::Open => {}
            Gate::Closed { next_open, details } => { *attempts = Attempts::default(); return model::credential_write(c, &clock_origin, |tx| park(tx, &bot, next_open, details, clock.now())); }
            Gate::Retry(m) => return handle_failure(c, bot_id, Fail::Transient(m), clock, attempts, venue.rules(), &cx.credential_version),
            Gate::InvalidKey(m) => {
                let fail = Fail::General { errors: vec![m.clone()], message: m, failed_row: false };
                return handle_failure(c, bot_id, fail, clock, attempts, venue.rules(), &cx.credential_version);
            }
        }
    }

    let now = clock.now();
    model::credential_write(c, &cx.credential_version, |tx| {
        if bot.transient.get("waiting_for_market_open").is_some_and(|v| !v.is_null() && *v != Value::Bool(false)) {
            model::merge_transient_compact(tx, bot_id, &[("waiting_for_market_open", Value::Null)])?;
        }
        let writes = vec![("last_action_job_at", json!(iso8601_ms(now)))];
        placement::remove_wait(tx, Some(bot_id))?;
        model::update_transient(tx, bot_id, &writes, now)
    })?;

    // Anything that goes wrong inside execute_action fails this bot's tick the way a StandardError does in
    // Rails (retrying, execution_failed, next checkpoint) — it never leaves the bot `executing`.
    let executed = match execute(c, venue, bot_id, clock, cx).await {
        Ok(r) => r,
        Err(e @ (EngineError::Lease(_) | EngineError::Store(_) | EngineError::CredentialsChanged)) => return Err(e),
        Err(e) => Err(Fail::General { message: format!("{e:?}"), errors: vec![], failed_row: false }),
    };
    let outcome = match executed {
        Ok(placed) => {
            *attempts = Attempts::default();
            // clear_failure_state!, then back to scheduled unless stopped meanwhile.
            model::credential_write(c, &cx.credential_version, |tx| {
                if model::load_bot(tx, bot_id)?.last_failure_kind().is_some() { record_failure(tx, bot_id, None)?; }
                Ok(if model::transition_working(tx, bot_id, BotStatus::Scheduled, clock.now())? { TickOutcome::Done { placed } } else { TickOutcome::Skipped })
            })?
        }
        Err(fail) => {
            handle_failure(c, bot_id, fail, clock, attempts, venue.rules(), &cx.credential_version)?
        }
    };
    Ok(outcome)
}

/// DcaMultiAsset#execute_action with the sweep in front and Fundable behind. Ok(placed) = success.
async fn execute<V: Venue + Attributed>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Result<bool, Fail>, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    // Bot::Rebalanceable's stand-down, the outermost decorator of execute_action (rebalanceable.rb:75-82): a restatement the
    // market has not priced yet, or a split the bot cannot size. No sweep, no order, no balance read; success, so the bot goes
    // back to scheduled, and the contribution is carried by the interval count.
    if splits::untrusted(c, &bot, clock.now())? {
        model::log_activity(c, bot_id, "dca_skipped_restatement", Level::Info, json!({}), clock.now())?;
        return Ok(Ok(false));
    }

    if let Err(f) = polling::sweep(c, venue, &bot, clock.now()).await {
        return Ok(Err(match f {
            PollFailure::CredentialsChanged => Fail::CredentialsChanged,
            PollFailure::RateLimited(m) => Fail::RateLimited(m),
            PollFailure::Transient(m) => Fail::Transient(m),
            PollFailure::General(m) => Fail::General { errors: vec![m.clone()], message: m, failed_row: false },
        }));
    }
    let bot = model::load_bot(c, bot_id)?;
    let reconciled = splits::snapshot(c, &bot)?;
    if let Some(reason) = splits::refusal(c, venue, &bot, clock.now()).await? {
        model::credential_write(c, &cx.credential_version, |tx| {
            model::merge_transient_compact(tx, bot_id, &[("rust_split_hold", json!({"reason":reason,"at":iso8601_ms(clock.now())}))])?;
            model::log_activity(tx, bot_id, "dca_skipped_restatement", Level::Info, json!({"reason":reason}), clock.now())
        })?;
        super::log(&format!("[engine] bot {bot_id}: split reconciliation required: {reason}; refresh the ledger and correct its split records before restarting; no trade is required"));
        return Ok(Ok(false));
    }
    if splits::snapshot(c, &bot)? != reconciled {
        model::log_activity(c, bot_id, "dca_skipped_restatement", Level::Info, json!({"reason":"split generation or held assets changed during reconciliation"}), clock.now())?;
        return Ok(Ok(false));
    }
    c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.rust_split_hold') WHERE id=?1 AND json_extract(transient_data,'$.rust_split_hold') IS NOT NULL", [bot_id])?;
    // Statusable#transition_working!: every status write of the tick moves only a still-working bot, so a stop that
    // lands meanwhile (during the sweep, or while AddOrder awaits its reply) wins. Stopped before executing, nothing
    // is placed, but Fundable still reads the balance as it wraps execute_action.
    let first_tick = own_rows(c, &bot)?.is_empty();
    let working = model::transition_working(c, bot_id, BotStatus::Executing, clock.now())?;
    let bot = model::load_bot(c, bot_id)?;
    let mut placed = false;
    if working {
        // Bots::DcaMultiAsset#refresh_composition: the members are re-derived and written before anything is sized; with no
        // tradable weighted member the tick fails with no order row.
        // The composition this tick buys: an index bot derives it from data-api's ranking with price probes (index.rs), a basket
        // from its settings (basket.rs). A Failure fails the tick with no order row; a probe's transport failure retries.
        let refreshed = if bot.bot_type == "Bots::DcaIndex" {
            index::refresh_composition_captured(c, venue, &bot, clock, cx.prices, &cx.credential_version).await?
        } else {
            basket::refresh_composition(c, &bot, clock.now())?.map_err(index::Refusal::Failure)
        };
        match refreshed {
            Ok(()) => {}
            Err(index::Refusal::Failure(m)) => return Ok(Err(Fail::General { message: m, errors: vec![], failed_row: false })),
            Err(index::Refusal::Transient(m)) => return Ok(Err(Fail::Transient(m))),
        }
        let mut x = amount::pending_quote_amount(c, &bot, clock.now().timestamp_micros())?;
        // Bot::QuoteAmountLimitable decorates pending_quote_amount: `[super, available].min`. The cap applies to the carry,
        // so the last order is cut to what is left, never skipped for exceeding it; nothing left is no order, no row, no log.
        if let Some(available) = amount::quote_amount_available(c, &bot)? { if available < x { x = available; } }
        if !x.is_zero() {
            let mut legs = Legs::default();
            let bought = buy(c, venue, &bot, &x, clock, cx, &mut legs, &reconciled).await;
            // set_orders' `ensure record_skipped_orders!`: on every way out of the loop, a raise included.
            if matches!(bought, Err(EngineError::CredentialsChanged)) { return Err(EngineError::CredentialsChanged); }
            if !legs.skipped.is_empty() {
                model::credential_write(c, &cx.credential_version, |tx| record_skipped(tx, &bot, &legs.skipped, legs.placed, clock.now()))?;
            }
            placed = legs.placed;
            if let Err(f) = bought? { return Ok(Err(f)); }
        }
        // A stop that landed while AddOrder awaited its reply stays; the orders already sent stand and are still polled.
        model::transition_working(c, bot_id, BotStatus::Waiting, clock.now())?;
        if first_tick {
            let rows = own_rows(c, &bot)?;
            if !rows.is_empty() && rows.iter().all(|(_, status)| *status == 2) {
                (cx.below_minimum)(bot_id, rows.into_iter().map(|(id, _)| id).collect());
            }
        }
    }
    // Bot::Fundable#funds_are_low?: the quote asset's balance, whatever the member tickers (an index bot has no allocations),
    // spent as Exchanges::Alpaca#spendable_balance picks it. A failed balance Result means "not low"; only a transport
    // failure raises.
    let funds=venue.balance_result(&model::quote_symbol(c, &bot)?, model::all_crypto(c, &bot)?).await;
    let funds_origin=funds.origin().clone();
    match funds.value {
        Ok(free) => {
            let interval_seconds = match bot.interval().map(|i| i.as_str()) { Some("hour") => 3_600.0, Some("day") => 86_400.0, Some("week") => 604_800.0, _ => 2_629_746.0 };
            let buffer = super::accounting::balance_buffer(&bot, interval_seconds, THREE_DAYS)?;
            if free < buffer && !notified_in_last_day(c, &bot, clock)? {
                // `update!(last_end_of_funds_notification:)` then notify_end_of_funds.
                model::credential_write(c, &funds_origin, |tx| stamp_funds_low(tx, &bot, clock.now(), true))?;
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
    Ok(Ok(placed))
}

/// What a basket buy did before it returned, for the skipped-leg report that runs on every way out.
#[derive(Default)]
struct Legs { placed: bool, skipped: Vec<amount::OrderPlan> }

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
        let key = (bot.exchange_id, member.ticker.id, side);
        let fetched = match cx.prices.get(key, clock.now(), &cx.credential_version) {
            Some(hit) => Ok(hit),
            // Only a usable price is stored (the venue returns a zero book as an error).
            None => { let result=venue.price_result(&member.ticker,side).await; if let Ok(price)=&result.value { cx.prices.put_result(key,clock.now(),result.clone().map(|_|price.clone())); } result.value },
        };
        let reference = match fetched {
            Ok(p) => p,
            // A zero book is the venue's own "Wrong … price" error, which the composition rescues into "No price for …".
            Err(VenueError::Rejected(e)) => return Ok(Err(Fail::Transient(format!("No price for {}: {}", member.ticker.base_symbol, to_sentence(&e))))),
            // An in-app client raises the transport failure itself (Client.network_failure); the composition re-raises it
            // unwrapped, so it reaches retry_on with its own message.
            Err(VenueError::Transient(m)) if venue.rules().transport_raises => return Ok(Err(Fail::Transient(m))),
            Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => return Ok(Err(Fail::Transient(format!("No price for {}: {m}", member.ticker.base_symbol)))),
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
            Sizing::Ignored(plan) => model::log_activity(c, bot.id, "order_ignored", Level::Info, plan.log_details(), clock.now())?,
            Sizing::BelowMinimum(plan) => legs.skipped.push(plan), // reported once, at the end (record_skipped)
            Sizing::ZeroPrice { decimals } => {
                let m = format!("limit price rounds to zero at {decimals} decimals");
                return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
            }
            Sizing::Place(plan) => {
                // A stop or a composition edit always wins over a tick in progress: an order already sent stands, and nothing
                // sized before the change is placed (the fence is in the intent's own transaction).
                let intent = match placement::begin_with_credentials(c, bot, &plan, &tickers, &composition, reconciled, &cx.credential_version, clock)? {
                    placement::Begun::Intent(intent) => intent,
                    placement::Begun::Changed => break,
                    placement::Begun::CredentialsChanged => return Ok(Err(Fail::CredentialsChanged)),
                    placement::Begun::BelowMinimum(skipped) => { legs.skipped.push(skipped); continue; }
                };
                match placement::send(venue, &intent, clock).await {
                    Sent::Accepted(txid) => { placement::record_accepted(c, bot, &intent, &txid)?; legs.placed = true; }
                    Sent::Rejected(errs) => {
                        let row = placement::record_rejected(c, bot, &intent, &errs)?;
                        let m = to_sentence(&errs);
                        return Ok(Err(if row { Fail::General { message: m, errors: errs, failed_row: true } } else { Fail::PlacementSafe(m) }));
                    }
                    Sent::Ambiguous(m) => return Ok(Err(Fail::Ambiguous(m))), // the intent stays; later legs wait for the next checkpoint
                    // Nothing reached the venue. After a placed leg, Bot::ActionJob's already-placed guard refuses the replay
                    // (action_job.rb:223-235); before one, retry_on replays the whole tick.
                    Sent::NotSent(m) => {
                        placement::drop_intent(c, bot.id)?;
                        return Ok(Err(if legs.placed { Fail::TransientAfterPlacement } else { Fail::Transient(m) }));
                    }
                }
            }
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

fn retry_credentials(c: &Connection, bot_id: i64, now: DateTime<Utc>, attempts: &mut Attempts) -> Result<TickOutcome, EngineError> {
    model::locked(c, |tx| {
        if model::transition_working(tx, bot_id, BotStatus::Scheduled, now)? {
            placement::run_now(tx, &model::load_bot(tx, bot_id)?, now)?;
        }
        Ok(())
    })?;
    *attempts = Attempts::default();
    Ok(TickOutcome::Skipped)
}
fn handle_failure(c: &Connection, bot_id: i64, fail: Fail, clock: &dyn Clock, attempts: &mut Attempts, rules: &VenueRules, version: &Option<model::CredentialVersion>) -> Result<TickOutcome, EngineError> {
    if matches!(fail, Fail::CredentialsChanged) { return retry_credentials(c, bot_id, clock.now(), attempts); }
    model::credential_write(c, version, |tx| {
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
