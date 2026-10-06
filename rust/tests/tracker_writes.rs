//! Real HTTP writes, including rollback, ownership and cancellation-safe delivery.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session, App};
use rusqlite::Connection;
use serde_json::{json, Value};

fn install() -> (tempfile::TempDir, Connection, i64, App, Browser) {
    let (dir, opened, seeded) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-09-10T12:00:30.123456Z"));
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) };
    (dir, c, seeded.user_id, app, browser)
}
const HEADERS: &[(&str, &str)] = &[("origin", "http://localhost:3000"), ("accept", "text/vnd.turbo-stream.html, text/html")];
const SETTINGS: &str = "/tracker/save_export_settings";
const FUNDS: &str = "/tracker/fund_classifications";
fn settings(c: &Connection, owner: i64) -> Value {
    let text: String = c.query_row("SELECT coalesce(tracker_settings,'{}') FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
    serde_json::from_str(&text).unwrap()
}
fn count(c: &Connection) -> i64 { c.query_row("SELECT count(*) FROM fund_classifications", [], |r| r.get(0)).unwrap() }

#[tokio::test(flavor="current_thread")]
async fn settings_model_validation_keeps_invalid_users_unchanged_with_rails_success() {
    let (_dir,c,owner,app,mut b) = install();
    // Corrupt stored values after authentication, as legacy/imported rows can be invalid.
    for (column, value) in [("time_zone", "Invalid/Zone"), ("time_zone", ""),
        ("locale", "invalid"), ("locale", ""), ("display_currency", "BTC"),
        ("display_currency", "usd"), ("wash_sale_jurisdiction", "DE"), ("email", "\u{3000}")] {
        let original: rusqlite::types::Value = c.query_row(&format!("SELECT {column} FROM users WHERE id=?1"), [owner], |r| r.get(0)).unwrap();
        c.execute(&format!("UPDATE users SET {column}=?1 WHERE id=?2"), (value,owner)).unwrap();
        let snapshot = || c.query_row("SELECT * FROM users WHERE id=?1", [owner], |r| {
            (0..r.as_ref().column_count()).map(|i| r.get::<_,rusqlite::types::Value>(i)).collect::<Result<Vec<_>,_>>()
        }).unwrap();
        let before = snapshot();
        let response = b.send(&app,"PATCH",SETTINGS,Some(&[("country","US")]),Csrf::Header,HEADERS).await;
        assert_eq!(response.status,200,"{column}: Rails ignores update's false return");
        assert_eq!(response.body,"");
        assert_eq!(response.header("content-type"),Some("text/vnd.turbo-stream.html"));
        assert_eq!(snapshot(),before,"{column}: validation must preserve the entire user row");
        c.execute(&format!("UPDATE users SET {column}=?1 WHERE id=?2"), (original,owner)).unwrap();
    }
}

#[tokio::test(flavor="current_thread")]
async fn settings_model_validation_obeys_update_context_and_optional_preferences() {
    let (_dir,c,owner,app,mut b) = install();
    // Unchanged email format/name and absent password are not validated on this save.
    c.execute("UPDATE users SET name='',email='legacy-format',locale=NULL,wash_sale_jurisdiction=' ' WHERE id=?1",[owner]).unwrap();
    let response = b.send(&app,"PATCH",SETTINGS,Some(&[("country","US")]),Csrf::Header,HEADERS).await;
    assert_eq!(response.status,200);
    assert_eq!(settings(&c,owner)["country"],"US");
}

#[tokio::test(flavor="current_thread")]
async fn settings_allowlist_blank_preservation_and_owned_update() {
    let (_dir,c,owner,app,mut b) = install();
    c.execute("INSERT INTO users(id,email,encrypted_password,tracker_settings,created_at,updated_at) VALUES(999,'foreign@example.test','x','{\"country\":\"PL\"}','2026-01-01','2026-01-01')",[]).unwrap();
    c.execute("UPDATE users SET tracker_settings=?1 WHERE id=?2", (json!({"show_cash":true,"pending_report":{"country":"DE"}}).to_string(), owner)).unwrap();
    let a = b.send(&app,"PATCH",SETTINGS,Some(&[("country","US"),("year","2024"),("export_type","tax"),("report_scope","crypto"),("show_cash","false"),("user_id","999")]),Csrf::Header,HEADERS).await;
    assert_eq!(a.status,200);
    assert_eq!(a.body,"");
    assert_eq!(settings(&c,owner),json!({"show_cash":true,"pending_report":{"country":"DE"},"country":"US","year":"2024","export_type":"tax","report_scope":"crypto"}));
    let a = b.send(&app,"PATCH",SETTINGS,Some(&[("country"," "),("year","")]),Csrf::Header,HEADERS).await;
    assert_eq!(a.status,200);
    assert_eq!(settings(&c,owner)["country"],"US");
    assert_eq!(settings(&c,owner)["year"],"2024");
    assert_eq!(settings(&c,999),json!({"country":"PL"}));
}

#[tokio::test(flavor="current_thread")]
async fn classifications_keep_valid_rows_after_an_invalid_row_and_preserve_spelling() {
    let (_dir,c,owner,app,mut b) = install();
    let fields = [("classifications[][symbol]","ETF"),("classifications[][kind]","fund"),
        ("classifications[][symbol]"," AaPl "),("classifications[][kind]","share"),("classifications[][fund_category]","equity_fund")];
    let a = b.send(&app,"PATCH",FUNDS,Some(&fields),Csrf::Header,HEADERS).await;
    assert_eq!(a.status,422);
    assert_eq!(a.body,"");
    let row: (i64,String,i64,Option<i64>) = c.query_row("SELECT user_id,symbol,kind,fund_category FROM fund_classifications",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(row,(owner," AaPl ".into(),0,None));
    let valid = [("classifications[][symbol]","ETF"),("classifications[][kind]","fund"),("classifications[][fund_category]","equity_fund")];
    assert_eq!(b.send(&app,"PATCH",FUNDS,Some(&valid),Csrf::Header,HEADERS).await.status,200);
    let before: String = c.query_row("SELECT updated_at FROM fund_classifications WHERE symbol='ETF'",[],|r|r.get(0)).unwrap();
    assert_eq!(b.send(&app,"PATCH",FUNDS,Some(&valid),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(count(&c),2);
    assert_eq!(c.query_row("SELECT updated_at FROM fund_classifications WHERE symbol='ETF'",[],|r|r.get::<_,String>(0)).unwrap(),before);
}

#[tokio::test(flavor="current_thread")]
async fn classification_shape_and_kind_validation_do_not_write() {
    let (_dir,c,_owner,app,mut b) = install();
    for fields in [vec![("classifications[0][symbol]","AAPL"),("classifications[0][kind]","share")],
        vec![("classifications[][symbol]","AAPL"),("classifications[][kind]","unknown")],
        vec![("classifications[][symbol]"," "),("classifications[][kind]","share")]] {
        assert_eq!(b.send(&app,"PATCH",FUNDS,Some(&fields),Csrf::Header,HEADERS).await.status,200);
        assert_eq!(count(&c),0);
    }
}

#[tokio::test(flavor="current_thread")]
async fn missing_csrf_and_foreign_origin_leave_both_writes_untouched() {
    let (_dir,c,owner,app,mut b) = install();
    let before = settings(&c,owner);
    for path in [SETTINGS,FUNDS] {
        let fields = [("country","DE"),("classifications[][symbol]","AAPL"),("classifications[][kind]","share")];
        assert_eq!(b.send(&app,"PATCH",path,Some(&fields),Csrf::None,HEADERS).await.status,302);
        assert_eq!(b.send(&app,"PATCH",path,Some(&fields),Csrf::Header,&[("origin","https://foreign.invalid")]).await.status,302);
        assert_eq!(settings(&c,owner),before);
        assert_eq!(count(&c),0);
    }
}

#[tokio::test(flavor="current_thread")]
async fn sql_failure_rolls_back_the_entire_classification_batch() {
    let (_dir,c,_owner,app,mut b) = install();
    c.execute_batch("CREATE TRIGGER refuse_second BEFORE INSERT ON fund_classifications WHEN NEW.symbol='FAIL' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    let fields = [("classifications[][symbol]","AAPL"),("classifications[][kind]","share"),("classifications[][symbol]","FAIL"),("classifications[][kind]","share")];
    assert_eq!(b.send(&app,"PATCH",FUNDS,Some(&fields),Csrf::Header,HEADERS).await.status,500);
    assert_eq!(count(&c),0,"a database failure must not leave a partial batch");
}

#[tokio::test(flavor="current_thread")]
async fn settings_database_failure_is_not_success_and_does_not_wake() {
    let (_dir,c,owner,app,mut b) = install();
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    c.execute_batch("CREATE TRIGGER refuse_settings BEFORE UPDATE OF tracker_settings ON users BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    let before = settings(&c,owner);
    assert_eq!(b.send(&app,"PATCH",SETTINGS,Some(&[("country","US")]),Csrf::Header,HEADERS).await.status,500);
    assert_eq!(settings(&c,owner),before);
    assert!(tokio::time::timeout(std::time::Duration::from_millis(10),wake.notified()).await.is_err());
}

#[tokio::test(flavor="current_thread")]
async fn successful_writes_wake_after_commit() {
    let (_dir,c,owner,app,mut b) = install();
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    for path in [SETTINGS,FUNDS] {
        let fields = [("country","DE"),("classifications[][symbol]","AAPL"),("classifications[][kind]","share")];
        assert_eq!(b.send(&app,"PATCH",path,Some(&fields),Csrf::Header,HEADERS).await.status,200);
        tokio::time::timeout(std::time::Duration::from_millis(100),wake.notified()).await.expect("committed write wakes engine");
    }
    assert_eq!(settings(&c,owner)["country"],"DE");
    assert_eq!(count(&c),1);
}

#[tokio::test(flavor="current_thread")]
async fn engine_guard_refuses_and_rolls_back_each_write() {
    let (_dir,c,owner,app,mut b) = install();
    c.execute("INSERT INTO bots(user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES(?1,1,'Bots::DcaSingleAsset',1,'{}','{}','2026-01-01','2026-01-01')",[owner]).unwrap();
    let before = settings(&c,owner);
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    for path in [SETTINGS,FUNDS] {
        let fields = [("country","DE"),("classifications[][symbol]","AAPL"),("classifications[][kind]","share")];
        let a = b.send(&app,"PATCH",path,Some(&fields),Csrf::Header,HEADERS).await;
        assert_eq!(a.status,422);
        assert!(!a.body.is_empty(),"guard refusal states its reason");
        assert_eq!(settings(&c,owner),before);
        assert_eq!(count(&c),0);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10),wake.notified()).await.is_err());
    }
}

#[tokio::test(flavor="current_thread")]
async fn unauthenticated_writes_and_foreign_classification_ids_change_nothing_foreign() {
    let (_dir,c,owner,app,mut b) = install();
    let fields = [("country","DE"),("user_id","999"),("classifications[][user_id]","999"),("classifications[][symbol]","AAPL"),("classifications[][kind]","share")];
    let mut anonymous = Browser::default();
    for path in [SETTINGS,FUNDS] {
        let a = anonymous.send(&app,"PATCH",path,Some(&fields),Csrf::None,HEADERS).await;
        assert_eq!(a.status,302);
    }
    c.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES(999,'foreign@example.test','x','2026-01-01','2026-01-01')",[]).unwrap();
    c.execute("INSERT INTO fund_classifications(user_id,symbol,kind,created_at,updated_at) VALUES(999,'AAPL',2,'2026-01-01','2026-01-01')",[]).unwrap();
    assert_eq!(b.send(&app,"PATCH",FUNDS,Some(&fields),Csrf::Header,HEADERS).await.status,200);
    let rows: Vec<(i64,i64)> = c.prepare("SELECT user_id,kind FROM fund_classifications ORDER BY user_id").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(rows,vec![(owner,0),(999,2)]);
}

#[tokio::test(flavor="current_thread")]
async fn tracker_json_uses_rails_query_precedence_and_scalar_casting() {
    let (_dir,c,owner,app,mut b) = install();
    let headers = [("origin","http://localhost:3000"),("accept","text/vnd.turbo-stream.html"),("content-type","application/json")];
    let a = b.send_body(&app,"PATCH","/tracker/save_export_settings?country=DE",Some(json!({"country":"US","year":2024}).to_string()),Csrf::Header,&headers).await;
    assert_eq!(a.status,200);
    assert_eq!(settings(&c,owner)["country"],"DE");
    assert_eq!(settings(&c,owner)["year"],2024);
    let a = b.send_body(&app,"PATCH",FUNDS,Some(json!({"classifications":[{"symbol":123,"kind":"share"}]}).to_string()),Csrf::Header,&headers).await;
    assert_eq!(a.status,200);
    assert_eq!(c.query_row("SELECT symbol FROM fund_classifications",[],|r|r.get::<_,String>(0)).unwrap(),"123");
}

#[tokio::test(flavor="current_thread")]
async fn round1_string_columns_use_active_model_casts() {
    let (_dir,c,owner,app,mut b)=install();
    let headers=[("origin","http://localhost:3000"),("accept","text/vnd.turbo-stream.html"),("content-type","application/json")];
    for symbol in [json!(true),json!(false),json!(123),json!(1.25),Value::Null] {
        let r=b.send_body(&app,"PATCH",FUNDS,Some(json!({"classifications":[{"symbol":symbol,"kind":"share"}]}).to_string()),Csrf::Header,&headers).await;
        assert_eq!(r.status,200);
    }
    let symbols:Vec<String>=c.prepare("SELECT symbol FROM fund_classifications WHERE user_id=?1 ORDER BY symbol").unwrap().query_map([owner],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(symbols,vec!["1.25","123","t"]);
}
