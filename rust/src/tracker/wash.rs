//! Tracker::LedgerJob#arm_wash_sale_locks and WashSaleLock.confirm!: every sale the walk judged a loss inside the
//! horizon locks the symbol's assets out of every buy leg for the user's wash-sale window. Re-derived on every walk,
//! idempotent, and never shortening a lock. The engine refuses an install with wash-sale protection on
//! (`eligibility::check_install`), so these rows are what Rails' bots read after a handback; eligibility reads none of
//! them, so no write here passes the guard.
use crate::codec::format_time;
use crate::figures::FiguresError;
use chrono::{DateTime, Days, NaiveDate, Utc};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// User#wash_sale_days: zero unless the rule is on; else the window of the user's jurisdiction (US when none is set).
pub fn days(c: &Connection, user_id: i64) -> Result<u64, FiguresError> {
    let (enabled, jurisdiction): (Option<bool>, Option<String>) =
        c.query_row("SELECT wash_sale_enabled, wash_sale_jurisdiction FROM users WHERE id = ?1", [user_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if enabled != Some(true) { return Ok(0); }
    // Tax::Jurisdictions: the registry's wash_sale_days (`presence || 'US'`).
    Ok(match jurisdiction.filter(|j| !j.trim().is_empty()).as_deref().unwrap_or("US") { "US" | "GB" => 30, "IE" => 28, _ => 0 })
}

/// What `arm_wash_sale_locks(user, summary)` locks: per loss sale, every asset a ticker of that symbol names
/// (over-locking is the safe direction), with the window's days. Read before the write unit: the tickers in one query
/// (Rails' `Ticker.where(base: symbols).pluck`, ledger_job.rb:48), each row read a step, indexed by symbol.
pub fn targets(c: &Connection, user_id: i64, loss_sales: &[(String, NaiveDate)]) -> Result<(u64, Vec<(i64, NaiveDate)>), FiguresError> {
    let days = days(c, user_id)?;
    if days == 0 || loss_sales.is_empty() { return Ok((days, vec![])); }
    let wanted: HashSet<&str> = loss_sales.iter().map(|(s, _)| s.as_str()).collect();
    let mut by_symbol: HashMap<String, Vec<i64>> = HashMap::new();
    let mut s = c.prepare("SELECT base, base_asset_id FROM tickers ORDER BY id")?;
    let mut q = s.query([])?;
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        let (base, asset): (String, Option<i64>) = (r.get(0)?, r.get(1)?);
        let Some(asset) = asset else { continue };
        if !wanted.contains(base.as_str()) { continue; }
        let assets = by_symbol.entry(base).or_default();
        if !assets.contains(&asset) { assets.push(asset); } // one symbol's tickers: a handful
    }
    let mut out = vec![];
    for (symbol, on) in loss_sales {
        crate::figures::budget::charge(1, 0)?;
        for asset in by_symbol.get(symbol).map_or(&[][..], Vec::as_slice) { out.push((*asset, *on)); }
    }
    Ok((days, out))
}

/// The locks `targets` named, confirmed (inside the caller's write unit).
/// The user's lock rows are read in one query and indexed by asset; each target is then one write.
pub fn confirm_all(c: &Connection, user_id: i64, (days, targets): &(u64, Vec<(i64, NaiveDate)>), now: DateTime<Utc>) -> Result<(), FiguresError> {
    if targets.is_empty() { return Ok(()); }
    let mut locks: HashMap<i64, i64> = HashMap::new();
    let mut s = c.prepare("SELECT asset_id, id FROM wash_sale_locks WHERE user_id = ?1 ORDER BY id")?;
    let mut q = s.query([user_id])?;
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        let (asset, id): (i64, i64) = (r.get(0)?, r.get(1)?);
        locks.entry(asset).or_insert(id);
    }
    for (asset_id, on) in targets {
        crate::figures::budget::charge(1, 0)?;
        confirm(c, user_id, &mut locks, *asset_id, *on, *days, now)?;
    }
    Ok(())
}

/// `arm_wash_sale_locks(user, summary)`: `targets`, confirmed.
pub fn arm(c: &Connection, user_id: i64, loss_sales: &[(String, NaiveDate)], now: DateTime<Utc>) -> Result<(), FiguresError> {
    confirm_all(c, user_id, &targets(c, user_id, loss_sales)?, now)
}

/// `WashSaleLock.confirm!(user:, asset_id:, from:, source: 'ledger')`: the deadline is the start of the day `days + 1`
/// after the sale; both deadlines are raised to it, never lowered.
fn confirm(c: &Connection, user_id: i64, locks: &mut HashMap<i64, i64>, asset_id: i64, on: NaiveDate, days: u64, now: DateTime<Utc>) -> Result<(), FiguresError> {
    let deadline = on.checked_add_days(Days::new(days + 1)).and_then(|d| d.and_hms_opt(0, 0, 0)).map(|t| format_time(t.and_utc()))
        .ok_or_else(|| FiguresError::Data("a deadline out of range".into()))?;
    let id = match locks.get(&asset_id) {
        Some(&id) => id,
        None => {
            let at = format_time(now);
            c.execute("INSERT INTO wash_sale_locks (asset_id, user_id, source, created_at, updated_at) VALUES (?1, ?2, 'bot', ?3, ?3)", rusqlite::params![asset_id, user_id, at])?;
            let id = c.last_insert_rowid();
            locks.insert(asset_id, id);
            id
        }
    };
    c.execute("UPDATE wash_sale_locks SET confirmed_locked_until = COALESCE(MAX(confirmed_locked_until, ?1), ?1), \
               buy_locked_until = COALESCE(MAX(buy_locked_until, ?1), ?1), source = 'ledger' WHERE id = ?2", rusqlite::params![deadline, id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(enabled: bool) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, wash_sale_enabled BOOLEAN, wash_sale_jurisdiction TEXT);
                         CREATE TABLE tickers (id INTEGER PRIMARY KEY, base TEXT, base_asset_id INTEGER);
                         INSERT INTO tickers VALUES (1, 'AAPL', 10), (2, 'AAPL', 11), (3, 'AAPL', 10), (4, 'QQQM', 12);
                         CREATE TABLE wash_sale_locks (id INTEGER PRIMARY KEY, asset_id INTEGER, user_id INTEGER, source TEXT, created_at TEXT, updated_at TEXT,
                                                       confirmed_locked_until TEXT, buy_locked_until TEXT);
                         INSERT INTO wash_sale_locks (asset_id, user_id, source, created_at, updated_at, buy_locked_until)
                         VALUES (11, 1, 'bot', '2026-09-01 00:00:00', '2026-09-01 00:00:00', '2026-12-01 00:00:00');").unwrap();
        c.execute("INSERT INTO users VALUES (1, ?1, NULL)", [enabled]).unwrap();
        c
    }

    fn locks(c: &Connection) -> Vec<(i64, Option<String>, Option<String>, String)> {
        let mut s = c.prepare("SELECT asset_id, confirmed_locked_until, buy_locked_until, source FROM wash_sale_locks ORDER BY asset_id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }

    /// A loss sale on 20 September locks every asset a ticker of its symbol names until the start of the 31st day after
    /// it (US: 30 days), never shortening a longer lock; with protection off nothing is written.
    #[test]
    fn a_loss_sale_locks_every_asset_of_its_symbol_and_never_shortens_a_lock() {
        let now: DateTime<Utc> = "2026-10-01T03:00:00Z".parse().unwrap();
        let sale = [("AAPL".to_string(), NaiveDate::from_ymd_opt(2026, 9, 20).unwrap())];
        let c = db(true);
        arm(&c, 1, &sale, now).unwrap();
        let until = Some("2026-10-21 00:00:00".to_string());
        assert_eq!(locks(&c), [(10, until.clone(), until.clone(), "ledger".to_string()), (11, until, Some("2026-12-01 00:00:00".to_string()), "ledger".to_string())]);
        let off = db(false);
        arm(&off, 1, &sale, now).unwrap();
        assert_eq!(locks(&off).len(), 1, "protection off: the bot's lock alone");
    }
}
