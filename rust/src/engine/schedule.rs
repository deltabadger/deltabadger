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

#[derive(Debug, Clone, PartialEq)]
pub enum Effective {
    /// `1.month` itself: the month branch, stepping by calendar months.
    Month,
    Seconds(f64),
    /// A smart split whose seconds equal 1.month's value: `== 1.month` is true, so Rails takes the month
    /// branch's `loop { checkpoint += duration }`, but the duration is a seconds Duration — it steps by
    /// 2 629 746 s, not by calendar months (Jan 31 10:00 → Mar 2 20:29:06).
    MonthSeconds(f64),
    /// A selling bot's Smart Intervals split (Bot::Reversible): a BigDecimal-valued Duration. Its #to_f steps the grid, but
    /// the interval count is `(Float / BigDecimal).ceil`, a decimal quotient, so the exact value is kept beside the Float.
    Decimal(f64, crate::ruby::BigDec),
}

impl Effective {
    /// `effective_interval_duration.to_f`.
    pub fn seconds(&self) -> f64 { match self { Self::Month => MONTH_SECONDS, Self::Seconds(s) | Self::MonthSeconds(s) | Self::Decimal(s, _) => *s } }
}

/// `effective_interval_duration`. Smart intervals divide the interval by quote/smart as floats, then `.seconds`.
pub use super::accounting::effective_interval as effective;

/// A stored setting's `.to_d`: an Integer exact, a Float by Float#to_d, a String by String#to_d (unreadable is 0); None for
/// anything else (nil, a boolean).
pub fn to_d(v: &serde_json::Value) -> Option<crate::ruby::BigDec> {
    use crate::ruby::BigDec;
    match v {
        serde_json::Value::Number(n) if n.is_i64() => n.as_i64().map(BigDec::from_i64),
        serde_json::Value::Number(n) => n.as_f64().and_then(|f| BigDec::from_f64(f).ok()),
        serde_json::Value::String(t) => Some(BigDec::parse(t.trim()).unwrap_or_else(|_| BigDec::zero())),
        _ => None,
    }
}

/// Bots::DcaMultiAsset#one_asset?: one key in `allocations` (a Hash with any), else one of `Array(base_asset_ids)`.
pub fn one_asset(s: &serde_json::Map<String, serde_json::Value>) -> bool {
    use serde_json::Value;
    match (s.get("allocations"), s.get("base_asset_ids")) {
        (Some(Value::Object(a)), _) if !a.is_empty() => a.len() == 1,
        (_, Some(Value::Array(ids))) => ids.len() == 1,
        (_, None | Some(Value::Null)) => false,
        _ => true,
    }
}

/// Bot::Reversible#effective_interval_duration while selling, from the stored settings: the sell interval (the buy interval
/// until one is picked), subdivided by the base split `sell_amount / smart_interval_base_amount` only for a base-denominated
/// sell (a one-asset basket with `sell_denomination` "base"; Bots::DcaMultiAsset#sell_denomination defaults to quote) with Smart Intervals on and a positive amount. None when the bot buys, or no interval reads: then the buy cadence
/// (`|| super`). Rails divides a Duration by `BigDecimal / Float`, so this is a decimal quotient (a float one misses
/// Rails' checkpoints: 3600 / (0.3 / 0.1) is 1200.0000000000002 in floats); tests/schedule.rs replays sell_vectors.json. The one sell-duration function: engine, schedule key and web read it.
pub fn sell_effective(s: &serde_json::Map<String, serde_json::Value>) -> Option<Effective> {
    use serde_json::Value;
    let present = |k: &str| s.get(k).filter(|v| !matches!(v, Value::Null) && v.as_str().is_none_or(|t| !t.trim().is_empty()));
    if present("direction").and_then(Value::as_str) != Some("selling") { return None; }
    let interval = present("sell_interval").or_else(|| present("interval")).and_then(Value::as_str).and_then(Interval::parse)?;
    use crate::ruby::BigDec;
    // `smart_interval_base_amount.to_f`, then Float#to_d as BigDecimal#/ coerces it.
    let float = |v: &Value| v.as_f64().or_else(|| v.as_str().map(|t| t.trim().parse::<f64>().unwrap_or(0.0))).unwrap_or(0.0);
    let base = one_asset(s) && present("sell_denomination").and_then(Value::as_str) == Some("base");
    let amount = present("sell_amount").and_then(to_d).filter(BigDec::is_positive);
    let split = present("smart_interval_base_amount").map(float);
    match (base && s.get("smart_intervaled") == Some(&Value::Bool(true)), amount, split) {
        (true, Some(amount), Some(split)) => {
            // `(base_duration / (sell_amount / split)).seconds`: two BigDecimal divisions; `== 1.month` compares that
            // BigDecimal exactly, and the checkpoints step by its #to_f (Float / BigDecimal coerces to Floats).
            let seconds = BigDec::from_f64(split).ok().and_then(|split| amount.div(&split))
                .and_then(|q| BigDec::from_i64(interval.seconds() as i64).div(&q))?;
            Some(if seconds == BigDec::from_i64(MONTH_SECONDS as i64) { Effective::MonthSeconds(MONTH_SECONDS) } else { Effective::Decimal(seconds.to_f(), seconds) })
        }
        _ => Some(if interval == Interval::Month { Effective::Month } else { Effective::Seconds(interval.seconds()) }),
    }
}

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
    let elapsed = time_minus(now_us.checked_sub(anchor_us).ok_or_else(time_range_error)?);
    let grid = |us: i64| Unrounded { base_us: us, terms: vec![] };
    let sum = |terms: &[(f64, i64)]| Unrounded { base_us: anchor_us, terms: terms.to_vec() };
    if anchor_us > now_us {
        // checkpoint.future? → the anchor itself; last steps back one interval the usual way.
        let last = match &eff {
            Effective::Month => grid(at(anchor_us)?.checked_sub_months(Months::new(1)).ok_or_else(time_range_error)?.timestamp_micros()),
            Effective::Seconds(d) | Effective::MonthSeconds(d) | Effective::Decimal(d, _) => sum(&[(*d, -1)]),
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
        Effective::Decimal(d, exact) => {
            // `((Time.current - checkpoint) / duration.seconds).ceil`: the Float elapsed coerced to BigDecimal, divided exactly as
            // Ruby does; then `checkpoint + n * duration.to_f`, an Integer times a Float.
            let n = crate::ruby::BigDec::from_f64(elapsed).ok().and_then(|e| e.div(&exact)).ok_or_else(time_range_error)?.ceil(0).to_f();
            let step = n * d;
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

/// `(Time.current - last_action_job_at) < effective_interval_duration`: a Float against the Duration's value (a BigDecimal
/// compares the Float as Float#to_d does).
pub fn within(elapsed_us: i64, eff: &Effective) -> bool {
    let elapsed = time_minus(elapsed_us);
    match eff {
        Effective::Decimal(_, exact) => crate::ruby::BigDec::from_f64(elapsed).is_ok_and(|e| &e < exact),
        _ => elapsed < eff.seconds(),
    }
}

/// `Time - Time` (rb_time_unmagnify_to_float): whole seconds exactly, else the nanosecond count as a double divided by 1e9.
fn time_minus(us: i64) -> f64 {
    let ns = i128::from(us) * 1000;
    if ns % 1_000_000_000 == 0 { (ns / 1_000_000_000) as f64 } else { ns as f64 / 1e9 }
}

/// `((last_interval_checkpoint_at.round(6) - calc_since.round(6)) / effective_interval_duration).floor + 1`
pub fn interval_count(last_us: i64, since_us: i64, eff: Effective) -> Result<i64, EngineError> {
    let count = ((last_us.checked_sub(since_us).ok_or_else(time_range_error)? as f64 / 1_000_000.0) / eff.seconds()).floor();
    if !count.is_finite() || count < i64::MIN as f64 || count >= i64::MAX as f64 { return Err(time_range_error()); }
    (count as i64).checked_add(1).ok_or_else(time_range_error)
}
