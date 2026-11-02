//! The United Kingdom (TCGA 1992 ss. 1, 1H, 1K, 104, 105, 106A, 288(1ZA); HMRC Cryptoassets Manual): the SA108 summary
//! and its notes (A12-5). The engine's part is `fifo::Options::average_all` (the s. 104 pool) with the identification
//! (`fifo::Options::uk`), `lost_kept`, `own_fee_disposed`, `swap_fee_split` and `defer_group_fees`.
//! - D-TAX-GB-SA108: SA108 boxes 13.1–13.5 per tax year (box numbers from 2024/25), the annual exempt amount, both rates
//!   per rate period (10/20% to 29 October 2024, 18/24% from 30 October 2024, FA 2025 s. 7).
//! - D-TAX-GB-TAX-YEAR: 6 April to 5 April (s. 288(1ZA)), on the taxpayer's calendar.
use super::{cents, fifo::Disposal, line, s, Cell};
use crate::figures::{dec::Dec, FiguresError};
use crate::web::i18n::{text, Arg};
use chrono::{Datelike, NaiveDate};

/// D-TAX-GB-YEARS: the tax years served, by the year they start in (2020 = 6 April 2020 to 5 April 2021).
pub const FIRST_YEAR: i32 = 2020;
pub const LAST_YEAR: i32 = 2026;

/// D-TAX-GB-TAX-YEAR: the tax year a local day is in, by the year it starts in.
pub fn tax_year(day: NaiveDate) -> i32 { if (day.month(), day.day()) >= (4, 6) { day.year() } else { day.year() - 1 } }

/// The annual exempt amount (s. 1K; SI 2020/333, FA 2021 s. 40, FA 2023 s. 8).
fn aea(year: i32) -> i64 { match year { ..=2022 => 12_300, 2023 => 6_000, _ => 3_000 } }

fn d(v: &str) -> Dec { Dec::strict(v).unwrap_or_else(|_| Dec::zero()) }
fn max0(v: Dec) -> Dec { if v.is_negative() { Dec::zero() } else { v } }
fn min(a: &Dec, b: &Dec) -> Dec { if a < b { a.clone() } else { b.clone() } }

/// The summary after the rows and the NOTE blocks at the file's end.
pub(super) struct Year { pub summary: String, pub notes: String }

/// D-TAX-GB-SA108 from the tax year's rows: a disposal is a coin's disposals of one local day (s. 105(1)(a)), its result
/// the sum of its rows; the year's losses in full, then the annual exempt amount (s. 1(3), 1K(4)), off the 18/24% gains
/// first in 2024/25. NOTE `gb_provisional` while a later purchase can still be matched (`today` up to the last
/// disposal's local day + 32: the 30 days, and the two days the look-ahead reads past them for Alpaca's fees).
pub(super) fn year(locale: &str, year: i32, results: &[Disposal], local: &dyn Fn(&Disposal) -> NaiveDate, today: NaiveDate) -> Result<Year, FiguresError> {
    let t = |k: &str| text(locale, &format!("tax_report.gb.{k}"), &[]);
    let rate_change = super::jurisdictions::ymd((2024, 10, 30));
    // (coin, day) → (result, from 30 October 2024).
    let mut days: Vec<((String, NaiveDate), Dec)> = vec![];
    let (mut proceeds, mut costs) = (Dec::zero(), Dec::zero());
    for r in results {
        proceeds = (&proceeds + &r.proceeds)?;
        costs = (&(&costs + &r.cost_basis)? + &r.fee)?;
        let k = (r.asset_id.clone(), local(r));
        match days.iter_mut().find(|(x, _)| *x == k) { Some((_, v)) => *v = (&*v + &r.gain_loss)?, None => days.push((k, r.gain_loss.clone())) }
    }
    let (mut before, mut from, mut losses) = (Dec::zero(), Dec::zero(), Dec::zero());
    for ((_, day), v) in &days {
        if v.is_negative() { losses = (&losses - v)?; } else if *day >= rate_change { from = (&from + v)?; } else { before = (&before + v)?; }
    }
    let gains = (&before + &from)?;
    let net = (&gains - &losses)?;
    let exempt = Dec::from_i64(aea(year));
    // The year's losses, then the exempt amount, off the from-30-October gains first.
    let off_from = min(&from, &losses);
    let (from_net, before_net) = ((&from - &off_from)?, max0((&before - &(&losses - &off_from)?)?));
    let aea_from = min(&from_net, &exempt);
    let (from_taxable, before_taxable) = ((&from_net - &aea_from)?, max0((&before_net - &(&exempt - &aea_from)?)?));
    let taxable = (&from_taxable + &before_taxable)?;
    // Rates by period: 10/20% to 29 October 2024, 18/24% from 30 October 2024 (FA 2025 s. 7(3)).
    let tax = |low: &str, high: &str| -> Result<Dec, FiguresError> { Ok((&(&before_taxable * &d(low))? + &(&from_taxable * &d(high))?)?.round(2)?) };
    let (tax_lower, tax_higher) = (tax("0.10", "0.18")?, tax("0.20", "0.24")?);
    let boxed = |k: &str, n: u32| if year >= 2024 { format!("{} (box 13.{n})", t(k)) } else { t(k) };
    let mut out = String::new();
    if !results.is_empty() {
        line(&mut out, &[]);
        let heading = text(locale, "tax_report.gb.heading", &[("from", Arg::Text(&year.to_string())), ("to", Arg::Text(&(year + 1).to_string()))]);
        line(&mut out, &[s(heading)]);
        line(&mut out, &[s(boxed("disposals", 1)), Cell::Raw(days.len().to_string())]);
        line(&mut out, &[s(boxed("proceeds", 2)), cents(&proceeds)?]);
        line(&mut out, &[s(boxed("costs", 3)), cents(&costs)?]);
        line(&mut out, &[s(boxed("gains", 4)), cents(&gains)?]);
        line(&mut out, &[s(boxed("losses", 5)), cents(&losses)?]);
        line(&mut out, &[s(t("net")), cents(&net)?]);
        if year == 2024 {
            line(&mut out, &[s(format!("  {}", t("gains_before"))), cents(&before_net)?]);
            line(&mut out, &[s(format!("  {}", t("gains_from"))), cents(&from_net)?]);
        }
        line(&mut out, &[s(t("aea")), cents(&exempt)?]);
        line(&mut out, &[s(t("taxable")), cents(&taxable)?]);
        if (&losses - &gains)?.is_positive() { line(&mut out, &[s(t("loss_carried")), cents(&(&losses - &gains)?)?]); }
        line(&mut out, &[s(t("tax_lower")), cents(&tax_lower)?]);
        line(&mut out, &[s(t("tax_higher")), cents(&tax_higher)?]);
        let threshold = if year <= 2022 { "49,200" } else { "50,000" };
        line(&mut out, &[s(text(locale, "tax_report.gb.hint", &[("threshold", Arg::Text(threshold))]))]);
        if year == 2024 { line(&mut out, &[s(t("hint_2024"))]); }
        if year < 2024 { line(&mut out, &[s(t("hint_before_2024"))]); }
    }

    // A later purchase can still be matched: up to the disposal's local day + 32; listed with the first day it cannot.
    let mut open: Vec<(String, NaiveDate)> = vec![];
    for r in results {
        let k = (r.asset.clone(), local(r));
        if !open.contains(&k) { open.push(k); }
    }
    let lines: Vec<Vec<Cell>> = open.into_iter().filter_map(|(asset, day)| {
        let closed = day + chrono::Duration::days(33);
        (today < closed).then(|| vec![s(format!("{asset} {}", day.format("%Y-%m-%d"))), s(closed.format("%Y-%m-%d").to_string())])
    }).collect();
    let mut notes = String::new();
    if !lines.is_empty() {
        let w = |k: &str| text(locale, &format!("tax_report.warnings.{k}"), &[]);
        line(&mut notes, &[]);
        line(&mut notes, &[s(format!("NOTE: {}:", w("gb_provisional")))]);
        for l in lines { line(&mut notes, &l); }
        line(&mut notes, &[s(w("gb_provisional_hint"))]);
    }
    Ok(Year { summary: out, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figures::at::At;

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }
    fn disposal(on: &str, gain: &str) -> Disposal {
        let at = At::from_utc(day(on).and_hms_opt(12, 0, 0).unwrap().and_utc()).unwrap();
        Disposal { at, acquired: None, asset: "BTC".into(), amount: d("1"), proceeds: d("1000"), cost_basis: (&d("1000") - &d(gain)).unwrap(),
                   fee: Dec::zero(), gain_loss: d(gain), holding_days: 0, complete: true, incomplete: false, tx_id: None, exchange: "kraken".into(),
                   long_held: None, uncovered: Dec::zero(), averaged: false, mined: false, source: 0, recent: false, restricted: Dec::zero(),
                   released: Dec::zero(), unusable: Dec::zero(), effective: at, rule: Some("section104"), asset_id: "bitcoin".into() }
    }

    /// D-TAX-GB-TAX-YEAR: 5 April is the year before's, 6 April the year's own.
    #[test]
    fn the_tax_year_starts_on_6_april() {
        assert_eq!((tax_year(day("2025-04-05")), tax_year(day("2025-04-06")), tax_year(day("2026-01-01"))), (2024, 2025, 2025));
    }

    /// r5: `gb_provisional` while today is up to the disposal's local day + 32: a disposal on 2026-04-01 → shown on
    /// 2026-04-20 and 2026-05-03, not on 2026-05-04.
    #[test]
    fn the_provisional_note_follows_the_look_ahead() {
        let rows = [disposal("2026-04-01", "100")];
        let local = |r: &Disposal| r.at.utc().date_naive();
        for (today, shown) in [("2026-04-20", true), ("2026-05-01", true), ("2026-05-03", true), ("2026-05-04", false)] {
            let y = year("en", 2025, &rows, &local, day(today)).unwrap();
            assert_eq!(y.notes.contains("2026-05-04"), shown, "{today}: {}", y.notes);
        }
    }

    /// gb_rate_change_2024: +7,000 (29 October), +4,000 (30 October), −2,000 → before 7,000 / from 2,000 net; the AEA off
    /// the from gains first → taxable 6,000, tax 600.00 / 1,200.00.
    #[test]
    fn the_2024_rates_by_period() {
        let mut eth = disposal("2024-11-15", "-2000");
        eth.asset_id = "ethereum".into();
        let rows = [disposal("2024-10-29", "7000"), disposal("2024-10-30", "4000"), eth];
        let y = year("en", 2024, &rows, &|r: &Disposal| r.at.utc().date_naive(), day("2026-10-01")).unwrap();
        for want in ["(box 13.1),3\n", ",7000.0\n", ",2000.0\n", "Taxable gains (before losses brought forward),6000.0\n", "basic rate band,600.0\n", "higher rate,1200.0\n"] {
            assert!(y.summary.contains(want), "{want}: {}", y.summary);
        }
    }
}
