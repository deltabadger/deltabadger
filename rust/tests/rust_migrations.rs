//! Rust's own schema migrations (store::check / migrate), proved with the test-only probe migration in
//! tests/fixtures/rust_migrations; the production registry (rust/migrations) is checked against its contracts too.
mod common;
use common::{as_migrations, probe_migrations, PROBE, PROBE_TABLES};
use deltabadger::lease::{self, LeaseError};
use deltabadger::store::{self, test_support, Migration, Paths, Registry, Report, StoreError, MIGRATIONS, REGISTRY, RUST_MIGRATIONS};
use rusqlite::Connection;
use std::path::Path;
use std::process::Command;

fn paths(dir: &Path) -> Paths { Paths::from_env(&|_| None, dir) }
fn exec(path: &Path, sql: &str) { Connection::open(path).unwrap().execute_batch(sql).unwrap(); }
fn lock(p: &Paths) -> lease::EngineLock { lease::lock(p, chrono::Utc::now()).unwrap() }
fn recorded(p: &Paths) -> Vec<String> {
    let c = Connection::open(&p.primary).unwrap();
    let mut s = c.prepare("SELECT version FROM rust_schema_migrations ORDER BY version").unwrap();
    s.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}
fn has_table(p: &Paths, name: &str) -> bool {
    Connection::open(&p.primary).unwrap()
        .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |r| r.get::<_, i64>(0)).unwrap() == 1
}
fn pending(list: &[&str]) -> Report { Report { pending_rust: list.iter().map(|v| v.to_string()).collect() } }
/// Every byte and modification time of both database files, and of a -wal file with frames in it. (A reader of a WAL
/// database opens an empty -wal and maps a -shm, SQLite's shared-memory index; neither holds data.)
fn snapshot(p: &Paths) -> Vec<(Vec<u8>, Option<std::time::SystemTime>)> {
    [&p.primary, &p.queue].iter().flat_map(|f| [f.to_path_buf(), Path::new(&format!("{}-wal", f.display())).to_path_buf()]).map(|f| {
        let bytes = std::fs::read(&f).unwrap_or_default();
        let modified = std::fs::metadata(&f).ok().filter(|_| !bytes.is_empty()).map(|m| m.modified().unwrap());
        (bytes, modified)
    }).collect()
}
fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, sql) in files { std::fs::write(dir.path().join(name), sql).unwrap(); }
    dir
}

#[test]
fn an_install_upgraded_from_the_current_schema_gets_the_probe_once() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    assert_eq!(store::check_with(&p, &registry).unwrap(), pending(&[PROBE]));
    let l = lock(&p);
    assert_eq!(store::migrate_with(&p, &l, &registry).unwrap(), vec![PROBE.to_string()]);
    assert!(has_table(&p, "rust_migration_probe"));
    assert_eq!(recorded(&p), vec![PROBE.to_string()]);
    let applied_at: String = Connection::open(&p.primary).unwrap().query_row("SELECT applied_at FROM rust_schema_migrations", [], |r| r.get(0)).unwrap();
    assert!(applied_at.starts_with("20"), "{applied_at}");
    assert_eq!(store::check_with(&p, &registry).unwrap(), pending(&[]));
    let before = snapshot(&p);
    assert_eq!(store::migrate_with(&p, &l, &registry).unwrap(), Vec::<String>::new(), "a restart applies nothing");
    assert_eq!(snapshot(&p), before, "and writes nothing");
    // Rails' history is Rails' own: still exactly its migrations.
    let rails: i64 = Connection::open(&p.primary).unwrap().query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
    assert_eq!(rails as usize, MIGRATIONS.len());
}

#[test]
fn a_failed_migration_leaves_nothing_of_itself_keeps_the_ones_before_and_a_restart_applies_only_it() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    let first = ("20990101000001_create_probe.sql", "CREATE TABLE rust_migration_probe (id INTEGER PRIMARY KEY);");
    let broken = dir_with(&[first, ("20990101000002_add_half.sql", "CREATE TABLE half (id INTEGER PRIMARY KEY);\nINSERT INTO nowhere VALUES (1);")]);
    let list = test_support::load(broken.path()).unwrap();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    let l = lock(&p);
    match store::migrate_with(&p, &l, &registry) {
        Err(StoreError::Migration { version, name, error }) => {
            assert_eq!((version.as_str(), name.as_str()), ("20990101000002", "add_half"));
            assert!(error.contains("nowhere"), "{error}");
        }
        other => panic!("expected the second migration's failure, got {other:?}"),
    }
    assert_eq!(recorded(&p), vec!["20990101000001".to_string()], "the first stays applied and recorded");
    assert!(has_table(&p, "rust_migration_probe"));
    assert!(!has_table(&p, "half"), "the failed migration left nothing");
    assert_eq!(store::check_with(&p, &registry).unwrap(), pending(&["20990101000002"]));

    let fixed = dir_with(&[first, ("20990101000002_add_half.sql", "CREATE TABLE half (id INTEGER PRIMARY KEY);")]);
    let list = test_support::load(fixed.path()).unwrap();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    assert_eq!(store::migrate_with(&p, &l, &registry).unwrap(), vec!["20990101000002".to_string()], "only the fixed one runs");
    assert!(has_table(&p, "half"));
}

/// The build refuses COMMIT and the other transaction statements in a file; the migrator also refuses a file that
/// still ends its own transaction (END), and nothing of it is left, not even the migrations table.
#[test]
fn a_migration_that_ends_its_own_transaction_is_rolled_back_and_refused() {
    for end in ["END", "COMMIT"] {
        let dir = common::rails_install_unmigrated();
        let p = paths(dir.path());
        let sql = format!("CREATE TABLE escaped (id INTEGER PRIMARY KEY); {end};");
        let migrations = [Migration { version: "20990101000001", name: "escape", sql: &sql }];
        let registry = Registry { migrations: &migrations, tables: &[] };
        let r = store::migrate_with(&p, &lock(&p), &registry);
        assert!(matches!(r, Err(StoreError::Migration { ref version, .. }) if version == "20990101000001"), "{end}: {r:?}");
        assert!(!has_table(&p, "escaped") && !has_table(&p, "rust_schema_migrations"), "{end}: nothing applied");
    }
}

#[test]
fn check_reports_pending_migrations_and_writes_nothing() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    let before = snapshot(&p);
    std::thread::sleep(std::time::Duration::from_millis(20)); // a write would show in the mtime
    assert_eq!(store::check_with(&p, &registry).unwrap(), pending(&[PROBE]));
    let after = snapshot(&p);
    let brief = |s: &[(Vec<u8>, Option<std::time::SystemTime>)]| s.iter().map(|(b, t)| (b.len(), *t)).collect::<Vec<_>>();
    assert_eq!(brief(&after), brief(&before), "sizes and mtimes unchanged, nothing in a -wal");
    assert!(after == before, "bytes unchanged");
    assert!(!has_table(&p, "rust_schema_migrations"));
}

#[test]
fn an_unknown_rust_migration_is_unsupported_even_with_others_pending() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    exec(&p.primary, "CREATE TABLE rust_schema_migrations (version TEXT NOT NULL PRIMARY KEY, applied_at TEXT NOT NULL);
                      INSERT INTO rust_schema_migrations VALUES ('29991231000000', '2099-12-31 00:00:00');");
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    let r = store::check_with(&p, &registry);
    assert!(matches!(r, Err(StoreError::Unsupported { ref unknown }) if unknown == &vec!["29991231000000".to_string()]), "{r:?}");
}

/// A binary rolled back across a Rust migration refuses the install (the recovery is the operator's backup).
#[test]
fn an_older_binary_refuses_an_install_a_newer_one_migrated() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    store::migrate_with(&p, &lock(&p), &registry).unwrap();
    let r = store::check(&p); // this build's registry does not have the probe
    assert!(matches!(r, Err(StoreError::Unsupported { ref unknown }) if unknown == &vec![PROBE.to_string()]), "{r:?}");
    assert!(store::open(&p).is_err());
}

/// (name, in the queue database, SQL that breaks it, the error expected)
type Case = (&'static str, bool, &'static str, fn(&StoreError) -> bool);

#[test]
fn pending_migrations_never_hide_a_rails_or_queue_problem() {
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
    let cases: [Case; 3] = [
        ("Rails structure", false, "ALTER TABLE app_configs DROP COLUMN value",
         |e| matches!(e, StoreError::Incompatible { problems } if problems.iter().any(|m| m.contains("app_configs.value")))),
        ("queue structure", true, "ALTER TABLE solid_queue_processes RENAME COLUMN last_heartbeat_at TO heartbeat",
         |e| matches!(e, StoreError::Incompatible { problems } if problems.iter().any(|m| m.contains("solid_queue_processes.last_heartbeat_at")))),
        ("Rails history", false, "DELETE FROM schema_migrations WHERE version = (SELECT max(version) FROM schema_migrations)",
         |e| matches!(e, StoreError::Behind { .. })),
    ];
    for (name, in_queue, sql, expected) in cases {
        let dir = common::rails_install_unmigrated();
        let p = paths(dir.path());
        exec(if in_queue { &p.queue } else { &p.primary }, sql);
        match store::check_with(&p, &registry) {
            Err(e) => assert!(expected(&e), "{name}: {e:?}"),
            Ok(r) => panic!("{name}: passed with {r:?}"),
        }
        // (A broken queue already stops the engine lock, which reads Rails' heartbeat there.)
        if let Ok(l) = lease::lock(&p, chrono::Utc::now()) { assert!(store::migrate_with(&p, &l, &registry).is_err(), "{name}: nothing is migrated"); }
        assert!(!has_table(&p, "rust_schema_migrations"), "{name}");
    }
}

#[test]
fn a_recorded_migration_whose_table_is_missing_or_wrong_is_incompatible() {
    for (name, sql) in [("missing", "DROP TABLE rust_migration_probe"),
                        ("wrong", "DROP TABLE rust_migration_probe; CREATE TABLE rust_migration_probe (other TEXT)")] {
        let dir = common::rails_install_unmigrated();
        let p = paths(dir.path());
        let list = probe_migrations();
        let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES };
        store::migrate_with(&p, &lock(&p), &registry).unwrap();
        exec(&p.primary, sql);
        match store::check_with(&p, &registry) {
            Err(StoreError::Incompatible { problems }) => assert!(problems.iter().any(|m| m.contains("rust_migration_probe.id")), "{name}: {problems:?}"),
            other => panic!("{name}: expected Incompatible, got {other:?}"),
        }
    }
}

/// `migrate` takes the engine lock as proof; while one process holds it no other gets one, and `run` (which
/// migrates) refuses before touching either database.
#[test]
fn no_second_process_migrates_while_one_holds_the_engine_lock() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    let held = lock(&p);
    assert!(matches!(lease::lock(&p, chrono::Utc::now()), Err(LeaseError::Locked)));
    let before = snapshot(&p);
    let out = Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("run").env("STORAGE_DIR", dir.path())
        .env("SECRET_KEY_BASE", "engine-test-secret").env_remove("DATABASE_URL").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("another Deltabadger engine is running"), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(snapshot(&p), before);
    drop(held);
}

/// Pending Rust migrations never block `open`: `check`, `handback` and a manual sync run on an install `run` has not
/// migrated, and none of them migrates it.
#[test]
fn open_based_commands_work_on_an_unmigrated_install_and_never_migrate_it() {
    let dir = common::rails_install_unmigrated();
    let p = paths(dir.path());
    store::open(&p).unwrap();
    for args in [&["check"][..], &["handback"], &["sync", "balances"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_deltabadger")).args(args).env("STORAGE_DIR", dir.path())
            .env("SECRET_KEY_BASE", "engine-test-secret").env_remove("DATABASE_URL")
            .env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT").output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        if args == ["check"] {
            assert!(String::from_utf8_lossy(&out.stdout).contains("pending Rust migration 20261022000001"), "check names the identity migration");
        }
    }
    assert!(!has_table(&p, "rust_schema_migrations") && !has_table(&p, "exchange_accounts"), "none of them migrates, not even the migrations table");
}

/// (name, migration files, what the refusal says)
type Refused<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

#[test]
fn the_build_refuses_a_misnamed_or_repeated_version_a_rails_version_and_a_transaction_statement() {
    let rails = format!("{}_clash.sql", MIGRATIONS[0]);
    let refused: &[Refused<'_>] = &[
        ("repeated version", &[("20990101000001_a.sql", "SELECT 1;"), ("20990101000001_b.sql", "SELECT 1;")], "used by two"),
        ("Rails version", &[(&rails, "SELECT 1;")], "Rails migration"),
        ("misnamed", &[("2099_short.sql", "SELECT 1;")], "named"),
        ("no name", &[("20990101000001_.sql", "SELECT 1;")], "named"),
        ("BEGIN", &[("20990101000001_t.sql", "BEGIN;\nCREATE TABLE t (id INTEGER);")], "BEGIN"),
        ("COMMIT", &[("20990101000001_t.sql", "CREATE TABLE t (id INTEGER);\n  commit;")], "COMMIT"),
        ("ROLLBACK", &[("20990101000001_t.sql", "CREATE TABLE t (id INTEGER); ROLLBACK TRANSACTION;")], "ROLLBACK"),
        ("SAVEPOINT", &[("20990101000001_t.sql", "SAVEPOINT s; CREATE TABLE t (id INTEGER); RELEASE s;")], "SAVEPOINT"),
        ("after a comment", &[("20990101000001_t.sql", "-- the table\n/* first */ BEGIN TRANSACTION;")], "BEGIN"),
    ];
    for (name, files, says) in refused {
        let dir = dir_with(files);
        match test_support::load(dir.path()) {
            Err(e) => assert!(e.contains(says), "{name}: {e}"),
            Ok(found) => panic!("{name}: accepted {found:?}"),
        }
    }
    // Words in comments, quoted text and a trigger's own BEGIN … END are not transaction statements.
    let accepted = dir_with(&[("20990101000001_t.sql", "-- BEGIN;\nCREATE TABLE t (note TEXT DEFAULT 'commit; begin');\n\
        CREATE TRIGGER t_ai AFTER INSERT ON t BEGIN UPDATE t SET note = 'x'; END;"), (".gitkeep", "")]);
    assert_eq!(test_support::load(accepted.path()).unwrap().len(), 1);
}

/// build.rs embeds rust/migrations through the same loader, and every Rust table's contract names one of them.
#[test]
fn the_build_embeds_exactly_rust_migrations_and_each_contract_names_one() {
    let on_disk = test_support::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")).unwrap();
    let embedded: Vec<(String, String, String)> = RUST_MIGRATIONS.iter().map(|m| (m.version.into(), m.name.into(), m.sql.into())).collect();
    assert_eq!(embedded, on_disk);
    for (version, table) in REGISTRY.tables {
        assert!(RUST_MIGRATIONS.iter().any(|m| m.version == *version), "{} is keyed by {version}, which is no migration", table.name);
    }
}

/// `create_table "<name>"` lines in a schema dump for any of `tables`.
fn leaked(schema: &str, tables: &[&str]) -> Vec<String> {
    tables.iter().filter(|t| schema.contains(&format!("create_table \"{t}\""))).map(|t| t.to_string()).collect()
}

/// Rails' db:schema:dump includes every table it finds; db/schema.rb must never carry Rust's.
#[test]
fn db_schema_rb_holds_no_rust_table() {
    let schema = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../db/schema.rb")).unwrap();
    let probe = Registry { migrations: &[], tables: PROBE_TABLES };
    for tables in [REGISTRY.owned_tables(), probe.owned_tables()] {
        assert_eq!(leaked(&schema, &tables), Vec::<String>::new(), "db/schema.rb was dumped from a Rust-migrated database");
    }
    let dumped = format!("{schema}\n  create_table \"rust_migration_probe\", force: :cascade do |t|\n  end\n");
    assert_eq!(leaked(&dumped, &probe.owned_tables()), vec!["rust_migration_probe".to_string()], "a leaked dump is caught");
}
