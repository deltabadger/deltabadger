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

/// Rails' page with the listed divergences applied (`withheld`), and with no display rate each transaction row's money
/// stated as unavailable (D5a's rule: never a dollar figure where the reader's currency was asked for).
fn withheld_page(name: &str, body: &str) -> String {
    let [_, _, tiles, holdings, _] = rails_regions(body);
    let (new_tiles, new_holdings) = withheld(name, tiles.clone(), holdings.clone());
    let mut out = body.to_string();
    if !tiles.is_empty() { out = out.replacen(&tiles, &new_tiles, 1); }
    out = out.replacen(&holdings, &new_holdings, 1);
    if name == "populated_dead" {
        // The navbar ring withholds balances no current key produced already (S1); Rails draws them.
        let head = out.find("      <circle cx=\"12\" cy=\"12\" r=\"9\" fill=\"none\"\n").unwrap();
        let end = head + out[head..].find("</svg>\n").unwrap();
        out.replace_range(head..end, "    <circle class=\"stroke--icon\" cx=\"12\" cy=\"12\" r=\"9\"\n            fill=\"none\" stroke-width=\"2\"/>\n");
    }
    if name == "populated_fx_unavailable" {
        while let Some(start) = out.find("<tr id=\"account_transaction_") {
            let id: String = out[start + 28..].chars().take_while(char::is_ascii_digit).collect();
            let end = start + out[start..].find("</tr>\n").unwrap() + 6;
            out.replace_range(start..end, &deltabadger::web::tracker::row::unavailable(id.parse().unwrap()).replace("<tr id", "<TR id"));
        }
        out = out.replace("<TR id", "<tr id");
    }
    out
}

/// The request's context, as the router builds it for a signed-in GET of `path`.
fn ctx(app: &deltabadger::web::App, c: &Connection, owner: i64, path: &str, token: &str) -> deltabadger::web::layout::Ctx {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let locale = deltabadger::web::locale::LOCALES.iter().copied().find(|l| route.starts_with(&format!("/{l}/")));
    let route_path = locale.map_or(route.to_string(), |l| route[l.len() + 1..].to_string());
    let query = query.split('&').filter(|p| !p.is_empty()).map(|p| { let (k, v) = p.split_once('=').unwrap_or((p, "")); (k.to_string(), v.to_string()) }).collect();
    let params = Params { full_path: route.into(), fullpath: path.into(), route_path, path_locale: locale, query, form: vec![], json: None, upload: None };
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
            generated = common::recording("pages_tracker_populated", common::has_subdirs, |out| common::record_grid("script/rust/pages_tracker_populated.rb", out));
            generated.path.to_str().unwrap().to_string()
        }
    };
    let selected = std::env::var("D5_POPULATED_CASES").unwrap_or_default();
    let mut names = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap()).filter(|e| e.file_type().unwrap().is_dir()).map(|e| e.file_name().into_string().unwrap()).collect::<Vec<_>>();
    names.sort();
    assert_eq!(names.len(), 76, "the populated grid");
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
        // The pass writes today's snapshot as Rails' job does; the recorded page read none (Rails' fixture computed the
        // ledger alone), so the history the page draws is put back as it was.
        c.execute_batch("CREATE TEMP TABLE kept_whole AS SELECT * FROM portfolio_snapshots; CREATE TEMP TABLE kept_venues AS SELECT * FROM portfolio_venue_snapshots;").unwrap();
        if !matches!(name.as_str(), "populated_cold" | "populated_cold_hidden" | "populated_cold_scope" | "populated_dead") {
            let db = deltabadger::jobs::Db::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(), deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, scenario["secret_key_base"].as_str().unwrap()).unwrap()));
            deltabadger::tracker::jobs::ledger_run(&db, None::<&deltabadger::jobs::data_api::DataApi<deltabadger::venue::http::ScriptedTransport>>, None, owner, &deltabadger::engine::FixedClock(at), Arc::new(move || at), &mut deltabadger::tracker::jobs::Allowance::run()).await.unwrap_or_else(|e| panic!("{name}: ledger pass: {e}"));
        }
        c.execute_batch("DELETE FROM portfolio_snapshots; INSERT INTO portfolio_snapshots SELECT * FROM kept_whole; DELETE FROM portfolio_venue_snapshots; INSERT INTO portfolio_venue_snapshots SELECT * FROM kept_venues; DROP TABLE kept_whole; DROP TABLE kept_venues;").unwrap();
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
        // The page, served: every region, through the router, exactly as a signed-in browser asks for it.
        let (hash,): (String,) = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [owner], |r| Ok((r.get(0)?,))).unwrap();
        let token = deltabadger::web::csrf::new_token();
        let data = session::SessionData { user: Some((owner, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
        let mut browser = common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
        let headers = step["headers"].as_object().unwrap().iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect::<Vec<_>>();
        let answer = browser.send(&app, "GET", step["path"].as_str().unwrap(), None, common::web::Csrf::None, &headers).await;
        assert_eq!(answer.status, 200, "{name}: {}", &answer.body[..answer.body.len().min(300)]);
        let ours = grid::masked(&answer.body, Some(&app), Some(&token));
        let theirs = grid::masked(&withheld_page(&name, expected["body"].as_str().unwrap()), None, None);
        if ours != theirs {
            std::fs::write(Path::new(&root).join(format!("{name}.rust.html")), &ours).unwrap();
            std::fs::write(Path::new(&root).join(format!("{name}.rails.html")), &theirs).unwrap();
            failures.push(name.clone());
        }
    }
    assert!(failures.is_empty(), "{} pages differ (rust/rails pairs written beside the grid):\n{}", failures.len(), failures.join("\n"));
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

const AT: &str = "2026-09-20T02:00:00Z";

/// An Alpaca account whose ledger the scheduler has cached: a dollar deposit and a bitcoin buy, and (`synced`) the
/// balances of a completed sync by the current key.
async fn warm_install(synced: bool) -> (tempfile::TempDir, deltabadger::web::App, i64, std::path::PathBuf) {
    let (dir, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    let at: chrono::DateTime<chrono::Utc> = AT.parse().unwrap();
    common::seed::fresh_stock_jobs(c, at);
    let row = "INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'{}','{}',?8,?8)";
    c.execute(row, rusqlite::params![s.user_id, s.exchange_id, 4, "USD", "1000", None::<&str>, None::<&str>, "2026-09-01 00:00:00"]).unwrap();
    c.execute(row, rusqlite::params![s.user_id, s.exchange_id, 0, "BTC", "0.01", "USD", "600", "2026-09-02 00:00:00"]).unwrap();
    let v = deltabadger::engine::model::credential_version_by_id(c, s.api_key_id).unwrap().unwrap();
    if synced {
        let usd: i64 = c.query_row("SELECT id FROM assets WHERE symbol='USD' ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap_or_else(|_| { c.execute("INSERT INTO assets(external_id,symbol,name,category,created_at,updated_at) VALUES('usd','USD','US Dollar','Currency',?1,?1)", [AT]).unwrap(); c.last_insert_rowid() });
        for (asset, free, price) in [(usd, "400", "1"), (s.btc, "0.01", "60000")] {
            c.execute("INSERT INTO account_balances(user_id,exchange_id,asset_id,free,locked,usd_price,usd_value,priced_at,synced_at,created_at,updated_at) VALUES(?1,?2,?3,?4,0,?5,?4*?5,'2026-09-20 02:00:00','2026-09-20 02:00:00','2026-09-20 02:00:00','2026-09-20 02:00:00')",
                      rusqlite::params![s.user_id, s.exchange_id, asset, free, price]).unwrap();
        }
        c.execute("UPDATE api_keys SET balances_synced_at='2026-09-20 02:00:00' WHERE id=?1", [s.api_key_id]).unwrap();
        deltabadger::engine::model::credential_write(c, &Some(v.clone()), |f| deltabadger::sync::cache::record(f, s.user_id, s.exchange_id, &v, true, at)).unwrap();
    }
    deltabadger::engine::model::credential_write(c, &Some(v.clone()), |f| deltabadger::sync::cache::record_ledger(f, s.api_key_id, &v, at)).unwrap();
    let db = deltabadger::jobs::Db::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(), common::seed::cipher());
    deltabadger::tracker::jobs::ledger_run(&db, None::<&deltabadger::jobs::data_api::DataApi<deltabadger::venue::http::ScriptedTransport>>, None, s.user_id, &deltabadger::engine::FixedClock(at), Arc::new(move || at), &mut deltabadger::tracker::jobs::Allowance::run()).await.unwrap();
    let state = deltabadger::tracker::cache::read(c, s.user_id, None, at).unwrap();
    assert!(matches!(state, deltabadger::tracker::cache::State::Warm(_)), "the ledger is cached");
    let app = common::web::app(dir.path(), common::web::SECRET, TestClock::at(AT));
    let file = dir.path().join("production.sqlite3");
    (dir, app, s.user_id, file)
}

async fn regions_of(app: &deltabadger::web::App, owner: i64, before: impl FnOnce(&Connection) + Send + 'static) -> populated::Regions {
    let page = app.clone();
    let built = app.db(move |c| { before(c); let ctx = ctx(&page, c, owner, "/tracker", "token"); let held = fx::held(&page, c, owner)?; let out = populated::regions(c, &ctx, owner, &held); c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>); out }).await.unwrap();
    match built { populated::Built::Regions(r) => r, populated::Built::Refused(m) => panic!("refused: {m}") }
}

/// A balance sync that commits while the page reads cannot leave it approving one set of balances and stating figures
/// from another: every input is read in one snapshot.
#[tokio::test(flavor = "current_thread")]
async fn a_sync_landing_mid_read_cannot_split_the_page() {
    let (_dir, app, owner, file) = warm_install(true).await;
    let calm = regions_of(&app, owner, |_| {}).await;
    assert!(calm.tiles.contains("<small>$</small>600.00"), "{}", calm.tiles);
    // After the balances were approved and the ledger read, before the figures: the sync's next batch lands, its
    // stamp not yet complete, with a new price for bitcoin.
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook = fired.clone();
    let mid = regions_of(&app, owner, move |c| c.authorizer(Some(move |ctx: rusqlite::hooks::AuthContext<'_>| {
        if matches!(ctx.action, rusqlite::hooks::AuthAction::Read { table_name: "fx_rates", .. }) && !hook.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let w = Connection::open(&file).unwrap();
            w.execute("UPDATE account_balances SET usd_price=90000, usd_value=900 WHERE usd_price=60000", []).unwrap();
            let (key, user, exchange): (i64, i64, i64) = w.query_row("SELECT id,user_id,exchange_id FROM api_keys LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            let v = deltabadger::engine::model::credential_version_by_id(&w, key).unwrap().unwrap();
            deltabadger::engine::model::credential_write(&w, &Some(v.clone()), |f| deltabadger::sync::cache::record(f, user, exchange, &v, false, AT.parse().unwrap())).unwrap();
        }
        rusqlite::hooks::Authorization::Allow
    }))).await;
    assert!(fired.load(std::sync::atomic::Ordering::SeqCst), "the sync landed during the read");
    assert_eq!((mid.tiles, mid.holdings), (calm.tiles, calm.holdings), "the page read one snapshot");
}

/// A ledger can be cached before the first balance sync completes: the card says the account was never synced, and no
/// tile states money the balances decide (money in, value, unrealised, total; fees and realised are the ledger's).
#[tokio::test(flavor = "current_thread")]
async fn a_warm_ledger_before_the_first_balance_sync_withholds_the_money() {
    let (_dir, app, owner, _) = warm_install(false).await;
    let r = regions_of(&app, owner, |_| {}).await;
    assert!(r.holdings.contains(&deltabadger::web::i18n::t("en", "tracker.portfolio.never_synced", &[])), "{}", r.holdings);
    assert_eq!(r.tiles.matches("<span class=\"no-value\" title=\"Tracker figures unavailable: balances not yet synced with the current key\">").count(), 4, "{}", r.tiles);
}
