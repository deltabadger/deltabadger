//! The sync's pure mappings against vectors recorded from Rails (script/rust/record_sync_vectors.rb). Rails-free.
use deltabadger::codec::format_time;
use deltabadger::crypto::Credentials;
use deltabadger::ruby::BigDec;
use deltabadger::sync::activities::{self, CryptoPair, CryptoPairs, Raw};
use deltabadger::sync::wire::{self, Budget, Node, Refused};
use deltabadger::sync::{number, scrub, SYNC_ERROR_LIMIT};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/sync_vectors.json")).expect("sync_vectors.json parses") }
fn list(v: &Value) -> Vec<Value> { v.as_array().expect("a list").clone() }
fn float(hex: &Value) -> f64 { f64::from_bits(u64::from_str_radix(hex.as_str().unwrap(), 16).unwrap()) }
fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn report(what: &str, failures: Vec<String>, total: usize) {
    assert!(failures.is_empty(), "{} of {total} {what} differ:\n{}", failures.len(), failures[..failures.len().min(10)].join("\n"));
}

#[test]
fn the_ported_ruby_is_the_ruby_that_was_recorded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (file, recorded) in vectors()["ported_sources"].as_object().unwrap() {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-record script/rust/record_sync_vectors.rb and re-check rust/src/sync against it");
    }
    // The JSON vectors are the app's parser, which is Oj (config/initializers/oj.rb, pinned above), not the json gem
    // on its own: the two disagree on a key written twice. Both gems' versions are pinned with the vectors.
    let parser = &vectors()["json_parser"];
    assert_eq!(parser["json_parse_is_oj"], true, "the vectors must be recorded inside the app (bin/rails runner)");
    let lock = std::fs::read_to_string(root.join("Gemfile.lock")).unwrap();
    for (gem, version) in parser["gems"].as_object().unwrap() {
        let pinned = format!("    {gem} ({})\n", version.as_str().unwrap());
        assert!(lock.contains(&pinned), "Gemfile.lock no longer has {}: re-record the vectors and re-check sync::wire against them", pinned.trim());
    }
    assert_eq!(parser["gems"].as_object().unwrap().keys().collect::<Vec<_>>(), ["json", "oj"]);
}

#[test]
fn every_recorded_scrub_and_its_stored_cut_is_reproduced() {
    let cases = list(&vectors()["scrub"]);
    assert_eq!(cases.len(), 52);
    let mut failures = vec![];
    for c in &cases {
        let text = |k: &str| c["key"][k].as_str().map(str::to_string);
        let credentials = Credentials { key: text("key").unwrap_or_default(), secret: text("secret").unwrap_or_default(), passphrase: text("passphrase") };
        let scrubbed = scrub(c["text"].as_str().unwrap(), &credentials);
        let stored: String = scrubbed.chars().take(SYNC_ERROR_LIMIT).collect();
        if scrubbed != c["scrubbed"] || stored != c["stored"] { failures.push(format!("{}\n  rust: {scrubbed}\n  ruby: {}", c["text"], c["scrubbed"])); }
    }
    report("scrubs", failures, cases.len());
}

#[test]
fn every_recorded_split_ratio_and_rationalisation_is_reproduced() {
    let cases = list(&vectors()["split_ratio_label"]);
    assert!(cases.len() > 300);
    let failures: Vec<String> = cases.iter().filter_map(|c| {
        let got = activities::split_ratio_label(&bd(c[0].as_str().unwrap()), &bd(c[1].as_str().unwrap())).expect("a ratio Ruby computed");
        (json!(got) != c[2]).then(|| format!("{} -> {} = {got:?}, Ruby {}", c[0], c[1], c[2]))
    }).collect();
    report("split ratios", failures, cases.len());
    let cases = list(&vectors()["rationalize"]);
    assert_eq!(cases.len(), 300);
    let failures: Vec<String> = cases.iter().filter_map(|c| {
        let (p, q) = activities::rationalize(float(&c[0]), float(&c[1])).expect("a fraction Ruby computed");
        ((p.to_string(), q.to_string()) != (c[2].as_str().unwrap().to_string(), c[3].as_str().unwrap().to_string())).then(|| format!("{c} = {p}/{q}"))
    }).collect();
    report("rationalisations", failures, cases.len());
}

fn pairs() -> CryptoPairs {
    let pair = |base: &str, quote: &str, symbol: Option<&str>| CryptoPair { base: base.into(), quote: quote.into(), asset_symbol: symbol.map(str::to_string), base_asset_id: 1 };
    CryptoPairs([("ETHUSD".to_string(), pair("ETH", "USD", Some("ETH"))), ("XBTUSDT".to_string(), pair("XBT", "USDT", Some("BTC"))),
                 ("NOSYMUSD".to_string(), pair("NOSYM", "USD", None))].into_iter().collect())
}

#[test]
fn every_recorded_activity_becomes_the_entry_rails_makes_of_it() {
    let streams = list(&vectors()["ledger_entries"]);
    let (mut failures, mut total) = (vec![], 0);
    for stream in &streams {
        let normalised: Vec<_> = list(&stream["activities"]).iter().filter_map(|a| activities::normalize(&Raw::from_value(a.clone()), &pairs()).unwrap()).collect();
        let entries = activities::merge_splits(normalised).unwrap();
        let want = list(&stream["entries"]);
        assert_eq!(entries.len(), want.len(), "entries of {}", stream["activities"]);
        for (e, w) in entries.iter().zip(&want) {
            total += 1;
            let got = json!({
                "entry_type": e.entry_type, "base_currency": e.base_currency, "base_amount": e.base_amount.to_s_f(), "quote_currency": e.quote_currency,
                "quote_amount": e.quote_amount.as_ref().map(BigDec::to_s_f), "fee_currency": e.fee_currency, "fee_amount": e.fee_amount.as_ref().map(BigDec::to_s_f),
                "tx_id": e.tx_id, "group_id": e.group_id, "description": e.description, "transacted_at": e.transacted_at.map(format_time), "raw": e.raw.value, "raw_text": e.raw.text(),
            });
            if &got != w { failures.push(format!("rust: {got}\n  ruby: {w}")); }
        }
    }
    assert_eq!(total, 70, "entries compared");
    report("entries", failures, total);
}

#[test]
fn activity_times_are_read_as_rails_reads_them() {
    let times = &vectors()["times"];
    for c in list(&times["trade"]) {
        let fill = json!({ "id": "f", "activity_type": "FILL", "symbol": "AAPL", "side": "buy", "qty": "1", "price": "1", "transaction_time": c[0] });
        assert_eq!(json!(activities::normalize(&Raw::from_value(fill), &pairs()).unwrap().unwrap().transacted_at.map(format_time)), c[1], "{c}");
    }
    for c in list(&times["non_trade"]) {
        let key = if c[0].as_str().unwrap().len() == 10 { "date" } else { "transaction_time" };
        let interest = json!({ "id": "i", "activity_type": "INT", "net_amount": "1", key: c[0] });
        assert_eq!(json!(activities::normalize(&Raw::from_value(interest), &pairs()).unwrap().unwrap().transacted_at.map(format_time)), c[1], "{c}");
    }
    // What Ruby reads and this port refuses (the sync fails, as Ruby's raise fails the job): a time in no known shape.
    let odd = json!({ "id": "i", "activity_type": "INT", "net_amount": "1", "date": "19 Mar 2026" });
    assert_eq!(activities::normalize(&Raw::from_value(odd), &pairs()), Err("unreadable activity time".to_string()), "and the error does not repeat the value");
}

/// Every number of an answer is bounded before any arithmetic (`sync::number`): one value per way of being hostile.
/// None of these may reach `ruby::BigDec`, which expands a number digit by digit, on a thread a dropped job cannot stop.
#[test]
fn every_hostile_number_is_refused_before_any_arithmetic_and_every_plain_one_is_read() {
    let started = std::time::Instant::now();
    let digits = |n: usize| "7".repeat(n);
    let refused: Vec<String> = [
        r#""1e400""#, r#""-1e-400""#, r#""1e41""#, r#""1e-41""#, r#""1e999999999""#, r#""1e+0000041""#, r#""NaN""#, r#""Infinity""#, r#""-Infinity""#,
        r#""0x10""#, r#""1_000""#, r#""12abc""#, r#""--1""#, r#""+-1""#, r#"".""#, r#""e5""#, r#""1e""#, r#""1.2.3""#, r#""१२""#,
        "1e41", "1e-41", "-1e300", "1e-999", "-1e-999", "1E-400", "0.1e-40", "true", "false", "[1]", r#"{"qty":1}"#, "",
    ].iter().map(|s| s.to_string()).chain([
        format!("\"{}\"", digits(65)), format!("\"{}\"", digits(300)), format!("\"0.{}\"", digits(65)), digits(70), format!("{}.5", digits(300)),
        format!("\"1e{}\"", digits(250)), format!("\"{}\"", "0".repeat(257)),
    ]).collect();
    for token in &refused {
        let error = number::json(token).expect_err(token);
        assert!(error.len() < 60 && !error.contains("777"), "{token}: the error names the reason, never the value: {error}");
    }
    let read = |token: &str| number::json(token).unwrap_or_else(|e| panic!("{token}: {e}")).map(|d| d.to_s_f());
    assert_eq!((read("null"), read(r#""""#), read(r#""  ""#)), (None, Some("0.0".into()), Some("0.0".into())), "nil&.to_d, and a blank String#to_d");
    assert_eq!((read("12"), read("-0.5"), read(r#""0.359712230""#), read(r#"" 5 ""#), read(r#""+.5""#)), (Some("12.0".into()), Some("-0.5".into()), Some("0.35971223".into()), Some("5.0".into()), Some("0.5".into())));
    assert_eq!((read(r#""1e40""#), read(r#""1e-40""#), read("1e40"), read("0.0")), (Some(format!("1{}.0", "0".repeat(40))), Some(format!("0.{}1", "0".repeat(39))), Some(format!("1{}.0", "0".repeat(40))), Some("0.0".into())));
    assert_eq!(read(&format!("\"7.{}\"", digits(63))), Some(format!("7.{}", digits(63))), "64 significant digits are read whole");
    assert_eq!(read(&format!("\"{}7\"", "0".repeat(200))), Some("7.0".into()), "leading zeros are not digits of the value");
    assert_eq!(read("18446744073709551617"), Some("18446744073709551617.0".into()), "a JSON integer is exact, whatever a double holds");
    assert_eq!(read("0.1"), Some("0.1".into()), "any other JSON number is a Float (Float#to_d)");
    // A magnitude is read off the token, before any Float: an underflow is an error, never the zero a double makes of
    // it; a zero written with any exponent is zero.
    assert_eq!((number::json("1e-999"), read("0e-999"), read("0.0e-999"), read("1e-40")), (Err("beyond 10^±40".to_string()), Some("0.0".into()), Some("0.0".into()), Some(format!("0.{}1", "0".repeat(39)))));

    // The same caps at the activity: the sync fails, and the error names the field.
    let fill = |qty: &str| Raw::parse(&format!(r#"{{"id":"f","activity_type":"FILL","symbol":"AAPL","side":"buy","qty":{qty},"price":"1","transaction_time":"2026-09-10T14:30:00Z"}}"#)).unwrap().unwrap();
    assert_eq!(activities::normalize(&fill(r#""1e400""#), &pairs()), Err("unreadable qty: beyond 10^±40".to_string()));
    assert_eq!(activities::normalize(&fill(&digits(70)), &pairs()), Err("unreadable qty: more than 64 significant digits".to_string()));
    assert_eq!(activities::normalize(&fill("1e-999"), &pairs()), Err("unreadable qty: beyond 10^±40".to_string()), "an unquoted underflow is not a quantity of zero");
    assert!(activities::normalize(&fill(r#""1""#), &pairs()).is_ok());
    // A value read back from the database has the database caps.
    use rusqlite::types::ValueRef;
    assert!(number::stored(ValueRef::Text(digits(300).as_bytes())).is_err() && number::stored(ValueRef::Text(b"1e401")).is_err() && number::stored(ValueRef::Real(f64::NAN)).is_err());
    assert_eq!(number::stored(ValueRef::Text(b"1e400")).unwrap().map(|d| d.precision()), Some(401));
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "every refusal is immediate: {:?}", started.elapsed());
}

/// A split ratio is a fraction of two integers this port can hold, reached in a bounded number of steps, or an error:
/// no double makes `rationalize` loop (Ruby raises FloatDomainError on a ratio that is not finite, NaN included; this
/// port's walk, written without that check, would not end on NaN).
#[test]
fn no_split_quantity_makes_the_ratio_loop_overflow_or_lie() {
    let started = std::time::Instant::now();
    for (f, eps) in [(f64::NAN, 0.1), (f64::INFINITY, 0.1), (f64::NEG_INFINITY, 0.1), (1.5, f64::NAN), (1.5, f64::INFINITY), (1e300, 1e297), (1e31, 1e28),
                     (5e-324, 0.0), (f64::MAX, 0.0), (f64::MIN_POSITIVE, 0.0), (0.0, 0.0)] {
        assert_eq!(activities::rationalize(f, eps), None, "{f:e} within {eps:e}");
    }
    // A walk is over long before the cap, and the cap is there: the golden ratio is the slowest fraction there is.
    assert_eq!(activities::rationalize(1.5, 0.0015), Some((3, 2)));
    assert_eq!(activities::rationalize(1.0, 0.0), Some((1, 1)));
    assert_eq!(activities::rationalize(-1.0, 0.001), Some((-1, 1)));
    assert!(activities::rationalize((1.0 + 5f64.sqrt()) / 2.0, 1e-300).is_some_and(|(p, q)| p > 0 && q > 0));
    assert_eq!(activities::RATIONALIZE_STEPS, 64);
    // The label: both counts positive and different, or none; a factor past this port's integers is an error.
    let label = |old: &str, new: &str| activities::split_ratio_label(&bd(old), &bd(new));
    assert_eq!((label("1", "10"), label("3", "1"), label("0", "5"), label("5", "5"), label("-1", "5")), (Ok(Some("10:1".into())), Ok(Some("1:3".into())), Ok(None), Ok(None), Ok(None)));
    assert!(label("1", "1e40").is_err() && label("1e40", "1").is_err() && label("1e-40", "1e40").is_err());
    // Two legs that are each within the caps and whose ratio is not: the group, and so the sync, fails.
    let leg = |id: &str, qty: &str| activities::normalize(&Raw::from_value(json!({ "id": id, "activity_type": "SPLIT", "symbol": "KLAC", "qty": qty, "date": "2026-09-15" })), &pairs()).unwrap().unwrap();
    assert_eq!(activities::merge_splits(vec![leg("a", "-1"), leg("b", "1e40")]), Err("a split whose ratio is not a usable number".to_string()));
    assert!(activities::merge_splits(vec![leg("a", "-1"), leg("b", "10")]).is_ok());
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

/// An answer is held as the app's parser holds it (`wire`): the vectors are `JSON.parse` inside the app (Oj),
/// re-generated. A key met twice keeps its last value in its first place, at every level; more than 100 levels are
/// refused; what Ruby's parser refuses is refused.
#[test]
fn an_answer_is_held_as_rubys_parser_holds_it() {
    // The value, with every number as a number (Ruby prints a Float its own way, and -0.0 as 0.0), and the keys in order.
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => for (k, v) in m { out.push(k.clone()); keys(v, out); },
            Value::Array(a) => for v in a { out.push("[".into()); keys(v, out); },
            _ => {}
        }
    }
    let exact = |text: &str| wire::exact(text).map(|v| { let mut order = vec![]; keys(&v, &mut order); (v, order) });
    let cases = list(&vectors()["json_canonical"]);
    assert_eq!(cases.len(), 34);
    // One text Ruby's parser lets through and serde_json does not: an escape that is none. (So is half a surrogate
    // pair, which Ruby turns into bytes that are not UTF-8.)
    let stricter = [r#"{"a":"\x"}"#];
    assert_eq!(wire::read(r#"["\ud800"]"#, &mut Budget(usize::MAX), None), Err(Refused::NotJson));
    let (mut nesting, mut refused, mut held) = (0, 0, 0);
    for c in &cases {
        let text = c[0].as_str().unwrap();
        let read = wire::read(text, &mut Budget(usize::MAX), None);
        match (c[1].as_str(), c[2].as_str()) {
            (None, Some("nesting")) => { nesting += 1; assert_eq!(read, Err(Refused::TooDeep), "{text}"); }
            (None, Some(_)) => { refused += 1; assert_eq!(read, Err(Refused::NotJson), "{text}"); }
            (Some(_), _) if stricter.contains(&text) => assert_eq!(read, Err(Refused::NotJson), "{text}"),
            (Some(ruby), _) => { held += 1; assert_eq!(exact(&read.unwrap_or_else(|e| panic!("{text}: {e:?}")).text()).unwrap(), exact(ruby).unwrap(), "{text}"); }
            other => panic!("{text}: {other:?}"),
        }
    }
    assert_eq!((nesting, refused, held), (3, 12, 18));
    // The text itself: scalars are the venue's, to the letter.
    let node = wire::read(r#" { "n" : 18446744073709551617 , "f":1.0000000000000000001, "n": [ 1e2 , "a\u0062" ] } "#, &mut Budget(usize::MAX), None).unwrap();
    assert_eq!(node.text(), r#"{"n":[1e2,"a\u0062"],"f":1.0000000000000000001}"#);
}

/// What an answer may cost is settled while it is read, before a tree of it exists: a body inside its byte limit can
/// hold a million values or keys.
#[test]
fn an_answer_is_refused_at_its_first_value_over_the_budget() {
    let started = std::time::Instant::now();
    let many_scalars = format!("[{}]", vec!["[]"; 1_300_000].join(","));          // 3.9 MB of empty lists
    let many_keys = format!("{{{}}}", (0..300_000).map(|i| format!("\"k{i}\":0")).collect::<Vec<_>>().join(","));
    let one_wide_item = format!("[{{\"id\":\"a\",\"wide\":[{}]}}]", vec!["0"; 100_000].join(","));
    assert!(many_scalars.len() < 4 * 1024 * 1024 && many_keys.len() < 4 * 1024 * 1024 && one_wide_item.len() < 256 * 1024);
    assert_eq!(wire::read(&many_scalars, &mut Budget(300_000), None), Err(Refused::OverBudget));
    assert_eq!(wire::read(&many_scalars, &mut Budget(usize::MAX), Some(5_000)), Err(Refused::TooManyItems), "a list stops at its first item over the limit");
    assert_eq!(wire::read(&many_keys, &mut Budget(300_000), None), Err(Refused::OverBudget), "a key is counted, and so is its value");
    assert_eq!(wire::read(&one_wide_item, &mut Budget(20_000), Some(100)), Err(Refused::OverBudget), "one item of a short page can still be too much");
    // The budget is one count across answers: what a page spends, the run has no more.
    let mut run = Budget(10);
    assert!(matches!(wire::read(r#"[{"a":1},{"b":[2,3]}]"#, &mut run, Some(100)), Ok(Node::Array(items)) if items.len() == 2));
    assert_eq!(run, Budget(1), "one for the list, two objects, two keys, a scalar, a list and its two scalars");
    assert_eq!(wire::read("[1,2]", &mut run, None), Err(Refused::OverBudget));
    assert_eq!((Refused::OverBudget.text("an activities page"), Refused::TooManyItems.text("positions")),
               ("an activities page with more values than one answer may hold".to_string(), "positions with more items than were asked for".to_string()));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

/// What is stored as raw_data is what Rails stores, byte for byte: the app's JSON column over what the app's parser
/// held, both Oj's. A Float is re-printed in Oj's sixteen digits (`0.30000000000000004` is stored as `0.3`), an Integer
/// is kept exactly, a string is written with Rails' escapes. The vectors are `ActiveRecord::Type::Json#serialize` of
/// `JSON.parse(text)`, recorded inside the app.
#[test]
fn raw_data_is_written_as_the_apps_json_column_writes_it() {
    let cases = list(&vectors()["json_stored"]);
    let mut failures = vec![];
    for c in &cases {
        let text = c[0].as_str().unwrap();
        let stored = wire::read(text, &mut Budget(usize::MAX), None).unwrap_or_else(|e| panic!("{text}: {e:?}")).stored();
        if json!(stored) != c[1] && !(c[1].is_null() && stored == "null") { failures.push(format!("{text}: rust {stored}, rails {}", c[1])); }
    }
    assert_eq!(cases.len(), 802);
    report("stored JSON texts", failures, cases.len());
    // Rails prints a value twice (the assignment casts it through its own text, the save serialises that). One vector
    // shows it: the largest double, whose sixteen digits read back as Infinity.
    let twice: Vec<&Value> = cases.iter().filter(|c| !c[2].is_null()).collect();
    assert_eq!(twice, [&json!(["[1.7976931348623157e308]", "[null]", "[1.797693134862316e+308]"])]);
    // One printing, in the open: zero of either sign, a whole number, sixteen digits, C's exponent, and Float#to_s
    // where the sixteen digits end in 0001 or 9999.
    let floats = [(0.30000000000000004, "0.3"), (1.2345678901234567, "1.234567890123457"), (-0.0, "0.0"), (100.0, "100.0"), (1e15, "1000000000000000.0"), (1e20, "1e+20"),
                  (1e-5, "1e-05"), (1e-7, "1e-07"), (0.0001, "0.0001"), (1234567890123456.7, "1234567890123457"), (5e-324, "4.940656458412465e-324"),
                  (1984.0207455399993, "1984.0207455399993"), (9671.23262804, "9671.23262804"), (-1.5e-10, "-1.5e-10"), (f64::MAX, "1.797693134862316e+308"),
                  (f64::INFINITY, "null"), (f64::NAN, "null"), (-9_223_372_036_854_775_808.0, "-9223372036854775808.0"), (9_223_372_036_854_774_784.0, "9223372036854774784.0")];
    for (float, text) in floats { assert_eq!(wire::stored_float(float), text, "{float:e}"); }
    // 2^63 as a Float is the one value Oj prints differently by processor (a C cast that is undefined): x86-64's here.
    assert_eq!(wire::stored_float(9_223_372_036_854_775_808.0), "9.223372036854776e+18");

    // An activity: a number is read from the venue's own text, and stored as Rails stores it.
    let text = r#"{"id":"a","reference":18446744073709551617,"long_fraction":1.0000000000000000001,"nested":{"ids":[-9223372036854775809, 1e2],"x":1,"x":{"y":1,"y":2}},"qty":"5","tiny":1e-999,"f":0.30000000000000004,"id":"b"}"#;
    let mut raw = Raw::parse(text).unwrap().unwrap();
    assert_eq!(raw.value["id"], "b", "a repeated key keeps its last value");
    assert_eq!(raw.text(), r#"{"id":"b","reference":18446744073709551617,"long_fraction":1.0,"nested":{"ids":[-9223372036854775809,100.0],"x":{"y":2}},"qty":"5","tiny":0.0,"f":0.3}"#,
               "and its first place, at every level; integers exact, Floats as Oj prints them");
    assert_eq!(raw.number("tiny"), Err("unreadable tiny: beyond 10^±40".to_string()), "the size of a number is judged on what the venue wrote, not on the Float");
    assert_eq!(raw.number("f").unwrap().map(|d| d.to_s_f()), Some("0.3".into()), "Float#to_d, as before");
    raw.set("corporate_action", json!("split"));
    raw.set("qty", json!("6"));
    raw.set_node("merged_activity_ids", Node::Array(vec![raw.member("id").cloned().unwrap(), Node::Scalar("null".into())]));
    assert!(raw.text().ends_with(r#""qty":"6","tiny":0.0,"f":0.3,"corporate_action":"split","merged_activity_ids":["b",null]}"#), "{}", raw.text());
    assert_eq!(raw.value["merged_activity_ids"], json!(["b", null]));
    assert_eq!(raw.number("reference").unwrap().map(|d| d.to_s_f()), Some("18446744073709551617.0".into()));
    assert_eq!((Raw::parse("[1]").map(|r| r.is_err()), Raw::parse("{").is_none()), (Some(true), true), "not an object; not JSON");
    assert_eq!(Raw::from_value(json!({ "a": 1, "b": "x<y", "c": 2.5, "d": { "e": [1e-7] } })).text(), r#"{"a":1,"b":"x\u003cy","c":2.5,"d":{"e":[1e-07]}}"#);
    assert_eq!(Raw::parse(r#"{"k\"ey":1}"#).unwrap().unwrap().text(), r#"{"k\"ey":1}"#, "a key is written as JSON");
}

/// Codex round 4, finding 4: a value that is not JSON is refused where it stands, also where a later value of the same
/// key would overwrite it, at any depth. The app's parser refuses every one of these bodies.
#[test]
fn a_value_that_is_not_json_is_refused_even_where_a_later_one_overwrites_it() {
    let cases = list(&vectors()["json_overwritten"]);
    assert_eq!(cases.len(), 63);
    for c in &cases {
        let text = c[0].as_str().unwrap();
        assert_eq!(c[2], "parser", "{text}: the app's parser refuses it");
        assert_eq!(wire::read(text, &mut Budget(usize::MAX), None), Err(Refused::NotJson), "{text}");
    }
    // What is overwritten is gone only when it was JSON.
    assert_eq!(wire::read(r#"{"qty":"wat","qty":"10"}"#, &mut Budget(usize::MAX), None).map(|n| n.text()), Ok(r#"{"qty":"10"}"#.to_string()));
    assert_eq!(wire::read(r#"{"a":{"b":[1,{"c":[true, null],"c":1}]}}"#, &mut Budget(usize::MAX), None).map(|n| n.text()), Ok(r#"{"a":{"b":[1,{"c":1}]}}"#.to_string()));
    // Stricter than the app's parser, in an overwritten position as anywhere (a listed divergence): Oj lets these
    // through; here the body is not JSON and the sync fails.
    for lenient in [r#"{"a":"\x","a":1}"#, r#"{"a":-,"a":1}"#, r#"{"a":00,"a":1}"#, r#"{"a":1e400,"a":1}"#, "{\"a\":\"tab\tin\",\"a\":1}"] {
        assert_eq!(wire::read(lenient, &mut Budget(usize::MAX), None), Err(Refused::NotJson), "{lenient}");
    }
}

#[test]
fn the_copied_constants_are_rails_constants() {
    let c = &vectors()["constants"];
    assert_eq!(json!(activities::SPLIT_TYPES), c["split_types"]);
    for t in list(&c["cash_activity_types"]) { assert!(activities::cash_activity(t.as_str().unwrap()), "{t}"); }
    for t in ["FILL", "CFEE", "DIVROC", "SPLIT", "SSP", "MA"] { assert!(!activities::cash_activity(t), "{t}"); }
    let types = &c["entry_types"];
    assert_eq!((types["buy"].as_i64(), types["sell"].as_i64(), types["deposit"].as_i64(), types["withdrawal"].as_i64(), types["fee"].as_i64()),
               (Some(activities::BUY), Some(activities::SELL), Some(activities::DEPOSIT), Some(activities::WITHDRAWAL), Some(activities::FEE)));
    assert_eq!((types["other_income"].as_i64(), types["withholding_tax"].as_i64(), types["return_of_capital"].as_i64(), types["adjustment"].as_i64(), types["unsupported_activity"].as_i64()),
               (Some(activities::OTHER_INCOME), Some(activities::WITHHOLDING_TAX), Some(activities::RETURN_OF_CAPITAL), Some(activities::ADJUSTMENT), Some(activities::UNSUPPORTED_ACTIVITY)));
    assert_eq!(c["sync_error_limit"], json!(SYNC_ERROR_LIMIT));
    // Alpaca's one condemning text, and no permission error (so ApiKey#missing_permission? never holds for it).
    assert_eq!((&c["invalid_key_errors"], &c["permission_errors"]), (&json!(["unauthorized"]), &json!([])));
    // The schedule the jobs declare (src/sync/jobs.rs) is Rails' recurring.yml.
    assert_eq!(c["recurring"], json!({ "sync_all_account_transactions_job": { "class": "AccountTransaction::SyncAllJob", "schedule": "0 2 * * *" },
                                       "sync_all_account_balances_job": { "class": "AccountBalance::SyncAllJob", "schedule": "30 2 * * *" } }));
}

/// The transport stops reading a body at the caller's limit: an answer of any size holds at most that much memory.
#[tokio::test(flavor = "current_thread")]
async fn a_response_body_over_the_limit_is_refused_while_it_is_read() {
    use deltabadger::venue::http::{client, HttpRequest, ReqwestTransport, Transport, TransportError, MAX_BODY};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let body = format!("[{}]", vec![r#"{"id":"0123456789"}"#; 20_000].join(","));
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_string(body.clone())).mount(&server).await;
    let request = HttpRequest { method: "GET", base: server.uri(), path: "/v2/account/activities".into(), query: vec![], body: None, not_after: None };
    let transport = ReqwestTransport::new(client(), "PKTEST".into(), "s3cret".into());
    assert!(body.len() > 256 * 1024 && body.len() < MAX_BODY);
    assert_eq!(transport.send_limited(&request, 256 * 1024).await, Err(TransportError::MaybeSent("the response body is over 262144 bytes".into())));
    assert_eq!(transport.send_limited(&request, body.len()).await.map(|r| r.body.len()), Ok(body.len()), "a body of exactly the limit is read");
    assert_eq!(transport.send(&request).await.map(|r| r.body.len()), Ok(body.len()), "every other caller has the generous default");
}
// ---- the ledger's pure parts (Task 4) ----

#[test]
fn the_after_parameter_and_the_ledgers_constants_are_rails() {
    use deltabadger::sync::ledger;
    for c in list(&vectors()["times"]["after"]) {
        assert_eq!(json!(ledger::after(deltabadger::codec::parse_time(c[0].as_str().unwrap()).unwrap())), c[1], "{c}");
    }
    let c = &vectors()["constants"];
    assert_eq!(json!(ledger::CRYPTO_COINGECKO_IDS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>()), c["crypto_coingecko_ids"]);
    assert_eq!(json!(ledger::CRYPTO_QUOTES), c["crypto_quotes"]);
    assert_eq!(json!(ledger::FIAT_CURRENCIES), c["fiat_currencies"]);
    assert_eq!((c["asset_catch_up_days"].as_i64(), c["feed_retention_days"].as_i64(), c["transfer_window_hours"].as_i64()),
               (Some(ledger::ASSET_CATCH_UP_DAYS), Some(ledger::FEED_RETENTION_DAYS), Some(ledger::TRANSFER_WINDOW_HOURS)));
    assert_eq!((&c["transfer_tolerance"], &c["tombstone_prefix"]), (&json!("0.02"), &json!("__stale_")));
    assert_eq!((ledger::MAX_PAGE_BYTES, ledger::MAX_PAGES, ledger::MAX_ACTIVITIES, ledger::BATCH), (262_144, 500, 50_000, 100), "the limits the plan states");
    assert_eq!((ledger::MAX_PAGE_NODES, ledger::MAX_RUN_NODES, ledger::Limits::RUN.pages, wire::MAX_NESTING), (20_000, 3_000_000, 500, 100));
}

/// The harness compares JSON without rounding an integer (`parity::exact`): one no 64-bit type holds becomes a marker.
#[test]
fn the_harness_reads_json_without_losing_an_integer() {
    use deltabadger::sync::parity::exact;
    let read = exact(r#"{"a":[18446744073709551617, 18446744073709551615, -9223372036854775809, -9223372036854775808, 1.5, 1e400, "18446744073709551617 \" 99999999999999999999"],"b":123456789012345678901234567890}"#);
    assert!(read.is_err(), "1e400 is no JSON number serde reads");
    let read = exact(r#"{"a":[18446744073709551617, 18446744073709551615, -9223372036854775809, -9223372036854775808, 1.5, "18446744073709551617 \" 99999999999999999999"],"b":123456789012345678901234567890}"#).unwrap();
    assert_eq!(read, json!({ "a": ["<integer 18446744073709551617>", 18446744073709551615u64, "<integer -9223372036854775809>", i64::MIN, 1.5, "18446744073709551617 \" 99999999999999999999"],
                             "b": "<integer 123456789012345678901234567890>" }));
    assert_ne!(exact("[18446744073709551617]").unwrap(), exact("[18446744073709551616]").unwrap(), "which plain parsing reads as the same double");
    assert_eq!(serde_json::from_str::<Value>("[18446744073709551617]").unwrap(), serde_json::from_str::<Value>("[18446744073709551616]").unwrap());
}
