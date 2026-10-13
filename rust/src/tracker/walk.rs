//! Tracker::Ledger's walk (app/models/tracker/ledger.rb) over every venue: the rows priced (Tax::PriceService#enrich,
//! the cash beside a grouped leg and a fee moved onto its coin leg included), opened with what must have been held,
//! walked twice through FIFO lots with the tracker's extensions (Tracker::Ledger::Engine over Tax::Methods::Fifo with
//! AcquisitionFee and ReturnOfCapital): account-wide for the loss sales, and located (each venue its own lots, a linked
//! transfer carrying its lots across) for everything else; and read for money in through the cash book
//! (Tracker::CashBook) with the inferred funding of venues that do not lend cash (Tracker::UnfundedCash).
//!
//! What this walk states is what the snapshots, the figures and the arming read: per venue and for the whole account,
//! positions, money in (and each row's term of it), the cash and the basis it carried, whether the figures are
//! complete, and the loss sales. Round trips, received, realised and the unpriced proceeds are the pages'.
use super::prices::PriceService;
use super::rows::{Kind, Stored};
use super::{cash, fiat, stable};
use crate::figures::{at::At, dec::Dec, FiguresError};
use chrono::NaiveDate;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Tracker::UnfundedCash::LENDS_CASH.
pub const LENDS_CASH: [&str; 2] = ["alpaca", "ibkr"];

/// An enriched row: what Tax::PriceService#enrich hands the engines, and the walk's own marks. `uid` is the row's
/// identity (Rails keys several tables by the row object).
#[derive(Clone, Debug)]
pub struct Row {
    pub uid: usize,
    pub kind: Kind,
    pub base: String,
    pub amount: Dec,
    pub quote: Option<String>,
    pub quote_amount: Option<Dec>,
    pub fiat_value: Dec,
    pub fee_fiat_value: Dec,
    pub fee_currency: Option<String>,
    pub fee_amount: Option<Dec>,
    pub at: At,
    pub group: Option<String>,
    pub price_missing: bool,
    /// None on a row built outside the price service (an opening): `value_missing?` then reads `price_missing`.
    pub value_missing: Option<bool>,
    pub exchange: String,
    pub linked: bool,
    pub transfer_fee: Option<Dec>,
    pub swap_fiat_cost: Option<Dec>,
    pub swap_stable_cost: Option<Dec>,
    pub per_share: Option<Dec>,
    pub to_exchange: Option<String>,
    pub orphan: bool,
    /// An opening balance the walk booked (`open_with_what_must_have_been_held`), not a stored row.
    pub opening: bool,
}

impl Row {
    fn value_missing(&self) -> bool { self.value_missing.unwrap_or(self.price_missing) }
    fn fee(&self) -> Dec { self.fee_amount.clone().unwrap_or_else(Dec::zero) }
    fn fee_is(&self, currency: &str) -> bool { self.fee_currency.as_deref() == Some(currency) }
    fn fee_present(&self) -> bool { self.fee_currency.as_deref().is_some_and(|c| !c.trim().is_empty()) }
}

/// One term of money in, as the chart reads it day by day (Tracker::Ledger::Term).
#[derive(Clone, Debug)]
pub struct Term { pub at: At, pub amount: Dec, pub complete: bool, pub opens: Option<(String, Dec)>, pub exchange: String }

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

/// Every scope from one walk (`Ledger.scopes`): each venue's summary by exchange id, in the order the walk met the
/// venues, the whole account's, and the terms.
#[derive(Clone, Debug)]
pub struct Walked { pub venues: Vec<(i64, Summary)>, pub whole: Summary, pub terms: Vec<Term> }

impl Walked {
    /// The venue's scope, or the empty ledger Rails reads for a venue the walk has no rows of.
    pub fn venue(&self, id: i64) -> Option<&Summary> { self.venues.iter().find(|(v, _)| *v == id).map(|(_, s)| s) }
}

fn sum(values: impl Iterator<Item = Result<Dec, crate::figures::num::NumError>>) -> Result<Dec, FiguresError> {
    let mut total = Dec::zero();
    for v in values { total = (&total + &v?)?; }
    Ok(total)
}
/// Ruby's `[a, b].max` and `.min`: the first on a tie.
fn max(a: &Dec, b: &Dec) -> Dec { if b > a { b.clone() } else { a.clone() } }
fn min(a: &Dec, b: &Dec) -> Dec { if b < a { b.clone() } else { a.clone() } }
fn abs(a: &Dec) -> Dec { if a.is_negative() { a.neg() } else { a.clone() } }

// ---------------------------------------------------------------------------------------------------------------
// Enrichment (Tax::PriceService#enrich)
// ---------------------------------------------------------------------------------------------------------------

/// What `cash_leg_context` says about one leg of a group.
#[derive(Clone, Debug, Default)]
struct Leg { kind: Option<Kind>, quote: Option<String>, fiat_value: Option<Dec>, price_missing: Option<bool>, swap_fiat: Option<Dec>, swap_stable: Option<Dec> }

/// `shares`: by amount while the legs are one asset; else evenly, and every leg says so.
fn shares(legs: &[&Stored]) -> Result<Vec<(i64, Dec, bool)>, FiguresError> {
    let one_asset = legs.iter().map(|t| t.base.as_str()).collect::<HashSet<_>>().len() == 1;
    let total = sum(legs.iter().map(|t| Ok(t.amount.clone())))?;
    legs.iter().map(|t| Ok(if one_asset && total.is_positive() { (t.id, t.amount.div(&total)?, false) }
                           else { (t.id, Dec::one().div(&Dec::from_i64(legs.len() as i64))?, true) })).collect()
}

/// `cash_faces`: [fiat, stablecoin] faces in USD, or None when a leg could not be converted (its warnings taken back).
fn cash_faces(legs: &[&Stored], ps: &mut PriceService) -> Result<Option<(Dec, Dec)>, FiguresError> {
    let kept = ps.warnings.len();
    let (mut f, mut s) = (Dec::zero(), Dec::zero());
    for t in legs {
        if stable(&t.base) { s = (&s + &ps.convert_fiat(&t.amount, "USD", "USD", t.at)?)?; }
        else { f = (&f + &ps.convert_fiat(&t.amount, &t.base, "USD", t.at)?)?; }
    }
    if ps.warnings.len() == kept { return Ok(Some((f, s))); }
    ps.warnings.truncate(kept);
    Ok(None)
}

/// `quote_of`: a fiat currency if any leg carries one, else the first leg's.
fn quote_of(legs: &[&Stored]) -> Option<String> { legs.iter().find(|t| fiat(&t.base)).or(legs.first()).map(|t| t.base.clone()) }

/// `cash_leg_context`: per grouped leg, what the cash beside it says it was worth, and whether a coin-for-coin pair is a
/// swap however the venue booked it.
fn cash_leg_context(rows: &[Stored], ps: &mut PriceService) -> Result<HashMap<i64, Leg>, FiguresError> {
    let mut order: Vec<(i64, String)> = vec![];
    let mut groups: HashMap<(i64, String), Vec<&Stored>> = HashMap::new();
    for t in rows {
        let Some(g) = &t.group else { continue };
        let key = (t.exchange_id, g.clone());
        let legs = groups.entry(key.clone()).or_default();
        if legs.is_empty() { order.push(key); }
        legs.push(t);
    }
    let mut context: HashMap<i64, Leg> = HashMap::new();
    for key in order {
        crate::figures::budget::check()?;
        let legs = &groups[&key];
        if legs.len() < 2 { continue; }
        let (cash_legs, coins): (Vec<&Stored>, Vec<&Stored>) = legs.iter().partition(|t| cash(&t.base));
        let outs: Vec<&Stored> = coins.iter().copied().filter(|t| t.kind.out_leg() && t.quote_amount.is_none()).collect();
        let ins: Vec<&Stored> = coins.iter().copied().filter(|t| t.kind.in_leg() && t.quote_amount.is_none()).collect();
        let cash_out: Vec<&Stored> = cash_legs.iter().copied().filter(|t| t.kind.out_leg()).collect();
        let cash_in: Vec<&Stored> = cash_legs.iter().copied().filter(|t| t.kind.in_leg()).collect();
        if cash_legs.is_empty() {
            if outs.is_empty() || ins.is_empty() { continue; }
            for t in &outs { context.insert(t.id, Leg { kind: Some(Kind::SwapOut), ..Leg::default() }); }
            for t in &ins { context.insert(t.id, Leg { kind: Some(Kind::SwapIn), ..Leg::default() }); }
        } else if !cash_in.is_empty() && !ins.is_empty() {
            for t in &outs { context.insert(t.id, Leg { price_missing: Some(true), quote: quote_of(&cash_in), ..Leg::default() }); }
            for t in &ins { context.insert(t.id, Leg { price_missing: Some(true), ..Leg::default() }); }
        } else {
            if !cash_out.is_empty() && !ins.is_empty() {
                if let Some((f, s)) = cash_faces(&cash_out, ps)? {
                    let mixed = !outs.is_empty();
                    for (id, share, assumed) in shares(&ins)? {
                        context.insert(id, Leg { swap_fiat: Some((&f * &share)?), swap_stable: Some((&s * &share)?),
                                                 fiat_value: if mixed { None } else { Some((&(&f + &s)? * &share)?) },
                                                 price_missing: assumed.then_some(true), ..Leg::default() });
                    }
                }
            }
            if !cash_in.is_empty() && !outs.is_empty() {
                if let Some((f, s)) = cash_faces(&cash_in, ps)? {
                    let quote = quote_of(&cash_in);
                    for (id, share, assumed) in shares(&outs)? {
                        context.insert(id, Leg { fiat_value: Some((&(&f + &s)? * &share)?), quote: quote.clone(), price_missing: Some(assumed), ..Leg::default() });
                    }
                }
            }
        }
    }
    Ok(context)
}

/// `resolve_fiat_value` in USD.
fn fiat_value(t: &Stored, leg: Option<&Leg>, ps: &mut PriceService) -> Result<Dec, FiguresError> {
    if fiat(&t.base) { return Ok(Dec::zero()); }
    if let (Some(q), Some(qa)) = (t.quote.as_deref(), &t.quote_amount) {
        if q == "USD" { return Ok(qa.clone()); }
        if stable(q) { return ps.convert_fiat(qa, "USD", "USD", t.at); }
        if fiat(q) { return ps.convert_fiat(qa, q, "USD", t.at); }
    }
    if let Some(v) = leg.and_then(|l| l.fiat_value.clone()) { return Ok(v); }
    if let Some(price) = &t.stated { return ps.convert_fiat(&(price * &t.amount)?, "USD", "USD", t.at); }
    let price = ps.price_at(&t.base, "USD", t.at, Some(&t.venue))?;
    Ok((&price * &t.amount)?)
}

/// `resolve_fee_fiat_value`.
fn fee_fiat_value(t: &Stored, ps: &mut PriceService) -> Result<Dec, FiguresError> {
    let Some(fee) = t.fee_amount.as_ref().filter(|f| !f.is_zero()) else { return Ok(Dec::zero()) };
    let currency = t.fee_currency.as_deref().unwrap_or("");
    if currency == "USD" { return Ok(fee.clone()); }
    if fiat(currency) { return ps.convert_fiat(fee, currency, "USD", t.at); }
    if stable(currency) { return ps.convert_fiat(fee, "USD", "USD", t.at); }
    let price = ps.price_at(currency, "USD", t.at, Some(&t.venue))?;
    Ok((&price * fee)?)
}

/// `enrich(ordered, currency: 'USD')`, then the venue each linked row's coins went to and the orphan swap legs.
/// `deposits` are the base amounts of every row of the account, for a linked withdrawal's fee.
pub fn enrich(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService) -> Result<Vec<Row>, FiguresError> { enrich_with(stored, deposits, ps, true) }

/// `enrich`, its prefetch left out when `prefetch` is false (the tax report reads stored prices only).
pub fn enrich_with(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService, prefetch: bool) -> Result<Vec<Row>, FiguresError> {
    let legs = cash_leg_context(stored, ps)?;
    if prefetch { ps.prefetch(stored, &|t| legs.get(&t.id).is_some_and(|l| l.fiat_value.is_some()))?; }
    let links: HashMap<i64, i64> = stored.iter().filter_map(|t| t.linked_to.map(|d| (t.id, d))).collect();
    let linked_deposits: HashSet<i64> = links.values().copied().collect();
    let mut rows = Vec::with_capacity(stored.len());
    for (uid, t) in stored.iter().enumerate() {
        crate::figures::budget::check()?;
        let before = ps.warnings.len();
        let leg = legs.get(&t.id);
        let value = if matches!(t.kind, Kind::Withdrawal | Kind::Lost) {
            let kept = ps.warnings.len();
            let v = fiat_value(t, leg, ps)?;
            ps.warnings.truncate(kept);
            v
        } else { fiat_value(t, leg, ps)? };
        let value_missing = ps.warnings.len() > before || leg.and_then(|l| l.price_missing) == Some(true);
        let fee_value = fee_fiat_value(t, ps)?;
        let price_missing = value_missing || ps.warnings.len() > before;
        let transfer_fee = match (t.kind, links.get(&t.id)) {
            (Kind::Withdrawal, Some(d)) => match deposits.get(d) { Some(deposit) => Some((&t.amount - deposit)?), None => None },
            _ => None,
        };
        rows.push(Row {
            uid, kind: leg.and_then(|l| l.kind).unwrap_or(t.kind), base: t.base.clone(), amount: t.amount.clone(),
            quote: leg.and_then(|l| l.quote.clone()).or_else(|| t.quote.clone()), quote_amount: t.quote_amount.clone(),
            fiat_value: value, fee_fiat_value: fee_value, fee_currency: t.fee_currency.clone(), fee_amount: t.fee_amount.clone(), at: t.at,
            group: t.group.clone(), price_missing, value_missing: Some(value_missing), exchange: t.venue.clone(),
            linked: links.contains_key(&t.id) || linked_deposits.contains(&t.id), transfer_fee,
            swap_fiat_cost: leg.and_then(|l| l.swap_fiat.clone()), swap_stable_cost: leg.and_then(|l| l.swap_stable.clone()),
            per_share: t.per_share.clone(), to_exchange: t.to_venue.clone(), orphan: false, opening: false,
        });
    }
    attribute_quote_row_fees(&mut rows);
    mark_orphans(&mut rows);
    Ok(rows)
}

/// `attribute_quote_row_fees`: a fee on a group's one fiat leg moves to its one coin trade leg, when that leg has none.
fn attribute_quote_row_fees(rows: &mut [Row]) {
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() { if let Some(g) = &r.group { groups.entry((r.exchange.clone(), g.clone())).or_default().push(i); } }
    for members in groups.values() {
        if members.len() < 2 { continue; }
        let donors: Vec<usize> = members.iter().copied().filter(|&i| fiat(&rows[i].base) && rows[i].fee_amount.is_some()).collect();
        let targets: Vec<usize> = members.iter().copied().filter(|&i| matches!(rows[i].kind, Kind::Buy | Kind::SwapIn | Kind::Sell | Kind::SwapOut) && !fiat(&rows[i].base)).collect();
        let ([donor], [target]) = (donors.as_slice(), targets.as_slice()) else { continue };
        if rows[*target].fee_present() { continue; }
        let (fc, fa, ff, pm) = (rows[*donor].fee_currency.clone(), rows[*donor].fee_amount.clone(), rows[*donor].fee_fiat_value.clone(), rows[*donor].price_missing);
        let t = &mut rows[*target];
        (t.fee_currency, t.fee_amount, t.fee_fiat_value) = (fc, fa, ff);
        t.price_missing = t.price_missing || pm;
        let d = &mut rows[*donor];
        (d.fee_currency, d.fee_amount, d.fee_fiat_value) = (None, None, Dec::zero());
    }
}

/// `leg_direction`.
fn direction(kind: Kind) -> Option<bool> { match kind { Kind::SwapIn | Kind::Buy => Some(true), Kind::SwapOut | Kind::Sell => Some(false), _ => None } }

/// `mark_orphans`: a swap leg with no leg going the other way in its group, or no group.
fn mark_orphans(rows: &mut [Row]) {
    let mut directions: HashMap<(String, Option<String>), HashSet<bool>> = HashMap::new();
    for r in rows.iter() { if let Some(d) = direction(r.kind) { directions.entry((r.exchange.clone(), r.group.clone())).or_default().insert(d); } }
    for r in rows.iter_mut() {
        let Some(d) = direction(r.kind) else { continue };
        if !matches!(r.kind, Kind::SwapIn | Kind::SwapOut) { continue; }
        r.orphan = r.group.is_none() || !directions.get(&(r.exchange.clone(), r.group.clone())).is_some_and(|s| s.contains(&!d));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Quantities and openings
// ---------------------------------------------------------------------------------------------------------------

/// `Ledger.quantity_moves`: what one row does to the quantity of every non-cash asset it touches.
#[allow(clippy::too_many_arguments)]
pub fn quantity_moves(kind: Kind, base: &str, amount: &Dec, fee_currency: Option<&str>, fee_amount: Option<&Dec>, linked: bool, transfer_fee: Option<&Dec>) -> Result<Vec<(String, Dec)>, FiguresError> {
    let zero = Dec::zero();
    let fee = fee_amount.cloned().unwrap_or_else(Dec::zero);
    let fee_in_base = if fee_currency == Some(base) { fee.clone() } else { Dec::zero() };
    let mut moves = vec![];
    if kind == Kind::Adjustment {
        moves.push((base.to_string(), amount.clone()));
    } else if kind.base_in() {
        if !(kind == Kind::Deposit && linked) { moves.push((base.to_string(), max(&(amount - &fee_in_base)?, &zero))); }
    } else if kind == Kind::Withdrawal {
        moves.push((base.to_string(), (if linked { transfer_fee.cloned().unwrap_or_else(Dec::zero) } else { amount.clone() }).neg()));
    } else if kind.base_out() {
        let on_top = if matches!(kind, Kind::Sell | Kind::SwapOut) && !cash(base) { fee_in_base.clone() } else { Dec::zero() };
        moves.push((base.to_string(), (amount + &on_top)?.neg()));
    }
    if let Some(fc) = fee_currency.filter(|c| !c.trim().is_empty()) {
        if fc != base && fee.is_positive() { moves.push((fc.to_string(), fee.neg())); }
    }
    Ok(moves)
}

fn row_moves(r: &Row) -> Result<Vec<(String, Dec)>, FiguresError> {
    quantity_moves(r.kind, &r.base, &r.amount, r.fee_currency.as_deref(), r.fee_amount.as_ref(), r.linked, r.transfer_fee.as_ref())
}

fn between_venues(r: &Row) -> bool { r.linked && r.kind == Kind::Withdrawal && r.to_exchange.as_deref().is_some_and(|t| !t.is_empty() && t != r.exchange) }

/// `located_moves`: a linked transfer between venues takes the whole amount off the source and lands what arrived on
/// the destination.
fn located_moves(r: &Row) -> Result<Vec<(Option<String>, String, Dec)>, FiguresError> {
    let moves = row_moves(r)?.into_iter().map(|(s, a)| (Some(r.exchange.clone()), s, a));
    if !between_venues(r) { return Ok(moves.collect()); }
    let mut out: Vec<(Option<String>, String, Dec)> = moves.filter(|(_, s, _)| *s != r.base).collect();
    out.push((Some(r.exchange.clone()), r.base.clone(), r.amount.neg()));
    out.push((r.to_exchange.clone(), r.base.clone(), (&r.amount - &r.transfer_fee.clone().unwrap_or_else(Dec::zero))?));
    Ok(out)
}

/// `transfers_first`: within one second, a withdrawal to another venue is walked as early as its own venue allows.
pub fn transfers_first(rows: &[Row]) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        let mut j = i;
        while j + 1 < rows.len() && rows[j + 1].at == rows[i].at { j += 1; }
        let mut walked: Vec<Row> = vec![];
        for r in &rows[i..=j] {
            if !between_venues(r) { walked.push(r.clone()); continue; }
            let coin = &r.base;
            let after = walked.iter().rposition(|e| e.exchange == r.exchange
                && (e.base == *coin || e.quote.as_deref() == Some(coin.as_str()) || e.fee_currency.as_deref() == Some(coin.as_str())));
            walked.insert(after.map_or(0, |a| a + 1), r.clone());
        }
        out.extend(walked);
        i = j + 1;
    }
    out
}

/// `open_with_what_must_have_been_held`: an asset whose running quantity goes below zero is opened, a second before its
/// first row, with the least that keeps it at or above zero, at that day's price (a zero, unpriced, when nobody has
/// one); located, per venue.
pub fn open_with_held(rows: Vec<Row>, ps: &mut PriceService, located: bool, next_uid: &mut usize) -> Result<Vec<Row>, FiguresError> {
    type Key = (Option<String>, String);
    let mut running: HashMap<Key, Dec> = HashMap::new();
    let mut lowest: Vec<(Key, Dec)> = vec![];
    let mut low_at: HashMap<Key, usize> = HashMap::new();
    let mut first: HashMap<Key, usize> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        crate::figures::budget::check()?;
        let moves = if located { located_moves(row)? } else { row_moves(row)?.into_iter().map(|(s, a)| (None, s, a)).collect() };
        for (venue, symbol, amount) in moves {
            if cash(&symbol) { continue; }
            let key = (venue, symbol);
            first.entry(key.clone()).or_insert(i);
            let now = (running.get(&key).unwrap_or(&Dec::zero()) + &amount)?;
            running.insert(key.clone(), now.clone());
            crate::figures::budget::charge(1, 0)?;
            match low_at.get(&key) {
                Some(&j) => if now < lowest[j].1 { lowest[j].1 = now },
                None if now.is_negative() => { low_at.insert(key.clone(), lowest.len()); lowest.push((key, now)); }
                None => {}
            }
        }
    }
    let mut before: HashMap<usize, Vec<Row>> = HashMap::new();
    for ((venue, symbol), low) in lowest {
        let Some((i, row)) = first.get(&(venue.clone(), symbol.clone())).and_then(|&i| rows.get(i).map(|r| (i, r))) else { continue };
        let venue = venue.unwrap_or_else(|| row.exchange.clone());
        let at = row.at.plus_seconds(-1).ok_or_else(|| FiguresError::Data("an instant out of range".into()))?;
        let quantity = low.neg();
        let kept = ps.warnings.len();
        let price = ps.price_at(&symbol, "USD", at, Some(&venue))?;
        ps.warnings.truncate(kept);
        let uid = *next_uid;
        *next_uid += 1;
        before.entry(i).or_default().push(Row {
            uid, kind: Kind::Deposit, base: symbol, fiat_value: (&price * &quantity)?, fee_fiat_value: Dec::zero(), fee_currency: None, fee_amount: None,
            amount: quantity, quote: None, quote_amount: None, at, group: None, price_missing: price.is_zero(), value_missing: None, exchange: venue,
            linked: false, transfer_fee: None, swap_fiat_cost: None, swap_stable_cost: None, per_share: None, to_exchange: None, orphan: false, opening: true,
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

/// `taxable`: a fiat row reaches the engine only to pay a fee in a coin.
fn taxable(rows: &[Row]) -> Vec<Row> {
    rows.iter().filter(|r| !fiat(&r.base) || (r.fee().is_positive() && r.fee_present() && !r.fee_currency.as_deref().is_some_and(cash))).cloned().collect()
}

// ---------------------------------------------------------------------------------------------------------------
// The engine (Tracker::Ledger::Engine < Tax::Methods::Fifo, crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Lot { amount: Dec, cost_per_unit: Dec, date: At, basis_assumed: bool, unpriced: Dec }
#[derive(Clone, Debug)]
struct Tranche { amount: Dec, cost: Dec, date: At, basis_assumed: bool, unpriced: Dec, weight: Dec }

/// A disposal, as far as the figures and the arming read one.
#[derive(Clone, Debug)]
pub struct Disposal { pub at: At, pub asset: String, pub exchange: String, pub unpriced_quantity: Dec, pub any_lot_lost: Option<bool> }

type GroupKey = (String, String);

pub struct Engine {
    located: bool,
    venue: Option<String>,
    lots: BTreeMap<(Option<String>, String), VecDeque<Lot>>,
    pub disposals: Vec<Disposal>,
    pub uncovered: bool,
    uncovered_venues: HashSet<Option<String>>,
    released: HashMap<(Option<String>, String, String), VecDeque<Dec>>,
    moved: HashMap<usize, (Dec, bool)>,
    fee_basis: HashMap<usize, Dec>,
    original: HashMap<usize, Row>,
    row: Option<usize>,
    fee_consumed: bool,
    paying_trade_fee: bool,
    swap_in_groups: HashMap<GroupKey, (HashMap<usize, Dec>, bool)>,
    swap_out_groups: HashMap<GroupKey, (HashMap<usize, Dec>, Dec)>,
    transferred: HashMap<GroupKey, Vec<Tranche>>,
}

fn group_key(r: &Row) -> Option<GroupKey> { r.group.clone().map(|g| (r.exchange.clone(), g)) }
/// `fiat_disposal?` with stablecoins as cash.
fn fiat_disposal(r: &Row) -> bool { r.quote.as_deref().is_some_and(|q| !q.trim().is_empty() && (fiat(q) || stable(q))) }
/// `crypto_fee_asset?`.
fn crypto_fee_asset(r: &Row) -> bool { r.fee_present() && r.fee_currency != r.quote && !r.fee_currency.as_deref().is_some_and(fiat) }
fn unit(cost: &Dec, amount: &Dec) -> Result<Dec, FiguresError> { Ok(if amount.is_positive() { cost.div(amount)? } else { Dec::zero() }) }

/// `allocate_shares`: the last share is what is left.
fn allocate_shares(weights: &[Dec]) -> Result<Vec<Dec>, FiguresError> {
    let total = sum(weights.iter().map(|w| Ok(w.clone())))?;
    let mut allocated = Dec::zero();
    let mut out = vec![];
    for (i, w) in weights.iter().enumerate() {
        let share = if i == weights.len() - 1 { (&Dec::one() - &allocated)? } else { w.div(&total)? };
        allocated = (&allocated + &share)?;
        out.push(share);
    }
    Ok(out)
}

impl Engine {
    pub fn new(located: bool) -> Engine {
        Engine { located, venue: None, lots: BTreeMap::new(), disposals: vec![], uncovered: false, uncovered_venues: HashSet::new(), released: HashMap::new(),
                 moved: HashMap::new(), fee_basis: HashMap::new(), original: HashMap::new(), row: None, fee_consumed: false, paying_trade_fee: false,
                 swap_in_groups: HashMap::new(), swap_out_groups: HashMap::new(), transferred: HashMap::new() }
    }

    fn key(&self, asset: &str) -> (Option<String>, String) { (if self.located { self.venue.clone() } else { None }, asset.to_string()) }
    fn lots(&mut self, asset: &str) -> &mut VecDeque<Lot> { let k = self.key(asset); self.lots.entry(k).or_default() }
    fn held(&mut self, asset: &str) -> Result<Dec, FiguresError> { sum(self.lots(asset).iter().map(|l| Ok(l.amount.clone()))) }
    fn pool_basis(&mut self, asset: &str) -> Result<Dec, FiguresError> { sum(self.lots(asset).iter().map(|l| &l.amount * &l.cost_per_unit)) }

    /// `basis_released`: what the lots gave up when these coins left, in the order they left.
    fn basis_released(&mut self, asset: &str, amount: &Dec, venue: &str) -> Option<Dec> {
        let v = if self.located { Some(venue.to_string()) } else { None };
        self.released.get_mut(&(v, asset.to_string(), amount.to_s_f())).and_then(VecDeque::pop_front)
    }

    /// `dequeue_tranches`.
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
            tranches.push(Tranche { cost: (&take * &lot.cost_per_unit)?, amount: take, date: lot.date, basis_assumed: lot.basis_assumed, unpriced: unpriced.clone(), weight: Dec::zero() });
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

    fn sort_lots(&mut self, key: (Option<String>, String)) {
        if let Some(lots) = self.lots.get_mut(&key) { lots.make_contiguous().sort_by_key(|l| l.date); }
    }

    /// `consume_fee_asset`: a fee in a third coin leaves that coin's lots.
    fn consume_fee_asset(&mut self, asset: &str, amount: &Dec) -> Result<(), FiguresError> {
        self.fee_consumed = true;
        self.dequeue(asset, amount)?;
        Ok(())
    }

    /// `apply_acquisition_fee`: the quantity acquired and its cost, after the fee.
    fn acquisition_fee(&mut self, r: &Row, amount: &Dec, value: &Dec) -> Result<(Dec, Dec), FiguresError> {
        if r.fee_present() && r.fee_is(&r.base) {
            let Some(fee) = &r.fee_amount else { return Ok((amount.clone(), value.clone())) };
            let net = (amount - fee)?;
            return Ok((if net.is_negative() { Dec::zero() } else { net }, value.clone()));
        }
        if r.fee().is_positive() && crypto_fee_asset(r) {
            let fc = r.fee_currency.clone().unwrap_or_default();
            self.consume_fee_asset(&fc, &r.fee())?;
        }
        Ok((amount.clone(), (value + &r.fee_fiat_value)?))
    }

    /// The tracker's `consume_disposal_fee`: a coin's own sale fee in that coin leaves on top; else AcquisitionFee's.
    fn disposal_fee(&mut self, r: &Row) -> Result<(), FiguresError> {
        self.paying_trade_fee = true;
        let fee = r.fee();
        let out = if r.fee_is(&r.base) && !cash(&r.base) {
            if fee.is_positive() { self.consume_fee_asset(&r.base.clone(), &fee) } else { Ok(()) }
        } else if !fee.is_positive() || r.fee_is(&r.base) || !crypto_fee_asset(r) { Ok(()) }
        else { let fc = r.fee_currency.clone().unwrap_or_default(); self.consume_fee_asset(&fc, &fee) };
        self.paying_trade_fee = false;
        out
    }

    /// `lot_lost_verdict`: true, false, or None when a price nobody had decides it.
    fn verdict(r: &Row, tranches: &[Tranche], amount: &Dec, value: &Dec) -> Result<Option<bool>, FiguresError> {
        if !amount.is_positive() { return Ok(Some(false)); }
        if r.value_missing() && value.is_zero() { return Ok(None); }
        let (priced, unpriced): (Vec<&Tranche>, Vec<&Tranche>) = tranches.iter().partition(|t| t.unpriced.is_zero());
        for t in priced { if (value * &t.amount)? < (&t.cost * amount)? { return Ok(Some(true)); } }
        Ok(if unpriced.is_empty() { Some(false) } else { None })
    }

    /// The tracker's `record_disposal`: cash leaves silently; anything else is a disposal, and one that takes more than
    /// was held leaves the walk uncovered.
    fn record_disposal(&mut self, r: &Row, asset: &str, amount: &Dec, value: &Dec) -> Result<(), FiguresError> {
        if cash(asset) { self.dequeue(asset, amount)?; return self.disposal_fee(r); }
        let held = self.held(asset)?;
        let (tranches, _) = self.dequeue(asset, amount)?;
        self.disposal_fee(r)?;
        let unpriced_quantity = sum(tranches.iter().map(|t| Ok(t.unpriced.clone())))?;
        self.disposals.push(Disposal { at: r.at, asset: asset.into(), exchange: r.exchange.clone(), unpriced_quantity, any_lot_lost: Engine::verdict(r, &tranches, amount, value)? });
        if held < *amount { self.uncovered = true; self.uncovered_venues.insert(self.venue.clone()); }
        Ok(())
    }

    /// The tracker's `consume_fee_in_kind`: coins that left for nothing take their cost from the lots, and the basis they
    /// took is recorded for money in.
    fn consume_in_kind(&mut self, asset: &str, amount: &Dec) -> Result<(), FiguresError> {
        let record = !self.paying_trade_fee;
        let before = self.pool_basis(asset)?;
        if !fiat(asset) && amount.is_positive() && !self.lots(asset).is_empty() { self.dequeue(asset, amount)?; }
        if record {
            let released = (&before - &self.pool_basis(asset)?)?;
            let v = if self.located { self.venue.clone() } else { None };
            self.released.entry((v, asset.into(), amount.to_s_f())).or_default().push_back(released);
        }
        Ok(())
    }

    /// The tracker's `shrink_pool_for_transfer_fee`: the fee slice leaves the source; located, the rest of the coins go to
    /// the venue they arrived at, as the lots they were.
    fn transfer(&mut self, r: &Row) -> Result<(), FiguresError> {
        let before = self.pool_basis(&r.base)?;
        if let Some(fee) = r.transfer_fee.as_ref().filter(|f| f.is_positive()) { self.dequeue(&r.base, fee)?; }
        if cash(&r.base) { return Ok(()); }
        let after = self.pool_basis(&r.base)?;
        self.fee_basis.insert(r.uid, (&before - &after)?);
        let Some(destination) = r.to_exchange.clone().filter(|d| !d.is_empty()) else { return Ok(()) };
        if !self.located || destination == r.exchange { return Ok(()); }
        let amount = (&r.amount - &r.transfer_fee.clone().unwrap_or_else(Dec::zero))?;
        if !amount.is_positive() { return Ok(()); }
        let (mut tranches, held) = self.dequeue(&r.base, &amount)?;
        let uncovered = (&amount - &min(&amount, &held))?;
        if uncovered.is_positive() {
            self.uncovered = true;
            self.uncovered_venues.insert(self.venue.clone());
            tranches.push(Tranche { amount: uncovered.clone(), cost: Dec::zero(), date: r.at, basis_assumed: true, unpriced: uncovered, weight: Dec::zero() });
        }
        let key = (Some(destination), r.base.clone());
        let lots = self.lots.entry(key.clone()).or_default();
        for t in &tranches {
            if !t.amount.is_positive() { continue; }
            lots.push_back(Lot { amount: t.amount.clone(), cost_per_unit: t.cost.div(&t.amount)?, date: t.date, basis_assumed: t.basis_assumed, unpriced: t.unpriced.clone() });
        }
        self.sort_lots(key);
        self.moved.insert(r.uid, (sum(tranches.iter().map(|t| Ok(t.cost.clone())))?, tranches.iter().any(|t| t.unpriced.is_positive())));
        Ok(())
    }

    /// `transfer_swap_out`: the out-leg hands its lots over as tranches; coins never held, as a tranche of nothing.
    fn swap_out(&mut self, r: &Row, amount: &Dec) -> Result<(), FiguresError> {
        let (mut tranches, held) = self.dequeue(&r.base, amount)?;
        let Some(key) = group_key(r) else { return Ok(()) };
        let uncovered = (amount - &min(amount, &held))?;
        if uncovered.is_positive() {
            tranches.push(Tranche { amount: uncovered.clone(), cost: Dec::zero(), date: r.at, basis_assumed: true, unpriced: uncovered, weight: Dec::zero() });
        }
        let weight = self.swap_out_groups.get(&key).and_then(|(w, _)| w.get(&r.uid)).cloned()
            .ok_or_else(|| FiguresError::Raised("KeyError: a swap leg outside its group".into()))?;
        let handed = sum(tranches.iter().map(|t| Ok(t.amount.clone())))?;
        for t in &mut tranches { t.weight = (&weight * &t.amount)?.div(&handed)?; }
        self.transferred.entry(key).or_default().extend(tranches);
        Ok(())
    }

    /// `add_non_taxable_swap_in_lots`: an in-leg opens one lot per tranche its group handed over, scaled to its share,
    /// and a lot of its own for cash spent through the group.
    fn swap_in(&mut self, r: &Row, amount: &Dec, value: &Dec) -> Result<(), FiguresError> {
        let key = group_key(r);
        let tranches: Vec<Tranche> = key.as_ref().and_then(|k| self.transferred.get(k)).cloned().unwrap_or_default();
        let cash_present = r.swap_fiat_cost.is_some() || r.swap_stable_cost.is_some();
        let cash_cost = (&r.swap_fiat_cost.clone().unwrap_or_else(Dec::zero) + &r.swap_stable_cost.clone().unwrap_or_else(Dec::zero))?;
        let cash_cost = if r.swap_fiat_cost.is_none() && r.swap_stable_cost.is_none() { Dec::zero() } else { cash_cost };
        let (lot_amount, paid) = self.acquisition_fee(r, amount, &cash_cost)?;
        let fee_cost = (&paid - &cash_cost)?;
        let (share, share_assumed) = match key.as_ref().and_then(|k| self.swap_in_groups.get(k)) {
            Some((shares, assumed)) => (shares.get(&r.uid).cloned().ok_or_else(|| FiguresError::Raised("KeyError: a swap leg outside its group".into()))?, *assumed),
            None => (Dec::one(), false),
        };
        let assumed = r.price_missing || share_assumed;
        let lots_key = self.key(&r.base);
        if tranches.is_empty() && !cash_present {
            let lot = Lot { amount: lot_amount.clone(), cost_per_unit: unit(&(value + &fee_cost)?, &lot_amount)?, date: r.at, basis_assumed: assumed,
                            unpriced: if r.value_missing() { lot_amount.clone() } else { Dec::zero() } };
            self.lots(&r.base).push_back(lot);
            return Ok(());
        }
        let transferred_cost = (&sum(tranches.iter().map(|t| Ok(t.cost.clone())))? * &share)?;
        let coin_value = (&key.as_ref().and_then(|k| self.swap_out_groups.get(k)).map(|(_, v)| v.clone()).unwrap_or_else(Dec::zero) * &share)?;
        let cash_amount = if tranches.is_empty() { lot_amount.clone() }
            else if !cash_cost.is_positive() { Dec::zero() }
            else {
                let scale = if coin_value.is_positive() { coin_value.clone() } else { transferred_cost.clone() };
                let whole = (&cash_cost + &scale)?;
                if whole.is_positive() { (&lot_amount * &cash_cost)?.div(&whole)? } else { lot_amount.clone() }
            };
        let rest = (&lot_amount - &cash_amount)?;
        let total_weight = sum(tranches.iter().map(|t| Ok(t.weight.clone())))?;
        let mut allocated = Dec::zero();
        let mut opened: Vec<Lot> = vec![];
        for (i, t) in tranches.iter().enumerate() {
            crate::figures::budget::check()?;
            let tranche_amount = if i == tranches.len() - 1 { (&rest - &allocated)? } else { (&rest * &t.weight)?.div(&total_weight)? };
            allocated = (&allocated + &tranche_amount)?;
            if !tranche_amount.is_positive() { continue; }
            let cost = (&(&(&t.cost * &share)? * &total_weight)?.div(&t.weight)? + &(&fee_cost * &t.weight)?.div(&total_weight)?)?;
            let unpriced = if t.amount.is_positive() { (&tranche_amount * &t.unpriced)?.div(&t.amount)? } else { Dec::zero() };
            opened.push(Lot { amount: tranche_amount, cost_per_unit: cost.div(&rest)?, date: t.date, basis_assumed: t.basis_assumed || assumed, unpriced });
        }
        if cash_amount.is_positive() {
            let scale = if tranches.is_empty() { fee_cost.clone() } else if coin_value.is_positive() { coin_value.clone() } else { transferred_cost.clone() };
            opened.push(Lot { amount: cash_amount, cost_per_unit: unit(&(&cash_cost + &scale)?, &lot_amount)?, date: r.at,
                              basis_assumed: assumed || (rest.is_zero() && tranches.iter().any(|t| t.basis_assumed)), unpriced: Dec::zero() });
        }
        self.lots(&r.base).extend(opened);
        self.sort_lots(lots_key);
        Ok(())
    }

    /// Tax::Methods::Fifo#apply_split.
    fn split(&mut self, r: &Row) -> Result<(), FiguresError> {
        let amount = &r.amount;
        let pool = self.held(&r.base)?;
        let factor = if pool.is_positive() { (&pool + amount)?.div(&pool)? } else { Dec::zero() };
        let at = r.at;
        let lots = self.lots(&r.base);
        if !amount.is_zero() && pool.is_positive() && factor.is_positive() {
            for lot in lots.iter_mut() {
                lot.cost_per_unit = lot.cost_per_unit.div(&factor)?;
                lot.amount = (&lot.amount * &factor)?;
                lot.unpriced = (&lot.unpriced * &factor)?;
            }
        } else if amount.is_negative() && !factor.is_positive() {
            lots.clear();
        } else if amount.is_positive() && pool.is_zero() {
            lots.push_back(Lot { amount: amount.clone(), cost_per_unit: Dec::zero(), date: at, basis_assumed: true, unpriced: amount.clone() });
        }
        Ok(())
    }

    /// ReturnOfCapital#reduce_lot_basis.
    fn return_capital(&mut self, r: &Row) -> Result<(), FiguresError> {
        let per_unit = match &r.per_share {
            Some(per_share) if per_share.is_positive() => {
                let fx = match &r.quote_amount { Some(q) if !q.is_zero() => r.fiat_value.div(q)?, _ => Dec::one() };
                Some((per_share * &fx)?)
            }
            _ => None,
        };
        let value = r.fiat_value.clone();
        let lots = self.lots(&r.base);
        if let (Some(per_unit), false) = (&per_unit, lots.is_empty()) {
            for lot in lots.iter_mut() { lot.cost_per_unit = max(&(&lot.cost_per_unit - per_unit)?, &Dec::zero()); }
            return Ok(());
        }
        let mut remaining = value;
        for lot in lots.iter_mut() {
            if !remaining.is_positive() { break; }
            let lot_cost = (&lot.amount * &lot.cost_per_unit)?;
            let take = min(&lot_cost, &remaining);
            lot.cost_per_unit = if lot.amount.is_positive() { (&lot_cost - &take)?.div(&lot.amount)? } else { Dec::zero() };
            remaining = (&remaining - &take)?;
        }
        Ok(())
    }

    /// `settle_fee`: a fee in a coin that FIFO never applied still left the account, at the basis it carried.
    fn settle_fee(&mut self) -> Result<(), FiguresError> {
        let Some(uid) = self.row.take() else { return Ok(()) };
        if self.fee_consumed { return Ok(()); }
        let Some(row) = self.original.get(&uid).cloned() else { return Ok(()) };
        let amount = row.fee();
        let Some(asset) = row.fee_currency.clone().filter(|c| !c.trim().is_empty()) else { return Ok(()) };
        if !amount.is_positive() || cash(&asset) { return Ok(()); }
        if row.quote.as_deref() == Some(asset.as_str()) { return Ok(()); }
        if asset == row.base && !matches!(row.kind, Kind::Sell | Kind::SwapOut) { return Ok(()); }
        self.consume_fee_asset(&asset, &amount)
    }

    /// `remap`: a fiat row moves nothing but its coin fee; a lost coin is a sale for nothing; an unlinked withdrawal and
    /// an orphan swap-out left the tracked universe.
    fn remap(r: &Row) -> Row {
        let mut m = r.clone();
        if fiat(&r.base) { m.kind = Kind::Unsupported; return m; }
        match r.kind {
            Kind::Lost => { m.kind = Kind::Sell; m.fiat_value = Dec::zero(); m.quote = None; m.fee_currency = None; m.fee_amount = None; m.fee_fiat_value = Dec::zero(); }
            Kind::Withdrawal if !r.linked => m.kind = Kind::Fee,
            Kind::SwapOut if r.orphan => m.kind = Kind::Fee,
            _ => {}
        }
        m
    }

    /// `build_swap_in_groups` / `build_swap_out_groups` over the remapped rows.
    fn groups(&mut self, rows: &[Row]) -> Result<(), FiguresError> {
        let mut ins: Vec<(GroupKey, Vec<&Row>)> = vec![];
        let mut outs: Vec<(GroupKey, Vec<&Row>)> = vec![];
        for r in rows {
            if stable(&r.base) { continue; }
            let Some(key) = group_key(r) else { continue };
            let list = match r.kind { Kind::SwapIn => &mut ins, Kind::SwapOut if !fiat_disposal(r) => &mut outs, _ => continue };
            match list.iter_mut().find(|(k, _)| *k == key) { Some((_, legs)) => legs.push(r), None => list.push((key, vec![r])) }
        }
        for (key, legs) in ins {
            let one_asset = legs.iter().map(|r| r.base.as_str()).collect::<HashSet<_>>().len() == 1;
            let weights: Vec<Dec> = legs.iter().map(|r| if one_asset { r.amount.clone() } else { r.fiat_value.clone() }).collect();
            let equal = weights.iter().any(Dec::is_zero);
            let shares = allocate_shares(&if equal { vec![Dec::one(); legs.len()] } else { weights })?;
            self.swap_in_groups.insert(key, (legs.iter().map(|r| r.uid).zip(shares).collect(), equal));
        }
        for (key, legs) in outs {
            let values: Vec<Dec> = legs.iter().map(|r| r.fiat_value.clone()).collect();
            let total = sum(values.iter().map(|v| Ok(v.clone())))?;
            let weights = if values.iter().any(Dec::is_zero) { vec![Dec::one(); legs.len()] } else { values };
            self.swap_out_groups.insert(key, (legs.iter().map(|r| r.uid).zip(weights).collect(), total));
        }
        Ok(())
    }

    /// Tracker::Ledger::Engine#calculate over the taxable rows.
    pub fn calculate(&mut self, rows: &[Row]) -> Result<(), FiguresError> {
        for r in rows { self.original.insert(r.uid, r.clone()); }
        let remapped: Vec<Row> = rows.iter().map(Engine::remap).collect();
        self.groups(&remapped)?;
        for r in &remapped {
            crate::figures::budget::check()?;
            self.settle_fee()?;
            self.row = Some(r.uid);
            self.fee_consumed = false;
            if self.located { self.venue = Some(r.exchange.clone()); }
            let (asset, amount) = (r.base.clone(), r.amount.clone());
            match r.kind {
                Kind::Buy | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => self.acquire(r, r.price_missing)?,
                Kind::Deposit if r.linked => {}
                Kind::Deposit => self.acquire(r, true)?,
                Kind::SwapIn => if !stable(&asset) { self.swap_in(r, &amount, &r.fiat_value)? },
                Kind::SwapOut => {
                    if stable(&asset) {} else if !fiat_disposal(r) { self.swap_out(r, &amount)? } else { self.record_disposal(r, &asset, &amount, &r.fiat_value)? }
                }
                Kind::Sell => self.record_disposal(r, &asset, &amount, &r.fiat_value)?,
                Kind::Adjustment => self.split(r)?,
                Kind::ReturnOfCapital => self.return_capital(r)?,
                Kind::Fee => self.consume_in_kind(&asset, &amount)?,
                Kind::Withdrawal => self.transfer(r)?,
                Kind::Lost | Kind::WithholdingTax | Kind::Unsupported => {}
            }
        }
        self.settle_fee()
    }

    /// An acquisition's lot, its fee applied.
    fn acquire(&mut self, r: &Row, basis_assumed: bool) -> Result<(), FiguresError> {
        let (lot_amount, lot_cost) = self.acquisition_fee(r, &r.amount, &r.fiat_value)?;
        let cost_per_unit = unit(&lot_cost, &lot_amount)?;
        let unpriced = if r.value_missing() { lot_amount.clone() } else { Dec::zero() };
        self.lots(&r.base).push_back(Lot { amount: lot_amount, cost_per_unit, date: r.at, basis_assumed, unpriced });
        Ok(())
    }

    /// `positions_from` over one venue's lots (None: every lot of an unlocated walk), cash left out, largest cost first.
    fn positions(&self, venue: Option<&str>) -> Result<Vec<Position>, FiguresError> {
        let mut out = vec![];
        for ((at, symbol), lots) in &self.lots {
            if at.as_deref() != venue || cash(symbol) { continue; }
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

// ---------------------------------------------------------------------------------------------------------------
// The cash book and money in
// ---------------------------------------------------------------------------------------------------------------

/// An insertion-ordered map, as a Ruby Hash iterates.
#[derive(Default, Clone)]
struct Pots { keys: Vec<(String, String)>, index: HashMap<(String, String), usize>, values: Vec<Dec> }
impl Pots {
    fn slot(&mut self, venue: &str, currency: &str) -> &mut Dec {
        let k = (venue.to_string(), currency.to_string());
        let i = match self.index.get(&k) { Some(&i) => i, None => { self.index.insert(k.clone(), self.keys.len()); self.keys.push(k); self.values.push(Dec::zero()); self.values.len() - 1 } };
        &mut self.values[i]
    }
    fn get(&self, venue: &str, currency: &str) -> Dec { self.index.get(&(venue.to_string(), currency.to_string())).map_or_else(Dec::zero, |&i| self.values[i].clone()) }
    fn iter(&self) -> impl Iterator<Item = (&(String, String), &Dec)> { self.keys.iter().zip(self.values.iter()) }
}

/// Tracker::CashBook: per venue and currency, the units held, the dollars they carried in, and what they realised.
#[derive(Default)]
struct Book { cash: Pots, basis: Pots, realised: HashMap<String, Dec> }

impl Book {
    fn value(currency: &str, units: &Dec, at: At, ps: &mut PriceService) -> Result<Dec, FiguresError> {
        if units.is_zero() || currency == "USD" || stable(currency) { return Ok(units.clone()); }
        ps.convert_fiat(units, currency, "USD", at)
    }
    fn realise(&mut self, venue: &str, usd: Dec) -> Result<(), FiguresError> {
        let r = self.realised.entry(venue.into()).or_insert_with(Dec::zero);
        *r = (&*r + &usd)?;
        Ok(())
    }
    fn credit(&mut self, venue: &str, currency: &str, units: &Dec, usd: &Dec) -> Result<(), FiguresError> {
        let c = self.cash.slot(venue, currency); *c = (&*c + units)?;
        let b = self.basis.slot(venue, currency); *b = (&*b + usd)?;
        Ok(())
    }
    fn release(&mut self, venue: &str, currency: &str, units: &Dec, at: At, ps: &mut PriceService) -> Result<Dec, FiguresError> {
        let held = self.cash.get(venue, currency);
        let covered = if held.is_positive() { min(units, &held) } else { Dec::zero() };
        let from_pot = if covered.is_positive() { (&self.basis.get(venue, currency) * &covered)?.div(&held)? } else { Dec::zero() };
        let released = (&from_pot + &Book::value(currency, &(units - &covered)?, at, ps)?)?;
        let b = self.basis.slot(venue, currency); *b = (&*b - &released)?;
        let c = self.cash.slot(venue, currency); *c = (&*c - units)?;
        Ok(released)
    }
    /// `move`: returns the basis the withdrawn units carried.
    #[allow(clippy::too_many_arguments)]
    fn shift(&mut self, venue: &str, currency: &str, amount: &Dec, at: At, cost: &Dec, withdrawn: &Dec, worth: Option<&Dec>, ps: &mut PriceService) -> Result<Dec, FiguresError> {
        if amount.is_positive() {
            let usd = match worth { Some(w) => w.clone(), None => Book::value(currency, amount, at, ps)? };
            self.credit(venue, currency, amount, &usd)?;
            return Ok(Dec::zero());
        }
        let out = amount.neg();
        let released = self.release(venue, currency, &out, at, ps)?;
        let withdrawn = min(withdrawn, &out);
        let cost = min(cost, &(&out - &withdrawn)?);
        let paid = (&(&out - &withdrawn)? - &cost)?;
        let share = |units: &Dec| -> Result<Dec, FiguresError> { Ok((&released * units)?.div(&out)?) };
        let paid_value = if paid.is_positive() { match worth { Some(w) => w.clone(), None => Book::value(currency, &paid, at, ps)? } } else { Dec::zero() };
        self.realise(venue, (&(&paid_value - &share(&paid)?)? - &share(&cost)?)?)?;
        share(&withdrawn)
    }
    /// `carry`: what arrives lands on `to` with the basis it carried.
    #[allow(clippy::too_many_arguments)]
    fn carry(&mut self, venue: &str, currency: &str, units: &Dec, to: &str, at: At, cost: &Dec, ps: &mut PriceService) -> Result<Dec, FiguresError> {
        let released = self.release(venue, currency, units, at, ps)?;
        let cost = min(cost, units);
        let arrived = (units - &cost)?;
        let carried = (&released * &arrived)?.div(units)?;
        self.realise(venue, (&released - &carried)?.neg())?;
        self.credit(to, currency, &arrived, &carried)?;
        Ok(carried)
    }
    fn expense(&mut self, venue: &str, currency: &str, units: &Dec, at: At, ps: &mut PriceService) -> Result<(), FiguresError> {
        let usd = Book::value(currency, units, at, ps)?;
        self.realise(venue, usd.neg())
    }
}

/// Tracker::UnfundedCash.moves: the cash one row moves.
#[allow(clippy::too_many_arguments)]
pub fn cash_moves(kind: Kind, base: &str, amount: &Dec, quote: Option<&str>, quote_amount: Option<&Dec>, fee_currency: Option<&str>, fee_amount: Option<&Dec>) -> Result<Vec<(String, Dec)>, FiguresError> {
    let fee = fee_amount.cloned().unwrap_or_else(Dec::zero);
    let mut moves: Vec<(Option<String>, Dec)> = vec![];
    if fee.is_positive() && fee_currency != Some(base) { moves.push((fee_currency.map(String::from), fee.neg())); }
    if let Some(qa) = quote_amount {
        let direction = match kind { Kind::Buy => -1, Kind::Sell | Kind::ReturnOfCapital => 1, _ => 0 };
        moves.push((quote.map(String::from), (&Dec::from_i64(direction) * qa)?));
    }
    if kind.base_in() || kind.base_out() {
        let mut a = amount.clone();
        if fee_currency == Some(base) {
            if kind.base_in() { a = max(&(&a - &fee)?, &Dec::zero()); }
            if matches!(kind, Kind::Sell | Kind::SwapOut) && quote_amount.is_none() { a = (&a + &fee)?; }
        }
        moves.push((Some(base.to_string()), if kind.base_in() { a } else { a.neg() }));
    }
    Ok(moves.into_iter().filter_map(|(c, a)| c.filter(|c| cash(c)).filter(|_| !a.is_zero()).map(|c| (c, a))).collect())
}

fn row_cash_moves(r: &Row) -> Result<Vec<(String, Dec)>, FiguresError> {
    cash_moves(r.kind, &r.base, &r.amount, r.quote.as_deref(), r.quote_amount.as_ref(), r.fee_currency.as_deref(), r.fee_amount.as_ref())
}

fn in_kind(r: &Row) -> bool { r.kind.in_kind() || (r.kind == Kind::SwapIn && r.orphan) }
fn valued_by_price(r: &Row) -> bool { in_kind(r) || (r.kind == Kind::Deposit && !r.linked && !cash(&r.base)) }
fn cash_for_cash(r: &Row) -> bool { r.quote_amount.is_some() && cash(&r.base) && r.quote.as_deref().is_some_and(cash) }

/// `fee_already_counted?`.
fn fee_already_counted(r: &Row) -> bool {
    if r.fee_is(&r.base) {
        if r.kind.cost() || r.kind == Kind::Withdrawal { return true; }
        if matches!(r.kind, Kind::Sell | Kind::SwapOut) { return r.quote_amount.is_some(); }
    }
    if cash(&r.base) { return r.kind == Kind::Buy && cash_for_cash(r); }
    r.kind.acquisition() || (r.kind == Kind::Deposit && !r.linked) || r.kind == Kind::Sell
        || (r.kind == Kind::SwapOut && !r.orphan && r.quote.as_deref().is_some_and(cash))
}

/// `cash_costs`: units of cash, per currency, that left this row with nothing in return.
fn cash_costs(r: &Row) -> Result<Vec<(String, Dec)>, FiguresError> {
    let mut costs: Vec<(String, Dec)> = vec![];
    let mut add = |c: &str, a: &Dec| -> Result<(), FiguresError> {
        match costs.iter_mut().find(|(k, _)| k == c) { Some((_, v)) => *v = (&*v + a)?, None => costs.push((c.to_string(), (&Dec::zero() + a)?)) }
        Ok(())
    };
    if r.kind.cost() && cash(&r.base) { add(&r.base, &abs(&r.amount))?; }
    let fee = r.fee();
    if !(fee.is_positive() && r.fee_currency.as_deref().is_some_and(cash)) || fee_already_counted(r) { return Ok(costs); }
    let fc = r.fee_currency.clone().unwrap_or_default();
    if fc == r.base && r.kind.base_in() { add(&r.base, &min(&fee, &r.amount))?; }
    else if fc == r.base { add(&r.base, &fee)?; }
    else { add(&fc, &fee)?; }
    Ok(costs)
}

/// `cash_counterpart_worth`: cash bought with cash is worth what the quote came to (and a buy's other-cash fee).
fn counterpart_worth(r: &Row, ps: &mut PriceService) -> Result<Option<(String, Dec)>, FiguresError> {
    if !cash_for_cash(r) { return Ok(None); }
    let (Some(q), Some(qa)) = (r.quote.as_deref(), &r.quote_amount) else { return Ok(None) };
    let mut worth = Book::value(q, qa, r.at, ps)?;
    if r.kind == Kind::Buy && r.fee().is_positive() && !r.fee_is(&r.base) && r.fee_currency.as_deref().is_some_and(cash) {
        worth = (&worth + &Book::value(r.fee_currency.as_deref().unwrap_or(""), &r.fee(), r.at, ps)?)?;
    }
    Ok(Some((r.base.clone(), worth)))
}

/// `cash_conversions`: a conversion booked as legs of its own is valued as a single-row one is. Out-leg uid → USD.
fn cash_conversions(rows: &[Row], ps: &mut PriceService) -> Result<HashMap<usize, Dec>, FiguresError> {
    let mut order: Vec<(String, String)> = vec![];
    let mut groups: HashMap<(String, String), Vec<&Row>> = HashMap::new();
    for r in rows {
        let Some(g) = &r.group else { continue };
        let key = (r.exchange.clone(), g.clone());
        let legs = groups.entry(key.clone()).or_default();
        if legs.is_empty() { order.push(key); }
        legs.push(r);
    }
    let mut worths = HashMap::new();
    for key in order {
        let legs = &groups[&key];
        if legs.iter().any(|l| !cash(&l.base) || l.quote_amount.is_some()) { continue; }
        let outs: Vec<&&Row> = legs.iter().filter(|l| matches!(l.kind, Kind::Sell | Kind::SwapOut)).collect();
        let ins: Vec<&&Row> = legs.iter().filter(|l| matches!(l.kind, Kind::Buy | Kind::SwapIn)).collect();
        if outs.is_empty() || ins.is_empty() { continue; }
        let mut came = Dec::zero();
        for l in &ins { came = (&came + &Book::value(&l.base, &l.amount, l.at, ps)?)?; }
        let mut went = vec![];
        for l in &outs { went.push(Book::value(&l.base, &l.amount, l.at, ps)?); }
        let total = sum(went.iter().map(|v| Ok(v.clone())))?;
        for (l, v) in outs.iter().zip(went) { worths.insert(l.uid, if total.is_positive() { (&came * &v)?.div(&total)? } else { v }); }
    }
    Ok(worths)
}

/// Tracker::UnfundedCash.closers: which positions a shortfall may be read at, and for which venue.
fn closers(rows: &[Row]) -> HashMap<usize, String> {
    closers_of(&rows.iter().map(|r| (r.exchange.as_str(), r.group.as_deref())).collect::<Vec<_>>())
}

/// `UnfundedCash.closers` over (venue, group id) entries: position → the venue a shortfall may be read for there.
pub fn closers_of(entries: &[(&str, Option<&str>)]) -> HashMap<usize, String> {
    let mut last: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, (venue, group)) in entries.iter().enumerate() { if let Some(g) = group { last.insert((venue, g), i); } }
    let mut open: HashMap<&str, i64> = HashMap::new();
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut closing = HashMap::new();
    for (i, (venue, group)) in entries.iter().enumerate() {
        if let Some(g) = group {
            if seen.insert((venue, g)) { *open.entry(venue).or_insert(0) += 1; }
            if last.get(&(*venue, *g)) == Some(&i) { *open.entry(venue).or_insert(0) -= 1; }
        }
        if open.get(venue).copied().unwrap_or(0) == 0 { closing.insert(i, venue.to_string()); }
    }
    closing
}

/// `book_cash`: every cash move of the row into the book. Returns the basis the withdrawn cash carried; a carried basis
/// is left in `carried`.
fn book_cash(book: &mut Book, r: &Row, carried: &mut HashMap<usize, Dec>, conversions: &HashMap<usize, Dec>, ps: &mut PriceService) -> Result<Dec, FiguresError> {
    let (at, venue) = (r.at, r.exchange.as_str());
    let mut costs = cash_costs(r)?;
    let mut moves = row_cash_moves(r)?;
    if r.linked && cash(&r.base) {
        if r.kind == Kind::Withdrawal {
            let fee = max(&r.transfer_fee.clone().unwrap_or_else(Dec::zero), &Dec::zero());
            let to = r.to_exchange.clone().unwrap_or_else(|| venue.to_string());
            let c = book.carry(venue, &r.base, &r.amount, &to, at, &fee, ps)?;
            carried.insert(r.uid, c);
        } else {
            let arrived = sum(moves.iter().map(|(c, a)| Ok(if *c == r.base { a.clone() } else { Dec::zero() })))?;
            let netted = (&r.amount - &arrived)?;
            if netted.is_positive() { book.shift(venue, &r.base, &netted.neg(), at, &netted, &Dec::zero(), None, ps)?; }
        }
        moves.retain(|(c, _)| *c != r.base);
        costs.retain(|(c, _)| *c != r.base);
    }
    let withdrawing = (r.kind == Kind::Withdrawal && !r.linked) || (r.kind == Kind::SwapOut && r.orphan);
    let mut worth = counterpart_worth(r, ps)?;
    if worth.is_none() { if let Some(w) = conversions.get(&r.uid) { worth = Some((r.base.clone(), w.clone())); } }
    let worth_of = |c: &str| worth.as_ref().filter(|(b, _)| b == c).map(|(_, w)| w.clone());
    let mut withdrawn = Dec::zero();
    for (currency, amount) in &moves {
        let out = if amount.is_negative() { amount.neg() } else { Dec::zero() };
        let have = costs.iter().find(|(k, _)| k == currency).map_or_else(Dec::zero, |(_, v)| v.clone());
        let cost = min(&have, &out);
        match costs.iter_mut().find(|(k, _)| k == currency) { Some((_, v)) => *v = (&*v - &cost)?, None => costs.push((currency.clone(), cost.neg())) }
        let taken = if withdrawing && *currency == r.base { (&out - &cost)? } else { Dec::zero() };
        withdrawn = (&withdrawn + &book.shift(venue, currency, amount, at, &cost, &taken, worth_of(currency).as_ref(), ps)?)?;
    }
    let nothing_arrived = !moves.iter().any(|(c, a)| *c == r.base && a.is_positive());
    if let (Some(w), true) = (worth_of(&r.base), r.kind == Kind::Buy && nothing_arrived) { book.realise(venue, w.neg())?; }
    for (currency, units) in &costs { if units.is_positive() { book.expense(venue, currency, units, at, ps)?; } }
    Ok(withdrawn)
}

/// `coin_cost`: the basis coins took with them when they left for nothing.
fn coin_cost(r: &Row, engine: &mut Engine) -> Dec {
    if cash(&r.base) { return Dec::zero(); }
    match r.kind {
        Kind::Fee => engine.basis_released(&r.base, &r.amount, &r.exchange).unwrap_or_else(Dec::zero),
        Kind::Withdrawal if r.linked => engine.fee_basis.get(&r.uid).cloned().unwrap_or_else(Dec::zero),
        _ => Dec::zero(),
    }
}

/// `contribution`: money in from outside.
fn contribution(r: &Row, engine: &mut Engine, withdrawn: &Dec, ps: &mut PriceService) -> Result<Dec, FiguresError> {
    let direction = match r.kind {
        Kind::Deposit => 1,
        Kind::Withdrawal => -1,
        Kind::SwapOut => if r.orphan { -1 } else { return Ok(Dec::zero()) },
        _ => return if in_kind(r) { arrival(r, ps) } else { Ok(Dec::zero()) },
    };
    if r.linked { return Ok(Dec::zero()); }
    let value = if cash(&r.base) && direction < 0 { withdrawn.clone() }
        else if fiat(&r.base) { ps.convert_fiat(&r.amount, &r.base, "USD", r.at)? }
        else if stable(&r.base) { r.amount.clone() }
        else if direction > 0 { r.fiat_value.clone() }
        else { engine.basis_released(&r.base, &r.amount, &r.exchange).unwrap_or_else(Dec::zero) };
    Ok((&value * &Dec::from_i64(direction))?)
}

/// `arrival`: what a coin arriving free was worth that day.
fn arrival(r: &Row, ps: &mut PriceService) -> Result<Dec, FiguresError> {
    if !fiat(&r.base) { return Ok(r.fiat_value.clone()); }
    ps.convert_fiat(&r.amount, &r.base, "USD", r.at)
}

/// `unfunded_contribution`: cash the venue spent without ever seeing it arrive is money from outside.
fn unfunded(book: &mut Book, venue: &str, r: &Row, ps: &mut PriceService) -> Result<Dec, FiguresError> {
    if LENDS_CASH.contains(&venue) { return Ok(Dec::zero()); }
    let standing: Vec<((String, String), Dec)> = book.cash.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut total = Dec::zero();
    for ((exchange, currency), balance) in standing {
        if exchange != venue || !cash(&currency) || !balance.is_negative() { continue; }
        let shortfall = balance.neg();
        book.shift(&exchange, &currency, &shortfall, r.at, &Dec::zero(), &Dec::zero(), None, ps)?;
        total = (&total + &Book::value(&currency, &shortfall, r.at, ps)?)?;
    }
    Ok(total)
}

/// `money_in_terms`: one term per row (two more for a linked transfer between venues), and the cash book.
fn money_in(rows: &[Row], engine: &mut Engine, ps: &mut PriceService) -> Result<(Vec<Term>, Book), FiguresError> {
    let mut book = Book::default();
    let mut carried: HashMap<usize, Dec> = HashMap::new();
    let conversions = cash_conversions(rows, ps)?;
    let closes = closers(rows);
    let mut terms = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        crate::figures::budget::check()?;
        let kept = ps.warnings.len();
        let withdrawn = book_cash(&mut book, r, &mut carried, &conversions, ps)?;
        coin_cost(r, engine);
        let mut amount = contribution(r, engine, &withdrawn, ps)?;
        if let Some(venue) = closes.get(&i) { amount = (&amount + &unfunded(&mut book, venue, r, ps)?)?; }
        let complete = ps.warnings.len() == kept && !(r.price_missing && valued_by_price(r));
        terms.push(Term { at: r.at, amount, complete, opens: r.opening.then(|| (r.base.clone(), r.amount.clone())), exchange: r.exchange.clone() });
        if between_venues(r) {
            let moved = if cash(&r.base) { carried.get(&r.uid).cloned().unwrap_or_else(Dec::zero) } else { engine.moved.get(&r.uid).map_or_else(Dec::zero, |(m, _)| m.clone()) };
            let complete = !engine.moved.get(&r.uid).is_some_and(|(_, u)| *u);
            let to = r.to_exchange.clone().unwrap_or_default();
            terms.push(Term { at: r.at, amount: moved.neg(), complete, opens: None, exchange: r.exchange.clone() });
            terms.push(Term { at: r.at, amount: moved, complete, opens: None, exchange: to });
        }
    }
    Ok((terms, book))
}

/// `loss_sales`: per symbol, the latest loss-making disposal no older than 31 days.
pub fn loss_sales(disposals: &[Disposal], today: NaiveDate) -> Result<Vec<(String, NaiveDate)>, FiguresError> {
    let horizon = today.checked_sub_days(chrono::Days::new(31)).unwrap_or(today);
    let mut acc: Vec<(String, NaiveDate)> = vec![];
    let mut at: HashMap<&str, usize> = HashMap::new();
    for d in disposals.iter().filter(|d| d.any_lot_lost == Some(true)) {
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

/// The two walks over one enrichment, and the money-in terms: `Ledger.walk`.
pub struct Walk { pub rows: Vec<Row>, pub engine: Engine, pub global: Engine, pub terms: Vec<Term>, book: Book }

/// `Ledger.walk(user)`: enriched, ordered for transfers, walked account-wide and then located, and read for money in.
pub fn walk_rows(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService) -> Result<Walk, FiguresError> {
    let rows = enrich(stored, deposits, ps)?;
    let mut next_uid = rows.len();
    let located_order = transfers_first(&rows);
    let opened = open_with_held(rows, ps, false, &mut next_uid)?;
    let mut global = Engine::new(false);
    global.calculate(&taxable(&opened))?;
    let located = open_with_held(located_order, ps, true, &mut next_uid)?;
    let mut engine = Engine::new(true);
    engine.calculate(&taxable(&located))?;
    let (terms, book) = money_in(&located, &mut engine, ps)?;
    Ok(Walk { rows: located, engine, global, terms, book })
}

/// `Ledger.money_in`: the terms alone, as the backfill reads them (no summary, so nothing valued today).
pub fn terms(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService) -> Result<Vec<Term>, FiguresError> {
    Ok(walk_rows(stored, deposits, ps)?.terms)
}

/// `Ledger.scopes`: each venue's summary (by `ids`, Exchange name_id → id) and the whole account's. `now` is
/// `Time.current`, at which cash is valued.
pub fn scopes(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService, ids: &HashMap<String, i64>, now: At) -> Result<Walked, FiguresError> {
    let w = walk_rows(stored, deposits, ps)?;
    let mut venues: Vec<String> = vec![];
    for r in &w.rows { for v in [Some(&r.exchange), r.to_exchange.as_ref()].into_iter().flatten() { if !venues.contains(v) { venues.push(v.clone()); } } }
    let mut by_venue = vec![];
    for v in &venues {
        let id = *ids.get(v).ok_or_else(|| FiguresError::Raised(format!("KeyError: key not found: \"{v}\"")))?;
        by_venue.push((id, summarise(&w, v, ps, now)?));
    }
    let whole = whole(&w, &by_venue, ps)?;
    Ok(Walked { venues: by_venue, whole, terms: w.terms })
}

/// `summarise`: a venue's slice of the walk.
fn summarise(w: &Walk, venue: &str, ps: &mut PriceService, now: At) -> Result<Summary, FiguresError> {
    let positions = w.engine.positions(Some(venue))?;
    let terms: Vec<&Term> = w.terms.iter().filter(|t| t.exchange == venue).collect();
    let rows: Vec<&Row> = w.rows.iter().filter(|r| r.exchange == venue).collect();
    let cash: Vec<(String, Dec)> = w.book.cash.iter().filter(|((v, _), _)| v == venue).map(|((_, c), a)| (c.clone(), a.clone())).collect();
    let kept = ps.warnings.len();
    // `cash_in_usd` and `fees`: their values are the pages', but what they look up warns as Rails' does.
    for (currency, amount) in &cash { if !stable(currency) { ps.convert_fiat(amount, currency, "USD", now)?; } }
    for r in &rows { if r.kind == Kind::Fee && fiat(&r.base) { ps.convert_fiat(&r.amount, &r.base, "USD", r.at)?; } }
    let incomplete = positions.iter().any(|p| p.unpriced.is_positive()) || w.engine.uncovered_venues.contains(&Some(venue.to_string()))
        || w.engine.disposals.iter().any(|d| d.exchange == venue && d.unpriced_quantity.is_positive())
        || rows.iter().any(|r| r.price_missing) || terms.iter().any(|t| !t.complete) || ps.warnings.len() > kept;
    Ok(Summary {
        positions,
        total_invested: sum(terms.iter().map(|t| Ok(t.amount.clone())))?,
        cash,
        cash_basis: w.book.basis.iter().filter(|((v, _), b)| v == venue && !b.is_zero()).map(|((_, c), b)| (c.clone(), b.clone())).collect(),
        incomplete,
        loss_sales: vec![],
    })
}

/// `whole`: the venues added up, and the account-wide walk's loss sales.
fn whole(w: &Walk, venues: &[(i64, Summary)], ps: &PriceService) -> Result<Summary, FiguresError> {
    let mut total = Dec::zero();
    let mut cash: Vec<(String, Dec)> = vec![];
    let mut basis: Vec<(String, Dec)> = vec![];
    let add = |list: &mut Vec<(String, Dec)>, c: &str, a: &Dec| -> Result<(), FiguresError> {
        match list.iter_mut().find(|(k, _)| k == c) { Some((_, v)) => *v = (&*v + a)?, None => list.push((c.to_string(), (&Dec::zero() + a)?)) }
        Ok(())
    };
    let mut held: Vec<(String, Vec<Position>)> = vec![];
    for (_, s) in venues {
        total = (&total + &s.total_invested)?;
        for (c, a) in &s.cash { add(&mut cash, c, a)?; }
        for (c, a) in &s.cash_basis { add(&mut basis, c, a)?; }
        for p in &s.positions {
            match held.iter_mut().find(|(k, _)| *k == p.symbol) { Some((_, l)) => l.push(p.clone()), None => held.push((p.symbol.clone(), vec![p.clone()])) }
        }
    }
    let mut positions = vec![];
    for (symbol, list) in held {
        if list.len() == 1 { positions.extend(list); continue; }
        let quantity = sum(list.iter().map(|p| Ok(p.quantity.clone())))?;
        let cost = sum(list.iter().map(|p| Ok(p.cost.clone())))?;
        positions.push(Position { symbol, avg_cost: cost.div(&quantity)?, quantity, cost, estimated: list.iter().any(|p| p.estimated),
                                  unpriced: sum(list.iter().map(|p| Ok(p.unpriced.clone())))? });
    }
    positions.sort_by(|a, b| b.cost.cmp(&a.cost));
    Ok(Summary {
        positions, total_invested: total, cash, cash_basis: basis.into_iter().filter(|(_, b)| !b.is_zero()).collect(),
        incomplete: venues.iter().any(|(_, s)| s.incomplete) || w.engine.uncovered || !ps.warnings.is_empty(),
        loss_sales: loss_sales(&w.global.disposals, ps.today)?,
    })
}
