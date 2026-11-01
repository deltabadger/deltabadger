//! `run`/`serve` create a 3.0 install on an empty STORAGE_DIR (store::bootstrap, PLAN-B1 §6, r6.1, r1.1, r1.7, r2.1),
//! through the executable:
//! - T3: a debug build dies (std::process::abort) at each step named by DELTABADGER_TEST_ABORT; a restart on what
//!   survived (-wal included) ends equal to an uninterrupted bootstrap. Every restart, and the reference, stops at
//!   `after-claim`, so both sides end at the same point.
//! - §6 + r6.1: one test per state of the primary and of the queue; refusals leave every file as it was.
//! - T4: a missing queue is still Missing, on a Rails-built install and on a Rust-born one.
//! - r2.1: handback refuses a Rust-born install and writes nothing. Every other command creates no database file.
//!
//! Install-time stamps differ run to run and are masked, exactly these: the seed `now` (every seed stamp, the
//! bootstrap's app_configs rows, rust_taken_over_at, rust_created_at, and `rails_at` inside the rust_job.* rows, which
//! carry it); app_configs.created_at/updated_at (the rows the claim and the start write: engine_lease, rust_job.*);
//! the lease's `since` (rewritten by the claim); rust_schema_migrations.applied_at; ar_internal_metadata's stamps
//! (common::schema::dump leaves them out).
// The abort points exist only in debug builds; under --release `run` would not die and these tests would wait.
#![cfg(all(unix, debug_assertions))]
mod common;
use common::schema::{dump, next_ids, rows, snapshot, Rows, Tables};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const SECRET: &str = "engine-test-secret"; // common::seed::cipher's, so dump decrypts app_configs
const PRIMARY: &str = "production.sqlite3";
const QUEUE: &str = "production_queue.sqlite3";
const STEPS: [&str; 7] = ["after-queue", "after-wal", "mid-baseline", "mid-seed", "before-commit", "after-commit", "mid-migrate"];

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.args(args).env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", SECRET).env("IBKR_CATALOGUE_URL", "off").stdin(Stdio::null());
    for var in ["DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL", "CACHE_DATABASE_URL", "CABLE_DATABASE_URL",
                "DATABASE_PATH", "QUEUE_DATABASE_PATH", "SMTP_ADDRESS", "MARKET_DATA_URL", "DELTABADGER_TEST_ABORT"] {
        c.env_remove(var);
    }
    c
}

/// The command's output, killed if it is still running after 120 s (a missed abort point must fail, not hang).
fn output(mut c: impl std::borrow::BorrowMut<Command>) -> Output {
    let c = c.borrow_mut();
    let child = c.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let pid = child.id();
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if wait.recv_timeout(std::time::Duration::from_secs(120)).is_err() {
            let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
        }
    });
    let out = child.wait_with_output().unwrap();
    let _ = done.send(());
    watchdog.join().unwrap();
    assert_ne!(out.status.signal(), Some(9), "still running after 120 s:\n{}", said(&out));
    out
}

fn said(out: &Output) -> String { format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)) }

/// `run`, killed at `step`.
fn run_until(dir: &Path, step: &str) -> Output {
    let out = output(command(dir, &["run"]).env("DELTABADGER_TEST_ABORT", step));
    assert_eq!(out.status.signal(), Some(6), "run should die at {step}: {:?}\n{}", out.status, said(&out));
    out
}

/// `run` that is refused before it claims anything.
fn refused(dir: &Path) -> String {
    let out = output(command(dir, &["run"]).env("DELTABADGER_TEST_ABORT", "after-claim"));
    assert_eq!(out.status.code(), Some(1), "run should be refused: {:?}\n{}", out.status, said(&out));
    said(&out)
}

/// Every file in `dir` with a hash of its bytes, but the lock, the -shm index and an empty -wal (a read-only reader of a
/// WAL database creates both; neither holds data).
fn files(dir: &Path) -> BTreeMap<String, String> {
    use sha2::Digest;
    std::fs::read_dir(dir).unwrap().map(|e| e.unwrap())
        .filter(|e| {
            let n = e.file_name().into_string().unwrap();
            n != ".engine.lock" && !n.ends_with("-shm") && !(n.ends_with("-wal") && e.metadata().unwrap().len() == 0)
        })
        .map(|e| (e.file_name().into_string().unwrap(), hex::encode(sha2::Sha256::digest(std::fs::read(e.path()).unwrap())))).collect()
}

/// The database files in `dir` (no -wal/-shm, no lock file).
fn databases(dir: &Path) -> Vec<String> {
    files(dir).into_keys().filter(|n| n.ends_with(".sqlite3")).collect()
}

fn sql(dir: &Path, file: &str, statements: &str) {
    Connection::open(dir.join(file)).unwrap().execute_batch(statements).unwrap();
}

fn plain(c: &Connection, key: &str) -> Option<String> { deltabadger::app_config::get_plain(c, key).unwrap() }

/// The install in `dir` as T1 compares it, structure apart from rows, and its rows (dump) with the install-time stamps
/// masked (the list in the header). Also: integrity and foreign keys hold, the lease is Rust's, the marker is there and
/// the claim wrote no new rust_taken_over_at.
fn installed(dir: &Path) -> (BTreeMap<String, Rows>, Tables, BTreeMap<String, i64>) {
    let c = Connection::open(dir.join(PRIMARY)).unwrap();
    assert_eq!(rows(&c, "PRAGMA integrity_check"), vec![vec![rusqlite::types::Value::Text("ok".into())]]);
    assert!(rows(&c, "PRAGMA foreign_key_check").is_empty(), "foreign keys hold");
    let lease = deltabadger::lease::read(&c, &common::seed::cipher()).unwrap().expect("a lease row");
    assert_eq!(lease["engine"], "rust", "{lease}");
    let created = plain(&c, deltabadger::lease::CREATED_AT).expect("the marker");
    assert_eq!(plain(&c, deltabadger::lease::TAKEN_OVER_AT).as_deref(), Some(created.as_str()), "the first claim continues the bootstrap's ownership");

    let mut structure = snapshot(&c);
    structure.retain(|section, _| !section.ends_with(": rows"));
    let seed_now: String = c.query_row("SELECT created_at FROM exchanges WHERE id = 1", [], |r| r.get(0)).unwrap();
    let millis = chrono::NaiveDateTime::parse_from_str(&seed_now, "%Y-%m-%d %H:%M:%S%.f").unwrap().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    assert_eq!(created, millis, "one instant for the whole bootstrap");
    let columns = |t: &str| -> Vec<String> { common::schema::texts(&c, &format!("SELECT name FROM pragma_table_info('{t}') ORDER BY cid")) };
    let at = |t: &str, col: &str| 1 + columns(t).iter().position(|c| c == col).unwrap();
    let mut tables = dump(&c);
    for rows in tables.values_mut() {
        for cell in rows.iter_mut().flatten().flatten() {
            *cell = cell.replace(&seed_now, "<seed now>").replace(&millis, "<seed now>");
        }
    }
    let (created_at, updated_at, value, key) = (at("app_configs", "created_at"), at("app_configs", "updated_at"), at("app_configs", "value"), at("app_configs", "key"));
    for row in tables.get_mut("app_configs").unwrap() {
        row[created_at] = Some("<stamp>".into());
        row[updated_at] = Some("<stamp>".into());
        if row[key].as_deref() == Some("text:engine_lease") {
            let mut lease: serde_json::Value = serde_json::from_str(row[value].as_ref().unwrap().strip_prefix("text:").unwrap()).unwrap();
            lease["since"] = "<claim>".into();
            row[value] = Some(format!("text:{lease}"));
        }
    }
    let applied = at("rust_schema_migrations", "applied_at");
    for row in tables.get_mut("rust_schema_migrations").unwrap() { row[applied] = Some("<applied>".into()); }
    (structure, tables, next_ids(&c))
}

fn reference() -> (BTreeMap<String, Rows>, Tables, BTreeMap<String, i64>) {
    let dir = tempfile::tempdir().unwrap();
    let out = run_until(dir.path(), "after-claim");
    assert!(said(&out).contains("took over (AfterCrash)"), "{}", said(&out));
    installed(dir.path())
}

fn assert_same(got: &(BTreeMap<String, Rows>, Tables, BTreeMap<String, i64>), want: &(BTreeMap<String, Rows>, Tables, BTreeMap<String, i64>), what: &str) {
    assert_eq!(got.0.keys().collect::<Vec<_>>(), want.0.keys().collect::<Vec<_>>(), "{what}: tables or indexes");
    for (section, rows) in &want.0 { assert_eq!(&got.0[section], rows, "{what}: {section}"); }
    assert_eq!(got.1.keys().collect::<Vec<_>>(), want.1.keys().collect::<Vec<_>>(), "{what}: tables");
    for (table, rows) in &want.1 {
        if let Some(i) = (0..rows.len().max(got.1[table].len())).find(|&i| got.1[table].get(i) != rows.get(i)) {
            panic!("{what}: {table} differs first at row {i}:\n  uninterrupted {:?}\n  restarted     {:?}", rows.get(i), got.1[table].get(i));
        }
    }
    assert_eq!(got.2, want.2, "{what}: next ids");
}

#[test]
fn a_bootstrap_killed_at_any_step_ends_as_an_uninterrupted_one_after_a_restart() {
    let want = reference();
    for step in STEPS {
        let dir = tempfile::tempdir().unwrap();
        run_until(dir.path(), step);
        assert!(dir.path().join(QUEUE).exists(), "{step}: the queue comes first");
        if ["after-queue", "after-wal", "mid-baseline", "mid-seed", "before-commit"].contains(&step) {
            // Nothing committed: `check` sees no install, as on an empty directory.
            let check = output(command(dir.path(), &["check"]));
            assert!(said(&check).contains("there's no Deltabadger data at") && !check.status.success(), "{step}: {}", said(&check));
        }
        let out = run_until(dir.path(), "after-claim");
        assert!(said(&out).contains("took over (AfterCrash)"), "{step}: {}", said(&out));
        assert_same(&installed(dir.path()), &want, step);
        assert_eq!(databases(dir.path()), [PRIMARY, QUEUE], "{step}: two files, no cache or cable");
    }
}

#[test]
fn an_uninterrupted_bootstrap_makes_two_files_owned_by_rust() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_until(dir.path(), "after-claim");
    assert!(said(&out).contains(&format!("set up a new install at {}\n", dir.path().join(PRIMARY).display())), "{}", said(&out));
    assert_eq!(databases(dir.path()), [PRIMARY, QUEUE], "no cache or cable");
    assert!(!dir.path().join("backups").exists(), "a new install has nothing to back up (PLAN-B2 §5)");
    for file in [PRIMARY, QUEUE] {
        let c = Connection::open(dir.path().join(file)).unwrap();
        assert_eq!(c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal", "{file}");
    }
    let q = Connection::open(dir.path().join(QUEUE)).unwrap();
    assert_eq!(q.query_row("SELECT count(*) FROM solid_queue_jobs", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    let c = Connection::open(dir.path().join(PRIMARY)).unwrap();
    assert!(c.query_row("SELECT count(*) FROM tickers", [], |r| r.get::<_, i64>(0)).unwrap() > 10_000, "seeded");
    assert_eq!(c.query_row("SELECT count(*) FROM users", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "no user: /setup");
}

#[test]
fn a_zero_byte_or_empty_primary_is_bootstrapped() {
    for empty in ["0-byte", "empty schema"] {
        let dir = tempfile::tempdir().unwrap();
        match empty {
            "0-byte" => { std::fs::write(dir.path().join(PRIMARY), b"").unwrap(); }
            _ => sql(dir.path(), PRIMARY, "PRAGMA journal_mode=WAL"),
        }
        let out = run_until(dir.path(), "after-claim");
        assert!(said(&out).contains("took over (AfterCrash)"), "{empty}: {}", said(&out));
        installed(dir.path());
    }
}

#[test]
fn a_primary_that_is_not_sqlite_or_not_an_install_is_refused_and_untouched() {
    for (what, make) in [("not SQLite", None), ("no schema_migrations", Some("CREATE TABLE notes (body TEXT); INSERT INTO notes VALUES ('mine')"))] {
        let dir = tempfile::tempdir().unwrap();
        match make {
            None => std::fs::write(dir.path().join(PRIMARY), b"these bytes are not a database, and they are someone's").unwrap(),
            Some(statements) => sql(dir.path(), PRIMARY, statements),
        }
        let before = files(dir.path());
        let said = refused(dir.path());
        if make.is_some() { assert!(said.contains("is not a Deltabadger database"), "{what}: {said}"); }
        assert_eq!(files(dir.path()), before, "{what}: every file as it was, and no queue created");
    }
}

#[test]
fn pending_rust_migrations_after_the_bootstrap_commit_are_resumed() {
    let dir = tempfile::tempdir().unwrap();
    run_until(dir.path(), "after-commit");
    let check = output(command(dir.path(), &["check"]));
    assert!(said(&check).contains("pending Rust migration"), "{}", said(&check));
    let out = run_until(dir.path(), "after-claim");
    assert!(said(&out).contains("applied Rust migration"), "{}", said(&out));
    assert!(!dir.path().join("backups").exists(), "the restart of a new install is Rust's (lease `rust`): no copy");
    let c = Connection::open(dir.path().join(PRIMARY)).unwrap();
    let n: i64 = c.query_row("SELECT count(*) FROM rust_schema_migrations", [], |r| r.get(0)).unwrap();
    assert_eq!(n as usize, deltabadger::store::REGISTRY.migrations.len());
}

/// A bootstrapped install, changed by `change`, is refused by `run` with nothing written.
fn refused_after(change: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    run_until(dir.path(), "after-claim");
    sql(dir.path(), PRIMARY, &format!("{change}; PRAGMA wal_checkpoint(TRUNCATE)"));
    let before = files(dir.path());
    let said = refused(dir.path());
    assert_eq!(files(dir.path()), before, "{change}: nothing written");
    said
}

#[test]
fn a_rails_history_other_than_the_frozen_one_is_refused() {
    let said = refused_after("DELETE FROM schema_migrations WHERE version = (SELECT max(version) FROM schema_migrations)");
    assert!(said.contains("this data is behind this build"), "{said}");
}

#[test]
fn a_rust_migration_this_build_does_not_know_is_unsupported() {
    let said = refused_after("INSERT INTO rust_schema_migrations (version, applied_at) VALUES ('29991231000001', '2999-12-31 00:00:00')");
    assert!(said.contains("this data has migrations this build does not know (29991231000001)"), "{said}");
}

#[test]
fn a_read_only_directory_fails_naming_the_path_and_leaves_nothing() {
    use std::os::unix::fs::PermissionsExt;
    for lock_there in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        if lock_there { std::fs::write(dir.path().join(".engine.lock"), b"").unwrap(); }
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let out = output(command(dir.path(), &["run"]));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", said(&out));
        let named = if lock_there { dir.path().join(QUEUE) } else { dir.path().join(".engine.lock") };
        assert!(said(&out).contains(&named.display().to_string()), "lock there {lock_there}: {}", said(&out));
        assert!(files(dir.path()).is_empty(), "nothing partial: {:?}", files(dir.path()).keys());
    }
}

#[test]
fn a_queue_is_created_when_absent_or_empty_and_reused_when_it_is_an_empty_solid_queue() {
    for (what, make) in [("0-byte", Some("")), ("empty schema", Some("PRAGMA journal_mode=WAL")), ("an empty Solid Queue", None)] {
        let dir = tempfile::tempdir().unwrap();
        match make {
            Some("") => std::fs::write(dir.path().join(QUEUE), b"").unwrap(),
            Some(statements) => sql(dir.path(), QUEUE, statements),
            None => deltabadger::store::create_queue_schema(&Connection::open(dir.path().join(QUEUE)).unwrap()).unwrap(),
        }
        let stamp = |dir: &Path| -> Option<String> {
            Connection::open(dir.join(QUEUE)).unwrap().query_row("SELECT created_at FROM ar_internal_metadata", [], |r| r.get(0)).ok()
        };
        let before = stamp(dir.path());
        run_until(dir.path(), "after-claim");
        installed(dir.path());
        assert!(stamp(dir.path()).is_some(), "{what}: the queue has its schema");
        if make.is_none() { assert_eq!(stamp(dir.path()), before, "{what}: reused, not recreated"); }
    }
}

#[test]
fn a_queue_with_anything_else_in_it_refuses_and_neither_file_is_written() {
    let jobs = "INSERT INTO solid_queue_jobs (class_name, queue_name, created_at, updated_at) VALUES ('Bot::SignalJob', 'default', '2026-11-01', '2026-11-01')";
    for (what, extra) in [("a job", Some(jobs)), ("a foreign table", Some("CREATE TABLE notes (body TEXT)")), ("a foreign database", None)] {
        let dir = tempfile::tempdir().unwrap();
        match extra {
            Some(statements) => {
                deltabadger::store::create_queue_schema(&Connection::open(dir.path().join(QUEUE)).unwrap()).unwrap();
                sql(dir.path(), QUEUE, &format!("{statements}; PRAGMA wal_checkpoint(TRUNCATE)"));
            }
            None => sql(dir.path(), QUEUE, "CREATE TABLE notes (body TEXT); INSERT INTO notes VALUES ('mine')"),
        }
        let before = files(dir.path());
        let said = refused(dir.path());
        let (queue, primary) = (dir.path().join(QUEUE), dir.path().join(PRIMARY));
        assert!(said.contains(&format!("can't set up a new install: {} already has data in it, but {} doesn't exist. If this is an existing install, restore {} or set DATABASE_PATH to it. To start fresh, move {} out of the way.",
                                       queue.display(), primary.display(), primary.display(), queue.display())), "{what}: {said}");
        assert_eq!(files(dir.path()), before, "{what}: no primary created, the queue as it was");
    }
}

/// T4: Rust creates a queue only for a primary it is creating.
#[test]
fn an_install_with_its_queue_missing_is_still_missing() {
    let rails = common::rails_install();
    let rust = tempfile::tempdir().unwrap();
    run_until(rust.path(), "after-claim");
    for (what, dir) in [("Rails-built", rails.path()), ("Rust-born", rust.path())] {
        for suffix in ["", "-wal", "-shm"] { let _ = std::fs::remove_file(dir.join(format!("{QUEUE}{suffix}"))); }
        let missing = deltabadger::store::check(&deltabadger::store::Paths::from_env(&|_| None, dir)).unwrap_err();
        assert!(matches!(&missing, deltabadger::store::StoreError::Missing { path } if path.ends_with(QUEUE)), "{what}: {missing:?}");
        let said = refused(dir);
        assert!(said.contains(&format!("the job queue {} is missing, but this install's data is at {}. Put the queue file back, or set QUEUE_DATABASE_PATH to where it is, then start again.",
                                       dir.join(QUEUE).display(), dir.join(PRIMARY).display())), "{what}: {said}");
        assert!(!dir.join(QUEUE).exists(), "{what}: no queue created for an existing install");
    }
}

/// r2.1: refused before anything is written; the lease stays Rust's and the marker stays.
#[test]
fn handback_refuses_an_install_rust_created() {
    let dir = tempfile::tempdir().unwrap();
    run_until(dir.path(), "after-claim");
    sql(dir.path(), PRIMARY, "PRAGMA wal_checkpoint(TRUNCATE)");
    let before = files(dir.path());
    let out = output(command(dir.path(), &["handback"]));
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("deltabadger: handback refused: this install was created by Deltabadger 3.0, so there is no earlier version to hand it back to.\n"), "{}", said(&out));
    assert_eq!(files(dir.path()), before, "nothing written");
    let c = Connection::open(dir.path().join(PRIMARY)).unwrap();
    assert_eq!(deltabadger::lease::read(&c, &common::seed::cipher()).unwrap().unwrap()["engine"], "rust");
    assert!(plain(&c, deltabadger::lease::CREATED_AT).is_some(), "the marker is kept");
}

#[test]
fn every_other_command_creates_no_database_file() {
    for args in [&["check"][..], &["resolve-withdrawal", "1", "--not-sent"], &["handback"], &["resolve-placement", "1", "--not-placed"], &["sync", "ledger"]] {
        let dir = tempfile::tempdir().unwrap();
        let out = output(command(dir.path(), args));
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", said(&out));
        assert!(said(&out).contains(&format!("there's no Deltabadger data at {} yet. Start \"deltabadger serve\" to set up a new install.", dir.path().join(PRIMARY).display())), "{args:?}: {}", said(&out));
        assert!(files(dir.path()).is_empty(), "{args:?}: {:?}", files(dir.path()).keys());
    }
}
