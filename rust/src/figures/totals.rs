//! The account's figures on the bots list: every bot's profit in USD, the account's total and its history, and the
//! currency they are shown in. Everything is computed in each bot's quote currency and carried to USD at
//! Utilities::Currency's rates.
use super::at::At;
use super::budget;
use super::db;
use super::dec::Dec;
use super::json::J;
use super::market::{Failure, Fetch, MarketData, Quoted};
use super::num::Num;
use super::walk::{Metrics, Unpriced};
use super::{Absent, FiguresError};
use bigdecimal::num_bigint::BigInt;
use rusqlite::Connection;

/// Utilities::Currency::STABLECOIN_IDS: the provider's id of each stablecoin.
pub const STABLECOIN_IDS: [(&str, &str); 11] = [
    ("USDC", "usd-coin"), ("USDT", "tether"), ("DAI", "dai"), ("BUSD", "binance-usd"), ("TUSD", "true-usd"), ("USDP", "paxos-standard"),
    ("GUSD", "gemini-dollar"), ("FRAX", "frax"), ("LUSD", "liquity-usd"), ("USDD", "usdd"), ("PYUSD", "paypal-usd"),
];
/// Utilities::Currency::FIAT_SYMBOLS.
pub const FIAT_SYMBOLS: [&str; 33] = [
    "USD", "EUR", "GBP", "JPY", "CAD", "AUD", "CHF", "CNY", "INR", "MXN", "BRL", "KRW", "SGD", "HKD", "NOK", "SEK", "DKK", "NZD", "ZAR", "RUB", "TRY",
    "PLN", "THB", "IDR", "MYR", "PHP", "CZK", "ILS", "ARS", "CLP", "COP", "PEN", "UAH",
];

/// Utilities::Currency.exchange_rate, uncached: what one `from` is worth in `to`, a fiat currency. A Float one for a
/// currency against itself; otherwise whatever Ruby made of the provider's answer (an Integer divides as one, and
/// a coin's price that came as a String is handed on as that String). The outer error is this library's own.
/// The figures only ever convert into a fiat currency (USD, or the one the account is shown in), so of Rails' four
/// paths the two with a coin as the target are not here: asked for one, this fails as Rails' last line does.
fn exchange_rate(c: &Connection, market: &dyn MarketData, from: &str, to: &str) -> Result<Fetch<Quoted>, FiguresError> {
    let (from, to) = (from.to_uppercase(), to.to_uppercase());
    if from == to { return Ok(Ok(Quoted::Num(Num::Float(1.0)))); }
    let from_asset = db::asset_by_symbol(c, &from)?;
    let to_asset = db::asset_by_symbol(c, &to)?;
    let category = |asset: &Option<db::Asset>| asset.as_ref().and_then(|asset| asset.category.clone());
    let stablecoin = STABLECOIN_IDS.iter().find(|(symbol, _)| *symbol == from).map(|(_, id)| id.to_string());
    let fiat = |symbol: &str, asset: &Option<db::Asset>| category(asset).as_deref() == Some("Currency") || FIAT_SYMBOLS.contains(&symbol);
    let unknown = || Failure::Failed(format!("Unable to determine conversion path from {from} to {to}"));
    if !fiat(&to, &to_asset) { return Ok(Err(unknown())); }

    if fiat(&from, &from_asset) {
        // The provider's rates are BTC-based: if 1 BTC = X EUR and 1 BTC = Y USD, then 1 EUR = Y / X USD.
        let rates = match market.exchange_rates() { Ok(rates) => rates, Err(failure) => return Ok(Err(failure)) };
        let rate = |symbol: &str| rates.iter().find(|(currency, _)| *currency == symbol.to_lowercase()).map(|(_, value)| value.clone());
        let (Some(from_rate), Some(to_rate)) = (rate(&from), rate(&to)) else { return Ok(Err(Failure::Failed(format!("Exchange rate not found for {from} or {to}")))) };
        // The two rates this conversion uses, and no other: a rate that is no number for a currency nobody asked
        // about stays where it is.
        return Ok(match (from_rate, to_rate) {
            (Ok(from_rate), Ok(to_rate)) => to_rate.div(&from_rate).map(Quoted::Num).map_err(Failure::from),
            (Err(error), _) | (_, Err(error)) => Err(Failure::from(error)),
        });
    }
    if category(&from_asset).as_deref() == Some("Cryptocurrency") || stablecoin.is_some() {
        // The coin's price in the fiat currency, by its id at the provider: a stablecoin's known id, else the asset's.
        let coin = stablecoin.or_else(|| from_asset.as_ref().and_then(|asset| asset.external_id.clone()));
        let Some(coin) = coin else { return Ok(Err(Failure::Failed(format!("No CoinGecko ID found for {from}")))) };
        return Ok(market.coin_price(&coin, &to.to_lowercase()));
    }
    Ok(Err(unknown()))
}

/// One lookup per currency for a page of bots.
#[derive(Default)]
pub struct Rates(Vec<(String, Quoted)>);

impl Rates {
    /// The rate into USD, or None where Rails has a Failure.
    fn usd(&mut self, c: &Connection, market: &dyn MarketData, currency: &str) -> Result<Option<Quoted>, FiguresError> {
        if let Some((_, rate)) = self.0.iter().find(|(known, _)| known == currency) { return Ok(Some(rate.clone())); }
        let rate = lift(exchange_rate(c, market, currency, "USD")?)?;
        // Only an answer is kept: Rails never caches a failure.
        if let Some(rate) = &rate { self.0.push((currency.to_string(), rate.clone())); }
        Ok(rate)
    }
}

fn lift<T>(result: Fetch<T>) -> Result<Option<T>, FiguresError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Failure::Failed(_)) => Ok(None),
        Err(Failure::Raised(error)) => Err(FiguresError::Raised(error)),
        Err(Failure::NotComputed(reason)) => Err(FiguresError::NotComputed(reason)),
    }
}

/// Bot#profit_in_usd: value minus invested, in USD. None when there is no rate for the bot's quote currency; a
/// profit of nothing is nothing in every currency and needs none.
pub fn profit_in_usd(c: &Connection, market: &dyn MarketData, rates: &mut Rates, quote: Option<&str>, metrics: &Metrics) -> Result<Option<Num>, FiguresError> {
    budget::within(|| profit(c, market, rates, quote, metrics))
}

fn profit(c: &Connection, market: &dyn MarketData, rates: &mut Rates, quote: Option<&str>, metrics: &Metrics) -> Result<Option<Num>, FiguresError> {
    let profit = (&metrics.total_amount_value_in_quote.to_d()? - &metrics.total_quote_amount_invested.to_d()?)?;
    if profit.is_zero() { return Ok(Some(Num::Dec(profit))); }
    let Some(currency) = quote else { return Ok(None) };
    let Some(rate) = rates.usd(c, market, currency)? else { return Ok(None) };
    Ok(Some(Num::Dec((&profit * &rate.to_d()?)?)))
}

/// The fiat an account's figures are shown in, and the USD rate into it (Denomination.for). USD when the rate
/// cannot be had: a currency sign on a dollar figure would be a lie.
#[derive(Clone, Debug, PartialEq)]
pub struct Denomination { pub currency: String, pub rate: Dec }

pub fn denomination(c: &Connection, market: &dyn MarketData, display_currency: &str) -> Result<Denomination, FiguresError> {
    budget::within(|| shown_in(c, market, display_currency))
}

fn shown_in(c: &Connection, market: &dyn MarketData, display_currency: &str) -> Result<Denomination, FiguresError> {
    let currency = display_currency.to_uppercase();
    let usd = || Denomination { currency: "USD".into(), rate: Dec::one() };
    if currency.trim().is_empty() || currency == "USD" { return Ok(usd()); }
    match lift(exchange_rate(c, market, "USD", &currency)?)? {
        Some(rate) => Ok(Denomination { rate: rate.to_d()?, currency }),
        None => Ok(usd()),
    }
}

/// One bot's part in the account's totals: every bot of the account that is not deleted is a part, whatever
/// became of its figures. The totals are of the whole account or they are not computed (`complete`).
pub struct Part<'a> {
    pub bot_id: i64,
    /// `bot.quote_asset&.symbol`.
    pub quote: Option<&'a str>,
    /// Whether it has a submitted order.
    pub traded: bool,
    /// Its figures: the live ones for `global_pnl` and its snapshot, the marked ones for `pnl_history`.
    /// `Ok(None)`: there are none yet (for the snapshot: Rails would hold none cached, because they are stale or
    /// the bot has no chart point). `Err`: Rails raised computing them, or this library does not compute them.
    pub figures: Result<Option<&'a Metrics>, &'a Absent>,
}

impl<'a> Part<'a> {
    fn metrics(&self) -> Option<&'a Metrics> { self.figures.ok().flatten() }
}

/// No total is of fewer bots than the account has. A bot this library does not compute (another type, a number
/// out of range, a history beyond the budget) makes every total not computed, with its reason, wherever it stands
/// among the parts: a number that left it out would be another account's.
fn complete(parts: &[Part<'_>]) -> Result<(), FiguresError> {
    match parts.iter().find_map(|part| part.figures.err().filter(|absent| matches!(absent, Absent::NotComputed(_)))) {
        Some(absent) => Err(FiguresError::from(absent)),
        None => Ok(()),
    }
}

/// A bot that has traded and whose figures the caller did not bring: no total without it either.
fn missing(part: &Part<'_>) -> FiguresError { FiguresError::NotComputed(format!("bot {} has traded and has no figures", part.bot_id)) }

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalPnl { pub percent: Num, pub profit_usd: Num }

impl GlobalPnl {
    pub fn to_json(&self) -> J { J::Obj(vec![("percent".to_string(), J::num(&self.percent)), ("profit_usd".to_string(), J::num(&self.profit_usd))]) }
}

/// `Hash.new(0)` summed per currency, in the order the currencies were first met.
fn by_currency(parts: &[Part<'_>], pick: impl Fn(&Metrics) -> &Num) -> Result<Vec<(String, Num)>, FiguresError> {
    let mut sums: Vec<(String, Num)> = vec![];
    for part in parts {
        let (Some(metrics), Some(quote)) = (part.metrics(), part.quote) else { continue };
        let at = match sums.iter().position(|(currency, _)| currency == quote) { Some(at) => at, None => { sums.push((quote.to_string(), Num::Int(0))); sums.len() - 1 } };
        sums[at].1 = sums[at].1.add(pick(metrics))?;
    }
    Ok(sums)
}

/// Utilities::Currency.batch_convert to USD: a Float zero, plus every amount at its rate.
fn batch_convert(c: &Connection, market: &dyn MarketData, rates: &mut Rates, amounts: &[(String, Num)]) -> Result<Option<Num>, FiguresError> {
    let mut total = Num::Float(0.0);
    for (currency, amount) in amounts {
        if amount.is_zero() { continue; }
        let converted = if currency.to_uppercase() == "USD" { amount.clone() } else {
            match rates.usd(c, market, currency)? {
                None => return Ok(None),
                Some(Quoted::Num(rate)) => amount.mul(&rate)?,
                // `amount * rate` with a String for a rate.
                Some(Quoted::Text(_)) => {
                    let class = match amount { Num::Int(_) => "Integer", Num::Dec(_) => "BigDecimal", Num::Float(_) => "Float" };
                    return Err(FiguresError::Raised(format!("TypeError: String can't be coerced into {class}")));
                }
            }
        };
        total = total.add(&converted)?;
    }
    Ok(Some(total))
}

fn result(invested: &Num, value: &Num) -> Result<GlobalPnl, FiguresError> {
    let profit_usd = value.sub(invested)?;
    Ok(GlobalPnl { percent: profit_usd.div(invested)?, profit_usd })
}

/// User#global_pnl: the account's profit over everything put in, in USD. None with no bots, no rate, or nothing
/// invested. Every part carries its live figures: what Rails raised computing one is raised here, and a bot
/// that has traded and has none makes the total not computed.
pub fn global_pnl(c: &Connection, market: &dyn MarketData, rates: &mut Rates, parts: &[Part<'_>]) -> Result<Option<GlobalPnl>, FiguresError> {
    budget::within(|| {
        complete(parts)?;
        for part in parts {
            match part.figures { Err(absent) => return Err(FiguresError::from(absent)), Ok(None) if part.traded => return Err(missing(part)), Ok(_) => {} }
        }
        total(c, market, rates, parts)
    })
}

fn total(c: &Connection, market: &dyn MarketData, rates: &mut Rates, parts: &[Part<'_>]) -> Result<Option<GlobalPnl>, FiguresError> {
    if !parts.iter().any(|part| part.metrics().is_some() && part.quote.is_some()) { return Ok(None); }
    let Some(invested) = batch_convert(c, market, rates, &by_currency(parts, |m| &m.total_quote_amount_invested)?)? else { return Ok(None) };
    let Some(value) = batch_convert(c, market, rates, &by_currency(parts, |m| &m.total_amount_value_in_quote)?)? else { return Ok(None) };
    if invested.is_zero() { return Ok(None); }
    Ok(Some(result(&invested, &value)?))
}

/// What the account's totals do not count, per bot, beside the totals and not in them. From the live figures: the
/// held assets `global_pnl` and its snapshot leave out, because the live pass could not price them. From the
/// marked figures: those, and the holdings `pnl_history`'s curve leaves out of its earlier points (`In::Chart`).
#[derive(Clone, Debug, PartialEq)]
pub struct LeftOut { pub bot_id: i64, pub of: In, pub holding: Unpriced }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum In {
    /// The value at current prices.
    Value,
    /// The points of the chart marked at market.
    Chart,
}

pub fn left_out(parts: &[Part<'_>]) -> Vec<LeftOut> {
    let mut out = vec![];
    for part in parts {
        let Some(metrics) = part.metrics() else { continue };
        out.extend(metrics.unpriced.iter().map(|holding| LeftOut { bot_id: part.bot_id, of: In::Value, holding: holding.clone() }));
        out.extend(metrics.chart_omitted.iter().map(|holding| LeftOut { bot_id: part.bot_id, of: In::Chart, holding: holding.clone() }));
    }
    out
}

/// User#global_pnl_snapshot: the same figure on three terms: ready, still loading, or nothing to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot<T> { pub result: Option<T>, pub loading: bool }

/// The parts carry what Rails would hold cached. A bot whose figures Rails raised computing has none cached, and
/// reads as one that has none yet; a bot this library does not compute has none to stand in for them.
pub fn global_pnl_snapshot(c: &Connection, market: &dyn MarketData, rates: &mut Rates, parts: &[Part<'_>]) -> Result<Snapshot<GlobalPnl>, FiguresError> {
    budget::within(|| { complete(parts)?; cached(c, market, rates, parts) })
}

fn cached(c: &Connection, market: &dyn MarketData, rates: &mut Rates, parts: &[Part<'_>]) -> Result<Snapshot<GlobalPnl>, FiguresError> {
    // A bot that has traded is expected to have figures shortly; one that never has contributes nothing.
    let mut loading = parts.iter().any(|part| part.metrics().is_none() && part.traded);
    let invested = batch_convert(c, market, rates, &by_currency(parts, |m| &m.total_quote_amount_invested)?)?;
    let value = batch_convert(c, market, rates, &by_currency(parts, |m| &m.total_amount_value_in_quote)?)?;
    loading = loading || invested.is_none() || value.is_none();
    let (Some(invested), Some(value), false) = (invested, value, loading) else { return Ok(Snapshot { result: None, loading: true }) };
    if invested.is_zero() { return Ok(Snapshot { result: None, loading: false }); }
    Ok(Snapshot { result: Some(result(&invested, &value)?), loading: false })
}

/// User::PnlHistory's curve: the account's profit at evenly spaced moments.
#[derive(Clone, Debug, PartialEq)]
pub struct History { pub percent: Vec<f64>, pub profit_usd: Vec<f64>, pub at: Vec<i64>, pub days: f64 }

impl History {
    pub fn to_json(&self) -> J {
        let floats = |serie: &Vec<f64>| J::arr(serie, |f| J::Float(*f));
        J::Obj(vec![
            ("percent".to_string(), floats(&self.percent)), ("profit_usd".to_string(), floats(&self.profit_usd)),
            ("at".to_string(), J::arr(&self.at, |at| J::Int(*at))), ("days".to_string(), J::Float(self.days)),
        ])
    }
}

/// Columns the curve is resampled onto (MAX_POINTS).
pub const MAX_POINTS: usize = 180;

/// One bot's readings: `(time, value in USD, invested in USD)`, ascending.
pub type Track = Vec<(At, Dec, Dec)>;

/// An instant `seconds` (a Float, taken exactly, as Ruby's Time#+ takes it) after `base`, in a fraction of a
/// nanosecond: `numerator / 2^shift` nanoseconds since the epoch.
struct Exact { numerator: BigInt, shift: u32 }

impl Exact {
    fn after(base: At, seconds: f64) -> Exact {
        let bits = seconds.to_bits();
        let (negative, exponent, fraction) = (bits >> 63 == 1, ((bits >> 52) & 0x7ff) as i64, bits & ((1 << 52) - 1));
        // seconds = mantissa x 2^power
        let (mantissa, power) = if exponent == 0 { (fraction, -1074) } else { (fraction | (1 << 52), exponent - 1075) };
        let nanos = BigInt::from(mantissa) * BigInt::from(1_000_000_000u64) * if negative { -1 } else { 1 };
        if power >= 0 { return Exact { numerator: BigInt::from(base.0) + (nanos << power as usize), shift: 0 }; }
        let shift = (-power) as u32;
        Exact { numerator: (BigInt::from(base.0) << shift as usize) + nanos, shift }
    }
    /// `reading <= self`
    fn reached_by(&self, reading: At) -> bool { (BigInt::from(reading.0) << self.shift as usize) <= self.numerator }
    /// Time#to_i: whole seconds, rounded down.
    fn seconds(&self) -> i64 {
        let divisor = BigInt::from(1_000_000_000u64) << self.shift as usize;
        let (quotient, remainder) = (&self.numerator / &divisor, &self.numerator % &divisor);
        let floor = if remainder < BigInt::from(0) { quotient - 1 } else { quotient };
        i64::try_from(floor).unwrap_or(i64::MAX)
    }
}

/// User::PnlHistory#merge: every bot holds its last reading until the next one, so the account's curve is a sum of
/// step functions read down a column at a time. One column more than there are readings, in front of them: the
/// account had made nothing before its first purchase.
pub fn merge(tracks: &[Track]) -> Result<Option<History>, FiguresError> {
    budget::within(|| merged(tracks))
}

fn merged(tracks: &[Track]) -> Result<Option<History>, FiguresError> {
    let first = tracks.iter().filter_map(|track| track.first()).map(|reading| reading.0).min();
    let last = tracks.iter().filter_map(|track| track.last()).map(|reading| reading.0).max();
    let (Some(first), Some(last)) = (first, last) else { return Ok(None) };
    let span = last.minus(first);
    if span <= 0.0 { return Ok(None); }
    let columns = (tracks.iter().map(Vec::len).sum::<usize>() + 1).clamp(3, MAX_POINTS);
    let step = span / (columns as f64 - 2.0);
    let mut cursors = vec![0usize; tracks.len()];
    let mut history = History { percent: vec![], profit_usd: vec![], at: vec![], days: span / 86_400.0 };
    for i in 0..columns {
        // The last column is the last reading itself: that point is the headline, and it has to be exact.
        let at = if i == 0 { Exact::after(first, -step) } else if i == columns - 1 { Exact::after(last, 0.0) } else { Exact::after(first, step * (i as f64 - 1.0)) };
        let (mut value, mut invested) = (Dec::zero(), Dec::zero());
        for (track, cursor) in tracks.iter().zip(cursors.iter_mut()) {
            while *cursor + 1 < track.len() && at.reached_by(track[*cursor + 1].0) { *cursor += 1; }
            let Some(reading) = track.get(*cursor) else { continue }; // a bot with no reading adds nothing
            if !at.reached_by(reading.0) { continue; } // this bot had not started yet
            value = (&value + &reading.1)?;
            invested = (&invested + &reading.2)?;
        }
        history.at.push(at.seconds());
        let pnl = (&value - &invested)?;
        history.profit_usd.push(pnl.to_f());
        // A moment before any money went in sits on the zero line.
        history.percent.push(if invested.is_positive() { pnl.div(&invested)?.to_f() } else { 0.0 });
    }
    Ok(Some(history))
}

/// User::PnlHistory.snapshot(live: true). Every part carries its marked figures.
/// Loading while a rate is missing: a partial curve under a total headline would be another account's history.
pub fn pnl_history(c: &Connection, market: &dyn MarketData, rates: &mut Rates, parts: &[Part<'_>]) -> Result<Snapshot<History>, FiguresError> {
    budget::within(|| { complete(parts)?; history(c, market, rates, parts) })
}

fn history(c: &Connection, market: &dyn MarketData, rates: &mut Rates, parts: &[Part<'_>]) -> Result<Snapshot<History>, FiguresError> {
    let mut tracks: Vec<Track> = vec![];
    for part in parts {
        // What Rails raised computing this bot's chart is raised here, when the walk over the bots reaches it.
        let metrics = match part.figures.map_err(FiguresError::from)? { Some(metrics) => metrics, None if part.traded => return Err(missing(part)), None => continue };
        if metrics.chart.labels.is_empty() { continue; }
        let Some(currency) = part.quote.filter(|q| !q.trim().is_empty()) else { continue };
        let Some(rate) = rates.usd(c, market, &currency.to_uppercase())? else { return Ok(Snapshot { result: None, loading: true }) };
        let rate = rate.to_d()?;
        let mut track = vec![];
        for (i, at) in metrics.chart.labels.iter().enumerate() {
            let reading = |serie: &Vec<Num>| serie.get(i).map_or(Ok(Dec::zero()), Num::to_d);
            track.push((*at, (&reading(&metrics.chart.value)? * &rate)?, (&reading(&metrics.chart.invested)? * &rate)?));
        }
        tracks.push(track);
    }
    Ok(Snapshot { result: merge(&tracks)?, loading: false })
}
