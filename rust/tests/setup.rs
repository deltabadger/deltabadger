//! The first-run setup against Rails' own answers (script/rust/setup.rb -> fixtures/setup_vectors.json): the setup
//! page, the SETUP_TOKEN gate, admin creation and the redirect to /setup in front of every page while no admin
//! exists. Rails-free: each scenario runs on an install made from the exported schema, seeded with the rows Rails
//! seeded, and every response, user row and app_configs row is compared.
mod common;
use common::html;
use common::web::{Answer, Browser, Csrf, TestClock};
use deltabadger::store::{self, Paths};
use deltabadger::web::{self, csrf, session, App, Config};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

fn vectors() -> &'static Value {
    static V: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    V.get_or_init(|| serde_json::from_str(include_str!("fixtures/setup_vectors.json")).unwrap())
}

#[test]
fn the_rails_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let pinned = vectors()["sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 16);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the vectors were recorded: re-run script/rust/setup.rb");
    }
}

/// An install as `db:prepare` leaves it (the exported schema), with no users.
fn install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (file, schema) in [("production.sqlite3", include_str!("fixtures/settings_primary_schema.sql")), ("production_queue.sqlite3", include_str!("fixtures/settings_queue_schema.sql"))] {
        let c = rusqlite::Connection::open(dir.path().join(file)).unwrap();
        c.execute_batch(&format!("BEGIN IMMEDIATE;\n{schema}\nCOMMIT;")).unwrap();
    }
    dir
}

fn insert_rows(dir: &Path, table: &str, rows: &Value) {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    for row in rows.as_array().unwrap() {
        let row = row.as_object().unwrap();
        let columns: Vec<&String> = row.keys().collect();
        let sql = format!("INSERT INTO {table} ({}) VALUES ({})", columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(","), vec!["?"; columns.len()].join(","));
        let values: Vec<rusqlite::types::Value> = row.values().map(|v| match v {
            Value::Null => rusqlite::types::Value::Null,
            Value::Number(n) if n.is_i64() => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
            Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap()),
            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            other => rusqlite::types::Value::Text(other.to_string()),
        }).collect();
        c.execute(&sql, rusqlite::params_from_iter(values)).unwrap();
    }
}

fn table(dir: &Path, name: &str) -> Vec<serde_json::Map<String, Value>> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut statement = c.prepare(&format!("SELECT * FROM {name} ORDER BY id")).unwrap();
    let names: Vec<String> = statement.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = statement.query([]).unwrap();
    let mut out = vec![];
    while let Some(row) = rows.next().unwrap() {
        out.push(names.iter().enumerate().map(|(i, name)| (name.clone(), match row.get_ref(i).unwrap() {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(n) => json!(n),
            rusqlite::types::ValueRef::Real(n) => json!(n),
            rusqlite::types::ValueRef::Text(t) => json!(std::str::from_utf8(t).unwrap()),
            rusqlite::types::ValueRef::Blob(_) => panic!("blob in {name}"),
        })).collect());
    }
    out
}

/// The app on `dir` with exactly `env` (SECRET_KEY_BASE added): what `deltabadger serve` builds after a restart.
fn app(dir: &Path, env: &HashMap<String, String>, clock: Arc<TestClock>) -> App {
    let opened = store::open(&Paths::from_env(&|_| None, dir)).expect("the install passes store::check");
    let secret = vectors()["secret_key_base"].as_str().unwrap().to_string();
    let lookup = move |name: &str| if name == "SECRET_KEY_BASE" { Some(secret.clone()) } else { env.get(name).cloned() };
    App::new(Config::from_env(&lookup).unwrap(), &lookup, opened.primary, clock).unwrap()
        .with_figure_source(web::figure::loading::Source::Disabled).unwrap()
}

const HEADERS: [&str; 11] = ["location", "content-type", "cache-control", "x-frame-options", "x-xss-protection", "x-content-type-options",
                             "x-permitted-cross-domain-policies", "referrer-policy", "content-security-policy-report-only", "retry-after", "set-cookie"];

fn masked_nonce(policy: &str) -> String {
    let Some(start) = policy.find("'nonce-") else { return policy.to_string() };
    let end = start + 7 + policy[start + 7..].find('\'').unwrap_or(0);
    format!("{}'nonce-[nonce]{}", &policy[..start], &policy[end..])
}

/// One response as tests/pages.rs compares it: status, the listed headers (the session cookie only by its
/// attributes), and the normalised body.
fn comparable(status: u64, headers: &BTreeMap<String, String>, body: &str) -> Value {
    let mut out = BTreeMap::new();
    for name in HEADERS {
        let Some(value) = headers.get(name) else { continue };
        let value = match name {
            "location" => html::location(value),
            "content-security-policy-report-only" => masked_nonce(value),
            "set-cookie" => match value.strip_prefix("_deltabadger_session=").or_else(|| value.strip_prefix("_deltabadger_rust_session=")) {
                Some(cookie) => format!("[session]{}", cookie.find(';').map_or("", |at| &cookie[at..])),
                None => continue,
            },
            _ => value.clone(),
        };
        out.insert(name, value);
    }
    let markup = headers.get("content-type").is_some_and(|v| v.starts_with("text/html") || v.starts_with("text/vnd.turbo-stream.html"));
    json!({ "status": status, "headers": out, "body": if markup { json!(html::normalize(body)) } else { json!(body) } })
}

/// Rails writes its cookie on every response; this crate only when the session's content changed (web::session).
fn rails_answer(name: &str, recorded: &Value, launchpad: Option<&str>) -> Value {
    let changed = recorded["session_changed"].as_bool().unwrap();
    let headers = recorded["headers"].as_object().unwrap().iter()
        .filter(|(name, _)| changed || name.as_str() != "set-cookie")
        .map(|(name, value)| (name.clone(), value.as_str().unwrap().to_string())).collect();
    let mut body = masked_launchpad(recorded["body"].as_str().unwrap(), launchpad);
    if recorded["status"] == 422 {
        // Named security divergence: retain Rails' recorded answers, but require a fixed message
        // in place of the upstream error text/URL. All surrounding markup and effects still match.
        let diagnostic = match name {
            "claim_server_json" => Some("{&quot;error&quot;:&quot;boom&quot;}"),
            "claim_server_html" => Some("HTTP 502"),
            "claim_server_blank" => Some("the server responded with status 503 for POST [launchpad]/api/claim"),
            _ => None,
        };
        if let Some(diagnostic) = diagnostic {
            assert_eq!(body.matches(diagnostic).count(), 1, "{name}: recorded diagnostic changed");
            body = body.replace(diagnostic, web::launchpad::CLAIM_FAILED);
        }
    }
    comparable(recorded["status"].as_u64().unwrap(), &headers, &body)
}

fn rust_answer(answer: &Answer, launchpad: Option<&str>) -> Value {
    let headers = answer.headers.iter().fold(BTreeMap::new(), |mut all, (name, value)| { all.entry(name.clone()).or_insert_with(|| value.clone()); all });
    comparable(answer.status.into(), &headers, &masked_launchpad(&answer.body, launchpad))
}

/// Each side's fake launchpad listens on its own port, and a message may name the claim URL.
fn masked_launchpad(body: &str, url: Option<&str>) -> String {
    url.map_or_else(|| body.to_string(), |url| body.replace(url, "[launchpad]"))
}

/// The launchpad as script/rust/setup.rb fakes it: the recorded answer to every claim, a hang-up after the request
/// ('close'), or a port nothing listens on ('refused'). Returns the base URL and what it was sent.
async fn fake_launchpad(answer: &str) -> (String, Arc<std::sync::Mutex<Vec<Value>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    if answer == "refused" { return (url, requests); } // dropped: the port refuses
    let reply = (answer != "close").then(|| vectors()["launchpad_answers"][answer].clone());
    let seen = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut byte = [0u8; 4096];
            let (head, body) = loop {
                let n = stream.read(&mut byte).await.unwrap_or(0);
                if n == 0 { return; }
                raw.extend_from_slice(&byte[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                if body.len() >= length { break (head.to_string(), body.to_string()); }
            };
            let content_type = head.lines().find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.trim().to_string()));
            seen.lock().unwrap().push(json!({ "request_line": head.lines().next().unwrap_or_default(), "content_type": content_type, "body": body }));
            if let Some(reply) = &reply {
                let text = reply["body"].as_str().unwrap();
                let out = format!("HTTP/1.1 {} Answer\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", reply["status"], reply["content_type"].as_str().unwrap(), text.len());
                let _ = stream.write_all(out.as_bytes()).await;
            }
        }
    });
    (url, requests)
}

/// The hidden authenticity_token of the form on `page` whose action is exactly `action`.
fn form_token(page: &str, action: &str) -> Option<String> {
    let action = action.replace('&', "&amp;");
    page.split("<form").skip(1).find_map(|form| {
        let (tag, rest) = form.split_once('>')?;
        if !tag.contains(&format!(" action=\"{action}\"")) { return None; }
        rest.split("</form>").next()?.split("name=\"authenticity_token\" value=\"").nth(1)?.split('"').next().map(str::to_string)
    })
}

/// Every CSRF token this crate rendered must unmask to the session's token, every asset path must be served.
fn assert_genuine(app: &App, browser: &Browser, answer: &Answer, now: chrono::DateTime<chrono::Utc>, step: &str) {
    let masked = html::masked_values(&answer.body);
    for path in &masked.assets { assert!(web::assets::find(path).is_some(), "{step}: {path} is not an embedded asset"); }
    if masked.tokens.is_empty() { return; }
    let token = browser.cookie.as_deref().and_then(|cookie| session::open(&app.keys.session, cookie, now)).and_then(|s| s.csrf)
        .unwrap_or_else(|| panic!("{step}: the page carries CSRF tokens but the session has none"));
    for rendered in &masked.tokens { assert!(csrf::valid(&token, rendered), "{step}: a rendered CSRF token does not verify"); }
}

struct Played { responses: Vec<Value>, dir: tempfile::TempDir, app: App, requests: Vec<Value>, woken: usize }

async fn play(name: &str, scenario: &Value) -> Played {
    let dir = install();
    insert_rows(dir.path(), "users", &scenario["seed"]["users"]);
    insert_rows(dir.path(), "app_configs", &scenario["seed"]["app_configs"]);
    let at = vectors()["at"].as_str().unwrap();
    let clock = TestClock::at(at);
    let mut env: HashMap<String, String> = HashMap::new();
    let apply = |env: &mut HashMap<String, String>, changes: &Value| for (key, value) in changes.as_object().into_iter().flatten() {
        match value.as_str() { Some(v) => { env.insert(key.clone(), v.to_string()); } None => { env.remove(key); } }
    };
    apply(&mut env, &scenario["env"]);
    let (mut launchpad, mut requests) = (None, Arc::new(std::sync::Mutex::new(Vec::new())));
    if let Some(answer) = scenario["launchpad"].as_str() {
        let (url, seen) = fake_launchpad(answer).await;
        env.insert("LAUNCHPAD_URL".into(), url.clone());
        (launchpad, requests) = (Some(url), seen);
    }
    let wakers = deltabadger::jobs::Wakers::default();
    let start = |env: &HashMap<String, String>| { let app = app(dir.path(), env, clock.clone()); app.attach_jobs(wakers.clone()).unwrap(); app };
    let mut app = start(&env);
    let mut browsers: BTreeMap<String, Browser> = BTreeMap::new();
    let mut responses = vec![];
    for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        if !step["env"].is_null() {
            apply(&mut env, &step["env"]);
            drop(app);
            app = start(&env); // a restart with the new environment; browsers keep their cookies
        }
        let browser = browsers.entry(step["client"].as_str().unwrap_or("main").to_string()).or_default();
        let path = step["path"].as_str().unwrap();
        let page = browser.page.clone().unwrap_or_default();
        let mut form: Option<Vec<(String, String)>> = step["form"].as_object().map(|f| f.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect());
        let csrf = match step["csrf"].as_str() {
            Some("form") => {
                // Rails' last page had this form (setup.rb stops otherwise); a page of ours without it already differs.
                if let (Some(form), Some(token)) = (form.as_mut(), form_token(&page, path)) { form.push(("authenticity_token".into(), token)); }
                Csrf::None
            }
            Some("header") if common::web::meta_token(&page).is_some() => Csrf::Header,
            _ => Csrf::None,
        };
        let fields: Option<Vec<(&str, &str)>> = form.as_ref().map(|f| f.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect());
        let headers: Vec<(&str, &str)> = step["headers"].as_object().map(|h| h.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect()).unwrap_or_default();
        let answer = if let Some(body) = step.get("json") {
            browser.send_body(&app, step["method"].as_str().unwrap(), path, Some(body.to_string()), csrf, &headers).await
        } else { browser.send(&app, step["method"].as_str().unwrap(), path, fields.as_deref(), csrf, &headers).await };
        assert_genuine(&app, browser, &answer, clock_now(at), &format!("{name} step {index}"));
        responses.push(rust_answer(&answer, launchpad.as_deref()));
    }
    let woken = wakers.pending().iter().filter(|(job, _)| job == deltabadger::jobs::setup_sync::NAME).count();
    let requests = requests.lock().unwrap().clone();
    Played { responses, dir, app, requests, woken }
}

fn clock_now(at: &str) -> chrono::DateTime<chrono::Utc> { at.parse().unwrap() }

/// The user rows, compared column by column. What differs by design is checked on its own terms: the bcrypt
/// hash must verify the submitted password (Rails' test environment hashes at cost 1, this crate at 11), and the
/// OTP seed must decrypt, under the same keys, to a 32-character base32 seed in active_model_otp's envelope.
fn users_difference(app: &App, scenario: &Value, rails: &[Value], rust: &[serde_json::Map<String, Value>]) -> Option<String> {
    if rails.len() != rust.len() { return Some(format!("{} user rows in Rails, {} here", rails.len(), rust.len())); }
    let seeded = scenario["seed"]["users"].as_array().unwrap().len();
    let password = scenario["steps"].as_array().unwrap().iter().rev().find_map(|s| s["form"]["user[password]"].as_str().or_else(|| s["json"]["user"]["password"].as_str()));
    for (index, (theirs, ours)) in rails.iter().zip(rust).enumerate() {
        for (column, value) in theirs.as_object().unwrap() {
            let mine = ours.get(column).unwrap_or(&Value::Null);
            let created = index >= seeded;
            let same = match column.as_str() {
                "encrypted_password" if created => {
                    let hash = mine.as_str().unwrap_or_default();
                    hash.starts_with("$2a$11$") && password.is_some_and(|p| deltabadger::crypto::verify_password(p, hash))
                }
                "otp_secret_key" if created => {
                    let (Some(envelope), Some(theirs)) = (mine.as_str(), value.as_str()) else { return Some("no OTP seed".into()) };
                    let shape = |text: &str| { let v: Value = serde_json::from_str(text).unwrap(); v["h"].as_object().unwrap().keys().cloned().collect::<Vec<_>>() };
                    let seed = app.cipher.decrypt(envelope).unwrap_or_default();
                    let rails_seed = app.cipher.decrypt(theirs).unwrap_or_default();
                    shape(envelope) == shape(theirs) && seed.len() == 32 && rails_seed.len() == 32
                        && seed.bytes().all(|b| b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".contains(&b)) && seed != rails_seed
                }
                _ => value == mine,
            };
            if !same { return Some(format!("users[{index}].{column}: Rails {value}, Rust {mine}")); }
        }
    }
    None
}

fn app_configs_difference(app: &App, rails: &[Value], rust: &[serde_json::Map<String, Value>]) -> Option<String> {
    let plain = |row: &serde_json::Map<String, Value>| {
        let mut row = row.clone();
        if let Some(value) = row.get("value").and_then(Value::as_str) { row.insert("value".into(), json!(app.cipher.decrypt(value).unwrap())); }
        Value::Object(row)
    };
    let theirs: Vec<Value> = rails.iter().map(|r| plain(r.as_object().unwrap())).collect();
    let ours: Vec<Value> = rust.iter().map(plain).collect();
    (theirs != ours).then(|| format!("app_configs differ:\n  rails: {theirs:?}\n  rust:  {ours:?}"))
}

fn first_difference(name: &str, rails: &[Value], rust: &[Value]) -> Option<String> {
    if rails.len() != rust.len() { return Some(format!("{name}: {} responses in Rails, {} here", rails.len(), rust.len())); }
    for (index, (a, b)) in rails.iter().zip(rust).enumerate() {
        if a == b { continue; }
        if a["status"] != b["status"] || a["headers"] != b["headers"] {
            return Some(format!("{name} step {index}:\n  rails: {} {}\n  rust:  {} {}", a["status"], a["headers"], b["status"], b["headers"]));
        }
        let lines = |body: &Value| match body {
            Value::Array(lines) => lines.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect::<Vec<_>>(),
            text => vec![text.as_str().unwrap_or_default().to_string()],
        };
        return Some(format!("{name} step {index}: {}", html::first_difference(&lines(&a["body"]), &lines(&b["body"])).unwrap_or_default()));
    }
    None
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_answer_the_setup_grid_alike() {
    let only = std::env::var("SETUP").ok();
    let mut failures = vec![];
    let scenarios = vectors()["scenarios"].as_object().unwrap();
    assert_eq!(scenarios.len(), 137, "a scenario was dropped or added without this count");
    for (name, scenario) in scenarios {
        if only.as_deref().is_some_and(|prefixes| !prefixes.split(',').any(|p| name.starts_with(p))) { continue; }
        let mut played = play(name, scenario).await;
        let mut rails: Vec<Value> = scenario["responses"].as_array().unwrap().iter().map(|r| rails_answer(name, r, scenario["launchpad_url"].as_str())).collect();
        if name == "invalid_missing_user" || name == "followup_json_empty_user" {
            // ActionController::ParameterMissing. Rails' test environment shows its exception page for it; in
            // production it is a bare 400. Here: 400 with public/400.html, made below the controllers.
            let (Some(theirs), Some(ours)) = (rails.pop(), played.responses.pop()) else { panic!("{name}: no responses") };
            let page = html::normalize(&String::from_utf8_lossy(web::assets::find("/400.html").unwrap().body));
            if theirs["status"] != 400 || ours["status"] != 400 || ours["body"] != json!(page) { failures.push(format!("{name}: expected 400 with public/400.html, got {ours}")); }
        }
        if let Some(message) = first_difference(name, &rails, &played.responses) { failures.push(message); continue; }
        // The claim requests the launchpad received, and Setup::SeedAndSyncJob: enqueued in Rails, woken here.
        if scenario["launchpad_requests"] != json!(played.requests) { failures.push(format!("{name}: launchpad requests: Rails {}, Rust {:?}", scenario["launchpad_requests"], played.requests)); }
        let enqueued = scenario["enqueued"].as_array().unwrap().iter().filter(|job| *job == "Setup::SeedAndSyncJob").count();
        if enqueued != played.woken { failures.push(format!("{name}: Rails enqueued the setup sync {enqueued} times, Rust woke it {}", played.woken)); }
        let users = table(played.dir.path(), "users");
        if let Some(message) = users_difference(&played.app, scenario, scenario["users"].as_array().unwrap(), &users) { failures.push(format!("{name}: {message}")); }
        if let Some(message) = app_configs_difference(&played.app, scenario["app_configs"].as_array().unwrap(), &table(played.dir.path(), "app_configs")) { failures.push(format!("{name}: {message}")); }
    }
    assert!(failures.is_empty(), "{} scenarios differ:\n{}", failures.len(), failures.join("\n\n"));
}

async fn fresh(env: &[(&str, &str)]) -> (tempfile::TempDir, App) {
    let dir = install();
    let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let app = app(dir.path(), &env, TestClock::at("2026-09-10T12:00:30.123456Z"));
    (dir, app)
}

fn admins(dir: &Path) -> i64 {
    rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap().query_row("SELECT count(*) FROM users WHERE admin = 1", [], |r| r.get(0)).unwrap()
}

fn account(email: &str) -> Vec<(&str, &str)> {
    vec![("locale", "en"), ("user[name]", "Owner"), ("user[email]", email), ("user[password]", "Correct-horse-9")]
}

async fn submit(browser: &mut Browser, app: &App, path: &str, mut form: Vec<(&str, &str)>) -> Answer {
    let page = browser.page.clone().unwrap_or_default();
    let token = form_token(&page, path).expect("the setup page has the account form");
    form.push(("authenticity_token", &token));
    browser.send(app, "POST", path, Some(&form), Csrf::None, &[]).await
}

/// Two owners submit the form at the same moment, both having loaded it while no admin existed. Rails checks in
/// a before_action and saves later, so both can pass the check; here the check and the insert are one IMMEDIATE
/// transaction, and the request that loses gets Rails' "admin exists" answer. Both requests are held after the
/// password is hashed (the hook) until both have arrived, so each has passed any check made before that point.
#[tokio::test(flavor = "current_thread")]
async fn two_concurrent_creates_leave_one_admin() {
    let dir = install();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let hook = barrier.clone();
    let app = app(dir.path(), &HashMap::new(), TestClock::at("2026-09-10T12:00:30.123456Z"))
        .with_password_hook(Arc::new(move || { hook.wait(); })).unwrap();
    let (mut a, mut b) = (Browser::default(), Browser::default());
    assert_eq!(a.get(&app, "/setup").await.status, 200);
    assert_eq!(b.get(&app, "/setup").await.status, 200);
    let (first, second) = tokio::join!(submit(&mut a, &app, "/setup?locale=en", account("one@example.com")), submit(&mut b, &app, "/setup?locale=en", account("two@example.com")));
    let mut locations: Vec<_> = [first, second].iter().map(|answer| (answer.status, answer.header("location").map(str::to_string))).collect();
    locations.sort();
    assert_eq!(locations, vec![(302, Some("/".into())), (302, Some("/bots".into()))], "one creates, the other is told an admin exists");
    assert_eq!(admins(dir.path()), 1);
}

/// SETUP_TOKEN set but blank locks setup, and no request unlocks it: not an empty param, not an empty session.
#[tokio::test(flavor = "current_thread")]
async fn a_blank_setup_token_matches_nothing() {
    for value in ["", "   "] {
        let (dir, app) = fresh(&[("SETUP_TOKEN", value)]).await;
        let mut browser = Browser::default();
        for path in ["/setup", "/setup?token=", "/setup?token=%20%20%20", "/setup?token=%00"] {
            assert_eq!(browser.get(&app, path).await.status, 403, "{value:?} {path}");
        }
        let answer = browser.send(&app, "POST", "/setup", Some(&[("token", value), ("user[name]", "Owner"), ("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]), Csrf::Header, &[]).await;
        assert_eq!(answer.status, 403);
        assert_eq!(admins(dir.path()), 0);
    }
}

/// The token is compared whole: a prefix, a longer value, another case or the value with whitespace is refused.
#[tokio::test(flavor = "current_thread")]
async fn only_the_exact_setup_token_unlocks() {
    for wrong in ["expected", "expected-token-", "Expected-token", "expected-token%20", "%20expected-token", "expected-tokem"] {
        let (_dir, app) = fresh(&[("SETUP_TOKEN", "expected-token")]).await; // a fresh rate-limit window for each
        let mut browser = Browser::default();
        assert_eq!(browser.get(&app, &format!("/setup?token={wrong}")).await.status, 403, "{wrong}");
        assert_eq!(browser.get(&app, "/setup").await.status, 403, "{wrong}: nothing was kept in the session");
    }
    let (_dir, app) = fresh(&[("SETUP_TOKEN", "expected-token")]).await;
    let mut browser = Browser::default();
    assert_eq!(browser.get(&app, "/setup?token=expected-token").await.header("location"), Some("/setup"));
    assert_eq!(browser.get(&app, "/setup").await.status, 200);
}

/// The value never comes back out: not in a Location, a page, or a cookie readable without the key.
#[tokio::test(flavor = "current_thread")]
async fn the_setup_token_is_never_echoed() {
    let secret = "s3cr3t-0123456789abcdef";
    let (_dir, app) = fresh(&[("SETUP_TOKEN", secret)]).await;
    let mut browser = Browser::default();
    for path in [format!("/setup?token={secret}&x=1"), format!("/setup?token={secret}x&locale=de"), "/setup".into(), format!("/setup.json?token={secret}0")] {
        let answer = browser.get(&app, &path).await;
        assert!(!answer.body.contains(secret) && answer.headers.iter().all(|(_, v)| !v.contains(secret)), "{path}: {} {:?}", answer.status, answer.headers);
    }
    assert!(!deltabadger::web::locale::without_secret_query(&format!("/setup?token={secret}&code=c&claim_code=d&x=1")).contains(secret));
}

/// Admin creation is a CSRF-protected POST: without the session's token nothing is written.
#[tokio::test(flavor = "current_thread")]
async fn admin_creation_requires_the_csrf_token() {
    let (dir, app) = fresh(&[]).await;
    let mut browser = Browser::default();
    browser.get(&app, "/setup").await;
    let answer = browser.send(&app, "POST", "/setup?locale=en", Some(&account("o@example.com")), Csrf::None, &[]).await;
    assert_eq!((answer.status, answer.header("location")), (302, Some("/")));
    let forged = browser.send(&app, "POST", "/setup?locale=en", Some(&[account("o@example.com"), vec![("authenticity_token", "AAAA")]].concat()), Csrf::None, &[]).await;
    assert_eq!(forged.status, 302);
    assert_eq!(admins(dir.path()), 0);
}

/// Once an admin exists, setup is closed for everyone, whatever SETUP_TOKEN says, and a second admin cannot be made.
#[tokio::test(flavor = "current_thread")]
async fn setup_closes_once_an_admin_exists() {
    let (dir, app) = fresh(&[("SETUP_TOKEN", "expected-token")]).await;
    let mut owner = Browser::default();
    owner.get(&app, "/setup?token=expected-token").await;
    owner.get(&app, "/setup").await;
    assert_eq!(submit(&mut owner, &app, "/setup?locale=en", account("owner@example.com")).await.header("location"), Some("/bots"));
    let mut late = Browser::default();
    assert_eq!(late.get(&app, "/setup?token=expected-token").await.header("location"), Some("/"));
    let late_post = late.send(&app, "POST", "/setup", Some(&[account("late@example.com"), vec![("token", "expected-token")]].concat()), Csrf::None, &[]).await;
    assert_ne!(late_post.status, 200);
    assert_eq!(owner.get(&app, "/setup").await.header("location"), Some("/"));
    assert_eq!(admins(dir.path()), 1);
}

/// While no admin exists every page answers 302 to /setup, error routes and unknown paths included; what Rails
/// keeps outside ApplicationController is untouched. With an admin, the gate is gone.
#[tokio::test(flavor = "current_thread")]
async fn the_gate_covers_the_app_zone_and_nothing_else() {
    let (dir, app) = fresh(&[]).await;
    let mut browser = Browser::default();
    for path in ["/", "/no/such/page", "/de/setup", "/404", "/422", "/500", "/bots/new", "/settings/connect", "/jobs", "/api/remove_invalid_keys"] {
        let answer = browser.get(&app, path).await;
        assert_eq!((answer.status, answer.header("location")), (302, Some("/setup")), "{path}");
    }
    for (path, status) in [("/up", 200), ("/health-check", 200), ("/api/v1/bots", 401), ("/settings", 302)] {
        let answer = browser.get(&app, path).await;
        assert_eq!(answer.status, status, "{path}");
        assert_ne!(answer.header("location"), Some("/setup"), "{path}");
    }
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute("INSERT INTO users (name, email, encrypted_password, admin, confirmed_at, created_at, updated_at) VALUES ('O', 'o@example.com', 'x', 1, '2026-01-01', '2026-01-01', '2026-01-01')", []).unwrap();
    assert_eq!(browser.get(&app, "/bots").await.header("location"), Some("/login"));
}

/// `serve` binds on an install with no admin: the first visitor is sent to /setup instead of a refusal to start.
#[tokio::test(flavor = "current_thread")]
async fn serve_binds_an_install_without_an_admin() {
    let (_dir, app) = fresh(&[]).await;
    let listener = web::server::bind(&app, 0).await.expect("no-admin installs are served");
    drop(listener);
}

/// User#valid? on the setup form's fields, in all 15 locales: the full messages, in Rails' order, for names,
/// emails and passwords around every rule, beside an existing member and a Gmail account.
#[test]
fn validation_messages_match_rails_in_every_locale() {
    let dir = install();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute_batch("INSERT INTO users (name, email, created_at, updated_at) VALUES ('Member', 'member@example.com', '2026-01-01', '2026-01-01'), ('Gmail', 'jan.doe@gmail.com', '2026-01-01', '2026-01-01')").unwrap();
    let cases = vectors()["validations"].as_array().unwrap();
    assert_eq!(cases.len(), 15 * 47);
    let mut failures = vec![];
    for case in cases {
        let text = |key: &str| case[key].as_str().unwrap();
        let email = web::setup::normalized_email(text("email"));
        assert_eq!(email, text("normalized_email"), "{case}");
        let errors = web::setup::validate(&c, text("locale"), Some(text("name")), &email, Some(text("password"))).unwrap();
        let full: Vec<&str> = errors.iter().map(|e| e.full.as_str()).collect();
        let password: Vec<&str> = errors.iter().filter(|e| e.attribute == "password").map(|e| e.message.as_str()).collect();
        let strings = |key: &str| case[key].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect::<Vec<_>>();
        if full != strings("messages") || password != strings("password_messages") || full.is_empty() != case["valid"].as_bool().unwrap() {
            failures.push(format!("{case}\n  rust: {full:?} / {password:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// The token comparison is constant-time (SHA-256 digests compared with `ct_eq`, as ActiveSupport's secure_compare
/// does), which no answer of the server shows: this reads the source of `token_matches` instead.
#[test]
fn the_setup_token_is_compared_in_constant_time() {
    let source = include_str!("../src/web/setup.rs");
    let start = source.find("pub fn token_matches(").expect("token_matches is defined");
    let body = &source[start..start + source[start..].find("\n}\n").expect("its end")];
    assert!(body.contains("Sha256::digest(given.as_bytes()).ct_eq(&Sha256::digest(expected.as_bytes()))"), "{body}");
    assert!(!body.contains("==") && !body.contains(".eq("), "{body}");
    for (given, expected, matches) in [("t", "t", true), ("t", "u", false), ("", "", false), (" ", " ", false), ("t", "t ", false), ("token", "tok", false)] {
        assert_eq!(web::setup::token_matches(given, expected), matches, "{given:?} {expected:?}");
    }
}

/// Setup::SeedAndSyncJob after a claim: pending -> in_progress, the deltabadger branch's syncs against the data-api the
/// claim configured (each recorded under its own name), then completed and the banner removed. A sync that fails is
/// logged and the job still completes, as Rails' does.
#[tokio::test(flavor = "current_thread")]
async fn the_setup_sync_runs_the_reference_syncs_and_completes() {
    use deltabadger::jobs::{setup_sync::SeedAndSync, data_api::DataApi, notifications::Notifications, Cx, Db, Job, Wakers};
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any()).respond_with(wiremock::ResponseTemplate::new(503)).mount(&server).await;
    let dir = install();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "setup-sync-secret").unwrap());
    let now = common::web::at("2026-09-10T12:00:30Z");
    for (key, value) in [("market_data_url", server.uri()), ("market_data_token", "t".into()), ("market_data_provider", "deltabadger".into()), ("setup_sync_status", "pending".into())] {
        deltabadger::app_config::set(&c, &cipher, key, &value, now).unwrap();
    }
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = published.clone();
    let db = Db::new(c, cipher.clone()).with_notifications(Notifications::new(move |stream, payload| { sink.lock().unwrap().push((stream.to_string(), payload.to_string())); Ok(()) }));
    let clock = TestClock::at("2026-09-10T12:00:30Z");
    let job = SeedAndSync::new(&|_| None, DataApi::live);
    assert_eq!(job.spec().schedule, None, "on demand only: the claim wakes it");
    let outcome = job.run(Cx { db, clock: &*clock, wakers: Wakers::default() }, vec![]).await;
    assert_eq!(outcome, deltabadger::jobs::Outcome::Done);
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(deltabadger::app_config::get(&c, &cipher, "setup_sync_status").unwrap().as_deref(), Some("completed"));
    assert_eq!(*published.lock().unwrap(), vec![("settings_sync".to_string(), "<turbo-stream action=\"remove\" target=\"flash-syncing\"></turbo-stream>".to_string())]);
    let paths: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| r.url.path().to_string()).collect();
    assert!(!paths.is_empty(), "the syncs asked the configured data-api");
    for name in [deltabadger::jobs::reference::ASSETS, deltabadger::jobs::reference::INDICES, deltabadger::jobs::reference::STOCKS] {
        assert!(deltabadger::jobs::state::read(&c, name, None).unwrap().last_error.is_some(), "{name} recorded its failure");
    }
}

/// Neither the claim code nor the market-data token reaches a page, a header or a log path a refusal prints.
#[tokio::test(flavor = "current_thread")]
async fn claim_secrets_are_not_echoed() {
    let (url, _) = fake_launchpad("ok").await;
    let (dir, app) = fresh(&[("LAUNCHPAD_URL", &url)]).await;
    let mut browser = Browser::default();
    browser.get(&app, "/setup").await;
    let answer = browser.send(&app, "POST", "/setup/platform_connection?code=dbc_secret_code", Some(&[("claim_code", "dbc_secret_code")]), Csrf::Header, &[]).await;
    assert_eq!(answer.status, 200);
    for secret in ["dbc_secret_code", "md-token-123"] {
        assert!(!answer.body.contains(secret) && answer.headers.iter().all(|(_, v)| !v.contains(secret)), "{secret}");
    }
    let stored: Vec<String> = table(dir.path(), "app_configs").iter().map(|r| r["value"].to_string()).collect();
    assert!(stored.iter().all(|v| !v.contains("md-token-123") && !v.contains("proxy.example")), "values are encrypted");
    assert!(!deltabadger::web::locale::without_secret_query("/setup/platform_connection?code=c&claim_code=d").contains("=c"));
}

const JSON_HEADERS: &[(&str, &str)] = &[("content-type", "application/json"), ("accept", "text/html")];

#[tokio::test(flavor = "current_thread")]
async fn regression_setup_query_token_wins_over_body() {
    for path in ["/setup", "/setup.html", "/setup/platform_connection"] {
        for json_body in [true, false] {
            let (url, requests) = fake_launchpad("ok").await;
            let (dir, app) = fresh(&[("SETUP_TOKEN", "expected-token"), ("LAUNCHPAD_URL", &url)]).await;
            let mut browser = Browser::default();
            browser.get(&app, "/setup?token=expected-token").await;
            browser.get(&app, "/setup").await;
            let path = format!("{path}?token=wrong");
            let answer = if json_body {
                browser.send_body(&app, "POST", &path, Some(json!({"token": "expected-token", "claim_code": "unused",
                    "user": {"name": "Owner", "email": "owner@example.com", "password": "Correct-horse-9"}}).to_string()), Csrf::Header, JSON_HEADERS).await
            } else {
                let mut fields = account("owner@example.com");
                fields.push(("token", "expected-token"));
                browser.send(&app, "POST", &path, Some(&fields), Csrf::Header, &[]).await
            };
            assert_eq!(answer.status, 403, "{path}, JSON: {json_body}");
            assert_eq!(admins(dir.path()), 0);
            assert!(table(dir.path(), "app_configs").is_empty());
            assert!(requests.lock().unwrap().is_empty(), "a rejected token must not redeem a claim");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn regression_claim_query_code_wins_over_body() {
    for json_body in [true, false] {
        let (url, requests) = fake_launchpad("ok").await;
        let (_dir, app) = fresh(&[("LAUNCHPAD_URL", &url)]).await;
        let mut browser = Browser::default();
        browser.get(&app, "/setup").await;
        let path = "/setup/platform_connection?claim_code=dbc_query";
        let answer = if json_body {
            browser.send_body(&app, "POST", path, Some(json!({"claim_code": "dbc_body"}).to_string()), Csrf::Header, JSON_HEADERS).await
        } else {
            browser.send(&app, "POST", path, Some(&[("claim_code", "dbc_body")]), Csrf::Header, &[]).await
        };
        assert_eq!(answer.status, 200);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(serde_json::from_str::<Value>(requests[0]["body"].as_str().unwrap()).unwrap(), json!({"code": "dbc_query"}));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn regression_setup_query_user_replaces_body_shallowly() {
    let (dir, app) = fresh(&[]).await;
    let mut browser = Browser::default();
    browser.get(&app, "/setup").await;
    let answer = browser.send_body(&app, "POST", "/setup?user%5Bname%5D=Query&locale=de", Some(json!({
        "locale": "en", "user": {"name": "Body", "email": "body@example.com", "password": "Correct-horse-9"}
    }).to_string()), Csrf::Header, JSON_HEADERS).await;
    assert_eq!(answer.status, 422, "query user replaces the entire body user, including email and password");
    assert_eq!(admins(dir.path()), 0);
    assert!(answer.body.contains("value=\"Query\""));
    assert!(!answer.body.contains("body@example.com"));
    assert!(answer.body.contains("lang=\"de\""));
}

#[tokio::test(flavor = "current_thread")]
async fn regression_launchpad_error_secrets_never_reach_response_or_flash() {
    use wiremock::{Mock, MockServer, ResponseTemplate};
    const TOKEN: &str = "dbc_claim_secret_sentinel";
    const CREDENTIAL: &str = "upstream_credential_sentinel";
    for status in [400, 401, 422, 500, 502, 503] {
        for (content_type, body) in [
            ("application/json", json!({"error": format!("{TOKEN} {CREDENTIAL}")}).to_string()),
            ("text/plain", format!("{TOKEN} {CREDENTIAL}")),
            ("text/html", format!("<html>{TOKEN} {CREDENTIAL}</html>")),
        ] {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::method("POST")).and(wiremock::matchers::path("/api/claim"))
                .respond_with(ResponseTemplate::new(status).set_body_raw(body, content_type)).mount(&server).await;
            for automatic in [false, true] {
                let url = server.uri();
                let mut env = vec![("LAUNCHPAD_URL", url.as_str())];
                if automatic { env.push(("CLAIM_TOKEN", TOKEN)); }
                let (dir, app) = fresh(&env).await;
                let mut browser = Browser::default();
                let initial = browser.get(&app, "/setup").await;
                let answer = if automatic { initial } else {
                    browser.send(&app, "POST", "/setup/platform_connection", Some(&[("claim_code", TOKEN)]), Csrf::Header, &[]).await
                };
                assert_eq!(answer.status, if automatic { 200 } else { 422 });
                for secret in [TOKEN, CREDENTIAL] {
                    assert!(!answer.body.contains(secret), "status {status}, {content_type}, automatic: {automatic}");
                    assert!(!format!("{:?}", answer.headers).contains(secret));
                    let data = session::open(&app.keys.session, browser.cookie.as_deref().unwrap(), app.now()).unwrap();
                    assert!(!format!("{:?}", data.flash).contains(secret));
                }
                let safe = if status == 401 { web::launchpad::INVALID_CLAIM } else { "Deltabadger could not complete the claim. Please try again." };
                assert!(answer.body.contains(safe));
                assert!(table(dir.path(), "app_configs").is_empty());
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn claim_cannot_write_after_an_admin_is_created_while_launchpad_replies() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (arrived_tx, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        loop {
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            assert_ne!(n, 0);
            raw.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&raw).contains("dbc_delayed") { break; }
        }
        arrived_tx.send(()).unwrap();
        release_rx.await.unwrap();
        let body = vectors()["launchpad_answers"]["ok"]["body"].as_str().unwrap();
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    let (dir, app) = fresh(&[("LAUNCHPAD_URL", &url)]).await;
    let wakers = deltabadger::jobs::Wakers::default();
    app.attach_jobs(wakers.clone()).unwrap();
    let mut claimant = Browser::default();
    let mut owner = Browser::default();
    claimant.get(&app, "/setup").await;
    owner.get(&app, "/setup").await;
    let claim = claimant.send(&app, "POST", "/setup/platform_connection", Some(&[("claim_code", "dbc_delayed")]), Csrf::Header, &[]);
    let create = async {
        arrived_rx.await.unwrap();
        assert_eq!(submit(&mut owner, &app, "/setup?locale=en", account("owner@example.com")).await.header("location"), Some("/bots"));
        release_tx.send(()).unwrap();
    };
    let (answer, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(claim, create) }).await.unwrap();
    upstream.await.unwrap();
    assert!(table(dir.path(), "app_configs").is_empty(), "a late claim must not write configuration");
    assert!(wakers.pending().is_empty(), "a refused claim must not enqueue setup sync");
    assert_eq!(answer.status, 422);
    assert!(!answer.body.contains("setup_identity_fields"), "a refused claim cannot set platform identity");
    assert_eq!(admins(dir.path()), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn recurring_jobs_resolve_market_data_config_at_each_run() {
    use deltabadger::jobs::{self, data_api::{self, DataApi}, Cx, Db, Wakers};
    let old = wiremock::MockServer::start().await;
    let new = wiremock::MockServer::start().await;
    for server in [&old, &new] {
        wiremock::Mock::given(wiremock::matchers::any()).respond_with(wiremock::ResponseTemplate::new(503)).mount(server).await;
    }
    for initially_configured in [false, true] {
        let (dir, app) = fresh(&[]).await;
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        let clock = TestClock::at("2026-09-10T12:00:30Z");
        let set = |url: &str, token: &str| {
            for (key, value) in [("market_data_provider", "deltabadger"), ("market_data_url", url), ("market_data_token", token)] {
                deltabadger::app_config::set(&c, &app.cipher, key, value, clock_now("2026-09-10T12:00:30Z")).unwrap();
            }
        };
        if initially_configured { set(&old.uri(), "old-token"); }
        let api = std::rc::Rc::new(data_api::config(&|_| None, &c, &app.cipher).unwrap().map(DataApi::live));
        let resolve = jobs::resolve::all(deltabadger::venue::alpaca::LiveFactory::new(), api, deltabadger::tracker::jobs::system_wall());
        let job = jobs::resolve::configured(resolve(jobs::reference::ASSETS, None).unwrap(), &|_| None, Default::default());
        set(&new.uri(), "new-token");
        let db = Db::new(c, app.cipher.clone());
        job.run(Cx { db: db.clone(), clock: &*clock, wakers: Wakers::default() }, vec![]).await;
        let requests = new.received_requests().await.unwrap();
        assert!(requests.iter().any(|r| r.headers.get("authorization").is_some_and(|v| v == "Bearer new-token")), "the next recurring run must use the claimed config, initially configured: {initially_configured}");
        new.reset().await;
        wiremock::Mock::given(wiremock::matchers::any()).respond_with(wiremock::ResponseTemplate::new(503)).mount(&new).await;
        db.run(move |c, cipher| {
            deltabadger::app_config::set(c, cipher, "market_data_token", "rotated-token", clock_now("2026-09-10T12:00:30Z"))
        }).await.unwrap();
        job.run(Cx { db, clock: &*clock, wakers: Wakers::default() }, vec![]).await;
        assert!(new.received_requests().await.unwrap().iter().any(|r| r.headers.get("authorization").is_some_and(|v| v == "Bearer rotated-token")));
        assert!(old.received_requests().await.unwrap().is_empty());
        new.reset().await;
        wiremock::Mock::given(wiremock::matchers::any()).respond_with(wiremock::ResponseTemplate::new(503)).mount(&new).await;
    }
}
