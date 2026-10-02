//! BigDec refuses what it cannot hold cheaply: every refusal is an error returned at once, never an allocation.
use deltabadger::ruby::{from_sql, BigDec};
use rusqlite::types::ValueRef;

#[test]
fn huge_and_tiny_exponents_are_refused() {
    for s in ["1e-1000000000", "1e1000000000", "-1E+1000000000", "1e401", "1e-401", "0.5e-400", "1e99999999999999999999999"] {
        assert!(BigDec::parse(s).is_err(), "{s} must be refused");
    }
    // The edges of the range still parse, and so does every f64 written out in full.
    for s in ["1e400", "1e-400", "1.7976931348623157e308", "4.9406564584124654e-324", "0.000000000000000001"] {
        assert!(BigDec::parse(s).is_ok(), "{s} must parse");
    }
}

#[test]
fn an_over_long_input_is_refused_before_it_is_read() {
    assert!(BigDec::parse(&"1".repeat(256)).is_ok());
    assert!(BigDec::parse(&"1".repeat(257)).is_err());
    assert!(BigDec::parse(&format!("0.{}", "0".repeat(10_000_000))).is_err());
}

#[test]
fn a_value_needing_more_than_the_digit_cap_is_refused() {
    // 200 digits ending at 10^-512: 512 digits written out is the cap; one more is refused (the exponent, 10^-313, is in range).
    // On the integer side the exponent cap comes first: 200 digits up to 10^400 parse, up to 10^401 do not.
    assert!(BigDec::parse(&format!("{}e201", "9".repeat(200))).is_ok());
    assert!(BigDec::parse(&format!("{}e202", "9".repeat(200))).is_err());
    assert!(BigDec::parse(&format!("0.{}e-312", "9".repeat(200))).is_ok());
    assert!(BigDec::parse(&format!("0.{}e-313", "9".repeat(200))).is_err());
}

#[test]
fn a_decimal_column_holding_a_huge_exponent_is_an_error() {
    assert!(from_sql(ValueRef::Text(b"1e-1000000000")).is_err());
    assert!(from_sql(ValueRef::Text(b"1e1000000000")).is_err());
}

#[test]
fn a_huge_rescale_is_an_error_and_a_wide_one_never_pads() {
    use deltabadger::ruby::scale;
    for bad in [1_000_000_000, 41, -1, i64::MAX, i64::MIN] { assert!(scale(bad).is_err(), "{bad}"); }
    assert_eq!((scale(0).unwrap(), scale(18).unwrap(), scale(40).unwrap()), (0, 18, 40));
    // places is a u8: the widest is 255, and a value already within it comes back as it is.
    let tiny = BigDec::parse("1e-400").unwrap();
    assert_eq!(tiny.floor(255), BigDec::zero());
    assert_eq!(tiny.ceil(255).to_s_f(), format!("0.{}1", "0".repeat(254)));
    assert_eq!(BigDec::parse("1.25").unwrap().round(255), BigDec::parse("1.25").unwrap());
}

#[test]
fn a_venue_number_that_is_not_a_finite_decimal_is_an_error_not_zero() {
    use deltabadger::ruby::json_to_d;
    use serde_json::json;
    for bad in [json!("NaN"), json!("nan"), json!("Infinity"), json!("-Infinity"), json!("inf"), json!("garbage"), json!("12abc"), json!(""),
                json!("1e-1000000000"), json!(true), json!([]), json!({})] {
        assert!(json_to_d(&bad).is_err(), "{bad}");
    }
    assert_eq!(json_to_d(&json!(null)).unwrap(), None);
    assert_eq!(json_to_d(&json!("64328.1")).unwrap(), Some(BigDec::parse("64328.1").unwrap()));
    assert_eq!(json_to_d(&json!(" 0.5 ")).unwrap(), Some(BigDec::parse("0.5").unwrap()));
    assert_eq!(json_to_d(&json!(64321.5)).unwrap(), Some(BigDec::parse("64321.5").unwrap()));
    assert_eq!(json_to_d(&json!(7)).unwrap(), Some(BigDec::from_i64(7)));
}
