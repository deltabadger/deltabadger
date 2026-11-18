//! D-SELL-ONLY-*: a bot whose every order is a sell (it holds what it sells from outside) states what it sold, what that
//! brought in, and, when the account's history knows what the sold coins cost, the realised P/L. The cost is read from
//! the bot sales the tracker walk publishes beside its ledger entry (`tracker::cache::bot_sales`); figures never walk.
use super::{at::At, budget, db::{Order, Subject}, dec::Dec, fill, num::Num, FiguresError};
use crate::tracker::cache::{self, Sales};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq)]
pub struct SellOnly {
    /// What it sold, when that is one asset: (amount, symbol).
    pub sold: Option<(Dec, String)>,
    pub received: Dec,
    /// (at, received, cost in the quote) per order with a fill, in order.
    pub sales: Vec<(At, Dec, Option<Dec>)>,
    /// Σ cost; None unless D-SELL-ONLY-KNOWN-COST holds for the whole bot and the cost is positive.
    pub cost: Option<Dec>,
}
impl SellOnly {
    /// D-SELL-ONLY-PNL-IS-RECEIVED-MINUS-COST: Received, as the bot shows it, less what the sold coins cost.
    pub fn pnl(&self) -> Result<Option<Dec>, FiguresError> {
        self.cost.as_ref().map(|cost| Ok((&self.received - cost)?)).transpose()
    }
}

/// None for a bot that bought or has no orders. `pair`: read as the pair's walk reads its rows (`fill::pair_row`).
pub fn sell_only(s: &Subject, pair: bool, sales: &Sales) -> Result<Option<SellOnly>, FiguresError> {
    of(&s.orders, s.quote.as_deref(), pair, sales)
}

fn of(orders: &[Order], quote: Option<&str>, pair: bool, sales: &Sales) -> Result<Option<SellOnly>, FiguresError> {
    if orders.is_empty() || orders.iter().any(|order| !order.sell) { return Ok(None); }
    let by_order = match sales { Sales::Warm { by_order, .. } => Some(by_order), Sales::Cold => None };
    let (mut amount, mut received, mut bases, mut out) = (Dec::zero(), Dec::zero(), HashSet::new(), vec![]);
    let (mut cost, mut known) = (Dec::zero(), by_order.is_some());
    for order in orders {
        let Some(filled) = (if pair { fill::pair_row(order)? } else { fill::parse(order)? }) else {
            // Units that left with no fill, as the bot's own reader reads it (an executed sale without proceeds): their
            // cost cannot be set against anything.
            if fill::special_sell(order)?.is_some_and(|sale| sale.executed.is_positive()) { known = false; }
            continue;
        };
        // D-SELL-ONLY-KNOWN-COST 3–5: a known record of every unit of the fill, in the bot's quote.
        let sale = by_order.and_then(|by| by.get(&order.id))
            .filter(|sale| sale.known && sale.amount == filled.quantity && quote.is_some_and(|q| q == sale.quote))
            .map(|sale| sale.cost.clone());
        match &sale { Some(c) => cost = (&cost + c)?, None => known = false }
        amount = (&amount + &filled.quantity)?;
        received = (&received + &filled.value)?;
        if let Some(base) = &order.base { bases.insert(base.clone()); }
        out.push((order.at, filled.value, sale));
    }
    let sold = (bases.len() == 1).then(|| (amount, bases.into_iter().next().unwrap_or_default()));
    // A cost of nothing is no cost to state a P/L against: decided here, for the box, the tile and the chart.
    let cost = (known && !out.is_empty() && cost.is_positive()).then_some(cost);
    Ok(Some(SellOnly { sold, received, sales: out, cost }))
}

/// D-SELL-ONLY-CHART, over `labels` (ascending): value = Received up to each label; invested = the cost up to it when the
/// bot's cost is known, else zero.
pub fn series(sell_only: &SellOnly, labels: &[At]) -> Result<(Vec<Num>, Vec<Num>), FiguresError> {
    budget::charge((labels.len() + sell_only.sales.len()) as u64, 0)?;
    let known = sell_only.cost.is_some();
    let (mut value, mut invested) = (Vec::with_capacity(labels.len()), Vec::with_capacity(labels.len()));
    let (mut received, mut cost, mut next) = (Dec::zero(), Dec::zero(), 0);
    for label in labels {
        while let Some((_, got, paid)) = sell_only.sales.get(next).filter(|sale| sale.0 <= *label) {
            received = (&received + got)?;
            if let (true, Some(paid)) = (known, paid) { cost = (&cost + paid)?; }
            next += 1;
        }
        value.push(Num::Dec(received.clone()));
        invested.push(Num::Dec(cost.clone()));
    }
    Ok((value, invested))
}

/// Whether the account has a bot with submitted orders, all sells: the statuses and side `fill::orders` reads. One
/// indexed query; NOT EXISTS stops at a bot's first buy.
pub fn any_sell_only(c: &Connection, user_id: i64) -> Result<bool, FiguresError> {
    use crate::enums::{BotStatus, TxSide, TxStatus};
    Ok(c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM bots b WHERE b.user_id=?1 AND b.status!={} \
        AND EXISTS(SELECT 1 FROM transactions t WHERE t.bot_id=b.id AND t.status={submitted}) \
        AND NOT EXISTS(SELECT 1 FROM transactions t WHERE t.bot_id=b.id AND t.status={submitted} AND t.side IS NOT {}))",
        BotStatus::Deleted as i64, TxSide::Sell as i64, submitted = TxStatus::Submitted as i64), [user_id], |r| r.get(0))?)
}

/// The bot sales the figures read: None for an account with no sell-only bot (nothing read), else the published row,
/// Cold when it cannot be read for any reason but the database's.
pub fn sales(c: &Connection, user_id: i64, now: DateTime<Utc>) -> Result<Option<Sales>, FiguresError> {
    if !any_sell_only(c, user_id)? { return Ok(None); }
    match cache::bot_sales(c, user_id, now) {
        Err(FiguresError::Sqlite(e)) => Err(FiguresError::Sqlite(e)),
        Err(_) => Ok(Some(Sales::Cold)),
        Ok(sales) => Ok(Some(sales)),
    }
}

/// What figures rendered now would be held to: None without a sell-only bot, else the sales' stamp.
pub fn dependency(c: &Connection, user_id: i64, now: DateTime<Utc>) -> Result<Option<u64>, FiguresError> {
    Ok(sales(c, user_id, now)?.map(|sales| sales.stamp()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::walk::BotSale;
    use std::collections::HashMap;

    fn d(s: &str) -> Dec { Dec::strict(s).unwrap() }
    fn order(id: i64, amount: &str, value: &str, kind: &str) -> Order {
        Order { id, at: At(id), exchange_id: Some(1), raw: fill::Raw::new(Some(d("1")), Some(d(amount)), Some(d(amount)), Some(d(value))),
                base: Some("BTC".into()), asset_id: Some(4), sell: true, buy: false, closed: true, kind: kind.into(), abandoned_request: None }
    }
    fn sale(order: i64, amount: &str, quote: &str, cost: &str, known: bool) -> BotSale {
        BotSale { order, at: At(order), amount: d(amount), quote: quote.into(), cost: d(cost), known }
    }
    fn warm(sales: Vec<BotSale>) -> Sales { Sales::Warm { by_order: sales.into_iter().map(|s| (s.order, s)).collect::<HashMap<_, _>>(), expires: Utc::now() } }
    fn cost(orders: &[Order], sales: &Sales) -> Option<Dec> { of(orders, Some("USDC"), false, sales).unwrap().unwrap().cost }

    #[test]
    fn known_sales_state_their_cost_and_pnl() {
        let orders = [order(1, "0.002", "5", "REGULAR"), order(2, "0.001", "2.6", "REGULAR")];
        let s = of(&orders, Some("USDC"), false, &warm(vec![sale(1, "0.002", "USDC", "4", true), sale(2, "0.001", "USDC", "2", true)])).unwrap().unwrap();
        assert_eq!((s.cost, s.received.clone()), (Some(d("6")), d("7.6")));
        assert_eq!(s.sold, Some((d("0.003"), "BTC".into())));
        assert_eq!(s.sales.iter().map(|(_, r, c)| (r.clone(), c.clone())).collect::<Vec<_>>(), [(d("5"), Some(d("4"))), (d("2.6"), Some(d("2")))]);
        assert_eq!(SellOnly { cost: Some(d("6")), ..s }.pnl().unwrap(), Some(d("1.6")));
    }

    #[test]
    fn any_doubt_withholds_the_cost() {
        let orders = [order(1, "0.002", "5", "REGULAR")];
        assert_eq!(cost(&orders, &Sales::Cold), None, "cold");
        assert_eq!(cost(&orders, &warm(vec![])), None, "no record");
        assert_eq!(cost(&orders, &warm(vec![sale(1, "0.002", "USDC", "4", false)])), None, "not known");
        assert_eq!(cost(&orders, &warm(vec![sale(1, "0.0019", "USDC", "4", true)])), None, "the ledger has part of the fill");
        assert_eq!(cost(&orders, &warm(vec![sale(1, "0.002", "EUR", "4", true)])), None, "another quote");
        assert_eq!(cost(&orders, &warm(vec![sale(1, "0.002", "USDC", "4", true)])), Some(d("4")));
        assert_eq!(of(&orders, Some("USDC"), false, &warm(vec![])).unwrap().unwrap().pnl().unwrap(), None);
        // Coins that cost nothing: no cost to state a P/L against, so none (box, tile and chart alike).
        assert_eq!(cost(&orders, &warm(vec![sale(1, "0.002", "USDC", "0", true)])), None, "a known cost of zero");
    }

    #[test]
    fn an_executed_sale_without_proceeds_withholds_the_bots_cost() {
        let mut liquidation = order(2, "0.5", "0", "LIQUIDATION");
        liquidation.raw = fill::Raw::new(Some(d("1")), Some(d("0.5")), Some(d("0.5")), None);
        let orders = [order(1, "0.002", "5", "REGULAR"), liquidation];
        let sales = warm(vec![sale(1, "0.002", "USDC", "4", true)]);
        assert_eq!(cost(&orders, &sales), None);
        assert_eq!(cost(&orders[..1], &sales), Some(d("4")));
    }

    #[test]
    fn a_pair_reads_a_special_sell_by_its_own_fill() {
        // A pair values an unpriced liquidation from its proceeds (`pair_row`); the special-sell gate is not the pair's.
        let mut liquidation = order(1, "1", "150", "LIQUIDATION");
        liquidation.raw = fill::Raw::new(None, Some(d("1")), Some(d("1")), Some(d("150")));
        let sales = warm(vec![sale(1, "1", "USDC", "100", true)]);
        assert_eq!(of(std::slice::from_ref(&liquidation), Some("USDC"), true, &sales).unwrap().unwrap().cost, Some(d("100")));
        assert_eq!(of(&[liquidation], Some("USDC"), false, &sales).unwrap().unwrap().cost, None);
    }

    #[test]
    fn the_chart_is_received_and_cost_up_to_each_label() {
        let orders = [order(10, "0.002", "5", "REGULAR"), order(20, "0.001", "2.6", "REGULAR")];
        let labels = [At(5), At(10), At(15), At(20), At(30)];
        let nums = |serie: Vec<Num>| serie.iter().map(|n| n.to_d().unwrap()).collect::<Vec<_>>();
        let known = of(&orders, Some("USDC"), false, &warm(vec![sale(10, "0.002", "USDC", "4", true), sale(20, "0.001", "USDC", "2", true)])).unwrap().unwrap();
        let (value, invested) = series(&known, &labels).unwrap();
        assert_eq!(nums(value), [d("0"), d("5"), d("5"), d("7.6"), d("7.6")]);
        assert_eq!(nums(invested), [d("0"), d("4"), d("4"), d("6"), d("6")]);
        let unknown = of(&orders, Some("USDC"), false, &Sales::Cold).unwrap().unwrap();
        let (value, invested) = series(&unknown, &labels).unwrap();
        assert_eq!(nums(value), [d("0"), d("5"), d("5"), d("7.6"), d("7.6")]);
        assert!(nums(invested).iter().all(Dec::is_zero), "no cost while it is not known");
    }

    #[test]
    fn a_bot_that_bought_or_never_ordered_is_not_sell_only() {
        let mut buy = order(1, "1", "1", "REGULAR");
        buy.sell = false; buy.buy = true;
        assert_eq!(of(&[buy, order(2, "1", "1", "REGULAR")], Some("USDC"), false, &Sales::Cold).unwrap(), None);
        assert_eq!(of(&[], Some("USDC"), false, &Sales::Cold).unwrap(), None);
    }
}
