mod common;
use deltabadger::store::{self, Paths, StoreError, MIGRATIONS};
use rusqlite::Connection;
use std::path::Path;

fn paths(dir: &Path) -> Paths { Paths::from_env(&|_| None, dir) }

fn exec(path: &Path, sql: &str) { Connection::open(path).unwrap().execute_batch(sql).unwrap(); }

fn refusal(p: &Paths) -> StoreError {
    let checked = store::check(p).expect_err("check refuses");
    assert!(store::open(p).is_err(), "open refuses whatever check refuses");
    checked
}

#[test]
fn paths_follow_rails_env_names_with_storage_defaults() {
    let p = Paths::from_env(&|k| (k == "QUEUE_DATABASE_PATH").then(|| "/q/queue.sqlite3".to_string()), Path::new("/data"));
    assert_eq!(p.primary, Path::new("/data/production.sqlite3"));
    assert_eq!(p.queue, Path::new("/q/queue.sqlite3"));
    assert_eq!(p.lock_file(), Path::new("/data/.engine.lock"));
}

#[test]
fn the_build_knows_exactly_the_migrations_in_db_migrate() {
    let mut on_disk: Vec<String> = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../db/migrate")).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy()[..14].to_string()).collect();
    on_disk.sort();
    assert_eq!(MIGRATIONS, on_disk.iter().map(String::as_str).collect::<Vec<_>>().as_slice());
}

#[test]
fn an_install_rails_prepared_is_accepted_and_opened_read_write() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    store::check(&p).unwrap();
    let o = store::open(&p).unwrap();
    o.primary.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('probe', 'x', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", []).unwrap();
    let n: i64 = o.queue.query_row("SELECT count(*) FROM solid_queue_processes", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn a_missing_or_empty_database_is_refused_and_never_created() {
    for victim in ["production.sqlite3", "production_queue.sqlite3"] {
        let dir = common::rails_install();
        let path = dir.path().join(victim);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(refusal(&paths(dir.path())), StoreError::Missing { path: ref m } if *m == path), "{victim}");
        assert!(!path.exists(), "{victim} not created");
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(refusal(&paths(dir.path())), StoreError::Missing { .. }), "empty {victim}");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0, "empty {victim} left as it was");
    }
}

#[test]
fn a_foreign_sqlite_file_is_refused_and_preserved() {
    for victim in ["production.sqlite3", "production_queue.sqlite3"] {
        let dir = common::rails_install();
        let path = dir.path().join(victim);
        std::fs::remove_file(&path).unwrap();
        exec(&path, "CREATE TABLE sentinel (v text); INSERT INTO sentinel VALUES ('keep');");
        assert!(matches!(refusal(&paths(dir.path())), StoreError::Unrecognised { .. }), "{victim}");
        let v: String = Connection::open(&path).unwrap().query_row("SELECT v FROM sentinel", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "keep", "{victim} left intact");
    }
}

#[test]
fn a_database_rails_has_not_fully_migrated_is_behind() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    let last = *MIGRATIONS.last().unwrap();
    exec(&p.primary, &format!("DELETE FROM schema_migrations WHERE version = '{last}'"));
    assert!(matches!(refusal(&p), StoreError::Behind { missing } if missing == vec![last.to_string()]));
}

#[test]
fn a_migration_this_build_does_not_know_is_unsupported_and_nothing_is_written() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    exec(&p.primary, "INSERT INTO schema_migrations VALUES ('29991231000000')");
    let before = std::fs::read(&p.primary).unwrap();
    assert!(matches!(refusal(&p), StoreError::Unsupported { unknown } if unknown == vec!["29991231000000".to_string()]));
    assert_eq!(std::fs::read(&p.primary).unwrap(), before, "primary contents unchanged");
}

#[test]
fn missing_and_unknown_migrations_together_are_a_diverged_history() {
    let dir = common::rails_install();
    let p = paths(dir.path());
    let first = MIGRATIONS[0];
    exec(&p.primary, &format!("DELETE FROM schema_migrations WHERE version = '{first}'; INSERT INTO schema_migrations VALUES ('29991231000000');"));
    assert!(matches!(refusal(&p), StoreError::Diverged { missing, unknown }
        if missing == vec![first.to_string()] && unknown == vec!["29991231000000".to_string()]));
}

/// Matching versions do not prove matching structure: the tables Rust reads and writes are checked too.
#[test]
fn a_table_rust_uses_that_differs_in_structure_is_incompatible() {
    let cases = [ // (name, in the queue database?, change, what the problem must name)
        ("dropped column", false, "ALTER TABLE app_configs DROP COLUMN value", "app_configs.value"),
        ("nullability", false,
         "ALTER TABLE app_configs RENAME TO old; CREATE TABLE app_configs (id integer PRIMARY KEY, created_at datetime(6) NOT NULL, key varchar, updated_at datetime(6) NOT NULL, value text); DROP TABLE old; CREATE UNIQUE INDEX index_app_configs_on_key ON app_configs (key);",
         "app_configs.key"),
        ("unique index", false, "DROP INDEX index_app_configs_on_key", "app_configs (key)"),
        ("queue column", true, "ALTER TABLE solid_queue_processes RENAME COLUMN last_heartbeat_at TO heartbeat", "solid_queue_processes.last_heartbeat_at"),
    ];
    for (name, in_queue, sql, named) in cases {
        let dir = common::rails_install();
        let p = paths(dir.path());
        exec(if in_queue { &p.queue } else { &p.primary }, sql);
        match refusal(&p) {
            StoreError::Incompatible { problems } => assert!(problems.iter().any(|m| m.contains(named)), "{name}: {problems:?}"),
            other => panic!("{name}: expected Incompatible, got {other:?}"),
        }
    }
}
