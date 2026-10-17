//! Page parity: the Rails app and this crate answer the same requests on identical installs with the
//! same frozen clock, and every response must be the same page (script/rust/pages.rb is the Rails half).
//! `PAGES=login,two_factor cargo test --test pages` runs only the scenarios with those name prefixes.
mod common;
#[path="support/settings_snapshot.rs"] mod settings_snapshot;
use common::html;
use common::web::{self, Answer, Browser, Csrf, TestClock};
use deltabadger::web::{cable, csrf, session, App};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Headers compared exactly (after masking). Everything else Rails sends (etag, x-request-id,
/// x-runtime, content-length, vary) is not part of what a page is.
const HEADERS: [&str; 11] = ["location", "content-type", "cache-control", "x-frame-options", "x-xss-protection", "x-content-type-options",
                             "x-permitted-cross-domain-policies", "referrer-policy", "content-security-policy-report-only", "retry-after", "set-cookie"];
const USER_COLUMNS: [&str; 6] = ["id", "failed_attempts", "locked_at", "last_otp_at", "remember_created_at", "updated_at"];
/// Legacy read-only scenarios retain their final bot-row check; actions compare all columns per step.
const BOT_COLUMNS: [&str; 9] = ["id", "status", "label", "position", "settings", "transient_data", "stop_message_key", "started_at", "updated_at"];

fn masked_nonce(policy: &str) -> (String, Option<String>) {
    let Some(start) = policy.find("'nonce-") else { return (policy.to_string(), None) };
    let end = start + 7 + policy[start + 7..].find('\'').unwrap_or(0);
    (format!("{}'nonce-[nonce]{}", &policy[..start], &policy[end..]), Some(policy[start + 7..end].to_string()))
}

/// A step's `headers_extra`: the headers compared beside HEADERS on that step alone (an API answer's etag).
fn extra_headers(step: &Value) -> Vec<String> {
    step["headers_extra"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// One response as the comparison sees it: status, the listed headers, and the normalised body.
fn comparable(status: u64, headers: &BTreeMap<String, Vec<String>>, body: &str, extra: &[String]) -> Value {
    let mut out = BTreeMap::new();
    for name in HEADERS.iter().copied().chain(extra.iter().map(String::as_str)) {
        let Some(values) = headers.get(name) else { continue };
        let value = match name {
            "location" => html::location(&values[0]),
            "content-security-policy-report-only" => {
                let (masked, nonce) = masked_nonce(&values[0]);
                if let Some(meta) = html::meta(body, "csp-nonce") {
                    assert_eq!(Some(meta), nonce, "the page's csp-nonce meta tag must be the nonce of its own policy header");
                }
                masked
            }
            // The session cookie under either name, its value masked; the attributes must match exactly.
            "set-cookie" => match values.iter().find_map(|v| v.strip_prefix("_deltabadger_session=").or_else(|| v.strip_prefix("_deltabadger_rust_session="))) {
                Some(cookie) => format!("[session]{}", cookie.find(';').map_or("", |at| &cookie[at..])),
                None => continue,
            },
            _ => values[0].clone(),
        };
        out.insert(name, value);
    }
    // A Turbo stream is markup too: `<turbo-stream>` elements around `<template>`s.
    let html_body = headers.get("content-type").is_some_and(|v| v[0].starts_with("text/html") || v[0].starts_with("text/vnd.turbo-stream.html"));
    json!({ "status": status, "headers": out, "body": if html_body { json!(html::normalize(body)) } else { json!(body) } })
}

/// A listed divergence of the `countdown_*` scenarios: when a bot acts next. Rails reads it from its
/// job table. This build has no job table and derives it from the bot's row (the next interval
/// checkpoint), which is the same instant after every completed tick; two times are in no row: when a
/// retry the engine has in hand fires, and the checkpoint Rails waits for after a restart that was not
/// a fresh start. There the countdown's end,
/// and with it the progress bar's end and width, differ: Rails has the job's time, this build the
/// checkpoint or nothing. The three values are taken out of both pages, and what they were is
/// returned, so the caller can hold the two sides to differing exactly there.
fn without_countdown(body: &mut Value) -> Vec<String> {
    let mut taken = vec![];
    let Value::Array(lines) = body else { return taken };
    for line in lines.iter_mut() {
        let mut text = line.as_str().unwrap_or_default().to_string();
        if !text.contains("data-controller=\"countdown\"") && !text.contains("data-controller=\"progress-bar\"") { continue; }
        for attribute in [" data-countdown-end-time-value=\"", " data-progress-bar-end-time-value=\""] {
            if let Some(start) = text.find(attribute) {
                let end = start + attribute.len() + text[start + attribute.len()..].find('"').unwrap() + 1;
                taken.push(text[start..end].trim().to_string());
                text.replace_range(start..end, "");
            }
        }
        if let Some(start) = text.find("width: ") {
            let end = start + text[start..].find('%').unwrap();
            taken.push(text[start..end].to_string());
            text.replace_range(start..end, "width: [progress]");
        }
        *line = json!(text);
    }
    taken
}

/// Rails writes its session cookie on every response, this crate only when the request changed the
/// session's content (web::session says why). So Rails' `Set-Cookie` counts only on the responses
/// where pages.rb recorded that the content changed (`session_changed`). The comparison then demands
/// both directions: where Rails' session changed, this crate must set a cookie with the same
/// attributes; where it did not, this crate must set none.
fn rails_answer(recorded: &Value, step: &Value) -> Value {
    let changed = recorded["session_changed"].as_bool().expect("pages.rb records whether the session's content changed");
    let headers = recorded["headers"].as_object().unwrap().iter().filter(|(name, _)| changed || name.as_str() != "set-cookie").map(|(name, value)| {
        let values = match value {
            Value::Array(all) => all.iter().map(|v| v.as_str().unwrap().to_string()).collect(),
            one => vec![one.as_str().unwrap().to_string()],
        };
        (name.clone(), values)
    }).collect();
    let status = recorded["status"].as_u64().unwrap_or_else(|| panic!("recorded response has no status"));
    let body = recorded["body"].as_str().unwrap_or_else(|| panic!("recorded response has no body"));
    let safe_body=body.to_string();
    // #498 and locale filtering are now the oracle: compare the rendered bytes directly.
    let mut answer = comparable(status, &headers, &safe_body, &extra_headers(step));
    for key in ["action_snapshot", "rows_before", "rows_after", "other_rows_before", "other_rows_after", "exception", "settings_rows", "broadcasts",
                "venue_requests", "may_mutate", "raw_rows", "session", "job_classes"] {
        if let Some(value) = recorded.get(key) { answer[key] = value.clone(); }
    }
    answer
}

fn rust_answer(answer: &Answer, step: &Value) -> Value {
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in &answer.headers { headers.entry(name.clone()).or_default().push(value.clone()); }
    comparable(answer.status.into(), &headers, &answer.body, &extra_headers(step))
}

/// The Rails job a wake of this crate's runner stands for, for a step's `jobs` (pages.rb records the classes its
/// step enqueued). A wake with no counterpart is written as its own name, so it can never match a Rails class.
const JOB_CLASSES: [(&str, &str); 3] = [("api_key_validator", "ApiKeyValidatorJob"), ("tracker_ledger", "Tracker::LedgerJob"),
                                        ("portfolio_backfill", "PortfolioSnapshot::BackfillJob")];

fn job_classes(wakes: &[(String, Option<String>)]) -> Value {
    let mut classes: Vec<String> = wakes.iter().map(|(name, _)| JOB_CLASSES.iter().find(|(rust, _)| rust == name).map_or_else(|| format!("rust:{name}"), |(_, rails)| rails.to_string())).collect();
    classes.sort();
    json!(classes)
}

/// Masking must not hide a broken value: every CSRF token this crate rendered has to unmask to the
/// token of the session the response left the browser with, every signed stream name has to verify,
/// and every `/assets/` path has to be a file this binary serves. (script/rust/pages.rb checks
/// Rails' the same way.)
fn assert_genuine(app: &App, browser: &Browser, answer: &Answer, now: chrono::DateTime<chrono::Utc>, step: &str) {
    let html::Masked { tokens, streams, assets } = html::masked_values(&answer.body);
    for signed in &streams {
        assert!(cable::verified_stream_name(&app.keys.streams, signed).is_some(), "{step}: a rendered signed stream name does not verify: {signed}");
    }
    for path in &assets {
        assert!(deltabadger::web::assets::find(path).is_some(), "{step}: the page refers to {path}, which is not an embedded asset");
    }
    if tokens.is_empty() { return; }
    let session = browser.cookie.as_deref().and_then(|cookie| session::open(&app.keys.session, cookie, now));
    let token = session.and_then(|s| s.csrf).unwrap_or_else(|| panic!("{step}: the page carries CSRF tokens but the session has none"));
    for rendered in &tokens {
        assert!(csrf::valid(&token, rendered), "{step}: a rendered CSRF token does not verify against the session: {rendered}");
    }
}

fn users(dir: &Path) -> Value {
    rows(dir, "users", &USER_COLUMNS)
}

const ACTION_TABLES: [&str; 6] = ["bots", "bot_index_assets", "bot_activity_logs", "transactions", "api_keys", "users"];
const ACTION_MUTATION_TABLES: [&str; 3] = ["bots", "bot_index_assets", "bot_activity_logs"];

fn quoted(identifier: &str) -> String { format!("\"{}\"", identifier.replace('"', "\"\"")) }

fn read_rows(c: &rusqlite::Connection, table: &str, columns: &[&str], parse_json: bool) -> Result<Value, Box<dyn std::error::Error>> {
    let selection = if columns.is_empty() { "*".to_string() } else { columns.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(", ") };
    let mut statement = c.prepare(&format!("SELECT {selection} FROM {}", quoted(table)))?;
    let names: Vec<String> = statement.column_names().iter().map(|s| s.to_string()).collect();
    let order = if names.iter().any(|s| s == "id") { quoted("id") } else { names.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(", ") };
    statement = c.prepare(&format!("SELECT {selection} FROM {} ORDER BY {order}", quoted(table)))?;
    let mut cursor = statement.query([])?;
    let mut rows = vec![];
    while let Some(row) = cursor.next()? {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            let mut value = match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Real(n) => Value::Number(serde_json::Number::from_f64(n).ok_or("nonfinite SQLite REAL")?),
                rusqlite::types::ValueRef::Text(t) => json!(std::str::from_utf8(t)?),
                rusqlite::types::ValueRef::Blob(_) => return Err(format!("unexpected blob in {table}.{name}").into()),
            };
            let json_column = matches!((table, name.as_str()), ("bots", "settings" | "transient_data")
                | ("bot_activity_logs", "details") | ("transactions", "error_messages"));
            if parse_json && json_column {
                if let Some(text) = value.as_str() { value = serde_json::from_str(text)?; }
            }
            object.insert(name.clone(), value);
        }
        rows.push(Value::Object(object));
    }
    Ok(Value::Array(rows))
}

fn rows(dir: &Path, table: &str, columns: &[&str]) -> Value {
    let result = (|| {
        let c = rusqlite::Connection::open(dir.join("production.sqlite3"))?;
        read_rows(&c, table, columns, false)
    })();
    result.unwrap_or_else(|error| panic!("reading {table}: {error}"))
}

// Called only before send or after its awaited response. The App has no background engine/writer.
// Credentials are compared as they decrypt (settings_snapshot::decrypted), never as ciphertext.
fn action_rows(dir: &Path, cipher: &deltabadger::crypto::Cipher) -> Result<(Value, Value), Box<dyn std::error::Error>> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3"))?;
    let mut statement = c.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let tables = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let (mut actions, mut others) = (serde_json::Map::new(), serde_json::Map::new());
    for table in tables {
        let mut value = read_rows(&c, &table, &[], true)?;
        if ACTION_TABLES.contains(&table.as_str()) {
            for row in value.as_array_mut().into_iter().flatten().filter_map(Value::as_object_mut) {
                for (name, column) in row.iter_mut() { *column = settings_snapshot::decrypted(&table, name, column.take(), cipher); }
            }
            actions.insert(table, value);
        }
        else { others.insert(table, value); }
    }
    Ok((Value::Object(actions), Value::Object(others)))
}

// Compare row ids and every column, retaining JSON types, absent keys and array order.
fn row_difference(context: &str, before: &Value, after: &Value) -> Option<String> {
    let (Some(a), Some(b)) = (before.as_object(), after.as_object()) else { return Some(format!("{context}: missing row snapshot")) };
    let tables: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
    for table in tables {
        let (Some(left), Some(right)) = (a.get(table).and_then(Value::as_array), b.get(table).and_then(Value::as_array)) else {
            return Some(format!("{context}: table {table} missing"));
        };
        let ids: std::collections::BTreeSet<_> = left.iter().chain(right).map(|row| row.get("id").unwrap_or(row).to_string()).collect();
        for id in ids {
            let find = |rows: &Vec<Value>| rows.iter().find(|row| { let key = row.get("id").unwrap_or(row).to_string(); key == id }).cloned();
            let (l, r) = (find(left), find(right));
            if l == r { continue; }
            let (Some(l), Some(r)) = (l.as_ref().and_then(Value::as_object), r.as_ref().and_then(Value::as_object)) else {
                return Some(format!("{context}: table {table} id={id} column <row> inserted/deleted: rails/before={l:?}, rust/after={r:?}"));
            };
            for column in l.keys().chain(r.keys()).collect::<std::collections::BTreeSet<_>>() {
                if l.get(column) != r.get(column) {
                    return Some(format!("{context}: table {table} id={id} column {column}: rails/before={:?}, rust/after={:?}", l.get(column), r.get(column)));
                }
            }
        }
        if left.len() != right.len() { return Some(format!("{context}: table {table} duplicate primary key")); }
        if left != right { return Some(format!("{context}: table {table} row order differs")); }
    }
    None
}

fn action_snapshot_difference(context: &str, rails: &Value, rust: &Value) -> Option<String> {
    for (side, response) in [("Rails", rails), ("Rust", rust)] {
        for key in ["rows_before", "rows_after", "other_rows_before", "other_rows_after"] {
            let Some(rows) = response.get(key).and_then(Value::as_object) else { return Some(format!("{context}: {side} missing {key}")) };
            if key.starts_with("rows_") {
                for table in ACTION_TABLES {
                    if !rows.get(table).is_some_and(Value::is_array) { return Some(format!("{context}: {side} {key} missing table {table}")); }
                }
            }
        }
        // A step may declare the tables it may write (`may_mutate`); without one, a bot action's.
        let allowed: Vec<&str> = match response.get("may_mutate") {
            Some(list) => list.as_array().into_iter().flatten().filter_map(Value::as_str).collect(),
            None => ACTION_MUTATION_TABLES.to_vec(),
        };
        let unchanged = |key: &str| -> Value {
            Value::Object(response[key].as_object().into_iter().flatten()
                .filter(|(table, _)| !allowed.contains(&table.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        };
        for (before, after) in [("rows_before", "rows_after"), ("other_rows_before", "other_rows_after")] {
            if let Some(message) = row_difference(&format!("{context} {side} forbidden write"), &unchanged(before), &unchanged(after)) { return Some(message); }
        }
    }
    for key in ["rows_before", "rows_after", "other_rows_before", "other_rows_after"] {
        if let Some(message) = row_difference(&format!("{context} {key}"), &rails[key], &rust[key]) { return Some(message); }
    }
    None
}

/// Things that happen to the install between two requests, outside any browser (script/rust/pages.rb BEFORE).
fn before(dir: &Path, what: &str) {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    match what {
        "change_password" => c.execute("UPDATE users SET encrypted_password = ?1 WHERE id = (SELECT min(id) FROM users)", [deltabadger::crypto::hash_password("Another-horse-7").unwrap()]).unwrap(),
        "unconfirm" => c.execute("UPDATE users SET confirmed_at = NULL WHERE id = (SELECT min(id) FROM users)", []).unwrap(),
        "disable_kraken_xbt" => c.execute("UPDATE tickers SET available = 0 WHERE ticker = 'XBTEUR'", []).unwrap(),
        other => panic!("unknown step.before {other}"),
    };
}

fn scenario(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap()
}

/// Runs one scenario's steps against this crate; returns what Rails' rails.json holds.
struct TestMail;
impl deltabadger::web::settings::mail::Mailer for TestMail{
    fn deliver(&self,message:deltabadger::mail::Message,_settings:Option<deltabadger::mail::smtp::Settings>,now:chrono::DateTime<chrono::Utc>)->deltabadger::web::settings::mail::Delivery{
        Box::pin(async move{message.encode(now,"fixture@deltabadger").expect("real message encodes");Ok(())})
    }
}

async fn run(dir: &Path) -> Value {
    let scenario = scenario(dir);
    assert_eq!(scenario["page_parity_scratch"], true, "{} is not a page-parity scratch copy", dir.display());
    let mut now: chrono::DateTime<chrono::Utc> = scenario["at"].as_str().unwrap().parse().unwrap();
    let clock = TestClock::at(scenario["at"].as_str().unwrap());
    let key_server=wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET")).and(wiremock::matchers::path_regex("/v2/account$")).respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({"status":"ACTIVE","cash":"0"}))).with_priority(10).mount(&key_server).await;
    wiremock::Mock::given(wiremock::matchers::method("GET")).and(wiremock::matchers::path_regex("/v2/positions$")).respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!([]))).mount(&key_server).await;
    wiremock::Mock::given(wiremock::matchers::header("APCA-API-KEY-ID","incorrect-key")).respond_with(wiremock::ResponseTemplate::new(401)).with_priority(1).mount(&key_server).await;
    wiremock::Mock::given(wiremock::matchers::header("APCA-API-KEY-ID","failure-key")).respond_with(wiremock::ResponseTemplate::new(503).set_body_json(json!({"message":"unavailable"}))).with_priority(1).mount(&key_server).await;
    wiremock::Mock::given(wiremock::matchers::path_regex("/v2/positions$")).and(wiremock::matchers::header("APCA-API-KEY-ID","positions-bad")).respond_with(wiremock::ResponseTemplate::new(401).set_body_json(json!({"message":"unauthorized"}))).with_priority(1).mount(&key_server).await;
    for (key,body) in [("r9-no-cash",json!({"status":"ACTIVE"})),("r9-null-cash",json!({"status":"ACTIVE","cash":null}))]{
        wiremock::Mock::given(wiremock::matchers::path_regex("/v2/account$")).and(wiremock::matchers::header("APCA-API-KEY-ID",key)).respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body)).with_priority(1).mount(&key_server).await;
    }
    wiremock::Mock::given(wiremock::matchers::path_regex("/v2/positions$")).and(wiremock::matchers::header("APCA-API-KEY-ID","r9-unnamed-position")).respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!([{"asset_class":"us_equity","qty":"2"}]))).with_priority(1).mount(&key_server).await;
    mount_key_checks(&key_server).await;
    let app = web::app(dir, scenario["secret_key_base"].as_str().unwrap(), clock.clone()).with_settings_mailer(std::sync::Arc::new(TestMail)).unwrap().with_settings_key_boundary(venue_hosts(&key_server.uri()),deltabadger::web::settings::keys::logger()).unwrap()
        .with_venue_key_urls(&format!("{}/api.kraken.com", key_server.uri()), &format!("{}/api.binance.com", key_server.uri())).unwrap();
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    app.attach_jobs(Default::default()).unwrap();
    let mut observer = None;
    let mut browsers: BTreeMap<String, Browser> = BTreeMap::new();
    let mut responses = vec![];
    let mut etags: BTreeMap<String, String> = BTreeMap::new();
    for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        now += chrono::Duration::seconds(step["advance"].as_i64().unwrap_or(0));
        clock.set(now);
        if let Some(what) = step["before"].as_str() { before(dir, what); }
        let browser = browsers.entry(step["client"].as_str().unwrap_or("main").to_string()).or_default();
        let request_path=if step["confirmation_token_from_user"]==true{
            let c=rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
            let token:String=c.query_row("SELECT confirmation_token FROM users ORDER BY id LIMIT 1",[],|row|row.get(0)).unwrap();
            let token:String=form_urlencoded::byte_serialize(token.as_bytes()).collect();
            step["path"].as_str().unwrap().replace("__R2_TOKEN__",&token)
        }else{step["path"].as_str().unwrap().to_string()};
        let form: Option<Vec<(&str, &str)>> = step["form"].as_object().map(|f| f.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect());
        let mut headers: Vec<(&str, &str)> = step["headers"].as_object().map(|h| h.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect()).unwrap_or_default();
        // pages.rb `if_none_match`: 'last' is the etag of this client's last response, 'last_strong' it without W/.
        let client = step["client"].as_str().unwrap_or("main").to_string();
        let last = etags.get(&client).cloned().unwrap_or_default();
        let tag = match step["if_none_match"].as_str() { Some("last") => Some(last), Some("last_strong") => Some(last.trim_start_matches("W/").to_string()), other => other.map(str::to_string) };
        if let Some(tag) = tag.as_deref() { headers.push(("if-none-match", tag)); }
        let wanted = match step["csrf"].as_str() { Some("form") => Csrf::Form, Some("header") => Csrf::Header, Some("both") => Csrf::Both, _ => Csrf::None };
        // Rails' last page had the form or meta tag this step takes its token from (pages.rb stops
        // otherwise). When this crate's page has not, that page already differs from Rails' and is
        // reported as such; the step is sent without the token so the rest can still be compared.
        let page = browser.page.clone().unwrap_or_default();
        let has_form = web::form_token(&page, step["path"].as_str().unwrap().split('?').next().unwrap()).is_some();
        let has_meta = web::meta_token(&page).is_some();
        let csrf = match wanted {
            Csrf::Form if !has_form => Csrf::None,
            Csrf::Both if !has_form => if has_meta { Csrf::Header } else { Csrf::None },
            Csrf::Both | Csrf::Header if !has_meta => Csrf::None,
            other => other,
        };
        let snapshot = step["action_snapshot"] == true;
        // An API step has no signed-in browser to watch the streams with; such a step must write nothing (below).
        let watcher = browsers.get("main").and_then(|browser| browser.cookie.as_deref()).map(str::to_owned);
        if snapshot && observer.is_none() && watcher.is_some() {
            let cookie = watcher;
            observer = Some(ActionObserver::open(&app, cookie.as_deref(), &["user_1:bot_updates", "user_1:bot_1"]).await.unwrap_or_else(|e| panic!("action observer: {e}")));
        }
        // A step that names streams records what this crate sent on them, as pages.rb records Rails' payloads.
        let streams: Vec<&str> = step["broadcasts"].as_array().map(|s| s.iter().map(|v| v.as_str().unwrap()).collect()).unwrap_or_default();
        let mut listener = if streams.is_empty() { None } else {
            let cookie = browsers.get("main").and_then(|browser| browser.cookie.as_deref()).map(str::to_owned);
            Some(ActionObserver::open(&app, cookie.as_deref(), &streams).await.unwrap_or_else(|e| panic!("broadcast listener: {e}")))
        };
        let browser = browsers.entry(step["client"].as_str().unwrap_or("main").to_string()).or_default();
        let seen = key_server.received_requests().await.map_or(0, |requests| requests.len());
        let before = snapshot.then(|| action_rows(dir, &app.cipher).unwrap_or_else(|error| panic!("action rows before: {error}")));
        let woken = app.job_wakers().map_or(0, |jobs| jobs.pending().len());
        // `raw_body`: a body sent as it is, whatever the method (pages.rb hands it to Rails unparsed).
        let answer = match step["json"].as_str().or(step["raw_body"].as_str()) {
            Some(json) => browser.send_body(&app, step["method"].as_str().unwrap(), &request_path, Some(json.to_string()), csrf, &headers).await,
            None => browser.send(&app, step["method"].as_str().unwrap(), &request_path, form.as_deref(), csrf, &headers).await,
        };
        assert_genuine(&app, browser, &answer, now, &format!("{} step {index}", dir.file_name().unwrap().to_string_lossy()));
        etags.insert(client, answer.header("etag").unwrap_or_default().to_string());
        let mut response = rust_answer(&answer, step);
        if step["jobs"] == true { response["job_classes"] = job_classes(&app.job_wakers().map(|jobs| jobs.pending()).unwrap_or_default()[woken..]); }
        if step["settings_snapshot"] == true { response["settings_rows"] = settings_snapshot::rows(dir,&app.cipher); }
        let requests = venue_transcript(&key_server.received_requests().await.unwrap_or_default()[seen..]);
        if !requests.is_empty() { response["venue_requests"] = json!(requests); }
        if let Some(tables) = step.get("may_mutate") { response["may_mutate"] = tables.clone(); }
        if let Some(spec) = step["raw_rows"].as_object() { response["raw_rows"] = raw_rows(dir, spec, &step["raw_clock"]); }
        if let Some(keys) = step["session"].as_array() {
            let data = browser.cookie.as_deref().and_then(|cookie| session::open(&app.keys.session, cookie, now)).unwrap_or_default();
            response["session"] = keys.iter().filter_map(Value::as_str).map(|key| (key.to_string(), session_value(&data, key))).collect::<serde_json::Map<_, _>>().into();
        }
        if let Some(listener) = listener.as_mut() { response["broadcasts"] = json!(listener.drain(&app).await.unwrap_or_else(|e| panic!("broadcasts: {e}"))); }
        if let Some((rows_before, other_before)) = before {
            let (rows_after, other_after) = action_rows(dir, &app.cipher).unwrap_or_else(|error| panic!("action rows after: {error}"));
            let changed = rows_before != rows_after;
            let woke = tokio::time::timeout(std::time::Duration::ZERO, wake.notified()).await.is_ok();
            let broadcasts = match observer.as_mut() {
                Some(observer) => observer.drain(&app).await.unwrap_or_else(|e| panic!("action broadcasts: {e}")),
                None if !changed => vec![],
                None => panic!("missing action observer: a step that writes needs a signed-in browser to watch its broadcasts"),
            };
            assert_eq!(woke, changed, "wake must follow an action write: {step}");
            if !changed {
                assert!(broadcasts.is_empty(), "unchanged action woke engine or broadcast: {step}");
            }
            response["action_snapshot"] = json!(true);
            response["rows_before"] = rows_before;
            response["rows_after"] = rows_after;
            response["other_rows_before"] = other_before;
            response["other_rows_after"] = other_after;
        }
        responses.push(response);
    }
    drop(app);
    json!({ "responses": responses, "users": users(dir), "bots": rows(dir, "bots", &BOT_COLUMNS) })
}

/// pages_wizard.rb's key checks, answered by the key a step sends: Kraken's GetApiKeyInfo (`API-Key`) and Binance's
/// apiRestrictions (`X-MBX-APIKEY`).
async fn mount_key_checks(server: &wiremock::MockServer) {
    use wiremock::{matchers, Mock, ResponseTemplate};
    let required = json!(["query-funds", "query-open-trades", "query-closed-trades", "modify-trades", "close-trades", "query-ledger"]);
    let permission = json!(["query-funds", "query-open-trades", "query-closed-trades", "modify-trades", "close-trades", "withdraw-funds"]);
    for (key, body) in [("kraken-key", json!({"error": [], "result": {"permissions": required}})), ("kraken-correct", json!({"error": [], "result": {"permissions": required}})),
                        ("kraken-incorrect", json!({"error": ["EAPI:Invalid key"]})), ("kraken-permission", json!({"error": [], "result": {"permissions": permission}})),
                        ("kraken-failure", json!({"error": ["EService:Unavailable"]}))] {
        Mock::given(matchers::method("POST")).and(matchers::path("/api.kraken.com/0/private/GetApiKeyInfo")).and(matchers::header("API-Key", key))
            .respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(server).await;
    }
    let flags = |restricted: bool| json!({"ipRestrict": restricted, "enableFixApiTrade": false, "enableFixReadOnly": false, "enableFutures": false,
        "enableInternalTransfer": false, "enableMargin": false, "enablePortfolioMarginTrading": false, "enableReading": true, "enableSpotAndMarginTrading": true,
        "enableVanillaOptions": false, "enableWithdrawals": false, "permitsUniversalTransfer": false});
    for (key, status, body) in [("binance-key", 200, Some(flags(false))), ("binance-correct", 200, Some(flags(true))), ("binance-unrestricted", 200, Some(flags(false))),
                                ("binance-incorrect", 401, Some(json!({"code": -2015, "msg": "Invalid API-key, IP, or permissions for action."}))), ("binance-failure", 500, None)] {
        let answer = body.map_or_else(|| ResponseTemplate::new(status), |body| ResponseTemplate::new(status).set_body_json(body));
        Mock::given(matchers::method("GET")).and(matchers::path("/api.binance.com/sapi/v1/account/apiRestrictions")).and(matchers::header("X-MBX-APIKEY", key))
            .respond_with(answer).mount(server).await;
    }
}

/// The venue hosts this crate calls, each a path prefix on the one wiremock server: the server sees the requests in the
/// order they were made, and the first path segment names the host Rails' WebMock saw.
fn venue_hosts(uri: &str) -> deltabadger::venue::alpaca::Hosts {
    let at = |host: &str| format!("{uri}/{host}");
    deltabadger::venue::alpaca::Hosts { paper: at("paper-api.alpaca.markets"), live: at("api.alpaca.markets"), data: at("data.alpaca.markets") }
}

/// What reached the venue server, as pages.rb `venue_request` writes Rails' requests: method, host, path, and the
/// query and form params sorted, without a signature, timestamp or nonce; any other body as its text.
fn venue_transcript(requests: &[wiremock::Request]) -> Vec<Value> {
    const UNSIGNED: [&str; 3] = ["signature", "timestamp", "nonce"];
    requests.iter().map(|request| {
        let (host, path) = request.url.path().trim_start_matches('/').split_once('/').map_or(("", request.url.path()), |(host, rest)| (host, rest));
        let form = request.headers.get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
        let mut params: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
        if form { params.extend(form_urlencoded::parse(&request.body).into_owned()); }
        params.retain(|(name, _)| !UNSIGNED.contains(&name.as_str()));
        params.sort();
        let mut entry = json!({ "method": request.method.as_str(), "host": host, "path": format!("/{path}"), "params": params });
        if !form && !request.body.is_empty() { entry["body"] = json!(String::from_utf8_lossy(&request.body)); }
        entry
    }).collect()
}

/// pages.rb `raw_rows`: the named tables' rows as stored, JSON columns as their text, "[clock]" in the `raw_clock` columns.
fn raw_rows(dir: &Path, spec: &serde_json::Map<String, Value>, clock: &Value) -> Value {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let clock: Vec<&str> = clock.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    spec.iter().map(|(table, columns)| {
        let columns: Vec<&str> = columns.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let mut rows = read_rows(&c, table, &columns, false).unwrap_or_else(|error| panic!("raw rows of {table}: {error}"));
        for row in rows.as_array_mut().into_iter().flatten().filter_map(Value::as_object_mut) {
            for (name, value) in row.iter_mut() {
                if clock.contains(&format!("{table}.{name}").as_str()) && !value.is_null() { *value = json!("[clock]"); }
            }
        }
        (table.clone(), rows)
    }).collect::<serde_json::Map<_, _>>().into()
}

/// A Rails session key as this crate's session holds it, in the JSON Rails' cookie serializer gives it.
fn session_value(data: &session::SessionData, key: &str) -> Value {
    match key {
        "tracker_connect" => data.tracker_connect.map_or(Value::Null, |id| json!({ "exchange_id": id })),
        "auto_open_bot_wizard" => if data.auto_open_bot_wizard { json!(true) } else { Value::Null },
        "bot_config" => data.bot_config.clone().unwrap_or(Value::Null),
        other => panic!("session key {other} has no counterpart in this crate's session"),
    }
}

fn copy_scenario(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for file in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(file), to.join(file)).unwrap(); }
    for scope in ["de", "crypto", "crypto_de"] {
        if from.join(scope).is_dir() { copy_scenario(&from.join(scope), &to.join(scope)); }
    }
}

fn difference(name: &str, rails: &Value, rust: &Value) -> Option<String> {
    let theirs = rails["responses"].as_array()?;
    let ours = rust["responses"].as_array()?;
    for (index, (a, b)) in theirs.iter().zip(ours).enumerate() {
        if a["action_snapshot"] == true || b["action_snapshot"] == true || (name.starts_with("actions_") && index + 1 == theirs.len()) {
            if let Some(message) = action_snapshot_difference(&format!("{name} step {index} (Rails {}, Rust {})", a["status"], b["status"]), a, b) { return Some(message); }
        }
    }
    if rails["users"] != rust["users"] {
        return Some(format!("{name}: users rows differ\n  rails: {}\n  rust:  {}", rails["users"], rust["users"]));
    }
    if !name.starts_with("actions_") && rails["bots"] != rust["bots"] {
        return Some(format!("{name}: bots rows differ\n  rails: {}\n  rust:  {}", rails["bots"], rust["bots"]));
    }
    let (ours, theirs) = (rust["responses"].as_array().unwrap(), rails["responses"].as_array().unwrap());
    if ours.len() != theirs.len() {
        return Some(format!("{name}: Rails recorded {} responses, this crate gave {}", theirs.len(), ours.len()));
    }
    for (index, (a, b)) in theirs.iter().zip(ours).enumerate() {
        if a == b { continue; }
        if a.get("settings_rows") != b.get("settings_rows") { return Some(format!("{name} step {index}: settings rows differ\n  rails: {}\n  rust: {}",a["settings_rows"],b["settings_rows"])); }
        for key in ["venue_requests", "raw_rows", "session", "job_classes"] {
            if a.get(key) != b.get(key) { return Some(format!("{name} step {index}: {key} differ\n  rails: {}\n  rust:  {}", a[key], b[key])); }
        }
        if a.get("broadcasts") != b.get("broadcasts") { return Some(format!("{name} step {index}: broadcasts differ\n  rails: {}\n  rust:  {}",a["broadcasts"],b["broadcasts"])); }
        if a["status"] != b["status"] || a["headers"] != b["headers"] {
            return Some(format!("{name} step {index}:\n  rails: {} {}\n  rust:  {} {}", a["status"], a["headers"], b["status"], b["headers"]));
        }
        if a.get("exception") != b.get("exception") {
            return Some(format!("{name} step {index}: recorded Rails exception {:?}; Rust {:?}", a.get("exception"), b.get("exception")));
        }
        let lines = |body: &Value| match body {
            Value::Array(lines) => lines.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect::<Vec<_>>(),
            text => vec![text.as_str().unwrap_or_default().to_string()],
        };
        return Some(format!("{name} step {index}: {}", html::first_difference(&lines(&a["body"]), &lines(&b["body"])).unwrap_or_default()));
    }
    None
}

/// What a scenario states about Rails' own answers, so that "equal" cannot mean "equally wrong":
/// a step's `expect` is the status Rails must have answered, and `expect_users` the columns the first
/// `users` row must end with.
fn unmet_expectation(name: &str, scenario: &Value, rails: &Value) -> Option<String> {
    for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        if !step["expect"].is_null() && rails["responses"][index]["status"] != step["expect"] {
            return Some(format!("{name} step {index}: the scenario expects {}, Rails answered {}", step["expect"], rails["responses"][index]["status"]));
        }
    }
    for (column, value) in scenario["expect_users"].as_object().into_iter().flatten() {
        if rails["users"][0][column] != *value {
            return Some(format!("{name}: the scenario expects users.{column} = {value}, Rails left {}", rails["users"][0][column]));
        }
    }
    None
}

/// Builds the grid with Rails, records Rails' answers, and returns (rails_root, rust_root, scenario dirs).
fn recorded_grid() -> (tempfile::TempDir, tempfile::TempDir, Vec<PathBuf>) {
    let (scratch, rails_root, rust_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::rails(scratch.path(), "test", &["db:schema:load"]);
    common::rails(scratch.path(), "test", &["runner", "script/rust/pages.rb", "grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in &dirs { copy_scenario(dir, &rust_root.path().join(dir.file_name().unwrap())); } // before Rails writes to its copies
    common::rails(scratch.path(), "test", &["runner", "script/rust/pages.rb", "record", rails_root.path().to_str().unwrap()]);
    (rails_root, rust_root, dirs)
}

/// Scenarios whose LAST response this crate deliberately answers otherwise than Rails. Everything
/// before it, and the `users` rows, are compared as in any scenario; only the last response is
/// judged on its own terms:
/// - `unrouted_*`: Rails has no such route (404; in production its exceptions app then redirects to the
///   root). This crate answers 501 and names the request, the same answer as for any page it does not
///   serve. Rails' 404 here is the test environment's error page, made below the controllers, so no
///   field of it is this app's: the refusal is checked against its own contract, every field of it.
/// - `not_ported_*`: a page Rails serves (200) and this crate does not yet. 501, naming the request.
///   The status and the body are what the refusal replaces. Every header is compared with Rails',
///   except Rails' `Set-Cookie`: the page Rails rendered may have changed its session, and a refusal
///   changes nothing, so this crate must set none.
/// - `missing_*`: a record that is not this user's, looked up where Rails lets
///   ActiveRecord::RecordNotFound through. Rails answers 404; in the test environment the body is its
///   debugging page, in production its exceptions app redirects to the root. This crate answers 404
///   with public/404.html and the headers of a response made below the controllers.
/// - `stricter_*`: only the `Location` may differ. Rails sends the browser back to a referer on its
///   host whatever the scheme; this crate requires its own origin and otherwise goes to `/`.
///
/// Ruling R: these five legacy fixtures contain priced balances but no producer digest.
/// Compare the whole response after replacing only Rails' measured tracker SVG with
/// Rust's asserted unavailable SVG. No amount, other icon, header or row is masked.
fn unknown_origin_ring(name: &str, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    if !["bot_page_ring", "bots_empty_cash_shown", "bots_empty_with_holdings", "bots_list_ring", "bots_list_ring_with_cash"].contains(&name) { return None; }
    Some((|| {
        let mut expected=rails.clone();
        let responses=expected["responses"].as_array_mut().ok_or("missing responses")?;
        let actual=rust["responses"].as_array().ok_or("missing actual responses")?;
        if responses.len()!=actual.len(){return Err("response count changed".into());}
        let mut reviewed=0;
        for (left,right) in responses.iter_mut().zip(actual) {
            let Some(lines)=left["body"].as_array_mut() else { continue; };
            let Some(ours)=right["body"].as_array() else {continue;};
            let Some(anchor)=lines.iter().position(|v|v.as_str().is_some_and(|s|s.contains("data-tile=\"tracker\""))) else {continue;};
            let start=anchor+1;
            let end=start+lines[start..].iter().position(|v|v.as_str().is_some_and(|s|s.trim()=="</svg>" )).ok_or("missing Rails tracker svg end")?;
            let ours_anchor=ours.iter().position(|v|v.as_str().is_some_and(|s|s.contains("data-tile=\"tracker\""))).ok_or("missing Rust tracker anchor")?;
            let ours_start=ours_anchor+1;
            let ours_end=ours_start+ours[ours_start..].iter().position(|v|v.as_str().is_some_and(|s|s.trim()=="</svg>" )).ok_or("missing Rust tracker svg end")?;
            let original=lines[start..=end].iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
            let replacement=ours[ours_start..=ours_end].iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
            if !original.contains("stroke-dasharray=") || original.contains("stroke--icon") {return Err("Rails no longer presents the legacy balances as current: review R divergence".into());}
            if replacement.contains("stroke-dasharray=") || !replacement.contains("<circle class=\"stroke--icon\" cx=\"12\" cy=\"12\" fill=\"none\" r=\"9\" stroke-width=\"2\">") || replacement.matches("<circle").count()!=1 {return Err("Rust rendered a current legacy holding".into());}
            lines.splice(start..=end,ours[ours_start..=ours_end].iter().cloned());
            reviewed+=1;
        }
        if reviewed==0{return Err("R divergence exercised no tracker ring".into());}
        match difference(name,&expected,rust){Some(message)=>Err(message),None=>Ok(())}
    })())
}

/// ActiveRecord::RecordInvalid raised by a settings write (`update!` on a user the model rejects): Rails' test
/// environment answers 422 with its debugging page, production with public/422.html, which is this crate's answer,
/// made below the controllers. Rails' recorded rows and broadcasts are kept and compared as they are.
fn as_record_invalid(answer: &mut Value) {
    if answer["exception"]["class"] != "ActiveRecord::RecordInvalid" || answer["status"] != 422 { return; }
    let page = html::normalize(&String::from_utf8_lossy(deltabadger::web::assets::find("/422.html").expect("public/422.html is embedded").body));
    answer["body"] = json!(page);
    answer["headers"] = json!({
        "content-type": "text/html; charset=utf-8", "cache-control": "no-cache",
        "content-security-policy-report-only": masked_nonce(&deltabadger::web::headers::content_security_policy("n")).0,
    });
    answer.as_object_mut().unwrap().remove("exception");
}

/// A REST request whose JSON body Rails cannot parse (ActionDispatch::Http::Parameters::ParseError, raised where an
/// action first reads `params`): the test environment answers 400 with its debugging page; this crate answers 400 with
/// public/400.html, made below the controllers, as it does for every request Rails refuses before an action.
fn as_parse_error(answer: &mut Value) {
    if answer["exception"]["class"] != "ActionDispatch::Http::Parameters::ParseError" || answer["status"] != 400 { return; }
    let page = html::normalize(&String::from_utf8_lossy(deltabadger::web::assets::find("/400.html").expect("public/400.html is embedded").body));
    answer["body"] = json!(page);
    answer["headers"] = json!({
        "content-type": "text/html; charset=utf-8", "cache-control": "no-cache",
        "content-security-policy-report-only": masked_nonce(&deltabadger::web::headers::content_security_policy("n")).0,
    });
    answer.as_object_mut().unwrap().remove("exception");
}

/// A listed divergence of the `settings_pref_regenerate*` scenarios: the personal REST token and the personal
/// application's uid are random on either side. Each recorded step's revealed token must be the one live
/// token of its own side's rows, every token 64 hex digits, the uid and secret present; then they read as placeholders.
fn without_random_tokens(answer: &mut Value) -> Result<(), String> {
    for response in answer["responses"].as_array_mut().ok_or("no responses")? {
        let Some(rows) = response.get_mut("settings_rows") else { continue };
        let tokens = rows["oauth_access_tokens"].as_array_mut().ok_or("no oauth_access_tokens rows")?;
        let live: Vec<String> = tokens.iter().filter(|row| row["revoked_at"].is_null()).filter_map(|row| row["token"].as_str().map(str::to_string)).collect();
        for row in tokens.iter_mut() {
            let token = row["token"].as_str().unwrap_or_default();
            // Doorkeeper's generator: SecureRandom.urlsafe_base64(32).
            if token.len() != 43 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { return Err(format!("token {token:?} is not Doorkeeper's urlsafe base64")); }
            row["token"] = json!("[random]");
        }
        for row in rows["oauth_applications"].as_array_mut().ok_or("no oauth_applications rows")?.iter_mut().filter(|row| row["personal_access_token"] == json!(1)) {
            if row["uid"].as_str().is_none_or(str::is_empty) { return Err("personal application has no uid".into()); }
            row["uid"] = json!("[random]");
        }
        let [token] = live.as_slice() else { return Err(format!("{} live personal tokens", live.len())) };
        let mut shown = 0;
        for line in response["body"].as_array_mut().ok_or("the answer is not markup")? {
            let text = line.as_str().unwrap_or_default();
            shown += text.matches(token.as_str()).count();
            *line = json!(text.replace(token.as_str(), "[token]"));
        }
        if shown != 2 { return Err(format!("the live token is shown {shown} times, not twice")); }
    }
    Ok(())
}

fn listed_divergence(name: &str, scenario: &Value, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    let kind = ["unrouted_", "not_ported_api_", "not_ported_", "stricter_", "missing_"].into_iter().find(|prefix| name.starts_with(prefix))?;
    let responses = |v: &Value| v["responses"].as_array().unwrap().clone();
    let (mut theirs, mut ours) = (responses(rails), responses(rust));
    let (Some(rails_last), Some(rust_last)) = (theirs.pop(), ours.pop()) else { return Some(Err("no responses".into())) };
    if let Some(message) = difference(name, &json!({ "responses": theirs, "users": rails["users"], "bots": rails["bots"] }), &json!({ "responses": ours, "users": rust["users"], "bots": rust["bots"] })) {
        return Some(Err(format!("before its last response: {message}")));
    }
    let step = scenario["steps"].as_array().unwrap().last().unwrap().clone();
    let method = step["form"]["_method"].as_str().map_or_else(|| step["method"].as_str().unwrap().to_string(), str::to_uppercase);
    let named = rust_last["body"].to_string().contains(&format!("{method} {}", step["path"].as_str().unwrap()));
    let without_location = |v: &Value| { let mut v = v.clone(); v["headers"].as_object_mut().unwrap().remove("location"); v };
    Some(match kind {
        "stricter_" if without_location(&rails_last) != without_location(&rust_last) => Err(format!("more than the Location differs:\n  rails: {rails_last}\n  rust:  {rust_last}")),
        "stricter_" if rust_last["headers"]["location"] != "/" || rails_last["headers"]["location"] == "/" => {
            Err(format!("expected Rails to go back to the referer and this crate to `/`: rails {}, rust {}", rails_last["headers"]["location"], rust_last["headers"]["location"]))
        }
        "stricter_" => Ok(()),
        "not_ported_api_" => {
            // The 501 envelope behind Rails' own authentication and tool gate (which the api_gated_ scenarios compare).
            let path = step["path"].as_str().unwrap().split('?').next().unwrap();
            let body = format!("{{\"data\":null,\"error\":{{\"code\":\"not_ported\",\"message\":\"{} {path} is not served by this build yet.\"}}}}", step["method"].as_str().unwrap());
            let mut headers = refusal_headers(false);
            headers["content-type"] = json!("application/json; charset=utf-8");
            let expected = json!({ "status": 501, "headers": headers, "body": body });
            let without_snapshot = |v: &Value| json!({ "status": v["status"], "headers": v["headers"], "body": v["body"] });
            if rails_last["status"] == 501 { Err("Rails now answers 501: drop or rename this scenario".into()) }
            else if without_snapshot(&rust_last) != expected { Err(format!("expected the not_ported envelope:\n  expected: {expected}\n  rust:     {rust_last}")) }
            else { Ok(()) }
        }
        "missing_" => {
            let page = html::normalize(&String::from_utf8_lossy(deltabadger::web::assets::find("/404.html").expect("public/404.html is embedded").body));
            let expected = json!({ "status": 404, "body": page, "headers": {
                "content-type": "text/html; charset=utf-8", "cache-control": "no-cache",
                "content-security-policy-report-only": masked_nonce(&deltabadger::web::headers::content_security_policy("n")).0,
            } });
            if rails_last["status"] != 404 { Err(format!("Rails now answers {}: drop or rename this scenario", rails_last["status"])) }
            else if rust_last != expected { Err(format!("expected 404 with public/404.html:\n  expected: {expected}\n  rust:     {rust_last}")) }
            else { Ok(()) }
        }
        _ if rails_last["status"] != if kind == "unrouted_" { 404 } else { 200 } => Err(format!("Rails now answers {}: drop or rename this scenario", rails_last["status"])),
        _ if rust_last["status"] != 501 || !named => Err(format!("expected 501 naming `{method} {}`, got {} {}", step["path"], rust_last["status"], rust_last["body"])),
        "not_ported_" => {
            let mut expected = rails_last["headers"].clone();
            expected.as_object_mut().unwrap().remove("set-cookie");
            if rust_last["headers"] == expected { Ok(()) } else { Err(format!("the refusal's headers are not the page's:\n  rails: {expected}\n  rust:  {}", rust_last["headers"])) }
        }
        _ => {
            let expected = refusal_headers(SIGNED_IN_REFUSALS.contains(&name));
            if rust_last["headers"] == expected { Ok(()) } else { Err(format!("the refusal's headers:\n  expected: {expected}\n  rust:     {}", rust_last["headers"])) }
        }
    })
}

/// The `unrouted_*` scenarios whose last request is made signed in.
const SIGNED_IN_REFUSALS: [&str; 2] = ["unrouted_get_logout", "unrouted_wizard_order_get"];

/// Every compared header of this crate's 501, and no other: an HTML page with the controllers'
/// default headers and the policy, never stored when the request was signed in, and no `Location`,
/// `Set-Cookie` or `Retry-After`.
fn refusal_headers(signed_in: bool) -> Value {
    json!({
        "content-type": "text/html; charset=utf-8",
        "cache-control": if signed_in { "no-store" } else { "no-cache" },
        "x-frame-options": "SAMEORIGIN",
        "x-xss-protection": "0",
        "x-content-type-options": "nosniff",
        "x-permitted-cross-domain-policies": "none",
        "referrer-policy": "strict-origin-when-cross-origin",
        "content-security-policy-report-only": masked_nonce(&deltabadger::web::headers::content_security_policy("n")).0,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_serve_the_same_pages_across_the_scenario_grid() {
    let (_rails_root, rust_root, dirs) = recorded_grid();
    assert!(!dirs.is_empty(), "the grid is empty: no scenario name starts with a prefix in PAGES");
    if std::env::var("PAGES").ok().as_deref() == Some("actions") {
        assert_eq!(dirs.len(), 247, "all action scenarios must remain selected");
    }
    let (mut failures, mut countdown_pages) = (vec![], 0);
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with("actions_") { assert!(dir.join("de/scenario.json").is_file(), "missing German variant: {name}"); }
        if ["actions_single_amount", "actions_single_start", "actions_start_missed_false", "actions_start_missed_true"].contains(&name.as_str()) {
            for scope in ["crypto", "crypto_de"] { assert!(dir.join(scope).join("scenario.json").is_file(), "missing {scope} variant: {name}"); }
        }
        for scope in ["", "de", "crypto", "crypto_de"].into_iter().filter(|scope| scope.is_empty() || dir.join(scope).is_dir()) {
        let dir = &dir.join(scope);
        let scenario = scenario(dir);
        let recorded: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
        let mut answers: Vec<Value> = recorded["responses"].as_array().unwrap().iter().zip(scenario["steps"].as_array().unwrap()).map(|(answer, step)| rails_answer(answer, step)).collect();
        let rails_countdowns: Vec<Vec<String>> = if name.starts_with("countdown_") { answers.iter_mut().map(|answer| without_countdown(&mut answer["body"])).collect() } else { vec![] };
        let rails = json!({ "responses": answers, "users": recorded["users"], "bots": recorded["bots"] });
        if let Some(message) = unmet_expectation(&name, &scenario, &rails) { failures.push(message); }
        // A page that reached the network in Rails was compared on whatever the network said: no scenario may.
        let network = recorded["network"].as_array().expect("pages.rb records the network calls a scenario made");
        if !network.is_empty() { failures.push(format!("{name}: Rails reached the network: {network:?}")); }
        // Rails must leave the bots as the grid built them (the copy this crate runs on was taken before Rails' requests).
        let built = rows(&rust_root.path().join(&name).join(scope), "bots", &BOT_COLUMNS);
        // A wizard's last step creates a bot (it declares `may_mutate`), and the rows it writes are compared step by step.
        let creates = scenario["steps"].as_array().unwrap().iter().any(|step| step["may_mutate"].as_array().is_some_and(|tables| tables.iter().any(|t| t == "bots")));
        if !name.starts_with("actions_") && !creates && recorded["bots"] != built { failures.push(format!("{name}: Rails changed a bot while it rendered:\n  before: {built}\n  after:  {}", recorded["bots"])); }
        let mut rust = run(&rust_root.path().join(&name).join(scope)).await;
        if name.starts_with("countdown_") {
            let ours: Vec<Vec<String>> = rust["responses"].as_array_mut().unwrap().iter_mut().map(|answer| without_countdown(&mut answer["body"])).collect();
            // Rails knows a time on every page of these scenarios, and this build must not claim the same one.
            for (index, (theirs, ours)) in rails_countdowns.iter().zip(&ours).enumerate().skip(2) {
                let end = |values: &Vec<String>| values.iter().find(|value| value.starts_with("data-countdown-end-time-value")).cloned();
                if end(theirs).is_none() || end(theirs) == end(ours) { failures.push(format!("{name} step {index}: expected Rails to know the time and this build not to: {theirs:?} / {ours:?}")); }
                countdown_pages += 1;
            }
        }
        let mut rails = rails;
        if name.starts_with("settings_") {
            for answer in rails["responses"].as_array_mut().unwrap() { as_record_invalid(answer); }
        }
        if name.starts_with("api_") {
            for answer in rails["responses"].as_array_mut().unwrap() { as_parse_error(answer); }
        }
        if name.starts_with("settings_pref_regenerate") {
            if let Err(message) = without_random_tokens(&mut rails).and_then(|()| without_random_tokens(&mut rust)) {
                failures.push(format!("{name} (listed divergence): {message}"));
                continue;
            }
        }
        match unknown_origin_ring(&name, &rails, &rust).or_else(||action_divergence(&name, &scenario, &rails, &rust)).or_else(||listed_divergence(&name, &scenario, &rails, &rust)) {
            Some(Ok(())) => {}
            Some(Err(message)) => failures.push(format!("{name} (listed divergence): {message}")),
            None => if let Some(message) = difference(&name, &rails, &rust) { failures.push(message); },
        }
    }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    println!("{} scenarios; {countdown_pages} pages where only Rails knows when the bot acts next", dirs.len());
    if std::env::var("PAGES").is_err() {
        assert_eq!(dirs.iter().filter(|dir| !dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("actions_") || n.to_string_lossy().starts_with("settings_"))).count(), 160 + 4 + 9 + 2 + 6 + 3 + 38 + 70, "completed read-only baseline (with the market-closed weekend page and the expired start on bots that are not stopped), with D5b's two missing_ and two not_ported_ connect scenarios, the venue pages (eight served, one refused), the selling pages (three served, one cap refused, in place of two refusals), the venue basket pages (seven served, in place of the one refusal), the asset search on each venue, the wizard's chooser and order switch (twelve served, one unrouted GET), its asset and exchange steps (ten served), its broker, key and quote steps to a created bot (nine served), the index and signal tiles and the IBKR and KuCoin key steps (four not_ported_), and the first empty bots page that opens it (two), and the REST API's (thirty-five on the five served reads' tokens and grants, fourteen on the routes it gates but does not serve, three on typed creates, nine unrouted paths, seven on parameters, one on conditional GETs and one on an install without an admin)");
        assert_eq!(dirs.iter().filter(|dir| dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("settings_"))).count(), 413 + 3 + 45 + 52 + 5 + 1 + 71, "complete S1 inventory, D5b's connect flow, the preference writes, the following client's card, the switch back to the default locale and the settings dialog (two more sections, in-place confirms, the page behind a language change)");
        assert_eq!(dirs.iter().filter(|dir| dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("actions_"))).count(), 197 + 50, "action inventory (with the five ⇄ rotations and nine sell-settings writes, twenty-two Kraken/Binance venue actions, and fifty on their baskets)");
        assert_eq!(dirs.len(), 160 + 4 + 9 + 2 + 6 + 3 + 38 + 70 + 197 + 50 + 413 + 3 + 45 + 52 + 5 + 1 + 71, "a scenario was dropped or added without this count");
        assert_eq!(countdown_pages, 4, "the pages where Rails knows a time this build does not: the listed divergence grew or shrank");
    }
}

// The response is deliberately identical: only a primary row is corrupted in each case.
fn assert_action_snapshot_corruption(table: &str, column: &str, changed: Value) {
    let snapshot = json!({
        "bots": [{"id": 1, "settings_changed_at": "2026-09-10 12:00:30.123456"}],
        "bot_index_assets": [{"id": 3, "target_allocation": 0.6}],
        "bot_activity_logs": [{"id": 7, "event": "started"}],
        "transactions": [], "api_keys": [], "users": []
    });
    let rails = json!({"responses": [{"status": 200, "headers": {}, "body": "same",
        "action_snapshot": true, "rows_before": snapshot, "rows_after": snapshot,
        "other_rows_before": {}, "other_rows_after": {}}]});
    let mut rust = rails.clone();
    rust["responses"][0]["rows_after"][table][0][column] = changed;
    let message = difference("actions_sensitivity", &rails, &rust)
        .unwrap_or_else(|| panic!("corruption of {table}.{column} passed"));
    assert!(message.contains("actions_sensitivity") && message.contains(table)
        && message.contains("id=") && message.contains(column), "{message}");
    assert_eq!(difference("actions_sensitivity", &rails, &rails), None);
}

#[test]
fn action_snapshot_missing_is_an_error() {
    let answer = json!({"responses": [{"status": 200, "headers": {}, "body": "same", "action_snapshot": true}]});
    let message = difference("actions_missing", &answer, &answer)
        .unwrap_or_else(|| panic!("missing snapshots passed"));
    assert!(message.contains("rows_before"), "{message}");
}

#[test]
fn action_snapshot_allocation() {
    assert_action_snapshot_corruption("bot_index_assets", "target_allocation", json!(0.4));
}

#[test]
fn action_snapshot_settings_timestamp() {
    assert_action_snapshot_corruption("bots", "settings_changed_at", json!("2026-09-10 12:00:30.123000"));
}

#[test]
fn action_snapshot_activity_event() {
    assert_action_snapshot_corruption("bot_activity_logs", "event", json!("stopped"));
}

#[test]
fn action_snapshot_sqlite_types() -> Result<(), Box<dyn std::error::Error>> {
    let c = rusqlite::Connection::open_in_memory()?;
    c.execute_batch("CREATE TABLE bots (id INTEGER, allocation REAL, settings TEXT, transient_data TEXT, label TEXT, stopped_at TEXT);
        INSERT INTO bots VALUES (1, 0.6, '{\"amount\":1,\"carry\":\"1\",\"null\":null,\"array\":[2,1]}', '{}', '{\"plain\":true}', NULL);")?;
    let actual = read_rows(&c, "bots", &[], true)?;
    assert_eq!(actual, json!([{"id": 1, "allocation": 0.6, "settings": {"amount": 1, "carry": "1", "null": null, "array": [2,1]},
        "transient_data": {}, "label": "{\"plain\":true}", "stopped_at": null}]));
    for value in [json!({"amount": "1", "carry": "1", "null": null, "array": [2,1]}),
        json!({"amount": 1, "carry": "1", "array": [2,1]}),
        json!({"amount": 1, "carry": "1", "null": null, "array": [1,2]})] {
        let mut changed = actual.clone();
        changed[0]["settings"] = value;
        assert!(row_difference("actions_types", &json!({"bots": actual}), &json!({"bots": changed})).is_some());
    }
    Ok(())
}

#[test]
fn action_snapshot_forbids_other_primary_writes() {
    let rows: serde_json::Map<String, Value> = ACTION_TABLES.into_iter().map(|table| (table.to_string(), json!([]))).collect();
    let mut response = json!({"rows_before": rows, "rows_after": rows,
        "other_rows_before": {"app_configs": [{"id": 1, "value": "old"}]},
        "other_rows_after": {"app_configs": [{"id": 1, "value": "new"}]}});
    let message = action_snapshot_difference("actions_forbidden", &response, &response)
        .unwrap_or_else(|| panic!("equal forbidden writes passed"));
    assert!(message.contains("forbidden write") && message.contains("app_configs") && message.contains("value"), "{message}");
    response["other_rows_after"] = response["other_rows_before"].clone();
    response["rows_after"]["transactions"] = json!([{"id": 1}]);
    let message = action_snapshot_difference("actions_forbidden", &response, &response)
        .unwrap_or_else(|| panic!("transaction insert passed"));
    assert!(message.contains("transactions") && message.contains("forbidden write"), "{message}");
}

#[test]
fn action_snapshot_scopes_writes_to_the_declared_tables() {
    let rows: serde_json::Map<String, Value> = ACTION_TABLES.into_iter().map(|table| (table.to_string(), json!([]))).collect();
    let mut response = json!({"may_mutate": ["api_keys"], "rows_before": rows, "rows_after": rows, "other_rows_before": {}, "other_rows_after": {}});
    response["rows_after"]["api_keys"] = json!([{"id": 1, "key": "placeholder-key"}]);
    assert_eq!(action_snapshot_difference("scoped", &response, &response), None, "a declared table may change");
    response["rows_after"]["bots"] = json!([{"id": 1}]);
    let message = action_snapshot_difference("scoped", &response, &response).unwrap_or_else(|| panic!("an undeclared bot insert passed"));
    assert!(message.contains("forbidden write") && message.contains("bots"), "{message}");
    response["rows_after"]["bots"] = json!([]);
    response.as_object_mut().unwrap().remove("may_mutate");
    let message = action_snapshot_difference("scoped", &response, &response).unwrap_or_else(|| panic!("a key write passed a bot action"));
    assert!(message.contains("forbidden write") && message.contains("api_keys"), "{message}");
}

#[test]
fn venue_transcripts_raw_rows_and_session_values_are_compared() {
    let account = json!({"method": "GET", "host": "paper-api.alpaca.markets", "path": "/v2/account", "params": []});
    let rails = json!({"responses": [{"status": 200, "headers": {}, "body": "same", "venue_requests": [account],
        "raw_rows": {"bots": [{"id": 1, "settings": "{\"a\":1.0}"}]}, "session": {"tracker_connect": {"exchange_id": 1}}, "job_classes": []}], "users": [], "bots": []});
    assert_eq!(difference("transcript", &rails, &rails), None);
    let changed = |key: &str, value: Value| { let mut rust = rails.clone(); rust["responses"][0][key] = value; rust };
    let extra = json!([account, {"method": "DELETE", "host": "paper-api.alpaca.markets", "path": "/v2/orders/1", "params": []}]);
    for (key, value) in [("venue_requests", extra), ("venue_requests", Value::Null), ("raw_rows", json!({"bots": [{"id": 1, "settings": "{\"a\":1}"}]})),
                         ("session", json!({"tracker_connect": null})), ("job_classes", json!(["rust:ledger_sync"])), ("job_classes", Value::Null)] {
        let message = difference("transcript", &rails, &changed(key, value.clone())).unwrap_or_else(|| panic!("{key} = {value} passed"));
        assert!(message.contains(key), "{message}");
    }
}

/// The explicitly different action contracts still assert both databases, including Rails' writes.
/// Only the named action response is adapted; authentication and every other fragment stay compared.
fn action_divergence(name: &str, scenario: &Value, rails: &Value, rust: &Value) -> Option<Result<(),String>> {
    if !name.starts_with("actions_") { return None }
    // Rust-only ownership bookkeeping is asserted by the lifecycle/placement protocol
    // tests. Keep every business field and response byte in this Rails comparison.
    let business = |source: &Value| {
        let mut value = source.clone();
        for response in value["responses"].as_array_mut().unwrap() {
            for key in ["rows_before", "rows_after"] {
                if response.get(key).is_some() {
                    response[key] = common::ownership_business_rows(response[key].take());
                }
            }
        }
        value
    };
    let rails = business(rails);
    let rust = business(rust);
    let (rails, rust) = (&rails, &rust);
    Some((|| {
        if let Some(kind) = scenario.get("divergence") {
            if kind != "crypto_actions" { return Err(format!("unknown divergence: {kind}")); }
        }
        let mut expected = rails.clone();
        let steps = scenario["steps"].as_array().ok_or("missing steps")?;
        let responses = expected["responses"].as_array_mut().ok_or("missing responses")?;
        let ours = rust["responses"].as_array().ok_or("missing Rust responses")?;
        if responses.len()!=ours.len() { return Err("response count differs".into()) }
        for (index,(a,b)) in responses.iter_mut().zip(ours).enumerate() {
            let step = steps.get(index).ok_or("missing step")?;
            if step["action_snapshot"] != true { continue }
            for (side,value) in [("Rails",&*a),("Rust",b)] {
                if let Some(error)=action_snapshot_difference(side,value,value) { return Err(error) }
            }
            if let Some(error)=row_difference("before action",&a["rows_before"],&b["rows_before"]) { return Err(error) }
            let path=step["path"].as_str().ok_or("missing action path")?;
            let locale=if path.starts_with("/de/") {"de"}else{"en"};
            let n=name.strip_prefix("actions_").ok_or("action name")?;
            if scenario["divergence"] == "crypto_actions" {
                if a["status"] != 200 || b["status"] != 200 { return Err(format!("crypto action expected 200: {} / {}", a["status"], b["status"])); }
                if path.contains("start_fresh=false") {
                    let request = json!({"requested_at":"2026-09-10T12:00:30.123456Z", "was_stopped":true});
                    let bot = b["rows_after"]["bots"].as_array().and_then(|rows| rows.iter().find(|row| row["id"] == 1)).ok_or("crypto bot")?;
                    if bot["transient_data"]["rust_continue_start"] != request { return Err("continue request payload differs".into()); }
                    let expected_bot = a["rows_after"]["bots"].as_array_mut().and_then(|rows| rows.iter_mut().find(|row| row["id"] == 1)).ok_or("Rails crypto bot")?;
                    if expected_bot["transient_data"].get("rust_continue_start").is_some() { return Err("Rails unexpectedly has engine request".into()); }
                    expected_bot["transient_data"]["rust_continue_start"] = request;
                }
                continue;
            }
            if n=="start_no_key" {
                // #507 closes this former divergence: require real 422 parity and no writes on either side.
                for (side,response) in [("Rails",&*a),("Rust",b)] {
                    if response["status"]!=422 { return Err(format!("{side} key validation must return 422")); }
                    if let Some(error)=row_difference(side,&response["rows_before"],&response["rows_after"]) { return Err(error); }
                }
            }
            if let Some(venue)=n.strip_prefix("venue_basket_").and_then(|rest|rest.strip_suffix("_not_ported_reverse")) {
                // Rails flips a buying basket of several into selling. This crate commits the same rotation and answers the listed
                // ⇄ refresh (below), and the page it refreshes to is then refused: the engine does not sell a basket of several on
                // Kraken or Binance (eligibility::admission). Not ported until it does.
                let exchange=if venue=="kraken" {"Exchanges::Kraken"} else {"Exchanges::Binance"};
                if deltabadger::engine::eligibility::admission(Some(exchange),false,2,true).is_none() { return Err(format!("{n}: a selling basket of several is served on {venue} now: retire this case")) }
                if a["status"]!=200 || b["status"]!=200 { return Err(format!("{n}: expected 200 from both, got {} / {}",a["status"],b["status"])) }
                let flipped=a["rows_after"]["bots"].as_array().and_then(|rows|rows.iter().find(|row|row["id"]==1)).ok_or("Rails bot 1")?;
                if flipped["settings"]["direction"]!="selling" { return Err(format!("{n}: Rails no longer flips the basket into selling")) }
                if b["body"]!=json!(html::normalize(deltabadger::web::turbo::refresh())) { return Err(format!("{n}: expected a refresh, got {}",b["body"])) }
                a["body"]=b["body"].clone();
                continue;
            }
            if let Some(rotation)=n.strip_prefix("reverse_").or_else(|| n.strip_prefix("venue_kraken_reverse_")).or_else(|| n.strip_prefix("venue_binance_reverse_")) {
                // Listed divergence (plan 6a, Revision r4): Rails answers ⇄ with the settings, Start/Stop and metrics panels
                // (bots/reverse.turbo_stream.erb). The metrics panel is drawn from figures this build does not compute for a
                // request, so it commits the same rotation and answers a refresh, which redraws the bot page it now serves. The
                // request calls no venue: a resting order's cancel is left to the engine (`rust_reversal_cancels`), where Rails
                // sends it inline. Every other row, the status and the headers stay Rails'.
                if a["status"]!=200 || b["status"]!=200 { return Err(format!("{n}: expected 200 from both, got {} / {}",a["status"],b["status"])) }
                if b["body"]!=json!(html::normalize(deltabadger::web::turbo::refresh())) { return Err(format!("{n}: expected a refresh, got {}",b["body"])) }
                if rotation=="scheduled" {
                    // Rails enqueues Bot::ActionJob at next_interval_checkpoint_at from the new start. Live, its clock has moved past
                    // that start, so the job lands one interval on (hourly here); the recorder's frozen clock puts it at the start
                    // itself. This crate holds the live answer in its own deferral, which the queue Rails' job sits in is not compared with.
                    let bot=b["rows_after"]["bots"].as_array().and_then(|rows|rows.iter().find(|row|row["id"]==1)).ok_or("Rust bot 1")?;
                    let deferral=&bot["transient_data"]["rust_defer_until"];
                    // One interval of each bot: hourly (Alpaca's coin), a day (Kraken), a week (Binance).
                    let until=if n.starts_with("venue_kraken_") {"2026-09-11T12:00:30.123456Z"} else if n.starts_with("venue_binance_") {"2026-09-17T12:00:30.123456Z"} else {"2026-09-10T13:00:30.123456Z"};
                    if deferral["until"]!=until || deferral["origin"]!="local" { return Err(format!("{n}: the next checkpoint is not one interval after the flip: {deferral}")) }
                    let expected=a["rows_after"]["bots"].as_array_mut().and_then(|rows|rows.iter_mut().find(|row|row["id"]==1)).ok_or("Rails bot 1")?;
                    expected["transient_data"]["rust_defer_until"]=deferral.clone();
                }
                if rotation=="waiting_buy" {
                    let bot=a["rows_after"]["bots"].as_array_mut().and_then(|rows|rows.iter_mut().find(|row|row["id"]==1)).ok_or("Rails bot 1")?;
                    if bot["transient_data"]["flip_reserve_pending"]!=json!([1]) { return Err("Rails no longer leaves the resting buy's reserve to its final poll".into()) }
                    bot["transient_data"][deltabadger::engine::reversal::CANCELS]=json!([{"id":1,"registered":true,"sent":false}]);
                    // Rails cancels and re-reads the order inline; this crate sends nothing from the request.
                    let order=|method:&str| json!({"method":method,"host":"paper-api.alpaca.markets","path":"/v2/orders/order-1-BTC-2026-09-10T11:00:00Z","params":[]});
                    if a.get("venue_requests")!=Some(&json!([order("DELETE"),order("GET")])) || b.get("venue_requests").is_some() {
                        return Err(format!("{n}: expected Rails' inline cancel and no request from this crate: {:?} / {:?}",a.get("venue_requests"),b.get("venue_requests")))
                    }
                    a.as_object_mut().ok_or("response object")?.remove("venue_requests");
                }
                a["body"]=b["body"].clone();
                continue;
            }
            let start_guard=matches!(n,"single_start"|"basket_start"|"index_start"|"start_created"|"start_stopped"|"start_missing"|"start_future"|"start_missed_false"|"start_missed_true"|"extra_paid_start_start_fresh_false"|"extra_paid_start_start_fresh_true"|"extra_defaults_start_start_fresh_true"|"extra_hour"|"extra_start_0"|"extra_start_1"|"extra_start_TRUE"|"extra_start_false"
                |"index_history_start_false"|"index_history_start_true"|"basket_sold_start_false"|"basket_sold_start_true"
                |"venue_basket_kraken_start_capped"|"venue_basket_binance_start_capped");
            // Their histories pass the page and the engine; the guard refuses only what refuses every stock bot of this grid, whose
            // listings carry no instrument type.
            if n.starts_with("index_history_start") || n.starts_with("basket_sold_start") {
                let body=b["body"].to_string();
                if !body.contains("asset category Stock / instrument") || body.contains("order(s)") || body.contains("row(s)") {
                    return Err(format!("{n}: the guard refused something other than the grid's stock listings: {body}"));
                }
            }
            let working_start=matches!(n,"start_scheduled"|"start_executing"|"start_retrying"|"start_waiting");
            let working_settings=matches!(n,"working_amount"|"working_limit"|"working_smart");
            let html=matches!(n,"extra_html_update"|"extra_html_start_start_fresh_true"|"extra_html_stop"|"extra_html_archive"|"venue_basket_kraken_html"|"venue_basket_binance_html");
            let scope=matches!(n,"start_invalid"|"modal_within_modal"|"start_within_true"|"start_within_false"|"update_exchange_ibkr");
            let safety=matches!(n,"stop_invalid"|"archive_invalid"|"stop_archived");
            // No flag starts fresh (start_missing is an ordinary guarded start); only a junk flag is refused as such.
            // A junk flag is refused before the row is read, so a working bot answers it too (start_junk_working).
            let flag=matches!(n,"start_junk"|"start_junk_working");
            let empty=n=="extra_empty_root";
            if start_guard || working_start || working_settings || html || scope || safety || flag || empty {
                let rails_status=if html {406}
                    else if empty {400} else if working_start || flag || matches!(n,"start_no_key"|"start_invalid"|"start_within_true"|"start_within_false"|"archive_invalid") {422} else {200};
                let rust_status=if empty {400} else if html {406} else if scope {501} else if safety {200} else {422};
                if a["status"]!=rails_status || b["status"]!=rust_status { return Err(format!("{n}: expected Rails {rails_status}, Rust {rust_status}; got {} / {}",a["status"],b["status"])) }
                // HTML update, stop and archive are refused before they write; HTML start still commits first.
                // Rails' stop skips settings validation, so an invalid row stops as any other does.
                let rails_rows=expected_action_rows(if n=="stop_invalid" {"extra_html_stop"} else {n},step,&a["rows_before"],rails_status==200 || n=="extra_html_start_start_fresh_true")?;
                if let Some(error)=row_difference("Rails measured action",&rails_rows,&a["rows_after"]) { return Err(error) }
                let rust_rows=if safety { expected_action_rows(if n=="archive_invalid" {"extra_html_archive"}else{"extra_html_stop"},step,&b["rows_before"],true)? } else { b["rows_before"].clone() };
                if let Some(error)=row_difference("Rust fixed action contract",&rust_rows,&b["rows_after"]) { return Err(error) }
                let mut headers=refusal_headers(true);
                if html { headers.as_object_mut().ok_or("headers")?.remove("content-type"); }
                else if !scope { headers["content-type"]=json!("text/vnd.turbo-stream.html; charset=utf-8"); }
                if b["headers"]!=headers { return Err(format!("{n}: Rust action headers differ: {} / {headers}",b["headers"])) }
                let body=b["body"].to_string();
                if html {
                    if !body.contains("Not Acceptable") && b["body"]!=json!("") && b["body"]!=json!([]) { return Err(format!("unexpected 406 body: {body}")) }
                } else if scope {
                    if !body.contains(path) { return Err("scope refusal does not name the request".into()) }
                } else if safety {
                    if n=="stop_invalid" && !body.contains("refresh") { return Err("unrenderable safety write must refresh".into()) }
                    if n=="archive_invalid" && stream_targets(b)!=vec!["status_bar_bots_dca_multi_asset_1","progress_bar_bots_dca_multi_asset_1","status_button_bots_dca_multi_asset_1","tile_status_button_bots_dca_multi_asset_1","menu_bots_dca_multi_asset_1","bot-count"] { return Err("safety Archive fragments differ".into()) }
                    if n=="stop_archived" && stream_targets(b)!=vec!["settings","exchange_select","status_button_bots_dca_multi_asset_1","tile_status_button_bots_dca_multi_asset_1","status_button_bots_dca_multi_asset_2","tile_status_button_bots_dca_multi_asset_2"] { return Err("archived Stop fragments differ".into()) }
                } else {
                    // A refused start on a working bot re-renders its panel, whichever refusal it is.
                    let six=working_start || n=="start_junk_working" || working_settings || empty;
                    let targets=stream_targets(b);
                    let wanted=if six { vec!["label_bots_dca_multi_asset_1","settings","exchange_select","status_bar_bots_dca_multi_asset_1","progress_bar_bots_dca_multi_asset_1","status_button_bots_dca_multi_asset_1","tile_status_button_bots_dca_multi_asset_1","flash"] } else {vec!["flash"]};
                    if targets!=wanted { return Err(format!("{n}: stream targets {targets:?}, expected {wanted:?}")) }
                    let key=if working_start {"engine.already_running"} else if flag {"engine.invalid_start_fresh"} else {"engine.write_refused"};
                    if start_guard || working_start || working_settings || flag {
                        let text=deltabadger::web::i18n::text(locale,key,&[("reason",deltabadger::web::i18n::Arg::Text("[reason]"))]);
                        let needle=text.split("[reason]").next().ok_or("translation")?;
                        if !body.contains(needle) { return Err(format!("{n}: missing {key}: {body}")) }
                    }
                    if working_settings {
                        // The five shared fragments remain exact even though the guard adds a flash.
                        if without_flash(&a["body"])!=without_flash(&b["body"]) { return Err(format!("{n}: a non-flash fragment differs")) }
                    }
                }
                // These are the exact differing fields after asserting their individual contracts.
                for key in ["status","headers","body","rows_after","exception"] {
                    if let Some(value)=b.get(key) { a[key]=value.clone(); } else { a.as_object_mut().ok_or("response object")?.remove(key); }
                }
            } else if matches!(n,"single_label"|"basket_label"|"index_label") {
                // Rails trusts its label as stream HTML. Compare the entire remainder, and require
                // the Rust label to be a single escaped text node with the exact submitted text.
                let mut raw=a["body"].as_array().ok_or("Rails HTML")?.clone();
                let replacement=b["body"].as_array().ok_or("Rust HTML")?;
                let end=raw.iter().position(|v|v.as_str().is_some_and(|s|s.trim()=="</turbo-stream>")).ok_or("label stream")?;
                let rust_end=replacement.iter().position(|v|v.as_str().is_some_and(|s|s.trim()=="</turbo-stream>")).ok_or("Rust label stream")?;
                if !raw[..end].iter().any(|v|v.as_str().is_some_and(|s|s.trim()=="<b>")) { return Err("Rails label is no longer unescaped: retire divergence".into()) }
                if replacement[..rust_end].iter().any(|v|v.as_str().is_some_and(|s|s.trim()=="<b>")) || !b["body"].to_string().contains("A <b>&") { return Err("unsafe or changed label".into()) }
                raw.splice(..=end,replacement[..=rust_end].iter().cloned());
                a["body"]=json!(raw);
            }
        }
        difference(name,&expected,rust).map_or(Ok(()),Err)
    })())
}

fn stream_targets(response: &Value) -> Vec<&str> {
    response["body"].as_array().into_iter().flatten().filter_map(|v| {
        let s=v.as_str()?.trim();
        if !s.starts_with("<turbo-stream ") { return None }
        s.split("target=\"").nth(1)?.split('"').next()
    }).collect()
}
fn without_flash(body: &Value) -> Value {
    let mut out=vec![];
    let mut flash=false;
    for line in body.as_array().into_iter().flatten() {
        let text=line.as_str().unwrap_or("");
        if text.contains("<turbo-stream action=\"prepend\" target=\"flash\">") { flash=true; }
        if !flash { out.push(line.clone()); }
        if flash && text.trim()=="</turbo-stream>" { flash=false; }
    }
    json!(out)
}

fn expected_action_rows(name: &str, step: &Value, before: &Value, writes: bool) -> Result<Value,String> {
    let mut rows=before.clone();
    if !writes { return Ok(rows) }
    if matches!(name,"modal_within_modal"|"stop_archived") { return Ok(rows) }
    let at="2026-09-10 12:00:30.123456";
    let bots=rows["bots"].as_array_mut().ok_or("bots snapshot")?;
    let bot=bots.iter_mut().find(|r|r["id"]==1).ok_or("bot 1")?;
    let mut event=None;
    if name=="update_exchange_ibkr" {
        bot["exchange_id"]=json!(2); bot["updated_at"]=json!(at); bot["settings_changed_at"]=json!(at);
        for key in ["price_limit_in_ticker_id","price_drop_limit_in_ticker_id","moving_average_limit_in_ticker_id","indicator_limit_in_ticker_id","sell_price_limit_in_ticker_id","sell_price_drop_limit_in_ticker_id","sell_moving_average_limit_in_ticker_id","sell_indicator_limit_in_ticker_id"] {bot["settings"][key]=json!(4);}
        for member in rows["bot_index_assets"].as_array_mut().ok_or("members")? {
            if member["bot_id"]==1 { member["ticker_id"]=json!(if member["asset_id"]==2 {2}else{4});member["updated_at"]=json!(at); }
        }
        return Ok(rows)
    }
    if name.starts_with("working_") || name=="extra_html_update" {
        if name=="extra_html_update" { bot["label"]=json!("Renamed"); }
        else {
            let form=step["form"].as_object().ok_or("working form")?;
            for (key,value) in form {
                let field=key.strip_prefix("bots_dca_multi_asset[").and_then(|s|s.strip_suffix(']')).ok_or("working field")?;
                let text=value.as_str().ok_or("working value")?;
                bot["settings"][field]=match field {
                    "limit_ordered"|"smart_intervaled" => json!(text=="1"),
                    "limit_order_pcnt_distance" => json!(0.04),
                    _ => json!(text.parse::<f64>().map_err(|e|e.to_string())?),
                };
            }
            bot["settings_changed_at"]=json!(at);
            bot["transient_data"]["missed_quote_amount"]=if name=="working_smart" {json!(10.0)}else{json!(5)};
            bot["transient_data"]["missed_quote_amount_was_set"]=Value::Null;
        }
        bot["updated_at"]=json!(at);
    } else if matches!(name,"extra_html_stop"|"extra_html_archive") {
        bot["status"]=json!(if name=="extra_html_archive" {7}else{2});bot["stopped_at"]=json!(at);bot["updated_at"]=json!(at);bot["stop_message_key"]=Value::Null;
        event=Some(("stopped",json!({})));
    } else {
        let path=step["path"].as_str().ok_or("start path")?;
        let fresh=!(path.ends_with("=false") || path.ends_with("=0"));
        bot["status"]=json!(1);bot["updated_at"]=json!(at);bot["stop_message_key"]=Value::Null;
        if fresh {
            bot["started_at"]=json!(match name {"start_future"=>"2026-09-12 10:00:00","extra_hour"=>"2026-09-11 10:00:00",_=>at});
            if bot["transient_data"].get("last_action_job_at").is_some() { bot["transient_data"]["last_action_job_at"]=Value::Null; }
            bot["transient_data"]["missed_quote_amount"]=Value::Null;
        }
        if name=="extra_hour" {
            bot["settings"]["start_at"]=json!("2026-09-11T10:00:00Z");bot["settings_changed_at"]=json!(at);
            bot["transient_data"]["missed_quote_amount"]=json!(0);
            bot["transient_data"]["missed_quote_amount_was_set"]=Value::Null;
        }
        if name=="extra_defaults_start_start_fresh_true" {
            for (key,value) in [("smart_interval_quote_amount",json!(10.0)),("limit_ordered",json!(false)),("quote_amount_limit",json!(1000)),("price_limit",json!(1000000))] { bot["settings"][key]=value; }
        }
        event=Some(("started",json!({"start_fresh":fresh})));
    }
    if let Some((event,details))=event {
        let logs=rows["bot_activity_logs"].as_array_mut().ok_or("logs")?;
        let id=logs.iter().filter_map(|r|r["id"].as_i64()).max().unwrap_or(0)+1;
        logs.push(json!({"id":id,"bot_id":1,"event":event,"level":0,"message":null,"details":details,"created_at":at}));
    }
    Ok(rows)
}

#[test]
fn action_unknown_divergence_is_rejected() {
    let scenario = json!({"divergence": "typo", "steps": []});
    let answer = json!({"responses": [], "users": [], "bots": []});
    assert!(matches!(action_divergence("actions_single_start", &scenario, &answer, &answer),
        Some(Err(message)) if message.contains("unknown divergence")));
}

// Subscribe to the streams, then use a marker to collect broadcasts without a timing sleep.
struct ActionObserver {
    /// The streams subscribed to; the first carries the marker that ends a drain.
    streams: Vec<String>,
    socket: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    _server: ActionServer,
}
struct ActionServer(tokio::task::JoinHandle<()>);
impl Drop for ActionServer { fn drop(&mut self) { self.0.abort(); } }
impl ActionObserver {
    async fn open(app: &App, cookie: Option<&str>, streams: &[&str]) -> Result<Self, Box<dyn std::error::Error>> {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let served = app.clone();
        let server = ActionServer(tokio::spawn(async move { let _ = deltabadger::web::server::serve_on(listener, served, Default::default()).await; }));
        let mut request = format!("ws://{address}/cable").into_client_request()?;
        request.headers_mut().insert("origin", format!("http://{address}").parse()?);
        request.headers_mut().insert("sec-websocket-protocol", "actioncable-v1-json".parse()?);
        request.headers_mut().insert("cookie", format!("_deltabadger_rust_session={}", cookie.ok_or("signed-in observer cookie")?).parse()?);
        let socket = tokio::time::timeout(std::time::Duration::from_secs(5), tokio_tungstenite::connect_async(request)).await??.0;
        let mut observer = Self { streams: streams.iter().map(|s| s.to_string()).collect(), socket, _server: server };
        for stream in streams {
            let identifier = json!({"channel":"Turbo::StreamsChannel", "signed_stream_name":cable::signed_stream_name(&app.keys.streams, stream)}).to_string();
            observer.socket.send(Message::Text(json!({"command":"subscribe", "identifier":identifier}).to_string().into())).await?;
            observer.receive(true).await?;
        }
        Ok(observer)
    }
    /// The payloads received until the subscription is confirmed or the marker arrives.
    async fn receive(&mut self, subscribing: bool) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        use futures_util::StreamExt;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut payloads = vec![];
            while let Some(message) = self.socket.next().await {
                let value: Value = serde_json::from_str(message?.to_text()?)?;
                if subscribing && value["type"] == "confirm_subscription" || value["message"] == "parity-marker" { return Ok(payloads); }
                if let Some(payload) = value["message"].as_str() { payloads.push(payload.to_string()); }
            }
            Err("action observer closed".into())
        }).await?
    }
    async fn drain(&mut self, app: &App) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        app.hub.broadcast(&self.streams[0], "parity-marker");
        self.receive(false).await
    }
}

#[path = "support/page_figures_grid.rs"]
mod page_figures_grid;

/// The Rails files the wizard's served steps port (chooser, order switch, asset and exchange steps): a change to one of them
/// re-records the wizard_ scenarios and updates rust/src/web/wizard in the same change.
#[test]
fn wizard_oracle_sources_stay_pinned() {
    use sha2::{Digest, Sha256};
    let pins: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("fixtures/wizard_sources.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in pins {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(&path)).unwrap())), hash, "{path}: record the wizard_ page grid again (script/rust/repin)");
    }
}

/// The Rails files the REST routes this build serves port (web::rest, web::bot_api, and the MCP list tools that format
/// bot_api's data): a change to one of them re-records the api_ page grid and updates the Rust side in the same change.
#[test]
fn api_oracle_sources_stay_pinned() {
    use sha2::{Digest, Sha256};
    let pins: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("fixtures/api_sources.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in pins {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(&path)).unwrap())), hash, "{path}: record the api_ page grid again (script/rust/repin)");
    }
}
