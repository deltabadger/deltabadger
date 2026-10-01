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
