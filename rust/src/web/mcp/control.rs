//! MCP control responses adapt BotApi (web::bot_api::bot_write), while bot::write owns every mutation.
use super::{protocol::{tool_text,metadata,validate}, tools};
use crate::web::{App, WebError, bearer::Bearer, bot::{action_params::ActionParams, write::Outcome}, bot_api::{self, Shown, Write}};
use rusqlite::Connection;
use serde_json::Value;

pub const NAMES: [&str;7]=["stop_bot","archive_bot","unarchive_bot","delete_bot","update_bot_settings","start_bot","liquidate_exited_asset"];
// Outcome owns refusal classification. The shown text is the answer only; never infer
// success from an HTTP 200, a shared view, or a guard-message prefix.
fn finish(out:Outcome<Shown>)->Value {
    match out {
        Outcome::Missing=>tool_text("Bot not found.",true), // BotApi failure: report_error.
        Outcome::Unported(reason)=>tool_text(&bot_api::refused(reason),true), // Rust boundary divergence.
        Outcome::GuardRefused(shown)=>tool_text(&shown.text,true), // Rust guard/key/fence divergence.
        Outcome::Invalid(shown,_)=>tool_text(&shown.text,true), // BotApi validation/conflict failure: report_error.
        Outcome::NoChange(shown)=>tool_text(&shown.text,false), // Rails unchanged result.
        Outcome::Committed(shown)=>tool_text(&shown.text,false),
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
        return Ok(tool_text("Bot not found.",true));
    }
    let id=numeric_id as i64;
    let write=Write::tool(name).ok_or_else(||WebError::Config("unknown control tool".into()))?;
    let arguments=if write==Write::Liquidate {serde_json::json!({"symbol":args["symbol"],"dry_run":tools::paper_trading(c,who.user_id)?})}else{args.clone()};
    let submitted=ActionParams::from_mcp(arguments).map_err(|_|WebError::Config("MCP arguments exceed parameter bounds".into()))?;
    Ok(finish(bot_api::bot_write(c,app,who.user_id,id,write,&submitted)?))
}
/// D-SPLIT-API: split_bots, the web's split through bot_api::split. Success is the data as JSON text; a refusal (and a split
/// this build does not serve) is an error whose JSON text holds the stable code, the message and the unanswered offers.
pub fn split(c:&Connection,app:&App,user:i64,args:&Value)->Result<Value,WebError>{
    let res=bot_api::split(c,app,user,args.get("bot_ids"),args.get("keep_proceeds_for"))?;
    Ok(match res.code {
        None=>tool_text(&crate::web::rest::encode(&res.data),false),
        Some(code)=>tool_text(&crate::web::rest::encode(&serde_json::json!({"code":code,"message":res.message,"offers":res.data.get("offers").cloned().unwrap_or(serde_json::json!([]))})),true),
    })
}
