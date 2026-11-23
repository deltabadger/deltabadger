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
use chrono::{DateTime, Timelike, Utc};
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
    /// #710 7a rules M1, N4–N6, P5, P6, Q3, Q6: what the import can see wrong with writing the file where it would go, and
    /// the answers (each bound to what was seen, `Choice::value`); nothing written. An imported file is the user's word:
    /// it is warned about, never refused for its content.
    Warned(Vec<Warning>, Vec<Choice>),
}

/// One thing an import warns about before writing (#710 7a). Accounts are plain labels.
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// Rule M1(b): another live account (`0`) already holds this export's account: key read, key or file observation.
    HeldElsewhere(String),
    /// Rules M1(a)/(c), N5: the destination (`0`) is, reads or holds another account than this export's.
    OtherAccount(String),
    /// Rules P5/Q6: `0` rows of the file already exist on another live account (`1`) of the exchange, or on two
    /// destinations of this upload.
    Existing(usize, String),
    /// Rule N6: `0` rows of a Deltabadger CSV name accounts this install lacks; they are left out.
    Unrouted(usize),
    /// Rules P6/Q3: Binance rows exported from synced history this account has no record of (or edited since).
    Retained,
    /// Rule N4: what the answer was given for changed since; asked again.
    Changed,
}

/// What an "import anyway" option does (its label is the page's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Accept {
    /// Onto the destination the file would go to.
    Here,
    /// Onto a new account, the destination the file would go to.
    New,
    /// Onto another account (the choice's `label`), the one holding the export's account.
    To,
    /// A Deltabadger CSV's rows, warnings and all.
    Anyway,
    /// A Deltabadger CSV's rows that have an account here.
    Rest,
}

/// #710 7a §3: an option of the import's question, posted back as `api_key_id`. `value`: a key id (that key's venue, the
/// value 2.x posts), `v<exchange>` (a venue), `a<account>` (an account) or `n<exchange>` (a new account there). `key`: the
/// venue's lone keyed account's reading key, the option 2.x offered; `new`: the "a new account" option (its label is the
/// page's). `venue`: the venue a key answer stands for (rule S5: the page keeps it, so the answer keeps its venue after
/// its key is gone).
/// `accept`: an "import anyway" answer of a warning (#710 7a), `label` the account it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice { pub value: String, pub label: String, pub key: Option<i64>, pub new: bool, pub venue: Option<i64>, pub accept: Option<Accept> }

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

/// Import::Run#import!: `bytes` read in `format` at `offset` and written for `user_id` (`read`, then `write`), as an
/// upload with no parked subject (`write`'s `subject` empty).
#[allow(clippy::too_many_arguments)]
pub fn import(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, bytes: &[u8], offset: Option<&str>, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    match read(format, bytes, offset) { Ok(file) => write(c, cipher, user_id, format, file, "", chosen, now), Err(answer) => Ok(answer) }
}

/// Import::Run#route and #write for a file `read` accepted. `chosen` is the `api_key_id` a question was answered with:
/// taken only when it is one of the options offered and what it was offered for still holds (#710 7a rule N4).
/// `subject`: the parked upload's token, which every answer is bound to. Routing and writing run under one write lock
/// (`BEGIN IMMEDIATE`).
#[allow(clippy::too_many_arguments)]
pub fn write(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, subject: &str, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    match write_transaction(c, cipher, user_id, format, file, subject, chosen, now) {
        Err(error) => match crate::sync::binance_writer::csv_refusal(&error) {
            Some(reason) => Ok(Answer::Refused(reason.to_owned())),
            None => Err(error),
        },
        answer => answer,
    }
}

/// Rule N4: an answer posted back is its option's value and, after `!`, the digest it was rendered with: `p` a pick of
/// the account question, `w` an "import anyway". A value without one is 2.x's (a key id) or a venue.
fn split_answer(chosen: &str) -> (&str, Option<(char, &str)>) {
    match chosen.rsplit_once('!') {
        Some((body, tag)) if tag.len() > 1 && (tag.starts_with('p') || tag.starts_with('w')) => (body, Some((if tag.starts_with('p') { 'p' } else { 'w' }, &tag[1..]))),
        _ => (chosen, None),
    }
}

/// Where rows go: a live account, or a new account of an exchange (made only once the write goes ahead).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Dest { Account(i64), New(i64) }

fn sha(text: &str) -> String { use sha2::Digest; hex::encode(&sha2::Sha256::digest(text.as_bytes())[..8]) }

/// Rule N4: the digest an answer for `dest` is bound to: its purpose, the upload, the destination and its slots (each key
/// of the account, its type and its ciphertext; a new account: the exchange's live accounts) and what was seen there.
fn digest(c: &Connection, user_id: i64, purpose: char, subject: &str, dest: Dest, evidence: &str) -> Result<String, SyncError> {
    Ok(seal(purpose, subject, dest, &slot(c, user_id, dest)?, evidence))
}
pub(crate) fn seal(purpose: char, subject: &str, dest: Dest, slot: &str, evidence: &str) -> String { sha(&format!("{purpose}|{subject}|{dest:?}|{slot}|{evidence}")) }
/// A destination's slots: each key of the account, its type, status and ciphertext; for a new account, the exchange's
/// live accounts (one made meanwhile changes it).
pub(crate) fn slot(c: &Connection, user_id: i64, dest: Dest) -> Result<String, SyncError> {
    Ok(match dest {
        Dest::Account(id) => match crate::identity::Account::by_id(c, id)? {
            Some(account) => c.prepare(&format!("SELECT k.id, k.key_type, k.status, coalesce(k.key, ''), coalesce(k.secret, ''), coalesce(k.passphrase, '') FROM api_keys k WHERE {} ORDER BY k.id", account.keys("k")))?
                .query_map([], |r| Ok(format!("{}:{}:{}:{}", r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
                                               sha(&format!("{}|{}|{}", r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))))?
                .collect::<Result<Vec<_>, _>>()?.join(","),
            None => "gone".into(),
        },
        Dest::New(exchange) => crate::identity::accounts_of(c, user_id, exchange)?.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>().join(","),
    })
}

/// Rule N4: an answer is used once: consumed in the write it allows (a second answer of the same question finds it gone).
pub(crate) fn consume(c: &Connection, digest: &str, now: DateTime<Utc>) -> Result<bool, SyncError> {
    if !crate::engine::rules::table_exists(c, "parked_answers")? { return Ok(true); }
    Ok(c.execute("INSERT INTO parked_answers (digest, created_at) VALUES (?1, ?2) ON CONFLICT (digest) DO NOTHING", rusqlite::params![digest, crate::sync::sql_time(now)])? == 1)
}

/// The rows a destination would take, as the exact-id check reads them (rules P5/Q6).
struct Bound<'a> { entries: &'a [Entry], kucoin: &'a [kucoin::SourceRow] }

/// Rules P5/Q6: of the rows `bound` would write on `dest` (account `own`, or a new one) of `exchange`, those that exist
/// on another live account of it: an entry whose venue id it holds or merged (Alpaca's `merged_activity_ids`), an
/// id-less entry exactly repeating one of its rows (type, currency, amount, UTC second), a KuCoin row it holds. The
/// first such account, the count, and the ids seen (for the digest).
fn existing(c: &Connection, user_id: i64, exchange: i64, own: Option<i64>, bound: &Bound) -> Result<Option<(i64, usize, String)>, SyncError> {
    let others: Vec<i64> = crate::identity::accounts_of(c, user_id, exchange)?.into_iter().map(|(id, _)| id).filter(|id| Some(*id) != own).collect();
    if others.is_empty() { return Ok(None); }
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange], |r| r.get::<_, Option<String>>(0))?.unwrap_or_default();
    let (mut first, mut count, mut ids) = (None, 0, vec![]);
    for other in others {
        let key = writer(c, &[], other)?;
        let merged = ledger::stored_merged(c, &key)?;
        let at_ = "account_transactions";
        let mut hit = |id: String, first: &mut Option<i64>| { count += 1; ids.push(id); first.get_or_insert(other); };
        for e in bound.entries {
            match e.tx_id.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(id) => if crate::venue_ids::known(c, &key.scope, id)? || merged.contains(id) { hit(id.to_string(), &mut first) },
                None => {
                    let (Some(at), Some(base)) = (e.transacted_at, e.base_currency.as_deref()) else { continue };
                    let second = at.with_nanosecond(0).unwrap_or(at);
                    let same: bool = c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM account_transactions {} WHERE user_id = ?1 AND exchange_id = ?2 AND {} AND entry_type = ?3 \
                                                          AND base_currency = ?4 AND base_amount = ?5 AND transacted_at >= ?6 AND transacted_at < ?7)", ledger::BY_TIME, key.scope.rows(at_)),
                                                 rusqlite::params![user_id, exchange, e.entry_type, base, ledger::amount_sql(&exchange_type, &e.base_amount),
                                                                   crate::sync::sql_time(second), crate::sync::sql_time(second + chrono::Duration::seconds(1))], |r| r.get(0))?;
                    if same { hit(format!("{}|{base}|{}|{}", e.entry_type, e.base_amount.to_s_f(), second.timestamp()), &mut first) }
                }
            }
        }
        for row in bound.kucoin {
            let held: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM import_rows WHERE account_id = ?1 AND row_id = ?2)", rusqlite::params![other, row.row_id], |r| r.get(0))?;
            if held { hit(row.row_id.clone(), &mut first) }
        }
    }
    Ok(first.map(|a| (a, count, ids.join(","))))
}

/// What the import sees at `dest` before writing (rules M1, N5, P5, P8, Q6): the warnings, the accounts holding the
/// export's account (offered first), and the evidence the answer's digest binds.
#[allow(clippy::too_many_arguments)]
fn seen(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, dest: Dest, file: Option<&crate::identity::AccountId>, bound: &Bound, now: DateTime<Utc>)
        -> Result<(Vec<Warning>, Vec<i64>, String), SyncError> {
    let (exchange, own) = match dest {
        Dest::Account(id) => (crate::identity::Account::by_id(c, id)?.ok_or_else(|| SyncError(format!("import account {id} is gone")))?.exchange_id, Some(id)),
        Dest::New(exchange) => (exchange, None),
    };
    let (mut warnings, mut holders, mut facts) = (vec![], vec![], String::new());
    if let Some(id) = file.filter(|_| crate::identity::ready(c).unwrap_or(false)) {
        let e = crate::identity::import_evidence(c, cipher, user_id, exchange, own, id, now).map_err(SyncError)?;
        if let Some(first) = e.holders.first() { warnings.push(Warning::HeldElsewhere(crate::identity::label(c, *first)?.plain())); }
        if e.foreign { if let Some(own) = own { warnings.push(Warning::OtherAccount(crate::identity::label(c, own)?.plain())); } }
        (holders, facts) = (e.holders, e.facts);
    }
    if let Some((on, n, ids)) = existing(c, user_id, exchange, own, bound)? {
        warnings.push(Warning::Existing(n, crate::identity::label(c, on)?.plain()));
        facts.push_str(&format!("|ids:{ids}"));
    }
    Ok((warnings, holders, facts))
}

/// Where rows of one routed group go: a key's (or a keyless account's) writer, or a new account of an exchange.
#[derive(Clone)]
enum Target { Key(Key), New(i64) }

/// What a question asked before writing offers (rule N4): a venue export's destination and the accounts holding the
/// export's account, or a Deltabadger CSV's whole routing (`rest`: some of its rows have no account here).
enum Offer { Native { dest: Dest, holders: Vec<i64> }, Csv { rest: bool }, Unbound }

/// The routing of a file, before anything is written.
struct Plan { targets: Vec<(Target, Vec<Entry>)>, unrouted: Vec<String>, warnings: Vec<Warning>, evidence: String, changed: bool, offer: Offer }

/// The origin each importer's rows are recorded under (rule N8).
fn origin(format: &str) -> &'static str { match format { "binance" => "binance", "kucoin" => "kucoin", _ => "deltabadger" } }

// Returning an error drops the transaction before `write` translates a named refusal.
#[allow(clippy::too_many_arguments)]
fn write_transaction(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, subject: &str, chosen: Option<&str>, now: DateTime<Utc>) -> Result<Answer, SyncError> {
    let Read { entries, unrecognised, rows: kucoin_rows, account_id } = file;
    let tx = model::immediate(c)?;
    let keys = reading(&tx, user_id)?;
    let (body, tag) = match chosen { Some(chosen) => { let (body, tag) = split_answer(chosen); (Some(body), tag) } None => (None, None) };
    let accepting = matches!(tag, Some(('w', _)));
    // A venue's own export goes to that venue's account.
    let venues: &[&str] = match format { "binance" => &[BINANCE, BINANCE_US], "kucoin" => &[KUCOIN], _ => &[] };
    let plan = if venues.is_empty() { csv_plan(&tx, user_id, &keys, entries, body, tag)? } else {
        let entries: Vec<Entry> = entries.into_iter().map(|(_, e)| e).collect();
        match native_plan(&tx, cipher, user_id, venues, account_id.as_ref(), &keys, entries, &kucoin_rows, subject, body, tag, now)? {
            Ok(plan) => plan,
            Err(answer) => return Ok(answer),
        }
    };
    let Plan { targets, unrouted, warnings, evidence, changed, offer } = plan;
    let bound_entries: Vec<Entry> = targets.iter().flat_map(|(_, rows)| rows.iter().cloned()).collect();
    let ask = |c: &Connection, mut shown: Vec<Warning>, evidence: &str, changed: bool| -> Result<Answer, SyncError> {
        if changed { shown.insert(0, Warning::Changed); }
        let bound = Bound { entries: &bound_entries, kucoin: &kucoin_rows };
        Ok(Answer::Warned(shown, choices(c, cipher, user_id, subject, &offer, account_id.as_ref(), &bound, evidence, now)?))
    };
    // Rule N4 / M1: asked (again) before writing anything. Each question's choices (and their digests' slots) are read in
    // this transaction, so no credential change in between binds them to another slot; it commits only what routing made
    // (a first-named exchange's eager number-1 row, the identity key): the rows a write made first are undone.
    if changed || (!warnings.is_empty() && !accepting) { let asked = ask(&tx, warnings, &evidence, changed); tx.commit()?; return asked; }
    // The destination's slots as the answer saw them, before this write adds to them.
    let before = match &offer { Offer::Native { dest, .. } => Some((*dest, slot(&tx, user_id, *dest)?)), Offer::Csv { .. } => Some((Dest::New(0), slot(&tx, user_id, Dest::New(0))?)), Offer::Unbound => None };
    // A question found while writing is asked over the state before the write (`undo`).
    tx.execute_batch("SAVEPOINT import_write")?;
    let undo = |tx: &rusqlite::Transaction<'_>| tx.execute_batch("ROLLBACK TO import_write");
    let written = write_targets(&tx, cipher, user_id, format, targets, &kucoin_rows, account_id.as_ref(), unrecognised, unrouted, accepting, now);
    let (answer, stripped) = match written {
        Ok(written) => written,
        Err(error) => match crate::sync::binance_writer::warned(&error) {
            // Rules P6/Q3: Binance rows the writer warns about; nothing kept, asked with them.
            Some(units) => {
                undo(&tx)?;
                let mut shown = warnings;
                shown.push(Warning::Retained);
                let asked = ask(&tx, shown, &format!("{evidence}|retained:{units:?}"), false);
                tx.commit()?;
                return asked;
            }
            None => return Err(error),
        },
    };
    let evidence = if stripped.is_empty() { evidence } else { format!("{evidence}|retained:{stripped:?}") };
    if let Some((purpose, posted)) = tag {
        // An "import anyway" holds only while everything it was given for does; any answer counts once.
        let stale = purpose == 'w' && before.as_ref().is_none_or(|(dest, slot)| posted != seal('w', subject, *dest, slot, &evidence));
        if stale || !consume(&tx, posted, now)? {
            undo(&tx)?;
            let mut shown = warnings;
            if !stripped.is_empty() { shown.push(Warning::Retained); }
            let asked = ask(&tx, shown, &evidence, true);
            tx.commit()?;
            return asked;
        }
    }
    // Rule N4: a parked upload is written once, whichever of its answers comes first; a later one writes nothing.
    if !subject.is_empty() && !consume(&tx, &sha(&format!("u|{subject}")), now)? {
        drop(tx);
        return Ok(Answer::Written { imported: 0, duplicates: 0, skipped: 0, unrecognised: vec![], unrouted: vec![], generic: 0 });
    }
    tx.commit()?;
    Ok(answer)
}

/// The answers of a warning (rule M1): each account holding the export's account first ("import to"), then the
/// destination ("import here", or a new account); a Deltabadger CSV's "import anyway" / "import the rest". Cancel is the
/// page's.
#[allow(clippy::too_many_arguments)]
fn choices(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, subject: &str, offer: &Offer, file: Option<&crate::identity::AccountId>, bound: &Bound,
           evidence: &str, now: DateTime<Utc>) -> Result<Vec<Choice>, SyncError> {
    let choice = |value: String, label: String, venue: Option<i64>, accept: Accept| Choice { value, label, key: None, new: false, venue, accept: Some(accept) };
    Ok(match offer {
        Offer::Native { dest, holders } => {
            let mut out = vec![];
            for holder in holders {
                let at = Dest::Account(*holder);
                let (_, _, facts) = seen(c, cipher, user_id, at, file, bound, now)?;
                let venue = crate::identity::Account::by_id(c, *holder)?.map(|a| a.exchange_id);
                out.push(choice(format!("a{holder}!w{}", digest(c, user_id, 'w', subject, at, &facts)?), crate::identity::label(c, *holder)?.plain(), venue, Accept::To));
            }
            let d = digest(c, user_id, 'w', subject, *dest, evidence)?;
            out.push(match dest {
                Dest::Account(id) => choice(format!("a{id}!w{d}"), crate::identity::label(c, *id)?.plain(), crate::identity::Account::by_id(c, *id)?.map(|a| a.exchange_id), Accept::Here),
                Dest::New(exchange) => choice(format!("n{exchange}!w{d}"), String::new(), Some(*exchange), Accept::New),
            });
            out
        }
        Offer::Csv { rest } => vec![choice(format!("r!w{}", digest(c, user_id, 'w', subject, Dest::New(0), evidence)?), String::new(), None, if *rest { Accept::Rest } else { Accept::Anyway })],
        Offer::Unbound => vec![],
    })
}

/// An "import anyway" answer's destination, while it is still one (a live account of the user on a venue of the
/// format, or a new account on one): else None, and the import is asked again.
fn accepted_dest(c: &Connection, user_id: i64, venues: &[&str], body: &str) -> Result<Option<Dest>, SyncError> {
    let venue_of = |exchange: i64| -> rusqlite::Result<bool> {
        let t: Option<Option<String>> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange], |r| r.get(0)).optional()?;
        Ok(t.flatten().is_some_and(|t| venues.contains(&t.as_str())))
    };
    if let Some(id) = body.strip_prefix('a').and_then(|n| n.parse::<i64>().ok()) {
        let row: Option<(i64, i64, bool)> = c.query_row("SELECT user_id, exchange_id, removed_at IS NULL FROM exchange_accounts WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
        return Ok(match row { Some((owner, exchange, true)) if owner == user_id && venue_of(exchange)? => Some(Dest::Account(id)), _ => None });
    }
    if let Some(exchange) = body.strip_prefix('n').and_then(|n| n.parse::<i64>().ok()) { return Ok(venue_of(exchange)?.then_some(Dest::New(exchange))); }
    Ok(None)
}

/// The destination a pick names: an account (`a<id>~…`), a new account (`n<exchange>~…`), a key's account.
fn named_dest(c: &Connection, user_id: i64, keys: &[Reading], body: &str) -> Result<Option<Dest>, SyncError> {
    let (head, _) = body.split_once('~').unwrap_or((body, ""));
    if let Some(id) = head.strip_prefix('a').and_then(|n| n.parse::<i64>().ok()) { return Ok(Some(Dest::Account(id))); }
    if let Some(exchange) = head.strip_prefix('n').and_then(|n| n.parse::<i64>().ok()) { return Ok(Some(Dest::New(exchange))); }
    let Some(k) = head.parse::<i64>().ok().and_then(|id| keys.iter().find(|k| k.id == id)) else { return Ok(None) };
    Ok(match k.account_id { Some(a) => Some(a), None => crate::identity::Account::first(c, user_id, k.exchange_id)?.id }.map(Dest::Account))
}

/// A venue export's plan (`route_file`, then what the destination shows): Err with the answer when it is a question of
/// routing or no account can take it.
#[allow(clippy::too_many_arguments)]
fn native_plan(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, venues: &[&str], file: Option<&crate::identity::AccountId>, keys: &[Reading], entries: Vec<Entry>,
               kucoin_rows: &[kucoin::SourceRow], subject: &str, body: Option<&str>, tag: Option<(char, &str)>, now: DateTime<Utc>) -> Result<Result<Plan, Answer>, SyncError> {
    let bound = Bound { entries: &entries, kucoin: kucoin_rows };
    // An "import anyway" names its destination itself; it holds while its digest does (`write_transaction`).
    let accepted = match (tag, body) { (Some(('w', _)), Some(body)) => accepted_dest(c, user_id, venues, body)?, _ => None };
    let mut changed = tag.is_some_and(|(t, _)| t == 'w') && accepted.is_none();
    let routing = if tag.is_some_and(|(t, _)| t == 'w') { None } else { body };
    let dest = match accepted {
        Some(dest) => dest,
        None => match route_file(c, cipher, user_id, venues, file, routing, keys, now)? {
            Routed::To(account) => Dest::Account(account),
            Routed::New(exchange) => Dest::New(exchange),
            Routed::Key(k) => {
                let key = Key { id: k.id, user_id, exchange_id: k.exchange_id, status: 1, key_type: k.key_type, last_synced_at: None, last_sync_error: None,
                                exchange_type: k.exchange_type, scope: crate::sync::key_scope(c, k.id)? };
                return Ok(Ok(Plan { targets: vec![(Target::Key(key), entries)], unrouted: vec![], warnings: vec![], evidence: String::new(), changed: false, offer: Offer::Unbound }));
            }
            Routed::None => return Ok(Err(Answer::Error("no_account_for_format"))),
            // Rule N4: each account the question offers is bound to what its destination holds now. The venue question's
            // key answer stays 2.x's (a key id): it holds only while its venue's one account and key do (`route_file`).
            Routed::Ask(mut options) => {
                for o in options.iter_mut().filter(|o| o.key.is_none()) {
                    if let Some(dest) = named_dest(c, user_id, keys, &o.value)? {
                        let (_, _, facts) = seen(c, cipher, user_id, dest, file, &bound, now)?;
                        o.value = format!("{}!p{}", o.value, digest(c, user_id, 'p', subject, dest, &facts)?);
                    }
                }
                return Ok(Err(Answer::Ambiguous(options)));
            }
        },
    };
    match (tag, body) {
        // A pick names the destination it was offered for: one the file's account moved to meanwhile is asked again.
        (Some(('p', _)), Some(body)) => if named_dest(c, user_id, keys, body)? != Some(dest) { changed = true },
        // An account or new-account answer rendered with no digest: asked again.
        (None, Some(body)) if body.starts_with('a') || body.starts_with('n') => changed = true,
        _ => {}
    }
    let (warnings, holders, evidence) = seen(c, cipher, user_id, dest, file, &bound, now)?;
    if let Some(('p', posted)) = tag { if posted != digest(c, user_id, 'p', subject, dest, &evidence)? { changed = true; } }
    let target = match dest { Dest::Account(account) => Target::Key(writer(c, keys, account)?), Dest::New(exchange) => Target::New(exchange) };
    Ok(Ok(Plan { targets: vec![(target, entries)], unrouted: vec![], warnings, evidence, changed, offer: Offer::Native { dest, holders } }))
}

/// A Deltabadger CSV's plan: each row to the account its venue and number name.
fn csv_plan(c: &Connection, user_id: i64, keys: &[Reading], entries: Vec<Named>, body: Option<&str>, tag: Option<(char, &str)>) -> Result<Plan, SyncError> {
    let mut routed: Vec<(Key, Vec<Entry>)> = vec![];
    let mut unrouted: Vec<String> = vec![];
    // `index_by(name_id)`: of two venues of one kind, the later key; #710: of the account the row's number names (an
    // account this install does not have is unrouted, named as plain text names it: `kraken 2`).
    // #710 7a §3: every live account takes its rows: one with no reading key (keyless, one an import made, or keyed only
    // for withdrawals or with an invalid key) writes them naming no key.
    let mut numbered: Vec<(Key, i64)> = vec![];
    for k in keys {
        let number = match k.account_id { Some(id) => c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0))?, None => 1 };
        numbered.push((Key { id: k.id, user_id, exchange_id: k.exchange_id, status: 1, key_type: k.key_type, last_synced_at: None, last_sync_error: None,
                             exchange_type: k.exchange_type.clone(), scope: crate::sync::key_scope(c, k.id)? }, number));
    }
    if crate::identity::ready(c)? {
        let accounts: Vec<(i64, i64, i64)> = c.prepare("SELECT id, exchange_id, number FROM exchange_accounts WHERE user_id = ?1 AND removed_at IS NULL ORDER BY id")?
            .query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        for (account, exchange, number) in accounts {
            if numbered.iter().any(|(k, n)| k.exchange_id == exchange && *n == number) { continue }
            numbered.push((writer(c, &[], account)?, number));
        }
    }
    let by_name = |name: &str, number: u32| numbered.iter().rev().find(|(k, n)| crate::tracker::rows::name_id(&k.exchange_type) == name && *n == i64::from(number)).map(|(k, _)| k);
    let mut left_out = 0;
    for (venue, entry) in entries {
        match venue.as_ref().and_then(|(name, number)| by_name(name, *number)) {
            Some(key) => match routed.iter_mut().find(|(k, _)| (k.exchange_id, k.scope.account, k.id) == (key.exchange_id, key.scope.account, key.id)) { Some((_, rows)) => rows.push(entry), None => routed.push((key.clone(), vec![entry])) },
            // Rule N6: a row naming no exchange has no account here either: counted, left out.
            None => {
                left_out += 1;
                if let Some((venue, number)) = venue {
                    let venue = if number == 1 { venue } else { format!("{venue} {number}") };
                    if !unrouted.contains(&venue) { unrouted.push(venue); }
                }
            }
        }
    }
    unrouted.sort();
    // Rule N6: rows naming accounts this install lacks are said before writing. Rules P5/Q6: rows another live account
    // holds, and a venue id this upload routes to two accounts. The digest binds every group's account, key and slots.
    let mut warnings = vec![];
    if left_out > 0 { warnings.push(Warning::Unrouted(left_out)); }
    let mut evidence = format!("unrouted:{left_out}:{};", unrouted.join(","));
    let (mut found, mut twice): (Option<(i64, usize)>, usize) = (None, 0);
    for (i, (key, rows)) in routed.iter().enumerate() {
        let own = match key.scope.account { Some(a) => Some(a), None => crate::identity::accounts_of(c, user_id, key.exchange_id)?.first().map(|(a, _)| *a) };
        let slot = match own { Some(a) => digest(c, user_id, 's', "", Dest::Account(a), "")?, None => String::new() };
        evidence.push_str(&format!("{}:{}:{:?}:{}:{slot};", key.id, key.exchange_id, own, rows.len()));
        for id in rows.iter().filter_map(|e| e.tx_id.as_deref()).filter(|t| !t.trim().is_empty()) {
            if routed[..i].iter().any(|(other, theirs)| other.exchange_id == key.exchange_id && theirs.iter().any(|x| x.tx_id.as_deref() == Some(id))) {
                twice += 1;
                evidence.push_str(&format!("twice:{id};"));
                if found.is_none() { found = own.map(|a| (a, 0)); }
            }
        }
        if let Some((on, n, ids)) = existing(c, user_id, key.exchange_id, own, &Bound { entries: rows, kucoin: &[] })? {
            evidence.push_str(&format!("ids:{ids};"));
            found = Some(match found { Some((first, m)) if m > 0 => (first, m + n), _ => (on, n) });
        }
    }
    if let Some((on, n)) = found { warnings.push(Warning::Existing(n + twice, crate::identity::label(c, on)?.plain())); }
    // Only "import anyway" (`r`) answers a Deltabadger CSV's warning; anything else posted is asked again.
    let changed = tag.is_some() && (body != Some("r") || tag.is_some_and(|(t, _)| t != 'w'));
    Ok(Plan { targets: routed.into_iter().map(|(k, rows)| (Target::Key(k), rows)).collect(), unrouted, warnings, evidence, changed, offer: Offer::Csv { rest: left_out > 0 } })
}

/// #write: one transaction across every account the file touches. Fenced on no credential: an import reads none (the
/// venue's API is never called), so no credential change can make what it writes stale. `accept`: the Binance rows the
/// writer would warn about are written plain (rules P6/Q3); returns the answer and those it stripped (`key:unit`).
#[allow(clippy::too_many_arguments)]
fn write_targets(tx: &rusqlite::Transaction<'_>, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, targets: Vec<(Target, Vec<Entry>)>, kucoin_rows: &[kucoin::SourceRow],
                 account_id: Option<&crate::identity::AccountId>, unrecognised: Vec<String>, unrouted: Vec<String>, accept: bool, now: DateTime<Utc>) -> Result<(Answer, Vec<String>), SyncError> {
    let fenced = model::fence_versions(tx, &[])?;
    let (mut imported, mut duplicates, mut skipped, mut generic) = (0, 0, 0, 0);
    let mut stripped = vec![];
    let origin = origin(format);
    for (target, rows) in targets {
        let key = match target { Target::Key(key) => key, Target::New(exchange) => writer(&fenced, &[], crate::identity::new_account(&fenced, user_id, exchange, now)?)? };
        let (routed_account, exchange_id) = (key.scope.account, key.exchange_id);
        if format == "kucoin" {
            // #710 slice 7: the source rows are stored by (user, exchange), whatever account the file was routed to; routing
            // imports by account is slice 7's.
            let out = kucoin::store(&fenced, &key, kucoin_rows, now)?;
            (imported, duplicates, skipped, generic) = (imported + out.imported, duplicates + out.duplicates, skipped + out.skipped, generic + out.generic);
        } else {
            let out = if [BINANCE, BINANCE_US].contains(&key.exchange_type.as_str()) {
                let (out, units) = crate::sync::binance_writer::store_csv(&fenced, cipher, &key, &rows, origin, accept, now)?;
                stripped.extend(units.into_iter().map(|u| format!("{}:{u}", key.id)));
                out
            } else if key.exchange_type == KUCOIN {
                // D-KUCOIN-HF-LEDGER: each row's exact decimals ride with it, and an HF-enabled history judges the
                // seconds the file touched before the import commits.
                let rows: Vec<Entry> = rows.into_iter().map(crate::sync::kucoin_hf::with_amounts).collect();
                let seconds = crate::sync::kucoin_hf::seconds_of(&rows);
                let out = ledger::store_file(&fenced, &key, rows, origin, now)?;
                crate::sync::kucoin_hf::flush(&fenced, &key, seconds, now)?;
                out
            } else { ledger::store_file(&fenced, &key, rows, origin, now)? };
            (imported, duplicates, skipped) = (imported + out.imported, duplicates + out.duplicates, skipped + out.skipped);
        }
        // Recorded only: the rows went where they always go, whatever account the file names.
        // #710: the file's identity is the account its key belongs to (the one the rows went to).
        crate::identity::record_import_to(&fenced, cipher, user_id, exchange_id, routed_account, account_id, now).map_err(SyncError)?;
    }
    drop(fenced);
    Ok((Answer::Written { imported, duplicates, skipped, unrecognised, unrouted, generic }, stripped))
}

/// Where a venue's own export goes (#710 7a §3).
enum Routed { To(i64), New(i64), Ask(Vec<Choice>), Key(Reading), None }

/// SQL on account row `x`: the fingerprint a file routes by (`route_file`): the account's own, else its first import
/// observation's.
const ROUTING: &str = "COALESCE(x.fingerprint, (SELECT i.fingerprint FROM exchange_account_imports i WHERE i.account_id = x.id ORDER BY i.id LIMIT 1))";
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
        let chosen = chosen.map(|c| c.split_once('~').map_or(c, |(key, _)| key));
        return Ok(match (chosen.and_then(|id| options.iter().find(|k| k.id.to_string() == id)), options.as_slice()) {
            (Some(k), _) | (None, [k]) => Routed::Key((*k).clone()),
            (None, []) => Routed::None,
            (None, _) => Routed::Ask(options.iter().map(|k| Choice { value: k.id.to_string(), label: k.exchange_name.clone(), key: Some(k.id), new: false, venue: Some(k.exchange_id), accept: None }).collect()),
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
    // (account, number, fingerprint) of each live account of `exchange`. #710 7a rule M3: a file sets no fingerprint, so an
    // account known only from files routes by its first import observation (the UID a file named there first), as the
    // fingerprint that file used to set.
    let live = |exchange: i64| -> rusqlite::Result<Vec<(i64, i64, Option<String>)>> {
        c.prepare(&format!("SELECT id, number, {ROUTING} FROM exchange_accounts x WHERE user_id = ?1 AND exchange_id = ?2 AND removed_at IS NULL ORDER BY number"))?
            .query_map([user_id, exchange], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
    };
    let offered = |exchange: i64| exchanges.iter().find(|(id, _, _)| *id == exchange);
    // The venue question's answer for a venue: its lone account's lone reading key as 2.x answered it (one account), else
    // the venue.
    let venue_key = |id: i64| -> rusqlite::Result<Option<i64>> {
        let keyed: Vec<&Reading> = keys.iter().filter(|k| k.exchange_id == id).collect();
        Ok((keyed.len() == 1 && live(id)?.len() <= 1).then(|| keyed[0].id))
    };
    let ask_venue = |among: &[i64]| -> rusqlite::Result<Routed> {
        let mut choices = vec![];
        for (id, _, name) in exchanges.iter().filter(|(id, _, _)| among.contains(id)) {
            let key = venue_key(*id)?;
            choices.push(Choice { value: key.map_or_else(|| format!("v{id}"), |k| k.to_string()), label: name.clone(), key, new: false, venue: Some(*id), accept: None });
        }
        Ok(Routed::Ask(choices))
    };
    // The account question's candidates for a venue (its unfingerprinted live accounts), and the set an answer is bound to.
    let owners = |venue: i64| -> rusqlite::Result<Vec<i64>> { Ok(live(venue)?.into_iter().filter(|(_, _, held)| held.is_none()).map(|(id, _, _)| id).collect()) };
    let set_of = |owners: &[i64]| owners.iter().map(i64::to_string).collect::<Vec<_>>().join("-");
    // The parked answer (rule H3): an account (`a<id>~<set>`, or a key id: the venue question's, 2.x's, the parity
    // runner's), a new account (`n<exchange>~<set>`), or a venue (`v<exchange>`). Each is bound to what it was rendered
    // for, revalidated here under the import's lock: a key id only while the venue question would still offer it (its
    // venue's one account), an account or new-account answer only while its venue's candidates are the `set` it was
    // rendered with; otherwise asked again. Its venue is the venue for the rest of routing. It is read here and taken only
    // after the file's fingerprint (1.), which it never overrides.
    let (mut venue, mut stale, mut chose_venue, mut parked) = (None, false, false, None::<Result<i64, i64>>);
    if let Some(chosen) = chosen {
        let (body, set) = chosen.split_once('~').map_or((chosen, None), |(b, s)| (b, Some(s)));
        let number = |prefix: &str| body.strip_prefix(prefix).and_then(|n| n.parse::<i64>().ok());
        let key = body.parse::<i64>().ok().and_then(|id| keys.iter().find(|k| k.id == id)).filter(|k| offered(k.exchange_id).is_some());
        let offered_key = match key { Some(k) => venue_key(k.exchange_id)? == Some(k.id), None => false };
        let keyed = key.filter(|_| offered_key).map(|k| k.account_id.map_or_else(|| crate::identity::ensure_account(c, user_id, k.exchange_id, now), Ok)).transpose()?;
        // A key id the venue question no longer offers: the venue is asked again.
        let bound = |exchange: i64| -> rusqlite::Result<bool> { Ok(set == Some(set_of(&owners(exchange)?).as_str())) };
        if let Some(account) = number("a").or(keyed) {
            let row: Option<(i64, i64, Option<String>, bool)> = c.query_row(&format!("SELECT exchange_id, user_id, {ROUTING}, removed_at IS NOT NULL FROM exchange_accounts x WHERE id = ?1"), [account],
                                                                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
            if let Some((exchange, owner, held, removed)) = row {
                if let Some((_, exchange_type, _)) = offered(exchange).filter(|_| owner == user_id) {
                    // Removed, merged or read as another account since the question: asked again, never routed elsewhere.
                    let current = keyed.is_some() || bound(exchange)?;
                    if current && !removed && (held.is_none() || held == fingerprint(exchange_type)) { parked = Some(Ok(account)) } else { stale = true }
                    venue = Some(exchange);
                }
            }
        } else if let Some(exchange) = number("n").filter(|e| offered(*e).is_some()) {
            venue = Some(exchange);
            if bound(exchange)? { parked = Some(Err(exchange)) } else { stale = true }
        } else if let Some(k) = key {
            // 7a rule R10: a key answer the venue question no longer offers keeps its venue, and is asked again there.
            (venue, stale) = (Some(k.exchange_id), true);
        } else if body.parse::<i64>().is_ok() {
            // Rule S5: a key answer whose key is gone is asked again on the venue it was given for (`~v<exchange>`, kept
            // by the page); one that names no venue asks the venue again. Never "no venue", so never a fingerprint elsewhere.
            match set.and_then(|s| s.strip_prefix('v')).and_then(|e| e.parse::<i64>().ok()).filter(|e| offered(*e).is_some()) {
                Some(exchange) => (venue, stale) = (Some(exchange), true),
                None => return Ok(ask_venue(&exchanges.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())?),
            }
        } else {
            venue = number("v").filter(|e| offered(*e).is_some());
        }
        chose_venue = venue.is_some();
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
        // R10: with no venue chosen, a file another venue holds as a conflict is never routed here on a fingerprint match:
        // the venue is asked (that venue answered, it is refused there).
        [(exchange, account)] => {
            if !chose_venue {
                for (other, _, _) in exchanges.iter().filter(|(e, _, _)| e != exchange) {
                    if conflict(*other)?.is_some() { return Ok(ask_venue(&exchanges.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())?) }
                }
            }
            // Rule M1(b): a file another account holds as a conflict lands here, with its warning (`seen`).
            return Ok(Routed::To(*account))
        }
        many => return Ok(ask_venue(&many.iter().map(|(e, _)| *e).collect::<Vec<_>>())?),
    }
    match parked {
        Some(Ok(account)) => return Ok(Routed::To(account)),
        Some(Err(exchange)) => return Ok(Routed::New(exchange)),
        None => {}
    }
    let venue = match (venue, exchanges.as_slice()) {
        (Some(v), _) => v,
        // No venue of the format on this install: nothing could hold the file.
        (None, []) => return Ok(Routed::None),
        (None, [(only, _, _)]) => *only,
        (None, _) => {
            // Plan L8, as main: the one reading key of the format's venues, its venue's one account, takes a clean upload,
            // unless another of them already observed the file's UID (rules G1/J2: then the venue is asked).
            let readers: Vec<&Reading> = keys.iter().filter(|k| exchanges.iter().any(|(id, _, _)| *id == k.exchange_id)).collect();
            if let [k] = readers.as_slice().iter().filter(|k| venue_key(k.exchange_id).ok().flatten() == Some(k.id)).collect::<Vec<_>>().as_slice() {
                let mut seen_elsewhere = false;
                for (other, exchange_type, _) in exchanges.iter().filter(|(id, _, _)| *id != k.exchange_id) {
                    let Some(print) = fingerprint(exchange_type) else { continue };
                    seen_elsewhere |= c.query_row("SELECT EXISTS(SELECT 1 FROM exchange_account_imports WHERE user_id = ?1 AND exchange_id = ?2 AND fingerprint = ?3)
                                                     OR EXISTS(SELECT 1 FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id WHERE k.user_id = ?1 AND k.exchange_id = ?2 AND o.fingerprint = ?3)
                                                     OR EXISTS(SELECT 1 FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND fingerprint = ?3)",
                                                  rusqlite::params![user_id, other, print], |r| r.get::<_, bool>(0))?;
                }
                if !seen_elsewhere && !stale {
                    return Ok(Routed::To(match k.account_id { Some(a) => a, None => crate::identity::ensure_account(c, user_id, k.exchange_id, now)? }));
                }
            }
            return Ok(ask_venue(&exchanges.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())?)
        }
    };
    // 2 and 3: the venue's unfingerprinted accounts.
    let owners = owners(venue)?;
    let set = set_of(&owners);
    Ok(match owners.as_slice() {
        [] if !stale => Routed::New(venue),
        [one] if !stale => Routed::To(*one),
        many => {
            let mut choices = many.iter().map(|id| Ok(Choice { value: format!("a{id}~{set}"), label: crate::identity::label(c, *id)?.plain(), key: None, new: false, venue: Some(venue), accept: None })).collect::<rusqlite::Result<Vec<_>>>()?;
            choices.push(Choice { value: format!("n{venue}~{set}"), label: String::new(), key: None, new: true, venue: Some(venue), accept: None });
            Routed::Ask(choices)
        }
    })
}

/// The writer's key for `account`: its reading key, or none (`sync::NO_KEY`: a keyless account's rows have no api_key_id).
fn writer(c: &Connection, keys: &[Reading], account: i64) -> Result<Key, SyncError> {
    let target = crate::identity::Account::by_id(c, account)?.ok_or_else(|| SyncError(format!("import account {account} is gone")))?;
    // #710 7a rule S9: a reader read before its account had a row (a legacy key, the answer making number 1's row) is
    // matched by its account now, so its rows keep its key as main's do.
    let mut reading = None;
    for k in keys.iter().filter(|k| k.exchange_id == target.exchange_id) {
        let of = match k.account_id { Some(a) => Some(a), None => crate::identity::account_of_key(c, k.id)? };
        if of == Some(account) { reading = Some(k); break; }
    }
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
