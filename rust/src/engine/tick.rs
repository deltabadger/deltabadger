//! One tick of an eligible bot: Bot::ActionJob#perform around DcaMultiAsset#execute_action (with the
//! Fundable and LimitOrderable decorators), and the failure handling of ActionJob's rescues.
use super::amount::{self, RowKind, Sizing};
use super::venue_rules::VenueRules;
use super::model::{self, Level};
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

/// ActiveJob's exception_executions, one counter per retry_on handler.
#[derive(Debug, Default, Clone, Copy)]
pub struct Attempts { pub transient: u32, pub rate: u32 }

#[derive(Debug)]
pub enum TickOutcome {
    Skipped, Done { placed: bool }, RetryAfter(Duration), Rescheduled, Stopped, AwaitingReconciliation,
    /// Reference data only a Rails job refreshes is past its bound (staleness.rs): nothing was read from the venue, nothing
    /// written, and the bot stays due.
    Stale { source: &'static str, message: String },
}

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
    /// A transport failure after this run already placed an order (Bot::ActionJob's TransientNetworkError rescue with a
    /// submitted row since action_started_at): never retried, because the replay would place a second order.
    TransientAfterPlacement,
}

type PriceKey = (i64, i64, PriceSide);

/// Exchange#get_{ask,last}_price's Rails.cache entry (exchanges/alpaca.rb:273-335, exchanges/kraken.rb:218-260): kept
/// 5 s from when it was written, keyed by exchange, ticker and side; only a usable (non-zero) price is stored. It lives
/// in the engine (and across a parity scenario's retries), never in a venue instance, so it survives venue recreation.
#[derive(Default)]
pub struct PriceCache(std::cell::RefCell<std::collections::HashMap<PriceKey, (chrono::DateTime<chrono::Utc>, BigDec)>>);

impl PriceCache {
    pub fn get(&self, key: PriceKey, now: chrono::DateTime<chrono::Utc>) -> Option<BigDec> {
        // ActiveSupport::Cache::Entry#expired?: created_at + expires_in <= now.
        self.0.borrow().get(&key).filter(|(at, _)| now < *at + chrono::Duration::seconds(5)).map(|(_, p)| p.clone())
    }
    pub fn put(&self, key: PriceKey, now: chrono::DateTime<chrono::Utc>, price: BigDec) { self.0.borrow_mut().insert(key, (now, price)); }
}

/// What a tick borrows from the engine that runs it.
pub struct TickContext<'a> {
    pub prices: &'a PriceCache,
    /// When this process started: Alpaca absence is trusted only a full margin (20 min) after it (placement::recover_since).
    pub process_start: chrono::DateTime<chrono::Utc>,
    /// A stop was requested (the engine's Shutdown); checked between recovery and execution.
    pub stopping: &'a dyn Fn() -> bool,
}

fn record_failure(c: &Connection, bot_id: i64, kind: Option<&str>) -> Result<(), EngineError> {
    model::merge_transient_compact(c, bot_id, &[("last_failure_kind", kind.map(Value::from).unwrap_or(Value::Null))])
}

pub async fn tick<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts) -> Result<TickOutcome, EngineError> {
    let prices = PriceCache::default();
    let cx = TickContext { prices: &prices, process_start: chrono::DateTime::<chrono::Utc>::MIN_UTC, stopping: &|| false };
    tick_recovering(c, venue, bot_id, clock, attempts, &mut None, &cx).await
}

/// `tick`, also reporting the transaction a persisted intent was settled into this tick (its row carries the intent's
/// earlier `created_at`, so the caller cannot find it by time and must queue its follow-up poll itself).
pub async fn tick_recovering<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, attempts: &mut Attempts, recovered: &mut Option<i64>, cx: &TickContext<'_>) -> Result<TickOutcome, EngineError> {
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

    // The checkpoint a settled intent deferred to (placement::defer_to_next_checkpoint) has come: this is that tick.
    c.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_defer_until') \
               WHERE id = ?1 AND json_extract(transient_data, '$.rust_defer_until') IS NOT NULL", [bot_id])?;
    let now = clock.now();
    // Rails' store_accessor writes a key only when the value changes, so `waiting_for_market_open: nil` touches the
    // key only where it holds something non-null; an absent key stays absent.
    let mut writes = vec![("last_action_job_at", json!(iso8601_ms(now)))];
    if bot.transient.get("waiting_for_market_open").is_some_and(|v| !v.is_null()) { writes.push(("waiting_for_market_open", Value::Null)); }
    model::update_transient(c, bot_id, &writes, now)?;

    // Anything that goes wrong inside execute_action fails this bot's tick the way a StandardError does in
    // Rails (retrying, execution_failed, next checkpoint) — it never leaves the bot `executing`.
    let executed = match execute(c, venue, bot_id, clock, cx).await {
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
async fn execute<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Result<bool, Fail>, EngineError> {
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
        // Bots::DcaMultiAsset#refresh_composition: the members are re-derived and written before anything is sized; with no
        // tradable weighted member the tick fails with no order row.
        if let Err(m) = basket::refresh_composition(c, &bot, clock.now())? {
            return Ok(Err(Fail::General { message: m, errors: vec![], failed_row: false }));
        }
        let x = amount::pending_quote_amount(c, &bot, clock.now().timestamp_micros())?;
        if !x.is_zero() {
            let mut legs = Legs::default();
            let bought = buy(c, venue, &bot, &x, clock, cx, &mut legs).await;
            // set_orders' `ensure record_skipped_orders!`: on every way out of the loop, a raise included.
            record_skipped(c, &bot, &legs.skipped, legs.placed, clock.now())?;
            placed = legs.placed;
            if let Err(f) = bought? { return Ok(Err(f)); }
        }
        // A stop that landed while AddOrder awaited its reply stays; the orders already sent stand and are still polled.
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
async fn buy<V: Venue>(c: &Connection, venue: &V, bot: &model::Bot, x: &BigDec, clock: &dyn Clock, cx: &TickContext<'_>, legs: &mut Legs) -> Result<Result<(), Fail>, EngineError> {
    let members = basket::members(c, bot)?;
    if members.is_empty() {
        let m = "No assets in composition".to_string();
        return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
    }
    let holdings = basket::holdings(c, bot)?;
    let reserved = basket::reserved(c, bot)?;
    // Bot::OrderSetter#reference_price: the last trade for a limit buy, the ask for a market buy.
    let side = if bot.limit_distance().is_some() { PriceSide::Last } else { PriceSide::Ask };
    let mut priced = vec![];
    for member in members {
        let key = (bot.exchange_id, member.ticker.id, side);
        let fetched = match cx.prices.get(key, clock.now()) {
            Some(hit) => Ok(hit),
            // Only a usable price is stored (the venue returns a zero book as an error).
            None => venue.price(&member.ticker, side).await.inspect(|p| cx.prices.put(key, clock.now(), p.clone())),
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
        let price = amount::order_price(bot, &member.ticker, &reference);
        priced.push(basket::Priced { member, reference, price });
    }
    let split = match basket::split(&priced, &holdings, &reserved, x) {
        Ok(split) => split,
        Err(decimals) => {
            let m = format!("limit price rounds to zero at {decimals} decimals");
            return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
        }
    };
    for leg in split {
        match amount::size(bot, &leg.ticker, &leg.quote, &leg.reference, venue.rules().minimum_logic) {
            Sizing::Nothing => {}
            Sizing::Ignored(plan) => model::log_activity(c, bot.id, "order_ignored", Level::Info, plan.log_details(), clock.now())?,
            Sizing::BelowMinimum(plan) => legs.skipped.push(plan), // reported once, at the end (record_skipped)
            Sizing::ZeroPrice { decimals } => {
                let m = format!("limit price rounds to zero at {decimals} decimals");
                return Ok(Err(Fail::General { errors: vec![m.clone()], message: m, failed_row: false }));
            }
            Sizing::Place(plan) => {
                let intent = placement::begin(c, bot, &plan, clock)?;
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
fn record_skipped(c: &Connection, bot: &model::Bot, skipped: &[amount::OrderPlan], placed_any: bool, now: DateTime<Utc>) -> Result<(), EngineError> {
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
        Fail::TransientAfterPlacement => {
            *attempts = Attempts::default();
            record_failure(c, bot_id, Some("transient"))?;
            Ok(TickOutcome::Rescheduled)
        }
        Fail::General { message, errors, failed_row } => {
            *attempts = Attempts::default();
            let kind = rules.failure_kind(if errors.is_empty() { std::slice::from_ref(&message) } else { &errors });
            let previous = model::load_bot(c, bot_id)?.last_failure_kind();
            let blocking = kind.is_some_and(|k| BLOCKING_KINDS.contains(&k) && previous.as_deref() == Some(k));
            // Bot::Failable#record_failure!(kind, notified:): a buy-side insufficient-funds failure shares Bot::Fundable's daily
            // budget (#notified_in_last_day?, per user and quote asset); when it notifies, it stamps last_end_of_funds_notification
            // with update_column, so updated_at does not move.
            if kind == Some("insufficient_funds") {
                let bot = model::load_bot(c, bot_id)?;
                if !notified_in_last_day(c, &bot, clock)? {
                    c.execute("UPDATE bots SET last_end_of_funds_notification = ?1 WHERE id = ?2", params![format_time(now), bot_id])?;
                }
            }
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
