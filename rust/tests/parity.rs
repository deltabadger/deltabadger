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

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_scenario_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 207, "the grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.unwrap();
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
