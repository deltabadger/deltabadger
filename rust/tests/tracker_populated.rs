//! The populated tracker index's regions (pending report, venue filters, tiles, holdings, sync warnings) against the
//! pages Rails rendered for the same installs (script/rust/pages_tracker_populated.rb). The page itself stays the 501
//! until the chart and the record are ported, so the regions are compared one by one, byte for byte.
mod common;
#[allow(dead_code)]
#[path = "support/tracker_index_grid.rs"]
mod grid;
use common::web::TestClock;
use deltabadger::web::{auth, session, tracker::{fx, populated}, Params};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

/// The Rails page, cut where its regions begin and end.
fn rails_regions(body: &str) -> [String; 5] {
    let cut = |from: &str, to: &str| -> String {
        let Some(start) = body.find(from) else { return String::new() };
        let end = body[start..].find(to).map_or(body.len(), |i| start + i);
        body[start..end].to_string()
    };
    let stream = body.find("signed-stream-name=\"InVzZXJfMTpzeW5jIg").map(|i| i + body[i..].find('\n').unwrap() + 1).unwrap();
    let pending = body[stream..body.find("  <div class=\"tracker\">").unwrap()].to_string();
    let tiles = if body.contains("<div class=\"widget data-grid data-grid--six\">") { cut("<div class=\"widget data-grid data-grid--six\">", "<div class=\"widget tracker-holdings") } else { String::new() };
    let filters = { let start = body.find("<div class=\"tracker-exchanges\">").unwrap(); let end = ["\n    <div class=\"widget widget--chart\">", "\n    \n"].iter().filter_map(|m| body[start..].find(m)).min().unwrap(); body[start..start + end].to_string() };
    [pending, filters, tiles,
     cut("<div class=\"widget tracker-holdings", "    <div class=\"tracker-record\""), cut("<div id=\"sync-warnings\" data-turbo-permanent>", "\n    <input data-turbo-permanent id=\"hamburger\"")]
}

/// Rails' regions with SPEC's never-a-number rule applied (RULING-D5B2B §4), each a listed money divergence: where
/// Rust withholds an amount the cell states why, and the holdings card is its reason. Balances that are stale, unpriced
/// or not produced by the current key withhold what they decide (money in, value, unrealised, total); a display currency
/// with no rate withholds every amount.
fn withheld(name: &str, tiles: String, holdings: String) -> (String, String) {
    let (reason, all) = match name {
        "populated_dead" => ("Tracker figures unavailable: balances not yet synced with the current key", false),
        "populated_stale" => ("Tracker figures unavailable: stale prices", false),
        "populated_unpriced" => ("Tracker figures unavailable: unpriced holdings", false),
        "populated_fx_unavailable" => ("Currency conversion unavailable", true),
        _ => return (tiles, holdings),
    };
    let marker = "    <div class=\"data-grid__item\"";
    let mut parts: Vec<String> = tiles.split(marker).map(str::to_string).collect();
    for (i, part) in parts.iter_mut().enumerate().skip(1) {
        if !(all || matches!(i, 1 | 2 | 5 | 6)) || part.contains("loader--small") { continue; }
        for tone in ["text-success", "text-danger"] { *part = part.replace(&format!("data-grid__item__value {tone}\">"), "data-grid__item__value \">"); }
        let head = part.find("<div class=\"data-grid__item__value ").unwrap();
        let line = head + part[head..].find('\n').unwrap() + 1;
        let end = line + part[line..].find('\n').unwrap();
        part.replace_range(line..end, &format!("      <span class=\"no-value\" title=\"{reason}\">—</span>"));
    }
    (parts.join(marker), format!("<div class=\"widget tracker-holdings\"><p class=\"tracker-holdings__empty\">{reason}</p></div>\n\n"))
}

/// The request's context, as the router builds it for a signed-in GET of `path`.
fn ctx(app: &deltabadger::web::App, c: &Connection, owner: i64, path: &str, token: &str) -> deltabadger::web::layout::Ctx {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let locale = deltabadger::web::locale::LOCALES.iter().copied().find(|l| route.starts_with(&format!("/{l}/")));
    let route_path = locale.map_or(route.to_string(), |l| route[l.len() + 1..].to_string());
    let query = query.split('&').filter(|p| !p.is_empty()).map(|p| { let (k, v) = p.split_once('=').unwrap_or((p, "")); (k.to_string(), v.to_string()) }).collect();
    let params = Params { full_path: route.into(), fullpath: path.into(), route_path, path_locale: locale, query, form: vec![], json: None };
    let user = auth::User::find(c, owner).unwrap().unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
    let data = session::SessionData { user: Some((owner, hash.chars().take(29).collect())), csrf: Some(token.into()), ..Default::default() };
    let request = axum::http::Request::builder().uri(path).body(axum::body::Body::empty()).unwrap();
    deltabadger::web::layout::Ctx::new(app.clone(), Arc::new(params), session::Session::new(data), auth::Current::SignedIn(Arc::new(user)), "nonce".into(), app.now(), &request)
}

/// The completed stamps a Rust sync leaves for the keys that read (Rails keeps no provenance).
fn stamp(c: &Connection, at: chrono::DateTime<chrono::Utc>) {
    let keys = c.prepare("SELECT id,user_id,exchange_id FROM api_keys WHERE key_type!=1 AND status=1").unwrap()
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    for (id, user, exchange) in keys {
        let v = deltabadger::engine::model::credential_version_by_id(c, id).unwrap().unwrap();
        deltabadger::engine::model::credential_write(c, &Some(v.clone()), |f| { deltabadger::sync::cache::record(f, user, exchange, &v, true, at)?; deltabadger::sync::cache::record_ledger(f, id, &v, at) }).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn populated_regions_match_rails() {
    let generated;
    let root = match std::env::var("D5_POPULATED_GRID") {
        Ok(root) => root,
        Err(_) => {
            let boot = tempfile::tempdir().unwrap();
            generated = tempfile::tempdir().unwrap();
            let root = generated.path().to_str().unwrap().to_string();
            common::rails(boot.path(), "test", &["db:schema:load"]);
            for command in ["grid", "record"] { common::rails(boot.path(), "test", &["runner", "script/rust/pages_tracker_populated.rb", command, &root]); }
            root
        }
    };
    let selected = std::env::var("D5_POPULATED_CASES").unwrap_or_default();
    let mut names = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap()).filter(|e| e.file_type().unwrap().is_dir()).map(|e| e.file_name().into_string().unwrap()).collect::<Vec<_>>();
    names.sort();
    assert_eq!(names.len(), 66, "the populated grid");
    let mut failures = vec![];
    for name in names {
        if !selected.is_empty() && !selected.split(',').any(|s| s == name) { continue; }
        let source = Path::new(&root).join(&name);
        let scenario: Value = serde_json::from_slice(&std::fs::read(source.join("scenario.json")).unwrap()).unwrap();
        let rails: Value = serde_json::from_slice(&std::fs::read(source.join("rails.json")).unwrap()).unwrap();
        let expected = rails["responses"].as_array().unwrap().last().unwrap();
        let step = scenario["steps"].as_array().unwrap().last().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for file in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(source.join(file), dir.path().join(file)).unwrap(); }
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        grid::restore(&c, expected);
        let at: chrono::DateTime<chrono::Utc> = scenario["at"].as_str().unwrap().parse().unwrap();
        stamp(&c, at);
        let owner: i64 = c.query_row("SELECT id FROM users ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
        // The pending report's files, as the scenario's hook wrote them beside Rails.
        let tmp = dir.path().join("tmp");
        let reports = tmp.join("tax_reports");
        std::fs::create_dir_all(&reports).unwrap();
        let csv = reports.join(format!("{owner}_DE_2025_broker.csv"));
        if name == "populated_report_refused" { std::fs::write(format!("{}.refused.json", csv.display()), r#"{"reason":"tokenized_unsupported","symbols":["TSLAx","NVDAx"]}"#).unwrap(); }
        if name == "populated_report_ready" { std::fs::write(&csv, "date,amount\n").unwrap(); }
        // The ledger the scheduler would have cached by now (none where Rails has none cached either).
        if !matches!(name.as_str(), "populated_cold" | "populated_cold_hidden" | "populated_cold_scope" | "populated_dead") {
            let db = deltabadger::jobs::Db::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(), deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, scenario["secret_key_base"].as_str().unwrap()).unwrap()));
            deltabadger::tracker::jobs::ledger_run(&db, None::<&deltabadger::jobs::data_api::DataApi<deltabadger::venue::http::ScriptedTransport>>, None, owner, &deltabadger::engine::FixedClock(at), Arc::new(move || at), &mut deltabadger::tracker::jobs::Allowance::run()).await.unwrap_or_else(|e| panic!("{name}: ledger pass: {e}"));
        }
        let env = { let secret = scenario["secret_key_base"].as_str().unwrap().to_string(); let tmp = tmp.display().to_string(); move |n: &str| match n { "SECRET_KEY_BASE" => Some(secret.clone()), "APP_TMP_DIR" => Some(tmp.clone()), _ => None } };
        let opened = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
        let app = deltabadger::web::App::new(deltabadger::web::Config::from_env(&env).unwrap(), &env, opened.primary, TestClock::at(scenario["at"].as_str().unwrap())).unwrap();
        let rates = if name == "populated_fx_unavailable" { json!({}) } else { json!({"usd":{"value":100.0},"eur":{"value":80.0},"gbp":{"value":50.0}}) };
        let app = app.with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({"GET 127.0.0.1:1/api/v1/exchange_rates":{"body":{"data":rates}}}))).unwrap();
        let raw: String = c.query_row("SELECT display_currency FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
        let wanted = deltabadger::figures::totals::normalized_currency(&raw);
        if wanted != "USD" {
            let (identity, config) = app.db(fx::provider).await.unwrap();
            fx::fill(&app, &identity, config, vec![("USD".into(), wanted)]).await.unwrap();
        }
        let token = deltabadger::web::csrf::new_token();
        let path = step["path"].as_str().unwrap().to_string();
        let built = { let app2 = app.clone(); let token = token.clone(); app.db(move |c| { let ctx = ctx(&app2, c, owner, &path, &token); let held = fx::held(&app2, c, owner)?; populated::regions(c, &ctx, owner, &held) }).await.unwrap() };
        let populated::Built::Regions(r) = built else { panic!("{name}: refused") };
        let warnings = format!("<div id=\"sync-warnings\" data-turbo-permanent>\n    {}\n  </div>\n\n    </div>\n", r.warnings);
        let ours = [r.pending, r.filters, r.tiles, r.holdings, warnings];
        let [pending, filters, tiles, holdings, warns] = rails_regions(expected["body"].as_str().unwrap());
        let (tiles, holdings) = withheld(&name, tiles, holdings);
        let theirs = [pending, filters, tiles, holdings, warns];
        for ((label, ours), theirs) in ["pending", "filters", "tiles", "holdings", "warnings"].iter().zip(ours).zip(theirs) {
            let ours = grid::masked(&ours, None, None);
            let theirs = grid::masked(&theirs, None, None);
            if ours != theirs {
                std::fs::write(Path::new(&root).join(format!("{name}.{label}.rust.html")), &ours).unwrap();
                std::fs::write(Path::new(&root).join(format!("{name}.{label}.rails.html")), &theirs).unwrap();
                failures.push(format!("{name} {label}"));
            }
        }
    }
    assert!(failures.is_empty(), "{} regions differ (rust/rails pairs written beside the grid):\n{}", failures.len(), failures.join("\n"));
}

/// The Rails files the grid was recorded from: a change to one is recorded again with the Rust side in the same change.
#[test]
fn populated_sources_and_ci_are_pinned() {
    use sha2::{Digest, Sha256};
    let sources: Value = serde_json::from_str(include_str!("fixtures/tracker_populated_sources.json")).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in sources.as_object().unwrap() { assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}: record the populated grid again"); }
    let workflow = include_str!("../../.github/workflows/rust.yml");
    for path in ["script/rust/pages_tracker_populated.rb", "app/jobs/tax/generate_report_job.rb"] { assert_eq!(workflow.matches(&format!("- '{path}'")).count(), 2, "both CI events cover {path}"); }
}
