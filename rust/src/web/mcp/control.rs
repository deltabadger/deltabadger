//! MCP control responses adapt BotApi (web::bot_api::bot_write), while bot::write owns every mutation.
use super::{protocol::{tool_text,metadata,validate}, tools};
use crate::web::{App, WebError, bearer::Bearer, bot::{action_params::ActionParams, write::Outcome}, bot_api::{self, Shown, Write}};
use rusqlite::Connection;
use chrono::TimeZone as _;
use serde_json::Value;

pub const NAMES: [&str;8]=["stop_bot","archive_bot","unarchive_bot","delete_bot","update_bot_settings","start_bot","liquidate_exited_asset","answer_redeploy_offer"];
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
    let arguments=match write {
        Write::Liquidate=>serde_json::json!({"symbol":args["symbol"],"dry_run":tools::paper_trading(c,who.user_id)?}),
        Write::Redeploy=>serde_json::json!({"accept":args["accept"],"dry_run":tools::paper_trading(c,who.user_id)?}),
        _=>args.clone(),
    };
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
/// A10: create_bot, create_index_bot and create_signal_bot (CreateBotTool, CreateIndexBotTool, CreateSignalBotTool#perform):
/// the service's refusal as the error text; a success in the tool's words. A `number` property reaches Rails as a Float.
pub fn create(c:&Connection,app:&App,who:Bearer,kind:crate::web::bot::create::Kind,args:&Value)->Result<Value,WebError>{
    use crate::web::bot::create::Kind;
    let res=bot_api::create(c,app,who.user_id,kind,&args.as_object().cloned().unwrap_or_default())?;
    if res.code.is_some() {return Ok(tool_text(&res.message,true));}
    let (d,s)=(&res.data,tools::str_value);
    let float=|v:&Value| v.as_f64().map_or_else(||s(v),crate::ruby::float_to_s);
    let upcase=|v:&Value| s(v).to_uppercase();
    Ok(tool_text(&match kind {
        Kind::Dca=>{
            let summary=format!("{} on {}, {} {}/{}.",s(&d["pair"]),s(&d["exchange"]),float(&args["quote_amount"]),upcase(&args["quote_asset"]),s(&args["interval"]));
            if bot_api::present(&args["start_at"]) {format!("Bot '{}' created — scheduled to start {} — {summary}",s(&d["label"]),zoned(c,who.user_id,&d["started_at"])?)}
            else {format!("Bot '{}' created and started — {summary}",s(&d["label"]))}
        }
        Kind::Index=>format!("Bot '{}' created and started — {} top {} on {}, {} {}/{}.",s(&d["label"]),s(&d["index"]),s(&d["num_coins"]),s(&d["exchange"]),float(&d["quote_amount"]),s(&d["quote_asset"]),s(&d["interval"])),
        Kind::Signal=>format!("Signal bot '{}' created and started — {} on {}. Place orders through it with market_buy / market_sell and bot_id {}.",s(&d["label"]),s(&d["pair"]),s(&d["exchange"]),d["id"]),
    },false))
}
/// `started_at.iso8601` under ActionMCP, which sets Time.zone to the user's: the time in their zone with its offset ("Z" in UTC).
fn zoned(c:&Connection,user:i64,at:&Value)->Result<String,WebError>{
    let Some(text)=at.as_str() else {return Ok(tools::str_value(at))};
    let at=crate::codec::parse_time(text).map_err(|e|WebError::Config(format!("a created bot's started_at: {e:?}")))?;
    let zone:String=c.query_row("SELECT time_zone FROM users WHERE id=?1",[user],|r|r.get(0))?;
    let local=crate::web::timezone::local(at,&zone);
    Ok(if local.timezone()==chrono_tz::Tz::UTC {at.format("%Y-%m-%dT%H:%M:%SZ").to_string()} else {local.format("%Y-%m-%dT%H:%M:%S%:z").to_string()})
}
