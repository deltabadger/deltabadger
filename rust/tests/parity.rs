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
    rust_kept_the_intent(dir, rust_out)
}

/// One AddOrder sent, no order row, the bot retrying with its intent kept and one placement_ambiguous log.
fn rust_kept_the_intent(dir: &Path, rust_out: &serde_json::Value) -> Result<(), String> {
    if rust_out["sent"].as_array().map(Vec::len) != Some(1) { return Err(format!("Rust must send exactly one AddOrder: {rust_out}")); }
    if !rust_out["changes"]["transactions"].as_array().unwrap().is_empty() { return Err(format!("Rust wrote an order row: {rust_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, intent): (i64, Option<String>) = c.query_row(
        "SELECT status, json_extract(transient_data, '$.rust_placement') FROM bots", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let logged: i64 = c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event = 'placement_ambiguous'", [], |r| r.get(0)).unwrap();
    if (status, intent.is_some(), logged) != (5, true, 1) { return Err(format!("expected retrying with the intent kept and one placement_ambiguous log, got status {status}, intent {intent:?}, {logged} log(s)")); }
    Ok(())
}

/// Both grids' `-unreadable_{price,placed,poll}_{nan,infinity,garbage}` variants: a venue number that is "NaN", "Infinity" or
/// "garbage", in a price, in the placement's answer (Kraken: the placed order's first poll) and in a fill poll. Rust
/// refuses every one as an unreadable answer (ruby::json_to_d): a price fails the tick with no order (retried); the Alpaca
/// placement answer keeps the intent for recovery; a poll changes no row and records no fill. What Rails did, recorded
/// from the grid (Kraken parses with honeymaker's strict BigDecimal(), Alpaca with String#to_d):
/// - price, garbage: both venues read 0 and raise their own "Wrong ask/last price … 0.0": retried, no order (only the
///   message differs from Rust's).
/// - price, NaN or Infinity: both venues accept the non-finite price and sizing raises "comparison of BigDecimal with 0
///   failed": execution_failed, no order.
/// - placed, Kraken (the placed order's first poll): garbage raises in the follow-up job (the row stays waiting); NaN closes
///   the order with amount_exec written as NULL (NaN); Infinity closes it with amount_exec +Inf.
/// - placed, Alpaca (the POST answer): all three are ignored and the order is recorded as submitted.
/// - poll, Kraken (the sweep): garbage raises (execution_failed, no order); NaN closes the waiting order with a NULL
///   amount_exec and, where an amount is still owed, the same tick places another order; Infinity closes it with +Inf
///   (and some ticks then fail "comparison of BigDecimal with 0 failed").
/// - poll, Alpaca (the follow-up): garbage closes the order with a ZERO fill (amount_exec and quote_amount_exec 0, whose
///   amount the next tick buys again); NaN closes it with NULL fills; Infinity closes it with +Inf fills.
const UNREADABLE: &str = "-unreadable_";

fn unreadable_number_refused(name: &str, dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, alpaca: bool) -> Result<(), String> {
    if rails_out == rust_out { return Err("Rails now decides as Rust: drop the listed divergence".into()); }
    let sent = rust_out["sent"].as_array().map_or(0, Vec::len);
    let rows = rust_out["changes"]["transactions"].as_array().unwrap();
    let logged_unreadable = rust_out["changes"]["bot_activity_logs"].to_string().contains("unreadable");
    let ok = if name.contains("-unreadable_price_") {
        sent == 0 && rows.is_empty() && logged_unreadable
    } else if name.contains("-unreadable_placed_") && alpaca {
        return rust_kept_the_intent(dir, rust_out);
    } else if name.contains("-unreadable_placed_") {
        // The order was placed and recorded at placement; its unreadable first poll records nothing on it.
        sent == 1 && rows.len() == 1 && rows[0]["after"]["external_status"] == 0
            && rows[0]["after"]["amount_exec"].is_null() && rows[0]["after"]["quote_amount_exec"].is_null()
            && rust_out["poll_error"].as_str().is_some_and(|e| e.contains("unreadable"))
    } else if alpaca {
        sent == 0 && rows.is_empty() && rust_out["poll_error"].as_str().is_some_and(|e| e.contains("unreadable"))
    } else {
        sent == 0 && rows.is_empty() && logged_unreadable
    };
    if ok { Ok(()) } else { Err(format!("Rust must refuse the number as unreadable: {rust_out}")) }
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_scenario_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 306, "the grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        if DIVERGENCES.iter().any(|v| name.ends_with(&format!("-{v}"))) {
            if let Err(e) = intent_kept(&rust_root.path().join(&name), &rails_out, &rust_out) { failures.push(format!("{name} (listed divergence): {e}")); }
            continue;
        }
        if name.contains(UNREADABLE) {
            if let Err(e) = unreadable_number_refused(&name, &rust_root.path().join(&name), &rails_out, &rust_out, false) { failures.push(format!("{name} (listed divergence): {e}")); }
            continue;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    // The mails are part of what is compared; pin what Rails enqueues, so that two silent sides cannot pass. Checked
    // before the comparison's own verdict, so it holds even while other scenarios differ.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        let expected: Option<&[&str]> = [("-blocking", &["stopped_by_error"][..]), ("-rejected", &["end_of_funds"]), ("-low_funds", &["end_of_funds"]), ("-on_schedule", &[])]
            .into_iter().find(|(suffix, _)| name.ends_with(suffix)).map(|(_, mails)| mails);
        if let Some(expected) = expected { assert_eq!(mailed, expected, "{name}: the mails Rails enqueued"); }
        // The starting-time scenarios must reach Bot::Startable#disable_starting_time!, or they compare nothing new.
        if name.ends_with("-start_time_first_tick") {
            let bot = &rails_out["changes"]["bots"][0]["after"];
            assert_eq!(bot["settings"]["start_time_enabled"], false, "{name}: the first run turns the rule off");
            assert!(rails_out["sent"].as_array().is_some_and(|s| !s.is_empty()), "{name}: the first run buys");
        }
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
        .env("DELTABADGER_PARITY_BIN", env!("CARGO_BIN_EXE_deltabadger"))
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
    // 369 (with #451's unreadable-number variants) and the six mail variants (54 scenarios).
    assert_eq!(dirs.len(), 423, "the Alpaca grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let mut rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        // The Rust engine's mail markers, seeded on the row before Rails ticked it: Rails must hand them back as they were.
        if let Some(left) = rails_out.as_object_mut().unwrap().remove("markers") {
            assert!(name.contains("-markers_survive_"), "{name}: only these scenarios seed markers");
            assert_eq!(left, seeded_markers(), "{name}: Rails changed or dropped a marker");
        } else {
            assert!(!name.contains("-markers_survive_"), "{name}: Rails reported no markers");
        }
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        let listed = if name.contains(UNREADABLE) {
            Some(unreadable_number_refused(&name, &rust_dir, &rails_out, &rust_out, true))
        } else if name.ends_with("-untradable_clock_closed") {
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
    // The comparison below sees the funds notification only if Rails really sends it one way and not the other: a Rust side
    // that suppressed it (or sent it always) then differs in `funds_notified` and fails.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        if name.ends_with("-funds_low_buying_power") { assert_eq!(rails_out["funds_notified"], true, "{name}: Rails must notify"); }
        if name.ends_with("-funds_low_cash_only") { assert_eq!(rails_out["funds_notified"], false, "{name}: Rails must not notify"); }
        if name.ends_with("-poll_partially_filled") { assert_eq!(rails_out["poll_error"], serde_json::Value::Null, "{name}: mid-fill is open, asked again"); }
        if name.ends_with("-insufficient_buying_power") { assert_eq!(rails_out["funds_notified"], true, "{name}: Bot::Failable stamps the budget"); }
        // The mails are compared above, which proves nothing if Rails mails in neither scenario or in both: pin Rails' side.
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        let expected: Option<&[&str]> = [("-funds_low_buying_power", &["end_of_funds"][..]), ("-funds_budget_spent", &[]), ("-funds_budget_reopened", &["end_of_funds"]),
                                         ("-error_budget_spent", &[]), ("-error_budget_reopened", &["notify_about_error"]), ("-insufficient_buying_power", &["end_of_funds"]),
                                         ("-unauthorized_twice", &["stopped_by_error"]), ("-markers_survive_a_tick", &[]), ("-markers_survive_a_failure", &["end_of_funds"]),
                                         ("-on_schedule", &[])].into_iter().find(|(suffix, _)| name.ends_with(suffix)).map(|(_, mails)| mails);
        if let Some(expected) = expected { assert_eq!(mailed, expected, "{name}: the mails Rails enqueued"); }
        if name.ends_with("-error_budget_reopened") { assert_eq!(rails_out["mails"][0]["errors"], serde_json::json!(["qty must be > 0 & <sane>"]), "{name}"); }
        if name.ends_with("-unauthorized_twice") { assert!(rails_out["mails"][0]["errors"][0].as_str().unwrap().starts_with("Alpaca rejected the API key."), "{name}"); }
        if name.ends_with("-retry_reuses_cached_price") {
            assert_eq!(rails_out["sent"].as_array().map(Vec::len), Some(2), "{name}: one failed and one accepted POST");
            assert_eq!(rails_out["sent"][0], rails_out["sent"][1], "{name}: Rails' retry reused its cached price");
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}

/// script/rust/decisions.rb SEEDED_MARKERS.
fn seeded_markers() -> serde_json::Value {
    serde_json::json!({ "rust_funds_mail_pending": { "quote_asset": 2, "stamped_at": "2026-08-31T09:00:00.000Z" },
                        "rust_error_mail_pending": { "unknown": { "error": "an <old> \"error\"", "stamped_at": "2026-08-31T09:00:00.000Z" } },
                        "rust_stopped_mail_pending": { "error": "unauthorized.", "stamped_at": "2026-08-31T09:00:00.000Z" },
                        "rust_limit_mail_pending": { "stamped_at": "2026-08-31T09:00:00.000Z" } })
}

/// The same four markers through Rails' web paths: the settings form on the running bot, a stop, the settings form on
/// the stopped bot. Rails writes `transient_data` back whole from the row it loaded, unknown keys included.
#[test]
fn a_rails_web_save_of_the_bot_keeps_the_mail_markers() {
    let dir = tempfile::tempdir().unwrap();
    rails(&["web-save", dir.path().to_str().unwrap()]);
    let out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("web_save.json")).unwrap()).unwrap();
    // Rails really saved, three times: or the markers' survival proves nothing.
    assert_eq!((out["saved_while_running"].clone(), out["stopped"].clone(), out["saved_while_stopped"].clone()), (serde_json::json!(true), serde_json::json!(true), serde_json::json!(true)), "{out}");
    assert_eq!(out["settings"], serde_json::json!({ "quote_amount": 80.0, "interval": "day", "limit_ordered": true }), "{out}");
    assert_eq!((out["status"].as_str(), out["label"].as_str()), (Some("stopped"), Some("Renamed again")), "{out}");
    assert_eq!(out["markers"], seeded_markers(), "Rails changed or dropped a marker");
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

/// R5: failure bookkeeping preserves unrelated NULL evidence. The recorded grids expose
/// Rails deleting this pre-existing NULL flag; require Rust to preserve it explicitly.
/// Return a comparison copy of Rails; never rewrite either recorded output or any money field.
fn r5_cleanup_expected(rails: &serde_json::Value, rust: &serde_json::Value) -> serde_json::Value {
    const KEY: &str = "missed_quote_amount_was_set";
    let mut expected = rails.clone();
    for row in expected["changes"]["bots"].as_array_mut().unwrap() {
        if row["before"]["transient_data"].get(KEY) != Some(&serde_json::Value::Null)
            || !row["after"]["transient_data"].is_object()
            || row["after"]["transient_data"].get(KEY).is_some() { continue; }
        let mine = rust["changes"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == row["id"]).unwrap();
        assert_eq!(mine["before"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 same initial NULL flag");
        assert_eq!(mine["after"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 failure cleanup preserves unrelated NULL flag");
        row["after"]["transient_data"][KEY] = serde_json::Value::Null;
    }
    expected
}

type Outputs = Vec<(String, PathBuf, serde_json::Value, serde_json::Value)>;

/// Builds `command`'s grid, records Rails over it, and decides every scenario in Rust on a pristine copy, asserting the grid's
/// size so a silently shrinking grid fails: (name, Rust's copy, Rails' output, Rust's output), in name order. The TempDir keeps
/// Rust's copies for the asserters that read them.
async fn grid_outputs(command: &str, expected: usize) -> (tempfile::TempDir, Outputs) {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&[command, rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), expected, "{command} has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    if command == "grid-basket" {
        let normalized = tempfile::tempdir().unwrap();
        let paired: Vec<_> = dirs.iter().filter(|d| d.file_name().unwrap().to_string_lossy().contains("-zero_quote_exec-")).collect();
        assert_eq!(paired.len(), 2);
        for d in &paired { copy_dir(d, &normalized.path().join(d.file_name().unwrap())); }
        rails(&["record-normalized", normalized.path().to_str().unwrap()]);
        for d in &paired {
            let name = d.file_name().unwrap();
            std::fs::copy(normalized.path().join(name).join("rails.json"), rust_root.path().join(name).join("rails-normalized.json")).unwrap();
        }
    }
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut out = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        out.push((name, rust_dir, rails_out, rust_out));
    }
    (rust_root, out)
}

/// `out` without the rows `drop` picks, and without row ids in transactions and bot_activity_logs: one side's extra row shifts
/// every later id on that side.
fn normalised(out: &serde_json::Value, drop: &dyn Fn(&str, &serde_json::Value) -> bool) -> serde_json::Value {
    let mut out = out.clone();
    for table in ["transactions", "bot_activity_logs"] {
        let rows: Vec<serde_json::Value> = out["changes"][table].as_array().unwrap().iter().filter(|r| !drop(table, r)).map(|r| {
            let mut r = r.clone();
            r.as_object_mut().unwrap().remove("id");
            if let Some(after) = r["after"].as_object_mut() { after.remove("id"); }
            r
        }).collect();
        out["changes"][table] = serde_json::Value::Array(rows);
    }
    out
}

fn notional_sum(sent: &[serde_json::Value]) -> f64 { sent.iter().map(|o| o["notional"].as_str().unwrap_or("0").parse::<f64>().unwrap()).sum() }

/// The 5xx leg's mail, listed with its failed row: Rails fails the run, mails notify_about_error for it and spends that mail's
/// day (failure_notifications.unknown); the engine keeps the intent, owes no mail and spends nothing. Each side must show
/// exactly that; the comparison then leaves the mails and that budget out.
fn five_xx_mails(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(serde_json::Value, serde_json::Value), String> {
    let mailed = |out: &serde_json::Value| -> Vec<String> { out["mails"].as_array().into_iter().flatten().filter_map(|m| m["mail"].as_str().map(str::to_string)).collect() };
    if mailed(rails_out) != ["notify_about_error"] { return Err(format!("Rails mailed {:?} for the 5xx leg, not notify_about_error once", mailed(rails_out))); }
    if !mailed(rust_out).is_empty() { return Err(format!("Rust mailed {:?} for an ambiguous leg; it owes none", mailed(rust_out))); }
    let budget = |out: &serde_json::Value| out["changes"]["bots"].as_array().into_iter().flatten()
        .any(|b| !b["after"]["transient_data"]["failure_notifications"]["unknown"].is_null());
    if !budget(rails_out) { return Err("Rails spent no notify_about_error day for the 5xx leg".into()); }
    if budget(rust_out) { return Err("Rust spent a notify_about_error day for an ambiguous leg".into()); }
    let strip = |out: &serde_json::Value| {
        let mut out = out.clone();
        out.as_object_mut().unwrap().remove("mails");
        for b in out["changes"]["bots"].as_array_mut().into_iter().flatten() {
            for side in ["before", "after"] {
                if let Some(t) = b[side]["transient_data"].as_object_mut() { t.remove("failure_notifications"); }
            }
        }
        out
    };
    Ok((strip(rails_out), strip(rust_out)))
}

/// A 5xx on leg k: Rails writes legs 1..k-1 and a failed row for leg k; Rust writes the same k-1 rows, keeps leg k's intent on
/// leg k's own ticker, and logs placement_ambiguous once. Apart from that one row and that one log, everything else (the
/// POSTs, the bot's row with its carry and settings, the members, the logs, the funds notification) must equal Rails'.
fn leg_intent_kept(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, k: usize) -> Result<(), String> {
    let rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().clone();
    let rails_rows = rows(rails_out);
    let failed = rails_rows.iter().filter(|r| r["after"]["status"] == 1).count();
    if rails_rows.len() != k || rails_rows[k - 1]["after"]["status"] != 1 || failed != 1 {
        return Err(format!("Rails no longer writes legs 1..{} and one failed leg {k}: drop the listed divergence\n  rails: {rails_out}", k - 1));
    }
    let ambiguous = |r: &serde_json::Value| r["after"]["event"] == "placement_ambiguous";
    let logged = rust_out["changes"]["bot_activity_logs"].as_array().unwrap().iter().filter(|r| ambiguous(r)).count();
    if logged != 1 { return Err(format!("Rust logged placement_ambiguous {logged} time(s), not once\n  rust: {rust_out}")); }
    let (rails_cmp, rust_cmp) = five_xx_mails(rails_out, rust_out)?;
    let rails = normalised(&rails_cmp, &|t, r| t == "transactions" && r["after"]["status"] == 1);
    let rust = normalised(&rust_cmp, &|t, r| t == "bot_activity_logs" && ambiguous(r));
    if rails != rust { return Err(format!("beyond leg {k}'s failed row and Rust's ambiguity log\n  rails: {rails}\n  rust:  {rust}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, ticker): (i64, Option<i64>) = c.query_row(
        "SELECT status, json_extract(transient_data, '$.rust_placement.ticker_id') FROM bots", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let leg_k: i64 = c.query_row("SELECT id FROM tickers WHERE ticker = ?1", [rust_out["sent"][k - 1]["symbol"].as_str().unwrap()], |r| r.get(0)).unwrap();
    if (status, ticker) != (5, Some(leg_k)) {
        return Err(format!("expected retrying with the intent on ticker {leg_k}; got status {status}, intent ticker {ticker:?}"));
    }
    Ok(())
}

/// A recover-* scenario: leg k was ambiguous, the engine's reconciliation tick settled it as not placed and sent nothing, and
/// at the next checkpoint both sides buy what was not bought. Listed differences, removed before comparing: Rust's
/// placement_ambiguous resolution log; in the 5xx variant also Rails' failed row for leg k and Rust's ambiguity log for it
/// (Rails logs none there). Each is removed only after it is counted exactly once, so a duplicate cannot hide.
fn settled_like_rails(rails_out: &serde_json::Value, rust_out: &serde_json::Value, five_xx: bool) -> Result<(), String> {
    if rust_out["recover_sent"] != 0 { return Err(format!("the reconciliation tick sent {} order(s); it must send none", rust_out["recover_sent"])); }
    let count = |out: &serde_json::Value, table: &str, f: &dyn Fn(&serde_json::Value) -> bool| out["changes"][table].as_array().unwrap().iter().filter(|r| f(&r["after"])).count();
    let resolved = count(rust_out, "bot_activity_logs", &|a| a["event"] == "placement_ambiguous" && a["details"]["resolution"] == "not_placed");
    if resolved != 1 { return Err(format!("Rust logged the not_placed resolution {resolved} time(s), not once\n  rust: {rust_out}")); }
    if five_xx {
        let failed = count(rails_out, "transactions", &|a| a["status"] == 1);
        if failed != 1 { return Err(format!("Rails wrote {failed} failed row(s) for the 5xx leg, not one: drop or revisit the listed difference\n  rails: {rails_out}")); }
        let ambiguity = count(rust_out, "bot_activity_logs", &|a| a["event"] == "placement_ambiguous" && a["details"]["resolution"].is_null());
        if ambiguity != 1 { return Err(format!("Rust logged the 5xx leg's ambiguity {ambiguity} time(s), not once\n  rust: {rust_out}")); }
    }
    let (rails_out, mut rust) = if five_xx { five_xx_mails(rails_out, rust_out)? } else { (rails_out.clone(), rust_out.clone()) };
    let rails_out = &rails_out;
    rust.as_object_mut().unwrap().remove("recover_sent");
    let rust = normalised(&rust, &|t, r| t == "bot_activity_logs" && r["after"]["event"] == "placement_ambiguous"
        && (five_xx || r["after"]["details"]["resolution"] == "not_placed"));
    let rails = normalised(rails_out, &|t, r| five_xx && t == "transactions" && r["after"]["status"] == 1);
    if rails != rust { return Err(format!("\n  rails: {rails}\n  rust:  {rust}")); }
    Ok(())
}

/// A landed ambiguous leg: what it was and what it must show. `k` counts the POSTs of the first tick through the ambiguous
/// one; `pair` is the landed leg's pair, `quote` and `base` the fill the lookup reported; `gap` is what Rails, with no row for
/// the landed order, buys more at the next checkpoint.
struct Landed { k: usize, pair: &'static str, quote: f64, base: f64, gap: f64 }

/// Rust's OTX-L row: status, external status, quote exec, base exec, base asset, the landed pair's asset, the bot's intent.
type LandedRow = (i64, i64, f64, f64, i64, i64, Option<String>);

/// A landed leg recovered by client order id, held to Rails' own decision. `reference` is Rails' output on the scenario
/// `<name>-reference`: the same bot at the next checkpoint, holding the rows the engine holds by then (the accepted legs,
/// swept to filled, and the landed leg as the engine records it). Rust must send exactly Rails' orders there, symbol by
/// symbol and amount by amount; its first `k` POSTs are Rails' own; its reconciliation tick sends nothing; its OTX-L row is
/// on the landed pair, closed, with the lookup's fill; no intent is left. Rails' own run still buys `gap` more: the listed
/// divergence. A wrong split (the whole total in one member, say) fails the comparison with the reference. `reference`
/// carries Rust's copy of the reference scenario too, whose decision equals Rails' there (its own grid cell), for the
/// members' final rows.
fn landed_matches_reference(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, reference: Option<(&Path, &serde_json::Value)>, l: &Landed) -> Result<(), String> {
    let (reference_dir, reference) = reference.ok_or("no -reference scenario for this landed recovery")?;
    if rust_out["recover_sent"] != 0 { return Err(format!("the reconciliation tick sent {} order(s); it must send none", rust_out["recover_sent"])); }
    let (rails_sent, rust_sent, reference_sent) = (rails_out["sent"].as_array().unwrap(), rust_out["sent"].as_array().unwrap(), reference["sent"].as_array().unwrap());
    if rails_sent.len() < l.k || rust_sent.len() < l.k || rails_sent[..l.k] != rust_sent[..l.k] {
        return Err(format!("the first {} POSTs differ\n  rails: {rails_sent:?}\n  rust:  {rust_sent:?}", l.k));
    }
    if reference_sent.is_empty() { return Err("the reference buys nothing, so it proves nothing".into()); }
    if rust_sent[l.k..] != reference_sent[..] {
        return Err(format!("the next checkpoint's orders are not Rails' on the recovered rows\n  reference: {reference_sent:?}\n  rust:      {:?}", &rust_sent[l.k..]));
    }
    // The ledger the next checkpoint writes and the bot it leaves, against the reference. The reference starts from the rows
    // the engine holds, so its new rows are exactly the next checkpoint's: Rust's rows created at or after the reference's
    // first new row must equal them, ids aside (Rust's copy holds more rows, so its ids run ahead). A row the reference
    // updated (an accepted leg, swept to filled) must end with the same fill on Rust's side. The bot must end with the same
    // status, stop key and time, settings and carry (the engine's own keys are outside both snapshots).
    let strip = |r: &serde_json::Value| { let mut a = r["after"].clone(); a.as_object_mut().unwrap().remove("id"); a };
    let rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().clone();
    let reference_new: Vec<serde_json::Value> = rows(reference).iter().filter(|r| r["before"].is_null()).map(strip).collect();
    let Some(since) = reference_new.iter().filter_map(|r| r["created_at"].as_str()).min().map(str::to_string) else {
        return Err("the reference writes no row at the next checkpoint, so it proves nothing".into());
    };
    let rust_new: Vec<serde_json::Value> = rows(rust_out).iter().map(strip).filter(|r| r["created_at"].as_str().is_some_and(|c| c >= since.as_str())).collect();
    if rust_new != reference_new {
        return Err(format!("the next checkpoint's ledger rows are not Rails'\n  reference: {reference_new:?}\n  rust:      {rust_new:?}"));
    }
    let fill = |r: &serde_json::Value| ["status", "external_status", "price", "amount_exec", "quote_amount_exec", "base_asset_id", "order_type"].map(|k| r[k].clone());
    for updated in rows(reference).iter().filter(|r| !r["before"].is_null()) {
        let ext = &updated["after"]["external_id"];
        let mine = rows(rust_out).iter().find(|r| r["after"]["external_id"] == *ext).map(strip);
        if mine.as_ref().map(fill) != Some(fill(&updated["after"])) {
            return Err(format!("order {ext} ends differently\n  reference: {}\n  rust:      {mine:?}", updated["after"]));
        }
    }
    // The logs: the first tick's (and the reconciliation's) are Rails' own run's; the next checkpoint's are the reference's.
    let logs = |out: &serde_json::Value, next: bool| -> Vec<serde_json::Value> {
        out["changes"]["bot_activity_logs"].as_array().unwrap().iter().map(strip)
            .filter(|r| r["created_at"].as_str().is_some_and(|c| (c >= since.as_str()) == next)).collect()
    };
    if logs(rust_out, false) != logs(rails_out, false) {
        return Err(format!("the logs before the next checkpoint are not Rails' own\n  rails: {:?}\n  rust:  {:?}", logs(rails_out, false), logs(rust_out, false)));
    }
    if logs(rust_out, true) != logs(reference, true) {
        return Err(format!("the next checkpoint's logs are not the reference's\n  reference: {:?}\n  rust:      {:?}", logs(reference, true), logs(rust_out, true)));
    }
    // The members as both copies end: weights, membership, exits, and whether the next checkpoint wrote them. Ids and the
    // build's wall-clock stamps aside (a row nothing rewrote keeps updated_at = created_at, read as unwritten).
    let members = |d: &Path| -> Vec<String> {
        let c = rusqlite::Connection::open(d.join("production.sqlite3")).unwrap();
        let mut s = c.prepare("SELECT asset_id, ticker_id, in_index, target_allocation, current_allocation, exited_at, \
                               CASE WHEN updated_at = created_at THEN NULL ELSE updated_at END FROM bot_index_assets ORDER BY asset_id").unwrap();
        s.query_map([], |r| Ok(format!("{:?}", (0..7).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect::<Result<Vec<_>, _>>()?)))
            .unwrap().collect::<Result<_, _>>().unwrap()
    };
    if members(dir) != members(reference_dir) {
        return Err(format!("the members end differently\n  reference: {:?}\n  rust:      {:?}", members(reference_dir), members(dir)));
    }
    let bot = |out: &serde_json::Value| out["changes"]["bots"].as_array().unwrap().first().map(|b| b["after"].clone()).unwrap_or_default();
    let (theirs, mine, rails_own) = (bot(reference), bot(rust_out), bot(rails_out));
    for k in ["status", "stop_message_key", "stopped_at", "settings"] {
        if theirs[k] != mine[k] { return Err(format!("the bot's {k} ends differently\n  reference: {}\n  rust:      {}", theirs[k], mine[k])); }
    }
    // R5's comparison copy requires the unrelated NULL flag to survive the first tick's failure.
    // The reference never failed: now both complete blobs must match, with no broad NULL filtering.
    if mine["transient_data"] != rails_own["transient_data"] || mine["transient_data"] != theirs["transient_data"] {
        return Err(format!("the bot's transient_data ends differently\n  rails:     {}\n  reference: {}\n  rust:      {}",
                           rails_own["transient_data"], theirs["transient_data"], mine["transient_data"]));
    }
    let landed = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().iter().any(|r| r["after"]["external_id"] == "OTX-L");
    if landed(rails_out) { return Err(format!("Rails now records the landed order OTX-L: drop the listed divergence\n  rails: {rails_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let row: Option<LandedRow> = c.query_row(
        "SELECT x.status, x.external_status, x.quote_amount_exec, x.amount_exec, x.base_asset_id, (SELECT base_asset_id FROM tickers WHERE ticker = ?1), \
                (SELECT json_extract(transient_data, '$.rust_placement') FROM bots) FROM transactions x WHERE x.external_id = 'OTX-L'",
        [l.pair], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).ok();
    match row {
        Some((0, 2, q, b, asset, pair_asset, None)) if q == l.quote && b == l.base && asset == pair_asset => {}
        other => return Err(format!("Rust's OTX-L row (status, external status, quote exec, base exec, asset, {} asset, intent) is {other:?}; \
                                     expected submitted, closed, {} and {} on {}, no intent left", l.pair, l.quote, l.base, l.pair)),
    }
    let more = notional_sum(&rails_sent[l.k..]) - notional_sum(&rust_sent[l.k..]);
    // Each side floors each of up to three legs to the cent, so the gap may be off by up to 3 cents; compared in whole cents,
    // since 36.03 − 36.00 is 0.0300000000000011 in f64.
    if ((more - l.gap) * 100.0).round().abs() > 3.0 { return Err(format!("Rails buys {more:.2} more at the next checkpoint, not the landed {:.2}", l.gap)); }
    Ok(())
}

/// On Rust's copy after its ticks, the exited member (SOL) is still an `in_index = 0` row and eligibility admits the bot: an
/// exited member is a holding, never a refusal.
fn exited_member_admitted(dir: &Path) -> Result<(), String> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let exited: i64 = c.query_row("SELECT count(*) FROM bot_index_assets i JOIN tickers t ON t.base_asset_id = i.asset_id \
                                   WHERE t.ticker = 'SOL/USD' AND i.in_index = 0", [], |r| r.get(0)).unwrap();
    let r = deltabadger::engine::eligibility::check_install(&c).map_err(|e| format!("{e:?}"))?;
    if exited != 1 || !r.problems.is_empty() || r.eligible.len() != 1 {
        return Err(format!("expected SOL exited and the bot admitted; got {exited} exited row(s), problems {:?}, eligible {:?}", r.problems, r.eligible));
    }
    Ok(())
}

/// Rails' tie order is undefined: a 50/50 basket may place its two legs in either order. Equal once both sides' orders and new
/// rows are sorted by member.
fn equal_up_to_tie_order(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> bool {
    let canon = |out: &serde_json::Value| {
        let mut o = normalised(out, &|_, _| false);
        let mut sent = o["sent"].as_array().unwrap().clone();
        sent.sort_by_key(|s| s["symbol"].to_string());
        o["sent"] = serde_json::Value::Array(sent);
        let mut rows = o["changes"]["transactions"].as_array().unwrap().clone();
        rows.sort_by_key(|r| r["after"]["base_asset_id"].to_string());
        o["changes"]["transactions"] = serde_json::Value::Array(rows);
        o
    };
    canon(rails_out) == canon(rust_out)
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_basket_grid() {
    let (_copies, outputs) = grid_outputs("grid-basket", 79).await;
    let reference = |name: &str| outputs.iter().find(|(n, ..)| *n == format!("{name}-reference")).map(|(_, d, r, _)| (d.as_path(), r));
    let mut failures = vec![];
    for (name, dir, rails_out, rust_out) in &outputs {
        let k = name.rsplit('-').next().and_then(|s| s.strip_prefix('k')).and_then(|s| s.parse().ok()).unwrap_or(0);
        if name.starts_with("basket-exited-") {
            if let Err(e) = exited_member_admitted(dir) { failures.push(format!("{name}: {e}")); }
        }
        let listed = if name.contains("-zero_quote_exec-") {
            let normalized: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails-normalized.json")).unwrap()).unwrap();
            assert_ne!(&normalized, rails_out, "{name}: the recorded normalization divergence disappeared");
            Some(if &normalized == rust_out { Ok(()) } else { Err(format!("normalized Rails: {normalized}\nRust: {rust_out}")) })
        } else if name.contains("-ambiguous_5xx-") {
            Some(leg_intent_kept(dir, rails_out, rust_out, k))
        } else if name == "basket-exited-intent-tiers-k3-landed" {
            // SOL's leg was ambiguous, then SOL was delisted and exited while its intent was unresolved. The reconciliation
            // still finds the order by client order id and records it against SOL.
            Some(landed_matches_reference(dir, rails_out, rust_out, reference(name), &Landed { k: 3, pair: "SOL/USD", quote: 24.0, base: 0.16, gap: 24.0 }))
        } else if name.ends_with("-landed") {
            Some(landed_matches_reference(dir, rails_out, rust_out, reference(name), &Landed { k: 2, pair: "ETH/USD", quote: 36.0, base: 0.0144, gap: 36.0 }))
        } else if name.starts_with("basket-recover-") && !name.ends_with("-reference") {
            Some(settled_like_rails(rails_out, rust_out, name.ends_with("-5xx")))
        } else {
            None
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out == rust_out => {}
            None if name.contains("-w50-") && equal_up_to_tie_order(rails_out, rust_out) => {
                eprintln!("{name}: the tied legs in the other order (listed divergence: Rails' tie order is undefined)");
            }
            None => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Rails' side must show what each cell exists for, or an equal comparison proves nothing.
    let events = |out: &serde_json::Value| -> Vec<String> {
        out["changes"]["bot_activity_logs"].as_array().unwrap().iter().filter_map(|l| l["after"]["event"].as_str().map(str::to_string)).collect()
    };
    for (name, _, rails_out, _) in &outputs {
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        let sent = rails_out["sent"].as_array().unwrap().len();
        if name.starts_with("basket-minimum-one_below-") { assert!(events(rails_out).contains(&"orders_below_minimum".to_string()), "{name}: {rails_out}"); }
        if name.starts_with("basket-minimum-all_below-") { assert_eq!(rows.iter().filter(|r| r["after"]["status"] == 2).count(), 2, "{name}: two skipped rows"); }
        if name == "basket-untradable-one-market" {
            assert!(rails_out["changes"]["bot_index_assets"].as_array().unwrap().iter().any(|r| r["after"]["in_index"] == 0), "{name}: ETH exited");
        }
        if name == "basket-safety-tiers-pre_send-k2" { assert_eq!(sent, 2, "{name}: no retry after a placed leg"); }
        if name == "basket-safety-tiers-pre_send-k1" { assert!(sent > 1, "{name}: retry_on replays a run that placed nothing"); }
        if name == "basket-sizing-w70-resting_partial-market" { assert!(sent > 0, "{name}: Alpaca's partially_filled is open; the sweep goes on"); }
        if name == "basket-recover-tiers-k2-network" { assert_eq!(sent, 5, "{name}: two in the first tick, three at the next checkpoint"); }
        if name == "basket-exited-holds-tiers" { assert_eq!(sent, 2, "{name}: the exited SOL is never bought"); }
        if name == "basket-exited-intent-tiers-k3-landed" { assert_eq!(sent, 5, "{name}: three legs, then BTC and ETH only"); }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_basket_copy_is_planned_with_every_members_prices() {
    let src = tempfile::tempdir().unwrap();
    let grid = tempfile::tempdir().unwrap();
    rails(&["grid-basket", grid.path().to_str().unwrap()]);
    let one = grid.path().join("basket-sizing-w70-resting-market"); // a 70/30 basket with one resting ETH buy, OOPEN-1
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let pair = |sym: &str, p: f64| (format!("{sym}/USD"), serde_json::json!({
        "quotes": { "quotes": { format!("{sym}/USD"): { "ap": p, "bp": p } } }, "trades": { "trades": { format!("{sym}/USD"): { "p": p } } } }));
    let tickers: serde_json::Map<String, serde_json::Value> = [pair("BTC", 64000.0), pair("ETH", 2500.0)].into_iter().collect();
    let out = tempfile::tempdir().unwrap();
    let n = deltabadger::parity::plan_copy(src.path(), &serde_json::Value::Object(tickers), out.path(), "2026-09-10T00:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(n, 1);
    let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join("bot-1/scenario.json")).unwrap()).unwrap();
    let quotes = &sc["script"]["alpaca"]["GET /v1beta3/crypto/us/latest/quotes"][0]["body"]["quotes"];
    assert!(quotes.get("BTC/USD").is_some() && quotes.get("ETH/USD").is_some(), "every leg reads its own pair: {quotes}");
    assert_eq!(sc["script"]["alpaca"]["GET /v2/orders/OOPEN-1"][0]["body"]["symbol"], "ETH/USD", "a waiting order is answered with its own pair");
    // Executed on both sides, each accepted leg is its own transaction. Both engines deduplicate an accepted order by its
    // external id, so the copy's script answers every placement with a distinct one.
    let bot_dir = out.path().join("bot-1");
    let rails_root = tempfile::tempdir().unwrap();
    copy_dir(&bot_dir, &rails_root.path().join("bot-1"));
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rails_root.path().join("bot-1/rails.json")).unwrap()).unwrap();
    let rust_out = deltabadger::parity::decide(&bot_dir).await.unwrap();
    assert_eq!(rails_out, rust_out, "the copy decides identically");
    let ids: Vec<&str> = rust_out["changes"]["transactions"].as_array().unwrap().iter()
        .filter_map(|r| r["after"]["external_id"].as_str().filter(|e| e.starts_with("OPARITY-"))).collect();
    assert_eq!(rust_out["sent"].as_array().unwrap().len(), 2, "the 70/30 basket buys both members: {rust_out}");
    assert_eq!(ids.len(), 2, "one transaction per accepted leg: {ids:?}");
}

/// Amount-limit scenarios in which Rails mails stopped_by_amount_limit (Bot::Notifyable, deliver_later on every qualifying fill,
/// no dedupe), and so does the engine (notice::LIMIT, written with the fill). Archived: Rails mails although
/// Bot::Lifecycle#stop changes nothing.
const LIMIT_MAILS: [&str; 10] = ["limit-poll-reaches", "limit-poll-reaches-limit_order", "limit-poll-stopped_bot", "limit-poll-archived",
                                 "limit-sweep-reaches", "limit-sweep-reaches-under_floor", "limit-sweep-reaches-twice", "limit-poll-cancelled_partial_reaches",
                                 "limit-poll-cancelled_fractional_reaches", "limit-tick_then_poll"];

/// An unresolved intent counts as spent. Rails writes no row for the ambiguous 60 USD order that in fact landed, so its next
/// checkpoint spends the whole 100 USD cap again (160 sent against the cap); Rust recovers the order by its client order id,
/// counts it, and spends the 40 left (100 sent).
fn overspend_prevented(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, reference: Option<(&Path, &serde_json::Value)>) -> Result<(), String> {
    landed_matches_reference(dir, rails_out, rust_out, reference, &Landed { k: 1, pair: "BTC/USD", quote: 60.0, base: 0.0009375, gap: 60.0 })?;
    let total = |out: &serde_json::Value| notional_sum(out["sent"].as_array().unwrap());
    if total(rails_out) <= 100.0 { return Err(format!("Rails no longer overspends the cap ({:.2} sent): drop the listed divergence", total(rails_out))); }
    if total(rust_out) > 100.0 { return Err(format!("Rust sent {:.2} against a 100 cap", total(rust_out))); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_amount_limit_grid() {
    let (_copies, outputs) = grid_outputs("grid-limit", 45).await;
    let reference = |name: &str| outputs.iter().find(|(n, ..)| *n == format!("{name}-reference")).map(|(_, d, r, _)| (d.as_path(), r));
    let mut failures = vec![];
    for (name, dir, rails_out, rust_out) in &outputs {
        let (mut rails_out, rust_out) = (rails_out.clone(), rust_out.clone());
        let mailed = |out: &serde_json::Value| -> Vec<String> {
            out["mails"].as_array().unwrap_or_else(|| panic!("{name}: no mails reported")).iter().map(|m| m["mail"].as_str().unwrap().to_string()).collect()
        };
        // Rails' side pinned, so that two silent sides cannot pass.
        let expected: &[&str] = if name == "limit-sweep-reaches-twice" {
            &["stopped_by_amount_limit", "stopped_by_amount_limit"] // one per callback
        } else if LIMIT_MAILS.contains(&name.as_str()) {
            &["stopped_by_amount_limit"]
        } else if name.ends_with("-5xx") {
            &["notify_about_error"] // Rails' failed row for the 5xx leg (Bot::Notifyable#notify_about_error)
        } else {
            &[]
        };
        if mailed(&rails_out) != expected { failures.push(format!("{name}: Rails mailed {:?}, expected {expected:?}", mailed(&rails_out))); }
        if name == "limit-sweep-reaches-twice" {
            // Listed divergence: the engine's mail marker is one key, so the two callbacks of one sweep owe one mail.
            if mailed(&rust_out) != ["stopped_by_amount_limit"] { failures.push(format!("{name}: Rust mailed {:?}, expected one", mailed(&rust_out))); }
            rails_out["mails"] = rust_out["mails"].clone();
        }
        let rust_out = &rust_out;
        let listed = if name == "limit-ambiguous_overspend" {
            Some(overspend_prevented(dir, &rails_out, rust_out, reference(name)))
        } else if name.ends_with("-landed") {
            Some(landed_matches_reference(dir, &rails_out, rust_out, reference(name), &Landed { k: 2, pair: "ETH/USD", quote: 36.0, base: 0.0144, gap: 36.0 }))
        } else if (name.starts_with("limit-recover-") && !name.ends_with("-reference")) || name == "limit-ambiguous_not_placed" {
            Some(settled_like_rails(&rails_out, rust_out, name.ends_with("-5xx")))
        } else {
            None
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out == *rust_out => {}
            None => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Rails' side must show what each cell exists for, or an equal comparison proves nothing.
    for (name, _, rails_out, _) in &outputs {
        let bot = rails_out["changes"]["bots"].as_array().unwrap().first().map(|b| b["after"].clone()).unwrap_or_default();
        let sent: Vec<&str> = rails_out["sent"].as_array().unwrap().iter().filter_map(|o| o["notional"].as_str()).collect();
        let skipped = rails_out["changes"]["transactions"].as_array().unwrap().iter().filter(|r| r["after"]["status"] == 2).count();
        if LIMIT_MAILS.contains(&name.as_str()) && name != "limit-poll-archived" {
            assert_eq!((bot["status"].clone(), bot["stop_message_key"].clone()), (serde_json::json!(2), serde_json::json!("bot.settings.extra_amount_limit.amount_spent")), "{name}");
        }
        match name.as_str() {
            "limit-poll-short" => assert_ne!(bot["status"], 2, "{name}: 40 left, not reached"),
            "limit-state-exact" | "limit-state-overspent" => assert!(sent.is_empty() && rails_out["changes"]["bot_activity_logs"].as_array().unwrap().is_empty(), "{name}: no order, no log"),
            "limit-cut-market" | "limit-tally-closed" => assert_eq!(sent, vec!["40.00"], "{name}: cut to what is left"),
            "limit-tally-cancelled_partial" => assert_eq!(sent, vec!["80.00"], "{name}: the 20 filled counts in pending and in the cap"),
            "limit-sweep-reaches-under_floor" => assert_eq!((sent.len(), skipped), (0, 1), "{name}: the run sizes the 0.005 left before Bot::StopJob runs"),
            "limit-sweep-reaches-twice" => assert_eq!(rails_out["changes"]["bot_activity_logs"].as_array().unwrap().iter()
                .filter(|l| l["after"]["event"] == "stopped").count(), 2, "{name}: one StopJob, and one stopped line, per qualifying fill"),
            "limit-state-under_floor" | "limit-under_minimum-market" | "limit-under_minimum-limit" => {
                assert_eq!((sent.len(), skipped), (0, 1), "{name}: skipped, and the bot keeps running");
                assert_ne!(bot["status"], 2, "{name}");
            }
            _ => {}
        }
    }
}

/// Stock grid variants where Rust deliberately decides otherwise than Rails (spec Amendment 2026-10-02b), each asserted below.
const STOCK_DIVERGENCES: [&str; 7] = ["clock_5xx", "clock_unreadable", "clock_certificate", "clock_stale_body", "clock_stale_cache",
                                      "clock_past_next_open", "add_server_error"];

fn events_and_status(dir: &Path) -> Result<(Vec<String>, i64), String> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).map_err(|e| e.to_string())?;
    let mut s = c.prepare("SELECT event FROM bot_activity_logs ORDER BY id").map_err(|e| e.to_string())?;
    let events = s.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok((events, c.query_row("SELECT status FROM bots", [], |r| r.get(0)).map_err(|e| e.to_string())?))
}

/// Rails reads the clock as open and places; Rust places nothing. `parked`: Rust read the fresh clock as closed (no cache);
/// otherwise it retried as transient and ended `retrying` with execution_retrying.
fn clock_failed_closed(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, parked: bool) -> Result<(), String> {
    if rails_out["sent"].as_array().map(Vec::len) != Some(1) { return Err(format!("Rails no longer places on this clock: drop the listed divergence\n  rails: {rails_out}")); }
    if !rust_out["sent"].as_array().ok_or("expected JSON array")?.is_empty() { return Err(format!("Rust sent an order: {rust_out}")); }
    let (events, status) = events_and_status(dir)?;
    let want: &[&str] = if parked { &["market_closed"] } else { &["execution_retrying"] };
    if events != want || (!parked && status != 5) { return Err(format!("Rust: events {events:?}, status {status}")); }
    Ok(())
}

/// Rails parks on a closed clock whose next_open is already past (and would spin on its cache); Rust retries and parks nothing.
fn past_next_open_retried(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let rails_events: Vec<&str> = rails_out["changes"]["bot_activity_logs"].as_array().ok_or("expected JSON array")?.iter().filter_map(|l| l["after"]["event"].as_str()).collect();
    if rails_events != ["market_closed"] { return Err(format!("Rails no longer parks on a past next_open: {rails_out}")); }
    if !rust_out["sent"].as_array().ok_or("expected JSON array")?.is_empty() { return Err(format!("Rust sent an order: {rust_out}")); }
    let (events, status) = events_and_status(dir)?;
    if events != ["execution_retrying"] || status != 5 { return Err(format!("Rust: events {events:?}, status {status}")); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_stock_grid() -> Result<(), Box<dyn std::error::Error>> {
    let rails_root = grid_dir("stock");
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid-stock", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 124, "the stock grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); }
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json"))?)?;
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.map_err(|e| format!("{e:?}"))?;
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        let variant = STOCK_DIVERGENCES.iter().find(|v| name.ends_with(&format!("-{v}")));
        let listed = match variant.copied() {
            Some("add_server_error") => Some(intent_kept(&rust_dir, &rails_out, &rust_out)),
            Some("clock_past_next_open") => Some(past_next_open_retried(&rust_dir, &rails_out, &rust_out)),
            Some("clock_stale_cache") => Some(clock_failed_closed(&rust_dir, &rails_out, &rust_out, true)),
            Some(_) => Some(clock_failed_closed(&rust_dir, &rails_out, &rust_out, false)),
            None => None,
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => {}
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    // The comparison proves parity only where Rails really does what the variant names: pin those facts on Rails' side.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let r: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json"))?)?;
        let events: Vec<&str> = r["changes"]["bot_activity_logs"].as_array().ok_or("expected JSON array")?.iter().filter_map(|l| l["after"]["event"].as_str()).collect();
        if name.ends_with("-closed") { assert_eq!(events, ["market_closed"], "{name}"); }
        if name.ends_with("-split_recent") || name.ends_with("-split_unresolved") { assert_eq!(events, ["dca_skipped_restatement"], "{name}"); }
        if name.ends_with("-sweep_done_for_day") || name.ends_with("-sweep_held") { assert!(!events.contains(&"execution_failed"), "{name}: open, the sweep goes on"); }
        if name.ends_with("-funds_low_buying_power") { assert_eq!(r["funds_notified"], true, "{name}: a stock bot spends buying_power"); }
        if name.ends_with("-funds_low_cash_only") { assert_eq!(r["funds_notified"], false, "{name}"); }
        if name.ends_with("-first_tick") { assert_eq!(r["sent"][0]["time_in_force"], "day", "{name}"); }
    }
    Ok(())
}
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_index_grid() -> Result<(), Box<dyn std::error::Error>> {
    let rails_root = grid_dir("index");
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid-index", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 66, "the index grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); }
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json"))?)?;
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.map_err(|e| format!("{e:?}"))?;
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
        // Pinned on Rails' side, so a vacuous pass is impossible: what each composition names really happened there.
        let members: Vec<&serde_json::Value> = rails_out["changes"]["bot_index_assets"].as_array().ok_or("expected JSON array")?.iter().collect();
        if name.contains("-duplicate-") && name.ends_with("-all_flat") {
            let allocations = members.iter().map(|m| -> Result<_, Box<dyn std::error::Error>> { Ok(f64::from_bits(u64::from_str_radix(m["after"]["target_allocation"]["f"].as_str().ok_or("expected allocation bits")?, 16)?)) }).collect::<Result<Vec<_>, _>>()?;
            assert_eq!(allocations,vec![0.704545,0.295455],"Rails recorded [A,A,B]: {name}");
        }
        if name.contains("-leaver-") {
            assert!(members.iter().any(|m| m["after"]["in_index"] == 0 && m["after"]["exited_at"].is_string()), "{name}: CCC marked out");
            assert!(rails_out["sent"].as_array().ok_or("expected JSON array")?.iter().all(|o| o["side"] == "buy" && o["symbol"] != "CCC"), "{name}: never sold, not bought");
        }
        if name.contains("-index_missing-") { assert!(rails_out["sent"].as_array().ok_or("expected JSON array")?.is_empty(), "{name}"); }
        if name.contains("-incumbent_unpriced-") { assert!(rails_out["sent"].as_array().ok_or("expected JSON array")?.is_empty(), "{name}: the buy stalls"); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    Ok(())
}


mod common;
#[tokio::test(flavor = "current_thread")]
async fn a_stock_and_an_index_copy_are_planned_with_stock_bodies_and_an_open_clock() -> Result<(), Box<dyn std::error::Error>> {
    use common::seed::{self, BotSpec};
    use deltabadger::store::{self, Paths};
    use serde_json::{json, Value};
    use chrono::{DateTime, Utc};
    let src = common::rails_install();
    {
        let o = store::open(&Paths::from_env(&|_| None, src.path())).map_err(|e| format!("{e:?}"))?;
        let s = seed::seed_alpaca(&o.primary, &seed::cipher());
        let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
        seed::add_alpaca_stock(&o.primary, &s, "MSFT");
        seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(aapl, 1.0)]));
        seed::insert_index(&o.primary, "nasdaq-100", &["MSFT.US", "AAPL.US"], &json!({ "MSFT.US": 3.1e12, "AAPL.US": 3.4e12 }));
        seed::index_bot(&o.primary, &s, "nasdaq-100", 2, 0.0, false);
    }
    let tickers = json!({ "AAPL": { "quote": { "symbol": "AAPL", "quote": { "ap": 187.43 } }, "trade": { "symbol": "AAPL", "trade": { "p": 187.41 } } },
                          "MSFT": { "quote": { "symbol": "MSFT", "quote": { "ap": 401.25 } }, "trade": { "symbol": "MSFT", "trade": { "p": 401.2 } } } });
    let out = tempfile::tempdir().unwrap();
    assert_eq!(deltabadger::parity::plan_copy(src.path(), &tickers, out.path(), "2026-09-10T12:00:00Z".parse()?).map_err(|e| format!("{e:?}"))?, 2);
    let mut index_scripts = 0;
    for dir in std::fs::read_dir(out.path()).unwrap() {
        let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir?.path().join("scenario.json"))?)?;
        let a = &sc["script"]["alpaca"];
        assert_eq!(a["GET /v2/stocks/AAPL/quotes/latest"][0]["body"]["quote"]["ap"], 187.43, "{sc}");
        assert_eq!(a["GET /v2/stocks/AAPL/trades/latest"][0]["body"]["trade"]["p"], 187.41, "{sc}");
        let clock = &a["GET /v2/clock"][0]["body"];
        let close: DateTime<Utc> = clock["next_close"].as_str().ok_or("expected JSON string")?.parse()?;
        let tick: DateTime<Utc> = sc["at"].as_str().ok_or("expected JSON string")?.parse()?;
        assert!(clock["is_open"] == true && close > tick, "both engines decide an open session: {sc}");
        if a.get("GET /v2/stocks/MSFT/quotes/latest").is_some() { index_scripts += 1; }
    }
    assert_eq!(index_scripts, 1, "only the index bot prices MSFT (a candidate it may probe)");
    Ok(())
}


struct GridDir { path: PathBuf, _temp: Option<tempfile::TempDir> }
impl GridDir { fn path(&self) -> &Path { &self.path } }
fn grid_dir(name: &str) -> GridDir {
    if let Ok(root) = std::env::var("RUST_GRID_CACHE") {
        // The source text keys the cache, so edited fixtures never reuse old results.
        use std::hash::{Hash, Hasher};
        let mut h=std::collections::hash_map::DefaultHasher::new();
        include_str!("../../script/rust/decisions.rb").hash(&mut h);
        let path=Path::new(&root).join(format!("{name}-{:x}",h.finish()));
        std::fs::create_dir_all(&path).unwrap(); GridDir {path,_temp:None}
    } else { let temp=tempfile::tempdir().unwrap(); GridDir {path:temp.path().into(),_temp:Some(temp)} }
}
