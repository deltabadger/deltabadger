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
        for (method,path) in [("GET","/tracker"),("GET","/tracker?all=1&show_cash=1"),("GET","/tracker/import/new"),("POST","/tracker/import"),("GET","/tracker/export"),("GET","/tracker/tax_report?country=US&year=2024"),("GET","/tracker/download_tax_report?country=US&year=2024"),("GET","/tracker/add_api_key/new"),("GET","/tracker/pick_exchange/new"),("GET","/tracker/setup_coingecko"),("GET","/tracker/connect_market_data")] {
            let path=format!("{prefix}{path}");
            let answer=browser.send(&app,method,&path,Some(&[]),Csrf::Header,&[("origin","http://localhost:3000"),("accept","text/html"),("turbo-frame","modal")]).await;
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
