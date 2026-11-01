//! The crypto tax report against Rails (script/rust/tax_vectors.rb records fixtures/tax_vectors.json): each scenario's
//! install is rebuilt from the Rails schema and the rows Rails read, and the report must be Rails' own bytes or refusal,
//! or, for a scenario that says "diverges: D-TAX-…", the Rust file checked by hand and committed in
//! fixtures/tax_divergences/<name>.csv (TAX_DIVERGENCES_WRITE=1 writes them afresh, for review).
use deltabadger::figures::FiguresError;
use deltabadger::tax::{crypto_report, Outcome};
use rusqlite::{types::Value as Sql, Connection};
use serde_json::Value;
use std::path::PathBuf;

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/tax_vectors.json")).expect("tax vectors are JSON")
}

fn install(tables: &Value) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(include_str!("fixtures/settings_primary_schema.sql")).unwrap();
    c.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    // Rust's own NBP tables (D-TAX-PL-NBP), from the migration that creates them.
    c.execute_batch(include_str!("../migrations/20261103000001_nbp_rates.sql")).unwrap();
    for (table, rows) in tables.as_object().unwrap() {
        for row in rows.as_array().unwrap() {
            let row = row.as_object().unwrap();
            let columns: Vec<String> = row.keys().map(|k| format!("\"{k}\"")).collect();
            let marks = vec!["?"; row.len()].join(", ");
            let values: Vec<Sql> = row.values().map(|v| match v {
                Value::Null => Sql::Null,
                Value::Bool(b) => Sql::Integer(i64::from(*b)),
                Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
                Value::Number(n) => Sql::Real(n.as_f64().unwrap()),
                Value::String(s) => Sql::Text(s.clone()),
                other => Sql::Text(other.to_string()),
            }).collect();
            c.execute(&format!("INSERT INTO {table} ({}) VALUES ({marks})", columns.join(", ")), rusqlite::params_from_iter(values)).unwrap();
        }
    }
    c
}

/// A scenario's NBP rates and coverage, as given: coverage is never inferred from the rates.
fn nbp(c: &Connection, s: &Value) {
    for r in s["nbp"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO nbp_rates (currency, date, rate) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    for r in s["nbp_coverage"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO nbp_coverage (currency, from_date, to_date) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
}

fn rails(s: &Value) -> Outcome {
    let rails = &s["rails"];
    match rails.get("csv") {
        Some(csv) => Outcome::Csv(csv.as_str().unwrap().to_string()),
        None => Outcome::Refused {
            reason: rails["refused"]["reason"].as_str().unwrap().to_string(),
            symbols: rails["refused"]["symbols"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect(),
        },
    }
}

fn divergence(name: &str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tax_divergences").join(format!("{name}.csv")) }

#[test]
fn the_crypto_report_is_rails_own_or_the_named_divergence() {
    let v = vectors();
    let scenarios = v["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 147);
    let write = std::env::var("TAX_DIVERGENCES_WRITE").is_ok();
    let mut failures = vec![];
    for s in scenarios {
        let name = s["name"].as_str().unwrap();
        let country = s["country"].as_str().unwrap();
        let c = install(&s["tables"]);
        nbp(&c, s);
        let year = s["year"].as_i64().unwrap();
        let today = chrono::NaiveDate::parse_from_str(&s["now"].as_str().unwrap()[..10], "%Y-%m-%d").unwrap();
        let got = crypto_report(&c, s["user_id"].as_i64().unwrap(), country, year as i32,
                                s["stablecoin_as_fiat"].as_bool().unwrap(), today);
        // Recorded for its later slice: a jurisdiction this build does not serve states no figure.
        if !deltabadger::tax::jurisdictions::ported(country, year) {
            if !matches!(got, Err(FiguresError::NotComputed(_))) { failures.push(format!("{name}: {country} is not ported, got {got:?}")); }
            continue;
        }
        let got = match got { Ok(g) => g, Err(e) => { failures.push(format!("{name}: {e:?}")); continue } };
        let expect = s["expect"].as_str().unwrap();
        match expect.strip_prefix("diverges: ") {
            None => if got != rails(s) { failures.push(format!("{name}: not Rails' file\n--- rails\n{:?}\n--- rust\n{got:?}", rails(s))) },
            Some(id) => {
                assert!(id.starts_with("D-TAX-"), "{name}: {id}");
                let Outcome::Csv(csv) = &got else { failures.push(format!("{name}: refused, {got:?}")); continue };
                if *csv == match rails(s) { Outcome::Csv(r) => r, _ => String::new() } { failures.push(format!("{name}: {id} names a divergence, but Rust wrote Rails' file")); }
                let path = divergence(name);
                if write { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(&path, csv).unwrap(); }
                match std::fs::read_to_string(&path) {
                    Ok(want) if want == *csv => {}
                    Ok(want) => failures.push(format!("{name}: not the reviewed {id} file\n--- reviewed\n{want}\n--- rust\n{csv}")),
                    Err(e) => failures.push(format!("{name}: {}: {e}", path.display())),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios:\n{}", failures.len(), scenarios.len(), failures.join("\n\n"));
}

/// Every reviewed divergence file belongs to a scenario that says so.
#[test]
fn no_divergence_file_is_left_over() {
    let v = vectors();
    let named: std::collections::HashSet<String> = v["scenarios"].as_array().unwrap().iter()
        .filter(|s| s["expect"].as_str().unwrap().starts_with("diverges: ") && deltabadger::tax::jurisdictions::ported(s["country"].as_str().unwrap(), s["year"].as_i64().unwrap())).map(|s| format!("{}.csv", s["name"].as_str().unwrap())).collect();
    for f in std::fs::read_dir(divergence("x").parent().unwrap()).unwrap() {
        let f = f.unwrap().file_name().into_string().unwrap();
        assert!(named.contains(&f), "{f} has no diverging scenario");
    }
}

#[test]
fn jurisdictions_not_yet_ported_state_no_figure() {
    let c = install(&serde_json::json!({}));
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    for country in ["AT", "FR", "IT", "BG", "GR", "NL", "CH", "GB", "SE", "IE", "DK", "CZ", "XX"] {
        assert!(matches!(crypto_report(&c, 1, country, 2026, false, today), Err(FiguresError::NotComputed(m)) if m.contains(&format!("jurisdiction {country}"))), "{country}");
    }
    // Poland's regime starts in 2019.
    assert!(matches!(crypto_report(&c, 1, "PL", 2018, false, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction PL")));
}

/// The Ruby this report mirrors, by content: a change fails here until the port is checked and the vectors recorded again.
#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let v = vectors();
    let pinned = v["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 13);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the tax vectors were recorded");
    }
}
