//! What decides whether a bot may be started, and the figures two rule forms print from the same
//! sums: Bot::SmartIntervalable's floor, Bot::QuoteAmountLimitable's remainder, and
//! `bot.invalid?(:start)`.
use super::{Bot, Kind, Stored};
use crate::codec::format_time;
use crate::ruby::BigDec;
use crate::web::format::{float_to_s, input_value, Num};
use crate::web::{i18n, WebError};
use crate::engine::schedule::Interval;
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::Value;

/// Bot::Startable::MODES: a weekday, a date, or every day at an hour.
pub const MODES: [&str; 9] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "date", "hour"];

/// Automation::Schedulable::INTERVALS, in seconds as `interval_duration.to_f` gives them.
fn interval_seconds(interval: Interval) -> f64 {
    match interval { Interval::Hour => 3_600.0, Interval::Day => 86_400.0, Interval::Week => 604_800.0, Interval::Month => 2_629_746.0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason { None, Exchange, Frequency, Precision }

/// Bot::SmartIntervalable#smart_interval_minimum(:quote).
pub struct Minimum {
    /// An Integer 0 without tickers, else a Float.
    pub value: Num,
    pub reason: Reason,
    pub decimals: u8,
}

pub fn smart_interval_minimum(bot: &Bot) -> Minimum {
    let Some(decimals) = bot.tickers.iter().map(|ticker| ticker.quote_decimals).min() else { return Minimum { value: Num::Int(0), reason: Reason::None, decimals: 0 } };
    // At most one order every five minutes.
    let frequency = match (bot.number("quote_amount"), bot.interval()) {
        (Some(amount), Some(interval)) => amount.to_f() / interval_seconds(interval) * 300.0,
        _ => 0.0,
    };
    let scale = 10f64.powi(i32::from(decimals));
    let precision = 1.0 / scale;
    // A one-asset basket keeps the pair bot's floor, which never included the venue's minimum.
    let largest = |tickers: Vec<&super::Ticker>| tickers.iter().map(|ticker| ticker.minimum_quote_size.clone()).max().map_or(0.0, |size| size.to_f());
    let exchange = match bot.kind {
        Kind::Basket if bot.one_asset() => 0.0,
        Kind::Basket => largest(bot.composition_tickers()),
        Kind::Index => largest(bot.tickers.iter().collect()),
    };
    let reason = if exchange >= frequency && exchange >= precision { Reason::Exchange } else if frequency >= precision { Reason::Frequency } else { Reason::Precision };
    let rounded_up = round_up(frequency, scale); // Utilities::Number.round_up
    Minimum { value: Num::Float(rounded_up.max(precision).max(exchange)), reason, decimals }
}

/// Bot::SmartIntervalable#initialize_smart_intervalable_settings: the amount a row without one is
/// given on load. `[quote_amount / 10, minimum * 10].max.round(decimals).to_f`, in Ruby's classes:
/// an Integer amount divides as an Integer (25 / 10 is 2). `None` without an amount or a ticker.
pub fn default_smart_interval_quote_amount(bot: &Bot) -> Option<f64> {
    let decimals = bot.tickers.iter().map(|ticker| ticker.quote_decimals).min()?;
    let tenth = match bot.number("quote_amount")? {
        Num::Int(amount) => Num::Int(amount.div_euclid(10)),
        amount => Num::Float(amount.to_f() / 10.0),
    };
    let floor = smart_interval_minimum(bot).value.to_f() * 10.0;
    let larger = if floor > tenth.to_f() { Num::Float(floor) } else { tenth };
    Some(larger.round(decimals).to_f())
}

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

fn add(a: Num, b: Num) -> Option<Num> {
    // a + b is a - (0 - b) in every class Ruby would pick.
    a.sub(&Num::Int(0).sub(&b)?)
}

/// `None` unless the cap is on and has a value (Rails then reads infinity). An order whose amounts
/// are missing where Rails adds them up is an error there (nil cannot be added) and here.
pub fn amount_limit(c: &Connection, bot: &Bot) -> Result<Option<Limit>, WebError> {
    if !bot.on("quote_amount_limited") { return Ok(None); }
    let Some(limit) = bot.number("quote_amount_limit") else { return Ok(None) };
    let missing = || WebError::Engine(crate::engine::EngineError::Data(format!("bot {}: an order under the spending cap has no amount", bot.id)));
    // A stored amount outside what this build reads fails as `bot::Unreadable`, which the handlers answer with the 501 page.
    let decimal = |row: &rusqlite::Row<'_>, column: usize| -> Result<Option<BigDec>, WebError> { Ok(row.get::<_, Stored>(column)?.0) };
    // `created_at >= NULL` matches nothing: a cap switched on before the column existed counts no order.
    let since = bot.transient.get("quote_amount_limit_enabled_at").and_then(Value::as_str).and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| format_time(time.with_timezone(&Utc)));
    let scope = "FROM transactions WHERE bot_id = ?1 AND status = 0 AND side = 0 AND transaction_type = 'REGULAR' AND created_at >= ?2";
    let mut total = Num::Int(0);
    let mut work = 0;
    // Closed orders: what they cost. `pluck(:quote_amount_exec).sum`.
    let mut closed = c.prepare(&format!("SELECT quote_amount_exec {scope} AND external_status = 2 LIMIT 100001"))?;
    let mut rows = closed.query((bot.id, &since))?;
    while let Some(row) = rows.next()? {
        work += 1;
        if work > HISTORY_WORK_BUDGET { return Err(history_error()); }
        // `return nil if closed_quote_amounts.include?(nil)`: nothing after it is read.
        let Some(spent) = decimal(row, 0)? else { return Ok(Some(Limit { left: None, reached: false })) };
        total = add(total, Num::Dec(spent)).ok_or_else(missing)?;
    }
    // Waiting orders: what they ask for. `quote_amount || (amount * price)`.
    let mut open = c.prepare(&format!("SELECT quote_amount, amount, price {scope} AND external_status IN (0, 1) LIMIT 100001"))?;
    let mut rows = open.query((bot.id, &since))?;
    while let Some(row) = rows.next()? {
        work += 1;
        if work > HISTORY_WORK_BUDGET { return Err(history_error()); }
        let asked = match decimal(row, 0)? {
            Some(quote_amount) => quote_amount,
            None => &decimal(row, 1)?.ok_or_else(missing)? * &decimal(row, 2)?.ok_or_else(missing)?,
        };
        total = add(total, Num::Dec(asked)).ok_or_else(missing)?;
    }
    // Cancelled and abandoned orders: what they filled before. Plucked through SQL, so not cast: SQLite's own Integer or Float.
    let mut stopped = c.prepare(&format!("SELECT COALESCE(quote_amount_exec, 0) {scope} AND external_status IN (3, 4) LIMIT 100001"))?;
    let mut rows = stopped.query((bot.id, &since))?;
    while let Some(row) = rows.next()? {
        work += 1;
        if work > HISTORY_WORK_BUDGET { return Err(history_error()); }
        // Read first as every stored number is: an infinity in a REAL column is not a Float to add. Then in SQLite's own class.
        let read = decimal(row, 0)?;
        let filled = match row.get_ref(0)? {
            ValueRef::Integer(i) => Num::Int(i),
            ValueRef::Real(f) => Num::Float(f),
            _ => Num::Dec(read.ok_or_else(missing)?),
        };
        total = add(total, filled).ok_or_else(missing)?;
    }
    let left = limit.sub(&total).ok_or_else(missing)?.at_least_zero();
    let floor = minimum_quote_amount_limit(bot);
    let reached = match &left {
        Num::Dec(decimal) => BigDec::parse(&float_to_s(floor)).is_ok_and(|floor| *decimal < floor),
        other => other.to_f() < floor,
    };
    Ok(Some(Limit { left: Some(left), reached }))
}

/// Bot::QuoteAmountLimitable#minimum_quote_amount_limit: the smallest amount the quote of the bot's
/// tickers can state, and 0 when it has none (app/models/bot/quote_amount_limitable.rb:94). Only
/// `bot.tickers`: a basket whose members are no longer listed has none, whatever its memberships say.
pub fn minimum_quote_amount_limit(bot: &Bot) -> f64 {
    bot.tickers.iter().map(|ticker| ticker.quote_decimals).min().map_or(0.0, |decimals| 1.0 / 10f64.powi(i32::from(decimals)))
}

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

/// At this magnitude a Float already has no fractional precision at the requested scale.
/// Avoid overflowing an intermediate multiply merely to round that unchanged Float up.
pub(super) fn round_up(value: f64, scale: f64) -> f64 {
    if value > f64::MAX / scale { value } else { (value * scale).ceil() / scale }
}

/// A request streams at most this many relevant cap rows. The next row is an explicit 501
/// read-bound refusal before any mutation, even if a history spans several external statuses.
pub const HISTORY_WORK_BUDGET: usize = 100_000;
pub(super) const HISTORY_BOUND: &str = "bot action history exceeds the 100000-row work budget";
pub(super) fn history_error() -> WebError { super::data(HISTORY_BOUND.to_owned()) }
