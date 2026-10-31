//! Explicit liquidation requests shared by the modal, REST and MCP. No venue reads in a request.
use crate::{engine::{self,eligibility,model},figures::{at::At,db::Subject,walk},web::{bot_api::{self,Domain,Res},layout::Ctx,WebError}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::{json,Value};
type Holdings=Vec<(String,i64)>;

pub(crate) fn strings(value:&Value)->Vec<String> {
    match value { Value::Null=>vec![],Value::Array(a)=>a.iter().map(crate::web::mcp::tools::str_value).collect(),v=>vec![crate::web::mcp::tools::str_value(v)] }
}
/// Rails held_assets: ledger amounts, known assets and the venue base minimum; no price dependency.
pub(crate) fn held(c:&Connection,id:i64,now:chrono::DateTime<chrono::Utc>)->Result<Holdings,WebError> {
    crate::figures::budget::within(|| {
        let s=Subject::load(c,id).map_err(super::fill_error)?;
        let m=walk::metrics(c,&s,At::from_utc(now).ok_or_else(||super::data("invalid time".into()))?).map_err(super::fill_error)?;
        let bot=model::load_bot(c,id)?;
        let mut out=vec![];
        for (key,amount) in &m.asset_breakdown {
            let Some(asset)=m.key_assets.iter().find(|(k,_)|k==key).and_then(|(_,a)|*a) else {continue};
            let Some(ticker)=model::ticker_for_asset(c,&bot,asset)? else {continue};
            if crate::ruby::BigDec::parse(&amount.amount.to_d().map_err(|e|super::fill_error(e.into()))?.to_s_f()).map_err(|e|super::data(format!("{e:?}")))? >= ticker.minimum_base_size {out.push((key.clone(),asset));}
        }
        Ok(out)
    })
}

pub(crate) fn selected(c:&Connection,held:&[(String,i64)],params:&Value,web:bool)->Result<Result<Holdings,Res>,WebError> {
    let mut identifiers=strings(&params["symbol"]); let expected=strings(&params["asset_id"]);
    if !web {
        let mut unique=vec![];
        for name in identifiers {let name=super::mcp_input::strip(&name).to_string();if !unique.contains(&name) {unique.push(name);}}
        identifiers=unique;
    }
    let mut selected=vec![];
    for (index,name) in identifiers.iter().enumerate() {
        let name=name.as_str();
        let mut matches:Vec<_>=held.iter().filter(|(key,asset)|key==name || (!web && asset.to_string()==name)).cloned().collect();
        if !web && matches.is_empty() {
            for (key,asset) in held {
                let symbol:Option<String>=c.query_row("SELECT symbol FROM assets WHERE id=?1",[asset],|r|r.get(0))?;
                if symbol.is_some_and(|s|s.eq_ignore_ascii_case(name)) {matches.push((key.clone(),*asset));}
            }
        }
        if matches.len()>1 { return Ok(Err(Res::failure(Domain::ValidationFailed,"holding_ambiguous",format!("{name} names more than one holding: {}. Use one of those.",matches.iter().map(|(k,_)|k.as_str()).collect::<Vec<_>>().join(", "))))); }
        matches.retain(|(_,asset)|expected.get(index).is_none_or(|e|e.trim().is_empty() || *e==asset.to_string()));
        if matches.is_empty() && !web {
            let keys=held.iter().map(|(k,_)|k.as_str()).collect::<Vec<_>>().join(", ");
            return Ok(Err(Res::failure(Domain::NotFound,"holding_not_held",format!("{name} is not a position this bot holds. Held: {}.",if keys.is_empty(){"none"}else{&keys}))));
        }
        for h in matches {if !selected.contains(&h) {selected.push(h);}}
    }
    Ok(if selected.is_empty() {Err(Res::failure(Domain::NotFound,"holding_not_held","No symbol is a position this bot holds.".into()))}else{Ok(selected)})
}

/// The service's lookup: an owned, undeleted composition bot with its label, refused when archived. `unsupported` is the
/// service's own not_composition_bot message.
pub(super) fn composition_bot(tx:&Connection,ctx:&Ctx,user:i64,id:i64,unsupported:&str)->Result<Result<(model::Bot,String),Res>,WebError> {
    let found:Option<(String,String,i64)>=tx.query_row("SELECT type,COALESCE(label,''),status FROM bots WHERE id=?1 AND user_id=?2 AND status<>3",(id,user),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((class,label,status))=found else {return Ok(Err(Res::failure(Domain::NotFound,"bot_not_found","Bot not found.".into())))};
    if !["Bots::DcaIndex","Bots::DcaMultiAsset"].contains(&class.as_str()) {return Ok(Err(Res::failure(Domain::ValidationFailed,"not_composition_bot",unsupported.into())))}
    let bot=model::load_bot(tx,id)?;
    let label=if label.trim().is_empty(){crate::web::mcp::reads::generated_label(tx,user,id,&class,&bot.settings,ctx.locale)?}else{label};
    if status==7 {return Ok(Err(Res::failure(Domain::Conflict,"bot_archived",format!("Bot '{label}' is archived; reactivate it first."))))}
    Ok(Ok((bot,label)))
}

/// D-6C-7: the page's own refusal for every surface.
pub(super) fn page_refused(tx:&Connection,ctx:&Ctx,id:i64)->Result<Option<Res>,WebError> {
    let (provider,_)=crate::web::bots::market_data(tx,&ctx.app)?;
    Ok(super::refusal(tx,id,provider,super::For::Settings)?.map(|reason|Res::failure(Domain::NotPorted,"not_ported",bot_api::refused(reason))))
}

pub fn request(c:&Connection,ctx:&Ctx,user:i64,id:i64,params:&Value,web:bool)->Result<Res,WebError> {
    let tx=model::immediate(c)?;
    let (bot,label)=match composition_bot(&tx,ctx,user,id,"Only index and basket bots hold sellable positions.")? {Ok(found)=>found,Err(res)=>return Ok(res)};
    if engine::composition::in_flight(&tx,&bot,ctx.now)? {
        engine::liquidation::refuse(&tx,id,json!(["in_flight"]),ctx.app.clock.as_ref())?;
        tx.commit()?;
        return Ok(Res::failure(Domain::Conflict,"write_refused",i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(&i18n::text(ctx.locale,"errors.bots.index_switch.in_flight",&[])))])));
    }
    if let Some(res)=page_refused(&tx,ctx,id)? {return Ok(res)}
    let holdings=match selected(&tx,&held(&tx,id,ctx.now)?,params,web)? {Ok(h)=>h,Err(r)=>return Ok(r)};
    let dry=!web && params.get("dry_run").and_then(Value::as_bool).unwrap_or(crate::web::mcp::tools::paper_trading(&tx,user)?);
    let symbols:Vec<_>=holdings.iter().map(|(key,_)|key.clone()).collect();
    if !dry {
        // Bot#mark_selling! uses Hash#compact on the merged transient data.
        let mut changes:Vec<_>=bot.transient.as_object().into_iter().flatten().filter(|(_,v)|v.is_null()).map(|(k,_)|(k.as_str(),Value::Null)).collect();
        changes.push((engine::liquidation::REQUEST_KEY,json!({"id":uuid::Uuid::new_v4().to_string(),"at":ctx.now.timestamp(),"holdings":holdings})));
        model::merge_transient_compact(&tx,id,&changes)?;
        model::log_activity(&tx,id,"liquidation_requested",model::Level::Info,json!({"user_id":user,"base":symbols.join(", ")}),ctx.now)?;
    }
    if let Err(refusal)=eligibility::guard(&tx,&ctx.app.cipher,Some(id)) {return Ok(Res::failure(Domain::Conflict,"write_refused",bot_api::refused(&refusal.reason())))}
    tx.commit()?;
    if !dry {changed(c,ctx,id);}
    Ok(Res::success(json!({"id":id,"label":label,"symbol":symbols.join(", "),"symbols":symbols,"dry_run":dry})))
}

use axum::{extract::{Extension,Path,State},http::StatusCode,response::{IntoResponse,Response}};
use crate::web::{auth,flash,i18n,layout::{self,Page},shell::{self,Shell},App};
use super::{action_params::ActionParams,action_view,page};
use askama::Template;

#[derive(Template)]
#[template(path="bots/liquidation.html")]
struct Modal {
    title:String,confirm:String,partial:String,table:String,harvest:bool,action:String,csrf:String,
    cancel:String,sell:String,selling:String,colour:&'static str,
    /// bots/wash_sale_prompts/_question, for an account that has not answered: it answers in the same submit (Sell is dead
    /// until it does).
    question:String,decided:bool,
}

pub(super) fn missing(ctx:&Ctx)->Response {
    (StatusCode::NOT_FOUND,[(axum::http::header::CONTENT_TYPE,if ctx.stream {"text/vnd.turbo-stream.html"}else{"text/html"})]).into_response()
}
pub async fn new(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>)->Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(&segment) else {return Ok(missing(&ctx))};
    let Ok(params)=ActionParams::parse_rails(&ctx.params) else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    let (inner,view,owner)=(app.clone(),ctx.clone(),user.clone());
    let result=app.db(move |c| {
        let tx=c.unchecked_transaction()?;
        let (found,_)=page::find(&tx,&inner,&owner,&segment,false,super::For::Settings,view.locale)?;
        let page::Found::Bot(bot)=found else {return Ok(Err(found))};
        let selected=match selected(&tx,&held(&tx,id,view.now)?,params.value(),true)? {Ok(h)=>h,Err(_)=>return Ok(Err(page::Found::Missing))};
        let (table,harvest,exited)=crate::web::figure::liquidation_table(&tx,&view,id,&selected).map_err(super::fill_error)?;
        let mut query=form_urlencoded::Serializer::new(String::new());
        // Rails route helper sorts the keys: asset_id, then symbol.
        for (_,asset) in &selected {query.append_pair("asset_id[]",&asset.to_string());}
        for (key,_) in &selected {query.append_pair("symbol[]",key);}
        let t=|key:&str|i18n::text(view.locale,key,&[]);
        let csrf=view.csrf_token();
        let (enabled,jurisdiction):(Option<bool>,Option<String>)=tx.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1",[owner.id],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let question=if enabled.is_none() {crate::web::settings::view::question(&view,None,jurisdiction.as_deref())} else {String::new()};
        let body=Modal {
            question,decided:enabled.is_some(),
            title:t(if exited {if bot.kind==super::Kind::Index {"bot.dca_index.out_of_the_index"}else{"bot.dca_multi_asset.removed_from_portfolio"}}else{"bot.liquidation.title"}),
            confirm:t(if selected.len()>1 {"bot.liquidation.sell_all_confirm"}else{"bot.liquidation.sell_confirm"}),
            partial:t("bot.liquidation.partial_note"),table,harvest,action:format!("{}?{}",view.path(&format!("/bots/{id}/liquidation")),query.finish()),
            csrf:csrf.clone(),cancel:t("button.cancel"),sell:t("bot.liquidation.sell"),selling:t("bot.liquidation.selling"),
            colour:if selected.len()==1 && harvest {"rbutton--success"}else{"rbutton--danger"},
        }.render()?;
        let shell=Shell::load(&tx,&inner,&owner)?;
        tx.commit()?;
        Ok(Ok((body,csrf,shell)))
    }).await;
    match result {
        Ok(Ok((body,csrf,shell)))=>shell::application(&ctx,&csrf,&user,&shell,Page {status:StatusCode::OK,body,flash_now:vec![]}),
        Ok(Err(page::Found::NotPorted(reason)))=>Ok(layout::refused(&ctx,reason)),
        Ok(Err(_))=>Ok(missing(&ctx)),
        Err(e)=>layout::or_refused(&ctx,e),
    }
}
pub async fn create(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>)->Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(segment.strip_suffix(".turbo_stream").unwrap_or(&segment)) else {return Ok(missing(&ctx))};
    let Ok(params)=ActionParams::parse_rails(&ctx.params) else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    // LiquidationsController#wash_sale_answer_recorded?: this sale can start a window, so an account that has not answered
    // records its answer before the order is queued; no answer, or one not saved, refuses the sale. The bot and the symbols
    // are judged first (set_bot, set_symbols).
    let (view,values)=(ctx.clone(),params.value().clone());
    // `bots.find` includes a deleted bot: its answer is recorded, and the service then refuses the bot.
    // Some(false): decided, the request answers as before. None: undecided and nothing named is held (set_symbols ends the
    // request there, so no later read may let the sale through unasked), or not the user's composition bot.
    let undecided=app.db(move |c| {
        let enabled:Option<bool>=c.query_row("SELECT wash_sale_enabled FROM users WHERE id=?1",[user.id],|r|r.get(0))?;
        if enabled.is_some() {return Ok(Some(false))}
        let found:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id=?1 AND user_id=?2 AND type IN ('Bots::DcaIndex','Bots::DcaMultiAsset'))",(id,user.id),|r|r.get(0))?;
        if !found {return Ok(Some(false))}
        Ok(selected(c,&held(c,id,view.now)?,&values,true)?.is_ok().then_some(true))
    }).await;
    match undecided {
        Ok(None) => return Ok(missing(&ctx)),
        Ok(Some(true)) => {
            use crate::web::settings::tax::{self,Decision};
            let refused=||tax::refuse(&ctx,i18n::text(ctx.locale,"settings.wash_sale.prompt_missing",&[]));
            let Some(answer)=ctx.params.form("wash_sale[enabled]").filter(|s|!crate::ruby::blank(s)) else {return refused()};
            let enabled=!["0","f","F","false","FALSE","off","OFF"].contains(&answer);
            let jurisdiction=ctx.params.form("wash_sale[jurisdiction]").filter(|s|!crate::ruby::blank(s)).map(str::to_string);
            let (jobs,inner)=(app.job_wakers()?,app.clone());
            match app.db(move |c|tax::record_decision(c,&inner,&jobs,user.id,enabled,jurisdiction)).await? {
                Decision::Saved=>{}
                Decision::Invalid=>return refused(),
                Decision::Refused(reason)=>return tax::refuse(&ctx,i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(&reason))])),
            }
        }
        Ok(_)=>{}
        Err(e)=>return layout::or_refused(&ctx,e),
    }
    let view=ctx.clone();
    let res=match app.db(move |c|request(c,&view,user.id,id,params.value(),true)).await {Ok(r)=>r,Err(e)=>return layout::or_refused(&ctx,e)};
    if res.status==Domain::NotPorted {return Ok(layout::refused(&ctx,&res.message))}
    if res.status==Domain::NotFound {return Ok(missing(&ctx))}
    let failed=res.code.is_some();
    let text=if failed {res.message}else{i18n::text(ctx.locale,if res.data["symbols"].as_array().is_some_and(|a|a.len()>1) {"bot.liquidation.started_all"}else{"bot.liquidation.started"},&[])};
    Ok(action_view::response(crate::web::turbo::prepend_flash(flash::render(&[flash::Message {style:if failed {"danger"}else{"primary"},text}])?.trim_end()),failed))
}
pub async fn clear(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>,headers:axum::http::HeaderMap)->Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(&segment) else {return Ok(missing(&ctx))};
    let Ok(params)=ActionParams::parse_rails(&ctx.params) else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    let view=ctx.clone();
    let result=app.db(move |c| {
        let found:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id=?1 AND user_id=?2 AND status<>3 AND type IN ('Bots::DcaIndex','Bots::DcaMultiAsset'))",(id,user.id),|r|r.get(0))?;
        if !found {return Ok(None)};
        let nonce=params.value()["intent_id"].as_str().unwrap_or("");
        let ids=strings(&params.value()["order_ids"]).iter().map(|s|bot_api::to_i(&json!(s)).unwrap_or(0)).collect::<Vec<_>>();
        let waiting=engine::liquidation::resolve(c,id,user.id,nonce,&ids,view.now)?;
        changed(c,&view,id);
        Ok(Some(waiting))
    }).await;
    match result {
        Ok(None)=>Ok(missing(&ctx)),
        Ok(Some(waiting))=>{
            let (style,text)=if let Some(order)=waiting {(flash::ALERT,i18n::text(ctx.locale,"bot.liquidation.still_open",&[("order_id",i18n::Arg::Text(&order))]))}
                else {(flash::NOTICE,i18n::text(ctx.locale,"bot.liquidation.resolved",&[]))};
            flash::set(&ctx.session,style,text);
            Ok(layout::redirect(StatusCode::FOUND,&layout::back(&app.config,&headers).unwrap_or_else(||ctx.path(&format!("/bots/{id}")))))
        }
        Err(e)=>layout::or_refused(&ctx,e),
    }
}


/// Render after commit from the current row: a request that already finished cannot repaint an old spinner.
pub(super) fn changed(c:&Connection,ctx:&Ctx,id:i64) {
    ctx.app.wake_engine();ctx.app.figure_service.mark(id);
    match super::live::render(c,&ctx.app,&engine::events::EngineEvent::TickEnded {bot_id:id}) {
        Ok(messages)=>for (stream,html) in messages {ctx.app.hub.broadcast(&stream,&html);},
        Err(e)=>engine::log(&format!("[web] composition status bot={id}: {e:?}")),
    }
}
