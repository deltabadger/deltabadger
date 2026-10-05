//! Bot::ChartSeries and Measurable#metrics_with_current_prices_and_candles: the chart with every point valued at
//! market. The ruler is a price grid per holding: the venue's candle opens, the live price as the last mark, the
//! bot's own fill prices where the candles do not reach, and two pinned marks either side of every split.
//! A point is re-marked only when every holding at that moment has a price there; an uncovered fill keeps the mark
//! the walk gave it, and an uncovered candle time is dropped.
use super::at::At;
use super::budget;
use super::db::{self, Subject};
use super::dec::Dec;
use super::json::{self, J};
use super::live::{ticker_for_key, unlisted, venue};
use super::market::MarketData;
use super::num::{Num, NumError};
use super::splits::{self, Event};
use super::walk::{AssetSeries, Chart, Metrics, Row, Unpriced};
use super::FiguresError;
use chrono_tz::Tz;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// `[[time, price], ...]`, ascending.
pub type Marks = Vec<(At, Dec)>;
/// `{ key => marks }` in Ruby's insertion order.
pub type Grids = Vec<(String, Marks)>;

/// How finely the buy marks are worth shipping (MARK_BUCKETS).
pub const MARK_BUCKETS: usize = 500;

fn grid_places(grids: &Grids) -> Result<HashMap<String, usize>, NumError> {
    let mut places = HashMap::new();
    for (i, (key, _)) in grids.iter().enumerate() { budget::charge(1, 0)?; places.entry(key.clone()).or_insert(i); }
    Ok(places)
}
fn set(grids: &mut Grids, places: &mut HashMap<String, usize>, key: &str, marks: Marks) {
    match places.get(key) {
        Some(&i) => grids[i].1 = marks,
        None => { places.insert(key.to_string(), grids.len()); grids.push((key.to_string(), marks)); }
    }
}

/// #optimal_candles_timeframe_for_duration: about 300 points when possible. Seconds in, seconds out.
pub fn timeframe(duration: f64) -> i64 {
    const MINUTE: f64 = 60.0;
    if duration < 300.0 * MINUTE { 60 }
    else if duration < 5.0 * 300.0 * MINUTE { 300 }
    else if duration < 15.0 * 300.0 * MINUTE { 900 }
    else if duration < 30.0 * 300.0 * MINUTE { 1800 }
    else if duration < 300.0 * 60.0 * MINUTE { 3600 }
    else { 86_400 }
}

/// #chart_grid_price: the price a grid reads at `time`. An exact mark, else a reading of the line between the two
/// marks around it; None outside the grid's reach.
pub fn grid_price(marks: &[(At, Dec)], time: At) -> Result<Option<Dec>, NumError> {
    let (Some(first), Some(last)) = (marks.first(), marks.last()) else { return Ok(None) };
    if time < first.0 || time > last.0 { return Ok(None); }
    let index = marks.partition_point(|mark| mark.0 < time);
    let Some(at) = marks.get(index) else { return Ok(None) };
    if at.0 == time || index == 0 { return Ok(Some(at.1.clone())); }
    let before = &marks[index - 1];
    let span = at.0.minus(before.0);
    if span == 0.0 { return Ok(Some(before.1.clone())); }
    // Multiplied before it is divided, as Rails does: (30 x 3600) / 10800 is exactly 10.
    let elapsed = Num::Float(time.minus(before.0)).to_d()?;
    let rise = (&(&at.1 - &before.1)? * &elapsed)?;
    let step = rise.div(&Num::Float(span).to_d()?)?;
    Ok(Some((&before.1 + &step)?))
}

/// One fill as the chart marks it under the plot: when, into which holding, how much, for how much, and how many
/// fills the mark stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct BuyMark { pub at: At, pub key: String, pub amount: Dec, pub quote: Dec, pub fills: i64 }

/// #chart_thinned_marks: one mark per holding per 1/500th of the history, timed at the first fill in it and summed
/// over all of them. Only when there are more marks than buckets.
pub fn thinned_marks(marks: Vec<BuyMark>) -> Result<Vec<BuyMark>, NumError> {
    if marks.len() <= MARK_BUCKETS { return Ok(marks); }
    let (first, last) = (marks[0].at, marks[marks.len() - 1].at);
    let span = last.minus(first);
    if span == 0.0 { return Ok(marks); }
    // Ruby groups by [bucket, key] in a Hash: the groups keep the order they were first met in.
    let mut sums: Vec<BuyMark> = vec![];
    let mut place: HashMap<(u64, String), usize> = HashMap::new();
    for mark in marks {
        budget::charge(1, 0)?;
        let bucket = (mark.at.minus(first) / span * MARK_BUCKETS as f64).floor();
        match place.get(&(bucket.to_bits(), mark.key.clone())) {
            Some(&at) => {
                let sum = &mut sums[at];
                (sum.amount, sum.quote, sum.fills) = ((&sum.amount + &mark.amount)?, (&sum.quote + &mark.quote)?, sum.fills + mark.fills);
            }
            None => { place.insert((bucket.to_bits(), mark.key.clone()), sums.len()); sums.push(mark); }
        }
    }
    Ok(sums)
}

/// Measurable#chart_row_key: the key a row is charted under, its holding's.
struct RowKeys<'a> { assets: HashMap<i64, &'a str>, strings: HashMap<&'a str, &'a str> }
impl<'a> RowKeys<'a> {
    fn new(metrics: &'a Metrics) -> Result<Self, FiguresError> {
        let mut assets = HashMap::new();
        let mut unresolved = HashSet::new();
        for (key, id) in &metrics.key_assets {
            budget::charge(1, 0)?;
            match id { Some(id) => { assets.entry(*id).or_insert(key.as_str()); }, None => { unresolved.insert(key.as_str()); } }
        }
        let mut strings = HashMap::new();
        for (key, names) in &metrics.key_strings {
            budget::charge(1, 0)?;
            if !unresolved.contains(key.as_str()) { continue; }
            for name in names { budget::charge(1, 0)?; strings.insert(name.as_str(), key.as_str()); }
        }
        Ok(Self { assets, strings })
    }
    fn key(&self, base: Option<&str>, asset_id: Option<i64>) -> String {
        let base = base.unwrap_or_default();
        match asset_id { Some(id) => self.assets.get(&id), None => self.strings.get(base) }.copied().unwrap_or(base).to_string()
    }
}

/// #chart_buy_marks: the buys the walk counts, as executed, oldest first.
/// ponytail: Rails orders these by created_at alone, so two buys of one moment come as SQLite returns them; here
/// they come by id, which is what its index gives. Only the order of two marks of one instant could differ.
pub fn buy_marks(s: &Subject, metrics: &Metrics) -> Result<Vec<BuyMark>, FiguresError> {
    let mut marks = vec![];
    let keys = RowKeys::new(metrics)?;
    for order in &s.orders {
        budget::charge(1, 0)?;
        let Some(fill)=super::fill::parse(order)? else{continue};
        if !order.buy { continue; }
        let (amount,quote)=(fill.quantity,fill.value);
        marks.push(BuyMark { at: order.at, key: keys.key(order.base.as_deref(), order.asset_id), amount, quote, fills: 1 });
    }
    Ok(thinned_marks(marks)?)
}

/// #chart_fill_marks_by_symbol: the price each fill marked its holding at. An order that executed nothing marks
/// nothing; both reported and price-derived execution values mark a fill. The later same-time fill stands.
fn fill_marks(s: &Subject, metrics: &Metrics) -> Result<Grids, FiguresError> {
    let mut out: Grids = vec![];
    let keys = RowKeys::new(metrics)?;
    let mut places = HashMap::new();
    for order in &s.orders {
        budget::charge(1, 0)?;
        let Some(fill)=super::fill::parse(order)? else{continue};
        let price=fill.unit_price()?;
        let key = keys.key(order.base.as_deref(), order.asset_id);
        let at = *places.entry(key.clone()).or_insert_with(|| { out.push((key, vec![])); out.len() - 1 });
        // The orders come oldest first, so a fill of the same moment can only be the last mark so far.
        match out[at].1.last_mut().filter(|(time, _)| *time == order.at) { Some(mark) => mark.1 = price.clone(), None => out[at].1.push((order.at, price.clone())) }
    }
    Ok(out)
}

/// #chart_backfilled_grids: what a holding's candles do not span is filled in from the bot's own fill prices. Only
/// outside the candles' span, and only on the candles' side of any split.
fn backfilled(mut grids: Grids, symbols: &[String], from: At, to: At, events: &[Event], s: &Subject, metrics: &Metrics) -> Result<Grids, FiguresError> {
    let mut places = grid_places(&grids)?;
    let mut fills: Option<HashMap<String, Marks>> = None;
    for symbol in symbols {
        budget::charge(1, 0)?;
        let marks = places.get(symbol).map(|&i| &grids[i].1);
        budget::charge(marks.map_or(0, |marks| marks.len()) as u64, 0)?;
        let marks = marks.cloned().unwrap_or_default();
        if let (Some(first), Some(last)) = (marks.first(), marks.last()) { if first.0 <= from && last.0 >= to { continue; } }
        let fills = match &mut fills {
            Some(fills) => fills,
            none => {
                let rows = fill_marks(s, metrics)?;
                budget::charge(rows.len() as u64, 0)?;
                none.insert(rows.into_iter().collect())
            }
        };
        let extra = fills.get(symbol);
        budget::charge(extra.map_or(0, |marks| marks.len()) as u64 + 2 * events.len() as u64, 0)?;
        let Some(mut extra) = extra.cloned().filter(|extra| !extra.is_empty()) else { continue };
        if let (Some(first), Some(last)) = (marks.first().map(|m| m.0), marks.last().map(|m| m.0)) {
            let cuts = || events.iter().filter(|event| event.key == *symbol).map(|event| event.at);
            let floor = cuts().filter(|at| *at <= first).max();
            let ceiling = cuts().filter(|at| *at > last).min();
            extra.retain(|(time, _)| !((first <= *time && *time <= last) || floor.is_some_and(|floor| *time < floor) || ceiling.is_some_and(|ceiling| *time >= ceiling)));
        }
        let mut all = marks;
        all.extend(extra);
        all.sort_by_key(|mark| mark.0);
        budget::check()?;
        set(&mut grids, &mut places, symbol, all);
    }
    Ok(grids)
}

/// #chart_split_pinned_grids: the last price before a split, pinned one second before it, and the first price
/// after it, pinned at it, so no point is read off a line drawn between two share bases.
fn split_pinned(mut grids: Grids, events: &[Event]) -> Result<Grids, FiguresError> {
    let mut places = grid_places(&grids)?;
    for event in events {
        budget::charge(1, 0)?;
        let Some(marks) = places.get(&event.key).map(|&i| &grids[i].1).filter(|marks| !marks.is_empty()) else { continue };
        let after_index = marks.partition_point(|(time, _)| *time < event.at);
        budget::check()?;
        let (Some(before), Some(after)) = (after_index.checked_sub(1).and_then(|i| marks.get(i)), marks.get(after_index)) else { continue };
        let second_before = event.at.plus_seconds(-1).ok_or_else(|| FiguresError::Data("a split at the edge of time".into()))?;
        budget::charge(4 * marks.len() as u64, 0)?;
        let mut pinned = marks.clone();
        for pin in [(second_before, before.1.clone()), (event.at, after.1.clone())] {
            if !pinned.iter().any(|(time, _)| *time == pin.0) { pinned.push(pin); }
        }
        pinned.sort_by_key(|mark| mark.0);
        budget::check()?;
        set(&mut grids, &mut places, &event.key, pinned);
    }
    Ok(grids)
}

struct Window<'a> { s: &'a Subject, metrics: &'a Metrics, market: &'a dyn MarketData, since: At, timeframe: i64, now: At }

impl Window<'_> {
    /// #chart_candle_grids over CandleSeriesCache with nothing cached: the closed candles' opens, each time once,
    /// ascending, then the live price as the last mark. A holding whose candles fail or are empty gets no grid.
    fn candle_grids(&self, symbols: &[String], restated: bool) -> Result<Grids, FiguresError> {
        let mut grids: Grids = vec![];
        let venue = venue(self.s)?;
        for symbol in symbols {
        budget::charge(1, 0)?;
            let Some(ticker) = ticker_for_key(self.s, self.metrics, symbol) else { continue };
            // A failure of either kind is this holding's alone: Rails' fetch threads rescue what a client raises.
            let candles = match self.market.candles(&venue, ticker, self.since, self.timeframe, restated && self.s.restated_candles(ticker)) {
                Ok(candles) => candles,
                Err(super::market::Failure::NotComputed(reason)) => return Err(FiguresError::NotComputed(reason)),
                Err(_) => continue,
            };
            let mut marks: Marks = vec![];
            // Of two candles of one time the first stands (Ruby's uniq). Keyed by the nanoseconds: a set of `At`
            // would compare instants as its buckets happen to fall, and the steps would not be the same twice.
            let mut seen: HashSet<i64> = HashSet::new();
            for (time, open) in candles {
                budget::charge(1, 0)?;
                let closed = time.plus_seconds(self.timeframe).is_some_and(|close| close <= self.now);
                if closed && seen.insert(time.0) { marks.push((time, open)); }
            }
            if marks.is_empty() { continue; }
            marks.sort_by_key(|mark| mark.0);
            budget::check()?;
            if let (Some(price), Some(last)) = (self.metrics.live_prices.as_ref().and_then(|prices| prices.iter().find(|(k, _)| k == symbol)), self.metrics.chart.labels.last()) {
                marks.push((*last, price.1.to_d()?));
                marks.sort_by_key(|mark| mark.0);
            budget::check()?;
            }
            grids.push((symbol.clone(), marks));
        }
        Ok(grids)
    }
}

fn charge_chart(chart: &Chart) -> Result<(), NumError> {
    budget::charge(chart.labels.len() as u64, 0)?;
    for row in chart.extra.iter().chain(&chart.invested_by) { budget::charge(1 + row.len() as u64, 0)?; }
    Ok(())
}

/// #chart_marked_at_market. `priceable` are the holdings the bot can price at all; only they take part.
pub fn marked_at_market(chart: &Chart, grids: &Grids, display: &Grids, priceable: &[String]) -> Result<Chart, FiguresError> {
    let Some(first) = chart.labels.first() else { return Ok(chart.clone()) };
    // The walk's rows run parallel to its labels; a chart from anywhere else is refused here, not indexed past its end.
    if [chart.value.len(), chart.invested.len(), chart.extra.len(), chart.invested_by.len(), chart.cash.len()] != [chart.labels.len(); 5] {
        return Err(FiguresError::Data("a chart whose series are not of one length".into()));
    }
    let mut transaction_times = HashSet::new();
    let mut axis = vec![];
    for label in &chart.labels { budget::charge(1, 0)?; transaction_times.insert(label.0); axis.push(*label); }
    let mut grid_lookup = HashMap::new();
    for (key, marks) in grids {
        budget::charge(1, 0)?;
        grid_lookup.entry(key.as_str()).or_insert(marks);
        for (time, _) in marks { budget::charge(1, 0)?; if time >= first { axis.push(*time); } }
    }
    axis.sort();
    axis.dedup();
    budget::check()?;
    let slice = |row: &Row| -> Result<Row, NumError> {
        let mut lookup = HashMap::new();
        for (key, value) in row { budget::charge(1, 0)?; lookup.entry(key).or_insert(value); }
        let mut out = vec![];
        for key in priceable {
            budget::charge(1, 0)?;
            if let Some(value) = lookup.get(key) { out.push((key.clone(), (*value).clone())); }
        }
        Ok(out)
    };
    charge_chart(chart)?;
    let mut marked = Chart { extra: chart.extra.clone(), invested_by: chart.invested_by.clone(), cash: chart.cash.clone(), ..Chart::default() };
    let mut prices: Vec<(String, Vec<Option<Num>>)> = display.iter().map(|(key, _)| (key.clone(), vec![])).collect();
    let mut split_rows: Vec<Option<Row>> = vec![];
    let mut basis_rows: Vec<Row> = vec![];
    let mut cursor = 0;
    for time in axis {
        budget::charge(1, 0)?;
        // Post-trade holdings: a transaction's own timestamp lands on that transaction, and where several share
        // one, on the last of them.
        while cursor + 1 < chart.labels.len() && chart.labels[cursor + 1] <= time { budget::charge(1, 0)?; cursor += 1; }
        let mut row = cursor;
        let mut held = slice(&chart.extra[row])?;
        if row > 0 && held.is_empty() { row -= 1; held = slice(&chart.extra[row])?; }
        // What each holding is worth at market, or None unless every holding has a price here.
        let mut split: Option<Row> = Some(vec![]);
        for (key, amount) in &held {
            let amount = amount.to_d()?;
            let value = if amount.is_zero() { Some(Dec::zero()) } else { grid_price(grid_lookup.get(key.as_str()).map_or(&[][..], |marks| marks.as_slice()), time)?.map(|price| &amount * &price).transpose()? };
            match (value, split.as_mut()) { (Some(value), Some(values)) => values.push((key.clone(), Num::Dec(value))), _ => { split = None; break; } }
        }
        match &split {
            Some(values) => {
                let mut at_market = Num::Int(0);
                for (_, value) in values { at_market = at_market.add(value)?; }
                marked.value.push(at_market.add(&chart.cash[cursor])?);
            }
            None if transaction_times.contains(&time.0) => marked.value.push(chart.value[cursor].clone()),
            None => continue,
        }
        marked.labels.push(time);
        marked.invested.push(chart.invested[cursor].clone());
        for ((_, serie), (_, marks)) in prices.iter_mut().zip(display) { budget::charge(1, 0)?; serie.push(grid_price(marks, time)?.map(Num::Dec)); }
        split_rows.push(split);
        basis_rows.push(slice(&chart.invested_by[row])?);
    }
    // #chart_asset_series: the per-point splits turned into one series per holding.
    let mut symbols = vec![];
    let mut seen = HashSet::new();
    let mut split_lookup = vec![];
    for split in &split_rows {
        budget::charge(1, 0)?;
        let mut lookup = HashMap::new();
        if let Some(row) = split {
            for (key, value) in row {
                budget::charge(1, 0)?;
                if seen.insert(key) { symbols.push(key); }
                lookup.entry(key).or_insert(value);
            }
        }
        split_lookup.push(split.as_ref().map(|_| lookup));
    }
    let mut basis_lookup = vec![];
    for row in &basis_rows {
        budget::charge(1, 0)?;
        let mut lookup = HashMap::new();
        for (key, value) in row { budget::charge(1, 0)?; lookup.entry(key).or_insert(value); }
        basis_lookup.push(lookup);
    }
    let mut assets = vec![];
    for key in symbols {
        budget::charge(1, 0)?;
        let mut value = vec![];
        let mut invested = vec![];
        for (split, basis) in split_lookup.iter().zip(&basis_lookup) {
            budget::charge(1, 0)?;
            value.push(split.as_ref().map(|row| row.get(key).map_or(Num::Int(0), |n| (*n).clone())));
            invested.push(basis.get(key).map_or(Num::Int(0), |n| (*n).clone()));
        }
        assets.push((key.clone(), AssetSeries { value, invested }));
    }
    marked.assets = Some(assets);
    marked.prices = Some(prices);
    Ok(marked)
}

/// Measurable#metrics_with_current_prices_and_candles: `live`'s figures with the chart marked at market. With no
/// candles for any holding, the fill marks the walk drew are the chart.
pub fn marked(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
    budget::within(|| at_market(c, s, live, market, now))
}

fn at_market(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
    charge_chart(&live.chart)?;
    let mut data = live.clone();
    let (Some(first), Some(last)) = (data.chart.labels.first().copied(), data.chart.labels.last().copied()) else { return Ok(data) };
    let symbols: Vec<String> = data.asset_breakdown.iter().map(|(key, _)| key.clone()).collect();
    if s.tickers.is_empty() || symbols.is_empty() { return Ok(data); }
    // Candles from one timeframe before the first transaction, so the first buy has a mark below it.
    let timeframe = timeframe(now.minus(first));
    let since = first.plus_seconds(-timeframe).ok_or_else(|| FiguresError::Data("a first order at the edge of time".into()))?;
    let window = Window { s, metrics: &data, market, since, timeframe, now };
    let grids = window.candle_grids(&symbols, false)?;
    if grids.is_empty() { return Ok(data); }
    let events = splits::events(c, s.bot.user_id, &s.orders, &data.holdings()?, now)?;
    let grids = split_pinned(backfilled(grids, &symbols, first, last, &events, s, &data)?, &events)?;

    // The price overlay is read off each restating holding's history as its venue reads it today, one basis end
    // to end; everything else stays on the valuation grids.
    let restating: Vec<String> = symbols.iter().filter(|key| ticker_for_key(s, &data, key).is_some_and(|ticker| s.restated_candles(ticker))).cloned().collect();
    let mut display = grids.clone();
    if !restating.is_empty() {
        let mut places = grid_places(&display)?;
        for (key, marks) in window.candle_grids(&restating, true)? { budget::charge(1, 0)?; set(&mut display, &mut places, &key, marks); }
    }

    let priceable: Vec<String> = symbols.iter().filter(|key| ticker_for_key(s, &data, key).is_some()).cloned().collect();
    // What Rails leaves out in silence is named beside the chart: a holding with no ticker today is no part of any
    // point's value, though what was paid for it stays in what went in. `live` names such a holding only while it
    // is held; one sold out since is named here.
    let priceable_set: HashSet<_> = priceable.iter().collect();
    let mut held_once = HashSet::new();
    for row in &data.chart.extra {
        budget::charge(1, 0)?;
        for (key, amount) in row { budget::charge(1, 0)?; if amount.is_positive() { held_once.insert(key); } }
    }
    budget::charge(data.key_assets.len() as u64, 0)?;
    let key_assets: HashMap<_, _> = data.key_assets.iter().map(|(key, id)| (key, *id)).collect();
    for key in symbols.iter().filter(|key| !priceable_set.contains(key)) {
        budget::charge(1, 0)?;
        let held_once = held_once.contains(key);
        if !held_once { continue; }
        let asset_id = key_assets.get(key).copied().flatten();
        data.chart_omitted.push(Unpriced { key: key.clone(), asset_id, reason: unlisted(c, s, asset_id)? });
    }
    data.chart = marked_at_market(&data.chart, &grids, &display, &priceable)?;
    Ok(data)
}

/// What the chart on the bot's page is drawn from: the data attributes of `bots/_chart`.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub bot: i64,
    pub quote: String,
    pub decimals: i64,
    pub labels: Vec<At>,
    /// [value, invested], rounded once to the quote's precision.
    pub series: [Vec<Num>; 2],
    /// value minus invested, off the rounded series.
    pub pnl: Vec<f64>,
    /// Per holding: its value at each point (None where the point kept its fill mark) and what it had cost.
    pub assets: Vec<(String, Vec<Option<f64>>, Vec<f64>)>,
    /// Balances are hidden: the chart shows the P/L curve only.
    pub pnl_only: bool,
    pub buys: Vec<(At, String, f64, f64, i64)>,
    pub prices: Vec<(String, Vec<Option<f64>>)>,
    /// The asset behind each charted key, for its logo and colour: every key bought or priced.
    pub logo_assets: Vec<(String, i64)>,
}

/// `bot.decimals[:quote] || 8`: the coarsest quote precision among the bot's tickers.
fn quote_decimals(c: &Connection, s: &Subject) -> Result<i64, FiguresError> {
    let quotes: Vec<Option<i64>> = if s.tickers.is_empty() && s.bot.kind == db::Kind::Basket { db::composition_quote_decimals(c, s.bot.id)? } else { s.tickers.iter().map(|t| t.quote_decimals).collect() };
    Ok(quotes.into_iter().flatten().min().unwrap_or(8))
}

/// The chart of a bot's page, or None for a bot with nothing to plot.
pub fn page(c: &Connection, s: &Subject, marked: &Metrics, hide_balances: bool) -> Result<Option<Page>, FiguresError> {
    budget::within(|| drawn(c, s, marked, hide_balances))
}

fn drawn(c: &Connection, s: &Subject, marked: &Metrics, hide_balances: bool) -> Result<Option<Page>, FiguresError> {
    if marked.chart.labels.is_empty() { return Ok(None); }
    let decimals = quote_decimals(c, s)?;
    let quote = s.quote.clone().ok_or_else(|| FiguresError::Data(format!("bot {} has no quote asset", s.bot.id)))?;
    let round = |serie: &Vec<Num>| serie.iter().map(|amount| amount.round(decimals)).collect::<Result<Vec<Num>, NumError>>();
    let series = [round(&marked.chart.value)?, round(&marked.chart.invested)?];
    let mut pnl = vec![];
    for (i, value) in series[0].iter().enumerate() {
        let invested = series[1].get(i).map_or(Ok(Dec::zero()), Num::to_d)?;
        pnl.push((&value.to_d()? - &invested)?.to_f());
    }
    let mut assets = vec![];
    for (key, serie) in marked.chart.assets.iter().flatten() {
        budget::charge(1 + serie.value.len() as u64 + serie.invested.len() as u64, 0)?;
        let value = serie.value.iter().map(|amount| amount.as_ref().map(|amount| amount.round(decimals).map(|n| n.to_f())).transpose()).collect::<Result<Vec<_>, _>>()?;
        let invested = serie.invested.iter().map(|amount| Num::Dec(amount.to_d()?).round(decimals).map(|n| n.to_f())).collect::<Result<Vec<_>, _>>()?;
        assets.push((key.clone(), value, invested));
    }
    for (_, serie) in marked.chart.prices.iter().flatten() { budget::charge(1 + serie.len() as u64, 0)?; }
    let prices: Vec<(String, Vec<Option<f64>>)> = marked.chart.prices.iter().flatten()
        .map(|(key, serie)| (key.clone(), serie.iter().map(|price| price.as_ref().map(Num::to_f)).collect())).collect();
    let marks = buy_marks(s, marked)?;

    // Allocatable#holding_assets, for every key bought or priced: by id, or for a holding known only by its
    // string, the asset the venue spells that way.
    let mut keys = vec![];
    let mut seen = HashSet::new();
    for key in marks.iter().map(|mark| &mark.key).chain(prices.iter().map(|(key, _)| key)) {
        budget::charge(1, 0)?;
        if seen.insert(key) { keys.push(key); }
    }
    let mut key_assets = HashMap::new();
    let mut ids = vec![];
    for (key, id) in &marked.key_assets {
        budget::charge(1, 0)?;
        key_assets.entry(key).or_insert(id);
        if let Some(id) = id { ids.push(*id); }
    }
    let existing: HashSet<_> = db::existing_assets(c, &ids)?.into_iter().collect();
    let mut logo_assets = vec![];
    for key in keys {
        budget::charge(1, 0)?;
        let Some(asset_id) = key_assets.get(key) else { continue };
        let id = match asset_id {
            Some(id) => existing.contains(id).then_some(*id),
            None => ticker_for_key(s, marked, key).filter(|ticker| ticker.base_asset_exists).map(|ticker| ticker.base_asset_id),
        };
        if let Some(id) = id { logo_assets.push((key.clone(), id)); }
    }

    Ok(Some(Page {
        bot: s.bot.id, quote, decimals, labels: marked.chart.labels.clone(), series, pnl, assets, pnl_only: hide_balances,
        buys: marks.into_iter().map(|mark| (mark.at, mark.key, mark.amount.to_f(), mark.quote.to_f(), mark.fills)).collect(),
        prices, logo_assets,
    }))
}

impl Page {
    /// The `data-bot--chart-<name>-value` attributes, each value as Rails' tag helper writes it: a string as it
    /// is, anything else as JSON. Times are in `zone` with `digits` fractional digits (Rails: 3).
    pub fn attributes(&self, zone: &Tz, digits: usize) -> Vec<(&'static str, String)> {
        let time = |at: &At| J::Str(json::time(&at.utc().with_timezone(zone), digits));
        let floats = |serie: &Vec<f64>| J::arr(serie, |f| J::Float(*f));
        let optional = |serie: &Vec<Option<f64>>| J::arr(serie, |f| J::opt(f, |f| J::Float(*f)));
        vec![
            ("bot", self.bot.to_string()),
            ("quote", self.quote.clone()),
            ("decimals", self.decimals.to_string()),
            ("labels", J::arr(&self.labels, time).write()),
            ("series", J::Arr(self.series.iter().map(|serie| J::arr(serie, J::num)).collect()).write()),
            ("pnl", floats(&self.pnl).write()),
            ("assets", J::Obj(self.assets.iter().map(|(key, value, invested)| (key.clone(), J::Obj(vec![
                ("value".to_string(), optional(value)), ("invested".to_string(), floats(invested)),
            ]))).collect()).write()),
            ("pnl-only", self.pnl_only.to_string()),
            ("buys", J::arr(&self.buys, |(at, key, amount, quote, fills)| J::Arr(vec![time(at), J::Str(key.clone()), J::Float(*amount), J::Float(*quote), J::Int(*fills)])).write()),
            ("prices", J::Obj(self.prices.iter().map(|(key, serie)| (key.clone(), optional(serie))).collect()).write()),
            ("logo-assets", J::Obj(self.logo_assets.iter().map(|(key, id)| (key.clone(), J::Int(*id))).collect()).write()),
        ]
    }
}
