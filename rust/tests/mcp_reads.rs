//! The same raw HTTP messages and fixed tokens on Rails-created SQLite databases.
mod common;
use common::web::{self, TestClock};
use axum::{body::{Body, to_bytes}, http::Request};
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt;
use base64::Engine;
const EXPECTED_READS: &[&str] = &["get_exchange_balances","list_open_orders","get_bot_details","get_portfolio_summary"];
const ALL_READS: &[&str] = &["get_exchange_balances","list_open_orders","get_bot_details","get_portfolio_summary"];
const AT: &str = "2026-09-10T12:00:30.123456Z";
const HEADERS: [&str; 12] = ["content-type", "www-authenticate", "mcp-session-id", "mcp-protocol-version", "allow", "cache-control", "x-frame-options", "x-xss-protection", "x-content-type-options", "x-permitted-cross-domain-policies", "referrer-policy", "content-security-policy-report-only"];
fn read(p: impl AsRef<Path>) -> Value { serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap() }
fn copy_install(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "market.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}
async fn play(dir: &Path, steps: &[Value], mut session: Option<String>) -> Value {
    let app = web::app(dir, web::SECRET, TestClock::at(AT));
    let script = if dir.join("market.json").is_file() {read(dir.join("market.json"))} else {json!({})};
    let app=app.with_figure_source(deltabadger::web::figure::loading::Source::Script(script)).unwrap();
    let router = deltabadger::web::router(app);
    let mut responses = vec![];
    for step in steps {
        if let Some(sql) = step["sql"].as_array() {
            let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
            for s in sql { c.execute_batch(s.as_str().unwrap()).unwrap(); }
        }
        // Capture every bot column after scenario setup, immediately before this read.
        let before_bots = snapshot(dir)["bots"].clone();
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
        assert_eq!(snapshot(dir)["bots"], before_bots, "MCP reads must leave every bot row byte-identical");
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
async fn four_reads_match_rails() {
    for name in ALL_READS {assert_eq!(deltabadger::web::mcp::tools::NAMES.contains(name),EXPECTED_READS.contains(name),"registry stage: {name}");}
    let root=tempfile::tempdir().unwrap(); let boot=tempfile::tempdir().unwrap();
    common::rails(boot.path(),"test",&["runner","script/rust/mcp.rb","reads_grid",root.path().to_str().unwrap()]);
    let mut actual=vec![];
    for e in std::fs::read_dir(root.path()).unwrap() {
        let dir=e.unwrap().path(); if !dir.join("steps.json").is_file(){continue;}
        let copy=tempfile::tempdir().unwrap(); copy_install(&dir,copy.path());
        let steps=read(dir.join("steps.json"));
        if dir.file_name().unwrap()=="m3_owner" { bounds_transport(&dir,&steps).await; }
        let got=play(copy.path(),steps.as_array().unwrap(),None).await;
        if EXPECTED_READS.contains(&"get_bot_details") && ["m3_r3_basket_legacy_buy","m3_r3_index_legacy_buy","m3_r3_basket_partial_sell","m3_r3_index_partial_sell"].iter().any(|n|dir.file_name().unwrap()==*n) {
            use deltabadger::{figures::{at::At,db::Subject,walk,page_market::{Cache,Reader}},web::figure};
            let c=rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
            let now=At::from_utc(chrono::DateTime::parse_from_rfc3339(AT).unwrap().to_utc()).unwrap();
            let s=Subject::load(&c,1).unwrap();
            assert!(format!("{:?}",walk::metrics(&c,&s,now)).contains("executed fill value unavailable"));
            let cache=Cache::default();let market=Reader::new(&cache,now.utc().timestamp());
            let page=figure::account(&c,1,&market,now,"en","","");
            assert!(format!("{page:?}").contains("executed fill value unavailable"),"pages cannot publish the skipped-fill figure");
            let rendered=figure::loading::render(&c,1,&figure::loading::Snapshot::Failed,"en","","").unwrap().unwrap();
            for key in ["tile","metrics","chart"]{assert!(rendered["bots"]["1"][key].as_str().unwrap().contains("Figures unavailable: executed fill value unavailable"));}
            assert!(rendered["account"].as_str().unwrap().contains("Figures unavailable: executed fill value unavailable"));
        }
        actual.push((dir,steps,got));
    }
    assert_eq!(actual.len(),97,"all recorded M3 scenarios");
    common::rails(boot.path(),"test",&["runner","script/rust/mcp.rb","record",root.path().to_str().unwrap()]);
    if let Ok(root)=std::env::var("M3_RECORDINGS"){
        for (dir,_,got) in &actual {
            let destination=std::path::Path::new(&root).join(dir.file_name().unwrap());
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::write(destination.join("rust.json"),serde_json::to_string_pretty(got).unwrap()).unwrap();
            for file in ["rails.json","steps.json"]{std::fs::copy(dir.join(file),destination.join(file)).unwrap();}
        }
    }
    let mut failed=vec![];
    for (dir,steps,got) in actual {
        let name=dir.file_name().unwrap().to_str().unwrap();
        let mut want=read(dir.join("rails.json"));
        if ["m3_priced_fill_control","m3_basket_priced_fill","m3_index_priced_fill"].contains(&name) {
            let texts=want["responses"].as_array().unwrap().iter().map(|r|r["body"].as_str().unwrap()).collect::<Vec<_>>().join(" ");
            assert!(texts.contains("Global P/L: +15.0%") && texts.contains("+$30.0"),"$200 + $30 = $230 priced control");
        }
        if name.ends_with("_liquidated_other") {
            assert!(want["responses"].to_string().contains("Global P/L: +10.0%"));
        }
        let oracle_texts=want["responses"].as_array().unwrap().iter().skip(2).map(|r|{
            let body:Value=serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
            body["result"]["content"][0]["text"].as_str().unwrap_or("").to_string()
        }).collect::<Vec<_>>();
        if name.starts_with("m3_start_") && name!="m3_start_combined" {
            let inactive=name.starts_with("m3_start_index_");
            assert_eq!(oracle_texts[0].contains("Started:"),inactive,"{name}: pending condition");
            assert!(oracle_texts[1].contains(if inactive{"Started: 2026-03-02 15:30 UTC"}else{"Started: 2026-09-09 16:00 UTC"}),"{name}: later condition");
            assert!(oracle_texts[2].contains("Started: 2026-03-02 15:30 UTC"),"{name}: older condition");
        }
        if ["m3_pair_locked","m3_signal_locked"].contains(&name){assert!(oracle_texts[0].contains("Locked out of buying (wash sale): AAA 10d"));}
        if name.ends_with("_known_price") && EXPECTED_READS.len()==4 {
            let texts=got["responses"].to_string();
            assert!(texts.contains("Global P/L: +15.0%") && texts.contains("+$30.0"),"{name}: normalized $230/+15% required");
        }
        if name=="m3_order_identity" {
            let text=got["responses"].to_string();
            assert!(text.contains("known-tombstone") && text.contains("AAA/USD"));
            assert!(!text.contains("wrong-class") && !text.contains("ext: unknown"));
        }
        let replacements=match name {
            "m3_other_venue_figures"=>vec![("Portfolio Summary\n================\nTotal bots: 3 (3 active, 0 stopped, 0 not started)\n\nGlobal P/L: +1.14%\nProfit (USD): +$9.64\n\n--- Per-Bot Summary ---\n- Basket (AAA+BBB/USD) | scheduled | P/L: +0.08% | Invested: 200.0 USD\n- Limited (AAA/USD) | scheduled | P/L: +3.64% | Invested: 346.99 USD\n- ND100 (N/A) | scheduled | P/L: -1.05% | Invested: 300.0 USD","this build reads Alpaca only")],
            "m3_other_venue"=>vec![("Failed to fetch balances from Kraken: EAPI:Invalid key","this build reads Alpaca only")],
            "m3_unpriced"=>vec![("Global P/L: -13.83%\nProfit (USD): $-117.15","Global P/L: Not available (needs market data)")],
            "m3_stored_zero_price"=>vec![("AAA/USD @ 0.0 (Market order)","AAA/USD  (Market order)"),("@ 0.0 USD","")],
            "m3_market_no_price"=>vec![("AAA/USD @ 0 (Market order)","AAA/USD  (Market order)")],
            "m3_limit_no_price"=>vec![("AAA/USD @ 0 (Limit order)","AAA/USD  (Limit order)")],
            "m3_empty_redeploy_minimum"=>vec![("Redeploy offer (answer with answer_redeploy_offer): 0.01 USD","Redeploy unavailable: no composition minimum")],
            "m3_r3_single_partial_sell"|"m3_r3_signal_partial_sell"=>vec![("--- Performance ---\nTotal invested: 200.0 USD\nCurrent value: 200.0 USD\nP/L: +0.0%\nAverage buy price: 100.0 USD\nTotal acquired: 2.0 AAA","Metrics unavailable: executed fill value unavailable"),("Global P/L: +20.0%\nProfit (USD): +$40.0","Global P/L: Not available (executed fill value unavailable)"),("- Basket (AAA/USD) | scheduled | P/L: +0.0% | Invested: 200.0 USD","- Basket (AAA/USD) | scheduled | Metrics unavailable: executed fill value unavailable")],
            "m3_r3_basket_legacy_buy"=>vec![("Holdings (sellable with liquidate_exited_asset): AAA\n",""),("--- Performance ---\nTotal invested: 200.0 USD\nCurrent value: 200.0 USD\nP/L: +0.0%\nAverage buy price: 100.0 USD\nTotal acquired: 2.0 AAA","Metrics unavailable: executed fill value unavailable"),("Global P/L: +20.0%\nProfit (USD): +$40.0","Global P/L: Not available (executed fill value unavailable)"),("- Basket (AAA/USD) | scheduled | P/L: +0.0% | Invested: 200.0 USD","- Basket (AAA/USD) | scheduled | Metrics unavailable: executed fill value unavailable")],
            "m3_r3_index_legacy_buy"=>vec![("Holdings (sellable with liquidate_exited_asset): AAA\n",""),("--- Performance ---\nTotal invested: 200.0 USD\nCurrent value: 200.0 USD\nP/L: +0.0%","Metrics unavailable: executed fill value unavailable"),("Global P/L: +20.0%\nProfit (USD): +$40.0","Global P/L: Not available (executed fill value unavailable)"),("- Basket (N/A) | scheduled | P/L: +0.0% | Invested: 200.0 USD","- Basket (N/A) | scheduled | Metrics unavailable: executed fill value unavailable")],
            "m3_r3_basket_partial_sell"=>vec![("Holdings (sellable with liquidate_exited_asset): AAA\n",""),("--- Performance ---\nTotal invested: 200.0 USD\nCurrent value: 200.0 USD\nP/L: +0.0%\nAverage buy price: 100.0 USD\nTotal acquired: 1.0 AAA","Metrics unavailable: executed fill value unavailable"),("Global P/L: +10.0%\nProfit (USD): +$20.0","Global P/L: Not available (executed fill value unavailable)"),("- Basket (AAA/USD) | scheduled | P/L: +0.0% | Invested: 200.0 USD","- Basket (AAA/USD) | scheduled | Metrics unavailable: executed fill value unavailable")],
            "m3_r3_index_partial_sell"=>vec![("Holdings (sellable with liquidate_exited_asset): AAA\n",""),("--- Performance ---\nTotal invested: 200.0 USD\nCurrent value: 200.0 USD\nP/L: +0.0%","Metrics unavailable: executed fill value unavailable"),("Global P/L: +10.0%\nProfit (USD): +$20.0","Global P/L: Not available (executed fill value unavailable)"),("- Basket (N/A) | scheduled | P/L: +0.0% | Invested: 200.0 USD","- Basket (N/A) | scheduled | Metrics unavailable: executed fill value unavailable")],
            "m3_r4_single_known_price"|"m3_r4_signal_known_price"|"m3_null_fill_price"|"m3_basket_null_fill"=>vec![("Current value: 200.0 USD\nP/L: +0.0%","Current value: 220.0 USD\nP/L: +10.0%"),("Total acquired: 2.0 AAA","Total acquired: 1.0 AAA"),("Global P/L: +20.0%\nProfit (USD): +$40.0","Global P/L: +15.0%\nProfit (USD): +$30.0"),("P/L: +0.0% | Invested: 200.0 USD","P/L: +10.0% | Invested: 200.0 USD")],
            "m3_r4_basket_known_price"=>vec![("Current value: 200.0 USD\nP/L: +0.0%","Current value: 220.0 USD\nP/L: +10.0%"),("Global P/L: +10.0%\nProfit (USD): +$20.0","Global P/L: +15.0%\nProfit (USD): +$30.0"),("P/L: +0.0% | Invested: 200.0 USD","P/L: +10.0% | Invested: 200.0 USD")],
            "m3_r4_index_known_price"=>vec![("Current value: 200.0 USD\nP/L: +0.0%","Current value: 220.0 USD\nP/L: +10.0%"),("Global P/L: +10.0%\nProfit (USD): +$20.0","Global P/L: +15.0%\nProfit (USD): +$30.0"),("P/L: +0.0% | Invested: 200.0 USD","P/L: +10.0% | Invested: 200.0 USD")],
            "m3_index_null_fill"=>vec![("Current value: 200.0 USD\nP/L: +0.0%","Current value: 220.0 USD\nP/L: +10.0%"),("Global P/L: +20.0%\nProfit (USD): +$40.0","Global P/L: +15.0%\nProfit (USD): +$30.0"),("P/L: +0.0% | Invested: 200.0 USD","P/L: +10.0% | Invested: 200.0 USD")],
            "m3_r3_single_held_other"|"m3_r3_signal_held_other"=>vec![("Portfolio Summary\n================\nTotal bots: 1 (1 active, 0 stopped, 0 not started)\n\nGlobal P/L: +10.0%\nProfit (USD): +$20.0\n\n--- Per-Bot Summary ---\n- Basket (AAA/USD) | scheduled | P/L: +10.0% | Invested: 200.0 USD","this build reads Alpaca only")],
            "m3_missing_quote"=>vec![("Global P/L: -25.46%\nProfit (USD): $-164.69","Global P/L: Not available (quote currency unavailable)\nProfit (USD): Not available (quote currency unavailable)")],
            _=>vec![],
        };
        for (from,to) in replacements { assert_eq!(replace_text(&mut want,from,to),2,"one response and its stored message: {name}"); }
        if name == "m3_stranded_offset" {
            // RULING-1: Rails loses this saved preference during a read. This one cell
            // is the only persisted-field exception; all other columns still compare.
            assert_eq!(got["rows"]["bots"][0]["id"], 1);
            assert_eq!(want["rows"]["bots"][0]["id"], 1);
            assert_eq!(got["rows"]["bots"][0]["redeploy_declined_offset"], 100);
            assert_eq!(want["rows"]["bots"][0]["redeploy_declined_offset"], 0);
            want["rows"]["bots"][0]["redeploy_declined_offset"] = json!(100);
        }
        if name.ends_with("_label") {
            let original=if name.ends_with("_null_label"){Value::Null}else{json!("   ")};
            let expected=if name.contains("_whole_category_"){ "S&P 500 · 10" }else if name.contains("_category_"){ "Layer 1 · 20" }else if name.contains("_index_"){ "ND10" }else{ "AAA Inc." };
            assert_eq!(want["rows"]["bots"][0]["label"],json!(expected),"{name}: Rails generated label: {}",want["responses"]);
            assert_eq!(got["rows"]["bots"][0]["label"],original,"{name}: stored label unchanged");
            want["rows"]["bots"][0]["label"]=original;
            // Rails update! timestamps this label write; Rust preserves the original timestamp.
            assert_ne!(want["rows"]["bots"][0]["updated_at"],got["rows"]["bots"][0]["updated_at"]);
            want["rows"]["bots"][0]["updated_at"]=got["rows"]["bots"][0]["updated_at"].clone();
        }
        let (got,want)=(comparable(got),comparable(want));
        if EXPECTED_READS.len()<ALL_READS.len(){
            for (i,step) in steps.as_array().unwrap().iter().enumerate(){
                let call:Value=serde_json::from_str(step["body"].as_str().unwrap()).unwrap();
                let tool=call["params"]["name"].as_str().unwrap_or("");
                if ALL_READS.contains(&tool)&&!EXPECTED_READS.contains(&tool){
                    let body:Value=serde_json::from_str(got["responses"][i]["body"].as_str().unwrap()).unwrap();
                    assert_eq!(body["error"]["code"],-32602,"{name} step {i}: tool not ported yet");
                }else{assert_eq!(got["responses"][i],want["responses"][i],"{name} step {i}");}
            }
        }else if got != want {
            failed.push(name.to_string());

            for (i,(g,w)) in got["responses"].as_array().unwrap().iter().zip(want["responses"].as_array().unwrap()).enumerate(){
                if g!=w {eprintln!("{name} step {i} got {g} want {w}");}
            }
        }
    }
    assert!(failed.is_empty(),"{failed:?}");
}

// Only exact, scenario-specific authorized text divergences change here.
fn replace_text(value:&mut Value,from:&str,to:&str)->usize {
    match value {
        Value::String(text)=>{
            if text.starts_with('{') {if let Ok(mut inner)=serde_json::from_str::<Value>(text){
                let n=replace_text(&mut inner,from,to);if n>0{*text=inner.to_string();}return n;
            }}
            let n=text.matches(from).count();*text=text.replace(from,to);n
        },
        Value::Array(a)=>a.iter_mut().map(|v|replace_text(v,from,to)).sum(),
        Value::Object(o)=>o.values_mut().map(|v|replace_text(v,from,to)).sum(),
        _=>0,
    }
}

// Exercise admission and final rendering through the real MCP transport as each tool enters the registry.
async fn bounds_transport(dir:&Path,steps:&Value){
    if !EXPECTED_READS.contains(&"list_open_orders"){return}
    let ready=&steps.as_array().unwrap()[..2];
    let calls=steps.as_array().unwrap().iter().filter(|s|s["body"].as_str().unwrap().contains("tools/call")).collect::<Vec<_>>();
    let order=(*calls.iter().find(|s|s["body"].as_str().unwrap().contains("list_open_orders")).unwrap()).clone();
    for venue in [false,true]{for n in [if venue{50}else{100},if venue{51}else{101}]{
        let copy=tempfile::tempdir().unwrap();copy_install(dir,copy.path());
        let c=rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
        if venue{
            c.execute("DELETE FROM transactions",[]).unwrap();
            let mut market=read(copy.path().join("market.json"));
            let value=&mut market["GET paper-api.alpaca.markets/v2/orders?limit=50&status=open"]["body"];
            let raw=value[0].clone();
            *value=json!((0..n).map(|i|{let mut r=raw.clone();r["id"]=json!(format!("venue-{i}"));r}).collect::<Vec<_>>());
            std::fs::write(copy.path().join("market.json"),market.to_string()).unwrap();
        }else{
            c.execute("DELETE FROM transactions WHERE id != (SELECT id FROM transactions WHERE status=0 AND external_status=1 LIMIT 1)",[]).unwrap();
            let columns=c.prepare("PRAGMA table_info(transactions)").unwrap().query_map([],|r|r.get::<_,String>(1)).unwrap().map(Result::unwrap).filter(|n|n!="id").collect::<Vec<_>>().join(",");
            for i in 1..n{let select=columns.split(',').map(|v|if v=="external_id"{format!("external_id||'-{i}'")}else{v.to_string()}).collect::<Vec<_>>().join(",");c.execute(&format!("INSERT INTO transactions({columns}) SELECT {select} FROM transactions LIMIT 1"),[]).unwrap();}
        }
        let got=play(copy.path(),&[ready.to_vec(),vec![order.clone()]].concat(),None).await;
        let body=got["responses"].as_array().unwrap().last().unwrap()["body"].as_str().unwrap();
        assert_eq!(body.contains("Read unavailable: request data exceeds read limits"),n>if venue{50}else{100},"order row cap: venue={venue} n={n} {body}");
    }}
    if EXPECTED_READS.contains(&"get_bot_details"){
        let detail=(*calls.iter().find(|s|s["body"].as_str().unwrap().contains("get_bot_details")).unwrap()).clone();
        for n in [16384,16385]{
            let copy=tempfile::tempdir().unwrap();copy_install(dir,copy.path());
            let c=rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
            c.execute("UPDATE bots SET label=?1 WHERE id=1",["x".repeat(n)]).unwrap();
            let got=play(copy.path(),&[ready.to_vec(),vec![detail.clone()]].concat(),None).await;
            let body=got["responses"].as_array().unwrap().last().unwrap()["body"].as_str().unwrap();
            assert_eq!(body.contains("Read unavailable: request data exceeds read limits"),n>16384,"stored string cap: n={n}");
        }
    }
    let copy=tempfile::tempdir().unwrap();copy_install(dir,copy.path());
    let c=rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    c.execute("DELETE FROM transactions WHERE id != (SELECT id FROM transactions WHERE status=0 AND external_status=1 LIMIT 1)",[]).unwrap();
    c.execute("UPDATE transactions SET external_id=?1",["x".repeat(1000)]).unwrap();
    let columns=c.prepare("PRAGMA table_info(transactions)").unwrap().query_map([],|r|r.get::<_,String>(1)).unwrap().map(Result::unwrap).filter(|n|n!="id").collect::<Vec<_>>().join(",");
    for i in 1..100{let select=columns.split(',').map(|v|if v=="external_id"{format!("external_id||'-{i}'")}else{v.to_string()}).collect::<Vec<_>>().join(",");c.execute(&format!("INSERT INTO transactions({columns}) SELECT {select} FROM transactions LIMIT 1"),[]).unwrap();}
    let got=play(copy.path(),&[ready.to_vec(),vec![order]].concat(),None).await;
    let body=got["responses"].as_array().unwrap().last().unwrap()["body"].as_str().unwrap();
    assert!(body.contains("Read unavailable: response exceeds 65536 bytes"),"final text cap {body}");
}

#[tokio::test(flavor="current_thread")]
async fn polled_open_market_zero_price_uses_the_nil_branch() {
    use common::seed::{self,BotSpec,TxSpec};
    use deltabadger::{engine::polling,venue::alpaca::parse_order,web::mcp::{reads,tools::Called}};
    let (dir,o,s)=common::install_alpaca();
    let bot=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(60.0,"2026-09-01 10:00:00"));
    let tx=seed::insert_tx(&o.primary,&s,bot,&TxSpec{status:0,external_status:Some(0),external_id:Some("polled-market".into()),order_type:0,amount:Some("2"),quote_amount:None,price:None,quote_amount_exec:None,amount_exec:None,created_at:"2026-09-10 12:00:00".into()});
    let state=parse_order("polled-market",&json!({"status":"new","side":"buy","type":"market","qty":"2","filled_qty":"0","symbol":"BTC/USD"})).unwrap();
    polling::apply_in(&o.primary,bot,tx,&state,true,AT.parse().unwrap()).unwrap();
    let stored:(f64,f64,i64)=o.primary.query_row("SELECT price,amount_exec,external_status FROM transactions WHERE id=?1",[tx],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(stored,(0.0,0.0,1),"polling persists its unknown sentinel");
    o.primary.execute("DELETE FROM api_keys",[]).unwrap();
    o.primary.execute("UPDATE users SET time_zone='UTC'",[]).unwrap();
    let app=web::app(dir.path(),web::SECRET,TestClock::at(AT));
    let before=snapshot(dir.path())["transactions"].clone();
    let Called::Fetch(fetch)=reads::plan(&o.primary,&app,s.user_id,"list_open_orders",&json!({})).unwrap() else{panic!("orders fetch")};
    let got=reads::finish(&o.primary,reads::fetch(&app,fetch).await).unwrap();
    assert_eq!(got["content"][0]["text"],format!("Open orders (1):\n- [2026-09-10 12:00] #{tx} BUY 2.0 BTC/USD  (Market order) | Alpaca | ext: polled-market"));
    // Read the actual engine-polled database through the unchanged Rails service/presenter.
    let boot=tempfile::tempdir().unwrap();
    let oracle=dir.path().join("polled-rails.json");
    common::rails(boot.path(),"test",&["runner",r#"ActiveRecord::Base.establish_connection(adapter: 'sqlite3',database: File.join(ARGV[0],'production.sqlite3')); Time.zone='UTC'; order=BotApi::Orders::ListOpen.call(user: User.find(ARGV[1])).data[:orders].sole; presenter=ListOpenOrdersTool.allocate; File.write(ARGV[2],JSON.generate(zero: presenter.send(:db_line,order),nil: presenter.send(:db_line,order.merge(price: nil))))"#,dir.path().to_str().unwrap(),&s.user_id.to_string(),oracle.to_str().unwrap()]);
    let rails=read(&oracle);
    let line=got["content"][0]["text"].as_str().unwrap().strip_prefix("Open orders (1):\n").unwrap();
    assert!(rails["zero"].as_str().unwrap().contains("@ 0.0"));
    assert_eq!(line,rails["nil"].as_str().unwrap(),"exact Rails no-price branch");
    if let Ok(root)=std::env::var("M3_RECORDINGS") {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(Path::new(&root).join("polled-engine.json"),serde_json::to_string_pretty(&json!({"stored_price":stored.0,"stored_amount_exec":stored.1,"external_status":stored.2,"rails_stored_zero":rails["zero"],"rails_nil_price":rails["nil"],"rust_stored_zero":line})).unwrap()).unwrap();
    }
    assert_eq!(snapshot(dir.path())["transactions"],before,"MCP preserves the persisted zero");
    o.primary.execute("UPDATE transactions SET price=NULL",[]).unwrap();
    let Called::Fetch(fetch)=reads::plan(&o.primary,&app,s.user_id,"list_open_orders",&json!({})).unwrap() else{panic!("orders fetch")};
    assert_eq!(reads::finish(&o.primary,reads::fetch(&app,fetch).await).unwrap(),got);
}

#[test]
fn mcp_transaction_zero_price_uses_the_nil_branch() {
    use common::seed::{self,BotSpec,TxSpec};
    use deltabadger::web::{bearer::Bearer,mcp::tools::{self,Called}};
    let (dir,o,s)=common::install_alpaca();
    let bot=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(60.0,"2026-09-01 10:00:00"));
    seed::insert_tx(&o.primary,&s,bot,&TxSpec{status:0,external_status:Some(2),external_id:Some("filled".into()),order_type:0,amount:Some("2"),quote_amount:None,price:None,quote_amount_exec:Some("200"),amount_exec:Some("2"),created_at:"2026-09-10 12:00:00".into()});
    o.primary.execute("UPDATE users SET time_zone='UTC'",[]).unwrap();
    o.primary.execute("INSERT INTO oauth_applications(name,uid,personal_access_token,personal_owner_id,created_at,updated_at) VALUES('test','test',1,?1,'2026-09-10','2026-09-10')",[s.user_id]).unwrap();
    let who=Bearer{user_id:s.user_id,application_id:o.primary.last_insert_rowid(),token_id:1};
    let app=web::app(dir.path(),web::SECRET,TestClock::at(AT));
    let read=||{
        let Called::Done(result)=tools::call(&o.primary,&app,who,"list_transactions",&json!({})).unwrap() else{panic!("local transaction read")};
        result["content"][0]["text"].as_str().unwrap().to_owned()
    };
    let absent=read();
    assert_eq!(absent,"Transactions (1):\n- [2026-09-10 12:00] BUY 2.0 BTC  (200.0 USD) | submitted");
    for zero in ["0","0.0","'0.000'","'-0.0'"] {
        o.primary.execute_batch(&format!("UPDATE transactions SET price={zero}")).unwrap();
        assert_eq!(read(),absent,"stored {zero} is unknown");
    }
    o.primary.execute_batch("UPDATE transactions SET quote_amount_exec=NULL").unwrap();
    assert_eq!(read(),"Transactions (1):\n- [2026-09-10 12:00] BUY 2.0 BTC   | submitted");
    o.primary.execute_batch("UPDATE transactions SET price=100.5").unwrap();
    assert!(read().contains("@ 100.5 USD"));
}

#[tokio::test(flavor="current_thread")]
async fn collision_mcp_identity_precedes_parsing() {
    use deltabadger::web::mcp::{reads,tools::Called};
    for identified in [false,true] {
        for defect in ["status","number","missing","bare_number"] {
            let (dir,o,s)=common::install_alpaca();
            let mut bad=json!({"id":"bad","symbol":if identified {"BTC/USD"} else {"UNKNOWN"},"asset_class":"crypto","status":"new","filled_qty":"0","qty":"2","side":"buy","type":"market"});
            match defect {
                "status" => bad["status"]=json!("future_status"),
                "number" => bad["filled_qty"]=json!("unreadable"),
                "missing" => {bad.as_object_mut().unwrap().remove("filled_qty");},
                _ => {},
            }
            let good=json!({"id":"good","symbol":"BTC/USD","asset_class":"crypto","status":"new","filled_qty":"0","qty":"2","side":"buy","type":"market"});
            let body=json!([bad,good]);
            let body=if defect=="bare_number" {json!(body.to_string().replacen("\"filled_qty\":\"0\"", "\"filled_qty\":1e-350",1))} else {body};
            let script=json!({"GET paper-api.alpaca.markets/v2/orders?limit=50&status=open":{"status":200,"body":body}});
            let app=web::app(dir.path(),"engine-test-secret",TestClock::at(AT)).with_figure_source(deltabadger::web::figure::loading::Source::Script(script)).unwrap();
            let Called::Fetch(fetch)=reads::plan(&o.primary,&app,s.user_id,"list_open_orders",&json!({})).unwrap() else{panic!("orders fetch")};
            let result=reads::finish(&o.primary,reads::fetch(&app,fetch).await);
            if identified && defect!="status" {
                assert!(result.is_err(),"identified {defect}: {result:?}");
            } else {
                let result=result.unwrap();
                let text=result["content"][0]["text"].as_str().unwrap();
                assert!(text.contains("ext: good"),"{text}");
                assert_eq!(text.contains("ext: bad"),identified,"{text}");
            }
        }
    }
}
