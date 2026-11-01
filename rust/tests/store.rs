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

/// rust/schema/rails_versions.txt (what the build embeds) is exactly db/migrate's list while db/ exists (B1; B11 deletes
/// this test with db/).
#[test]
fn the_frozen_rails_versions_are_exactly_the_migrations_in_db_migrate() {
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
        // An order id is unique per venue only; the old global key would refuse one venue's order for another's id.
        ("transactions' old unique index", false,
         "DROP INDEX index_transactions_on_exchange_id_and_external_id; CREATE UNIQUE INDEX index_transactions_on_external_id ON transactions (external_id);",
         "transactions (exchange_id, external_id)"),
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

// --- B2-4: store::backup (PLAN-B2 r3.4) ---

/// A 2.x install's two files at `p` (each made from the Rails template).
fn install_at(p: &Paths) {
    let template = common::rails_install_unmigrated();
    for (from, to) in [("production.sqlite3", &p.primary), ("production_queue.sqlite3", &p.queue)] {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(template.path().join(from), to).unwrap();
    }
}

fn made(b: Result<store::Backup, store::BackupError>) -> std::path::PathBuf {
    match b { Ok(store::Backup::Made(dir)) => dir, other => panic!("expected a new copy, got {other:?}") }
}

/// QUEUE_DATABASE_PATH may name a file with the primary's name in another folder, or a file may be named like the digest:
/// one folder can't hold them all, so the start stops before anything is copied, naming the file.
#[test]
fn a_queue_named_like_the_primary_or_the_digest_stops_before_any_copy() {
    for (primary, queue, name) in [("data/db.sqlite3", "queue/db.sqlite3", "db.sqlite3"), ("data/digest", "queue/q.sqlite3", "digest")] {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths { primary: dir.path().join(primary), queue: dir.path().join(queue) };
        install_at(&p);
        assert_eq!(store::backup(&p, chrono::Utc::now()), Err(store::BackupError::SameName(name.into())));
        assert!(!dir.path().join("data/backups").exists(), "{name}: nothing made");
    }
}

/// Neither a full disk nor a folder that can't be written to: the system's own words, naming the path.
#[test]
fn any_other_failure_is_told_in_the_systems_words() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    install_at(&p);
    std::fs::write(dir.path().join("backups"), b"a file where the folder goes").unwrap();
    match store::backup(&p, chrono::Utc::now()) {
        Err(store::BackupError::Other(why)) => assert!(why.contains(&dir.path().join("backups").display().to_string()), "{why}"),
        other => panic!("{other:?}"),
    }
}

/// A read-only `backups/` is "can't be written to", and leaves no copy behind (Unix mode bits).
#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_written_to_is_said_so() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    install_at(&p);
    let shelf = dir.path().join("backups");
    std::fs::create_dir(&shelf).unwrap();
    std::fs::set_permissions(&shelf, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(store::backup(&p, chrono::Utc::now()), Err(store::BackupError::NotWritable));
    std::fs::set_permissions(&shelf, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read_dir(&shelf).unwrap().count(), 0);
}

/// The newest copy damaged after it was made (a file truncated): an unchanged install is copied again, not deduplicated
/// against a digest its files no longer match.
#[test]
fn a_damaged_newest_copy_is_not_taken_as_the_install_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    install_at(&p);
    let first = made(store::backup(&p, chrono::Utc::now()));
    assert!(matches!(store::backup(&p, chrono::Utc::now()), Ok(store::Backup::Unchanged(d)) if d == first), "unchanged while intact");
    std::fs::write(first.join("production.sqlite3"), b"").unwrap();
    made(store::backup(&p, chrono::Utc::now()));
}

/// A clock set back: each new copy still sorts after the newest, and names keep one length.
#[test]
fn copies_made_under_a_clock_set_back_sort_after_the_newest_with_bounded_names() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    install_at(&p);
    let now = chrono::Utc::now();
    let mut names = vec![made(store::backup(&p, now))];
    for n in 0..5 {
        exec(&p.primary, &format!("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('k{n}', 'v', '2026-01-01', '2026-01-01')"));
        names.push(made(store::backup(&p, now - chrono::Duration::hours(1))));
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, names, "newest last");
    assert!(names.iter().all(|n| n.as_os_str().len() == names[0].as_os_str().len()), "{names:?}");
}
