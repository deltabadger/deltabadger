//! Tax reports (roadmap stage F). This build: the crypto report for the United States (Tax::GenerateReportJob#crypto_csv
//! with country 'US'), for one Alpaca account of quoted trades in dollars (`tracker::rows::alpaca_sweep_domain`),
//! in US dollars, so no FX is involved. Its CSV is Rails' byte for byte, and so is its refusal of tokenized securities.
//!
//! Where Rails states a figure that cannot be defended, this build states none (FiguresError::NotComputed, the reason
//! in words): a sale of more than the history holds, a price that is missing, a coin fee that belongs to no fill, a coin
//! withdrawal not linked to its deposit, a lost coin, a corporate action. The reasons are recorded as defects of the
//! Rails report, to be fixed there first.
//!
//! Sales are split into one row per holding term, the term read on the calendar in the user's time zone, the tax year
//! is the user's, and an Alpaca CFEE is folded into that day's fills (Tax::Report#holding_rule, #attribute_trade_fees).
use crate::figures::{at::At, budget, dec::Dec, FiguresError};
use crate::tracker::prices::{self, alias_coin, catalogue_coin, PriceService, Reference, Unfetched, Venue};
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
    // The walk sets the futures and margin wallets aside; the report reads every row, so it states nothing instead.
    let borrowed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND (tx_id LIKE '%futures%' \
                                      OR tx_id LIKE '%margin-interest%' OR tx_id LIKE '%liquidation-%'))", [user_id], |r| r.get(0))?;
    if borrowed { return Err(no_figure("rows of a futures or margin wallet")); }
    let venue = Venue::alpaca(c)?;
    let scope = CryptoScope::new(c, user_id)?;
    // The rows of the walk Plan D4a ported (one Alpaca account in dollars); anything else is refused, as it was.
    let all = rows::load_all(c, user_id)?;
    rows::alpaca_sweep_domain(c, user_id, &all).map_err(|why| crate::tracker::refused(&why))?;
    let mut rows = vec![];
    for r in rows::walk_rows(&all)? { if scope.crypto(c, &r.base)? { rows.push(r); } }

    let zone: String = c.query_row("SELECT time_zone FROM users WHERE id = ?1", [user_id], |r| r.get(0))?;
    // Tax::Report#rows_until: the end of the year in the user's zone, or in UTC where that is later.
    let utc_end = Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).single();
    let local_end = crate::web::timezone::strict(&zone).unwrap_or(chrono_tz::Tz::UTC).with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).earliest()
        .map(|t| t.with_timezone(&Utc));
    let rows_until = utc_end.zip(local_end).map(|(u, l)| u.max(l)).and_then(At::from_utc)
        .ok_or_else(|| FiguresError::Data("a report year out of range".into()))?;
    let tokenized = tokenized_symbols(c, &venue, rows.iter().filter(|r| r.at.0 < rows_until.0))?;
    if !tokenized.is_empty() { return Ok(Outcome::Refused { reason: "tokenized_unsupported".into(), symbols: tokenized }); }
    let banners = sync_issues(c, user_id)?;

    // Every row up to the end of the year, for the basis; only this year's disposals are reported.
    let scoped = rows::ordered(rows.into_iter().filter(|r| r.at.0 <= rows_until.0).collect());
    for r in &scoped {
        match r.kind {
            Kind::Withdrawal if r.linked_to.is_none() => return Err(no_figure("a coin withdrawal not linked to its deposit: Rails keeps its lots for later sales")),
            Kind::Lost => return Err(no_figure("a lost coin: Rails keeps its lots for later sales")),
            Kind::Unsupported | Kind::Adjustment | Kind::ReturnOfCapital => return Err(no_figure("a corporate action, split or return of capital on a coin")),
            _ => {}
        }
    }

    let mut enriched = enrich(c, &venue, &scoped, today)?;
    let folded = attribute_trade_fees(c, &mut enriched)?;
    let local = |at: At| crate::web::timezone::local(at.utc(), &zone).date_naive();
    let results: Vec<Disposal> = fifo(&enriched, &folded, &local)?.into_iter().filter(|d| local(d.at).year() == year).collect();
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
            _ => match alias_coin(&r.base, Some(venue), Some(r.date()))
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
struct Entry { id: i64, row: walk::Row, tx_id: Option<String>, stated: bool }

/// Tax::PriceService#enrich(currency: 'USD'). Prices come from `historical_prices`; one Rails would fetch first is not
/// fetched here, and a missing one states no figure (Rails values it at zero and banners the report incomplete).
fn enrich(c: &Connection, venue: &Venue, scoped: &[Stored], today: NaiveDate) -> Result<Vec<Entry>, FiguresError> {
    // Only the prices the rows use: Rails' range prefetch adds days no row reads, and a day a row reads but the table
    // lacks halts the walk below.
    let reference = Reference::load(c, venue, &prices::symbols(scoped))?;
    let fx = crate::tracker::fx::FxTable::load(c)?;
    let mut none = Unfetched;
    let mut ps = PriceService::new(&reference, &fx, today, &mut none);
    let deposits: HashMap<i64, Dec> = scoped.iter().map(|r| (r.id, r.amount.clone())).collect();
    let rows = walk::enrich_with(scoped, &deposits, &mut ps, false)?;
    // A price the table lacks: the first one Rails would have valued at zero ("BTC/USD 2026-09-10").
    if let Some(w) = ps.warnings.first() {
        let (asset, day) = w.split_once(' ').map_or((w.as_str(), ""), |(a, d)| (a.split('/').next().unwrap_or(a), d));
        return Err(no_figure(&format!("no price of {asset} on {day}")));
    }
    Ok(rows.into_iter().zip(scoped).map(|(row, s)| Entry { id: s.id, row, tx_id: s.tx_id.clone(), stated: s.stated.is_some() && !s.quoted() }).collect())
}

/// Tax::Report#attribute_trade_fees: an Alpaca CFEE (a coin fee of its own row, dated to the day) goes to that day's fills
/// of the coin when the UTC and the New York calendars give the same fills, buys first (fewer coins, same cost), else
/// sales (more coins out, same proceeds), pro rata by amount, the last fill taking what is left. Returns the ids of the
/// fees folded away; one left standing is refused by the engine.
fn attribute_trade_fees(c: &Connection, entries: &mut [Entry]) -> Result<HashSet<i64>, FiguresError> {
    let mut s = c.prepare("SELECT id FROM account_transactions WHERE json_extract(raw_data, '$.activity_type') = 'CFEE'")?;
    let cfee: HashSet<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let new_york = |at: At| at.utc().with_timezone(&chrono_tz::America::New_York).date_naive();
    let mut folded = HashSet::new();
    for i in 0..entries.len() {
        let fee = &entries[i];
        if fiat(&fee.row.base) || fee.row.kind != Kind::Fee || !cfee.contains(&fee.id) || !fee.row.amount.is_positive() { continue; }
        let (day, base, amount) = (fee.row.at.utc().date_naive(), fee.row.base.clone(), fee.row.amount.clone());
        let same_coin: Vec<usize> = (0..entries.len()).filter(|&j| {
            let r = &entries[j].row;
            matches!(r.kind, Kind::Buy | Kind::Sell) && !fiat(&r.base) && r.base == base && r.amount.is_positive()
        }).collect();
        let fills: Vec<usize> = same_coin.iter().copied().filter(|&j| entries[j].row.at.utc().date_naive() == day).collect();
        let new_york_fills: Vec<usize> = same_coin.iter().copied().filter(|&j| new_york(entries[j].row.at) == day).collect();
        if fills != new_york_fills { continue; }
        let buys: Vec<usize> = fills.iter().copied().filter(|&j| entries[j].row.kind == Kind::Buy).collect();
        let fills = if buys.is_empty() { fills } else { buys };
        if fills.is_empty() { continue; }
        let mut total = Dec::zero();
        for &j in &fills { total = (&total + &entries[j].row.amount)?; }
        let mut left = amount.clone();
        for (n, &j) in fills.iter().enumerate() {
            let r = &mut entries[j].row;
            let share = if n == fills.len() - 1 { left.clone() } else { (&amount * &r.amount)?.div(&total)? };
            left = (&left - &share)?;
            r.amount = if r.kind == Kind::Buy { let net = (&r.amount - &share)?; if net.is_negative() { Dec::zero() } else { net } } else { (&r.amount + &share)? };
        }
        folded.insert(entries[i].id);
    }
    Ok(folded)
}

struct Lot { amount: Dec, cost_per_unit: Dec, at: At, assumed: bool }
struct Tranche { amount: Dec, cost: Dec, at: At, assumed: bool }

/// One reported row of a sale (Tax::Methods::Fifo#disposal_row): one per holding term.
struct Disposal {
    at: At, acquired: Option<At>, asset: String, amount: Dec, proceeds: Dec, cost_basis: Dec, fee: Dec, gain_loss: Dec,
    holding_days: i64, complete: bool, incomplete: bool, tx_id: Option<String>, long_held: bool,
}

/// Tax::Methods::Fifo#calculate over the taxable rows (a fiat row reaches no engine; a folded fee is gone), for the
/// kinds this build reports, with the US holding rule.
fn fifo(entries: &[Entry], folded: &HashSet<i64>, local: &dyn Fn(At) -> NaiveDate) -> Result<Vec<Disposal>, FiguresError> {
    let mut lots: HashMap<String, VecDeque<Lot>> = HashMap::new();
    let mut out = vec![];
    for e in entries.iter().filter(|e| !fiat(&e.row.base) && !folded.contains(&e.id)) {
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
            Kind::Sell => out.extend(dispose(lots.entry(r.base.clone()).or_default(), e, local)?),
            Kind::Fee => return Err(no_figure("a fee paid in a coin that belongs to no fill: Rails takes the basis of the coins it consumed out of the report")),
            _ => {}
        }
    }
    Ok(out)
}

/// Jurisdictions.held_long?(acquired, disposed, 1.year): a date after the anniversary, 29 February's being 28 February.
fn held_long(acquired: NaiveDate, disposed: NaiveDate) -> bool {
    acquired.checked_add_months(Months::new(12)).is_none_or(|anniversary| disposed > anniversary)
}

/// Fifo#record_disposal_by_term: the lots taken, grouped by term in the order first met, each group its own row with
/// its share of the proceeds and the fee; more than one row is settled in cents (`settle_in_cents`).
fn dispose(lots: &mut VecDeque<Lot>, e: &Entry, local: &dyn Fn(At) -> NaiveDate) -> Result<Vec<Disposal>, FiguresError> {
    let r = &e.row;
    let has_lots = !lots.is_empty();
    let mut held = Dec::zero();
    for l in lots.iter() { held = (&held + &l.amount)?; }
    if held < r.amount { return Err(no_figure("a sale of more than the history holds: Rails reports part or all of it at a zero basis")); }
    // dequeue_tranches.
    let (mut remaining, mut tranches) = (r.amount.clone(), vec![]);
    while remaining.is_positive() {
        budget::check()?;
        let Some(lot) = lots.front_mut() else { break };
        let take = if remaining < lot.amount { remaining.clone() } else { lot.amount.clone() };
        tranches.push(Tranche { cost: (&take * &lot.cost_per_unit)?, amount: take, at: lot.at, assumed: lot.assumed });
        if lot.amount <= remaining {
            remaining = (&remaining - &lot.amount)?;
            lots.pop_front();
        } else {
            lot.amount = (&lot.amount - &remaining)?;
            remaining = Dec::zero();
        }
    }
    let sold = local(r.at);
    let mut groups: Vec<(bool, Vec<&Tranche>)> = vec![];
    for t in &tranches {
        let long = held_long(local(t.at), sold);
        match groups.iter_mut().find(|(k, _)| *k == long) { Some((_, g)) => g.push(t), None => groups.push((long, vec![t])) }
    }
    // Covered in full (above), so the uncovered units the short side takes are a zero, added as Rails adds it; a sale of
    // nothing is one short row.
    let mut taken = Dec::zero();
    for t in &tranches { taken = (&taken + &t.amount)?; }
    let uncovered = (&r.amount - &taken)?;
    if groups.is_empty() { groups.push((false, vec![])); }
    let fee = Dec::zero();
    let mut out = vec![];
    for (long_held, group) in &groups {
        let mut amount = Dec::zero();
        let mut cost_basis = Dec::zero();
        for t in group { amount = (&amount + &t.amount)?; cost_basis = (&cost_basis + &t.cost)?; }
        if !long_held { amount = (&amount + &uncovered)?; }
        let proceeds = (&r.fiat_value * &amount)?.div(&r.amount)?;
        let group_fee = (&fee * &amount)?.div(&r.amount)?;
        let acquired = group.first().map(|t| t.at);
        let assumed = group.iter().any(|t| t.assumed);
        // ((transacted_at - holding_ref_date) / 1.day).to_i: a Float of seconds, truncated.
        let holding_days = acquired.map_or(0, |a| (r.at.minus(a) / 86_400.0) as i64);
        out.push(Disposal { at: r.at, acquired, asset: r.base.clone(), gain_loss: (&(&proceeds - &cost_basis)? - &group_fee)?, amount, proceeds,
                            cost_basis, fee: group_fee, holding_days, complete: has_lots, incomplete: r.price_missing || assumed || !has_lots,
                            tx_id: e.tx_id.clone(), long_held: *long_held });
    }
    if out.len() > 1 { settle_in_cents(&mut out, &r.fiat_value, &fee)?; }
    Ok(out)
}

/// Fifo#settle_in_cents: every row but the last cut to the cent, the last taking what is left of the sale (its gain what
/// is left of the sale's gain as printed), so the printed rows add up to the printed sale.
fn settle_in_cents(rows: &mut [Disposal], proceeds: &Dec, fee: &Dec) -> Result<(), FiguresError> {
    let mut basis = Dec::zero();
    for row in rows.iter() { basis = (&basis + &row.cost_basis)?; }
    let mut left = [proceeds.clone(), fee.clone(), basis.clone(), (&(proceeds - &basis)? - fee)?.round(2)?];
    let count = rows.len();
    for (n, row) in rows.iter_mut().enumerate() {
        if n == count - 1 {
            row.proceeds = left[0].clone(); row.fee = left[1].clone(); row.cost_basis = left[2].clone(); row.gain_loss = left[3].clone();
        } else {
            row.proceeds = row.proceeds.round(2)?; row.fee = row.fee.round(2)?; row.cost_basis = row.cost_basis.round(2)?;
            row.gain_loss = (&(&row.proceeds - &row.cost_basis)? - &row.fee)?;
        }
        let taken = [&row.proceeds, &row.fee, &row.cost_basis, &row.gain_loss];
        for (l, t) in left.iter_mut().zip(taken) { *l = (&*l - t)?; }
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
        let term = if d.long_held { "long" } else { "short" };
        line(&mut out, &[s(iso(d.at)), d.acquired.map_or(Cell::Nil, |a| s(iso(a))), s(d.asset.clone()), n(&d.amount), cents(&d.proceeds)?,
                         cents(&d.cost_basis)?, cents(&d.gain_loss)?, s(CURRENCY), Cell::Raw(d.holding_days.to_string()), cents(&d.fee)?,
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
