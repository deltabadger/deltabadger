//! Italy (TUIR art. 67 c. 1 lett. c-sexies, art. 68 c. 9-bis; L. 197/2022, L. 207/2024, L. 199/2025; Circolare 30/E 2023):
//! the quadro RT summary and the euro e-money token rules (A12-4). The engine's part is `fifo::Options::{lifo,
//! fees_ignored_from, swaps_taxable_before, deposit_cost_zero}`.
//! - D-TAX-IT-SWAPS: an exchange of crypto-assets is no realisation, but one with an e-money token leg (USDC, EURC) is a
//!   disposal at the value received (`emt_side`, `emt_groups_as_trades`); other stablecoins are crypto (NOTE `it_stablecoins`).
//! - D-TAX-IT-THRESHOLD: no `exempt` column (the €2,000 is on the year's net, `year`); the `tax_rate` column (`rate`).
//! - D-TAX-IT-EMT-RATE: from 2026 rows disposing of EURC and EURC staking are taxed at 26%; EURC sold for euros is no
//!   realisation (L. 199/2025 c. 28): out of the rows, listed (NOTE `it_eurc_conversion`).
//! - D-TAX-IT-RT: the quadro RT figures of the year (`year`).
//! - D-TAX-IT-INCOME: staking in the summary at the row's gross value; other receipts listed apart (NOTE `it_income`).
use super::{cents, date, fifo::{Disposal, Tx}, line, Cell, Entry, Report};
use crate::figures::{dec::Dec, FiguresError};
use crate::tracker::rows::{Kind, Stored};
use crate::tracker::prices::PriceService;
use crate::tracker::{fiat, stable, walk};
use crate::web::i18n::text;
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;

/// D-TAX-IT-YEARS: the crypto-asset regime starts on 1 January 2023 (L. 197/2022 c. 126).
pub const FIRST_YEAR: i32 = 2023;
/// D-TAX-IT-YEARS: the TUIR is repealed from 2027 (D.Lgs. 117/2026), not yet reviewed.
pub const LAST_YEAR: i32 = 2026;
/// L. 207/2024 c. 25: the €2,000 applies to disposals up to 31 December 2024.
const THRESHOLD_UNTIL: i32 = 2024;
const THRESHOLD: i64 = 2000;
/// L. 207/2024 c. 24 (33%) and L. 199/2025 c. 28 (26% for euro e-money tokens), from 2026.
const RATE_33_FROM: i32 = 2026;
/// The euro e-money token this report knows (pegged to the euro, reserves in euro at EU-authorised entities: assumed).
const EURO_EMT: &str = "EURC";

/// The MiCA e-money tokens this report knows: exchanging one is a disposal (this report's reading, flagged).
pub(super) fn emt(symbol: &str) -> bool { matches!(symbol, "USDC" | EURO_EMT) }

/// `value_groups`' side: the coins received, for a group with an e-money token leg; any other group is no disposal.
pub(super) fn emt_side(ins: &[&walk::Row], outs: &[&walk::Row]) -> Option<bool> { ins.iter().chain(outs).any(|r| emt(&r.base)).then_some(true) }

/// The exchanges with an e-money token leg: groups of coin legs only (no fiat), legs both ways, as (ins, outs).
fn emt_groups(rows: &[Entry]) -> Vec<(Vec<usize>, Vec<usize>)> {
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, e) in rows.iter().enumerate() { if let Some(g) = &e.row.group { groups.entry((e.row.exchange.clone(), g.clone())).or_default().push(i); } }
    // The trade's legs only: a fee row booked in the group (a USDC fee) makes no exchange with an e-money token.
    let legs = |all: Vec<usize>| -> Vec<usize> { all.into_iter().filter(|&i| rows[i].row.kind.in_leg() || rows[i].row.kind.out_leg()).collect() };
    let mut out: Vec<(Vec<usize>, Vec<usize>)> = groups.into_values().map(legs)
        .filter(|legs| !legs.iter().any(|&i| fiat(&rows[i].row.base)) && legs.iter().any(|&i| emt(&rows[i].row.base)))
        .map(|legs| (legs.iter().copied().filter(|&i| rows[i].row.kind.in_leg()).collect(), legs.iter().copied().filter(|&i| rows[i].row.kind.out_leg()).collect()))
        .filter(|(ins, outs): &(Vec<usize>, Vec<usize>)| !ins.is_empty() && !outs.is_empty()).collect();
    out.sort();
    out
}

/// D-TAX-IT-SWAPS: in an exchange with an e-money token every coin received is valued at its own price (the enrichment,
/// Rails' reading, values a coin received beside a stablecoin given at the stablecoin, and leaves a coin received beside
/// a stablecoin received unvalued), so `value_groups` can give the coins given the value received.
pub(super) fn value_received(rows: &mut [Entry], stored: &[Stored], ps: &mut PriceService, currency: &str) -> Result<(), FiguresError> {
    for (ins, _) in emt_groups(rows) {
        for i in ins {
            let (r, t) = (&mut rows[i].row, &stored[i]);
            r.fiat_value = (&ps.price_at_recorded(&r.base, currency, r.at, Some(&t.venue), t.base_asset.as_ref())? * &r.amount)?;
            let missing = r.fiat_value.is_zero();
            (r.price_missing, r.value_missing) = (missing, Some(missing));
        }
    }
    Ok(())
}

/// D-TAX-IT-SWAPS: the swap legs of an exchange with an e-money token walk as a sale and a purchase.
pub(super) fn emt_groups_as_trades(rows: &mut [Entry]) {
    for (ins, outs) in emt_groups(rows) {
        for i in ins.into_iter().chain(outs) {
            let r = &mut rows[i].row;
            r.kind = match r.kind { Kind::SwapIn => Kind::Buy, Kind::SwapOut => Kind::Sell, k => k };
        }
    }
}

/// D-TAX-IT-EMT-RATE: whether a disposal (or staking) of `asset` on `day` is in the 26% bucket of 2026 on.
fn euro_emt(day: NaiveDate, asset: &str) -> bool { day.year() >= RATE_33_FROM && asset == EURO_EMT }

/// The `tax_rate` column: Rails' rate by the local date, and 26% for EURC from 2026.
pub(super) fn rate((before, after, (y, m, d)): (&'static str, &'static str, (i32, u32, u32)), day: NaiveDate, asset: &str) -> &'static str {
    if day < super::jurisdictions::ymd((y, m, d)) || euro_emt(day, asset) { before } else { after }
}

/// What the Italian report states beside the rows.
#[derive(Default)]
pub(super) struct Notes { conversions: Vec<(String, Dec)>, stablecoins: Vec<String>, other_income: Vec<(String, Dec)> }

fn pos(v: Dec) -> Dec { if v.is_positive() { v } else { Dec::zero() } }
fn pct(v: &Dec, rate: &str) -> Result<Dec, FiguresError> { Ok((v * &Dec::strict(rate)?)?.round(2)?) }

impl Report {
    /// D-TAX-IT-RT, D-TAX-IT-EMT-RATE, D-TAX-IT-INCOME over the year's rows (each to the cent first, fees counted
    /// nowhere): the summary lines and the notes. EURC sold for euros from 2026 leaves `results`.
    pub(super) fn it_year(&self, results: &mut Vec<Disposal>, txs: &[Tx], enriched: &[Entry], income: &[(usize, &str, Dec)],
                          held: Option<&Vec<(String, Dec)>>) -> Result<(String, Notes), FiguresError> {
        let (l, year) = (self.j.locale, self.year);
        let mut notes = Notes::default();
        let in_group = |t: &Tx, base: &str| t.row.group.as_ref().is_some_and(|g| enriched.iter().any(|e| e.row.base == base && e.row.exchange == t.row.exchange && e.row.group.as_ref() == Some(g)));
        let for_euros = |d: &Disposal| txs.iter().find(|t| t.row.uid == d.source).is_some_and(|t| t.row.quote.as_deref() == Some("EUR") || in_group(t, "EUR"));
        let mut kept = vec![];
        for mut d in results.drain(..) {
            (d.proceeds, d.cost_basis, d.fee) = (d.proceeds.round(2)?, d.cost_basis.round(2)?, d.fee.round(2)?);
            d.gain_loss = (&d.proceeds - &d.cost_basis)?;
            if euro_emt(self.local(d.at), &d.asset) && for_euros(&d) {
                notes.conversions.push((format!("{} {} {}", d.asset, d.amount.to_s_f(), date(d.at)), d.gain_loss.clone()));
            } else {
                kept.push(d);
            }
        }
        *results = kept;
        let (mut proceeds, mut costs, mut staking, mut emt_bucket) = (Dec::zero(), Dec::zero(), Dec::zero(), Dec::zero());
        for d in results.iter() {
            (proceeds, costs) = ((&proceeds + &d.proceeds)?, (&costs + &d.cost_basis)?);
            if euro_emt(self.local(d.at), &d.asset) { emt_bucket = (&emt_bucket + &d.gain_loss)?; }
        }
        let mut staked = false;
        for (i, name, value) in income {
            let r = &enriched[*i].row;
            if *name != "staking_reward" {
                let label = text(l, &format!("tax_report.income.types.{name}"), &[]);
                notes.other_income.push((format!("{label} {} {} {}", r.base, r.amount.to_s_f(), date(r.at)), value.clone()));
                continue;
            }
            staked = true;
            let v = value.round(2)?;
            staking = (&staking + &v)?;
            if euro_emt(self.local(r.at), &r.base) { emt_bucket = (&emt_bucket + &v)?; }
        }
        let mut coins: Vec<String> = held.into_iter().flatten().filter(|(sym, units)| (stable(sym) || emt(sym)) && units.is_positive()).map(|(sym, _)| sym.clone())
            .chain(enriched.iter().map(|e| &e.row).filter(|r| (stable(&r.base) || emt(&r.base)) && self.local(r.at).year() == year).map(|r| r.base.clone())).collect();
        coins.sort();
        coins.dedup();
        notes.stablecoins = coins;

        let mut out = String::new();
        if results.is_empty() && !staked { return Ok((out, notes)); }
        let result = (&(&proceeds - &costs)? + &staking)?;
        let threshold = (year <= THRESHOLD_UNTIL).then(|| Dec::from_i64(THRESHOLD));
        let zero = Dec::zero();
        let taxable = pos((&result - threshold.as_ref().unwrap_or(&zero))?);
        let loss_carried = pos((&(&zero - &result)? - threshold.as_ref().unwrap_or(&zero))?);
        let t = |key: &str| Cell::Text(text(l, &format!("tax_report.it.{key}"), &[]));
        line(&mut out, &[]);
        line(&mut out, &[t(if year == FIRST_YEAR { "heading_2023" } else { "heading" })]);
        for (key, v) in [("proceeds", &proceeds), ("costs", &costs), ("staking", &staking), ("result", &result)] { line(&mut out, &[t(key), cents(v)?]); }
        if let Some(v) = &threshold { line(&mut out, &[t("threshold"), cents(v)?]); }
        line(&mut out, &[t("taxable"), cents(&taxable)?]);
        line(&mut out, &[t("loss_carried"), cents(&loss_carried)?]);
        if year < RATE_33_FROM {
            line(&mut out, &[t("tax_26"), cents(&pct(&taxable, "0.26")?)?]);
        } else {
            // Each bucket's net, a negative one offsetting the other.
            let other = (&result - &emt_bucket)?;
            let net = |a: &Dec, b: &Dec| -> Result<Dec, FiguresError> { Ok(pos((a + &if b.is_negative() { b.clone() } else { Dec::zero() })?)) };
            line(&mut out, &[t("tax_33"), cents(&pct(&net(&other, &emt_bucket)?, "0.33")?)?]);
            if !emt_bucket.is_zero() { line(&mut out, &[t("tax_26_emt"), cents(&pct(&net(&emt_bucket, &other)?, "0.26")?)?]); }
        }
        // The 2023 form's own wording: a net of 2,000 or more is taxed whole.
        if year == FIRST_YEAR { line(&mut out, &[t("cliff_2023"), cents(&if result >= Dec::from_i64(THRESHOLD) { pct(&result, "0.26")? } else { Dec::zero() })?]); }
        line(&mut out, &[t(if year == FIRST_YEAR { "hint_2023" } else { "hint" })]);
        if year == LAST_YEAR { line(&mut out, &[t("hint_2026")]); }
        Ok((out, notes))
    }

    /// Italy's NOTE blocks: D-TAX-IT-EMT-RATE, D-TAX-IT-SWAPS, D-TAX-IT-INCOME.
    pub(super) fn it_notes(&self, out: &mut String, n: &Notes, currency: &str) -> Result<(), FiguresError> {
        self.valued(out, "NOTE", "it_eurc_conversion", &n.conversions, Some(currency))?;
        self.block(out, "NOTE", "it_stablecoins", "it_stablecoins_hint", n.stablecoins.iter().cloned())?;
        self.valued(out, "NOTE", "it_income", &n.other_income, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn the_rate_column_is_26_to_2025_and_33_after_but_for_eurc() {
        let r = ("26%", "33%", (2026, 1, 1));
        assert_eq!([rate(r, day("2025-12-31"), "BTC"), rate(r, day("2026-01-01"), "BTC"), rate(r, day("2026-01-01"), "EURC"), rate(r, day("2026-03-01"), "USDC")],
                   ["26%", "33%", "26%", "33%"]);
    }

    #[test]
    fn usdc_and_eurc_are_the_e_money_tokens() {
        assert!(emt("USDC") && emt("EURC") && !emt("USDT") && !emt("DAI") && !emt("BTC"));
    }
}
