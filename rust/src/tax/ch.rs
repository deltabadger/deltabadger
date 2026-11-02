//! Switzerland (DBG art. 16, 20; StHG art. 13–17; ESTV Arbeitspapier Kryptowährungen; KS Nr. 36): the wealth snapshot at
//! 31 December at the ESTV Kursliste values (A12-6), over `wealth::held` and `estv`:
//! - D-TAX-CH-YEAR-END: units held before 1 January `year`+1 00:00 in the taxpayer's zone (Rails: 23:59:59 UTC on 31
//!   December); a year not ended: units to the start of tomorrow, prices dated today, provisional (`ch_provisional`).
//! - D-TAX-CH-ESTV: a coin ESTV lists is valued at its Kursliste value of 31 December; one it does not list, or a year it
//!   was not reached for (provisional), at the daily price, listed (`ch_market_value`). Only where the ticker names the
//!   holding's coin, and only it (r3 item 2, r4 item 1).
//! - D-TAX-CH-NO-VALUE: a coin with neither is listed at 0 (`ch_no_value`: declare it at its purchase price).
//! - D-TAX-CH-INCOME: the year's income valued in CHF on receipt (Rails: "income not included").
//! - D-TAX-CH-KS36: the year's sales and the fiat trading volume against five times the holdings on 1 January
//!   (`ch_private_gains`).
use super::wealth::{fixed, summary, Held, Wealth};
use super::{cents, line, local_midnight, local_start, n, s, Cell, Report, INCOME};
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::tracker::prices::PriceService;
use crate::tracker::rows::Kind;
use crate::tracker::{fiat, walk};
use crate::web::i18n::{text, Arg};
use chrono::{Datelike, NaiveDate};
use rusqlite::Connection;

/// D-TAX-CH-YEARS: the first and last years whose law was reviewed.
pub const FIRST_YEAR: i32 = 2020;
pub const LAST_YEAR: i32 = 2026;

/// How a holding was valued.
#[derive(PartialEq)]
enum Valued { Estv, Market, Unresolved, Nothing }

fn ymd(y: i32, m: u32, d: u32) -> Result<NaiveDate, FiguresError> { NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| FiguresError::Data("a date out of range".into())) }
fn instant(a: Option<At>) -> Result<At, FiguresError> { a.ok_or_else(|| FiguresError::Data("a date out of range".into())) }

impl Report {
    /// The ESTV values the report reads: those of the year when it has ended, and of the year before (KS 36's 1 January), for
    /// the coins held then that have no row yet.
    pub fn estv_needs(&self, today: NaiveDate) -> Result<Vec<(i32, Vec<String>)>, FiguresError> {
        let Some(estv) = &self.estv else { return Ok(vec![]) };
        let y = self.year;
        let mut out = vec![];
        let mut years = vec![(y - 1, instant(local_midnight(&self.zone, y))?)];
        if today.year() > y { years.push((y, instant(local_midnight(&self.zone, y + 1))?)); }
        for (year, until) in years {
            let mut symbols: Vec<String> = self.held_at(until)?.into_iter().map(|h| h.symbol).filter(|sym| estv.value(year, sym).is_none()).collect();
            symbols.sort();
            symbols.dedup();
            if !symbols.is_empty() { out.push((year, symbols)); }
        }
        Ok(out)
    }

    /// The ESTV values read again (the job loads them after `prepare`, from `estv_needs`).
    pub fn reload_estv(&mut self, c: &Connection) -> Result<(), FiguresError> {
        if self.estv.is_some() { self.estv = Some(super::estv::Estv::load(c)?); }
        Ok(())
    }

    /// A holding's value in CHF on `day`: at the ESTV value of `estv_year` where one applies, else at the market.
    fn ch_value(&self, ps: &mut PriceService, h: &Held, all: &[Held], estv_year: Option<i32>, day: NaiveDate) -> Result<(Dec, Valued), FiguresError> {
        let mut state = Valued::Market;
        if let Some(year) = estv_year.filter(|_| self.priceable(h, all, day)) {
            match self.estv.as_ref().and_then(|e| e.value(year, &h.symbol)) {
                Some(Some(v)) => return Ok(((&h.units * v)?.round(2)?, Valued::Estv)),
                Some(None) => {}
                None => state = Valued::Unresolved,
            }
        }
        Ok(match self.unit_price(ps, h, all, "CHF", day)? {
            Some(p) => ((&h.units * &p)?.round(2)?, state),
            // Unresolved and unpriced: no value, and the report stays provisional (ESTV may list it).
            None => (Dec::zero(), if state == Valued::Unresolved { Valued::Unresolved } else { Valued::Nothing }),
        })
    }

    pub(super) fn ch(&self, ps: &mut PriceService, today: NaiveDate) -> Result<Wealth, FiguresError> {
        let (y, l) = (self.year, self.j.locale);
        let t = |k: &str| text(l, k, &[]);
        let ended = today.year() > y;
        let (until, day) = if ended { (instant(local_midnight(&self.zone, y + 1))?, ymd(y, 12, 31)?) } else {
            let tomorrow = today.succ_opt().ok_or_else(|| FiguresError::Data("a date out of range".into()))?;
            (instant(local_start(&self.zone, (tomorrow.year(), tomorrow.month(), tomorrow.day())))?, today)
        };
        let held = self.held_at(until)?;
        let (mut lines, mut total) = (vec![], Dec::zero());
        let (mut market, mut nothing, mut provisional) = (vec![], vec![], !ended);
        for h in &held {
            let (value, how) = self.ch_value(ps, h, &held, ended.then_some(y), day)?;
            let label = format!("{} {}", h.symbol, h.units.to_s_f());
            match how {
                Valued::Estv => {}
                Valued::Market if ended => market.push(label),
                Valued::Market => {}
                Valued::Unresolved => { provisional = true; if value.is_zero() { nothing.push(label) } else { market.push(label) } }
                Valued::Nothing => nothing.push(label),
            }
            total = (&total + &value)?;
            lines.push((h.clone(), value));
        }
        let mut out = String::new();
        summary(&mut out, t("tax_report.summary.total_value"), Some(cents(&total)?));
        line(&mut out, &[s(t("tax_report.ch.hint"))]);

        // D-TAX-CH-INCOME: the year's income in CHF on receipt.
        let received: Vec<_> = self.scoped.iter().filter(|r| self.in_year(r.at) && INCOME.iter().any(|(k, _)| *k == r.kind)).cloned().collect();
        let rows = walk::enrich_with(&received, &self.deposits, ps, true, "CHF", &mut |_| {})?;
        let mut income = String::new();
        if !rows.is_empty() {
            line(&mut income, &[]);
            line(&mut income, &[s(t("tax_report.income.header"))]);
            let label = |kind: Kind| INCOME.iter().find(|(k, _)| *k == kind).map(|(_, name)| *name).unwrap_or("other_income");
            let mut totals: Vec<(&str, Dec)> = vec![];
            for r in &rows {
                let value = if fiat(&r.base) { ps.convert_fiat(&r.amount, &r.base, "CHF", r.at)? } else { r.fiat_value.clone() };
                let name = label(r.kind);
                line(&mut income, &[s(self.local(r.at).format("%Y-%m-%d").to_string()), s(t(&format!("tax_report.income.types.{name}"))), s(r.base.clone()), n(&r.amount), cents(&value)?, s("CHF")]);
                match totals.iter_mut().find(|(k, _)| *k == name) { Some((_, v)) => *v = (&*v + &value)?, None => totals.push((name, value)) }
            }
            for (name, value) in totals {
                let kind = t(&format!("tax_report.income.types.{name}"));
                line(&mut income, &[Cell::Nil, s(text(l, "tax_report.income.total", &[("type", Arg::Text(&kind))])), Cell::Nil, Cell::Nil, cents(&value)?, s("CHF")]);
            }
        }

        // D-TAX-CH-KS36: the year's sales, and the volume against five times the holdings on 1 January (their value at the
        // year before's end).
        let start = self.held_at(instant(local_midnight(&self.zone, y))?)?;
        let mut opening = Dec::zero();
        for h in &start { opening = (&opening + &self.ch_value(ps, h, &start, Some(y - 1), ymd(y - 1, 12, 31)?)?.0)?; }
        let (paid, got) = self.fiat_trades(ps, "CHF")?;
        let volume = (&paid + &got)?;
        let sales: Vec<String> = self.scoped.iter().filter(|r| self.in_year(r.at) && r.kind == Kind::Sell && !fiat(&r.base))
            .map(|r| format!("{} {} {}", r.base, r.amount.to_s_f(), r.at.utc().format("%Y-%m-%d"))).collect();
        let mut notes = String::new();
        self.block(&mut notes, "NOTE", "ch_market_value", "ch_market_value_hint", market.into_iter())?;
        self.block(&mut notes, "NOTE", "ch_no_value", "ch_no_value_hint", nothing.into_iter())?;
        if !sales.is_empty() || volume.is_positive() {
            let (v, limit) = (fixed(&volume, 2)?, fixed(&(&opening * &Dec::from_i64(5))?, 2)?);
            let mut listed = sales;
            listed.push(text(l, "tax_report.warnings.ch_private_gains_volume", &[("volume", Arg::Text(&v)), ("limit", Arg::Text(&limit))]));
            self.block(&mut notes, "NOTE", "ch_private_gains", "ch_private_gains_hint", listed.into_iter())?;
        }
        if provisional {
            line(&mut notes, &[]);
            line(&mut notes, &[s(format!("NOTE: {}:", t("tax_report.warnings.ch_provisional")))]);
            line(&mut notes, &[s(t("tax_report.warnings.ch_provisional_hint"))]);
        }
        Ok(Wealth { day, lines, summary: out, income, notes, until })
    }
}
