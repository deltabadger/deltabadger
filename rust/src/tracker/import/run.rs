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
use crate::identity::Label;
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
    /// `destinations`: (exchange, account, rows) of each account the write went to, an account it made included
    /// (PLAN-710-confirm rule L8).
    Written { imported: usize, duplicates: usize, skipped: usize, unrecognised: Vec<String>, unrouted: Vec<String>, generic: usize, destinations: Vec<(i64, i64, usize)> },
    /// #710 7a rules M1, N4–N6, P5, P6, Q3, Q6: what the import can see wrong with writing the file where it would go, and
    /// the answers (each bound to what was seen, `Choice::value`); nothing written. An imported file is the user's word:
    /// it is warned about, never refused for its content.
    Warned(Vec<Warning>, Vec<Choice>),
}

/// One thing an import warns about before writing (#710 7a). Accounts are labels, numbered as the question names them
/// (#710 8a rule R6).
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// Rule M1(b): another live account (`0`) already holds this export's account: key read, key or file observation.
    HeldElsewhere(Label),
    /// Rules M1(a)/(c), N5: the destination (`0`) is, reads or holds another account than this export's.
    OtherAccount(Label),
    /// Rules P5/Q6, #710 8a rule R12: per other live account of the exchange, the rows of the file it already holds.
    Existing(Vec<(Label, usize)>),
    /// #710 8a rule R2: where a Deltabadger CSV's rows go, per account (asked when an exchange of it has 2+ accounts).
    Destination(Vec<(Label, usize)>),
    /// #710 8a rule R13: the venue ids this file sends to two or more accounts.
    Twice(usize),
    /// Rule N6: `0` rows of a Deltabadger CSV name accounts this install lacks; they are left out.
    Unrouted(usize),
    /// Rules P6/Q3: Binance rows exported from synced history this account has no record of (or edited since).
    Retained,
    /// D-BINANCE-DUST-WARNS: a Binance dust conversion leg without its pair, or a file pair whose stored pair another of
    /// the file's pairs already matched.
    Dust,
    /// Rule N4: what the answer was given for changed since; asked again.
    Changed,
    /// PLAN-710-confirm rule L2: the names of the accounts a Deltabadger CSV's left-out rows name.
    LeftOut(Vec<String>),
    /// PLAN-710-confirm rules M4/Q1: the transfer links on other accounts the write undoes (a link cleared, or a row deleted
    /// whose partner sits elsewhere), and those accounts.
    Undone(usize, Vec<Label>),
}

/// PLAN-710-confirm rules M1/N1: what a question's option is. `Select` picks a venue that still has several possible
/// accounts (its click only asks the next question); `Authorize` writes on its click to the one destination its text
/// names, while the route it was issued for holds; `Accept` is an answer 8a's digest seals (a pick, an "import anyway").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Select, Authorize, Accept }

/// What a question issued for each of its options (parallel to the answer's choices): its kind, the route its click was
/// issued for (rule M1, an `Authorize`), and the transfer links on other accounts it showed (rule M4; None: none computed).
#[derive(Debug, Clone, PartialEq)]
pub struct Issued { pub kind: Kind, pub route: Option<String>, pub outside: Option<String> }

/// A confirmation's sealed context (PLAN-710-confirm): the answer posted (`chosen`), and what its handle was issued for.
/// `stale`: its handle did not resolve (asked again, `Changed`); `select`: a venue pick, which only asks; `picked`: the venue
/// the user picked earlier (rule L3, persisted).
#[derive(Debug, Clone, Default)]
pub struct Confirm<'a> { pub chosen: Option<&'a str>, pub stale: bool, pub select: bool, pub route: Option<&'a str>, pub outside: Option<&'a str>, pub picked: Option<i64> }

/// `write_in`'s result: the answer and, for a question, what it issued per choice.
#[derive(Debug, Clone)]
pub struct Ran { pub answer: Answer, pub issued: Vec<Issued> }

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
    /// #710 8a rule R2: a Deltabadger CSV whose only note is where its rows go.
    Import,
}

/// #710 7a §3: an option of the import's question, posted back as `api_key_id`. `value`: a key id (that key's venue, the
/// value 2.x posts), `v<exchange>` (a venue), `a<account>` (an account) or `n<exchange>` (a new account there). `key`: the
/// venue's lone keyed account's reading key, the option 2.x offered; `new`: the "a new account" option (its label is the
/// page's). `venue`: the venue a key answer stands for (rule S5: the page keeps it, so the answer keeps its venue after
/// its key is gone).
/// `accept`: an "import anyway" answer of a warning (#710 7a). #710 8a rule R3: `shown` the account (or venue) it names,
/// `count` the file's rows (rule R7) and `notes` the warnings its digest binds, which its click accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice { pub value: String, pub shown: Label, pub key: Option<i64>, pub new: bool, pub venue: Option<i64>, pub accept: Option<Accept>, pub count: usize, pub notes: Vec<Warning> }

impl Choice {
    fn new(value: String, shown: Label, venue: Option<i64>) -> Self { Self { value, shown, key: None, new: false, venue, accept: None, count: 0, notes: vec![] } }
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
/// `records`: the file's input records (#710 8a rule R7): a Binance export's non-empty records, a KuCoin export's rows, a
/// Deltabadger CSV's entries.
/// `digest`: the file's bytes as read, which a venue export's answer binds (#710 8a review r1).
pub struct Read { entries: Vec<Named>, unrecognised: Vec<String>, rows: Vec<kucoin::SourceRow>, account_id: Option<crate::identity::AccountId>, records: usize, digest: String }

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
    let mut records = 0;
    let (entries, unrecognised): (Vec<Named>, Vec<String>) = match format {
        // D-IMPORT-UID: Binance's own export names its account (`User ID`, or `User_ID`) on every record. It is read
        // over every record before the reader skips any; a missing, blank, masked or mixed id is a malformed export.
        "binance" => match csv::Table::parse(&text).and_then(|table| { records = table.rows.iter().filter(|row| !row.is_empty()).count(); crate::identity::export_account_id(&table, &crate::identity::BINANCE_EXPORT_COLUMNS) })
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
    if format != "binance" { records = entries.len() + rows.len(); }
    Ok(Read { entries, unrecognised, rows, account_id, records, digest: sha(&text) })
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
    // Nothing here signs out: the None arm is never taken.
    Ok(write_signed_in(c, cipher, user_id, format, file, subject, chosen, now, &|_| Ok(true))?.unwrap_or(Answer::Error("unreadable")))
}

/// Why a write stopped before reading anything: the request's sign-in ended (`write_signed_in`).
const SIGNED_OUT: &str = "the sign-in ended before the import was written";

/// `write` for a web request: `signed_in` is asked first inside the write's transaction (web::auth::still_signed_in).
/// None when it says no: nothing is read or written.
#[allow(clippy::too_many_arguments)]
pub fn write_signed_in(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, subject: &str, chosen: Option<&str>, now: DateTime<Utc>,
                       signed_in: &dyn Fn(&Connection) -> Result<bool, SyncError>) -> Result<Option<Answer>, SyncError> {
    let run = || -> Result<Answer, SyncError> {
        let tx = model::immediate(c)?;
        if !signed_in(&tx)? { return Err(SyncError(SIGNED_OUT.into())); }
        let ran = write_in(&tx, cipher, user_id, format, file, subject, &Confirm { chosen, ..Default::default() }, now)?;
        tx.commit()?;
        Ok(ran.answer)
    };
    match run() {
        Err(error) if error.0 == SIGNED_OUT => Ok(None),
        Err(error) => match refusal(&error) {
            Some(reason) => Ok(Some(Answer::Refused(reason))),
            None => Err(error),
        },
        Ok(answer) => Ok(Some(answer)),
    }
}

/// A named Binance identity refusal a write stopped on (`binance_writer::csv_refusal`), else None.
pub fn refusal(error: &SyncError) -> Option<String> { crate::sync::binance_writer::csv_refusal(error).map(str::to_owned) }

/// PLAN-710-confirm rule M2: Import::Run#route and #write inside the caller's transaction (`BEGIN IMMEDIATE`), which it
/// never commits: what a write did stays for the caller to commit with its own bookkeeping; a question, an error or a
/// refusal leaves nothing (rule K6: a question has no side effect, no account or identity key it made for routing).
#[allow(clippy::too_many_arguments)]
pub fn write_in(tx: &rusqlite::Transaction<'_>, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, subject: &str, confirm: &Confirm,
                now: DateTime<Utc>) -> Result<Ran, SyncError> {
    tx.execute_batch("SAVEPOINT import_run")?;
    let floor: i64 = tx.query_row("SELECT coalesce(max(id), 0) FROM exchange_accounts", [], |r| r.get(0)).or_else(|_| Ok::<_, rusqlite::Error>(i64::MAX))?;
    FLOOR.with(|f| f.set(floor));
    let out = run_in(tx, cipher, user_id, format, file, subject, confirm, now);
    FLOOR.with(|f| f.set(i64::MAX));
    match out {
        Ok((ran, true)) => { tx.execute_batch("RELEASE import_run")?; Ok(ran) }
        Ok((ran, false)) => { tx.execute_batch("ROLLBACK TO import_run; RELEASE import_run")?; Ok(ran) }
        Err(error) => { let _ = tx.execute_batch("ROLLBACK TO import_run; RELEASE import_run"); Err(error) }
    }
}

thread_local! {
    /// The highest account id when the running `write_in` began: one above it is an account this run made (a legacy key's
    /// number 1, routing's), which a route seal names by its key and number, never its id (PLAN-710-confirm rules M6/L9).
    static FLOOR: std::cell::Cell<i64> = const { std::cell::Cell::new(i64::MAX) };
}

/// An account as a route seal names it: its id, or a prospective account (one this run made) by key and number.
fn canon(c: &Connection, account: Option<i64>, key: i64) -> Result<String, SyncError> {
    let Some(id) = account else { return Ok(format!("k{key}#1")) };
    if id <= FLOOR.with(std::cell::Cell::get) { return Ok(id.to_string()); }
    let number: i64 = c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0))?;
    Ok(format!("k{key}#{number}"))
}

/// PLAN-710-confirm rules L1/M1: the route a click writes: every destination (its exchange, account, key, slots and rows),
/// the rows no account takes and their names, and the file. A sole account replaced, a sibling made or a key moved
/// changes it.
fn route(c: &Connection, user_id: i64, targets: &[(Target, Vec<Entry>)], kucoin: usize, unrouted: &[String], records: usize, file: &str) -> Result<String, SyncError> {
    let mut parts = vec![];
    for (target, rows) in targets {
        parts.push(match target {
            Target::Key(key) => {
                let account = match key.scope.account { Some(a) => Some(a), None => crate::identity::Account::first(c, user_id, key.exchange_id)?.id };
                let slot = match account { Some(a) => slot(c, user_id, Dest::Account(a))?, None => String::new() };
                format!("key:{}:{}:{}:{slot}:{}", key.exchange_id, canon(c, account, key.id)?, key.id, rows.len() + kucoin)
            }
            Target::New(exchange) => format!("new:{exchange}:{}:{}", slot(c, user_id, Dest::New(*exchange))?, rows.len() + kucoin),
        });
    }
    Ok(sha(&format!("{}|unrouted:{}|records:{records}|file:{file}", parts.join(";"), unrouted.join(","))))
}

/// The answer a choice posts back (rule S5: a key answer carries the venue its question offered it for, `~v<exchange>`;
/// rule N4: its digest rides after it).
pub fn posted(choice: &Choice) -> String {
    let (body, digest) = choice.value.rsplit_once('!').map_or((choice.value.as_str(), None), |(b, d)| (b, Some(d)));
    let body = match choice.venue.filter(|_| body.parse::<i64>().is_ok()) { Some(e) => format!("{body}~v{e}"), None => body.to_string() };
    digest.map_or(body.clone(), |d| format!("{body}!{d}"))
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
/// live accounts (one made meanwhile changes it) and the number it would take (#710 8a rule R9).
pub(crate) fn slot(c: &Connection, user_id: i64, dest: Dest) -> Result<String, SyncError> {
    Ok(match dest {
        Dest::Account(id) => match crate::identity::Account::by_id(c, id)? {
            Some(account) => c.prepare(&format!("SELECT k.id, k.key_type, k.status, coalesce(k.key, ''), coalesce(k.secret, ''), coalesce(k.passphrase, '') FROM api_keys k WHERE {} ORDER BY k.id", account.keys("k")))?
                .query_map([], |r| Ok(format!("{}:{}:{}:{}", r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
                                               sha(&format!("{}|{}|{}", r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))))?
                .collect::<Result<Vec<_>, _>>()?.join(","),
            None => "gone".into(),
        },
        Dest::New(exchange) => {
            let live = crate::identity::accounts_of(c, user_id, exchange)?.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>().join(",");
            if crate::identity::ready(c)? { format!("{live}#{}", crate::identity::next_number(c, user_id, exchange)?) } else { live }
        }
    })
}
/// #710 8a rule R10: a Deltabadger CSV's answer is bound to its whole routing (the evidence), never to a new account's slot.
const CSV_SLOT: &str = "csv";

/// Rule N4: an answer is used once: consumed in the write it allows (a second answer of the same question finds it gone).
pub(crate) fn consume(c: &Connection, digest: &str, now: DateTime<Utc>) -> Result<bool, SyncError> {
    if !crate::engine::rules::table_exists(c, "parked_answers")? { return Ok(true); }
    Ok(c.execute("INSERT INTO parked_answers (digest, created_at) VALUES (?1, ?2) ON CONFLICT (digest) DO NOTHING", rusqlite::params![digest, crate::sync::sql_time(now)])? == 1)
}

/// The rows a destination would take, as the exact-id check reads them (rules P5/Q6).
struct Bound<'a> { entries: &'a [Entry], kucoin: &'a [kucoin::SourceRow] }

/// Rules P5/Q6: of the rows `bound` would write on `dest` (account `own`, or a new one) of `exchange`, those that exist
/// on another live account of it: an entry whose venue id it holds or merged (Alpaca's `merged_activity_ids`), an
/// id-less entry exactly repeating one of its rows (type, currency, amount, UTC second), a KuCoin row it holds. Per such
/// account (#710 8a rule R12): the account, its count, and the ids seen (for the digest).
fn existing(c: &Connection, user_id: i64, exchange: i64, own: Option<i64>, bound: &Bound) -> Result<Vec<(i64, usize, String)>, SyncError> {
    let others: Vec<i64> = crate::identity::accounts_of(c, user_id, exchange)?.into_iter().map(|(id, _)| id).filter(|id| Some(*id) != own).collect();
    let mut out = vec![];
    if others.is_empty() { return Ok(out); }
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange], |r| r.get::<_, Option<String>>(0))?.unwrap_or_default();
    for other in others {
        let (mut count, mut ids) = (0, vec![]);
        let key = writer(c, &[], other)?;
        let merged = ledger::stored_merged(c, &key)?;
        let at_ = "account_transactions";
        // Each id with the account holding it: a Merge that moves it changes what the warning named.
        let mut hit = |id: String| { count += 1; ids.push(format!("{other}:{id}")); };
        for e in bound.entries {
            match e.tx_id.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(id) => if crate::venue_ids::known(c, &key.scope, id)? || merged.contains(id) { hit(id.to_string()) },
                None => {
                    let (Some(at), Some(base)) = (e.transacted_at, e.base_currency.as_deref()) else { continue };
                    let second = at.with_nanosecond(0).unwrap_or(at);
                    let same: bool = c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM account_transactions {} WHERE user_id = ?1 AND exchange_id = ?2 AND {} AND entry_type = ?3 \
                                                          AND base_currency = ?4 AND base_amount = ?5 AND transacted_at >= ?6 AND transacted_at < ?7)", ledger::BY_TIME, key.scope.rows(at_)),
                                                 rusqlite::params![user_id, exchange, e.entry_type, base, ledger::amount_sql(&exchange_type, &e.base_amount),
                                                                   crate::sync::sql_time(second), crate::sync::sql_time(second + chrono::Duration::seconds(1))], |r| r.get(0))?;
                    if same { hit(format!("{}|{base}|{}|{}", e.entry_type, e.base_amount.to_s_f(), second.timestamp())) }
                }
            }
        }
        for row in bound.kucoin {
            let held: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM import_rows WHERE account_id = ?1 AND row_id = ?2)", rusqlite::params![other, row.row_id], |r| r.get(0))?;
            if held { hit(row.row_id.clone()) }
        }
        if count > 0 { out.push((other, count, ids.join(","))); }
    }
    Ok(out)
}

/// What the import sees at `dest` before writing (rules M1, N5, P5, P8, Q6): the warnings, the accounts holding the
/// export's account (offered first), and the evidence the answer's digest binds. `named`: the question offers a new
/// account, so every account shows its number (#710 8a rule R6); the evidence binds it and each label shown (rule R11).
#[allow(clippy::too_many_arguments)]
fn seen(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, dest: Dest, file: Option<&crate::identity::AccountId>, bound: &Bound, named: bool, now: DateTime<Utc>)
        -> Result<(Vec<Warning>, Vec<i64>, String), SyncError> {
    let (exchange, own) = match dest {
        Dest::Account(id) => (crate::identity::Account::by_id(c, id)?.ok_or_else(|| SyncError(format!("import account {id} is gone")))?.exchange_id, Some(id)),
        Dest::New(exchange) => (exchange, None),
    };
    let (mut warnings, mut holders, mut facts, mut shown) = (vec![], vec![], String::new(), vec![]);
    let mut name = |account: i64| -> rusqlite::Result<Label> {
        let label = if named { crate::identity::label_numbered(c, account)? } else { crate::identity::label(c, account)? };
        shown.push((account, label.number));
        Ok(label)
    };
    if let Some(id) = file.filter(|_| crate::identity::ready(c).unwrap_or(false)) {
        let e = crate::identity::file_evidence(c, cipher, user_id, exchange, own, id, now).map_err(SyncError)?;
        if let Some(first) = e.holders.first() { warnings.push(Warning::HeldElsewhere(name(*first)?)); }
        if e.foreign { if let Some(own) = own { warnings.push(Warning::OtherAccount(name(own)?)); } }
        (holders, facts) = (e.holders, e.facts);
    }
    let mut lines = vec![];
    for (on, n, ids) in existing(c, user_id, exchange, own, bound)? {
        lines.push((name(on)?, n));
        facts.push_str(&format!("|ids:{ids}"));
    }
    if !lines.is_empty() { warnings.push(Warning::Existing(lines)); }
    facts.push_str(&format!("|named:{}|shown:{shown:?}", u8::from(named)));
    Ok((warnings, holders, facts))
}

/// The label a question shows for `dest` (#710 8a rule R6): an account's, numbered when `named`; a new account's
/// prospective one.
fn shown(c: &Connection, user_id: i64, dest: Dest, named: bool) -> rusqlite::Result<Label> {
    match dest {
        Dest::Account(id) if named => crate::identity::label_numbered(c, id),
        Dest::Account(id) => crate::identity::label(c, id),
        Dest::New(exchange) => crate::identity::prospective(c, user_id, exchange),
    }
}

fn exchange_of(c: &Connection, dest: Dest) -> Result<i64, SyncError> {
    match dest {
        Dest::Account(id) => Ok(crate::identity::Account::by_id(c, id)?.ok_or_else(|| SyncError(format!("import account {id} is gone")))?.exchange_id),
        Dest::New(exchange) => Ok(exchange),
    }
}

/// #710 8a rule R5: the offer a venue export's question shows on `exchange`, whichever of its buttons is clicked: where the
/// file routes there with no account answer (an account and its number, a new account and the number it would take, or the
/// account question's options), each account holding the export's account with its number, the number a new account
/// would take, and where the file routes with no answer at all (review r1: a venue that became ambiguous since is asked
/// again). Recomputed under the import's lock when a click is validated.
struct Offered { text: String, named: bool, venues: Option<Vec<Choice>> }
#[allow(clippy::too_many_arguments)]
fn offered(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, venues: &[&str], file: Option<&crate::identity::AccountId>, keys: &[Reading], exchange: i64,
           picked: Option<i64>, now: DateTime<Utc>) -> Result<Offered, SyncError> {
    let number = |id: i64| -> rusqlite::Result<i64> { c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)) };
    let summary = |routed: &Routed| -> rusqlite::Result<String> {
        Ok(match routed {
            Routed::To(account) => format!("to:{account}#{}", number(*account)?),
            Routed::New(e) => format!("new:{e}"),
            Routed::Ask(options) => format!("ask:{}", options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>().join(",")),
            Routed::Key(k) => format!("key:{}", k.id),
            Routed::None => "none".into(),
        })
    };
    let here = route_file(c, cipher, user_id, venues, file, Some(&format!("v{exchange}")), keys, now)?;
    let (named, own) = match &here { Routed::New(_) => (true, None), Routed::Ask(options) => (options.iter().any(|o| o.new), None), Routed::To(a) => (false, Some(*a)), _ => (false, None) };
    // Review r1: whether the file settles its venue itself (routing with no answer lands on `exchange`), or its venue was
    // the user's answer. A venue the file settled that is in question by the click is asked again; one the user answered
    // stays theirs.
    let anywhere = route_file(c, cipher, user_id, venues, file, None, keys, now)?;
    // PLAN-710-confirm rule L3: a venue the user picked is theirs (persisted with the upload), whatever routing says now.
    let settled = picked != Some(exchange) && match &anywhere {
        Routed::To(account) => crate::identity::Account::by_id(c, *account)?.is_some_and(|a| a.exchange_id == exchange),
        Routed::New(e) => *e == exchange,
        Routed::Ask(options) => options.iter().any(|o| o.new && o.venue == Some(exchange)),
        _ => false,
    };
    let mut text = format!("{}|venue:{}", summary(&here)?, if settled { "settled" } else { "answered" });
    let ready = crate::identity::ready(c)?;
    if ready { text.push_str(&format!("|next:{}", crate::identity::next_number(c, user_id, exchange)?)); }
    if let Some(id) = file.filter(|_| ready) {
        for holder in crate::identity::file_evidence(c, cipher, user_id, exchange, own, id, now).map_err(SyncError)?.holders { text.push_str(&format!("|holder:{holder}#{}", number(holder)?)); }
    }
    let venues = match anywhere { Routed::Ask(options) if !options.iter().any(|o| o.new) => Some(options), _ => None };
    Ok(Offered { text, named, venues })
}

/// Where rows of one routed group go: a key's (or a keyless account's) writer, or a new account of an exchange.
#[derive(Clone)]
enum Target { Key(Key), New(i64) }

/// What a question asked before writing offers (rule N4): a venue export's destination and the accounts holding the
/// export's account, with the offer as shown (`offered`, #710 8a rule R5), whether it numbers every account and the file's
/// rows; or a Deltabadger CSV's whole routing (`rest`: some of its rows have no account here) and its routed rows.
/// `venues`: the venue question the file would get with no answer, asked instead when the answer went stale (review r1).
enum Offer { Native { dest: Dest, holders: Vec<i64>, offer: String, named: bool, count: usize, venues: Option<Vec<Choice>> }, Csv { rest: bool, count: usize }, Unbound }

/// The routing of a file, before anything is written. `confirm`: asked before writing even with nothing to warn about
/// (#710 8a rules R1/R2: the destination is one of an exchange's several accounts, or a new one beside a live one).
/// `destinations`: a Deltabadger CSV's accounts and their rows (PLAN-710-confirm rule L2), shown whenever it asks.
struct Plan { targets: Vec<(Target, Vec<Entry>)>, unrouted: Vec<String>, warnings: Vec<Warning>, evidence: String, changed: bool, confirm: bool, offer: Offer, destinations: Vec<(Label, usize)> }

/// The origin each importer's rows are recorded under (rule N8).
fn origin(format: &str) -> &'static str { match format { "binance" => "binance", "kucoin" => "kucoin", _ => "deltabadger" } }

/// `write_in`'s work, inside its savepoint: the answer, what a question issued, and whether what it did is kept (only a
/// write is).
#[allow(clippy::too_many_arguments)]
fn run_in(tx: &rusqlite::Transaction<'_>, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, file: Read, subject: &str, confirm: &Confirm, now: DateTime<Utc>)
          -> Result<(Ran, bool), SyncError> {
    let Read { entries, unrecognised, rows: kucoin_rows, account_id, records, digest: file_digest } = file;
    let keys = reading(tx, user_id)?;
    let (body, tag) = match confirm.chosen { Some(chosen) => { let (body, tag) = split_answer(chosen); (Some(body), tag) } None => (None, None) };
    let accepting = matches!(tag, Some(('w', _)));
    // A venue's own export goes to that venue's account.
    let venues: &[&str] = match format { "binance" => &[BINANCE, BINANCE_US], "kucoin" => &[KUCOIN], _ => &[] };
    let native: Vec<Entry> = if venues.is_empty() { vec![] } else { entries.iter().map(|(_, e)| e.clone()).collect() };
    // Rules M1/N1: each venue option of a venue question, as its click would write: `Authorize` (one destination, nothing to
    // warn about) bound to that route, else `Select`. An account question's options are 8a's sealed picks.
    let issue = |answer: &Answer| -> Result<Vec<Issued>, SyncError> {
        let Answer::Ambiguous(options) = answer else { return Ok(vec![]) };
        let mut out = vec![];
        for o in options {
            if o.new || o.value.contains("!p") { out.push(Issued { kind: Kind::Accept, route: None, outside: None }); continue }
            tx.execute_batch("SAVEPOINT import_click")?;
            let click = native_plan(tx, cipher, user_id, venues, account_id.as_ref(), &keys, native.clone(), &kucoin_rows, (records, &file_digest), subject, Some(&posted(o)), None, confirm.picked, now)
                .and_then(|plan| match plan {
                    Ok(p) if !p.changed && !p.confirm && p.warnings.is_empty() => Ok(Some(route(tx, user_id, &p.targets, kucoin_rows.len(), &p.unrouted, records, &file_digest)?)),
                    _ => Ok(None),
                });
            tx.execute_batch("ROLLBACK TO import_click; RELEASE import_click")?;
            out.push(match click? { Some(route) => Issued { kind: Kind::Authorize, route: Some(route), outside: None }, None => Issued { kind: Kind::Select, route: None, outside: None } });
        }
        Ok(out)
    };
    let plan = if venues.is_empty() { csv_plan(tx, user_id, &keys, entries, body, tag)? } else {
        match native_plan(tx, cipher, user_id, venues, account_id.as_ref(), &keys, native.clone(), &kucoin_rows, (records, &file_digest), subject, body, tag, confirm.picked, now)? {
            Ok(plan) => plan,
            Err(answer) => { let issued = issue(&answer)?; return Ok((Ran { answer, issued }, false)) }
        }
    };
    let Plan { targets, unrouted, warnings, evidence, mut changed, confirm: mut asks, offer, destinations } = plan;
    changed |= confirm.stale;
    asks |= confirm.select;
    // Rule M3: the route a click was issued for is checked before any writer runs: a stale one is asked again, never refused.
    if let Some(sealed) = confirm.route { if sealed != route(tx, user_id, &targets, kucoin_rows.len(), &unrouted, records, &file_digest)? { changed = true; } }
    let bound_entries: Vec<Entry> = targets.iter().flat_map(|(_, rows)| rows.iter().cloned()).collect();
    // A question: what it shows (rule L2: a Deltabadger CSV's destinations; rule M4: the links it undoes elsewhere), its
    // choices, and what each was issued for (the destination's, last, binds the links shown).
    let ask = |c: &Connection, mut shown: Vec<Warning>, evidence: &str, changed: bool, undone: Option<&Undone>| -> Result<Ran, SyncError> {
        // Review r1: a stale answer on a file whose venue is now in question asks the venue again (rule L3: unless the user
        // picked that venue).
        if let (true, Offer::Native { venues: Some(venues), dest, .. }) = (changed, &offer) {
            if confirm.picked.is_none_or(|v| exchange_of(c, *dest).ok() != Some(v)) { let answer = Answer::Ambiguous(venues.clone()); return Ok(Ran { issued: issue(&answer)?, answer }); }
        }
        if changed { shown.insert(0, Warning::Changed); }
        if matches!(offer, Offer::Csv { .. }) && !shown.iter().any(|w| matches!(w, Warning::Destination(_))) {
            shown.insert(usize::from(changed), Warning::Destination(destinations.clone()));
        }
        if let Some(u) = undone.filter(|u| !u.rows.is_empty()) { shown.push(Warning::Undone(u.rows.len(), u.accounts.clone())); }
        let bound = Bound { entries: &bound_entries, kucoin: &kucoin_rows };
        let choices = choices(c, cipher, user_id, subject, &offer, account_id.as_ref(), &bound, &shown, evidence, now)?;
        let last = choices.len().saturating_sub(1);
        let issued = (0..choices.len()).map(|i| Issued { kind: Kind::Accept, route: None, outside: (i == last).then(|| undone.map(|u| u.digest.clone())).flatten() }).collect();
        Ok(Ran { answer: Answer::Warned(shown, choices), issued })
    };
    // What writing would undo on other accounts, written and rolled back (a question shows and binds it).
    let probe = || -> Result<Option<Undone>, SyncError> {
        tx.execute_batch("SAVEPOINT import_probe")?;
        let watched = watch(tx)?;
        let written = write_targets(tx, cipher, user_id, format, targets.clone(), &kucoin_rows, account_id.as_ref(), unrecognised.clone(), unrouted.clone(), false, now);
        let out = match (&written, watched) { (Ok((Answer::Written { destinations, .. }, _)), true) => Some(undone(tx, user_id, destinations)?), _ => None };
        tx.execute_batch("ROLLBACK TO import_probe; RELEASE import_probe")?;
        Ok(out)
    };
    // Rule N4 / M1: asked (again) before writing anything. #710 8a rule R4: a pick whose digest still holds accepts what its
    // button showed; it is never asked again for it.
    let picked = matches!(tag, Some(('p', _)));
    if changed || (!accepting && !picked && (asks || !warnings.is_empty())) { let undone = probe()?; return Ok((ask(tx, warnings, &evidence, changed, undone.as_ref())?, false)); }
    // The destination's slots as the answer saw them, before this write adds to them.
    let before = match &offer { Offer::Native { dest, .. } => Some((*dest, slot(tx, user_id, *dest)?)), Offer::Csv { .. } => Some((Dest::New(0), CSV_SLOT.to_string())), Offer::Unbound => None };
    // A question found while writing is asked over the state before the write (`undo`).
    tx.execute_batch("SAVEPOINT import_write")?;
    let undo = |tx: &rusqlite::Transaction<'_>| tx.execute_batch("ROLLBACK TO import_write; RELEASE import_write");
    let watched = watch(tx)?;
    let written = write_targets(tx, cipher, user_id, format, targets, &kucoin_rows, account_id.as_ref(), unrecognised, unrouted, accepting, now);
    let (answer, stripped) = match written {
        Ok(written) => written,
        Err(error) => match crate::sync::binance_writer::warned(&error) {
            // Rules P6/Q3: Binance rows the writer warns about; nothing kept, asked with them.
            Some(units) => {
                undo(tx)?;
                let mut shown = warnings;
                shown.extend(unit_warnings(&units));
                return Ok((ask(tx, shown, &format!("{evidence}|retained:{units:?}"), false, None)?, false));
            }
            None => return Err(error),
        },
    };
    let undone = match (&answer, watched) { (Answer::Written { destinations, .. }, true) => Some(undone(tx, user_id, destinations)?), _ => None };
    unwatch(tx)?;
    let unseen = evidence.clone();
    let evidence = if stripped.is_empty() { evidence } else { format!("{evidence}|retained:{stripped:?}") };
    if let Some((purpose, posted)) = tag {
        // An "import anyway" holds only while everything it was given for does; any answer counts once.
        let stale = purpose == 'w' && before.as_ref().is_none_or(|(dest, slot)| posted != seal('w', subject, *dest, slot, &evidence));
        // Review r1: an answer that held for all it showed, the writer then finding retained ids, is asked about them, not
        // told something changed.
        let first_seen = stale && !stripped.is_empty() && before.as_ref().is_some_and(|(dest, slot)| posted == seal('w', subject, *dest, slot, &unseen));
        if stale || !consume(tx, posted, now)? {
            undo(tx)?;
            let mut shown = warnings;
            shown.extend(unit_warnings(&stripped));
            return Ok((ask(tx, shown, &evidence, !first_seen, None)?, false));
        }
    }
    // Rules M4/Q2: a write that undoes transfer links on other accounts writes only past a question that showed exactly
    // those (sealed with the click); else it is asked, the links still in place.
    if let Some(u) = &undone {
        if (confirm.outside.is_some() || !u.rows.is_empty()) && confirm.outside != Some(u.digest.as_str()) {
            undo(tx)?;
            return Ok((ask(tx, warnings, &evidence, confirm.outside.is_some(), Some(u))?, false));
        }
    }
    // Rule N4: a parked upload is written once, whichever of its answers comes first; a later one writes nothing.
    if !subject.is_empty() && !consume(tx, &sha(&format!("u|{subject}")), now)? {
        return Ok((Ran { answer: Answer::Written { imported: 0, duplicates: 0, skipped: 0, unrecognised: vec![], unrouted: vec![], generic: 0, destinations: vec![] }, issued: vec![] }, false));
    }
    tx.execute_batch("RELEASE import_write")?;
    Ok((Ran { answer, issued: vec![] }, true))
}

/// What a write undid on other accounts (rules M4/Q1/P2): each removed transfer link (a row this write deleted and its
/// partner, or a row whose link it cleared and the row it pointed to) whose surviving end is the owner's on another account
/// than the end that went; those accounts; and the digest a question binds (both ends' ids, contents, owners, exchanges
/// and accounts, resolved through membership).
#[derive(Debug, Clone)]
struct Undone { rows: Vec<i64>, accounts: Vec<Label>, digest: String }

/// Arms the watch on ledger rows for the rest of the caller's savepoint: each row deleted (with its link partner) and each
/// link cleared, as they were. False on an install without the membership tables (nothing to resolve accounts by).
fn watch(c: &Connection) -> Result<bool, SyncError> {
    if !crate::identity::ready(c)? || !crate::venue_ids::ready(c)? { return Ok(false); }
    let row = |r: &str| format!("'{r}', {r}.id, {r}.user_id, {r}.exchange_id, (SELECT m.account_id FROM exchange_account_rows m WHERE m.account_transaction_id = {r}.id), \
        json_array({r}.entry_type, {r}.base_currency, {r}.base_amount, {r}.quote_currency, {r}.quote_amount, {r}.fee_currency, {r}.fee_amount, {r}.transacted_at, {r}.tx_id, \
        {r}.linked_transaction_id, {r}.group_id, {r}.raw_data, {r}.manual_values, {r}.description), {r}.linked_transaction_id");
    c.execute_batch(&format!("CREATE TEMP TABLE IF NOT EXISTS import_touched (kind TEXT, id INTEGER, user_id INTEGER, exchange_id INTEGER, member INTEGER, contents TEXT, linked INTEGER);
        DELETE FROM temp.import_touched;
        CREATE TEMP TRIGGER IF NOT EXISTS import_touched_delete BEFORE DELETE ON main.account_transactions BEGIN
          INSERT INTO import_touched SELECT {};
          INSERT INTO import_touched SELECT {} FROM main.account_transactions partner WHERE partner.id = OLD.linked_transaction_id;
        END;
        CREATE TEMP TRIGGER IF NOT EXISTS import_touched_unlink BEFORE UPDATE OF linked_transaction_id ON main.account_transactions
          WHEN OLD.linked_transaction_id IS NOT NULL AND NEW.linked_transaction_id IS NOT OLD.linked_transaction_id BEGIN
          INSERT INTO import_touched SELECT {};
        END;", row("OLD").replacen("'OLD'", "'deleted'", 1), row("partner"), row("OLD").replacen("'OLD'", "'cleared'", 1)))?;
    Ok(true)
}

fn unwatch(c: &Connection) -> Result<(), SyncError> {
    c.execute_batch("DROP TRIGGER IF EXISTS temp.import_touched_delete; DROP TRIGGER IF EXISTS temp.import_touched_unlink; DROP TABLE IF EXISTS temp.import_touched;")?;
    Ok(())
}

/// The links the watched write undid on other accounts (`Undone`).
fn undone(c: &Connection, owner: i64, _destinations: &[(i64, i64, usize)]) -> Result<Undone, SyncError> {
    type End = (i64, i64, i64, Option<i64>, String, Option<i64>);
    let touched: Vec<(String, End)> = c.prepare("SELECT kind, id, user_id, exchange_id, member, contents, linked FROM temp.import_touched ORDER BY rowid")?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))))?.collect::<Result<_, _>>()?;
    let gone: std::collections::BTreeSet<i64> = touched.iter().filter(|(k, _)| k == "deleted").map(|(_, e)| e.0).collect();
    // Each end as it first was (before this write changed it).
    let mut first: std::collections::BTreeMap<i64, End> = Default::default();
    for (_, e) in &touched { first.entry(e.0).or_insert_with(|| e.clone()); }
    let account = |e: &End| -> Result<Option<i64>, SyncError> {
        Ok(match e.3 { Some(a) => Some(a), None => crate::identity::Account::first(c, e.1, e.2)?.id })
    };
    let mut pairs: std::collections::BTreeSet<(i64, i64)> = Default::default();
    for (kind, e) in &touched {
        match (kind.as_str(), e.5) {
            // A row deleted and the partner it pointed to; a row unlinked and the row it pointed to.
            ("deleted", Some(p)) if !gone.contains(&p) && first.contains_key(&p) => { pairs.insert((e.0, p)); }
            ("cleared", Some(p)) if !gone.contains(&e.0) => { pairs.insert((p, e.0)); }
            _ => {}
        }
    }
    let (mut rows, mut accounts, mut sealed) = (vec![], Vec::<i64>::new(), vec![]);
    for (went, stays) in pairs {
        let (Some(w), Some(s)) = (first.get(&went), first.get(&stays)) else { continue };
        let (wa, sa) = (account(w)?, account(s)?);
        // Rule P3: only the owner's rows; one on the same account as the end that went is that account's own.
        if s.1 != owner || sa == wa { continue }
        rows.push(stays);
        if let Some(a) = sa { if !accounts.contains(&a) { accounts.push(a); } }
        sealed.push(format!("{}:{}:{}:{}:{wa:?}|{}:{}:{}:{}:{sa:?}", w.0, w.4, w.1, w.2, s.0, s.4, s.1, s.2));
    }
    accounts.sort();
    let accounts = accounts.into_iter().map(|a| crate::identity::label(c, a)).collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Undone { rows, accounts, digest: sha(&format!("undone|{}", sealed.join(";"))) })
}

/// The warnings of the units the Binance writer warned about or stripped: retained ids, and (D-BINANCE-DUST-WARNS) dust
/// units, spelled `…:dust`.
fn unit_warnings(units: &[String]) -> Vec<Warning> {
    let dust = units.iter().filter(|u| u.ends_with(crate::sync::binance_writer::DUST_UNIT)).count();
    [(units.len() > dust).then_some(Warning::Retained), (dust > 0).then_some(Warning::Dust)].into_iter().flatten().collect()
}

/// The answers of a question before writing (rule M1): each account holding the export's account first ("import to"),
/// then the destination (an account, or a new account), each named with the file's rows and the warnings its digest binds
/// (#710 8a rule R3); a Deltabadger CSV's one sealed answer ("import", "import anyway", "import the rest"). Cancel is the
/// page's. `warnings`: what the question shows, the destination's own warnings among them.
#[allow(clippy::too_many_arguments)]
fn choices(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, subject: &str, offer: &Offer, file: Option<&crate::identity::AccountId>, bound: &Bound,
           warnings: &[Warning], evidence: &str, now: DateTime<Utc>) -> Result<Vec<Choice>, SyncError> {
    let choice = |value: String, shown: Label, venue: Option<i64>, accept: Accept, count: usize, notes: Vec<Warning>| Choice { value, shown, key: None, new: false, venue, accept: Some(accept), count, notes };
    // PLAN-710-confirm rule K6: an account this run made (a legacy key's number 1) is gone once the question rolls back, so
    // its answer names the key that routed there; the click routes again and its digest binds the account that makes.
    let body = |id: i64| -> Result<String, SyncError> {
        if id > FLOOR.with(std::cell::Cell::get) {
            if let Some(account) = crate::identity::Account::by_id(c, id)? {
                let key: Option<i64> = c.query_row(&format!("SELECT min(k.id) FROM api_keys k WHERE {}", account.keys("k")), [], |r| r.get(0))?;
                if let Some(key) = key { return Ok(key.to_string()); }
            }
        }
        Ok(format!("a{id}"))
    };
    Ok(match offer {
        Offer::Native { dest, holders, offer, named, count, .. } => {
            let mut out = vec![];
            for holder in holders {
                let at = Dest::Account(*holder);
                let (notes, _, facts) = seen(c, cipher, user_id, at, file, bound, *named, now)?;
                let venue = crate::identity::Account::by_id(c, *holder)?.map(|a| a.exchange_id);
                out.push(choice(format!("{}!w{}", body(*holder)?, digest(c, user_id, 'w', subject, at, &format!("{facts}|offer:{offer}"))?), shown(c, user_id, at, *named)?, venue, Accept::To, *count, notes));
            }
            let d = digest(c, user_id, 'w', subject, *dest, evidence)?;
            let notes: Vec<Warning> = warnings.iter().filter(|w| !matches!(w, Warning::Changed | Warning::Retained | Warning::Dust | Warning::Undone(..))).cloned().collect();
            let label = shown(c, user_id, *dest, *named)?;
            out.push(match dest {
                Dest::Account(id) => choice(format!("{}!w{d}", body(*id)?), label, crate::identity::Account::by_id(c, *id)?.map(|a| a.exchange_id), Accept::Here, *count, notes),
                Dest::New(exchange) => choice(format!("n{exchange}!w{d}"), label, Some(*exchange), Accept::New, *count, notes),
            });
            out
        }
        Offer::Csv { rest, count } => {
            let accept = if *rest { Accept::Rest } else if warnings.iter().all(|w| matches!(w, Warning::Destination(_) | Warning::Changed | Warning::Undone(..))) { Accept::Import } else { Accept::Anyway };
            vec![choice(format!("r!w{}", seal('w', subject, Dest::New(0), CSV_SLOT, evidence)), Label::bare(""), None, accept, *count, vec![])]
        }
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
               kucoin_rows: &[kucoin::SourceRow], (records, file_digest): (usize, &str), subject: &str, body: Option<&str>, tag: Option<(char, &str)>, picked: Option<i64>,
               now: DateTime<Utc>) -> Result<Result<Plan, Answer>, SyncError> {
    // Review r1: every answer binds the file it counted.
    let sealed = |o: &Offered| format!("{}|file:{file_digest}|records:{records}", o.text);
    let bound = Bound { entries: &entries, kucoin: kucoin_rows };
    // An "import anyway" names its destination itself; it holds while its digest does (`write_transaction`).
    // An "import anyway" for an account its question's own run made names that account's key (rule K6): routed again.
    let keyed = body.is_some_and(|b| !b.is_empty() && b.bytes().all(|x| x.is_ascii_digit()));
    let accepted = match (tag, body) { (Some(('w', _)), Some(body)) if !keyed => accepted_dest(c, user_id, venues, body)?, _ => None };
    let mut changed = tag.is_some_and(|(t, _)| t == 'w') && accepted.is_none() && !keyed;
    let routing = if tag.is_some_and(|(t, _)| t == 'w') && !keyed { None } else { body };
    let dest = match accepted {
        Some(dest) => dest,
        None => match route_file(c, cipher, user_id, venues, file, routing, keys, now)? {
            Routed::To(account) => Dest::Account(account),
            Routed::New(exchange) => Dest::New(exchange),
            Routed::Key(k) => {
                let key = Key { id: k.id, user_id, exchange_id: k.exchange_id, status: 1, key_type: k.key_type, last_synced_at: None, last_sync_error: None,
                                exchange_type: k.exchange_type, scope: crate::sync::key_scope(c, k.id)? };
                return Ok(Ok(Plan { targets: vec![(Target::Key(key), entries)], unrouted: vec![], warnings: vec![], evidence: String::new(), changed: false, confirm: false, offer: Offer::Unbound, destinations: vec![] }));
            }
            Routed::None => return Ok(Err(Answer::Error("no_account_for_format"))),
            // Rule N4: each account the question offers is bound to what its destination holds now and to the offer as
            // shown (#710 8a rules R3/R5), and named with the file's rows and that destination's warnings, which its click
            // accepts (rule R4). The venue question's key answer stays 2.x's (a key id): it holds only while its venue's one
            // account and key do (`route_file`).
            Routed::Ask(mut options) => {
                for o in options.iter_mut().filter(|o| o.key.is_none()) {
                    let Some(dest) = named_dest(c, user_id, keys, &o.value)? else { continue };
                    // Review r1: numbered as this question is built, a new account among its options.
                    let offer = sealed(&offered(c, cipher, user_id, venues, file, keys, exchange_of(c, dest)?, picked, now)?);
                    let (notes, _, facts) = seen(c, cipher, user_id, dest, file, &bound, true, now)?;
                    o.value = format!("{}!p{}", o.value, digest(c, user_id, 'p', subject, dest, &format!("{facts}|offer:{offer}"))?);
                    (o.shown, o.count, o.notes) = (shown(c, user_id, dest, true)?, records, notes);
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
    let exchange = exchange_of(c, dest)?;
    let o = offered(c, cipher, user_id, venues, file, keys, exchange, picked, now)?;
    let (offer, named) = (sealed(&o), o.named);
    let (warnings, holders, facts) = seen(c, cipher, user_id, dest, file, &bound, named, now)?;
    let evidence = format!("{facts}|offer:{offer}");
    // A pick is checked as the account question rendered it: every account numbered (a new one is among its options).
    if let Some(('p', posted)) = tag {
        let picked = if named { evidence.clone() } else { format!("{}|offer:{offer}", seen(c, cipher, user_id, dest, file, &bound, true, now)?.2) };
        if posted != digest(c, user_id, 'p', subject, dest, &picked)? { changed = true; }
    }
    // #710 8a rule R1: one of an exchange's several accounts, or a new account beside a live one, is named before writing.
    let live = crate::identity::accounts_of(c, user_id, exchange)?.len();
    let confirm = tag.is_none() && match dest { Dest::Account(_) => live >= 2, Dest::New(_) => live >= 1 };
    let target = match dest { Dest::Account(account) => Target::Key(writer(c, keys, account)?), Dest::New(exchange) => Target::New(exchange) };
    Ok(Ok(Plan { targets: vec![(target, entries)], unrouted: vec![], warnings, evidence, changed, confirm, offer: Offer::Native { dest, holders, offer, named, count: records, venues: o.venues }, destinations: vec![] }))
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
    // #710 8a rule R15: an unrouted row naming account 2+ is named as an account is, by its exchange's name; the CSV's
    // own spelling when this install has no such exchange.
    let names: Vec<(String, String)> = c.prepare("SELECT type, name FROM exchanges ORDER BY id")?
        .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default())))?.collect::<Result<_, _>>()?;
    let by_name = |name: &str, number: u32| numbered.iter().rev().find(|(k, n)| crate::tracker::rows::name_id(&k.exchange_type) == name && *n == i64::from(number)).map(|(k, _)| k);
    let mut left_out = 0;
    for (venue, entry) in entries {
        match venue.as_ref().and_then(|(name, number)| by_name(name, *number)) {
            Some(key) => match routed.iter_mut().find(|(k, _)| (k.exchange_id, k.scope.account, k.id) == (key.exchange_id, key.scope.account, key.id)) { Some((_, rows)) => rows.push(entry), None => routed.push((key.clone(), vec![entry])) },
            // Rule N6: a row naming no exchange has no account here either: counted, left out.
            None => {
                left_out += 1;
                if let Some((venue, number)) = venue {
                    // A number-1 row keeps the CSV's spelling, as 2.x (and every recorded import) names it.
                    let venue = if number == 1 { venue } else {
                        format!("{} {number}", names.iter().find(|(t, _)| crate::tracker::rows::name_id(t) == venue).map_or(venue, |(_, name)| name.clone()))
                    };
                    if !unrouted.contains(&venue) { unrouted.push(venue); }
                }
            }
        }
    }
    unrouted.sort();
    // Rule N6: rows naming accounts this install lacks are said before writing. Rules P5/Q6: rows another live account
    // holds (#710 8a rule R12: per account), and the venue ids this upload sends to two or more accounts (rule R13). #710 8a
    // rule R2: where the rows go, when an exchange of them has several accounts. The digest binds every group's account,
    // key and slots, each id's destinations, and each label shown (rule R11).
    let mut evidence = format!("unrouted:{left_out}:{};", unrouted.join(","));
    let (mut confirm, mut destinations, mut held, mut shown) = (false, vec![], Vec::<(i64, usize)>::new(), vec![]);
    let mut sent: std::collections::BTreeMap<(i64, String), std::collections::BTreeSet<String>> = Default::default();
    for (key, rows) in &routed {
        let own = match key.scope.account { Some(a) => Some(a), None => crate::identity::accounts_of(c, user_id, key.exchange_id)?.first().map(|(a, _)| *a) };
        let slot = match own { Some(a) => digest(c, user_id, 's', "", Dest::Account(a), "")?, None => String::new() };
        evidence.push_str(&format!("{}:{}:{:?}:{}:{slot};", key.id, key.exchange_id, own, rows.len()));
        let dest = own.map_or_else(|| format!("k{}", key.id), |a| format!("a{a}"));
        for id in rows.iter().filter_map(|e| e.tx_id.as_deref()).filter(|t| !t.trim().is_empty()) { sent.entry((key.exchange_id, id.to_string())).or_default().insert(dest.clone()); }
        for (on, n, ids) in existing(c, user_id, key.exchange_id, own, &Bound { entries: rows, kucoin: &[] })? {
            evidence.push_str(&format!("ids:{ids};"));
            match held.iter_mut().find(|(a, _)| *a == on) { Some((_, m)) => *m += n, None => held.push((on, n)) }
        }
        confirm |= crate::identity::accounts_of(c, user_id, key.exchange_id)?.len() >= 2;
        // Review r1: a legacy key's account has no row yet: it is its exchange's only one, named by the exchange.
        let label = match own { Some(a) => crate::identity::label(c, a)?, None => Label::bare(c.query_row("SELECT name FROM exchanges WHERE id = ?1", [key.exchange_id], |r| r.get::<_, Option<String>>(0))?.unwrap_or_default()) };
        shown.push((own, label.number));
        destinations.push((label, rows.len()));
    }
    let twice: Vec<_> = sent.iter().filter(|(_, to)| to.len() >= 2).collect();
    for ((exchange, id), to) in &twice { evidence.push_str(&format!("twice:{exchange}:{id}:{};", to.iter().cloned().collect::<Vec<_>>().join(","))); }
    let mut warnings = vec![];
    if confirm { warnings.push(Warning::Destination(destinations.clone())); }
    if !twice.is_empty() { warnings.push(Warning::Twice(twice.len())); }
    if left_out > 0 { warnings.push(Warning::Unrouted(left_out)); }
    // PLAN-710-confirm rule L2: the accounts the left-out rows name, by name.
    if !unrouted.is_empty() { warnings.push(Warning::LeftOut(unrouted.clone())); }
    held.sort();
    if !held.is_empty() {
        let mut lines = vec![];
        for (on, n) in held { let label = crate::identity::label(c, on)?; shown.push((Some(on), label.number)); lines.push((label, n)); }
        warnings.push(Warning::Existing(lines));
    }
    evidence.push_str(&format!("named:0|shown:{shown:?}"));
    // Only "import anyway" (`r`) answers a Deltabadger CSV's warning; anything else posted is asked again.
    let changed = tag.is_some() && (body != Some("r") || tag.is_some_and(|(t, _)| t != 'w'));
    let count = routed.iter().map(|(_, rows)| rows.len()).sum();
    Ok(Plan { targets: routed.into_iter().map(|(k, rows)| (Target::Key(k), rows)).collect(), unrouted, warnings, evidence, changed, confirm, offer: Offer::Csv { rest: left_out > 0, count }, destinations })
}

/// #write: one transaction across every account the file touches. Fenced on no credential: an import reads none (the
/// venue's API is never called), so no credential change can make what it writes stale. `accept`: the Binance rows the
/// writer would warn about are written plain (rules P6/Q3); returns the answer and those it stripped (`dest:unit`, #710 8a
/// rule R14: `a<account>`, or `n<exchange>` for an account this write makes).
#[allow(clippy::too_many_arguments)]
fn write_targets(tx: &rusqlite::Transaction<'_>, cipher: &crate::crypto::Cipher, user_id: i64, format: &str, targets: Vec<(Target, Vec<Entry>)>, kucoin_rows: &[kucoin::SourceRow],
                 account_id: Option<&crate::identity::AccountId>, unrecognised: Vec<String>, unrouted: Vec<String>, accept: bool, now: DateTime<Utc>) -> Result<(Answer, Vec<String>), SyncError> {
    let fenced = model::fence_versions(tx, &[])?;
    let (mut imported, mut duplicates, mut skipped, mut generic) = (0, 0, 0, 0);
    let mut stripped = vec![];
    let mut destinations: Vec<(i64, i64, usize)> = vec![];
    let origin = origin(format);
    for (target, rows) in targets {
        let before = imported;
        let (key, unit) = match target {
            Target::Key(key) => { let unit = key.scope.account.map_or_else(|| format!("k{}", key.id), |a| format!("a{a}")); (key, unit) }
            Target::New(exchange) => {
                let account = crate::identity::new_account(&fenced, user_id, exchange, now)?;
                // Rule N7: a second account can disqualify a reader with no UID read; its generation moves now. A keyless
                // account makes no reader, so nothing is woken.
                crate::sync::settle_readers(&fenced, user_id, exchange)?;
                (writer(&fenced, &[], account)?, format!("n{exchange}"))
            }
        };
        let (routed_account, exchange_id) = (key.scope.account, key.exchange_id);
        if format == "kucoin" {
            // #710 slice 7: the source rows are stored by (user, exchange), whatever account the file was routed to; routing
            // imports by account is slice 7's.
            let out = kucoin::store(&fenced, &key, kucoin_rows, now)?;
            (imported, duplicates, skipped, generic) = (imported + out.imported, duplicates + out.duplicates, skipped + out.skipped, generic + out.generic);
        } else {
            let out = if [BINANCE, BINANCE_US].contains(&key.exchange_type.as_str()) {
                let (out, units) = crate::sync::binance_writer::store_csv(&fenced, cipher, &key, &rows, &unit, origin, accept, now)?;
                stripped.extend(units);
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
        let account = match routed_account { Some(a) => Some(a), None => crate::identity::Account::first(&fenced, user_id, exchange_id).ok().and_then(|a| a.id) };
        if let Some(account) = account { destinations.push((exchange_id, account, imported - before)); }
    }
    drop(fenced);
    Ok((Answer::Written { imported, duplicates, skipped, unrecognised, unrouted, generic, destinations }, stripped))
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
            (None, _) => Routed::Ask(options.iter().map(|k| Choice { key: Some(k.id), ..Choice::new(k.id.to_string(), Label::bare(k.exchange_name.clone()), Some(k.exchange_id)) }).collect()),
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
    // PLAN-710-confirm rule N4: an identity unknown here (its key unreadable) routes by no UID, and the account is asked.
    let identity = crate::identity::import_key(c, cipher, now).map_err(SyncError)?;
    let fingerprint = |exchange_type: &str| identity.as_ref().and_then(|key| file.map(|id| key.fingerprint(exchange_type, id)));
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
            choices.push(Choice { key, ..Choice::new(key.map_or_else(|| format!("v{id}"), |k| k.to_string()), Label::bare(name.clone()), Some(*id)) });
        }
        Ok(Routed::Ask(choices))
    };
    // The account question's candidates for a venue (its unfingerprinted live accounts), and the set an answer is bound to.
    // PLAN-710-confirm rule N4: with the identity unknown, no fingerprint can be compared: every live account is one.
    let owners = |venue: i64| -> rusqlite::Result<Vec<i64>> { Ok(live(venue)?.into_iter().filter(|(_, _, held)| held.is_none() || identity.is_none()).map(|(id, _, _)| id).collect()) };
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
                    if current && !removed && (held.is_none() || identity.is_none() || held == fingerprint(exchange_type)) { parked = Some(Ok(account)) } else { stale = true }
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
    if file.is_some() && identity.is_none() { stale = true; }
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
            // Plan L8, as main: the one reading key of the format's venues together, its venue's one account, takes a clean
            // upload, unless another of them already observed the file's UID (rules G1/J2: then the venue is asked).
            let readers: Vec<&Reading> = keys.iter().filter(|k| exchanges.iter().any(|(id, _, _)| *id == k.exchange_id)).collect();
            if let [k] = readers.as_slice().iter().filter(|k| readers.len() == 1 && venue_key(k.exchange_id).ok().flatten() == Some(k.id)).collect::<Vec<_>>().as_slice() {
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
            let mut choices = many.iter().map(|id| Ok(Choice::new(format!("a{id}~{set}"), crate::identity::label(c, *id)?, Some(venue)))).collect::<rusqlite::Result<Vec<_>>>()?;
            choices.push(Choice { new: true, ..Choice::new(format!("n{venue}~{set}"), Label::bare(""), Some(venue)) });
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
