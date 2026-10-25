//! Candles as the conditions read them (Exchange#get_indicator_candles, `Venue::candles`), and Exchange::CandleBuilder,
//! which builds the timeframes a venue has no interval for (Kraken's 3 days, 1 week and 1 month) from smaller ones.
//! Keyed by ticker and venue only: nothing here knows the bot.
use crate::ruby::BigDec;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};

/// The timeframes Rails asks candles for (TechnicallyAnalyzable): its ActiveSupport durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Timeframe { Minute, FiveMinutes, FifteenMinutes, ThirtyMinutes, Hour, FourHours, Day, ThreeDays, Week, Month }

impl Timeframe {
    /// `Duration#to_i`: a month is ActiveSupport's 1/12 of a Gregorian year.
    pub fn seconds(self) -> i64 {
        match self {
            Self::Minute => 60, Self::FiveMinutes => 300, Self::FifteenMinutes => 900, Self::ThirtyMinutes => 1800, Self::Hour => 3600,
            Self::FourHours => 14_400, Self::Day => 86_400, Self::ThreeDays => 259_200, Self::Week => 604_800, Self::Month => 2_629_746,
        }
    }
    /// `Duration#ago` in UTC: a calendar month back for a month (`advance(months: -1)`, the day clamped to the month's
    /// last), else whole seconds.
    pub fn ago(self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Month => now.checked_sub_months(chrono::Months::new(1)).expect("in range"),
            _ => now - Duration::seconds(self.seconds()),
        }
    }
}

/// The most pages one candle read walks (Exchange::MAX_CANDLE_PAGES). DIVERGES (D-COND-CANDLE-CAP): Binance's and
/// Alpaca's Rails walks have no cap; past it the read is a settled failure, never an endless walk.
pub const MAX_CANDLE_PAGES: usize = 1000;

/// One candle from the venue's open, high, low, close and volume, each `to_d`. DIVERGES (D-COND-CANDLE-UNREADABLE): a
/// price that is absent, null or no finite decimal is unreadable (`None`), where Rails' `to_d` reads it as 0.0; a zero
/// high or low would move an all-time high or a low. An absent or null volume is Rails' 0.0 (conditions never read it).
pub fn read(at: DateTime<Utc>, prices: [&serde_json::Value; 4], volume: &serde_json::Value) -> Option<Candle> {
    let price = |v: &serde_json::Value| crate::ruby::json_to_d(v).ok().flatten();
    let [open, high, low, close] = prices.map(price);
    Some(Candle { at, open: open?, high: high?, low: low?, close: close?, volume: crate::ruby::json_to_d(volume).ok()?.unwrap_or_else(BigDec::zero) })
}

/// One candle: its period's start, then open, high, low, close and volume, each as the venue's `to_d`.
#[derive(Clone, Debug, PartialEq)]
pub struct Candle { pub at: DateTime<Utc>, pub open: BigDec, pub high: BigDec, pub low: BigDec, pub close: BigDec, pub volume: BigDec }

/// The period a candle falls in (CandleBuilder#grouped_candles): the UTC week from Monday (`beginning_of_week`), the
/// calendar month, else epoch-aligned whole seconds.
fn period(at: DateTime<Utc>, timeframe: Timeframe) -> DateTime<Utc> {
    let midnight = |d: chrono::NaiveDate| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight"));
    match timeframe {
        Timeframe::Week => midnight(at.date_naive() - Duration::days(i64::from(at.weekday().num_days_from_monday()))),
        Timeframe::Month => midnight(at.date_naive().with_day(1).expect("the first")),
        _ => {
            let step = timeframe.seconds();
            Utc.timestamp_opt(at.timestamp().div_euclid(step) * step, 0).single().expect("in range")
        }
    }
}

/// CandleBuilder#build_candles_from_candles: candles grouped by period in the order each period first appears; a period
/// starting before the first candle is dropped. Open is the first's, high the max, low the min, close the last's, volume
/// the sum.
pub fn build(candles: &[Candle], timeframe: Timeframe) -> Vec<Candle> {
    let Some(first) = candles.first() else { return vec![] };
    let mut groups: Vec<(DateTime<Utc>, Vec<&Candle>)> = vec![];
    for c in candles {
        let key = period(c.at, timeframe);
        match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, g)) => g.push(c), None => groups.push((key, vec![c])) }
    }
    groups.into_iter().filter(|(at, _)| *at >= first.at).map(|(at, g)| {
        let (head, tail) = (g[0], g[g.len() - 1]);
        Candle {
            at, open: head.open.clone(),
            high: g.iter().map(|c| c.high.clone()).reduce(BigDec::max).expect("non-empty"),
            low: g.iter().map(|c| c.low.clone()).reduce(|a, b| if b < a { b } else { a }).expect("non-empty"),
            close: tail.close.clone(),
            volume: g.iter().fold(BigDec::zero(), |sum, c| &sum + &c.volume),
        }
    }).collect()
}
