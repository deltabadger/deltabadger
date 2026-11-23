//! Exchange-account identity (plan AF, slice AF1): what account each connected credential and each identity-bearing
//! import belongs to. Recorded only: nothing here refuses or changes a key, a sync, a bot or an import.
//!
//! - The account id a venue reports is read at the transport boundary (`Transport::send_observed`), before the body is
//!   scrubbed of credentials, and only that one field leaves it (`AccountId`, whose Debug prints nothing).
//! - It is stored as a fingerprint (an HMAC under the install's `rust_account_identity_key_v1`) and encrypted in the
//!   credential envelope; never in plain text, never in a log.
//! - 2.x's encryption reset deletes every app_configs row: a missing (or unreadable) identity key while accounts exist
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
/// Rust's identity tables (rust/migrations/20261022000001_create_exchange_accounts.sql, 20261119000001_exchange_account_numbers.sql,
/// 20261119000002_exchange_account_ownership.sql, 20261120000001_exchange_account_ids.sql).
/// 7a: and the proofs and reader generations (20261125000001_account_fingerprint_proofs.sql).
pub const TABLES: [&str; 10] = ["exchange_accounts", "exchange_account_credentials", "exchange_account_imports", "exchange_account_keys", "exchange_account_bots",
                               "exchange_account_rules", "exchange_account_transactions", "exchange_account_rows", "account_fingerprint_proofs", "ledger_reader_generations"];
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

/// Whether a failed identity read is the venue's definite answer on the key's current material (#710): its auth refused
/// (401, 403), or an answer under a success status that names no account (masked, or an error envelope). A 5xx, a rate
/// limit (429, 418), any other status and the reads that reached no venue (`transport`, `no egress`, `proxy
/// unreadable`) say nothing about the account. `code` as `failure` writes it (`http <status>[; code <code>]`).
pub fn definite_failure(code: &str) -> bool {
    code.strip_prefix("http ").and_then(|rest| rest.split(';').next()).and_then(|s| s.trim().parse::<u16>().ok())
        .is_some_and(|status| (200..300).contains(&status) || status == 401 || status == 403)
}
/// SQL condition: failure code expression `code` is a definite failure (`definite_failure`).
fn definite_sql(code: &str) -> String {
    format!("({code} LIKE 'http 2__' OR {code} LIKE 'http 2__;%' OR {code} LIKE 'http 401' OR {code} LIKE 'http 401;%' OR {code} LIKE 'http 403' OR {code} LIKE 'http 403;%')")
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
    pub fn fingerprint(&self, exchange_type: &str, id: &AccountId) -> String { self.fingerprint_text(exchange_type, &id.0) }
    /// `fingerprint` of an id a venue spells with letters (IBKR's U…, DU…), as given.
    pub fn fingerprint_text(&self, exchange_type: &str, id: &str) -> String { self.mac("deltabadger-account-fingerprint-v1", &[exchange_type, id]) }
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
    let accounts: i64 = c.query_row(KNOWN, [], |r| r.get(0)).map_err(db)?;
    if stored || accounts > 0 {
        crate::engine::log(if stored { "account identity: the identity key cannot be read; known account identities are cleared" }
                           else { "account identity: the identity key is gone (an encryption reset); known account identities are cleared" });
    }
    c.execute_batch("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL;
                     DELETE FROM exchange_account_credentials; DELETE FROM exchange_account_imports;").map_err(db)?;
    // Every known identity is gone: so are the standings that rested on them (every account's key event).
    // A real reset of an existing identity (a key stored but unreadable, or identities known without one): each standing
    // user's whole-portfolio points go with the standings (`standing_changed`). The first key of an install resets nothing.
    if (stored || accounts > 0) && standings_ready(c).map_err(db)? { c.execute_batch("DELETE FROM portfolio_snapshots WHERE user_id IN (SELECT user_id FROM exchange_account_standings); DELETE FROM exchange_account_standings").map_err(db)?; }
    crate::app_config::delete(c, KEY_CONFIG)?;
    let bytes: [u8; 32] = rand::random();
    crate::app_config::set(c, cipher, KEY_CONFIG, &hex::encode(bytes), now)?;
    Ok(IdentityKey(bytes))
}

/// How many accounts hold an identity, and identity observations there are: what a reset has to clear. An eager account
/// row (#710) that never saw one is not counted.
const KNOWN: &str = "SELECT (SELECT count(*) FROM exchange_accounts WHERE fingerprint IS NOT NULL OR account_id_enc IS NOT NULL)
                          + (SELECT count(*) FROM exchange_account_credentials WHERE material_digest IS NOT NULL OR fingerprint IS NOT NULL)
                          + (SELECT count(*) FROM exchange_account_imports)";

/// At startup: a reset 2.x made while Rust was stopped is handled before anything is recorded. Writes nothing on an
/// install with a readable key or with nothing known. Then (#710 7a rule N7) every live account with no reader
/// generation yet (one from before the table) gets its reader recorded, so its first handoff counts as one.
pub fn initialize(c: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<(), String> {
    if !ready(c).map_err(|e| e.to_string())? { return Ok(()); }
    if existing_key(c, cipher).is_none() {
        let known: i64 = c.query_row(KNOWN, [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if known > 0 {
            let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
            key(&tx, cipher, now)?;
            tx.commit().map_err(|e| e.to_string())?;
            crate::store::mark_changed();
        }
    }
    // An install 2.x owns is not written before its claim (a refused start leaves it as it was); its takeover seeds them.
    if !crate::lease::read(c, cipher).ok().flatten().is_some_and(|v| v["engine"] == "rust") { return Ok(()); }
    crate::sync::seed_readers(c).map_err(|e| e.0)
}

/// The account (user, exchange) is read as while it has one (#710 slices 1-6): its lowest live number.
fn first_account(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<Option<(i64, Option<String>)>> {
    c.query_row("SELECT id, fingerprint FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND removed_at IS NULL ORDER BY number LIMIT 1",
                [user_id, exchange_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()
}

/// `first_account`'s id, created unknown when there is none (the next number, tombstones included: numbers are never
/// reused). Every write that first names a (user, exchange) calls it in its own transaction, so rows stay eager.
pub fn ensure_account(c: &Connection, user_id: i64, exchange_id: i64, now: DateTime<Utc>) -> rusqlite::Result<i64> {
    if let Some((id, _)) = first_account(c, user_id, exchange_id)? { return Ok(id); }
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
               SELECT ?1, ?2, coalesce(max(number), 0) + 1, ?3, ?3 FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2
               ON CONFLICT (user_id, exchange_id, number) DO NOTHING", params![user_id, exchange_id, format_time(now)])?;
    first_account(c, user_id, exchange_id)?.map(|(id, _)| id).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// A write that first names (user, exchange): its number-1 account exists afterwards. Nothing on an install whose
/// identity tables are absent (a 2.x install Rust has not migrated), as the rest of this module.
pub fn touch(c: &Connection, user_id: i64, exchange_id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    if ready(c)? { ensure_account(c, user_id, exchange_id, now)?; }
    Ok(())
}

/// The migration's eager rows (20261119000001), again: number 1 for every (user, exchange) a table names and no account
/// has (2.x's balances and venue snapshots: `sync::balance_store::refill_number_one`, which reads them). Run in the takeover's claim after 2.x owned the install (its keys, bots and rules name exchanges Rust never saw).
pub const FILL: &str = "INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT user_id, exchange_id, 1, ?1, ?1 FROM (
    SELECT user_id, exchange_id FROM api_keys
    UNION SELECT user_id, exchange_id FROM bots
    UNION SELECT b.user_id, t.exchange_id FROM transactions t JOIN bots b ON b.id = t.bot_id
    UNION SELECT user_id, exchange_id FROM rules
    UNION SELECT user_id, exchange_id FROM account_transactions
    UNION SELECT user_id, exchange_id FROM exchange_account_imports
    UNION SELECT user_id, exchange_id FROM import_rows
    UNION SELECT user_id, exchange_id FROM import_cutoffs
    UNION SELECT o.user_id, s.exchange_id FROM rust_api_order_standalone s JOIN rust_api_orders o ON o.id = s.order_id
  ) n WHERE user_id IS NOT NULL AND exchange_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = n.user_id AND a.exchange_id = n.exchange_id)
  ORDER BY user_id, exchange_id";
/// `FILL`: the number of accounts it made.
pub fn fill(c: &Connection, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    if !ready(c)? { return Ok(0); }
    c.execute(FILL, [format_time(now)])
}

// ---- The resolver (#710 §3): membership row if present, else the number-1 account of the subject's (user, exchange).
// A membership row naming another user's or exchange's account resolves to nothing.

/// SQL: the account of row `alias` of `api_keys` or `bots` (`membership`, `column`: its membership table and column), NULL
/// when it resolves to none (a foreign membership row, or a number 1 that has no row yet).
fn account_sql(membership: &str, column: &str, alias: &str) -> String {
    format!("(SELECT a.id FROM exchange_accounts a LEFT JOIN {membership} m ON m.{column} = {alias}.id
              WHERE a.user_id = {alias}.user_id AND a.exchange_id = {alias}.exchange_id
              AND CASE WHEN m.account_id IS NULL THEN a.number = 1 ELSE a.id = m.account_id END)")
}
/// A unit test's in-memory install: the account tables, each key's number 1 made as it is inserted (rows are eager), and
/// the balance and venue snapshot stores' `account_id`, the columns the readers name.
#[doc(hidden)]
pub const TEST_ACCOUNT_TABLES: &str = "CREATE TABLE exchange_accounts(id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, number INTEGER,
      fingerprint TEXT, account_id_enc BLOB, removed_at TEXT, created_at TEXT, updated_at TEXT);
    CREATE TABLE exchange_account_keys(api_key_id INTEGER PRIMARY KEY, account_id INTEGER);
    CREATE TABLE exchange_account_rows(account_transaction_id INTEGER PRIMARY KEY, account_id INTEGER, venue_tx_id TEXT);
    CREATE TRIGGER test_eager_accounts AFTER INSERT ON api_keys BEGIN
      INSERT INTO exchange_accounts(user_id, exchange_id, number) SELECT NEW.user_id, NEW.exchange_id, 1
      WHERE NOT EXISTS (SELECT 1 FROM exchange_accounts WHERE user_id = NEW.user_id AND exchange_id = NEW.exchange_id AND number = 1); END;";

/// Whether accounts are numbered on this install (#710 slice 1 migrated): an install without it reads every exchange's one
/// account as its number 1's stand-in (`AccountRow::unplaced`).
pub fn numbered(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('exchange_accounts') WHERE name = 'number')", [], |r| r.get(0))
}

/// SQL: api key row `alias`'s account id (NULL as the resolver reads it: a foreign membership, a number 1 with no row).
pub fn key_account_sql(alias: &str) -> String { account_sql("exchange_account_keys", "api_key_id", alias) }
/// SQL: bot row `alias`'s account id, as `key_account_sql`.
pub fn bot_account_sql(alias: &str) -> String { account_sql("exchange_account_bots", "bot_id", alias) }
/// `with_key_account` for bot row `alias`.
pub fn with_bot_account<T>(alias: &str, read: impl Fn(&str) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    unscoped_or(read(&bot_account_sql(alias)), || read("NULL"))
}

// ---- Names (#710 §4.9, PLAN-710-6 §3): `Kraken`, or `Kraken 2` / `Kraken<sup>2</sup>` while its exchange has 2+ live
// accounts. A removed account always shows its number.

/// An account's name: its exchange's, and the number it shows (None: the exchange has one live account).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label { pub name: String, pub number: Option<i64> }

impl Label {
    /// The name alone (an exchange, or an account that shows no number).
    pub fn bare(name: impl Into<String>) -> Self { Self { name: name.into(), number: None } }
    /// Plain text: `Kraken`, `Kraken 2` (Q2).
    pub fn plain(&self) -> String { match self.number { Some(n) => format!("{} {n}", self.name), None => self.name.clone() } }
    /// HTML: the escaped name, then `<sup>n</sup>`.
    pub fn html(&self) -> String {
        let name = crate::web::i18n::escape(&self.name);
        match self.number { Some(n) => format!("{name}<sup>{n}</sup>"), None => name }
    }
}

/// SQL: the number account `account` (an id expression) shows in its label, NULL for none. `"NULL"` (an install without
/// the account tables, or no account) stays NULL and names no table.
pub fn label_number_sql(account: &str) -> String {
    if account == "NULL" { return "NULL".into(); }
    format!("(SELECT CASE WHEN lbl_a.removed_at IS NOT NULL OR (SELECT count(*) FROM exchange_accounts lbl_o WHERE lbl_o.user_id = lbl_a.user_id \
             AND lbl_o.exchange_id = lbl_a.exchange_id AND lbl_o.removed_at IS NULL) >= 2 THEN lbl_a.number END FROM exchange_accounts lbl_a WHERE lbl_a.id = {account})")
}
/// SQL: `Label::plain` of the account `account` whose exchange is named `name` (both SQL expressions).
pub fn plain_label_sql(name: &str, account: &str) -> String {
    if account == "NULL" { return name.into(); }
    format!("({name} || COALESCE(' ' || {}, ''))", label_number_sql(account))
}
/// D-API-ACCOUNT-FIELDS (PLAN-710-6 §4.4): an account-owned REST/MCP object gains `account_id` and `account` (its number)
/// beside its unchanged `exchange`, only while the number shows (`label_number_sql`): a single-account answer keeps its bytes.
pub fn account_fields(object: &mut Value, account: Option<i64>, number: Option<i64>) {
    if let (Some(id), Some(n), Some(o)) = (account, number, object.as_object_mut()) {
        o.insert("account_id".into(), id.into());
        o.insert("account".into(), n.into());
    }
}
/// A REST/MCP text's venue: `exchange`, then the object's `account` number (`account_fields`).
pub fn venue_text(object: &Value, exchange: &str) -> String {
    match object["account"].as_i64() { Some(n) => format!("{exchange} {n}"), None => exchange.to_string() }
}
/// Account `account_id`'s label. One statement.
pub fn label(c: &Connection, account_id: i64) -> rusqlite::Result<Label> {
    c.query_row(&format!("SELECT e.name, {} FROM exchange_accounts x JOIN exchanges e ON e.id = x.exchange_id WHERE x.id = ?1", label_number_sql("x.id")), [account_id],
                |r| Ok(Label { name: r.get::<_, Option<String>>(0)?.unwrap_or_default(), number: r.get(1)? }))
}
/// The label of every account of the user, by id. One statement (none on an install without the account tables).
pub fn labels(c: &Connection, user_id: i64) -> rusqlite::Result<std::collections::HashMap<i64, Label>> {
    if !numbered(c)? { return Ok(Default::default()); }
    let mut s = c.prepare(&format!("SELECT x.id, e.name, {} FROM exchange_accounts x JOIN exchanges e ON e.id = x.exchange_id WHERE x.user_id = ?1", label_number_sql("x.id")))?;
    let out = s.query_map([user_id], |r| Ok((r.get(0)?, Label { name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), number: r.get(2)? })))?.collect();
    out
}
/// Account `account`'s label, or the bare exchange `name` for a number 1 with no row yet (it is its exchange's only one).
pub fn label_or(c: &Connection, account: Option<i64>, name: &str) -> rusqlite::Result<Label> {
    match account { Some(id) if numbered(c)? => label(c, id), _ => Ok(Label::bare(name)) }
}
/// `read` given the SQL of api key row `alias`'s account (`account_of_key`, NULL as `account_sql`), run again with NULL for
/// every key on an install without the account tables. One statement on a migrated install (page read budgets count them).
pub fn with_key_account<T>(alias: &str, read: impl Fn(&str) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    unscoped_or(read(&account_sql("exchange_account_keys", "api_key_id", alias)), || read("NULL"))
}
/// `scoped`'s answer to a read that named the account tables: on an install without them (one `run`/`serve` has not
/// migrated, a 2.x install) the read again by (user, exchange), as before #710.
fn unscoped_or<T>(scoped: rusqlite::Result<T>, unscoped: impl FnOnce() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    match scoped {
        Err(e) if e.to_string().contains("no such table: exchange_account") => unscoped(),
        out => out,
    }
}

/// An account as key and bot lookups scope by it: its row's `id`, None for the number 1 of (user, exchange) while no row
/// exists yet (a key or bot 2.x saved; `FILL` writes the row at the next takeover). On an install without the membership
/// tables (`scoped`) it is (user, exchange) itself and writes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Account { pub user_id: i64, pub exchange_id: i64, pub id: Option<i64>, scoped: bool }

impl Account {
    /// The account (user, exchange) means while it has one (`first_account`).
    pub fn first(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<Self> {
        unscoped_or(first_account(c, user_id, exchange_id).map(|row| Self { user_id, exchange_id, id: row.map(|(id, _)| id), scoped: true }),
                    || Ok(Self { user_id, exchange_id, id: None, scoped: false }))
    }
    /// Api key `api_key_id`'s. None: no such key, or its membership row names another user's or exchange's account.
    pub fn of_key(c: &Connection, api_key_id: i64) -> rusqlite::Result<Option<Self>> { Self::of(c, "api_keys", "exchange_account_keys", "api_key_id", api_key_id) }
    /// Bot `bot_id`'s. None also for a bot with no exchange.
    pub fn of_bot(c: &Connection, bot_id: i64) -> rusqlite::Result<Option<Self>> { Self::of(c, "bots", "exchange_account_bots", "bot_id", bot_id) }
    fn of(c: &Connection, subject: &str, membership: &str, column: &str, id: i64) -> rusqlite::Result<Option<Self>> {
        let account = account_sql(membership, column, "s");
        let scoped = c.query_row(&format!("SELECT s.user_id, s.exchange_id, {account},
                              s.exchange_id IS NOT NULL AND (NOT EXISTS (SELECT 1 FROM {membership} WHERE {column} = s.id) OR {account} IS NOT NULL)
                              FROM {subject} s WHERE s.id = ?1"),
                    [id], |r| r.get::<_, bool>(3)?.then(|| Ok(Self { user_id: r.get(0)?, exchange_id: r.get(1)?, id: r.get(2)?, scoped: true })).transpose())
            .optional().map(Option::flatten);
        unscoped_or(scoped, || c.query_row(&format!("SELECT user_id, exchange_id FROM {subject} WHERE id = ?1 AND exchange_id IS NOT NULL"), [id],
                                           |r| Ok(Self { user_id: r.get(0)?, exchange_id: r.get(1)?, id: None, scoped: false })).optional())
    }
    /// Withdrawal rule `rule_id`'s (exchange_account_rules). None: no such rule, or its account is another user's or exchange's.
    pub fn of_rule(c: &Connection, rule_id: i64) -> rusqlite::Result<Option<Self>> {
        c.query_row("SELECT r.user_id, r.exchange_id, r.account_id FROM exchange_account_rules r JOIN exchange_accounts a ON a.id = r.account_id
                     AND a.user_id = r.user_id AND a.exchange_id = r.exchange_id WHERE r.id = ?1", [rule_id],
                    |r| Ok(Self { user_id: r.get(0)?, exchange_id: r.get(1)?, id: Some(r.get(2)?), scoped: true })).optional()
    }
    /// Bot order `transaction_id`'s (exchange_account_transactions, else the number 1 of its bot's user and its own exchange):
    /// the account it was placed on, whatever its bot trades on now. None: no such order, or its membership names another
    /// user's or exchange's account.
    pub fn of_order(c: &Connection, transaction_id: i64) -> rusqlite::Result<Option<Self>> {
        let scoped = c.query_row("SELECT b.user_id, t.exchange_id, m.account_id, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = b.user_id AND a.exchange_id = t.exchange_id
                                    AND CASE WHEN m.account_id IS NULL THEN a.number = 1 ELSE a.id = m.account_id END)
                                  FROM transactions t JOIN bots b ON b.id = t.bot_id LEFT JOIN exchange_account_transactions m ON m.transaction_id = t.id
                                  WHERE t.id = ?1 AND t.exchange_id IS NOT NULL", [transaction_id],
                                 |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?))).optional()
            .map(|row| row.and_then(|(user_id, exchange_id, member, id)| (member.is_none() || id.is_some()).then_some(Self { user_id, exchange_id, id, scoped: true })));
        unscoped_or(scoped, || c.query_row("SELECT b.user_id, t.exchange_id FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.id = ?1 AND t.exchange_id IS NOT NULL",
                                           [transaction_id], |r| Ok(Self { user_id: r.get(0)?, exchange_id: r.get(1)?, id: None, scoped: false })).optional())
    }
    /// SQL condition: order row `alias` of `transactions` (of one of this user's bots) was placed on this account.
    pub fn orders(&self, c: &Connection, alias: &str) -> rusqlite::Result<String> {
        if !self.scoped { return Ok(format!("({alias}.exchange_id = {})", self.exchange_id)); }
        let member = match self.id { Some(id) if self.number(c)? != 1 => id.to_string(), _ => "NULL".into() };
        Ok(format!("({alias}.exchange_id = {} AND (SELECT m.account_id FROM exchange_account_transactions m WHERE m.transaction_id = {alias}.id) IS {member})", self.exchange_id))
    }
    /// Account row `id` (live or not). None: no such row.
    pub fn by_id(c: &Connection, id: i64) -> rusqlite::Result<Option<Self>> {
        if !ready(c)? { return Ok(None); }
        c.query_row("SELECT user_id, exchange_id FROM exchange_accounts WHERE id = ?1", [id], |r| Ok(Self { user_id: r.get(0)?, exchange_id: r.get(1)?, id: Some(id), scoped: true })).optional()
    }
    /// This account with its row: `ensure_account`'s when it has none yet. Call in the write's own transaction. A removed
    /// account (a Merge tombstoned it after the write was validated) is refused: nothing may be written to it.
    pub fn ensure(self, c: &Connection, now: DateTime<Utc>) -> rusqlite::Result<Self> {
        if let Some(id) = self.id.filter(|_| self.scoped) {
            let removed: bool = c.query_row("SELECT removed_at IS NOT NULL FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)).optional()?.unwrap_or(false);
            if removed { return Err(rusqlite::Error::InvalidParameterName(REMOVED.into())); }
        }
        if self.id.is_some() || !self.scoped { return Ok(self); }
        Ok(Self { id: Some(ensure_account(c, self.user_id, self.exchange_id, now)?), ..self })
    }
    /// Its number; a number 1 with no row yet is 1.
    pub fn number(&self, c: &Connection) -> rusqlite::Result<i64> {
        match self.id { Some(id) => c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)), None => Ok(1) }
    }
    /// SQL condition: api key row `alias` is one of this account's keys.
    pub fn keys(&self, alias: &str) -> String { self.condition(&account_sql("exchange_account_keys", "api_key_id", alias), alias) }
    /// SQL condition: bot row `alias` is one of this account's bots.
    pub fn bots(&self, alias: &str) -> String { self.condition(&account_sql("exchange_account_bots", "bot_id", alias), alias) }
    fn condition(&self, account: &str, alias: &str) -> String {
        if !self.scoped { return format!("({alias}.user_id = {} AND {alias}.exchange_id = {})", self.user_id, self.exchange_id); }
        let id = self.id.map_or_else(|| "NULL".to_string(), |id| id.to_string());
        format!("({alias}.user_id = {} AND {alias}.exchange_id = {} AND {account} IS {id})", self.user_id, self.exchange_id)
    }
    /// Its key of `key_type`: the lowest id (`Key::read`'s ORDER BY id LIMIT 1).
    pub fn key(&self, c: &Connection, key_type: i64) -> rusqlite::Result<Option<i64>> {
        c.query_row(&format!("SELECT k.id FROM api_keys k WHERE {} AND k.key_type = ?1 ORDER BY k.id LIMIT 1", self.keys("k")), [key_type], |r| r.get(0)).optional()
    }
    /// Api key `api_key_id`, just inserted, joins this account: a membership row, written only for a number other than 1
    /// (a key with none is number 1's).
    pub fn add_key(&self, c: &Connection, api_key_id: i64) -> rusqlite::Result<()> {
        let Some(id) = self.id else { return Ok(()) };
        // A key event of this account (a key saved, or one a Merge moves in).
        key_event(c, id)?;
        if self.number(c)? != 1 { c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [api_key_id, id])?; }
        Ok(())
    }
}

/// #710 slice 5 (PLAN-710-5 §5): an account as the tracker and the balance stores read it: the unit of balances, venue
/// snapshots, freshness and provenance. A number 1 with no row yet (`Account::id` None: a key 2.x saved, before the
/// takeover's FILL) reads as `id` = minus its exchange id: its keys and ledger rows are the number-1 ones, it holds no
/// stored row (every store write makes the row first), and its provenance key is number 1's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccountRow { pub id: i64, pub user_id: i64, pub exchange_id: i64, pub number: i64 }

impl AccountRow {
    const COLUMNS: &'static str = "id, user_id, exchange_id, number";
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> { Ok(Self { id: r.get(0)?, user_id: r.get(1)?, exchange_id: r.get(2)?, number: r.get(3)? }) }
    /// Account row `id` (live or not).
    pub fn by_id(c: &Connection, id: i64) -> rusqlite::Result<Option<Self>> {
        if !numbered(c)? { return Ok(None); }
        c.query_row(&format!("SELECT {} FROM exchange_accounts WHERE id = ?1", Self::COLUMNS), [id], Self::read).optional()
    }
    /// `account`'s row; a number 1 no write has made yet as its stand-in (none on an install without the account tables).
    pub fn of(c: &Connection, account: &Account) -> rusqlite::Result<Option<Self>> {
        match account.id {
            Some(id) => Self::by_id(c, id),
            // Also on an install without the account tables (`scoped` false): every exchange's one account is its number 1.
            None => Ok(Some(Self::unplaced(account.user_id, account.exchange_id))),
        }
    }
    /// The stand-in of (user, exchange)'s number 1 while it has no row.
    pub fn unplaced(user_id: i64, exchange_id: i64) -> Self { Self { id: -exchange_id, user_id, exchange_id, number: 1 } }
    /// Whether this is a stand-in (`unplaced`), which no store row names.
    pub fn placed(&self) -> bool { self.id > 0 }
    /// Api key `api_key_id`'s account's row.
    pub fn of_key(c: &Connection, api_key_id: i64) -> rusqlite::Result<Option<Self>> {
        match Account::of_key(c, api_key_id)? { Some(a) => Self::of(c, &a), None => Ok(None) }
    }
    /// Api key `api_key_id`'s account's row, made (as `Account::ensure`) where its number 1 has none. Call in the write's
    /// own transaction.
    pub fn ensure_of_key(c: &Connection, api_key_id: i64, now: DateTime<Utc>) -> rusqlite::Result<Option<Self>> {
        match Account::of_key(c, api_key_id)? { Some(a) => Self::of(c, &a.ensure(c, now)?), None => Ok(None) }
    }
    /// The number 1 of (user, exchange), when it has its row.
    pub fn first(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<Option<Self>> {
        if !numbered(c)? { return Ok(None); }
        c.query_row(&format!("SELECT {} FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND number = 1", Self::COLUMNS), [user_id, exchange_id], Self::read).optional()
    }
    /// The user's accounts whose ids `sql` (a SELECT of account ids, `?1` the user) names, by id; a stand-in's id (minus
    /// its exchange id) as that stand-in, after them. One statement.
    pub fn listed(c: &Connection, user_id: i64, sql: &str) -> rusqlite::Result<Vec<Self>> {
        let standins = "SELECT DISTINCT x.id, ?1 AS user_id, -x.id AS exchange_id, 1 AS number FROM x WHERE x.id < 0";
        let rows = if numbered(c)? { format!("SELECT {} FROM exchange_accounts WHERE user_id = ?1 AND id IN (SELECT id FROM x) UNION ALL {standins}", Self::COLUMNS) } else { standins.to_string() };
        let mut s = c.prepare(&format!("WITH x(id) AS ({sql}) SELECT * FROM ({rows}) ORDER BY id < 0, abs(id)"))?;
        let out = s.query_map([user_id], Self::read)?.collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }
    /// The resolver's form of this account (key and bot conditions).
    pub fn account(&self) -> Account { Account { user_id: self.user_id, exchange_id: self.exchange_id, id: self.placed().then_some(self.id), scoped: true } }
    /// Its ledger rows' and bot orders' scope (venue_ids).
    pub fn venue_ids(&self) -> crate::venue_ids::Scope { crate::venue_ids::Scope::of_row(self) }
    /// SQL condition: api key row `alias` is one of this account's keys. A stand-in's are its exchange's (a number 1 with no
    /// row has every key there; rows are eager), with no account table named, so it reads on an install without them.
    pub fn keys(&self, alias: &str) -> String {
        if self.placed() { self.account().keys(alias) } else { format!("({alias}.user_id = {} AND {alias}.exchange_id = {})", self.user_id, self.exchange_id) }
    }
}

/// Bot `bot_id` of (`user_id`, `exchange_id`), just inserted or just moved to that exchange, belongs to `account` (None: the
/// exchange's number 1) from now on, in the write's own transaction: the account row exists afterwards (rows stay eager) and
/// a membership row only for a number other than 1. An account of another user or exchange is refused. Nothing on an
/// install without the account tables.
pub fn place_bot(c: &Connection, bot_id: i64, user_id: i64, exchange_id: i64, account: Option<Account>, now: DateTime<Utc>) -> rusqlite::Result<()> {
    if !ready(c)? { return Ok(()); }
    let account = match account {
        Some(a) if a.user_id == user_id && a.exchange_id == exchange_id => a,
        Some(_) => return Err(rusqlite::Error::InvalidParameterName("an account of another user or exchange".into())),
        None => Account::first(c, user_id, exchange_id)?,
    }.ensure(c, now)?;
    // The bot's former account's row goes: the bot moves, as a whole, to this account.
    c.execute("DELETE FROM exchange_account_bots WHERE bot_id = ?1", [bot_id])?;
    let Some(id) = account.id else { return Ok(()) };
    if account.number(c)? != 1 { c.execute("INSERT INTO exchange_account_bots (bot_id, account_id) VALUES (?1, ?2)", [bot_id, id])?; }
    Ok(())
}

/// `Account::ensure`'s refusal of a removed account.
pub const REMOVED: &str = "the account was removed";

/// The account an order of bot `bot_id` is placed on when its intent names none: the bot's, as its venue ids are scoped
/// (venue_ids). A bot whose account resolves to none has no key either (`bot_keys`): its rows are number 1's form.
pub fn order_scope(c: &Connection, bot_id: i64) -> rusqlite::Result<crate::venue_ids::Scope> {
    match crate::venue_ids::Scope::of_bot(c, bot_id)? {
        Some(scope) => Ok(scope),
        None => c.query_row("SELECT user_id, exchange_id FROM bots WHERE id = ?1", [bot_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .and_then(|(u, e)| crate::venue_ids::Scope::first(c, u, e)),
    }
}

/// The scope of account `account_id` for an order of bot `bot_id` (the account its intent was placed on). An account of
/// another user or exchange than the bot's is refused.
pub fn order_scope_on(c: &Connection, bot_id: i64, account_id: i64) -> rusqlite::Result<crate::venue_ids::Scope> {
    let matches: bool = c.query_row("SELECT a.user_id = b.user_id AND a.exchange_id = b.exchange_id FROM exchange_accounts a, bots b WHERE a.id = ?1 AND b.id = ?2",
                                    [account_id, bot_id], |r| r.get(0))?;
    if !matches { return Err(rusqlite::Error::InvalidParameterName("an account of another user or exchange than the order's".into())); }
    crate::venue_ids::Scope::of(c, &Account::by_id(c, account_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?)
}

/// SQL condition: bot row `b` trades on bot `bot_id`'s account (false when that resolves to none).
pub fn sibling_bots(c: &Connection, bot_id: i64) -> rusqlite::Result<String> {
    Ok(Account::of_bot(c, bot_id)?.map_or_else(|| "0".to_string(), |account| account.bots("b")))
}

/// SQL condition: api key row `alias` belongs to bot `bot_id`'s account (false when it resolves to none).
pub fn bot_keys(c: &Connection, bot_id: i64, alias: &str) -> rusqlite::Result<String> {
    Ok(Account::of_bot(c, bot_id)?.map_or_else(|| "0".to_string(), |account| account.keys(alias)))
}

/// The account of api key `api_key_id`.
pub fn account_of_key(c: &Connection, api_key_id: i64) -> rusqlite::Result<Option<i64>> { Ok(Account::of_key(c, api_key_id)?.and_then(|a| a.id)) }
/// The account of bot `bot_id`.
pub fn account_of_bot(c: &Connection, bot_id: i64) -> rusqlite::Result<Option<i64>> { Ok(Account::of_bot(c, bot_id)?.and_then(|a| a.id)) }

/// The account of withdrawal rule `rule_id`.
pub fn account_of_rule(c: &Connection, rule_id: i64) -> rusqlite::Result<Option<i64>> { Ok(Account::of_rule(c, rule_id)?.and_then(|a| a.id)) }

/// The key of `key_type` that belongs to account `account_id` (the lowest id, as `Key::read`'s LIMIT 1).
pub fn key_of(c: &Connection, account_id: i64, key_type: i64) -> rusqlite::Result<Option<i64>> {
    match Account::by_id(c, account_id)? { Some(account) => account.key(c, key_type), None => Ok(None) }
}

/// The live accounts of (user, exchange): (id, number), by number.
pub fn accounts_of(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
    if !ready(c)? { return Ok(vec![]); }
    let mut s = c.prepare("SELECT id, number FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND removed_at IS NULL ORDER BY number")?;
    let out = s.query_map([user_id, exchange_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(out)
}

/// What a write that names an exchange (and maybe an account) of the user acts on (PLAN-710-6 §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    One(Account),
    /// The posted account is not a live account of this user and exchange: a 404 like a foreign bot id.
    NotFound,
    /// No account posted and the exchange has 2+ live accounts: each one's label and id, by number.
    Ambiguous(Vec<(Label, i64)>),
}

impl Pick {
    /// The REST/MCP refusal of an `Ambiguous` pick (English, like the services' other refusals).
    pub fn required(exchange: &str, accounts: &[(Label, i64)]) -> String {
        let listed = accounts.iter().map(|(l, id)| format!("{} (account_id {id})", l.plain())).collect::<Vec<_>>().join(", ");
        format!("{exchange} has {} accounts: {listed}. Name one with account_id.", accounts.len())
    }
}

/// The one resolver of a posted (user, exchange, account): the posted account when it is a live one of theirs, else
/// `NotFound`; with none posted, the exchange's account while it has at most one live account, else `Ambiguous`.
pub fn pick(c: &Connection, user_id: i64, exchange_id: i64, posted: Option<i64>) -> rusqlite::Result<Pick> {
    if let Some(id) = posted {
        if !ready(c)? { return Ok(Pick::NotFound); }
        let live: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM exchange_accounts WHERE id = ?1 AND user_id = ?2 AND exchange_id = ?3 AND removed_at IS NULL)",
                                     [id, user_id, exchange_id], |r| r.get(0))?;
        return Ok(if live { Account::by_id(c, id)?.map_or(Pick::NotFound, Pick::One) } else { Pick::NotFound });
    }
    let live = accounts_of(c, user_id, exchange_id)?;
    if live.len() < 2 { return Ok(Pick::One(Account::first(c, user_id, exchange_id)?)); }
    Ok(Pick::Ambiguous(live.into_iter().map(|(id, _)| Ok((label(c, id)?, id))).collect::<rusqlite::Result<_>>()?))
}

/// What a bot write's posted exchange and account do to the bot's account (PLAN-710-6 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// The bot stays on its account.
    Keep,
    To(Account),
    /// A posted account that is not a live one of the owner, or not of the posted exchange: a 404.
    NotFound,
    /// Another exchange posted alone, which has 2+ live accounts.
    Ambiguous(Vec<(Label, i64)>),
}

impl Switch {
    /// The account the bot is on after the switch, when it moves.
    pub fn to(&self) -> Option<Account> { match self { Self::To(a) => Some(*a), _ => None } }
}

/// The one resolver of a bot write's switch, run before anything is written, in order: a posted account (a live one of
/// `owner`, of the posted exchange when one is posted too; `Keep` when it is the bot's own); the bot's own exchange alone;
/// another exchange alone (its account, or `Ambiguous` with 2+ live); nothing posted.
pub fn pick_switch(c: &Connection, owner: i64, bot_id: i64, posted_exchange: Option<i64>, posted_account: Option<i64>) -> rusqlite::Result<Switch> {
    let own_exchange: Option<i64> = c.query_row("SELECT exchange_id FROM bots WHERE id = ?1", [bot_id], |r| r.get(0)).optional()?.flatten();
    if let Some(id) = posted_account {
        if !ready(c)? { return Ok(Switch::NotFound); }
        let found: Option<i64> = c.query_row("SELECT exchange_id FROM exchange_accounts WHERE id = ?1 AND user_id = ?2 AND removed_at IS NULL", [id, owner], |r| r.get(0)).optional()?;
        let Some(exchange) = found.filter(|e| posted_exchange.is_none_or(|p| p == *e)) else { return Ok(Switch::NotFound) };
        let Some(account) = Account::by_id(c, id)? else { return Ok(Switch::NotFound) };
        let own = Account::of_bot(c, bot_id)?;
        let number = account.number(c)?;
        return Ok(if own.is_some_and(|a| a.id == Some(id) || (a.id.is_none() && a.exchange_id == exchange && number == 1)) { Switch::Keep } else { Switch::To(account) });
    }
    match posted_exchange {
        None => Ok(Switch::Keep),
        Some(e) if Some(e) == own_exchange => Ok(Switch::Keep),
        Some(e) => Ok(match pick(c, owner, e, None)? { Pick::One(a) => Switch::To(a), Pick::Ambiguous(list) => Switch::Ambiguous(list), Pick::NotFound => Switch::NotFound }),
    }
}

// ---- Duplicate standing (#710 §4.8, PLAN-710-6 §5.1): two live accounts of one user and exchange found to be one venue
// account. Derived when read, never stored: the higher number is the duplicate of the lowest live account its links reach.

/// The venues whose trade, fill or ledger ids are venue-native and unique across accounts (PLAN-710-6 §5.1's table). A
/// venue not listed gives no overlap evidence; KuCoin, Binance and Binance.US report an account id (`VENUES`) instead.
/// BingX stays out until its trade id's scope is vetted.
pub const OVERLAP_VENUES: [&str; 10] = ["Exchanges::Kraken", "Exchanges::Coinbase", "Exchanges::Bitvavo", "Exchanges::Gemini", "Exchanges::Bitget",
                                        "Exchanges::Mexc", "Exchanges::Hyperliquid", "Exchanges::Bybit", "Exchanges::Bitrue", "Exchanges::Alpaca"];
/// Shared trades two accounts need.
// ponytail: one fixed bar of 3 (one identical fill under a venue-native id is near-certain; three guard against one
// replayed import row); raise it per venue if a venue is found reusing ids.
pub const MATCHES: usize = 3;
/// The duplicate's earliest trade rows that are compared.
pub const SAMPLE: i64 = 200;
/// Two accounts' latest balance reads at most this many seconds apart are comparable.
pub const READS_APART: i64 = 900;
/// Two amounts agree within the venues' rounding: a relative difference at most this.
// ponytail: one bar for every venue (eight decimals); per-venue rounding if one is found rounding coarser.
const ROUNDING: f64 = 1e-8;

/// What two accounts' balances say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reads { Agree, Differ, NotComparable }

fn eng(e: crate::engine::EngineError) -> rusqlite::Error { rusqlite::Error::InvalidParameterName(format!("{e:?}")) }

/// Whether this install stores standings (`exchange_account_standings`).
fn standings_ready(c: &Connection) -> rusqlite::Result<bool> {
    Ok(numbered(c)? && crate::engine::rules::table_exists(c, "exchange_account_standings")?)
}

/// The user's duplicates and their survivors, (duplicate, survivor), by duplicate id: the stored standing
/// (`exchange_account_standings`), which `reconsider` alone writes and every key event clears.
pub fn duplicates(c: &Connection, user_id: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
    if !standings_ready(c)? { return Ok(vec![]); }
    let mut q = c.prepare("SELECT duplicate_id, survivor_id FROM exchange_account_standings WHERE user_id = ?1 ORDER BY duplicate_id")?;
    let out = q.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect();
    out
}

/// The stored standing of duplicate `duplicate` of user `user_id`: its survivor, basis (`standing_basis` at entry) and
/// when it was entered.
pub fn standing_of(c: &Connection, user_id: i64, duplicate: i64) -> rusqlite::Result<Option<(i64, String, String)>> {
    if !standings_ready(c)? { return Ok(None); }
    c.query_row("SELECT survivor_id, basis, since FROM exchange_account_standings WHERE user_id = ?1 AND duplicate_id = ?2", [user_id, duplicate],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()
}
/// Whether a live key of account `d` or `s` had an identity read fail definitely (`definite_failure`), or name another
/// account than its account's, since `since` (a standing's entry): Merge does not rest on it.
pub fn identity_changed_since(c: &Connection, d: i64, s: i64, since: &str) -> rusqlite::Result<bool> {
    if !ready(c)? { return Ok(false); }
    c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id JOIN exchange_accounts a ON a.id = {}
                          WHERE k.status = 1 AND a.id IN (?1, ?2) AND ((o.last_failure_at > ?3 AND {}) OR (o.fingerprint IS NOT NULL AND a.fingerprint IS NOT NULL AND o.fingerprint <> a.fingerprint)))",
                         key_account_sql("k"), definite_sql("o.last_failure_code")), params![d, s, since], |r| r.get(0))
}

/// A key event of account `account` (a real credential change, a key added or removed, a Merge moving keys, the account
/// removed): every standing whose pair includes it goes. The next `reconsider` (a sync's completion) may enter it again,
/// on evidence of the keys as they are now.
pub fn key_event(c: &Connection, account: i64) -> rusqlite::Result<()> {
    if !standings_ready(c)? { return Ok(()); }
    let user: Option<i64> = c.query_row("SELECT user_id FROM exchange_accounts WHERE id = ?1", [account], |r| r.get(0)).optional()?;
    if c.execute("DELETE FROM exchange_account_standings WHERE duplicate_id = ?1 OR survivor_id = ?1", [account])? > 0 {
        if let Some(user) = user { standing_changed(c, user)?; }
    }
    Ok(())
}
/// A change of user `user_id`'s standing: the whole-portfolio points counted under the old one go now, in the same
/// transaction (the history sweep rebuilds them under the new standing; today's writer is woken by the sync that ran).
fn standing_changed(c: &Connection, user_id: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM portfolio_snapshots WHERE user_id = ?1", [user_id])?;
    Ok(())
}
/// `key_event` of the account key `api_key_id` belongs to.
pub fn key_event_of_key(c: &Connection, api_key_id: i64) -> rusqlite::Result<()> {
    if !standings_ready(c)? { return Ok(()); }
    match account_of_key(c, api_key_id)? { Some(account) => key_event(c, account), None => Ok(()) }
}

/// What a standing rests on: both accounts' live keys (correct, every type) and a digest of each one's stored credential
/// (its encrypted columns as stored). Merge requires it unchanged since entry.
pub fn standing_basis(c: &Connection, d: i64, s: i64) -> rusqlite::Result<String> {
    use sha2::Digest;
    let keys = |a: i64| -> rusqlite::Result<Vec<Value>> {
        let Some(account) = AccountRow::by_id(c, a)? else { return Ok(vec![]) };
        let mut q = c.prepare(&format!("SELECT k.id, k.key, k.secret, k.passphrase, k.access_token, k.rsa_signature_key, k.rsa_encryption_key, k.dh_param \
                                        FROM api_keys k WHERE {} AND k.status = 1 ORDER BY k.id", account.keys("k")))?;
        let out = q.query_map([], |r| {
            let mut hash = sha2::Sha256::new();
            for i in 1..8 { hash.update(r.get::<_, Option<String>>(i)?.unwrap_or_default().as_bytes()); hash.update([0u8]); }
            Ok(serde_json::json!([r.get::<_, i64>(0)?, hex::encode(hash.finalize())]))
        })?.collect();
        out
    };
    Ok(serde_json::json!({ "d": keys(d)?, "s": keys(s)? }).to_string())
}

/// The one writer of the stored standing of (user, exchange) (PLAN-710-6 §5.1), run after a complete balance read or
/// ledger sync of one of its keys, and after a Merge.
///
/// A lower live account S and a higher one D are an accepted pair when every signal agrees:
/// - the venue reports an account id and both read the same one: equal fingerprints every live key of both confirms,
///   and balances that do not differ at comparable reads; or
/// - on a venue whose trade ids are venue-native (`OVERLAP_VENUES`): `MATCHES` shared trade ids with equal facts among
///   the rows each account's current keys imported, none conflicting (`trades_match`), balances read within
///   `READS_APART` that agree (`reads`), and every live key of both taking part.
///
/// The accepted pairs make components; each member of one whose members never contradict each other enters as a
/// duplicate of its lowest number. A positive contradiction (balances that differ at comparable reads, a shared id with
/// other facts, fingerprints that differ, a live key's identity read failed or masked) removes the stored standings it
/// falls inside. Nothing comparable changes nothing: a stored standing stays until a contradiction or a key event.
pub fn reconsider(c: &Connection, user_id: i64, exchange_id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    // One immediate write transaction: the evidence, the basis and the rows it writes are one state, which no key save can
    // change in between.
    if c.is_autocommit() {
        let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
        reconsider(&tx, user_id, exchange_id, now)?;
        return tx.commit();
    }
    if !standings_ready(c)? || !crate::venue_ids::ready(c)? || !crate::engine::rules::table_exists(c, "exchange_account_credentials")? { return Ok(()); }
    let mut q = c.prepare("SELECT id FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND removed_at IS NULL ORDER BY number")?;
    let ids: Vec<i64> = q.query_map([user_id, exchange_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    drop(q);
    let mut q = c.prepare("SELECT duplicate_id, survivor_id FROM exchange_account_standings WHERE user_id = ?1 AND exchange_id = ?2")?;
    let stored: std::collections::HashMap<i64, i64> = q.query_map([user_id, exchange_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    drop(q);
    if ids.len() < 2 {
        if !stored.is_empty() { c.execute("DELETE FROM exchange_account_standings WHERE user_id = ?1 AND exchange_id = ?2", [user_id, exchange_id])?; standing_changed(c, user_id)?; }
        return Ok(());
    }
    let kind: String = c.query_row("SELECT COALESCE(type, '') FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get(0))?;
    let overlap = OVERLAP_VENUES.contains(&kind.as_str());
    let mut rows = vec![];
    for id in &ids {
        let Some(row) = AccountRow::by_id(c, *id)? else { continue };
        let (print, confirmed, contradicted): (Option<String>, bool, bool) = c.query_row(&format!("SELECT x.fingerprint, {}, {} FROM exchange_accounts x WHERE x.id = ?1", confirmed("x"), contradicted("x")),
                                                                                     [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.push((row, print, confirmed, contradicted));
    }
    // Every pair, higher number first: accepted, contradicting, or neither.
    let (mut links, mut vetoed) = (vec![], vec![]);
    for (i, (s, fs, cs, _)) in rows.iter().enumerate() {
        for (d, fd, cd, _) in rows.iter().skip(i + 1) {
            let read = reads(c, d, s)?;
            let prints_differ = matches!((fd, fs), (Some(a), Some(b)) if a != b);
            if read == Reads::Differ || prints_differ || (overlap && crate::venue_ids::overlap_conflict(c, &d.venue_ids(), &s.venue_ids(), "1", "1")?) {
                vetoed.push((d.id, s.id));
                continue;
            }
            let accepted = *cd && *cs && if fd.is_some() && fd == fs { true } else { overlap && read == Reads::Agree && trades_match(c, d, s)? };
            if accepted { links.push((d.id, s.id)); }
        }
    }
    let contradicted: std::collections::HashSet<i64> = rows.iter().filter(|r| r.3).map(|r| r.0.id).collect();
    let prints: std::collections::HashMap<i64, String> = rows.iter().filter_map(|r| r.1.clone().map(|f| (r.0.id, f))).collect();
    let number: std::collections::HashMap<i64, i64> = rows.iter().map(|r| (r.0.id, r.0.number)).collect();
    let vetoes = |members: &[i64]| members.iter().any(|x| contradicted.contains(x) || members.iter().any(|y| vetoed.contains(&(*x, *y))))
        || { let mut seen: Vec<&String> = members.iter().filter_map(|a| prints.get(a)).collect(); seen.sort(); seen.dedup(); seen.len() > 1 };
    // LEAVE: a stored group (a survivor and its duplicates) a contradiction falls inside.
    let mut groups: std::collections::BTreeMap<i64, Vec<i64>> = std::collections::BTreeMap::new();
    for (d, s) in &stored { groups.entry(*s).or_insert_with(|| vec![*s]).push(*d); }
    let mut kept = stored.clone();
    // Only live accounts stand (a removed one's went with its removal, `tombstone`).
    kept.retain(|d, s| ids.contains(d) && ids.contains(s));
    for members in groups.values() {
        if vetoes(members) { for m in members { kept.remove(m); } }
    }
    // ENTER: the union of the retained edges and the accepted pairs (a retained edge is never overwritten); each component
    // of it that no veto falls inside, every member a duplicate of its lowest number.
    let mut edges = links.clone();
    edges.extend(kept.iter().map(|(d, s)| (*d, *s)));
    let fresh: std::collections::HashSet<i64> = links.iter().flat_map(|(d, s)| [*d, *s]).collect();
    let mut want: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for members in &components(&edges) {
        if vetoes(members) { continue; }
        let Some(&lowest) = members.iter().min_by_key(|a| number.get(a).copied().unwrap_or(i64::MAX)) else { continue };
        for m in members.iter().filter(|m| **m != lowest) { want.insert(*m, lowest); }
    }
    let mut changed = false;
    for (d, s) in &stored {
        if want.get(d) != Some(s) { c.execute("DELETE FROM exchange_account_standings WHERE duplicate_id = ?1", [d])?; changed = true; }
    }
    for (d, s) in &want {
        if stored.get(d) == Some(s) {
            // Entered again on every signal now: it rests on the keys as they are.
            if fresh.contains(d) { c.execute("UPDATE exchange_account_standings SET basis = ?2 WHERE duplicate_id = ?1", params![d, standing_basis(c, *d, *s)?])?; }
            continue;
        }
        c.execute("INSERT INTO exchange_account_standings (duplicate_id, survivor_id, user_id, exchange_id, since, basis) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                  params![d, s, user_id, exchange_id, format_time(now), standing_basis(c, *d, *s)?])?;
        changed = true;
    }
    if changed { standing_changed(c, user_id)?; }
    Ok(())
}
/// Whether accounts `x` and `y` of one exchange contradict each other (`reconsider`'s veto): balances that differ at
/// comparable reads, known fingerprints that differ, a shared venue-native trade id with other facts (on an overlap venue),
/// or either's fingerprint contradicted by one of its live keys. An account no longer there is a veto.
pub fn vetoed(c: &Connection, x: i64, y: i64) -> rusqlite::Result<bool> {
    let (Some(a), Some(b)) = (AccountRow::by_id(c, x)?, AccountRow::by_id(c, y)?) else { return Ok(true) };
    let (d, s) = if a.number > b.number { (a, b) } else { (b, a) };
    let kind: String = c.query_row("SELECT COALESCE(type, '') FROM exchanges WHERE id = ?1", [d.exchange_id], |r| r.get(0))?;
    let row = |id: i64| -> rusqlite::Result<(Option<String>, bool)> {
        c.query_row(&format!("SELECT x.fingerprint, {} FROM exchange_accounts x WHERE x.id = ?1", contradicted("x")), [id], |r| Ok((r.get(0)?, r.get(1)?)))
    };
    let ((fd, cd), (fs, cs)) = (row(d.id)?, row(s.id)?);
    Ok(cd || cs || matches!((&fd, &fs), (Some(p), Some(q)) if p != q) || reads(c, &d, &s)? == Reads::Differ
       || (OVERLAP_VENUES.contains(&kind.as_str()) && crate::venue_ids::overlap_conflict(c, &d.venue_ids(), &s.venue_ids(), "1", "1")?))
}
/// `reconsider` for the (user, exchange) of key `api_key_id`.
pub fn reconsider_for_key(c: &Connection, api_key_id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let row: Option<(i64, i64)> = c.query_row("SELECT user_id, exchange_id FROM api_keys WHERE id = ?1", [api_key_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    match row { Some((user, exchange)) => reconsider(c, user, exchange, now), None => Ok(()) }
}
/// The connected groups of `links`' accounts.
fn components(links: &[(i64, i64)]) -> Vec<Vec<i64>> {
    let mut out: Vec<Vec<i64>> = vec![];
    for (a, b) in links {
        let found: Vec<usize> = out.iter().enumerate().filter(|(_, g)| g.contains(a) || g.contains(b)).map(|(i, _)| i).collect();
        let mut merged = vec![*a, *b];
        for i in found.iter().rev() { merged.extend(out.remove(*i)); }
        merged.sort();
        merged.dedup();
        out.push(merged);
    }
    out
}
/// Whether two accounts' balances agree: each read completely by its reading key at its current version
/// (`sync::cache::balances_read_at`), the reads within `READS_APART` of each other, and every asset either holds at a
/// non-zero amount the same within `ROUNDING`. Reads not both present, or too far apart, are not comparable.
fn reads(c: &Connection, d: &AccountRow, s: &AccountRow) -> rusqlite::Result<Reads> {
    let (Some(td), Some(ts)) = (crate::sync::cache::balances_read_at(c, d).map_err(eng)?, crate::sync::cache::balances_read_at(c, s).map_err(eng)?) else { return Ok(Reads::NotComparable) };
    // A read that skipped a position is not the whole account.
    if !skipped(c, d)?.is_empty() || !skipped(c, s)?.is_empty() { return Ok(Reads::NotComparable); }
    if (td - ts).num_seconds().abs() > READS_APART { return Ok(Reads::NotComparable); }
    let held = |a: i64| -> rusqlite::Result<std::collections::HashMap<i64, f64>> {
        let mut q = c.prepare("SELECT asset_id, sum(CAST(free AS REAL) + CAST(locked AS REAL)) FROM exchange_account_balances WHERE account_id = ?1 GROUP BY asset_id")?;
        let out = q.query_map([a], |r| Ok((r.get(0)?, r.get(1)?)))?.collect();
        out
    };
    let (hd, hs) = (held(d.id)?, held(s.id)?);
    for asset in hd.keys().chain(hs.keys()) {
        let (a, b) = (*hd.get(asset).unwrap_or(&0.0), *hs.get(asset).unwrap_or(&0.0));
        if (a - b).abs() > ROUNDING * a.abs().max(b.abs()) { return Ok(Reads::Differ); }
    }
    // What a read holds but cannot name (Kraken's unresolved positive codes, recorded beside the rows) takes part too.
    if unresolved(c, d)? != unresolved(c, s)? { return Ok(Reads::Differ); }
    Ok(Reads::Agree)
}
/// The codes account `a`'s reading key's last balance read held but could not name (`rust_sync.kraken_unresolved:<key>`).
fn unresolved(c: &Connection, a: &AccountRow) -> rusqlite::Result<Vec<String>> { codes(c, a, "rust_sync.kraken_unresolved") }
/// What account `a`'s reading key's last balance read dropped although it held something (an asset the read names
/// none of, a class this build does not hold: `rust_sync.skipped:<key>`): a read with any is no complete account.
fn skipped(c: &Connection, a: &AccountRow) -> rusqlite::Result<Vec<String>> { codes(c, a, "rust_sync.skipped") }
fn codes(c: &Connection, a: &AccountRow, prefix: &str) -> rusqlite::Result<Vec<String>> {
    let Some(key) = crate::sync::cache::reading_key(c, a).map_err(eng)? else { return Ok(vec![]) };
    let raw = crate::app_config::get_plain(c, &format!("{prefix}:{key}")).map_err(rusqlite::Error::InvalidParameterName)?;
    let mut codes: Vec<String> = raw.and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["codes"].as_array().cloned()).unwrap_or_default()
        .into_iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
    codes.sort();
    Ok(codes)
}
/// After key `api_key_id`'s balance read completed: the exchange's standing is reconsidered (`reconsider`), and each
/// candidate partner of its account (equal fingerprints, or a shared trade id) whose read is not comparable with it has
/// its reading key returned, to be read now (so the two become comparable).
pub fn after_balance_read(c: &Connection, api_key_id: i64, now: DateTime<Utc>) -> rusqlite::Result<Vec<i64>> {
    if !numbered(c)? || !crate::venue_ids::ready(c)? { return Ok(vec![]); }
    let Some(account) = AccountRow::of_key(c, api_key_id)? else { return Ok(vec![]) };
    if !account.placed() { return Ok(vec![]); }
    reconsider(c, account.user_id, account.exchange_id, now)?;
    let mut q = c.prepare("SELECT id FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2 AND id <> ?3 AND removed_at IS NULL ORDER BY number")?;
    let others: Vec<i64> = q.query_map([account.user_id, account.exchange_id, account.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    drop(q);
    let mut wake = vec![];
    for other in others {
        let Some(other) = AccountRow::by_id(c, other)? else { continue };
        let (d, s) = if account.number > other.number { (account, other) } else { (other, account) };
        let prints: bool = c.query_row("SELECT d.fingerprint IS NOT NULL AND d.fingerprint = s.fingerprint FROM exchange_accounts d, exchange_accounts s WHERE d.id = ?1 AND s.id = ?2",
                                       [d.id, s.id], |r| r.get(0))?;
        if !prints && !crate::venue_ids::any_shared_id(c, &d.venue_ids(), &s.venue_ids())? { continue; }
        if reads(c, &d, &s)? != Reads::NotComparable { continue; }
        // Only a partner whose read is more than READS_APART older than this one, woken for this pair at most hourly.
        let Some(k) = crate::sync::cache::reading_key(c, &other).map_err(eng)? else { continue };
        let read_at = |key: i64| crate::sync::cache::read_time(c, key);
        let mine = read_at(api_key_id)?.unwrap_or(now);
        if read_at(k)?.is_some_and(|theirs| (mine - theirs).num_seconds() <= READS_APART) { continue; }
        let woke = crate::app_config::get_plain(c, &format!("rust_standing_wake:{}:{}", d.id, s.id)).map_err(rusqlite::Error::InvalidParameterName)?.and_then(|t| parse_time(&t).ok());
        if woke.is_some_and(|t| now - t < Duration::hours(1)) { continue; }
        crate::app_config::set_plain(c, &format!("rust_standing_wake:{}:{}", d.id, s.id), &format_time(now), now).map_err(rusqlite::Error::InvalidParameterName)?;
        wake.push(k);
    }
    Ok(wake)
}


/// The one write of a stored key's status (0 unchecked, 1 correct, 2 incorrect, 3 awaiting activation): `at` stamps
/// updated_at (only when the status changes, if `on_change`), and `clear_error` clears its last sync error. A key turning
/// live (correct) or no longer live is a key event of its account (`key_event`). Returns whether the status changed.
pub fn write_status(c: &Connection, id: i64, status: i64, at: &str, on_change: bool, clear_error: bool) -> rusqlite::Result<bool> {
    let Some(old) = c.query_row("SELECT status FROM api_keys WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).optional()? else { return Ok(false) };
    c.execute(&format!("UPDATE api_keys SET status = ?2, updated_at = CASE WHEN ?4 AND status = ?2 THEN updated_at ELSE ?3 END{} WHERE id = ?1",
                       if clear_error { ", last_sync_error = NULL" } else { "" }), params![id, status, at, on_change])?;
    if (old == 1) != (status == 1) { key_event_of_key(c, id)?; }
    // A key turning live or not live may move its account's reader (rule M4).
    if old != status { crate::sync::settle_readers_of_key(c, id).map_err(|e| rusqlite::Error::InvalidParameterName(e.0))?; }
    Ok(old != status)
}
/// The one write of a stored key's credential material: api_keys' key, secret and passphrase, as the ciphertext given (the
/// version a validation was made under). Every path that replaces a stored key's material calls it (the Settings form and
/// the wizards through `keys::submit_candidate`, the JSON save `POST /api/api_keys`).
///
/// When the key, secret or passphrase differs from the stored one (decrypted, compared in constant time), the key is another
/// credential (#497, #710): its ledger watermark and per-key sync state reset, its identity observation (of the old
/// material) is cleared, and the change is recorded (`credential_changed_key`), so that no row its old material imported is
/// evidence of the new one (`trades_match`). Returns whether it differed.
pub fn write_credential(c: &Connection, cipher: &Cipher, id: i64, key: Option<&str>, secret: Option<&str>, passphrase: Option<&str>, now: DateTime<Utc>) -> rusqlite::Result<bool> {
    let stored: (Option<String>, Option<String>, Option<String>) = c.query_row("SELECT key, secret, passphrase FROM api_keys WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let plain = |v: Option<&str>| v.filter(|v| !crate::v2::blank(v)).map(|v| cipher.decrypt(v).unwrap_or_else(|_| v.to_string()));
    let same = |a: Option<String>, b: Option<String>| match (a, b) {
        (Some(a), Some(b)) => bool::from(subtle::ConstantTimeEq::ct_eq(a.as_bytes(), b.as_bytes())),
        (a, b) => a.is_none() && b.is_none(),
    };
    let changed = !(same(plain(stored.0.as_deref()), plain(key)) && same(plain(stored.1.as_deref()), plain(secret)) && same(plain(stored.2.as_deref()), plain(passphrase)));
    // Identical material is written too, under its fresh envelopes: the new ciphertext version fences any result read under
    // the old one (R4). Only a real change is an event.
    c.execute("UPDATE api_keys SET key = ?2, secret = ?3, passphrase = ?4 WHERE id = ?1", params![id, key, secret, passphrase])?;
    if changed {
        c.execute("UPDATE api_keys SET last_synced_at = NULL WHERE id = ?1", [id])?;
        c.execute("DELETE FROM app_configs WHERE key IN (?1, ?2, ?3, ?4)", params![format!("rust_sync.ledger:{id}"), format!("rust_sync.ledger_splits:{id}"),
                  format!("rust_sync.kraken_unresolved:{id}"), format!("rust_sync.kraken_repair_deferred:{id}")])?;
        c.execute("DELETE FROM app_configs WHERE key IN (?1, ?2)", [format!("rust_sync.skipped:{id}"), format!("rust_sync.read_at:{id}")])?;
        if ready(c)? {
            c.execute("UPDATE exchange_account_credentials SET material_digest = NULL, observed_credential_version = NULL, fingerprint = NULL, account_id_enc = NULL,
                       observed_at = NULL, last_failure_at = NULL, last_failure_code = NULL WHERE api_key_id = ?1", [id])?;
        }
        crate::app_config::set_plain(c, &credential_changed_key(id), &format_time(now), now).map_err(rusqlite::Error::InvalidParameterName)?;
        key_event_of_key(c, id)?;
        crate::sync::settle_readers_of_key(c, id).map_err(|e| rusqlite::Error::InvalidParameterName(e.0))?;
    }
    Ok(changed)
}
/// The app_configs key holding when key `id`'s credential last changed (a settings save that replaced its material).
pub fn credential_changed_key(id: i64) -> String { format!("rust_key_credential_changed:{id}") }

/// The shared-trade signal: every live key of both accounts (correct, not a withdrawal key) has its ledger import from its
/// current credential (`sync::cache::ledger_current_for`); among the rows those keys imported since they were made (a
/// replaced key's rows from before its replacement never),
/// the duplicate's earliest `SAMPLE` trades meet `MATCHES` distinct venue trades of the survivor's under their venue-native
/// ids with equal facts (Coinbase's `-in`/`-out` legs are one trade); no shared id anywhere in their histories has other
/// facts; and every one of those keys imported at least one of the matched rows.
fn trades_match(c: &Connection, d: &AccountRow, s: &AccountRow) -> rusqlite::Result<bool> {
    let live = |a: &AccountRow| -> rusqlite::Result<Vec<i64>> {
        let mut q = c.prepare(&format!("SELECT k.id FROM api_keys k WHERE {} AND k.key_type != 1 AND k.status = 1 ORDER BY k.id", a.keys("k")))?;
        let out = q.query_map([], |r| r.get(0))?.collect();
        out
    };
    let (kd, ks) = (live(d)?, live(s)?);
    if kd.is_empty() || ks.is_empty() { return Ok(false); }
    for k in kd.iter().chain(&ks) { if !crate::sync::cache::ledger_current_for(c, *k).map_err(eng)? { return Ok(false); } }
    // A row counts when one of the account's live keys imported it, no earlier than that key was made (a key's rows carry no
    // credential version of their own) and strictly after its credential was last replaced (`write_credential`).
    let counted = |a: &str, keys: &[i64]| {
        let secs = |e: &str| format!("CAST(strftime('%s', {e}) AS INTEGER)");
        let changed = "(SELECT x.value FROM app_configs x WHERE x.key = 'rust_key_credential_changed:' || k.id)".to_string();
        // The cutoff at full precision, strictly after: both are format_time text, whose order is their time's.
        format!("EXISTS(SELECT 1 FROM api_keys k WHERE k.id = {a}.api_key_id AND k.id IN ({}) AND {} >= {} AND ({changed} IS NULL OR {a}.created_at > {changed}))",
                keys.iter().map(i64::to_string).collect::<Vec<_>>().join(","), secs(&format!("{a}.created_at")), secs("k.created_at"))
    };
    if crate::venue_ids::overlap_conflict(c, &d.venue_ids(), &s.venue_ids(), &counted("a", &kd), &counted("b", &ks))? { return Ok(false); }
    let sample = crate::venue_ids::overlap_sample(c, &d.venue_ids(), SAMPLE, &counted("a", &kd))?;
    let (scope, theirs_counted) = (s.venue_ids(), counted("a", &ks));
    let (mut strong, mut took) = (std::collections::HashSet::new(), std::collections::HashSet::new());
    for (row, key) in &sample {
        match crate::venue_ids::overlap_match(c, &scope, &row.venue_id, &theirs_counted)? {
            Some((theirs, k)) if theirs == *row => {
                // Coinbase's `-in`/`-out` legs are one trade; `native` admits no leg without its own id before the suffix.
                let trade = row.venue_id.strip_suffix("-in").or_else(|| row.venue_id.strip_suffix("-out")).filter(|t| !t.is_empty()).unwrap_or(&row.venue_id).to_string();
                strong.insert(trade); took.insert(*key); took.insert(k);
            }
            Some(_) => return Ok(false),
            None => {}
        }
    }
    Ok(strong.len() >= MATCHES && kd.iter().chain(&ks).all(|k| took.contains(k)))
}

/// What a Merge's live check found (PLAN-710-6 §6): fresh data on which the pair agrees; data too old or missing, with the
/// syncs to wake (job, key); or fresh reads that disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
/// `Unreadable`: an account no key can read the balances of (none correct but a withdrawal key): no sync can make it fresh.
/// `Unresolved`: what either account holds but cannot name (Kraken's unresolved codes, stored without their amounts; Alpaca's
/// skipped positions), so no read can show the two hold the same of it, each named by its account (`Kraken 2: XTOKEN`).
pub enum Live { Fresh, Stale(Vec<(&'static str, i64)>), Differ, Unreadable(i64), Unresolved(Vec<String>) }
/// How fresh a Merge's data must be.
pub const LIVE_SECS: i64 = 900;

/// The live check of a Merge of `d` into `s`, inside its transaction: each account's reader completed a ledger sync
/// under its current credential within `LIVE_SECS`, and each account's reading key read its
/// balances completely under its current credential within `LIVE_SECS`; then those reads must agree.
pub fn live_check(c: &Connection, d: &AccountRow, s: &AccountRow, now: DateTime<Utc>) -> rusqlite::Result<Live> {
    let since = now - Duration::seconds(LIVE_SECS);
    let mut wake = vec![];
    for a in [d, s] {
        // #710 7a rule Q4: the account's reader (the one selector), not every live key: a key that is not the reader
        // produced none of the account's history.
        let keys: Vec<i64> = crate::sync::cache::reading_key(c, a).map_err(eng)?.into_iter().collect();
        for key in keys {
            let state = crate::jobs::state::read(c, crate::sync::jobs::LEDGER_SYNC, Some(&key.to_string())).map_err(rusqlite::Error::InvalidParameterName)?;
            let fresh = crate::sync::cache::ledger_current_for(c, key).map_err(eng)? && state.incomplete_since.is_none() && state.last_success_at.is_some_and(|t| t >= since);
            if !fresh { wake.push((crate::sync::jobs::LEDGER_SYNC, key)); }
        }
        let read = crate::sync::cache::balances_read_at(c, a).map_err(eng)?;
        if read.is_none_or(|t| t < since) {
            match crate::sync::cache::reading_key(c, a).map_err(eng)? {
                Some(key) => wake.push((crate::sync::jobs::BALANCE_SYNC, key)),
                // No key reads this account's balances: nothing can make them fresh until one does.
                None => return Ok(Live::Unreadable(a.id)),
            }
        }
    }
    if !wake.is_empty() { return Ok(Live::Stale(wake)); }
    // What a read skipped or could not name: the reads cannot show the two hold the same.
    let mut held = vec![];
    for a in [s, d] { for code in unresolved(c, a)?.into_iter().chain(skipped(c, a)?) { held.push(format!("{}: {code}", label(c, a.id)?.plain())); } }
    if !held.is_empty() { return Ok(Live::Unresolved(held)); }
    if reads(c, d, s)? != Reads::Agree { return Ok(Live::Differ); }
    Ok(Live::Fresh)
}

/// SQL condition: account row `x`'s known fingerprint (if any) is what its keys still read: every live key's identity
/// observation confirms it and none reads another account. A stale or contradicted one is neither duplicate nor survivor.
fn confirmed(x: &str) -> String {
    // Only live keys speak for it (a key incorrect, revoked or awaiting activation confirms nothing, whatever it read).
    let keys = format!("(SELECT o.fingerprint FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id WHERE {} = {x}.id AND k.status = 1 AND o.fingerprint IS NOT NULL)", key_account_sql("k"));
    // Every live key confirms it: one with no observation of its current material, whose read came back without an id
    // (failed, masked), or whose latest answer is a standing definite failure, is no confirmation beside one that did.
    let masked = format!("EXISTS (SELECT 1 FROM api_keys k WHERE {} = {x}.id AND k.status = 1 \
                          AND NOT EXISTS (SELECT 1 FROM exchange_account_credentials o WHERE o.api_key_id = k.id AND o.fingerprint = {x}.fingerprint \
                            AND NOT (o.last_failure_at IS NOT NULL AND {} AND (o.observed_at IS NULL OR o.last_failure_at > o.observed_at))))",
                         key_account_sql("k"), definite_sql("o.last_failure_code"));
    format!("({x}.fingerprint IS NULL OR ({x}.fingerprint IN {keys} AND NOT EXISTS (SELECT 1 FROM {keys} f WHERE f.fingerprint <> {x}.fingerprint) AND NOT {masked}))")
}
/// SQL condition: account row `x`'s known fingerprint is contradicted by a live key: its identity read names another
/// account, or its latest answer (newer than its last success) is a definite failure (`definite_failure`).
fn contradicted(x: &str) -> String {
    format!("({x}.fingerprint IS NOT NULL AND EXISTS (SELECT 1 FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id WHERE {} = {x}.id \
             AND k.status = 1 AND (o.fingerprint <> {x}.fingerprint OR (o.last_failure_at IS NOT NULL AND {} AND (o.observed_at IS NULL OR o.last_failure_at > o.observed_at)))))",
            key_account_sql("k"), definite_sql("o.last_failure_code"))
}
/// The ids of the user's duplicates (`duplicates`).
pub fn duplicate_ids(c: &Connection, user_id: i64) -> rusqlite::Result<Vec<i64>> { Ok(duplicates(c, user_id)?.into_iter().map(|(d, _)| d).collect()) }
/// SQL condition: account expression `account` (NULL: a number 1) is none of `ids` (`"1"` when there is none).
pub fn not_duplicate(account: &str, ids: &[i64]) -> String {
    if ids.is_empty() { "1".into() } else { format!("COALESCE({account}, 0) NOT IN ({})", ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")) }
}
/// SQL condition: api key row `alias` is no duplicate account's key (`"1"` with no duplicate). One statement.
pub fn key_counted(c: &Connection, user_id: i64, alias: &str) -> rusqlite::Result<String> {
    let ids = duplicate_ids(c, user_id)?;
    Ok(if ids.is_empty() { "1".into() } else { not_duplicate(&key_account_sql(alias), &ids) })
}
/// SQL condition: account expression `account` is none of the user's duplicates. One statement.
pub fn counted(c: &Connection, user_id: i64, account: &str) -> rusqlite::Result<String> { Ok(not_duplicate(account, &duplicate_ids(c, user_id)?)) }

/// SQL condition: api key row `k` is fenced (#710 7a): a key whose latest identity read names another account than its
/// account's known one, whatever its status (a revalidation that timed out leaves it pending, still fenced). It places
/// nothing, withdraws nothing and syncs nothing until a read matches again; its rows stay. `"0"` on an install without the
/// account tables.
pub fn fenced_sql(c: &Connection, k: &str) -> rusqlite::Result<String> {
    let observed: bool = c.query_row("SELECT count(*) = 1 FROM sqlite_master WHERE type = 'table' AND name = 'exchange_account_credentials'", [], |r| r.get(0))?;
    if !observed || !crate::venue_ids::ready(c)? { return Ok("0".into()); }
    // #710 7a rule M3: only a fingerprint a key read proved backs a fence (a file's never does).
    let proven = if proofs_ready(c)? { " AND EXISTS (SELECT 1 FROM account_fingerprint_proofs fp WHERE fp.account_id = fx.id)" } else { "" };
    Ok(format!("(EXISTS (SELECT 1 FROM exchange_account_credentials fo JOIN exchange_accounts fx ON fx.id = {} \
                WHERE fo.api_key_id = {k}.id AND fo.fingerprint IS NOT NULL AND fx.fingerprint IS NOT NULL AND fo.fingerprint <> fx.fingerprint{proven}))", key_account_sql(k)))
}
fn proofs_ready(c: &Connection) -> rusqlite::Result<bool> { crate::engine::rules::table_exists(c, "account_fingerprint_proofs") }
/// #710 7a rule N2: whether a key read proved account `id`'s fingerprint (`account_fingerprint_proofs`, durable: no
/// observation upsert, takeover sentinel or credential removal deletes it; Merge carries it; the export keeps it). An
/// install without the table proves every fingerprint, as before.
pub fn proven(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    if !proofs_ready(c)? { return Ok(true); }
    c.query_row("SELECT EXISTS(SELECT 1 FROM account_fingerprint_proofs WHERE account_id = ?1)", [id], |r| r.get(0))
}
fn prove(c: &Connection, id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    if proofs_ready(c)? { c.execute("INSERT INTO account_fingerprint_proofs (account_id, created_at) VALUES (?1, ?2) ON CONFLICT (account_id) DO NOTHING", params![id, format_time(now)])?; }
    Ok(())
}
/// Rules M3/N2: a key read of `account` as `fingerprint`: an unknown fingerprint is filled, an unproven one that differs is
/// replaced (a file's or a restore's: the read is the fact), and either, like an equal one, is proven. A proven one that
/// differs stays: the key is fenced there (`fenced_sql`).
fn establish_by_read(c: &Connection, account: i64, fingerprint: &str, encrypted: &[u8], now: DateTime<Utc>) -> rusqlite::Result<()> {
    let known: Option<String> = c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [account], |r| r.get(0)).optional()?.flatten();
    match known {
        None => { if establish(c, account, fingerprint, encrypted, now)? { prove(c, account, now)?; } }
        Some(k) if k == fingerprint => prove(c, account, now)?,
        Some(_) if !proven(c, account)? => {
            c.execute("UPDATE exchange_accounts SET fingerprint = ?2, account_id_enc = ?3, updated_at = ?4 WHERE id = ?1", params![account, fingerprint, encrypted, format_time(now)])?;
            key_event(c, account)?;
            prove(c, account, now)?;
        }
        Some(_) => {}
    }
    Ok(())
}
/// #710 7a rule Q2: whether key `api_key_id` has no successful UID read of its current material: no observation, one
/// that names nothing (a failed or masked read, the takeover's sentinel, P8), or one made under another ciphertext.
pub fn unread(c: &Connection, api_key_id: i64) -> Result<bool, String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    if !ready(c).map_err(db)? { return Ok(false); }
    let row: Option<(Option<String>, Option<String>, Option<String>)> = c.query_row("SELECT fingerprint, observed_at, observed_credential_version FROM exchange_account_credentials WHERE api_key_id = ?1",
        [api_key_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(db)?;
    let Some((Some(fingerprint), Some(_), Some(version))) = row else { return Ok(true) };
    if fingerprint == UNCONFIRMED_AFTER_V2 { return Ok(true); }
    let Some(read) = serde_json::from_str::<Value>(&version).ok().and_then(|stored| crate::engine::model::CredentialVersion::from_stamp(&stored)) else { return Ok(true) };
    Ok(read.id != api_key_id || !crate::engine::model::credential_is_current(c, &read).map_err(|e| format!("{e:?}"))?)
}
/// The handoff of every account of key `api_key_id`'s exchange (`sync::settle_readers`).
fn settle(c: &Connection, api_key_id: i64) -> Result<(), String> { crate::sync::settle_readers_of_key(c, api_key_id).map(|_| ()).map_err(|e| e.0) }

/// What a key save's account read came to (#710 7a, PLAN-710-7 §1.1), for `route_key`.
pub enum Reading<'a> {
    /// The venue named the key's account.
    Read(&'a AccountId),
    /// An identity venue's read failed or was masked.
    Unread,
    /// A venue that reports no account id.
    NotAnIdentityVenue,
}
/// Where a saved key goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// This live account (`target` or another).
    To(i64),
    /// A new account, numbered after every one of the exchange, tombstones included.
    New,
    /// The read could not tell, and the key could land on the wrong account: refused, retryable, nothing created.
    Unconfirmed,
}

/// The one routing of a saved key (PLAN-710-7 §1.1, master plan §4.3), in the save's write transaction: `target` the
/// account the form named (a Reconnect, a bot's sheet, a 2.x form's number 1), None for a venue tile. In order:
/// 1. a read whose fingerprint a live account has: there (of two unmerged duplicates, the survivor, else the lowest);
/// 2. a read no account has: `target` when it has no fingerprint; with no target, the exchange's only live account when
///    it has no fingerprint and no key of `kind`; else a new account;
/// 3. an identity venue's failed or masked read: only a named `target` with no fingerprint that is the exchange's only
///    live account; else `Unconfirmed`;
/// 4. no identity venue: the account of a stored key of (user, exchange) with this material (compared decrypted, the
///    venue's own fields only); else `target`; else a new account, but a lone live account with no key at all takes it.
#[allow(clippy::too_many_arguments)]
pub fn route_key(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, exchange_type: &str, kind: i64, target: Option<i64>, reading: &Reading,
                 material: &Credentials, skip: Option<i64>, now: DateTime<Utc>) -> Result<Route, String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    let live = accounts_of(c, user_id, exchange_id).map_err(db)?;
    let fingerprint_of = |id: i64| c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).map_err(db);
    // Rule M3: an account whose fingerprint no key read proved takes a key as one with none.
    let unset = |id: i64| -> Result<bool, String> { Ok(fingerprint_of(id)?.is_none() || !proven(c, id).map_err(db)?) };
    // Rule N3: before a new account, the installed evidence: the exact key and secret already stored on a live account
    // (`unknown`: only one whose fingerprint no read proved), or a Hyperliquid wallet already connected (its address is the
    // account; each agent secret is a key of it).
    let installed = |unknown: bool| -> Result<Route, String> {
        let wallet = exchange_type == crate::venue::hyperliquid::HYPERLIQUID;
        let mut s = c.prepare("SELECT id FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY id").map_err(db)?;
        let ids: Vec<i64> = s.query_map([user_id, exchange_id], |r| r.get(0)).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
        for id in ids.into_iter().filter(|id| Some(*id) != skip) {
            let Some(stored) = stored_material(c, cipher, id) else { continue };
            let same = if wallet { stored.key.trim().eq_ignore_ascii_case(material.key.trim()) && !material.key.trim().is_empty() }
                       else { stored.key == material.key && stored.secret == material.secret && !material.secret.is_empty() };
            if !same { continue; }
            let Some(account) = account_of_key(c, id).map_err(db)?.filter(|a| live.iter().any(|(l, _)| l == a)) else { continue };
            if !unknown || unset(account)? { return Ok(Route::To(account)); }
        }
        Ok(Route::New)
    };
    let keys_of = |id: i64| -> Result<i64, String> {
        let account = Account::by_id(c, id).map_err(db)?.ok_or("account identity: no account")?;
        c.query_row(&format!("SELECT count(*) FROM api_keys k WHERE {}", account.keys("k")), [], |r| r.get(0)).map_err(db)
    };
    match reading {
        Reading::Read(id) => {
            let fingerprint = key(c, cipher, now)?.fingerprint(exchange_type, id);
            let mut holders = vec![];
            for (account, _) in &live { if fingerprint_of(*account)?.as_deref() == Some(fingerprint.as_str()) { holders.push(*account); } }
            if !holders.is_empty() {
                // Two unmerged duplicates: the survivor of the stored standing, else the lowest number (`live` is by number).
                let survivor = holders.iter().find_map(|h| standing_of(c, user_id, *h).ok().flatten().map(|(s, _, _)| s)).filter(|s| holders.contains(s));
                return Ok(Route::To(survivor.unwrap_or(holders[0])));
            }
            if let Some(t) = target { if unset(t)? { return Ok(Route::To(t)); } return installed(true); }
            if let [(only, _)] = live.as_slice() {
                let has_kind: bool = Account::by_id(c, *only).map_err(db)?.map(|a| a.key(c, kind)).transpose().map_err(db)?.flatten().is_some();
                if unset(*only)? && !has_kind { return Ok(Route::To(*only)); }
            }
            installed(true)
        }
        Reading::Unread => match (target, live.as_slice()) {
            (Some(t), [(only, _)]) if *only == t && unset(t)? => Ok(Route::To(t)),
            _ => Ok(Route::Unconfirmed),
        },
        Reading::NotAnIdentityVenue => {
            if let Some(found) = same_material(c, cipher, user_id, exchange_id, exchange_type, material, skip)? {
                if let Some(account) = account_of_key(c, found).map_err(db)?.filter(|a| live.iter().any(|(l, _)| l == a)) { return Ok(Route::To(account)); }
            }
            if let Some(t) = target { return Ok(Route::To(t)); }
            if let [(only, _)] = live.as_slice() { if keys_of(*only)? == 0 { return Ok(Route::To(*only)); } }
            installed(false)
        }
    }
}

/// A venue's own credential fields, as a save compares them (`venue/registry.rs`): a passphrase only where the venue
/// signs with it, Alpaca's empty mode as `paper`.
fn compared(exchange_type: &str, c: &Credentials) -> (String, String, String) {
    let passphrase = if exchange_type == crate::sync::ALPACA { c.passphrase.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "paper".into()) }
                     else if crate::venue::registry::row(exchange_type).is_some_and(|r| r.passphrase) { c.passphrase.clone().unwrap_or_default() } else { String::new() };
    (c.key.clone(), c.secret.clone(), passphrase)
}

/// A stored key of (user, exchange) other than `skip` whose material is `material`'s (`compared`), decrypted: the stored
/// digest is an index only, never the truth (a 2.x save can replace material in place). Each key's digest is filled as it
/// goes.
pub fn same_material(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, exchange_type: &str, material: &Credentials, skip: Option<i64>) -> Result<Option<i64>, String> {
    Ok(all_material(c, cipher, user_id, exchange_id, exchange_type, material, skip)?.into_iter().next())
}
/// Every stored key of (user, exchange) but `skip` whose material is `material`'s, by id.
fn all_material(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, exchange_type: &str, material: &Credentials, skip: Option<i64>) -> Result<Vec<i64>, String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    let want = compared(exchange_type, material);
    let mut s = c.prepare("SELECT id FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY id").map_err(db)?;
    let ids: Vec<i64> = s.query_map([user_id, exchange_id], |r| r.get(0)).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
    Ok(ids.into_iter().filter(|id| Some(*id) != skip).filter(|id| stored_material(c, cipher, *id).is_some_and(|stored| compared(exchange_type, &stored) == want)).collect())
}
/// Key `id`'s key, secret and passphrase, decrypted (None: unreadable). Inside any transaction.
fn stored_material(c: &Connection, cipher: &Cipher, id: i64) -> Option<Credentials> {
    let (key, secret, passphrase): (Option<String>, Option<String>, Option<String>) =
        c.query_row("SELECT key, secret, passphrase FROM api_keys WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok()?;
    let open = |v: Option<String>| v.map(|v| cipher.decrypt(&v).ok()).unwrap_or(Some(String::new()));
    Some(Credentials { redaction_values: vec![], key: open(key)?, secret: open(secret)?, passphrase: match passphrase { Some(p) => Some(cipher.decrypt(&p).ok()?), None => None }, ibkr: None })
}
/// Copy `other` of a key a save just read as account `read`'s (§9.1): its observation names that account now, so it is
/// fenced on its own account at once; its account's standings end (a key event). The next read of it replaces this.
pub fn record_copy(c: &Connection, cipher: &Cipher, other: i64, exchange_type: &str, read: &AccountId, now: DateTime<Utc>) -> Result<(), String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    let fingerprint = key(c, cipher, now)?.fingerprint(exchange_type, read);
    let exchange: i64 = c.query_row("SELECT exchange_id FROM api_keys WHERE id = ?1", [other], |r| r.get(0)).map_err(db)?;
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, fingerprint, account_id_enc, observed_at) VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT (api_key_id) DO UPDATE SET fingerprint = excluded.fingerprint, account_id_enc = excluded.account_id_enc, observed_at = excluded.observed_at,
               last_failure_at = NULL, last_failure_code = NULL", params![other, exchange, fingerprint, cipher.encrypt(&read.0).into_bytes(), format_time(now)]).map_err(db)?;
    if let Some(account) = account_of_key(c, other).map_err(db)? { key_event(c, account).map_err(db)?; }
    Ok(())
}
/// The observation fingerprint of a key whose material 2.x replaced in place under a known account (§9.2): it names no
/// account, so the key is fenced on its account (`fenced_sql`) until a fresh read records what it reads.
pub const UNCONFIRMED_AFTER_V2: &str = "unconfirmed-after-2.x";
/// At a takeover after 2.x's window (#710 7a §1.1, §9.2): 2.x can replace a key's material in place. Every observed key
/// under a fingerprinted account is compared by decrypting its stored material against the digest of the material it was
/// read with; one that differs, or whose digest is unknown (nothing proves it unchanged), is fenced until a fresh identity
/// read matches. #710 7a rule R5: the digests stay, so a later takeover compares again: a hand-back, a replacement in 2.x
/// and a second takeover fence a key the first takeover saw unchanged. Rule S3: every key of a fingerprinted account is
/// weighed, an observation or not: one 2.x added (no observation, no digest) is fenced the same way until a read.
pub fn after_v2(c: &Connection, cipher: &Cipher) -> rusqlite::Result<()> {
    if !ready(c)? { return Ok(()); }
    let Some(key) = existing_key(c, cipher) else { return Ok(()) };
    let mut s = c.prepare(&format!("SELECT k.id, k.exchange_id, o.material_digest FROM api_keys k LEFT JOIN exchange_account_credentials o ON o.api_key_id = k.id
                                    JOIN exchange_accounts a ON a.id = {} WHERE a.fingerprint IS NOT NULL ORDER BY k.id", key_account_sql("k")))?;
    let rows: Vec<(i64, i64, Option<String>)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    drop(s);
    let mut touched = vec![];
    for (id, exchange, digest) in rows {
        if digest.as_ref().is_some_and(|digest| stored_material(c, cipher, id).is_some_and(|m| key.material_digest(&m) == *digest)) { continue; }
        // Rules K4/N1: on an exchange with one live account a key with no digest (one 2.x added) syncs as on main until
        // discovery reads it; with two or more, the reader selector waits for its read (`unread`) anyway.
        if digest.is_none() {
            let user: i64 = c.query_row("SELECT user_id FROM api_keys WHERE id = ?1", [id], |r| r.get(0))?;
            if accounts_of(c, user, exchange)?.len() < 2 { continue; }
        }
        touched.push(id);
        c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, fingerprint) VALUES (?1, ?2, ?3)
                   ON CONFLICT (api_key_id) DO UPDATE SET fingerprint = excluded.fingerprint, account_id_enc = NULL, observed_credential_version = NULL",
                  params![id, exchange, UNCONFIRMED_AFTER_V2])?;
    }
    for id in touched { settle(c, id).map_err(rusqlite::Error::InvalidParameterName)?; }
    Ok(())
}
/// Whether stored key `id`'s material is `material`'s (`compared`).
pub fn is_material(c: &Connection, cipher: &Cipher, id: i64, exchange_type: &str, material: &Credentials) -> bool {
    stored_material(c, cipher, id).is_some_and(|stored| compared(exchange_type, &stored) == compared(exchange_type, material))
}
/// #710 7a rules F5 and R2: what a successful identity read of `material` (as account `read`) taught, whoever made it:
/// discovery, a validation, a key save, the JSON save, and a save refused after its read. Every stored copy of that
/// material of (user, exchange), in every slot, but `skip` (the key the read was made with, which `record` records itself)
/// whose account knows another fingerprint is recorded as reading `read` (`record_copy`), so each is fenced on its own
/// account at once. A copy that agrees, or whose account knows none, is left as it is.
#[allow(clippy::too_many_arguments)]
pub fn fence_copies(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, exchange_type: &str, material: &Credentials, read: &AccountId,
                    skip: Option<i64>, now: DateTime<Utc>) -> Result<(), String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    let fingerprint = key(c, cipher, now)?.fingerprint(exchange_type, read);
    for other in all_material(c, cipher, user_id, exchange_id, exchange_type, material, skip)? {
        let known: Option<String> = match account_of_key(c, other).map_err(db)? {
            Some(account) => c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [account], |r| r.get(0)).optional().map_err(db)?.flatten(),
            None => None,
        };
        if known.is_some_and(|k| k != fingerprint) { record_copy(c, cipher, other, exchange_type, read, now)?; }
    }
    crate::sync::settle_readers(c, user_id, exchange_id).map_err(|e| e.0)?;
    Ok(())
}

/// A new account of (user, exchange): `max(number) + 1` over every row, tombstones included, so no number is reused.
pub fn new_account(c: &Connection, user_id: i64, exchange_id: i64, now: DateTime<Utc>) -> rusqlite::Result<i64> {
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
               SELECT ?1, ?2, coalesce(max(number), 0) + 1, ?3, ?3 FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2",
              params![user_id, exchange_id, format_time(now)])?;
    Ok(c.last_insert_rowid())
}

/// A save's identity read establishes its routed account's fingerprint (every key type), in the save's transaction.
pub fn establish_read(c: &Connection, cipher: &Cipher, account: i64, exchange_type: &str, id: &AccountId, now: DateTime<Utc>) -> Result<(), String> {
    let fingerprint = key(c, cipher, now)?.fingerprint(exchange_type, id);
    establish_by_read(c, account, &fingerprint, &cipher.encrypt(&id.0).into_bytes(), now).map_err(|e| format!("account identity: {e}"))
}

/// The key's material digest (`IdentityKey::material_digest`), written for every venue's key with its save.
/// None while the install has no identity key (a digest is an index only; `same_material` decrypts every key anyway).
pub fn write_digest(c: &Connection, cipher: &Cipher, api_key_id: i64, exchange_id: i64, material: &Credentials) -> Result<(), String> {
    if !ready(c).map_err(|e| format!("account identity: {e}"))? { return Ok(()); }
    let Some(key) = existing_key(c, cipher) else { return Ok(()) };
    let digest = key.material_digest(material);
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, material_digest) VALUES (?1, ?2, ?3)
               ON CONFLICT (api_key_id) DO UPDATE SET material_digest = excluded.material_digest", params![api_key_id, exchange_id, digest])
        .map(|_| ()).map_err(|e| format!("account identity: {e}"))
}

/// Fills an unknown account's fingerprint. A known one is never overwritten.
fn establish(c: &Connection, account: i64, fingerprint: &str, encrypted: &[u8], now: DateTime<Utc>) -> rusqlite::Result<bool> {
    c.execute("UPDATE exchange_accounts SET fingerprint = ?2, account_id_enc = ?3, updated_at = ?4 WHERE id = ?1 AND fingerprint IS NULL",
              params![account, fingerprint, encrypted, format_time(now)]).map(|n| n == 1)
}

/// Whether `api_key_id` is the key the sync reads its account with (ApiKey.reading, sync::reading_keys_of): the only
/// credential that may establish an account.
fn reading_key(c: &Connection, user_id: i64, api_key_id: i64) -> Result<bool, String> {
    Ok(crate::tracker::import::run::reading(c, user_id).map_err(|e| e.0)?.iter().any(|k| k.id == api_key_id))
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
            // A live key's read naming another account than its account's known one ends the account's standings now (a key
            // event); a key that is not live speaks for nothing.
            let live: bool = c.query_row("SELECT status = 1 FROM api_keys WHERE id = ?1", [api_key_id], |r| r.get(0)).map_err(db)?;
            if let Some(account) = account_of_key(c, api_key_id).map_err(db)?.filter(|_| live) {
                let known: Option<String> = c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [account], |r| r.get(0)).optional().map_err(db)?.flatten();
                if known.is_some_and(|k| k != fingerprint) { key_event(c, account).map_err(db)?; }
            }
            c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, material_digest, observed_credential_version, fingerprint, account_id_enc, observed_at)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (api_key_id) DO UPDATE SET exchange_id = excluded.exchange_id,
                       material_digest = excluded.material_digest, observed_credential_version = excluded.observed_credential_version, fingerprint = excluded.fingerprint,
                       account_id_enc = excluded.account_id_enc, observed_at = excluded.observed_at, last_failure_at = NULL, last_failure_code = NULL",
                      params![api_key_id, exchange_id, digest, version, fingerprint, encrypted, at]).map_err(db)?;
            ensure_account(c, user_id, exchange_id, now).map_err(db)?;
            if reading_key(c, user_id, api_key_id)? {
                if let Some(account) = account_of_key(c, api_key_id).map_err(db)? {
                    let print = |c: &Connection| c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [account], |r| r.get::<_, Option<String>>(0)).optional();
                    let was = print(c).map_err(db)?.flatten();
                    establish_by_read(c, account, &fingerprint, &encrypted, now).map_err(db)?;
                    // A fingerprint filled or replaced is new evidence: the exchange's duplicate standings are judged again now.
                    if print(c).map_err(db)?.flatten() != was { reconsider(c, user_id, exchange_id, now).map_err(db)?; }
                }
            }
            fence_copies(c, cipher, user_id, exchange_id, &exchange_type, credentials, id, Some(api_key_id), now)?;
        }
        Observed::Failed(code) => {
            // A live key's definite failure on its current material is a contradiction: the account's standings end now. A
            // transient one changes nothing; a key that is not live speaks for nothing (discovery still reads it, as before).
            let live: bool = c.query_row("SELECT status = 1 FROM api_keys WHERE id = ?1", [api_key_id], |r| r.get(0)).map_err(db)?;
            if definite_failure(code) && live {
                if let Some(account) = account_of_key(c, api_key_id).map_err(db)? { key_event(c, account).map_err(db)?; }
            }
            let Some(key) = existing_key(c, cipher) else {
                c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, last_failure_at, last_failure_code) VALUES (?1, ?2, ?3, ?4)
                           ON CONFLICT (api_key_id) DO UPDATE SET exchange_id = excluded.exchange_id, last_failure_at = excluded.last_failure_at, last_failure_code = excluded.last_failure_code",
                          params![api_key_id, exchange_id, at, code]).map_err(db)?;
                return Ok(());
            };
            let digest = key.material_digest(credentials);
            let stored: Option<(Option<String>, Option<String>)> = c.query_row("SELECT material_digest, last_failure_code FROM exchange_account_credentials WHERE api_key_id = ?1",
                                                                               [api_key_id], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(db)?;
            // A definite failure on this material is sticky: a transient one after it leaves it standing; only a successful
            // read (Observed::Read) lifts it.
            let sticky = !definite_failure(code) && stored.as_ref().and_then(|(_, last)| last.as_deref()).is_some_and(definite_failure);
            // #710 7a §9.1/§9.2: a read that names nobody never lifts a fence (a read of another account, the takeover's
            // `UNCONFIRMED_AFTER_V2`), even when the material is unknown (digests dropped at a takeover) or changed: only a
            // successful read naming the account does.
            let fences: bool = c.query_row(&format!("SELECT {} FROM api_keys k WHERE k.id = ?1", fenced_sql(c, "k").map_err(db)?), [api_key_id], |r| r.get(0)).optional().map_err(db)?.unwrap_or(false);
            match stored.map(|(digest, _)| digest) {
                Some(Some(stored)) if stored == digest && sticky => Ok(0),
                Some(Some(stored)) if stored == digest => c.execute("UPDATE exchange_account_credentials SET last_failure_at = ?2, last_failure_code = ?3 WHERE api_key_id = ?1",
                                                              params![api_key_id, at, code]),
                Some(_) => c.execute("UPDATE exchange_account_credentials SET exchange_id = ?2, material_digest = ?3, observed_credential_version = NULL,
                                      fingerprint = CASE WHEN ?6 THEN ?7 END, account_id_enc = NULL,
                                      observed_at = NULL, last_failure_at = ?4, last_failure_code = ?5 WHERE api_key_id = ?1",
                                     params![api_key_id, exchange_id, digest, at, code, fences, UNCONFIRMED_AFTER_V2]),
                None => c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, material_digest, last_failure_at, last_failure_code) VALUES (?1, ?2, ?3, ?4, ?5)",
                                  params![api_key_id, exchange_id, digest, at, code]),
            }.map_err(db)?;
        }
    }
    // Rules M4/N7: a fence, or its lifting, moves the account's reader.
    settle(c, api_key_id)?;
    Ok(())
}

/// A deleted key of `account` (resolved before the delete): its observation and membership go, and the account's identity
/// too when it has no other key, the user no history on its exchange (ledger rows carry no account before #710 slice
/// 4, so every one counts), and the account no stored balance or venue snapshot of its own (#710 slice 5). The account row stays (numbers are never reused); it is tombstoned only from #710 slice 7 on,
/// since a tombstone followed by a reconnect would make number 2 before the slices that carry one.
pub fn forget(c: &Connection, api_key_id: i64, account: &Account, now: DateTime<Utc>) -> rusqlite::Result<()> {
    if !ready(c)? { return Ok(()); }
    if let Some(id) = account.id { key_event(c, id)?; }
    c.execute("DELETE FROM exchange_account_credentials WHERE api_key_id = ?1", [api_key_id])?;
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [api_key_id])?;
    let Some(id) = account.id else { return Ok(()) };
    // #710 7a §4: an account 2+ nothing names any more is tombstoned (its number never reused); one with history stays.
    let number: i64 = c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)).optional()?.unwrap_or(1);
    // Anything else naming it keeps it live, and with it its fingerprint: a reconnect of another wallet never inherits
    // the bot, rule or order that stays (it routes to a new account, §1.1). One predicate, for every number.
    let named = named(c, id)?;
    let settled = |c: &Connection| crate::sync::settle_readers(c, account.user_id, account.exchange_id).map(|_| ()).map_err(|e| rusqlite::Error::InvalidParameterName(e.0));
    if number > 1 && !named { tombstone(c, id, now)?; return settled(c); }
    settled(c)?;
    if named { return Ok(()); }
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1 AND (fingerprint IS NOT NULL OR account_id_enc IS NOT NULL)", [id])?;
    Ok(())
}

/// Whether anything names placed account `id` (#710 7a §4, one presence rule; rule F6): a key, a bot on it (any status),
/// a bot order or ledger row placed under it, a balance, a snapshot, an import row, observation or cutoff, a rule or a
/// standalone order. A number 1's keys, bots, bot orders and ledger rows name it without a membership row.
pub fn named(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    let first: Option<(i64, i64)> = c.query_row("SELECT user_id, exchange_id FROM exchange_accounts WHERE id = ?1 AND number = 1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if let Some((user, exchange)) = first {
        let implicit: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM api_keys k WHERE k.user_id = ?1 AND k.exchange_id = ?2 AND NOT EXISTS(SELECT 1 FROM exchange_account_keys m WHERE m.api_key_id = k.id))
             OR EXISTS(SELECT 1 FROM bots b WHERE b.user_id = ?1 AND b.exchange_id = ?2 AND NOT EXISTS(SELECT 1 FROM exchange_account_bots m WHERE m.bot_id = b.id))
             OR EXISTS(SELECT 1 FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE b.user_id = ?1 AND t.exchange_id = ?2
                       AND NOT EXISTS(SELECT 1 FROM exchange_account_transactions m WHERE m.transaction_id = t.id))
             OR EXISTS(SELECT 1 FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2
                       AND NOT EXISTS(SELECT 1 FROM exchange_account_rows m WHERE m.account_transaction_id = a.id))",
            [user, exchange], |r| r.get(0))?;
        if implicit { return Ok(true); }
    }
    // #710 7a rule R3: every row of a Rust-owned table that names the account (KuCoin's coverage windows and withheld fills
    // as much as keys, bots, rows, balances, snapshots, imports, rules and standalone orders), read from the schema.
    let tables = naming_tables(c)?;
    if tables.is_empty() { return Ok(false); }
    let any = tables.iter().map(|t| format!("EXISTS(SELECT 1 FROM \"{t}\" WHERE account_id = ?1)")).collect::<Vec<_>>().join(" OR ");
    c.query_row(&format!("SELECT {any}"), [id], |r| r.get(0))
}
/// #710 7a rule R3: the Rust-owned tables (`store::REGISTRY`) whose rows name an account: every one with an `account_id`
/// column in this install's schema, so a table added later counts without being listed.
pub fn naming_tables(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let owned = crate::store::REGISTRY.live_tables();
    let mut s = c.prepare("SELECT m.name FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type = 'table' AND p.name = 'account_id' ORDER BY m.name")?;
    let names = s.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    // Rule P4: an account's proof and reader generation are bookkeeping about it, not holdings of it.
    Ok(names.into_iter().filter(|n| owned.contains(&n.as_str()) && !["account_fingerprint_proofs", "ledger_reader_generations"].contains(&n.as_str())).collect())
}

/// #710 7a §4, the tracker's presence rule: the user's live accounts numbered 2+ that `named` keeps, as (exchange, account).
/// A number 1 is present by its keys and rows, which name it without a membership row.
pub fn kept(c: &Connection, user_id: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
    if !numbered(c)? { return Ok(vec![]); }
    let mut s = c.prepare("SELECT exchange_id, id FROM exchange_accounts WHERE user_id = ?1 AND number > 1 AND removed_at IS NULL ORDER BY id")?;
    let rows = s.query_map([user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = vec![];
    for (exchange, id) in rows { if named(c, id)? { out.push((exchange, id)); } }
    Ok(out)
}

/// #710 Merge (PLAN-710-6 §6.2 step 11): account `id` (a number 2+) is removed: its identity goes, its row stays as the
/// tombstone that keeps its number from being reused. Every live lookup (`first_account`, `accounts_of`, `pick`) skips it.
pub fn tombstone(c: &Connection, id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    key_event(c, id)?;
    let removed = c.execute("UPDATE exchange_accounts SET removed_at = ?2, fingerprint = NULL, account_id_enc = NULL, updated_at = ?2 WHERE id = ?1 AND number > 1",
                            params![id, format_time(now)])?;
    // Rule P4: its proof and reader generation go with it.
    if removed > 0 {
        for t in ["account_fingerprint_proofs", "ledger_reader_generations"] {
            if crate::engine::rules::table_exists(c, t)? { c.execute(&format!("DELETE FROM {t} WHERE account_id = ?1"), [id])?; }
        }
    }
    Ok(())
}

/// An import for (user, exchange), inside the import's own transaction: the account row exists afterwards; a venue-native
/// export's id is recorded as an import observation (never the account's fingerprint, rule M3).
pub fn record_import(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, id: Option<&AccountId>, now: DateTime<Utc>) -> Result<(), String> {
    record_import_to(c, cipher, user_id, exchange_id, None, id, now)
}
/// `record_import` for the account the file was routed to (`account`: its key's; None: the exchange's number 1).
pub fn record_import_to(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, account: Option<i64>, id: Option<&AccountId>, now: DateTime<Utc>) -> Result<(), String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    if !ready(c).map_err(db)? { return Ok(()); }
    let account = match account { Some(a) => a, None => ensure_account(c, user_id, exchange_id, now).map_err(db)? };
    let Some(id) = id else { return Ok(()) };
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get::<_, Option<String>>(0)).map_err(db)?.unwrap_or_default();
    let key = key(c, cipher, now)?;
    let fingerprint = key.fingerprint(&exchange_type, id);
    let encrypted = cipher.encrypt(&id.0).into_bytes();
    // #710 7a rule M3/L3: a file is the user's word, never the account's identity: its UID is an import observation only
    // (a routing hint, and what the import's warnings read); only a key read sets a fingerprint.
    c.execute("INSERT INTO exchange_account_imports (user_id, exchange_id, fingerprint, account_id_enc, observed_at, account_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
              params![user_id, exchange_id, fingerprint, encrypted, format_time(now), account]).map_err(db)?;
    Ok(())
}

/// #710 7a: the live account of (user, exchange) that holds an import of export account `id` recorded as a conflict
/// (the account's own fingerprint another): a file imported there before 7a, whose rows must not be imported again.
pub fn imported_as_conflict(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, id: &AccountId, now: DateTime<Utc>) -> Result<Option<i64>, String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    if !ready(c).map_err(db)? { return Ok(None); }
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get::<_, Option<String>>(0)).map_err(db)?.unwrap_or_default();
    let fingerprint = key(c, cipher, now)?.fingerprint(&exchange_type, id);
    c.query_row("SELECT x.id FROM exchange_account_imports i JOIN exchange_accounts x ON x.id = i.account_id WHERE i.user_id = ?1 AND i.exchange_id = ?2 \
                 AND i.fingerprint = ?3 AND x.removed_at IS NULL AND x.fingerprint IS NOT NULL AND x.fingerprint <> ?3 ORDER BY x.number LIMIT 1",
                params![user_id, exchange_id, fingerprint], |r| r.get(0)).optional().map_err(db)
}

/// #710 7a rules M1, N5, P8: what the identity records say about writing export account `id`'s history on `dest` (None: a
/// new account) of (user, exchange), read inside the import's transaction. `holders`: the other live accounts whose
/// fingerprint, key observation or import observation is the export's (by number). `foreign`: `dest`'s fingerprint, a
/// key observation of it, or an import observation of it names another account. The takeover's sentinel names nobody.
/// `facts`: what was compared (each account's distinct fingerprints seen), for the answer's digest (rule N4).
pub struct ImportEvidence { pub holders: Vec<i64>, pub foreign: bool, pub facts: String }
pub fn import_evidence(c: &Connection, cipher: &Cipher, user_id: i64, exchange_id: i64, dest: Option<i64>, id: &AccountId, now: DateTime<Utc>) -> Result<ImportEvidence, String> {
    let db = |e: rusqlite::Error| format!("account identity: {e}");
    let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get::<_, Option<String>>(0)).map_err(db)?.unwrap_or_default();
    let file = key(c, cipher, now)?.fingerprint(&exchange_type, id);
    let mut out = ImportEvidence { holders: vec![], foreign: false, facts: String::new() };
    for (account, _) in accounts_of(c, user_id, exchange_id).map_err(db)? {
        let Some(row) = Account::by_id(c, account).map_err(db)? else { continue };
        let own: Option<String> = c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [account], |r| r.get(0)).map_err(db)?;
        let mut seen: Vec<String> = c.prepare(&format!("SELECT o.fingerprint FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id
                                                         WHERE {} AND o.fingerprint IS NOT NULL AND o.fingerprint <> ?1
                                                         UNION SELECT fingerprint FROM exchange_account_imports WHERE account_id = ?2 ORDER BY 1", row.keys("k"))).map_err(db)?
            .query_map(params![UNCONFIRMED_AFTER_V2, account], |r| r.get(0)).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
        seen.extend(own.clone());
        seen.sort();
        seen.dedup();
        let theirs = seen.contains(&file);
        let other = seen.iter().any(|f| *f != file);
        // Rule N4: the fingerprints themselves, so one more seen meanwhile makes an open answer stale.
        out.facts.push_str(&format!("{account}:{};", seen.join(",")));
        if Some(account) == dest { out.foreign = other; } else if theirs { out.holders.push(account); }
    }
    Ok(out)
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
    /// A known fingerprint, and every current credential seen and every imported export agrees with it.
    Bound,
    /// A current credential or an imported export belongs to another account than the known one, or, the account
    /// unknown, two of them belong to different accounts. Whichever came first established the account, so a file
    /// after a key and a key after a file both read here.
    Conflicted,
}

/// The distinct fingerprints of (user, exchange)'s current credentials (keys still stored) and of its imported exports,
/// and the account's own.
/// #710 7a §3: of the exchange's number-1 account only, so a file or key routed to another account is no conflict here.
fn seen(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<(Option<String>, Vec<String>)> {
    let first = first_account(c, user_id, exchange_id)?;
    let account = first.as_ref().and_then(|(_, fingerprint)| fingerprint.clone());
    let mut s = c.prepare(&format!("SELECT o.fingerprint FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id
                           WHERE k.user_id = ?1 AND k.exchange_id = ?2 AND o.fingerprint IS NOT NULL AND {} IS ?3
                           UNION SELECT fingerprint FROM exchange_account_imports WHERE user_id = ?1 AND exchange_id = ?2 AND coalesce(account_id, ?3) IS ?3 ORDER BY 1", key_account_sql("k")))?;
    let observed = s.query_map(params![user_id, exchange_id, first.map(|(id, _)| id)], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
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
    let row: Option<(Option<String>, Option<String>)> = with_key_account("k", |account| c.query_row(
        &format!("SELECT o.fingerprint, (SELECT fingerprint FROM exchange_accounts WHERE id = {account}) FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id
                  WHERE o.api_key_id = ?1"),
        [api_key_id], |r| Ok((r.get(0)?, r.get(1)?))).optional())?;
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
                           FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id LEFT JOIN exchange_account_credentials o ON o.api_key_id = k.id
                           ORDER BY (o.observed_at IS NOT NULL OR o.last_failure_at IS NOT NULL), max(coalesce(o.observed_at, ''), coalesce(o.last_failure_at, '')), k.id").map_err(db)?;
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
    // The key's account too (#710): a reply read before a Merge moved the key is not its new account's.
    if !crate::engine::model::credential_is_current(&tx, &due.version).map_err(|e| format!("{e:?}"))?
        || !crate::engine::model::account_is_current(&tx, &due.version).map_err(|e| format!("{e:?}"))? { return Ok(false); }
    record(&tx, cipher, due.api_key_id, &due.credentials, &due.version, observed, now)?;
    tx.commit().map_err(|e: rusqlite::Error| e.to_string())?;
    Ok(true)
}

/// (account, reader generation) of every live account of key `key`'s exchange.
pub fn generations_of_key(c: &Connection, key: i64) -> Result<Vec<(i64, i64)>, String> {
    let row: Option<(i64, i64)> = c.query_row("SELECT user_id, exchange_id FROM api_keys WHERE id = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())?;
    let Some((user, exchange)) = row else { return Ok(vec![]) };
    let mut out = vec![];
    for (account, _) in accounts_of(c, user, exchange).map_err(|e| e.to_string())? { out.push((account, crate::sync::reader_generation(c, account).map_err(|e| e.0)?)); }
    Ok(out)
}

/// #710 7a rules K5/M4: the readers of key `key`'s exchange's accounts whose generation moved since `before`
/// (`generations_of_key`, read before the write): each has its ledger and balance jobs woken after the commit.
pub fn moved_readers(c: &Connection, key: i64, before: &[(i64, i64)]) -> Result<Vec<i64>, String> {
    let mut out = vec![];
    for (account, generation) in generations_of_key(c, key)? {
        if before.contains(&(account, generation)) { continue; }
        let Some(a) = Account::by_id(c, account).map_err(|e| e.to_string())? else { continue };
        out.extend(crate::sync::reader_of(c, &a.keys("k")).map_err(|e| e.0)?);
    }
    Ok(out)
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
                let key = credential.api_key_id;
                let before = match cx.db.run(move |c, _| generations_of_key(c, key)).await { Ok(g) => g, Err(e) => return Outcome::Failed(e) };
                if let Err(e) = cx.db.run(move |c, cipher| publish(c, cipher, &credential, &observed, at)).await { return Outcome::Failed(e); }
                // #710 7a rules K5/M4: a read that moved an account's reader (a fence, or its lifting) wakes the new reader.
                let moved = match cx.db.run(move |c, _| moved_readers(c, key, &before)).await { Ok(m) => m, Err(e) => return Outcome::Failed(e) };
                for r in moved { for job in [crate::sync::jobs::LEDGER_SYNC, crate::sync::jobs::BALANCE_SYNC] { cx.wakers.wake(job, Some(&r.to_string()), None); } }
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
        let c = Credentials { key: "k".into(), secret: "s".into(), passphrase: None, redaction_values: vec![], ibkr: None };
        assert_ne!(key.material_digest(&c), key.mac("deltabadger-account-fingerprint-v1", &["k", "s", ""]), "the two derivations never meet");
        // Field boundaries are part of the material: "ab"+"c" is not "a"+"bc".
        let shifted = Credentials { key: "ks".into(), secret: "".into(), ..c.clone() };
        assert_ne!(key.material_digest(&c), key.material_digest(&shifted));
    }

    #[test]
    fn only_definite_failures_count() {
        for code in ["http 401", "http 401; code 400003", "http 403", "http 200", "http 200; code -2015"] { assert!(definite_failure(code), "{code}"); }
        for code in ["http 500", "http 502", "http 429", "http 418", "http 400; code -1021", "transport", "no egress", "proxy unreadable"] { assert!(!definite_failure(code), "{code}"); }
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
