//! The in-process scheduler. While the Rust engine owns an install, Rails and Solid Queue are stopped, so the
//! recurring jobs of config/recurring.yml that the install needs run here. There is no job table: like the bots' ticks
//! (engine/run.rs), each job's next run is derived from its schedule and its last success (state.rs).
//!
//! One runner runs one job at a time. When several are due, the oldest due runs first; a job due only because it was
//! woken never runs ahead of a job whose scheduled fire is overdue past its staleness bound. Every run has an overall
//! deadline (Spec::deadline, DEADLINE by default): past it the run is dropped and recorded as an error. A retry is a later
//! due time, never a sleep. A wake (Wakers::wake, or an engine event the job wants) makes the job due from the moment the
//! runner sees it. One that arrives while the job runs runs it again right after: Solid Queue's discard-on-conflict, plus
//! the delay Rails adds so a sync lands after the one in flight (app/models/transaction.rb:46-49), become "run again once
//! the current run ends". On stop, the job in hand is dropped at its next await and nothing is recorded for it, so a
//! scheduled run is due again at the next start. A job's blocking work is a sequence of bounded units (one chunk each,
//! import::publish), so a deadline or a stop takes effect within one unit: no abandoned closure outlives its unit.
//!
//! A job may be registered once per scope (`Spec::scope`, e.g. a ledger sync per API key): each scope has its own due
//! time, wakes and record (`rust_job.<name>:<scope>`).
pub mod data_api;
pub mod schedule;
pub mod state;

use crate::crypto::Cipher;
use crate::engine::events::EngineEvent;
use crate::engine::{log, Clock};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use schedule::{first_due, Jitter, Schedule};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc::{error::TryRecvError, UnboundedReceiver};
use tokio::sync::{watch, Notify};

/// While idle, re-read the clock at least this often: a wall-clock step is noticed within a minute.
const IDLE: Duration = Duration::from_secs(60);

/// How long the runner's record waits for SQLite's write lock (another job's write unit holds it at most 100 ms).
const RECORD_BUSY: Duration = Duration::from_secs(1);

/// What the runner records for a run.
#[derive(Debug)]
enum Record { Success, Run, Error(String) }

/// A run's overall deadline unless its Spec declares another. Generous for every reference job: the longest, the stock
/// pull, is two 60 s reads and a chunked import. A job that may need longer (a full ledger sync) declares its own.
pub const DEADLINE: Duration = Duration::from_secs(600);

pub type JobFuture<'a> = Pin<Box<dyn Future<Output = Outcome> + 'a>>;

/// Rows per write unit of a bulk import (import::publish): no other writer waits on SQLite's write lock for longer
/// than one chunk.
pub const CHUNK: usize = 500;

/// What a run gets: the scheduler's database handle and the clock.
pub struct Cx<'a> { pub db: Db, pub clock: &'a dyn Clock }

/// The scheduler's connection for job work (a `store::open` of its own) and the instance's cipher.
#[derive(Clone)]
pub struct Db { conn: Arc<Mutex<Connection>>, cipher: Arc<Cipher> }

impl Db {
    pub fn new(conn: Connection, cipher: Cipher) -> Self { Self { conn: Arc::new(Mutex::new(conn)), cipher: Arc::new(cipher) } }

    /// Runs `f` on tokio's blocking pool, as the web's App::db does. Every SQLite statement and every large JSON walk of a
    /// job goes through here: nothing holds the runtime thread the engine ticks on past its 250 ms bound, and a wait
    /// on SQLite's lock happens off that thread. One call is one bounded unit (a read, or one chunk's transaction): a
    /// blocking closure cannot be cancelled, so a dropped run's work ends with its unit. Inside:
    /// `BEGIN IMMEDIATE` for a multi-statement write, and no transaction outlives the closure.
    pub async fn run<R: Send + 'static>(&self, f: impl FnOnce(&Connection, &Cipher) -> Result<R, String> + Send + 'static) -> Result<R, String> {
        let me = self.clone();
        tokio::task::spawn_blocking(move || f(&me.conn.lock().unwrap_or_else(PoisonError::into_inner), &me.cipher))
            .await
            .map_err(|e| format!("the blocking pool lost the work: {e}"))?
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The state key (`rust_job.<name>`), the log prefix, and what Wakers::wake names.
    pub name: &'static str,
    /// None for a job registered once; Some for one registered per scope (an API key id, …): its own schedule, wakes and
    /// record, `rust_job.<name>:<scope>`. Owned: scopes are known only at run time, and nothing is leaked.
    pub scope: Option<String>,
    /// None: on demand only (wakes and events).
    pub schedule: Option<Schedule>,
    pub jitter: Jitter,
    pub retry: Retry,
    /// The run's overall deadline: past it the run is dropped and recorded as an error. DEADLINE unless the job
    /// declares its own.
    pub deadline: Duration,
}

/// ActiveJob's retry_on. Transient and rate-limited outcomes are counted apart, as ActiveJob counts each retry_on apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Retry {
    None,
    /// `wait: :polynomially_longer, attempts: n`: executions⁴ + 2 s. ActiveJob's 15 % jitter is left out (timing only).
    Polynomial { attempts: u32 },
    /// `wait: w, attempts: n`.
    Fixed { wait_secs: u64, attempts: u32 },
}

impl Retry {
    /// The wait before the next execution after `n` failed ones, or None once the attempts are spent.
    pub fn wait(&self, n: u32) -> Option<Duration> {
        match *self {
            Retry::None => None,
            Retry::Polynomial { attempts } => (n < attempts).then(|| Duration::from_secs((n as u64).pow(4) + 2)),
            Retry::Fixed { wait_secs, attempts } => (n < attempts).then(|| Duration::from_secs(wait_secs)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The run refreshed its data completely: recorded as a success, which is what a source's freshness reads.
    Done,
    /// The run ended well but refreshed nothing (an empty payload, no venue to serve): recorded as a run only, so the
    /// source keeps its age.
    NothingNew,
    /// An error Rails logs, raises or returns without retrying. Recorded as the job's last error.
    Failed(String),
    /// Client::TransientNetworkError, or any failure the job's Rails class retries.
    Transient(String),
    /// Client::RateLimitedError.
    RateLimited(String),
}

/// Why a run happened. A run receives every wake that made it due since the previous run, in arrival order.
#[derive(Clone, Debug, PartialEq)]
pub enum Wake { Schedule, Manual(Option<i64>), Event(EngineEvent) }

pub trait Job {
    fn spec(&self) -> Spec;
    /// Engine events this job runs on. None by default.
    fn wants(&self, _event: &EngineEvent) -> bool { false }
    fn run<'a>(&'a self, cx: Cx<'a>, wakes: Vec<Wake>) -> JobFuture<'a>;
}

/// Wakes jobs on demand from anywhere in the process. It is Send + Sync, because the web runs on other threads.
#[derive(Clone, Default)]
pub struct Wakers(Arc<Shared>);

type Pending = (String, Option<String>, Wake);

#[derive(Default)]
struct Shared { pending: Mutex<Vec<Pending>>, notify: Notify }

impl Wakers {
    /// Runs `job` (in `scope`, for a job registered per scope) as soon as the runner is free. `key` is the job's own to
    /// interpret (an api key id, a user id). If it is running, it runs again right after. A wake for a name and scope that
    /// is no job here is logged and dropped.
    pub fn wake(&self, job: &str, scope: Option<&str>, key: Option<i64>) {
        self.0.pending.lock().unwrap_or_else(PoisonError::into_inner).push((job.to_string(), scope.map(String::from), Wake::Manual(key)));
        self.0.notify.notify_one(); // a permit is kept while the runner is busy: no wake is lost
    }

    fn take(&self) -> Vec<Pending> {
        std::mem::take(&mut *self.0.pending.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

struct Slot {
    job: Box<dyn Job>,
    spec: Spec,
    /// The next scheduled fire, plus its draw.
    scheduled: Option<DateTime<Utc>>,
    retry_at: Option<DateTime<Utc>>,
    /// The failed run's wakes, which its retry replays (ActiveJob retries with the same arguments).
    replay: Vec<Wake>,
    pending: Vec<Wake>,
    /// When the runner first saw a pending wake: its due time, for "oldest due first".
    pending_since: Option<DateTime<Utc>>,
    transient: u32,
    rate: u32,
}

impl Slot {
    fn due(&self) -> Option<DateTime<Utc>> { [self.scheduled, self.retry_at, self.pending_since].into_iter().flatten().min() }

    fn push(&mut self, wake: Wake, now: DateTime<Utc>) {
        self.pending_since.get_or_insert(now);
        self.pending.push(wake);
    }

    /// Due now only because it was woken: neither its schedule nor its retry is due.
    fn woken_only(&self, now: DateTime<Utc>) -> bool {
        self.scheduled.is_none_or(|t| t > now) && self.retry_at.is_none_or(|t| t > now)
    }

    /// Its scheduled fire is overdue past the job's staleness bound (Schedule::stale_after: 2 × period + 1 h).
    fn overdue(&self, now: DateTime<Utc>) -> bool {
        matches!((self.spec.schedule, self.scheduled), (Some(s), Some(t)) if now - t > s.stale_after())
    }
}

pub struct Scheduler {
    db: Db,
    /// The database file, for the runner's own state writes on a connection of their own (never behind `db`'s mutex,
    /// which a run dropped at its deadline may hold for its unit in hand). None: an in-memory database.
    records: Option<String>,
    slots: Vec<Slot>,
    wakers: Wakers,
    events: Option<UnboundedReceiver<EngineEvent>>,
}

impl Scheduler {
    /// `db`: the scheduler's own connection (its own `store::open`, as the web has its own). `events`: the engine's
    /// subscription (Engine::subscribe), or None. Ties in due time go by the order of `jobs`.
    pub fn new(db: Connection, cipher: Cipher, jobs: Vec<Box<dyn Job>>, events: Option<UnboundedReceiver<EngineEvent>>) -> Self {
        let slots = jobs.into_iter()
            .map(|job| Slot { spec: job.spec(), job, scheduled: None, retry_at: None, replay: vec![], pending: vec![], pending_since: None, transient: 0, rate: 0 })
            .collect();
        let records = db.path().filter(|p| !p.is_empty()).map(String::from);
        Self { db: Db::new(db, cipher), records, slots, wakers: Wakers::default(), events }
    }

    pub fn wakers(&self) -> Wakers { self.wakers.clone() }

    /// Runs jobs until `stop` turns true, then returns Ok at once, dropping the job in hand. It returns no other way:
    /// a job's failure is recorded and is the job's own, and a failed state write is logged.
    pub async fn run(mut self, mut stop: watch::Receiver<bool>, clock: &dyn Clock) -> Result<(), String> {
        let now = clock.now();
        for s in &mut self.slots {
            let Some(schedule) = s.spec.schedule else { continue };
            let (name, scope) = (s.spec.name, s.spec.scope.clone());
            let last = match self.db.run(move |c, _| state::read(c, name, scope.as_deref())).await {
                Ok(st) => st.last_run_at.max(st.last_success_at),
                Err(e) => { log(&format!("[jobs] {name}: state unreadable ({e}); treated as never run")); None }
            };
            s.scheduled = Some(first_due(schedule, s.spec.jitter.draw(), last, now));
        }
        loop {
            if *stop.borrow() { return Ok(()); }
            let now = clock.now();
            self.collect(now);
            if let Some(i) = self.pick(now) {
                if !self.run_slot(i, &mut stop, clock).await { return Ok(()); }
                continue;
            }
            let wait = self.slots.iter().filter_map(Slot::due).min().map_or(IDLE, |t| (t - now).to_std().unwrap_or_default().min(IDLE));
            let event = tokio::select! {
                _ = tokio::time::sleep(wait) => None,
                _ = self.wakers.0.notify.notified() => None,
                e = next_event(&mut self.events) => Some(e),
                _ = stop.wait_for(|stopped| *stopped) => return Ok(()),
            };
            match event {
                Some(Some(e)) => self.route(&e, clock.now()),
                Some(None) => self.events = None, // the engine is gone; the supervisor ends the process
                None => {}
            }
        }
    }

    /// The slot to run now: the oldest due first, ties by registration order. A slot due only because it was woken
    /// never runs ahead of a job whose scheduled fire is overdue past its staleness bound: the oldest of those runs first.
    fn pick(&self, now: DateTime<Utc>) -> Option<usize> {
        let oldest = |overdue_only: bool| self.slots.iter().enumerate()
            .filter(|(_, s)| !overdue_only || s.overdue(now))
            .filter_map(|(i, s)| s.due().filter(|t| *t <= now).map(|t| (t, i)))
            .min().map(|(_, i)| i);
        let i = oldest(false)?;
        if self.slots[i].woken_only(now) { oldest(true).or(Some(i)) } else { Some(i) }
    }

    /// Moves the wakes and the queued events into their slots, due from `now`.
    fn collect(&mut self, now: DateTime<Utc>) {
        for (name, scope, wake) in self.wakers.take() {
            match self.slots.iter_mut().find(|s| s.spec.name == name && s.spec.scope == scope) {
                Some(s) => s.push(wake, now),
                None => log(&format!("[jobs] a wake for {name} {scope:?}, which is no job here, was dropped")),
            }
        }
        let (mut queued, mut closed) = (vec![], false);
        if let Some(rx) = &mut self.events {
            loop {
                match rx.try_recv() {
                    Ok(e) => queued.push(e),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => { closed = true; break; }
                }
            }
        }
        if closed { self.events = None; }
        for e in queued { self.route(&e, now); }
    }

    fn route(&mut self, e: &EngineEvent, now: DateTime<Utc>) {
        for s in self.slots.iter_mut().filter(|s| s.job.wants(e)) { s.push(Wake::Event(e.clone()), now); }
    }

    /// Runs slot `i` once, within its deadline, and records its outcome. False: a stop arrived mid-run (the run was
    /// dropped, nothing recorded).
    async fn run_slot(&mut self, i: usize, stop: &mut watch::Receiver<bool>, clock: &dyn Clock) -> bool {
        let now = clock.now();
        let s = &mut self.slots[i];
        let fired = s.scheduled.is_some_and(|t| t <= now);
        let mut wakes = std::mem::take(&mut s.replay);
        s.retry_at = None; // a wake before the retry's time runs the retry with it
        if fired { wakes.push(Wake::Schedule); }
        wakes.append(&mut s.pending);
        s.pending_since = None;
        let (name, scope, deadline) = (s.spec.name, s.spec.scope.clone(), s.spec.deadline);
        let run = tokio::time::timeout(deadline, self.slots[i].job.run(Cx { db: self.db.clone(), clock }, wakes.clone()));
        let outcome = tokio::select! {
            o = run => o.unwrap_or_else(|_| Outcome::Failed(format!("dropped past its {deadline:?} deadline"))),
            _ = stop.wait_for(|stopped| *stopped) => {
                log(&format!("[jobs] {name} {scope:?}: stopped mid-run; a scheduled run is due again at the next start"));
                return false;
            }
        };
        let end = clock.now();
        let s = &mut self.slots[i];
        if fired { s.scheduled = s.spec.schedule.map(|sch| sch.next_fire(end) + s.spec.jitter.draw()); }
        let (count, message) = match outcome {
            Outcome::Done | Outcome::NothingNew => {
                (s.transient, s.rate) = (0, 0);
                let what = if outcome == Outcome::Done { Record::Success } else { Record::Run };
                log(&format!("[jobs] {name} {scope:?}: {what:?}"));
                return self.record(name, scope, end, what, stop).await;
            }
            Outcome::Failed(m) => (None, m),
            Outcome::Transient(m) => { s.transient += 1; (Some(s.transient), m) }
            Outcome::RateLimited(m) => { s.rate += 1; (Some(s.rate), m) }
        };
        let line = match count.and_then(|n| s.spec.retry.wait(n)) {
            Some(wait) => {
                s.retry_at = Some(end + chrono::Duration::from_std(wait).expect("a short wait"));
                s.replay = wakes;
                format!("{message} (retrying in {} s)", wait.as_secs())
            }
            None => { (s.transient, s.rate) = (0, 0); message }
        };
        log(&format!("[jobs] {name} {scope:?}: {line}"));
        self.record(name, scope, end, Record::Error(line), stop).await
    }

    /// The runner's own state write, stop-aware: a stop that lands while it waits returns at once
    /// (false), and the record is skipped; the run is being dropped anyway, and a scheduled run is due again at the next
    /// start. (The abandoned write may still land within RECORD_BUSY if the lock frees: either way the record is true.)
    /// On the blocking pool, on a connection of its own, so never behind the job connection's mutex; it waits for SQLite's
    /// write lock at most RECORD_BUSY, then gives up (logged; the in-memory schedule still drives the next run).
    async fn record(&self, name: &'static str, scope: Option<String>, at: DateTime<Utc>, what: Record, stop: &mut watch::Receiver<bool>) -> bool {
        let write = move |c: &Connection| match &what {
            Record::Success => state::record_success(c, name, scope.as_deref(), at),
            Record::Run => state::record_run(c, name, scope.as_deref(), at),
            Record::Error(m) => state::record_error(c, name, scope.as_deref(), at, m),
        };
        let writing = async {
            match self.records.clone() {
                Some(path) => tokio::task::spawn_blocking(move || {
                    let c = Connection::open(&path).map_err(|e| e.to_string())?;
                    c.busy_timeout(RECORD_BUSY).map_err(|e| e.to_string())?;
                    write(&c)
                }).await.unwrap_or_else(|e| Err(format!("the blocking pool lost the write: {e}"))),
                None => self.db.run(move |c, _| write(c)).await,
            }
        };
        tokio::select! {
            written = writing => {
                if let Err(e) = written { log(&format!("[jobs] {name}: the state write failed: {e}")); }
                true
            }
            _ = stop.wait_for(|stopped| *stopped) => {
                log(&format!("[jobs] {name}: stopped while recording; the record is skipped"));
                false
            }
        }
    }
}

async fn next_event(rx: &mut Option<UnboundedReceiver<EngineEvent>>) -> Option<EngineEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}
