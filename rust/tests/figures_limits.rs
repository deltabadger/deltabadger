//! The hostile numbers of rust/examples/figures_limits.rs, in `cargo test`. The example is how they are run in the
//! release profile, where a panic aborts.
#[path = "../examples/figures_limits.rs"]
mod limits;

#[test]
fn hostile_numbers_are_refused_quickly_and_nothing_is_computed_from_them() {
    assert_eq!(limits::run(), Ok(89));
}


#[test]
fn distinct_unfilled_symbols_share_the_walk_budget() {
    use deltabadger::figures::{at::At, budget::{self, Limits}, db, walk, FiguresError, OVER_BUDGET};
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE assets (id INTEGER, symbol TEXT, name TEXT);
        INSERT INTO assets VALUES (2, 'AAA', 'Asset');
        CREATE TABLE tickers (exchange_id INTEGER, base TEXT, base_asset_id INTEGER);
        CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, entry_type INTEGER, raw_data TEXT, base_currency TEXT, base_asset_id INTEGER, transacted_at TEXT);").unwrap();
    let s = db::Subject {
        bot: db::Bot { id: 1, user_id: 1, exchange_id: Some(1), kind: db::Kind::Basket,
            exchange_type: Some("Exchanges::Alpaca".into()), quote_asset_id: Some(1), base_asset_ids: vec![2] },
        quote: Some("USD".into()), tickers: vec![],
        orders: (0..100_000).map(|i| db::Order {
            id: i, at: At(i), exchange_id: Some(1), raw: deltabadger::figures::fill::Raw::new(None,None,None,None), base: Some(format!("SYM{i:06}")), asset_id: Some(2),
            sell: false, buy: true, closed: false, kind: "REGULAR".into(),
        }).collect(),
    };
    let small = Limits { steps: 50_000, held: 1_000_000 };
    let (out, used) = budget::scope(small, || walk::metrics(&c, &s, At(100_000)));
    assert!(matches!(out, Err(FiguresError::NotComputed(reason)) if reason == OVER_BUDGET));
    assert!(used.steps <= small.steps);
    let enough = Limits { steps: 2_000_000, held: 1_000_000 };
    let (out, used) = budget::scope(enough, || walk::metrics(&c, &s, At(100_000)));
    let data = out.unwrap();
    assert_eq!(data.key_strings[0].1.len(), 100_000);
    assert_eq!(data.key_strings[0].1[0], "SYM000000");
    assert_eq!(data.key_strings[0].1[99_999], "SYM099999");
    assert!(used.steps >= 300_000 && used.steps <= enough.steps, "{used:?}");
    // Split and chart preparation must check the same meter even when every row is unfilled.
    let holdings = data.holdings().unwrap();
    let (out, used) = budget::scope(small, || deltabadger::figures::splits::events(&c, 1, &s.orders, &holdings, At(100_000)));
    assert!(matches!(out, Err(FiguresError::NotComputed(reason)) if reason == OVER_BUDGET));
    assert!(used.steps <= small.steps);
    let (out, used) = budget::scope(small, || deltabadger::figures::chart::buy_marks(&s, &data));
    assert!(matches!(out, Err(FiguresError::NotComputed(reason)) if reason == OVER_BUDGET));
    assert!(used.steps <= small.steps);
}

#[test]
fn settings_ids_follow_ruby_integer_conversion() {
    use deltabadger::figures::db;
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE bots (id INTEGER, user_id INTEGER, exchange_id INTEGER, type TEXT, settings TEXT);
        INSERT INTO bots VALUES (1, 1, NULL, 'Bots::DcaMultiAsset', '{}');").unwrap();
    // ActiveRecord's integer predicate truncates Floats, and excludes integers outside SQLite's range.
    for (value, expected) in [("1.0", Some(1)), ("1.9", Some(1)), ("-1.9", Some(-1)),
        ("1e100", None), ("-1e100", None), ("9223372036854775808.0", None),
        ("-9223372036854775808.0", Some(i64::MIN))] {
        c.execute("UPDATE bots SET settings = ?1", [format!("{{\"quote_asset_id\":{value}}}")]).unwrap();
        assert_eq!(db::bot(&c, 1).unwrap().quote_asset_id, expected, "{value}");
    }
    for (value, expected) in [("1.9", 1), ("-1.9", -1), ("\"  +1_2suffix\"", 12)] {
        c.execute("UPDATE bots SET settings = ?1", [format!("{{\"base_asset_ids\":[{value}]}}")]).unwrap();
        assert_eq!(db::bot(&c, 1).unwrap().base_asset_ids, vec![expected]);
    }
    c.execute("UPDATE bots SET settings = ?1", [r#"{"base_asset_ids":[1e100]}"#]).unwrap();
    assert!(matches!(db::bot(&c, 1), Err(deltabadger::figures::FiguresError::NotComputed(reason))
        if reason == deltabadger::figures::OUT_OF_RANGE));
}
