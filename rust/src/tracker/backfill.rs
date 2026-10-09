//! PortfolioSnapshot::BackfillJob (app/jobs/portfolio_snapshot/backfill_job.rb): one forward sweep from the first
//! transaction to yesterday, valuing what was held at the end of each day at that day's close, with money in read
//! term by term off the ledger walk. Rewrites every earlier day of both tables and stamps the history's version.
use super::prices::{self, coin_ids_over, Fetch, Venue};
use super::rows::{Kind, Stored};
use super::snapshot::{upsert_whole, Day};
use super::walk::Term;
use super::{cash, fiat, stable};
use crate::crypto::Cipher;
use crate::figures::{dec::Dec, FiguresError};
use chrono::{Datelike, Days, NaiveDate};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};

/// How far a last observed price is carried over a hole (CARRY_LIMIT).
pub const CARRY_LIMIT: usize = 7;
/// The categories whose prices live under `stock:` and come from the broker's own daily bars.
const STOCK_CATEGORIES: [&str; 4] = ["Stock", "Common Stock", "ETF", "Fund"];
/// PortfolioSnapshot::HISTORY_VERSION.
const HISTORY_VERSION: i64 = 1;

pub fn history_key(user_id: i64) -> String { format!("snapshot_history_version_{user_id}") }
pub fn price_key(user_id: i64) -> String { format!("snapshot_price_generation_{user_id}") }

/// `PortfolioSnapshot.history_version`: what a stored history was swept from (every row, today's included).
pub fn history_version(c: &Connection, user_id: i64) -> Result<String, FiguresError> {
    let (count, latest): (i64, Option<String>) = c.query_row("SELECT count(*), max(updated_at) FROM account_transactions WHERE user_id = ?1", [user_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let latest = latest.map(|t| crate::codec::parse_time(&t).map(|t| t.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()))
        .transpose().map_err(|e| FiguresError::Data(format!("{e:?}")))?;
    Ok(format!("{HISTORY_VERSION}_{count}_{}", latest.unwrap_or_default()))
}

/// `HistoricalPrice.generation`.
pub fn generation(c: &Connection) -> Result<i64, FiguresError> {
    Ok(c.query_row("SELECT coalesce(max(id), 0) FROM historical_prices", [], |r| r.get(0))?)
}

/// Whether a sweep is wanted, as the tracker page asks for one (`load_history`), for either setting of its cash
/// switch and for each scope it can show (the whole account, and each venue with rows, its own history in
/// `portfolio_venue_snapshots`): a day in the window was never swept, the transactions moved since the last sweep, or a
/// partial day may now be priced. The window is what a sweep writes: the scope's first transaction to yesterday.
/// Stricter than the page: any day of that window missing is wanted (an install offline for a week leaves that week
/// out, a venue's row can be missing beside the whole account's, and the page, which reads what is stored, notices
/// neither). A row outside the window, unswept, missing or partial, never asks for one: no sweep can reach it.
pub fn wanted(c: &Connection, cipher: &Cipher, user_id: i64, today: NaiveDate) -> Result<bool, FiguresError> {
    let yesterday = today.pred_opt().unwrap_or(today);
    let date = |t: Option<String>| t.map(|t| crate::codec::parse_time(&t).map(|t| t.date_naive())).transpose().map_err(|e| FiguresError::Data(format!("{e:?}")));
    // (day, never swept, partial)
    let days = |sql: &str, scope: Option<i64>| -> Result<Vec<(NaiveDate, bool, bool)>, FiguresError> {
        let mut s = c.prepare(sql)?;
        let days: Vec<(String, bool, bool)> = s.query_map(rusqlite::params![user_id, scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        Ok(days.into_iter().filter_map(|(d, unswept, partial)| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok().map(|d| (d, unswept, partial))).collect())
    };
    // (scope, its first transaction): the whole account, then each venue with rows.
    let mut scopes: Vec<(Option<i64>, Option<NaiveDate>)> =
        vec![(None, date(c.query_row("SELECT min(transacted_at) FROM account_transactions WHERE user_id = ?1", [user_id], |r| r.get(0))?)?)];
    let mut s = c.prepare("SELECT exchange_id, min(transacted_at) FROM account_transactions WHERE user_id = ?1 GROUP BY exchange_id ORDER BY exchange_id")?;
    for venue in s.query_map([user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? { let (id, first) = venue?; scopes.push((Some(id), date(first)?)); }
    // `stale_prices?`: a partial whole-account day a sweep can reach.
    let mut partial = false;
    for (scope, first) in scopes {
        let Some(first) = first.filter(|f| *f <= yesterday) else { continue };
        let days = match scope {
            None => days("SELECT date, held_cost_usd IS NULL, partial FROM portfolio_snapshots WHERE user_id = ?1 AND ?2 IS NULL ORDER BY date", None)?,
            Some(id) => days("SELECT date, held_cost_usd IS NULL, partial FROM portfolio_venue_snapshots WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY date", Some(id))?,
        };
        let window: Vec<&(NaiveDate, bool, bool)> = days.iter().filter(|(d, _, _)| first <= *d && *d <= yesterday).collect();
        crate::figures::budget::charge(days.len() as u64, 0)?;
        if window.iter().any(|(_, unswept, _)| *unswept) || (window.len() as i64) < prices::span(first, yesterday) { return Ok(true); }
        partial |= scope.is_none() && window.iter().any(|(_, _, p)| *p);
    }
    if crate::app_config::get(c, cipher, &history_key(user_id)).map_err(FiguresError::Data)? != Some(history_version(c, user_id)?) { return Ok(true); }
    Ok(partial && crate::app_config::get(c, cipher, &price_key(user_id)).map_err(FiguresError::Data)? != Some(generation(c)?.to_string()))
}

/// One instrument of `load_prices`: the key its prices are stored under, the symbol, whether it is a stock, the first
/// day it was touched, and for a coin the one coin its identity names (its external id), if one is named.
#[derive(Clone, Debug)]
pub struct Instrument { pub key: String, pub symbol: String, pub stock: bool, pub from: NaiveDate, pub coin: Option<Option<String>> }

/// `identity_of` on the stock venue: Some((true, None)) a stock, Some((false, the one coin named)) a coin, None a name
/// that is both. The assets the venue's rows recorded under the symbol decide; with none recorded, what the venue lists
/// under it, then the catalogue.
fn identity_of(c: &Connection, user_id: i64, venue: &Venue, symbol: &str) -> Result<Option<(bool, Option<Option<String>>)>, FiguresError> {
    type Candidate = (i64, Option<String>, Option<String>);
    let read = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> Result<Vec<Candidate>, FiguresError> {
        let mut s = c.prepare_cached(sql)?;
        let mut q = s.query(params)?;
        let mut out: Vec<Candidate> = vec![];
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let asset: Candidate = (row.get(0)?, row.get(1)?, row.get(2)?);
            if !out.iter().any(|(id, _, _)| *id == asset.0) { out.push(asset); }
        }
        Ok(out)
    };
    let mut candidates = read("SELECT a.id, a.category, a.external_id FROM account_transactions t JOIN assets a ON a.id = t.base_asset_id \
                               WHERE t.user_id = ?1 AND t.exchange_id = ?2 AND t.base_currency = ?3 ORDER BY t.id", rusqlite::params![user_id, venue.id, symbol])?;
    if candidates.is_empty() {
        candidates = read("SELECT a.id, a.category, a.external_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                           WHERE t.exchange_id = ?1 AND t.base = ?2 ORDER BY t.id", rusqlite::params![venue.id, symbol])?;
    }
    if candidates.is_empty() { candidates = read("SELECT id, category, external_id FROM assets WHERE symbol = ?1 ORDER BY id", rusqlite::params![symbol])?; }
    let (stocks, coins): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|(_, category, _)| category.as_deref().is_some_and(|c| STOCK_CATEGORIES.contains(&c)));
    Ok(match (stocks.is_empty(), coins.len()) {
        (true, 1) => Some((false, Some(coins[0].2.clone()))),
        (true, _) => Some((false, None)),
        (false, 0) => Some((true, None)),
        _ => None,
    })
}

/// The coin a symbol means over each stretch of the window: the one its identity named, else Tax::AssetIdentity's.
fn coins_of(reference: &prices::Reference, venue: &Venue, i: &Instrument, last: NaiveDate) -> Vec<(NaiveDate, NaiveDate, String)> {
    match &i.coin {
        Some(named) => named.iter().map(|coin| (i.from, last, coin.clone())).collect(),
        None => coin_ids_over(reference, &i.symbol, venue, i.from, last),
    }
}

/// A stock's closes to fetch from the broker (`stock_price_range`): the key to read them with, and the window.
#[derive(Clone, Debug)]
pub struct Bars { pub api_key_id: i64, pub symbol: String, pub from: NaiveDate, pub to: NaiveDate }

/// One fetch of `load_prices`: a coin's range from data-api, or a stock's closes from the broker.
#[derive(Clone, Debug)]
pub enum Close { Range(Fetch), Bars(Bars) }

/// What `load_prices` fetches before it reads, instrument by instrument.
#[derive(Clone, Debug, Default)]
pub struct Plan { pub instruments: Vec<Instrument>, pub fetches: Vec<Close> }

/// `touched` and the instruments over the account's rows: per symbol (cash left out) the first day it was touched,
/// keyed `stock:SYM` when its identity (`identity_of`) is a stock. A name that is both a stock and a coin has no
/// instrument: its days are unpriced.
pub fn plan(c: &Connection, user_id: i64, rows: &[Stored], first: NaiveDate, last: NaiveDate) -> Result<Plan, FiguresError> {
    let venue = Venue::alpaca(c)?;
    // The catalogue and the stored closes, read once; the venue's key, once.
    let reference = prices::Reference::load(c, &venue, &prices::symbols(rows))?;
    let key: Option<i64> = c.query_row("SELECT id FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY id LIMIT 1", [user_id, venue.id], |r| r.get(0)).optional()?;
    let mut plan = Plan::default();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for r in rows {
        crate::figures::budget::charge(1, 0)?;
        let date = r.date().max(first);
        for symbol in [Some(r.base.as_str()), r.quote.as_deref()].into_iter().flatten() {
            if fiat(symbol) || stable(symbol) || !seen.insert(symbol) { continue; }
            let Some((stock, coin)) = identity_of(c, user_id, &venue, symbol)? else { continue };
            let key = if stock { format!("stock:{symbol}") } else { symbol.to_string() };
            plan.instruments.push(Instrument { key, symbol: symbol.into(), stock, from: date, coin });
        }
    }
    for i in &plan.instruments {
        crate::figures::budget::charge(1, 0)?;
        if !i.stock {
            for (from, to, coin) in coins_of(&reference, &venue, i, last) {
                plan.fetches.push(Close::Range(Fetch { coin, symbol: i.symbol.clone(), from, to, asked: None }));
            }
            continue;
        }
        // `stock_price_range`: fetch only when nothing is stored from the window's last weekday on, and only with a key
        // of the venue and a ticker of the symbol.
        let last_weekday = (0..prices::span(i.from, last)).map(|n| last.checked_sub_days(Days::new(n as u64)).unwrap_or(last))
            .find(|d| (1..=5).contains(&d.weekday().number_from_sunday().saturating_sub(1))).unwrap_or(last);
        // What the venue lists under the symbol is checked before any stored close is used: a coin whose symbol the
        // catalogue also has as a stock would otherwise be valued at the stock's closes.
        let ticker = reference.listed_category(&i.symbol);
        if ticker == Some(Some("Cryptocurrency")) { return Err(super::refused("a stock whose ticker on Alpaca is a coin's")); }
        let have = reference.prices_over(&i.key, last_weekday, last).next().is_some();
        if have || ticker.is_none() { continue; }
        let Some(api_key_id) = key else { continue };
        plan.fetches.push(Close::Bars(Bars { api_key_id, symbol: i.symbol.clone(), from: i.from, to: last }));
    }
    Ok(plan)
}

/// The closes a bar answer gives for the window, as `stock_price_range` stores them: the days not yet stored. None
/// when a bar cannot be read, which Rails rescues into storing nothing.
pub fn bar_rows(body: &serde_json::Value, bars: &Bars, have: &std::collections::HashSet<NaiveDate>) -> Option<Vec<prices::PriceRow>> {
    let mut candles = vec![];
    for bar in body["bars"].as_array().into_iter().flatten() {
        let day = bar["t"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.naive_utc().date())?;
        candles.push((day, Dec::to_d(&bar["c"]).ok()?));
    }
    let mut seen = have.clone();
    Some(candles.into_iter().filter(|(day, _)| *day >= bars.from && *day <= bars.to && seen.insert(*day))
        .map(|(day, close)| (format!("stock:{}", bars.symbol), day, close.to_s_f())).collect())
}

/// price key → day → the price that day, the last observed carried at most CARRY_LIMIT days and never across a day
/// the symbol changed coin. Sparse: a day with no price has no entry, so what is held is the prices, which the budget
/// meters; every day walked is a step.
fn prices_by_day(c: &Connection, plan: &Plan, last: NaiveDate) -> Result<HashMap<String, BTreeMap<NaiveDate, Dec>>, FiguresError> {
    let venue = Venue::alpaca(c)?;
    // The catalogue and every stored close of the instruments, read once.
    let reference = prices::Reference::load(c, &venue, &plan.instruments.iter().map(|i| i.symbol.clone()).collect())?;
    let mut out = HashMap::new();
    for i in &plan.instruments {
        // ponytail: `coins` holds one stretch per dated alias of the symbol (ALIASES has eight), so this find is bounded.
        let coins = if i.stock { vec![] } else { coins_of(&reference, &venue, i, last) };
        let coin_on = |d: NaiveDate| if i.stock { Some(i.key.clone()) } else { coins.iter().find(|(f, l, _)| *f <= d && d <= *l).map(|(_, _, c)| c.clone()) };
        let observed: HashMap<&NaiveDate, &Dec> = reference.prices_over(&i.key, i.from, last).collect();
        let (mut latest, mut carried, mut days) = (None::<Dec>, 0usize, BTreeMap::new());
        let mut day = i.from;
        while day <= last {
            crate::figures::budget::charge(1, 0)?;
            if day > i.from && coin_on(day) != coin_on(day.pred_opt().unwrap_or(day)) { latest = None; }
            carried = if observed.contains_key(&day) { 0 } else { carried + 1 };
            if let Some(p) = observed.get(&day) { latest = Some((*p).clone()); }
            if let (true, Some(p)) = (carried <= CARRY_LIMIT, &latest) { days.insert(day, p.clone()); }
            day = match day.succ_opt() { Some(d) => d, None => break };
        }
        out.insert(i.key.clone(), days);
    }
    Ok(out)
}

/// A day's sweep output: the whole account's row and the venue's, when the venue's history has begun.
pub struct Swept { pub whole: Vec<(NaiveDate, Day)>, pub venue: Vec<(NaiveDate, Day)> }

/// `sweep`: the rows applied as their day comes round, then money in up to the day, then the venue's quantities
/// valued. One venue, which lends cash (Tracker::UnfundedCash::LENDS_CASH): a shortfall is never inferred, and cash
/// below zero is owed.
pub fn sweep(c: &Connection, plan: &Plan, rows: &[Stored], terms: &[Term], first: NaiveDate, last: NaiveDate) -> Result<Swept, FiguresError> {
    let prices = prices_by_day(c, plan, last)?;
    let keys: HashMap<&str, &str> = plan.instruments.iter().map(|i| (i.symbol.as_str(), i.key.as_str())).collect();
    let mut by_date: Vec<&Stored> = rows.iter().collect();
    crate::figures::budget::charge(super::rows::sort_steps(by_date.len()), 0)?;
    by_date.sort_by_key(|r| (r.at.0, r.id));
    let deposits: HashMap<i64, &Stored> = rows.iter().map(|r| (r.id, r)).collect();
    let linked_to_me: std::collections::HashSet<i64> = rows.iter().filter_map(|r| r.linked_to).collect();
    let dust = Dec::strict("0.00000001")?;
    // Each symbol's position in `balances`, the order it was first touched.
    let mut balances: Vec<(String, Dec)> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut has_venue = false;
    let (mut invested, mut incomplete, mut invested_seen) = (Dec::zero(), false, false);
    let opened = by_date.first().map(|r| r.date());
    let (mut next_row, mut next_term) = (0usize, 0usize);
    let mut out = Swept { whole: vec![], venue: vec![] };
    let add = |balances: &mut Vec<(String, Dec)>, index: &mut HashMap<String, usize>, symbol: &str, amount: Dec| -> Result<(), FiguresError> {
        crate::figures::budget::charge(1, 0)?;
        match index.get(symbol) {
            Some(&i) => { let q = &mut balances[i].1; *q = (&*q + &amount)?; }
            None => { index.insert(symbol.into(), balances.len()); balances.push((symbol.into(), (&Dec::zero() + &amount)?)); }
        }
        Ok(())
    };
    let mut day = first;
    while day <= last {
        crate::figures::budget::check()?;
        while next_row < by_date.len() && by_date[next_row].date() <= day {
            let t = by_date[next_row];
            next_row += 1;
            has_venue = true;
            let linked = t.linked_to.is_some() || linked_to_me.contains(&t.id);
            match t.kind {
                Kind::Buy | Kind::SwapIn | Kind::StakingReward | Kind::LendingInterest | Kind::Airdrop | Kind::Mining | Kind::OtherIncome => add(&mut balances, &mut index, &t.base, t.amount.clone())?,
                Kind::Deposit => if !linked { add(&mut balances, &mut index, &t.base, t.amount.clone())? },
                Kind::Sell | Kind::SwapOut | Kind::Fee | Kind::Lost | Kind::WithholdingTax => add(&mut balances, &mut index, &t.base, t.amount.neg())?,
                Kind::Withdrawal => match t.linked_to.and_then(|d| deposits.get(&d)) {
                    // Linked on the same venue: only the network fee leaves.
                    Some(deposit) => { let fee = (&t.amount - &deposit.amount)?; add(&mut balances, &mut index, &t.base, if fee.is_positive() { fee } else { Dec::zero() }.neg())? }
                    None => add(&mut balances, &mut index, &t.base, t.amount.neg())?,
                },
                Kind::Adjustment => add(&mut balances, &mut index, &t.base, t.amount.clone())?,
                Kind::ReturnOfCapital | Kind::Unsupported => {}
            }
            if let (Some(quote), Some(qa)) = (t.quote.as_deref(), &t.quote_amount) {
                match t.kind { Kind::Buy => add(&mut balances, &mut index, quote, qa.neg())?, Kind::Sell | Kind::ReturnOfCapital => add(&mut balances, &mut index, quote, qa.clone())?, _ => {} }
            }
        }
        while next_term < terms.len() && terms[next_term].at.utc().date_naive() <= day {
            let term = &terms[next_term];
            next_term += 1;
            invested_seen = true;
            invested = (&invested + &term.amount)?;
            incomplete |= !term.complete;
            if let Some((symbol, quantity)) = &term.opens { add(&mut balances, &mut index, symbol, quantity.clone())?; }
        }
        let mut days = vec![];
        if has_venue || invested_seen {
            // The day reads every balance: a step each.
            crate::figures::budget::charge(balances.len() as u64, 0)?;
            let owed = |symbol: &str, q: &Dec| q.is_negative() && cash(symbol);
            let mut unpriced = balances.iter().any(|(s, q)| *q < dust.neg() && !owed(s, q));
            let (mut total, mut held) = (Dec::zero(), Dec::zero());
            for (symbol, quantity) in &balances {
                if !(quantity.is_positive() || owed(symbol, quantity)) { continue; }
                let value = if stable(symbol) { Some(quantity.clone()) }
                    else if fiat(symbol) { Some((quantity * &Dec::one())?) }
                    else {
                        // A symbol with no instrument (`identity_of` found it both a stock and a coin) has no prices.
                        let key = keys.get(symbol.as_str()).copied().unwrap_or(symbol.as_str());
                        match prices.get(key).and_then(|d| d.get(&day)) { Some(p) => Some((quantity * p)?), None => None }
                    };
                unpriced |= value.is_none();
                let value = value.unwrap_or_else(Dec::zero);
                total = (&total + &value)?;
                if !cash(symbol) { held = (&held + &value)?; }
            }
            let held_cost = (&invested - &(&total - &held)?)?;
            days.push(Day { value: total, invested: invested.clone(), held_value: Some(held), held_cost: Some(held_cost), partial: unpriced || incomplete });
        }
        let sum = |f: &dyn Fn(&Day) -> Dec| -> Result<Dec, FiguresError> { let mut s = Dec::zero(); for d in &days { s = (&s + &f(d))?; } Ok(s) };
        out.whole.push((day, Day {
            value: sum(&|d| d.value.clone())?, invested: sum(&|d| d.invested.clone())?,
            held_value: Some(sum(&|d| d.held_value.clone().unwrap_or_else(Dec::zero))?), held_cost: Some(sum(&|d| d.held_cost.clone().unwrap_or_else(Dec::zero))?),
            partial: days.iter().any(|d| d.partial),
        }));
        if let (Some(since), Some(row)) = (opened, days.into_iter().next()) { if day >= since { out.venue.push((day, row)); } }
        day = match day.succ_opt() { Some(d) => d, None => break };
    }
    Ok(out)
}

/// `store`: both tables and the version they were swept from, in one transaction; then the price generation.
pub fn store(c: &Connection, cipher: &Cipher, user_id: i64, swept: &Swept, last: NaiveDate, version: &str, now: chrono::DateTime<chrono::Utc>) -> Result<(), FiguresError> {
    let exchange_id = Venue::alpaca(c)?.id;
    c.execute_batch("BEGIN IMMEDIATE")?;
    let written = (|| -> Result<(), FiguresError> {
        for (date, day) in &swept.whole { crate::figures::budget::charge(1, 0)?; upsert_whole(c, user_id, *date, day)?; }
        c.execute("DELETE FROM portfolio_venue_snapshots WHERE user_id = ?1 AND date <= ?2", rusqlite::params![user_id, last.to_string()])?;
        let mut s = c.prepare_cached(
            "INSERT INTO portfolio_venue_snapshots (user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')) ON CONFLICT DO NOTHING")?;
        for (date, day) in &swept.venue {
            crate::figures::budget::charge(1, 0)?;
            let col = |d: &Option<Dec>| d.as_ref().map(super::snapshot::column).transpose();
            s.execute(rusqlite::params![user_id, exchange_id, date.to_string(), super::snapshot::column(&day.value)?, super::snapshot::column(&day.invested)?,
                                        col(&day.held_value)?, col(&day.held_cost)?, day.partial])?;
        }
        crate::app_config::set(c, cipher, &history_key(user_id), version, now).map_err(FiguresError::Data)
    })();
    match written {
        Ok(()) => c.execute_batch("COMMIT")?,
        Err(e) => { let _ = c.execute_batch("ROLLBACK"); return Err(e); }
    }
    crate::app_config::set(c, cipher, &price_key(user_id), &generation(c)?.to_string(), now).map_err(FiguresError::Data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::EncryptionKeys;
    use crate::figures::{at::At, budget};

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    /// Alpaca listing BTC as the coin, the catalogue also holding a BTC security, and a ledger from 1 September.
    fn install() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca');
            CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT, symbol TEXT, market_cap_rank INTEGER);
            INSERT INTO assets VALUES (1, 'bitcoin', 'Cryptocurrency', 'BTC', 1), (2, 'BTC.US', 'Stock', 'BTC', NULL);
            CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, base_asset_id INTEGER); INSERT INTO tickers VALUES (1, 1, 'BTC', 1);
            CREATE TABLE api_keys (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER); INSERT INTO api_keys VALUES (1, 1, 1);
            CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC);
            CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, transacted_at TEXT, updated_at TEXT, base_currency TEXT, base_asset_id INTEGER);
            INSERT INTO account_transactions VALUES (1, 1, 1, '2026-09-01 14:00:00', '2026-09-01 15:00:00', 'USD', NULL);
            CREATE TABLE portfolio_snapshots (id INTEGER PRIMARY KEY, user_id INTEGER, date TEXT, held_cost_usd NUMERIC, partial BOOLEAN);
            CREATE TABLE portfolio_venue_snapshots (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, date TEXT, held_cost_usd NUMERIC, partial BOOLEAN);
            CREATE TABLE app_configs (id INTEGER PRIMARY KEY, key TEXT UNIQUE, value TEXT, created_at TEXT, updated_at TEXT);").unwrap();
        c
    }

    fn bought(symbol: &str) -> Stored {
        Stored { id: 1, exchange_id: 1, kind: Kind::Buy, base: symbol.into(), amount: Dec::one(), quote: Some("USD".into()), quote_amount: Some(Dec::one()), tx_id: None,
                 group: None, at: At::from_sql("2026-09-02 14:30:00").unwrap(), per_share: None, stated: None, linked_to: None }
    }

    /// PortfolioSnapshot::BackfillJob#identity_of under a name Alpaca can list twice (its BTC security beside BTC/USD):
    /// the asset the venue's rows recorded decides, else what the venue lists, else the catalogue; a name the rows
    /// recorded as both is not guessed at and stays unpriced. A coin is never valued at the security's stored closes.
    #[test]
    fn a_coin_is_not_valued_at_a_security_s_stored_closes() {
        let c = install();
        c.execute("INSERT INTO historical_prices (asset, currency, date, price) VALUES ('stock:BTC', 'USD', '2026-09-30', 41.5)", []).unwrap();
        let instrument = |c: &Connection| plan(c, 1, &[bought("BTC")], day("2026-09-02"), day("2026-09-30")).map(|p| p.instruments.into_iter().find(|i| i.symbol == "BTC"));
        // Nothing recorded: the venue lists only the coin.
        let coin = instrument(&c).unwrap().unwrap();
        assert_eq!((coin.key.as_str(), coin.stock, coin.coin.clone()), ("BTC", false, Some(Some("bitcoin".to_string()))));
        // The rows recorded the coin, beside a venue listing both.
        c.execute_batch("INSERT INTO tickers VALUES (2, 1, 'BTC', 2); INSERT INTO account_transactions VALUES (2, 1, 1, '2026-09-02 14:30:00', '2026-09-02 15:00:00', 'BTC', 1);").unwrap();
        let coin = instrument(&c).unwrap().unwrap();
        assert_eq!((coin.key.as_str(), coin.stock), ("BTC", false));
        assert!(!prices_by_day(&c, &Plan { instruments: vec![coin], fetches: vec![] }, day("2026-09-30")).unwrap()["BTC"].contains_key(&day("2026-09-30")),
                "never the security's 41.5");
        // The rows recorded the security alone: a stock, which this build refuses while the venue's first BTC ticker is the coin's.
        c.execute("UPDATE account_transactions SET base_asset_id = 2 WHERE id = 2", []).unwrap();
        assert!(matches!(instrument(&c), Err(FiguresError::NotComputed(ref m)) if m == "the tracker walk is not ported for a stock whose ticker on Alpaca is a coin's"));
        // The rows recorded both: no instrument, so its days are unpriced.
        c.execute("INSERT INTO account_transactions VALUES (3, 1, 1, '2026-09-03 14:30:00', '2026-09-03 15:00:00', 'BTC', 1)", []).unwrap();
        assert!(instrument(&c).unwrap().is_none());
        let swept = sweep(&c, &plan(&c, 1, &[bought("BTC")], day("2026-09-02"), day("2026-09-30")).unwrap(), &[bought("BTC")], &[], day("2026-09-02"), day("2026-09-03")).unwrap();
        assert!(swept.whole.iter().all(|(_, d)| d.partial && d.held_value.as_ref().is_some_and(Dec::is_zero)), "unpriced, not valued at either close");
    }

    /// Every day from the first transaction to yesterday, in both tables, a swept row each: the history a sweep writes.
    fn swept(c: &Connection) -> crate::crypto::Cipher {
        let cipher = crate::crypto::Cipher::new(&EncryptionKeys::resolve(&|_| None, "backfill-tests").unwrap());
        crate::app_config::set(c, &cipher, &history_key(1), &history_version(c, 1).unwrap(), "2026-09-23T03:00:00Z".parse().unwrap()).unwrap();
        let mut d = day("2026-09-01");
        while d <= day("2026-10-01") {
            c.execute("INSERT INTO portfolio_snapshots (user_id, date, held_cost_usd, partial) VALUES (1, ?1, 400, 0)", [d.to_string()]).unwrap();
            c.execute("INSERT INTO portfolio_venue_snapshots (user_id, exchange_id, date, held_cost_usd, partial) VALUES (1, 1, ?1, 400, 0)", [d.to_string()]).unwrap();
            d = d.succ_opt().unwrap();
        }
        cipher
    }

    /// A day of the sweep's window missing from either table wants a sweep, though nothing else moved: a week offline
    /// with no transactions; a venue's row missing beside the whole account's; a venue's history starting late; a venue
    /// day never swept; a partial day once prices have moved. A row before the first transaction, which no sweep reaches,
    /// never does, unswept or partial.
    #[test]
    fn a_missing_day_in_either_table_wants_a_sweep() {
        let today = day("2026-10-01");
        let c = install();
        let cipher = swept(&c);
        assert!(!wanted(&c, &cipher, 1, today).unwrap(), "every day stored, nothing moved");
        // Unswept and partial before the first transaction, the prices' generation never stamped: still nothing a sweep can reach.
        c.execute("INSERT INTO portfolio_snapshots (user_id, date, held_cost_usd, partial) VALUES (1, '2026-08-20', NULL, 1)", []).unwrap();
        c.execute("INSERT INTO portfolio_venue_snapshots (user_id, exchange_id, date, held_cost_usd, partial) VALUES (1, 1, '2026-08-25', 400, 0)", []).unwrap();
        assert!(!wanted(&c, &cipher, 1, today).unwrap(), "rows before the first transaction");
        for (why, change) in [("a week offline", "DELETE FROM portfolio_snapshots WHERE date BETWEEN '2026-09-23' AND '2026-09-30'"),
                              ("a venue's row missing", "DELETE FROM portfolio_venue_snapshots WHERE date = '2026-09-15'"),
                              ("a venue's history starting late", "DELETE FROM portfolio_venue_snapshots WHERE date BETWEEN '2026-09-01' AND '2026-09-03'"),
                              ("a venue day never swept", "UPDATE portfolio_venue_snapshots SET held_cost_usd = NULL WHERE date = '2026-09-10'"),
                              ("a partial day in the window, with prices since", "UPDATE portfolio_snapshots SET partial = 1 WHERE date = '2026-09-10'")] {
            let c = install();
            let cipher = swept(&c);
            c.execute(change, []).unwrap();
            assert!(wanted(&c, &cipher, 1, today).unwrap(), "{why}");
        }
    }

    /// Planning many distinct symbols is linear work, each row and instrument a step: refused under an allowance it
    /// exceeds.
    #[test]
    fn many_symbols_plan_in_linear_work_within_the_allowance() {
        let c = install();
        let rows: Vec<Stored> = (0..3000).map(|i| Stored { id: i, base: format!("S{i}"), ..bought("BTC") }).collect();
        let (out, used) = budget::scope(budget::FIGURE, || plan(&c, 1, &rows, day("2026-09-02"), day("2026-09-30")));
        assert_eq!(out.unwrap().instruments.len(), 3000);
        assert!(used.steps >= 6_000 && used.steps < 1_000_000, "{} steps", used.steps);
        let (out, _) = budget::scope(budget::Limits { steps: 1_000, held: budget::FIGURE.held }, || plan(&c, 1, &rows, day("2026-09-02"), day("2026-09-30")));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET));
    }

    /// The closes are held only for the days that have one (a close carried at most CARRY_LIMIT days), and every day
    /// walked is a step: instruments × days with no closes hold nothing, and are refused under a limit they exceed.
    #[test]
    fn the_closes_by_day_hold_only_prices_and_every_day_walked_is_charged() {
        let c = install();
        c.execute("INSERT INTO historical_prices (asset, currency, date, price) VALUES ('stock:AAPL', 'USD', '2026-09-01', 228.5)", []).unwrap();
        let instrument = |n: usize| { let symbol = if n == 0 { "AAPL".to_string() } else { format!("S{n}") }; Instrument { key: format!("stock:{symbol}"), symbol, stock: true, from: day("2016-10-01"), coin: None } };
        let plan = Plan { instruments: (0..500).map(instrument).collect(), fetches: vec![] };
        let (by_day, used) = budget::scope(budget::FIGURE, || prices_by_day(&c, &plan, day("2026-09-30")));
        let by_day = by_day.unwrap();
        assert_eq!(by_day.values().map(BTreeMap::len).sum::<usize>(), 1 + CARRY_LIMIT, "AAPL's close and the days it is carried");
        assert!(used.steps >= 500 * 3_652, "{} steps", used.steps);
        let (out, _) = budget::scope(budget::Limits { steps: 1_000_000, held: budget::FIGURE.held }, || prices_by_day(&c, &plan, day("2026-09-30")));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET));
    }

    /// The sweep of 100,000 symbols finds each balance through an index, every add and every balance a day reads a
    /// step: bounded, and refused under an allowance it exceeds.
    #[test]
    fn many_symbols_sweep_in_linear_work_within_the_allowance() {
        let c = install();
        let rows: Vec<Stored> = (0..100_000).map(|i| Stored { id: i, base: format!("S{i}"), ..bought("BTC") }).collect();
        let sweep_in = |limits| budget::scope(limits, || sweep(&c, &Plan::default(), &rows, &[], day("2026-09-02"), day("2026-09-03")));
        let (out, used) = sweep_in(budget::FIGURE);
        assert_eq!(out.unwrap().whole.len(), 2);
        assert!(used.steps >= 400_000 && used.steps < budget::FIGURE.steps / 10, "{} steps", used.steps);
        assert!(matches!(sweep_in(budget::Limits { steps: 50_000, held: budget::FIGURE.held }).0, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET));
    }

    /// A symbol first traded today, swept to yesterday, with a close already stored: its window is empty (Rails selects
    /// no prices from a reversed range), so the sweep runs and values nothing of it, and nothing panics.
    #[test]
    fn a_symbol_first_traded_today_has_no_prices_in_the_sweep() {
        let c = install();
        c.execute_batch("INSERT INTO assets VALUES (3, 'AAPL.US', 'Stock', 'AAPL', NULL); INSERT INTO tickers VALUES (2, 1, 'AAPL', 3);
                         INSERT INTO historical_prices (asset, currency, date, price) VALUES ('stock:AAPL', 'USD', '2026-09-30', 228.5);").unwrap();
        let deposit = Stored { kind: Kind::Deposit, base: "USD".into(), amount: Dec::from_i64(1000), quote: None, quote_amount: None,
                               at: At::from_sql("2026-09-01 14:00:00").unwrap(), ..bought("USD") };
        let today = Stored { id: 2, at: At::from_sql("2026-10-01 14:30:00").unwrap(), ..bought("AAPL") };
        let rows = [deposit, today];
        let (first, last) = (day("2026-09-01"), day("2026-09-30"));
        let plan = plan(&c, 1, &rows, first, last).unwrap();
        assert!(plan.instruments.iter().any(|i| i.key == "stock:AAPL" && i.from == day("2026-10-01")));
        assert!(prices_by_day(&c, &plan, last).unwrap()["stock:AAPL"].is_empty(), "no AAPL price on any swept day");
        let swept = sweep(&c, &plan, &rows, &[], first, last).unwrap();
        assert_eq!(swept.whole.len(), 30);
    }
}
