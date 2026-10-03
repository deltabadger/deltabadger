//! OAuth parity: the Rails app and this crate answer the same request sequences on identical installs
//! under the same clock, and every response and every row written must be the same
//! (script/rust/oauth.rb is the Rails half). A scenario with a `cut` is also played with the two
//! taking its legs in turns on one database: the switch to Rust and the handback to Rails.
//! `OAUTH=token,refresh cargo test --test oauth` runs only the scenarios with those name prefixes;
//! `OAUTH_KEEP=<an empty directory>` keeps every install and transcript there.
mod common;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use common::html;
use common::web::{self, Browser, TestClock};
use deltabadger::web::{bearer, csrf, oauth, session, App};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// Headers compared exactly, after masking. What else Rails sends (etag, x-request-id, x-runtime,
/// content-length, vary) is not part of an answer.
const HEADERS: [&str; 13] = ["location", "content-type", "cache-control", "pragma", "www-authenticate", "retry-after", "x-frame-options", "x-xss-protection",
                             "x-content-type-options", "x-permitted-cross-domain-policies", "referrer-policy", "content-security-policy-report-only", "set-cookie"];
/// The tables OAuth writes, compared whole (every column, rows by id).
const TABLES: [&str; 4] = ["oauth_applications", "oauth_access_grants", "oauth_access_tokens", "connected_clients"];
const COOKIE: &str = "_deltabadger_rust_session";

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

/// The legs of a scenario: a step with `cut` starts a new one.
fn legs(steps: &[Value]) -> Vec<Vec<Value>> {
    let mut legs: Vec<Vec<Value>> = vec![];
    for step in steps {
        if legs.is_empty() || step["cut"] == true { legs.push(vec![]); }
        legs.last_mut().unwrap().push(step.clone());
    }
    legs
}

/// Which runtime plays leg `leg` of an install of mode `mode` (script/rust/oauth.rb `runtime`).
fn runtime(mode: &str, leg: usize) -> &'static str {
    match mode {
        "rails" => "rails",
        "rust" => "rust",
        "rails_first" => if leg.is_multiple_of(2) { "rails" } else { "rust" },
        _ => if leg.is_multiple_of(2) { "rust" } else { "rails" },
    }
}

/// `$name` in any string of a step is a value an earlier response gave (script/rust/oauth.rb `fill`).
fn fill(value: &Value, vars: &Map<String, Value>) -> Value {
    match value {
        Value::String(text) => {
            let mut out = String::new();
            let mut rest = text.as_str();
            while let Some(at) = rest.find('$') {
                out.push_str(&rest[..at]);
                let name: String = rest[at + 1..].chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
                let mut end = at + 1 + name.len();
                let mut key = name.clone();
                if rest[end..].starts_with('.') {
                    let digits: String = rest[end + 1..].chars().take_while(char::is_ascii_digit).collect();
                    if !digits.is_empty() { key = format!("{name}.{digits}"); end += 1 + digits.len(); }
                }
                if name.is_empty() { out.push('$'); } else { out.push_str(vars.get(&key).and_then(Value::as_str).unwrap_or_else(|| panic!("no value for ${key} yet"))); }
                rest = &rest[end..];
            }
            out.push_str(rest);
            json!(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| fill(item, vars)).collect()),
        Value::Object(members) => Value::Object(members.iter().map(|(name, item)| (name.clone(), fill(item, vars))).collect()),
        other => other.clone(),
    }
}

/// Keeps a value a response gave under `name` (the latest) and `name.N` (the Nth of the scenario).
fn remember(vars: &mut Map<String, Value>, name: &str, value: Option<&str>) {
    let Some(value) = value.filter(|v| !v.trim().is_empty()) else { return };
    if vars.values().any(|known| known == value) { return; }
    let number = 1 + vars.keys().filter(|key| key.starts_with(&format!("{name}."))).count();
    vars.insert(format!("{name}.{number}"), json!(value));
    vars.insert(name.to_string(), json!(value));
}

/// `name=value` pairs of a JSON object, an array value giving one pair per element.
fn pairs(fields: &Value) -> Vec<(String, String)> {
    fields.as_object().into_iter().flatten().flat_map(|(name, value)| match value {
        Value::Array(all) => all.iter().map(|one| (name.clone(), text(one))).collect(),
        Value::Null => vec![],
        one => vec![(name.clone(), text(one))],
    }).collect()
}

fn text(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

fn encode(pairs: &[(String, String)]) -> String {
    let mut encoded = form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs { encoded.append_pair(name, value); }
    encoded.finish()
}

/// The fields a browser would send for the form the page has for `action` and `method`: its hidden
/// inputs, its ticked boxes and the button pressed (script/rust/oauth.rb `form_fields`).
fn form_fields(page: &str, action: &str, method: &str) -> Option<Vec<(String, String)>> {
    let document = scraper::Html::parse_document(page);
    let select = |css: &str| scraper::Selector::parse(css).unwrap();
    let form = document.select(&select("form")).find(|form| {
        let overridden = form.select(&select("input[name=\"_method\"]")).next().and_then(|input| input.value().attr("value"));
        form.value().attr("action") == Some(action) && overridden.or(form.value().attr("method")).unwrap_or("get").eq_ignore_ascii_case(method)
    })?;
    let mut inputs: Vec<scraper::ElementRef> = form.select(&select("input")).collect();
    let inside = form.select(&select("input[type=\"submit\"]")).next();
    let outside = form.value().attr("id").and_then(|id| document.select(&select(&format!("input[type=\"submit\"][form=\"{id}\"]"))).next());
    if let Some(button) = inside.or(outside) {
        if !inputs.iter().any(|input| input.id() == button.id()) { inputs.push(button); }
    }
    Some(inputs.iter().filter_map(|input| {
        let (name, kind) = (input.value().attr("name")?, input.value().attr("type").unwrap_or(""));
        (["hidden", "submit"].contains(&kind) || (kind == "checkbox" && input.value().attr("checked").is_some()))
            .then(|| (name.to_string(), input.value().attr("value").unwrap_or("").to_string()))
    }).collect())
}

/// Every CSRF token a page of this crate carries must unmask to the token of the session the
/// response left the browser with, and every asset it names must be embedded: the comparison masks
/// both. (script/rust/pages.rb `verify_rendered!` checks Rails' pages the same way.)
fn assert_genuine(app: &App, browser: &Browser, body: &str, now: chrono::DateTime<chrono::Utc>, step: &str) {
    let html::Masked { tokens, assets, .. } = html::masked_values(body);
    for path in &assets {
        assert!(deltabadger::web::assets::find(path).is_some(), "{step}: the page refers to {path}, which is not an embedded asset");
    }
    if tokens.is_empty() { return; }
    let token = browser.cookie.as_deref().and_then(|cookie| session::open(&app.keys.session, cookie, now)).and_then(|s| s.csrf)
        .unwrap_or_else(|| panic!("{step}: the page carries CSRF tokens but the session has none"));
    for rendered in &tokens {
        assert!(csrf::valid(&token, rendered), "{step}: a rendered CSRF token does not verify against the session");
    }
}

fn run_sql(dir: &Path, step: &Value) {
    let Some(statements) = step["sql"].as_array() else { return };
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    for sql in statements { c.execute_batch(sql.as_str().unwrap()).unwrap(); }
}

/// This crate plays leg `leg` of the install in `dir`: the Rust half of script/rust/oauth.rb `play_leg`.
async fn play_leg(dir: &Path, leg: usize) {
    let scenario = read(&dir.join("scenario.json"));
    assert_eq!(scenario["oauth_parity_scratch"], true, "{} is not an OAuth-parity scratch copy", dir.display());
    let Some(steps) = legs(scenario["steps"].as_array().unwrap()).into_iter().nth(leg) else { return };
    let mut state = read(&dir.join("state.json"));
    let mut transcript = read(&dir.join("transcript.json"));
    let started: chrono::DateTime<chrono::Utc> = scenario["at"].as_str().unwrap().parse().unwrap();
    let clock = TestClock::at(scenario["at"].as_str().unwrap());
    let app = web::app_allowing(dir, scenario["secret_key_base"].as_str().unwrap(), scenario["allowed_hosts"].as_str(), clock.clone());
    let all_steps = scenario["steps"].as_array().unwrap().clone();
    let mut browsers: BTreeMap<String, Browser> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate() {
        state["elapsed"] = json!(state["elapsed"].as_i64().unwrap() + step["advance"].as_i64().unwrap_or(0));
        let now = started + chrono::Duration::seconds(state["elapsed"].as_i64().unwrap());
        clock.set(now);
        let step = fill(step, state["vars"].as_object().unwrap());
        run_sql(dir, &step);
        let kind = step["do"].as_str().unwrap();
        let mut answer = match kind {
            "resolve" | "use" => {
                let (header, scope, retire) = (step["authorization"].as_str().map(str::to_string), step["scope"].as_str().unwrap().to_string(), kind == "use");
                let before = table(dir, "oauth_access_tokens");
                let presented = header.clone();
                let at_now = clock.clone(); // set to this step's time above
                let mcp_status = if retire {
                    let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"oauth-cross","version":"1"},"capabilities":{}}});
                    let mut request = Request::builder().method("POST").uri("/mcp").header("host",web::HOST)
                        .header("content-type","application/json").header("accept","application/json, text/event-stream");
                    if let Some(value)=header.as_deref(){request=request.header("authorization",value);}
                    Some(deltabadger::web::router(app.clone()).oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap().status().as_u16())
                } else { None };
                let resolved = app.db(move |c| bearer::resolve(c, header.as_deref(), &scope, &*at_now)).await.unwrap();
                if let Some(status)=mcp_status{assert_eq!(status,if resolved.is_ok(){200}else{401},"OAuth token must reach the real MCP endpoint");}
                let mut answer = json!({ "resolved": match resolved {
                    Ok(bearer) => json!({ "error": null, "user_id": bearer.user_id, "application_id": bearer.application_id, "token_id": bearer.token_id }),
                    Err(refusal) => json!({ "error": refusal.name(), "user_id": null, "application_id": null, "token_id": null }),
                } });
                let after = table(dir, "oauth_access_tokens");
                let at = format!("{} step {index}", dir.display());
                if retire {
                    let parents = parents(&all_steps, transcript.as_array().unwrap());
                    answer["retired"] = retirement(&at, &before, &after, bearer::bearer_token(presented.as_deref()), &parents, now);
                } else {
                    assert_eq!(before, after, "{at}: resolving a token is a read");
                }
                answer
            }
            "uri" => json!({ "allowed": oauth::redirect_uri_allowed(step["url"].as_str().unwrap(), step["registered"].as_str().unwrap()),
                             "host": oauth::Uri::parse(step["url"].as_str().unwrap()).and_then(|uri| uri.host) }),
            _ => requested(&app, &mut browsers, &step, state["vars"].as_object_mut().unwrap(), now, &format!("{} step {index}", dir.display())).await,
        };
        answer["by"] = json!("rust");
        transcript.as_array_mut().unwrap().push(answer);
    }
    drop(app);
    std::fs::write(dir.join("state.json"), state.to_string()).unwrap();
    std::fs::write(dir.join("transcript.json"), serde_json::to_string_pretty(&transcript).unwrap()).unwrap();
}

/// One HTTP step against this crate's router, in-process.
async fn requested(app: &App, browsers: &mut BTreeMap<String, Browser>, step: &Value, vars: &mut Map<String, Value>, now: chrono::DateTime<chrono::Utc>, at: &str) -> Value {
    let client = step["client"].as_str().unwrap_or("main").to_string();
    let page_of = step["page_of"].as_str().unwrap_or(&client).to_string();
    let page = browsers.entry(page_of).or_default().page.clone().unwrap_or_default();
    let browser = browsers.entry(client).or_default();
    let kind = step["do"].as_str().unwrap();
    let mut path = step["path"].as_str().or(step["action"].as_str()).unwrap().to_string();
    if step["query"].is_object() { path = format!("{path}?{}", encode(&pairs(&step["query"]))); }
    let mut headers: Vec<(String, String)> = pairs(&step["headers"]);
    let has = |headers: &[(String, String)], name: &str| headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
    let body = if kind == "submit" {
        let set = step["set"].as_object().unwrap();
        let mut fields = form_fields(&page, step["action"].as_str().unwrap(), step["method"].as_str().unwrap())
            .unwrap_or_else(|| panic!("{at}: the last page has no {} form for {}", step["method"], step["action"]));
        fields.retain(|(name, _)| !set.contains_key(name));
        fields.extend(pairs(&step["set"]));
        headers.push(("content-type".into(), "application/x-www-form-urlencoded".into()));
        Some(encode(&fields))
    } else if step["form"].is_object() {
        if !has(&headers, "content-type") { headers.push(("content-type".into(), "application/x-www-form-urlencoded".into())); }
        Some(encode(&pairs(&step["form"])))
    } else if !step["json"].is_null() {
        if !has(&headers, "content-type") { headers.push(("content-type".into(), "application/json".into())); }
        Some(step["json"].as_str().map_or_else(|| step["json"].to_string(), str::to_string))
    } else {
        None
    };
    for (name, value) in &mut headers {
        if let Some(plain) = value.strip_prefix("Basic-of ").filter(|_| name.eq_ignore_ascii_case("authorization")) { *value = format!("Basic {}", B64.encode(plain)); }
    }
    if step["csrf"] == "header" {
        headers.push(("x-csrf-token".into(), web::meta_token(&page).unwrap_or_else(|| panic!("{at}: the last page has no csrf-token meta tag"))));
    }
    let method = match kind { "get" => "GET", "head" => "HEAD", _ => "POST" };
    let mut request = Request::builder().method(method).uri(&path);
    if !has(&headers, "host") { request = request.header(header::HOST, web::HOST); }
    if let Some(cookie) = &browser.cookie { request = request.header(header::COOKIE, format!("{COOKIE}={cookie}")); }
    for (name, value) in &headers { request = request.header(name.as_str(), value.as_str()); }
    let mut request = request.body(body.map_or_else(Body::empty, Body::from)).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    let response = deltabadger::web::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let mut answered: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in response.headers() { answered.entry(name.as_str().to_string()).or_default().push(value.to_str().unwrap().to_string()); }
    let body = String::from_utf8_lossy(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned();
    // This crate writes its cookie exactly when the request changed the session.
    let cookie = answered.get("set-cookie").and_then(|all| all.iter().find_map(|value| value.strip_prefix(&format!("{COOKIE}="))));
    let session_changed = cookie.is_some();
    if let Some(cookie) = cookie { browser.cookie = Some(cookie.split(';').next().unwrap().to_string()); }
    if web::meta_token(&body).is_some() { browser.page = Some(body.clone()); }
    assert_genuine(app, browser, &body, now, at);
    if answered.get("content-type").is_some_and(|v| v[0].starts_with("application/json")) {
        if let Ok(Value::Object(members)) = serde_json::from_str::<Value>(&body) {
            for name in ["client_id", "access_token", "refresh_token", "registration_access_token"] { remember(vars, name, members.get(name).and_then(Value::as_str)); }
        }
    }
    let location = answered.get("location").map(|v| v[0].clone()).unwrap_or_default();
    let code = ["?code=", "&code=", "#code="].iter().find_map(|mark| location.split_once(mark)).map(|(_, rest)| rest.split(['&', '#']).next().unwrap().to_string());
    remember(vars, "code", code.as_deref());
    json!({ "status": status, "headers": answered, "body": body, "session_changed": session_changed })
}

/// What masking replaces, longest first: the values the scenario's responses gave, by their
/// numbered names (`[access_token.2]`), so that "the same token again" is still compared.
fn masks(state: &Value) -> Vec<(String, String)> {
    let mut masks: Vec<(String, String)> = state["vars"].as_object().unwrap().iter().filter(|(name, _)| name.contains('.'))
        .map(|(name, value)| (value.as_str().unwrap().to_string(), format!("[{name}]"))).collect();
    masks.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
    masks
}

fn masked(text: &str, masks: &[(String, String)]) -> String {
    masks.iter().fold(text.to_string(), |text, (value, name)| text.replace(value, name))
}

/// A mask must not hide a broken value. Every generated value has the shape its generator gives
/// it, is unlike every other, and is the one stored in the row it names. This reads the install's
/// own database, so it holds the values Rails issued and the ones this crate issued to the same rule.
fn assert_generated_values_are_real(dir: &Path, state: &Value) {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let vars = state["vars"].as_object().unwrap();
    let mut seen: Vec<&str> = vec![];
    for (name, value) in vars.iter().filter(|(name, _)| name.contains('.')) {
        let (kind, value) = (name.split('.').next().unwrap(), value.as_str().unwrap());
        let base64url = value.len() == 43 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        let hex = value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        assert!(if kind == "registration_access_token" { hex } else { base64url }, "{}: ${name} does not look generated", dir.display());
        assert!(!seen.contains(&value), "{}: ${name} repeats an earlier value", dir.display());
        seen.push(value);
        let (table, column) = match kind {
            "client_id" => ("oauth_applications", "uid"),
            "registration_access_token" => ("oauth_applications", "registration_access_token"),
            "code" => ("oauth_access_grants", "token"),
            "access_token" => ("oauth_access_tokens", "token"),
            _ => ("oauth_access_tokens", "refresh_token"),
        };
        let stored: i64 = c.query_row(&format!("SELECT count(*) FROM {table} WHERE {column} = ?1"), [value], |r| r.get(0)).unwrap();
        assert_eq!(stored, 1, "{}: ${name} is not the value stored in {table}.{column}", dir.display());
    }
}

fn masked_nonce(policy: &str) -> String {
    let Some(start) = policy.find("'nonce-") else { return policy.to_string() };
    let end = start + 7 + policy[start + 7..].find('\'').unwrap_or(0);
    format!("{}'nonce-[nonce]{}", &policy[..start], &policy[end..])
}

/// The page Rails' PublicExceptions serves for an error below the app (a path with no route, a body
/// that does not parse, a failed CSRF check under `protect_from_forgery with: :exception`). In
/// production these go through the app's own exceptions app instead; only the status is compared.
fn below_the_app(answer: &Value) -> bool {
    answer["headers"]["content-type"] == "text/html; charset=UTF-8"
}

/// One answer as the comparison sees it.
fn comparable(answer: &Value, masks: &[(String, String)]) -> Value {
    if !answer["resolved"].is_null() || !answer["allowed"].is_null() {
        let mut answer = answer.clone();
        answer.as_object_mut().unwrap().remove("by");
        answer.as_object_mut().unwrap().remove("retired");
        return answer;
    }
    if below_the_app(answer) { return json!({ "status": answer["status"], "below the app": true }); }
    let by_rails = answer["by"] == "rails";
    let header = |name: &str| -> Option<String> {
        match &answer["headers"][name] {
            Value::Array(all) => all.first().and_then(Value::as_str).map(str::to_string),
            Value::String(one) => Some(one.clone()),
            _ => None,
        }
    };
    let mut headers = BTreeMap::new();
    for name in HEADERS {
        let Some(value) = header(name) else { continue };
        let value = match name {
            "location" => html::location(&masked(&value, masks)),
            "content-security-policy-report-only" => masked_nonce(&value),
            // Rails writes its cookie on every response, this crate when the session changed
            // (web::session): Rails' cookie counts only where its session's content changed.
            "set-cookie" => {
                if by_rails && answer["session_changed"] != true { continue; }
                let all: Vec<String> = match &answer["headers"]["set-cookie"] { Value::Array(all) => all.iter().map(text).collect(), one => text(one).lines().map(str::to_string).collect() };
                match all.iter().find_map(|v| v.strip_prefix("_deltabadger_session=").or_else(|| v.strip_prefix("_deltabadger_rust_session="))) {
                    Some(cookie) => format!("[session]{}", cookie.find(';').map_or("", |at| &cookie[at..])),
                    None => continue,
                }
            }
            _ => masked(&value, masks),
        };
        headers.insert(name, value);
    }
    let body = answer["body"].as_str().unwrap();
    // Listed divergence (finding F5): Rails also accepts the `plain` challenge method and says so.
    let body = if by_rails { body.replace("must be one of plain, S256.", "must be S256.") } else { body.to_string() };
    let content_type = header("content-type").unwrap_or_default();
    let body = if content_type.starts_with("application/json") {
        serde_json::from_str::<Value>(&masked(&body, masks)).unwrap_or_else(|_| json!(body))
    } else if content_type.starts_with("text/html") {
        json!(html::normalize(&body).iter().map(|line| masked(line, masks)).collect::<Vec<_>>())
    } else {
        json!(body)
    };
    json!({ "status": answer["status"], "headers": headers, "body": body })
}

/// Every row of one table, every column, as stored.
fn table(dir: &Path, table: &str) -> Vec<Value> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut statement = c.prepare(&format!("SELECT * FROM {table} ORDER BY id")).unwrap();
    let names: Vec<String> = statement.column_names().iter().map(|name| name.to_string()).collect();
    statement.query_map([], |r| {
        Ok(Value::Object(names.iter().enumerate().map(|(i, name)| {
            let value = match r.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                other => json!(format!("{other:?}")),
            };
            Ok((name.clone(), value))
        }).collect::<rusqlite::Result<_>>()?))
    }).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

/// Every row of the OAuth tables, every column, generated values masked.
fn rows(dir: &Path, masks: &[(String, String)]) -> Value {
    Value::Object(TABLES.iter().map(|name| {
        let masked_rows = table(dir, name).into_iter().map(|row| Value::Object(row.as_object().unwrap().iter().map(|(column, value)| {
            (column.clone(), value.as_str().map_or_else(|| value.clone(), |text| json!(masked(text, masks))))
        }).collect())).collect();
        (name.to_string(), Value::Array(masked_rows))
    }).collect())
}

/// Which refresh token each refresh token was issued for, read from what was asked and answered so
/// far and not from the rows: a refresh that got a 200 was given the token its request presented.
fn parents(steps: &[Value], transcript: &[Value]) -> BTreeMap<String, String> {
    let (mut vars, mut parents) = (Map::new(), BTreeMap::new());
    for (step, answer) in steps.iter().zip(transcript) {
        let step = fill(step, &vars);
        let body = answer["body"].as_str().and_then(|body| serde_json::from_str::<Value>(body).ok()).unwrap_or(Value::Null);
        for name in ["client_id", "access_token", "refresh_token", "registration_access_token"] { remember(&mut vars, name, body[name].as_str()); }
        let location = match &answer["headers"]["location"] { Value::Array(all) => all.first().map(text), Value::String(one) => Some(one.clone()), _ => None }.unwrap_or_default();
        let code = ["?code=", "&code=", "#code="].iter().find_map(|mark| location.split_once(mark)).map(|(_, rest)| rest.split(['&', '#']).next().unwrap().to_string());
        remember(&mut vars, "code", code.as_deref());
        let refreshing = step["path"].as_str().is_some_and(|path| path.starts_with("/oauth/token")) && answer["status"] == 200
            && (step["form"]["grant_type"] == "refresh_token" || step["json"]["grant_type"] == "refresh_token");
        if refreshing {
            let presented = step["form"]["refresh_token"].as_str().or(step["json"]["refresh_token"].as_str()).expect("a refresh that names its token in the body");
            parents.insert(body["refresh_token"].as_str().expect("a refresh answers with a refresh token").to_string(), presented.to_string());
        }
    }
    parents
}

/// What presenting an access token to this crate may write, and must: every refresh token the
/// presented one descends from (by the history of the scenario's own requests) is revoked at this
/// moment unless it was revoked before; the presented row names no previous token any more (every
/// chain here is shorter than one walk); and no other row and no other column changed, the
/// ancestors' own links included. Returns the cells written: [id, column, value].
fn retirement(at: &str, before: &[Value], after: &[Value], presented: Option<&str>, parents: &BTreeMap<String, String>, now: chrono::DateTime<chrono::Utc>) -> Value {
    assert_eq!(before.len(), after.len(), "{at}: presenting a token adds or removes no row");
    let presented = presented.and_then(|token| before.iter().find(|row| row["token"] == token));
    let mut ancestors: Vec<&String> = vec![];
    let mut link = presented.and_then(|row| row["refresh_token"].as_str()).and_then(|token| parents.get(token));
    while let Some(ancestor) = link {
        assert!(!ancestors.contains(&ancestor), "{at}: the scenario's refreshes go round in a circle");
        ancestors.push(ancestor);
        link = parents.get(ancestor);
    }
    let now = deltabadger::codec::format_time(now);
    let mut written = vec![];
    for (old, new) in before.iter().zip(after) {
        let mut expected = old.clone();
        if presented.is_some_and(|row| row["id"] == old["id"]) {
            expected["previous_refresh_token"] = json!("");
        } else if old["revoked_at"].is_null() && old["refresh_token"].as_str().is_some_and(|token| ancestors.iter().any(|ancestor| *ancestor == token)) {
            expected["revoked_at"] = json!(now);
        }
        assert_eq!(*new, expected, "{at}: token row {} after the token was presented ({} refresh tokens before it)", old["id"], ancestors.len());
        for column in ["revoked_at", "previous_refresh_token"] {
            if new[column] != old[column] { written.push(json!([old["id"], column, new[column]])); }
        }
    }
    json!(written)
}

/// Rails' rows with what this crate's retirements wrote laid over them: what the install must hold
/// when a leg played by this crate presented tokens. Every cell is then compared.
fn with_retirements(rails: &Value, raw: &[Value]) -> Value {
    let mut rows = rails.clone();
    for write in raw.iter().filter_map(|answer| answer["retired"].as_array()).flatten() {
        if let Some(row) = rows["oauth_access_tokens"].as_array_mut().unwrap().iter_mut().find(|row| row["id"] == write[0]) {
            row[write[1].as_str().unwrap()] = write[2].clone();
        }
    }
    rows
}

/// What one install ended with, as the comparison sees it.
struct Played {
    answers: Vec<Value>,
    raw: Vec<Value>,
    rows: Value,
}

fn played(dir: &Path) -> Played {
    let state = read(&dir.join("state.json"));
    assert_generated_values_are_real(dir, &state);
    let masks = masks(&state);
    let raw = read(&dir.join("transcript.json")).as_array().unwrap().clone();
    Played { answers: raw.iter().map(|answer| comparable(answer, &masks)).collect(), raw, rows: rows(dir, &masks) }
}

fn first_difference(name: &str, mode: &str, rails: &[Value], other: &[Value]) -> Option<String> {
    if rails.len() != other.len() {
        return Some(format!("{name} [{mode}]: Rails answered {} steps, {mode} {}", rails.len(), other.len()));
    }
    // Where Rails answered below the app, only the status is its own.
    let other: Vec<Value> = rails.iter().zip(other).map(|(a, b)| if a["below the app"] == true { json!({ "status": b["status"], "below the app": true }) } else { b.clone() }).collect();
    let (index, (a, b)) = rails.iter().zip(&other).enumerate().find(|(_, (a, b))| a != b)?;
    if a["status"] != b["status"] || a["headers"] != b["headers"] || !a["body"].is_array() {
        return Some(format!("{name} [{mode}] step {index}:\n  rails: {a}\n  {mode}: {b}"));
    }
    let lines = |body: &Value| body.as_array().map(|lines| lines.iter().map(text).collect::<Vec<_>>()).unwrap_or_else(|| vec![text(body)]);
    Some(format!("{name} [{mode}] step {index}: {}", html::first_difference(&lines(&a["body"]), &lines(&b["body"])).unwrap_or_default()))
}

/// A scenario's `expect` on a step is the status Rails itself must answer there, so that two equal
/// wrong answers cannot pass.
fn unmet_expectation(name: &str, steps: &[Value], rails: &Played) -> Option<String> {
    steps.iter().enumerate().find_map(|(index, step)| {
        (!step["expect"].is_null() && rails.raw[index]["status"] != step["expect"])
            .then(|| format!("{name} step {index}: the scenario expects {}, Rails answered {}", step["expect"], rails.raw[index]["status"]))
    })
}

/// Scenarios whose LAST response this crate deliberately answers otherwise than Rails. Everything
/// before it, and (except where stated) the rows, are compared as in any scenario.
/// - `unrouted_`: Rails has no such route (404). This crate: 501, naming the request.
/// - `not_ported_`: Rails serves it (200) and this crate does not: Doorkeeper's out-of-band code
///   page, which no http(s) client can reach, and its introspection and token-info endpoints, which
///   the metadata does not advertise. 501, naming the request.
/// - `stricter_`: a POST or DELETE to /oauth/authorize that fails the CSRF check. Rails raises (422,
///   an error page); this crate's pipeline answers as it does for every page: back, with a flash.
///   Nothing is written by either.
/// - `tighter_`: a `plain` PKCE challenge. Rails shows the consent screen (200); this crate refuses
///   (400, the error page). See web::consent.
///
/// A fifth kind, `retired_`, is judged with the modes in the test itself, because who answers how
/// depends on who played the `use` step: a refresh token from before an access token that was
/// presented to this crate. Rails honours it for ever (200); once this crate has seen its successor
/// used, it is retired (400 invalid_grant), for this crate and for Rails after a handback.
fn listed_divergence(name: &str, steps: &[Value], rails: &Played, rust: &Played) -> Option<Result<(), String>> {
    let kind = ["unrouted_", "not_ported_", "stricter_", "tighter_"].into_iter().find(|prefix| name.starts_with(prefix))?;
    let last = steps.len() - 1;
    if let Some(message) = first_difference(name, "rust", &rails.answers[..last], &rust.answers[..last]) {
        return Some(Err(format!("before its last response: {message}")));
    }
    let step = &steps[last];
    let (theirs, ours) = (&rails.raw[last], &rust.raw[last]);
    let named = {
        let method = step["form"]["_method"].as_str().map_or_else(|| if step["do"] == "get" { "GET".to_string() } else { "POST".to_string() }, str::to_uppercase);
        ours["body"].as_str().unwrap().contains(&format!("{method} {}", step["path"].as_str().unwrap_or_default()))
    };
    let statuses = |rails_status: u64, rust_status: u64| (theirs["status"] == rails_status && ours["status"] == rust_status)
        .then_some(()).ok_or_else(|| format!("expected Rails {rails_status} and this crate {rust_status}; got {} and {}", theirs["status"], ours["status"]));
    let same_rows = || (rails.rows == rust.rows).then_some(()).ok_or_else(|| format!("rows differ:\n  rails: {}\n  rust:  {}", rails.rows, rust.rows));
    Some(match kind {
        "unrouted_" => statuses(404, 501).and(named.then_some(()).ok_or_else(|| "the 501 does not name the request".to_string())).and_then(|()| same_rows()),
        "not_ported_" => statuses(200, 501).and(named.then_some(()).ok_or_else(|| "the 501 does not name the request".to_string())).and_then(|()| same_rows()),
        "stricter_" => statuses(422, 302).and_then(|()| same_rows()).and_then(|()| {
            (ours["headers"]["location"][0] == "/" && ours["session_changed"] == true).then_some(()).ok_or_else(|| format!("expected a redirect to / with a flash, got {}", ours["headers"]))
        }),
        _ => statuses(200, 400).and_then(|()| same_rows())
            .and_then(|()| ours["body"].as_str().unwrap().contains(oauth::text::CODE_CHALLENGE_METHOD).then_some(()).ok_or_else(|| "the refusal does not say why".to_string())),
    })
}

fn rails_runner(scratch: &Path, args: &[&str]) {
    let mut all = vec!["runner", "script/rust/oauth.rb"];
    all.extend(args);
    common::rails(scratch, "test", &all);
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_answer_the_same_oauth_requests_and_write_the_same_rows() {
    let (scratch, temporary) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // OAUTH_KEEP=<an empty directory> keeps the installs and their transcripts there, for reading.
    let root = std::env::var("OAUTH_KEEP").map_or_else(|_| temporary.path().to_path_buf(), PathBuf::from);
    let grid = root.to_str().unwrap();
    common::rails(scratch.path(), "test", &["db:schema:load"]);
    rails_runner(scratch.path(), &["grid", grid]);

    // The catalogue the consent screen and the grants are built from is Rails' own.
    let catalogue = read(&root.join("catalogue.json"));
    assert_eq!(catalogue["defaults"], Value::Object(oauth::TOOL_DEFAULTS.iter().map(|(name, on)| (name.to_string(), json!(on))).collect()), "AppConfig::MCP_TOOL_DEFAULTS");
    assert_eq!(catalogue["groups"], Value::Object(oauth::TOOL_GROUPS.iter().map(|(name, tools)| (name.to_string(), json!(tools))).collect()), "AppConfig::TOOL_GROUPS");
    assert!(catalogue["rest_defaults"].as_object().unwrap().iter().map(|(name, on)| (name.as_str(), on)).eq(oauth::TOOL_DEFAULTS.iter().map(|(name, _)| (*name, &json!(false)))), "AppConfig::REST_TOOL_DEFAULTS");

    let installs = |mode: &str| -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join(mode)).into_iter().flatten().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        dirs
    };
    let names: Vec<String> = installs("rails").iter().map(|dir| dir.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert!(!names.is_empty(), "the grid is empty: no scenario name starts with a prefix in OAUTH");
    let most_legs = names.iter().map(|name| legs(read(&root.join("rails").join(name).join("scenario.json"))["steps"].as_array().unwrap()).len()).max().unwrap();
    for leg in 0..most_legs {
        rails_runner(scratch.path(), &["play", grid, &leg.to_string()]);
        for mode in ["rust", "rails_first", "rust_first"] {
            for dir in installs(mode) {
                if runtime(mode, leg) == "rust" { play_leg(&dir, leg).await; }
            }
        }
    }

    let (mut failures, mut crossed, mut steps_compared) = (vec![], 0, 0);
    for name in &names {
        let steps = read(&root.join("rails").join(name).join("scenario.json"))["steps"].as_array().unwrap().clone();
        let rails = played(&root.join("rails").join(name));
        let rust = played(&root.join("rust").join(name));
        steps_compared += steps.len();
        if let Some(message) = unmet_expectation(name, &steps, &rails) { failures.push(message); }
        match listed_divergence(name, &steps, &rails, &rust) {
            Some(Ok(())) => continue,
            Some(Err(message)) => { failures.push(format!("{name} (listed divergence): {message}")); continue; }
            None => {}
        }
        let mut modes = vec![("rust", rust)];
        for mode in ["rails_first", "rust_first"] {
            let dir = root.join(mode).join(name);
            if dir.is_dir() { modes.push((mode, played(&dir))); crossed += 1; }
        }
        for (mode, other) in &modes {
            // A `retired_` scenario ends on a refresh token this crate has retired, if this crate played a `use`.
            let retired = name.starts_with("retired_") && other.raw.iter().any(|answer| answer["retired"].is_array());
            if name.starts_with("retired_") && *mode == "rust" && !retired { failures.push(format!("{name}: no `use` step")); continue; }
            let compared = steps.len() - usize::from(retired);
            if let Some(message) = first_difference(name, mode, &rails.answers[..compared.min(rails.answers.len())], &other.answers[..compared.min(other.answers.len())]) { failures.push(message); continue; }
            // The rows are Rails' own, with the cells this crate's retirements wrote; nothing is left out.
            let mut expected = with_retirements(&rails.rows, &other.raw);
            if retired {
                let (theirs, ours) = (&rails.raw[compared]["status"], &other.raw[compared]["status"]);
                if *theirs != 200 || *ours != 400 {
                    failures.push(format!("{name} [{mode}] (listed divergence): expected Rails 200 and a 400 after this crate's retirement; got {theirs} and {ours}"));
                    continue;
                }
                // Rails has the one row more that the retired token was still good for.
                expected["oauth_access_tokens"].as_array_mut().unwrap().pop();
            }
            if expected != other.rows { failures.push(format!("{name} [{mode}]: rows differ\n  expected: {expected}\n  {mode}: {}", other.rows)); }
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), names.len(), failures.join("\n"));
    println!("{} scenarios, {steps_compared} steps; {crossed} cross-runtime plays", names.len());
    if std::env::var("OAUTH").is_err() {
        assert_eq!((names.len(), crossed), (125, 18), "a scenario was dropped or added without this count");
    }
}

/// An install with an owner, a registered public client (`client-uid`) and one unspent code for it
/// (`the-code`, issued at `AT` for `VERIFIER`'s S256 challenge).
const AT: &str = "2026-09-10T12:00:30.123456Z";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

fn seeded() -> (tempfile::TempDir, App) {
    let (dir, app, _) = seeded_with_clock();
    (dir, app)
}

fn seeded_with_clock() -> (tempfile::TempDir, App, std::sync::Arc<TestClock>) {
    let dir = common::rails_install();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute_batch("INSERT INTO users (email, encrypted_password, name, admin, confirmed_at, created_at, updated_at) VALUES ('o@example.com', 'x', 'Owner', 1, '2026-01-01', '2026-01-01', '2026-01-01');
                     INSERT INTO oauth_applications (name, uid, redirect_uri, scopes, confidential, created_at, updated_at) VALUES ('Client', 'client-uid', 'https://client.example/callback', 'mcp', 0, '2026-01-01', '2026-01-01');
                     INSERT INTO oauth_access_grants (application_id, resource_owner_id, token, expires_in, redirect_uri, scopes, code_challenge, code_challenge_method, created_at)
                       VALUES (1, 1, 'the-code', 600, 'https://client.example/callback', 'mcp', 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM', 'S256', '2026-09-10 12:00:30.123456');").unwrap();
    let clock = TestClock::at(AT);
    let app = web::app(dir.path(), web::SECRET, clock.clone());
    (dir, app, clock)
}

struct Sent {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

async fn send(app: &App, method: &str, path: &str, content_type: &str, body: &str, cookie: Option<&str>) -> Sent {
    let mut request = Request::builder().method(method).uri(path).header(header::HOST, web::HOST).header(header::CONTENT_TYPE, content_type);
    if let Some(cookie) = cookie { request = request.header(header::COOKIE, cookie); }
    let mut request = request.body(Body::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    let response = deltabadger::web::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().iter().map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap().to_string())).collect();
    Sent { status, headers, body: String::from_utf8_lossy(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned() }
}

const FORM: &str = "application/x-www-form-urlencoded";

fn exchange_form() -> String {
    format!("grant_type=authorization_code&code=the-code&redirect_uri=https%3A%2F%2Fclient.example%2Fcallback&code_verifier={VERIFIER}&client_id=client-uid")
}

fn count(dir: &Path, table: &str) -> i64 {
    rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap().query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

/// The routes a client calls on its own are outside the session and the CSRF check. That is true of
/// POST on exactly these three paths: under any other method they are not served, the request's
/// cookie is never answered, and nothing is written.
#[tokio::test(flavor = "current_thread")]
async fn the_client_routes_are_exempt_for_their_one_method_and_never_touch_the_session() {
    let (dir, app) = seeded();
    for path in ["/oauth/register", "/oauth/token", "/oauth/revoke"] {
        for method in ["GET", "PUT", "PATCH", "DELETE"] {
            let answer = send(&app, method, path, FORM, &exchange_form(), None).await;
            assert_eq!(answer.status, 501, "{method} {path}");
        }
        // A form may ask for another method (Rack::MethodOverride): then it is that method that is not served.
        let overridden = send(&app, "POST", path, FORM, &format!("_method=delete&{}", exchange_form()), None).await;
        assert_eq!(overridden.status, 501, "POST {path} with _method=delete");
    }
    assert_eq!((count(dir.path(), "oauth_access_tokens"), count(dir.path(), "oauth_applications")), (0, 1), "none of them did anything");

    // A browser's session: the login page gives it one, with a CSRF token.
    let page = send(&app, "GET", "/login", FORM, "", None).await;
    let cookie = page.headers.iter().find(|(name, _)| name == "set-cookie").map(|(_, value)| value.split(';').next().unwrap().to_string()).expect("the login page sets the session cookie");
    for cookie in [cookie.as_str(), "_deltabadger_rust_session=garbage"] {
        let registered = send(&app, "POST", "/oauth/register", "application/json", r#"{"redirect_uris":["https://client.example/callback"]}"#, Some(cookie)).await;
        let refused = send(&app, "POST", "/oauth/token", FORM, "grant_type=refresh_token&refresh_token=nothing&client_id=client-uid", Some(cookie)).await;
        let revoked = send(&app, "POST", "/oauth/revoke", FORM, "token=nothing&client_id=client-uid", Some(cookie)).await;
        let document = send(&app, "GET", "/.well-known/oauth-authorization-server", FORM, "", Some(cookie)).await;
        for (answer, status) in [(&registered, 201), (&refused, 400), (&revoked, 200), (&document, 200)] {
            assert_eq!(answer.status, status, "{}", answer.body);
            assert!(!answer.headers.iter().any(|(name, _)| name == "set-cookie"), "no cookie is ever sent from these routes");
        }
    }
    // The pages are not exempt: approving without the session's token writes no code.
    let approved = send(&app, "POST", "/oauth/authorize", FORM, "client_id=client-uid&redirect_uri=https%3A%2F%2Fclient.example%2Fcallback&response_type=code", Some(&cookie)).await;
    assert_eq!((approved.status, count(dir.path(), "oauth_access_grants")), (302, 1), "{}", approved.body);
}

#[tokio::test(flavor = "current_thread")]
async fn a_code_is_exchanged_once_however_many_ask_at_once() {
    let (dir, app) = seeded();
    let attempts: Vec<_> = (0..8).map(|_| { let app = app.clone(); tokio::spawn(async move { send(&app, "POST", "/oauth/token", FORM, &exchange_form(), None).await.status }) }).collect();
    let mut statuses = vec![];
    for attempt in attempts { statuses.push(attempt.await.unwrap()); }
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 400, 400, 400, 400, 400, 400, 400]);
    assert_eq!(count(dir.path(), "oauth_access_tokens"), 1);
}

/// A refusal says which kind of thing was wrong and never repeats what was sent: a code, a verifier,
/// a token or a secret in a response body or header would end up in a client's log.
#[tokio::test(flavor = "current_thread")]
async fn nothing_a_request_sent_comes_back_in_a_refusal() {
    let (_dir, app) = seeded();
    for body in ["grant_type=authorization_code&code=SENT-CODE&redirect_uri=https%3A%2F%2Fclient.example%2Fcallback&code_verifier=SENT-VERIFIER&client_id=client-uid",
                 "grant_type=authorization_code&code=the-code&redirect_uri=https%3A%2F%2Fclient.example%2Fcallback&code_verifier=SENT-VERIFIER&client_id=client-uid",
                 "grant_type=refresh_token&refresh_token=SENT-REFRESH&client_id=client-uid",
                 "grant_type=refresh_token&refresh_token=SENT-REFRESH&client_id=client-uid&client_secret=SENT-SECRET",
                 "grant_type=SENT-GRANT&client_id=SENT-CLIENT"] {
        let answer = send(&app, "POST", "/oauth/token", FORM, body, None).await;
        assert!((400..=401).contains(&answer.status), "{body}: {}", answer.status);
        let everything = format!("{} {:?}", answer.body, answer.headers);
        assert!(!everything.contains("SENT-"), "{body}: the refusal repeats the request: {everything}");
    }
    let revoked = send(&app, "POST", "/oauth/revoke", FORM, "token=SENT-TOKEN&client_id=SENT-CLIENT", None).await;
    assert!(revoked.status == 403 && !revoked.body.contains("SENT-"), "{}", revoked.body);
}

/// A JSON body is read under the form's limit, and one that is not JSON is refused before any handler.
#[tokio::test(flavor = "current_thread")]
async fn a_json_body_is_bounded_and_must_be_json() {
    let (dir, app) = seeded();
    let large = format!(r#"{{"redirect_uris":["https://client.example/callback"],"client_name":"{}"}}"#, "n".repeat(deltabadger::web::FORM_LIMIT));
    assert_eq!(send(&app, "POST", "/oauth/register", "application/json", &large, None).await.status, 413);
    assert_eq!(send(&app, "POST", "/oauth/register", "application/json", r#"{"redirect_uris": ["#, None).await.status, 400);
    for no_parameters in ["", "[]", "\"text\"", "null", "7"] {
        let answer = send(&app, "POST", "/oauth/register", "application/json", no_parameters, None).await;
        assert!(answer.status == 400 && answer.body.contains("redirect_uris is required"), "{no_parameters:?}: {} {}", answer.status, answer.body);
    }
    // Only the three client routes read JSON: elsewhere such a body is not read at all.
    assert_eq!(send(&app, "POST", "/csp-report", "application/json", &large, None).await.status, 204);
    assert_eq!(count(dir.path(), "oauth_applications"), 1);
}

/// Token rows as Rails leaves them: `(token, the token it was refreshed from)`, each with the refresh
/// token `r<token>`, none revoked, every link kept.
fn seed_tokens(dir: &Path, rows: &[(String, String)]) {
    let mut seed = String::from("BEGIN;");
    for (token, from) in rows {
        let previous = if from.is_empty() { String::new() } else { format!("r{from}") };
        seed.push_str(&format!("INSERT INTO oauth_access_tokens (application_id, resource_owner_id, token, refresh_token, previous_refresh_token, scopes, expires_in, created_at)
                                VALUES (1, 1, '{token}', 'r{token}', '{previous}', 'mcp', 3600, '2026-09-10 12:00:00.000000');"));
    }
    rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap().execute_batch(&(seed + "COMMIT;")).unwrap();
}

/// (token, revoked_at, previous_refresh_token) of every token row, in the order they were issued.
fn token_rows(dir: &Path) -> Vec<(String, Option<String>, String)> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut rows = c.prepare("SELECT token, revoked_at, previous_refresh_token FROM oauth_access_tokens ORDER BY id").unwrap();
    rows.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

/// Presents `token` to this crate at `time`.
async fn present(app: &App, clock: &std::sync::Arc<TestClock>, token: &str, time: &str) -> Result<i64, bearer::Refusal> {
    clock.set(web::at(time));
    let (clock, header) = (clock.clone(), format!("Bearer {token}"));
    app.db(move |c| bearer::authenticate(c, Some(&header), "mcp", &*clock)).await.unwrap().map(|bearer| bearer.user_id)
}

fn refresh_form(token: &str) -> String {
    format!("grant_type=refresh_token&refresh_token={token}&client_id=client-uid")
}

/// An install that comes from Rails has every refresh token it ever issued still live, in chains of
/// hundreds, and a refresh token redeemed twice has two tokens that share all of that. One
/// presentation walks `RETIRED_AT_ONCE` rows back and leaves the presented row pointing at the
/// next; the presentations after it finish the chain. The walk goes through rows that are revoked
/// already, so a client that goes on with the second of two sibling tokens still reaches every
/// ancestor the first one did not. Each ancestor is written once, with the time of the
/// presentation that revoked it, and keeps its own link. Another chain is never touched.
#[tokio::test(flavor = "current_thread")]
async fn a_long_chain_shared_by_two_tokens_is_retired_to_its_end_and_no_other_chain_is_touched() {
    let (dir, app, clock) = seeded_with_clock();
    let chain = |family: &'static str, length: usize| (1..=length).map(move |n| (format!("{family}{n}"), if n == 1 { String::new() } else { format!("{family}{}", n - 1) }));
    // a1 .. a250, then `first` and `second`, both refreshed from a250; and b1 .. b3.
    let rows: Vec<(String, String)> = chain("a", 250).chain([("first".to_string(), "a250".to_string()), ("second".to_string(), "a250".to_string())]).chain(chain("b", 3)).collect();
    seed_tokens(dir.path(), &rows);
    let as_rails_left_them = token_rows(dir.path());
    assert_eq!(bearer::RETIRED_AT_ONCE, 100);
    // What must be there: a<n> revoked at the time given for the first range that holds n, and the two siblings' links.
    let expected = |times: &[(usize, &str)], first: &str, second: &str| -> Vec<(String, Option<String>, String)> {
        as_rails_left_them.iter().map(|(token, _, previous)| match token.strip_prefix('a').and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => (token.clone(), times.iter().find(|(from, _)| n >= *from).map(|(_, time)| time.to_string()), previous.clone()),
            None if token == "first" => (token.clone(), None, first.to_string()),
            None if token == "second" => (token.clone(), None, second.to_string()),
            None => (token.clone(), None, previous.clone()),
        }).collect()
    };
    // As ActiveRecord writes a time: no fraction when there is none.
    let (one, three, four) = ("2026-09-10 12:00:10", "2026-09-10 12:00:20", "2026-09-10 12:00:25.500000");
    assert_eq!(present(&app, &clock, "first", "2026-09-10T12:00:10Z").await, Ok(1));
    assert_eq!(token_rows(dir.path()), expected(&[(151, one)], "ra150", "ra250"), "the first sibling: a250 to a151, each keeping its own link");
    // The client goes on with the second sibling, for good.
    assert_eq!(present(&app, &clock, "second", "2026-09-10T12:00:15Z").await, Ok(1));
    assert_eq!(token_rows(dir.path()), expected(&[(151, one)], "ra150", "ra150"), "the second sibling walks through the hundred that are revoked, and changes none");
    assert_eq!(present(&app, &clock, "second", "2026-09-10T12:00:20Z").await, Ok(1));
    assert_eq!(token_rows(dir.path()), expected(&[(151, one), (51, three)], "ra150", "ra50"), "then a150 to a51");
    assert_eq!(present(&app, &clock, "second", "2026-09-10T12:00:25.5Z").await, Ok(1));
    let done = expected(&[(151, one), (51, three), (1, four)], "ra150", "");
    assert_eq!(token_rows(dir.path()), done, "then the rest: every ancestor is retired, and the presented row names nothing more");
    assert_eq!(present(&app, &clock, "second", "2026-09-10T12:00:26Z").await, Ok(1));
    assert_eq!(token_rows(dir.path()), done, "one more presentation writes nothing");
    assert_eq!(token_rows(dir.path())[252..], as_rails_left_them[252..], "the other chain is as Rails left it");

    // The retired tokens are refused, the oldest and the latest of them alike; the siblings' own and the other chain's work.
    clock.set(web::at(AT));
    for (token, status) in [("ra1", 400), ("ra150", 400), ("ra151", 400), ("ra250", 400), ("rsecond", 200), ("rfirst", 200), ("rb1", 200)] {
        assert_eq!(send(&app, "POST", "/oauth/token", FORM, &refresh_form(token), None).await.status, status, "{token}");
    }
}

/// The stored links are data, and a walk along them ends whatever they say: a loop is left where it
/// closes, a row that names itself names nothing, and the presented token is never revoked by its own walk.
#[tokio::test(flavor = "current_thread")]
async fn a_chain_that_loops_ends_and_the_presented_token_is_not_revoked_by_it() {
    let (dir, app, clock) = seeded_with_clock();
    let row = |token: &str, from: &str| (token.to_string(), from.to_string());
    // x1 <- x2 <- x3 <- x1: a loop of three. `own` names itself. y2 comes from y1, which names the presented row.
    seed_tokens(dir.path(), &[row("x1", "x3"), row("x2", "x1"), row("x3", "x2"), row("own", "own"), row("y1", "y2"), row("y2", "y1")]);
    let at = "2026-09-10 12:00:10";
    assert_eq!(present(&app, &clock, "x3", "2026-09-10T12:00:10Z").await, Ok(1));
    assert_eq!(present(&app, &clock, "own", "2026-09-10T12:00:10Z").await, Ok(1));
    assert_eq!(present(&app, &clock, "y2", "2026-09-10T12:00:10Z").await, Ok(1));
    let some = |time: &str| Some(time.to_string());
    assert_eq!(token_rows(dir.path()), [
        ("x1".to_string(), some(at), "rx3".to_string()), ("x2".to_string(), some(at), "rx1".to_string()), ("x3".to_string(), None, String::new()),
        ("own".to_string(), None, String::new()),
        ("y1".to_string(), some(at), "ry2".to_string()), ("y2".to_string(), None, String::new()),
    ]);
    let after = token_rows(dir.path());
    for token in ["x3", "own", "y2"] { assert_eq!(present(&app, &clock, token, "2026-09-10T12:00:20Z").await, Ok(1)); }
    assert_eq!(token_rows(dir.path()), after, "and a second presentation of each writes nothing");
}

/// A loop longer than one walk ends too, across presentations: the place kept for the next one must
/// name a row older than every row the walk passed, which a chain that was issued always does. The
/// rows are inserted oldest first, as issuance writes them; only the link that closes each loop
/// names a newer row. Without that rule a loop of 100 would be walked whole by every presentation,
/// and a loop of 101 would move the place back one row each time, for ever.
#[tokio::test(flavor = "current_thread")]
async fn a_chain_that_loops_longer_than_one_walk_ends_across_presentations() {
    let (dir, app, clock) = seeded_with_clock();
    // p <- c1 <- ... <- c100 <- c1, and q <- d1 <- ... <- d101 <- d1; c100 and d101 the oldest rows.
    let cycle = |family: &str, length: usize, presented: &str| -> Vec<(String, String)> {
        (1..=length).rev().map(|n| (format!("{family}{n}"), format!("{family}{}", if n == length { 1 } else { n + 1 })))
            .chain([(presented.to_string(), format!("{family}1"))]).collect()
    };
    seed_tokens(dir.path(), &[cycle("c", 100, "p"), cycle("d", 101, "q")].concat());
    let as_seeded = token_rows(dir.path());
    let (one, two) = ("2026-09-10 12:00:10", "2026-09-10 12:00:20");
    // Every row of a family revoked at the time given for it, its link kept; the presented row's link as given.
    let expected = |family: char, times: &dyn Fn(usize) -> Option<&'static str>, presented: &str, link: &str| -> Vec<(String, Option<String>, String)> {
        as_seeded.iter().map(|(token, revoked_at, previous)| match token.strip_prefix(family).and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => (token.clone(), times(n).map(str::to_string), previous.clone()),
            None if token == presented => (token.clone(), None, link.to_string()),
            None => (token.clone(), revoked_at.clone(), previous.clone()),
        }).collect()
    };

    // A loop of exactly one walk: every row revoked, and the place it closes on is not kept.
    assert_eq!(present(&app, &clock, "p", "2026-09-10T12:00:10Z").await, Ok(1));
    let c_done = expected('c', &|_| Some(one), "p", "");
    assert_eq!(token_rows(dir.path()), c_done, "the loop of 100 is ended by its first presentation");
    assert_eq!(present(&app, &clock, "p", "2026-09-10T12:00:20Z").await, Ok(1));
    assert_eq!(token_rows(dir.path()), c_done, "a second presentation writes nothing");

    // One row longer: the first walk stops at d100 and keeps d101, the oldest row, as the place.
    assert_eq!(present(&app, &clock, "q", "2026-09-10T12:00:10Z").await, Ok(1));
    let d_one: Vec<_> = expected('d', &|n| (n <= 100).then_some(one), "q", "rd101").into_iter().zip(&c_done)
        .map(|(d, c)| if d.0.starts_with('c') || d.0 == "p" { c.clone() } else { d }).collect();
    assert_eq!(token_rows(dir.path()), d_one, "the loop of 101 after one presentation");
    // The second walk revokes d101 and comes round to rows newer than it: the loop is ended.
    assert_eq!(present(&app, &clock, "q", "2026-09-10T12:00:20Z").await, Ok(1));
    let d_done: Vec<_> = expected('d', &|n| Some(if n <= 100 { one } else { two }), "q", "").into_iter().zip(&c_done)
        .map(|(d, c)| if d.0.starts_with('c') || d.0 == "p" { c.clone() } else { d }).collect();
    assert_eq!(token_rows(dir.path()), d_done, "the loop of 101 is ended by its second presentation");
    assert_eq!(present(&app, &clock, "q", "2026-09-10T12:00:30Z").await, Ok(1));
    assert_eq!(token_rows(dir.path()), d_done, "a third presentation writes nothing");
}

/// A code is spent by a write, not by a comparison of times: the exchange sets `revoked_at` where
/// it is not set and issues only if that changed the row. A second exchange whose clock is behind
/// what the first one wrote, as a request that arrived earlier and ran later would have it, issues
/// nothing and moves nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_spent_code_stays_spent_for_a_request_whose_clock_is_behind_the_first_ones() {
    let (dir, app, clock) = seeded_with_clock();
    clock.set(web::at("2026-09-10T12:00:50Z"));
    assert_eq!(send(&app, "POST", "/oauth/token", FORM, &exchange_form(), None).await.status, 200);
    clock.set(web::at("2026-09-10T12:00:31Z"));
    let second = send(&app, "POST", "/oauth/token", FORM, &exchange_form(), None).await;
    assert!(second.status == 400 && second.body.contains("invalid_grant"), "{} {}", second.status, second.body);
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let revoked_at: String = c.query_row("SELECT revoked_at FROM oauth_access_grants WHERE token = 'the-code'", [], |r| r.get(0)).unwrap();
    assert_eq!((count(dir.path(), "oauth_access_tokens"), revoked_at.as_str()), (1, "2026-09-10 12:00:50"));
}

/// A refresh and the retirement of its token, in both orders, each with the second one's clock
/// behind the first one's. Retired first: the refresh issues nothing. Redeemed first: the token it
/// issued stands (the refresh token was good when it was redeemed), the retirement still happens,
/// keeps the time it was first given, and from then on the refresh token is refused.
#[tokio::test(flavor = "current_thread")]
async fn a_refresh_and_a_retirement_do_not_depend_on_whose_clock_is_ahead() {
    let row = |token: &str, from: &str| (token.to_string(), from.to_string());
    let (early, late) = ("2026-09-10T12:00:31Z", "2026-09-10T12:00:50Z");

    let (dir, app, clock) = seeded_with_clock();
    seed_tokens(dir.path(), &[row("p1", ""), row("p2", "p1")]);
    assert_eq!(present(&app, &clock, "p2", late).await, Ok(1));
    clock.set(web::at(early));
    assert_eq!(send(&app, "POST", "/oauth/token", FORM, &refresh_form("rp1"), None).await.status, 400, "retired at 12:00:50, redeemed by a clock at 12:00:31");
    assert_eq!(token_rows(dir.path()), [("p1".to_string(), Some("2026-09-10 12:00:50".to_string()), String::new()), ("p2".to_string(), None, String::new())]);

    let (dir, app, clock) = seeded_with_clock();
    seed_tokens(dir.path(), &[row("p1", ""), row("p2", "p1")]);
    clock.set(web::at(late));
    let redeemed = send(&app, "POST", "/oauth/token", FORM, &refresh_form("rp1"), None).await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);
    let sibling = serde_json::from_str::<Value>(&redeemed.body).unwrap()["access_token"].as_str().unwrap().to_string();
    assert_eq!(present(&app, &clock, "p2", early).await, Ok(1));
    let retired = |rows: &[(String, Option<String>, String)]| rows[0].1.clone();
    assert_eq!(retired(&token_rows(dir.path())).as_deref(), Some("2026-09-10 12:00:31"));
    // The sibling's own walk reaches the same row later, and so does a second look by a later clock: the time stays.
    assert_eq!(present(&app, &clock, &sibling, "2026-09-10T12:00:55Z").await, Ok(1));
    assert_eq!(retired(&token_rows(dir.path())).as_deref(), Some("2026-09-10 12:00:31"));
    clock.set(web::at(AT)); // 12:00:30.123456: before the retirement's own time
    assert_eq!(send(&app, "POST", "/oauth/token", FORM, &refresh_form("rp1"), None).await.status, 400);
    assert_eq!(count(dir.path(), "oauth_access_tokens"), 3);
}

/// The moment a request is judged by, and writes, is read after it has the database, not when it
/// arrived: a request that waited while another held the database is dated by when its turn came.
#[tokio::test(flavor = "current_thread")]
async fn a_request_that_waited_for_the_database_is_dated_when_its_turn_came() {
    let (dir, app, clock) = seeded_with_clock();
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (taken, is_taken) = tokio::sync::oneshot::channel::<()>();
    let holder = { let app = app.clone(); tokio::spawn(async move { app.db(move |_| { let _ = taken.send(()); let _ = held.recv(); Ok(()) }).await }) };
    is_taken.await.unwrap();
    let waiting = { let app = app.clone(); tokio::spawn(async move { send(&app, "POST", "/oauth/token", FORM, &exchange_form(), None).await }) };
    // The request arrives and gets as far as the database, which is held.
    for _ in 0..50 { tokio::task::yield_now().await; }
    clock.set(web::at("2026-09-10T12:05:00Z"));
    release.send(()).unwrap();
    let (answer, held_ok) = (waiting.await.unwrap(), holder.await.unwrap());
    assert!(held_ok.is_ok() && answer.status == 200, "{}", answer.body);
    assert_eq!(serde_json::from_str::<Value>(&answer.body).unwrap()["created_at"], web::at("2026-09-10T12:05:00Z").timestamp());
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let written: (String, String) = c.query_row("SELECT g.revoked_at, t.created_at FROM oauth_access_grants g, oauth_access_tokens t WHERE g.token = 'the-code'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(written, ("2026-09-10 12:05:00".to_string(), "2026-09-10 12:05:00".to_string()));
}
