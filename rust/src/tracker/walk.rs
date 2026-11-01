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

#[cfg(test)]
#[test]
fn binance_a12_recorded_accounting_baselines() {
    use serde_json::{json, Value};
    let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/binance_identity.json")).unwrap();
    let cases=fixture["accounting"].as_array().unwrap();
    assert_eq!(cases.len(), 1 + 1, "complete two-pair history + reduced rollback baseline");
    for case in cases {
        let rows: Vec<Row> = case["rows"].as_array().unwrap().iter().enumerate().map(|(uid,r)| {
            let d=|k:&str| Dec::parse(r[k].as_str().unwrap()).unwrap();
            let kind=match r["entry_type"].as_str().unwrap() { "buy"=>Kind::Buy,"sell"=>Kind::Sell,"deposit"=>Kind::Deposit,
                "withdrawal"=>Kind::Withdrawal,"staking_reward"=>Kind::StakingReward,"airdrop"=>Kind::Airdrop,
                "swap_in"=>Kind::SwapIn,"swap_out"=>Kind::SwapOut,_=>panic!("unexpected baseline kind") };
            Row {uid,kind,base:r["base_currency"].as_str().unwrap().into(),amount:d("base_amount"),quote:None,quote_amount:None,
                fiat_value:d("fiat_value"),fee_fiat_value:d("fee_fiat_value"),fee_currency:r["fee_currency"].as_str().map(str::to_owned),fee_amount:r["fee_amount"].as_str().map(|s|Dec::parse(s).unwrap()),
                at:At::from_sql(r["transacted_at"].as_str().unwrap()).unwrap(),group:r["group_id"].as_str().map(str::to_owned),
                price_missing:false,value_missing:None,exchange:"binance".into(),linked:false,transfer_fee:None,swap_fiat_cost:None,
                swap_stable_cost:None,per_share:None,to_exchange:None,orphan:false,opening:false,stated_value:false,fee_missing:false}
        }).collect();
        let mut engine=Engine::new(false); engine.calculate(&rows).unwrap();
        let lots: serde_json::Map<String,Value>=engine.lots.iter().map(|((_,asset),ls)|
            (asset.clone(),json!(ls.iter().map(|l|vec![l.amount.to_s_f(),l.cost_per_unit.to_s_f()]).collect::<Vec<_>>()))).collect();
        assert_eq!(json!(lots),case["lots"],"exact per-lot remaining quantities and unit basis");
        let mut held:BTreeMap<String,Dec>=BTreeMap::new();
        for r in &rows { for (coin,amount) in quantity_moves(r.kind,&r.base,&r.amount,r.fee_currency.as_deref(),r.fee_amount.as_ref(),false,None).unwrap() {
            let old=held.entry(coin).or_insert_with(Dec::zero); *old=(&*old+&amount).unwrap();
        }}
        assert_eq!(json!(held.into_iter().map(|(k,v)|(k,v.to_s_f())).collect::<BTreeMap<_,_>>()),case["holdings"]);
        assert_eq!(engine.disposals.len(),case["disposals"].as_array().unwrap().len());
        for (actual,expected) in engine.disposals.iter().zip(case["disposals"].as_array().unwrap()) {
            for (name,value) in [("amount",&actual.amount),("proceeds",&actual.proceeds),("cost_basis",&actual.cost_basis),("gain_loss",&actual.gain_loss)] {
                assert_eq!(value.to_s_f(),expected[name].as_str().unwrap(),"{name}");
            }
        }
        assert_eq!(sum(engine.disposals.iter().map(|d|Ok(d.gain_loss.clone()))).unwrap().to_s_f(),case["realised"]);
    }
}

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
    /// `stated_value`: a price the user stated, on a row neither its own quote nor the cash beside it values.
    pub stated_value: bool,
    /// Whether the fee's own valuation warned (`price_missing` folds it in with the row's value).
    pub fee_missing: bool,
}

impl Row {
    fn value_missing(&self) -> bool { self.value_missing.unwrap_or(self.price_missing) }
    fn fee(&self) -> Dec { self.fee_amount.clone().unwrap_or_else(Dec::zero) }
    fn fee_is(&self, currency: &str) -> bool { self.fee_currency.as_deref() == Some(currency) }
    fn fee_present(&self) -> bool { self.fee_currency.as_deref().is_some_and(|c| !c.trim().is_empty()) }
}

/// One term of money in, as the chart reads it day by day (Tracker::Ledger::Term).
#[derive(Clone, Debug)]
pub struct Term { pub at: At, pub amount: Dec, pub complete: bool, pub opens: Option<(String, Dec)>, pub exchange: String,
                  /// `in_kind?` of the row the term is paired with: its amount is money in nobody paid for (`received_usd`).
                  pub received: bool }

/// A position the lots hold (Tracker::Ledger::Position).
#[derive(Clone, Debug)]
pub struct Position { pub symbol: String, pub quantity: Dec, pub cost: Dec, pub avg_cost: Dec, pub opened_at: Option<At>, pub estimated: bool, pub unpriced: Dec }

/// Tracker::Ledger::RoundTrip: one row per round trip, not per sell.
#[derive(Clone, Debug)]
pub struct RoundTrip { pub symbol: String, pub opened_at: Option<At>, pub closed_at: At, pub quantity: Dec, pub invested: Dec, pub proceeds: Dec, pub fees: Dec, pub realised: Dec, pub incomplete: bool }

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
    pub round_trips: Vec<RoundTrip>,
    /// The part of money in nobody paid for (`received_usd`).
    pub received: Dec,
    /// `realised_pnl_usd`: the disposals' gains, and what return of capital past the basis, third-coin fees, the cash
    /// book and cash that left for nothing realised.
    pub realised: Dec,
    pub fees: Dec,
    /// The cash at today's rates (`cash_usd`).
    pub cash_usd: Dec,
    pub unpriced_proceeds: Dec,
    /// Per asset, what must have been held before its history begins, in the order first opened.
    pub openings: Vec<(String, Dec)>,
}

impl Summary {
    /// `empty_summary`: a venue the walk has no rows of.
    pub fn empty() -> Summary { Summary { positions: vec![], total_invested: Dec::zero(), cash: vec![], cash_basis: vec![], incomplete: false, loss_sales: vec![],
                                        round_trips: vec![], received: Dec::zero(), realised: Dec::zero(), fees: Dec::zero(), cash_usd: Dec::zero(),
                                        unpriced_proceeds: Dec::zero(), openings: vec![] } }
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
/// D-TAX-BG-CURRENCY, D-TAX-CZ-CNB: under a statutory conversion a leg that could not be converted is a missing value
/// (its warning kept, the third field true), never replaced by the coin's price.
fn cash_faces(legs: &[&Stored], ps: &mut PriceService, currency: &str) -> Result<Option<(Dec, Dec, bool)>, FiguresError> {
    let kept = ps.warnings.len();
    let (mut f, mut s) = (Dec::zero(), Dec::zero());
    for t in legs {
        if stable(&t.base) { s = (&s + &ps.convert_fiat(&t.amount, "USD", currency, t.at)?)?; }
        else { f = (&f + &ps.convert_fiat(&t.amount, &t.base, currency, t.at)?)?; }
    }
    if ps.warnings.len() == kept { return Ok(Some((f, s, false))); }
    if ps.statutory.is_some() { return Ok(Some((f, s, true))); }
    ps.warnings.truncate(kept);
    Ok(None)
}

/// `quote_of`: a fiat currency if any leg carries one, else the first leg's.
fn quote_of(legs: &[&Stored]) -> Option<String> { legs.iter().find(|t| fiat(&t.base)).or(legs.first()).map(|t| t.base.clone()) }

/// `cash_leg_context`: per grouped leg, what the cash beside it says it was worth, and whether a coin-for-coin pair is a
/// swap however the venue booked it.
fn cash_leg_context(rows: &[Stored], ps: &mut PriceService, currency: &str) -> Result<HashMap<i64, Leg>, FiguresError> {
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
                if let Some((f, s, missing)) = cash_faces(&cash_out, ps, currency)? {
                    let mixed = !outs.is_empty();
                    for (id, share, assumed) in shares(&ins)? {
                        context.insert(id, Leg { swap_fiat: Some((&f * &share)?), swap_stable: Some((&s * &share)?),
                                                 fiat_value: if mixed { None } else { Some((&(&f + &s)? * &share)?) },
                                                 price_missing: (assumed || missing).then_some(true), ..Leg::default() });
                    }
                }
            }
            if !cash_in.is_empty() && !outs.is_empty() {
                if let Some((f, s, missing)) = cash_faces(&cash_in, ps, currency)? {
                    let quote = quote_of(&cash_in);
                    for (id, share, assumed) in shares(&outs)? {
                        context.insert(id, Leg { fiat_value: Some((&(&f + &s)? * &share)?), quote: quote.clone(), price_missing: Some(assumed || missing), ..Leg::default() });
                    }
                }
            }
        }
    }
    Ok(context)
}

/// `resolve_fiat_value`.
fn fiat_value(t: &Stored, leg: Option<&Leg>, ps: &mut PriceService, currency: &str) -> Result<Dec, FiguresError> {
    if fiat(&t.base) { return Ok(Dec::zero()); }
    if let (Some(q), Some(qa)) = (t.quote.as_deref(), &t.quote_amount) {
        if q == currency { return Ok(qa.clone()); }
        if stable(q) { return ps.convert_fiat(qa, "USD", currency, t.at); }
        if fiat(q) { return ps.convert_fiat(qa, q, currency, t.at); }
    }
    if let Some(v) = leg.and_then(|l| l.fiat_value.clone()) { return Ok(v); }
    if let Some(price) = &t.stated { return ps.convert_fiat(&(price * &t.amount)?, "USD", currency, t.at); }
    let price = ps.price_at_recorded(&t.base, currency, t.at, Some(&t.venue), t.base_asset.as_ref())?;
    Ok((&price * &t.amount)?)
}

/// `resolve_fee_fiat_value`.
fn fee_fiat_value(t: &Stored, ps: &mut PriceService, report: &str) -> Result<Dec, FiguresError> {
    let Some(fee) = t.fee_amount.as_ref().filter(|f| !f.is_zero()) else { return Ok(Dec::zero()) };
    let currency = t.fee_currency.as_deref().unwrap_or("");
    if currency == report { return Ok(fee.clone()); }
    if fiat(currency) { return ps.convert_fiat(fee, currency, report, t.at); }
    if stable(currency) { return ps.convert_fiat(fee, "USD", report, t.at); }
    let price = ps.price_at(currency, report, t.at, Some(&t.venue))?;
    Ok((&price * fee)?)
}

/// `enrich(ordered, currency: 'USD')`, then the venue each linked row's coins went to and the orphan swap legs.
/// `deposits` are the base amounts of every row of the account, for a linked withdrawal's fee.
pub fn enrich(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService) -> Result<Vec<Row>, FiguresError> { enrich_with(stored, deposits, ps, true, "USD", &mut |_| {}) }

/// `enrich`, its prefetch left out when `prefetch` is false (the tax report reads stored prices only).
/// `on_progress(percent)` as PriceService#enrich reports it: 0–21 over the prefetch's ranges, then 21–100 over the rows.
pub fn enrich_with(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService, prefetch: bool, currency: &str, on_progress: &mut dyn FnMut(u32)) -> Result<Vec<Row>, FiguresError> {
    ps.record(stored);
    let legs = cash_leg_context(stored, ps, currency)?;
    // `(done.to_f / total * 21).to_i`
    if prefetch { ps.prefetch(stored, currency, &|t| legs.get(&t.id).is_some_and(|l| l.fiat_value.is_some()), &mut |done, total| on_progress(if total > 0 { (done as f64 / total as f64 * 21.0) as u32 } else { 0 }))?; }
    let total = stored.len();
    let links: HashMap<i64, i64> = stored.iter().filter_map(|t| t.linked_to.map(|d| (t.id, d))).collect();
    let linked_deposits: HashSet<i64> = links.values().copied().collect();
    let mut rows = Vec::with_capacity(stored.len());
    for (uid, t) in stored.iter().enumerate() {
        crate::figures::budget::check()?;
        let before = ps.warnings.len();
        let leg = legs.get(&t.id);
        let value = if matches!(t.kind, Kind::Withdrawal | Kind::Lost) {
            let kept = ps.warnings.len();
            let v = fiat_value(t, leg, ps, currency)?;
            ps.warnings.truncate(kept);
            v
        } else { fiat_value(t, leg, ps, currency)? };
        let value_missing = ps.warnings.len() > before || leg.and_then(|l| l.price_missing) == Some(true);
        let before_fee = ps.warnings.len();
        let fee_value = fee_fiat_value(t, ps, currency)?;
        let fee_missing = ps.warnings.len() > before_fee;
        let price_missing = value_missing || ps.warnings.len() > before;
        // `21 + ((index + 1).to_f / total * 79).to_i`
        on_progress(21 + ((uid + 1) as f64 / total as f64 * 79.0) as u32);
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
            stated_value: t.stated.is_some() && !(t.quoted() || leg.is_some_and(|l| l.fiat_value.is_some())), fee_missing,
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
        let fm = rows[*donor].fee_missing;
        let t = &mut rows[*target];
        (t.fee_currency, t.fee_amount, t.fee_fiat_value) = (fc, fa, ff);
        t.price_missing = t.price_missing || pm;
        t.fee_missing = t.fee_missing || fm;
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
pub(crate) fn located_moves(r: &Row) -> Result<Vec<(Option<String>, String, Dec)>, FiguresError> {
    let moves = row_moves(r)?.into_iter().map(|(s, a)| (Some(r.exchange.clone()), s, a));
    if !between_venues(r) { return Ok(moves.collect()); }
    let mut out: Vec<(Option<String>, String, Dec)> = moves.filter(|(_, s, _)| *s != r.base).collect();
    out.push((Some(r.exchange.clone()), r.base.clone(), r.amount.neg()));
    out.push((r.to_exchange.clone(), r.base.clone(), (&r.amount - &r.transfer_fee.clone().unwrap_or_else(Dec::zero))?));
    Ok(out)
}

/// Whether `e`, earlier in its instant, must be walked before `t`, a withdrawal of `coin` from `e`'s venue: a row of that
/// venue touching the coin, except a linked deposit that moves none of it there (its withdrawal carried the units; an
/// own-base fee on a coin moves nothing either). A linked deposit's positive fee still counts in a third coin that is
/// `coin`, or in its own base where that base is cash (the cash book nets it, at the instant's rate).
fn transfer_barrier(e: &Row, t: &Row) -> bool {
    let coin = t.base.as_str();
    if e.exchange != t.exchange { return false; }
    if !(e.base == coin || e.quote.as_deref() == Some(coin) || e.fee_currency.as_deref() == Some(coin)) { return false; }
    if e.kind == Kind::Deposit && e.linked {
        return e.fee().is_positive() && e.fee_is(coin) && (e.base != coin || cash(coin));
    }
    true
}

/// D-TRACKER-TRANSFER-ORDER (`transfers_first`): within one instant (identical `At`), a withdrawal to another venue is walked
/// as early as its funds allow. Rails walked each right after the last row of its venue touching its coin, so a linked
/// deposit there (whose units its withdrawal had already carried) held it back behind a sale on the far venue that needed
/// its lots. A schedule, one row per step: (1) the latest ready transfer (Rails' tie order: later ones land first); (2) else
/// the next other row, in input order (what a blocked transfer waits for); (3) else, only transfers waiting on each other
/// left, the latest whose barriers are walked. A transfer is ready when its barriers (`transfer_barrier`) are walked and its
/// coin is cash, or its venue holds the amount, or nothing left in the instant brings the coin there. What a venue holds is
/// `open_with_held`'s own arithmetic over the walked rows, same-day adjustments netted: the running quantity, the openings
/// already forced counted. Instants before `from` keep their order. Returns the rows reordered, nothing else changed.
pub fn transfers_first(rows: &[Row], from: At) -> Result<Vec<Row>, FiguresError> {
    type Key = (String, String);
    let netted = net_daily_splits(rows.to_vec())?;
    let mut running: HashMap<Key, Dec> = HashMap::new();
    let mut lowest: HashMap<Key, Dec> = HashMap::new();
    let walk = |i: usize, running: &mut HashMap<Key, Dec>, lowest: &mut HashMap<Key, Dec>| -> Result<(), FiguresError> {
        for (venue, symbol, amount) in located_moves(&netted[i])? {
            if cash(&symbol) { continue; }
            crate::figures::budget::charge(1, 0)?;
            let key = (venue.unwrap_or_default(), symbol);
            let now = (running.get(&key).unwrap_or(&Dec::zero()) + &amount)?;
            if now < *lowest.get(&key).unwrap_or(&Dec::zero()) { lowest.insert(key.clone(), now.clone()); }
            running.insert(key, now);
        }
        Ok(())
    };
    let available = |key: &Key, running: &HashMap<Key, Dec>, lowest: &HashMap<Key, Dec>| -> Result<Dec, FiguresError> {
        Ok((running.get(key).unwrap_or(&Dec::zero()) - lowest.get(key).unwrap_or(&Dec::zero()))?)
    };
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        let mut j = i;
        while j + 1 < rows.len() && rows[j + 1].at == rows[i].at { j += 1; }
        let span: Vec<usize> = (i..=j).collect();
        if rows[i].at < from || !span.iter().any(|&k| between_venues(&rows[k])) {
            for &k in &span { walk(k, &mut running, &mut lowest)?; out.push(rows[k].clone()); }
            i = j + 1;
            continue;
        }
        // Per transfer: its barriers not yet walked. Per (venue, coin): the rows left that bring the coin there.
        let transfers: Vec<usize> = span.iter().copied().filter(|&k| between_venues(&rows[k])).collect();
        let mut waiting: HashMap<usize, usize> = HashMap::new();
        let mut holds_back: HashMap<usize, Vec<usize>> = HashMap::new();
        for &t in &transfers {
            for e in i..t {
                crate::figures::budget::charge(1, 0)?;
                if transfer_barrier(&rows[e], &rows[t]) { *waiting.entry(t).or_insert(0) += 1; holds_back.entry(e).or_default().push(t); }
            }
        }
        let mut bringing: HashMap<Key, usize> = HashMap::new();
        let mut brings: HashMap<usize, Vec<Key>> = HashMap::new();
        for &k in &span {
            for (venue, symbol, amount) in located_moves(&netted[k])? {
                if amount.is_positive() && !cash(&symbol) {
                    let key = (venue.unwrap_or_default(), symbol);
                    *bringing.entry(key.clone()).or_insert(0) += 1;
                    brings.entry(k).or_default().push(key);
                }
            }
        }
        let mut left: Vec<usize> = span.clone();
        while !left.is_empty() {
            crate::figures::budget::charge(left.len() as u64, 0)?;
            let walkable = |t: usize| between_venues(&rows[t]) && waiting.get(&t).copied().unwrap_or(0) == 0;
            let mut pick = None;
            for &t in left.iter().rev() {
                if !walkable(t) { continue; }
                let r = &rows[t];
                let key = (r.exchange.clone(), r.base.clone());
                let others = bringing.get(&key).copied().unwrap_or(0).saturating_sub(brings.get(&t).map_or(0, |b| b.iter().filter(|k| **k == key).count()));
                if cash(&r.base) || available(&key, &running, &lowest)? >= r.amount || others == 0 { pick = Some(t); break; }
            }
            let pick = pick.or_else(|| left.iter().copied().find(|&k| !between_venues(&rows[k])))
                .or_else(|| left.iter().rev().copied().find(|&t| walkable(t)))
                .ok_or_else(|| FiguresError::Data("a transfer schedule with nothing to walk".into()))?;
            left.retain(|&k| k != pick);
            for t in holds_back.get(&pick).into_iter().flatten() { if let Some(w) = waiting.get_mut(t) { *w -= 1; } }
            for key in brings.get(&pick).into_iter().flatten() { if let Some(n) = bringing.get_mut(key) { *n -= 1; } }
            walk(pick, &mut running, &mut lowest)?;
            out.push(rows[pick].clone());
        }
        i = j + 1;
    }
    Ok(out)
}

/// `open_with_what_must_have_been_held`: an asset whose running quantity goes below zero is opened, a second before its
/// first row, with the least that keeps it at or above zero, at that day's price (a zero, unpriced, when nobody has
/// one); located, per venue.
pub fn open_with_held(rows: Vec<Row>, ps: &mut PriceService, located: bool, next_uid: &mut usize) -> Result<Vec<Row>, FiguresError> {
    let rows = net_daily_splits(rows)?;
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
            linked: false, transfer_fee: None, swap_fiat_cost: None, swap_stable_cost: None, per_share: None, to_exchange: None, orphan: false, opening: true, stated_value: false, fee_missing: false,
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

/// D-ALPACA-SPLIT-GROUPING: a venue's same-day adjustments of one symbol are one restatement.
/// Applying an intermediate removal first can erase FIFO basis or invent an opening balance. Net before
/// either walk, at the first row of that day; retain the other row identities with zero quantity changes.
fn net_daily_splits(mut rows: Vec<Row>) -> Result<Vec<Row>, FiguresError> {
    let mut first = HashMap::new();
    for i in 0..rows.len() {
        crate::figures::budget::charge(1, 0)?;
        let row = &rows[i];
        if row.kind != Kind::Adjustment { continue; }
        let key = (row.exchange.clone(), row.base.clone(), row.at.utc().date_naive());
        let at = *first.entry(key).or_insert(i);
        if at != i {
            rows[at].amount = (&rows[at].amount + &rows[i].amount)?;
            rows[i].amount = Dec::zero();
        }
    }
    Ok(rows)
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

/// A disposal (Tax::Methods::Fifo#disposal_row, without a holding rule), as the figures, the round trips and the arming
/// read one.
#[derive(Clone, Debug)]
pub struct Disposal { pub at: At, pub acquired: Option<At>, pub asset: String, pub exchange: String, pub amount: Dec, pub proceeds: Dec, pub cost_basis: Dec,
                      pub fee: Dec, pub gain_loss: Dec, pub data_incomplete: bool, pub unpriced_quantity: Dec, pub unpriced_proceeds: Dec,
                      pub closes_position: bool, pub any_lot_lost: Option<bool> }

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
    /// Per venue: return of capital past the basis (`roc_at`), and what third-coin fees realised (`fee_asset_realised_at`).
    roc: HashMap<Option<String>, Dec>,
    fee_realised: HashMap<Option<String>, Dec>,
    /// The value the fee being paid was counted at, and whether nothing counted it (a cost).
    fee_value: Dec,
    fee_is_cost: bool,
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
                 swap_in_groups: HashMap::new(), swap_out_groups: HashMap::new(), transferred: HashMap::new(),
                 roc: HashMap::new(), fee_realised: HashMap::new(), fee_value: Dec::zero(), fee_is_cost: true }
    }

    fn add(map: &mut HashMap<Option<String>, Dec>, venue: Option<String>, amount: &Dec) -> Result<(), FiguresError> {
        let entry = map.entry(venue).or_insert_with(Dec::zero);
        *entry = (&*entry + amount)?;
        Ok(())
    }
    /// Remaining FIFO quantities and unit bases, grouped by asset and optional venue.
    pub fn remaining_lots(&self) -> impl Iterator<Item = (&str, Option<&str>, Vec<(&Dec, &Dec)>)> {
        self.lots.iter().map(|((venue, asset), lots)|
            (asset.as_str(), venue.as_deref(), lots.iter().map(|lot| (&lot.amount, &lot.cost_per_unit)).collect()))
    }
    pub fn roc_at(&self, venue: &str) -> Dec { self.roc.get(&Some(venue.to_string())).cloned().unwrap_or_else(Dec::zero) }
    pub fn fee_asset_realised_at(&self, venue: &str) -> Dec { self.fee_realised.get(&Some(venue.to_string())).cloned().unwrap_or_else(Dec::zero) }

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
    /// The tracker's override: the difference from the basis the coins carried is theirs to realise.
    fn consume_fee_asset(&mut self, asset: &str, amount: &Dec) -> Result<(), FiguresError> {
        self.fee_consumed = true;
        let before = self.pool_basis(asset)?;
        self.dequeue(asset, amount)?;
        // A stablecoin fee is cash: the cash book pays it, at face, and it gains nothing.
        if cash(asset) { return Ok(()); }
        let released = (&before - &self.pool_basis(asset)?)?;
        let counted = if self.fee_is_cost { Dec::zero() } else { self.fee_value.clone() };
        Engine::add(&mut self.fee_realised, self.venue.clone(), &(&counted - &released)?)
    }

    /// `apply_acquisition_fee`: the quantity acquired and its cost, after the fee.
    fn acquisition_fee(&mut self, r: &Row, amount: &Dec, value: &Dec) -> Result<(Dec, Dec), FiguresError> {
        self.fee_value = r.fee_fiat_value.clone();
        // Cash bought is the cash book's: a coin paid as its fee has no lot to go into, and leaves at its basis.
        self.fee_is_cost = cash(&r.base);
        let (quantity, cost) = self.acquisition_fee_applied(r, amount, value)?;
        // A same-coin fee that took everything that arrived opens a lot of nothing, and its cost goes with it: spent.
        if !self.fee_is_cost && r.kind != Kind::SwapIn && quantity.is_zero() && cost.is_positive() {
            Engine::add(&mut self.fee_realised, self.venue.clone(), &cost.neg())?;
        }
        Ok((quantity, cost))
    }

    /// AcquisitionFee#apply_acquisition_fee.
    fn acquisition_fee_applied(&mut self, r: &Row, amount: &Dec, value: &Dec) -> Result<(Dec, Dec), FiguresError> {
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
        self.fee_value = r.fee_fiat_value.clone();
        // Off a coin's gain it was counted at its value; off cash sold, nothing counts it — a cost.
        self.fee_is_cost = cash(&r.base);
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
        let has_lots = !self.lots(asset).is_empty();
        let (tranches, _) = self.dequeue(asset, amount)?;
        self.disposal_fee(r)?;
        let cost_basis = sum(tranches.iter().map(|t| Ok(t.cost.clone())))?;
        let unpriced_quantity = sum(tranches.iter().map(|t| Ok(t.unpriced.clone())))?;
        let fee = r.fee_fiat_value.clone();
        let covered = held >= *amount;
        let closes_position = held.is_positive() && covered && self.lots(asset).is_empty();
        self.disposals.push(Disposal {
            at: r.at, acquired: tranches.first().map(|t| t.date), asset: asset.into(), exchange: r.exchange.clone(), amount: amount.clone(),
            proceeds: value.clone(), gain_loss: (&(value - &cost_basis)? - &fee)?, cost_basis, fee,
            // FIFO's `data_incomplete?`, and a sale only partly covered (which FIFO books at zero basis).
            data_incomplete: r.price_missing || tranches.iter().any(|t| t.basis_assumed) || !has_lots || !covered,
            unpriced_proceeds: if amount.is_positive() { (value * &unpriced_quantity)?.div(amount)? } else { Dec::zero() },
            unpriced_quantity, closes_position, any_lot_lost: Engine::verdict(r, &tranches, amount, value)?,
        });
        if !covered { self.uncovered = true; self.uncovered_venues.insert(self.venue.clone()); }
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
        // D-TRACKER-COIN-QUOTE: an own-coin fee that took every unit that arrived opens nothing; the basis carried in (and the
        // cash spent through the group) is spent, realised once as a loss, as an acquisition's is (`acquisition_fee`).
        if !lot_amount.is_positive() {
            let spent = (&(&transferred_cost + &cash_cost)? + &fee_cost)?;
            if spent.is_positive() { Engine::add(&mut self.fee_realised, self.venue.clone(), &spent.neg())?; }
            return Ok(());
        }
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
            // D-TRACKER-SWAP-FEE: the lot takes weight/total of `cost`, so the fee joins it whole (Rails weighted it twice, and two
            // equal tranches carried half of it).
            let cost = (&(&(&t.cost * &share)? * &total_weight)?.div(&t.weight)? + &fee_cost)?;
            let unpriced = if t.amount.is_positive() { (&tranche_amount * &t.unpriced)?.div(&t.amount)? } else { Dec::zero() };
            opened.push(Lot { amount: tranche_amount, cost_per_unit: cost.div(&rest)?, date: t.date, basis_assumed: t.basis_assumed || assumed, unpriced });
        }
        if cash_amount.is_positive() {
            let mut scale = if tranches.is_empty() { fee_cost.clone() } else if coin_value.is_positive() { coin_value.clone() } else { transferred_cost.clone() };
            // D-TRACKER-SWAP-FEE: when no tranche took any units, the fee joins the cash lot (Rails dropped it).
            if !tranches.is_empty() && !rest.is_positive() { scale = (&scale + &fee_cost)?; }
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
        let venue = self.venue.clone();
        let lots = self.lots(&r.base);
        // What is returned past the basis is realised (`roc_at`).
        let excess = if let (Some(per_unit), false) = (&per_unit, lots.is_empty()) {
            let mut excess = Dec::zero();
            for lot in lots.iter_mut() {
                excess = (&excess + &(&lot.amount * &max(&(per_unit - &lot.cost_per_unit)?, &Dec::zero()))?)?;
                lot.cost_per_unit = max(&(&lot.cost_per_unit - per_unit)?, &Dec::zero());
            }
            excess
        } else {
            let mut remaining = value;
            for lot in lots.iter_mut() {
                if !remaining.is_positive() { break; }
                let lot_cost = (&lot.amount * &lot.cost_per_unit)?;
                let take = min(&lot_cost, &remaining);
                lot.cost_per_unit = if lot.amount.is_positive() { (&lot_cost - &take)?.div(&lot.amount)? } else { Dec::zero() };
                remaining = (&remaining - &take)?;
            }
            remaining
        };
        Engine::add(&mut self.roc, venue, &excess)
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
        self.fee_value = Dec::zero();
        self.fee_is_cost = true;
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
        let remapped = net_daily_splits(rows.iter().map(Engine::remap).collect())?;
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
            out.push(Position { symbol: symbol.clone(), avg_cost: cost.div(&quantity)?, quantity, cost, opened_at: lots.iter().map(|l| l.date).min(),
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

/// The terms, the cash book, and per venue what coins cost when they left for nothing.
type MoneyIn = (Vec<Term>, Book, HashMap<String, Dec>);

/// `money_in_terms`: one term per row (two more for a linked transfer between venues), and the cash book.
fn money_in(rows: &[Row], engine: &mut Engine, ps: &mut PriceService) -> Result<MoneyIn, FiguresError> {
    let mut book = Book::default();
    // Per venue: what coins cost when they left for nothing (a fee row, a transfer's fee slice), realised as a loss.
    let mut costs: HashMap<String, Dec> = HashMap::new();
    let mut carried: HashMap<usize, Dec> = HashMap::new();
    let conversions = cash_conversions(rows, ps)?;
    let closes = closers(rows);
    let mut terms = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        crate::figures::budget::check()?;
        let kept = ps.warnings.len();
        let withdrawn = book_cash(&mut book, r, &mut carried, &conversions, ps)?;
        let cost = coin_cost(r, engine);
        let entry = costs.entry(r.exchange.clone()).or_insert_with(Dec::zero);
        *entry = (&*entry - &cost)?;
        let mut amount = contribution(r, engine, &withdrawn, ps)?;
        if let Some(venue) = closes.get(&i) { amount = (&amount + &unfunded(&mut book, venue, r, ps)?)?; }
        let complete = ps.warnings.len() == kept && !(r.price_missing && valued_by_price(r));
        terms.push(Term { at: r.at, amount, complete, opens: r.opening.then(|| (r.base.clone(), r.amount.clone())), exchange: r.exchange.clone(), received: in_kind(r) });
        if between_venues(r) {
            let moved = if cash(&r.base) { carried.get(&r.uid).cloned().unwrap_or_else(Dec::zero) } else { engine.moved.get(&r.uid).map_or_else(Dec::zero, |(m, _)| m.clone()) };
            let complete = !engine.moved.get(&r.uid).is_some_and(|(_, u)| *u);
            let to = r.to_exchange.clone().unwrap_or_default();
            terms.push(Term { at: r.at, amount: moved.neg(), complete, opens: None, exchange: r.exchange.clone(), received: false });
            terms.push(Term { at: r.at, amount: moved, complete, opens: None, exchange: to, received: false });
        }
    }
    Ok((terms, book, costs))
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
pub struct Walk { pub rows: Vec<Row>, pub engine: Engine, pub global: Engine, pub terms: Vec<Term>, book: Book, costs: HashMap<String, Dec> }

/// `Ledger.walk(user)`: enriched, ordered for transfers, walked account-wide and then located, and read for money in.
pub fn walk_rows(stored: &[Stored], deposits: &HashMap<i64, Dec>, ps: &mut PriceService) -> Result<Walk, FiguresError> {
    let rows = enrich(stored, deposits, ps)?;
    let mut next_uid = rows.len();
    let located_order = transfers_first(&rows, At(i64::MIN))?;
    let opened = open_with_held(rows, ps, false, &mut next_uid)?;
    let mut global = Engine::new(false);
    global.calculate(&taxable(&opened))?;
    let located = open_with_held(located_order, ps, true, &mut next_uid)?;
    let mut engine = Engine::new(true);
    engine.calculate(&taxable(&located))?;
    let (terms, book, costs) = money_in(&located, &mut engine, ps)?;
    Ok(Walk { rows: located, engine, global, terms, book, costs })
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

/// `round_trips`: one row per round trip, not per sell. What is still accumulating when the disposals run out is flushed
/// as its own row, only when FIFO matched a basis.
fn round_trips(disposals: &[&Disposal]) -> Result<Vec<RoundTrip>, FiguresError> {
    let mut open: Vec<RoundTrip> = vec![];
    let mut closed = vec![];
    for d in disposals {
        crate::figures::budget::check()?;
        let i = match open.iter().position(|t| t.symbol == d.asset) {
            Some(i) => i,
            None => { open.push(RoundTrip { symbol: d.asset.clone(), opened_at: d.acquired, closed_at: d.at, quantity: Dec::zero(), invested: Dec::zero(),
                                            proceeds: Dec::zero(), fees: Dec::zero(), realised: Dec::zero(), incomplete: false }); open.len() - 1 }
        };
        let t = &mut open[i];
        t.quantity = (&t.quantity + &d.amount)?;
        t.invested = (&t.invested + &d.cost_basis)?;
        t.proceeds = (&t.proceeds + &d.proceeds)?;
        t.fees = (&t.fees + &d.fee)?;
        t.realised = (&t.realised + &d.gain_loss)?;
        t.closed_at = d.at;
        t.incomplete = t.incomplete || d.data_incomplete;
        if d.closes_position { closed.push(open.remove(i)); }
    }
    closed.extend(open.into_iter().filter(|t| t.invested.is_positive()));
    Ok(closed)
}

/// Adds `amount` to `key`'s entry, a new key going last, as a Ruby Hash does.
fn add_to(list: &mut Vec<(String, Dec)>, key: &str, amount: &Dec) -> Result<(), FiguresError> {
    match list.iter_mut().find(|(k, _)| k == key) { Some((_, v)) => *v = (&*v + amount)?, None => list.push((key.to_string(), (&Dec::zero() + amount)?)) }
    Ok(())
}

/// `summarise`: a venue's slice of the walk.
fn summarise(w: &Walk, venue: &str, ps: &mut PriceService, now: At) -> Result<Summary, FiguresError> {
    let positions = w.engine.positions(Some(venue))?;
    let terms: Vec<&Term> = w.terms.iter().filter(|t| t.exchange == venue).collect();
    let disposals: Vec<&Disposal> = w.engine.disposals.iter().filter(|d| d.exchange == venue).collect();
    let rows: Vec<&Row> = w.rows.iter().filter(|r| r.exchange == venue).collect();
    let cash: Vec<(String, Dec)> = w.book.cash.iter().filter(|((v, _), _)| v == venue).map(|((_, c), a)| (c.clone(), a.clone())).collect();
    let kept = ps.warnings.len();
    // `cash_in_usd`, then `fees` (a fee ROW in fiat is valued here: `enrich` prices a fiat base at zero).
    let mut cash_usd = Dec::zero();
    for (currency, amount) in &cash { cash_usd = (&cash_usd + &if stable(currency) { amount.clone() } else { ps.convert_fiat(amount, currency, "USD", now)? })?; }
    let mut fees = Dec::zero();
    for r in &rows {
        fees = (&fees + &r.fee_fiat_value)?;
        if r.kind == Kind::Fee { fees = (&fees + &if fiat(&r.base) { ps.convert_fiat(&r.amount, &r.base, "USD", r.at)? } else { r.fiat_value.clone() })?; }
    }
    let incomplete = positions.iter().any(|p| p.unpriced.is_positive()) || w.engine.uncovered_venues.contains(&Some(venue.to_string()))
        || disposals.iter().any(|d| d.unpriced_quantity.is_positive())
        || rows.iter().any(|r| r.price_missing) || terms.iter().any(|t| !t.complete) || ps.warnings.len() > kept;
    let gains = sum(disposals.iter().map(|d| Ok(d.gain_loss.clone())))?;
    let book_realised = w.book.realised.get(venue).cloned().unwrap_or_else(Dec::zero);
    let costs = w.costs.get(venue).cloned().unwrap_or_else(Dec::zero);
    let mut openings: Vec<(String, Dec)> = vec![];
    for r in rows.iter().filter(|r| r.opening) {
        match openings.iter_mut().find(|(k, _)| *k == r.base) { Some((_, v)) => *v = r.amount.clone(), None => openings.push((r.base.clone(), r.amount.clone())) }
    }
    Ok(Summary {
        positions,
        total_invested: sum(terms.iter().map(|t| Ok(t.amount.clone())))?,
        received: sum(terms.iter().filter(|t| t.received).map(|t| Ok(t.amount.clone())))?,
        realised: (&(&(&(&gains + &w.engine.roc_at(venue))? + &w.engine.fee_asset_realised_at(venue))? + &book_realised)? + &costs)?,
        fees,
        cash_usd,
        unpriced_proceeds: sum(disposals.iter().map(|d| Ok(d.unpriced_proceeds.clone())))?,
        round_trips: round_trips(&disposals)?,
        cash,
        cash_basis: w.book.basis.iter().filter(|((v, _), b)| v == venue && !b.is_zero()).map(|((_, c), b)| (c.clone(), b.clone())).collect(),
        incomplete,
        loss_sales: vec![],
        openings,
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
    let (mut received, mut realised, mut fees, mut cash_usd, mut unpriced_proceeds) = (Dec::zero(), Dec::zero(), Dec::zero(), Dec::zero(), Dec::zero());
    let (mut round_trips, mut openings) = (vec![], vec![]);
    for (_, s) in venues {
        total = (&total + &s.total_invested)?;
        received = (&received + &s.received)?;
        realised = (&realised + &s.realised)?;
        fees = (&fees + &s.fees)?;
        cash_usd = (&cash_usd + &s.cash_usd)?;
        unpriced_proceeds = (&unpriced_proceeds + &s.unpriced_proceeds)?;
        round_trips.extend(s.round_trips.iter().cloned());
        for (symbol, quantity) in &s.openings { add_to(&mut openings, symbol, quantity)?; }
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
        positions.push(Position { symbol, avg_cost: cost.div(&quantity)?, quantity, cost, opened_at: list.iter().filter_map(|p| p.opened_at).min(), estimated: list.iter().any(|p| p.estimated),
                                  unpriced: sum(list.iter().map(|p| Ok(p.unpriced.clone())))? });
    }
    positions.sort_by(|a, b| b.cost.cmp(&a.cost));
    Ok(Summary {
        positions, total_invested: total, cash, cash_basis: basis.into_iter().filter(|(_, b)| !b.is_zero()).collect(),
        incomplete: venues.iter().any(|(_, s)| s.incomplete) || w.engine.uncovered || !ps.warnings.is_empty(),
        loss_sales: loss_sales(&w.global.disposals, ps.today)?,
        round_trips, received, realised, fees, cash_usd, unpriced_proceeds, openings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "2026-09-10 10:00:00";
    fn d(s: &str) -> Dec { Dec::strict(s).unwrap() }
    fn row(uid: usize, kind: Kind, base: &str, amount: &str, at: &str, exchange: &str) -> Row {
        Row { uid, kind, base: base.into(), amount: d(amount), quote: None, quote_amount: None, fiat_value: Dec::zero(), fee_fiat_value: Dec::zero(),
              fee_currency: None, fee_amount: None, at: At::from_sql(at).unwrap(), group: None, price_missing: false, value_missing: None,
              exchange: exchange.into(), linked: false, transfer_fee: None, swap_fiat_cost: None, swap_stable_cost: None, per_share: None,
              to_exchange: None, orphan: false, opening: false, stated_value: false, fee_missing: false }
    }
    /// A 1 BTC withdrawal from `from` linked to a deposit on `to`.
    fn send(uid: usize, from: &str, to: &str) -> Row { Row { linked: true, to_exchange: Some(to.into()), ..row(uid, Kind::Withdrawal, "BTC", "1", T, from) } }
    fn holds(uid: usize, venue: &str) -> Row { row(uid, Kind::Deposit, "BTC", "1", "2026-09-01 10:00:00", venue) }
    fn order(rows: &[Row]) -> Vec<usize> { transfers_first(rows, At(i64::MIN)).unwrap().iter().map(|r| r.uid).collect() }

    /// D-TRACKER-TRANSFER-ORDER: a withdrawal waits for its funds, never for a linked deposit that moves none of its coin;
    /// with nothing to wait for it goes as Rails sends it (later ones first); transfers waiting on each other go latest first.
    #[test]
    fn a_transfer_waits_for_its_funds() {
        // X1: a sale on c, the linked deposit on b of coins that arrived earlier, then b→c: the withdrawal goes first.
        let arrived = Row { linked: true, ..row(2, Kind::Deposit, "BTC", "1", T, "b") };
        assert_eq!(order(&[holds(0, "b"), row(1, Kind::Sell, "BTC", "1", T, "c"), arrived, send(3, "b", "c")]), [0, 3, 1, 2]);
        // X2: nobody holds; a deposit on a funds a→b, which funds b→c.
        assert_eq!(order(&[row(0, Kind::Deposit, "BTC", "1", T, "a"), send(1, "a", "b"), send(2, "b", "c")]), [0, 1, 2]);
        // X3 and X3b: a holds 1; the chain runs a→b, b→c whatever the input order.
        assert_eq!(order(&[holds(0, "a"), send(1, "a", "b"), send(2, "b", "c")]), [0, 1, 2]);
        assert_eq!(order(&[holds(0, "a"), send(1, "b", "c"), send(2, "a", "b")]), [0, 2, 1]);
        // X4 and X4b: reciprocal, a holds 1: a→b first either way.
        assert_eq!(order(&[holds(0, "a"), send(1, "a", "b"), send(2, "b", "a")]), [0, 1, 2]);
        assert_eq!(order(&[holds(0, "a"), send(1, "b", "a"), send(2, "a", "b")]), [0, 2, 1]);
        // X5: a cycle nobody funds: the latest goes first, then each as it is funded.
        assert_eq!(order(&[send(1, "a", "b"), send(2, "b", "c"), send(3, "c", "a")]), [3, 1, 2]);
        // X6 and X6b: a funded cycle runs from its funds, whatever the input order.
        assert_eq!(order(&[holds(0, "a"), send(1, "a", "b"), send(2, "b", "c"), send(3, "c", "a")]), [0, 1, 2, 3]);
        assert_eq!(order(&[holds(0, "a"), send(1, "c", "a"), send(2, "b", "c"), send(3, "a", "b")]), [0, 3, 2, 1]);
        // X7: an opening already forced on a (a withdrawal with no history) counts: a holds 1 again after the deposit.
        let earlier = [row(0, Kind::Withdrawal, "BTC", "1", "2026-09-08 10:00:00", "a"), row(1, Kind::Deposit, "BTC", "1", "2026-09-09 10:00:00", "a")];
        assert_eq!(order(&[earlier[0].clone(), earlier[1].clone(), send(2, "a", "b"), send(3, "b", "a")]), [0, 1, 2, 3]);
        // X9: same-day adjustments that net to nothing force no opening: a holds nothing, b's 1 goes first.
        let adjust = |uid, amount: &str| row(uid, Kind::Adjustment, "BTC", amount, "2026-09-09 10:00:00", "a");
        assert_eq!(order(&[holds(0, "b"), adjust(1, "-1"), adjust(2, "1"), send(3, "b", "a"), send(4, "a", "b")]), [0, 1, 2, 3, 4]);
        // Underfunded with nothing to wait for: as Rails sends it, before the other rows (later ones first).
        assert_eq!(order(&[row(1, Kind::Sell, "BTC", "1", T, "b"), send(2, "a", "b"), send(3, "c", "b")]), [3, 2, 1]);
    }

    /// The barriers: a row of the venue touching the coin, a linked deposit only when its positive fee is a third coin that
    /// is the coin, or its own cash base. One instant is one `At`: a nanosecond apart is another instant.
    #[test]
    fn a_linked_deposit_holds_a_transfer_back_only_by_a_fee_that_moves_its_coin() {
        let linked = |uid, base: &str, fee: Option<(&str, &str)>| Row { linked: true, fee_currency: fee.map(|f| f.0.into()), fee_amount: fee.map(|f| d(f.1)),
                                                                        ..row(uid, Kind::Deposit, base, "1", T, "a") };
        let first = |deposit: Row| order(&[holds(0, "a"), row(1, Kind::Sell, "BTC", "1", T, "b"), deposit, send(3, "a", "b")]);
        for (fee, held_back) in [(None, false), (Some(("BTC", "0")), false), (Some(("BTC", "0.001")), false), (Some(("BNB", "0.1")), false)] {
            assert_eq!(first(linked(2, "BTC", fee)), if held_back { vec![0, 1, 2, 3] } else { vec![0, 3, 1, 2] }, "{fee:?}");
        }
        // An ETH deposit whose fee is in BTC moves BTC on a: it holds the BTC withdrawal back.
        assert_eq!(first(linked(2, "ETH", Some(("BTC", "0.001")))), [0, 1, 2, 3]);
        // A cash deposit's own fee is a barrier for that cash; without a fee it is none.
        let euros = |fee| order(&[row(0, Kind::Deposit, "EUR", "100", "2026-09-01 10:00:00", "a"), row(1, Kind::Buy, "BTC", "1", T, "b"), linked(2, "EUR", fee),
                                  Row { linked: true, to_exchange: Some("b".into()), ..row(3, Kind::Withdrawal, "EUR", "100", T, "a") }]);
        assert_eq!(euros(Some(("EUR", "1"))), [0, 1, 2, 3]);
        assert_eq!(euros(None), [0, 3, 1, 2]);
        // A nanosecond later is another instant; before `from` nothing moves.
        let later = At(At::from_sql(T).unwrap().0 + 1);
        assert_eq!(order(&[holds(0, "a"), row(1, Kind::Sell, "BTC", "1", T, "b"), Row { at: later, ..send(2, "a", "b") }]), [0, 1, 2]);
        let rows = [holds(0, "a"), row(1, Kind::Sell, "BTC", "1", T, "b"), send(2, "a", "b")];
        assert_eq!(transfers_first(&rows, At(At::from_sql(T).unwrap().0 + 1)).unwrap().iter().map(|r| r.uid).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(order(&rows), [0, 2, 1]);
    }

    /// D-TRACKER-COIN-QUOTE: a swap-in whose own-coin fee takes every unit that arrived opens nothing, and the basis it
    /// carried is realised as a loss, once (Rails, booking the trade as a buy, realised the buy's cost instead).
    #[test]
    fn a_swap_in_fee_that_takes_everything_realises_the_carried_basis() {
        let g = Some("coin-quote:2".to_string());
        let rows = [Row { fiat_value: d("100"), ..row(0, Kind::Buy, "BTC", "0.002", "2026-09-01 10:00:00", "a") },
                    Row { fiat_value: d("150"), group: g.clone(), ..row(1, Kind::SwapOut, "BTC", "0.002", T, "a") },
                    Row { fiat_value: d("150"), group: g, fee_currency: Some("ETH".into()), fee_amount: Some(d("0.05")), fee_fiat_value: d("150"),
                          ..row(2, Kind::SwapIn, "ETH", "0.05", T, "a") }];
        let mut e = Engine::new(true);
        e.calculate(&rows).unwrap();
        assert!(e.positions(Some("a")).unwrap().is_empty());
        assert_eq!(e.fee_asset_realised_at("a").to_s_f(), "-100.0");
    }

    /// D-TRACKER-SWAP-FEE: a swap-in's fee joins its lots whole, however many tranches it carries; with tranches that took no
    /// units, it joins the cash lot.
    #[test]
    fn a_swap_in_fee_joins_its_lots_once() {
        let cost = |lots: &[(&str, &str)]| {
            let mut rows = vec![Row { fiat_value: d("300"), ..row(0, Kind::Buy, "BNB", "1", "2026-09-01 10:00:00", "a") }];
            for (i, (amount, value)) in lots.iter().enumerate() { rows.push(Row { fiat_value: d(value), ..row(i + 1, Kind::Buy, "ETH", amount, "2026-09-02 10:00:00", "a") }); }
            let g = Some("swap".to_string());
            rows.push(Row { fiat_value: d("2000"), group: g.clone(), ..row(10, Kind::SwapOut, "ETH", "1", T, "a") });
            rows.push(Row { fiat_value: d("3000"), group: g, fee_currency: Some("BNB".into()), fee_amount: Some(d("0.1")), fee_fiat_value: d("30"),
                            ..row(11, Kind::SwapIn, "BTC", "0.05", T, "a") });
            let mut e = Engine::new(true);
            e.calculate(&rows).unwrap();
            e.positions(Some("a")).unwrap().into_iter().find(|p| p.symbol == "BTC").unwrap().cost.to_s_f()
        };
        assert_eq!(cost(&[("1", "2000")]), "2030.0");
        assert_eq!(cost(&[("0.5", "1000"), ("0.5", "1000")]), "2030.0", "Rails: 2,015");
        assert_eq!(cost(&[("0.25", "500"), ("0.75", "1500")]), "2030.0");
        // A coin nobody held (a tranche of nothing at no cost) and $100 of cash: every unit is the cash lot's, the fee with it.
        let g = Some("swap".to_string());
        let rows = [Row { fiat_value: d("300"), ..row(0, Kind::Buy, "BNB", "1", "2026-09-01 10:00:00", "a") },
                    Row { group: g.clone(), ..row(1, Kind::SwapOut, "XYZ", "1", T, "a") },
                    Row { group: g, swap_fiat_cost: Some(d("100")), fee_currency: Some("BNB".into()), fee_amount: Some(d("0.1")), fee_fiat_value: d("30"),
                          ..row(2, Kind::SwapIn, "BTC", "0.05", T, "a") }];
        let mut e = Engine::new(true);
        e.calculate(&rows).unwrap();
        assert_eq!(e.positions(Some("a")).unwrap().into_iter().find(|p| p.symbol == "BTC").unwrap().cost.to_s_f(), "130.0", "Rails: 100");
    }
}
