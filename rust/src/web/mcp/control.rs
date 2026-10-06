//! MCP control responses adapt BotApi, while bot::write owns every mutation.
use super::{protocol::{tool_text,metadata,validate}, tools};
use crate::web::{App, WebError, bearer::Bearer, bot::{action_params::ActionParams, draft::Draft, mcp_input, write::{self, Action, LifecycleView, Outcome, Prepared}}, layout::Ctx};
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;
use std::sync::Arc;

pub const NAMES: [&str;6]=["stop_bot","archive_bot","unarchive_bot","delete_bot","update_bot_settings","start_bot"];
fn prepared(response: Value) -> Prepared<Value> {Prepared{response,broadcasts:vec![]}}
fn guard_text(ctx:&Ctx,reason:&str)->Value {
    tool_text(&crate::web::i18n::text(ctx.locale,"engine.write_refused", &[("reason",crate::web::i18n::Arg::Text(reason))]),true)
}
fn settings_response(_: &Connection, _: &Ctx, draft: &Draft) -> Result<Prepared<Value>,WebError> {
    let text=if draft.errors.is_empty() {
        format!("Bot '{}' settings updated: {}.",draft.candidate.label,draft.submitted.keys().cloned().collect::<Vec<_>>().join(", "))
    } else if draft.errors.first().is_some_and(|e|e.field=="base") {mcp_input::full_messages(&draft.errors)}
    else {format!("Failed to update bot: {}",mcp_input::full_messages(&draft.errors))};
    Ok(prepared(tool_text(&text,false)))
}
fn lifecycle_response(c:&Connection,_:&Ctx,id:i64,action:Action,view:&LifecycleView)->Result<Prepared<Value>,WebError>{
    let label:String=c.query_row("SELECT COALESCE(label,'') FROM bots WHERE id=?1",[id],|r|r.get(0))?;
    let text=if view.errors.is_empty() {
        match action {
            Action::Stop=>format!("Bot '{label}' stopped."),
            Action::Archive=>format!("Bot '{label}' archived."),
            Action::Unarchive=>format!("Bot '{label}' reactivated (stopped)."),
            Action::Delete=>format!("Bot '{label}' deleted."),
            Action::Start=>format!("Bot '{label}' started successfully."),
        }
    } else {
        let errors=mcp_input::full_messages(&view.errors);
        // Service conflict messages are ordinary successful tool results in Rails.
        if errors.starts_with("Bot '") || errors.starts_with("This app can't run that yet:") {errors} else {match action {
            Action::Start=>format!("Failed to start bot '{label}': {errors}"),
            Action::Stop=>format!("Failed to stop bot '{label}'."),
            Action::Archive=>format!("Failed to archive bot: {errors}"),
            Action::Unarchive=>format!("Failed to reactivate bot: {errors}"),
            Action::Delete=>format!("Failed to delete bot: {errors}"),
        }}
    };
    Ok(prepared(tool_text(&text,false)))
}
// Outcome owns refusal classification. Prepared views carry text only; never infer
// success from an HTTP 200, a shared view, or a guard-message prefix.
fn classified(response:Value,is_error:bool)->Result<Value,WebError> {
    let text=response["content"][0]["text"].as_str().ok_or_else(||WebError::Config("invalid prepared control response".into()))?;
    Ok(tool_text(text,is_error))
}
fn finish(out:Outcome<Value>,ctx:&Ctx)->Result<Value,WebError> {
    match out {
        Outcome::Missing=>Ok(tool_text("Bot not found.",false)), // Rails refusal: ordinary text.
        Outcome::Unported(reason)=>classified(guard_text(ctx,reason),true), // Rust boundary divergence.
        Outcome::GuardRefused(response)=>classified(response,true), // Rust guard/key/fence divergence.
        Outcome::Invalid(response)=>classified(response,false), // Rails validation/conflict transcript.
        Outcome::NoChange(response)=>classified(response,false), // Rails unchanged result.
        Outcome::Committed(response)=>classified(response,false),
    }
}
pub fn call(c:&Connection,app:&App,who:Bearer,name:&str,args:&Value)->Result<Value,WebError>{
    if let Some(refusal)=tools::gate(c,who,name)? {return Ok(refusal);}
    let schema=metadata()["tools"].as_array().into_iter().flatten().find(|t|t["name"]==name).map(|t|&t["inputSchema"]).ok_or_else(||WebError::Config("missing control schema".into()))?;
    let errors=validate(args,schema,"");
    if !errors.is_empty(){return Ok(tool_text(&format!("Invalid input: {}",errors.join(", ")),true));}
    let numeric_id=args["bot_id"].as_f64().ok_or_else(||WebError::Config("validated bot id is not numeric".into()))?;
    // Rails' query treats ids outside SQLite INTEGER range as not found, rather
    // than saturating onto an owned row at the boundary.
    if !(i64::MIN as f64..-(i64::MIN as f64)).contains(&numeric_id) {
        return Ok(tool_text("Bot not found.",false));
    }
    let id=numeric_id as i64;
    let submitted=ActionParams::from_mcp(args.clone()).map_err(|_|WebError::Config("MCP arguments exceed parameter bounds".into()))?;
    // English is ActionMCP's locale; browser cookies, locale and sessions do not authenticate this call.
    let ctx=Ctx{app:app.clone(),params:Arc::new(crate::web::Params{full_path:"/mcp".into(),fullpath:"/mcp".into(),route_path:"/mcp".into(),path_locale:None,query:vec![],form:vec![],json:None}),
        session:crate::web::session::Session::new(Default::default()),current:crate::web::auth::Current::SignedOut,
        method:axum::http::Method::POST,locale:"en",nonce:String::new(),now:app.now(),turbo_frame:None};
    // Unsupported classes have no Draft representation. This is a refusal-only transaction,
    // never a second mutation path, and the same guard supplies its named reason.
    let class:Option<String>=c.query_row("SELECT type FROM bots WHERE id=?1 AND user_id=?2 AND status<>3",(id,who.user_id),|r|r.get(0)).optional()?;
    let Some(class)=class else{return finish(Outcome::Missing,&ctx);};
    if !["Bots::DcaMultiAsset","Bots::DcaIndex"].contains(&class.as_str()) {
        let tx=crate::engine::model::immediate(c)?;
        let response=match crate::engine::eligibility::guard(&tx,&app.cipher,Some(id)) {
            Err(refusal)=>guard_text(&ctx,&refusal.reason()),
            Ok(())=>{
                let bot=crate::engine::model::load_bot(&tx,id)?;
                let reason=crate::engine::eligibility::bot_reasons(&tx,&bot)?.join("; ");
                guard_text(&ctx,&reason)
            },
        };
        tx.rollback()?;
        return finish(Outcome::GuardRefused(response),&ctx);
    }
    let out=if name=="update_bot_settings" { write::settings(c,&ctx,who.user_id,id,&submitted,settings_response)? }
    else {
        let action=match name {"stop_bot"=>Action::Stop,"archive_bot"=>Action::Archive,"unarchive_bot"=>Action::Unarchive,"delete_bot"=>Action::Delete,"start_bot"=>Action::Start,_=>return Err(WebError::Config("unknown control tool".into()))};
        write::lifecycle(c,&ctx,who.user_id,id,action,&submitted,|c,ctx,view|lifecycle_response(c,ctx,id,action,view))?
    };
    finish(out,&ctx)
}
