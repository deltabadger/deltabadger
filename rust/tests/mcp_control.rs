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
        let unicode_symbol=dir.file_name().unwrap()=="settings_r2_unicode_symbol" || dir.file_name().unwrap().to_string_lossy().ends_with("schedule_bounds");
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
        if name.ends_with("schedule_bounds") {
            let rails:Value=serde_json::from_str(want["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert!(rails["result"]["content"][0]["text"].as_str().unwrap().contains(if name.starts_with("settings_") {"settings updated:"} else {"started successfully."}));
            let response:Value=serde_json::from_str(got["responses"][2]["body"].as_str().unwrap()).unwrap();
            let expected=if name.starts_with("settings_") {"Failed to update bot: Quote amount The invested amount must be greater than 0"} else {"Failed to start bot 'Control': Quote amount The invested amount must be greater than 0"};
            assert_eq!(response["result"]["content"][0]["text"],expected);
            assert_eq!(response["result"]["isError"],true);
            for table in ["bots","transactions","api_keys","bot_index_assets","bot_activity_logs","users"] {
                assert_eq!(got["rows"][table],got["before_business"][table],"schedule refusal rolls back {name}: {table}");
            }
            if let Ok(path)=std::env::var("M4_EVIDENCE") {
                std::fs::write(std::path::Path::new(&path).join(format!("{name}.json")),serde_json::to_vec_pretty(&json!({"rails":want,"rust":got})).unwrap()).unwrap();
            }
            continue;
        }
        if name=="registry" {
            let body:Value=serde_json::from_str(got["responses"][2]["body"].as_str().unwrap()).unwrap();
            let names:Vec<_>=body["result"]["tools"].as_array().unwrap().iter().map(|t|t["name"].as_str().unwrap()).collect();
            assert_eq!(names,deltabadger::web::mcp::tools::NAMES,"complete registry without a cursor");
            assert!(body["result"].get("nextCursor").is_none());
            if let Ok(root)=std::env::var("M4_EVIDENCE") {
                std::fs::write(std::path::Path::new(&root).join("registry-pages.json"),serde_json::to_string_pretty(&json!({"rails":want,"rust":got})).unwrap()).unwrap();
            }
        }
        if name.starts_with("paper_refused_") {
            // ApplicationMCPTool: paper trading refuses start_bot before lookup, validation or any write.
            let rails:Value=serde_json::from_str(want["responses"][2]["body"].as_str().unwrap()).unwrap();
            assert_eq!(rails["result"]["content"][0]["text"],"[DRY RUN] Paper trading is on, so nothing was created or started: 'start_bot' would leave a bot or rule running that moves real money. Turn Paper Trading off in Settings > MCP to use it.","{name}");
            assert_eq!(rails["result"]["isError"],true,"{name}");
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
            // #507 rejects missing/pending/incorrect keys in production validation, before arming a tick.
            for transcript in [&want, &got] {
                let response:Value=serde_json::from_str(transcript["responses"][2]["body"].as_str().unwrap()).unwrap();
                assert_eq!(response["result"]["isError"],Value::Null,"Rails validation classification {name}");
                assert_eq!(response["result"]["content"][0]["text"],"Failed to start bot 'Control': A valid trading API key is required to start this bot.","{name}");
                assert_eq!(transcript["rows"]["bots"][0]["status"],0,"key refusal does not arm {name}");
            }
            // Fall through to the full transcript/row comparison: no normalization or divergence.
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

#[derive(Clone)]
struct PriceBarrier {
    transport:deltabadger::venue::http::ScriptedTransport,
    entered:std::rc::Rc<tokio::sync::Notify>, release:std::rc::Rc<tokio::sync::Notify>, held:std::rc::Rc<std::cell::Cell<bool>>,
    sent:std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
}
impl deltabadger::venue::http::Transport for PriceBarrier {
    async fn send(&self,request:&deltabadger::venue::http::HttpRequest)->Result<deltabadger::venue::http::HttpResponse,deltabadger::venue::http::TransportError> {
        if request.path=="/v1beta3/crypto/us/latest/quotes" && !self.held.replace(true) {
            self.entered.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(20),self.release.notified()).await.map_err(|_|deltabadger::venue::http::TransportError::Permanent("price barrier expired".into()))?;
        }
        if request.method=="POST" {self.sent.borrow_mut().push(request.body.clone().unwrap());}
        self.transport.send(request).await
    }
}
impl deltabadger::venue::VenueFactory for PriceBarrier {
    type V=deltabadger::venue::alpaca::AlpacaVenue<Self>;
    fn for_bot(&self,_:&str,credentials:Option<deltabadger::crypto::Credentials>)->Self::V {
        deltabadger::venue::alpaca::AlpacaVenue::new(self.clone(),deltabadger::venue::alpaca::Urls::for_passphrase(credentials.as_ref().and_then(|c|c.passphrase.as_deref())))
    }
}
fn changes(before:&Value,after:&Value)->Value {
    let mut changes=serde_json::Map::new();
    for table in ["bots","bot_index_assets","bot_activity_logs","transactions","api_keys","users"] {
        let mut rows=vec![];
        for row in after[table].as_array().unwrap() {
            let old=before[table].as_array().unwrap().iter().find(|r|r["id"]==row["id"]);
            let delta:serde_json::Map<String,Value>=row.as_object().unwrap().iter().filter(|(key,value)|old.is_none_or(|r|r[*key]!=**value)).map(|(key,value)|(key.clone(),value.clone())).collect();
            if !delta.is_empty(){rows.push(json!({"id":row["id"],"delta":delta}));}
        }
        if !rows.is_empty(){changes.insert(table.into(),json!(rows));}
    }
    json!(changes)
}
#[tokio::test(flavor="current_thread")]
async fn real_rails_ticks_and_mcp_tools_race_on_identical_files() {
    use deltabadger::{crypto::{Cipher,EncryptionKeys},engine::{model,tick,provider},venue::alpaca::{AlpacaVenue,Urls}};
    use std::rc::Rc;
    let root=tempfile::tempdir().unwrap();
    let output=root.path().join("record");
    let boot=tempfile::tempdir().unwrap();
    common::rails(boot.path(),"test",&["runner","script/rust/mcp_control_races.rb",output.to_str().unwrap()]);
    let lost=read(output.join("settings_lost_update/race.json"));
    assert_eq!((lost["read"].as_f64(),lost["committed"].as_f64(),lost["final"].as_f64()),(Some(50.0),Some(20.0),Some(20.0)),"the rename keeps the amount committed after its load");
    for name in deltabadger::web::mcp::control::NAMES {
        let dir=output.join(name);let rails=read(dir.join("race.json"));
        let rust=tempfile::tempdir().unwrap();
        copy_install(&dir,rust.path());
        std::fs::copy(dir.join("before.sqlite3"),rust.path().join("production.sqlite3")).unwrap();
        let ready:Vec<Value>=vec![
            json!({"method":"POST","path":"/mcp","headers":{"Authorization":"Bearer m2-token","Content-Type":"application/json","Accept":"application/json, text/event-stream"},"body":json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"M4 race","version":"1"}}}).to_string()}),
            json!({"method":"POST","path":"/mcp","headers":{"Authorization":"Bearer m2-token","Content-Type":"application/json","Accept":"application/json, text/event-stream","Mcp-Session-Id":"$session"},"body":json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string()})];
        let init=play(rust.path(),&ready,None).await;
        let c=rusqlite::Connection::open(rust.path().join("production.sqlite3")).unwrap();
        let secret=std::fs::read_to_string(rust.path().join("secret_key_base")).unwrap();
        let cipher=Cipher::new(&EncryptionKeys::resolve(&|_|None,&secret).unwrap());
        provider::bind(&c,&cipher,&|_|None).unwrap();
        let script=PriceBarrier{transport:common::scripted::script(json!({})),entered:Rc::new(tokio::sync::Notify::new()),release:Rc::new(tokio::sync::Notify::new()),held:Rc::new(std::cell::Cell::new(false)),sent:Rc::new(std::cell::RefCell::new(vec![]))};
        let venue=AlpacaVenue::new(script.clone(),Urls::for_passphrase(Some("paper")));
        let clock=TestClock::at(AT);let mut attempts=tick::Attempts::default();
        let future=tick::tick(&c,&venue,1,&*clock,&mut attempts);tokio::pin!(future);
        tokio::select!{_ = script.entered.notified()=>{},result=&mut future=>panic!("{name}: ended before price: {result:?}"),_ = tokio::time::sleep(std::time::Duration::from_secs(20))=>panic!("{name}: no price barrier")}
        let before=snapshot(rust.path());
        let step=read(dir.join("tool.json"));
        let got=play(rust.path(),&[step],Some(init["session"].as_str().unwrap().to_owned())).await;
        assert_eq!(got["responses"][0]["status"],rails["response"]["status"],"{name}");
        assert_eq!(got["responses"][0]["body"],rails["response"]["body"],"{name}: byte-equal racing response");
        assert_eq!(changes(&before,&got["rows"]),changes(&rails["before_tool"],&rails["at_tool"]),"{name}: exact tool write delta");
        script.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(20),&mut future).await.unwrap().unwrap();
        let safety=["stop_bot","archive_bot","delete_bot"].contains(&name);
        assert_eq!(rails["sent"].as_array().unwrap().len(),2,"Rails known pre-placement race");
        assert_eq!(script.sent.borrow().len(),if safety {0} else {2},"{name}: placement fence");
        let state=model::load_bot(&c,1).unwrap();
        assert_eq!(state.status as i64,rails["final"]["bots"][0]["status"].as_i64().unwrap(),"{name}: never resurrected");
        if !safety {
            for (got,want) in script.sent.borrow().iter().zip(rails["sent"].as_array().unwrap()) {
                assert_eq!(got["symbol"],want["symbol"]);
                let amount=|v:&Value|{let text=v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string());deltabadger::ruby::BigDec::parse(&text).unwrap()};
                assert_eq!(amount(&got["notional"]),amount(&want["notional"]),"{name}: exact scripted contribution");
            }
        }
    }
    // Fresh MCP start: complete HTTP and row parity, actual immediate scheduler,
    // exact first contribution, no second contribution at the same clock.
    let dir=output.join("fresh_start");let rails=read(dir.join("fresh.json"));
    assert_eq!(rails["job_at"],Value::Null);
    let rust=tempfile::tempdir().unwrap();copy_install(&dir,rust.path());
    std::fs::copy(dir.join("before.sqlite3"),rust.path().join("production.sqlite3")).unwrap();
    let steps=read(dir.join("steps.json"));
    let got=play(rust.path(),steps.as_array().unwrap(),None).await;
    assert_eq!(comparable(got),comparable(rails["tool"].clone()),"fresh start: all HTTP bytes and columns");
    let c=rusqlite::Connection::open(rust.path().join("production.sqlite3")).unwrap();
    let secret=std::fs::read_to_string(rust.path().join("secret_key_base")).unwrap();
    let cipher=Cipher::new(&EncryptionKeys::resolve(&|_|None,&secret).unwrap());provider::bind(&c,&cipher,&|_|None).unwrap();
    let mut polls=serde_json::Map::new();
    for (i,order) in rails["sent"].as_array().unwrap().iter().enumerate() {
        polls.insert(format!("GET /v2/orders/OTX-{}",i+1),json!([common::scripted::ok(json!({"id":format!("OTX-{}",i+1),"status":"new","symbol":order["symbol"],"type":"market","side":"buy","notional":order["notional"],"qty":null,"filled_qty":"0","filled_avg_price":null,"limit_price":null}))]));
    }
    let script=PriceBarrier{transport:common::scripted::script(json!(polls)),entered:Rc::new(tokio::sync::Notify::new()),release:Rc::new(tokio::sync::Notify::new()),held:Rc::new(std::cell::Cell::new(true)),sent:Rc::new(std::cell::RefCell::new(vec![]))};
    let clock=TestClock::at("2026-09-10T12:00:30.123457Z");
    let paths=deltabadger::store::Paths::from_env(&|_|None,rust.path());
    let lock=deltabadger::lease::lock(&paths,deltabadger::engine::Clock::now(&*clock)).unwrap();
    let mut engine=deltabadger::engine::run::Engine::new(c,script.clone(),cipher,lock);
    deltabadger::engine::run::step(&mut engine,&*clock).await.unwrap();
    assert_eq!(script.sent.borrow().len(),2,"fresh start immediately buys both legs");
    for (got,want) in script.sent.borrow().iter().zip(rails["sent"].as_array().unwrap()) {
        assert_eq!(got["symbol"],want["symbol"]);
        let amount=|value:&Value|deltabadger::ruby::BigDec::parse(&value.as_str().map(str::to_owned).unwrap_or_else(||value.to_string())).unwrap();
        assert_eq!(amount(&got["notional"]),amount(&want["notional"]),"fresh start exact contribution");
    }
    deltabadger::engine::run::step(&mut engine,&*clock).await.unwrap();
    assert_eq!(script.sent.borrow().len(),2,"fresh start never doubles at the same clock");
}
