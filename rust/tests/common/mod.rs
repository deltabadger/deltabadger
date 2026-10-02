#![allow(dead_code)] // each test binary uses a different part of this module

pub mod html;
pub mod seed;
pub mod scripted;
pub mod web;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/ruby_vectors.json")).expect("ruby_vectors.json parses")
}

/// Runs `bin/rails <args>` in the repo against scratch databases only: every *_DATABASE_URL points at a
/// file under `scratch`, and SKIP_TEST_DATABASE keeps `db:schema:load` off the repo's own test files.
/// APP_ROOT_URL is given because config/environments/development.rb requires it and a checkout may have no .env.
pub fn rails(scratch: &Path, rails_env: &str, args: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut cmd = Command::new(root.join("bin/rails"));
    cmd.current_dir(root).args(args).env_remove("DATABASE_URL")
        .env("RAILS_ENV", rails_env).env("SKIP_TEST_DATABASE", "true").env("APP_ROOT_URL", "http://localhost:3000");
    for db in ["primary", "queue", "cache", "cable"] {
        cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.display()));
    }
    let out = cmd.output().expect("bin/rails runs");
    assert!(out.status.success(), "bin/rails {args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
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
        cmd.current_dir(&root).args(["runner", "script/rust/prepare_install.rb"]).arg(&out).env_remove("DATABASE_URL")
            .env("APP_ROOT_URL", "http://localhost:3000"); // config/environments/development.rb requires it; a checkout may have no .env
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

/// A Rails-prepared install with the Alpaca fixtures seeded.
pub fn install_alpaca() -> (tempfile::TempDir, deltabadger::store::Opened, seed::Seeded) {
    let dir = rails_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    (dir, o, s)
}
