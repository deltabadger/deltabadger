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
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=produced_by(&tx,user,exchange,version)?;tx.commit()?;return Ok(out)}
    let stamp = app_config::get_plain(c, &key(user, exchange)).map_err(EngineError::Data)?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(stamp.is_some_and(|stamp| stamp["complete"] == Value::Bool(true) && model::stamp_current_for(&stamp,&version.cache_stamp()).is_fresh()))
}
/// Missing, malformed, partial or differently credentialed cache origins are sync-pending.
/// Select the same reading slot as the sync scheduler: trading before read-only, never withdrawal.
pub fn stale(c: &Connection, user: i64, exchange: Option<i64>) -> Result<bool, EngineError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=stale(&tx,user,exchange)?;tx.commit()?;return Ok(out)}
    let mut stmt = c.prepare("SELECT DISTINCT exchange_id FROM account_balances WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2) UNION SELECT DISTINCT exchange_id FROM api_keys WHERE user_id=?1 AND key_type!=1 AND (?2 IS NULL OR exchange_id=?2)")?;
    let scopes = stmt.query_map(rusqlite::params![user, exchange], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    for exchange in scopes {
        let id: Option<i64> = c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND status=1 AND key_type!=1 ORDER BY CASE WHEN key_type=0 THEN 0 ELSE 1 END,id LIMIT 1", [user, exchange], |r| r.get(0)).optional()?;
        let stamp = app_config::get_plain(c, &key(user, exchange)).map_err(EngineError::Data)?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let producer=stamp.as_ref().and_then(CredentialVersion::from_stamp);
        let fresh=match (&producer,id,&stamp) {
            (Some(producer),Some(id),Some(stamp)) if producer.id==id && stamp["complete"]==Value::Bool(true)=>model::credential_is_current(c,producer)?,
            _=>false,
        };
        if !fresh { return Ok(true); }
    }
    Ok(false)
}

/// A read's credential set, captured alongside the cached values in one SQLite snapshot.
/// The reader that owns that snapshot checks again after releasing it, so an intervening
/// replacement cannot make an old value current through a newly completed cache stamp.
#[derive(Clone)]
pub struct ReadOrigin { user:i64, exchange:Option<i64>, versions:Vec<CredentialVersion>, balance_producers:Vec<model::Produced<()>>, balance_inputs:bool }
pub struct ReadValue<T> { pub value:T, pub origin:ReadOrigin }
impl ReadOrigin {
    pub fn producer_stamps(&self)->Value {Value::Array(self.versions.iter().map(CredentialVersion::cache_stamp).collect())}
    pub fn ledger_is_current(&self,c:&Connection)->Result<bool,EngineError>{
        if self.versions.is_empty(){return Ok(true)}
        let mut stmt=c.prepare("SELECT DISTINCT exchange_id FROM api_keys WHERE user_id=?1 AND key_type!=1 AND (?2 IS NULL OR exchange_id=?2) ORDER BY exchange_id")?;
        let scopes=stmt.query_map(rusqlite::params![self.user,self.exchange],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        for exchange in scopes {
            // The scheduler imports one healthy reading slot per venue, trading before read-only.
            // Keep the complete captured set for rotation fencing, but do not demand a watermark
            // from an unused invalid slot which did not produce this venue's import.
            let id:Option<i64>=c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND status=1 AND key_type!=1 ORDER BY CASE WHEN key_type=0 THEN 0 ELSE 1 END,id LIMIT 1",[self.user,exchange],|r|r.get(0)).optional()?;
            let Some(version)=self.versions.iter().find(|version|Some(version.id)==id) else {return Ok(false)};
            if !ledger_produced_by(c,version.id,version)? || state::read(c,"ledger_sync",Some(&version.id.to_string())).map_err(EngineError::Data)?.incomplete_since.is_some() {return Ok(false)}
        }
        Ok(true)
    }
    pub fn derive<T>(self,value:T)->ReadValue<T>{ReadValue{value,origin:self}}
    /// Snapshot inputs retain their stored balance producer even when the current key has rotated.
    pub fn with_balance_producers(&self,c:&Connection)->Result<Self,EngineError>{
        let mut stmt=c.prepare("SELECT DISTINCT exchange_id FROM account_balances WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2) ORDER BY exchange_id")?;
        let scopes=stmt.query_map(rusqlite::params![self.user,self.exchange],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        let mut balance_producers=vec![];
        for exchange in scopes{
            let stamp=app_config::get_plain(c,&key(self.user,exchange)).map_err(EngineError::Data)?.and_then(|s|serde_json::from_str::<Value>(&s).ok());
            let complete=stamp.as_ref().is_some_and(|stamp|stamp["complete"]==Value::Bool(true));
            balance_producers.push(model::Produced::new((),stamp.as_ref().and_then(CredentialVersion::from_stamp)).with_completion(complete));
        }
        Ok(Self{user:self.user,exchange:self.exchange,versions:self.versions.clone(),balance_producers,balance_inputs:true})
    }
}
pub fn capture_read(c:&Connection,user:i64,exchange:Option<i64>)->Result<ReadOrigin,EngineError>{
    let mut stmt=c.prepare("SELECT id FROM api_keys WHERE user_id=?1 AND key_type!=1 AND (?2 IS NULL OR exchange_id=?2) ORDER BY id")?;
    let ids=stmt.query_map(rusqlite::params![user,exchange],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    let versions=ids.into_iter().map(|id|model::credential_version_by_id(c,id)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten().collect();
    Ok(ReadOrigin{user,exchange,versions,balance_producers:vec![],balance_inputs:false})
}
pub fn read_is_current(c:&Connection,origin:&ReadOrigin)->Result<bool,EngineError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=read_is_current(&tx,origin)?;tx.commit()?;return Ok(out)}
    let current=capture_read(c,origin.user,origin.exchange)?;
    Ok(current.versions.len()==origin.versions.len()&&current.versions.iter().zip(&origin.versions).all(|(a,b)|model::current_for(Some(a),Some(b)).is_fresh()))
}

/// The incremental ledger watermark belongs only to its completed credential version.
pub fn ledger_produced_by(c:&Connection,id:i64,version:&CredentialVersion)->Result<bool,EngineError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=ledger_produced_by(&tx,id,version)?;tx.commit()?;return Ok(out)}
    let stamp=app_config::get_plain(c,&state::key("ledger_origin",Some(&id.to_string()))).map_err(EngineError::Data)?.and_then(|s|serde_json::from_str::<Value>(&s).ok());
    Ok(stamp.is_some_and(|stamp|model::stamp_current_for(&stamp,&version.cache_stamp()).is_fresh()))
}
/// Called inside the Q-checked completion transaction, alongside the public watermark.
pub fn record_ledger(c:&model::FencedTransaction<'_>,id:i64,version:&CredentialVersion,now:DateTime<Utc>)->Result<(),EngineError>{

    app_config::set_plain(c,&state::key("ledger_origin",Some(&id.to_string())),&version.cache_stamp().to_string(),now).map_err(EngineError::Data)
}

pub fn fence_read<'a>(tx:&'a rusqlite::Transaction<'_>,origin:&ReadOrigin)->Result<model::FencedTransaction<'a>,EngineError>{
    if origin.balance_inputs {
        let current_balances=origin.with_balance_producers(tx)?;
        if current_balances.balance_producers.len()!=origin.balance_producers.len(){return Err(EngineError::CredentialsChanged)}
        for (producer,current) in origin.balance_producers.iter().zip(&current_balances.balance_producers){
            let fresh=producer.current_for(current.origin()).is_fresh() && model::produced_is_current(tx,current)?;
            if !fresh{return Err(EngineError::CredentialsChanged)}
        }
    }
    let fenced=model::fence_versions(tx,&origin.versions)?;
    if !read_is_current(tx,origin)?{crate::engine::log(model::CREDENTIALS_CHANGED);return Err(EngineError::CredentialsChanged)}
    Ok(fenced)
}

/// Persisted ledger freshness consumes the existing producer stamp; the central comparison reads current ciphertext.
pub fn ledger_current_for(c:&Connection,id:i64)->Result<bool,EngineError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let fresh=ledger_current_for(&tx,id)?;tx.commit()?;return Ok(fresh)}
    let stamp=app_config::get_plain(c,&state::key("ledger_origin",Some(&id.to_string()))).map_err(EngineError::Data)?;
    let producer=stamp.and_then(|stamp|serde_json::from_str::<Value>(&stamp).ok()).and_then(|stamp|CredentialVersion::from_stamp(&stamp));
    match producer {Some(producer) if producer.id==id=>model::credential_is_current(c,&producer),_=>Ok(false)}
}
