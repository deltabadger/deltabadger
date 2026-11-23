//! The tracker's background syncs for an Alpaca key, as 2.x's jobs run them: the account-transaction (ledger) sync
//! (AccountTransaction::SyncJob) and the account-balance sync (AccountBalance::SyncJob). Each is an idempotent async
//! function, safe to call at any time; when to call them is the scheduler's business.
//!
//! The write rule (these jobs write the SQLite file the engine trades on):
//! - every database phase runs off the runtime thread (`jobs::Db::run`), in short `BEGIN IMMEDIATE` transactions
//!   (`write`), with `WRITE_GAP` after each; a transaction that writes what eligibility reads (`bots`, a split's
//!   counters, or a split row) passes the engine's `eligibility::guard` before it commits;
//! - every answer is read to a stated size, off the runtime thread (`parsed`), as 2.x would hold it and within a
//!   budget of values (`wire`); every number in it goes through `number`, which bounds it before any arithmetic;
//! - no value of an answer is logged: a log line says what happened and where, never an id, a cursor or a body;
//! - no transaction is open across a network call: the fetch phases take no database handle at all;
//! - written: `account_transactions`, `exchange_account_balances` (by account), the sync columns of `api_keys` (`last_synced_at`,
//!   `last_sync_error`, `balances_synced_at`, and `status` → incorrect when Alpaca rejects the key on a balance read),
//!   `bots.restatement_generation` (one single-column statement) and `asset_split` rows of `bot_activity_logs`;
//! - never written: `transactions`, and on `bots` nothing but that counter (not `status`, `settings` or
//!   `transient_data`, where the engine keeps its placement intent). No JSON column of an existing row is rewritten
//!   except the `details` of an `asset_split` line the sync itself wrote.
pub mod activities;
pub mod balances;
pub mod balance_store;
pub mod binance_ledger;
pub mod binance_order_links;
pub mod binance_identity;
pub mod binance_writer;
pub mod binance_repair;
pub mod bitvavo_ledger;
pub mod bitget_ledger;
pub mod mexc_ledger;
pub mod bybit_ledger;
pub mod bitrue_ledger;
pub mod bingx_ledger;
pub mod kraken_ledger;
pub mod kucoin_ledger;
pub mod kucoin_hf;
pub mod kucoin_hf_ledger;
pub mod gemini_ledger;
pub mod hyperliquid_ledger;
pub mod fills;
pub mod cache;
pub mod continuation;
mod pending_transfers;
pub mod jobs;
pub mod ledger;
pub mod parity;
pub mod number;
pub mod wire;
pub mod venue_history;
pub mod venue_quotes;
pub mod venue_routes;

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

/// This process failing, as opposed to the venue: SQLite, a stored value 2.x did not write, a key that is not an
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
    commit_bots_for_ciphered(db, version, move |fenced, _| work(fenced)).await
}
/// `commit_bots_for`, the work also given the instance's cipher.
pub async fn commit_bots_for_ciphered<T: Send + 'static>(db: &Db, version: &crate::engine::model::CredentialVersion, work: impl FnOnce(&crate::engine::model::FencedTransaction<'_>, &Cipher) -> Result<(T, bool), SyncError> + Send + 'static) -> Result<T, SyncError> {
    commit_checked(db, version, true, work).await
}
/// `commit_bots_for_ciphered` for a key's own validation status and identity read, which a fenced key still records
/// (`check_credential_record`).
pub async fn commit_record_for<T: Send + 'static>(db: &Db, version: &crate::engine::model::CredentialVersion, work: impl FnOnce(&crate::engine::model::FencedTransaction<'_>, &Cipher) -> Result<(T, bool), SyncError> + Send + 'static) -> Result<T, SyncError> {
    commit_checked(db, version, false, work).await
}
async fn commit_checked<T: Send + 'static>(db: &Db, version: &crate::engine::model::CredentialVersion, fence: bool, work: impl FnOnce(&crate::engine::model::FencedTransaction<'_>, &Cipher) -> Result<(T, bool), SyncError> + Send + 'static) -> Result<T, SyncError> {
    let version = Some(version.clone());
    let reader = if fence { current_reader() } else { None };
    let (out,held)=db.run(move |c,cipher| write_timed(c,cipher,|tx| {
        let fenced=if fence {crate::engine::model::check_credential_result(tx,&version)?} else {crate::engine::model::check_credential_record(tx,&version)?};
        // Rule N7: a run of a former reader (or of an earlier generation) checkpoints and publishes nothing.
        if let Some(r) = &reader { if !reader_holds(tx, r)? { crate::engine::log(crate::engine::model::CREDENTIALS_CHANGED); return Err(SyncError(crate::engine::model::CREDENTIALS_CHANGED.into())); } }
        work(&fenced,cipher)
    }).map_err(|e|e.0)).await.map_err(SyncError)?;
    db.note_write_hold(held);tokio::time::sleep(WRITE_GAP).await;Ok(out)
}
tokio::task_local! {
    /// The account the running job captured its key's version on (#710 §6.4): a later capture inside it must name the same.
    static BOUND: Option<i64>;
}
tokio::task_local! {
    /// What the running ledger or balance job captured at its entry (rule N7): every fenced commit inside it checks it.
    static READER: Option<Reader>;
}
/// Runs `work` as reader `r` (`commit_checked` refuses a commit once the account's reader or generation moved).
pub async fn as_reader<F: std::future::Future>(r: Option<Reader>, work: F) -> F::Output { READER.scope(r, work).await }
/// The reader the running job captured, if any.
pub fn current_reader() -> Option<Reader> { READER.try_with(|r| *r).ok().flatten() }
/// Runs `work` bound to `account` (`capture_bound_version` refuses a capture on another account inside it).
pub async fn bound<F: std::future::Future>(account: Option<i64>, work: F) -> F::Output { BOUND.scope(account, work).await }

pub async fn capture_bound_version(db: &Db, key_id: i64, supplied: &Credentials) -> Result<crate::engine::model::CredentialVersion, SyncError> {
    let supplied = supplied.clone();
    let bound = BOUND.try_with(|a| *a).ok().flatten();
    db.run(move |c, cipher| {
        use subtle::ConstantTimeEq;
        let (current, version) = credentials_with_version(c, cipher, key_id).map_err(|e| e.0)?;
        let same = |a: &str, b: &str| bool::from(a.as_bytes().ct_eq(b.as_bytes()));
        let phrase = match (&current.passphrase, &supplied.passphrase) { (Some(a),Some(b)) => same(a,b), (None,None) => true, _ => false };
        if !same(&current.key, &supplied.key) || !same(&current.secret, &supplied.secret) || !phrase {
            crate::engine::log(crate::engine::model::CREDENTIALS_CHANGED);
            return Err(crate::engine::model::CREDENTIALS_CHANGED.into());
        }
        if bound.is_some_and(|account| version.account() != Some(account)) {
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
    /// Not JSON as 2.x's parser reads it (nested past its limit included): Clients::Alpaca#with_rescue's message for
    /// the failed request, which 2.x returns and the job goes on from.
    NotJson,
    /// Where 2.x raises, or a limit of this port: the job stops.
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
/// a failed request and an unparsable body are 2.x's returned Failure; `Unread::Raised` is where 2.x raises.
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
/// 2.x job re-raises (a transport failure 2.x calls retryable): the rest of the job did not run.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure { pub error: String, pub raised: bool }

/// The key being synced.
#[derive(Clone, Debug)]
pub struct Key {
    pub id: i64, pub user_id: i64, pub exchange_id: i64, pub status: i64, pub key_type: i64,
    pub last_synced_at: Option<DateTime<Utc>>, pub last_sync_error: Option<String>,
    /// `exchanges.type`: one of the venues this build syncs.
    pub exchange_type: String,
    /// #710: the key's account, as its ledger rows' venue ids are scoped (venue_ids).
    pub scope: crate::venue_ids::Scope,
}

/// #710 7a §3: the writer's key id for a keyless account's import: its rows are written with no `api_key_id`.
pub const NO_KEY: i64 = 0;

impl Key {
    /// The `api_key_id` its rows are written with.
    pub fn api_key_id(&self) -> Option<i64> { (self.id != NO_KEY).then_some(self.id) }
}

/// The venues whose balances this build syncs (`registry::Row::balances`).
pub fn balance_venues() -> Vec<&'static str> { crate::venue::registry::types(|r| r.balances.is_some()) }

/// The key being synced, when it is an Alpaca key: the ledger sync's key.
pub fn load_key(c: &Connection, id: i64) -> Result<Key, SyncError> {
    load_key_of(c, id, &[ALPACA])
}

/// The key being synced, when its venue is one of `venues`.
pub fn load_key_of(c: &Connection, id: i64, venues: &[&str]) -> Result<Key, SyncError> {
    let row = c.query_row(
        "SELECT k.user_id, k.exchange_id, k.status, k.key_type, k.last_synced_at, k.last_sync_error, e.type \
         FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.id = ?1",
        [id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?,
                      r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?))).optional()?;
    let Some((user_id, exchange_id, status, key_type, synced, last_sync_error, exchange_type)) = row else {
        return Err(SyncError(format!("api key {id} does not exist")));
    };
    let Some(exchange_type) = exchange_type.filter(|t| venues.contains(&t.as_str())) else {
        return Err(SyncError(if venues == [ALPACA] { format!("api key {id} is not an Alpaca key") } else { format!("api key {id} is not on a venue this build syncs") }));
    };
    let last_synced_at = synced.map(|s| parse_time(&s)).transpose()?;
    let scope = key_scope(c, id)?;
    Ok(Key { id, user_id, exchange_id, status, key_type, last_synced_at, last_sync_error, exchange_type, scope })
}

/// #710: api key `id`'s account's scope; a key whose membership names another user's or exchange's account has none.
pub fn key_scope(c: &Connection, id: i64) -> Result<crate::venue_ids::Scope, SyncError> {
    crate::venue_ids::Scope::of_key(c, id)?.ok_or_else(|| SyncError(format!("api key {id} belongs to no account of its exchange")))
}

/// ApiKey.reading, for Alpaca: per user, the key the venue is read with. Status correct, never a withdrawal key; the
/// trading key if there is one, else the read-only one. (`missing_permission?` cannot hold for Alpaca: it lists no
/// permission error.) This is what the nightly jobs iterate.
pub fn reading_keys(c: &Connection) -> Result<Vec<i64>, SyncError> { reading_keys_of(c, &[ALPACA]) }

/// ApiKey.reading over the venues whose balances this build syncs.
pub fn balance_reading_keys(c: &Connection) -> Result<Vec<i64>, SyncError> { reading_keys_of(c, &balance_venues()) }

/// The venues with a ledger job (`registry::Row::ledger`): the ones the tracker's history-gap step judges (tracker::gaps).
pub fn ledger_venues() -> Vec<&'static str> { crate::venue::registry::types(|r| r.ledger.is_some()) }

/// ApiKey.reading over `ledger_venues`.
pub fn ledger_reading_keys(c: &Connection) -> Result<Vec<i64>, SyncError> { reading_keys_of(c, &ledger_venues()) }

/// A key the reader selector weighs: (id, user, exchange, key type, venue, last sync error, account).
type Candidate = (i64, i64, i64, i64, String, Option<String>, Option<i64>);
/// The keys that can read, by id: status correct, never a withdrawal key, not fenced (#710 7a §9.1), and (rule N1) on an
/// exchange with two or more live accounts never an identity venue's key without a UID read of its current material
/// (`identity::unread`): it is read first. `filter` narrows the rows (SQL on alias `k`).
fn candidates(c: &Connection, filter: &str) -> Result<Vec<Candidate>, SyncError> {
    let rows = crate::identity::with_key_account("k", |account| c.prepare(&format!(
        "SELECT k.id, k.user_id, k.exchange_id, k.key_type, e.type, k.last_sync_error, {account} FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
         WHERE k.status = 1 AND k.key_type != 1 AND NOT {} AND {filter} ORDER BY k.id", crate::identity::fenced_sql(c, "k")?))?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                                      r.get::<_, Option<String>>(5)?, r.get::<_, Option<i64>>(6)?)))?
        .collect::<Result<Vec<_>, _>>())?;
    let mut out = vec![];
    let mut several: std::collections::HashMap<(i64, i64), bool> = std::collections::HashMap::new();
    for r in rows {
        if crate::identity::VENUES.contains(&r.4.as_str()) {
            let many = match several.get(&(r.1, r.2)) { Some(m) => *m, None => { let m = crate::identity::accounts_of(c, r.1, r.2)?.len() >= 2; several.insert((r.1, r.2), m); m } };
            if many && crate::identity::unread(c, r.0).map_err(SyncError)? { continue; }
        }
        out.push(r);
    }
    Ok(out)
}
/// ApiKey.reading among one account's candidates: the trading key that lacks no permission (`missing_permission?`), else
/// the read-only one, else the first.
fn choose(group: &[&Candidate]) -> Option<i64> {
    group.iter().find(|r| r.3 == 0 && !crate::tracker::import::run::missing_permission(&r.4, r.5.as_deref()))
        .or_else(|| group.iter().find(|r| r.3 == 2)).or_else(|| group.first()).map(|r| r.0)
}
/// ApiKey.reading: per user and account (#710; an exchange while it has one), `choose` over `candidates`.
fn reading_keys_of(c: &Connection, venues: &[&str]) -> Result<Vec<i64>, SyncError> {
    let rows: Vec<_> = candidates(c, "1")?.into_iter().filter(|r| venues.contains(&r.4.as_str())).collect();
    let mut groups: Vec<(i64, i64, Option<i64>)> = vec![];
    for r in &rows { if !groups.contains(&(r.1, r.2, r.6)) { groups.push((r.1, r.2, r.6)); } }
    Ok(groups.into_iter().filter_map(|g| choose(&rows.iter().filter(|r| (r.1, r.2, r.6) == g).collect::<Vec<_>>())).collect())
}
/// #710 7a rules M4/Q4: THE reader selector, for one account (`keys`: its key condition on alias `k`, `Account::keys`):
/// the key the scheduler reads its ledger and balances with, which freshness and Merge ask. None: no key can read it.
pub fn reader_of(c: &Connection, keys: &str) -> Result<Option<i64>, SyncError> {
    Ok(choose(&candidates(c, keys)?.iter().collect::<Vec<_>>()))
}

/// What a ledger or balance job captured at its entry (rules N7/P7): its key was its account's reader in that account's
/// reader generation. `account` None: a number 1 with no row yet, which keeps no generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reader { pub key: i64, pub account: Option<i64>, pub generation: i64 }
fn generations_ready(c: &Connection) -> rusqlite::Result<bool> { crate::engine::rules::table_exists(c, "ledger_reader_generations") }
/// Account `account`'s reader generation (0 before its first handoff).
pub fn reader_generation(c: &Connection, account: i64) -> Result<i64, SyncError> {
    if !generations_ready(c)? { return Ok(0); }
    Ok(c.query_row("SELECT generation FROM ledger_reader_generations WHERE account_id = ?1", [account], |r| r.get(0)).optional()?.unwrap_or(0))
}
/// Rule P7: in one read, whether key `key` is its account's reader, and that account's generation. None: it is not (a job
/// woken for it does nothing).
pub fn reader_entry(c: &Connection, key: i64) -> Result<Option<Reader>, SyncError> {
    if c.is_autocommit() { let tx = c.unchecked_transaction()?; let out = reader_entry(&tx, key)?; tx.commit()?; return Ok(out); }
    let Some(account) = crate::identity::Account::of_key(c, key)? else { return Ok(None) };
    if reader_of(c, &account.keys("k"))? != Some(key) { return Ok(None); }
    Ok(Some(Reader { key, account: account.id, generation: match account.id { Some(a) => reader_generation(c, a)?, None => 0 } }))
}
/// Rule N7: whether what `r` captured still holds (call in the transaction that checkpoints or publishes): the same
/// account's reader, in the same generation.
pub fn reader_holds(c: &Connection, r: &Reader) -> Result<bool, SyncError> {
    Ok(match (reader_entry(c, r.key)?, r.account) {
        (None, _) => false,
        (Some(now), Some(_)) => now == *r,
        // A number 1 that had no row at entry may get it during the run (the run's own writes ensure it): still its reader,
        // in its first generation.
        (Some(now), None) => now.generation == 0,
    })
}
/// Rules M4/N7: the handoff, in the caller's write transaction, wherever an account's reader may have changed (a fence
/// or its lifting, a permission fallback, a key saved, replaced or removed, a takeover): the account's reader is chosen
/// again; when it is another key than the one stored, the generation is bumped (a run of the former reader publishes
/// nothing) and the incoming reader's ledger and balance stamps and its saved continuation go, so the account is not
/// current until that reader's own job has run. The first time an account is seen its reader is only stored. Returns
/// the new reader when it changed (the caller wakes its jobs after commit).
pub fn settle_reader(c: &Connection, account: i64) -> Result<Option<i64>, SyncError> {
    if !generations_ready(c)? { return Ok(None); }
    let Some(a) = crate::identity::Account::by_id(c, account)? else { return Ok(None) };
    let reader = reader_of(c, &a.keys("k"))?;
    let stored: Option<Option<i64>> = c.query_row("SELECT reader FROM ledger_reader_generations WHERE account_id = ?1", [account], |r| r.get(0)).optional()?;
    match stored {
        None => { c.execute("INSERT INTO ledger_reader_generations (account_id, generation, reader) VALUES (?1, 0, ?2)", rusqlite::params![account, reader])?; return Ok(None); }
        Some(was) if was == reader => return Ok(None),
        Some(_) => {}
    }
    c.execute("UPDATE ledger_reader_generations SET generation = generation + 1, reader = ?2 WHERE account_id = ?1", rusqlite::params![account, reader])?;
    let Some(k) = reader else { return Ok(None) };
    let scope = k.to_string();
    for key in [crate::jobs::state::key("ledger_origin", Some(&scope)), format!("rust_sync.venue_ledger:{k}"), format!("rust_sync.ledger:{k}")] {
        c.execute("DELETE FROM app_configs WHERE key = ?1", [key])?;
    }
    if let Some(row) = crate::identity::AccountRow::by_id(c, account)? { c.execute("DELETE FROM app_configs WHERE key = ?1", [cache::balance_origin_key_of(&row)])?; }
    Ok(Some(k))
}
/// `settle_reader` for every live account of (user, exchange); the readers that changed.
pub fn settle_readers(c: &Connection, user: i64, exchange: i64) -> Result<Vec<i64>, SyncError> {
    if !generations_ready(c)? { return Ok(vec![]); }
    let mut out = vec![];
    for (account, _) in crate::identity::accounts_of(c, user, exchange)? { out.extend(settle_reader(c, account)?); }
    Ok(out)
}
/// `settle_readers` for key `key`'s exchange (nothing for a key gone).
pub fn settle_readers_of_key(c: &Connection, key: i64) -> Result<Vec<i64>, SyncError> {
    if !generations_ready(c)? { return Ok(vec![]); }
    let row: Option<(i64, i64)> = c.query_row("SELECT user_id, exchange_id FROM api_keys WHERE id = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    match row { Some((user, exchange)) => settle_readers(c, user, exchange), None => Ok(vec![]) }
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
    let columns=credential_columns(c,key_id)?;
    let mut credentials=credentials_from_columns(cipher,key_id,&columns)?;
    // An IBKR key's session material (Session#initialize's reads), when all of it is stored.
    let venue:Option<String>=c.query_row("SELECT e.type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1",[key_id],|r|r.get(0)).optional()?;
    if venue.as_deref()==Some(crate::venue::ibkr::IBKR) {
        let open=|v:&Option<String>|v.as_ref().map(|v|cipher.decrypt(v).map_err(|_|SyncError(format!("api key {key_id} is unreadable")))).transpose();
        if let [Some(access_token),Some(signature_key),Some(encryption_key),Some(dh_param)]=[open(&columns[3])?,open(&columns[4])?,open(&columns[5])?,open(&columns[6])?] {
            let realm:Option<String>=c.query_row("SELECT ibkr_realm FROM api_keys WHERE id=?1",[key_id],|r|r.get(0))?;
            credentials.ibkr=Some(Box::new(crate::venue::signers::ibkr::Material{consumer_key:credentials.key.clone(),access_token,access_token_secret:credentials.secret.clone(),
                signature_key,encryption_key,dh_param,realm:realm.unwrap_or_default()}));
        }
    }
    Ok(credentials)
}

/// Stored (key, secret, passphrase) decrypted, with no read: an error is only ever an unreadable value.
pub fn decrypted(cipher: &Cipher, key_id: i64, (key, secret, passphrase): (Option<String>, Option<String>, Option<String>)) -> Result<Credentials, SyncError> {
    let open = |v: Option<String>| v.map(|v| cipher.decrypt(&v).map_err(|_| SyncError(format!("api key {key_id} is unreadable (is SECRET_KEY_BASE this instance's own?)")))).transpose();
    Ok(Credentials { redaction_values:vec![],
        key: open(key)?.unwrap_or_default(), // allow-swallow: an Option; a value never saved is 2.x's empty string
        secret: open(secret)?.unwrap_or_default(), // allow-swallow: an Option; a value never saved is 2.x's empty string
        passphrase: open(passphrase)?,
        ibkr: None,
    })
}

/// How a failed venue read reaches the job: Clients::Alpaca#with_rescue returns a Failure (its first error is the
/// text), and Client.network_failure raises Client::TransientNetworkError for a transport failure a retry could fix.
pub fn venue_failure(e: VenueError) -> (String, bool) {
    match e {
        VenueError::Rejected(errors) | VenueError::Unauthorized(errors) | VenueError::Coded { errors, .. } => (errors.into_iter().next().unwrap_or_default(), false),
        VenueError::Transient(m) | VenueError::Ambiguous(m) => (format!("Client::TransientNetworkError: {m}"), true),
    }
}

fn token_char(c: char) -> bool { c.is_ascii_alphanumeric() || c == '_' || c == '-' }
/// 2.x's `\s`: ASCII whitespace only.
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
    // A permission error moves the account's reader to its read-only key (rule M4's permission fallback).
    settle_readers_of_key(c, key_id)?;
    Ok(stored)
}

/// 2.x's quoted time for a bound parameter or a stored column.
pub fn sql_time(t: DateTime<Utc>) -> String { format_time(t) }

pub mod coinbase_ledger;
pub mod coinbase_repair;
