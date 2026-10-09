//! Tracker::Figures (app/models/tracker/figures.rb): the ledger reconciled with the balances, as far as the snapshots
//! read it. The venue's balance is the truth about what is held; where the history disagrees, the extra left at cost
//! or arrived at the balance's price, and money in moves with it. The notes Rails writes beside each assumption are
//! the page's (Plan D5); the figures they shape are here.
use super::rows::{borrowed, Kind};
use super::walk::{cash_moves, quantity_moves, Summary};
use super::{cash, stable};
use crate::figures::{at::At, dec::Dec, FiguresError};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// An `account_balances` row with something in it (`nonzero`), as the model reads it.
#[derive(Clone, Debug)]
pub struct Balance { pub category: Option<String>, pub symbol: String, pub free: Dec, pub locked: Dec, pub usd_price: Option<Dec>, pub usd_value: Option<Dec>, pub priced_at: Option<At>, pub synced_at: Option<At> }

/// A decimal column of `scale` with a precision, as ActiveRecord casts what SQLite returns: a REAL is
/// `BigDecimal(float.round(scale), 16)` then the scale (sync::balances::cast_float's rule), anything else to_d and the
/// scale.
pub fn scaled(v: rusqlite::types::ValueRef<'_>, scale: i32) -> Result<Option<Dec>, FiguresError> {
    let d = match v {
        rusqlite::types::ValueRef::Real(f) => {
            let rounded = crate::sync::balances::float_round(f, scale);
            Some(Dec::strict(&format!("{rounded:.15e}"))?)
        }
        other => Dec::from_sql(other)?,
    };
    Ok(d.map(|d| d.round(i64::from(scale))).transpose()?)
}

fn instant(text: Option<String>) -> Result<Option<At>, FiguresError> {
    text.map(|t| At::from_sql(&t).ok_or_else(|| FiguresError::Data(format!("a time Rails did not write: {t:?}")))).transpose()
}

/// `AccountBalance.for_user(user).nonzero` (and `.for_exchange`), with each row's asset symbol.
pub fn balances(c: &Connection, user_id: i64, exchange_id: Option<i64>) -> Result<Vec<Balance>, FiguresError> {
    let mut s = c.prepare(
        "SELECT a.symbol, b.free, b.locked, b.usd_price, b.usd_value, b.priced_at, b.synced_at, a.category FROM account_balances b JOIN assets a ON a.id = b.asset_id \
         WHERE b.user_id = ?1 AND (?2 IS NULL OR b.exchange_id = ?2) AND b.free + b.locked > 0 ORDER BY b.id")?;
    let mut q = s.query(rusqlite::params![user_id, exchange_id])?;
    let mut out = vec![];
    while let Some(r) = q.next()? {
        out.push(Balance {
            category: r.get(7)?, symbol: r.get(0)?, free: scaled(r.get_ref(1)?, 16)?.unwrap_or_else(Dec::zero), locked: scaled(r.get_ref(2)?, 16)?.unwrap_or_else(Dec::zero),
            usd_price: scaled(r.get_ref(3)?, 8)?, usd_value: scaled(r.get_ref(4)?, 8)?, priced_at: instant(r.get(5)?)?, synced_at: instant(r.get(6)?)?,
        });
    }
    Ok(out)
}

/// `PortfolioSnapshot.watermarks`: per venue, when its balances were last taken, over every key of the user.
fn watermarks(c: &Connection, user_id: i64, exchange_id: Option<i64>) -> Result<HashMap<i64, At>, FiguresError> {
    let mut s = c.prepare("SELECT exchange_id, balances_synced_at FROM api_keys WHERE user_id = ?1 AND (?2 IS NULL OR exchange_id = ?2) ORDER BY id")?;
    let rows: Vec<(i64, Option<String>)> = s.query_map(rusqlite::params![user_id, exchange_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut marks: HashMap<i64, At> = HashMap::new();
    for (exchange, at) in rows {
        crate::figures::budget::charge(1, 0)?;
        let Some(at) = instant(at)? else { continue };
        let m = marks.entry(exchange).or_insert(at);
        if at > *m { *m = at }
    }
    Ok(marks)
}

/// `Figures.moved_since(pending_scope, watermarks)`: what the ledger has moved since each venue's balances were taken,
/// by symbol, the cash it spent or returned included. A venue with no watermark is not brought forward. (A linked
/// transfer here is dollars, which move no quantity.)
pub fn pending(c: &Connection, user_id: i64, exchange_id: Option<i64>) -> Result<Vec<(String, Dec)>, FiguresError> {
    let marks = watermarks(c, user_id, exchange_id)?;
    let mut moved: Vec<(String, Dec)> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    let Some(since) = marks.values().copied().min() else { return Ok(moved) };
    let mut s = c.prepare(
        "SELECT exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at FROM account_transactions \
         WHERE user_id = ?1 AND (?2 IS NULL OR exchange_id = ?2) ORDER BY id")?;
    let mut q = s.query(rusqlite::params![user_id, exchange_id])?;
    let mut add = |symbol: String, amount: Dec| -> Result<(), FiguresError> {
        crate::figures::budget::charge(1, 0)?;
        match index.get(&symbol) {
            Some(&i) => { let m = &mut moved[i].1; *m = (&*m + &amount)?; }
            None => { index.insert(symbol.clone(), moved.len()); moved.push((symbol, (&Dec::zero() + &amount)?)); }
        }
        Ok(())
    };
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        let (exchange, tx_id, at): (i64, Option<String>, String) = (r.get(0)?, r.get(6)?, r.get(7)?);
        let Some(at) = instant(Some(at))? else { continue };
        if at < since || borrowed(tx_id.as_deref()) { continue; }
        let Some(&taken) = marks.get(&exchange) else { continue };
        if at < taken { continue; }
        let kind = Kind::from_stored(r.get(1)?).ok_or_else(|| FiguresError::Data("an entry type Rails does not have".into()))?;
        let base: String = r.get(2)?;
        let amount = Dec::from_sql(r.get_ref(3)?)?.ok_or_else(|| FiguresError::Data("a row with no base amount".into()))?;
        let (quote, quote_amount): (Option<String>, Option<Dec>) = (r.get(4)?, Dec::from_sql(r.get_ref(5)?)?);
        for (symbol, a) in quantity_moves(kind, &base, &amount, false, None)? { if !cash(&symbol) { add(symbol, a)?; } }
        for (currency, a) in cash_moves(kind, &base, &amount, quote.as_deref(), quote_amount.as_ref())? { add(currency, a)?; }
    }
    Ok(moved)
}

/// One holding: what the venue holds of a symbol (and what arrived since its sync), what it is worth, and its cost
/// against the history (none for cash).
#[derive(Clone, Debug)]
pub struct Holding { pub symbol: String, pub quantity: Dec, pub value: Dec, pub cost: Option<Dec>, pub cash: bool }

/// Tracker::Figures::Result as the snapshot reads it.
#[derive(Clone, Debug)]
pub struct Figures { pub value: Dec, pub invested: Dec, pub holdings: Vec<Holding> }

impl Figures {
    /// `Result#without_cash`: the cash taken off both sides, (value, invested).
    pub fn without_cash(&self) -> Result<(Dec, Dec), FiguresError> {
        let mut idle = Dec::zero();
        for h in self.holdings.iter().filter(|h| h.cash) { idle = (&idle + &h.value)?; }
        Ok(((&self.value - &idle)?, (&self.invested - &idle)?))
    }
}

/// `rate(currency)`: a dollar and a stablecoin are one dollar; no other cash reaches this build.
fn rate(currency: &str) -> Result<Dec, FiguresError> {
    if currency == "USD" || stable(currency) { Ok(Dec::one()) } else { Err(super::refused("cash other than US dollars")) }
}

/// A (key, value) list by its key; each entry a step.
fn indexed(list: &[(String, Dec)]) -> Result<HashMap<&str, &Dec>, FiguresError> {
    crate::figures::budget::charge(list.len() as u64, 0)?;
    Ok(list.iter().map(|(k, v)| (k.as_str(), v)).collect())
}

/// `Figures.for(user, ledger:, balances:, pending:)`. Every symbol's balances, pending move and position are found
/// through an index, each lookup a step.
pub fn compute(c: &Connection, user_id: i64, ledger: &Summary, balances: &[Balance], pending: &[(String, Dec)]) -> Result<Figures, FiguresError> {
    // Historical ledger keys remain unchanged; refuse mixed classes before quantities merge.
    let mut classes: HashMap<&str, HashSet<Option<String>>> = HashMap::new();
    // Fiat and Currency are both cash: one class (Fiat::CATEGORIES).
    let class = |c: Option<String>| match c.as_deref() { Some("Currency") => Some("Fiat".to_string()), _ => c };
    for balance in balances { classes.entry(&balance.symbol).or_default().insert(class(balance.category.clone())); }
    for (symbol, _) in pending {
        // Only the assets this user's own rows recorded: the catalogue's BTC stock beside BTC crypto is not ambiguity here.
        let mut stmt = c.prepare("SELECT DISTINCT a.category FROM account_transactions t JOIN assets a ON a.id = t.base_asset_id \
                                  WHERE t.user_id = ?1 AND t.base_currency = ?2")?;
        for category in stmt.query_map(rusqlite::params![user_id, symbol], |r| r.get::<_, Option<String>>(0))? {
            classes.entry(symbol).or_default().insert(class(category?));
        }
    }
    if classes.values().any(|categories| categories.len() > 1) {
        return Err(FiguresError::NotComputed("Tracker figures unavailable: ambiguous asset class for a shared symbol".into()));
    }
    let zero = Dec::zero();
    let mut moved = Dec::zero();
    // The symbols in the order Rails meets them (balances, then what arrived since), each with its balance rows.
    let mut symbols: Vec<String> = vec![];
    let mut rows_of: HashMap<&str, Vec<&Balance>> = HashMap::new();
    for b in balances {
        crate::figures::budget::charge(1, 0)?;
        let rows = rows_of.entry(b.symbol.as_str()).or_default();
        if rows.is_empty() { symbols.push(b.symbol.clone()); }
        rows.push(b);
    }
    let pending_of = indexed(pending)?;
    let mut listed: HashSet<String> = symbols.iter().cloned().collect();
    for (symbol, m) in pending { if m.is_positive() && listed.insert(symbol.clone()) { symbols.push(symbol.clone()); } }
    crate::figures::budget::charge(ledger.positions.len() as u64, 0)?;
    let position_of: HashMap<&str, &super::walk::Position> = ledger.positions.iter().map(|p| (p.symbol.as_str(), p)).collect();
    // Whether the catalogue knows a symbol only arrived since the sync: the catalogue's symbols read once, when any did.
    let unlisted: HashSet<&str> = symbols.iter().map(String::as_str).filter(|s| !rows_of.contains_key(s)).collect();
    let mut known: HashSet<String> = HashSet::new();
    if !unlisted.is_empty() {
        let mut s = c.prepare("SELECT DISTINCT symbol FROM assets")?;
        let mut q = s.query([])?;
        while let Some(r) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            if let Some(symbol) = r.get::<_, Option<String>>(0)? { if unlisted.contains(symbol.as_str()) { known.insert(symbol); } }
        }
    }
    let mut holdings = vec![];
    for symbol in symbols {
        crate::figures::budget::charge(1, 0)?;
        let rows: &[&Balance] = rows_of.get(symbol.as_str()).map_or(&[], Vec::as_slice);
        let mut held = pending_of.get(symbol.as_str()).map_or_else(Dec::zero, |m| (*m).clone());
        let mut units = Dec::zero();
        for r in rows { units = (&units + &(&r.free + &r.locked)?)?; }
        held = (&units + &held)?;
        if !held.is_positive() { continue; }
        if rows.is_empty() && !known.contains(&symbol) { continue; }
        let mut prices = Dec::zero();
        let mut values = Dec::zero();
        for r in rows {
            prices = (&prices + r.usd_price.as_ref().unwrap_or(&zero))?;
            values = (&values + r.usd_value.as_ref().unwrap_or(&zero))?;
        }
        let price = if prices.is_positive() { Some(values.div(&units)?) } else { None };
        if cash(&symbol) {
            let value = if stable(&symbol) { held.clone() } else { (&held * &match &price { Some(p) => p.clone(), None => rate(&symbol)? })? };
            holdings.push(Holding { symbol, quantity: held, value, cost: None, cash: true });
            continue;
        }
        let position = position_of.get(symbol.as_str()).copied();
        let history = position.map_or_else(Dec::zero, |p| p.quantity.clone());
        let avg = position.map_or_else(Dec::zero, |p| p.avg_cost.clone());
        let delta = (&held - &history)?;
        let cost = if delta.is_negative() {
            moved = (&moved - &(&avg * &delta.neg())?)?;
            (&avg * &held)?
        } else if delta.is_positive() {
            let unit = price.clone().unwrap_or(avg);
            let arrived = (&delta * &unit)?;
            moved = (&moved + &arrived)?;
            (position.map_or(&zero, |p| &p.cost) + &arrived)?
        } else {
            position.map_or_else(Dec::zero, |p| p.cost.clone())
        };
        let value = match &price { Some(p) => (&held * p)?, None => cost.clone() };
        holdings.push(Holding { symbol, quantity: held, value, cost: Some(cost), cash: false });
    }
    // Coins the history holds that no venue reports: left at cost.
    let held_symbols: HashSet<&str> = holdings.iter().map(|h| h.symbol.as_str()).collect();
    for p in &ledger.positions {
        crate::figures::budget::charge(1, 0)?;
        if held_symbols.contains(p.symbol.as_str()) || cash(&p.symbol) || !p.quantity.is_positive() { continue; }
        moved = (&moved - &p.cost)?;
    }
    // Cash against the basis the ledger carried it at.
    let mut venue_cash: HashMap<&str, Dec> = HashMap::new();
    let mut currencies: Vec<&str> = ledger.cash.iter().map(|(c, _)| c.as_str()).collect();
    let mut known: HashSet<&str> = currencies.iter().copied().collect();
    for h in holdings.iter().filter(|h| h.cash) {
        crate::figures::budget::charge(1, 0)?;
        let q = venue_cash.entry(h.symbol.as_str()).or_insert_with(Dec::zero);
        *q = (&*q + &h.quantity)?;
        if known.insert(h.symbol.as_str()) { currencies.push(h.symbol.as_str()); }
    }
    let (cash_of, basis_of) = (indexed(&ledger.cash)?, indexed(&ledger.cash_basis)?);
    for currency in currencies {
        crate::figures::budget::charge(1, 0)?;
        let history = cash_of.get(currency).map_or_else(Dec::zero, |d| (*d).clone());
        let held = venue_cash.get(currency).cloned().unwrap_or_else(Dec::zero);
        let basis = basis_of.get(currency).map_or_else(Dec::zero, |d| (*d).clone());
        let gap = (&history - &held)?;
        if gap.is_positive() {
            moved = (&moved - &(&basis * &gap)?.div(&history)?)?;
        } else if gap.is_negative() {
            moved = (&moved + &(&gap.neg() * &rate(currency)?)?)?;
        }
    }
    let mut value = Dec::zero();
    for h in &holdings { value = (&value + &h.value)?; }
    Ok(Figures { value, invested: (&ledger.total_invested + &moved)?, holdings })
}
