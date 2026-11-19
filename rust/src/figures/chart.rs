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
/// `{ key => marks }` in 2.x's insertion order.
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
    // Multiplied before it is divided, as 2.x does: (30 x 3600) / 10800 is exactly 10.
    let elapsed = Num::Float(time.minus(before.0)).to_d()?;
    let rise = (&(&at.1 - &before.1)? * &elapsed)?;
    let step = rise.div(&Num::Float(span).to_d()?)?;
    Ok(Some((&before.1 + &step)?))
}

/// One fill as the chart marks it under the plot: when, into which holding, how much, for how much, and how many
/// fills the mark stands for. The key is None for a pair bot's tickerless row recorded without a base (2.x's nil key).
#[derive(Clone, Debug, PartialEq)]
pub struct BuyMark { pub at: At, pub key: Option<String>, pub amount: Dec, pub quote: Dec, pub fills: i64 }

/// #chart_thinned_marks: one mark per holding per 1/500th of the history, timed at the first fill in it and summed
/// over all of them. Only when there are more marks than buckets.
pub fn thinned_marks(marks: Vec<BuyMark>) -> Result<Vec<BuyMark>, NumError> {
    if marks.len() <= MARK_BUCKETS { return Ok(marks); }
    let (first, last) = (marks[0].at, marks[marks.len() - 1].at);
    let span = last.minus(first);
    if span == 0.0 { return Ok(marks); }
    // 2.x groups by [bucket, key] in a Hash: the groups keep the order they were first met in.
    let mut sums: Vec<BuyMark> = vec![];
    let mut place: HashMap<(u64, Option<String>), usize> = HashMap::new();
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
/// ponytail: 2.x orders these by created_at alone, so two buys of one moment come as SQLite returns them; here
/// they come by id, which is what its index gives. Only the order of two marks of one instant could differ.
pub fn buy_marks(s: &Subject, metrics: &Metrics) -> Result<Vec<BuyMark>, FiguresError> {
    let mut marks = vec![];
    let keys = RowKeys::new(metrics)?;
    for order in &s.orders {
        budget::charge(1, 0)?;
        let Some(fill)=super::fill::parse(order)? else{continue};
        if !order.buy { continue; }
        let (amount,quote)=(fill.quantity,fill.value);
        marks.push(BuyMark { at: order.at, key: Some(keys.key(order.base.as_deref(), order.asset_id)), amount, quote, fills: 1 });
    }
    Ok(thinned_marks(marks)?)
}

/// A pair bot's #chart_buy_marks: the same rows, each keyed by Bot::ChartSeries#chart_row_key, the ticker's base, else
/// the row's own base as it is (nil included). A buy whose value cannot be known marks nothing, as 2.x skips it.
pub fn pair_buy_marks(s: &Subject, ticker_base: Option<&str>) -> Result<Vec<BuyMark>, FiguresError> {
    let mut marks = vec![];
    for order in &s.orders {
        budget::charge(1, 0)?;
        if !order.buy { continue; }
        let fill = match super::fill::mark(order) {
            Ok(fill) => fill,
            Err(FiguresError::NotComputed(why)) if why == super::fill::VALUE_UNAVAILABLE => None,
            Err(e) => return Err(e),
        };
        let Some(fill) = fill else { continue };
        let key = ticker_base.map(str::to_string).or_else(|| order.base.clone());
        marks.push(BuyMark { at: order.at, key, amount: fill.quantity, quote: fill.value, fills: 1 });
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
        let Some(fill)=super::fill::mark(order)? else{continue};
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

/// What a candle fetch that raises does to the chart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// That holding gets no grid: a composition's fetch threads rescue what a client raises (#chart_candle_grids), as
    /// every overlay fetch does (#chart_display_grids).
    Rescue,
    /// The chart is raised to the caller's retry: a pair's valuation fetch is not rescued (#chart_price_grids).
    Propagate,
}

impl Window<'_> {
    /// #chart_candle_grids over CandleSeriesCache with nothing cached: the closed candles' opens, each time once,
    /// ascending, then the live price as the last mark. A holding whose candles fail or are empty gets no grid.
    fn candle_grids(&self, symbols: &[String], restated: bool) -> Result<Grids, FiguresError> {
        let mut keyed = vec![];
        for symbol in symbols { budget::charge(1, 0)?; if let Some(ticker) = ticker_for_key(self.s, self.metrics, symbol) { keyed.push((symbol.clone(), ticker)); } }
        self.grids(&keyed, restated, Policy::Rescue)
    }

    fn grids(&self, keyed: &[(String, &db::Ticker)], restated: bool, policy: Policy) -> Result<Grids, FiguresError> {
        let mut grids: Grids = vec![];
        let venue = venue(self.s)?;
        for (symbol, ticker) in keyed {
            budget::charge(1, 0)?;
            let candles = match self.market.candles(&venue, ticker, self.since, self.timeframe, restated && self.s.restated_candles(ticker)) {
                Ok(candles) => candles,
                Err(super::market::Failure::NotComputed(reason)) => return Err(FiguresError::NotComputed(reason)),
                Err(super::market::Failure::Raised(error)) if policy == Policy::Propagate => return Err(FiguresError::Raised(error)),
                Err(_) => continue,
            };
            let mut marks: Marks = vec![];
            // Of two candles of one time the first stands (2.x's uniq). Keyed by the nanoseconds: a set of `At`
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

/// #chart_marked_at_market. `priceable` are the holdings the bot can price at all; only they take part. `basis`: the
/// chart also carries the per-holding split (`assets`), as a composition's does; a pair's has none.
pub fn marked_at_market(chart: &Chart, grids: &Grids, display: &Grids, priceable: &[String], basis: bool) -> Result<Chart, FiguresError> {
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
    marked.assets = basis.then_some(assets);
    marked.prices = Some(prices);
    Ok(marked)
}

/// Measurable#metrics_with_current_prices_and_candles: `live`'s figures with the chart marked at market. With no
/// candles for any holding, the fill marks the walk drew are the chart.
pub fn marked(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
    marked_at(c, s, live, market, now).map(|(data, _)| data)
}
/// `marked`, and whether any holding had a candle grid to mark it with. 2.x caches the marked chart only then
/// (`metrics_with_current_prices_and_candles` returns from its cache block when the grids are blank), so a bot page draws
/// it inline only then; otherwise the chart arrives in its own frame.
pub fn marked_at(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<(Metrics, bool), FiguresError> {
    budget::within(|| at_market(c, s, live, market, now))
}

fn at_market(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<(Metrics, bool), FiguresError> {
    charge_chart(&live.chart)?;
    let mut data = live.clone();
    let (Some(first), Some(last)) = (data.chart.labels.first().copied(), data.chart.labels.last().copied()) else { return Ok((data, false)) };
    let symbols: Vec<String> = data.asset_breakdown.iter().map(|(key, _)| key.clone()).collect();
    if s.tickers.is_empty() || symbols.is_empty() { return Ok((data, false)); }
    // Candles from one timeframe before the first transaction, so the first buy has a mark below it.
    let timeframe = timeframe(now.minus(first));
    let since = first.plus_seconds(-timeframe).ok_or_else(|| FiguresError::Data("a first order at the edge of time".into()))?;
    let window = Window { s, metrics: &data, market, since, timeframe, now };
    let grids = window.candle_grids(&symbols, false)?;
    if grids.is_empty() { return Ok((data, false)); }
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
    // What 2.x leaves out in silence is named beside the chart: a holding with no ticker today is no part of any
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
    data.chart = marked_at_market(&data.chart, &grids, &display, &priceable, true)?;
    Ok((data, true))
}

/// Bots::DcaSingleAsset::Measurable#metrics_with_current_prices_and_candles: `live`'s chart marked on its ticker's
/// candles, under `key` (the ticker's base). The valuation fetch raises to the caller (no rescue); the overlay's falls
/// back to the valuation grid; the grid is pinned at `events` and never backfilled from fills. Whether a grid was had:
/// 2.x caches the result only then, stale live figures or not.
pub fn pair_marked(s: &Subject, live: &Metrics, key: &str, ticker: &db::Ticker, events: &[Event], market: &dyn MarketData, now: At) -> Result<(Metrics, bool), FiguresError> {
    budget::within(|| {
        charge_chart(&live.chart)?;
        let mut data = live.clone();
        let Some(first) = data.chart.labels.first().copied() else { return Ok((data, false)) };
        let timeframe = timeframe(now.minus(first));
        let since = first.plus_seconds(-timeframe).ok_or_else(|| FiguresError::Data("a first order at the edge of time".into()))?;
        let window = Window { s, metrics: &data, market, since, timeframe, now };
        let keyed = [(key.to_string(), ticker)];
        let grids = window.grids(&keyed, false, Policy::Propagate)?;
        if grids.is_empty() { return Ok((data, false)); }
        let grids = split_pinned(grids, events)?;
        let mut display = grids.clone();
        if s.restated_candles(ticker) {
            let restated = window.grids(&keyed, true, Policy::Rescue)?;
            if !restated.is_empty() { display = restated; }
        }
        data.chart = marked_at_market(&data.chart, &grids, &display, &[key.to_string()], false)?;
        Ok((data, true))
    })
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
    pub buys: Vec<(At, Option<String>, f64, f64, i64)>,
    pub prices: Vec<(String, Vec<Option<f64>>)>,
    /// The asset behind each charted key, for its logo and colour: every key bought or priced.
    pub logo_assets: Vec<(String, i64)>,
    /// D-SELL-ONLY-CHART: Some(whether the cost is known) for a sell-only bot, whose series are Received and its cost.
    pub received: Option<bool>,
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
    let marks = buy_marks(s, marked)?;
    let mut page = plotted(s, marked, hide_balances, decimals, marks)?;

    // Allocatable#holding_assets, for every key bought or priced: by id, or for a holding known only by its
    // string, the asset the venue spells that way.
    let mut key_assets = HashMap::new();
    let mut ids = vec![];
    for (key, id) in &marked.key_assets {
        budget::charge(1, 0)?;
        key_assets.entry(key).or_insert(id);
        if let Some(id) = id { ids.push(*id); }
    }
    let existing: HashSet<_> = db::existing_assets(c, &ids)?.into_iter().collect();
    for key in charted_keys(&page)?.into_iter().flatten() {
        budget::charge(1, 0)?;
        let Some(asset_id) = key_assets.get(&key) else { continue };
        let id = match asset_id {
            Some(id) => existing.contains(id).then_some(*id),
            None => ticker_for_key(s, marked, &key).filter(|ticker| ticker.base_asset_exists).map(|ticker| ticker.base_asset_id),
        };
        if let Some(id) = id { page.logo_assets.push((key, id)); }
    }
    Ok(Some(page))
}

/// Every key bought or priced, once each, in the order the page meets them (marks first).
fn charted_keys(page: &Page) -> Result<Vec<Option<String>>, NumError> {
    let mut keys = vec![];
    let mut seen = HashSet::new();
    for key in page.buys.iter().map(|mark| mark.1.clone()).chain(page.prices.iter().map(|(key, _)| Some(key.clone()))) {
        budget::charge(1, 0)?;
        if seen.insert(key.clone()) { keys.push(key); }
    }
    Ok(keys)
}

/// A pair bot's chart (`bots/_chart` for Bots::DcaSingleAsset and Bots::Signal): rounded at its ticker's quote precision
/// (8 with none), its marks keyed by Bot::ChartSeries#chart_row_key, and the logos by Bot::ChartSeries#chart_logo_assets:
/// the venue's listings spelled as each key, the last of a spelling standing, an asset that is gone left out.
pub fn pair_page(c: &Connection, s: &Subject, marked: &Metrics, hide_balances: bool) -> Result<Option<Page>, FiguresError> {
    budget::within(|| {
        if marked.chart.labels.is_empty() { return Ok(None); }
        let ticker = s.tickers.first();
        let decimals = ticker.and_then(|t| t.quote_decimals).unwrap_or(8);
        let marks = pair_buy_marks(s, ticker.map(|t| t.base.as_str()))?;
        let mut page = plotted(s, marked, hide_balances, decimals, marks)?;
        let keys = charted_keys(&page)?;
        if let Some(exchange) = s.bot.exchange_id {
            for (base, id) in db::logo_listings(c, exchange, &keys)? { budget::charge(1, 0)?; page.logo_assets.push((base, id)); }
        }
        Ok(Some(page))
    })
}

/// What the page's chart shows of `marked`, rounded at `decimals`, with `marks` under it and no logos yet.
fn plotted(s: &Subject, marked: &Metrics, hide_balances: bool, decimals: i64, marks: Vec<BuyMark>) -> Result<Page, FiguresError> {
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
    Ok(Page {
        bot: s.bot.id, quote, decimals, labels: marked.chart.labels.clone(), series, pnl, assets, pnl_only: hide_balances,
        buys: marks.into_iter().map(|mark| (mark.at, mark.key, mark.amount.to_f(), mark.quote.to_f(), mark.fills)).collect(),
        prices, logo_assets: vec![], received: None,
    })
}

impl Page {
    /// The `data-bot--chart-<name>-value` attributes, each value as 2.x's tag helper writes it: a string as it
    /// is, anything else as JSON. Times are in `zone` with `digits` fractional digits (2.x: 3).
    pub fn attributes(&self, zone: &Tz, digits: usize) -> Vec<(&'static str, String)> {
        let time = |at: &At| J::Str(json::time(&at.utc().with_timezone(zone), digits));
        let floats = |serie: &Vec<f64>| J::arr(serie, |f| J::Float(*f));
        let optional = |serie: &Vec<Option<f64>>| J::arr(serie, |f| J::opt(f, |f| J::Float(*f)));
        let mut out = vec![
            ("bot", self.bot.to_string()),
            ("quote", self.quote.clone()),
            ("decimals", self.decimals.to_string()),
            ("labels", J::arr(&self.labels, time).write()),
            ("series", J::Arr(self.series.iter().map(|serie| J::arr(serie, J::num)).collect()).write()),
            // Received is no profit: with the cost unknown there is no P/L curve to send.
            ("pnl", if self.received == Some(false) { "[]".into() } else { floats(&self.pnl).write() }),
            ("assets", J::Obj(self.assets.iter().map(|(key, value, invested)| (key.clone(), J::Obj(vec![
                ("value".to_string(), optional(value)), ("invested".to_string(), floats(invested)),
            ]))).collect()).write()),
            ("pnl-only", self.pnl_only.to_string()),
            ("buys", J::arr(&self.buys, |(at, key, amount, quote, fills)| J::Arr(vec![time(at), J::opt(key, |key| J::Str(key.clone())), J::Float(*amount), J::Float(*quote), J::Int(*fills)])).write()),
            ("prices", J::Obj(self.prices.iter().map(|(key, serie)| (key.clone(), optional(serie))).collect()).write()),
            ("logo-assets", J::Obj(self.logo_assets.iter().map(|(key, id)| (key.clone(), J::Int(*id))).collect()).write()),
        ];
        if let Some(known) = self.received { out.push(("received", if known { "known" } else { "unknown" }.into())); }
        out
    }
}

/// D-CHART-DATA-REQUEST: what the bot chart's data request (`GET /bots/{id}/chart/data`) answers from. A decimal-free
/// copy of a `Page`, made by the background render beside the chart's shell (`figure::plot::render`) and kept with the
/// rendered figures of the same record as its JSON text, so the account's 8 MiB admission counts it (`figure::computed`)
/// and it lives and dies with that record. `rev` is the record's: the fill's time, which its shell carries too.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Series {
    pub rev: i64,
    pub quote: String,
    pub decimals: i64,
    /// Unix milliseconds, ascending, one point per millisecond: of two in one millisecond the later stands.
    pub t: Vec<i64>,
    pub value: Vec<f64>,
    pub invested: Vec<f64>,
    /// Per holding: its value at each point (None where the point kept its fill mark) and what it had cost.
    pub assets: Vec<(String, Vec<Option<f64>>, Vec<f64>)>,
    pub prices: Vec<(String, Vec<Option<f64>>)>,
    /// (ms, key, amount, quote, fills). Marks of one holding at one instant are one mark: the controller sums a holding's
    /// marks under one stack anyway, so the sum draws the same, and it bounds them at `MARK_BUCKETS + 1` per holding.
    pub buys: Vec<(i64, Option<String>, f64, f64, i64)>,
    /// key -> (image, colour).
    pub logos: Vec<(String, Option<String>, String)>,
}

/// The query of a data request: the view (Unix ms; None: the history's own end), the points the plot has room for, the
/// focused holding, the extras that are on, and (D-CHART-LIVE) the `closed` and `sum` of the answer in hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query { pub from: Option<i64>, pub to: Option<i64>, pub points: usize, pub asset: Option<String>, pub prices: bool, pub buys: bool, pub after: Option<(i64, String)> }
/// `points` when the query names none, and the range it may name.
pub const POINTS: usize = 300;
pub const POINTS_RANGE: std::ops::RangeInclusive<usize> = 2..=2000;
/// The numbers one answer's series carry at most: `points` is lowered to keep `series * (2 * points + 4)` under it.
pub const NUMBERS: usize = 100_000;

/// Rung `i` of the bucket ladder, in ms: 1 s, 10 s, 1 min, 5 min, 15 min, 30 min, 1 h, 4 h, 1 day, 1 week, then 2, 4, 8…
/// weeks.
fn rung(i: usize) -> i64 {
    const LADDER: [i64; 10] = [1, 10, 60, 300, 900, 1800, 3600, 14_400, 86_400, 604_800];
    let seconds = LADDER.get(i).copied().unwrap_or_else(|| 604_800_i64.saturating_mul(1_i64.checked_shl((i - 9) as u32).unwrap_or(i64::MAX)));
    seconds.saturating_mul(1000)
}
fn ceil_div(a: i64, b: i64) -> i64 { (a + b - 1).div_euclid(b) }
/// FNV-1a, 64 bits: `sum` only has to be the same for the same readings on every server, and not guessable is no need.
fn fnv(hash: &mut u64, bytes: &[u8]) { for byte in bytes { *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3); } }
/// The step a view `width` ms wide takes at `points`: the smallest rung whose aligned buckets the view can touch number at
/// most `points` (`ceil(width / step) + 1`), and the widths that take it, both ends included.
pub fn step(width: i64, points: usize) -> (i64, [i64; 2]) {
    let p = points as i64;
    let mut i = 0;
    loop {
        let step = rung(i);
        if ceil_div(width.max(0), step).saturating_add(1) <= p || step == i64::MAX {
            let lo = if i == 0 { 0 } else { (p - 1).saturating_mul(rung(i - 1)).saturating_add(1) };
            return (step, [lo, (p - 1).saturating_mul(step)]);
        }
        i += 1;
    }
}

impl Series {
    /// A `Page` as the data request reads it, with its logos, under the record's `rev`.
    pub fn of(page: &Page, logos: Vec<(String, Option<String>, String)>, rev: i64) -> Series {
        let ms = |at: &At| at.0.div_euclid(1_000_000);
        // The indices that stand: the last of each millisecond.
        let kept: Vec<usize> = (0..page.labels.len()).filter(|&i| page.labels.get(i + 1).is_none_or(|next| ms(next) != ms(&page.labels[i]))).collect();
        let pick = |serie: &[f64]| kept.iter().map(|&i| serie.get(i).copied().unwrap_or_default()).collect::<Vec<f64>>();
        let pick_opt = |serie: &[Option<f64>]| kept.iter().map(|&i| serie.get(i).copied().flatten()).collect::<Vec<Option<f64>>>();
        let [value, invested] = &page.series;
        let floats = |serie: &Vec<Num>| serie.iter().map(Num::to_f).collect::<Vec<f64>>();
        let mut buys: Vec<(i64, Option<String>, f64, f64, i64)> = vec![];
        let mut place: HashMap<(i64, Option<String>), usize> = HashMap::new();
        for (at, key, amount, quote, fills) in &page.buys {
            match place.get(&(ms(at), key.clone())) {
                Some(&i) => { let mark = &mut buys[i]; mark.2 += amount; mark.3 += quote; mark.4 += fills; }
                None => { place.insert((ms(at), key.clone()), buys.len()); buys.push((ms(at), key.clone(), *amount, *quote, *fills)); }
            }
        }
        Series {
            rev, quote: page.quote.clone(), decimals: page.decimals,
            t: kept.iter().map(|&i| ms(&page.labels[i])).collect(),
            value: pick(&floats(value)), invested: pick(&floats(invested)),
            assets: page.assets.iter().map(|(key, value, invested)| (key.clone(), pick_opt(value), pick(invested))).collect(),
            prices: page.prices.iter().map(|(key, serie)| (key.clone(), pick_opt(serie))).collect(),
            buys, logos,
        }
    }

    /// The answer to `query`: ONE selection of indices, every series read at it (D-CHART-THIN).
    pub fn window(&self, query: &Query) -> serde_json::Value {
        use serde_json::{json, Map, Value};
        let n = self.t.len();
        let asset = query.asset.as_ref().and_then(|key| self.assets.iter().find(|(k, ..)| k == key));
        let series = 3 + if asset.is_some() { 2 } else { 0 } + if query.prices { self.prices.len() } else { 0 };
        let points = query.points.min((NUMBERS / series).saturating_sub(4) / 2).max(*POINTS_RANGE.start());
        let (first, last) = (self.t.first().copied().unwrap_or_default(), self.t.last().copied().unwrap_or_default());
        let (from, to) = (query.from.unwrap_or(first), query.to.unwrap_or(last));
        let (step, spans) = step(to - from, points);
        let pad = (points / 2) as i64;
        let (lo, hi) = (from.div_euclid(step) - pad, to.div_euclid(step) + pad);
        // [start, end): the points in the padded view.
        let start = self.t.partition_point(|&t| t.div_euclid(step) < lo);
        let end = self.t.partition_point(|&t| t.div_euclid(step) <= hi);
        let marked = |i: usize| asset.is_none_or(|(_, value, _)| value[i].is_some());
        let asset_first = asset.and_then(|(_, value, _)| value.iter().position(Option::is_some));
        let mut picked: Vec<usize> = vec![];
        if start > 0 { picked.push(start - 1); }
        let mut i = start;
        while i < end {
            let bucket = self.t[i].div_euclid(step);
            let group_end = i + self.t[i..end].partition_point(|&t| t.div_euclid(step) == bucket);
            let chosen = (i..group_end).rev().find(|&j| marked(j)).unwrap_or(group_end - 1);
            picked.push(chosen);
            // The curve starts where the history starts, and ends where it ends, whatever the bucket prefers; a focused
            // holding's curve starts at its own first marked point.
            for edge in [Some(0), Some(n - 1), asset_first].into_iter().flatten() { if (i..group_end).contains(&edge) { picked.push(edge); } }
            i = group_end;
        }
        if end < n { picked.push(end); }
        picked.sort_unstable();
        picked.dedup();
        // D-CHART-LIVE: `sum` hashes every selected point before `closed` (the start of the latest selected point's bucket),
        // its time and each series' reading in this answer; an ask repeating it with `after` gets only the points from
        // `after` on when its own selection before `after` hashes the same. Returns how many points precede `upto`.
        let prices: &[(String, Vec<Option<f64>>)] = if query.prices { &self.prices } else { &[] };
        let sum = |upto: i64| -> (usize, String) {
            let mut hash = 0xcbf2_9ce4_8422_2325_u64;
            let reading = |hash: &mut u64, amount: Option<f64>| match amount { None => fnv(hash, &[0]), Some(amount) => { fnv(hash, &[1]); fnv(hash, &amount.to_bits().to_le_bytes()) } };
            let before = picked.partition_point(|&i| self.t[i] < upto);
            for &i in &picked[..before] {
                fnv(&mut hash, &self.t[i].to_le_bytes());
                for amount in [self.value[i], self.invested[i]] { reading(&mut hash, Some(amount)); }
                if let Some((_, value, invested)) = asset { reading(&mut hash, value[i]); reading(&mut hash, Some(invested[i])); }
                for (_, serie) in prices { reading(&mut hash, serie[i]); }
            }
            (before, format!("{hash:016x}"))
        };
        let closed = picked.last().map(|&i| self.t[i].div_euclid(step).saturating_mul(step)).filter(|&closed| picked.first().is_some_and(|&i| self.t[i] < closed));
        let skip = query.after.as_ref().and_then(|(after, asked)| { let (before, hash) = sum(*after); (&hash == asked).then_some(before) });
        let shown = &picked[skip.unwrap_or(0)..];
        let at = |serie: &[f64]| shown.iter().map(|&i| serie[i]).collect::<Vec<f64>>();
        let at_opt = |serie: &[Option<f64>]| shown.iter().map(|&i| serie[i]).collect::<Vec<Option<f64>>>();
        let (since, until) = (lo.saturating_mul(step), hi.saturating_add(1).saturating_mul(step));
        let mut out = json!({
            "rev": self.rev, "quote": self.quote, "decimals": self.decimals, "step": step / 1000, "spans": spans,
            // The padded view the selection covers, [since, until): the data in hand reach this far, and no further.
            "padded": [since, until], "closed": closed, "append": skip.is_some(),
            "extent": [first, last], "t": shown.iter().map(|&i| self.t[i]).collect::<Vec<i64>>(),
            "value": at(&self.value), "invested": at(&self.invested),
            "holdings": self.assets.iter().filter(|(_, value, _)| value.iter().any(Option::is_some)).map(|(key, ..)| key).collect::<Vec<_>>(),
            "priced": self.prices.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            "logos": Value::Object(self.logos.iter().map(|(key, image, color)| (key.clone(), json!({ "image": image, "color": color }))).collect::<Map<_, _>>()),
        });
        if !self.assets.is_empty() {
            // One ruler across the holdings, over the whole history (the controller's `#assetBounds`).
            let (mut value, mut low, mut high) = (0.0_f64, 0.0_f64, 0.0_f64);
            for (_, values, invested) in &self.assets {
                for (amount, cost) in values.iter().zip(invested) {
                    value = value.max(*cost);
                    let Some(amount) = amount else { continue };
                    value = value.max(*amount);
                    (low, high) = (low.min(amount - cost), high.max(amount - cost));
                }
            }
            out["bounds"] = json!({ "value": value, "low": low, "high": high });
        }
        if let Some((key, value, invested)) = asset { out["asset"] = json!({ "key": key, "value": at_opt(value), "invested": at(invested) }); }
        if query.prices { out["prices"] = Value::Object(self.prices.iter().map(|(key, serie)| (key.clone(), json!(at_opt(serie)))).collect()); }
        if let Some(closed) = closed { out["sum"] = json!(sum(closed).1); }
        if query.buys {
            // Always whole for the padded view, appended or not.
            out["buys"] = Value::Array(self.buys.iter().filter(|mark| since <= mark.0 && mark.0 < until).map(|(at, key, amount, quote, fills)| json!([at, key, amount, quote, fills])).collect());
        }
        out
    }
}

#[cfg(test)]
mod series_tests {
    use super::*;
    use serde_json::Value;

    fn series(t: &[i64]) -> Series {
        Series { rev: 7, quote: "USD".into(), decimals: 2, t: t.to_vec(), value: t.iter().map(|&t| t as f64).collect(), invested: vec![1.0; t.len()],
            assets: vec![], prices: vec![], buys: vec![], logos: vec![] }
    }
    fn view(from: i64, to: i64, points: usize) -> Query { Query { from: Some(from), to: Some(to), points, ..Query::default() } }
    fn whole(points: usize) -> Query { Query { points, ..Query::default() } }
    fn times(answer: &Value) -> Vec<i64> { answer["t"].as_array().unwrap().iter().map(|t| t.as_i64().unwrap()).collect() }

    #[test]
    fn of_each_bucket_its_last_point_at_its_own_time_and_the_first_point_of_history() {
        // 1 s buckets: {0, 999}, {1000, 1999}, {2000}.
        let answer = series(&[0, 999, 1000, 1999, 2000]).window(&whole(300));
        assert_eq!(answer["step"], 1);
        assert_eq!(times(&answer), [0, 999, 1999, 2000]);
        assert_eq!(answer["value"], serde_json::json!([0.0, 999.0, 1999.0, 2000.0]), "each point's own reading");
    }

    #[test]
    fn an_empty_bucket_selects_nothing() {
        assert_eq!(times(&series(&[0, 5000]).window(&whole(300))), [0, 5000]);
    }

    #[test]
    fn the_neighbour_before_and_after_the_padded_view() {
        // points 2: buckets of 1 s, padded by one either side: [99 s, 103 s).
        let answer = series(&[0, 50_000, 98_999, 100_500, 103_000, 150_000]).window(&view(100_000, 101_000, 2));
        assert_eq!(answer["step"], 1);
        assert_eq!(times(&answer), [98_999, 100_500, 103_000]);
    }

    #[test]
    fn a_view_inside_a_gap_answers_the_two_points_around_it() {
        assert_eq!(times(&series(&[0, 100_000]).window(&view(40_000, 41_000, 2))), [0, 100_000]);
    }

    #[test]
    fn a_view_before_after_or_across_the_history() {
        let s = series(&[100_000, 200_000]);
        assert_eq!(times(&s.window(&view(0, 1000, 2))), [100_000], "before: the first point");
        assert_eq!(times(&s.window(&view(300_000, 301_000, 2))), [200_000], "after: the last point");
        assert_eq!(times(&s.window(&view(0, 300_000, 300))), [100_000, 200_000], "across");
        assert_eq!(s.window(&whole(300))["extent"], serde_json::json!([100_000, 200_000]));
    }

    #[test]
    fn one_point() {
        let answer = series(&[5000]).window(&whole(300));
        assert_eq!((times(&answer), &answer["value"]), (vec![5000], &serde_json::json!([5000.0])));
    }

    #[test]
    fn two_fills_200_ms_apart_are_one_point_at_any_step_the_finest_bucket_being_a_second() {
        for points in [2, 300, 2000] {
            assert_eq!(times(&series(&[0, 10_000, 10_200]).window(&whole(points))), [0, 10_200], "points {points}");
        }
    }

    #[test]
    fn a_focused_holding_prefers_its_own_marked_point_and_every_series_shares_the_indices() {
        let mut s = series(&[0, 200, 400, 600, 1500]);
        s.assets = vec![("BTC".into(), vec![None, Some(2.0), Some(4.0), None, None], vec![0.0, 1.0, 1.0, 1.0, 1.0])];
        s.prices = vec![("BTC".into(), vec![Some(10.0), Some(20.0), Some(40.0), Some(60.0), Some(15.0)])];
        let asked = Query { asset: Some("BTC".into()), prices: true, ..whole(300) };
        let answer = s.window(&asked);
        // Bucket 0 holds 0..600: its last marked point (400), the history's first (0); bucket 1 its last (1500), which is
        // the history's last. The holding's curve starts at its own first marked point (200).
        assert_eq!(times(&answer), [0, 200, 400, 1500]);
        assert_eq!(answer["asset"]["value"], serde_json::json!([null, 2.0, 4.0, null]));
        assert_eq!(answer["asset"]["invested"], serde_json::json!([0.0, 1.0, 1.0, 1.0]));
        assert_eq!(answer["prices"]["BTC"], serde_json::json!([10.0, 20.0, 40.0, 15.0]));
        assert_eq!(answer["value"], serde_json::json!([0.0, 200.0, 400.0, 1500.0]));
        assert_eq!(times(&s.window(&whole(300))), [0, 600, 1500], "without it, the bucket's last");
        assert_eq!(answer["holdings"], serde_json::json!(["BTC"]));
        assert_eq!(answer["bounds"], serde_json::json!({ "value": 4.0, "low": 0.0, "high": 3.0 }));
        let unknown = s.window(&Query { asset: Some("ETH".into()), ..whole(300) });
        assert!(unknown.get("asset").is_none(), "an unknown holding is answered without it");
    }

    #[test]
    fn a_price_spike_on_a_point_its_bucket_does_not_select_is_absent() {
        // Named (D-CHART-THIN): of the bucket only its last point is drawn.
        let mut s = series(&[0, 300, 600, 900]);
        s.prices = vec![("BTC".into(), vec![Some(10.0), Some(10.0), Some(99.0), Some(11.0)])];
        let answer = s.window(&Query { prices: true, ..whole(300) });
        assert_eq!(answer["prices"]["BTC"], serde_json::json!([10.0, 11.0]));
    }

    #[test]
    fn marks_inside_the_padded_view_each_as_it_is() {
        let mut s = series(&[0, 50_000, 100_500, 150_000]);
        // The padded view is [99 s, 103 s); the view [100 s, 101 s]. Two of BTC either side of each edge of the view.
        s.buys = [98_999, 99_000, 99_999, 100_000, 101_000, 101_001, 102_999, 103_000].iter().map(|&at| (at, Some("BTC".into()), 1.0, 10.0, 1)).collect();
        let answer = s.window(&Query { buys: true, ..view(100_000, 101_000, 2) });
        let at: Vec<i64> = answer["buys"].as_array().unwrap().iter().map(|mark| mark[0].as_i64().unwrap()).collect();
        assert_eq!(at, [99_000, 99_999, 100_000, 101_000, 101_001, 102_999]);
        assert!(answer["buys"].as_array().unwrap().iter().all(|mark| mark[2] == 1.0 && mark[4] == 1), "unsummed");
        assert!(s.window(&whole(300)).get("buys").is_none(), "only when asked");
    }

    #[test]
    fn several_buys_and_points_inside_the_first_bucket_keep_the_first_point_and_every_buy() {
        let mut s = series(&[0, 100, 200]);
        s.buys = [0, 100, 200].iter().map(|&at| (at, Some("BTC".into()), 1.0, 10.0, 1)).collect();
        let answer = s.window(&Query { buys: true, ..whole(300) });
        assert_eq!(times(&answer), [0, 200]);
        assert_eq!(answer["buys"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn views_of_one_width_take_one_step_and_answer_at_most_2_points_plus_2() {
        let s = series(&(0..18_200).map(|i| i * 1000).collect::<Vec<_>>());
        let (a, b) = (s.window(&view(0, 17_999_000, 300)), s.window(&view(59_000, 18_058_000, 300)));
        assert_eq!(a["step"], b["step"]);
        assert_eq!(a["step"], 300, "ceil(17999 / 60) + 1 = 301 buckets of a minute: five minutes");
        for answer in [a, b] { assert!(times(&answer).len() <= 602, "{}", times(&answer).len()); }
    }

    #[test]
    fn a_ten_year_view_at_300_points_takes_two_week_buckets() {
        assert_eq!(step(3_650 * 86_400_000, 300).0, 2 * 604_800_000);
    }

    #[test]
    fn spans_are_exact_at_both_ends_wherever_the_view_is() {
        for points in [2, 7, 300, 2000] {
            for width in [0, 1, 999, 1000, 59_999, 600_000, 86_400_000, 3_650 * 86_400_000_i64] {
                let (at, [lo, hi]) = step(width, points);
                assert!(lo <= width && width <= hi, "{points} {width}: {lo}..{hi}");
                assert_eq!(step(lo, points).0, at);
                assert_eq!(step(hi, points).0, at);
                assert_ne!(step(hi + 1, points).0, at, "{points} {width}: past the top");
                if lo > 0 { assert_ne!(step(lo - 1, points).0, at, "{points} {width}: under the bottom"); }
            }
        }
    }

    #[test]
    fn the_number_budget_is_never_passed() {
        let mut s = series(&(0..20_000).map(|i| i * 1000).collect::<Vec<_>>());
        s.assets = vec![("A".into(), vec![Some(1.0); 20_000], vec![1.0; 20_000])];
        s.prices = (0..70).map(|k| (format!("P{k}"), vec![Some(1.0); 20_000])).collect();
        for points in [2, 300, 2000] {
            let answer = s.window(&Query { asset: Some("A".into()), prices: true, ..whole(points) });
            let n = times(&answer).len();
            assert!((3 + 2 + 70) * n <= NUMBERS, "points {points}: {n} points");
            assert!(n >= 2);
        }
    }

    /// D-CHART-LIVE: the next ask of an answer in hand, and the client's merge of what it gets.
    fn next(asked: &Query, held: &Value) -> Query { Query { after: Some((held["closed"].as_i64().unwrap(), held["sum"].as_str().unwrap().into())), ..asked.clone() } }
    fn merged(held: &Value, answer: &Value, after: i64) -> (Vec<i64>, Vec<f64>) {
        let keep = times(held).iter().filter(|&&t| t < after).count();
        let value = |answer: &Value| answer["value"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<f64>>();
        ([&times(held)[..keep], &times(answer)[..]].concat(), [&value(held)[..keep], &value(answer)[..]].concat())
    }

    #[test]
    fn a_publication_that_only_added_points_is_answered_by_them() {
        let asked = whole(300);
        let held = series(&(0..=10).map(|i| i * 1000).collect::<Vec<_>>()).window(&asked);
        assert_eq!((held["closed"].as_i64(), held["append"].as_bool()), (Some(10_000), Some(false)));
        let grown = series(&(0..=12).map(|i| i * 1000).collect::<Vec<_>>());
        let answer = grown.window(&next(&asked, &held));
        assert_eq!(answer["append"], true);
        assert_eq!(times(&answer), [10_000, 11_000, 12_000], "from `after` on");
        let whole = grown.window(&asked);
        assert_eq!(merged(&held, &answer, 10_000), (times(&whole), whole["value"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()));
        assert_eq!((answer["closed"].clone(), answer["sum"].clone()), (whole["closed"].clone(), whole["sum"].clone()), "the next ask's");
    }

    #[test]
    fn the_latest_point_updated_in_place_is_appended() {
        let asked = whole(300);
        let s = series(&[0, 1000, 2000]);
        let held = s.window(&asked);
        let mut moved = s.clone();
        moved.value[2] = 99.0;
        let answer = moved.window(&next(&asked, &held));
        assert_eq!((answer["append"].as_bool(), times(&answer)), (Some(true), vec![2000]));
        assert_eq!(answer["value"], serde_json::json!([99.0]));
    }

    #[test]
    fn anything_else_is_the_whole_answer() {
        let asked = whole(300);
        let mut s = series(&[0, 2000, 5000]);
        s.prices = vec![("BTC".into(), vec![Some(1.0), Some(2.0), Some(3.0)])];
        let held = s.window(&asked);
        let whole_answer = |answer: &Value, n: usize| { assert_eq!(answer["append"], false); assert_eq!(times(answer).len(), n); };
        let mut restated = s.clone();
        restated.value[1] = 7.0;
        whole_answer(&restated.window(&next(&asked, &held)), 3);
        // Past 299 s at 300 points the step moves from 1 s to 10 s.
        let mut longer = series(&[0, 2000, 5000, 400_000]);
        longer.prices = vec![("BTC".into(), vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)])];
        let answer = longer.window(&next(&asked, &held));
        assert_eq!(answer["step"], 10);
        whole_answer(&answer, 3);
        whole_answer(&s.window(&next(&Query { prices: true, ..asked.clone() }, &held)), 3);
        // 2000 was its bucket's last point; 2700 now is, before `closed`.
        let mut later = series(&[0, 2000, 2700, 5000]);
        later.prices = vec![("BTC".into(), vec![Some(1.0), Some(2.0), Some(2.0), Some(3.0)])];
        let answer = later.window(&next(&asked, &held));
        whole_answer(&answer, 3);
        assert_eq!(times(&answer), [0, 2700, 5000]);
        let wrong = s.window(&Query { after: Some((5000, "0".into())), ..asked.clone() });
        whole_answer(&wrong, 3);
    }

    #[test]
    fn no_selected_point_before_closed_has_no_closed_and_no_sum() {
        for t in [&[5000][..], &[0, 100, 200]] {
            let answer = series(t).window(&whole(300));
            assert!(answer["closed"].is_null(), "{t:?}");
            assert!(answer.get("sum").is_none(), "{t:?}");
        }
        let empty = series(&[]).window(&whole(300));
        assert!(empty["closed"].is_null());
    }

    #[test]
    fn padded_is_the_view_the_selection_covers() {
        // points 2: 1 s buckets padded by one either side.
        assert_eq!(series(&[0, 100_500]).window(&view(100_000, 101_000, 2))["padded"], serde_json::json!([99_000, 103_000]));
    }

    fn page(labels: &[i64], buys: Vec<(At, Option<String>, f64, f64, i64)>) -> Page {
        let at = |ms: i64| At(ms * 1_000_000);
        Page { bot: 1, quote: "USD".into(), decimals: 2, labels: labels.iter().map(|&ms| at(ms)).collect(),
            series: [labels.iter().enumerate().map(|(i, _)| Num::Float(i as f64 * 1.5)).collect(), labels.iter().map(|_| Num::Float(1.0)).collect()],
            pnl: vec![], assets: vec![("BTC".into(), labels.iter().enumerate().map(|(i, _)| (i % 3 != 0).then_some(i as f64)).collect(), vec![2.0; labels.len()])],
            pnl_only: false, buys, prices: vec![("BTC".into(), labels.iter().map(|&ms| Some(ms as f64 / 7.0)).collect())], logo_assets: vec![], received: None }
    }

    #[test]
    fn of_two_points_of_one_millisecond_the_later_stands() {
        let at = |nanos: i64| At(nanos);
        let mut p = page(&[0, 0, 5], vec![]);
        p.labels = vec![at(1_000_000), at(1_400_000), at(5_000_000)];
        let s = Series::of(&p, vec![], 1);
        assert_eq!(s.t, [1, 5]);
        assert_eq!(s.value, [1.5, 3.0]);
    }

    #[test]
    fn a_whole_history_answer_is_the_pages_own_points_with_its_readings() {
        let labels: Vec<i64> = (0..5_000).map(|i| i * 37_000 + i % 11).collect();
        let p = page(&labels, vec![]);
        let s = Series::of(&p, vec![], 1);
        let answer = s.window(&Query { asset: Some("BTC".into()), prices: true, ..whole(300) });
        let t = times(&answer);
        assert!(t.len() > 100 && t.len() < 600, "{}", t.len());
        for (k, ms) in t.iter().enumerate() {
            let i = labels.iter().position(|l| l == ms).expect("a Page point");
            assert_eq!(answer["value"][k], p.series[0][i].to_f());
            assert_eq!(answer["invested"][k], p.series[1][i].to_f());
            assert_eq!(answer["asset"]["value"][k], serde_json::json!(p.assets[0].1[i]));
            assert_eq!(answer["prices"]["BTC"][k], serde_json::json!(p.prices[0].1[i]));
        }
        assert_eq!((t[0], *t.last().unwrap()), (labels[0], *labels.last().unwrap()), "both ends of the history");
    }

    #[test]
    fn marks_of_one_instant_past_501_are_one_mark_inside_the_budget() {
        let at = At(9_000 * 1_000_000);
        let buys = (0..2_000).map(|_| (at, Some("BTC".into()), 0.5, 10.0, 1)).chain([(at, Some("ETH".into()), 1.0, 3.0, 2)]).collect();
        let s = Series::of(&page(&[0, 9_000], buys), vec![], 1);
        assert_eq!(s.buys, vec![(9_000, Some("BTC".into()), 1000.0, 20_000.0, 2_000), (9_000, Some("ETH".into()), 1.0, 3.0, 2)]);
        let answer = s.window(&Query { buys: true, ..whole(300) });
        assert!(5 * answer["buys"].as_array().unwrap().len() + 3 * times(&answer).len() <= NUMBERS);
    }
}
