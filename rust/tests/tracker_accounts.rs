//! #710 slice 5 (PLAN-710-5 §6): balances, venue snapshots and the tracker per account, proven on two accounts of one
//! exchange (tests/common/two_accounts.rs). The account is the unit: each has its own rows, figures, freshness and
//! provenance; "All" is the whole portfolio.
mod common;
use common::seed;
use common::two_accounts::Two;
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::identity::AccountRow;
use deltabadger::jobs::Db;
use deltabadger::store::{self, Paths};
use deltabadger::sync::{balances::{self, NoPrices}, cache};
use deltabadger::tracker::Scope;
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Value};

const NOW: &str = "2026-09-20T02:00:00Z";

fn install() -> (tempfile::TempDir, store::Opened) {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o)
}
fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T { c.query_row(sql, [], |r| r.get(0)).unwrap() }
/// The material each account's key was stored with (seed::seed_kraken, two_accounts).
fn material(key: i64, two: &Two) -> Credentials {
    let (k, s) = if key == two.keys[0] { ("test-key", "dGVzdC1zZWNyZXQ=") } else { ("account-2-key", "account-2-secret") };
    Credentials { ibkr: None, redaction_values: vec![], key: k.into(), secret: s.into(), passphrase: None }
}

/// Kraken with two accounts, BTC and EUR listed, and the second connection the syncs write through.
fn kraken() -> (tempfile::TempDir, Connection, Db, Two, [AccountRow; 2]) {
    let (dir, o) = install();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let two = Two::kraken(&o.primary, &seed::cipher());
    for asset in [two.seeded.btc, two.seeded.quote] {
        c.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, 'now', 'now')", [asset, two.seeded.exchange_id]).unwrap();
    }
    let accounts = [0, 1].map(|n| AccountRow::of(&c, &two.accounts[n]).unwrap().unwrap());
    (dir, c, Db::new(o.primary, seed::cipher()), two, accounts)
}

fn answer(held: Value) -> ScriptedTransport { ScriptedTransport::from_script(&json!({ "POST /0/private/BalanceEx": [{ "status": 200, "body": { "error": [], "result": held } }] })) }

async fn sync(db: &Db, two: &Two, key: i64, held: Value) {
    balances::sync_kraken(db, answer(held), &NoPrices, key, &material(key, two), &FixedClock(NOW.parse().unwrap())).await.unwrap().unwrap();
}

fn store(c: &Connection) -> Vec<(i64, String, String)> {
    c.prepare("SELECT b.account_id, a.symbol, CAST(b.free AS TEXT) FROM exchange_account_balances b JOIN assets a ON a.id = b.asset_id ORDER BY b.account_id, a.symbol").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
}

/// §6 test 6: key B's sync writes, keeps and deletes only account B's rows; key A's only A's.
#[tokio::test(flavor = "current_thread")]
async fn each_keys_balance_sync_writes_keeps_and_deletes_only_its_own_accounts_rows() {
    let (_d, c, db, two, [a, b]) = kraken();
    sync(&db, &two, two.keys[0], json!({ "XBT": { "balance": "0.5", "hold_trade": "0" } })).await;
    sync(&db, &two, two.keys[1], json!({ "XBT": { "balance": "2", "hold_trade": "0" }, "ZEUR": { "balance": "10", "hold_trade": "0" } })).await;
    let s = |x: &str| x.to_string();
    assert_eq!(store(&c), [(a.id, s("BTC"), s("0.5")), (b.id, s("BTC"), s("2")), (b.id, s("EUR"), s("10"))]);
    // B no longer holds BTC: its row goes, A's stays.
    sync(&db, &two, two.keys[1], json!({ "ZEUR": { "balance": "10", "hold_trade": "0" } })).await;
    assert_eq!(store(&c), [(a.id, s("BTC"), s("0.5")), (b.id, s("EUR"), s("10"))]);
    // A's sync of nothing removes only A's.
    sync(&db, &two, two.keys[0], json!({})).await;
    assert_eq!(store(&c), [(b.id, s("EUR"), s("10"))]);
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM account_balances"), 0, "2.x's table is not written");
}

/// §6 test 5: each account's balances carry their own provenance. B's stamp missing makes B's scope and "All" stale and
/// leaves A's fresh; A's sync does not make B's old prices reusable.
#[tokio::test(flavor = "current_thread")]
async fn each_accounts_balance_provenance_is_its_own() {
    let (_d, c, db, two, [a, b]) = kraken();
    sync(&db, &two, two.keys[0], json!({ "XBT": { "balance": "0.5", "hold_trade": "0" } })).await;
    sync(&db, &two, two.keys[1], json!({ "XBT": { "balance": "2", "hold_trade": "0" } })).await;
    let user = two.seeded.user_id;
    let stale = |scope: Scope| cache::stale(&c, user, &scope).unwrap();
    assert!(!stale(Scope::All) && !stale(Scope::Account(a)) && !stale(Scope::Account(b)), "both synced");
    assert_ne!(cache::balance_origin_key_of(&a), cache::balance_origin_key_of(&b));
    assert_eq!(cache::balance_origin_key_of(&a), cache::balance_origin_key(user, two.seeded.exchange_id), "number 1 keeps 2.x's key");
    c.execute("DELETE FROM app_configs WHERE key = ?1", [cache::balance_origin_key_of(&b)]).unwrap();
    assert!(stale(Scope::All) && stale(Scope::Account(b)), "B's stamp is missing");
    assert!(!stale(Scope::Account(a)), "A's own stamp stands");
    sync(&db, &two, two.keys[0], json!({ "XBT": { "balance": "0.5", "hold_trade": "0" } })).await;
    assert!(stale(Scope::Account(b)), "A's sync stamps A only");
    let version = deltabadger::engine::model::credential_version_by_id(&c, two.keys[1]).unwrap().unwrap();
    assert!(!cache::produced_by(&c, &b, &version).unwrap(), "B's old prices are not reusable after A's sync");
}
