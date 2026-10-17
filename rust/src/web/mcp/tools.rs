use super::protocol::{metadata, tool_text, validate};
use super::reads;
use crate::web::{bearer::Bearer, consent, App, WebError, timezone};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
pub const NAMES: [&str;14] = ["list_bots","get_bot_details","list_exchanges","get_exchange_balances","get_portfolio_summary","list_transactions","start_bot","stop_bot","update_bot_settings","list_open_orders","list_tax_jurisdictions","delete_bot","archive_bot","unarchive_bot"];
pub fn registry(c: &Connection, who: Bearer) -> Result<Vec<String>,WebError> {
    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;
    Ok(enabled.into_iter().filter(|n| NAMES.contains(&n.as_str()) && granted.contains(n)).collect())
}
/// Separate from lookup, as ApplicationMCPTool#call is; also exercised directly with stale registries.
pub fn gate(c: &Connection, who: Bearer, name: &str) -> Result<Option<Value>,WebError> {
    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;
    Ok(if !enabled.iter().any(|n| n == name) {Some(tool_text(&format!("Tool '{name}' is disabled. Enable it in Settings > MCP."),true))}
       else if !granted.iter().any(|n| n == name) {Some(tool_text(&format!("Tool '{name}' is not available to this client. Grant it in Settings > Connect."),true))}
       else if LIVE_AUTOMATION_TOOLS.contains(&name) && paper_trading(c,who.user_id)? {Some(tool_text(&format!("[DRY RUN] Paper trading is on, so nothing was created or started: '{name}' would leave a bot or rule running that moves real money. Turn Paper Trading off in Settings > MCP to use it."),true))}
       else {None})
}
/// ApplicationMCPTool::LIVE_AUTOMATION_TOOLS; only start_bot is served here, the rest stay Rails'.
const LIVE_AUTOMATION_TOOLS: [&str;5] = ["create_bot","create_index_bot","create_signal_bot","start_bot","start_rule"];
/// User#mcp_dry_run?: `mcp_settings['dry_run'] == true`, nothing looser.
fn paper_trading(c: &Connection, user_id: i64) -> Result<bool,WebError> {
    let stored: Option<String> = c.query_row("SELECT mcp_settings FROM users WHERE id = ?1", [user_id], |r| r.get(0)).optional()?.flatten();
    let Some(text) = stored else { return Ok(false) };
    let settings: Value = serde_json::from_str(&text).map_err(|_| WebError::Config("unreadable mcp_settings".into()))?;
    Ok(settings["dry_run"] == Value::Bool(true))
}
pub(crate) fn str_value(v: &Value) -> String { match v {Value::Null => String::new(), Value::String(s) => s.clone(), _ => v.to_string()} }
pub(crate) fn asset(c: &Connection,id: &Value) -> Result<Option<String>,WebError> {
    let Some(id) = id.as_i64().filter(|id|*id>0).or_else(||id.as_str().and_then(crate::web::bot::id_from_path)) else{return Ok(None)};
    Ok(c.query_row("SELECT symbol FROM assets WHERE id=?1",[id],|r|r.get(0)).optional()?)
}
pub(crate) const STATUSES: [&str;8] = ["created","scheduled","stopped","deleted","executing","retrying","waiting","archived"];
/// ListBotsTool#present, from BotApi::Bots::List's rows.
fn bots(data: &Value) -> String {
    let rows = data["bots"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() { return "No bots found.".into(); }
    let or_na = |v: &Value| if v.is_null() {"N/A".to_string()} else {str_value(v)};
    let lines: Vec<String> = rows.iter().map(|r| format!("- {} (ID {}) | {} | {} | {} | {} | {} {}/{}", str_value(&r["label"]), r["id"], type_name(r["type"].as_str().unwrap_or("")),
        or_na(&r["pair"]), or_na(&r["exchange"]), str_value(&r["status"]), str_value(&r["quote_amount"]), str_value(&r["quote_asset"]), or_na(&r["interval"]))).collect();
    format!("Bots ({}):\n{}", data["count"], lines.join("\n"))
}
/// `type.to_s.demodulize.titleize` for the bot classes there are.
pub(crate) fn type_name(kind:&str)->&str{match kind{"Bots::DcaMultiAsset"=>"Dca Multi Asset","Bots::DcaIndex"=>"Dca Index","Bots::DcaSingleAsset"=>"Dca Single Asset","Bots::Signal"=>"Signal",_=>kind.rsplit("::").next().unwrap_or("")}}
/// ListExchangesTool#perform's text.
fn exchanges(data: &Value) -> String {
    let rows = data["exchanges"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() { return "No exchanges connected. Add an API key when creating a bot.".into(); }
    let lines: Vec<String> = rows.iter().map(|r| format!("- {} | API key status: {}", str_value(&r["name"]), str_value(&r["api_key_status"]))).collect();
    format!("Connected Exchanges ({}):\n{}", data["count"], lines.join("\n"))
}
/// ListTransactionsTool#present: the times in the user's zone (ActionMCP::Current sets it).
fn transactions(c: &Connection, user: i64, data: &Value) -> Result<String, WebError> {
    let rows = data["transactions"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() { return Ok("No transactions found.".into()); }
    let zone: String = c.query_row("SELECT time_zone FROM users WHERE id=?1",[user],|r|r.get(0))?;
    let mut lines = vec![];
    for r in &rows {
        let t = crate::codec::parse_time(r["created_at"].as_str().unwrap_or("")).map_err(|e|WebError::Config(format!("MCP timestamp: {e:?}")))?;
        let date = timezone::local(t,&zone).format("%Y-%m-%d %H:%M");
        let (base, quote) = (str_value(&r["base"]), str_value(&r["quote"]));
        let amount = if r["amount_exec"].is_null() {"N/A".to_string()} else {format!("{} {base}", str_value(&r["amount_exec"]))};
        let nonzero = r["price"].as_str().and_then(|p| crate::ruby::BigDec::parse(p).ok()).is_some_and(|p| !p.is_zero());
        let price = if nonzero {format!("@ {} {quote}", str_value(&r["price"]))} else {String::new()};
        let cost = if r["quote_amount_exec"].is_null() {String::new()} else {format!("({} {quote})", str_value(&r["quote_amount_exec"]))};
        lines.push(format!("- [{date}] {} {amount} {price} {cost} | {}", str_value(&r["side"]).to_uppercase(), str_value(&r["status"])));
    }
    Ok(format!("Transactions ({}):\n{}", data["count"], lines.join("\n")))
}
/// What a call answers now, or the venue or market read it needs first (`reads`).
pub enum Called { Done(Value), Fetch(reads::Fetch) }
pub fn call(c:&Connection,app:&App,who:Bearer,name:&str,args:&Value)->Result<Called,WebError>{
    if super::control::NAMES.contains(&name) {
        return Ok(Called::Done(match super::control::call(c,app,who,name,args) {
            Ok(value)=>value,
            Err(_)=>{
                // Match ActionMCP's exception response without exposing credentials or DB text.
                crate::engine::log("MCP control tool failed");
                tool_text("An unexpected error occurred.",true)
            }
        }));
    }
    if let Some(refusal)=gate(c,who,name)? {return Ok(Called::Done(refusal));}
    let schema=metadata()["tools"].as_array().into_iter().flatten().find(|t|t["name"]==name).map(|t|&t["inputSchema"]).unwrap_or(&Value::Null);
    let errors=validate(args,schema,"");
    if !errors.is_empty(){return Ok(Called::Done(tool_text(&format!("Invalid input: {}",errors.join(", ")),true)));}
    if reads::NAMES.contains(&name){return Ok(reads::plan(c,app,who.user_id,name,args).unwrap_or_else(|_|Called::Done(tool_text("An unexpected error occurred.",true))));}
    use crate::web::bot_api;
    let text=match name {
        "list_bots"=>bot_api::bots(c,who.user_id,args.get("status")).map(|r|(bots(&r.data),false)),
        "list_exchanges"=>bot_api::exchanges(c,who.user_id).map(|r|(exchanges(&r.data),false)),
        // MCP's schema has already made both numbers (or absent).
        "list_transactions"=>bot_api::transactions(c,who.user_id,args.get("bot_id").unwrap_or(&Value::Null),args.get("limit").unwrap_or(&Value::Null)).and_then(|r|match r {
            Some(r) if r.code.is_some()=>Ok((r.message,true)),
            Some(r)=>transactions(c,who.user_id,&r.data).map(|t|(t,false)),
            None=>Err(WebError::Config("unreadable list_transactions arguments".into())),
        }),
        "list_tax_jurisdictions"=>{
            let data=bot_api::tax_jurisdictions().data;
            let rows:Vec<_>=data["jurisdictions"].as_array().into_iter().flatten().map(|r|format!("- {} — {} | Method: {} | Currency: {}",str_value(&r["code"]),str_value(&r["name"]),str_value(&r["method"]),str_value(&r["currency"]))).collect();
            Ok((format!("Supported tax jurisdictions ({}):\n{}",data["count"],rows.join("\n")),false))
        },_=>Err(WebError::Config("unregistered MCP tool".into()))
    };
    Ok(Called::Done(match text{Ok((t,is_error))=>tool_text(&t,is_error),Err(_)=>tool_text("An unexpected error occurred.",true)}))
}
