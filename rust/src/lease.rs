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
    Ok(EngineLock { _file: std::sync::Arc::new(file) })
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

fn write(primary: &Connection, cipher: &Cipher, value: &Value, now: DateTime<Utc>) -> Result<(), LeaseError> {
    primary.execute(
        "INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        (KEY, cipher.encrypt(&value.to_string()), format_time(now)))?;
    Ok(())
}

pub fn claim(_proof: &EngineLock, primary: &Connection, cipher: &Cipher, version: &str, now: DateTime<Utc>) -> Result<Claim, LeaseError> {
    let from = match read(primary, cipher)? {
        None => Claim::FromRails,
        Some(v) if v["engine"] == "rust" => Claim::AfterCrash,
        Some(v) if v["released_by"] == "rust" && v["handed_back"] == true => Claim::AfterHandback,
        Some(_) => Claim::FromRails, // `none` written by a Rails-side tool: Rails owned it last
    };
    write(primary, cipher, &json!({ "engine": "rust", "version": version, "since": now.to_rfc3339() }), now)?;
    Ok(from)
}

/// Plan 2 calls this inside the same transaction that leaves every working bot `scheduled`.
pub fn hand_back(_proof: &EngineLock, primary: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<(), LeaseError> {
    write(primary, cipher, &json!({ "engine": "none", "released_by": "rust", "handed_back": true, "at": now.to_rfc3339() }), now)
}
