//! Sweden (IL 44, 48 kap. 7 §, 52 kap. 3, 5 §§, 65 kap. 7 §, 67 kap. 10 §; Skatteverket "Kryptotillgångar"): the K4
//! avsnitt D summary and its notes (A12-5). The engine's part is `fifo::Options::average_all` (genomsnittsmetoden, one
//! average per coin per person) with `lost_kept`, `own_fee_disposed` and `defer_group_fees`.
//! - D-TAX-SE-K4: per coin a gains row and a losses row of K4 avsnitt D, the sums for INK1 7.5 (gains) and 8.4 (the full
//!   loss), the 70% Skatteverket applies, staking and lending income (7.2), the year's surplus or deficit, its 30% tax or
//!   its reduction (30% to 100 000 kr, 21% above). Rails: Σ gains, Σ losses and 70% only.
//! - D-TAX-SE-INCOME, D-TAX-SE-AIRDROP: mining (hobby income, 1.6) and airdrops (no guidance; out of the income section,
//!   their lots assumed) listed in NOTE `se_income`.
use super::{cents, fifo::Disposal, line, s, Cell};
use crate::figures::{dec::Dec, FiguresError};
use crate::web::i18n::text;
use std::collections::BTreeMap;

/// D-TAX-SE-YEARS: the first and last years whose law was reviewed.
pub const FIRST_YEAR: i32 = 2020;
pub const LAST_YEAR: i32 = 2026;

fn d(v: &str) -> Dec { Dec::strict(v).unwrap_or_else(|_| Dec::zero()) }

/// One K4 row: antal, försäljningspris (proceeds less fees), omkostnadsbelopp, each summed from the printed rows in cents.
struct K4 { rows: usize, units: Dec, price: Dec, cost: Dec }

impl K4 {
    fn new() -> K4 { K4 { rows: 0, units: Dec::zero(), price: Dec::zero(), cost: Dec::zero() } }
    fn add(&mut self, units: &Dec, price: &Dec, cost: &Dec) -> Result<(), FiguresError> {
        self.rows += 1;
        (self.units, self.price, self.cost) = ((&self.units + units)?, (&self.price + price)?, (&self.cost + cost)?);
        Ok(())
    }
    fn result(&self) -> Result<Dec, FiguresError> { Ok((&self.price - &self.cost)?) }
}

/// D-TAX-SE-K4 after the rows, when the year has a disposal or staking and lending income (`capital_income`, the values
/// of the year's 7.2 receipts). A disposal with no gain is a row of the gains line (reported, vinst 0).
pub(super) fn k4(locale: &str, results: &[Disposal], capital_income: &[Dec]) -> Result<String, FiguresError> {
    let mut out = String::new();
    if results.is_empty() && capital_income.is_empty() { return Ok(out); }
    let t = |k: &str| text(locale, &format!("tax_report.se.{k}"), &[]);
    let mut coins: BTreeMap<&str, (K4, K4)> = BTreeMap::new();
    for r in results {
        let price = (&r.proceeds.round(2)? - &r.fee.round(2)?)?;
        let cost = r.cost_basis.round(2)?;
        let coin = coins.entry(&r.asset).or_insert_with(|| (K4::new(), K4::new()));
        let side = if (&price - &cost)?.is_negative() { &mut coin.1 } else { &mut coin.0 };
        side.add(&r.amount, &price, &cost)?;
    }
    line(&mut out, &[]);
    line(&mut out, &[s(t("heading"))]);
    line(&mut out, &t("columns").split(", ").map(|c| s(c.to_string())).collect::<Vec<_>>());
    let (mut gains, mut losses) = (Dec::zero(), Dec::zero());
    for (coin, (gain, loss)) in &coins {
        for (k4, is_gain) in [(gain, true), (loss, false)] {
            if k4.rows == 0 { continue; }
            let result = k4.result()?;
            let mut row = vec![s(coin.to_string()), Cell::Raw(k4.units.round(8)?.to_s_f()), cents(&k4.price)?, cents(&k4.cost)?];
            if is_gain { gains = (&gains + &result)?; row.extend([cents(&result)?, Cell::Nil]); }
            else { losses = (&losses - &result)?; row.extend([Cell::Nil, cents(&result.neg())?]); }
            line(&mut out, &row);
        }
    }
    let deductible = (&losses * &d("0.7"))?.round(2)?;
    let mut income = Dec::zero();
    for v in capital_income { income = (&income + &v.round(2)?)?; }
    let result = (&(&gains - &deductible)? + &income)?;
    line(&mut out, &[s(t("sum_gain")), cents(&gains)?]);
    line(&mut out, &[s(t("sum_loss")), cents(&losses)?]);
    line(&mut out, &[s(t("deductible")), cents(&deductible)?]);
    line(&mut out, &[s(t("capital_income")), cents(&income)?]);
    line(&mut out, &[s(t("result")), cents(&result)?]);
    if result.is_negative() {
        // 67 kap. 10 §: 30% of the deficit up to 100 000 kr, 21% of the rest.
        let deficit = result.neg();
        let cap = Dec::from_i64(100_000);
        let (low, high) = if deficit > cap { (cap.clone(), (&deficit - &cap)?) } else { (deficit, Dec::zero()) };
        line(&mut out, &[s(t("reduction")), cents(&(&(&low * &d("0.3"))? + &(&high * &d("0.21"))?)?)?]);
    } else {
        line(&mut out, &[s(t("tax")), cents(&(&result * &d("0.3"))?)?]);
    }
    line(&mut out, &[s(t("hint"))]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figures::at::At;

    fn sale(asset: &str, amount: &str, proceeds: &str, cost: &str, fee: &str) -> Disposal {
        let gain = (&(&d(proceeds) - &d(cost)).unwrap() - &d(fee)).unwrap();
        Disposal { at: At(0), acquired: None, asset: asset.into(), amount: d(amount), proceeds: d(proceeds), cost_basis: d(cost), fee: d(fee), gain_loss: gain,
                   holding_days: 0, complete: true, incomplete: false, tx_id: None, exchange: "Kraken".into(), long_held: None, uncovered: Dec::zero(),
                   averaged: false, mined: false, source: 0, recent: false, restricted: Dec::zero(), released: Dec::zero(), unusable: Dec::zero(),
                   effective: At(0), rule: None }
    }

    /// `se_average_gain_and_loss`'s rows: BTC vinst 1.5 / 439,835 / 313,681.50 / 126,153.50; ETH förlust 2 / 33,000 /
    /// 44,000 / 11,000; 7.5 126,153.50, 8.4 11,000, 70% 7,700, result 118,453.50, tax 35,536.05.
    #[test]
    fn the_k4_rows_and_the_tax() {
        let rows = [sale("BTC", "0.5", "165000", "82582.5", "165"), sale("ETH", "2", "33000", "44000", "0"), sale("BTC", "1", "275000", "231099", "0")];
        let out = k4("sv", &rows, &[]).unwrap();
        assert!(out.contains("\nBTC,1.5,439835.0,313681.5,126153.5,\nETH,2.0,33000.0,44000.0,,11000.0\n"), "{out}");
        for want in ["(punkt 7.5),126153.5\n", "hela förlusten)\",11000.0\n", "automatiskt)\",7700.0\n", "(punkt 7.2),0.0\n", "kryptotillgångar\",118453.5\n", "Skatt 30 %,35536.05\n"] {
            assert!(out.contains(want), "{want}: {out}");
        }
    }

    /// 67 kap. 10 §: a deficit of 308,000 → 30% × 100,000 + 21% × 208,000 = 73,680; no disposal and no income: nothing.
    #[test]
    fn the_reduction_above_100k() {
        let out = k4("sv", &[sale("BTC", "1", "660000", "1100000", "0")], &[]).unwrap();
        assert!(out.contains(",-308000.0\n") && out.contains("därutöver)\",73680.0\n"), "{out}");
        assert_eq!(k4("sv", &[], &[]).unwrap(), "");
    }
}
