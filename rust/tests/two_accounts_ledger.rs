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
    c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('x@example.com', 'x', 0, 'now', 'now')", []).unwrap();
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

// ---- commit 2: ledger writes and dedup (§6 tests 2–4, 13) ----

use deltabadger::engine::model;
use deltabadger::sync::{self, activities::{Entry, Raw}, binance_identity::{Context, Event, Source}, binance_writer as writer, binance_repair as repair};
use deltabadger::v2::BigDec;
use serde_json::json;

fn now() -> chrono::DateTime<chrono::Utc> { "2026-09-10T00:00:00Z".parse().unwrap() }

fn entry(kind: i64, coin: &str, amount: &str, id: Option<&str>, group: Option<&str>, at: &str) -> Entry {
    Entry { entry_type: kind, base_currency: Some(coin.into()), base_amount: BigDec::parse(amount).unwrap(), quote_currency: None, quote_amount: None,
            fee_currency: None, fee_amount: None, tx_id: id.map(str::to_string), group_id: group.map(str::to_string), description: None,
            transacted_at: Some(at.parse().unwrap()), raw: Raw::from_value(json!({})) }
}

fn store(c: &Connection, key: &sync::Key, entries: Vec<Entry>) -> sync::ledger::Outcome {
    let tx = model::immediate(c).unwrap();
    let out = sync::ledger::store_file(&model::fence_versions(&tx, &[]).unwrap(), key, entries, now()).unwrap();
    tx.commit().unwrap();
    out
}

fn keys(c: &Connection, two: &Two, venue: &str) -> [sync::Key; 2] { two.keys.map(|id| sync::load_key_of(c, id, &[venue]).unwrap()) }

/// §6 tests 2 and 3: the same entries synced for account 1 then 2 are stored twice and re-synced as duplicates, account 2's
/// 2.x id NULL; an id-less row of one account is never the other's id'd row's twin, either way round.
#[test]
fn each_account_stores_and_dedups_its_own_rows_and_id_less_rows_pair_only_within_an_account() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [k1, k2] = keys(c, &two, "Exchanges::Kraken");
    assert_eq!((k1.scope, k2.scope), (two.scopes[0], two.scopes[1]));
    let shared = || vec![entry(4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01T10:00:00Z")];
    for key in [&k1, &k2] { assert_eq!(store(c, key, shared()).imported, 1); }
    for key in [&k1, &k2] { assert_eq!((store(c, key, shared()).imported, store(c, key, shared()).duplicates), (0, 1)); }
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions WHERE tx_id IS NULL"), 1, "account 2's 2.x id stays NULL");
    for scope in two.scopes { assert_eq!(ids(c, &scope).iter().map(|r| r.1.clone()).collect::<Vec<_>>(), [Some(SHARED_TX.to_string())]); }
    // Account 1 holds an id-less file row; account 2's API row of the same type, amount and second is its own.
    assert_eq!(store(c, &k1, vec![entry(4, "BTC", "5", None, None, "2026-09-02T10:00:00Z")]).imported, 1);
    assert_eq!(store(c, &k2, vec![entry(4, "BTC", "5", Some("api-5"), None, "2026-09-02T10:00:00Z")]).imported, 1, "nothing adopted, nothing skipped");
    assert_eq!(store(c, &k1, vec![entry(4, "BTC", "5", Some("api-5"), None, "2026-09-02T10:00:00Z")]).duplicates, 1, "number 1 pairs its own as before");
    // And the reverse.
    assert_eq!(store(c, &k2, vec![entry(4, "BTC", "7", None, None, "2026-09-03T10:00:00Z")]).imported, 1);
    assert_eq!(store(c, &k1, vec![entry(4, "BTC", "7", Some("api-7"), None, "2026-09-03T10:00:00Z")]).imported, 1);
    assert_eq!(store(c, &k2, vec![entry(4, "BTC", "7", Some("api-7"), None, "2026-09-03T10:00:00Z")]).duplicates, 1);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), 6);
}

fn claim(c: &Connection, dir: &std::path::Path) -> deltabadger::lease::EngineLock {
    let lock = deltabadger::lease::lock(&Paths::from_env(&|_| None, dir), now()).unwrap();
    deltabadger::lease::claim(&lock, c, &seed::cipher(), "test", now()).unwrap();
    lock
}

fn deposit(id: &str) -> writer::Input {
    let raw = json!({"id": id, "txId": "hash", "coin": "BTC", "network": "BTC", "address": "addr", "addressTag": "", "amount": "1",
                     "insertTime": 1786960800000_i64, "applyTime": "2026-08-17 10:00:00", "transactionFee": "0.01"});
    writer::Input { event: Event::read(Source::Deposit, &raw, Context { parent: Some(""), occurred_ms: Some(1786960800000), ..Context::default() }).unwrap(),
        entry: Entry { entry_type: 4, base_currency: Some("BTC".into()), base_amount: BigDec::parse("1").unwrap(), quote_currency: None, quote_amount: None,
                       fee_currency: None, fee_amount: None, tx_id: Some("hash".into()), group_id: None, description: None,
                       transacted_at: Some("2026-08-17T10:00:00Z".parse().unwrap()), raw: Raw::from_value(raw) } }
}

fn dust_csv(coin: &str, amount: &str, at: &str) -> Vec<Entry> {
    let group = Some(format!("dustcsv_{coin}"));
    vec![Entry { group_id: group.clone(), ..entry(3, coin, amount, None, None, at) }, Entry { group_id: group, ..entry(2, "BNB", "0.1", None, None, at) }]
}

/// §6 test 4 (Binance): the writer keeps each account's buckets (number 1's names unchanged), one deposit stored once per
/// account; one CSV's dust pairs import into both accounts and twice; an incomplete pair on one account blocks only its own.
#[test]
fn the_binance_writer_keeps_buckets_and_dust_pairs_per_account() {
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    let _lock = claim(c, dir.path());
    let [k1, k2] = keys(c, &two, "Exchanges::Binance");
    let write = |key: &sync::Key, rows: &[writer::Input]| { let tx = model::immediate(c).unwrap();
        let out = writer::store(&model::fence_versions(&tx, &[]).unwrap(), &seed::cipher(), key, rows, now()).unwrap(); tx.commit().unwrap(); out };
    for key in [&k1, &k2] { assert_eq!(write(key, &[deposit("d1")]).imported, 1); }
    for key in [&k1, &k2] { assert_eq!(write(key, &[deposit("d1")]).duplicates, 1); }
    let buckets: Vec<String> = c.prepare("SELECT key FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%' ORDER BY key").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let (u, e, n) = (two.seeded.user_id, two.seeded.exchange_id, two.accounts[1].id.unwrap());
    assert_eq!(buckets.len(), 2);
    assert!(buckets.iter().any(|b| b.starts_with(&format!("rust_binance_a12_v1:bucket:{u}:{e}:"))), "number 1's name as before: {buckets:?}");
    assert!(buckets.iter().any(|b| b.starts_with(&format!("rust_binance_a12_v1:bucket:{u}:{e}@{n}:"))), "{buckets:?}");
    let csv = |key: &sync::Key, rows: Vec<Entry>| { let tx = model::immediate(c).unwrap();
        let out = writer::store_csv(&model::fence_versions(&tx, &[]).unwrap(), &seed::cipher(), key, &rows, now()); if out.is_ok() { tx.commit().unwrap(); } out };
    for key in [&k1, &k2] { assert_eq!(csv(key, dust_csv("ETH", "1", "2026-08-20T10:00:00Z")).unwrap().imported, 2); }
    for key in [&k1, &k2] { assert_eq!(csv(key, dust_csv("ETH", "1", "2026-08-20T10:00:00Z")).unwrap().duplicates, 2); }
    // A lone dust leg on account 1 near a second CSV pair's time.
    two.row(c, 0, 3, "SOL", "9", None, Some("dust_lone"), "2026-08-21 10:00:00");
    assert_eq!(csv(&k2, dust_csv("ADA", "2", "2026-08-21T10:00:00Z")).unwrap().imported, 2, "account 1's incomplete pair blocks nothing of account 2");
    assert!(csv(&k1, dust_csv("ADA", "2", "2026-08-21T10:00:00Z")).unwrap_err().0.contains("partial existing CSV dust pair"));
}

/// §6 test 4 (A12 repair): a preview names its account (version 2), and a version-1 manifest is refused with the fresh-preview
/// line, nothing written.
#[test]
fn a_repair_manifest_from_before_accounts_is_refused_and_writes_nothing() {
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    let _lock = claim(c, dir.path());
    let [_, k2] = keys(c, &two, "Exchanges::Binance");
    let fetched = repair::Evidence { inputs: vec![deposit("d9")], coverage: vec![] };
    let version = model::credential_version_by_id(c, k2.id).unwrap().unwrap();
    let mut m = repair::dry_run(c, &seed::cipher(), &k2, &version, now(), &fetched).unwrap();
    assert_eq!((m.version, m.account_id), (repair::MANIFEST_VERSION, two.accounts[1].id));
    let rows = one::<i64>(c, "SELECT count(*) FROM account_transactions");
    m.version = 1;
    let digest = m.digest();
    assert_eq!(repair::ApplySession::new(&m, &digest, &fetched).err().unwrap().0, repair::OLD_MANIFEST);
    assert_eq!(repair::validate(c, &seed::cipher(), &k2, &m, &digest, &fetched).unwrap_err().0, repair::OLD_MANIFEST);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), rows);
    // The version-2 preview applies on account 2, its row relocated.
    m.version = repair::MANIFEST_VERSION;
    let digest = m.digest();
    let mut session = repair::ApplySession::new(&m, &digest, &fetched).unwrap();
    for n in 0..m.batches.len() { session.apply_batch(c, &seed::cipher(), &k2, n).unwrap(); }
    session.verify(c, &seed::cipher(), &k2).unwrap();
    assert_eq!(ids(c, &two.scopes[1]).len(), 1);
    assert!(ids(c, &two.scopes[0]).is_empty());
}

/// §6 test 13: the withheld KuCoin fills keep their rows and content through the hop, each on its (user, exchange)'s
/// number 1 (made where there was none).
#[test]
fn withheld_kucoin_fills_move_to_their_number_one_through_the_hop() {
    use deltabadger::store::{Registry, REGISTRY, RUST_MIGRATIONS};
    const V: &str = "20261120000002";
    let dir = common::v2_install_unmigrated();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&p, now()).unwrap();
    let m = RUST_MIGRATIONS.iter().position(|m| m.version >= V).unwrap();
    let t = REGISTRY.tables.iter().position(|(v, _)| *v >= V).unwrap();
    store::migrate_with(&p, &lock, &Registry { migrations: &RUST_MIGRATIONS[..m], tables: &REGISTRY.tables[..t], retired: REGISTRY.retired }).unwrap();
    let c = Connection::open(&p.primary).unwrap();
    c.execute_batch("PRAGMA foreign_keys = OFF; DELETE FROM exchange_accounts;
        INSERT INTO kucoin_withheld_fills (id, user_id, exchange_id, tx_id, transacted_at, entry, created_at) VALUES
          (4, 1, 2, 'f1', '2026-01-01', '{\"orderId\":\"o\"}', '2026-01-02'), (9, 3, 2, 'f1', '2026-01-03', '{}', '2026-01-04');").unwrap();
    assert_eq!(store::migrate(&p, &lock).unwrap()[0], V);
    let rows: Vec<(i64, i64, i64, String, String, String, String)> = c.prepare(
        "SELECT w.id, a.user_id, a.number, w.tx_id, w.transacted_at, w.entry, w.created_at FROM kucoin_withheld_fills w JOIN exchange_accounts a ON a.id = w.account_id ORDER BY w.id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, [(4, 1, 1, "f1".into(), "2026-01-01".into(), "{\"orderId\":\"o\"}".into(), "2026-01-02".into()),
                      (9, 3, 1, "f1".into(), "2026-01-03".into(), "{}".into(), "2026-01-04".into())]);
    assert!(store::check(&p).unwrap().pending_rust.is_empty());
}

// ---- commit 3: bot orders (§6 test 8) ----

/// Places one order of bot `bot` with venue id `id` (accepted, then recorded as recovery records it); its row.
async fn place(c: &Connection, bot: i64, id: &str, at: chrono::DateTime<chrono::Utc>) -> i64 {
    use deltabadger::engine::{amount::{self, Sizing}, placement::{self, Recovery}, FixedClock};
    use deltabadger::venue::fake::{AddOutcome, FakeVenue};
    let bot = model::load_bot(c, bot).unwrap();
    let ticker = model::ticker_for(c, &bot).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
    let venue = FakeVenue::new().next_add(AddOutcome::Accept(id.into())).order(id, json!({
        "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012", "cost": "60", "oflags": "", "descr": { "ordertype": "market", "price": "0" } }));
    let intent = placement::begin(c, &bot, &plan, &FixedClock(at)).unwrap();
    placement::send(c, &venue, &intent, &FixedClock(at)).await.unwrap();
    assert!(matches!(placement::recover(c, &venue, &model::load_bot(c, bot.id).unwrap(), &FixedClock(at + chrono::Duration::hours(2))).await.unwrap(), Recovery::Recorded(_)));
    c.query_row(&format!("SELECT id FROM transactions WHERE bot_id = ?1 AND {} = ?2", venue_ids::order_id(c, "transactions").unwrap()), params![bot.id, id], |r| r.get(0)).unwrap()
}

/// §6 test 8: account 2 places an order whose venue id account 1's bot already holds: both recorded, each found where its
/// account keeps it (placement's own lookup, a repeat write); a bot that moves accounts lists both of its orders' ids in its
/// order CSV (the web download's and the portable export's one writer).
#[tokio::test(flavor = "current_thread")]
async fn two_accounts_hold_one_order_id_and_a_bot_lists_its_orders_from_both() {
    use common::two_accounts::SHARED_ORDER;
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let t0: chrono::DateTime<chrono::Utc> = "2026-09-30T12:00:00Z".parse().unwrap();
    let rows = [place(c, two.bots[0], SHARED_ORDER, t0).await, place(c, two.bots[1], SHARED_ORDER, t0).await];
    assert_ne!(rows[0], rows[1]);
    assert_eq!(one::<Option<String>>(c, &format!("SELECT external_id FROM transactions WHERE id = {}", rows[1])), None, "account 2's 2.x id is NULL");
    for (n, scope) in two.scopes.iter().enumerate() {
        assert_eq!(venue_ids::order_by_id(c, scope, SHARED_ORDER).unwrap(), Some(rows[n]));
        assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM transactions t WHERE t.exchange_id = {} AND {}", two.seeded.exchange_id, scope.orders("t"))), 1);
    }
    // The same acceptance written again is the row it already has, on either account.
    for (n, bot) in two.bots.iter().enumerate() {
        let bot = model::load_bot(c, *bot).unwrap();
        let ticker = model::ticker_for(c, &bot).unwrap().unwrap();
        let deltabadger::engine::amount::Sizing::Place(plan) = deltabadger::engine::amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000),
            &deltabadger::engine::venue_rules::KRAKEN).unwrap() else { panic!() };
        let tx = c.unchecked_transaction().unwrap();
        assert_eq!(deltabadger::engine::amount::write_accepted_order(&tx, &bot, &plan, SHARED_ORDER, t0, None).unwrap(), rows[n]);
    }
    // Bot 1 moves to account 2 and places again: its CSV lists both venue ids.
    deltabadger::identity::place_bot(c, two.bots[0], two.seeded.user_id, two.seeded.exchange_id, Some(two.accounts[1]), t0).unwrap();
    let moved = place(c, two.bots[0], "after-move", t0 + chrono::Duration::days(1)).await;
    assert_eq!(deltabadger::identity::Account::of_order(c, moved).unwrap(), Some(two.accounts[1]));
    assert_eq!(deltabadger::identity::Account::of_order(c, rows[0]).unwrap(), Some(two.accounts[0]), "the old order stays where it was placed");
    let csv = deltabadger::web::bot::csv::export_csv(c, two.bots[0]).unwrap().unwrap();
    assert!(csv.contains(SHARED_ORDER) && csv.contains("after-move"), "{csv}");
}

// ---- commit 4: links between orders and the ledger (§6 tests 9, 10) ----

/// A bot order of account `n` with venue id `id`, written as placement writes one (its venue id where its account keeps it).
fn order_row(c: &Connection, two: &Two, n: usize, id: &str) -> i64 {
    let scope = &two.scopes[n];
    let row = seed::insert_row(c, &two.seeded, two.bots[n], two.seeded.btc, &json!({"external_status": 2, "status": 0, "amount": "0.001", "created_at": "2026-09-01 10:00:00"}));
    c.execute("UPDATE transactions SET external_id = ?2 WHERE id = ?1", params![row, scope.stored(Some(id))]).unwrap();
    venue_ids::after_order_insert(c, scope, row, Some(id)).unwrap();
    row
}

/// §6 test 10: a Binance fill links to the order of its own account, both as it is stored (`bot_order`) and from stored
/// history (`link_batch`), when both accounts hold one order id.
#[test]
fn binance_fills_link_to_their_own_accounts_order() {
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    let _lock = claim(c, dir.path());
    let orders = [order_row(c, &two, 0, "BTCEUR-77"), order_row(c, &two, 1, "BTCEUR-77")];
    let fill = |id: &str| Entry { quote_currency: Some("EUR".into()), quote_amount: Some(BigDec::parse("50").unwrap()),
        raw: Raw::from_value(json!({"orderId": 77, "id": id.rsplit('-').next().unwrap().parse::<i64>().unwrap()})), ..entry(0, "BTC", "0.001", Some(id), None, "2026-09-01T10:00:05Z") };
    let ks = keys(c, &two, "Exchanges::Binance");
    for (n, key) in ks.iter().enumerate() {
        assert_eq!(store(c, key, vec![fill("BTCEUR-77-1")]).imported, 1);
        let linked: Option<i64> = c.query_row(&format!("SELECT a.transaction_id FROM account_transactions a WHERE {} AND {} = 'BTCEUR-77-1'",
                                                       two.scopes[n].rows("a"), venue_ids::row_id("a")), [], |r| r.get(0)).unwrap();
        assert_eq!(linked, Some(orders[n]), "account {}'s new fill", n + 1);
    }
    // Stored history: two unlinked fills, one per account, linked by the pass.
    for n in 0..2 {
        let row = two.row(c, n, 0, "BTC", "0.002", Some("BTCEUR-77-2"), None, "2026-09-01 10:00:06");
        c.execute("UPDATE account_transactions SET raw_data = ?2 WHERE id = ?1", params![row, json!({"orderId": 77, "id": 2}).to_string()]).unwrap();
    }
    sync::binance_order_links::link_stored(c, &seed::cipher(), now()).unwrap();
    for (n, (scope, order)) in two.scopes.iter().zip(orders).enumerate() {
        let linked: Option<i64> = c.query_row(&format!("SELECT a.transaction_id FROM account_transactions a WHERE {} AND {} = 'BTCEUR-77-2'",
                                                       scope.rows("a"), venue_ids::row_id("a")), [], |r| r.get(0)).unwrap();
        assert_eq!(linked, Some(order), "account {}'s stored fill", n + 1);
    }
}

/// §6 test 9: bot 1 fills order X on account 1, moves to account 2, and account 2 places X too; each account's late fill
/// links to its own order, and what the ledger holds of the old order is read from account 1 only.
#[test]
fn a_delayed_sync_links_each_fill_to_its_own_accounts_order() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let old = order_row(c, &two, 0, "X");
    deltabadger::identity::place_bot(c, two.bots[0], two.seeded.user_id, two.seeded.exchange_id, Some(two.accounts[1]), now()).unwrap();
    let new = order_row(c, &two, 1, "X");
    let ks = keys(c, &two, "Exchanges::Kraken");
    let fill = |size: &str| Entry { raw: Raw::from_value(json!({"orderId": "X", "createdAt": 1, "size": size, "funds": "1", "fee": "0", "feeCurrency": "EUR"})),
                                    ..entry(0, "BTC", size, Some(&format!("fill-{size}")), None, "2026-09-01T10:00:05Z") };
    for (n, size) in [(0, "1"), (1, "2")] { assert_eq!(store(c, &ks[n], vec![Entry { raw: Raw::from_value(json!({"order_id": "X"})), ..fill(size) }]).imported, 1); }
    let link = |n: usize| -> Option<i64> { c.query_row(&format!("SELECT a.transaction_id FROM account_transactions a WHERE {}", two.scopes[n].rows("a")), [], |r| r.get(0)).unwrap() };
    assert_eq!((link(0), link(1)), (Some(old), Some(new)));
    // known_fills of the old order: account 1's fill alone.
    for (n, size) in [(0, "3"), (1, "4")] { assert_eq!(store(c, &ks[n], vec![fill(size)]).imported, 1); }
    let known = deltabadger::engine::reconcile::known_fills(c, &deltabadger::identity::Account::of_order(c, old).unwrap().unwrap(), "X", "Classic").unwrap();
    assert_eq!(known.size, BigDec::parse("3").unwrap());
}

// ---- commit 5: readers (§6 tests 5–7) ----

/// §6 tests 5 and 6: the legs of one group id on both accounts are two groups to every reader, a leg is never priced by
/// the other account's (`Row::counterpart`), and an account 2+'s futures row is borrowed by its relocated id.
#[test]
fn readers_keep_each_accounts_groups_and_ids() {
    use common::two_accounts::SHARED_GROUP;
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    // A coin leg and a cash leg of g1 on account 1; on account 2 only a coin leg, plus another account-2 cash leg there.
    let a = two.row(c, 0, 0, "BTC", "1", Some("a-coin"), Some(SHARED_GROUP), "2026-09-01 10:00:00");
    two.row(c, 0, 1, "EUR", "100", Some("a-cash"), Some(SHARED_GROUP), "2026-09-01 10:00:00");
    let b = two.row(c, 1, 0, "BTC", "1", Some("b-coin"), Some(SHARED_GROUP), "2026-09-01 10:00:00");
    let lone = two.row(c, 1, 0, "ETH", "1", Some("b-eth"), Some("g2"), "2026-09-01 10:00:00");
    two.row(c, 0, 1, "EUR", "7", Some("a-g2-cash"), Some("g2"), "2026-09-01 10:00:00");
    let fut = two.row(c, 1, 4, "USDT", "1", Some("futures-1"), None, "2026-09-02 10:00:00");
    let rows = deltabadger::tracker::rows::load_all(c, two.seeded.user_id).unwrap();
    let group = |id: i64| rows.iter().find(|r| r.id == id).unwrap().group.clone();
    assert_eq!(group(a).as_deref(), Some(SHARED_GROUP), "number 1's group as before");
    assert_ne!(group(a), group(b), "two accounts' legs of one group id are two groups");
    assert!(rows.iter().find(|r| r.id == fut).unwrap().borrowed(), "account 2's futures row is read by its relocated id");
    assert_eq!(rows.iter().find(|r| r.id == b).unwrap().tx_id.as_deref(), Some("b-coin"));
    let price = |id: i64| deltabadger::web::tracker::row::Row::load(c, two.seeded.user_id, id).unwrap().unwrap().counterpart(c).unwrap();
    assert!(price(a).is_some(), "account 1's pair prices its coin leg");
    assert_eq!((price(b), price(lone)), (None, None), "no leg of the other account prices it");
}

/// §6 test 7: a transfer between the two accounts sharing one venue id: both rows stored and linked as any transfer.
#[tokio::test(flavor = "current_thread")]
async fn a_transfer_between_the_two_accounts_is_stored_on_both_and_linked() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [k1, k2] = keys(c, &two, "Exchanges::Kraken");
    assert_eq!(store(c, &k1, vec![entry(5, "BTC", "1", Some("hop-1"), None, "2026-09-01T10:00:00Z")]).imported, 1);
    assert_eq!(store(c, &k2, vec![entry(4, "BTC", "1", Some("hop-1"), None, "2026-09-01T10:05:00Z")]).imported, 1);
    let db = deltabadger::jobs::Db::new(store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary, seed::cipher());
    assert_eq!(sync::ledger::link_transfers(&db, two.seeded.user_id, now()).await.unwrap(), 1);
    let linked: Option<i64> = c.query_row(&format!("SELECT a.linked_transaction_id FROM account_transactions a WHERE {}", two.scopes[0].rows("a")), [], |r| r.get(0)).unwrap();
    assert_eq!(linked, Some(ids(c, &two.scopes[1])[0].0));
}

// ---- commit 6: export and re-import (§6 test 11) ----

fn import(c: &Connection, user: i64, text: &str) -> deltabadger::tracker::import::run::Answer {
    deltabadger::tracker::import::run::import(c, &seed::cipher(), user, "deltabadger", text.as_bytes(), None, None, now()).unwrap()
}

/// §6 test 11 (CSV): the export carries `account` 1/2 and account 2's venue ids; read back into another install with two
/// accounts each row goes to its own; a file without the column goes to number 1; `3` is unrouted as `kraken 3`; `0` or
/// `x` is malformed.
#[test]
fn the_csv_names_each_rows_account_and_a_reimport_routes_by_it() {
    use deltabadger::tracker::import::run::Answer;
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    two.row(c, 0, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
    two.row(c, 1, 4, "BTC", "2", Some(SHARED_TX), None, "2026-09-01 11:00:00");
    let csv = deltabadger::web::tracker::csv_rows(c, &format!("SELECT {} FROM account_transactions ORDER BY transacted_at ASC",
                                                              deltabadger::web::tracker::csv_columns(c).unwrap()), vec![]).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert!(lines[0].ends_with(",description,account"), "{csv}");
    assert!(lines[1].ends_with(&format!(",kraken,{SHARED_TX},,,1")) && lines[2].ends_with(&format!(",kraken,{SHARED_TX},,,2")), "{csv}");
    // Another install, two accounts.
    let (_d2, o2) = install();
    let c2 = &o2.primary;
    let other = Two::kraken(c2, &seed::cipher());
    assert!(matches!(import(c2, other.seeded.user_id, &csv), Answer::Written { imported: 2, .. }));
    for (n, amount) in [(0, 1.0), (1, 2.0)] {
        let rows: Vec<(Option<String>, f64)> = c2.prepare(&format!("SELECT {}, a.base_amount FROM account_transactions a WHERE {}", venue_ids::row_id("a"), other.scopes[n].rows("a"))).unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(rows, [(Some(SHARED_TX.to_string()), amount)], "account {}", n + 1);
    }
    let header = "date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description";
    assert!(matches!(import(c2, other.seeded.user_id, &format!("{header}\n2026-09-02T10:00:00Z,deposit,BTC,5,,,,,kraken,old-file,,\n")), Answer::Written { imported: 1, .. }));
    assert!(venue_ids::known(c2, &other.scopes[0], "old-file").unwrap(), "a 12-column file is number 1's");
    match import(c2, other.seeded.user_id, &format!("{header},account\n2026-09-03T10:00:00Z,deposit,BTC,5,,,,,kraken,third,,,3\n")) {
        Answer::Written { unrouted, .. } => assert_eq!(unrouted, ["kraken 3"]),
        other => panic!("{other:?}"),
    }
    for bad in ["0", "x"] {
        assert!(matches!(import(c2, other.seeded.user_id, &format!("{header},account\n2026-09-03T10:00:00Z,deposit,BTC,5,,,,,kraken,bad,,,{bad}\n")), Answer::Error("malformed")), "{bad}");
    }
}

/// §6 test 11 (portable database): `deltabadger export` of an install with account-2 rows and orders opens as an install
/// with every membership 1:1, both accounts' ids read back through the accessor, and the triggers present.
#[cfg(unix)]
#[test]
fn the_portable_export_keeps_every_membership_and_its_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("storage")).unwrap();
    std::fs::write(root.join(".dockerenv"), "").unwrap();
    let source = common::baseline_install();
    for f in common::INSTALL_FILES { std::fs::copy(source.path().join(f), root.join("storage").join(f)).unwrap(); }
    let members = |c: &Connection| -> Vec<(i64, i64, Option<String>)> {
        c.prepare("SELECT account_transaction_id, account_id, venue_tx_id FROM exchange_account_rows UNION ALL \
                   SELECT transaction_id, account_id, venue_external_id FROM exchange_account_transactions ORDER BY 1, 3").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    let (want, scopes) = {
        let c = store::open(&Paths::from_env(&|_| None, &root.join("storage"))).unwrap().primary;
        let two = Two::kraken(&c, &seed::cipher());
        two.row(&c, 0, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
        two.row(&c, 1, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
        order_row(&c, &two, 1, common::two_accounts::SHARED_ORDER);
        (members(&c), two.scopes)
    };
    assert_eq!(want.len(), 2);
    let scratch = deltabadger::export::Scratch { storage: root.join("storage"), output: root.join("export"), marker: root.join(".dockerenv") };
    deltabadger::export::run(&scratch, &|k| (k == "EXPORT_SCRATCH").then(|| "1".into()), now()).unwrap_or_else(|e| panic!("{}", e.message()));
    let portable = Paths { primary: root.join("export/portable/production.sqlite3"), queue: root.join("export/portable/production_queue.sqlite3") };
    store::check(&portable).unwrap();
    let c = store::open(&portable).unwrap().primary;
    assert_eq!(members(&c), want);
    for scope in scopes { assert!(venue_ids::known(&c, &scope, SHARED_TX).unwrap()); }
    assert!(venue_ids::order_by_id(&c, &scopes[1], common::two_accounts::SHARED_ORDER).unwrap().is_some());
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'exchange_account_%_member'"), 2);
}

// ---- §6 test 4's remaining venues, Bybit's placement time, and the group column (lead's follow-up) ----

/// §6 test 4 (Coinbase): both accounts hold an adjusted fill under one venue id; account 1's sync of the original FILL
/// re-keys account 1's row only, and stores the FILL under the freed id.
#[test]
fn a_coinbase_rekey_on_one_account_leaves_the_others_rows() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Coinbase", "Coinbase");
    let adjusted = json!({"trade_id": "T1", "trade_type": "ADVANCED_TRADE_FILL", "entry_id": "E1"}).to_string();
    for n in 0..2 {
        let row = two.row(c, n, 0, "BTC", "1", Some("T1"), None, "2026-09-01 10:00:00");
        c.execute("UPDATE account_transactions SET raw_data = ?2 WHERE id = ?1", params![row, adjusted]).unwrap();
    }
    let [k1, _] = keys(c, &two, "Exchanges::Coinbase");
    let fill = Entry { raw: Raw::from_value(json!({"trade_id": "T1", "trade_type": "FILL"})), ..entry(0, "BTC", "2", Some("T1"), None, "2026-09-02T10:00:00Z") };
    assert_eq!(store(c, &k1, vec![fill]).imported, 1);
    let mut first: Vec<String> = ids(c, &two.scopes[0]).into_iter().filter_map(|r| r.1).collect();
    first.sort();
    assert_eq!(first, ["E1", "T1"], "account 1's adjusted row re-keyed, its FILL under the freed id");
    assert_eq!(ids(c, &two.scopes[1]).into_iter().map(|r| r.1).collect::<Vec<_>>(), [Some("T1".to_string())], "account 2's row untouched");
}

/// §6 test 4 (Kraken): both accounts hold an internal pair under one refid; each pairs within its own account.
#[tokio::test(flavor = "current_thread")]
async fn kraken_internal_pairs_link_within_each_account() {
    use common::two_accounts::SHARED_GROUP;
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let raw = json!({"type": "earn", "subtype": "allocation"}).to_string();
    let mut legs = vec![];
    for n in 0..2 {
        let wd = two.row(c, n, 5, "BTC", "1", Some(&format!("w{n}")), Some(SHARED_GROUP), "2026-09-01 10:00:00");
        let dep = two.row(c, n, 4, "BTC", "1", Some(&format!("d{n}")), Some(SHARED_GROUP), "2026-09-01 10:01:00");
        for row in [wd, dep] { c.execute("UPDATE account_transactions SET raw_data = ?2 WHERE id = ?1", params![row, raw]).unwrap(); }
        legs.push((wd, dep));
    }
    let db = deltabadger::jobs::Db::new(store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary, seed::cipher());
    assert_eq!(sync::ledger::link_transfers(&db, two.seeded.user_id, now()).await.unwrap(), 2);
    for (wd, dep) in legs {
        assert_eq!(one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {wd}")), Some(dep));
    }
}

/// §6 test 4 (Binance trades): account 1 holds trade ids up to 900 on BTCUSDT, account 2 up to 400, and both hold 400:
/// each account's trade read starts after its own highest id.
#[tokio::test(flavor = "current_thread")]
async fn binance_trade_cursors_are_each_accounts() {
    use deltabadger::venue::http::ScriptedTransport;
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    for (n, id) in [(0, "BTCUSDT-1-900"), (0, "BTCUSDT-1-400"), (1, "BTCUSDT-1-400")] { two.row(c, n, 0, "BTC", "1", Some(id), None, "2026-09-01 10:00:00"); }
    let db = deltabadger::jobs::Db::new(store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary, seed::cipher());
    let r = |body: serde_json::Value| json!([{ "status": 200, "body": body }]);
    for (n, want) in [(0, "901"), (1, "401")] {
        let t = ScriptedTransport::from_script(&json!({
            "GET /sapi/v1/capital/deposit/hisrec": r(json!([])), "GET /sapi/v1/capital/withdraw/history": r(json!([])),
            "GET /api/v3/exchangeInfo": r(json!({"symbols": [{"symbol": "BTCUSDT", "baseAsset": "BTC", "quoteAsset": "USDT", "status": "TRADING"}]})),
            "GET /api/v3/account": r(json!({"balances": []})),
            // The test stops at the first trade read: what it asked for is the point.
            "GET /api/v3/myTrades": [{ "status": 400, "body": {"code": -1100, "msg": "stop"} }] }));
        let (credentials, _) = sync::credentials_with_version(c, &seed::cipher(), two.keys[n]).unwrap();
        let _ = sync::binance_ledger::sync(&db, t.clone(), two.keys[n], &credentials, &deltabadger::engine::FixedClock(now()), sync::binance_ledger::Pacing::NONE).await.unwrap();
        let from: Vec<String> = t.requests().iter().filter(|q| q.path == "/api/v3/myTrades")
            .map(|q| q.query.iter().find(|(k, _)| *k == "fromId").unwrap().1.clone()).collect();
        assert_eq!(from, [want], "account {}", n + 1);
    }
}

fn deposit_tx(id: &str, tx: &str) -> writer::Input {
    let mut i = deposit(id);
    let mut raw = i.entry.raw.value.clone();
    raw["txId"] = json!(tx);
    i.event = Event::read(Source::Deposit, &raw, Context { parent: Some(""), occurred_ms: Some(1786960800000), ..Context::default() }).unwrap();
    i.entry.tx_id = Some(tx.into());
    i.entry.raw = Raw::from_value(raw);
    i
}

/// §6 test 4 (A12 repair): both accounts share a legacy bucket id, each repaired into its own buckets. A repair of account 2
/// stopped after its first batch, then resumed from a version-1 manifest: refused, nothing written, the committed batch's
/// rows and journal kept; a fresh preview then applies the rest.
#[test]
fn a_repair_resumed_from_a_manifest_before_accounts_is_refused_and_a_fresh_preview_finishes_it() {
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    let _lock = claim(c, dir.path());
    let ks = keys(c, &two, "Exchanges::Binance");
    let fetched = repair::Evidence { inputs: vec![deposit_tx("d1", "h1"), deposit_tx("d2", "h2")], coverage: vec![] };
    let plan = |key: &sync::Key| { let version = model::credential_version_by_id(c, key.id).unwrap().unwrap();
                                   repair::dry_run(c, &seed::cipher(), key, &version, now(), &fetched).unwrap() };
    let m = plan(&ks[1]);
    assert_eq!(m.batches.len(), 2);
    repair::ApplySession::new(&m, &m.digest(), &fetched).unwrap().apply_batch(c, &seed::cipher(), &ks[1], 0).unwrap();
    let journal = |c: &Connection| one::<i64>(c, "SELECT count(*) FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:repair:%'");
    let (rows, journaled) = (ids(c, &two.scopes[1]), journal(c));
    assert_eq!((rows.len(), journaled), (1, 1));
    let mut old = m.clone();
    old.version = 1;
    let digest = old.digest();
    assert_eq!(repair::ApplySession::new(&old, &digest, &fetched).err().unwrap().0, repair::OLD_MANIFEST);
    assert_eq!((ids(c, &two.scopes[1]), journal(c)), (rows, journaled), "nothing written; the committed batch stays");
    let fresh = plan(&ks[1]);
    let mut session = repair::ApplySession::new(&fresh, &fresh.digest(), &fetched).unwrap();
    for n in 0..fresh.batches.len() { session.apply_batch(c, &seed::cipher(), &ks[1], n).unwrap(); }
    session.verify(c, &seed::cipher(), &ks[1]).unwrap();
    assert_eq!(ids(c, &two.scopes[1]).len(), 2);
    // Account 1 repairs the same legacy buckets into its own names.
    let first = plan(&ks[0]);
    let mut session = repair::ApplySession::new(&first, &first.digest(), &fetched).unwrap();
    for n in 0..first.batches.len() { session.apply_batch(c, &seed::cipher(), &ks[0], n).unwrap(); }
    session.verify(c, &seed::cipher(), &ks[0]).unwrap();
    assert_eq!(ids(c, &two.scopes[0]).len(), 2);
    let (u, e, a) = (two.seeded.user_id, two.seeded.exchange_id, two.accounts[1].id.unwrap());
    let names: Vec<String> = c.prepare("SELECT key FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(names.iter().filter(|k| k.starts_with(&format!("rust_binance_a12_v1:bucket:{u}:{e}:"))).count(), 2);
    assert_eq!(names.iter().filter(|k| k.starts_with(&format!("rust_binance_a12_v1:bucket:{u}:{e}@{a}:"))).count(), 2);
}

/// Bybit's history fallback reads an order's placement time from the account the venue trades for: both accounts hold
/// one order id, placed a month apart, and each venue walks from its own account's placement.
#[tokio::test(flavor = "current_thread")]
async fn bybit_reads_the_placement_time_of_its_own_accounts_order() {
    use deltabadger::venue::{bybit, http::ScriptedTransport, Venue};
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Bybit", "Bybit");
    let placed = ["2026-09-01 00:00:00", "2026-08-01 00:00:00"];
    for (n, at) in placed.iter().enumerate() {
        let row = order_row(c, &two, n, "old-id");
        c.execute("UPDATE transactions SET created_at = ?2 WHERE id = ?1", params![row, at]).unwrap();
    }
    let raw = json!({"orderId": "old-id", "orderLinkId": "x", "symbol": "BTCUSDT", "orderType": "Market", "side": "Buy", "qty": "0.001", "avgPrice": "60000",
                     "price": "0", "cumExecQty": "0.001", "cumExecValue": "60", "orderStatus": "Filled"});
    let reply = |rows: serde_json::Value| json!({"status": 200, "body": {"retCode": 0, "result": {"list": rows}}});
    for (n, at) in placed.iter().enumerate() {
        let t = ScriptedTransport::from_script(&json!({"GET /v5/order/realtime": [reply(json!([]))], "GET /v5/order/history": [reply(json!([raw.clone()]))]}));
        let v = bybit::BybitVenue::new(t.clone(), bybit::URL).with_history_db(dir.path().join("production.sqlite3"), "2026-10-15T00:00:00Z".parse().unwrap())
            .with_order_scope(two.scopes[n]);
        assert_eq!(v.orders(&["old-id".into()]).await.unwrap()[0].txid, "old-id");
        let history = t.requests().into_iter().find(|r| r.path == bybit::HISTORY_PATH).unwrap();
        let start = history.query.iter().find(|(k, _)| *k == "startTime").unwrap().1.clone();
        let placed_at: chrono::DateTime<chrono::Utc> = deltabadger::codec::parse_time(at).unwrap();
        assert_eq!(start, (placed_at - chrono::Duration::hours(1)).timestamp_millis().to_string(), "account {}", n + 1);
    }
}

/// The group text `<group>@<account>` is the readers' in-memory key only: the CSV (web, API, portable) writes an account-2
/// row's group as the venue gave it.
#[test]
fn an_account_2_rows_group_is_exported_as_the_venue_group() {
    use common::two_accounts::SHARED_GROUP;
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    two.row(c, 1, 0, "BTC", "1", Some("b-coin"), Some(SHARED_GROUP), "2026-09-01 10:00:00");
    let csv = deltabadger::web::tracker::csv_rows(c, &format!("SELECT {} FROM account_transactions ORDER BY transacted_at ASC",
                                                              deltabadger::web::tracker::csv_columns(c).unwrap()), vec![]).unwrap();
    assert!(csv.lines().nth(1).unwrap().ends_with(&format!(",b-coin,{SHARED_GROUP},,2")), "{csv}");
    assert!(!csv.contains('@'), "{csv}");
}
