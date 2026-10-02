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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

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
    /// Set by the first `step`: an Alpaca absence is trusted only a full margin (20 min) after it (placement::recover_since).
    process_start: Option<DateTime<Utc>>,
    /// Notified to make the loop step now (a stop request; the web UI after it starts or stops a bot).
    wake: Arc<Notify>,
    stop: Arc<AtomicBool>,
    /// The awaitable side of `stop`, for the parts of `serve` beside the engine (`Shutdown::requested`).
    stopped: Arc<tokio::sync::watch::Sender<bool>>,
    /// Set by `supervisor::serve`: every other writer of `bots` in this process runs `eligibility::guard` before it
    /// commits, so a pass that finds the install ineligible means one skipped it.
    pub(crate) writers_guarded: bool,
}

impl<F: VenueFactory> Engine<F> {
    pub fn new(primary: Connection, factory: F, cipher: Cipher, lock: EngineLock) -> Self {
        Self { primary, factory, cipher, lock, started: false, attempts: HashMap::new(), retry_at: HashMap::new(), polls: HashMap::new(), reconcile_at: HashMap::new(),
               prices: PriceCache::default(), process_start: None, wake: Arc::new(Notify::new()), stop: Arc::new(AtomicBool::new(false)),
               stopped: Arc::new(tokio::sync::watch::Sender::new(false)), writers_guarded: false }
    }
    pub fn wake_handle(&self) -> Arc<Notify> { self.wake.clone() }
    pub fn stop_handle(&self) -> Shutdown { Shutdown { flag: self.stop.clone(), wake: self.wake.clone(), stopped: self.stopped.clone() } }
    fn stopping(&self) -> bool { self.stop.load(Ordering::SeqCst) }
    #[doc(hidden)] pub fn inject_stale_retry(&mut self, bot: i64, at_us: i64) { self.retry_at.insert(bot, at_us); }
    fn venue_for(&self, bot: &model::Bot) -> Result<F::V, EngineError> {
        Ok(self.factory.for_bot(&model::exchange_type(&self.primary, bot)?, model::credentials_for(&self.primary, &self.cipher, bot)?))
    }
}

/// A stop request for the loop: the tick in hand finishes, nothing new starts, and `run` returns `EngineError::Stopped`.
/// The process's one stop signal (`serve`: the engine, the web and every background service).
#[derive(Clone)]
pub struct Shutdown { flag: Arc<AtomicBool>, wake: Arc<Notify>, stopped: Arc<tokio::sync::watch::Sender<bool>> }

impl Shutdown {
    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.stopped.send_replace(true);
        self.wake.notify_one(); // a permit is stored if the loop is not asleep yet
    }
    pub fn is_requested(&self) -> bool { self.flag.load(Ordering::SeqCst) }
    /// `true` once a stop is requested: for a service that selects on a `watch` (a scheduler).
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> { self.stopped.subscribe() }
    /// Resolves once a stop is requested, at once if it already was. It never takes the engine's wake permit.
    pub async fn requested(&self) {
        let mut rx = self.subscribe();
        let _ = rx.wait_for(|stopped| *stopped).await;
    }
    /// Requests the stop on SIGTERM (`docker stop`) or SIGINT; off Unix, on Ctrl-C (the only signal Windows delivers).
    /// The handlers are registered before this returns, so a signal that arrives afterwards is never lost. Call inside
    /// the runtime.
    #[cfg(unix)]
    pub fn on_signals(&self) {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("a SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("a SIGINT handler");
        let me = self.clone();
        tokio::spawn(async move {
            tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            me.request();
        });
    }
    #[cfg(not(unix))]
    pub fn on_signals(&self) {
        // The same as `tokio::signal::ctrl_c()`, but this registers the console handler now, not on first poll.
        let mut ctrl_c = tokio::signal::windows::ctrl_c().expect("a Ctrl-C handler");
        let me = self.clone();
        tokio::spawn(async move {
            ctrl_c.recv().await;
            me.request();
        });
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
    // In one process every other writer of `bots` runs eligibility::guard, so this is a write that skipped it.
    // Debug builds (every test) name the bot; release builds end the engine, and the process with it.
    #[cfg(debug_assertions)]
    if e.writers_guarded { assert_guarded(&e.primary, &report); }
    if !report.problems.is_empty() { return Err(EngineError::Ineligible(report.problems)); }
    for (id, err) in &report.unreadable { super::log(&format!("[engine] bot {id} is unreadable and skipped: {err}")); }
    // A bot that left the working set (stopped from the web UI) starts again as a fresh Rails job would: no retry
    // counters and no retry time carried over. Polls (per order) and reconciliation (per intent) are not per job and stay.
    // ponytail: a stop and a start that both land inside one pass keep the counters; key them by stopped_at if that matters.
    e.attempts.retain(|id, _| report.eligible.contains(id));
    e.retry_at.retain(|id, _| report.eligible.contains(id));

    if !e.started {
        // Rebuilt obligation: every outstanding order is polled once, as Rails' adopt_handback! does.
        let now_us = clock.now().timestamp_micros();
        for (tx, bot) in outstanding_orders(&e.primary)? { e.polls.entry(tx).or_insert((bot, now_us, Attempts::default())); }
        e.started = true;
    }

    let mut wake = clock.now().timestamp_micros() + IDLE_US;
    run_polls(e, clock, &mut wake).await;
    for id in idle_bots_with_intents(&e.primary)? {
        if e.stopping() { break; }
        if let Err(err) = reconcile_idle(e, id, clock, &mut wake).await {
            if matches!(err, EngineError::Lease(_) | EngineError::Store(_)) { return Err(err); }
            super::log(&format!("[engine] bot {id}: placement reconciliation failed: {err:?}; retrying in 30 s"));
            let at = clock.now().timestamp_micros() + RECONCILE_EVERY_US;
            e.reconcile_at.insert(id, at);
            wake = wake.min(at);
        }
    }
    for id in report.eligible {
        if e.stopping() { break; }
        if let Err(err) = step_bot(e, id, clock, &mut wake).await {
            // Lease/store errors end the engine. Sqlite errors stay per bot and are retried next pass; a replay after an
            // accepted order is prevented by the early last_action_job_at write and the persisted intent.
            if matches!(err, EngineError::Lease(_) | EngineError::Store(_)) { return Err(err); }
            super::log(&format!("[engine] bot {id}: {err:?}; retrying at the next pass"));
        }
    }
    // Polls queued during this pass (a placement, a recovered intent) must pull the wake in too.
    for (_, at, _) in e.polls.values() { wake = wake.min(*at); }
    let now_us = clock.now().timestamp_micros();
    Ok(wake.max(now_us + 1))
}

/// Every bot a write that skipped `eligibility::guard` left behind, as the guard would have refused it: outside the
/// slice, unreadable, or changed under an unresolved order (stranded). Release builds skip the last two
/// as before (unreadable rows are logged and skipped; a stranded intent waits for `resolve-placement`).
#[cfg(debug_assertions)]
fn assert_guarded(c: &Connection, report: &eligibility::Report) {
    let mut named = report.problems.clone();
    named.extend(report.unreadable.iter().map(|(id, err)| format!("bot {id}: unreadable ({err})")));
    // An error here (a row it cannot load) is an unreadable bot, named above.
    let stranded = placement::stranded(c).unwrap_or_default();
    named.extend(stranded.iter().map(|id| format!("bot {id}: stranded: its composition, asset, exchange or quote changed while its order is unresolved")));
    assert!(named.is_empty(), "a write to bots skipped eligibility::guard: {}", named.join("; "));
}

/// Due follow-up polls, for any bot whatever its status (a job Rails enqueued at placement runs even if the
/// bot is stopped meanwhile). Transient/RateLimited failures retry as FetchAndUpdateOrderJob's retry_on.
async fn run_polls<F: VenueFactory>(e: &mut Engine<F>, clock: &dyn Clock, wake: &mut i64) {
    let now_us = clock.now().timestamp_micros();
    let due: Vec<(i64, i64, Attempts)> = e.polls.iter().filter(|(_, (_, at, _))| *at <= now_us).map(|(tx, (bot, _, a))| (*tx, *bot, *a)).collect();
    for (tx, id, mut attempts) in due {
        if e.stopping() { break; }
        e.polls.remove(&tx);
        let venue = match model::load_bot(&e.primary, id).and_then(|bot| e.venue_for(&bot)) {
            Ok(v) => v,
            Err(err) => {
                super::log(&format!("[engine] bot {id}: follow-up poll deferred 30 s: {err:?}"));
                e.polls.insert(tx, (id, clock.now().timestamp_micros() + RECONCILE_EVERY_US, attempts));
                continue;
            }
        };
        let retry = match polling::follow_up(&e.primary, &venue, id, tx, clock.now()).await {
            Ok(()) => None,
            Err(polling::PollFailure::Transient(m)) => { attempts.transient += 1; (attempts.transient < 3).then(|| (tick::retry_wait(attempts.transient, false), m)) }
            Err(polling::PollFailure::RateLimited(m)) => { attempts.rate += 1; (attempts.rate < 4).then(|| (tick::retry_wait(attempts.rate, true), m)) }
            Err(polling::PollFailure::General(m)) => { super::log(&format!("[engine] bot {id}: follow-up poll failed: {m}")); None }
        };
        if let Some((wait, m)) = retry {
            super::log(&format!("[engine] bot {id}: follow-up poll failed ({m}); retrying in {}s", wait.as_secs()));
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
        if model::unstick(&e.primary, id, clock.now())? { super::log(&format!("[engine] bot {id}: left {:?} by an earlier tick; back to scheduled", bot.status)); }
        let tick_start = crate::codec::format_time(clock.now());
        let attempts = e.attempts.entry(id).or_default();
        let mut recovered = None;
        let stop = e.stop.clone();
        let stopping = move || stop.load(Ordering::SeqCst);
        let cx = TickContext { prices: &e.prices, process_start: e.process_start.expect("set by step"), stopping: &stopping };
        let outcome = tick::tick_recovering(&e.primary, &venue, id, clock, attempts, &mut recovered, &cx).await?;
        super::log(&format!("bot {id}: {outcome:?}"));
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

/// Runs until a stop request (`Err(EngineError::Stopped)`) or an engine-level error; the caller exits accordingly.
/// Release builds abort on panic, so a half-alive process that serves pages while nothing trades cannot exist.
pub async fn run<F: VenueFactory>(mut e: Engine<F>, clock: &dyn Clock) -> Result<Infallible, EngineError> {
    loop {
        if e.stopping() { return Err(EngineError::Stopped); }
        let wake = step(&mut e, clock).await?;
        if e.stopping() { return Err(EngineError::Stopped); }
        let wait = (wake - clock.now().timestamp_micros()).max(0) as u64;
        // A notify (a stop, or the web UI) cuts the sleep short; one stored before the sleep began counts too.
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_micros(wait)) => {}
            _ = e.wake.notified() => {}
        }
    }
}
