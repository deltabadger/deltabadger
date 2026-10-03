//! Every number a sync reads from an Alpaca answer or from the database, through a conversion that can fail and is
//! bounded before any arithmetic: a number of any length would otherwise be expanded digit by digit (`ruby::BigDec`),
//! on the blocking pool, where dropping the job cancels nothing.
//!
//! The caps are the engine's: for a venue's number 64 significant digits and a decimal exponent within ±40; for a
//! value read back from the database 512 digits and ±400; 256 characters of input either way. The engine's own bounded
//! `ruby::BigDec` is landing on main separately: when it has, `decimal` becomes a thin call to it with these caps.
use crate::ruby::BigDec;
use rusqlite::types::ValueRef;

pub struct Caps { pub chars: usize, pub digits: usize, pub exponent: i64 }
pub const VENUE: Caps = Caps { chars: 256, digits: 64, exponent: 40 };
pub const DATABASE: Caps = Caps { chars: 256, digits: 512, exponent: 400 };

/// A plain decimal ("-12.5", ".5", "1e-8") within `caps`. Blank text, a sign alone, "NaN", "Infinity", digits with
/// separators and anything with a tail are errors. The error never repeats the input.
pub fn decimal(text: &str, caps: &Caps) -> Result<BigDec, String> {
    if text.len() > caps.chars { return Err(format!("longer than {} characters", caps.chars)); }
    let plain = "not a plain decimal number".to_string();
    let s = text.trim();
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) { Some((m, e)) => (m, Some(e)), None => (unsigned, None) };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits_only = |d: &str| d.bytes().all(|b| b.is_ascii_digit());
    if int.len() + frac.len() == 0 || !digits_only(int) || !digits_only(frac) { return Err(plain); }
    let exponent: i64 = match exponent {
        None => 0,
        Some(e) => {
            let d = e.strip_prefix(['+', '-']).unwrap_or(e);
            if d.is_empty() || d.len() > 6 || !digits_only(d) { return Err(plain); }
            e.parse().map_err(|_| plain.clone())?
        }
    };
    // The digits that carry value, and where the first of them stands: value = 0.d1d2… × 10^(magnitude + 1).
    let all: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
    if let Some(first) = all.iter().position(|b| *b != b'0') {
        let last = all.iter().rposition(|b| *b != b'0').unwrap_or(first);
        if last - first + 1 > caps.digits { return Err(format!("more than {} significant digits", caps.digits)); }
        let magnitude = int.len() as i64 - 1 - first as i64 + exponent;
        if magnitude.abs() > caps.exponent { return Err(format!("beyond 10^±{}", caps.exponent)); }
    } else {
        // A zero written with any exponent is zero; BigDec::parse refuses an exponent past ±656 whatever the digits.
        return Ok(BigDec::zero());
    }
    BigDec::parse(s).map_err(|_| plain)
}

/// `value&.to_d` for one JSON value as the venue wrote it (its text): `null` is none; a string is String#to_d (blank is
/// 0); an integer is exact; any other number is a Float (Float#to_d: 16 significant digits). A boolean, an array or an
/// object is an error, where Ruby raises NoMethodError.
pub fn json(token: &str) -> Result<Option<BigDec>, String> {
    let token = token.trim();
    match token.as_bytes().first() {
        None => Err("no value".into()),
        Some(b'n') if token == "null" => Ok(None),
        Some(b'"') => {
            let text: String = serde_json::from_str(token).map_err(|_| "not a JSON string".to_string())?;
            if text.trim().is_empty() { Ok(Some(BigDec::zero())) } else { decimal(&text, &VENUE).map(Some) }
        }
        Some(b'-' | b'0'..=b'9') => {
            // The caps are read off the token itself, before any Float: `1e-999` is beyond 10^-40, not the zero a
            // double makes of it (a quantity of zero would drop a holding and remove its balance).
            let exact = decimal(token, &VENUE)?;
            if token.bytes().all(|b| b == b'-' || b.is_ascii_digit()) { return Ok(Some(exact)); }
            let f: f64 = token.parse().map_err(|_| "not a number".to_string())?;
            BigDec::from_f64(f).map(Some).map_err(|_| "not a finite number".to_string())
        }
        _ => Err("not a number".into()),
    }
}

/// A decimal column as ActiveRecord reads it from SQLite (INTEGER, REAL or TEXT), within the database caps.
pub fn stored(v: ValueRef<'_>) -> Result<Option<BigDec>, String> {
    match v {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(i) => Ok(Some(BigDec::from_i64(i))),
        ValueRef::Real(f) => BigDec::from_f64(f).map(Some).map_err(|_| "a number in the database that is not finite".to_string()),
        ValueRef::Text(t) => decimal(std::str::from_utf8(t).map_err(|_| "text in a decimal column that is not UTF-8".to_string())?, &DATABASE).map(Some)
            .map_err(|why| format!("a number in the database that is {why}")),
        ValueRef::Blob(_) => Err("a blob in a decimal column".into()),
    }
}
