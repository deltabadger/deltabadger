//! The German broker report (Anlage KAP / KAP-INV, `report_scope=broker`): Tax::BrokerReport, Tax::BrokerReportCsv,
//! Tax::LotLedger, Tax::Vorabpauschale and FundClassification.resolve over the Alpaca ledger, by the law digested in
//! specs/law-DE-broker.md (EStG §§ 20, 32d; InvStG §§ 2, 16, 18–20; KStG § 27 Abs. 8; DBA USA Art. 10, 11). Rails' bytes
//! where Rails is right; where it is not, a named divergence:
//! - D-TAX-BROKER-DISCLOSURES: the legal notes drop Rails' three that no longer hold (start value, interest net, the full
//!   credit) and state the USD cash rule in full, the credit, the return of capital and the Vorabpauschale's distributions.
//! - D-TAX-BROKER-CREDIT: Zeile 41 takes per withholding the creditable tax (§ 32d Abs. 5 S. 1): a US dividend's at most
//!   15 % (DBA USA Art. 10 Abs. 2 b) and 25 % of its taxable part, a non-US issuer's 25 % (listed), US interest's none
//!   (Art. 11); the rest is a row of its own. A withholding with no distribution of its symbol that day is not credited.
//! - D-TAX-BROKER-ROC-DIVIDEND: a return of capital is a distribution (§ 27 Abs. 8 S. 9 KStG; § 2 Abs. 11 InvStG), cost
//!   unchanged.
//! - D-TAX-BROKER-VAP-START: the Vorabpauschale's start value is the year's first close (§ 18 Abs. 1 S. 2, 3 InvStG).
//! - D-TAX-BROKER-VAP-MONTHS: the purchase year's twelfths come off the Vorabpauschale itself (§ 18 Abs. 2).
//! - D-TAX-BROKER-VAP-DISTRIBUTIONS: the fund's distributions of the whole year count for every lot (§ 18 Abs. 1 S. 1).
//! - D-TAX-BROKER-VAP-NO-PRICE: a year with no positive Basiszins asks for no price.
//! - D-TAX-BROKER-2023-LINES: the 2023 report has no Vorabpauschale rows (KAP-INV 2023 lines 9–13 "frei").
//! - D-TAX-BROKER-PANEL: the classification panel shows an uncovered sale and a missing Basiszins too (`classification_rows`);
//!   a missing close or ECB rate stays the report's (the run fetches both before its walk).
use super::{line, Cell, CryptoScope};
use crate::figures::{dec::Dec, FiguresError};
use crate::tracker::fx::FxTable;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Tax::BrokerReport::COUNTRY.
pub const COUNTRY: &str = "DE";
/// Tax::BrokerReport::SUPPORTED_YEARS: the years whose forms were verified (no 2026 form exists yet).
pub const FIRST_YEAR: i32 = 2023;
pub const LAST_YEAR: i32 = 2025;

/// Tax::Vorabpauschale::BASISZINS (BMF, § 18 Abs. 4 InvStG), checked against the letters (law-DE-broker.md § 5).
const BASISZINS: [(i32, &str); 9] = [(2018, "0.0087"), (2019, "0.0052"), (2020, "0.0007"), (2021, "-0.0045"), (2022, "-0.0005"),
                                     (2023, "0.0255"), (2024, "0.0229"), (2025, "0.0253"), (2026, "0.0320")];

const REFUSAL_ORDER: [&str; 6] = ["uncovered_disposal", "unsupported_activity", "pre_2018_fund_lot", "missing_price", "missing_fx", "missing_basiszins"];
const KINDS: [&str; 3] = ["share", "fund", "other_security"];
const CATEGORIES: [&str; 5] = ["equity_fund", "mixed_fund", "real_estate_fund", "foreign_real_estate_fund", "other_fund"];
/// BrokerReportCsv::KAP_INV_ZEILEN: (distributions, vorabpauschale, sale_result) per category, in this order.
const ZEILEN: [(&str, &str, &str); 5] = [("4", "9", "14"), ("5", "10", "17"), ("6", "11", "20"), ("7", "12", "23"), ("8", "13", "26")];
/// § 20 InvStG's Teilfreistellung per category, in percent (the credit's 25 % cap is of the taxable part, BMF Rn. 148).
const PARTIAL_EXEMPTION: [i64; 5] = [30, 15, 60, 80, 0];
const ENTRY_TYPES: [&str; 17] = ["buy", "sell", "swap_in", "swap_out", "deposit", "withdrawal", "staking_reward", "lending_interest", "airdrop",
                                 "mining", "fee", "other_income", "lost", "withholding_tax", "return_of_capital", "adjustment", "unsupported_activity"];
const BUY: i64 = 0;
const SELL: i64 = 1;
const FEE: i64 = 10;
const OTHER_INCOME: i64 = 11;
const WITHHOLDING: i64 = 13;
const RETURN_OF_CAPITAL: i64 = 14;
const ADJUSTMENT: i64 = 15;
const UNSUPPORTED: i64 = 16;

type R<T> = Result<T, FiguresError>;

fn d(text: &str) -> Dec { Dec::parse(text).unwrap_or_else(|_| Dec::zero()) }
fn add(a: &Dec, b: &Dec) -> R<Dec> { Ok((a + b)?) }
fn sub(a: &Dec, b: &Dec) -> R<Dec> { Ok((a - b)?) }
fn mul(a: &Dec, b: &Dec) -> R<Dec> { Ok((a * b)?) }
fn div(a: &Dec, b: &Dec) -> R<Dec> { Ok(a.div(b)?) }
fn min(a: Dec, b: Dec) -> Dec { if b < a { b } else { a } }
fn max(a: Dec, b: Dec) -> Dec { if b > a { b } else { a } }
fn abs(a: &Dec) -> Dec { if a.is_negative() { a.neg() } else { a.clone() } }
fn ymd(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap_or(NaiveDate::MIN) }

/// Tax::Vorabpauschale::BASISZINS.fetch(year).
pub fn basiszins(year: i32) -> Option<Dec> { BASISZINS.iter().find(|(y, _)| *y == year).map(|(_, r)| d(r)) }

/// GenerateReportJob#broker_csv's checks (its ArgumentErrors fail the run), then `broker_exchange`.
pub fn check(c: &Connection, user: i64, country: &str, year: i64) -> Result<i64, String> {
    if country != COUNTRY || !(i64::from(FIRST_YEAR)..=i64::from(LAST_YEAR)).contains(&year) {
        return Err(format!("Unsupported broker report country {country:?} and year {year:?}"));
    }
    broker_exchange(c, user).map_err(|e| format!("{e:?}"))?.ok_or_else(|| format!("No Alpaca broker ledger found for user {user}"))
}

/// GenerateReportJob.broker_exchange: the Alpaca venue the user has a key on or a row of.
pub fn broker_exchange(c: &Connection, user: i64) -> R<Option<i64>> {
    Ok(c.query_row("SELECT e.id FROM exchanges e WHERE e.type='Exchanges::Alpaca' AND (EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id=e.id) \
                    OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1 AND exchange_id=e.id)) ORDER BY e.id LIMIT 1", [user], |r| r.get(0)).optional()?)
}

// ---- the lot ledger (Tax::LotLedger) ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Lot { units: Dec, cost: Dec, acquired: NaiveDate, fx: Option<Dec>, vap_per_unit: Dec, blind: bool, uncertain: BTreeSet<(i32, &'static str)> }

#[derive(Clone, Debug)]
struct Match { units: Dec, cost: Dec, acquired: Option<NaiveDate>, fx: Option<Dec>, vap: Dec, uncovered: bool, uncertain: BTreeSet<(i32, &'static str)> }

#[derive(Default)]
struct Ledger { lots: Vec<Lot>, splits: Vec<(NaiveDate, Dec)>, blind_since: Option<NaiveDate> }

impl Ledger {
    /// Lot#units_in_terms_of: every later split of the ledger divided out (also before the lot's purchase).
    fn units_in_terms_of(&self, lot: &Lot, date: NaiveDate) -> R<Dec> {
        let mut units = lot.units.clone();
        for (day, factor) in &self.splits { if *day > date { units = div(&units, factor)?; } }
        Ok(units)
    }

    /// A lot. `blind` when it is bought in a year in which, before it, no units were held at some moment: the fund's
    /// distributions of that year before then are not in the ledger.
    fn acquire(&mut self, units: Dec, cost: Dec, date: NaiveDate, fx: Option<Dec>) {
        if self.lots.iter().all(|l| l.units.is_zero()) { self.blind_since = Some(date); }
        let blind = self.blind_since.is_some_and(|since| since.year() == date.year());
        self.lots.push(Lot { units, cost, acquired: date, fx, vap_per_unit: Dec::zero(), blind, uncertain: BTreeSet::new() });
    }

    fn adjust(&mut self, delta: &Dec, date: NaiveDate) -> R<()> {
        let mut pool = Dec::zero();
        for l in &self.lots { pool = add(&pool, &l.units)?; }
        if !pool.is_positive() { return Ok(()); }
        let factor = div(&add(&pool, delta)?, &pool)?;
        // A non-positive factor is not a split.
        if !factor.is_positive() { return Ok(()); }
        for l in &mut self.lots {
            l.units = mul(&l.units, &factor)?;
            l.vap_per_unit = div(&l.vap_per_unit, &factor)?;
        }
        self.splits.push((date, factor));
        Ok(())
    }

    fn dispose(&mut self, units: &Dec) -> R<Vec<Match>> {
        let mut remaining = units.clone();
        let mut matches = vec![];
        while remaining.is_positive() && !self.lots.is_empty() {
            let lot = &mut self.lots[0];
            let taken = min(lot.units.clone(), remaining.clone());
            let (cost, gone) = if lot.units <= remaining { (lot.cost.clone(), true) } else {
                let cost = div(&mul(&lot.cost, &taken)?, &lot.units)?;
                lot.units = sub(&lot.units, &taken)?;
                lot.cost = sub(&lot.cost, &cost)?;
                (cost, false)
            };
            matches.push(Match { vap: mul(&taken, &lot.vap_per_unit)?, units: taken.clone(), cost, acquired: Some(lot.acquired), fx: lot.fx.clone(), uncovered: false,
                                 uncertain: lot.uncertain.clone() });
            if gone { self.lots.remove(0); }
            remaining = sub(&remaining, &taken)?;
        }
        if remaining.is_positive() {
            matches.push(Match { units: remaining, cost: Dec::zero(), acquired: None, fx: None, vap: Dec::zero(), uncovered: true, uncertain: BTreeSet::new() });
        }
        Ok(matches)
    }
}

/// The Vorabpauschale of one lot (§ 18 InvStG), gross of the Teilfreistellung. `None`: no Basiszins for the year.
/// D-TAX-BROKER-VAP-MONTHS: max(0, min(Basisertrag, Mehrbetrag) − distributions), then less 1/12 per full month before the
/// purchase month in the purchase year (Rails took the twelfths off the Basisertrag before the cap).
fn vorabpauschale(year: i32, acquired: NaiveDate, start: &Dec, end: &Dec, distributions: &Dec) -> R<Option<Dec>> {
    let Some(rate) = basiszins(year) else { return Ok(None) };
    if !rate.is_positive() { return Ok(Some(Dec::zero())); }
    let basisertrag = mul(&mul(start, &d("0.7"))?, &rate)?;
    let mehrbetrag = add(&sub(end, start)?, distributions)?;
    let vap = max(Dec::zero(), sub(&min(basisertrag, mehrbetrag), distributions)?);
    if acquired.year() != year { return Ok(Some(vap)); }
    Ok(Some(div(&mul(&vap, &Dec::from_i64(13 - i64::from(acquired.month())))?, &Dec::from_i64(12))?))
}

// ---- the ledger rows ---------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Rec { id: i64, kind: i64, key: Option<i64>, base: String, amount: Dec, quote: Option<String>, quote_amount: Option<Dec>, fee: Option<Dec>, at: DateTime<Utc>, local: NaiveDate, raw: Value }

impl Rec {
    /// D-TAX-BROKER-LOCAL-DATE: a fill's day is the taxpayer's (Alpaca trades until 20:00 ET, past midnight in Berlin); a
    /// non-trade row keeps the day Alpaca booked it on (UTC), as Rails.
    fn date(&self) -> NaiveDate { if matches!(self.kind, BUY | SELL) { self.local } else { self.at.date_naive() } }
    fn activity(&self) -> String {
        self.raw["activity_type"].as_str().map(str::to_string)
            .unwrap_or_else(|| usize::try_from(self.kind).ok().and_then(|k| ENTRY_TYPES.get(k)).map_or_else(String::new, |s| (*s).to_string()))
    }
    /// The instrument a row is about (`instrument_symbol`): the dividend family and withholding name it in the quote.
    fn symbol(&self) -> Option<String> {
        let raw = if matches!(self.kind, OTHER_INCOME | WITHHOLDING) { self.quote.as_deref() } else { Some(self.base.as_str()) };
        raw.filter(|s| !s.trim().is_empty() && !crate::tracker::fiat(s)).map(str::to_string)
    }
    /// The money a distribution paid: a return of capital's is its net amount (its `base_amount` is units), a dividend's its
    /// signed amount.
    fn paid_usd(&self) -> R<Dec> {
        if self.kind != RETURN_OF_CAPITAL { return self.signed_usd(); }
        match self.raw.get("net_amount").filter(|v| !v.is_null() && v.as_str() != Some("")) {
            Some(v) => Ok(Dec::to_d(v)?),
            None => Ok(self.quote_amount.clone().unwrap_or_else(Dec::zero)),
        }
    }
    /// What a withholding row withheld, signed: Alpaca books the debit negative and a refund positive.
    fn withheld_usd(&self) -> R<Dec> {
        // Without Alpaca's net amount (an imported row) the row's own sign: a positive amount withheld, a negative one refunded.
        match self.raw.get("net_amount").filter(|v| !v.is_null() && v.as_str() != Some("")) { Some(v) => Ok(Dec::to_d(v)?.neg()), None => Ok(self.amount.clone()) }
    }
    /// The shares a distribution was paid on (Alpaca's `qty`), when the activity states them.
    fn qty(&self) -> R<Option<Dec>> {
        match self.raw.get("qty").filter(|v| !v.is_null() && v.as_str() != Some("")) { Some(v) => Ok(Some(Dec::to_d(v)?).filter(Dec::is_positive)), None => Ok(None) }
    }
    /// `signed_usd`: the activity's own net amount keeps a reversal's sign.
    fn signed_usd(&self) -> R<Dec> {
        match self.raw.get("net_amount").filter(|v| !v.is_null() && v.as_str() != Some("")) {
            Some(v) => Ok(Dec::to_d(v)?),
            None => Ok(self.amount.clone()),
        }
    }
    fn per_share(&self) -> R<Option<Dec>> {
        match self.raw.get("per_share_amount").filter(|v| !v.is_null() && v.as_str() != Some("")) { Some(v) => Ok(Some(Dec::to_d(v)?)), None => Ok(None) }
    }
}

fn records(c: &Connection, user: i64, exchange: i64, year: i32) -> R<Vec<Rec>> {
    let zone: String = c.query_row("SELECT COALESCE(time_zone, 'UTC') FROM users WHERE id = ?1", [user], |r| r.get(0))?;
    let mut s = c.prepare("SELECT id, entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_amount, transacted_at, raw_data, api_key_id \
                           FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at < ?3 ORDER BY transacted_at, id")?;
    // Every row of the year in any zone; each kept by its own day's year.
    let until = format!("{}-01-02 12:00:00", year + 1);
    let mut rows = s.query(rusqlite::params![user, exchange, until])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        let number = |i: usize| -> R<Option<Dec>> { Ok(Dec::from_sql(r.get_ref(i)?)?) };
        let at: String = r.get(7)?;
        let at = crate::codec::parse_time(&at).map_err(|_| FiguresError::NotComputed("an unreadable transaction time".into()))?;
        let raw: Option<String> = r.get(8)?;
        let local = crate::web::timezone::local(at, &zone).date_naive();
        out.push(Rec { id: r.get(0)?, kind: r.get(1)?, key: r.get(9)?, base: r.get(2)?, amount: number(3)?.unwrap_or_else(Dec::zero), quote: r.get(4)?,
                       quote_amount: number(5)?, fee: number(6)?, at, local,
                       raw: raw.and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| serde_json::json!({})) });
    }
    out.retain(|r| r.date().year() <= year);
    Ok(out)
}

// ---- year-boundary closes --------------------------------------------------------------------------------------------

/// A window of closes the report asks for (`stock_price_range`'s from..=to): a year's start (`first`) or its end.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Window { pub symbol: String, pub from: NaiveDate, pub to: NaiveDate, pub first: bool }

impl Window {
    /// The days a stored close makes the window fresh (no fetch): the end window's from its last weekday on (Rails); the start
    /// window's on its first weekday (in a holiday year the window is fetched again; the answer holds the first trading
    /// day's close, which must lie within the first three weekdays: D-TAX-BROKER-VAP-START).
    pub fn fresh(&self) -> (NaiveDate, NaiveDate) {
        if !self.first { return (crate::tracker::prices::last_weekday(self.from, self.to), self.to); }
        // The first three weekdays from 2 January: a holiday (2 January 2023) moves the first close a day or two.
        let weekday = |d: &NaiveDate| d.weekday().number_from_monday() <= 5;
        let days: Vec<NaiveDate> = self.from.iter_days().filter(weekday).take(3).collect();
        (days[0], days[0])
    }
}

/// The third weekday from `from`: a start close later than it is not the year's first (a holiday moves it a day or two).
fn first_days_end(from: NaiveDate) -> NaiveDate { from.iter_days().filter(|d| d.weekday().number_from_monday() <= 5).nth(2).unwrap_or(from) }

/// A year-boundary close: none, the close (its day, EUR), or a close whose day has no ECB rate.
enum Pick { Nothing, Close(NaiveDate, Dec), NoRate(NaiveDate) }

/// Where a year-boundary close comes from.
pub enum Closes<'a> {
    /// The run's first pass: every window asked for, each answered as if priced.
    Needs(RefCell<BTreeSet<Window>>),
    /// The stored closes, but the windows whose fetch failed (Rails' empty answer).
    Stored { c: &'a Connection, failed: &'a HashSet<Window> },
    /// The panel: the stored closes, and a missing one refuses nothing (the run fetches first).
    Panel(&'a Connection),
}

impl Closes<'_> {
    /// The window's first (start) or last (end) close, chosen by day, then in EUR at that day's ECB rate.
    fn pick(&self, fx: &FxTable, w: Window) -> R<Pick> {
        let c = match self {
            Closes::Needs(asked) => { let day = w.from; asked.borrow_mut().insert(w); return Ok(Pick::Close(day, Dec::one())); }
            Closes::Stored { failed, .. } if failed.contains(&w) => return Ok(Pick::Nothing),
            Closes::Stored { c, .. } | Closes::Panel(c) => c,
        };
        let closes = crate::tracker::prices::stored_closes(c, &format!("stock:{}", w.symbol), w.from, w.to)?;
        let chosen = if w.first { closes.into_iter().next() } else { closes.into_iter().next_back() };
        let Some((day, close)) = chosen else { return Ok(Pick::Nothing) };
        // A start close past the first trading days (a partial cache, an empty fetch) is no start value.
        if w.first && day > first_days_end(w.from) { return Ok(Pick::Nothing); }
        Ok(match fx.rate("EUR", "USD", day)? { Some(rate) => Pick::Close(day, div(&close, &rate)?), None => Pick::NoRate(day) })
    }
    fn panel(&self) -> bool { matches!(self, Closes::Panel(_)) }
}

// ---- the walk --------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Detail { Date(NaiveDate), Year(i32), Text(String), List(Vec<String>) }

impl Detail {
    fn text(&self) -> String {
        match self { Detail::Date(day) => day.to_string(), Detail::Year(y) => y.to_string(), Detail::Text(t) => t.clone(), Detail::List(l) => l.join(", ") }
    }
}

#[derive(Clone, Debug)]
struct Warning { code: &'static str, symbol: Option<String>, detail: Detail }

#[derive(Clone, Debug)]
struct Kap { z19: Dec, z20: Dec, z22: Dec, z23: Dec, z41: Dec }
impl Kap { fn new() -> Kap { Kap { z19: Dec::zero(), z20: Dec::zero(), z22: Dec::zero(), z23: Dec::zero(), z41: Dec::zero() } } }

#[derive(Clone, Debug)]
struct Inv { distributions: Dec, vap: Dec, sale: Dec }
impl Inv { fn new() -> Inv { Inv { distributions: Dec::zero(), vap: Dec::zero(), sale: Dec::zero() } } }

struct Dist { date: NaiveDate, gross_eur: Option<Dec>, per_share_usd: Option<Dec>, units_outstanding: Dec, fx: Option<Dec> }

const UNSEEN: &str = "vorabpauschale_distributions_unknown";
const ESTIMATED: &str = "vorabpauschale_distributions_estimated";

struct State { symbol: String, kind: Option<usize>, category: Option<usize>, persisted: bool, recs: Vec<usize>, ledger: Ledger, distributions: Vec<Dist>, kap: Kap, inv: Inv }

impl State {
    fn fund(&self) -> bool { self.kind == Some(1) }
    /// The Teilfreistellung of this instrument's income, in percent (0 for shares, other securities, unclassified).
    fn exemption(&self) -> i64 { if self.fund() { self.category.map_or(0, |k| PARTIAL_EXEMPTION[k]) } else { 0 } }
}

struct Disposal { symbol: String, kind: Option<usize>, category: Option<usize>, sold_on: NaiveDate, units: Dec, proceeds_eur: Option<Dec>, proceeds_usd: Dec,
                  fx: Option<Dec>, fee_eur: Option<Dec>, cost: Dec, gain: Option<Dec>, vap: Dec, refused: bool, matches: Vec<Match> }

struct Income { symbol: Option<String>, date: NaiveDate, activity: String, bucket: &'static str, usd: Option<Dec>, fx: Option<Dec>, eur: Option<Dec>, refused: bool }

/// One row of the classification panel (`symbol_rows`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolRow { pub symbol: String, pub kind: Option<usize>, pub category: Option<usize>, pub persisted: bool, pub reasons: Vec<&'static str> }

struct Walk<'a> {
    c: &'a Connection, user: i64, year: i32, fx: FxTable, closes: &'a Closes<'a>,
    recs: Vec<Rec>, rates: BTreeMap<i64, Option<Dec>>, gross_usd: BTreeMap<i64, Dec>, scale: BTreeMap<i64, Dec>,
    /// D-TAX-BROKER-CREDIT: the EUR gross of the distributions of a (symbol, day), None when one lacked its rate.
    gross_by_day: BTreeMap<(String, NaiveDate), Option<Dec>>,
    /// What has been credited against each (symbol, day)'s income: one allowance per income item, not per withholding row.
    credited: BTreeMap<(String, NaiveDate), Dec>,
    withheld: BTreeMap<(String, NaiveDate, bool), Dec>,
    /// How many distribution rows each (symbol, day) has (more than one: the allowance is pooled, and said).
    paid_by_day: BTreeMap<(String, NaiveDate), usize>,
    states: BTreeMap<String, State>, refusals: BTreeMap<String, BTreeSet<&'static str>>,
    warnings: Vec<Warning>, warned: HashSet<(&'static str, Option<String>, Detail)>,
    disposals: Vec<Disposal>, income: Vec<Income>, fee_total: Dec, cash: Kap,
}

impl<'a> Walk<'a> {
    fn new(c: &'a Connection, user: i64, exchange: i64, year: i32, closes: &'a Closes<'a>) -> R<Walk<'a>> {
        let scope = CryptoScope::new(c, user)?;
        let mut w = Walk { c, user, year, fx: FxTable::load(c)?, closes, recs: vec![], rates: BTreeMap::new(), gross_usd: BTreeMap::new(),
                           scale: BTreeMap::new(), gross_by_day: BTreeMap::new(), credited: BTreeMap::new(), withheld: BTreeMap::new(), paid_by_day: BTreeMap::new(), states: BTreeMap::new(), refusals: BTreeMap::new(),
                           warnings: vec![], warned: HashSet::new(), disposals: vec![], income: vec![], fee_total: Dec::zero(), cash: Kap::new() };
        // `universe.reject { cryptocurrency?(instrument_symbol(record)) }`, the ambiguous categories warned on the way.
        for r in records(c, user, exchange, year)? {
            let crypto = match r.symbol() {
                None => false,
                Some(symbol) => {
                    if let Some(categories) = scope.ambiguous(c, &symbol)? { w.warn("ambiguous_symbol", Some(symbol.clone()), Detail::List(categories)); }
                    scope.crypto(c, &symbol)?
                }
            };
            if !crypto { w.recs.push(r); }
        }
        // FIFO is per depot (BMF Rn. 97): several Alpaca accounts are walked as one, and said so.
        let keys: BTreeSet<i64> = w.recs.iter().filter_map(|r| r.key).collect();
        if keys.len() > 1 { w.warn("several_accounts", None, Detail::Text(keys.len().to_string())); }
        w.build_states()?;
        Ok(w)
    }

    fn warn(&mut self, code: &'static str, symbol: Option<String>, detail: Detail) {
        if self.warned.insert((code, symbol.clone(), detail.clone())) { self.warnings.push(Warning { code, symbol, detail }); }
    }

    fn refuse(&mut self, symbol: &str, code: &'static str, detail: Detail) {
        self.refusals.entry(symbol.to_string()).or_default().insert(code);
        self.warn(code, Some(symbol.to_string()), detail);
    }

    fn refused(&self, symbol: &str) -> bool { self.refusals.get(symbol).is_some_and(|r| !r.is_empty()) }

    fn report_year(&self, r: &Rec) -> bool { r.date().year() == self.year }

    fn build_states(&mut self) -> R<()> {
        let symbols: BTreeSet<String> = self.recs.iter().filter_map(Rec::symbol).collect();
        for symbol in symbols {
            let (kind, category, persisted) = resolve(self.c, self.user, &symbol)?;
            let recs = self.recs.iter().enumerate().filter(|(_, r)| r.symbol().as_deref() == Some(symbol.as_str())).map(|(i, _)| i).collect();
            self.states.insert(symbol.clone(), State { symbol, kind, category, persisted, recs, ledger: Ledger::default(), distributions: vec![],
                                                       kap: Kap::new(), inv: Inv::new() });
        }
        Ok(())
    }

    /// `prepare_distribution_gross_up`: a dividend with a DIVFT the same day is net; its gross is the two together.
    fn gross_up(&mut self) -> R<()> {
        let mut divft: BTreeMap<(String, NaiveDate), Dec> = BTreeMap::new();
        for r in self.recs.iter().filter(|r| r.kind == WITHHOLDING && r.activity() == "DIVFT") {
            let Some(symbol) = r.symbol() else { continue };
            let total = divft.entry((symbol, r.date())).or_insert_with(Dec::zero);
            *total = add(total, &r.withheld_usd()?)?;
        }
        let mut groups: BTreeMap<(String, NaiveDate), Vec<usize>> = BTreeMap::new();
        for (i, r) in self.recs.iter().enumerate() {
            if matches!(r.kind, OTHER_INCOME | RETURN_OF_CAPITAL) { if let Some(symbol) = r.symbol() { groups.entry((symbol, r.date())).or_default().push(i); } }
        }
        for (key, members) in groups {
            let mut net = Dec::zero();
            for &i in &members { net = add(&net, &self.recs[i].paid_usd()?)?; }
            let tax = divft.get(&key).cloned().unwrap_or_else(Dec::zero);
            let scale = if net.is_zero() { Dec::one() } else { div(&add(&net, &tax)?, &net)? };
            let mut remaining = add(&net, &tax)?;
            for (n, &i) in members.iter().enumerate() {
                let gross = if n == members.len() - 1 { remaining.clone() } else { mul(&self.recs[i].paid_usd()?, &scale)? };
                remaining = sub(&remaining, &gross)?;
                self.gross_usd.insert(self.recs[i].id, gross);
                self.scale.insert(self.recs[i].id, scale.clone());
            }
        }
        Ok(())
    }

    /// The USD of a distribution as declared: grossed up by a same-day DIVFT (a return of capital too: D-TAX-BROKER-ROC-DIVIDEND).
    fn distribution_usd(&self, r: &Rec) -> R<Dec> {
        match self.gross_usd.get(&r.id) { Some(g) => Ok(g.clone()), None => r.paid_usd() }
    }

    fn prepare_refusals(&mut self) {
        let unsupported: Vec<(Option<String>, String)> = self.recs.iter().filter(|r| r.kind == UNSUPPORTED).map(|r| (r.symbol(), r.activity())).collect();
        for (symbol, activity) in unsupported {
            match symbol {
                Some(symbol) => self.refuse(&symbol, "unsupported_activity", Detail::Text(activity)),
                None => self.warn("unsupported_activity", None, Detail::Text(activity)),
            }
        }
        let early: Vec<(String, NaiveDate)> = self.states.values().filter(|s| s.fund())
            .flat_map(|s| s.recs.iter().map(|&i| &self.recs[i]).filter(|r| r.kind == BUY && r.date() < ymd(2018, 1, 1)).map(|r| (s.symbol.clone(), r.date())).collect::<Vec<_>>())
            .collect();
        for (symbol, day) in early { self.refuse(&symbol, "pre_2018_fund_lot", Detail::Date(day)); }
    }

    fn prepare_fx(&mut self) -> R<()> {
        for i in 0..self.recs.len() {
            let r = &self.recs[i];
            if !matches!(r.kind, BUY | SELL | OTHER_INCOME | WITHHOLDING | RETURN_OF_CAPITAL | FEE) { continue; }
            let (id, day, symbol) = (r.id, r.date(), r.symbol());
            let rate = self.fx.rate("EUR", "USD", day)?;
            self.rates.insert(id, rate.clone());
            // The run loads the ECB's rates before its walk; the panel reads what is stored, so a gap there refuses nothing.
            if rate.is_some() || self.closes.panel() { continue; }
            match symbol {
                Some(symbol) => self.refuse(&symbol, "missing_fx", Detail::Date(day)),
                None => self.warn("missing_fx", None, Detail::Date(day)),
            }
        }
        // D-TAX-BROKER-CREDIT: each (symbol, day)'s distributions in EUR, the gross a withholding is capped against.
        for i in 0..self.recs.len() {
            let r = &self.recs[i];
            if !matches!(r.kind, OTHER_INCOME | RETURN_OF_CAPITAL) { continue; }
            let Some(symbol) = r.symbol() else { continue };
            let eur = match self.rates.get(&r.id).cloned().flatten() { Some(rate) => Some(div(&self.distribution_usd(r)?, &rate)?), None => None };
            *self.paid_by_day.entry((symbol.clone(), r.date())).or_insert(0) += 1;
            let entry = self.gross_by_day.entry((symbol, r.date())).or_insert_with(|| Some(Dec::zero()));
            *entry = match (entry.take(), eur) { (Some(sum), Some(eur)) => Some(add(&sum, &eur)?), _ => None };
        }
        Ok(())
    }

    fn rate(&self, r: &Rec) -> Option<Dec> { self.rates.get(&r.id).cloned().flatten() }

    fn walk_symbol(&mut self, symbol: &str) -> R<()> {
        // In the order of the days the report dates them (a fill by the taxpayer's day, a non-trade row by Alpaca's), so a year
        // boundary never passes a row of the year before it.
        let mut recs = self.states[symbol].recs.clone();
        recs.sort_by_key(|&i| (self.recs[i].date(), self.recs[i].at, self.recs[i].id));
        let first_buy = recs.iter().map(|&i| &self.recs[i]).filter(|r| r.kind == BUY).map(Rec::date).min();
        let mut boundary = first_buy.map(|day| day.year());
        for i in recs {
            let day = self.recs[i].date();
            while let Some(y) = boundary.filter(|y| *y < self.year && day >= ymd(y + 1, 1, 1)) {
                if !self.refused(symbol) { self.accrue(symbol, y)?; }
                boundary = Some(y + 1);
            }
            self.handle(symbol, i)?;
        }
        while let Some(y) = boundary.filter(|y| *y < self.year) {
            if !self.refused(symbol) { self.accrue(symbol, y)?; }
            boundary = Some(y + 1);
        }
        Ok(())
    }

    fn handle(&mut self, symbol: &str, i: usize) -> R<()> {
        let r = self.recs[i].clone();
        match r.kind {
            BUY => {
                let rate = self.rate(&r);
                let usd = add(&r.quote_amount.clone().unwrap_or_else(Dec::zero), &r.fee.clone().unwrap_or_else(Dec::zero))?;
                let cost = match &rate { Some(rate) => div(&usd, rate)?, None => Dec::zero() };
                self.state(symbol).ledger.acquire(r.amount.clone(), cost, r.date(), rate);
            }
            SELL => self.sell(symbol, &r)?,
            // Alpaca has already merged split legs, so base_amount is the net delta.
            ADJUSTMENT => self.state(symbol).ledger.adjust(&r.amount, r.date())?,
            OTHER_INCOME | RETURN_OF_CAPITAL => self.distribution(symbol, &r)?,
            WITHHOLDING => self.withholding(Some(symbol), &r)?,
            _ => {}
        }
        Ok(())
    }

    fn state(&mut self, symbol: &str) -> &mut State { self.states.get_mut(symbol).expect("a walked symbol has its state") }

    fn sell(&mut self, symbol: &str, r: &Rec) -> R<()> {
        let sold_on = r.date();
        let matches = self.state(symbol).ledger.dispose(&r.amount)?;
        if matches.iter().any(|m| m.uncovered) { self.refuse(symbol, "uncovered_disposal", Detail::Date(sold_on)); }
        let rate = self.rate(r);
        let quote = r.quote_amount.clone().unwrap_or_else(Dec::zero);
        let fee = r.fee.clone().unwrap_or_else(Dec::zero);
        let usd = |amount: Option<&Dec>| -> R<Option<Dec>> {
            match (amount, &rate) { (None, _) => Ok(Some(Dec::zero())), (Some(_), None) => Ok(None), (Some(a), Some(rate)) => Ok(Some(div(a, rate)?)) }
        };
        let fee_eur = usd(r.fee.as_ref())?;
        let proceeds_usd = sub(&quote, &fee)?;
        let proceeds_eur = usd(Some(&proceeds_usd))?;
        let mut cost = Dec::zero();
        let mut vap = Dec::zero();
        for m in &matches { cost = add(&cost, &m.cost)?; vap = add(&vap, &m.vap)?; }
        let state = &self.states[symbol];
        let gain = match &proceeds_eur { None => None, Some(p) => { let g = sub(p, &cost)?; Some(if state.fund() { sub(&g, &vap)? } else { g }) } };
        if !self.report_year(r) { return Ok(()); }
        // A deduction resting on an uncertain year (an estimated one even at zero: it may have been too low) is said again here.
        let uncertain: BTreeSet<(i32, &'static str)> = matches.iter().flat_map(|m| m.uncertain.iter().copied()).collect();
        for (year, code) in uncertain { self.warn(code, Some(symbol.to_string()), Detail::Year(year)); }
        if let Some(g) = &gain {
            let s = self.state(symbol);
            match s.kind {
                Some(1) => s.inv.sale = add(&s.inv.sale, g)?,
                Some(0) => {
                    s.kap.z19 = add(&s.kap.z19, g)?;
                    if g.is_positive() { s.kap.z20 = add(&s.kap.z20, g)?; }
                    if g.is_negative() { s.kap.z23 = sub(&s.kap.z23, g)?; }
                }
                Some(2) => {
                    s.kap.z19 = add(&s.kap.z19, g)?;
                    if g.is_negative() { s.kap.z22 = sub(&s.kap.z22, g)?; }
                }
                _ => {}
            }
        }
        let s = &self.states[symbol];
        self.disposals.push(Disposal { symbol: symbol.to_string(), kind: s.kind, category: s.category, sold_on, units: r.amount.clone(), proceeds_eur,
                                       proceeds_usd, fx: rate, fee_eur, cost, gain, vap, refused: false, matches });
        Ok(())
    }

    fn distribution(&mut self, symbol: &str, r: &Rec) -> R<()> {
        let rate = self.rate(r);
        let gross_usd = self.distribution_usd(r)?;
        let gross_eur = match &rate { Some(rate) => Some(div(&gross_usd, rate)?), None => None };
        let scale = self.scale.get(&r.id).cloned().unwrap_or_else(Dec::one);
        // Per share as stated, else over the shares it was paid on (`qty`); only then over the shares held that day (Rails).
        let per_share_usd = match (r.per_share()?, r.qty()?) {
            (Some(p), _) => Some(mul(&p, &scale)?),
            (None, Some(qty)) => Some(div(&gross_usd, &qty)?),
            _ => None,
        };
        let date = r.date();
        let mut outstanding = Dec::zero();
        for l in self.states[symbol].ledger.lots.iter().filter(|l| l.acquired <= date) { outstanding = add(&outstanding, &l.units)?; }
        self.state(symbol).distributions.push(Dist { date, gross_eur: gross_eur.clone(), per_share_usd, units_outstanding: outstanding, fx: rate.clone() });
        if !self.report_year(r) { return Ok(()); }
        let fund = self.states[symbol].fund();
        self.income.push(Income { symbol: Some(symbol.to_string()), date, activity: r.activity(), bucket: if fund { "fund_distribution" } else { "kap_dividend" },
                                  usd: Some(gross_usd), fx: rate, eur: gross_eur.clone(), refused: false });
        let Some(eur) = gross_eur else { return Ok(()) };
        let s = self.state(symbol);
        if s.fund() { s.inv.distributions = add(&s.inv.distributions, &eur)?; } else if s.kind.is_some() { s.kap.z19 = add(&s.kap.z19, &eur)?; }
        Ok(())
    }

    /// A withholding row: what was withheld is shown, what is creditable goes to Zeile 41 (D-TAX-BROKER-CREDIT), the rest is
    /// a row of its own. Per (symbol, day) the credit is min(max(Σ signed withheld, 0), cap) over that day's distributions; each
    /// row adds the change of it, so a second row cannot reuse the allowance and a refund lowers the credit only below the cap.
    fn withholding(&mut self, symbol: Option<&str>, r: &Rec) -> R<()> {
        let rate = self.rate(r);
        // Alpaca books a withholding as a debit (negative net amount) and a refund as a credit.
        let usd = r.withheld_usd()?;
        let eur = match &rate { Some(rate) => Some(div(&usd, rate)?), None => None };
        if !self.report_year(r) { return Ok(()); }
        let activity = r.activity();
        self.income.push(Income { symbol: symbol.map(str::to_string), date: r.date(), activity: activity.clone(), bucket: "withholding_tax",
                                  usd: Some(usd), fx: rate, eur: eur.clone(), refused: false });
        let Some(eur) = eur else { return Ok(()) };
        let creditable = match symbol {
            // DBA USA Art. 11: interest is taxed only where its owner lives; US tax on it is reclaimed, never credited.
            _ if activity.starts_with("INT") => Dec::zero(),
            // A dividend's withholding that names no security matches no income: not credited, and said.
            None => { self.warn("withholding_unmatched", None, Detail::Date(r.date())); Dec::zero() }
            Some(symbol) => {
                let key = (symbol.to_string(), r.date());
                let gross = self.gross_by_day.get(&key).cloned().flatten().filter(Dec::is_positive);
                match gross {
                    None => { self.warn("withholding_unmatched", Some(symbol.to_string()), Detail::Date(r.date())); Dec::zero() }
                    Some(gross) => {
                        if self.paid_by_day.get(&key).is_some_and(|n| *n > 1) { self.warn("withholding_pooled", Some(symbol.to_string()), Detail::Date(r.date())); }
                        // One German ceiling per item (25 % of its taxable part); under it each kind of withholding its own allowance:
                        // US tax at most 15 % of the gross (the treaty), another state's (DIVFT) the ceiling alone. Order-free.
                        let ceiling = mul(&div(&mul(&gross, &Dec::from_i64(100 - self.states[symbol].exemption()))?, &Dec::from_i64(100))?, &d("0.25"))?;
                        let us = matches!(activity.as_str(), "DIVNRA" | "DIVTW");
                        if !us { self.warn("withholding_country_unknown", Some(symbol.to_string()), Detail::Date(r.date())); }
                        let held = self.withheld.entry((key.0.clone(), key.1, us)).or_insert_with(Dec::zero);
                        *held = add(held, &eur)?;
                        let part = |us: bool| -> R<Dec> {
                            let held = self.withheld.get(&(key.0.clone(), key.1, us)).cloned().unwrap_or_else(Dec::zero);
                            let cap = if us { min(ceiling.clone(), mul(&gross, &d("0.15"))?) } else { ceiling.clone() };
                            Ok(min(max(Dec::zero(), held), cap))
                        };
                        let total = min(add(&part(true)?, &part(false)?)?, ceiling.clone());
                        let credited = self.credited.entry(key).or_insert_with(Dec::zero);
                        let change = sub(&total, credited)?;
                        *credited = total;
                        change
                    }
                }
            }
        };
        let kap = match symbol { Some(symbol) => &mut self.state(symbol).kap, None => &mut self.cash };
        kap.z41 = add(&kap.z41, &creditable)?;
        let rest = sub(&eur, &creditable)?;
        if !rest.is_zero() {
            self.income.push(Income { symbol: symbol.map(str::to_string), date: r.date(), activity, bucket: "withholding_not_creditable",
                                      usd: None, fx: None, eur: Some(rest), refused: false });
        }
        Ok(())
    }

    fn cash(&mut self) -> R<()> {
        for i in 0..self.recs.len() {
            let r = self.recs[i].clone();
            if r.symbol().is_some() { continue; }
            match r.kind {
                OTHER_INCOME => {
                    let rate = self.rate(&r);
                    let usd = r.signed_usd()?;
                    let eur = match &rate { Some(rate) => Some(div(&usd, rate)?), None => None };
                    if !self.report_year(&r) { continue; }
                    // D-TAX-BROKER-NEGATIVE-INTEREST: Alpaca's INT is credit and margin interest; a debit is interest paid, which
                    // § 20 Abs. 9 EStG does not let reduce the income (a reversal of a credit would: the user is asked).
                    if usd.is_negative() {
                        self.warn("negative_interest", None, Detail::Date(r.date()));
                        self.income.push(Income { symbol: None, date: r.date(), activity: r.activity(), bucket: "margin_interest", usd: Some(usd), fx: rate, eur, refused: false });
                        continue;
                    }
                    self.income.push(Income { symbol: None, date: r.date(), activity: r.activity(), bucket: "kap_interest", usd: Some(usd), fx: rate, eur: eur.clone(), refused: false });
                    if let Some(eur) = eur { self.cash.z19 = add(&self.cash.z19, &eur)?; }
                }
                WITHHOLDING => self.withholding(None, &r)?,
                FEE => {
                    if !self.report_year(&r) { continue; }
                    // Alpaca's daily regulatory aggregate belongs to no sale: disclosed, deducted from nothing.
                    if let Some(rate) = self.rate(&r) { self.fee_total = add(&self.fee_total, &div(&abs(&r.amount), &rate)?)?; }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `accrue_vorabpauschale` for computation year `year`, declared in the report of `year` + 1.
    fn accrue(&mut self, symbol: &str, year: i32) -> R<()> {
        let s = &self.states[symbol];
        if !s.fund() || s.ledger.lots.is_empty() { return Ok(()); }
        // D-TAX-BROKER-VAP-NO-PRICE: no positive Basiszins, no Vorabpauschale and no price asked.
        match basiszins(year) {
            None => { self.refuse(symbol, "missing_basiszins", Detail::Year(year)); return Ok(()); }
            Some(rate) if !rate.is_positive() => return Ok(()),
            Some(_) => {}
        }
        // D-TAX-BROKER-VAP-START: the year's first close and its last, each chosen by day.
        let window = |from: NaiveDate, to: NaiveDate, first: bool| Window { symbol: symbol.to_string(), from, to, first };
        let start = self.closes.pick(&self.fx, window(ymd(year, 1, 2), ymd(year, 1, 10), true))?;
        let end = self.closes.pick(&self.fx, window(ymd(year, 12, 22), ymd(year, 12, 31), false))?;
        let ((start_day, start), (end_day, end)) = match (start, end) {
            (Pick::Close(a, x), Pick::Close(b, y)) => ((a, x), (b, y)),
            (Pick::NoRate(day), _) | (_, Pick::NoRate(day)) => { if !self.closes.panel() { self.refuse(symbol, "missing_fx", Detail::Date(day)); } return Ok(()); }
            _ => { if !self.closes.panel() { self.refuse(symbol, "missing_price", Detail::Year(year)); } return Ok(()); }
        };
        let year_end = ymd(year, 12, 31);
        let declared = year == self.year - 1;
        let s = &self.states[symbol];
        let mut accruals = vec![];
        for (n, lot) in s.ledger.lots.iter().enumerate() {
            // A lot bought after the year's end has no Vorabpauschale for it.
            if lot.acquired > year_end { continue; }
            // Each lot's units in the terms of the chosen close's day.
            let start_value = mul(&s.ledger.units_in_terms_of(lot, start_day)?, &start)?;
            let end_value = mul(&s.ledger.units_in_terms_of(lot, end_day)?, &end)?;
            let mut distributions = Dec::zero();
            let mut estimated = false;
            // D-TAX-BROKER-VAP-DISTRIBUTIONS: the fund's distributions of the whole year, a lot bought after one included.
            for dist in s.distributions.iter().filter(|x| x.date.year() == year) {
                let units = s.ledger.units_in_terms_of(lot, dist.date)?;
                let eur = match (&dist.per_share_usd, &dist.fx, &dist.gross_eur) {
                    (Some(per_share), Some(fx), _) => div(&mul(per_share, &units)?, fx)?,
                    (Some(_), None, _) => Dec::zero(),
                    // Neither a per-share amount nor the shares it was paid on: split by the holding that day, an estimate.
                    (None, _, Some(gross)) if dist.units_outstanding.is_positive() => { estimated = true; div(&mul(gross, &units)?, &dist.units_outstanding)? }
                    _ => Dec::zero(),
                };
                distributions = add(&distributions, &eur)?;
            }
            let vap = vorabpauschale(year, lot.acquired, &start_value, &end_value, &distributions)?.unwrap_or_else(Dec::zero);
            accruals.push((n, vap, estimated));
        }
        // A blind lot may have missed distributions the ledger cannot show: its VAP is an upper bound, marked on the lot (a later
        // sale's deduction is then a lower bound of its gain) and said where it is declared.
        let unseen = accruals.iter().any(|(n, vap, _)| vap.is_positive() && s.ledger.lots[*n].blind && s.ledger.lots[*n].acquired.year() == year);
        // An estimate can err either way, so it is said even where it gave no Vorabpauschale.
        let estimated = accruals.iter().any(|(_, _, e)| *e);
        if unseen && declared { self.warn(UNSEEN, Some(symbol.to_string()), Detail::Year(year)); }
        if estimated && declared { self.warn(ESTIMATED, Some(symbol.to_string()), Detail::Year(year)); }
        let s = self.state(symbol);
        for (n, vap, estimated) in accruals {
            let lot = &mut s.ledger.lots[n];
            if vap.is_positive() && lot.blind && lot.acquired.year() == year { lot.uncertain.insert((year, UNSEEN)); }
            if estimated { lot.uncertain.insert((year, ESTIMATED)); }
            if lot.units.is_positive() { lot.vap_per_unit = add(&lot.vap_per_unit, &div(&vap, &lot.units)?)?; }
            if declared { s.inv.vap = add(&s.inv.vap, &vap)?; }
        }
        Ok(())
    }

    fn run(&mut self) -> R<()> {
        self.gross_up()?;
        self.prepare_refusals();
        self.prepare_fx()?;
        let symbols: Vec<String> = self.states.keys().cloned().collect();
        for symbol in symbols { self.walk_symbol(&symbol)?; }
        self.cash()?;
        for row in &mut self.disposals { row.refused = self.refusals.get(&row.symbol).is_some_and(|r| !r.is_empty()); }
        for row in &mut self.income { row.refused = row.symbol.as_ref().is_some_and(|s| self.refusals.get(s).is_some_and(|r| !r.is_empty())); }
        self.disposals.sort_by(|a, b| (a.sold_on, &a.symbol).cmp(&(b.sold_on, &b.symbol)));
        self.income.sort_by(|a, b| (a.date, a.symbol.as_deref().unwrap_or(""), a.activity.as_str()).cmp(&(b.date, b.symbol.as_deref().unwrap_or(""), b.activity.as_str())));
        Ok(())
    }

    fn symbol_rows(&self) -> Vec<SymbolRow> {
        self.states.values().map(|s| SymbolRow {
            symbol: s.symbol.clone(), kind: s.kind, category: s.category, persisted: s.persisted,
            reasons: REFUSAL_ORDER.iter().copied().filter(|r| self.refusals.get(&s.symbol).is_some_and(|set| set.contains(r))).collect(),
        }).collect()
    }

    /// `aggregate_kap`: the cash rows and every symbol not refused; a refused symbol's credit still counts, and says so.
    fn kap(&mut self) -> R<Kap> {
        let mut total = self.cash.clone();
        let mut credited = vec![];
        for s in self.states.values() {
            let refused = self.refusals.get(&s.symbol).is_some_and(|r| !r.is_empty());
            if !refused {
                total = Kap { z19: add(&total.z19, &s.kap.z19)?, z20: add(&total.z20, &s.kap.z20)?, z22: add(&total.z22, &s.kap.z22)?,
                              z23: add(&total.z23, &s.kap.z23)?, z41: add(&total.z41, &s.kap.z41)? };
                continue;
            }
            if s.kap.z41.is_zero() { continue; }
            total.z41 = add(&total.z41, &s.kap.z41)?;
            credited.push(s.symbol.clone());
        }
        if !credited.is_empty() { credited.sort(); self.warn("withholding_credited_for_refused_symbol", None, Detail::List(credited)); }
        Ok(total)
    }

    fn kap_inv(&self) -> R<BTreeMap<usize, Inv>> {
        let mut out: BTreeMap<usize, Inv> = BTreeMap::new();
        for s in self.states.values() {
            let (true, Some(category)) = (s.fund(), s.category) else { continue };
            if self.refusals.get(&s.symbol).is_some_and(|r| !r.is_empty()) { continue; }
            let t = out.entry(category).or_insert_with(Inv::new);
            *t = Inv { distributions: add(&t.distributions, &s.inv.distributions)?, vap: add(&t.vap, &s.inv.vap)?, sale: add(&t.sale, &s.inv.sale)? };
        }
        Ok(out)
    }
}

/// FundClassification.resolve: the user's classification, else a proposal from the catalogue (an ETF a fund of the
/// other category, a stock a share), else unclassified. (kind, category, persisted).
pub fn resolve(c: &Connection, user: i64, symbol: &str) -> R<(Option<usize>, Option<usize>, bool)> {
    let saved: Option<(i64, Option<i64>)> = c.query_row("SELECT kind, fund_category FROM fund_classifications WHERE user_id = ?1 AND symbol = ?2",
                                                        rusqlite::params![user, symbol], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if let Some((kind, category)) = saved {
        let bad = || FiguresError::NotComputed("an unknown fund classification".into());
        let kind = usize::try_from(kind).ok().filter(|k| *k < KINDS.len()).ok_or_else(bad)?;
        let category = category.map(|k| usize::try_from(k).ok().filter(|k| *k < CATEGORIES.len()).ok_or_else(bad)).transpose()?;
        return Ok((Some(kind), category, true));
    }
    let mut s = c.prepare_cached("SELECT instrument_type FROM assets WHERE symbol = ?1 ORDER BY id")?;
    let types: Vec<Option<String>> = s.query_map([symbol], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let chosen = types.iter().find(|t| matches!(t.as_deref(), Some("stock" | "etf"))).or(types.first()).cloned().flatten();
    Ok(match chosen.as_deref() { Some("etf") => (Some(1), Some(4), false), Some("stock") => (Some(0), None, false), _ => (None, None, false) })
}

/// The windows of closes the report of `year` will ask for (the job fetches them first).
pub fn needs(c: &Connection, user: i64, year: i32) -> R<Vec<Window>> {
    let Some(exchange) = broker_exchange(c, user)? else { return Ok(vec![]) };
    let closes = Closes::Needs(RefCell::new(BTreeSet::new()));
    let mut walk = Walk::new(c, user, exchange, year, &closes)?;
    walk.run()?;
    drop(walk);
    let Closes::Needs(asked) = closes else { return Ok(vec![]) };
    Ok(asked.into_inner().into_iter().collect())
}

/// `classification_rows` (D-TAX-BROKER-PANEL): the panel's to-do list, from the same walk run on what is stored, so it
/// shows every refusal the report would make but a missing close or ECB rate (the run fetches both first).
pub fn classification_rows(c: &Connection, user: i64, exchange: i64, year: i32) -> R<Vec<SymbolRow>> {
    let closes = Closes::Panel(c);
    let mut walk = Walk::new(c, user, exchange, year, &closes)?;
    walk.run()?;
    Ok(walk.symbol_rows())
}

/// Tax::BrokerReport#to_csv, in German (the form's language), from the stored closes but the `failed` windows.
pub fn report(c: &Connection, user: i64, year: i32, failed: &HashSet<Window>) -> R<String> {
    let exchange = broker_exchange(c, user)?.ok_or_else(|| FiguresError::NotComputed("no Alpaca broker ledger".into()))?;
    let name: String = c.query_row("SELECT name FROM exchanges WHERE id = ?1", [exchange], |r| r.get(0))?;
    let closes = Closes::Stored { c, failed };
    let mut walk = Walk::new(c, user, exchange, year, &closes)?;
    walk.run()?;
    let unclassified: Vec<String> = walk.states.values().filter(|s| s.kind.is_none()).map(|s| s.symbol.clone()).collect();
    let summaries = unclassified.is_empty();
    let kap = if summaries { Some(walk.kap()?) } else { None };
    let inv = if summaries { Some(walk.kap_inv()?) } else { None };
    let complete = walk.warnings.is_empty();
    csv(&walk, &name, kap.as_ref(), inv.as_ref(), &unclassified, complete)
}

// ---- the CSV (Tax::BrokerReportCsv) ------------------------------------------------------------------------------------

fn t(key: &str) -> String { crate::web::i18n::text("de", key, &[]) }
fn s(v: impl Into<String>) -> Cell { Cell::Text(v.into()) }
fn eur(v: Option<&Dec>) -> R<Cell> { Ok(match v { Some(v) => Cell::Raw(v.round(2)?.to_s_f()), None => Cell::Nil }) }
fn raw(v: Option<&Dec>) -> Cell { v.map_or(Cell::Nil, |v| Cell::Raw(v.to_s_f())) }
fn opt(v: Option<String>) -> Cell { v.map_or(Cell::Nil, Cell::Text) }

fn warning_text(w: &Warning) -> String {
    let key = format!("tax_report.broker.warnings.{}", w.code);
    let detail = w.detail.text();
    crate::web::i18n::text_chain("de", &[key], &[("detail", crate::web::i18n::Arg::Text(&detail))])
        .unwrap_or_else(|| crate::web::i18n::text("de", "tax_report.broker.warnings.unknown", &[("code", crate::web::i18n::Arg::Text(w.code)), ("detail", crate::web::i18n::Arg::Text(&detail))]))
}

/// D-TAX-BROKER-DISCLOSURES: the legal notes, in order.
pub const DISCLOSURES: [&str; 9] = ["tax_report.broker.disclosures.not_tax_advice", "tax_report.broker.disclosures.usd_cash_fx",
    "tax_report.broker.disclosures.us_etf_default", "tax_report.broker.disclosures.fx_source", "tax_report.broker.disclosures.withholding_credit",
    "tax_report.broker.disclosures.return_of_capital", "tax_report.broker.disclosures.vorabpauschale_distributions", "tax_report.broker.disclosures.classification_history",
    "tax_report.broker.disclosures.regulatory_fees"];

fn csv(w: &Walk, exchange: &str, kap: Option<&Kap>, inv: Option<&BTreeMap<usize, Inv>>, unclassified: &[String], complete: bool) -> R<String> {
    let mut out = String::new();
    let o = &mut out;
    let prefix = t("tax_report.incomplete_banner_prefix");
    line(o, &[s(t("tax_report.broker.title")), s(exchange), Cell::Raw(w.year.to_string())]);
    if kap.is_none() { line(o, &[s(format!("{prefix}: {}", t("tax_report.broker.unclassified_banner")))]); }
    if !complete { line(o, &[s(format!("{prefix}: {}", t("tax_report.broker.incomplete_banner")))]); }
    if let Some(k) = kap {
        line(o, &[]);
        line(o, &[s(t("tax_report.broker.sections.kap"))]);
        line(o, &[s(t("tax_report.broker.headers.zeile")), s(t("tax_report.broker.headers.description")), s(t("tax_report.broker.headers.amount_eur"))]);
        for (zeile, key, v) in [("19", "z19", &k.z19), ("20", "z20", &k.z20), ("22", "z22", &k.z22), ("23", "z23", &k.z23), ("41", "z41", &k.z41)] {
            line(o, &[s(zeile), s(t(&format!("tax_report.broker.kap.{key}"))), eur(Some(v))?]);
        }
    }
    if let Some(inv) = inv.filter(|i| !i.is_empty()) {
        line(o, &[]);
        line(o, &[s(t("tax_report.broker.sections.kap_inv"))]);
        line(o, &[s(t("tax_report.broker.headers.zeile")), s(t("tax_report.broker.headers.fund_category")), s(t("tax_report.broker.headers.position")),
                  s(t("tax_report.broker.headers.amount_eur"))]);
        for (category, (z_dist, z_vap, z_sale)) in ZEILEN.iter().enumerate() {
            let Some(v) = inv.get(&category) else { continue };
            let name = t(&format!("tax_report.broker.fund_categories.{}", CATEGORIES[category]));
            line(o, &[s(*z_dist), s(name.clone()), s(t("tax_report.broker.kap_inv.distributions")), eur(Some(&v.distributions))?]);
            // D-TAX-BROKER-2023-LINES: KAP-INV 2023 has no Vorabpauschale lines (the 2022 Basiszins was negative).
            if w.year != 2023 { line(o, &[s(*z_vap), s(name.clone()), s(t("tax_report.broker.kap_inv.vorabpauschale")), eur(Some(&v.vap))?]); }
            line(o, &[s(*z_sale), s(name), s(t("tax_report.broker.kap_inv.sale_result")), eur(Some(&v.sale))?]);
        }
    }
    if !w.disposals.is_empty() {
        line(o, &[]);
        line(o, &[s(t("tax_report.broker.sections.disposals"))]);
        let headers = ["symbol", "kind", "fund_category", "date", "units", "proceeds_usd", "fx_rate", "proceeds_eur", "fee_eur", "cost_eur", "vorabpauschale_eur", "gain_eur", "status"];
        line(o, &headers.iter().map(|h| s(t(&format!("tax_report.broker.headers.{h}")))).collect::<Vec<_>>());
        for d in &w.disposals {
            line(o, &[s(d.symbol.clone()), opt(d.kind.map(|k| t(&format!("tax_report.broker.kinds.{}", KINDS[k])))),
                      opt(d.category.map(|k| t(&format!("tax_report.broker.fund_categories.{}", CATEGORIES[k])))), s(d.sold_on.to_string()), raw(Some(&d.units)),
                      eur(Some(&d.proceeds_usd))?, raw(d.fx.as_ref()), eur(d.proceeds_eur.as_ref())?, eur(d.fee_eur.as_ref())?, eur(Some(&d.cost))?,
                      eur(Some(&d.vap))?, eur(d.gain.as_ref())?, if d.refused { s(t("tax_report.broker.status.refused")) } else { Cell::Nil }]);
            for m in &d.matches {
                line(o, &[Cell::Nil, s(t("tax_report.broker.rows.lot")), Cell::Nil, opt(m.acquired.map(|a| a.to_string())), raw(Some(&m.units)), Cell::Nil,
                          raw(m.fx.as_ref()), Cell::Nil, Cell::Nil, eur(Some(&m.cost))?, eur(Some(&m.vap))?, Cell::Nil,
                          if m.uncovered { s(t("tax_report.broker.status.uncovered")) } else { Cell::Nil }]);
            }
        }
    }
    if !w.income.is_empty() || !w.fee_total.is_zero() {
        line(o, &[]);
        line(o, &[s(t("tax_report.broker.sections.income"))]);
        let headers = ["date", "symbol", "activity_type", "bucket", "amount_usd", "fx_rate", "amount_eur_income", "status"];
        line(o, &headers.iter().map(|h| s(t(&format!("tax_report.broker.headers.{h}")))).collect::<Vec<_>>());
        for i in &w.income {
            line(o, &[s(i.date.to_string()), opt(i.symbol.clone()), s(i.activity.clone()), s(t(&format!("tax_report.broker.buckets.{}", i.bucket))),
                      eur(i.usd.as_ref())?, raw(i.fx.as_ref()), eur(i.eur.as_ref())?, if i.refused { s(t("tax_report.broker.status.refused")) } else { Cell::Nil }]);
        }
        if !w.fee_total.is_zero() {
            line(o, &[Cell::Nil, Cell::Nil, s(t("tax_report.broker.rows.fee_total")), Cell::Nil, Cell::Nil, Cell::Nil, eur(Some(&w.fee_total))?, Cell::Nil]);
        }
    }
    if !unclassified.is_empty() || !w.warnings.is_empty() {
        line(o, &[]);
        line(o, &[s(t("tax_report.broker.sections.warnings"))]);
        if !unclassified.is_empty() {
            line(o, &[s(t("tax_report.broker.warnings.summaries_blocked"))]);
            for symbol in unclassified { line(o, &[s(symbol.clone()), s(t("tax_report.broker.warnings.unclassified"))]); }
        }
        for warning in &w.warnings { line(o, &[opt(warning.symbol.clone()), s(warning_text(warning))]); }
    }
    line(o, &[]);
    line(o, &[s(t("tax_report.broker.sections.disclosures"))]);
    for key in DISCLOSURES { line(o, &[s(t(key))]); }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Dec { d(s) }
    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn the_basiszins_table_is_the_bmf_s() {
        assert_eq!(basiszins(2024).unwrap().to_s_f(), "0.0229");
        assert_eq!(basiszins(2026).unwrap().to_s_f(), "0.032");
        assert!(basiszins(2017).is_none() && basiszins(2027).is_none());
    }

    // Rails' vorabpauschale_test, with D-TAX-BROKER-VAP-MONTHS.
    #[test]
    fn a_full_year_lot_is_capped_by_the_basisertrag_and_the_mehrbetrag() {
        // 1,000 × 0.7 × 2.29 % = 16.03; Mehrbetrag 100 does not bite.
        assert_eq!(vorabpauschale(2024, day("2023-05-01"), &n("1000"), &n("1100"), &n("0")).unwrap().unwrap(), n("16.03"));
        // Mehrbetrag 5 bites.
        assert_eq!(vorabpauschale(2024, day("2023-05-01"), &n("1000"), &n("1005"), &n("0")).unwrap().unwrap(), n("5"));
        // Distributions above the Basisertrag leave none; a fall leaves none; a negative Basiszins none.
        assert!(vorabpauschale(2024, day("2023-05-01"), &n("1000"), &n("1100"), &n("20")).unwrap().unwrap().is_zero());
        assert!(vorabpauschale(2024, day("2023-05-01"), &n("1000"), &n("900"), &n("0")).unwrap().unwrap().is_zero());
        assert!(vorabpauschale(2022, day("2021-05-01"), &n("1000"), &n("1100"), &n("0")).unwrap().unwrap().is_zero());
        assert!(vorabpauschale(2027, day("2026-05-01"), &n("1000"), &n("1100"), &n("0")).unwrap().is_none());
    }

    #[test]
    fn the_purchase_year_s_twelfths_come_off_the_vorabpauschale_itself() {
        // April: three full months before. min(16.03, 10) × 9/12 = 7.5 (Rails: min(12.0225, 10) = 10).
        assert_eq!(vorabpauschale(2024, day("2024-04-15"), &n("1000"), &n("1010"), &n("0")).unwrap().unwrap(), n("7.5"));
        // December: one twelfth. 16.03 / 12.
        assert_eq!(vorabpauschale(2024, day("2024-12-01"), &n("1000"), &n("1100"), &n("0")).unwrap().unwrap(), div(&n("16.03"), &n("12")).unwrap());
        // January: whole.
        assert_eq!(vorabpauschale(2024, day("2024-01-02"), &n("1000"), &n("1100"), &n("0")).unwrap().unwrap(), n("16.03"));
    }

    // Rails' lot_ledger_test.
    #[test]
    fn a_disposal_spans_lots_first_in_first_out_with_their_vap() {
        let mut l = Ledger::default();
        l.acquire(n("10"), n("100"), day("2024-01-01"), None);
        l.acquire(n("10"), n("300"), day("2024-02-01"), None);
        l.lots[1].vap_per_unit = n("2");
        let m = l.dispose(&n("15")).unwrap();
        assert_eq!((m[0].units.clone(), m[0].cost.clone(), m[1].units.clone(), m[1].cost.clone(), m[1].vap.clone()), (n("10"), n("100"), n("5"), n("150"), n("10")));
        assert_eq!((l.lots[0].units.clone(), l.lots[0].cost.clone()), (n("5"), n("150")));
        let over = l.dispose(&n("8")).unwrap();
        assert!(over[1].uncovered && over[1].units == n("3") && l.lots.is_empty());
    }

    /// An install with the Rails schema, an Alpaca user (id 1, venue 1) and ECB dollar rates of 1.08 through 2023 and 2024.
    fn install() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../tests/fixtures/settings_primary_schema.sql")).unwrap();
        c.execute_batch("PRAGMA foreign_keys = OFF;
            INSERT INTO users (id, email, created_at, updated_at, time_zone) VALUES (1, 'o@example.com', '2026-01-01', '2026-01-01', 'UTC');
            INSERT INTO exchanges (id, type, name, created_at, updated_at) VALUES (1, 'Exchanges::Alpaca', 'Alpaca', '2026-01-01', '2026-01-01');
            INSERT INTO assets (external_id, symbol, name, category, instrument_type, created_at, updated_at) VALUES ('FNDX.XNAS', 'FNDX', 'FNDX', 'ETF', 'etf', '2026-01-01', '2026-01-01');").unwrap();
        let mut day = ymd(2023, 1, 1);
        while day <= ymd(2025, 12, 31) {
            c.execute("INSERT INTO fx_rates (currency, date, rate) VALUES ('USD', ?1, '1.08')", [day.to_string()]).unwrap();
            day = day.succ_opt().unwrap();
        }
        c
    }

    fn row(c: &Connection, kind: i64, amount: &str, usd: &str, at: &str) {
        c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, transacted_at, raw_data, created_at, updated_at) \
                   VALUES (1, 1, ?1, 'FNDX', ?2, 'USD', ?3, ?4, '{}', '2026-01-01', '2026-01-01')", rusqlite::params![kind, amount, usd, at]).unwrap();
    }

    /// D-TAX-BROKER-VAP-START: 2 January 2023 was an NYSE holiday; the 3rd is the year's first close and makes the window fresh.
    #[test]
    fn the_year_s_first_close_may_follow_a_holiday() {
        let w = Window { symbol: "FNDX".into(), from: ymd(2023, 1, 2), to: ymd(2023, 1, 10), first: true };
        assert_eq!(w.fresh(), (ymd(2023, 1, 2), ymd(2023, 1, 2)));
        let c = install();
        c.execute_batch("INSERT INTO historical_prices (asset, currency, date, price) VALUES ('stock:FNDX', 'USD', '2023-01-03', '108'), ('stock:FNDX', 'USD', '2023-01-09', '120')").unwrap();
        let fx = FxTable::load(&c).unwrap();
        let failed = HashSet::new();
        let closes = Closes::Stored { c: &c, failed: &failed };
        assert!(matches!(closes.pick(&fx, w.clone()).unwrap(), Pick::Close(day, eur) if day == ymd(2023, 1, 3) && eur == n("100")));
        // Only a later close stored: no start value.
        c.execute_batch("DELETE FROM historical_prices WHERE date = '2023-01-03'").unwrap();
        assert!(matches!(closes.pick(&fx, w).unwrap(), Pick::Nothing));
    }

    /// D-TAX-BROKER-PANEL: the panel's rows carry the walk's refusals but a missing close, and the walk asks for nothing.
    #[test]
    fn the_panel_shows_the_walk_s_refusals_but_a_missing_close() {
        let c = install();
        row(&c, BUY, "100", "10800", "2023-03-01 15:00:00");
        row(&c, SELL, "150", "16200", "2025-03-03 15:00:00");
        let before: i64 = c.query_row("SELECT count(*) FROM historical_prices", [], |r| r.get(0)).unwrap();
        let rows = classification_rows(&c, 1, 1, LAST_YEAR).unwrap();
        assert_eq!(rows, vec![SymbolRow { symbol: "FNDX".into(), kind: Some(1), category: Some(4), persisted: false, reasons: vec!["uncovered_disposal"] }]);
        assert_eq!(c.query_row("SELECT count(*) FROM historical_prices", [], |r| r.get::<_, i64>(0)).unwrap(), before);
        // The report itself refuses the missing closes too.
        let csv = report(&c, 1, LAST_YEAR, &HashSet::new()).unwrap();
        assert!(csv.contains("Kein Rücknahmepreis zum Jahreswechsel 2023") && csv.contains("ohne vollständig belegten Bestand"), "{csv}");
    }

    #[test]
    fn a_split_restates_units_and_keeps_cost_and_vap_per_lot() {
        let mut l = Ledger::default();
        l.acquire(n("10"), n("1000"), day("2023-03-01"), None);
        l.lots[0].vap_per_unit = n("4");
        l.adjust(&n("30"), day("2024-06-03")).unwrap();
        assert_eq!((l.lots[0].units.clone(), l.lots[0].cost.clone(), l.lots[0].vap_per_unit.clone()), (n("40"), n("1000"), n("1")));
        assert_eq!(l.units_in_terms_of(&l.lots[0], day("2024-01-01")).unwrap(), n("10"));
        assert_eq!(l.units_in_terms_of(&l.lots[0], day("2024-12-31")).unwrap(), n("40"));
        // A degenerate adjustment is no split.
        l.adjust(&n("-40"), day("2024-07-01")).unwrap();
        assert_eq!(l.splits.len(), 1);
    }
}
