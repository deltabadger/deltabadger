//! The tracker's background syncs for an Alpaca key, as Rails' jobs run them: the account-transaction (ledger) sync
//! (AccountTransaction::SyncJob) and the account-balance sync (AccountBalance::SyncJob). Each is an idempotent async
//! function, safe to call at any time; when to call them is the scheduler's business.
//!
//! The write rule (these jobs write the SQLite file the engine trades on):
//! - every database phase runs off the runtime thread (`jobs::Db::run`), in short `BEGIN IMMEDIATE` transactions
//!   (`write`), with `WRITE_GAP` after each; a transaction that writes what eligibility reads (`bots`, a split's
//!   counters, or a split row) passes the engine's `eligibility::guard` before it commits;
//! - every answer is read to a stated size, off the runtime thread (`parsed`), as Rails would hold it and within a
//!   budget of values (`wire`); every number in it goes through `number`, which bounds it before any arithmetic;
//! - no value of an answer is logged: a log line says what happened and where, never an id, a cursor or a body;
//! - no transaction is open across a network call: the fetch phases take no database handle at all;
//! - written: `account_transactions`, `account_balances`, the sync columns of `api_keys` (`last_synced_at`,
//!   `last_sync_error`, `balances_synced_at`, and `status` → incorrect when Alpaca rejects the key on a balance read),
//!   `bots.restatement_generation` (one single-column statement) and `asset_split` rows of `bot_activity_logs`;
//! - never written: `transactions`, and on `bots` nothing but that counter (not `status`, `settings` or
//!   `transient_data`, where the engine keeps its placement intent). No JSON column of an existing row is rewritten
//!   except the `details` of an `asset_split` line the sync itself wrote.
pub mod activities;
pub mod balances;
pub mod cache;
pub mod jobs;
pub mod ledger;
pub mod parity;
pub mod number;
pub mod wire;

use crate::codec::{format_time, parse_time};
use crate::crypto::{Cipher, Credentials};
use crate::venue::alpaca::Body;
use crate::venue::VenueError;
use std::time::{Duration, Instant};
use chrono::{DateTime, Utc};
use crate::jobs::Db;
use rusqlite::{params, Connection, OptionalExtension};

/// ApiKey::SYNC_ERROR_LIMIT.
pub const SYNC_ERROR_LIMIT: usize = 200;
pub const ALPACA: &str = "Exchanges::Alpaca";
/// Before 3.0 a live Alpaca key is not synced (the engine refuses it too): nothing is sent, and the key says why.
/// Worded without the passphrase itself ("live"), which the scrub would redact out of the stored text.
pub const LIVE_REFUSED: &str = "a real-money Alpaca key is not synced by this build (paper only before 3.0)";

/// This process failing, as opposed to the venue: SQLite, a stored value Rails did not write, a key that is not an
/// Alpaca key, a database phase whose task was cancelled.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncError(pub String);
impl From<crate::engine::EngineError> for SyncError {
    fn from(e: crate::engine::EngineError) -> Self {
        match e {
            crate::engine::EngineError::CredentialsChanged => Self(crate::engine::model::CREDENTIALS_CHANGED.into()),
            other => Self(format!("{other:?}")),
        }
    }
}
impl From<rusqlite::Error> for SyncError { fn from(e: rusqlite::Error) -> Self { Self(format!("{e}")) } }
impl From<crate::codec::CodecError> for SyncError { fn from(e: crate::codec::CodecError) -> Self { Self(format!("{e:?}")) } }

/// One database phase of a sync, on the blocking pool (`jobs::Db::run`): no SQLite statement of a sync runs on the
/// runtime thread, and no transaction outlives its phase.
pub async fn phase<T: Send + 'static>(db: &Db, work: impl FnOnce(&Connection) -> Result<T, SyncError> + Send + 'static) -> Result<T, SyncError> {
    db.run(move |c, _| work(c).map_err(|e| e.0)).await.map_err(SyncError)
}

/// What a sync's error starts with when the engine's guard refused one of its writes; the guard's reason follows.
pub const GUARD_REFUSED: &str = "the engine's guard refused this write";

/// What a write phase waits after its transaction, before the sync's next unit (Plan 2f's `CHUNK_GAP`): a writer on
/// another thread, whose SQLite busy handler sleeps up to 100 ms between two tries, gets a try in every gap, and the
/// engine, on the runtime thread, runs in it.
pub const WRITE_GAP: Duration = Duration::from_millis(110);

/// One write transaction, the only place a sync commits: the write lock is taken up front, the statements run, and it
/// commits. `f` says whether it wrote what eligibility reads (a split's row and its bots' `restatement_generation`:
/// the writes of a sync that can change a bot's eligibility); if it did, `eligibility::guard` has its say first, as
/// for every writer of `bots` beside the engine. A refusal rolls the transaction back and fails the sync with the guard's reason (bot ids and
/// `check`'s words, never a secret). Every other unit writes only tables eligibility does not read, and does not pay
/// for the guard (20 to 45 ms a call on a mid-sized install).
pub fn write<T>(c: &Connection, cipher: &Cipher, f: impl FnOnce(&Connection) -> Result<(T, bool), SyncError>) -> Result<T, SyncError> {
    write_timed(c, cipher, |tx| f(tx)).map(|(out, _)| out)
}

/// `write`, and how long its transaction held the write lock (from the lock being taken to the commit returning).
fn write_timed<T>(c: &Connection, cipher: &Cipher, f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<(T, bool), SyncError>) -> Result<(T, Duration), SyncError> {
    let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
    let held = Instant::now();
    let (out, wrote_bots) = f(&tx)?;
    if wrote_bots {
        // A sync writes for a key, not for one bot: 0 names none in the guard's own log line (written only when its check cannot run).
        crate::engine::eligibility::guard(&tx, cipher, None).map_err(|refusal| SyncError(format!("{GUARD_REFUSED}: {}", refusal.reason())))?;
    }
    tx.commit()?;
    Ok((out, held.elapsed()))
}

/// One write phase of a sync that writes no bot: `write`, on the blocking pool, then `WRITE_GAP` with nothing held.
pub async fn commit<T: Send + 'static>(db: &Db, work: impl FnOnce(&Connection) -> Result<T, SyncError> + Send + 'static) -> Result<T, SyncError> {
    commit_bots(db, move |c| work(c).map(|out| (out, false))).await
}

/// One write phase that may write what eligibility reads: `work` says whether it did, and then the engine's guard is asked.
pub async fn commit_bots<T: Send + 'static>(db: &Db, work: impl FnOnce(&Connection) -> Result<(T, bool), SyncError> + Send + 'static) -> Result<T, SyncError> {
    let (out, held) = db.run(move |c, cipher| write_timed(c, cipher, |tx| work(tx)).map_err(|e| e.0)).await.map_err(SyncError)?;
    db.note_write_hold(held);
    tokio::time::sleep(WRITE_GAP).await;
    Ok(out)
}

/// Q: every unit derived from this credential read uses the shared check under
/// the same immediate lock as its write, including errors and watermarks.
pub async fn commit_for<T: Send + 'static>(db: &Db, version: &crate::engine::model::CredentialVersion, work: impl FnOnce(&crate::engine::model::FencedTransaction<'_>) -> Result<T, SyncError> + Send + 'static) -> Result<T, SyncError> {
    commit_bots_for(db, version, move |c| work(c).map(|out| (out, false))).await
}
pub async fn commit_bots_for<T: Send + 'static>(db: &Db, version: &crate::engine::model::CredentialVersion, work: impl FnOnce(&crate::engine::model::FencedTransaction<'_>) -> Result<(T, bool), SyncError> + Send + 'static) -> Result<T, SyncError> {
    let version = Some(version.clone());
    let (out,held)=db.run(move |c,cipher| write_timed(c,cipher,|tx| {
        let fenced=crate::engine::model::check_credential_result(tx,&version)?;
        work(&fenced)
    }).map_err(|e|e.0)).await.map_err(SyncError)?;
    db.note_write_hold(held);tokio::time::sleep(WRITE_GAP).await;Ok(out)
}
pub async fn capture_bound_version(db: &Db, key_id: i64, supplied: &Credentials) -> Result<crate::engine::model::CredentialVersion, SyncError> {
    let supplied = supplied.clone();
    db.run(move |c, cipher| {
        use subtle::ConstantTimeEq;
        let (current, version) = credentials_with_version(c, cipher, key_id).map_err(|e| e.0)?;
        let same = |a: &str, b: &str| bool::from(a.as_bytes().ct_eq(b.as_bytes()));
        let phrase = match (&current.passphrase, &supplied.passphrase) { (Some(a),Some(b)) => same(a,b), (None,None) => true, _ => false };
        if !same(&current.key, &supplied.key) || !same(&current.secret, &supplied.secret) || !phrase {
            crate::engine::log(crate::engine::model::CREDENTIALS_CHANGED);
            return Err(crate::engine::model::CREDENTIALS_CHANGED.into());
        }
        Ok(version)
    }).await.map_err(SyncError)
}

/// Capture credentials and ciphertext version in one read transaction before building a venue.
pub fn credentials_with_version(c: &Connection, cipher: &Cipher, key_id: i64) -> Result<(Credentials, crate::engine::model::CredentialVersion), SyncError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=credentials_with_version(&tx,cipher,key_id)?;tx.commit()?;return Ok(out)}
    let version = crate::engine::model::credential_version_by_id(c, key_id)?.ok_or_else(|| SyncError("credential no longer exists".into()))?;
    let credentials = credentials(c, cipher, key_id)?;
    Ok((credentials, version))
}
/// Validation may diagnose any stored credential: capture the complete redaction set with P.
pub fn validation_material(c:&Connection,cipher:&Cipher,key_id:i64)->Result<(Credentials,crate::engine::model::CredentialVersion,Vec<Option<String>>),SyncError>{
    let tx=c.unchecked_transaction()?;
    let (credentials,version)=credentials_with_version(&tx,cipher,key_id)?;
    let extras:[Option<String>;4]=tx.query_row("SELECT access_token,rsa_signature_key,rsa_encryption_key,dh_param FROM api_keys WHERE id=?1",[key_id],|r|Ok([r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?]))?;
    let mut values=vec![Some(credentials.key.clone()),Some(credentials.secret.clone()),credentials.passphrase.clone()];
    for value in extras{values.push(value.map(|v|cipher.decrypt(&v).map_err(|_|SyncError("unreadable validation credential".into()))).transpose()?);}
    tx.commit()?;Ok((credentials,version,values))
}

/// Why an answer was not read (`parsed`).
#[derive(Clone, Debug, PartialEq)]
pub enum Unread {
    /// Not JSON as Ruby's parser reads it (nested past its limit included): Clients::Alpaca#with_rescue's message for
    /// the failed request, which Rails returns and the job goes on from.
    NotJson,
    /// Where Ruby raises, or a limit of this port: the job stops.
    Raised(String),
}

impl Unread {
    /// What `wire::read` refused, for an answer called `what` ("an activities page").
    pub fn refused(refused: wire::Refused, what: &str) -> Self {
        match refused {
            wire::Refused::NotJson | wire::Refused::TooDeep => Self::NotJson,
            wire::Refused::OverBudget | wire::Refused::TooManyItems => Self::Raised(refused.text(what)),
        }
    }
}

/// Reads a fetched answer on the blocking pool, so no body is parsed on the runtime thread. `read` gets the body of
/// a 2xx answer (its size was bounded when it was fetched). The error is `(text, raised)` as `venue_failure` gives it:
/// a failed request and an unparsable body are Rails' returned Failure; `Unread::Raised` is where Ruby raises.
pub async fn parsed<R: Send + 'static>(body: Body, read: impl FnOnce(&str) -> Result<R, Unread> + Send + 'static) -> Result<R, (String, bool)> {
    let work = tokio::task::spawn_blocking(move || match body.text() {
        Err(e) => Err(venue_failure(e)),
        Ok(text) => match read(text) {
            Err(Unread::NotJson) => Err(venue_failure(body.unreadable())),
            Err(Unread::Raised(why)) => Err((why, true)),
            Ok(r) => Ok(r),
        },
    });
    work.await.unwrap_or_else(|_| Err(("reading the venue's answer was interrupted".into(), true)))
}

/// A sync that did not complete. `error` is the text now in `api_keys.last_sync_error`. `raised` is true where the
/// Rails job re-raises (a transport failure Rails calls retryable): the rest of the job did not run.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure { pub error: String, pub raised: bool }

/// The key being synced.
#[derive(Clone, Debug)]
pub struct Key {
    pub id: i64, pub user_id: i64, pub exchange_id: i64, pub status: i64, pub key_type: i64,
    pub last_synced_at: Option<DateTime<Utc>>, pub last_sync_error: Option<String>,
}

pub fn load_key(c: &Connection, id: i64) -> Result<Key, SyncError> {
    let row = c.query_row(
        "SELECT k.user_id, k.exchange_id, k.status, k.key_type, k.last_synced_at, k.last_sync_error, e.type \
         FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.id = ?1",
        [id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?,
                      r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?))).optional()?;
    let Some((user_id, exchange_id, status, key_type, synced, last_sync_error, exchange_type)) = row else {
        return Err(SyncError(format!("api key {id} does not exist")));
    };
    if exchange_type.as_deref() != Some(ALPACA) { return Err(SyncError(format!("api key {id} is not an Alpaca key"))); }
    let last_synced_at = synced.map(|s| parse_time(&s)).transpose()?;
    Ok(Key { id, user_id, exchange_id, status, key_type, last_synced_at, last_sync_error })
}

/// ApiKey.reading, for Alpaca: per user, the key the venue is read with. Status correct, never a withdrawal key; the
/// trading key if there is one, else the read-only one. (`missing_permission?` cannot hold for Alpaca: it lists no
/// permission error.) This is what the nightly jobs iterate.
pub fn reading_keys(c: &Connection) -> Result<Vec<i64>, SyncError> {
    let mut s = c.prepare(
        "SELECT k.id, k.user_id, k.exchange_id, k.key_type FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
         WHERE k.status = 1 AND k.key_type != 1 AND e.type = ?1 ORDER BY k.id")?;
    let rows = s.query_map([ALPACA], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut picked: Vec<(i64, i64, i64, i64)> = vec![];
    for row in rows {
        match picked.iter_mut().find(|p| (p.1, p.2) == (row.1, row.2)) {
            Some(p) if p.3 != 0 && row.3 == 0 => *p = row, // a trading key replaces the read-only one
            Some(_) => {}
            None => picked.push(row),
        }
    }
    Ok(picked.into_iter().map(|p| p.0).collect())
}

/// Capture all seven encrypted columns.
/// Keep database read failures typed so figure subscriptions retry transient failures.
pub fn credential_columns(c:&Connection,key_id:i64)->Result<[Option<String>;7],rusqlite::Error>{
    c.query_row("SELECT key,secret,passphrase,access_token,rsa_signature_key,rsa_encryption_key,dh_param FROM api_keys WHERE id=?1",[key_id],|r|Ok([r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?]))
}
/// Decrypt the complete material already captured in the caller's read transaction.
pub fn credentials_from_columns(cipher:&Cipher,key_id:i64,columns:&[Option<String>;7])->Result<Credentials,SyncError>{
    let mut credentials=decrypted(cipher,key_id,(columns[0].clone(),columns[1].clone(),columns[2].clone()))?;
    credentials.redaction_values=columns.iter().flatten().map(|value|cipher.decrypt(value).map_err(|_|SyncError("unreadable diagnostic credential".into()))).collect::<Result<Vec<_>,_>>()?;
    Ok(credentials)
}
/// The key's credentials, decrypted. An unreadable value is an error, never an empty credential.
pub fn credentials(c: &Connection, cipher: &Cipher, key_id: i64) -> Result<Credentials, SyncError> {
    credentials_from_columns(cipher,key_id,&credential_columns(c,key_id)?)
}

/// Stored (key, secret, passphrase) decrypted, with no read: an error is only ever an unreadable value.
pub fn decrypted(cipher: &Cipher, key_id: i64, (key, secret, passphrase): (Option<String>, Option<String>, Option<String>)) -> Result<Credentials, SyncError> {
    let open = |v: Option<String>| v.map(|v| cipher.decrypt(&v).map_err(|_| SyncError(format!("api key {key_id} is unreadable (is SECRET_KEY_BASE this instance's own?)")))).transpose();
    Ok(Credentials { redaction_values:vec![],
        key: open(key)?.unwrap_or_default(), // allow-swallow: an Option; a value never saved is Rails' empty string
        secret: open(secret)?.unwrap_or_default(), // allow-swallow: an Option; a value never saved is Rails' empty string
        passphrase: open(passphrase)?,
    })
}

pub fn live(credentials: &Credentials) -> bool { credentials.passphrase.as_deref() == Some("live") }

/// How a failed venue read reaches the job: Clients::Alpaca#with_rescue returns a Failure (its first error is the
/// text), and Client.network_failure raises Client::TransientNetworkError for a transport failure a retry could fix.
pub fn venue_failure(e: VenueError) -> (String, bool) {
    match e {
        VenueError::Rejected(errors) => (errors.into_iter().next().unwrap_or_default(), false),
        VenueError::Transient(m) | VenueError::Ambiguous(m) => (format!("Client::TransientNetworkError: {m}"), true),
    }
}

fn token_char(c: char) -> bool { c.is_ascii_alphanumeric() || c == '_' || c == '-' }
/// Ruby's `\s`: ASCII whitespace only.
fn space(c: char) -> bool { matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r') }

/// Replaces every maximal run of `class` characters that `hit` accepts.
fn redact_runs(text: &str, class: impl Fn(char) -> bool, hit: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() { out.push_str(if hit(run) { "[redacted]" } else { run }); run.clear(); }
    };
    for c in text.chars() {
        if class(c) { run.push(c); } else { flush(&mut run, &mut out); out.push(c); }
    }
    flush(&mut run, &mut out);
    out
}

/// /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i → "[redacted]".
fn redact_emails(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    let domain = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    let mut out = String::with_capacity(text.len());
    let mut i = 0; // everything before `i` is already written
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '@' { at += 1; continue; }
        let mut start = at;
        while start > i && local(chars[start - 1]) { start -= 1; }
        let mut end = at + 1;
        while end < chars.len() && domain(chars[end]) { end += 1; }
        // The last dot of the domain run that has a label before it and two letters after it; the match ends with
        // that dot's letters.
        let stop = (at + 2..end).rev().find(|&d| chars[d] == '.' && d + 2 < chars.len() && chars[d + 1].is_ascii_alphabetic() && chars[d + 2].is_ascii_alphabetic())
            .map(|d| { let mut e = d + 1; while e < chars.len() && chars[e].is_ascii_alphabetic() { e += 1; } e });
        match stop {
            Some(stop) if start < at => {
                out.extend(&chars[i..start]);
                out.push_str("[redacted]");
                i = stop;
                at = stop;
            }
            _ => at += 1,
        }
    }
    out.extend(&chars[i..]);
    out
}

/// %r{(https?://\S+?)\?\S*} → "\1?[redacted]".
fn redact_queries(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let starts_with = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c));
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let scheme = if starts_with(i, "https://") { 8 } else if starts_with(i, "http://") { 7 } else { 0 };
        // `\S+?` needs one character before the question mark; a space before any question mark ends the attempt.
        let mark = (scheme > 0).then(|| (i + scheme..chars.len()).take_while(|&k| !space(chars[k])).find(|&k| k > i + scheme && chars[k] == '?')).flatten();
        match mark {
            Some(mark) => {
                out.extend(&chars[i..mark]);
                out.push_str("?[redacted]");
                i = mark;
                while i < chars.len() && !space(chars[i]) { i += 1; }
            }
            None => { out.push(chars[i]); i += 1; }
        }
    }
    out
}

/// ApiKey#scrub: the key's own credentials by value (longest first), then emails, URL query strings, tokens of twenty
/// or more `[A-Za-z0-9_-]` characters that contain a digit, and runs of nine or more digits.
pub fn scrub(text: &str, credentials: &Credentials) -> String {
    let text=credentials.venue_text(text);
    if text==crate::crypto::VENUE_TEXT_REDACTED{return text;}
    let text = redact_queries(&redact_emails(&text));
    let text = redact_runs(&text, token_char, |run| run.chars().count() >= 20 && run.chars().any(|c| c.is_ascii_digit()));
    redact_runs(&text, |c| c.is_ascii_digit(), |run| run.len() >= 9)
}

/// ApiKey#record_sync_error!: scrubbed, cut to 200 characters, written without moving `updated_at`. Returns the text.
pub fn record_sync_error(c: &crate::engine::model::FencedTransaction<'_>, key_id: i64, text: &str, credentials: &Credentials) -> Result<String, SyncError> {
    let stored: String = scrub(text, credentials).chars().take(SYNC_ERROR_LIMIT).collect();
    c.execute("UPDATE api_keys SET last_sync_error = ?1 WHERE id = ?2", params![stored, key_id])?;
    Ok(stored)
}

/// Rails' quoted time for a bound parameter or a stored column.
pub fn sql_time(t: DateTime<Utc>) -> String { format_time(t) }
