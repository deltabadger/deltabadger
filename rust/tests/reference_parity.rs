#![cfg(unix)] // shells out to bin/rails, as rust/tests/parity.rs does
//! The reference-data jobs write exactly the rows Rails writes, for every scripted data-api payload of the grid
//! (script/rust/reference_data.rb is the Rails half).
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn rails(args: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL").env_remove("MARKET_DATA_URL").env_remove("MARKET_DATA_TOKEN")
            .env("APP_ROOT_URL", "http://localhost:3000").env("SKIP_TEST_DATABASE", "true");
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        let out = cmd.output().expect("bin/rails runs");
        assert!(out.status.success(), "bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    };
    run(&["runner", "script/rust/schema_template.rb"]);
    let mut full = vec!["runner", "script/rust/reference_data.rb"];
    full.extend_from_slice(args);
    run(&full);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// Named divergences (PLAN-A17 A17-2): (scenario, divergence, requests Rails makes and Rust does not, assets Rails mints,
/// tickers Rails writes). Rust's rows and requests are Rails' without those (and without the exchange assets of those
/// assets); the scenarios order them last, so every other row matches id for id.
type Divergent = (&'static str, &'static str, &'static [&'static str], &'static [&'static str], &'static [&'static str]);
const DIVERGENT: &[Divergent] = &[
    // Rails binds SYM to its first id and ONE to shared-id (first wins); Rust leaves SYM, ONE and TWO unresolved.
    ("cg-tickers-ambiguous", "D-CG-SYMBOL-AMBIGUOUS", &[], &["sym-one", "shared-id"], &["SYMUSD", "ONEUSD"]),
    // Kraken's crawl answers 429: Rails crawls Coinbase next and creates its new pair; Rust asks no further crawl.
    ("cg-tickers-429", "D-CG-429-NO-MORE-CRAWLS", &["/exchanges/gdax/tickers"], &["ethereum"], &["ETH-USD"]),
];

fn without(rails: &Value, (_, _, requests, assets, tickers): &Divergent) -> Value {
    let mut out = rails.clone();
    out["requests"] = Value::Array(rails["requests"].as_array().unwrap().iter().filter(|r| !requests.iter().any(|x| r.as_str().unwrap().contains(x))).cloned().collect());
    let changes = |t: &str| rails["changes"][t].as_array().unwrap().clone();
    let dropped: Vec<Value> = changes("assets").iter().filter(|c| assets.iter().any(|a| c["after"]["external_id"] == *a)).map(|c| c["id"].clone()).collect();
    out["changes"]["assets"] = Value::Array(changes("assets").into_iter().filter(|c| !dropped.contains(&c["id"])).collect());
    out["changes"]["exchange_assets"] = Value::Array(changes("exchange_assets").into_iter().filter(|c| !dropped.contains(&c["after"]["asset_id"])).collect());
    out["changes"]["tickers"] = Value::Array(changes("tickers").into_iter().filter(|c| !tickers.iter().any(|t| c["after"]["ticker"] == *t)).collect());
    out
}

/// PLAN-A17 A17-4, the CoinGecko category pass (the `coingecko-categories*` scenarios), compared with its named columns apart:
/// D-INDEX-TOP-N (Rust ranks Top Coins first; Rails' job never does: answered empty here, Rust refuses it and writes nothing,
/// so only its request is dropped) and D-CG-CATEGORY-STORED (Rust stores every valid member with its caps: `top_coins`
/// compared on Rails' first 10, `weights` apart). The rankings (rust_index_rankings) are Rust's only and not snapshotted.
fn categories_view(v: &Value) -> Value {
    let mut out = v.clone();
    out["requests"] = Value::Array(v["requests"].as_array().unwrap().iter()
        .filter(|r| { let r = r.as_str().unwrap(); !r.contains("/coins/markets?") || r.contains("category=") }).cloned().collect());
    for change in out["changes"]["indices"].as_array_mut().unwrap() {
        for side in ["before", "after"] {
            let Some(row) = change[side].as_object_mut() else { continue };
            row.remove("weights");
            if let Some(top) = row.get_mut("top_coins").and_then(Value::as_array_mut) { top.truncate(10); }
        }
    }
    out
}

/// (scenario, divergence, Rails requests Rust does not make): Rust retries each (Transient or RateLimited), Rails does not.
const CATEGORY_DIVERGENT: &[(&str, &str, &[&str])] = &[
    // A failed categories read: Rails returns; Rust's run is Transient, retried 4 x 15 min.
    ("coingecko-categories-failed", "D-CG-INDEX-PULL-RETRIED", &[]),
    // music answers 429: Rails asks pump-fun next (and skips it: no venue); Rust asks nothing more and ends RateLimited.
    ("coingecko-categories-429-midway", "D-CG-INDEX-429-STOPS", &["category=pump-fun"]),
];

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_write_the_same_reference_rows_for_every_scripted_payload() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 70, "the grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::reference(&rust_root.path().join(&name)).await.unwrap();
        // The venue catalogues are really read and applied, not failed alike on both sides.
        if name == "cg-tickers-import" {
            let venues = rails_out["requests"].as_array().unwrap().iter().filter(|r| r.as_str().unwrap().contains("api.kraken.com/") || r.as_str().unwrap().contains("api.coinbase.com/")).count();
            if venues != 2 || rails_out["changes"]["tickers"].as_array().unwrap().len() < 4 { failures.push(format!("{name}: the catalogues were not applied: {rails_out}")); }
        }
        if name.starts_with("coingecko-categories") {
            let (mut expected, rust) = (categories_view(&rails_out), categories_view(&rust_out));
            if name == "coingecko-categories" {
                // D-CG-CATEGORY-STORED, positively: payments' 12 valid members, Rails' first 10, Rust's caps.
                let payments = |o: &Value| o["changes"]["indices"].as_array().unwrap().iter().find(|c| c["after"]["external_id"] == "payments").map(|c| c["after"].clone()).unwrap_or_default();
                let (r, u) = (payments(&rails_out), payments(&rust_out));
                if r["top_coins"].as_array().map(Vec::len) != Some(10) || u["top_coins"].as_array().map(Vec::len) != Some(12)
                    || r["weights"] != serde_json::json!({}) || u["weights"].as_object().map(serde_json::Map::len) != Some(12) {
                    failures.push(format!("{name}: D-CG-CATEGORY-STORED not seen\n  rails: {r}\n  rust:  {u}"));
                }
            }
            if let Some((_, divergence, requests)) = CATEGORY_DIVERGENT.iter().find(|d| d.0 == name) {
                if rails_out["retry"] != false || rust_out["retry"] != true { failures.push(format!("{name}: {divergence}: Rails retries {} and Rust {}", rails_out["retry"], rust_out["retry"])); }
                expected["retry"] = true.into();
                let before = expected["requests"].as_array().unwrap().len();
                expected["requests"] = Value::Array(expected["requests"].as_array().unwrap().iter().filter(|r| !requests.iter().any(|x| r.as_str().unwrap().contains(x))).cloned().collect());
                if expected["requests"].as_array().unwrap().len() != before - requests.len() { failures.push(format!("{name}: {divergence}: Rails no longer makes {requests:?}")); }
            }
            if expected != rust { failures.push(format!("{name}\n  rails: {expected}\n  rust:  {rust}")); }
            continue;
        }
        let expected = match DIVERGENT.iter().find(|d| d.0 == name) {
            Some(d) => {
                if rails_out == rust_out { failures.push(format!("{name}: Rails and Rust agree; the named divergence {} is gone", d.1)); }
                without(&rails_out, d)
            }
            None => rails_out,
        };
        if expected != rust_out { failures.push(format!("{name}\n  rails: {expected}\n  rust:  {rust_out}")); }
    }
    // Listed divergence: a JSON column's text. ActiveSupport escapes <, > and & (as \u003c and so on) and serde does not;
    // both snapshots compare the parsed JSON, which is equal.
    let weights = |root: &Path| -> String {
        rusqlite::Connection::open(root.join("indices-import/production.sqlite3")).unwrap()
            .query_row("SELECT weights FROM indices WHERE external_id = 'nasdaq-100'", [], |r| r.get(0)).unwrap()
    };
    let (rails_text, rust_text) = (weights(rails_root.path()), weights(rust_root.path()));
    if rails_text == rust_text || !rails_text.contains("\\u0026") {
        failures.push(format!("indices-import: Rails no longer escapes JSON text ({rails_text}): drop the listed divergence"));
    }
    if serde_json::from_str::<Value>(&rails_text).unwrap() != serde_json::from_str::<Value>(&rust_text).unwrap() {
        failures.push(format!("indices-import: the weights JSON differs\n  rails: {rails_text}\n  rust:  {rust_text}"));
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}
