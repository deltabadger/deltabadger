use deltabadger::store::{self, Artifacts, Paths, Preflight, StoreError, EMBEDDED};
use std::io::Write;
use std::path::Path;

fn paths(dir: &Path) -> Paths { Paths::from_env(&|_| None, dir) }

fn versions(c: &rusqlite::Connection) -> Vec<String> {
    let mut s = c.prepare("SELECT version FROM schema_migrations ORDER BY version").unwrap();
    s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

fn gz(sql: &str) -> &'static [u8] {
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(sql.as_bytes()).unwrap();
    Box::leak(z.finish().unwrap().into_boxed_slice())
}

#[test]
fn paths_follow_rails_env_names_with_storage_defaults() {
    let p = Paths::from_env(&|k| (k == "QUEUE_DATABASE_PATH").then(|| "/q/queue.sqlite3".to_string()), Path::new("/data"));
    assert_eq!(p.primary, Path::new("/data/production.sqlite3"));
    assert_eq!(p.queue, Path::new("/q/queue.sqlite3"));
    assert_eq!(p.cable, Path::new("/data/production_cable.sqlite3"));
    assert_eq!(p.lock_file(), Path::new("/data/.engine.lock"));
}

#[test]
fn a_missing_install_is_created_complete_and_seeded() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(store::preflight(&paths(dir.path()), &EMBEDDED).unwrap(), Preflight::Create));
    let opened = store::open(&paths(dir.path()), &EMBEDDED).unwrap();
    assert!(opened.created);
    let kraken: i64 = opened.primary.query_row("SELECT count(*) FROM exchanges WHERE type = 'Exchanges::Kraken'", [], |r| r.get(0)).unwrap();
    assert_eq!(kraken, 1);
    let jm: String = opened.primary.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(jm, "wal");
    let sq: i64 = opened.queue.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'solid_queue_processes'", [], |r| r.get(0)).unwrap();
    assert_eq!(sq, 1);
    for name in ["production_cache.sqlite3", "production_cable.sqlite3"] { assert!(dir.path().join(name).exists(), "{name}"); }
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".creating")).collect();
    assert!(leftovers.is_empty(), "temporary build files are renamed into place");
}

#[test]
fn a_failed_creation_leaves_nothing_that_looks_like_an_install() {
    let dir = tempfile::tempdir().unwrap();
    let broken = Artifacts { seed_gz: gz("INSERT INTO no_such_table VALUES (1);"), ..EMBEDDED };
    assert!(store::open(&paths(dir.path()), &broken).is_err());
    assert!(!dir.path().join("production.sqlite3").exists(), "no half-built primary");
    let opened = store::open(&paths(dir.path()), &EMBEDDED).unwrap();
    assert!(opened.created, "the next start builds it from scratch, seed included");
}

#[test]
fn reopening_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let before = versions(&store::open(&paths(dir.path()), &EMBEDDED).unwrap().primary);
    assert!(matches!(store::preflight(&paths(dir.path()), &EMBEDDED).unwrap(), Preflight::Upgrade { pending } if pending.is_empty()));
    let again = store::open(&paths(dir.path()), &EMBEDDED).unwrap();
    assert!(!again.created && again.applied_twins.is_empty());
    assert_eq!(versions(&again.primary), before);
}

#[test]
fn pending_twins_are_applied_in_order_with_their_version() {
    let dir = tempfile::tempdir().unwrap();
    store::open(&paths(dir.path()), &EMBEDDED).unwrap();
    let with_twin = Artifacts { twins: &[("29990101000000", "CREATE TABLE rust_twin_probe (id integer);")], ..EMBEDDED };
    let opened = store::open(&paths(dir.path()), &with_twin).unwrap();
    assert_eq!(opened.applied_twins, vec!["29990101000000".to_string()]);
    assert!(versions(&opened.primary).contains(&"29990101000000".to_string()));
}

#[test]
fn a_database_from_a_newer_app_is_refused_before_any_file_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    drop(store::open(&p, &EMBEDDED).unwrap()); // WAL mode, as Rails runs it
    rusqlite::Connection::open(&p.primary).unwrap().execute("INSERT INTO schema_migrations VALUES ('29991231000000')", []).unwrap();
    for aux in [&p.queue, &p.cache, &p.cable] { std::fs::remove_file(aux).unwrap(); }
    // The guarantee is about database CONTENTS: a read-only WAL reader may create -shm/-wal sidecars,
    // but never writes the main file (it cannot checkpoint).
    let before = std::fs::read(&p.primary).unwrap();
    match store::open(&p, &EMBEDDED) {
        Err(StoreError::NewerSchema { unknown }) => assert_eq!(unknown, vec!["29991231000000".to_string()]),
        other => panic!("expected NewerSchema, got {:?}", other.map(|o| o.created)),
    }
    assert_eq!(std::fs::read(&p.primary).unwrap(), before, "primary unchanged");
    for aux in [&p.queue, &p.cache, &p.cable] { assert!(!aux.exists(), "{aux:?} must not be created on refusal"); }
}

#[test]
fn a_foreign_sqlite_file_is_refused_and_preserved() {
    for victim in ["production.sqlite3", "production_queue.sqlite3"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(victim);
        rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TABLE sentinel (v text); INSERT INTO sentinel VALUES ('keep');").unwrap();
        assert!(matches!(store::open(&paths(dir.path()), &EMBEDDED), Err(StoreError::Unrecognised { .. })), "{victim}");
        let v: String = rusqlite::Connection::open(&path).unwrap().query_row("SELECT v FROM sentinel", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "keep", "{victim} left intact");
        assert!(!dir.path().join("production_cache.sqlite3").exists(), "nothing else created");
    }
}

#[test]
fn a_zero_byte_file_counts_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("production.sqlite3"), b"").unwrap();
    assert!(store::open(&paths(dir.path()), &EMBEDDED).unwrap().created);
}

#[test]
fn a_database_older_than_the_baseline_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let first = versions(&store::open(&p, &EMBEDDED).unwrap().primary).remove(0);
    rusqlite::Connection::open(&p.primary).unwrap().execute("DELETE FROM schema_migrations WHERE version = ?1", [&first]).unwrap();
    assert!(matches!(store::open(&p, &EMBEDDED), Err(StoreError::OlderSchema { missing }) if missing == vec![first]));
}
