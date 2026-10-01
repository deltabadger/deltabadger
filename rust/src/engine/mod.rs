//! The bot engine (spec §3): one-asset Kraken DCA baskets, decided exactly as Rails decides them.
use chrono::{DateTime, Utc};

pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> { Utc::now() }
}

pub struct FixedClock(pub DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> { self.0 }
}

/// Advances by `step` on every read: stands in for time passing during awaited calls.
pub struct SteppingClock { now: std::cell::Cell<DateTime<Utc>>, step: chrono::Duration }
impl SteppingClock {
    pub fn new(start: DateTime<Utc>, step: chrono::Duration) -> Self { Self { now: std::cell::Cell::new(start), step } }
}
impl Clock for SteppingClock {
    fn now(&self) -> DateTime<Utc> { let t = self.now.get(); self.now.set(t + self.step); t }
}

#[derive(Debug)]
pub enum EngineError {
    Sqlite(rusqlite::Error),
    /// A stored value this build cannot read the way Rails wrote it.
    Data(String),
    Ineligible(Vec<String>),
    /// Bots whose order Kraken could not yet account for (handback refuses while any exist).
    Unresolved(Vec<i64>),
    Lease(crate::lease::LeaseError),
    Store(crate::store::StoreError),
}
impl From<rusqlite::Error> for EngineError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<crate::lease::LeaseError> for EngineError { fn from(e: crate::lease::LeaseError) -> Self { Self::Lease(e) } }
impl From<crate::store::StoreError> for EngineError { fn from(e: crate::store::StoreError) -> Self { Self::Store(e) } }
pub mod schedule;
pub mod model;
pub mod eligibility;
pub mod amount;
pub mod placement;
pub mod polling;
pub mod kraken_errors;
pub mod tick;
pub mod handover;
