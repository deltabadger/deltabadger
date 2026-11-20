//! What the tracker holds with no market price (Rust only, PLAN-venue-prices S1): the page names the assets, per venue,
//! rather than withholding its figures in silence.
//! - now: the balances with no value, by venue (the predicate `figures::page::cold` reads as unpriced);
//! - history: what the last sweep could not price on some day, by venue: `rust_snapshot_unpriced.<owner>`, written with
//!   the snapshots it describes (`backfill::store`, and the no-history branch of the backfill job);
//! - whether the chart is incomplete at all: the stored `partial` flags of the days it draws.
use crate::figures::FiguresError;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Symbols with no price, by account id (#710 slice 5; version 1 keyed them by exchange id).
pub type ByVenue = BTreeMap<i64, BTreeSet<String>>;
const VERSION: i64 = 2;

pub fn key(owner: i64) -> String { format!("rust_snapshot_unpriced.{owner}") }

/// Records what a sweep could not price; an empty map is a sweep that priced everything.
pub fn store(c: &Connection, owner: i64, unpriced: &ByVenue, now: DateTime<Utc>) -> Result<(), FiguresError> {
    let venues: serde_json::Map<String, Value> = unpriced.iter().map(|(id, symbols)| (id.to_string(), json!(symbols))).collect();
    crate::app_config::set_plain(c, &key(owner), &json!({ "v": VERSION, "venues": venues }).to_string(), now).map_err(FiguresError::Data)
}

/// The last sweep's record; none when there is none yet (a history swept before it existed) or it cannot be read. A
/// version-1 record (by exchange id) reads as the owner's number 1 of each exchange; an exchange with no account of the
/// owner's is left out, as nothing can show it. The next sweep writes version 2.
pub fn swept(c: &Connection, owner: i64) -> Result<ByVenue, FiguresError> {
    let Some(raw) = crate::app_config::get_plain(c, &key(owner)).map_err(FiguresError::Data)? else { return Ok(ByVenue::new()) };
    let parsed = serde_json::from_str::<Value>(&raw).ok().filter(|v| v["v"] == VERSION || v["v"] == 1).and_then(|v| {
        let by_exchange = v["v"] == 1;
        v["venues"].as_object().map(|venues| venues.iter().filter_map(|(id, symbols)| {
            Some((id.parse::<i64>().ok()?, symbols.as_array()?.iter().filter_map(|s| s.as_str().map(str::to_string)).collect::<BTreeSet<String>>()))
        }).collect::<Vec<_>>()).map(|entries| (by_exchange, entries))
    });
    let Some((by_exchange, entries)) = parsed else { crate::engine::log(&format!("[tracker] unpriced record for user {owner} not read")); return Ok(ByVenue::new()) };
    let mut out = ByVenue::new();
    for (id, symbols) in entries {
        // An install without numbered accounts reads its number 1 as the stand-in (minus the exchange id).
        let account = if !by_exchange { Some(id) } else if crate::identity::numbered(c)? { crate::identity::AccountRow::first(c, owner, id)?.map(|a| a.id) } else { Some(-id) };
        if let Some(a) = account { out.entry(a).or_default().extend(symbols); }
    }
    Ok(out)
}

/// One account's gaps (its exchange's id and name); `exchange_id` None: holdings no account has reported yet.
#[derive(Clone, Debug, PartialEq)]
pub struct VenueGap { pub exchange_id: Option<i64>, pub venue: Option<String>, pub now: Vec<String>, pub history: Vec<String> }

/// Every account of the scope with a holding priced at nothing now, or an asset the stored history could not price,
/// by its exchange's name; `arrived` (symbols held since the sync, with no balance row and no price) as an account-less
/// entry.
pub fn read(c: &Connection, owner: i64, scope: &super::Scope, arrived: &[String]) -> Result<Vec<VenueGap>, FiguresError> {
    let mut now = ByVenue::new();
    let mut s = c.prepare(&format!("SELECT b.account_id, a.symbol FROM exchange_account_balances b JOIN assets a ON a.id = b.asset_id \
                           WHERE b.user_id = ?1 AND {} AND b.free + b.locked > 0 AND (b.usd_value IS NULL OR b.usd_value <= 0)", scope.store("b")))?;
    let mut q = s.query([owner])?;
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        if let Some(symbol) = r.get::<_, Option<String>>(1)? { now.entry(r.get(0)?).or_default().insert(symbol); }
    }
    let wanted = |id: i64| match scope { super::Scope::All => true, super::Scope::Account(a) => a.id == id, super::Scope::EmptyLegacyExchange(_) => false };
    let history: ByVenue = swept(c, owner)?.into_iter().filter(|(id, _)| wanted(*id)).collect();
    let mut gaps = vec![];
    for id in now.keys().chain(history.keys()).copied().collect::<BTreeSet<i64>>() {
        // A stand-in (minus its exchange id) is its exchange's number 1.
        let venue: Option<(i64, String)> = rusqlite::OptionalExtension::optional(if id < 0 {
            c.query_row("SELECT id, name FROM exchanges WHERE id = -?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        } else {
            c.query_row("SELECT e.id, e.name FROM exchange_accounts a JOIN exchanges e ON e.id = a.exchange_id WHERE a.id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        })?;
        let list = |m: &ByVenue| m.get(&id).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        gaps.push(VenueGap { exchange_id: venue.as_ref().map(|v| v.0), venue: venue.map(|v| v.1), now: list(&now), history: list(&history) });
    }
    let mut arrived: Vec<String> = arrived.to_vec();
    arrived.sort();
    arrived.dedup();
    if !arrived.is_empty() { gaps.push(VenueGap { exchange_id: None, venue: None, now: arrived, history: vec![] }); }
    Ok(gaps)
}

/// Whether any day the chart draws for the scope is stored as partial.
pub fn chart_incomplete(c: &Connection, owner: i64, scope: &super::Scope) -> Result<bool, FiguresError> {
    Ok(match scope {
        super::Scope::All => c.query_row("SELECT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id = ?1 AND partial)", [owner], |r| r.get(0))?,
        scope => c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM exchange_account_snapshots s WHERE s.user_id = ?1 AND {} AND s.partial)", scope.store("s")), [owner], |r| r.get(0))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{identity::AccountRow, tracker::Scope};

    /// Accounts 1 (Alpaca) and 2 (Kraken) of user 1, 3 (Alpaca) of user 2.
    fn install() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE app_configs (id INTEGER PRIMARY KEY, key TEXT UNIQUE, value TEXT, created_at TEXT, updated_at TEXT);
            CREATE TABLE exchanges (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE exchange_accounts (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, number INTEGER);
            CREATE TABLE assets (id INTEGER PRIMARY KEY, symbol TEXT);
            CREATE TABLE exchange_account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, asset_id INTEGER, free REAL, locked REAL, usd_value REAL, account_id INTEGER);
            CREATE TABLE portfolio_snapshots (user_id INTEGER, date TEXT, partial BOOLEAN);
            CREATE TABLE exchange_account_snapshots (user_id INTEGER, exchange_id INTEGER, date TEXT, partial BOOLEAN, account_id INTEGER);
            INSERT INTO exchanges VALUES (1, 'Alpaca'), (2, 'Kraken');
            INSERT INTO exchange_accounts VALUES (1, 1, 1, 1), (2, 1, 2, 1), (3, 2, 1, 1);
            INSERT INTO assets VALUES (1, 'BTC'), (2, 'AAPL'), (3, 'ETH');
            INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, usd_value, account_id) VALUES
              (1, 1, 1, 0.1, 0, NULL, 1), (1, 1, 2, 2, 0, 400, 1), (1, 2, 1, 0.2, 0, 12000, 2), (1, 2, 3, 1, 0, 0, 2), (1, 2, 2, 0, 0, NULL, 2), (2, 1, 1, 1, 0, NULL, 3);").unwrap();
        c
    }
    fn account(id: i64) -> Scope { Scope::Account(AccountRow { id, user_id: 1, exchange_id: id, number: 1 }) }

    #[test]
    fn the_balances_with_no_value_are_named_by_venue_and_scoped() {
        let c = install();
        let gaps = read(&c, 1, &Scope::All, &[]).unwrap();
        assert_eq!(gaps, vec![
            VenueGap { exchange_id: Some(1), venue: Some("Alpaca".into()), now: vec!["BTC".into()], history: vec![] },
            VenueGap { exchange_id: Some(2), venue: Some("Kraken".into()), now: vec!["ETH".into()], history: vec![] },
        ], "a zero value is no price; an empty balance is not held");
        assert_eq!(read(&c, 1, &account(2), &[]).unwrap().iter().map(|g| g.exchange_id).collect::<Vec<_>>(), vec![Some(2)]);
        let arrived = read(&c, 1, &account(1), &["SOL".into(), "SOL".into()]).unwrap();
        assert_eq!(arrived.last().unwrap(), &VenueGap { exchange_id: None, venue: None, now: vec!["SOL".into()], history: vec![] });
    }

    #[test]
    fn the_sweep_s_record_is_read_back_and_an_unreadable_one_is_none() {
        let c = install();
        let now = chrono::Utc::now();
        assert!(swept(&c, 1).unwrap().is_empty(), "never swept");
        store(&c, 1, &ByVenue::from([(2, BTreeSet::from(["DOT".to_string()]))]), now).unwrap();
        let gaps = read(&c, 1, &account(2), &[]).unwrap();
        assert_eq!(gaps[0].history, vec!["DOT".to_string()]);
        assert_eq!(gaps[0].now, vec!["ETH".to_string()]);
        store(&c, 1, &ByVenue::new(), now).unwrap();
        assert!(swept(&c, 1).unwrap().is_empty(), "a sweep that priced everything");
        c.execute("UPDATE app_configs SET value = '{\"v\":9}'", []).unwrap();
        assert!(swept(&c, 1).unwrap().is_empty());
    }

    #[test]
    fn the_chart_is_incomplete_when_a_drawn_day_is_partial() {
        let c = install();
        assert!(!chart_incomplete(&c, 1, &Scope::All).unwrap());
        c.execute_batch("INSERT INTO portfolio_snapshots VALUES (1, '2026-11-01', 0); INSERT INTO exchange_account_snapshots VALUES (1, 2, '2026-11-01', 1, 2);").unwrap();
        assert!(!chart_incomplete(&c, 1, &Scope::All).unwrap());
        assert!(chart_incomplete(&c, 1, &account(2)).unwrap());
        assert!(!chart_incomplete(&c, 1, &account(1)).unwrap());
    }
}
