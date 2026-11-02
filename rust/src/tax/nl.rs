//! The Netherlands (Wet IB 2001 art. 5.1–5.3, 5.19, 5.25–5.36): the box 3 snapshot at 1 January (A12-6), over
//! `wealth::held`:
//! - D-TAX-NL-PEILDATUM: units held before 1 January `year` 00:00 in the taxpayer's zone (Rails: UTC), valued at the daily
//!   price dated 1 January.
//! - D-TAX-NL-RATES: the forfaitair rendement, allowance and rate of each year 2021–2026 (`RATES`); Rails had 2025 and 2026
//!   only, 2026 at 7.78% and every earlier year at 2026's.
//! - D-TAX-NL-BOX3: Rails' summary rows, Dutch labels, a hint.
//! - D-TAX-NL-ACTUAL: once 1 January `year`+1 has come, crypto's contribution to the actual return (tegenbewijs): the
//!   value then, less the value at the peildatum, less the fiat paid for coins, plus the fiat received for them and income
//!   paid in fiat (gross of fees), plus the value of the coins paid as fees (each once, at the day's price).
use super::wealth::{fixed, summary, Held, Wealth};
use super::{cents, line, local_midnight, Report, INCOME};
use crate::figures::{dec::Dec, FiguresError};
use crate::tracker::prices::PriceService;
use crate::tracker::rows::Kind;
use crate::tracker::fiat;
use crate::web::i18n::{text, Arg};
use chrono::NaiveDate;

/// D-TAX-NL-YEARS: the first and last years whose figures were reviewed (the forfaitair rendement per category from 2021).
pub const FIRST_YEAR: i32 = 2021;
pub const LAST_YEAR: i32 = 2026;

/// D-TAX-NL-RATES: (year, heffingsvrij vermogen, forfaitair rendement overige bezittingen, tarief), Wet IB art. 5.2, 5.5,
/// 2.13 and the Belastingdienst's tables (2026: 6% final).
const RATES: [(i32, i64, &str, &str); 6] = [(2021, 50_000, "0.0569", "0.31"), (2022, 50_650, "0.0553", "0.31"), (2023, 57_000, "0.0617", "0.32"),
                                            (2024, 57_000, "0.0604", "0.36"), (2025, 57_684, "0.0588", "0.36"), (2026, 59_357, "0.06", "0.36")];

fn day(y: i32) -> Result<NaiveDate, FiguresError> { NaiveDate::from_ymd_opt(y, 1, 1).ok_or_else(|| FiguresError::Data("a year out of range".into())) }

impl Report {
    /// The holdings at the start of `y` (local), each valued at its price dated 1 January; their total of rounded values.
    fn nl_value(&self, ps: &mut PriceService, y: i32) -> Result<(Vec<(Held, Dec)>, Dec), FiguresError> {
        let until = local_midnight(&self.zone, y).ok_or_else(|| FiguresError::Data("a year out of range".into()))?;
        let held = self.held_at(until)?;
        let (mut lines, mut total) = (vec![], Dec::zero());
        for h in &held {
            let value = match self.unit_price(ps, h, &held, "EUR", day(y)?)? { Some(p) => (&p * &h.units)?.round(2)?, None => Dec::zero() };
            total = (&total + &value)?;
            lines.push((h.clone(), value));
        }
        Ok((lines, total))
    }

    pub(super) fn nl(&self, ps: &mut PriceService, today: NaiveDate) -> Result<Wealth, FiguresError> {
        let (y, l) = (self.year, self.j.locale);
        let t = |k: &str| text(l, k, &[]);
        let (lines, v0) = self.nl_value(ps, y)?;
        let &(_, allowance, deemed, rate) = RATES.iter().find(|r| r.0 == y).ok_or_else(|| FiguresError::NotComputed(format!("no box 3 rates for {y}")))?;
        let (deemed, rate) = (Dec::strict(deemed)?, Dec::strict(rate)?);
        let taxable = (&v0 - &Dec::from_i64(allowance))?;
        let taxable = if taxable.is_positive() { taxable } else { Dec::zero() };
        let deemed_return = (&taxable * &deemed)?;
        let tax = (&deemed_return * &rate)?;
        let hundred = Dec::from_i64(100);
        let mut out = String::new();
        summary(&mut out, t("tax_report.summary.total_value"), Some(cents(&v0)?));
        summary(&mut out, t("tax_report.summary.allowance"), Some(super::Cell::Raw(allowance.to_string())));
        summary(&mut out, t("tax_report.summary.taxable_wealth"), Some(cents(&taxable)?));
        summary(&mut out, format!("{} ({}%)", t("tax_report.summary.deemed_return"), fixed(&(&deemed * &hundred)?, 2)?), Some(cents(&deemed_return)?));
        summary(&mut out, format!("{} ({}%)", t("tax_report.summary.tax"), fixed(&(&rate * &hundred)?, 0)?), Some(cents(&tax)?));
        let next = day(y + 1)?;
        let mut until = local_midnight(&self.zone, y).ok_or_else(|| FiguresError::Data("a year out of range".into()))?;
        if today >= next {
            let (_, v1) = self.nl_value(ps, y + 1)?;
            let (paid, mut received) = self.fiat_trades(ps, "EUR")?;
            for r in self.scoped.iter().filter(|r| self.in_year(r.at) && fiat(&r.base) && INCOME.iter().any(|(k, _)| *k == r.kind)) {
                received = (&received + &ps.convert_fiat(&r.amount, &r.base, "EUR", r.at)?)?;
            }
            let fees = self.coin_fees(ps)?;
            let contribution = (&(&(&(&v1 - &v0)? - &paid)? + &received)? + &fees)?;
            let year = (y + 1).to_string();
            summary(&mut out, text(l, "tax_report.nl.value_next", &[("year", Arg::Text(&year))]), Some(cents(&v1)?));
            summary(&mut out, t("tax_report.nl.purchases"), Some(cents(&paid)?));
            summary(&mut out, t("tax_report.nl.sales"), Some(cents(&received)?));
            summary(&mut out, t("tax_report.nl.coin_fees"), Some(cents(&fees)?));
            summary(&mut out, t("tax_report.nl.actual_return"), Some(cents(&contribution)?));
            until = local_midnight(&self.zone, y + 1).ok_or_else(|| FiguresError::Data("a year out of range".into()))?;
        } else {
            summary(&mut out, text(l, "tax_report.nl.actual_unknown", &[("year", Arg::Text(&(y + 1).to_string()))]), None);
        }
        line(&mut out, &[super::s(t("tax_report.nl.hint"))]);
        Ok(Wealth { day: day(y)?, lines, summary: out, income: String::new(), notes: String::new(), until })
    }

    /// D-TAX-NL-ACTUAL (r2 item 1, r4 item 2): the value, at the day's price, of every coin paid as a fee in the year: a fee
    /// attached to any row (own coin or third), a fee row of its own, a linked transfer's network fee.
    fn coin_fees(&self, ps: &mut PriceService) -> Result<Dec, FiguresError> {
        let mut total = Dec::zero();
        for r in self.scoped.iter().filter(|r| self.in_year(r.at)) {
            let mut paid: Vec<(&str, Dec)> = vec![];
            let linked_out = r.kind == Kind::Withdrawal && r.linked_to.is_some();
            if let (Some(coin), Some(fee)) = (r.fee_currency.as_deref().filter(|c| !c.trim().is_empty() && !fiat(c)), &r.fee_amount) {
                // A linked withdrawal's own-coin fee is in its network fee (its amount less the deposit's).
                if fee.is_positive() && !(linked_out && coin == r.base) { paid.push((coin, fee.clone())); }
            }
            if !fiat(&r.base) {
                if r.kind == Kind::Fee && r.amount.is_positive() { paid.push((&r.base, r.amount.clone())); }
                if let Some(deposit) = r.linked_to.filter(|_| linked_out).and_then(|d| self.deposits.get(&d)) {
                    let network = (&r.amount - deposit)?;
                    if network.is_positive() { paid.push((&r.base, network)); }
                }
            }
            for (coin, units) in paid {
                let price = ps.price_at(coin, "EUR", r.at, Some(&r.venue))?;
                total = (&total + &(&price * &units)?)?;
            }
        }
        Ok(total)
    }
}
