//! The same raw HTTP messages and fixed tokens on Rails-created SQLite databases.
mod common;
use common::web::{self, TestClock};
use axum::{body::{Body, to_bytes}, http::Request};
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt;
use base64::Engine;
const AT: &str = "2026-09-10T12:00:30.123456Z";
const HEADERS: [&str; 12] = ["content-type", "www-authenticate", "mcp-session-id", "mcp-protocol-version", "allow", "cache-control", "x-frame-options", "x-xss-protection", "x-content-type-options", "x-permitted-cross-domain-policies", "referrer-policy", "content-security-policy-report-only"];
fn read(p: impl AsRef<Path>) -> Value { serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap() }
fn copy_install(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}
async fn play(dir: &Path, steps: &[Value], mut session: Option<String>) -> Value {
    let app = web::app(dir, web::SECRET, TestClock::at(AT));
    let router = deltabadger::web::router(app);
    let mut responses = vec![];
    for step in steps {
        if let Some(sql) = step["sql"].as_array() {
            let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
            for s in sql { c.execute_batch(s.as_str().unwrap()).unwrap(); }
        }
        let mut req = Request::builder().method(step["method"].as_str().unwrap()).uri(step["path"].as_str().unwrap()).header("host", "localhost:3000");
        for (k, v) in step["headers"].as_object().unwrap() {
            let val = v.as_str().and_then(|v| if v == "$session" { session.as_deref() } else { Some(v) });
            if let Some(v) = val { req = req.header(k, v); }
        }
        let r = router.clone().oneshot(req.body(Body::from(step["body"].as_str().unwrap().to_owned())).unwrap()).await.unwrap();
        let hs: serde_json::Map<String, Value> = HEADERS.iter().filter_map(|h| r.headers().get(*h).map(|v| (h.to_string(), json!(v.to_str().unwrap())))).collect();
        if let Some(id) = hs.get("mcp-session-id").and_then(Value::as_str) {
            assert_eq!(id.len(), 32); assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
            session = Some(id.to_string());
        }
        let status = r.status().as_u16();
        let body = String::from_utf8(to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        responses.push(json!({"status":status,"headers":hs,"body":body}));
    }
    json!({"responses":responses,"session":session,"rows":snapshot(dir)})
}
fn snapshot(dir:&Path)->Value {
    let c=rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut tables=serde_json::Map::new();
    for table in ["action_mcp_sessions","action_mcp_session_messages","action_mcp_session_subscriptions","oauth_access_tokens","connected_clients","bots","transactions","api_keys"] {
        let mut q=c.prepare(&format!("SELECT * FROM {table} ORDER BY id")).unwrap();
        let names:Vec<String>=q.column_names().iter().map(|s|s.to_string()).collect();
        let rows=q.query_map([],|r|{
            let mut row=serde_json::Map::new();
            for (i,n) in names.iter().enumerate(){let value=match r.get_ref(i)? {rusqlite::types::ValueRef::Null=>Value::Null,rusqlite::types::ValueRef::Integer(n)=>json!(n),rusqlite::types::ValueRef::Real(n)=>json!({"float_bits":format!("{:016x}",n.to_bits())}),rusqlite::types::ValueRef::Text(v)=>json!(std::str::from_utf8(v).unwrap()),_=>panic!("unexpected blob")};row.insert(n.clone(),value);}
            Ok(Value::Object(row))
        }).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        tables.insert(table.to_string(),json!(rows));
    }
    Value::Object(tables)
}
fn comparable(mut transcript: Value) -> Value {
    if let Some(id) = transcript["session"].as_str().map(str::to_string) {
        assert_eq!(id.len(),32); assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        for table in transcript["rows"].as_object_mut().unwrap().values_mut() {
            for row in table.as_array_mut().unwrap(){for (key,value) in row.as_object_mut().unwrap(){if ["id","session_id"].contains(&key.as_str()) && value==&json!(id){*value=json!("<session>");}}}
        }
        for response in transcript["responses"].as_array_mut().unwrap() {
            if response["headers"].get("mcp-session-id").is_some() { response["headers"]["mcp-session-id"] = json!("<session>"); }
        }
    }
    for response in transcript["responses"].as_array_mut().unwrap(){
        if let Some(policy)=response["headers"]["content-security-policy-report-only"].as_str(){
            let normalized=policy.split(' ').map(|part| if let Some(nonce)=part.strip_prefix("'nonce-"){
                let nonce=nonce.strip_suffix("';").expect("CSP nonce terminates its directive");
                assert_eq!(base64::engine::general_purpose::STANDARD.decode(nonce).unwrap().len(),16);
                "'nonce-<nonce>';"
            }else{part}).collect::<Vec<_>>().join(" ");
            response["headers"]["content-security-policy-report-only"]=json!(normalized);
        }
    }
    transcript.as_object_mut().unwrap().shift_remove("session");
    transcript
}
#[tokio::test(flavor="current_thread")]
async fn rails_and_rust_mcp_transcripts() {
    let root = tempfile::tempdir().unwrap();
    let boot = tempfile::tempdir().unwrap();
    common::rails(boot.path(), "test", &["runner", "script/rust/mcp.rb", "grid", root.path().to_str().unwrap()]);
    let dirs: Vec<_> = std::fs::read_dir(root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.join("steps.json").is_file()).collect();
    assert!(!dirs.is_empty(), "empty MCP grid");
    let mut actual = vec![];
    for dir in &dirs {
        let steps = read(dir.join("steps.json"));
        let rust = tempfile::tempdir().unwrap();
        copy_install(dir, rust.path());
        actual.push(play(rust.path(), steps.as_array().unwrap(), None).await);
    }
    common::rails(boot.path(), "test", &["runner", "script/rust/mcp.rb", "record", root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for (dir, got) in dirs.iter().zip(actual) {
        let want = read(dir.join("rails.json"));
        if dir.file_name().unwrap() == "oversize" {
            assert_eq!(got["responses"][2]["status"], 413);
            assert_eq!(want["responses"][2]["status"], 200); // Rails has no body bound.
        } else {
            let (got,want)=(comparable(got),comparable(want));
            if got!=want {failures.push(dir.file_name().unwrap().to_string_lossy().to_string());
                for (i,(g,w)) in got["responses"].as_array().unwrap().iter().zip(want["responses"].as_array().unwrap()).enumerate(){
                    if g!=w{eprintln!("{} step {i}\ngot {g}\nwant {w}",dir.file_name().unwrap().to_string_lossy());break;}
                }
                if got["rows"]!=want["rows"]{for (table,rows) in got["rows"].as_object().unwrap(){let expected=&want["rows"][table];if rows!=expected{for (i,(g,w)) in rows.as_array().unwrap().iter().zip(expected.as_array().unwrap()).enumerate(){if g!=w{eprintln!("{} {table} row {i} got {g} want {w}",dir.file_name().unwrap().to_string_lossy());break;}} if rows.as_array().unwrap().len()!=expected.as_array().unwrap().len(){eprintln!("{table} lengths {} {}",rows.as_array().unwrap().len(),expected.as_array().unwrap().len());}}}}
            }
        }
    }
    assert!(failures.is_empty(), "mismatches: {failures:?}");
}

fn rails_leg(boot:&Path,dir:&Path,steps:&[Value],sid:Option<&str>)->Value{
    std::fs::write(dir.join("leg.json"),serde_json::to_vec(steps).unwrap()).unwrap();
    let mut args=vec!["runner","script/rust/mcp.rb","play",dir.to_str().unwrap()];
    if let Some(id)=sid{args.push(id);}
    common::rails(boot,"test",&args);
    read(dir.join("rails.json"))
}
fn request(method:&str,body:Value,sid:bool)->Value{
    let mut headers=json!({"Authorization":"Bearer m2-token","Content-Type":"application/json","Accept":"application/json, text/event-stream"});
    if sid{headers["Mcp-Session-Id"]=json!("$session");}
    json!({"method":method,"path":"/mcp","headers":headers,"body":body.to_string()})
}
fn initialize()->Value{request("POST",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"cross","version":"1"}}}),false)}
fn rpc(method:&str,params:Value)->Value{request("POST",json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),true)}
fn grid(boot:&Path,root:&Path){common::rails(boot,"test",&["runner","script/rust/mcp.rb","grid",root.to_str().unwrap()]);}
#[tokio::test(flavor="current_thread")]
async fn sessions_cross_before_and_after_initialized_in_both_directions(){
    let root=tempfile::tempdir().unwrap();let boot=tempfile::tempdir().unwrap();grid(boot.path(),root.path());
    let steps=vec![initialize(),request("POST",json!({"jsonrpc":"2.0","method":"notifications/initialized"}),true),rpc("tools/list",json!({})),rpc("tools/call",json!({"name":"list_bots","arguments":{}})),request("DELETE",Value::Null,true),rpc("ping",json!({}))];
    let oracle=tempfile::tempdir().unwrap();copy_install(&root.path().join(".template"),oracle.path());
    let expected=rails_leg(boot.path(),oracle.path(),&steps,None);
    for cut in [1,2]{for rails_first in [true,false]{
        let dir=tempfile::tempdir().unwrap();copy_install(&root.path().join(".template"),dir.path());
        let first=if rails_first{rails_leg(boot.path(),dir.path(),&steps[..cut],None)}else{play(dir.path(),&steps[..cut],None).await};
        let sid=first["session"].as_str().unwrap();
        let mut second=if rails_first{play(dir.path(),&steps[cut..],Some(sid.into())).await}else{rails_leg(boot.path(),dir.path(),&steps[cut..],Some(sid))};
        let mut responses=first["responses"].as_array().unwrap().clone();responses.extend(second["responses"].as_array().unwrap().clone());second["responses"]=json!(responses);
        assert_eq!(comparable(second),comparable(expected.clone()),"cut {cut}, rails_first {rails_first}");
    }}
}
#[tokio::test(flavor="current_thread")]
async fn every_unported_name_stays_absent_even_if_granted_and_enabled(){
    let root=tempfile::tempdir().unwrap();let boot=tempfile::tempdir().unwrap();grid(boot.path(),root.path());
    let dir=&root.path().join(".template");
    let c=rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let all:Vec<_>=deltabadger::web::oauth::TOOL_DEFAULTS.iter().map(|(n,_)|*n).collect();
    let overrides:serde_json::Map<String,Value>=all.iter().map(|n|(n.to_string(),json!(true))).collect();
    c.execute("UPDATE users SET mcp_settings=?1",[json!({"tool_permissions":overrides}).to_string()]).unwrap();
    c.execute("UPDATE connected_clients SET mcp_tools=?1",[json!(all).to_string()]).unwrap();
    drop(c);
    let mut steps=vec![initialize(),request("POST",json!({"jsonrpc":"2.0","method":"notifications/initialized"}),true),rpc("tools/list",json!({}))];
    for name in &all{if !deltabadger::web::mcp::tools::NAMES.contains(name){steps.push(rpc("tools/call",json!({"name":name,"arguments":{}})));}}
    let out=play(dir,&steps,None).await;
    let listed:Value=serde_json::from_str(out["responses"][2]["body"].as_str().unwrap()).unwrap();
    let names:Vec<_>=listed["result"]["tools"].as_array().unwrap().iter().map(|v|v["name"].as_str().unwrap()).collect();
    assert_eq!(names,deltabadger::web::mcp::tools::NAMES);
    for r in out["responses"].as_array().unwrap().iter().skip(3){let body:Value=serde_json::from_str(r["body"].as_str().unwrap()).unwrap();assert_eq!(body["error"]["code"],-32602);}
}
#[tokio::test(flavor="current_thread")]
async fn a_refresh_and_revocation_cross_the_actual_mcp_endpoint(){
    let root=tempfile::tempdir().unwrap();let boot=tempfile::tempdir().unwrap();grid(boot.path(),root.path());
    let dir=&root.path().join(".template");
    let c=rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let uid:String=c.query_row("SELECT uid FROM oauth_applications WHERE id=1",[],|r|r.get(0)).unwrap();drop(c);
    let token_request=|path:&str,body:Value|json!({"method":"POST","path":path,"headers":{"Content-Type":"application/json"},"body":body.to_string()});
    let issued=play(dir,&[token_request("/oauth/token",json!({"grant_type":"refresh_token","client_id":uid,"refresh_token":"refresh-m2-token"}))],None).await;
    assert_eq!(issued["responses"][0]["status"],200);
    let pair:Value=serde_json::from_str(issued["responses"][0]["body"].as_str().unwrap()).unwrap();
    let mut init=initialize();init["headers"]["Authorization"]=json!(format!("Bearer {}",pair["access_token"].as_str().unwrap()));
    let accepted=rails_leg(boot.path(),dir,&[init.clone()],None);assert_eq!(accepted["responses"][0]["status"],200);
    let handed_back=rails_leg(boot.path(),dir,&[token_request("/oauth/token",json!({"grant_type":"refresh_token","client_id":uid,"refresh_token":pair["refresh_token"]}))],None);
    assert_eq!(handed_back["responses"][0]["status"],200);
    let next:Value=serde_json::from_str(handed_back["responses"][0]["body"].as_str().unwrap()).unwrap();
    init["headers"]["Authorization"]=json!(format!("Bearer {}",next["access_token"].as_str().unwrap()));
    let accepted=play(dir,&[init.clone()],None).await;assert_eq!(accepted["responses"][0]["status"],200);
    // M1's already-approved retirement is inherited, not reimplemented: the presented chain is retired.
    let c=rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM oauth_access_tokens WHERE token IN ('m2-token',?1) AND revoked_at IS NOT NULL",[pair["access_token"].as_str().unwrap()],|r|r.get::<_,i64>(0)).unwrap(),2);drop(c);
    let revoked=play(dir,&[token_request("/oauth/revoke",json!({"client_id":uid,"token":next["access_token"]}))],None).await;assert_eq!(revoked["responses"][0]["status"],200);
    let rails=rails_leg(boot.path(),dir,&[init.clone()],None);let rust=play(dir,&[init],None).await;
    assert_eq!(comparable(rust),comparable(rails));
}
#[test]
fn stale_registry_gate_has_both_refusal_texts(){
    let dir=common::rails_install();let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    common::seed::seed_alpaca(&c,&common::seed::cipher());
    let who=deltabadger::web::bearer::Bearer{user_id:1,application_id:999,token_id:999};
    let refusal=deltabadger::web::mcp::tools::gate(&c,who,"list_bots").unwrap().unwrap();
    assert_eq!(refusal,deltabadger::web::mcp::protocol::tool_text("Tool 'list_bots' is not available to this client. Grant it in Settings > Connect.",true));
    c.execute("UPDATE users SET mcp_settings=?1",[r#"{"tool_permissions":{"list_bots":false}}"#]).unwrap();
    let refusal=deltabadger::web::mcp::tools::gate(&c,who,"list_bots").unwrap().unwrap();
    assert_eq!(refusal,deltabadger::web::mcp::protocol::tool_text("Tool 'list_bots' is disabled. Enable it in Settings > MCP.",true));
}
#[tokio::test(flavor="current_thread")]
async fn slow_body_keeps_the_existing_deadline_and_no_token_is_checked_first(){
    let (dir,opened,_)=common::install_alpaca();drop(opened);
    let app=web::app(dir.path(),web::SECRET,TestClock::at(AT));
    let pending=||Body::from_stream(futures_util::stream::pending::<Result<Vec<u8>,std::io::Error>>());
    let build=|auth:bool|{
        let mut r=Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("content-type","application/json");
        if auth{r=r.header("authorization","Bearer unknown");}r.body(pending()).unwrap()
    };
    let no_token=deltabadger::web::mcp::entry(app.clone(),build(false),std::time::Duration::from_millis(1)).await;
    assert_eq!(no_token.status(),401);
    let timed_out=deltabadger::web::mcp::entry(app,build(true),std::time::Duration::from_millis(1)).await;
    assert_eq!(timed_out.status(),408);assert_eq!(timed_out.headers()["connection"],"close");
}
