//! Import::Run#import!, and the controller's zone step (Tracker::ImportsController#resolve_offset): a file read in its
//! format, each row routed to the account it belongs to, and everything written by the sync's own writer
//! (`sync::ledger::store_file`) in ONE transaction, as 2.x writes it: a file lands whole or not at all, and an API
//! sync of the same venue, which takes the same write lock, waits for it and then reads the whole file as stored.
use super::{binance, csv, deltabadger, kucoin};
use crate::engine::model;
use crate::sync::activities::Entry;
use crate::sync::{ledger, Key, SyncError};
use crate::venue::binance::{BINANCE, BINANCE_US};
use crate::venue::kucoin::KUCOIN;
use chrono::{DateTime, Utc};
use rusqlite::Connection;

/// Import::Run::FORMATS, in their order, and Import::Run::DEFAULT_FORMAT. `kucoin` is Rust's own (D-IMPORT-FORMATS).
pub const FORMATS: [&str; 3] = ["deltabadger", "binance", "kucoin"];
pub const DEFAULT_FORMAT: &str = "deltabadger";

/// A key ApiKey.reading answers: the one a venue of the user's is read with.
#[derive(Debug, Clone, PartialEq)]
/// `account_id`: its account (crate::identity::account_of_key; None for a number 1 with no row yet).
pub struct Reading { pub id: i64, pub exchange_id: i64, pub account_id: Option<i64>, pub exchange_type: String, pub exchange_name: String, pub key_type: i64 }

/// What Import::Run#import! answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// `error:` — "unreadable", "malformed" or "no_account_for_format"; nothing written.
    Error(&'static str),
    /// `ambiguous: true` and the accounts the file could be from; nothing written.
    Ambiguous(Vec<Reading>),
    /// A named Binance identity refusal, returned only after the entire write has rolled back.
    Refused(String),
    /// The counts of the one write, with the readers' unrecognised operations and the venues no account takes.
    /// `generic`: of the imported entries, those a KuCoin file could only label generically (D-IMPORT-WARNINGS).
    Written { imported: usize, duplicates: usize, skipped: usize, unrecognised: Vec<String>, unrouted: Vec<String>, generic: usize },
}

/// Tracker::ImportsController#resolve_offset for a fresh upload: `None` for a format that needs no zone, else the
/// file's own zone (Import::BinanceCsv.offset_from), or `Some(None)` when it names none (`unknown_timezone`).
pub fn offset(format: &str, filename: Option<&str>, bytes: &[u8]) -> Option<Option<String>> {
    (format == "binance").then(|| csv::offset_from(filename, Some(bytes)))
}

/// Exchange#permission_error?'s `known_errors[:permission_denied]`, by venue: a key whose last sync failed on one of
/// these is `missing_permission?`.
const PERMISSION_DENIED: [(&str, &[&str]); 9] = [
    ("Exchanges::Bingx", &["API key was created without the permission", "IP does not match IP whitelist"]),
    ("Exchanges::Mexc", &["No permission to access the endpoint", "IP non white list"]),
    ("Exchanges::Bitget", &["Incorrect permissions", "The user does not have this permission"]),
    ("Exchanges::Bitvavo", &["\"errorCode\":307", "\"errorCode\":310", "\"errorCode\":311", "\"errorCode\":312"]),
    ("Exchanges::Gemini", &["MissingRole"]),
    ("Exchanges::Bitrue", &["You don't have permission", "has no operation privilege"]),
    ("Exchanges::Bybit", &["Permission denied, please check your API key permissions"]),
    ("Exchanges::Kraken", &["EGeneral:Permission denied"]),
    ("Exchanges::Kucoin", &["Access Denied"]),
];

pub(crate) fn missing_permission(exchange_type: &str, error: Option<&str>) -> bool {
    let Some(error) = error.filter(|e| !crate::v2::blank(e)) else { return false };
    PERMISSION_DENIED.iter().find(|(t, _)| *t == exchange_type).is_some_and(|(_, words)| words.iter().any(|w| error.contains(w)))
}

/// ApiKey.reading over the user's keys: status correct, never a withdrawal key; per venue the trading key that lacks no
/// permission, else the read-only one, else the first.
pub fn reading(c: &Connection, user_id: i64) -> Result<Vec<Reading>, SyncError> {
    let keys = crate::identity::with_key_account("k", |account| c.prepare(&format!("SELECT k.id, k.exchange_id, k.key_type, k.last_sync_error, e.type, e.name, {account} FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
                           WHERE k.user_id = ?1 AND k.status = 1 AND k.key_type != 1 ORDER BY k.id"))?.query_map([user_id], |r| Ok((Reading { id: r.get(0)?, exchange_id: r.get(1)?, account_id: r.get(6)?, key_type: r.get(2)?, exchange_type: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                                                         exchange_name: r.get::<_, Option<String>>(5)?.unwrap_or_default() }, r.get::<_, Option<String>>(3)?)))?
        .collect::<Result<Vec<_>, _>>())?;
    // Per account (#710), in the order its first key was made.
    let mut accounts: Vec<(i64, Option<i64>)> = vec![];
    for (k, _) in &keys { if !accounts.contains(&(k.exchange_id, k.account_id)) { accounts.push((k.exchange_id, k.account_id)); } }
    Ok(accounts.into_iter().filter_map(|account| {
        let group: Vec<&(Reading, Option<String>)> = keys.iter().filter(|(k, _)| (k.exchange_id, k.account_id) == account).collect();
        group.iter().find(|(k, error)| k.key_type == 0 && !missing_permission(&k.exchange_type, error.as_deref()))
            .or_else(|| group.iter().find(|(k, _)| k.key_type == 2)).or_else(|| group.first()).map(|(k, _)| k.clone())
    }).collect())
}

/// A file as read, before any database is touched: each entry with the venue it names (None: the format's own venue),
/// the operations the reader did not recognise, a KuCoin export's rows (stored as rows, `kucoin::store`), and a
/// venue-native export's account id (crate::identity).
/// An entry with the venue and account number (#710) its row names, where the format names one.
type Named = (Option<(String, u32)>, Entry);
pub struct Read { entries: Vec<Named>, unrecognised: Vec<String>, rows: Vec<kucoin::SourceRow>, account_id: Option<crate::identity::AccountId> }

/// What a column holds of an amount: its nearest double, which must be finite (2.x stores 1e309 as Infinity, which
/// no later read takes; this refuses the file instead).
fn storable(e: &Entry) -> bool {
    [Some(&e.base_amount), e.quote_amount.as_ref(), e.fee_amount.as_ref()].into_iter().flatten().all(|d| d.to_f().is_finite())
}

/// Import::Run#outcome up to routing: the file read in `format` at `offset`, or the answer refusing it. Pure: run it
/// before taking a connection.
pub fn read(format: &str, bytes: &[u8], offset: Option<&str>) -> Result<Read, Answer> {
    let text = csv::as_text(bytes);
    let mut account_id = None;
    let mut rows = vec![];
    let (entries, unrecognised): (Vec<Named>, Vec<String>) = match format {
        // D-IMPORT-UID: Binance's own export names its account (`User ID`, or `User_ID`) on every record. It is read
        // over every record before the reader skips any; a missing, blank, masked or mixed id is a malformed export.
        "binance" => match csv::Table::parse(&text).and_then(|table| crate::identity::export_account_id(&table, &crate::identity::BINANCE_EXPORT_COLUMNS))
            .and_then(|id| { account_id = id; binance::read(&text, offset) }) {
            Ok(read) => (read.entries.into_iter().map(|e| (None, e)).collect(), read.unrecognised),
            Err(_) => return Err(Answer::Error("malformed")),
        },
        // KuCoin's export names its account (`UID`) on every record, read as Binance's is.
        "kucoin" => match csv::Table::parse(&text).and_then(|table| {
            account_id = crate::identity::export_account_id(&table, &crate::identity::KUCOIN_EXPORT_COLUMNS)?;
            kucoin::read(&table)
        }) {
            Ok(read) => { rows = read; (vec![], vec![]) }
            Err(_) => return Err(Answer::Error("malformed")),
        },
        "deltabadger" => match deltabadger::read(&text) {
            Ok((rows, unrecognised)) => (rows.into_iter().map(|r| (r.exchange.map(|v| (v, r.account)), r.entry)).collect(), unrecognised),
            Err(_) => return Err(Answer::Error("malformed")),
        },
        _ => return Err(Answer::Error("unreadable")),
    };
    if !entries.iter().all(|(_, e)| storable(e)) { return Err(Answer::Error("malformed")); }
    if entries.is_empty() && unrecognised.is_empty() && rows.is_empty() { return Err(Answer::Error("unreadable")); }
    Ok(Read { entries, unrecognised, rows, account_id })
}

/// Import::Run#import!: `bytes` read in `format` at `offset` and written for `user_id` (`read`, then `write`).
#[allow(clippy::too_many_arguments)]
pub fn import(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, bytes: &[u8], offset: Option<&str>, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    match read(format, bytes, offset) { Ok(file) => write(c, cipher, user_id, format, file, chosen, now), Err(answer) => Ok(answer) }
}

/// Import::Run#route and #write for a file `read` accepted. `chosen` is the `api_key_id` the Binance/Binance.US
/// question was answered with: taken only when it is one of the accounts offered. Routing and writing run under one
/// write lock (`BEGIN IMMEDIATE`).
pub fn write(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    match write_transaction(c, cipher, user_id, format, file, chosen, now) {
        Err(error) => match crate::sync::binance_writer::csv_refusal(&error) {
            Some(reason) => Ok(Answer::Refused(reason.to_owned())),
            None => Err(error),
        },
        answer => answer,
    }
}

// Returning an error drops the transaction before `write` translates a named refusal.
fn write_transaction(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    let Read { entries, unrecognised, rows: kucoin_rows, account_id } = file;
    let tx = model::immediate(c)?;
    let keys = reading(&tx, user_id)?;
    // #route: [account, its entries] in the order the rows first name them, and the venues nothing here takes.
    let mut routed: Vec<(Reading, Vec<Entry>)> = vec![];
    let mut unrouted: Vec<String> = vec![];
    // A venue's own export goes to that venue's account.
    let venues: &[&str] = match format { "binance" => &[BINANCE, BINANCE_US], "kucoin" => &[KUCOIN], _ => &[] };
    if !venues.is_empty() {
        let options: Vec<Reading> = keys.into_iter().filter(|k| venues.contains(&k.exchange_type.as_str())).collect();
        let picked = match chosen.and_then(|id| options.iter().find(|k| k.id.to_string() == id)) {
            Some(key) => key.clone(),
            None if options.is_empty() => return Ok(Answer::Error("no_account_for_format")),
            None if options.len() > 1 => return Ok(Answer::Ambiguous(options)),
            None => options[0].clone(),
        };
        routed.push((picked, entries.into_iter().map(|(_, e)| e).collect()));
    } else {
        // `index_by(name_id)`: of two venues of one kind, the later key; #710: of the account the row's number names (an
        // account this install does not have is unrouted, named as plain text names it: `kraken 2`).
        let numbered: Vec<(&Reading, i64)> = keys.iter().map(|k| Ok((k, match k.account_id {
            Some(id) => tx.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0))?,
            None => 1,
        }))).collect::<Result<_, rusqlite::Error>>()?;
        let by_name = |name: &str, number: u32| numbered.iter().rev().find(|(k, n)| crate::tracker::rows::name_id(&k.exchange_type) == name && *n == i64::from(number)).map(|(k, _)| *k);
        for (venue, entry) in entries {
            match venue.as_ref().and_then(|(name, number)| by_name(name, *number)) {
                Some(key) => match routed.iter_mut().find(|(k, _)| k.id == key.id) { Some((_, rows)) => rows.push(entry), None => routed.push((key.clone(), vec![entry])) },
                None => if let Some((venue, number)) = venue {
                    let venue = if number == 1 { venue } else { format!("{venue} {number}") };
                    if !unrouted.contains(&venue) { unrouted.push(venue); }
                },
            }
        }
        unrouted.sort();
    }
    // #write: one transaction across every account the file touches. Fenced on no credential: an import reads none
    // (the venue's API is never called), so no credential change can make what it writes stale.
    let fenced = model::fence_versions(&tx, &[])?;
    let (mut imported, mut duplicates, mut skipped, mut generic) = (0, 0, 0, 0);
    for (reading, rows) in routed {
        let key = Key { id: reading.id, user_id, exchange_id: reading.exchange_id, status: 1, key_type: reading.key_type, last_synced_at: None,
                        last_sync_error: None, exchange_type: reading.exchange_type, scope: crate::sync::key_scope(&tx, reading.id)? };
        if format == "kucoin" {
            let out = kucoin::store(&fenced, &key, &kucoin_rows, now)?;
            (imported, duplicates, skipped, generic) = (imported + out.imported, duplicates + out.duplicates, skipped + out.skipped, generic + out.generic);
        } else {
            let out = if [BINANCE, BINANCE_US].contains(&key.exchange_type.as_str()) {
                crate::sync::binance_writer::store_csv(&fenced, cipher, &key, &rows, now)?
            } else if key.exchange_type == KUCOIN {
                // D-KUCOIN-HF-LEDGER: each row's exact decimals ride with it, and an HF-enabled history judges the
                // seconds the file touched before the import commits.
                let rows: Vec<Entry> = rows.into_iter().map(crate::sync::kucoin_hf::with_amounts).collect();
                let seconds = crate::sync::kucoin_hf::seconds_of(&rows);
                let out = ledger::store_file(&fenced, &key, rows, now)?;
                crate::sync::kucoin_hf::flush(&fenced, &key, seconds, now)?;
                out
            } else { ledger::store_file(&fenced, &key, rows, now)? };
            (imported, duplicates, skipped) = (imported + out.imported, duplicates + out.duplicates, skipped + out.skipped);
        }
        // Recorded only: the rows went where they always go, whatever account the file names.
        crate::identity::record_import(&fenced, cipher, user_id, reading.exchange_id, account_id.as_ref(), now).map_err(SyncError)?;
    }
    drop(fenced);
    tx.commit()?;
    Ok(Answer::Written { imported, duplicates, skipped, unrecognised, unrouted, generic })
}

#[cfg(test)]
mod tests {
    /// An amount BigDec holds but the column's double does not (2.x's BigDecimal holds it too, and 2.x stores
    /// Infinity) refuses the file before any database is touched.
    #[test]
    fn an_amount_no_double_holds_is_malformed() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        let file = "User ID,Time,Account,Operation,Coin,Change,Remark\n1,2026-08-01 10:00:00,Spot,Deposit,USDT,1e309,";
        assert_eq!(super::import(&c, &crate::crypto::Cipher::new(&crate::crypto::EncryptionKeys::resolve(&|_| None, "test").unwrap()), 1, "binance", file.as_bytes(), Some("+00:00"), None, chrono::Utc::now()).unwrap(), super::Answer::Error("malformed"));
    }

    #[test]
    fn a_permission_error_is_read_per_venue() {
        assert!(super::missing_permission("Exchanges::Kraken", Some("EGeneral:Permission denied")));
        assert!(!super::missing_permission("Exchanges::Binance", Some("EGeneral:Permission denied")));
        // KuCoin's 400007: the key is fine, it lacks the scope the sync needs.
        assert!(super::missing_permission("Exchanges::Kucoin", Some(r#"{"code":"400007","msg":"Access Denied"}"#)));
        assert!(!super::missing_permission("Exchanges::Kraken", Some(" ")) && !super::missing_permission("Exchanges::Kraken", None));
    }
}
