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

/// AppConfig.set(key, value) with a value that may be nil (a parameter the request did not send): a nil value on a row
/// that holds nil is not dirty; a new row is saved with a NULL value.
pub fn set_nullable(c: &Connection, cipher: &Cipher, key: &str, value: Option<&str>, now: DateTime<Utc>) -> Result<(), String> {
    match (value, stored(c, key)?) {
        (Some(value), _) => set(c, cipher, key, value, now),
        (None, Some(None)) => Ok(()),
        (None, Some(Some(_))) => c.execute("UPDATE app_configs SET value = NULL, updated_at = ?2 WHERE key = ?1", (key, format_time(now)))
            .map(drop).map_err(|e| format!("app_configs[{key}]: {e}")),
        (None, None) => c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, NULL, ?2, ?2)", (key, format_time(now)))
            .map(drop).map_err(|e| format!("app_configs[{key}]: {e}")),
    }
}

/// AppConfig.delete(key): `find_by(key:)&.destroy`.
pub fn delete(c: &Connection, key: &str) -> Result<(), String> {
    c.execute("DELETE FROM app_configs WHERE key = ?1", [key]).map(drop).map_err(|e| format!("app_configs[{key}]: {e}"))
}

/// A key only Rust reads and writes, stored plain: there is no secret in it, so `deltabadger check` reads it without the
/// instance's keys. Rails never reads these keys, and would read a plain value as itself.
pub fn get_plain(c: &Connection, key: &str) -> Result<Option<String>, String> { Ok(stored(c, key)?.flatten()) }

pub fn set_plain(c: &Connection, key: &str, value: &str, now: DateTime<Utc>) -> Result<(), String> { write(c, key, value, now) }

/// An update of the existing row, else an insert, as find_or_initialize_by + save! does. (An upsert would spend an
/// AUTOINCREMENT id on every update, and the next new row would get a different id than Rails gives it.)
fn write(c: &Connection, key: &str, value: &str, now: DateTime<Utc>) -> Result<(), String> {
    let at = format_time(now);
    let updated = c.execute("UPDATE app_configs SET value = ?2, updated_at = ?3 WHERE key = ?1", (key, value, &at)).map_err(|e| format!("app_configs[{key}]: {e}"))?;
    if updated == 0 {
        c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", (key, value, &at)).map_err(|e| format!("app_configs[{key}]: {e}"))?;
    }
    Ok(())
}
