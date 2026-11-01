//! The Czech Republic by its own law (ZDP § 4 odst. 1 písm. zj), zk), § 4 odst. 3, § 10 odst. 4–5, § 38v; zák. 32/2025 and
//! 360/2025; KV 625 with GFŘ; GFŘ 2022), on the FIFO rows with the time test of `holding_rule`:
//! - D-TAX-CZ-EMT: USDC and EURC (MiCA e-money tokens) are outside the value test and never exempt by it (zj).
//! - D-TAX-CZ-CAP: when the year's time-test proceeds S exceed 40,000,000 CZK, (S − 40M)/S of each such row's proceeds and
//!   expenses is taxable (§ 4 odst. 3, § 10 odst. 4); 2025 counts crypto only here and says so (securities are not here).
//! - D-TAX-CZ-PRILOHA2: příloha č. 2, druh C (ř. 207–209) from the taxable rows only, expenses capped at the income;
//!   § 38v: each exempt receipt above 5,000,000 CZK is listed. Rails' summary mixed exempt rows into its totals.
use super::{cents, fifo::Disposal, line, s, Cell};
use crate::figures::{dec::Dec, FiguresError};
use crate::web::i18n::text;
use chrono::NaiveDate;

/// § 4 odst. 1 písm. zj): the year's gross proceeds at most this are exempt.
const VALUE_TEST: i64 = 100_000;
/// § 4 odst. 3: the time-test proceeds above this are taxable in proportion.
const CAP: i64 = 40_000_000;
/// § 38v: an exempt receipt above this is notified.
const NOTICE: i64 = 5_000_000;
/// D-TAX-CZ-EMT: the e-money tokens the report recognises (MiCA EMTs); other tokens' status is the user's to check.
const EMT: [&str; 2] = ["USDC", "EURC"];
pub(super) const TIME_TEST: &str = "časový test";
const VALUE_TEST_REASON: &str = "hodnotový test";

/// The year's rows classified, and what the file states beside them.
#[derive(Default)]
pub(super) struct Year {
    /// Per row: (tax_exempt, exempt_reason).
    pub exempt: Vec<(bool, Option<String>)>,
    /// § 38v: (receipt, its exempt part).
    pub notice: Vec<(String, Dec)>,
    pub summary: String,
}

fn int(v: i64) -> Dec { Dec::from_i64(v) }

/// The rows of the year (each already in cents), `local` their day in the taxpayer's zone, `in_force` 15 February 2025.
pub(super) fn year(locale: &str, year: i32, results: &[Disposal], local: &dyn Fn(&Disposal) -> NaiveDate, in_force: NaiveDate) -> Result<Year, FiguresError> {
    let mut y = Year::default();
    if results.is_empty() { return Ok(y); }
    let counted = |d: &Disposal| local(d) >= in_force && !EMT.contains(&d.asset.as_str());
    let mut window = Dec::zero();
    for d in results.iter().filter(|d| counted(d)) { window = (&window + &d.proceeds)?; }
    // zj: a cliff, from 15 February 2025 (KV 625 2.2.1.5); zk-exempt proceeds count too (the literal reading, as Rails).
    let passed = window <= int(VALUE_TEST);
    let reasons: Vec<Option<&str>> = results.iter().map(|d| {
        if passed && counted(d) { Some(VALUE_TEST_REASON) } else if d.long_held == Some(true) { Some(TIME_TEST) } else { None }
    }).collect();
    let time = |i: usize| reasons[i] == Some(TIME_TEST);
    let mut time_exempt = Dec::zero();
    for (i, d) in results.iter().enumerate() { if time(i) { time_exempt = (&time_exempt + &d.proceeds)?; } }
    // D-TAX-CZ-CAP: (S − 40,000,000) / S of each time-test row.
    let share = if time_exempt > int(CAP) { (&time_exempt - &int(CAP))?.div(&time_exempt)? } else { Dec::zero() };
    let capped = share.is_positive();
    let (mut income, mut expenses) = (Dec::zero(), Dec::zero());
    for (i, d) in results.iter().enumerate() {
        let part = if time(i) { share.clone() } else if reasons[i].is_none() { Dec::one() } else { continue };
        income = (&income + &(&d.proceeds * &part)?)?;
        expenses = (&expenses + &(&(&d.cost_basis + &d.fee)? * &part)?)?;
    }
    let r207 = income.round(2)?;
    let r208 = { let e = expenses.round(2)?; if e > r207 { r207.clone() } else { e } };
    let r209 = (&r207 - &r208)?;
    let capped_reason = text(locale, "tax_report.cz.exempt_reason_capped", &[]);
    y.exempt = reasons.iter().enumerate().map(|(i, r)| match r {
        Some(_) if time(i) && capped => (true, Some(capped_reason.clone())),
        Some(r) => (true, Some((*r).to_string())),
        None => (false, None),
    }).collect();

    // § 38v: per receipt (a disposal's rows summed), its exempt part after the cap.
    // (the disposed row, the asset), "asset day", amount, exempt part.
    type Receipt = ((usize, String), String, Dec, Dec);
    let mut receipts: Vec<Receipt> = vec![];
    let day = |d: &Disposal| d.at.utc().format("%Y-%m-%d").to_string();
    for (i, d) in results.iter().enumerate() {
        let Some(_) = reasons[i] else { continue };
        let exempt = if time(i) { (&d.proceeds * &(&Dec::one() - &share)?)? } else { d.proceeds.clone() };
        let key = (d.source, d.asset.clone());
        match receipts.iter_mut().find(|r| r.0 == key) {
            Some(r) => { r.2 = (&r.2 + &d.amount)?; r.3 = (&r.3 + &exempt)?; }
            None => receipts.push((key, format!("{} {}", d.asset, day(d)), d.amount.clone(), exempt)),
        }
    }
    for (_, label, amount, exempt) in receipts {
        let exempt = exempt.round(2)?;
        let (asset, day) = label.split_once(' ').unwrap_or((&label, ""));
        if exempt > int(NOTICE) { y.notice.push((format!("{asset} {} {day}", amount.to_s_f()), exempt)); }
    }

    let t = |key: &str| text(locale, &format!("tax_report.cz.{key}"), &[]);
    let out = &mut y.summary;
    line(out, &[]);
    line(out, &[s(t(if year == 2025 { "heading_2025" } else { "heading" }))]);
    if year >= 2025 {
        line(out, &[s(t("value_test")), cents(&window)?]);
        line(out, &[s(t("value_test_passed")), s(text(locale, if passed { "tax_report.summary.yes" } else { "tax_report.summary.no" }, &[]))]);
        line(out, &[s(t("time_exempt")), cents(&time_exempt)?]);
        if capped { line(out, &[s(t("cap_share")), s(format!("{}%", (&share * &int(100))?.round(2)?.to_s_f()))]); }
    }
    for (key, v) in [("r207", &r207), ("r208", &r208), ("r209", &r209)] { line(out, &[s(t(key)), cents(v)?]); }
    line(out, &[Cell::Text(t("hint"))]);
    if year == 2025 { line(out, &[Cell::Text(t("cap_2025_hint"))]); }
    Ok(y)
}
