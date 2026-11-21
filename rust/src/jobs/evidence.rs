//! Delisted coins (plan r10 §1.5, PR0): `listing_evidence`, every ticker binding an import observed, as intervals.
//!
//! Each import run of a venue's catalogue (data-api's tickers, Alpaca's stocks or coins, IBKR's listings, CoinGecko's
//! venue catalogue) is one `listing_observations` row of its (venue, scope), taken before its first unit. A binding seen
//! by the run extends its interval only when the interval was last extended by the run's predecessor, and only when that
//! predecessor completed with every unit's evidence written and no barrier came after it. So nothing is bridged across a
//! run that did not observe it: one that failed, wrote no evidence, saw an empty catalogue, or a barrier (a takeover, a
//! handback, a catalogue snapshot's publish, which observes no venue).
//!
//! The evidence of a unit is written in a transaction of its own after the unit's commit: no evidence failure, an
//! SQLITE_FULL that rolls the transaction back included, can undo or change the ticker write. Every failure is logged and
//! leaves its run incomplete (in the table, and in memory when the table cannot be written either).
use super::import::{in_transaction, Touched};
use super::Db;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// (side, spelling, asset id, ticker id): one binding a unit wrote.
pub type Binding = (&'static str, String, i64, i64);

/// The latest interval of one binding.
pub const LATEST: &str = "SELECT id, observation_id FROM listing_evidence WHERE exchange_id = ?1 AND side = ?2 AND spelling = ?3 AND scope = ?4 \
                          AND asset_id = ?5 AND ticker_id = ?6 ORDER BY last_seen DESC, id DESC LIMIT 1";

/// One import run's observation.
#[derive(Clone, Debug)]
pub struct Observation { id: i64, exchange_id: i64, scope: String, at: String, continues: Option<i64>, failed: Arc<AtomicBool> }

fn log(what: &str, exchange_id: i64, e: impl std::fmt::Display) { crate::engine::log(&format!("[reference] listing evidence {what} for exchange {exchange_id}: {e}")); }

/// Opens a run of (venue, scope) at `at`. None when it cannot be recorded (logged): the run records nothing, and a
/// barrier is attempted so the next run does not extend across it.
pub async fn begin(db: &Db, exchange_id: i64, scope: &str, at: &str) -> Option<Observation> {
    let (s, a) = (scope.to_string(), at.to_string());
    let opened = db.run(move |c, cipher| in_transaction(c, cipher, "listing evidence", |c| open(c, exchange_id, &s, &a).map(|o| (o, Touched::None)).map_err(|e| e.to_string()))).await;
    match opened {
        Ok(o) => Some(o),
        Err(e) => { log("not begun", exchange_id, e); barrier_for(db, exchange_id, scope, at).await; None }
    }
}

fn open(c: &Connection, exchange_id: i64, scope: &str, at: &str) -> rusqlite::Result<Observation> {
    // The latest row of this (venue, scope), or any barrier after it: only a completed import there can be extended.
    let last: Option<(i64, String, i64)> = c.query_row("SELECT id, kind, complete FROM listing_observations WHERE (exchange_id = ?1 AND scope = ?2) OR exchange_id IS NULL \
                                                       ORDER BY id DESC LIMIT 1", params![exchange_id, scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    let continues = last.filter(|(_, kind, complete)| kind == "import" && *complete == 1).map(|(id, ..)| id);
    c.execute("INSERT INTO listing_observations (exchange_id, scope, kind, at) VALUES (?1, ?2, 'import', ?3)", params![exchange_id, scope, at])?;
    Ok(Observation { id: c.last_insert_rowid(), exchange_id, scope: scope.into(), at: at.into(), continues, failed: Arc::default() })
}

/// The bindings of a unit that committed, in a write unit of their own.
pub async fn record(db: &Db, o: &Observation, bindings: Vec<Binding>) {
    if bindings.is_empty() || o.failed.load(Ordering::SeqCst) { return; }
    let mine = o.clone();
    let written = db.run(move |c, cipher| in_transaction(c, cipher, "listing evidence", |c| write(c, &mine, &bindings).map(|()| ((), Touched::None)).map_err(|e| e.to_string()))).await;
    if let Err(e) = written { fail(db, o, e).await; }
}

fn write(c: &Connection, o: &Observation, bindings: &[Binding]) -> rusqlite::Result<()> {
    for (side, spelling, asset, ticker) in bindings {
        let latest: Option<(i64, i64)> = c.prepare_cached(LATEST)?.query_row(params![o.exchange_id, side, spelling, o.scope, asset, ticker], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        match latest {
            Some((_, by)) if by == o.id => {}
            Some((id, by)) if Some(by) == o.continues => {
                c.prepare_cached("UPDATE listing_evidence SET last_seen = ?1, observation_id = ?2 WHERE id = ?3")?.execute(params![o.at, o.id, id])?;
            }
            _ => {
                c.prepare_cached("INSERT INTO listing_evidence (exchange_id, scope, side, spelling, asset_id, ticker_id, first_seen, last_seen, observation_id) \
                                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)")?.execute(params![o.exchange_id, o.scope, side, spelling, asset, ticker, o.at, o.id])?;
            }
        }
    }
    Ok(())
}

/// A unit's evidence was not written: the run is marked failed (complete = -1) and never completes.
async fn fail(db: &Db, o: &Observation, why: String) {
    log("not written", o.exchange_id, &why);
    o.failed.store(true, Ordering::SeqCst);
    let id = o.id;
    let marked = db.run(move |c, _| c.execute("UPDATE listing_observations SET complete = -1 WHERE id = ?1", [id]).map_err(|e| e.to_string())).await;
    // ponytail: unmarked, the run stays incomplete (0), which no later run extends from either.
    if let Err(e) = marked { log("failure not marked", o.exchange_id, e); }
}

/// The run published every unit and wrote every unit's evidence: the next run may extend from it.
pub async fn complete(db: &Db, o: &Observation) {
    if o.failed.load(Ordering::SeqCst) { return; }
    let id = o.id;
    let done = db.run(move |c, _| c.execute("UPDATE listing_observations SET complete = 1 WHERE id = ?1 AND complete = 0", [id]).map_err(|e| e.to_string())).await;
    if let Err(e) = done { log("not completed", o.exchange_id, e); }
}

/// A run that observed an empty catalogue: begun and completed with no binding, so the next run extends nothing.
pub async fn observe_nothing(db: &Db, exchange_id: i64, scope: &str, at: &str) {
    if let Some(o) = begin(db, exchange_id, scope, at).await { complete(db, &o).await; }
}

/// A barrier: no observation of (venue, scope) (None: of every venue) before it is extended after it.
pub fn barrier(c: &Connection, venue: Option<(i64, &str)>, at: &str) -> rusqlite::Result<()> {
    let present: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'listing_observations')", [], |r| r.get(0))?;
    if !present { return Ok(()); } // an unmigrated copy (a parity scratch) observes nothing
    c.execute("INSERT INTO listing_observations (exchange_id, scope, kind, at) VALUES (?1, ?2, 'barrier', ?3)", params![venue.map(|v| v.0), venue.map(|v| v.1), at])?;
    Ok(())
}

/// `barrier` for one venue's scope, from the job (logged on failure).
pub async fn barrier_for(db: &Db, exchange_id: i64, scope: &str, at: &str) {
    let (scope, at) = (scope.to_string(), at.to_string());
    if let Err(e) = db.run(move |c, _| barrier(c, Some((exchange_id, &scope)), &at).map_err(|e| e.to_string())).await { log("barrier not written", exchange_id, e); }
}
