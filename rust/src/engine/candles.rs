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

/// `candles::read` over a row's open, high, low, close and volume, by index (an array row) or by key (an object row).
pub fn fields<I: serde_json::value::Index + Copy>(at: DateTime<Utc>, row: &serde_json::Value, [o, h, l, c, v]: [I; 5]) -> Option<Candle> {
    let f = |i: I| row.get(i).unwrap_or(&serde_json::Value::Null);
    read(at, [f(o), f(h), f(l), f(c)], f(v))
}

/// A candle's time as the venues that send whole numbers are read (`to_i`): an integer, or a string of one. DIVERGES
/// (D-COND-CANDLE-UNREADABLE): anything else (a fraction, a word, null) is unreadable, where `to_i` truncates it or reads 0.
pub fn to_i(v: &serde_json::Value) -> Option<i64> {
    match v { serde_json::Value::Number(n) => n.as_i64(), serde_json::Value::String(s) => s.parse().ok(), _ => None }
}
/// `Time.at(ms / 1000)`: whole seconds (Integer#/ floors).
pub fn at_ms(ms: i64) -> Option<DateTime<Utc>> { DateTime::from_timestamp(ms.div_euclid(1000), 0) }

/// A 2xx candle answer as honeymaker's json middleware reads it: one that is not JSON is its Failure (the text, as
/// `redact` scrubs it), never a raise; a number past the venue caps is unreadable.
pub fn decode(text: &str, redact: impl Fn(&str) -> String, unreadable: fn() -> crate::venue::VenueError) -> Result<serde_json::Value, crate::venue::VenueError> {
    match crate::venue::http::decode_json(text) {
        Err(crate::venue::http::DecodeError::NotJson) => Err(crate::venue::VenueError::Rejected(vec![redact(text)])),
        other => other.map_err(|_| unreadable()),
    }
}

/// One candle read as honeymaker's #with_rescue answers it, the reader every venue from KuCoin on shares: a transport
/// failure is the shared read split (Transient); a non-2xx is `message(body)` (the venue's #parse_error_message, else the
/// body), Faraday's text when the body is empty, a 401 `Unauthorized` and, where the venue's adapter names one, a 429 its
/// throttle (spec E §2.10); a 2xx is `decode`d.
pub async fn get<T: crate::venue::http::Transport>(t: &T, r: &crate::venue::http::HttpRequest, message: impl Fn(&str) -> String, throttle: Option<&str>,
                                                  unreadable: fn() -> crate::venue::VenueError) -> Result<serde_json::Value, crate::venue::VenueError> {
    use crate::venue::VenueError;
    let resp = t.send_limited(r, crate::venue::http::MAX_BODY).await
        .map_err(|e| crate::venue::http::transport_failure(e, false, |m| t.redact_diagnostic(&m)))?;
    if !(200..300).contains(&resp.status) {
        let text = if resp.body.is_empty() { format!("the server responded with status {} for {} {}", resp.status, r.method, r.url()) } else { message(&resp.body) };
        return Err(match (resp.status, throttle) {
            (401, _) => VenueError::Unauthorized(vec![text]),
            (429, Some(throttle)) => VenueError::Rejected(vec![throttle.into()]),
            _ => VenueError::Rejected(vec![text]),
        });
    }
    decode(&resp.body, |s| t.redact_diagnostic(s), unreadable)
}

/// D-COND-CANDLE-CAP's settled failure.
pub fn too_many_pages(venue: &str) -> crate::venue::VenueError {
    crate::venue::VenueError::Rejected(vec![format!("{venue}: more than {MAX_CANDLE_PAGES} pages of candles")])
}

/// Gemini's and Bitrue's failure when a built timeframe has nothing to build from (`Duration#inspect` of both).
pub fn nothing_to_build(source: Timeframe, timeframe: Timeframe, venue: &str) -> crate::venue::VenueError {
    crate::venue::VenueError::Rejected(vec![format!("No {} source candles to build {} on {venue}", source.inspect(), timeframe.inspect())])
}

impl Timeframe {
    /// ActiveSupport's `Duration#inspect`.
    pub fn inspect(self) -> &'static str {
        match self {
            Self::Minute => "1 minute", Self::FiveMinutes => "5 minutes", Self::FifteenMinutes => "15 minutes", Self::ThirtyMinutes => "30 minutes",
            Self::Hour => "1 hour", Self::FourHours => "4 hours", Self::Day => "1 day", Self::ThreeDays => "3 days", Self::Week => "1 week",
            Self::Month => "1 month",
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
