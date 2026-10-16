//! The import readers against vectors recorded from Rails (script/rust/record_import_vectors.rb). Rails-free.
use base64::Engine;
use deltabadger::sync::activities::Entry;
use deltabadger::tracker::import::{binance, csv, deltabadger as own};
use deltabadger::web::tracker::{row::TYPES, CSV_HEADERS};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/import_vectors.json")).expect("import_vectors.json parses") }
fn b64(v: &Value) -> Vec<u8> { base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap() }
fn report(what: &str, failures: Vec<String>, total: usize) {
    assert!(failures.is_empty(), "{} of {total} {what} differ:\n{}", failures.len(), failures[..failures.len().min(10)].join("\n"));
}

/// The recorder's `serialize`: decimals as to_s('F'), times as iso8601(6), raw_data as its stored text.
fn serialize(e: &Entry, exchange: Option<&Option<String>>) -> Value {
    let d = |v: &Option<deltabadger::ruby::BigDec>| v.as_ref().map(|d| d.to_s_f());
    let mut out = json!({
        "entry_type": TYPES[e.entry_type as usize], "base_currency": e.base_currency, "base_amount": e.base_amount.to_s_f(),
        "quote_currency": e.quote_currency, "quote_amount": d(&e.quote_amount), "fee_currency": e.fee_currency, "fee_amount": d(&e.fee_amount),
        "tx_id": e.tx_id, "group_id": e.group_id, "description": e.description,
        "transacted_at": e.transacted_at.map(|t| t.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()), "raw_data": e.raw.text(),
    });
    if let Some(x) = exchange { out["exchange_name_id"] = json!(x); }
    out
}

#[test]
fn the_ported_ruby_is_the_ruby_that_was_recorded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let v = vectors();
    for (file, recorded) in v["ported_sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-record script/rust/record_import_vectors.rb and re-check rust/src/tracker/import");
    }
    let lock = std::fs::read_to_string(root.join("Gemfile.lock")).unwrap();
    for (gem, version) in v["gems"].as_object().unwrap() {
        assert!(lock.contains(&format!("    {gem} ({})\n", version.as_str().unwrap())), "Gemfile.lock moved {gem}: re-record the import vectors");
    }
    let ops: Vec<Value> = binance::OPERATIONS.iter().map(|(op, kind)| json!([op, kind])).collect();
    let recorded: Vec<Value> = v["operations"].as_object().unwrap().iter().map(|(op, kind)| json!([op, kind])).collect();
    assert_eq!(ops, recorded);
    assert_eq!(v["cash"], binance::CASH);
    assert_eq!(v["convert_window"], binance::CONVERT_WINDOW);
    assert_eq!(v["csv_headers"], json!(CSV_HEADERS));
    let types: Vec<Value> = v["entry_types"].as_object().unwrap().iter().map(|(k, id)| json!([k, id])).collect();
    assert_eq!(types, TYPES.iter().enumerate().map(|(i, t)| json!([t, i])).collect::<Vec<_>>());
    assert_eq!(v["tz"], "UTC");
}

#[test]
fn every_recorded_text_offset_and_amount_is_reproduced() {
    let v = vectors();
    let mut failures = vec![];
    let cases = v["as_text"].as_array().unwrap();
    for c in cases {
        let got = csv::as_text(&b64(&c[0]));
        if got != c[1].as_str().unwrap() { failures.push(format!("as_text {}: {got:?} != {}", c[0], c[1])); }
    }
    let offsets = v["offset_from"].as_array().unwrap();
    for c in offsets {
        let got = csv::offset_from(c[0].as_str(), None);
        if json!(got) != c[1] { failures.push(format!("offset_from {}: {got:?} != {}", c[0], c[1])); }
    }
    // Import::Run.amount: DECIMAL, then to_d. The recorded to_d is the value wherever DECIMAL accepts the text.
    let decimal = regex::Regex::new(r"^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?$").unwrap();
    let amounts = v["to_d"].as_array().unwrap();
    for c in amounts {
        let text = c[0].as_str().unwrap();
        let got = csv::amount(Some(text)).map(|d| d.to_s_f()).ok();
        let want = decimal.is_match(deltabadger::ruby::strip(text)).then(|| c[1].as_str().unwrap().to_string());
        if got != want { failures.push(format!("amount {text:?}: {got:?} != {want:?}")); }
    }
    report("primitives", failures, cases.len() + offsets.len() + amounts.len());
}

/// Import::Run's answer for a read: :malformed when the reader raised, :unreadable when it found nothing at all.
fn error(read: &Result<(Vec<Value>, Vec<String>), csv::Malformed>) -> Value {
    match read {
        Err(_) => json!("malformed"),
        Ok((entries, unrecognised)) if entries.is_empty() && unrecognised.is_empty() => json!("unreadable"),
        Ok(_) => Value::Null,
    }
}

#[test]
fn every_recorded_file_reads_to_the_same_entries_in_the_same_order() {
    let v = vectors();
    let mut failures = vec![];
    let mut total = 0;
    for format in ["binance", "deltabadger"] {
        for c in v[format].as_array().unwrap() {
            total += 1;
            let name = c["name"].as_str().unwrap();
            let bytes = b64(&c["bytes"]);
            let text = csv::as_text(&bytes);
            if text != c["text"].as_str().unwrap() { failures.push(format!("{name}: text differs")); continue; }
            let read = if c["format"] == "binance" {
                let offset = csv::offset_from(c["filename"].as_str(), Some(&bytes));
                if json!(offset) != c["offset"] { failures.push(format!("{name}: offset {offset:?} != {}", c["offset"])); continue; }
                let Some(offset) = offset else {
                    if c["error"] != "unknown_timezone" { failures.push(format!("{name}: no zone but {}", c["error"])); }
                    continue;
                };
                binance::read(&text, Some(&offset)).map(|r| (r.entries.iter().map(|e| serialize(e, None)).collect(), r.unrecognised))
            } else {
                own::read(&text).map(|(rows, unrecognised)| (rows.iter().map(|r| serialize(&r.entry, Some(&r.exchange))).collect(), unrecognised))
            };
            let got_error = error(&read);
            if got_error != c["error"] { failures.push(format!("{name}: error {got_error} != {}", c["error"])); continue; }
            let (entries, unrecognised) = read.unwrap_or_default();
            if json!(entries) != c["entries"] {
                failures.push(format!("{name}: entries\n  got  {}\n  want {}", json!(entries), c["entries"]));
            }
            if json!(unrecognised) != c["unrecognised"] { failures.push(format!("{name}: unrecognised {unrecognised:?} != {}", c["unrecognised"])); }
        }
    }
    assert!(total > 100, "the vectors hold every recorded file");
    report("files", failures, total);
}
