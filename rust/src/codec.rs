//! How Rails stores values in SQLite, and the conversions that match it.
use chrono::{DateTime, NaiveDateTime, Timelike, Utc};
use rusqlite::types::ValueRef;
use rust_decimal::Decimal;
use std::str::FromStr;

#[derive(Debug)]
pub enum CodecError {
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

pub fn parse_time(s: &str) -> Result<DateTime<Utc>, CodecError> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .map(|n| n.and_utc())
        .map_err(|e| CodecError::Time(format!("{s:?}: {e}")))
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
