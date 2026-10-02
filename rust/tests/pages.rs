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
    let html_body = headers.get("content-type").is_some_and(|v| v[0].starts_with("text/html"));
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
    comparable(recorded["status"].as_u64().unwrap(), &headers, recorded["body"].as_str().unwrap())
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
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut statement = c.prepare(&format!("SELECT {} FROM users ORDER BY id", USER_COLUMNS.join(", "))).unwrap();
    let rows = statement.query_map([], |r| {
        Ok(Value::Object(USER_COLUMNS.iter().enumerate().map(|(i, name)| {
            let value = match r.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                other => json!(format!("{other:?}")),
            };
            Ok((name.to_string(), value))
        }).collect::<rusqlite::Result<_>>()?))
    }).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    Value::Array(rows)
}

/// Things that happen to the install between two requests, outside any browser (script/rust/pages.rb BEFORE).
fn before(dir: &Path, what: &str) {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    match what {
        "change_password" => c.execute("UPDATE users SET encrypted_password = ?1 WHERE id = (SELECT min(id) FROM users)", [deltabadger::crypto::hash_password("Another-horse-7")]).unwrap(),
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
        let answer = browser.send(&app, step["method"].as_str().unwrap(), step["path"].as_str().unwrap(), form.as_deref(), csrf, &headers).await;
        assert_genuine(&app, browser, &answer, now, &format!("{} step {index}", dir.file_name().unwrap().to_string_lossy()));
        responses.push(rust_answer(&answer));
    }
    drop(app);
    json!({ "responses": responses, "users": users(dir) })
}

fn copy_scenario(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for file in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(file), to.join(file)).unwrap(); }
}

fn difference(name: &str, rails: &Value, rust: &Value) -> Option<String> {
    if rails["users"] != rust["users"] {
        return Some(format!("{name}: users rows differ\n  rails: {}\n  rust:  {}", rails["users"], rust["users"]));
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
/// - `stricter_*`: only the `Location` may differ. Rails sends the browser back to a referer on its
///   host whatever the scheme; this crate requires its own origin and otherwise goes to `/`.
fn listed_divergence(name: &str, scenario: &Value, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    let kind = ["unrouted_", "not_ported_", "stricter_"].into_iter().find(|prefix| name.starts_with(prefix))?;
    let responses = |v: &Value| v["responses"].as_array().unwrap().clone();
    let (mut theirs, mut ours) = (responses(rails), responses(rust));
    let (Some(rails_last), Some(rust_last)) = (theirs.pop(), ours.pop()) else { return Some(Err("no responses".into())) };
    if let Some(message) = difference(name, &json!({ "responses": theirs, "users": rails["users"] }), &json!({ "responses": ours, "users": rust["users"] })) {
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
    let (mut failures, mut wizard_pages) = (vec![], 0);
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let scenario = scenario(dir);
        let recorded: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
        let mut answers: Vec<Value> = recorded["responses"].as_array().unwrap().iter().map(rails_answer).collect();
        wizard_pages += answers.iter_mut().filter_map(|answer| without_wizard(&mut answer["body"]).then_some(())).count();
        let rails = json!({ "responses": answers, "users": recorded["users"] });
        if let Some(message) = unmet_expectation(&name, &scenario, &rails) { failures.push(message); }
        let rust = run(&rust_root.path().join(&name)).await;
        match listed_divergence(&name, &scenario, &rails, &rust) {
            Some(Ok(())) => {}
            Some(Err(message)) => failures.push(format!("{name} (listed divergence): {message}")),
            None => if let Some(message) = difference(&name, &rails, &rust) { failures.push(message); },
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    println!("{} scenarios; Rails opened the wizard on {wizard_pages} pages", dirs.len());
}
