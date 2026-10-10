//! What decides whether a bot may be started, and the figures two rule forms print from the same
//! sums: Bot::SmartIntervalable's floor, Bot::QuoteAmountLimitable's remainder, and
//! `bot.invalid?(:start)`.
use super::Bot;
use crate::ruby::BigDec;
use crate::web::format::{input_value, Num};
use crate::web::{i18n, WebError};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::Value;

/// Bot::Startable::MODES: a weekday, a date, or every day at an hour.
pub const MODES: [&str; 9] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "date", "hour"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason { None, Exchange, Frequency, Precision }

pub struct Minimum {
    /// An Integer 0 without tickers, else a Float.
    pub value: Num,
    pub reason: Reason,
    pub decimals: u8,
}

pub use crate::engine::accounting::{smart_interval_minimum, default_smart_interval_quote_amount};

/// Bot::SmartIntervalable#smart_interval_minimum_message(:quote): `I18n.t`, in the request's locale.
pub fn smart_interval_minimum_message(bot: &Bot, minimum: &Minimum, locale: &str) -> Option<String> {
    let currency = bot.quote_symbol().unwrap_or("");
    // `BigDecimal(minimum[:value].to_s).to_s('F')`, a trailing `.0` dropped.
    let amount = input_value(&Num::Dec(BigDec::parse(&minimum.value.to_s()).ok()?))?;
    let decimals = minimum.decimals.to_string();
    let (key, extra): (&str, (&str, &str)) = match minimum.reason {
        Reason::None => return None,
        Reason::Exchange => ("bot.smart_intervals_disclaimer", ("exchange", &bot.exchange.name)),
        Reason::Frequency => ("bot.smart_intervals_minimum_frequency", ("exchange", "")),
        Reason::Precision => ("bot.smart_intervals_minimum_precision", ("decimals", &decimals)),
    };
    Some(i18n::text(locale, key, &[("currency", i18n::Arg::Text(currency)), ("minimum", i18n::Arg::Text(&amount)), (extra.0, i18n::Arg::Text(extra.1))]))
}

/// Bot::QuoteAmountLimitable: what is left of the spending cap, and whether that is nothing.
pub struct Limit {
    /// `quote_amount_available_before_limit_reached`, in whichever of Ruby's number classes the sums came out as.
    /// `None` when a closed buy never reported its cost: Rails' nil, the spend unknown.
    pub left: Option<Num>,
    pub reached: bool,
}

pub use crate::engine::accounting::{web_amount_limit as amount_limit, minimum_quote_amount_limit};

/// `"HH:MM"` as Bot::Startable#parse_hhmm accepts it.
pub(super) fn hhmm(value: Option<&str>) -> bool {
    let Some((hours, minutes)) = value.and_then(|text| text.split_once(':')) else { return false };
    let part = |text: &str, max: u32| (1..=2).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()) && text.parse::<u32>().is_ok_and(|n| n <= max);
    !minutes.contains(':') && part(hours, 23) && part(minutes, 59)
}

/// What `bot.invalid?(:start)` leaves behind: whether the bot may not start, and the errors on the
/// attributes that have a form field on the page, which Rails then prints under that field
/// (config/initializers/inline_form_errors.rb).
#[derive(Default)]
pub struct Check {
    pub invalid: bool,
    pub errors: Vec<super::draft::FieldError>,
    pub rejected: serde_json::Map<String, Value>,
    /// The message under the Smart Intervals amount: Bot::SmartIntervalable's floor, in words.
    pub smart_interval_quote_amount: Option<String>,
    /// `blank` or `must_be_future`.
    pub start_at: Option<&'static str>,
    pub start_time_of_day: bool,
    /// The rule is on and its mode is none of `MODES` (never chosen, or emptied).
    pub start_time_mode: bool,
}

/// The read-only page shares the complete validator, then projects its field errors onto the
/// existing form controls. Persisted-row refusal still runs before this entry point.
pub fn check(c: &Connection, bot: &Bot, now: DateTime<Utc>, market_data_configured: bool, locale: &str) -> Result<Check, WebError> {
    let mut draft = super::draft::Draft::from_bot(bot.clone());
    draft.validate(c, super::draft::ValidationContext::Start, now, market_data_configured, locale)?;
    Ok(draft.check())
}

/// A request streams at most this many relevant cap rows. The next row is an explicit 501
/// read-bound refusal before any mutation, even if a history spans several external statuses.
pub const HISTORY_WORK_BUDGET: usize = 100_000;
pub(super) const HISTORY_BOUND: &str = "bot action history exceeds the 100000-row work budget";
pub(crate) fn history_error() -> WebError { super::data(HISTORY_BOUND.to_owned()) }

/// Bot::Startable#initial_start_at's answer, and whether it falls BEFORE the chosen wall-clock time on its day. Rails steps
/// a passed candidate forward by fixed UTC days (`candidate + 1.day` / `+ 7.days` on a UTC Time), so across a DST change
/// in between it lands an hour off the chosen local time; when that hour is earlier, the start is refused (it would buy
/// before the time the owner set). An hour later is Rails' answer and is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartAt { pub at: DateTime<Utc>, pub early: bool }

/// Bot::Startable#initial_start_at for an enabled rule, in the zone named `zone` (User#time_zone; an unknown name is
/// UTC, as `user_time_zone` falls back). `Err` where Rails answers nil (an unknown mode, a malformed time or date): the
/// :start validation refuses those first, and nil must never read as "start now" here.
pub fn initial_start_at(mode: Option<&str>, time_of_day: Option<&str>, start_at: Option<&str>, now: DateTime<Utc>, zone: &str)
    -> Result<StartAt, &'static str> {
    use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
    let mode = mode.ok_or("missing start mode")?;
    if mode == "date" {
        let at = crate::codec::parse_time(start_at.ok_or("invalid start date")?).map_err(|_| "invalid start date")?;
        return Ok(StartAt { at, early: false });
    }
    let weekday = MODES.iter().position(|m| *m == mode).filter(|n| *n < 7);
    if mode != "hour" && weekday.is_none() { return Err("invalid start mode"); }
    // Startable#parse_hhmm: two parts of one or two digits each, 0-23 and 0-59.
    let (h, m) = time_of_day.and_then(|s| s.split_once(':')).ok_or("invalid start time")?;
    let part = |s: &str| -> Result<u32, &'static str> {
        if !matches!(s.len(), 1 | 2) || !s.bytes().all(|b| b.is_ascii_digit()) { return Err("invalid start time"); }
        s.parse::<u32>().map_err(|_| "invalid start time")
    };
    let (hour, minute) = (part(h)?, part(m)?);
    if hour > 23 || minute > 59 { return Err("invalid start time"); }
    // ActiveSupport::TimeZone[] takes a Rails zone name or an IANA identifier; an unknown name is refused, never UTC.
    let zone = match crate::web::timezone::zone(zone) {
        Some(zone) => zone,
        None => zone.parse::<chrono_tz::Tz>().map_err(|_| "unknown time zone")?,
    };
    // TimeZone#local / TimeWithZone: a wall time in a gap moves forward an hour, a repeated one takes the first (DST) instant.
    let wall = |naive: NaiveDateTime| -> Result<(NaiveDateTime, DateTime<Utc>), &'static str> {
        let (wall, at) = match zone.from_local_datetime(&naive) {
            LocalResult::Single(at) => (naive, at),
            LocalResult::Ambiguous(a, b) => (naive, a.min(b)),
            LocalResult::None => {
                let later = naive.checked_add_signed(Duration::hours(1)).ok_or("start date overflow")?;
                (later, zone.from_local_datetime(&later).earliest().ok_or("unresolvable start time")?)
            }
        };
        Ok((wall, at.with_timezone(&Utc)))
    };
    let local = now.with_timezone(&zone);
    let at = |day: NaiveDate| day.and_hms_opt(hour, minute, 0).ok_or("invalid start time");
    let (today_wall, today) = wall(at(local.date_naive())?)?;
    let (step, candidate) = match weekday {
        None => (1, today),
        Some(w) => {
            // `today_at_time + days_ahead.days`: calendar days added to today's (gap-moved) wall time, resolved again.
            let days = (w as i64 - i64::from(local.weekday().num_days_from_monday())).rem_euclid(7);
            (7, wall(today_wall.checked_add_signed(Duration::days(days)).ok_or("start date overflow")?)?.1)
        }
    };
    if candidate > now { return Ok(StartAt { at: candidate, early: false }); }
    // `candidate + 1.day` / `+ 7.days` on a UTC Time: fixed seconds.
    let stepped = candidate.checked_add_signed(Duration::days(step)).ok_or("start date overflow")?;
    // The time the owner chose, `step` local days after the candidate's.
    let day = candidate.with_timezone(&zone).date_naive().checked_add_signed(Duration::days(step)).ok_or("start date overflow")?;
    let chosen = wall(at(day)?)?.1;
    Ok(StartAt { at: stepped, early: stepped < chosen })
}
