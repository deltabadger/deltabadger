//! actionmcp 0.201.0's non-streaming transport on its existing SQLite tables.
pub mod protocol;
pub mod tools;
use super::{App,WebError,bearer};
use axum::{body::to_bytes,extract::Request,http::{HeaderMap,Method,StatusCode},response::{Response,IntoResponse}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::{Value,json};
use base64::{Engine,engine::general_purpose::{URL_SAFE,URL_SAFE_NO_PAD}};
use rand::RngCore;
use protocol::{error,result,metadata};
const VERSION:&str="2025-11-25";
pub fn instructions(c:&Connection)->Result<String,WebError>{
    let mut q=c.prepare("SELECT name FROM exchanges WHERE available=1 AND type NOT IN ('Exchanges::Bitmart')")?;
    let exchanges=q.query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?.join(", ");
    Ok(format!("Deltabadger is a user's personal investing server. Available exchanges: {exchanges}.\nSupports both cryptocurrency and stocks/ETFs (via Alpaca).\nTrading is available either via DCA bots, or by direct access to connected exchanges\nWhen the user asks to trade stocks or ETFs (e.g., QQQM, SPY, AAPL), use the Alpaca exchange.\nUse list_exchanges to see which exchanges the user has connected before placing orders."))
}
fn response(status:u16,body:Option<Value>,controller:bool)->Response {
    let content=if controller{"application/json; charset=utf-8"}else{"application/json"};
    let text=body.map(|v|v.to_string()).unwrap_or_default();
    let mut r=(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),[("content-type",content),("cache-control",if status==200{"max-age=0, private, must-revalidate"}else{"no-cache"})],text).into_response();
    if controller || matches!(status,200|202|204|405) { let status=r.status();super::headers::controller_defaults(r.headers_mut(),status,false); }
    if status==204{r.headers_mut().remove("content-type");}
    r
}
fn err(status:u16,id:&Value,code:i64,text:&str)->Response{response(status,Some(error(id,code,text)),true)}
fn header<'a>(h:&'a HeaderMap,k:&str)->Option<&'a str>{h.get(k).and_then(|v|v.to_str().ok())}
fn add(r:&mut Response,k:&'static str,v:&str){if let Ok(v)=v.parse(){r.headers_mut().insert(k,v);}}
fn supported_origin(origin:&str)->bool{
    super::oauth::Uri::parse(origin).is_some_and(|u| {
        matches!(u.scheme.as_deref(),Some("http"|"https")) && u.userinfo.is_none() && u.query.is_none() && u.fragment.is_none() && u.path.is_empty()
            && u.host.is_some_and(|h|["localhost","127.0.0.1","::1","[::1]"].iter().any(|allowed|h.eq_ignore_ascii_case(allowed)))
    })
}
fn accepts(h:&HeaderMap)->bool{
    let mut found=[false;2];
    for entry in header(h,"accept").unwrap_or("").split(','){
        let mut parts=entry.trim().split(';');let media=parts.next().unwrap_or("").trim();
        let quality=parts.find_map(|p|p.trim().strip_prefix("q=").map(|q|q.parse::<f64>().unwrap_or(0.0))).unwrap_or(1.0);
        for (i,name) in ["application/json","text/event-stream"].iter().enumerate(){if media.eq_ignore_ascii_case(name)&&quality>0.0{found[i]=true;}}
    }
    found.into_iter().all(|v|v)
}
/// Called by entry after host/query limits, before its form parsing, locale and cookie handling.
pub async fn entry(app:App,request:Request,deadline:std::time::Duration)->Response{
    let (parts,body)=request.into_parts();
    let h=parts.headers;
    let auth=header(&h,"authorization").map(str::to_string);
    if bearer::bearer_token(auth.as_deref()).is_none(){
        let mut r=response(401,Some(json!({"error":"unauthorized","error_description":"Bearer token required"})),false);
        let scheme=if app.config.force_ssl || header(&h,"x-forwarded-proto").is_some_and(|v|v.starts_with("https")){"https"}else{"http"};
        add(&mut r,"www-authenticate",&format!("Bearer resource_metadata=\"{scheme}://{}/.well-known/oauth-protected-resource\"",header(&h,"host").unwrap_or("")));
        return r;
    }
    let path=parts.uri.path().trim_end_matches('/');
    if path=="/mcp/up"&&matches!(parts.method,Method::GET|Method::HEAD){
        let mut r=if header(&h,"accept").is_some_and(|a|a.split(',').any(|m|m.trim().starts_with("application/json"))) {
            response(200,Some(json!({"status":"up","timestamp":app.now().to_rfc3339_opts(chrono::SecondsFormat::Secs,true)})),true)
        }else{super::up(axum::extract::State(app.clone())).await};
        if parts.method==Method::HEAD{*r.body_mut()=axum::body::Body::empty();}
        return r;
    }
    if path!="/mcp"{return StatusCode::NOT_FOUND.into_response();}
    if header(&h,"origin").is_some_and(|o|!supported_origin(o)) {return response(403,Some(error(&Value::Null,-32600,"Forbidden: invalid Origin header")),false);}
    let mut payload=Value::Null;
    if parts.method==Method::POST{
        if !header(&h,"content-type").unwrap_or("").split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"){
            return response(415,Some(error(&Value::Null,-32000,"Unsupported Media Type: Content-Type must be application/json")),false);
        }
        let bytes=match tokio::time::timeout(deadline,to_bytes(body,super::FORM_LIMIT)).await{
            Err(_)=>return (StatusCode::REQUEST_TIMEOUT,[("connection","close")],"The form did not arrive in time\n").into_response(),
            Ok(Err(_))=>return (StatusCode::PAYLOAD_TOO_LARGE,"Form too large\n").into_response(),
            Ok(Ok(bytes))=>bytes
        };
        payload=match serde_json::from_slice(&bytes){Ok(v)=>v,Err(_)=>return response(400,Some(error(&Value::Null,-32700,"Parse error")),false)};
        if !protocol::valid_envelope(&payload){
            let id=payload.get("id").filter(|id|id.is_string()||id.is_number()).unwrap_or(&Value::Null);
            return response(400,Some(error(id,-32600,"Invalid Request")),false);
        }
        if !accepts(&h){return err(406,&payload["id"],-32000,"Not Acceptable: Client must accept both application/json and text/event-stream");}
    }
    let app2=app.clone();
    let id=payload["id"].clone();
    let answer=app.db(move|c|{
        let who=match bearer::authenticate(c,auth.as_deref(),"mcp",app2.clock.as_ref())?{
            Ok(w)=>w,Err(why)=>{
                let message=match why{bearer::Refusal::Missing=>"Missing bearer token",bearer::Refusal::Invalid=>"Invalid access token",bearer::Refusal::Revoked=>"Access token revoked",bearer::Refusal::Expired=>"Access token expired",bearer::Refusal::InsufficientScope=>"Access token missing required scope",bearer::Refusal::UserNotFound=>"User not found"};
                let mut r=err(401,&payload["id"],-32000,message);add(&mut r,"www-authenticate","Bearer error=\"invalid_token\"");return Ok(r);
            }
        };
        let initializing=payload["method"]=="initialize";
        if initializing{c.execute_batch("SAVEPOINT mcp_initialize")?;}
        let answer=dispatch(c,&app2,who,&parts.method,&h,&payload);
        if initializing{
            if answer.is_err(){c.execute_batch("ROLLBACK TO mcp_initialize")?;}
            c.execute_batch("RELEASE mcp_initialize")?;
        }
        answer
    }).await;
    match answer{Ok(r)=>r,Err(_)=>err(500,&id,-32603,"An unexpected error occurred.")}
}
struct Session{ id:String,status:String,initialized:bool,version:Option<String> }
fn load(c:&Connection,id:&str)->Result<Option<Session>,WebError>{Ok(c.query_row("SELECT id,status,initialized,protocol_version FROM action_mcp_sessions WHERE id=?1",[id],|r|Ok(Session{id:r.get(0)?,status:r.get(1)?,initialized:r.get(2)?,version:r.get(3)?})).optional()?)}
fn history(c:&Connection,s:&Session,payload:&Value,outgoing:bool,now:&str)->Result<(),WebError>{
    let id=payload.get("id");
    let kind=if payload.get("method").is_some(){if id.is_some(){"request"}else{"notification"}}else if payload.get("error").is_some(){"error"}else{"response"};
    let ping=payload["method"]=="ping";
    let id_text=id.map(|v|v.as_str().map(str::to_string).unwrap_or_else(||v.to_string()));
    c.execute("INSERT INTO action_mcp_session_messages (session_id,direction,message_json,message_type,jsonrpc_id,is_ping,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",rusqlite::params![s.id,if outgoing{"client"}else{"server"},payload.to_string(),kind,id_text,ping,now])?;
    c.execute("UPDATE action_mcp_sessions SET messages_count=messages_count+1 WHERE id=?1",[&s.id])?;
    if outgoing&&id.is_some(){
        let request:Option<(i64,bool)>=c.query_row("SELECT id,is_ping FROM action_mcp_session_messages WHERE session_id=?1 AND direction='server' AND message_type='request' AND jsonrpc_id=?2 ORDER BY created_at DESC LIMIT 1",rusqlite::params![s.id,id_text],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((request,ping))=request{
            let response=c.last_insert_rowid();
            c.execute("UPDATE action_mcp_session_messages SET request_acknowledged=1,updated_at=?2 WHERE id=?1",(request,now))?;
            c.execute("UPDATE action_mcp_session_messages SET is_ping=?2 WHERE id=?1",(response,ping))?;
        }
    }
    Ok(())
}
fn dispatch(c:&Connection,app:&App,who:bearer::Bearer,http:&Method,h:&HeaderMap,v:&Value)->Result<Response,WebError>{
    let method=v["method"].as_str().unwrap_or("");let id=&v["id"];
    if let Some((code,message))=protocol::params_error(v){return Ok(err(if v.get("id").is_some(){200}else{400},id,code,&message));}
    let sid=header(h,"mcp-session-id").filter(|s|!s.trim().is_empty());
    let initialize=http==Method::POST&&method=="initialize"&&v.get("id").is_some();
    let now=crate::codec::format_time(app.now());
    let names=tools::registry(c,who)?;
    let info=json!({"name":"Deltabadger","version":metadata()["version"]});
    let caps=json!({"tools":{"listChanged":false},"logging":{},"completions":{}});
    let mut s=if let Some(sid)=sid{
        let Some(s)=load(c,sid)?else{return Ok(err(404,id,-32001,"Session not found."));};
        if s.status=="closed"{return Ok(err(404,id,-32001,"Session has been terminated."));}
        if initialize{return Ok(err(400,id,-32600,"Initialize requests must not include an Mcp-Session-Id header."));}s
    }else if initialize{
        let mut bytes=[0;16];rand::thread_rng().fill_bytes(&mut bytes);let sid=hex::encode(bytes);
        c.execute("INSERT INTO action_mcp_sessions (id,protocol_version,server_info,server_capabilities,tool_registry,session_data,prompt_registry,resource_registry,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'[\"*\"]','[\"*\"]',?7,?7)",rusqlite::params![sid,VERSION,info.to_string(),caps.to_string(),json!(names).to_string(),json!({"user_id":who.user_id}).to_string(),now])?;
        Session{id:sid,status:"pre_initialize".into(),initialized:false,version:Some(VERSION.into())}
    }else if matches!(*http,Method::GET|Method::HEAD){let mut r=response(405,None,false);add(&mut r,"allow","POST, DELETE");return Ok(r);}
    else{return Ok(err(400,id,-32600,if http==Method::DELETE{"Mcp-Session-Id header is required for DELETE requests."}else{"Mcp-Session-Id header is required for this request."}));};
    if !initialize{
        let ver=header(h,"mcp-protocol-version");
        if !(ver.is_none()&&s.version.as_deref()==Some(VERSION)){
            if ver!=Some(VERSION){return Ok(err(400,id,-32000,&format!("Unsupported MCP-Protocol-Version: {}. Supported versions: {VERSION}",ver.unwrap_or(""))));}
            if ver!=s.version.as_deref(){return Ok(err(400,id,-32000,&format!("MCP-Protocol-Version header ({}) does not match negotiated version ({})",ver.unwrap_or(""),s.version.as_deref().unwrap_or(""))));}
        }
    }
    if matches!(*http,Method::GET|Method::HEAD){let mut r=response(405,None,false);add(&mut r,"allow","POST, DELETE");return Ok(r);}
    if http==Method::DELETE{
        let count:i64=c.query_row("SELECT messages_count FROM action_mcp_sessions WHERE id=?1",[&s.id],|r|r.get(0))?;
        if count==0{c.execute("DELETE FROM action_mcp_sessions WHERE id=?1",[&s.id])?;}else{
            c.execute("UPDATE action_mcp_sessions SET status='closed',ended_at=?2,updated_at=?2,tool_registry=?3,session_data=?4 WHERE id=?1",rusqlite::params![s.id,now,json!(names).to_string(),json!({"user_id":who.user_id}).to_string()])?;
            c.execute("DELETE FROM action_mcp_session_subscriptions WHERE session_id=?1",[&s.id])?;
        }return Ok(response(204,None,false));
    }
    if http!=Method::POST{return Ok(StatusCode::NOT_FOUND.into_response());}
    let initialized_notice=method=="notifications/initialized"&&v.get("id").is_none();
    if initialized_notice{
        if s.status!="initializing"||s.initialized{return Ok(err(400,id,-32600,"Session is not awaiting an initialized notification."));}
    }else if !(initialize || s.initialized || s.status=="initializing"&&method=="ping"&&v.get("id").is_some()){
        return Ok(err(400,id,-32600,"Session initialization is incomplete."));
    }
    history(c,&s,v,false,&now)?;
    if method=="logging/setLevel" {
        c.execute("UPDATE action_mcp_sessions SET session_data=?2,tool_registry=?3,updated_at=?4 WHERE id=?1",rusqlite::params![s.id,json!({"user_id":who.user_id,"action_mcp_logging_level":v["params"]["level"]}).to_string(),json!(names).to_string(),now])?;
    }
    if initialize||initialized_notice{
        c.execute("UPDATE action_mcp_sessions SET status=?2,initialized=?3,client_info=COALESCE(?4,client_info),client_capabilities=COALESCE(?5,client_capabilities),tool_registry=?6,session_data=?7,updated_at=?8 WHERE id=?1",rusqlite::params![s.id,if initialize{"initializing"}else{"initialized"},!initialize,initialize.then(||v["params"]["clientInfo"].to_string()),initialize.then(||v["params"]["capabilities"].to_string()),json!(names).to_string(),json!({"user_id":who.user_id}).to_string(),now])?;
        s.initialized = !initialize;
    }
    if v.get("id").is_none()||v.get("method").is_none(){return Ok(response(202,None,false));}
    let p=&v["params"];
    let answer=match method{
        "initialize"=>result(id,json!({"protocolVersion":VERSION,"serverInfo":info,"capabilities":caps,"instructions":app.mcp_instructions})),
        "ping"|"logging/setLevel"=>result(id,json!({})),
        "prompts/list"=>result(id,json!({"prompts":[]})),
        "completion/complete"=>error(id,-32602,"Unknown completion reference or argument"),
        "tools/list"=>{
            if let Some(token)=p["_meta"].get("progressToken"){
                history(c,&s,&json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":token,"progress":0,"message":"Starting tools list retrieval"}}),true,&now)?;
            }
            let offset=if let Some(cursor)=p["cursor"].as_str(){
                if cursor.is_empty(){Err("Cursor must be a non-empty string")}else{
                    URL_SAFE_NO_PAD.decode(cursor).or_else(|_|URL_SAFE.decode(cursor)).map_err(|_|"Invalid cursor encoding").and_then(|s|{
                        if s.is_empty()||!s.iter().all(u8::is_ascii_digit){Err("Invalid cursor format")}else{Ok(std::str::from_utf8(&s).ok().and_then(|s|s.parse::<usize>().ok()).unwrap_or(usize::MAX))}
                    })
                }
            }else{Ok(0)};
            match offset{Err(e)=>error(id,-32602,e),Ok(n)=>{
                let all:Vec<Value>=names.iter().filter_map(|name|metadata()["tools"].as_array()?.iter().find(|t|t["name"]==*name).cloned()).skip(n).take(10).collect();if let Some(token)=p["_meta"].get("progressToken"){
                    history(c,&s,&json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":token,"progress":100,"message":"Tools list retrieval complete"}}),true,&now)?;
                }result(id,json!({"tools":all}))
            }}
        },
        "tools/call"=>{
            let name=p["name"].as_str().unwrap_or("");
            if !names.iter().any(|n|n==name){error(id,-32602,&format!("Tool '{name}' not found or not registered for this session"))}
            else{result(id,tools::call(c,who,name,p.get("arguments").unwrap_or(&json!({})))?)}
        },
        m if m.starts_with("tools/")=>error(id,-32601,&format!("Unknown tools method: {m}")),
        m if m.starts_with("resources/")=>error(id,-32601,"Resources are not available for this session"),
        m if m.starts_with("tasks/")=>error(id,-32601,"Tasks are not available for this session"),
        _=>error(id,-32601,&format!("Method not found: {method}"))
    };
    history(c,&s,&answer,true,&now)?;
    let mut r=response(200,Some(answer),false);
    if initialize{add(&mut r,"mcp-session-id",&s.id);}
    if s.initialized{add(&mut r,"mcp-protocol-version",VERSION);}
    Ok(r)
}
