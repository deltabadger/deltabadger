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

/// Symbols with no price, by exchange id.
pub type ByVenue = BTreeMap<i64, BTreeSet<String>>;
const VERSION: i64 = 1;

pub fn key(owner: i64) -> String { format!("rust_snapshot_unpriced.{owner}") }

/// Records what a sweep could not price; an empty map is a sweep that priced everything.
pub fn store(c: &Connection, owner: i64, unpriced: &ByVenue, now: DateTime<Utc>) -> Result<(), FiguresError> {
    let venues: serde_json::Map<String, Value> = unpriced.iter().map(|(id, symbols)| (id.to_string(), json!(symbols))).collect();
    crate::app_config::set_plain(c, &key(owner), &json!({ "v": VERSION, "venues": venues }).to_string(), now).map_err(FiguresError::Data)
}

/// The last sweep's record; none when there is none yet (a history swept before it existed) or it cannot be read.
pub fn swept(c: &Connection, owner: i64) -> Result<ByVenue, FiguresError> {
    let Some(raw) = crate::app_config::get_plain(c, &key(owner)).map_err(FiguresError::Data)? else { return Ok(ByVenue::new()) };
    let parsed = serde_json::from_str::<Value>(&raw).ok().filter(|v| v["v"] == VERSION).and_then(|v| {
        v["venues"].as_object().map(|venues| venues.iter().filter_map(|(id, symbols)| {
            Some((id.parse::<i64>().ok()?, symbols.as_array()?.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()))
        }).collect::<ByVenue>())
    });
    Ok(parsed.unwrap_or_else(|| { crate::engine::log(&format!("[tracker] unpriced record for user {owner} not read")); ByVenue::new() }))
}

/// One venue's gaps; `exchange_id` None: holdings no venue has reported yet.
#[derive(Clone, Debug, PartialEq)]
pub struct VenueGap { pub exchange_id: Option<i64>, pub venue: Option<String>, pub now: Vec<String>, pub history: Vec<String> }

/// Every venue of the scope with a holding priced at nothing now, or an asset the stored history could not price,
/// by venue name; `arrived` (symbols held since the sync, with no balance row and no price) as a venue-less entry.
pub fn read(c: &Connection, owner: i64, scope: Option<i64>, arrived: &[String]) -> Result<Vec<VenueGap>, FiguresError> {
    let mut now = ByVenue::new();
    let mut s = c.prepare("SELECT b.exchange_id, a.symbol FROM account_balances b JOIN assets a ON a.id = b.asset_id \
                           WHERE b.user_id = ?1 AND (?2 IS NULL OR b.exchange_id = ?2) AND b.free + b.locked > 0 AND (b.usd_value IS NULL OR b.usd_value <= 0)")?;
    let mut q = s.query(rusqlite::params![owner, scope])?;
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        if let Some(symbol) = r.get::<_, Option<String>>(1)? { now.entry(r.get(0)?).or_default().insert(symbol); }
    }
    let history: ByVenue = swept(c, owner)?.into_iter().filter(|(id, _)| scope.is_none_or(|s| s == *id)).collect();
    let mut gaps = vec![];
    for id in now.keys().chain(history.keys()).copied().collect::<BTreeSet<i64>>() {
        let venue: Option<String> = rusqlite::OptionalExtension::optional(c.query_row("SELECT name FROM exchanges WHERE id = ?1", [id], |r| r.get(0)))?;
        let list = |m: &ByVenue| m.get(&id).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        gaps.push(VenueGap { exchange_id: Some(id), venue, now: list(&now), history: list(&history) });
    }
    let mut arrived: Vec<String> = arrived.to_vec();
    arrived.sort();
    arrived.dedup();
    if !arrived.is_empty() { gaps.push(VenueGap { exchange_id: None, venue: None, now: arrived, history: vec![] }); }
    Ok(gaps)
}

/// Whether any day the chart draws for the scope is stored as partial.
pub fn chart_incomplete(c: &Connection, owner: i64, scope: Option<i64>) -> Result<bool, FiguresError> {
    Ok(match scope {
        None => c.query_row("SELECT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id = ?1 AND partial)", [owner], |r| r.get(0))?,
        Some(id) => c.query_row("SELECT EXISTS(SELECT 1 FROM portfolio_venue_snapshots WHERE user_id = ?1 AND exchange_id = ?2 AND partial)", rusqlite::params![owner, id], |r| r.get(0))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE app_configs (id INTEGER PRIMARY KEY, key TEXT UNIQUE, value TEXT, created_at TEXT, updated_at TEXT);
            CREATE TABLE exchanges (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE assets (id INTEGER PRIMARY KEY, symbol TEXT);
            CREATE TABLE account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, asset_id INTEGER, free REAL, locked REAL, usd_value REAL);
            CREATE TABLE portfolio_snapshots (user_id INTEGER, date TEXT, partial BOOLEAN);
            CREATE TABLE portfolio_venue_snapshots (user_id INTEGER, exchange_id INTEGER, date TEXT, partial BOOLEAN);
            INSERT INTO exchanges VALUES (1, 'Alpaca'), (2, 'Kraken');
            INSERT INTO assets VALUES (1, 'BTC'), (2, 'AAPL'), (3, 'ETH');
            INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_value) VALUES
              (1, 1, 1, 0.1, 0, NULL), (1, 1, 2, 2, 0, 400), (1, 2, 1, 0.2, 0, 12000), (1, 2, 3, 1, 0, 0), (1, 2, 2, 0, 0, NULL), (2, 1, 1, 1, 0, NULL);").unwrap();
        c
    }

    #[test]
    fn the_balances_with_no_value_are_named_by_venue_and_scoped() {
        let c = install();
        let gaps = read(&c, 1, None, &[]).unwrap();
        assert_eq!(gaps, vec![
            VenueGap { exchange_id: Some(1), venue: Some("Alpaca".into()), now: vec!["BTC".into()], history: vec![] },
            VenueGap { exchange_id: Some(2), venue: Some("Kraken".into()), now: vec!["ETH".into()], history: vec![] },
        ], "a zero value is no price; an empty balance is not held");
        assert_eq!(read(&c, 1, Some(2), &[]).unwrap().iter().map(|g| g.exchange_id).collect::<Vec<_>>(), vec![Some(2)]);
        let arrived = read(&c, 1, Some(1), &["SOL".into(), "SOL".into()]).unwrap();
        assert_eq!(arrived.last().unwrap(), &VenueGap { exchange_id: None, venue: None, now: vec!["SOL".into()], history: vec![] });
    }

    #[test]
    fn the_sweep_s_record_is_read_back_and_an_unreadable_one_is_none() {
        let c = install();
        let now = chrono::Utc::now();
        assert!(swept(&c, 1).unwrap().is_empty(), "never swept");
        store(&c, 1, &ByVenue::from([(2, BTreeSet::from(["DOT".to_string()]))]), now).unwrap();
        let gaps = read(&c, 1, Some(2), &[]).unwrap();
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
        assert!(!chart_incomplete(&c, 1, None).unwrap());
        c.execute_batch("INSERT INTO portfolio_snapshots VALUES (1, '2026-11-01', 0); INSERT INTO portfolio_venue_snapshots VALUES (1, 2, '2026-11-01', 1);").unwrap();
        assert!(!chart_incomplete(&c, 1, None).unwrap());
        assert!(chart_incomplete(&c, 1, Some(2)).unwrap());
        assert!(!chart_incomplete(&c, 1, Some(1)).unwrap());
    }
}
