//! PLAN-A12 A12-1: the tax report's request rows, run, publish and recovery (D-TAX-QUEUE, D-TAX-RECONCILE,
//! D-TAX-NOT-PORTED-GATE), which no Rails recording can show: races, crash points and the per-request panels. The
//! surfaces' answers are held to Rails by the page and MCP grids (api_tax_*, d5_tax_*, tax_*).
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::engine::FixedClock;
use deltabadger::jobs::{notifications::Notifications, Cx, Db, Outcome, Wakers};
use deltabadger::tax::job as tax;
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const T0: &str = "2026-09-10T12:00:00Z";

struct Install { d: tempfile::TempDir, user: i64 }

/// An Alpaca owner with a buy and a sale of bitcoin in 2025, quoted in dollars, and this build's migrations applied.
fn install() -> Install {
    let (d, o, s) = common::install_alpaca();
    let c = &o.primary;
    deltabadger::store::test_support::migrate(c).unwrap();
    c.execute("UPDATE users SET encrypted_password=?1, confirmed_at='2026-01-01', wash_sale_enabled=0, time_zone='UTC'",
              [deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    for (kind, amount, quote, at) in [(0, "1", "20000", "2025-01-05 10:00:00"), (1, "0.5", "15000", "2025-06-05 10:00:00")] {
        c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
                   tx_id, transacted_at, raw_data, manual_values, transfer_link_rejected, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, ?4, 'BTC', ?5, 'USD', ?6, ?7, ?8, '{}', '{}', 0, ?8, ?8)",
                  rusqlite::params![s.user_id, s.api_key_id, s.exchange_id, kind, amount, quote, format!("t-{kind}"), at]).unwrap();
    }
    Install { d, user: s.user_id }
}

fn db_path(i: &Install) -> PathBuf { i.d.path().join("production.sqlite3") }
fn open(i: &Install) -> Connection { Connection::open(db_path(i)).unwrap() }
fn tmp(i: &Install) -> PathBuf { i.d.path().join("tmp") }

/// The web app on the install (its tmp is `<install>/tmp`), with market data from data-api.
fn app(i: &Install) -> deltabadger::web::App {
    let extra: Map<String, Value> = Map::from_iter([("MARKET_DATA_URL".to_string(), json!("http://127.0.0.1:9"))]);
    let app = web::app_in(i.d.path(), "engine-test-secret", None, &extra, TestClock::at(T0));
    app.attach_engine(Default::default());
    app.attach_jobs(Wakers::default()).unwrap();
    app
}

fn args(v: Value) -> Map<String, Value> { v.as_object().unwrap().clone() }

type Sent = Arc<Mutex<Vec<(String, String)>>>;

/// The job's database handle on its own connection, its broadcasts captured.
fn job_db(i: &Install) -> (Db, Sent) {
    let sent: Sent = Arc::default();
    let into = sent.clone();
    let db = Db::new(open(i), common::seed::cipher()).with_notifications(Notifications::new(move |stream, payload| { into.lock().unwrap().push((stream.into(), payload.into())); Ok(()) }));
    (db, sent)
}

async fn run(i: &Install, db: &Db) -> Outcome {
    let job = tax::job::<ScriptedTransport>(std::rc::Rc::new(None), None, tmp(i));
    let clock = FixedClock(web::at(T0));
    job.run(Cx { db: db.clone(), clock: &clock, wakers: Wakers::default() }, vec![]).await
}

fn rows(c: &Connection) -> Vec<(String, String)> {
    let mut s = c.prepare("SELECT request, state FROM rust_tax_reports ORDER BY id").unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("tax_reports")).map(|d| d.flatten().map(|e| e.file_name().into_string().unwrap()).collect()).unwrap_or_default();
    names.sort();
    names
}

fn queued(c: &Connection, user: i64, request: &str, country: &str, state: &str) {
    c.execute("INSERT INTO rust_tax_reports (request, user_id, scope, country, year, stablecoin_as_fiat, locale, state, created_at) \
               VALUES (?1, ?2, 'crypto', ?3, 2025, 0, 'en', ?4, '2026-09-10 12:00:00')", rusqlite::params![request, user, country, state]).unwrap();
}

fn write(path: &Path, body: &str) { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(path, body).unwrap(); }

const R1: &str = "0123456789abcdef0123456789abcdef";
const R2: &str = "fedcba9876543210fedcba9876543210";

#[tokio::test]
async fn an_accepted_request_runs_once_with_progress_then_its_ready_panel() {
    let i = install();
    let app = app(&i);
    let res = deltabadger::tax::api::create(&open(&i), &app, i.user, &args(json!({"country": "us", "year": "2025"})), "en").unwrap();
    assert_eq!(res.code, None, "{}", res.message);
    assert_eq!(res.data, json!({"country": "US", "year": 2025, "name": "United States", "ready": false, "state": "generating"}));
    let settings: String = open(&i).query_row("SELECT tracker_settings FROM users WHERE id=?1", [i.user], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&settings).unwrap()["pending_report"], json!({"country": "US", "year": 2025, "report_scope": "crypto"}));
    assert_eq!(app.job_wakers().unwrap().pending(), vec![(tax::JOB.to_string(), None)]);
    let request = rows(&open(&i))[0].0.clone();

    let (db, sent) = job_db(&i);
    assert_eq!(run(&i, &db).await, Outcome::Done);
    assert_eq!(rows(&open(&i)), vec![(request.clone(), "ready".into())]);
    assert_eq!(files(&tmp(&i)), vec![format!("{}_US_2025_crypto.csv", i.user)]);
    let csv = std::fs::read_to_string(tax::report_path(&tmp(&i), i.user, "US", 2025, "crypto")).unwrap();
    assert!(csv.starts_with("Date,") && csv.contains("BTC"), "{csv}");
    let sent = sent.lock().unwrap().clone();
    assert!(sent.iter().all(|(stream, html)| stream == &format!("user_{}:tax_report", i.user) && html.contains(&format!("target=\"tax-report-{request}\""))), "{sent:?}");
    let percents: Vec<u32> = sent.iter().filter_map(|(_, h)| h.split("<span>").nth(1)).map(|s| s.split('%').next().unwrap().rsplit(' ').next().unwrap().parse().unwrap()).collect();
    assert!(!percents.is_empty() && percents.windows(2).all(|w| w[0] < w[1]) && *percents.last().unwrap() == 100, "{percents:?}");
    assert!(sent.last().unwrap().1.contains("auto-download") && sent.last().unwrap().1.contains("/tracker/download_tax_report?country=US&amp;report_scope=crypto&amp;year=2025"));
    // A request runs once: nothing is left to take.
    assert_eq!(run(&i, &db).await, Outcome::NothingNew);

    // Ready and not forced: 409; forced: the next request replaces the retained one, the report stepped aside meanwhile.
    let again = deltabadger::tax::api::create(&open(&i), &app, i.user, &args(json!({"country": "US", "year": 2025})), "en").unwrap();
    assert_eq!(again.code, Some("report_ready"));
    let forced = deltabadger::tax::api::create(&open(&i), &app, i.user, &args(json!({"country": "US", "year": 2025, "force": true})), "en").unwrap();
    assert_eq!(forced.code, None);
    assert_eq!(rows(&open(&i)).iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), vec!["queued"], "the earlier outcome is gone with the new request");
    assert_eq!(files(&tmp(&i)), Vec::<String>::new(), "the stale report is removed once the request committed");
}

#[test]
fn two_creates_racing_take_one_slot() {
    let i = install();
    let app = app(&i);
    let go = Arc::new(std::sync::Barrier::new(2));
    let answers: Vec<Option<&'static str>> = (0..2).map(|_| {
        let (app, go, path, user) = (app.clone(), go.clone(), db_path(&i), i.user);
        std::thread::spawn(move || {
            let c = Connection::open(path).unwrap();
            c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            go.wait();
            deltabadger::tax::api::create(&c, &app, user, &args(json!({"country": "US", "year": 2025})), "en").unwrap().code
        })
    }).collect::<Vec<_>>().into_iter().map(|t| t.join().unwrap()).collect();
    let mut answers = answers;
    answers.sort();
    assert_eq!(answers, vec![None, Some("report_generating")]);
    assert_eq!(rows(&open(&i)).len(), 1);
}

#[test]
fn status_and_download_read_the_files_in_rails_order() {
    let i = install();
    let app = app(&i);
    let status = |c: &Connection| deltabadger::tax::api::status(c, &app, i.user, &args(json!({"country": "US", "year": 2025}))).unwrap().data;
    let c = open(&i);
    assert_eq!(status(&c)["state"], "none");
    queued(&c, i.user, R1, "US", "queued");
    assert_eq!(status(&c)["state"], "generating");
    let refusal = tax::refusal_path(&tmp(&i), i.user, "US", 2025, "crypto");
    write(&refusal, r#"{"reason":"tokenized_unsupported","symbols":["TSLAx"]}"#);
    assert_eq!(status(&c), json!({"country": "US", "year": 2025, "ready": false, "state": "refused", "reason": "tokenized_unsupported", "symbols": ["TSLAx"]}));
    let download = || deltabadger::tax::api::download(&app, i.user, &args(json!({"country": "US", "year": 2025}))).unwrap();
    assert_eq!(download().err().and_then(|r| r.code), Some("report_refused"));
    write(&tax::report_path(&tmp(&i), i.user, "US", 2025, "crypto"), "a,b\n");
    // File first, then refusal (Rails' order), and the API never deletes.
    assert_eq!(status(&c)["state"], "ready");
    assert_eq!(download().ok(), Some(("a,b\n".to_string(), "deltabadger-tax-report-us-2025.csv".to_string())));
    assert_eq!(download().ok().map(|d| d.0).as_deref(), Some("a,b\n"));
}

#[test]
fn the_not_ported_gate_refuses_before_anything_is_queued() {
    let i = install();
    let app = app(&i);
    let c = open(&i);
    let res = deltabadger::tax::api::create(&c, &app, i.user, &args(json!({"country": "FR", "year": 2025})), "en").unwrap();
    assert_eq!((res.status, res.code), (deltabadger::web::bot_api::Domain::NotPorted, Some("not_ported")));
    let text = deltabadger::tax::api::mcp(&c, &app, i.user, "generate_tax_report", &json!({"country": "FR", "year": 2025.0})).unwrap();
    assert_eq!(text["isError"], true, "{text}");
    // Validation still answers first, as Rails.
    assert_eq!(deltabadger::tax::api::create(&c, &app, i.user, &args(json!({"country": "ZZ", "year": 2025})), "en").unwrap().code, Some("unknown_country"));
    // An install on CoinGecko prices: refused up front (A17 lifts it).
    let plain = web::app_in(i.d.path(), "engine-test-secret", None, &Map::new(), TestClock::at(T0));
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('market_data_provider', 'coingecko', '2026-01-01', '2026-01-01')", []).unwrap();
    assert_eq!(deltabadger::tax::api::create(&c, &plain, i.user, &args(json!({"country": "US", "year": 2025})), "en").unwrap().code, Some("not_ported"));
    assert!(rows(&c).is_empty());
}

#[tokio::test]
async fn a_failed_run_keeps_its_failed_outcome_and_tells_its_panel() {
    let i = install();
    // A row this build cannot compute (nothing queues one; the run refuses it all the same).
    queued(&open(&i), i.user, R1, "FR", "queued");
    let (db, sent) = job_db(&i);
    assert_eq!(run(&i, &db).await, Outcome::Done);
    assert_eq!(rows(&open(&i)), vec![(R1.into(), "failed".into())]);
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.starts_with(&format!("<turbo-stream action=\"replace\" target=\"tax-report-{R1}\">")) && sent[0].1.contains("salert--danger"), "{sent:?}");
    assert_eq!(files(&tmp(&i)), Vec::<String>::new());
}

#[tokio::test]
async fn recovery_finishes_or_undoes_what_a_crash_left_and_is_idempotent() {
    let i = install();
    let c = open(&i);
    let t = tmp(&i);
    let user = i.user;
    let report = |country: &str| tax::report_path(&t, user, country, 2025, "crypto");
    // Crash mid-run, after the tmp write: the row ends failed, its output goes.
    queued(&c, user, R1, "US", "running");
    write(&PathBuf::from(format!("{}.tmp.{R1}", report("US").display())), "partial");
    // Crash after the commit, before the rename: the rename is finished, the stale refusal beside it removed.
    let r3 = "00000000000000000000000000000003";
    c.execute("INSERT INTO rust_tax_reports (request, user_id, scope, country, year, stablecoin_as_fiat, locale, state, created_at) \
               VALUES (?1, 99, 'crypto', 'GB', 2025, 0, 'en', 'ready', '2026-09-10 12:00:00')", [r3]).unwrap();
    let gb = tax::report_path(&t, 99, "GB", 2025, "crypto");
    write(&PathBuf::from(format!("{}.tmp.{r3}", gb.display())), "gb report");
    write(&tax::refusal_path(&t, 99, "GB", 2025, "crypto"), "{}");
    // A create that died before committing: its stale file comes back where nothing replaced it, else goes.
    let r4 = "00000000000000000000000000000004";
    write(&PathBuf::from(format!("{}.stale.{r4}", report("AT").display())), "old AT");
    write(&PathBuf::from(format!("{}.stale.{r4}", report("FR").display())), "old FR");
    write(&report("FR"), "new FR");
    // A create that committed and died before removing its stale file: removed.
    let r5 = "00000000000000000000000000000005";
    c.execute("INSERT INTO rust_tax_reports (request, user_id, scope, country, year, stablecoin_as_fiat, locale, state, created_at) \
               VALUES (?1, 98, 'crypto', 'IT', 2025, 0, 'en', 'queued', '2026-09-10 12:00:00')", [r5]).unwrap();
    write(&PathBuf::from(format!("{}.stale.{r5}", tax::report_path(&t, 98, "IT", 2025, "crypto").display())), "old IT");

    let (db, sent) = job_db(&i);
    for _ in 0..2 {
        tax::recover(&db, &t).await.unwrap();
        let mut want = vec![format!("{user}_AT_2025_crypto.csv"), format!("{user}_FR_2025_crypto.csv"), "99_GB_2025_crypto.csv".to_string()];
        want.sort();
        assert_eq!(files(&t), want);
    }
    assert_eq!(std::fs::read_to_string(report("AT")).unwrap(), "old AT");
    assert_eq!(std::fs::read_to_string(report("FR")).unwrap(), "new FR");
    assert_eq!(std::fs::read_to_string(&gb).unwrap(), "gb report");
    assert_eq!(c.query_row("SELECT state FROM rust_tax_reports WHERE request=?1", [R1], |r| r.get::<_, String>(0)).unwrap(), "failed");
    assert_eq!(sent.lock().unwrap().len(), 1, "the dead run's panel is told once, however often recovery runs");
}

#[tokio::test]
async fn the_deadline_path_fails_the_running_request() {
    let i = install();
    queued(&open(&i), i.user, R1, "US", "running");
    let (db, sent) = job_db(&i);
    db.deadline_completion(None, tax::JOB, None).await;
    assert_eq!(rows(&open(&i)), vec![(R1.into(), "failed".into())]);
    assert!(sent.lock().unwrap()[0].1.contains("salert--danger"));
}

async fn signed_in(i: &Install) -> (deltabadger::web::App, Browser) {
    let app = app(i);
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(browser.get(&app, "/settings/account").await.status, 200);
    (app, browser)
}

const TURBO: [(&str, &str); 1] = [("accept", "text/vnd.turbo-stream.html, text/html, application/xhtml+xml")];

#[tokio::test]
async fn the_web_request_answers_its_own_panel_and_restores_a_refusal_it_could_not_replace() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let refusal = tax::refusal_path(&tmp(&i), i.user, "US", 2025, "crypto");
    write(&refusal, r#"{"reason":"tokenized_unsupported","symbols":["TSLAx"]}"#);
    // Another request holds the slot: not accepted, the refusal comes back and is the answer (Rails' reconcile).
    queued(&open(&i), i.user, R2, "US", "running");
    let busy = browser.send(&app, "GET", "/tracker/tax_report?country=US&year=2025", None, Csrf::None, &TURBO).await;
    assert_eq!(busy.status, 200);
    assert!(busy.body.contains("TSLAx") && busy.body.contains("salert--warning"), "{}", busy.body);
    assert!(refusal.exists());
    assert_eq!(files(&tmp(&i)), vec![format!("{}_US_2025_crypto.csv.refused.json", i.user)]);
    // Without a refusal, the holder's progress panel.
    std::fs::remove_file(&refusal).unwrap();
    let held = browser.send(&app, "GET", "/tracker/tax_report?country=US&year=2025", None, Csrf::None, &TURBO).await;
    assert!(held.body.contains(&format!("id=\"tax-report-{R2}\"")), "{}", held.body);
    // The slot free: accepted, the refusal stepped aside and removed, the new request's progress panel.
    open(&i).execute("UPDATE rust_tax_reports SET state='failed'", []).unwrap();
    write(&refusal, r#"{"reason":"tokenized_unsupported","symbols":["TSLAx"]}"#);
    let accepted = browser.send(&app, "GET", "/tracker/tax_report?country=US&year=2025&stablecoin_as_fiat=true", None, Csrf::None, &TURBO).await;
    let rows = rows(&open(&i));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "queued");
    assert!(accepted.body.contains(&format!("id=\"tax-report-{}\"", rows[0].0)) && accepted.body.contains("broadcast--on-connect"), "{}", accepted.body);
    assert!(!refusal.exists());
    assert!(open(&i).query_row("SELECT stablecoin_as_fiat FROM rust_tax_reports", [], |r| r.get::<_, bool>(0)).unwrap());
    // The gate: a jurisdiction not ported is the refused flash, nothing queued.
    let fr = browser.send(&app, "GET", "/tracker/tax_report?country=FR&year=2025", None, Csrf::None, &TURBO).await;
    assert_eq!(fr.status, 501);
    assert_eq!(rows.len(), 1);

    // The reconcile endpoint: only the owner's request; a terminal one is broadcast again.
    let state = |request: &str| json!({ "request": request }).to_string();
    let page = browser.get(&app, "/settings/account").await;
    assert_eq!(page.status, 200);
    let own = browser.send_body(&app, "POST", "/broadcasts/tax_report_state", Some(state(&rows[0].0)), Csrf::Header, &[("content-type", "application/json")]).await;
    assert_eq!(own.status, 200);
    let gone = browser.send_body(&app, "POST", "/broadcasts/tax_report_state", Some(state(R2)), Csrf::Header, &[("content-type", "application/json")]).await;
    assert_eq!(gone.status, 404, "A's request was replaced by B's: its late state request is ignored");
}

#[tokio::test]
async fn the_web_download_sends_once_and_a_reload_shows_a_failed_request() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let path = tax::report_path(&tmp(&i), i.user, "US", 2025, "crypto");
    write(&path, "a,b\n");
    let sent = browser.get(&app, "/tracker/download_tax_report?country=us&year=2025").await;
    assert_eq!((sent.status, sent.body.as_str(), sent.header("content-type")), (200, "a,b\n", Some("text/csv; charset=utf-8")));
    assert!(sent.header("content-disposition").unwrap().contains("deltabadger-tax-report-us-2025.csv"));
    assert!(!path.exists(), "the web download deletes the file");
    let expired = browser.get(&app, "/tracker/download_tax_report?country=us&year=2025").await;
    assert_eq!((expired.status, expired.header("location")), (302, Some("/tracker")));
    // A reload after a failed run: no file marks the failure, the retained row does.
    open(&i).execute("UPDATE users SET tracker_settings='{\"pending_report\":{\"country\":\"US\",\"year\":2025,\"report_scope\":\"crypto\"}}'", []).unwrap();
    queued(&open(&i), i.user, R1, "US", "failed");
    let page = browser.get(&app, "/tracker").await;
    assert!(page.body.contains("salert--danger"), "{}", page.body);
}

#[tokio::test]
async fn a_report_that_cannot_be_written_fails_its_request() {
    let i = install();
    queued(&open(&i), i.user, R1, "US", "queued");
    // `tmp/tax_reports` is a file: the report's directory cannot be made.
    write(&tmp(&i).join("tax_reports"), "not a directory");
    let (db, sent) = job_db(&i);
    assert_eq!(run(&i, &db).await, Outcome::Done);
    assert_eq!(rows(&open(&i)), vec![(R1.into(), "failed".into())]);
    assert!(sent.lock().unwrap().last().unwrap().1.contains("salert--danger"));
}

/// D-TAX-PL-DECLARED-CARRY: the stored carry, by year.
fn carry(c: &Connection, user: i64) -> Vec<(i64, String)> {
    let mut s = c.prepare("SELECT year, costs FROM rust_tax_pl_carry WHERE user_id = ?1 ORDER BY year").unwrap();
    s.query_map([user], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
fn free_slot(c: &Connection) { c.execute("UPDATE rust_tax_reports SET state = 'failed' WHERE state IN ('queued', 'running')", []).unwrap(); }
fn pending(c: &Connection, user: i64) -> Option<String> { c.query_row("SELECT tracker_settings FROM users WHERE id = ?1", [user], |r| r.get(0)).unwrap() }
/// A write to the carry table aborts (the fault after the enqueue).
fn break_carry(c: &Connection) {
    c.execute_batch("CREATE TRIGGER no_insert BEFORE INSERT ON rust_tax_pl_carry BEGIN SELECT RAISE(ABORT, 'carry'); END; \
                     CREATE TRIGGER no_update BEFORE UPDATE ON rust_tax_pl_carry BEGIN SELECT RAISE(ABORT, 'carry'); END; \
                     CREATE TRIGGER no_delete BEFORE DELETE ON rust_tax_pl_carry BEGIN SELECT RAISE(ABORT, 'carry'); END;").unwrap();
}

#[test]
fn the_api_saves_a_declared_pl_carry_only_with_the_request_it_queues() {
    let i = install();
    let app = app(&i);
    let c = open(&i);
    let create = |v: Value| deltabadger::tax::api::create(&c, &app, i.user, &args(v), "en").unwrap().code;
    assert_eq!(create(json!({"country": "PL", "year": 2025, "pl_carried_costs": "6014"})), None);
    assert_eq!(carry(&c, i.user), vec![(2025, "6014.0".to_string())]);
    // The slot is held: refused, nothing saved.
    assert_eq!(create(json!({"country": "PL", "year": 2025, "pl_carried_costs": "1"})), Some("report_generating"));
    assert_eq!(carry(&c, i.user), vec![(2025, "6014.0".to_string())]);
    free_slot(&c);
    // Absent keeps it; a JSON number with cents; null and "" clear.
    assert_eq!(create(json!({"country": "PL", "year": 2025})), None);
    assert_eq!(carry(&c, i.user), vec![(2025, "6014.0".to_string())]);
    free_slot(&c);
    assert_eq!(create(json!({"country": "PL", "year": 2024, "pl_carried_costs": 6014.5})), None);
    assert_eq!(carry(&c, i.user), vec![(2024, "6014.5".to_string()), (2025, "6014.0".to_string())]);
    free_slot(&c);
    assert_eq!(create(json!({"country": "PL", "year": 2024, "pl_carried_costs": null})), None);
    free_slot(&c);
    assert_eq!(create(json!({"country": "PL", "year": 2025, "pl_carried_costs": ""})), None);
    assert!(carry(&c, i.user).is_empty());
    free_slot(&c);
    // Refused before anything: another country, an amount it does not read.
    let queued_before = rows(&c).len();
    assert_eq!(create(json!({"country": "DE", "year": 2025, "pl_carried_costs": "1"})), Some("carried_costs_pl_only"));
    assert_eq!(create(json!({"country": "DE", "year": 2025, "pl_carried_costs": null})), Some("carried_costs_pl_only"));
    for bad in [json!("6014,50"), json!("-1"), json!("1e3"), json!("6014.501"), json!("6014."), json!(".5"), json!(true), json!([]), json!(1e21), json!("12345678901234")] {
        assert_eq!(create(json!({"country": "PL", "year": 2025, "pl_carried_costs": bad})), Some("invalid_carried_costs"), "{bad}");
    }
    assert_eq!(rows(&c).len(), queued_before);
    // A report already there and not forced: refused, nothing saved.
    let report = tax::report_path(&tmp(&i), i.user, "PL", 2025, "crypto");
    write(&report, "a,b\n");
    assert_eq!(create(json!({"country": "PL", "year": 2025, "pl_carried_costs": "5"})), Some("report_ready"));
    assert!(carry(&c, i.user).is_empty());
    // The write fails after the enqueue (forced replacement): the old declaration, the queue, pending_report and the report
    // file are as before.
    c.execute("INSERT INTO rust_tax_pl_carry (user_id, year, costs, updated_at) VALUES (?1, 2025, '100.0', '2026-01-01')", [i.user]).unwrap();
    break_carry(&c);
    let (before_rows, before_pending) = (rows(&c), pending(&c, i.user));
    assert!(deltabadger::tax::api::create(&c, &app, i.user, &args(json!({"country": "PL", "year": 2025, "pl_carried_costs": "200", "force": true})), "en").is_err());
    assert_eq!(carry(&c, i.user), vec![(2025, "100.0".to_string())]);
    assert_eq!((rows(&c), pending(&c, i.user)), (before_rows, before_pending));
    assert_eq!(std::fs::read_to_string(&report).unwrap(), "a,b\n");
    // MCP is the same call.
    c.execute_batch("DROP TRIGGER no_insert; DROP TRIGGER no_update; DROP TRIGGER no_delete;").unwrap();
    std::fs::remove_file(&report).unwrap();
    let text = deltabadger::tax::api::mcp(&c, &app, i.user, "generate_tax_report", &json!({"country": "PL", "year": 2025.0, "pl_carried_costs": "7000.25"})).unwrap();
    assert_ne!(text["isError"], true, "{text}");
    assert_eq!(carry(&c, i.user), vec![(2025, "7000.25".to_string())]);
}

#[tokio::test]
async fn the_pl_form_generates_with_its_carry_and_keeps_the_modal_on_a_refused_amount() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let c = open(&i);
    let frame = [("turbo-frame", "modal")];
    let modal = browser.send(&app, "GET", "/tracker/export_modal", None, Csrf::None, &frame).await;
    assert_eq!(modal.status, 200);
    for y in 2021..=2026 { assert!(modal.body.contains(&format!("<form id=\"pl-carry-{y}\" class=\"pl-carry\" data-action=\"turbo:submit-end-&gt;modal--base#submitEnd\" action=\"/tracker/tax_report\"")), "{y}"); }
    assert!(!modal.body.contains("pl-carry-2020"));
    assert!(modal.body.contains("<style>.pl-carry{display:none}"));
    assert!(modal.body.contains("Unused costs from your PIT-38 for 2024 (column f), PLN"));
    // The URL's locale is the form's.
    let pl = browser.send(&app, "GET", "/pl/tracker/export_modal", None, Csrf::None, &frame).await;
    assert!(pl.body.contains("action=\"/pl/tracker/tax_report\"") && pl.body.contains("Nieodliczone koszty z Twojego PIT-38 za 2024 r."), "{}", pl.body);

    let post = |amount: &'static str| [("year", "2025"), ("pl_carried_costs", amount)];
    let ok = browser.send(&app, "POST", "/tracker/tax_report", Some(&post("6014")), Csrf::Header, &TURBO).await;
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert!(ok.body.contains("broadcast--on-connect"), "{}", ok.body);
    assert_eq!(carry(&c, i.user), vec![(2025, "6014.0".to_string())]);
    let r: (String, i64) = c.query_row("SELECT country, year FROM rust_tax_reports WHERE state = 'queued'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(r, ("PL".to_string(), 2025));
    // The slot held: the holder's panel, nothing saved.
    let held = browser.send(&app, "POST", "/tracker/tax_report", Some(&post("1")), Csrf::Header, &TURBO).await;
    assert_eq!(held.status, 200);
    assert_eq!(carry(&c, i.user), vec![(2025, "6014.0".to_string())]);
    free_slot(&c);
    // A refused amount: 422, the form again with the error and the amount as typed, nothing saved or queued.
    let queued_before = rows(&c).len();
    let bad = browser.send(&app, "POST", "/tracker/tax_report", Some(&post("6014,50")), Csrf::Header, &TURBO).await;
    assert_eq!(bad.status, 422);
    assert!(bad.body.starts_with("<turbo-stream action=\"replace\" target=\"pl-carry-2025\">") && bad.body.contains("value=\"6014,50\"")
            && bad.body.contains("Enter an amount in PLN") && bad.body.contains("modal--base#submitEnd"), "{}", bad.body);
    assert_eq!((rows(&c).len(), carry(&c, i.user)), (queued_before, vec![(2025, "6014.0".to_string())]));
    // The modal shows what is saved.
    let modal = browser.send(&app, "GET", "/tracker/export_modal", None, Csrf::None, &frame).await;
    assert!(modal.body.contains("value=\"6014.0\""), "{}", modal.body);
    // Empty clears.
    assert_eq!(browser.send(&app, "POST", "/tracker/tax_report", Some(&post("")), Csrf::Header, &TURBO).await.status, 200);
    assert!(carry(&c, i.user).is_empty());
    free_slot(&c);
    // The write fails after the enqueue: nothing queued, the old declaration kept.
    c.execute("INSERT INTO rust_tax_pl_carry (user_id, year, costs, updated_at) VALUES (?1, 2025, '100.0', '2026-01-01')", [i.user]).unwrap();
    break_carry(&c);
    let before = rows(&c);
    assert_eq!(browser.send(&app, "POST", "/tracker/tax_report", Some(&post("200")), Csrf::Header, &TURBO).await.status, 500);
    assert_eq!((rows(&c), carry(&c, i.user)), (before, vec![(2025, "100.0".to_string())]));
}
