//! The redeploy prompt's Yes and No and the halt's Clear, shared by the panel, REST and MCP (BotApi::Bots::AnswerRedeploy).
//! No venue reads in a request: the engine takes a Yes (D-6C-3), and a No is written here, in one transaction (T5).
use crate::{engine::{self,eligibility,model,redeploy},figures::{at::At,db::Subject,walk},web::{bot_api::{self,Domain,Res},layout::Ctx,WebError}};
use super::liquidation::{changed,composition_bot,missing,page_refused};
use rusqlite::Connection;
use serde_json::{json,Value};

/// BotApi::Boolean.parse: only true, false, "true" and "false" answer; anything else is no answer at all.
pub(crate) fn accept(value:&Value)->Option<bool> {
    match value { Value::Bool(b)=>Some(*b), Value::String(s) if s=="true"=>Some(true), Value::String(s) if s=="false"=>Some(false), _=>None }
}

/// #safe_offer: reported, never gated on; nil where the ledger cannot be read.
fn safe_offer(c:&Connection,id:i64,now:chrono::DateTime<chrono::Utc>)->Value {
    let read=crate::figures::budget::within(|| -> Result<String,crate::figures::FiguresError> {
        let s=Subject::load(c,id)?;
        let m=walk::metrics(c,&s,At::from_utc(now).ok_or_else(||crate::figures::FiguresError::Data("invalid time".into()))?)?;
        Ok(redeploy::offer(c,&s,&m)?.to_s_f())
    });
    match read { Ok(offer)=>json!(offer), Err(e)=>{engine::log(&format!("[web] redeploy offer read failed bot={id}: {e:?}"));Value::Null} }
}

pub fn answer(c:&Connection,ctx:&Ctx,user:i64,id:i64,accept:Option<bool>,dry:bool)->Result<Res,WebError> {
    let Some(accept)=accept else {return Ok(Res::failure(Domain::ValidationFailed,"accept_required","accept must be true or false.".into()))};
    let tx=model::immediate(c)?;
    let (bot,label)=match composition_bot(&tx,ctx,user,id,"Only index and basket bots have a redeploy offer.")? {Ok(found)=>found,Err(res)=>return Ok(res)};
    if let Some(res)=page_refused(&tx,ctx,user,id)? {return Ok(res)}
    // Read before the answer is written, as Rails reads it before its job runs.
    let data=json!({"id":id,"label":label,"accepted":accept,"offer":safe_offer(&tx,id,ctx.now),"dry_run":dry});
    if dry {return Ok(Res::success(data))}
    // D-6C-8: a Yes the engine has not finished, or a No while a redeploy works, is refused rather than queued again.
    let in_flight=||Res::failure(Domain::Conflict,"write_refused",crate::web::i18n::text(ctx.locale,"bot.redeploy.in_flight",&[]));
    if accept {
        if !bot.transient[redeploy::REQUEST_KEY].is_null() {
            redeploy::refuse(&tx,id,json!(["in_flight"]),ctx.app.clock.as_ref())?;
            tx.commit()?;
            return Ok(in_flight());
        }
        model::merge_transient_compact(&tx,id,&[(redeploy::REQUEST_KEY,json!({"id":uuid::Uuid::new_v4().to_string(),"at":ctx.now.timestamp()}))])?;
        model::log_activity(&tx,id,"redeploy_requested",model::Level::Info,json!({"user_id":user}),ctx.now)?;
        if let Err(refusal)=eligibility::guard(&tx,&ctx.app.cipher,Some(id)) {return Ok(Res::failure(Domain::Conflict,"write_refused",bot_api::refused(&refusal.reason())))}
        tx.commit()?;
    } else {
        drop(tx);
        if redeploy::decline(c,id,user,ctx.now)?==redeploy::Declined::Refused {return Ok(in_flight())}
    }
    changed(c,ctx,id);
    Ok(Res::success(data))
}

use axum::{extract::{Extension,Path,State},http::{HeaderMap,StatusCode},response::{IntoResponse,Response}};
use crate::web::{auth,flash,i18n,layout,turbo,App};
use super::{action_params::ActionParams,action_view};

/// BotHelper#redeploy_answer_spinner: where Yes and No stood, until the engine's repaint lands.
fn spinner(label:&str)->String { format!("<span class=\"loader--small\" role=\"status\" aria-label=\"{}\"></span>",i18n::escape(label)) }

async fn respond(app:App,ctx:Ctx,segment:String,headers:HeaderMap,accept:bool)->Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(segment.strip_suffix(".turbo_stream").unwrap_or(&segment)) else {return Ok(missing(&ctx))};
    let view=ctx.clone();
    let res=match app.db(move |c|answer(c,&view,user.id,id,Some(accept),false)).await {Ok(r)=>r,Err(e)=>return layout::or_refused(&ctx,e)};
    if res.status==Domain::NotPorted {return Ok(layout::refused(&ctx,&res.message))}
    if res.status==Domain::NotFound {return Ok(missing(&ctx))}
    if res.code==Some("not_composition_bot") {
        flash::set(&ctx.session,flash::ALERT,i18n::text(ctx.locale,"bot.redeploy.unsupported",&[]));
        return Ok(layout::redirect(StatusCode::FOUND,&layout::back(&app.config,&headers).unwrap_or_else(||ctx.path("/bots"))));
    }
    if res.code.is_some() {
        let alert=flash::render(&flash::take(&ctx.session,&[(flash::ALERT,res.message)]))?;
        return Ok(action_view::response(turbo::prepend_flash(alert.trim_end()),true));
    }
    let text=i18n::text(ctx.locale,if accept {"bot.redeploy.started"} else {"bot.redeploy.declined"},&[]);
    let notice=flash::render(&flash::take(&ctx.session,&[(flash::NOTICE,text.clone())]))?;
    Ok(action_view::response(turbo::prepend_flash(notice.trim_end())+&turbo::stream("update","redeploy-prompt-actions",&spinner(&text)),false))
}
pub async fn create(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>,headers:HeaderMap)->Result<Response,WebError> {
    respond(app,ctx,segment,headers,true).await
}
pub async fn destroy(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>,headers:HeaderMap)->Result<Response,WebError> {
    respond(app,ctx,segment,headers,false).await
}

/// Bots::RedeployResolutionsController#create. D-6C-2: refused while a REDEPLOY row waits, and resolved here, in the
/// request, under the lock recovery takes; a page older than the halt it shows clears nothing.
pub async fn clear(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>,headers:HeaderMap)->Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(&segment) else {return Ok(missing(&ctx))};
    let Ok(params)=ActionParams::parse_rails(&ctx.params) else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    let view=ctx.clone();
    let result=app.db(move |c| {
        let found:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id=?1 AND user_id=?2 AND status<>3 AND type IN ('Bots::DcaIndex','Bots::DcaMultiAsset'))",(id,user.id),|r|r.get(0))?;
        if !found {return Ok(None)};
        let nonce=crate::web::mcp::tools::str_value(&params.value()["intent_id"]);
        let waiting=redeploy::resolve(c,id,user.id,&nonce,view.now)?;
        changed(c,&view,id);
        Ok(Some(waiting))
    }).await;
    match result {
        Ok(None)=>Ok(missing(&ctx)),
        Ok(Some(waiting))=>{
            let (style,text)=match waiting {
                Some(order)=>(flash::ALERT,i18n::text(ctx.locale,"bot.redeploy.still_open",&[("order_id",i18n::Arg::Text(&order))])),
                None=>(flash::NOTICE,i18n::text(ctx.locale,"bot.redeploy.resolved",&[])),
            };
            flash::set(&ctx.session,style,text);
            Ok(layout::redirect(StatusCode::FOUND,&layout::back(&app.config,&headers).unwrap_or_else(||ctx.path(&format!("/bots/{id}")))))
        }
        Err(e)=>layout::or_refused(&ctx,e),
    }
}
