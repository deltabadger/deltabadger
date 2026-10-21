//! Bots::DcaSingleAsset::Measurable, which Bots::Signal includes: a pair bot's walk over its orders (`metrics`), the
//! live pass (`live`) and the chart marked at market (`marked_at`), its three cache layers. Each is computed as Rails
//! computes it, apart from the named divergences D1 and D2 (`fill::pair_row`). The figures ride in `walk::Metrics`, so
//! the totals, the chart page and every reader keep one implementation; `Pair::to_json` writes Rails' own hash.
use super::at::At;
use super::db::{self, Bot, Kind, Subject};
use super::json::J;
use super::market::{Failure, MarketData};
use super::num::Num;
use super::splits::{self, Event, Holding, QUARANTINE_SECONDS};
use super::walk::Metrics;
use super::{budget, chart, fill, live, FiguresError};
use rusqlite::Connection;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Pair {
    pub subject: Subject,
    /// The figures: `chart.extra` holds the base held after each point (under `split_key`), `chart.cash` the proceeds
    /// realized by then; `invested_by` is empty.
    pub metrics: Metrics,
    /// total_base_amount.
    pub amount: Num,
    /// average_buy_price: over the buys, none without one.
    pub average: Option<Num>,
    /// total_realized_proceeds.
    pub proceeds: Num,
    /// The last restatement that moved the position.
    pub restated_at: Option<At>,
    /// Restatable#split_holdings' one key: the ticker's base, else the first distinct base the rows were recorded under.
    pub split_key: Option<String>,
    holdings: Vec<Holding>,
    events: Vec<Event>,
}

fn bad() -> FiguresError { FiguresError::Data("Invalid pair settings".into()) }

/// Whether bots of this type are pair bots, whose figures this module computes.
pub fn is_pair(kind: &str) -> bool { matches!(kind, "Bots::DcaSingleAsset" | "Bots::Signal") }

pub fn load(c: &Connection, id: i64) -> Result<Subject, FiguresError> {
    let (user, exchange, settings): (i64, i64, String) = c.query_row("SELECT user_id,exchange_id,settings FROM bots WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let v: Value = serde_json::from_str(&settings).map_err(|_| bad())?;
    let quote = v["quote_asset_id"].as_i64();
    let base = v["base_asset_id"].as_i64();
    let kind: String = c.query_row("SELECT type FROM exchanges WHERE id=?1", [exchange], |r| r.get(0))?;
    // Only the catalogue shape is shared with a basket; this module owns the pair's walk.
    let bot = Bot { id, user_id: user, exchange_id: Some(exchange), kind: Kind::Basket, exchange_type: Some(kind), quote_asset_id: quote, base_asset_ids: base.into_iter().collect() };
    let tickers = db::pair_ticker(c, &bot)?;
    Ok(Subject { orders: db::orders(c, id)?, tickers, quote: db::asset_symbol(c, quote)?, bot })
}

/// #calculate_pnl.
fn pnl(invested: &Num, value: &Num) -> Result<Num, FiguresError> {
    Ok(if invested.is_zero() { Num::Float(0.0) } else { Num::Float(value.sub(invested)?.to_f()).div(invested)? })
}

/// `transactions.submitted.distinct.pluck(:base).compact_blank`, as Rails asks SQLite for it (the first row is the
/// split key of a pair with no ticker, and an unordered DISTINCT answers in the order the same query always does).
fn spellings(c: &Connection, bot_id: i64) -> Result<Vec<String>, FiguresError> {
    let mut statement = c.prepare("SELECT DISTINCT \"transactions\".\"base\" FROM \"transactions\" WHERE \"transactions\".\"bot_id\" = ?1 AND \"transactions\".\"status\" = ?2")?;
    let rows = statement.query_map(rusqlite::params![bot_id, crate::enums::TxStatus::Submitted as i64], |r| r.get::<_, Option<String>>(0))?;
    let mut out = vec![];
    for base in rows { budget::charge(1, 0)?; if let Some(base) = base?.filter(|b| !b.trim().is_empty()) { out.push(base); } }
    Ok(out)
}

/// The walk's running state (#initialize_totals_data).
struct Totals { net_base: Num, invested: Num, proceeds: Num, value: Num, buys: Vec<(Num, Num)>, last_price: Option<Num> }

impl Pair {
    fn point(&mut self, at: At, t: &Totals) {
        let key = self.split_key.clone().unwrap_or_default();
        let chart = &mut self.metrics.chart;
        chart.labels.push(at);
        chart.invested.push(t.invested.clone());
        chart.value.push(t.value.clone());
        chart.extra.push(vec![(key, t.net_base.clone())]);
        chart.cash.push(t.proceeds.clone());
        chart.invested_by.push(vec![]);
    }

    /// #apply_due_splits for one event: the position, the buys behind the average and the last price restated; a point
    /// only where something was held.
    fn split(&mut self, e: &Event, t: &mut Totals) -> Result<(), FiguresError> {
        let factor = Num::Dec(e.factor.clone());
        let held = Num::Dec(t.net_base.to_d()?);
        t.net_base = held.mul(&factor)?;
        for (price, amount) in &mut t.buys {
            budget::charge(1, 0)?;
            *amount = Num::Dec(amount.to_d()?).mul(&factor)?;
            *price = Num::Dec(price.to_d()?).div(&factor)?;
        }
        if let Some(price) = &mut t.last_price { *price = Num::Dec(price.to_d()?).div(&factor)?; }
        if held.is_zero() { return Ok(()); }
        self.restated_at = Some(self.restated_at.map_or(e.at, |at| at.max(e.at)));
        let last = Num::Dec(t.last_price.as_ref().map_or(Ok(super::dec::Dec::zero()), Num::to_d)?);
        t.value = t.proceeds.add(&t.net_base.mul(&last)?)?;
        self.point(e.at, t);
        Ok(())
    }

    /// Whether the market's price can be multiplied by this position: Restatable#restated_prices_untrusted?.
    fn untrusted(&self, c: &Connection, now: At) -> Result<bool, FiguresError> {
        if splits::unresolved(c, self.subject.bot.user_id, &self.subject.orders, &self.holdings, now)? { return Ok(true); }
        Ok(self.restated_at.is_some_and(|at| now.plus_seconds(-QUARANTINE_SECONDS).is_none_or(|cutoff| at > cutoff)))
    }

    /// The hash as Rails' `to_json` writes it, times in UTC with `digits` fractional digits.
    pub fn to_json(&self, digits: usize) -> J {
        let time = |at: &At| J::Str(super::json::time_utc(&at.utc(), digits));
        let m = &self.metrics;
        let held = J::Arr(m.chart.extra.iter().map(|row| row.first().map_or(J::Null, |(_, amount)| J::num(amount))).collect());
        let mut chart = vec![
            ("labels".to_string(), J::arr(&m.chart.labels, time)),
            ("series".to_string(), J::Arr(vec![J::arr(&m.chart.value, J::num), J::arr(&m.chart.invested, J::num)])),
            ("extra_series".to_string(), J::Arr(vec![held, J::arr(&m.chart.cash, J::num)])),
        ];
        if let Some(prices) = &m.chart.prices {
            chart.push(("prices".to_string(), J::obj(prices, |serie| J::arr(serie, |price| J::opt(price, J::num)))));
        }
        let pair = |name: &str, value: J| (name.to_string(), value);
        let mut out = vec![
            pair("chart", J::Obj(chart)),
            pair("total_base_amount", J::num(&self.amount)),
            pair("total_quote_amount_invested", J::num(&m.total_quote_amount_invested)),
            pair("total_realized_proceeds", J::num(&self.proceeds)),
            pair("total_amount_value_in_quote", J::num(&m.total_amount_value_in_quote)),
            pair("pnl", J::opt(&m.pnl, J::num)),
            pair("average_buy_price", J::opt(&self.average, J::num)),
        ];
        if let Some(at) = &self.restated_at { out.push(pair("restated_at", time(at))); }
        if m.prices_stale { out.push(pair("prices_stale", J::Bool(true))); }
        if let Some(prices) = &m.live_prices { out.push(pair("live_prices", J::obj(prices, J::num))); }
        J::Obj(out)
    }
}

/// #metrics: every submitted order, oldest first, read by `fill::pair_row`, with the restatements due before each.
pub fn metrics(c: &Connection, id: i64, now: At) -> Result<Pair, FiguresError> {
    budget::within(|| {
        let subject = load(c, id)?;
        let mut p = Pair { subject, metrics: Metrics::empty(), amount: Num::Int(0), average: None, proceeds: Num::Int(0), restated_at: None,
                           split_key: None, holdings: vec![], events: vec![] };
        if p.subject.orders.is_empty() { return Ok(p); }
        let strings = spellings(c, id)?;
        p.split_key = p.subject.tickers.first().map(|t| t.base.clone()).or_else(|| strings.first().cloned());
        p.holdings = vec![Holding { key: p.split_key.clone().unwrap_or_default(), asset_id: p.subject.bot.base_asset_ids.first().copied(), strings }];
        p.events = splits::events(c, p.subject.bot.user_id, &p.subject.orders, &p.holdings, now)?;
        let orders = std::mem::take(&mut p.subject.orders);
        let events = p.events.clone();
        let mut t = Totals { net_base: Num::Int(0), invested: Num::Int(0), proceeds: Num::Int(0), value: Num::Int(0), buys: vec![], last_price: None };
        let mut pending = events.iter().peekable();
        for order in &orders {
            // Before the order: a split sharing its instant is applied first.
            while let Some(e) = pending.next_if(|e| e.at <= order.at) { p.split(e, &mut t)?; }
            budget::charge(1, 0)?;
            let Some(fill) = fill::pair_row(order)? else { continue };
            let price = Num::Dec(fill::pair_price(order, &fill)?);
            let (amount, quote) = (Num::Dec(fill.quantity), Num::Dec(fill.value));
            if order.sell {
                // Sold beyond what the bot holds: the excess is bought at its own sale price, P/L-neutral.
                let excess = Num::max2(amount.sub(&t.net_base)?, Num::Int(0))?;
                t.proceeds = t.proceeds.add(&quote)?;
                t.net_base = Num::max2(t.net_base.sub(&amount)?, Num::Int(0))?;
                if excess.is_positive() { t.invested = t.invested.add(&quote.mul(&excess)?.div(&amount)?)?; }
            } else {
                t.invested = t.invested.add(&quote)?;
                t.net_base = t.net_base.add(&amount)?;
                t.buys.push((price.clone(), amount));
            }
            t.value = t.proceeds.add(&t.net_base.mul(&price)?)?;
            t.last_price = Some(price);
            p.point(order.at, &t);
        }
        for e in pending { p.split(e, &mut t)?; }
        p.subject.orders = orders;
        p.amount = t.net_base;
        p.proceeds = t.proceeds;
        p.metrics.total_quote_amount_invested = t.invested;
        p.metrics.total_amount_value_in_quote = t.value;
        p.metrics.pnl = Some(pnl(&p.metrics.total_quote_amount_invested, &p.metrics.total_amount_value_in_quote)?);
        // Utilities::Math.weighted_average over the buys.
        if !t.buys.is_empty() {
            let (mut cost, mut amount) = (Num::Int(0), Num::Int(0));
            for (price, qty) in &t.buys { budget::charge(1, 0)?; cost = cost.add(&price.mul(qty)?)?; amount = amount.add(qty)?; }
            p.average = Some(cost.div(&Num::Float(amount.to_f()))?);
        }
        Ok(p)
    })
}

fn stale(mut p: Pair) -> Pair { p.metrics.prices_stale = true; p }

/// #metrics_with_current_prices: the walk marked at its ticker's price, with one more point at `now`, and whether Rails
/// writes it to its cache. The venue is asked first, before any check; with no point yet or no ticker the walk stands,
/// unflagged and unwritten; a restatement the market may not have priced, a failed read, or no price leave it stale.
pub fn live(c: &Connection, walked: &Pair, market: &dyn MarketData, now: At) -> Result<(Pair, bool), FiguresError> {
    budget::within(|| {
        let mut p = walked.clone();
        let Some(ticker) = p.subject.tickers.first().cloned() else { return Ok((p, false)) };
        if p.metrics.chart.labels.is_empty() { return Ok((p, false)); }
        let prices = match market.prices(&live::venue(&p.subject)?, std::slice::from_ref(&ticker.ticker)) {
            Ok(prices) => Some(prices),
            Err(Failure::Failed(_)) => None,
            Err(Failure::Raised(error)) => return Err(FiguresError::Raised(error)),
            Err(Failure::NotComputed(reason)) => return Err(FiguresError::NotComputed(reason)),
        };
        if p.untrusted(c, now)? { return Ok((stale(p), false)); }
        let Some(price) = prices.and_then(|prices| prices.into_iter().find(|(code, _)| *code == ticker.ticker)) else { return Ok((stale(p), false)) };
        let price = Num::Dec(price.1?);
        let value = p.proceeds.add(&p.amount.mul(&price)?)?;
        p.metrics.pnl = Some(pnl(&p.metrics.total_quote_amount_invested, &value)?);
        p.metrics.total_amount_value_in_quote = value.clone();
        let chart = &mut p.metrics.chart;
        chart.value.push(value);
        chart.invested.push(p.metrics.total_quote_amount_invested.clone());
        chart.labels.push(now);
        chart.extra.push(vec![(ticker.base.clone(), p.amount.clone())]);
        chart.cash.push(p.proceeds.clone());
        chart.invested_by.push(vec![]);
        p.metrics.live_prices = Some(vec![(ticker.base.clone(), price)]);
        Ok((p, true))
    })
}

/// #metrics_with_current_prices_and_candles over `live` (cached or not, stale or not), and whether Rails writes it to
/// its cache: only when there was a candle grid. A raised valuation fetch is raised.
pub fn marked_at(live: &Pair, market: &dyn MarketData, now: At) -> Result<(Pair, bool), FiguresError> {
    let mut p = live.clone();
    let Some(ticker) = p.subject.tickers.first() else { return Ok((p, false)) };
    if p.metrics.chart.labels.is_empty() { return Ok((p, false)); }
    let (marked, gridded) = chart::pair_marked(&p.subject, &p.metrics, &ticker.base, ticker, &p.events, market, now)?;
    p.metrics = marked;
    Ok((p, gridded))
}
