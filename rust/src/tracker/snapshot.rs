//! PortfolioSnapshot.record!: today's row for the whole account and one per venue
//! the account has rows or balances on, each the figures the tiles state, written as 2.x writes them.
use super::figures::{self, Balance};
use super::walk::{Summary, Walked};
use super::Scope;
use crate::identity::AccountRow;
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
    // A duplicate account's key is no key of the figures (#710, PLAN-710-6 §5.2).
    let mut s = c.prepare(&format!("SELECT k.last_sync_error FROM api_keys k WHERE k.user_id = ?1 AND k.key_type != 1 AND {}", crate::identity::key_counted(c, user_id, "k")?))?;
    let errors: Vec<Option<String>> = s.query_map([user_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    if errors.iter().flatten().any(|e| !e.trim().is_empty()) { return Ok(true); }
    let oldest = balances.iter().filter_map(|b| b.priced_at).min();
    let newest = balances.iter().filter_map(|b| b.synced_at).max();
    Ok(matches!((oldest, newest), (Some(o), Some(n)) if n.minus(o) > 300.0))
}

/// `today_row`: none when the scope has neither a balance nor a row.
pub fn today_row(c: &Connection, user_id: i64, scope: &Scope, ledger: &Summary) -> Result<Option<Day>, FiguresError> {
    today_row_on(c, user_id, scope, ledger, &super::fx::FxTable::load(c)?, chrono::Utc::now().date_naive())
}

/// `today_row` with the ECB table read and today's date (Figures values other cash at today's rate).
pub fn today_row_on(c: &Connection, user_id: i64, scope: &Scope, ledger: &Summary, fx: &super::fx::FxTable, today: NaiveDate) -> Result<Option<Day>, FiguresError> {
    if c.is_autocommit(){
        let tx=c.unchecked_transaction()?;
        let origin=crate::sync::cache::capture_read(&tx,user_id,scope).map_err(|_|FiguresError::Data("balance cache provenance unavailable".into()))?;
        let mut day=today_row_on(&tx,user_id,scope,ledger,fx,today)?;tx.commit()?;
        if !crate::sync::cache::read_is_current(c,&origin).map_err(|_|FiguresError::Data("balance cache provenance unavailable".into()))?{if let Some(day)=&mut day{day.partial=true;}}
        return Ok(day)
    }

    let balances = figures::balances(c, user_id, scope)?;
    let counted = crate::identity::counted(c, user_id, &crate::venue_ids::Scope::install(c)?.member_sql("a"))?;
    let rows: bool = c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM account_transactions a WHERE a.user_id = ?1 AND {} AND {counted})", scope.rows("a")), [user_id], |r| r.get(0))?;
    if balances.is_empty() && !rows { return Ok(None); }
    let pending = figures::pending(c, user_id, scope, &ledger.labels)?;
    let f = figures::compute_on(c, user_id, scope, ledger, &balances, &pending, fx, today)?;
    let (held_value, held_cost) = f.without_cash()?;
    // A holding valued without a market price (other cash at par) marks the day partial too (PLAN-venue-prices S1):
    // the chart says so for as long as this point is stored.
    let unpriced = !f.all_priced();
    Ok(Some(Day { value: f.value, invested: f.invested, held_value: Some(held_value), held_cost: Some(held_cost),
                  partial: crate::sync::cache::stale(c, user_id, scope).map_err(|_| FiguresError::Data("balance cache provenance unavailable".into()))? || partial(c, user_id, &balances)? || ledger.incomplete || unpriced }))
}

/// `venues`, by account (#710 slice 5): the accounts the user has rows or balances on, by id; a number 1 with rows and no
/// row of its own yet as its stand-in (`AccountRow::unplaced`), after them.
pub fn venues(c: &Connection, user_id: i64) -> Result<Vec<AccountRow>, FiguresError> {
    let any = crate::venue_ids::Scope::install(c)?;
    let mut s = c.prepare(&format!("SELECT DISTINCT COALESCE({}, -t.exchange_id) AS a FROM account_transactions t WHERE t.user_id = ?1 \
                                    UNION SELECT account_id FROM exchange_account_balances WHERE user_id = ?1 AND free + locked > 0 ORDER BY a", any.row_account("t")))?;
    let ids: Vec<i64> = s.query_map([user_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    // A duplicate account (#710, PLAN-710-6 §5.2) is its survivor's: it has no venue snapshot of its own.
    let duplicates = crate::identity::duplicate_ids(c, user_id)?;
    let ids: Vec<i64> = ids.into_iter().filter(|id| !duplicates.contains(id)).collect();
    let mut out = vec![];
    for id in ids {
        crate::figures::budget::charge(1, 0)?;
        if id > 0 { out.extend(AccountRow::by_id(c, id)?); } else { out.push(AccountRow::unplaced(user_id, -id)); }
    }
    out.sort_by_key(|a| (!a.placed(), a.id.abs()));
    Ok(out)
}

/// The account a stored snapshot row is written on: `account` (an id; minus an exchange id for a number 1 with no row yet,
/// whose row is made here, in the write's own transaction, as every write makes it).
pub fn placed(c: &Connection, user_id: i64, account: i64, now: chrono::DateTime<chrono::Utc>) -> Result<AccountRow, FiguresError> {
    let found = if account > 0 { AccountRow::by_id(c, account)? }
                else { AccountRow::of(c, &crate::identity::Account::first(c, user_id, -account)?.ensure(c, now)?)? };
    found.filter(AccountRow::placed).ok_or_else(|| FiguresError::Data("a snapshot of no account".into()))
}

fn values(day: &Day) -> Result<[Option<String>; 4], FiguresError> {
    Ok([Some(column(&day.value)?), Some(column(&day.invested)?), day.held_value.as_ref().map(column).transpose()?, day.held_cost.as_ref().map(column).transpose()?])
}

/// `PortfolioSnapshot.upsert(row, unique_by: %i[user_id date], record_timestamps: true)`, as 2.x generates it.
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

/// `PortfolioVenueSnapshot.upsert_all(rows, unique_by: %i[user_id exchange_id date], record_timestamps: true)`, by account
/// (#710 slice 5: `exchange_account_snapshots`, unique by account and date).
pub fn upsert_account(c: &crate::engine::model::FencedTransaction<'_>, account: &AccountRow, date: NaiveDate, day: &Day) -> Result<(), FiguresError> {
    let [value, invested, held_value, held_cost] = values(day)?;
    c.prepare_cached(
        "INSERT INTO exchange_account_snapshots (user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at, account_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW'), ?9) \
         ON CONFLICT (account_id, date) DO UPDATE SET updated_at = (CASE WHEN (value_usd IS excluded.value_usd AND invested_usd IS excluded.invested_usd \
         AND held_value_usd IS excluded.held_value_usd AND held_cost_usd IS excluded.held_cost_usd AND partial IS excluded.partial) \
         THEN exchange_account_snapshots.updated_at ELSE STRFTIME('%Y-%m-%d %H:%M:%f', 'NOW') END), value_usd = excluded.value_usd, invested_usd = excluded.invested_usd, \
         held_value_usd = excluded.held_value_usd, held_cost_usd = excluded.held_cost_usd, partial = excluded.partial")?
        .execute(rusqlite::params![account.user_id, account.exchange_id, date.to_string(), value, invested, held_value, held_cost, day.partial, account.id])?;
    record_origin(c,account.user_id,Some(account),date)?;
    Ok(())
}

/// `PortfolioSnapshot.record!(user, scopes:)`'s rows, from the scopes just walked: the whole account's (no venue) and
/// each venue's, not yet dated. A venue the walk has no rows of reads as an empty ledger, as 2.x reads one. Computed
/// before the write unit, so the write lock is held for the writes alone.
/// Each of them is reconciled with its own balances: the whole account's row is not the sum of the venues'.
pub fn today_rows(c: &Connection, user_id: i64, walked: &Walked, fx: &super::fx::FxTable, today: NaiveDate) -> Result<Vec<(Option<AccountRow>, Day)>, FiguresError> {
    let mut rows = vec![];
    if let Some(day) = today_row_on(c, user_id, &Scope::All, &walked.whole, fx, today)? { rows.push((None, day)); }
    let empty = Summary::empty();
    for account in venues(c, user_id)? {
        let ledger = walked.venue(account.id).unwrap_or(&empty);
        if let Some(day) = today_row_on(c, user_id, &Scope::Account(account), ledger, fx, today)? { rows.push((Some(account), day)); }
    }
    Ok(rows)
}

/// `record!`'s upserts: today's rows, dated `today`.
pub fn write(c: &crate::engine::model::FencedTransaction<'_>, user_id: i64, rows: &[(Option<AccountRow>, Day)], today: NaiveDate) -> Result<(), FiguresError> {
    for (scope, day) in rows {
        crate::figures::budget::charge(1, 0)?;
        match scope { None => upsert_whole(c, user_id, today, day)?, Some(account) => upsert_account(c, &placed(c, user_id, account.id, chrono::Utc::now())?, today, day)? }
    }
    Ok(())
}

/// `<user>:<exchange>:<date>` for a number 1 (2.x's form, kept), `<user>:<exchange>:<account>:<date>` for an account 2+.
fn record_origin(c:&crate::engine::model::FencedTransaction<'_>,user:i64,scope:Option<&AccountRow>,date:NaiveDate)->Result<(),FiguresError>{
    let scope=match scope{None=>"all".into(),Some(a) if a.number==1=>a.exchange_id.to_string(),Some(a)=>format!("{}:{}",a.exchange_id,a.id)};
    let key=crate::jobs::state::key("snapshot_origin",Some(&format!("{user}:{scope}:{date}")));
    crate::app_config::set_plain(c,&key,&c.producer_stamp().to_string(),chrono::Utc::now()).map_err(FiguresError::Data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figures::at::At;

    /// decimal(20, 8) as ActiveRecord writes it: `BigDecimal#round(8).to_s('F')` (measured in 2.x: 1.12345679, 2.0,
    /// -1.0, 0.00000001).
    #[test]
    fn a_value_is_written_as_v2_writes_a_decimal_column() {
        let written: Vec<String> = ["1.123456785", "2", "-1.000000004", "0.000000005"].iter().map(|v| column(&Dec::strict(v).unwrap()).unwrap()).collect();
        assert_eq!(written, ["1.12345679", "2.0", "-1.0", "0.00000001"]);
    }

    fn balance(usd_value: Option<&str>, priced_at: &str) -> Balance {
        Balance { asset: 1, account: -1, category: Some("Stock".into()), symbol: "AAPL".into(), exchange: "Alpaca".into(), free: Dec::one(), locked: Dec::zero(), usd_price: None, usd_value: usd_value.map(|v| Dec::strict(v).unwrap()),
                  priced_at: Some(At::from_sql(priced_at).unwrap()), synced_at: Some(At::from_sql("2026-10-01 02:30:00").unwrap()) }
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
