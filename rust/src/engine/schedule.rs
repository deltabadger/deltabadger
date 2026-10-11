//! Automation::Schedulable + Bot::SmartIntervalable, as Rails computes them (UTC, jobs pinned to UTC).
use crate::ruby::{exceeds, checked_round6_micros};
use super::EngineError;
use chrono::{DateTime, Months, Utc};

pub(super) const MONTH_SECONDS: f64 = 2_629_746.0; // ActiveSupport 1.month

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Interval { Hour, Day, Week, Month }

impl Interval {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s { "hour" => Self::Hour, "day" => Self::Day, "week" => Self::Week, "month" => Self::Month, _ => return None })
    }
    pub fn as_str(&self) -> &'static str {
        match self { Self::Hour => "hour", Self::Day => "day", Self::Week => "week", Self::Month => "month" }
    }
    pub(super) fn seconds(&self) -> f64 {
        match self { Self::Hour => 3_600.0, Self::Day => 86_400.0, Self::Week => 604_800.0, Self::Month => MONTH_SECONDS }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effective {
    /// `1.month` itself: the month branch, stepping by calendar months.
    Month,
    Seconds(f64),
    /// A smart split whose seconds equal 1.month's value: `== 1.month` is true, so Rails takes the month
    /// branch's `loop { checkpoint += duration }`, but the duration is a seconds Duration — it steps by
    /// 2 629 746 s, not by calendar months (Jan 31 10:00 → Mar 2 20:29:06).
    MonthSeconds(f64),
}

impl Effective {
    /// `effective_interval_duration.to_f`.
    pub fn seconds(&self) -> f64 { match self { Self::Month => MONTH_SECONDS, Self::Seconds(s) | Self::MonthSeconds(s) => *s } }
}

/// `effective_interval_duration`. Smart intervals divide the interval by quote/smart as floats, then `.seconds`.
pub use super::accounting::effective_interval as effective;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkpoints { pub next_us: i64, pub last_us: i64 }

impl Checkpoints {
    pub fn validate_times(&self) -> Result<(), EngineError> {
        at(self.next_us)?;
        at(self.last_us)?;
        Ok(())
    }
}

pub fn time_range_error() -> EngineError { EngineError::Data("schedule time is outside the representable range; bot refused for this tick".into()) }
fn at(us: i64) -> Result<DateTime<Utc>, EngineError> { DateTime::from_timestamp_micros(us).ok_or_else(time_range_error) }

/// A checkpoint as Rails holds it before any rounding: a time on the microsecond grid plus Floats added to it, each
/// `k` times. `Time + Float` adds the Float's exact binary value, so the sum is exact and is kept as its parts.
#[derive(Debug, Clone, PartialEq)]
pub struct Unrounded { pub base_us: i64, pub terms: Vec<(f64, i64)> }

impl Unrounded {
    /// `.round(6)`, in microseconds.
    pub fn rounded(&self) -> Result<i64, EngineError> { checked_round6_micros(self.base_us, &self.terms).ok_or_else(time_range_error) }
}

/// `next_interval_checkpoint_at` and `last_interval_checkpoint_at`, both `.round(6)`.
pub fn checkpoints(anchor_us: i64, now_us: i64, eff: Effective) -> Result<Checkpoints, EngineError> {
    let (next, last) = unrounded(anchor_us, now_us, eff)?;
    Ok(Checkpoints { next_us: next.rounded()?, last_us: last.rounded()? })
}

/// The same two checkpoints before `.round(6)`: what Rails hands on when it enqueues the next job
/// (`wait_until: next_interval_checkpoint_at`) and measures the progress bar from.
pub fn unrounded(anchor_us: i64, now_us: i64, eff: Effective) -> Result<(Unrounded, Unrounded), EngineError> {
    at(anchor_us)?;
    at(now_us)?;
    if !eff.seconds().is_finite() || eff.seconds() <= 0.0 { return Err(time_range_error()); }
    let elapsed = now_us.checked_sub(anchor_us).ok_or_else(time_range_error)? as f64 / 1_000_000.0;
    let grid = |us: i64| Unrounded { base_us: us, terms: vec![] };
    let sum = |terms: &[(f64, i64)]| Unrounded { base_us: anchor_us, terms: terms.to_vec() };
    if anchor_us > now_us {
        // checkpoint.future? → the anchor itself; last steps back one interval the usual way.
        let last = match eff {
            Effective::Month => grid(at(anchor_us)?.checked_sub_months(Months::new(1)).ok_or_else(time_range_error)?.timestamp_micros()),
            Effective::Seconds(d) | Effective::MonthSeconds(d) => sum(&[(d, -1)]),
        };
        return Ok((grid(anchor_us), last));
    }
    Ok(match eff {
        Effective::Month => {
            let mut checkpoint = at(anchor_us)?;
            loop {
                checkpoint = checkpoint.checked_add_months(Months::new(1)).ok_or_else(time_range_error)?; // clamps the day, and the clamp sticks
                if checkpoint.timestamp_micros() > now_us { break; }
            }
            let last = checkpoint.checked_sub_months(Months::new(1)).ok_or_else(time_range_error)?;
            (grid(checkpoint.timestamp_micros()), grid(last.timestamp_micros()))
        }
        Effective::Seconds(d) => {
            let n = (elapsed / d).ceil();
            let step = n * d; // Ruby: intervals_since_checkpoint * duration.to_f, a float product
            (sum(&[(step, 1)]), sum(&[(step, 1), (d, -1)]))
        }
        Effective::MonthSeconds(d) => {
            // loop { checkpoint += d; return checkpoint if checkpoint > now }: the first k with anchor + k·d > now, exactly.
            let mut k = (elapsed / d).floor() as i64;
            while exceeds(anchor_us, (d, k), now_us) { k -= 1; }
            while !exceeds(anchor_us, (d, k), now_us) || k < 1 { k += 1; }
            (sum(&[(d, k)]), sum(&[(d, k - 1)]))
        }
    })
}

/// `((last_interval_checkpoint_at.round(6) - calc_since.round(6)) / effective_interval_duration).floor + 1`
pub fn interval_count(last_us: i64, since_us: i64, eff: Effective) -> Result<i64, EngineError> {
    let count = ((last_us.checked_sub(since_us).ok_or_else(time_range_error)? as f64 / 1_000_000.0) / eff.seconds()).floor();
    if !count.is_finite() || count < i64::MIN as f64 || count >= i64::MAX as f64 { return Err(time_range_error()); }
    (count as i64).checked_add(1).ok_or_else(time_range_error)
}
