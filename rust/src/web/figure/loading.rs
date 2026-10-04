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
    // A failure to read the key is the read's failure: the caller's retry keeps the demand. Only an undecryptable key
    // means no figures (this install's configuration, not a passing fault).
    let stored:(Option<String>,Option<String>,Option<String>)=c.query_row("SELECT key,secret,passphrase FROM api_keys WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let credentials=match crate::sync::decrypted(&app.cipher,id,stored.clone()) { Ok(c)=>c,Err(_)=>return Ok(None) };
    if !matches!(credentials.passphrase.as_deref(),None|Some("paper")) { return Ok(None); }
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
                // A test holds an answer back to keep a fill in flight; the live source never reads a script.
                if let Some(ms)=reply["delay_ms"].as_u64(){tokio::time::sleep(Duration::from_millis(ms)).await;}
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
            Ok(result.ok()) // allow-swallow: a computation that fails is a failed fill (no value, retried after its minute)
        }).await;
        let Ok(Some((demands,ready)))=pass else { return false };
        if demands.is_empty(){return ready;}
        if demands.len()>crate::figures::page_market::MAX_ENTRIES{return false;}
        cache.fill(wire,demands,now.utc().timestamp()).await;
    }
    false
}
pub async fn prepare(app:&App,user:i64)->Result<Snapshot,WebError>{
    begin(app,user).await
}
/// Boxed, so that its type is known: the end of a fill publishes, and publishing begins a fill.
type Begun<'a>=std::pin::Pin<Box<dyn std::future::Future<Output=Result<Snapshot,WebError>>+Send+'a>>;
fn begin(app:&App,user:i64)->Begun<'_>{ Box::pin(async move {
    if matches!(app.figure_source,Source::Disabled){return Ok(Snapshot::Cold);}
    let inner=app.clone();
    let Some(info)=app.db(move|c|read_info(c,&inner,user)).await? else{return Ok(Snapshot::Failed)};
    let Some(now)=At::from_utc(app.now()) else{return Ok(Snapshot::Failed)};
    match app.figure_service.begin(user,&info.identity,info.revision,now.utc().timestamp()) {
        Load::Cold=>Ok(Snapshot::Cold),Load::Failed=>Ok(Snapshot::Failed),
        // Other accounts hold both fills: the demand is kept, and the end of one of theirs serves it.
        Load::Busy=>{app.figure_service.want(user);Ok(Snapshot::Cold)},
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
                ticket.finish(cache,ready&&unchanged,inner.now().timestamp());
                // Whoever waits and has no fill running hears the end of this one: this account, or one the two-fill
                // limit turned away. A fill a write overtook is not published: publishing starts the next fill, and each
                // fill a waiting account starts spends one of its own allowance (`service::ALLOWANCE`), so failures and
                // writes end in a publication, not a loop. Waiting behind another account spends nothing.
                // ponytail: every waiting account per fill end; one queue of accounts if installs grow past a handful.
                // A publication that fails here (a transient database error) is handed to the `figures` service, which tries
                // it again with its backoff (`follow`).
                for waiting in inner.figure_service.waiting_idle() {
                    if publish_with(inner.clone(),waiting).await.is_err() { inner.figure_service.mark_owner(waiting); }
                }
            });
            Ok(Snapshot::Cold)
        }
    }
})}
/// Converts a terminal computation failure into absence at every dependent presentation target.
/// `Ok(None)` while the figures are cold; an error only when the fallback cannot read the account's bots.
pub fn render(c:&Connection,user:i64,snapshot:&Snapshot,locale:&str,csrf:&str,prefix:&str)->Result<Option<Value>,WebError>{
    if matches!(snapshot,Snapshot::Cold){return Ok(None);}
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
        if let Ok(value)=computed{return Ok(Some(value));}
    }
    // The fallback is every bot's no value, or nothing: a failure to read the bots fails the render, never publishes the
    // headline alone.
    let mut bots=serde_json::Map::new();
    let ids=db::account_bots(c,user).map_err(|e|WebError::Task(format!("figures fallback: {e:?}")))?;
    for (id,kind) in ids {
        let kind=if kind.as_deref()==Some("Bots::DcaIndex"){"dca_index"}else{"dca_multi_asset"};
        bots.insert(id.to_string(),serde_json::json!({"tile":format!("<div id=\"pnl_bots_{kind}_{id}\">{}</div>",super::NO_VALUE),"metrics":format!("<div id=\"metrics\">{}</div>",super::NO_VALUE),"chart":format!("<div id=\"chart\">{}</div>",super::NO_VALUE)}));
    }
    Ok(Some(serde_json::json!({"bots":bots,"account":format!("<div id=\"global-pnl\">{}</div>",super::NO_VALUE)})))
}
/// Puts the user's figures on their streams (`figure::streams`): every tile, each bot page's metrics and chart, and the
/// account headline. Each is rendered as Rails renders a broadcast: in the user's locale and under its paths
/// (Bot#with_user_locale, ApplicationController#default_url_options), with no token in a form (the renderer has no
/// session). Rails broadcasts what was asked for; one publication carries every target, each payload Rails' own.
/// While a fill is in flight nothing is published: the user is noted as waiting, and the fill's end publishes.
pub async fn publish(app:&App,user:i64)->Result<(),WebError>{
    publish_with(app.clone(),user).await
}
async fn publish_with(app:App,user:i64)->Result<(),WebError>{
    // Ready figures are held against the account as it is once rendered, just before delivery: a publication whose
    // figures expired, were written or were refilled meanwhile delivers nothing, and is tried once more, which finds them
    // ready anew or starts a fill whose end publishes.
    for _ in 0..2 {
        let snapshot=begin(&app,user).await?;
        if matches!(snapshot,Snapshot::Cold){app.figure_service.want(user);return Ok(());}
        let serial=app.figure_service.settled(user);
        let ready=matches!(snapshot,Snapshot::Ready(..));
        let inner=app.clone();
        let (streams,info)=app.db(move|c|{
            let chosen:Option<String>=c.query_row("SELECT locale FROM users WHERE id=?1",[user],|r|r.get(0)).optional()?.flatten();
            let locale=crate::web::locale::switch(None,chosen.as_deref());
            let streams=match render(c,user,&snapshot,locale,"",&crate::web::locale::path(locale,""))? {
                // A failure here fails the publication: nothing is recorded or delivered, and the caller's retry keeps the demand.
                Some(rendered)=>super::streams(c,user,&rendered).map_err(|e|WebError::Task(format!("figure streams: {e:?}")))?,
                None=>vec![],
            };
            let info=if ready { read_info(c,&inner,user)? } else { None };
            Ok((streams,info))
        }).await?;
        if ready {
            let fresh=info.is_some_and(|info|app.figure_service.record(user,serial,&info.identity,info.revision,app.now().timestamp(),&streams));
            if !fresh { continue; }
        }
        app.figure_service.served(user);
        deliver(&app,streams).await;
        return Ok(());
    }
    // Overtaken twice: the account waits, and the end of the next fill publishes.
    app.figure_service.want(user);
    Ok(())
}

/// Puts payloads on the hub. Each connection's mailbox keeps the latest payload of every target (`cable::Mailbox`), so
/// a large account's publication neither waits on a slow connection nor pushes a healthy one off.
pub async fn deliver(app:&App,streams:Vec<(String,String)>){
    for (stream,html) in streams { app.hub.broadcast(&stream,&html); }
}

/// What a connection that (re)subscribes to one of `user`'s figure streams is sent: the account's latest publication on
/// it while its figures are fresh (`Service::latest`). Otherwise nothing, and one publication is asked for, as a page
/// asks (`publish`): ready figures go out at once, and otherwise the account waits, through the fill and allowance
/// machinery, and the fill's end publishes. So a page that stays mounted converges after any reconnect: after a
/// dropped publication, after its figures expired or were written, after the account was evicted, after a restart.
/// An account with no figures (no paper Alpaca key, or market data disabled) is sent and asks nothing.
pub async fn resubscribed(app:&App,user:i64,stream:&str)->Vec<std::sync::Arc<str>>{
    if matches!(app.figure_source,Source::Disabled){return Vec::new();}
    let inner=app.clone();
    // A failure to read the account (a transient database error) asks for the publication through the retry path.
    let info=match app.db(move|c|read_info(c,&inner,user)).await { Ok(info)=>info, Err(_)=>{ app.figure_service.mark_owner(user); return Vec::new(); } };
    let Some(info)=info else { return Vec::new() };
    if let Some(kept)=app.figure_service.latest(user,&info.identity,info.revision,app.now().timestamp(),stream) { return kept; }
    let app=app.clone();
    tokio::spawn(async move { if publish_with(app.clone(),user).await.is_err() { app.figure_service.mark_owner(user); } });
    Vec::new()
}

/// How long the `figures` service gathers marks before it publishes: a basket's legs and a sweep's fills come together.
const COALESCE:Duration=Duration::from_millis(500);
/// A publication that failed (a transient database error) is tried again after `BACKOFF`, doubling, at most `RETRIES`
/// times in a row: about eight seconds in all.
const BACKOFF:Duration=Duration::from_millis(250);
const RETRIES:u32=5;
/// The `figures` service of `deltabadger serve`: an order the engine recorded or wrote changes its bot's figures, and
/// Rails then broadcasts them (Bot::UpdateMetricsJob). Two parts in one future. One reads the engine's queue as fast as
/// it fills and only marks the bot (`Service::mark`: O(1), one mark per bot, never waiting), so nothing accumulates
/// there while a publication runs. The other wakes on a mark, gathers the burst for `COALESCE`, and publishes each
/// owner once. A write moved the revision, so a publication starts a fill and its end publishes. Returns once a stop is
/// requested; an engine that went away leaves it waiting for that stop.
pub async fn follow(app:App,mut events:tokio::sync::mpsc::UnboundedReceiver<crate::engine::events::EngineEvent>,mut stop:tokio::sync::watch::Receiver<bool>)->Result<(),String>{
    use crate::engine::events::EngineEvent;
    let service=app.figure_service.clone();
    let marking=async {
        while let Some(event)=events.recv().await {
            if let EngineEvent::OrderRecorded{bot_id,..}|EngineEvent::OrderUpdated{bot_id,..}=event { service.mark(bot_id); }
        }
    };
    let publishing=async {
        let mut failures=0u32;
        loop {
            service.marked_wait().await;
            tokio::time::sleep(COALESCE).await;
            let bots=service.take_marked();
            let mut owners=service.take_owners();
            if bots.is_empty() && owners.is_empty() { continue; }
            // A failed lookup or publication keeps its bots or its account marked: they are tried again after a backoff.
            let mut failed_bots=Vec::new();
            if !bots.is_empty() {
                let ids=serde_json::json!(bots).to_string();
                let found=app.db(move|c|{
                    let mut s=c.prepare("SELECT DISTINCT user_id FROM bots WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY user_id")?;
                    let users=s.query_map([ids],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
                    Ok(users)
                }).await;
                match found { Ok(users)=>owners.extend(users), Err(_)=>failed_bots.extend(bots) }
            }
            let mut failed_owners=Vec::new();
            for user in owners { if publish(&app,user).await.is_err() { failed_owners.push(user); } }
            if failed_bots.is_empty() && failed_owners.is_empty() { failures=0; continue; }
            failures+=1;
            // ponytail: RETRIES attempts, then the demand waits for the next order or page request, as Rails' job would.
            if failures>RETRIES { failures=0; continue; }
            tokio::time::sleep(BACKOFF*2u32.pow(failures-1)).await;
            for bot in failed_bots { service.mark(bot); }
            for user in failed_owners { service.mark_owner(user); }
        }
    };
    let engine_gone=tokio::select!{
        _=stop.wait_for(|stopped|*stopped)=>false,
        _=marking=>true,
        _=publishing=>false,
    };
    if engine_gone { let _=stop.wait_for(|stopped|*stopped).await; } // allow-swallow: an error means no stop can come, and returning is the stop
    Ok(())
}
