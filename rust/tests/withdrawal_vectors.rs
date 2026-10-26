//! The withdrawal-rule vectors (PLAN-withdrawal-rules X0), recorded from Rails by script/rust/record_withdrawal_vectors.rb
//! under the locked honeymaker. No engine reads them yet (E0 does); this holds the fixture to its pins and its shape.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).expect("withdrawal_vectors.json parses") }

fn cases<'a>(v: &'a Value, section: &str) -> &'a Vec<Value> { v[section].as_array().unwrap_or_else(|| panic!("no {section} section")) }

fn by_case<'a>(v: &'a Value, section: &str, name: &str) -> &'a Value {
    cases(v, section).iter().find(|c| c["case"] == name).unwrap_or_else(|| panic!("{section} has no case {name}"))
}

fn only_row(c: &Value) -> (&str, &str) {
    let rows = c["rule_logs"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{}: one rule_logs row", c["case"]);
    (rows[0]["status"].as_str().unwrap(), rows[0]["message"].as_str().unwrap())
}

#[test]
fn the_rails_sources_and_honeymaker_are_the_ones_recorded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let v = vectors();
    for (file, recorded) in v["sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: run script/rust/repin");
    }
    assert_eq!(v["gems"]["honeymaker"], "0.12.5", "the oracle is the honeymaker Rails is locked at");
    let lock = std::fs::read_to_string(root.join("Gemfile.lock")).unwrap();
    assert!(lock.contains("    honeymaker (0.12.5)\n"), "Gemfile.lock moved honeymaker: run script/rust/repin");
}

#[test]
fn every_plan_classifier_case_is_recorded_as_one_rails_row() {
    let v = vectors();
    for name in ["gemini_http_500_system", "gemini_http_400_insufficient_funds", "binance_code_4026", "binance_code_4027", "binance_html_502",
                 "kraken_max_fee_exceeded", "kraken_internal_error", "mexc_code_10101", "mexc_code_500",
                 "binance_timeout", "kraken_timeout", "gemini_timeout", "mexc_timeout", "binance_accepted_unreadable"] {
        by_case(&v, "classifier", name);
    }
    let withdraw = [("binance", "/sapi/v1/capital/withdraw/apply"), ("kraken", "/0/private/Withdraw"), ("gemini", "/v1/withdraw/btc"),
                    ("mexc", "/api/v3/capital/withdraw/apply")];
    for c in cases(&v, "classifier") {
        let (status, message) = only_row(c);
        let venue = c["venue"].as_str().unwrap();
        let sent = c["requests"].as_array().unwrap().iter().any(|r| withdraw.contains(&(venue, r["path"].as_str().unwrap())));
        if message.starts_with("Withdrawal failed: ") || message.starts_with("Unexpected error: ") {
            assert_eq!(status, "failed", "{}", c["case"]);
        } else {
            assert!(!sent, "{}: a row that is not a failure sent nothing", c["case"]);
        }
    }
    // The status a venue answered never reaches the row (honeymaker 0.12.5 keeps it in `data` only): Gemini's HTTP 500
    // `System` and its HTTP 400 `InsufficientFunds` are told apart by their bodies alone.
    let (_, system) = only_row(by_case(&v, "classifier", "gemini_http_500_system"));
    assert_eq!(system, r#"Withdrawal failed: {"result":"error","reason":"System","message":"A system error occurred, please try again"}"#);
    let (_, funds) = only_row(by_case(&v, "classifier", "gemini_http_400_insufficient_funds"));
    assert!(funds.starts_with(r#"Withdrawal failed: {"result":"error","reason":"InsufficientFunds""#), "{funds}");
    assert_eq!(only_row(by_case(&v, "classifier", "binance_code_4027")).1, "Withdrawal failed: Failed to obtain tranId.");
    assert_eq!(only_row(by_case(&v, "classifier", "kraken_internal_error")).1, "Withdrawal failed: EGeneral:Internal error");
    assert!(only_row(by_case(&v, "classifier", "binance_accepted_unreadable")).1.starts_with("Unexpected error: "));
}

#[test]
fn the_execute_matrix_and_surfaces_are_whole() {
    let v = vectors();
    let statuses: BTreeSet<&str> = ["pending", "success", "failed", "transient"].into();
    let mut names = BTreeSet::new();
    for c in cases(&v, "execute") {
        assert!(names.insert(c["case"].as_str().unwrap()), "duplicate execute case {}", c["case"]);
        for row in c["rule_logs"].as_array().unwrap() { assert!(statuses.contains(row["status"].as_str().unwrap()), "{}", c["case"]); }
    }
    assert!(by_case(&v, "execute", "not_scheduled")["rule_logs"].as_array().unwrap().is_empty());
    for venue in ["binance", "kraken", "gemini", "mexc"] {
        assert_eq!(only_row(by_case(&v, "execute", &format!("success_{venue}"))).0, "success");
    }
    // Every API cell on both surfaces.
    let mut surfaces: std::collections::BTreeMap<&str, BTreeSet<&str>> = Default::default();
    for c in cases(&v, "api") { surfaces.entry(c["case"].as_str().unwrap()).or_default().insert(c["surface"].as_str().unwrap()); }
    assert!(surfaces.len() >= 30 && surfaces.values().all(|s| s.len() == 2), "{surfaces:?}");
    assert_eq!(by_case(&v, "pages", "post_withdrawals")["status"], 404);
}
