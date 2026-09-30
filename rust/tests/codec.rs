mod common;
use chrono::{DateTime, Utc};
use deltabadger::codec::*;
use rusqlite::types::ValueRef;
use rust_decimal::Decimal;
use std::str::FromStr;

#[test]
fn datetimes_format_and_parse_like_rails_quoted_date() {
    for pair in common::vectors()["times"].as_array().unwrap() {
        let t: DateTime<Utc> = pair[0].as_str().unwrap().parse().unwrap();
        let rails = pair[1].as_str().unwrap();
        assert_eq!(format_time(t), rails);
        assert_eq!(parse_time(rails).unwrap(), t);
    }
}

#[test]
fn decimals_read_from_real_like_rails() {
    for pair in common::vectors()["decimals"].as_array().unwrap() {
        let f = f64::from_bits(u64::from_str_radix(pair[0].as_str().unwrap(), 16).unwrap());
        let got = decimal_from_sql(ValueRef::Real(f)).unwrap().unwrap();
        assert_eq!(got, Decimal::from_str(pair[1].as_str().unwrap()).unwrap(), "{pair}");
    }
}

#[test]
fn decimals_read_from_integer_text_and_null() {
    assert_eq!(decimal_from_sql(ValueRef::Integer(60)).unwrap(), Some(Decimal::from(60)));
    assert_eq!(decimal_from_sql(ValueRef::Text(b"30.0003")).unwrap(), Some(Decimal::from_str("30.0003").unwrap()));
    assert_eq!(decimal_from_sql(ValueRef::Null).unwrap(), None);
    assert!(decimal_from_sql(ValueRef::Real(1.0e-40)).is_err(), "out of Decimal range is an error, not a panic");
    assert!(decimal_from_sql(ValueRef::Real(f64::MAX)).is_err());
}

#[test]
fn decimals_written_as_text_become_real_under_numeric_affinity() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE t (amount decimal)").unwrap();
    db.execute("INSERT INTO t VALUES (?1)", [decimal_to_sql(Decimal::from_str("30.0003").unwrap())]).unwrap();
    let ty: String = db.query_row("SELECT typeof(amount) FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(ty, "real");
}

#[test]
fn the_ruby_gems_the_vectors_came_from_are_the_ones_in_the_lockfile() {
    let lock = include_str!("../../Gemfile.lock");
    for (gem, version) in common::vectors()["gems"].as_object().unwrap() {
        let line = format!("    {gem} ({})", version.as_str().unwrap());
        assert!(lock.contains(&line), "{gem} moved: re-run script/rust/record_vectors.rb and re-prove (expected `{line}`)");
    }
}
