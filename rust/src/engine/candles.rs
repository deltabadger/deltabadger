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
