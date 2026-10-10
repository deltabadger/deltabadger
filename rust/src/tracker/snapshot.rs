//! PortfolioSnapshot.record! (app/models/portfolio_snapshot.rb): today's row for the whole account and one per venue
//! the account has rows or balances on, each the figures the tiles state, written as Rails writes them.
use super::figures::{self, Balance};
use super::walk::{Summary, Walked};
use crate::figures::{dec::Dec, FiguresError};
use chrono::NaiveDate;
use rusqlite::Connection;

/// One day of one table: value, money in, and the same with the cash taken off both sides.
#[derive(Clone, Debug, PartialEq)]
pub struct Day { pub value: Dec, pub invested: Dec, pub held_value: Option<Dec>, pub held_cost: Option<Dec>, pub partial: bool }

/// A decimal(20, 8) column as ActiveRecord writes one: rounded to the scale (half up), as a literal SQLite reads into
/// a number.
pub fn column(d: &Dec) -> Result<String, FiguresError> { Ok(d.round(8)?.to_s_f()) }

/// `partial?`: something held could not be priced, a key's last sync failed, or prices lag the balances beside them
/// by more than five minutes.
pub fn partial(c: &Connection, user_id: i64, balances: &[Balance]) -> Result<bool, FiguresError> {
    if balances.iter().any(|b| b.usd_value.as_ref().is_none_or(Dec::is_zero)) { return Ok(true); }
    // ApiKey#sync_issue's :failed: a key that is not the withdrawal key, with a recorded error.
    let mut s = c.prepare("SELECT last_sync_error FROM api_keys WHERE user_id = ?1 AND key_type != 1")?;
    let errors: Vec<Option<String>> = s.query_map([user_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    if errors.iter().flatten().any(|e| !e.trim().is_empty()) { return Ok(true); }
    let oldest = balances.iter().filter_map(|b| b.priced_at).min();
    let newest = balances.iter().filter_map(|b| b.synced_at).max();
    Ok(matches!((oldest, newest), (Some(o), Some(n)) if n.minus(o) > 300.0))
}

/// `today_row`: none when the scope has neither a balance nor a row.
pub fn today_row(c: &Connection, user_id: i64, exchange_id: Option<i64>, ledger: &Summary) -> Result<Option<Day>, FiguresError> {
    if c.is_autocommit(){
        let tx=c.unchecked_transaction()?;
        let origin=crate::sync::cache::capture_read(&tx,user_id,exchange_id).map_err(|_|FiguresError::Data("balance cache provenance unavailable".into()))?;
        let mut day=today_row(&tx,user_id,exchange_id,ledger)?;tx.commit()?;
        if !crate::sync::cache::read_is_current(c,&origin).map_err(|_|FiguresError::Data("balance cache provenance unavailable".into()))?{if let Some(day)=&mut day{day.partial=true;}}
        return Ok(day)
    }

    let balances = figures::balances(c, user_id, exchange_id)?;
    let rows: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND (?2 IS NULL OR exchange_id = ?2))", rusqlite::params![user_id, exchange_id], |r| r.get(0))?;
    if balances.is_empty() && !rows { return Ok(None); }
    let pending = figures::pending(c, user_id, exchange_id)?;
    let f = figures::compute(c, user_id, ledger, &balances, &pending)?;
    let (held_value, held_cost) = f.without_cash()?;
    Ok(Some(Day { value: f.value, invested: f.invested, held_value: Some(held_value), held_cost: Some(held_cost),
                  partial: crate::sync::cache::stale(c, user_id, exchange_id).map_err(|_| FiguresError::Data("balance cache provenance unavailable".into()))? || partial(c, user_id, &balances)? || ledger.incomplete }))
}

/// `venues`: the exchanges the account has rows or balances on, by id.
pub fn venues(c: &Connection, user_id: i64) -> Result<Vec<i64>, FiguresError> {
    let mut s = c.prepare("SELECT id FROM exchanges WHERE id IN (SELECT exchange_id FROM account_transactions WHERE user_id = ?1 \
                           UNION SELECT exchange_id FROM account_balances WHERE user_id = ?1 AND free + locked > 0) ORDER BY id")?;
    let ids = s.query_map([user_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

fn values(day: &Day) -> Result<[Option<String>; 4], FiguresError> {
    Ok([Some(column(&day.value)?), Some(column(&day.invested)?), day.held_value.as_ref().map(column).transpose()?, day.held_cost.as_ref().map(column).transpose()?])
}

/// `PortfolioSnapshot.upsert(row, unique_by: %i[user_id date], record_timestamps: true)`, as Rails generates it.
pub fn upsert_whole(c: &crate::engine::model::FencedTransaction<'_>, user_id: i64, date: NaiveDate, day: &Day) -> Result<(), FiguresError> {
    let [value, invested, held_value, held_cost] = values(day)?;
    c.prepare_cached(
        "INSERT INTO portfolio_snapshots (user_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')) \
         ON CONFLICT (user_id, date) DO UPDATE SET updated_at = (CASE WHEN (value_usd IS excluded.value_usd AND invested_usd IS excluded.invested_usd \
         AND held_value_usd IS excluded.held_value_usd AND held_cost_usd IS excluded.held_cost_usd AND partial IS excluded.partial) \
         THEN portfolio_snapshots.updated_at ELSE STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW') END), value_usd = excluded.value_usd, invested_usd = excluded.invested_usd, \
         held_value_usd = excluded.held_value_usd, held_cost_usd = excluded.held_cost_usd, partial = excluded.partial")?
        .execute(rusqlite::params![user_id, date.to_string(), value, invested, held_value, held_cost, day.partial])?;
    record_origin(c,user_id,None,date)?;
    Ok(())
}

/// `PortfolioVenueSnapshot.upsert_all(rows, unique_by: %i[user_id exchange_id date], record_timestamps: true)`.
pub fn upsert_venue(c: &crate::engine::model::FencedTransaction<'_>, user_id: i64, exchange_id: i64, date: NaiveDate, day: &Day) -> Result<(), FiguresError> {
    let [value, invested, held_value, held_cost] = values(day)?;
    c.prepare_cached(
        "INSERT INTO portfolio_venue_snapshots (user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW')) \
         ON CONFLICT (user_id, exchange_id, date) DO UPDATE SET updated_at = (CASE WHEN (value_usd IS excluded.value_usd AND invested_usd IS excluded.invested_usd \
         AND held_value_usd IS excluded.held_value_usd AND held_cost_usd IS excluded.held_cost_usd AND partial IS excluded.partial) \
         THEN portfolio_venue_snapshots.updated_at ELSE STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW') END), value_usd = excluded.value_usd, invested_usd = excluded.invested_usd, \
         held_value_usd = excluded.held_value_usd, held_cost_usd = excluded.held_cost_usd, partial = excluded.partial")?
        .execute(rusqlite::params![user_id, exchange_id, date.to_string(), value, invested, held_value, held_cost, day.partial])?;
    record_origin(c,user_id,Some(exchange_id),date)?;
    Ok(())
}

/// The venue every walked row is on: its id.
fn walked_venue(c: &Connection) -> Result<Option<i64>, FiguresError> {
    use rusqlite::OptionalExtension;
    Ok(c.query_row("SELECT id FROM exchanges WHERE type = ?1 ORDER BY id LIMIT 1", [super::VENUE_TYPE], |r| r.get(0)).optional()?)
}

/// `PortfolioSnapshot.record!(user, scopes:)`'s rows, from the scopes just walked: the whole account's (no venue) and
/// each venue's, not yet dated. A venue the walk has no rows of reads as an empty ledger, as Rails reads one. Computed
/// before the write unit, so the write lock is held for the writes alone.
pub fn today_rows(c: &Connection, user_id: i64, walked: &Walked) -> Result<Vec<(Option<i64>, Day)>, FiguresError> {
    let mut rows = vec![];
    if let Some(day) = today_row(c, user_id, None, &walked.whole)? { rows.push((None, day)); }
    let alpaca = walked_venue(c)?;
    let empty = Summary::empty();
    for exchange_id in venues(c, user_id)? {
        let ledger = match (&walked.venue, alpaca) { (Some(v), Some(a)) if a == exchange_id => v, _ => &empty };
        if let Some(day) = today_row(c, user_id, Some(exchange_id), ledger)? { rows.push((Some(exchange_id), day)); }
    }
    Ok(rows)
}

/// `record!`'s upserts: today's rows, dated `today`.
pub fn write(c: &crate::engine::model::FencedTransaction<'_>, user_id: i64, rows: &[(Option<i64>, Day)], today: NaiveDate) -> Result<(), FiguresError> {
    for (scope, day) in rows {
        crate::figures::budget::charge(1, 0)?;
        match scope { None => upsert_whole(c, user_id, today, day)?, Some(id) => upsert_venue(c, user_id, *id, today, day)? }
    }
    Ok(())
}

fn record_origin(c:&crate::engine::model::FencedTransaction<'_>,user:i64,scope:Option<i64>,date:NaiveDate)->Result<(),FiguresError>{
    let key=crate::jobs::state::key("snapshot_origin",Some(&format!("{user}:{}:{date}",match scope{Some(v)=>v.to_string(),None=>"all".into()})));
    crate::app_config::set_plain(c,&key,&c.producer_stamp().to_string(),chrono::Utc::now()).map_err(FiguresError::Data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figures::at::At;

    /// decimal(20, 8) as ActiveRecord writes it: `BigDecimal#round(8).to_s('F')` (measured in Ruby: 1.12345679, 2.0,
    /// -1.0, 0.00000001).
    #[test]
    fn a_value_is_written_as_rails_writes_a_decimal_column() {
        let written: Vec<String> = ["1.123456785", "2", "-1.000000004", "0.000000005"].iter().map(|v| column(&Dec::strict(v).unwrap()).unwrap()).collect();
        assert_eq!(written, ["1.12345679", "2.0", "-1.0", "0.00000001"]);
    }

    fn balance(usd_value: Option<&str>, priced_at: &str) -> Balance {
        Balance { category: Some("Stock".into()), symbol: "AAPL".into(), free: Dec::one(), locked: Dec::zero(), usd_price: None, usd_value: usd_value.map(|v| Dec::strict(v).unwrap()),
                  priced_at: At::from_sql(priced_at), synced_at: At::from_sql("2026-10-01 02:30:00") }
    }

    /// `partial?`: a holding with no value, a key whose last sync failed, or prices more than five minutes behind the
    /// balances beside them.
    #[test]
    fn a_day_is_partial_when_a_holding_is_unpriced_a_sync_failed_or_prices_lag() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys (id INTEGER PRIMARY KEY, user_id INTEGER, key_type INTEGER, last_sync_error TEXT);
                         INSERT INTO api_keys VALUES (1, 1, 0, NULL), (2, 1, 1, 'withdrawal keys do not count');").unwrap();
        let on_time = balance(Some("210"), "2026-10-01 02:25:00");
        assert!(!partial(&c, 1, std::slice::from_ref(&on_time)).unwrap(), "300 seconds behind");
        assert!(partial(&c, 1, &[on_time.clone(), balance(Some("0"), "2026-10-01 02:30:00")]).unwrap(), "a zero value");
        assert!(partial(&c, 1, &[balance(Some("210"), "2026-10-01 02:24:59")]).unwrap(), "301 seconds behind");
        c.execute("UPDATE api_keys SET last_sync_error = 'Faraday::TimeoutError' WHERE id = 1", []).unwrap();
        assert!(partial(&c, 1, &[on_time]).unwrap(), "a failed sync");
    }
}
