#![cfg(unix)] // shells out to bin/rails, as rust/tests/parity.rs does
//! The reference-data jobs write exactly the rows Rails writes, for every scripted data-api payload of the grid
//! (script/rust/reference_data.rb is the Rails half).
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn rails(args: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL").env_remove("MARKET_DATA_URL").env_remove("MARKET_DATA_TOKEN")
            .env("APP_ROOT_URL", "http://localhost:3000").env("SKIP_TEST_DATABASE", "true");
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        let out = cmd.output().expect("bin/rails runs");
        assert!(out.status.success(), "bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    };
    run(&["db:schema:load"]);
    let mut full = vec!["runner", "script/rust/reference_data.rb"];
    full.extend_from_slice(args);
    run(&full);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// Listed divergence: a legacy alpaca_<uuid> stock row. Rails' canonical backfill rewrites it and
/// syncs; Rust refuses before any request or write.
const DIVERGENCES: [&str; 1] = ["stocks-legacy-row"];

fn wrote(out: &Value) -> bool { out["changes"].as_object().unwrap().values().any(|t| !t.as_array().unwrap().is_empty()) }

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_write_the_same_reference_rows_for_every_scripted_payload() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 29, "the grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::reference(&rust_root.path().join(&name)).await.unwrap();
        if DIVERGENCES.contains(&name.as_str()) {
            if !wrote(&rails_out) { failures.push(format!("{name}: Rails no longer writes for a legacy row: drop the listed divergence")); }
            if wrote(&rust_out) || !rust_out["requests"].as_array().unwrap().is_empty() {
                failures.push(format!("{name}: Rust must refuse before any request or write: {rust_out}"));
            }
            continue;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    // Listed divergence: a JSON column's text. ActiveSupport escapes <, > and & (as \u003c and so on) and serde does not;
    // both snapshots compare the parsed JSON, which is equal.
    let weights = |root: &Path| -> String {
        rusqlite::Connection::open(root.join("indices-import/production.sqlite3")).unwrap()
            .query_row("SELECT weights FROM indices WHERE external_id = 'nasdaq-100'", [], |r| r.get(0)).unwrap()
    };
    let (rails_text, rust_text) = (weights(rails_root.path()), weights(rust_root.path()));
    if rails_text == rust_text || !rails_text.contains("\\u0026") {
        failures.push(format!("indices-import: Rails no longer escapes JSON text ({rails_text}): drop the listed divergence"));
    }
    if serde_json::from_str::<Value>(&rails_text).unwrap() != serde_json::from_str::<Value>(&rust_text).unwrap() {
        failures.push(format!("indices-import: the weights JSON differs\n  rails: {rails_text}\n  rust:  {rust_text}"));
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}
