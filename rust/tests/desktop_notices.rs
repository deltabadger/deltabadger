//! PLAN-B6-3 §3 + r4 §3.1 (B6-3b, D-DESKTOP-NOTIFY): on the desktop the mail sender offers bot alerts to the launcher
//! instead of a mail server; the launcher lists them at `GET /desktop/notices` and acks them at
//! `POST /desktop/notices/ack`; an acked notice clears its marker, an unacked one is offered again on the retry schedule.
mod common;
use common::web::{app_in, Browser, Csrf, TestClock, SECRET};
use deltabadger::mail::outbox::{Notice, Outbox};
use deltabadger::web::{desktop, App};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;

const LAUNCHER: &str = "0123456789abcdef0123456789abcdef";
const NOW: &str = "2026-11-05T12:00:00Z";
const FRESH: &str = "2026-11-05T11:00:00.000Z";
const OLD: &str = "2026-10-27T11:00:00.000Z";

fn install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (file, schema) in [("production.sqlite3", include_str!("fixtures/settings_primary_schema.sql")), ("production_queue.sqlite3", include_str!("fixtures/settings_queue_schema.sql"))] {
        let c = rusqlite::Connection::open(dir.path().join(file)).unwrap();
        c.execute_batch(&format!("BEGIN IMMEDIATE;\n{schema}\nCOMMIT;")).unwrap();
    }
    dir
}

fn db(dir: &tempfile::TempDir) -> rusqlite::Connection { rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap() }

fn user(c: &rusqlite::Connection, email: &str, admin: bool) -> i64 {
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES (?1, 'x', 'Someone', ?2, '2026-01-01 00:00:00', '2026-01-01 00:00:00')", (email, admin)).unwrap();
    c.last_insert_rowid()
}

/// A bot on a Kraken account with `transient` as its markers.
fn bot(c: &rusqlite::Connection, user: i64, label: &str, settings: &str, transient: Value) -> i64 {
    c.execute("INSERT OR IGNORE INTO exchanges (id, name, type, created_at, updated_at) VALUES (1, 'Kraken', 'Exchanges::Kraken', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", []).unwrap();
    c.execute("INSERT INTO bots (user_id, exchange_id, type, label, settings, transient_data, created_at, updated_at) VALUES (?1, 1, 'Bots::DcaSingleAsset', ?2, ?3, ?4, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
              (user, label, settings, transient.to_string())).unwrap();
    c.last_insert_rowid()
}

fn markers(c: &rusqlite::Connection, bot: i64) -> Value {
    let raw: String = c.query_row("SELECT transient_data FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap();
    let all: Map<String, Value> = serde_json::from_str(&raw).unwrap();
    Value::Object(all.into_iter().filter(|(k, _)| k.ends_with("_mail_pending")).collect())
}

fn stopped(at: &str) -> Value { json!({ "rust_stopped_mail_pending": { "error": "EOrder:Insufficient funds", "stamped_at": at } }) }

struct Fixed;
impl deltabadger::engine::Clock for Fixed { fn now(&self) -> chrono::DateTime<chrono::Utc> { common::web::at(NOW) } }

/// The sender as `serve` runs it on the desktop, fast: looks every 20 ms, waits `ack_wait` for an ack, retries after 400 ms.
fn run_sender(dir: &tempfile::TempDir, outbox: Arc<Outbox>, account: i64, ack_wait: Duration) -> (tokio::sync::watch::Sender<bool>, tokio::task::JoinHandle<Result<(), String>>) {
    let opened = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, SECRET).unwrap());
    let mut s = deltabadger::mail::sender::Sender::new(opened.primary, cipher, &|_| None, Fixed).desktop(outbox, account);
    (s.poll, s.waits, s.ack_wait) = (Duration::from_millis(20), vec![Duration::from_millis(400)], ack_wait);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(s.run::<()>(stopped, None));
    (stop, run)
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ok() { return; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

#[tokio::test]
async fn an_acked_notice_clears_its_marker_and_is_not_offered_again() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let b = bot(&c, me, "Weekly BTC", "{}", stopped(FRESH));
    let outbox = Arc::new(Outbox::default());
    // A long ack wait and a long retry: only the ack can clear it this fast.
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    until("offered", || !outbox.list(20).is_empty()).await;
    let listed = outbox.list(20);
    assert_eq!(listed.len(), 1);
    let n = &listed[0];
    assert_eq!(n.id, format!("{b}:rust_stopped_mail_pending::{FRESH}"));
    assert_eq!(n.title, "Weekly BTC was stopped");
    assert!(n.body.starts_with("Kraken kept refusing its orders: "), "{}", n.body);
    assert!(!n.body.contains('<'), "no markup: {}", n.body);
    outbox.ack(std::slice::from_ref(&n.id));
    until("cleared", || markers(&c, b) == json!({})).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(outbox.list(20).is_empty());
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_unacked_notice_times_out_keeps_its_marker_and_is_offered_again_after_the_retry_wait() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let b = bot(&c, me, "Weekly BTC", "{}", stopped(FRESH));
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_millis(100));
    until("offered", || !outbox.list(20).is_empty()).await;
    until("timed out", || outbox.list(20).is_empty()).await;
    assert_ne!(markers(&c, b), json!({}), "no ack: the marker stays");
    // Not at once: after the retry wait (400 ms).
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(outbox.list(20).is_empty(), "offered again before its retry was due");
    until("offered again", || !outbox.list(20).is_empty()).await;
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn two_error_kinds_with_one_stamp_are_two_notices_and_an_ack_clears_only_its_own() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let errors = json!({ "rust_error_mail_pending": {
        "transient": { "error": "EService:Unavailable", "stamped_at": FRESH },
        "insufficient_funds": { "error": "EOrder:Insufficient funds", "stamped_at": FRESH } } });
    let b = bot(&c, me, "Weekly BTC", "{}", errors);
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    until("both offered", || outbox.list(20).len() == 2).await;
    let ids: Vec<String> = outbox.list(20).into_iter().map(|n| n.id).collect();
    assert!(ids.contains(&format!("{b}:rust_error_mail_pending:transient:{FRESH}")), "{ids:?}");
    let funds = format!("{b}:rust_error_mail_pending:insufficient_funds:{FRESH}");
    assert!(ids.contains(&funds), "{ids:?}");
    outbox.ack(std::slice::from_ref(&funds));
    until("one kind cleared", || markers(&c, b)["rust_error_mail_pending"].as_object().is_some_and(|m| m.len() == 1)).await;
    assert!(markers(&c, b)["rust_error_mail_pending"].get("transient").is_some());
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

/// The ack names the marker as it was offered: one raised again since (another stamp) is kept, and offered anew.
#[tokio::test]
async fn an_ack_for_a_marker_raised_again_since_clears_nothing() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let b = bot(&c, me, "Weekly BTC", "{}", stopped(FRESH));
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    until("offered", || !outbox.list(20).is_empty()).await;
    let first = outbox.list(20)[0].id.clone();
    const AGAIN: &str = "2026-11-05T11:30:00.000Z";
    c.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", (stopped(AGAIN).to_string(), b)).unwrap();
    // Codex branch r1: the old notice leaves the outbox once the sender has read every bot again, acked or not.
    until("the new one offered", || outbox.list(20).iter().any(|n| n.id.ends_with(AGAIN))).await;
    until("the old one gone", || outbox.list(20).iter().all(|n| n.id != first)).await;
    outbox.ack(&[first]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(markers(&c, b)["rust_stopped_mail_pending"]["stamped_at"], AGAIN);
    assert_eq!(outbox.list(20).len(), 1);
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn seven_day_old_markers_expire_unoffered_for_every_user_and_another_users_fresh_one_is_never_offered() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let other = user(&c, "someone@example.com", false);
    let (mine_old, theirs_old, theirs_fresh) =
        (bot(&c, me, "Mine", "{}", stopped(OLD)), bot(&c, other, "Theirs", "{}", stopped(OLD)), bot(&c, other, "Theirs too", "{}", stopped(FRESH)));
    // Codex branch r1: another user's markers wait out their week even when no notice could be made of them.
    let funds = json!({ "rust_funds_mail_pending": { "quote_asset": 999_999, "stamped_at": FRESH } });
    let theirs_unsendable = bot(&c, other, "No quote", "{}", funds);
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    until("both old ones expired", || markers(&c, mine_old) == json!({}) && markers(&c, theirs_old) == json!({})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(outbox.list(20).is_empty(), "{:?}", outbox.list(20));
    for kept in [theirs_fresh, theirs_unsendable] { assert_ne!(markers(&c, kept), json!({}), "bot {kept}: kept for its seven days"); }
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

/// Codex branch r1: a deleted bot's notice leaves the outbox, and it never holds a place there.
#[tokio::test]
async fn a_deleted_bots_notice_leaves_the_outbox() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let b = bot(&c, me, "Weekly BTC", "{}", stopped(FRESH));
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    until("offered", || !outbox.list(20).is_empty()).await;
    c.execute("DELETE FROM bots WHERE id = ?1", [b]).unwrap();
    until("gone", || outbox.list(20).is_empty()).await;
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_notice_that_cannot_be_rendered_is_not_offered_and_its_marker_stays() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    let b = bot(&c, me, "Broken", "not json", stopped(FRESH));
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(outbox.list(20).is_empty());
    assert_ne!(markers(&c, b), json!({}));
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

/// r4 §3.1: the sender never waits for an ack in its loop. With a minute's ack wait, every bot's notice is offered at
/// once, up to the outbox's 50; the 51st is offered once room is made.
#[tokio::test]
async fn notices_are_offered_together_up_to_fifty_and_the_rest_wait_for_room() {
    let dir = install();
    let c = db(&dir);
    let me = user(&c, "desktop@deltabadger.invalid", true);
    for i in 0..51 { bot(&c, me, &format!("Bot {i}"), "{}", stopped(FRESH)); }
    let outbox = Arc::new(Outbox::default());
    let (stop, run) = run_sender(&dir, outbox.clone(), me, Duration::from_secs(60));
    until("fifty offered", || outbox.list(usize::MAX).len() == 50).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let held = outbox.list(usize::MAX);
    assert_eq!(held.len(), 50);
    outbox.ack(&[held[0].id.clone()]);
    until("the last one offered", || outbox.list(usize::MAX).iter().any(|n| n.title == "Bot 50 was stopped")).await;
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
}

// ---- the launcher's routes ----

fn desktop_env(launcher: Option<&str>) -> Map<String, Value> {
    let mut env = Map::new();
    env.insert("DELTABADGER_PLATFORM".into(), json!("desktop"));
    if let Some(l) = launcher { env.insert("DELTABADGER_DESKTOP_LAUNCHER".into(), json!(l)); }
    env
}

async fn desktop_app(dir: &tempfile::TempDir) -> App {
    let app = app_in(dir.path(), SECRET, None, &desktop_env(Some(LAUNCHER)), TestClock::at(NOW));
    desktop::prepare(&app).await.unwrap();
    app
}

fn notice(i: usize) -> Notice { Notice { id: format!("{i}:rust_limit_mail_pending::{FRESH}"), bot_id: i as i64, title: format!("title {i}"), body: format!("body {i}") } }

async fn list(app: &App, header: Option<&str>) -> common::web::Answer {
    let headers: Vec<(&str, &str)> = header.map(|h| vec![(desktop::LAUNCHER_HEADER, h)]).unwrap_or_default();
    Browser::default().send_body(app, "GET", "/desktop/notices", None, Csrf::None, &headers).await
}

async fn ack(app: &App, header: Option<&str>, ids: Value) -> common::web::Answer {
    let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
    if let Some(h) = header { headers.push((desktop::LAUNCHER_HEADER, h)); }
    Browser::default().send_body(app, "POST", "/desktop/notices/ack", Some(json!({ "ids": ids }).to_string()), Csrf::None, &headers).await
}

fn ids(answer: &common::web::Answer) -> Vec<String> {
    let body: Value = serde_json::from_str(&answer.body).unwrap_or_else(|_| panic!("not JSON: {}", answer.body));
    body["notices"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn the_launcher_lists_twenty_oldest_first_and_acks_with_its_header_only() {
    let dir = install();
    let app = desktop_app(&dir).await;
    let outbox = app.desktop_outbox().expect("the desktop has an outbox");
    let later = tokio::time::Instant::now() + Duration::from_secs(60);
    for i in 0..25 { assert!(outbox.offer(notice(i), later)); }
    for header in [None, Some("wrong")] {
        let answer = list(&app, header).await;
        assert_eq!(answer.status, 401, "{header:?}");
        assert!(answer.header("set-cookie").is_none());
        assert_eq!(ack(&app, header, json!([notice(0).id])).await.status, 401);
    }
    assert_eq!(outbox.list(usize::MAX).len(), 25, "nothing acked without the header");
    let answer = list(&app, Some(LAUNCHER)).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert!(answer.header("set-cookie").is_none());
    assert_eq!(ids(&answer), (0..20).map(|i| notice(i).id).collect::<Vec<_>>());
    let first: Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(first["notices"][0], json!({ "id": notice(0).id, "bot_id": 0, "title": "title 0", "body": "body 0" }));
    // Unknown and repeated ids are ignored.
    let acked = ack(&app, Some(LAUNCHER), json!([notice(0).id, notice(0).id, "nobody's", notice(1).id])).await;
    assert_eq!(acked.status, 204, "{}", acked.body);
    assert_eq!(ack(&app, Some(LAUNCHER), json!([notice(0).id])).await.status, 204);
    assert_eq!(ids(&list(&app, Some(LAUNCHER)).await)[0], notice(2).id);
    assert_eq!(ack(&app, Some(LAUNCHER), json!("not a list")).await.status, 400);
}

#[tokio::test]
async fn without_the_launcher_secret_the_notices_are_refused() {
    let dir = install();
    let app = app_in(dir.path(), SECRET, None, &desktop_env(None), TestClock::at(NOW));
    desktop::prepare(&app).await.unwrap();
    assert_eq!(list(&app, Some(LAUNCHER)).await.status, 401);
    assert_eq!(ack(&app, Some(LAUNCHER), json!([])).await.status, 401);
}

#[tokio::test]
async fn off_the_desktop_the_notice_routes_are_unknown_paths() {
    let dir = install();
    user(&db(&dir), "owner@example.com", true);
    let app = app_in(dir.path(), SECRET, None, &Map::new(), TestClock::at(NOW));
    assert!(app.desktop_outbox().is_none());
    let unknown = Browser::default().send_body(&app, "GET", "/nothing/here", None, Csrf::None, &[]).await.status;
    assert_eq!(list(&app, Some(LAUNCHER)).await.status, unknown);
    let unknown_post = Browser::default().send_body(&app, "POST", "/nothing/here", None, Csrf::None, &[]).await.status;
    assert_eq!(ack(&app, Some(LAUNCHER), json!([])).await.status, unknown_post);
}

#[tokio::test]
async fn the_desktop_settings_say_alerts_are_system_notifications() {
    let dir = install();
    let app = desktop_app(&dir).await;
    let launcher = [(desktop::LAUNCHER_HEADER, LAUNCHER)];
    let t = Browser::default().send_body(&app, "POST", "/desktop/ticket", None, Csrf::None, &launcher).await;
    let ticket = serde_json::from_str::<Value>(&t.body).unwrap()["ticket"].as_str().unwrap().to_string();
    let mut b = Browser::default();
    b.get(&app, &format!("/desktop/sign_in?ticket={ticket}")).await;
    let page = b.get(&app, "/settings/account").await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page.body.contains("Deltabadger shows bot alerts as system notifications."), "{}", page.body);
}
