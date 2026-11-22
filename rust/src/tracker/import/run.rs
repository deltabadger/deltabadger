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
use rusqlite::{Connection, OptionalExtension};

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
    /// `ambiguous: true` and the venues or accounts the file could be from (#710 7a §3); nothing written.
    Ambiguous(Vec<Choice>),
    /// A named Binance identity refusal, returned only after the entire write has rolled back.
    Refused(String),
    /// The counts of the one write, with the readers' unrecognised operations and the venues no account takes.
    /// `generic`: of the imported entries, those a KuCoin file could only label generically (D-IMPORT-WARNINGS).
    Written { imported: usize, duplicates: usize, skipped: usize, unrecognised: Vec<String>, unrouted: Vec<String>, generic: usize },
    /// #710 7a: the file's export account was imported on this account (plain label) as a conflict before; nothing written.
    ImportedElsewhere(String),
}

/// #710 7a §3: an option of the import's question, posted back as `api_key_id`. `value`: a key id (that key's venue, the
/// value 2.x posts), `v<exchange>` (a venue), `a<account>` (an account) or `n<exchange>` (a new account there). `key`: the
/// venue's lone keyed account's reading key, the option 2.x offered; `new`: the "a new account" option (its label is the
/// page's).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice { pub value: String, pub label: String, pub key: Option<i64>, pub new: bool }

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
    let keys = crate::identity::with_key_account("k", |account| c.prepare(&format!("SELECT k.id, k.exchange_id, k.key_type, k.last_sync_error, e.type, {}, {account} FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
                           WHERE k.user_id = ?1 AND k.status = 1 AND k.key_type != 1 ORDER BY k.id", crate::identity::plain_label_sql("e.name", account)))?.query_map([user_id], |r| Ok((Reading { id: r.get(0)?, exchange_id: r.get(1)?, account_id: r.get(6)?, key_type: r.get(2)?, exchange_type: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
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
    let mut routed: Vec<(Key, Vec<Entry>)> = vec![];
    let mut unrouted: Vec<String> = vec![];
    // A venue's own export goes to that venue's account.
    let venues: &[&str] = match format { "binance" => &[BINANCE, BINANCE_US], "kucoin" => &[KUCOIN], _ => &[] };
    if !venues.is_empty() {
        let account = match route_file(&tx, cipher, user_id, venues, account_id.as_ref(), chosen, &keys, now)? {
            Routed::To(account) => writer(&tx, &keys, account)?,
            Routed::Key(k) => Key { id: k.id, user_id, exchange_id: k.exchange_id, status: 1, key_type: k.key_type, last_synced_at: None, last_sync_error: None,
                                    exchange_type: k.exchange_type, scope: crate::sync::key_scope(&tx, k.id)? },
            Routed::None => return Ok(Answer::Error("no_account_for_format")),
            Routed::Ask(choices) => return Ok(Answer::Ambiguous(choices)),
            Routed::Elsewhere(on) => return Ok(Answer::ImportedElsewhere(crate::identity::label(&tx, on)?.plain())),
        };
        routed.push((account, entries.into_iter().map(|(_, e)| e).collect()));
    } else {
        // `index_by(name_id)`: of two venues of one kind, the later key; #710: of the account the row's number names (an
        // account this install does not have is unrouted, named as plain text names it: `kraken 2`).
        // #710 7a §3: every live account takes its rows: one with no reading key (keyless, one an import made, or keyed only
        // for withdrawals or with an invalid key) writes them naming no key.
        let mut numbered: Vec<(Key, i64)> = vec![];
        for k in &keys {
            let number = match k.account_id { Some(id) => tx.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0))?, None => 1 };
            numbered.push((Key { id: k.id, user_id, exchange_id: k.exchange_id, status: 1, key_type: k.key_type, last_synced_at: None, last_sync_error: None,
                                 exchange_type: k.exchange_type.clone(), scope: crate::sync::key_scope(&tx, k.id)? }, number));
        }
        if crate::identity::ready(&tx)? {
            let accounts: Vec<(i64, i64, i64)> = tx.prepare("SELECT id, exchange_id, number FROM exchange_accounts WHERE user_id = ?1 AND removed_at IS NULL ORDER BY id")?
                .query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
            for (account, exchange, number) in accounts {
                if numbered.iter().any(|(k, n)| k.exchange_id == exchange && *n == number) { continue }
                numbered.push((writer(&tx, &[], account)?, number));
            }
        }
        let by_name = |name: &str, number: u32| numbered.iter().rev().find(|(k, n)| crate::tracker::rows::name_id(&k.exchange_type) == name && *n == i64::from(number)).map(|(k, _)| k);
        for (venue, entry) in entries {
            match venue.as_ref().and_then(|(name, number)| by_name(name, *number)) {
                Some(key) => match routed.iter_mut().find(|(k, _)| (k.exchange_id, k.scope.account, k.id) == (key.exchange_id, key.scope.account, key.id)) { Some((_, rows)) => rows.push(entry), None => routed.push((key.clone(), vec![entry])) },
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
    for (key, rows) in routed {
        let (routed_account, exchange_id) = (key.scope.account, key.exchange_id);
        if format == "kucoin" {
            // #710 slice 7: the source rows are stored by (user, exchange), whatever account the file was routed to; routing
            // imports by account is slice 7's.
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
        // #710: the file's identity is the account its key belongs to (the one the rows went to).
        crate::identity::record_import_to(&fenced, cipher, user_id, exchange_id, routed_account, account_id.as_ref(), now).map_err(SyncError)?;
    }
    drop(fenced);
    tx.commit()?;
    Ok(Answer::Written { imported, duplicates, skipped, unrecognised, unrouted, generic })
}

/// Where a venue's own export goes (#710 7a §3).
enum Routed { To(i64), Ask(Vec<Choice>), Elsewhere(i64), Key(Reading), None }

/// The file's account, under the write lock: 1. a live account of `venues` with the file's fingerprint; else, the venue
/// known (one venue, or the one chosen), 2. its one unfingerprinted live account, 3. none: a new keyless account; two or
/// more: the question, "a new account" offered too. A Binance-format file matched nowhere asks the venue first. A parked
/// answer is taken only while it still holds (an account removed, merged or given another fingerprint is asked again).
#[allow(clippy::too_many_arguments)]
fn route_file(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, venues: &[&str], file: Option<&crate::identity::AccountId>, chosen: Option<&str>,
              keys: &[Reading], now: DateTime<Utc>) -> Result<Routed, SyncError> {
    if !crate::identity::ready(c)? {
        // An install without the account tables (a `run` not migrated yet): 2.x's question over the venues' keys.
        let options: Vec<&Reading> = keys.iter().filter(|k| venues.contains(&k.exchange_type.as_str())).collect();
        return Ok(match (chosen.and_then(|id| options.iter().find(|k| k.id.to_string() == id)), options.as_slice()) {
            (Some(k), _) | (None, [k]) => Routed::Key((*k).clone()),
            (None, []) => Routed::None,
            (None, _) => Routed::Ask(options.iter().map(|k| Choice { value: k.id.to_string(), label: k.exchange_name.clone(), key: Some(k.id), new: false }).collect()),
        });
    }
    let exchanges: Vec<(i64, String, String)> = c.prepare("SELECT id, type, name FROM exchanges WHERE type IN (SELECT value FROM json_each(?1)) ORDER BY id")?
        .query_map([serde_json::json!(venues).to_string()], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<String>>(2)?.unwrap_or_default())))?.collect::<Result<_, _>>()?;
    // #710 7a §0.11, rule H4: a file another account of ITS venue holds as a conflict (imported there before 7a) is never
    // imported again, whoever now owns its fingerprint and whatever a parked answer says; a Binance conflict is no
    // Binance.US file's. Read once the venue is known, before any account is made.
    let conflict = |exchange: i64| -> Result<Option<i64>, SyncError> {
        match file { Some(id) => crate::identity::imported_as_conflict(c, cipher, user_id, exchange, id, now).map_err(SyncError), None => Ok(None) }
    };
    let identity = crate::identity::key(c, cipher, now).map_err(SyncError)?;
    let fingerprint = |exchange_type: &str| file.map(|id| identity.fingerprint(exchange_type, id));
    // (account, number, fingerprint) of each live account of `exchange`.
    let live = |exchange: i64| -> rusqlite::Result<Vec<(i64, i64, Option<String>)>> {
        c.prepare("SELECT id, number, fingerprint FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND removed_at IS NULL ORDER BY number")?
            .query_map([user_id, exchange], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
    };
    let offered = |exchange: i64| exchanges.iter().find(|(id, _, _)| *id == exchange);
    // The venue question: per venue, its lone account's lone reading key as 2.x answered it (one account), else the venue.
    let ask_venue = |among: &[i64]| -> rusqlite::Result<Routed> {
        let mut choices = vec![];
        for (id, _, name) in exchanges.iter().filter(|(id, _, _)| among.contains(id)) {
            let keyed: Vec<&Reading> = keys.iter().filter(|k| k.exchange_id == *id).collect();
            let key = (keyed.len() == 1 && live(*id)?.len() <= 1).then(|| keyed[0].id);
            choices.push(Choice { value: key.map_or_else(|| format!("v{id}"), |k| k.to_string()), label: name.clone(), key, new: false });
        }
        Ok(Routed::Ask(choices))
    };
    // The parked answer: an account (`a<id>`, or a key id: 2.x's answer, the parity runner's), a new account (`n<exchange>`),
    // or a venue (`v<exchange>`). It is read here and taken only after the file's fingerprint (1.), which it never overrides.
    let (mut venue, mut stale, mut chose_venue, mut parked) = (None, false, false, None::<Result<i64, i64>>);
    if let Some(chosen) = chosen {
        let number = |prefix: &str| chosen.strip_prefix(prefix).and_then(|n| n.parse::<i64>().ok());
        let keyed = chosen.parse::<i64>().ok().and_then(|id| keys.iter().find(|k| k.id == id))
            .map(|k| k.account_id.map_or_else(|| crate::identity::ensure_account(c, user_id, k.exchange_id, now), Ok)).transpose()?;
        if let Some(account) = number("a").or(keyed) {
            let row: Option<(i64, i64, Option<String>, bool)> = c.query_row("SELECT exchange_id, user_id, fingerprint, removed_at IS NOT NULL FROM exchange_accounts WHERE id = ?1", [account],
                                                                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
            if let Some((exchange, owner, held, removed)) = row {
                if let Some((_, exchange_type, _)) = offered(exchange).filter(|_| owner == user_id) {
                    // Removed, merged or read as another account since the question: asked again, never routed elsewhere.
                    if !removed && (held.is_none() || held == fingerprint(exchange_type)) { parked = Some(Ok(account)) } else { stale = true }
                    venue = Some(exchange);
                }
            }
        } else if let Some(exchange) = number("n").filter(|e| offered(*e).is_some()) {
            (venue, parked) = (Some(exchange), Some(Err(exchange)));
        } else {
            venue = number("v").filter(|e| offered(*e).is_some());
            chose_venue = venue.is_some();
        }
    }
    // 1. The file's fingerprint, under this lock: it wins over any parked account answer (another tab may have bound the
    // file's account meanwhile, or an earlier "a new account" answer made it). Matched in both venues: the venue is asked.
    let mut matched: Vec<(i64, i64)> = vec![];
    for (exchange, exchange_type, _) in &exchanges {
        if chose_venue && venue != Some(*exchange) { continue }
        if let Some((account, _, _)) = live(*exchange)?.into_iter().find(|(_, _, held)| held.is_some() && *held == fingerprint(exchange_type)) { matched.push((*exchange, account)) }
    }
    match matched.as_slice() {
        [] => {}
        [(exchange, account)] => return Ok(conflict(*exchange)?.map_or(Routed::To(*account), Routed::Elsewhere)),
        many => return Ok(ask_venue(&many.iter().map(|(e, _)| *e).collect::<Vec<_>>())?),
    }
    let known = venue.or(match exchanges.as_slice() { [(only, _, _)] => Some(*only), _ => None });
    if let Some(on) = known.map(conflict).transpose()?.flatten() { return Ok(Routed::Elsewhere(on)) }
    match parked {
        Some(Ok(account)) => return Ok(Routed::To(account)),
        Some(Err(exchange)) => return Ok(Routed::To(crate::identity::new_account(c, user_id, exchange, now)?)),
        None => {}
    }
    let venue = match (venue, exchanges.as_slice()) {
        (Some(v), _) => v,
        // No venue of the format on this install: nothing could hold the file.
        (None, []) => return Ok(Routed::None),
        (None, [(only, _, _)]) => *only,
        (None, _) => return Ok(ask_venue(&exchanges.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())?),
    };
    // 2 and 3: the venue's unfingerprinted accounts.
    let owners: Vec<i64> = live(venue)?.into_iter().filter(|(_, _, held)| held.is_none()).map(|(id, _, _)| id).collect();
    Ok(match owners.as_slice() {
        [] if !stale => Routed::To(crate::identity::new_account(c, user_id, venue, now)?),
        [one] if !stale => Routed::To(*one),
        many => {
            let mut choices = many.iter().map(|id| Ok(Choice { value: format!("a{id}"), label: crate::identity::label(c, *id)?.plain(), key: None, new: false })).collect::<rusqlite::Result<Vec<_>>>()?;
            choices.push(Choice { value: format!("n{venue}"), label: String::new(), key: None, new: true });
            Routed::Ask(choices)
        }
    })
}

/// The writer's key for `account`: its reading key, or none (`sync::NO_KEY`: a keyless account's rows have no api_key_id).
fn writer(c: &Connection, keys: &[Reading], account: i64) -> Result<Key, SyncError> {
    let target = crate::identity::Account::by_id(c, account)?.ok_or_else(|| SyncError(format!("import account {account} is gone")))?;
    let reading = keys.iter().find(|k| k.exchange_id == target.exchange_id && k.account_id == Some(account));
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [target.exchange_id], |r| r.get::<_, Option<String>>(0))?.unwrap_or_default();
    Ok(Key { id: reading.map_or(crate::sync::NO_KEY, |k| k.id), user_id: target.user_id, exchange_id: target.exchange_id, status: 1, key_type: reading.map_or(0, |k| k.key_type),
             last_synced_at: None, last_sync_error: None, exchange_type, scope: crate::venue_ids::Scope::of(c, &target)? })
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
