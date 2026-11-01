//! Tax reports (roadmap stage F): Tax::GenerateReportJob#crypto_csv and Tax::Report#to_csv over every venue the user
//! has, for the jurisdictions `jurisdictions::PORTED` names (US DE ES PT SK PL). Byte for byte Rails' file where Rails is
//! right; where it is not, a named divergence (the engine's own are listed in fifo.rs):
//! - D-TAX-UNCOVERED: a sale of more than the history holds is incomplete, counted in the incomplete banner, and listed
//!   (`tax_report.warnings.uncovered_sales`).
//! - D-TAX-UNLINKED-DISCLOSED: coin withdrawals not linked to a deposit keep their lots (as Rails) and are listed.
//! - D-TAX-LOST: lost coins leave the lots with no disposal, and are listed (`tax_report.warnings.lost`).
//! - D-TAX-FEES-DISCLOSED: every coin fee of a row of its own is named in one NOTE block (`tax_report.warnings.coin_fees`),
//!   which replaces Rails' block of the Alpaca fees whose calendar day is ambiguous; such an Alpaca fee is walked after
//!   its day's fills (`walk_standing_cfees_after_their_fills`).
//! - D-TAX-STABLE-CASH: where swaps are not taxable and the user treats stablecoins as fiat, a stablecoin row is cash
//!   and reaches no engine (Rails sold the stablecoins again, at a zero basis, after the coin that bought them was sold).
//! - D-TAX-COIN-QUOTE: a coin-for-coin trade booked as one row quoted in a coin is read as its two swap legs
//!   (`rows::split_coin_quoted_trades`, the tracker's own reading, D-TRACKER-COIN-QUOTE), so the quote coin moves too.
//! - D-TAX-STABLE-VALUE: a stablecoin bought or sold for fiat in a two-row group (Kraken) is valued at the fiat that
//!   changed hands, not the dollar peg (`quote_stablecoin_legs`); Portugal's immediate fiat exit follows.
//! - D-TAX-LOCAL-YEAR: income and the excess-ROC refusal by the taxpayer's year, as disposals (Rails: the UTC year).
//! - D-TAX-ORPHAN-SWAP: where swaps are not taxable, swap legs with nothing going the other way are listed.
//! - D-TAX-SK-SWAPS: Slovakia taxes a coin exchanged for a coin as a sale (`jurisdictions::REGISTRY`).
//! - D-TAX-DE-WALLET: Germany keeps lots per wallet (one per venue) from the first row, as the US does from 2025.
//! - D-TAX-US-WALLET: US tax years from 2025, lots per account; a placement at 1 January 2025 that was a choice is listed
//!   lot by lot (`tax_report.warnings.wallet_allocation`), as this report's reconstruction.
//! - Austria (the engine's own in fifo.rs): D-TAX-AT-WALLET, lots per wallet (one per venue) from the first row (VO § 2
//!   Abs. 1, § 6 Z 2); D-TAX-AT-STABLECOIN, a stablecoin is crypto (Rz 6178b), the flag is not read; D-TAX-AT-REWARDS,
//!   staking and airdrops leave the income section and are listed (`zero_cost_rewards`); D-TAX-AT-MONTHLY-VALUE, income
//!   paid more than three times a month valued at the 1st's close (VO § 4 Abs. 2); D-TAX-AT-INCOME-1-MARCH-2022, income
//!   before § 27b listed apart (`income_before_regime`); D-TAX-AT-SPECULATION, § 31 sales listed apart (`speculation`).
//! - Poland is no FIFO: its PIT-38 workpaper and its divergences are pl.rs's (D-TAX-PL-*), its NBP rates nbp.rs's.
//! - Bulgaria (bg.rs): D-TAX-BG-AVERAGE, D-TAX-BG-CURRENCY, D-TAX-BG-ANNUAL, D-TAX-BG-DEPOSIT-ZERO. The Czech Republic
//!   (cz.rs, its ČNB rates cnb.rs's): D-TAX-CZ-CNB, D-TAX-CZ-EMT, D-TAX-CZ-CAP, D-TAX-CZ-PRILOHA2, D-TAX-CZ-MINING
//!   (mining at cost 0, no income), D-TAX-CZ-SWAP-FEE (a swap's in-leg fee is an expense of the disposal). Both:
//!   D-TAX-SWAP-RECEIVED (`value_groups`): a swap's proceeds are the value received.
//! - Denmark (SL § 5 a, PSL § 3, DJV C.C.2.1.3.3.5): D-TAX-DK-PER-PART, a row per FIFO part (`fifo::Options::row_per_lot`);
//!   D-TAX-DK-NO-WASH-SALE, no loss denied and no `loss_denied` column; D-TAX-DK-RUBRIK, rubrik 20 and 58 in place of Rails'
//!   per-asset lines; D-TAX-DK-SWAP-VALUE, one value for both sides of an exchange (`value_groups`); D-TAX-DK-DEPOSIT-ZERO,
//!   an unlinked deposit at cost 0; D-TAX-DK-STABLECOIN, USDT, USDC and USDP out of the rubrik totals, listed.
//! - Greece: D-TAX-GR-NO-TAX, no tax on an individual's crypto gains is in force up to 2026 (no tax line, a NOTE).
//! - Italy (it.rs; LIFO as Rails, the engine's D-TAX-IT-FEES and D-TAX-IT-PRE2023-BASIS): D-TAX-IT-SWAPS, D-TAX-IT-DEPOSIT-ZERO,
//!   D-TAX-IT-THRESHOLD, D-TAX-IT-EMT-RATE, D-TAX-IT-RT, D-TAX-IT-INCOME.
//! - Ireland (ie.rs; the engine's s. 581 in fifo.rs): D-TAX-IE-4WEEK (`matching_rule`), D-TAX-IE-RING-FENCE (with the 28
//!   days after the year walked only to fill fences), D-TAX-IE-RATE, D-TAX-IE-PERIODS, D-TAX-IE-SUMMARY, D-TAX-IE-YEARS;
//!   D-TAX-SWAP-RECEIVED as BG and CZ.
//!
//! The report is read (`prepare`, on a connection) apart from its enrichment (`Report::compute`, with no connection held):
//! the job fetches the prices the table lacks between the enrichment's steps, as the tracker's pass does.
pub mod api;
mod bg;
pub mod cnb;
mod cz;
pub mod fifo;
pub mod job;
pub mod jurisdictions;
mod it;
mod ie;
pub mod nbp;
pub mod pl;

use crate::figures::{at::At, budget, dec::Dec, FiguresError};
use crate::tracker::prices::{self, alias_coin, catalogue_coin, Fetcher, PriceService, Reference, Unfetched, Venue};
use crate::tracker::rows::{self, Kind, Stored};
use crate::tracker::{fiat, stable, walk};
use crate::web::i18n::{text, Arg};
use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use jurisdictions::Jurisdiction;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

/// What the job leaves for the user: the report, or Rails' refusal (`<path>.refused.json`).
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome { Csv(String), Refused { reason: String, symbols: Vec<String> } }

/// Tax::Report::INCOME_TYPES, by entry type.
const INCOME: [(Kind, &str); 5] = [(Kind::StakingReward, "staking_reward"), (Kind::LendingInterest, "lending_interest"),
                                   (Kind::Airdrop, "airdrop"), (Kind::Mining, "mining"), (Kind::OtherIncome, "other_income")];

/// Tax::GenerateReportJob#perform(user, country, year, stablecoin_as_fiat) for the crypto scope, in one budget, with a
/// provider that has no price (`Unfetched`): what the parity vectors record, where Rails' market data answers every
/// range with a failure.
pub fn crypto_report(c: &Connection, user_id: i64, country: &str, year: i32, stablecoin_as_fiat: bool, today: NaiveDate) -> Result<Outcome, FiguresError> {
    budget::within(|| match prepare(c, user_id, country, year, stablecoin_as_fiat)? {
        Prepared::Done(outcome) => Ok(outcome),
        Prepared::Report(report) => report.compute(&mut Unfetched, today, &mut |_| {}),
    })
}

/// What the report reads from the database, read before any price is fetched, or the answer already known.
pub enum Prepared { Done(Outcome), Report(Box<Report>) }

/// Every input of one report.
pub struct Report {
    j: &'static Jurisdiction, year: i32, zone: String, stablecoin_as_fiat: bool, banners: Vec<String>, scoped: Vec<Stored>,
    deposits: HashMap<i64, Dec>, reference: Reference, fx: crate::tracker::fx::FxTable, cfee: HashSet<i64>,
    /// Poland's NBP rates (D-TAX-PL-NBP); None elsewhere.
    nbp: Option<nbp::Nbp>,
    /// Poland: column d as the user declared it, by year up to the report's (D-TAX-PL-DECLARED-CARRY).
    carry: std::collections::BTreeMap<i32, Dec>,
    /// The Czech Republic's ČNB fixings (D-TAX-CZ-CNB); None elsewhere.
    cnb: Option<cnb::Cnb>,
    /// D-TAX-IE-RING-FENCE (r1 item 4): Ireland's rows of the 28 days after the year, which only fill the year's fences.
    ahead: Vec<Stored>,
}

/// The reads of `crypto_csv` up to the enrichment: the scope, the tokenized refusal, the banners.
pub fn prepare(c: &Connection, user_id: i64, country: &str, year: i32, stablecoin_as_fiat: bool) -> Result<Prepared, FiguresError> {
    let j = jurisdictions::find(country).filter(|_| jurisdictions::ported(country, i64::from(year)))
        .ok_or_else(|| FiguresError::NotComputed(format!("the tax report is not ported for the jurisdiction {country}")))?;
    // D-TAX-STABLECOIN-QUESTION: the flag is read only where the export form asks it (`stablecoin_ambiguous`), and no
    // served country asks it (D-TAX-AT-STABLECOIN, D-TAX-SK-SWAPS). Rails' form sends a hidden box's value with every
    // request, and REST and MCP take it for any country; this one gate makes every surface compute what the web shows.
    let stablecoin_as_fiat = stablecoin_as_fiat && j.stablecoin_ambiguous;
    let all = rows::load_all(c, user_id)?;
    let deposits: HashMap<i64, Dec> = all.iter().map(|r| (r.id, r.amount.clone())).collect();
    // GenerateReportJob#crypto_transactions: every row of a crypto venue; of a stock venue, those of a crypto symbol.
    let stock: HashSet<i64> = {
        let mut s = c.prepare("SELECT id, type FROM exchanges")?;
        let kinds = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        kinds.into_iter().filter(|(_, t)| t.as_deref().is_some_and(|t| prices::STOCK_TYPES.contains(&t))).map(|(id, _)| id).collect()
    };
    let scope = CryptoScope::new(c, user_id)?;
    let mut rows = vec![];
    for r in all { if !stock.contains(&r.exchange_id) || scope.crypto(c, &r.base)? { rows.push(r); } }

    let zone: String = c.query_row("SELECT time_zone FROM users WHERE id = ?1", [user_id], |r| r.get(0))?;
    // Tax::Report#rows_until: the end of the year in the user's zone, or in UTC where that is later.
    let utc_end = Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).single();
    let local_end = local_midnight(&zone, year + 1);
    let rows_until = utc_end.and_then(At::from_utc).zip(local_end).map(|(u, l)| if l.0 > u.0 { l } else { u })
        .ok_or_else(|| FiguresError::Data("a report year out of range".into()))?;
    let tokenized = tokenized_symbols(c, rows.iter().filter(|r| r.at.0 < rows_until.0))?;
    if !tokenized.is_empty() { return Ok(Prepared::Done(Outcome::Refused { reason: "tokenized_unsupported".into(), symbols: tokenized })); }
    let banners = sync_issues(c, user_id, j.locale)?;

    // D-TAX-COIN-QUOTE after ordering, so the two legs keep the row's place (a negated id must not anchor the group).
    let stable_cash = !j.swaps_taxable() && stablecoin_as_fiat;
    // D-TAX-IE-RING-FENCE: a loss of 31 December is restricted by a purchase up to 28 January (the window's last day).
    let ahead_until = if j.code == "IE" { local_start(&zone, (year + 1, 1, 29)) } else { None };
    // D-TAX-IE-RING-FENCE: in Ireland a row at the year's very end (midnight) is the next year's, read only to fill fences.
    // D-TAX-IE-RING-FENCE: in Ireland the year ends at the taxpayer's midnight (east of UTC too, before the UTC end); a row
    // from that instant is the next year's, read only to fill fences.
    let ie_end = local_end.filter(|_| j.code == "IE");
    let in_year = |r: &Stored| match ie_end { Some(end) => r.at.0 < end.0, None => r.at.0 <= rows_until.0 };
    let (year_rows, later): (Vec<Stored>, Vec<Stored>) = rows.into_iter().partition(in_year);
    let mut scoped = rows::ordered_legs(year_rows, stable_cash)?;
    quote_stablecoin_legs(&mut scoped)?;
    let mut ahead = rows::ordered_legs(later.into_iter().filter(|r| ahead_until.is_some_and(|u| r.at.0 < u.0)).collect(), stable_cash)?;
    quote_stablecoin_legs(&mut ahead)?;
    let mut symbols = prices::symbols(&scoped);
    symbols.extend(prices::symbols(&ahead));
    let mut reference = Reference::load(c, &Venue::alpaca(c)?, &symbols)?;
    reference.record_coins(c, user_id)?;
    let fx = crate::tracker::fx::FxTable::load(c)?;
    let mut s = c.prepare("SELECT id FROM account_transactions WHERE json_extract(raw_data, '$.activity_type') = 'CFEE'")?;
    let cfee: HashSet<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let nbp = if j.code == "PL" { Some(nbp::Nbp::load(c)?) } else { None };
    let carry = if j.code == "PL" { pl::declared(c, user_id, year)? } else { Default::default() };
    let cnb = if j.code == "CZ" { Some(cnb::Cnb::load(c)?) } else { None };
    Ok(Prepared::Report(Box::new(Report { j, year, zone, stablecoin_as_fiat, banners, scoped, deposits, reference, fx, cfee, nbp, carry, cnb, ahead })))
}

/// Midnight starting 1 January of `year` in the user's zone (`ActiveSupport::TimeZone#local`).
fn local_midnight(zone: &str, year: i32) -> Option<At> { local_start(zone, (year, 1, 1)) }

/// Midnight starting a day in the user's zone.
fn local_start(zone: &str, (y, m, d): (i32, u32, u32)) -> Option<At> {
    crate::web::timezone::strict(zone).unwrap_or(chrono_tz::Tz::UTC).with_ymd_and_hms(y, m, d, 0, 0, 0).earliest()
        .and_then(|t| At::from_utc(t.with_timezone(&Utc)))
}

/// § 124b Z 384 lit. a: § 27b EStG applies from 1 March 2022.
const AT_REGIME_FROM: (i32, u32, u32) = (2022, 3, 1);
/// KryptowährungsVO § 7, Rz 6178aa: the moving average from 1 January 2023; Rz 6178p: VO § 4 Abs. 2 from then too.
const AT_AVERAGE_FROM: (i32, u32, u32) = (2023, 1, 1);

/// What a report states beside the disposals: Austria's § 27b income and the rest listed apart; Bulgaria's and the Czech
/// Republic's summaries and notes.
#[derive(Default)]
struct AtNotes { speculation: Vec<(String, Dec)>, before_regime: Vec<(String, Dec)>, zero_cost: Vec<String>, monthly: Vec<String>, averaged: bool,
                 /// D-TAX-SWAP-FEE-EXHAUSTED (any country where swaps are not taxable): the swap-ins of the year and their basis.
                 exhausted: Vec<(String, Dec)>,
                 /// The summary block after the rows (D-TAX-BG-ANNUAL, D-TAX-CZ-PRILOHA2), as CSV lines.
                 summary: String,
                 /// D-TAX-CZ-PRILOHA2: per row, (tax_exempt, exempt_reason).
                 exempt: Vec<(bool, Option<String>)>,
                 /// § 38v: the exempt receipts above 5,000,000 CZK.
                 notice: Vec<(String, Dec)>,
                 /// D-TAX-CZ-MINING: mining receipts held at cost 0.
                 mined: Vec<String>,
                 /// D-TAX-BG-AVERAGE: a disposal took the average.
                 bg_average: bool,

                 /// D-TAX-DK-SWAP-VALUE: the exchanges valued at one value.
                 swap_values: Vec<String>,
                 /// D-TAX-DK-STABLECOIN: the stablecoins held in the year (from the engine's year-start units and the year's rows).
                 stablecoins: Vec<String>,
                 /// Italy's notes (it.rs).
                 it: it::Notes,
                 /// Ireland's NOTE blocks (D-TAX-IE-RING-FENCE, `ie_provisional`), as CSV lines at the file's end.
                 tail: String }

/// `Report::txs`: the rows kept, whether each was folded away, and what the engine walks.
type Walked = (Vec<Entry>, Vec<bool>, Vec<fifo::Tx>);

/// An enriched row and what the report reads beside it.
#[derive(Clone)]
struct Entry { id: i64, row: walk::Row, tx_id: Option<String> }

impl Report {
    fn local(&self, at: At) -> NaiveDate { crate::web::timezone::local(at.utc(), &self.zone).date_naive() }

    /// Midnight starting the local day `days` after `at`'s (s. 581's windows); `at` itself where out of range.
    fn local_day_start(&self, at: At, days: i64) -> At {
        let day = self.local(at) + chrono::Duration::days(days);
        local_start(&self.zone, (day.year(), day.month(), day.day())).unwrap_or(at)
    }

    /// Poland's NBP rates read again (the job loads them after `prepare`, from `nbp_needs`).
    pub fn reload_nbp(&mut self, c: &Connection) -> Result<(), FiguresError> {
        if self.nbp.is_some() { self.nbp = Some(nbp::Nbp::load(c)?); }
        Ok(())
    }

    /// D-TAX-CZ-CNB: the years whose ČNB file the Czech report reads, from the year of its first value less a week (early
    /// January reads December's fixing) to the report year; none elsewhere.
    pub fn cnb_needs(&self) -> Vec<i32> {
        if self.cnb.is_none() { return vec![]; }
        let Some(first) = self.scoped.iter().map(|r| self.local(r.at)).min() else { return vec![] };
        let from = first.checked_sub_days(chrono::Days::new(7)).unwrap_or(first).year();
        (from..=self.year).collect()
    }

    /// The ČNB rates read again (the job loads them after `prepare`, from `cnb_needs`).
    pub fn reload_cnb(&mut self, c: &Connection) -> Result<(), FiguresError> {
        if self.cnb.is_some() { self.cnb = Some(cnb::Cnb::load(c)?); }
        Ok(())
    }

    /// Tax::Report#to_csv from the enrichment on: prices the table lacks are fetched through `fetcher` (Rails'
    /// `prefetch` and single-day fetches), and `on_progress(percent)` is told as PriceService#enrich tells it.
    pub fn compute(&self, fetcher: &mut dyn Fetcher, today: NaiveDate, on_progress: &mut dyn FnMut(u32)) -> Result<Outcome, FiguresError> {
        if self.j.code == "PL" { return self.pl_compute(fetcher, today); }
        let (j, year) = (self.j, self.year);
        let currency = j.currency(year);
        let mut ps = PriceService::new(&self.reference, &self.fx, today, fetcher);
        // D-TAX-BG-CURRENCY: BNB's peg over the ECB's rates; D-TAX-CZ-CNB: the ČNB fixing of the day.
        let peg = bg::Peg { fx: &self.fx };
        let none = cnb::Cnb::default();
        let daily = cnb::Daily { cnb: self.cnb.as_ref().unwrap_or(&none), zone: &self.zone };
        ps.statutory = match j.code { "BG" => Some(&peg), "CZ" => Some(&daily), _ => None };
        let mut enriched: Vec<Entry> = walk::enrich_with(&self.scoped, &self.deposits, &mut ps, true, currency, on_progress)?.into_iter().zip(&self.scoped)
            .map(|(row, s)| Entry { id: s.id, row, tx_id: s.tx_id.clone() }).collect();
        if !j.swaps_taxable() && !self.stablecoin_as_fiat { swap_coin_for_stablecoin_groups(&mut enriched, j.code == "IT"); }
        // D-TAX-SWAP-RECEIVED: a swap's proceeds are the value received (BG чл. 33 ал. 5; CZ § 3 odst. 2, GFŘ 2022; IE s. 547
        // and TDM 02-01-03 §4, the money's worth received).
        let received = matches!(j.code, "BG" | "CZ" | "IE");
        if received { value_groups(&mut enriched, |_, _| Some(true))?; }
        // D-TAX-IT-SWAPS: an exchange with an e-money token is a disposal at the value received.
        if j.code == "IT" {
            it::value_received(&mut enriched, &self.scoped, &mut ps, currency)?;
            value_groups(&mut enriched, it::emt_side)?;
            it::emt_groups_as_trades(&mut enriched);
        }
        let austria = (j.code == "AT").then(|| self.austria()).transpose()?;
        let mut notes = AtNotes::default();
        if j.code == "CZ" {
            // D-TAX-CZ-MINING: mining is no income when received; the mined units cost 0 (GFŘ 2022 Ad A).
            for e in enriched.iter_mut().filter(|e| e.row.kind == Kind::Mining && !fiat(&e.row.base)) {
                notes.mined.push(format!("{} {} {}", e.row.base, e.row.amount.to_s_f(), date(e.row.at)));
                (e.row.fiat_value, e.row.price_missing, e.row.value_missing) = (Dec::zero(), false, Some(false));
            }
        }
        let dk = j.code == "DK";
        // D-TAX-DK-SWAP-VALUE (SKM2024.218.SKTST): one value for both sides of an exchange.
        if dk { notes.swap_values = value_groups(&mut enriched, dk_value_side)?; }
        if let Some(a) = &austria { notes.monthly = self.value_monthly(&mut enriched, &mut ps, currency, a.average_from)?; }
        // Income of the UTC year, a fiat row converted at the day's rate (Tax::Report#income_value), valued before the
        // engine: its missing rate counts towards the banner.
        let mut income: Vec<(usize, &str, Dec)> = vec![];
        for (i, e) in enriched.iter().enumerate() {
            let Some((kind, name)) = INCOME.iter().find(|(k, _)| *k == e.row.kind) else { continue };
            // D-TAX-CZ-MINING.
            if j.code == "CZ" && *kind == Kind::Mining && !fiat(&e.row.base) { continue; }
            if let Some(a) = austria.as_ref().filter(|_| matches!(kind, Kind::StakingReward | Kind::Airdrop) && !fiat(&e.row.base)) {
                // D-TAX-AT-REWARDS: not income; held at zero, listed (the whole history, as the deposit block).
                if e.row.at.0 >= a.regime_from.0 { notes.zero_cost.push(format!("{} {} {}", e.row.base, e.row.amount.to_s_f(), date(e.row.at))); continue; }
            }
            // D-TAX-LOCAL-YEAR: income by the year the taxpayer was in, as disposals are (Rails: the UTC year).
            if self.local(e.row.at).year() != year { continue; }
            let value = if fiat(&e.row.base) { ps.convert_fiat(&e.row.amount, &e.row.base, currency, e.row.at)? } else { e.row.fiat_value.clone() };
            // D-TAX-AT-INCOME-1-MARCH-2022: before § 27b, no part of the income taxed at 27.5%.
            if austria.as_ref().is_some_and(|a| e.row.at.0 < a.regime_from.0) {
                notes.before_regime.push((format!("{} {} {}", e.row.base, e.row.amount.to_s_f(), date(e.row.at)), value));
                continue;
            }
            income.push((i, name, value));
        }
        let warnings = { let mut seen = HashSet::new(); ps.warnings.iter().filter(|w| seen.insert(w.as_str())).cloned().collect::<Vec<_>>() };
        drop(ps);

        let (taxable, folded, mut txs) = self.txs(&enriched)?;
        let standing_fees: Vec<&walk::Row> = taxable.iter().zip(&folded).filter(|(e, f)| !**f && e.row.kind == Kind::Fee && e.row.amount.is_positive())
            .map(|(e, _)| &e.row).collect();
        // D-TAX-IE-RING-FENCE (r2 item 4, r3 item 3): the 28 days after the year, enriched apart (their warnings dropped,
        // their uids past the year's), walked after the year's rows; their disposals and income never reach the file.
        if !self.ahead.is_empty() {
            let mut ps = PriceService::new(&self.reference, &self.fx, today, fetcher);
            let next = enriched.iter().map(|e| e.row.uid + 1).max().unwrap_or(0);
            let mut ahead: Vec<Entry> = walk::enrich_with(&self.ahead, &self.deposits, &mut ps, true, currency, &mut |_| {})?.into_iter().zip(&self.ahead)
                .map(|(mut row, s)| { row.uid += next; Entry { id: s.id, row, tx_id: s.tx_id.clone() } }).collect();
            if received { value_groups(&mut ahead, |_, _| Some(true))?; }
            txs.extend(self.txs(&ahead)?.2);
        }

        let holding = self.holding_rule();
        // D-TAX-DE-WALLET: Germany keeps the order of use per wallet from the first row (BMF 6 March 2025, Rz. 61).
        // D-TAX-AT-WALLET: Austria's average is per wallet (VO § 2 Abs. 1; an exchange account is one, § 6 Z 2).
        let per_venue_from = match j.code { "US" if year >= 2025 => local_midnight(&self.zone, 2025), "DE" | "AT" => Some(At(i64::MIN)), _ => None };
        // D-TAX-CZ-SWAP-FEE: a fee on a swap's in-leg is an expense of the disposal (§ 10 odst. 5), from the start of time.
        let swap_cost_to_disposed_from = if j.code == "CZ" { Some(At(i64::MIN)) } else { per_venue_from.filter(|_| j.code == "US") };
        // D-TAX-US-WALLET: per venue, a withdrawal to another venue is walked within its instant as soon as its funds allow
        // (`transfers_first`, the tracker's own order), so a same-instant sale on the far venue takes the lots it brought.
        // The pooled years keep Rails' order.
        if let Some(from) = per_venue_from {
            let order = walk::transfers_first(&txs.iter().map(|t| t.row.clone()).collect::<Vec<_>>(), from)?;
            let mut by_uid: HashMap<usize, fifo::Tx> = txs.drain(..).map(|t| (t.row.uid, t)).collect();
            txs.extend(order.iter().filter_map(|r| by_uid.remove(&r.uid)));
        }
        // Italy's regime (L. 197/2022) and Ireland's s. 581 windows, on the taxpayer's calendar.
        let (it, ie) = (j.code == "IT", j.code == "IE");
        let it_from = if it { local_midnight(&self.zone, it::FIRST_YEAR) } else { None };
        let ie_from = |at: At| self.local_day_start(at, -28);
        let ie_until = |at: At| self.local_day_start(at, 29);
        let mut engine = fifo::Fifo::new(fifo::Options {
            swaps_taxable: j.swaps_taxable(), stablecoin_as_fiat: self.stablecoin_as_fiat,
            holding: holding.as_ref().map(|h| h as &dyn Fn(At, At) -> bool), swap_resets_holding_period: j.swap_resets_holding_period, per_venue_from,
            swap_cost_to_disposed_from, austria,
            // D-TAX-BG-AVERAGE, D-TAX-BG-DEPOSIT-ZERO; D-TAX-DK-PER-PART, D-TAX-DK-DEPOSIT-ZERO, Denmark's mining provenance
            // and year-start units.
            average_all: j.code == "BG", row_per_lot: dk, deposit_cost_zero: j.code == "BG" || dk || it, tag_mining: dk,
            // D-TAX-IT-DEPOSIT-ZERO (art. 68 c. 9-bis); the year-start units for Italy's stablecoin NOTE too.
            snapshot_at: if dk || it { local_midnight(&self.zone, year) } else { None },
            // D-TAX-IT-FEES, D-TAX-IT-PRE2023-BASIS; LIFO as Rails.
            lifo: it, fees_ignored_from: it_from, swaps_taxable_before: it_from,
            ireland: ie.then_some(fifo::Ireland { from: &ie_from, until: &ie_until }),
        });
        engine.calculate(&txs)?;
        // Tax::Report#refuse_excess_return_of_capital: this year's only (D-TAX-LOCAL-YEAR: the taxpayer's year).
        let mut symbols: Vec<String> = engine.excess_roc_rows.iter().map(|&i| &txs[i].row).filter(|r| self.local(r.at).year() == year).map(|r| r.base.clone()).collect();
        symbols.sort();
        symbols.dedup();
        if !symbols.is_empty() { return Ok(Outcome::Refused { reason: "excess_return_of_capital".into(), symbols }); }
        let mut results: Vec<fifo::Disposal> = engine.disposals.into_iter().filter(|d| self.local(d.at).year() == year).collect();
        if matches!(j.code, "BG" | "CZ") {
            // Each printed row reconciles in cents (proceeds − cost − fee = gain), and the summary adds the printed figures.
            for d in &mut results {
                (d.proceeds, d.cost_basis, d.fee) = (d.proceeds.round(2)?, d.cost_basis.round(2)?, d.fee.round(2)?);
                d.gain_loss = (&(&d.proceeds - &d.cost_basis)? - &d.fee)?;
            }
        }
        if j.code == "BG" {
            bg::annual(&mut notes.summary, j.locale, year, &results)?;
            notes.bg_average = !results.is_empty();
        }
        if it { (notes.summary, notes.it) = self.it_year(&mut results, &txs, &enriched, &income, engine.snapshot.as_ref())?; }
        if let Some(czech) = j.czech_exemptions {
            let y = cz::year(j.locale, year, &results, &|d| self.local(d.at), jurisdictions::ymd(czech.exemptions_from))?;
            (notes.summary, notes.exempt, notes.notice) = (y.summary, y.exempt, y.notice);
        }
        if let Some(a) = &austria {
            // D-TAX-AT-SPECULATION: before § 27b, a sale held a year or less was § 31 income.
            notes.speculation = results.iter().filter(|d| d.at.0 < a.regime_from.0 && d.long_held != Some(true))
                .map(|d| (format!("{} {} {}", d.asset, d.amount.to_s_f(), date(d.at)), d.gain_loss.clone())).collect();
            notes.averaged = results.iter().any(|d| d.averaged);
        }
        // D-TAX-ORPHAN-SWAP: where swaps are not taxable, a swap leg with nothing going the other way carries no basis.
        let unmatched: Vec<&walk::Row> = if j.swaps_taxable() { vec![] } else {
            txs.iter().map(|t| &t.row).filter(|r| r.orphan && matches!(r.kind, Kind::SwapIn | Kind::SwapOut) && !(self.stablecoin_as_fiat && stable(&r.base))).collect()
        };
        // D-TAX-SWAP-FEE-EXHAUSTED: the swaps of the taxpayer's year whose fee took everything received.
        for (uid, basis) in &engine.exhausted {
            let Some(r) = txs.iter().map(|t| &t.row).find(|r| r.uid == *uid).filter(|r| self.local(r.at).year() == year) else { continue };
            notes.exhausted.push((format!("{} {} {}", r.base, r.amount.to_s_f(), date(r.at)), basis.clone()));
        }
        if dk {
            // D-TAX-DK-STABLECOIN: held at the year's start, or any row of it in the year.
            let mut held: Vec<String> = engine.snapshot.iter().flatten().filter(|(sym, units)| dk_stablecoin(sym) && units.is_positive()).map(|(sym, _)| sym.clone())
                .chain(enriched.iter().map(|e| &e.row).filter(|r| dk_stablecoin(&r.base) && self.local(r.at).year() == year).map(|r| r.base.clone())).collect();
            held.sort();
            held.dedup();
            notes.stablecoins = held;
        }
        if j.code == "IE" {
            let y = ie::year(j.locale, j.annual_exemption.unwrap_or(0), &results, &|d| self.local(d.at), today)?;
            (notes.summary, notes.tail) = (y.summary, y.notes);
        }
        let allocation = if engine.allocation_chosen { engine.allocation.as_slice() } else { &[] };
        Ok(Outcome::Csv(self.csv(currency, &results, &enriched, &income, &warnings, &standing_fees, &unmatched, allocation, &notes)?))
    }

    /// Tax::Report#taxable_entries: no fiat row reaches the engine (D-TAX-STABLE-CASH: nor a stablecoin row where a
    /// stablecoin is cash), and an Alpaca CFEE goes to its fills. Returns the rows kept, whether each was folded away, and
    /// what the engine walks.
    fn txs(&self, enriched: &[Entry]) -> Result<Walked, FiguresError> {
        let stable_cash = !self.j.swaps_taxable() && self.stablecoin_as_fiat;
        let mut taxable: Vec<Entry> = enriched.iter().filter(|e| !(fiat(&e.row.base) || stable_cash && stable(&e.row.base))).cloned().collect();
        // D-TAX-IT-FEES: in Italy no CFEE joins its fill's cost; each walks after its fills as a coin fee.
        // D-TAX-IT-FEES: from 2023 no CFEE joins its fill's cost; each walks after its fills as a coin fee. Before 2023 fees
        // were costs (D-TAX-IT-PRE2023-BASIS) and fold as everywhere.
        let cfee: HashSet<i64> = match (self.j.code == "IT").then(|| local_midnight(&self.zone, it::FIRST_YEAR)).flatten() {
            Some(from) => taxable.iter().filter(|e| self.cfee.contains(&e.id) && e.row.at.0 < from.0).map(|e| e.id).collect(),
            None => self.cfee.clone(),
        };
        let folded = attribute_trade_fees(&cfee, &mut taxable)?;
        let identity = |symbol: &str, r: &walk::Row| prices::coin_id(&self.reference, symbol, self.reference.venue(&r.exchange), Some(r.at.utc().date_naive()))
            .unwrap_or_else(|| format!("symbol:{symbol}"));
        let mut entries: Vec<(i64, fifo::Tx)> = taxable.iter().zip(&folded).filter(|(_, f)| !**f)
            .map(|(e, _)| (e.id, fifo::Tx { row: e.row.clone(), tx_id: e.tx_id.clone(), fiat_exit: e.row.quote.as_deref().is_some_and(fiat),
                                            asset: identity(&e.row.base, &e.row),
                                            fee_asset: e.row.fee_currency.as_deref().filter(|f| !f.trim().is_empty()).map(|f| identity(f, &e.row)),
                                            cash_group: false })).collect();
        // Groups with a fiat leg, read before the fiat rows leave.
        let cash_groups: HashSet<(String, String)> = enriched.iter().filter(|e| fiat(&e.row.base))
            .filter_map(|e| e.row.group.clone().map(|g| (e.row.exchange.clone(), g))).collect();
        for (_, t) in &mut entries { t.cash_group = t.row.group.as_ref().is_some_and(|g| cash_groups.contains(&(t.row.exchange.clone(), g.clone()))); }
        let local_year = |at: At| self.local(at).year();
        walk_standing_cfees_after_their_fills(&self.cfee, &mut entries, &local_year);
        Ok((taxable, folded, entries.into_iter().map(|(_, t)| t).collect()))
    }

    fn austria(&self) -> Result<fifo::Austria, FiguresError> {
        let at = |day| local_start(&self.zone, day).ok_or_else(|| FiguresError::Data("an Austrian date out of range".into()));
        Ok(fifo::Austria { new_stock_from: at(self.j.old_stock_cutoff.unwrap_or((2021, 3, 1)))?, regime_from: at(AT_REGIME_FROM)?, average_from: at(AT_AVERAGE_FROM)? })
    }

    /// D-TAX-AT-MONTHLY-VALUE (VO § 4 Abs. 2, from 2023, Rz 6178p): income at receipt from one process paid more than three
    /// times in a calendar month (the user's zone) is valued, for the income and the cost, at the closing price of the
    /// 1st: the app's daily price dated the 2nd (the provider's 00:00 UTC point). The ledger has no process id: a process
    /// is read as (venue, coin, entry type), and each group so valued is listed.
    fn value_monthly(&self, enriched: &mut [Entry], ps: &mut PriceService, currency: &str, from: At) -> Result<Vec<String>, FiguresError> {
        let mut groups: std::collections::BTreeMap<(String, String, &str, i32, u32), Vec<usize>> = Default::default();
        for (i, e) in enriched.iter().enumerate() {
            let r = &e.row;
            let Some((_, name)) = INCOME.iter().find(|(k, _)| *k == r.kind && !matches!(k, Kind::StakingReward | Kind::Airdrop)) else { continue };
            if r.at.0 < from.0 || fiat(&r.base) { continue; }
            let day = self.local(r.at);
            groups.entry((r.exchange.clone(), r.base.clone(), name, day.year(), day.month())).or_default().push(i);
        }
        let mut listed = vec![];
        for ((venue, base, name, y, m), rows) in groups.into_iter().filter(|(_, rows)| rows.len() > 3) {
            let second = NaiveDate::from_ymd_opt(y, m, 2).and_then(|d| d.and_hms_opt(0, 0, 0)).and_then(|t| At::from_utc(t.and_utc()))
                .ok_or_else(|| FiguresError::Data("a month out of range".into()))?;
            let price = ps.price_at(&base, currency, second, Some(&self.scoped[rows[0]].venue))?;
            for &i in &rows {
                let r = &mut enriched[i].row;
                r.fiat_value = (&price * &r.amount)?;
                if price.is_zero() { (r.price_missing, r.value_missing) = (true, Some(true)); }
            }
            let label = text(self.j.locale, &format!("tax_report.income.types.{name}"), &[]);
            listed.push(format!("{venue} {base} {label} {y}-{m:02} {}", rows.len()));
        }
        Ok(listed)
    }

    /// Tax::Report#holding_rule: the period on the calendar in the user's zone; AT's old stock only before its cutoff;
    /// the Czech exemption only from the day it took force.
    fn holding_rule(&self) -> Option<impl Fn(At, At) -> bool + '_> {
        let (period, inclusive) = self.j.holding_period()?;
        let cutoff = self.j.old_stock_cutoff.map(jurisdictions::ymd);
        let in_force = self.j.czech_exemptions.map(|cz| jurisdictions::ymd(cz.exemptions_from));
        Some(move |acquired: At, disposed: At| {
            let (acquired, disposed) = (self.local(acquired), self.local(disposed));
            if cutoff.is_some_and(|c| acquired >= c) || in_force.is_some_and(|f| disposed < f) { return false; }
            jurisdictions::held_long(acquired, disposed, period, inclusive)
        })
    }

    fn t(&self, key: &str) -> Cell { Cell::Text(text(self.j.locale, key, &[])) }

    /// The file, in Rails' order, with Rust's warning blocks where they belong.
    #[allow(clippy::too_many_arguments)]
    fn csv(&self, currency: &str, results: &[fifo::Disposal], enriched: &[Entry], income: &[(usize, &str, Dec)], warnings: &[String],
           standing_fees: &[&walk::Row], unmatched: &[&walk::Row], allocation: &[fifo::Placed], at: &AtNotes) -> Result<String, FiguresError> {
        let (j, l) = (self.j, self.j.locale);
        let mut out = String::new();
        let mut keys = vec!["date", "acquisition_date", "asset", "amount", "proceeds", "cost_basis", "gain_loss", "currency", "holding_days", "fee",
                            "exchange", "tx_id", "cost_basis_complete", "data_incomplete"];
        if j.holding_exemption.is_some() { keys.push("tax_exempt"); }
        if j.old_stock_cutoff.is_some() { keys.push("old_stock"); }
        if j.short_long_term { keys.push("term"); }
        if j.method == "fifo_4week" { keys.push("matching_rule"); }
        if j.split_payment { keys.push("period"); }
        if j.tax_rate.is_some() { keys.push("tax_rate"); }
        if j.czech_exemptions.is_some() { keys.extend(["tax_exempt", "exempt_reason"]); }
        line(&mut out, &keys.iter().map(|k| self.t(&format!("tax_report.headers.{k}"))).collect::<Vec<_>>());
        for b in &self.banners { line(&mut out, &[s(b.clone())]); }
        let uncovered: Vec<&fifo::Disposal> = results.iter().filter(|d| d.uncovered.is_positive()).collect();
        // D-TAX-UNCOVERED: an uncovered sale is a value missing too.
        let missing = warnings.len() + uncovered.len();
        if missing > 0 {
            let banner = text(l, "tax_report.incomplete_banner", &[("missing", Arg::Text(&missing.to_string()))]);
            line(&mut out, &[s(format!("{}: {banner}", text(l, "tax_report.incomplete_banner_prefix", &[])))]);
        }
        if results.is_empty() { line(&mut out, &[self.t("tax_report.no_taxable_transactions")]); }
        for (i, d) in results.iter().enumerate() {
            let held = d.long_held == Some(true);
            let mut row = vec![s(iso(d.at)), d.acquired.map_or(Cell::Nil, |a| s(iso(a))), s(d.asset.clone()), n(&d.amount), cents(&d.proceeds)?,
                               cents(&d.cost_basis)?, cents(&d.gain_loss)?, s(currency), Cell::Raw(d.holding_days.to_string()), cents(&d.fee)?,
                               s(d.exchange.clone()), d.tx_id.clone().map_or(Cell::Nil, s), Cell::Raw(d.complete.to_string()), Cell::Raw(d.incomplete.to_string())];
            if j.holding_exemption.is_some() { row.push(Cell::Raw(held.to_string())); }
            if j.old_stock_cutoff.is_some() { row.push(Cell::Raw(held.to_string())); }
            if j.short_long_term { row.push(self.t(&format!("tax_report.values.term_{}", if held { "long" } else { "short" }))); }
            // D-TAX-IE-4WEEK: `4_week_rule` where a part was an acquisition of the 4 weeks before (Rails' values).
            if j.method == "fifo_4week" { row.push(s(if d.recent { "4_week_rule" } else { "fifo" })); }
            // December's disposals are the later period (s. 959AQ), on the taxpayer's calendar.
            if j.split_payment { row.push(s(if self.local(d.at).month() == 12 { "later" } else { "initial" })); }
            if let Some(rate) = j.tax_rate { row.push(s(it::rate(rate, self.local(d.at), &d.asset))); }
            if j.czech_exemptions.is_some() {
                let (exempt, reason) = at.exempt.get(i).cloned().unwrap_or((false, None));
                row.extend([Cell::Raw(exempt.to_string()), reason.map_or(Cell::Nil, s)]);
            }
            line(&mut out, &row);
        }
        out.push_str(&at.summary);
        // D-TAX-GR-NO-TAX: Σ gains and Σ losses (Rails' two lines), no taxable income and no tax line.
        if j.code == "GR" && !results.is_empty() {
            let (mut gains, mut losses) = (Dec::zero(), Dec::zero());
            for d in results {
                if d.gain_loss.is_positive() { gains = (&gains + &d.gain_loss)?; } else if d.gain_loss.is_negative() { losses = (&losses - &d.gain_loss)?; }
            }
            line(&mut out, &[]);
            line(&mut out, &[self.t("tax_report.summary.total_gains"), cents(&gains)?]);
            line(&mut out, &[self.t("tax_report.summary.total_losses"), cents(&losses)?]);
        }
        if j.code == "DK" && (!results.is_empty() || !income.is_empty()) { self.dk_summary(&mut out, results, income)?; }
        if !income.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[self.t("tax_report.income.header")]);
            let label = |name: &str| text(l, &format!("tax_report.income.types.{name}"), &[]);
            for (i, name, value) in income {
                let r = &enriched[*i].row;
                line(&mut out, &[s(self.local(r.at).format("%Y-%m-%d").to_string()), s(label(name)), s(r.base.clone()), n(&r.amount), cents(value)?, s(currency)]);
            }
            let mut kinds: Vec<&str> = vec![];
            for (_, name, _) in income { if !kinds.contains(name) { kinds.push(name); } }
            for name in kinds {
                let mut total = Dec::zero();
                for (_, _, v) in income.iter().filter(|(_, k, _)| *k == name) { total = (&total + v)?; }
                let label = text(l, "tax_report.income.total", &[("type", Arg::Text(&label(name)))]);
                line(&mut out, &[Cell::Nil, s(label), Cell::Nil, Cell::Nil, cents(&total)?, s(currency)]);
            }
        }
        if !warnings.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("WARNING: {}:", text(l, "tax_report.warnings.missing_prices", &[])))]);
            for w in warnings { line(&mut out, &[s(w.clone())]); }
            line(&mut out, &[self.t("tax_report.warnings.upgrade_hint")]);
        }
        let listed = |r: &walk::Row, amount: &Dec| format!("{} {} {}", r.base, amount.to_s_f(), date(r.at));
        self.block(&mut out, "WARNING", "uncovered_sales", "uncovered_sales_hint",
                   uncovered.iter().map(|d| format!("{} {} {}", d.asset, d.uncovered.to_s_f(), date(d.at))))?;
        let deposits: Vec<String> = enriched.iter().map(|e| &e.row).filter(|r| r.kind == Kind::Deposit && !r.linked && !fiat(&r.base))
            .map(|r| listed(r, &r.amount)).collect();
        // D-TAX-BG-DEPOSIT-ZERO, D-TAX-DK-DEPOSIT-ZERO: at cost 0, not at an assumed market value.
        let (heading, hint) = match j.code { "BG" => ("deposit_basis_zero", "deposit_basis_zero_hint_bg"),
            "DK" => ("deposit_basis_zero", "deposit_basis_zero_hint_dk"), "IT" => ("deposit_basis_zero", "deposit_basis_zero_hint_it"), _ => ("deposit_basis_assumed", "deposit_basis_assumed_hint") };
        self.block(&mut out, "WARNING", heading, hint, deposits.into_iter())?;
        self.block(&mut out, "WARNING", "unlinked_withdrawals", "unlinked_withdrawals_hint",
                   enriched.iter().map(|e| &e.row).filter(|r| r.kind == Kind::Withdrawal && !r.linked && !fiat(&r.base)).map(|r| listed(r, &r.amount)))?;
        self.block(&mut out, "WARNING", "lost", "lost_hint",
                   enriched.iter().map(|e| &e.row).filter(|r| r.kind == Kind::Lost && !fiat(&r.base)).map(|r| listed(r, &r.amount)))?;
        // § 38v ZDP.
        self.valued(&mut out, "WARNING", "cz_notice_38v", &at.notice, None)?;
        let stated: Vec<&walk::Row> = enriched.iter().map(|e| &e.row).filter(|r| r.stated_value).collect();
        if !stated.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("NOTE: {}:", text(l, "tax_report.warnings.stated_values", &[])))]);
            for r in stated { line(&mut out, &[s(listed(r, &r.amount)), cents(&r.fiat_value)?]); }
            line(&mut out, &[self.t("tax_report.warnings.stated_values_hint")]);
        }
        self.block(&mut out, "WARNING", "unmatched_swaps", "unmatched_swaps_hint", unmatched.iter().map(|r| listed(r, &r.amount)))?;
        self.valued(&mut out, "NOTE", "swap_fee_exhausted", &at.exhausted, Some(currency))?;
        let unsynced = text(l, "tax_report.warnings.wallet_allocation_unsynced", &[]);
        let placed: Vec<String> = allocation.iter().map(|p| Ok::<_, FiguresError>(format!("{} {} {} {} {}", p.venue.as_deref().unwrap_or(&unsynced), p.symbol,
                                                     p.amount.to_s_f(), p.cost.round(2)?.to_s_f(), date(p.acquired)))).collect::<Result<_, _>>()?;
        self.block(&mut out, "NOTE", "wallet_allocation", "wallet_allocation_hint", placed.into_iter())?;
        // D-TAX-AT-FEES: in Austria a fee row of its own is a disposal.
        let hint = if j.code == "IT" { "coin_fees_hint_it" } else if j.swaps_taxable() || j.code == "AT" { "coin_fees_hint_disposal" } else { "coin_fees_hint_no_proceeds" };
        self.block(&mut out, "NOTE", "coin_fees", hint, standing_fees.iter().map(|r| listed(r, &r.amount)))?;
        self.valued(&mut out, "NOTE", "speculation", &at.speculation, None)?;
        self.valued(&mut out, "NOTE", "income_before_regime", &at.before_regime, None)?;
        self.block(&mut out, "NOTE", "cz_mining", "cz_mining_hint", at.mined.iter().cloned())?;
        self.block(&mut out, "NOTE", "zero_cost_rewards", "zero_cost_rewards_hint", at.zero_cost.iter().cloned())?;
        self.block(&mut out, "NOTE", "monthly_value", "monthly_value_hint", at.monthly.iter().cloned())?;
        if at.averaged {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("NOTE: {}:", text(l, "tax_report.warnings.average_cost", &[])))]);
            line(&mut out, &[self.t("tax_report.warnings.average_cost_hint")]);
        }
        if at.bg_average {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("NOTE: {}:", text(l, "tax_report.warnings.average_cost_bg", &[])))]);
            line(&mut out, &[self.t("tax_report.warnings.average_cost_hint_bg")]);
        }
        if j.code == "DK" { self.dk_notes(&mut out, results, enriched, income, at)?; }
        if j.code == "IT" { self.it_notes(&mut out, &at.it, currency)?; }
        if j.code == "GR" && !results.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("NOTE: {}:", text(l, "tax_report.warnings.gr_no_tax", &[])))]);
            line(&mut out, &[self.t("tax_report.warnings.gr_no_tax_hint")]);
        }
        out.push_str(&at.tail);
        Ok(out)
    }

    /// D-TAX-DK-RUBRIK: rubrik 20 = the gains, part by part, and the assets received (staking, lending, airdrops); rubrik 58
    /// = the losses, part by part. Contract stablecoins (D-TAX-DK-STABLECOIN) and sales of mined assets stay out.
    fn dk_summary(&self, out: &mut String, results: &[fifo::Disposal], income: &[(usize, &str, Dec)]) -> Result<(), FiguresError> {
        let (mut gains, mut losses, mut received) = (Dec::zero(), Dec::zero(), Dec::zero());
        for d in results.iter().filter(|d| !dk_contract(&d.asset) && !d.mined) {
            let v = d.gain_loss.round(2)?;
            if v.is_positive() { gains = (&gains + &v)?; } else { losses = (&losses - &v)?; }
        }
        for (_, _, v) in income.iter().filter(|(_, n, _)| DK_RECEIVED.contains(n)) { received = (&received + &v.round(2)?)?; }
        line(out, &[]);
        line(out, &[self.t("tax_report.dk.rubrik_20"), cents(&(&gains + &received)?)?]);
        line(out, &[s(format!("  {}", text(self.j.locale, "tax_report.dk.of_which_gains", &[]))), cents(&gains)?]);
        line(out, &[s(format!("  {}", text(self.j.locale, "tax_report.dk.of_which_income", &[]))), cents(&received)?]);
        line(out, &[self.t("tax_report.dk.rubrik_58"), cents(&losses)?]);
        line(out, &[self.t("tax_report.dk.hint")]);
        Ok(())
    }

    /// Denmark's NOTE blocks: D-TAX-DK-SWAP-VALUE, D-TAX-DK-STABLECOIN, and mining and other income (hobby, apart).
    fn dk_notes(&self, out: &mut String, results: &[fifo::Disposal], enriched: &[Entry], income: &[(usize, &str, Dec)], at: &AtNotes) -> Result<(), FiguresError> {
        let l = self.j.locale;
        self.block(out, "NOTE", "dk_swap_value", "dk_swap_value_hint", at.swap_values.iter().cloned())?;
        if !at.stablecoins.is_empty() {
            line(out, &[]);
            line(out, &[s(format!("NOTE: {}:", text(l, "tax_report.warnings.dk_stablecoins", &[])))]);
            let (contracts, others): (Vec<&String>, Vec<&String>) = at.stablecoins.iter().partition(|c| dk_contract(c));
            if !contracts.is_empty() {
                line(out, &[self.t("tax_report.warnings.dk_stablecoins_contracts")]);
                for c in contracts {
                    let mut result = Dec::zero();
                    for d in results.iter().filter(|d| d.asset == **c) { result = (&result + &d.gain_loss.round(2)?)?; }
                    line(out, &[s(c.clone()), cents(&result)?]);
                }
            }
            if !others.is_empty() {
                line(out, &[self.t("tax_report.warnings.dk_stablecoins_other")]);
                for c in others { line(out, &[s(c.clone())]); }
            }
            line(out, &[self.t("tax_report.warnings.dk_stablecoins_hint")]);
        }
        let mut apart: Vec<(String, Dec)> = vec![];
        for (i, name, value) in income.iter().filter(|(_, n, _)| !DK_RECEIVED.contains(n)) {
            let r = &enriched[*i].row;
            let label = text(l, &format!("tax_report.income.types.{name}"), &[]);
            apart.push((format!("{label} {} {} {}", r.base, r.amount.to_s_f(), date(r.at)), value.clone()));
        }
        let sold = text(l, "tax_report.headers.proceeds", &[]);
        for d in results.iter().filter(|d| d.mined) { apart.push((format!("{sold} {} {} {}", d.asset, d.amount.to_s_f(), date(d.at)), d.proceeds.clone())); }
        self.valued(out, "NOTE", "dk_mining_other", &apart, None)
    }

    /// A block whose lines carry an amount, as the stated-values one. `currency`: written after each amount
    /// (D-TAX-SWAP-FEE-EXHAUSTED, whose block may stand with no other figure).
    fn valued(&self, out: &mut String, label: &str, heading: &str, lines: &[(String, Dec)], currency: Option<&str>) -> Result<(), FiguresError> {
        if lines.is_empty() { return Ok(()); }
        line(out, &[]);
        line(out, &[s(format!("{label}: {}:", text(self.j.locale, &format!("tax_report.warnings.{heading}"), &[])))]);
        for (x, v) in lines {
            let mut cells = vec![s(x.clone()), cents(v)?];
            if let Some(c) = currency { cells.push(s(c.to_string())); }
            line(out, &cells);
        }
        line(out, &[self.t(&format!("tax_report.warnings.{heading}_hint"))]);
        Ok(())
    }

    /// A warning block as Rails lays out the deposit-basis one: a blank line, "<LABEL>: <heading>:", the lines, the hint.
    fn block(&self, out: &mut String, label: &str, heading: &str, hint: &str, lines: impl Iterator<Item = String>) -> Result<(), FiguresError> {
        let lines: Vec<String> = lines.collect();
        if lines.is_empty() { return Ok(()); }
        line(out, &[]);
        line(out, &[s(format!("{label}: {}:", text(self.j.locale, &format!("tax_report.warnings.{heading}"), &[])))]);
        for x in lines { line(out, &[s(x)]); }
        line(out, &[self.t(&format!("tax_report.warnings.{hint}"))]);
        Ok(())
    }
}

/// D-TAX-COIN-QUOTE, grouped legs: where swaps are not taxable and a stablecoin is a coin, a group of coin legs only (no
/// fiat) booked as buy and sell, both ways present (Kraken's ledger, which the enrichment reads with a stablecoin as
/// cash), is the swap it is: its legs walk as swap legs. A coin for a stablecoin and a stablecoin for another are both
/// crypto for crypto. Rails taxed the leg given as sold.
/// D-TAX-IT-SWAPS (`italy`): a group with an e-money token leg is left as booked (a sale and a purchase).
fn swap_coin_for_stablecoin_groups(rows: &mut [Entry], italy: bool) {
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, e) in rows.iter().enumerate() { if let Some(g) = &e.row.group { groups.entry((e.row.exchange.clone(), g.clone())).or_default().push(i); } }
    for legs in groups.values() {
        if legs.iter().any(|&i| fiat(&rows[i].row.base) || !matches!(rows[i].row.kind, Kind::Buy | Kind::Sell)) { continue; }
        if !(legs.iter().any(|&i| rows[i].row.kind == Kind::Buy) && legs.iter().any(|&i| rows[i].row.kind == Kind::Sell)) { continue; }
        if italy && legs.iter().any(|&i| it::emt(&rows[i].row.base)) { continue; }
        for &i in legs { rows[i].row.kind = if rows[i].row.kind == Kind::Buy { Kind::SwapIn } else { Kind::SwapOut }; }
    }
}

/// D-TAX-DK-RUBRIK: the income types in rubrik 20 (mining is a hobby account, other income for the user to classify).
const DK_RECEIVED: [&str; 3] = ["staking_reward", "lending_interest", "airdrop"];

/// D-TAX-DK-STABLECOIN: the stablecoins Skattestyrelsen treats as financial contracts (SKM2022.300.SR as skat.dk applies it).
fn dk_contract(symbol: &str) -> bool { matches!(symbol, "USDT" | "USDC" | "USDP") }
/// Every stablecoin the NOTE names: the contracts, and the others as not classified.
fn dk_stablecoin(symbol: &str) -> bool { stable(symbol) || symbol == "USDP" }

/// D-TAX-DK-SWAP-VALUE: the side whose value both take, `true` for the coins received: a fiat stablecoin's side, else
/// Bitcoin's, else Ether's, else the side received (Skattestyrelsen's list ends with 2022; no historical rank is held).
fn dk_value_side(ins: &[&walk::Row], outs: &[&walk::Row]) -> Option<bool> {
    let tests: [fn(&str) -> bool; 3] = [|b| matches!(b, "USDT" | "USDC" | "USDP" | "EURC"), |b| b == "BTC", |b| b == "ETH"];
    for t in tests {
        let (i, o) = (ins.iter().any(|r| t(&r.base)), outs.iter().any(|r| t(&r.base)));
        if i || o { return Some(i); }
    }
    Some(true)
}

/// D-TAX-SWAP-RECEIVED (BG, CZ, IE: `side` always the coins received; IT: only a group with an e-money token),
/// D-TAX-DK-SWAP-VALUE (DK: `dk_value_side`).
/// One value for both sides of an exchange of coins (a group of coin legs only): the side `side` names (`true`: the
/// coins received) gives its total, which the other side's legs share by their own values (evenly where one is
/// unpriced), the last taking the rest; set on this report's copy of the rows before the engine. Returns each group so
/// valued, listed.
fn value_groups(rows: &mut [Entry], side: fn(&[&walk::Row], &[&walk::Row]) -> Option<bool>) -> Result<Vec<String>, FiguresError> {
    let mut order: Vec<(String, String)> = vec![];
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, e) in rows.iter().enumerate() {
        let Some(g) = &e.row.group else { continue };
        let k = (e.row.exchange.clone(), g.clone());
        if !groups.contains_key(&k) { order.push(k.clone()); }
        groups.entry(k).or_default().push(i);
    }
    let mut listed = vec![];
    for k in order {
        // The trade's legs; a fee row booked in the group is no leg (it is walked on its own).
        let legs: Vec<usize> = groups[&k].iter().copied().filter(|&i| rows[i].row.kind.in_leg() || rows[i].row.kind.out_leg()).collect();
        if legs.iter().any(|&i| fiat(&rows[i].row.base)) { continue; }
        let (ins, outs): (Vec<usize>, Vec<usize>) = legs.iter().partition(|&&i| rows[i].row.kind.in_leg());
        if ins.is_empty() || outs.is_empty() { continue; }
        // `None`: the group is not valued at all (Italy: only an exchange with an e-money token is a disposal).
        let Some(received) = side(&ins.iter().map(|&i| &rows[i].row).collect::<Vec<_>>(), &outs.iter().map(|&i| &rows[i].row).collect::<Vec<_>>()) else { continue };
        let (from, to) = if received { (&ins, &outs) } else { (&outs, &ins) };
        // A side whose value is not known values nothing: the other side keeps its own values as an estimate, marked
        // missing, so the disposal is incomplete and counted in the banner.
        if from.iter().any(|&i| rows[i].row.value_missing.unwrap_or(rows[i].row.price_missing)) {
            for &i in to { let r = &mut rows[i].row; (r.price_missing, r.value_missing) = (true, Some(true)); }
            continue;
        }
        let mut value = Dec::zero();
        for &i in from { value = (&value + &rows[i].row.fiat_value)?; }
        if !value.is_positive() { continue; }
        let even = to.iter().any(|&i| !rows[i].row.fiat_value.is_positive());
        let mut weight = Dec::zero();
        for &i in to { weight = (&weight + &if even { Dec::one() } else { rows[i].row.fiat_value.clone() })?; }
        let mut left = value.clone();
        for (n, &i) in to.iter().enumerate() {
            let r = &mut rows[i].row;
            let share = if n == to.len() - 1 { left.clone() } else { (&value * &if even { Dec::one() } else { r.fiat_value.clone() })?.div(&weight)? };
            left = (&left - &share)?;
            r.fiat_value = share;
            (r.price_missing, r.value_missing) = (r.fee_missing, Some(false));
        }
        let legs = |side: &[usize]| side.iter().map(|&i| format!("{} {}", rows[i].row.base, rows[i].row.amount.to_s_f())).collect::<Vec<_>>().join(" + ");
        listed.push(format!("{} → {} {}", legs(&outs), legs(&ins), date(rows[outs[0]].row.at)));
    }
    Ok(listed)
}

/// D-TAX-STABLE-VALUE: a venue's ledger that books a stablecoin bought or sold for fiat as two grouped rows and no quote
/// (Kraken) gets the quote it left out, on this report's copy of the rows: the stablecoin legs (one coin, one direction)
/// take the fiat legs (one currency, the other direction, nothing else in the group) as their quote, shared by quantity,
/// the last taking what is left. Rails values them at the dollar peg instead of the cash that changed hands, and so never
/// sees the Portuguese immediate fiat exit there. The tracker's enrichment is untouched.
fn quote_stablecoin_legs(rows: &mut [Stored]) -> Result<(), FiguresError> {
    let mut groups: HashMap<(i64, String), Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() { if let Some(g) = &r.group { groups.entry((r.exchange_id, g.clone())).or_default().push(i); } }
    let direction = |k: Kind| if k.in_leg() { Some(true) } else if k.out_leg() { Some(false) } else { None };
    for legs in groups.values() {
        let (coins, cash): (Vec<usize>, Vec<usize>) = legs.iter().partition(|&&i| stable(&rows[i].base));
        let (Some(&first), Some(&paid)) = (coins.first(), cash.first()) else { continue };
        let side = direction(rows[first].kind);
        if side.is_none() || !coins.iter().all(|&i| rows[i].base == rows[first].base && direction(rows[i].kind) == side && rows[i].quote_amount.is_none())
            || !cash.iter().all(|&i| fiat(&rows[i].base) && rows[i].base == rows[paid].base && direction(rows[i].kind) == side.map(|d| !d)) { continue; }
        let mut total = Dec::zero();
        for &i in &cash { total = (&total + &rows[i].amount)?; }
        let mut quantity = Dec::zero();
        for &i in &coins { quantity = (&quantity + &rows[i].amount)?; }
        if !quantity.is_positive() { continue; }
        let mut left = total.clone();
        for (n, &i) in coins.iter().enumerate() {
            let share = if n == coins.len() - 1 { left.clone() } else { (&total * &rows[i].amount)?.div(&quantity)? };
            left = (&left - &share)?;
            rows[i].quote = Some(rows[paid].base.clone());
            rows[i].quote_amount = Some(share);
        }
    }
    Ok(())
}

/// An Alpaca CFEE left standing (its day's fills differ in UTC and New York time) is dated midnight UTC; it is walked
/// after the last fill of its coin on its venue in that UTC day and in the fee's own local tax year, so it takes lots its
/// day's buys opened, and every year's report (each loads its whole local year) orders it alike. Its own date stays (the
/// year, the holding days and the listing read it).
fn walk_standing_cfees_after_their_fills(cfee: &HashSet<i64>, rows: &mut Vec<(i64, fifo::Tx)>, local_year: &dyn Fn(At) -> i32) {
    let fees: Vec<i64> = rows.iter().filter(|(id, t)| cfee.contains(id) && t.row.kind == Kind::Fee).map(|(id, _)| *id).collect();
    for fee in fees {
        let Some(i) = rows.iter().position(|(id, _)| *id == fee) else { continue };
        let f = &rows[i].1.row;
        let day = f.at.utc().date_naive();
        let last = rows.iter().rposition(|(_, t)| {
            let r = &t.row;
            matches!(r.kind, Kind::Buy | Kind::Sell) && r.exchange == f.exchange && r.base == f.base && r.amount.is_positive()
                && r.at.utc().date_naive() == day && local_year(r.at) == local_year(f.at)
        });
        if let Some(last) = last.filter(|&l| l > i) { let moved = rows.remove(i); rows.insert(last, moved); }
    }
}

/// Tax::CryptoScope#crypto? over the catalogue and the user's own classifications.
struct CryptoScope { classified: HashSet<String>, memo: std::cell::RefCell<HashMap<String, bool>> }

impl CryptoScope {
    fn new(c: &Connection, user_id: i64) -> Result<CryptoScope, FiguresError> {
        let mut s = c.prepare("SELECT symbol FROM fund_classifications WHERE user_id = ?1")?;
        let classified = s.query_map([user_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(CryptoScope { classified, memo: Default::default() })
    }

    fn crypto(&self, c: &Connection, symbol: &str) -> Result<bool, FiguresError> {
        if let Some(&known) = self.memo.borrow().get(symbol) { return Ok(known); }
        let mut s = c.prepare_cached("SELECT category, instrument_type FROM assets WHERE symbol = ?1")?;
        let assets: Vec<(Option<String>, Option<String>)> = s.query_map([symbol], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        let mut categories: Vec<&str> = assets.iter().filter_map(|(c, _)| c.as_deref()).collect();
        categories.sort_unstable();
        categories.dedup();
        let crypto = !symbol.trim().is_empty() && !assets.is_empty() && !self.classified.contains(symbol)
            && !assets.iter().any(|(_, t)| matches!(t.as_deref(), Some("stock" | "etf" | "tokenized")))
            && categories == ["Cryptocurrency"];
        self.memo.borrow_mut().insert(symbol.to_string(), crypto);
        Ok(crypto)
    }
}

/// Tax::Report#tokenized_symbols_in_scope: the wrappers the rows touch, by what the row's venue lists under the symbol,
/// then by the coin the symbol means there; the catalogue's spelling, sorted.
fn tokenized_symbols<'a>(c: &Connection, rows: impl Iterator<Item = &'a Stored>) -> Result<Vec<String>, FiguresError> {
    let canonical = |s: &str| s.strip_suffix("SPV").map_or_else(|| s.to_string(), |head| format!("{head}x")).to_uppercase();
    let rows: Vec<&Stored> = rows.filter(|r| !r.base.trim().is_empty()).collect();
    let names: HashSet<String> = rows.iter().flat_map(|r| [r.base.clone(), canonical(&r.base)]).collect();
    let reference = Reference::load(c, &Venue::alpaca(c)?, &names)?;
    let mut listed = c.prepare("SELECT a.symbol, a.instrument_type FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                                WHERE t.exchange_id = ?1 AND t.base IN (?2, ?3) ORDER BY t.id LIMIT 1")?;
    let mut wrapper = c.prepare("SELECT symbol FROM assets WHERE external_id = ?1 AND instrument_type = 'tokenized' LIMIT 1")?;
    let mut out = vec![];
    for r in rows {
        let venue_asset: Option<(Option<String>, Option<String>)> =
            listed.query_row(rusqlite::params![r.exchange_id, r.base, canonical(&r.base)], |x| Ok((x.get(0)?, x.get(1)?))).optional()?;
        let venue = reference.venue(&r.venue);
        let found = match venue_asset {
            Some((symbol, Some(t))) if t == "tokenized" => symbol,
            _ => match alias_coin(&r.base, venue, Some(r.date()))
                .or_else(|| venue.and_then(|v| catalogue_coin(&reference, &r.base, v)))
                .or_else(|| venue.and_then(|v| catalogue_coin(&reference, &canonical(&r.base), v))) {
                Some(coin) => wrapper.query_row([coin], |x| x.get::<_, Option<String>>(0)).optional()?.flatten(),
                None => None,
            },
        };
        if let Some(symbol) = found { if !out.contains(&symbol) { out.push(symbol); } }
    }
    out.sort();
    Ok(out)
}

/// ApiKey#sync_issue for every key but those of a stock venue other than Alpaca, as banner lines in the report's locale.
fn sync_issues(c: &Connection, user_id: i64, locale: &str) -> Result<Vec<String>, FiguresError> {
    let mut s = c.prepare("SELECT e.name, k.key_type, k.status, k.last_sync_error, k.last_synced_at FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
                           WHERE k.user_id = ?1 AND e.type IS NOT 'Exchanges::Ibkr' ORDER BY k.id")?;
    let keys = s.query_map([user_id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
                                               r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)))?;
    let prefix = text(locale, "tax_report.incomplete_banner_prefix", &[]);
    let mut out = vec![];
    for key in keys {
        let (name, key_type, status, error, synced) = key?;
        let name = name.unwrap_or_default();
        // key_type 1 is :withdrawal; status 1 is :correct.
        let reason = if key_type == 1 { None }
            else if error.is_some_and(|e| !e.trim().is_empty()) { Some("exchange_sync_failed") }
            else if status == 1 && synced.is_none() { Some("exchange_never_synced") }
            else { None };
        if let Some(reason) = reason {
            out.push(format!("{prefix}: {}", text(locale, &format!("tax_report.warnings.{reason}"), &[("exchange", Arg::Text(&name))])));
        }
    }
    Ok(out)
}

/// Tax::Report#attribute_trade_fees over the taxable rows: an Alpaca CFEE (a coin fee of its own row, dated to the day)
/// goes to that day's fills of the coin on its venue when the UTC and the New York calendars give the same fills, buys
/// first (fewer coins, same cost), else sales (more coins out, same proceeds), pro rata by amount, the last fill taking
/// what is left. Returns, per row, whether it was folded away.
fn attribute_trade_fees(cfee: &HashSet<i64>, entries: &mut [Entry]) -> Result<Vec<bool>, FiguresError> {
    let new_york = |at: At| at.utc().with_timezone(&chrono_tz::America::New_York).date_naive();
    let ids: Vec<i64> = entries.iter().map(|e| e.id).collect();
    let mut rows: Vec<&mut walk::Row> = entries.iter_mut().map(|e| &mut e.row).collect();
    let mut folded = vec![false; rows.len()];
    for i in 0..rows.len() {
        let fee = &rows[i];
        if fee.kind != Kind::Fee || !cfee.contains(&ids[i]) || !fee.amount.is_positive() { continue; }
        let (day, base, venue, amount) = (fee.at.utc().date_naive(), fee.base.clone(), fee.exchange.clone(), fee.amount.clone());
        let same_coin: Vec<usize> = (0..rows.len()).filter(|&j| {
            let r = &rows[j];
            matches!(r.kind, Kind::Buy | Kind::Sell) && r.exchange == venue && r.base == base && r.amount.is_positive()
        }).collect();
        let fills: Vec<usize> = same_coin.iter().copied().filter(|&j| rows[j].at.utc().date_naive() == day).collect();
        let new_york_fills: Vec<usize> = same_coin.iter().copied().filter(|&j| new_york(rows[j].at) == day).collect();
        if (fills.is_empty() && new_york_fills.is_empty()) || fills != new_york_fills { continue; }
        let buys: Vec<usize> = fills.iter().copied().filter(|&j| rows[j].kind == Kind::Buy).collect();
        let fills = if buys.is_empty() { fills } else { buys };
        let mut total = Dec::zero();
        for &j in &fills { total = (&total + &rows[j].amount)?; }
        let mut left = amount.clone();
        for (n, &j) in fills.iter().enumerate() {
            let r = &mut *rows[j];
            let share = if n == fills.len() - 1 { left.clone() } else { (&amount * &r.amount)?.div(&total)? };
            left = (&left - &share)?;
            r.amount = if r.kind == Kind::Buy { let net = (&r.amount - &share)?; if net.is_negative() { Dec::zero() } else { net } } else { (&r.amount + &share)? };
        }
        folded[i] = true;
    }
    Ok(folded)
}

/// A cell as Ruby's CSV writes it through CsvSafe: a String (escaped when it reads as a formula after Ruby's lstrip, which
/// takes a NUL too; `ruby::strip` answers the same, as what lstrip leaves never starts with what strip removes), anything
/// else as its `to_s`, or nil.
pub(crate) enum Cell { Text(String), Raw(String), Nil }

fn s(v: impl Into<String>) -> Cell { Cell::Text(v.into()) }
fn n(v: &Dec) -> Cell { Cell::Raw(v.to_s_f()) }
fn cents(v: &Dec) -> Result<Cell, FiguresError> { Ok(Cell::Raw(v.round(2)?.to_s_f())) }
fn iso(at: At) -> String { at.utc().format("%Y-%m-%dT%H:%M:%SZ").to_string() }
fn date(at: At) -> String { at.utc().format("%Y-%m-%d").to_string() }

pub(crate) fn line(out: &mut String, cells: &[Cell]) {
    const LEADERS: [char; 7] = ['=', '+', '-', '@', '\t', '\r', '\n'];
    let fields: Vec<String> = cells.iter().map(|c| match c {
        Cell::Nil => String::new(),
        Cell::Raw(v) => quote(v),
        Cell::Text(v) if v.is_empty() => "\"\"".into(),
        Cell::Text(v) if v.starts_with(LEADERS) || crate::ruby::strip(v).starts_with(LEADERS) => quote(&format!("'{v}")),
        Cell::Text(v) => quote(v),
    }).collect();
    out.push_str(&fields.join(","));
    out.push('\n');
}

fn quote(v: &str) -> String {
    if v.contains([',', '"', '\r', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}
