//! Tax reports (roadmap stage F). This build: the crypto report for the United States (Tax::GenerateReportJob#crypto_csv
//! with country 'US'), from the rows the tracker walk accepts (`tracker::rows::load`: Alpaca, quoted trades, dollars),
//! in US dollars, so no FX is involved. Its CSV is Rails' byte for byte, and so is its refusal of tokenized securities.
//!
//! Where Rails states a figure that cannot be defended, this build states none (FiguresError::NotComputed, the reason
//! in words): a sale spanning lots of more than one holding term, a holding term the day count gets wrong, a sale whose
//! tax year differs in the user's time zone, a sale of more than the history holds, a price that is missing, a fee paid
//! in a coin, a coin withdrawal not linked to its deposit, a lost coin, a corporate action. The reasons are recorded as
//! defects of the Rails report, to be fixed there first.
use crate::figures::{at::At, budget, dec::Dec, FiguresError};
use crate::tracker::prices::{self, alias_coin, catalogue_coin, Halt, PriceBook, Reference, Venue};
use crate::tracker::rows::{self, Kind, Stored};
use crate::tracker::{fiat, walk};
use crate::web::i18n::{text, Arg};
use chrono::{Datelike, Months, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet, VecDeque};

/// What the job leaves for the user: the report, or Rails' refusal (`<path>.refused.json`).
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome { Csv(String), Refused { reason: String, symbols: Vec<String> } }

/// Tax::Jurisdictions::REGISTRY['US']: FIFO, US dollars, English headers, short and long term.
const LOCALE: &str = "en";
const CURRENCY: &str = "USD";
const LONG_TERM_DAYS: i64 = 365;
/// Tax::Report::INCOME_TYPES, by entry type.
const INCOME: [(Kind, &str); 5] = [(Kind::StakingReward, "staking_reward"), (Kind::LendingInterest, "lending_interest"),
                                   (Kind::Airdrop, "airdrop"), (Kind::Mining, "mining"), (Kind::OtherIncome, "other_income")];

fn no_figure(why: &str) -> FiguresError { FiguresError::NotComputed(format!("the tax report states no figure for {why}")) }

/// Tax::GenerateReportJob#perform(user, country, year) for the crypto scope.
pub fn crypto_report(c: &Connection, user_id: i64, country: &str, year: i32, today: NaiveDate) -> Result<Outcome, FiguresError> {
    if country != "US" { return Err(FiguresError::NotComputed(format!("the tax report is not ported for the jurisdiction {country}"))); }
    budget::within(|| report(c, user_id, year, today))
}

fn report(c: &Connection, user_id: i64, year: i32, today: NaiveDate) -> Result<Outcome, FiguresError> {
    // rows::load sets the futures and margin wallets aside; the report reads every row, so it states nothing instead.
    let borrowed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND (tx_id LIKE '%futures%' \
                                      OR tx_id LIKE '%margin-interest%' OR tx_id LIKE '%liquidation-%'))", [user_id], |r| r.get(0))?;
    if borrowed { return Err(no_figure("rows of a futures or margin wallet")); }
    let venue = Venue::alpaca(c)?;
    let scope = CryptoScope::new(c, user_id)?;
    let mut rows = vec![];
    for r in rows::load(c, user_id)? { if scope.crypto(c, &r.base)? { rows.push(r); } }

    let next_year = Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).single().and_then(At::from_utc)
        .ok_or_else(|| FiguresError::Data("a report year out of range".into()))?;
    let tokenized = tokenized_symbols(c, &venue, rows.iter().filter(|r| r.at.0 < next_year.0))?;
    if !tokenized.is_empty() { return Ok(Outcome::Refused { reason: "tokenized_unsupported".into(), symbols: tokenized }); }
    let banners = sync_issues(c, user_id)?;
    // Rails files a sale under its UTC year, including one past the rows it reads; the user's year may be the other.
    let zone: String = c.query_row("SELECT time_zone FROM users WHERE id = ?1", [user_id], |r| r.get(0))?;
    for r in rows.iter().filter(|r| r.kind == Kind::Sell) {
        let (utc_year, local_year) = (r.at.utc().year(), crate::web::timezone::local(r.at.utc(), &zone).year());
        if utc_year != local_year && (utc_year == year || local_year == year) {
            return Err(no_figure("a sale whose tax year differs in the user's time zone from the UTC year Rails files it under"));
        }
    }

    // Every row up to the first instant of next year, for the basis; only this year's disposals are reported.
    let scoped = rows::ordered(rows.into_iter().filter(|r| r.at.0 <= next_year.0).collect());
    for r in &scoped {
        match r.kind {
            Kind::Fee => return Err(no_figure("a fee paid in a coin: Rails takes the basis of the coins it consumed out of the report")),
            Kind::Withdrawal if r.linked_to.is_none() => return Err(no_figure("a coin withdrawal not linked to its deposit: Rails keeps its lots for later sales")),
            Kind::Lost => return Err(no_figure("a lost coin: Rails keeps its lots for later sales")),
            Kind::Unsupported | Kind::Adjustment | Kind::ReturnOfCapital => return Err(no_figure("a corporate action, split or return of capital on a coin")),
            _ => {}
        }
    }

    let enriched = enrich(c, &venue, &scoped, today)?;
    let results: Vec<Disposal> = fifo(&enriched)?.into_iter().filter(|d| d.at.utc().year() == year).collect();
    for d in &results { check_term(d, &zone)?; }
    Ok(Outcome::Csv(csv(year, &banners, &results, &enriched)?))
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

/// Tax::Report#tokenized_symbols_in_scope: the wrappers the rows touch, by what the venue lists under the symbol, then by
/// the coin the symbol means; the catalogue's spelling, sorted.
fn tokenized_symbols<'a>(c: &Connection, venue: &Venue, rows: impl Iterator<Item = &'a Stored>) -> Result<Vec<String>, FiguresError> {
    let canonical = |s: &str| s.strip_suffix("SPV").map_or_else(|| s.to_string(), |head| format!("{head}x")).to_uppercase();
    let rows: Vec<&Stored> = rows.filter(|r| !r.base.trim().is_empty()).collect();
    let names: HashSet<String> = rows.iter().flat_map(|r| [r.base.clone(), canonical(&r.base)]).collect();
    let reference = Reference::load(c, venue, &names)?;
    let mut listed = c.prepare("SELECT a.symbol, a.instrument_type FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                                WHERE t.exchange_id = ?1 AND t.base IN (?2, ?3) ORDER BY t.id LIMIT 1")?;
    let mut wrapper = c.prepare("SELECT symbol FROM assets WHERE external_id = ?1 AND instrument_type = 'tokenized' LIMIT 1")?;
    let mut out = vec![];
    for r in rows {
        let venue_asset: Option<(Option<String>, Option<String>)> =
            listed.query_row(rusqlite::params![venue.id, r.base, canonical(&r.base)], |x| Ok((x.get(0)?, x.get(1)?))).optional()?;
        let found = match venue_asset {
            Some((symbol, Some(t))) if t == "tokenized" => symbol,
            _ => match alias_coin(&r.base, venue, Some(r.date()))
                .or_else(|| catalogue_coin(&reference, &r.base, venue))
                .or_else(|| catalogue_coin(&reference, &canonical(&r.base), venue)) {
                Some(coin) => wrapper.query_row([coin], |x| x.get::<_, Option<String>>(0)).optional()?.flatten(),
                None => None,
            },
        };
        if let Some(symbol) = found { if !out.contains(&symbol) { out.push(symbol); } }
    }
    out.sort();
    Ok(out)
}

/// ApiKey#sync_issue for every key but those of a stock venue other than Alpaca, as banner lines.
fn sync_issues(c: &Connection, user_id: i64) -> Result<Vec<String>, FiguresError> {
    let mut s = c.prepare("SELECT e.name, k.key_type, k.status, k.last_sync_error, k.last_synced_at FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
                           WHERE k.user_id = ?1 AND e.type IS NOT 'Exchanges::Ibkr' ORDER BY k.id")?;
    let keys = s.query_map([user_id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
                                               r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)))?;
    let prefix = text(LOCALE, "tax_report.incomplete_banner_prefix", &[]);
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
            out.push(format!("{prefix}: {}", text(LOCALE, &format!("tax_report.warnings.{reason}"), &[("exchange", Arg::Text(&name))])));
        }
    }
    Ok(out)
}

/// An enriched row (Tax::PriceService#enrich) with what the report reads beside the walk's.
struct Entry { row: walk::Row, tx_id: Option<String>, stated: bool }

/// Tax::PriceService#enrich(currency: 'USD'). Prices come from `historical_prices`; one Rails would fetch first is not
/// fetched here, and a missing one states no figure (Rails values it at zero and banners the report incomplete).
fn enrich(c: &Connection, venue: &Venue, scoped: &[Stored], today: NaiveDate) -> Result<Vec<Entry>, FiguresError> {
    // Only the prices the rows use: Rails' range prefetch adds days no row reads, and a day a row reads but the table
    // lacks halts the walk below.
    let reference = Reference::load(c, venue, &prices::symbols(scoped))?;
    let fetched = HashMap::new();
    let mut book = PriceBook { r: &reference, venue: venue.clone(), today, fetched: &fetched, warnings: 0 };
    let rows = match walk::enrich(scoped, &mut book) {
        Ok(rows) => rows,
        Err(Halt::Fetch(f)) => return Err(no_figure(&format!("no price of {} on {}", f.symbol, f.asked.unwrap_or(f.from)))),
        Err(Halt::Fail(FiguresError::NotComputed(m))) => return Err(no_figure(&m)),
        Err(Halt::Fail(e)) => return Err(e),
    };
    Ok(rows.into_iter().zip(scoped).map(|(row, s)| Entry { row, tx_id: s.tx_id.clone(), stated: s.stated.is_some() && !s.quoted() }).collect())
}

struct Lot { amount: Dec, cost_per_unit: Dec, at: At, assumed: bool }
struct Tranche { at: At }

/// One reported sale (Tax::Methods::Fifo#record_disposal), and the lots it took.
struct Disposal {
    at: At, acquired: Option<At>, asset: String, amount: Dec, proceeds: Dec, cost_basis: Dec, gain_loss: Dec,
    holding_days: i64, complete: bool, incomplete: bool, tx_id: Option<String>, tranches: Vec<Tranche>,
}

/// Tax::Methods::Fifo#calculate over the taxable rows (a fiat row reaches no engine), for the kinds this build reports.
fn fifo(entries: &[Entry]) -> Result<Vec<Disposal>, FiguresError> {
    let mut lots: HashMap<String, VecDeque<Lot>> = HashMap::new();
    let mut out = vec![];
    for e in entries.iter().filter(|e| !fiat(&e.row.base)) {
        budget::check()?;
        let r = &e.row;
        let open = |assumed: bool| -> Result<Lot, FiguresError> {
            // apply_acquisition_fee with no fee: the value plus a zero fee.
            let cost = (&r.fiat_value + &Dec::zero())?;
            let cost_per_unit = if r.amount.is_positive() { cost.div(&r.amount)? } else { Dec::zero() };
            Ok(Lot { amount: r.amount.clone(), cost_per_unit, at: r.at, assumed })
        };
        match r.kind {
            Kind::Buy | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => {
                let lot = open(r.price_missing)?;
                lots.entry(r.base.clone()).or_default().push_back(lot);
            }
            Kind::Deposit if r.linked => {}
            Kind::Deposit => { let lot = open(true)?; lots.entry(r.base.clone()).or_default().push_back(lot); }
            Kind::Sell => out.push(dispose(lots.entry(r.base.clone()).or_default(), e)?),
            _ => {}
        }
    }
    Ok(out)
}

fn dispose(lots: &mut VecDeque<Lot>, e: &Entry) -> Result<Disposal, FiguresError> {
    let r = &e.row;
    let has_lots = !lots.is_empty();
    let acquired = lots.front().map(|l| l.at);
    let mut held = Dec::zero();
    for l in lots.iter() { held = (&held + &l.amount)?; }
    if held < r.amount { return Err(no_figure("a sale of more than the history holds: Rails reports part or all of it at a zero basis")); }
    // dequeue_tranches.
    let (mut remaining, mut cost_basis, mut assumed, mut tranches) = (r.amount.clone(), Dec::zero(), false, vec![]);
    while remaining.is_positive() {
        budget::check()?;
        let Some(lot) = lots.front_mut() else { break };
        let take = if remaining < lot.amount { remaining.clone() } else { lot.amount.clone() };
        cost_basis = (&cost_basis + &(&take * &lot.cost_per_unit)?)?;
        assumed |= lot.assumed;
        tranches.push(Tranche { at: lot.at });
        if lot.amount <= remaining {
            remaining = (&remaining - &lot.amount)?;
            lots.pop_front();
        } else {
            lot.amount = (&lot.amount - &remaining)?;
            remaining = Dec::zero();
        }
    }
    let gain_loss = (&(&r.fiat_value - &cost_basis)? - &Dec::zero())?;
    // ((transacted_at - holding_ref_date) / 1.day).to_i: a Float of seconds, truncated.
    let holding_days = acquired.map_or(0, |a| (r.at.minus(a) / 86_400.0) as i64);
    Ok(Disposal { at: r.at, acquired, asset: r.base.clone(), amount: r.amount.clone(), proceeds: r.fiat_value.clone(), cost_basis, gain_loss,
                  holding_days, complete: has_lots, incomplete: r.price_missing || assumed || !has_lots, tx_id: e.tx_id.clone(), tranches })
}

/// Rails' term (`holding_days > 365`, decided by the oldest lot) against the calendar's for every lot the sale took, in
/// the user's time zone: long-term when sold after the anniversary of the acquisition day.
fn check_term(d: &Disposal, zone: &str) -> Result<(), FiguresError> {
    let day = |at: At| crate::web::timezone::local(at.utc(), zone).date_naive();
    let sold = day(d.at);
    let terms: Vec<bool> = d.tranches.iter().map(|t| day(t.at).checked_add_months(Months::new(12)).is_none_or(|anniversary| sold > anniversary)).collect();
    if terms.windows(2).any(|w| w[0] != w[1]) { return Err(no_figure("a sale of lots of more than one holding term: Rails reports it as one row of the oldest lot's term")); }
    if terms.first().is_some_and(|&long| long != (d.holding_days > LONG_TERM_DAYS)) {
        return Err(no_figure("a holding term the day count gets wrong: Rails counts 365 whole days, not the anniversary"));
    }
    Ok(())
}

/// A cell as Ruby's CSV writes it through CsvSafe: a String (escaped when it reads as a formula after Ruby's lstrip, which
/// takes a NUL too; `ruby::strip` answers the same, as what lstrip leaves never starts with what strip removes), anything
/// else as
/// its `to_s`, or nil.
enum Cell { Text(String), Raw(String), Nil }

fn t(key: &str) -> Cell { Cell::Text(text(LOCALE, key, &[])) }
fn s(v: impl Into<String>) -> Cell { Cell::Text(v.into()) }
fn n(v: &Dec) -> Cell { Cell::Raw(v.to_s_f()) }
fn cents(v: &Dec) -> Result<Cell, FiguresError> { Ok(Cell::Raw(v.round(2)?.to_s_f())) }
fn iso(at: At) -> String { at.utc().format("%Y-%m-%dT%H:%M:%SZ").to_string() }
fn date(at: At) -> String { at.utc().format("%Y-%m-%d").to_string() }

fn line(out: &mut String, cells: &[Cell]) {
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

/// Tax::Report#to_csv for 'US' when no price is missing.
fn csv(year: i32, banners: &[String], results: &[Disposal], enriched: &[Entry]) -> Result<String, FiguresError> {
    let mut out = String::new();
    let headers = ["date", "acquisition_date", "asset", "amount", "proceeds", "cost_basis", "gain_loss", "currency", "holding_days", "fee",
                   "exchange", "tx_id", "cost_basis_complete", "data_incomplete", "term"];
    line(&mut out, &headers.map(|k| t(&format!("tax_report.headers.{k}"))));
    for b in banners { line(&mut out, &[s(b.clone())]); }
    if results.is_empty() { line(&mut out, &[t("tax_report.no_taxable_transactions")]); }
    for d in results {
        let term = if d.holding_days > LONG_TERM_DAYS { "long" } else { "short" };
        line(&mut out, &[s(iso(d.at)), d.acquired.map_or(Cell::Nil, |a| s(iso(a))), s(d.asset.clone()), n(&d.amount), cents(&d.proceeds)?,
                         cents(&d.cost_basis)?, cents(&d.gain_loss)?, s(CURRENCY), Cell::Raw(d.holding_days.to_string()), cents(&Dec::zero())?,
                         s(crate::tracker::VENUE), d.tx_id.clone().map_or(Cell::Nil, s), Cell::Raw(d.complete.to_string()),
                         Cell::Raw(d.incomplete.to_string()), t(&format!("tax_report.values.term_{term}"))]);
    }
    // Income is read from every enriched row of the year (a fiat-base dividend included), valued as enriched.
    let income: Vec<(&Entry, &str)> = enriched.iter()
        .filter_map(|e| INCOME.iter().find(|(k, _)| *k == e.row.kind).map(|(_, name)| (e, *name)))
        .filter(|(e, _)| e.row.at.utc().year() == year).collect();
    if !income.is_empty() {
        line(&mut out, &[]);
        line(&mut out, &[t("tax_report.income.header")]);
        for (e, name) in &income {
            line(&mut out, &[s(date(e.row.at)), t(&format!("tax_report.income.types.{name}")), s(e.row.base.clone()), n(&e.row.amount),
                             cents(&e.row.fiat_value)?, s(CURRENCY)]);
        }
        let mut kinds: Vec<&str> = vec![];
        for (_, name) in &income { if !kinds.contains(name) { kinds.push(name); } }
        for name in kinds {
            let mut total = Dec::zero();
            for (e, _) in income.iter().filter(|(_, k)| *k == name) { total = (&total + &e.row.fiat_value)?; }
            let label = text(LOCALE, "tax_report.income.total", &[("type", Arg::Text(&text(LOCALE, &format!("tax_report.income.types.{name}"), &[])))]);
            line(&mut out, &[Cell::Nil, s(label), Cell::Nil, Cell::Nil, cents(&total)?, s(CURRENCY)]);
        }
    }
    let assumed: Vec<&Entry> = enriched.iter().filter(|e| e.row.kind == Kind::Deposit && !e.row.linked && !fiat(&e.row.base)).collect();
    if !assumed.is_empty() {
        line(&mut out, &[]);
        line(&mut out, &[s(format!("WARNING: {}:", text(LOCALE, "tax_report.warnings.deposit_basis_assumed", &[])))]);
        for e in assumed { line(&mut out, &[s(format!("{} {} {}", e.row.base, e.row.amount.to_s_f(), date(e.row.at)))]); }
        line(&mut out, &[t("tax_report.warnings.deposit_basis_assumed_hint")]);
    }
    let stated: Vec<&Entry> = enriched.iter().filter(|e| e.stated).collect();
    if !stated.is_empty() {
        line(&mut out, &[]);
        line(&mut out, &[s(format!("NOTE: {}:", text(LOCALE, "tax_report.warnings.stated_values", &[])))]);
        for e in stated { line(&mut out, &[s(format!("{} {} {}", e.row.base, e.row.amount.to_s_f(), date(e.row.at))), cents(&e.row.fiat_value)?]); }
        line(&mut out, &[t("tax_report.warnings.stated_values_hint")]);
    }
    Ok(out)
}
