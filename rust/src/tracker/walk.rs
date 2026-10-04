//! Tracker::Ledger's walk (app/models/tracker/ledger.rb), for the rows `rows::load` accepts: the rows priced
//! (Tax::PriceService#enrich), opened with what must have been held, walked through FIFO lots with the tracker's
//! extensions (Tracker::Ledger::Engine over Tax::Methods::Fifo), and read for money in through the cash book
//! (Tracker::CashBook). One venue: the located and the account-wide walks are the same walk, so one serves both
//! the figures and the wash-sale arming.
//!
//! What this walk states is what the snapshots, the figures and the arming read: positions, money in (and each row's
//! term of it), the cash and the basis it carried, whether the figures are complete, and the loss sales. Round trips,
//! fees, what was received and what was realised are the pages' (Plan D5).
use super::prices::{Halt, PriceBook};
use super::rows::{Kind, Stored};
use super::{cash, fiat, stable};
use crate::figures::{at::At, dec::Dec, FiguresError};
use chrono::NaiveDate;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// An enriched row: what Tax::PriceService#enrich hands the engines, and the walk's own marks.
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub base: String,
    pub amount: Dec,
    pub quote: Option<String>,
    pub quote_amount: Option<Dec>,
    pub fiat_value: Dec,
    pub at: At,
    pub price_missing: bool,
    pub linked: bool,
    pub transfer_fee: Option<Dec>,
    pub per_share: Option<Dec>,
    /// An opening balance the walk booked (`open_with_what_must_have_been_held`), not a stored row.
    pub opening: bool,
}

/// One term of money in, as the chart reads it day by day (Tracker::Ledger::Term).
#[derive(Clone, Debug)]
pub struct Term { pub at: At, pub amount: Dec, pub complete: bool, pub opens: Option<(String, Dec)> }

/// A position the lots hold (Tracker::Ledger::Position, without its dates).
#[derive(Clone, Debug)]
pub struct Position { pub symbol: String, pub quantity: Dec, pub cost: Dec, pub avg_cost: Dec, pub estimated: bool, pub unpriced: Dec }

/// What the snapshots, the figures and the arming read of Tracker::Ledger::Summary.
#[derive(Clone, Debug)]
pub struct Summary {
    pub positions: Vec<Position>,
    pub total_invested: Dec,
    /// Per currency, in its units: every currency the book touched, a zero included, in the order it was first touched.
    pub cash: Vec<(String, Dec)>,
    /// Per currency, the dollars it carried in; zeros left out.
    pub cash_basis: Vec<(String, Dec)>,
    pub incomplete: bool,
    /// Per symbol, the day of its latest loss sale inside the wash-sale horizon, in the order first sold. The whole
    /// account's only.
    pub loss_sales: Vec<(String, NaiveDate)>,
}

impl Summary {
    /// `empty_summary`: a venue the walk has no rows of.
    pub fn empty() -> Summary { Summary { positions: vec![], total_invested: Dec::zero(), cash: vec![], cash_basis: vec![], incomplete: false, loss_sales: vec![] } }
}

/// Every scope from one walk: the venue's summary and the whole account's, the walked rows, and their terms.
#[derive(Clone, Debug)]
pub struct Walked { pub venue: Option<Summary>, pub whole: Summary, pub terms: Vec<Term> }

/// `enrich(ordered, currency: 'USD')` for the rows this build walks: each row's value in USD, and whether a price it
/// needed was missing.
pub fn enrich(stored: &[Stored], prices: &mut PriceBook) -> Result<Vec<Row>, Halt> {
    let links: HashMap<i64, i64> = stored.iter().filter_map(|t| t.linked_to.map(|d| (t.id, d))).collect();
    let deposits: std::collections::HashSet<i64> = links.values().copied().collect();
    let amounts: HashMap<i64, &Dec> = stored.iter().filter(|t| deposits.contains(&t.id)).map(|t| (t.id, &t.amount)).collect();
    let mut rows = Vec::with_capacity(stored.len());
    for t in stored {
        crate::figures::budget::check()?;
        let kept = prices.warnings;
        let fiat_value = row_value(t, prices)?;
        let transfer_fee = match (t.kind, links.get(&t.id)) {
            (Kind::Withdrawal, Some(d)) => match amounts.get(d) { Some(deposit) => Some((&t.amount - *deposit)?), None => None },
            _ => None,
        };
        rows.push(Row {
            kind: t.kind, base: t.base.clone(), amount: t.amount.clone(), quote: t.quote.clone(), quote_amount: t.quote_amount.clone(),
            fiat_value, at: t.at, price_missing: prices.warnings > kept, linked: links.contains_key(&t.id) || deposits.contains(&t.id),
            transfer_fee, per_share: t.per_share.clone(), opening: false,
        });
    }
    Ok(rows)
}

/// `resolve_row_value`: a withdrawal's or a lost coin's value is for the record, so its lookup never warns.
fn row_value(t: &Stored, prices: &mut PriceBook) -> Result<Dec, Halt> {
    if matches!(t.kind, Kind::Withdrawal | Kind::Lost) {
        let kept = prices.warnings;
        let value = fiat_value(t, prices);
        prices.warnings = kept;
        return value;
    }
    fiat_value(t, prices)
}

/// `resolve_fiat_value` in USD: a fiat base is worth nothing to the engines; then the row's own cash quote; then the
/// price the user stated; then the day's price.
fn fiat_value(t: &Stored, prices: &mut PriceBook) -> Result<Dec, Halt> {
    if fiat(&t.base) { return Ok(Dec::zero()); }
    if let (Some(quote), Some(amount)) = (t.quote.as_deref(), &t.quote_amount) {
        if quote == "USD" || stable(quote) { return Ok(amount.clone()); }
    }
    if let Some(price) = &t.stated { return Ok((price * &t.amount)?); }
    let price = prices.usd(&t.base, t.date())?;
    Ok((&price * &t.amount)?)
}

/// `quantity_moves`: what one row does to the quantity of the asset it touches (no row here carries a fee).
pub fn quantity_moves(kind: Kind, base: &str, amount: &Dec, linked: bool, transfer_fee: Option<&Dec>) -> Result<Vec<(String, Dec)>, FiguresError> {
    let zero = Dec::zero();
    Ok(if kind == Kind::Adjustment {
        vec![(base.into(), amount.clone())]
    } else if kind.base_in() {
        if kind == Kind::Deposit && linked { vec![] } else { vec![(base.into(), max(&(amount - &zero)?, &zero))] }
    } else if kind == Kind::Withdrawal {
        vec![(base.into(), if linked { transfer_fee.unwrap_or(&zero).neg() } else { amount.neg() })]
    } else if kind.base_out() {
        vec![(base.into(), (amount + &zero)?.neg())]
    } else { vec![] })
}

/// Ruby's `[a, b].max` and `.min`: the first on a tie.
fn max(a: &Dec, b: &Dec) -> Dec { if b > a { b.clone() } else { a.clone() } }
fn min(a: &Dec, b: &Dec) -> Dec { if b < a { b.clone() } else { a.clone() } }

/// `open_with_what_must_have_been_held`: an asset whose running quantity goes below zero is opened, a second before
/// its first row, with the least that keeps it at or above zero, at that day's price (unpriced when there is none).
pub fn open_with_held(rows: Vec<Row>, prices: &mut PriceBook) -> Result<Vec<Row>, Halt> {
    let mut running: HashMap<String, Dec> = HashMap::new();
    // Each asset's lowest running quantity below zero, in the order it first went below, found through `low_at`.
    let mut lowest: Vec<(String, Dec)> = vec![];
    let mut low_at: HashMap<String, usize> = HashMap::new();
    let mut first: HashMap<String, usize> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        crate::figures::budget::check()?;
        for (symbol, amount) in quantity_moves(row.kind, &row.base, &row.amount, row.linked, row.transfer_fee.as_ref())? {
            if cash(&symbol) { continue; }
            first.entry(symbol.clone()).or_insert(i);
            let now = (running.get(&symbol).unwrap_or(&Dec::zero()) + &amount)?;
            running.insert(symbol.clone(), now.clone());
            crate::figures::budget::charge(1, 0)?;
            match low_at.get(&symbol) {
                Some(&j) => if now < lowest[j].1 { lowest[j].1 = now },
                None if now.is_negative() => { low_at.insert(symbol.clone(), lowest.len()); lowest.push((symbol, now)); }
                None => {}
            }
        }
    }
    let mut before: HashMap<usize, Vec<Row>> = HashMap::new();
    for (symbol, low) in lowest {
        let Some((i, row)) = first.get(&symbol).and_then(|&i| rows.get(i).map(|r| (i, r))) else { continue };
        let at = row.at.plus_seconds(-1).ok_or_else(|| FiguresError::Data("an instant out of range".into()))?;
        let quantity = low.neg();
        let price = prices.usd_quiet(&symbol, at.utc().date_naive())?;
        before.entry(i).or_default().push(Row {
            kind: Kind::Deposit, base: symbol, fiat_value: (&price * &quantity)?, amount: quantity, quote: None, quote_amount: None, at,
            price_missing: price.is_zero(), linked: false, transfer_fee: None, per_share: None, opening: true,
        });
    }
    if before.is_empty() { return Ok(rows); }
    let mut out = Vec::with_capacity(rows.len() + before.len());
    for (i, row) in rows.into_iter().enumerate() {
        if let Some(openings) = before.remove(&i) { out.extend(openings); }
        out.push(row);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct Lot { amount: Dec, cost_per_unit: Dec, basis_assumed: bool, unpriced: Dec }
struct Tranche { amount: Dec, cost: Dec, unpriced: Dec }

/// A disposal, as far as the figures and the arming read one.
#[derive(Clone, Debug)]
pub struct Disposal { pub at: At, pub asset: String, pub unpriced_quantity: Dec, pub any_lot_lost: bool }

/// Tracker::Ledger::Engine on one venue: FIFO lots per asset, the disposals, whether a disposal took more than the
/// lots held, and the basis each in-kind consumption released (read back by money in).
#[derive(Default)]
pub struct Engine {
    lots: BTreeMap<String, VecDeque<Lot>>,
    pub disposals: Vec<Disposal>,
    pub uncovered: bool,
    released: HashMap<(String, String), VecDeque<Dec>>,
}

fn sum(values: impl Iterator<Item = Result<Dec, crate::figures::num::NumError>>) -> Result<Dec, FiguresError> {
    let mut total = Dec::zero();
    for v in values { total = (&total + &v?)?; }
    Ok(total)
}

impl Engine {
    fn lots(&mut self, asset: &str) -> &mut VecDeque<Lot> { self.lots.entry(asset.into()).or_default() }

    fn pool_basis(&mut self, asset: &str) -> Result<Dec, FiguresError> { sum(self.lots(asset).iter().map(|l| &l.amount * &l.cost_per_unit)) }

    /// Tax::Methods::Fifo#dequeue_tranches.
    fn dequeue(&mut self, asset: &str, amount: &Dec) -> Result<(Vec<Tranche>, Dec), FiguresError> {
        let lots = self.lots(asset);
        let held = sum(lots.iter().map(|l| Ok(l.amount.clone())))?;
        let mut remaining = amount.clone();
        let mut tranches = vec![];
        while remaining.is_positive() && !lots.is_empty() {
            crate::figures::budget::check()?;
            let Some(lot) = lots.front_mut() else { break };
            let take = min(&lot.amount, &remaining);
            let unpriced = if lot.amount.is_positive() { (&lot.unpriced * &take)?.div(&lot.amount)? } else { Dec::zero() };
            tranches.push(Tranche { cost: (&take * &lot.cost_per_unit)?, amount: take, unpriced: unpriced.clone() });
            if lot.amount <= remaining {
                remaining = (&remaining - &lot.amount)?;
                lots.pop_front();
            } else {
                lot.amount = (&lot.amount - &remaining)?;
                lot.unpriced = (&lot.unpriced - &unpriced)?;
                remaining = Dec::zero();
            }
        }
        Ok((tranches, held))
    }

    /// An acquisition's lot (no fee is carried on any row here).
    fn acquire(&mut self, row: &Row, basis_assumed: bool) -> Result<(), FiguresError> {
        let cost = (&row.fiat_value + &Dec::zero())?;
        let cost_per_unit = if row.amount.is_positive() { cost.div(&row.amount)? } else { Dec::zero() };
        let unpriced = if row.price_missing { row.amount.clone() } else { Dec::zero() };
        self.lots(&row.base).push_back(Lot { amount: row.amount.clone(), cost_per_unit, basis_assumed, unpriced });
        Ok(())
    }

    /// Engine#record_disposal: cash leaves its lots silently; anything else is a disposal, and one that takes more than
    /// was held leaves the walk uncovered.
    fn dispose(&mut self, asset: &str, amount: &Dec, fiat_value: &Dec, at: At) -> Result<(), FiguresError> {
        if cash(asset) { self.dequeue(asset, amount)?; return Ok(()); }
        let (tranches, held) = self.dequeue(asset, amount)?;
        let unpriced_quantity = sum(tranches.iter().map(|t| Ok(t.unpriced.clone())))?;
        let mut any_lot_lost = false;
        if amount.is_positive() {
            for t in &tranches { if (fiat_value * &t.amount)? < (&t.cost * amount)? { any_lot_lost = true; break; } }
        }
        self.disposals.push(Disposal { at, asset: asset.into(), unpriced_quantity, any_lot_lost });
        if held < *amount { self.uncovered = true; }
        Ok(())
    }

    /// Tax::Methods::Fifo#apply_split: every lot keeps its cost and its date; the pool is scaled by the net delta.
    fn split(&mut self, row: &Row) -> Result<(), FiguresError> {
        let amount = &row.amount;
        let pool = sum(self.lots(&row.base).iter().map(|l| Ok(l.amount.clone())))?;
        let factor = if pool.is_positive() { (&pool + amount)?.div(&pool)? } else { Dec::zero() };
        let lots = self.lots(&row.base);
        if !amount.is_zero() && pool.is_positive() && factor.is_positive() {
            for lot in lots.iter_mut() {
                lot.cost_per_unit = lot.cost_per_unit.div(&factor)?;
                lot.amount = (&lot.amount * &factor)?;
                lot.unpriced = (&lot.unpriced * &factor)?;
            }
        } else if amount.is_negative() && !factor.is_positive() {
            lots.clear();
        } else if amount.is_positive() && pool.is_zero() {
            lots.push_back(Lot { amount: amount.clone(), cost_per_unit: Dec::zero(), basis_assumed: true, unpriced: amount.clone() });
        }
        Ok(())
    }

    /// ReturnOfCapital#reduce_lot_basis: per share when the row states a rate and lots are held, else dollar for
    /// dollar, oldest lot first. What finds no basis is excess, which only the realised figure reads.
    fn return_capital(&mut self, row: &Row) -> Result<(), FiguresError> {
        let per_unit = match &row.per_share {
            Some(per_share) if per_share.is_positive() => {
                let fx = match &row.quote_amount { Some(q) if !q.is_zero() => row.fiat_value.div(q)?, _ => Dec::one() };
                Some((per_share * &fx)?)
            }
            _ => None,
        };
        let lots = self.lots(&row.base);
        if let (Some(per_unit), false) = (&per_unit, lots.is_empty()) {
            for lot in lots.iter_mut() { lot.cost_per_unit = max(&(&lot.cost_per_unit - per_unit)?, &Dec::zero()); }
            return Ok(());
        }
        let mut remaining = row.fiat_value.clone();
        for lot in lots.iter_mut() {
            if !remaining.is_positive() { break; }
            let lot_cost = (&lot.amount * &lot.cost_per_unit)?;
            let take = min(&lot_cost, &remaining);
            lot.cost_per_unit = if lot.amount.is_positive() { (&lot_cost - &take)?.div(&lot.amount)? } else { Dec::zero() };
            remaining = (&remaining - &take)?;
        }
        Ok(())
    }

    /// Engine#consume_fee_in_kind: coins that left for nothing take their cost from the lots, and the basis they took is
    /// recorded for money in.
    fn consume_in_kind(&mut self, asset: &str, amount: &Dec) -> Result<(), FiguresError> {
        let before = self.pool_basis(asset)?;
        if !fiat(asset) && amount.is_positive() && !self.lots(asset).is_empty() { self.dequeue(asset, amount)?; }
        let released = (&before - &self.pool_basis(asset)?)?;
        self.released.entry((asset.into(), amount.to_s_f())).or_default().push_back(released);
        Ok(())
    }

    /// `basis_released`: what the lots gave up when these coins left, in the order they left.
    fn basis_released(&mut self, asset: &str, amount: &Dec) -> Option<Dec> {
        self.released.get_mut(&(asset.to_string(), amount.to_s_f())).and_then(VecDeque::pop_front)
    }

    /// Tax::Methods::Fifo#calculate over the taxable rows (a fiat row reaches no engine), remapped as the tracker
    /// remaps them: a lost coin is a sale for nothing; an unlinked withdrawal left the tracked universe (a fee).
    pub fn calculate(&mut self, rows: &[Row]) -> Result<(), FiguresError> {
        for row in rows.iter().filter(|r| !fiat(&r.base)) {
            crate::figures::budget::check()?;
            match row.kind {
                Kind::Buy | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => self.acquire(row, row.price_missing)?,
                Kind::Deposit if row.linked => {}
                Kind::Deposit => self.acquire(row, true)?,
                Kind::Sell => self.dispose(&row.base, &row.amount, &row.fiat_value, row.at)?,
                Kind::Lost => self.dispose(&row.base, &row.amount, &Dec::zero(), row.at)?,
                Kind::Adjustment => self.split(row)?,
                Kind::ReturnOfCapital => self.return_capital(row)?,
                Kind::Fee => self.consume_in_kind(&row.base, &row.amount)?,
                Kind::Withdrawal if !row.linked => self.consume_in_kind(&row.base, &row.amount)?,
                Kind::Withdrawal | Kind::SwapIn | Kind::SwapOut => return Err(super::refused("a coin's transfer or swap")),
                Kind::WithholdingTax | Kind::Unsupported => {}
            }
        }
        Ok(())
    }

    /// `positions_from`: what the lots hold, cash left out, largest cost first.
    fn positions(&self) -> Result<Vec<Position>, FiguresError> {
        let mut out = vec![];
        for (symbol, lots) in &self.lots {
            if cash(symbol) { continue; }
            let quantity = sum(lots.iter().map(|l| Ok(l.amount.clone())))?;
            if !quantity.is_positive() { continue; }
            let cost = sum(lots.iter().map(|l| &l.amount * &l.cost_per_unit))?;
            out.push(Position { symbol: symbol.clone(), avg_cost: cost.div(&quantity)?, quantity, cost,
                                estimated: lots.iter().any(|l| l.basis_assumed), unpriced: sum(lots.iter().map(|l| Ok(l.unpriced.clone())))? });
        }
        crate::figures::budget::charge(super::rows::sort_steps(out.len()), 0)?;
        out.sort_by(|a, b| b.cost.cmp(&a.cost));
        Ok(out)
    }
}

/// Tracker::CashBook on one venue: per currency, the units held and the dollars they carried in.
#[derive(Default)]
struct Book { cash: Vec<(String, Dec)>, basis: Vec<(String, Dec)> }

fn slot<'a>(pots: &'a mut Vec<(String, Dec)>, currency: &str) -> &'a mut Dec {
    let i = match pots.iter().position(|(c, _)| c == currency) { Some(i) => i, None => { pots.push((currency.into(), Dec::zero())); pots.len() - 1 } };
    &mut pots[i].1
}

impl Book {
    /// `value`: dollars and stablecoins at face (no other cash reaches this build).
    fn value(currency: &str, units: &Dec) -> Result<Dec, FiguresError> {
        if units.is_zero() || currency == "USD" || stable(currency) { return Ok(units.clone()); }
        Err(super::refused("cash other than US dollars"))
    }

    fn credit(&mut self, currency: &str, units: &Dec, usd: &Dec) -> Result<(), FiguresError> {
        let c = slot(&mut self.cash, currency); *c = (&*c + units)?;
        let b = slot(&mut self.basis, currency); *b = (&*b + usd)?;
        Ok(())
    }

    /// The basis `units` take with them; what the pot never had leaves at the day's value.
    fn release(&mut self, currency: &str, units: &Dec) -> Result<Dec, FiguresError> {
        let held = slot(&mut self.cash, currency).clone();
        let covered = if held.is_positive() { min(units, &held) } else { Dec::zero() };
        let basis = slot(&mut self.basis, currency).clone();
        let from_pot = if covered.is_positive() { (&basis * &covered)?.div(&held)? } else { Dec::zero() };
        let released = (&from_pot + &Book::value(currency, &(units - &covered)?)?)?;
        let b = slot(&mut self.basis, currency); *b = (&*b - &released)?;
        let c = slot(&mut self.cash, currency); *c = (&*c - units)?;
        Ok(released)
    }

    /// `move`: returns the basis the withdrawn units carried. (What it cost and what it paid for only the realised
    /// figure reads.)
    fn shift(&mut self, currency: &str, amount: &Dec, withdrawn: &Dec, worth: Option<&Dec>) -> Result<Dec, FiguresError> {
        if amount.is_positive() {
            let usd = match worth { Some(w) => w.clone(), None => Book::value(currency, amount)? };
            self.credit(currency, amount, &usd)?;
            return Ok(Dec::zero());
        }
        let out = amount.neg();
        let released = self.release(currency, &out)?;
        let withdrawn = min(withdrawn, &out);
        Ok((&released * &withdrawn)?.div(&out)?)
    }

    /// `carry` to the same venue: a linked dollar transfer, its fee left behind.
    fn carry(&mut self, currency: &str, units: &Dec, cost: &Dec) -> Result<(), FiguresError> {
        let released = self.release(currency, units)?;
        let cost = min(cost, units);
        let arrived = (units - &cost)?;
        let carried = (&released * &arrived)?.div(units)?;
        self.credit(currency, &arrived, &carried)
    }
}

/// Tracker::UnfundedCash.moves for a row with no fee: the quote leg of a trade, and the base leg where the base is
/// itself cash; zeros and anything not cash left out.
pub fn cash_moves(kind: Kind, base: &str, amount: &Dec, quote: Option<&str>, quote_amount: Option<&Dec>) -> Result<Vec<(String, Dec)>, FiguresError> {
    let mut moves = vec![];
    if let Some(qa) = quote_amount {
        let direction = match kind { Kind::Buy => -1, Kind::Sell | Kind::ReturnOfCapital => 1, _ => 0 };
        moves.push((quote.unwrap_or("").to_string(), (&Dec::from_i64(direction) * qa)?));
    }
    if kind.base_in() || kind.base_out() { moves.push((base.into(), if kind.base_in() { amount.clone() } else { amount.neg() })); }
    Ok(moves.into_iter().filter(|(c, a)| cash(c) && !a.is_zero()).collect())
}

/// Ledger.money_in_terms on one venue, and the cash left standing: one term per row.
fn money_in(rows: &[Row], engine: &mut Engine, book: &mut Book) -> Result<Vec<Term>, FiguresError> {
    let mut terms = Vec::with_capacity(rows.len());
    for row in rows {
        crate::figures::budget::check()?;
        let withdrawn = book_cash(book, row)?;
        // `coin_cost` (ledger.rb:884, :1067-1075): a coin fee takes the basis its lots released off the queue, so a
        // later withdrawal of the same amount reads its own. (A linked coin withdrawal's slice is refused upstream.)
        if row.kind == Kind::Fee && !cash(&row.base) { engine.basis_released(&row.base, &row.amount); }
        let amount = contribution(row, engine, &withdrawn)?;
        let valued_by_price = row.kind.in_kind() || (row.kind == Kind::Deposit && !row.linked && !cash(&row.base));
        terms.push(Term { at: row.at, amount, complete: !(row.price_missing && valued_by_price),
                          opens: row.opening.then(|| (row.base.clone(), row.amount.clone())) });
    }
    Ok(terms)
}

/// `book_cash`: every cash move of the row into the book, each outflow sorted into what it cost and what was
/// withdrawn. Returns the basis the withdrawn cash carried.
fn book_cash(book: &mut Book, row: &Row) -> Result<Dec, FiguresError> {
    let mut costs: Vec<(String, Dec)> = vec![];
    if matches!(row.kind, Kind::Fee | Kind::WithholdingTax | Kind::Lost) && cash(&row.base) {
        let abs = if row.amount.is_negative() { row.amount.neg() } else { row.amount.clone() };
        let c = slot(&mut costs, &row.base); *c = (&*c + &abs)?;
    }
    let mut moves = cash_moves(row.kind, &row.base, &row.amount, row.quote.as_deref(), row.quote_amount.as_ref())?;
    if row.linked && cash(&row.base) {
        if row.kind == Kind::Withdrawal {
            let fee = max(row.transfer_fee.as_ref().unwrap_or(&Dec::zero()), &Dec::zero());
            book.carry(&row.base, &row.amount, &fee)?;
        } else {
            let arrived = sum(moves.iter().map(|(c, a)| Ok(if *c == row.base { a.clone() } else { Dec::zero() })))?;
            let netted = (&row.amount - &arrived)?;
            if netted.is_positive() { book.shift(&row.base, &netted.neg(), &Dec::zero(), None)?; }
        }
        moves.retain(|(c, _)| *c != row.base);
        costs.retain(|(c, _)| *c != row.base);
    }
    let withdrawing = row.kind == Kind::Withdrawal && !row.linked;
    // `cash_counterpart_worth`: cash bought with cash is worth what the quote came to.
    let worth = match (&row.quote, &row.quote_amount) {
        (Some(q), Some(qa)) if cash(&row.base) && cash(q) => Some((row.base.clone(), Book::value(q, qa)?)),
        _ => None,
    };
    let mut withdrawn = Dec::zero();
    for (currency, amount) in &moves {
        let out = if amount.is_negative() { amount.neg() } else { Dec::zero() };
        let c = slot(&mut costs, currency);
        let cost = min(c, &out);
        *c = (&*c - &cost)?;
        let taken = if withdrawing && *currency == row.base { (&out - &cost)? } else { Dec::zero() };
        let w = worth.as_ref().filter(|(b, _)| b == currency).map(|(_, w)| w);
        withdrawn = (&withdrawn + &book.shift(currency, amount, &taken, w)?)?;
    }
    Ok(withdrawn)
}

/// `contribution`: money in from outside. A deposit at its basis, a withdrawal at the basis it took, an arrival at
/// its value; a linked transfer, a trade and everything else nothing.
fn contribution(row: &Row, engine: &mut Engine, withdrawn: &Dec) -> Result<Dec, FiguresError> {
    let direction = match row.kind {
        Kind::Deposit => 1,
        Kind::Withdrawal => -1,
        k if k.in_kind() => return Ok(if fiat(&row.base) { row.amount.clone() } else { row.fiat_value.clone() }),
        _ => return Ok(Dec::zero()),
    };
    if row.linked { return Ok(Dec::zero()); }
    let value = if cash(&row.base) && direction < 0 {
        withdrawn.clone()
    } else if fiat(&row.base) || stable(&row.base) {
        row.amount.clone()
    } else if direction > 0 {
        row.fiat_value.clone()
    } else {
        engine.basis_released(&row.base, &row.amount).unwrap_or_else(Dec::zero)
    };
    Ok((&value * &Dec::from_i64(direction))?)
}

/// `loss_sales`: per symbol, the latest loss-making disposal no older than 31 days.
pub fn loss_sales(disposals: &[Disposal], today: NaiveDate) -> Result<Vec<(String, NaiveDate)>, FiguresError> {
    let horizon = today.checked_sub_days(chrono::Days::new(31)).unwrap_or(today);
    let mut acc: Vec<(String, NaiveDate)> = vec![];
    let mut at: HashMap<&str, usize> = HashMap::new();
    for d in disposals.iter().filter(|d| d.any_lot_lost) {
        crate::figures::budget::charge(1, 0)?;
        let on = d.at.utc().date_naive();
        if on < horizon { continue; }
        match at.get(d.asset.as_str()) {
            Some(&i) => if on > acc[i].1 { acc[i].1 = on },
            None => { at.insert(&d.asset, acc.len()); acc.push((d.asset.clone(), on)); }
        }
    }
    Ok(acc)
}

/// The rows enriched and opened, ready for the engine: the part of the walk that can halt for a price.
pub fn prepare(stored: &[Stored], prices: &mut PriceBook) -> Result<Vec<Row>, Halt> {
    let rows = enrich(stored, prices)?;
    open_with_held(rows, prices)
}

/// Tracker::Ledger.scopes over prepared rows: the venue's summary (None when the account has no rows) and the
/// whole account's.
pub fn walk(rows: &[Row], warnings: usize, today: NaiveDate) -> Result<Walked, FiguresError> {
    let mut engine = Engine::default();
    engine.calculate(rows)?;
    let mut book = Book::default();
    let terms = money_in(rows, &mut engine, &mut book)?;
    let whole_incomplete = |venue: bool| venue || engine.uncovered || warnings > 0;
    let positions = engine.positions()?;
    let venue = if rows.is_empty() { None } else {
        let incomplete = positions.iter().any(|p| p.unpriced.is_positive()) || engine.uncovered
            || engine.disposals.iter().any(|d| d.unpriced_quantity.is_positive())
            || rows.iter().any(|r| r.price_missing) || terms.iter().any(|t| !t.complete);
        Some(Summary {
            positions: positions.clone(),
            total_invested: sum(terms.iter().map(|t| Ok(t.amount.clone())))?,
            cash: book.cash.clone(),
            cash_basis: book.basis.iter().filter(|(_, b)| !b.is_zero()).cloned().collect(),
            incomplete, loss_sales: vec![],
        })
    };
    // The whole account: the venues added up (one venue here), and the loss sales.
    let whole = match &venue {
        None => Summary { incomplete: whole_incomplete(false), ..Summary::empty() },
        Some(v) => Summary {
            positions: v.positions.clone(),
            total_invested: (&Dec::zero() + &v.total_invested)?,
            cash: v.cash.iter().map(|(c, a)| Ok((c.clone(), (&Dec::zero() + a)?))).collect::<Result<_, FiguresError>>()?,
            cash_basis: v.cash_basis.iter().map(|(c, a)| Ok((c.clone(), (&Dec::zero() + a)?))).collect::<Result<Vec<_>, FiguresError>>()?
                .into_iter().filter(|(_, a)| !a.is_zero()).collect(),
            incomplete: whole_incomplete(v.incomplete),
            loss_sales: loss_sales(&engine.disposals, today)?,
        },
    };
    Ok(Walked { venue, whole, terms })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec { Dec::strict(s).unwrap() }
    fn at(s: &str) -> At { At::from_sql(s).unwrap() }
    fn row(kind: Kind, base: &str, amount: &str, quote: Option<(&str, &str)>, when: &str) -> Row {
        let quote_amount = quote.map(|(_, a)| d(a));
        Row { kind, base: base.into(), amount: d(amount), quote: quote.map(|(q, _)| q.into()), fiat_value: if super::fiat(base) { Dec::zero() } else { quote_amount.clone().unwrap_or_else(Dec::zero) },
              quote_amount, at: at(when), price_missing: false, linked: false, transfer_fee: None, per_share: None, opening: false }
    }
    fn today() -> NaiveDate { NaiveDate::from_ymd_opt(2026, 10, 1).unwrap() }
    fn texts(list: &[(String, Dec)]) -> Vec<(String, String)> { list.iter().map(|(c, a)| (c.clone(), a.to_s_f())).collect() }

    /// Rails' scopes for the grid's `sell_at_loss_wash_on` (rust/tests/tracker_parity.rs): a deposit, two AAPL at 200,
    /// one sold at 150.
    #[test]
    fn a_loss_sale_walks_as_rails_walks_it() {
        let rows = vec![row(Kind::Deposit, "USD", "1000", None, "2026-09-01 14:00:00"), row(Kind::Buy, "AAPL", "2", Some(("USD", "400")), "2026-09-02 14:30:00"),
                        row(Kind::Sell, "AAPL", "1", Some(("USD", "150")), "2026-09-20 14:30:00")];
        let w = walk(&rows, 0, today()).unwrap();
        let p = &w.whole.positions[0];
        assert_eq!((p.symbol.as_str(), p.quantity.to_s_f(), p.cost.to_s_f(), p.avg_cost.to_s_f(), p.estimated, p.unpriced.to_s_f()),
                   ("AAPL", "1.0".into(), "200.0".into(), "200.0".into(), false, "0.0".into()));
        assert_eq!((w.whole.total_invested.to_s_f(), texts(&w.whole.cash), texts(&w.whole.cash_basis), w.whole.incomplete),
                   ("1000.0".into(), vec![("USD".into(), "750.0".into())], vec![("USD".into(), "750.0".into())], false));
        assert_eq!(w.whole.loss_sales, [("AAPL".to_string(), NaiveDate::from_ymd_opt(2026, 9, 20).unwrap())]);
        assert!(w.venue.unwrap().loss_sales.is_empty(), "only the whole account judges a loss");
    }

    /// `dividends_fees_withdrawal`: a dividend is money in at its value; a withholding, a fee and a withdrawal take their
    /// basis out of the cash.
    #[test]
    fn dividends_fees_and_a_withdrawal_move_money_in_and_the_cash_as_rails_does() {
        let rows = vec![row(Kind::Deposit, "USD", "1000", None, "2026-09-01 14:00:00"), row(Kind::Buy, "QQQM", "1", Some(("USD", "500")), "2026-09-02 14:30:00"),
                        row(Kind::OtherIncome, "USD", "5", None, "2026-09-10 00:00:00"), row(Kind::WithholdingTax, "USD", "0.75", None, "2026-09-10 00:00:00"),
                        row(Kind::Fee, "USD", "1", None, "2026-09-11 00:00:00"), row(Kind::Withdrawal, "USD", "100", None, "2026-09-12 00:00:00")];
        let w = walk(&rows, 0, today()).unwrap();
        assert_eq!((w.whole.total_invested.to_s_f(), texts(&w.whole.cash), texts(&w.whole.cash_basis)),
                   ("905.0".into(), vec![("USD".into(), "403.25".into())], vec![("USD".into(), "403.25".into())]));
        assert_eq!(w.terms.iter().map(|t| t.amount.to_s_f()).collect::<Vec<_>>(), ["1000.0", "0.0", "5.0", "0.0", "0.0", "-100.0"]);
    }

    /// A coin fee consumes the basis its lots released, as Rails' `coin_cost` does, so a later withdrawal of the same
    /// amount takes the basis of the lot it left from: 0.1 BTC bought for $10, paid as a fee, 0.1 bought for $20 and
    /// withdrawn takes $20 out of the $100 put in.
    #[test]
    fn a_coin_fee_takes_its_own_released_basis_before_a_withdrawal() {
        let rows = vec![row(Kind::Deposit, "USD", "100", None, "2026-09-01 14:00:00"), row(Kind::Buy, "BTC", "0.1", Some(("USD", "10")), "2026-09-02 14:00:00"),
                        row(Kind::Fee, "BTC", "0.1", None, "2026-09-03 14:00:00"), row(Kind::Buy, "BTC", "0.1", Some(("USD", "20")), "2026-09-04 14:00:00"),
                        row(Kind::Withdrawal, "BTC", "0.1", None, "2026-09-05 14:00:00")];
        let w = walk(&rows, 0, today()).unwrap();
        assert_eq!((w.whole.total_invested.to_s_f(), w.terms.last().map(|t| t.amount.to_s_f())), ("80.0".into(), Some("-20.0".into())));
    }

    /// `return_of_capital`: a rate per share comes off every lot's cost.
    #[test]
    fn a_return_of_capital_per_share_lowers_the_cost() {
        let mut roc = row(Kind::ReturnOfCapital, "AAPL", "10", Some(("USD", "5")), "2026-09-20 00:00:00");
        roc.per_share = Some(d("0.5"));
        let rows = vec![row(Kind::Deposit, "USD", "2000", None, "2026-09-01 14:00:00"), row(Kind::Buy, "AAPL", "10", Some(("USD", "1000")), "2026-09-02 14:30:00"), roc];
        let p = walk(&rows, 0, today()).unwrap().whole.positions[0].clone();
        assert_eq!((p.cost.to_s_f(), p.avg_cost.to_s_f()), ("995.0".into(), "99.5".into()));
    }

    /// A sale that takes more than was held leaves the walk incomplete, and a warning anywhere leaves the whole so.
    #[test]
    fn an_overdrawn_sale_and_a_warning_are_incomplete() {
        let rows = vec![row(Kind::Buy, "AAPL", "1", Some(("USD", "100")), "2026-09-02 14:30:00"), row(Kind::Sell, "AAPL", "2", Some(("USD", "250")), "2026-09-03 14:30:00")];
        let w = walk(&rows, 0, today()).unwrap();
        assert!(w.whole.incomplete && w.venue.unwrap().incomplete);
        let fine = vec![row(Kind::Buy, "AAPL", "1", Some(("USD", "100")), "2026-09-02 14:30:00")];
        assert_eq!((walk(&fine, 0, today()).unwrap().whole.incomplete, walk(&fine, 1, today()).unwrap().whole.incomplete), (false, true));
    }

    /// Budgets: a long history walks well within one figure's limits, and a history whose numbers grow without bound
    /// stops at the limit with the reason, rather than running for hours.
    #[test]
    fn a_long_history_walks_within_its_budget_and_a_runaway_one_stops_at_it() {
        let mut rows = vec![row(Kind::Deposit, "USD", "1000000", None, "2020-01-01 00:00:00")];
        let start = at("2020-01-02 00:00:00");
        for i in 0..5_000i64 {
            let when = At(start.0 + i * 3_600_000_000_000);
            let mut r = match i % 4 {
                0 | 1 => row(Kind::Buy, if i % 8 < 4 { "AAPL" } else { "QQQM" }, "0.333", Some(("USD", "71.07")), "2020-01-01 00:00:00"),
                2 => row(Kind::Sell, if i % 8 < 4 { "AAPL" } else { "QQQM" }, "0.1", Some(("USD", "23.13")), "2020-01-01 00:00:00"),
                _ => row(Kind::OtherIncome, "USD", "0.07", None, "2020-01-01 00:00:00"),
            };
            r.at = when;
            rows.push(r);
        }
        let (out, used) = crate::figures::budget::scope(crate::figures::budget::FIGURE, || walk(&rows, 0, today()));
        assert!(out.is_ok(), "{:?}", out.err());
        assert!(used.steps < crate::figures::budget::FIGURE.steps / 20, "5,001 rows took {} steps", used.steps);
        // Splits by a factor whose quotient never ends: each one adds sixteen digits to every lot's cost per unit.
        let mut runaway = vec![row(Kind::Buy, "AAPL", "3", Some(("USD", "100")), "2020-01-01 00:00:00")];
        for i in 0..20_000i64 { let mut s = row(Kind::Adjustment, "AAPL", if i % 2 == 0 { "4" } else { "-4" }, None, "2020-01-01 00:00:00"); s.at = At(start.0 + i); runaway.push(s); }
        let (out, _) = crate::figures::budget::scope(crate::figures::budget::FIGURE, || walk(&runaway, 0, today()));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET), "{:?}", out.err());
    }

    /// 100,000 symbols, each bought and sold at a loss, walk in linear work: their lowest quantities and loss sales are
    /// found through an index, each lookup a step, and the whole is refused under an allowance it exceeds. Sold before
    /// any purchase, their openings are found the same way before the first price is asked for.
    #[test]
    fn many_symbols_walk_in_linear_work_within_the_allowance() {
        use crate::figures::budget::{scope, Limits, FIGURE};
        let start = at("2026-09-20 00:00:00");
        let symbols: Vec<String> = (0..100_000).map(|i| format!("S{i}")).collect();
        let mut rows = vec![];
        for (i, s) in symbols.iter().enumerate() {
            let mut buy = row(Kind::Buy, s, "1", Some(("USD", "10")), "2026-09-20 00:00:00");
            let mut sell = row(Kind::Sell, s, "1", Some(("USD", "5")), "2026-09-20 00:00:00");
            (buy.at, sell.at) = (At(start.0 + 2 * i as i64), At(start.0 + 2 * i as i64 + 1));
            rows.extend([buy, sell]);
        }
        let small = Limits { steps: 50_000, held: FIGURE.held };
        let (out, used) = scope(FIGURE, || walk(&rows, 0, today()));
        assert_eq!(out.unwrap().whole.loss_sales.len(), 100_000);
        assert!(used.steps < FIGURE.steps / 10, "{} steps", used.steps);
        assert!(matches!(scope(small, || walk(&rows, 0, today())).0, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET));
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); CREATE TABLE historical_prices (asset TEXT, currency TEXT, date TEXT, price NUMERIC);
                         CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, base_asset_id INTEGER);
                         CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT, symbol TEXT, market_cap_rank INTEGER);").unwrap();
        let sold: Vec<Row> = rows.iter().filter(|r| r.kind == Kind::Sell).cloned().collect();
        let fetched = std::collections::HashMap::new();
        let venue = super::super::prices::Venue { id: 1, name_id: "alpaca".into(), stock: true };
        let opened = |limits| scope(limits, || {
            let r = super::super::prices::Reference::load(&c, &venue, &symbols.iter().cloned().collect()).map_err(Halt::Fail)?;
            let mut book = PriceBook { r: &r, venue: venue.clone(), today: today(), fetched: &fetched, warnings: 0 };
            open_with_held(sold.clone(), &mut book).map(|_| ())
        });
        let (out, used) = opened(FIGURE);
        assert!(matches!(out, Err(Halt::Fail(FiguresError::NotComputed(ref m))) if m.starts_with("no price of S0 on 2026-09-19: no coin can be named")), "{:?}", out.err());
        assert!(used.steps >= 100_000 && used.steps < FIGURE.steps / 10, "{} steps", used.steps);
        assert!(matches!(opened(small).0, Err(Halt::Fail(FiguresError::NotComputed(ref m))) if m == crate::figures::OVER_BUDGET));
    }
}
