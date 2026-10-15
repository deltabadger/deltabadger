mod common;
use chrono::{DateTime, Utc};
use deltabadger::ruby::*;

fn micros(t: &str) -> i64 { t.parse::<DateTime<Utc>>().unwrap().timestamp_micros() }
fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

#[test]
fn every_recorded_bigdecimal_operation_is_reproduced() {
    let cases = common::vectors()["bigdec"].as_array().unwrap().clone();
    assert!(cases.len() > 5_000);
    let mut failures = vec![];
    for c in &cases {
        let (op, a, b, want) = (c[0].as_str().unwrap(), c[1].as_str().unwrap(), c[2].as_str().unwrap(), c[3].as_str().unwrap());
        let got = match op {
            "div" => bd(a).div(&bd(b)).unwrap().to_s_f(),
            "mul" => (&bd(a) * &bd(b)).to_s_f(),
            "add" => (&bd(a) + &bd(b)).to_s_f(),
            "sub" => (&bd(a) - &bd(b)).to_s_f(),
            "to_f" => format!("{:016x}", bd(a).to_f().to_bits()),
            "precision" => bd(a).precision().to_string(),
            "floor" => bd(a).floor(b.parse().unwrap()).to_s_f(),
            "ceil" => bd(a).ceil(b.parse().unwrap()).to_s_f(),
            "round18" => bd(a).round(18).to_s_f(),
            other => panic!("unknown op {other}"),
        };
        if got != want { failures.push(format!("{op}({a}, {b}) = {got}, Ruby {want}")); }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures[..failures.len().min(10)].join("\n"));
}

#[test]
fn float_to_d_matches_the_codec_rule() {
    for pair in common::vectors()["decimals"].as_array().unwrap() {
        let f = f64::from_bits(u64::from_str_radix(pair[0].as_str().unwrap(), 16).unwrap());
        assert_eq!(BigDec::from_f64(f).unwrap(), bd(pair[1].as_str().unwrap()), "{pair}");
    }
}

#[test]
fn zero_divisors_are_refused_not_infinite() {
    assert_eq!(bd("60").div(&BigDec::zero()), None);
    assert_eq!(BigDec::zero().div(&bd("3")), Some(BigDec::zero()));
}

#[test]
fn columns_read_and_write_as_rails_does() {
    use rusqlite::types::ValueRef;
    assert_eq!(from_sql(ValueRef::Integer(60)).unwrap(), Some(bd("60")));
    assert_eq!(from_sql(ValueRef::Real(30.0003)).unwrap(), Some(bd("30.0003")));
    assert_eq!(from_sql(ValueRef::Text(b"0.0012")).unwrap(), Some(bd("0.0012")));
    assert_eq!(from_sql(ValueRef::Null).unwrap(), None);
    // Rails' SQLite adapter binds BigDecimal#to_f after round(18) — not the decimal text.
    assert_eq!(to_sql(&bd("58054.794626705257545240")).to_bits(), 0x40ec58d96d94fbf4);
}

#[test]
fn iso8601_ms_and_round6_match_ruby_time() {
    let r = &common::vectors()["ruby"];
    for p in r["iso8601_ms"].as_array().unwrap() {
        assert_eq!(iso8601_ms(p[0].as_str().unwrap().parse().unwrap()), p[1].as_str().unwrap());
    }
    for p in r["round6"].as_array().unwrap() {
        let off = f64::from_bits(u64::from_str_radix(p[1].as_str().unwrap(), 16).unwrap());
        assert_eq!(round6_micros(micros(p[0].as_str().unwrap()), &[(off, 1)]), micros(p[2].as_str().unwrap()), "{p}");
    }
}

#[test]
fn to_sentence_and_inspect_match_ruby() {
    let r = &common::vectors()["ruby"];
    let strs = |v: &serde_json::Value| v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect::<Vec<_>>();
    for p in r["to_sentence"].as_array().unwrap() { assert_eq!(to_sentence(&strs(&p[0])), p[1].as_str().unwrap()); }
    for p in r["inspect"].as_array().unwrap() { assert_eq!(inspect(&strs(&p[0])), p[1].as_str().unwrap()); }
}

#[test]
fn float_to_d_reads_what_the_codec_cannot() {
    let cases = common::vectors()["decimals_wide"].as_array().unwrap().clone();
    assert!(cases.len() >= 8);
    for pair in &cases {
        let f = f64::from_bits(u64::from_str_radix(pair[0].as_str().unwrap(), 16).unwrap());
        assert_eq!(BigDec::from_f64(f).unwrap().to_s_f(), pair[1].as_str().unwrap(), "{pair}");
    }
}

fn bits(v: &serde_json::Value) -> f64 { f64::from_bits(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap()) }

#[test]
fn multi_term_round6_matches_ruby_time_arithmetic() {
    let rows = common::vectors()["ruby"]["round6_multi"].as_array().unwrap().clone();
    assert!(rows.len() >= 5);
    for p in &rows {
        let terms: Vec<(f64, i64)> = p[1].as_array().unwrap().iter().map(|t| (bits(&t[0]), t[1].as_i64().unwrap())).collect();
        assert_eq!(round6_micros(micros(p[0].as_str().unwrap()), &terms), micros(p[2].as_str().unwrap()), "{p}");
    }
}

#[test]
fn exceeds_matches_ruby_on_the_boundary_and_either_side() {
    let rows = common::vectors()["ruby"]["exceeds"].as_array().unwrap().clone();
    assert!(rows.len() >= 12);
    for p in &rows {
        let got = exceeds(micros(p[0].as_str().unwrap()), (bits(&p[1]), p[2].as_i64().unwrap()), micros(p[3].as_str().unwrap()));
        assert_eq!(got, p[4].as_bool().unwrap(), "{p}");
    }
}

#[test]
fn float_sum_is_rubys_compensated_sum() {
    let cases = common::vectors()["float_sum"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 22);
    for c in cases {
        let values: Vec<f64> = c[0].as_array().unwrap().iter().map(bits).collect();
        assert_eq!(float_sum(&values).to_bits(), bits(&c[1]).to_bits(), "{values:?}");
    }
    assert_ne!(float_sum(&[0.1, 0.2, 0.3]).to_bits(), (0.1f64 + 0.2 + 0.3).to_bits(), "not a plain left fold");
}

#[test]
fn a_decimal_10_6_column_reads_and_writes_as_activemodel_does() {
    let cases = common::vectors()["decimal_10_6"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 61);
    for c in cases {
        let f = bits(&c[0]);
        assert_eq!(decimal_column(f, 10, 6).unwrap().to_s_f(), c[1].as_str().unwrap(), "{f:e}");
    }
}

#[test]
fn time_as_json_is_activesupports_rendering_of_a_parsed_clock_time() {
    let cases = common::vectors()["time_as_json"].as_object().unwrap().clone();
    assert_eq!(cases.len(), 6);
    for (raw, want) in cases {
        let t = chrono::DateTime::parse_from_rfc3339(&raw).unwrap();
        assert_eq!(deltabadger::ruby::time_as_json(&raw, &t), want.as_str().unwrap(), "{raw}");
    }
}

/// Float#to_d and Float#to_s over 10k recorded floats, ties between two shortest digit strings among them
/// (script/rust/record_float_digits.rb). Every Rust port of either is held to the same rows.
#[test]
fn float_digits_match_ruby_on_ties_and_everything_else() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/float_digits.json")).unwrap();
    let rows = fixture["floats"].as_array().unwrap();
    assert!(rows.len() >= 10_000 && rows.iter().filter(|r| r[3] == true).count() >= 100);
    let mut failures = vec![];
    for r in rows {
        let f = bits(&r[0]);
        let (to_d, to_s) = (r[1].as_str().unwrap(), r[2].as_str().unwrap());
        let unsigned_zero = |s: String| if f == 0.0 { s.trim_start_matches('-').to_string() } else { s };
        let got = [
            ("BigDec::from_f64", unsigned_zero(BigDec::from_f64(f).unwrap().to_s_f()), unsigned_zero(to_d.into())),
            ("figures Dec::from_f64", deltabadger::figures::dec::Dec::from_f64(f).unwrap().to_s_f(), to_d.into()),
            ("ruby::float_to_s", float_to_s(f), to_s.into()),
            ("figures float_to_s", deltabadger::figures::num::float_to_s(f), to_s.into()),
            ("web float_to_s", deltabadger::web::format::float_to_s(f), to_s.into()),
        ];
        for (port, got, want) in got { if got != want { failures.push(format!("{port}({}) = {got}, Ruby {want}", r[0])); } }
        if let Ok(d) = deltabadger::codec::real_to_decimal(f) {
            if bd(&d.to_string()) != BigDec::from_f64(f).unwrap() { failures.push(format!("real_to_decimal({}) = {d}, Ruby {to_d}", r[0])); }
        }
    }
    let ports: std::collections::BTreeMap<&str, usize> = failures.iter().fold(Default::default(), |mut m, f| { *m.entry(f.split('(').next().unwrap()).or_default() += 1; m });
    assert!(failures.is_empty(), "{} differ {ports:?}:\n{}", failures.len(), failures.iter().step_by((failures.len() / 20).max(1)).cloned().collect::<Vec<_>>().join("\n"));
}
