use crate::common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session, cable};
use rusqlite::{Connection, types::{Value as Sql, ValueRef}};
use serde_json::{json, Map, Value};
use std::path::Path;
#[derive(Clone,Copy)]
pub enum Mode { Exact, Deferred, FirstSync }
pub fn snapshot(c: &Connection) -> Value {
    let names: Vec<String> = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    let mut tables = Map::new();
    for name in names {
        let mut statement = c.prepare(&format!("SELECT * FROM {name} ORDER BY 1")).unwrap();
        let columns: Vec<String> = statement.column_names().iter().map(|s|s.to_string()).collect();
        let rows = statement.query_map([],|r| {
            let mut values = Map::new();
            for (i,column) in columns.iter().enumerate() {
                let value = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null, ValueRef::Integer(n) => json!(n), ValueRef::Real(n) => json!({"__sqlite_real__":format!("{:016x}",n.to_bits())}),
                    ValueRef::Text(s) => json!(String::from_utf8_lossy(s)), ValueRef::Blob(_) => panic!("unexpected blob in {name}.{column}"),
                };
                values.insert(column.clone(),value);
            }
            Ok(Value::Object(values))
        }).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        tables.insert(name,Value::Array(rows));
    }
    Value::Object(tables)
}
fn combined(answer: &Value, phase: &str) -> Value {
    let mut tables = answer[format!("rows_{phase}")].as_object().unwrap().clone();
    tables.extend(answer[format!("other_rows_{phase}")].as_object().unwrap().clone());
    Value::Object(tables)
}
fn scalar(value: &Value) -> Sql {
    match value { Value::Null => Sql::Null, Value::Bool(v) => Sql::Integer(i64::from(*v)),
        Value::Number(v) => v.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(v.as_f64().unwrap())),
        Value::Object(v) if v.contains_key("__sqlite_real__") => Sql::Real(f64::from_bits(u64::from_str_radix(v["__sqlite_real__"].as_str().unwrap(),16).unwrap())),
        Value::String(v) => Sql::Text(v.clone()), _ => Sql::Text(value.to_string()) }
}
fn same_rows(actual: Value, expected: Value, label: &str) {
    // Rails' action snapshot already serializes SQLite values, not ActiveRecord attributes.
    for (table,rows) in expected.as_object().unwrap() { assert_eq!(&actual[table],rows,"{label}: {table}"); }
    assert_eq!(actual.as_object().unwrap().len(),expected.as_object().unwrap().len());
}

// Mask only authenticated variable values; preserve every other byte and whitespace.
fn masked(body:&str,app:Option<&deltabadger::web::App>,token:Option<&str>)->String {
    let values=common::html::masked_values(body);let mut out=body.to_string();
    for v in values.tokens {if let Some(t)=token{assert!(csrf::valid(t,&v));}out=out.replace(&v,"[csrf]");}
    if let Some(rest)=body.split("<meta name=\"csp-nonce\" content=\"").nth(1){
        let nonce=rest.split('"').next().unwrap();assert!(!nonce.is_empty());out=out.replace(nonce,"[nonce]");
    }
    for v in values.assets {
        if app.is_some(){assert!(deltabadger::web::assets::find(&v).is_some(),"asset {v}");}
        if let Some((a,b))=v.rsplit_once('-'){if let Some((digest,ext))=b.split_once('.'){
            if (8..=64).contains(&digest.len()) && digest.bytes().all(|c|c.is_ascii_hexdigit()){out=out.replace(&v,&format!("{a}-[digest].{ext}"));}
        }}
    }
    for v in values.streams {
        let name=if let Some(app)=app {cable::verified_stream_name(&app.keys.streams,&v).expect("valid Rust stream")}
        else {use base64::Engine;let bytes=base64::engine::general_purpose::STANDARD.decode(v.split_once("--").unwrap().0).unwrap();serde_json::from_slice::<String>(&bytes).unwrap()};
        out=out.replace(&v,&format!("[signed {name}]"));
    }
    out
}

pub async fn check_grid(script:&str,variable:&str,count:usize,selected:&[&str],mode:fn(&str)->Mode) {
    let generated;
    let root=match std::env::var(variable) {
        Ok(root)=>root,
        Err(_)=>{let boot=tempfile::tempdir().unwrap();generated=tempfile::tempdir().unwrap();let root=generated.path().to_str().unwrap();
            common::rails(boot.path(),"test",&["db:schema:load"]);
            for command in ["grid","record"]{common::rails(boot.path(),"test",&["runner",script,command,root]);}
            root.to_string()}
    };
    let mut names=std::fs::read_dir(&root).unwrap().map(|e|e.unwrap()).filter(|e|e.file_type().unwrap().is_dir()).map(|e|e.file_name().into_string().unwrap()).collect::<Vec<_>>();names.sort();
    assert_eq!(names.len(),count);
    for name in selected {assert!(names.iter().any(|n|n==name),"missing regression {name}");}
    for name in names {
        if !selected.is_empty() && !selected.contains(&name.as_str()) {continue;}
        let source=Path::new(&root).join(&name);
        let scenario:Value=serde_json::from_slice(&std::fs::read(source.join("scenario.json")).unwrap()).unwrap();
        let rails:Value=serde_json::from_slice(&std::fs::read(source.join("rails.json")).unwrap()).unwrap();
        assert!(rails["network"].as_array().unwrap().is_empty());
        let expected=rails["responses"].as_array().unwrap().last().unwrap();
        let step=scenario["steps"].as_array().unwrap().last().unwrap();
        let dir=tempfile::tempdir().unwrap();
        for file in ["production.sqlite3","production_queue.sqlite3"]{std::fs::copy(source.join(file),dir.path().join(file)).unwrap();}
        let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
        c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE").unwrap();let before=combined(expected,"before");
        for(table,rows)in before.as_object().unwrap(){c.execute(&format!("DELETE FROM {table}"),[]).unwrap();
            for row in rows.as_array().unwrap(){let row=row.as_object().unwrap();let columns=row.keys().cloned().collect::<Vec<_>>().join(",");let slots=(1..=row.len()).map(|i|format!("?{i}")).collect::<Vec<_>>().join(",");
                c.execute(&format!("INSERT INTO {table}({columns}) VALUES({slots})"),rusqlite::params_from_iter(row.values().map(scalar))).unwrap();}}
        c.execute_batch("COMMIT; PRAGMA foreign_keys=ON").unwrap();
        let app=web::app(dir.path(),scenario["secret_key_base"].as_str().unwrap(),TestClock::at(scenario["at"].as_str().unwrap()));
        let (owner,hash):(i64,String)=c.query_row("SELECT id,encrypted_password FROM users ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let token=csrf::new_token();let data=session::SessionData{user:Some((owner,hash.chars().take(29).collect())),csrf:Some(token.clone()),..Default::default()};
        let mut browser=Browser{cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:None};
        let queue=Connection::open(dir.path().join("production_queue.sqlite3")).unwrap();let queue_before=snapshot(&queue);
        let headers=step["headers"].as_object().unwrap().iter().map(|(k,v)|(k.as_str(),v.as_str().unwrap())).collect::<Vec<_>>();
        let answer=browser.send(&app,"GET",step["path"].as_str().unwrap(),None,Csrf::None,&headers).await;
        for secret in ["synthetic-d5-market-token","synthetic-d5-paper-key","synthetic-d5-paper-secret"] {assert!(!answer.body.contains(secret),"{name}: secret in page");}
        same_rows(snapshot(&c),before,&name);assert_eq!(snapshot(&queue),queue_before,"{name}: queue changed");
        if matches!(mode(&name),Mode::Deferred) {
            assert_eq!(answer.status,501,"{name}: deferred scope must be refused");assert!(answer.body.contains("not-ported"));continue;
        }
        assert_eq!(json!(answer.status),expected["status"],"{name}");
        if expected.get("exception").is_some(){
            let body=match answer.status {404=>include_str!("../../../public/404.html"),500=>include_str!("../../../public/500.html"),status=>panic!("unexpected exception status {status}")};
            assert_eq!(answer.body,body,"{name}: public error body");continue;
        }
        let actual=masked(&answer.body,Some(&app),Some(&token));let mut oracle=masked(expected["body"].as_str().unwrap(),None,None);
        if matches!(mode(&name),Mode::FirstSync) && !c.query_row("SELECT hide_balances FROM users WHERE id=?1",[owner],|r|r.get::<_,bool>(0)).unwrap() {
            let zero=match name.as_str() {
                "first_currency_EUR"=>"      <small>€</small>0.00\n",
                "first_currency_GBP"=>"      <small>£</small>0.00\n",
                "first_currency_CHF"=>"      0.00 <small>Fr.</small>\n",
                "first_currency_PLN"=>"      0.00 <small>zł</small>\n",
                _=>"      <small>$</small>0.00\n",
            };
            assert_eq!(oracle.matches(zero).count(),1,"{name}: measured Rails zero cell");
            let locale=step["path"].as_str().unwrap().split('/').nth(1).unwrap();
            let locale=if deltabadger::web::locale::LOCALES.contains(&locale){locale}else{"en"};
            let reason=deltabadger::web::i18n::t(locale,"tracker.portfolio.never_synced",&[]);
            let cell=format!("      <span class=\"no-value\" title=\"{reason}\">—</span>\n");
            oracle=oracle.replacen(zero,&cell,1);
        }
        if matches!(mode(&name),Mode::FirstSync) {
            std::fs::write(source.join("rust.html"),&actual).unwrap();
            std::fs::write(source.join("rails-masked.html"),masked(expected["body"].as_str().unwrap(),None,None)).unwrap();
        }
        if actual!=oracle {std::fs::write(Path::new(&root).join("actual.html"),&actual).unwrap();std::fs::write(Path::new(&root).join("expected.html"),&oracle).unwrap();}
        assert!(actual==oracle,"{name}: byte equality; inspect actual.html and expected.html");
        for h in ["content-type","cache-control","x-frame-options","x-xss-protection","x-content-type-options","x-permitted-cross-domain-policies","referrer-policy"]{assert_eq!(answer.header(h),expected["headers"][h].as_str(),"{name}: {h}");}
    }
}

