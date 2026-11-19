//! #710 slice 4 (PLAN-710-4 §6): the ledger and bot orders per account, proven on two accounts of one exchange that share
//! transaction, order and group ids (tests/common/two_accounts.rs). Only one account can exist until slice 7, so every path
//! that reads or writes a venue id is run here over both forms: number 1's id in 2.x's column, 2+'s in its membership.
mod common;
use common::seed;
use common::two_accounts::{Two, SHARED_TX};
use deltabadger::store::{self, Paths};
use deltabadger::venue_ids::{self, Scope, Write};
use rusqlite::{params, Connection};

fn install() -> (tempfile::TempDir, store::Opened) {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o)
}

fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T { c.query_row(sql, [], |r| r.get(0)).unwrap() }

fn ids(c: &Connection, scope: &Scope) -> Vec<(i64, Option<String>)> {
    c.prepare(&format!("SELECT a.id, {} FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND {} ORDER BY a.id",
                       venue_ids::row_id("a"), scope.rows("a"))).unwrap()
        .query_map([scope.user_id, scope.exchange_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}

fn constraint(e: rusqlite::Error) -> bool { matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation) }

/// §6 test 1: each accessor over number 1 and 2; the trigger refuses every membership but an account 2+'s own, id-less row.
#[test]
fn the_accessor_reads_and_writes_each_accounts_ids_and_the_trigger_keeps_the_two_forms_apart() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [first, second] = two.scopes;
    assert_eq!((first.member, second.member), (None, two.accounts[1].id));
    let a = two.row(c, 0, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
    let b = two.row(c, 1, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
    let bare = two.row(c, 1, 4, "BTC", "2", None, None, "2026-09-01 10:00:00");
    // The two forms: number 1 in 2.x's column, 2+ in the membership.
    assert_eq!(one::<Option<String>>(c, &format!("SELECT tx_id FROM account_transactions WHERE id = {a}")).as_deref(), Some(SHARED_TX));
    assert_eq!(one::<Option<String>>(c, &format!("SELECT tx_id FROM account_transactions WHERE id = {b}")), None);
    assert_eq!(ids(c, &first), [(a, Some(SHARED_TX.into()))]);
    assert_eq!(ids(c, &second), [(b, Some(SHARED_TX.into())), (bare, None)]);
    for (row, account) in [(a, two.accounts[0].id), (b, two.accounts[1].id), (bare, two.accounts[1].id)] {
        assert_eq!(one::<Option<i64>>(c, &format!("SELECT {} FROM account_transactions a WHERE a.id = {row}", venue_ids::row_account("a"))), account);
    }
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM account_transactions a WHERE {}", venue_ids::id_less("a"))), 1);
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM account_transactions a WHERE {}", venue_ids::has_id("a"))), 2);
    assert_eq!((first.stored(Some("x")), second.stored(Some("x"))), (Some("x"), None));
    for scope in [first, second] {
        assert!(venue_ids::known(c, &scope, SHARED_TX).unwrap());
        assert!(!venue_ids::known(c, &scope, "other").unwrap());
    }
    // The same id twice on one account is the unique clash 2.x's index makes on number 1.
    let clash = c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, created_at, updated_at) \
                           VALUES (?1, ?2, 4, 'BTC', 3, '2026-09-01', 'now', 'now')", [two.seeded.user_id, two.seeded.exchange_id]).map(|_| c.last_insert_rowid()).unwrap();
    assert!(constraint(venue_ids::after_insert(c, &second, clash, Some(SHARED_TX)).unwrap_err()));
    c.execute("DELETE FROM account_transactions WHERE id = ?1", [clash]).unwrap();

    // set_id's three semantics, on both accounts; a row of the other account is never written.
    for (scope, row) in [(first, a), (second, b)] {
        assert_eq!(venue_ids::set_id(c, &scope, row, None, Write::IfGiven).unwrap(), 1);
        assert_eq!(ids(c, &scope)[0].1.as_deref(), Some(SHARED_TX), "IfGiven without an id keeps the stored one");
        venue_ids::set_id(c, &scope, row, Some("new"), Write::IfGiven).unwrap();
        assert_eq!(ids(c, &scope)[0].1.as_deref(), Some("new"), "IfGiven with an id replaces it");
        assert_eq!(venue_ids::set_id(c, &scope, row, Some("other"), Write::Adopt).unwrap(), 0, "Adopt takes only an id-less row");
        venue_ids::set_id(c, &scope, row, Some(SHARED_TX), Write::Replace).unwrap();
        assert_eq!(ids(c, &scope)[0].1.as_deref(), Some(SHARED_TX));
    }
    assert_eq!(venue_ids::set_id(c, &first, b, Some("x"), Write::Replace).unwrap(), 0, "number 1 never writes a member");
    assert_eq!(venue_ids::set_id(c, &second, a, Some("x"), Write::Replace).unwrap(), 0, "account 2 never writes number 1's row");
    assert_eq!(one::<Option<String>>(c, &format!("SELECT tx_id FROM account_transactions WHERE id = {b}")), None, "a member's 2.x column stays NULL");
    assert_eq!(venue_ids::set_id(c, &second, bare, Some("adopted"), Write::Adopt).unwrap(), 1);
    venue_ids::set_id(c, &second, bare, None, Write::Replace).unwrap();

    // The trigger: a membership on number 1, on another exchange's or user's account, or on a row holding a 2.x id.
    let insert = |row: i64, account: i64| c.execute("INSERT INTO exchange_account_rows (account_transaction_id, account_id, venue_tx_id) VALUES (?1, ?2, 'z')", [row, account]);
    let fresh = |tx: Option<&str>| {
        c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, created_at, updated_at) \
                   VALUES (?1, ?2, 4, 'BTC', 9, ?3, '2026-09-02', 'now', 'now')", params![two.seeded.user_id, two.seeded.exchange_id, tx]).unwrap();
        c.last_insert_rowid()
    };
    let other = Two::venue(c, &seed::cipher(), &two.seeded, "Exchanges::Binance", "Binance");
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('x@example.com', 'x', 'X', 0, 'now', 'now')", []).unwrap();
    let stranger = c.last_insert_rowid();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [stranger, two.seeded.exchange_id]).unwrap();
    let strangers = c.last_insert_rowid();
    for (row, account, why) in [(fresh(None), two.accounts[0].id.unwrap(), "number 1"), (fresh(None), other.accounts[1].id.unwrap(), "another exchange's"),
                                (fresh(None), strangers, "another user's"), (fresh(Some("held")), two.accounts[1].id.unwrap(), "a row holding a 2.x id")] {
        let e = insert(row, account).unwrap_err();
        assert!(e.to_string().contains("a member is an account 2+"), "{why}: {e}");
    }
    insert(fresh(None), two.accounts[1].id.unwrap()).unwrap();

    // adopt_pair: both legs or neither; a clash of the second leg on account 2 adopts neither and reads as the clash.
    for scope in [first, second] {
        let n = if scope.member.is_some() { 1 } else { 0 };
        let (out, inn) = (two.row(c, n, 6, "ETH", "1", None, Some("dustcsv1"), "2026-09-03"), two.row(c, n, 5, "BNB", "1", None, Some("dustcsv1"), "2026-09-03"));
        assert_eq!(venue_ids::adopt_pair(c, &scope, [(out, "dust-1-out"), (inn, "dust-1-in")]).unwrap(), 2);
        let (out, inn) = (two.row(c, n, 6, "ETH", "2", None, Some("dustcsv2"), "2026-09-04"), two.row(c, n, 5, "BNB", "2", None, Some("dustcsv2"), "2026-09-04"));
        let e = venue_ids::adopt_pair(c, &scope, [(out, "dust-2-out"), (inn, "dust-1-in")]).unwrap_err();
        assert!(constraint(e));
        let left: Vec<Option<String>> = [out, inn].iter().map(|r| one(c, &format!("SELECT {} FROM account_transactions a WHERE a.id = {r}", venue_ids::row_id("a")))).collect();
        assert_eq!(left, [None, None], "neither leg adopted");
    }
}

/// §6 test 12 (§2.1): the web's and the jobs' connections enforce foreign keys, so a ledger row's delete cascades its
/// membership; the cascade takes that membership and no other.
#[tokio::test(flavor = "current_thread")]
async fn the_web_and_jobs_connections_enforce_foreign_keys_and_a_delete_cascades_only_its_membership() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (b, kept) = (two.row(c, 1, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01"), two.row(c, 1, 4, "BTC", "2", Some("kept"), None, "2026-09-01"));
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at("2026-09-05T10:00:00Z"));
    assert_eq!(app.db(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))?)).await.unwrap(), 1);
    let db = deltabadger::jobs::Db::new(store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary, seed::cipher());
    assert_eq!(db.run(|c, _| c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())).await.unwrap(), 1);
    db.run(move |c, _| c.execute("DELETE FROM account_transactions WHERE id = ?1", [b]).map_err(|e| e.to_string())).await.unwrap();
    let left: Vec<i64> = c.prepare("SELECT account_transaction_id FROM exchange_account_rows").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(left, [kept]);
}
