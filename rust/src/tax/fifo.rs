//! Tax::Methods::Fifo with AcquisitionFee and ReturnOfCapital (app/models/tax/methods/{fifo,acquisition_fee,
//! return_of_capital}.rb), method by method, over the rows Tax::Report hands it, with Report#calculation_options.
//! One lot store per asset, as Rails (`new_lot_store`); LIFO and the 4-week rule (A12-4) hang on `Order` and `Fifo::take`.
//!
//! Where Rust departs from Rails, by name:
//! - D-TAX-UNCOVERED: a disposal some unit of which no lot covered is `data_incomplete`, its uncovered quantity kept for
//!   the report's warning block (Rails marks it complete when any lot existed).
//! - D-TAX-LOST: a `lost` row takes its lots in FIFO order with no disposal row (Rails has no branch, and sells the lost
//!   coins again later at their old basis).
//! - D-TAX-COIN-FEE: where spending a coin is a disposal (`crypto_to_crypto_taxable`), a coin fee of a row of its own is
//!   a disposal at its market value, its basis from the lots and no fee column; elsewhere its lots go at zero, as Rails.
//! - D-TAX-US-WALLET (`Options::per_venue_from`): from the start of 2025 in the user's zone, lots per venue (US
//!   Treas. Reg. 1.1012-1(j), wallet by wallet). The walk before it is one pool, as Rails; at the boundary the pool as it
//!   stands is placed on the venues that hold the units then (oldest lots to the venue first seen), so no lot an earlier
//!   year's report used is used again. A linked transfer between venues carries its lots; an unlinked withdrawal takes
//!   them off the venue; an unlinked deposit opens at market value, as everywhere.
use crate::figures::{at::At, budget, dec::Dec, FiguresError};
use crate::tracker::rows::Kind;
use crate::tracker::walk::Row;
use crate::tracker::{fiat, stable};
use std::collections::{HashMap, VecDeque};

/// Tax::Methods::Fifo::IMMEDIATE_EXIT, in seconds.
const IMMEDIATE_EXIT: f64 = 24.0 * 3600.0;
/// Fifo::CENTS.
const CENTS: i64 = 2;

/// A row as the engine reads it: the enriched row, its venue id, and whether it leaves for fiat (`fiat_exit?`).
pub struct Tx { pub row: Row, pub tx_id: Option<String>, pub fiat_exit: bool }

/// Report#calculation_options for this engine.
pub struct Options<'a> {
    pub swaps_taxable: bool,
    pub stablecoin_as_fiat: bool,
    /// `holding_rule`: (acquired_at, disposed_at) → held long.
    pub holding: Option<&'a dyn Fn(At, At) -> bool>,
    pub swap_resets_holding_period: bool,
    /// D-TAX-US-WALLET: the instant lots start being kept per venue.
    pub per_venue_from: Option<At>,
}

#[derive(Clone, Debug)]
pub struct Lot { pub amount: Dec, pub cost_per_unit: Dec, pub date: At, pub basis_assumed: bool, pub unpriced: Dec,
                 holding_start: Option<At>, technical_start: Option<At> }

#[derive(Clone, Debug)]
struct Tranche { amount: Dec, cost: Dec, date: At, basis_assumed: bool, unpriced: Dec, holding_start: Option<At>,
                 technical_start: Option<At>, weight: Dec }

/// One disposal row (`disposal_row`), `long_held` set where a holding rule split the sale.
#[derive(Clone, Debug)]
pub struct Disposal {
    pub at: At, pub acquired: Option<At>, pub asset: String, pub amount: Dec, pub proceeds: Dec, pub cost_basis: Dec, pub fee: Dec,
    pub gain_loss: Dec, pub holding_days: i64, pub complete: bool, pub incomplete: bool, pub tx_id: Option<String>,
    pub exchange: String, pub long_held: Option<bool>,
    /// D-TAX-UNCOVERED: the units of this row no lot covered.
    pub uncovered: Dec,
}

type Key = (Option<String>, String);
type Group = (String, String);
struct InGroup { shares: HashMap<usize, Dec>, basis_assumed: bool }
struct OutGroup { weights: HashMap<usize, Dec>, value: Dec }

pub struct Fifo<'a> {
    o: Options<'a>,
    lots: HashMap<Key, VecDeque<Lot>>,
    in_groups: HashMap<Group, InGroup>,
    out_groups: HashMap<Group, OutGroup>,
    transferred: HashMap<Group, Vec<Tranche>>,
    per_venue: bool,
    /// D-TAX-US-WALLET, before the boundary: what each venue holds of each asset, in the order first touched.
    held: Vec<((String, String), Dec)>,
    pub disposals: Vec<Disposal>,
    /// `excess_roc_rows`, as indexes into the rows.
    pub excess_roc_rows: Vec<usize>,
    pub excess_roc: Dec,
    /// Basis that left the lots with no disposal row: fees in kind, transfer fees, lost coins, coins off a venue.
    pub consumed: Dec,
}

fn sum<'a>(it: impl Iterator<Item = &'a Dec>) -> Result<Dec, FiguresError> {
    let mut t = Dec::zero();
    for v in it { t = (&t + v)?; }
    Ok(t)
}
fn unit(cost: &Dec, amount: &Dec) -> Result<Dec, FiguresError> { Ok(if amount.is_positive() { cost.div(amount)? } else { Dec::zero() }) }
fn max0(v: Dec) -> Dec { if v.is_negative() { Dec::zero() } else { v } }
fn present(s: Option<&str>) -> bool { s.is_some_and(|s| !s.trim().is_empty()) }

impl<'a> Fifo<'a> {
    pub fn new(o: Options<'a>) -> Fifo<'a> {
        Fifo { o, lots: HashMap::new(), in_groups: HashMap::new(), out_groups: HashMap::new(), transferred: HashMap::new(), per_venue: false,
               held: vec![], disposals: vec![], excess_roc_rows: vec![], excess_roc: Dec::zero(), consumed: Dec::zero() }
    }

    /// The lots left standing, per (venue, asset).
    pub fn lots(&self) -> impl Iterator<Item = (&Key, &VecDeque<Lot>)> { self.lots.iter() }

    fn key(&self, venue: &str, asset: &str) -> Key { (self.per_venue.then(|| venue.to_string()), asset.to_string()) }
    fn store(&mut self, venue: &str, asset: &str) -> &mut VecDeque<Lot> { let k = self.key(venue, asset); self.lots.entry(k).or_default() }

    /// `calculate(transactions, **options)`.
    pub fn calculate(&mut self, txs: &[Tx]) -> Result<(), FiguresError> {
        if !self.o.swaps_taxable {
            self.in_groups = self.build_in_groups(txs)?;
            self.out_groups = self.build_out_groups(txs)?;
        }
        for (index, tx) in txs.iter().enumerate() {
            budget::check()?;
            self.enter_row(tx)?;
            let r = &tx.row;
            let (asset, amount, fiat_value) = (r.base.as_str(), &r.amount, &r.fiat_value);
            match r.kind {
                Kind::Buy | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => {
                    let (lot_amount, lot_cost) = self.apply_acquisition_fee(r, amount, fiat_value)?;
                    let lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: r.price_missing,
                                    unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                                    holding_start: None, technical_start: None };
                    self.store(&r.exchange, asset).push_back(lot);
                }
                Kind::Deposit => {
                    if r.linked { continue; }
                    let (lot_amount, lot_cost) = self.apply_acquisition_fee(r, amount, fiat_value)?;
                    let lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: true,
                                    unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                                    holding_start: None, technical_start: None };
                    self.store(&r.exchange, asset).push_back(lot);
                }
                Kind::SwapIn => {
                    if self.non_taxable_stablecoin_swap(asset) { continue; }
                    self.add_swap_in_lot(tx)?;
                }
                Kind::SwapOut => {
                    if self.non_taxable_stablecoin_swap(asset) { continue; }
                    if !self.o.swaps_taxable && !self.fiat_disposal(r) { self.transfer_swap_out(tx)?; }
                    else { self.record_disposal(tx, amount, fiat_value)?; }
                }
                Kind::Sell => self.record_disposal(tx, amount, fiat_value)?,
                Kind::Adjustment => { let at = r.at; let lots = self.store(&r.exchange, asset); apply_split(lots, amount, at)?; }
                Kind::ReturnOfCapital => {
                    let lots = self.key(&r.exchange, asset);
                    let excess = reduce_lot_basis(self.lots.entry(lots).or_default(), r)?;
                    self.excess_roc = (&self.excess_roc + &excess)?;
                    if excess.is_positive() { self.excess_roc_rows.push(index); }
                }
                // D-TAX-COIN-FEE.
                Kind::Fee if self.o.swaps_taxable && !fiat(asset) && amount.is_positive() => self.record_disposal(tx, amount, fiat_value)?,
                Kind::Fee => self.consume_fee_in_kind(&r.exchange, asset, amount)?,
                Kind::Withdrawal => self.withdraw(r)?,
                // D-TAX-LOST.
                Kind::Lost => { let k = self.key(&r.exchange, asset); self.dequeue_cost(&k, amount)?; }
                // withholding_tax and unsupported_activity change no holding.
                _ => {}
            }
        }
        Ok(())
    }

    /// `enter_row`, D-TAX-US-WALLET: before the boundary, what each venue holds; at it, the pool placed on the venues.
    fn enter_row(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let Some(from) = self.o.per_venue_from else { return Ok(()) };
        if self.per_venue { return Ok(()); }
        if tx.row.at.0 < from.0 {
            for (venue, asset, delta) in crate::tracker::walk::located_moves(&tx.row)? {
                let Some(venue) = venue else { continue };
                let k = (venue, asset);
                match self.held.iter_mut().find(|(h, _)| *h == k) {
                    Some((_, q)) => *q = (&*q + &delta)?,
                    None => self.held.push((k, delta)),
                }
            }
            return Ok(());
        }
        self.per_venue = true;
        let pooled: Vec<(Key, VecDeque<Lot>)> = self.lots.drain().collect();
        for ((_, asset), mut lots) in pooled {
            let holders: Vec<(String, Dec)> = self.held.iter().filter(|((_, a), q)| *a == asset && q.is_positive()).map(|((v, _), q)| (v.clone(), q.clone())).collect();
            for (venue, quantity) in holders {
                let (taken, _) = take(&mut lots, &quantity)?;
                self.lots.entry((Some(venue), asset.clone())).or_default().extend(taken);
            }
            // What is left is held off every synced venue (an unlinked withdrawal's coins).
            for l in &lots { self.consumed = (&self.consumed + &(&l.amount * &l.cost_per_unit)?)?; }
        }
        Ok(())
    }

    /// `:withdrawal`: a linked one sheds its network fee at zero; D-TAX-US-WALLET, per venue: a linked one to another venue
    /// carries what arrived there, an unlinked one takes its coins off the venue.
    fn withdraw(&mut self, r: &Row) -> Result<(), FiguresError> {
        let k = self.key(&r.exchange, &r.base);
        if r.linked {
            let fee = r.transfer_fee.clone().filter(Dec::is_positive);
            if let Some(fee) = &fee { self.dequeue_cost(&k, fee)?; }
            let to = r.to_exchange.as_deref().filter(|t| !t.is_empty() && *t != r.exchange);
            if let (true, Some(to)) = (self.per_venue, to) {
                let moved = match &fee { Some(f) => (&r.amount - f)?, None => r.amount.clone() };
                let (taken, _) = take(self.lots.entry(k).or_default(), &moved)?;
                let dest = self.lots.entry((Some(to.to_string()), r.base.clone())).or_default();
                dest.extend(taken);
                dest.make_contiguous().sort_by_key(|l| l.date.0);
            }
        } else if self.per_venue {
            self.dequeue_cost(&k, &r.amount)?;
        }
        Ok(())
    }

    // --- AcquisitionFee ---------------------------------------------------------------------------------------------

    /// `apply_acquisition_fee`: the quantity acquired and its cost, after the fee.
    fn apply_acquisition_fee(&mut self, r: &Row, amount: &Dec, fiat_value: &Dec) -> Result<(Dec, Dec), FiguresError> {
        if present(r.fee_currency.as_deref()) && r.fee_currency.as_deref() == Some(r.base.as_str()) {
            let Some(fee) = &r.fee_amount else { return Ok((amount.clone(), fiat_value.clone())) };
            return Ok((max0((amount - fee)?), fiat_value.clone()));
        }
        if let Some(fee) = r.fee_amount.as_ref().filter(|f| f.is_positive()) {
            if crypto_fee_asset(r) {
                let k = self.key(&r.exchange, r.fee_currency.as_deref().unwrap_or(""));
                self.dequeue_cost(&k, fee)?;
                return Ok((amount.clone(), (fiat_value + &r.fee_fiat_value)?));
            }
        }
        Ok((amount.clone(), (fiat_value + &r.fee_fiat_value)?))
    }

    /// `consume_disposal_fee`.
    fn consume_disposal_fee(&mut self, r: &Row) -> Result<(), FiguresError> {
        let Some(fee) = r.fee_amount.as_ref().filter(|f| f.is_positive()) else { return Ok(()) };
        if r.fee_currency.as_deref() == Some(r.base.as_str()) || !crypto_fee_asset(r) { return Ok(()); }
        let k = self.key(&r.exchange, r.fee_currency.as_deref().unwrap_or(""));
        self.dequeue_cost(&k, fee)?;
        Ok(())
    }

    // --- Fifo -------------------------------------------------------------------------------------------------------

    /// `consume_fee_in_kind`.
    fn consume_fee_in_kind(&mut self, venue: &str, asset: &str, amount: &Dec) -> Result<(), FiguresError> {
        if fiat(asset) || !amount.is_positive() { return Ok(()); }
        let k = self.key(venue, asset);
        if self.lots.get(&k).is_none_or(VecDeque::is_empty) { return Ok(()); }
        self.dequeue_cost(&k, amount)?;
        Ok(())
    }

    fn record_disposal(&mut self, tx: &Tx, amount: &Dec, fiat_value: &Dec) -> Result<(), FiguresError> {
        let r = &tx.row;
        let k = self.key(&r.exchange, &r.base);
        let lots = self.lots.entry(k).or_default();
        let has_lots = !lots.is_empty();
        let (tranches, _) = dequeue_tranches(lots, amount)?;
        self.consume_disposal_fee(r)?;
        if self.o.holding.is_some() { return self.record_disposal_by_term(tx, amount, fiat_value, tranches, has_lots); }
        let uncovered = max0((amount - &sum(tranches.iter().map(|t| &t.amount))?)?);
        let row = self.disposal_row(tx, amount, fiat_value, &r.fee_fiat_value, &tranches, has_lots, uncovered)?;
        self.disposals.push(row);
        Ok(())
    }

    /// `record_disposal_by_term`: one row per term, first met first; uncovered units go with the short side.
    fn record_disposal_by_term(&mut self, tx: &Tx, amount: &Dec, fiat_value: &Dec, tranches: Vec<Tranche>, has_lots: bool) -> Result<(), FiguresError> {
        let rule = self.o.holding.expect("a holding rule");
        let r = &tx.row;
        let mut groups: Vec<(bool, Vec<Tranche>)> = vec![];
        for t in tranches {
            let long = rule(self.holding_ref(&t, tx), r.at);
            match groups.iter_mut().find(|(g, _)| *g == long) { Some((_, g)) => g.push(t), None => groups.push((long, vec![t])) }
        }
        let mut taken = Dec::zero();
        for (_, g) in &groups { taken = (&taken + &sum(g.iter().map(|t| &t.amount))?)?; }
        let uncovered = (amount - &taken)?;
        if (uncovered.is_positive() || groups.is_empty()) && !groups.iter().any(|(l, _)| !l) { groups.push((false, vec![])); }
        let fee = r.fee_fiat_value.clone();
        let mut rows = vec![];
        for (long, group) in &groups {
            let mut group_amount = sum(group.iter().map(|t| &t.amount))?;
            if !long { group_amount = (&group_amount + &uncovered)?; }
            let proceeds = (fiat_value * &group_amount)?.div(amount)?;
            let group_fee = (&fee * &group_amount)?.div(amount)?;
            let short_uncovered = if *long { Dec::zero() } else { max0(uncovered.clone()) };
            let mut row = self.disposal_row(tx, &group_amount, &proceeds, &group_fee, group, has_lots, short_uncovered)?;
            row.long_held = Some(*long);
            rows.push(row);
        }
        if rows.len() > 1 { settle_in_cents(&mut rows, fiat_value, &fee)?; }
        self.disposals.extend(rows);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn disposal_row(&self, tx: &Tx, amount: &Dec, fiat_value: &Dec, fee: &Dec, tranches: &[Tranche], has_lots: bool, uncovered: Dec) -> Result<Disposal, FiguresError> {
        let r = &tx.row;
        let acquired = tranches.first().map(|t| self.holding_ref(t, tx));
        let cost_basis = sum(tranches.iter().map(|t| &t.cost))?;
        let basis_assumed = tranches.iter().any(|t| t.basis_assumed);
        // ((transacted_at - holding_ref_date) / 1.day).to_i: a Float of seconds, truncated.
        let holding_days = acquired.map_or(0, |a| (r.at.minus(a) / 86_400.0) as i64);
        Ok(Disposal {
            at: r.at, acquired, asset: r.base.clone(), amount: amount.clone(), proceeds: fiat_value.clone(), cost_basis: cost_basis.clone(),
            fee: fee.clone(), gain_loss: (&(fiat_value - &cost_basis)? - fee)?, holding_days, complete: has_lots,
            // `data_incomplete?`, and D-TAX-UNCOVERED.
            incomplete: r.price_missing || basis_assumed || !has_lots || uncovered.is_positive(),
            tx_id: tx.tx_id.clone(), exchange: r.exchange.clone(), long_held: None, uncovered,
        })
    }

    /// `holding_ref`.
    fn holding_ref(&self, t: &Tranche, tx: &Tx) -> At {
        if self.immediate_fiat_exit(t, tx) { if let Some(s) = t.technical_start { return s; } }
        t.holding_start.unwrap_or(t.date)
    }

    /// `immediate_fiat_exit?`: within 24 hours of the swap, by the clock, for fiat (D-TAX-PT-FIAT-EXIT: `Tx::fiat_exit`).
    fn immediate_fiat_exit(&self, t: &Tranche, tx: &Tx) -> bool {
        let (Some(_), Some(start)) = (t.technical_start, t.holding_start) else { return false };
        tx.fiat_exit && tx.row.at.minus(start) <= IMMEDIATE_EXIT
    }

    fn add_swap_in_lot(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        if self.o.swaps_taxable {
            let (lot_amount, lot_cost) = self.apply_acquisition_fee(r, &r.amount, &r.fiat_value)?;
            let lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: r.price_missing,
                            unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                            holding_start: None, technical_start: None };
            self.store(&r.exchange, &r.base).push_back(lot);
            return Ok(());
        }
        self.add_non_taxable_swap_in_lots(tx)
    }

    /// `add_non_taxable_swap_in_lots`: the chain across a group of any shape.
    fn add_non_taxable_swap_in_lots(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        let key = group_key(r);
        let tranches: Vec<Tranche> = key.as_ref().and_then(|k| self.transferred.get(k)).cloned().unwrap_or_default();
        let (cash_present, cash) = self.swap_cash_cost(r)?;
        let (lot_amount, paid) = self.apply_acquisition_fee(r, &r.amount, &cash)?;
        let fee_cost = (&paid - &cash)?;
        let (share, share_assumed) = self.swap_in_share(r);
        let assumed = r.price_missing || share_assumed;
        if tranches.is_empty() && !cash_present {
            let unpriced = if value_missing(r) { lot_amount.clone() } else { Dec::zero() };
            let cpu = unit(&(&r.fiat_value + &fee_cost)?, &lot_amount)?;
            self.open_swap_lot(tx, lot_amount, cpu, r.at, assumed, None, unpriced);
            return Ok(());
        }
        let transferred_cost = (&sum(tranches.iter().map(|t| &t.cost))? * &share)?;
        let coin_value = (&self.swap_out_value(key.as_ref()) * &share)?;
        let cash_amount = cash_lot_amount(&lot_amount, &cash, &coin_value, &transferred_cost, &tranches)?;
        let rest = (&lot_amount - &cash_amount)?;
        let total_weight = sum(tranches.iter().map(|t| &t.weight))?;
        let mut allocated = Dec::zero();
        for (i, t) in tranches.iter().enumerate() {
            let tranche_amount = if i == tranches.len() - 1 { (&rest - &allocated)? } else { (&rest * &t.weight)?.div(&total_weight)? };
            allocated = (&allocated + &tranche_amount)?;
            if !tranche_amount.is_positive() { continue; }
            let carried = (&(&t.cost * &share)? * &total_weight)?.div(&t.weight)?;
            let fee_part = (&fee_cost * &t.weight)?.div(&total_weight)?;
            let cost = (&carried + &fee_part)?;
            let unpriced = if t.amount.is_positive() { (&tranche_amount * &t.unpriced)?.div(&t.amount)? } else { Dec::zero() };
            let cpu = cost.div(&rest)?;
            self.open_swap_lot(tx, tranche_amount, cpu, t.date, t.basis_assumed || assumed, t.holding_start, unpriced);
        }
        if cash_amount.is_positive() {
            let scale = if tranches.is_empty() { fee_cost.clone() } else if coin_value.is_positive() { coin_value.clone() } else { transferred_cost.clone() };
            let cpu = unit(&(&cash + &scale)?, &lot_amount)?;
            let lot_assumed = assumed || (rest.is_zero() && tranches.iter().any(|t| t.basis_assumed));
            self.open_swap_lot(tx, cash_amount, cpu, r.at, lot_assumed, None, Dec::zero());
        }
        // A tranche can predate everything already held, and the lots are read in order.
        self.store(&r.exchange, &r.base).make_contiguous().sort_by_key(|l| l.date.0);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn open_swap_lot(&mut self, tx: &Tx, amount: Dec, cost_per_unit: Dec, date: At, basis_assumed: bool, holding_start: Option<At>, unpriced: Dec) {
        let r = &tx.row;
        let mut lot = Lot { amount, cost_per_unit, date, basis_assumed, unpriced, holding_start: None, technical_start: None };
        if self.o.swap_resets_holding_period {
            lot.holding_start = Some(r.at);
            let held_since = holding_start.unwrap_or(date);
            if self.technical_stablecoin_swap(r, held_since) { lot.technical_start = Some(held_since); }
        } else if holding_start.is_some() {
            lot.holding_start = holding_start;
        }
        self.store(&r.exchange, &r.base).push_back(lot);
    }

    /// `technical_stablecoin_swap?`.
    fn technical_stablecoin_swap(&self, r: &Row, held_since: At) -> bool {
        stable(&r.base) && self.o.holding.is_some_and(|rule| rule(held_since, r.at))
    }

    /// `transfer_swap_out`: the lots an out-leg took handed to its group as tranches, weighted by the leg's worth.
    fn transfer_swap_out(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        let k = self.key(&r.exchange, &r.base);
        let (mut tranches, held) = dequeue_tranches(self.lots.entry(k).or_default(), &r.amount)?;
        let Some(key) = group_key(r) else { return Ok(()) };
        let covered = if held < r.amount { held } else { r.amount.clone() };
        let uncovered = (&r.amount - &covered)?;
        if uncovered.is_positive() {
            tranches.push(Tranche { amount: uncovered.clone(), cost: Dec::zero(), date: r.at, basis_assumed: true, unpriced: uncovered,
                                    holding_start: None, technical_start: None, weight: Dec::zero() });
        }
        let weight = self.out_groups.get(&key).and_then(|g| g.weights.get(&r.uid)).cloned()
            .ok_or_else(|| FiguresError::Data("a swap out-leg outside its group".into()))?;
        let handed = sum(tranches.iter().map(|t| &t.amount))?;
        for t in &mut tranches { t.weight = (&weight * &t.amount)?.div(&handed)?; }
        self.transferred.entry(key).or_default().extend(tranches);
        Ok(())
    }

    /// `build_swap_in_groups`.
    fn build_in_groups(&self, txs: &[Tx]) -> Result<HashMap<Group, InGroup>, FiguresError> {
        let mut out = HashMap::new();
        for (key, rows) in self.group_legs(txs, Kind::SwapIn) {
            let first = &rows[0].base;
            let one_asset = rows.iter().all(|r| r.base == *first);
            let weights: Vec<Dec> = rows.iter().map(|r| if one_asset { r.amount.clone() } else { r.fiat_value.clone() }).collect();
            let equal = weights.iter().any(Dec::is_zero);
            let shares = allocate_shares(&if equal { vec![Dec::one(); rows.len()] } else { weights })?;
            out.insert(key, InGroup { shares: rows.iter().map(|r| r.uid).zip(shares).collect(), basis_assumed: equal });
        }
        Ok(out)
    }

    /// `build_swap_out_groups`.
    fn build_out_groups(&self, txs: &[Tx]) -> Result<HashMap<Group, OutGroup>, FiguresError> {
        let mut out = HashMap::new();
        for (key, rows) in self.group_legs(txs, Kind::SwapOut) {
            let values: Vec<Dec> = rows.iter().map(|r| r.fiat_value.clone()).collect();
            let value = sum(values.iter())?;
            let weights = if values.iter().any(Dec::is_zero) { vec![Dec::one(); rows.len()] } else { values };
            out.insert(key, OutGroup { weights: rows.iter().map(|r| r.uid).zip(weights).collect(), value });
        }
        Ok(out)
    }

    /// `group_legs`.
    fn group_legs<'t>(&self, txs: &'t [Tx], kind: Kind) -> Vec<(Group, Vec<&'t Row>)> {
        let mut groups: Vec<(Group, Vec<&Row>)> = vec![];
        for r in txs.iter().map(|t| &t.row) {
            if r.kind != kind || self.non_taxable_stablecoin_swap(&r.base) { continue; }
            if kind == Kind::SwapOut && self.fiat_disposal(r) { continue; }
            let Some(key) = group_key(r) else { continue };
            match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, g)) => g.push(r), None => groups.push((key, vec![r])) }
        }
        groups
    }

    fn swap_in_share(&self, r: &Row) -> (Dec, bool) {
        match group_key(r).and_then(|k| self.in_groups.get(&k)) {
            Some(g) => (g.shares.get(&r.uid).cloned().unwrap_or_else(Dec::one), g.basis_assumed),
            None => (Dec::one(), false),
        }
    }

    fn swap_out_value(&self, key: Option<&Group>) -> Dec { key.and_then(|k| self.out_groups.get(k)).map_or_else(Dec::zero, |g| g.value.clone()) }

    /// `swap_cash_cost`: whether the group spent cash beside the coins, and how much (stablecoins only under the flag).
    fn swap_cash_cost(&self, r: &Row) -> Result<(bool, Dec), FiguresError> {
        let mut present = r.swap_fiat_cost.is_some();
        let mut cost = r.swap_fiat_cost.clone().unwrap_or_else(Dec::zero);
        if self.o.stablecoin_as_fiat {
            if let Some(s) = &r.swap_stable_cost { present = true; cost = (&cost + s)?; }
        }
        Ok((present, cost))
    }

    fn non_taxable_stablecoin_swap(&self, asset: &str) -> bool { !self.o.swaps_taxable && self.o.stablecoin_as_fiat && stable(asset) }

    /// `fiat_disposal?`.
    fn fiat_disposal(&self, r: &Row) -> bool {
        let Some(quote) = r.quote.as_deref().filter(|q| !q.trim().is_empty()) else { return false };
        fiat(quote) || (self.o.stablecoin_as_fiat && stable(quote))
    }

    /// `dequeue_cost`: the lots taken at zero, their basis gone.
    fn dequeue_cost(&mut self, k: &Key, amount: &Dec) -> Result<(), FiguresError> {
        let (tranches, _) = dequeue_tranches(self.lots.entry(k.clone()).or_default(), amount)?;
        self.consumed = (&self.consumed + &sum(tranches.iter().map(|t| &t.cost))?)?;
        Ok(())
    }
}

fn value_missing(r: &Row) -> bool { r.value_missing.unwrap_or(r.price_missing) }

fn group_key(r: &Row) -> Option<Group> { r.group.as_ref().map(|g| (r.exchange.clone(), g.clone())) }

/// `crypto_fee_asset?`: a fee in neither fiat nor the row's own quote.
fn crypto_fee_asset(r: &Row) -> bool {
    let fc = r.fee_currency.as_deref();
    present(fc) && fc != r.quote.as_deref() && !fc.is_some_and(fiat)
}

/// `cash_lot_amount`.
fn cash_lot_amount(lot_amount: &Dec, cash: &Dec, coin_value: &Dec, transferred_cost: &Dec, tranches: &[Tranche]) -> Result<Dec, FiguresError> {
    if tranches.is_empty() { return Ok(lot_amount.clone()); }
    if !cash.is_positive() { return Ok(Dec::zero()); }
    let scale = if coin_value.is_positive() { coin_value } else { transferred_cost };
    let whole = (cash + scale)?;
    Ok(if whole.is_positive() { (lot_amount * cash)?.div(&whole)? } else { lot_amount.clone() })
}

/// `allocate_shares`: the last share is what is left.
fn allocate_shares(weights: &[Dec]) -> Result<Vec<Dec>, FiguresError> {
    let total = sum(weights.iter())?;
    let mut allocated = Dec::zero();
    let mut out = vec![];
    for (i, w) in weights.iter().enumerate() {
        let share = if i == weights.len() - 1 { (&Dec::one() - &allocated)? } else { w.div(&total)? };
        allocated = (&allocated + &share)?;
        out.push(share);
    }
    Ok(out)
}

/// `apply_split`: every lot keeps its total cost and its date.
fn apply_split(lots: &mut VecDeque<Lot>, amount: &Dec, at: At) -> Result<(), FiguresError> {
    let pool = sum(lots.iter().map(|l| &l.amount))?;
    let factor = if pool.is_positive() { (&pool + amount)?.div(&pool)? } else { Dec::zero() };
    if !amount.is_zero() && pool.is_positive() && factor.is_positive() {
        for l in lots.iter_mut() {
            l.cost_per_unit = l.cost_per_unit.div(&factor)?;
            l.amount = (&l.amount * &factor)?;
            l.unpriced = (&l.unpriced * &factor)?;
        }
    } else if amount.is_negative() && !factor.is_positive() {
        lots.clear();
    } else if amount.is_positive() && pool.is_zero() {
        lots.push_back(Lot { amount: amount.clone(), cost_per_unit: Dec::zero(), date: at, basis_assumed: true, unpriced: amount.clone(),
                             holding_start: None, technical_start: None });
    }
    Ok(())
}

/// ReturnOfCapital#reduce_lot_basis: per share when the row says, else FIFO by money; returns the excess.
fn reduce_lot_basis(lots: &mut VecDeque<Lot>, r: &Row) -> Result<Dec, FiguresError> {
    if let (Some(per_unit), false) = (roc_per_unit(r)?, lots.is_empty()) {
        let mut excess = Dec::zero();
        for l in lots.iter_mut() {
            excess = (&excess + &(&l.amount * &max0((&per_unit - &l.cost_per_unit)?))?)?;
            l.cost_per_unit = max0((&l.cost_per_unit - &per_unit)?);
        }
        return Ok(excess);
    }
    let mut remaining = r.fiat_value.clone();
    for l in lots.iter_mut() {
        if !remaining.is_positive() { break; }
        let lot_cost = (&l.amount * &l.cost_per_unit)?;
        let take = if remaining < lot_cost { remaining.clone() } else { lot_cost.clone() };
        l.cost_per_unit = if l.amount.is_positive() { (&lot_cost - &take)?.div(&l.amount)? } else { Dec::zero() };
        remaining = (&remaining - &take)?;
    }
    Ok(remaining)
}

/// `roc_per_unit`: Alpaca's per-share rate times the row's own FX multiplier.
fn roc_per_unit(r: &Row) -> Result<Option<Dec>, FiguresError> {
    let Some(per_share) = r.per_share.as_ref().filter(|p| p.is_positive()) else { return Ok(None) };
    let multiplier = match r.quote_amount.as_ref().filter(|q| !q.is_zero()) { Some(q) => r.fiat_value.div(q)?, None => Dec::one() };
    Ok(Some((per_share * &multiplier)?))
}

/// `settle_in_cents`: every row but the last cut to the cent, the last taking what is left.
fn settle_in_cents(rows: &mut [Disposal], proceeds: &Dec, fee: &Dec) -> Result<(), FiguresError> {
    let basis = sum(rows.iter().map(|r| &r.cost_basis))?;
    let mut left = [proceeds.clone(), fee.clone(), basis.clone(), (&(proceeds - &basis)? - fee)?.round(CENTS)?];
    let count = rows.len();
    for (n, row) in rows.iter_mut().enumerate() {
        if n == count - 1 {
            [row.proceeds, row.fee, row.cost_basis, row.gain_loss] = left.clone();
        } else {
            row.proceeds = row.proceeds.round(CENTS)?; row.fee = row.fee.round(CENTS)?; row.cost_basis = row.cost_basis.round(CENTS)?;
            row.gain_loss = (&(&row.proceeds - &row.cost_basis)? - &row.fee)?;
        }
        let taken = [&row.proceeds, &row.fee, &row.cost_basis, &row.gain_loss];
        for (l, t) in left.iter_mut().zip(taken) { *l = (&*l - t)?; }
    }
    Ok(())
}

/// The lots that make up `amount`, oldest first, split where a lot is only partly taken (its unpriced units in
/// proportion), and what was held before.
fn take(lots: &mut VecDeque<Lot>, amount: &Dec) -> Result<(Vec<Lot>, Dec), FiguresError> {
    let held = sum(lots.iter().map(|l| &l.amount))?;
    let mut remaining = amount.clone();
    let mut out = vec![];
    while remaining.is_positive() {
        budget::check()?;
        let Some(lot) = lots.front_mut() else { break };
        let part = if remaining < lot.amount { remaining.clone() } else { lot.amount.clone() };
        let unpriced = if lot.amount.is_positive() { (&lot.unpriced * &part)?.div(&lot.amount)? } else { Dec::zero() };
        out.push(Lot { amount: part, unpriced: unpriced.clone(), ..lot.clone() });
        if lot.amount <= remaining {
            remaining = (&remaining - &lot.amount)?;
            lots.pop_front();
        } else {
            lot.amount = (&lot.amount - &remaining)?;
            lot.unpriced = (&lot.unpriced - &unpriced)?;
            remaining = Dec::zero();
        }
    }
    Ok((out, held))
}

/// `dequeue_tranches`.
fn dequeue_tranches(lots: &mut VecDeque<Lot>, amount: &Dec) -> Result<(Vec<Tranche>, Dec), FiguresError> {
    let (taken, held) = take(lots, amount)?;
    let tranches = taken.into_iter().map(|l| Ok(Tranche { cost: (&l.amount * &l.cost_per_unit)?, amount: l.amount, date: l.date,
                                                         basis_assumed: l.basis_assumed, unpriced: l.unpriced, holding_start: l.holding_start,
                                                         technical_start: l.technical_start, weight: Dec::zero() }))
        .collect::<Result<_, FiguresError>>()?;
    Ok((tranches, held))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec { Dec::parse(s).unwrap() }
    fn at(day: i64) -> At { At(1_700_000_000_000_000_000 + day * 86_400_000_000_000) }

    fn row(uid: usize, kind: Kind, base: &str, amount: &str, value: &str, day: i64, venue: &str) -> Tx {
        Tx { row: Row { uid, kind, base: base.into(), amount: d(amount), quote: Some("USD".into()), quote_amount: None, fiat_value: d(value),
                        fee_fiat_value: Dec::zero(), fee_currency: None, fee_amount: None, at: at(day), group: None, price_missing: false,
                        value_missing: Some(false), exchange: venue.into(), linked: false, transfer_fee: None, swap_fiat_cost: None,
                        swap_stable_cost: None, per_share: None, to_exchange: None, orphan: false, opening: false, stated_value: false },
             tx_id: None, fiat_exit: true }
    }

    fn remaining(f: &Fifo) -> Dec {
        let mut t = Dec::zero();
        for (_, lots) in f.lots() { for l in lots { t = (&t + &(&l.amount * &l.cost_per_unit).unwrap()).unwrap(); } }
        t
    }

    /// Σ basis used + basis left = Σ cost acquired, through sales (whole, partial, oversold), fees in kind and in a third
    /// coin, a lost coin, transfers, and the per-venue boundary.
    #[test]
    fn basis_is_conserved() {
        let cases: [(bool, Option<At>); 3] = [(true, None), (false, None), (true, Some(at(5)))];
        for (taxable, per_venue_from) in cases {
            let mut rows = vec![
                row(0, Kind::Buy, "BTC", "1", "300", 0, "a"), row(1, Kind::Buy, "BTC", "3", "1000", 1, "b"), row(2, Kind::Buy, "ETH", "7", "70", 2, "a"),
                row(3, Kind::Sell, "BTC", "0.7", "900", 3, "a"), row(4, Kind::Fee, "BTC", "0.01", "5", 4, "b"), row(5, Kind::Lost, "ETH", "1", "0", 6, "a"),
                row(6, Kind::Buy, "BTC", "2", "2500", 7, "a"), row(7, Kind::Sell, "BTC", "4", "9000", 8, "b"), row(8, Kind::Sell, "BTC", "3", "9000", 9, "a"),
                row(9, Kind::Withdrawal, "ETH", "2", "0", 10, "a"),
            ];
            rows[6].row.fee_currency = Some("ETH".into());
            rows[6].row.fee_amount = Some(d("0.5"));
            rows[6].row.fee_fiat_value = d("7");
            let acquired = sum([d("300"), d("1000"), d("70"), d("2507")].iter()).unwrap();
            let holding = |_: At, _: At| false;
            let mut f = Fifo::new(Options { swaps_taxable: taxable, stablecoin_as_fiat: false, holding: Some(&holding), swap_resets_holding_period: false, per_venue_from });
            f.calculate(&rows).unwrap();
            let used = sum(f.disposals.iter().map(|x| &x.cost_basis)).unwrap();
            let total = (&(&used + &f.consumed).unwrap() + &remaining(&f)).unwrap();
            let gap = (&total - &acquired).unwrap();
            assert!(gap.round(9).unwrap().is_zero(), "taxable {taxable}, per venue {per_venue_from:?}: {} off", gap.to_s_f());
        }
    }

    #[test]
    fn a_lost_coin_takes_its_lots_with_no_row_and_an_oversold_sale_is_incomplete() {
        let rows = [row(0, Kind::Buy, "BTC", "1", "100", 0, "a"), row(1, Kind::Buy, "BTC", "1", "500", 1, "a"), row(2, Kind::Lost, "BTC", "1", "0", 2, "a"),
                    row(3, Kind::Sell, "BTC", "1.5", "900", 3, "a")];
        let mut f = Fifo::new(Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None });
        f.calculate(&rows).unwrap();
        assert_eq!(f.disposals.len(), 1);
        let x = &f.disposals[0];
        assert_eq!((x.cost_basis.to_s_f(), x.uncovered.to_s_f(), x.complete, x.incomplete), ("500.0".into(), "0.5".into(), true, true));
        assert_eq!(f.consumed.to_s_f(), "100.0");
    }
}
