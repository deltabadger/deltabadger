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
    check_unsaved(c, bot, None, now, market_data_configured, locale)
}

/// `invalid?(:start)` on a bot an update refused: Rails asks the unsaved record, whose changes (a venue, a composition) are
/// still measured against the stored row.
pub fn check_unsaved(c: &Connection, bot: &Bot, stored: Option<&Bot>, now: DateTime<Utc>, market_data_configured: bool, locale: &str) -> Result<Check, WebError> {
    let mut draft = super::draft::Draft::from_bot(bot.clone());
    if let Some(stored) = stored { draft.as_unsaved_of(stored); }
    draft.validate(c, super::draft::ValidationContext::Start, now, market_data_configured, locale)?;
    Ok(draft.check())
}

/// A request streams at most this many relevant cap rows. The next row is an explicit 501
/// read-bound refusal before any mutation, even if a history spans several external statuses.
pub const HISTORY_WORK_BUDGET: usize = 100_000;
pub(super) const HISTORY_BOUND: &str = "bot action history exceeds the 100000-row work budget";
pub(crate) fn history_error() -> WebError { super::data(HISTORY_BOUND.to_owned()) }

/// Startable#parse_hhmm: two parts of one or two digits each, 0-23 and 0-59.
pub fn parse_hhmm(time_of_day: Option<&str>) -> Result<(u32, u32), &'static str> {
    let (h, m) = time_of_day.and_then(|s| s.split_once(':')).ok_or("invalid start time")?;
    let part = |s: &str| -> Result<u32, &'static str> {
        if !matches!(s.len(), 1 | 2) || !s.bytes().all(|b| b.is_ascii_digit()) { return Err("invalid start time"); }
        s.parse::<u32>().map_err(|_| "invalid start time")
    };
    let (hour, minute) = (part(h)?, part(m)?);
    if hour > 23 || minute > 59 { return Err("invalid start time"); }
    Ok((hour, minute))
}

/// A chosen starting wall time in `zone`, as TimeZone#local resolves it, with D-START-REPEAT-EARLIER: a wall time that
/// occurs twice that day is its earlier occurrence (Rails' pick in every zone recorded, Dublin and Casablanca included,
/// where Rails follows the server's zoneinfo flags), and one that does not occur moves forward an hour at a time until it
/// does (TimeWithZone's `@time += 1.hour; retry`, a 30-minute gap included). `None` past chrono's range.
pub fn local(zone: chrono_tz::Tz, naive: chrono::NaiveDateTime) -> Option<DateTime<Utc>> {
    use chrono::{LocalResult, TimeZone};
    let mut wall = naive;
    for _ in 0..48 {
        match zone.from_local_datetime(&wall) {
            LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => return Some(at.with_timezone(&Utc)),
            LocalResult::None => wall = wall.checked_add_signed(chrono::Duration::hours(1))?,
        }
    }
    None
}

/// Bot::Startable#initial_start_at for an enabled rule, in the zone named `zone` (User#time_zone; an unknown name is
/// refused, D-START-ZONE-UNKNOWN: Rails would read UTC, and no stored row can hold one). `Err` where Rails answers nil (an
/// unknown mode, a malformed time or date): the :start validation refuses those first, and nil must never read as "start
/// now" here.
///
/// D-START-WALL-STEP: the chosen wall time on the first day (of the chosen weekday) whose instant is after `now`, each day
/// resolved on its own. Rails steps a passed candidate by fixed UTC days (`candidate + 1.day` / `+ 7.days`) and adds
/// calendar days to today's gap-moved wall time, so across a clock change it runs up to an hour early or late.
pub fn initial_start_at(mode: Option<&str>, time_of_day: Option<&str>, start_at: Option<&str>, now: DateTime<Utc>, zone: &str)
    -> Result<DateTime<Utc>, &'static str> {
    use chrono::{Datelike, Duration};
    let mode = mode.ok_or("missing start mode")?;
    // ActiveSupport::TimeZone[] takes a Rails zone name or an IANA identifier.
    let zone = crate::web::timezone::strict(zone).ok_or("unknown time zone")?;
    if mode == "date" {
        return crate::codec::parse_time(start_at.ok_or("invalid start date")?).map_err(|_| "invalid start date");
    }
    let weekday = MODES.iter().position(|m| *m == mode).filter(|n| *n < 7);
    if mode != "hour" && weekday.is_none() { return Err("invalid start mode"); }
    let (hour, minute) = parse_hhmm(time_of_day)?;
    let today = now.with_timezone(&zone).date_naive();
    let (first, step) = match weekday {
        None => (0, 1),
        Some(w) => ((w as i64 - i64::from(today.weekday().num_days_from_monday())).rem_euclid(7), 7),
    };
    let on = |days: i64| -> Result<DateTime<Utc>, &'static str> {
        let day = today.checked_add_signed(Duration::days(days)).ok_or("start date overflow")?;
        local(zone, day.and_hms_opt(hour, minute, 0).ok_or("invalid start time")?).ok_or("start date overflow")
    };
    let candidate = on(first)?;
    if candidate > now { Ok(candidate) } else { on(first + step) }
}
