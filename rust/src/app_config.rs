//! `app_configs` as AppConfig reads and writes it (app/models/app_config.rb:4, :106-119). `value` is an encrypted attribute,
//! read with support_unencrypted_data (config/initializers/active_record_encryption.rb:131): a plain value reads as itself.
use crate::codec::format_time;
use crate::crypto::Cipher;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

/// The stored text, and whether a row exists at all: `Some(None)` is a row whose value is NULL.
fn stored(c: &Connection, key: &str) -> Result<Option<Option<String>>, String> {
    c.query_row("SELECT value FROM app_configs WHERE key = ?1", [key], |r| r.get(0)).optional().map_err(|e| format!("app_configs[{key}]: {e}"))
}

/// AppConfig.get(key): the decrypted value; None without a row or for a NULL value.
pub fn get(c: &Connection, cipher: &Cipher, key: &str) -> Result<Option<String>, String> {
    stored(c, key)?.flatten()
        .map(|v| cipher.decrypt(&v).map_err(|e| format!("app_configs[{key}] is unreadable: {e:?}")))
        .transpose()
}

/// Whether a row exists (AppConfig.market_data_url's `return record.value if record`).
pub fn exists(c: &Connection, key: &str) -> Result<bool, String> { Ok(stored(c, key)?.is_some()) }

/// AppConfig.set(key, value): find_or_initialize_by + save!. An unchanged value is not dirty, so nothing is written and
/// updated_at stays. Otherwise the value is encrypted afresh and updated_at (on insert also created_at) is `now`.
pub fn set(c: &Connection, cipher: &Cipher, key: &str, value: &str, now: DateTime<Utc>) -> Result<(), String> {
    if get(c, cipher, key)?.as_deref() == Some(value) { return Ok(()); }
    write(c, key, &cipher.encrypt(value), now)
}

/// A key only Rust reads and writes, stored plain: there is no secret in it, so `deltabadger check` reads it without the
/// instance's keys. Rails never reads these keys, and would read a plain value as itself.
pub fn get_plain(c: &Connection, key: &str) -> Result<Option<String>, String> { Ok(stored(c, key)?.flatten()) }

pub fn set_plain(c: &Connection, key: &str, value: &str, now: DateTime<Utc>) -> Result<(), String> { write(c, key, value, now) }

fn write(c: &Connection, key: &str, value: &str, now: DateTime<Utc>) -> Result<(), String> {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) \
               ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              (key, value, format_time(now)))
        .map(|_| ())
        .map_err(|e| format!("app_configs[{key}]: {e}"))
}
