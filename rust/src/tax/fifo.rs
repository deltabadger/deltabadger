//! Tax::Methods::Fifo with AcquisitionFee and ReturnOfCapital (app/models/tax/methods/{fifo,acquisition_fee,
//! return_of_capital}.rb), method by method, over the rows Tax::Report hands it, with Report#calculation_options.
//! LIFO and the 4-week rule (A12-4) hang on `take`.
//!
//! Where Rust departs from Rails, by name:
//! - D-TAX-ASSET-IDENTITY: lots are kept per coin (`Tx::asset`, the coin the symbol means on its venue that day), not
//!   per ticker, so two coins sharing a symbol never take each other's basis.
//! - D-TAX-UNCOVERED: a disposal some unit of which no lot covered is `data_incomplete` and not `cost_basis_complete`,
//!   its uncovered quantity kept for the report's warning block (Rails marks it complete when any lot existed).
//! - D-TAX-LOST: a `lost` row takes its lots in FIFO order with no disposal row (Rails has no branch, and sells the lost
//!   coins again later at their old basis).
//! - D-TAX-COIN-FEE: where spending a coin is a disposal (`crypto_to_crypto_taxable`), every fee paid in a coin is one:
//!   a fee of a row of its own, a fee in a third coin on a trade, and a linked transfer's network fee are each a
//!   disposal of that coin at its value, its basis from the lots, no fee column. Elsewhere their lots go at zero, as Rails,
//!   and so do a non-taxable swap out-leg's (Rails never took those).
//! - D-TAX-SWAP-FEE: a fee on a swap-in that carries several tranches joins the cost once (Rails weighted it twice, so
//!   two equal tranches carried half of it).
//! - D-TAX-US-SWAP-COST: US, from 2025 (Treas. Reg. 1.1012-1(h)(2)): the cost of a coin-for-coin exchange belongs to the
//!   coin given up; a fee on the in-leg comes off the out-leg's proceeds instead of joining the new coin's basis.
//! - D-TAX-ORPHAN-SWAP: where swaps are not taxable, a swap-in with nothing to carry opens at market value marked assumed.
//! - D-TAX-US-WALLET (`Options::per_venue_from`): from the start of 2025 in the user's zone, lots per account (one per
//!   venue in this app: one key of each type per user and exchange). Before it one pool, as Rails; at the boundary the
//!   pool as it stands is placed on the venues that hold the units then, the oldest lots to the venue first seen, and the
//!   placement is kept for the report (`allocation`), so no lot an earlier year's report used is used again. A linked
//!   transfer between venues carries its lots; an unlinked withdrawal takes them off the venue.
use crate::figures::{at::At, budget, dec::Dec, FiguresError};
use crate::tracker::rows::Kind;
use crate::tracker::walk::Row;
use crate::tracker::{fiat, stable};
use std::collections::{HashMap, VecDeque};

/// Tax::Methods::Fifo::IMMEDIATE_EXIT, in seconds.
const IMMEDIATE_EXIT: f64 = 24.0 * 3600.0;
/// Fifo::CENTS.
const CENTS: i64 = 2;

/// A row as the engine reads it: the enriched row, whether it leaves for fiat (`fiat_exit?`), and the identity of its
/// base and fee coins (D-TAX-ASSET-IDENTITY).
pub struct Tx { pub row: Row, pub tx_id: Option<String>, pub fiat_exit: bool, pub asset: String, pub fee_asset: Option<String>,
                /// Whether the row's group has a fiat leg (fiat rows reach no engine).
                pub cash_group: bool }

/// Report#calculation_options for this engine.
pub struct Options<'a> {
    pub swaps_taxable: bool,
    pub stablecoin_as_fiat: bool,
    /// `holding_rule`: (acquired_at, disposed_at) → held long.
    pub holding: Option<&'a dyn Fn(At, At) -> bool>,
    pub swap_resets_holding_period: bool,
    /// D-TAX-US-WALLET: the instant lots start being kept per venue.
    pub per_venue_from: Option<At>,
    /// D-TAX-US-SWAP-COST: the instant a swap's cost starts belonging to the coin given up.
    pub swap_cost_to_disposed_from: Option<At>,
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

/// D-TAX-US-WALLET: one lot as placed at the boundary (`venue` None: held on no synced venue).
#[derive(Clone, Debug)]
pub struct Placed { pub venue: Option<String>, pub symbol: String, pub amount: Dec, pub cost: Dec, pub acquired: At }

type Key = (Option<String>, String);
type Group = (String, String);
struct InGroup { shares: HashMap<usize, Dec>, basis_assumed: bool }
struct OutGroup { weights: HashMap<usize, Dec>, value: Dec }

/// What one disposal sells: which coin (its identity and its symbol), how much, for what, and whether its value is known.
struct Sale<'s> { asset: &'s str, symbol: &'s str, amount: Dec, proceeds: Dec, fee: Dec, price_missing: bool }

pub struct Fifo<'a> {
    o: Options<'a>,
    lots: HashMap<Key, VecDeque<Lot>>,
    in_groups: HashMap<Group, InGroup>,
    out_groups: HashMap<Group, OutGroup>,
    transferred: HashMap<Group, Vec<Tranche>>,
    /// D-TAX-US-SWAP-COST: the in-legs whose fee moved, and what each out-leg's proceeds bear of it.
    moved_in: HashMap<usize, Dec>,
    moved_out: HashMap<usize, Dec>,
    per_venue: bool,
    /// D-TAX-US-WALLET, before the boundary: what each venue holds of each coin, in the order first touched.
    held: Vec<((String, String), Dec)>,
    symbols: HashMap<String, String>,
    pub disposals: Vec<Disposal>,
    /// `excess_roc_rows`, as indexes into the rows.
    pub excess_roc_rows: Vec<usize>,
    pub excess_roc: Dec,
    /// Basis that left the lots with no disposal row: fees in kind, transfer fees, lost coins, coins off a venue.
    pub consumed: Dec,
    /// D-TAX-US-WALLET: the lots as placed at the boundary, and whether the placement was a choice (a coin held on more
    /// than one venue, or off every synced one).
    pub allocation: Vec<Placed>,
    pub allocation_chosen: bool,
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
        Fifo { o, lots: HashMap::new(), in_groups: HashMap::new(), out_groups: HashMap::new(), transferred: HashMap::new(),
               moved_in: HashMap::new(), moved_out: HashMap::new(), per_venue: false, held: vec![], symbols: HashMap::new(), disposals: vec![],
               excess_roc_rows: vec![], excess_roc: Dec::zero(), consumed: Dec::zero(), allocation: vec![], allocation_chosen: false }
    }

    /// The lots left standing, per (venue, coin).
    pub fn lots(&self) -> impl Iterator<Item = (&Key, &VecDeque<Lot>)> { self.lots.iter() }

    fn key(&self, venue: &str, asset: &str) -> Key { (self.per_venue.then(|| venue.to_string()), asset.to_string()) }
    fn store(&mut self, venue: &str, asset: &str) -> &mut VecDeque<Lot> { let k = self.key(venue, asset); self.lots.entry(k).or_default() }

    /// `calculate(transactions, **options)`.
    pub fn calculate(&mut self, txs: &[Tx]) -> Result<(), FiguresError> {
        for tx in txs {
            self.symbols.entry(tx.asset.clone()).or_insert_with(|| tx.row.base.clone());
            if let (Some(a), Some(s)) = (&tx.fee_asset, &tx.row.fee_currency) { self.symbols.entry(a.clone()).or_insert_with(|| s.clone()); }
        }
        if !self.o.swaps_taxable {
            self.in_groups = self.build_in_groups(txs)?;
            self.out_groups = self.build_out_groups(txs)?;
        }
        self.move_swap_costs(txs)?;
        for (index, tx) in txs.iter().enumerate() {
            budget::check()?;
            self.enter_row(tx)?;
            let r = &tx.row;
            let (asset, amount, fiat_value) = (tx.asset.as_str(), &r.amount, &r.fiat_value);
            match r.kind {
                Kind::Buy | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => {
                    let (lot_amount, lot_cost) = self.apply_acquisition_fee(tx, amount, fiat_value)?;
                    let lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: r.price_missing,
                                    unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                                    holding_start: None, technical_start: None };
                    self.store(&r.exchange, asset).push_back(lot);
                }
                Kind::Deposit => {
                    if r.linked { continue; }
                    let (lot_amount, lot_cost) = self.apply_acquisition_fee(tx, amount, fiat_value)?;
                    let lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: true,
                                    unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                                    holding_start: None, technical_start: None };
                    self.store(&r.exchange, asset).push_back(lot);
                }
                Kind::SwapIn => {
                    if self.non_taxable_stablecoin_swap(&r.base) { continue; }
                    self.add_swap_in_lot(tx)?;
                }
                Kind::SwapOut => {
                    if self.non_taxable_stablecoin_swap(&r.base) { continue; }
                    if !self.o.swaps_taxable && !self.fiat_disposal(r) { self.transfer_swap_out(tx)?; }
                    else { self.sell(tx)?; }
                }
                Kind::Sell => self.sell(tx)?,
                Kind::Adjustment => { let at = r.at; let lots = self.store(&r.exchange, asset); apply_split(lots, amount, at)?; }
                Kind::ReturnOfCapital => {
                    let lots = self.key(&r.exchange, asset);
                    let excess = reduce_lot_basis(self.lots.entry(lots).or_default(), r)?;
                    self.excess_roc = (&self.excess_roc + &excess)?;
                    if excess.is_positive() { self.excess_roc_rows.push(index); }
                }
                // D-TAX-COIN-FEE.
                Kind::Fee if self.o.swaps_taxable && !fiat(&r.base) && amount.is_positive() => {
                    let sale = Sale { asset, symbol: &r.base, amount: amount.clone(), proceeds: fiat_value.clone(), fee: r.fee_fiat_value.clone(),
                                      price_missing: r.price_missing };
                    self.dispose(tx, sale, true)?;
                }
                Kind::Fee => self.consume_fee_in_kind(&r.exchange, &r.base, asset, amount)?,
                Kind::Withdrawal => self.withdraw(tx)?,
                // D-TAX-LOST.
                Kind::Lost => { let k = self.key(&r.exchange, asset); self.dequeue_cost(&k, amount)?; }
                // withholding_tax and unsupported_activity change no holding.
                _ => {}
            }
        }
        if self.o.per_venue_from.is_some() && !self.per_venue { self.place()?; }
        Ok(())
    }

    /// D-TAX-US-SWAP-COST: per group of coin legs only (a swap, or a coin-for-stablecoin trade booked as buy and sell)
    /// from the instant, the in-legs' fees come off the out-legs' proceeds, shared by the out-legs' worth (evenly where
    /// one is unpriced), the last taking what is left. A fee in the received coin is valued at what those units were
    /// worth, and the net units keep only the rest of the value as their basis.
    fn move_swap_costs(&mut self, txs: &[Tx]) -> Result<(), FiguresError> {
        let Some(from) = self.o.swap_cost_to_disposed_from.filter(|_| self.o.swaps_taxable) else { return Ok(()) };
        let mut groups: HashMap<Group, (Vec<&Row>, Vec<&Row>, bool)> = HashMap::new();
        for t in txs.iter().filter(|t| t.row.at.0 >= from.0) {
            let r = &t.row;
            let Some(k) = group_key(r) else { continue };
            let g = groups.entry(k).or_default();
            if t.cash_group || fiat(&r.base) { g.2 = true; }
            if r.kind.in_leg() { g.0.push(r); } else if r.kind.out_leg() { g.1.push(r); }
        }
        for (ins, outs, cash) in groups.values() {
            if outs.is_empty() || *cash { continue; }
            let mut moving: Vec<(usize, Dec)> = vec![];
            for r in ins {
                let in_base = r.fee_currency.as_deref() == Some(r.base.as_str());
                let value = match (&r.fee_amount, in_base) {
                    (Some(fee), true) if fee.is_positive() && r.amount.is_positive() => (&r.fiat_value * fee)?.div(&r.amount)?,
                    (_, false) => r.fee_fiat_value.clone(),
                    _ => Dec::zero(),
                };
                if value.is_positive() { moving.push((r.uid, value)); }
            }
            if moving.is_empty() { continue; }
            let total = sum(moving.iter().map(|(_, v)| v))?;
            let values: Vec<Dec> = outs.iter().map(|r| r.fiat_value.clone()).collect();
            let shares = allocate_shares(&if values.iter().any(Dec::is_zero) { vec![Dec::one(); outs.len()] } else { values })?;
            for (r, share) in outs.iter().zip(shares) { self.moved_out.insert(r.uid, (&total * &share)?); }
            self.moved_in.extend(moving);
        }
        Ok(())
    }

    /// `enter_row`, D-TAX-US-WALLET: before the boundary, what each venue holds; at it, the pool placed on the venues.
    fn enter_row(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let Some(from) = self.o.per_venue_from else { return Ok(()) };
        if self.per_venue { return Ok(()); }
        if tx.row.at.0 < from.0 {
            for (venue, symbol, delta) in crate::tracker::walk::located_moves(&tx.row)? {
                let Some(venue) = venue else { continue };
                let asset = if symbol == tx.row.base { tx.asset.clone() } else if tx.row.fee_currency.as_deref() == Some(symbol.as_str()) {
                    tx.fee_asset.clone().unwrap_or(symbol) } else { symbol };
                let k = (venue, asset);
                match self.held.iter_mut().find(|(h, _)| *h == k) {
                    Some((_, q)) => *q = (&*q + &delta)?,
                    None => self.held.push((k, delta)),
                }
            }
            return Ok(());
        }
        self.place()
    }

    /// D-TAX-US-WALLET: the pool placed on the venues holding the units (at the first row from the boundary, or at the
    /// end of a history that stops before it).
    fn place(&mut self) -> Result<(), FiguresError> {
        self.per_venue = true;
        let mut pooled: Vec<(Key, VecDeque<Lot>)> = self.lots.drain().filter(|(_, l)| !l.is_empty()).collect();
        pooled.sort_by(|a, b| a.0.1.cmp(&b.0.1));
        for ((_, asset), mut lots) in pooled {
            let symbol = self.symbols.get(&asset).cloned().unwrap_or_else(|| asset.clone());
            let holders: Vec<(String, Dec)> = self.held.iter().filter(|((_, a), q)| *a == asset && q.is_positive()).map(|((v, _), q)| (v.clone(), q.clone())).collect();
            if holders.len() > 1 { self.allocation_chosen = true; }
            for (venue, quantity) in holders {
                let (taken, _) = take(&mut lots, &quantity)?;
                for l in &taken {
                    self.allocation.push(Placed { venue: Some(venue.clone()), symbol: symbol.clone(), amount: l.amount.clone(), cost: (&l.amount * &l.cost_per_unit)?, acquired: l.date });
                }
                self.lots.entry((Some(venue), asset.clone())).or_default().extend(taken);
            }
            // What is left is held off every synced venue (an unlinked withdrawal's coins): out of the synced lots, listed.
            for l in &lots {
                self.allocation_chosen = true;
                let cost = (&l.amount * &l.cost_per_unit)?;
                self.consumed = (&self.consumed + &cost)?;
                self.allocation.push(Placed { venue: None, symbol: symbol.clone(), amount: l.amount.clone(), cost, acquired: l.date });
            }
        }
        Ok(())
    }

    /// `:withdrawal`: a linked one sheds its network fee (D-TAX-COIN-FEE: as a disposal where spending a coin is one); per
    /// venue (D-TAX-US-WALLET), a linked one to another venue carries what arrived there, an unlinked one takes its coins
    /// off the venue.
    fn withdraw(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        if r.linked {
            let fee = r.transfer_fee.clone().filter(Dec::is_positive);
            if let Some(fee) = &fee {
                if self.o.swaps_taxable {
                    // The withdrawal's value is read without warnings (a transfer needs no price): an unpriced one is flagged here.
                    let proceeds = if r.amount.is_positive() { (&r.fiat_value * fee)?.div(&r.amount)? } else { Dec::zero() };
                    let sale = Sale { asset: &tx.asset, symbol: &r.base, amount: fee.clone(), proceeds, fee: Dec::zero(), price_missing: r.fiat_value.is_zero() };
                    self.dispose(tx, sale, false)?;
                } else {
                    let k = self.key(&r.exchange, &tx.asset);
                    self.dequeue_cost(&k, fee)?;
                }
            }
            let to = r.to_exchange.as_deref().filter(|t| !t.is_empty() && *t != r.exchange);
            if let (true, Some(to)) = (self.per_venue, to) {
                let moved = match &fee { Some(f) => (&r.amount - f)?, None => r.amount.clone() };
                let k = self.key(&r.exchange, &tx.asset);
                let (taken, _) = take(self.lots.entry(k).or_default(), &moved)?;
                let dest = self.lots.entry((Some(to.to_string()), tx.asset.clone())).or_default();
                dest.extend(taken);
                dest.make_contiguous().sort_by_key(|l| l.date.0);
            }
        } else if self.per_venue {
            let k = self.key(&r.exchange, &tx.asset);
            self.dequeue_cost(&k, &r.amount)?;
        }
        Ok(())
    }

    // --- AcquisitionFee ---------------------------------------------------------------------------------------------

    /// `apply_acquisition_fee`: the quantity acquired and its cost, after the fee (D-TAX-US-SWAP-COST: a moved fee adds
    /// nothing to the cost).
    fn apply_acquisition_fee(&mut self, tx: &Tx, amount: &Dec, fiat_value: &Dec) -> Result<(Dec, Dec), FiguresError> {
        let r = &tx.row;
        let moved = self.moved_in.get(&r.uid);
        if present(r.fee_currency.as_deref()) && r.fee_currency.as_deref() == Some(r.base.as_str()) {
            let Some(fee) = &r.fee_amount else { return Ok((amount.clone(), fiat_value.clone())) };
            let cost = match moved { Some(value) => max0((fiat_value - value)?), None => fiat_value.clone() };
            return Ok((max0((amount - fee)?), cost));
        }
        let fee_cost = if moved.is_some() { Dec::zero() } else { r.fee_fiat_value.clone() };
        if let Some(fee) = r.fee_amount.as_ref().filter(|f| f.is_positive()) {
            if crypto_fee_asset(r) { self.spend_fee_coin(tx, fee)?; }
        }
        Ok((amount.clone(), (fiat_value + &fee_cost)?))
    }

    /// `consume_disposal_fee`. D-TAX-COIN-FEE: a fee in the coin sold leaves on top of the amount sold (the ledger's
    /// convention, `walk::quantity_moves`); Rails left those units in the lots.
    fn consume_disposal_fee(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        let Some(fee) = r.fee_amount.as_ref().filter(|f| f.is_positive()) else { return Ok(()) };
        if r.fee_currency.as_deref() == Some(r.base.as_str()) {
            if r.kind.out_leg() && !crate::tracker::cash(&r.base) { return self.spend_fee_coin(tx, fee); }
            return Ok(());
        }
        if !crypto_fee_asset(r) { return Ok(()); }
        self.spend_fee_coin(tx, fee)
    }

    /// `consume_fee_asset`: a fee paid in a third coin leaves that coin's lots; D-TAX-COIN-FEE: as a disposal at the fee's
    /// value where spending a coin is one (its value missing when the row's only missing price is its fee's).
    fn spend_fee_coin(&mut self, tx: &Tx, fee: &Dec) -> Result<(), FiguresError> {
        let r = &tx.row;
        let (symbol, asset) = (r.fee_currency.clone().unwrap_or_default(), tx.fee_asset.clone().unwrap_or_else(|| r.fee_currency.clone().unwrap_or_default()));
        if self.o.swaps_taxable {
            let sale = Sale { asset: &asset, symbol: &symbol, amount: fee.clone(), proceeds: r.fee_fiat_value.clone(), fee: Dec::zero(),
                              price_missing: r.fee_missing };
            return self.dispose(tx, sale, false);
        }
        let k = self.key(&r.exchange, &asset);
        self.dequeue_cost(&k, fee)
    }

    // --- Fifo -------------------------------------------------------------------------------------------------------

    /// `consume_fee_in_kind`.
    fn consume_fee_in_kind(&mut self, venue: &str, symbol: &str, asset: &str, amount: &Dec) -> Result<(), FiguresError> {
        if fiat(symbol) || !amount.is_positive() { return Ok(()); }
        let k = self.key(venue, asset);
        if self.lots.get(&k).is_none_or(VecDeque::is_empty) { return Ok(()); }
        self.dequeue_cost(&k, amount)
    }

    /// `record_disposal` of the row's own coin; its fee as Rails takes it, plus any swap cost moved onto it.
    fn sell(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        let fee = match self.moved_out.get(&r.uid) { Some(m) => (&r.fee_fiat_value + m)?, None => r.fee_fiat_value.clone() };
        let sale = Sale { asset: &tx.asset, symbol: &r.base, amount: r.amount.clone(), proceeds: r.fiat_value.clone(), fee, price_missing: r.price_missing };
        self.dispose(tx, sale, true)
    }

    /// `record_disposal`: the lots taken, then (for the row's own sale) its fee coin spent, then the rows.
    fn dispose(&mut self, tx: &Tx, sale: Sale, own: bool) -> Result<(), FiguresError> {
        let k = self.key(&tx.row.exchange, sale.asset);
        let lots = self.lots.entry(k).or_default();
        let has_lots = !lots.is_empty();
        let (tranches, _) = dequeue_tranches(lots, &sale.amount)?;
        if own { self.consume_disposal_fee(tx)?; }
        if self.o.holding.is_some() { return self.dispose_by_term(tx, &sale, tranches, has_lots); }
        let uncovered = max0((&sale.amount - &sum(tranches.iter().map(|t| &t.amount))?)?);
        let row = self.disposal_row(tx, &sale, &sale.amount, &sale.proceeds, &sale.fee, &tranches, has_lots, uncovered)?;
        self.disposals.push(row);
        Ok(())
    }

    /// `record_disposal_by_term`: one row per term, first met first; uncovered units go with the short side.
    fn dispose_by_term(&mut self, tx: &Tx, sale: &Sale, tranches: Vec<Tranche>, has_lots: bool) -> Result<(), FiguresError> {
        let rule = self.o.holding.expect("a holding rule");
        let at = tx.row.at;
        let mut groups: Vec<(bool, Vec<Tranche>)> = vec![];
        for t in tranches {
            let long = rule(self.holding_ref(&t, tx), at);
            match groups.iter_mut().find(|(g, _)| *g == long) { Some((_, g)) => g.push(t), None => groups.push((long, vec![t])) }
        }
        let mut taken = Dec::zero();
        for (_, g) in &groups { taken = (&taken + &sum(g.iter().map(|t| &t.amount))?)?; }
        let uncovered = (&sale.amount - &taken)?;
        if (uncovered.is_positive() || groups.is_empty()) && !groups.iter().any(|(l, _)| !l) { groups.push((false, vec![])); }
        let mut rows = vec![];
        for (long, group) in &groups {
            let mut group_amount = sum(group.iter().map(|t| &t.amount))?;
            if !long { group_amount = (&group_amount + &uncovered)?; }
            let proceeds = (&sale.proceeds * &group_amount)?.div(&sale.amount)?;
            let group_fee = (&sale.fee * &group_amount)?.div(&sale.amount)?;
            let short_uncovered = if *long { Dec::zero() } else { max0(uncovered.clone()) };
            let mut row = self.disposal_row(tx, sale, &group_amount, &proceeds, &group_fee, group, has_lots, short_uncovered)?;
            row.long_held = Some(*long);
            rows.push(row);
        }
        if rows.len() > 1 { settle_in_cents(&mut rows, &sale.proceeds, &sale.fee)?; }
        self.disposals.extend(rows);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn disposal_row(&self, tx: &Tx, sale: &Sale, amount: &Dec, proceeds: &Dec, fee: &Dec, tranches: &[Tranche], has_lots: bool, uncovered: Dec) -> Result<Disposal, FiguresError> {
        let r = &tx.row;
        let acquired = tranches.first().map(|t| self.holding_ref(t, tx));
        let cost_basis = sum(tranches.iter().map(|t| &t.cost))?;
        let basis_assumed = tranches.iter().any(|t| t.basis_assumed);
        // ((transacted_at - holding_ref_date) / 1.day).to_i: a Float of seconds, truncated.
        let holding_days = acquired.map_or(0, |a| (r.at.minus(a) / 86_400.0) as i64);
        Ok(Disposal {
            at: r.at, acquired, asset: sale.symbol.to_string(), amount: amount.clone(), proceeds: proceeds.clone(), cost_basis: cost_basis.clone(),
            fee: fee.clone(), gain_loss: (&(proceeds - &cost_basis)? - fee)?, holding_days,
            // D-TAX-UNCOVERED: complete only when every unit had a lot.
            complete: has_lots && !uncovered.is_positive(),
            // `data_incomplete?`, and D-TAX-UNCOVERED.
            incomplete: sale.price_missing || basis_assumed || !has_lots || uncovered.is_positive(),
            tx_id: tx.tx_id.clone(), exchange: r.exchange.clone(), long_held: None, uncovered,
        })
    }

    /// `holding_ref`.
    fn holding_ref(&self, t: &Tranche, tx: &Tx) -> At {
        if self.immediate_fiat_exit(t, tx) { if let Some(s) = t.technical_start { return s; } }
        t.holding_start.unwrap_or(t.date)
    }

    /// `immediate_fiat_exit?`: within 24 hours of the swap, by the clock, for fiat.
    fn immediate_fiat_exit(&self, t: &Tranche, tx: &Tx) -> bool {
        let (Some(_), Some(start)) = (t.technical_start, t.holding_start) else { return false };
        tx.fiat_exit && tx.row.at.minus(start) <= IMMEDIATE_EXIT
    }

    fn add_swap_in_lot(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        if self.o.swaps_taxable {
            let (lot_amount, lot_cost) = self.apply_acquisition_fee(tx, &r.amount, &r.fiat_value)?;
            let lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: r.price_missing,
                            unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                            holding_start: None, technical_start: None };
            self.store(&r.exchange, &tx.asset).push_back(lot);
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
        let (lot_amount, paid) = self.apply_acquisition_fee(tx, &r.amount, &cash)?;
        let fee_cost = (&paid - &cash)?;
        let (share, share_assumed) = self.swap_in_share(r);
        let assumed = r.price_missing || share_assumed;
        if tranches.is_empty() && !cash_present {
            // D-TAX-ORPHAN-SWAP: nothing to carry; market value is an assumption, and says so.
            let unpriced = if value_missing(r) { lot_amount.clone() } else { Dec::zero() };
            let cpu = unit(&(&r.fiat_value + &fee_cost)?, &lot_amount)?;
            self.open_swap_lot(tx, lot_amount, cpu, r.at, true, None, unpriced);
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
            // D-TAX-SWAP-FEE: the lot gets weight/total of `cost`, so the fee joins whole here (Rails weighted it again).
            let cost = (&carried + &fee_cost)?;
            let unpriced = if t.amount.is_positive() { (&tranche_amount * &t.unpriced)?.div(&t.amount)? } else { Dec::zero() };
            let cpu = cost.div(&rest)?;
            self.open_swap_lot(tx, tranche_amount, cpu, t.date, t.basis_assumed || assumed, t.holding_start, unpriced);
        }
        if cash_amount.is_positive() {
            let mut scale = if tranches.is_empty() { fee_cost.clone() } else if coin_value.is_positive() { coin_value.clone() } else { transferred_cost.clone() };
            // D-TAX-SWAP-FEE: when no tranche took any units, the fee joins the cash lot (Rails dropped it).
            if !tranches.is_empty() && !rest.is_positive() { scale = (&scale + &fee_cost)?; }
            let cpu = unit(&(&cash + &scale)?, &lot_amount)?;
            let lot_assumed = assumed || (rest.is_zero() && tranches.iter().any(|t| t.basis_assumed));
            self.open_swap_lot(tx, cash_amount, cpu, r.at, lot_assumed, None, Dec::zero());
        }
        // A tranche can predate everything already held, and the lots are read in order.
        self.store(&r.exchange, &tx.asset).make_contiguous().sort_by_key(|l| l.date.0);
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
        self.store(&r.exchange, &tx.asset).push_back(lot);
    }

    /// `technical_stablecoin_swap?`.
    fn technical_stablecoin_swap(&self, r: &Row, held_since: At) -> bool {
        stable(&r.base) && self.o.holding.is_some_and(|rule| rule(held_since, r.at))
    }

    /// `transfer_swap_out`: the lots an out-leg took handed to its group as tranches, weighted by the leg's worth.
    fn transfer_swap_out(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        let k = self.key(&r.exchange, &tx.asset);
        let (mut tranches, held) = dequeue_tranches(self.lots.entry(k).or_default(), &r.amount)?;
        // D-TAX-COIN-FEE: the leg's fee in a coin leaves its lots too (Rails left those units to be sold again).
        self.consume_disposal_fee(tx)?;
        let Some(key) = group_key(r) else {
            // An out-leg with no group: its basis goes nowhere (listed by the report as an unmatched swap).
            self.consumed = (&self.consumed + &sum(tranches.iter().map(|t| &t.cost))?)?;
            return Ok(());
        };
        if !self.in_groups.contains_key(&key) {
            // A group with no leg coming in: the basis goes nowhere (listed by the report as an unmatched swap).
            self.consumed = (&self.consumed + &sum(tranches.iter().map(|t| &t.cost))?)?;
            return Ok(());
        }
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
        let mut index: HashMap<Group, usize> = HashMap::new();
        for r in txs.iter().map(|t| &t.row) {
            if r.kind != kind || self.non_taxable_stablecoin_swap(&r.base) { continue; }
            if kind == Kind::SwapOut && self.fiat_disposal(r) { continue; }
            let Some(key) = group_key(r) else { continue };
            match index.get(&key) {
                Some(&i) => groups[i].1.push(r),
                None => { index.insert(key.clone(), groups.len()); groups.push((key, vec![r])); }
            }
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
                        swap_stable_cost: None, per_share: None, to_exchange: None, orphan: false, opening: false, stated_value: false, fee_missing: false },
             tx_id: None, fiat_exit: true, asset: base.into(), fee_asset: None, cash_group: false }
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
            rows[6].fee_asset = Some("ETH".into());
            let acquired = sum([d("300"), d("1000"), d("70"), d("2507")].iter()).unwrap();
            let holding = |_: At, _: At| false;
            let mut f = Fifo::new(Options { swaps_taxable: taxable, stablecoin_as_fiat: false, holding: Some(&holding), swap_resets_holding_period: false, per_venue_from, swap_cost_to_disposed_from: None });
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
        let mut f = Fifo::new(Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None, swap_cost_to_disposed_from: None });
        f.calculate(&rows).unwrap();
        assert_eq!(f.disposals.len(), 1);
        let x = &f.disposals[0];
        assert_eq!((x.cost_basis.to_s_f(), x.uncovered.to_s_f(), x.complete, x.incomplete), ("500.0".into(), "0.5".into(), false, true));
        assert_eq!(f.consumed.to_s_f(), "100.0");
    }
}
