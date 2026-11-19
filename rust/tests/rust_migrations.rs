//! Rust's own schema migrations (store::check / migrate), proved with the test-only probe migration in
//! tests/fixtures/rust_migrations; the production registry (rust/migrations) is checked against its contracts too.
mod common;
use common::{as_migrations, probe_migrations, PROBE, PROBE_TABLES};
use deltabadger::lease::{self, LeaseError};
use deltabadger::store::{self, test_support, Migration, Paths, Registry, Report, StoreError, TableContract, MIGRATIONS, REGISTRY, RUST_MIGRATIONS};
use rusqlite::Connection;
use serde_json::json;
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
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
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
    // 2.x's history is 2.x's own: still exactly its migrations.
    let v2: i64 = Connection::open(&p.primary).unwrap().query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
    assert_eq!(v2 as usize, MIGRATIONS.len());
}

#[test]
fn a_failed_migration_leaves_nothing_of_itself_keeps_the_ones_before_and_a_restart_applies_only_it() {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let first = ("20990101000001_create_probe.sql", "CREATE TABLE rust_migration_probe (id INTEGER PRIMARY KEY);");
    let broken = dir_with(&[first, ("20990101000002_add_half.sql", "CREATE TABLE half (id INTEGER PRIMARY KEY);\nINSERT INTO nowhere VALUES (1);")]);
    let list = test_support::load(broken.path()).unwrap();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
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
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
    assert_eq!(store::migrate_with(&p, &l, &registry).unwrap(), vec!["20990101000002".to_string()], "only the fixed one runs");
    assert!(has_table(&p, "half"));
}

/// The build refuses COMMIT and the other transaction statements in a file; the migrator also refuses a file that
/// still ends its own transaction (END), and nothing of it is left, not even the migrations table.
#[test]
fn a_migration_that_ends_its_own_transaction_is_rolled_back_and_refused() {
    for end in ["END", "COMMIT"] {
        let dir = common::v2_install_unmigrated();
        let p = paths(dir.path());
        let sql = format!("CREATE TABLE escaped (id INTEGER PRIMARY KEY); {end};");
        let migrations = [Migration { version: "20990101000001", name: "escape", sql: &sql }];
        let registry = Registry { migrations: &migrations, tables: &[], retired: &[] };
        let r = store::migrate_with(&p, &lock(&p), &registry);
        assert!(matches!(r, Err(StoreError::Migration { ref version, .. }) if version == "20990101000001"), "{end}: {r:?}");
        assert!(!has_table(&p, "escaped") && !has_table(&p, "rust_schema_migrations"), "{end}: nothing applied");
    }
}

#[test]
fn check_reports_pending_migrations_and_writes_nothing() {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
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
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    exec(&p.primary, "CREATE TABLE rust_schema_migrations (version TEXT NOT NULL PRIMARY KEY, applied_at TEXT NOT NULL);
                      INSERT INTO rust_schema_migrations VALUES ('29991231000000', '2099-12-31 00:00:00');");
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
    let r = store::check_with(&p, &registry);
    assert!(matches!(r, Err(StoreError::Unsupported { ref unknown }) if unknown == &vec!["29991231000000".to_string()]), "{r:?}");
}

/// A binary rolled back across a Rust migration refuses the install (the recovery is the operator's backup).
#[test]
fn an_older_binary_refuses_an_install_a_newer_one_migrated() {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
    store::migrate_with(&p, &lock(&p), &registry).unwrap();
    let r = store::check(&p); // this build's registry does not have the probe
    assert!(matches!(r, Err(StoreError::Unsupported { ref unknown }) if unknown == &vec![PROBE.to_string()]), "{r:?}");
    assert!(store::open(&p).is_err());
}

/// (name, in the queue database, SQL that breaks it, the error expected)
type Case = (&'static str, bool, &'static str, fn(&StoreError) -> bool);

#[test]
fn pending_migrations_never_hide_a_v2_or_queue_problem() {
    let list = probe_migrations();
    let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
    let cases: [Case; 3] = [
        ("2.x structure", false, "ALTER TABLE app_configs DROP COLUMN value",
         |e| matches!(e, StoreError::Incompatible { problems } if problems.iter().any(|m| m.contains("app_configs.value")))),
        ("queue structure", true, "ALTER TABLE solid_queue_processes RENAME COLUMN last_heartbeat_at TO heartbeat",
         |e| matches!(e, StoreError::Incompatible { problems } if problems.iter().any(|m| m.contains("solid_queue_processes.last_heartbeat_at")))),
        ("2.x history", false, "DELETE FROM schema_migrations WHERE version = (SELECT max(version) FROM schema_migrations)",
         |e| matches!(e, StoreError::Behind { .. })),
    ];
    for (name, in_queue, sql, expected) in cases {
        let dir = common::v2_install_unmigrated();
        let p = paths(dir.path());
        exec(if in_queue { &p.queue } else { &p.primary }, sql);
        match store::check_with(&p, &registry) {
            Err(e) => assert!(expected(&e), "{name}: {e:?}"),
            Ok(r) => panic!("{name}: passed with {r:?}"),
        }
        // (A broken queue already stops the engine lock, which reads 2.x's heartbeat there.)
        if let Ok(l) = lease::lock(&p, chrono::Utc::now()) { assert!(store::migrate_with(&p, &l, &registry).is_err(), "{name}: nothing is migrated"); }
        assert!(!has_table(&p, "rust_schema_migrations"), "{name}");
    }
}

#[test]
fn a_recorded_migration_whose_table_is_missing_or_wrong_is_incompatible() {
    for (name, sql) in [("missing", "DROP TABLE rust_migration_probe"),
                        ("wrong", "DROP TABLE rust_migration_probe; CREATE TABLE rust_migration_probe (other TEXT)")] {
        let dir = common::v2_install_unmigrated();
        let p = paths(dir.path());
        let list = probe_migrations();
        let registry = Registry { migrations: &as_migrations(&list), tables: PROBE_TABLES, retired: &[] };
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
    let dir = common::v2_install_unmigrated();
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
    let dir = common::v2_install_unmigrated();
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
fn the_build_refuses_a_misnamed_or_repeated_version_a_v2_version_a_transaction_statement_and_an_altered_table() {
    let v2 = format!("{}_clash.sql", MIGRATIONS[0]);
    let refused: &[Refused<'_>] = &[
        ("repeated version", &[("20990101000001_a.sql", "SELECT 1;"), ("20990101000001_b.sql", "SELECT 1;")], "used by two"),
        ("2.x version", &[(&v2, "SELECT 1;")], "2.x migration"),
        ("misnamed", &[("2099_short.sql", "SELECT 1;")], "named"),
        ("no name", &[("20990101000001_.sql", "SELECT 1;")], "named"),
        ("BEGIN", &[("20990101000001_t.sql", "BEGIN;\nCREATE TABLE t (id INTEGER);")], "BEGIN"),
        ("COMMIT", &[("20990101000001_t.sql", "CREATE TABLE t (id INTEGER);\n  commit;")], "COMMIT"),
        ("ROLLBACK", &[("20990101000001_t.sql", "CREATE TABLE t (id INTEGER); ROLLBACK TRANSACTION;")], "ROLLBACK"),
        ("SAVEPOINT", &[("20990101000001_t.sql", "SAVEPOINT s; CREATE TABLE t (id INTEGER); RELEASE s;")], "SAVEPOINT"),
        ("after a comment", &[("20990101000001_t.sql", "-- the table\n/* first */ BEGIN TRANSACTION;")], "BEGIN"),
        ("ALTER TABLE", &[("20990101000001_t.sql", "CREATE TABLE t (id INTEGER);\nalter  table bots ADD COLUMN x TEXT;")], "ALTER TABLE"),
        ("DROP TABLE", &[("20990101000001_t.sql", "/* gone */ DROP TABLE IF EXISTS bots;")], "DROP TABLE"),
        // D-B11-9-DROP-OWN-TABLE: DROP TABLE only of a bare name an earlier Rust file or statement created with CREATE TABLE.
        ("ALTER TABLE of its own table", &[("20990101000001_t.sql", "CREATE TABLE x (id INTEGER);\nALTER TABLE x RENAME TO y;")], "ALTER TABLE"),
        ("ALTER TABLE in a later file", &[("20990101000001_a.sql", "CREATE TABLE x (id INTEGER);"), ("20990101000002_b.sql", "ALTER TABLE x ADD COLUMN y TEXT;")], "ALTER TABLE"),
        ("DROP TABLE IF EXISTS of its own table", &[("20990101000001_t.sql", "CREATE TABLE x (id INTEGER); DROP TABLE IF EXISTS x;")], "IF EXISTS"),
        ("DROP of a 2.x table", &[("20990101000001_t.sql", "DROP TABLE bots;")], "no earlier Rust migration created"),
        ("DROP of a double-quoted name", &[("20990101000001_t.sql", "DROP TABLE \"bots\";")], "no earlier Rust migration created"),
        ("DROP of a backquoted name", &[("20990101000001_t.sql", "DROP TABLE `bots`;")], "no earlier Rust migration created"),
        ("DROP of a bracketed name", &[("20990101000001_t.sql", "DROP TABLE [bots];")], "no earlier Rust migration created"),
        ("DROP of a qualified name", &[("20990101000001_t.sql", "DROP TABLE main.bots;")], "no earlier Rust migration created"),
        ("DROP of a qualified own name", &[("20990101000001_t.sql", "CREATE TABLE x (id INTEGER); DROP TABLE temp.x;")], "no earlier Rust migration created"),
        ("a quoted CREATE authorises nothing", &[("20990101000001_t.sql", "CREATE TABLE \"x\" (id INTEGER); DROP TABLE x;")], "no earlier Rust migration created"),
        ("DROP of a quoted own name", &[("20990101000001_t.sql", "CREATE TABLE x (id INTEGER); DROP TABLE \"x\";")], "no earlier Rust migration created"),
        ("CREATE IF NOT EXISTS authorises nothing", &[("20990101000001_t.sql", "CREATE TABLE IF NOT EXISTS bots (id INTEGER); DROP TABLE bots;")], "no earlier Rust migration created"),
        ("CREATE TEMP authorises nothing", &[("20990101000001_t.sql", "CREATE TEMP TABLE x (id INTEGER); DROP TABLE x;")], "no earlier Rust migration created"),
        ("DROP before its CREATE", &[("20990101000001_t.sql", "DROP TABLE x; CREATE TABLE x (id INTEGER);")], "no earlier Rust migration created"),
        ("DROP before a later file's CREATE", &[("20990101000001_a.sql", "DROP TABLE x;"), ("20990101000002_b.sql", "CREATE TABLE x (id INTEGER);")], "no earlier Rust migration created"),
        ("DROP twice", &[("20990101000001_t.sql", "CREATE TABLE x (id INTEGER); DROP TABLE x; DROP TABLE x;")], "no earlier Rust migration created"),
        ("a longer name authorises nothing", &[("20990101000001_t.sql", "CREATE TABLE bots$copy (id INTEGER); DROP TABLE bots;")], "bind parameter"),
        ("a non-ASCII name authorises nothing", &[("20990101000001_t.sql", "CREATE TABLE botsé (id INTEGER); DROP TABLE bots;")], "non-ASCII"),
        ("after a byte-order mark", &[("20990101000001_t.sql", "\u{feff}DROP TABLE bots;")], "non-ASCII"),
        ("after a byte-order mark past a statement", &[("20990101000001_t.sql", "SELECT 1;\u{feff}DROP TABLE bots;")], "non-ASCII"),
        ("a byte-order mark inside a name", &[("20990101000001_t.sql", "CREATE TABLE bots\u{feff}copy (id INTEGER); DROP TABLE bots;")], "non-ASCII"),
        ("after a byte-order mark past a comment", &[("20990101000001_t.sql", "-- x\n\u{feff}ALTER TABLE bots ADD COLUMN y TEXT;")], "non-ASCII"),
        ("BEGIN after a byte-order mark", &[("20990101000001_t.sql", "\u{feff}BEGIN; CREATE TABLE t (id INTEGER);")], "non-ASCII"),
        ("a bind parameter", &[("20990101000001_t.sql", "SELECT $x(;CREATE/**/TABLE/**/bots); DROP TABLE bots;")], "bind parameter"),
        ("a ? parameter", &[("20990101000001_t.sql", "SELECT ?1; CREATE TABLE x (id INTEGER);")], "bind parameter"),
        ("a :name parameter", &[("20990101000001_t.sql", "SELECT :x; CREATE TABLE x (id INTEGER);")], "bind parameter"),
        ("DROP of two names", &[("20990101000001_t.sql", "CREATE TABLE x (id INTEGER); DROP TABLE x, bots;")], "no earlier Rust migration created"),
    ];
    for (name, files, says) in refused {
        let dir = dir_with(files);
        match test_support::load(dir.path()) {
            Err(e) => assert!(e.contains(says), "{name}: {e}"),
            Ok(found) => panic!("{name}: accepted {found:?}"),
        }
    }
    // Words in comments, quoted text and a trigger's own BEGIN … END are not transaction statements.
    let accepted = dir_with(&[("20990101000001_t.sql", "-- BEGIN; DROP TABLE bots;\nCREATE TABLE t (note TEXT DEFAULT 'commit; begin; alter table bots');\n\
        CREATE TRIGGER t_ai AFTER INSERT ON t BEGIN UPDATE t SET note = 'x'; END;"), (".gitkeep", "")]);
    assert_eq!(test_support::load(accepted.path()).unwrap().len(), 1);
    // A table an earlier file created, a hop table an earlier statement created (any case: SQLite's names are), and a
    // table created again after its drop.
    let replaced = dir_with(&[("20990101000001_a.sql", "CREATE TABLE x (id INTEGER);"),
        ("20990101000002_b.sql", "-- the hop, ‘in transit’\nCREATE TABLE Hop (id INTEGER, note TEXT DEFAULT 'café');\nINSERT INTO hop SELECT id FROM x;\ndrop  table x;\n\
          CREATE TABLE x (id INTEGER, note TEXT);\nINSERT INTO x (id) SELECT id FROM hop;\nDROP TABLE hop;")]);
    assert_eq!(test_support::load(replaced.path()).unwrap().len(), 2);
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

/// The KuCoin cutoffs earlier builds kept in app_configs move to import_cutoffs, and only they leave app_configs.
#[test]
fn the_kucoin_cutoffs_move_from_app_configs_to_their_own_table() {
    const CUTOFFS: &str = "20261023000001";
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let before = &RUST_MIGRATIONS[..RUST_MIGRATIONS.iter().position(|m| m.version == CUTOFFS).unwrap()];
    let tables = &REGISTRY.tables[..REGISTRY.tables.iter().position(|(v, _)| *v == CUTOFFS).unwrap()];
    store::migrate_with(&p, &lock(&p), &Registry { migrations: before, tables, retired: &[] }).unwrap();
    exec(&p.primary, r#"INSERT INTO app_configs (key, value, created_at, updated_at) VALUES
        ('rust_kucoin_csv.cutoff:7:3', '{"fills":"2026-09-14 12:00:00","deposits":"2026-09-16 10:00:00"}', '2026-09-20', '2026-09-20 02:00:00'),
        ('rust_kucoin_csv.cutoff:12:40', '{"withdrawals":"2026-09-01 00:00:00"}', '2026-09-20', '2026-09-21 00:00:00'),
        ('rust_kucoin_csv.cutoff:8:3', 'not json', '2026-09-20', '2026-09-20'),
        ('rust_sync.ledger:1', '{}', '2026-09-20', '2026-09-20')"#);
    let after: Vec<String> = RUST_MIGRATIONS[before.len()..].iter().map(|m| m.version.to_string()).collect();
    assert_eq!(store::migrate_with(&p, &lock(&p), &REGISTRY).unwrap(), after, "the cutoffs migration and every later one");
    let c = Connection::open(&p.primary).unwrap();
    let mut s = c.prepare("SELECT user_id, exchange_id, class, cutoff_at, updated_at FROM import_cutoffs ORDER BY user_id, class").unwrap();
    let rows: Vec<(i64, i64, String, String, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap().collect::<Result<_, _>>().unwrap();
    let row = |u, e, class: &str, at: &str, upd: &str| (u, e, class.to_string(), at.to_string(), upd.to_string());
    assert_eq!(rows, vec![row(7, 3, "deposits", "2026-09-16 10:00:00", "2026-09-20 02:00:00"), row(7, 3, "fills", "2026-09-14 12:00:00", "2026-09-20 02:00:00"),
                          row(12, 40, "withdrawals", "2026-09-01 00:00:00", "2026-09-21 00:00:00")]);
    let keys: Vec<String> = c.prepare("SELECT key FROM app_configs ORDER BY key").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(keys, ["rust_sync.ledger:1"]);
}

const STRAY: &str = "20261025000001";

/// An unmigrated install brought up to just before the stray-conditions migration, opened, with the Kraken fixtures seeded.
fn before_stray() -> (tempfile::TempDir, Paths, store::Opened, common::seed::Seeded) {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let n = RUST_MIGRATIONS.iter().position(|m| m.version == STRAY).unwrap();
    let k = REGISTRY.tables.iter().position(|(v, _)| *v >= STRAY).unwrap_or(REGISTRY.tables.len());
    let before = Registry { migrations: &RUST_MIGRATIONS[..n], tables: &REGISTRY.tables[..k], retired: &[] };
    store::migrate_with(&p, &lock(&p), &before).unwrap();
    let o = store::open_with(&p, &before).unwrap();
    let s = common::seed::seed_kraken(&o.primary, &common::seed::cipher());
    (dir, p, o, s)
}
fn row(o: &store::Opened, id: i64) -> (String, String) {
    o.primary.query_row("SELECT settings, updated_at FROM bots WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
}

/// D-STRAY-FLAGS-MIGRATION: 2.x never read an index or signal bot's rule switches or spending cap; the first start of this
/// build turns each one that is set off (a malformed "1" too), leaves updated_at, every other key and every other bot as
/// they were, and the install is admitted.
#[test]
fn the_stray_conditions_migration_turns_off_index_and_signal_switches_once() {
    let (_d, p, o, s) = before_stray();
    let index = common::seed::index_bot(&o.primary, &s, "ignored", 3, 0.0, false);
    o.primary.execute("UPDATE bots SET settings = json_remove(json_set(settings, '$.index_type', 'top', '$.price_limited', json('true'), \
                       '$.sell_price_drop_limited', '1', '$.moving_average_limited', json('false'), '$.quote_amount_limited', json('true'), \
                       '$.quote_amount_limit', 10, '$.price_limit', 5), '$.index_category_id'), updated_at = '2026-09-01 00:00:00' WHERE id = ?1", [index]).unwrap();
    let signal = common::seed::insert_bot(&o.primary, &s, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE bots SET type = 'Bots::Signal', settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.indicator_limited', 1) WHERE id = ?1", [signal]).unwrap();
    let basket = common::seed::insert_bot(&o.primary, &s, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limited", json!(true)).with("quote_amount_limited", json!(true)));
    let quiet = common::seed::index_bot(&o.primary, &s, "other", 3, 0.0, false);
    o.primary.execute("UPDATE bots SET settings = '{\"interval\": \"week\" , \"price_limited\": false}' WHERE id = ?1", [quiet]).unwrap();
    let (basket_before, quiet_before, index_before) = (row(&o, basket), row(&o, quiet), row(&o, index));
    assert!(store::migrate_with(&p, &lock(&p), &REGISTRY).unwrap().contains(&STRAY.to_string()));
    let (settings, updated) = row(&o, index);
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    for key in ["price_limited", "sell_price_drop_limited", "moving_average_limited", "quote_amount_limited"] { assert_eq!(settings[key], json!(false), "{key}"); }
    for key in ["indicator_limited", "sell_price_limited", "price_drop_limited"] { assert!(settings.get(key).is_none(), "{key}: only existing keys"); }
    assert_eq!((&settings["quote_amount_limit"], &settings["price_limit"], &settings["index_type"]), (&json!(10), &json!(5), &json!("top")));
    assert_eq!(updated, index_before.1, "updated_at unchanged");
    let signal_settings: serde_json::Value = serde_json::from_str(&row(&o, signal).0).unwrap();
    assert_eq!((&signal_settings["quote_amount_limited"], &signal_settings["indicator_limited"]), (&json!(false), &json!(false)));
    assert_eq!(row(&o, basket), basket_before, "a basket's rules are its own");
    assert_eq!(row(&o, quiet), quiet_before, "nothing set: the row's text is untouched");
    o.primary.execute("DELETE FROM bots WHERE id IN (?1, ?2, ?3)", [signal, basket, quiet]).unwrap();
    let r = deltabadger::engine::eligibility::check_install(&o.primary).unwrap();
    assert!(r.problems.is_empty() && r.unreadable.is_empty(), "{:?} {:?}", r.problems, r.unreadable);
    assert_eq!(r.eligible, vec![index]);
}

/// Applied once, with its version row: a cap switched on after it (by this engine) survives every later start.
#[test]
fn a_cap_switched_on_after_the_stray_conditions_migration_survives_restarts() {
    let (_d, p, o, s) = before_stray();
    let index = common::seed::index_bot(&o.primary, &s, "ignored", 3, 0.0, false);
    store::migrate_with(&p, &lock(&p), &REGISTRY).unwrap();
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.price_limited', json('true')) WHERE id = ?1", [index]).unwrap();
    let before = row(&o, index);
    for _ in 0..3 { assert_eq!(store::migrate_with(&p, &lock(&p), &REGISTRY).unwrap(), Vec::<String>::new()); }
    assert_eq!(row(&o, index), before);
    assert_eq!(recorded(&p).iter().filter(|v| *v == STRAY).count(), 1);
}

/// A failure inside the migration leaves no switch changed and no version row; the next start applies it.
#[test]
fn a_failed_stray_conditions_migration_keeps_nothing_and_runs_at_the_next_start() {
    let (_d, p, o, s) = before_stray();
    let index = common::seed::index_bot(&o.primary, &s, "ignored", 3, 0.0, false);
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true')) WHERE id = ?1", [index]).unwrap();
    let before = row(&o, index);
    let real = RUST_MIGRATIONS.iter().find(|m| m.version == STRAY).unwrap();
    let broken = format!("{}\nINSERT INTO nowhere VALUES (1);", real.sql);
    let n = RUST_MIGRATIONS.iter().position(|m| m.version == STRAY).unwrap();
    let mut list: Vec<Migration> = RUST_MIGRATIONS[..n].to_vec();
    list.push(Migration { version: STRAY, name: real.name, sql: &broken });
    assert!(matches!(store::migrate_with(&p, &lock(&p), &Registry { migrations: &list, tables: REGISTRY.tables, retired: REGISTRY.retired }), Err(StoreError::Migration { .. })));
    assert_eq!(row(&o, index), before, "rolled back with its version row");
    assert!(!recorded(&p).contains(&STRAY.to_string()));
    store::migrate_with(&p, &lock(&p), &REGISTRY).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&row(&o, index).0).unwrap();
    assert_eq!(settings["quote_amount_limited"], json!(false));
}

// ---- replacing a table (PLAN-B11-9 §2): the fixture migrations in tests/fixtures/rust_replacements ----------------

const CREATED: &str = "20990201000001";
const REPLACED: &str = "20990201000002";
const FX_OLD: TableContract = TableContract { name: "rust_fx_old", columns: &[("name", "TEXT", true), ("at", "TEXT", false)], unique_indexes: &[] };
const FX_NEW: TableContract = TableContract { name: "rust_fx_new", columns: FX_OLD.columns, unique_indexes: &[] };
const FX_KEPT_BEFORE: TableContract = TableContract { name: "rust_fx_kept",
    columns: &[("id", "INTEGER", false), ("label", "TEXT", true), ("kind", "TEXT", true)], unique_indexes: &[&["label"]] };
const FX_KEPT_AFTER: TableContract = TableContract { name: "rust_fx_kept",
    columns: &[("id", "INTEGER", false), ("label", "TEXT", false), ("kind", "TEXT", true), ("note", "TEXT", true)], unique_indexes: &[] };
const FX_TABLES: &[(&str, TableContract)] = &[(CREATED, FX_OLD), (CREATED, FX_KEPT_BEFORE), (REPLACED, FX_NEW), (REPLACED, FX_KEPT_AFTER)];
const FX_RETIRED: &[(&str, &str)] = &[(REPLACED, "rust_fx_old")];

fn replacements() -> Vec<(String, String, String)> {
    test_support::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust_replacements")).unwrap()
}
/// An install that applied the fixture's first migration, with rows in both tables.
fn before_replacement(list: &[(String, String, String)]) -> (tempfile::TempDir, Paths) {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let m = as_migrations(list);
    store::migrate_with(&p, &lock(&p), &Registry { migrations: &m[..1], tables: &FX_TABLES[..2], retired: &[] }).unwrap();
    exec(&p.primary, "INSERT INTO rust_fx_old (rowid, name, at) VALUES (5, 'five', NULL), (9, 'nine', '2026-11-15');
                      INSERT INTO rust_fx_kept (id, label, kind) VALUES (3, 'x', 'a'), (7, 'y', 'b');");
    (dir, p)
}
/// Every object's SQL and every table's rows, as text.
fn contents(p: &Paths) -> Vec<String> {
    let c = Connection::open(&p.primary).unwrap();
    let objects: Vec<(String, String)> = c.prepare("SELECT name, coalesce(sql, '') FROM sqlite_master ORDER BY type, name").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    let mut out = Vec::new();
    for (name, sql) in objects {
        out.push(sql.clone());
        if sql.starts_with("CREATE TABLE") {
            let mut s = c.prepare(&format!("SELECT rowid, * FROM \"{name}\" ORDER BY rowid")).unwrap();
            let n = s.column_count();
            let rows: Vec<String> = s.query_map([], |r| Ok(format!("{name}: {:?}", (0..n).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect::<Result<Vec<_>, _>>()?))).unwrap()
                .collect::<Result<_, _>>().unwrap();
            out.extend(rows);
        }
    }
    out
}
fn rows(p: &Paths, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let c = Connection::open(&p.primary).unwrap();
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    s.query_map([], |r| (0..n).map(|i| r.get(i)).collect()).unwrap().collect::<Result<_, _>>().unwrap()
}
fn incompatible(r: Result<Report, StoreError>) -> Vec<String> {
    match r { Err(StoreError::Incompatible { problems }) => problems, other => panic!("expected Incompatible, got {other:?}") }
}

/// D-B11-9-DROP-OWN-TABLE: a rename (create, copy with rowid, drop) and a same-name rebuild through a hop table keep every
/// row and id; a restart applies nothing; a build without the replacement refuses the install.
#[test]
fn a_replacement_renames_and_rebuilds_keeping_every_row_and_a_restart_applies_nothing() {
    use rusqlite::types::Value::{Integer, Null, Text};
    let list = replacements();
    let (_d, p) = before_replacement(&list);
    let m = as_migrations(&list);
    let full = Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED };
    assert_eq!(store::check_with(&p, &full).unwrap(), pending(&[REPLACED]));
    assert_eq!(store::migrate_with(&p, &lock(&p), &full).unwrap(), vec![REPLACED.to_string()]);
    let t = |s: &str| Text(s.into());
    assert_eq!(rows(&p, "SELECT rowid, name, at FROM rust_fx_new ORDER BY rowid"), vec![vec![Integer(5), t("five"), Null], vec![Integer(9), t("nine"), t("2026-11-15")]]);
    assert_eq!(rows(&p, "SELECT id, label, kind, note FROM rust_fx_kept ORDER BY id"), vec![vec![Integer(3), t("x"), t("a"), t("")], vec![Integer(7), t("y"), t("c"), t("")]]);
    assert!(!has_table(&p, "rust_fx_old") && !has_table(&p, "rust_fx_kept_hop"));
    let c = Connection::open(&p.primary).unwrap();
    assert!(c.execute("INSERT INTO rust_fx_kept (label, kind) VALUES ('z', 'b')", []).is_err(), "the new CHECK applies");
    c.execute("INSERT INTO rust_fx_kept (label, kind) VALUES (NULL, 'c'), ('x', 'a')", []).unwrap();
    c.execute("DELETE FROM rust_fx_kept WHERE id > 7", []).unwrap();
    drop(c);
    assert_eq!(store::check_with(&p, &full).unwrap(), pending(&[]));
    let before = snapshot(&p);
    assert_eq!(store::migrate_with(&p, &lock(&p), &full).unwrap(), Vec::<String>::new(), "a restart applies nothing");
    assert_eq!(snapshot(&p), before);
    let older = Registry { migrations: &m[..1], tables: &FX_TABLES[..2], retired: &[] };
    let r = store::check_with(&p, &older);
    assert!(matches!(r, Err(StoreError::Unsupported { ref unknown }) if unknown == &vec![REPLACED.to_string()]), "{r:?}");
    assert!(matches!(store::open_with(&p, &older), Err(StoreError::Unsupported { .. })));
}

/// D-B11-9-RETIRED-CONTRACT: of a table's recorded contracts only the newest applies. The rebuild breaks the old
/// contract (label nullable, its unique index gone), so passing with both recorded proves the old one is not applied.
#[test]
fn only_the_newest_recorded_contract_of_a_table_applies() {
    let list = replacements();
    let m = as_migrations(&list);
    let full = Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED };
    // Before the rebuild is recorded the old contract applies: no `note` is fine, a nullable label is not.
    let (_d, p) = before_replacement(&list);
    assert_eq!(store::check_with(&p, &full).unwrap(), pending(&[REPLACED]));
    exec(&p.primary, "DROP TABLE rust_fx_kept; CREATE TABLE rust_fx_kept (id INTEGER PRIMARY KEY, label TEXT, kind TEXT NOT NULL);
                      CREATE UNIQUE INDEX i ON rust_fx_kept (label);");
    assert!(incompatible(store::check_with(&p, &full)).contains(&"rust_fx_kept.label must be NOT NULL".to_string()));

    let (_d, p) = before_replacement(&list);
    store::migrate_with(&p, &lock(&p), &full).unwrap();
    let only = |tables| Registry { migrations: &m, tables, retired: FX_RETIRED };
    let problems = incompatible(store::check_with(&p, &only(&[(CREATED, FX_KEPT_BEFORE)])));
    assert_eq!(problems, ["rust_fx_kept.label must be NOT NULL", "rust_fx_kept (label) is missing a unique index"], "the rebuild breaks the old contract");
    assert_eq!(store::check_with(&p, &only(&[(REPLACED, FX_KEPT_AFTER)])).unwrap(), pending(&[]));
    assert_eq!(store::check_with(&p, &full).unwrap(), pending(&[]), "both recorded: only the newer applies");
    exec(&p.primary, "DROP TABLE rust_fx_kept; CREATE TABLE rust_fx_kept (id INTEGER PRIMARY KEY, label TEXT, kind TEXT NOT NULL);");
    assert_eq!(incompatible(store::check_with(&p, &full)), ["rust_fx_kept.note is missing"], "and the newer one is checked");
}

/// A recorded retirement means the table must be absent.
#[test]
fn a_retired_table_still_present_is_incompatible() {
    let list = replacements();
    let m = as_migrations(&list);
    let full = Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED };
    let (_d, p) = before_replacement(&list);
    store::migrate_with(&p, &lock(&p), &full).unwrap();
    exec(&p.primary, "CREATE TABLE rust_fx_old (name TEXT NOT NULL, at TEXT);");
    assert_eq!(incompatible(store::check_with(&p, &full)), [format!("rust_fx_old should be gone (retired by {REPLACED})")]);
    assert!(store::open_with(&p, &full).is_err());
}

/// A replacement that fails on its last statement leaves both old tables, every row and the history as they were; the
/// next start applies it.
#[test]
fn a_replacement_failing_on_its_last_statement_leaves_the_old_tables_and_rows() {
    let list = replacements();
    let (_d, p) = before_replacement(&list);
    let before = contents(&p);
    let broken = dir_with(&[(&format!("{CREATED}_{}.sql", list[0].1), &list[0].2), (&format!("{REPLACED}_{}.sql", list[1].1), &format!("{}\nINSERT INTO nowhere VALUES (1);", list[1].2))]);
    let broken = test_support::load(broken.path()).unwrap();
    let m = as_migrations(&broken);
    match store::migrate_with(&p, &lock(&p), &Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED }) {
        Err(StoreError::Migration { version, error, .. }) => assert!(version == REPLACED && error.contains("nowhere"), "{version}: {error}"),
        other => panic!("expected the replacement's failure, got {other:?}"),
    }
    assert_eq!(contents(&p), before, "every object and row as before");
    assert_eq!(recorded(&p), vec![CREATED.to_string()]);
    let m = as_migrations(&list);
    let full = Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED };
    assert_eq!(store::check_with(&p, &full).unwrap(), pending(&[REPLACED]));
    assert_eq!(store::migrate_with(&p, &lock(&p), &full).unwrap(), vec![REPLACED.to_string()]);
}

/// D-B11-9-REPLACEMENT-PENDING: with a replacement pending, `open` (every command that does not migrate) refuses an
/// install that still has a table it replaces, and writes nothing; `check` reports it pending. An install without those
/// tables (one that never ran their migration) opens as before, and so does a migrated one.
#[test]
fn an_install_with_a_replacement_pending_is_not_opened() {
    let list = replacements();
    let m = as_migrations(&list);
    let full = Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED };
    let (_d, p) = before_replacement(&list);
    let before = snapshot(&p);
    match store::open_with(&p, &full) {
        Err(StoreError::ReplacementPending { versions, tables }) => {
            assert_eq!((versions, tables), (vec![REPLACED.to_string()], vec!["rust_fx_kept".to_string(), "rust_fx_old".to_string()]));
        }
        other => panic!("expected ReplacementPending, got {:?}", other.map(|_| ())),
    }
    assert!(matches!(store::check_open_with(&p, &full), Err(StoreError::ReplacementPending { .. })));
    assert_eq!(store::check_with(&p, &full).unwrap(), pending(&[REPLACED]));
    assert!(snapshot(&p) == before, "nothing written");
    store::migrate_with(&p, &lock(&p), &full).unwrap();
    store::open_with(&p, &full).unwrap();

    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    assert_eq!(store::check_open_with(&p, &full).unwrap(), pending(&[CREATED, REPLACED]));
    store::open_with(&p, &full).unwrap();
}

/// Every DROP TABLE in a migration is a retirement at its version, a rebuild (a new contract for the same table at that
/// version) or a hop table the same file created; every retirement is a DROP of that file; nothing is both contracted
/// and retired at one version.
fn replacement_problems(r: &Registry) -> Vec<String> {
    let mut problems = Vec::new();
    let drops = test_support::drops(r.migrations).unwrap();
    for (version, table, hop) in &drops {
        let rebuilt = r.tables.iter().any(|(v, t)| v == version && t.name == table);
        let retired = r.retired.iter().any(|(v, t)| v == version && t == table);
        if !(*hop || rebuilt || retired) { problems.push(format!("{version} drops {table}: neither retired, rebuilt nor a hop table")); }
    }
    for (version, table) in r.retired {
        if !r.migrations.iter().any(|m| m.version == *version) { problems.push(format!("{table} is retired by {version}, which is no migration")); }
        if !drops.iter().any(|(v, t, _)| v == version && t == table) { problems.push(format!("{table} is retired by {version}, whose file does not drop it")); }
        if r.tables.iter().any(|(v, t)| v == version && t.name == *table) { problems.push(format!("{table} is both contracted and retired by {version}")); }
    }
    problems
}

#[test]
fn every_dropped_table_is_retired_rebuilt_or_a_hop_table() {
    assert_eq!(replacement_problems(&REGISTRY), Vec::<String>::new());
    let list = replacements();
    let m = as_migrations(&list);
    assert_eq!(test_support::drops(&m).unwrap(), [(REPLACED.into(), "rust_fx_old".into(), false), (REPLACED.into(), "rust_fx_kept".into(), false),
                                                  (REPLACED.into(), "rust_fx_kept_hop".into(), true)]);
    assert_eq!(replacement_problems(&Registry { migrations: &m, tables: FX_TABLES, retired: FX_RETIRED }), Vec::<String>::new());
    assert_eq!(replacement_problems(&Registry { migrations: &m, tables: FX_TABLES, retired: &[] }),
               [format!("{REPLACED} drops rust_fx_old: neither retired, rebuilt nor a hop table")]);
    assert_eq!(replacement_problems(&Registry { migrations: &m, tables: FX_TABLES, retired: &[(REPLACED, "rust_fx_old"), (CREATED, "rust_fx_kept")] }),
               [format!("rust_fx_kept is retired by {CREATED}, whose file does not drop it"), format!("rust_fx_kept is both contracted and retired by {CREATED}")]);
}

/// `owned_tables` keeps a retired table (it is Rust-born); `live_tables` is what the newest version leaves.
#[test]
fn live_tables_leave_out_a_retired_table() {
    let fx = Registry { migrations: &[], tables: FX_TABLES, retired: FX_RETIRED };
    assert_eq!(fx.owned_tables(), ["rust_schema_migrations", "rust_fx_old", "rust_fx_kept", "rust_fx_new"]);
    assert_eq!(fx.live_tables(), ["rust_schema_migrations", "rust_fx_kept", "rust_fx_new"]);
    for (_, t) in REGISTRY.retired { assert!(REGISTRY.owned_tables().contains(t) && !REGISTRY.live_tables().contains(t), "{t}"); }
}

/// The migrator's own guard, whatever the build's scan let through: a file that drops or rebuilds a table no Rust
/// contract names (a 2.x-born one) is rolled back and refused.
#[test]
fn a_migration_that_drops_or_rebuilds_a_v2_table_is_rolled_back() {
    for sql in ["DROP TABLE rules;", "DROP TABLE rules; CREATE TABLE rules (id INTEGER PRIMARY KEY, status INTEGER NOT NULL);"] {
        let dir = common::v2_install_unmigrated();
        let p = paths(dir.path());
        let before = contents(&p);
        let migrations = [Migration { version: "20990101000001", name: "unsafe", sql }];
        match store::migrate_with(&p, &lock(&p), &Registry { migrations: &migrations, tables: &[], retired: &[] }) {
            Err(StoreError::Migration { error, .. }) => assert!(error.contains("rules, which is not a Rust table"), "{sql}: {error}"),
            other => panic!("{sql}: expected the refusal, got {other:?}"),
        }
        assert_eq!(contents(&p), before, "{sql}: rolled back");
    }
}

// ---- #710 slice 5 (PLAN-710-5 §4): balances and venue snapshots move to stores keyed by account ----

const STORES: &str = "20261121000001";

/// §6 test 1: 2.x's balances and venue snapshots, with gappy ids and a sequence above the largest, are in the stores with
/// their ids and content, each on its (user, exchange)'s number 1 (made where none was there), and each store's sequence
/// is its 2.x table's. 2.x's tables keep their rows.
#[test]
fn the_balances_and_venue_snapshots_move_to_stores_keyed_by_account_keeping_ids_and_sequences() {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let m = RUST_MIGRATIONS.iter().position(|m| m.version >= STORES).unwrap();
    let t = REGISTRY.tables.iter().position(|(v, _)| *v >= STORES).unwrap_or(REGISTRY.tables.len());
    store::migrate_with(&p, &lock(&p), &Registry { migrations: &RUST_MIGRATIONS[..m], tables: &REGISTRY.tables[..t], retired: REGISTRY.retired }).unwrap();
    exec(&p.primary, "PRAGMA foreign_keys = OFF; DELETE FROM exchange_accounts;
        INSERT INTO exchange_accounts (id, user_id, exchange_id, number, created_at, updated_at) VALUES (40, 1, 2, 1, 'now', 'now');
        INSERT INTO account_balances (id, user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at) VALUES
          (3, 1, 2, 7, 1.5, 0, 100.25, 150.375, 'p', 's', 'c', 'u'), (8, 1, 3, 7, '0.1', 2, NULL, NULL, NULL, 's2', 'c2', 'u2');
        UPDATE sqlite_sequence SET seq = 20 WHERE name = 'account_balances';
        INSERT INTO portfolio_venue_snapshots (id, user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at) VALUES
          (5, 1, 2, '2026-01-01', 10.5, 9, NULL, 1.25, 1, 'c', 'u'), (6, 1, 2, '2026-01-02', 11, 9, 2, 1, 0, 'c', 'u');
        UPDATE sqlite_sequence SET seq = 30 WHERE name = 'portfolio_venue_snapshots';");
    assert_eq!(store::migrate(&p, &lock(&p)).unwrap()[0], STORES);
    let rows = |sql: &str| rows(&p, sql);
    let same = |v2: &str, store: &str, columns: &str| {
        assert_eq!(rows(&format!("SELECT {columns} FROM {store} ORDER BY id")), rows(&format!("SELECT {columns} FROM {v2} ORDER BY id")), "{store}: ids and content are 2.x's");
        assert_eq!(rows(&format!("SELECT seq FROM sqlite_sequence WHERE name = '{store}'")), rows(&format!("SELECT seq FROM sqlite_sequence WHERE name = '{v2}'")));
    };
    same("account_balances", "exchange_account_balances", "id, asset_id, created_at, exchange_id, free, typeof(free), locked, priced_at, synced_at, updated_at, usd_price, typeof(usd_price), usd_value, user_id");
    same("portfolio_venue_snapshots", "exchange_account_snapshots", "id, user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at");
    use rusqlite::types::Value::Integer;
    assert_eq!(rows("SELECT b.id, a.user_id, a.exchange_id, a.number FROM exchange_account_balances b JOIN exchange_accounts a ON a.id = b.account_id ORDER BY b.id"),
               [[Integer(3), Integer(1), Integer(2), Integer(1)], [Integer(8), Integer(1), Integer(3), Integer(1)]], "each on its number 1, made where missing");
    assert_eq!(rows("SELECT DISTINCT account_id FROM exchange_account_snapshots"), [[Integer(40)]]);
    assert_eq!(rows("SELECT count(*) FROM account_balances"), [[Integer(2)]], "2.x's rows stay");
    // A new row takes the id 2.x would have given it.
    exec(&p.primary, "INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, synced_at, created_at, updated_at, account_id) VALUES (1, 2, 9, 's', 'c', 'u', 40)");
    assert_eq!(rows("SELECT max(id) FROM exchange_account_balances"), [[Integer(21)]]);
    assert!(store::check(&p).unwrap().pending_rust.is_empty());
}
/// §6 test 14: outside the stores' own module and the 2.x schema contract, no Rust code names 2.x's balance and venue
/// snapshot tables (comments aside).
#[test]
fn only_the_store_module_names_the_2x_balance_and_snapshot_tables() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = vec![];
    let mut dirs = vec![root.join("src"), root.join("examples")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { dirs.push(path); continue; }
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("src/store.rs") || path.ends_with("src/sync/balance_store.rs") { continue; }
            for (n, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
                let code = line.split("//").next().unwrap();
                if ["account_balances", "portfolio_venue_snapshots"].iter().any(|t| code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).any(|w| w == *t)) {
                    found.push(format!("{}:{}", path.strip_prefix(root).unwrap().display(), n + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "read balances and venue snapshots from the stores (sync::balance_store): {found:?}");
}
