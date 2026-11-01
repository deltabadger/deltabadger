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
//! - Austria (`Options::austria`; EStG §§ 27b, 124b Z 384, KryptowährungsVO), each its own name:
//!   - D-TAX-AT-SWAP-1-MARCH-2022: before `regime_from` a swap is a Tausch under § 31 (the coin given is sold at its
//!     value, the coin received bought at it) and fees are costs; from it, § 27b: a swap carries the cost.
//!   - D-TAX-AT-AVERAGE: from `average_from` every store holds its new stock as one lot at the moving average, behind
//!     its old stock (VO § 2 Abs. 1, § 3 Z 2; re-averaged once at the instant, Rz 6178aa); before it FIFO (Rz 6103l).
//!     Every arrival goes through `receive`.
//!   - D-TAX-AT-OLD-SWAP: from `regime_from`, old stock given in a swap enters it at its value, dated at the swap (Rz 6103k).
//!   - D-TAX-AT-FEES: from `regime_from`, a coin paid on a swap leaves the store and its cost stays (Rz 6178ad); a coin
//!     paid for any other service (a third-coin fee, a network fee, linked or not, a fee row) is a disposal (Rz 6178v).
//!   - D-TAX-AT-SALE-FEE: from `regime_from`, a sale fee does not reduce the gain (§ 20 Abs. 2 Z 3 lit. a); it stays shown.
//!   - D-TAX-AT-REWARDS: from `regime_from`, staking rewards and airdrops open at cost zero (§ 27a Abs. 4 Z 5).
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
    /// Austria's own rules, from the instants they take effect.
    pub austria: Option<Austria>,
    /// D-TAX-BG-AVERAGE: every store holds one lot at the weighted average from the first row (`receive` merges every
    /// arrival into it).
    pub average_all: bool,
    /// D-TAX-BG-DEPOSIT-ZERO, D-TAX-DK-DEPOSIT-ZERO: an unlinked deposit opens at cost 0 (no documented purchase), neither
    /// assumed nor unpriced.
    pub deposit_cost_zero: bool,
    /// D-TAX-DK-PER-PART: a sale gives one row per FIFO part (a lot or its share, an uncovered part its own row).
    pub row_per_lot: bool,
    /// Denmark's mining provenance: lots from mining carry `mined` to their disposal rows.
    pub tag_mining: bool,
    /// D-TAX-DK-STABLECOIN: the instant each coin's units held are recorded (`Fifo::snapshot`).
    pub snapshot_at: Option<At>,
    /// Italy (Circolare 30/E p. 54): lots leave newest first, everywhere they leave (Rails' Lifo overrides the dequeue).
    pub lifo: bool,
    /// D-TAX-IT-FEES: from this instant fees count nowhere: none joins a cost, none comes off a gain, and units paid as an
    /// own-coin fee on an acquisition leave at their share of its cost.
    pub fees_ignored_from: Option<At>,
    /// D-TAX-IT-PRE2023-BASIS: before this instant a swap is an exchange (the coin given sold, the coin received bought at
    /// its value) even where `swaps_taxable` is false.
    pub swaps_taxable_before: Option<At>,
    /// Ireland's s. 581: the 4-week matching (D-TAX-IE-4WEEK) and the loss restriction (D-TAX-IE-RING-FENCE).
    pub ireland: Option<Ireland<'a>>,
}

/// s. 581's windows, on the taxpayer's calendar: `from(at)` the first instant of the 4 weeks before a disposal at `at`
/// (its local date − 28 days, from midnight), `until(at)` the first instant after the 4 weeks after it (its local date + 29
/// days, midnight): day 28 is inside both ways.
#[derive(Clone, Copy)]
pub struct Ireland<'a> { pub from: &'a dyn Fn(At) -> At, pub until: &'a dyn Fn(At) -> At }

/// The order lots leave a store in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Fifo,
    /// Newest first; the parts come back oldest first (Rails' `unshift`).
    Lifo,
    /// D-TAX-IE-4WEEK (s. 581(1)–(2)): the lots dated from `.0` to `.1` newest first, then the rest oldest first.
    Recent(At, At),
}

/// The instants Austria's rules turn on, in the taxpayer's zone.
#[derive(Clone, Copy, Debug)]
pub struct Austria {
    /// § 124b Z 384 lit. a: acquired from 1 March 2021, new stock; before it, old stock (outside § 27b).
    pub new_stock_from: At,
    /// § 124b Z 384 lit. a: § 27b's rules (swaps no realisation, its fees) apply from 1 March 2022.
    pub regime_from: At,
    /// KryptowährungsVO § 7: the moving average, mandatory for realisations from 1 January 2023.
    pub average_from: At,
}

#[derive(Clone, Debug)]
pub struct Lot { pub amount: Dec, pub cost_per_unit: Dec, pub date: At, pub basis_assumed: bool, pub unpriced: Dec,
                 holding_start: Option<At>, technical_start: Option<At>,
                 /// D-TAX-AT-AVERAGE: more than one acquisition averaged into this lot.
                 pub averaged: bool,
                 /// `Options::tag_mining`: acquired by mining.
                 pub mined: bool,
                 /// D-TAX-IE-RING-FENCE: restricted loss carried by this lot (never part of its cost), and the units it covers.
                 pub fenced: Dec, pub fenced_units: Dec }

#[derive(Clone, Debug)]
struct Tranche { amount: Dec, cost: Dec, date: At, basis_assumed: bool, unpriced: Dec, holding_start: Option<At>,
                 technical_start: Option<At>, weight: Dec, averaged: bool, mined: bool, fenced: Dec, recent: bool }

/// One disposal row (`disposal_row`), `long_held` set where a holding rule split the sale.
#[derive(Clone, Debug)]
pub struct Disposal {
    pub at: At, pub acquired: Option<At>, pub asset: String, pub amount: Dec, pub proceeds: Dec, pub cost_basis: Dec, pub fee: Dec,
    pub gain_loss: Dec, pub holding_days: i64, pub complete: bool, pub incomplete: bool, pub tx_id: Option<String>,
    pub exchange: String, pub long_held: Option<bool>,
    /// D-TAX-UNCOVERED: the units of this row no lot covered.
    pub uncovered: Dec,
    /// D-TAX-AT-AVERAGE: some of it came from an averaged lot.
    pub averaged: bool,
    /// `Options::tag_mining`: some of it came from a mined lot (with `row_per_lot`, all of it).
    pub mined: bool,
    /// The row it disposes of (`Row::uid`): the rows of one disposal share it.
    pub source: usize,
    /// D-TAX-IE-4WEEK: some part was matched with an acquisition of the 4 weeks before.
    pub recent: bool,
    /// D-TAX-IE-RING-FENCE: the part of this row's loss restricted by a reacquisition within 4 weeks after it.
    pub restricted: Dec,
    /// D-TAX-IE-RING-FENCE: restricted losses this row's parts used (each up to its own gain), and what they could not.
    pub released: Dec, pub unusable: Dec,
}

/// D-TAX-IE-RING-FENCE: a loss waiting for a reacquisition: the store, the window (after the disposal, before `until`),
/// the units still to cover, the loss per unit, and the row.
struct Fence { key: Key, after: At, until: At, units: Dec, per_unit: Dec, loss_left: Dec, row: usize }

/// D-TAX-US-WALLET: one lot as placed at the boundary (`venue` None: held on no synced venue).
#[derive(Clone, Debug)]
pub struct Placed { pub venue: Option<String>, pub symbol: String, pub amount: Dec, pub cost: Dec, pub acquired: At }

type Key = (Option<String>, String);
type Group = (String, String);
/// D-TAX-SWAP-FEE-EXHAUSTED, Austria: an exhausted leg waiting for its swap's pool: (uid, basis, assumed).
type Held = (usize, Dec, bool);
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
    /// D-TAX-AT-AVERAGE: past `average_from`.
    averaging: bool,
    /// D-TAX-US-WALLET, before the boundary: what each venue holds of each coin, in the order first touched.
    held: Vec<((String, String), Dec)>,
    symbols: HashMap<String, String>,
    pub disposals: Vec<Disposal>,
    /// `excess_roc_rows`, as indexes into the rows.
    pub excess_roc_rows: Vec<usize>,
    pub excess_roc: Dec,
    /// Basis that left the lots with no disposal row: fees in kind, transfer fees, lost coins, coins off a venue.
    pub consumed: Dec,
    /// D-TAX-SWAP-FEE-EXHAUSTED: swap-ins whose own-coin fee took every unit received, with the basis they could not
    /// carry (row uid, cost); listed by the report, never a loss.
    pub exhausted: Vec<(usize, Dec)>,
    /// Austria: an exhausted leg's basis waiting for a same-coin leg of its swap, by (group, coin): (uid, basis, assumed).
    held_back: HashMap<(Group, String), Vec<Held>>,
    /// D-TAX-US-WALLET: the lots as placed at the boundary, and whether the placement was a choice (a coin held on more
    /// than one venue, or off every synced one).
    pub allocation: Vec<Placed>,
    pub allocation_chosen: bool,
    /// D-TAX-DK-STABLECOIN: each coin's units held at `Options::snapshot_at`, by symbol.
    pub snapshot: Option<Vec<(String, Dec)>>,
    fences: Vec<Fence>,
    /// The latest instant walked: a row walked out of time order (an Alpaca CFEE after its day's fills) takes effect then.
    clock: At,
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
               moved_in: HashMap::new(), moved_out: HashMap::new(), per_venue: false, averaging: false, held: vec![], symbols: HashMap::new(), disposals: vec![],
               excess_roc_rows: vec![], excess_roc: Dec::zero(), consumed: Dec::zero(), exhausted: vec![], held_back: HashMap::new(), allocation: vec![], allocation_chosen: false, snapshot: None, fences: vec![], clock: At(i64::MIN) }
    }

    /// The lots left standing, per (venue, coin).
    pub fn lots(&self) -> impl Iterator<Item = (&Key, &VecDeque<Lot>)> { self.lots.iter() }

    fn key(&self, venue: &str, asset: &str) -> Key { (self.per_venue.then(|| venue.to_string()), asset.to_string()) }
    fn store(&mut self, venue: &str, asset: &str) -> &mut VecDeque<Lot> { let k = self.key(venue, asset); self.lots.entry(k).or_default() }

    /// Whether a swap at `at` is a disposal: where the law says so, and in Austria before § 27b (a Tausch under § 31).
    fn swaps_taxable_at(&self, at: At) -> bool {
        self.o.swaps_taxable || self.o.austria.is_some_and(|a| at.0 < a.regime_from.0) || self.o.swaps_taxable_before.is_some_and(|b| at.0 < b.0)
    }
    /// D-TAX-IT-FEES.
    fn fees_ignored(&self, at: At) -> bool { self.o.fees_ignored_from.is_some_and(|f| at.0 >= f.0) }
    /// The order every lot leaves in, but a disposal's under s. 581.
    fn order(&self) -> Order { if self.o.lifo { Order::Lifo } else { Order::Fifo } }
    /// Whether Austria's § 27b is in force at `at`.
    fn austrian(&self, at: At) -> bool { self.o.austria.is_some_and(|a| at.0 >= a.regime_from.0) }
    /// D-TAX-COIN-FEE, D-TAX-AT-FEES: whether a coin paid for a service (not on a non-taxable swap) is a disposal.
    fn coin_fees_disposed(&self, at: At) -> bool { self.swaps_taxable_at(at) || self.austrian(at) }
    /// D-TAX-AT-SALE-FEE: whether a sale's fee comes off its gain.
    fn fee_deducted(&self, at: At) -> bool { !self.austrian(at) && !self.fees_ignored(at) }

    /// D-TAX-AT-AVERAGE: every arrival. Averaging, new stock joins the store's one averaged lot (last), old stock goes
    /// ahead of it in date order; otherwise the lot queues.
    fn receive(&mut self, k: Key, lot: Lot) -> Result<(), FiguresError> {
        if self.o.average_all {
            // D-TAX-BG-AVERAGE: the weighted average of every unit held (ЗДДФЛ чл. 33 ал. 4).
            let lots = self.lots.entry(k).or_default();
            match lots.back_mut() { Some(average) => *average = merge(average, lot)?, None => lots.push_back(lot) }
            return Ok(());
        }
        let new_from = self.o.austria.filter(|_| self.averaging).map(|a| a.new_stock_from);
        let lots = self.lots.entry(k).or_default();
        match new_from {
            Some(from) if lot.date.0 >= from.0 => match lots.back_mut().filter(|l| l.date.0 >= from.0) {
                Some(average) => *average = merge(average, lot)?,
                None => lots.push_back(lot),
            },
            Some(_) => { let at = lots.iter().position(|l| l.date.0 > lot.date.0).unwrap_or(lots.len()); lots.insert(at, lot); }
            None => lots.push_back(lot),
        }
        Ok(())
    }

    /// D-TAX-AT-AVERAGE at `average_from`: each store's new stock left is averaged once, total cost over units (Rz 6178aa).
    fn reaverage(&mut self, new_from: At) -> Result<(), FiguresError> {
        self.averaging = true;
        for lots in self.lots.values_mut() {
            let (old, new): (Vec<Lot>, Vec<Lot>) = lots.drain(..).partition(|l| l.date.0 < new_from.0);
            lots.extend(old);
            let mut new = new.into_iter();
            let Some(mut average) = new.next() else { continue };
            for l in new { average = merge(&average, l)?; }
            lots.push_back(average);
        }
        Ok(())
    }

    /// D-TAX-AT-FEES: units paid as a swap's fee leave the store and their cost stays in it (Rz 6178ad), on its first
    /// new-stock lot (an old unit's cost is outside the tax and goes); with no such lot it goes.
    fn shed(&mut self, k: &Key, amount: &Dec, new_from: At) -> Result<(), FiguresError> {
        let lots = self.lots.entry(k.clone()).or_default();
        let (taken, _) = take(lots, amount, Order::Fifo)?;
        let (mut kept, mut gone) = (Dec::zero(), Dec::zero());
        for l in &taken {
            let cost = (&l.amount * &l.cost_per_unit)?;
            if l.date.0 < new_from.0 { gone = (&gone + &cost)?; } else { kept = (&kept + &cost)?; }
        }
        match lots.iter_mut().find(|l| l.date.0 >= new_from.0 && l.amount.is_positive()) {
            Some(l) => l.cost_per_unit = (&(&l.amount * &l.cost_per_unit)? + &kept)?.div(&l.amount)?,
            None => gone = (&gone + &kept)?,
        }
        self.consumed = (&self.consumed + &gone)?;
        Ok(())
    }

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
            if tx.row.at.0 > self.clock.0 { self.clock = tx.row.at; }
            if let Some(a) = self.o.austria.filter(|a| !self.averaging && tx.row.at.0 >= a.average_from.0) { self.reaverage(a.new_stock_from)?; }
            if self.o.snapshot_at.is_some_and(|s| self.snapshot.is_none() && tx.row.at.0 >= s.0) { self.take_snapshot()?; }
            self.enter_row(tx)?;
            let r = &tx.row;
            let (asset, amount, fiat_value) = (tx.asset.as_str(), &r.amount, &r.fiat_value);
            match r.kind {
                Kind::Buy | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => {
                    let (lot_amount, lot_cost) = self.apply_acquisition_fee(tx, amount, fiat_value)?;
                    // D-TAX-AT-REWARDS: staking and airdrops cost nothing (§ 27b Abs. 2, § 27a Abs. 4 Z 5).
                    let zero = matches!(r.kind, Kind::StakingReward | Kind::Airdrop) && self.austrian(r.at);
                    let lot = Lot { cost_per_unit: if zero { Dec::zero() } else { unit(&lot_cost, &lot_amount)? }, date: r.at, basis_assumed: r.price_missing && !zero,
                                    unpriced: if value_missing(r) && !zero { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                                    holding_start: None, technical_start: None, averaged: false, mined: self.o.tag_mining && r.kind == Kind::Mining,
                                    fenced: Dec::zero(), fenced_units: Dec::zero() };
                    let k = self.key(&r.exchange, asset);
                    // D-TAX-IE-RING-FENCE: a purchase is a reacquisition (income is not: this report's reading).
                    let mut lot = lot;
                    if r.kind == Kind::Buy { self.fence_fill(&k, &mut lot)?; }
                    self.receive(k, lot)?;
                }
                Kind::Deposit => {
                    if r.linked { continue; }
                    let (lot_amount, lot_cost) = self.apply_acquisition_fee(tx, amount, fiat_value)?;
                    // D-TAX-DK-DEPOSIT-ZERO (and BG's): with no documented purchase the cost is 0, a fact, not an assumption.
                    let zero = self.o.deposit_cost_zero;
                    let lot = Lot { cost_per_unit: if zero { Dec::zero() } else { unit(&lot_cost, &lot_amount)? }, date: r.at, basis_assumed: !zero,
                                    unpriced: if value_missing(r) && !zero { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                                    holding_start: None, technical_start: None, averaged: false, mined: false, fenced: Dec::zero(), fenced_units: Dec::zero() };
                    let k = self.key(&r.exchange, asset);
                    self.receive(k, lot)?;
                }
                Kind::SwapIn => {
                    if self.non_taxable_stablecoin_swap(&r.base) { continue; }
                    self.add_swap_in_lot(tx)?;
                }
                Kind::SwapOut => {
                    if self.non_taxable_stablecoin_swap(&r.base) { continue; }
                    if !self.swaps_taxable_at(r.at) && !self.fiat_disposal(r) { self.transfer_swap_out(tx)?; }
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
                // D-TAX-COIN-FEE, D-TAX-AT-FEES.
                Kind::Fee if self.coin_fees_disposed(r.at) && !fiat(&r.base) && amount.is_positive() => {
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
        // Austria: a basis no leg of its swap took is listed after all.
        let mut held: Vec<Held> = self.held_back.drain().flat_map(|(_, v)| v).collect();
        held.sort_by_key(|h| h.0);
        for (uid, basis, _) in held { self.list_exhausted(uid, basis)?; }
        if self.o.snapshot_at.is_some() && self.snapshot.is_none() { self.take_snapshot()?; }
        Ok(())
    }

    /// D-TAX-DK-STABLECOIN: the units of each coin held now, across every store, by symbol.
    fn take_snapshot(&mut self) -> Result<(), FiguresError> {
        let mut held: Vec<(String, Dec)> = vec![];
        for ((_, asset), lots) in &self.lots {
            let symbol = self.symbols.get(asset).cloned().unwrap_or_else(|| asset.clone());
            let units = sum(lots.iter().map(|l| &l.amount))?;
            match held.iter_mut().find(|(s, _)| *s == symbol) { Some((_, u)) => *u = (&*u + &units)?, None => held.push((symbol, units)) }
        }
        held.sort_by(|a, b| a.0.cmp(&b.0));
        self.snapshot = Some(held);
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
                let (taken, _) = take(&mut lots, &quantity, Order::Fifo)?;
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
                if self.coin_fees_disposed(r.at) {
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
                let order = self.order();
                let (taken, _) = take(self.lots.entry(k).or_default(), &moved, order)?;
                let dest = (Some(to.to_string()), tx.asset.clone());
                for l in taken { self.receive(dest.clone(), l)?; }
                self.lots.entry(dest).or_default().make_contiguous().sort_by_key(|l| l.date.0);
            }
        } else {
            // D-TAX-COIN-FEE: an unlinked withdrawal's fee in its own coin is inside its amount (`walk::quantity_moves`) and
            // was spent, not moved: it leaves the lots as the fee it is, the rest as the withdrawal. Rails kept it.
            let mut amount = r.amount.clone();
            if let Some(fee) = r.fee_amount.as_ref().filter(|f| f.is_positive() && r.fee_currency.as_deref() == Some(r.base.as_str())) {
                self.spend_fee_coin(tx, fee)?;
                amount = max0((&amount - fee)?);
            }
            if self.per_venue {
                let k = self.key(&r.exchange, &tx.asset);
                self.dequeue_cost(&k, &amount)?;
            }
        }
        // D-TAX-COIN-FEE: a fee in another coin on any withdrawal leaves that coin's lots (Rails never took it).
        if let Some(fee) = r.fee_amount.as_ref().filter(|f| f.is_positive()).filter(|_| crypto_fee_asset(r) && r.fee_currency.as_deref() != Some(r.base.as_str())) {
            self.spend_fee_coin(tx, fee)?;
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
            let net = max0((amount - fee)?);
            if self.fees_ignored(r.at) && amount.is_positive() {
                // D-TAX-IT-FEES: the fee's units leave at their share of the cost; the rest stays the units' cost.
                let kept = (&cost * &net)?.div(amount)?;
                self.consumed = (&self.consumed + &(&cost - &kept)?)?;
                return Ok((net, kept));
            }
            return Ok((net, cost));
        }
        let fee_cost = if moved.is_some() || self.fees_ignored(r.at) { Dec::zero() } else { r.fee_fiat_value.clone() };
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
            // A stablecoin that reached the engine is a coin here (a stablecoin that is cash never does).
            if r.kind.out_leg() && !fiat(&r.base) { return self.spend_fee_coin(tx, fee); }
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
        let swap_leg = matches!(r.kind, Kind::SwapIn | Kind::SwapOut) && !self.swaps_taxable_at(r.at);
        if !swap_leg && self.coin_fees_disposed(r.at) {
            let sale = Sale { asset: &asset, symbol: &symbol, amount: fee.clone(), proceeds: r.fee_fiat_value.clone(), fee: Dec::zero(),
                              price_missing: r.fee_missing };
            return self.dispose(tx, sale, false);
        }
        let k = self.key(&r.exchange, &asset);
        // D-TAX-AT-FEES: a swap's fee in a coin is no realisation and its cost stays (§ 27b Abs. 3, Rz 6178ad).
        if let Some(a) = self.o.austria.filter(|_| swap_leg && self.austrian(r.at)) { return self.shed(&k, fee, a.new_stock_from); }
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
        // D-TAX-IE-4WEEK: the acquisitions of the 4 weeks before first, newest first. A row walked out of time order (a CFEE
        // after its day's fills) takes effect when walked: the fills it pays for come before it.
        let effective = self.clock;
        let order = match self.o.ireland { Some(ie) => Order::Recent((ie.from)(effective), effective), None => self.order() };
        let lots = self.lots.entry(k.clone()).or_default();
        let has_lots = !lots.is_empty();
        let (tranches, _) = dequeue_tranches(lots, &sale.amount, order)?;
        if own { self.consume_disposal_fee(tx)?; }
        if self.o.holding.is_some() { return self.dispose_by_term(tx, &sale, tranches, has_lots); }
        if self.o.row_per_lot { return self.dispose_per_lot(tx, &sale, tranches, has_lots); }
        let uncovered = max0((&sale.amount - &sum(tranches.iter().map(|t| &t.amount))?)?);
        let mut row = self.disposal_row(tx, &sale, &sale.amount, &sale.proceeds, &sale.fee, &tranches, has_lots, uncovered)?;
        if let Some(ie) = self.o.ireland {
            // D-TAX-IE-RING-FENCE: each part's restricted loss is used up to that part's own gain.
            for t in tranches.iter().filter(|t| t.fenced.is_positive()) {
                let part = |v: &Dec| -> Result<Dec, FiguresError> { if sale.amount.is_positive() { Ok((v * &t.amount)?.div(&sale.amount)?) } else { Ok(Dec::zero()) } };
                let gain = max0((&(&part(&sale.proceeds)? - &t.cost)? - &part(&sale.fee)?)?);
                let used = if gain < t.fenced { gain } else { t.fenced.clone() };
                row.unusable = (&row.unusable + &(&t.fenced - &used)?)?;
                row.released = (&row.released + &used)?;
            }
            self.disposals.push(row);
            let row = self.disposals.len() - 1;
            let d = &self.disposals[row];
            // s. 581(3): the disposal's own loss waits for a reacquisition within 4 weeks after it.
            // A residue below a cent (a USD conversion's last digits) is no loss.
            if d.gain_loss.round(CENTS)?.is_negative() && d.amount.is_positive() {
                let loss = (&Dec::zero() - &d.gain_loss)?;
                let fence = Fence { key: k, after: effective, until: (ie.until)(effective), units: d.amount.clone(), per_unit: loss.div(&d.amount)?, loss_left: loss, row };
                self.fences.push(fence);
            }
            return Ok(());
        }
        self.disposals.push(row);
        Ok(())
    }

    /// D-TAX-IE-RING-FENCE: a reacquisition of `k` covers the open losses, the oldest first.
    fn fence_fill(&mut self, k: &Key, lot: &mut Lot) -> Result<(), FiguresError> {
        if self.o.ireland.is_none() { return Ok(()); }
        for n in 0..self.fences.len() { if self.fences[n].key == *k { self.fill_one(n, lot)?; } }
        Ok(())
    }

    fn fill_one(&mut self, n: usize, lot: &mut Lot) -> Result<(), FiguresError> {
        let f = &mut self.fences[n];
        // Walked after the loss, so a purchase at the same instant is after it too.
        if !f.units.is_positive() || lot.date.0 < f.after.0 || lot.date.0 >= f.until.0 { return Ok(()); }
        let free = max0((&lot.amount - &lot.fenced_units)?);
        let q = if f.units < free { f.units.clone() } else { free };
        if !q.is_positive() { return Ok(()); }
        // The last units take the loss that is left, so a whole restriction is the whole loss to the cent.
        let loss = if q == f.units { f.loss_left.clone() } else { (&q * &f.per_unit)? };
        f.loss_left = (&f.loss_left - &loss)?;
        f.units = (&f.units - &q)?;
        lot.fenced = (&lot.fenced + &loss)?;
        lot.fenced_units = (&lot.fenced_units + &q)?;
        let d = &mut self.disposals[f.row];
        d.restricted = (&d.restricted + &loss)?;
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
        if rows.len() > 1 { settle_in_cents(&mut rows, &sale.proceeds, &sale.fee, self.fee_deducted(at))?; }
        self.disposals.extend(rows);
        Ok(())
    }

    /// D-TAX-DK-PER-PART: one row per FIFO part, then the uncovered part (no acquisition date); proceeds and fee by
    /// quantity share; every row reconciles in cents (proceeds, cost and fee each to the cent, the last part taking the
    /// rounded rest of each, the last covered part the cost's; the gain from the three).
    fn dispose_per_lot(&mut self, tx: &Tx, sale: &Sale, tranches: Vec<Tranche>, has_lots: bool) -> Result<(), FiguresError> {
        let taken = sum(tranches.iter().map(|t| &t.amount))?;
        let uncovered = max0((&sale.amount - &taken)?);
        let mut parts: Vec<(Dec, Vec<Tranche>)> = tranches.into_iter().map(|t| (t.amount.clone(), vec![t])).collect();
        if uncovered.is_positive() || parts.is_empty() { parts.push((uncovered.clone(), vec![])); }
        let share = |v: &Dec, amount: &Dec| -> Result<Dec, FiguresError> {
            if sale.amount.is_positive() { Ok((v * amount)?.div(&sale.amount)?) } else { Ok(v.clone()) }
        };
        let mut rows = vec![];
        for (amount, part) in &parts {
            let row = self.disposal_row(tx, sale, amount, &share(&sale.proceeds, amount)?, &share(&sale.fee, amount)?, part, has_lots,
                                            if part.is_empty() { uncovered.clone() } else { Dec::zero() })?;
            rows.push(row);
        }
        let total = |f: fn(&Disposal) -> &Dec| -> Result<Dec, FiguresError> { Ok(sum(rows.iter().map(f))?.round(CENTS)?) };
        let mut left = [total(|r| &r.proceeds)?, total(|r| &r.cost_basis)?, total(|r| &r.fee)?];
        let count = rows.len();
        // The cost's rest goes to the last part a lot covered (an uncovered part has none).
        let cost_last = rows.iter().rposition(|r| !r.uncovered.is_positive()).unwrap_or(count - 1);
        for (n, row) in rows.iter_mut().enumerate() {
            let lasts = [n == count - 1, n == cost_last, n == count - 1];
            let mut three = [row.proceeds.clone(), row.cost_basis.clone(), row.fee.clone()];
            for ((t, l), last) in three.iter_mut().zip(left.iter_mut()).zip(lasts) {
                *t = if last { l.clone() } else { t.round(CENTS)? };
                *l = (&*l - &*t)?;
            }
            [row.proceeds, row.cost_basis, row.fee] = three;
            row.gain_loss = if self.fee_deducted(row.at) { (&(&row.proceeds - &row.cost_basis)? - &row.fee)? } else { (&row.proceeds - &row.cost_basis)? };
        }
        self.disposals.extend(rows);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn disposal_row(&self, tx: &Tx, sale: &Sale, amount: &Dec, proceeds: &Dec, fee: &Dec, tranches: &[Tranche], has_lots: bool, uncovered: Dec) -> Result<Disposal, FiguresError> {
        let r = &tx.row;
        // Rails' Lifo dates a disposal at its newest part (`latest_date`).
        let acquired = if self.o.lifo { tranches.last() } else { tranches.first() }.map(|t| self.holding_ref(t, tx));
        let cost_basis = sum(tranches.iter().map(|t| &t.cost))?;
        let basis_assumed = tranches.iter().any(|t| t.basis_assumed);
        // ((transacted_at - holding_ref_date) / 1.day).to_i: a Float of seconds, truncated.
        let holding_days = acquired.map_or(0, |a| (r.at.minus(a) / 86_400.0) as i64);
        Ok(Disposal {
            at: r.at, acquired, asset: sale.symbol.to_string(), amount: amount.clone(), proceeds: proceeds.clone(), cost_basis: cost_basis.clone(),
            fee: fee.clone(), gain_loss: if self.fee_deducted(r.at) { (&(proceeds - &cost_basis)? - fee)? } else { (proceeds - &cost_basis)? }, holding_days,
            // D-TAX-UNCOVERED: complete only when every unit had a lot.
            complete: has_lots && !uncovered.is_positive(),
            // `data_incomplete?`, and D-TAX-UNCOVERED.
            incomplete: sale.price_missing || basis_assumed || !has_lots || uncovered.is_positive(),
            tx_id: tx.tx_id.clone(), exchange: r.exchange.clone(), long_held: None, uncovered, averaged: tranches.iter().any(|t| t.averaged), source: r.uid,
            mined: tranches.iter().any(|t| t.mined), recent: tranches.iter().any(|t| t.recent),
            restricted: Dec::zero(), released: Dec::zero(), unusable: Dec::zero(),
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
        if self.swaps_taxable_at(r.at) {
            // D-TAX-AT-SWAP-1-MARCH-2022: an Austrian Tausch before § 27b acquires at the value of what was given (§ 6 Z 14),
            // the out-legs' worth by this leg's share; elsewhere, and with no out-leg priced, its own value.
            let given = self.o.austria.and_then(|_| group_key(r)).map(|k| self.swap_out_value(Some(&k))).filter(Dec::is_positive);
            // A fiat top-up in the group is part of what was given.
            let value = match given { Some(v) => (&(&v * &self.swap_in_share(r).0)? + &self.swap_cash_cost(r)?.1)?, None => r.fiat_value.clone() };
            let (lot_amount, lot_cost) = self.apply_acquisition_fee(tx, &r.amount, &value)?;
            let mut lot = Lot { cost_per_unit: unit(&lot_cost, &lot_amount)?, date: r.at, basis_assumed: r.price_missing,
                            unpriced: if value_missing(r) { lot_amount.clone() } else { Dec::zero() }, amount: lot_amount,
                            holding_start: None, technical_start: None, averaged: false, mined: false, fenced: Dec::zero(), fenced_units: Dec::zero() };
            let k = self.key(&r.exchange, &tx.asset);
            self.fence_fill(&k, &mut lot)?;
            return self.receive(k, lot);
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
        // D-TAX-AT-FEES: a crypto↔crypto swap's costs are ignored (§ 27b Abs. 3), a fee in fiat too; a group that spent fiat
        // is a purchase, whose fee is part of the cost (Rz 6178y). A stablecoin's leg (cash to the enrichment, cost 0) is not.
        let fee_cost = if self.austrian(r.at) && !cash.is_positive() { Dec::zero() } else { (&paid - &cash)? };
        let (share, share_assumed) = self.swap_in_share(r);
        let assumed = r.price_missing || share_assumed;
        if tranches.is_empty() && !cash_present {
            // D-TAX-IT-DEPOSIT-ZERO: with no documented cost the cost is zero (art. 68 c. 9-bis), as an unlinked deposit's.
            if self.o.deposit_cost_zero { return self.open_swap_lot(tx, lot_amount, Dec::zero(), r.at, false, None, Dec::zero()); }
            // D-TAX-ORPHAN-SWAP: nothing to carry; market value is an assumption, and says so.
            let unpriced = if value_missing(r) { lot_amount.clone() } else { Dec::zero() };
            let cpu = unit(&(&r.fiat_value + &fee_cost)?, &lot_amount)?;
            return self.open_swap_lot(tx, lot_amount, cpu, r.at, true, None, unpriced);
        }
        let mut transferred_cost = (&sum(tranches.iter().map(|t| &t.cost))? * &share)?;
        // D-TAX-IT-FEES: units paid as this leg's own-coin fee leave at their share of the carried basis.
        let kept = if self.fees_ignored(r.at) && r.amount.is_positive() && lot_amount < r.amount && lot_amount.is_positive() {
            Some(lot_amount.div(&r.amount)?) } else { None };
        if let Some(f) = &kept {
            let after = (&transferred_cost * f)?;
            self.consumed = (&self.consumed + &(&transferred_cost - &after)?)?;
            transferred_cost = after;
        }
        if !lot_amount.is_positive() {
            let assumed = assumed || tranches.iter().any(|t| t.basis_assumed);
            return self.exhaust(tx, (&(&transferred_cost + &cash)? + &fee_cost)?, assumed);
        }
        let coin_value = (&self.swap_out_value(key.as_ref()) * &share)?;
        let cash_amount = cash_lot_amount(&lot_amount, &cash, &coin_value, &transferred_cost, &tranches)?;
        let rest = (&lot_amount - &cash_amount)?;
        let total_weight = sum(tranches.iter().map(|t| &t.weight))?;
        let mut allocated = Dec::zero();
        for (i, t) in tranches.iter().enumerate() {
            let tranche_amount = if i == tranches.len() - 1 { (&rest - &allocated)? } else { (&rest * &t.weight)?.div(&total_weight)? };
            allocated = (&allocated + &tranche_amount)?;
            if !tranche_amount.is_positive() { continue; }
            let mut carried = (&(&t.cost * &share)? * &total_weight)?.div(&t.weight)?;
            if let Some(f) = &kept { carried = (&carried * f)?; }
            // D-TAX-SWAP-FEE: the lot gets weight/total of `cost`, so the fee joins whole here (Rails weighted it again).
            let cost = (&carried + &fee_cost)?;
            let unpriced = if t.amount.is_positive() { (&tranche_amount * &t.unpriced)?.div(&t.amount)? } else { Dec::zero() };
            let cpu = cost.div(&rest)?;
            // Under LIFO the coin received is the newest acquisition of it, dated at the swap (its cost carried).
            let date = if self.o.lifo { r.at } else { t.date };
            self.open_swap_lot(tx, tranche_amount, cpu, date, t.basis_assumed || assumed, t.holding_start.filter(|_| !self.o.lifo), unpriced)?;
        }
        if cash_amount.is_positive() {
            let mut scale = if tranches.is_empty() { fee_cost.clone() } else if coin_value.is_positive() { coin_value.clone() } else { transferred_cost.clone() };
            // D-TAX-SWAP-FEE: when no tranche took any units, the fee joins the cash lot (Rails dropped it).
            if !tranches.is_empty() && !rest.is_positive() { scale = (&scale + &fee_cost)?; }
            let cpu = unit(&(&cash + &scale)?, &lot_amount)?;
            let lot_assumed = assumed || (rest.is_zero() && tranches.iter().any(|t| t.basis_assumed));
            self.open_swap_lot(tx, cash_amount, cpu, r.at, lot_assumed, None, Dec::zero())?;
        }
        // A tranche can predate everything already held, and the lots are read in order.
        self.store(&r.exchange, &tx.asset).make_contiguous().sort_by_key(|l| l.date.0);
        // D-TAX-SWAP-FEE-EXHAUSTED, Austria: a same-coin leg of this swap that came first and was all fee joins this pool.
        for (_, basis, assumed) in key.and_then(|k| self.held_back.remove(&(k, tx.asset.clone()))).unwrap_or_default() { self.retain(tx, &basis, assumed)?; }
        Ok(())
    }

    /// D-TAX-SWAP-FEE-EXHAUSTED: the fee took every unit received, so no lot carries `basis`. Austria: the fee's units left
    /// the pool and their cost stays in it (Rz 6178ad), on the wallet's first new-stock lot of the coin, as `shed`; with
    /// none, and elsewhere, it is listed (no authority makes it a loss) and accounted for as consumed.
    /// In Austria a same-coin leg of the same swap still to come keeps it (`held_back`, either order gives one pool).
    fn exhaust(&mut self, tx: &Tx, basis: Dec, assumed: bool) -> Result<(), FiguresError> {
        let r = &tx.row;
        if self.austrian(r.at) {
            if self.retain(tx, &basis, assumed)? { return Ok(()); }
            if let Some(k) = group_key(r) {
                self.held_back.entry((k, tx.asset.clone())).or_default().push((r.uid, basis, assumed));
                return Ok(());
            }
        }
        self.list_exhausted(r.uid, basis)
    }

    /// Austria: `basis` onto the wallet's first new-stock lot of the coin, keeping its assumption; false with none.
    fn retain(&mut self, tx: &Tx, basis: &Dec, assumed: bool) -> Result<bool, FiguresError> {
        let Some(from) = self.o.austria.map(|a| a.new_stock_from) else { return Ok(false) };
        let k = self.key(&tx.row.exchange, &tx.asset);
        let Some(l) = self.lots.entry(k).or_default().iter_mut().find(|l| l.date.0 >= from.0 && l.amount.is_positive()) else { return Ok(false) };
        l.cost_per_unit = (&(&l.amount * &l.cost_per_unit)? + basis)?.div(&l.amount)?;
        l.basis_assumed |= assumed;
        Ok(true)
    }

    fn list_exhausted(&mut self, uid: usize, basis: Dec) -> Result<(), FiguresError> {
        self.consumed = (&self.consumed + &basis)?;
        self.exhausted.push((uid, basis));
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn open_swap_lot(&mut self, tx: &Tx, amount: Dec, cost_per_unit: Dec, date: At, basis_assumed: bool, holding_start: Option<At>, unpriced: Dec) -> Result<(), FiguresError> {
        let r = &tx.row;
        let mut lot = Lot { amount, cost_per_unit, date, basis_assumed, unpriced, holding_start: None, technical_start: None, averaged: false, mined: false,
                            fenced: Dec::zero(), fenced_units: Dec::zero() };
        if self.o.swap_resets_holding_period {
            lot.holding_start = Some(r.at);
            let held_since = holding_start.unwrap_or(date);
            if self.technical_stablecoin_swap(r, held_since) { lot.technical_start = Some(held_since); }
        } else if holding_start.is_some() {
            lot.holding_start = holding_start;
        }
        let k = self.key(&r.exchange, &tx.asset);
        self.receive(k, lot)
    }

    /// `technical_stablecoin_swap?`.
    fn technical_stablecoin_swap(&self, r: &Row, held_since: At) -> bool {
        stable(&r.base) && self.o.holding.is_some_and(|rule| rule(held_since, r.at))
    }

    /// `transfer_swap_out`: the lots an out-leg took handed to its group as tranches, weighted by the leg's worth.
    fn transfer_swap_out(&mut self, tx: &Tx) -> Result<(), FiguresError> {
        let r = &tx.row;
        let k = self.key(&r.exchange, &tx.asset);
        let order = self.order();
        let (mut tranches, held) = dequeue_tranches(self.lots.entry(k).or_default(), &r.amount, order)?;
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
        // D-TAX-AT-OLD-SWAP: old stock enters the swap at the value of the units given, dated at the swap (Rz 6103k Bsp. 2).
        if let Some(a) = self.o.austria.filter(|_| self.austrian(r.at)) {
            for t in tranches.iter_mut().filter(|t| t.date.0 < a.new_stock_from.0) {
                self.consumed = (&self.consumed + &t.cost)?;
                t.cost = if r.amount.is_positive() { (&r.fiat_value * &t.amount)?.div(&r.amount)? } else { Dec::zero() };
                (t.date, t.basis_assumed, t.holding_start) = (r.at, r.price_missing, None);
                t.unpriced = if value_missing(r) { t.amount.clone() } else { Dec::zero() };
            }
        }
        let covered = if held < r.amount { held } else { r.amount.clone() };
        let uncovered = (&r.amount - &covered)?;
        if uncovered.is_positive() {
            tranches.push(Tranche { amount: uncovered.clone(), cost: Dec::zero(), date: r.at, basis_assumed: true, unpriced: uncovered,
                                    holding_start: None, technical_start: None, weight: Dec::zero(), averaged: false, mined: false,
                                    fenced: Dec::zero(), recent: false });
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
        let order = self.order();
        let (tranches, _) = dequeue_tranches(self.lots.entry(k.clone()).or_default(), amount, order)?;
        self.consumed = (&self.consumed + &sum(tranches.iter().map(|t| &t.cost))?)?;
        Ok(())
    }
}

fn value_missing(r: &Row) -> bool { r.value_missing.unwrap_or(r.price_missing) }

/// D-TAX-AT-AVERAGE: two lots as one, units and cost summed, dated at the earlier.
fn merge(a: &Lot, b: Lot) -> Result<Lot, FiguresError> {
    let amount = (&a.amount + &b.amount)?;
    let cost = (&(&a.amount * &a.cost_per_unit)? + &(&b.amount * &b.cost_per_unit)?)?;
    Ok(Lot { cost_per_unit: unit(&cost, &amount)?, date: if b.date.0 < a.date.0 { b.date } else { a.date }, basis_assumed: a.basis_assumed || b.basis_assumed,
             unpriced: (&a.unpriced + &b.unpriced)?, amount, holding_start: None, technical_start: None, averaged: true, mined: a.mined || b.mined,
             fenced: (&a.fenced + &b.fenced)?, fenced_units: (&a.fenced_units + &b.fenced_units)? })
}

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
            l.fenced_units = (&l.fenced_units * &factor)?;
        }
    } else if amount.is_negative() && !factor.is_positive() {
        lots.clear();
    } else if amount.is_positive() && pool.is_zero() {
        lots.push_back(Lot { amount: amount.clone(), cost_per_unit: Dec::zero(), date: at, basis_assumed: true, unpriced: amount.clone(),
                             holding_start: None, technical_start: None, averaged: false, mined: false, fenced: Dec::zero(), fenced_units: Dec::zero() });
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

/// `settle_in_cents`: every row but the last cut to the cent, the last taking what is left; the fee off the gain only
/// where `deducted` (D-TAX-AT-SALE-FEE).
fn settle_in_cents(rows: &mut [Disposal], proceeds: &Dec, fee: &Dec, deducted: bool) -> Result<(), FiguresError> {
    let basis = sum(rows.iter().map(|r| &r.cost_basis))?;
    let off = |f: &Dec| if deducted { f.clone() } else { Dec::zero() };
    let mut left = [proceeds.clone(), fee.clone(), basis.clone(), (&(proceeds - &basis)? - &off(fee))?.round(CENTS)?];
    let count = rows.len();
    for (n, row) in rows.iter_mut().enumerate() {
        if n == count - 1 {
            [row.proceeds, row.fee, row.cost_basis, row.gain_loss] = left.clone();
        } else {
            row.proceeds = row.proceeds.round(CENTS)?; row.fee = row.fee.round(CENTS)?; row.cost_basis = row.cost_basis.round(CENTS)?;
            row.gain_loss = (&(&row.proceeds - &row.cost_basis)? - &off(&row.fee))?;
        }
        let taken = [&row.proceeds, &row.fee, &row.cost_basis, &row.gain_loss];
        for (l, t) in left.iter_mut().zip(taken) { *l = (&*l - t)?; }
    }
    Ok(())
}

/// The lots that make up `amount` in `order`, split where a lot is only partly taken (its unpriced units and its fence in
/// proportion), and what was held before. `Lifo` hands the parts back oldest first; `Recent` the recent ones first.
fn take(lots: &mut VecDeque<Lot>, amount: &Dec, order: Order) -> Result<(Vec<Lot>, Dec), FiguresError> {
    let (parts, held) = take_parts(lots, amount, order)?;
    Ok((parts.into_iter().map(|(_, l)| l).collect(), held))
}

/// `take`, each part with whether it was recent.
fn take_parts(lots: &mut VecDeque<Lot>, amount: &Dec, order: Order) -> Result<(Vec<(bool, Lot)>, Dec), FiguresError> {
    let held = sum(lots.iter().map(|l| &l.amount))?;
    let mut remaining = amount.clone();
    let mut out = vec![];
    // The positions to take from, in turn; a position taken whole is removed after the walk.
    let picks: Vec<usize> = match order {
        Order::Fifo => (0..lots.len()).collect(),
        Order::Lifo => (0..lots.len()).rev().collect(),
        Order::Recent(from, to) => {
            let mut recent: Vec<usize> = (0..lots.len()).filter(|&i| lots[i].date.0 >= from.0 && lots[i].date.0 <= to.0).collect();
            recent.sort_by_key(|&i| std::cmp::Reverse((lots[i].date.0, i)));
            let rest = (0..lots.len()).filter(|i| !recent.contains(i));
            recent.iter().copied().chain(rest).collect()
        }
    };
    let recent_count = match order { Order::Recent(from, to) => lots.iter().filter(|l| l.date.0 >= from.0 && l.date.0 <= to.0).count(), _ => 0 };
    let mut emptied = vec![];
    for (n, i) in picks.into_iter().enumerate() {
        budget::check()?;
        if !remaining.is_positive() { break; }
        let lot = &mut lots[i];
        let part = if remaining < lot.amount { remaining.clone() } else { lot.amount.clone() };
        let share = |v: &Dec| -> Result<Dec, FiguresError> { if lot.amount.is_positive() { Ok((v * &part)?.div(&lot.amount)?) } else { Ok(Dec::zero()) } };
        let (unpriced, fenced, fenced_units) = (share(&lot.unpriced)?, share(&lot.fenced)?, share(&lot.fenced_units)?);
        out.push((n < recent_count, Lot { amount: part.clone(), unpriced: unpriced.clone(), fenced: fenced.clone(), fenced_units: fenced_units.clone(), ..lot.clone() }));
        if lot.amount <= remaining {
            remaining = (&remaining - &lot.amount)?;
            emptied.push(i);
        } else {
            lot.amount = (&lot.amount - &remaining)?;
            lot.unpriced = (&lot.unpriced - &unpriced)?;
            lot.fenced = (&lot.fenced - &fenced)?;
            lot.fenced_units = (&lot.fenced_units - &fenced_units)?;
            remaining = Dec::zero();
        }
    }
    emptied.sort_unstable();
    for i in emptied.into_iter().rev() { lots.remove(i); }
    if order == Order::Lifo { out.reverse(); }
    Ok((out, held))
}

/// `dequeue_tranches`.
fn dequeue_tranches(lots: &mut VecDeque<Lot>, amount: &Dec, order: Order) -> Result<(Vec<Tranche>, Dec), FiguresError> {
    let (taken, held) = take_parts(lots, amount, order)?;
    let tranches = taken.into_iter().map(|(recent, l)| Ok(Tranche { cost: (&l.amount * &l.cost_per_unit)?, amount: l.amount, date: l.date,
                                                         basis_assumed: l.basis_assumed, unpriced: l.unpriced, holding_start: l.holding_start,
                                                         technical_start: l.technical_start, weight: Dec::zero(), averaged: l.averaged, mined: l.mined,
                                                         fenced: l.fenced, recent }))
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
        // D-TAX-BG-AVERAGE: averaging conserves units and cost across partial sales, later buys, fees and transfers.
        let cases: [(bool, Option<At>, bool); 4] = [(true, None, false), (false, None, false), (true, Some(at(5)), false), (true, None, true)];
        for (taxable, per_venue_from, average_all) in cases {
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
            let mut f = Fifo::new(Options { swaps_taxable: taxable, stablecoin_as_fiat: false, holding: Some(&holding), swap_resets_holding_period: false, per_venue_from, swap_cost_to_disposed_from: None, austria: None, average_all, row_per_lot: false, deposit_cost_zero: false, tag_mining: false, snapshot_at: None, lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None });
            f.calculate(&rows).unwrap();
            let used = sum(f.disposals.iter().map(|x| &x.cost_basis)).unwrap();
            let total = (&(&used + &f.consumed).unwrap() + &remaining(&f)).unwrap();
            let gap = (&total - &acquired).unwrap();
            assert!(gap.round(9).unwrap().is_zero(), "taxable {taxable}, per venue {per_venue_from:?}, average {average_all}: {} off", gap.to_s_f());
        }
    }

    fn swap_leg(uid: usize, kind: Kind, base: &str, amount: &str, value: &str, day: i64) -> Tx {
        let mut t = row(uid, kind, base, amount, value, day, "a");
        t.row.group = Some("g".into());
        t.row.quote = None;
        t
    }
    fn fee_in_own_coin(t: &mut Tx, amount: &str, value: &str) {
        t.row.fee_currency = Some(t.row.base.clone());
        t.row.fee_amount = Some(d(amount));
        t.row.fee_fiat_value = d(value);
        t.fee_asset = Some(t.asset.clone());
    }
    fn not_taxable() -> Options<'static> {
        Options { swaps_taxable: false, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None, swap_cost_to_disposed_from: None, austria: None, average_all: false, row_per_lot: false, deposit_cost_zero: false, tag_mining: false, snapshot_at: None, lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None }
    }

    /// D-TAX-SWAP-FEE-EXHAUSTED: a swap-in whose own-coin fee takes every unit received opens no lot; the basis it would
    /// have carried (two unequal lots here) is listed, never a loss, and stays accounted for.
    #[test]
    fn a_swap_fee_taking_everything_lists_the_basis_it_could_not_carry() {
        let mut rows = vec![row(0, Kind::Buy, "BTC", "1", "100", 0, "a"), row(1, Kind::Buy, "BTC", "1", "300", 1, "a"),
                            swap_leg(2, Kind::SwapOut, "BTC", "2", "400", 2), swap_leg(3, Kind::SwapIn, "ETH", "0.05", "400", 2)];
        fee_in_own_coin(&mut rows[3], "0.05", "400");
        let mut f = Fifo::new(not_taxable());
        f.calculate(&rows).unwrap();
        assert!(f.disposals.is_empty());
        assert_eq!(f.exhausted.iter().map(|(u, c)| (*u, c.to_s_f())).collect::<Vec<_>>(), vec![(3, "400.0".to_string())]);
        assert_eq!(remaining(&f).to_s_f(), "0.0");
        assert_eq!(f.consumed.to_s_f(), "400.0");
    }

    /// One exhausted leg among two in-coins (shares 25% / 75% by value): its share is listed, the other keeps its own.
    #[test]
    fn only_the_exhausted_legs_share_is_listed() {
        let mut rows = vec![row(0, Kind::Buy, "BTC", "1", "400", 0, "a"), swap_leg(1, Kind::SwapOut, "BTC", "1", "400", 2),
                            swap_leg(2, Kind::SwapIn, "ETH", "1", "100", 2), swap_leg(3, Kind::SwapIn, "SOL", "3", "300", 2)];
        fee_in_own_coin(&mut rows[2], "1", "100");
        let mut f = Fifo::new(not_taxable());
        f.calculate(&rows).unwrap();
        assert_eq!(f.exhausted.iter().map(|(u, c)| (*u, c.to_s_f())).collect::<Vec<_>>(), vec![(2, "100.0".to_string())]);
        assert_eq!(remaining(&f).to_s_f(), "300.0");
    }

    /// D-TAX-IT-DEPOSIT-ZERO: under `deposit_cost_zero` a swap-in with nothing to carry opens at cost 0, not assumed.
    #[test]
    fn an_orphan_swap_in_costs_zero_under_deposit_cost_zero() {
        let rows = vec![swap_leg(0, Kind::SwapIn, "ETH", "1", "2000", 0), row(1, Kind::Sell, "ETH", "1", "2500", 1, "a")];
        let mut f = Fifo::new(Options { deposit_cost_zero: true, lifo: true, ..not_taxable() });
        f.calculate(&rows).unwrap();
        let d = &f.disposals[0];
        assert_eq!((d.cost_basis.to_s_f(), d.gain_loss.to_s_f(), d.complete, d.incomplete), ("0.0".into(), "2500.0".into(), true, false));
        let mut f = Fifo::new(not_taxable());
        f.calculate(&rows).unwrap();
        assert_eq!(f.disposals[0].cost_basis.to_s_f(), "2000.0", "elsewhere at market value, assumed");
    }

    fn austria() -> Options<'static> {
        let a = Austria { new_stock_from: at(-1000), regime_from: at(-500), average_from: at(-100) };
        Options { austria: Some(a), ..not_taxable() }
    }

    /// Austria: an exhausted leg's basis stays with the coin's pool (Rz 6178ad), whichever same-coin leg of the swap comes
    /// first: €400 into 1 ETH (all fee) and 3 ETH keeps €400 on the 3 ETH, in both orders, and nothing is listed.
    #[test]
    fn an_austrian_exhausted_leg_joins_its_sibling_in_either_order() {
        for exhausted_first in [true, false] {
            let mut a = swap_leg(2, Kind::SwapIn, "ETH", "1", "100", 2);
            fee_in_own_coin(&mut a, "1", "100");
            let mut b = swap_leg(3, Kind::SwapIn, "ETH", "3", "300", 2);
            if !exhausted_first { b.row.uid = 2; a.row.uid = 3; }
            let (first, second) = if exhausted_first { (a, b) } else { (b, a) };
            let rows = vec![row(0, Kind::Buy, "BTC", "1", "400", 0, "a"), swap_leg(1, Kind::SwapOut, "BTC", "1", "400", 2), first, second];
            let mut f = Fifo::new(austria());
            f.calculate(&rows).unwrap();
            assert!(f.exhausted.is_empty(), "exhausted first {exhausted_first}");
            assert_eq!(remaining(&f).round(9).unwrap().to_s_f(), "400.0", "exhausted first {exhausted_first}");
        }
    }

    /// Austria: two same-coin legs both all fee, and no pool: each is listed with its own basis.
    #[test]
    fn two_exhausted_austrian_legs_are_listed_apart() {
        let mut a = swap_leg(2, Kind::SwapIn, "ETH", "1", "100", 2);
        fee_in_own_coin(&mut a, "1", "100");
        let mut b = swap_leg(3, Kind::SwapIn, "ETH", "3", "300", 2);
        fee_in_own_coin(&mut b, "3", "300");
        let rows = vec![row(0, Kind::Buy, "BTC", "1", "400", 0, "a"), swap_leg(1, Kind::SwapOut, "BTC", "1", "400", 2), a, b];
        let mut f = Fifo::new(austria());
        f.calculate(&rows).unwrap();
        assert_eq!(f.exhausted.iter().map(|(u, c)| (*u, c.round(9).unwrap().to_s_f())).collect::<Vec<_>>(),
                   vec![(2, "100.0".to_string()), (3, "300.0".to_string())]);
    }

    /// Austria: a basis that was assumed stays marked so when it joins a complete pool.
    #[test]
    fn an_austrian_retained_basis_keeps_its_assumption() {
        let mut btc = row(1, Kind::Buy, "BTC", "1", "400", 1, "a");
        btc.row.price_missing = true;
        let mut leg = swap_leg(3, Kind::SwapIn, "ETH", "0.05", "400", 2);
        fee_in_own_coin(&mut leg, "0.05", "400");
        let rows = vec![row(0, Kind::Buy, "ETH", "1", "200", 0, "a"), btc, swap_leg(2, Kind::SwapOut, "BTC", "1", "400", 2), leg];
        let mut f = Fifo::new(austria());
        f.calculate(&rows).unwrap();
        assert!(f.exhausted.is_empty());
        let eth: Vec<&Lot> = f.lots().filter(|(k, _)| k.1 == "ETH").flat_map(|(_, l)| l.iter()).collect();
        assert_eq!(eth.len(), 1);
        assert_eq!((&eth[0].amount * &eth[0].cost_per_unit).unwrap().to_s_f(), "600.0");
        assert!(eth[0].basis_assumed);
    }

    /// A fee short of the whole receipt changes nothing: the net units carry the basis.
    #[test]
    fn a_fee_short_of_the_receipt_lists_nothing() {
        let mut rows = vec![row(0, Kind::Buy, "BTC", "1", "400", 0, "a"), swap_leg(1, Kind::SwapOut, "BTC", "1", "400", 2),
                            swap_leg(2, Kind::SwapIn, "ETH", "1", "400", 2)];
        fee_in_own_coin(&mut rows[2], "0.5", "200");
        let mut f = Fifo::new(not_taxable());
        f.calculate(&rows).unwrap();
        assert!(f.exhausted.is_empty());
        assert_eq!(remaining(&f).to_s_f(), "400.0");
    }

    #[test]
    fn a_lost_coin_takes_its_lots_with_no_row_and_an_oversold_sale_is_incomplete() {
        let rows = [row(0, Kind::Buy, "BTC", "1", "100", 0, "a"), row(1, Kind::Buy, "BTC", "1", "500", 1, "a"), row(2, Kind::Lost, "BTC", "1", "0", 2, "a"),
                    row(3, Kind::Sell, "BTC", "1.5", "900", 3, "a")];
        let mut f = Fifo::new(Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None, swap_cost_to_disposed_from: None, austria: None, average_all: false, row_per_lot: false, deposit_cost_zero: false, tag_mining: false, snapshot_at: None, lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None });
        f.calculate(&rows).unwrap();
        assert_eq!(f.disposals.len(), 1);
        let x = &f.disposals[0];
        assert_eq!((x.cost_basis.to_s_f(), x.uncovered.to_s_f(), x.complete, x.incomplete), ("500.0".into(), "0.5".into(), false, true));
        assert_eq!(f.consumed.to_s_f(), "100.0");
    }

    /// D-TAX-BG-AVERAGE: a sale takes the average of every unit held, and a later buy joins it; D-TAX-BG-DEPOSIT-ZERO: an
    /// unlinked deposit adds units at no cost, neither assumed nor unpriced.
    #[test]
    fn the_bulgarian_average_and_the_deposit_at_zero() {
        let mut rows = vec![row(0, Kind::Buy, "BTC", "1", "30000", 0, "a"), row(1, Kind::Buy, "BTC", "1", "40000", 1, "b"),
                            row(2, Kind::Sell, "BTC", "1", "50000", 2, "a"), row(3, Kind::Buy, "BTC", "1", "50000", 3, "a"),
                            row(4, Kind::Deposit, "BTC", "1", "0", 4, "a"), row(5, Kind::Sell, "BTC", "3", "90000", 5, "b")];
        rows[4].row.price_missing = true;
        rows[4].row.value_missing = Some(true);
        let mut f = Fifo::new(Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None,
                                        swap_cost_to_disposed_from: None, austria: None, average_all: true, row_per_lot: false, deposit_cost_zero: true, tag_mining: false, snapshot_at: None, lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None });
        f.calculate(&rows).unwrap();
        let got: Vec<(String, bool)> = f.disposals.iter().map(|x| (x.cost_basis.round(2).unwrap().to_s_f(), x.incomplete)).collect();
        // 35,000; then 1 @ 35,000 + 1 @ 50,000 + 1 @ 0 = 85,000 for 3.
        assert_eq!(got, [("35000.0".to_string(), false), ("85000.0".to_string(), false)]);
    }

    /// D-TAX-DK-PER-PART: one row per FIFO part with sub-cent residues, opposite signs and an uncovered part; every row
    /// reconciles in cents, and the parts add up to the sale's rounded totals.
    #[test]
    fn a_sale_per_lot_reconciles_every_row_in_cents() {
        let mut rows = vec![row(0, Kind::Buy, "BTC", "1", "100.003", 0, "a"), row(1, Kind::Buy, "BTC", "1", "300.004", 1, "a"),
                            row(2, Kind::Mining, "BTC", "0.5", "25.0005", 2, "a"), row(3, Kind::Sell, "BTC", "2.7", "540.0054", 3, "a")];
        rows[3].row.fee_fiat_value = d("1.001");
        let mut f = Fifo::new(Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None,
                                        swap_cost_to_disposed_from: None, austria: None, average_all: false, row_per_lot: true, deposit_cost_zero: false, tag_mining: true, snapshot_at: None, lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None });
        f.calculate(&rows).unwrap();
        let x = &f.disposals;
        assert_eq!(x.len(), 4);
        for r in x {
            for v in [&r.proceeds, &r.cost_basis, &r.fee, &r.gain_loss] { assert_eq!(v.round(2).unwrap(), *v); }
            assert_eq!(r.gain_loss, (&(&r.proceeds - &r.cost_basis).unwrap() - &r.fee).unwrap());
        }
        let total = |g: fn(&Disposal) -> &Dec| sum(x.iter().map(g)).unwrap().to_s_f();
        assert_eq!((total(|r| &r.proceeds), total(|r| &r.cost_basis), total(|r| &r.fee)), ("540.01".into(), "425.01".into(), "1.0".into()));
        assert!(x[0].gain_loss.is_positive() && x[1].gain_loss.is_negative());
        assert_eq!((x[2].mined, x[0].mined, x[3].acquired, x[3].uncovered.to_s_f(), x[3].complete, x[0].complete), (true, false, None, "0.2".into(), false, true));
    }

    /// D-TAX-DK-DEPOSIT-ZERO: an unlinked deposit opens at cost 0, its sale complete; the year-start units are recorded.
    #[test]
    fn an_unlinked_deposit_at_zero_and_the_snapshot() {
        let mut rows = vec![row(0, Kind::Deposit, "ETH", "1", "2000", 0, "a"), row(1, Kind::Buy, "USDT", "10", "10", 1, "a"), row(2, Kind::Sell, "ETH", "1", "2500", 5, "a")];
        rows[0].row.value_missing = Some(true);
        let mut f = Fifo::new(Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None,
                                        swap_cost_to_disposed_from: None, austria: None, average_all: false, row_per_lot: true, deposit_cost_zero: true, tag_mining: false, snapshot_at: Some(at(3)), lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None });
        f.calculate(&rows).unwrap();
        let x = &f.disposals[0];
        assert_eq!((x.cost_basis.to_s_f(), x.gain_loss.to_s_f(), x.complete, x.incomplete), ("0.0".into(), "2500.0".into(), true, false));
        let snap = f.snapshot.unwrap();
        assert_eq!(snap.iter().map(|(s, u)| format!("{s} {}", u.to_s_f())).collect::<Vec<_>>(), ["ETH 1.0", "USDT 10.0"]);
    }
    fn plain() -> Options<'static> {
        Options { swaps_taxable: true, stablecoin_as_fiat: false, holding: None, swap_resets_holding_period: false, per_venue_from: None,
                  swap_cost_to_disposed_from: None, austria: None, average_all: false, row_per_lot: false, deposit_cost_zero: false, tag_mining: false,
                  snapshot_at: None, lifo: false, fees_ignored_from: None, swaps_taxable_before: None, ireland: None }
    }
    fn cents(v: &Dec) -> String { v.round(2).unwrap().to_s_f() }
    fn ie_from(a: At) -> At { At(a.0 - 28 * 86_400_000_000_000) }
    fn ie_until(a: At) -> At { At(a.0 + 29 * 86_400_000_000_000) }
    fn ireland() -> Options<'static> { Options { ireland: Some(Ireland { from: &ie_from, until: &ie_until }), ..plain() } }

    /// Italy: the newest lot leaves first, the parts come back oldest first, and the row is dated at its newest part.
    #[test]
    fn lifo_takes_the_newest_lot_first() {
        let rows = vec![row(0, Kind::Buy, "BTC", "1", "10000", 0, "a"), row(1, Kind::Buy, "BTC", "1", "30000", 1, "a"),
                        row(2, Kind::Sell, "BTC", "1.5", "30000", 2, "a")];
        let mut f = Fifo::new(Options { lifo: true, ..plain() });
        f.calculate(&rows).unwrap();
        assert_eq!((cents(&f.disposals[0].cost_basis), f.disposals[0].acquired), ("35000.0".to_string(), Some(at(1))));
        assert_eq!(cents(&remaining(&f)), "5000.0");
    }

    /// D-TAX-IE-4WEEK: every acquisition of the 4 weeks before, newest first, then FIFO; none dated after the disposal.
    #[test]
    fn the_four_weeks_before_go_first_newest_first() {
        let rows = vec![row(0, Kind::Buy, "BTC", "1", "10000", 0, "a"), row(1, Kind::Buy, "BTC", "0.5", "15000", 50, "a"),
                        row(2, Kind::Buy, "BTC", "0.5", "20000", 59, "a"), row(3, Kind::Sell, "BTC", "1.2", "43200", 69, "a")];
        let mut f = Fifo::new(ireland());
        f.calculate(&rows).unwrap();
        let d = &f.disposals[0];
        assert_eq!((cents(&d.cost_basis), d.recent, d.acquired), ("37000.0".to_string(), true, Some(at(59))));
        assert_eq!(cents(&remaining(&f)), "8000.0");
    }

    /// D-TAX-IE-RING-FENCE: a loss followed by a purchase within 4 weeks is restricted; a later sale uses it only up to the
    /// gain of the part bought again; the rest is not usable. Basis is untouched by the fence.
    #[test]
    fn a_restricted_loss_is_used_by_its_own_part_only() {
        let sale = |price: &str| vec![row(0, Kind::Buy, "ETH", "1", "1000", 0, "a"), row(1, Kind::Buy, "ETH", "1", "5000", 100, "a"),
                                      row(2, Kind::Sell, "ETH", "1", "3000", 100, "a"), row(3, Kind::Buy, "ETH", "1", "3000", 108, "a"),
                                      row(4, Kind::Sell, "ETH", "2", price, 191, "a")];
        let mut f = Fifo::new(ireland());
        f.calculate(&sale("6000")).unwrap();
        let (loss, later) = (&f.disposals[0], &f.disposals[1]);
        assert_eq!((cents(&loss.gain_loss), cents(&loss.restricted)), ("-2000.0".to_string(), "2000.0".to_string()));
        assert_eq!((cents(&later.gain_loss), cents(&later.released), cents(&later.unusable)), ("2000.0".to_string(), "0.0".to_string(), "2000.0".to_string()));
        let mut f = Fifo::new(ireland());
        f.calculate(&sale("10000")).unwrap();
        assert_eq!((cents(&f.disposals[1].released), cents(&f.disposals[1].unusable)), ("2000.0".to_string(), "0.0".to_string()));
        // Outside the window (day 29 after): nothing restricted.
        let mut rows = sale("6000");
        rows[3].row.at = at(129);
        let mut f = Fifo::new(ireland());
        f.calculate(&rows).unwrap();
        assert!(f.disposals[0].restricted.is_zero());
    }

    /// D-TAX-IE-4WEEK: a CFEE walked after its day's fills takes effect then: it is matched with the purchase it follows
    /// (no loss, nothing restricted), even though its own time is earlier.
    #[test]
    fn a_fee_walked_after_its_fills_takes_effect_after_them() {
        let mut rows = vec![row(0, Kind::Buy, "ETH", "1", "4000", 0, "a"), row(1, Kind::Buy, "ETH", "0.1", "200", 400, "a"),
                            row(2, Kind::Fee, "ETH", "0.1", "200", 399, "a")];
        rows[2].row.at = At(at(400).0 - 3_600_000_000_000);
        let mut f = Fifo::new(ireland());
        f.calculate(&rows).unwrap();
        let fee = &f.disposals[0];
        assert_eq!((cents(&fee.cost_basis), cents(&fee.gain_loss), fee.recent, fee.restricted.is_zero()), ("200.0".to_string(), "0.0".to_string(), true, true));
    }

    /// D-TAX-IE-RING-FENCE: a purchase at the very instant of the loss, walked after it, buys again.
    #[test]
    fn a_purchase_at_the_same_instant_after_the_loss_restricts_it() {
        let rows = vec![row(0, Kind::Buy, "ETH", "1", "4000", 0, "a"), row(1, Kind::Sell, "ETH", "1", "2000", 100, "a"), row(2, Kind::Buy, "ETH", "1", "2100", 100, "a")];
        let mut f = Fifo::new(ireland());
        f.calculate(&rows).unwrap();
        assert_eq!(cents(&f.disposals[0].restricted), "2000.0");
    }

    /// D-TAX-IE-RING-FENCE: a whole restriction is the whole loss, to the cent (6 units, a loss of 2.015).
    #[test]
    fn a_whole_restriction_keeps_the_last_cent() {
        let rows = vec![row(0, Kind::Buy, "XRP", "6", "6", 0, "a"), row(1, Kind::Sell, "XRP", "6", "3.985", 100, "a"), row(2, Kind::Buy, "XRP", "6", "3.9", 101, "a")];
        let mut f = Fifo::new(ireland());
        f.calculate(&rows).unwrap();
        assert_eq!(f.disposals[0].restricted.to_s_f(), (&Dec::zero() - &f.disposals[0].gain_loss).unwrap().to_s_f());
    }

    /// D-TAX-IT-FEES: an own-coin fee's units leave at their share of the purchase's cost; a fiat fee joins no cost.
    #[test]
    fn ignored_fees_leave_at_their_share() {
        let mut rows = vec![row(0, Kind::Buy, "BTC", "1", "10000", 0, "a"), row(1, Kind::Buy, "ETH", "1", "2000", 1, "a")];
        (rows[0].row.fee_currency, rows[0].row.fee_amount) = (Some("BTC".into()), Some(d("0.01")));
        (rows[1].row.fee_currency, rows[1].row.fee_amount, rows[1].row.fee_fiat_value) = (Some("EUR".into()), Some(d("20")), d("20"));
        let mut f = Fifo::new(Options { fees_ignored_from: Some(at(0)), ..plain() });
        f.calculate(&rows).unwrap();
        assert_eq!((cents(&remaining(&f)), cents(&f.consumed)), ("11900.0".to_string(), "100.0".to_string()));
    }

}
