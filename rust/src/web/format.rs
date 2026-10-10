//! Numbers and times as Ruby and Rails print them into a page. Every function here is pinned by
//! vectors recorded from Ruby (script/rust/record_vectors.rb: "bot_pages").
use super::{i18n, timezone};
use super::WebError;
use crate::engine::EngineError;
use crate::ruby::{from_sql, to_sentence, BigDec};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::Value;

/// A number as Ruby holds one read from a JSON column or computed from one: the three classes
/// print, round and subtract differently, and the pages show the difference ("50", "50.0", "25.5").
#[derive(Clone, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
    Dec(BigDec),
}

/// Float#to_s: the shortest digits that read back as the same double, in plain notation for
/// exponents from -4 to 14 and as `1.0e+15` or `1.0e-05` beyond.
pub fn float_to_s(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() { return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }; }
    if f == 0.0 { return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() }; }
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point = exponent.parse::<i64>().unwrap_or(0) + 1; // digits before the decimal point
    let sign = if f < 0.0 { "-" } else { "" };
    let count = digits.len() as i64;
    if 0 < point && point <= 15 {
        if count <= point {
            format!("{sign}{digits}{}.0", "0".repeat((point - count) as usize))
        } else {
            let (whole, fraction) = digits.split_at(point as usize);
            format!("{sign}{whole}.{fraction}")
        }
    } else if -4 < point && point <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let (first, rest) = digits.split_at(1);
        let exponent = point - 1;
        format!("{sign}{first}.{}e{}{:02}", if rest.is_empty() { "0" } else { rest }, if exponent < 0 { "-" } else { "+" }, exponent.abs())
    }
}

/// libm's frexp exponent: `f = m * 2^e` with 0.5 <= |m| < 1.
fn binary_exponent(f: f64) -> i64 {
    let bits = ((f.to_bits() >> 52) & 0x7ff) as i64;
    if bits == 0 { f.abs().log2().floor() as i64 + 1 } else { bits - 1022 }
}

/// Float#round(digits) for digits > 0, as MRI's float.c computes it (round half up, with its two
/// shortcuts for a number that has no such digit or is nothing but such digits).
/// ponytail: up to 14 digits, where MRI multiplies by a power of ten as this does; beyond that it
/// goes through a Rational. The pages round to a ticker's decimals, and a bot whose tickers state
/// more than 14 is refused (`bot::MAX_DECIMALS`); port the Rational path before raising that.
pub fn float_round(f: f64, digits: i64) -> f64 {
    if f == 0.0 || !f.is_finite() { return f; }
    let exponent = binary_exponent(f);
    if digits >= 17 - if exponent > 0 { exponent / 4 } else { exponent / 3 - 1 } { return f; }
    if digits < -(if exponent > 0 { exponent / 3 + 1 } else { exponent / 4 }) { return 0.0; }
    let scale = 10f64.powi(digits.clamp(0, 14) as i32);
    let mut rounded = (f * scale).round();
    if f > 0.0 {
        if (rounded + 0.5) / scale <= f { rounded += 1.0; }
    } else if (rounded - 0.5) / scale >= f {
        rounded -= 1.0;
    }
    rounded / scale
}

impl Num {
    /// A JSON number as ActiveSupport::JSON decodes it: an Integer without a fraction or exponent, else a Float.
    pub fn from_json(value: &Value) -> Option<Num> {
        let number = value.as_number()?;
        number.as_i64().map(Num::Int).or_else(|| number.as_f64().map(Num::Float))
    }

    /// `to_d`: exact for an Integer, Float#to_d (16 significant digits) for a Float.
    pub fn to_d(&self) -> Option<BigDec> {
        match self {
            Num::Int(i) => Some(BigDec::from_i64(*i)),
            Num::Float(f) => BigDec::from_f64(*f).ok(),
            Num::Dec(d) => Some(d.clone()),
        }
    }

    pub fn to_f(&self) -> f64 {
        match self {
            Num::Int(i) => *i as f64,
            Num::Float(f) => *f,
            Num::Dec(d) => d.to_f(),
        }
    }

    /// `to_s`, with ActiveSupport's BigDecimal#to_s (plain notation).
    pub fn to_s(&self) -> String {
        match self {
            Num::Int(i) => i.to_string(),
            Num::Float(f) => float_to_s(*f),
            Num::Dec(d) => d.to_s_f(),
        }
    }

    /// `round(digits)` for digits >= 0: an Integer stays itself, a Float stays a Float, a BigDecimal rounds half up.
    pub fn round(&self, digits: u8) -> Num {
        match self {
            Num::Int(i) => Num::Int(*i),
            Num::Float(f) if digits > 0 => Num::Float(float_round(*f, i64::from(digits))),
            // Float#round(0) is an Integer in Ruby.
            Num::Float(f) => Num::Int(f.round() as i64),
            Num::Dec(d) => Num::Dec(d.round(digits)),
        }
    }

    pub fn is_positive(&self) -> bool {
        match self {
            Num::Int(i) => *i > 0,
            Num::Float(f) => *f > 0.0,
            Num::Dec(d) => d.is_positive(),
        }
    }

    /// `[self, 0].max`: itself unless it is negative, and then the Integer 0.
    pub fn at_least_zero(self) -> Num {
        let negative = match &self {
            Num::Int(i) => *i < 0,
            Num::Float(f) => *f < 0.0,
            Num::Dec(d) => *d < BigDec::zero(),
        };
        if negative { Num::Int(0) } else { self }
    }

    /// `self - other` with Ruby's coercion: Integer and Integer stay Integer, a Float on either side
    /// of an Integer makes a Float, and a BigDecimal on either side makes a BigDecimal (a Float is
    /// read through its shortest digits, as BigDecimal's coercion reads it).
    pub fn sub(&self, other: &Num) -> Option<Num> {
        let exact = |float: f64| BigDec::parse(&float_to_s(float)).ok();
        Some(match (self, other) {
            (Num::Int(a), Num::Int(b)) => Num::Int(a.checked_sub(*b)?),
            (Num::Dec(a), Num::Dec(b)) => Num::Dec(a - b),
            (Num::Dec(a), Num::Int(b)) => Num::Dec(a - &BigDec::from_i64(*b)),
            (Num::Int(a), Num::Dec(b)) => Num::Dec(&BigDec::from_i64(*a) - b),
            (Num::Dec(a), Num::Float(b)) => Num::Dec(a - &exact(*b)?),
            (Num::Float(a), Num::Dec(b)) => Num::Dec(&exact(*a)? - b),
            (a, b) => Num::Float(a.to_f() - b.to_f()),
        })
    }
}

/// String#to_i: leading ASCII whitespace, one sign, then digits, with an underscore allowed between
/// two digits; whatever follows is ignored, and a text with no number at its start is 0. It is how
/// Rails reads a bot's id out of a path (ActiveModel's integer type) and an id out of the feed's
/// cursor. Ruby has no largest Integer; this one stops at i128's, far past any id.
pub fn to_i(text: &str) -> i128 {
    let mut bytes = text.bytes().skip_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')).peekable();
    let negative = match bytes.peek() {
        Some(b'-') => { bytes.next(); true }
        Some(b'+') => { bytes.next(); false }
        _ => false,
    };
    let (mut number, mut after_digit) = (0i128, false);
    while let Some(byte) = bytes.next() {
        match byte {
            b'0'..=b'9' => {
                number = number.saturating_mul(10).saturating_add(i128::from(byte - b'0'));
                after_digit = true;
            }
            b'_' if after_digit && bytes.peek().is_some_and(u8::is_ascii_digit) => {}
            _ => break,
        }
    }
    if negative { -number } else { number }
}

/// `value.to_d.to_s('F').sub(/([0-9]\d*)\.0$/, '\1')`: how the settings forms write a stored number into an input.
pub fn input_value(number: &Num) -> Option<String> {
    let plain = number.to_d()?.to_s_f();
    Some(plain.strip_suffix(".0").map_or(plain.clone(), str::to_string))
}

/// `number_with_precision(number, precision:)`, with `delimiter: ','` when asked: rounded half up in
/// decimal (a Float through its `to_s`, as ActiveSupport's RoundingHelper converts it).
pub fn number_with_precision(number: &Num, precision: u8, delimiter: bool) -> Option<String> {
    let decimal = match number {
        Num::Float(f) => BigDec::parse(&float_to_s(*f)).ok()?,
        other => other.to_d()?,
    };
    let plain = decimal.round(precision).to_s_f();
    let (whole, fraction) = plain.split_once('.').unwrap_or((&plain, ""));
    let (sign, whole) = whole.strip_prefix('-').map_or(("", whole), |digits| ("-", digits));
    let mut grouped = String::new();
    for (position, digit) in whole.chars().enumerate() {
        if delimiter && position > 0 && (whole.len() - position) % 3 == 0 { grouped.push(','); }
        grouped.push(digit);
    }
    if precision == 0 { return Some(format!("{sign}{grouped}")); }
    let fraction: String = fraction.chars().chain(std::iter::repeat('0')).take(usize::from(precision)).collect();
    Some(format!("{sign}{grouped}.{fraction}"))
}

/// `distance_of_time_in_words(duration)`, which in this app is the dotiw gem's: every unit down to
/// the second, in the locale's words (`datetime.dotiw`, English where the gem has no file for the
/// locale), joined as an English sentence. Up to four weeks the words follow from the seconds
/// alone; from there on dotiw counts calendar months and days from the present moment.
pub fn distance_of_time_in_words(seconds: f64, now: DateTime<Utc>, locale: &str) -> String {
    const UNITS: [&str; 7] = ["years", "months", "weeks", "days", "hours", "minutes", "seconds"];
    let mut left = seconds.abs().trunc() as i64;
    let mut parts = [0i64; 7];
    while left > 0 {
        let (unit, size) = match left {
            ..60 => (6, 1),
            60..3600 => (5, 60),
            3600..86_400 => (4, 3600),
            86_400..604_800 => (3, 86_400),
            604_800..2_419_200 => (2, 604_800),
            _ => {
                // A span the calendar cannot hold has no words. No page asks for one: a bot that far apart is refused (`bot::MAX_SPAN_SECONDS`).
                let Some(end) = chrono::Duration::try_seconds(left).and_then(|span| now.checked_add_signed(span)) else { break };
                let (years, months, weeks, days) = calendar_distance(now, end);
                (parts[0], parts[1], parts[2], parts[3]) = (years, months, weeks, days);
                left %= 86_400;
                continue;
            }
        };
        parts[unit] = left / size;
        left %= size;
    }
    let word = |unit: &str, count: i64| i18n::text(locale, &format!("datetime.dotiw.{unit}"), &[("count", i18n::Arg::Count(count))]);
    let phrases: Vec<String> = UNITS.iter().zip(parts).filter(|(_, count)| *count != 0).map(|(unit, count)| word(unit, count)).collect();
    if phrases.is_empty() {
        return i18n::text(locale, "datetime.dotiw.less_than_x", &[("distance", i18n::Arg::Text(&word("seconds", 1)))]);
    }
    to_sentence(&phrases)
}

/// DOTIW::TimeHash#build_years_months_weeks_days between two UTC times: (years, months, weeks, days).
fn calendar_distance(smallest: DateTime<Utc>, largest: DateTime<Utc>) -> (i64, i64, i64, i64) {
    let months = (i64::from(largest.year()) - i64::from(smallest.year())) * 12 + i64::from(largest.month()) - i64::from(smallest.month());
    let (mut years, mut months) = (months.div_euclid(12), months.rem_euclid(12));
    let days = i64::from(largest.day()) - i64::from(smallest.day());
    let (mut weeks, mut days) = (days.div_euclid(7), days.rem_euclid(7));
    if largest.hour() < smallest.hour() { days -= 1; }
    if days < 0 {
        weeks -= 1;
        days += 7;
    }
    if weeks < 0 {
        months -= 1;
        // The month before the later time's: `largest.advance(months: -1)`.
        let (year, month) = if largest.month() == 1 { (largest.year() - 1, 12) } else { (largest.year(), largest.month() - 1) };
        let next = if month == 12 { chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1) } else { chrono::NaiveDate::from_ymd_opt(year, month + 1, 1) };
        let in_month = next.and_then(|first| first.pred_opt()).map_or(30, |last| i64::from(last.day()));
        weeks += in_month / 7;
        days += in_month % 7;
        if days >= 7 {
            days -= 7;
            weeks += 1;
        }
        if weeks == -1 {
            months -= 1;
            weeks = 4;
            days -= 4;
        }
    }
    if months < 0 {
        years -= 1;
        months += 12;
    }
    (years, months, weeks, days)
}

/// `Time#iso8601` of a UTC time: whole seconds, the fraction cut off, and `Z`.
pub fn iso8601(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// ApplicationHelper#table_date: `%Y/%m/%d` in the reader's zone.
pub fn table_date(time: DateTime<Utc>, zone: &str) -> String {
    timezone::local(time, zone).format("%Y/%m/%d").to_string()
}

/// ApplicationHelper#table_clock: `time.formats.table_clock`, which is `%-l:%M %P` in English and
/// `%H:%M` in every other locale.
pub fn table_clock(time: DateTime<Utc>, zone: &str, locale: &str) -> String {
    let local = timezone::local(time, zone);
    if locale != "en" { return local.format("%H:%M").to_string(); }
    let (pm, hour) = local.hour12();
    format!("{hour}:{:02} {}", local.minute(), if pm { "pm" } else { "am" })
}

/// `Time.current.in_time_zone(zone).strftime('%Z')`: the zone's abbreviation at that moment.
pub fn zone_abbreviation(time: DateTime<Utc>, zone: &str) -> String {
    timezone::local(time, zone).format("%Z").to_string()
}

/// `time.in_time_zone(zone).strftime('%Y-%m-%dT%H:%M')`: the value of a datetime-local input.
pub fn datetime_local(time: DateTime<Utc>, zone: &str) -> String {
    timezone::local(time, zone).format("%Y-%m-%dT%H:%M").to_string()
}

/// Bot::Startable#default_start_time_selection: the NYSE's next Monday open (09:30 in New York,
/// today included) as a weekday and a clock time in the user's zone.
pub fn default_start_time_selection(now: DateTime<Utc>, zone: &str) -> Option<(&'static str, String)> {
    const WEEKDAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    let new_york = chrono_tz::America::New_York;
    let today = now.with_timezone(&new_york).date_naive();
    let monday = today + chrono::Duration::days((7 - i64::from(today.weekday().num_days_from_monday())) % 7);
    let open = monday.and_hms_opt(9, 30, 0)?.and_local_timezone(new_york).single()?;
    let local = timezone::local(open.with_timezone(&Utc), zone);
    Some((WEEKDAYS.get(local.weekday().num_days_from_monday() as usize)?, local.format("%H:%M").to_string()))
}

/// What a refusal for a stored number says. The number itself is not repeated: it may be long.
pub const UNREADABLE: &str = "a stored number outside what this build reads";

/// The mark on a failure to read a stored number: a decimal outside `ruby::BigDec`'s bounds (256
/// characters, an exponent within ±400, 512 digits written out), or a precision outside
/// `ruby::scale`'s. Rails reads such a row and prints it; this build refuses the pages that
/// read it instead of sizing work by it (`layout::or_refused`, which answers the 501 page).
#[derive(Debug)]
pub struct Unreadable(String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreadable {}

pub(crate) fn unread<T>(error: crate::codec::CodecError) -> FromSqlResult<T> {
    Err(FromSqlError::Other(Box::new(Unreadable(format!("{error:?}")))))
}

/// A decimal column (NUMERIC affinity: INTEGER, REAL or TEXT) as ActiveRecord reads it, within
/// `ruby::BigDec`'s bounds. Every amount, price, size and value the pages of bots and the navbar take
/// from a row is read as this.
pub struct Stored(pub Option<BigDec>);

impl FromSql for Stored {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        from_sql(value).map(Stored).or_else(unread)
    }
}

/// A decimal column's value that was taken out of its row as it is, read now: `Stored`, for a value
/// whose row is only read once it is known to be shown (the feed's eleventh row is not).
pub fn stored(value: &rusqlite::types::Value) -> Result<Option<BigDec>, WebError> {
    let value = ValueRef::from(value);
    let failed = |error: FromSqlError| {
        let cause: Box<dyn std::error::Error + Send + Sync> = match error { FromSqlError::Other(cause) => cause, other => Box::new(other) };
        WebError::from(rusqlite::Error::FromSqlConversionFailure(0, value.data_type(), cause))
    };
    Stored::column_result(value).map(|stored| stored.0).map_err(failed)
}

/// Whether a request failed on a stored number this build does not read (`Unreadable`).
pub fn unreadable(error: &WebError) -> bool {
    matches!(error, WebError::Engine(EngineError::Sqlite(rusqlite::Error::FromSqlConversionFailure(_, _, cause))) if cause.is::<Unreadable>() || cause.is::<crate::figures::fill::UnreadableFill>())
}
