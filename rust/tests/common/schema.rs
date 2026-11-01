//! The comparisons the schema and seed tests share: tests/schema_baseline.rs's T1 snapshot and next ids, and
//! tests/seeds.rs's dump of every row (record_seeds.rb's `dump`).
use rusqlite::{types::Value, Connection};
use std::collections::BTreeMap;

pub type Rows = Vec<Vec<Value>>;

pub fn ident(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }
pub fn lit(text: &str) -> String { format!("'{}'", text.replace('\'', "''")) }

pub fn rows(c: &Connection, sql: &str) -> Rows {
    let mut st = c.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let n = st.column_count();
    st.query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect()).unwrap().map(Result::unwrap).collect()
}

pub fn texts(c: &Connection, sql: &str) -> Vec<String> {
    c.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

pub fn tables(c: &Connection) -> Vec<String> { texts(c, "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name") }

/// Everything T1 compares, by section, so a difference names its table.
pub fn snapshot(c: &Connection) -> BTreeMap<String, Rows> {
    let mut s = BTreeMap::new();
    // By (type, name), not rowid: Rails rebuilds tables for their foreign keys, so its rowid order differs.
    s.insert("sqlite_master".into(), rows(c, "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"));
    for pragma in ["journal_mode", "page_size", "auto_vacuum", "encoding", "user_version", "application_id"] {
        s.insert(format!("pragma {pragma}"), rows(c, &format!("PRAGMA {pragma}")));
    }
    for t in tables(c) {
        let arg = lit(&t);
        s.insert(format!("{t}: table_xinfo"), rows(c, &format!("SELECT * FROM pragma_table_xinfo({arg})")));
        s.insert(format!("{t}: index_list"), rows(c, &format!("SELECT * FROM pragma_index_list({arg})")));
        s.insert(format!("{t}: foreign_key_list"), rows(c, &format!("SELECT * FROM pragma_foreign_key_list({arg})")));
        for index in texts(c, &format!("SELECT name FROM pragma_index_list({arg})")) {
            s.insert(format!("{t}: index_xinfo {index}"), rows(c, &format!("SELECT * FROM pragma_index_xinfo({})", lit(&index))));
        }
        // applied_at is Utc::now; ar_internal_metadata's value and timestamps depend on the RAILS_ENV and the clock.
        let select = match t.as_str() {
            "rust_schema_migrations" => "rowid, version",
            "ar_internal_metadata" => "rowid, key",
            _ => "rowid, *",
        };
        s.insert(format!("{t}: rows"), rows(c, &format!("SELECT {select} FROM {} ORDER BY rowid", ident(&t))));
    }
    s
}

/// The id one plain insert into each table gets (AUTOINCREMENT tables continue from sqlite_sequence, the others from
/// max(rowid)). Every column but the rowid alias gets a dummy value; CHECKs and foreign keys are off for it.
pub fn next_ids(c: &Connection) -> BTreeMap<String, i64> {
    c.execute_batch("PRAGMA ignore_check_constraints = ON; PRAGMA foreign_keys = OFF").unwrap();
    tables(c).into_iter().filter(|t| !t.starts_with("sqlite_")).map(|t| {
        let mut st = c.prepare(&format!("SELECT name, type, pk, hidden FROM pragma_table_xinfo({})", lit(&t))).unwrap();
        let cols: Vec<(String, String, i64, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
        let keys = cols.iter().filter(|col| col.2 > 0).count();
        let given: Vec<String> = cols.iter()
            .filter(|(_, ty, pk, hidden)| *hidden == 0 && !(keys == 1 && *pk == 1 && ty.eq_ignore_ascii_case("INTEGER")))
            .map(|col| ident(&col.0)).collect();
        let sql = if given.is_empty() { format!("INSERT INTO {} DEFAULT VALUES", ident(&t)) } else {
            format!("INSERT INTO {} ({}) VALUES ({})", ident(&t), given.join(", "), vec!["'b1-next-id'"; given.len()].join(", "))
        };
        c.execute(&sql, []).unwrap_or_else(|e| panic!("{sql}: {e}"));
        (t, c.last_insert_rowid())
    }).collect()
}

pub type Tables = BTreeMap<String, Vec<Vec<Option<String>>>>;

/// record_seeds.rb's `cell`: "<typeof>:<text>", a real as its IEEE-754 bits (SQLite's printf differs between versions).
fn cell(v: rusqlite::types::Value) -> Option<String> {
    use rusqlite::types::Value as V;
    match v {
        V::Null => None,
        V::Integer(i) => Some(format!("integer:{i}")),
        V::Real(f) => Some(format!("real:{:016x}", f.to_bits())),
        V::Text(t) => Some(format!("text:{t}")),
        V::Blob(b) => Some(format!("blob:{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())),
    }
}

/// The recorder's dump (record_seeds.rb `dump`): every table, rowid first, each cell by `cell`, app_configs.value
/// decrypted (Rust's plain rust_* rows as stored), ar_internal_metadata without the baseline's stamps.
pub fn dump(c: &Connection) -> Tables {
    let cipher = super::seed::cipher();
    let names: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    names.into_iter().map(|table| {
        let mut columns: Vec<String> = c.prepare(&format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid")).unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        if table == "ar_internal_metadata" { columns.retain(|c| c != "created_at" && c != "updated_at"); }
        let sql = format!("SELECT rowid, {} FROM \"{table}\" ORDER BY rowid",
                          columns.iter().map(|col| format!("\"{col}\"")).collect::<Vec<_>>().join(", "));
        let mut st = c.prepare(&sql).unwrap();
        let n = st.column_count();
        let mut rows: Vec<Vec<Option<String>>> = st.query_map([], |r| (0..n).map(|i| Ok(cell(r.get(i)?))).collect()).unwrap().map(Result::unwrap).collect();
        if table == "app_configs" {
            let (key, at) = (1 + columns.iter().position(|c| c == "key").unwrap(), 1 + columns.iter().position(|c| c == "value").unwrap());
            for r in &mut rows {
                // Rust's own rows (rust_*) are plain by design (lease::TAKEN_OVER_AT, jobs::state).
                if r[key].as_deref().is_some_and(|k| k.starts_with("text:rust_")) { continue; }
                // Encrypted as Rails writes it (an envelope; decrypt alone would pass a plain value through).
                r[at] = r[at].take().map(|v| {
                    let stored = v.strip_prefix("text:").unwrap();
                    assert!(stored.starts_with("{\"p\":") && stored.contains("\"iv\""), "app_configs.value is not encrypted: {stored}");
                    format!("text:{}", cipher.decrypt(stored).unwrap())
                });
            }
        }
        (table, rows)
    }).collect()
}

