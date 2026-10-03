//! Page parity: the Rails app and this crate answer the same requests on identical installs with the
//! same frozen clock, and every response must be the same page (script/rust/pages.rb is the Rails half).
//! `PAGES=login,two_factor cargo test --test pages` runs only the scenarios with those name prefixes.
mod common;
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

/// One response as the comparison sees it: status, the listed headers, and the normalised body.
fn comparable(status: u64, headers: &BTreeMap<String, Vec<String>>, body: &str) -> Value {
    let mut out = BTreeMap::new();
    for name in HEADERS {
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

/// A listed divergence, to be removed by the plan that ports the wizard: on the first bots page after
/// a sign-in with no bots, Rails hides the page's chrome (`<body class="hide-chrome">`) and opens the
/// wizard in the modal frame (`src="...auto_open=true"`). This crate serves no wizard yet, and chrome
/// hidden behind a frame that cannot be closed would leave the user with an empty page, so it renders
/// that page as any later visit. Here Rails' page is brought to that form, after checking it has both
/// marks; the comparison that follows then holds this crate to having neither. Returns whether it applied.
fn without_wizard(body: &mut Value) -> bool {
    let Value::Array(lines) = body else { return false };
    let text = |line: &Value| line.as_str().unwrap_or_default().to_string();
    let Some(frame) = lines.iter().position(|l| text(l).trim_start().starts_with("<turbo-frame id=\"modal\" src=\"") && text(l).ends_with("/bots/dca_single_assets/pick_exchange/new?auto_open=true\">")) else { return false };
    let hidden = lines.iter().position(|l| text(l).trim_start().starts_with("<body class=\"hide-chrome")).expect("Rails hides the chrome whenever it opens the wizard");
    let indent = |line: &Value| text(line).chars().take_while(|c| *c == ' ').collect::<String>();
    lines[frame] = json!(format!("{}<turbo-frame id=\"modal\">", indent(&lines[frame])));
    lines[hidden] = json!(text(&lines[hidden]).replacen(" class=\"hide-chrome\"", "", 1).replacen("class=\"hide-chrome ", "class=\"", 1));
    true
}

/// A listed divergence of the `countdown_*` scenarios: when a bot acts next. Rails reads it from its
/// job table. This build has no job table and derives it from the bot's row (the next interval
/// checkpoint), which is the same instant after every completed tick; three times are in no row: when
/// a closed market opens, when a retry the engine has in hand fires, and the checkpoint Rails waits
/// for after a restart that was not a fresh start. There the countdown's end,
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
fn rails_answer(recorded: &Value) -> Value {
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
    let mut answer = comparable(status, &headers, body);
    for key in ["action_snapshot", "rows_before", "rows_after", "other_rows_before", "other_rows_after", "exception"] {
        if let Some(value) = recorded.get(key) { answer[key] = value.clone(); }
    }
    answer
}

fn rust_answer(answer: &Answer) -> Value {
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in &answer.headers { headers.entry(name.clone()).or_default().push(value.clone()); }
    comparable(answer.status.into(), &headers, &answer.body)
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
fn action_rows(dir: &Path) -> Result<(Value, Value), Box<dyn std::error::Error>> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3"))?;
    let mut statement = c.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let tables = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let (mut actions, mut others) = (serde_json::Map::new(), serde_json::Map::new());
    for table in tables {
        let value = read_rows(&c, &table, &[], true)?;
        if ACTION_TABLES.contains(&table.as_str()) { actions.insert(table, value); }
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
        let unchanged = |key: &str| -> Value {
            Value::Object(response[key].as_object().into_iter().flatten()
                .filter(|(table, _)| !ACTION_MUTATION_TABLES.contains(&table.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        };
        if let Some(message) = row_difference(&format!("{context} {side} forbidden write"), &unchanged("rows_before"), &unchanged("rows_after")) { return Some(message); }
        if let Some(message) = row_difference(&format!("{context} {side} forbidden write"), &response["other_rows_before"], &response["other_rows_after"]) { return Some(message); }
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
        other => panic!("unknown step.before {other}"),
    };
}

fn scenario(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap()
}

/// Runs one scenario's steps against this crate; returns what Rails' rails.json holds.
async fn run(dir: &Path) -> Value {
    let scenario = scenario(dir);
    assert_eq!(scenario["page_parity_scratch"], true, "{} is not a page-parity scratch copy", dir.display());
    let mut now: chrono::DateTime<chrono::Utc> = scenario["at"].as_str().unwrap().parse().unwrap();
    let clock = TestClock::at(scenario["at"].as_str().unwrap());
    let app = web::app(dir, scenario["secret_key_base"].as_str().unwrap(), clock.clone());
    let mut browsers: BTreeMap<String, Browser> = BTreeMap::new();
    let mut responses = vec![];
    for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        now += chrono::Duration::seconds(step["advance"].as_i64().unwrap_or(0));
        clock.set(now);
        if let Some(what) = step["before"].as_str() { before(dir, what); }
        let browser = browsers.entry(step["client"].as_str().unwrap_or("main").to_string()).or_default();
        let form: Option<Vec<(&str, &str)>> = step["form"].as_object().map(|f| f.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect());
        let headers: Vec<(&str, &str)> = step["headers"].as_object().map(|h| h.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect()).unwrap_or_default();
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
        let before = snapshot.then(|| action_rows(dir).unwrap_or_else(|error| panic!("action rows before: {error}")));
        let answer = browser.send(&app, step["method"].as_str().unwrap(), step["path"].as_str().unwrap(), form.as_deref(), csrf, &headers).await;
        assert_genuine(&app, browser, &answer, now, &format!("{} step {index}", dir.file_name().unwrap().to_string_lossy()));
        let mut response = rust_answer(&answer);
        if let Some((rows_before, other_before)) = before {
            let (rows_after, other_after) = action_rows(dir).unwrap_or_else(|error| panic!("action rows after: {error}"));
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

fn copy_scenario(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for file in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(file), to.join(file)).unwrap(); }
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
fn listed_divergence(name: &str, scenario: &Value, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    let kind = ["unrouted_", "not_ported_", "stricter_", "missing_"].into_iter().find(|prefix| name.starts_with(prefix))?;
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
const SIGNED_IN_REFUSALS: [&str; 1] = ["unrouted_get_logout"];

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
    let (mut failures, mut wizard_pages, mut countdown_pages) = (vec![], 0, 0);
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let scenario = scenario(dir);
        let recorded: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
        let mut answers: Vec<Value> = recorded["responses"].as_array().unwrap().iter().map(rails_answer).collect();
        wizard_pages += answers.iter_mut().filter_map(|answer| without_wizard(&mut answer["body"]).then_some(())).count();
        let rails_countdowns: Vec<Vec<String>> = if name.starts_with("countdown_") { answers.iter_mut().map(|answer| without_countdown(&mut answer["body"])).collect() } else { vec![] };
        let rails = json!({ "responses": answers, "users": recorded["users"], "bots": recorded["bots"] });
        if let Some(message) = unmet_expectation(&name, &scenario, &rails) { failures.push(message); }
        // A page that reached the network in Rails was compared on whatever the network said: no scenario may.
        let network = recorded["network"].as_array().expect("pages.rb records the network calls a scenario made");
        if !network.is_empty() { failures.push(format!("{name}: Rails reached the network: {network:?}")); }
        // Rails must leave the bots as the grid built them (the copy this crate runs on was taken before Rails' requests).
        let built = rows(&rust_root.path().join(&name), "bots", &BOT_COLUMNS);
        if !name.starts_with("actions_") && recorded["bots"] != built { failures.push(format!("{name}: Rails changed a bot while it rendered:\n  before: {built}\n  after:  {}", recorded["bots"])); }
        let mut rust = run(&rust_root.path().join(&name)).await;
        if name.starts_with("countdown_") {
            let ours: Vec<Vec<String>> = rust["responses"].as_array_mut().unwrap().iter_mut().map(|answer| without_countdown(&mut answer["body"])).collect();
            // Rails knows a time on every page of these scenarios, and this build must not claim the same one.
            for (index, (theirs, ours)) in rails_countdowns.iter().zip(&ours).enumerate().skip(2) {
                let end = |values: &Vec<String>| values.iter().find(|value| value.starts_with("data-countdown-end-time-value")).cloned();
                if end(theirs).is_none() || end(theirs) == end(ours) { failures.push(format!("{name} step {index}: expected Rails to know the time and this build not to: {theirs:?} / {ours:?}")); }
                countdown_pages += 1;
            }
        }
        match listed_divergence(&name, &scenario, &rails, &rust) {
            Some(Ok(())) => {}
            Some(Err(message)) => failures.push(format!("{name} (listed divergence): {message}")),
            None => if let Some(message) = difference(&name, &rails, &rust) { failures.push(message); },
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    println!("{} scenarios; Rails opened the wizard on {wizard_pages} pages; {countdown_pages} pages where only Rails knows when the bot acts next", dirs.len());
    if std::env::var("PAGES").is_err() {
        assert_eq!(dirs.len(), 149 + 145, "a scenario was dropped or added without this count");
        assert_eq!(countdown_pages, 6, "the pages where Rails knows a time this build does not: the listed divergence grew or shrank");
        assert_eq!(wizard_pages, 17, "the pages where Rails opens the wizard and this crate does not: the listed divergence grew or shrank");
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
