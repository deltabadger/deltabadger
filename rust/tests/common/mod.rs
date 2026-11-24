#![allow(dead_code)] // each test binary uses a different part of this module

#[cfg(unix)]
pub mod decisions;
pub mod expected;
pub mod freeze;
pub use freeze::frozen;
#[cfg(unix)]
pub mod venue_grid;
pub mod html;
pub mod seed;
pub mod schema;
pub mod scripted;
pub mod smtp;
pub mod two_accounts;
pub mod csv_account;
pub mod web;
pub mod v2;

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
use std::sync::OnceLock;

pub fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/v2_vectors.json")).expect("v2_vectors.json parses")
}

pub const INSTALL_FILES: [&str; 2] = ["production.sqlite3", "production_queue.sqlite3"];

/// A fresh install for the general tests: the primary from `store::create_schema` (rust/schema/baseline.sql) with this
/// build's Rust migrations applied as `run` and `serve` apply them at startup; no marker, no seeds. The queue is the
/// 2.x-built one (`v2_install_template`), so takeover paths still read a 2.x queue.
pub fn baseline_install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(v2_install_template().join(INSTALL_FILES[1]), dir.path().join(INSTALL_FILES[1])).unwrap();
    let c = rusqlite::Connection::open(dir.path().join(INSTALL_FILES[0])).unwrap();
    deltabadger::store::create_schema(&c).expect("baseline.sql builds the primary");
    deltabadger::store::test_support::migrate(&c).expect("this build's Rust migrations apply");
    dir
}

/// A fresh copy of an install as 2.x prepares one (`v2_install_unmigrated`), with this build's Rust migrations
/// applied as `run` and `serve` apply them at startup. For the tests whose point is a 2.x-built (2.x) install.
pub fn v2_install() -> tempfile::TempDir {
    let dir = v2_install_unmigrated();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    deltabadger::store::test_support::migrate(&c).expect("this build's Rust migrations apply");
    dir
}

/// A fresh copy of an install exactly as 2.x prepared one (2.x's schema and queue schema loaded by 2.x itself,
/// frozen at the oracle commit: recorded/v2-install), from the template of this test run. No Rust migration applied.
pub fn v2_install_unmigrated() -> tempfile::TempDir {
    let template = v2_install_template();
    let dir = tempfile::tempdir().unwrap();
    for file in INSTALL_FILES {
        std::fs::copy(template.join(file), dir.path().join(file)).unwrap();
    }
    dir
}

/// The test-only probe migration (tests/fixtures/rust_migrations), loaded as build.rs loads rust/migrations.
pub fn probe_migrations() -> Vec<(String, String, String)> {
    deltabadger::store::test_support::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust_migrations")).unwrap()
}
pub const PROBE: &str = "20990101000001";
pub const PROBE_TABLES: &[(&str, deltabadger::store::TableContract)] =
    &[(PROBE, deltabadger::store::TableContract { name: "rust_migration_probe", columns: &[("id", "INTEGER", false)], unique_indexes: &[] })];

/// `list` as a registry's migrations.
pub fn as_migrations(list: &[(String, String, String)]) -> Vec<deltabadger::store::Migration<'_>> {
    list.iter().map(|(version, name, sql)| deltabadger::store::Migration { version, name, sql }).collect()
}

/// The template v2_install copies, built once per test run (see `run_template`).
pub fn v2_install_template() -> &'static Path {
    static TEMPLATE: OnceLock<PathBuf> = OnceLock::new();
    TEMPLATE.get_or_init(|| frozen("v2-install", install_template_is_whole))
}

/// The v2_install template's build, in an explicit base and run, for the tests of the template itself.
pub fn install_template_for(base: &Path, run: &str) -> PathBuf {
    shared_template(base, &format!("v2-install-{run}"), install_template_is_whole, |out| copy_tree(v2_install_template(), out))
}

/// A directory `build` fills once per test RUN and every process of that run then reads (and copies before it
/// writes). nextest runs every test in its own process, so a per-process OnceLock rebuilt the same 2.x output
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

/// A 2.x recording (`recording`): the restored frozen directory, shared by the run.
pub struct Recording { pub path: PathBuf }

/// 2.x's side of a grid, frozen under `name` (`frozen`); the Rust side is always run live. The directory is shared, so
/// a caller copies what it writes to.
pub fn recording(name: &str, is_whole: fn(&Path) -> bool) -> Recording {
    Recording { path: frozen(name, is_whole) }
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

/// A baseline install with the Kraken fixtures seeded.
pub fn install() -> (tempfile::TempDir, deltabadger::store::Opened, seed::Seeded) {
    let dir = baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    (dir, o, s)
}

/// A baseline install with the Alpaca fixtures seeded.
pub fn install_alpaca() -> (tempfile::TempDir, deltabadger::store::Opened, seed::Seeded) {
    let dir = baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    (dir, o, s)
}

/// Named run-ownership divergence witnesses recorded from 2.x at the oracle commit.
pub fn ownership_case(name: &str) -> serde_json::Value {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../fixtures/run_ownership.json")).unwrap();
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
            // The engine's composition resync marker (engine::index::RESYNC_KEY), where 2.x enqueues its job.
            if let Some(marker) = bot["transient_data"].as_object_mut().unwrap().remove("rust_index_resync") {
                assert!(marker["n"].as_i64().is_some_and(|n| n > 0) && marker["at"].is_string(), "invalid resync marker: {marker}");
            }
            // A17-4 r18: the provenance of a membership this engine's composition wrote (engine::index::COMPOSED_KEY): the
            // id the bot follows.
            if let Some(marker) = bot["transient_data"].as_object_mut().unwrap().remove("rust_composed_index") {
                assert!(marker.is_string(), "invalid composition marker: {marker}");
            }
        }
    }
    if let Some(rows) = tables["transactions"].as_array_mut() {
        for row in rows { row.as_object_mut().unwrap().remove("run_identity"); }
    }
    tables
}

/// Fixture F of PLAN-conditions-every-bot: hand-computed money values for the conditions and cap cells.
pub fn conditions_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("../fixtures/conditions_every_bot_expected.json")).expect("conditions_every_bot_expected.json parses")
}

/// D-MCP-COND-KINDS: `body` with update_bot_settings' field descriptions as 2.x records them (protocol::recorded), where
/// this build's schema says which rule fields an index bot takes (protocol::metadata). Each description is JSON-escaped text.
#[allow(dead_code)]
pub fn recorded_rule_kinds(body: &str) -> String {
    use deltabadger::web::mcp::protocol;
    let fields = |v: &serde_json::Value| v["tools"].as_array().unwrap().iter().find(|t| t["name"] == "update_bot_settings").unwrap()["inputSchema"]["properties"].clone();
    let (ours, v2) = (fields(protocol::metadata()), fields(&protocol::recorded()));
    let mut body = body.to_string();
    for (name, schema) in ours.as_object().unwrap() {
        if v2.get(name).is_none() { continue; }
        let (a, b) = (schema["description"].to_string(), v2[name]["description"].to_string());
        if a != b { body = body.replace(a.trim_matches('"'), b.trim_matches('"')); }
    }
    // D-API-ACCOUNT-ID (#710 slice 6): the listed `account_id` property (protocol::ACCOUNT_ID_TOOLS), absent from 2.x's.
    let field = serde_json::json!({"type": ["integer", "null"], "description": protocol::ACCOUNT_ID}).to_string();
    body.replace(&format!(",\"account_id\":{field}"), "").replace(&format!("\"account_id\":{field},"), "")
}

/// Drops a wiremock server from async code. wiremock's Drop runs `futures::executor::block_on(verify())`, and verify
/// takes a tokio RwLock read. If the dropping task has spent its tokio coop budget, that acquire returns Pending and
/// tokio defers its wake to the runtime's scheduler, which never runs again: block_on parks the runtime's thread for
/// good. With the budget lifted for the drop, the acquire goes through.
pub async fn drop_mock_server(server: wiremock::MockServer) {
    tokio::task::unconstrained(async move { drop(server) }).await
}

/// #710 slice 5: the id of (user, exchange)'s number-1 account, made where it has none (a fixture's key was inserted
/// directly, as no Rust write leaves one): the account a balance or venue snapshot row is stored on.
pub fn number_one(c: &rusqlite::Connection, user_id: i64, exchange_id: i64) -> i64 {
    deltabadger::identity::Account::first(c, user_id, exchange_id).unwrap().ensure(c, chrono::Utc::now()).unwrap().id.unwrap()
}

/// #710 slice 9: Coinbase keys as the identity tests send them. An Ed25519 secret (32 bytes, base64) signs every request,
/// so each carries a JWT whose header names its key (`kid`), which `CoinbaseKid` matches.
pub mod coinbase_key {
    use base64::Engine;
    pub const SECRET: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
    pub const PORTFOLIO_A: &str = "5b2c6a1e-8f3d-4e7a-9c0b-1d2e3f4a5b6c";
    pub const PORTFOLIO_B: &str = "9d8e7f6a-5b4c-4d3e-8f2a-1b0c9d8e7f6a";
    pub fn credentials(key: &str) -> deltabadger::crypto::Credentials {
        deltabadger::crypto::Credentials { ibkr: None, key: key.into(), secret: SECRET.into(), passphrase: None, redaction_values: vec![] }
    }
    /// GET /api/v3/brokerage/key_permissions as Coinbase's public docs type it: a read-only key's flags and its portfolio.
    pub fn permissions(uuid: &str, can_trade: bool) -> serde_json::Value {
        serde_json::json!({"can_view": true, "can_trade": can_trade, "can_transfer": false, "portfolio_uuid": uuid, "portfolio_type": "DEFAULT"})
    }
    /// A request signed by key `0`.
    pub struct Kid(pub &'static str);
    impl wiremock::Match for Kid {
        fn matches(&self, r: &wiremock::Request) -> bool {
            let Some(jwt) = r.headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) else { return false };
            let header = jwt.split('.').next().and_then(|h| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(h).ok());
            header.and_then(|h| serde_json::from_slice::<serde_json::Value>(&h).ok()).is_some_and(|h| h["kid"] == self.0)
        }
    }
}
