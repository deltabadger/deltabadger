//! `deltabadger export` (PLAN-B4b): Rails' `export:portable` (app/services/export/portable.rb, lib/export_scratch.rb) for
//! a 3.0 install. Launchpad runs it in a throwaway container (network none) on a private copy of the instance's storage;
//! it writes `/export/portable/{production,production_queue}.sqlite3`, `csv/tracker.csv`, `csv/bots/<id>-<label>.csv`,
//! `csv/README_CSV.txt` and `manifest.json`, scrubbed of everything that grants access to a venue, the platform or the
//! instance. Lines printed name steps, tables, columns and paths, never a value.
//!
//! Steps: the scratch guard, the engine flock (held to the end), a byte copy of the primary (and its `-wal`) into a fresh
//! 0700 temp dir, a fresh empty queue there, `store::check` on that pair, a generated column refused, foreign keys off,
//! every trigger and view of the copy dropped, this build's pending Rust migrations, the canonical projection into a
//! second temp database with this build's own schema, the scrub in one transaction, `VACUUM INTO` the output, the CSVs,
//! the manifest. Any error empties the output and drops the temp dir.
//!
//! Named divergences from Rails (PLAN-B4b §8, and what the recorder and the build found):
//! - D-EXPORT-NO-KEYS: the export needs no secret at all; every scrub step is a raw write or a delete.
//! - D-EXPORT-READ-ONLY-SOURCE: the source is only read, and never opened by SQLite (Rails migrates and scrubs in place).
//! - D-EXPORT-LEASE-KEPT: the engine lease stays (Rails deletes it), so 2.x Rails never adopts a Rust install.
//! - D-EXPORT-REBALANCE-OFF: rebalancing off on every bot (A29's D-RESET-REBALANCE-OFF).
//! - D-EXPORT-ACCOUNT-ANCHORS-KEPT: which account each key, import and IBKR bot belongs to stays, with the identity key
//!   that fingerprints it, readable under the shipped pair (the encryption reset clears them: its keys are gone), so a
//!   restored bot refuses another account. Only the old key's observations, IBKR reservations and never-sent
//!   withdrawals go.
//! - D-EXPORT-NO-CACHE-CABLE: no cache or cable file in the download (D-BOOT-NO-CACHE-CABLE).
//! - D-EXPORT-SCHEMA-RUST: the manifest's `schema_version` is this build's newest Rust migration.
//! - D-EXPORT-EXIT-INCOMPATIBLE: 65 for a copy this build can't read; 1 for a refused environment; 2 otherwise.
//! - D-EXPORT-FLOCK: refused while the engine flock is held.
//! - D-EXPORT-CANONICAL-SCHEMA: the download has this build's schema with the rows projected into it.
//! - D-EXPORT-SWAPS-HALTED: a swap part-way through is halted (rebalance::halt), its evidence kept.
//! - D-EXPORT-REQUESTS-DROPPED: a sale or redeploy the owner asked for and the engine has not taken yet is dropped by its
//!   operation's own compare-clear (liquidation::clear, redeploy::clear) with its own refusal line; Rails' stopped bots
//!   run no such request, and a restore must place no order before the owner acts.
//! - D-EXPORT-JOB-ERRORS: `last_error` is removed from JSON job-state rows (an upstream reply's first 200 characters).
//! - D-EXPORT-APP-CONFIG-ALLOWLIST: app_configs rows are kept by an allowlist (Rails deletes by a denylist).
//! - D-EXPORT-OWED-BY-HAND: owed withdrawals are settled with `resolve-withdrawal` after a restore.
//! - D-EXPORT-ORPHAN-BOT: a bot whose owner is gone is written in UTC and named as Rails names a new bot (Rails raises on
//!   `user.time_zone`, failing the whole export).
//! - D-EXPORT-LABEL-NFC: a file label is transliterated without Unicode NFC normalisation (no normaliser in this build): a
//!   decomposed accent becomes `-` where Rails folds it into its letter. Only a file name; it still matches launchpad's.
#![cfg(unix)]
use crate::lease::{self, LeaseError};
use crate::store::{self, Paths, StoreError};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OpenFlags};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The container's paths, fixed (nothing is read from the environment). Tests construct their own.
pub struct Scratch { pub storage: PathBuf, pub output: PathBuf, pub marker: PathBuf }

impl Scratch {
    pub fn container() -> Self {
        Scratch { storage: "/app/storage".into(), output: "/export".into(), marker: "/.dockerenv".into() }
    }
}

#[derive(Debug)]
pub enum ExportError {
    /// The environment is not a scratch container's, or the install is in use (exit 1).
    Refused(String),
    /// The copy is not one this build can read: newer, older or foreign (exit 65).
    Incompatible(String),
    /// Anything else: I/O, SQLite (exit 2).
    Failed(String),
}

impl ExportError {
    pub fn exit_code(&self) -> i32 { match self { Self::Refused(_) => 1, Self::Incompatible(_) => 65, Self::Failed(_) => 2 } }
    pub fn message(&self) -> &str { match self { Self::Refused(m) | Self::Incompatible(m) | Self::Failed(m) => m } }
}

fn refused(m: impl Into<String>) -> ExportError { ExportError::Refused(m.into()) }
fn incompatible(m: impl Into<String>) -> ExportError { ExportError::Incompatible(m.into()) }
fn failed(step: &str) -> impl Fn(rusqlite::Error) -> ExportError + '_ { move |e| ExportError::Failed(format!("{step}: {e}")) }
fn io(step: &str) -> impl Fn(std::io::Error) -> ExportError + '_ { move |e| ExportError::Failed(format!("{step}: {e}")) }

/// What `store::check` refused, by names only: every one of these is a copy this build can't read.
fn store_error(step: &str, e: StoreError) -> ExportError {
    match e {
        StoreError::Behind { missing } => incompatible(format!("{step}: the copy is older than this build (missing migrations {})", missing.join(", "))),
        StoreError::Unsupported { unknown } => incompatible(format!("{step}: the copy is newer than this build (unknown migrations {})", unknown.join(", "))),
        StoreError::Diverged { missing, unknown } =>
            incompatible(format!("{step}: the copy's migrations differ from this build's (missing {}; unknown {})", missing.join(", "), unknown.join(", "))),
        StoreError::Incompatible { problems } => incompatible(format!("{step}: the copy's tables differ from this build's: {}", problems.join("; "))),
        StoreError::Unrecognised { .. } | StoreError::Missing { .. } => incompatible(format!("{step}: the copy is not a Deltabadger database")),
        StoreError::Migration { version, name, error } => ExportError::Failed(format!("{step}: migration {version} {name} failed: {error}")),
        other => ExportError::Failed(format!("{step}: {other:?}")),
    }
}

const PRIMARY: &str = "production.sqlite3";
const QUEUE: &str = "production_queue.sqlite3";
const SIDECARS: [&str; 4] = ["", "-wal", "-shm", "-journal"];
/// The variables that would point Rails or Rust at other files (main.rs refuse_url_overrides, and the two paths).
const OVERRIDES: [&str; 7] = ["DATABASE_PATH", "QUEUE_DATABASE_PATH", "DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL",
                              "CACHE_DATABASE_URL", "CABLE_DATABASE_URL"];

/// ExportScratch.validate!, before any database is opened.
fn guard(s: &Scratch, env: &dyn Fn(&str) -> Option<String>) -> Result<(), ExportError> {
    if env("EXPORT_SCRATCH").as_deref() != Some("1") || !std::fs::symlink_metadata(&s.marker).is_ok_and(|m| m.file_type().is_file()) {
        return Err(refused("export runs only in a scratch container with EXPORT_SCRATCH=1"));
    }
    if let Some(var) = OVERRIDES.into_iter().find(|v| env(v).is_some_and(|x| !x.trim().is_empty())) {
        return Err(refused(format!("export does not accept database path overrides ({var} is set)")));
    }
    let real = std::fs::canonicalize(&s.storage).ok();
    if !std::fs::symlink_metadata(&s.storage).is_ok_and(|m| m.file_type().is_dir()) || real.as_deref() != Some(s.storage.as_path()) {
        return Err(refused(format!("export storage must be {} without symlinks", s.storage.display())));
    }
    let files = [PRIMARY, QUEUE].iter().flat_map(|f| SIDECARS.map(|x| format!("{f}{x}"))).chain([".secrets".to_string(), ".engine.lock".to_string()]);
    for name in files {
        match std::fs::symlink_metadata(s.storage.join(&name)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(m) if m.file_type().is_file() && m.nlink() == 1 => {}
            _ => return Err(refused(format!("export storage holds a linked or non-regular {name}"))),
        }
    }
    if !s.storage.join(PRIMARY).is_file() { return Err(refused(format!("export requires a copied primary database ({PRIMARY})"))); }
    // A hot rollback journal is an interrupted write: the copy is not one consistent state.
    if s.storage.join(format!("{PRIMARY}-journal")).exists() { return Err(refused(format!("{PRIMARY}-journal is present: the copy was taken mid-write"))); }
    match std::fs::symlink_metadata(&s.output) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(m) if m.file_type().is_dir() && std::fs::read_dir(&s.output).is_ok_and(|mut d| d.next().is_none()) => Ok(()),
        _ => Err(refused("export output directory must be empty and not a symlink")),
    }
}

/// A private 0700 directory under the system temp dir, removed when dropped.
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Result<Self, ExportError> {
        let path = std::env::temp_dir().join(format!("deltabadger-export-{}", hex::encode(rand::random::<[u8; 12]>())));
        std::fs::DirBuilder::new().mode(0o700).create(&path).map_err(io("temp dir"))?;
        Ok(TempDir(path))
    }
}
impl Drop for TempDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

/// One source file copied byte for byte: opened without following a link, checked regular with one link, written 0600.
fn copy_file(from: &Path, to: &Path) -> Result<(), ExportError> {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut src = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(from).map_err(io(&format!("reading {name}")))?;
    let m = src.metadata().map_err(io(&format!("reading {name}")))?;
    if !m.file_type().is_file() || m.nlink() != 1 { return Err(refused(format!("export storage holds a linked or non-regular {name}"))); }
    let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(to).map_err(io(&format!("copying {name}")))?;
    std::io::copy(&mut src, &mut dst).map_err(io(&format!("copying {name}")))?;
    dst.flush().map_err(io(&format!("copying {name}")))
}

fn quote(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }

fn names(c: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<Vec<String>> {
    let mut s = c.prepare(sql)?;
    let rows = s.query_map(args, |r| r.get::<_, String>(0))?.collect();
    rows
}

/// The export. Returns the lines to print; the caller exits with `ExportError::exit_code` on an error.
pub fn run(s: &Scratch, env: &dyn Fn(&str) -> Option<String>, now: DateTime<Utc>) -> Result<Vec<String>, ExportError> {
    guard(s, env)?;
    // Held to the end: nothing may start on the source while it is read.
    let _flock = lease::flock_existing(&s.storage.join(".engine.lock")).map_err(|e| match e {
        LeaseError::Locked => refused("the install is in use: another process holds its engine lock"),
        other => ExportError::Failed(format!("engine lock: {other:?}")),
    })?;
    let created_output = !s.output.exists();
    let result = (|| {
        let temp = TempDir::new()?;
        let lines = export(s, &temp.0, now)?;
        drop(temp);
        Ok(lines)
    })();
    if result.is_err() {
        // Rails rm_rf's /export; the directory is the container's, so it is emptied (and removed if this run made it).
        if let Ok(entries) = std::fs::read_dir(&s.output) {
            for e in entries.flatten() {
                let p = e.path();
                let _ = if p.is_dir() && !p.is_symlink() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
        if created_output { let _ = std::fs::remove_dir(&s.output); }
    }
    result
}

fn export(s: &Scratch, temp: &Path, now: DateTime<Utc>) -> Result<Vec<String>, ExportError> {
    let mut lines = vec![];
    // The source is never opened by SQLite: a read-only open of a WAL database can still write its -shm.
    let copy = temp.join("copy.sqlite3");
    copy_file(&s.storage.join(PRIMARY), &copy)?;
    let wal = s.storage.join(format!("{PRIMARY}-wal"));
    if wal.exists() { copy_file(&wal, &temp.join("copy.sqlite3-wal"))?; }
    // The source queue is never read: a fresh empty one, the download's queue too.
    let queue = temp.join("queue.sqlite3");
    store::create_queue_schema(&Connection::open(&queue).map_err(failed("queue"))?).map_err(failed("queue"))?;
    // Opened for writes first: SQLite replays the copied WAL and builds the copy's own -shm.
    let c = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(failed("copy"))?;
    c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).map_err(|_| incompatible("the copy is not a SQLite database"))?;
    store::check(&Paths { primary: copy.clone(), queue: queue.clone() }).map_err(|e| store_error("check", e))?;
    // A generated column could materialise an encoding of a credential into a kept ordinary column.
    let generated = names(&c, "SELECT m.name || '.' || x.name FROM sqlite_master m JOIN pragma_table_xinfo(m.name) x \
                              WHERE m.type = 'table' AND x.hidden <> 0 ORDER BY 1", &[]).map_err(failed("copy"))?;
    if !generated.is_empty() { return Err(incompatible(format!("the copy has generated columns, which this build never creates: {}", generated.join(", ")))); }
    // Not store::configure, which turns them on: an input FK action could write an input default into a kept column.
    c.pragma_update(None, "foreign_keys", "OFF").map_err(failed("copy"))?;
    let objects: Vec<(String, String)> = {
        let mut st = c.prepare("SELECT type, name FROM sqlite_master WHERE type IN ('trigger', 'view') ORDER BY type, name").map_err(failed("copy"))?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(failed("copy"))?.collect::<Result<_, _>>().map_err(failed("copy"))?;
        rows
    };
    for (kind, name) in &objects {
        c.execute_batch(&format!("DROP {} {}", kind.to_uppercase(), quote(name))).map_err(failed("copy"))?;
        lines.push(format!("left out of the download: {kind} {name}"));
    }
    let opened = store::Opened { primary: c, queue: Connection::open(&queue).map_err(failed("queue"))? };
    store::migrate_copy(&opened).map_err(|e| store_error("migrate", e))?;
    drop(opened);

    let canon_path = temp.join("canonical.sqlite3");
    let canon = Connection::open(&canon_path).map_err(failed("canonical"))?;
    project(&canon, &copy, &mut lines)?;
    let tx = rusqlite::Transaction::new_unchecked(&canon, rusqlite::TransactionBehavior::Immediate).map_err(failed("scrub"))?;
    scrub(&tx, now)?;
    tx.commit().map_err(failed("scrub"))?;
    verify(&canon)?;

    std::fs::create_dir_all(s.output.join("portable")).map_err(io("output"))?;
    std::fs::create_dir_all(s.output.join("csv/bots")).map_err(io("output"))?;
    // VACUUM INTO copies live rows only: no free page keeps a deleted credential.
    canon.execute("VACUUM INTO ?1", [s.output.join("portable").join(PRIMARY).to_string_lossy()]).map_err(failed("writing the primary"))?;
    Connection::open(&queue).and_then(|q| q.execute("VACUUM INTO ?1", [s.output.join("portable").join(QUEUE).to_string_lossy()]))
        .map_err(failed("writing the queue"))?;
    csvs(&canon, &s.output.join("csv"))?;
    manifest(&s.output)?;
    lines.push("Portable export complete: /export/manifest.json".into());
    Ok(lines)
}

/// D-EXPORT-CANONICAL-SCHEMA: `canon` gets this build's schema (`store::create_schema` and every Rust migration, as
/// `bootstrap` creates it) with no row of its own, then each of its tables' rows from `copy`, over its own column list.
/// Input tables, columns, indexes and defaults it doesn't have are left out, by name.
fn project(canon: &Connection, copy: &Path, lines: &mut Vec<String>) -> Result<(), ExportError> {
    let f = failed("canonical");
    store::create_schema(canon).map_err(&f)?;
    store::apply_pending(canon).map_err(|e| store_error("canonical", e))?;
    canon.pragma_update(None, "foreign_keys", "OFF").map_err(&f)?;
    let tables = names(canon, "SELECT name FROM main.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name", &[]).map_err(&f)?;
    for t in &tables { canon.execute_batch(&format!("DELETE FROM main.{}", quote(t))).map_err(&f)?; }
    let sequenced = names(canon, "SELECT name FROM main.sqlite_master WHERE type = 'table' AND sql LIKE '%AUTOINCREMENT%' ORDER BY name", &[]).map_err(&f)?;
    canon.execute_batch("DELETE FROM main.sqlite_sequence").map_err(&f)?;
    canon.execute("ATTACH DATABASE ?1 AS copy", [copy.to_string_lossy()]).map_err(&f)?;
    let input = names(canon, "SELECT name FROM copy.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name", &[]).map_err(&f)?;
    for t in &tables {
        if !input.contains(t) { return Err(incompatible(format!("the copy has no table {t}"))); }
        let ours = names(canon, "SELECT name FROM pragma_table_info(?1, 'main') ORDER BY cid", &[t]).map_err(&f)?;
        let theirs = names(canon, "SELECT name FROM pragma_table_info(?1, 'copy') ORDER BY cid", &[t]).map_err(&f)?;
        if let Some(missing) = ours.iter().find(|c| !theirs.contains(c)) { return Err(incompatible(format!("the copy has no column {t}.{missing}"))); }
        for extra in theirs.iter().filter(|c| !ours.contains(c)) { lines.push(format!("left out of the download: column {t}.{extra}")); }
        let cols = ours.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ");
        canon.execute_batch(&format!("INSERT INTO main.{q} ({cols}) SELECT {cols} FROM copy.{q}", q = quote(t))).map_err(&f)?;
    }
    for t in input.iter().filter(|t| !tables.contains(t)) { lines.push(format!("left out of the download: table {t}")); }
    let indexes = names(canon, "SELECT name FROM main.sqlite_master WHERE type = 'index'", &[]).map_err(&f)?;
    for i in names(canon, "SELECT name FROM copy.sqlite_master WHERE type = 'index' AND sql IS NOT NULL ORDER BY name", &[]).map_err(&f)? {
        if !indexes.contains(&i) { lines.push(format!("left out of the download: index {i}")); }
    }
    // A high-water mark survives, for canonical tables only and only as a non-negative integer.
    let has_sequence: bool = canon.query_row("SELECT EXISTS (SELECT 1 FROM copy.sqlite_master WHERE name = 'sqlite_sequence')", [], |r| r.get(0)).map_err(&f)?;
    if has_sequence {
        let mut st = canon.prepare("SELECT name, seq FROM copy.sqlite_sequence WHERE typeof(seq) = 'integer' AND seq >= 0").map_err(&f)?;
        let marks = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(&f)?.collect::<Result<Vec<_>, _>>().map_err(&f)?;
        drop(st);
        for (name, seq) in marks.into_iter().filter(|(n, _)| sequenced.contains(n)) {
            let n = canon.execute("UPDATE main.sqlite_sequence SET seq = max(seq, ?2) WHERE name = ?1", params![name, seq]).map_err(&f)?;
            if n == 0 { canon.execute("INSERT INTO main.sqlite_sequence (name, seq) VALUES (?1, ?2)", params![name, seq]).map_err(&f)?; }
        }
    }
    canon.execute_batch("DETACH DATABASE copy").map_err(&f)?;
    verify(canon)
}

/// `foreign_key_check` and `integrity_check` on the canonical database: a violation refuses, naming the table.
fn verify(c: &Connection) -> Result<(), ExportError> {
    let f = failed("check");
    let broken = names(c, "SELECT DISTINCT \"table\" FROM pragma_foreign_key_check ORDER BY 1", &[]).map_err(&f)?;
    if !broken.is_empty() { return Err(incompatible(format!("the copy has rows whose references are missing, in {}", broken.join(", ")))); }
    let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(&f)?;
    if ok != "ok" { return Err(incompatible("the copy failed SQLite's integrity check")); }
    Ok(())
}

// ---- the scrub (PLAN-B4b §3) ----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// The column is set NULL, the row kept (api_keys, then `status = incorrect`).
    Clear,
    /// The rows are deleted.
    DeleteRows,
    /// Kept or deleted per key (`APP_CONFIGS`, `APP_CONFIGS_DELETED`).
    FilterAppConfigs,
    /// Kept, for the reason given: it ships with the keys that read it.
    Keep(&'static str),
}

/// Every `rotation::COVERAGE` column's export policy (tests: the keys equal COVERAGE's exactly).
pub const POLICY: &[((&str, &str), Policy)] = &[
    (("api_keys", "key"), Policy::Clear), (("api_keys", "secret"), Policy::Clear), (("api_keys", "passphrase"), Policy::Clear),
    (("api_keys", "access_token"), Policy::Clear), (("api_keys", "rsa_signature_key"), Policy::Clear),
    (("api_keys", "rsa_encryption_key"), Policy::Clear), (("api_keys", "dh_param"), Policy::Clear),
    (("fee_api_keys", "key"), Policy::DeleteRows), (("fee_api_keys", "secret"), Policy::DeleteRows), (("fee_api_keys", "passphrase"), Policy::DeleteRows),
    (("app_configs", "value"), Policy::FilterAppConfigs),
    (("users", "otp_secret_key"), Policy::Keep("sign in with two-factor after a restore (Rails keeps it; the keys ship)")),
    (("rules", "address"), Policy::Keep("the owner's own withdrawal addresses (Rails keeps them; the keys ship)")),
    (("exchange_accounts", "account_id_enc"), Policy::Keep("the account a venue key belongs to: a key on another account shows as a conflict (D-EXPORT-ACCOUNT-ANCHORS-KEPT)")),
    (("exchange_account_credentials", "account_id_enc"), Policy::DeleteRows),
    (("exchange_account_imports", "account_id_enc"), Policy::Keep("the account an imported history belongs to (D-EXPORT-ACCOUNT-ANCHORS-KEPT)")),
    (("ibkr_bindings", "account_id_enc"), Policy::Keep("the account an IBKR connection is bound to (D-EXPORT-ACCOUNT-ANCHORS-KEPT)")),
    (("ibkr_bot_accounts", "account_id_enc"), Policy::Keep("the account an IBKR bot belongs to: a restored bot refuses another (D-EXPORT-ACCOUNT-ANCHORS-KEPT)")),
    (("rust_withdrawal_intents", "params_enc"), Policy::Keep("owed withdrawals (dispatched or unresolved), settled by hand after a restore; prepared ones are credential-bound")),
];

/// Rails' Export::Portable::SESSION_TABLES: every row goes (REST/MCP tokens, client secrets, sessions).
pub const SESSION_TABLES: [&str; 8] = ["oauth_access_grants", "oauth_access_tokens", "connected_clients", "oauth_applications",
    "action_mcp_session_messages", "action_mcp_session_subscriptions", "action_mcp_session_tasks", "action_mcp_sessions"];
/// Columns the scrub's own statements clear, besides POLICY and SESSION_TABLES.
pub const CLEARED: &[(&str, &str)] = &[("users", "reset_password_token"), ("users", "confirmation_token"), ("users", "remember_created_at"),
    ("bot_signals", "token")];

/// A secret-shaped column name the export keeps as it is, each with the reason it is no credential.
pub const REVIEWED_PLAIN: &[((&str, &str), &str)] = &[
    (("users", "encrypted_password"), "the bcrypt hash: sign in with the same password after a restore (Rails and launchpad's README promise it)"),
    (("users", "otp_module"), "whether two-factor is on, not its seed"),
    (("rust_withdrawal_intents", "credential_digest"), "SHA-256 of the old key's ciphertext, which is gone: it recovers nothing"),
    (("rust_withdrawal_intents", "key_identity"), "a public fingerprint of the old key"),
    (("rust_api_orders", "credential_version"), "a version stamp of the key a request was validated against"),
    (("users", "last_otp_at"), "when a two-factor code was last used, not the code"),
    (("users", "reset_password_sent_at"), "when a reset mail was sent; its token is cleared"),
    (("app_configs", "key"), "the setting's name; its value is filtered"),
    (("ar_internal_metadata", "key"), "Rails' schema metadata name"),
    (("ibkr_locks", "key"), "the name of an IBKR session lock, no credential"),
    (("idempotency_keys", "key"), "a REST client's own request id (Rails keeps it); no access to anything"),
    (("wash_sale_locks", "claim_token"), "an internal claim id between two of the engine's own steps"),
    (("rust_recorder_runs", "destination_digest"), "a digest of a withdrawal destination the owner chose, kept with the addresses"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key { Exact(&'static str), Prefix(&'static str) }
impl Key {
    pub fn matches(&self, key: &str) -> bool { match self { Key::Exact(k) => key == *k, Key::Prefix(p) => key.starts_with(p) } }
}

/// Rails' deletions (Export::Portable::APP_CONFIG_KEYS and its `proxy_`/`smtp_` prefixes).
pub const APP_CONFIGS_DELETED: &[Key] = &[
    Key::Exact("coingecko_api_key"), Key::Exact("market_data_token"), Key::Exact("market_data_url"), Key::Exact("market_data_provider"),
    Key::Exact("platform_connected_at"), Key::Exact("alpaca_api_key"), Key::Exact("alpaca_api_secret"), Key::Exact("alpaca_mode"),
    Key::Exact("smtp_provider"), Key::Exact("smtp_username"), Key::Exact("smtp_password"), Key::Exact("smtp_host"), Key::Exact("smtp_port"),
    Key::Prefix("proxy_"), Key::Prefix("smtp_"),
];

/// D-EXPORT-APP-CONFIG-ALLOWLIST: the only app_configs rows a download keeps, each with why it is no credential. Any other
/// key is deleted, so a secret stored later under a new key never ships unreviewed (tests scan the source's writers).
pub const APP_CONFIGS: &[(Key, &str)] = &[
    (Key::Exact(crate::identity::KEY_CONFIG), "the HMAC key of the kept account fingerprints; unread, startup clears them (D-EXPORT-ACCOUNT-ANCHORS-KEPT)"),
    (Key::Exact(crate::lease::KEY), "the engine lease: a restore is this engine starting again (D-EXPORT-LEASE-KEPT)"),
    (Key::Exact("registration_open"), "Rails' sign-up setting"),
    (Key::Exact("setup_sync_status"), "Rails' first-sync state"),
    (Key::Exact("stock_sync_enabled"), "Rails' stock-sync setting"),
    (Key::Exact("stock_canonical_backfill_completed_at"), "a reference-data migration's time"),
    (Key::Exact("alpaca_listings_last_good_count"), "a count of Alpaca's listings"),
    (Key::Exact("alpaca_crypto_listings_last_good_count"), "a count of Alpaca's crypto listings"),
    (Key::Prefix("snapshot_history_version_"), "the tracker's per-user backfill version"),
    (Key::Prefix("snapshot_price_generation_"), "the tracker's per-user price generation"),
    (Key::Prefix("snapshot_calculation_version_"), "the tracker's per-user calculation version"),
    (Key::Prefix("rust_job."), "job schedule state (last_error removed: D-EXPORT-JOB-ERRORS)"),
    (Key::Prefix("rust_sync."), "tracker sync cursors and progress"),
    (Key::Exact(crate::lease::CREATED_AT), "the marker of an install this engine created"),
    (Key::Exact(crate::lease::TAKEN_OVER_AT), "when this engine took the install over"),
    (Key::Exact(crate::lease::HANDED_BACK_AT), "when this engine handed it back"),
    (Key::Prefix("rust_tracker_ledger."), "the tracker's cached ledger summary"),
    (Key::Prefix("rust_tracker_gaps."), "the tracker's history gaps"),
    (Key::Prefix("rust_snapshot_prices_pending_"), "a tracker backfill waiting for prices"),
    (Key::Prefix("rust_kucoin_hf_gap."), "a KuCoin history gap"),
    (Key::Prefix("rust_kucoin_order:"), "a KuCoin order's record"),
    (Key::Prefix("rust_kucoin_evidence:"), "a KuCoin order's settlement evidence"),
    (Key::Prefix("rust_kucoin_origin:"), "where a KuCoin order's record came from"),
    (Key::Prefix("rust_kucoin_unresolved:"), "a KuCoin order whose result is unknown"),
    (Key::Prefix("rust_cancel_reread:"), "a cancel whose result is re-read"),
    (Key::Prefix("rust_binance_a12_v1:"), "the Binance history repair's journal and marker"),
    (Key::Prefix("rust_binance_repair_gaps."), "Binance history gaps"),
    (Key::Exact("rust_rules_fired_at"), "when rules last ran"),
    (Key::Exact("rust_rule_import"), "the Rails rule logs the takeover imported"),
    (Key::Exact("rust_rule_job_cleanup"), "the Rails rule jobs still to clean up"),
    (Key::Exact("rust_coinbase_size_in_quote_repaired_at"), "a Coinbase repair's time"),
    (Key::Exact("rust_rails_unrecorded.live_cursor"), "the unrecorded-orders check's cursor"),
    (Key::Exact("rust_rails_queue_done"), "the Rails jobs the takeover already ran"),
    (Key::Exact("rust_rails_wakes"), "the Rails wakes the takeover carried over"),
    (Key::Exact("rust_signal_import"), "the Rails signal calls the takeover imported"),
    (Key::Exact("rust_app_update"), "the update check's state"),
    (Key::Exact("rust_set_aside"), "the bots the takeover set aside"),
];

/// Whether a download keeps the app_configs row under `key`.
pub fn app_config_kept(key: &str) -> bool {
    !APP_CONFIGS_DELETED.iter().any(|k| k.matches(key)) && APP_CONFIGS.iter().any(|(k, _)| k.matches(key))
}

/// The activity line of a swap halted for the download (D-EXPORT-SWAPS-HALTED).
pub const HALT_REASON: &str = "This copy of the bot was exported for download; check the swap on the exchange before resuming it.";

/// The scrub, on the canonical database, in the caller's transaction. Raw writes and deletes only: nothing is decrypted.
pub fn scrub(tx: &Connection, now: DateTime<Utc>) -> Result<(), ExportError> {
    let f = failed("scrub");
    // Rails' policy on its own tables, statement for statement (update_all and update_columns touch no stamp).
    let cleared = POLICY.iter().filter(|(_, p)| *p == Policy::Clear).map(|((_, c), _)| format!("{c} = NULL")).collect::<Vec<_>>().join(", ");
    tx.execute_batch(&format!("UPDATE api_keys SET {cleared}, status = 2")).map_err(&f)?;
    tx.execute_batch("DELETE FROM fee_api_keys").map_err(&f)?;
    let keys = names(tx, "SELECT key FROM app_configs ORDER BY id", &[]).map_err(&f)?;
    for key in keys.iter().filter(|k| !app_config_kept(k)) { tx.execute("DELETE FROM app_configs WHERE key = ?1", [key]).map_err(&f)?; }
    tx.execute_batch("UPDATE bots SET status = 2 WHERE status NOT IN (3, 7); UPDATE rules SET status = 2 WHERE status NOT IN (3, 7)").map_err(&f)?;
    for id in tx.prepare("SELECT id FROM bot_signals ORDER BY id").and_then(|mut s| s.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()).map_err(&f)? {
        tx.execute("UPDATE bot_signals SET token = ?1, enabled = 0 WHERE id = ?2", params![hex::encode(rand::random::<[u8; 32]>()), id]).map_err(&f)?;
    }
    tx.execute_batch("UPDATE users SET reset_password_token = NULL, confirmation_token = NULL, remember_created_at = NULL").map_err(&f)?;
    for t in SESSION_TABLES { tx.execute_batch(&format!("DELETE FROM {t}")).map_err(&f)?; }
    // Rust's own rows.
    // Rust's: the old key's observations, IBKR reservations and never-sent withdrawals go; the account anchors stay.
    tx.execute_batch("DELETE FROM exchange_account_credentials; DELETE FROM ibkr_reservations; \
                      DELETE FROM rust_withdrawal_intents WHERE state = 'prepared'").map_err(&f)?;
    crate::rotation::rebalancing_off(tx).map_err(&f)?;
    let engine = |e: crate::engine::EngineError| ExportError::Failed(format!("scrub: {e:?}"));
    let mut st = tx.prepare("SELECT id, transient_data FROM bots ORDER BY id").map_err(&f)?;
    let bots = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).map_err(&f)?.collect::<Result<Vec<_>, _>>().map_err(&f)?;
    drop(st);
    for (id, transient) in bots {
        let t: serde_json::Value = transient.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default();
        if crate::engine::rebalance::is_pending(&t) { crate::engine::rebalance::halt(tx, id, HALT_REASON, now).map_err(engine)?; }
        let clock = crate::engine::FixedClock(now);
        for (key, liquidating) in [(crate::engine::liquidation::REQUEST_KEY, true), (crate::engine::redeploy::REQUEST_KEY, false)] {
            let Some(request) = t.get(key).filter(|r| r.is_object()) else { continue };
            if liquidating {
                crate::engine::liquidation::clear(tx, id, request).map_err(engine)?;
                crate::engine::liquidation::refuse(tx, id, serde_json::json!("exported"), &clock).map_err(engine)?;
            } else {
                crate::engine::redeploy::clear(tx, id, request).map_err(engine)?;
                crate::engine::redeploy::refuse(tx, id, serde_json::json!("exported"), &clock).map_err(engine)?;
            }
        }
    }
    tx.execute_batch("UPDATE app_configs SET value = json_remove(value, '$.last_error') WHERE substr(key, 1, 9) = 'rust_job.' \
                      AND json_valid(value) AND json_type(value) = 'object' AND json_type(value, '$.last_error') IS NOT NULL").map_err(&f)?;
    Ok(())
}

// ---- the files (PLAN-B4b §2.1, §4) ----------------------------------------------------------------------------------

/// I18n::Backend::Transliterator::HashTransliterator::DEFAULT_APPROXIMATIONS, recorded from Ruby (tests compare).
pub const TRANSLITERATION: &[(char, &str)] = &[
    ('À', "A"), ('Á', "A"), ('Â', "A"), ('Ã', "A"), ('Ä', "A"), ('Å', "A"), ('Æ', "AE"), ('Ç', "C"), ('È', "E"),
    ('É', "E"), ('Ê', "E"), ('Ë', "E"), ('Ì', "I"), ('Í', "I"), ('Î', "I"), ('Ï', "I"), ('Ð', "D"), ('Ñ', "N"),
    ('Ò', "O"), ('Ó', "O"), ('Ô', "O"), ('Õ', "O"), ('Ö', "O"), ('×', "x"), ('Ø', "O"), ('Ù', "U"), ('Ú', "U"),
    ('Û', "U"), ('Ü', "U"), ('Ý', "Y"), ('Þ', "Th"), ('ß', "ss"), ('à', "a"), ('á', "a"), ('â', "a"), ('ã', "a"),
    ('ä', "a"), ('å', "a"), ('æ', "ae"), ('ç', "c"), ('è', "e"), ('é', "e"), ('ê', "e"), ('ë', "e"), ('ì', "i"),
    ('í', "i"), ('î', "i"), ('ï', "i"), ('ð', "d"), ('ñ', "n"), ('ò', "o"), ('ó', "o"), ('ô', "o"), ('õ', "o"),
    ('ö', "o"), ('ø', "o"), ('ù', "u"), ('ú', "u"), ('û', "u"), ('ü', "u"), ('ý', "y"), ('þ', "th"), ('ÿ', "y"),
    ('Ā', "A"), ('ā', "a"), ('Ă', "A"), ('ă', "a"), ('Ą', "A"), ('ą', "a"), ('Ć', "C"), ('ć', "c"), ('Ĉ', "C"),
    ('ĉ', "c"), ('Ċ', "C"), ('ċ', "c"), ('Č', "C"), ('č', "c"), ('Ď', "D"), ('ď', "d"), ('Đ', "D"), ('đ', "d"),
    ('Ē', "E"), ('ē', "e"), ('Ĕ', "E"), ('ĕ', "e"), ('Ė', "E"), ('ė', "e"), ('Ę', "E"), ('ę', "e"), ('Ě', "E"),
    ('ě', "e"), ('Ĝ', "G"), ('ĝ', "g"), ('Ğ', "G"), ('ğ', "g"), ('Ġ', "G"), ('ġ', "g"), ('Ģ', "G"), ('ģ', "g"),
    ('Ĥ', "H"), ('ĥ', "h"), ('Ħ', "H"), ('ħ', "h"), ('Ĩ', "I"), ('ĩ', "i"), ('Ī', "I"), ('ī', "i"), ('Ĭ', "I"),
    ('ĭ', "i"), ('Į', "I"), ('į', "i"), ('İ', "I"), ('ı', "i"), ('Ĳ', "IJ"), ('ĳ', "ij"), ('Ĵ', "J"), ('ĵ', "j"),
    ('Ķ', "K"), ('ķ', "k"), ('ĸ', "k"), ('Ĺ', "L"), ('ĺ', "l"), ('Ļ', "L"), ('ļ', "l"), ('Ľ', "L"), ('ľ', "l"),
    ('Ŀ', "L"), ('ŀ', "l"), ('Ł', "L"), ('ł', "l"), ('Ń', "N"), ('ń', "n"), ('Ņ', "N"), ('ņ', "n"), ('Ň', "N"),
    ('ň', "n"), ('ŉ', "'n"), ('Ŋ', "NG"), ('ŋ', "ng"), ('Ō', "O"), ('ō', "o"), ('Ŏ', "O"), ('ŏ', "o"), ('Ő', "O"),
    ('ő', "o"), ('Œ', "OE"), ('œ', "oe"), ('Ŕ', "R"), ('ŕ', "r"), ('Ŗ', "R"), ('ŗ', "r"), ('Ř', "R"), ('ř', "r"),
    ('Ś', "S"), ('ś', "s"), ('Ŝ', "S"), ('ŝ', "s"), ('Ş', "S"), ('ş', "s"), ('Š', "S"), ('š', "s"), ('Ţ', "T"),
    ('ţ', "t"), ('Ť', "T"), ('ť', "t"), ('Ŧ', "T"), ('ŧ', "t"), ('Ũ', "U"), ('ũ', "u"), ('Ū', "U"), ('ū', "u"),
    ('Ŭ', "U"), ('ŭ', "u"), ('Ů', "U"), ('ů', "u"), ('Ű', "U"), ('ű', "u"), ('Ų', "U"), ('ų', "u"), ('Ŵ', "W"),
    ('ŵ', "w"), ('Ŷ', "Y"), ('ŷ', "y"), ('Ÿ', "Y"), ('Ź', "Z"), ('ź', "z"), ('Ż', "Z"), ('ż', "z"), ('Ž', "Z"),
    ('ž', "z"), ('Ǫ', "O"), ('ǫ', "o"), ('Ǭ', "O"), ('ǭ', "o"), ('ẞ', "SS"),
];

/// The file label Rails writes: `label.parameterize.gsub(/[^a-z0-9-]/, '').first(50).presence || 'bot'`.
/// parameterize = I18n.transliterate (an unmapped non-ASCII character is `?`), runs of anything but `[a-z0-9_-]` (any
/// case) → `-`, repeated `-` squeezed, a leading and trailing `-` removed, lowercased (D-EXPORT-LABEL-NFC).
pub fn file_label(label: &str) -> String {
    let mut ascii = String::new();
    for ch in label.chars() {
        if ch.is_ascii() { ascii.push(ch) } else { ascii.push_str(TRANSLITERATION.iter().find(|(c, _)| *c == ch).map_or("?", |(_, s)| s)) }
    }
    // gsub(/[^a-z0-9\-_]+/i, '-'), then /-{2,}/ squeezed.
    let mut squeezed = String::new();
    for ch in ascii.chars() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '_' { ch } else { '-' };
        if !(ch == '-' && squeezed.ends_with('-')) { squeezed.push(ch) }
    }
    let trimmed = squeezed.strip_prefix('-').unwrap_or(&squeezed);
    let trimmed = trimmed.strip_suffix('-').unwrap_or(trimmed);
    let out: String = trimmed.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-').take(50).collect();
    if out.is_empty() { "bot".into() } else { out }
}

/// Rails' csv/README_CSV.txt.
pub fn readme_csv() -> String {
    format!("Exported by Deltabadger {}.\nRestore with this version or newer.\n\n\
             tracker.csv is transaction interchange: it excludes manual prices, transfer links\n\
             and raw data. Import it using the Deltabadger CSV format.\n\
             bots/*.csv holds submitted, closed orders only, one file per bot.\n\
             These files do not contain bot configuration.\n", crate::VERSION)
}

fn csvs(c: &Connection, dir: &Path) -> Result<(), ExportError> {
    let web = |step: &'static str| move |e: crate::web::WebError| ExportError::Failed(format!("{step}: {e:?}"));
    let tracker = crate::web::tracker::csv_rows(c, &format!("SELECT {} FROM account_transactions ORDER BY transacted_at ASC", crate::web::tracker::CSV_COLUMNS), vec![])
        .map_err(web("tracker.csv"))?;
    std::fs::write(dir.join("tracker.csv"), tracker).map_err(io("tracker.csv"))?;
    let f = failed("bots");
    let mut st = c.prepare("SELECT id, user_id, coalesce(type, ''), label, settings FROM bots ORDER BY id").map_err(&f)?;
    let bots = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)))
        .map_err(&f)?.collect::<Result<Vec<_>, _>>().map_err(&f)?;
    drop(st);
    for (id, user, kind, label, settings) in bots {
        // Bot#label: a blank stored name reads as Automation::Labelable#generate_label (after_find), in the default locale.
        let label = match label.filter(|l| !l.trim().is_empty()) {
            Some(l) => l,
            None => {
                let settings: serde_json::Value = settings.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
                user.and_then(|u| crate::web::mcp::reads::generated_label(c, u, id, &kind, &settings, "en").ok())
                    .unwrap_or_else(|| crate::web::i18n::text("en", "bot.new", &[]))
            }
        };
        let body = crate::web::bot::csv::export_csv(c, id).map_err(web("bot csv"))?.unwrap_or_default();
        std::fs::write(dir.join("bots").join(format!("{id}-{}.csv", file_label(&label))), body).map_err(io("bot csv"))?;
    }
    std::fs::write(dir.join("README_CSV.txt"), readme_csv()).map_err(io("README_CSV.txt"))
}

/// The newest Rust migration of this build (D-EXPORT-SCHEMA-RUST).
pub fn schema_version() -> i64 {
    store::REGISTRY.migrations.iter().filter_map(|m| m.version.parse::<i64>().ok()).max().unwrap_or(0)
}

fn manifest(output: &Path) -> Result<(), ExportError> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() { walk(root, &p, out)? } else if p.is_file() { out.push(p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned()) }
        }
        Ok(())
    }
    let mut files = vec![];
    walk(output, output, &mut files).map_err(io("manifest"))?;
    files.retain(|f| f != "manifest.json");
    files.sort();
    let m = serde_json::json!({ "app_version": crate::VERSION, "schema_version": schema_version(), "files": files });
    let text = serde_json::to_string_pretty(&m).map_err(|e| ExportError::Failed(format!("manifest: {e}")))?;
    std::fs::write(output.join("manifest.json"), format!("{text}\n")).map_err(io("manifest"))
}
