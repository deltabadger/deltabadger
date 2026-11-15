//! PLAN-B11-9 §2.4/§5: the frozen beta fixture, tests/fixtures/b11_9_beta. With `B11_9_FREEZE=1` the tests that call
//! `freeze` write their end state there as SQL text: installs the pre-rename (beta) writers produced, for the rename
//! migration's upgrade tests. Once the writers use the new names they can't produce these any more: the files are frozen
//! data, never regenerated after the rename (its PR deletes the calls).
use rusqlite::{types::ValueRef, Connection};
use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/b11_9_beta") }

/// Writes `<name>.primary.sql` (and `<name>.queue.sql`) when `B11_9_FREEZE=1`; otherwise nothing.
pub fn freeze(name: &str, primary: &Connection, queue: Option<&Connection>) {
    if std::env::var("B11_9_FREEZE").map_or(true, |v| v != "1") { return }
    std::fs::create_dir_all(dir()).unwrap();
    std::fs::write(dir().join(format!("{name}.primary.sql")), dump(primary)).unwrap();
    if let Some(q) = queue { std::fs::write(dir().join(format!("{name}.queue.sql")), dump(q)).unwrap(); }
}

/// The database as SQL that rebuilds it: each table, its rows with their rowids, `sqlite_sequence`, then the indexes,
/// triggers and views (after the rows, so no trigger fires while loading), each in `sqlite_master` order.
pub fn dump(c: &Connection) -> String {
    let objects: Vec<(String, String, String)> = c.prepare("SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY rowid").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap();
    let ident = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = String::from("PRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;\n");
    for (_, name, sql) in objects.iter().filter(|(ty, _, _)| ty == "table") {
        out.push_str(&format!("{sql};\n"));
        let columns: Vec<String> = c.prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE hidden = 0").unwrap()
            .query_map([name], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        let list = std::iter::once("rowid".to_string()).chain(columns.iter().map(|c| ident(c))).collect::<Vec<_>>().join(", ");
        let mut s = c.prepare(&format!("SELECT {list} FROM {} ORDER BY rowid", ident(name))).unwrap();
        let mut rows = s.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let values: Vec<String> = (0..=columns.len()).map(|i| literal(r.get_ref(i).unwrap())).collect();
            out.push_str(&format!("INSERT INTO {} ({list}) VALUES ({});\n", ident(name), values.join(", ")));
        }
    }
    let sequence: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'sqlite_sequence')", [], |r| r.get(0)).unwrap();
    if sequence {
        out.push_str("DELETE FROM sqlite_sequence;\n");
        let mut s = c.prepare("SELECT name, seq FROM sqlite_sequence ORDER BY rowid").unwrap();
        let mut rows = s.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            out.push_str(&format!("INSERT INTO sqlite_sequence (name, seq) VALUES ({}, {});\n", literal(r.get_ref(0).unwrap()), literal(r.get_ref(1).unwrap())));
        }
    }
    for (_, _, sql) in objects.iter().filter(|(ty, _, _)| ty != "table") { out.push_str(&format!("{sql};\n")); }
    out.push_str("COMMIT;\n");
    out
}

fn literal(v: ValueRef) -> String {
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => format!("{f:?}"),
        ValueRef::Text(t) => format!("'{}'", String::from_utf8(t.to_vec()).unwrap().replace('\'', "''")),
        ValueRef::Blob(b) => format!("X'{}'", hex::encode(b)),
    }
}

/// Every frozen install, loaded into its own directory as an install (production.sqlite3 and production_queue.sqlite3;
/// a fixture without a queue dump gets an empty queue), by name.
pub fn installs() -> Vec<(String, tempfile::TempDir)> {
    let mut names: Vec<String> = std::fs::read_dir(dir()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter_map(|f| f.strip_suffix(".primary.sql").map(str::to_string)).collect();
    names.sort();
    names.into_iter().map(|name| {
        let install = tempfile::tempdir().unwrap();
        let primary = Connection::open(install.path().join("production.sqlite3")).unwrap();
        primary.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        primary.execute_batch(&std::fs::read_to_string(dir().join(format!("{name}.primary.sql"))).unwrap()).unwrap();
        let queue = Connection::open(install.path().join("production_queue.sqlite3")).unwrap();
        match std::fs::read_to_string(dir().join(format!("{name}.queue.sql"))) {
            Ok(sql) => { queue.execute_batch("PRAGMA journal_mode=WAL").unwrap(); queue.execute_batch(&sql).unwrap(); }
            Err(_) => deltabadger::store::create_queue_schema(&queue).unwrap(),
        }
        (name, install)
    }).collect()
}
