//! Tracker::Figures (app/models/tracker/figures.rb): the ledger reconciled with the balances, as far as the snapshots
//! read it. The venue's balance is the truth about what is held; where the history disagrees, the extra left at cost
//! or arrived at the balance's price, and money in moves with it. The notes Rails writes beside each assumption are
//! the page's (Plan D5); the figures they shape are here.
use super::fx::FxTable;
use super::rows::{borrowed, Kind};
use super::walk::{cash_moves, quantity_moves, Summary};
use super::{cash, stable};
use crate::figures::{at::At, dec::Dec, FiguresError};
use chrono::NaiveDate;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// An `account_balances` row with something in it (`nonzero`), as the model reads it.
#[derive(Clone, Debug)]
pub struct Balance { pub asset: i64, pub category: Option<String>, pub symbol: String, pub exchange: String, pub free: Dec, pub locked: Dec, pub usd_price: Option<Dec>, pub usd_value: Option<Dec>, pub priced_at: Option<At>, pub synced_at: Option<At> }

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
    text.map(|t| At::from_sql(&t).map_err(|e| FiguresError::Data(format!("{e:?}")))).transpose()
}

/// `AccountBalance.for_user(user).nonzero` (and `.for_exchange`), with each row's asset symbol.
pub fn balances(c: &Connection, user_id: i64, exchange_id: Option<i64>) -> Result<Vec<Balance>, FiguresError> {
    let mut s = c.prepare(
        "SELECT a.symbol, b.free, b.locked, b.usd_price, b.usd_value, b.priced_at, b.synced_at, a.category, e.name, a.id FROM account_balances b JOIN assets a ON a.id = b.asset_id \
         JOIN exchanges e ON e.id = b.exchange_id \
         WHERE b.user_id = ?1 AND (?2 IS NULL OR b.exchange_id = ?2) AND b.free + b.locked > 0 ORDER BY b.id")?;
    let mut q = s.query(rusqlite::params![user_id, exchange_id])?;
    let mut out = vec![];
    while let Some(r) = q.next()? {
        out.push(Balance {
            asset: r.get(9)?, category: r.get(7)?, symbol: r.get(0)?, exchange: r.get(8)?, free: scaled(r.get_ref(1)?, 16)?.unwrap_or_else(Dec::zero), locked: scaled(r.get_ref(2)?, 16)?.unwrap_or_else(Dec::zero),
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
/// by symbol, the cash it spent or returned included. A venue with no watermark is not brought forward. A transfer is
/// linked here only when its partner moved since too (`quantity_row`).
pub fn pending(c: &Connection, user_id: i64, exchange_id: Option<i64>) -> Result<Vec<(String, Dec)>, FiguresError> {
    let marks = watermarks(c, user_id, exchange_id)?;
    let mut moved: Vec<(String, Dec)> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    let Some(since) = marks.values().copied().min() else { return Ok(moved) };
    // `linked_transaction || inverse_link`: the row it names, else the row that names it.
    let mut links: HashMap<i64, i64> = HashMap::new();
    let mut inverse: HashMap<i64, i64> = HashMap::new();
    {
        let mut s = c.prepare("SELECT id, linked_transaction_id FROM account_transactions WHERE user_id = ?1 AND linked_transaction_id IS NOT NULL ORDER BY id")?;
        let mut q = s.query([user_id])?;
        while let Some(r) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let (id, to): (i64, i64) = (r.get(0)?, r.get(1)?);
            links.insert(id, to);
            inverse.entry(to).or_insert(id);
        }
    }
    type Pending = (i64, Kind, String, Dec, Option<String>, Option<Dec>, Option<String>, Option<Dec>);
    let mut rows: Vec<Pending> = vec![];
    let mut s = c.prepare(
        "SELECT exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at, id, fee_currency, fee_amount \
         FROM account_transactions WHERE user_id = ?1 AND (?2 IS NULL OR exchange_id = ?2) ORDER BY id")?;
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
        rows.push((r.get(8)?, kind, base, amount, quote, quote_amount, r.get(9)?, Dec::from_sql(r.get_ref(10)?)?));
    }
    let ids: HashSet<i64> = rows.iter().map(|r| r.0).collect();
    let amounts: HashMap<i64, Dec> = rows.iter().map(|r| (r.0, r.3.clone())).collect();
    for (id, kind, base, amount, quote, quote_amount, fee_currency, fee_amount) in &rows {
        let partner = links.get(id).or_else(|| inverse.get(id)).copied();
        let linked = partner.is_some_and(|p| ids.contains(&p));
        let fee = match (linked, kind, partner.and_then(|p| amounts.get(&p))) { (true, Kind::Withdrawal, Some(p)) => Some((amount - p)?), _ => None };
        for (symbol, a) in quantity_moves(*kind, base, amount, fee_currency.as_deref(), fee_amount.as_ref(), linked, fee.as_ref())? { if !cash(&symbol) { add(symbol, a)?; } }
        for (currency, a) in cash_moves(*kind, base, amount, quote.as_deref(), quote_amount.as_ref(), fee_currency.as_deref(), fee_amount.as_ref())? { add(currency, a)?; }
    }
    Ok(moved)
}

/// One holding: what the venue holds of a symbol (and what arrived since its sync), what it is worth, and its cost
/// against the history (none for cash).
#[derive(Clone, Debug)]
pub struct Holding { pub asset: Option<i64>, pub symbol: String, pub quantity: Dec, pub value: Dec, pub cost: Option<Dec>, pub cash: bool, pub unrealised: Option<Dec>, pub note: Option<Note> }

impl Holding {
    /// `Holding#percent`: the gain on its cost, none without one.
    pub fn percent(&self) -> Result<Option<Dec>, FiguresError> {
        let Some(cost) = self.cost.as_ref().filter(|c| c.is_positive()) else { return Ok(None) };
        Ok(Some((&(&self.value.div(cost)? - &Dec::one())? * &Dec::from_i64(100))?))
    }
}

/// `Figures::Note`: an assumption, for the page to say. `exchange` is the venue's name when one venue holds the asset.
#[derive(Clone, Debug, PartialEq)]
pub struct Note { pub kind: &'static str, pub symbol: String, pub exchange: Option<String>, pub history: Option<Dec>, pub held: Option<Dec>, pub amount: Dec }

/// The notes a computation wrote, in the order it wrote them (`@notes`).
#[derive(Default)]
struct Notes(Vec<Note>);

impl Notes {
    /// `note`: below a dollar an assumption is applied in silence (`NOTE_FLOOR`), unless it is always stated.
    #[allow(clippy::too_many_arguments)] // Rails' `note(kind, symbol, exchange, history, held, amount_usd, always:)`
    fn note(&mut self, kind: &'static str, symbol: &str, exchange: Option<&str>, history: Option<&Dec>, held: Option<&Dec>, amount: Dec, always: bool) -> Option<Note> {
        let floor = if amount.is_negative() { amount.neg() } else { amount.clone() } >= Dec::one();
        if !(always || floor) { return None; }
        let note = Note { kind, symbol: symbol.into(), exchange: exchange.map(Into::into), history: history.cloned(), held: held.cloned(), amount };
        self.0.push(note.clone());
        Some(note)
    }
    /// `provenance_note`: units nobody could price, or a basis that is a day's market price, not a fill.
    fn provenance(&mut self, symbol: &str, position: &super::walk::Position, held: &Dec, price: Option<&Dec>, exchange: Option<&str>) -> Result<Option<Note>, FiguresError> {
        if position.unpriced.is_positive() {
            let amount = (&position.unpriced * price.unwrap_or(&Dec::zero()))?;
            return Ok(self.note("unpriced_held", symbol, exchange, Some(&position.unpriced), Some(held), amount, true));
        }
        Ok(if position.estimated { self.note("estimated", symbol, exchange, Some(&position.quantity), Some(held), position.cost.clone(), true) } else { None })
    }
}

/// Tracker::Figures::Result.
#[derive(Clone, Debug)]
pub struct Figures { pub value: Dec, pub invested: Dec, pub fees: Dec, pub realised: Dec, pub unrealised: Dec, pub total: Dec, pub holdings: Vec<Holding>, pub notes: Vec<Note> }

impl Figures {
    /// `Result#without_cash`: the cash taken off both sides, (value, invested).
    pub fn without_cash(&self) -> Result<(Dec, Dec), FiguresError> {
        let mut idle = Dec::zero();
        for h in self.holdings.iter().filter(|h| h.cash) { idle = (&idle + &h.value)?; }
        Ok(((&self.value - &idle)?, (&self.invested - &idle)?))
    }

    /// `Result#without_cash` as the page reads it: the cash rows gone, and taken off the value and money in.
    pub fn without_cash_result(&self) -> Result<Figures, FiguresError> {
        let (value, invested) = self.without_cash()?;
        Ok(Figures { value, invested, holdings: self.holdings.iter().filter(|h| !h.cash).cloned().collect(), ..self.clone() })
    }
}

/// The asset a holding is drawn as (`rows.first&.asset || Ledger.symbol_index(user, [symbol])[symbol] ||
/// Asset.find_by(symbol:)`): a balance of the user's in that symbol, else the catalog's coin, then its currency, else any
/// asset of that symbol.
fn asset_for(c: &Connection, user_id: i64, symbol: &str) -> Result<Option<i64>, FiguresError> {
    use rusqlite::OptionalExtension;
    let held: Option<i64> = c.query_row("SELECT b.asset_id FROM account_balances b JOIN assets a ON a.id = b.asset_id WHERE b.user_id = ?1 AND a.symbol = ?2 ORDER BY b.id LIMIT 1",
        rusqlite::params![user_id, symbol], |r| r.get(0)).optional()?;
    if held.is_some() { return Ok(held); }
    let catalog: Option<i64> = c.query_row("SELECT id FROM assets WHERE symbol = ?1 AND category IN ('Cryptocurrency','Fiat','Currency') ORDER BY category = 'Cryptocurrency' DESC, id LIMIT 1",
        [symbol], |r| r.get(0)).optional()?;
    if catalog.is_some() { return Ok(catalog); }
    Ok(c.query_row("SELECT id FROM assets WHERE symbol = ?1 ORDER BY id LIMIT 1", [symbol], |r| r.get(0)).optional()?)
}

/// `rate(currency)`: a dollar and a stablecoin are one dollar; other cash at today's ECB rate, at par when there is none,
/// which is noted each time it is asked.
fn rate(currency: &str, fx: &FxTable, today: NaiveDate, notes: &mut Notes) -> Result<Dec, FiguresError> {
    if currency == "USD" || stable(currency) { return Ok(Dec::one()); }
    Ok(match fx.rate(currency, "USD", today)? {
        Some(rate) => rate,
        None => { notes.note("at_par", currency, None, None, None, Dec::zero(), true); Dec::one() }
    })
}

/// `venue_of`: the venue a coin's history was booked on, when it is one venue.
fn venue_of(c: &Connection, user_id: i64, symbol: &str) -> Result<Option<String>, FiguresError> {
    let mut s = c.prepare("SELECT e.name FROM exchanges e WHERE e.id IN (SELECT exchange_id FROM account_transactions WHERE user_id = ?1 AND base_currency = ?2)")?;
    let names: Vec<String> = s.query_map(rusqlite::params![user_id, symbol], |r| r.get(0))?.collect::<Result<_, _>>()?;
    crate::figures::budget::charge(names.len() as u64, 0)?;
    Ok(if names.len() == 1 { names.into_iter().next() } else { None })
}

/// A (key, value) list by its key; each entry a step.
fn indexed(list: &[(String, Dec)]) -> Result<HashMap<&str, &Dec>, FiguresError> {
    crate::figures::budget::charge(list.len() as u64, 0)?;
    Ok(list.iter().map(|(k, v)| (k.as_str(), v)).collect())
}

/// `Figures.for(user, ledger:, balances:, pending:)`. Every symbol's balances, pending move and position are found
/// through an index, each lookup a step.
pub fn compute(c: &Connection, user_id: i64, ledger: &Summary, balances: &[Balance], pending: &[(String, Dec)]) -> Result<Figures, FiguresError> {
    compute_on(c, user_id, ledger, balances, pending, &FxTable::load(c)?, chrono::Utc::now().date_naive())
}

/// `compute` with the ECB table read and today's date.
pub fn compute_on(c: &Connection, user_id: i64, ledger: &Summary, balances: &[Balance], pending: &[(String, Dec)], fx: &FxTable, today: NaiveDate) -> Result<Figures, FiguresError> {
    // Historical ledger keys remain unchanged; refuse mixed classes before quantities merge.
    let mut classes: HashMap<&str, HashSet<Option<String>>> = HashMap::new();
    // Fiat and Currency are both cash, and a tokenized share tracks its company's share: one class each.
    let class = |c: Option<String>| match c.as_deref() { Some("Currency") => Some("Fiat".to_string()), Some("Tokenized Stock") => Some("Stock".to_string()), _ => c };
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
    let mut notes = Notes::default();
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
        let asset = match rows.first() { Some(r) => Some(r.asset), None => asset_for(c, user_id, &symbol)? };
        let exchange = rows.first().map(|r| r.exchange.as_str()).filter(|name| rows.iter().all(|r| r.exchange == *name));
        let mut prices = Dec::zero();
        let mut values = Dec::zero();
        for r in rows {
            prices = (&prices + r.usd_price.as_ref().unwrap_or(&zero))?;
            values = (&values + r.usd_value.as_ref().unwrap_or(&zero))?;
        }
        let price = if prices.is_positive() { Some(values.div(&units)?) } else { None };
        if cash(&symbol) {
            let value = if stable(&symbol) { held.clone() } else { (&held * &match &price { Some(p) => p.clone(), None => rate(&symbol, fx, today, &mut notes)? })? };
            holdings.push(Holding { asset, symbol, quantity: held, value, cost: None, cash: true, unrealised: None, note: None });
            continue;
        }
        let position = position_of.get(symbol.as_str()).copied();
        let since_sync = rows.is_empty();
        let history = position.map_or_else(Dec::zero, |p| p.quantity.clone());
        let avg = position.map_or_else(Dec::zero, |p| p.avg_cost.clone());
        let delta = (&held - &history)?;
        // `resolve`: the holding's cost against the history, and the assumption that closed the gap, if any.
        let (cost, note) = if delta.is_negative() {
            // The extra left at cost.
            let left = (&avg * &delta.neg())?;
            moved = (&moved - &left)?;
            let note = match notes.note("left", &symbol, exchange, Some(&history), Some(&held), left, false) {
                Some(note) => Some(note),
                None => match position { Some(p) => notes.provenance(&symbol, p, &held, price.as_ref(), exchange)?, None => None },
            };
            ((&avg * &held)?, note)
        } else if delta.is_positive() {
            // The extra arrived: at the venue's price, at cost while it has none.
            let unit = price.clone().unwrap_or(avg);
            let arrived = (&delta * &unit)?;
            moved = (&moved + &arrived)?;
            let kind = if since_sync { "since_sync" } else { "arrived" };
            let note = match notes.note(kind, &symbol, exchange, Some(&history), Some(&held), arrived.clone(), since_sync) {
                Some(note) => Some(note),
                None => match position { Some(p) => notes.provenance(&symbol, p, &held, price.as_ref(), exchange)?, None => None },
            };
            ((position.map_or(&zero, |p| &p.cost) + &arrived)?, note)
        } else {
            let note = match position { Some(p) => notes.provenance(&symbol, p, &held, price.as_ref(), exchange)?, None => None };
            (position.map_or_else(Dec::zero, |p| p.cost.clone()), note)
        };
        // A coin the venue has not reported yet is held at cost until it does, and says so whatever else was assumed.
        let note = match note { None if since_sync => notes.note("since_sync", &symbol, exchange, Some(&history), Some(&held), cost.clone(), true), note => note };
        let value = match &price { Some(p) => (&held * p)?, None => cost.clone() };
        holdings.push(Holding { asset, symbol, quantity: held, unrealised: Some((&value - &cost)?), value, cost: Some(cost), cash: false, note });
    }
    // `sort_by { -value }`.
    holdings.sort_by(|a, b| b.value.cmp(&a.value));
    // `resolve_departed`: coins the history holds that no venue reports, left at cost.
    let held_symbols: HashSet<String> = holdings.iter().map(|h| h.symbol.clone()).collect();
    for p in &ledger.positions {
        crate::figures::budget::charge(1, 0)?;
        if held_symbols.contains(p.symbol.as_str()) || cash(&p.symbol) || !p.quantity.is_positive() { continue; }
        moved = (&moved - &p.cost)?;
        notes.note("left", &p.symbol, venue_of(c, user_id, &p.symbol)?.as_deref(), Some(&p.quantity), Some(&Dec::zero()), p.cost.clone(), false);
    }
    // `resolve_cash`: cash against the basis the ledger carried it at; its worth beyond the basis is unrealised.
    let mut venue_cash: HashMap<&str, (Dec, Dec)> = HashMap::new();
    let mut currencies: Vec<&str> = ledger.cash.iter().map(|(c, _)| c.as_str()).collect();
    let mut listed: HashSet<&str> = currencies.iter().copied().collect();
    for h in holdings.iter().filter(|h| h.cash) {
        crate::figures::budget::charge(1, 0)?;
        let (q, w) = venue_cash.entry(h.symbol.as_str()).or_insert_with(|| (Dec::zero(), Dec::zero()));
        *q = (&*q + &h.quantity)?;
        *w = (&*w + &h.value)?;
        if listed.insert(h.symbol.as_str()) { currencies.push(h.symbol.as_str()); }
    }
    let (cash_of, basis_of) = (indexed(&ledger.cash)?, indexed(&ledger.cash_basis)?);
    let mut cash_unrealised = Dec::zero();
    for currency in currencies {
        crate::figures::budget::charge(1, 0)?;
        let history = cash_of.get(currency).map_or_else(Dec::zero, |d| (*d).clone());
        let (held, worth) = venue_cash.get(currency).cloned().unwrap_or_else(|| (Dec::zero(), Dec::zero()));
        let mut basis = basis_of.get(currency).map_or_else(Dec::zero, |d| (*d).clone());
        let gap = (&history - &held)?;
        if gap.is_positive() {
            let left = (&basis * &gap)?.div(&history)?;
            moved = (&moved - &left)?;
            basis = (&basis - &left)?;
            notes.note("cash_out", currency, None, Some(&history), Some(&held), left, false);
        } else if gap.is_negative() {
            let arrived = (&gap.neg() * &rate(currency, fx, today, &mut notes)?)?;
            moved = (&moved + &arrived)?;
            basis = (&basis + &arrived)?;
            notes.note("cash_in", currency, None, Some(&history), Some(&held), arrived, false);
        }
        cash_unrealised = (&cash_unrealised + &(&worth - &basis)?)?;
    }
    let mut value = Dec::zero();
    for h in &holdings { value = (&value + &h.value)?; }
    let invested = (&ledger.total_invested + &moved)?;
    let mut unrealised = Dec::zero();
    for u in holdings.iter().filter_map(|h| h.unrealised.as_ref()) { unrealised = (&unrealised + u)?; }
    let unrealised = (&unrealised + &cash_unrealised)?;
    // `unpriced_note`.
    if ledger.unpriced_proceeds.is_positive() { notes.note("unpriced", "", None, None, None, ledger.unpriced_proceeds.clone(), false); }
    // `backstop`: the two halves must agree to a cent, or a thousandth of the value.
    let drift = (&(&value - &invested)? - &(&ledger.realised + &unrealised)?)?;
    let magnitude = |d: &Dec| if d.is_negative() { d.neg() } else { d.clone() };
    let tolerance = std::cmp::max(Dec::strict("0.01")?, (&magnitude(&value) * &Dec::strict("0.001")?)?);
    if magnitude(&drift) > tolerance { notes.note("figures_disagree", "", None, None, None, drift, true); }
    let mut notes = notes.0;
    notes.sort_by(|a, b| (a.symbol.as_str(), a.kind).cmp(&(b.symbol.as_str(), b.kind)));
    Ok(Figures { total: (&value - &invested)?, value, invested, fees: ledger.fees.clone(), realised: ledger.realised.clone(), unrealised, holdings, notes })
}

/// Page-specific cold results share this service and its decimal boundary.
pub mod page;
