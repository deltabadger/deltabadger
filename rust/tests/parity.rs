#![cfg(unix)] // shells out to bin/rails and sets Unix file modes, so it only builds and runs on Unix
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn rails(args: &[&str]) { if let Err(e) = try_rails(args) { panic!("{e}"); } }

pub fn try_rails(args: &[&str]) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| -> Result<(), String> {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL")
            .env("PROXY_KRAKEN", "http://127.0.0.1:9") // any unscripted real call fails fast instead of trading
            .env("APP_ROOT_URL", "http://localhost:3000") // config/environments/development.rb requires it
            .env("SKIP_TEST_DATABASE", "true"); // schema:load in development also purges the repo's storage/test*.sqlite3
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        let out = cmd.output().expect("bin/rails runs");
        if out.status.success() { Ok(()) } else { Err(format!("bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr))) }
    };
    // The oracle's own queue/cache/cable databases must exist (ActionJob queries Solid Queue directly,
    // broadcasts write Solid Cable): load every schema into the scratch files first.
    run(&["db:schema:load"])?;
    let mut full = vec!["runner", "script/rust/decisions.rb"];
    full.extend_from_slice(args);
    run(&full)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// Grid variants where Rust deliberately decides otherwise than Rails, each asserted on its own terms below.
/// add_service_unavailable: AddOrder answered EService:Unavailable may still have been placed. Rails writes a failed
/// row; Rust keeps the intent and settles it by cl_ord_id recovery, as for a lost reply.
const DIVERGENCES: [&str; 1] = ["add_service_unavailable"];

fn intent_kept(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let failed_rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().iter().filter(|t| t["after"]["status"] == 1).count();
    if failed_rows(rails_out) == 0 { return Err(format!("Rails no longer writes a failed row: drop the listed divergence\n  rails: {rails_out}")); }
    if rust_out["sent"].as_array().map(Vec::len) != Some(1) { return Err(format!("Rust must send exactly one AddOrder: {rust_out}")); }
    if !rust_out["changes"]["transactions"].as_array().unwrap().is_empty() { return Err(format!("Rust wrote an order row: {rust_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, intent): (i64, Option<String>) = c.query_row(
        "SELECT status, json_extract(transient_data, '$.rust_placement') FROM bots", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let logged: i64 = c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event = 'placement_ambiguous'", [], |r| r.get(0)).unwrap();
    if (status, intent.is_some(), logged) != (5, true, 1) { return Err(format!("expected retrying with the intent kept and one placement_ambiguous log, got status {status}, intent {intent:?}, {logged} log(s)")); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_scenario_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 216, "the grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.unwrap();
        if DIVERGENCES.iter().any(|v| name.ends_with(&format!("-{v}"))) {
            if let Err(e) = intent_kept(&rust_root.path().join(&name), &rails_out, &rust_out) { failures.push(format!("{name} (listed divergence): {e}")); }
            continue;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn decide_refuses_anything_but_a_marked_scratch_copy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("scenario.json"), r#"{"bot_id": 1, "at": "2026-09-01T00:00:00Z", "script": {}}"#).unwrap();
    let err = deltabadger::parity::decide(dir.path()).await.unwrap_err();
    assert!(matches!(&err, deltabadger::engine::EngineError::Data(m) if m.contains("parity_scratch")), "{err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_copy_is_planned_per_eligible_bot_at_its_next_checkpoint() {
    let src = tempfile::tempdir().unwrap();
    let rails_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let one = rails_root.path().join("week-market-on_schedule");
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let out = tempfile::tempdir().unwrap();
    let body = serde_json::json!({ "error": [], "result": { "XXBTZEUR": { "a": ["2", "1", "1.000"], "b": ["1", "1", "1.000"], "c": ["1.5", "0.1"], "v": ["12.5", "30.1"], "p": ["1.5", "1.5"], "t": [100, 250], "l": ["1.5", "1.5"], "h": ["1.5", "1.5"], "o": "1.5" } } });
    let n = deltabadger::parity::plan_copy(src.path(), &serde_json::json!({ "XBTEUR": body }), out.path(), "2026-09-10T00:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(n, 1);
    let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join("bot-1/scenario.json")).unwrap()).unwrap();
    assert_eq!(sc["parity_scratch"], true);
    assert_eq!(sc["at"], "2026-09-15T10:00:01.123456Z", "one second after the next weekly checkpoint");
    assert_eq!(sc["script"]["http"]["/0/public/Ticker"][0], body);
}

fn alive(pid: &str) -> bool {
    Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().unwrap().success()
}

#[test]
fn a_killed_or_failed_parity_run_leaves_no_copies_and_no_children() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let src = tempfile::tempdir().unwrap();
    let grid = tempfile::tempdir().unwrap();
    rails(&["grid", grid.path().to_str().unwrap()]);
    let one = grid.path().join("week-market-on_schedule");
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let body = serde_json::json!({ "error": [], "result": { "XXBTZEUR": { "a": ["2", "1", "1.000"], "b": ["1", "1", "1.000"], "c": ["1.5", "0.1"], "v": ["12.5", "30.1"], "p": ["1.5", "1.5"], "t": [100, 250], "l": ["1.5", "1.5"], "h": ["1.5", "1.5"], "o": "1.5" } } });
    let tickers = src.path().join("tickers.json");
    std::fs::write(&tickers, serde_json::json!({ "XBTEUR": body }).to_string()).unwrap();
    // A stand-in `sh` for the script's Rails step: it records its sleeping child's pid and then idles, so the run is
    // still mid-Rails (copies on disk, child alive) when the SIGTERM lands, however fast the real Rails would be.
    for kill_it in [true, false] {
    let fake = tempfile::tempdir().unwrap();
    let pid_file = fake.path().join("pid");
    let sh = fake.path().join("sh");
    std::fs::write(&sh, format!("#!/bin/bash\nsleep 600 &\necho $! > '{}'\n{}\n", pid_file.display(), if kill_it { "wait" } else { "kill $!; sleep 0.3; exit 1" })).unwrap();
    std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut child = Command::new(root.join("script/rust/parity_on_copy.sh")).arg(src.path()).arg(&tickers)
        .env("TMPDIR", tmp.path()).env("PATH", format!("{}:{}", fake.path().display(), std::env::var("PATH").unwrap()))
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !pid_file.exists() {
        assert!(std::time::Instant::now() < deadline && child.try_wait().unwrap().is_none(), "the Rails step never started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::thread::sleep(std::time::Duration::from_millis(200)); // the pid file is written before the line ends
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    if kill_it { assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success()); }
    child.wait().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive(&pid) && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(50)); }
    let still = alive(&pid);
    if still { Command::new("kill").args(["-KILL", &pid]).status().ok(); } // do not leak from a red run
    let left: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    let how = if kill_it { "SIGTERM" } else { "a failing Rails step" };
    assert!(left.is_empty(), "scratch copies survived {how}: {left:?}");
    assert!(!still, "the Rails step's child survived {how}");
    }
}

/// Alpaca grid variants where Rust deliberately decides otherwise than Rails, each asserted on its own terms below.
/// add_server_error, add_unreadable: Alpaca may have placed the order. Rails writes a failed row; Rust keeps the intent and
/// settles it by client_order_id (the spec's 5xx ruling). untradable_clock_closed: Rails asks the stock market's clock for a
/// crypto bot whose ticker went untradable (an empty ticker list is not "all crypto") and parks it until the open; Rust
/// treats crypto as always open and fails the tick as the open-clock case does.
const ALPACA_DIVERGENCES: [&str; 3] = ["add_server_error", "add_unreadable", "untradable_clock_closed"];

fn crypto_ignores_the_stock_clock(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let rails_events: Vec<&str> = rails_out["changes"]["bot_activity_logs"].as_array().unwrap().iter().filter_map(|l| l["after"]["event"].as_str()).collect();
    if !rails_events.contains(&"market_closed") { return Err(format!("Rails no longer parks it behind the clock: drop the listed divergence\n  rails: {rails_out}")); }
    if !rust_out["sent"].as_array().unwrap().is_empty() { return Err(format!("Rust sent an order: {rust_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, error): (i64, String) = c.query_row(
        "SELECT b.status, json_extract(l.details, '$.error') FROM bots b JOIN bot_activity_logs l ON l.bot_id = b.id WHERE l.event = 'execution_failed'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| format!("no execution_failed log: {e}"))?;
    if (status, error.as_str()) != (5, "None of the portfolio's weighted assets trade on Alpaca") { return Err(format!("got status {status}, error {error:?}")); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_alpaca_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid-alpaca", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 288, "the Alpaca grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        let listed = if name.ends_with("-untradable_clock_closed") {
            Some(crypto_ignores_the_stock_clock(&rust_dir, &rails_out, &rust_out))
        } else if ALPACA_DIVERGENCES.iter().any(|v| name.ends_with(&format!("-{v}"))) {
            Some(intent_kept(&rust_dir, &rails_out, &rust_out))
        } else {
            None
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => {}
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    // The comparison above sees the funds notification only if Rails really sends it one way and not the other: a Rust side
    // that suppressed it (or sent it always) then differs in `funds_notified` and fails.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        if name.ends_with("-funds_low_buying_power") { assert_eq!(rails_out["funds_notified"], true, "{name}: Rails must notify"); }
        if name.ends_with("-funds_low_cash_only") { assert_eq!(rails_out["funds_notified"], false, "{name}: Rails must not notify"); }
        if name.ends_with("-poll_partially_filled") { assert_eq!(rails_out["poll_error"], "Order OOPEN-5 status is unknown.", "{name}"); }
        if name.ends_with("-insufficient_buying_power") { assert_eq!(rails_out["funds_notified"], true, "{name}: Bot::Failable stamps the budget"); }
        if name.ends_with("-retry_reuses_cached_price") {
            assert_eq!(rails_out["sent"].as_array().map(Vec::len), Some(2), "{name}: one failed and one accepted POST");
            assert_eq!(rails_out["sent"][0], rails_out["sent"][1], "{name}: Rails' retry reused its cached price");
        }
    }
}

#[test]
fn a_suppressed_funds_notification_is_a_difference() {
    // The grid compares whole outputs: flipping only `funds_notified` must make them unequal.
    let rails = serde_json::json!({ "sent": [], "changes": {}, "funds_notified": true, "poll_error": null });
    let mut rust = rails.clone();
    rust["funds_notified"] = serde_json::json!(false);
    assert_ne!(rails, rust);
}

#[tokio::test(flavor = "current_thread")]
async fn an_alpaca_copy_is_planned_with_alpaca_bodies() {
    let src = tempfile::tempdir().unwrap();
    let grid = tempfile::tempdir().unwrap();
    rails(&["grid-alpaca", grid.path().to_str().unwrap()]);
    let one = grid.path().join("day-market-sweep_rejected_ignored"); // one waiting order, OMKT-1
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let quotes = serde_json::json!({ "quotes": { "BTC/USD": { "ap": 2.0, "bp": 1.0 } } });
    let trades = serde_json::json!({ "trades": { "BTC/USD": { "p": 1.5 } } });
    let out = tempfile::tempdir().unwrap();
    let n = deltabadger::parity::plan_copy(src.path(), &serde_json::json!({ "BTC/USD": { "quotes": quotes, "trades": trades } }), out.path(),
                                           "2026-09-10T00:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(n, 1);
    let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join("bot-1/scenario.json")).unwrap()).unwrap();
    assert_eq!((sc["parity_scratch"].as_bool(), sc["venue"].as_str()), (Some(true), Some("alpaca")));
    assert_eq!(sc["script"]["alpaca"]["GET /v1beta3/crypto/us/latest/quotes"][0]["body"], quotes);
    assert_eq!(sc["script"]["alpaca"]["GET /v2/orders/OMKT-1"][0]["body"]["status"], "accepted", "a waiting order is answered as resting");
}

/// A scenario whose script lacks a path the tick needs must fail the run loudly, on either venue (never reach the network).
#[test]
fn an_unscripted_call_fails_the_run_on_both_venues() {
    for (grid, scenario) in [("grid", "script"), ("grid-alpaca", "alpaca")] {
        let built = tempfile::tempdir().unwrap();
        rails(&[grid, built.path().to_str().unwrap()]);
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(built.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        let root = tempfile::tempdir().unwrap();
        let from = dirs.iter().find(|d| {
            let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("scenario.json")).unwrap()).unwrap();
            sc.get("tick").and_then(|t| t.as_bool()).unwrap_or(true)
        }).unwrap();
        copy_dir(from, &root.path().join("one"));
        let path = root.path().join("one/scenario.json");
        let mut sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if scenario == "script" { sc["script"]["http"] = serde_json::json!({}); } else { sc["script"]["alpaca"] = serde_json::json!({}); }
        std::fs::write(&path, sc.to_string()).unwrap();
        let err = try_rails(&["record", root.path().to_str().unwrap()]).expect_err(&format!("{grid}: an unscripted call must fail the run"));
        assert!(err.contains("unscripted"), "{grid}: {err}");
    }
}
