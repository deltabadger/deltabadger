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
