//! D5a scope: supported endpoints work alone; deferred destinations visibly refuse.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session};
use rusqlite::{Connection,types::ValueRef};
use serde_json::{json,Map,Value};

#[tokio::test(flavor = "current_thread")]
async fn tracker_d5a_routes_and_modal_destinations_are_self_contained() {
    let (dir, opened, seeded) = common::install_alpaca();
    opened.primary.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    let hash: String = opened.primary.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-09-10T12:00:30.123456Z"));
    // The export modal's setup branch is deliberately outside D5. This route probe
    // exercises the configured page; the refusal is a separate contract.
    let connection = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    for (key,value) in [("market_data_provider","deltabadger"),("market_data_url","http://127.0.0.1:1"),("market_data_token","synthetic-d5-token")] {
        deltabadger::app_config::set(&connection,&app.cipher,key,value,app.now()).unwrap();
    }
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let mut browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())),
        page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) };
    let before = snapshot(&connection);
    let modal = browser.send(&app,"GET","/tracker/export_modal",None,Csrf::None,&[("turbo-frame","modal")]).await;
    assert_eq!(modal.status,200);
    assert!(modal.body.contains("<turbo-frame complete=\"\" id=\"modal\">"));
    for target in ["/tracker/export", "/tracker/tax_report", "/tracker/fund_classifications", "/tracker/save_export_settings"] {
        assert!(modal.body.contains(target),"missing modal destination {target}");
    }
    assert_eq!(snapshot(&connection),before,"modal must be a pure view");
    for prefix in ["", "/de"] {
        // S1 implements the credential form: a missing selection redirects to exchange picking.
        let mut credential_browser=Browser {cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:None};
        let path=format!("{prefix}/tracker/add_api_key/new");
        let answer=credential_browser.send(&app,"GET",&path,None,Csrf::None,&[("turbo-frame","modal")]).await;
        assert_eq!(answer.status,302,"{path}");
        assert_eq!(answer.header("location"),Some(format!("{prefix}/tracker/pick_exchange/new").as_str()));
        let picker=credential_browser.send(&app,"GET",&format!("{prefix}/tracker/pick_exchange/new"),None,Csrf::None,&[("turbo-frame","modal")]).await;
        assert_eq!(picker.status,200,"D5b: the redirect lands on the picker");assert!(picker.body.contains("id=\"exchange-picker\""));
        assert_eq!(snapshot(&connection),before,"the picker is a pure view");
        assert_eq!(snapshot(&connection),before,"credential form redirect must not mutate rows");
        let path=format!("{prefix}/tracker/add_api_key/new?exchange_id={}&key_type=read_only",seeded.exchange_id);
        let answer=credential_browser.send(&app,"GET",&path,None,Csrf::None,&[("turbo-frame","modal")]).await;
        assert_eq!(answer.status,200,"selected Alpaca credential form {path}");
        assert!(answer.body.contains("<form")&&answer.body.contains("api_key[key]"));
        assert_eq!(snapshot(&connection),before,"credential form must be a pure view");
        for (method,path) in [("GET","/tracker"),("GET","/tracker?all=1&show_cash=1"),("GET","/tracker/import/new"),("POST","/tracker/import"),("GET","/tracker/tax_report?country=US&year=2024"),("GET","/tracker/download_tax_report?country=US&year=2024"),("GET","/tracker/setup_coingecko"),("GET","/tracker/connect_market_data")] {
            let path=format!("{prefix}{path}");
            let answer=browser.send(&app,method,&path,Some(&[]),Csrf::Header,&[("origin","http://localhost:3000"),("accept","text/html"),("turbo-frame","modal")]).await;
            if path==format!("{prefix}/tracker") || path==format!("{prefix}/tracker?all=1&show_cash=1") {
                assert_eq!(answer.status,200,"{path}: populated first-sync page");
                assert!(answer.body.contains("tracker-record"));
                assert_eq!(snapshot(&connection),before,"first-sync GET must not mutate rows");
                continue;
            }
            if path==format!("{prefix}/tracker/import/new") || path==format!("{prefix}/tracker/import") {
                // I1: the dialog, and the answer to a submission with no file.
                let dialog=path.ends_with("/new");
                assert_eq!(answer.status,if dialog{200}else{422},"{path}");
                assert!(answer.body.contains("<turbo-frame complete=\"\" id=\"modal\">"),"{path}");
                assert!(if dialog{answer.body.contains(&format!("action=\"{prefix}/tracker/import\""))}else{answer.body.contains("<p class=\"tracker-empty\">")},"{path}");
                assert_eq!(snapshot(&connection),before,"import {path} without a file must not mutate rows");
                continue;
            }
            assert_eq!(answer.status,501,"{path}");
            assert!(answer.body.contains("Not available in the Rust build yet"),"{path}");
            assert!(answer.body.contains("<turbo-frame id=\"modal\">"),"{path}");
            assert_eq!(answer.header("location"),None);
            assert_eq!(snapshot(&connection),before,"deferred action {path} must not mutate rows");
        }
        let path=format!("{prefix}/tracker/tax_report?country=DE&year=2024&report_scope=broker");
        let answer=browser.send(&app,"GET",&path,None,Csrf::None,&[("accept","text/vnd.turbo-stream.html")]).await;
        assert_eq!(answer.status,501);
        assert_eq!(answer.header("content-type"),Some("text/vnd.turbo-stream.html; charset=utf-8"));
        assert!(answer.body.starts_with("<turbo-stream action=\"append\" target=\"flash\"><template>"));
        assert!(answer.body.contains("Not available in the Rust build yet"));
        assert_eq!(snapshot(&connection),before,"deferred report must not save pending_report");
    }
    for path in ["/tracker/save_export_settings","/tracker/fund_classifications"] {
        assert_eq!(browser.send(&app,"PATCH",path,Some(&[]),Csrf::Header,&[("origin","http://localhost:3000")]).await.status,200);
    }
    // Setup is a deferred page; opening the modal cannot start setup or persist proposals.
    deltabadger::app_config::set(&connection,&app.cipher,"market_data_provider","",app.now()).unwrap();
    let before=snapshot(&connection);
    let answer=browser.send(&app,"GET","/tracker/export_modal",None,Csrf::None,&[("turbo-frame","modal")]).await;
    assert_eq!(answer.status,501);
    assert!(answer.body.contains("Not available in the Rust build yet"));
    assert_eq!(snapshot(&connection),before);
}

#[tokio::test(flavor = "current_thread")]
async fn the_import_reads_a_multipart_body_under_its_bounds() {
    let (dir, opened, seeded) = common::install_alpaca();
    opened.primary.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    let hash: String = opened.primary.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-09-10T12:00:30.123456Z"));
    let connection = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let mut browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    let masked = csrf::masked(&token);
    let before = snapshot(&connection);
    let mut post = async |fields: &[(&str, &str)], files: &[(&str, &str, &[u8])]| {
        let (content_type, body) = web::multipart(fields, files);
        browser.send_bytes(&app, "POST", "/tracker/import", Some(body), Csrf::None, &[("content-type", content_type.as_str()), ("origin", "http://localhost:3000")]).await
    };
    // The token travels as a multipart text field (what the form's hidden input posts).
    for files in [&[][..], &[("file", "", &b""[..])][..], &[("file", "Binance (UTC+3).csv", &b" \r\n"[..])][..]] {
        let answer = post(&[("authenticity_token", &masked), ("format", "binance")], files).await;
        assert_eq!(answer.status, 422, "no file: {}", answer.body);
        assert!(answer.body.contains("<p class=\"tracker-empty\">"));
    }
    let answer = post(&[("format", "binance")], &[]).await;
    assert_ne!(answer.status, 422, "no token, no import");
    let answer = post(&[("authenticity_token", &masked)], &[("file", "Binance (UTC+3).csv", "\u{feff}Time\r\n".as_bytes())]).await;
    assert_eq!(answer.status, 501, "reading a file is not served yet");
    let answer = post(&[("authenticity_token", &masked), ("token", "0123456789abcdef")], &[]).await;
    assert_eq!(answer.status, 501, "neither is the parked-upload answer");
    let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[]);
    let truncated = browser.send_bytes(&app, "POST", "/tracker/import", Some(body[..body.len() - 8].to_vec()), Csrf::None, &[("content-type", content_type.as_str())]).await;
    assert_eq!(truncated.status, 400);
    let mut huge = vec![b'a'; deltabadger::web::IMPORT_LIMIT];
    huge.extend_from_slice(b"overflow");
    let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[("file", "big.csv", &huge)]);
    let oversize = browser.send_bytes(&app, "POST", "/tracker/import", Some(body), Csrf::None, &[("content-type", content_type.as_str())]).await;
    assert_eq!(oversize.status, 413);
    assert_eq!(snapshot(&connection), before);
}

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
                    ValueRef::Null => Value::Null, ValueRef::Integer(n) => json!(n), ValueRef::Real(n) => json!(n),
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
