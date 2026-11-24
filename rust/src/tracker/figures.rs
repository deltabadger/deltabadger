//! Tracker::Figures: the ledger reconciled with the balances, as far as the snapshots
//! read it. The venue's balance is the truth about what is held; where the history disagrees, the extra left at cost
//! or arrived at the balance's price, and money in moves with it. The notes 2.x writes beside each assumption are
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
pub struct Balance { pub asset: i64,
    /// The balance's account (minus its exchange id without an account table), for its label (`Labels::asset`).
    pub account: i64, pub category: Option<String>, pub symbol: String, pub exchange: String, pub free: Dec, pub locked: Dec, pub usd_price: Option<Dec>, pub usd_value: Option<Dec>, pub priced_at: Option<At>, pub synced_at: Option<At> }

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

/// `AccountBalance.for_user(user).nonzero` (and, #710 slice 5, one account's), with each row's asset symbol.
pub fn balances(c: &Connection, user_id: i64, scope: &super::Scope) -> Result<Vec<Balance>, FiguresError> {
    // Each row named by its account (#710 slice 6); a stand-in (an install without the account tables) by its exchange.
    let name = if crate::identity::numbered(c)? { crate::identity::plain_label_sql("e.name", "b.account_id") } else { "e.name".into() };
    let has_account: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('exchange_account_balances') WHERE name = 'account_id')", [], |r| r.get(0))?;
    let account = if has_account { "COALESCE(b.account_id, -b.exchange_id)" } else { "-b.exchange_id" };
    let mut s = c.prepare(&format!(
        "SELECT a.symbol, b.free, b.locked, b.usd_price, b.usd_value, b.priced_at, b.synced_at, a.category, {name}, a.id, {account} FROM exchange_account_balances b JOIN assets a ON a.id = b.asset_id \
         JOIN exchanges e ON e.id = b.exchange_id \
         WHERE b.user_id = ?1 AND {} AND {} AND b.free + b.locked > 0 ORDER BY b.id", scope.store("b"), crate::identity::counted(c, user_id, "b.account_id")?))?;
    let mut q = s.query([user_id])?;
    let mut out = vec![];
    while let Some(r) = q.next()? {
        out.push(Balance {
            asset: r.get(9)?, account: r.get(10)?, category: r.get(7)?, symbol: r.get(0)?, exchange: r.get(8)?, free: scaled(r.get_ref(1)?, 16)?.unwrap_or_else(Dec::zero), locked: scaled(r.get_ref(2)?, 16)?.unwrap_or_else(Dec::zero),
            usd_price: scaled(r.get_ref(3)?, 8)?, usd_value: scaled(r.get_ref(4)?, 8)?, priced_at: instant(r.get(5)?)?, synced_at: instant(r.get(6)?)?,
        });
    }
    Ok(out)
}

/// `PortfolioSnapshot.watermarks`, by account (#710 slice 5, PLAN-710-5 §5.4): when each account's balances were last
/// taken, over its own keys (a key whose number 1 has no row yet: minus its exchange id, as a row of that account reads).
fn watermarks(c: &Connection, user_id: i64, scope: &super::Scope) -> Result<HashMap<i64, At>, FiguresError> {
    let key_account = if crate::identity::numbered(c)? { crate::identity::key_account_sql("k") } else { "NULL".into() };
    let account = format!("COALESCE({key_account}, -k.exchange_id)");
    let mut s = c.prepare(&format!("SELECT {account}, k.balances_synced_at FROM api_keys k WHERE k.user_id = ?1 AND {} ORDER BY k.id", scope.keys("k")))?;
    let rows: Vec<(i64, Option<String>)> = s.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut marks: HashMap<i64, At> = HashMap::new();
    for (account, at) in rows {
        crate::figures::budget::charge(1, 0)?;
        let Some(at) = instant(at)? else { continue };
        let m = marks.entry(account).or_insert(at);
        if at > *m { *m = at }
    }
    Ok(marks)
}

/// `Figures.moved_since(pending_scope, watermarks)`: what the ledger has moved since each account's balances were taken,
/// by symbol, the cash it spent or returned included. An account with no watermark is not brought forward. A transfer is
/// linked here only when its partner moved since too (`quantity_row`).
pub fn pending(c: &Connection, user_id: i64, scope: &super::Scope, labels: &super::labels::Labels) -> Result<Vec<(String, Dec)>, FiguresError> {
    let marks = watermarks(c, user_id, scope)?;
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
    let mut rows: Vec<super::rows::Stored> = vec![];
    let any = crate::venue_ids::Scope::install(c)?;
    let mut s = c.prepare(&format!(
        "SELECT exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, {}, transacted_at, id, fee_currency, fee_amount, group_id, {}, \
         COALESCE({}, -exchange_id) FROM account_transactions WHERE user_id = ?1 AND {} AND {} ORDER BY id",
        any.row_id("account_transactions"), any.member_sql("account_transactions"), any.row_account("account_transactions"), scope.rows("account_transactions"),
        crate::identity::counted(c, user_id, &any.member_sql("account_transactions"))?))?;
    let mut q = s.query([user_id])?;
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
        let account: i64 = r.get(13)?;
        let Some(&taken) = marks.get(&account) else { continue };
        if at < taken { continue; }
        let kind = Kind::from_stored(r.get(1)?).ok_or_else(|| FiguresError::Data("an entry type 2.x does not have".into()))?;
        let amount = Dec::from_sql(r.get_ref(3)?)?.ok_or_else(|| FiguresError::Data("a row with no base amount".into()))?;
        rows.push(super::rows::Stored {
            id: r.get(8)?, exchange_id: exchange, venue: String::new(), kind, base: r.get(2)?, amount, quote: r.get(4)?, quote_amount: Dec::from_sql(r.get_ref(5)?)?,
            fee_currency: r.get(9)?, fee_amount: Dec::from_sql(r.get_ref(10)?)?, tx_id, group: super::rows::group_id(r.get(11)?),
            at,
            per_share: None, stated: None, linked_to: None, to_venue: None, to_exchange_id: None, base_asset: None, cfee: Default::default(), bot_order: None,
            account, to_account: None, to_base: None,
        });
    }
    labels.relabel(&mut rows);
    let ids: HashSet<i64> = rows.iter().map(|r| r.id).collect();
    let amounts: HashMap<i64, Dec> = rows.iter().map(|r| (r.id, r.amount.clone())).collect();
    // D-TRACKER-COIN-QUOTE: a coin-quoted single row moves as its two legs, as the walk reads it (a new leg is unlinked).
    for r in &super::rows::split_coin_quoted_trades(rows, true)? {
        let partner = links.get(&r.id).or_else(|| inverse.get(&r.id)).copied().filter(|p| !labels.void(r.id, *p));
        let linked = partner.is_some_and(|p| ids.contains(&p));
        let fee = match (linked, r.kind, partner.and_then(|p| amounts.get(&p))) { (true, Kind::Withdrawal, Some(p)) => Some((&r.amount - p)?), _ => None };
        for (symbol, a) in quantity_moves(r.kind, &r.base, &r.amount, r.fee_currency.as_ref(), r.fee_amount.as_ref(), linked, fee.as_ref())? { if !cash(&symbol) { add(symbol.to_string(), a)?; } }
        for (currency, a) in cash_moves(r.kind, &r.base, &r.amount, r.quote.as_ref(), r.quote_amount.as_ref(), r.fee_currency.as_ref(), r.fee_amount.as_ref())? { add(currency.to_string(), a)?; }
    }
    Ok(moved)
}

/// One holding: what the venue holds of a symbol (and what arrived since its sync), what it is worth, and its cost
/// against the history (none for cash).
#[derive(Clone, Debug)]
pub struct Holding { pub asset: Option<i64>, pub symbol: String, pub quantity: Dec, pub value: Dec, pub cost: Option<Dec>, pub cash: bool, pub unrealised: Option<Dec>, pub note: Option<Note>,
    /// Whether `value` is a market value (Rust only, PLAN-venue-prices S1): false when it stands in for one (a coin the
    /// venue reports at cost for want of a price, other cash at par for want of an ECB rate). A coin arrived since the
    /// sync, at cost under its `since_sync` note, counts as priced. The arithmetic is 2.x's; the page shows
    /// no number for an unpriced holding.
    pub priced: bool }

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

/// `NOTE_FLOOR`: an assumption of a dollar or more is stated; below it, applied in silence unless always stated.
pub(crate) fn stated(amount: &Dec) -> bool { (if amount.is_negative() { amount.neg() } else { amount.clone() }) >= Dec::one() }

/// The notes a computation wrote, in the order it wrote them (`@notes`).
#[derive(Default)]
struct Notes(Vec<Note>);

impl Notes {
    /// `note`: below a dollar an assumption is applied in silence (`NOTE_FLOOR`), unless it is always stated.
    #[allow(clippy::too_many_arguments)] // 2.x's `note(kind, symbol, exchange, history, held, amount_usd, always:)`
    fn note(&mut self, kind: &'static str, symbol: &str, exchange: Option<&str>, history: Option<&Dec>, held: Option<&Dec>, amount: Dec, always: bool) -> Option<Note> {
        if !(always || stated(&amount)) { return None; }
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

    /// Whether every holding has a market value (`Holding::priced`).
    pub fn all_priced(&self) -> bool { self.holdings.iter().all(|h| h.priced) }

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
    let held: Option<i64> = c.query_row("SELECT b.asset_id FROM exchange_account_balances b JOIN assets a ON a.id = b.asset_id WHERE b.user_id = ?1 AND a.symbol = ?2 ORDER BY b.id LIMIT 1",
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

/// `venue_of`: the venue a coin's history was booked on, when it is one venue: one account (#710 slice 6), named by its label.
fn venue_of(c: &Connection, user_id: i64, symbol: &str) -> Result<Option<String>, FiguresError> {
    let mut s = c.prepare("SELECT e.name, e.id FROM exchanges e WHERE e.id IN (SELECT exchange_id FROM account_transactions WHERE user_id = ?1 AND base_currency = ?2)")?;
    let names: Vec<(String, i64)> = s.query_map(rusqlite::params![user_id, symbol], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    crate::figures::budget::charge(names.len() as u64, 0)?;
    let Ok([(name, exchange)]) = <[_; 1]>::try_from(names) else { return Ok(None) };
    if !crate::identity::numbered(c)? || crate::identity::accounts_of(c, user_id, exchange)?.len() < 2 { return Ok(Some(name)); }
    let mut s = c.prepare(&format!("SELECT DISTINCT {} FROM account_transactions t WHERE t.user_id = ?1 AND t.exchange_id = ?2 AND t.base_currency = ?3", crate::venue_ids::row_account("t")))?;
    let accounts: Vec<Option<i64>> = s.query_map(rusqlite::params![user_id, exchange, symbol], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(match accounts.as_slice() { [Some(id)] => Some(crate::identity::label(c, *id)?.plain()), _ => None })
}

/// `balances` under their labels: each takes its account's label for its asset, else keeps its symbol.
pub(crate) fn labelled(balances: &[Balance], labels: &super::labels::Labels) -> Vec<Balance> {
    balances.iter().map(|b| { let mut b = b.clone(); if let Some(l) = labels.asset(b.account, b.asset) { b.symbol = l.to_string(); } b }).collect()
}

/// A (key, value) list by its key; each entry a step.
fn indexed(list: &[(String, Dec)]) -> Result<HashMap<&str, &Dec>, FiguresError> {
    crate::figures::budget::charge(list.len() as u64, 0)?;
    Ok(list.iter().map(|(k, v)| (k.as_str(), v)).collect())
}

/// `Figures.for(user, ledger:, balances:, pending:)`. Every symbol's balances, pending move and position are found
/// through an index, each lookup a step.
pub fn compute(c: &Connection, user_id: i64, ledger: &Summary, balances: &[Balance], pending: &[(String, Dec)]) -> Result<Figures, FiguresError> {
    compute_on(c, user_id, &super::Scope::All, ledger, balances, pending, &FxTable::load(c)?, chrono::Utc::now().date_naive())
}

/// The refusal when one symbol names two asset classes and nothing says which a row is.
pub const AMBIGUOUS_CLASS: &str = "Tracker figures unavailable: ambiguous asset class for a shared symbol";

/// `compute` with the ECB table read and today's date.
/// `scope`: the figures' own scope, whose duplicates' balances are classification evidence (#710).
#[allow(clippy::too_many_arguments)]
pub fn compute_on(c: &Connection, user_id: i64, scope: &super::Scope, ledger: &Summary, balances: &[Balance], pending: &[(String, Dec)], fx: &FxTable, today: NaiveDate) -> Result<Figures, FiguresError> {
    // Delisted coins: balances join the positions by the map's label (`pending` is labelled as it is read).
    let labelled = labelled(balances, &ledger.labels);
    let balances = labelled.as_slice();
    let spelling = |label: &str| ledger.labels.spelling(label).to_string();
    // Historical ledger keys remain unchanged; refuse mixed classes before quantities merge.
    let mut classes: HashMap<&str, HashSet<Option<String>>> = HashMap::new();
    // Fiat and Currency are both cash, and a tokenized share tracks its company's share: one class each.
    let class = |c: Option<String>| match c.as_deref() { Some("Currency") => Some("Fiat".to_string()), Some("Tokenized Stock") => Some("Stock".to_string()), _ => c };
    for balance in balances { classes.entry(&balance.symbol).or_default().insert(class(balance.category.clone())); }
    for (symbol, _) in pending {
        // Only the assets this user's own rows recorded: the catalogue's BTC stock beside BTC crypto is not ambiguity here.
        // Classification (#710): a duplicate is the same venue account, so its rows are evidence of what an asset is, as on main.
        let mut stmt = c.prepare("SELECT DISTINCT a.category FROM account_transactions t JOIN assets a ON a.id = t.base_asset_id \
                                  WHERE t.user_id = ?1 AND t.base_currency = ?2")?;
        for category in stmt.query_map(rusqlite::params![user_id, spelling(symbol)], |r| r.get::<_, Option<String>>(0))? {
            classes.entry(symbol).or_default().insert(class(category?));
        }
    }
    // Classification (#710): a duplicate's balances are evidence of what its held assets are, though they count for no
    // amount (`balances` leaves them out), for every symbol the figures name (a balance's or a pending movement's). An
    // account's chip reads only its own exchange's duplicates.
    let exchange = match scope { super::Scope::All => None, super::Scope::Account(a) => Some(a.exchange_id), super::Scope::EmptyLegacyExchange(id) => Some(*id) };
    let duplicates = crate::identity::duplicate_ids(c, user_id)?;
    if !duplicates.is_empty() {
        let list = duplicates.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let mut stmt = c.prepare(&format!("SELECT a.symbol, a.category FROM exchange_account_balances b JOIN assets a ON a.id = b.asset_id \
                                           WHERE b.user_id = ?1 AND b.account_id IN ({list}) AND (?2 IS NULL OR b.exchange_id = ?2) AND b.free + b.locked > 0"))?;
        let held: Vec<(Option<String>, Option<String>)> = stmt.query_map(rusqlite::params![user_id, exchange], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        let named: HashSet<&str> = balances.iter().map(|b| b.symbol.as_str()).chain(pending.iter().map(|(s, _)| s.as_str())).collect();
        for (symbol, category) in held {
            let Some(symbol) = symbol.as_deref().and_then(|s| named.get(s).copied()) else { continue };
            classes.entry(symbol).or_default().insert(class(category));
        }
    }
    if classes.values().any(|categories| categories.len() > 1) {
        return Err(FiguresError::NotComputed(AMBIGUOUS_CLASS.into()));
    }
    let zero = Dec::zero();
    let mut moved = Dec::zero();
    // The symbols in the order 2.x meets them (balances, then what arrived since), each with its balance rows.
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
            if let Some(symbol) = r.get::<_, Option<String>>(0)? { known.insert(symbol); }
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
        if rows.is_empty() && !known.contains(&spelling(&symbol)) { continue; }
        let asset = match rows.first() { Some(r) => Some(r.asset), None => match ledger.labels.asset_of(&symbol) { Some(a) => Some(a), None => asset_for(c, user_id, &spelling(&symbol))? } };
        let exchange = rows.first().map(|r| r.exchange.as_str()).filter(|name| rows.iter().all(|r| r.exchange == *name));
        let mut prices = Dec::zero();
        let mut values = Dec::zero();
        for r in rows {
            prices = (&prices + r.usd_price.as_ref().unwrap_or(&zero))?;
            values = (&values + r.usd_value.as_ref().unwrap_or(&zero))?;
        }
        let price = if prices.is_positive() { Some(values.div(&units)?) } else { None };
        // A row of the symbol with no value leaves the symbol's price short of it: the whole holding is unpriced.
        let every_row_valued = rows.iter().all(|r| r.usd_value.as_ref().is_some_and(Dec::is_positive));
        if cash(&symbol) {
            // Priced by the rate actually used: the balances' own when they have one (then every row must be valued), else ECB's.
            let priced = symbol == "USD" || stable(&symbol) || if price.is_some() { every_row_valued } else { fx.rate(&symbol, "USD", today)?.is_some() };
            let value = if stable(&symbol) { held.clone() } else { (&held * &match &price { Some(p) => p.clone(), None => rate(&symbol, fx, today, &mut notes)? })? };
            holdings.push(Holding { asset, symbol, quantity: held, value, cost: None, cash: true, unrealised: None, note: None, priced });
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
        // A coin arrived since the sync is held at cost under its own note until the venue reports it (2.x's reading,
        // kept: a bot's buy between two syncs is not a missing price).
        let priced = (price.is_some() || since_sync) && every_row_valued;
        holdings.push(Holding { asset, symbol, quantity: held, unrealised: Some((&value - &cost)?), value, cost: Some(cost), cash: false, note, priced });
    }
    // `sort_by { -value }`.
    holdings.sort_by(|a, b| b.value.cmp(&a.value));
    // `resolve_departed`: coins the history holds that no venue reports, left at cost.
    let held_symbols: HashSet<String> = holdings.iter().map(|h| h.symbol.clone()).collect();
    for p in &ledger.positions {
        crate::figures::budget::charge(1, 0)?;
        if held_symbols.contains(p.symbol.as_str()) || cash(&p.symbol) || !p.quantity.is_positive() { continue; }
        moved = (&moved - &p.cost)?;
        notes.note("left", &p.symbol, venue_of(c, user_id, &spelling(&p.symbol))?.as_deref(), Some(&p.quantity), Some(&Dec::zero()), p.cost.clone(), false);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn balance(symbol: &str, category: &str, exchange: &str, value: Option<&str>) -> Balance {
        Balance { asset: 1, account: -1, category: Some(category.into()), symbol: symbol.into(), exchange: exchange.into(), free: Dec::one(), locked: Dec::zero(),
                  usd_price: value.map(|v| Dec::strict(v).unwrap()), usd_value: value.map(|v| Dec::strict(v).unwrap()), priced_at: None, synced_at: None }
    }

    fn priced(balances: &[Balance], pending: &[(String, Dec)]) -> Vec<(String, bool)> {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE assets (id INTEGER PRIMARY KEY, symbol TEXT, category TEXT);
                         CREATE TABLE account_transactions (user_id INTEGER, base_currency TEXT, base_asset_id INTEGER, exchange_id INTEGER);
                         CREATE TABLE exchange_account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, asset_id INTEGER);
                         CREATE TABLE exchanges (id INTEGER PRIMARY KEY, name TEXT);
                         INSERT INTO assets VALUES (1, 'BTC', 'Cryptocurrency'), (2, 'SOL', 'Cryptocurrency'), (3, 'EUR', 'Fiat');").unwrap();
        let f = crate::figures::budget::within(|| compute_on(&c, 1, &crate::tracker::Scope::All, &Summary::empty(), balances, pending, &FxTable::default(), NaiveDate::from_ymd_opt(2026, 11, 16).unwrap())).unwrap();
        let mut out: Vec<(String, bool)> = f.holdings.iter().map(|h| (h.symbol.clone(), h.priced)).collect();
        out.sort();
        out
    }

    /// PLAN-venue-prices S1: a holding is priced only when its value is a market value: a coin with a value on every one
    /// of its balance rows, cash at a balance price or an ECB rate; other cash at par for want of a rate is not. A coin
    /// arrived since the sync keeps 2.x's labelled cost reading.
    #[test]
    fn a_holding_valued_without_a_market_price_is_not_priced() {
        assert_eq!(priced(&[balance("BTC", "Cryptocurrency", "Kraken", Some("60000"))], &[]), [("BTC".to_string(), true)]);
        assert_eq!(priced(&[balance("BTC", "Cryptocurrency", "Alpaca", None)], &[]), [("BTC".to_string(), false)]);
        assert_eq!(priced(&[balance("BTC", "Cryptocurrency", "Kraken", Some("60000")), balance("BTC", "Cryptocurrency", "Alpaca", None)], &[]),
                   [("BTC".to_string(), false)], "one venue's row with no value");
        assert_eq!(priced(&[], &[("SOL".to_string(), Dec::one())]), [("SOL".to_string(), true)], "arrived since the sync: at cost under its own note");
        assert_eq!(priced(&[balance("EUR", "Fiat", "Kraken", None)], &[]), [("EUR".to_string(), false)], "no ECB rate: at par");
        assert_eq!(priced(&[balance("EUR", "Fiat", "Kraken", Some("110")), balance("EUR", "Fiat", "Bitvavo", None)], &[]), [("EUR".to_string(), false)],
                   "the balances' price leaves one row out");
        assert_eq!(priced(&[balance("USD", "Fiat", "Kraken", None), balance("USDT", "Cryptocurrency", "Kraken", None)], &[]),
                   [("USD".to_string(), true), ("USDT".to_string(), true)]);
    }
}
