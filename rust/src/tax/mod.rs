//! Tax reports (roadmap stage F): Tax::GenerateReportJob#crypto_csv and Tax::Report#to_csv over every venue the user
//! has, for the jurisdictions `jurisdictions::PORTED` names (US DE ES PT SK). Byte for byte Rails' file where Rails is
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
//!   (`split_coin_quoted_trades`), so the quote coin moves too.
//! - D-TAX-STABLE-VALUE: a stablecoin bought or sold for fiat in a two-row group (Kraken) is valued at the fiat that
//!   changed hands, not the dollar peg (`quote_stablecoin_legs`); Portugal's immediate fiat exit follows.
//! - D-TAX-LOCAL-YEAR: income and the excess-ROC refusal by the taxpayer's year, as disposals (Rails: the UTC year).
//! - D-TAX-ORPHAN-SWAP: where swaps are not taxable, swap legs with nothing going the other way are listed.
//! - D-TAX-US-WALLET: US tax years from 2025, lots per account; a placement at 1 January 2025 that was a choice is listed
//!   lot by lot (`tax_report.warnings.wallet_allocation`), as this report's reconstruction.
//!
//! The report is read (`prepare`, on a connection) apart from its enrichment (`Report::compute`, with no connection held):
//! the job fetches the prices the table lacks between the enrichment's steps, as the tracker's pass does.
pub mod api;
pub mod fifo;
pub mod job;
pub mod jurisdictions;

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
}

/// The reads of `crypto_csv` up to the enrichment: the scope, the tokenized refusal, the banners.
pub fn prepare(c: &Connection, user_id: i64, country: &str, year: i32, stablecoin_as_fiat: bool) -> Result<Prepared, FiguresError> {
    let j = jurisdictions::find(country).filter(|_| jurisdictions::ported(country))
        .ok_or_else(|| FiguresError::NotComputed(format!("the tax report is not ported for the jurisdiction {country}")))?;
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

    let mut scoped = rows::ordered(split_coin_quoted_trades(rows.into_iter().filter(|r| r.at.0 <= rows_until.0).collect()));
    quote_stablecoin_legs(&mut scoped)?;
    let mut reference = Reference::load(c, &Venue::alpaca(c)?, &prices::symbols(&scoped))?;
    reference.record_coins(c, user_id)?;
    let fx = crate::tracker::fx::FxTable::load(c)?;
    let mut s = c.prepare("SELECT id FROM account_transactions WHERE json_extract(raw_data, '$.activity_type') = 'CFEE'")?;
    let cfee: HashSet<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(Prepared::Report(Box::new(Report { j, year, zone, stablecoin_as_fiat, banners, scoped, deposits, reference, fx, cfee })))
}

/// Midnight starting 1 January of `year` in the user's zone (`ActiveSupport::TimeZone#local`).
fn local_midnight(zone: &str, year: i32) -> Option<At> {
    crate::web::timezone::strict(zone).unwrap_or(chrono_tz::Tz::UTC).with_ymd_and_hms(year, 1, 1, 0, 0, 0).earliest()
        .and_then(|t| At::from_utc(t.with_timezone(&Utc)))
}

/// An enriched row and what the report reads beside it.
#[derive(Clone)]
struct Entry { id: i64, row: walk::Row, tx_id: Option<String> }

impl Report {
    fn local(&self, at: At) -> NaiveDate { crate::web::timezone::local(at.utc(), &self.zone).date_naive() }

    /// Tax::Report#to_csv from the enrichment on: prices the table lacks are fetched through `fetcher` (Rails'
    /// `prefetch` and single-day fetches), and `on_progress(percent)` is told as PriceService#enrich tells it.
    pub fn compute(&self, fetcher: &mut dyn Fetcher, today: NaiveDate, on_progress: &mut dyn FnMut(u32)) -> Result<Outcome, FiguresError> {
        let (j, year) = (self.j, self.year);
        let currency = j.currency(year);
        let mut ps = PriceService::new(&self.reference, &self.fx, today, fetcher);
        let enriched: Vec<Entry> = walk::enrich_with(&self.scoped, &self.deposits, &mut ps, true, currency, on_progress)?.into_iter().zip(&self.scoped)
            .map(|(row, s)| Entry { id: s.id, row, tx_id: s.tx_id.clone() }).collect();
        // Income of the UTC year, a fiat row converted at the day's rate (Tax::Report#income_value), valued before the
        // engine: its missing rate counts towards the banner.
        let mut income: Vec<(usize, &str, Dec)> = vec![];
        for (i, e) in enriched.iter().enumerate() {
            let Some((_, name)) = INCOME.iter().find(|(k, _)| *k == e.row.kind) else { continue };
            // D-TAX-LOCAL-YEAR: income by the year the taxpayer was in, as disposals are (Rails: the UTC year).
            if self.local(e.row.at).year() != year { continue; }
            let value = if fiat(&e.row.base) { ps.convert_fiat(&e.row.amount, &e.row.base, currency, e.row.at)? } else { e.row.fiat_value.clone() };
            income.push((i, name, value));
        }
        let warnings = { let mut seen = HashSet::new(); ps.warnings.iter().filter(|w| seen.insert(w.as_str())).cloned().collect::<Vec<_>>() };

        // Tax::Report#taxable_entries: no fiat row reaches the engine (D-TAX-STABLE-CASH: nor a stablecoin row where a
        // stablecoin is cash), and an Alpaca CFEE goes to its fills.
        let stable_cash = !j.swaps_taxable() && self.stablecoin_as_fiat;
        let mut taxable: Vec<Entry> = enriched.iter().filter(|e| !(fiat(&e.row.base) || stable_cash && stable(&e.row.base))).cloned().collect();
        let folded = attribute_trade_fees(&self.cfee, &mut taxable)?;
        let standing_fees: Vec<&walk::Row> = taxable.iter().zip(&folded).filter(|(e, f)| !**f && e.row.kind == Kind::Fee && e.row.amount.is_positive())
            .map(|(e, _)| &e.row).collect();
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
        let mut txs: Vec<fifo::Tx> = entries.into_iter().map(|(_, t)| t).collect();

        let holding = self.holding_rule();
        let per_venue_from = (j.code == "US" && year >= 2025).then(|| local_midnight(&self.zone, 2025)).flatten();
        // D-TAX-US-WALLET: per venue, a withdrawal to another venue is walked first within its second (`transfers_first`),
        // so a same-second sale on the far venue takes the lots it brought. The pooled years keep Rails' order.
        if let Some(from) = per_venue_from {
            let split = txs.iter().position(|t| t.row.at.0 >= from.0).unwrap_or(txs.len());
            let tail: Vec<fifo::Tx> = txs.split_off(split);
            let order = walk::transfers_first(&tail.iter().map(|t| t.row.clone()).collect::<Vec<_>>());
            let mut by_uid: HashMap<usize, fifo::Tx> = tail.into_iter().map(|t| (t.row.uid, t)).collect();
            txs.extend(order.iter().filter_map(|r| by_uid.remove(&r.uid)));
        }
        let mut engine = fifo::Fifo::new(fifo::Options {
            swaps_taxable: j.swaps_taxable(), stablecoin_as_fiat: self.stablecoin_as_fiat,
            holding: holding.as_ref().map(|h| h as &dyn Fn(At, At) -> bool), swap_resets_holding_period: j.swap_resets_holding_period, per_venue_from,
            swap_cost_to_disposed_from: per_venue_from,
        });
        engine.calculate(&txs)?;
        // Tax::Report#refuse_excess_return_of_capital: this year's only (D-TAX-LOCAL-YEAR: the taxpayer's year).
        let mut symbols: Vec<String> = engine.excess_roc_rows.iter().map(|&i| &txs[i].row).filter(|r| self.local(r.at).year() == year).map(|r| r.base.clone()).collect();
        symbols.sort();
        symbols.dedup();
        if !symbols.is_empty() { return Ok(Outcome::Refused { reason: "excess_return_of_capital".into(), symbols }); }
        let results: Vec<fifo::Disposal> = engine.disposals.into_iter().filter(|d| self.local(d.at).year() == year).collect();
        // D-TAX-ORPHAN-SWAP: where swaps are not taxable, a swap leg with nothing going the other way carries no basis.
        let unmatched: Vec<&walk::Row> = if j.swaps_taxable() { vec![] } else {
            txs.iter().map(|t| &t.row).filter(|r| r.orphan && matches!(r.kind, Kind::SwapIn | Kind::SwapOut) && !(self.stablecoin_as_fiat && stable(&r.base))).collect()
        };
        let allocation = if engine.allocation_chosen { engine.allocation.as_slice() } else { &[] };
        Ok(Outcome::Csv(self.csv(currency, &results, &enriched, &income, &warnings, &standing_fees, &unmatched, allocation)?))
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
           standing_fees: &[&walk::Row], unmatched: &[&walk::Row], allocation: &[fifo::Placed]) -> Result<String, FiguresError> {
        let (j, l) = (self.j, self.j.locale);
        let mut out = String::new();
        let mut keys = vec!["date", "acquisition_date", "asset", "amount", "proceeds", "cost_basis", "gain_loss", "currency", "holding_days", "fee",
                            "exchange", "tx_id", "cost_basis_complete", "data_incomplete"];
        if j.holding_exemption.is_some() { keys.push("tax_exempt"); }
        if j.old_stock_cutoff.is_some() { keys.push("old_stock"); }
        if j.short_long_term { keys.push("term"); }
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
        for d in results {
            let held = d.long_held == Some(true);
            let mut row = vec![s(iso(d.at)), d.acquired.map_or(Cell::Nil, |a| s(iso(a))), s(d.asset.clone()), n(&d.amount), cents(&d.proceeds)?,
                               cents(&d.cost_basis)?, cents(&d.gain_loss)?, s(currency), Cell::Raw(d.holding_days.to_string()), cents(&d.fee)?,
                               s(d.exchange.clone()), d.tx_id.clone().map_or(Cell::Nil, s), Cell::Raw(d.complete.to_string()), Cell::Raw(d.incomplete.to_string())];
            if j.holding_exemption.is_some() { row.push(Cell::Raw(held.to_string())); }
            if j.old_stock_cutoff.is_some() { row.push(Cell::Raw(held.to_string())); }
            if j.short_long_term { row.push(self.t(&format!("tax_report.values.term_{}", if held { "long" } else { "short" }))); }
            line(&mut out, &row);
        }
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
        self.block(&mut out, "WARNING", "deposit_basis_assumed", "deposit_basis_assumed_hint", deposits.into_iter())?;
        self.block(&mut out, "WARNING", "unlinked_withdrawals", "unlinked_withdrawals_hint",
                   enriched.iter().map(|e| &e.row).filter(|r| r.kind == Kind::Withdrawal && !r.linked && !fiat(&r.base)).map(|r| listed(r, &r.amount)))?;
        self.block(&mut out, "WARNING", "lost", "lost_hint",
                   enriched.iter().map(|e| &e.row).filter(|r| r.kind == Kind::Lost && !fiat(&r.base)).map(|r| listed(r, &r.amount)))?;
        let stated: Vec<&walk::Row> = enriched.iter().map(|e| &e.row).filter(|r| r.stated_value).collect();
        if !stated.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("NOTE: {}:", text(l, "tax_report.warnings.stated_values", &[])))]);
            for r in stated { line(&mut out, &[s(listed(r, &r.amount)), cents(&r.fiat_value)?]); }
            line(&mut out, &[self.t("tax_report.warnings.stated_values_hint")]);
        }
        self.block(&mut out, "WARNING", "unmatched_swaps", "unmatched_swaps_hint", unmatched.iter().map(|r| listed(r, &r.amount)))?;
        let unsynced = text(l, "tax_report.warnings.wallet_allocation_unsynced", &[]);
        let placed: Vec<String> = allocation.iter().map(|p| Ok::<_, FiguresError>(format!("{} {} {} {} {}", p.venue.as_deref().unwrap_or(&unsynced), p.symbol,
                                                     p.amount.to_s_f(), p.cost.round(2)?.to_s_f(), date(p.acquired)))).collect::<Result<_, _>>()?;
        self.block(&mut out, "NOTE", "wallet_allocation", "wallet_allocation_hint", placed.into_iter())?;
        let hint = if j.swaps_taxable() { "coin_fees_hint_disposal" } else { "coin_fees_hint_no_proceeds" };
        self.block(&mut out, "NOTE", "coin_fees", hint, standing_fees.iter().map(|r| listed(r, &r.amount)))?;
        Ok(out)
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

/// D-TAX-COIN-QUOTE: a trade of a coin for a coin booked as one buy or sell row quoted in the other coin (Bybit, MEXC,
/// Bitget, Gemini, BingX, Bitrue, Bitvavo, Hyperliquid, KuCoin; Binance and Coinbase store two swap legs) becomes, on this
/// report's copy of the rows, the two legs it is: the row's coin coming in or going out, and the quote coin the other
/// way, grouped. Rails read the row alone, so the quote coin never moved: the coin paid kept its lots, the coin received
/// had none, and where swaps are not taxable the trade was taxed as a sale. The fee goes with the leg of its own coin,
/// else with the coin coming in (as Binance books it). The new leg's id is the row's negated, unique and never stored.
fn split_coin_quoted_trades(rows: Vec<Stored>) -> Vec<Stored> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let Some((quote, paid)) = r.quote.clone().zip(r.quote_amount.clone()) else { out.push(r); continue };
        if !matches!(r.kind, Kind::Buy | Kind::Sell) || crate::tracker::cash(&quote) || quote.trim().is_empty() || r.group.is_some() { out.push(r); continue; }
        let buying = r.kind == Kind::Buy;
        let group = Some(format!("coin-quote:{}", r.id));
        let fee_on_quote = r.fee_currency.as_deref() == Some(quote.as_str()) || (!buying && r.fee_currency.as_deref() != Some(r.base.as_str()));
        let (fee_currency, fee_amount) = (r.fee_currency.clone(), r.fee_amount.clone());
        let other = Stored {
            id: -r.id, kind: if buying { Kind::SwapOut } else { Kind::SwapIn }, base: quote, amount: paid, quote: None, quote_amount: None,
            fee_currency: if fee_on_quote { fee_currency.clone() } else { None }, fee_amount: if fee_on_quote { fee_amount.clone() } else { None },
            tx_id: r.tx_id.as_ref().map(|t| format!("{t}-quote")), group: group.clone(), stated: None, linked_to: None, to_venue: None,
            to_exchange_id: None, base_asset: None, per_share: None, ..r.clone()
        };
        out.push(Stored { kind: if buying { Kind::SwapIn } else { Kind::SwapOut }, quote: None, quote_amount: None, group,
                          fee_currency: if fee_on_quote { None } else { fee_currency }, fee_amount: if fee_on_quote { None } else { fee_amount }, ..r });
        out.push(other);
    }
    out
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
