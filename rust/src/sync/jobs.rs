//! The syncs as jobs of the engine's scheduler (`crate::jobs`, Plan 2f): one ledger job and one balance
//! job per API key, `ledger_sync` and `balance_sync` with the key's id as 2f's scope, so each has its own record
//! (`app_configs` row `rust_job.ledger_sync:<api_key_id>`). The runner, not the job, writes that record and enforces
//! the deadline: a job only returns its `Outcome`. A run with nothing to import is `Done`, so a first success differs
//! from "never ran".
use super::balances;
use crate::engine::events::EngineEvent;
use crate::jobs::data_api::PriceSource;
use crate::jobs::schedule::{Jitter, Schedule};
use crate::jobs::{Cx, Db, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use super::{credentials, ledger, reading_keys, Failure, SyncError, ALPACA};
use crate::crypto::Credentials;
use crate::venue::alpaca::{AlpacaVenue, LiveFactory};
use crate::venue::http::{ReqwestTransport, Transport};
use crate::venue::VenueFactory;
use rusqlite::Connection;
use std::rc::Rc;
use std::time::Duration;

pub const LEDGER_SYNC: &str = "ledger_sync";
pub const BALANCE_SYNC: &str = "balance_sync";
/// A ledger sync's own deadline (2f R1 lets it declare one): a first sync reads an account's whole history, a page
/// of 100 per request. Past it the runner drops the run at an await; nothing is half written, and the next run
/// starts from the stored watermark.
pub const LEDGER_DEADLINE: Duration = Duration::from_secs(3600);

/// How a job reaches Alpaca for one key. `LiveFactory` is the real one (paper host only; a live key gets a transport
/// that sends nothing).
pub trait Connect {
    type T: Transport;
    fn connect(&self, credentials: &Credentials) -> AlpacaVenue<Self::T>;
}
impl Connect for LiveFactory {
    type T = ReqwestTransport;
    fn connect(&self, credentials: &Credentials) -> AlpacaVenue<ReqwestTransport> { self.for_bot(ALPACA, Some(credentials.clone())) }
}

async fn key_credentials(db: &Db, key: i64) -> Result<Credentials, SyncError> {
    db.run(move |c, cipher| credentials(c, cipher, key).map_err(|e| e.0)).await.map_err(SyncError)
}

/// `Done` is a sync that reached the end of what the venue has, with or without anything to import: its
/// `last_success_at` is what tells a key's first successful sync from "never ran" (Plan 2d). `NothingNew` is an
/// import that stopped at its page cap with more to read: the run went well and the ledger is not yet whole, so the
/// runner records the run and no success, and the next run continues. Never `Transient`: Rails' jobs declare no retry.
fn outcome<T>(done: Result<Result<T, Failure>, SyncError>, complete: impl Fn(&T) -> bool) -> Outcome {
    match done {
        Ok(Ok(done)) if !complete(&done) => Outcome::NothingNew,
        Ok(Ok(_)) => Outcome::Done,
        Ok(Err(failure)) => Outcome::Failed(failure.error),
        Err(SyncError(e)) => Outcome::Failed(e),
    }
}

/// One run of a job outside the scheduler (the hand-run command), held to the deadline the scheduler's runner would
/// hold it to: past `Spec::deadline` the run is dropped at its next await and reported in the runner's words.
pub async fn run_within_deadline(job: &dyn Job, cx: Cx<'_>, wakes: Vec<Wake>) -> Outcome {
    let deadline = job.spec().deadline;
    match tokio::time::timeout(deadline, job.run(cx, wakes)).await {
        Ok(outcome) => outcome,
        Err(_) => Outcome::Failed(format!("dropped past its {}s deadline", deadline.as_secs())),
    }
}

/// `ledger_sync`, scoped by the key: Rails' `AccountTransaction::SyncJob` for one key. It runs nightly at 02:00 UTC
/// (`AccountTransaction::SyncAllJob`'s fan-out, recurring key `sync_all_account_transactions_job`) and after every
/// order row the engine records (`Transaction`'s after_create_commit). No retry: Rails' job declares none, and the
/// next order or the next night is the retry. An import longer than one run wakes itself and goes on at once.
pub struct LedgerSync<C: Connect> { venues: C, key_id: i64, limits: ledger::Limits }

impl<C: Connect> LedgerSync<C> {
    pub fn new(venues: C, key_id: i64) -> Self { Self { venues, key_id, limits: ledger::Limits::RUN } }
    /// The same job with a run of fewer pages (tests: an import that takes several runs).
    pub fn within(self, limits: ledger::Limits) -> Self { Self { limits, ..self } }
}

impl<C: Connect> Job for LedgerSync<C> {
    fn spec(&self) -> Spec {
        Spec { name: LEDGER_SYNC, scope: Some(self.key_id.to_string()), schedule: Some(Schedule::Daily { hour: 2, minute: 0 }), jitter: Jitter::NONE, retry: Retry::None,
               deadline: LEDGER_DEADLINE }
    }
    /// Every order, whatever its status and whichever bot placed it: an event names a bot, and which key that is
    /// takes the database. On an install with several keys each key's job runs and the others find nothing new.
    fn wants(&self, event: &EngineEvent) -> bool { matches!(event, EngineEvent::OrderRecorded { .. }) }
    /// Whatever woke it (the schedule, a manual wake, any number of orders), a run is one sync of this key.
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            let credentials = match key_credentials(&cx.db, self.key_id).await { Ok(c) => c, Err(SyncError(e)) => return Outcome::Failed(e) };
            let key = self.key_id.to_string();
            let at = cx.clock.now();
            if let Err(e) = cx.db.run(move |c, _| crate::jobs::state::mark_incomplete(c, LEDGER_SYNC, Some(&key), at)).await { return Outcome::Failed(e); }
            let outcome = outcome(ledger::sync_within(&cx.db, &self.venues.connect(&credentials), self.key_id, &credentials, cx.clock, self.limits).await, |out| out.complete);
            // S-7.7: an import that stopped at its page cap goes on as soon as the runner is free, as Rails' one job reads the
            // whole history at once; other due jobs run in between.
            if outcome == Outcome::NothingNew { cx.wakers.wake(LEDGER_SYNC, Some(&self.key_id.to_string()), None); }
            // Rails' SyncJob ends in a Tracker::LedgerJob: the user's tracker walks again once the ledger is whole.
            else { crate::tracker::jobs::wake_for_key(&cx, self.key_id).await; }
            outcome
        })
    }
}

/// `balance_sync`, scoped by the key: Rails' `AccountBalance::SyncJob` for one key. It runs nightly at 02:30 UTC
/// (`AccountBalance::SyncAllJob`'s fan-out, recurring key `sync_all_account_balances_job`), after the ledger, or on a
/// manual wake. No retry. The price source is the scheduler's one data-api client, shared (2f S-10).
pub struct BalanceSync<C: Connect> { venues: C, prices: Rc<dyn PriceSource>, key_id: i64 }

impl<C: Connect> BalanceSync<C> {
    pub fn new(venues: C, prices: Rc<dyn PriceSource>, key_id: i64) -> Self { Self { venues, prices, key_id } }
}

impl<C: Connect> Job for BalanceSync<C> {
    fn spec(&self) -> Spec {
        Spec { name: BALANCE_SYNC, scope: Some(self.key_id.to_string()), schedule: Some(Schedule::Daily { hour: 2, minute: 30 }), jitter: Jitter::NONE, retry: Retry::None,
               deadline: DEADLINE }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            let credentials = match key_credentials(&cx.db, self.key_id).await { Ok(c) => c, Err(SyncError(e)) => return Outcome::Failed(e) };
            // As the ledger: from before the first write unit until a complete success, the key's balances may be half
            // written (a run dropped, or a hand run stopped, between two units); a start finding the mark runs the job at once.
            let (key, at) = (self.key_id.to_string(), cx.clock.now());
            if let Err(e) = cx.db.run(move |c, _| crate::jobs::state::mark_incomplete(c, BALANCE_SYNC, Some(&key), at)).await { return Outcome::Failed(e); }
            let outcome = outcome(balances::sync(&cx.db, &self.venues.connect(&credentials), self.prices.as_ref(), self.key_id, &credentials, cx.clock).await, |_| true);
            // Rails' AccountBalance::SyncJob ends in PortfolioSnapshot.record!, whatever the sync did: the user's tracker
            // rewrites today's rows.
            crate::tracker::jobs::wake_for_key(&cx, self.key_id).await;
            outcome
        })
    }
}

/// The jobs of an install, for the scheduler's start: per reading Alpaca key (`ApiKey.reading`, what Rails' nightly
/// jobs iterate) one ledger job and one balance job, every ledger job before every balance job, so jobs due together
/// run in Rails' order. Keys are read once, here: a key added or condemned later is picked up at the next start.
pub fn register<C: Connect + Clone + 'static>(c: &Connection, venues: &C, prices: Rc<dyn PriceSource>) -> Result<Vec<Box<dyn Job>>, SyncError> {
    let keys = reading_keys(c)?;
    let mut jobs: Vec<Box<dyn Job>> = vec![];
    for key in &keys { jobs.push(Box::new(LedgerSync::new(venues.clone(), *key))); }
    for key in &keys { jobs.push(Box::new(BalanceSync::new(venues.clone(), prices.clone(), *key))); }
    Ok(jobs)
}
