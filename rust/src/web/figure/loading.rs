//! Network work never owns the database lock. Only raw JSON and request descriptions cross threads.
use super::{account,service::Load};
use crate::figures::{at::At,budget,db,page_market::{Cache,Reader}};
use crate::venue::http::{self,HttpRequest,HttpResponse,ReqwestTransport,Transport,TransportError};
use crate::web::{App,WebError};
use crate::crypto::Credentials;
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;
use sha2::{Digest,Sha256};
use std::time::Duration;

#[derive(Clone,Default)]
pub enum Source { #[default] Live, Disabled, Script(Value) }
#[derive(Clone)]
pub enum Snapshot { Cold, Failed, Ready(Cache,At,u64) }
struct Info { identity:String,revision:u64,credentials:Credentials }
fn revision(c:&Connection)->Result<u64,rusqlite::Error> {
    let version:u64=c.query_row("PRAGMA data_version",[],|r|r.get(0))?;
    Ok(version.wrapping_mul(1_000_000_007).wrapping_add(c.total_changes()))
}
fn read_info(c:&Connection,app:&App,user:i64)->Result<Option<Info>,WebError> {
    let id:Option<i64>=c.query_row("SELECT k.id FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0 AND e.type='Exchanges::Alpaca' ORDER BY k.id LIMIT 1",[user],|r|r.get(0)).optional()?;
    let Some(id)=id else { return Ok(None) };
    let credentials=match crate::sync::credentials(c,&app.cipher,id) { Ok(c)=>c,Err(_)=>return Ok(None) };
    if !matches!(credentials.passphrase.as_deref(),None|Some("paper")) { return Ok(None); }
    let stored:(Option<String>,Option<String>,Option<String>)=c.query_row("SELECT key,secret,passphrase FROM api_keys WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let identity=hex::encode(Sha256::digest(serde_json::json!([user,id,stored.0,stored.1,stored.2]).to_string()));
    let revision=revision(c)?;
    Ok(Some(Info{identity,revision,credentials}))
}
struct Wire { real:ReqwestTransport,source:Source }
impl Transport for Wire {
    async fn send(&self,r:&HttpRequest)->Result<HttpResponse,TransportError> {
        match &self.source {
            Source::Live=>self.real.send(r).await,
            Source::Disabled=>Err(TransportError::Permanent("Market data unavailable".into())),
            Source::Script(script)=>{
                let picked:Vec<_>=r.query.iter().filter(|(k,_)|r.path.ends_with("/bars")&&(*k=="adjustment"||*k=="symbols")).collect();
                let suffix=if picked.is_empty(){String::new()}else{format!("?{}",picked.iter().map(|(k,v)|format!("{k}={v}")).collect::<Vec<_>>().join("&"))};
                let key=format!("GET data.alpaca.markets{}{suffix}",r.path);
                let reply=script.get(&key).ok_or_else(||TransportError::Permanent("Unscripted market request".into()))?;
                if reply["network"].is_string(){return Err(TransportError::NotSent("Market data unavailable".into()));}
                Ok(HttpResponse{status:reply["status"].as_u64().unwrap_or(200) as u16,body:reply["body"].as_str().map(str::to_string).unwrap_or_else(||reply["body"].to_string())})
            }
        }
    }
    async fn send_limited(&self,r:&HttpRequest,limit:usize)->Result<HttpResponse,TransportError>{
        if matches!(self.source,Source::Live){return self.real.send_limited(r,limit).await;}
        let reply=self.send(r).await?;
        if reply.body.len()>limit{Err(TransportError::Permanent("Market data unavailable".into()))}else{Ok(reply)}
    }
}
pub fn symbols(c:&Connection,user:i64)->Result<Vec<String>,crate::figures::FiguresError>{
    let mut names=std::collections::BTreeSet::new();
    for (id,_) in db::account_bots(c,user)? {
        let bot=db::bot(c,id)?;
        for ticker in db::tickers(c,&bot)? { names.insert(ticker.ticker); }
    }
    Ok(names.into_iter().collect())
}
async fn fill(app:&App,user:i64,cache:&mut Cache,wire:&Wire,now:At)->bool {
    for _ in 0..4 {
        let snapshot=cache.clone();
        let pass=app.db(move|c|{
            let result=budget::within(||{
                let names=symbols(c,user)?;
                let reader=Reader::new(&snapshot,now.utc().timestamp()).with_symbols(names);
                let ready=account(c,user,&reader,now,"en","","").is_ok() && !reader.failed();
                Ok::<_,crate::figures::FiguresError>((reader.demands(),ready))
            });
            Ok(result.ok())
        }).await;
        let Ok(Some((demands,ready)))=pass else { return false };
        if demands.is_empty(){return ready;}
        if demands.len()>crate::figures::page_market::MAX_ENTRIES{return false;}
        cache.fill(wire,demands,now.utc().timestamp()).await;
    }
    false
}
pub async fn prepare(app:&App,user:i64)->Result<Snapshot,WebError>{
    if matches!(app.figure_source,Source::Disabled){return Ok(Snapshot::Cold);}
    let inner=app.clone();
    let Some(info)=app.db(move|c|read_info(c,&inner,user)).await? else{return Ok(Snapshot::Failed)};
    let Some(now)=At::from_utc(app.now()) else{return Ok(Snapshot::Failed)};
    match app.figure_service.begin(user,&info.identity,info.revision,now.utc().timestamp()) {
        Load::Cold=>Ok(Snapshot::Cold),Load::Failed=>Ok(Snapshot::Failed),
        Load::Ready(cache,_)=>{let at=cache.stamp().unwrap_or(now);Ok(Snapshot::Ready(cache,at,info.revision))},
        Load::Start(ticket,mut cache)=>{
            let inner=app.clone();let source=app.figure_source.clone();
            tokio::spawn(async move {
                let wire=Wire{real:ReqwestTransport::new(http::client(),info.credentials.key.clone(),info.credentials.secret.clone()),source};
                let ready=tokio::time::timeout(Duration::from_secs(90),fill(&inner,user,&mut cache,&wire,now)).await.unwrap_or(false);
                let check=inner.clone();
                let current=inner.db(move|c|read_info(c,&check,user)).await;
                let unchanged=matches!(current,Ok(Some(ref current)) if current.identity==info.identity && current.revision==info.revision);
                cache.set_stamp(now);
                ticket.finish(cache,ready&&unchanged);
            });
            Ok(Snapshot::Cold)
        }
    }
}
/// Converts a terminal computation failure into absence at every dependent presentation target.
pub fn render(c:&Connection,user:i64,snapshot:&Snapshot,locale:&str,csrf:&str,prefix:&str)->Option<Value>{
    if matches!(snapshot,Snapshot::Cold){return None;}
    if let Snapshot::Ready(cache,now,prepared_revision)=snapshot {
        let computed=budget::within(||{
            let unavailable=||crate::figures::FiguresError::NotComputed("Market data unavailable".into());
            if revision(c)? != *prepared_revision { return Err(unavailable()); }
            let names=symbols(c,user)?;
            let reader=Reader::new(cache,now.utc().timestamp()).with_symbols(names);
            let value=account(c,user,&reader,*now,locale,csrf,prefix)?;
            // Core live calculations may return ledger values after a market failure.
            // Publish only after all reads succeeded against the prepared revision.
            if reader.failed() || !reader.demands().is_empty() || revision(c)? != *prepared_revision {
                return Err(unavailable());
            }
            Ok(value)
        });
        if let Ok(value)=computed{return Some(value);}
    }
    let mut bots=serde_json::Map::new();
    if let Ok(ids)=db::account_bots(c,user){
        for (id,_) in ids {
            let kind=c.query_row("SELECT type FROM bots WHERE id=?1",[id],|r|r.get::<_,String>(0)).unwrap_or_default();
            let kind=if kind=="Bots::DcaIndex"{"dca_index"}else{"dca_multi_asset"};
            bots.insert(id.to_string(),serde_json::json!({"tile":format!("<div id=\"pnl_bots_{kind}_{id}\">{}</div>",super::NO_VALUE),"metrics":format!("<div id=\"metrics\">{}</div>",super::NO_VALUE),"chart":format!("<div id=\"chart\">{}</div>",super::NO_VALUE)}));
        }
    }
    Some(serde_json::json!({"bots":bots,"account":format!("<div id=\"global-pnl\">{}</div>",super::NO_VALUE)}))
}
