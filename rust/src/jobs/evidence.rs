//! Delisted coins (plan r10 §1.5, PR0): `listing_evidence`, every ticker binding an import observed, as intervals.
//!
//! Each import run of a venue's catalogue in a scope (data-api's tickers, Alpaca's stocks or coins, IBKR's listings,
//! CoinGecko's venue catalogue) is one `listing_observations` row, taken before its first unit. A binding the run sees
//! extends its interval only when the interval was last extended by the scope's previous recorded run, and that run
//! (a) completed with every unit's evidence written, (b) was observed no later than this one, and (c) no more than
//! MAX_OBSERVATION_GAP before it. Everything else starts a new interval: a run that failed, wrote no evidence, saw an
//! empty catalogue, came too long after the last, or a clock gone backward (such a run records nothing at all). No barrier
//! or other extra write is needed for this to hold: a run that records nothing (a missed run, one that stopped before
//! publishing, a handback period, a catalogue snapshot, which observes no venue) only widens the gap. Missing observations
//! cost coverage, never correctness: an interval is never extended across more than MAX_OBSERVATION_GAP unobserved.
//!
//! A unit's evidence is written in a transaction of its own right after the unit's commit, on the same blocking call, so
//! no evidence failure can undo or change a ticker write and a dropped caller cannot skip it. A failure is logged, marks
//! the run failed (then it never completes), and the later units' evidence is still written.
//!
//! Accepted (Codex PR0 r2 P1-1): evidence takes database pages, so on a database at its page limit a later legacy unit
//! can fail where it would have fit without them. No install sets `max_page_count`, and a full disk fails every write of
//! every job alike; the table grows only when a binding changes (an unchanged one is an update in place), so it cannot be
//! what fills a disk. No space reservation or retry is built for it.
use super::import::{in_transaction, Touched};
use super::Db;
use crate::crypto::Cipher;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The longest a binding is taken to hold unobserved between two runs. The catalogue imports run every 4 hours (venue
/// tickers) or daily (Alpaca, IBKR), each with retries: three days tolerate a day or two of missed runs (an install that
/// was off over a weekend) before an interval is cut. The residual is a venue renaming a spelling and reverting it inside
/// that window unobserved, which venues do not do.
pub const MAX_OBSERVATION_GAP: Duration = Duration::days(3);

/// (side, spelling, asset id, ticker id): one binding a unit wrote.
pub type Binding = (&'static str, String, i64, i64);

/// The latest interval of one binding.
pub const LATEST: &str = "SELECT id, observation_id FROM listing_evidence WHERE exchange_id = ?1 AND side = ?2 AND spelling = ?3 AND scope = ?4 \
                          AND asset_id = ?5 AND ticker_id = ?6 ORDER BY last_seen DESC, id DESC LIMIT 1";

/// One import run's observation.
#[derive(Clone, Debug)]
pub struct Observation { id: i64, exchange_id: i64, scope: String, at: String, continues: Option<i64>, failed: Arc<AtomicBool> }

fn log(what: &str, exchange_id: i64, e: impl std::fmt::Display) { crate::engine::log(&format!("[reference] listing evidence {what} for exchange {exchange_id}: {e}")); }

fn parse(at: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M:%S%.f").ok().map(|t| t.and_utc())
}

/// Opens a run of (venue, scope) observed at `at`. None when it records nothing: its clock is not after the scope's
/// last run (a regressed clock, or a feed whose evidence is older), or it cannot be written (logged).
pub async fn begin(db: &Db, exchange_id: i64, scope: &str, at: DateTime<Utc>) -> Option<Observation> {
    let s = scope.to_string();
    let opened = db.run(move |c, cipher| in_transaction(c, cipher, "listing evidence", |c| open(c, exchange_id, &s, at).map(|o| (o, Touched::None)).map_err(|e| e.to_string()))).await;
    opened.map_err(|e| log("not begun", exchange_id, e)).ok().flatten()
}

fn open(c: &Connection, exchange_id: i64, scope: &str, at: DateTime<Utc>) -> rusqlite::Result<Option<Observation>> {
    let last: Option<(i64, String, i64)> = c.query_row("SELECT id, at, complete FROM listing_observations WHERE exchange_id = ?1 AND scope = ?2 ORDER BY id DESC LIMIT 1",
                                                       params![exchange_id, scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    let before = last.as_ref().and_then(|(_, t, _)| parse(t));
    if before.is_some_and(|t| t >= at) || (last.is_some() && before.is_none()) { return Ok(None); }
    let continues = last.zip(before).filter(|((_, _, complete), t)| *complete == 1 && at - *t <= MAX_OBSERVATION_GAP).map(|((id, ..), _)| id);
    let at = crate::codec::format_time(at);
    c.execute("INSERT INTO listing_observations (exchange_id, scope, at) VALUES (?1, ?2, ?3)", params![exchange_id, scope, at])?;
    Ok(Some(Observation { id: c.last_insert_rowid(), exchange_id, scope: scope.into(), at, continues, failed: Arc::default() }))
}

/// The bindings of a unit that just committed, in a write unit of their own, on the caller's connection (the unit's own
/// blocking call). A failure is logged and marks the run failed.
pub fn record(c: &Connection, cipher: &Cipher, o: &Observation, bindings: &[Binding]) {
    if bindings.is_empty() { return; }
    let written = in_transaction(c, cipher, "listing evidence", |c| write(c, o, bindings).map(|()| ((), Touched::None)).map_err(|e| e.to_string()));
    if let Err(e) = written { fail(c, o, e); }
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
fn fail(c: &Connection, o: &Observation, why: String) {
    log("not written", o.exchange_id, &why);
    o.failed.store(true, Ordering::SeqCst);
    // ponytail: unmarked (the mark failing too), the run stays incomplete (0), which no later run extends from either.
    if let Err(e) = c.execute("UPDATE listing_observations SET complete = -1 WHERE id = ?1", [o.id]) { log("failure not marked", o.exchange_id, e); }
}

/// The run published every unit and wrote every unit's evidence: the next run may extend from it.
pub async fn complete(db: &Db, o: &Observation) {
    if o.failed.load(Ordering::SeqCst) { return; }
    let id = o.id;
    let done = db.run(move |c, _| c.execute("UPDATE listing_observations SET complete = 1 WHERE id = ?1 AND complete = 0", [id]).map_err(|e| e.to_string())).await;
    if let Err(e) = done { log("not completed", o.exchange_id, e); }
}

/// A complete run of `bindings` alone, writing no ticker (IBKR's evidence-only revision, data-api's empty catalogue).
pub async fn observe(db: &Db, exchange_id: i64, scope: &str, at: DateTime<Utc>, bindings: Vec<Binding>) {
    let Some(o) = begin(db, exchange_id, scope, at).await else { return };
    let mine = o.clone();
    if db.run(move |c, cipher| { record(c, cipher, &mine, &bindings); Ok(()) }).await.is_ok() { complete(db, &o).await; }
}
