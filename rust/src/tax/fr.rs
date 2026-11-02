//! France (CGI art. 150 VH bis; BOI-RPPM-PVBMC-30; form 2086): the PVCT report (A12-6). The engine's part is
//! `fifo::Options::pvct_from` (the units held before each cession and the running acquisition price).
//! - D-TAX-FR-FEE-RATIO: l. 218 = C − f; the fraction l. 223 × C / V takes the gross price (BOFiP -30-20 § 50); Rails put
//!   C − f in the ratio too. Fractions carried unrounded across every cession from 2019, printed in cents.
//! - D-TAX-FR-PORTFOLIO-VALUE: V = each coin held × the day's price, the coin sold at the cession's own price × max(units
//!   held, units sold), so V ≥ C; a coin is priced only where its symbol that day still means the coin held.
//! - D-TAX-FR-INCOMPLETE: an uncertain l. 220 (`Point::uncertain`), a missing V price, a missing cession value or an
//!   uncovered sale makes that cession and every later one incomplete.
//! - D-TAX-FR-305, D-TAX-FR-RATE, D-TAX-FR-2086: the 2086 summary (`summary`).
use super::{cents, fifo::{Disposal, Point}, line, s, Cell};
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::tracker::prices::{coin_id, PriceService};
use crate::web::i18n::{text, Arg};
use chrono::NaiveDate;

/// D-TAX-FR-YEARS: the first and last years whose law was reviewed (150 VH bis applies to cessions from 1 January 2019).
pub const FIRST_YEAR: i32 = 2019;
pub const LAST_YEAR: i32 = 2026;

fn d(v: &str) -> Dec { Dec::strict(v).unwrap_or_else(|_| Dec::zero()) }

/// One cession's 2086 figures: l. 213 `proceeds`, 214 `fee`, 212 `value`, 220 `gross`, 221 `fractions` (before it), the gain.
#[derive(Clone, Debug)]
pub(super) struct Cession { pub source: usize, pub proceeds: Dec, pub fee: Dec, pub value: Dec, pub gross: Dec, pub fractions: Dec, pub gain: Dec, pub incomplete: bool }

impl Cession {
    /// l. 218.
    pub fn net(&self) -> Result<Dec, FiguresError> { Ok((&self.proceeds - &self.fee)?) }
    /// l. 223 (no soulte is detected: l. 222 is 0).
    pub fn pta(&self) -> Result<Dec, FiguresError> { Ok((&self.gross - &self.fractions)?) }
}

/// What one cession brings to the chain: C, f, l. 220, V, and whether anything it rests on is missing.
pub(super) struct Input { pub source: usize, pub proceeds: Dec, pub fee: Dec, pub gross: Dec, pub value: Dec, pub missing: bool }

/// D-TAX-FR-FEE-RATIO, D-TAX-FR-INCOMPLETE over every cession from 2019 in order: PV = C − f − l. 223 × C / V, l. 221 grows by
/// each fraction (unrounded); once one rests on a missing figure, it and every later one are incomplete.
pub(super) fn chain(inputs: Vec<Input>) -> Result<Vec<Cession>, FiguresError> {
    let (mut fractions, mut contaminated, mut out) = (Dec::zero(), false, vec![]);
    for i in inputs {
        contaminated |= i.missing;
        let pta = (&i.gross - &fractions)?;
        let fraction = if i.value.is_positive() { (&pta * &i.proceeds)?.div(&i.value)? } else { Dec::zero() };
        let gain = (&(&i.proceeds - &i.fee)? - &fraction)?;
        out.push(Cession { source: i.source, proceeds: i.proceeds, fee: i.fee, value: i.value, gross: i.gross, fractions: fractions.clone(), gain, incomplete: contaminated });
        fractions = (&fractions + &fraction)?;
    }
    Ok(out)
}

/// r4 item 1: two coins held under one ticker (TON on two venues): prices are stored by ticker, so neither is priced from it.
fn ambiguous(held: &[(String, String, String, Dec)], id: &str, symbol: &str) -> bool { held.iter().any(|(i, s, ..)| s == symbol && i != id) }

/// D-TAX-FR-PORTFOLIO-VALUE: V before the cession `d`, and whether a coin of it had no price. Each other coin held at the
/// day's price of its symbol on its venue, only where that symbol that day resolves to the coin held and no other coin held
/// shares the ticker (else unpriced and warned); the coin sold at C / units sold × max(units held, units sold).
pub(super) fn value(ps: &mut PriceService, currency: &str, p: &Point, d: &Disposal) -> Result<(Dec, bool), FiguresError> {
    let day = d.at.utc().date_naive();
    let (mut v, mut missing, mut sold) = (Dec::zero(), false, Dec::zero());
    for (id, symbol, venue, units) in &p.held {
        if *id == d.asset_id { sold = units.clone(); continue; }
        let same = !ambiguous(&p.held, id, symbol)
            && coin_id(ps.r, symbol, ps.r.venue(venue), Some(day)).unwrap_or_else(|| format!("symbol:{symbol}")) == *id;
        let price = if same { ps.price_at(symbol, currency, d.at, Some(venue))? } else { ps.warnings.push(format!("{symbol}/{currency} {day}")); Dec::zero() };
        if price.is_zero() { missing = true; }
        v = (&v + &(units * &price)?)?;
    }
    let held = if sold > d.amount { sold } else { d.amount.clone() };
    let own = if d.amount.is_positive() { (&d.proceeds * &held)?.div(&d.amount)? } else { d.proceeds.clone() };
    Ok(((&v + &own)?, missing))
}

/// D-TAX-FR-RATE: social levies 17.2% on income to 2024, 18.6% from income 2025 (LFSS 2026, CSS L. 136-6 I e)).
fn levies(year: i32) -> (&'static str, &'static str) { if year <= 2024 { ("0.172", "17,2 %") } else { ("0.186", "18,6 %") } }

/// D-TAX-FR-2086, D-TAX-FR-305, D-TAX-FR-RATE: the year's cessions as the 2086 lists them, 224 (Σ printed gains), 51 (Σ printed
/// net prices), the €305 test (all or nothing), 3AN or 3BN and the flat tax.
pub(super) fn summary(locale: &str, year: i32, rows: &[(&Disposal, &Cession)], local: &dyn Fn(At) -> NaiveDate) -> Result<String, FiguresError> {
    let mut out = String::new();
    if rows.is_empty() { return Ok(out); }
    let t = |k: &str| text(locale, &format!("tax_report.fr.{k}"), &[]);
    line(&mut out, &[]);
    line(&mut out, &[s(text(locale, "tax_report.fr.heading", &[("year", Arg::Text(&year.to_string()))]))]);
    line(&mut out, &t("columns").split(", ").map(|c| s(c.to_string())).collect::<Vec<_>>());
    let (mut total, mut prices) = (Dec::zero(), Dec::zero());
    for (n, (d, c)) in rows.iter().enumerate() {
        let (net, gain) = (c.net()?.round(2)?, c.gain.round(2)?);
        total = (&total + &gain)?;
        prices = (&prices + &net)?;
        line(&mut out, &[Cell::Raw((n + 1).to_string()), s(local(d.at).format("%Y-%m-%d").to_string()), cents(&c.value)?, cents(&c.proceeds)?, cents(&c.fee)?,
                         cents(&net)?, cents(&c.gross)?, cents(&c.fractions)?, cents(&c.pta()?)?, cents(&gain)?]);
    }
    line(&mut out, &[s(t("total")), cents(&total)?]);
    line(&mut out, &[s(t("prices")), cents(&prices)?]);
    if prices <= Dec::from_i64(305) {
        line(&mut out, &[s(t("exempt"))]);
        line(&mut out, &[s(t("box_3an")), cents(&Dec::zero())?]);
    } else if total.is_negative() {
        line(&mut out, &[s(t("box_3bn")), cents(&total.neg())?]);
    } else {
        let (rate, label) = levies(year);
        let (ir, ps) = ((&total * &d("0.128"))?.round(2)?, (&total * &d(rate))?.round(2)?);
        line(&mut out, &[s(t("box_3an")), cents(&total)?]);
        line(&mut out, &[s(t("tax_ir")), cents(&ir)?]);
        line(&mut out, &[s(text(locale, "tax_report.fr.tax_ps", &[("rate", Arg::Text(label))])), cents(&ps)?]);
        line(&mut out, &[s(t("tax_total")), cents(&(&ir + &ps)?)?]);
    }
    line(&mut out, &[s(t("hint"))]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn input(source: usize, c: &str, f: &str, gross: &str, v: &str, missing: bool) -> Input {
        Input { source, proceeds: d(c), fee: d(f), gross: d(gross), value: d(v), missing }
    }

    /// Three cessions of a third each: the fractions are carried unrounded (1,000 / 3 each, l. 221 666.67 printed before the
    /// third), and the printed gains add up in cents; a missing figure on the second makes the third incomplete too.
    #[test]
    fn fractions_carried_unrounded_over_three_cessions() {
        let out = chain(vec![input(1, "100", "0", "1000", "300", false), input(2, "100", "1", "1000", "200", true), input(3, "100", "0", "1000", "100", false)]).unwrap();
        let first = &out[0];
        assert_eq!((first.gain.round(2).unwrap().to_s_f(), first.incomplete), ("-233.33".to_string(), false));
        // l. 223 after the first: 1000 − 333.333… = 666.666…; × 100 / 200 = 333.333…
        assert_eq!((out[1].pta().unwrap().round(2).unwrap().to_s_f(), out[1].gain.round(2).unwrap().to_s_f(), out[1].incomplete), ("666.67".into(), "-234.33".into(), true));
        assert_eq!((out[2].fractions.round(2).unwrap().to_s_f(), out[2].gain.round(2).unwrap().to_s_f(), out[2].incomplete), ("666.67".into(), "-233.33".into(), true));
    }

    /// r4 item 1: two identities under one ticker at a cession are priced for neither: both 0 in V, warned, a missing value.
    #[test]
    fn an_ambiguous_ticker_is_priced_for_neither() {
        use crate::tracker::{fx::FxTable, prices::{Reference, Unfetched}};
        let held = vec![("bitcoin".to_string(), "BTC".to_string(), "kraken".to_string(), d("1")),
                        ("the-open-network".into(), "TON".into(), "kraken".into(), d("10")), ("symbol:TON".into(), "TON".into(), "binance".into(), d("10"))];
        assert!(ambiguous(&held, "the-open-network", "TON") && ambiguous(&held, "symbol:TON", "TON") && !ambiguous(&held, "bitcoin", "BTC"));
        let (r, fx, mut f) = (Reference::default(), FxTable::default(), Unfetched);
        let mut ps = PriceService::new(&r, &fx, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), &mut f);
        let at = At::from_utc(chrono::Utc.with_ymd_and_hms(2025, 6, 1, 10, 0, 0).unwrap()).unwrap();
        let p = Point { uid: 7, held, gross: d("1000"), uncertain: false };
        let sale = Disposal { at, acquired: None, asset: "BTC".into(), amount: d("0.5"), proceeds: d("40000"), cost_basis: Dec::zero(), fee: Dec::zero(),
                              gain_loss: Dec::zero(), holding_days: 0, complete: true, incomplete: false, tx_id: None, exchange: "kraken".into(), long_held: None,
                              uncovered: Dec::zero(), averaged: false, mined: false, source: 7, recent: false, restricted: Dec::zero(), released: Dec::zero(),
                              unusable: Dec::zero(), effective: at, rule: None, asset_id: "bitcoin".into() };
        let (v, missing) = value(&mut ps, "EUR", &p, &sale).unwrap();
        assert_eq!((v.to_s_f(), missing), ("80000.0".to_string(), true));
        assert_eq!(ps.warnings, ["TON/EUR 2025-06-01", "TON/EUR 2025-06-01"]);
    }

    /// `fr_two_sales`: the fee comes off the first term only (+1,267.71, not Rails' +1,283.14), then −315.71.
    #[test]
    fn the_fee_comes_off_the_first_term_only() {
        let out = chain(vec![input(1, "9000", "18", "18000", "21000", false), input(2, "15000", "30", "23000", "15000", false)]).unwrap();
        let printed: Vec<String> = out.iter().map(|c| c.gain.round(2).unwrap().to_s_f()).collect();
        assert_eq!(printed, ["1267.71", "-315.71"]);
        assert_eq!(out[1].pta().unwrap().round(2).unwrap().to_s_f(), "15285.71");
    }
}
