//! R: cache provenance lives in the existing Rust job-state store. History is retained.
use crate::{app_config, engine::{model::{self, CredentialVersion}, EngineError}, jobs::state};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

fn key(user: i64, exchange: i64) -> String {
    state::key("balance_origin", Some(&format!("{user}:{exchange}")))
}
/// Call in the checked balance write transaction. Incomplete batches cannot publish current figures.
pub fn record(c: &model::FencedTransaction<'_>, user: i64, exchange: i64, version: &CredentialVersion, complete: bool, now: DateTime<Utc>) -> Result<(), EngineError> {

    let mut stamp = version.cache_stamp();
    stamp["complete"] = Value::Bool(complete);
    app_config::set_plain(c, &key(user, exchange), &stamp.to_string(), now).map_err(EngineError::Data)
}
/// An old balance price may be reused only by the credential version that produced it.
pub fn produced_by(c: &Connection, user: i64, exchange: i64, version: &CredentialVersion) -> Result<bool, EngineError> {
    let stamp = app_config::get_plain(c, &key(user, exchange)).map_err(EngineError::Data)?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(stamp.is_some_and(|stamp| stamp["complete"] == Value::Bool(true) && version.matches_cache_stamp(&stamp)))
}
/// Missing, malformed, partial or differently credentialed cache origins are sync-pending.
/// Select the same reading slot as the sync scheduler: trading before read-only, never withdrawal.
pub fn stale(c: &Connection, user: i64, exchange: Option<i64>) -> Result<bool, EngineError> {
    let mut stmt = c.prepare("SELECT DISTINCT exchange_id FROM account_balances WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2) UNION SELECT DISTINCT exchange_id FROM api_keys WHERE user_id=?1 AND key_type!=1 AND (?2 IS NULL OR exchange_id=?2)")?;
    let scopes = stmt.query_map(rusqlite::params![user, exchange], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    for exchange in scopes {
        let id: Option<i64> = c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND status=1 AND key_type!=1 ORDER BY CASE WHEN key_type=0 THEN 0 ELSE 1 END,id LIMIT 1", [user, exchange], |r| r.get(0)).optional()?;
        let current = id.map(|id| model::credential_version_by_id(c, id)).transpose()?.flatten();
        let stamp = app_config::get_plain(c, &key(user, exchange)).map_err(EngineError::Data)?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        if !matches!((current, stamp), (Some(version), Some(stamp)) if stamp["complete"] == Value::Bool(true) && version.matches_cache_stamp(&stamp)) { return Ok(true); }
    }
    Ok(false)
}

/// A read's credential set, captured alongside the cached values in one SQLite snapshot.
/// The reader that owns that snapshot checks again after releasing it, so an intervening
/// replacement cannot make an old value current through a newly completed cache stamp.
pub struct ReadOrigin { user:i64, exchange:Option<i64>, versions:Vec<CredentialVersion> }
pub fn capture_read(c:&Connection,user:i64,exchange:Option<i64>)->Result<ReadOrigin,EngineError>{
    let mut stmt=c.prepare("SELECT id FROM api_keys WHERE user_id=?1 AND key_type!=1 AND (?2 IS NULL OR exchange_id=?2) ORDER BY id")?;
    let ids=stmt.query_map(rusqlite::params![user,exchange],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    let versions=ids.into_iter().map(|id|model::credential_version_by_id(c,id)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten().collect();
    Ok(ReadOrigin{user,exchange,versions})
}
pub fn read_is_current(c:&Connection,origin:&ReadOrigin)->Result<bool,EngineError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=read_is_current(&tx,origin)?;tx.commit()?;return Ok(out)}
    let current=capture_read(c,origin.user,origin.exchange)?;
    Ok(current.versions.len()==origin.versions.len()&&current.versions.iter().zip(&origin.versions).all(|(a,b)|a.same_credentials(b)))
}

/// The incremental ledger watermark belongs only to its completed credential version.
pub fn ledger_produced_by(c:&Connection,id:i64,version:&CredentialVersion)->Result<bool,EngineError>{
    let stamp=app_config::get_plain(c,&state::key("ledger_origin",Some(&id.to_string()))).map_err(EngineError::Data)?.and_then(|s|serde_json::from_str::<Value>(&s).ok());
    Ok(stamp.is_some_and(|stamp|version.matches_cache_stamp(&stamp)))
}
/// Called inside the Q-checked completion transaction, alongside the public watermark.
pub fn record_ledger(c:&model::FencedTransaction<'_>,id:i64,version:&CredentialVersion,now:DateTime<Utc>)->Result<(),EngineError>{

    app_config::set_plain(c,&state::key("ledger_origin",Some(&id.to_string())),&version.cache_stamp().to_string(),now).map_err(EngineError::Data)
}

pub fn fence_read<'a>(tx:&'a rusqlite::Transaction<'_>,origin:&ReadOrigin)->Result<model::FencedTransaction<'a>,EngineError>{
    let fenced=model::fence_versions(tx,&origin.versions)?;
    if !read_is_current(tx,origin)?{crate::engine::log(model::CREDENTIALS_CHANGED);return Err(EngineError::CredentialsChanged)}
    Ok(fenced)
}
