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
        ValueRef::Real(f) if f.is_infinite() => (if f > 0.0 { "9e999" } else { "-9e999" }).into(),
        ValueRef::Real(f) => format!("{f:?}"),
        ValueRef::Text(t) => match std::str::from_utf8(t) {
            Ok(text) => format!("'{}'", text.replace('\'', "''")),
            Err(_) => format!("CAST(X'{}' AS TEXT)", hex::encode(t)), // bytes a text cell holds that are not UTF-8, as stored
        },
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

/// PLAN-B11 B11-1: where Rails' frozen answers live, one directory per recording (`frozen`'s name).
pub fn recorded() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/recorded") }

const SQLITE: &[u8] = b"SQLite format 3\0";
const PRAGMAS: [&str; 6] = ["page_size", "auto_vacuum", "encoding", "user_version", "application_id", "journal_mode"];

/// Rails' answer under recorded/<name> (PLAN-B11 §2 T1/T3/T4/T5), frozen at the commit tagged rails-oracle-final: each
/// SQLite database stored as `<file>.sql` (`dump`) and `<file>.pragmas.json`, every other file as it is. The caller gets
/// the restored files, built once per test run (super::run_template) and shared, so it copies what it writes to.
pub fn frozen(name: &str, is_whole: fn(&Path) -> bool) -> PathBuf {
    let name = name.replace('/', "_");
    super::run_template(&format!("frozen-{name}"), is_whole, |out| {
        let at = recorded().join(&name);
        assert!(at.is_dir(), "{name}: no frozen Rails answer at {}", at.display());
        restore_tree(&at, out);
    })
}

fn restore_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (path, name) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
        if path.is_dir() { restore_tree(&path, &to.join(&name)); continue }
        if name.ends_with(".pragmas.json") { continue }
        let db = name.strip_suffix(".sql").filter(|db| from.join(format!("{db}.pragmas.json")).is_file());
        match db {
            Some(db) => {
                let pragmas = serde_json::from_slice(&std::fs::read(from.join(format!("{db}.pragmas.json"))).unwrap()).unwrap();
                load(&to.join(db), &current_version(&std::fs::read_to_string(&path).unwrap()), &pragmas);
            }
            None => match std::fs::read_to_string(&path).ok().filter(|t| t.contains(FROZEN_VERSION)) {
                Some(text) => std::fs::write(to.join(&name), current_version(&text)).unwrap(),
                None => { std::fs::copy(&path, to.join(&name)).unwrap(); }
            },
        }
    }
}

/// The release the freeze ran on (rails-oracle-final). Rust shows the current release (`deltabadger::VERSION`), so each
/// bump would fail every recording that shows it: `restore_tree` swaps it in the two places 2.x shows the version, the
/// settings nav and MCP's serverInfo, raw or JSON-escaped, and nowhere else.
pub const FROZEN_VERSION: &str = "3.0.0-beta.8";
const VERSION_AT: [&str; 4] = [r#"settings__version">"#, r#"settings__version\">"#, r#""Deltabadger","version":""#, r#""Deltabadger\",\"version\":\""#];

pub fn current_version(text: &str) -> String {
    VERSION_AT.iter().fold(text.to_string(), |t, at| t.replace(&format!("{at}{FROZEN_VERSION}"), &format!("{at}{}", deltabadger::VERSION)))
}

fn pragmas(c: &Connection) -> serde_json::Map<String, serde_json::Value> {
    PRAGMAS.iter().map(|p| {
        let v: rusqlite::types::Value = c.query_row(&format!("PRAGMA {p}"), [], |r| r.get(0)).unwrap();
        (p.to_string(), match v { rusqlite::types::Value::Integer(i) => i.into(), rusqlite::types::Value::Text(t) => t.into(), other => panic!("PRAGMA {p}: {other:?}") })
    }).collect()
}

/// A new database at `path` from `sql` (`dump`) and its pragmas: the file-format ones before the first table, the rest
/// after; closed, so no -wal content is left.
fn load(path: &Path, sql: &str, pragmas: &serde_json::Map<String, serde_json::Value>) {
    let c = Connection::open(path).unwrap();
    let value = |p: &str| pragmas[p].as_i64().map_or_else(|| format!("'{}'", pragmas[p].as_str().unwrap()), |i| i.to_string());
    for p in ["page_size", "auto_vacuum", "encoding"] { c.execute_batch(&format!("PRAGMA {p} = {}", value(p))).unwrap(); }
    c.execute_batch(sql).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for p in ["user_version", "application_id"] { c.execute_batch(&format!("PRAGMA {p} = {}", value(p))).unwrap(); }
    let _: String = c.query_row(&format!("PRAGMA journal_mode = {}", pragmas["journal_mode"].as_str().unwrap()), [], |r| r.get(0)).unwrap();
}

/// One Rails command's answer on `dir` (`args`, whose last one is `dir`), frozen by what it was given: recorded/<test
/// binary>/<command>-<digest of dir's contents> holds the files the command created or changed, laid over `dir`. An
/// input that was never frozen (the test now hands Rails something else) fails: no Rails is left to answer it.
pub fn step(args: &[&str]) {
    let dir = Path::new(args.last().expect("the directory the command writes to"));
    let command = args[..args.len() - 1].join("_").replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "_");
    let digest = { use sha2::{Digest, Sha256}; let mut h = Sha256::new(); for (path, text) in &contents(dir) { h.update(path.as_os_str().as_encoded_bytes()); h.update([0]); h.update(text); h.update([0]); } hex::encode(h.finalize()) };
    let at = recorded().join(env!("CARGO_CRATE_NAME")).join(format!("{command}-{}", &digest[..16]));
    assert!(at.is_dir(), "Rails' answer to `{}` on this input is not frozen at {}: the input changed since the freeze", args[..args.len() - 1].join(" "), at.display());
    restore_over(&at, dir);
}

/// Every file under `dir` by relative path: a database as its dump, any other file as its bytes; a database's
/// -wal/-shm/-journal is part of the database.
fn contents(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, rel: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(root.join(rel)) else { return };
        for entry in entries.flatten() {
            let (name, path) = (entry.file_name().to_string_lossy().into_owned(), rel.join(entry.file_name()));
            if entry.file_type().unwrap().is_dir() { walk(root, &path, out); continue }
            if ["-wal", "-shm", "-journal"].iter().any(|s| name.strip_suffix(s).is_some_and(|db| root.join(rel).join(db).is_file())) { continue }
            let bytes = std::fs::read(root.join(&path)).unwrap();
            // A database is its rows and its pragmas: a command that changes only `user_version` or `journal_mode` changed it.
            let text = if bytes.starts_with(SQLITE) {
                let c = Connection::open(root.join(&path)).unwrap();
                format!("{}\n{}", dump(&c), serde_json::Value::Object(pragmas(&c))).into_bytes()
            } else { bytes };
            out.insert(path, text);
        }
    }
    let mut out = Default::default();
    walk(dir, Path::new(""), &mut out);
    out
}

/// `restore_tree` onto an existing directory: each recorded file replaces the one there (a database's -wal/-shm go).
fn restore_over(from: &Path, to: &Path) {
    let restored = tempfile::tempdir().unwrap();
    restore_tree(from, restored.path());
    fn lay(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let (path, dest) = (entry.path(), to.join(entry.file_name()));
            if path.is_dir() { lay(&path, &dest); continue }
            for suffix in ["-wal", "-shm"] { let _ = std::fs::remove_file(to.join(format!("{}{suffix}", entry.file_name().to_string_lossy()))); }
            std::fs::copy(&path, &dest).unwrap();
        }
    }
    lay(restored.path(), to);
}
