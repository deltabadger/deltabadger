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
    for f in ["production.sqlite3", "production_queue.sqlite3", "secret_key_base"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}
async fn play(dir: &Path, steps: &[Value], mut session: Option<String>) -> Value {
    let app = web::app(dir, &std::fs::read_to_string(dir.join("secret_key_base")).unwrap(), TestClock::at(AT));
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
    for table in ["action_mcp_sessions","action_mcp_session_messages","action_mcp_session_subscriptions","oauth_access_tokens","connected_clients","bots","transactions","api_keys","bot_index_assets","bot_activity_logs","users"] {
        let mut q=c.prepare(&format!("SELECT * FROM {table} ORDER BY id")).unwrap();
        let names:Vec<String>=q.column_names().iter().map(|s|s.to_string()).collect();
        let rows=q.query_map([],|r|{
            let mut row=serde_json::Map::new();
            for (i,n) in names.iter().enumerate(){let value=match r.get_ref(i)? {rusqlite::types::ValueRef::Null=>Value::Null,rusqlite::types::ValueRef::Integer(n)=>json!(n),rusqlite::types::ValueRef::Real(n)=>json!({"float_bits":format!("{:016x}",n.to_bits())}),rusqlite::types::ValueRef::Text(v)=>json!(std::str::from_utf8(v).unwrap()),_=>panic!("unexpected blob")};let value = if (table == "bots" && ["settings","transient_data"].contains(&n.as_str()) || table == "bot_activity_logs" && n == "details") && value.is_string() { serde_json::from_str(value.as_str().unwrap()).unwrap() } else { value }; row.insert(n.clone(),value);}
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
async fn unregistered_tools_return_unknown_tool() {
    let root=tempfile::tempdir().unwrap();let boot=tempfile::tempdir().unwrap();
    common::rails(boot.path(),"test",&["runner","script/rust/mcp_control.rb","grid",root.path().to_str().unwrap()]);
    let dir=std::fs::read_dir(root.path()).unwrap().map(|e|e.unwrap().path()).find(|p|p.join("steps.json").is_file()).unwrap();
    let ready=read(dir.join("steps.json"));let mut steps=ready.as_array().unwrap()[..2].to_vec();
    let names:Vec<_>=["stop_bot","archive_bot","unarchive_bot","delete_bot","update_bot_settings","start_bot","create_bot","create_index_bot","create_signal_bot"].into_iter().filter(|n|!deltabadger::web::mcp::tools::NAMES.contains(n)).collect();
    assert!(names.len()>=3);
    for name in &names {steps.push(json!({"method":"POST","path":"/mcp","headers":{"Authorization":"Bearer m2-token","Content-Type":"application/json","Accept":"application/json, text/event-stream","Mcp-Session-Id":"$session"},"body":json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":{"bot_id":1}}}).to_string()}));}
    let output=play(&dir,&steps,None).await;
    for (name,response) in names.iter().zip(output["responses"].as_array().unwrap().iter().skip(2)) {
        let body:Value=serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["error"]["code"],-32602,"absent {name}: {body}");
    }
}

#[tokio::test(flavor="current_thread")]
async fn control_transcripts() {
    let root = tempfile::tempdir().unwrap();
    let boot = tempfile::tempdir().unwrap();
    common::rails(boot.path(), "test", &["runner", "script/rust/mcp_control.rb", "grid", root.path().to_str().unwrap()]);
    let dirs: Vec<_> = std::fs::read_dir(root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.join("steps.json").is_file()).collect();
    assert!(!dirs.is_empty());
    let mut results = vec![];
    for dir in &dirs {
        let steps = read(dir.join("steps.json"));
        let rust = tempfile::tempdir().unwrap();
        copy_install(dir, rust.path());
        let unicode_symbol=dir.file_name().unwrap()=="settings_r2_unicode_symbol";
        let before=if unicode_symbol {
            let c=rusqlite::Connection::open(rust.path().join("production.sqlite3")).unwrap();
            for step in steps.as_array().unwrap() {if let Some(sql)=step["sql"].as_array() {for command in sql {c.execute_batch(command.as_str().unwrap()).unwrap();}}}
            Some(snapshot(rust.path()))
        } else {None};
        let mut result=play(rust.path(),steps.as_array().unwrap(),None).await;
        if let Some(before)=before {result["before_business"]=before;}
        results.push(result);
    }
    common::rails(boot.path(), "test", &["runner", "script/rust/mcp_control.rb", "record", root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for (dir,got) in dirs.iter().zip(results) {
        let want = read(dir.join("rails.json"));
        let (mut got,want)=(comparable(got),comparable(want));
        // Plan 3c divergence 9: the engine is held for the HTTP snapshot. Assert
        // the exact request, then compare every other primary column and JSON key.
        let mut requests=0;
        for (row,rails) in got["rows"]["bots"].as_array_mut().unwrap().iter_mut().zip(want["rows"]["bots"].as_array().unwrap()) {
            if let Some(request)=row["transient_data"].as_object_mut().unwrap().shift_remove("rust_continue_start") {
                requests+=1;
                assert_eq!(request,json!({"requested_at":AT,"was_stopped":rails["status"]==1}));
                assert_eq!(rails["status"],1);
            }
        }
        let name=dir.file_name().unwrap().to_string_lossy();
        if name=="registry" {
            let body:Value=serde_json::from_str(got["responses"][2]["body"].as_str().unwrap()).unwrap();
            let names:Vec<_>=body["result"]["tools"].as_array().unwrap().iter().map(|t|t["name"].as_str().unwrap()).collect();
            assert_eq!(names,deltabadger::web::mcp::tools::NAMES,"complete registry without a cursor");
            assert!(body["result"].get("nextCursor").is_none());
            if let Ok(root)=std::env::var("M4_EVIDENCE") {
                std::fs::write(std::path::Path::new(&root).join("registry-pages.json"),serde_json::to_string_pretty(&json!({"rails":want,"rust":got})).unwrap()).unwrap();
            }
        }
        if name == "stop_bot_r1_invalid" {
            let response:Value=serde_json::from_str(want["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(response["result"]["content"][0]["text"],"Failed to stop bot 'Control'.");
            assert_eq!(want["rows"]["bots"][0]["status"],1);
            let response:Value=serde_json::from_str(got["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(response["result"]["content"][0]["text"],"Bot 'Control' stopped.");
            assert_eq!(response["result"]["isError"],Value::Null);
            assert_eq!(got["rows"]["bots"][0]["status"],2);
            assert_eq!(got["rows"]["bots"][0]["settings"],want["rows"]["bots"][0]["settings"]);
            continue; // R1 named safety divergence; no row normalization claims parity.
        }
        if name.starts_with("start_bot_r1_key_") {
            let rails:Value=serde_json::from_str(want["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(rails["result"]["isError"],Value::Null);
            assert_eq!(want["rows"]["bots"][0]["status"],1);
            let response:Value=serde_json::from_str(got["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(response["result"]["isError"],true,"R1 key refusal {name}");
            assert!(response["result"]["content"][0]["text"].as_str().unwrap().starts_with("This app can't run that yet:"));
            assert_eq!(got["rows"]["bots"][0]["status"],0);
            continue; // Rails arms a bot without a ready key; explicit guard divergence.
        }
        if name=="settings_r2_unicode_symbol" {
            let rails:Value=serde_json::from_str(want["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(rails["result"]["content"][0]["text"],"Bot 'Control' settings updated: allocations.","R2 Unicode Rails accepts");
            assert_eq!(want["rows"]["bots"][0]["settings"]["allocations"],json!({"16":0.7,"17":0.3}));
            let response:Value=serde_json::from_str(got["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(response["result"]["content"][0]["text"],"ſOL is not in this basket; membership cannot be changed here.","R2 Unicode ASCII-only refusal");
            assert_eq!(response["result"]["isError"],true,"R2 Unicode refusal classification");
            for table in ["oauth_access_tokens","connected_clients","bots","transactions","api_keys","bot_index_assets","bot_activity_logs","users"] {
                assert_eq!(got["rows"][table],got["before_business"][table],"R2 Unicode no commit: {table}");
            }
            if let Ok(path)=std::env::var("M4_EVIDENCE") {
                let path=std::path::Path::new(&path).join("r2-unicode-symbol.json");
                std::fs::write(path,serde_json::to_vec_pretty(&json!({"rails":want,"rust":got})).unwrap()).unwrap();
            }
            continue; // BUILD.md explicitly keeps ASCII lookup; no parity normalization.
        }
        if name.starts_with("settings_r2_negative_carry_") || name=="start_bot_r2_negative_carry" {
            let rails:Value=serde_json::from_str(want["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert!(rails["result"]["content"][0]["text"].as_str().unwrap().contains(if name.starts_with("settings_") {"settings updated:"} else {"started successfully."}),"R2 carry Rails accepts {name}");
            if let Ok(path)=std::env::var("M4_EVIDENCE") {
                std::fs::write(std::path::Path::new(&path).join(format!("{name}.json")),serde_json::to_vec_pretty(&json!({"rails":want,"rust":got})).unwrap()).unwrap();
            }
        }
        let continuation=["start_bot_status_2","start_bot_fractional_id","start_continue_empty","start_continue_within"].contains(&name.as_ref());
        assert_eq!(requests,usize::from(continuation),"{name}");
        if got!=want {
            failures.push(dir.file_name().unwrap().to_string_lossy().to_string());
            for (i,(g,w)) in got["responses"].as_array().unwrap().iter().zip(want["responses"].as_array().unwrap()).enumerate() {
                if g!=w { eprintln!("{} step {i} got {} want {}",dir.display(),g["body"],w["body"]); break; }
            }
            for (table,rows) in got["rows"].as_object().unwrap() {
                if rows!=&want["rows"][table] { eprintln!("{} table {table} got {rows} want {}",dir.display(),want["rows"][table]); }
            }
        }
    }
    assert!(failures.is_empty(), "M4 mismatches: {failures:?}");
}
