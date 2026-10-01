//! The engine loop. No job table: each pass re-reads the working bots (UI changes are seen within a minute),
//! ticks the due ones, and sleeps until the earliest next event. What Rails would hold in Solid Queue —
//! follow-up polls, retries — is kept in memory and rebuilt from the database on start.
use super::schedule::{checkpoints, effective};
use super::tick::{self, Attempts, PriceCache, TickContext, TickOutcome};
use super::{eligibility, model, placement, polling, Clock, EngineError};
use crate::crypto::Cipher;
use crate::lease::EngineLock;
use crate::venue::VenueFactory;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::collections::HashMap;
use std::convert::Infallible;

const IDLE_US: i64 = 60_000_000;
const AFTER_CHECKPOINT_US: i64 = 1_000;
const POLL_AFTER_US: i64 = 5_000_000;
const RECONCILE_EVERY_US: i64 = 30_000_000;

pub struct Engine<F: VenueFactory> {
    pub primary: Connection, pub factory: F, pub cipher: Cipher, pub lock: EngineLock,
    started: bool,
    attempts: HashMap<i64, Attempts>,
    retry_at: HashMap<i64, i64>,
    /// Follow-up polls owed, one per ORDER (FetchAndUpdateOrderJob): tx id → (bot, due time, that job's own retry counters).
    polls: HashMap<i64, (i64, i64, Attempts)>,
    reconcile_at: HashMap<i64, i64>,
    /// Rails' 5 s price cache (Rails.cache), shared by every tick of this process.
    prices: PriceCache,
    /// Set by the first `step`: an Alpaca absence is trusted only a full window after it (placement::recover_since).
    process_start: Option<DateTime<Utc>>,
}

impl<F: VenueFactory> Engine<F> {
    pub fn new(primary: Connection, factory: F, cipher: Cipher, lock: EngineLock) -> Self {
        Self { primary, factory, cipher, lock, started: false, attempts: HashMap::new(), retry_at: HashMap::new(), polls: HashMap::new(), reconcile_at: HashMap::new(),
               prices: PriceCache::default(), process_start: None }
    }
    #[doc(hidden)] pub fn inject_stale_retry(&mut self, bot: i64, at_us: i64) { self.retry_at.insert(bot, at_us); }
    fn venue_for(&self, bot: &model::Bot) -> Result<F::V, EngineError> {
        Ok(self.factory.for_bot(&model::exchange_type(&self.primary, bot)?, model::credentials_for(&self.primary, &self.cipher, bot)?))
    }
}

/// Every order still unknown/open, whatever its bot's status (Rails' adopt_handback! polls them all): (tx, bot).
fn outstanding_orders(c: &Connection) -> Result<Vec<(i64, i64)>, EngineError> {
    let mut s = c.prepare("SELECT id, bot_id FROM transactions WHERE status = 0 AND external_status IN (0, 1) AND bot_id IS NOT NULL ORDER BY id")?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<(i64, i64)>, _>>()?;
    Ok(rows)
}

/// Bots outside the working set that still hold a placement intent (stopped after an ambiguous send). No tick
/// runs for them, so the loop reconciles them itself, every 30 s until settled.
fn idle_bots_with_intents(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare(&format!(
        "SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL AND status NOT IN ({}) ORDER BY id",
        model::working_list()))?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}

async fn reconcile_idle<F: VenueFactory>(e: &mut Engine<F>, id: i64, clock: &dyn Clock, wake: &mut i64) -> Result<(), EngineError> {
    if e.reconcile_at.get(&id).is_some_and(|&t| t > clock.now().timestamp_micros()) { *wake = (*wake).min(e.reconcile_at[&id]); return Ok(()); }
    let bot = model::load_bot(&e.primary, id)?;
    let venue = e.venue_for(&bot)?;
    match placement::recover_since(&e.primary, &venue, &bot, clock, e.process_start.expect("set by step")).await? {
        placement::Recovery::Pending => {
            let at = clock.now().timestamp_micros() + RECONCILE_EVERY_US;
            e.reconcile_at.insert(id, at);
            *wake = (*wake).min(at);
        }
        placement::Recovery::Recorded(tx) => {
            e.reconcile_at.remove(&id);
            e.polls.insert(tx, (id, clock.now().timestamp_micros() + POLL_AFTER_US, Attempts::default()));
        }
        placement::Recovery::NotPlaced | placement::Recovery::NoIntent => { e.reconcile_at.remove(&id); }
    }
    Ok(())
}

pub async fn step<F: VenueFactory>(e: &mut Engine<F>, clock: &dyn Clock) -> Result<i64, EngineError> {
    e.process_start.get_or_insert(clock.now());
    let report = eligibility::check_install(&e.primary)?;
    if !report.problems.is_empty() { return Err(EngineError::Ineligible(report.problems)); }
    for (id, err) in &report.unreadable { eprintln!("[engine] bot {id} is unreadable and skipped: {err}"); }

    if !e.started {
        // Rebuilt obligation: every outstanding order is polled once, as Rails' adopt_handback! does.
        let now_us = clock.now().timestamp_micros();
        for (tx, bot) in outstanding_orders(&e.primary)? { e.polls.entry(tx).or_insert((bot, now_us, Attempts::default())); }
        e.started = true;
    }

    let mut wake = clock.now().timestamp_micros() + IDLE_US;
    run_polls(e, clock, &mut wake).await;
    for id in idle_bots_with_intents(&e.primary)? {
        if let Err(err) = reconcile_idle(e, id, clock, &mut wake).await {
            if matches!(err, EngineError::Lease(_) | EngineError::Store(_)) { return Err(err); }
            eprintln!("[engine] bot {id}: placement reconciliation failed: {err:?}; retrying in 30 s");
            let at = clock.now().timestamp_micros() + RECONCILE_EVERY_US;
            e.reconcile_at.insert(id, at);
            wake = wake.min(at);
        }
    }
    for id in report.eligible {
        if let Err(err) = step_bot(e, id, clock, &mut wake).await {
            // Lease/store errors end the engine. Sqlite errors stay per bot and are retried next pass; a replay after an
            // accepted order is prevented by the early last_action_job_at write and the persisted intent.
            if matches!(err, EngineError::Lease(_) | EngineError::Store(_)) { return Err(err); }
            eprintln!("[engine] bot {id}: {err:?}; retrying at the next pass");
        }
    }
    // Polls queued during this pass (a placement, a recovered intent) must pull the wake in too.
    for (_, at, _) in e.polls.values() { wake = wake.min(*at); }
    let now_us = clock.now().timestamp_micros();
    Ok(wake.max(now_us + 1))
}

/// Due follow-up polls, for any bot whatever its status (a job Rails enqueued at placement runs even if the
/// bot is stopped meanwhile). Transient/RateLimited failures retry as FetchAndUpdateOrderJob's retry_on.
async fn run_polls<F: VenueFactory>(e: &mut Engine<F>, clock: &dyn Clock, wake: &mut i64) {
    let now_us = clock.now().timestamp_micros();
    let due: Vec<(i64, i64, Attempts)> = e.polls.iter().filter(|(_, (_, at, _))| *at <= now_us).map(|(tx, (bot, _, a))| (*tx, *bot, *a)).collect();
    for (tx, id, mut attempts) in due {
        e.polls.remove(&tx);
        let venue = match model::load_bot(&e.primary, id).and_then(|bot| e.venue_for(&bot)) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("[engine] bot {id}: follow-up poll deferred 30 s: {err:?}");
                e.polls.insert(tx, (id, clock.now().timestamp_micros() + RECONCILE_EVERY_US, attempts));
                continue;
            }
        };
        let retry = match polling::follow_up(&e.primary, &venue, id, tx, clock.now()).await {
            Ok(()) => None,
            Err(polling::PollFailure::Transient(m)) => { attempts.transient += 1; (attempts.transient < 3).then(|| (tick::retry_wait(attempts.transient, false), m)) }
            Err(polling::PollFailure::RateLimited(m)) => { attempts.rate += 1; (attempts.rate < 4).then(|| (tick::retry_wait(attempts.rate, true), m)) }
            Err(polling::PollFailure::General(m)) => { eprintln!("[engine] bot {id}: follow-up poll failed: {m}"); None }
        };
        if let Some((wait, m)) = retry {
            eprintln!("[engine] bot {id}: follow-up poll failed ({m}); retrying in {}s", wait.as_secs());
            e.polls.insert(tx, (id, clock.now().timestamp_micros() + wait.as_micros() as i64, attempts));
        }
    }
    for (_, at, _) in e.polls.values() { *wake = (*wake).min(*at); }
}

async fn step_bot<F: VenueFactory>(e: &mut Engine<F>, id: i64, clock: &dyn Clock, wake: &mut i64) -> Result<(), EngineError> {
    let now_us = clock.now().timestamp_micros();
    let bot = model::load_bot(&e.primary, id)?;
    let venue = e.venue_for(&bot)?;

    let (Some(anchor), Some(interval), Some(quote)) = (bot.started_at_us, bot.interval(), bot.quote_amount()) else { return Ok(()) };
    let eff = effective(interval, quote, bot.smart_quote_amount());
    let cps = checkpoints(anchor, now_us, eff);
    let due = if bot.rust_placement().is_some() {
        e.reconcile_at.get(&id).is_none_or(|&t| t <= now_us)
    } else if bot.status == crate::enums::BotStatus::Retrying {
        e.retry_at.get(&id).is_none_or(|&t| t <= now_us) // no in-memory state (a restart): due at once
    } else {
        anchor <= now_us && bot.last_action_job_at_us()?.is_none_or(|t| t.div_euclid(1000) < cps.last_us.div_euclid(1000)) // stored value is ms-truncated
    };

    if due {
        // The loop is single-threaded: a due bot met `executing`/`waiting` was left there by an error that escaped its
        // tick, and the tick would skip it forever.
        if model::unstick(&e.primary, id, clock.now())? { eprintln!("[engine] bot {id}: left {:?} by an earlier tick; back to scheduled", bot.status); }
        let tick_start = crate::codec::format_time(clock.now());
        let attempts = e.attempts.entry(id).or_default();
        let mut recovered = None;
        let cx = TickContext { prices: &e.prices, process_start: e.process_start.expect("set by step"), stopping: &|| false };
        let outcome = tick::tick_recovering(&e.primary, &venue, id, clock, attempts, &mut recovered, &cx).await?;
        // An order accepted this tick gets one follow-up poll shortly after (a deliberate small delay; Rails enqueues
        // FetchAndUpdateOrderJob at placement), whatever the tick's final outcome.
        let mut s = e.primary.prepare(
            "SELECT id FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1) AND created_at >= ?2")?;
        let accepted = s.query_map(rusqlite::params![id, tick_start], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        drop(s);
        for tx in accepted.into_iter().chain(recovered) { e.polls.insert(tx, (id, clock.now().timestamp_micros() + POLL_AFTER_US, Attempts::default())); }
        match outcome {
            TickOutcome::RetryAfter(d) => { e.retry_at.insert(id, clock.now().timestamp_micros() + d.as_micros() as i64); }
            TickOutcome::AwaitingReconciliation => { e.retry_at.remove(&id); e.reconcile_at.insert(id, clock.now().timestamp_micros() + RECONCILE_EVERY_US); }
            TickOutcome::Done { .. } => { e.retry_at.remove(&id); e.reconcile_at.remove(&id); }
            // Rescheduled: `retrying` until the next checkpoint, as Rails' reschedule leaves it.
            TickOutcome::Rescheduled => { e.retry_at.insert(id, cps.next_us + AFTER_CHECKPOINT_US); e.reconcile_at.remove(&id); }
            TickOutcome::Skipped | TickOutcome::Stopped => { e.retry_at.remove(&id); e.reconcile_at.remove(&id); e.attempts.remove(&id); }
        }
    }
    // Only entries that still apply count: a stale one (bot stopped and restarted) would spin the loop.
    if bot.status != crate::enums::BotStatus::Retrying && !due { e.retry_at.remove(&id); }
    if bot.rust_placement().is_none() && !due { e.reconcile_at.remove(&id); }
    for at in [e.retry_at.get(&id), e.reconcile_at.get(&id)].into_iter().flatten() { *wake = (*wake).min(*at); }
    let next = checkpoints(anchor, clock.now().timestamp_micros(), eff).next_us;
    *wake = (*wake).min(next + AFTER_CHECKPOINT_US);
    Ok(())
}

/// Runs until an engine-level error; the caller exits non-zero. Release builds abort on panic, so a
/// half-alive process that serves pages while nothing trades cannot exist (spec §3 Supervision).
pub async fn run<F: VenueFactory>(mut e: Engine<F>, clock: &dyn Clock) -> Result<Infallible, EngineError> {
    loop {
        let wake = step(&mut e, clock).await?;
        let wait = (wake - clock.now().timestamp_micros()).max(0) as u64;
        tokio::time::sleep(std::time::Duration::from_micros(wait)).await;
    }
}
