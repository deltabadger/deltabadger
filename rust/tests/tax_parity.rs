//! The US crypto tax report against Rails (script/rust/tax_vectors.rb records fixtures/tax_vectors.json): each scenario's
//! install is rebuilt from the Rails schema and the rows Rails read, and the report must be Rails' own bytes or refusal,
//! or, where the scenario says so, no figure at all with the stated reason: where Rails states a figure that cannot be defended.
use deltabadger::figures::FiguresError;
use deltabadger::tax::{crypto_report, Outcome};
use rusqlite::{types::Value as Sql, Connection};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/tax_vectors.json")).expect("tax vectors are JSON")
}

fn install(tables: &Value) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(include_str!("fixtures/settings_primary_schema.sql")).unwrap();
    c.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
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

#[test]
fn the_us_crypto_report_is_rails_own_or_states_no_figure() {
    let v = vectors();
    let scenarios = v["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 19);
    for s in scenarios {
        let name = s["name"].as_str().unwrap();
        let c = install(&s["tables"]);
        let today = chrono::NaiveDate::parse_from_str(&s["now"].as_str().unwrap()[..10], "%Y-%m-%d").unwrap();
        let got = crypto_report(&c, s["user_id"].as_i64().unwrap(), "US", s["year"].as_i64().unwrap() as i32, today);
        let expect = s["expect"].as_str().unwrap();
        match expect.strip_prefix("refuses: ") {
            None => {
                let rails = &s["rails"];
                let want = match rails.get("csv") {
                    Some(csv) => Outcome::Csv(csv.as_str().unwrap().to_string()),
                    None => Outcome::Refused {
                        reason: rails["refused"]["reason"].as_str().unwrap().to_string(),
                        symbols: rails["refused"]["symbols"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect(),
                    },
                };
                assert_eq!(got.unwrap_or_else(|e| panic!("{name}: {e:?}")), want, "{name}");
            }
            Some(reason) => match got {
                Err(FiguresError::NotComputed(m)) => assert!(m.contains(reason), "{name}: {m}"),
                other => panic!("{name}: Rails states a figure Rust must not ({reason}), got {other:?}"),
            },
        }
    }
}

#[test]
fn other_jurisdictions_are_not_ported_yet() {
    let c = install(&serde_json::json!({}));
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    assert!(matches!(crypto_report(&c, 1, "DE", 2026, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction DE")));
}

/// The Ruby this report mirrors, by content: a change fails here until the port is checked and the vectors recorded again.
#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let v = vectors();
    let pinned = v["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 10);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the tax vectors were recorded");
    }
}
