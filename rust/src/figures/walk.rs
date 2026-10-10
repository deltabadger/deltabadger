//! Bot::Composition::Measurable#metrics: one pass over every order the bot ever submitted, oldest first, folding
//! each fill into the ledger (books), the tax lots and the chart, with corporate actions folded in between as
//! events of their own. The figures here are valued at the price each asset last traded at; `live` marks them at
//! the market and `chart` re-marks the curve.
use super::at::At;
use super::books::{Books, Fill, Ledger};
use super::budget;
use super::db::{self, Order, Subject};
use super::dec::Dec;
use super::json::J;
use super::keys::{self, Identity};
use super::lots::{self, Lot, Lots};
use super::num::Num;
use super::splits::{self, Event, Holding};
use super::FiguresError;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// One point's reading per holding: `{ key => number }` in the ledger's order.
pub type Row = Vec<(String, Num)>;

/// The chart as the walk draws it: one point per fill and per restatement. `prices` and `assets` are added by
/// `chart::marked`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chart {
    pub labels: Vec<At>,
    /// series[0]: the portfolio's value.
    pub value: Vec<Num>,
    /// series[1]: what was put in.
    pub invested: Vec<Num>,
    /// What each holding amounted to after each point.
    pub extra: Vec<Row>,
    /// What each holding had cost after each point.
    pub invested_by: Vec<Row>,
    /// Proceeds realised and not yet spent, after each point.
    pub cash: Vec<Num>,
    pub prices: Option<Vec<(String, Vec<Option<Num>>)>>,
    pub assets: Option<Vec<(String, AssetSeries)>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetSeries { pub value: Vec<Option<Num>>, pub invested: Vec<Num> }

#[derive(Clone, Debug, PartialEq)]
pub struct Breakdown { pub amount: Num, pub quote_invested: Num, pub tax_basis: Num, pub tax_units: Num, pub tax_cost_unknown: bool }

#[derive(Clone, Debug, PartialEq)]
pub struct AssetValue {
    pub amount: Num, pub quote_invested: Num, pub current_value: Num, pub current_price: Num, pub avg_price: Num,
    pub pnl_percentage: Num, pub tax_basis: Num, pub harvestable: bool,
}

/// A held asset the live pass left out of the portfolio's value, and why. Rails leaves it out without a word
/// (`measurable.rb:241-245`); this is returned beside the figures, and is no part of their JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct Unpriced { pub key: String, pub asset_id: Option<i64>, pub reason: Why }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The venue lists nothing for it.
    NoTicker,
    /// The venue lists it, but not as an available, trading ticker in the bot's quote currency.
    Delisted,
    /// It has a ticker, and the venue's answer has no price for it.
    NoPrice,
}

impl Why {
    pub fn as_str(self) -> &'static str { match self { Why::NoTicker => "no_ticker", Why::Delisted => "delisted", Why::NoPrice => "no_price" } }
}

/// What only a bot with submitted orders has. Rails adds these keys as the walk goes, so their order in its hash
/// (and in JSON) depends on the history: `external_sales` and `restated_at` appear in the order they were first set.
#[derive(Clone, Debug, PartialEq)]
pub struct Walked {
    /// `{ asset id => [keys] }`: holdings recorded without an asset under one of that asset's names.
    pub shadowed_by: Vec<(i64, Vec<String>)>,
    /// A sale of more than the ledger held.
    pub external_sales: bool,
    /// The last restatement that moved a position.
    pub restated_at: Option<At>,
    pub restated_before_external_sales: bool,
    pub realised_cash: Num,
    pub asset_lots: Vec<(String, Lots)>,
    pub tax_pnl_by_transaction: Vec<(i64, Dec)>,
    /// true: the sale lost on a lot; false: it did not; None: unknown.
    pub loss_lot_by_transaction: Vec<(i64, Option<bool>)>,
}

/// Rails' metrics hash. `to_json` writes it key for key as Rails' `to_json` does.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub chart: Chart,
    pub total_quote_amount_invested: Num,
    pub total_amount_value_in_quote: Num,
    pub rebalance_cash: Num,
    pub estimated_proceeds: Num,
    pub realised_pnl: Num,
    pub pnl: Option<Num>,
    pub asset_breakdown: Vec<(String, Breakdown)>,
    pub asset_values: Vec<(String, AssetValue)>,
    /// key => the asset behind it, None for a holding known only by its symbol string.
    pub key_assets: Vec<(String, Option<i64>)>,
    /// key => the strings its rows were recorded under.
    pub key_strings: Vec<(String, Vec<String>)>,
    pub num_assets: i64,
    pub walked: Option<Walked>,
    /// Set by `live`: the figures fell back to the last fill prices.
    pub prices_stale: bool,
    /// Set by `live`: the mark of every holding that priced.
    pub live_prices: Option<Vec<(String, Num)>>,
    /// Set by `live`, and not written by `to_json`: the held assets it left out of the value. Empty when the
    /// figures are the walk's own (no chart point yet, or stale).
    pub unpriced: Vec<Unpriced>,
    /// Set by `chart::marked`, and not written by `to_json`: the holdings the marked chart leaves out of every
    /// point's value, though the bot held them at some point, because it has no ticker to price them on today.
    /// Empty when the chart is the walk's own (no candles for any holding).
    pub chart_omitted: Vec<Unpriced>,
}

impl Metrics {
    /// #initialize_metrics_data.
    pub fn empty() -> Metrics {
        Metrics {
            chart: Chart::default(), total_quote_amount_invested: Num::Int(0), total_amount_value_in_quote: Num::Int(0), rebalance_cash: Num::Int(0),
            estimated_proceeds: Num::Int(0), realised_pnl: Num::Int(0), pnl: None, asset_breakdown: vec![], asset_values: vec![], key_assets: vec![],
            key_strings: vec![], num_assets: 0, walked: None, prices_stale: false, live_prices: None, unpriced: vec![], chart_omitted: vec![],
        }
    }

    /// Allocatable#key_for: the key the holding of this asset is known by.
    pub fn key_for(&self, asset_id: i64) -> Option<&str> {
        self.key_assets.iter().find(|(_, id)| *id == Some(asset_id)).map(|(key, _)| key.as_str())
    }

    /// Measurable#split_holdings.
    pub fn holdings(&self) -> Result<Vec<Holding>, FiguresError> {
        let mut strings = HashMap::new();
        for (key, names) in &self.key_strings { budget::charge(1, 0)?; strings.entry(key).or_insert(names); }
        let mut out = vec![];
        for (key, asset_id) in &self.key_assets {
            let names = strings.get(key).copied();
            budget::charge(1 + names.map_or(0, |names| names.len()) as u64, 0)?;
            out.push(Holding { key: key.clone(), asset_id: *asset_id, strings: names.cloned().unwrap_or_default() });
        }
        Ok(out)
    }

    /// The hash as Rails' `to_json` writes it; times in UTC with `digits` fractional digits (Rails: 3).
    pub fn to_json(&self, digits: usize) -> J {
        let amounts = |row: &Row| J::obj(row, J::num);
        let time = |at: &At| J::Str(super::json::time_utc(&at.utc(), digits));
        let mut chart = vec![
            ("labels".to_string(), J::arr(&self.chart.labels, time)),
            ("series".to_string(), J::Arr(vec![J::arr(&self.chart.value, J::num), J::arr(&self.chart.invested, J::num)])),
            ("extra_series".to_string(), J::arr(&self.chart.extra, amounts)),
            ("invested_series".to_string(), J::arr(&self.chart.invested_by, amounts)),
            ("cash_series".to_string(), J::arr(&self.chart.cash, J::num)),
        ];
        if let Some(prices) = &self.chart.prices {
            chart.push(("prices".to_string(), J::obj(prices, |serie| J::arr(serie, |price| J::opt(price, J::num)))));
        }
        if let Some(assets) = &self.chart.assets {
            chart.push(("assets".to_string(), J::obj(assets, |serie| J::Obj(vec![
                ("value".to_string(), J::arr(&serie.value, |value| J::opt(value, J::num))),
                ("invested".to_string(), J::arr(&serie.invested, J::num)),
            ]))));
        }
        let pair = |name: &str, value: J| (name.to_string(), value);
        let mut out = vec![
            pair("chart", J::Obj(chart)),
            pair("total_quote_amount_invested", J::num(&self.total_quote_amount_invested)),
            pair("total_amount_value_in_quote", J::num(&self.total_amount_value_in_quote)),
            pair("rebalance_cash", J::num(&self.rebalance_cash)),
            pair("estimated_proceeds", J::num(&self.estimated_proceeds)),
            pair("realised_pnl", J::num(&self.realised_pnl)),
            pair("pnl", J::opt(&self.pnl, J::num)),
            pair("asset_breakdown", J::obj(&self.asset_breakdown, |b| J::Obj(vec![
                pair("amount", J::num(&b.amount)), pair("quote_invested", J::num(&b.quote_invested)), pair("tax_basis", J::num(&b.tax_basis)),
                pair("tax_units", J::num(&b.tax_units)), pair("tax_cost_unknown", J::Bool(b.tax_cost_unknown)),
            ]))),
            pair("asset_values", J::obj(&self.asset_values, |v| J::Obj(vec![
                pair("amount", J::num(&v.amount)), pair("quote_invested", J::num(&v.quote_invested)), pair("current_value", J::num(&v.current_value)),
                pair("current_price", J::num(&v.current_price)), pair("avg_price", J::num(&v.avg_price)), pair("pnl_percentage", J::num(&v.pnl_percentage)),
                pair("tax_basis", J::num(&v.tax_basis)), pair("harvestable", J::Bool(v.harvestable)),
            ]))),
            pair("key_assets", J::obj(&self.key_assets, |id| J::opt(id, |id| J::Int(*id)))),
            pair("key_strings", J::obj(&self.key_strings, |strings| J::arr(strings, |s| J::Str(s.clone())))),
            pair("num_assets", J::Int(self.num_assets)),
        ];
        if let Some(w) = &self.walked {
            out.push(pair("shadowed_by", J::Obj(w.shadowed_by.iter().map(|(id, keys)| (id.to_string(), J::arr(keys, |k| J::Str(k.clone())))).collect())));
            let restated = w.restated_at.as_ref().map(|at| pair("restated_at", time(at)));
            let sales = w.external_sales.then(|| pair("external_sales", J::Bool(true)));
            if w.restated_before_external_sales { out.extend(restated.into_iter().chain(sales)); } else { out.extend(sales.into_iter().chain(restated)); }
            out.push(pair("realised_cash", J::num(&w.realised_cash)));
            out.push(pair("asset_lots", J::obj(&w.asset_lots, |list| J::Arr(list.iter().map(|lot| J::Obj(vec![
                pair("amount", J::Str(lot.amount.to_s_f())), pair("cost", J::opt(&lot.cost, |cost| J::Str(cost.to_s_f()))),
            ])).collect()))));
            out.push(pair("tax_pnl_by_transaction", J::Obj(w.tax_pnl_by_transaction.iter().map(|(id, pnl)| (id.to_string(), J::Str(pnl.to_s_f()))).collect())));
            out.push(pair("loss_lot_by_transaction", J::Obj(w.loss_lot_by_transaction.iter().map(|(id, loss)| (id.to_string(), J::opt(loss, |l| J::Bool(*l)))).collect())));
        }
        if self.prices_stale { out.push(pair("prices_stale", J::Bool(true))); }
        if let Some(prices) = &self.live_prices { out.push(pair("live_prices", J::obj(prices, J::num))); }
        J::Obj(out)
    }
}

/// `x.to_d` on a column: nil is zero.
pub fn d(value: &Option<Dec>) -> Dec { value.clone().unwrap_or_else(Dec::zero) }

fn identity(o: &Order) -> Identity {
    match o.asset_id { Some(id) => Identity::Asset(id), None => Identity::Text(o.base.clone().unwrap_or_default()) }
}

/// #calculate_pnl: `(to - from).to_f / from`, a Float zero when nothing went in.
pub fn pnl(from: &Num, to: &Num) -> Result<Num, FiguresError> {
    if from.is_zero() { return Ok(Num::Float(0.0)); }
    Ok(Num::Float(to.sub(from)?.to_f()).div(from)?)
}

struct Walk {
    ledger: Ledger,
    books: Books,
    lots: Vec<(String, Lots)>,
    /// The price each holding last traded at.
    prices: Vec<(String, Num)>,
    chart: Chart,
    estimated_proceeds: Num,
    external_sales: bool,
    restated_at: Option<At>,
    restated_before_external_sales: bool,
}

impl Walk {
    /// `lots[key]`: the Hash's default proc makes an empty list for a key it has not seen, and the list stays.
    fn lots(&mut self, key: &str) -> &mut Lots {
        let at = match self.lots.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => { self.lots.push((key.to_string(), Lots::new())); self.lots.len() - 1 }
        };
        &mut self.lots[at].1
    }

    fn point(&mut self, at: At) -> Result<(), FiguresError> {
        let cash = self.books.uninvested_cash()?;
        self.chart.labels.push(at);
        self.chart.value.push(self.ledger.value(&self.prices)?.add(&cash)?);
        self.chart.invested.push(self.books.contributed.clone());
        self.chart.extra.push(self.ledger.0.iter().map(|(key, entry)| (key.clone(), entry.amount.clone())).collect());
        self.chart.invested_by.push(self.ledger.0.iter().map(|(key, entry)| (key.clone(), entry.invested.clone())).collect());
        self.chart.cash.push(cash);
        Ok(())
    }

    /// #apply_due_splits: folds in every restatement due at or before `until` (all of them when None) and returns
    /// what is left. A split multiplies the position and the lots, divides the last traded price, and moves no money.
    fn apply_due_splits(&mut self, pending: Vec<Event>, until: Option<At>) -> Result<Vec<Event>, FiguresError> {
        if pending.is_empty() { return Ok(pending); }
        budget::charge(pending.len() as u64, 0)?;
        let (due, rest): (Vec<Event>, Vec<Event>) = pending.into_iter().partition(|event| until.is_none_or(|until| event.at <= until));
        for event in due {
            budget::charge(1, 0)?;
            if let Some((_, list)) = self.lots.iter_mut().find(|(key, _)| *key == event.key) { lots::split(list, &event.factor)?; }
            let factor = Num::Dec(event.factor.clone());
            let Some((_, entry)) = self.ledger.0.iter_mut().find(|(key, _)| *key == event.key) else { continue };
            if entry.amount.to_d()?.is_zero() { continue; }
            entry.amount = Num::Dec(entry.amount.to_d()?).mul(&factor)?;
            if let Some((_, price)) = self.prices.iter_mut().find(|(key, _)| *key == event.key) { *price = price.div(&factor)?; }
            if self.restated_at.is_none() && !self.external_sales { self.restated_before_external_sales = true; }
            self.restated_at = Some(self.restated_at.map_or(event.at, |at| at.max(event.at)));
            self.point(event.at)?;
        }
        Ok(rest)
    }
}

/// The asset each listing of the venue stands for, by the names a row without an asset could have been recorded
/// under (#unresolved_shadows): `{ asset id => [keys of the holdings recorded under one of its names] }`.
fn unresolved_shadows(c: &Connection, s: &Subject, keys: &[(Identity, String)]) -> Result<Vec<(i64, Vec<String>)>, FiguresError> {
    let mut unresolved = HashMap::new();
    let mut ids = vec![];
    for (identity, key) in keys {
        budget::charge(1, 0)?;
        match identity {
            Identity::Text(text) => { unresolved.insert(text.to_uppercase(), key); }
            Identity::Asset(id) => ids.push(*id),
        }
    }
    let Some(exchange_id) = s.bot.exchange_id else { return Ok(vec![]) };
    if unresolved.is_empty() || ids.is_empty() { return Ok(vec![]); }
    let mut out: Vec<(i64, Vec<String>)> = vec![];
    let mut places = HashMap::new();
    let mut seen = HashSet::new();
    for (base, asset_id, symbol) in db::listings(c, exchange_id, &ids)? {
        budget::charge(1, 0)?;
        for name in [Some(db::base_spelling(&base)), symbol.as_deref()].into_iter().flatten().filter(|name| !name.trim().is_empty()) {
            budget::charge(1, 0)?;
            let Some(key) = unresolved.get(&name.to_uppercase()) else { continue };
            if !seen.insert((asset_id, (*key).clone())) { continue; }
            let at = *places.entry(asset_id).or_insert_with(|| { out.push((asset_id, vec![])); out.len() - 1 });
            out[at].1.push((*key).clone());
        }
    }
    Ok(out)
}

/// Lots of the same asset recorded without it may be the ones FIFO consumed: the sale's verdict is unknown.
fn shadowed(walk: &Walk, shadowed_by: &[(i64, Vec<String>)], order: &Order) -> bool {
    let Some((_, shadows)) = order.asset_id.and_then(|id| shadowed_by.iter().find(|(asset, _)| *asset == id)) else { return false };
    // Rails sums the lots' units for every sale. No lot is ever below zero (one opens on a positive fill, shrinks to what is
    // left of it, and is multiplied by a positive factor), so the sum is positive exactly when a lot is.
    shadows.iter().any(|key| walk.lots.iter().find(|(k, _)| k == key).is_some_and(|(_, list)| list.iter().any(|lot| lot.amount.is_positive())))
}

/// Bot::Composition::Measurable#metrics, uncached, within one figure's budget.
pub fn metrics(c: &Connection, s: &Subject, now: At) -> Result<Metrics, FiguresError> {
    budget::within(|| walked(c, s, now))
}

fn walked(c: &Connection, s: &Subject, now: At) -> Result<Metrics, FiguresError> {
    let mut data = Metrics::empty();
    if s.orders.is_empty() { return Ok(data); }

    // One holding per asset, whatever symbol its rows were recorded under; a row recorded before orders stored
    // their asset is its own holding, by its string.
    let mut identities = vec![];
    let mut seen = HashSet::new();
    let mut fills:Vec<Option<super::fill::Fill>>=Vec::with_capacity(s.orders.len());
    for order in &s.orders {
        budget::charge(1, 0)?;
        fills.push(super::fill::parse(order)?);
        let id = identity(order);
        if seen.insert(id.clone()) { identities.push(id); }
    }
    let mut asset_ids = vec![];
    for id in &identities { budget::charge(1, 0)?; if let Identity::Asset(id) = id { asset_ids.push(*id); } }
    let mut names = HashMap::new();
    for (id, symbol, name) in db::asset_names(c, &asset_ids)? { budget::charge(1, 0)?; names.insert(id, (symbol, name)); }
    let mut candidates = vec![];
    for identity in identities {
        budget::charge(1, 0)?;
        let candidate = match &identity {
            Identity::Asset(id) => names.get(id).map_or_else(|| id.to_string(), |(symbol, name)| keys::candidate(*id, symbol.as_deref(), name.as_deref())),
            Identity::Text(text) => text.clone(),
        };
        candidates.push((identity, candidate));
    }
    let keys = keys::call(&candidates)?;
    let mut key_lookup = HashMap::new();
    for (identity, key) in &keys {
        budget::charge(1, 0)?;
        key_lookup.insert(identity.clone(), key.clone());
        data.key_assets.push((key.clone(), match identity { Identity::Asset(id) => Some(*id), Identity::Text(_) => None }));
    }
    let key_of = |order: &Order| -> String { key_lookup[&identity(order)].clone() };
    let mut string_places = HashMap::new();
    let mut strings_seen = HashSet::new();
    for order in &s.orders {
        budget::charge(1, 0)?;
        let (key, text) = (key_of(order), order.base.clone().unwrap_or_default()); // allow-swallow: an Option; a legacy row without a symbol has an empty spelling
        if !strings_seen.insert((key.clone(), text.clone())) { continue; }
        let at = *string_places.entry(key.clone()).or_insert_with(|| {
            data.key_strings.push((key, vec![])); data.key_strings.len() - 1
        });
        data.key_strings[at].1.push(text);
    }
    let shadowed_by = unresolved_shadows(c, s, &keys)?;

    let mut walk = Walk {
        ledger: Ledger::default(), books: Books::default(), lots: vec![], prices: vec![], chart: Chart::default(),
        estimated_proceeds: Num::Int(0), external_sales: false, restated_at: None, restated_before_external_sales: false,
    };
    let mut tax_pnl: Vec<(i64, Dec)> = vec![];
    let mut loss_lot: Vec<(i64, Option<bool>)> = vec![];
    // Corporate actions are events in this walk like any fill: applied before the first order at or after them.
    let mut pending = splits::events(c, s.bot.user_id, &s.orders, &data.holdings()?, now)?;

    for (order,fill) in s.orders.iter().zip(fills) {
        budget::charge(1, 0)?;
        let base = key_of(order);
        pending = walk.apply_due_splits(pending, Some(order.at))?;
        // A non-REGULAR sell Rails does not price (MQ6): its tax view on the raw proceeds, then gate 1. A priced one is
        // the fill below.
        if let Some(sell) = super::fill::special_sell(order)?.filter(|sell| sell.fill().is_none()) {
            let held = match walk.ledger.get(&base) { Some(entry) => entry.amount.to_d()?, None => Dec::zero() };
            if sell.executed > held { walk.external_sales = true; }
            if sell.executed.is_positive() {
                let list: &Lots = walk.lots(&base);
                let mut verdict = match &sell.reported {
                    Some(proceeds) => { tax_pnl.push((order.id, (proceeds - &lots::cost_of(list, &sell.executed)?)?)); lots::loss_in(list, &sell.executed, proceeds)? }
                    None => if list.is_empty() { Some(false) } else { None }, // #unpriced_sale_verdict
                };
                if shadowed(&walk, &shadowed_by, order) { verdict = None; }
                loss_lot.push((order.id, verdict));
                lots::consume(walk.lots(&base), &sell.executed)?;
            }
            if sell.unpriced() {
                let released = walk.books.unpriced_liquidation(&mut walk.ledger, &base, &sell.executed)?;
                walk.estimated_proceeds = walk.estimated_proceeds.add(&released)?;
                walk.point(order.at)?; // at the marks that already stand
            }
            continue;
        }
        let Some(fill)=fill else{continue};
        let price=fill.unit_price()?;
        let executed=fill.quantity;
        let value=fill.value;

        // A sale of more than the ledger held: units that reached the venue some other way.
        let held = match walk.ledger.get(&base) { Some(entry) => entry.amount.to_d()?, None => Dec::zero() };
        if order.sell && executed > held { walk.external_sales = true; }

        // Tax lots and performance consume the same normalized value.
        if executed.is_positive() {
            if order.sell {
                let list: &Lots = walk.lots(&base); // read where it stands: a copy per sale would be the whole history each time
                tax_pnl.push((order.id, (&value - &lots::cost_of(list, &executed)?)?));
                let mut verdict=lots::loss_in(list,&executed,&value)?;
                if shadowed(&walk, &shadowed_by, order) { verdict = None; }
                loss_lot.push((order.id, verdict));
                lots::consume(walk.lots(&base), &executed)?;
            } else {
                walk.lots(&base).push_back(Lot { amount: executed.clone(), cost: Some(value.clone()) });
            }
        }

        walk.books.apply(&mut walk.ledger, Fill::of(order.sell, &order.kind), &base, &Num::Dec(executed), &Num::Dec(value))?;
        match walk.prices.iter_mut().find(|(key, _)| *key == base) {
            Some(entry) => entry.1 = Num::Dec(price.clone()),
            None => walk.prices.push((base.clone(), Num::Dec(price.clone()))),
        }
        walk.point(order.at)?;
    }
    // The ordinary case: a split lands and the bot has not traded since.
    walk.apply_due_splits(pending, None)?;

    data.total_quote_amount_invested = walk.books.contributed.clone();
    data.rebalance_cash = walk.books.uninvested_cash()?;
    data.realised_pnl = walk.books.realised_pnl.clone();
    data.total_amount_value_in_quote = walk.ledger.value(&walk.prices)?.add(&data.rebalance_cash)?;
    data.pnl = Some(pnl(&data.total_quote_amount_invested, &data.total_amount_value_in_quote)?);
    data.estimated_proceeds = walk.estimated_proceeds.clone();
    for (key, entry) in walk.ledger.0.clone() {
        let list: &Lots = walk.lots(&key);
        data.asset_breakdown.push((key, Breakdown {
            amount: entry.amount, quote_invested: entry.invested, tax_basis: lots::basis(list)?, tax_units: lots::units(list)?,
            tax_cost_unknown: lots::unknown_cost(list),
        }));
    }
    data.num_assets = walk.ledger.0.iter().filter(|(_, entry)| entry.amount.is_positive()).count() as i64;
    data.chart = walk.chart;
    data.walked = Some(Walked {
        shadowed_by, external_sales: walk.external_sales, restated_at: walk.restated_at,
        restated_before_external_sales: walk.restated_before_external_sales, realised_cash: walk.books.realised_cash,
        asset_lots: walk.lots, tax_pnl_by_transaction: tax_pnl, loss_lot_by_transaction: loss_lot,
    });
    Ok(data)
}
