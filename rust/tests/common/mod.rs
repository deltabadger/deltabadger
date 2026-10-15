#![allow(dead_code)] // each test binary uses a different part of this module

pub mod html;
pub mod seed;
pub mod scripted;
pub mod smtp;
pub mod web;

type OrderCommitHook = Box<dyn FnMut() -> rusqlite::Result<()>>;
thread_local! {
    static ORDER_COMMIT_HOOK: std::cell::RefCell<Option<OrderCommitHook>> = const { std::cell::RefCell::new(None) };
}

/// Pause after a submitted order commits and releases SQLite's write lock, before the tick
/// continues. The competing write uses its own connection, so the barrier cannot bypass a lock.
pub fn after_order_commit(c: &rusqlite::Connection, bot_id: i64, action: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<()> + 'static) {
    let path = c.path().unwrap().to_owned();
    let mut action = Some(action);
    ORDER_COMMIT_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
        if action.is_none() { return Ok(()) }
        let writer = rusqlite::Connection::open(&path)?;
        let recorded: bool = writer.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND status=0)", [bot_id], |r|r.get(0))?;
        if recorded { action.take().unwrap()(&writer)?; }
        Ok(())
    })));
    c.wal_hook(Some(|_, _| ORDER_COMMIT_HOOK.with(|hook| {
        hook.borrow_mut().as_mut().map_or(Ok(()), |work| work())
    })));
}

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

pub const INSTALL_FILES: [&str; 2] = ["production.sqlite3", "production_queue.sqlite3"];

/// A fresh copy of an install exactly as Rails prepares one: db/schema.rb and db/queue_schema.rb loaded
/// by Rails itself (script/rust/prepare_install.rb), from the template of this test run.
pub fn rails_install() -> tempfile::TempDir {
    let template = rails_install_template();
    let dir = tempfile::tempdir().unwrap();
    for file in INSTALL_FILES {
        std::fs::copy(template.join(file), dir.path().join(file)).unwrap();
    }
    dir
}

/// The template rails_install copies, built once per test run (see `run_template`).
pub fn rails_install_template() -> &'static Path {
    static TEMPLATE: OnceLock<PathBuf> = OnceLock::new();
    TEMPLATE.get_or_init(|| run_template("rails-install", install_template_is_whole, prepare_install))
}

/// The rails_install template's build, in an explicit base and run, for the tests of the template itself.
pub fn install_template_for(base: &Path, run: &str) -> PathBuf {
    shared_template(base, &format!("rails-install-{run}"), install_template_is_whole, prepare_install)
}

/// A directory `build` fills once per test RUN and every process of that run then reads (and copies before it
/// writes). nextest runs every test in its own process, so a per-process OnceLock rebuilt the same Rails output
/// once per test. Keyed by NEXTEST_RUN_ID, it is never reused by a later run, so it is never stale; without a run
/// id (cargo test) it is per process, as before.
pub fn run_template(name: &str, is_whole: fn(&Path) -> bool, build: impl FnOnce(&Path)) -> PathBuf {
    let run = std::env::var("NEXTEST_RUN_ID").unwrap_or_else(|_| format!("pid-{}", std::process::id()));
    shared_template(&Path::new(env!("CARGO_TARGET_TMPDIR")).join("run-templates"), &format!("{name}-{run}"), is_whole, build)
}

/// `<base>/<name>`, built under `<base>/<name>.lock`: one process builds it in a staging directory, marks it
/// complete and publishes it with a rename, so a template that exists is a complete one; the others wait on the
/// lock and then read it. A build killed part-way leaves only its staging directory, which the next builder
/// removes. A published template is still checked before it is handed out, and rebuilt if it fails the check.
pub fn shared_template(base: &Path, name: &str, is_whole: fn(&Path) -> bool, build: impl FnOnce(&Path)) -> PathBuf {
    let out = base.join(name);
    let whole = |dir: &Path| dir.join(".complete").is_file() && is_whole(dir);
    std::fs::create_dir_all(base).unwrap();
    let lock = std::fs::File::create(base.join(format!("{name}.lock"))).unwrap();
    lock.lock().unwrap(); // released when `lock` drops, or when the process dies
    if whole(&out) { return out; }
    let _ = std::fs::remove_dir_all(&out);
    for entry in std::fs::read_dir(base).unwrap().flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with(&format!("{name}.staging-"));
        if stale { let _ = std::fs::remove_dir_all(entry.path()); } // a killed builder's; we hold the lock, so no one is writing it
    }
    sweep_old_templates(base);
    let staging = base.join(format!("{name}.staging-{}", std::process::id()));
    std::fs::create_dir_all(&staging).unwrap();
    build(&staging);
    assert!(is_whole(&staging), "the build of {name} left an incomplete template in {}", staging.display());
    std::fs::write(staging.join(".complete"), b"").unwrap();
    std::fs::rename(&staging, &out).unwrap();
    out
}

fn prepare_install(out: &Path) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let boot = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(root.join("bin/rails"));
    cmd.current_dir(&root).args(["runner", "script/rust/prepare_install.rb"]).arg(out).env_remove("DATABASE_URL")
        .env("APP_ROOT_URL", "http://localhost:3000"); // config/environments/development.rb requires it; a checkout may have no .env
    for db in ["primary", "queue", "cache", "cable"] { // boot Rails against scratch databases only
        cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", boot.path().display()));
    }
    let run = cmd.output().expect("bin/rails runs");
    assert!(run.status.success(), "script/rust/prepare_install.rb failed:\n{}", String::from_utf8_lossy(&run.stderr));
}

/// Both databases there, intact, each with its schema loaded, and nothing left in a -wal file.
fn install_template_is_whole(dir: &Path) -> bool {
    INSTALL_FILES.iter().all(|file| {
        let path = dir.join(file);
        let wal = dir.join(format!("{file}-wal"));
        if !path.is_file() || std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) { return false; }
        let Ok(c) = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return false };
        let ok = c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)).is_ok_and(|s| s == "ok");
        let migrated = c.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'ar_internal_metadata'", [], |r| r.get::<_, i64>(0)).is_ok_and(|n| n == 1);
        ok && migrated
    })
}

/// Templates, locks and staging directories of runs that ended more than a day ago.
// ponytail: age-based sweep; a run longer than a day would lose its template mid-run and rebuild it.
fn sweep_old_templates(base: &Path) {
    let day = std::time::Duration::from_secs(24 * 3600);
    for entry in std::fs::read_dir(base).unwrap().flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day));
        if old {
            if entry.path().is_dir() { let _ = std::fs::remove_dir_all(entry.path()); } else { let _ = std::fs::remove_file(entry.path()); }
        }
    }
}

/// A grid template is whole once it holds a scenario directory (the rename publishes it complete).
pub fn has_subdirs(dir: &Path) -> bool { dir.read_dir().is_ok_and(|mut e| e.any(|e| e.is_ok_and(|e| e.path().is_dir()))) }

/// Copies every file under `from` into `to` (APFS clones them), keeping the tree's shape.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (path, dest) = (entry.path(), to.join(entry.file_name()));
        if path.is_dir() { copy_tree(&path, &dest) } else if entry.file_name() != ".complete" { std::fs::copy(&path, &dest).unwrap(); }
    }
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

/// Named run-ownership divergence witnesses recorded from unchanged main Rails.
/// Pin both sides: a future Rails fix must update the explicitly stricter comparison.
pub fn ownership_case(name: &str) -> serde_json::Value {
    use sha2::{Digest, Sha256};
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../fixtures/run_ownership.json")).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in fixture["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}: re-record run-ownership witnesses from Rails");
    }
    fixture["cases"].get(name).unwrap().clone()
}

/// Exclude only Rust's protocol metadata from business-row parity. Exact generations,
/// attribution, durability and cleanup are asserted in placement, tick_basket and web_actions.
/// Never remove settings, carry, statuses, orders, or activity from this comparison.
pub fn ownership_business_rows(mut tables: serde_json::Value) -> serde_json::Value {
    if let Some(bots) = tables["bots"].as_array_mut() {
        for bot in bots {
            for key in ["run_generation", "tick_generation"] {
                if let Some(value) = bot["transient_data"].as_object_mut().unwrap().remove(key) {
                    assert!(value.as_i64().is_some_and(|n| n >= 0), "invalid ownership counter: {value}");
                }
            }
        }
    }
    if let Some(rows) = tables["transactions"].as_array_mut() {
        for row in rows { row.as_object_mut().unwrap().remove("run_identity"); }
    }
    tables
}
