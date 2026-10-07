//! Exact body bytes and every primary row for the implemented write slice.
//! The complete tracker page grid remains a separate acceptance gate.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session};
use rusqlite::{Connection, types::{Value as Sql, ValueRef}};
use serde_json::{json, Map, Value};
use std::path::Path;

fn snapshot(c: &Connection) -> Value {
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
fn flat(prefix: &str, value: &Value, out: &mut Vec<(String,String)>) {
    match value {
        Value::Object(rows) => for (name,value) in rows { flat(&if prefix.is_empty() { name.clone() } else {format!("{prefix}[{name}]")},value,out); },
        Value::Array(rows) => for value in rows { flat(&format!("{prefix}[]"),value,out); },
        Value::Null => {}, Value::String(s) => out.push((prefix.into(),s.clone())), _ => panic!("unsupported fixture scalar"),
    }
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

#[tokio::test(flavor="current_thread")]
async fn current_fx_matches_rails_rows_and_response_bytes() {
    compare(&["fx_EUR","fx_GBP","fx_CHF","fx_PLN"]).await;
}

fn mask_csrf(body: &str) -> String {
    let prefix = "name=\"authenticity_token\" value=\"";
    let mut rest=body;
    let mut result=String::new();
    while let Some((before,tail))=rest.split_once(prefix) {
        result.push_str(before); result.push_str(prefix);
        let (token,after)=tail.split_once('"').expect("closed token attribute");
        assert!(!token.is_empty());
        result.push_str("[csrf]\""); rest=after;
    }
    result.push_str(rest);
    for path in common::html::masked_values(&result).assets {
        if let Some((base,tail))=path.rsplit_once('-') {
            if let Some((digest,extension))=tail.split_once('.') {
                if (8..=64).contains(&digest.len()) && digest.bytes().all(|b|b.is_ascii_hexdigit()) {
                    result=result.replace(&path,&format!("{base}-[digest].{extension}"));
                }
            }
        }
    }
    result
}

async fn compare(cases: &[&str]) {
    let generated;
    let root = match std::env::var("D5_FX_GRID") {
        Ok(root) => root,
        Err(_) => {
            let boot = tempfile::tempdir().unwrap();
            generated = tempfile::tempdir().unwrap();
            let root = generated.path().to_str().unwrap();
            common::rails(boot.path(),"test",&["db:schema:load"]);
            common::rails(boot.path(),"test",&["runner","script/rust/pages_tracker_fx.rb","grid",root]);
            common::rails(boot.path(),"test",&["runner","script/rust/pages_tracker_fx.rb","record",root]);
            root.to_string()
        }
    };
    let mut names=cases.iter().map(|s|s.to_string()).collect::<Vec<_>>();
    for entry in std::fs::read_dir(&root).unwrap() {
        let name=entry.unwrap().file_name().to_str().unwrap().to_string();
        if name.starts_with("d5_fx_") && !names.iter().any(|n|format!("d5_{n}")==name) {names.push(name.trim_start_matches("d5_").to_string());}
    }
    assert_eq!(names.len(),33,"FX locales, fragment actions and round-1/round-2 regressions required");
    for required in ["fx_review_clear","fx_review_precision","fx_review_reverse","fx_review_zero","fx_review_negative"] {assert!(names.iter().any(|name|name==required));}
    for name in &names {
        let directory = if name.starts_with("sync_") { std::env::var("D5_SYNC_GRID").unwrap_or_else(|_|root.clone()) } else { root.clone() };
        let source = Path::new(&directory).join(format!("d5_{name}"));
        let scenario: Value = serde_json::from_slice(&std::fs::read(source.join("scenario.json")).unwrap()).unwrap();
        let rails: Value = serde_json::from_slice(&std::fs::read(source.join("rails.json")).unwrap()).unwrap();
        assert!(rails["network"].as_array().unwrap().is_empty());
        let expected = rails["responses"].as_array().unwrap().last().unwrap();
        let step = scenario["steps"].as_array().unwrap().last().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for name in ["production.sqlite3","production_queue.sqlite3"] { std::fs::copy(source.join(name),dir.path().join(name)).unwrap(); }
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        // The record directory contains the post-request database. Rebuild the pre-request
        // row state from the recorder before opening App, retaining the real Rails schema.
        c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE").unwrap();
        let before = combined(expected,"before");
        for (table,rows) in before.as_object().unwrap() {
            c.execute(&format!("DELETE FROM {table}"),[]).unwrap();
            for row in rows.as_array().unwrap() {
                let row = row.as_object().unwrap();
                let columns = row.keys().cloned().collect::<Vec<_>>().join(",");
                let placeholders = (1..=row.len()).map(|i|format!("?{i}")).collect::<Vec<_>>().join(",");
                c.execute(&format!("INSERT INTO {table}({columns}) VALUES({placeholders})"),rusqlite::params_from_iter(row.values().map(scalar))).unwrap();
            }
        }
        // Explicit IDs are part of equality. DELETE does not rewind SQLite's sequence.
        c.execute("DELETE FROM sqlite_sequence WHERE name='fund_classifications'",[]).unwrap();
        c.execute_batch("COMMIT; PRAGMA foreign_keys=ON").unwrap();
        let secret = scenario["secret_key_base"].as_str().unwrap();
        let rates=match name.as_str() {
            "fx_string"=>json!({"usd":{"value":"100.0"},"eur":{"value":"80.0"}}),
            "fx_string_garbage"=>json!({"usd":{"value":"100.0"},"eur":{"value":"12abc"}}),
            "fx_string_empty"=>json!({"usd":{"value":"100.0"},"eur":{"value":""}}),
            "fx_integer"=>json!({"usd":{"value":100},"eur":{"value":80}}),
            "fx_review_clear"=>json!({"usd":{"value":64123.456},"eur":{"value":55210.987}}),
            "fx_review_precision"=>json!({"usd":{"value":1.0},"eur":{"value":1.0000000000000002}}),
            "fx_review_reverse"=>json!({"usd":{"value":1.0000000000000002},"eur":{"value":1.0}}),
            "fx_review_zero"=>json!({"usd":{"value":100.0},"eur":{"value":0.0}}),
            "fx_review_negative"=>json!({"usd":{"value":-100.0},"eur":{"value":-80.0}}),
            _=>json!({"usd":{"value":100.0},"eur":{"value":80.0},"gbp":{"value":50.0},"chf":{"value":90.0},"pln":{"value":400.0}}),
        };
        let app = web::app(dir.path(),secret,TestClock::at(scenario["at"].as_str().unwrap())).with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({
            "GET 127.0.0.1:1/api/v1/exchange_rates":{"body":{"data":rates}}
        }))).unwrap();
        same_rows(snapshot(&c),before,&format!("{name} before"));
        let (owner,hash): (i64,String) = c.query_row("SELECT id,encrypted_password FROM users ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let token = csrf::new_token();
        let data = session::SessionData { user: Some((owner,hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
        let mut browser = Browser { cookie: Some(session::seal(&app.keys.session,&data,app.now())),page:Some(format!("<meta name=\"csrf-token\" content=\"{}\">",csrf::masked(&token))) };
        let mut fields = vec![];
        flat("",&step["form"],&mut fields);
        let form: Vec<_> = fields.iter().map(|(k,v)|(k.as_str(),v.as_str())).collect();
        let headers: Vec<_> = step["headers"].as_object().unwrap().iter().map(|(k,v)|(k.as_str(),v.as_str().unwrap())).collect();
        let answer = if let Some(body) = step["json"].as_str() {
            browser.send_body(&app,step["method"].as_str().unwrap(),step["path"].as_str().unwrap(),Some(body.into()),Csrf::Header,&headers).await
        } else { browser.send(&app,step["method"].as_str().unwrap(),step["path"].as_str().unwrap(),Some(&form),Csrf::Header,&headers).await };
        if ["fx_review_zero","fx_review_negative","fx_string_garbage","fx_string_empty"].contains(&name.as_str()) {
            // Rulings: invalid or prefix-garbage FX is refused with no effects; Rails falls back or parses a prefix.
            assert_eq!(expected["status"],200);
            assert_eq!(answer.status,422);assert!(answer.body.contains("Currency conversion unavailable"));
            same_rows(snapshot(&c),combined(expected,"before"),&format!("{name}: refused unchanged"));
            continue;
        }
        if name=="fx_review_clear" {assert!(answer.body.contains("placeholder=\"0.8610107820763747\""));}
        if name=="fx_review_precision" {
            let stored:String=c.query_row("SELECT manual_values FROM account_transactions WHERE id=4",[],|r|r.get(0)).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(),json!({"price":"100000000000000.02"}));
        }
        assert_eq!(u64::from(answer.status),expected["status"].as_u64().unwrap(),"{name}");
        if expected.get("exception").is_none() { {
            for path in common::html::masked_values(&answer.body).assets {
                assert!(deltabadger::web::assets::find(&path).is_some(),"missing embedded asset {path}");
            }
            for rendered in common::html::masked_values(&answer.body).tokens {
                assert!(csrf::valid(&token,&rendered),"invalid rendered CSRF");
            }
            assert_eq!(mask_csrf(&answer.body),mask_csrf(expected["body"].as_str().unwrap()),"{name}: body bytes");
        } }
        else {
            // Existing web-foundation exception contract: public error file and below-controller
            // headers. Rails recordings omit their debugging page, which is not production HTML.
            let page = if answer.status == 404 { include_str!("../../public/404.html") } else if answer.status == 422 { include_str!("../../public/422.html") } else { include_str!("../../public/500.html") };
            assert_eq!(answer.body,page,"{name}: public error page");
            assert_eq!(answer.header("content-type"),Some("text/html; charset=utf-8"));
            assert_eq!(answer.header("cache-control"),Some("no-cache"));
            for header in ["x-frame-options","x-xss-protection","x-content-type-options","x-permitted-cross-domain-policies","referrer-policy"] { assert_eq!(answer.header(header),None); }
        }
        for header in ["content-type","cache-control","x-frame-options","x-xss-protection","x-content-type-options","x-permitted-cross-domain-policies","referrer-policy"] {
            if expected.get("exception").is_none() { assert_eq!(answer.header(header),expected["headers"][header].as_str(),"{name}: {header}"); }
        }
        if name.starts_with("export_") { assert_eq!(answer.header("content-disposition"),expected["headers"]["content-disposition"].as_str(),"{name}: disposition"); }
        same_rows(snapshot(&c),combined(expected,"after"),&format!("{name} after"));
    }
}
