//! A local copy of the scheduler interface the syncs plug into: Plan 2f
//! (docs/superpowers/plans/2026-10-02-rust-scheduler-reference-data.md), section "Interface", as it stood on
//! 2026-10-02 at 18:05. Matched line for line: S-2 "Registering a job" (the code block at lines 95-139: `Job`,
//! `JobFuture`, `Cx`, `Db` with `run`'s exact signature, `Spec` with its `scope` and `deadline`, `DEADLINE`,
//! `Schedule`, `Jitter`, `Retry`, `Outcome`, `Wake`), S-5 "Engine events" (lines 202-212: `EngineEvent`) and S-10
//! "The data-api price lookup" (lines 345-353: `PriceSource`, `PriceFuture`; its `ApiError` is that plan's Task 5,
//! lines 2247-2262). That plan's code is not merged yet. When it is, delete this file and point every `job_api::` at
//! `crate::jobs::` (`EngineEvent` at `crate::engine::events::EngineEvent`, the price items at
//! `crate::jobs::data_api::`): `sync/jobs.rs` compiles unchanged.
//!
//! What that interface stands on is real code since Plan 2e merged (#449): `supervisor::Service` and
//! `supervisor::serve(.., services)` (its S-6a), the engine's `Shutdown::subscribe` (its N1) and `eligibility::guard`
//! (its S-7.3), which `sync::write` calls. Everything in this file (S-2, S-5, S-10) exists only in that plan's text.
//!
//! Additions, for building one without the scheduler: `Db::new` (2f builds its `Db` inside `Scheduler::new`), the
//! longest write hold of this `Db`'s sync units (`note_write_hold`, `longest_write_hold`), and the derives the tests
//! compare with. Left out: `CHUNK`, `Scheduler`, `Wakers`, `DataApi` and `ApiError::rate_limited`,
//! which no job here names.
use crate::crypto::Cipher;
use crate::engine::Clock;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

// ---- Plan 2f, S-2 ----

pub trait Job {
    fn spec(&self) -> Spec;
    /// Engine events this job runs on (S-5). None by default.
    fn wants(&self, _event: &EngineEvent) -> bool { false }
    /// One run. `wakes`: every reason this run happened since the last one, in arrival order (S-4).
    fn run<'a>(&'a self, cx: Cx<'a>, wakes: Vec<Wake>) -> JobFuture<'a>;
}
pub type JobFuture<'a> = Pin<Box<dyn Future<Output = Outcome> + 'a>>; // not Send: polled on the runtime thread

pub struct Cx<'a> { pub db: Db, pub clock: &'a dyn Clock }

/// The scheduler's own connection and the instance's cipher, used on tokio's blocking pool.
#[derive(Clone)]
pub struct Db { connection: Arc<Mutex<Connection>>, cipher: Arc<Cipher>, longest_hold_us: Arc<AtomicU64> }

impl Db {
    pub fn new(connection: Connection, cipher: Arc<Cipher>) -> Self {
        Self { connection: Arc::new(Mutex::new(connection)), cipher, longest_hold_us: Arc::new(AtomicU64::new(0)) }
    }
    /// The longest any sync write transaction on this `Db` (and its clones) has held SQLite's write lock, from the lock
    /// being taken to the commit returning: `sync::commit_bots` notes each one. Scoped to the `Db`, so one install's
    /// measurement never includes another's units.
    pub fn longest_write_hold(&self) -> Duration { Duration::from_micros(self.longest_hold_us.load(Ordering::Relaxed)) }
    pub fn note_write_hold(&self, held: Duration) { self.longest_hold_us.fetch_max(u64::try_from(held.as_micros()).unwrap_or(u64::MAX), Ordering::Relaxed); }
    /// Runs `f` on the blocking pool (as the web's App::db). Every SQLite statement and every large JSON walk of a job goes
    /// through here, never on the runtime thread. One call is one bounded unit (a read, or one chunk's transaction).
    pub async fn run<R: Send + 'static>(&self, f: impl FnOnce(&Connection, &Cipher) -> Result<R, String> + Send + 'static) -> Result<R, String> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || f(&db.connection.lock().unwrap_or_else(PoisonError::into_inner), &db.cipher))
            .await
            .map_err(|e| e.to_string())?
    }
}

pub struct Spec {
    pub name: &'static str,         // state key `rust_job.<name>`, log prefix, the name Wakers::wake takes
    pub scope: Option<String>,      // P2-7: Some for a job registered once per scope (an API key id): `rust_job.<name>:<scope>`
    pub schedule: Option<Schedule>, // None: on demand only
    pub jitter: Jitter,             // drawn afresh for every fire
    pub retry: Retry,               // ActiveJob's retry_on
    pub deadline: Duration,         // R1: the run's overall deadline; jobs::DEADLINE (10 min) unless the job declares its own
}
pub const DEADLINE: Duration = Duration::from_secs(600);
#[derive(Debug, PartialEq)]
pub enum Schedule { Daily { hour: u32, minute: u32 }, EveryHours { every: u32, minute: u32 } } // "M H * * *", "M */N * * *", UTC
#[derive(Debug, PartialEq)]
pub struct Jitter { pub min_secs: i64, pub max_secs: i64 } // rand(min..=max) s; Jitter::NONE
impl Jitter { pub const NONE: Self = Self { min_secs: 0, max_secs: 0 }; }
#[derive(Debug, PartialEq)]
pub enum Retry { None, Polynomial { attempts: u32 }, Fixed { wait_secs: u64, attempts: u32 } } // :polynomially_longer = n⁴+2 s
#[derive(Debug, PartialEq)]
pub enum Outcome { Done, NothingNew, Failed(String), Transient(String), RateLimited(String) } // NothingNew: ran well, refreshed nothing
#[derive(Clone, Debug, PartialEq)]
pub enum Wake { Schedule, Manual(Option<i64>), Event(EngineEvent) }

// ---- Plan 2f, S-5 ----

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// A `transactions` row this engine inserted (submitted, failed or skipped) or recovered.
    OrderRecorded { bot_id: i64, transaction_id: i64 },
    /// The engine stamped bots.last_end_of_funds_notification.
    FundsLow { bot_id: i64, user_id: i64, quote_asset_id: Option<i64> },
}

// ---- Plan 2f, S-10 ----

pub trait PriceSource {
    /// Prices in `currency` by external id; ids data-api does not price are absent. Err: the request failed.
    fn prices<'a>(&'a self, external_ids: &'a [String], currency: &'a str) -> PriceFuture<'a>;
}
pub type PriceFuture<'a> = Pin<Box<dyn Future<Output = Result<BTreeMap<String, f64>, ApiError>> + 'a>>; // not Send

#[derive(Debug, PartialEq)]
pub enum ApiError {
    /// Client.network_failure raised Client::TransientNetworkError: the request got no answer.
    Transient(String),
    /// A Result::Failure: an HTTP error (`status`), an unreadable body, or a permanent transport failure (no status).
    Failed { status: Option<u16>, message: String },
}

impl ApiError {
    pub fn message(&self) -> String {
        match self { Self::Transient(m) | Self::Failed { message: m, .. } => m.clone() }
    }
}
