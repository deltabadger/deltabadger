use std::path::{Path, PathBuf};
use std::process::Command;

pub fn rails(args: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL")
            .env("PROXY_KRAKEN", "http://127.0.0.1:9") // any unscripted real call fails fast instead of trading
            .env("SKIP_TEST_DATABASE", "true"); // schema:load in development also purges the repo's storage/test*.sqlite3
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        let out = cmd.output().expect("bin/rails runs");
        assert!(out.status.success(), "bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    };
    // The oracle's own queue/cache/cable databases must exist (ActionJob queries Solid Queue directly,
    // broadcasts write Solid Cable): load every schema into the scratch files first.
    run(&["db:schema:load"]);
    let mut full = vec!["runner", "script/rust/decisions.rb"];
    full.extend_from_slice(args);
    run(&full);
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
