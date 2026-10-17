//! The decision-parity grids of script/rust/decisions.rb: built and recorded by Rails on scratch databases, played by
//! deltabadger::parity::decide on pristine copies (tests/parity.rs, tests/tick_basket.rs).
use std::path::Path;
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

/// `command`'s grid, as `rails(&[command, into])` builds it: built once per test run (super::run_template), and
/// copied into `into` for every test that asks.
pub fn built_grid(command: &str, into: &Path) {
    let template = super::run_template(&format!("decisions-{command}"), super::has_subdirs,
        |out| rails(&[command, out.to_str().unwrap()]));
    super::copy_tree(&template, into);
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// R5: failure bookkeeping preserves unrelated NULL evidence. The recorded grids expose
/// Rails deleting this pre-existing NULL flag; require Rust to preserve it explicitly.
/// Return a comparison copy of Rails; never rewrite either recorded output or any money field.
pub fn r5_cleanup_expected(rails: &serde_json::Value, rust: &serde_json::Value) -> serde_json::Value {
    const KEY: &str = "missed_quote_amount_was_set";
    let mut expected = rails.clone();
    for row in expected["changes"]["bots"].as_array_mut().unwrap() {
        if row["before"]["transient_data"].get(KEY) != Some(&serde_json::Value::Null)
            || !row["after"]["transient_data"].is_object()
            || row["after"]["transient_data"].get(KEY).is_some() { continue; }
        // Rust left the bot row untouched (a swap that started and ended within the run): the flag kept is all Rails changed.
        if let Some(mine) = rust["changes"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == row["id"]) {
            assert_eq!(mine["before"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 same initial NULL flag");
            assert_eq!(mine["after"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 failure cleanup preserves unrelated NULL flag");
        }
        row["after"]["transient_data"][KEY] = serde_json::Value::Null;
    }
    // A row whose only change was that flag is no change at all once it is kept.
    expected["changes"]["bots"].as_array_mut().unwrap().retain(|row| row["before"] != row["after"]);
    expected
}
