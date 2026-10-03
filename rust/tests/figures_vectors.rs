//! Every pure rule of rust/src/figures against what Ruby recorded (script/rust/record_figures_vectors.rb).
use deltabadger::figures::at::At;
use deltabadger::figures::dec::Dec;
use deltabadger::figures::json::{self, J};
use deltabadger::figures::num::{float_to_s, oj_float, Num, NumError};
use serde_json::Value;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/figures_vectors.json")).expect("figures_vectors.json parses") }
fn float(v: &Value) -> f64 { f64::from_bits(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap()) }
fn num(v: &Value) -> Num {
    if let Some(i) = v.get("i") { return Num::Int(i.as_i64().unwrap()); }
    if let Some(d) = v.get("d") { return Num::Dec(Dec::parse(d.as_str().unwrap()).unwrap()); }
    Num::Float(float(&v["f"]))
}
/// A Num as the recorder tags one, so a kind that differs is a difference.
fn tagged(n: &Num) -> Value {
    match n {
        Num::Int(i) => serde_json::json!({ "i": i }),
        Num::Dec(d) => serde_json::json!({ "d": d.to_s_f() }),
        Num::Float(f) => serde_json::json!({ "f": format!("{:016x}", f.to_bits()) }),
    }
}

#[test]
fn a_float_prints_as_ruby_prints_it_and_as_rails_json_does() {
    let v = vectors();
    let (to_s, oj) = (v["float_to_s"].as_array().unwrap(), v["oj_float"].as_array().unwrap());
    assert!(to_s.len() > 1_200 && oj.len() > 1_200);
    let wrong: Vec<String> = to_s.iter().filter(|c| float_to_s(float(&c[0])) != c[1].as_str().unwrap()).map(|c| format!("to_s {c} gave {}", float_to_s(float(&c[0]))))
        .chain(oj.iter().filter(|c| oj_float(float(&c[0])) != c[1].as_str().unwrap()).map(|c| format!("json {c} gave {}", oj_float(float(&c[0])))))
        .collect();
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong[..wrong.len().min(10)].join("\n"));
    // The fallback is exercised, and so is the loss: sixteen digits are not always the double Rails computed.
    assert!(oj.iter().any(|c| c[1].as_str().unwrap().len() > 17 && float_to_s(float(&c[0])) == c[1].as_str().unwrap()));
    assert!(oj.iter().any(|c| c[1].as_str().unwrap().parse::<f64>().unwrap() != float(&c[0])), "no vector shows the sixteen-digit loss");
}

#[test]
fn every_recorded_operation_keeps_rubys_value_and_kind() {
    let v = vectors();
    let cases = v["num"].as_array().unwrap();
    assert_eq!(cases.len(), 11_200);
    let (mut wrong, mut negative_zeros) = (vec![], 0);
    for c in cases {
        let (a, b) = (num(&c[1]), num(&c[2]));
        let got = match c[0].as_str().unwrap() {
            "+" => a.add(&b), "-" => a.sub(&b), "*" => a.mul(&b), "/" => a.div(&b),
            "<=>" => a.compare(&b).map(|o| Num::Int(o as i64)),
            "min" => Num::min2(a.clone(), b.clone()), "max" => Num::max2(a.clone(), b.clone()),
            other => panic!("unknown op {other}"),
        };
        let got = match &got { Ok(n) => tagged(n), Err(_) => serde_json::json!({ "raise": "not a number" }) };
        // Ruby's BigDecimal has a negative zero (0 x -1 is -0.0) and Rails prints it: the sign is part of the answer.
        if c[3]["d"] == "-0.0" { negative_zeros += 1; }
        if got != c[3] { wrong.push(format!("{c} gave {got}")); }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong[..wrong.len().min(15)].join("\n"));
    assert!(negative_zeros > 100, "only {negative_zeros} answers are the negative zero");
}

#[test]
fn predicates_conversions_and_rounding_match_ruby() {
    let v = vectors();
    for c in v["predicates"].as_array().unwrap() {
        let n = num(&c[0]);
        assert_eq!((n.is_zero(), n.is_positive(), n.is_negative()), (c[1].as_bool().unwrap(), c[2].as_bool().unwrap(), c[3].as_bool().unwrap()), "{c}");
        assert_eq!(tagged(&Num::Dec(n.to_d().unwrap())), c[4], "to_d {c}");
        assert_eq!(n.to_f().to_bits(), float(&c[5]).to_bits(), "to_f {c}");
    }
    let rounds = v["round"].as_array().unwrap();
    assert!(rounds.len() >= 80);
    for c in rounds { assert_eq!(tagged(&num(&c[0]).round(c[1].as_i64().unwrap()).unwrap()), c[2], "{c}"); }
}

fn j(v: &Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => n.as_i64().map(J::Int).unwrap_or_else(|| J::Float(n.as_f64().unwrap())),
        Value::String(s) => J::Str(s.clone()),
        Value::Array(a) => J::Arr(a.iter().map(j).collect()),
        Value::Object(o) => J::Obj(o.iter().map(|(k, v)| (k.clone(), j(v))).collect()),
    }
}

#[test]
fn json_text_and_times_are_written_as_rails_writes_them() {
    let v = vectors();
    for c in v["json"].as_array().unwrap() { assert_eq!(j(&c[0]).write(), c[1].as_str().unwrap()); }
    let times = v["times"].as_array().unwrap();
    assert_eq!(times.len(), 64);
    // A zone at no offset that is not UTC, and UTC itself.
    assert!(times.iter().any(|c| c[1] == "London" && c[3].as_str().unwrap().ends_with("+00:00\"")) && times.iter().any(|c| c[1] == "London" && c[3].as_str().unwrap().ends_with("+01:00\"")));
    assert!(times.iter().any(|c| c[1] == "UTC" && c[3].as_str().unwrap().ends_with("Z\"")));
    for c in times {
        let at = At(c[0].as_i64().unwrap()).utc();
        let zone = deltabadger::web::timezone::zone(c[1].as_str().unwrap()).unwrap();
        let text = json::time(&at.with_timezone(&zone), c[2].as_u64().unwrap() as usize);
        assert_eq!(J::Str(text).write(), c[3].as_str().unwrap(), "{c}");
    }
    // A text is read as a number only where it is a clean decimal, and then as String#to_d reads it. What Ruby
    // reads off the front of anything else ("12abc" is 12), and its Infinity and NaN, are no figure here.
    let strings = v["to_d"].as_array().unwrap();
    assert_eq!(strings.len(), 49);
    let (mut read, mut refused) = (0, vec![]);
    for c in strings {
        match Dec::strict(c[0].as_str().unwrap()) {
            Ok(d) => { assert_eq!(d.to_s_f(), c[1].as_str().unwrap(), "{:?}.to_d", c[0]); read += 1; }
            Err(error) => { assert_eq!(error, NumError::NotANumber, "{:?}", c[0]); refused.push(c[0].as_str().unwrap()); }
        }
    }
    assert_eq!((read, refused.len()), (15, 34), "refused: {refused:?}");
    assert!(["NaN", "Infinity", "-Infinity", "12abc", "abc", "", " 12 ", "1_000.5", ".5", "5."].iter().all(|text| refused.contains(text)));
    for c in v["time_minus"].as_array().unwrap() {
        assert_eq!(At(c[0].as_i64().unwrap()).minus(At(c[1].as_i64().unwrap())).to_bits(), float(&c[2]).to_bits(), "{c}");
    }
}
