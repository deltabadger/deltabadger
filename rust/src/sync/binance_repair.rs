//! Explicit A12 maintenance. No startup caller, normal watermark changes or transfer matching.
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
    let current=super::load_key_of(&copy,key.id,&[crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US])?;
    if (current.user_id,current.exchange_id)!=(key.user_id,key.exchange_id) {return Err(refuse("credential scope changed"))}
    let mut m=Manifest{version:1,user_id:key.user_id,exchange_id:key.exchange_id,key_id:key.id,cutoff:cutoff.to_rfc3339(),credential:version.cache_stamp(),
        evidence:evidence_digest(e),original:rows(&copy)?,retained:metadata(&copy,key)?,coverage:e.coverage.clone(),unresolved:vec![],tally:Tally::default(),batches:vec![]};
    let grouped=groups(e)?;let mut locations=BTreeMap::<String,std::collections::HashSet<String>>::new();
    for (bucket,input) in &grouped {for i in input {if let Some(k)=i.event.key(){locations.entry(k.tuple().into()).or_default().insert(bucket.clone());}}}
    for (bucket,input) in grouped {
        let name=bucket_name(key,&bucket);let before=app_config::get_plain(&copy,&name).map_err(SyncError)?;
        let old=rows(&copy)?;
        let incomplete=e.coverage.iter().any(|w|w.unresolved.is_some() && input.iter().any(|i|covers(w,i)));
        if incomplete || input.iter().any(|i|i.event.key().is_some_and(|k|locations[k.tuple()].len()>1)) {
            m.unresolved.push(format!("{}: {}",hash(&bucket),if incomplete {"source coverage is incomplete"}else{"canonical identity appears in multiple buckets"}));tally(&mut m.tally,&input,&old,None)?;unresolved_windows(&mut m,&input,"unresolved collision bucket");continue;
        }
        let tx=model::immediate(&copy)?;
        let fenced=model::fence_versions(&tx,std::slice::from_ref(version))?;
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
            let inserted=rows(&fenced)?.keys().filter(|id|!old.contains_key(id)).map(|id|writer::image(&fenced,*id)).collect::<Result<Vec<_>,_>>()?;
            m.batches.push(Batch{bucket,before,after:after.ok_or_else(||refuse("missing writer journal"))?,inserted,
                coverage:m.coverage.iter().filter(|w|input.iter().any(|i|covers(w,i))).cloned().collect(),
                original_checksum:hash(&json!(m.original).to_string())});
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
    let current=super::load_key_of(&fenced,key.id,&[crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US])?;
    if (current.user_id,current.exchange_id)!=(key.user_id,key.exchange_id) {return Err(refuse("credential scope changed"))}
    if m.batches.is_empty() && (rows(&fenced)?!=m.original || metadata(&fenced,key)?!=m.retained) {return Err(refuse("ledger changed"))} Ok(())
}
/// Each call is one complete collision bucket (at most the guarded writer's 1000 rows).
/// The checkpoint contains the exact after-images and alias/anchor decision, atomically with the writer.
#[allow(clippy::too_many_arguments)]
pub fn apply_batch(c:&Connection,cipher:&Cipher,key:&Key,m:&Manifest,approved:&str,e:&Evidence,n:usize)->Result<usize,SyncError> {
    if m.version!=1 || m.digest()!=approved || evidence_digest(e)!=m.evidence || (key.id,key.user_id,key.exchange_id)!=(m.key_id,m.user_id,m.exchange_id) {return Err(refuse("approval, scope or fetched evidence changed"))}
    let cutoff=m.cutoff.parse::<DateTime<Utc>>().map_err(|_|refuse("invalid cutoff"))?;
    let batch=m.batches.get(n).ok_or_else(||refuse("unknown batch"))?;
    let version=CredentialVersion::from_stamp(&m.credential).ok_or_else(||refuse("unreadable credential fence"))?;
    let tx=model::immediate(c)?;let fenced=model::fence_versions(&tx,&[version])?;require_ownership(&fenced,cipher)?;
    let current=super::load_key_of(&fenced,key.id,&[crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US])?;
    if (current.user_id,current.exchange_id)!=(key.user_id,key.exchange_id) {return Err(refuse("credential scope changed"))}
    let mut expected=m.original.clone();let mut expected_metadata=m.retained.clone();
    for (i,b) in m.batches.iter().enumerate() {
        if let Some(saved)=app_config::get_plain(&fenced,&journal(approved,i)).map_err(SyncError)? {
            if saved!=serde_json::to_string(b).expect("batch") || app_config::get_plain(&fenced,&bucket_name(key,&b.bucket)).map_err(SyncError)?.as_deref()!=Some(&b.after) {return Err(refuse("committed batch changed"))}
            expected_metadata.insert(bucket_name(key,&b.bucket),hash(&b.after));
            for row in &b.inserted {expected.insert(row["id"].as_i64().ok_or_else(||refuse("invalid after-image"))?,hash(&row.to_string()));}
        } else if i<n {return Err(refuse("resume batches in manifest order"))}
    }
    if rows(&fenced)?!=expected || metadata(&fenced,key)?!=expected_metadata {return Err(refuse("ledger rows or links changed; repeat dry run"))}
    let checkpoint=journal(approved,n);
    if app_config::get_plain(&fenced,&checkpoint).map_err(SyncError)?.is_some() {return Ok(0)}
    let name=bucket_name(key,&batch.bucket);
    if app_config::get_plain(&fenced,&name).map_err(SyncError)?!=batch.before {return Err(refuse("bucket or suppression changed; repeat dry run"))}
    let input=e.inputs.iter().filter_map(|i|match writer::bucket_id(&i.entry) {
        Ok(bucket) if bucket==batch.bucket=>Some(Ok(i.clone())),Ok(_)=>None,Err(e)=>Some(Err(e)),
    }).collect::<Result<Vec<_>,_>>()?;
    if input.is_empty(){return Err(refuse("missing bucket evidence"))}
    let out=writer::store(&fenced,cipher,key,&input,cutoff)?;
    if app_config::get_plain(&fenced,&name).map_err(SyncError)?.as_deref()!=Some(&batch.after) {return Err(refuse("writer decision changed; repeat dry run"))}
    for row in &batch.inserted {if writer::image(&fenced,row["id"].as_i64().ok_or_else(||refuse("invalid after-image"))?)?!=*row {return Err(refuse("inserted after-image changed"))}}
    if out.imported!=batch.inserted.len() {return Err(refuse("insert count changed"))}
    let actual=rows(&fenced)?;for row in &batch.inserted {expected.insert(row["id"].as_i64().unwrap(),hash(&row.to_string()));}
    if actual!=expected {return Err(refuse("existing row changed"))}integrity(&fenced)?;
    writer::mark(&fenced,cutoff)?;
    app_config::set_plain(&fenced,&checkpoint,&serde_json::to_string(batch).expect("batch"),cutoff).map_err(SyncError)?;
    tx.commit()?;Ok(out.imported)
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
                let body=match venue.signed_get(path,q,clock.now(),b::MAX_PAGE_BYTES).await {Ok(body)=>body,Err(_)=>{w.unresolved=Some("venue window unavailable".into());break}};
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
