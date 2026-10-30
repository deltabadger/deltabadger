//! A12 history repair, shared by the versioned background job and operator CLI.
//! No normal watermark changes or transfer matching.
//! Preview uses SQLite's consistent backup and the production writer on a private database.
use super::{binance_writer::{self as writer, Input}, Key, SyncError};
use crate::{app_config, crypto::Cipher, engine::model::{self, CredentialVersion}, lease};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account { pub user_id:i64, pub exchange_id:i64, pub key_id:Option<i64> }
/// Include historical scopes even when their original key was deleted or replaced.
pub fn inventory(c:&Connection)->Result<Vec<Account>,SyncError> {
    let mut s=c.prepare("SELECT user_id,exchange_id FROM account_transactions UNION SELECT user_id,exchange_id FROM api_keys")?;
    let scopes=s.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    let keys=super::reading_keys_of(c,&[crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US])?;
    let keys=keys.into_iter().map(|id|super::load_key_of(c,id,&[crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US])).collect::<Result<Vec<_>,_>>()?;
    let mut out=vec![];
    for (user_id,exchange_id) in scopes {
        let venue:String=c.query_row("SELECT coalesce(type,'') FROM exchanges WHERE id=?1",[exchange_id],|r|r.get(0))?;
        if [crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US].contains(&venue.as_str()) {
            out.push(Account{user_id,exchange_id,key_id:keys.iter().find(|k|k.user_id==user_id && k.exchange_id==exchange_id).map(|k|k.id)});
        }
    } Ok(out)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub source:String, pub kind:Option<String>, pub start:i64, pub end:i64,
    pub pages:Vec<String>, pub unresolved:Option<String>,
}
#[derive(Clone)]
pub struct Evidence { pub inputs:Vec<Input>, pub coverage:Vec<Window> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub bucket:String, pub before:Option<String>, pub after:String, pub inserted:Vec<Value>,
    pub coverage:Vec<Window>, pub original_checksum:String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tally { pub fetched:usize, pub missing:usize, pub present:usize, pub suppressed:usize, pub unresolved:usize,
    /// Exact fetched decimal sums by canonical source, coin and entry type.
    pub amounts:BTreeMap<String,BTreeMap<String,String>>,
    pub counts:BTreeMap<String,BTreeMap<String,usize>>,
    #[serde(skip)] seen:std::collections::HashSet<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version:u8, pub user_id:i64, pub exchange_id:i64, pub key_id:i64, pub cutoff:String,
    pub credential:Value, pub evidence:String, pub original:BTreeMap<i64,String>, pub retained:BTreeMap<String,String>,
    pub coverage:Vec<Window>, pub unresolved:Vec<String>, pub tally:Tally, pub batches:Vec<Batch>,
}
impl Manifest { pub fn digest(&self)->String { hash(&serde_json::to_string(self).expect("manifest")) } }
fn hash(text:&str)->String { hex::encode(Sha256::digest(text.as_bytes())) }
fn refuse(reason:&str)->SyncError { SyncError(format!("Binance repair refused: {reason}")) }
fn bucket_name(key:&Key,bucket:&str)->String { format!("rust_binance_a12_v1:bucket:{}:{}:{}",key.user_id,key.exchange_id,hash(bucket)) }
fn journal(digest:&str,n:usize)->String { format!("rust_binance_a12_v1:repair:{digest}:{n}") }
fn groups(e:&Evidence)->Result<BTreeMap<String,Vec<Input>>,SyncError> {
    let mut out=BTreeMap::<String,Vec<Input>>::new();
    for i in &e.inputs {out.entry(writer::bucket_id(&i.entry)?).or_default().push(i.clone());} Ok(out)
}
fn evidence_digest(e:&Evidence)->String {
    let mut rows:Vec<_>=e.inputs.iter().map(|i|format!("{:?}:{}",i.entry,serde_json::to_string(&i.event).expect("event"))).collect();
    rows.sort();hash(&json!([e.coverage,rows]).to_string())
}
fn metadata(c:&Connection,key:&Key)->Result<BTreeMap<String,String>,SyncError> {
    let prefix=format!("rust_binance_a12_v1:bucket:{}:{}:",key.user_id,key.exchange_id);
    let mut s=c.prepare("SELECT key,value FROM app_configs WHERE key LIKE ?1 ORDER BY key")?;
    let rows=s.query_map([format!("{prefix}%")],|r|Ok((r.get::<_,String>(0)?,hash(&r.get::<_,String>(1)?))))?.collect::<Result<_,_>>()?;Ok(rows)
}
fn rows(c:&Connection)->Result<BTreeMap<i64,String>,SyncError> {
    let mut s=c.prepare("SELECT id FROM account_transactions ORDER BY id")?;
    let ids=s.query_map([],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    ids.into_iter().map(|id|Ok((id,hash(&writer::image(c,id)?.to_string())))).collect()
}
pub fn require_ownership(c:&Connection,cipher:&Cipher)->Result<(),SyncError> {
    if !lease::read(c,cipher).map_err(|_|refuse("unreadable ownership"))?.is_some_and(|v|v["engine"]=="rust") {return Err(refuse("Rust ownership required"))} Ok(())
}
fn require_scope(c:&Connection,key:&Key)->Result<(),SyncError> {
    let current=super::load_key_of(c,key.id,&VENUES)?;
    if (current.user_id,current.exchange_id)!=(key.user_id,key.exchange_id) {return Err(refuse("credential scope changed"))} Ok(())
}
fn integrity(c:&Connection)->Result<(),SyncError> {
    if c.query_row("PRAGMA quick_check",[],|r|r.get::<_,String>(0))?!="ok" || c.prepare("PRAGMA foreign_key_check")?.exists([])? {return Err(refuse("database integrity check failed"))} Ok(())
}
fn covers(w:&Window,i:&Input)->bool {
    use super::binance_ledger as b;
    let Some(k)=i.event.key() else{return false};let tuple:Value=serde_json::from_str(k.tuple()).expect("canonical tuple");
    let path=match tuple[1].as_str().unwrap_or("") {"deposit"=>b::DEPOSITS_PATH,"withdrawal"=>b::WITHDRAWALS_PATH,"dust-in"|"dust-out"=>b::DUST_PATH,
        prefix=>b::EARN.iter().find(|e|e.2==prefix).map_or("",|e|e.0)};
    w.source==path && w.kind.as_deref().is_none_or(|kind|i.entry.raw.value["type"].as_str()==Some(kind)) && i.entry.transacted_at.is_some_and(|t|{let at=writer::identity_ms(t);at>=w.start && at<=w.end})
}
fn unresolved_windows(m:&mut Manifest,input:&[Input],reason:&str) {
    for w in &mut m.coverage {if w.unresolved.is_none() && input.iter().any(|i|covers(w,i)) {w.unresolved=Some(reason.into());}}
}
/// No source writes, including a marker, checkpoint, timestamp or cache change.
pub fn dry_run(c:&Connection,cipher:&Cipher,key:&Key,version:&CredentialVersion,cutoff:DateTime<Utc>,e:&Evidence)->Result<Manifest,SyncError> {
    let mut copy=Connection::open_in_memory()?;
    rusqlite::backup::Backup::new(c,&mut copy)?.run_to_completion(256,std::time::Duration::from_millis(5),None)?;
    require_ownership(&copy,cipher)?;integrity(&copy)?;
    if !model::credential_is_current(&copy,version)? {return Err(refuse("credentials changed"))}
    require_scope(&copy,key)?;
    let mut m=Manifest{version:1,user_id:key.user_id,exchange_id:key.exchange_id,key_id:key.id,cutoff:cutoff.to_rfc3339(),credential:version.cache_stamp(),
        evidence:evidence_digest(e),original:rows(&copy)?,retained:metadata(&copy,key)?,coverage:e.coverage.clone(),unresolved:vec![],tally:Tally::default(),batches:vec![]};
    let mut old=m.original.clone();
    let original_checksum=hash(&json!(m.original).to_string());
    let grouped=groups(e)?;let mut locations=BTreeMap::<String,std::collections::HashSet<String>>::new();
    for (bucket,input) in &grouped {for i in input {if let Some(k)=i.event.key(){locations.entry(k.tuple().into()).or_default().insert(bucket.clone());}}}
    for (bucket,input) in grouped {
        let name=bucket_name(key,&bucket);let before=app_config::get_plain(&copy,&name).map_err(SyncError)?;
        let incomplete=e.coverage.iter().any(|w|w.unresolved.is_some() && input.iter().any(|i|covers(w,i)));
        if incomplete || input.iter().any(|i|i.event.key().is_some_and(|k|locations[k.tuple()].len()>1)) {
            m.unresolved.push(format!("{}: {}",hash(&bucket),if incomplete {"source coverage is incomplete"}else{"canonical identity appears in multiple buckets"}));tally(&mut m.tally,&input,&old,None)?;unresolved_windows(&mut m,&input,"unresolved collision bucket");continue;
        }
        let tx=model::immediate(&copy)?;
        let fenced=model::fence_versions(&tx,std::slice::from_ref(version))?;
        let _guard=AppendOnly::new(&copy);
        let out=match writer::store(&fenced,cipher,key,&input,cutoff) {
            Ok(out)=>out,
            Err(error) if error.0.starts_with("Binance A12 unresolved:")=> {
                m.unresolved.push(format!("{}: {}",hash(&bucket),error.0));tally(&mut m.tally,&input,&old,None)?;unresolved_windows(&mut m,&input,"unresolved collision bucket");continue;
            }, Err(error)=>return Err(error),
        };
        let after=app_config::get_plain(&fenced,&name).map_err(SyncError)?;
        tally(&mut m.tally,&input,&old,after.as_deref())?;
        if out.skipped>0 {unresolved_windows(&mut m,&input,"intentionally suppressed; unrecovered");}
        if after!=before {
            let inserted=new_rows(&fenced,old.last_key_value().map_or(0,|(id,_)|*id))?;
            for row in &inserted {old.insert(row["id"].as_i64().expect("row id"),hash(&row.to_string()));}
            m.batches.push(Batch{bucket,before,after:after.ok_or_else(||refuse("missing writer journal"))?,inserted,
                coverage:m.coverage.iter().filter(|w|input.iter().any(|i|covers(w,i))).cloned().collect(),
                original_checksum:original_checksum.clone()});
        }
        tx.commit()?;
    }
    if m.tally.fetched!=m.tally.missing+m.tally.present+m.tally.suppressed+m.tally.unresolved {return Err(refuse("coverage equality failed"))}
    for sums in m.tally.amounts.values() {
        let value=|category:&str|crate::ruby::BigDec::parse(sums.get(category).map_or("0",String::as_str)).map_err(|_|refuse("invalid sum"));
        let mut parts=crate::ruby::BigDec::zero();
        for category in ["missing","present","suppressed","unresolved"] {parts=&parts+&value(category)?;}
        if parts!=value("fetched")? {return Err(refuse("decimal coverage equality failed"))}
    }
    Ok(m)
}
// Count canonical identities, not repeated page observations, and partition exact source amounts.
fn tally(t:&mut Tally,input:&[Input],old:&BTreeMap<i64,String>,after:Option<&str>)->Result<(),SyncError> {
    let metadata:Value=after.map(serde_json::from_str).transpose().map_err(|_|refuse("unreadable writer decision"))?.unwrap_or(Value::Null);
    for i in input {
        let event=serde_json::to_value(&i.event).expect("event");
        if !t.seen.insert(event["key"].to_string()) {continue}
        let binding=metadata["bindings"].as_array().and_then(|b|b.iter().find(|b|b["event"]["key"]==event["key"]));
        let category=match binding {
            None=>{t.unresolved+=1;"unresolved"},Some(b) if b["suppressed"]==true=>{t.suppressed+=1;"suppressed"},
            Some(b) if b["row_id"].as_i64().is_some_and(|id|old.contains_key(&id))=>{t.present+=1;"present"},Some(_)=>{t.missing+=1;"missing"},
        };t.fetched+=1;
        let scope=json!([event["source"],i.entry.base_currency,i.entry.entry_type]).to_string();
        let counts=t.counts.entry(scope.clone()).or_default();for category in ["fetched",category] {*counts.entry(category.into()).or_default()+=1;}
        let sums=t.amounts.entry(scope).or_default();
        for category in ["fetched",category] {let old=sums.entry(category.into()).or_insert_with(||"0.0".into());
            *old=(&crate::ruby::BigDec::parse(old).map_err(|_|refuse("invalid sum"))?+&i.entry.base_amount).to_s_f();}
    } Ok(())
}
/// Validate even a zero-batch approval. Nothing is written on a no-op or stale approval.
pub fn validate(c:&Connection,cipher:&Cipher,key:&Key,m:&Manifest,approved:&str,e:&Evidence)->Result<(),SyncError> {
    if m.version!=1 || m.digest()!=approved || evidence_digest(e)!=m.evidence || (key.id,key.user_id,key.exchange_id)!=(m.key_id,m.user_id,m.exchange_id) {return Err(refuse("approval, scope or fetched evidence changed"))}
    let version=CredentialVersion::from_stamp(&m.credential).ok_or_else(||refuse("unreadable credential fence"))?;
    let tx=model::immediate(c)?;let fenced=model::fence_versions(&tx,&[version])?;require_ownership(&fenced,cipher)?;
    require_scope(&fenced,key)?;
    if m.batches.is_empty() && (rows(&fenced)?!=m.original || metadata(&fenced,key)?!=m.retained) {return Err(refuse("ledger changed"))} Ok(())
}
/// Only the new rows are read after a bucket. The preview maintains its original map incrementally.
fn new_rows(c:&Connection,after:i64)->Result<Vec<Value>,SyncError> {
    let mut s=c.prepare("SELECT id FROM account_transactions WHERE id>?1 ORDER BY id")?;
    let ids=s.query_map([after],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    ids.into_iter().map(|id|writer::image(c,id).map_err(SyncError::from)).collect()
}

/// SQLite enforces preservation during each writer unit, including statements issued by triggers.
/// The writer only appends ledger rows. It never edits existing rows, links or API-key watermarks.
/// Refusing triggered inserts also prevents REPLACE's implicit deletes from bypassing this guard.
struct AppendOnly<'a>(&'a Connection);
impl<'a> AppendOnly<'a> {
    fn new(c:&'a Connection)->Self {
        use rusqlite::hooks::{AuthAction,AuthContext,Authorization};
        c.authorizer(Some(|ctx:AuthContext<'_>| match ctx.action {
            AuthAction::Update{table_name:"account_transactions"|"api_keys"|"transactions",..}
            | AuthAction::Delete{table_name:"account_transactions"|"api_keys"|"transactions"} => Authorization::Deny,
            AuthAction::Insert{..}|AuthAction::Update{..}|AuthAction::Delete{..} if ctx.accessor.is_some()=>Authorization::Deny,
            _=>Authorization::Allow,
        }));Self(c)
    }
}
impl Drop for AppendOnly<'_> {
    fn drop(&mut self){self.0.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>)->rusqlite::hooks::Authorization>);}
}

/// An approval's verified state. Whole-ledger checks happen once, on any intervening database change,
/// and at final verification. SQLite's connection change counter and data_version cover local and
/// other-connection writes; the address binds the cache to the connection that was actually checked.
/// Never persisted: after a crash the first batch validates the durable checkpoints again.
pub struct ApplySession {
    approved:String, manifest:std::sync::Arc<Manifest>, grouped:BTreeMap<String,Vec<Input>>,
    stamp:Option<(usize,u64,i64)>, expected:BTreeMap<i64,String>, retained:BTreeMap<String,String>,
    committed:std::collections::BTreeSet<usize>, checks:usize,
}
impl ApplySession {
    pub fn new(m:&Manifest,approved:&str,e:&Evidence)->Result<Self,SyncError> {
        if m.version!=1 || m.digest()!=approved || evidence_digest(e)!=m.evidence {return Err(refuse("approval or fetched evidence changed"))}
        Ok(Self{approved:approved.into(),manifest:std::sync::Arc::new(m.clone()),grouped:groups(e)?,stamp:None,
            expected:BTreeMap::new(),retained:BTreeMap::new(),committed:Default::default(),checks:0})
    }
    #[doc(hidden)]
    pub fn full_checks(&self)->usize {self.checks}
    fn stamp(c:&Connection)->Result<(usize,u64,i64),SyncError> {
        Ok((c as *const Connection as usize,c.total_changes(),c.query_row("PRAGMA data_version",[],|r|r.get(0))?))
    }
    fn check(&mut self,c:&Connection,key:&Key,m:&Manifest)->Result<(),SyncError> {
        self.checks+=1;
        let mut expected=m.original.clone();let mut retained=m.retained.clone();let mut committed=std::collections::BTreeSet::new();
        for (i,b) in m.batches.iter().enumerate() {
            if let Some(saved)=app_config::get_plain(c,&journal(&self.approved,i)).map_err(SyncError)? {
                if saved!=serde_json::to_string(b).expect("batch") || app_config::get_plain(c,&bucket_name(key,&b.bucket)).map_err(SyncError)?.as_deref()!=Some(&b.after) {return Err(refuse("committed batch changed"))}
                committed.insert(i);retained.insert(bucket_name(key,&b.bucket),hash(&b.after));
                for row in &b.inserted {expected.insert(row["id"].as_i64().ok_or_else(||refuse("invalid after-image"))?,hash(&row.to_string()));}
            }
        }
        if rows(c)?!=expected || metadata(c,key)?!=retained {return Err(refuse("ledger rows or links changed; repeat dry run"))}
        integrity(c)?;
        self.expected=expected;self.retained=retained;self.committed=committed;Ok(())
    }
    fn scope(&self,key:&Key,m:&Manifest)->Result<(),SyncError> {
        if (key.id,key.user_id,key.exchange_id)!=(m.key_id,m.user_id,m.exchange_id) {return Err(refuse("approval scope changed"))} Ok(())
    }
    /// One bounded, atomic collision bucket. A stale database always requires fresh validation.
    pub fn apply_batch(&mut self,c:&Connection,cipher:&Cipher,key:&Key,n:usize)->Result<usize,SyncError> {
        // The session owns its approved manifest and inputs; neither can change between units.
        let manifest=self.manifest.clone();let m=manifest.as_ref();
        self.scope(key,m)?;
        let cutoff=m.cutoff.parse::<DateTime<Utc>>().map_err(|_|refuse("invalid cutoff"))?;
        let batch=m.batches.get(n).ok_or_else(||refuse("unknown batch"))?;
        let version=CredentialVersion::from_stamp(&m.credential).ok_or_else(||refuse("unreadable credential fence"))?;
        let tx=model::immediate(c)?;let fenced=model::fence_versions(&tx,&[version])?;require_ownership(&fenced,cipher)?;
        require_scope(&fenced,key)?;
        let stamp=Self::stamp(c)?;
        if self.stamp!=Some(stamp) {self.check(&fenced,key,m)?;}
        if (0..n).any(|i|!self.committed.contains(&i)) {return Err(refuse("resume batches in manifest order"))}
        if self.committed.contains(&n) {self.stamp=Some(stamp);return Ok(0)}
        // A failed unit must not leave a reusable validation cache.
        self.stamp=None;
        let name=bucket_name(key,&batch.bucket);
        if app_config::get_plain(&fenced,&name).map_err(SyncError)?!=batch.before {return Err(refuse("bucket or suppression changed; repeat dry run"))}
        let input=self.grouped.get(&batch.bucket).ok_or_else(||refuse("missing bucket evidence"))?;
        let _guard=AppendOnly::new(c);
        let out=writer::store(&fenced,cipher,key,input,cutoff)?;
        if app_config::get_plain(&fenced,&name).map_err(SyncError)?.as_deref()!=Some(&batch.after) {return Err(refuse("writer decision changed; repeat dry run"))}
        let inserted=new_rows(&fenced,self.expected.last_key_value().map_or(0,|(id,_)|*id))?;
        if inserted!=batch.inserted || out.imported!=batch.inserted.len() {return Err(refuse("inserted after-image or count changed"))}
        writer::mark(&fenced,cutoff)?;
        app_config::set_plain(&fenced,&journal(&self.approved,n),&serde_json::to_string(batch).expect("batch"),cutoff).map_err(SyncError)?;
        tx.commit()?;
        for row in &inserted {self.expected.insert(row["id"].as_i64().expect("row id"),hash(&row.to_string()));}
        self.retained.insert(name,hash(&batch.after));self.committed.insert(n);
        // Keep the data_version read under the transaction, never one read after another writer could commit.
        self.stamp=Some((stamp.0,c.total_changes(),stamp.2));Ok(out.imported)
    }
    pub fn verify(&mut self,c:&Connection,cipher:&Cipher,key:&Key)->Result<(),SyncError> {
        self.finish(c,cipher,key,|_|Ok(()))
    }
    fn finish(&mut self,c:&Connection,cipher:&Cipher,key:&Key,done:impl FnOnce(&model::FencedTransaction<'_>)->Result<(),SyncError>)->Result<(),SyncError> {
        let manifest=self.manifest.clone();let m=manifest.as_ref();
        self.scope(key,m)?;
        let tx=model::immediate(c)?;
        let version=CredentialVersion::from_stamp(&m.credential).ok_or_else(||refuse("unreadable credential fence"))?;
        let fenced=model::fence_versions(&tx,&[version])?;require_ownership(&fenced,cipher)?;
        require_scope(&fenced,key)?;
        self.check(&fenced,key,m)?;
        if self.committed.len()!=m.batches.len() {return Err(refuse("uncommitted batches"))}
        let _guard=AppendOnly::new(c);
        done(&fenced)?;tx.commit()?;Ok(())
    }
}

/// Standalone CLI-compatible unit; callers applying several batches reuse ApplySession.
#[allow(clippy::too_many_arguments)]
pub fn apply_batch(c:&Connection,cipher:&Cipher,key:&Key,m:&Manifest,approved:&str,e:&Evidence,n:usize)->Result<usize,SyncError> {
    ApplySession::new(m,approved,e)?.apply_batch(c,cipher,key,n)
}

/// Re-fetch every reachable affected source, independent of normal sync watermarks.
/// No margin/futures or negative-control endpoint is requested. Failures are fixed diagnostics,
/// never venue response bodies (which can echo credentials).
pub async fn fetch<T:crate::venue::http::Transport>(venue:&crate::venue::binance::BinanceVenue<T>,clock:&dyn crate::engine::Clock,cutoff:DateTime<Utc>,us:bool,pacing:super::binance_ledger::Pacing)->Evidence {
    use super::binance_ledger as b;
    let mut e=Evidence{inputs:vec![],coverage:vec![Window{source:"margin/futures".into(),kind:None,start:b::HISTORY_FROM_MS,end:cutoff.timestamp_millis(),pages:vec![],unresolved:Some("excluded under Q2/Q11; losses unresolved".into())}]};
    let mut sources=vec![(b::DEPOSITS_PATH,None,b::HISTORY_FROM_MS,89),(b::WITHDRAWALS_PATH,None,b::HISTORY_FROM_MS,89)];
    if !us {
        sources.push((b::DUST_PATH,None,b::DUST_FROM_MS,89));
        for (path,_,_,_) in b::EARN {for kind in if path==b::EARN[0].0 {vec![Some("BONUS"),Some("REALTIME"),Some("REWARDS")]}else{vec![None]} {sources.push((path,kind,b::EARN_FROM_MS,30));}}
    }
    for (path,kind,floor,days) in sources {
        if floor>b::HISTORY_FROM_MS {e.coverage.push(Window{source:path.into(),kind:kind.map(str::to_string),start:b::HISTORY_FROM_MS,end:floor.min(cutoff.timestamp_millis()),pages:vec![],unresolved:Some("history predates endpoint retention".into())});}
        let capital=[b::DEPOSITS_PATH,b::WITHDRAWALS_PATH].contains(&path);let dust=path==b::DUST_PATH;let size=if capital {b::PAGE}else{100};
        for (start,end) in b::windows(floor,cutoff.timestamp_millis(),days) {
            let mut w=Window{source:path.into(),kind:kind.map(str::to_string),start,end,pages:vec![],unresolved:None};
            let mut window_inputs=vec![];let mut seen=std::collections::HashSet::new();let mut expected_total=None;
            for page in 0..=b::MAX_ENTRIES/size {
                let delay=if path==b::WITHDRAWALS_PATH {pacing.withdrawals}else if !capital && !dust {pacing.earn}else{std::time::Duration::ZERO};
                tokio::time::sleep(delay).await;
                let mut q=vec![("startTime",start.to_string()),("endTime",end.to_string()),("recvWindow",crate::venue::binance::READ_RECV_WINDOW.to_string())];
                if capital {q.extend([("offset",(page*size).to_string()),("limit",size.to_string())]);}
                else if !dust {q.extend([("current",(page+1).to_string()),("size",size.to_string())]);}
                if let Some(kind)=kind {q.push(("type",kind.into()));}
                let body=match venue.history_get(path,q,clock.now(),b::MAX_PAGE_BYTES).await {
                    Ok(body)=>body,
                    // Only an absent/gone endpoint establishes an unavailable window. Authentication,
                    // bad-request, throttling and transport errors must not complete the repair version.
                    Err(status) if !matches!(status,Some(404|410))=>{
                        w.unresolved=Some("retryable venue read".into());e.coverage.push(w);return e;
                    },
                    Err(_)=>{w.unresolved=Some("venue window unavailable".into());break},
                };
                w.pages.push(hash(&body));
                let decoded=(|| {
                    let node=b::page_of(&body,"repair page",b::MAX_PAGE_NODES).map_err(|_|"malformed page")?;
                    if capital {return b::objects(&node,"transfers").map_err(|_|"missing transfers array")}
                    let raw=super::activities::Raw::from_node(&node).map_err(|_|"missing page object")?;
                    if !dust {
                        let total=raw.value["total"].as_u64().or_else(||raw.value["total"].as_str().and_then(|s|s.parse().ok())).ok_or("missing total")? as usize;
                        if total>b::MAX_ENTRIES || expected_total.is_some_and(|old|old!=total) {return Err("changing or excessive total")}
                        expected_total=Some(total);
                    }
                    b::objects(raw.member(if dust {"userAssetDribblets"}else{"rows"}).ok_or("missing history array")?,"history").map_err(|_|"malformed history array")
                })();
                let rows=match decoded {Ok(rows)=>rows,Err(why)=>{w.unresolved=Some(why.into());break}};
                if (dust && rows.len()>=100) || rows.len()>size || expected_total.is_some_and(|n|page*size+rows.len()>n) || (!rows.is_empty() && !seen.insert(hash(&body))) {w.unresolved=Some("truncated or repeated page".into());break}
                for raw in &rows {
                    if dust && !raw.value["userAssetDribbletDetails"].is_array() {w.unresolved=Some("missing dust details".into());break}
                    match b::repair_map(path,kind,raw) {
                    Ok(inputs) if inputs.iter().all(|i|i.entry.transacted_at.is_some_and(|t|{let at=writer::identity_ms(t);at>=start && at<=end}))=>window_inputs.extend(inputs),
                    _=>{w.unresolved=Some("unresolved identity, pending transfer or out-of-window record".into());break}
                }}
                if w.unresolved.is_some(){break}
                if window_inputs.len()+e.inputs.len()>b::MAX_ENTRIES {w.unresolved=Some("history exceeds run bound".into());break}
                if !capital && !dust && rows.len()<size && (page*size+rows.len())!=expected_total.unwrap_or(0) {w.unresolved=Some("incomplete page total".into());break}
                if dust || rows.len()<size || expected_total.is_some_and(|n|(page+1)*size>=n) {break}
                if page==b::MAX_ENTRIES/size {w.unresolved=Some("page budget exhausted".into());}
            }
            if w.unresolved.is_none() {
                // Inclusive abutting windows can repeat their boundary record. Preserve multiplicity
                // within an answer: two indistinguishable rewards must still be unresolved.
                let prior:std::collections::HashSet<_>=e.inputs.iter().map(|i|format!("{:?}",i.entry)).collect();
                e.inputs.extend(window_inputs.into_iter().filter(|i|!prior.contains(&format!("{:?}",i.entry))));
            }
            e.coverage.push(w);
        }
    } e
}

/// Increment for a new repair algorithm. Completion is install-wide and scoped by this version;
/// it does not certify that Binance supplied every historical window.
pub const REPAIR_VERSION:i64=1;
pub const JOB:&str="binance_history_repair";
const ACCOUNT_JOB:&str="binance_history_repair_account";
const VENUES:[&str;2]=[crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US];

/// The live reader uses precisely the sync job's credentials, proxy, hosts and pacing.
#[allow(async_fn_in_trait)]
pub trait HistoryReader {
    async fn read(&self,key:&Key,credentials:&crate::crypto::Credentials,proxy:Option<&str>,clock:&dyn crate::engine::Clock,cutoff:DateTime<Utc>)->Evidence;
}
impl<C:super::jobs::Connect> HistoryReader for C {
    async fn read(&self,key:&Key,credentials:&crate::crypto::Credentials,proxy:Option<&str>,clock:&dyn crate::engine::Clock,cutoff:DateTime<Utc>)->Evidence {
        let venue=crate::venue::binance::BinanceVenue::new(self.binance_transport(credentials,proxy),
            &self.binance_base(&key.exchange_type).unwrap_or_default(),&credentials.secret);
        fetch(&venue,clock,cutoff,key.exchange_type==VENUES[1],self.binance_pacing()).await
    }
}

pub struct HistoryRepair<R> { pub reader:R, version:i64 }
impl<R> HistoryRepair<R> {
    pub fn new(reader:R)->Self {Self{reader,version:REPAIR_VERSION}}
    #[doc(hidden)]
    pub fn with_version(self,version:i64)->Self {Self{version,..self}}
}
impl<R:HistoryReader> crate::jobs::Job for HistoryRepair<R> {
    fn spec(&self)->crate::jobs::Spec {
        use crate::jobs::{schedule::{Schedule,Jitter},Retry,Spec};
        Spec{name:JOB,scope:Some(self.version.to_string()),schedule:Some(Schedule::Daily{hour:3,minute:0}),
            jitter:Jitter::NONE,retry:Retry::Polynomial{attempts:5},deadline:super::jobs::LEDGER_DEADLINE}
    }
    fn run<'a>(&'a self,cx:crate::jobs::Cx<'a>,_:Vec<crate::jobs::Wake>)->crate::jobs::JobFuture<'a> {
        Box::pin(async move {match self.run_history(&cx).await {
            Ok(out)=>out,
            Err(_)=>crate::jobs::Outcome::Transient("Binance history repair incomplete; retry scheduled".into()),
        }})
    }
}
fn account_scope(version:i64,key:&Key)->String {format!("{version}:{}:{}",key.user_id,key.exchange_id)}
pub fn gap_key(owner:i64)->String {format!("rust_binance_repair_gaps.{owner}")}

impl<R:HistoryReader> HistoryRepair<R> {
    async fn run_history(&self,cx:&crate::jobs::Cx<'_>)->Result<crate::jobs::Outcome,String> {
        use crate::jobs::{state,Outcome};
        let (revision,now)=(self.version,cx.clock.now());
        let start=cx.db.run(move |c,cipher| {
            if require_ownership(c,cipher).is_err(){return Ok(None)}
            let scope=revision.to_string();let mut state=state::read(c,JOB,Some(&scope))?;
            if state.last_success_at.is_some(){return Ok(None)}
            let keys=super::reading_keys_of(c,&VENUES).map_err(|e|e.0)?;
            if keys.is_empty(){return Ok(None)}
            let cutoff=state.source_at.unwrap_or(now);
            state.source_at=Some(cutoff);state.importing_revision=Some(revision);state.incomplete_since.get_or_insert(now);
            state::write(c,JOB,Some(&scope),&state,now)?;
            Ok(Some((cutoff,keys)))
        }).await?;
        let Some((cutoff,keys))=start else{return Ok(Outcome::NothingNew)};
        let mut completed=vec![];let mut inserted=0;let mut unresolved=0;
        for id in keys {
            let captured=cx.db.run(move |c,cipher| {
                let tx=c.unchecked_transaction().map_err(|e|e.to_string())?;
                require_ownership(&tx,cipher).map_err(|e|e.0)?;
                let key=super::load_key_of(&tx,id,&VENUES).map_err(|e|e.0)?;
                let (credentials,version)=super::credentials_with_version(&tx,cipher,id).map_err(|e|e.0)?;
                let saved=app_config::get_plain(&tx,&state::key(ACCOUNT_JOB,Some(&account_scope(revision,&key))))?
                    .and_then(|s|serde_json::from_str::<Value>(&s).ok());
                let done=saved.as_ref().is_some_and(|v|v["credential"]==version.cache_stamp() && v["last_success_at"].is_string());
                let gaps=saved.as_ref().and_then(|v|v["unresolved"].as_u64()).unwrap_or(0) as usize;
                Ok((key,credentials,version,done,gaps))
            }).await?;
            let (key,credentials,version,done,gaps)=captured;
            completed.push((key.clone(),version.clone()));
            if done {unresolved+=gaps;continue}
            let (_,proxy)=super::jobs::venue_of(&cx.db,id).await.map_err(|e|e.0)?;
            let evidence=self.reader.read(&key,&credentials,proxy.as_deref(),cx.clock,cutoff).await;
            if evidence.coverage.iter().any(|w|w.unresolved.as_deref()==Some("retryable venue read")) {
                return Err("retryable venue read".into())
            }
            let key_copy=key.clone();let version_copy=version.clone();
            let (manifest,mut session)=cx.db.run(move |c,cipher| {
                let m=dry_run(c,cipher,&key_copy,&version_copy,cutoff,&evidence).map_err(|e|e.0)?;
                let session=ApplySession::new(&m,&m.digest(),&evidence).map_err(|e|e.0)?;
                Ok((std::sync::Arc::new(m),session))
            }).await?;
            for n in 0..manifest.batches.len() {
                let key=key.clone();
                let (next,count)=cx.db.run(move |c,cipher| {
                    let count=session.apply_batch(c,cipher,&key,n).map_err(|e|e.0)?;Ok((session,count))
                }).await?;
                session=next;inserted+=count;
                // Normal sync's write gap gives other engine and web writers a turn between atomic buckets.
                tokio::time::sleep(super::WRITE_GAP).await;
            }
            let gaps=manifest.unresolved.len()+manifest.coverage.iter().filter(|w|w.unresolved.is_some()).count();
            unresolved+=gaps;
            let owner=key.user_id;
            cx.db.run(move |c,cipher| {
                session.finish(c,cipher,&key,|fenced| {
                    // The diagnostic has no monetary estimate and uses the existing Import history UI.
                    let name=gap_key(key.user_id);
                    let mut stored:Value=app_config::get_plain(fenced,&name).map_err(SyncError)?
                        .and_then(|s|serde_json::from_str(&s).ok()).filter(Value::is_object).unwrap_or_else(||json!({}));
                    stored[key.exchange_id.to_string()]=json!({"credential":version.cache_stamp(),"unresolved":gaps,"version":revision});
                    app_config::set_plain(fenced,&name,&stored.to_string(),now).map_err(SyncError)?;
                    let scope=account_scope(revision,&key);
                    app_config::set_plain(fenced,&state::key(ACCOUNT_JOB,Some(&scope)),
                        &json!({"last_success_at":now.to_rfc3339(),"revision":revision,"credential":version.cache_stamp(),"unresolved":gaps}).to_string(),now).map_err(SyncError)?;
                    Ok(())
                }).map_err(|e|e.0)
            }).await?;
            cx.wakers.wake(crate::tracker::jobs::TRACKER_LEDGER,Some(&owner.to_string()),None);
            cx.db.notifications.sync_done(owner);
        }
        cx.db.run(move |c,cipher| {
            let tx=model::immediate(c).map_err(|e|SyncError::from(e).0)?;
            let versions:Vec<_>=completed.iter().map(|(_,v)|v.clone()).collect();
            let fenced=model::fence_versions(&tx,&versions).map_err(|e|SyncError::from(e).0)?;
            require_ownership(&fenced,cipher).map_err(|e|e.0)?;
            for (key,_) in &completed {require_scope(&fenced,key).map_err(|e|e.0)?;}
            // A key added while the requests were in flight must be included before install completion.
            let mut current=super::reading_keys_of(&fenced,&VENUES).map_err(|e|e.0)?;
            let mut expected:Vec<_>=completed.iter().map(|(key,_)|key.id).collect();current.sort();expected.sort();
            if current!=expected {return Err("reading keys changed".into())}
            let scope=revision.to_string();let mut state=state::read(&fenced,JOB,Some(&scope))?;
            state.revision=Some(revision);state.importing_revision=None;state.incomplete_since=None;
            state.last_success_at=Some(now);state.last_run_at=Some(now);
            let _guard=AppendOnly::new(c);
            state::write(&fenced,JOB,Some(&scope),&state,now)?;tx.commit().map_err(|e|e.to_string())
        }).await?;
        crate::engine::log(&format!("[binance-history-repair] version {revision}: inserted {inserted} rows, {unresolved} unresolved buckets/windows"));
        Ok(Outcome::Done)
    }
}
