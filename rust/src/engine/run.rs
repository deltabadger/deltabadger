//! The engine loop. No job table: each pass re-reads the working bots (UI changes are seen within a minute),
//! ticks the due ones, and sleeps until the earliest next event. What Rails would hold in Solid Queue —
//! follow-up polls, retries — is kept in memory and rebuilt from the database on start.
use super::events::{EngineEvent, EngineEvents};
use super::schedule::{checkpoints, effective};
use super::tick::{self, Attempts, PriceCache, TickContext, TickOutcome};
use super::{amount, eligibility, model, placement, polling, Clock, EngineError};
use crate::crypto::Cipher;
use crate::lease::EngineLock;
use crate::venue::VenueFactory;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Notify;

const IDLE_US: i64 = 60_000_000;
const AFTER_CHECKPOINT_US: i64 = 1_000;
const POLL_AFTER_US: i64 = 5_000_000;
const RECONCILE_EVERY_US: i64 = 30_000_000;
/// How long a bot refused for stale reference data waits before it is ticked again. Rails' catalog sync runs once a day and the
/// bound is 49 h, so 5 minutes delays the first tick after a refresh by at most 5 minutes for 12 staleness reads an hour.
pub const STALE_RECHECK_US: i64 = 300_000_000;

type CacheInsertStep=Box<dyn FnOnce(&Connection)->Result<(),EngineError>>;

pub struct Engine<F: VenueFactory> {
    pub primary: Connection, pub factory: F, pub cipher: Cipher, pub lock: EngineLock,
    started: bool,
    attempts: HashMap<i64, model::Produced<Attempts>>,
    retry_at: HashMap<i64, model::Produced<i64>>,
    closed_until: HashMap<i64, (model::Produced<i64>, serde_json::Value)>,
    cache_insert_step: Option<CacheInsertStep>,
    /// Follow-up polls owed, one per ORDER (FetchAndUpdateOrderJob): tx id → (bot, due time, that job's own retry counters).
    polls: HashMap<i64, model::Produced<(i64, i64, Attempts)>>,
    reconcile_at: HashMap<i64, model::Produced<i64>>,
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
    /// Bots whose tick was refused for stale reference data, by the source named: logged once, not on every pass.
    stale_logged: HashMap<i64, &'static str>,
    /// Bots refused for a persisted deferral this build cannot read: logged once, not on every pass.
    defer_refused: HashSet<i64>,
    /// Told after each tick what it recorded, never called into.
    events: EngineEvents,
}

impl<F: VenueFactory> Engine<F> {
    pub fn new(primary: Connection, factory: F, cipher: Cipher, lock: EngineLock) -> Self {
        Self { primary, factory, cipher, lock, started: false, attempts: HashMap::new(), retry_at: HashMap::new(), closed_until: HashMap::new(), cache_insert_step:None, polls: HashMap::new(), reconcile_at: HashMap::new(),
               prices: PriceCache::default(), process_start: None, wake: Arc::new(Notify::new()), stop: Arc::new(AtomicBool::new(false)),
               stopped: Arc::new(tokio::sync::watch::Sender::new(false)), writers_guarded: false, stale_logged: HashMap::new(), defer_refused: HashSet::new(),
               events: EngineEvents::default() }
    }
    #[doc(hidden)] pub fn inject_cache_insert_step(&mut self, step: impl FnOnce(&Connection)->Result<(),EngineError> + 'static) { self.cache_insert_step=Some(Box::new(step)); }
    pub fn wake_handle(&self) -> Arc<Notify> { self.wake.clone() }
    /// A receiver of every event this engine sends from now on. Taken before `run::run` consumes the engine.
    pub fn subscribe(&mut self) -> UnboundedReceiver<EngineEvent> { self.events.subscribe() }
    /// The events `wants` accepts (`EngineEvents::subscribe_to`).
    pub fn subscribe_to(&mut self, wants: fn(&EngineEvent) -> bool) -> UnboundedReceiver<EngineEvent> { self.events.subscribe_to(wants) }
    pub fn stop_handle(&self) -> Shutdown { Shutdown { flag: self.stop.clone(), wake: self.wake.clone(), stopped: self.stopped.clone() } }
    fn stopping(&self) -> bool { self.stop.load(Ordering::SeqCst) }
    #[doc(hidden)] pub fn inject_stale_retry(&mut self, bot: i64, at_us: i64) { self.retry_at.insert(bot, model::Produced::new(at_us,None)); }
    fn venue_for(&self, bot: &model::Bot) -> Result<(crate::venue::Handle<F::V>, Option<model::CredentialVersion>), EngineError> {
        let (credentials,version,sensitive)=model::credentials_with_redaction(&self.primary,&self.cipher,bot)?;
        Ok((crate::venue::Handle::new(self.factory.for_bot_with_redaction(&model::exchange_type(&self.primary, bot)?, credentials,sensitive),version.clone()),version))
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

/// After a tick, whether it ended well or in an error: each order it placed gets one follow-up poll shortly after (a
/// deliberate small delay; Rails enqueues FetchAndUpdateOrderJob at placement), and each row it inserted or recovered is
/// announced. An order it placed is in no later sweep's waiting set, so this is its only announcement and its only poll.
fn after_tick<F: VenueFactory>(e: &mut Engine<F>, id: i64, tick_start: &str, last_tx: i64, recovered: Option<i64>, clock: &dyn Clock, producer:&Option<model::CredentialVersion>) -> Result<(), EngineError> {
    let mut s = e.primary.prepare(
        "SELECT id FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1) AND created_at >= ?2")?;
    let accepted = s.query_map(rusqlite::params![id, tick_start], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for tx in accepted.into_iter().chain(recovered) { e.polls.insert(tx, model::Produced::new((id, clock.now().timestamp_micros() + POLL_AFTER_US, Attempts::default()),producer.clone())); }
    let mut s = e.primary.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND id > ?2 ORDER BY id")?;
    let created = s.query_map(rusqlite::params![id, last_tx], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for tx in created.iter().copied().chain(recovered.filter(|r| !created.contains(r))) {
        e.events.send(EngineEvent::OrderRecorded { bot_id: id, transaction_id: tx });
    }
    Ok(())
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
    let bot = model::load_bot(&e.primary, id)?;
    let (venue, producer) = e.venue_for(&bot)?;
    if e.reconcile_at.get(&id).is_some_and(|state|!state.current_for(&producer).is_fresh()){e.reconcile_at.remove(&id);}
    if let Some(state)=e.reconcile_at.get(&id){if state.value>clock.now().timestamp_micros(){*wake=(*wake).min(state.value);return Ok(())}}
    match placement::recover_since(&e.primary, &venue, &bot, clock, e.process_start.expect("set by step")).await? {
        placement::Recovery::Pending => {
            let at = clock.now().timestamp_micros() + RECONCILE_EVERY_US;
            e.reconcile_at.insert(id, model::Produced::new(at,producer.clone()));
            *wake = (*wake).min(at);
        }
        placement::Recovery::Recorded(tx) => {
            e.events.send(EngineEvent::OrderRecorded { bot_id: id, transaction_id: tx });
            e.reconcile_at.remove(&id);
            e.polls.insert(tx, model::Produced::new((id, clock.now().timestamp_micros() + POLL_AFTER_US, Attempts::default()),producer.clone()));
        }
        placement::Recovery::NotPlaced | placement::Recovery::NoIntent => { e.reconcile_at.remove(&id); }
    }
    Ok(())
}

pub async fn step<F: VenueFactory>(e: &mut Engine<F>, clock: &dyn Clock) -> Result<i64, EngineError> {
    e.process_start.get_or_insert(clock.now());
    super::provider::bind_cipher(&e.primary,&e.cipher)?;
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
    e.closed_until.retain(|id, _| report.eligible.contains(id));

    if !e.started {
        // Rebuilt obligation: every outstanding order is polled once, as Rails' adopt_handback! does.
        let now_us = clock.now().timestamp_micros();
        for (tx, bot) in outstanding_orders(&e.primary)? {
            let producer=model::load_bot(&e.primary,bot).and_then(|row|e.venue_for(&row))?.1;
            e.polls.entry(tx).or_insert_with(||model::Produced::new((bot,now_us,Attempts::default()),producer));
        }
        // Amount-limit stops a swept fill committed before a crash (polling::apply_committed): Rails' StopJobs would have run.
        let mut s = e.primary.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_amount_limit_stops_pending') IS NOT NULL ORDER BY id")?;
        let pending = s.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        drop(s);
        for id in pending {
            // Per bot: one this build cannot read (eligibility lists it as unreadable) is skipped here as everywhere else.
            match tick::run_pending_amount_limit_stops(&e.primary, id, clock.now()) {
                Err(err @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(err),
                Err(err) => super::log(&format!("[engine] bot {id}: its counted amount-limit stops could not run: {err:?}; skipped")),
                Ok(()) => {}
            }
        }
        e.started = true;
    }

    let mut wake = clock.now().timestamp_micros() + IDLE_US;
    run_polls(e, clock, &mut wake).await;
    for id in idle_bots_with_intents(&e.primary)? {
        if e.stopping() { break; }
        if let Err(err) = reconcile_idle(e, id, clock, &mut wake).await {
            if matches!(err, EngineError::Lease(_) | EngineError::Store(_)) { return Err(err); }
            let changed=matches!(err,EngineError::CredentialsChanged);
            if !changed {super::log(&format!("[engine] bot {id}: placement reconciliation failed: {err:?}; retrying in 30 s"));}
            let delay=if changed {0}else{RECONCILE_EVERY_US};
            let at = clock.now().timestamp_micros() + delay;
            e.reconcile_at.insert(id, model::Produced::new(at,None));
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
    for state in e.polls.values() { wake = wake.min(state.value.1); }
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
/// bot is stopped meanwhile). Transient/RateLimited failures and a still-open order retry as FetchAndUpdateOrderJob's retry_on.
async fn run_polls<F: VenueFactory>(e: &mut Engine<F>, clock: &dyn Clock, wake: &mut i64) {
    let now_us = clock.now().timestamp_micros();
    let entries:Vec<_>=e.polls.iter().map(|(tx,state)|(*tx,state.clone())).collect();
    for (tx,state) in entries {
        if e.stopping() { break; }
        let (id,at,mut attempts)=state.value;
        let (venue,producer)=match model::load_bot(&e.primary,id).and_then(|bot|e.venue_for(&bot)){
            Ok(value)=>value,
            Err(err)=>{super::log(&format!("[engine] bot {id}: follow-up poll deferred 30 s: {err:?}"));e.polls.insert(tx,model::Produced::new((id,clock.now().timestamp_micros()+RECONCILE_EVERY_US,attempts),state.origin().clone()));continue;}
        };
        let fresh=state.current_for(&producer).is_fresh();
        if fresh && at>now_us {continue}
        e.polls.remove(&tx);
        if !fresh {attempts=Attempts::default();}
        let mut announce_retry=true;
        let retry = match polling::follow_up(&e.primary, &venue, id, tx, clock.now()).await {
            Ok(polling::FollowUp::Done) => None,
            Ok(polling::FollowUp::StillOpen) => { attempts.open += 1; (attempts.open < 8).then(|| (tick::retry_wait(attempts.open, false), "the order is still open".to_string())) }
            Err(polling::PollFailure::CredentialsChanged) => {announce_retry=false;Some((std::time::Duration::ZERO, "credentials changed; retry with fresh credentials".into()))},
            Err(polling::PollFailure::Transient(m)) => { attempts.transient += 1; (attempts.transient < 3).then(|| (tick::retry_wait(attempts.transient, false), m)) }
            Err(polling::PollFailure::RateLimited(m)) => { attempts.rate += 1; (attempts.rate < 4).then(|| (tick::retry_wait(attempts.rate, true), m)) }
            Err(polling::PollFailure::General(m)) => { super::log(&format!("[engine] bot {id}: follow-up poll failed: {m}")); None }
        };
        // Whatever the poll wrote, on success or on error, the bot is announced; nothing is read to decide.
        e.events.send(EngineEvent::OrderUpdated { bot_id: id });
        if let Some((wait, m)) = retry {
            if announce_retry {super::log(&format!("[engine] bot {id}: follow-up poll failed ({m}); retrying in {}s", wait.as_secs()));}
            e.polls.insert(tx, model::Produced::new((id, clock.now().timestamp_micros() + wait.as_micros() as i64, attempts),producer));
        }
    }
    for state in e.polls.values() { *wake = (*wake).min(state.value.1); }
}

/// The web continued this bot (Bot::Lifecycle#start(start_fresh: false)) and left `rust_continue_start` for the engine.
/// Rails' decision (amount::continue_runs_now) becomes a persisted wait: one that has ended (run now) or one for the next
/// checkpoint. In the same transaction the request goes, with any amount-limit stop still counted (the user's resume
/// overrides a stop Rails would already have run) and the old wait (the decision replaces it). A request whose value is not
/// `{"requested_at": a readable timestamp}` refuses without deleting the request (R9). The web also sends
/// `was_stopped`: only an originally stopped bot may wait. Older requests omit it and
/// retain the stopped-bot contract that preceded lifecycle writes.
fn continue_start(c: &Connection, id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    let bot = model::load_bot(&tx, id)?;
    let Some(request) = bot.transient.get("rust_continue_start") else { return Ok(()) };
    crate::codec::optional_time(request.get("requested_at")).map_err(|_|EngineError::Data("unreadable stored timestamp".into()))?
        .ok_or_else(||EngineError::Data("missing continue timestamp".into()))?;
    if let (Some(anchor), Some(interval), Some(quote)) = (bot.started_at_us()?, bot.interval(), bot.quote_amount()) {
        checkpoints(anchor, now.timestamp_micros(), effective(interval, quote, bot.smart_quote_amount()))?.validate_times()?;
    }
    tx.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_continue_start', '$.rust_amount_limit_stops_pending') \
                WHERE id = ?1", [id])?;
    placement::remove_wait(&tx, Some(id))?;
    let decision = if bot.started_at_us()?.is_none() || bot.interval().is_none() || bot.quote_amount().is_none() {
        "never ticks" // step_bot skips it as before
    } else if request.get("was_stopped").and_then(serde_json::Value::as_bool) == Some(false)
        || amount::continue_runs_now(&tx, &bot, now.timestamp_micros())? {
        placement::run_now(&tx, &bot, now)?;
        "runs now"
    } else {
        placement::defer_to_next_checkpoint(&tx, &bot, now)?;
        "waits for its next checkpoint"
    };
    tx.commit()?;
    super::log(&format!("[engine] bot {id}: continued; {decision}"));
    Ok(())
}

// A stop/start between loop passes also invalidates a wait, as does a changed index definition.
fn market_wait_key(c: &Connection, bot: &model::Bot) -> Result<serde_json::Value, EngineError> {
    let stopped: Option<String> = c.query_row("SELECT stopped_at FROM bots WHERE id=?1", [bot.id], |r| r.get(0))?;
    let index: String = c.query_row("SELECT json_group_array(json_array(source,top_coins,weights)) FROM indices WHERE external_id=?1",
        [bot.index_category_id()], |r| r.get(0))?;
    Ok(serde_json::json!({"composition":placement::composition_snapshot(c, bot)?, "index":index,
        "started":bot.started_at_us()?, "stopped":stopped, "changed":bot.settings_changed_at_us()?}))
}

async fn step_bot<F: VenueFactory>(e: &mut Engine<F>, id: i64, clock: &dyn Clock, wake: &mut i64) -> Result<(), EngineError> {
    let now_us = clock.now().timestamp_micros();
    let mut bot = model::load_bot(&e.primary, id)?;
    // A persisted deferral is a schedule decision: one this build cannot read (null, garbage or out of range) refuses
    // the bot and is kept, never removed, so a restart cannot turn it into an early buy.
    if let Err(err) = bot.rust_defer() {
        if e.defer_refused.insert(id) { super::log(&format!("[engine] warning: bot {id}: unreadable rust_defer_until: {err:?}; refused until it is readable")); }
        return Ok(());
    }
    e.defer_refused.remove(&id);
    bot.validate_times()?;
    if bot.transient.get("rust_continue_start").is_some() {
        e.closed_until.remove(&id);
        match continue_start(&e.primary, id, clock.now()) {
            Ok(()) => bot = model::load_bot(&e.primary, id)?,
            Err(err @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(err),
            // The request stays: the bot is skipped this pass and decided again on the next one, never left silently.
            Err(err) => {
                super::log(&format!("[engine] warning: bot {id}: its continue could not be decided: {err:?}; skipped this pass, retried on the next"));
                return Ok(());
            }
        }
    }
    let (venue, credential_version) = e.venue_for(&bot)?;
    // Failure counters and retry waits belong to the immutable handle that produced them.
    if e.attempts.get(&id).is_some_and(|state|!state.current_for(&credential_version).is_fresh()) {e.attempts.remove(&id);}
    if e.retry_at.get(&id).is_some_and(|state|!state.current_for(&credential_version).is_fresh()) {e.retry_at.remove(&id);}

    if e.reconcile_at.get(&id).is_some_and(|state|!state.current_for(&credential_version).is_fresh()){e.reconcile_at.remove(&id);}
    if let Some((until, key)) = e.closed_until.get(&id) {
        if !until.current_for(&credential_version).is_fresh() || until.value <= now_us || model::all_crypto(&e.primary, &bot)? || *key != market_wait_key(&e.primary, &bot)? {
            e.closed_until.remove(&id);
        }
    }

    let (Some(anchor), Some(interval), Some(quote)) = (bot.started_at_us()?, bot.interval(), bot.quote_amount()) else { return Ok(()) };
    let eff = effective(interval, quote, bot.smart_quote_amount());
    let cps = checkpoints(anchor, now_us, eff)?;
    cps.validate_times()?;
    // A rescheduled run waits for the next checkpoint, across a restart too, while the schedule it was computed under holds.
    // A fresh start or an interval edit voids it: the bot then follows its schedule as a scheduled bot does.
    let wait_tx=model::immediate(&e.primary)?;
    let wait_bot=model::load_bot(&wait_tx,id)?;
    let defer = match wait_bot.rust_defer() {
        Ok(d) => match d {
            Some((t,schedule)) => {
                let fresh=model::wait_is_current(&wait_tx,&wait_bot.transient["rust_defer_until"])?;
                if !fresh {placement::remove_wait(&wait_tx,Some(id))?;None}
                else {let key = bot.schedule_key()?; Some((Some(schedule)==key).then_some(t))}
            }
            None=>None,
        },
        // Changed since the check above: refused and kept, as there; the next pass logs it.
        Err(_) => return Ok(()),
    };
    wait_tx.commit()?;
    let deferred = defer.flatten().filter(|&t| t >= now_us);
    let on_schedule = || -> Result<bool, EngineError> {
        Ok(anchor <= now_us && bot.last_action_job_at_us()?.is_none_or(|t| t.div_euclid(1000) < cps.last_us.div_euclid(1000))) // stored value is ms-truncated
    };
    let due = if bot.rust_placement().is_some() {
        e.reconcile_at.get(&id).is_none_or(|t| t.value <= now_us)
    } else if e.closed_until.get(&id).is_some_and(|(t, _)| t.value > now_us) || deferred.is_some() {
        false
    } else if defer.flatten().is_some_and(|t| t < now_us) {
        true // the wait has ended (strictly after it, as a checkpoint is): a continue start Rails runs at once (placement::run_now)
    } else if defer == Some(None) {
        on_schedule()? // a retrying bot too: its in-memory wait was computed under the old schedule
    } else if bot.status == crate::enums::BotStatus::Retrying {
        e.retry_at.get(&id).is_none_or(|t| t.value <= now_us) // no in-memory state (a restart): due at once
    } else {
        on_schedule()?
    };

    if due {
        // The loop is single-threaded: a due bot met `executing`/`waiting` was left there by an error that escaped its
        // tick, and the tick would skip it forever.
        if model::unstick(&e.primary, id, clock.now())? { super::log(&format!("[engine] bot {id}: left {:?} by an earlier tick; back to scheduled", bot.status)); }
        let tick_start = crate::codec::format_time(clock.now());
        let attempts = &mut e.attempts.entry(id).or_insert_with(||model::Produced::new(Attempts::default(),credential_version.clone())).value;
        let retry_producer=credential_version.clone();
        let mut recovered = None;
        let stop = e.stop.clone();
        let stopping = move || stop.load(Ordering::SeqCst);
        let below_minimum = |bot_id, transaction_ids| e.events.send(EngineEvent::BelowMinimum { bot_id, transaction_ids });
        let cx = TickContext { credential_version, prices: &e.prices, process_start: e.process_start.ok_or_else(||EngineError::Data("engine start time missing".into()))?, stopping: &stopping, below_minimum: &below_minimum };
        // What this tick records is announced after it returns, when its writes have committed: rows by id (every row
        // the tick inserts, as Rails' after_create_commit hears every create), and a new funds mail marker.
        let last_tx: i64 = e.primary.query_row("SELECT coalesce(max(id), 0) FROM transactions", [], |r| r.get(0))?;
        let funds_marker = |c: &Connection| c.query_row(&format!("SELECT json_extract(transient_data, '$.{}') FROM bots WHERE id = ?1", tick::FUNDS_MAIL_PENDING),
                                                         [id], |r| r.get::<_, Option<String>>(0));
        let funds_before = funds_marker(&e.primary)?;
        let ticked = tick::tick_recovering(&e.primary, &venue, id, clock, attempts, &mut recovered, &cx).await;
        // Whatever the tick wrote (its sweep's fills, its placement), on success or on error, the bot is announced, with
        // nothing read to decide; each order it placed is announced and gets its follow-up poll (`after_tick`).
        e.events.send(EngineEvent::OrderUpdated { bot_id: id });
        let committed = after_tick(e, id, &tick_start, last_tx, recovered, clock,&retry_producer);
        let outcome = ticked?;
        committed?;
        let repeat = matches!(&outcome, TickOutcome::Stale { source, .. } if e.stale_logged.get(&id) == Some(source));
        if !repeat { super::log(&format!("bot {id}: {outcome:?}")); }
        match &outcome {
            TickOutcome::Stale { source, .. } => { e.stale_logged.insert(id, *source); }
            _ => { e.stale_logged.remove(&id); }
        }
        // tick.rs leaves the funds marker with the stamp where Rails mails BotAlertsMailer#end_of_funds. The stamp alone is
        // no signal: a failure that stops the bot stamps it too, and its mail rides the error or stopped marker instead.
        let funds_after = funds_marker(&e.primary)?;
        if funds_after.is_some() && funds_after != funds_before {
            e.events.send(EngineEvent::FundsLow { bot_id: id, user_id: bot.user_id, quote_asset_id: bot.quote_asset_id() });
        }
        match outcome {
            TickOutcome::MarketClosed { until, producer } => {
                if let Some(step)=e.cache_insert_step.take(){step(&e.primary)?;}
                e.closed_until.insert(id, (model::Produced::new(until.timestamp_micros(),producer), market_wait_key(&e.primary, &model::load_bot(&e.primary, id)?)?));
                e.retry_at.remove(&id); e.reconcile_at.remove(&id); e.attempts.remove(&id);
            }
            TickOutcome::RetryAfter(d) => { e.retry_at.insert(id, model::Produced::new(clock.now().timestamp_micros() + d.as_micros() as i64,retry_producer.clone())); }
            TickOutcome::AwaitingReconciliation => { e.retry_at.remove(&id); e.reconcile_at.insert(id, model::Produced::new(clock.now().timestamp_micros() + RECONCILE_EVERY_US,retry_producer.clone())); }
            TickOutcome::Done { .. } => { e.retry_at.remove(&id); e.reconcile_at.remove(&id); }
            // Rescheduled: `retrying` until the next checkpoint, as Rails' reschedule leaves it.
            TickOutcome::Rescheduled => { e.retry_at.insert(id, model::Produced::new(cps.next_us + AFTER_CHECKPOINT_US,retry_producer.clone())); e.reconcile_at.remove(&id); }
            // The bot stays due and is rechecked after a bounded wait: an expired retry time left here would pull every wake to now.
            TickOutcome::Stale { .. } => { e.retry_at.insert(id, model::Produced::new(clock.now().timestamp_micros() + STALE_RECHECK_US,retry_producer.clone())); }
            TickOutcome::Skipped | TickOutcome::Stopped => { e.retry_at.remove(&id); e.reconcile_at.remove(&id); e.attempts.remove(&id); }
        }
    }
    // Only entries that still apply count: a stale one (bot stopped and restarted) would spin the loop.
    if bot.status != crate::enums::BotStatus::Retrying && !due { e.retry_at.remove(&id); }
    if bot.rust_placement().is_none() && !due { e.reconcile_at.remove(&id); }
    for at in [e.retry_at.get(&id).map(|t|&t.value), e.reconcile_at.get(&id).map(|t|&t.value), e.closed_until.get(&id).map(|(t, _)| &t.value).filter(|&&t| t > now_us)].into_iter().flatten() { *wake = (*wake).min(*at); }
    if let Some(t) = deferred { *wake = (*wake).min(t + AFTER_CHECKPOINT_US); }
    let next = checkpoints(anchor, clock.now().timestamp_micros(), eff)?.next_us;
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
