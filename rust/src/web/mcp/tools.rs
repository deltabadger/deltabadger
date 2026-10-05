use super::protocol::{metadata, tool_text, validate};
use super::reads;
use crate::web::{bearer::Bearer, consent, App, WebError, timezone};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
pub const NAMES: [&str;8] = ["list_bots","get_bot_details","list_exchanges","get_exchange_balances","get_portfolio_summary","list_transactions","list_open_orders","list_tax_jurisdictions"];
pub fn registry(c: &Connection, who: Bearer) -> Result<Vec<String>,WebError> {
    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;
    Ok(enabled.into_iter().filter(|n| NAMES.contains(&n.as_str()) && granted.contains(n)).collect())
}
/// Separate from lookup, as ApplicationMCPTool#call is; also exercised directly with stale registries.
pub fn gate(c: &Connection, who: Bearer, name: &str) -> Result<Option<Value>,WebError> {
    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;
    Ok(if !enabled.iter().any(|n| n == name) {Some(tool_text(&format!("Tool '{name}' is disabled. Enable it in Settings > MCP."),true))}
       else if !granted.iter().any(|n| n == name) {Some(tool_text(&format!("Tool '{name}' is not available to this client. Grant it in Settings > Connect."),true))} else {None})
}
pub(super) fn str_value(v: &Value) -> String { match v {Value::Null => String::new(), Value::String(s) => s.clone(), _ => v.to_string()} }
pub(super) fn asset(c: &Connection,id: &Value) -> Result<Option<String>,WebError> {
    let id = id.as_i64().unwrap_or_else(|| crate::ruby::to_i(id.as_str().unwrap_or("")));
    Ok(c.query_row("SELECT symbol FROM assets WHERE id=?1",[id],|r|r.get(0)).optional()?)
}
pub(super) const STATUSES: [&str;8] = ["created","scheduled","stopped","deleted","executing","retrying","waiting","archived"];
fn bots(c: &Connection,user:i64,args:&Value) -> Result<String,WebError> {
    let filter = args["status"].as_str().filter(|s| !s.trim().is_empty());
    let mut q = c.prepare("SELECT b.type,b.label,b.settings,b.status,e.name FROM bots b LEFT JOIN exchanges e ON b.exchange_id=e.id WHERE b.user_id=?1 AND b.status != 3")?;
    let rows = q.query_map([user],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<String>>(4)?)))?;
    let mut lines=vec![];
    for row in rows {
        let (kind,label,settings,status,exchange) = row?;
        if !["Bots::DcaMultiAsset","Bots::DcaSingleAsset","Bots::DcaIndex","Bots::Signal"].contains(&kind.as_str()){return Err(WebError::Config("unknown bot class".into()));}
        let name = usize::try_from(status).ok().and_then(|i|STATUSES.get(i)).copied().unwrap_or("");
        if filter.is_some_and(|f| f != name && f.parse::<i64>().ok() != Some(status)) {continue;}
        let s:Value = serde_json::from_str(&settings).unwrap_or(Value::Null);
        let quote=asset(c,&s["quote_asset_id"])?;
        let multi = kind == "Bots::DcaMultiAsset";
        let base = if multi {
            let ids:Vec<Value> = if let Some(a)=s["allocations"].as_object().filter(|a|!a.is_empty()) {a.keys().map(|k|json!(k)).collect()} else {s["base_asset_ids"].as_array().cloned().unwrap_or_default()};
            let mut symbols=vec![];
            for id in ids {if let Some(symbol)=asset(c,&id)? {symbols.push(symbol);}}
            Some(symbols.join("+"))
        } else {asset(c,&s["base_asset_id"])?};
        let pair=if multi || (base.is_some() && quote.is_some()) {format!("{}/{}",base.unwrap_or_default(),quote.clone().unwrap_or_default())} else {"N/A".into()};
        let type_name=type_name(&kind);
        lines.push(format!("- {} | {type_name} | {pair} | {} | {name} | {} {}/{}",label.unwrap_or_default(),exchange.unwrap_or_else(||"N/A".into()),str_value(&s["quote_amount"]),quote.unwrap_or_default(),s.get("interval").filter(|v|!v.is_null()).map(str_value).unwrap_or_else(||"N/A".into())));
    }
    Ok(if lines.is_empty(){"No bots found.".into()}else{format!("Bots ({}):\n{}",lines.len(),lines.join("\n"))})
}
/// `type.to_s.demodulize.titleize` for the bot classes there are.
pub(super) fn type_name(kind:&str)->&str{match kind{"Bots::DcaMultiAsset"=>"Dca Multi Asset","Bots::DcaIndex"=>"Dca Index","Bots::DcaSingleAsset"=>"Dca Single Asset","Bots::Signal"=>"Signal",_=>kind.rsplit("::").next().unwrap_or("")}}
fn exchanges(c:&Connection,user:i64)->Result<String,WebError>{
    let mut q=c.prepare("SELECT e.name,k.status FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0")?;
    let rows=q.query_map([user],|r|Ok((r.get::<_,String>(0)?,r.get::<_,usize>(1)?)))?;
    let mut lines=vec![];
    for row in rows{let (name,status)=row?;lines.push(format!("- {name} | API key status: {}",["pending_validation","correct","incorrect","pending_activation"].get(status).copied().unwrap_or("")));}
    Ok(if lines.is_empty(){"No exchanges connected. Add an API key when creating a bot.".into()}else{format!("Connected Exchanges ({}):\n{}",lines.len(),lines.join("\n"))})
}
pub(super) fn number(r:&rusqlite::Row<'_>,col:usize)->rusqlite::Result<Option<String>> {
    let value = r.get_ref(col)?;
    // Preserve SQLite INTEGER digits; retain the existing REAL and TEXT decimal conversions.
    crate::ruby::from_sql(value)
        .map(|v| v.map(|d| d.to_s_f()))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(col, value.data_type(), format!("{e:?}").into()))
}
fn transactions(c:&Connection,user:i64,args:&Value)->Result<String,WebError>{
    let bot=args["bot_id"].as_f64().map(|v|v as i64);
    if let Some(id)=bot {if c.query_row("SELECT id FROM bots WHERE id=?1 AND user_id=?2 AND status!=3",(id,user),|r|r.get::<_,i64>(0)).optional()?.is_none(){return Ok("Bot not found.".into());}}
    let raw=args["limit"].as_f64().unwrap_or(20.0) as i64;
    let limit=if raw<=0{20}else{raw.min(100)};
    let zone:String=c.query_row("SELECT time_zone FROM users WHERE id=?1",[user],|r|r.get(0))?;
    let mut q=c.prepare("SELECT t.created_at,t.side,t.status,t.amount_exec,t.base,t.price,t.quote,t.quote_amount_exec FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND (?2 IS NULL OR b.id=?2) ORDER BY t.created_at DESC LIMIT ?3")?;
    let rows=q.query_map(rusqlite::params![user,bot,limit],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,usize>(2)?,number(r,3)?,r.get::<_,Option<String>>(4)?,number(r,5)?,r.get::<_,Option<String>>(6)?,number(r,7)?)))?;
    let mut lines=vec![];
    for row in rows{
        let (time,side,status,amount,base,price,quote,cost)=row?;
        let t=crate::codec::parse_time(&time).map_err(|e|WebError::Config(format!("MCP timestamp: {e:?}")))?;
        let date=timezone::local(t,&zone).format("%Y-%m-%d %H:%M");
        let quote=quote.unwrap_or_default();
        lines.push(format!("- [{date}] {} {} {} {} | {}",side.and_then(|s|usize::try_from(s).ok()).and_then(|s|["BUY","SELL"].get(s).copied()).unwrap_or(""),amount.map(|v|format!("{v} {}",base.unwrap_or_default())).unwrap_or_else(||"N/A".into()),price.map(|v|format!("@ {v} {quote}")).unwrap_or_default(),cost.map(|v|format!("({v} {quote})")).unwrap_or_default(),["submitted","failed","skipped"].get(status).copied().unwrap_or("")));
    }
    Ok(if lines.is_empty(){"No transactions found.".into()}else{format!("Transactions ({}):\n{}",lines.len(),lines.join("\n"))})
}
/// What a call answers now, or the venue or market read it needs first (`reads`).
pub enum Called { Done(Value), Fetch(reads::Fetch) }
pub fn call(c:&Connection,app:&App,who:Bearer,name:&str,args:&Value)->Result<Called,WebError>{
    if let Some(refusal)=gate(c,who,name)? {return Ok(Called::Done(refusal));}
    let schema=metadata()["tools"].as_array().into_iter().flatten().find(|t|t["name"]==name).map(|t|&t["inputSchema"]).unwrap_or(&Value::Null);
    let errors=validate(args,schema,"");
    if !errors.is_empty(){return Ok(Called::Done(tool_text(&format!("Invalid input: {}",errors.join(", ")),true)));}
    if reads::NAMES.contains(&name){return Ok(reads::plan(c,app,who.user_id,name,args).unwrap_or_else(|_|Called::Done(tool_text("An unexpected error occurred.",true))));}
    let text=match name {
        "list_bots"=>bots(c,who.user_id,args),
        "list_exchanges"=>exchanges(c,who.user_id),
        "list_transactions"=>transactions(c,who.user_id,args),
        "list_tax_jurisdictions"=>{
            let rows:Vec<_>=metadata()["tax"]["jurisdictions"].as_array().into_iter().flatten().map(|r|format!("- {} — {} | Method: {} | Currency: {}",str_value(&r["code"]),str_value(&r["name"]),str_value(&r["method"]),str_value(&r["currency"]))).collect();
            Ok(format!("Supported tax jurisdictions ({}):\n{}",rows.len(),rows.join("\n")))
        },_=>Err(WebError::Config("unregistered MCP tool".into()))
    };
    Ok(Called::Done(match text{Ok(t)=>tool_text(&t,false),Err(_)=>tool_text("An unexpected error occurred.",true)}))
}
