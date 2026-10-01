mod common;
use deltabadger::store::{self, Paths, StoreError};

#[test]
fn a_rails_prepared_install_satisfies_the_engine_contract() {
    let dir = common::rails_install();
    store::check(&Paths::from_env(&|_| None, dir.path())).expect("every engine table and column is present as Rails creates it");
}

#[test]
fn a_missing_engine_column_is_named() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    rusqlite::Connection::open(&p.primary).unwrap()
        .execute_batch("ALTER TABLE transactions DROP COLUMN quote_amount_exec").unwrap();
    match store::check(&p) {
        Err(StoreError::Incompatible { problems }) => assert!(problems.iter().any(|m| m.contains("transactions.quote_amount_exec")), "{problems:?}"),
        other => panic!("expected Incompatible, got {other:?}"),
    }
}

#[test]
fn the_seed_helpers_write_rows_rails_can_read() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let o = store::open(&p).unwrap();
    let s = common::seed::seed_kraken(&o.primary, &common::seed::cipher());
    let bot = common::seed::insert_bot(&o.primary, &s, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let ty: String = o.primary.query_row("SELECT type FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap();
    assert_eq!(ty, "Bots::DcaMultiAsset");
}
