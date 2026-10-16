//! Import::Run#import!, and the controller's zone step (Tracker::ImportsController#resolve_offset): a file read in its
//! format, each row routed to the account it belongs to, and everything written by the sync's own writer
//! (`sync::ledger::store_file`) in ONE transaction, as Rails writes it: a file lands whole or not at all, and an API
//! sync of the same venue, which takes the same write lock, waits for it and then reads the whole file as stored.
use super::{binance, csv, deltabadger};
use crate::engine::model;
use crate::sync::activities::Entry;
use crate::sync::{ledger, Key, SyncError};
use crate::venue::binance::{BINANCE, BINANCE_US};
use chrono::{DateTime, Utc};
use rusqlite::Connection;

/// Import::Run::FORMATS, in their order, and Import::Run::DEFAULT_FORMAT.
pub const FORMATS: [&str; 2] = ["deltabadger", "binance"];
pub const DEFAULT_FORMAT: &str = "deltabadger";

/// A key ApiKey.reading answers: the one a venue of the user's is read with.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading { pub id: i64, pub exchange_id: i64, pub exchange_type: String, pub exchange_name: String, pub key_type: i64 }

/// What Import::Run#import! answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// `error:` — "unreadable", "malformed" or "no_account_for_format"; nothing written.
    Error(&'static str),
    /// `ambiguous: true` and the accounts the file could be from; nothing written.
    Ambiguous(Vec<Reading>),
    /// The counts of the one write, with the readers' unrecognised operations and the venues no account takes.
    Written { imported: usize, duplicates: usize, skipped: usize, unrecognised: Vec<String>, unrouted: Vec<String> },
}

/// Tracker::ImportsController#resolve_offset for a fresh upload: `None` for a format that needs no zone, else the
/// file's own zone (Import::BinanceCsv.offset_from), or `Some(None)` when it names none (`unknown_timezone`).
pub fn offset(format: &str, filename: Option<&str>, bytes: &[u8]) -> Option<Option<String>> {
    (format == "binance").then(|| csv::offset_from(filename, Some(bytes)))
}

/// Exchange#permission_error?'s `known_errors[:permission_denied]`, by venue: a key whose last sync failed on one of
/// these is `missing_permission?`.
const PERMISSION_DENIED: [(&str, &[&str]); 8] = [
    ("Exchanges::Bingx", &["API key was created without the permission", "IP does not match IP whitelist"]),
    ("Exchanges::Mexc", &["No permission to access the endpoint", "IP non white list"]),
    ("Exchanges::Bitget", &["Incorrect permissions", "The user does not have this permission"]),
    ("Exchanges::Bitvavo", &["\"errorCode\":307", "\"errorCode\":310", "\"errorCode\":311", "\"errorCode\":312"]),
    ("Exchanges::Gemini", &["MissingRole"]),
    ("Exchanges::Bitrue", &["You don't have permission", "has no operation privilege"]),
    ("Exchanges::Bybit", &["Permission denied, please check your API key permissions"]),
    ("Exchanges::Kraken", &["EGeneral:Permission denied"]),
];

fn missing_permission(exchange_type: &str, error: Option<&str>) -> bool {
    let Some(error) = error.filter(|e| !crate::ruby::blank(e)) else { return false };
    PERMISSION_DENIED.iter().find(|(t, _)| *t == exchange_type).is_some_and(|(_, words)| words.iter().any(|w| error.contains(w)))
}

/// ApiKey.reading over the user's keys: status correct, never a withdrawal key; per venue the trading key that lacks no
/// permission, else the read-only one, else the first.
pub fn reading(c: &Connection, user_id: i64) -> Result<Vec<Reading>, SyncError> {
    let mut s = c.prepare("SELECT k.id, k.exchange_id, k.key_type, k.last_sync_error, e.type, e.name FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
                           WHERE k.user_id = ?1 AND k.status = 1 AND k.key_type != 1 ORDER BY k.id")?;
    let keys = s.query_map([user_id], |r| Ok((Reading { id: r.get(0)?, exchange_id: r.get(1)?, key_type: r.get(2)?, exchange_type: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                                                         exchange_name: r.get::<_, Option<String>>(5)?.unwrap_or_default() }, r.get::<_, Option<String>>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut venues: Vec<i64> = vec![];
    for (k, _) in &keys { if !venues.contains(&k.exchange_id) { venues.push(k.exchange_id); } }
    Ok(venues.into_iter().filter_map(|venue| {
        let group: Vec<&(Reading, Option<String>)> = keys.iter().filter(|(k, _)| k.exchange_id == venue).collect();
        group.iter().find(|(k, error)| k.key_type == 0 && !missing_permission(&k.exchange_type, error.as_deref()))
            .or_else(|| group.iter().find(|(k, _)| k.key_type == 2)).or_else(|| group.first()).map(|(k, _)| k.clone())
    }).collect())
}

/// Import::Run#import!: `bytes` read in `format` at `offset` and written for `user_id`. `chosen` is the `api_key_id`
/// the Binance/Binance.US question was answered with: taken only when it is one of the accounts offered. Routing and
/// writing run under one write lock (`BEGIN IMMEDIATE`).
pub fn import(c: &Connection, user_id: i64, format: &str, bytes: &[u8], offset: Option<&str>, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    let text = csv::as_text(bytes);
    // Each entry, and the venue it names (None: the format's own venue).
    let (entries, unrecognised): (Vec<(Option<String>, Entry)>, Vec<String>) = match format {
        "binance" => match binance::read(&text, offset) {
            Ok(read) => (read.entries.into_iter().map(|e| (None, e)).collect(), read.unrecognised),
            Err(_) => return Ok(Answer::Error("malformed")),
        },
        "deltabadger" => match deltabadger::read(&text) {
            Ok((rows, unrecognised)) => (rows.into_iter().map(|r| (r.exchange, r.entry)).collect(), unrecognised),
            Err(_) => return Ok(Answer::Error("malformed")),
        },
        _ => return Ok(Answer::Error("unreadable")),
    };
    if entries.is_empty() && unrecognised.is_empty() { return Ok(Answer::Error("unreadable")); }
    let tx = model::immediate(c)?;
    let keys = reading(&tx, user_id)?;
    // #route: [account, its entries] in the order the rows first name them, and the venues nothing here takes.
    let mut routed: Vec<(Reading, Vec<Entry>)> = vec![];
    let mut unrouted: Vec<String> = vec![];
    if format == "binance" {
        let options: Vec<Reading> = keys.into_iter().filter(|k| [BINANCE, BINANCE_US].contains(&k.exchange_type.as_str())).collect();
        let picked = match chosen.and_then(|id| options.iter().find(|k| k.id.to_string() == id)) {
            Some(key) => key.clone(),
            None if options.is_empty() => return Ok(Answer::Error("no_account_for_format")),
            None if options.len() > 1 => return Ok(Answer::Ambiguous(options)),
            None => options[0].clone(),
        };
        routed.push((picked, entries.into_iter().map(|(_, e)| e).collect()));
    } else {
        // `index_by(name_id)`: of two venues of one kind, the later key.
        let by_name = |name: &str| keys.iter().rev().find(|k| crate::tracker::rows::name_id(&k.exchange_type) == name);
        for (venue, entry) in entries {
            match venue.as_deref().and_then(by_name) {
                Some(key) => match routed.iter_mut().find(|(k, _)| k.id == key.id) { Some((_, rows)) => rows.push(entry), None => routed.push((key.clone(), vec![entry])) },
                None => if let Some(venue) = venue { if !unrouted.contains(&venue) { unrouted.push(venue); } },
            }
        }
        unrouted.sort();
    }
    // #write: one transaction across every account the file touches. Fenced on no credential: an import reads none
    // (the venue's API is never called), so no credential change can make what it writes stale.
    let fenced = model::fence_versions(&tx, &[])?;
    let (mut imported, mut duplicates, mut skipped) = (0, 0, 0);
    for (reading, rows) in routed {
        let key = Key { id: reading.id, user_id, exchange_id: reading.exchange_id, status: 1, key_type: reading.key_type, last_synced_at: None,
                        last_sync_error: None, exchange_type: reading.exchange_type };
        let out = ledger::store_file(&fenced, &key, rows, now)?;
        (imported, duplicates, skipped) = (imported + out.imported, duplicates + out.duplicates, skipped + out.skipped);
    }
    drop(fenced);
    tx.commit()?;
    Ok(Answer::Written { imported, duplicates, skipped, unrecognised, unrouted })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_permission_error_is_read_per_venue() {
        assert!(super::missing_permission("Exchanges::Kraken", Some("EGeneral:Permission denied")));
        assert!(!super::missing_permission("Exchanges::Binance", Some("EGeneral:Permission denied")));
        assert!(!super::missing_permission("Exchanges::Kraken", Some(" ")) && !super::missing_permission("Exchanges::Kraken", None));
    }
}
