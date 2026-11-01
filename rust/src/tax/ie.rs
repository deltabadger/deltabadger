//! Ireland (TCA 1997 ss. 28, 31, 546, 581, 601, 959AM): the CGT summary and its payment periods (A12-4). The engine's part
//! is `fifo::Options::ireland` (s. 581's matching and loss restriction).
//! - D-TAX-IE-RATE: 33% to 6 October 2026, 31% from 7 October 2026 (the taxpayer's date; TDM 02-03-01A); losses and the
//!   exemption off the 33% gains first (s. 546(6), s. 601(3)). Rails: 33% on everything.
//! - D-TAX-IE-PERIODS: the 15 December figure is the year's computation over the disposals to 30 November (s. 959AM), the
//!   31 January one the rest; a negative rest is due 0.00 and shown as overpaid. Rails split the tax by gross gains.
//! - D-TAX-IE-SUMMARY: Rails' lines where right, with the restricted and released losses, the 31% lines and the hint.
use super::{cents, date, fifo::Disposal, line, s, Cell};
use crate::figures::{dec::Dec, FiguresError};
use crate::web::i18n::text;
use chrono::NaiveDate;

/// D-TAX-IE-YEARS: the first full year at 33% (FA 2013 s. 43).
pub const FIRST_YEAR: i32 = 2013;
/// D-TAX-IE-YEARS: the last year whose law was reviewed.
pub const LAST_YEAR: i32 = 2026;
/// D-TAX-IE-RATE: the first day at 31% (Budget 2027, eBrief 142/26).
const LOW_RATE_FROM: (i32, u32, u32) = (2026, 10, 7);

/// One printed row as the year reads it: its result, the restricted loss it carries and the restricted losses it used
/// (each in cents), its rate bucket and its payment period.
struct Row { gain: Dec, restricted: Dec, released: Dec, low: bool, initial: bool }

fn d(v: &str) -> Dec { Dec::strict(v).unwrap_or_else(|_| Dec::zero()) }
fn min(a: &Dec, b: &Dec) -> Dec { if a < b { a.clone() } else { b.clone() } }

/// s. 31 then s. 601 over `rows` (r2 item 2): each row's adjusted result a = gain − released; a > 0 is a gain in its
/// bucket, a < 0 an allowable loss of |a| − restricted; the losses then the exemption come off the 33% gains first.
/// Returns the taxable gains per bucket (33%, 31%).
fn taxable<'a>(rows: impl Iterator<Item = &'a Row>, exemption: &Dec) -> Result<(Dec, Dec), FiguresError> {
    let (mut high, mut low, mut off) = (Dec::zero(), Dec::zero(), exemption.clone());
    for r in rows {
        let a = (&r.gain - &r.released)?;
        if a.is_positive() { if r.low { low = (&low + &a)?; } else { high = (&high + &a)?; } }
        else if a.is_negative() { off = (&off + &(&a.neg() - &r.restricted)?)?; }
    }
    let first = min(&high, &off);
    let off = (&off - &first)?;
    let second = min(&low, &off);
    Ok(((&high - &first)?, (&low - &second)?))
}

/// The tax on the two buckets, each rounded half up to the cent.
fn tax((high, low): &(Dec, Dec)) -> Result<(Dec, Dec), FiguresError> {
    Ok(((high * &d("0.33"))?.round(2)?, (low * &d("0.31"))?.round(2)?))
}

/// The summary after the rows and the NOTE blocks at the end of the file.
pub(super) struct Year { pub summary: String, pub notes: String }

/// D-TAX-IE-SUMMARY, D-TAX-IE-RATE, D-TAX-IE-PERIODS from the year's rows; the NOTE blocks of D-TAX-IE-RING-FENCE and
/// `ie_provisional` (a loss whose 4 weeks after are not over on `today`, with the first day after them).
pub(super) fn year(locale: &str, exemption: i64, results: &[Disposal], local: &dyn Fn(&Disposal) -> NaiveDate, today: NaiveDate) -> Result<Year, FiguresError> {
    let t = |k: &str| text(locale, k, &[]);
    let low_from = super::jurisdictions::ymd(LOW_RATE_FROM);
    let mut rows = vec![];
    for r in results {
        let day = local(r);
        rows.push(Row { gain: r.gain_loss.round(2)?, restricted: r.restricted.round(2)?, released: r.released.round(2)?, low: day >= low_from,
                        initial: chrono::Datelike::month(&day) < 12 });
    }
    let exemption_d = Dec::from_i64(exemption);
    let year = taxable(rows.iter(), &exemption_d)?;
    let (tax_high, tax_low) = tax(&year)?;
    let total = (&tax_high + &tax_low)?;
    let (initial_high, initial_low) = tax(&taxable(rows.iter().filter(|r| r.initial), &exemption_d)?)?;
    let initial = (&initial_high + &initial_low)?;
    let later = (&total - &initial)?;
    let (mut gains, mut losses, mut restricted, mut released) = (Dec::zero(), Dec::zero(), Dec::zero(), Dec::zero());
    for r in &rows {
        if r.gain.is_positive() { gains = (&gains + &r.gain)?; } else { losses = (&losses - &r.gain)?; }
        restricted = (&restricted + &r.restricted)?;
        released = (&released + &r.released)?;
    }
    // The 31% lines wherever the year has a disposal at 31%.
    let low_rows = rows.iter().any(|r| r.low);
    let mut out = String::new();
    if !results.is_empty() {
        line(&mut out, &[]);
        line(&mut out, &[s(t("tax_report.summary.total_gains")), cents(&gains)?]);
        line(&mut out, &[s(t("tax_report.summary.total_losses")), cents(&(&losses - &restricted)?)?]);
        if !restricted.is_zero() { line(&mut out, &[s(t("tax_report.ie.restricted_losses")), cents(&restricted)?]); }
        if !released.is_zero() { line(&mut out, &[s(t("tax_report.ie.released_losses")), cents(&released)?]); }
        line(&mut out, &[s(t("tax_report.summary.annual_exemption")), Cell::Raw(exemption.to_string())]);
        line(&mut out, &[s(t("tax_report.summary.taxable_gains")), cents(&(&year.0 + &year.1)?)?]);
        line(&mut out, &[s(t("tax_report.summary.cgt_33")), cents(&tax_high)?]);
        if low_rows {
            line(&mut out, &[s(t("tax_report.ie.cgt_31")), cents(&tax_low)?]);
            line(&mut out, &[s(t("tax_report.ie.cgt_total")), cents(&total)?]);
        }
        line(&mut out, &[s(format!("  {}", t("tax_report.summary.due_dec_15"))), cents(&initial)?]);
        line(&mut out, &[s(format!("  {}", t("tax_report.summary.due_jan_31"))), cents(&if later.is_negative() { Dec::zero() } else { later.clone() })?]);
        if later.is_negative() { line(&mut out, &[s(format!("  {}", t("tax_report.ie.overpaid"))), cents(&later.neg())?]); }
        line(&mut out, &[s(t("tax_report.ie.hint"))]);
    }

    let listed = |r: &Disposal| format!("{} {} {}", r.asset, r.amount.to_s_f(), date(r.at));
    let mut notes = String::new();
    let mut block = |heading: &str, lines: Vec<Vec<Cell>>| {
        if lines.is_empty() { return; }
        line(&mut notes, &[]);
        line(&mut notes, &[s(format!("NOTE: {}:", t(&format!("tax_report.warnings.{heading}"))))]);
        for l in lines { line(&mut notes, &l); }
        line(&mut notes, &[s(t(&format!("tax_report.warnings.{heading}_hint")))]);
    };
    let valued = |f: &dyn Fn(&Disposal) -> &Dec| -> Result<Vec<Vec<Cell>>, FiguresError> {
        results.iter().filter(|r| f(r).round(2).is_ok_and(|v| v.is_positive())).map(|r| Ok(vec![s(listed(r)), cents(f(r))?])).collect()
    };
    block("ie_loss_restricted", valued(&|r| &r.restricted)?);
    block("ie_loss_released", valued(&|r| &r.released)?);
    block("ie_loss_unusable", valued(&|r| &r.unusable)?);
    // r1 item 4: a purchase before the day after the window (local date + 29; day 28 inside) would still restrict the loss.
    // The window runs from when the loss took effect (a CFEE walked after its fills: then), as the engine's fence does.
    let open: Vec<Vec<Cell>> = results.iter().filter(|r| r.gain_loss.round(2).is_ok_and(|v| v.is_negative())).filter_map(|r| {
        let closed = local(&Disposal { at: r.effective, ..r.clone() }) + chrono::Duration::days(29);
        (today < closed).then(|| vec![s(listed(r)), s(closed.format("%Y-%m-%d").to_string())])
    }).collect();
    block("ie_provisional", open);
    Ok(Year { summary: out, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(gain: &str, restricted: &str, released: &str, low: bool) -> Row { Row { gain: d(gain), restricted: d(restricted), released: d(released), low, initial: true } }

    /// r2 item 2: a release beyond the row's gain is a loss; losses and the exemption off the 33% gains first.
    #[test]
    fn losses_and_the_exemption_come_off_the_33_percent_gains_first() {
        let e = Dec::from_i64(1270);
        // ie_release_overshoot: +6,000 (33%); −2,000 restricted (33%); 0.00 released 2,000 (31%) → 2,730 at 33%.
        let (h, l) = taxable([row("6000", "0", "0", false), row("-2000", "2000", "0", false), row("0", "0", "2000", true)].iter(), &e).unwrap();
        assert_eq!((h.to_s_f(), l.to_s_f()), ("2730.0".into(), "0.0".into()));
        // ie_rate_31: +6,000 (33%), −2,000, +4,000 (31%) → 900.90 + 1,240.00.
        let y = taxable([row("6000", "0", "0", false), row("-2000", "0", "0", true), row("4000", "0", "0", true)].iter(), &e).unwrap();
        let (a, b) = tax(&y).unwrap();
        assert_eq!((a.to_s_f(), b.to_s_f()), ("900.9".into(), "1240.0".into()));
        // A net loss: nothing taxable, the exemption wasted.
        let (h, l) = taxable([row("1000", "0", "0", false), row("-1500", "0", "0", false)].iter(), &e).unwrap();
        assert!(h.is_zero() && l.is_zero());
        // Losses past the 33% gains reach the 31% ones.
        let (h, l) = taxable([row("1000", "0", "0", false), row("-3000", "0", "0", false), row("5000", "0", "0", true)].iter(), &e).unwrap();
        assert_eq!((h.to_s_f(), l.to_s_f()), ("0.0".into(), "1730.0".into()));
    }

    /// Confirmation round: the window of `ie_provisional` runs from when the loss took effect (a CFEE walked after its
    /// fills), as the engine's fence does, not from the row's own time.
    #[test]
    fn the_open_window_counts_from_when_the_loss_took_effect() {
        use crate::figures::at::At;
        let at = |s: &str| At::from_utc(chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc)).unwrap();
        let loss = Disposal { at: at("2026-06-02T00:00:00Z"), acquired: None, asset: "ETH".into(), amount: d("0.1"), proceeds: d("200"), cost_basis: d("400"),
                           fee: Dec::zero(), gain_loss: d("-200"), holding_days: 0, complete: true, incomplete: false, tx_id: None, exchange: "alpaca".into(),
                           long_held: None, uncovered: Dec::zero(), averaged: false, mined: false, source: 0, recent: false, restricted: Dec::zero(),
                           released: Dec::zero(), unusable: Dec::zero(), effective: at("2026-06-03T00:30:00Z") };
        let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let y = year("en", 1270, &[loss], &|r: &Disposal| r.at.utc().date_naive(), today).unwrap();
        assert!(y.notes.contains("2026-07-02"), "{}", y.notes);
    }

}
