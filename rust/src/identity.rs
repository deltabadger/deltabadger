//! Exchange-account identity (plan AF, slice AF1): what account each connected credential and each identity-bearing
//! import belongs to. Recorded only: nothing here refuses or changes a key, a sync, a bot or an import.
//!
//! - The account id a venue reports is read at the transport boundary (`Transport::send_observed`), before the body is
//!   scrubbed of credentials, and only that one field leaves it (`AccountId`, whose Debug prints nothing).
//! - It is stored as a fingerprint (an HMAC under the install's `rust_account_identity_key_v1`) and encrypted in the
//!   credential envelope; never in plain text, never in a log.
//! - Rails' encryption reset deletes every app_configs row: a missing (or unreadable) identity key while accounts exist
//!   is that reset, and every fingerprint and observation is cleared before a new key is stored (`key`).
//! - Conflicts are never stored: they are derived when read (`standing`, `credential_standing`).
use crate::codec::{format_time, parse_time};
use crate::crypto::{Cipher, Credentials};
use crate::venue::binance::{BINANCE, BINANCE_US};
use crate::venue::http::{HttpRequest, Transport};
use crate::venue::kucoin::KUCOIN;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// The app_configs key of the install's identity key (32 random bytes, hex, in the credential envelope).
pub const KEY_CONFIG: &str = "rust_account_identity_key_v1";
/// Rust's identity tables (rust/migrations/20261022000001_create_exchange_accounts.sql).
pub const TABLES: [&str; 3] = ["exchange_accounts", "exchange_account_credentials", "exchange_account_imports"];
/// The venues whose account id is confirmed (AF1). Every other venue records nothing yet.
pub const VENUES: [&str; 3] = [KUCOIN, BINANCE, BINANCE_US];
/// A credential is observed again after this long, and after any change of its stored ciphertext.
pub fn observe_every() -> Duration { Duration::weeks(1) }
/// A failed read is retried no sooner than this.
// ponytail: a fixed wait (the job runs hourly); a growing one if a venue's limits ever ask for it.
pub fn retry_after() -> Duration { Duration::hours(1) }
/// The scheduled job that observes every credential of a supported venue, idle and legacy ones included.
pub const JOB: &str = "account_identity";
/// The most credentials one run reads: each read is one request, and the run has the scheduler's deadline.
const PER_RUN: usize = 10;

/// KuCoin's account read for a key: its own description, `data.uid` (the export's `UID`).
pub const KUCOIN_PATH: &str = "/api/v1/user/api-key";
/// Binance's and Binance.US's account read, `uid` (Binance's export's `User ID`).
pub const BINANCE_PATH: &str = "/api/v3/account";
/// The account-id column of each venue's own export, by venue: Binance's two spellings (main's recorded exports use both).
pub const BINANCE_EXPORT_COLUMNS: [&str; 2] = ["User ID", "User_ID"];
pub const KUCOIN_EXPORT_COLUMNS: [&str; 1] = ["UID"];

/// An account id as the venue states it, normalized. Its Debug prints nothing; it never reaches a log or a plain column.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountId(String);
impl std::fmt::Debug for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("AccountId([redacted])") }
}
impl AccountId {
    /// Every AF1 venue numbers its accounts: the id as canonical decimal text (no sign, no leading zeros). Anything else
    /// (blank, masked `12****89`, a fraction, zero) is no account id.
    // ponytail: one normalizer while every venue is decimal; addresses (Hyperliquid, lowercased) come with their slice.
    pub fn parse(raw: &str) -> Option<Self> {
        let t = raw.trim();
        if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) { return None; }
        let canonical = t.trim_start_matches('0');
        (!canonical.is_empty()).then(|| Self(canonical.to_string()))
    }
    fn from_json(v: &Value) -> Option<Self> {
        match v { Value::Number(n) if n.is_u64() => Self::parse(&n.to_string()), Value::String(s) => Self::parse(s), _ => None }
    }
}

/// What one identity read came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observed {
    Read(AccountId),
    /// The read failed: its HTTP status and the venue's error code only (never a body, never an id).
    Failed(String),
}

/// The failure of an answer: its status and the venue's numeric `code`, nothing else.
fn failure(status: u16, body: &str) -> Observed {
    let code = serde_json::from_str::<Value>(body).ok().and_then(|v| match v.get("code") {
        Some(Value::Number(n)) => n.as_i64().map(|n| n.to_string()),
        Some(Value::String(s)) if (1..=12).contains(&s.len()) && s.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()) => Some(s.clone()),
        _ => None,
    });
    Observed::Failed(match code { Some(code) => format!("http {status}; code {code}"), None => format!("http {status}") })
}

/// KuCoin's GET /api/v1/user/api-key: `data.uid` under the success code. The answer echoes the key itself, which is why
/// this runs before the transport scrubs the body.
pub fn kucoin_answer(status: u16, body: &str) -> Observed {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return failure(status, body) };
    if !(200..300).contains(&status) || v["code"] != "200000" { return failure(status, body); }
    // A success answer without a readable uid (KuCoin masks it for some keys): its success code is no error code.
    AccountId::from_json(&v["data"]["uid"]).map_or_else(|| Observed::Failed(format!("http {status}")), Observed::Read)
}

/// Binance's (and Binance.US's) GET /api/v3/account: its `uid`; an error envelope is a failure.
pub fn binance_answer(status: u16, body: &str) -> Observed {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return failure(status, body) };
    if !(200..300).contains(&status) || v.get("code").is_some_and(|c| !c.is_null()) { return failure(status, body); }
    AccountId::from_json(&v["uid"]).map_or_else(|| failure(status, body), Observed::Read)
}

/// The identity read of `exchange_type` through `transport`, or None for a venue without a confirmed identity.
pub fn request<T: Transport + Clone>(exchange_type: &str, transport: &T, base: &str, secret: &str) -> Option<HttpRequest> {
    use crate::venue::binance::{BinanceVenue, READ_RECV_WINDOW};
    match exchange_type {
        KUCOIN => Some(HttpRequest { method: "GET", base: base.into(), path: KUCOIN_PATH.into(), query: vec![], body: None, not_after: None }),
        BINANCE | BINANCE_US => Some(BinanceVenue::new(transport.clone(), base, secret)
            .signed("GET", BINANCE_PATH, vec![("omitZeroBalances", "true".into()), ("recvWindow", READ_RECV_WINDOW.into())], transport.now())),
        _ => None,
    }
}

/// How an answer of `exchange_type`'s identity read is read.
pub fn extractor(exchange_type: &str) -> fn(u16, &str) -> Observed { if exchange_type == KUCOIN { kucoin_answer } else { binance_answer } }

/// The identity read: one request, the id taken from the raw answer and the body dropped. None: not a supported venue.
pub async fn read<T: Transport + Clone>(exchange_type: &str, transport: &T, base: &str, secret: &str) -> Option<Observed> {
    let request = request(exchange_type, transport, base, secret)?;
    Some(match transport.send_observed(&request, &extractor(exchange_type)).await {
        Ok((_, observed)) => observed,
        Err(_) => Observed::Failed("transport".into()),
    })
}

/// `read` over the venue's production transport, sent through `client` (the venue's proxy when one is configured).
pub async fn read_live(exchange_type: &str, base: &str, client: reqwest::Client, credentials: &Credentials) -> Option<Observed> {
    use crate::venue::http::ReqwestTransport;
    let (key, secret, redaction) = (credentials.key.clone(), credentials.secret.clone(), credentials.redaction_values.clone());
    let transport = match exchange_type {
        KUCOIN => ReqwestTransport::kucoin(client, key, secret, credentials.passphrase.clone().unwrap_or_default()),
        BINANCE | BINANCE_US => ReqwestTransport::binance(client, key, secret),
        _ => return None,
    }.with_sensitive_values(redaction);
    read(exchange_type, &transport, base, &credentials.secret).await
}

/// The install's identity key.
pub struct IdentityKey([u8; 32]);
impl IdentityKey {
    fn mac(&self, domain: &str, parts: &[&str]) -> String {
        use hmac::Mac;
        let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(&self.0).expect("HMAC takes any key");
        mac.update(domain.as_bytes());
        for part in parts { mac.update(&[0]); mac.update(part.as_bytes()); }
        hex::encode(mac.finalize().into_bytes())[..32].to_string()
    }
    /// The account's fingerprint: domain-separated, per venue namespace (the exchange type).
    pub fn fingerprint(&self, exchange_type: &str, id: &AccountId) -> String { self.mac("deltabadger-account-fingerprint-v1", &[exchange_type, &id.0]) }
    /// What the credential is made of: it changes when the material does, never on a re-encryption.
    pub fn material_digest(&self, c: &Credentials) -> String {
        self.mac("deltabadger-credential-material-v1", &[&c.key, &c.secret, c.passphrase.as_deref().unwrap_or("")])
    }
}

/// Whether this install has the identity tables (an install `run`/`serve` has not migrated records nothing).
pub fn ready(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'exchange_accounts'", [], |r| r.get::<_, i64>(0)).map(|n| n == 1)
}

/// The stored key, if there is a readable one. Never writes.
pub fn existing_key(c: &Connection, cipher: &Cipher) -> Option<IdentityKey> {
    let hex = crate::app_config::get(c, cipher, KEY_CONFIG).ok()??;
    let bytes: [u8; 32] = hex::decode(hex).ok()?.try_into().ok()?;
    Some(IdentityKey(bytes))
}

/// The identity key, created when there is none. Call inside a write transaction. A missing key while accounts exist,
/// or one that cannot be read, is an encryption reset: every fingerprint and observation is cleared first (accounts and
/// history stay), then a new key is stored.
pub fn key(c: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<IdentityKey, String> {
    if let Some(key) = existing_key(c, cipher) { return Ok(key); }
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    let stored = crate::app_config::exists(c, KEY_CONFIG)?;
    let accounts: i64 = c.query_row("SELECT count(*) FROM exchange_accounts", [], |r| r.get(0)).map_err(db)?;
    if stored || accounts > 0 {
        crate::engine::log(if stored { "account identity: the identity key cannot be read; known account identities are cleared" }
                           else { "account identity: the identity key is gone (an encryption reset); known account identities are cleared" });
    }
    c.execute_batch("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL;
                     DELETE FROM exchange_account_credentials; DELETE FROM exchange_account_imports;").map_err(db)?;
    crate::app_config::delete(c, KEY_CONFIG)?;
    let bytes: [u8; 32] = rand::random();
    crate::app_config::set(c, cipher, KEY_CONFIG, &hex::encode(bytes), now)?;
    Ok(IdentityKey(bytes))
}

/// At startup: a reset Rails made while Rust was stopped is handled before anything is recorded. Writes nothing on an
/// install with a readable key or with no accounts.
pub fn initialize(c: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<(), String> {
    if !ready(c).map_err(|e| e.to_string())? || existing_key(c, cipher).is_some() { return Ok(()); }
    let accounts: i64 = c.query_row("SELECT count(*) FROM exchange_accounts", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if accounts == 0 { return Ok(()); }
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    key(&tx, cipher, now)?;
    tx.commit().map_err(|e| e.to_string())
}

/// The account row of (user, exchange), created unknown when there is none.
fn ensure_account(c: &Connection, user_id: i64, exchange_id: i64, now: DateTime<Utc>) -> rusqlite::Result<i64> {
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT (user_id, exchange_id) DO NOTHING",
              params![user_id, exchange_id, format_time(now)])?;
    c.query_row("SELECT id FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2", [user_id, exchange_id], |r| r.get(0))
}

/// Fills an unknown account's fingerprint. A known one is never overwritten.
fn establish(c: &Connection, account: i64, fingerprint: &str, encrypted: &[u8], now: DateTime<Utc>) -> rusqlite::Result<bool> {
    c.execute("UPDATE exchange_accounts SET fingerprint = ?2, account_id_enc = ?3, updated_at = ?4 WHERE id = ?1 AND fingerprint IS NULL",
              params![account, fingerprint, encrypted, format_time(now)]).map(|n| n == 1)
}

/// The key the sync reads (user, exchange) with (ApiKey.reading, sync::reading_keys_of): the only credential that
/// may establish an account.
fn reading_key(c: &Connection, user_id: i64, exchange_id: i64) -> Result<Option<i64>, String> {
    Ok(crate::tracker::import::run::reading(c, user_id).map_err(|e| e.0)?.into_iter().find(|k| k.exchange_id == exchange_id).map(|k| k.id))
}

/// (user, exchange, exchange type) of a stored key of a supported venue.
fn key_row(c: &Connection, api_key_id: i64) -> rusqlite::Result<Option<(i64, i64, String)>> {
    c.query_row("SELECT k.user_id, k.exchange_id, e.type FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.id = ?1", [api_key_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default()))).optional()
        .map(|row| row.filter(|(_, _, t)| VENUES.contains(&t.as_str())))
}

/// Records what an identity read of `api_key_id` (made with `credentials`, its current material) came to. Call inside the
/// write transaction that holds the credential fence, after the key's own row is written.
/// - A read id replaces the credential's observation; when the key is the one the sync reads with, it also fills the
///   account's unknown fingerprint.
/// - A failure only notes itself, and keeps the last observation unless the material changed (a real replacement).
///   Before any identity key exists it is noted without a material digest, so its backoff still holds and the job
///   reaches the credentials after it.
///
/// `version` is the ciphertext version the read was made under (the check's producer); it is stored only when it names
/// this key (a key saved for the first time was checked under a pending id, so the job re-reads it once).
#[allow(clippy::too_many_arguments)]
pub fn record(c: &Connection, cipher: &Cipher, api_key_id: i64, credentials: &Credentials, version: &crate::engine::model::CredentialVersion,
              observed: &Observed, now: DateTime<Utc>) -> Result<(), String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    if !ready(c).map_err(db)? { return Ok(()); }
    let Some((user_id, exchange_id, exchange_type)) = key_row(c, api_key_id).map_err(db)? else { return Ok(()) };
    let at = format_time(now);
    match observed {
        Observed::Read(id) => {
            let key = key(c, cipher, now)?;
            let (fingerprint, digest) = (key.fingerprint(&exchange_type, id), key.material_digest(credentials));
            let encrypted = cipher.encrypt(&id.0).into_bytes();
            let version = (version.id == api_key_id).then(|| version.cache_stamp().to_string());
            c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, material_digest, observed_credential_version, fingerprint, account_id_enc, observed_at)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (api_key_id) DO UPDATE SET exchange_id = excluded.exchange_id,
                       material_digest = excluded.material_digest, observed_credential_version = excluded.observed_credential_version, fingerprint = excluded.fingerprint,
                       account_id_enc = excluded.account_id_enc, observed_at = excluded.observed_at, last_failure_at = NULL, last_failure_code = NULL",
                      params![api_key_id, exchange_id, digest, version, fingerprint, encrypted, at]).map_err(db)?;
            let account = ensure_account(c, user_id, exchange_id, now).map_err(db)?;
            if reading_key(c, user_id, exchange_id)? == Some(api_key_id) { establish(c, account, &fingerprint, &encrypted, now).map_err(db)?; }
        }
        Observed::Failed(code) => {
            let Some(key) = existing_key(c, cipher) else {
                c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, last_failure_at, last_failure_code) VALUES (?1, ?2, ?3, ?4)
                           ON CONFLICT (api_key_id) DO UPDATE SET exchange_id = excluded.exchange_id, last_failure_at = excluded.last_failure_at, last_failure_code = excluded.last_failure_code",
                          params![api_key_id, exchange_id, at, code]).map_err(db)?;
                return Ok(());
            };
            let digest = key.material_digest(credentials);
            let stored: Option<Option<String>> = c.query_row("SELECT material_digest FROM exchange_account_credentials WHERE api_key_id = ?1", [api_key_id], |r| r.get(0)).optional().map_err(db)?;
            match stored {
                Some(Some(stored)) if stored == digest => c.execute("UPDATE exchange_account_credentials SET last_failure_at = ?2, last_failure_code = ?3 WHERE api_key_id = ?1",
                                                              params![api_key_id, at, code]),
                Some(_) => c.execute("UPDATE exchange_account_credentials SET exchange_id = ?2, material_digest = ?3, observed_credential_version = NULL, fingerprint = NULL,
                                      account_id_enc = NULL, observed_at = NULL, last_failure_at = ?4, last_failure_code = ?5 WHERE api_key_id = ?1",
                                     params![api_key_id, exchange_id, digest, at, code]),
                None => c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, material_digest, last_failure_at, last_failure_code) VALUES (?1, ?2, ?3, ?4, ?5)",
                                  params![api_key_id, exchange_id, digest, at, code]),
            }.map_err(db)?;
        }
    }
    Ok(())
}

/// A deleted key: its observation goes, and its account too when the user has no other key and no history there.
pub fn forget(c: &Connection, api_key_id: i64, user_id: i64, exchange_id: i64) -> rusqlite::Result<()> {
    if !ready(c)? { return Ok(()); }
    c.execute("DELETE FROM exchange_account_credentials WHERE api_key_id = ?1", [api_key_id])?;
    c.execute("DELETE FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2
               AND NOT EXISTS (SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2)
               AND NOT EXISTS (SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2)", [user_id, exchange_id])?;
    Ok(())
}

/// An import for (user, exchange), inside the import's own transaction: the account row exists afterwards; a venue-native
/// export's id fills its unknown fingerprint, and is recorded as an import observation either way.
pub fn record_import(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, id: Option<&AccountId>, now: DateTime<Utc>) -> Result<(), String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    if !ready(c).map_err(db)? { return Ok(()); }
    let account = ensure_account(c, user_id, exchange_id, now).map_err(db)?;
    let Some(id) = id else { return Ok(()) };
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get::<_, Option<String>>(0)).map_err(db)?.unwrap_or_default();
    let key = key(c, cipher, now)?;
    let fingerprint = key.fingerprint(&exchange_type, id);
    let encrypted = cipher.encrypt(&id.0).into_bytes();
    establish(c, account, &fingerprint, &encrypted, now).map_err(db)?;
    c.execute("INSERT INTO exchange_account_imports (user_id, exchange_id, fingerprint, account_id_enc, observed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
              params![user_id, exchange_id, fingerprint, encrypted, format_time(now)]).map_err(db)?;
    Ok(())
}

/// The account id of a venue-native export, read over every record before any is skipped: the first of `columns` the
/// header row has. Ok(None) for a file with no record at all (its import answers as before). A missing column, a blank,
/// masked or otherwise unreadable id on any record, or two different ids, is a malformed export. A line with no cells
/// at all is no record.
pub fn export_account_id(table: &crate::tracker::import::csv::Table, columns: &[&str]) -> Result<Option<AccountId>, crate::tracker::import::csv::Malformed> {
    use crate::tracker::import::csv::Malformed;
    let records: Vec<_> = table.rows.iter().filter(|row| !row.is_empty()).collect();
    if records.is_empty() { return Ok(None); }
    let Some(column) = columns.iter().find(|c| table.has_header(c)) else { return Err(Malformed("the export has no account id column".into())) };
    let mut found: Option<AccountId> = None;
    for row in records {
        let id = table.get(row, column).and_then(AccountId::parse).ok_or_else(|| Malformed("a record's account id is missing or masked".into()))?;
        match &found { Some(first) if *first != id => return Err(Malformed("the export holds more than one account".into())), Some(_) => {}, None => found = Some(id) }
    }
    Ok(found)
}

/// An exchange's account identity, derived from what is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// No fingerprint known, and at most one seen.
    Unknown,
    /// A known fingerprint, and every current credential seen agrees with it.
    Bound,
    /// A current credential belongs to another account than the known one, or, the account unknown, two credentials
    /// belong to different accounts.
    Conflicted,
}

/// The fingerprints of (user, exchange)'s current credentials (keys still stored), and the account's own.
fn seen(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<(Option<String>, Vec<String>)> {
    let account = c.query_row("SELECT fingerprint FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2", [user_id, exchange_id], |r| r.get(0)).optional()?.flatten();
    let mut s = c.prepare("SELECT DISTINCT o.fingerprint FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id
                           WHERE k.user_id = ?1 AND k.exchange_id = ?2 AND o.fingerprint IS NOT NULL ORDER BY 1")?;
    let observed = s.query_map([user_id, exchange_id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok((account, observed))
}

pub fn standing(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<Standing> {
    if !ready(c)? { return Ok(Standing::Unknown); }
    let (account, observed) = seen(c, user_id, exchange_id)?;
    Ok(match account {
        Some(fp) if observed.iter().any(|o| *o != fp) => Standing::Conflicted,
        Some(_) => Standing::Bound,
        None if observed.len() >= 2 => Standing::Conflicted,
        None => Standing::Unknown,
    })
}

/// One credential's identity, derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialStanding {
    /// No successful observation of its current material.
    Unobserved,
    /// Observed; the account is unknown.
    Seen,
    /// Observed as the account's own.
    Bound,
    /// Observed as another account than the known one.
    Conflict,
}

pub fn credential_standing(c: &Connection, api_key_id: i64) -> rusqlite::Result<CredentialStanding> {
    if !ready(c)? { return Ok(CredentialStanding::Unobserved); }
    let row: Option<(Option<String>, Option<String>)> = c.query_row(
        "SELECT o.fingerprint, a.fingerprint FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id
         LEFT JOIN exchange_accounts a ON a.user_id = k.user_id AND a.exchange_id = k.exchange_id WHERE o.api_key_id = ?1", [api_key_id],
        |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    Ok(match row {
        None | Some((None, _)) => CredentialStanding::Unobserved,
        Some((Some(_), None)) => CredentialStanding::Seen,
        Some((Some(own), Some(account))) => if own == account { CredentialStanding::Bound } else { CredentialStanding::Conflict },
    })
}

/// A credential the discovery job reads: its key, venue, material and the ciphertext version it was read under.
pub struct Due { pub api_key_id: i64, pub exchange_type: String, pub credentials: Credentials, pub version: crate::engine::model::CredentialVersion }

/// The credentials of a supported venue, every key type, due an observation at `now`: never observed (or not since its
/// material changed), its stored ciphertext changed since, or observed longer ago than `observe_every`, unless a read
/// failed within `retry_after`. A key whose values cannot be decrypted is left out.
pub fn due(c: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<Vec<Due>, String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    if !ready(c).map_err(db)? { return Ok(vec![]); }
    let key = existing_key(c, cipher);
    let mut s = c.prepare("SELECT k.id, e.type, o.material_digest, o.observed_credential_version, o.fingerprint, o.observed_at, o.last_failure_at
                           FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id LEFT JOIN exchange_account_credentials o ON o.api_key_id = k.id ORDER BY k.id").map_err(db)?;
    type Row = (i64, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(db)?
        .collect::<Result<Vec<Row>, _>>().map_err(db)?;
    let time = |t: &Option<String>| t.as_deref().and_then(|t| parse_time(t).ok());
    let mut out = vec![];
    for (id, exchange_type, digest, version, fingerprint, observed_at, failed_at) in rows {
        let Some(exchange_type) = exchange_type.filter(|t| VENUES.contains(&t.as_str())) else { continue };
        if time(&failed_at).is_some_and(|t| now - t < retry_after()) { continue; }
        let Ok((credentials, current, _)) = crate::sync::validation_material(c, cipher, id) else { continue };
        let material_same = matches!((&key, &digest), (Some(k), Some(d)) if k.material_digest(&credentials) == *d);
        let version_same = version.as_deref().and_then(|v| serde_json::from_str::<Value>(v).ok())
            .is_some_and(|stored| crate::engine::model::stamp_current_for(&stored, &current.cache_stamp()).is_fresh());
        let fresh = fingerprint.is_some() && time(&observed_at).is_some_and(|t| now - t < observe_every());
        if !(material_same && version_same && fresh) { out.push(Due { api_key_id: id, exchange_type, credentials, version: current }); }
        if out.len() == PER_RUN { break; }
    }
    Ok(out)
}

/// Records a discovery read, under the credential fence: nothing when the key changed or went since it was read.
pub fn publish(c: &Connection, cipher: &Cipher, due: &Due, observed: &Observed, now: DateTime<Utc>) -> Result<bool, String> {
    let tx = crate::engine::model::immediate(c).map_err(|e| format!("{e:?}"))?;
    if !crate::engine::model::credential_is_current(&tx, &due.version).map_err(|e| format!("{e:?}"))? { return Ok(false); }
    record(&tx, cipher, due.api_key_id, &due.credentials, &due.version, observed, now)?;
    tx.commit().map_err(|e: rusqlite::Error| e.to_string())?;
    Ok(true)
}

/// The discovery job: hourly, it observes the credentials `due` finds through each venue's key-check egress (its proxy
/// when one is configured). It survives restarts because its state is the observation rows.
pub struct Discovery<C: crate::sync::jobs::Connect> { pub venues: C }

impl<C: crate::sync::jobs::Connect> crate::jobs::Job for Discovery<C> {
    fn spec(&self) -> crate::jobs::Spec {
        crate::jobs::Spec { name: JOB, scope: None, schedule: Some(crate::jobs::schedule::Schedule::EveryHours { every: 1, minute: 23 }),
                            jitter: crate::jobs::schedule::Jitter::NONE, retry: crate::jobs::Retry::None, deadline: crate::jobs::DEADLINE }
    }
    fn run<'a>(&'a self, cx: crate::jobs::Cx<'a>, _wakes: Vec<crate::jobs::Wake>) -> crate::jobs::JobFuture<'a> {
        Box::pin(async move {
            use crate::jobs::Outcome;
            let now = cx.clock.now();
            let due = match cx.db.run(move |c, cipher| due(c, cipher, now)).await { Ok(due) => due, Err(e) => return Outcome::Failed(e) };
            if due.is_empty() { return Outcome::NothingNew; }
            for credential in due {
                let Some(proxy) = crate::venue::registry::key_check_proxy(&credential.exchange_type) else { continue };
                // As the key check (sync::jobs::Validator): a proxy setting that cannot be read sends nothing, never goes around it.
                let stored = cx.db.run(move |c, cipher| crate::app_config::get(c, cipher, &format!("proxy_{proxy}"))).await;
                let observed = match stored.map(|stored| self.venues.key_check_egress(&credential.exchange_type, stored)) {
                    Err(_) => Some(Observed::Failed("proxy unreadable".into())),
                    Ok(Ok((base, client))) => read_live(&credential.exchange_type, &base, client, &credential.credentials).await,
                    Ok(Err(_)) => Some(Observed::Failed("no egress".into())),
                };
                let Some(observed) = observed else { continue };
                let at = cx.clock.now();
                if let Err(e) = cx.db.run(move |c, cipher| publish(c, cipher, &credential, &observed, at)).await { return Outcome::Failed(e); }
            }
            Outcome::Done
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_canonical_decimal_text_and_nothing_else() {
        assert_eq!(AccountId::parse(" 0042 "), AccountId::parse("42"));
        for bad in ["", " ", "12****89", "1.5", "-3", "0", "abc"] { assert_eq!(AccountId::parse(bad), None, "{bad:?}"); }
        assert_eq!(format!("{:?}", AccountId::parse("165111215").unwrap()), "AccountId([redacted])");
    }

    #[test]
    fn derivation_is_deterministic_domain_separated_and_per_venue() {
        let key = IdentityKey([7; 32]);
        let id = AccountId::parse("165111215").unwrap();
        assert_eq!(key.fingerprint(KUCOIN, &id), key.fingerprint(KUCOIN, &AccountId::parse("0165111215").unwrap()));
        assert_eq!(key.fingerprint(KUCOIN, &id).len(), 32);
        assert_ne!(key.fingerprint(KUCOIN, &id), key.fingerprint(BINANCE, &id), "one id on two venues is two accounts");
        assert_ne!(key.fingerprint(KUCOIN, &id), IdentityKey([8; 32]).fingerprint(KUCOIN, &id), "another install, another fingerprint");
        let c = Credentials { key: "k".into(), secret: "s".into(), passphrase: None, redaction_values: vec![] };
        assert_ne!(key.material_digest(&c), key.mac("deltabadger-account-fingerprint-v1", &["k", "s", ""]), "the two derivations never meet");
        // Field boundaries are part of the material: "ab"+"c" is not "a"+"bc".
        let shifted = Credentials { key: "ks".into(), secret: "".into(), ..c.clone() };
        assert_ne!(key.material_digest(&c), key.material_digest(&shifted));
    }

    #[test]
    fn answers_yield_the_id_or_a_failure_without_body_text() {
        let kucoin = r#"{"code":"200000","data":{"uid":165111215,"apiKey":"the-key","remark":"r"}}"#;
        assert_eq!(kucoin_answer(200, kucoin), Observed::Read(AccountId::parse("165111215").unwrap()));
        assert_eq!(kucoin_answer(401, r#"{"code":"400003","msg":"KC-API-KEY not exists 165111215"}"#), Observed::Failed("http 401; code 400003".into()));
        assert_eq!(kucoin_answer(200, r#"{"code":"200000","data":{"uid":"16****15"}}"#), Observed::Failed("http 200".into()));
        assert_eq!(binance_answer(200, r#"{"uid":354937868,"balances":[]}"#), Observed::Read(AccountId::parse("354937868").unwrap()));
        assert_eq!(binance_answer(200, r#"{"code":-2015,"msg":"uid 354937868"}"#), Observed::Failed("http 200; code -2015".into()));
        assert_eq!(binance_answer(502, "<html>354937868</html>"), Observed::Failed("http 502".into()));
    }
}
