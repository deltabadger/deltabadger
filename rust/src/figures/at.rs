//! An instant as these walks hold it: Ruby's Time, to the nanosecond.
use crate::codec::parse_time;
use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug)]
pub struct At(pub i64);

// Compared by its nanoseconds, and every comparison is a step of the figure's budget: a pass that searches the
// instants so far once per order shows in the count, whatever the machine (rust/examples/figures_limits.rs).
impl PartialEq for At { fn eq(&self, o: &At) -> bool { super::budget::step(1); self.0 == o.0 } }
impl Eq for At {}
impl PartialOrd for At { fn partial_cmp(&self, o: &At) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) } }
impl Ord for At { fn cmp(&self, o: &At) -> std::cmp::Ordering { super::budget::step(1); self.0.cmp(&o.0) } }
impl std::hash::Hash for At { fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.0.hash(state); } }

impl At {
    /// None outside the years 1678 to 2261, which nanoseconds in 64 bits cannot hold.
    pub fn from_utc(t: DateTime<Utc>) -> Option<At> { t.timestamp_nanos_opt().map(At) }
    pub fn utc(self) -> DateTime<Utc> { DateTime::from_timestamp_nanos(self.0) }
    /// A datetime column as Rails wrote it (UTC, microseconds when it has any).
    pub fn from_sql(text: &str) -> Option<At> { parse_time(text).ok().and_then(At::from_utc) }
    pub fn plus_seconds(self, seconds: i64) -> Option<At> { seconds.checked_mul(1_000_000_000).and_then(|n| self.0.checked_add(n)).map(At) }
    /// `Time - Time`: a Float. Whole seconds are exact; otherwise Ruby divides the nanoseconds by 1e9 as doubles.
    pub fn minus(self, other: At) -> f64 {
        let nanos = i128::from(self.0) - i128::from(other.0);
        if nanos % 1_000_000_000 == 0 { (nanos / 1_000_000_000) as f64 } else { nanos as f64 / 1e9 }
    }
}
