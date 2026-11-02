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
    // The PL carry the user declares (D-TAX-PL-DECLARED-CARRY).
    c.execute_batch(include_str!("../migrations/20261104000001_rust_tax_pl_carry.sql")).unwrap();
    // The ČNB fixings and their coverage (D-TAX-CZ-CNB).
    c.execute_batch(include_str!("../migrations/20261105000001_cnb_rates.sql")).unwrap();
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

/// A scenario's NBP and ČNB rates and coverage, as given: coverage is never inferred from the rates.
fn nbp(c: &Connection, s: &Value) {
    for r in s["cnb"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO cnb_rates (currency, date, rate) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    for r in s["cnb_coverage"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO cnb_coverage (from_date, to_date) VALUES (?1, ?2)", rusqlite::params![r[0].as_str(), r[1].as_str()]).unwrap();
    }
    for r in s["nbp"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO nbp_rates (currency, date, rate) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    for r in s["nbp_coverage"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO nbp_coverage (currency, from_date, to_date) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    // [user, year, costs]: the scenario's user ("self") or another ("other").
    let me = s["user_id"].as_i64().unwrap();
    for r in s["pl_carry"].as_array().into_iter().flatten() {
        let user = if r[0] == "self" { me } else { me + 1 };
        c.execute("INSERT INTO rust_tax_pl_carry (user_id, year, costs, updated_at) VALUES (?1, ?2, ?3, '2026-09-10 12:00:00')",
                  rusqlite::params![user, r[1].as_i64(), r[2].as_str()]).unwrap();
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
    for s in scenarios.iter().filter(|s| s["scope"] != "broker") {
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
        // "refuses: D-TAX-… <reason> <symbols…>": Rust refuses where Rails writes a file.
        if let Some(rest) = expect.strip_prefix("refuses: ") {
            let mut words = rest.split(' ');
            let (id, reason) = (words.next().unwrap(), words.next().unwrap().to_string());
            assert!(id.starts_with("D-TAX-"), "{name}: {id}");
            let want = Outcome::Refused { reason, symbols: words.map(String::from).collect() };
            if got != want { failures.push(format!("{name}: {id}: {got:?}")); }
            continue;
        }
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

/// D-TAX-PL-DECLARED-CARRY: a declaration for a later year is not read, so the 2024 file is the one without it.
#[test]
fn a_later_years_declared_carry_is_not_read() {
    let file = |name: &str| std::fs::read_to_string(divergence(name)).unwrap();
    assert_eq!(file("pl_declared_future_ignored_2024"), file("pl_carry_two_years_2024"));
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
    for country in ["FR", "NL", "CH", "XX"] {
        assert!(matches!(crypto_report(&c, 1, country, 2026, false, today), Err(FiguresError::NotComputed(m)) if m.contains(&format!("jurisdiction {country}"))), "{country}");
    }
    // Poland's regime starts in 2019.
    assert!(matches!(crypto_report(&c, 1, "PL", 2018, false, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction PL")));
    // D-TAX-GR-NO-TAX: Greece is served up to 2026 only, until enacted law is reviewed.
    assert!(matches!(crypto_report(&c, 1, "GR", 2027, false, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction GR")));
    // D-TAX-IT-YEARS: Italy 2023–2026; D-TAX-IE-YEARS: Ireland 2013–2026; D-TAX-SE-YEARS, D-TAX-GB-YEARS: 2020–2026.
    for (country, year) in [("IT", 2022), ("IT", 2027), ("IE", 2012), ("IE", 2027), ("SE", 2019), ("SE", 2027), ("GB", 2019), ("GB", 2027)] {
        assert!(matches!(crypto_report(&c, 1, country, year, false, today), Err(FiguresError::NotComputed(m)) if m.contains(&format!("jurisdiction {country}"))), "{country} {year}");
    }
}

/// The Ruby this report mirrors, by content: a change fails here until the port is checked and the vectors recorded again.
#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let v = vectors();
    let pinned = v["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 23);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the tax vectors were recorded");
    }
}

/// The text before the broker report's legal notes (D-TAX-BROKER-DISCLOSURES: every broker file differs there).
fn before_disclosures(csv: &str) -> &str {
    let header = deltabadger::web::i18n::text("de", "tax_report.broker.sections.disclosures", &[]);
    csv.find(&format!("\n{header}\n")).map_or(csv, |at| &csv[..at])
}

/// PLAN-A12 A12-7: the German broker report against Rails (`scope: broker` scenarios). "parity": Rails' bytes up to the legal
/// notes, then this build's notes (D-TAX-BROKER-DISCLOSURES); "diverges: D-TAX-BROKER-…": the reviewed file; "failed": the
/// run fails as Rails' ArgumentError does. "fetch": Rails' side of a fetch, whose Rust side is tax_job.rs's.
#[test]
fn the_broker_report_is_rails_own_or_the_named_divergence() {
    let v = vectors();
    let scenarios: Vec<&Value> = v["scenarios"].as_array().unwrap().iter().filter(|s| s["scope"] == "broker").collect();
    assert!(scenarios.len() >= 30, "{}", scenarios.len());
    let write = std::env::var("TAX_DIVERGENCES_WRITE").is_ok();
    let mut failures = vec![];
    for s in scenarios {
        let name = s["name"].as_str().unwrap();
        let expect = s["expect"].as_str().unwrap();
        if expect == "fetch" { continue; }
        let c = install(&s["tables"]);
        let (user, year) = (s["user_id"].as_i64().unwrap(), s["year"].as_i64().unwrap());
        let checked = deltabadger::tax::broker::check(&c, user, s["country"].as_str().unwrap(), year);
        if expect == "failed" {
            if s["rails"].get("failed").is_none() { failures.push(format!("{name}: Rails did not fail")); }
            if checked.is_ok() { failures.push(format!("{name}: the run must fail")); }
            continue;
        }
        if let Err(e) = checked { failures.push(format!("{name}: {e}")); continue; }
        let csv = match deltabadger::tax::broker::report(&c, user, year as i32, &Default::default()) { Ok(csv) => csv, Err(e) => { failures.push(format!("{name}: {e:?}")); continue } };
        let rails = s["rails"]["csv"].as_str().unwrap_or("");
        // D-TAX-BROKER-DISCLOSURES: this build's legal notes, every one, and none of the three Rails states that no longer hold.
        let notes = &csv[before_disclosures(&csv).len()..];
        for key in deltabadger::tax::broker::DISCLOSURES { if !notes.contains(&deltabadger::web::i18n::text("de", key, &[])) { failures.push(format!("{name}: {key} missing")); } }
        for gone in ["vorabpauschale_start_value", "interest_withholding_net", "withholding_credit_limit", "usd_balance_fx"] {
            if notes.contains(&deltabadger::web::i18n::text("de", &format!("tax_report.broker.disclosures.{gone}"), &[])) { failures.push(format!("{name}: {gone} still stated")); }
        }
        match expect.strip_prefix("diverges: ") {
            None => if before_disclosures(&csv) != before_disclosures(rails) {
                failures.push(format!("{name}: not Rails' file\n--- rails\n{rails}\n--- rust\n{csv}"));
            },
            Some(id) => {
                assert!(id.starts_with("D-TAX-BROKER-"), "{name}: {id}");
                if before_disclosures(&csv) == before_disclosures(rails) { failures.push(format!("{name}: {id} names a divergence, but Rust wrote Rails' figures")); }
                let path = divergence(name);
                if write { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(&path, &csv).unwrap(); }
                match std::fs::read_to_string(&path) {
                    Ok(want) if want == csv => {}
                    Ok(want) => failures.push(format!("{name}: not the reviewed {id} file\n--- reviewed\n{want}\n--- rust\n{csv}")),
                    Err(e) => failures.push(format!("{name}: {}: {e}", path.display())),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}:\n{}", failures.len(), failures.join("\n\n"));
}
