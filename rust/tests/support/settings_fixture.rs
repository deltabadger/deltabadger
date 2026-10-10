//! Schema exported from the pinned Rails page grid; runtime tests use only SQLite and native fixtures.
use deltabadger::store::{self, Paths};
pub fn install() -> (
    tempfile::TempDir,
    store::Opened,
    crate::common::seed::Seeded,
) {
    let dir = tempfile::tempdir().unwrap();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute_batch(&format!("BEGIN IMMEDIATE;\n{}\nCOMMIT;",include_str!("../fixtures/settings_primary_schema.sql")))
        .unwrap();
    drop(c);
    let c = rusqlite::Connection::open(dir.path().join("production_queue.sqlite3")).unwrap();
    c.execute_batch(&format!("BEGIN IMMEDIATE;\n{}\nCOMMIT;",include_str!("../fixtures/settings_queue_schema.sql")))
        .unwrap();
    drop(c);
    let opened = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let seed = crate::common::seed::seed_alpaca(&opened.primary, &crate::common::seed::cipher());
    (dir, opened, seed)
}
