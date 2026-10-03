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
use super::walk::{confirmed_exec_amounts, d, AssetSeries, Chart, Metrics, Row, Unpriced};
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

fn grid<'a>(grids: &'a Grids, key: &str) -> Option<&'a Marks> { grids.iter().find(|(k, _)| k == key).map(|(_, marks)| marks) }
fn set(grids: &mut Grids, key: &str, marks: Marks) {
    match grids.iter_mut().find(|(k, _)| k == key) { Some(entry) => entry.1 = marks, None => grids.push((key.to_string(), marks)) }
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
fn row_key(metrics: &Metrics, base: Option<&str>, asset_id: Option<i64>) -> String {
    let base = base.unwrap_or_default();
    let key = match asset_id {
        Some(id) => metrics.key_for(id),
        // The later of two holdings recorded under one string stands.
        None => metrics.key_strings.iter().rev()
            .find(|(key, strings)| metrics.key_assets.iter().any(|(k, id)| k == key && id.is_none()) && strings.iter().any(|s| s == base))
            .map(|(key, _)| key.as_str()),
    };
    key.unwrap_or(base).to_string()
}

/// #chart_buy_marks: the buys the walk counts, as executed, oldest first.
/// ponytail: Rails orders these by created_at alone, so two buys of one moment come as SQLite returns them; here
/// they come by id, which is what its index gives. Only the order of two marks of one instant could differ.
pub fn buy_marks(s: &Subject, metrics: &Metrics) -> Result<Vec<BuyMark>, FiguresError> {
    let mut marks = vec![];
    for order in s.orders.iter().filter(|order| order.buy && order.price.is_some()) {
        let (Some(amount), Some(quote)) = confirmed_exec_amounts(order)? else { continue };
        if amount.is_zero() || quote.is_zero() { continue; }
        marks.push(BuyMark { at: order.at, key: row_key(metrics, order.base.as_deref(), order.asset_id), amount, quote, fills: 1 });
    }
    Ok(thinned_marks(marks)?)
}

/// #chart_fill_marks_by_symbol: the price each fill marked its holding at. An order that executed nothing marks
/// nothing, nor does a sale whose proceeds were never reported; of two fills at one moment the later stands.
fn fill_marks(s: &Subject, metrics: &Metrics) -> Result<Grids, FiguresError> {
    let mut out: Grids = vec![];
    for order in &s.orders {
        let Some(price) = &order.price else { continue };
        let (executed, proceeds) = confirmed_exec_amounts(order)?;
        if d(&executed).is_zero() || d(&proceeds).is_zero() { continue; }
        if order.sell && !d(&order.quote_amount_exec).is_positive() { continue; }
        let key = row_key(metrics, order.base.as_deref(), order.asset_id);
        let at = match out.iter().position(|(k, _)| *k == key) { Some(at) => at, None => { out.push((key, vec![])); out.len() - 1 } };
        // The orders come oldest first, so a fill of the same moment can only be the last mark so far.
        match out[at].1.last_mut().filter(|(time, _)| *time == order.at) { Some(mark) => mark.1 = price.clone(), None => out[at].1.push((order.at, price.clone())) }
    }
    Ok(out)
}

/// #chart_backfilled_grids: what a holding's candles do not span is filled in from the bot's own fill prices. Only
/// outside the candles' span, and only on the candles' side of any split.
fn backfilled(mut grids: Grids, symbols: &[String], from: At, to: At, events: &[Event], s: &Subject, metrics: &Metrics) -> Result<Grids, FiguresError> {
    let mut fills: Option<Grids> = None;
    for symbol in symbols {
        let marks = grid(&grids, symbol).cloned().unwrap_or_default();
        if let (Some(first), Some(last)) = (marks.first(), marks.last()) { if first.0 <= from && last.0 >= to { continue; } }
        let fills = match &mut fills { Some(fills) => fills, none => none.insert(fill_marks(s, metrics)?) };
        let Some(mut extra) = grid(fills, symbol).cloned().filter(|extra| !extra.is_empty()) else { continue };
        if let (Some(first), Some(last)) = (marks.first().map(|m| m.0), marks.last().map(|m| m.0)) {
            let cuts = || events.iter().filter(|event| event.key == *symbol).map(|event| event.at);
            let floor = cuts().filter(|at| *at <= first).max();
            let ceiling = cuts().filter(|at| *at > last).min();
            extra.retain(|(time, _)| !((first <= *time && *time <= last) || floor.is_some_and(|floor| *time < floor) || ceiling.is_some_and(|ceiling| *time >= ceiling)));
        }
        let mut all = marks;
        all.extend(extra);
        all.sort_by_key(|mark| mark.0);
        set(&mut grids, symbol, all);
    }
    Ok(grids)
}

/// #chart_split_pinned_grids: the last price before a split, pinned one second before it, and the first price
/// after it, pinned at it, so no point is read off a line drawn between two share bases.
fn split_pinned(mut grids: Grids, events: &[Event]) -> Result<Grids, FiguresError> {
    for event in events {
        let Some(marks) = grid(&grids, &event.key).filter(|marks| !marks.is_empty()) else { continue };
        let (Some(before), Some(after)) = (marks.iter().rfind(|(time, _)| *time < event.at), marks.iter().find(|(time, _)| *time >= event.at)) else { continue };
        let second_before = event.at.plus_seconds(-1).ok_or_else(|| FiguresError::Data("a split at the edge of time".into()))?;
        let mut pinned = marks.clone();
        for pin in [(second_before, before.1.clone()), (event.at, after.1.clone())] {
            if !pinned.iter().any(|(time, _)| *time == pin.0) { pinned.push(pin); }
        }
        pinned.sort_by_key(|mark| mark.0);
        set(&mut grids, &event.key, pinned);
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
                let closed = time.plus_seconds(self.timeframe).is_some_and(|close| close <= self.now);
                if closed && seen.insert(time.0) { marks.push((time, open)); }
            }
            if marks.is_empty() { continue; }
            marks.sort_by_key(|mark| mark.0);
            if let (Some(price), Some(last)) = (self.metrics.live_prices.as_ref().and_then(|prices| prices.iter().find(|(k, _)| k == symbol)), self.metrics.chart.labels.last()) {
                marks.push((*last, price.1.to_d()?));
                marks.sort_by_key(|mark| mark.0);
            }
            grids.push((symbol.clone(), marks));
        }
        Ok(grids)
    }
}

/// #chart_marked_at_market. `priceable` are the holdings the bot can price at all; only they take part.
pub fn marked_at_market(chart: &Chart, grids: &Grids, display: &Grids, priceable: &[String]) -> Result<Chart, FiguresError> {
    let Some(first) = chart.labels.first() else { return Ok(chart.clone()) };
    // The walk's rows run parallel to its labels; a chart from anywhere else is refused here, not indexed past its end.
    if [chart.value.len(), chart.invested.len(), chart.extra.len(), chart.invested_by.len(), chart.cash.len()] != [chart.labels.len(); 5] {
        return Err(FiguresError::Data("a chart whose series are not of one length".into()));
    }
    let transaction_times: HashSet<i64> = chart.labels.iter().map(|label| label.0).collect();
    let mut axis: Vec<At> = chart.labels.clone();
    axis.extend(grids.iter().flat_map(|(_, marks)| marks.iter().map(|(time, _)| *time)).filter(|time| time >= first));
    axis.sort();
    axis.dedup();
    let slice = |row: &Row| -> Row {
        priceable.iter().filter_map(|key| row.iter().find(|(k, _)| k == key).cloned()).collect()
    };
    let mut marked = Chart { extra: chart.extra.clone(), invested_by: chart.invested_by.clone(), cash: chart.cash.clone(), ..Chart::default() };
    let mut prices: Vec<(String, Vec<Option<Num>>)> = display.iter().map(|(key, _)| (key.clone(), vec![])).collect();
    let mut split_rows: Vec<Option<Row>> = vec![];
    let mut basis_rows: Vec<Row> = vec![];
    let mut cursor = 0;
    for time in axis {
        budget::check()?;
        // Post-trade holdings: a transaction's own timestamp lands on that transaction, and where several share
        // one, on the last of them.
        while cursor + 1 < chart.labels.len() && chart.labels[cursor + 1] <= time { cursor += 1; }
        let mut row = cursor;
        let mut held = slice(&chart.extra[row]);
        if row > 0 && held.is_empty() { row -= 1; held = slice(&chart.extra[row]); }
        // What each holding is worth at market, or None unless every holding has a price here.
        let mut split: Option<Row> = Some(vec![]);
        for (key, amount) in &held {
            let amount = amount.to_d()?;
            let value = if amount.is_zero() { Some(Dec::zero()) } else { grid_price(grid(grids, key).map_or(&[][..], Vec::as_slice), time)?.map(|price| &amount * &price).transpose()? };
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
        for ((_, serie), (_, marks)) in prices.iter_mut().zip(display) { serie.push(grid_price(marks, time)?.map(Num::Dec)); }
        split_rows.push(split);
        basis_rows.push(slice(&chart.invested_by[row]));
    }
    // #chart_asset_series: the per-point splits turned into one series per holding.
    let mut symbols: Vec<&String> = vec![];
    for (key, _) in split_rows.iter().flatten().flatten() { if !symbols.contains(&key) { symbols.push(key); } }
    let of = |row: &Row, key: &String| row.iter().find(|(k, _)| k == key).map_or(Num::Int(0), |(_, value)| value.clone());
    marked.assets = Some(symbols.into_iter().map(|key| (key.clone(), AssetSeries {
        value: split_rows.iter().map(|split| split.as_ref().map(|row| of(row, key))).collect(),
        invested: basis_rows.iter().map(|row| of(row, key)).collect(),
    })).collect());
    marked.prices = Some(prices);
    Ok(marked)
}

/// Measurable#metrics_with_current_prices_and_candles: `live`'s figures with the chart marked at market. With no
/// candles for any holding, the fill marks the walk drew are the chart.
pub fn marked(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
    budget::within(|| at_market(c, s, live, market, now))
}

fn at_market(c: &Connection, s: &Subject, live: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
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
    let events = splits::events(c, s.bot.user_id, &s.orders, &data.holdings(), now)?;
    let grids = split_pinned(backfilled(grids, &symbols, first, last, &events, s, &data)?, &events)?;

    // The price overlay is read off each restating holding's history as its venue reads it today, one basis end
    // to end; everything else stays on the valuation grids.
    let restating: Vec<String> = symbols.iter().filter(|key| ticker_for_key(s, &data, key).is_some_and(|ticker| s.restated_candles(ticker))).cloned().collect();
    let mut display = grids.clone();
    if !restating.is_empty() { for (key, marks) in window.candle_grids(&restating, true)? { set(&mut display, &key, marks); } }

    let priceable: Vec<String> = symbols.iter().filter(|key| ticker_for_key(s, &data, key).is_some()).cloned().collect();
    // What Rails leaves out in silence is named beside the chart: a holding with no ticker today is no part of any
    // point's value, though what was paid for it stays in what went in. `live` names such a holding only while it
    // is held; one sold out since is named here.
    for key in symbols.iter().filter(|key| !priceable.contains(key)) {
        let held_once = data.chart.extra.iter().any(|row| row.iter().any(|(k, amount)| k == key && amount.is_positive()));
        if !held_once { continue; }
        let asset_id = data.key_assets.iter().find(|(k, _)| k == key).and_then(|(_, id)| *id);
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
        let value = serie.value.iter().map(|amount| amount.as_ref().map(|amount| amount.round(decimals).map(|n| n.to_f())).transpose()).collect::<Result<Vec<_>, _>>()?;
        let invested = serie.invested.iter().map(|amount| Num::Dec(amount.to_d()?).round(decimals).map(|n| n.to_f())).collect::<Result<Vec<_>, _>>()?;
        assets.push((key.clone(), value, invested));
    }
    let prices: Vec<(String, Vec<Option<f64>>)> = marked.chart.prices.iter().flatten()
        .map(|(key, serie)| (key.clone(), serie.iter().map(|price| price.as_ref().map(Num::to_f)).collect())).collect();
    let marks = buy_marks(s, marked)?;

    // Allocatable#holding_assets, for every key bought or priced: by id, or for a holding known only by its
    // string, the asset the venue spells that way.
    let mut keys: Vec<&String> = vec![];
    for key in marks.iter().map(|mark| &mark.key).chain(prices.iter().map(|(key, _)| key)) { if !keys.contains(&key) { keys.push(key); } }
    let existing = db::existing_assets(c, &marked.key_assets.iter().filter_map(|(_, id)| *id).collect::<Vec<_>>())?;
    let logo_assets = keys.into_iter().filter_map(|key| {
        let (_, asset_id) = marked.key_assets.iter().find(|(k, _)| k == key)?;
        match asset_id {
            Some(id) => existing.contains(id).then_some((key.clone(), *id)),
            None => ticker_for_key(s, marked, key).filter(|ticker| ticker.base_asset_exists).map(|ticker| (key.clone(), ticker.base_asset_id)),
        }
    }).collect();

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
