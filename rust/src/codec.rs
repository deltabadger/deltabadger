//! How Rails stores values in SQLite, and the conversions that match it.
use chrono::{DateTime, NaiveDateTime, Timelike, Utc, TimeZone};
use rusqlite::types::ValueRef;
use rust_decimal::Decimal;
use std::str::FromStr;

#[derive(Debug)]
pub enum CodecError {
    /// R7: refuse fixed-width monetary overflow; never emulate Ruby bignums.
    IntegerOverflow(&'static str),
    Time(String),
    Decimal(String),
}

/// ActiveRecord's `quoted_date`: microseconds only when non-zero.
pub fn format_time(t: DateTime<Utc>) -> String {
    if t.nanosecond() / 1_000 == 0 {
        t.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        t.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
    }
}


/// R9: one fallible stored-timestamp parser. Zone-less ActiveRecord SQL times are UTC.
/// Explicit offsets retain their instant; only Rails SQL and RFC3339 shapes are admitted.
pub fn parse_time(s: &str) -> Result<DateTime<Utc>, CodecError> {
    parse_stored_time(s).map(|at|at.with_timezone(&Utc))
}
pub fn parse_time_offset(s:&str) -> Result<DateTime<chrono::FixedOffset>,CodecError> {parse_stored_time(s)}
fn parse_stored_time(s: &str) -> Result<DateTime<chrono::FixedOffset>, CodecError> {
    // R9c: keep the base SQL grammar, calendar range and diagnostic verbatim.
    // Do not pre-filter widths, signed years, fractional precision or leap seconds.
    let sql = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"));
    match sql {
        Ok(at) => Ok(at.and_utc().fixed_offset()),
        Err(error) => {
            if let Ok(at) = DateTime::parse_from_rfc3339(s) { return Ok(at); }
            for pattern in ["%Y-%m-%d %H:%M:%S%.f %:z", "%Y-%m-%d %H:%M:%S%.f %z",
                            "%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%d %H:%M:%S%.f%z"] {
                if let Ok(at) = DateTime::parse_from_str(s, pattern) { return Ok(at); }
            }
            if let Some(local) = s.strip_suffix(" UTC").or_else(|| s.strip_suffix('Z')) {
                if let Ok(at) = NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S%.f") {
                    return Ok(at.and_utc().fixed_offset());
                }
            }
            Err(CodecError::Time(format!("{s:?}: {error}")))
        }
    }
}
/// Missing/NULL is absence; a present malformed value is never silently turned into absence.
pub fn optional_time(value: Option<&serde_json::Value>) -> Result<Option<DateTime<Utc>>, CodecError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => parse_time(text).map(Some),
        Some(_) => Err(CodecError::Time("unreadable stored timestamp".into())),
    }
}
pub fn validate_bot_times(settings:&serde_json::Value, transient:&serde_json::Value) -> Result<(),CodecError> {
    optional_time(settings.get("start_at"))?;
    for key in ["last_action_job_at", "quote_amount_limit_enabled_at", "base_amount_limit_enabled_at"] {
        optional_time(transient.get(key))?;
    }
    for prefix in ["", "sell_"] {
        for rule in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
            for suffix in ["enabled_at", "condition_met_at"] { optional_time(transient.get(format!("{prefix}{rule}_{suffix}")))?; }
        }
    }
    if let Some(values)=transient.get("failure_notifications").and_then(serde_json::Value::as_object) {
        for value in values.values() {optional_time(Some(value))?;}
    }
    // No writer stores a null deferral marker (it is removed), so a present null is damage, as in Bot::rust_defer.
    if transient.get("rust_defer_until").is_some_and(serde_json::Value::is_null) {return Err(CodecError::Time("null defer marker".into()));}
    for key in ["rust_defer_until", "rust_placement", "rust_continue_start"] {
        if let Some(value)=transient.get(key).filter(|v| !v.is_null()) {
            for time in if key=="rust_defer_until" {&["until"][..]} else if key=="rust_placement" {&["deadline", "at"][..]} else {&["requested_at"][..]} {
                let at=optional_time(value.get(time))?;
                if key=="rust_continue_start" && at.is_none() {return Err(CodecError::Time("missing continue timestamp".into()));}
                if key=="rust_defer_until" && at.is_none() {return Err(CodecError::Time("missing defer timestamp".into()));}
                if key=="rust_defer_until" && value.get("schedule").and_then(serde_json::Value::as_str).is_none() {return Err(CodecError::Time("missing defer schedule".into()));}
            }
        }
    }
    Ok(())
}

/// SQLite gives a `decimal` column NUMERIC affinity, so Rails' decimal text comes back as REAL or
/// INTEGER. Rails then casts a Float with `to_d`, i.e. `BigDecimal(float, 0)`.
pub fn decimal_from_sql(v: ValueRef<'_>) -> Result<Option<Decimal>, CodecError> {
    match v {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(i) => Ok(Some(Decimal::from(i))),
        ValueRef::Real(f) => real_to_decimal(f).map(Some),
        ValueRef::Text(t) => {
            let s = std::str::from_utf8(t).map_err(|e| CodecError::Decimal(e.to_string()))?;
            Decimal::from_str(s).or_else(|_| Decimal::from_scientific(s)).map(|d| Some(d.normalize())).map_err(|e| CodecError::Decimal(format!("{s}: {e}")))
        }
        ValueRef::Blob(_) => Err(CodecError::Decimal("blob in a decimal column".into())),
    }
}

/// Rails' `BigDecimal(float, 0)`: the shortest round-trip digits of the float, which bigdecimal
/// truncates (not rounds) to 16 significant digits. `{:e}` prints exactly those shortest digits.
pub fn real_to_decimal(f: f64) -> Result<Decimal, CodecError> {
    let err = || CodecError::Decimal(format!("{f:e} is outside the decimal range"));
    if !f.is_finite() { return Err(err()); }
    let sci = format!("{f:e}"); // e.g. "-1.2345678912345679e8"
    let (mantissa, exp) = sci.split_once('e').ok_or_else(err)?;
    let exp: i64 = exp.parse().map_err(|_| err())?;
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).take(16).collect();
    let mut int: i128 = digits.parse().map_err(|_| err())?;
    let mut scale = digits.len() as i64 - 1 - exp; // fractional digits of int × 10^-scale
    while scale < 0 {
        int = int.checked_mul(10).ok_or_else(err)?;
        scale += 1;
    }
    if scale > 28 || int > 79_228_162_514_264_337_593_543_950_335 { return Err(err()); }
    let d = Decimal::from_i128_with_scale(if mantissa.starts_with('-') { -int } else { int }, scale as u32);
    Ok(d.normalize())
}

pub fn decimal_to_sql(d: Decimal) -> String {
    d.normalize().to_string()
}

use chrono::{Datelike, Duration, LocalResult, NaiveDate};
use serde_json::{Value,json};
use crate::web::WebError;
pub fn parse_form_time(value: &Value, name: &str, now: DateTime<Utc>) -> Result<Value, WebError> {
    let Some(input) = value.as_str() else { return Ok(Value::Null) };
    if input.len() > 128 { return Err(WebError::Engine(crate::engine::EngineError::Data("start date exceeds its bound".into()))); }
    let s = input.trim();
    if s.is_empty() { return Ok(Value::Null); }
    if let Ok(at) = DateTime::parse_from_rfc3339(s) { return Ok(json!(at.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))); }
    let zone = crate::web::timezone::zone(name).unwrap_or(chrono_tz::UTC);
    let naive = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter().find_map(|pattern| NaiveDateTime::parse_from_str(s, pattern).ok())
        .or_else(|| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0))
        .or_else(|| {
            // Date._parse's compact numeric fragments: decimal-separated digits are a
            // right-aligned HHMMSS, while a three-digit run supplies only yday. Time.zone.parse
            // ignores yday when it supplies the missing calendar fields from today's date.
            let day = now.with_timezone(&zone).date_naive();
            let runs: Vec<&str> = s.split(|ch: char| !ch.is_ascii_digit()).filter(|part| !part.is_empty()).collect();
            if let Some((whole, fraction)) = s.split_once(['.', ',']) {
                if !fraction.starts_with(|ch: char| ch.is_ascii_digit()) || whole.is_empty() || whole.len() > 6 || !whole.bytes().all(|b| b.is_ascii_digit()) { return None; }
                let n = whole.parse::<u32>().ok()?;
                return day.and_hms_opt(n / 10000, n / 100 % 100, n % 100);
            }
            if runs.last().is_some_and(|run| run.len() == 3) { return day.and_hms_opt(0, 0, 0); }
            if runs.len() == 1 && s.bytes().all(|b| b.is_ascii_digit()) && s.len() <= 2 {
                return day.with_day(s.parse().ok()?)?.and_hms_opt(0, 0, 0);
            }
            None
        });
    let Some(naive) = naive else { return Ok(Value::Null) };
    if !(1..=9999).contains(&naive.year()) { return Err(WebError::Engine(crate::engine::EngineError::Data("start date exceeds its bound".into()))); }
    // ActiveSupport chooses DST on overlap, and moves a nonexistent local time forward an hour.
    let at = match zone.from_local_datetime(&naive) {
        LocalResult::Single(at) => Some(at), LocalResult::Ambiguous(a, b) => Some(a.min(b)),
        LocalResult::None => naive.checked_add_signed(Duration::hours(1)).and_then(|next| zone.from_local_datetime(&next).earliest()),
    };
    Ok(at.map_or(Value::Null, |at| json!(at.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))))
}
