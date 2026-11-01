//! store::seed is Rails' db/seeds.rb (PLAN-B1 §4, T5, r1.6): on a fresh baseline primary, for each SMTP_ADDRESS /
//! MARKET_DATA_URL combination script/rust/record_seeds.rb recorded, every row of every table equals Rails' (ids,
//! sqlite_sequence, decimals and timestamps as stored, app_configs decrypted). Plus the embedded bundle equals
//! db/seed_data (B1_REGENERATE=1 rewrites it and fails, so the result is reviewed and committed), and a rolled-back seed
//! leaves the baseline as it was.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::store;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

type Tables = BTreeMap<String, Vec<Vec<Option<String>>>>;

fn manifest(p: &str) -> std::path::PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join(p) }

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
/// decrypted, ar_internal_metadata without the baseline's stamps.
fn dump(c: &Connection) -> Tables {
    let cipher = common::seed::cipher();
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
            let at = 1 + columns.iter().position(|c| c == "value").unwrap();
            for r in &mut rows {
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

fn fresh() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    store::create_schema(&c).unwrap();
    (dir, c)
}

fn seeded(env: &BTreeMap<String, String>, now: DateTime<Utc>) -> Tables {
    let (_dir, mut c) = fresh();
    let tx = c.transaction().unwrap();
    store::seed(&tx, &common::seed::cipher(), &|k| env.get(k).cloned(), now).unwrap();
    tx.commit().unwrap();
    dump(&c)
}

#[test]
fn rust_seeds_what_rails_seeds_under_each_environment() {
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(manifest("tests/fixtures/seeds_rails.json")).unwrap()).unwrap();
    for (path, hash) in fixture["sources"].as_object().unwrap() {
        use sha2::Digest;
        let now = hex::encode(sha2::Sha256::digest(std::fs::read(manifest("..").join(path)).unwrap()));
        assert_eq!(&now, hash.as_str().unwrap(), "{path} changed: re-record seeds_rails.json (script/rust/repin)");
    }
    let now = DateTime::parse_from_rfc3339(fixture["now"].as_str().unwrap()).unwrap().with_timezone(&Utc);
    let tables = |s: &Value| -> Tables { serde_json::from_value(s["tables"].clone()).unwrap() };
    let scenarios = fixture["scenarios"].as_array().unwrap();
    let first = tables(&scenarios[0]);
    let names: Vec<&str> = scenarios.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["neither", "smtp_only", "market_data_only", "both", "whitespace_only"]);
    for s in scenarios {
        let name = s["name"].as_str().unwrap();
        let env: BTreeMap<String, String> = serde_json::from_value(s["env"].clone()).unwrap();
        let rails = tables(s);
        let rust = seeded(&env, now);
        assert_eq!(rust.keys().collect::<Vec<_>>(), first.keys().collect::<Vec<_>>(), "{name}: the tables");
        for (table, rows) in &rust {
            let expected = rails.get(table).unwrap_or(&first[table]);
            if let Some(i) = (0..rows.len().max(expected.len())).find(|&i| rows.get(i) != expected.get(i)) {
                panic!("{name}: {table} differs first at row {i} of {} (Rails {}):\n  Rails {:?}\n  Rust  {:?}",
                       rows.len(), expected.len(), expected.get(i), rows.get(i));
            }
        }
    }
    // The seed is not trivially empty: the reference tables and both defaults are there.
    for (table, at_least) in [("exchanges", 15), ("assets", 1000), ("indices", 100), ("tickers", 10_000), ("exchange_assets", 1000)] {
        assert!(first[table].len() >= at_least, "{table}: {} rows", first[table].len());
    }
    assert_eq!(tables(&scenarios[3])["app_configs"].len(), 2, "both defaults under both variables");
}

#[test]
fn a_rolled_back_seed_leaves_the_baseline_as_it_was() {
    let (_dir, mut c) = fresh();
    let before = dump(&c);
    let tx = c.transaction().unwrap();
    let env = |k: &str| Some(format!("{k}-set"));
    store::seed(&tx, &common::seed::cipher(), &env, Utc::now()).unwrap();
    assert!(tx.query_row("SELECT count(*) FROM tickers", [], |r| r.get::<_, i64>(0)).unwrap() > 0, "the seed wrote");
    tx.rollback().unwrap();
    assert_eq!(dump(&c), before);
}

/// The bundle as the generator writes it: `{"<path>": <the file's text>, ...}` in path (byte) order.
fn bundle_from_db() -> Option<String> {
    let root = manifest("../db/seed_data");
    if !root.exists() { return None; } // B11 removes db/: the committed bundle is then the source
    let mut paths = vec!["assets.json".to_string(), "indices.json".to_string()];
    for e in std::fs::read_dir(root.join("tickers")).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".json") { paths.push(format!("tickers/{name}")); }
    }
    paths.sort();
    let parts: Vec<String> = paths.iter()
        .map(|p| format!("{}:{}", Value::String(p.clone()), std::fs::read_to_string(root.join(p)).unwrap().trim_end())).collect();
    Some(format!("{{{}}}", parts.join(",")))
}

#[test]
fn the_embedded_bundle_is_db_seed_data() {
    let path = manifest("seed/seed_data.json.gz");
    if let Some(expected) = bundle_from_db() {
        if std::env::var("B1_REGENERATE").is_ok_and(|v| v == "1") {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            gz.write_all(expected.as_bytes()).unwrap();
            std::fs::write(&path, gz.finish().unwrap()).unwrap();
            panic!("B1_REGENERATE=1: rewrote {}; review and commit it, then run without B1_REGENERATE", path.display());
        }
        let mut text = String::new();
        flate2::read::GzDecoder::new(std::fs::File::open(&path).unwrap()).read_to_string(&mut text).unwrap();
        assert!(text == expected, "{} is not db/seed_data: regenerate it (B1_REGENERATE=1)", path.display());
    }
    let files = store::seed_files().unwrap();
    // store::seed passes no public URL to plan_assets: right only while no asset carries a logo_url.
    let logos = files["assets.json"]["data"].as_array().unwrap().iter().filter(|a| a["logo_url"].as_str().is_some_and(|s| !s.trim().is_empty())).count();
    assert_eq!(logos, 0, "assets with a logo_url need MarketDataSettings.deltabadger_public_url in store::seed");
}
