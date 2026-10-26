//! Ticker::TechnicallyAnalyzable: RSI and the moving averages over a ticker's closed candles, the high and low of a
//! trailing window, and the all-time high the price-drop rule reads, which Rails keeps on the `tickers` row (`ath`,
//! `ath_updated_at`). Values only: the rules that read them (gate, check, status line) are 7b-3's. Each value carries
//! Ruby's numeric class (`Num`) and is computed as Ruby computes it (BigDecimal closes, Float smoothing, BigDecimal#/),
//! pinned by tests/fixtures/indicator_vectors.json.
//!
//! Every function answers Rails' `Result`: `Ok(Ok(value))` is a Success, `Ok(Err(e))` a Failure with the venue's text or
//! Rails' own; the outer `Err` is the engine's (the database).
use super::candles::{Candle, Timeframe};
use super::model::Ticker;
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::ruby::{float_round, from_sql, ruby_sum, BigDec, Num};
use crate::venue::{Venue, VenueError};
use chrono::{DateTime, Datelike, Duration, Months, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::cell::RefCell;
use std::collections::HashMap;

/// Ticker::TechnicallyAnalyzable::ATH_SEED_FALLBACK_WINDOWS, tried in order after an empty 20-year walk.
const ATH_SEED_FALLBACK_WINDOWS: [Span; 6] = [Span::Years(5), Span::Years(2), Span::Years(1), Span::Days(90), Span::Days(30), Span::Days(7)];
/// ActiveSupport::Duration::SECONDS_PER_YEAR: what `n.years` compares as.
const SECONDS_PER_YEAR: i64 = 31_556_952;

/// A price-drop rule's window (Bot::PriceDropLimitable::PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Window { Ath, TwentyFourHours, SevenDays }

impl Window {
    /// The window a stored `*_time_window_condition` names.
    pub fn of(key: &str) -> Option<Window> {
        match key { "ath" => Some(Self::Ath), "twenty_four_hours" => Some(Self::TwentyFourHours), "seven_days" => Some(Self::SevenDays), _ => None }
    }
    /// None for the ATH, which has no finite span.
    fn span(self) -> Option<Span> { match self { Self::Ath => None, Self::TwentyFourHours => Some(Span::Hours(24)), Self::SevenDays => Some(Span::Days(7)) } }
}

/// A duration as `high_for_duration` receives it: an ActiveSupport Duration (24.hours, 7.days, 5.years) or the Float
/// seconds `Time - Time` gives.
#[derive(Clone, Copy, Debug)]
enum Span { Hours(i64), Days(i64), Years(i64), Seconds(f64) }

impl Span {
    /// Its value in seconds, as `duration < 720.minutes` compares it.
    fn value(self) -> f64 {
        match self { Self::Hours(n) => (n * 3600) as f64, Self::Days(n) => (n * 86_400) as f64, Self::Years(n) => (n * SECONDS_PER_YEAR) as f64, Self::Seconds(f) => f }
    }
}

/// `moving_average_limit_in_ma_type`'s two values (any other raises in Rails, Bot::MovingAverageLimitable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaType { Sma, Ema }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key { Rsi(i64, usize, Timeframe), Ma(i64, usize, Timeframe), High(i64, Window), Low(i64, Window), SeedEmpty(i64) }

#[derive(Clone, Debug)]
enum Kept { Rsi(Num), Ma { sma: Num, ema: Num }, Price(Option<BigDec>), Empty }

/// DIVERGES (D-COND-CACHE): Rails.cache with Rails' keys and lifetimes, held in engine memory, so a restart forgets it.
/// RSI and the moving averages until their candle closes (Utilities::Time.seconds_to_current_candle_close), a window's
/// high or low 20 s, a known-empty ATH seed 1 h. A Failure is never kept.
#[derive(Default)]
pub struct Cache(RefCell<HashMap<Key, (DateTime<Utc>, Kept)>>);

impl Cache {
    fn get(&self, key: &Key, now: DateTime<Utc>) -> Option<Kept> {
        let mut entries = self.0.borrow_mut();
        if entries.get(key).is_some_and(|(until, _)| now >= *until) { entries.remove(key); }
        entries.get(key).map(|(_, v)| v.clone())
    }
    fn put(&self, key: Key, until: DateTime<Utc>, value: Kept) { self.0.borrow_mut().insert(key, (until, value)); }
}

/// When the candle holding `now` closes: the month's and week's last nanosecond (`end_of_month`, `end_of_week`), else the
/// epoch-aligned period's end (Utilities::Time.seconds_to_current_candle_close).
pub fn candle_close(now: DateTime<Utc>, tf: Timeframe) -> DateTime<Utc> {
    let midnight = |d: chrono::NaiveDate| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight"));
    match tf {
        Timeframe::Month => midnight(now.date_naive().with_day(1).expect("the first")).checked_add_months(Months::new(1)).expect("in range") - Duration::nanoseconds(1),
        Timeframe::Week => midnight(now.date_naive() - Duration::days(i64::from(now.weekday().num_days_from_monday())) + Duration::days(7)) - Duration::nanoseconds(1),
        _ => Utc.timestamp_opt((now.timestamp().div_euclid(tf.seconds()) + 1) * tf.seconds(), 0).single().expect("in range"),
    }
}

/// The check job adds Utilities::Time's Float duration back to Time.now. Preserve its nanosecond rounding.
pub fn next_check(now: DateTime<Utc>, tf: Timeframe) -> DateTime<Utc> {
    minus_float(now, -seconds_since(candle_close(now, tf), now))
}

/// `n` timeframes before `t` (`t - n * timeframe`, ActiveSupport's `advance`): calendar months for a month.
fn back(t: DateTime<Utc>, tf: Timeframe, n: u32) -> Option<DateTime<Utc>> {
    match tf { Timeframe::Month => t.checked_sub_months(Months::new(n)), _ => t.checked_sub_signed(Duration::seconds(i64::from(n) * tf.seconds())) }
}

/// `t - f` for Float seconds `f`, as Time#- (Time#since) takes a Float: its exact binary value, then down to the
/// nanosecond (every venue reads whole seconds or milliseconds of it).
fn minus_float(t: DateTime<Utc>, f: f64) -> DateTime<Utc> {
    let bits = f.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let (m, e) = if exp == 0 { ((bits & ((1 << 52) - 1)) as i128, -1074) } else { (((bits & ((1 << 52) - 1)) | (1 << 52)) as i128, exp - 1075) };
    let x = if f < 0.0 { -m } else { m } * 1_000_000_000; // f·10⁹ = x·2^e
    let ceil = if e >= 0 { x << e.min(40) } else {
        let k = -e;
        if k >= 120 { i128::from(x > 0) } else if x >= 0 { (x + (1i128 << k) - 1) >> k } else { -((-x) >> k) }
    };
    let ns = i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos()) - ceil;
    DateTime::from_timestamp(ns.div_euclid(1_000_000_000) as i64, ns.rem_euclid(1_000_000_000) as u32).expect("in range")
}

/// `Time.now.utc - earlier` as Ruby gives it: the nanosecond difference as a double, over 10⁹.
fn seconds_since(now: DateTime<Utc>, earlier: DateTime<Utc>) -> f64 {
    let ns = |t: DateTime<Utc>| i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos());
    (ns(now) - ns(earlier)) as f64 / 1e9
}

/// Ticker#closed_candle_closes: the closes of the candles since `(multiplier + 2)` timeframes ago, the newest dropped
/// while it is still forming. No candles at all is Rails' Failure.
async fn closed_candle_closes<V: Venue>(venue: &V, ticker: &Ticker, tf: Timeframe, multiplier: u32, now: DateTime<Utc>) -> Result<Vec<BigDec>, VenueError> {
    // D-COND-LOOKBACK-RANGE: Rails' arbitrary-range Time accepts dates outside chrono's range.
    // Such a lookback is a settled read failure, never an overflow or a panic.
    let since = multiplier.checked_add(2).and_then(|n| back(now, tf, n))
        .ok_or_else(|| VenueError::Rejected(vec!["condition candle lookback is outside the supported date range".into()]))?;
    let mut candles = venue.candles(ticker, since, tf).await?;
    if candles.is_empty() {
        return Err(VenueError::Rejected(vec![format!("Failed to get {} candles since {} for {}.", tf.inspect(), since.format("%Y-%m-%d %H:%M:%S UTC"), ticker.ticker)]));
    }
    let last = candles.last().expect("non-empty").at;
    let close = match tf { Timeframe::Month => last.checked_add_months(Months::new(1)).expect("in range"), _ => last + Duration::seconds(tf.seconds()) };
    if close > now { candles.pop(); }
    Ok(candles.into_iter().map(|c| c.close).collect())
}

fn to_f(n: &Num) -> f64 { match n { Num::Int(i) => *i as f64, Num::Float(f) => *f, Num::Dec(d) => d.to_f() } }

/// Float * other: a BigDecimal coerces the Float (Float#to_d) and multiplies exactly; anything else is double arithmetic.
fn mul_f(f: f64, n: &Num) -> Option<Num> {
    Some(match n { Num::Dec(d) => Num::Dec(&BigDec::from_f64(f).ok()? * d), _ => Num::Float(f * to_f(n)) })
}

/// Float / other: by a BigDecimal it is BigDecimal#/ of the coerced Float (a zero divisor raises: None).
fn div_f(f: f64, n: &Num) -> Option<Num> {
    Some(match n { Num::Dec(d) => Num::Dec(BigDec::from_f64(f).ok()?.div(d)?), _ => Num::Float(f / to_f(n)) })
}

/// RubyTechnicalAnalysis::RelativeStrengthIndex#call over BigDecimal closes (`period < closes.len()`): the first window
/// seeds both channels with WildersSmoothing (a Float mean), each later one smooths by 1/period. None where Ruby raises.
fn rsi_of(closes: &[BigDec], period: usize) -> Option<Num> {
    let one = 1.0 / period as f64;
    let two = 1.0 - one;
    let (mut up, mut down): (Option<Num>, Option<Num>) = (None, None);
    for index in 0..closes.len() - period {
        let window = &closes[index..=index + period];
        let (ups, downs): (Vec<Num>, Vec<Num>) = window.windows(2).map(|w| {
            let diff = &w[0] - &w[1];
            let zero = BigDec::zero();
            (if diff < zero { Num::Dec(&zero - &diff) } else { Num::Int(0) }, if diff > zero { Num::Dec(diff) } else { Num::Int(0) })
        }).unzip();
        let wilders = |ch: &[Num]| ruby_sum(ch).ok().map(|s| Num::Float(to_f(&s) / period as f64));
        let smooth = |ch: &[Num], last: &Num| mul_f(one, ch.last().expect("period > 0"))?.add(&mul_f(two, last)?).ok();
        up = match &up { None => wilders(&ups), Some(last) => smooth(&ups, last) };
        down = match &down { None => wilders(&downs), Some(last) => smooth(&downs, last) };
        up.as_ref()?;
        down.as_ref()?;
    }
    // 100.00 - (100.00 / ((up.to_f / down) + 1)). A BigDecimal zero `down` (period 1 after a flat step) divides to
    // BigDecimal NaN (0 / 0, None here) or +Infinity, which then gives BigDecimal 100.
    let up = to_f(up.as_ref()?);
    if let Some(Num::Dec(d)) = &down { if d.is_zero() { return (up != 0.0).then(|| Num::Dec(BigDec::from_i64(100))); } }
    let ratio = div_f(up, down.as_ref()?)?.add(&Num::Int(1)).ok()?;
    Num::Float(100.0).sub(&div_f(100.0, &ratio)?).ok()
}

/// `value.round(places)`: Float#round (Integer at 0 places) or BigDecimal#round (Integer at 0 places, else half up).
/// DIVERGES (D-COND-ROUND-UNPORTED): Float#round past 14 places (Ruby's rational path) is not ported: a settled failure.
fn round(n: &Num, places: i64) -> Option<Num> {
    match n {
        Num::Float(f) if places == 0 => f.is_finite().then(|| Num::Int(f.round() as i64)),
        Num::Float(f) => (1..=14).contains(&places).then(|| Num::Float(float_round(*f, places as i32))),
        Num::Dec(d) if places == 0 => d.round(0).to_s_f().trim_end_matches(".0").parse().ok().map(Num::Int),
        Num::Dec(d) => Some(Num::Dec(d.round(u8::try_from(places).ok()?))),
        Num::Int(_) => Some(n.clone()),
    }
}

/// Ticker#get_rsi_value. DIVERGES (D-COND-RSI-DEGENERATE): a series with no gain and no loss in its smoothing (a flat
/// series; with period 1, a flat last step) gives Rails NaN (Float, or BigDecimal at period 1), a Success that reads as
/// unmet on both sides and shows "NaN"; here it is Rails' own too-few-closes Failure, never kept.
pub async fn rsi<V: Venue>(venue: &V, cache: &Cache, ticker: &Ticker, tf: Timeframe, period: usize, now: DateTime<Utc>) -> Result<Num, VenueError> {
    let key = Key::Rsi(ticker.id, period, tf);
    if let Some(Kept::Rsi(v)) = cache.get(&key, now) { return Ok(v); }
    let multiplier = u32::try_from(period).ok().and_then(|p| p.checked_mul(10))
        .ok_or_else(|| VenueError::Rejected(vec!["condition candle lookback is outside the supported date range".into()]))?;
    let closes = closed_candle_closes(venue, ticker, tf, multiplier, now).await?;
    let failed = || VenueError::Rejected(vec![format!("Failed to calculate {} RSI for {} (period: {period})", tf.inspect(), ticker.ticker)]);
    if period == 0 || period >= closes.len() { return Err(failed()); }
    let value = rsi_of(&closes, period).and_then(|v| round(&v, 2)).ok_or_else(failed)?;
    if matches!(value, Num::Float(f) if f.is_nan()) { return Err(failed()); }
    cache.put(key, candle_close(now, tf), Kept::Rsi(value.clone()));
    Ok(value)
}

/// Ticker#get_moving_average_value: RubyTechnicalAnalysis::MovingAverages' SMA (`last(period).sum.to_f / period`) or EMA
/// (a BigDecimal fold with the gem's percentages, 12 and 26 hard-coded), each rounded to the ticker's price decimals.
/// Both are kept under one key, as Rails keeps them.
pub async fn moving_average<V: Venue>(venue: &V, cache: &Cache, ticker: &Ticker, tf: Timeframe, period: usize, kind: MaType, now: DateTime<Utc>) -> Result<Num, VenueError> {
    let pick = |sma: Num, ema: Num| match kind { MaType::Sma => sma, MaType::Ema => ema };
    let key = Key::Ma(ticker.id, period, tf);
    if let Some(Kept::Ma { sma, ema }) = cache.get(&key, now) { return Ok(pick(sma, ema)); }
    let multiplier = u32::try_from(period).ok().and_then(|p| p.checked_mul(10))
        .ok_or_else(|| VenueError::Rejected(vec!["condition candle lookback is outside the supported date range".into()]))?;
    let closes = closed_candle_closes(venue, ticker, tf, multiplier, now).await?;
    let failed = || VenueError::Rejected(vec![format!("Failed to calculate {} Moving Averages for {} (period: {period})", tf.inspect(), ticker.ticker)]);
    if period == 0 || period > closes.len() { return Err(failed()); }
    let sum = ruby_sum(&closes[closes.len() - period..].iter().cloned().map(Num::Dec).collect::<Vec<_>>()).map_err(|_| failed())?;
    let sma = Num::Float(to_f(&sum) / period as f64);
    let ema = if period == 1 { Num::Dec(closes.last().expect("period <= len").clone()) } else {
        let (p0, p1) = match period { 12 => (0.153846, 0.846154), 26 => (0.074074, 0.925926), _ => { let p = 2.0 / (period as f64 + 1.0); (p, 1.0 - p) } };
        let (p0, p1) = (BigDec::from_f64(p0).map_err(|_| failed())?, BigDec::from_f64(p1).map_err(|_| failed())?);
        Num::Dec(closes[1..].iter().fold(closes[0].clone(), |last, num| &(num * &p0) + &(&last * &p1)))
    };
    let (sma, ema) = (round(&sma, ticker.price_decimals).ok_or_else(failed)?, round(&ema, ticker.price_decimals).ok_or_else(failed)?);
    cache.put(key, candle_close(now, tf), Kept::Ma { sma: sma.clone(), ema: ema.clone() });
    Ok(pick(sma, ema))
}

/// TechnicallyAnalyzable#optimal_candles_timeframe_for_duration: Kraken's 720-candle ladder, a day beyond 720 hours.
fn ladder(duration: f64) -> Timeframe {
    let minutes = |n: i64| (n * 720 * 60) as f64;
    if duration < minutes(1) { Timeframe::Minute } else if duration < minutes(5) { Timeframe::FiveMinutes }
    else if duration < minutes(15) { Timeframe::FifteenMinutes } else if duration < minutes(30) { Timeframe::ThirtyMinutes }
    else if duration < (720 * 3600) as f64 { Timeframe::Hour } else { Timeframe::Day }
}

/// `(duration + timeframe).ago`: a Duration's calendar years first, then its days, hours and the timeframe; Float seconds
/// by their exact value.
fn window_start(span: Span, tf: Timeframe, now: DateTime<Utc>) -> DateTime<Utc> {
    let t = match span { Span::Years(n) => now.checked_sub_months(Months::new(12 * n as u32)).expect("in range"), _ => now };
    let t = t - Duration::seconds(tf.seconds() + match span { Span::Hours(n) => n * 3600, Span::Days(n) => n * 86_400, _ => 0 });
    match span { Span::Seconds(f) => minus_float(t, f), _ => t }
}

/// The high (or low) of the candles since the window's start, the forming one included; none is nil.
async fn extreme<V: Venue>(venue: &V, ticker: &Ticker, span: Span, low: bool, now: DateTime<Utc>) -> Result<Option<BigDec>, VenueError> {
    let tf = ladder(span.value());
    let candles: Vec<Candle> = venue.candles(ticker, window_start(span, tf, now), tf).await?;
    Ok(candles.into_iter().map(|c| if low { c.low } else { c.high }).reduce(|a, b| if low { if b < a { b } else { a } } else { a.max(b) }))
}

/// Ticker#get_low_of_last over a finite window.
pub async fn low_of_last<V: Venue>(venue: &V, cache: &Cache, ticker: &Ticker, window: Window, now: DateTime<Utc>) -> Result<Option<BigDec>, VenueError> {
    let key = Key::Low(ticker.id, window);
    if let Some(Kept::Price(v)) = cache.get(&key, now) { return Ok(v); }
    // Rails raises (FloatDomainError) on an infinite low window; the sell side never offers one.
    let span = window.span().ok_or_else(|| VenueError::Rejected(vec!["There is no all-time low window".into()]))?;
    let low = extreme(venue, ticker, span, true, now).await?;
    cache.put(key, now + Duration::seconds(20), Kept::Price(low.clone()));
    Ok(low)
}

/// Ticker#get_high_of_last. A finite window is its candles' high. The ATH (`Window::Ath`) is `tickers.ath`, kept current:
/// a ticker seeded on a history that cannot change reads only the candles since `ath_updated_at` and keeps the larger;
/// otherwise (unseeded, or an Alpaca stock, whose history every split restates) it walks 20 years of days, then the
/// fallback windows, and the full walk of a restated ticker replaces the high. Rails' `update!(ath:, ath_updated_at:)`
/// writes the row (only what changed, `updated_at` with it). An unseeded ticker with no history at all is remembered
/// for an hour and not walked again.
pub async fn high_of_last<V: Venue>(c: &Connection, venue: &V, cache: &Cache, ticker: &Ticker, window: Window, now: DateTime<Utc>) -> Result<Result<Option<BigDec>, VenueError>, EngineError> {
    let key = Key::High(ticker.id, window);
    if let Some(Kept::Price(v)) = cache.get(&key, now) { return Ok(Ok(v)); }
    let keep = |v: Option<BigDec>| { cache.put(key.clone(), now + Duration::seconds(20), Kept::Price(v.clone())); Ok(Ok(v)) };
    if let Some(span) = window.span() {
        return match extreme(venue, ticker, span, false, now).await { Ok(v) => keep(v), Err(e) => Ok(Err(e)) };
    }
    let (ath, updated_at, exchange_type) = c.query_row(
        "SELECT t.ath, t.ath_updated_at, e.type FROM tickers t JOIN exchanges e ON e.id = t.exchange_id WHERE t.id = ?1", [ticker.id],
        |r| Ok((from_sql(r.get_ref(0)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("tickers.ath: {e:?}")))?,
                r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?))).optional()?
        .ok_or_else(|| EngineError::Data(format!("ticker {} is gone", ticker.id)))?;
    let updated_at = updated_at.map(|s| parse_time(&s)).transpose().map_err(EngineError::Arithmetic)?;
    // Exchanges::Alpaca#restated_candles?: every ticker whose base is not a Cryptocurrency.
    let restated = exchange_type == "Exchanges::Alpaca" && !ticker.crypto;
    let mut deep = false;
    let found = match updated_at {
        Some(at) if !restated => match extreme(venue, ticker, Span::Seconds(seconds_since(now, at)), false, now).await { Ok(v) => v, Err(e) => return Ok(Err(e)) },
        _ => {
            let seed_key = Key::SeedEmpty(ticker.id);
            if ath.is_none() && cache.get(&seed_key, now).is_some() { return keep(None); }
            let twenty_years = now.checked_sub_months(Months::new(240)).expect("in range");
            let mut found = match extreme(venue, ticker, Span::Seconds(seconds_since(now, twenty_years)), false, now).await { Ok(v) => v, Err(e) => return Ok(Err(e)) };
            deep = found.is_some();
            for span in ATH_SEED_FALLBACK_WINDOWS {
                if found.is_some() { break; }
                found = match extreme(venue, ticker, span, false, now).await { Ok(v) => v, Err(e) => return Ok(Err(e)) };
            }
            if found.is_none() {
                if ath.is_none() { cache.put(seed_key, now + Duration::hours(1), Kept::Empty); }
                return keep(ath);
            }
            found
        }
    };
    let new_high = if restated && deep { found } else { [ath.clone(), found].into_iter().flatten().reduce(BigDec::max) };
    if let Some(high) = &new_high {
        let stamp = format_time(now);
        // update!: only changed attributes are written; ath_updated_at read back to the microsecond never equals a
        // nanosecond `now` unless both are whole microseconds.
        let ath_changed = ath.as_ref() != Some(high);
        if ath_changed {
            c.execute("UPDATE tickers SET ath = ?1, ath_updated_at = ?2, updated_at = ?2 WHERE id = ?3", params![high.to_f(), stamp, ticker.id])?;
        } else if updated_at != Some(now) {
            c.execute("UPDATE tickers SET ath_updated_at = ?1, updated_at = ?1 WHERE id = ?2", params![stamp, ticker.id])?;
        }
    }
    keep(new_high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_float_is_subtracted_by_its_exact_value_down_to_the_nanosecond() {
        let t = parse_time("2026-09-02T10:00:01.123456789Z").unwrap();
        assert_eq!(minus_float(t, 0.5), parse_time("2026-09-02T10:00:00.623456789Z").unwrap());
        // 0.1 is a hair above a tenth: the nanosecond below.
        assert_eq!(minus_float(t, 0.1), parse_time("2026-09-02T10:00:01.023456788Z").unwrap());
        assert_eq!(minus_float(t, -0.1), parse_time("2026-09-02T10:00:01.223456789Z").unwrap());
    }

    #[test]
    fn candles_close_at_their_period_end() {
        let t = parse_time("2026-10-01T00:00:01Z").unwrap();
        assert_eq!(candle_close(t, Timeframe::Month), parse_time("2026-10-31T23:59:59.999999999Z").unwrap());
        assert_eq!(candle_close(t, Timeframe::Week), parse_time("2026-10-04T23:59:59.999999999Z").unwrap());
        assert_eq!(candle_close(t, Timeframe::FourHours), parse_time("2026-10-01T04:00:00Z").unwrap());
    }
}
