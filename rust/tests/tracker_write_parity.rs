//! Exact body bytes and every primary row for the implemented write slice.
//! The complete tracker page grid remains a separate acceptance gate.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session};
use rusqlite::{Connection, types::{Value as Sql, ValueRef}};
use serde_json::{json, Map, Value};
use std::path::Path;

/// Every Rails table's rows. Rust's own tables (`REGISTRY.owned_tables()`) and the one app_configs row Rust writes on
/// an import (the identity key, `identity::KEY_CONFIG`) are left out: Rails has neither; `rust_effects` asserts them.
fn snapshot(c: &Connection) -> Value {
    let owned = deltabadger::store::REGISTRY.owned_tables();
    let names: Vec<String> = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([],|r|r.get(0)).unwrap().collect::<Result<Vec<String>,_>>().unwrap().into_iter().filter(|n| !owned.contains(&n.as_str())).collect();
    let mut tables = Map::new();
    for name in names {
        let skip = if name == "app_configs" { format!(" WHERE key IS NOT '{}'", deltabadger::identity::KEY_CONFIG) } else { String::new() };
        let mut statement = c.prepare(&format!("SELECT * FROM {name}{skip} ORDER BY 1")).unwrap();
        let columns: Vec<String> = statement.column_names().iter().map(|s|s.to_string()).collect();
        let rows = statement.query_map([],|r| {
            let mut values = Map::new();
            for (i,column) in columns.iter().enumerate() {
                let value = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null, ValueRef::Integer(n) => json!(n), ValueRef::Real(n) => json!({"__sqlite_real__":format!("{:016x}",n.to_bits())}),
                    ValueRef::Text(s) => json!(String::from_utf8_lossy(s)), ValueRef::Blob(_) => panic!("unexpected blob in {name}.{column}"),
                };
                values.insert(column.clone(),value);
            }
            Ok(Value::Object(values))
        }).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        tables.insert(name,Value::Array(rows));
    }
    Value::Object(tables)
}
fn combined(answer: &Value, phase: &str) -> Value {
    let mut tables = answer[format!("rows_{phase}")].as_object().unwrap().clone();
    tables.extend(answer[format!("other_rows_{phase}")].as_object().unwrap().clone());
    Value::Object(tables)
}
fn flat(prefix: &str, value: &Value, out: &mut Vec<(String,String)>) {
    match value {
        Value::Object(rows) => for (name,value) in rows { flat(&if prefix.is_empty() { name.clone() } else {format!("{prefix}[{name}]")},value,out); },
        Value::Array(rows) => for value in rows { flat(&format!("{prefix}[]"),value,out); },
        Value::Null => {}, Value::String(s) => out.push((prefix.into(),s.clone())), _ => panic!("unsupported fixture scalar"),
    }
}
fn scalar(value: &Value) -> Sql {
    match value { Value::Null => Sql::Null, Value::Bool(v) => Sql::Integer(i64::from(*v)),
        Value::Number(v) => v.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(v.as_f64().unwrap())),
        Value::Object(v) if v.contains_key("__sqlite_real__") => Sql::Real(f64::from_bits(u64::from_str_radix(v["__sqlite_real__"].as_str().unwrap(),16).unwrap())),
        Value::String(v) => Sql::Text(v.clone()), _ => Sql::Text(value.to_string()) }
}
fn same_rows(actual: Value, expected: Value, label: &str) {
    // Rails' action snapshot already serializes SQLite values, not ActiveRecord attributes.
    for (table,rows) in expected.as_object().unwrap() { assert_eq!(&actual[table],rows,"{label}: {table}"); }
    assert_eq!(actual.as_object().unwrap().len(),expected.as_object().unwrap().len());
}

#[tokio::test(flavor="current_thread")]
async fn implemented_writes_match_the_recorded_rails_bytes_and_all_primary_rows() {
    compare(&["settings_validation_zone", "settings_validation_blank_zone", "settings_validation_locale",
        "settings_validation_blank_locale", "settings_validation_currency", "settings_validation_lower_currency",
        "settings_validation_jurisdiction", "settings_validation_email", "settings_validation_valid_context"]).await;
    compare(&["fund_cast_0","fund_cast_1","fund_cast_2","fund_cast_3","fund_cast_4","settings","settings_blank","settings_query_json","fund_symbol_json","fund_share","fund_valid","fund_invalid","fund_mixed","fund_bad_shape","fund_unknown",
        "sync_empty","sync_alpaca","sync_read_only","sync_withdrawal","sync_incorrect"]).await;
}

#[tokio::test(flavor="current_thread")]
async fn price_and_transfer_match_rails_bytes_and_all_primary_rows() {
    compare(&["price_accept_0","price_accept_1","price_accept_2","link_accept_0","link_accept_1","link_accept_2","link_sql_precision","link_sql_reverse","price_blank_currency","price_overflow","price_12.5","price_0","price_-1","price_abc","price_clear","price_venue","link_unlink","link_missing","link_success","price_hidden","price_missing","price_small","price_negative_adjustment","price_cash_eur","price_flag","price_group_clear","price_quote_clear"]).await;
    let locales:Vec<String>=deltabadger::web::locale::LOCALES.iter().map(|locale|format!("price_locale_{locale}")).collect();
    compare(&locales.iter().map(String::as_str).collect::<Vec<_>>()).await;
}

fn mask_csrf(body: &str) -> String {
    let prefix = "name=\"authenticity_token\" value=\"";
    let mut rest=body;
    let mut result=String::new();
    while let Some((before,tail))=rest.split_once(prefix) {
        result.push_str(before); result.push_str(prefix);
        let (token,after)=tail.split_once('"').expect("closed token attribute");
        assert!(!token.is_empty());
        result.push_str("[csrf]\""); rest=after;
    }
    result.push_str(rest);
    let mut result = mask_import_token(&result);
    for path in common::html::masked_values(&result).assets {
        if let Some((base,tail))=path.rsplit_once('-') {
            if let Some((digest,extension))=tail.split_once('.') {
                if (8..=64).contains(&digest.len()) && digest.bytes().all(|b|b.is_ascii_hexdigit()) {
                    result=result.replace(&path,&format!("{base}-[digest].{extension}"));
                }
            }
        }
    }
    result
}

#[tokio::test(flavor="current_thread")]
async fn import_dialog_and_the_no_file_answer_match_rails() {
    compare(&["import_new","import_missing"]).await;
}

#[tokio::test(flavor="current_thread")]
async fn importing_a_file_matches_rails_bytes_and_all_primary_rows() {
    compare(&["import_binance_ok","import_nothing_new","import_unknown_timezone","import_unreadable","import_ambiguous","import_answer","import_bad_key"]).await;
}

/// The token a parked upload is filed under is random on both sides: masked like a CSRF token.
fn mask_import_token(body: &str) -> String {
    regex::Regex::new(r#"name="token" id="token" value="[0-9a-f]{16}""#).unwrap().replace_all(body, r#"name="token" id="token" value="[token]""#).into_owned()
}

#[tokio::test(flavor="current_thread")]
async fn export_modal_is_read_only_and_matches_rails_bytes() {
    compare(&["export_modal","modal_settings","modal_broker","modal_broker_empty","modal_broker_foreign_country"]).await;
    let locales:Vec<String>=deltabadger::web::locale::LOCALES.iter().map(|locale|format!("modal_locale_{locale}")).collect();
    compare(&locales.iter().map(String::as_str).collect::<Vec<_>>()).await;
}

/// The imports that write a venue export's account (`identity::record_import`): compared with the identity key absent
/// and with it already present.
const IDENTITY_CASES: [&str; 2] = ["import_binance_ok", "import_answer"];

/// What a migrated import writes beside Rails' rows: the identity key, created once and reused; one import observation
/// per identity-bearing file; no KuCoin source rows.
fn rust_effects(c: &Connection, name: &str, key_before: Option<String>) {
    let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    let key_after = deltabadger::app_config::get_plain(c, deltabadger::identity::KEY_CONFIG).unwrap();
    if key_before.is_some() { assert_eq!(key_after, key_before, "{name}: the identity key is reused"); }
    if IDENTITY_CASES.contains(&name) {
        assert!(key_after.is_some(), "{name}: the identity key exists");
        assert_eq!(count("SELECT count(*) FROM exchange_account_imports"), 1, "{name}: the export's account is recorded");
    }
    assert_eq!(count("SELECT count(*) FROM import_rows"), 0, "{name}");
}

/// A12-1: the tracker's tax report create (`tax_*`) and download (`download_*`) against Rails' answers, rows, enqueued
/// job, files and flash.
#[tokio::test(flavor="current_thread")]
async fn tax_report_requests_and_downloads_match_rails() {
    compare(&["tax_invalid","tax_lower_case","tax_pending","tax_crypto","tax_stablecoin","tax_locale_de","tax_stale_refusal",
        "tax_generating","tax_generating_refused","tax_broker","tax_de","tax_fr",
        "download_expired","download_refused","download_ready","download_broker"]).await;
}

/// D-TAX-NOT-PORTED-GATE (PLAN-A12 item 6): a create this build cannot compute (a jurisdiction not yet ported, the broker
/// scope until A12-7) answers the refused flash and writes, queues and moves nothing. Rails accepts both.
const TAX_GATED: [&str; 2] = ["tax_broker", "tax_fr"];
/// D-TAX-RECONCILE under a running report: Rails' `Generating.accepted?` reads Solid Queue's discarded job as accepted
/// (Active Job sets `successfully_enqueued` after the adapter returns; the destroyed row leaves no provider_job_id), so
/// it deletes the user's refusal and shows progress for a run that never happens. Rust keeps the refusal and answers its
/// flash, which pages_tracker.rb renders with Rails' partial (`reconciled.html`).
const TAX_RECONCILED: &str = "tax_generating_refused";

/// D-TAX-RECONCILE: every tax panel Rust renders is the request's own (`tax-report-<request>`, `data-tax-request`, and on
/// the progress panel the reconcile-on-connect controller); Rails' all share `tax-report-progress`. Normalised to Rails'.
fn reconcile(body: &str) -> String {
    let id = regex::Regex::new(r#"id="tax-report-[0-9a-f]{32}""#).unwrap();
    let attributes = regex::Regex::new(r#" data-tax-request="[0-9a-f]{32}"| data-controller="broadcast--on-connect"| data-broadcast--on-connect-[a-z-]+-value="[^"]*""#).unwrap();
    attributes.replace_all(&id.replace_all(body, r#"id="tax-report-progress""#), "").into_owned()
}

/// `rust_tax_reports`, oldest first.
fn tax_rows(c: &Connection) -> Vec<Value> {
    let mut statement = c.prepare("SELECT request,user_id,scope,country,year,stablecoin_as_fiat,locale,state FROM rust_tax_reports ORDER BY id").expect("rust_tax_reports (A12-1)");
    statement.query_map([], |r| Ok(json!({"request": r.get::<_,String>(0)?, "user_id": r.get::<_,i64>(1)?, "scope": r.get::<_,String>(2)?,
        "country": r.get::<_,String>(3)?, "year": r.get::<_,i64>(4)?, "stablecoin_as_fiat": r.get::<_,i64>(5)?, "locale": r.get::<_,String>(6)?,
        "state": r.get::<_,String>(7)?}))).unwrap().collect::<Result<_,_>>().unwrap()
}

/// The files under `<root>/tax_reports`, by name.
fn tax_files(root: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_dir(root.join("tax_reports")).into_iter().flatten().flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap())).collect()
}

/// D-IMPORT-FORMATS: the dialog offers Rust's KuCoin format beside Rails' two; the rest of it is Rails' bytes.
/// D-TAX-STABLECOIN-QUESTION: Rails asks "treat stablecoins as fiat" for AT and SK; the report reads the flag for neither
/// (D-TAX-AT-STABLECOIN, D-TAX-SK-SWAPS), so no country option is marked ambiguous and the checkbox is never shown.
fn stablecoin_question(body: &str) -> String {
    let mut out = body.to_string();
    for code in ["AT", "SK"] {
        let option = regex::Regex::new(&format!(r#"(<option value="{code}" [^>]*data-stablecoin-ambiguous=")true(")"#)).unwrap();
        assert!(option.is_match(&out), "Rails asks the stablecoin question for {code}");
        out = option.replace(&out, "${1}false${2}").into_owned();
    }
    assert!(!out.contains(r#"data-stablecoin-ambiguous="true""#), "Rails asks it for no other country");
    out
}

/// D-TAX-PL-DECLARED-CARRY (Rust-only, approved): the modal's PL forms, one per offered crypto year from 2019 (2021–2026 at
/// the recordings' 2026 clock), each empty (no recording declares a carry), with the style that shows the selected one,
/// directly before the footer. Rust's body without them is Rails' body.
fn without_pl_carry(body: &str) -> String {
    let block = regex::Regex::new(r#"(?s)      <style>\.pl-carry\{display:none\}[^<]*</style>\n((?:<form id="pl-carry-\d{4}" .*?</form>\n)+)(      <div class="modal__footer">)"#).unwrap();
    let found = block.captures(body).expect("the PL forms before the footer");
    let years: Vec<String> = regex::Regex::new(r#"<form id="pl-carry-(\d{4})""#).unwrap().captures_iter(&found[1]).map(|c| c[1].to_string()).collect();
    assert_eq!(years, ["2021", "2022", "2023", "2024", "2025", "2026"], "one PL form per offered year from 2019");
    assert_eq!(found[1].matches(r#"name="pl_carried_costs" class="rinput rinput--outline rinput--text""#).count(), 6);
    assert_eq!(found[1].matches(r#"aria-label=""#).count(), found[1].matches(r#" value="">"#).count(), "no carry declared in a recording");
    block.replace(body, "$2").into_owned()
}

fn without_kucoin_format(body: &str) -> String {
    let option = regex::Regex::new(r#"\n *<button type="button" class="segmented__option" role="radio" aria-checked="false" tabindex="-1" data-value="kucoin"[^\n]*</button>"#).unwrap();
    assert!(option.is_match(body), "the KuCoin format is offered");
    option.replace(body, "").into_owned()
}

async fn compare(cases: &[&str]) {
    let generated;
    let root = match std::env::var("D5_GRID") {
        Ok(root) => root,
        Err(_) => {
            generated = common::recording("pages_tracker", common::has_subdirs, |out| common::record_grid("script/rust/pages_tracker.rb", out));
            generated.path.to_str().unwrap().to_string()
        }
    };
    for &name in cases {
        compare_one(&root, name, false).await;
        if IDENTITY_CASES.contains(&name) { compare_one(&root, name, true).await; }
    }
}

async fn compare_one(root: &str, name: &str, identity_key: bool) {
    let (root, cases) = (root.to_string(), [name]);
    for name in cases {
        let directory = if name.starts_with("sync_") { std::env::var("D5_SYNC_GRID").unwrap_or_else(|_|root.clone()) } else { root.clone() };
        let source = Path::new(&directory).join(format!("d5_{name}"));
        let scenario: Value = serde_json::from_slice(&std::fs::read(source.join("scenario.json")).unwrap()).unwrap();
        let rails: Value = serde_json::from_slice(&std::fs::read(source.join("rails.json")).unwrap()).unwrap();
        assert!(rails["network"].as_array().unwrap().is_empty());
        let expected = rails["responses"].as_array().unwrap().last().unwrap();
        let step = scenario["steps"].as_array().unwrap().last().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for name in ["production.sqlite3","production_queue.sqlite3"] { std::fs::copy(source.join(name),dir.path().join(name)).unwrap(); }
        // A12-1: the report files as they were before Rails' request (Rails' tmp/ holds what its request left).
        let tax = name.starts_with("tax_") || name.starts_with("download_");
        if source.join("tmp_before").is_dir() { common::copy_tree(&source.join("tmp_before"), &dir.path().join("tmp")); }
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        // A migrated install, as `serve` runs on.
        deltabadger::store::test_support::migrate(&c).unwrap();
        // The record directory contains the post-request database. Rebuild the pre-request
        // row state from the recorder before opening App, retaining the real Rails schema.
        c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE").unwrap();
        let before = combined(expected,"before");
        for (table,rows) in before.as_object().unwrap() {
            c.execute(&format!("DELETE FROM {table}"),[]).unwrap();
            for row in rows.as_array().unwrap() {
                let row = row.as_object().unwrap();
                let columns = row.keys().cloned().collect::<Vec<_>>().join(",");
                let placeholders = (1..=row.len()).map(|i|format!("?{i}")).collect::<Vec<_>>().join(",");
                c.execute(&format!("INSERT INTO {table}({columns}) VALUES({placeholders})"),rusqlite::params_from_iter(row.values().map(scalar))).unwrap();
            }
        }
        // Explicit IDs are part of equality. DELETE does not rewind SQLite's sequence.
        c.execute("DELETE FROM sqlite_sequence WHERE name IN ('fund_classifications','account_transactions')",[]).unwrap();
        c.execute_batch("COMMIT; PRAGMA foreign_keys=ON").unwrap();
        let secret = scenario["secret_key_base"].as_str().unwrap();
        let app = web::app(dir.path(),secret,TestClock::at(scenario["at"].as_str().unwrap()));
        app.attach_jobs(Default::default()).unwrap();
        if identity_key { deltabadger::identity::key(&c,&app.cipher,app.now()).unwrap(); }
        let key_before = deltabadger::app_config::get_plain(&c,deltabadger::identity::KEY_CONFIG).unwrap();
        same_rows(snapshot(&c),before.clone(),&format!("{name} before"));
        // The running crypto report Rails' step found in Solid Queue (pages_tracker.rb `d5_tax_generating`), as Rust holds it.
        if step["before"] == "d5_tax_generating" {
            c.execute("INSERT INTO rust_tax_reports(request,user_id,scope,country,year,stablecoin_as_fiat,locale,state,created_at)
                SELECT '00000000000000000000000000000001',id,'crypto','US',2025,0,'en','queued','2026-09-10 12:00:00' FROM users ORDER BY id LIMIT 1",[]).expect("rust_tax_reports (A12-1)");
        }
        let tax_before = if tax { tax_rows(&c) } else { vec![] };
        let woken = app.job_wakers().map_or(0, |jobs| jobs.pending().len());
        let lock=name.starts_with("import").then(||deltabadger::lease::lock(&deltabadger::store::Paths::from_env(&|_|None,dir.path()),app.now()).unwrap());
        if let Some(lock)=&lock {deltabadger::lease::claim(lock,&c,&app.cipher,"test",app.now()).unwrap();}
        let ownership:Vec<_>=snapshot(&c)["app_configs"].as_array().unwrap().iter().filter(|r|[deltabadger::lease::KEY,deltabadger::lease::TAKEN_OVER_AT].contains(&r["key"].as_str().unwrap_or_default())).cloned().collect();
        let (owner,hash): (i64,String) = c.query_row("SELECT id,encrypted_password FROM users ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let token = csrf::new_token();
        let data = session::SessionData { user: Some((owner,hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
        let mut browser = Browser { cookie: Some(session::seal(&app.keys.session,&data,app.now())),page:Some(format!("<meta name=\"csrf-token\" content=\"{}\">",csrf::masked(&token))) };
        let mut fields = vec![];
        flat("",&step["form"],&mut fields);
        // A step that submits the previous response's form: that request first, then its hidden values.
        if let Some(from_last) = step["form_from_last"].as_object() {
            let earlier = &scenario["steps"].as_array().unwrap()[scenario["steps"].as_array().unwrap().len() - 2];
            let files: Vec<(&str,&str,&[u8])> = earlier["multipart"].as_array().unwrap().iter().map(|f|(f["name"].as_str().unwrap(),f["filename"].as_str().unwrap(),f["content"].as_str().unwrap().as_bytes())).collect();
            let mut earlier_fields = vec![];
            flat("",&earlier["form"],&mut earlier_fields);
            let form: Vec<_> = earlier_fields.iter().map(|(k,v)|(k.as_str(),v.as_str())).collect();
            let (content_type,body) = web::multipart(&form,&files);
            let mut headers: Vec<_> = earlier["headers"].as_object().unwrap().iter().map(|(k,v)|(k.as_str(),v.as_str().unwrap())).collect();
            headers.push(("content-type",content_type.as_str()));
            let last = browser.send_bytes(&app,"POST",earlier["path"].as_str().unwrap(),Some(body),Csrf::Header,&headers).await;
            for (name,pattern) in from_last {
                let value = regex::Regex::new(pattern.as_str().unwrap()).unwrap().captures(&last.body).unwrap_or_else(||panic!("{name}: no {name} in the previous answer"))[1].to_string();
                fields.push((name.clone(),value));
            }
        }
        let form: Vec<_> = fields.iter().map(|(k,v)|(k.as_str(),v.as_str())).collect();
        let headers: Vec<_> = step["headers"].as_object().unwrap().iter().map(|(k,v)|(k.as_str(),v.as_str().unwrap())).collect();
        let answer = if let Some(files) = step["multipart"].as_array() {
            let files: Vec<(&str,&str,&[u8])> = files.iter().map(|f|(f["name"].as_str().unwrap(),f["filename"].as_str().unwrap(),f["content"].as_str().unwrap().as_bytes())).collect();
            let (content_type,body) = web::multipart(&form,&files);
            let mut headers = headers.clone(); headers.push(("content-type",content_type.as_str()));
            browser.send_bytes(&app,step["method"].as_str().unwrap(),step["path"].as_str().unwrap(),Some(body),Csrf::Header,&headers).await
        } else if let Some(body) = step["json"].as_str() {
            browser.send_body(&app,step["method"].as_str().unwrap(),step["path"].as_str().unwrap(),Some(body.into()),Csrf::Header,&headers).await
        } else { browser.send(&app,step["method"].as_str().unwrap(),step["path"].as_str().unwrap(),Some(&form),Csrf::Header,&headers).await };
        let wakes: Vec<_> = app.job_wakers().map(|jobs| jobs.pending()).unwrap_or_default().split_off(woken);
        if TAX_GATED.contains(&name) {
            assert_eq!(answer.status, 501, "{name}: D-TAX-NOT-PORTED-GATE");
            assert!(answer.body.starts_with(r#"<turbo-stream action="append" target="flash"><template>"#), "{name}: the refused flash: {}", answer.body);
            same_rows(snapshot(&c), before, &format!("{name}: a refused create writes nothing"));
            assert_eq!(tax_rows(&c), tax_before, "{name}: nothing queued");
            assert!(wakes.is_empty(), "{name}: nothing woken: {wakes:?}");
            assert_eq!(tax_files(&dir.path().join("tmp")), tax_files(&source.join("tmp_before")), "{name}: no file moved");
            continue;
        }
        // D-TRACKER-SETTINGS-422: a settings save the user validation refuses answers 422, as fund_classifications does;
        // Rails ignores update's false return and answers 200.
        let status = if name.starts_with("settings_validation_") && name != "settings_validation_valid_context" {
            assert_eq!(expected["status"], 200, "{name}: Rails now refuses the save: drop D-TRACKER-SETTINGS-422"); 422
        } else { expected["status"].as_u64().unwrap() };
        assert_eq!(u64::from(answer.status),status,"{name}");
        if expected.get("exception").is_none() { {
            for path in common::html::masked_values(&answer.body).assets {
                assert!(deltabadger::web::assets::find(&path).is_some(),"missing embedded asset {path}");
            }
            for rendered in common::html::masked_values(&answer.body).tokens {
                assert!(csrf::valid(&token,&rendered),"invalid rendered CSRF");
            }
            let body = if name == "import_new" { without_kucoin_format(&answer.body) } else if tax { reconcile(&answer.body) } else { answer.body.clone() };
            let body = if body.contains("<style>.pl-carry{") { without_pl_carry(&body) } else { body };
            let rails_body = if name == TAX_RECONCILED { std::fs::read_to_string(source.join("reconciled.html")).unwrap() } else { expected["body"].as_str().unwrap().to_string() };
            let rails_body = if rails_body.contains("data-stablecoin-ambiguous=") { stablecoin_question(&rails_body) } else { rails_body };
            assert_eq!(mask_csrf(&body),mask_csrf(&rails_body),"{name}: body bytes");
        } }
        else {
            // Existing web-foundation exception contract: public error file and below-controller
            // headers. Rails recordings omit their debugging page, which is not production HTML.
            let page = if answer.status == 404 { include_str!("../../public/404.html") } else if answer.status == 422 { include_str!("../../public/422.html") } else { include_str!("../../public/500.html") };
            assert_eq!(answer.body,page,"{name}: public error page");
            assert_eq!(answer.header("content-type"),Some("text/html; charset=utf-8"));
            assert_eq!(answer.header("cache-control"),Some("no-cache"));
            for header in ["x-frame-options","x-xss-protection","x-content-type-options","x-permitted-cross-domain-policies","referrer-policy"] { assert_eq!(answer.header(header),None); }
        }
        for header in ["content-type","cache-control","x-frame-options","x-xss-protection","x-content-type-options","x-permitted-cross-domain-policies","referrer-policy"] {
            if expected.get("exception").is_none() { assert_eq!(answer.header(header),expected["headers"][header].as_str(),"{name}: {header}"); }
        }
        if name.starts_with("export_") || name.starts_with("download_") { assert_eq!(answer.header("content-disposition"),expected["headers"]["content-disposition"].as_str(),"{name}: disposition"); }
        if name.starts_with("download_") { assert_eq!(answer.header("content-transfer-encoding"),expected["headers"]["content-transfer-encoding"].as_str(),"{name}: transfer encoding"); }
        if step["session"].as_array().is_some_and(|keys| keys.contains(&json!("flash"))) {
            let data = browser.cookie.as_deref().and_then(|cookie| session::open(&app.keys.session, cookie, app.now())).unwrap_or_default();
            let flash: Map<String, Value> = data.flash.iter().map(|(kind, message)| (kind.clone(), json!(message))).collect();
            assert_eq!(Value::Object(flash), json!(expected["session"]["flash"]["flashes"].as_object().cloned().unwrap_or_default()), "{name}: flash");
        }
        let actual=snapshot(&c);let mut expected_after=combined(expected,"after");
        let configs=expected_after["app_configs"].as_array_mut().unwrap();
        if lock.is_some() {configs.retain(|r|![deltabadger::lease::KEY,deltabadger::lease::TAKEN_OVER_AT].contains(&r["key"].as_str().unwrap_or_default()));configs.extend(ownership);}
        let a12:Vec<_>=actual["app_configs"].as_array().unwrap().iter().filter(|r|r["key"].as_str().unwrap_or_default().starts_with("rust_binance_a12_v1:")).cloned().collect();
        assert_eq!(a12.len(),usize::from(IDENTITY_CASES.contains(&name)),"{name}: exact A12 effects");
        if let Some(marker)=a12.first() {
            assert_eq!(marker["key"],deltabadger::sync::binance_writer::MARKER);assert_eq!(marker["value"],"1");
            assert_eq!(marker["created_at"],deltabadger::sync::sql_time(app.now()));assert_eq!(marker["updated_at"],marker["created_at"]);
            configs.push(marker.clone());
        }
        configs.sort_by_key(|r|r["id"].as_i64());
        same_rows(actual,expected_after,&format!("{name} after"));
        rust_effects(&c,name,key_before);
        if tax { tax_effects(&c, name, &source, dir.path(), expected, &answer.body, &tax_before, &wakes); }
        else { assert!(wakes.iter().all(|(job, _)| job != "tax_report"), "{name}: woke the tax report"); }
    }
}

/// A tax step's request rows, wakes and files against Rails' enqueued Tax::GenerateReportJob and its tmp/: every
/// accepted create inserts one queued row with the job's arguments (`[user, country, year, stablecoin, scope]`) and
/// request locale, wakes `tax_report` once, and names that row on its panel; nothing else moves a row; the files left
/// are Rails' (a download deletes its CSV; a refusal steps aside), Rust's staging names never among them.
#[allow(clippy::too_many_arguments)]
fn tax_effects(c: &Connection, name: &str, source: &Path, dir: &Path, expected: &Value, body: &str, before: &[Value], wakes: &[(String, Option<String>)]) {
    let after = tax_rows(c);
    for row in before { assert!(after.contains(row), "{name}: an earlier request moved: {row}"); }
    let new: Vec<&Value> = after.iter().filter(|row| !before.iter().any(|old| old["request"] == row["request"])).collect();
    let jobs: Vec<&Value> = expected["jobs"].as_array().unwrap().iter().filter(|job| job["job_class"] == "Tax::GenerateReportJob").collect();
    assert_eq!(new.len(), jobs.len(), "{name}: requests queued against Rails' enqueued jobs {jobs:?}");
    for (row, job) in new.iter().zip(&jobs) {
        let request = row["request"].as_str().unwrap();
        assert!(request.len() == 32 && request.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{name}: request token {request}");
        assert_eq!(json!([row["user_id"], row["country"], row["year"], row["stablecoin_as_fiat"] != 0, row["scope"]]), job["args"], "{name}: job arguments");
        assert_eq!(row["locale"], job["locale"], "{name}: job locale");
        assert_eq!(row["state"], "queued", "{name}");
        assert!(body.contains(&format!(r#" data-tax-request="{request}""#)), "{name}: the panel names its request");
    }
    let tax_wakes = wakes.iter().filter(|(job, _)| job == "tax_report").count();
    assert_eq!(tax_wakes, jobs.len(), "{name}: tax_report wakes {wakes:?}");
    assert!(wakes.iter().all(|(job, scope)| job == "tax_report" && scope.is_none()), "{name}: only the unscoped tax_report is woken: {wakes:?}");
    let rails = if name == TAX_RECONCILED { source.join("tmp_before") } else { source.join("tmp") };
    assert_eq!(tax_files(&dir.join("tmp")), tax_files(&rails), "{name}: report files after the request");
    if name == "download_ready" || name == "download_broker" {
        assert!(tax_files(&source.join("tmp")).is_empty() && tax_files(&dir.join("tmp")).is_empty(), "{name}: the downloaded report is deleted on both sides");
    }
}
