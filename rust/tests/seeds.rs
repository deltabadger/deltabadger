//! store::seed is Rails' db/seeds.rb (PLAN-B1 §4, T5, r1.6): on a fresh baseline primary, seeded from the bundle Rails'
//! answer was recorded with (tests/fixtures/seed_bundle_b11.json.gz, the bundle at the oracle commit), for each
//! SMTP_ADDRESS / MARKET_DATA_URL combination recorded, every row of every table equals Rails' (ids, sqlite_sequence,
//! decimals and timestamps as stored, app_configs decrypted); EXPECTED_REWRITE=1 rewrites seeds_expected.json from Rust's
//! rows (common::expected). The shipped bundle (rust/seed/seed_data.json.gz, refreshed by releases) is held to its shape
//! and floors (PLAN-B11 r1.4, PLAN-B8 r2.5), and a rolled-back seed leaves the baseline as it was.
mod common;
use common::schema::{dump, Tables};
use chrono::{DateTime, Utc};
use deltabadger::store;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

fn manifest(p: &str) -> std::path::PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join(p) }

fn fresh() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    store::create_schema(&c).unwrap();
    (dir, c)
}

/// The bundle Rails' recorded seeds were made from.
const FROZEN_BUNDLE: &[u8] = include_bytes!("fixtures/seed_bundle_b11.json.gz");

fn seeded(env: &BTreeMap<String, String>, now: DateTime<Utc>) -> Tables {
    let (_dir, mut c) = fresh();
    let tx = c.transaction().unwrap();
    store::test_support::seed_bundle(&tx, &common::seed::cipher(), &|k| env.get(k).cloned(), now, FROZEN_BUNDLE).unwrap();
    tx.commit().unwrap();
    dump(&c)
}

#[test]
fn rust_seeds_what_v2_seeds_under_each_environment() {
    let path = manifest("tests/fixtures/seeds_expected.json");
    let mut fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let now = DateTime::parse_from_rfc3339(fixture["now"].as_str().unwrap()).unwrap().with_timezone(&Utc);
    let tables = |s: &Value| -> Tables { serde_json::from_value(s["tables"].clone()).unwrap() };
    let scenarios = fixture["scenarios"].as_array().unwrap();
    let first = tables(&scenarios[0]);
    if common::expected::rewriting() {
        // The first scenario whole; each later one only the tables that differ from the first's.
        let mut first: Option<Tables> = None;
        for s in fixture["scenarios"].as_array_mut().unwrap() {
            let env: BTreeMap<String, String> = serde_json::from_value(s["env"].clone()).unwrap();
            let mut tables = seeded(&env, now);
            match &first {
                None => first = Some(tables.clone()),
                Some(first) => tables.retain(|table, rows| first.get(table) != Some(rows)),
            }
            s["tables"] = serde_json::to_value(tables).unwrap();
        }
        common::expected::rewrite(&path, (layout(&fixture, 0) + "\n").as_bytes());
    }
    let names: Vec<&str> = scenarios.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["neither", "smtp_only", "market_data_only", "both", "whitespace_only"]);
    for s in scenarios {
        let name = s["name"].as_str().unwrap();
        let env: BTreeMap<String, String> = serde_json::from_value(s["env"].clone()).unwrap();
        let v2 = tables(s);
        let rust = seeded(&env, now);
        assert_eq!(rust.keys().collect::<Vec<_>>(), first.keys().collect::<Vec<_>>(), "{name}: the tables");
        for (table, rows) in &rust {
            let expected = v2.get(table).unwrap_or(&first[table]);
            if let Some(i) = (0..rows.len().max(expected.len())).find(|&i| rows.get(i) != expected.get(i)) {
                panic!("{name}: {table} differs first at row {i} of {} (2.x {}):\n  2.x {:?}\n  Rust  {:?}",
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

/// seeds_expected.json's layout: indented by two, but a row (or an env), whose values are all scalars, on one line.
fn layout(v: &Value, depth: usize) -> String {
    fn flat<'a>(mut items: impl Iterator<Item = &'a Value>) -> bool { items.all(|i| !i.is_array() && !i.is_object()) }
    let pad = |d: usize| "  ".repeat(d);
    match v {
        Value::Array(a) if a.is_empty() || flat(a.iter()) => v.to_string(),
        Value::Object(o) if o.is_empty() || flat(o.values()) => v.to_string(),
        Value::Array(a) => format!("[\n{}\n{}]", a.iter().map(|i| format!("{}{}", pad(depth + 1), layout(i, depth + 1))).collect::<Vec<_>>().join(",\n"), pad(depth)),
        Value::Object(o) => format!("{{\n{}\n{}}}", o.iter().map(|(k, i)| format!("{}{}: {}", pad(depth + 1), Value::String(k.clone()), layout(i, depth + 1))).collect::<Vec<_>>().join(",\n"), pad(depth)),
        _ => v.to_string(),
    }
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

/// PLAN-B8 r2.5's floors: the 13 exchanges with a ticker file (EXCHANGES without Alpaca and IBKR) each have one.
const TICKER_FILES: [&str; 13] = ["binance", "binance_us", "kraken", "coinbase", "bitget", "kucoin", "bybit", "mexc", "gemini",
                                  "bitvavo", "hyperliquid", "bingx", "bitrue"];

/// The shipped bundle: assets.json, indices.json and the 13 ticker files, each with rows, every ticker one the import
/// keeps; no logo_url; and a fresh install seeds from it.
#[test]
fn the_shipped_bundle_has_its_shape_and_floors() {
    let files = store::seed_files().unwrap();
    let mut expected: Vec<String> = ["assets.json", "indices.json"].map(String::from).to_vec();
    expected.extend(TICKER_FILES.iter().map(|x| format!("tickers/{x}.json")));
    expected.sort();
    assert_eq!(files.keys().cloned().collect::<Vec<_>>(), expected, "the bundle's files");
    for (path, file) in &files {
        assert!(file["data"].as_array().is_some_and(|rows| !rows.is_empty()), "{path} has no rows");
    }
    // store::seed passes no public URL to plan_assets: right only while no asset carries a logo_url.
    let logos = files["assets.json"]["data"].as_array().unwrap().iter().filter(|a| a["logo_url"].as_str().is_some_and(|s| !s.trim().is_empty())).count();
    assert_eq!(logos, 0, "assets with a logo_url need MarketDataSettings.deltabadger_public_url in store::seed");
    let (_dir, mut c) = fresh();
    let tx = c.transaction().unwrap();
    store::seed(&tx, &common::seed::cipher(), &|_| None, Utc::now()).unwrap();
    tx.commit().unwrap();
    let seeded: BTreeMap<String, i64> = c.prepare("SELECT e.type, count(t.id) FROM exchanges e LEFT JOIN tickers t ON t.exchange_id = e.id GROUP BY e.type").unwrap()
        .query_map([], |r| Ok((deltabadger::jobs::reference::name_id(&r.get::<_, String>(0)?).to_string(), r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for name in TICKER_FILES { assert!(seeded.get(name).is_some_and(|n| *n >= 1), "{name}: no ticker seeded ({seeded:?})"); }
}
