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
// ---- commit 4: the scope is the account (§6 tests 2-4, 7-10) ----

use deltabadger::tracker::{figures, figures::page, gaps, snapshot, unpriced};
use rusqlite::params;

const DEPOSIT: i64 = 4;
fn rows_of(c: &Connection, two: &Two) -> [AccountRow; 2] { [0, 1].map(|n| AccountRow::of(c, &two.accounts[n]).unwrap().unwrap()) }
fn synced(c: &Connection, key: i64, at: &str) { c.execute("UPDATE api_keys SET balances_synced_at = ?2 WHERE id = ?1", params![key, at]).unwrap(); }
/// A balance row of account `a`: `units` of `asset` at `price`, priced and synced at `at`.
fn hold(c: &Connection, a: &AccountRow, asset: i64, units: &str, price: &str, at: &str) {
    c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at, account_id) \
               VALUES (?1, ?2, ?3, ?4, 0, ?5, ?4 * ?5, ?6, ?6, ?6, ?6, ?7)", params![a.user_id, a.exchange_id, asset, units, price, at, a.id]).unwrap();
}
fn moved(c: &Connection, user: i64, scope: Scope) -> Vec<(String, String)> {
    figures::pending(c, user, &scope).unwrap().into_iter().map(|(s, d)| (s, d.to_s_f())).collect()
}

/// §6 tests 2 and 3: each account's movements are brought forward against its own balance sync. A synced at 12:00 and B at
/// 11:00, then a B deposit at 11:30: "All" and B's scope bring it forward, A's scope has nothing; the history-gap step reads
/// B as pending and A as not. With the syncs the other way round, B's deposit is behind B's own sync.
#[test]
fn each_accounts_movements_are_pending_against_its_own_balance_sync() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [a, b] = rows_of(c, &two);
    let user = two.seeded.user_id;
    synced(c, two.keys[0], "2026-09-20 12:00:00");
    synced(c, two.keys[1], "2026-09-20 11:00:00");
    two.row(c, 1, DEPOSIT, "BTC", "0.5", Some("dep-b"), None, "2026-09-20 11:30:00");
    let btc = vec![("BTC".to_string(), "0.5".to_string())];
    assert_eq!(moved(c, user, Scope::All), btc, "All: B's deposit is past B's own sync");
    assert_eq!(moved(c, user, Scope::Account(b)), btc);
    assert!(moved(c, user, Scope::Account(a)).is_empty(), "A's scope holds none of B's rows");
    assert!(gaps::pending(c, &b).unwrap() && !gaps::pending(c, &a).unwrap());
    // The other order: B synced after its deposit.
    synced(c, two.keys[0], "2026-09-20 11:00:00");
    synced(c, two.keys[1], "2026-09-20 12:00:00");
    assert!(moved(c, user, Scope::All).is_empty() && moved(c, user, Scope::Account(b)).is_empty());
    assert!(!gaps::pending(c, &b).unwrap() && !gaps::pending(c, &a).unwrap());
}

/// §6 test 4: Kraken synced at 10:00 and Coinbase at 10:10, one account each: "All" is partial and its prices stale, as on
/// main; each account alone is neither.
#[test]
fn freshness_is_read_over_the_scopes_accounts() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let coinbase = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Coinbase", "Coinbase");
    let (k, cb) = (rows_of(c, &kraken)[0], rows_of(c, &coinbase)[0]);
    let user = kraken.seeded.user_id;
    hold(c, &k, kraken.seeded.btc, "1", "100", "2026-09-20 10:00:00");
    hold(c, &cb, kraken.seeded.btc, "1", "100", "2026-09-20 10:10:00");
    let partial = |scope: Scope| snapshot::partial(c, user, &figures::balances(c, user, &scope).unwrap()).unwrap();
    let stale = |scope: Scope| page::cold(c, user, &scope, true).unwrap().stale;
    assert!(partial(Scope::All) && stale(Scope::All));
    assert!(!partial(Scope::Account(k)) && !stale(Scope::Account(k)));
    assert!(!partial(Scope::Account(cb)) && !stale(Scope::Account(cb)));
    // §6 test 7: a key of another exchange whose last sync failed still marks Kraken's account partial, as main does.
    c.execute("UPDATE api_keys SET last_sync_error = 'Faraday::TimeoutError' WHERE id = ?1", [coinbase.keys[0]]).unwrap();
    assert!(partial(Scope::Account(k)));
}

/// §6 test 7: each account's scope reads its own balances; "All" reads both. A known exchange the user has no account on,
/// named by an old `exchange_id` link, reads nothing, never "All".
#[test]
fn an_account_scope_reads_its_own_balances_and_an_unknown_exchange_scope_reads_nothing() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [a, b] = rows_of(c, &two);
    let user = two.seeded.user_id;
    hold(c, &a, two.seeded.btc, "1", "100", "2026-09-20 10:00:00");
    hold(c, &b, two.seeded.btc, "2", "100", "2026-09-20 10:00:00");
    let value = |scope: Scope| page::cold(c, user, &scope, true).unwrap().holdings.iter().map(|h| h.value.as_ref().unwrap().to_s_f()).collect::<Vec<_>>();
    assert_eq!(value(Scope::Account(a)), ["100.0"]);
    assert_eq!(value(Scope::Account(b)), ["200.0"]);
    assert_eq!(value(Scope::All), ["300.0"], "one holding of both accounts' BTC");
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Binance', 'Binance', '0.1', '0.1', 'now', 'now')", []).unwrap();
    let binance = c.last_insert_rowid();
    let empty = Scope::of_exchange(c, user, Some(binance)).unwrap();
    assert_eq!(empty, Scope::EmptyLegacyExchange(binance));
    assert!(value(empty).is_empty(), "an exchange the user has no account on reads nothing");
    assert_eq!(Scope::of_exchange(c, user, Some(two.seeded.exchange_id)).unwrap(), Scope::Account(a), "an old link names number 1");
}

/// §6 test 7: a sweep's unpriced record written before the record was keyed by account (v1, by exchange id) reads as the
/// owner's number 1 of that exchange, whose account id is not the exchange id; the next sweep writes v2.
#[test]
fn an_unpriced_record_keyed_by_exchange_reads_as_its_number_one() {
    let (_d, o) = install();
    let c = &o.primary;
    // Account ids apart from exchange ids.
    for n in 0..5 { c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (999, ?1, 1, 'now', 'now')", [100 + n]).unwrap(); }
    let two = Two::kraken(c, &seed::cipher());
    let [a, b] = rows_of(c, &two);
    let user = two.seeded.user_id;
    assert_ne!(a.id, a.exchange_id);
    deltabadger::app_config::set_plain(c, &unpriced::key(user), &json!({"v": 1, "venues": {two.seeded.exchange_id.to_string(): ["XYZ"]}}).to_string(), chrono::Utc::now()).unwrap();
    assert_eq!(unpriced::swept(c, user).unwrap(), unpriced::ByVenue::from([(a.id, ["XYZ".to_string()].into())]));
    let history = |scope: Scope| unpriced::read(c, user, &scope, &[]).unwrap().into_iter().flat_map(|g| g.history).collect::<Vec<_>>();
    assert_eq!(history(Scope::Account(a)), ["XYZ"]);
    assert!(history(Scope::Account(b)).is_empty());
    unpriced::store(c, user, &unpriced::ByVenue::from([(b.id, ["ABC".to_string()].into())]), chrono::Utc::now()).unwrap();
    let raw: String = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [unpriced::key(user)], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&raw).unwrap()["v"], 2);
    assert_eq!(history(Scope::Account(b)), ["ABC"]);
}

// ---- §6 tests 8-10: chips, the scope parameter, REST/MCP, history-gap lines ----

/// The app over `dir`'s install, a market-data provider configured (the populated page of a venue other than Alpaca needs one).
fn web_app(dir: &std::path::Path, c: &Connection) -> deltabadger::web::App {
    c.execute("UPDATE users SET setup_completed = 1, confirmed_at = '2026-01-01'", []).unwrap();
    let app = common::web::app(dir, "engine-test-secret", common::web::TestClock::at("2026-09-20T12:00:00Z"));
    for (key, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(c, &app.cipher, key, value, app.now()).unwrap();
    }
    app
}
async fn get(app: &deltabadger::web::App, c: &Connection, owner: i64, path: &str) -> common::web::Answer {
    use deltabadger::web::session;
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
    let data = session::SessionData { user: Some((owner, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    let mut browser = common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    browser.send(app, "GET", path, None, common::web::Csrf::None, &[]).await
}
/// The chip of the switch whose `data-value` is `value`, as rendered.
fn chip<'a>(body: &'a str, value: &str) -> Option<&'a str> {
    let at = body.find(&format!("data-value=\"{value}\""))?;
    let start = body[..at].rfind("<a ")?;
    Some(&body[start..start + body[start..].find("</a>")?])
}

/// §6 tests 8 and 9: two accounts of one exchange are two chips, each named by its account (slice 6); `?exchange_id=`
/// names number 1 and `?account_id=` account 2; a foreign account id is a 404; account 2's CSV export holds its rows only.
/// REST/MCP keep `exchange_id` exchange-wide: the rows of both accounts.
#[tokio::test(flavor = "current_thread")]
async fn each_account_is_a_chip_and_its_own_scope() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [a, b] = rows_of(c, &two);
    let (user, exchange) = (two.seeded.user_id, two.seeded.exchange_id);
    two.row(c, 0, DEPOSIT, "BTC", "1", Some("t-a"), None, "2026-09-10 10:00:00");
    two.row(c, 1, DEPOSIT, "BTC", "2", Some("t-b"), None, "2026-09-11 10:00:00");
    let app = web_app(dir.path(), c);
    let page = get(&app, c, user, "/tracker").await;
    assert_eq!(page.status, 200, "{}", &page.body[..page.body.len().min(400)]);
    let (first, second) = (chip(&page.body, &exchange.to_string()).expect("number 1's chip"), chip(&page.body, &format!("a{}", b.id)).expect("account 2's chip"));
    assert!(first.ends_with(">Kraken<sup>1</sup>") && second.ends_with(">Kraken<sup>2</sup>"), "slice 6 names each account: {first}\n{second}");
    assert!(first.contains(&format!("exchange_id={exchange}")) && second.contains(&format!("account_id={}", b.id)));
    let on = |body: &str, value: &str| chip(body, value).is_some_and(|c| c.contains("is-on"));
    let old = get(&app, c, user, &format!("/tracker?exchange_id={exchange}")).await.body;
    assert!(on(&old, &exchange.to_string()) && !on(&old, &format!("a{}", b.id)), "an old link names number 1");
    let second_page = get(&app, c, user, &format!("/tracker?account_id={}", b.id)).await.body;
    assert!(on(&second_page, &format!("a{}", b.id)) && !on(&second_page, &exchange.to_string()));
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1 + 100, ?2, 1, 'now', 'now')", [user, exchange]).unwrap();
    let foreign = c.last_insert_rowid();
    assert_eq!(get(&app, c, user, &format!("/tracker?account_id={foreign}")).await.status, 404, "another user's account");
    assert_eq!(get(&app, c, user, "/tracker?account_id=999999").await.status, 404, "no such account");
    let csv = get(&app, c, user, &format!("/tracker/export?account_id={}", b.id)).await.body;
    assert!(csv.contains("t-b") && !csv.contains("t-a"), "{csv}");
    let csv = get(&app, c, user, &format!("/tracker/export?exchange_id={exchange}")).await.body;
    assert!(csv.contains("t-a") && !csv.contains("t-b"), "an old link exports number 1's: {csv}");
    let _ = a;
    let listed = deltabadger::web::tracker::api::mcp(c, &app, user, "list_account_transactions", &json!({"exchange_id": exchange})).unwrap();
    assert!(listed.to_string().contains("Account transactions (2)"), "{listed}");
}

/// §6 test 10: each account's gap is judged and retired against its own balances. A synced at T1, B at T2 > T1; A's rows
/// change after B's sync (counts unchanged, still older than B's): A's line retires, B's stays (an exchange-wide maximum
/// would hide A's change). A number 1's line keeps its exchange's markup; account 2's names its account.
#[test]
fn each_accounts_history_gap_retires_on_its_own_balances() {
    use deltabadger::tracker::cache::Fingerprint;
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [a, b] = rows_of(c, &two);
    let (user, exchange) = (two.seeded.user_id, two.seeded.exchange_id);
    hold(c, &a, two.seeded.btc, "1", "100", "2026-09-20 10:00:00");
    hold(c, &b, two.seeded.btc, "1", "100", "2026-09-20 12:00:00");
    let mut origins = vec![];
    for (account, at) in [(&a, "2026-09-20T10:00:00Z"), (&b, "2026-09-20T12:00:00Z")] {
        deltabadger::app_config::set_plain(c, &cache::balance_origin_key_of(account), "{}", at.parse().unwrap()).unwrap();
        origins.push(c.query_row("SELECT updated_at FROM app_configs WHERE key = ?1", [cache::balance_origin_key_of(account)], |r| r.get::<_, String>(0)).unwrap());
    }
    let from = Fingerprint::from_json(&json!({"history": "1_2_x", "prices": 0, "expires": "2026-10-31T02:00:00+00:00", "producers": []}));
    let line = |key: i64, origin: &str, newest: &str| json!({"exchange": exchange, "key": key, "balances": {"origin_updated_at": origin, "rows": 1, "max_updated_at": newest},
        "arrived_usd": "20.0", "notes": [{"kind": "arrived", "symbol": "ETH", "exchange": "Kraken", "history": "0.0", "held": "1.0", "amount": "20.0"}]});
    let mut second = line(two.keys[1], &origins[1], "2026-09-20 12:00:00");
    second["account"] = json!(b.id);
    let row = json!({"v": 1, "from": from.to_json(), "gaps": [line(two.keys[0], &origins[0], "2026-09-20 10:00:00"), second]});
    deltabadger::app_config::set_plain(c, &gaps::key(user), &row.to_string(), chrono::Utc::now()).unwrap();
    let reading = deltabadger::tracker::import::run::reading(c, user).unwrap();
    let read = || gaps::read(c, user, &Scope::All, Some(&from), Some(&reading)).unwrap();
    let found = read();
    assert_eq!(found.iter().map(|g| g.account.id).collect::<Vec<_>>(), [a.id, b.id]);
    let anchors: Vec<(String, String)> = found.iter().map(deltabadger::web::tracker::row::gap_anchor).collect();
    assert_eq!(anchors, [(exchange.to_string(), format!("exchange_id={exchange}")), (format!("a{}", b.id), format!("account_id={}", b.id))]);
    assert_eq!(gaps::read(c, user, &Scope::Account(b), Some(&from), Some(&reading)).unwrap().len(), 1, "B's scope reads B's line");
    // A's balances change after B's sync, still older than B's newest: A's line retires, B's stays.
    c.execute("UPDATE exchange_account_balances SET updated_at = '2026-09-20 11:00:00' WHERE account_id = ?1", [a.id]).unwrap();
    assert_eq!(read().iter().map(|g| g.account.id).collect::<Vec<_>>(), [b.id]);
}

// ---- commit 5: KuCoin HF coverage by account (§6 test 11) ----

/// §6 test 11: KuCoin HF coverage is each account's. A's `hf_enabled` does not enable B; A's export certifies A's history
/// only, so B's own unread months stay a gap; B, whose history starts in June and is certified from June, has no gap for
/// the months A's history covers before it.
#[test]
fn kucoin_hf_coverage_is_each_accounts_own() {
    use deltabadger::sync::kucoin_hf::{self as hf, Span};
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    let [a, b] = [0, 1].map(|n| deltabadger::sync::load_key_of(c, kucoin.keys[n], &[deltabadger::venue::kucoin::KUCOIN]).unwrap());
    let t = |s: &str| -> chrono::DateTime<chrono::Utc> { format!("{s}T00:00:00Z").parse().unwrap() };
    let now = t("2026-12-01");
    let span = |x: &str, y: &str| -> Span { (t(x), t(y)) };
    hf::add(c, &a, hf::ENABLED, span("2026-01-01", "2026-01-01"), now).unwrap();
    assert!(hf::enabled(c, &a.scope).unwrap() && !hf::enabled(c, &b.scope).unwrap(), "A's marker is A's");
    // Both read HF units from January; B's are certified from June only; A exported January to June.
    for key in [&a, &b] { hf::add(c, key, hf::HF_ATTEMPT, span("2026-01-01", "2026-12-01"), now).unwrap(); }
    hf::add(c, &b, hf::HF_API, span("2026-06-01", "2026-12-01"), now).unwrap();
    hf::add(c, &a, hf::EXPORT, span("2026-01-01", "2026-12-01"), now).unwrap();
    kucoin.row(c, 0, DEPOSIT, "BTC", "1", Some("k-a"), None, "2026-01-05 00:00:00");
    kucoin.row(c, 1, DEPOSIT, "BTC", "1", Some("k-b"), None, "2026-01-06 00:00:00");
    assert!(hf::missing(c, &a).unwrap().is_empty(), "A's export covers A");
    assert_eq!(hf::missing(c, &b).unwrap(), [span("2026-01-01", "2026-06-01")], "A's export certifies nothing of B's");
    // B's history starts in June: nothing before it is B's to read, though A's starts in January.
    c.execute("UPDATE account_transactions SET transacted_at = '2026-06-02 00:00:00' WHERE id IN (SELECT account_transaction_id FROM exchange_account_rows)", []).unwrap();
    c.execute(&format!("DELETE FROM kucoin_coverage WHERE account_id = {} AND source = ?1", b.scope.account_sql()), [hf::HF_ATTEMPT]).unwrap();
    hf::add(c, &b, hf::HF_ATTEMPT, span("2026-06-01", "2026-12-01"), now).unwrap();
    hf::remove(c, &a, hf::EXPORT).unwrap();
    assert!(hf::missing(c, &b).unwrap().is_empty(), "no January-May gap for B");
    assert_eq!(hf::missing(c, &a).unwrap(), [span("2026-01-01", "2026-12-01")], "A's own months, A's own gap");
}

// ---- an install no migration has given the stores (store::open lets every command but run/serve open one) ----

/// An unmigrated 2.x install reads its balances from 2.x's own table, through `balance_store::fallback`'s views: the whole
/// portfolio, an old `exchange_id` link's scope and the served tracker page show what 2.x's rows hold, and nothing is
/// written to the file; a write to a view refuses.
#[tokio::test(flavor = "current_thread")]
async fn an_unmigrated_install_reads_its_balances_from_2xs_table() {
    let dir = common::v2_install_unmigrated();
    let path = dir.path().join("production.sqlite3");
    let raw = Connection::open(&path).unwrap();
    let s = seed::seed_kraken(&raw, &seed::cipher());
    raw.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 2, 0, 100, 200, '2026-09-20 10:00:00', '2026-09-20 10:00:00', 'c', 'u')", [s.user_id, s.exchange_id, s.btc]).unwrap();
    drop(raw);
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let c = &o.primary;
    assert!(!deltabadger::sync::balance_store::ready(c).unwrap(), "no store on the file");
    let value = |scope: Scope| page::cold(c, s.user_id, &scope, true).unwrap().holdings.iter().map(|h| h.value.as_ref().unwrap().to_s_f()).collect::<Vec<_>>();
    assert_eq!(value(Scope::All), ["200.0"]);
    let old = Scope::of_exchange(c, s.user_id, Some(s.exchange_id)).unwrap();
    assert_eq!(old, Scope::Account(AccountRow::unplaced(s.user_id, s.exchange_id)));
    assert_eq!(value(old), ["200.0"], "an exchange's one account reads its rows");
    assert_eq!(figures::balances(c, s.user_id, &old).unwrap().len(), 1);
    // A write through the view lands in 2.x's table and nothing else; an account the install cannot have aborts.
    let id: i64 = one(c, "SELECT id FROM account_balances");
    c.execute("UPDATE exchange_account_balances SET free = 3 WHERE id = ?1", [id]).unwrap();
    assert_eq!(one::<f64>(c, "SELECT free FROM account_balances"), 3.0);
    c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, synced_at, created_at, updated_at, account_id) VALUES (?1, ?2, ?3, 's', 'c', 'u', ?4)",
              [s.user_id, s.exchange_id, s.quote, -s.exchange_id]).unwrap();
    assert_eq!(c.query_row("SELECT free FROM account_balances WHERE asset_id = ?1", [s.quote], |r| r.get::<_, f64>(0)).unwrap(), 0.0, "2.x's default");
    assert!(c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, synced_at, created_at, updated_at, account_id) VALUES (?1, ?2, ?3, 's', 'c', 'u', 99)",
                      [s.user_id, s.exchange_id, s.ticker_id]).is_err(), "an account 2+ aborts");
    assert!(c.execute("UPDATE exchange_account_balances SET account_id = 99 WHERE id = ?1", [id]).is_err(), "an account 2+ aborts");
    c.execute("DELETE FROM exchange_account_balances WHERE id = ?1", [id]).unwrap();
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_balances"), 1);
    c.execute("INSERT INTO exchange_account_snapshots (user_id, exchange_id, date, created_at, updated_at, account_id) VALUES (?1, ?2, '2026-09-20', 'c', 'u', ?3)",
              [s.user_id, s.exchange_id, -s.exchange_id]).unwrap();
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM portfolio_venue_snapshots WHERE value_usd = 0 AND partial = 0"), 1);
    c.execute("DELETE FROM exchange_account_snapshots", []).unwrap();
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM portfolio_venue_snapshots"), 0);
    let app = web_app(dir.path(), &Connection::open(&path).unwrap());
    let page = get(&app, c, s.user_id, "/tracker").await;
    assert_eq!(page.status, 200, "{}", &page.body[..page.body.len().min(400)]);
    let tables: i64 = Connection::open(&path).unwrap().query_row("SELECT count(*) FROM sqlite_master WHERE name LIKE 'exchange_account_%'", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0, "nothing written to the file");
}

/// Codex r2 P2: `sync balances` opens without migrating (store::open); on an install with slice 5 pending its writes
/// go through the fallback views into 2.x's table, as before the stores.
#[tokio::test(flavor = "current_thread")]
async fn a_balance_sync_on_an_unmigrated_install_writes_2xs_table() {
    let dir = common::v2_install_unmigrated();
    let raw = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let s = seed::seed_kraken(&raw, &seed::cipher());
    raw.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, 'now', 'now')", [s.btc, s.exchange_id]).unwrap();
    raw.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, synced_at, created_at, updated_at) VALUES (?1, ?2, ?3, 9, 0, 's', 'c', 'u')",
                [s.user_id, s.exchange_id, s.quote]).unwrap();
    drop(raw);
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let db = Db::new(o.primary, seed::cipher());
    let key = Credentials { ibkr: None, redaction_values: vec![], key: "test-key".into(), secret: "dGVzdC1zZWNyZXQ=".into(), passphrase: None };
    balances::sync_kraken(&db, answer(json!({ "XBT": { "balance": "0.5", "hold_trade": "0" } })), &NoPrices, s.api_key_id, &key, &FixedClock(NOW.parse().unwrap())).await.unwrap().unwrap();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let held: Vec<(i64, String)> = c.prepare("SELECT asset_id, CAST(free AS TEXT) FROM account_balances").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(held, [(s.btc, "0.5".to_string())], "BTC written, the gone EUR row deleted, in 2.x's table");
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM sqlite_master WHERE name LIKE 'exchange_account_balances%' OR name LIKE 'exchange_account_snapshots%'"), 0, "no store on the file");
}

/// Codex r1 P2: a stand-in's id (minus the exchange id) is every user's on that exchange, so its reads keep the owner. On
/// an unmigrated install user A holds Kraken balances; user B has only a pending Kraken key: B has nothing stale.
#[test]
fn a_stand_ins_balances_are_its_owners_only() {
    let dir = common::v2_install_unmigrated();
    let raw = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let s = seed::seed_kraken(&raw, &seed::cipher());
    raw.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 2, 0, 100, 200, 'p', 's', 'c', 'u')", [s.user_id, s.exchange_id, s.btc]).unwrap();
    raw.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('b@example.com', 'x', 'B', 0, 'now', 'now')", []).unwrap();
    let b = raw.last_insert_rowid();
    raw.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k', 's', 0, 0, 'now', 'now')", [b, s.exchange_id]).unwrap();
    drop(raw);
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let c = &o.primary;
    assert!(!cache::stale(c, b, &Scope::All).unwrap(), "A's balances are not B's");
    let b_scope = Scope::of_exchange(c, b, Some(s.exchange_id)).unwrap();
    assert!(page::cold(c, b, &b_scope, true).unwrap().holdings.is_empty(), "B's scope holds none of A's balances");
}
