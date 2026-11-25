//! PLAN-710-mcpadapter (D-IMPORT-API): `import_transactions_csv` over MCP and REST: the web's import, its choices and its
//! one write, through the same operation; the bearer checked before any body is read, while a file arrives and after;
//! closed forms; nothing of a file in MCP's history.
mod common;
use axum::body::{to_bytes, Body, Bytes};
use axum::http::Request;
use base64::Engine;
use common::seed;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::identity;
use deltabadger::web::{csrf, import_api, session, App};
use futures_util::StreamExt;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const MCP: &str = "imp-mcp";
const API: &str = "imp-api";
const CHUNK: usize = 64 * 1024;

struct Install { _d: tempfile::TempDir, c: Connection, user: i64, app: App, b: Browser, _clock: Arc<TestClock> }

/// An install whose owner has the import on for MCP and REST (`on`), a client granted every tool, its two tokens.
fn install(on: bool) -> Install {
    let (d, opened, seeded) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1, time_zone='UTC'", []).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let clock = TestClock::at("2026-09-20T12:00:00Z");
    let app = web::app(d.path(), web::SECRET, clock.clone());
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let b = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) };
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('imp','imp','http://localhost/cb',0,'mcp api','2026-09-01 12:00:00','2026-09-01 12:00:00')", []).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [(MCP, "mcp"), (API, "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-20 11:00:00',86400)", (application, seeded.user_id, token, scope)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-01 12:00:00','2026-09-01 12:00:00')", (seeded.user_id, application)).unwrap();
    if on { c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1", [json!({"tool_permissions": {import_api::TOOL: true}}).to_string()]).unwrap(); }
    Install { _d: d, c, user: seeded.user_id, app, b, _clock: clock }
}

fn venue(c: &Connection, exchange_type: &str, name: &str) -> i64 {
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?2, '0.1', '0.1', '2026-01-01', '2026-01-01')", [exchange_type, name]).unwrap();
    c.last_insert_rowid()
}
/// A Kraken of `n` accounts: number 1 keyed, the rest keyless. Their ids, by number.
fn krakens(c: &Connection, user: i64, n: usize) -> (i64, Vec<i64>) {
    let kraken = venue(c, "Exchanges::Kraken", "Kraken");
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![user, kraken, e("kraken-key"), e("s")]).unwrap();
    identity::ensure_account(c, user, kraken, chrono::Utc::now()).unwrap();
    let mut ids = vec![identity::Account::first(c, user, kraken).unwrap().id.unwrap()];
    for _ in 1..n { ids.push(identity::new_account(c, user, kraken, chrono::Utc::now()).unwrap()); }
    (kraken, ids)
}
fn ledger_csv(rows: &[(&str, &str, u32, u32)]) -> String {
    let lines: Vec<String> = rows.iter().map(|(tx, coin, day, n)| format!("2023-01-{day:02}T10:00:00Z,deposit,{coin},1,,,,,kraken,{tx},,,{n}")).collect();
    format!("{},account\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, lines.join("\n"))
}
/// A Deltabadger CSV that asks "Import the rest": Kraken 1 here, a row of Kraken 2 left out.
fn rest_csv() -> String { ledger_csv(&[("T1", "BTC", 5, 1), ("T2", "BTC", 6, 2)]) }
fn ledger(c: &Connection) -> i64 { c.query_row("SELECT count(*) FROM account_transactions", [], |r| r.get(0)).unwrap() }
fn b64(text: &str) -> String { base64::engine::general_purpose::STANDARD.encode(text) }

struct Sent { status: u16, retry_after: Option<String>, body: String }
impl Sent { fn json(&self) -> Value { serde_json::from_str(&self.body).unwrap_or(json!(self.body)) } }

async fn send(app: &App, method: &str, path: &str, token: Option<&str>, content_type: Option<&str>, session: Option<&str>, body: Body) -> (Sent, Option<String>) {
    let mut request = Request::builder().method(method).uri(path).header("host", "localhost:3000").header("accept", "application/json, text/event-stream");
    if let Some(token) = token { request = request.header("authorization", format!("Bearer {token}")); }
    if let Some(t) = content_type { request = request.header("content-type", t); }
    if let Some(session) = session { request = request.header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25"); }
    let response = deltabadger::web::router(app.clone()).oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let session = response.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_string());
    let retry_after = response.headers().get("retry-after").map(|v| v.to_str().unwrap().to_string());
    let body = String::from_utf8(to_bytes(response.into_body(), 64_000_000).await.unwrap().to_vec()).unwrap();
    (Sent { status, retry_after, body }, session)
}

/// REST with a JSON body.
async fn rest_json(app: &App, path: &str, body: Value) -> Sent {
    send(app, "POST", path, Some(API), Some("application/json"), None, Body::from(body.to_string())).await.0
}
/// REST's fresh upload.
async fn rest_upload(app: &App, fields: &[(&str, &str)], name: &str, text: &str) -> Sent {
    let (content_type, body) = web::multipart(fields, &[("file", name, text.as_bytes())]);
    send(app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, Body::from(body)).await.0
}

/// An initialized MCP session.
async fn session(app: &App) -> String {
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"imp","version":"1"}}});
    let (_, session) = send(app, "POST", "/mcp", Some(MCP), Some("application/json"), None, Body::from(init.to_string())).await;
    let session = session.unwrap();
    let (ack, _) = send(app, "POST", "/mcp", Some(MCP), Some("application/json"), Some(&session), Body::from(json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string())).await;
    assert_eq!(ack.status, 202);
    session
}
/// A raw JSON-RPC message on `session`.
async fn rpc(app: &App, session: &str, raw: &str) -> Sent { send(app, "POST", "/mcp", Some(MCP), Some("application/json"), Some(session), Body::from(raw.to_string())).await.0 }
/// The tool's result through MCP: its data, or its error's `{code, message}`, and whether it is an error.
async fn mcp(app: &App, args: Value) -> (Value, bool) {
    let s = session(app).await;
    let sent = rpc(app, &s, &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":import_api::TOOL,"arguments":args}}).to_string()).await;
    let result = sent.json()["result"].clone();
    let text = result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{}", sent.body)).to_string();
    (serde_json::from_str(&text).unwrap_or(json!(text)), result["isError"] == true)
}

/// A body of `content` in CHUNK-sized chunks that counts its polls and runs `at_first` when it is first polled.
fn counted(content: Vec<u8>, polls: Arc<AtomicUsize>, at_first: impl FnOnce() + Send + 'static) -> Body {
    let chunks: Vec<Bytes> = content.chunks(CHUNK).map(Bytes::copy_from_slice).collect();
    let mut first = Some(at_first);
    Body::from_stream(futures_util::stream::iter(chunks).map(move |chunk| {
        if polls.fetch_add(1, Ordering::SeqCst) == 0 { if let Some(first) = first.take() { first(); } }
        Ok::<_, std::convert::Infallible>(chunk)
    }))
}

/// Off by default (rule 5): REST answers 403 tool_disabled before reading the body; MCP lists no such tool and answers a
/// call -32602.
#[tokio::test(flavor = "current_thread")]
async fn the_tool_is_off_by_default() {
    let i = install(false);
    let polls = Arc::new(AtomicUsize::new(0));
    let (content_type, body) = web::multipart(&[], &[("file", "a.csv", rest_csv().as_bytes())]);
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, counted(body, polls.clone(), || {})).await;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone(), polls.load(Ordering::SeqCst)), (403, json!("tool_disabled"), 0));
    let s = session(&i.app).await;
    let listed = rpc(&i.app, &s, r#"{"jsonrpc":"2.0","id":5,"method":"tools/list","params":{}}"#).await.json();
    assert!(!listed["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == import_api::TOOL), "{listed}");
    let called = rpc(&i.app, &s, &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":import_api::TOOL,"arguments":{"content":b64("x")}}}).to_string()).await.json();
    assert_eq!(called["error"]["code"], -32602, "{called}");
}

/// Rule 2, F8, H5: a Deltabadger CSV through MCP asks what the web asks ("Import the rest", its lines in the owner's
/// language), its one choice of kind `rest` with its destination; the choice writes once, a second answer is no file.
#[tokio::test(flavor = "current_thread")]
async fn a_csv_question_through_mcp_writes_once() {
    let i = install(true);
    let (kraken, ids) = krakens(&i.c, i.user, 1);
    i.c.execute("UPDATE users SET locale='de'", []).unwrap();
    let (asked, failed) = mcp(&i.app, json!({"content": b64(&rest_csv()), "filename": "ledger.csv"})).await;
    assert!(!failed, "{asked}");
    assert_eq!((asked["kind"].clone(), asked["actions"].clone()), (json!("warning"), json!(["choose", "cancel"])), "{asked}");
    let choice = &asked["choices"][0];
    assert_eq!((choice["kind"].clone(), choice["label"].clone(), choice["rows_in_file"].clone()), (json!("rest"), json!(deltabadger::web::i18n::text("de", "tracker.import.import_rest", &[])), Value::Null));
    assert_eq!(choice["destinations"], json!([{"exchange_id": kraken, "account_id": ids[0], "rows": 1, "prospective": null}]));
    assert!(asked["lines"].as_array().unwrap().iter().all(|l| !l.as_str().unwrap().contains('<')), "plain text: {asked}");
    let (done, failed) = mcp(&i.app, json!({"upload": asked["upload"], "choice": choice["choice"]})).await;
    assert!(!failed, "{done}");
    assert_eq!((done["imported"].clone(), done["destinations_written"].clone()), (json!(1), json!([{"exchange_id": kraken, "account_id": ids[0], "rows": 1}])));
    assert_eq!(ledger(&i.c), 1);
    let again = rest_json(&i.app, "/api/v1/tracker/import", json!({"upload": asked["upload"], "choice": choice["choice"]})).await;
    assert_eq!((again.status, again.json()["error"]["code"].clone()), (422, json!("no_file")));
    assert_eq!(ledger(&i.c), 1);
}

/// Parity (G8/F9): a question the web issued, answered through REST, writes once; the web's own second answer of it is
/// no file. The REST question asked the same choices the web's page offers.
#[tokio::test(flavor = "current_thread")]
async fn a_web_question_answered_through_rest_writes_once() {
    let mut i = install(true);
    krakens(&i.c, i.user, 1);
    let (content_type, body) = web::multipart(&[("format", "deltabadger")], &[("file", "ledger.csv", rest_csv().as_bytes())]);
    let asked = i.b.send_bytes(&i.app, "POST", "/tracker/import", Some(body), Csrf::Header, &[("content-type", content_type.as_str())]).await;
    let token = common::import::token(&asked.body).unwrap();
    let handles = common::import::handles(&asked.body);
    assert_eq!(handles.len(), 1, "{}", asked.body);
    // Branch review r1 #12: both claims overlap: the web's and REST's confirmations start together; exactly one writes.
    let fields = [("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handles[0].as_str())];
    let mut web_b = i.b.clone();
    let (done, again) = tokio::join!(rest_json(&i.app, "/api/v1/tracker/import", json!({"upload": token, "choice": handles[0]})),
                                     web_b.send(&i.app, "POST", "/tracker/import", Some(&fields), Csrf::Header, &[]));
    let rest_wrote = done.status == 200 && done.json()["data"]["imported"] == json!(1);
    let web_wrote = again.status == 200 && again.body.contains("/tracker");
    let no_file = deltabadger::web::i18n::text("en", "tracker.import.error_no_file", &[]);
    assert!(rest_wrote != web_wrote, "exactly one wrote: {} {} / {} {}", done.status, done.body, again.status, again.body);
    assert!(if rest_wrote { again.status == 422 && again.body.contains(&no_file) } else { done.json()["error"]["code"] == json!("no_file") }, "the other is no file");
    assert_eq!(ledger(&i.c), 1);
    // Another such file through REST asks the same one choice.
    let rest_asked = rest_upload(&i.app, &[], "ledger.csv", &ledger_csv(&[("T3", "BTC", 7, 1), ("T4", "BTC", 8, 2)])).await;
    assert_eq!(rest_asked.json()["data"]["choices"].as_array().map(Vec::len), Some(1), "{}", rest_asked.body);
}

/// Cancel through MCP closes the question: the web's confirmation of it is no file, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_through_mcp_closes_the_question() {
    let mut i = install(true);
    krakens(&i.c, i.user, 1);
    let (asked, _) = mcp(&i.app, json!({"content": b64(&rest_csv())})).await;
    let (cancelled, failed) = mcp(&i.app, json!({"upload": asked["upload"], "cancel": true})).await;
    assert_eq!((cancelled, failed), (json!({"cancelled": true}), false));
    let token = asked["upload"].as_str().unwrap();
    let handle = asked["choices"][0]["choice"].as_str().unwrap();
    let web = i.b.send(&i.app, "POST", "/tracker/import", Some(&[("token", token), ("format", "deltabadger"), ("api_key_id", handle)]), Csrf::Header, &[]).await;
    assert_eq!(web.status, 422, "{}", web.body);
    assert_eq!(ledger(&i.c), 0);
}

/// F3: with both slots held, a fresh upload through MCP is busy (retry after 5 s), and REST's is a 503 with
/// Retry-After before any body poll (G5).
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn every_form_waits_for_a_slot_with_a_bound() {
    let i = install(true);
    krakens(&i.c, i.user, 1);
    let held = (deltabadger::tracker::import::service::permit(&i.app).await, deltabadger::tracker::import::service::permit(&i.app).await);
    let (busy, failed) = mcp(&i.app, json!({"content": b64(&rest_csv())})).await;
    assert_eq!((busy["code"].clone(), busy["retry_after"].clone(), failed), (json!("busy"), json!(5), true));
    let polls = Arc::new(AtomicUsize::new(0));
    let (content_type, body) = web::multipart(&[], &[("file", "a.csv", rest_csv().as_bytes())]);
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, counted(body, polls.clone(), || {})).await;
    assert_eq!((sent.status, sent.retry_after.as_deref(), sent.json()["error"]["code"].clone(), polls.load(Ordering::SeqCst)), (503, Some("5"), json!("busy"), 0));
    let confirm = rest_json(&i.app, "/api/v1/tracker/import", json!({"upload": "0123456789abcdef", "choice": format!("0123456789abcdef.1.0.{}", "a".repeat(32))})).await;
    assert_eq!(confirm.status, 503, "a confirmation waits too: {}", confirm.body);
    drop(held);
    assert_eq!(ledger(&i.c), 0);
}

/// H3/G2/I3/J2: one closed form, exactly: a mixed form, a member named twice (anywhere in the message), a web marker, a
/// second file part or filename, a differing filename field: refused, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn only_one_closed_form_is_taken() {
    let i = install(true);
    krakens(&i.c, i.user, 1);
    let (mixed, failed) = mcp(&i.app, json!({"upload": "0123456789abcdef", "choice": "x", "cancel": true})).await;
    assert!(failed && mixed.as_str().is_some_and(|t| t.starts_with("Invalid input")), "{mixed}");
    let (mixed, failed) = mcp(&i.app, json!({"upload": "0123456789abcdef", "content": b64(&rest_csv())})).await;
    assert!(failed, "{mixed}");
    let s = session(&i.app).await;
    let content = b64(&rest_csv());
    for raw in [format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"{}","arguments":{{"content":"{content}","content":"{content}"}}}}}}"#, import_api::TOOL),
                format!(r#"{{"jsonrpc":"2.0","id":3,"id":4,"method":"tools/call","params":{{"name":"{}","arguments":{{"content":"{content}"}}}}}}"#, import_api::TOOL),
                format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"{}","name":"{}","arguments":{{"content":"{content}"}}}}}}"#, import_api::TOOL, import_api::TOOL)] {
        let sent = rpc(&i.app, &s, &raw).await.json();
        assert_eq!((sent["error"]["code"].clone(), sent["error"]["message"].clone()), (json!(-32602), json!("ambiguous_request")), "{raw}");
    }
    let api = "/api/v1/tracker/import";
    let sent = send(&i.app, "POST", api, Some(API), Some("application/json"), None, Body::from(r#"{"upload":"0123456789abcdef","upload":"fedcba9876543210","cancel":true}"#)).await.0;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (400, json!("ambiguous_request")));
    assert_eq!(rest_json(&i.app, api, json!({"content": content})).await.status, 400, "REST's JSON is no fresh upload");
    for fields in [&[("token", "0123456789abcdef")][..], &[("api_key_id", "999")], &[("filename", "other.csv")], &[("format", "binance"), ("format", "kucoin")], &[("format", "nope")]] {
        let sent = rest_upload(&i.app, fields, "ledger.csv", &rest_csv()).await;
        assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (400, json!("ambiguous_request")), "{fields:?}");
    }
    let (content_type, two) = web::multipart(&[], &[("file", "a.csv", rest_csv().as_bytes()), ("file", "b.csv", rest_csv().as_bytes())]);
    assert_eq!(send(&i.app, "POST", api, Some(API), Some(&content_type), None, Body::from(two)).await.0.status, 400, "two files");
    let boundary = web::MULTIPART_BOUNDARY;
    let twice = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a (UTC+1).csv\"; filename=\"a (UTC+3).csv\"\r\n\r\n{}\r\n--{boundary}--\r\n", rest_csv());
    let sent = send(&i.app, "POST", api, Some(API), Some(&format!("multipart/form-data; boundary={boundary}")), None, Body::from(twice)).await.0;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (400, json!("ambiguous_request")), "J2: a filename given twice");
    assert_eq!(ledger(&i.c), 0);
}

/// F1/G1/I5: REST checks the bearer, the grant and the account from the headers alone: a refused request's multipart, JSON
/// or urlencoded body is never polled, and each refusal has its own code.
#[tokio::test(flavor = "current_thread")]
async fn rest_refuses_before_any_body_poll() {
    let i = install(true);
    let (multipart_type, multipart) = web::multipart(&[], &[("file", "a.csv", rest_csv().as_bytes())]);
    let bodies = [(multipart_type.as_str(), multipart), ("application/json", br#"{"upload":"0123456789abcdef","cancel":true}"#.to_vec()), ("application/x-www-form-urlencoded", b"upload=0123456789abcdef&cancel=1".to_vec())];
    let refused = |token: Option<&'static str>| token;
    for (token, status, code) in [(refused(None), 401, "missing_token"), (Some("nope"), 401, "invalid_token"), (Some(MCP), 403, "insufficient_scope")] {
        for (content_type, body) in &bodies {
            let polls = Arc::new(AtomicUsize::new(0));
            let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import.json", token, Some(content_type), None, counted(body.clone(), polls.clone(), || {})).await;
            assert_eq!((sent.status, sent.json()["error"]["code"].clone(), polls.load(Ordering::SeqCst)), (status, json!(code), 0), "{token:?} {content_type}");
        }
    }
    i.c.execute("UPDATE users SET locked_at='2026-09-20 11:59:00'", []).unwrap();
    let polls = Arc::new(AtomicUsize::new(0));
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&multipart_type), None, counted(bodies[0].1.clone(), polls.clone(), || {})).await;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone(), polls.load(Ordering::SeqCst)), (403, json!("account_inactive"), 0), "G1: an inactive owner");
    i.c.execute("UPDATE users SET locked_at=NULL", []).unwrap();
    i.c.execute("UPDATE oauth_access_tokens SET resource_owner_id = 999999 WHERE token = ?1", [API]).unwrap();
    let sent = rest_json(&i.app, "/api/v1/tracker/import", json!({"upload": "0123456789abcdef", "cancel": true})).await;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (401, json!("user_not_found")), "I5: a token whose owner is gone");
}

/// H2: a query naming any form's marker is refused before the body is read; the handle still answers afterwards.
#[tokio::test(flavor = "current_thread")]
async fn a_query_marker_is_refused() {
    let i = install(true);
    krakens(&i.c, i.user, 1);
    let (asked, _) = mcp(&i.app, json!({"content": b64(&rest_csv())})).await;
    let (upload, choice) = (asked["upload"].as_str().unwrap(), asked["choices"][0]["choice"].as_str().unwrap());
    for query in [format!("upload={upload}"), format!("token={upload}"), format!("choice={choice}"), "cancel=1".to_string(), "api_key_id=1".into(), format!("handle={choice}")] {
        let sent = send(&i.app, "POST", &format!("/api/v1/tracker/import?{query}"), Some(API), Some("application/json"), None, Body::from(json!({"upload": upload, "cancel": true}).to_string())).await.0;
        assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (400, json!("ambiguous_request")), "{query}");
    }
    let done = rest_json(&i.app, "/api/v1/tracker/import", json!({"upload": upload, "choice": choice})).await;
    assert_eq!(done.json()["data"]["imported"], json!(1), "{}", done.body);
}

/// I2: the web's import reserves the API's markers: an API choice beside a fresh file is no file, nothing written, and
/// the question still answers.
#[tokio::test(flavor = "current_thread")]
async fn the_web_reserves_the_api_markers() {
    let mut i = install(true);
    krakens(&i.c, i.user, 1);
    let (asked, _) = mcp(&i.app, json!({"content": b64(&rest_csv())})).await;
    let choice = asked["choices"][0]["choice"].as_str().unwrap();
    let (content_type, body) = web::multipart(&[("format", "deltabadger"), ("choice", choice)], &[("file", "other.csv", ledger_csv(&[("Z9", "ETH", 9, 1)]).as_bytes())]);
    let web = i.b.send_bytes(&i.app, "POST", "/tracker/import", Some(body), Csrf::Header, &[("content-type", content_type.as_str())]).await;
    assert_eq!(web.status, 422, "{}", web.body);
    assert_eq!(ledger(&i.c), 0);
    let (done, _) = mcp(&i.app, json!({"upload": asked["upload"], "choice": choice})).await;
    assert_eq!(done["imported"], json!(1), "{done}");
}

/// J1/I4: a token revoked while its file arrives stops the read within 1 MiB, its slot freed; a client disconnected while
/// an unreadable file arrives is `invalid_token`, never the parse error.
#[tokio::test(flavor = "current_thread")]
async fn a_bearer_ended_mid_read_stops_the_read() {
    let i = install(true);
    let big = format!("{}{}", rest_csv(), "x".repeat(4 * 1024 * 1024));
    let (content_type, body) = web::multipart(&[], &[("file", "a.csv", big.as_bytes())]);
    let chunks = body.len().div_ceil(CHUNK);
    let polls = Arc::new(AtomicUsize::new(0));
    let path = i.c.path().unwrap().to_string();
    let revoke = move || {
        let c = Connection::open(&path).unwrap();
        c.execute("UPDATE oauth_access_tokens SET revoked_at = '2026-09-20 11:59:00' WHERE token = ?1", [API]).unwrap();
    };
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, counted(body, polls.clone(), revoke)).await;
    let read = polls.load(Ordering::SeqCst);
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (401, json!("token_revoked")), "{}", sent.body);
    assert!(read <= 1024 * 1024 / CHUNK + 1 && read < chunks, "{read} of {chunks} chunks polled");
    assert_eq!(i.app.import_slots.available_permits(), deltabadger::web::IMPORT_SLOTS, "the slot is freed");
    i.c.execute("UPDATE oauth_access_tokens SET revoked_at = NULL", []).unwrap();
    let (content_type, body) = web::multipart(&[], &[("file", "a.csv", b"\xff\xfe not a ledger")]);
    let path = i.c.path().unwrap().to_string();
    let disconnect = move || {
        let c = Connection::open(&path).unwrap();
        c.execute("DELETE FROM oauth_access_tokens WHERE token = ?1", [API]).unwrap();
    };
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, counted(body, Arc::new(AtomicUsize::new(0)), disconnect)).await;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (401, json!("invalid_token")), "{}", sent.body);
}

/// Rule 4, F7, J5: MCP's history keeps no file: no content, no file name, no row of it, not as the JSON-RPC id, not in a
/// cancellation of the call; the request and its answer are both kept, under one correlation id.
#[tokio::test(flavor = "current_thread")]
async fn history_keeps_nothing_of_the_file() {
    let i = install(true);
    krakens(&i.c, i.user, 1);
    let s = session(&i.app).await;
    let name = "secret-ledger-2026.csv";
    let csv = rest_csv();
    let call = json!({"jsonrpc":"2.0","id":name,"method":"tools/call","params":{"name":import_api::TOOL,"arguments":{"content":b64(&csv),"filename":name}}});
    let answered = rpc(&i.app, &s, &call.to_string()).await.json();
    assert_eq!(answered["id"], json!(name), "the wire echoes the client's id");
    rpc(&i.app, &s, &json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":name,"reason":"user"}}).to_string()).await;
    let rows: Vec<(String, Option<String>, String)> = i.c.prepare("SELECT message_json, jsonrpc_id, message_type FROM action_mcp_session_messages WHERE session_id = ?1").unwrap()
        .query_map([&s], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    for (message, id, _) in &rows {
        for secret in [name, b64(&csv).as_str(), "2023-01-05T10"] {
            assert!(!message.contains(secret) && id.as_deref() != Some(secret), "{secret} in {message} / {id:?}");
        }
    }
    let ids: Vec<&str> = rows.iter().filter(|(m, _, _)| m.contains(import_api::TOOL) || m.contains("cancelled") || m.contains("redacted")).filter_map(|(_, id, _)| id.as_deref()).collect();
    assert!(ids.len() >= 2 && ids.iter().all(|id| *id == ids[0] && id.starts_with("import-")), "{rows:?}");
    let cancelled = rows.iter().find(|(m, _, _)| m.contains("notifications/cancelled")).unwrap();
    assert!(cancelled.0.contains(ids[0]), "{cancelled:?}");
    let acknowledged: bool = i.c.query_row("SELECT request_acknowledged FROM action_mcp_session_messages WHERE session_id = ?1 AND message_type = 'request' AND jsonrpc_id = ?2", [&s, &ids[0].to_string()], |r| r.get(0)).unwrap();
    assert!(acknowledged, "the answer acknowledges its request");
}

/// G4/H5/F4: an account question's choices carry their account ids and their own destinations; the new-account choice has
/// no account id and names the account it would make by its number.
#[tokio::test(flavor = "current_thread")]
async fn an_account_question_carries_ids_and_a_prospective_new_account() {
    let i = install(true);
    let kucoin = venue(&i.c, "Exchanges::Kucoin", "KuCoin");
    let e = |v: &str| seed::cipher().encrypt(v);
    i.c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, 1, '2026-01-01', '2026-01-01')",
                rusqlite::params![i.user, kucoin, e("kk"), e("s"), e("p")]).unwrap();
    identity::ensure_account(&i.c, i.user, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(&i.c, i.user, kucoin).unwrap().id.unwrap();
    let second = identity::new_account(&i.c, i.user, kucoin, chrono::Utc::now()).unwrap();
    let file = include_str!("fixtures/kucoin/trading_account.csv");
    let (asked, failed) = mcp(&i.app, json!({"format": "kucoin", "content": b64(file)})).await;
    assert!(!failed, "{asked}");
    assert_eq!(asked["kind"], json!("account"), "{asked}");
    let choices = asked["choices"].as_array().unwrap();
    let by = |id: Value| choices.iter().find(|c| c["account_id"] == id).unwrap_or_else(|| panic!("{asked}"));
    for account in [first, second] {
        let c = by(json!(account));
        assert_eq!((c["kind"].clone(), c["destinations"][0]["account_id"].clone()), (json!("account"), json!(account)), "{c}");
        // Branch review r1 #9: the web's action with the account's plain label ("KuCoin 2"), never stripped markup.
        let label = identity::label_numbered(&i.c, account).unwrap().plain();
        assert_eq!(c["label"], json!(deltabadger::web::i18n::text("en", "tracker.import.import_to_html", &[("account", deltabadger::web::i18n::Arg::Text(&label)), ("count", deltabadger::web::i18n::Arg::Count(c["rows_in_file"].as_i64().unwrap()))])), "{c}");
        assert!(c["rows_in_file"].as_u64().is_some_and(|n| n > 0), "{c}");
    }
    let new = choices.iter().find(|c| c["kind"] == "new").unwrap_or_else(|| panic!("{asked}"));
    assert_eq!((new["account_id"].clone(), new["new_account"].clone()), (Value::Null, json!(true)));
    assert_eq!(new["destinations"][0]["prospective"], json!({"key_id": null, "number": 3}), "{new}");
    let (done, _) = mcp(&i.app, json!({"upload": asked["upload"], "choice": by(json!(second))["choice"]})).await;
    assert!(done["imported"].as_u64().is_some_and(|n| n > 0), "{done}");
}

/// H8/F10: an owner with every control 2.x has on (and the import off, its default) sees the Control group as `partial` on
/// both surfaces, its switch submitting `enabled=1`; the import's own switch is off.
#[tokio::test(flavor = "current_thread")]
async fn the_control_group_is_partial_with_the_import_off() {
    let mut i = install(false);
    let control = deltabadger::web::oauth::TOOL_GROUPS.iter().find(|(g, _)| *g == "control").unwrap().1;
    let on: serde_json::Map<String, Value> = control.iter().filter(|t| **t != import_api::TOOL).map(|t| (t.to_string(), json!(true))).collect();
    i.c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1", [json!({"tool_permissions": on}).to_string()]).unwrap();
    for surface in ["mcp", "api"] {
        let page = i.b.get(&i.app, &format!("/settings/{surface}")).await;
        assert_eq!(page.status, 200, "{surface}");
        let group = page.body.find("data-group=\"control\"").unwrap_or_else(|| panic!("{surface}: {}", page.body));
        let row = &page.body[group..group + page.body[group..].find("srow--tool").unwrap()];
        assert!(row.contains("toggle--partial") && row.contains("value=\"1\""), "{surface}: {row}");
        let own = page.body.find(&format!("data-tool=\"{}\"", import_api::TOOL)).unwrap_or_else(|| panic!("{surface}: no import switch"));
        assert!(!page.body[own..own + page.body[own..].find("</div>").unwrap()].contains("toggle--on"), "{surface}");
    }
}

/// Branch review r1 #1: every message is parsed for a member named twice, whatever its tool's name looks like in the raw
/// text: an escaped tool name, or a `method` given twice (the last one a ping), is refused and nothing of it is kept.
#[tokio::test(flavor = "current_thread")]
async fn a_member_named_twice_is_refused_whatever_the_tool_name_looks_like() {
    let i = install(true);
    krakens(&i.c, i.user, 1);
    let s = session(&i.app).await;
    let content = b64(&rest_csv());
    for raw in [format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"import_transactions\u005fcsv","arguments":{{"content":"{content}","content":"{content}"}}}}}}"#),
                format!(r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","method":"ping","params":{{"name":"x","arguments":{{"content":"{content}"}}}}}}"#)] {
        let sent = rpc(&i.app, &s, &raw).await.json();
        assert_eq!((sent["error"]["code"].clone(), sent["error"]["message"].clone()), (json!(-32602), json!("ambiguous_request")), "{raw}: {sent}");
    }
    let kept: i64 = i.c.query_row("SELECT count(*) FROM action_mcp_session_messages WHERE instr(message_json, ?1) > 0", [&content], |r| r.get(0)).unwrap();
    assert_eq!((kept, ledger(&i.c)), (0, 0));
}

/// Branch review r1 #2: the bearer is judged by a fresh clock after the last lookup, its owner's included: a token that
/// expires during that lookup is refused.
#[tokio::test(flavor = "current_thread")]
async fn a_token_expiring_during_the_last_user_lookup_is_refused() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let i = install(true);
    let authorization = format!("Bearer {API}");
    let reads = Arc::new(AtomicUsize::new(0));
    let (app, counted) = (i.app.clone(), reads.clone());
    let auth = authorization.clone();
    let first = i.app.db(move |c| {
        c.authorizer(Some(move |ctx: AuthContext<'_>| { if matches!(ctx.action, AuthAction::Read { table_name: "users", .. }) { counted.fetch_add(1, Ordering::SeqCst); } Authorization::Allow }));
        let out = import_api::authorized(c, &app, import_api::Surface::Rest, Some(&auth));
        c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        out
    }).await.unwrap();
    assert_eq!(first, Ok(i.user));
    let last = reads.load(Ordering::SeqCst);
    let (app, clock, seen) = (i.app.clone(), i._clock.clone(), Arc::new(AtomicUsize::new(0)));
    let denied = i.app.db(move |c| {
        c.authorizer(Some(move |ctx: AuthContext<'_>| {
            if matches!(ctx.action, AuthAction::Read { table_name: "users", .. }) && seen.fetch_add(1, Ordering::SeqCst) + 1 == last { clock.set(web::at("2026-09-21T12:00:00Z")); }
            Authorization::Allow
        }));
        let out = import_api::authorized(c, &app, import_api::Surface::Rest, Some(&authorization));
        c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        out
    }).await.unwrap();
    assert_eq!(denied, Err(deltabadger::tracker::import::service::Denied::Bearer(deltabadger::web::bearer::Refusal::Expired)));
}

/// Branch review r1 #3: a cancellation is kept with its `requestId` as a digest and no `reason`, whether or not its call
/// is in history yet (one arriving while the call's body still arrives finds no request row).
#[tokio::test(flavor = "current_thread")]
async fn a_cancellation_keeps_neither_its_request_id_nor_its_reason() {
    let i = install(true);
    let s = session(&i.app).await;
    let name = "secret-ledger-2026.csv";
    rpc(&i.app, &s, &json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":name,"reason":format!("stop {name}")}}).to_string()).await;
    let kept: Vec<String> = i.c.prepare("SELECT message_json FROM action_mcp_session_messages WHERE session_id = ?1 AND instr(message_json, 'cancelled') > 0").unwrap()
        .query_map([&s], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].contains(name) && !kept[0].contains("reason"), "{kept:?}");
}

/// Branch review r1 #7: a REST confirmation's body is followed by the bearer's check before it is read: a token revoked
/// while a malformed one arrives is `token_revoked`, never `ambiguous_request`.
#[tokio::test(flavor = "current_thread")]
async fn a_rest_confirmation_is_checked_once_its_body_arrived() {
    let i = install(true);
    let path = i.c.path().unwrap().to_string();
    let revoke = move || { Connection::open(&path).unwrap().execute("UPDATE oauth_access_tokens SET revoked_at = '2026-09-20 11:59:00' WHERE token = ?1", [API]).unwrap(); };
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some("application/json"), None, counted(br#"{"upload": 1, "nope": true}"#.to_vec(), Arc::new(AtomicUsize::new(0)), revoke)).await;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (401, json!("token_revoked")), "{}", sent.body);
}

/// Branch review r1 #8: the 1 MiB check comes before a chunk is kept: a token revoked before one large chunk keeps at most
/// 1 MiB of it; and a stalled stream is stopped by the five-second check.
#[tokio::test(flavor = "current_thread")]
async fn a_read_is_checked_before_a_chunk_passes_a_mib_and_when_it_stalls() {
    let i = install(true);
    let (content_type, mut body) = web::multipart(&[], &[("file", "a.csv", "x".repeat(3 * 1024 * 1024).as_bytes())]);
    let path = i.c.path().unwrap().to_string();
    Connection::open(&path).unwrap().execute("UPDATE oauth_access_tokens SET revoked_at = '2026-09-20 11:59:00' WHERE token = ?1", [API]).unwrap();
    // Admitted before the revocation: the request's own first check is the read's.
    i._clock.set(web::at("2026-09-20T11:58:00Z"));
    let clock = i._clock.clone();
    let one = Body::from_stream(futures_util::stream::once(async move { clock.set(web::at("2026-09-20T12:00:00Z")); Ok::<_, std::convert::Infallible>(Bytes::from(std::mem::take(&mut body))) }));
    let (sent, _) = send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, one).await;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (401, json!("token_revoked")), "{}", sent.body);
    let kept = import_api::COLLECTED_AT_DENIAL.load(Ordering::SeqCst);
    assert!(kept > 0 && kept <= 1024 * 1024, "{kept} bytes kept");
    i.c.execute("UPDATE oauth_access_tokens SET revoked_at = NULL", []).unwrap();
    let path = i.c.path().unwrap().to_string();
    let head = Bytes::from(format!("--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.csv\"\r\n\r\nx", web::MULTIPART_BOUNDARY));
    let stalled = Body::from_stream(futures_util::stream::once(async move {
        Connection::open(&path).unwrap().execute("UPDATE oauth_access_tokens SET revoked_at = '2026-09-20 11:59:00' WHERE token = ?1", [API]).unwrap();
        Ok::<_, std::convert::Infallible>(head)
    }).chain(futures_util::stream::pending()));
    let sent = tokio::time::timeout(std::time::Duration::from_secs(30), send(&i.app, "POST", "/api/v1/tracker/import", Some(API), Some(&content_type), None, stalled)).await.expect("the stalled read is stopped").0;
    assert_eq!((sent.status, sent.json()["error"]["code"].clone()), (401, json!("token_revoked")), "{}", sent.body);
}
