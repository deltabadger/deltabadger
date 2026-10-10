//! Ruby-free source drift and recorded decision contract.
use serde_json::Value;
use sha2::{Digest,Sha256};

#[test]
fn sources_and_history_vectors_are_reviewable() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let bytes=std::fs::read(root.join("rust/tests/fixtures/histories.json")).expect("record the synthetic history vectors first");
    let v:Value=serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["synthetic_only"],true);
    assert_eq!(v["cases"].as_array().unwrap().len(),24);
    for (name,hash) in v["ported_sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(name)).unwrap())),hash.as_str().unwrap(),"{name}: re-record");
    }
    for name in ["null_price_buy","cancelled_partial_buy"] {
        let c=v["cases"].as_array().unwrap().iter().find(|c|c["name"]==name).unwrap();
        assert_eq!(c["rails_unchanged"]["orders"][0]["quote"],"75.0");
        assert_eq!(c["normalized"]["orders"][0]["quote"],"60.0");
        assert_eq!(c["rails_unchanged"]["contributed"],"70.0");
        assert_eq!(c["normalized"]["contributed"],"100.0");
    }
    for (name,first,later) in [("merge_before_start","100.0","200.0"),("merge_at_start","0.0","100.0")] {
        let c=v["cases"].as_array().unwrap().iter().find(|c|c["name"]==name).unwrap();
        assert_eq!(c["rails_unchanged"],c["normalized"]);
        assert_eq!(c["normalized"]["pending"],first);
        assert_eq!(c["one_week_pending"],later);
        assert_eq!(c["one_week_rails_pending"],later);
    }
}

#[test]
fn round_one_oracles_pin_every_source_and_warning_locale() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let v:Value=serde_json::from_slice(&std::fs::read(root.join("rust/tests/fixtures/histories_r1.json")).expect("record round-one vectors")).unwrap();
    assert_eq!(v["synthetic_only"],true);
    assert_eq!(v["caps"].as_array().unwrap().len(),3);
    for (name,hash) in v["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(name)).unwrap())),hash.as_str().unwrap(),"{name}: re-record");
    }
    for case in v["warnings"].as_array().unwrap() {
        assert_eq!(case["captures"].as_array().unwrap().len(),1);
        assert_eq!(case["second_warning_count"],0);
        assert_eq!(case["captures"][0]["payloads"].as_object().unwrap().len(),15);
    }
}

#[test]
fn round_two_exact_cost_and_rails_minimum_oracles_are_pinned() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let v:Value=serde_json::from_slice(&std::fs::read(root.join("rust/tests/fixtures/histories_r2.json")).expect("record round-two vectors")).unwrap();
    assert_eq!(v["synthetic_only"],true);
    assert_eq!(v["cases"].as_array().unwrap().len(),3);
    for (name,hash) in v["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(name)).unwrap())),hash.as_str().unwrap(),"{name}: re-record");
    }
    assert_eq!(v["cases"][0]["steps"],1);
    assert_eq!(v["cases"][1]["rails_wire"]["qty"],"0.333333333");
    assert_eq!(v["cases"][2]["captures"][0]["payloads"].as_object().unwrap().len(),15);
}


#[test]
fn round_three_sql_boundary_oracles_are_pinned() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let v:Value=serde_json::from_slice(&std::fs::read(root.join("rust/tests/fixtures/histories_r3.json")).expect("record round-three vectors")).unwrap();
    assert_eq!(v["synthetic_only"],true);
    for (name,hash) in v["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(name)).unwrap())),hash.as_str().unwrap(),"{name}: re-record");
    }
    assert_eq!(v["cases"][0]["cutoff"],3);
    assert_eq!(v["cases"][1]["cutoff"],"3");
    assert_eq!(v["cases"][0]["own_ids"],serde_json::json!([]));
    assert_eq!(v["cases"][1]["own_ids"],serde_json::json!([]));
    assert_eq!(v["cases"][0]["orders"],v["cases"][1]["orders"]);
    assert_eq!(v["cases"][0]["orders"][0]["quote"],"60.0");
    assert_eq!(v["cases"][0]["orders"][1]["quote"],"40.0");
}

#[test]
fn r6_oracle_sources_are_pinned() {
    use sha2::{Digest,Sha256};
    let v:Value=serde_json::from_str(include_str!("fixtures/histories_r6.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (f,h) in v["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(f)).unwrap())),h.as_str().unwrap());}
    assert_eq!(v["rails_unchanged"],v["normalized"]);
    assert_eq!(v["normalized"]["orders"][0]["notional"],"9.99");
}


#[test]
fn r7_integer_add_overflow_refuses() {
    use deltabadger::ruby::Num;
    for (a,b) in [(i64::MAX,1),(i64::MIN,-1)] {
        let result=Num::Int(a).add(&Num::Int(b));
        assert!(result.is_err(), "R7 integer add must refuse: {result:?}");
        assert!(format!("{result:?}").contains("IntegerOverflow"));
    }
    assert_eq!(Num::Int(i64::MAX-1).add(&Num::Int(1)).unwrap(),Num::Int(i64::MAX));
}
#[test]
fn r7_integer_sub_overflow_refuses() {
    use deltabadger::ruby::Num;
    for (a,b) in [(i64::MIN,1),(i64::MAX,-1)] {
        let result=Num::Int(a).sub(&Num::Int(b));
        assert!(result.is_err(), "R7 integer sub must refuse: {result:?}");
        assert!(format!("{result:?}").contains("IntegerOverflow"));
    }
    assert_eq!(Num::Int(i64::MIN+1).sub(&Num::Int(1)).unwrap(),Num::Int(i64::MIN));
}
#[test]
fn r7_integer_sum_overflow_refuses() {
    use deltabadger::ruby::{Num,ruby_sum};
    for values in [vec![Num::Int(5_000_000_000_000_000_000);2],vec![Num::Int(i64::MIN),Num::Int(-1)],vec![Num::Int(i64::MAX),Num::Int(1),Num::Float(0.0)]] {
        let result=ruby_sum(&values);
        assert!(result.is_err(), "R7 integer sum must refuse: {result:?}");
        assert!(format!("{result:?}").contains("IntegerOverflow"));
    }
    assert_eq!(ruby_sum(&[Num::Int(i64::MAX-1),Num::Int(1)]).unwrap(),Num::Int(i64::MAX));
}

#[test]
fn r7_integer_oracle_is_pinned() {
    use sha2::{Digest,Sha256};
    let v:serde_json::Value=serde_json::from_str(include_str!("fixtures/histories_r7.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (f,h) in v["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(f)).unwrap())),h.as_str().unwrap());}
    assert_eq!(v["rails_unchanged"],v["normalized"]);
    assert_eq!(v["normalized"]["sql_kinds"],serde_json::json!(["integer","integer"]));
    assert_eq!(v["normalized"]["cap"],"0");
    assert_eq!(v["normalized"]["carry"],"0.0");
    assert_eq!(v["normalized"]["orders"],serde_json::json!([]));
    assert_eq!(v["closed_reconstructed"]["rails_unchanged"]["cap"],serde_json::Value::Null);
    assert_eq!(v["closed_reconstructed"]["normalized"]["cap"],"90.0");
    assert_eq!(v["closed_reconstructed"]["normalized"]["pending"],"90.0");
}

#[test]
fn r8_integer_oracle_is_pinned() {
    use sha2::{Digest,Sha256};
    let v:serde_json::Value=serde_json::from_str(include_str!("fixtures/histories_r8.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (f,h) in v["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(f)).unwrap())),h.as_str().unwrap());}
    for case in v["cases"].as_array().unwrap() { assert_eq!(case["amount_class"],"Integer");assert_eq!(case["sql_kind"],"integer");assert_eq!(case["pending"],"100.0"); }
}

#[test]
fn r9_timestamp_parser_and_actual_rails_oracle() {
    use deltabadger::codec::parse_time;
    use sha2::{Digest,Sha256};
    let v:serde_json::Value=serde_json::from_str(include_str!("fixtures/histories_r9.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (file,hash) in v["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(file)).unwrap())),hash.as_str().unwrap(),"{file}");
    }
    for case in v["cases"].as_array().unwrap() {
        assert_eq!(parse_time(case["stamp"].as_str().unwrap()).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Micros,true),case["parsed"]);
        assert_eq!(case["carry"],"40.0");assert_eq!(case["pending"],"40.0");assert_eq!(case["orders"][0]["notional"],"40.00");
    }
    assert_eq!(parse_time("2026-01-01 00:00:00.123456").unwrap(),parse_time("2026-01-01T02:00:00.123456+02:00").unwrap());
    for bad in ["", "garbage", "2026-01-01", "2026-01-01 25:00:00", "2026-01-01 00:00:00 trailing", "2026-01-01 00:00:00 +éé"] {assert!(parse_time(bad).is_err(),"{bad}");}
}

#[test]
fn r9c_sql_timestamp_grammar_range_and_errors_match_base() {
    use chrono::NaiveDateTime;
    use deltabadger::codec::{parse_time,CodecError};
    for text in ["-262143-01-01 00:00:00", "+262142-12-31 23:59:59", "2026-1-1 0:0:0",
                 "2026-01-01 00:00:00.1234567890", "2026-01-01 00:00:60", "garbage", "", "2026-01-01 25:00:00"] {
        let base = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
            .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"));
        match base {
            Ok(at) => assert_eq!(parse_time(text).unwrap(), at.and_utc(), "{text}"),
            Err(error) => assert!(matches!(parse_time(text), Err(CodecError::Time(message)) if message == format!("{text:?}: {error}")), "{text}"),
        }
    }
}
