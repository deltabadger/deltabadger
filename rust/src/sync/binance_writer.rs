//! A12 identity writer shared by live Binance sync and file import.
//! Network work and input assembly happen outside the transaction.
//! The caller supplies complete collision buckets; errors must roll back the enclosing transaction.
use super::{activities::Entry, binance_identity::{classify, Candidate, Decision, Event}, ledger::{self, Outcome}, Key, SyncError};
use crate::{app_config, crypto::Cipher, engine::model::FencedTransaction, ruby::BigDec};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

/// Undo only Ruby's sub-millisecond floating-point timestamp rounding. Canonical
/// source evidence keeps integer milliseconds; ledger timestamps remain unchanged.
pub(crate) fn identity_ms(at: DateTime<Utc>) -> i64 { (at.timestamp_micros()+500).div_euclid(1000) }
pub const MARKER: &str = "rust_binance_a12_v1:install_written";
const PREFIX: &str = "rust_binance_a12_v1:bucket:";
const LIMIT: usize = 1000;
#[derive(Clone)]
pub struct Input { pub entry: Entry, pub event: Event }
#[derive(Serialize, Deserialize)]
struct Binding { event: Event, row_id: i64, raw: Value, at: i64, kind: i64, coin: String, amount: String,
    net: String, tx_id: String, group: Option<String>, inserted: bool, suppressed: bool, suppression_reason: Option<String>, after: Value }
#[derive(Serialize, Deserialize)]
struct Bucket { version: u8, legacy: String, bindings: Vec<Binding> }
fn refused(reason: &str) -> SyncError { SyncError(format!("Binance A12 unresolved: {reason}")) }
/// Only named import identity refusals are validation failures. Ownership, SQL and
/// unexpected errors remain errors even if they mention A12.
pub(crate) fn csv_refusal(error: &SyncError) -> Option<&str> {
    let reason = error.0.strip_prefix("Binance A12 unresolved: ")?;
    matches!(reason,
        "unreadable identity metadata" | "conflicting bucket digest" | "unsupported bucket" |
        "missing retained identity" | "duplicate or conflicting retained digest" |
        "suppression has no retained reason" | "unreadable retained identity evidence" |
        "partial CSV dust pair" | "exported identity has no retained binding" | "exported CSV identity changed" |
        "ambiguous CSV identity" | "partial suppressed pair" | "CSV identities are not one-to-one" |
        "CSV alias points to deleted row" | "CSV alias changed" | "missing CSV candidate raw data" |
        "unreadable CSV candidate raw data" | "CSV candidate identity changed" | "CSV candidate is not one whole pair" |
        "CSV cannot exclude a suppressed event" | "missing CSV outgoing leg" | "missing CSV incoming leg" |
        "ambiguous CSV dust pair" | "partial existing CSV dust pair" | "ambiguous CSV dust identity" |
        "CSV dust identity has no proven whole-pair match" | "CSV group is already occupied"
    ).then_some(reason)
}
fn hash(s: &str) -> String { hex::encode(Sha256::digest(s.as_bytes())) }
fn scope(key: &Key) -> String { format!("{PREFIX}{}:{}:",key.user_id,key.exchange_id) }
fn bucket_id(e: &Entry) -> Result<String,SyncError> {
    let id=e.tx_id.as_deref().filter(|s|!s.trim().is_empty()).ok_or_else(||refused("missing legacy bucket"))?;
    Ok(if id.starts_with("dust-") { id.strip_suffix("-in").or_else(||id.strip_suffix("-out")).unwrap_or(id) }else{id}.into())
}
/// Preserve complete legacy collision buckets across bounded write transactions.
pub(crate) fn batches(input: Vec<(usize, Input)>) -> Result<BTreeMap<usize, Vec<Input>>, SyncError> {
    let mut groups: BTreeMap<String, (usize, usize, Vec<Input>)> = BTreeMap::new();
    for (n, i) in input {let group=groups.entry(bucket_id(&i.entry)?).or_insert_with(|| (n,n,vec![]));group.1=n;group.2.push(i);}
    if groups.values().any(|(_, _, g)| g.len() > LIMIT) { return Err(refused("collision bucket exceeds bound")); }
    let mut groups:Vec<_>=groups.into_values().collect();groups.sort_by_key(|(first,_,_)|*first);
    let mut batches:BTreeMap<usize,Vec<Input>>=BTreeMap::new();let mut end=None;
    for (first,last,mut group) in groups {
        if batches.last_key_value().is_some_and(|(_,b)|end==Some(first) && b.len()+group.len()<=100) {
            batches.last_entry().unwrap().get_mut().append(&mut group);
        }else{batches.insert(first,group);}
        end=Some(last+1);
    }
    Ok(batches)
}

pub fn written(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE key=?1)",[MARKER],|r|r.get(0))
}
fn ownership(c: &FencedTransaction<'_>, cipher: &Cipher, key: &Key) -> Result<(),SyncError> {
    if ![crate::venue::binance::BINANCE,crate::venue::binance::BINANCE_US].contains(&key.exchange_type.as_str()) {return Err(refused("wrong venue"))}
    if c.is_autocommit() || !crate::lease::read(c,cipher).map_err(|_|refused("unreadable ownership"))?.is_some_and(|v|v["engine"]=="rust") {
        return Err(refused("Rust ownership required"));
    } Ok(())
}
fn mark(c: &FencedTransaction<'_>, now: DateTime<Utc>) -> Result<(),SyncError> {
    if !written(c)? {app_config::set_plain(c,MARKER,"1",now).map_err(SyncError)?;} Ok(())
}
fn invalidate(c: &Connection, owner: i64) -> Result<(),SyncError> {
    for key in [crate::tracker::cache::key(owner),crate::tracker::backfill::history_key(owner),crate::tracker::backfill::price_key(owner)] {
        c.execute("DELETE FROM app_configs WHERE key=?1",[key])?;
    }
    c.execute("DELETE FROM portfolio_snapshots WHERE user_id=?1",[owner])?;
    c.execute("DELETE FROM portfolio_venue_snapshots WHERE user_id=?1",[owner])?; Ok(())
}
fn image(c: &Connection, id: i64) -> Result<Value,rusqlite::Error> {
    let mut s=c.prepare("SELECT * FROM account_transactions WHERE id=?1")?;
    let names=s.column_names().iter().map(|n|n.to_string()).collect::<Vec<_>>();
    s.query_row([id],|r| {let mut out=serde_json::Map::new();for (i,n) in names.iter().enumerate() {
        let v=match r.get_ref(i)? {rusqlite::types::ValueRef::Null=>Value::Null,rusqlite::types::ValueRef::Integer(n)=>json!(n),
            rusqlite::types::ValueRef::Real(n)=>json!(n),rusqlite::types::ValueRef::Text(s)=>json!(String::from_utf8_lossy(s)),rusqlite::types::ValueRef::Blob(b)=>json!(hex::encode(b))};out.insert(n.clone(),v);
    }Ok(Value::Object(out))})
}
fn load(c: &Connection, name: &str, legacy: &str) -> Result<Bucket,SyncError> {
    let text:Option<Option<String>>=c.query_row("SELECT value FROM app_configs WHERE key=?1",[name],|r|r.get(0)).optional()?;
    let b:Bucket=match text {None=>Bucket{version:1,legacy:legacy.into(),bindings:vec![]},Some(Some(text))=>serde_json::from_str(&text).map_err(|_|refused("unreadable bucket"))?,Some(None)=>return Err(refused("empty bucket"))};
    if b.legacy!=legacy {return Err(refused("conflicting bucket digest"))} validate(&b)?;Ok(b)
}
fn validate(b: &Bucket) -> Result<(),SyncError> {
    if b.version!=1 || b.bindings.len()>LIMIT {return Err(refused("unsupported bucket"))}
    let mut keys=HashSet::new();
    for a in &b.bindings {let k=a.event.key().ok_or_else(||refused("missing retained identity"))?;
        if !keys.insert(k.digest()) {return Err(refused("duplicate or conflicting retained digest"))}
        if a.suppressed && a.suppression_reason.as_deref().is_none_or(str::is_empty) {return Err(refused("suppression has no retained reason"))}
        if !a.event.legacy(&a.raw,a.at).is_ok_and(|e|e.equivalent(&a.event)) {return Err(refused("unreadable retained identity evidence"))}}
    Ok(())
}
fn buckets(c: &Connection, key: &Key) -> Result<Vec<Bucket>,SyncError> {
    let mut stmt=c.prepare("SELECT key,value FROM app_configs WHERE key>=?1 AND key<?2 ORDER BY key")?;
    let prefix=scope(key);let texts=stmt.query_map(params![prefix,format!("{prefix}\u{7f}")],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    texts.into_iter().map(|(name,text)| {let b:Bucket=serde_json::from_str(&text).map_err(|_|refused("unreadable identity metadata"))?;
        if name!=format!("{prefix}{}",hash(&b.legacy)) {return Err(refused("conflicting bucket digest"))}validate(&b)?;Ok(b)}).collect()
}

/// All rows, aliases, journal after-images, the marker and invalidation share the caller's fence.
/// Existing rows are only read. Suppressions are checked before legacy matching or insertion.
pub fn store(c: &FencedTransaction<'_>, cipher: &Cipher, key: &Key, input: &[Input], now: DateTime<Utc>) -> Result<Outcome,SyncError> {
    ownership(c,cipher,key)?;
    let retained=buckets(c,key)?;
    let mut groups:BTreeMap<String,(usize,Vec<&Input>)>=BTreeMap::new();
    for (n,i) in input.iter().enumerate() {if i.event.key().is_none() {return Err(refused("missing stable identity"))}
        let at=i.entry.transacted_at.ok_or_else(||refused("missing time"))?;
        if !i.event.legacy(&i.entry.raw.value,identity_ms(at)).is_ok_and(|e|e.equivalent(&i.event)) {return Err(refused("input context disagrees with raw identity"))}
        let legacy=bucket_id(&i.entry)?;
        if retained.iter().any(|b|b.legacy!=legacy && b.bindings.iter().any(|a|a.event.key().is_some_and(|k|k.digest()==i.event.key().unwrap().digest()))) {
            return Err(refused("canonical identity moved to another legacy bucket"));
        }
        groups.entry(legacy).or_insert_with(||(n,vec![])).1.push(i);}
    let mut out=Outcome::default();
    let mut groups:Vec<_>=groups.into_iter().collect();groups.sort_by_key(|(_, (n,_))|*n);
    for (legacy,(_,mut items)) in groups {
        if items.len()>LIMIT {return Err(refused("collision bucket exceeds bound"))}
        items.sort_by(|a,b| {
            let order=|i:&Input|(i.event.pair_key().unwrap_or_else(||i.event.key().unwrap()).tuple().to_string(),i.entry.entry_type==2);
            order(a).cmp(&order(b))
        });
        let name=format!("{}{}",scope(key),hash(&legacy));let mut b=load(c,&name,&legacy)?;
        let before=serde_json::to_string(&b).expect("bucket");
        let mut live=vec![];
        for i in items {if let Some(a)=b.bindings.iter().find(|a|a.event.key()==i.event.key() && a.suppressed) {
            if !a.event.equivalent(&i.event) {return Err(refused("suppressed identity correction"))} out.skipped+=1;
        }else{live.push(i)}}
        if live.is_empty(){continue}
        let mut candidates=vec![];
        for a in b.bindings.iter().filter(|a|!a.suppressed) {
            let row=image(c,a.row_id).map_err(|_|refused("alias points to deleted row"))?;
            if row["user_id"]!=key.user_id || row["exchange_id"]!=key.exchange_id || row["tx_id"]!=a.tx_id {return Err(refused("alias scope changed"))}
            let mut raw:Value=serde_json::from_str(row["raw_data"].as_str().ok_or_else(||refused("missing raw data"))?).map_err(|_|refused("unreadable raw data"))?;
            if let Some(raw)=raw.as_object_mut(){raw.remove("file_twin_at");}
            if !a.event.legacy(&raw,a.at).is_ok_and(|e|e.equivalent(&a.event)) {return Err(refused("retained raw identity changed"))}
            candidates.push(Candidate{row_id:a.row_id,event:Some(a.event.clone()),legacy_id:true});
        }
        // Include every legacy leg and reject possible nil-ID twins before assigning an anchor.
        let ids:HashSet<_>=live.iter().filter_map(|i|i.entry.tx_id.as_deref()).collect();
        let capital_ids:Vec<_>=live.iter().filter(|i|i.event.capital()).map(|i|json!([i.entry.entry_type,super::binance_ledger::text(i.entry.raw.value.get("id"))])).collect();
        let mut stmt=c.prepare("SELECT id,tx_id,entry_type,base_currency,transacted_at,raw_data FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND (tx_id=?3 OR tx_id=?4 OR tx_id=?5 OR tx_id IS NULL OR (json_valid(raw_data) AND json_array(entry_type,CAST(json_extract(raw_data,'$.id') AS TEXT)) IN (SELECT value FROM json_each(?6)))) ORDER BY id")?;
        let rows=stmt.query_map(params![key.user_id,key.exchange_id,legacy,format!("{legacy}-in"),format!("{legacy}-out"),json!(capital_ids).to_string()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))?.collect::<Result<Vec<_>,_>>()?;
        for (id,txid,kind,coin,at,raw) in rows {
            if candidates.iter().any(|a|a.row_id==id){continue}
            let at=crate::codec::parse_time(&at)?;
            let decoded=serde_json::from_str::<Value>(&raw).ok();
            if txid.is_none() {
                if decoded.as_ref().is_some_and(|raw|capital_ids.contains(&json!([kind,super::binance_ledger::text(raw.get("id"))]))) {return Err(refused("nil-ID row retains this capital identity"))}
                if live.iter().any(|i|i.entry.entry_type==kind && i.entry.base_currency.as_deref()==Some(&coin) &&
                    (i.entry.transacted_at.is_some_and(|t|(t-at).num_seconds().abs()<=600) || (kind==5 && i.entry.raw.value["completeTime"].as_str()
                        .and_then(|t|chrono::NaiveDateTime::parse_from_str(t,"%Y-%m-%d %H:%M:%S").ok()).is_none_or(|t|(t.and_utc()-at).num_seconds().abs()<=1)))) {return Err(refused("possible nil-ID/CSV twin"))} continue;
            }
            let raw=decoded.ok_or_else(||refused("unreadable legacy raw data"))?;
            if !ids.contains(txid.as_deref().unwrap()) && !capital_ids.contains(&json!([kind,super::binance_ledger::text(raw.get("id"))])) {continue}
            let mut matches:Vec<Event>=live.iter().filter(|i|i.entry.entry_type==kind).filter_map(|i|i.event.legacy(&raw,identity_ms(at)).ok())
                .filter(|e|e.pair_key().is_none() || live.iter().any(|i|i.event.equivalent(e))).collect();
            matches.dedup_by(|a,b|a.equivalent(b));
            if matches.len()!=1 {return Err(refused("legacy survivor is not uniquely identified"))}
            candidates.push(Candidate{row_id:id,event:matches.pop(),legacy_id:true});
        }
        let events:Vec<_>=live.iter().map(|i|i.event.clone()).collect();let decisions=classify(&events,&candidates);
        if decisions.iter().any(|d|matches!(d,Decision::Conflict|Decision::Unresolved)){return Err(refused("conflicting or incomplete collision bucket"))}
        let mut occupied:HashSet<String>=b.bindings.iter().map(|a|a.tx_id.clone()).collect();
        for a in &candidates {occupied.insert(image(c,a.row_id)?["tx_id"].as_str().ok_or_else(||refused("nil anchor"))?.into());}
        for (i,decision) in live.into_iter().zip(decisions) {
            if b.bindings.iter().any(|a|a.event.key()==i.event.key()) {out.duplicates+=1;continue}
            let mut e=i.entry.clone();
            let (id,inserted)=match decision {
                Decision::Existing(id)=>{out.duplicates+=1;(id,false)},
                Decision::Missing=>{
                    if occupied.contains(e.tx_id.as_deref().unwrap()) {e.tx_id=i.event.sibling_id();}
                    if let Some(group)=i.event.group_id(){
                        let existing:usize=c.query_row("SELECT count(*) FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND group_id=?3",params![key.user_id,key.exchange_id,group],|r|r.get(0))?;
                        let own=b.bindings.iter().filter(|a|a.event.pair_key()==i.event.pair_key() && !a.suppressed).count();
                        if existing!=own {return Err(refused("canonical group already belongs to other rows"))} e.group_id=Some(group);
                    }
                    let id=insert(c,key,&e,now)?;out.imported+=1;occupied.insert(e.tx_id.clone().unwrap());(id,true)
                },_=>unreachable!(),
            };
            let after=image(c,id)?;
            b.bindings.push(Binding {event:i.event.clone(),row_id:id,raw:i.entry.raw.value.clone(),at:identity_ms(i.entry.transacted_at.ok_or_else(||refused("missing time"))?),
                kind:e.entry_type,coin:e.base_currency.clone().ok_or_else(||refused("missing coin"))?,amount:e.base_amount.to_s_f(),
                net:(&e.base_amount-&e.fee_amount.clone().unwrap_or_else(BigDec::zero)).to_s_f(),tx_id:after["tx_id"].as_str().ok_or_else(||refused("nil anchor"))?.into(),
                group:after["group_id"].as_str().map(str::to_string),inserted,suppressed:false,suppression_reason:None,after});
        }
        if b.bindings.len()>LIMIT {return Err(refused("retained bucket exceeds bound"))}
        let after=serde_json::to_string(&b).expect("bucket");if after!=before {mark(c,now)?;app_config::set_plain(c,&name,&after,now).map_err(SyncError)?;}
    }
    if out.imported>0 {invalidate(c,key.user_id)?;} Ok(out)
}

fn insert(c: &FencedTransaction<'_>, key: &Key, e: &Entry, now: DateTime<Utc>) -> Result<i64,SyncError> {
    let at=e.transacted_at.ok_or_else(||refused("missing time"))?;
    let coin=e.base_currency.as_deref().filter(|s|!s.is_empty()).ok_or_else(||refused("missing coin"))?;
    let asset=ledger::asset_rule(c,key.exchange_id,&key.exchange_type)?.asset_id(c,Some(coin),&e.raw.value,Some(at))?;
    let amount=|d:&BigDec|ledger::amount_sql(&key.exchange_type,d);
    let fee=e.fee_amount.as_ref().filter(|d|!d.is_zero());
    c.execute("INSERT INTO account_transactions (user_id,api_key_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,tx_id,group_id,description,transacted_at,raw_data,manual_values,base_asset_id,transfer_link_rejected,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,'{}',?16,0,?17,?17)",
        params![key.user_id,key.id,key.exchange_id,e.entry_type,coin,amount(&e.base_amount),e.quote_currency,e.quote_amount.as_ref().map(amount),fee.and(e.fee_currency.as_deref()),fee.map(amount),e.tx_id,e.group_id,e.description,super::sql_time(at),e.raw.text(),asset,super::sql_time(now)])?;
    Ok(c.last_insert_rowid())
}

/// Whole-pair matching also applies to already-stamped anchors. An incoming BNB leg alone
/// never establishes a duplicate; immutable outgoing evidence must identify the same pair.
pub fn store_csv(c: &FencedTransaction<'_>, cipher: &Cipher, key: &Key, entries: &[Entry], now: DateTime<Utc>) -> Result<Outcome,SyncError> {
    ownership(c,cipher,key)?;
    let bindings:Vec<_>=buckets(c,key)?.into_iter().flat_map(|b|b.bindings).collect();
    let mut units:BTreeMap<String,(usize,Vec<&Entry>)>=BTreeMap::new();
    for (n,e) in entries.iter().enumerate(){units.entry(e.group_id.clone().unwrap_or_else(||format!("row:{n}"))).or_insert_with(||(n,vec![])).1.push(e);}
    let mut out=Outcome::default();let mut used=HashSet::new();let mut fallback=vec![];
    let mut units:Vec<_>=units.into_values().collect();units.sort_by_key(|(n,_)|*n);
    for (_,rows) in &units {
        let dust=rows.iter().any(|e|e.group_id.as_deref().is_some_and(|g|g.starts_with("dust")) || e.raw.value["operation"]=="Small Assets Exchange BNB");
        if dust && (rows.len()!=2 || rows.iter().filter(|e|e.entry_type==2).count()!=1 || rows.iter().filter(|e|e.entry_type==3).count()!=1) {return Err(refused("partial CSV dust pair"))}
        let window=|a:&Binding|->Option<(i64,i64)> {
            if a.kind==5 {let at=chrono::NaiveDateTime::parse_from_str(a.raw["completeTime"].as_str()?,"%Y-%m-%d %H:%M:%S").ok()?.and_utc().timestamp_millis();Some((at-1000,at+1000))}
            else if a.kind==4 {Some((a.at,a.at+600_000))}else{Some((a.at-1000,a.at+1000))}
        };
        let exact = |a:&Binding,e:&Entry| -> bool {
            a.kind == e.entry_type && e.base_currency.as_deref() == Some(&a.coin) &&
                a.after["base_amount"].as_f64().and_then(|v|BigDec::from_f64(v).ok()).is_some_and(|v|v==e.base_amount) && e.group_id==a.group &&
                // The app export writes whole seconds, truncating fractional ledger timestamps.
                e.transacted_at.is_some_and(|t|a.after["transacted_at"].as_str().and_then(|s|crate::codec::parse_time(s).ok()).is_some_and(|stored|stored.timestamp()==t.timestamp())) &&
                e.fee_currency==a.after["fee_currency"].as_str().map(str::to_string) &&
                e.fee_amount.as_ref().map(BigDec::to_s_f)==a.after["fee_amount"].as_f64().map(|v|BigDec::from_f64(v).map(|d|d.to_s_f())).transpose().ok().flatten()
        };
        for e in rows {
            if e.tx_id.as_deref().is_some_and(|id|(id.starts_with("binance-a12-v1:") || id.starts_with("dust-a12-v1-")) && !bindings.iter().any(|a|a.tx_id==id)) {
                return Err(refused("exported identity has no retained binding"));
            }
            if let Some(a)=e.tx_id.as_ref().and_then(|id|bindings.iter().find(|a| &a.tx_id==id)) {
                if !exact(a,e) {return Err(refused("exported CSV identity changed"));}
            }
        }
        let fits=|a:&Binding,e:&Entry|->bool {
            if let Some(id)=&e.tx_id {return id==&a.tx_id && exact(a,e)}
            let same_kind=a.kind==e.entry_type || ([6,7].contains(&a.kind) && [6,7].contains(&e.entry_type));
            let amount=if dust && e.entry_type==2 {&a.net}else{&a.amount};
            same_kind && e.base_currency.as_deref()==Some(&a.coin) && e.transacted_at.is_some_and(|t|window(a).is_some_and(|(from,to)|t.timestamp_millis()>=from && t.timestamp_millis()<=to)) &&
                BigDec::parse(amount).is_ok_and(|n|if dust && e.entry_type==2 {let unit=BigDec::parse("0.00000001").unwrap();e.base_amount>=&n-&unit && e.base_amount<=&n+&unit}
                    else if a.kind==5 {a.raw["transactionFee"].as_str().and_then(|v|BigDec::parse(v).ok()).is_some_and(|fee|e.base_amount==&n+&fee)}else{e.base_amount==n})
        };
        let candidates:Vec<Vec<&Binding>>=if dust {
            bindings.iter().filter(|a|fits(a,rows[0])).filter_map(|a| {
                let matches:Vec<_>=rows.iter().map(|e|bindings.iter().filter(|b|b.event.pair_key()==a.event.pair_key() && fits(b,e)).collect::<Vec<_>>()).collect();
                matches.iter().all(|m|m.len()==1).then(||matches.into_iter().map(|m|m[0]).collect())
            }).collect()
        }else{
            let matches:Vec<_>=rows.iter().map(|e|bindings.iter().filter(|a|fits(a,e)).collect::<Vec<_>>()).collect();
            if matches.iter().any(|m|m.len()>1) {return Err(refused("ambiguous CSV identity"))}
            if matches.iter().all(|m|m.len()==1) {vec![matches.into_iter().map(|m|m[0]).collect()]}else{vec![]}
        };
        if candidates.len()>1 {return Err(refused("ambiguous CSV identity"))}
        if let Some(pair)=candidates.first() {
            if pair.iter().any(|a|a.suppressed) {if pair.iter().any(|a|!a.suppressed){return Err(refused("partial suppressed pair"))}out.skipped+=rows.len();continue}
            if pair.iter().any(|a|!used.insert(a.row_id)) {return Err(refused("CSV identities are not one-to-one"))}
            for a in pair {let row=match image(c,a.row_id) {
                Ok(row)=>row,
                Err(rusqlite::Error::QueryReturnedNoRows)=>return Err(refused("CSV alias points to deleted row")),
                Err(error)=>return Err(error.into()),
            };
                if row["user_id"]!=key.user_id || row["exchange_id"]!=key.exchange_id || row["tx_id"]!=a.tx_id || row["group_id"]!=json!(a.group) {return Err(refused("CSV alias changed"))}
                let mut raw:Value=serde_json::from_str(row["raw_data"].as_str().ok_or_else(||refused("missing CSV candidate raw data"))?).map_err(|_|refused("unreadable CSV candidate raw data"))?;
                if let Some(raw)=raw.as_object_mut(){raw.remove("file_twin_at");}
                if !a.event.legacy(&raw,a.at).is_ok_and(|e|e.equivalent(&a.event)) {return Err(refused("CSV candidate identity changed"))}
                if dust && c.query_row("SELECT count(*) FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND group_id=?3",params![key.user_id,key.exchange_id,a.group],|r|r.get::<_,i64>(0))?!=2 {return Err(refused("CSV candidate is not one whole pair"))}
            }
            out.duplicates+=rows.len();continue;
        }
        // Partial evidence must not fall through to the per-leg matcher (equal BNB alone is
        // common; an unmatched outgoing leg cannot exclude a suppression with missing evidence).
        if bindings.iter().any(|a|a.suppressed && rows.iter().any(|e| {
            (a.kind==e.entry_type || ([6,7].contains(&a.kind) && [6,7].contains(&e.entry_type))) &&
            e.base_currency.as_deref().is_none_or(|coin|coin==a.coin) &&
            e.transacted_at.is_none_or(|t|window(a).is_none_or(|(from,to)|t.timestamp_millis()>=from && t.timestamp_millis()<=to))
        })) {return Err(refused("CSV cannot exclude a suppressed event"))}
        if dust {
            let imported = csv_dust(c, key, rows, &mut used, now)?;
            if imported {out.imported+=2;mark(c,now)?;invalidate(c,key.user_id)?;}else{out.duplicates+=2;}
            continue;
        }
        fallback.extend(rows.iter().map(|e|(*e).clone()));
    }
    // Keep the ledger's stored-row and repeat counters for the whole file.
    if !fallback.is_empty() {
        let before=c.total_changes();let result=ledger::store_file(c,key,fallback,now)?;
        out.imported+=result.imported;out.duplicates+=result.duplicates;out.skipped+=result.skipped;
        if c.total_changes()!=before {mark(c,now)?;invalidate(c,key.user_id)?;}
    }
    Ok(out)
}


/// File-only dust has no venue parent ID. Keep its tx_ids nil, so a later API read
/// must resolve the possible CSV twin instead of speculatively creating another pair.
fn csv_dust(c:&FencedTransaction<'_>, key:&Key, rows:&[&Entry], used:&mut HashSet<i64>, now:DateTime<Utc>) -> Result<bool,SyncError> {
    let out=rows.iter().find(|e|e.entry_type==3).ok_or_else(||refused("missing CSV outgoing leg"))?;
    let inn=rows.iter().find(|e|e.entry_type==2).ok_or_else(||refused("missing CSV incoming leg"))?;
    if inn.base_currency.as_deref()!=Some("BNB") || out.base_currency.as_deref()==Some("BNB") ||
        out.transacted_at.is_none() || out.transacted_at!=inn.transacted_at || !out.base_amount.is_positive() || !inn.base_amount.is_positive() {
        return Err(refused("ambiguous CSV dust pair"));
    }
    let at=out.transacted_at.unwrap();
    let partial:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions a WHERE a.user_id=?1 AND a.exchange_id=?2 AND a.group_id LIKE 'dust%' AND a.transacted_at BETWEEN ?3 AND ?4 AND (SELECT count(*) FROM account_transactions b WHERE b.user_id=a.user_id AND b.exchange_id=a.exchange_id AND b.group_id=a.group_id)!=2)",
        params![key.user_id,key.exchange_id,super::sql_time(at-chrono::Duration::seconds(1)),super::sql_time(at+chrono::Duration::seconds(1))],|r|r.get(0))?;
    if partial {return Err(refused("partial existing CSV dust pair"));}
    let mut stmt=c.prepare("SELECT id,group_id FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND transacted_at BETWEEN ?3 AND ?4 AND entry_type=3 AND base_currency=?5 AND base_amount=?6")?;
    let candidates=stmt.query_map(params![key.user_id,key.exchange_id,super::sql_time(at-chrono::Duration::seconds(1)),super::sql_time(at+chrono::Duration::seconds(1)),out.base_currency,ledger::amount_sql(&key.exchange_type,&out.base_amount)],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    if !candidates.is_empty() {
        if candidates.len()!=1 {return Err(refused("ambiguous CSV dust identity"));}
        let (id,Some(group))=&candidates[0] else {return Err(refused("partial existing CSV dust pair"));};
        let mut stmt=c.prepare("SELECT id FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND group_id=?3 ORDER BY id")?;
        let ids=stmt.query_map(params![key.user_id,key.exchange_id,group],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        if ids.len()!=2 || !ids.contains(id) {return Err(refused("partial existing CSV dust pair"));}
        let stored=ids.iter().map(|id|image(c,*id)).collect::<Result<Vec<_>,_>>()?;
        let same=|a:&Value,e:&Entry| {
            let decimal=|k:&str| a[k].as_f64().and_then(|v|BigDec::from_f64(v).ok());
            a["tx_id"]==serde_json::json!(e.tx_id) && a["entry_type"]==e.entry_type && a["base_currency"]==serde_json::json!(e.base_currency) &&
                // Compare both sides through the same double/16-significant-digit readback as storage.
                decimal("base_amount")==BigDec::from_f64(e.base_amount.to_f()).ok() &&
                decimal("fee_amount")==e.fee_amount.as_ref().and_then(|v|BigDec::from_f64(v.to_f()).ok()) &&
                a["fee_currency"]==serde_json::json!(e.fee_currency) && a["transacted_at"]==super::sql_time(at)
        };
        if rows.iter().all(|e|stored.iter().filter(|a|same(a,e)).count()==1) {
            if ids.iter().any(|id|!used.insert(*id)) {return Err(refused("CSV identities are not one-to-one"));}
            return Ok(false);
        }
        return Err(refused("CSV dust identity has no proven whole-pair match"));
    }
    if rows.iter().any(|e|e.tx_id.as_deref().is_some_and(|id|id.contains("a12"))) {return Err(refused("exported identity has no retained binding"));}
    let tuple=serde_json::json!([key.user_id,key.exchange_id,out.group_id,out.base_currency,out.base_amount.to_s_f(),inn.base_amount.to_s_f(),at.timestamp_micros()]).to_string();
    let group=format!("dustcsv_a12_v1_{}",hash(&tuple));
    let occupied:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND group_id=?3)",params![key.user_id,key.exchange_id,group],|r|r.get(0))?;
    if occupied {return Err(refused("CSV group is already occupied"));}
    for e in rows {let mut e=(*e).clone();e.group_id=Some(group.clone());used.insert(insert(c,key,&e,now)?);}
    Ok(true)
}
