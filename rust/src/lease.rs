//! Never two engines on one install.
//! - One lifetime lock: flock on `<primary dir>/.engine.lock`. Every Rails process from the floor
//!   release on holds it SHARED (config/initializers/00_engine_lock.rb); Rust holds it EXCLUSIVE from
//!   before it reads or writes any database until it exits. A paused process keeps its lock.
//! - Rails releases older than the floor do not take it, so a Solid Queue heartbeat under 120 s old
//!   also refuses. That only catches a running one; against those the guarantee stays external
//!   (one container per instance).
//! - The handover row app_configs['engine_lease'] says who last owned the install. Rails refuses to boot
//!   while it says "rust", i.e. until `deltabadger handback` has run.
use crate::codec::{format_time, parse_time};
use crate::crypto::Cipher;
use crate::store::Paths;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};

pub const KEY: &str = "engine_lease";
const RAILS_ALIVE_WITHIN_SECONDS: i64 = 120;

#[derive(Debug)]
pub enum LeaseError {
    Locked,
    RailsAlive { seconds_ago: i64 },
    Unreadable,
    BinanceIdentityWritten,
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}
impl From<rusqlite::Error> for LeaseError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<std::io::Error> for LeaseError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

#[derive(Debug)]
pub enum Claim {
    FromRails,
    AfterHandback,
    AfterCrash,
}

/// Proof that this process holds the exclusive engine lock. A clone is the same lock: `serve` keeps one until the
/// process exits, after the engine that owned the first has returned. The lock is released when the last clone drops.
#[derive(Clone)]
pub struct EngineLock {
    _file: std::sync::Arc<File>,
}

pub fn lock(paths: &Paths, now: DateTime<Utc>) -> Result<EngineLock, LeaseError> {
    let path = paths.lock_file();
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    // Read-only when it exists: under Docker the Rails entrypoint may have created it as root (0644),
    // and flock needs no write access. Create it only when missing.
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => OpenOptions::new().write(true).create(true).truncate(false).open(&path)?,
        Err(e) => return Err(e.into()),
    };
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => LeaseError::Locked,
        std::fs::TryLockError::Error(io) => LeaseError::Io(io),
    })?;

    if paths.queue.exists() {
        let q = Connection::open_with_flags(&paths.queue, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let has_table: bool = q.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'solid_queue_processes'", [], |r| r.get::<_, i64>(0))? > 0;
        if has_table {
            let latest: Option<String> = q.query_row("SELECT max(last_heartbeat_at) FROM solid_queue_processes", [], |r| r.get(0))?;
            if let Some(at) = latest.as_deref().and_then(|s| parse_time(s).ok()) {
                let ago = (now - at).num_seconds();
                if ago < RAILS_ALIVE_WITHIN_SECONDS { return Err(LeaseError::RailsAlive { seconds_ago: ago }); }
            }
        }
    }
    let file = std::sync::Arc::new(file);
    HELD.lock().unwrap_or_else(|e| e.into_inner()).push(std::sync::Arc::downgrade(&file));
    Ok(EngineLock { _file: file })
}

/// Every engine lock this process took, for `release_all`.
static HELD: std::sync::Mutex<Vec<std::sync::Weak<File>>> = std::sync::Mutex::new(Vec::new());

/// Gives up every engine lock this process still holds, for a process that will never touch the install again but does
/// not exit yet: `serve` showing its refusal page (web::refused) before it has claimed anything. The handles stay open;
/// only the flock goes, so Rails or another engine may start on the install.
pub fn release_all() {
    for file in HELD.lock().unwrap_or_else(|e| e.into_inner()).drain(..).filter_map(|f| f.upgrade()) { let _ = file.unlock(); }
}

pub fn read(primary: &Connection, cipher: &Cipher) -> Result<Option<Value>, LeaseError> {
    let raw: Option<String> = primary.query_row("SELECT value FROM app_configs WHERE key = ?1", [KEY], |r| r.get(0)).optional()?;
    raw.map(|r| {
        cipher.decrypt(&r).ok()
            .and_then(|p| serde_json::from_str::<Value>(&p).ok())
            .filter(|v| v.get("engine").and_then(Value::as_str).is_some())
            .ok_or(LeaseError::Unreadable)
    }).transpose()
}

/// When this engine last handed the install back (`handover::hand_back_since`, in the handback's transaction) and last
/// began an ownership (`claim`, with the lease row). Plain `app_configs` rows: `check` reads them without keys
/// (engine::staleness), and Rails never reads them (its adoption deletes only `engine_lease`).
pub const HANDED_BACK_AT: &str = "rust_handed_back_at";
pub const TAKEN_OVER_AT: &str = "rust_taken_over_at";

fn upsert(primary: &Connection, key: &str, value: &str, now: DateTime<Utc>) -> Result<(), LeaseError> {
    primary.execute(
        "INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        (key, value, format_time(now)))?;
    Ok(())
}

fn write(primary: &Connection, cipher: &Cipher, value: &Value, now: DateTime<Utc>) -> Result<(), LeaseError> {
    upsert(primary, KEY, &cipher.encrypt(&value.to_string()), now)
}

/// A plain `app_configs` time (HANDED_BACK_AT, TAKEN_OVER_AT); unparseable reads as absent.
pub fn plain_at(c: &Connection, key: &str) -> Result<Option<DateTime<Utc>>, LeaseError> {
    let raw: Option<Option<String>> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    Ok(raw.flatten().and_then(|t| t.parse().ok()))
}

/// The marker of an install this engine created (store::bootstrap), plain and never deleted: `handback` refuses on it
/// (PLAN-B1 r2.1), since there is no earlier version to hand it back to.
pub const CREATED_AT: &str = "rust_created_at";

fn owned(version: &str, now: DateTime<Utc>) -> Value { json!({ "engine": "rust", "version": version, "since": now.to_rfc3339() }) }

/// A new install's ownership, in store::bootstrap's transaction (PLAN-B1 r1.1): the lease row and TAKEN_OVER_AT as a new
/// ownership writes them, and CREATED_AT. Rails refuses to boot on the file from its first commit, and the first `claim`
/// is AfterCrash, continuing this ownership without a new TAKEN_OVER_AT.
pub fn begin_created(_proof: &EngineLock, primary: &Connection, cipher: &Cipher, version: &str, now: DateTime<Utc>) -> Result<(), LeaseError> {
    let at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    write(primary, cipher, &owned(version, now), now)?;
    upsert(primary, TAKEN_OVER_AT, &at, now)?;
    upsert(primary, CREATED_AT, &at, now)
}

/// Two statements, so the takeover runs it inside its own transaction (handover::take_over): the lease row and the end of
/// Rails' ownership window commit together, and a crash can never leave that window open.
pub fn claim(_proof: &EngineLock, primary: &Connection, cipher: &Cipher, version: &str, now: DateTime<Utc>) -> Result<Claim, LeaseError> {
    let from = match read(primary, cipher)? {
        None => Claim::FromRails,
        Some(v) if v["engine"] == "rust" => Claim::AfterCrash,
        Some(v) if v["released_by"] == "rust" && v["handed_back"] == true => Claim::AfterHandback,
        Some(_) => Claim::FromRails, // `none` written by a Rails-side tool: Rails owned it last
    };
    write(primary, cipher, &owned(version, now), now)?;
    // A new ownership closes Rails' window. A restart after a crash continues this engine's own, unless the window it would
    // close is still open (a claim by a build that did not stamp).
    let new_ownership = match from {
        Claim::FromRails | Claim::AfterHandback => true,
        Claim::AfterCrash => match plain_at(primary, HANDED_BACK_AT)? {
            Some(back) => plain_at(primary, TAKEN_OVER_AT)?.is_none_or(|taken| taken < back),
            None => false,
        },
    };
    if new_ownership { upsert(primary, TAKEN_OVER_AT, &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), now)?; }
    Ok(from)
}

/// The engine calls this inside the same transaction that leaves every working bot `scheduled`.
pub fn hand_back(_proof: &EngineLock, primary: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<(), LeaseError> {
    if primary.is_autocommit() {
        let tx=rusqlite::Transaction::new_unchecked(primary,rusqlite::TransactionBehavior::Immediate)?;
        hand_back(_proof,&tx,cipher,now)?;tx.commit()?;return Ok(());
    }
    if crate::sync::binance_writer::written(primary)? {return Err(LeaseError::BinanceIdentityWritten)}
    write(primary, cipher, &json!({ "engine": "none", "released_by": "rust", "handed_back": true, "at": now.to_rfc3339() }), now)
}
