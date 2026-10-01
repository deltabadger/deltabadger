#![allow(dead_code)] // each test binary uses a different part of this module

pub mod seed;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/ruby_vectors.json")).expect("ruby_vectors.json parses")
}

/// A fresh copy of an install exactly as Rails prepares one: db/schema.rb and db/queue_schema.rb loaded
/// by Rails itself (script/rust/prepare_install.rb). Built once per test binary, so it is never stale.
pub fn rails_install() -> tempfile::TempDir {
    static TEMPLATE: OnceLock<PathBuf> = OnceLock::new();
    let template = TEMPLATE.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rails-install-{}", std::process::id()));
        let boot = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(&root).args(["runner", "script/rust/prepare_install.rb"]).arg(&out).env_remove("DATABASE_URL");
        for db in ["primary", "queue", "cache", "cable"] { // boot Rails against scratch databases only
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", boot.path().display()));
        }
        let run = cmd.output().expect("bin/rails runs");
        assert!(run.status.success(), "script/rust/prepare_install.rb failed:\n{}", String::from_utf8_lossy(&run.stderr));
        out
    });
    let dir = tempfile::tempdir().unwrap();
    for file in ["production.sqlite3", "production_queue.sqlite3"] {
        std::fs::copy(template.join(file), dir.path().join(file)).unwrap();
    }
    dir
}

/// A Rails-prepared install with the Kraken fixtures seeded.
pub fn install() -> (tempfile::TempDir, deltabadger::store::Opened, seed::Seeded) {
    let dir = rails_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    (dir, o, s)
}
