//! Bulgaria by its own law (ЗДДФЛ чл. 33 ал. 3–6, чл. 11 ал. 3, чл. 48; ГДД по чл. 50, Приложение № 5, Таблица 2):
//! - D-TAX-BG-AVERAGE: every sale's cost is the weighted average of the units of the kind held, across every account (чл.
//!   33 ал. 4; `fifo::Options::average_all`). Rails used FIFO.
//! - D-TAX-BG-CURRENCY: BGN for every tax year up to 2025, EUR from 2026 (чл. 11 ал. 3); EUR↔BGN at the peg 1.95583
//!   exactly, another currency at BNB's rate 1.95583 / ECB rounded to six significant digits (`Peg`). Rails: BGN for 2025
//!   only, at the ECB's BGN rate.
//! - D-TAX-BG-ANNUAL: Annex 5 Table 2 (`annual`): the 10% expenses once on the year's net result from 2024 (none before),
//!   tax 10% (чл. 48). Rails took 10% off each positive gain.
//! - D-TAX-BG-DEPOSIT-ZERO: an unlinked deposit costs 0 (чл. 33 ал. 6 т. 3; `fifo::Options::deposit_cost_zero`).
use super::{cents, fifo::Disposal, line, s, Cell};
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::tracker::fx::FxTable;
use crate::tracker::prices::Statutory;
use crate::web::i18n::{text, Arg};
use chrono::NaiveDate;

/// The last tax year declared in leva.
pub const BGN_UNTIL: i32 = 2025;
/// BGN per euro, fixed (Закон за БНБ, the currency board).
pub const PEG: &str = "1.95583";
/// From 2024 Table 2 has its crypto row (ред 2, код 5082) and the 10% expenses (ДВ бр. 106/2023).
const CRYPTO_ROW_FROM: i32 = 2024;

fn peg() -> Dec { Dec::strict(PEG).unwrap_or_else(|_| Dec::one()) }

/// `v` rounded to `digits` significant digits.
fn significant(v: &Dec, digits: i64) -> Result<Dec, FiguresError> {
    let text = v.to_s_f();
    let (int, frac) = text.trim_start_matches('-').split_once('.').unwrap_or((&text, ""));
    let places = if int.trim_start_matches('0').is_empty() { digits + frac.chars().take_while(|c| *c == '0').count() as i64 } else { digits - int.len() as i64 };
    Ok(v.round(places)?)
}

/// D-TAX-BG-CURRENCY: BNB's conversion over the ECB's rates (the UTC day, with the ECB table's week of lookback).
pub struct Peg<'a> { pub fx: &'a FxTable }

impl Peg<'_> {
    /// BGN per unit of `currency`: 1, the peg, or 1.95583 / the ECB's rate, to six significant digits (BNB's published rate).
    fn leva(&self, currency: &str, day: NaiveDate) -> Option<Dec> {
        match currency {
            "BGN" => Some(Dec::one()),
            "EUR" => Some(peg()),
            _ => significant(&peg().div(&self.fx.rate("EUR", currency, day).ok()??).ok()?, 6).ok(),
        }
    }
}

impl Statutory for Peg<'_> {
    fn day(&self, at: At) -> NaiveDate { at.utc().date_naive() }
    fn rate(&self, from: &str, to: &str, day: NaiveDate) -> Option<Dec> {
        if from == to { return Some(Dec::one()); }
        if from != "BGN" && to != "BGN" { return self.fx.rate(from, to, day).ok().flatten(); }
        self.leva(from, day)?.div(&self.leva(to, day)?).ok()
    }
}

/// D-TAX-BG-ANNUAL: Приложение № 5, Таблица 2 from the year's rows (each already in cents): к. 3 Σ(proceeds − fee), к. 4
/// Σ cost, к. 5 Σ gains, к. 6 Σ losses; ред 5 = max(к. 5 − к. 6, 0) for crypto-assets only; ред 6 = 10% of ред 5 from 2024;
/// ред 7 = ред 5 − ред 6; tax 10% of ред 7.
pub(super) fn annual(out: &mut String, locale: &str, year: i32, results: &[Disposal]) -> Result<(), FiguresError> {
    if results.is_empty() { return Ok(()); }
    let (mut k3, mut k4, mut k5, mut k6) = (Dec::zero(), Dec::zero(), Dec::zero(), Dec::zero());
    for d in results {
        k3 = (&k3 + &(&d.proceeds - &d.fee)?)?;
        k4 = (&k4 + &d.cost_basis)?;
        if d.gain_loss.is_positive() { k5 = (&k5 + &d.gain_loss)?; } else { k6 = (&k6 - &d.gain_loss)?; }
    }
    let tenth = Dec::strict("0.1")?;
    let r5 = { let net = (&k5 - &k6)?; if net.is_positive() { net } else { Dec::zero() } };
    let crypto_row = year >= CRYPTO_ROW_FROM;
    let r6 = if crypto_row { (&r5 * &tenth)?.round(2)? } else { Dec::zero() };
    let r7 = (&r5 - &r6)?;
    let tax = (&r7 * &tenth)?.round(2)?;
    let t = |key: &str| text(locale, &format!("tax_report.bg.{key}"), &[]);
    line(out, &[]);
    line(out, &[s(if crypto_row { text(locale, "tax_report.bg.heading", &[("row", Arg::Text("2")), ("code", Arg::Text("5082"))]) } else { t("heading_before_2024") })]);
    for (key, v) in [("col3", &k3), ("col4", &k4), ("col5", &k5), ("col6", &k6), ("row5", &r5)] { line(out, &[s(t(key)), cents(v)?]); }
    if crypto_row { line(out, &[s(t("row6")), cents(&r6)?]); }
    line(out, &[s(t("row7")), cents(&r7)?]);
    line(out, &[s(t("tax")), cents(&tax)?]);
    line(out, &[Cell::Text(t("hint"))]);
    if !crypto_row { line(out, &[Cell::Text(t("hint_before_2024"))]); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec { Dec::strict(s).unwrap() }
    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn bnb_crosses_through_the_peg_to_six_significant_digits() {
        assert_eq!(significant(&d("1.7780272727"), 6).unwrap().to_s_f(), "1.77803");
        assert_eq!(significant(&d("0.0123456789"), 6).unwrap().to_s_f(), "0.0123457");
        assert_eq!(significant(&d("123.456789"), 6).unwrap().to_s_f(), "123.457");
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE fx_rates (currency TEXT, date TEXT, rate NUMERIC); INSERT INTO fx_rates VALUES ('USD', '2025-06-02', '1.1');").unwrap();
        let fx = FxTable::load(&c).unwrap();
        let p = Peg { fx: &fx };
        let on = day("2025-06-04");
        assert_eq!(p.rate("USD", "BGN", on).unwrap().to_s_f(), "1.77803");
        assert_eq!(p.rate("EUR", "BGN", on).unwrap().to_s_f(), "1.95583");
        assert_eq!((&d("58674.90") * &p.rate("BGN", "EUR", on).unwrap()).unwrap().round(2).unwrap().to_s_f(), "30000.0");
        assert_eq!(p.rate("USD", "EUR", on).unwrap().round(6).unwrap().to_s_f(), "0.909091");
        assert!(p.rate("USD", "BGN", day("2025-06-20")).is_none(), "no ECB rate within the week: missing");
    }
}
