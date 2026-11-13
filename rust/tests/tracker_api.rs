//! PLAN-A11, Rust-only: the tracker through REST and MCP against the web form for the same input, its lock, its guard, the
//! wakes after a commit, and the bounds Rails has none of (D-EXPORT-SIZE, D-MCP-LIST-TEXT). The Rails parity is the page
//! grid's `api_tracker_*` and the control grid's `tracker_*` scenarios.
mod common;
use axum::{body::{to_bytes, Body}, http::Request};
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session, tracker::api, App};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const HEADERS: &[(&str, &str)] = &[("origin", "http://localhost:3000"), ("accept", "text/vnd.turbo-stream.html, text/html")];
const TOOLS: [&str; 6] = ["list_account_transactions", "export_transactions_csv", "set_transfer_link", "set_transaction_price", "sync_tracker", "list_indices"];

fn install() -> (tempfile::TempDir, Connection, i64, App, Browser) { let (dir, c, owner, app, b, _) = install_clocked(); (dir, c, owner, app, b) }
fn install_clocked() -> (tempfile::TempDir, Connection, i64, App, Browser, Arc<TestClock>) {
    let (dir, opened, seeded) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1, time_zone='UTC'", []).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let clock = TestClock::at("2026-09-10T12:00:30.123456Z");
    let app = web::app(dir.path(), web::SECRET, clock.clone());
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) };
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('a11','a11','http://localhost/cb',0,'mcp api','2026-09-01 12:00:00','2026-09-01 12:00:00')", []).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("a11-mcp", "mcp"), ("a11-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-10 11:00:00',86400)", (application, seeded.user_id, token, scope)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-01 12:00:00','2026-09-01 12:00:00')", (seeded.user_id, application)).unwrap();
    let on: serde_json::Map<String, Value> = TOOLS.iter().map(|t| (t.to_string(), json!(true))).collect();
    c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1", [json!({"tool_permissions": on}).to_string()]).unwrap();
    (dir, c, seeded.user_id, app, browser, clock)
}

fn exchange(c: &Connection) -> i64 { c.query_row("SELECT id FROM exchanges ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap() }

fn row(c: &Connection, user: i64, kind: i64, base: &str, amount: &str, at: &str) -> i64 {
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'{}','{}','2026-01-01 00:00:00','2026-01-01 00:00:00')",
              (user, exchange(c), kind, base, amount, at)).unwrap();
    c.last_insert_rowid()
}

/// One account_transactions row, every column.
fn stored(c: &Connection, id: i64) -> Value {
    let mut q = c.prepare("SELECT * FROM account_transactions WHERE id=?1").unwrap();
    let names: Vec<String> = q.column_names().iter().map(|s| s.to_string()).collect();
    q.query_row([id], |r| {
        Ok(Value::Object(names.iter().enumerate().map(|(i, n)| (n.clone(), match r.get_ref(i).unwrap() {
            rusqlite::types::ValueRef::Null => Value::Null, rusqlite::types::ValueRef::Integer(v) => json!(v), rusqlite::types::ValueRef::Real(v) => json!(v),
            rusqlite::types::ValueRef::Text(v) => json!(std::str::from_utf8(v).unwrap()), rusqlite::types::ValueRef::Blob(_) => panic!("blob"),
        })).collect()))
    }).unwrap()
}

async fn send(app: &App, method: &str, path: &str, token: &str, body: Option<Value>, session: Option<&str>) -> (u16, Option<String>, String) {
    let mut request = Request::builder().method(method).uri(path).header("host", "localhost:3000").header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json, text/event-stream");
    if body.is_some() { request = request.header("content-type", "application/json"); }
    if let Some(session) = session { request = request.header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25"); }
    let response = deltabadger::web::router(app.clone()).oneshot(request.body(Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let session = response.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_string());
    (status, session, String::from_utf8(to_bytes(response.into_body(), 64_000_000).await.unwrap().to_vec()).unwrap())
}

async fn rest(app: &App, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let (status, _, body) = send(app, method, path, "a11-api", body, None).await;
    (status, serde_json::from_str(&body).unwrap_or(json!(body)))
}

/// A tool's result, from a fresh session.
async fn mcp(app: &App, name: &str, args: Value) -> Value {
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"a11","version":"1"}}});
    let (_, session, _) = send(app, "POST", "/mcp", "a11-mcp", Some(init), None).await;
    let session = session.unwrap();
    assert_eq!(send(app, "POST", "/mcp", "a11-mcp", Some(json!({"jsonrpc":"2.0","method":"notifications/initialized"})), Some(&session)).await.0, 202);
    let (_, _, body) = send(app, "POST", "/mcp", "a11-mcp", Some(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}})), Some(&session)).await;
    serde_json::from_str::<Value>(&body).unwrap()["result"].clone()
}

fn text(result: &Value) -> (&str, bool) { (result["content"][0]["text"].as_str().unwrap(), result["isError"] == true) }

fn wakes(app: &App) -> Vec<(String, Option<String>)> { app.job_wakers().unwrap().pending() }

/// The same stated price through the web's form (a USD account: no conversion), REST and MCP writes the same row.
#[tokio::test(flavor = "current_thread")]
async fn a_price_is_the_same_row_from_the_web_rest_and_mcp() {
    let mut rows = vec![];
    for surface in ["web", "rest", "mcp"] {
        let (_dir, c, owner, app, mut b) = install();
        c.execute("UPDATE users SET display_currency='USD'", []).unwrap();
        let id = row(&c, owner, 8, "ETH", "2", "2026-09-02 10:00:00");
        match surface {
            "web" => assert_eq!(b.send(&app, "PATCH", &format!("/tracker/transactions/{id}/price"), Some(&[("price", "12.5")]), Csrf::Header, HEADERS).await.status, 200),
            "rest" => assert_eq!(rest(&app, "PATCH", &format!("/api/v1/transactions/account/{id}/price"), Some(json!({"price_usd":"12.5"}))).await,
                                 (200, json!({"data":{"id":id,"price_usd":"12.5"},"error":null}))),
            _ => assert_eq!(text(&mcp(&app, "set_transaction_price", json!({"transaction_id":id,"price_usd":"12.5"})).await), (format!("Transaction #{id} priced at 12.5 USD.").as_str(), false)),
        }
        rows.push(stored(&c, id));
    }
    assert_eq!(rows[0]["manual_values"], json!("{\"price\":\"12.5\"}"));
    assert_eq!(rows[0], rows[1]);
    assert_eq!(rows[0], rows[2]);
}

/// The web's toggle, REST's `linked: true` and MCP link (then unlink) the same pair the same way.
#[tokio::test(flavor = "current_thread")]
async fn a_link_and_an_unlink_are_the_same_rows_from_the_web_rest_and_mcp() {
    let mut linked = vec![];
    let mut unlinked = vec![];
    for surface in ["web", "rest", "mcp"] {
        let (_dir, c, owner, app, mut b) = install();
        let w = row(&c, owner, 5, "BTC", "0.3", "2026-09-03 12:00:00");
        let d = row(&c, owner, 4, "BTC", "0.2999", "2026-09-04 12:00:00");
        for link in [true, false] {
            match surface {
                "web" => assert_eq!(b.send(&app, "PATCH", &format!("/tracker/transactions/{w}/toggle_transfer"), Some(&[]), Csrf::Header, HEADERS).await.status, 200),
                "rest" => assert_eq!(rest(&app, "POST", &format!("/api/v1/transactions/account/{w}/transfer_link"), Some(json!({"linked":link}))).await,
                                     (200, json!({"data":{"withdrawal_id":w,"deposit_id":d,"linked":link},"error":null}))),
                _ => assert!(!text(&mcp(&app, "set_transfer_link", json!({"transaction_id":w,"linked":link})).await).1),
            }
            (if link { &mut linked } else { &mut unlinked }).push((stored(&c, w), stored(&c, d)));
        }
    }
    assert_eq!(linked[0].0["linked_transaction_id"], json!(linked[0].1["id"]));
    assert_eq!(unlinked[0].0["transfer_link_rejected"], json!(1));
    for rows in [&linked, &unlinked] { assert_eq!(rows[0], rows[1]); assert_eq!(rows[0], rows[2]); }
}

/// A write while another connection holds a write transaction waits for it (the IMMEDIATE transaction), then lands: both
/// writes are there.
#[tokio::test(flavor = "current_thread")]
async fn a_write_waits_for_a_held_write_and_then_lands() {
    let (dir, c, owner, app, _b) = install();
    let id = row(&c, owner, 8, "ETH", "2", "2026-09-02 10:00:00");
    let _ = &dir;
    let other = Connection::open(c.path().unwrap()).unwrap();
    other.execute_batch("BEGIN IMMEDIATE; UPDATE account_transactions SET description='engine' WHERE id=1;").unwrap();
    let request = tokio::spawn({ let app = app.clone(); async move { rest(&app, "PATCH", &format!("/api/v1/transactions/account/{id}/price"), Some(json!({"price_usd":"3"}))).await } });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!request.is_finished(), "the API write must wait for the held transaction");
    assert_eq!(stored(&c, id)["manual_values"], json!("{}"));
    other.execute_batch("COMMIT").unwrap();
    assert_eq!(request.await.unwrap().0, 200);
    let row = stored(&c, id);
    assert_eq!((row["description"].clone(), row["manual_values"].clone()), (json!("engine"), json!("{\"price\":\"3.0\"}")));
}

/// D-API-GUARD and r3 item 2: a write the engine's guard refuses is rolled back and wakes nothing; a refusal found inside
/// the transaction stays that refusal (the guard never runs) and wakes nothing either.
#[tokio::test(flavor = "current_thread")]
async fn a_guard_refusal_and_an_inner_refusal_write_and_wake_nothing() {
    let (_dir, c, owner, app, _b) = install();
    let wake = Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    let id = row(&c, owner, 8, "ETH", "2", "2026-09-02 10:00:00");
    // A working bot the guard cannot read (as tests/tracker_writes.rs engine_guard_refuses_and_rolls_back_each_write).
    c.execute("INSERT INTO bots(user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES(?1,1,'Bots::DcaSingleAsset',1,'{}','{}','2026-01-01','2026-01-01')", [owner]).unwrap();
    let before = stored(&c, id);
    let path = format!("/api/v1/transactions/account/{id}/price");
    let (status, body) = rest(&app, "PATCH", &path, Some(json!({"price_usd":"3"}))).await;
    assert_eq!((status, body["error"]["code"].clone()), (409, json!("write_refused")), "{body}");
    assert_eq!(rest(&app, "PATCH", &path, Some(json!({"price_usd":"abc"}))).await.1["error"]["code"], "invalid_price");
    assert_eq!(rest(&app, "POST", &format!("/api/v1/transactions/account/{id}/transfer_link"), Some(json!({"linked":false}))).await.1["error"]["code"], "not_linked");
    let result = mcp(&app, "set_transaction_price", json!({"transaction_id":id,"price_usd":"3"})).await;
    assert_eq!(text(&result), (body["error"]["message"].as_str().unwrap(), true), "MCP says the same refusal");
    assert_eq!(stored(&c, id), before);
    assert!(wakes(&app).is_empty(), "{:?}", wakes(&app));
    assert!(tokio::time::timeout(std::time::Duration::from_millis(10), wake.notified()).await.is_err());
}

/// After a commit, and only then: a price wakes the ledger; a link the backfill and the ledger; a sync each reading key's
/// ledger and balance syncs (D-SYNC-WAKES); on both surfaces.
#[tokio::test(flavor = "current_thread")]
async fn wakes_follow_commits_on_both_surfaces() {
    let (_dir, c, owner, app, _b) = install();
    let id = row(&c, owner, 8, "ETH", "2", "2026-09-02 10:00:00");
    let w = row(&c, owner, 5, "BTC", "0.3", "2026-09-03 12:00:00");
    row(&c, owner, 4, "BTC", "0.2999", "2026-09-04 12:00:00");
    let key: i64 = c.query_row("SELECT id FROM api_keys WHERE user_id=?1", [owner], |r| r.get(0)).unwrap();
    let me = Some(owner.to_string());
    let delta = |before: usize| wakes(&app)[before..].to_vec();
    let mut seen = wakes(&app).len();
    let mut step = |expected: Vec<(&str, Option<String>)>| { let got = delta(seen); seen += got.len(); assert_eq!(got, expected.into_iter().map(|(n, s)| (n.to_string(), s)).collect::<Vec<_>>()); };
    assert_eq!(rest(&app, "PATCH", &format!("/api/v1/transactions/account/{id}/price"), Some(json!({"price_usd":"3"}))).await.0, 200);
    step(vec![("tracker_ledger", me.clone())]);
    assert_eq!(rest(&app, "PATCH", &format!("/api/v1/transactions/account/{id}/price"), Some(json!({"price_usd":"-3"}))).await.0, 422);
    step(vec![]);
    assert!(!text(&mcp(&app, "set_transfer_link", json!({"transaction_id":w,"linked":true})).await).1);
    step(vec![("portfolio_backfill", me.clone()), ("tracker_ledger", me.clone())]);
    assert!(text(&mcp(&app, "set_transfer_link", json!({"transaction_id":w,"linked":true})).await).1);
    step(vec![]);
    assert_eq!(rest(&app, "POST", "/api/v1/tracker/sync", None).await.0, 202);
    step(vec![("ledger_sync", Some(key.to_string())), ("balance_sync", Some(key.to_string()))]);
    assert!(!text(&mcp(&app, "sync_tracker", json!({})).await).1);
    step(vec![("ledger_sync", Some(key.to_string())), ("balance_sync", Some(key.to_string()))]);
    c.execute("DELETE FROM api_keys", []).unwrap();
    assert_eq!(rest(&app, "POST", "/api/v1/tracker/sync", None).await.0, 422);
    step(vec![]);
}

/// D-EXPORT-SIZE: past 4 MB the export is refused on both surfaces (Rails builds any size).
#[tokio::test(flavor = "current_thread")]
async fn an_export_past_four_megabytes_is_refused() {
    let (_dir, c, owner, app, _b) = install();
    for i in 0..3 {
        let id = row(&c, owner, 4, "BTC", "1", &format!("2026-09-0{} 12:00:00", i + 1));
        c.execute("UPDATE account_transactions SET description=?1 WHERE id=?2", ("x".repeat(1_500_000), id)).unwrap();
    }
    assert_eq!(rest(&app, "GET", "/api/v1/transactions/export", None).await,
               (422, json!({"data":null,"error":{"code":"export_too_large","message":api::EXPORT_TOO_LARGE}})));
    assert_eq!(text(&mcp(&app, "export_transactions_csv", json!({})).await), (api::EXPORT_TOO_LARGE, true));
    c.execute("UPDATE account_transactions SET description='small'", []).unwrap();
    assert_eq!(rest(&app, "GET", "/api/v1/transactions/export?from_date=2026-09-02", None).await.0, 200);
}

/// D-MCP-LIST-TEXT: the list's text passes read_limits (64 KB), as every MCP read's.
#[tokio::test(flavor = "current_thread")]
async fn the_mcp_list_is_bounded_at_64_kilobytes() {
    let (_dir, c, owner, app, _b) = install();
    let id = row(&c, owner, 4, "B", "1", "2026-09-02 10:00:00");
    let length = |result: Value| text(&result).0.len();
    let one = length(mcp(&app, "list_account_transactions", json!({})).await);
    let fits = 65_536 - one + 1;
    c.execute("UPDATE account_transactions SET base_currency=?1 WHERE id=?2", ("B".repeat(fits), id)).unwrap();
    let at_limit = mcp(&app, "list_account_transactions", json!({})).await;
    assert_eq!(text(&at_limit).0.len(), 65_536);
    c.execute("UPDATE account_transactions SET base_currency=?1 WHERE id=?2", ("B".repeat(fits + 1), id)).unwrap();
    assert_eq!(text(&mcp(&app, "list_account_transactions", json!({})).await), ("Read unavailable: response exceeds 65536 bytes", false));
}

async fn patch(app: &App, b: &mut Browser, id: i64, value: &str) -> web::Answer {
    b.send(app, "PATCH", &format!("/tracker/transactions/{id}/price"), Some(&[("price", value)]), Csrf::Header, HEADERS).await
}

/// D-PRICE-WAITING: a row whose own daily price can still come (its UTC day is not over, and its key has been priced
/// before) shows "Waiting…" with the box disabled and refuses a stated price on every surface, changing nothing. Once
/// the day is over, or when waiting could never end, the box is open as before.
#[tokio::test(flavor = "current_thread")]
async fn a_row_waits_for_its_days_price_until_the_utc_day_is_over() {
    let (_dir, c, owner, app, mut b, clock) = install_clocked();
    c.execute("UPDATE users SET display_currency='USD', time_zone='Europe/Warsaw'", []).unwrap();
    let venue: String = c.query_row("SELECT type FROM exchanges WHERE id=?1", [exchange(&c)], |r| r.get(0)).unwrap();
    let key = |c: &Connection, base: &str| deltabadger::tracker::prices::row_price_key(c, owner, exchange(c), &venue, base, None, chrono::NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()).unwrap().unwrap();
    let price = |c: &Connection, asset: &str, currency: &str, date: &str, value: &str| { c.execute("INSERT INTO historical_prices(asset,currency,date,price) VALUES (?1,?2,?3,?4)", (asset, currency, date, value)).unwrap(); };
    let eth = key(&c, "ETH");
    price(&c, &eth, "USD", "2026-09-01", "2500");

    // Today and a future day wait; every surface refuses, nothing changes, nothing wakes.
    for at in ["2026-09-10 00:00:00", "2026-09-12 10:00:00"] {
        let id = row(&c, owner, 10, "ETH", "0.01", at);
        let (before, woken) = (stored(&c, id), wakes(&app));
        let answer = patch(&app, &mut b, id, "5").await;
        assert_eq!(answer.status, 422, "{at}: {}", answer.body);
        let (status, body) = rest(&app, "PATCH", &format!("/api/v1/transactions/account/{id}/price"), Some(json!({"price_usd":"5"}))).await;
        assert_eq!((status, body["error"]["code"].clone()), (409, json!("price_pending")), "{body}");
        assert_eq!(text(&mcp(&app, "set_transaction_price", json!({"transaction_id":id,"price_usd":"5"})).await), (body["error"]["message"].as_str().unwrap(), true));
        assert_eq!(stored(&c, id), before);
        assert_eq!(wakes(&app), woken);
        // Clearing is always allowed, and a cleared stated price goes back to waiting.
        c.execute("UPDATE account_transactions SET manual_values='{\"price\":\"0\"}' WHERE id=?1", [id]).unwrap();
        let answer = patch(&app, &mut b, id, "").await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert!(answer.body.contains("tracker-row__price--waiting") && answer.body.contains(" disabled placeholder=\"Waiting…\"") && answer.body.contains("Waiting for the end-of-day price."), "{}", answer.body);
        assert!(!answer.body.contains("could not price"), "{}", answer.body);
        assert_eq!(rest(&app, "PATCH", &format!("/api/v1/transactions/account/{id}/price"), Some(json!({"price_usd":null}))).await.0, 200);
        let cleared = mcp(&app, "set_transaction_price", json!({"transaction_id":id,"price_usd":""})).await;
        assert!(!text(&cleared).1, "{cleared}");
    }

    // Open as before: yesterday's row, a zero amount, a key never priced, only zero or non-USD history, a stated price.
    let yesterday = row(&c, owner, 10, "ETH", "0.01", "2026-09-09 23:59:59");
    let zero = row(&c, owner, 10, "ETH", "0", "2026-09-10 10:00:00");
    let unknown = row(&c, owner, 10, "ZZQX", "1", "2026-09-10 10:00:00");
    let zeroed = row(&c, owner, 10, "ZZQY", "1", "2026-09-10 10:00:00");
    price(&c, &key(&c, "ZZQY"), "USD", "2026-09-01", "0");
    let euro = row(&c, owner, 10, "ZZQZ", "1", "2026-09-10 10:00:00");
    price(&c, &key(&c, "ZZQZ"), "EUR", "2026-09-01", "3");
    for id in [yesterday, zero, unknown, zeroed, euro] {
        let answer = patch(&app, &mut b, id, "5").await;
        assert_eq!(answer.status, 200, "{id}: {}", answer.body);
        assert!(!answer.body.contains("--waiting"), "{}", answer.body);
    }
    let stated = row(&c, owner, 10, "ETH", "0.01", "2026-09-10 10:00:00");
    c.execute("UPDATE account_transactions SET manual_values='{\"price\":\"7\"}' WHERE id=?1", [stated]).unwrap();
    assert_eq!(rest(&app, "PATCH", &format!("/api/v1/transactions/account/{stated}/price"), Some(json!({"price_usd":"0"}))).await.0, 200, "a stated price is restated");

    // A price we have for today is shown, never hidden behind waiting.
    let priced = row(&c, owner, 10, "ETH", "0.01", "2026-09-10 11:00:00");
    price(&c, &eth, "USD", "2026-09-10", "2600");
    let answer = patch(&app, &mut b, priced, "").await;
    assert!(answer.body.contains("tracker-row__price--ours") && answer.body.contains("placeholder=\"2600"), "{}", answer.body);
    c.execute("DELETE FROM historical_prices WHERE date='2026-09-10'", []).unwrap();

    // The UTC midnight decides, not the owner's zone (Warsaw is already on the 11th at 22:00Z).
    let edge = row(&c, owner, 10, "ETH", "0.01", "2026-09-10 23:00:00");
    clock.set(web::at("2026-09-10T23:59:59Z"));
    assert_eq!(patch(&app, &mut b, edge, "5").await.status, 422);
    clock.set(web::at("2026-09-11T00:00:00Z"));
    let answer = patch(&app, &mut b, edge, "5").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.body.contains("tracker-row__price--stated"), "{}", answer.body);
}

fn fill(c: &Connection, user: i64, kind: i64, base: &str, amount: &str, (quote, paid): (&str, &str), at: &str) -> i64 {
    let id = row(c, user, kind, base, amount, at);
    c.execute("UPDATE account_transactions SET quote_currency=?1, quote_amount=?2 WHERE id=?3", (quote, paid, id)).unwrap();
    id
}
fn cfee(c: &Connection, user: i64, base: &str, amount: &str, at: &str) -> i64 {
    let id = row(c, user, 10, base, amount, at);
    c.execute("UPDATE account_transactions SET raw_data='{\"activity_type\":\"CFEE\"}' WHERE id=?1", [id]).unwrap();
    id
}
fn fill_price(c: &Connection, owner: i64, id: i64) -> Option<(String, String)> {
    let all = deltabadger::tracker::rows::load_all(c, owner).unwrap();
    let mine = all.iter().find(|r| r.id == id).unwrap().cfee.fill_price.clone();
    // The page reads the same rows on its own.
    assert_eq!(deltabadger::tracker::rows::cfee_price(c, owner, id).unwrap(), mine);
    mine.map(|(p, u)| (p.to_s_f(), u))
}

/// D-TAX-CFEE-PREVIOUS-DAY: an Alpaca CFEE dated D is priced at its coin's fills on the UTC day before (buys, else
/// sales), Σquote / Σbase, when all are quoted in one cash currency (a stablecoin reads as dollars); no price otherwise.
#[test]
fn a_cfee_is_priced_at_the_fills_of_the_day_before() {
    let (_dir, c, owner, _app, _b) = install();
    let at = |day: u32, time: &str| format!("2026-09-{day:02} {time}");
    // One buy.
    fill(&c, owner, 0, "BTC", "0.5", ("USD", "30000"), &at(1, "10:00:00"));
    let one = cfee(&c, owner, "BTC", "0.001", &at(2, "00:00:00"));
    assert_eq!(fill_price(&c, owner, one), Some(("60000.0".into(), "USD".into())));
    // Buys at two prices, a sale beside them, a stablecoin quote: the buys only, by amount.
    fill(&c, owner, 0, "ETH", "1", ("USD", "2000"), &at(3, "00:00:00"));
    fill(&c, owner, 0, "ETH", "3", ("USDC", "6600"), &at(3, "23:59:59"));
    fill(&c, owner, 1, "ETH", "1", ("USD", "9999"), &at(3, "12:00:00"));
    let buys = cfee(&c, owner, "ETH", "0.01", &at(4, "00:00:00"));
    assert_eq!(fill_price(&c, owner, buys), Some(("2150.0".into(), "USD".into())));
    // Only sales.
    fill(&c, owner, 1, "SOL", "2", ("USD", "300"), &at(5, "08:00:00"));
    let sold = cfee(&c, owner, "SOL", "0.1", &at(6, "00:00:00"));
    assert_eq!(fill_price(&c, owner, sold), Some(("150.0".into(), "USD".into())));
    // A fill on the fee's own day, or two days before, is not its fill.
    fill(&c, owner, 0, "AVAX", "1", ("USD", "20"), &at(8, "00:00:00"));
    fill(&c, owner, 0, "AVAX", "1", ("USD", "20"), &at(6, "23:59:59"));
    let none = cfee(&c, owner, "AVAX", "0.1", &at(8, "00:00:00"));
    assert_eq!(fill_price(&c, owner, none), None);
    // Two cash currencies, or a buy quoted in a coin (buys first: the dollar sale beside it is not taken).
    fill(&c, owner, 0, "DOT", "1", ("USD", "5"), &at(9, "10:00:00"));
    fill(&c, owner, 0, "DOT", "1", ("EUR", "5"), &at(9, "11:00:00"));
    let mixed = cfee(&c, owner, "DOT", "0.01", &at(10, "00:00:00"));
    assert_eq!(fill_price(&c, owner, mixed), None);
    fill(&c, owner, 0, "LINK", "1", ("BTC", "0.0002"), &at(11, "10:00:00"));
    fill(&c, owner, 1, "LINK", "1", ("USD", "12"), &at(11, "11:00:00"));
    let coin = cfee(&c, owner, "LINK", "0.01", &at(12, "00:00:00"));
    assert_eq!(fill_price(&c, owner, coin), None);
    // A fee row that is not a CFEE has none.
    fill(&c, owner, 0, "ADA", "10", ("USD", "5"), &at(13, "10:00:00"));
    let plain = row(&c, owner, 10, "ADA", "1", &at(14, "00:00:00"));
    assert_eq!(fill_price(&c, owner, plain), None);
}

/// The walk values a priced CFEE at its fills' price, with no daily price fetched for it (NoFetch fails on any request),
/// and keeps today's lookup for one with no fills.
#[test]
fn the_walk_values_a_cfee_at_its_fills_price_without_fetching() {
    use deltabadger::tracker::{fx::FxTable, prices::{NoFetch, PriceService, Reference}, rows, walk};
    let (_dir, c, owner, _app, _b) = install();
    fill(&c, owner, 0, "BTC", "0.5", ("USD", "30000"), "2026-09-01 10:00:00");
    fill(&c, owner, 0, "BTC", "0.5", ("USD", "31000"), "2026-09-01 20:00:00");
    let fee = cfee(&c, owner, "BTC", "0.001", "2026-09-02 00:00:00");
    let stored = rows::load(&c, owner).unwrap();
    let deposits = stored.iter().map(|r| (r.id, r.amount.clone())).collect();
    let (reference, table) = (Reference::default(), FxTable::default());
    let mut none = NoFetch;
    let mut ps = PriceService::new(&reference, &table, chrono::NaiveDate::from_ymd_opt(2026, 9, 20).unwrap(), &mut none);
    let w = walk::walk_rows(&stored, &deposits, &mut ps).unwrap();
    let uid = stored.iter().position(|r| r.id == fee).unwrap();
    let r = w.rows.iter().find(|r| r.uid == uid).unwrap();
    assert_eq!((r.fiat_value.to_s_f(), r.price_missing), ("61.0".to_string(), false));
    assert!(ps.warnings.is_empty(), "{:?}", ps.warnings);
}

/// The tracker row shows a priced CFEE's price as ours is shown, with its own explanation; a stated price still wins;
/// with no fills it is the daily close, or waiting, as before.
#[tokio::test(flavor = "current_thread")]
async fn a_cfee_row_shows_its_fills_price() {
    let (_dir, c, owner, app, mut b, _clock) = install_clocked();
    c.execute("UPDATE users SET display_currency='USD'", []).unwrap();
    fill(&c, owner, 0, "BTC", "0.5", ("USD", "30000"), "2026-09-09 10:00:00");
    // Charged today, before today's close exists: priced at once, not waiting.
    let fee = cfee(&c, owner, "BTC", "0.001", "2026-09-10 00:00:00");
    let answer = patch(&app, &mut b, fee, "").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.body.contains("tracker-row__price--fills") && answer.body.contains("placeholder=\"60000.0\"")
            && answer.body.contains("The price of the trades this fee was charged on."), "{}", answer.body);
    assert!(answer.body.contains("<td class=\"tracker-row__value\">60.00</td>"), "{}", answer.body);
    let answer = patch(&app, &mut b, fee, "55000").await;
    assert!(answer.body.contains("tracker-row__price--stated") && answer.body.contains("value=\"55000"), "{}", answer.body);
    // No fills the day before: today's own behaviour.
    let alone = cfee(&c, owner, "ETH", "0.01", "2026-09-10 00:00:00");
    let answer = patch(&app, &mut b, alone, "").await;
    assert!(!answer.body.contains("--fills"), "{}", answer.body);
}
