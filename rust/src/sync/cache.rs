//! R: cache provenance lives in the existing Rust job-state store. History is retained.
//! #710 slice 5 (PLAN-710-5 §5.4): balance provenance is per account; a scope's freshness is its accounts'.
use crate::{app_config, engine::{model::{self, CredentialVersion}, EngineError}, identity::AccountRow, jobs::state, tracker::Scope};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// `<user>:<exchange>` for a number 1 (2.x's key, kept), `<user>:<exchange>:<account>` for an account 2+.
fn key(a: &AccountRow) -> String {
    let scope = if a.number == 1 { format!("{}:{}", a.user_id, a.exchange_id) } else { format!("{}:{}:{}", a.user_id, a.exchange_id, a.id) };
    state::key("balance_origin", Some(&scope))
}
/// The balance origin row of a user's venue's number 1 (`rust_job.balance_origin:<user>:<exchange>`): its `updated_at`
/// moves once per balance sync, which is what the tracker's history-gap line checks its balances against.
pub fn balance_origin_key(user: i64, exchange: i64) -> String { state::key("balance_origin", Some(&format!("{user}:{exchange}"))) }
/// An account's balance origin row (`balance_origin_key` for a number 1).
pub fn balance_origin_key_of(account: &AccountRow) -> String { key(account) }
/// Call in the checked balance write transaction. Incomplete batches cannot publish current figures.
pub fn record_account(c: &model::FencedTransaction<'_>, account: &AccountRow, version: &CredentialVersion, complete: bool, now: DateTime<Utc>) -> Result<(), EngineError> {
    let mut stamp = version.cache_stamp();
    stamp["complete"] = Value::Bool(complete);
    app_config::set_plain(c, &key(account), &stamp.to_string(), now).map_err(EngineError::Data)
}
/// `record_account` for (user, exchange)'s number 1, its row made where it has none.
pub fn record(c: &model::FencedTransaction<'_>, user: i64, exchange: i64, version: &CredentialVersion, complete: bool, now: DateTime<Utc>) -> Result<(), EngineError> {
    let first = crate::identity::Account::first(c, user, exchange)?.ensure(c, now)?;
    let account = AccountRow::of(c, &first)?.ok_or_else(|| EngineError::Data("no account".into()))?;
    record_account(c, &account, version, complete, now)
}
/// An old balance price may be reused only by the credential version that produced it, on its own account.
pub fn produced_by(c: &Connection, account: &AccountRow, version: &CredentialVersion) -> Result<bool, EngineError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=produced_by(&tx,account,version)?;tx.commit()?;return Ok(out)}
    let stamp = app_config::get_plain(c, &key(account)).map_err(EngineError::Data)?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(stamp.is_some_and(|stamp| stamp["complete"] == Value::Bool(true) && model::stamp_current_for(&stamp,&version.cache_stamp()).is_fresh()))
}
/// The key an account's balances are synced with: trading before read-only, never withdrawal.
pub fn reading_key(c: &Connection, account: &AccountRow) -> Result<Option<i64>, EngineError> {
    // The scheduler's pick (sync::reading_keys_of): a trading key unless its last sync lacked a permission, then read-only.
    let mut s = c.prepare(&format!("SELECT k.id, k.key_type, e.type, k.last_sync_error FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
                           WHERE k.status=1 AND k.key_type!=1 AND {} ORDER BY k.id", account.keys("k")))?;
    let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.iter().find(|r| r.1 == 0 && !crate::tracker::import::run::missing_permission(&r.2, r.3.as_deref()))
        .or_else(|| rows.iter().find(|r| r.1 == 2)).or_else(|| rows.first()).map(|r| r.0))
}
/// The scope's accounts with balances (`balances`) or a non-withdrawal key (`keys`), by id; a key's number 1 with no row
/// yet as its stand-in (`AccountRow::unplaced`), after them.
fn accounts(c: &Connection, user: i64, scope: &Scope, balances: bool, keys: bool) -> Result<Vec<AccountRow>, EngineError> {
    let mut parts = vec![];
    let owned = format!("SELECT {} FROM api_keys k WHERE k.user_id = ?1 AND k.key_type != 1 AND {}", crate::identity::key_account_sql("k"), scope.keys("k"));
    if balances { parts.push(format!("SELECT b.account_id FROM exchange_account_balances b WHERE b.user_id = ?1 AND {}", scope.store("b"))); }
    if keys { parts.push(owned); }
    let mut out = AccountRow::listed(c, user, &parts.join(" UNION "))?;
    if keys {
        let mut s = c.prepare(&format!("SELECT DISTINCT k.exchange_id FROM api_keys k WHERE k.user_id = ?1 AND k.key_type != 1 AND {} AND {} IS NULL \
                                        AND NOT EXISTS (SELECT 1 FROM exchange_account_keys m WHERE m.api_key_id = k.id) ORDER BY k.exchange_id",
                                       scope.keys("k"), crate::identity::key_account_sql("k")))?;
        let unplaced = s.query_map([user], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        out.extend(unplaced.into_iter().map(|e| AccountRow::unplaced(user, e)));
    }
    Ok(out)
}
/// Missing, malformed, partial or differently credentialed cache origins are sync-pending.
/// Select the same reading slot as the sync scheduler: trading before read-only, never withdrawal.
pub fn stale(c: &Connection, user: i64, scope: &Scope) -> Result<bool, EngineError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=stale(&tx,user,scope)?;tx.commit()?;return Ok(out)}
    for account in accounts(c, user, scope, true, true)? {
        let id = reading_key(c, &account)?;
        // An account whose key cannot read yet (an IBKR key pending activation) and that holds no balances has nothing stale.
        if id.is_none() && !c.query_row("SELECT EXISTS(SELECT 1 FROM exchange_account_balances WHERE account_id=?1)", [account.id], |r| r.get::<_, bool>(0))? { continue; }
        let stamp = app_config::get_plain(c, &key(&account)).map_err(EngineError::Data)?
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
pub struct ReadOrigin { user:i64, scope:Scope, versions:Vec<CredentialVersion>, balance_producers:Vec<model::Produced<()>>, balance_inputs:bool }
pub struct ReadValue<T> { pub value:T, pub origin:ReadOrigin }
impl ReadOrigin {
    pub fn producer_stamps(&self)->Value {Value::Array(self.versions.iter().map(CredentialVersion::cache_stamp).collect())}
    /// An account whose key was deleted keeps its rows as final history (ApiKey `dependent: :nullify`): current, as
    /// 2.x shows it. An account with a key but no healthy reading slot is current only while it has no rows.
    pub fn ledger_is_current(&self,c:&Connection)->Result<bool,EngineError>{
        for account in accounts(c,self.user,&self.scope,false,true)? {
            // The scheduler imports one healthy reading slot per account, trading before read-only.
            // Keep the complete captured set for rotation fencing, but do not demand a watermark
            // from an unused invalid slot which did not produce this account's import.
            let id:Option<i64>=c.query_row(&format!("SELECT k.id FROM api_keys k WHERE k.status=1 AND k.key_type!=1 AND {} ORDER BY CASE WHEN k.key_type=0 THEN 0 ELSE 1 END,k.id LIMIT 1",account.keys("k")),[],|r|r.get(0)).optional()?;
            let Some(id)=id else {
                let rows:bool=c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM account_transactions a WHERE a.user_id=?1 AND {})",Scope::Account(account).rows("a")),[self.user],|r|r.get(0))?;
                if rows {return Ok(false)} else {continue}
            };
            let Some(version)=self.versions.iter().find(|version|version.id==id) else {return Ok(false)};
            if !ledger_produced_by(c,version.id,version)? || state::read(c,"ledger_sync",Some(&version.id.to_string())).map_err(EngineError::Data)?.incomplete_since.is_some() {return Ok(false)}
        }
        Ok(true)
    }
    /// Whether `key`, as this read captured it, produced its account's current ledger import (no run left incomplete) and
    /// its account's complete balances: what the tracker's history-gap step asks of an account's reading key.
    pub fn key_read_complete(&self,c:&Connection,key:i64,account:&AccountRow)->Result<bool,EngineError>{
        let Some(version)=self.versions.iter().find(|version|version.id==key) else {return Ok(false)};
        if !ledger_produced_by(c,key,version)? || state::read(c,"ledger_sync",Some(&key.to_string())).map_err(EngineError::Data)?.incomplete_since.is_some() {return Ok(false)}
        produced_by(c,account,version)
    }
    pub fn derive<T>(self,value:T)->ReadValue<T>{ReadValue{value,origin:self}}
    /// Snapshot inputs retain their stored balance producer even when the current key has rotated.
    pub fn with_balance_producers(&self,c:&Connection)->Result<Self,EngineError>{
        let mut balance_producers=vec![];
        for account in accounts(c,self.user,&self.scope,true,false)? {
            let stamp=app_config::get_plain(c,&key(&account)).map_err(EngineError::Data)?.and_then(|s|serde_json::from_str::<Value>(&s).ok());
            let complete=stamp.as_ref().is_some_and(|stamp|stamp["complete"]==Value::Bool(true));
            balance_producers.push(model::Produced::new((),stamp.as_ref().and_then(CredentialVersion::from_stamp)).with_completion(complete));
        }
        Ok(Self{user:self.user,scope:self.scope,versions:self.versions.clone(),balance_producers,balance_inputs:true})
    }
}
pub fn capture_read(c:&Connection,user:i64,scope:&Scope)->Result<ReadOrigin,EngineError>{
    let mut stmt=c.prepare(&format!("SELECT k.id FROM api_keys k WHERE k.user_id=?1 AND k.key_type!=1 AND {} ORDER BY k.id",scope.keys("k")))?;
    let ids=stmt.query_map([user],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    let versions=ids.into_iter().map(|id|model::credential_version_by_id(c,id)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten().collect();
    Ok(ReadOrigin{user,scope:*scope,versions,balance_producers:vec![],balance_inputs:false})
}
pub fn read_is_current(c:&Connection,origin:&ReadOrigin)->Result<bool,EngineError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=read_is_current(&tx,origin)?;tx.commit()?;return Ok(out)}
    let current=capture_read(c,origin.user,&origin.scope)?;
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

/// Whether the balances a read captured are still the ones current keys produced (always, for a read of no balances).
pub fn balances_current(tx:&Connection,origin:&ReadOrigin)->Result<bool,EngineError>{
    if origin.balance_inputs {
        let current_balances=origin.with_balance_producers(tx)?;
        if current_balances.balance_producers.len()!=origin.balance_producers.len(){return Ok(false)}
        for (producer,current) in origin.balance_producers.iter().zip(&current_balances.balance_producers){
            let fresh=producer.current_for(current.origin()).is_fresh() && model::produced_is_current(tx,current)?;
            if !fresh{return Ok(false)}
        }
    }
    Ok(true)
}

pub fn fence_read<'a>(tx:&'a rusqlite::Transaction<'_>,origin:&ReadOrigin)->Result<model::FencedTransaction<'a>,EngineError>{
    if !balances_current(tx,origin)? {return Err(EngineError::CredentialsChanged)}
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
