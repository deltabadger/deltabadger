//! The bot engine: one-asset DCA baskets (Kraken, Alpaca paper), decided exactly as Rails decides them.
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

/// One line to stdout, UTC-timestamped: `docker logs` is the engine's console.
pub fn log(line: &str) { println!("{} {line}", Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ")); }

#[derive(Debug)]
pub enum EngineError {
    Sqlite(rusqlite::Error),
    /// A stored value this build cannot read the way Rails wrote it.
    Data(String),
    Ineligible(Vec<String>),
    /// Bots whose order the venue could not yet account for (handback refuses while any exist).
    Unresolved(Vec<i64>),
    /// A requested stop (SIGTERM/SIGINT): the tick in hand finished and nothing new started.
    Stopped,
    Lease(crate::lease::LeaseError),
    Store(crate::store::StoreError),
}
impl From<rusqlite::Error> for EngineError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<crate::lease::LeaseError> for EngineError { fn from(e: crate::lease::LeaseError) -> Self { Self::Lease(e) } }
impl From<crate::store::StoreError> for EngineError { fn from(e: crate::store::StoreError) -> Self { Self::Store(e) } }
pub mod schedule;
pub mod model;
pub mod eligibility;
pub mod events;
pub mod amount;
pub mod placement;
pub mod polling;
pub mod venue_rules;
pub mod tick;
pub mod handover;
pub mod run;
