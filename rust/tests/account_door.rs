//! #710 slice 7a (PLAN-710-7 §1, §6): where a saved key goes. Every save routes through `identity::route_key`, proven
//! through the tracker's key form (a venue tile posts `untargeted`, a Reconnect its `account_id`, a 2.x form neither).
mod common;
use common::seed;
use deltabadger::identity;
use deltabadger::store::{self, Paths};
use rusqlite::Connection;
use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};

fn install() -> (tempfile::TempDir, store::Opened) {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o)
}
fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T { c.query_row(sql, [], |r| r.get(0)).unwrap() }
fn web_app(dir: &std::path::Path, c: &Connection) -> deltabadger::web::App {
    c.execute("UPDATE users SET setup_completed = 1, confirmed_at = '2026-01-01'", []).unwrap();
    let app = common::web::app(dir, "engine-test-secret", common::web::TestClock::at("2026-09-20T12:00:00Z"));
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    app
}
fn browser(app: &deltabadger::web::App, c: &Connection, owner: i64) -> common::web::Browser {
    use deltabadger::web::session;
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
    let data = session::SessionData { user: Some((owner, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None }
}
struct Quiet;
impl deltabadger::web::settings::keys::Logger for Quiet { fn warn(&self, _: &str) {} }

/// The tracker form's save: `exchange`, a reading key, `target` (`Some(id)`: a Reconnect of that account; None: a tile).
async fn save(app: &deltabadger::web::App, b: &mut common::web::Browser, exchange: i64, target: Option<i64>, fields: &[(&str, &str)]) -> common::web::Answer {
    let exchange = exchange.to_string();
    let target = target.map(|t| t.to_string());
    let mut form = vec![("exchange_id", exchange.as_str()), ("key_type", "read_only")];
    match &target { Some(t) => form.push(("account_id", t.as_str())), None => form.push(("untargeted", "1")) }
    form.extend_from_slice(fields);
    assert_eq!(b.get(app, "/settings/api_keys").await.status, 200);
    b.send(app, "POST", "/tracker/add_api_key", Some(&form), common::web::Csrf::Header, &[]).await
}
/// (account number, decrypted key) of every key of `exchange`, by id.
fn keys(c: &Connection, cipher: &deltabadger::crypto::Cipher, exchange: i64) -> Vec<(i64, String)> {
    let mut s = c.prepare("SELECT k.id, k.key FROM api_keys k WHERE k.exchange_id = ?1 ORDER BY k.id").unwrap();
    let rows: Vec<(i64, String)> = s.query_map([exchange], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    rows.into_iter().map(|(id, key)| {
        let account = identity::account_of_key(c, id).unwrap().unwrap();
        (one::<i64>(c, &format!("SELECT number FROM exchange_accounts WHERE id = {account}")), cipher.decrypt(&key).unwrap())
    }).collect()
}
fn numbers(c: &Connection, exchange: i64) -> Vec<i64> {
    c.prepare("SELECT number FROM exchange_accounts WHERE exchange_id = ?1 ORDER BY number").unwrap().query_map([exchange], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

// ---- no identity venue (Kraken): material decides (§1.1 rule 4) ----

/// Kraken's fixture with its key server: every key's permission read answers a reading key's permissions.
async fn kraken() -> (tempfile::TempDir, store::Opened, seed::Seeded, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/0/private/GetApiKeyInfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"error": [], "result": {"permissions": ["query-funds", "query-ledger"]}}))).mount(&server).await;
    let (dir, o) = install();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    identity::Account::first(&o.primary, s.user_id, s.exchange_id).unwrap().ensure(&o.primary, chrono::Utc::now()).unwrap();
    let app = web_app(dir.path(), &o.primary).with_venue_key_url("Exchanges::Kraken", &server.uri()).unwrap();
    (dir, o, s, server, app)
}

// ---- Alpaca (#710 slice 9: an identity venue, `id` of its account read) ----

/// The Alpaca fixture. Its account read names account A for every key (`PKTEST`, the seeded one, included) but
/// `second-key` and `reader-2`, which name B (`common::alpaca_key`).
async fn alpaca() -> (tempfile::TempDir, store::Opened, seed::Seeded, MockServer, deltabadger::web::App) {
    use common::alpaca_key::{account, ACCOUNT_A, ACCOUNT_B};
    let server = MockServer::start().await;
    for key in ["second-key", "reader-2"] {
        Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID", key)).respond_with(ResponseTemplate::new(200).set_body_json(account(ACCOUNT_B))).with_priority(1).mount(&server).await;
    }
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(account(ACCOUNT_A))).mount(&server).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let (dir, o) = install();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    identity::Account::first(&o.primary, s.user_id, s.exchange_id).unwrap().ensure(&o.primary, chrono::Utc::now()).unwrap();
    let app = web_app(dir.path(), &o.primary).with_settings_key_boundary(deltabadger::venue::alpaca::Hosts::all(&server.uri()), std::sync::Arc::new(Quiet)).unwrap();
    (dir, o, s, server, app)
}

/// A tile save with new material makes account 2; the same material again finds it (no 3); a Reconnect of 1 with 2's exact
/// key and secret, a leftover passphrase too, saves to 2 and leaves 1 as it was, saying so.
#[tokio::test(flavor = "current_thread")]
async fn a_tile_save_without_identity_makes_a_new_account_and_the_same_material_finds_it() {
    let (_d, o, s, _server, app) = kraken().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let before = keys(c, &cipher, s.exchange_id);
    let mut b = browser(&app, c, s.user_id);
    let first = save(&app, &mut b, s.exchange_id, None, &[("api_key[key]", "second-key"), ("api_key[secret]", "c2Vjb25kLXNlY3JldA==")]).await;
    assert!(first.status < 400, "{}: {}", first.status, first.body);
    assert_eq!(numbers(c, s.exchange_id), [1, 2]);
    let again = save(&app, &mut b, s.exchange_id, None, &[("api_key[key]", "second-key"), ("api_key[secret]", "c2Vjb25kLXNlY3JldA==")]).await;
    assert!(again.status < 400, "{}: {}", again.status, again.body);
    assert_eq!(numbers(c, s.exchange_id), [1, 2], "the same material is the same account");
    let account1 = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let reconnect = save(&app, &mut b, s.exchange_id, Some(account1), &[("api_key[key]", "second-key"), ("api_key[secret]", "c2Vjb25kLXNlY3JldA=="), ("api_key[passphrase]", "paper")]).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    let mut want = before.clone();
    want.push((2, "second-key".into()));
    assert_eq!(keys(c, &cipher, s.exchange_id), want, "account 1 keeps its key");
    let notice = b.get(&app, "/tracker").await;
    assert!(notice.body.contains("This key belongs to Kraken") && notice.body.contains("was not changed"), "{}", &notice.body[..notice.body.len().min(2000)]);
}

/// A 2.x form (no `account_id`, no `untargeted`) still saves into number 1, and again replaces that key, as before.
#[tokio::test(flavor = "current_thread")]
async fn a_legacy_form_replaces_number_1s_key() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    // D-ALPACA-ONE-KEY: Alpaca's tracker key is number 1's trading key, so the form replaces that one.
    let mut want = keys(c, &cipher, s.exchange_id);
    assert_eq!(want, [(1, "PKTEST".to_string())]);
    want[0].1 = "replaced".into();
    let mut b = browser(&app, c, s.user_id);
    let exchange = s.exchange_id.to_string();
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let answer = b.send(&app, "POST", "/tracker/add_api_key", Some(&[("exchange_id", exchange.as_str()), ("key_type", "read_only"), ("api_key[key]", "replaced"), ("api_key[secret]", "legacy-secret"), ("api_key[passphrase]", "paper")]),
                        common::web::Csrf::Header, &[]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), want, "number 1's reading key");
    let again = b.send(&app, "POST", "/tracker/add_api_key", Some(&[("exchange_id", exchange.as_str()), ("key_type", "read_only"), ("api_key[key]", "replaced-2"), ("api_key[secret]", "legacy-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert!(again.status < 400, "{}: {}", again.status, again.body);
    want.last_mut().unwrap().1 = "replaced-2".into();
    assert_eq!(keys(c, &cipher, s.exchange_id), want, "replaced in place, as before");
}

/// A posted account of another user or exchange, or a removed one, is a 404 and saves nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_forged_or_merged_target_is_a_404() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let before = keys(c, &cipher, s.exchange_id);
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1 + 100, ?2, 1, 'now', 'now')", [s.user_id, s.exchange_id]).unwrap();
    let foreign = c.last_insert_rowid();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, removed_at, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now', 'now')", [s.user_id, s.exchange_id]).unwrap();
    let removed = c.last_insert_rowid();
    let mut b = browser(&app, c, s.user_id);
    for target in [foreign, removed, 999_999] {
        let answer = save(&app, &mut b, s.exchange_id, Some(target), &[("api_key[key]", "x"), ("api_key[secret]", "y")]).await;
        assert_eq!(answer.status, 404, "{target}: {}", answer.body);
    }
    assert_eq!(keys(c, &cipher, s.exchange_id), before);
}

// ---- an identity venue (KuCoin): the account read decides (§1.1 rules 1-3) ----

const UID_A: &str = "165111215";
const UID_B: &str = "999000111";
const UID_C: &str = "555000333";

/// A KuCoin exchange of the Alpaca fixture's user, its key server answering each key's account: `key-a` reads A, `key-b`, `key-b2`, `key-b3`
/// read B, `key-c` C, any other key's read fails (a 503).
async fn kucoin() -> (tempfile::TempDir, store::Opened, i64, i64, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    for (key, uid) in [("key-a", UID_A), ("key-b", UID_B), ("key-b2", UID_B), ("key-b3", UID_B), ("key-c", UID_C)] {
        Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", key))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": uid, "permission":"General,Spot"}}))).mount(&server).await;
    }
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    Mock::given(method("GET")).and(path(deltabadger::venue::kucoin_account::MODE)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"selfAccountMode":"CLASSIC"}}))).mount(&server).await;
    Mock::given(method("GET")).and(path(deltabadger::venue::kucoin_account::OPENED)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":false}))).mount(&server).await;
    Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"400100","msg":"order not exist"}))).mount(&server).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":[]}))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let s = seed::seed_alpaca(c, &seed::cipher());
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, 'KuCoin', '0.1', '0.1', 'now', 'now')", [deltabadger::venue::kucoin::KUCOIN]).unwrap();
    let exchange = c.last_insert_rowid();
    let app = web_app(dir.path(), c).with_venue_key_url(deltabadger::venue::kucoin::KUCOIN, &server.uri()).unwrap();
    (dir, o, s.user_id, exchange, server, app)
}
fn kucoin_fields(key: &'static str) -> [(&'static str, &'static str); 3] { [("api_key[key]", key), ("api_key[secret]", "secret"), ("api_key[passphrase]", "phrase")] }

/// A failed identity read on a first-ever venue tile is refused (retryable) and makes no account.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_identity_read_on_a_tile_is_refused() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, None, &kucoin_fields("key-fail")).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(answer.body.contains("We could not confirm which account this key belongs to"), "{}", answer.body);
    assert_eq!((numbers(c, exchange), one::<i64>(c, &format!("SELECT count(*) FROM api_keys WHERE exchange_id = {exchange}"))), (vec![], 0));
}

/// Two tile saves of keys of two accounts make two accounts; the second key saved again goes to the second; a Reconnect of
/// account 1 with account 2's key saves to 2, account 1 untouched; then account 1's key read failing, a Reconnect of 1 with
/// 2's key is refused and the retry that reads it saves to 2.
#[tokio::test(flavor = "current_thread")]
async fn the_account_read_routes_each_save() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b", "key-b"] {
        let answer = save(&app, &mut b, exchange, None, &kucoin_fields(key)).await;
        assert!(answer.status < 400, "{key}: {}: {}", answer.status, answer.body);
    }
    assert_eq!(numbers(c, exchange), [1, 2]);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())]);
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let reconnect = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b")).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())], "account 1 untouched");
    // Account 2's key read failing once: a Reconnect of 1 with it is refused, account 1's key and bots untouched; the retry
    // that reads it saves to 2.
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-b")).respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1).with_priority(1).mount(&server).await;
    let failed = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b")).await;
    assert_eq!(failed.status, 422, "{}", failed.body);
    assert!(failed.body.contains("We could not confirm which account this key belongs to"), "{}", failed.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())]);
    let retried = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b")).await;
    assert!(retried.status < 400, "{}: {}", retried.status, retried.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())]);
}

/// After a reset (every fingerprint cleared), a Reconnect of account 2 with a fresh key stays account 2.
#[tokio::test(flavor = "current_thread")]
async fn after_a_reset_a_reconnect_of_account_2_stays_there() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    c.execute_batch("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL; DELETE FROM exchange_account_credentials;").unwrap();
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let answer = save(&app, &mut b, exchange, Some(accounts[1]), &kucoin_fields("key-b")).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(numbers(c, exchange), [1, 2]);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())]);
}

/// Two concurrent saves of one key make one account.
#[tokio::test(flavor = "current_thread")]
async fn two_concurrent_saves_of_one_key_make_one_account() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let (mut b1, mut b2) = (browser(&app, c, user), browser(&app, c, user));
    let fields = kucoin_fields("key-a");
    let (x, y) = tokio::join!(save(&app, &mut b1, exchange, None, &fields), save(&app, &mut b2, exchange, None, &fields));
    // One saves; the other saves the same (a no-op) or, its validation overlapping the first's KuCoin mode read, is refused
    // to be retried. Either way, one account and one key.
    assert!(x.status < 400 || y.status < 400, "{} {} / {} {}", x.status, x.body, y.status, y.body);
    assert_eq!(numbers(c, exchange), [1]);
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM api_keys WHERE exchange_id = {exchange}")), 1);
}

/// §0.10 with §9.1: account 1's key reads account 2's id (a key of B saved on A before 7a); a Reconnect that routes that
/// key to B leaves A's copy fenced (no sync reads with it) until A gets a key that reads A.
#[tokio::test(flavor = "current_thread")]
async fn a_key_routed_to_another_account_stays_fenced_on_its_old_one() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    // Before 7a, A's key was replaced in place by B's material (2.x's form): A's key now reads B.
    let a_key = identity::key_of(c, accounts[0], 2).unwrap().unwrap();
    identity::write_credential(c, &cipher, a_key, Some(&cipher.encrypt("key-b")), Some(&cipher.encrypt("secret")), Some(&cipher.encrypt("phrase")), chrono::Utc::now()).unwrap();
    let reconnect = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b")).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    assert!(!deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&a_key), "A's copy of B's key is fenced");
    let fixed = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-a")).await;
    assert!(fixed.status < 400, "{}: {}", fixed.status, fixed.body);
    assert!(deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&a_key), "a key that reads A lifts it");
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())]);
}

/// §1.1: a routed destination that holds a key is checked as Merge checks it. Account 2 has a placed standalone order not
/// found terminal; a Reconnect of account 1 with a fresh key of account 2 is refused (to retry once it settles), account
/// 2's key unchanged, so the order's cancel never goes through a key it was not placed with.
#[tokio::test(flavor = "current_thread")]
async fn a_routed_replacement_waits_for_the_destination_s_open_standalone_order() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, created_at, send_by) \
               VALUES (?1, NULL, 'buy', '10', 'quote', '{}', 'done', '{\"kind\":\"placed\"}', '2026-09-20 10:00:00', '2026-09-20 10:01:00')", [user]).unwrap();
    let order = c.last_insert_rowid();
    c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
               VALUES (?1, ?2, ?3, 1, 'limit', 'rest', 'ord-1', '{}')", rusqlite::params![order, accounts[1], exchange]).unwrap();
    let answer = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b2")).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    // D-KEY-REPLACE-WAITS: the refusal names what is in flight and what the user can do.
    assert!(answer.body.contains("An open order still uses the current key. Cancel it or wait until it fills, then try again."), "{}", answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())]);
    // Settled (found terminal), the same save replaces account 2's key.
    deltabadger::merge::record_terminal(c, order, "filled").unwrap();
    let answer = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b2")).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b2".into())]);
    // A withdrawal intent of account 2's key: its removal waits, and says so.
    let key_b = identity::key_of(c, accounts[1], 2).unwrap().unwrap();
    c.execute("INSERT INTO rust_withdrawal_intents(owner,state,client_id,created_at,api_key_id,credential_digest,host,account_scope,key_identity,params_enc,fee_basis,budget,free_balance) \
               VALUES ('rule:999','unresolved','test','2026-09-20 10:00:00',?1,'digest','host','spot','key','encrypted','flat','1','1')", [key_b]).unwrap();
    let removed = b.send(&app, "DELETE", &format!("/settings/destroy_api_key/{key_b}"), None, common::web::Csrf::Header, &[]).await;
    assert_eq!(removed.status, 422, "{}", removed.body);
    assert!(removed.body.contains("A withdrawal still uses the current key. Wait until it completes, then try again."), "{}", removed.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b2".into())]);
}

/// §1.1: the in-flight check is the destination account's, by account, not by venue order id. Accounts 1 and 2 both log a
/// standalone order `ord-1` (a venue id two wallets can share): account 1's is open, account 2's ended. A Reconnect of
/// account 1 with account 2's fresh key replaces account 2's key (account 1's open order is not account 2's); the same
/// open order on account 2 instead refuses it.
#[tokio::test(flavor = "current_thread")]
async fn a_routed_replacement_checks_the_destination_s_orders_not_another_s_with_the_same_venue_id() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let mut orders = vec![];
    for account in &accounts {
        c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, created_at, send_by) \
                   VALUES (?1, NULL, 'buy', '10', 'quote', '{}', 'done', '{\"kind\":\"placed\"}', '2026-09-20 10:00:00', '2026-09-20 10:01:00')", [user]).unwrap();
        orders.push(c.last_insert_rowid());
        c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
                   VALUES (?1, ?2, ?3, 1, 'limit', 'rest', 'ord-1', '{}')", rusqlite::params![c.last_insert_rowid(), account, exchange]).unwrap();
    }
    deltabadger::merge::record_terminal(c, orders[1], "filled").unwrap();
    let answer = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b2")).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b2".into())]);
    // Swapped: account 2's ord-1 open, account 1's ended. The save of account 2's key waits.
    c.execute("UPDATE rust_api_orders SET outcome = '{\"kind\":\"placed\"}' WHERE id = ?1", [orders[1]]).unwrap();
    deltabadger::merge::record_terminal(c, orders[0], "filled").unwrap();
    let answer = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b")).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(answer.body.contains("An open order still uses the current key."), "{}", answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b2".into())]);
}

/// §1.1: the destination's slot is fenced. A Reconnect of account 1 with a key of account 2 is still being read when another
/// request replaces account 2's key: under the write lock account 2's slot is no longer what it was, so the delayed save is
/// refused to retry and leaves both accounts' keys as they are.
#[tokio::test(flavor = "current_thread")]
async fn a_delayed_save_aimed_at_one_account_is_refused_when_its_destination_s_key_was_replaced_meanwhile() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let key_b = identity::key_of(c, accounts[1], 2).unwrap().unwrap();
    let db = c.path().unwrap().to_string();
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-b2")).respond_with(move |_: &wiremock::Request| {
        // Another request replaces account 2's key while this one validates.
        let other = Connection::open(&db).unwrap();
        let cipher = seed::cipher();
        identity::write_credential(&other, &cipher, key_b, Some(&cipher.encrypt("key-b3")), Some(&cipher.encrypt("secret")), Some(&cipher.encrypt("phrase")), chrono::Utc::now()).unwrap();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": UID_B, "permission":"General,Spot"}}))
    }).up_to_n_times(1).with_priority(1).mount(&server).await;
    let answer = save(&app, &mut b, exchange, Some(accounts[0]), &kucoin_fields("key-b2")).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(answer.body.contains("credential changed during validation"), "{}", answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b3".into())]);
}

/// §1.1, D-JSON-KEY-IDENTITY: the legacy JSON save reads an identity venue's account first, then routes like every save. A
/// key of account 2 posted for account 1 lands on 2 (201, account 1 untouched); a key whose read fails, with two live
/// accounts, is refused to retry (409) and saves nothing.
#[tokio::test(flavor = "current_thread")]
async fn the_json_save_reads_the_account_and_routes_the_key() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let (exchange_s, first) = (exchange.to_string(), accounts[0].to_string());
    let json = |key: &'static str| vec![("api_key[exchange_id]", exchange_s.clone()), ("api_key[account_id]", first.clone()), ("api_key[key_type]", "read_only".into()),
                                        ("api_key[key]", key.into()), ("api_key[secret]", "secret".into()), ("api_key[passphrase]", "phrase".into())];
    async fn post(app: &deltabadger::web::App, b: &mut common::web::Browser, fields: Vec<(&'static str, String)>) -> common::web::Answer {
        let form: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        b.send(app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await
    }
    let routed = post(&app, &mut b, json("key-b2")).await;
    assert_eq!((routed.status, routed.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b2".into())], "saved on account 2, account 1 untouched");
    let failed = post(&app, &mut b, json("key-fail")).await;
    assert_eq!(failed.status, 409, "{}", failed.body);
    assert!(failed.body.contains("We could not confirm which account this key belongs to"), "{}", failed.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b2".into())]);
}

/// §4: removing a key never removes history. Account 2 with a balance row keeps its row (shown broken) when its last key
/// goes; with nothing naming it, removing its last key tombstones it, and the next new account is 3 (numbers are never
/// reused). Number 1 is never tombstoned by a key removal.
#[tokio::test(flavor = "current_thread")]
async fn removing_an_account_s_last_key_tombstones_it_only_when_nothing_names_it() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let live = |c: &Connection| one::<i64>(c, &format!("SELECT count(*) FROM exchange_accounts WHERE exchange_id = {exchange} AND removed_at IS NULL"));
    let delete = |id: i64| format!("/settings/destroy_api_key/{id}");
    // A balance names account 2: its key goes, the account stays.
    c.execute("INSERT INTO exchange_account_balances (account_id, asset_id, user_id, exchange_id, free, created_at, synced_at, updated_at) VALUES (?1, 1, ?2, ?3, '1', 'now', 'now', 'now')",
              rusqlite::params![accounts[1], user, exchange]).unwrap();
    let key_b = identity::key_of(c, accounts[1], 2).unwrap().unwrap();
    assert!(b.send(&app, "DELETE", &delete(key_b), None, common::web::Csrf::Header, &[]).await.status < 400);
    assert_eq!(live(c), 2, "an account with history stays (broken)");
    // Nothing names it: a fresh key, then its removal, tombstones it.
    c.execute("DELETE FROM exchange_account_balances WHERE account_id = ?1", [accounts[1]]).unwrap();
    assert!(save(&app, &mut b, exchange, Some(accounts[1]), &kucoin_fields("key-b")).await.status < 400);
    let key_b = identity::key_of(c, accounts[1], 2).unwrap().unwrap();
    let removed = b.send(&app, "DELETE", &delete(key_b), None, common::web::Csrf::Header, &[]).await;
    assert!(removed.status < 400, "{}: {}", removed.status, removed.body);
    assert_eq!(live(c), 1, "an account nothing names is tombstoned");
    let key_a = identity::key_of(c, accounts[0], 2).unwrap().unwrap();
    assert!(b.send(&app, "DELETE", &delete(key_a), None, common::web::Csrf::Header, &[]).await.status < 400);
    assert_eq!(live(c), 1, "number 1 stays");
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-b")).await.status < 400);
    assert_eq!(numbers(c, exchange), [1, 2, 3], "the next new account is 3");
}

// ---- §2: the venue step offers accounts ----

/// A trading key saved from a venue tile (the tracker form, `untargeted`).
async fn trading_tile(app: &deltabadger::web::App, b: &mut common::web::Browser, exchange: i64, key: &str) -> common::web::Answer {
    let exchange = exchange.to_string();
    assert_eq!(b.get(app, "/settings/api_keys").await.status, 200);
    b.send(app, "POST", "/tracker/add_api_key", Some(&[("exchange_id", exchange.as_str()), ("key_type", "trading"), ("untargeted", "1"), ("api_key[key]", key),
                                                    ("api_key[secret]", "second-secret"), ("api_key[passphrase]", "paper")]), common::web::Csrf::Header, &[]).await
}
fn location(answer: &common::web::Answer) -> &str { answer.headers.iter().find(|(n, _)| n == "location").map_or("", |(_, v)| v.as_str()) }

/// §2, D-VENUE-TILE-ASKS-KEY: the Connected group draws one tile per account (a corner number with 2+), each posting its
/// `account_id`; picking one completes the key step with that account's key, and the DCA bot the wizard makes trades on it.
/// An Exchanges tile of the same venue always asks for a key; the saved key goes where §1 routes it and the wizard takes that
/// account.
#[tokio::test(flavor = "current_thread")]
async fn connected_tiles_are_accounts_and_the_wizard_makes_its_bot_on_the_picked_one() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let tile = trading_tile(&app, &mut b, s.exchange_id, "second-key").await;
    assert!(tile.status < 400, "{}: {}", tile.status, tile.body);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(accounts.len(), 2);
    b.get(&app, "/bots/new").await;
    let step = b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(step.status, 200);
    for account in &accounts { assert!(step.body.contains(&format!("account_id={account}\"")), "a tile for account {account}: {}", step.body); }
    assert!(step.body.contains("<sup class=\"exchange-grid__account\">2</sup>"), "{}", step.body);
    assert!(step.body.contains("name=\"untargeted\""), "{}", step.body);
    // A Connected tile: account 2's key is correct, so the key step is done.
    let exchange = s.exchange_id.to_string();
    let pick = |account: Option<i64>| (account.map_or("/bots/dca_single_assets/pick_exchange".into(), |a| format!("/bots/dca_single_assets/pick_exchange?account_id={a}")), exchange.clone());
    let (path, e) = pick(Some(accounts[1]));
    let picked = b.send(&app, "POST", &path, Some(&[("bots_dca_single_asset[exchange_id]", e.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await;
    assert_eq!(picked.status, 302, "{}", picked.body);
    let key_step = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(key_step.status, 302, "the picked account's key is correct: {}", key_step.body);
    // The bot: BTC bought with USD, on account 2.
    let btc = s.btc.to_string();
    b.send(&app, "POST", "/bots/dca_single_assets/pick_buyable_asset", Some(&[("bots_dca_single_asset[base_asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    let quote = s.quote.to_string();
    let made = b.send(&app, "POST", &format!("/bots/dca_multi_assets/pick_spendable_asset?exchange_id={exchange}&account_id={}", accounts[1]),
                      Some(&[("bots_dca_multi_asset[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
    assert!(made.status < 400, "{}: {}", made.status, made.body);
    let bot: i64 = one(c, "SELECT max(id) FROM bots");
    assert_eq!(identity::account_of_bot(c, bot).unwrap(), Some(accounts[1]), "the bot trades on the picked account");
    // An Exchanges tile of the same venue asks for a key, even though both accounts hold a correct one.
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    let (path, e) = pick(None);
    assert_eq!(b.send(&app, "POST", &path, Some(&[("bots_dca_single_asset[exchange_id]", e.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let asks = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(asks.status, 200, "{}", location(&asks));
    // The key it is given is account 2's material: saved there, and the wizard goes on with account 2.
    let saved = b.send(&app, "POST", &format!("/bots/dca_single_assets/add_api_key?exchange_id={exchange}&untargeted=1"), Some(&[("api_key[key]", "second-key"), ("api_key[secret]", "second-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert!(saved.status < 400, "{}: {}", saved.status, saved.body);
    assert_eq!(numbers(c, s.exchange_id), [1, 2], "no third account");
    assert_eq!(b.get(&app, "/bots/dca_single_assets/add_api_key/new").await.status, 302, "the session took account 2");
    // A 2.x form (exchange alone) keeps number 1's shortcut.
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/bots/dca_single_assets/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str())]), common::web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(b.get(&app, "/bots/dca_single_assets/add_api_key/new").await.status, 302);
}

// ---- §2: the withdrawal-rule wizard takes the pick ----

const ADDRESS_A: &str = "bc1qaccountonewallet";
const ADDRESS_B: &str = "bc1qaccounttwowallet";

/// A Gemini exchange (the Kraken fixture renamed) whose withdrawal key `test-key` is account 1's, and account 2 with its own
/// withdrawal key `account-2-key`; each key's listing names its own account's approved address.
async fn gemini_two_accounts() -> (tempfile::TempDir, store::Opened, seed::Seeded, i64, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    for (key, address) in [("account-2-key", ADDRESS_B), ("test-key", ADDRESS_A)] {
        Mock::given(path("/v1/approvedAddresses/account/bitcoin")).and(header("X-GEMINI-APIKEY", key))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"approvedAddresses":[{"address":address,"network":"bitcoin","status":"active"}]})))
            .mount(&server).await;
    }
    let (dir, o) = install();
    let (c, cipher) = (&o.primary, seed::cipher());
    let s = seed::seed_kraken(c, &cipher);
    c.execute("UPDATE exchanges SET type='Exchanges::Gemini', name='Gemini'", []).unwrap();
    c.execute("UPDATE tickers SET base='BTC'", []).unwrap();
    c.execute("UPDATE api_keys SET key_type=1", []).unwrap();
    c.execute("INSERT INTO exchange_assets(exchange_id,asset_id,withdrawal_chains,created_at,updated_at) VALUES (?1,?2,'[]','2026-01-01','2026-01-01')", (s.exchange_id, s.btc)).unwrap();
    identity::Account::first(c, s.user_id, s.exchange_id).unwrap().ensure(c, chrono::Utc::now()).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [s.user_id, s.exchange_id]).unwrap();
    let second = identity::Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 1, 'now', 'now')",
              rusqlite::params![s.user_id, s.exchange_id, cipher.encrypt("account-2-key"), cipher.encrypt("account-2-secret")]).unwrap();
    second.add_key(c, c.last_insert_rowid()).unwrap();
    let app = web_app(dir.path(), c).with_venue_key_url("Exchanges::Gemini", &server.uri()).unwrap();
    (dir, o, s, second.id.unwrap(), server, app)
}

/// §2: the withdrawal-rule wizard draws a Connected tile per account; picking account 2's reads account 2's withdrawal key and
/// its addresses, and the rule it makes is account 2's, to account 2's wallet.
#[tokio::test(flavor = "current_thread")]
async fn the_rule_wizard_makes_its_rule_on_the_picked_account_with_its_addresses() {
    let (_d, o, s, second, _server, app) = gemini_two_accounts().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, s.user_id);
    let step = b.get(&app, "/rules/withdrawals/pick_exchange/new").await;
    assert_eq!(step.status, 200, "{}", step.body);
    assert!(step.body.contains(&format!("account_id={second}\"")), "a tile for account 2: {}", step.body);
    let exchange = s.exchange_id.to_string();
    let picked = b.send(&app, "POST", &format!("/rules/withdrawals/pick_exchange?account_id={second}"),
                        Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await;
    assert_eq!(picked.status, 302, "{}", picked.body);
    let btc = s.btc.to_string();
    assert_eq!(pick_asset(&app, &mut b, &btc).await.status, 302);
    let key_step = b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    assert_eq!((key_step.status, location(&key_step)), (302, "/rules/withdrawals/confirm_settings/new"), "account 2's withdrawal key is correct: {}", key_step.body);
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert_eq!(confirm.status, 200, "{}", confirm.body);
    assert!(confirm.body.contains("bc1qaccounttw") && !confirm.body.contains("bc1qaccounton"), "account 2's address: {}", confirm.body);
    let second_s = second.to_string();
    let made = b.send(&app, "POST", "/rules/withdrawals/confirm_settings", Some(&[("account_id", second_s.as_str()), ("network", "bitcoin"), ("min_amount", "0.1"), ("threshold_type", "min_amount")]),
                      common::web::Csrf::Header, &[]).await;
    assert_eq!(made.status, 200, "{}", made.body);
    let (account, address): (i64, String) = c.query_row("SELECT account_id, address FROM exchange_account_rules", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((account, cipher.decrypt(&address).unwrap().as_str()), (second, ADDRESS_B), "account 2's rule, to account 2's wallet");
}

// ---- §2: the tracker's venue step takes the pick ----

/// §2: the tracker's venue step draws a tile per account; picking account 2's opens a key form bound to account 2, whose save
/// replaces account 2's reading key only. A venue tile always asks for a key, even with a healthy one, and its form saves
/// untargeted.
#[tokio::test(flavor = "current_thread")]
async fn the_tracker_venue_step_binds_its_key_form_to_the_picked_account() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let second = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap()[1].0;
    let before = keys(c, &cipher, s.exchange_id);
    let step = b.get(&app, "/tracker/pick_exchange/new").await;
    assert!(step.body.contains(&format!("account_id={second}\"")), "a tile for account 2: {}", step.body);
    let exchange = s.exchange_id.to_string();
    let picked = b.send(&app, "POST", &format!("/tracker/pick_exchange?account_id={second}"), Some(&[("exchange_id", exchange.as_str()), ("untargeted", "1")]),
                        common::web::Csrf::Header, &[]).await;
    assert_eq!((picked.status, location(&picked)), (302, "/tracker/add_api_key/new"), "{}", picked.body);
    let form = b.get(&app, "/tracker/add_api_key/new?key_type=read_only").await;
    assert_eq!(form.status, 200, "{}", location(&form));
    let action = format!("/tracker/add_api_key?exchange_id={exchange}&#38;key_type=read_only&#38;account_id={second}");
    assert!(form.body.contains(&action), "the form names account 2: {}", form.body);
    let saved = b.send(&app, "POST", &action.replace("&#38;", "&"), Some(&[("api_key[key]", "reader-2"), ("api_key[secret]", "reader-2-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert!(saved.status < 400, "{}: {}", saved.status, saved.body);
    // D-ALPACA-ONE-KEY: Alpaca's tracker key is the account's trading key, so the save replaces account 2's.
    let want: Vec<(i64, String)> = before.iter().map(|(n, k)| if *n == 2 { (2, "reader-2".into()) } else { (*n, k.clone()) }).collect();
    assert_eq!(keys(c, &cipher, s.exchange_id), want, "account 2's key; account 1 untouched");
    // A venue tile: the form always asks, and posts untargeted.
    b.get(&app, "/tracker/pick_exchange/new").await;
    let tile = b.send(&app, "POST", "/tracker/pick_exchange", Some(&[("exchange_id", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await;
    assert_eq!(tile.status, 302);
    let asks = b.get(&app, "/tracker/add_api_key/new").await;
    assert_eq!(asks.status, 200, "a venue tile asks for a key: {}", location(&asks));
    assert!(asks.body.contains("&#38;untargeted=1"), "{}", asks.body);
    // A forged account is a 404.
    let forged = b.send(&app, "POST", "/tracker/pick_exchange?account_id=999999", Some(&[("exchange_id", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await;
    assert_eq!(forged.status, 404);
}

// ---- §2: Reconnect is bound to the account it showed ----

/// Every `href`/`action` of `body` that contains `needle`, `&#38;`/`&amp;` read as `&`.
fn links(body: &str, needle: &str) -> Vec<String> {
    body.split(['"', '\'']).filter(|s| s.contains(needle)).map(|s| s.replace("&#38;", "&").replace("&amp;", "&")).collect()
}

/// §2: with two accounts, the sync warning's fixes, the broken chip, the history-gap fixes and the bot's key sheet name the
/// account they show; the bot's sheet refuses a post naming another account than the bot's.
#[tokio::test(flavor = "current_thread")]
async fn reconnect_links_and_the_bot_sheet_name_their_account() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let (first, second) = (accounts[0], accounts[1]);
    c.execute("UPDATE api_keys SET status = 2, last_sync_error = 'Unauthorized'", []).unwrap();
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    let fixes = links(&page.body, "/tracker/add_api_key/new");
    for account in [first, second] {
        assert!(fixes.iter().any(|l| l.contains("key_type=") && l.ends_with(&format!("account_id={account}"))), "a fix for account {account}: {fixes:?}");
    }
    assert!(fixes.iter().any(|l| l == &format!("/tracker/add_api_key/new?account_id={second}&exchange_id={}", s.exchange_id)), "account 2's broken chip: {fixes:?}");
    let gap = b.get(&app, &format!("/tracker/history_gap?account_id={second}")).await;
    assert_eq!(gap.status, 200, "{}", gap.body);
    let gap_fixes = links(&gap.body, "/tracker/add_api_key/new");
    assert!(!gap_fixes.is_empty() && gap_fixes.iter().all(|l| l.ends_with(&format!("account_id={second}"))), "{gap_fixes:?}");
    // The bot's sheet: account 2's bot names account 2; a post naming account 1 (the bot switched meanwhile) is refused.
    let bot = seed::insert_bot(c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("INSERT INTO exchange_account_bots (bot_id, account_id) VALUES (?1, ?2)", [bot, second]).unwrap();
    let sheet = b.get(&app, &format!("/bots/{bot}/add_api_key/new")).await;
    assert_eq!(sheet.status, 200, "{}", sheet.body);
    assert!(links(&sheet.body, "/add_api_key?").contains(&format!("/bots/{bot}/add_api_key?account_id={second}")), "{}", sheet.body);
    let before = keys(c, &cipher, s.exchange_id);
    let stale = b.send(&app, "POST", &format!("/bots/{bot}/add_api_key?account_id={first}"), Some(&[("api_key[key]", "other"), ("api_key[secret]", "other-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 404, "{}", stale.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), before);
}

/// §2, D-TRACKER-MENU-LISTS-CONNECTED: the tracker's connect menu lists a venue the user already holds too, as a venue tile:
/// its link asks for a key whatever keys there are, and the form saves untargeted.
#[tokio::test(flavor = "current_thread")]
async fn the_tracker_menu_lists_connected_venues_as_venue_tiles() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    c.execute("UPDATE exchanges SET available = 1", []).unwrap();
    let mut b = browser(&app, c, s.user_id);
    // Two accounts (the menu is drawn beside the chips), synced once: the populated page, not the first-sync one.
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    c.execute("UPDATE api_keys SET last_synced_at = '2026-09-20 00:00:00', balances_synced_at = '2026-09-20 00:00:00'", []).unwrap();
    let page = b.get(&app, "/tracker").await;
    assert!(page.body.contains("dropdown--exchanges"), "the populated page: {}", page.body);
    assert_eq!(page.status, 200);
    let link = format!("/tracker/add_api_key/new?exchange_id={}&untargeted=1", s.exchange_id);
    assert!(links(&page.body, "/tracker/add_api_key/new").contains(&link), "Alpaca in the menu: {:?}", links(&page.body, "/tracker/add_api_key/new"));
    let form = b.get(&app, &link).await;
    assert_eq!(form.status, 200, "a venue tile asks for a key: {}", location(&form));
    assert!(links(&form.body, "/tracker/add_api_key?").iter().any(|l| l.ends_with("&untargeted=1")), "{}", form.body);
}

// ---- §4: one presence rule for the tracker ----

/// §4: an account kept only by a snapshot (its keys removed, nothing else left of the user's) is on the tracker page as a
/// broken account, not the empty welcome screen.
#[tokio::test(flavor = "current_thread")]
async fn an_account_kept_by_a_snapshot_alone_shows_as_broken_on_the_tracker() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let second = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap()[1].0;
    c.execute("INSERT INTO exchange_account_snapshots(user_id,exchange_id,date,created_at,updated_at,account_id) VALUES(?1,?2,'2026-09-19','2026-09-19','2026-09-19',?3)",
              rusqlite::params![s.user_id, s.exchange_id, second]).unwrap();
    c.execute_batch("DELETE FROM exchange_account_keys; DELETE FROM api_keys;").unwrap();
    assert!(identity::named(c, second).unwrap(), "the snapshot names account 2");
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    let chip = format!("/tracker/add_api_key/new?account_id={second}&exchange_id={}", s.exchange_id);
    assert!(links(&page.body, "/tracker/add_api_key/new").contains(&chip), "account 2's broken chip: {}", page.body);
}

// ---- §2: wizard forms are bound to what they rendered ----

/// §2, two tabs: a key form rendered for an Exchanges tile, posted after another tab picked account 2's Connected tile, is
/// refused before any check: nothing saved, no account made.
#[tokio::test(flavor = "current_thread")]
async fn a_key_form_from_another_tab_is_refused() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let second = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap()[1].0;
    let exchange = s.exchange_id.to_string();
    let pick = |account: Option<i64>| account.map_or("/bots/dca_single_assets/pick_exchange".to_string(), |a| format!("/bots/dca_single_assets/pick_exchange?account_id={a}"));
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", &pick(None), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let tab_a = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(tab_a.status, 200);
    let action = format!("/bots/dca_single_assets/add_api_key?exchange_id={exchange}&untargeted=1");
    assert!(links(&tab_a.body, "/bots/dca_single_assets/add_api_key?").contains(&action), "the form names its pick: {}", tab_a.body);
    // Tab B picks account 2's Connected tile.
    assert_eq!(b.send(&app, "POST", &pick(Some(second)), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let before = keys(c, &cipher, s.exchange_id);
    let stale = b.send(&app, "POST", &action, Some(&[("api_key[key]", "third-key"), ("api_key[secret]", "third-secret"), ("api_key[passphrase]", "paper")]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!((keys(c, &cipher, s.exchange_id), numbers(c, s.exchange_id)), (before, vec![1, 2]), "nothing saved, no account made");
}

/// H5, the withdrawal wizard: a confirmation rendered for a 2.x pick (naming no account: number 1) is bound to that; posted
/// after another tab picked account 2, it makes no rule.
#[tokio::test(flavor = "current_thread")]
async fn h5_a_withdrawal_confirmation_naming_no_account_makes_no_rule_after_another_tab_picks_one() {
    let (_d, o, s, second, _server, app) = gemini_two_accounts().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let exchange = s.exchange_id.to_string();
    b.get(&app, "/rules/withdrawals/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/rules/withdrawals/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str())]), common::web::Csrf::Header, &[]).await.status, 302);
    let btc = s.btc.to_string();
    pick_asset(&app, &mut b, &btc).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert!(confirm.status == 200 && !confirm.body.contains("name=\"account_id\""), "a 2.x form: {}", confirm.body);
    // Tab B picks account 2, BTC again.
    let pick = format!("/rules/withdrawals/pick_exchange?account_id={second}");
    assert_eq!(b.send(&app, "POST", &pick, Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    pick_asset(&app, &mut b, &btc).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let stale = b.send(&app, "POST", "/rules/withdrawals/confirm_settings", Some(&[("network", "bitcoin"), ("min_amount", "0.1"), ("threshold_type", "min_amount")]),
                       common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_rules"), 0, "no rule");
}

/// §2, two tabs: a withdrawal confirmation rendered for account 2, posted after another tab picked account 1, makes no rule.
#[tokio::test(flavor = "current_thread")]
async fn a_withdrawal_confirmation_from_another_tab_makes_no_rule() {
    let (_d, o, s, second, _server, app) = gemini_two_accounts().await;
    let c = &o.primary;
    let first = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let exchange = s.exchange_id.to_string();
    let pick = |account: i64| format!("/rules/withdrawals/pick_exchange?account_id={account}");
    b.get(&app, "/rules/withdrawals/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", &pick(second), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let btc = s.btc.to_string();
    pick_asset(&app, &mut b, &btc).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert!(confirm.body.contains(&format!("<input type=\"hidden\" name=\"account_id\" value=\"{second}\" />")), "the form names account 2: {}", confirm.body);
    // Tab B picks account 1, BTC again and its address.
    assert_eq!(b.send(&app, "POST", &pick(first), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    pick_asset(&app, &mut b, &btc).await;
    assert_eq!(location(&b.get(&app, "/rules/withdrawals/add_api_key/new").await), "/rules/withdrawals/confirm_settings/new");
    let second_s = second.to_string();
    let stale = b.send(&app, "POST", "/rules/withdrawals/confirm_settings", Some(&[("account_id", second_s.as_str()), ("network", "bitcoin"), ("min_amount", "0.1"), ("threshold_type", "min_amount")]),
                       common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_rules"), 0, "no rule");
}

/// §2 stock basket: the sole broker listing the basket has two usable accounts, so the asset step does not take its shortcut;
/// the broker step draws one tile per account (corner numbers), and the bot is made on the picked one, account 2's key read.
#[tokio::test(flavor = "current_thread")]
async fn a_stock_basket_on_a_broker_with_two_accounts_asks_which_and_makes_its_bot_there() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let pair = serde_json::json!({ "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2, "minimum_base_size": "0.000027", "minimum_quote_size": "1" });
    let (stock, _) = seed::add_alpaca_crypto(c, &s, "AAPL", &pair);
    c.execute("UPDATE assets SET category = 'Stock' WHERE id = ?1", [stock]).unwrap();
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(accounts.len(), 2);
    b.get(&app, "/bots/new").await;
    b.send(&app, "POST", "/bots/dca_single_assets/order", Some(&[("flow", "asset_first")]), common::web::Csrf::Header, &[]).await;
    let stock_s = stock.to_string();
    b.send(&app, "POST", "/bots/dca_single_assets/pick_buyable_asset", Some(&[("bots_dca_single_asset[base_asset_id]", stock_s.as_str())]), common::web::Csrf::Header, &[]).await;
    let advanced = b.send(&app, "POST", "/bots/dca_single_assets/pick_buyable_asset/advance", Some(&[]), common::web::Csrf::Header, &[]).await;
    assert_eq!(location(&advanced), "/bots/dca_multi_assets/pick_stock_broker/new", "two accounts: no shortcut");
    let step = b.get(&app, "/bots/dca_multi_assets/pick_stock_broker/new").await;
    assert_eq!(step.status, 200);
    for account in &accounts { assert!(step.body.contains(&format!("pick_stock_broker?account_id={account}\"")), "a tile for account {account}: {}", step.body); }
    assert!(step.body.contains("<sup class=\"exchange-grid__account\">2</sup>"), "{}", step.body);
    let exchange = s.exchange_id.to_string();
    // A forged account is a 404.
    let forged = b.send(&app, "POST", "/bots/dca_multi_assets/pick_stock_broker?account_id=999999", Some(&[("bots_dca_multi_asset[exchange_id]", exchange.as_str())]), common::web::Csrf::Header, &[]).await;
    assert_eq!(forged.status, 404);
    let path = format!("/bots/dca_multi_assets/pick_stock_broker?account_id={}", accounts[1]);
    assert_eq!(b.send(&app, "POST", &path, Some(&[("bots_dca_multi_asset[exchange_id]", exchange.as_str())]), common::web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(b.get(&app, "/bots/dca_multi_assets/add_api_key/new").await.status, 302, "account 2's key is correct");
    let quote = s.quote.to_string();
    let made = b.send(&app, "POST", &format!("/bots/dca_multi_assets/pick_spendable_asset?exchange_id={exchange}&account_id={}", accounts[1]),
                      Some(&[("bots_dca_multi_asset[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
    assert!(made.status < 400, "{}: {}", made.status, made.body);
    let bot: i64 = one(c, "SELECT max(id) FROM bots");
    assert_eq!(identity::account_of_bot(c, bot).unwrap(), Some(accounts[1]), "the bot trades on the picked account");
}

/// Rule H5: the sole-broker shortcut picks the broker's one account for the form, so its quote form (a 2.x one, naming no
/// pick) is bound to that: posted after another tab picked account 2 of the broker, it makes no bot.
#[tokio::test(flavor = "current_thread")]
async fn h5_the_sole_broker_shortcut_s_form_makes_no_bot_after_another_tab_picks_account_2() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let pair = serde_json::json!({ "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2, "minimum_base_size": "0.000027", "minimum_quote_size": "1" });
    let (stock, _) = seed::add_alpaca_crypto(c, &s, "AAPL", &pair);
    c.execute("UPDATE assets SET category = 'Stock' WHERE id = ?1", [stock]).unwrap();
    let mut b = browser(&app, c, s.user_id);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    b.get(&app, "/bots/new").await;
    b.send(&app, "POST", "/bots/dca_single_assets/order", Some(&[("flow", "asset_first")]), common::web::Csrf::Header, &[]).await;
    let stock_s = stock.to_string();
    b.send(&app, "POST", "/bots/dca_single_assets/pick_buyable_asset", Some(&[("bots_dca_single_asset[base_asset_id]", stock_s.as_str())]), common::web::Csrf::Header, &[]).await;
    let advanced = b.send(&app, "POST", "/bots/dca_single_assets/pick_buyable_asset/advance", Some(&[]), common::web::Csrf::Header, &[]).await;
    assert_eq!(location(&advanced), "/bots/dca_multi_assets/add_api_key/new", "one account: the shortcut");
    assert_eq!(b.get(&app, "/bots/dca_multi_assets/add_api_key/new").await.status, 302);
    let step = b.get(&app, "/bots/dca_multi_assets/pick_spendable_asset/new").await;
    let action = "/bots/dca_multi_assets/pick_spendable_asset";
    assert!(step.status == 200 && links(&step.body, action).iter().any(|l| l == action), "a 2.x form: {}", step.body);
    // Tab B: a second account, and the wizard's pick moved to it.
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let exchange = s.exchange_id.to_string();
    let path = format!("/bots/dca_multi_assets/pick_stock_broker?account_id={}", accounts[1]);
    assert_eq!(b.send(&app, "POST", &path, Some(&[("bots_dca_multi_asset[exchange_id]", exchange.as_str())]), common::web::Csrf::Header, &[]).await.status, 302);
    let bots: i64 = one(c, "SELECT count(*) FROM bots");
    let quote = s.quote.to_string();
    let stale = b.send(&app, "POST", action, Some(&[("bots_dca_multi_asset[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM bots"), bots, "no bot");
}

/// §2, two tabs: a signal wizard's Start rendered for account 2, posted after another tab picked account 1, makes no bot.
#[tokio::test(flavor = "current_thread")]
async fn a_signal_start_from_another_tab_makes_no_bot() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let (exchange, btc, quote) = (s.exchange_id.to_string(), s.btc.to_string(), s.quote.to_string());
    let pick = |account: i64| format!("/bots/signals/pick_exchange?account_id={account}");
    b.get(&app, "/bots/new").await;
    b.send(&app, "POST", "/bots/signals/pick_buyable_asset", Some(&[("bots_signal[base_asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    assert_eq!(b.send(&app, "POST", &pick(accounts[1]), Some(&[("bots_signal[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(b.get(&app, "/bots/signals/add_api_key/new").await.status, 302, "account 2's key is correct");
    b.send(&app, "POST", "/bots/signals/pick_spendable_asset", Some(&[("bots_signal[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
    let confirm = b.get(&app, "/bots/signals/confirm_settings/new").await;
    let action = format!("/bots/signals?exchange_id={exchange}&account_id={}", accounts[1]);
    assert!(links(&confirm.body, "/bots/signals?").contains(&action), "Start names account 2: {:?}", links(&confirm.body, "/bots/signals"));
    // Tab B picks account 1.
    assert_eq!(b.send(&app, "POST", &pick(accounts[0]), Some(&[("bots_signal[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let bots: i64 = one(c, "SELECT count(*) FROM bots");
    let stale = b.send(&app, "POST", &action, Some(&[]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM bots"), bots, "no bot");
}

/// §2: a signal Start whose picked account was removed meanwhile (another tab) makes no bot at all, never one on account 1.
#[tokio::test(flavor = "current_thread")]
async fn a_signal_start_for_a_removed_account_makes_no_bot() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let (exchange, btc, quote) = (s.exchange_id.to_string(), s.btc.to_string(), s.quote.to_string());
    b.get(&app, "/bots/new").await;
    b.send(&app, "POST", "/bots/signals/pick_buyable_asset", Some(&[("bots_signal[base_asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    let pick = format!("/bots/signals/pick_exchange?account_id={}", accounts[1]);
    assert_eq!(b.send(&app, "POST", &pick, Some(&[("bots_signal[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(b.get(&app, "/bots/signals/add_api_key/new").await.status, 302);
    b.send(&app, "POST", "/bots/signals/pick_spendable_asset", Some(&[("bots_signal[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
    assert_eq!(b.get(&app, "/bots/signals/confirm_settings/new").await.status, 200);
    c.execute("UPDATE exchange_accounts SET removed_at = '2026-09-20 12:00:00' WHERE id = ?1", [accounts[1]]).unwrap();
    let bots: i64 = one(c, "SELECT count(*) FROM bots");
    let stale = b.send(&app, "POST", &format!("/bots/signals?exchange_id={exchange}&account_id={}", accounts[1]), Some(&[]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM bots"), bots, "no bot, on account 1 least of all");
}

/// §2, two tabs: the DCA quote step's form names its pick; posted after another tab picked account 1, it makes no bot.
#[tokio::test(flavor = "current_thread")]
async fn a_dca_quote_form_from_another_tab_makes_no_bot() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let (exchange, btc, quote) = (s.exchange_id.to_string(), s.btc.to_string(), s.quote.to_string());
    let pick = |account: i64| format!("/bots/dca_single_assets/pick_exchange?account_id={account}");
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", &pick(accounts[1]), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(b.get(&app, "/bots/dca_single_assets/add_api_key/new").await.status, 302);
    b.send(&app, "POST", "/bots/dca_single_assets/pick_buyable_asset", Some(&[("bots_dca_single_asset[base_asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    let step = b.get(&app, "/bots/dca_multi_assets/pick_spendable_asset/new").await;
    let action = format!("/bots/dca_multi_assets/pick_spendable_asset?exchange_id={exchange}&account_id={}", accounts[1]);
    assert!(links(&step.body, "/bots/dca_multi_assets/pick_spendable_asset?").contains(&action), "the form names its pick: {}", step.body);
    // Tab B picks account 1.
    assert_eq!(b.send(&app, "POST", &pick(accounts[0]), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let bots: i64 = one(c, "SELECT count(*) FROM bots");
    let stale = b.send(&app, "POST", &action, Some(&[("bots_dca_multi_asset[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM bots"), bots, "no bot");
}

/// §2, two tabs: the withdrawal wizard's key form names its pick (a venue tile here); posted after another tab picked account
/// 2's tile, it saves nothing into account 2's slot.
#[tokio::test(flavor = "current_thread")]
async fn a_withdrawal_key_form_from_another_tab_saves_nothing() {
    let (_d, o, s, second, _server, app) = gemini_two_accounts().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, s.user_id);
    let (exchange, btc) = (s.exchange_id.to_string(), s.btc.to_string());
    b.get(&app, "/rules/withdrawals/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/rules/withdrawals/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    pick_asset(&app, &mut b, &btc).await;
    let form = b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let action = format!("/rules/withdrawals/add_api_key?exchange_id={exchange}&untargeted=1");
    assert!(links(&form.body, "/rules/withdrawals/add_api_key?").contains(&action), "the form names its pick: {}", form.body);
    // Tab B picks account 2's Connected tile.
    assert_eq!(b.send(&app, "POST", &format!("/rules/withdrawals/pick_exchange?account_id={second}"), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str())]),
                      common::web::Csrf::Header, &[]).await.status, 302);
    let before = keys(c, &cipher, s.exchange_id);
    let stale = b.send(&app, "POST", &action, Some(&[("api_key[key]", "fresh-key"), ("api_key[secret]", "fresh-secret")]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), before, "nothing saved");
}

/// A failed identity read in the bot wizard's key step says so in the user's words, never as an internal code.
#[tokio::test(flavor = "current_thread")]
async fn the_wizard_key_step_names_a_failed_identity_read_in_words() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    let e = exchange.to_string();
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/bots/dca_single_assets/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", e.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let form = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(form.status, 200, "{}", location(&form));
    let answer = b.send(&app, "POST", &format!("/bots/dca_single_assets/add_api_key?exchange_id={e}&untargeted=1"), Some(&kucoin_fields("key-fail")), common::web::Csrf::Header, &[]).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(answer.body.contains("We could not confirm which account this key belongs to") && !answer.body.contains("identity_unconfirmed"), "{}", answer.body);
}

// ---- §3: an import goes to the file's account ----

const BINANCE_NAME: &str = "Binance-Transaction-History-202609190000(UTC+0)-part1-of1.csv";
fn binance_file(uid: &str) -> String { format!("User ID,Time,Account,Operation,Coin,Change,Remark\n{uid},2026-08-01 10:00:46,Spot,Deposit,USDT,1000,\n") }
fn kucoin_file(uid: &str) -> String { include_str!("fixtures/kucoin/trading_account.csv").replace("123456789,", &format!("{uid},")) }
fn venue(c: &Connection, exchange_type: &str, name: &str) -> i64 {
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?2, '0.1', '0.1', '2026-01-01', '2026-01-01')", [exchange_type, name]).unwrap();
    c.last_insert_rowid()
}
/// A correct trading key of `user` on `exchange`, on `account` (None: its number 1).
fn key_on(c: &Connection, user: i64, exchange: i64, account: Option<i64>) -> i64 {
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![user, exchange, e(&format!("k{}", rand::random::<u32>())), e("s")]).unwrap();
    let key = c.last_insert_rowid();
    match account { Some(a) => identity::Account::by_id(c, a).unwrap().unwrap().add_key(c, key).unwrap(), None => { identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap(); } }
    key
}
fn fingerprint(c: &Connection, account: i64, exchange_type: &str, uid: &str) {
    identity::establish_read(c, &seed::cipher(), account, exchange_type, &identity::AccountId::parse(uid).unwrap(), chrono::Utc::now()).unwrap();
}
/// The ledger rows of `account`.
fn rows_on(c: &Connection, account: i64) -> i64 {
    let scope = deltabadger::venue_ids::Scope::of(c, &identity::Account::by_id(c, account).unwrap().unwrap()).unwrap();
    one(c, &format!("SELECT count(*) FROM account_transactions a WHERE a.exchange_id = {} AND {}", scope.exchange_id, scope.rows("a")))
}
async fn upload(app: &deltabadger::web::App, b: &mut common::web::Browser, text: &str) -> common::web::Answer {
    assert_eq!(b.get(app, "/settings/api_keys").await.status, 200);
    let (content_type, body) = common::web::multipart(&[], &[("file", BINANCE_NAME, text.as_bytes())]);
    let mut answer = b.send_bytes(app, "POST", "/tracker/import", Some(body), common::web::Csrf::Header, &[("content-type", content_type.as_str())]).await;
    // PLAN-710-confirm: read as the values its handles stand for.
    common::import::decode_answer(app, &mut answer).await;
    answer
}
/// The question's options (`value`s) and the answer `value` posted back with the parked file.
/// #710 7a rule N4: without the digest each is rendered with (`!…`, `full_options`).
fn options(body: &str) -> Vec<String> {
    full_options(body).into_iter().map(|v| v.split_once('!').map_or(v.clone(), |(b, _)| b.to_string())).collect()
}
/// #710 8a: a segmented option (`data-value`), a named destination's button or a CSV's hidden answer (`name="api_key_id"`).
fn full_options(body: &str) -> Vec<String> {
    regex::Regex::new(r#"data-value="([^"]+)"|name="api_key_id" value="([^"]+)""#).unwrap().captures_iter(body)
        .map(|m| m.get(1).or(m.get(2)).unwrap().as_str().replace("&#33;", "!")).collect()
}
/// #710 8a: the markup of the button posting `value` (its digest aside) and the notes under it, up to the next button.
fn under(body: &str, value: &str) -> String {
    let start = body.find(&format!("name=\"api_key_id\" value=\"{value}")).unwrap_or_else(|| panic!("no button {value}: {body}"));
    let rest = &body[start..];
    let end = rest[1..].find("<button").or_else(|| rest.find("name=\"cancel\"")).map_or(rest.len(), |e| e + 1);
    rest[..end].to_string()
}
/// A CSV's sealed answer (#710 8a): the one hidden `api_key_id`.
fn hidden(body: &str) -> String {
    let all: Vec<String> = regex::Regex::new(r#"<input type="hidden" name="api_key_id" value="([^"]+)""#).unwrap().captures_iter(body).map(|m| m[1].replace("&#33;", "!")).collect();
    assert_eq!(all.len(), 1, "one sealed answer: {body}");
    all[0].clone()
}
/// `value` posted back as the page renders it: with its digest, when the page offered it.
async fn answer(app: &deltabadger::web::App, b: &mut common::web::Browser, asked: &common::web::Answer, value: &str) -> common::web::Answer {
    let token = regex::Regex::new(r#"name="token" id="token" value="([0-9a-f]{16})""#).unwrap().captures(&asked.body).unwrap_or_else(|| panic!("parked: {}", asked.body))[1].to_string();
    let full = full_options(&asked.body).into_iter().find(|v| v == value || v.starts_with(&format!("{value}!")) || v.starts_with(&format!("{value}~"))).unwrap_or_else(|| value.to_string());
    // PLAN-710-confirm (branch review r1 #9): exactly the handle the page rendered for it, whatever was issued since (one
    // never offered posts as it is).
    let handle = common::import::rendered(asked, &full).unwrap_or(full);
    let mut answer = b.send(app, "POST", "/tracker/import", Some(&[("token", token.as_str()), ("api_key_id", handle.as_str())]), common::web::Csrf::Header, &[]).await;
    common::import::decode_answer(app, &mut answer).await;
    answer
}
/// Cancel on the question parked by `asked`.
async fn cancel(app: &deltabadger::web::App, b: &mut common::web::Browser, asked: &common::web::Answer) -> common::web::Answer {
    let token = regex::Regex::new(r#"name="token" id="token" value="([0-9a-f]{16})""#).unwrap().captures(&asked.body).unwrap_or_else(|| panic!("parked: {}", asked.body))[1].to_string();
    b.send(app, "POST", "/tracker/import", Some(&[("token", token.as_str()), ("cancel", "Cancel")]), common::web::Csrf::Header, &[]).await
}
/// A Binance write needs this process to own the install (the lease), as `serve` does.
fn own(dir: &std::path::Path, c: &Connection, app: &deltabadger::web::App) -> deltabadger::lease::EngineLock {
    let lock = deltabadger::lease::lock(&Paths::from_env(&|_| None, dir), app.now()).unwrap();
    deltabadger::lease::claim(&lock, c, &app.cipher, "test", app.now()).unwrap();
    lock
}
fn imported(answer: &common::web::Answer) -> bool { answer.body.contains("turbo-stream") && answer.body.contains("/tracker") && answer.status == 200 }

/// §3, D-IMPORT-WITHOUT-KEY: with no key at all, a KuCoin file makes a keyless account and lands there; a Binance-format file
/// asks the venue first (nothing written), then makes Binance.US's account on that answer.
#[tokio::test(flavor = "current_thread")]
async fn an_import_with_no_key_makes_a_keyless_account_asking_a_binance_file_s_venue_first() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (_binance, us, kucoin) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"), venue(c, "Exchanges::Kucoin", "KuCoin"));
    let mut b = browser(&app, c, s.user_id);
    let done = upload(&app, &mut b, &kucoin_file("111")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    let accounts = identity::accounts_of(c, s.user_id, kucoin).unwrap();
    assert_eq!(accounts.len(), 1, "one keyless account");
    assert!(rows_on(c, accounts[0].0) > 0);
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM account_transactions WHERE exchange_id = {kucoin} AND api_key_id IS NOT NULL")), 0, "no key named");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(asked.status, 200);
    assert_eq!(options(&asked.body).len(), 2, "Binance or Binance.US: {}", asked.body);
    assert_eq!((one::<i64>(c, "SELECT count(*) FROM account_transactions"), identity::accounts_of(c, s.user_id, us).unwrap().len()), (ledger, 0), "nothing written yet");
    let done = answer(&app, &mut b, &asked, &format!("v{us}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    let accounts = identity::accounts_of(c, s.user_id, us).unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(rows_on(c, accounts[0].0), 1);
}

/// Rule N7 (Codex branch r5, run.rs:558): a native import that makes account 2 settles the exchange's readers in its
/// transaction. Account 1's key has no UID read: it read alone, and the second account disqualifies it at once; the
/// generation moves then, so a run that started before the import publishes nothing.
#[tokio::test(flavor = "current_thread")]
async fn n7_an_import_that_makes_an_account_settles_the_readers() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let kk = key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    deltabadger::sync::settle_readers_of_key(c, kk).unwrap();
    assert!(deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&kk), "one account: it reads");
    let before = deltabadger::sync::reader_generation(c, first).unwrap();
    let mut b = browser(&app, c, s.user_id);
    // #710 8a rule R1: a new account beside a live one is named first.
    let asked = upload(&app, &mut b, &kucoin_file("222")).await;
    let done = answer(&app, &mut b, &asked, &format!("n{kucoin}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(numbers(c, kucoin), [1, 2], "the file's own account");
    assert!(!deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&kk), "unread on a two-account exchange");
    assert_ne!(deltabadger::sync::reader_generation(c, first).unwrap(), before, "the handoff moved the generation");
}

/// §3: a Binance file whose UID is account 2's lands on 2, never asking; a Binance.US UID matching a keyless Binance.US
/// account lands there although the only key is Binance's.
#[tokio::test(flavor = "current_thread")]
async fn a_file_whose_account_is_known_lands_there_in_either_binance_venue() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    key_on(c, s.user_id, binance, Some(second));
    fingerprint(c, second, "Exchanges::Binance", "222");
    let keyless_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, keyless_us, "Exchanges::BinanceUs", "333");
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    // #710 8a rule R1: account 2 of two is named first.
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(options(&asked.body), [format!("a{second}")], "{}", asked.body);
    let done = answer(&app, &mut b, &asked, &format!("a{second}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, first), rows_on(c, second)), (0, 1), "account 2's file");
    let done = upload(&app, &mut b, &binance_file("333")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, keyless_us), 1, "the keyless Binance.US account");
    assert_eq!(identity::accounts_of(c, s.user_id, binance).unwrap().len(), 2, "nothing made");
    assert_eq!(identity::standing(c, s.user_id, binance).unwrap(), identity::Standing::Unknown, "a file routed to account 2 is no conflict of account 1");
}

/// §3 / §9.6: two unfingerprinted accounts (keyed or holding history) make a KuCoin file ask which, "a new account" offered;
/// nothing is written until then. A parked answer naming an account given another fingerprint meanwhile is asked again,
/// never routed elsewhere; the new-account answer makes account 3.
#[tokio::test(flavor = "current_thread")]
async fn two_unfingerprinted_accounts_make_the_import_ask_and_a_stale_answer_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    let set = format!("{first}-{second}");
    assert_eq!(options(&asked.body), [format!("a{first}~{set}"), format!("a{second}~{set}"), format!("n{kucoin}~{set}")], "{}", asked.body);
    assert!(asked.body.contains("This file could be from more than one account here. Which one?"), "{}", asked.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
    // Account 2 is read as another account meanwhile: the answer naming it is asked again.
    fingerprint(c, second, "Exchanges::Kucoin", "555");
    let again = answer(&app, &mut b, &asked, &format!("a{second}~{set}")).await;
    assert_eq!(options(&again.body), [format!("a{first}~{first}"), format!("n{kucoin}~{first}")], "asked again, account 2 no longer offered: {}", again.body);
    assert!(rows_on(c, first) == 0 && rows_on(c, second) == 0, "never routed to another account");
    // The new-account answer.
    let done = answer(&app, &mut b, &again, &format!("n{kucoin}~{first}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(identity::accounts_of(c, s.user_id, kucoin).unwrap().len(), 3, "a new account 3");
    assert_eq!((rows_on(c, first), rows_on(c, second)), (0, 0));
}

/// §3: a Binance-format UID that a live account holds in BOTH Binance and Binance.US asks the venue (never the first by id);
/// the answer lands it on that venue's account.
#[tokio::test(flavor = "current_thread")]
async fn a_uid_known_in_both_binance_venues_asks_which_venue() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let on_binance = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    fingerprint(c, on_binance, "Exchanges::Binance", "222");
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "222");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(options(&asked.body), [format!("v{binance}"), format!("v{us}")], "{}", asked.body);
    assert_eq!((rows_on(c, on_binance), rows_on(c, on_us)), (0, 0), "nothing written yet");
    let done = answer(&app, &mut b, &asked, &format!("v{us}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, on_binance), rows_on(c, on_us)), (0, 1));
}

/// §3: with two unfingerprinted Binance accounts (one keyed, one keyless), the venue question names the venue, never the
/// keyed account's key, so "Binance" leads to the account question instead of the keyed account.
#[tokio::test(flavor = "current_thread")]
async fn the_venue_question_names_the_venue_when_it_has_two_accounts() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    let keyless = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert_eq!(options(&asked.body), [format!("v{binance}"), format!("v{us}")], "{}", asked.body);
    let which = answer(&app, &mut b, &asked, &format!("v{binance}")).await;
    let set = format!("{first}-{keyless}");
    assert_eq!(options(&which.body), [format!("a{first}~{set}"), format!("a{keyless}~{set}"), format!("n{binance}~{set}")], "{}", which.body);
    assert_eq!((rows_on(c, first), rows_on(c, keyless)), (0, 0), "nothing written yet");
}

/// §3, #710 7a rule N4: a parked answer is checked against the file's account under the import's lock. Account 2 bound to the
/// file's UID in another tab: the parked "account 1" is asked again (never written there, nor on 2 unasked), and the answer
/// to that lands on 2; two parked "a new account" answers for one UID make one account, the second asked again.
#[tokio::test(flavor = "current_thread")]
async fn a_parked_answer_never_beats_the_file_s_account() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert_eq!(options(&asked.body).len(), 3, "{}", asked.body);
    fingerprint(c, second, "Exchanges::Kucoin", "444");
    let again = answer(&app, &mut b, &asked, &format!("a{first}~{first}-{second}")).await;
    assert!(again.body.contains("Something changed since you answered."), "asked again: {}", again.body);
    assert_eq!(options(&again.body), [format!("a{second}")], "{}", again.body);
    assert_eq!((rows_on(c, first), rows_on(c, second)), (0, 0), "nothing written");
    let done = answer(&app, &mut b, &again, &format!("a{second}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, first), 0, "never on account 1");
    assert!(rows_on(c, second) > 0, "on the file's account");
    // Two tabs, each asked (accounts 1 and 3 unfingerprinted), each answering "a new account".
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let (one, two) = (upload(&app, &mut b, &kucoin_file("888")).await, upload(&app, &mut b, &kucoin_file("888")).await);
    // Rule P5: the same export's rows are on account 2 already (KuCoin row ids carry no UID): shown on the button, and
    // (#710 8a rule R4) accepted by its click.
    assert!(under(&one.body, &options(&one.body)[2]).contains("Rows already on"), "{}", one.body);
    let done = answer(&app, &mut b, &one, &options(&one.body)[2]).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    let late = answer(&app, &mut b, &two, &options(&two.body)[2]).await;
    assert!(late.body.contains("Something changed since you answered."), "the second is asked again: {}", late.body);
    assert_eq!(identity::accounts_of(c, s.user_id, kucoin).unwrap().len(), 4, "one new account for the UID");
}

/// Rule H3: with the file's UID on one keyed account in each Binance venue, the venue question names each venue's key;
/// either answer settles the venue and lands the file there, never asking again.
#[tokio::test(flavor = "current_thread")]
async fn h3_a_key_answer_settles_the_venue_when_both_binance_venues_know_the_uid() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let (kb, ku) = (key_on(c, s.user_id, binance, None), key_on(c, s.user_id, us, None));
    let (on_binance, on_us) = (identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap(), identity::Account::first(c, s.user_id, us).unwrap().id.unwrap());
    fingerprint(c, on_binance, "Exchanges::Binance", "222");
    fingerprint(c, on_us, "Exchanges::BinanceUs", "222");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), ku.to_string()], "{}", asked.body);
    let done = answer(&app, &mut b, &asked, &kb.to_string()).await;
    assert!(imported(&done), "the answer is taken, not asked again: {}: {}", done.status, done.body);
    assert_eq!((rows_on(c, on_binance), rows_on(c, on_us)), (1, 0));
}

/// H3: the venue question offered Binance's key while Binance had one account. A second Binance account made meanwhile:
/// the key answer named that one account, so the import asks again and writes nothing.
#[tokio::test(flavor = "current_thread")]
async fn h3_a_key_answer_rendered_for_one_account_is_asked_again_once_the_venue_has_two() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let kb = key_on(c, s.user_id, binance, None);
    // Plan L8: one Binance reading key alone takes a clean upload; Binance.US having seen the UID asks the venue.
    seen_on_us(c, s.user_id, us, "777");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, &kb.to_string()).await;
    // R10: asked again on its own venue, Binance's accounts offered.
    let values = options(&again.body);
    assert!(values.len() == 3 && values[..2].iter().all(|v| v.starts_with('a')) && values[2].starts_with(&format!("n{binance}~")), "asked again on Binance: {}", again.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
}

/// H3: after "Binance", the account answer lands on Binance even once the file's UID appears on Binance.US meanwhile.
#[tokio::test(flavor = "current_thread")]
async fn h3_an_answer_after_choosing_binance_never_lands_on_binance_us() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    let which = answer(&app, &mut b, &asked, &format!("v{binance}")).await;
    let first_value = options(&which.body).into_iter().find(|v| v.starts_with(&format!("a{first}"))).unwrap_or_else(|| panic!("{}", which.body));
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "777");
    let done = answer(&app, &mut b, &which, &first_value).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, first), rows_on(c, on_us)), (1, 0), "on Binance, as answered");
}

/// H3: the account question's answer is bound to the accounts it offered. A third unfingerprinted account made meanwhile:
/// asked again, all three offered, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn h3_an_account_answer_is_asked_again_when_the_candidates_changed() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    let first_value = options(&asked.body).into_iter().find(|v| v.starts_with(&format!("a{first}"))).unwrap_or_else(|| panic!("{}", asked.body));
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, &first_value).await;
    assert_eq!(options(&again.body).len(), 4, "three accounts and a new one: {}", again.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
}

/// Rule H4: a conflict is looked up in the file's venue. A Binance file of UID 123 held on Binance's account 1 as a conflict
/// never refuses a Binance.US export of UID 123, which lands on the Binance.US account that UID names.
#[tokio::test(flavor = "current_thread")]
async fn h4_a_binance_conflict_never_refuses_a_binance_us_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    // Plan L8: the one Binance reading key takes the clean upload, as on main.
    let done = upload(&app, &mut b, &binance_file("123")).await;
    assert!(imported(&done) && rows_on(c, first) == 1, "{}: {}", done.status, done.body);
    // As before 7a: Binance's account 1 then read another account; its copy of 123's file is a conflict there.
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1", [first]).unwrap();
    fingerprint(c, first, "Exchanges::Binance", "111");
    fingerprint(c, on_us, "Exchanges::BinanceUs", "123");
    // R10: the file is held as a conflict on Binance, so the venue is asked first; Binance.US answered, it lands there.
    let asked = upload(&app, &mut b, &binance_file("123")).await;
    let done = answer(&app, &mut b, &asked, &format!("v{us}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, first), rows_on(c, on_us)), (1, 1));
}

/// §3: the gap dialog reads the line's account: account 1's healthy key does not hide account 2's reconnect; a keyless
/// account (one an import made) is offered Import, and its CSV example names its number, so re-imported it lands there.
#[tokio::test(flavor = "current_thread")]
async fn the_gap_dialog_reads_its_account_and_its_example_lands_on_it() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let second = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap()[1].0;
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [identity::key_of(c, second, 0).unwrap().unwrap()]).unwrap();
    let gap = b.get(&app, &format!("/tracker/history_gap?account_id={second}")).await;
    assert_eq!(gap.status, 200, "{}", gap.body);
    assert!(links(&gap.body, "/tracker/import/new").is_empty() && !links(&gap.body, "/tracker/add_api_key/new").is_empty(), "account 2 gets its reconnect: {}", gap.body);
    let third = identity::new_account(c, s.user_id, s.exchange_id, chrono::Utc::now()).unwrap();
    let gap = b.get(&app, &format!("/tracker/history_gap?account_id={third}")).await;
    assert_eq!(gap.status, 200, "{}", gap.body);
    assert!(!links(&gap.body, "/tracker/import/new").is_empty(), "a keyless account imports: {}", gap.body);
    let name = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let rows: Vec<String> = deltabadger::web::tracker::gap::examples(&name).into_iter().map(|r| format!("{r},3")).collect();
    assert!(gap.body.contains("group_id,description,account"), "{}", gap.body);
    for row in &rows { assert!(gap.body.contains(row.as_str()), "{row} in {}", gap.body); }
    let file = format!("{},account\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, rows.join("\n"));
    // #710 8a rule R2: where the rows go is said first.
    let done = import_confirmed(&app, &mut b, &file).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, third), rows.len() as i64, "on account 3");
}

// ---- §3: the tracker's shortcuts count accounts ----

/// §3: the first-sync and cold first-day pages stand for one account; two Alpaca accounts, never synced and then holding
/// only balances, get the full page with a chip per account.
#[tokio::test(flavor = "current_thread")]
async fn two_accounts_of_one_exchange_skip_the_first_sync_and_cold_pages() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap();
    let chip = format!("/tracker?account_id={}", accounts[1].0);
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    assert!(links(&page.body, "/tracker?").contains(&chip), "never synced: account 2's chip: {}", page.body);
    for (account, _) in &accounts {
        c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at, account_id) \
                   VALUES (?1, ?2, ?3, 1, 0, 60000, 60000, '2026-09-20 11:00:00', '2026-09-20 11:00:00', '2026-09-20 11:00:00', '2026-09-20 11:00:00', ?4)",
                  rusqlite::params![s.user_id, s.exchange_id, s.btc, account]).unwrap();
    }
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    assert!(links(&page.body, "/tracker?").contains(&chip), "balances only: account 2's chip: {}", page.body);
}

/// §3: a Deltabadger CSV whose rows partly name an account this install lacks imports the rest and still says which
/// accounts it left out (D-IMPORT-REPORTS-UNROUTED; before, only a file that imported nothing said so).
#[tokio::test(flavor = "current_thread")]
async fn a_partly_routed_csv_imports_the_rest_and_names_the_accounts_it_left_out() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let name = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let examples = deltabadger::web::tracker::gap::examples(&name);
    let last = examples.len() - 1;
    let rows: Vec<String> = examples.iter().enumerate().map(|(i, r)| format!("{r},{}", if i == last { 2 } else { 1 })).collect();
    let file = format!("{},account\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, rows.join("\n"));
    // #710 7a rule N6: said before writing; "import the rest" writes the rows that have an account.
    let first = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let asked = upload(&app, &mut b, &file).await;
    assert!(asked.body.contains("Rows naming accounts that aren&#39;t here: 1.") || asked.body.contains("Rows naming accounts that aren't here: 1."), "{}", asked.body);
    assert_eq!(options(&asked.body), ["r"], "{}", asked.body);
    assert_eq!(rows_on(c, first), 0, "nothing written yet");
    let done = answer(&app, &mut b, &asked, "r").await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, first), last as i64, "account 1's rows imported");
    // #710 8a rule R15: named as an account is.
    assert!(done.body.contains("No account connected for: Alpaca 2. Those rows were left out."), "{}", done.body);
}

/// Rule N6 (Codex branch r4 #10): a Deltabadger CSV row with no exchange is a row with no account here: counted and said
/// before writing, and "import the rest" leaves it out.
#[tokio::test(flavor = "current_thread")]
async fn n6_a_csv_row_with_no_exchange_is_counted_before_writing() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let v = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let header = format!("{},account", deltabadger::web::tracker::gap::CSV_HEADER);
    let file = format!("{header}\n2023-01-05T10:00:00Z,deposit,BTC,0.25,,,,,{v},T1,,,1\n2023-01-06T10:00:00Z,deposit,BTC,0.5,,,,,,T2,,,1\n");
    let first = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, &file).await;
    assert!(asked.body.contains("Rows naming accounts that aren&#39;t here: 1.") || asked.body.contains("Rows naming accounts that aren't here: 1."), "{}", asked.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written yet");
    let done = answer(&app, &mut b, &asked, "r").await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger + 1, "the routable row only");
    assert!(rows_on(c, first) >= 1);
}

/// §3, D-TRACKER-EMPTY-IMPORT: the empty tracker's welcome screen takes an import beside Connect, so a user with no key can
/// start from a venue's export (the import makes the keyless account): D-IMPORT-DETECT, a file dropped on the window.
#[tokio::test(flavor = "current_thread")]
async fn the_empty_tracker_offers_import_beside_connect() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    c.execute_batch("DELETE FROM exchange_account_keys; DELETE FROM api_keys;").unwrap();
    let mut b = browser(&app, c, s.user_id);
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<div class=\"welcome-screen\">"), "{}", page.body);
    assert!(!links(&page.body, "/tracker/pick_exchange/new").is_empty(), "Connect: {}", page.body);
    assert!(links(&page.body, "/tracker/import/new").is_empty(), "no Import link: {}", page.body);
    assert_eq!(page.body.matches(r#"<form class="window-drop" data-empty-import data-controller="window-drop" data-turbo-frame="modal" "#).count(), 1, "the window drop: {}", page.body);
    assert!(page.body.contains(r#"enctype="multipart/form-data" action="/tracker/import" "#) && page.body.contains(r#"<input type="file" name="file" "#), "{}", page.body);
}

/// D-IMPORT-DETECT: a file whose header names no format is refused before anything is read or parked.
#[tokio::test(flavor = "current_thread")]
async fn a_file_of_no_known_format_is_refused() {
    let (_d, o, s, _server, app) = alpaca().await;
    let mut b = browser(&app, &o.primary, s.user_id);
    let refused = upload(&app, &mut b, "a,b,c\n1,2,3\n").await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Deltabadger can&#39;t import this CSV file. Supported CSV formats: Deltabadger, KuCoin, Binance.")
            || refused.body.contains("Deltabadger can't import this CSV file. Supported CSV formats: Deltabadger, KuCoin, Binance."), "{}", refused.body);
}

/// Per currency, what `account`'s ledger rows move (base and quote sums): the figures a double import would inflate.
fn totals(c: &Connection, account: i64) -> Vec<(String, String)> {
    let scope = deltabadger::venue_ids::Scope::of(c, &identity::Account::by_id(c, account).unwrap().unwrap()).unwrap();
    c.prepare(&format!("SELECT currency, printf('%.10f', SUM(amount)) FROM (SELECT base_currency AS currency, base_amount AS amount FROM account_transactions a WHERE a.exchange_id = {e} AND {r} \
                        UNION ALL SELECT quote_currency, quote_amount FROM account_transactions a WHERE a.exchange_id = {e} AND {r} AND quote_currency IS NOT NULL) GROUP BY currency ORDER BY currency",
                       e = scope.exchange_id, r = scope.rows("a")))
        .unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}

/// §9.6: after a reset clears every fingerprint, both KuCoin accounts holding history are possible owners of a re-imported
/// file: the import asks (a new account offered), nothing is written until then, and the answer re-imports nothing new, so
/// each account's totals stay as they were; the other file then goes to the one account left unread, again adding nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_re_import_after_a_reset_asks_which_account_and_keeps_the_totals() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let mut b = browser(&app, c, s.user_id);
    for uid in ["111", "222"] {
        let mut done = upload(&app, &mut b, &kucoin_file(uid)).await;
        // Rule P5: the second export's rows (the same row ids) are on account 1: warned, imported to a new account anyway.
        if !imported(&done) { let value = options(&done.body).last().cloned().unwrap_or_default(); done = answer(&app, &mut b, &done, &value).await; }
        assert!(imported(&done), "{uid}: {}: {}", done.status, done.body);
    }
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, kucoin).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(accounts.len(), 2, "one account per file");
    let before: Vec<_> = accounts.iter().map(|a| totals(c, *a)).collect();
    assert!(before.iter().all(|t| !t.is_empty()));
    deltabadger::rotation::clear(c, chrono::Utc::now()).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, &kucoin_file("111")).await;
    let set = format!("{}-{}", accounts[0], accounts[1]);
    assert_eq!(options(&asked.body), [format!("a{}~{set}", accounts[0]), format!("a{}~{set}", accounts[1]), format!("n{kucoin}~{set}")], "{}", asked.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written before the answer");
    // Rule P5: account 2 holds the same row ids: shown on the pick, which (#710 8a rule R4) imports anyway, adding nothing.
    assert!(under(&asked.body, &format!("a{}~", accounts[0])).contains("Rows already on"), "{}", asked.body);
    let done = answer(&app, &mut b, &asked, &format!("a{}~{set}", accounts[0])).await;
    assert!(done.status == 200 && done.body.contains("Every transaction in this file is already in your history."), "{}: {}", done.status, done.body);
    let asked = upload(&app, &mut b, &kucoin_file("222")).await;
    assert!(asked.body.contains("Rows already on"), "{}", asked.body);
    let done = answer(&app, &mut b, &asked, &format!("a{}", accounts[1])).await;
    assert!(done.status == 200 && done.body.contains("Every transaction in this file is already in your history."), "the one unread account: {}: {}", done.status, done.body);
    assert_eq!(identity::accounts_of(c, s.user_id, kucoin).unwrap().len(), 2, "no account made");
    assert_eq!(accounts.iter().map(|a| totals(c, *a)).collect::<Vec<_>>(), before, "the totals hold");
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger);
}

/// §3: a Deltabadger CSV row naming a live account that holds only a withdrawal key lands there (no key named).
#[tokio::test(flavor = "current_thread")]
async fn a_csv_row_naming_an_account_with_only_a_withdrawal_key_lands_there() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let second = identity::new_account(c, s.user_id, s.exchange_id, chrono::Utc::now()).unwrap();
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, s.exchange_id, e("withdrawal-only"), e("s")]).unwrap();
    identity::Account::by_id(c, second).unwrap().unwrap().add_key(c, c.last_insert_rowid()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let name = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let rows: Vec<String> = deltabadger::web::tracker::gap::examples(&name).into_iter().map(|r| format!("{r},2")).collect();
    let file = format!("{},account\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, rows.join("\n"));
    let done = import_confirmed(&app, &mut b, &file).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, second), rows.len() as i64, "on account 2");
}

// ---- §6: export and re-import on another install ----

/// §3 / §6: a Deltabadger CSV exported from an install with two Alpaca accounts, imported on a fresh install seeded only with
/// the account mapping (keyless accounts 1 and 2), puts each row on the account of the same number.
#[tokio::test(flavor = "current_thread")]
async fn a_csv_export_restores_on_another_install_by_account_number() {
    let name = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let examples = deltabadger::web::tracker::gap::examples(&name);
    let split = examples.len() / 2;
    let rows: Vec<String> = examples.iter().enumerate().map(|(i, r)| format!("{r},{}", if i < split { 1 } else { 2 })).collect();
    let file = format!("{},account\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, rows.join("\n"));
    // The source: account 1 keyed, account 2 from a tile; each holds its half.
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let source: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    assert!(imported(&import_confirmed(&app, &mut b, &file).await));
    let held: Vec<i64> = source.iter().map(|a| rows_on(c, *a)).collect();
    assert_eq!(held, [split as i64, (examples.len() - split) as i64]);
    let export = b.get(&app, "/tracker/export").await;
    assert_eq!(export.status, 200, "{}", export.body);
    // The destination: the same accounts by number, keyless, nothing else.
    let (_d2, o2, s2, _server2, app2) = alpaca().await;
    let c2 = &o2.primary;
    c2.execute_batch("DELETE FROM exchange_account_keys; DELETE FROM api_keys;").unwrap();
    let second = identity::new_account(c2, s2.user_id, s2.exchange_id, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c2, s2.user_id, s2.exchange_id).unwrap().id.unwrap();
    let mut b2 = browser(&app2, c2, s2.user_id);
    // #710 8a rule R2: two accounts: where the rows go is confirmed first.
    let asked = upload(&app2, &mut b2, &export.body).await;
    assert_eq!((rows_on(c2, first), rows_on(c2, second)), (0, 0), "nothing written yet");
    let done = answer(&app2, &mut b2, &asked, &hidden(&asked.body)).await;
    assert!(imported(&done), "{}: {}\n{}", done.status, done.body, export.body);
    assert_eq!([rows_on(c2, first), rows_on(c2, second)], [held[0], held[1]], "each row on its number: {}", export.body);
}

/// A JSON save (`POST /api/api_keys`) of `fields` (with `kucoin_fields`'s secret and passphrase) for `account`'s read-only slot.
async fn json_save(app: &deltabadger::web::App, b: &mut common::web::Browser, exchange: i64, account: i64, key: &str) -> common::web::Answer {
    let (exchange, account) = (exchange.to_string(), account.to_string());
    let form = [("api_key[exchange_id]", exchange.as_str()), ("api_key[account_id]", account.as_str()), ("api_key[key_type]", "read_only"),
                ("api_key[key]", key), ("api_key[secret]", "secret"), ("api_key[passphrase]", "phrase")];
    b.send(app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await
}

/// Codex branch r1 (keys.rs:756): the JSON save fences its destination as the form does. A key of account 2 posted for
/// account 1 is still being read when another request replaces account 2's key: the slot is no longer what it was before
/// the read, so the delayed save is refused to retry and never overwrites the newer key.
#[tokio::test(flavor = "current_thread")]
async fn a_delayed_json_save_never_overwrites_a_key_replaced_while_it_read_the_account() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let key_b = identity::key_of(c, accounts[1], 2).unwrap().unwrap();
    let db = c.path().unwrap().to_string();
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-b2")).respond_with(move |_: &wiremock::Request| {
        let other = Connection::open(&db).unwrap();
        let cipher = seed::cipher();
        identity::write_credential(&other, &cipher, key_b, Some(&cipher.encrypt("key-b3")), Some(&cipher.encrypt("secret")), Some(&cipher.encrypt("phrase")), chrono::Utc::now()).unwrap();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": UID_B, "permission":"General,Spot"}}))
    }).up_to_n_times(1).with_priority(1).mount(&server).await;
    let answer = json_save(&app, &mut b, exchange, accounts[0], "key-b2").await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(answer.body.contains("credential changed during validation"), "{}", answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b3".into())], "the newer key stays");
}

/// Codex branch r1 (keys.rs:778): account 1's own key posted again through the JSON save, whose fresh read now names account
/// 2: the read is recorded (account 1's key is fenced, §9.1) and the save answers that the key opens another account,
/// never a 201 status reset. Nothing is saved.
#[tokio::test(flavor = "current_thread")]
async fn a_json_repost_whose_read_names_another_account_records_it_and_is_refused() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    let key_a = identity::key_of(c, accounts[0], 2).unwrap().unwrap();
    let fenced = |c: &Connection| one::<bool>(c, &format!("SELECT {} FROM api_keys k WHERE k.id = {key_a}", identity::fenced_sql(c, "k").unwrap()));
    assert!(!fenced(c));
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": UID_B, "permission":"General,Spot"}})))
        .with_priority(1).mount(&server).await;
    let answer = json_save(&app, &mut b, exchange, accounts[0], "key-a").await;
    assert_eq!(answer.status, 409, "{}", answer.body);
    assert!(answer.body.contains("This key opens a different exchange account than this one"), "{}", answer.body);
    assert!(fenced(c), "the read naming account 2 is recorded on account 1's key");
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())], "nothing saved");
}

/// Codex branch r1 (merge.rs:524), D-KEY-REPLACE-WAITS: a key's replacement reads what it waits on first, as Merge's
/// pre-flight does. A single account's standalone order the venue reports filled is recorded so, and the key is replaced;
/// one the venue still reports open keeps the key waiting.
#[tokio::test(flavor = "current_thread")]
async fn a_key_replacement_waits_only_on_a_standalone_order_still_open_at_its_venue() {
    let (_d, o, s, server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let account = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let version = deltabadger::engine::model::credential_version_by_id(c, s.api_key_id).unwrap().unwrap().test_stamp().to_string();
    let order = |external: &str| {
        c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, credential_version, created_at, send_by) \
                   VALUES (?1, NULL, 'buy', '0.001', 'base', '{}', 'done', '{\"kind\":\"placed\"}', ?2, '2026-09-20 10:00:00', '2026-09-20 10:01:00')", rusqlite::params![s.user_id, version]).unwrap();
        let id = c.last_insert_rowid();
        c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
                   VALUES (?1, ?2, ?3, ?4, 'market', 'rest', ?5, '{\"symbol\":\"BTC/USD\",\"side\":\"buy\",\"limit\":false}')", rusqlite::params![id, account, s.exchange_id, s.ticker_id, external]).unwrap();
        id
    };
    let answer = |status: &str| serde_json::json!({ "id": "x", "status": status, "symbol": "BTC/USD", "type": "market", "side": "buy", "qty": "0.001", "notional": null,
                                                   "filled_qty": if status == "filled" { "0.001" } else { "0" }, "filled_avg_price": if status == "filled" { serde_json::json!("60000") } else { serde_json::Value::Null }, "limit_price": null });
    Mock::given(method("GET")).and(path("/v2/orders/ord-filled")).respond_with(ResponseTemplate::new(200).set_body_json(answer("filled"))).with_priority(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/orders/ord-open")).respond_with(ResponseTemplate::new(200).set_body_json(answer("new"))).with_priority(1).mount(&server).await;
    let filled = order("ord-filled");
    let b = browser(&app, c, s.user_id);
    let reconnect = |key: &'static str| {
        let (exchange, target) = (s.exchange_id.to_string(), account.to_string());
        let mut b = b.clone();
        let app = app.clone();
        async move {
            assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
            b.send(&app, "POST", "/tracker/add_api_key", Some(&[("exchange_id", exchange.as_str()), ("key_type", "trading"), ("account_id", target.as_str()),
                                                               ("api_key[key]", key), ("api_key[secret]", "replacement-secret"), ("api_key[passphrase]", "paper")]), common::web::Csrf::Header, &[]).await
        }
    };
    let replaced = reconnect("replacement-key").await;
    assert!(replaced.status < 400, "{}: {}", replaced.status, replaced.body);
    assert_eq!(one::<Option<String>>(c, &format!("SELECT json_extract(outcome, '$.terminal') FROM rust_api_orders WHERE id = {filled}")).as_deref(), Some("filled"));
    let key: String = one(c, &format!("SELECT key FROM api_keys WHERE id = {}", s.api_key_id));
    assert_eq!(cipher.decrypt(&key).unwrap(), "replacement-key", "replaced once its order is done");
    // An order the venue still reports open, placed under the current key: the key waits.
    let version = deltabadger::engine::model::credential_version_by_id(c, s.api_key_id).unwrap().unwrap().test_stamp().to_string();
    let open = order("ord-open");
    c.execute("UPDATE rust_api_orders SET credential_version = ?1 WHERE id = ?2", rusqlite::params![version, open]).unwrap();
    let waiting = reconnect("another-key").await;
    assert_eq!(waiting.status, 422, "{}", waiting.body);
    assert!(waiting.body.contains("An open order still uses the current key."), "{}", waiting.body);
}

/// Answers Alpaca's order read (filled) and, as it answers, replaces account 1's key (D-ALPACA-ONE-KEY: its tracker key is
/// the trading key) from another request.
struct SaveWhileRead(std::path::PathBuf, i64, i64);
impl wiremock::Respond for SaveWhileRead {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let c = Connection::open(&self.0).unwrap();
        let cipher = seed::cipher();
        c.execute("UPDATE api_keys SET key = ?3, secret = ?4, updated_at = 'now' WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0",
                  rusqlite::params![self.1, self.2, cipher.encrypt("other-request-key"), cipher.encrypt("other-secret")]).unwrap();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "x", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "qty": "0.001",
            "notional": null, "filled_qty": "0.001", "filled_avg_price": "60000", "limit_price": null }))
    }
}

/// Rule F4 (Codex branch r2, keys.rs:761), a single-account install: the JSON save takes its slot snapshot before the
/// standalone-order pre-flight's venue read. Another request saves the slot while that read is out; the first save is
/// refused to retry and never overwrites it. The form save is ordered the same way (`snapshot_then_settle`).
#[tokio::test(flavor = "current_thread")]
async fn s7a_a_json_save_never_overwrites_a_key_saved_while_its_preflight_read_the_venue() {
    for form in [true, false] {
    let (_d, o, s, server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let account = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let version = deltabadger::engine::model::credential_version_by_id(c, s.api_key_id).unwrap().unwrap().test_stamp().to_string();
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, credential_version, created_at, send_by) \
               VALUES (?1, NULL, 'buy', '0.001', 'base', '{}', 'done', '{\"kind\":\"placed\"}', ?2, '2026-09-20 10:00:00', '2026-09-20 10:01:00')", rusqlite::params![s.user_id, version]).unwrap();
    let order = c.last_insert_rowid();
    c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
               VALUES (?1, ?2, ?3, ?4, 'market', 'rest', 'ord-race', '{\"symbol\":\"BTC/USD\",\"side\":\"buy\",\"limit\":false}')", rusqlite::params![order, account, s.exchange_id, s.ticker_id]).unwrap();
    Mock::given(method("GET")).and(path("/v2/orders/ord-race")).respond_with(SaveWhileRead(o.primary.path().unwrap().into(), s.user_id, s.exchange_id)).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, s.user_id);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let answer = if form { save(&app, &mut b, s.exchange_id, Some(account), &[("api_key[key]", "first-request-key"), ("api_key[secret]", "s"), ("api_key[passphrase]", "paper")]).await }
                 else { json_save(&app, &mut b, s.exchange_id, account, "first-request-key").await };
    assert!(answer.status >= 400, "form {form}: {}: {}", answer.status, answer.body);
    let saved: Vec<String> = c.prepare("SELECT key FROM api_keys WHERE key_type IN (0, 2)").unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap().map(|k| cipher.decrypt(&k.unwrap()).unwrap()).collect();
    assert_eq!(saved, ["other-request-key"], "form {form}: the other request's key stays");
    }
}

/// Rule F5 (Codex branch r2, keys.rs:803): a contradicting read fences every copy of that material, in every slot. Account
/// 1 holds key-a as its read-only and its trading key; a JSON repost of the read-only one reads account 2: both copies are
/// fenced, the trading one included. Then a tile save of key-a (the form path) lands on account 2 and fences them again.
#[tokio::test(flavor = "current_thread")]
async fn s7a_a_contradicting_read_fences_every_copy_of_the_key_on_the_account() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("key-a"), cipher.encrypt("secret"), cipher.encrypt("phrase")]).unwrap();
    let trading = c.last_insert_rowid();
    assert_eq!(identity::account_of_key(c, trading).unwrap(), Some(accounts[0]));
    let read_only = identity::key_of(c, accounts[0], 2).unwrap().unwrap();
    let fenced = |c: &Connection, k: i64| one::<bool>(c, &format!("SELECT {} FROM api_keys k WHERE k.id = {k}", identity::fenced_sql(c, "k").unwrap()));
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": UID_B, "permission":"General,Spot"}})))
        .with_priority(1).mount(&server).await;
    assert_eq!(json_save(&app, &mut b, exchange, accounts[0], "key-a").await.status, 409);
    assert_eq!((fenced(c, read_only), fenced(c, trading)), (true, true), "the JSON refusal fences both copies");
    c.execute("DELETE FROM exchange_account_credentials WHERE api_key_id IN (?1, ?2)", [read_only, trading]).unwrap();
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    assert_eq!((fenced(c, read_only), fenced(c, trading)), (true, true), "the form save fences both copies too");
}

/// Rule R2 (Codex branch r3, keys.rs:407): a key save refused after its account read still records what it learned. Account
/// 1 holds key-a as its read-only and its trading key; a tile save of key-a reads account 2, whose read-only slot an open
/// standalone order holds: the save is refused, and both of account 1's copies are fenced all the same.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r2_a_refused_save_still_fences_the_copies_its_read_contradicts() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["key-a", "key-b"] { assert!(save(&app, &mut b, exchange, None, &kucoin_fields(key)).await.status < 400); }
    let accounts: Vec<i64> = identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("key-a"), cipher.encrypt("secret"), cipher.encrypt("phrase")]).unwrap();
    let trading = c.last_insert_rowid();
    let read_only = identity::key_of(c, accounts[0], 2).unwrap().unwrap();
    let ticker: i64 = c.query_row("SELECT id FROM tickers LIMIT 1", [], |r| r.get(0)).unwrap();
    let held = identity::key_of(c, accounts[1], 2).unwrap().unwrap();
    let material = |k: i64| -> String { cipher.decrypt(&one::<String>(c, &format!("SELECT key FROM api_keys WHERE id = {k}"))).unwrap() };
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, credential_version, created_at, send_by) \
               VALUES (?1, NULL, 'buy', '0.001', 'base', '{}', 'done', '{\"kind\":\"placed\"}', 'null', '2026-09-20 10:00:00', '2026-09-20 10:01:00')", [user]).unwrap();
    let order = c.last_insert_rowid();
    c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
               VALUES (?1, ?2, ?3, ?4, 'market', 'rest', 'ord-open', '{\"symbol\":\"BTC-USDT\",\"side\":\"buy\",\"limit\":false}')", rusqlite::params![order, accounts[1], exchange, ticker]).unwrap();
    let fenced = |c: &Connection, k: i64| one::<bool>(c, &format!("SELECT {} FROM api_keys k WHERE k.id = {k}", identity::fenced_sql(c, "k").unwrap()));
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": UID_B, "permission":"General,Spot"}})))
        .with_priority(1).mount(&server).await;
    let answer = save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await;
    assert!(answer.status >= 400 || answer.body.contains("wait"), "refused: {} {}", answer.status, answer.body);
    assert_eq!((identity::key_of(c, accounts[1], 2).unwrap(), material(held)), (Some(held), "key-b".to_string()), "account 2 keeps key-b");
    assert_eq!((fenced(c, read_only), fenced(c, trading)), (true, true), "both of account 1's copies are fenced");
}

/// Answers Alpaca's order read as filled and, as it answers, replaces the install's read-only key in place: another
/// request settled the order and saved a new key while the delete's pre-flight read the venue.
struct ReplaceWhileRead(std::path::PathBuf, i64);
impl wiremock::Respond for ReplaceWhileRead {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let c = Connection::open(&self.0).unwrap();
        c.execute("UPDATE api_keys SET key = ?2, updated_at = 'later' WHERE id = ?1", rusqlite::params![self.1, seed::cipher().encrypt("replacement-key")]).unwrap();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "x", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "qty": "0.001",
            "notional": null, "filled_qty": "0.001", "filled_avg_price": "60000", "limit_price": null }))
    }
}

/// Rule R6 (Codex branch r3, keys.rs:628/673), a single-account install: a key deletion takes its key's version before the
/// standalone-order pre-flight reads the venue, and deletes only that version under the lock. A key replaced in place while
/// the read was out is kept, and the delete asks to retry.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r6_a_delete_never_removes_a_key_replaced_while_its_preflight_read_the_venue() {
    let (_d, o, s, server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, ?5, 1, 'now', 'now')",
              rusqlite::params![s.user_id, s.exchange_id, cipher.encrypt("first-key"), cipher.encrypt("s"), cipher.encrypt("paper")]).unwrap();
    let read_only = c.last_insert_rowid();
    let account = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let version = deltabadger::engine::model::credential_version_by_id(c, s.api_key_id).unwrap().unwrap().test_stamp().to_string();
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, credential_version, created_at, send_by) \
               VALUES (?1, NULL, 'buy', '0.001', 'base', '{}', 'done', '{\"kind\":\"placed\"}', ?2, '2026-09-20 10:00:00', '2026-09-20 10:01:00')", rusqlite::params![s.user_id, version]).unwrap();
    let order = c.last_insert_rowid();
    c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
               VALUES (?1, ?2, ?3, ?4, 'market', 'rest', 'ord-race', '{\"symbol\":\"BTC/USD\",\"side\":\"buy\",\"limit\":false}')", rusqlite::params![order, account, s.exchange_id, s.ticker_id]).unwrap();
    Mock::given(method("GET")).and(path("/v2/orders/ord-race")).respond_with(ReplaceWhileRead(o.primary.path().unwrap().into(), read_only)).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, s.user_id);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let removed = b.send(&app, "DELETE", &format!("/settings/destroy_api_key/{read_only}"), None, common::web::Csrf::Header, &[]).await;
    assert_eq!(removed.status, 422, "{}", removed.body);
    let kept: Option<String> = rusqlite::OptionalExtension::optional(c.query_row("SELECT key FROM api_keys WHERE id = ?1", [read_only], |r| r.get(0))).unwrap();
    assert_eq!(kept.map(|k| cipher.decrypt(&k).unwrap()).as_deref(), Some("replacement-key"), "the replacement stays");
}

/// 7a rule R10: an answer's venue is fixed once given. The venue question offered Binance's key while Binance had one
/// account; Binance gains a second, and the file's UID turns up on a Binance.US account meanwhile. The stale key answer is
/// asked again on Binance (its accounts offered), never routed to Binance.US.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r10_a_stale_key_answer_is_asked_again_on_its_own_venue() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let kb = key_on(c, s.user_id, binance, None);
    // Plan L8: one Binance reading key alone takes a clean upload; Binance.US having seen the UID asks the venue.
    seen_on_us(c, s.user_id, us, "777");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "777");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, &kb.to_string()).await;
    let values = options(&again.body);
    assert!(values.len() == 3 && values.iter().all(|v| v.starts_with('a') || v == &format!("n{binance}~{}", values[0].split('~').nth(1).unwrap())), "Binance's accounts asked: {}", again.body);
    assert_eq!((rows_on(c, on_us), one::<i64>(c, "SELECT count(*) FROM account_transactions")), (0, ledger), "nothing written, never on Binance.US");
}

/// R10: a fresh upload of a Binance file held on Binance's account 1 as a conflict is never routed to Binance.US because the
/// UID matches an account there: the venue is asked; Binance.US answered, it lands there.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r10_a_fresh_upload_of_a_held_file_asks_before_another_venue_takes_it() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let kb = key_on(c, s.user_id, binance, None);
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    // Plan L8: the one Binance reading key takes the clean upload, as on main.
    let done = upload(&app, &mut b, &binance_file("123")).await;
    assert!(imported(&done) && rows_on(c, first) == 1, "{}: {}", done.status, done.body);
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1", [first]).unwrap();
    fingerprint(c, first, "Exchanges::Binance", "111");
    fingerprint(c, on_us, "Exchanges::BinanceUs", "123");
    let fresh = upload(&app, &mut b, &binance_file("123")).await;
    assert_eq!(options(&fresh.body), [kb.to_string(), format!("v{us}")], "the venue is asked: {}", fresh.body);
    assert_eq!(rows_on(c, on_us), 0, "nothing on Binance.US yet");
    let done = answer(&app, &mut b, &fresh, &format!("v{us}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, first), rows_on(c, on_us)), (1, 1));
}

/// 7a rule R11: the bot's key sheet, rendered while its account was the exchange's only one (its action names no account),
/// saves nothing once the bot moved to account 2: a nameless sheet is account 1's.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r11_a_bot_sheet_rendered_for_the_sole_account_saves_nothing_once_the_bot_moved() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, s.user_id);
    let bot = seed::insert_bot(c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let sheet = b.get(&app, &format!("/bots/{bot}/add_api_key/new")).await;
    assert_eq!(sheet.status, 200, "{}", sheet.body);
    assert!(links(&sheet.body, "/add_api_key").contains(&format!("/bots/{bot}/add_api_key")), "a single-account sheet names no account: {}", sheet.body);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let second = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap()[1].0;
    c.execute("INSERT INTO exchange_account_bots (bot_id, account_id) VALUES (?1, ?2)", [bot, second]).unwrap();
    let before = keys(c, &cipher, s.exchange_id);
    let stale = b.send(&app, "POST", &format!("/bots/{bot}/add_api_key"), Some(&[("api_key[key]", "other"), ("api_key[secret]", "other-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 404, "{}", stale.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), before, "account 2's key untouched");
}

/// R11, the withdrawal wizard: a confirmation or a preview rendered for account 2, posted after another tab picked account 1,
/// is refused before any of its fields reaches the session: account 1's confirmation shows none of them.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r11_a_stale_withdrawal_confirmation_or_preview_copies_nothing_into_the_session() {
    let (_d, o, s, second, _server, app) = gemini_two_accounts().await;
    let c = &o.primary;
    let first = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let exchange = s.exchange_id.to_string();
    let pick = |account: i64| format!("/rules/withdrawals/pick_exchange?account_id={account}");
    let btc = s.btc.to_string();
    b.get(&app, "/rules/withdrawals/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", &pick(second), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    pick_asset(&app, &mut b, &btc).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    assert_eq!(b.get(&app, "/rules/withdrawals/confirm_settings/new").await.status, 200);
    // Tab B picks account 1.
    assert_eq!(b.send(&app, "POST", &pick(first), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    pick_asset(&app, &mut b, &btc).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let second_s = second.to_string();
    for path in ["/rules/withdrawals/confirm_settings", "/rules/withdrawals/confirm_settings/preview"] {
        let stale = b.send(&app, "POST", path, Some(&[("account_id", second_s.as_str()), ("network", "bitcoin"), ("min_amount", "0.777"), ("threshold_type", "min_amount")]),
                           common::web::Csrf::Header, &[]).await;
        assert_eq!(stale.status, 409, "{path}: {}", stale.body);
    }
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert_eq!(confirm.status, 200, "{}", confirm.body);
    assert!(!confirm.body.contains("0.777"), "account 1's session took none of account 2's fields: {}", confirm.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_rules"), 0, "no rule");
}

/// Rule S2 (Codex branch r4, key_check.rs:363/836): an identity read is recorded the moment it is read, whatever the
/// probes after it answer. Binance.US's account read names B's account for a key stored on A, then its trading probe
/// fails: the save is not made, and A's stored copies of that material are fenced all the same.
#[tokio::test(flavor = "current_thread")]
async fn s7a_s2_a_binance_us_read_naming_another_account_fences_even_when_the_probe_fails() {
    let server = MockServer::start().await;
    let path_ = deltabadger::web::settings::key_check::BINANCE_US_ACCOUNT_PATH;
    for (key, uid) in [("key-a", 111), ("key-b", 222)] {
        Mock::given(method("GET")).and(path(path_)).and(header("X-MBX-APIKEY", key))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"uid": uid, "canTrade": true, "balances": []}))).mount(&server).await;
    }
    let (_d, o) = install();
    let (c, cipher) = (&o.primary, seed::cipher());
    let s = seed::seed_alpaca(c, &cipher);
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::BinanceUs', 'Binance.US', '0.1', '0.1', 'now', 'now')", []).unwrap();
    let exchange = c.last_insert_rowid();
    let app = web_app(_d.path(), c).with_venue_key_url("Exchanges::BinanceUs", &server.uri()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let fields = |key: &'static str| [("api_key[key]", key), ("api_key[secret]", "secret")];
    for key in ["key-a", "key-b"] { let a = save(&app, &mut b, exchange, None, &fields(key)).await; assert!(a.status < 400, "{} {}", a.status, a.body); }
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, exchange).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(accounts.len(), 2);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, 'now', 'now')",
              rusqlite::params![s.user_id, exchange, cipher.encrypt("key-a"), cipher.encrypt("secret")]).unwrap();
    let trading = c.last_insert_rowid();
    let read_only = identity::key_of(c, accounts[0], 2).unwrap().unwrap();
    let fenced = |c: &Connection, k: i64| one::<bool>(c, &format!("SELECT {} FROM api_keys k WHERE k.id = {k}", identity::fenced_sql(c, "k").unwrap()));
    assert_eq!((fenced(c, read_only), fenced(c, trading)), (false, false));
    Mock::given(method("GET")).and(path(path_)).and(header("X-MBX-APIKEY", "key-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"uid": 222, "canTrade": true, "balances": []}))).with_priority(1).mount(&server).await;
    Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(503)).with_priority(1).mount(&server).await;
    let target = accounts[0].to_string();
    let exchange_s = exchange.to_string();
    let form = [("exchange_id", exchange_s.as_str()), ("key_type", "trading"), ("account_id", target.as_str()), ("api_key[key]", "key-a"), ("api_key[secret]", "secret")];
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let answer = b.send(&app, "POST", "/tracker/add_api_key", Some(&form), common::web::Csrf::Header, &[]).await;
    assert!(answer.status >= 400, "the failed probe saves nothing: {} {}", answer.status, answer.body);
    assert!(server.received_requests().await.unwrap().iter().any(|r| r.method.as_str() == "DELETE"), "the probe was sent");
    assert_eq!((fenced(c, read_only), fenced(c, trading)), (true, true), "both of account 1's copies are fenced");
}

/// Rule S5 (Codex branch r4, run.rs:300/331): a key answer keeps its venue after its key is gone. The venue question offered
/// Binance's key; the key is deleted and Binance.US takes the file's UID; "Binance" submitted is asked again on Binance,
/// and nothing lands on Binance.US.
#[tokio::test(flavor = "current_thread")]
async fn s7a_s5_a_key_answer_whose_key_is_gone_is_asked_again_on_its_venue() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let kb = key_on(c, s.user_id, binance, None);
    // Plan L8: one Binance reading key alone takes a clean upload; Binance.US having seen the UID asks the venue.
    seen_on_us(c, s.user_id, us, "777");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [kb]).unwrap();
    c.execute("DELETE FROM api_keys WHERE id = ?1", [kb]).unwrap();
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "777");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, &kb.to_string()).await;
    let values = options(&again.body);
    assert!(!values.is_empty() && values.iter().all(|v| v.starts_with('a') || v.starts_with(&format!("n{binance}~"))), "Binance's accounts asked: {}", again.body);
    assert_eq!((rows_on(c, on_us), one::<i64>(c, "SELECT count(*) FROM account_transactions")), (0, ledger), "nothing written, never on Binance.US");
}

/// Rule N4 (Codex branch r4 #3): a parked upload is written once. Two answers of one upload's venue question, one per venue
/// (two tabs, both holding the parked bytes): the first writes, the second writes nothing.
#[tokio::test(flavor = "current_thread")]
async fn n4_a_parked_upload_is_written_once_whatever_its_answers() {
    use deltabadger::tracker::import::run;
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let (kb, ku) = (key_on(c, s.user_id, binance, None), key_on(c, s.user_id, us, None));
    let file = binance_file("777");
    let write = |answer: Option<&str>| {
        let read = run::read("binance", file.as_bytes(), Some("+00:00")).unwrap_or_else(|_| panic!("readable"));
        run::write(c, &seed::cipher(), s.user_id, "binance", read, "00000000000000aa", answer, chrono::Utc::now()).unwrap()
    };
    assert!(matches!(write(None), run::Answer::Ambiguous(_)), "the venue is asked");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    assert!(matches!(write(Some(&kb.to_string())), run::Answer::Written { imported: 1, .. }));
    assert!(matches!(write(Some(&ku.to_string())), run::Answer::Written { imported: 0, .. }), "the second answer writes nothing");
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger + 1, "written once");
}

/// Rule S9 (Codex branch r4, run.rs:302/379) and plan L8 (Codex branch r4 #11), a single-account install: a legacy Binance
/// reading key with no account row yet, the only Binance-family reading key, Binance.US holding nothing of the file's UID:
/// the clean upload picks that key as main does, asking nothing, and its rows keep that key's id (the write makes number
/// 1's row after the readers were read).
#[tokio::test(flavor = "current_thread")]
async fn s7a_s9_a_legacy_key_answer_keeps_its_key_on_the_rows() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, binance, e("legacy"), e("s")]).unwrap();
    let kb = c.last_insert_rowid();
    assert_eq!(numbers(c, binance), Vec::<i64>::new(), "no account row yet");
    let mut b = browser(&app, c, s.user_id);
    let done = upload(&app, &mut b, &binance_file("555")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM account_transactions WHERE exchange_id = {binance} AND api_key_id = {kb}")), 1, "the row keeps its key");
    // Rules G1/J2: a Binance.US key (not a reading one: invalid) observed the next file's UID: the venue is asked.
    let ku = key_on(c, s.user_id, us, None);
    identity::record_copy(c, &seed::cipher(), ku, "Exchanges::BinanceUs", &identity::AccountId::parse("556").unwrap(), chrono::Utc::now()).unwrap();
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [ku]).unwrap();
    let asked = upload(&app, &mut b, &binance_file("556")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
}

/// Plan L8 (Codex branch r5, run.rs:727): main's sole reader is one reading key across Binance and Binance.US together. Two
/// Binance readers (two accounts) and one Binance.US reader: the venue is asked, never Binance.US picked as the only one.
#[tokio::test(flavor = "current_thread")]
async fn l8_two_binance_readers_and_one_binance_us_reader_ask_the_venue() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    key_on(c, s.user_id, binance, Some(second));
    let ku = key_on(c, s.user_id, us, None);
    let on_us = identity::Account::first(c, s.user_id, us).unwrap().id.unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert_eq!(options(&asked.body), [format!("v{binance}"), ku.to_string()], "the venue is asked: {}", asked.body);
    assert_eq!((rows_on(c, on_us), one::<i64>(c, "SELECT count(*) FROM account_transactions")), (0, ledger), "nothing written");
}

/// Rule S6 (Codex branch r4, keys.rs:208, validator.rs:75), a single-account install: a bot's key sheet is drawn for its
/// exchange and account. The sheet opened while the bot was on Kraken, the bot moved to Alpaca since: its submit is a 404
/// and Alpaca is never sent the secret. The sheet's bytes are as before (the binding rides in its masked CSRF token).
#[tokio::test(flavor = "current_thread")]
async fn s7a_s6_a_bot_sheet_drawn_for_another_exchange_sends_nothing_to_the_bot_s_venue() {
    let (_d, o, s, server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let kraken = venue(c, "Exchanges::Kraken", "Kraken");
    let mut b = browser(&app, c, s.user_id);
    let bot = seed::insert_bot(c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("DELETE FROM exchange_account_bots WHERE bot_id = ?1", [bot]).unwrap();
    c.execute("UPDATE bots SET exchange_id = ?1 WHERE id = ?2", [kraken, bot]).unwrap();
    let sheet = b.get(&app, &format!("/bots/{bot}/add_api_key/new")).await;
    assert_eq!(sheet.status, 200, "{}", sheet.body);
    assert!(sheet.body.contains("Kraken"), "Kraken's sheet: {}", sheet.body);
    let token = sheet_token(&sheet.body);
    c.execute("UPDATE bots SET exchange_id = ?1 WHERE id = ?2", [s.exchange_id, bot]).unwrap();
    let (before, stored) = (account_reads(&server).await, keys(c, &cipher, s.exchange_id));
    let stale = b.send(&app, "POST", &format!("/bots/{bot}/add_api_key"), Some(&[("authenticity_token", token.as_str()), ("api_key[key]", "kraken-key"), ("api_key[secret]", "kraken-secret")]),
                       common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 404, "{}", stale.body);
    assert_eq!((account_reads(&server).await, keys(c, &cipher, s.exchange_id)), (before, stored), "nothing sent to Alpaca, nothing saved");
    let fresh = b.get(&app, &format!("/bots/{bot}/add_api_key/new")).await;
    let token = sheet_token(&fresh.body);
    let saved = b.send(&app, "POST", &format!("/bots/{bot}/add_api_key"), Some(&[("authenticity_token", token.as_str()), ("api_key[key]", "alpaca-key"), ("api_key[secret]", "alpaca-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert_ne!(saved.status, 404, "the sheet drawn for Alpaca saves: {}", saved.body);
    assert!(account_reads(&server).await > before, "Alpaca validates the Alpaca sheet's key");
}

/// The token of the page's bot key sheet form (the shell's own forms carry others).
fn sheet_token(body: &str) -> String {
    regex::Regex::new(r#"action="[^"]*/bots/\d+/add_api_key"[^>]*><input type="hidden" name="authenticity_token" value="([^"]+)""#).unwrap().captures(body).unwrap_or_else(|| panic!("sheet form: {body}"))[1].to_string()
}
/// How many account reads Alpaca's key check has been sent.
async fn account_reads(server: &MockServer) -> usize { server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/v2/account").count() }

/// The withdrawal wizard's asset step, posted to the action its page drew (`pick_asset`, naming its pick).
async fn pick_asset(app: &deltabadger::web::App, b: &mut common::web::Browser, asset: &str) -> common::web::Answer {
    let form = b.get(app, "/rules/withdrawals/pick_asset/new").await;
    let action = asset_action(&form.body);
    b.send(app, "POST", &action, Some(&[("bots_dca_single_asset[asset_id]", asset)]), common::web::Csrf::Header, &[]).await
}
fn asset_action(body: &str) -> String {
    regex::Regex::new(r#"action="([^"]*/rules/withdrawals/pick_asset(?:\?[^"]*)?)""#).unwrap().captures(body).unwrap_or_else(|| panic!("asset form: {body}"))[1].replace("&amp;", "&")
}

/// Rule S7 (Codex branch r4, wizard.rs:164): the withdrawal wizard's asset step checks its pick before it writes. Account
/// 2's asset form left open, another tab picks account 1 and gets to its confirmation; account 2's form submitted then
/// is refused and account 1's asset and address stay.
#[tokio::test(flavor = "current_thread")]
async fn s7a_s7_a_stale_withdrawal_asset_form_writes_nothing_into_the_session() {
    let (_d, o, s, second, _server, app) = gemini_two_accounts().await;
    let c = &o.primary;
    let first = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let exchange = s.exchange_id.to_string();
    let pick = |account: i64| format!("/rules/withdrawals/pick_exchange?account_id={account}");
    let btc = s.btc.to_string();
    b.get(&app, "/rules/withdrawals/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", &pick(second), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let stale_action = asset_action(&b.get(&app, "/rules/withdrawals/pick_asset/new").await.body);
    // Tab B picks account 1, its asset, and reaches its confirmation (its address chosen).
    assert_eq!(b.send(&app, "POST", &pick(first), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(pick_asset(&app, &mut b, &btc).await.status, 302);
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    assert_eq!(b.get(&app, "/rules/withdrawals/confirm_settings/new").await.status, 200);
    let stale = b.send(&app, "POST", &stale_action, Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    assert_eq!(stale.status, 409, "{}", stale.body);
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert_eq!(confirm.status, 200, "account 1's address stays: {:?}", confirm.headers);
}

/// Rule Q1 (with P1): the save that would make an exchange's second account first reads every key of the exchange that has
/// no UID read of its current material. A key 2.x left on account 1, unread, that already booked a ledger row, is read
/// first: it opens B, so it is fenced on A's account, and the save makes account 2. When that read fails, the save is
/// refused, retryable, and allocates nothing.
#[tokio::test(flavor = "current_thread")]
async fn q1_a_second_account_forms_only_once_every_other_key_was_read() {
    for (unread, readable) in [("key-b", true), ("key-x", false)] {
        let (_d, o, user, exchange, _server, app) = kucoin().await;
        let (c, cipher) = (&o.primary, seed::cipher());
        let mut b = browser(&app, c, user);
        assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, 1, 'now', 'now')",
                  rusqlite::params![user, exchange, cipher.encrypt(unread), cipher.encrypt("secret"), cipher.encrypt("phrase")]).unwrap();
        let stray = c.last_insert_rowid();
        c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, created_at, updated_at)
                   VALUES (?1, ?2, ?3, 0, 'BTC', 1, '2026-09-01', 'now', 'now')", [user, stray, exchange]).unwrap();
        assert!(deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&stray), "{unread}: one account: it syncs, as on main");
        let mut answer = save(&app, &mut b, exchange, None, &kucoin_fields("key-b2")).await;
        if readable {
            // Rule M2: the stray key read as B's on account 1, which holds history of B: asked; "add anyway".
            assert!(answer.body.contains("already holds history of this KuCoin account"), "{}", answer.body);
            let add = answer.body.split("name=\"key_answer\" value=\"").skip(1).map(|v| v.split('"').next().unwrap_or_default()).find(|v| v.starts_with("n!")).unwrap().to_string();
            let mut fields = kucoin_fields("key-b2").to_vec();
            fields.push(("key_answer", add.as_str()));
            answer = save(&app, &mut b, exchange, None, &fields).await;
            assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
            assert_eq!(numbers(c, exchange), [1, 2]);
            assert!(!deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&stray), "read as B's: fenced on A's account");
            assert!(!identity::unread(c, stray).unwrap(), "read before account 2 formed");
        } else {
            assert_eq!(answer.status, 422, "{}", answer.body);
            assert!(answer.body.contains("Deltabadger is still checking your other KuCoin key. Try again in a minute."), "{}", answer.body);
            assert_eq!(numbers(c, exchange), [1], "nothing allocated");
            assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (1, unread.into())]);
        }
    }
}

/// A read-only key `key` of the KuCoin fixture placed on `account` as 2.x would leave it: no account read.
fn unread_key(c: &Connection, user: i64, exchange: i64, account: i64, key: &str) -> i64 {
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, ?5, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt(key), cipher.encrypt("secret"), cipher.encrypt("phrase")]).unwrap();
    let id = c.last_insert_rowid();
    identity::Account::by_id(c, account).unwrap().unwrap().add_key(c, id).unwrap();
    assert!(identity::unread(c, id).unwrap());
    id
}

/// Rule Q1 extended (slice 9 review b): on an exchange with two accounts, a Reconnect first reads the unread key it would
/// replace. Account 1's read-only key (A's, never read) and a keyless account 2: Reconnecting account 1's slot with C's key
/// reads A's key first, so C's key never lands on A's history; A's slot is untouched.
#[tokio::test(flavor = "current_thread")]
async fn q1_a_reconnect_reads_the_unread_key_it_would_replace() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let a = unread_key(c, user, exchange, first, "key-a");
    identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &kucoin_fields("key-c")).await;
    assert!(!identity::unread(c, a).unwrap(), "A's key was read first");
    let on_first: Vec<String> = keys(c, &cipher, exchange).into_iter().filter(|(n, _)| *n == 1).map(|(_, k)| k).collect();
    assert_eq!(on_first, ["key-a"], "A's slot untouched: {} {}", answer.status, answer.body);
}

/// Rule Q1 for every new account (slice 9 review c): a tile save that would make account 3 while account 2's key is unread
/// reads it first; another key of account 2's owner then lands on 2, and the exchange keeps two accounts.
#[tokio::test(flavor = "current_thread")]
async fn q1_a_third_account_forms_only_once_the_second_accounts_key_was_read() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    unread_key(c, user, exchange, second, "key-b");
    let answer = save(&app, &mut b, exchange, None, &kucoin_fields("key-b2")).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(numbers(c, exchange), [1, 2], "no third account");
    assert!(keys(c, &cipher, exchange).contains(&(2, "key-b2".into())), "{:?}", keys(c, &cipher, exchange));
}

/// Rule Q1 (Codex branch r5, keys.rs:76): each read a save makes first wakes the reader it moved, even when a later read
/// of the same save fails. Account 1's unread key reads A and becomes its reader; account 2's unread key fails: the save is
/// refused, and account 1's ledger and balance syncs are woken all the same.
#[tokio::test(flavor = "current_thread")]
async fn q1_a_read_before_a_failed_one_wakes_the_reader_it_moved() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let a = unread_key(c, user, exchange, first, "key-a");
    let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    unread_key(c, user, exchange, second, "key-fail");
    deltabadger::sync::settle_readers_of_key(c, a).unwrap();
    assert!(!deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&a), "unread: no reader");
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, None, &kucoin_fields("key-c")).await;
    assert_eq!(answer.status, 422, "the second read failed: {}", answer.body);
    assert!(!identity::unread(c, a).unwrap() && deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&a), "A's key read and reading");
    let pending = app.job_wakers().unwrap().pending();
    for job in [deltabadger::sync::jobs::LEDGER_SYNC, deltabadger::sync::jobs::BALANCE_SYNC] {
        assert!(pending.contains(&(job.to_string(), Some(a.to_string()))), "{job} is woken for the new reader: {pending:?}");
    }
}

/// Rule N3: the exact key reposted through a venue tile goes to its own account, even where no read has set that
/// account's fingerprint (a reset), never to a new one.
#[tokio::test(flavor = "current_thread")]
async fn n3_the_exact_key_reposted_on_a_tile_is_its_own_accounts() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    c.execute_batch("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL; DELETE FROM exchange_account_credentials; DELETE FROM account_fingerprint_proofs;").unwrap();
    let again = save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await;
    assert!(again.status < 400, "{}: {}", again.status, again.body);
    assert_eq!((numbers(c, exchange), keys(c, &cipher, exchange)), (vec![1], vec![(1, "key-a".into())]));
}

// ---- #710 7a part 2: an import or key save that can see a problem warns before writing; the user's answer decides ----

/// Rules G1/J2: a Binance.US key that is no reading key (invalid) observed `uid`: a Binance upload of it asks the venue.
fn seen_on_us(c: &Connection, user: i64, us: i64, uid: &str) {
    let key = key_on(c, user, us, None);
    identity::record_copy(c, &seed::cipher(), key, "Exchanges::BinanceUs", &identity::AccountId::parse(uid).unwrap(), chrono::Utc::now()).unwrap();
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [key]).unwrap();
}
fn observe_import(c: &Connection, user: i64, exchange: i64, account: i64, uid: &str) {
    identity::record_import_to(c, &seed::cipher(), user, exchange, Some(account), Some(&identity::AccountId::parse(uid).unwrap()), chrono::Utc::now()).unwrap();
}

/// Rule M1(b): another account holds the export's account (an import observation of it): the upload warns before writing,
/// that account offered first. Cancel writes nothing; an answer whose state changed meanwhile (a new account) is asked
/// again, writing nothing; the fresh "import to a new account" makes the account and writes there.
#[tokio::test(flavor = "current_thread")]
async fn m1_a_file_another_account_holds_warns_and_lands_where_answered() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    observe_import(c, s.user_id, kucoin, first, "444");
    let mut b = browser(&app, c, s.user_id);
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(asked.body.contains("may already hold this export&#39;s history") || asked.body.contains("may already hold this export's history"), "{}", asked.body);
    assert_eq!(options(&asked.body), [format!("a{first}"), format!("n{kucoin}")], "{}", asked.body);
    assert_eq!((one::<i64>(c, "SELECT count(*) FROM account_transactions"), numbers(c, kucoin)), (ledger, vec![1]), "nothing written");
    let cancelled = cancel(&app, &mut b, &asked).await;
    assert!(cancelled.body.contains("Nothing imported."), "{}", cancelled.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let again = answer(&app, &mut b, &asked, &format!("n{kucoin}")).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!((one::<i64>(c, "SELECT count(*) FROM account_transactions"), numbers(c, kucoin)), (ledger, vec![1, 2]), "nothing written");
    let done = answer(&app, &mut b, &again, &format!("n{kucoin}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(numbers(c, kucoin), [1, 2, 3]);
    let third = identity::accounts_of(c, s.user_id, kucoin).unwrap()[2].0;
    assert_eq!(rows_on(c, first), 0);
    assert!(rows_on(c, third) > 0, "on the account the answer made");
}

/// Rules M1(a)/N5: the destination's own key reads another account (its observation, no fingerprint): warned, and "import
/// here anyway" writes there. A clean upload of the same UID afterwards asks nothing more than that warning again.
#[tokio::test(flavor = "current_thread")]
async fn n5_a_file_unlike_the_destination_s_key_warns_and_imports_on_the_answer() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let key = key_on(c, s.user_id, kucoin, None);
    identity::record_copy(c, &seed::cipher(), key, "Exchanges::Kucoin", &identity::AccountId::parse("999").unwrap(), chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(asked.body.contains("This export is from another account than"), "{}", asked.body);
    assert_eq!(options(&asked.body), [format!("a{first}")], "{}", asked.body);
    assert_eq!(rows_on(c, first), 0, "nothing written");
    let done = answer(&app, &mut b, &asked, &format!("a{first}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert!(rows_on(c, first) > 0);
}

/// Rule N4 (Codex branch r4 #6): the warning's answer binds the fingerprints it compared, not only whether some matched or
/// differed. A further foreign UID observed on the destination while "import here anyway" is open: asked again, nothing
/// written.
#[tokio::test(flavor = "current_thread")]
async fn n4_a_new_foreign_observation_makes_an_open_import_warning_stale() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let key = key_on(c, s.user_id, kucoin, None);
    identity::record_copy(c, &seed::cipher(), key, "Exchanges::Kucoin", &identity::AccountId::parse("999").unwrap(), chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert_eq!(options(&asked.body), [format!("a{first}")], "{}", asked.body);
    // An import observation of a further UID on the destination (its keys, the slot, unchanged).
    observe_import(c, s.user_id, kucoin, first, "888");
    let again = answer(&app, &mut b, &asked, &format!("a{first}")).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(rows_on(c, first), 0, "nothing written");
}

/// Rules P5/Q6 (exact ids, no fuzzy matching): a Deltabadger CSV whose rows exist on another live account (by venue id, and
/// an id-less row repeating one there to the second), or whose venue id goes to two accounts in one upload, is counted
/// and asked; "import anyway" writes it.
#[tokio::test(flavor = "current_thread")]
async fn p5_rows_another_account_holds_are_counted_before_writing() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let v = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let header = format!("{},account", deltabadger::web::tracker::gap::CSV_HEADER);
    let first = format!("{header}\n2023-01-05T10:00:00Z,deposit,BTC,0.25,,,,,{v},T1,,,1\n");
    assert!(imported(&import_confirmed(&app, &mut b, &first).await));
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let second = format!("{header}\n2023-01-05T10:00:00Z,deposit,BTC,0.25,,,,,{v},T1,,,2\n2023-01-05T10:00:00Z,deposit,BTC,0.25,,,,,{v},,,,2\n\
                          2023-01-06T10:00:00Z,deposit,ETH,1,,,,,{v},T2,,,1\n2023-01-06T10:00:00Z,deposit,ETH,1,,,,,{v},T2,,,2\n");
    let asked = upload(&app, &mut b, &second).await;
    // #710 8a rules R12/R13: the two stored rows per account, the venue id sent to two accounts on its own line.
    assert!(asked.body.contains("Rows already on Alpaca<sup>1</sup>: 2."), "{}", asked.body);
    assert!(asked.body.contains("Rows this file sends to more than one account: 1."), "{}", asked.body);
    assert_eq!(options(&asked.body), ["r"], "{}", asked.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
    let done = answer(&app, &mut b, &asked, "r").await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert!(rows_on(c, accounts[1]) >= 2, "account 2's rows written");
}

/// Rule M2: a tile save that would make a new account for an exchange account another live account holds history of (its
/// import observation) asks first, the typed key kept (the answer goes to the form's question slot). Cancel saves nothing;
/// a stale answer is asked again; "add anyway" makes account 2.
#[tokio::test(flavor = "current_thread")]
async fn m2_a_new_account_another_one_holds_history_of_is_asked_first() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    let first = identity::accounts_of(c, user, exchange).unwrap()[0].0;
    observe_import(c, user, exchange, first, UID_B);
    let asked = save(&app, &mut b, exchange, None, &kucoin_fields("key-b")).await;
    assert_eq!(asked.status, 422, "{}", asked.body);
    assert!(asked.body.contains("api_key_question") && asked.body.contains("already holds history of this KuCoin account"), "{}", asked.body);
    assert_eq!(numbers(c, exchange), [1], "nothing made");
    let values: Vec<String> = regex::Regex::new(r#"name="key_answer" value="([^"]+)""#).unwrap().captures_iter(&asked.body).map(|m| m[1].replace("&#33;", "!")).collect();
    let add = values.iter().find(|v| v.starts_with("n!w")).unwrap_or_else(|| panic!("{}", asked.body)).clone();
    assert!(values.iter().any(|v| v == &format!("a{first}")) || values.iter().any(|v| v.starts_with(&format!("a{first}!w"))), "connect to account 1 offered: {values:?}");
    let with = |answer: &str| { let mut f: Vec<(&str, &str)> = kucoin_fields("key-b").to_vec(); f.push(("key_answer", answer)); f.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>() };
    let cancelled = { let f = with("x"); let refs: Vec<(&str, &str)> = f.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(); save(&app, &mut b, exchange, None, &refs).await };
    assert!(cancelled.body.contains("api_key_question") && !cancelled.body.contains("already holds history"), "{}", cancelled.body);
    assert_eq!(numbers(c, exchange), [1]);
    // An account made meanwhile (the new account's slot changed): the answer is asked again.
    identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let stale = { let f = with(&add); let refs: Vec<(&str, &str)> = f.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(); save(&app, &mut b, exchange, None, &refs).await };
    assert!(stale.body.contains("Something changed since you answered."), "{}", stale.body);
    assert_eq!(numbers(c, exchange), [1, 2]);
    let fresh: String = regex::Regex::new(r#"name="key_answer" value="(n[^"]+)""#).unwrap().captures(&stale.body).unwrap()[1].replace("&#33;", "!");
    let added = { let f = with(&fresh); let refs: Vec<(&str, &str)> = f.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(); save(&app, &mut b, exchange, None, &refs).await };
    assert!(added.status < 400, "{}: {}", added.status, added.body);
    assert_eq!(numbers(c, exchange), [1, 2, 3]);
}

/// Rule M2 (Codex branch r5, keys.rs:98): "connect to account 1 instead", posted after the observation that made account 1
/// a candidate went, is asked again: the save never falls through to a new account the user did not choose.
#[tokio::test(flavor = "current_thread")]
async fn m2_an_account_answer_whose_evidence_went_is_asked_again() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    let first = identity::accounts_of(c, user, exchange).unwrap()[0].0;
    observe_import(c, user, exchange, first, UID_B);
    let asked = save(&app, &mut b, exchange, None, &kucoin_fields("key-b")).await;
    let values: Vec<String> = regex::Regex::new(r#"name="key_answer" value="([^"]+)""#).unwrap().captures_iter(&asked.body).map(|m| m[1].replace("&#33;", "!")).collect();
    let to_first = values.iter().find(|v| v.starts_with(&format!("a{first}!w"))).unwrap_or_else(|| panic!("{}", asked.body)).clone();
    c.execute("DELETE FROM exchange_account_imports WHERE account_id = ?1", [first]).unwrap();
    let mut f: Vec<(&str, &str)> = kucoin_fields("key-b").to_vec();
    f.push(("key_answer", to_first.as_str()));
    let again = save(&app, &mut b, exchange, None, &f).await;
    assert_eq!(again.status, 422, "{}", again.body);
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(numbers(c, exchange), [1], "no account the user did not choose");
}

/// Rule M2: a Reconnect of an account whose read fingerprint is another (proven) one, nobody holding it: asked, not made a
/// new account silently.
#[tokio::test(flavor = "current_thread")]
async fn m2_a_reconnect_onto_another_account_is_asked() {
    let (_d, o, user, exchange, _server, app) = kucoin().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &kucoin_fields("key-a")).await.status < 400);
    let first = identity::accounts_of(c, user, exchange).unwrap()[0].0;
    let asked = save(&app, &mut b, exchange, Some(first), &kucoin_fields("key-b")).await;
    assert_eq!(asked.status, 422, "{}", asked.body);
    assert!(asked.body.contains("This key opens another KuCoin account than"), "{}", asked.body);
    assert_eq!(numbers(c, exchange), [1]);
}

// ---- #710 8a: the import dialog names the account ----

/// The input records of a KuCoin export (rule R7: what "rows in file" counts).
fn kucoin_records(text: &str) -> usize { text.lines().skip(1).filter(|l| !l.trim().is_empty()).count() }
/// A KuCoin export of `uid` holding only the fixture's records `lines` (1-based).
fn kucoin_part(uid: &str, lines: std::ops::RangeInclusive<usize>) -> String {
    let all = kucoin_file(uid);
    let mut rows = all.lines();
    let header = rows.next().unwrap();
    std::iter::once(header).chain(rows.enumerate().filter(|(i, _)| lines.contains(&(i + 1))).map(|(_, l)| l)).collect::<Vec<_>>().join("\n") + "\n"
}
/// A Deltabadger CSV of `rows`: (exchange name id, tx id, coin, day of January 2023, account number).
fn ledger_csv(rows: &[(&str, &str, &str, u32, u32)]) -> String {
    let lines: Vec<String> = rows.iter().map(|(v, tx, coin, day, n)| format!("2023-01-{day:02}T10:00:00Z,deposit,{coin},1,,,,,{v},{tx},,,{n}")).collect();
    format!("{},account\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, lines.join("\n"))
}
/// A Kraken of `n` accounts for `user`: number 1 keyed, the rest keyless. Their ids, by number.
fn krakens(c: &Connection, user: i64, n: usize) -> (i64, Vec<i64>) {
    let kraken = venue(c, "Exchanges::Kraken", "Kraken");
    key_on(c, user, kraken, None);
    let mut ids = vec![identity::Account::first(c, user, kraken).unwrap().id.unwrap()];
    for _ in 1..n { ids.push(identity::new_account(c, user, kraken, chrono::Utc::now()).unwrap()); }
    (kraken, ids)
}
/// Upload `text`; a question is answered with what it offers last (a CSV's sealed answer, a native file's destination).
async fn import_confirmed(app: &deltabadger::web::App, b: &mut common::web::Browser, text: &str) -> common::web::Answer {
    let asked = upload(app, b, text).await;
    if imported(&asked) { return asked; }
    let value = full_options(&asked.body).last().cloned().unwrap_or_else(|| panic!("asked nothing: {}", asked.body));
    answer(app, b, &asked, &value).await
}

/// R1/R6/R7: a Binance file whose UID is account 2's asks before writing, one named button per destination with its rows
/// in file, no selector; nothing is written until it is clicked, and the click writes on account 2.
#[tokio::test(flavor = "current_thread")]
async fn a_native_file_matched_to_binance_2_asks_with_named_buttons() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let binance = venue(c, "Exchanges::Binance", "Binance");
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Binance", "222");
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(options(&asked.body), [format!("a{second}")], "{}", asked.body);
    assert!(under(&asked.body, &format!("a{second}!")).contains("Import to Binance<sup>2</sup> (rows in file: 1)"), "{}", asked.body);
    assert!(!asked.body.contains("data-value") && asked.body.contains("name=\"cancel\""), "buttons and Cancel, no selector: {}", asked.body);
    assert_eq!((rows_on(c, first), rows_on(c, second)), (0, 0), "nothing written yet");
    let done = answer(&app, &mut b, &asked, &format!("a{second}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, first), rows_on(c, second)), (0, 1));
}

/// R3/R5/R6: each destination shows its own warnings; with a new account offered, KuCoin 1 shows as KuCoin¹ beside the new
/// KuCoin². A click validates against the offer rebuilt under the import's lock and writes what its button showed, with no
/// second question.
#[tokio::test(flavor = "current_thread")]
async fn each_destination_shows_its_own_warnings_and_the_default_writes_what_it_shows() {
    for pick_new in [false, true] {
        let (_d, o, s, _server, app) = alpaca().await;
        let c = &o.primary;
        let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
        key_on(c, s.user_id, kucoin, None);
        let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
        fingerprint(c, first, "Exchanges::Kucoin", "111");
        observe_import(c, s.user_id, kucoin, first, "444");
        let mut b = browser(&app, c, s.user_id);
        let file = kucoin_file("444");
        let n = kucoin_records(&file);
        let asked = upload(&app, &mut b, &file).await;
        assert_eq!(options(&asked.body), [format!("a{first}"), format!("n{kucoin}")], "{}", asked.body);
        let here = under(&asked.body, &format!("a{first}!"));
        assert!(here.contains(&format!("Import to KuCoin<sup>1</sup> (rows in file: {n})")), "{here}");
        assert!(here.contains("This export is from another account than KuCoin<sup>1</sup>"), "its own warning: {here}");
        let new = under(&asked.body, &format!("n{kucoin}!"));
        assert!(new.contains(&format!("Import to a new account, KuCoin<sup>2</sup> (rows in file: {n})")), "{new}");
        assert!(new.contains("KuCoin<sup>1</sup> may already hold this export") && !new.contains("This export is from another account"), "{new}");
        let done = answer(&app, &mut b, &asked, &if pick_new { format!("n{kucoin}") } else { format!("a{first}") }).await;
        assert!(imported(&done), "no second question: {}: {}", done.status, done.body);
        if pick_new {
            assert_eq!(numbers(c, kucoin), [1, 2]);
            assert_eq!(rows_on(c, first), 0);
            assert!(rows_on(c, identity::accounts_of(c, s.user_id, kucoin).unwrap()[1].0) > 0);
        } else {
            assert_eq!(numbers(c, kucoin), [1], "no account made");
            assert!(rows_on(c, first) > 0);
        }
    }
}

/// R3/R4: the account question's button shows the rows another account already holds, and its click accepts them (no
/// second question). Another of the file's rows landing on that account between preview and click asks again.
#[tokio::test(flavor = "current_thread")]
async fn an_account_pick_shows_rows_already_held_and_accepts_them_on_click() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let third = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    // One deposit of the export on account 2 (its UID 444 observed there).
    let asked = upload(&app, &mut b, &kucoin_part("444", 2..=2)).await;
    let done = answer(&app, &mut b, &asked, &format!("a{second}~{first}-{second}-{third}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    // The whole export of another UID: KuCoin row ids carry no UID, so account 2 holds one of its rows.
    let set = format!("{first}-{third}");
    let asked = upload(&app, &mut b, &kucoin_file("888")).await;
    assert_eq!(options(&asked.body), [format!("a{first}~{set}"), format!("a{third}~{set}"), format!("n{kucoin}~{set}")], "{}", asked.body);
    assert!(under(&asked.body, &format!("a{first}~")).contains("Rows already on KuCoin<sup>2</sup>: 1."), "{}", asked.body);
    // Another record lands on account 2 meanwhile: the pick is asked again, nothing written.
    let more = import_confirmed(&app, &mut b, &kucoin_part("444", 3..=3)).await;
    assert!(imported(&more), "{}: {}", more.status, more.body);
    let again = answer(&app, &mut b, &asked, &format!("a{first}~{set}")).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(rows_on(c, first), 0, "nothing written");
    assert!(under(&again.body, &format!("a{first}!")).contains("Rows already on KuCoin<sup>2</sup>: 2."), "{}", again.body);
    let done = answer(&app, &mut b, &again, &format!("a{first}")).await;
    assert!(imported(&done), "accepted on the click: {}: {}", done.status, done.body);
    assert!(rows_on(c, first) > 0);
}

/// R7: a KuCoin export to KuCoin 2 counts its source records, also when the writer adds nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_kucoin_export_to_kucoin_2_counts_its_source_rows() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    observe_import(c, s.user_id, kucoin, second, "444");
    let mut b = browser(&app, c, s.user_id);
    let file = kucoin_file("444");
    let label = format!("Import to KuCoin<sup>2</sup> (rows in file: {})", kucoin_records(&file));
    for again in [false, true] {
        let asked = upload(&app, &mut b, &file).await;
        assert!(under(&asked.body, &format!("a{second}!")).contains(&label), "{label}: {}", asked.body);
        let done = answer(&app, &mut b, &asked, &format!("a{second}")).await;
        if again { assert!(done.body.contains("Every transaction in this file is already in your history."), "{}", done.body) } else { assert!(imported(&done), "{}", done.body) }
    }
}

/// R7: a Binance fill on three records (buy, spend, fee) is one entry and three rows in file.
#[tokio::test(flavor = "current_thread")]
async fn a_three_record_binance_fill_counts_three_rows_in_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let binance = venue(c, "Exchanges::Binance", "Binance");
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Binance", "222");
    let file = "User ID,Time,Account,Operation,Coin,Change,Remark\n222,2026-08-01 10:00:46,Spot,Transaction Buy,BTC,0.001,\n\
                222,2026-08-01 10:00:46,Spot,Transaction Spend,USDT,-60,\n222,2026-08-01 10:00:46,Spot,Transaction Fee,BTC,-0.000001,\n";
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, file).await;
    assert!(under(&asked.body, &format!("a{second}!")).contains("(rows in file: 3)"), "{}", asked.body);
    let done = answer(&app, &mut b, &asked, &format!("a{second}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, second), 1, "one entry");
}

/// R1/R9: an automatic new account previews the number it will take (3; 4 when 3 is a tombstone), and the confirm binds it:
/// a number taken and tombstoned between preview and confirm asks again, writing nothing.
#[tokio::test(flavor = "current_thread")]
async fn an_automatic_new_account_previews_and_binds_its_number() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Kucoin", "222");
    let tombstone = |c: &Connection| { let id = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap(); c.execute("UPDATE exchange_accounts SET removed_at = '2026-01-01' WHERE id = ?1", [id]).unwrap(); };
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("333")).await;
    assert!(under(&asked.body, &format!("n{kucoin}!")).contains("a new account, KuCoin<sup>3</sup>"), "{}", asked.body);
    tombstone(c);
    let asked = upload(&app, &mut b, &kucoin_file("333")).await;
    assert!(under(&asked.body, &format!("n{kucoin}!")).contains("a new account, KuCoin<sup>4</sup>"), "{}", asked.body);
    let (ledger, rows) = (one::<i64>(c, "SELECT count(*) FROM account_transactions"), one::<i64>(c, "SELECT count(*) FROM exchange_accounts"));
    tombstone(c);
    let again = answer(&app, &mut b, &asked, &format!("n{kucoin}")).await;
    assert!(again.body.contains("Something changed since you answered.") && again.body.contains("a new account, KuCoin<sup>5</sup>"), "{}", again.body);
    assert_eq!((one::<i64>(c, "SELECT count(*) FROM account_transactions"), one::<i64>(c, "SELECT count(*) FROM exchange_accounts")), (ledger, rows + 1), "nothing written");
}

/// R6/R7: the account question names each account and the new one, each with the file's rows.
#[tokio::test(flavor = "current_thread")]
async fn an_explicit_pick_shows_counts() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let file = kucoin_file("444");
    let n = kucoin_records(&file);
    let asked = upload(&app, &mut b, &file).await;
    assert!(asked.body.contains("This file could be from more than one account here. Which one?"), "{}", asked.body);
    assert!(!asked.body.contains("data-value"), "buttons, no selector: {}", asked.body);
    assert!(under(&asked.body, &format!("a{first}~")).contains(&format!("Import to KuCoin<sup>1</sup> (rows in file: {n})")), "{}", asked.body);
    assert!(under(&asked.body, &format!("a{second}~")).contains(&format!("Import to KuCoin<sup>2</sup> (rows in file: {n})")), "{}", asked.body);
    assert!(under(&asked.body, &format!("n{kucoin}~")).contains(&format!("Import to a new account, KuCoin<sup>3</sup> (rows in file: {n})")), "{}", asked.body);
}

/// R2: a CSV over an exchange with two accounts lists where its rows go and posts one sealed value through Import.
#[tokio::test(flavor = "current_thread")]
async fn a_destination_only_csv_posts_its_sealed_value_through_import() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, s.exchange_id).unwrap().into_iter().map(|(id, _)| id).collect();
    let v = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let asked = upload(&app, &mut b, &ledger_csv(&[(&v, "T1", "BTC", 5, 1), (&v, "T2", "BTC", 6, 2)])).await;
    let value = hidden(&asked.body);
    assert!(!asked.body.contains("data-value") && asked.body.contains("value=\"Import\"") && asked.body.contains("name=\"cancel\""), "{}", asked.body);
    assert!(asked.body.contains("Rows going to Alpaca<sup>1</sup>: 1") && asked.body.contains("Rows going to Alpaca<sup>2</sup>: 1"), "{}", asked.body);
    assert_eq!((rows_on(c, accounts[0]), rows_on(c, accounts[1])), (0, 0), "nothing written yet");
    let done = answer(&app, &mut b, &asked, &value).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, accounts[0]), rows_on(c, accounts[1])), (1, 1));
}

/// R2: each destination account with its count.
#[tokio::test(flavor = "current_thread")]
async fn a_deltabadger_csv_over_kraken_1_and_2_lists_each_with_its_count() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let (_, ids) = krakens(c, s.user_id, 2);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &ledger_csv(&[("kraken", "T1", "BTC", 5, 1), ("kraken", "T2", "BTC", 6, 2), ("kraken", "T3", "ETH", 7, 2)])).await;
    assert!(asked.body.contains("Rows going to Kraken<sup>1</sup>: 1") && asked.body.contains("Rows going to Kraken<sup>2</sup>: 2"), "{}", asked.body);
    let done = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, ids[0]), rows_on(c, ids[1])), (1, 2));
}

/// R11: "import the rest" binds the accounts it would write: an empty Kraken 2 made while it is open asks again.
#[tokio::test(flavor = "current_thread")]
async fn an_empty_kraken_2_made_while_import_the_rest_is_open_re_asks() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let (kraken, ids) = krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &ledger_csv(&[("kraken", "T1", "BTC", 5, 1), ("kraken", "T2", "BTC", 6, 2)])).await;
    assert!(asked.body.contains("value=\"Import the rest\""), "{}", asked.body);
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    identity::new_account(c, s.user_id, kraken, chrono::Utc::now()).unwrap();
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
    assert_eq!(rows_on(c, ids[0]), 0);
}

/// R12: rows another account holds are counted per account.
#[tokio::test(flavor = "current_thread")]
async fn existing_rows_are_counted_per_account() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 3);
    let mut b = browser(&app, c, s.user_id);
    let stored = import_confirmed(&app, &mut b, &ledger_csv(&[("kraken", "T1", "BTC", 5, 1), ("kraken", "T2", "ETH", 6, 2)])).await;
    assert!(imported(&stored), "{}: {}", stored.status, stored.body);
    let asked = upload(&app, &mut b, &ledger_csv(&[("kraken", "T1", "BTC", 5, 3), ("kraken", "T2", "ETH", 6, 3)])).await;
    assert!(asked.body.contains("Rows already on Kraken<sup>1</sup>: 1.") && asked.body.contains("Rows already on Kraken<sup>2</sup>: 1."), "{}", asked.body);
    assert!(!asked.body.contains("more than one account"), "{}", asked.body);
}

/// R13: a venue id this file sends to two or more accounts counts once whatever the order of its groups; a confirm whose
/// id's destinations changed since asks again; only rows stored on another account are "already on" it.
#[tokio::test(flavor = "current_thread")]
async fn rows_sent_to_more_than_one_account_count_distinct_ids() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 3);
    let mut b = browser(&app, c, s.user_id);
    let twice = ledger_csv(&[("kraken", "T", "BTC", 5, 1), ("kraken", "T", "BTC", 6, 1), ("kraken", "T", "BTC", 7, 2), ("kraken", "U", "ETH", 8, 3)]);
    let reordered = ledger_csv(&[("kraken", "T", "BTC", 5, 2), ("kraken", "T", "BTC", 6, 1), ("kraken", "T", "BTC", 7, 1), ("kraken", "U", "ETH", 8, 3)]);
    for file in [&twice, &reordered] {
        let asked = upload(&app, &mut b, file).await;
        assert!(asked.body.contains("Rows this file sends to more than one account: 1."), "{}", asked.body);
        assert!(!asked.body.contains("Rows already on"), "nothing stored: {}", asked.body);
    }
    // PLAN-710-confirm rule L10 (D-IMPORT-FILE-IMMUTABLE): the parked file edited by the confirm (its T sent to a third
    // account, its groups' sizes unchanged) is no file: nothing written, no new question.
    let asked = upload(&app, &mut b, &twice).await;
    let token = regex::Regex::new(r#"name="token" id="token" value="([0-9a-f]{16})""#).unwrap().captures(&asked.body).unwrap()[1].to_string();
    let third = ledger_csv(&[("kraken", "T", "BTC", 5, 1), ("kraken", "U", "ETH", 6, 1), ("kraken", "T", "BTC", 7, 2), ("kraken", "T", "ETH", 8, 3)]);
    std::fs::write(common::import::parked(c, &token), &third).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert!(again.status == 422 && !again.body.contains("api_key_id"), "{}", again.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
}

/// R10: on a 2.x database Rust has not migrated, a clean CSV writes at once and a warned one writes on its confirm.
#[test]
fn a_clean_upload_on_an_unmigrated_2x_db_writes_at_once_and_confirms() {
    use deltabadger::tracker::import::run::{self, Answer};
    let dir = common::v2_install_unmigrated();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let s = seed::seed_kraken(&c, &seed::cipher());
    assert!(!identity::ready(&c).unwrap());
    let import = |text: &str, answer: Option<&str>| run::import(&c, &seed::cipher(), s.user_id, "deltabadger", text.as_bytes(), None, answer, chrono::Utc::now()).unwrap();
    assert!(matches!(import(&ledger_csv(&[("kraken", "T1", "BTC", 5, 1)]), None), Answer::Written { imported: 1, .. }));
    let rest = ledger_csv(&[("kraken", "T2", "BTC", 6, 1), ("kraken", "T3", "BTC", 7, 2)]);
    let Answer::Warned(_, choices) = import(&rest, None) else { panic!("warned") };
    let done = import(&rest, Some(&choices[0].value));
    assert!(matches!(&done, Answer::Written { imported: 1, unrouted, .. } if unrouted == &["Kraken 2"]), "{done:?}");
}

/// R1: an exchange with one account writes a native file on the first response, as on main.
#[test]
fn a_single_account_upload_writes_on_the_first_response() {
    let (_d, o, s) = common::install_alpaca();
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let file = kucoin_file("444");
    let answer = deltabadger::tracker::import::run::import(c, &seed::cipher(), s.user_id, "kucoin", file.as_bytes(), None, None, chrono::Utc::now()).unwrap();
    assert!(matches!(answer, deltabadger::tracker::import::run::Answer::Written { imported, .. } if imported > 0), "{answer:?}");
}

/// R15: an unrouted CSV row naming account 2+ is named as an account is ("Alpaca 2"); a number-1 row keeps the CSV's
/// spelling, as does a row for an exchange this install has no row for.
#[tokio::test(flavor = "current_thread")]
async fn unrouted_csv_rows_name_the_exchange_or_keep_the_csv_spelling() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let v = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    // A Kraken with no account: its number-1 row keeps the CSV's spelling, as 2.x and the recorded imports name it.
    venue(c, "Exchanges::Kraken", "Kraken");
    let done = import_confirmed(&app, &mut b, &ledger_csv(&[(&v, "T1", "BTC", 5, 1), (&v, "T2", "BTC", 6, 2), ("zzz", "T3", "BTC", 7, 1), ("kraken", "T4", "BTC", 8, 1)])).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert!(done.body.contains("No account connected for: Alpaca 2, kraken, zzz."), "{}", done.body);
}

// ---- #710 8a review r1 ----

/// R5 (r1 #1): a destination named without the venue question is bound to the venue routing too: the file's UID showing
/// up on Binance.US before the click asks the venue, never writing on Binance's account the user did not pick a venue for.
#[tokio::test(flavor = "current_thread")]
async fn a_destination_whose_venue_became_ambiguous_asks_the_venue_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Binance", "222");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(options(&asked.body), [format!("a{second}")], "{}", asked.body);
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "222");
    let again = answer(&app, &mut b, &asked, &format!("a{second}")).await;
    assert_eq!(options(&again.body), [format!("v{binance}"), format!("v{us}")], "the venue asked: {}", again.body);
    assert_eq!((rows_on(c, second), rows_on(c, on_us)), (0, 0), "nothing written");
}

/// R9 (r1 #2): every button binds the number a new account would take: a number taken and tombstoned while the account
/// question is open asks again, even for a pick of an existing account.
#[tokio::test(flavor = "current_thread")]
async fn an_account_pick_binds_the_proposed_new_number() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(under(&asked.body, &format!("n{kucoin}~")).contains("KuCoin<sup>3</sup>"), "{}", asked.body);
    let third = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    c.execute("UPDATE exchange_accounts SET removed_at = '2026-01-01' WHERE id = ?1", [third]).unwrap();
    let again = answer(&app, &mut b, &asked, &format!("a{first}~{first}-{second}")).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(rows_on(c, first), 0, "nothing written");
}

/// R7 (r1 #3): a button binds the file it counted: the parked export changed after the preview asks again.
#[tokio::test(flavor = "current_thread")]
async fn a_parked_file_changed_after_the_preview_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    observe_import(c, s.user_id, kucoin, second, "444");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_part("444", 2..=2)).await;
    assert!(under(&asked.body, &format!("a{second}!")).contains("(rows in file: 1)"), "{}", asked.body);
    let token = regex::Regex::new(r#"name="token" id="token" value="([0-9a-f]{16})""#).unwrap().captures(&asked.body).unwrap()[1].to_string();
    // PLAN-710-confirm rule L10 (D-IMPORT-FILE-IMMUTABLE): no file, no new question.
    std::fs::write(common::import::parked(c, &token), kucoin_part("444", 2..=3)).unwrap();
    let again = answer(&app, &mut b, &asked, &format!("a{second}")).await;
    assert!(again.status == 422 && !again.body.contains("api_key_id"), "{}", again.body);
    assert_eq!(rows_on(c, second), 0, "nothing written");
}

/// R6 (r1 #4): the account question numbers the accounts it offers beside a new one, also when it is asked again on its
/// venue after a stale venue answer.
#[tokio::test(flavor = "current_thread")]
async fn a_stale_venue_answer_numbers_the_accounts_it_offers() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let kb = key_on(c, s.user_id, binance, None);
    let first = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    seen_on_us(c, s.user_id, us, "777");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [kb]).unwrap();
    c.execute("DELETE FROM api_keys WHERE id = ?1", [kb]).unwrap();
    let again = answer(&app, &mut b, &asked, &kb.to_string()).await;
    assert!(under(&again.body, &format!("a{first}~")).contains("Import to Binance<sup>1</sup>"), "{}", again.body);
    assert!(under(&again.body, &format!("n{binance}~")).contains("a new account, Binance<sup>2</sup>"), "{}", again.body);
}

/// R6 (r1 #5): a legacy key's rows (its account has no row yet) go to the exchange's name.
#[tokio::test(flavor = "current_thread")]
async fn a_legacy_keys_destination_is_named_by_its_exchange() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    assert!(trading_tile(&app, &mut b, s.exchange_id, "second-key").await.status < 400);
    let kraken = venue(c, "Exchanges::Kraken", "Kraken");
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, kraken, e("legacy"), e("s")]).unwrap();
    let v = deltabadger::tracker::rows::name_id("Exchanges::Alpaca");
    let asked = upload(&app, &mut b, &ledger_csv(&[("kraken", "K1", "BTC", 5, 1), (&v, "T1", "BTC", 6, 1), (&v, "T2", "BTC", 7, 2)])).await;
    assert!(asked.body.contains("Rows going to Kraken: 1."), "{}", asked.body);
}

// ---- #710 slice 9: Coinbase names its portfolio (`portfolio_uuid` of its key check's own permission read) ----

/// A Coinbase exchange of the Alpaca fixture's user. `cb-a`, `cb-a2`, `cb-a3`, `cb-unread` read portfolio A; `cb-b`, `cb-b2`
/// B; `cb-nouuid`'s permission answer names no portfolio; any other key's permission read fails (a 503). Every portfolio's
/// breakdown answers.
async fn coinbase() -> (tempfile::TempDir, store::Opened, i64, i64, MockServer, deltabadger::web::App) {
    use common::coinbase_key::{permissions, Kid, PORTFOLIO_A, PORTFOLIO_B};
    let server = MockServer::start().await;
    let perms = deltabadger::venue::coinbase::PERMISSIONS;
    for (key, uuid) in [("cb-a", PORTFOLIO_A), ("cb-a2", PORTFOLIO_A), ("cb-a3", PORTFOLIO_A), ("cb-unread", PORTFOLIO_A), ("cb-b", PORTFOLIO_B), ("cb-b2", PORTFOLIO_B)] {
        Mock::given(method("GET")).and(path(perms)).and(Kid(key)).respond_with(ResponseTemplate::new(200).set_body_json(permissions(uuid, true))).mount(&server).await;
    }
    Mock::given(method("GET")).and(path(perms)).and(Kid("cb-nouuid"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"can_view": true, "can_trade": true, "can_transfer": false}))).mount(&server).await;
    Mock::given(method("GET")).and(path(perms)).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    Mock::given(method("GET")).and(wiremock::matchers::path_regex("^/api/v3/brokerage/portfolios/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"breakdown": {"spot_positions": []}}))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let s = seed::seed_alpaca(c, &seed::cipher());
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, 'Coinbase', '0.1', '0.1', 'now', 'now')", [deltabadger::venue::coinbase::COINBASE]).unwrap();
    let exchange = c.last_insert_rowid();
    let app = web_app(dir.path(), c).with_venue_key_url(deltabadger::venue::coinbase::COINBASE, &server.uri()).unwrap();
    (dir, o, s.user_id, exchange, server, app)
}
fn coinbase_fields(key: &'static str) -> [(&'static str, &'static str); 2] { [("api_key[key]", key), ("api_key[secret]", common::coinbase_key::SECRET)] }
fn accounts(c: &Connection, user: i64, exchange: i64) -> Vec<i64> { identity::accounts_of(c, user, exchange).unwrap().into_iter().map(|(id, _)| id).collect() }
/// The permission reads `server` received signed by `key`.
async fn permission_reads(server: &MockServer, key: &'static str) -> usize {
    use wiremock::Match;
    server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == deltabadger::venue::coinbase::PERMISSIONS && common::coinbase_key::Kid(key).matches(r)).count()
}

/// Two keys of one portfolio make one account; a key of another portfolio its own (tiles); a Reconnect of account 2 with a
/// key of portfolio A lands on account 1; a key whose permission answer names no portfolio is refused on a tile (Q5).
#[tokio::test(flavor = "current_thread")]
async fn s9_coinbase_one_portfolio_is_one_account_and_another_portfolio_another() {
    let (_d, o, user, exchange, _server, app) = coinbase().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["cb-a", "cb-b", "cb-a2"] {
        let answer = save(&app, &mut b, exchange, None, &coinbase_fields(key)).await;
        assert!(answer.status < 400, "{key}: {}: {}", answer.status, answer.body);
    }
    assert_eq!(keys(c, &cipher, exchange), [(1, "cb-a2".into()), (2, "cb-b".into())], "A's second key replaced A's on account 1");
    let reconnect = save(&app, &mut b, exchange, Some(accounts(c, user, exchange)[1]), &coinbase_fields("cb-a3")).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "cb-a3".into()), (2, "cb-b".into())], "routed to A's account, account 2 untouched");
    let refused = save(&app, &mut b, exchange, None, &coinbase_fields("cb-nouuid")).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("We could not confirm which account this key belongs to"), "{}", refused.body);
    assert_eq!((numbers(c, exchange), keys(c, &cipher, exchange)), (vec![1, 2], vec![(1, "cb-a3".into()), (2, "cb-b".into())]));
}

/// Rule Q1 on Coinbase: the JSON save that would make account 2 first reads account 1's unread key (one more permission
/// read, signed by it), then routes the new key by its own portfolio read.
#[tokio::test(flavor = "current_thread")]
async fn s9_the_coinbase_json_save_reads_the_unread_key_first_and_routes_by_portfolio() {
    let (_d, o, user, exchange, server, app) = coinbase().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &coinbase_fields("cb-a")).await.status < 400);
    let first = accounts(c, user, exchange)[0];
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("cb-unread"), cipher.encrypt(common::coinbase_key::SECRET)]).unwrap();
    let unread = c.last_insert_rowid();
    identity::Account::by_id(c, first).unwrap().unwrap().add_key(c, unread).unwrap();
    assert!(identity::unread(c, unread).unwrap());
    let (exchange_s, first_s) = (exchange.to_string(), first.to_string());
    let form = [("api_key[exchange_id]", exchange_s.as_str()), ("api_key[account_id]", first_s.as_str()), ("api_key[key_type]", "read_only"),
                ("api_key[key]", "cb-b"), ("api_key[secret]", common::coinbase_key::SECRET)];
    let asked = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!(permission_reads(&server, "cb-unread").await, 1, "the unread key was read first");
    // Rule M2: account 1 was named and portfolio B is another account: asked first; "add anyway" makes account 2.
    assert_eq!(asked.status, 409, "{}", asked.body);
    let add: serde_json::Value = serde_json::from_str(&asked.body).unwrap();
    let add = add["answers"].as_array().unwrap().iter().find_map(|a| a["value"].as_str().filter(|v| v.starts_with("n!"))).unwrap().to_string();
    let mut form = form.to_vec();
    form.push(("api_key[key_answer]", add.as_str()));
    let answer = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert!(!identity::unread(c, unread).unwrap());
    assert_eq!(keys(c, &cipher, exchange), [(1, "cb-a".into()), (1, "cb-unread".into()), (2, "cb-b".into())], "portfolio B is account 2");
}

/// The bot wizard's key save sees Coinbase's portfolio: a trading key of another portfolio than account 1's makes account
/// 2, and its observation is recorded.
#[tokio::test(flavor = "current_thread")]
async fn s9_the_wizard_save_routes_a_coinbase_key_by_its_portfolio() {
    let (_d, o, user, exchange, _server, app) = coinbase().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &coinbase_fields("cb-a")).await.status < 400);
    let e = exchange.to_string();
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/bots/dca_single_assets/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", e.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let form = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(form.status, 200, "{}", location(&form));
    let answer = b.send(&app, "POST", &format!("/bots/dca_single_assets/add_api_key?exchange_id={e}&untargeted=1"), Some(&coinbase_fields("cb-b")), common::web::Csrf::Header, &[]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "cb-a".into()), (2, "cb-b".into())]);
    let trading = identity::key_of(c, accounts(c, user, exchange)[1], 0).unwrap().unwrap();
    assert_eq!(identity::credential_standing(c, trading).unwrap(), identity::CredentialStanding::Bound);
}

/// G4 (r6): a Reconnect reposting account 1's own installed key, whose permission read names A and whose breakdown then
/// fails, is refused (nothing saved) and still records the read against the installed credential's current version: the
/// key counts as read; its status and material are as they were.
#[tokio::test(flavor = "current_thread")]
async fn s9_a_refused_coinbase_repost_records_its_portfolio_on_the_installed_key() {
    let (_d, o, user, exchange, server, app) = coinbase().await;
    let c = &o.primary;
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &coinbase_fields("cb-a")).await.status < 400);
    let first = accounts(c, user, exchange)[0];
    let key = identity::key_of(c, first, 2).unwrap().unwrap();
    c.execute("DELETE FROM exchange_account_credentials WHERE api_key_id = ?1", [key]).unwrap();
    assert!(identity::unread(c, key).unwrap());
    let row = |c: &Connection| one::<String>(c, &format!("SELECT status || ':' || key || ':' || secret FROM api_keys WHERE id = {key}"));
    let before = row(c);
    Mock::given(method("GET")).and(path(format!("/api/v3/brokerage/portfolios/{}", common::coinbase_key::PORTFOLIO_A)))
        .respond_with(ResponseTemplate::new(503)).up_to_n_times(1).with_priority(1).mount(&server).await;
    let answer = save(&app, &mut b, exchange, Some(first), &coinbase_fields("cb-a")).await;
    assert!(answer.status >= 400, "{}: {}", answer.status, answer.body);
    assert!(!identity::unread(c, key).unwrap(), "the portfolio read is recorded on the installed key");
    assert_eq!(identity::credential_standing(c, key).unwrap(), identity::CredentialStanding::Bound);
    assert_eq!(row(c), before, "status and material as they were");
}

/// G4 (r4/r5) at `revalidate`: Coinbase's permission read is its validation read. A key turned correct while its recheck
/// was in flight, whose portfolio read succeeds and whose breakdown then fails (a 503), keeps its correct status; the
/// breakdown request was sent. A key that was not correct is written pending as before.
#[tokio::test(flavor = "current_thread")]
async fn s9_a_transient_coinbase_breakdown_keeps_a_correct_keys_status_at_revalidate() {
    for (flipped, expected) in [(true, 1), (false, 0)] {
        let (_d, o, user, exchange, server, app) = coinbase().await;
        let c = &o.primary;
        let mut b = browser(&app, c, user);
        assert!(save(&app, &mut b, exchange, None, &coinbase_fields("cb-a")).await.status < 400);
        let first = accounts(c, user, exchange)[0];
        let key = identity::key_of(c, first, 2).unwrap().unwrap();
        c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [key]).unwrap();
        let db = c.path().unwrap().to_string();
        Mock::given(method("GET")).and(path(deltabadger::venue::coinbase::PERMISSIONS)).and(common::coinbase_key::Kid("cb-a")).respond_with(move |_: &wiremock::Request| {
            if flipped { Connection::open(&db).unwrap().execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [key]).unwrap(); }
            ResponseTemplate::new(200).set_body_json(common::coinbase_key::permissions(common::coinbase_key::PORTFOLIO_A, true))
        }).with_priority(1).mount(&server).await;
        let breakdown = format!("/api/v3/brokerage/portfolios/{}", common::coinbase_key::PORTFOLIO_A);
        Mock::given(method("GET")).and(path(breakdown.as_str())).respond_with(ResponseTemplate::new(503)).with_priority(1).mount(&server).await;
        let breakdowns = || async { server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == breakdown).count() };
        let before = breakdowns().await;
        let page = b.get(&app, &format!("/tracker/add_api_key/new?exchange_id={exchange}&key_type=read_only&account_id={first}")).await;
        assert_eq!(breakdowns().await, before + 1, "flipped {flipped}: the recheck read the breakdown: {} {}", page.status, location(&page));
        assert_eq!(one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {key}")), expected, "flipped {flipped}");
    }
}

// ---- #710 slice 9, review r1 ----

/// A read-only Coinbase key `key` placed on `account` as 2.x would leave it: correct, never read.
fn coinbase_unread(c: &Connection, user: i64, exchange: i64, account: i64, key: &str) -> i64 {
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt(key), cipher.encrypt(common::coinbase_key::SECRET)]).unwrap();
    let id = c.last_insert_rowid();
    identity::Account::by_id(c, account).unwrap().unwrap().add_key(c, id).unwrap();
    assert!(identity::unread(c, id).unwrap());
    id
}
/// A stored duplicate standing: `duplicate` is `survivor`'s twin.
fn stand(c: &Connection, user: i64, exchange: i64, duplicate: i64, survivor: i64) {
    c.execute("INSERT INTO exchange_account_standings (duplicate_id, survivor_id, user_id, exchange_id, since, basis) VALUES (?1, ?2, ?3, ?4, '2026-09-20 10:00:00', ?5)",
              rusqlite::params![duplicate, survivor, user, exchange, identity::standing_basis(c, duplicate, survivor).unwrap()]).unwrap();
}
fn standings(c: &Connection) -> i64 { one(c, "SELECT count(*) FROM exchange_account_standings") }

/// Review r1 #1: on an exchange with one account, a Reconnect that would replace an installed key nothing has read yet
/// reads that key first: portfolio B's key never takes the slot (and the history) of portfolio A's.
#[tokio::test(flavor = "current_thread")]
async fn s9_r1_a_reconnect_of_the_sole_account_reads_its_unread_key_first() {
    let (_d, o, user, exchange, server, app) = coinbase().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let installed = coinbase_unread(c, user, exchange, first, "cb-unread");
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &coinbase_fields("cb-b")).await;
    assert_eq!(permission_reads(&server, "cb-unread").await, 1, "the installed key was read first: {} {}", answer.status, answer.body);
    assert!(!identity::unread(c, installed).unwrap());
    let on_first: Vec<String> = keys(c, &cipher, exchange).into_iter().filter(|(n, _)| *n == 1).map(|(_, k)| k).collect();
    assert_eq!(on_first, ["cb-unread"], "B's key never replaced A's: {} {}", answer.status, answer.body);
    // The same key posted again is no other key: nothing more is read for it.
    let repost = save(&app, &mut b, exchange, Some(first), &coinbase_fields("cb-unread")).await;
    assert!(repost.status < 400, "{}: {}", repost.status, repost.body);
    assert_eq!(permission_reads(&server, "cb-unread").await, 2, "one read: the repost's own");
}

/// Review r1 #2: a refused repost records its read only on the installed key as it was before the request. A save that
/// rewrote the key meanwhile (the same material, a fresh ciphertext) is a newer version: nothing is stamped on it.
#[tokio::test(flavor = "current_thread")]
async fn s9_r1_a_refused_repost_never_stamps_a_key_rewritten_meanwhile() {
    let (_d, o, user, exchange, server, app) = coinbase().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let key = coinbase_unread(c, user, exchange, first, "cb-a");
    let db = c.path().unwrap().to_string();
    Mock::given(method("GET")).and(path(format!("/api/v3/brokerage/portfolios/{}", common::coinbase_key::PORTFOLIO_A))).respond_with(move |_: &wiremock::Request| {
        let other = Connection::open(&db).unwrap();
        let cipher = seed::cipher();
        identity::write_credential(&other, &cipher, key, Some(&cipher.encrypt("cb-a")), Some(&cipher.encrypt(common::coinbase_key::SECRET)), None, chrono::Utc::now()).unwrap();
        ResponseTemplate::new(503)
    }).up_to_n_times(1).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &coinbase_fields("cb-a")).await;
    assert!(answer.status >= 400, "{}: {}", answer.status, answer.body);
    assert!(identity::unread(c, key).unwrap(), "the read belongs to the version before the rewrite");
}

/// Review r1 #3: the previously-correct rule takes only a transient non-verdict. A correct key whose recheck's first read
/// is refused (401) is written pending and its standing ends; one that answers 503 stays correct and keeps it.
#[tokio::test(flavor = "current_thread")]
async fn s9_r1_a_definite_refusal_still_demotes_a_correct_key_at_revalidate() {
    for (answer, status, standing) in [(401, 0, 0), (503, 1, 1)] {
        let (_d, o, user, exchange, server, app) = coinbase().await;
        let c = &o.primary;
        let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
        let key = coinbase_unread(c, user, exchange, first, "cb-a");
        let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
        stand(c, user, exchange, second, first);
        c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [key]).unwrap();
        let db = c.path().unwrap().to_string();
        Mock::given(method("GET")).and(path(deltabadger::venue::coinbase::PERMISSIONS)).and(common::coinbase_key::Kid("cb-a")).respond_with(move |_: &wiremock::Request| {
            // Correct while the recheck is in flight.
            Connection::open(&db).unwrap().execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [key]).unwrap();
            ResponseTemplate::new(answer).set_body_json(serde_json::json!({"error": "UNAUTHENTICATED", "code": 16, "message": "refused", "details": []}))
        }).with_priority(1).mount(&server).await;
        let mut b = browser(&app, c, user);
        b.get(&app, &format!("/tracker/add_api_key/new?exchange_id={exchange}&key_type=read_only&account_id={first}")).await;
        assert_eq!(permission_reads(&server, "cb-a").await, 1, "{answer}: the recheck ran");
        assert_eq!((one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {key}")), standings(c)), (status, standing), "{answer}");
    }
}

/// Review r1 #4: the JSON save's same-key repost of a correct Coinbase key whose permission read answers 503 neither
/// demotes it nor ends its standing.
#[tokio::test(flavor = "current_thread")]
async fn s9_r1_a_json_same_key_repost_with_a_transient_read_keeps_a_correct_key() {
    let (_d, o, user, exchange, server, app) = coinbase().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let key = coinbase_unread(c, user, exchange, first, "cb-a");
    let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    stand(c, user, exchange, second, first);
    Mock::given(method("GET")).and(path(deltabadger::venue::coinbase::PERMISSIONS)).and(common::coinbase_key::Kid("cb-a"))
        .respond_with(ResponseTemplate::new(503)).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, user);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let (exchange_s, first_s) = (exchange.to_string(), first.to_string());
    let form = [("api_key[exchange_id]", exchange_s.as_str()), ("api_key[account_id]", first_s.as_str()), ("api_key[key_type]", "read_only"),
                ("api_key[key]", "cb-a"), ("api_key[secret]", common::coinbase_key::SECRET)];
    let answer = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(permission_reads(&server, "cb-a").await, 1);
    assert_eq!((one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {key}")), standings(c)), (1, 1));
}

/// Review r1 #5: the refused-repost recording is Coinbase's (`identity::VALIDATION_READS`): a KuCoin HF trading key
/// reposted, read for its UID and refused for its missing Spot permission, stays unread, as on main.
#[tokio::test(flavor = "current_thread")]
async fn s9_r1_a_refused_kucoin_repost_leaves_its_installed_key_unread() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let c = &o.primary;
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "key-hf"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":{"uid": UID_A, "permission":"General"}}))).with_priority(1).mount(&server).await;
    Mock::given(method("GET")).and(path(deltabadger::venue::kucoin_account::OPENED))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":"200000","data":true}))).with_priority(1).mount(&server).await;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("key-hf"), cipher.encrypt("secret"), cipher.encrypt("phrase")]).unwrap();
    let key = c.last_insert_rowid();
    identity::Account::by_id(c, first).unwrap().unwrap().add_key(c, key).unwrap();
    let mut b = browser(&app, c, user);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let (exchange_s, first_s) = (exchange.to_string(), first.to_string());
    let form = [("exchange_id", exchange_s.as_str()), ("key_type", "trading"), ("account_id", first_s.as_str()),
                ("api_key[key]", "key-hf"), ("api_key[secret]", "secret"), ("api_key[passphrase]", "phrase")];
    let answer = b.send(&app, "POST", "/tracker/add_api_key", Some(&form), common::web::Csrf::Header, &[]).await;
    assert!(answer.status >= 400 || answer.body.contains("Spot"), "refused for its permission: {}: {}", answer.status, answer.body);
    assert!(server.received_requests().await.unwrap().iter().any(|r| r.url.path() == identity::KUCOIN_PATH), "the UID was read");
    assert!(identity::unread(c, key).unwrap(), "a KuCoin refusal records nothing on the installed key");
}

/// Review r1 #6: a refused repost that makes the installed key its account's reader (its first read, on an exchange with
/// two accounts) wakes that reader's ledger and balance jobs.
#[tokio::test(flavor = "current_thread")]
async fn s9_r1_a_refused_repost_wakes_the_reader_it_made() {
    let (_d, o, user, exchange, server, app) = coinbase().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let key = coinbase_unread(c, user, exchange, first, "cb-a");
    let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    coinbase_unread(c, user, exchange, second, "cb-b");
    deltabadger::sync::settle_readers_of_key(c, key).unwrap();
    assert!(!deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&key), "unread: no reader");
    Mock::given(method("GET")).and(path(format!("/api/v3/brokerage/portfolios/{}", common::coinbase_key::PORTFOLIO_A)))
        .respond_with(ResponseTemplate::new(503)).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &coinbase_fields("cb-a")).await;
    assert!(answer.status >= 400, "{}: {}", answer.status, answer.body);
    assert!(!identity::unread(c, key).unwrap() && deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&key), "read and reading");
    let pending = app.job_wakers().unwrap().pending();
    for job in [deltabadger::sync::jobs::LEDGER_SYNC, deltabadger::sync::jobs::BALANCE_SYNC] {
        assert!(pending.contains(&(job.to_string(), Some(key.to_string()))), "{job} is woken for the new reader: {pending:?}");
    }
}

// ---- #710 slice 9: Alpaca names its account (`id` of its key check's own GET /v2/account; paper and live are two) ----

/// The Alpaca fixture on distinct paper and live hosts (`common::alpaca_key::hosts`): only the mocks a test mounts answer
/// the account read; positions answer empty on both.
async fn alpaca_modes() -> (tempfile::TempDir, store::Opened, seed::Seeded, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(wiremock::matchers::path_regex("/v2/positions$")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let (dir, o) = install();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    identity::Account::first(&o.primary, s.user_id, s.exchange_id).unwrap().ensure(&o.primary, chrono::Utc::now()).unwrap();
    let app = web_app(dir.path(), &o.primary).with_settings_key_boundary(common::alpaca_key::hosts(&server.uri()), std::sync::Arc::new(Quiet)).unwrap();
    (dir, o, s, server, app)
}
/// `key`'s account read on `host` (`paper` or `live`) answers `status` with account `uuid` (a 2xx) or Alpaca's error.
async fn alpaca_reads(server: &MockServer, host: &str, key: &str, status: u16, uuid: &str) {
    let body = if (200..300).contains(&status) { common::alpaca_key::account(uuid) } else { serde_json::json!({"code": 40110000, "message": "request is not authorized"}) };
    Mock::given(method("GET")).and(path(format!("/{host}/v2/account"))).and(header("APCA-API-KEY-ID", key)).respond_with(ResponseTemplate::new(status).set_body_json(body)).mount(server).await;
}
/// The browser's JSON save of an Alpaca trading key over `fields`.
async fn alpaca_json(app: &deltabadger::web::App, b: &mut common::web::Browser, exchange: i64, fields: &[(&str, &str)]) -> common::web::Answer {
    let exchange = exchange.to_string();
    let mut form = vec![("api_key[exchange_id]", exchange.as_str()), ("api_key[key_type]", "trading")];
    form.extend_from_slice(fields);
    assert_eq!(b.get(app, "/settings/api_keys").await.status, 200);
    b.send(app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await
}
/// The (path, key) of every account read `server` received.
async fn mode_reads(server: &MockServer) -> Vec<(String, String)> {
    common::alpaca_key::sent(server).await.into_iter().filter(|(p, _)| p.ends_with("/v2/account")).collect()
}
/// Key `id`'s row as stored: status, update time and every credential ciphertext.
fn key_row(c: &Connection, id: i64) -> String {
    one(c, &format!("SELECT status || '|' || updated_at || '|' || key || '|' || secret || '|' || coalesce(passphrase, '') FROM api_keys WHERE id = {id}"))
}
fn pair(p: &str, k: &str) -> (String, String) { (p.to_string(), k.to_string()) }

/// Two keys of one Alpaca account make one account; a key of another its own (tiles); a Reconnect of account 2 with a key
/// of account A lands on account 1; a key whose account read names no account is refused on a tile (Q5).
#[tokio::test(flavor = "current_thread")]
async fn s9_alpaca_one_account_is_one_account_and_another_account_another() {
    let (_d, o, s, server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID", "pk-noid"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ACTIVE", "cash": "0"}))).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, s.user_id);
    for key in ["pk-a2", "second-key"] {
        let answer = trading_tile(&app, &mut b, s.exchange_id, key).await;
        assert!(answer.status < 400, "{key}: {}: {}", answer.status, answer.body);
    }
    assert_eq!(keys(c, &cipher, s.exchange_id), [(1, "pk-a2".into()), (2, "second-key".into())], "A's second key replaced A's on account 1");
    let second = accounts(c, s.user_id, s.exchange_id)[1];
    let reconnect = save(&app, &mut b, s.exchange_id, Some(second), &[("api_key[key]", "pk-a3"), ("api_key[secret]", "pk-a3-secret"), ("api_key[passphrase]", "paper")]).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), [(1, "pk-a3".into()), (2, "second-key".into())], "routed to A's account, account 2 untouched");
    let refused = trading_tile(&app, &mut b, s.exchange_id, "pk-noid").await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("We could not confirm which account this key belongs to"), "{}", refused.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), [(1, "pk-a3".into()), (2, "second-key".into())]);
}

/// The effective mode (r4): a JSON repost of the installed live key that names no mode keeps the slot's, so the key is read
/// on the live host once and never on the paper one, and stays live.
#[tokio::test(flavor = "current_thread")]
async fn s9_an_alpaca_json_repost_without_a_mode_reads_the_installed_live_keys_host() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    c.execute("UPDATE api_keys SET passphrase = ?1 WHERE id = ?2", rusqlite::params![cipher.encrypt("live"), s.api_key_id]).unwrap();
    alpaca_reads(&server, "live", "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await;
    alpaca_reads(&server, "paper", "PKTEST", 401, "").await;
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret")]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(mode_reads(&server).await, [pair("/live/v2/account", "PKTEST")], "one live read, no paper read");
    let mode: String = one(c, &format!("SELECT passphrase FROM api_keys WHERE id = {}", s.api_key_id));
    assert_eq!(cipher.decrypt(&mode).unwrap(), "live");
    assert!(!identity::unread(c, s.api_key_id).unwrap());
}

/// Mode-picking Connect over one server's paper and live hosts, as the venue picks them from a key's mode.
struct Modes(String);
impl deltabadger::sync::jobs::Connect for Modes {
    type T = deltabadger::venue::http::ReqwestTransport;
    fn connect(&self, c: &deltabadger::crypto::Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(), c.key.clone(), c.secret.clone()),
                                                     deltabadger::venue::alpaca::Urls::pick(c.passphrase.as_deref(), &common::alpaca_key::hosts(&self.0)))
    }
}

/// r4: an explicit `live` over the paper slot with the same key and secret is new material: it is read live (and the
/// installed paper key, unread, is read first), written live, its read recorded against the live version, and the next
/// discovery reads the live host.
#[tokio::test(flavor = "current_thread")]
async fn s9_an_alpaca_json_switch_to_live_is_read_written_and_rediscovered_live() {
    use deltabadger::jobs::{Cx, Db, Job};
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    for host in ["paper", "live"] { alpaca_reads(&server, host, "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await; }
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "live")]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(mode_reads(&server).await, [pair("/live/v2/account", "PKTEST"), pair("/paper/v2/account", "PKTEST")], "the candidate live, then the installed paper key");
    assert_eq!(keys(c, &cipher, s.exchange_id), [(1, "PKTEST".into())], "in place");
    let mode: String = one(c, &format!("SELECT passphrase FROM api_keys WHERE id = {}", s.api_key_id));
    assert_eq!(cipher.decrypt(&mode).unwrap(), "live");
    assert!(!identity::unread(c, s.api_key_id).unwrap(), "the read is the live version's");
    let clock = deltabadger::engine::FixedClock("2026-09-29T12:00:00Z".parse().unwrap());
    let cx = Cx { db: Db::new(Connection::open(c.path().unwrap()).unwrap(), cipher.clone()), clock: &clock, wakers: Default::default() };
    identity::Discovery { venues: Modes(server.uri()) }.run(cx, vec![]).await;
    assert_eq!(mode_reads(&server).await[2..], [pair("/live/v2/account", "PKTEST")], "rediscovered on the live host");
}

/// r5: a switch the live host refuses (its 401) is refused; the paper slot is as it was, byte for byte.
#[tokio::test(flavor = "current_thread")]
async fn s9_a_refused_alpaca_switch_keeps_the_paper_slot_byte_for_byte() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let c = &o.primary;
    alpaca_reads(&server, "live", "PKTEST", 401, "").await;
    alpaca_reads(&server, "paper", "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await;
    let before = key_row(c, s.api_key_id);
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "live")]).await;
    assert_eq!((answer.status, answer.body.as_str()), (422, "{\"data\":false}"));
    assert_eq!(mode_reads(&server).await, [pair("/live/v2/account", "PKTEST")]);
    assert_eq!(key_row(c, s.api_key_id), before);
}

/// r5: a switch whose live read names another account is routed as any new key (rule M2's question, then account 2), never
/// refused as a repost reading elsewhere; the paper key stays on account 1 as it was.
#[tokio::test(flavor = "current_thread")]
async fn s9_an_alpaca_switch_reading_another_account_is_routed_as_a_new_key() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    alpaca_reads(&server, "paper", "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await;
    alpaca_reads(&server, "live", "PKTEST", 200, common::alpaca_key::ACCOUNT_B).await;
    let before = key_row(c, s.api_key_id);
    let mut b = browser(&app, c, s.user_id);
    let fields = [("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "live")];
    let asked = alpaca_json(&app, &mut b, s.exchange_id, &fields).await;
    assert_eq!(asked.status, 409, "{}", asked.body);
    let add: serde_json::Value = serde_json::from_str(&asked.body).unwrap();
    let add = add["answers"].as_array().unwrap_or_else(|| panic!("asked, not refused: {}", asked.body)).iter().find_map(|a| a["value"].as_str().filter(|v| v.starts_with("n!"))).unwrap().to_string();
    let mut answered = fields.to_vec();
    answered.push(("api_key[key_answer]", add.as_str()));
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &answered).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(keys(c, &cipher, s.exchange_id), [(1, "PKTEST".into()), (2, "PKTEST".into())], "the live account is account 2");
    assert_eq!(key_row(c, s.api_key_id), before, "the paper key as it was");
}

/// r6: `route_key`'s installed material includes the mode. A tile posting the paper slot's key and secret in live mode,
/// while account 1's account is not yet established by a read, never routes to the paper slot by material: the live
/// account is account 2 and the paper credential is kept.
#[tokio::test(flavor = "current_thread")]
async fn s9_an_alpaca_tile_in_live_mode_never_takes_the_paper_slot_by_material() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    alpaca_reads(&server, "live", "PKTEST", 200, common::alpaca_key::ACCOUNT_B).await;
    let (credentials, version, _) = deltabadger::sync::validation_material(c, &cipher, s.api_key_id).unwrap();
    identity::record(c, &cipher, s.api_key_id, &credentials, &version, &identity::Observed::Read(identity::AccountId::from_uuid(&serde_json::json!(common::alpaca_key::ACCOUNT_A)).unwrap()), chrono::Utc::now()).unwrap();
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE exchange_id = ?1", [s.exchange_id]).unwrap();
    let before = key_row(c, s.api_key_id);
    let mut b = browser(&app, c, s.user_id);
    let exchange = s.exchange_id.to_string();
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let answer = b.send(&app, "POST", "/tracker/add_api_key", Some(&[("exchange_id", exchange.as_str()), ("key_type", "trading"), ("untargeted", "1"), ("api_key[key]", "PKTEST"),
                                                                    ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "live")]), common::web::Csrf::Header, &[]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, s.exchange_id), [(1, "PKTEST".into()), (2, "PKTEST".into())], "two accounts");
    assert_eq!(key_row(c, s.api_key_id), before, "the paper credential is kept");
}

/// r6: a Reconnect reposting account 1's installed key as a reading key, whose account read names A and whose positions
/// read then fails (a 503), is refused (nothing saved) and still records the read against the installed version.
#[tokio::test(flavor = "current_thread")]
async fn s9_a_refused_alpaca_repost_records_its_account_on_the_installed_key() {
    let (_d, o, s, server, app) = alpaca().await;
    let c = &o.primary;
    assert!(identity::unread(c, s.api_key_id).unwrap());
    let before = key_row(c, s.api_key_id);
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, s.user_id);
    let first = accounts(c, s.user_id, s.exchange_id)[0];
    let answer = save(&app, &mut b, s.exchange_id, Some(first), &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "paper")]).await;
    assert!(answer.status >= 400, "{}: {}", answer.status, answer.body);
    assert!(server.received_requests().await.unwrap().iter().any(|r| r.url.path() == "/v2/positions"), "the positions read failed");
    assert!(!identity::unread(c, s.api_key_id).unwrap(), "the account read is recorded on the installed key");
    assert_eq!(identity::credential_standing(c, s.api_key_id).unwrap(), identity::CredentialStanding::Bound);
    assert_eq!(key_row(c, s.api_key_id), before, "status and material as they were");
}

/// G4 at `revalidate`: Alpaca's account read is its validation read. A key turned correct while its recheck was in flight
/// that answers 503 stays correct and keeps its account's standing; one the venue refuses (401) is incorrect and the
/// standing ends.
#[tokio::test(flavor = "current_thread")]
async fn s9_only_alpaca_s_401_demotes_a_correct_key_at_revalidate() {
    for (answer, status, standing) in [(401, 2, 0), (503, 1, 1)] {
        let (_d, o, s, server, app) = alpaca().await;
        let c = &o.primary;
        let first = accounts(c, s.user_id, s.exchange_id)[0];
        let second = identity::new_account(c, s.user_id, s.exchange_id, chrono::Utc::now()).unwrap();
        stand(c, s.user_id, s.exchange_id, second, first);
        c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [s.api_key_id]).unwrap();
        let (db, key) = (c.path().unwrap().to_string(), s.api_key_id);
        Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID", "PKTEST")).respond_with(move |_: &wiremock::Request| {
            Connection::open(&db).unwrap().execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [key]).unwrap();
            ResponseTemplate::new(answer).set_body_json(serde_json::json!({"code": 40110000, "message": "request is not authorized"}))
        }).with_priority(1).mount(&server).await;
        let mut b = browser(&app, c, s.user_id);
        // D-ALPACA-ONE-KEY: the trading card rechecks the key (the reading card shows it and leaves it to that one).
        b.get(&app, &format!("/tracker/add_api_key/new?exchange_id={}&key_type=trading&account_id={first}", s.exchange_id)).await;
        assert_eq!(account_reads(&server).await, 1, "{answer}: the recheck ran");
        assert_eq!((one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {key}")), standings(c)), (status, standing), "{answer}");
    }
}

/// G4 (as review r1 #4 on Coinbase): the JSON save's same-key repost of a correct Alpaca key whose account read answers 503
/// neither demotes it nor ends its standing.
#[tokio::test(flavor = "current_thread")]
async fn s9_an_alpaca_json_same_key_repost_with_a_transient_read_keeps_a_correct_key() {
    let (_d, o, s, server, app) = alpaca().await;
    let c = &o.primary;
    let first = accounts(c, s.user_id, s.exchange_id)[0];
    let second = identity::new_account(c, s.user_id, s.exchange_id, chrono::Utc::now()).unwrap();
    stand(c, s.user_id, s.exchange_id, second, first);
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID", "PKTEST")).respond_with(ResponseTemplate::new(503)).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, s.user_id);
    let first_s = first.to_string();
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[account_id]", first_s.as_str()), ("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret")]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(account_reads(&server).await, 1);
    assert_eq!((one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {}", s.api_key_id)), standings(c)), (1, 1));
}

// ---- #710 slice 9, Alpaca branch review r1 ----

/// r1 #1: a mode switch is written only when the new host's read succeeds. A live host answering 503 refuses the switch
/// ("try again"); the paper slot is as it was, byte for byte.
#[tokio::test(flavor = "current_thread")]
async fn s9_al_r1_a_transient_alpaca_switch_keeps_the_paper_slot() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let c = &o.primary;
    Mock::given(method("GET")).and(path("/live/v2/account")).and(header("APCA-API-KEY-ID", "PKTEST")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    alpaca_reads(&server, "paper", "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await;
    let before = key_row(c, s.api_key_id);
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "live")]).await;
    assert_eq!(answer.status, 409, "{}", answer.body);
    assert!(answer.body.contains("We could not confirm which account this key belongs to. Try again."), "{}", answer.body);
    assert_eq!(mode_reads(&server).await, [pair("/live/v2/account", "PKTEST")]);
    assert_eq!(key_row(c, s.api_key_id), before);
}

/// r1 #2: a JSON repost naming no account on an exchange with two is refused before any request: no key is sent anywhere.
#[tokio::test(flavor = "current_thread")]
async fn s9_al_r1_an_ambiguous_alpaca_repost_sends_nothing() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    c.execute("UPDATE api_keys SET passphrase = ?1 WHERE id = ?2", rusqlite::params![cipher.encrypt("live"), s.api_key_id]).unwrap();
    identity::new_account(c, s.user_id, s.exchange_id, chrono::Utc::now()).unwrap();
    let before = key_row(c, s.api_key_id);
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret")]).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent: {:?}", common::alpaca_key::sent(&server).await);
    assert_eq!(key_row(c, s.api_key_id), before);
}

/// r1 #5: a mode that selects the stored key's host is no switch, and the read is recorded on the stored material: a key
/// stored with no mode reposted as `paper` keeps its stored bytes and is not due again.
#[tokio::test(flavor = "current_thread")]
async fn s9_al_r1_an_equivalent_paper_mode_records_the_stored_material() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    c.execute("UPDATE api_keys SET passphrase = NULL WHERE id = ?1", [s.api_key_id]).unwrap();
    alpaca_reads(&server, "paper", "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await;
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "paper")]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(one::<Option<String>>(c, &format!("SELECT passphrase FROM api_keys WHERE id = {}", s.api_key_id)), None, "the stored mode as it was");
    let due = identity::due(c, &cipher, "2026-09-20T13:00:00Z".parse().unwrap()).unwrap();
    assert!(due.iter().all(|d| d.api_key_id != s.api_key_id), "the read is the stored material's");
}

/// r1 #6: the mode compares as the host it selects: the recorded spelling `Live` selects the paper host, so reposting the
/// key as `paper` is no switch; the stored bytes stay.
#[tokio::test(flavor = "current_thread")]
async fn s9_al_r1_a_spelling_that_selects_the_paper_host_is_paper() {
    let (_d, o, s, server, app) = alpaca_modes().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    c.execute("UPDATE api_keys SET passphrase = ?1 WHERE id = ?2", rusqlite::params![cipher.encrypt("Live"), s.api_key_id]).unwrap();
    alpaca_reads(&server, "paper", "PKTEST", 200, common::alpaca_key::ACCOUNT_A).await;
    let stored = one::<String>(c, &format!("SELECT passphrase FROM api_keys WHERE id = {}", s.api_key_id));
    let mut b = browser(&app, c, s.user_id);
    let answer = alpaca_json(&app, &mut b, s.exchange_id, &[("api_key[key]", "PKTEST"), ("api_key[secret]", "paper-secret"), ("api_key[passphrase]", "paper")]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(mode_reads(&server).await, [pair("/paper/v2/account", "PKTEST")]);
    assert_eq!(one::<String>(c, &format!("SELECT passphrase FROM api_keys WHERE id = {}", s.api_key_id)), stored, "no switch: the stored bytes");
}

// ---- #710 slice 9: Bybit names its UID (`result.userID` of GET /v5/user/query-api, after the permission check) ----

/// The UID read Bybit's public docs type: `userID` an integer (`text` sends it as a string), `id` the key's own id.
fn bybit_uid(uid: &str, text: bool) -> serde_json::Value {
    let user = if text { serde_json::json!(uid) } else { serde_json::json!(uid.parse::<u64>().unwrap()) };
    serde_json::json!({"retCode": 0, "retMsg": "", "result": {"id": "13770661", "apiKey": "masked", "userID": user, "parentUid": "0", "isMaster": true}, "retExtInfo": {}, "time": 1697525990798u64})
}
/// A Bybit exchange of the Alpaca fixture's user. `by-a`, `by-a2`, `by-a3` read UID A; `by-b` B (as text); `by-nouid`'s
/// answer names no UID; any other key's UID read fails (a 503). Every key's permission check passes.
async fn bybit() -> (tempfile::TempDir, store::Opened, i64, i64, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    for (key, uid, text) in [("by-a", UID_A, false), ("by-a2", UID_A, false), ("by-a3", UID_A, false), ("by-b", UID_B, true)] {
        Mock::given(method("GET")).and(path(identity::BYBIT_PATH)).and(header("X-BAPI-API-KEY", key)).respond_with(ResponseTemplate::new(200).set_body_json(bybit_uid(uid, text))).mount(&server).await;
    }
    Mock::given(method("GET")).and(path(identity::BYBIT_PATH)).and(header("X-BAPI-API-KEY", "by-nouid"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"retCode": 0, "result": {"id": "13770661"}}))).mount(&server).await;
    Mock::given(method("GET")).and(path(identity::BYBIT_PATH)).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    Mock::given(method("GET")).and(path(deltabadger::venue::bybit::ASSETS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"retCode": 0, "result": {"list": [{"coin": []}]}}))).mount(&server).await;
    Mock::given(method("POST")).and(path(deltabadger::venue::bybit::CANCEL_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"retCode": 170213, "retMsg": "Order does not exist."}))).mount(&server).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"retCode": 0, "result": {"list": []}}))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let s = seed::seed_alpaca(c, &seed::cipher());
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, 'Bybit', '0.1', '0.1', 'now', 'now')", [deltabadger::venue::bybit::BYBIT]).unwrap();
    let exchange = c.last_insert_rowid();
    let app = web_app(dir.path(), c).with_venue_key_url(deltabadger::venue::bybit::BYBIT, &server.uri()).unwrap();
    (dir, o, s.user_id, exchange, server, app)
}
fn bybit_fields(key: &'static str) -> [(&'static str, &'static str); 2] { [("api_key[key]", key), ("api_key[secret]", "by-secret")] }
/// The requests `server` received signed by `key` to `path`.
async fn bybit_sent(server: &MockServer, key: &str, to: &str) -> usize {
    server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == to && r.headers.get("X-BAPI-API-KEY").is_some_and(|v| v == key)).count()
}

/// Two keys of one UID make one account; a key of another UID its own (tiles); a Reconnect of account 2 with a key of UID A
/// lands on account 1; a key whose answer names no UID (only the key's own `id`) is refused on a tile (Q5).
#[tokio::test(flavor = "current_thread")]
async fn s9_bybit_one_uid_is_one_account_and_another_uid_another() {
    let (_d, o, user, exchange, server, app) = bybit().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["by-a", "by-b", "by-a2"] {
        let answer = save(&app, &mut b, exchange, None, &bybit_fields(key)).await;
        assert!(answer.status < 400, "{key}: {}: {}", answer.status, answer.body);
    }
    assert_eq!(keys(c, &cipher, exchange), [(1, "by-a2".into()), (2, "by-b".into())], "A's second key replaced A's on account 1");
    let reconnect = save(&app, &mut b, exchange, Some(accounts(c, user, exchange)[1]), &bybit_fields("by-a3")).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "by-a3".into()), (2, "by-b".into())], "routed to A's account, account 2 untouched");
    let refused = save(&app, &mut b, exchange, None, &bybit_fields("by-nouid")).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("We could not confirm which account this key belongs to"), "{}", refused.body);
    assert_eq!((numbers(c, exchange), keys(c, &cipher, exchange)), (vec![1, 2], vec![(1, "by-a3".into()), (2, "by-b".into())]));
    assert_eq!(bybit_sent(&server, "by-a", identity::BYBIT_PATH).await, 1, "one UID read per check");
}

/// The bot wizard's key save sees Bybit's UID: a trading key of another UID than account 1's makes account 2, and its
/// observation is recorded.
#[tokio::test(flavor = "current_thread")]
async fn s9_the_wizard_save_routes_a_bybit_key_by_its_uid() {
    let (_d, o, user, exchange, server, app) = bybit().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &bybit_fields("by-a")).await.status < 400);
    let e = exchange.to_string();
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/bots/dca_single_assets/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", e.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let form = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(form.status, 200, "{}", location(&form));
    let answer = b.send(&app, "POST", &format!("/bots/dca_single_assets/add_api_key?exchange_id={e}&untargeted=1"), Some(&bybit_fields("by-b")), common::web::Csrf::Header, &[]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "by-a".into()), (2, "by-b".into())]);
    let trading = identity::key_of(c, accounts(c, user, exchange)[1], 0).unwrap().unwrap();
    assert_eq!(identity::credential_standing(c, trading).unwrap(), identity::CredentialStanding::Bound);
    assert_eq!(bybit_sent(&server, "by-b", deltabadger::venue::bybit::CANCEL_PATH).await, 1, "the trading probe, then the UID read");
}

/// A read-only Bybit key `key` placed on `account` as 2.x would leave it: correct, never read.
fn bybit_unread(c: &Connection, user: i64, exchange: i64, account: i64, key: &str) -> i64 {
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt(key), cipher.encrypt("by-secret")]).unwrap();
    let id = c.last_insert_rowid();
    identity::Account::by_id(c, account).unwrap().unwrap().add_key(c, id).unwrap();
    id
}

/// G4 r6 at `revalidate`: Bybit's permission probe comes before its UID read. A key correct while its recheck is in flight
/// whose probe answers 10006 (a rate limit) or a bare 403 keeps its status and its account's standing, with no UID read
/// sent; one whose probe answers a refusal (10003 invalid, 10007 authentication failed) is no longer correct and its
/// standing ends. The probe was received each time.
#[tokio::test(flavor = "current_thread")]
async fn s9_a_transient_bybit_probe_keeps_a_correct_keys_status_and_standing_at_revalidate() {
    let body = |code: i64, message: &str| serde_json::json!({"retCode": code, "retMsg": message, "result": {}});
    for (status, answer, kept) in [(200, body(10006, "Too many visits!"), true), (403, serde_json::json!({}), true),
                                   (200, body(10003, "API key is invalid."), false), (200, body(10007, "User authentication failed."), false)] {
        let (_d, o, user, exchange, server, app) = bybit().await;
        let c = &o.primary;
        let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
        let key = bybit_unread(c, user, exchange, first, "by-a");
        let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
        stand(c, user, exchange, second, first);
        c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [key]).unwrap();
        let db = c.path().unwrap().to_string();
        let reply = answer.clone();
        Mock::given(method("GET")).and(path(deltabadger::venue::bybit::ASSETS_PATH)).and(header("X-BAPI-API-KEY", "by-a")).respond_with(move |_: &wiremock::Request| {
            // Correct while the recheck is in flight.
            Connection::open(&db).unwrap().execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [key]).unwrap();
            ResponseTemplate::new(status).set_body_json(reply.clone())
        }).with_priority(1).mount(&server).await;
        let mut b = browser(&app, c, user);
        b.get(&app, &format!("/tracker/add_api_key/new?exchange_id={exchange}&key_type=read_only&account_id={first}")).await;
        assert_eq!(bybit_sent(&server, "by-a", deltabadger::venue::bybit::ASSETS_PATH).await, 1, "{status} {answer}: the recheck sent its probe");
        assert_eq!(bybit_sent(&server, "by-a", identity::BYBIT_PATH).await, 0, "{status} {answer}: no UID read");
        let now = one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {key}"));
        assert_eq!((now == 1, standings(c)), (kept, kept as i64), "{status} {answer}: status {now}");
    }
}

/// The JSON save reads a Bybit key's UID before it saves (D-JSON-KEY-IDENTITY): a key of another UID than the named
/// account's is asked first (rule M2); a key of the same UID is saved there.
#[tokio::test(flavor = "current_thread")]
async fn s9_the_bybit_json_save_reads_the_uid_first() {
    let (_d, o, user, exchange, server, app) = bybit().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &bybit_fields("by-a")).await.status < 400);
    let (exchange_s, first_s) = (exchange.to_string(), accounts(c, user, exchange)[0].to_string());
    let post = |key: &'static str| [("api_key[exchange_id]", exchange_s.clone()), ("api_key[account_id]", first_s.clone()), ("api_key[key_type]", "read_only".into()),
                                     ("api_key[key]", key.into()), ("api_key[secret]", "by-secret".into())];
    let form = post("by-b");
    let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let asked = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!(asked.status, 409, "{}", asked.body);
    assert_eq!(bybit_sent(&server, "by-b", identity::BYBIT_PATH).await, 1, "the UID was read before the answer");
    let form = post("by-a2");
    let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let answer = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(keys(c, &cipher, exchange), [(1, "by-a2".into())]);
}

// ---- #710 slice 9, Bybit review r1 ----

/// A key `key` of `kind` placed on `account` as 2.x would leave it: correct, never read (`passphrase` where the venue has one).
fn unread_slot_key(c: &Connection, user: i64, exchange: i64, account: i64, kind: i64, (key, secret, passphrase): (&str, &str, Option<&str>)) -> i64 {
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 'now', 'now')",
              rusqlite::params![user, exchange, kind, cipher.encrypt(key), cipher.encrypt(secret), passphrase.map(|p| cipher.encrypt(p))]).unwrap();
    let id = c.last_insert_rowid();
    identity::Account::by_id(c, account).unwrap().unwrap().add_key(c, id).unwrap();
    assert!(identity::unread(c, id).unwrap());
    id
}

/// Review r1 #1 (every identity venue): a tile save landing on an account that holds an unread key of other material reads
/// that key first, even when the slot it fills is empty (the read-only slot beside an unread trading key): a key of
/// another exchange account never binds the account to its own id.
#[tokio::test(flavor = "current_thread")]
async fn s9r1_a_save_beside_an_unread_key_reads_it_first() {
    // Bybit: by-a (trading, unread) reads UID A; by-b (read-only) B.
    let (_d, o, user, exchange, server, app) = bybit().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let installed = unread_slot_key(c, user, exchange, first, 0, ("by-a", "by-secret", None));
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, None, &bybit_fields("by-b")).await;
    assert_eq!(bybit_sent(&server, "by-a", identity::BYBIT_PATH).await, 1, "the unread trading key was read first: {} {}", answer.status, answer.body);
    assert!(!identity::unread(c, installed).unwrap());
    assert_eq!(keys(c, &cipher, exchange), [(1, "by-a".into()), (2, "by-b".into())], "B's key is its own account: {} {}", answer.status, answer.body);
    // KuCoin: key-a (trading, unread) reads A; key-b (read-only) B.
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let installed = unread_slot_key(c, user, exchange, first, 0, ("key-a", "secret", Some("phrase")));
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, None, &kucoin_fields("key-b")).await;
    let reads = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == identity::KUCOIN_PATH && r.headers.get("KC-API-KEY").is_some_and(|v| v == "key-a")).count();
    assert_eq!(reads, 1, "KuCoin: the unread trading key was read first: {} {}", answer.status, answer.body);
    assert!(!identity::unread(c, installed).unwrap());
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into()), (2, "key-b".into())], "KuCoin: {} {}", answer.status, answer.body);
}

/// Review r1 #5: a Reconnect reposting the exact installed Bybit key whose probe answers a definite refusal (10007) is
/// refused, and that refusal is recorded on the installed key as a read's would be: its account's standing ends.
#[tokio::test(flavor = "current_thread")]
async fn s9r1_a_refused_repost_records_a_definite_bybit_refusal_on_the_installed_key() {
    let (_d, o, user, exchange, server, app) = bybit().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let key = unread_slot_key(c, user, exchange, first, 2, ("by-a", "by-secret", None));
    let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
    stand(c, user, exchange, second, first);
    Mock::given(method("GET")).and(path(deltabadger::venue::bybit::ASSETS_PATH)).and(header("X-BAPI-API-KEY", "by-a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"retCode": 10007, "retMsg": "User authentication failed."}))).with_priority(1).mount(&server).await;
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &bybit_fields("by-a")).await;
    assert!(answer.status >= 400, "{}: {}", answer.status, answer.body);
    assert_eq!(standings(c), 0, "the definite refusal ends the standing");
    assert!(identity::unread(c, key).unwrap(), "a refusal names no account");
}

/// Review r1 #7: the JSON save naming an account whose installed Bybit key nothing has read reads that key first (rule
/// Q1), then asks about the new key of another UID.
#[tokio::test(flavor = "current_thread")]
async fn s9r1_the_bybit_json_save_reads_an_unread_installed_key_first() {
    let (_d, o, user, exchange, server, app) = bybit().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let installed = unread_slot_key(c, user, exchange, first, 2, ("by-a", "by-secret", None));
    let mut b = browser(&app, c, user);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let (exchange_s, first_s) = (exchange.to_string(), first.to_string());
    let form = [("api_key[exchange_id]", exchange_s.as_str()), ("api_key[account_id]", first_s.as_str()), ("api_key[key_type]", "read_only"),
                ("api_key[key]", "by-b"), ("api_key[secret]", "by-secret")];
    let asked = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!(bybit_sent(&server, "by-a", identity::BYBIT_PATH).await, 1, "the installed key was read first: {} {}", asked.status, asked.body);
    assert!(!identity::unread(c, installed).unwrap());
    assert_eq!(asked.status, 409, "{}", asked.body);
}

/// Review r1 #2 (every identity venue): a Reconnect of the sole account, whose installed key nothing has read, with a key
/// whose permission check passes but whose own identity read is transient (a 503) is refused with "try again": it is never
/// accepted against the unfingerprinted account, and the installed key stays.
#[tokio::test(flavor = "current_thread")]
async fn s9r1_a_transient_candidate_read_never_replaces_an_unread_key() {
    let (_d, o, user, exchange, server, app) = bybit().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    unread_slot_key(c, user, exchange, first, 2, ("by-a", "by-secret", None));
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &bybit_fields("by-busy")).await;
    assert_eq!(bybit_sent(&server, "by-busy", identity::BYBIT_PATH).await, 1, "the candidate's UID was asked: {}", answer.body);
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(answer.body.contains("We could not confirm which account this key belongs to. Try again."), "{}", answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "by-a".into())]);

    let (_d, o, user, exchange, server, app) = kucoin().await;
    let c = &o.primary;
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    unread_slot_key(c, user, exchange, first, 2, ("key-a", "secret", Some("phrase")));
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &kucoin_fields("key-fail")).await;
    assert_eq!(kucoin_sent(&server, "key-fail").await, 1, "KuCoin: the candidate's UID was asked: {}", answer.body);
    assert_eq!(answer.status, 422, "KuCoin: {}", answer.body);
    assert!(answer.body.contains("We could not confirm which account this key belongs to. Try again."), "KuCoin: {}", answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-a".into())]);
}

/// The UID reads `server` received signed by KuCoin key `key`.
async fn kucoin_sent(server: &MockServer, key: &str) -> usize {
    server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == identity::KUCOIN_PATH && r.headers.get("KC-API-KEY").is_some_and(|v| v == key)).count()
}

/// Review r1 #2 follow-up: a Reconnect reposting the exact stored read-only key of the sole unfingerprinted KuCoin account
/// replaces nothing, so it is never refused for the unread trading key beside it, though the reposted key's UID read is a
/// 503 and its permission check passes.
#[tokio::test(flavor = "current_thread")]
async fn s9r1_an_exact_repost_beside_an_unread_key_is_not_refused() {
    let (_d, o, user, exchange, server, app) = kucoin().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    unread_slot_key(c, user, exchange, first, 0, ("key-trading", "secret", Some("phrase")));
    unread_slot_key(c, user, exchange, first, 2, ("key-reading", "secret", Some("phrase")));
    let mut b = browser(&app, c, user);
    let answer = save(&app, &mut b, exchange, Some(first), &kucoin_fields("key-reading")).await;
    assert_eq!(kucoin_sent(&server, "key-reading").await, 1, "the reposted key's UID was asked: {}", answer.body);
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "key-trading".into()), (1, "key-reading".into())]);
}

// ---- #710 slice 9: MEXC names its UID (`uid` of GET /api/v3/uid, after the permission check) ----

/// A MEXC exchange of the Alpaca fixture's user. `mx-a`, `mx-a2`, `mx-a3` read UID A (text, as MEXC's docs type it); `mx-b`
/// B (a number); `mx-nouid`'s answer names no UID; any other key's UID read fails (a 503). Every key's check passes.
async fn mexc() -> (tempfile::TempDir, store::Opened, i64, i64, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    for (key, uid) in [("mx-a", serde_json::json!(UID_A)), ("mx-a2", serde_json::json!(UID_A)), ("mx-a3", serde_json::json!(UID_A)), ("mx-b", serde_json::json!(UID_B.parse::<u64>().unwrap()))] {
        Mock::given(method("GET")).and(path("/api/v3/uid")).and(header("X-MEXC-APIKEY", key)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"uid": uid}))).mount(&server).await;
    }
    Mock::given(method("GET")).and(path("/api/v3/uid")).and(header("X-MEXC-APIKEY", "mx-nouid")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/v3/uid")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    Mock::given(method("GET")).and(path(deltabadger::venue::mexc::ACCOUNT_PATH)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"canTrade": true, "balances": []}))).mount(&server).await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let s = seed::seed_alpaca(c, &seed::cipher());
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, 'MEXC', '0.1', '0.1', 'now', 'now')", [deltabadger::venue::mexc::MEXC]).unwrap();
    let exchange = c.last_insert_rowid();
    let app = web_app(dir.path(), c).with_venue_key_url(deltabadger::venue::mexc::MEXC, &server.uri()).unwrap();
    (dir, o, s.user_id, exchange, server, app)
}
fn mexc_fields(key: &'static str) -> [(&'static str, &'static str); 2] { [("api_key[key]", key), ("api_key[secret]", "mx-secret")] }
/// The requests `server` received signed by `key` to `to`.
async fn mexc_sent(server: &MockServer, key: &str, to: &str) -> usize {
    server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == to && r.headers.get("X-MEXC-APIKEY").is_some_and(|v| v == key)).count()
}
/// A read-only MEXC key `key` placed on `account` as 2.x would leave it: correct, never read.
fn mexc_unread(c: &Connection, user: i64, exchange: i64, account: i64, key: &str) -> i64 {
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, 1, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt(key), cipher.encrypt("mx-secret")]).unwrap();
    let id = c.last_insert_rowid();
    identity::Account::by_id(c, account).unwrap().unwrap().add_key(c, id).unwrap();
    id
}

/// Two keys of one UID make one account; a key of another UID its own (tiles); a Reconnect of account 2 with a key of UID A
/// lands on account 1; a key whose answer names no UID is refused on a tile (Q5).
#[tokio::test(flavor = "current_thread")]
async fn s9_mexc_one_uid_is_one_account_and_another_uid_another() {
    let (_d, o, user, exchange, server, app) = mexc().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    for key in ["mx-a", "mx-b", "mx-a2"] {
        let answer = save(&app, &mut b, exchange, None, &mexc_fields(key)).await;
        assert!(answer.status < 400, "{key}: {}: {}", answer.status, answer.body);
    }
    assert_eq!(keys(c, &cipher, exchange), [(1, "mx-a2".into()), (2, "mx-b".into())], "A's second key replaced A's on account 1");
    let reconnect = save(&app, &mut b, exchange, Some(accounts(c, user, exchange)[1]), &mexc_fields("mx-a3")).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "mx-a3".into()), (2, "mx-b".into())], "routed to A's account, account 2 untouched");
    let refused = save(&app, &mut b, exchange, None, &mexc_fields("mx-nouid")).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("We could not confirm which account this key belongs to"), "{}", refused.body);
    assert_eq!((numbers(c, exchange), keys(c, &cipher, exchange)), (vec![1, 2], vec![(1, "mx-a3".into()), (2, "mx-b".into())]));
    assert_eq!(mexc_sent(&server, "mx-a", "/api/v3/uid").await, 1, "one UID read per check");
}

/// The bot wizard's key save sees MEXC's UID: a trading key of another UID than account 1's makes account 2.
#[tokio::test(flavor = "current_thread")]
async fn s9_the_wizard_save_routes_a_mexc_key_by_its_uid() {
    let (_d, o, user, exchange, _server, app) = mexc().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut b = browser(&app, c, user);
    assert!(save(&app, &mut b, exchange, None, &mexc_fields("mx-a")).await.status < 400);
    let e = exchange.to_string();
    b.get(&app, "/bots/new").await;
    b.get(&app, "/bots/dca_single_assets/pick_exchange/new").await;
    assert_eq!(b.send(&app, "POST", "/bots/dca_single_assets/pick_exchange", Some(&[("bots_dca_single_asset[exchange_id]", e.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    let form = b.get(&app, "/bots/dca_single_assets/add_api_key/new").await;
    assert_eq!(form.status, 200, "{}", location(&form));
    let answer = b.send(&app, "POST", &format!("/bots/dca_single_assets/add_api_key?exchange_id={e}&untargeted=1"), Some(&mexc_fields("mx-b")), common::web::Csrf::Header, &[]).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    assert_eq!(keys(c, &cipher, exchange), [(1, "mx-a".into()), (2, "mx-b".into())]);
    let trading = identity::key_of(c, accounts(c, user, exchange)[1], 0).unwrap().unwrap();
    assert_eq!(identity::credential_standing(c, trading).unwrap(), identity::CredentialStanding::Bound);
}

/// G4 r6 at `revalidate`: MEXC's account probe comes before its UID read. A key correct while its recheck is in flight whose
/// probe answers 503 (or a bare 403, or a 429) keeps its status and its account's standing, with no UID read sent; one
/// whose probe answers a refusal (10072, 700002) is no longer correct and its standing ends. The probe was received each time.
#[tokio::test(flavor = "current_thread")]
async fn s9_a_transient_mexc_probe_keeps_a_correct_keys_status_and_standing_at_revalidate() {
    let refusal = |code: i64, msg: &str| serde_json::json!({"code": code, "msg": msg});
    for (status, answer, kept) in [(503, serde_json::json!({}), true), (403, serde_json::json!({}), true), (429, refusal(10072, "Api key info invalid"), true),
                                   (400, refusal(10072, "Api key info invalid"), false), (400, refusal(700002, "Signature for this request is not valid."), false),
                                   // Review r1 #1, the recorded signature failure: 602 is no refusal, whatever its words.
                                   (400, refusal(602, "Signature verification failed"), true),
                                   // Review r1 #2: a code spelled as a float is that code.
                                   (403, serde_json::json!({"code": 700007.0, "msg": "No permission to access the endpoint."}), false)] {
        let (_d, o, user, exchange, server, app) = mexc().await;
        let c = &o.primary;
        let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
        let key = mexc_unread(c, user, exchange, first, "mx-a");
        let second = identity::new_account(c, user, exchange, chrono::Utc::now()).unwrap();
        stand(c, user, exchange, second, first);
        c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [key]).unwrap();
        let db = c.path().unwrap().to_string();
        let reply = answer.clone();
        Mock::given(method("GET")).and(path(deltabadger::venue::mexc::ACCOUNT_PATH)).and(header("X-MEXC-APIKEY", "mx-a")).respond_with(move |_: &wiremock::Request| {
            // Correct while the recheck is in flight.
            Connection::open(&db).unwrap().execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [key]).unwrap();
            ResponseTemplate::new(status).set_body_json(reply.clone())
        }).with_priority(1).mount(&server).await;
        let mut b = browser(&app, c, user);
        b.get(&app, &format!("/tracker/add_api_key/new?exchange_id={exchange}&key_type=read_only&account_id={first}")).await;
        assert_eq!(mexc_sent(&server, "mx-a", deltabadger::venue::mexc::ACCOUNT_PATH).await, 1, "{status} {answer}: the recheck sent its probe");
        assert_eq!(mexc_sent(&server, "mx-a", "/api/v3/uid").await, 0, "{status} {answer}: no UID read");
        let now = one::<i64>(c, &format!("SELECT status FROM api_keys WHERE id = {key}"));
        assert_eq!((now == 1, standings(c)), (kept, kept as i64), "{status} {answer}: status {now}");
    }
}

/// The JSON save naming an account whose installed MEXC key nothing has read reads that key first (rule Q1), then asks
/// about the new key of another UID; a key of the same UID is saved there.
#[tokio::test(flavor = "current_thread")]
async fn s9_the_mexc_json_save_reads_an_unread_installed_key_first() {
    let (_d, o, user, exchange, server, app) = mexc().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let first = identity::ensure_account(c, user, exchange, chrono::Utc::now()).unwrap();
    let installed = mexc_unread(c, user, exchange, first, "mx-a");
    let mut b = browser(&app, c, user);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let (exchange_s, first_s) = (exchange.to_string(), first.to_string());
    let post = |key: &'static str| [("api_key[exchange_id]", exchange_s.clone()), ("api_key[account_id]", first_s.clone()), ("api_key[key_type]", "read_only".into()),
                                     ("api_key[key]", key.into()), ("api_key[secret]", "mx-secret".into())];
    let form = post("mx-b");
    let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let asked = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!(mexc_sent(&server, "mx-a", "/api/v3/uid").await, 1, "the installed key was read first: {} {}", asked.status, asked.body);
    assert!(!identity::unread(c, installed).unwrap());
    assert_eq!(asked.status, 409, "{}", asked.body);
    assert_eq!(mexc_sent(&server, "mx-b", "/api/v3/uid").await, 1, "the new key's UID was read before the answer");
    let form = post("mx-a2");
    let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let answer = b.send(&app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await;
    assert_eq!((answer.status, answer.body.as_str()), (201, "{\"data\":true}"));
    assert_eq!(keys(c, &cipher, exchange), [(1, "mx-a2".into())]);
}

// ---- PLAN-710-confirm: a confirmation writes only what its page showed, once ----

/// The upload a question parked (its token), and its record's state (None: no record).
fn token_of(asked: &common::web::Answer) -> String { common::import::token(&asked.body).unwrap_or_else(|| panic!("parked: {}", asked.body)) }
fn state_of(c: &Connection, token: &str) -> Option<String> {
    c.query_row("SELECT state FROM import_parked_uploads WHERE token = ?1", [token], |r| r.get(0)).ok()
}
fn ledger(c: &Connection) -> i64 { one(c, "SELECT count(*) FROM account_transactions") }
/// A Deltabadger CSV that asks "Import the rest": Kraken 1 here, a row of Kraken 2 left out.
fn rest_csv() -> String { ledger_csv(&[("kraken", "T1", "BTC", 5, 1), ("kraken", "T2", "BTC", 6, 2)]) }
/// Posts `fields` (raw: the test names its own token and handle) to the import.
async fn post(app: &deltabadger::web::App, b: &mut common::web::Browser, path: &str, fields: &[(&str, &str)]) -> common::web::Answer {
    let mut answer = b.send(app, "POST", path, Some(fields), common::web::Csrf::Header, &[]).await;
    common::import::decode_answer(app, &mut answer).await;
    answer
}

/// K2 (D-IMPORT-FILE-IMMUTABLE): a parked Deltabadger CSV edited after its question (same length, routing and counts)
/// is no file: nothing written, no new question, the upload closed.
#[tokio::test(flavor = "current_thread")]
async fn k2_a_parked_csv_edited_after_its_question_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let token = token_of(&asked);
    let path = common::import::parked(c, &token);
    // Its first row's own id (only), so the file still reads, routes and counts alike.
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace(",T1,", ",T9,")).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert_eq!(again.status, 422, "{}", again.body);
    assert!(again.body.contains("Choose a file first."), "no file: {}", again.body);
    assert_eq!(ledger(c), before, "nothing written");
    assert_eq!(state_of(c, &token), None, "closed");
}

/// K2: bytes appended to a parked file are no file.
#[tokio::test(flavor = "current_thread")]
async fn k2_bytes_appended_to_a_parked_file_are_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let path = common::import::parked(c, &token_of(&asked));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\n");
    std::fs::write(&path, bytes).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert_eq!((again.status, ledger(c)), (422, before), "{}", again.body);
}

/// M8: the digest is over the raw bytes: two same-length files that read alike (lossy UTF-8) are two files.
#[tokio::test(flavor = "current_thread")]
async fn m8_a_parked_file_that_reads_alike_but_differs_in_bytes_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv().replace(",T1,", ",T\u{fffd},")).await;
    let path = common::import::parked(c, &token_of(&asked));
    let bytes = std::fs::read(&path).unwrap();
    let at = bytes.windows(3).position(|w| w == "\u{fffd}".as_bytes()).unwrap();
    let mut other = bytes.clone();
    // A truncated 4-byte sequence: three bytes, one U+FFFD, as the original's own.
    other.splice(at..at + 3, [0xf0, 0x9f, 0x98]);
    assert_eq!(String::from_utf8_lossy(&other), String::from_utf8_lossy(&bytes), "reads alike");
    assert_ne!(other, bytes);
    std::fs::write(&path, &other).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert_eq!((again.status, ledger(c)), (422, before), "{}", again.body);
}

/// D-IMPORT-ONE-CONFIRM: a second confirmation of one question is no file; the file is written once.
#[tokio::test(flavor = "current_thread")]
async fn a_second_confirmation_is_no_file_and_the_file_is_written_once() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let token = token_of(&asked);
    let handle = common::import::handle_of(&app, token.clone(), hidden(&asked.body)).await.unwrap();
    let before = ledger(c);
    let fields = [("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handle.as_str())];
    assert!(imported(&post(&app, &mut b, "/tracker/import", &fields).await));
    let twice = post(&app, &mut b, "/tracker/import", &fields).await;
    assert_eq!(twice.status, 422, "{}", twice.body);
    assert!(twice.body.contains("Choose a file first."), "no file, not the file's nothing-new answer: {}", twice.body);
    assert_eq!(ledger(c), before + 1, "written once");
}

/// L7: Cancel with the question's own form (its hidden handle posted beside it) cancels; the handle then writes nothing.
#[tokio::test(flavor = "current_thread")]
async fn l7_cancel_from_the_form_wins_over_its_hidden_handle() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let (token, handle) = (token_of(&asked), common::import::handle_of(&app, token_of(&asked), hidden(&asked.body)).await.unwrap());
    let before = ledger(c);
    let cancelled = post(&app, &mut b, "/tracker/import", &[("token", &token), ("format", "deltabadger"), ("api_key_id", &handle), ("cancel", "Cancel")]).await;
    assert!(cancelled.body.contains("Nothing imported."), "{}", cancelled.body);
    let after = post(&app, &mut b, "/tracker/import", &[("token", &token), ("format", "deltabadger"), ("api_key_id", &handle)]).await;
    assert_eq!((after.status, ledger(c)), (422, before), "{}", after.body);
}

/// P1: a Cancel in the query alone is a Cancel.
#[tokio::test(flavor = "current_thread")]
async fn p1_a_query_only_cancel_cancels() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let token = token_of(&asked);
    let cancelled = post(&app, &mut b, &format!("/tracker/import?cancel=1&token={token}"), &[("format", "deltabadger")]).await;
    assert!(cancelled.body.contains("Nothing imported."), "{}", cancelled.body);
    assert_eq!(state_of(c, &token), None);
}

/// N2/P1: a request carrying a token or a handle is never a fresh upload: a confirmation form with its token removed and
/// another clean file attached, a query token beside a clean file, and a file-valued `token` part are each no file.
#[tokio::test(flavor = "current_thread")]
async fn n2_a_request_with_a_token_or_a_handle_never_uploads_a_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let token = token_of(&asked);
    let handle = common::import::handle_of(&app, token.clone(), hidden(&asked.body)).await.unwrap();
    let clean = ledger_csv(&[("kraken", "C1", "ETH", 9, 1)]);
    let before = ledger(c);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let send = |path: String, fields: Vec<(&'static str, String)>, files: Vec<(&'static str, &'static str, Vec<u8>)>| {
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let files: Vec<(&str, &str, &[u8])> = files.iter().map(|(n, f, b)| (*n, *f, b.as_slice())).collect();
        let (content_type, body) = common::web::multipart(&fields, &files);
        (path, content_type, body)
    };
    for (path, content_type, body) in [
        send("/tracker/import".into(), vec![("format", "deltabadger".into()), ("api_key_id", handle.clone())], vec![("file", "ledger.csv", clean.clone().into_bytes())]),
        send(format!("/tracker/import?token={token}"), vec![("format", "deltabadger".into())], vec![("file", "ledger.csv", clean.clone().into_bytes())]),
        send("/tracker/import".into(), vec![("format", "deltabadger".into())], vec![("token", "t.csv", token.clone().into_bytes()), ("file", "ledger.csv", clean.clone().into_bytes())]),
        // Branch review r1 #1: a file-valued `api_key_id` carrying the handle, no token anywhere.
        send("/tracker/import".into(), vec![("format", "deltabadger".into())], vec![("api_key_id", "h.txt", handle.clone().into_bytes()), ("file", "ledger.csv", clean.clone().into_bytes())]),
    ] {
        let answer = b.send_bytes(&app, "POST", &path, Some(body), common::web::Csrf::Header, &[("content-type", content_type.as_str())]).await;
        assert_eq!(answer.status, 422, "{path}: {}", answer.body);
        assert_eq!(ledger(c), before, "{path}: nothing written");
    }
    assert_eq!(state_of(c, &token).as_deref(), Some("open"), "the question still open");
}

/// Q4: one token and one choice per request: upload A's pair in the query and upload B's form in the body write neither,
/// and both questions stay answerable.
#[tokio::test(flavor = "current_thread")]
async fn q4_two_uploads_pairs_in_one_request_write_neither() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let first = upload(&app, &mut b, &rest_csv()).await;
    let second = upload(&app, &mut b, &ledger_csv(&[("kraken", "U1", "ETH", 7, 1), ("kraken", "U2", "ETH", 8, 2)])).await;
    let (ta, tb) = (token_of(&first), token_of(&second));
    let (ha, hb) = (common::import::handle_of(&app, ta.clone(), hidden(&first.body)).await.unwrap(), common::import::handle_of(&app, tb.clone(), hidden(&second.body)).await.unwrap());
    let before = ledger(c);
    let both = post(&app, &mut b, &format!("/tracker/import?token={ta}&api_key_id={ha}"), &[("token", &tb), ("format", "deltabadger"), ("api_key_id", &hb)]).await;
    assert_eq!((both.status, ledger(c)), (422, before), "{}", both.body);
    assert!(imported(&answer(&app, &mut b, &first, &hidden(&first.body)).await));
    assert!(imported(&answer(&app, &mut b, &second, &hidden(&second.body)).await));
    assert_eq!(ledger(c), before + 2);
}

/// K3: another user presenting the owner's token and handle gets no file, for a confirmation and a Cancel; the owner's
/// question still writes.
#[tokio::test(flavor = "current_thread")]
async fn k3_another_users_token_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let token = token_of(&asked);
    let handle = common::import::handle_of(&app, token.clone(), hidden(&asked.body)).await.unwrap();
    c.execute("INSERT INTO users (email, encrypted_password, admin, setup_completed, confirmed_at, created_at, updated_at) SELECT 'other@example.com', encrypted_password, 0, 1, '2026-01-01', '2026-01-01', '2026-01-01' FROM users WHERE id = ?1", [s.user_id]).unwrap();
    let other = c.last_insert_rowid();
    let mut them = browser(&app, c, other);
    assert_eq!(them.get(&app, "/settings/api_keys").await.status, 200);
    let before = ledger(c);
    for fields in [vec![("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handle.as_str())], vec![("token", token.as_str()), ("format", "deltabadger"), ("cancel", "Cancel")]] {
        let answer = post(&app, &mut them, "/tracker/import", &fields).await;
        assert_eq!(answer.status, 422, "{}", answer.body);
    }
    assert_eq!((ledger(c), state_of(c, &token).as_deref()), (before, Some("open")));
    assert!(imported(&answer(&app, &mut b, &asked, &hidden(&asked.body)).await));
}

/// K3: an edited handle, and another upload's handle under this token, resolve to nothing: asked again, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn k3_an_edited_or_crossed_handle_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let first = upload(&app, &mut b, &rest_csv()).await;
    let second = upload(&app, &mut b, &ledger_csv(&[("kraken", "U1", "ETH", 7, 1), ("kraken", "U2", "ETH", 8, 2)])).await;
    let token = token_of(&first);
    let handle = common::import::handle_of(&app, token.clone(), hidden(&first.body)).await.unwrap();
    let crossed = common::import::handle_of(&app, token_of(&second), hidden(&second.body)).await.unwrap();
    let edited = format!("{}{}", &handle[..handle.len() - 1], if handle.ends_with('0') { '1' } else { '0' });
    let before = ledger(c);
    let mut asked = first.clone();
    for posted in [edited, crossed] {
        let again = post(&app, &mut b, "/tracker/import", &[("token", &token), ("format", "deltabadger"), ("api_key_id", &posted)]).await;
        assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
        assert_eq!(ledger(c), before, "nothing written");
        asked = again;
    }
    assert!(imported(&answer(&app, &mut b, &asked, &hidden(&asked.body)).await), "the new question's own handle writes");
}

/// K3/P5: a re-asked question replaces its generation: the earlier generation's handles never resolve again.
#[tokio::test(flavor = "current_thread")]
async fn p5_only_the_newest_generations_handles_resolve() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let (kraken, _) = krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let old = common::import::handle_of(&app, token_of(&asked), hidden(&asked.body)).await.unwrap();
    identity::new_account(c, s.user_id, kraken, chrono::Utc::now()).unwrap();
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    let before = ledger(c);
    let stale = post(&app, &mut b, "/tracker/import", &[("token", &token_of(&asked)), ("format", "deltabadger"), ("api_key_id", &old)]).await;
    assert!(stale.body.contains("Something changed since you answered."), "the old generation asks again: {}", stale.body);
    assert_eq!(ledger(c), before);
    assert!(imported(&answer(&app, &mut b, &stale, &hidden(&stale.body)).await));
    assert_eq!(ledger(c), before + 2, "both Krakens' rows");
}

/// K4: two confirmations at once: one writes, the other is no file.
#[tokio::test(flavor = "current_thread")]
async fn k4_two_confirmations_at_once_write_once() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let (token, handle) = (token_of(&asked), common::import::handle_of(&app, token_of(&asked), hidden(&asked.body)).await.unwrap());
    let before = ledger(c);
    let mut b2 = b.clone();
    let fields = [("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handle.as_str())];
    let (x, y) = tokio::join!(b.send(&app, "POST", "/tracker/import", Some(&fields), common::web::Csrf::Header, &[]),
                              b2.send(&app, "POST", "/tracker/import", Some(&fields), common::web::Csrf::Header, &[]));
    assert_eq!([imported(&x), imported(&y)].iter().filter(|w| **w).count(), 1, "{} / {}", x.body, y.body);
    assert!(x.status == 422 || y.status == 422);
    assert_eq!(ledger(c), before + 1);
}

/// K4: a Cancel racing a confirmation: once the Cancel commits, the confirmation writes nothing and asks nothing.
#[tokio::test(flavor = "current_thread")]
async fn k4_a_cancel_racing_a_confirmation_leaves_nothing_to_write() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let (token, handle) = (token_of(&asked), common::import::handle_of(&app, token_of(&asked), hidden(&asked.body)).await.unwrap());
    let before = ledger(c);
    let mut b2 = b.clone();
    let (cancel, click) = ([("token", token.as_str()), ("format", "deltabadger"), ("cancel", "Cancel")], [("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handle.as_str())]);
    let (x, y) = tokio::join!(b.send(&app, "POST", "/tracker/import", Some(&cancel), common::web::Csrf::Header, &[]),
                              b2.send(&app, "POST", "/tracker/import", Some(&click), common::web::Csrf::Header, &[]));
    if x.body.contains("Nothing imported.") {
        assert_eq!((y.status, ledger(c)), (422, before), "{}", y.body);
    } else {
        assert!(imported(&y) && x.status == 422, "{} / {}", x.body, y.body);
    }
    assert_eq!(state_of(c, &token), None);
}

/// M5: an upload past its expiry is no file, whatever its handle.
#[tokio::test(flavor = "current_thread")]
async fn m5_an_expired_question_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    c.execute("UPDATE import_parked_uploads SET expires_at = '2026-09-20 11:59:59'", []).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert_eq!((again.status, ledger(c)), (422, before), "{}", again.body);
}

/// M2/K1: a process killed before the joint commit wrote nothing and left the upload open (the click then writes once);
/// Q5: one killed after the commit, before the cleanup, wrote once, and the sweep takes its bytes and handles.
#[tokio::test(flavor = "current_thread")]
async fn m2_q5_a_crash_on_either_side_of_the_commit() {
    use deltabadger::tracker::import::service;
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let token = token_of(&asked);
    let before = ledger(c);
    service::CRASH.lock().unwrap().push((token.clone(), "before_commit"));
    let crashed = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert_eq!((crashed.status, ledger(c), state_of(c, &token).as_deref()), (500, before, Some("open")), "{}", crashed.body);
    service::CRASH.lock().unwrap().retain(|(t, _)| *t != token);
    service::CRASH.lock().unwrap().push((token.clone(), "before_cleanup"));
    let crashed = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    service::CRASH.lock().unwrap().retain(|(t, _)| *t != token);
    assert_eq!((crashed.status, ledger(c), state_of(c, &token).as_deref()), (500, before + 1, Some("written")));
    assert!(common::import::parked(c, &token).exists());
    // Branch review r1 #10: the restart sweeps what the killed process left.
    drop(app);
    let app = web_app(_d.path(), c);
    let mut b = browser(&app, c, s.user_id);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    assert!(!common::import::parked(c, &token).exists(), "the bytes are gone");
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM import_parked_choices WHERE token = '{token}'")), 0, "the handles are gone");
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert_eq!((again.status, ledger(c)), (422, before + 1), "written once");
}

/// K6: a question has no side effect: on an install with no identity key, a native file's question stores none, and makes
/// no account, observation, standing or snapshot.
#[tokio::test(flavor = "current_thread")]
async fn k6_a_question_writes_nothing_not_even_the_identity_key() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    c.execute("DELETE FROM app_configs WHERE key = ?1", [identity::KEY_CONFIG]).unwrap();
    let tables = ["exchange_accounts", "exchange_account_credentials", "exchange_account_imports", "app_configs", "exchange_account_standings", "portfolio_snapshots", "account_transactions"];
    let counts = |c: &Connection| tables.iter().map(|t| one::<i64>(c, &format!("SELECT count(*) FROM {t}"))).collect::<Vec<_>>();
    let before = counts(c);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(asked.body.contains("api_key_id"), "asked: {}", asked.body);
    assert_eq!(counts(c), before, "the question wrote nothing");
    let stale = post(&app, &mut b, "/tracker/import", &[("token", &token_of(&asked)), ("format", "kucoin"), ("api_key_id", &format!("{}.1.9.{}", token_of(&asked), "0".repeat(32)))]).await;
    assert!(stale.body.contains("api_key_id") && !imported(&stale), "asked again: {}", stale.body);
    assert_eq!(counts(c), before, "asking again wrote nothing");
}

/// N4: with the identity key unreadable, an import neither resets it nor touches another user's observations, standings or
/// snapshots, in its question or its write; the account is asked.
#[tokio::test(flavor = "current_thread")]
async fn n4_an_unreadable_identity_key_is_never_reset_by_an_import() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "444");
    c.execute("UPDATE app_configs SET value = 'unreadable' WHERE key = ?1", [identity::KEY_CONFIG]).unwrap();
    let state = |c: &Connection| (one::<i64>(c, "SELECT count(*) FROM exchange_account_credentials"), one::<i64>(c, "SELECT count(*) FROM exchange_accounts WHERE fingerprint IS NOT NULL"),
                                  one::<String>(c, &format!("SELECT value FROM app_configs WHERE key = '{}'", identity::KEY_CONFIG)));
    let before = state(c);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(asked.body.contains("api_key_id"), "the account is asked: {}", asked.body);
    assert_eq!(state(c), before, "the question reset nothing");
    let done = answer(&app, &mut b, &asked, &format!("a{first}")).await;
    assert!(imported(&done), "{}", done.body);
    assert_eq!(state(c), before, "the write reset nothing");
}

/// Q3: on first use (no key row, no identity known), the first key is stored by a plain insert: another user's failed
/// read survives a successful native import.
#[tokio::test(flavor = "current_thread")]
async fn q3_the_first_identity_key_clears_no_observation() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    c.execute("DELETE FROM app_configs WHERE key = ?1", [identity::KEY_CONFIG]).unwrap();
    c.execute("DELETE FROM exchange_account_credentials", []).unwrap();
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL", []).unwrap();
    c.execute("DELETE FROM exchange_account_imports", []).unwrap();
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, last_failure_at, last_failure_code) SELECT id, exchange_id, '2026-01-01', '401' FROM api_keys WHERE user_id = ?1 LIMIT 1", [s.user_id]).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let done = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(imported(&done), "{}", done.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_credentials WHERE last_failure_code = '401'"), 1, "the failed read survives");
    assert!(identity::existing_key(c, &seed::cipher()).is_some(), "the first key is stored");
}

/// The assets and tickers KuCoin's Convert legs pair by (kucoin_import's fixture).
fn kucoin_market(c: &Connection, exchange: i64) {
    let mut assets = vec![];
    for symbol in ["ETH", "BTC", "USDT", "KCS"] {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01') ON CONFLICT DO NOTHING",
                  [format!("seed-{symbol}"), symbol.to_string()]).unwrap();
        assets.push(one::<i64>(c, &format!("SELECT id FROM assets WHERE external_id = 'seed-{symbol}'")));
    }
    for (base, quote, b, q) in [("ETH", "BTC", 0, 1), ("BTC", "USDT", 1, 2), ("KCS", "USDT", 3, 2)] {
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 8, 8, 2, '0', '0', 1, 1, '2026-01-01', '2026-01-01')",
                  rusqlite::params![exchange, format!("{base}-{quote}"), base, quote, assets[b], assets[q]]).unwrap();
    }
}
/// A KuCoin export of `rows` ("Currency,Side,Amount,Fee,local time,Type") for `uid`.
fn kucoin_export(uid: &str, rows: &[&str]) -> String {
    let mut out = vec!["UID,Account Type,Currency,Side,Amount,Fee,Time(UTC+03:00),Remark,Type".to_string()];
    for r in rows { let f: Vec<&str> = r.split(',').collect(); out.push(format!("{uid},mainAccount,{},{},{},{},{},,{}", f[0], f[1], f[2], f[3], f[4], f[5])); }
    out.join("\n")
}
/// A one-account KuCoin with a lone Convert half imported, and a Kraken deposit (on Kraken's account) linked to it (or, with
/// `outgoing`, the half linked to the deposit). The half's id and the deposit's.
async fn convert_half_linked(app: &deltabadger::web::App, b: &mut common::web::Browser, c: &Connection, user: i64, outgoing: bool) -> (i64, i64, i64) {
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, user, kucoin, None);
    kucoin_market(c, kucoin);
    let kraken = venue(c, "Exchanges::Kraken", "Kraken");
    identity::ensure_account(c, user, kraken, chrono::Utc::now()).unwrap();
    let at = "2026-01-01 12:00:00";
    let done = upload(app, b, &kucoin_export("444", &[&format!("BTC,Deposit,0.1,0,{at},Deposit"), &format!("USDT,Withdrawal,100,0,{at},Convert")])).await;
    assert!(imported(&done), "{}", done.body);
    let half: i64 = one(c, "SELECT id FROM account_transactions WHERE entry_type = 15");
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, linked_transaction_id, created_at, updated_at)
               VALUES (?1, ?2, 4, 'USDT', 100, '2026-01-02 12:00:00', ?3, '2026-01-01', '2026-01-01')", rusqlite::params![user, kraken, (!outgoing).then_some(half)]).unwrap();
    let deposit = c.last_insert_rowid();
    if outgoing { c.execute("UPDATE account_transactions SET linked_transaction_id = ?1 WHERE id = ?2", [deposit, half]).unwrap(); }
    (kraken, half, deposit)
}
fn the_other_half() -> String { kucoin_export("444", &["ETH,Deposit,0.05,0,2026-01-01 12:00:00,Convert"]) }

/// Q2/M4: a one-account KuCoin import that would undo a Kraken deposit's link asks first, naming Kraken, the link still in
/// place; its click writes and undoes it.
#[tokio::test(flavor = "current_thread")]
async fn q2_a_write_that_undoes_a_link_on_another_account_asks_first() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let (_, half, deposit) = convert_half_linked(&app, &mut b, c, s.user_id, false).await;
    let asked = upload(&app, &mut b, &the_other_half()).await;
    assert!(asked.body.contains("Transfer links on other accounts that this import undoes: 1 (Kraken)."), "{}", asked.body);
    assert_eq!(one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {deposit}")), Some(half), "the link is still there");
    let done = import_answer(&app, &mut b, &asked).await;
    assert!(imported(&done), "{}", done.body);
    assert_eq!(one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {deposit}")), None);
}
/// The last option a question offers, posted.
async fn import_answer(app: &deltabadger::web::App, b: &mut common::web::Browser, asked: &common::web::Answer) -> common::web::Answer {
    let value = full_options(&asked.body).last().cloned().unwrap_or_else(|| panic!("asked nothing: {}", asked.body));
    answer(app, b, asked, &value).await
}

/// N3/M4: the linked row on the other account edited after the question (its count kept) is a change: asked again, the link
/// and the ledger as they were.
#[tokio::test(flavor = "current_thread")]
async fn n3_an_undone_link_edited_after_the_question_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let (_, half, deposit) = convert_half_linked(&app, &mut b, c, s.user_id, false).await;
    let asked = upload(&app, &mut b, &the_other_half()).await;
    c.execute("UPDATE account_transactions SET base_amount = 99 WHERE id = ?1", [deposit]).unwrap();
    let before = ledger(c);
    let again = import_answer(&app, &mut b, &asked).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!((ledger(c), one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {deposit}"))), (before, Some(half)));
}

/// Q1/P2: a row the rebuild deletes whose partner sits on another account is an undone link too; the partner moved to
/// another account after the question is a change, the link kept.
#[tokio::test(flavor = "current_thread")]
async fn q1_a_deleted_rows_partner_moved_after_the_question_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let (kraken, half, deposit) = convert_half_linked(&app, &mut b, c, s.user_id, true).await;
    let asked = upload(&app, &mut b, &the_other_half()).await;
    assert!(asked.body.contains("Transfer links on other accounts that this import undoes: 1 (Kraken)."), "{}", asked.body);
    let second = identity::new_account(c, s.user_id, kraken, chrono::Utc::now()).unwrap();
    c.execute("INSERT INTO exchange_account_rows (account_transaction_id, account_id) VALUES (?1, ?2)", [deposit, second]).unwrap();
    let again = import_answer(&app, &mut b, &asked).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {half}")), Some(deposit), "the link is kept");
}

/// P3: another user's row linked to the owner's is neither shown nor changed.
#[tokio::test(flavor = "current_thread")]
async fn p3_another_users_linked_row_is_never_shown_or_changed() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let (_, half, deposit) = convert_half_linked(&app, &mut b, c, s.user_id, false).await;
    c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('foreign@example.com', 'x', 0, '2026-01-01', '2026-01-01')", []).unwrap();
    c.execute("UPDATE account_transactions SET user_id = ?1 WHERE id = ?2", [c.last_insert_rowid(), deposit]).unwrap();
    let done = upload(&app, &mut b, &the_other_half()).await;
    assert!(imported(&done) && !done.body.contains("Transfer links"), "{}", done.body);
    assert_eq!(one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {deposit}")), Some(half), "never changed");
}

/// L2/N8: the "Import the rest" question lists where its rows go and the accounts its left-out rows name, in every locale.
#[tokio::test(flavor = "current_thread")]
async fn l2_the_import_the_rest_question_names_its_destinations_in_every_locale() {
    use deltabadger::web::i18n;
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    for locale in deltabadger::web::locale::LOCALES {
        c.execute("UPDATE users SET locale = ?1 WHERE id = ?2", rusqlite::params![locale, s.user_id]).unwrap();
        let asked = upload(&app, &mut b, &rest_csv()).await;
        let to = i18n::t(locale, "tracker.import.destination_html", &[("account", i18n::Arg::Html("Kraken")), ("count", i18n::Arg::Count(1))]);
        let left = i18n::t(locale, "tracker.import.rows_left_out_named_html", &[("exchanges", i18n::Arg::Text("Kraken 2"))]);
        assert!(asked.body.contains(&to) && asked.body.contains(&left), "{locale}: {}", asked.body);
        assert_ne!(left, "tracker.import.rows_left_out_named_html", "{locale}");
    }
}

/// M1: a venue selector option standing for its venue's sole account writes there on its click; that account replaced
/// (another account holding the same key) between the question and the click is a change: asked again, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn m1_an_authorized_venue_whose_sole_account_was_replaced_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let (kb, _) = (key_on(c, s.user_id, binance, None), key_on(c, s.user_id, us, None));
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert!(options(&asked.body).contains(&kb.to_string()), "{}", asked.body);
    let old = identity::Account::first(c, s.user_id, binance).unwrap().id.unwrap();
    c.execute("UPDATE exchange_accounts SET removed_at = '2026-01-01' WHERE id = ?1", [old]).unwrap();
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [kb]).unwrap();
    let replaced = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    identity::Account::by_id(c, replaced).unwrap().unwrap().add_key(c, kb).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &kb.to_string()).await;
    assert!(!imported(&again), "{}", again.body);
    assert_eq!(ledger(c), before, "nothing written");
}

/// M6/L9: a legacy key's prospective account is bound by its key: an unrelated account made between the question and the
/// click changes nothing, and the click writes.
#[tokio::test(flavor = "current_thread")]
async fn m6_an_unrelated_account_made_meanwhile_keeps_a_prospective_answer() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, binance, e("legacy"), e("s")]).unwrap();
    let legacy = c.last_insert_rowid();
    c.execute("DELETE FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2", [s.user_id, binance]).unwrap();
    key_on(c, s.user_id, us, None);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert!(options(&asked.body).contains(&legacy.to_string()), "{}", asked.body);
    let kraken = venue(c, "Exchanges::Kraken", "Kraken");
    identity::ensure_account(c, s.user_id, kraken, chrono::Utc::now()).unwrap();
    let done = answer(&app, &mut b, &asked, &legacy.to_string()).await;
    assert!(imported(&done), "{}", done.body);
}

/// N1: a venue option standing for a venue with several possible accounts only picks the venue: its click asks which
/// account, writing nothing.
#[tokio::test(flavor = "current_thread")]
async fn n1_a_venue_with_several_accounts_is_picked_not_written() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Binance", "222");
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "222");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    assert_eq!(options(&asked.body), [format!("v{binance}"), format!("v{us}")], "{}", asked.body);
    let picked = answer(&app, &mut b, &asked, &format!("v{binance}")).await;
    assert!(picked.body.contains("api_key_id") && !imported(&picked), "{}", picked.body);
    assert_eq!(rows_on(c, second), 0, "nothing written");
}

/// N7: a venue option's handle stands for its venue's exchange id (`Choice::venue`), a `select` one included.
#[tokio::test(flavor = "current_thread")]
async fn n7_a_venue_options_handle_names_its_exchange() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    key_on(c, s.user_id, binance, None);
    let second = identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Binance", "222");
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "222");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("222")).await;
    let token = token_of(&asked);
    let mut s2 = c.prepare("SELECT kind, json_extract(dest_json, '$.value'), json_extract(dest_json, '$.venue') FROM import_parked_choices WHERE token = ?1 ORDER BY rowid").unwrap();
    let rows: Vec<(String, String, i64)> = s2.query_map([&token], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows[0], ("select".to_string(), format!("v{binance}"), binance), "{rows:?}");
    assert_eq!((rows[1].1.clone(), rows[1].2), (format!("v{us}"), us), "{rows:?}");
}

/// K4: a confirmation whose request is dropped once its worker started still runs to its outcome: written once, closed.
#[tokio::test(flavor = "current_thread")]
async fn k4_a_dropped_confirmation_still_completes() {
    use deltabadger::tracker::import::service;
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &rest_csv()).await;
    let (token, handle) = (token_of(&asked), common::import::handle_of(&app, token_of(&asked), hidden(&asked.body)).await.unwrap());
    let before = ledger(c);
    let signed_in: service::SignedIn = std::sync::Arc::new(|_| Ok(true));
    // Polled once (the worker spawned), then dropped before it ran.
    let dropped = tokio::time::timeout(std::time::Duration::ZERO, service::confirm(&app, s.user_id, token.clone(), handle.clone(), signed_in.clone(), service::permit(&app).await)).await;
    assert!(dropped.is_err(), "dropped before its outcome");
    for _ in 0..200 {
        if state_of(c, &token).is_none() { break }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!((ledger(c), state_of(c, &token)), (before + 1, None), "the worker wrote once and closed the upload");
    assert_eq!(service::confirm(&app, s.user_id, token, handle, signed_in, service::permit(&app).await).await.unwrap(), service::Outcome::NoFile);
}

/// P4: an import's derived bookkeeping (the owner's cached figures and portfolio snapshots, rebuilt from the ledger) is no
/// effect the question shows or seals: the question keeps it and lists no link undone; the write still drops it.
#[tokio::test(flavor = "current_thread")]
async fn p4_snapshots_an_import_drops_are_not_shown_and_still_dropped() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let (kb, _) = (key_on(c, s.user_id, binance, None), key_on(c, s.user_id, us, None));
    c.execute("INSERT INTO portfolio_snapshots(user_id,date,created_at,updated_at) VALUES(?1,'2026-09-19','2026-09-19','2026-09-19')", [s.user_id]).unwrap();
    let snapshots = |c: &Connection| one::<i64>(c, &format!("SELECT count(*) FROM portfolio_snapshots WHERE user_id = {}", s.user_id));
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &binance_file("777")).await;
    assert!(asked.body.contains("api_key_id") && !asked.body.contains("Transfer links"), "{}", asked.body);
    assert_eq!(snapshots(c), 1, "the question kept the snapshot");
    let done = answer(&app, &mut b, &asked, &kb.to_string()).await;
    assert!(imported(&done), "{}", done.body);
    assert_eq!(snapshots(c), 0, "the write dropped it");
}

/// A Binance and a Binance.US account both of UID 1, Binance's holding a dust pair's leg without its pair: a dust file's
/// venue question, whose Binance option the write refuses ("partial existing CSV dust pair") and whose Binance.US option
/// writes.
async fn refusing_sibling(app: &deltabadger::web::App, b: &mut common::web::Browser, c: &Connection, user: i64) -> (common::web::Answer, i64, i64, i64) {
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let (kb, ku) = (key_on(c, user, binance, None), key_on(c, user, us, None));
    let (on_binance, on_us) = (identity::Account::first(c, user, binance).unwrap().id.unwrap(), identity::Account::first(c, user, us).unwrap().id.unwrap());
    fingerprint(c, on_binance, "Exchanges::Binance", "1");
    let dust = "User ID,UTC_Time,Account,Operation,Coin,Change,Remark\n1,2026-08-17 10:00:00,Spot,Small Assets Exchange BNB,BTC,-1,BTC\n1,2026-08-17 10:00:00,Spot,Small Assets Exchange BNB,BNB,0.09,BTC\n";
    let mut done = upload(app, b, dust).await;
    if !imported(&done) { done = answer(app, b, &done, &kb.to_string()).await; }
    assert!(imported(&done) && rows_on(c, on_binance) == 2, "{}", done.body);
    c.execute("DELETE FROM account_transactions WHERE entry_type = 2", []).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "1");
    let asked = upload(app, b, dust).await;
    assert_eq!(options(&asked.body), [kb.to_string(), ku.to_string()], "{}", asked.body);
    (asked, kb, ku, on_us)
}

/// N6: a committed refusal is final: the sibling option of the refused question is no file, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn n6_a_sibling_after_a_committed_refusal_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let mut b = browser(&app, c, s.user_id);
    let (asked, kb, ku, on_us) = refusing_sibling(&app, &mut b, c, s.user_id).await;
    let token = token_of(&asked);
    let before = ledger(c);
    let refused = answer(&app, &mut b, &asked, &kb.to_string()).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Nothing imported. This file doesn&#39;t match your Binance history"), "{}", refused.body);
    assert_eq!((ledger(c), state_of(c, &token)), (before, None), "nothing written, the upload closed");
    // Branch review r1 #9: the sibling's own handle, as its page rendered it, replayed.
    assert!(common::import::rendered(&asked, &full_options(&asked.body).into_iter().find(|v| v.starts_with(&format!("{ku}~")) || v.starts_with(&format!("{ku}!")) || *v == ku.to_string()).unwrap()).is_some(), "{}", asked.body);
    let sibling = answer(&app, &mut b, &asked, &ku.to_string()).await;
    assert_eq!((sibling.status, ledger(c), rows_on(c, on_us)), (422, before, 0), "{}", sibling.body);
    assert!(!sibling.body.contains("api_key_id"), "no new question: {}", sibling.body);
}

/// N6: a refusal that never committed (the process killed before its commit) was never observed: the upload stays open,
/// and its sibling to the healthy account writes once.
#[tokio::test(flavor = "current_thread")]
async fn n6_a_sibling_before_the_refusal_commits_writes_once() {
    use deltabadger::tracker::import::service;
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let mut b = browser(&app, c, s.user_id);
    let (asked, kb, ku, on_us) = refusing_sibling(&app, &mut b, c, s.user_id).await;
    let token = token_of(&asked);
    let before = ledger(c);
    service::CRASH.lock().unwrap().push((token.clone(), "before_commit"));
    let crashed = answer(&app, &mut b, &asked, &kb.to_string()).await;
    service::CRASH.lock().unwrap().retain(|(t, _)| *t != token);
    assert_eq!((crashed.status, ledger(c), state_of(c, &token).as_deref()), (500, before, Some("open")), "{}", crashed.body);
    let sibling = answer(&app, &mut b, &asked, &ku.to_string()).await;
    assert!(imported(&sibling), "{}", sibling.body);
    assert_eq!((rows_on(c, on_us), ledger(c)), (2, before + 2), "written once, on Binance.US");
    let again = answer(&app, &mut b, &asked, &ku.to_string()).await;
    assert_eq!((again.status, ledger(c)), (422, before + 2));
}

/// Branch review r1 #2: an "import anyway" is bound to the key its rows are written with: KuCoin's reading key switching to
/// the account's read-only key after the question (the trading key lost a permission) is a change, asked again.
#[tokio::test(flavor = "current_thread")]
async fn an_import_anyway_whose_reading_key_switched_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let (_, half, deposit) = convert_half_linked(&app, &mut b, c, s.user_id, false).await;
    let kucoin: i64 = one(c, "SELECT id FROM exchanges WHERE type = 'Exchanges::Kucoin'");
    let trading: i64 = one(c, &format!("SELECT id FROM api_keys WHERE exchange_id = {kucoin}"));
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, 1, '2026-01-02', '2026-01-02')",
              rusqlite::params![s.user_id, kucoin, e("read-only"), e("s")]).unwrap();
    let read_only = c.last_insert_rowid();
    identity::Account::first(c, s.user_id, kucoin).unwrap().add_key(c, read_only).unwrap();
    let asked = upload(&app, &mut b, &the_other_half()).await;
    assert!(asked.body.contains("Transfer links on other accounts"), "{}", asked.body);
    c.execute("UPDATE api_keys SET last_sync_error = 'Access Denied' WHERE id = ?1", [trading]).unwrap();
    let before = ledger(c);
    let again = import_answer(&app, &mut b, &asked).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!((ledger(c), one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {deposit}"))), (before, Some(half)));
}

/// Branch review r1 #3: the undone link's row is sealed whole: an asset resolved onto the linked deposit after the question
/// is a change, asked again, the link kept.
#[tokio::test(flavor = "current_thread")]
async fn an_undone_links_row_given_an_asset_after_the_question_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let mut b = browser(&app, c, s.user_id);
    let (_, half, deposit) = convert_half_linked(&app, &mut b, c, s.user_id, false).await;
    let asked = upload(&app, &mut b, &the_other_half()).await;
    assert!(asked.body.contains("Transfer links on other accounts"), "{}", asked.body);
    c.execute("UPDATE account_transactions SET base_asset_id = (SELECT id FROM assets WHERE symbol = 'USDT' LIMIT 1) WHERE id = ?1", [deposit]).unwrap();
    let again = import_answer(&app, &mut b, &asked).await;
    assert!(again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(one::<Option<i64>>(c, &format!("SELECT linked_transaction_id FROM account_transactions WHERE id = {deposit}")), Some(half));
}

/// Branch review r1 #5 (rule M3): "Import the rest" is bound to its route, checked before any writer runs: a left-out
/// Binance row that gains a destination whose identity data the Binance writer would refuse is a change, asked again,
/// never refused, the upload still open.
#[tokio::test(flavor = "current_thread")]
async fn a_stale_import_the_rest_is_asked_again_before_any_writer_refuses() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    krakens(c, s.user_id, 1);
    let binance = venue(c, "Exchanges::Binance", "Binance");
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &ledger_csv(&[("kraken", "T1", "BTC", 5, 1), ("binance", "B1", "BTC", 6, 1)])).await;
    let token = token_of(&asked);
    key_on(c, s.user_id, binance, None);
    deltabadger::app_config::set_plain(c, &format!("rust_binance_a12_v1:bucket:{}:{binance}:x", s.user_id), "not json", chrono::Utc::now()).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &hidden(&asked.body)).await;
    assert!(again.body.contains("Something changed since you answered."), "{} {}", again.status, again.body);
    assert_eq!((ledger(c), state_of(c, &token).as_deref()), (before, Some("open")));
}

/// Branch review r1 #6: a late warning on a legacy key's account, with both Binance venues present, completes on its
/// answer (which names the key and its venue).
#[tokio::test(flavor = "current_thread")]
async fn a_late_warning_on_a_legacy_key_with_both_binance_venues_completes() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let (binance, us) = (venue(c, "Exchanges::Binance", "Binance"), venue(c, "Exchanges::BinanceUs", "Binance.US"));
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, binance, e("legacy"), e("s")]).unwrap();
    let legacy = c.last_insert_rowid();
    c.execute("DELETE FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2", [s.user_id, binance]).unwrap();
    key_on(c, s.user_id, us, None);
    let mut b = browser(&app, c, s.user_id);
    let lone = "User ID,UTC_Time,Account,Operation,Coin,Change,Remark\n777,2026-08-17 10:00:00,Spot,Small Assets Exchange BNB,BTC,-1,BTC\n";
    let asked = upload(&app, &mut b, lone).await;
    assert!(options(&asked.body).contains(&legacy.to_string()), "{}", asked.body);
    let warned = answer(&app, &mut b, &asked, &legacy.to_string()).await;
    assert!(warned.body.contains("small-balance conversions"), "{}", warned.body);
    let done = import_answer(&app, &mut b, &warned).await;
    assert!(imported(&done), "{}", done.body);
}

/// Branch review r1 #8 (rule M7): a question's outcome names where its rows would go and the accounts its warnings name,
/// by id.
#[tokio::test(flavor = "current_thread")]
async fn a_questions_outcome_carries_its_destination_and_warning_account_ids() {
    use deltabadger::tracker::import::{run::Destination, service};
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let (kraken, ids) = krakens(c, s.user_id, 1);
    let signed_in: service::SignedIn = std::sync::Arc::new(|_| Ok(true));
    let outcome = service::upload(&app, s.user_id, "deltabadger", rest_csv().into_bytes(), Some("ledger.csv".into()), signed_in, service::permit(&app).await).await.unwrap();
    let service::Outcome::Question { destinations, warning_accounts, .. } = outcome else { panic!("{outcome:?}") };
    assert_eq!(destinations, [Destination { exchange_id: kraken, account_id: Some(ids[0]), rows: 1 }]);
    assert_eq!(warning_accounts.iter().map(|a| (a.account_id, a.exchange_id, a.rows)).collect::<Vec<_>>(), [(ids[0], kraken, Some(1))]);
}

/// Memory review: an account pick is bound to the route its click writes: KuCoin's reading key switching to account 1's
/// read-only key after the question (the trading key lost a permission) is a change, asked again, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn an_account_pick_whose_reading_key_switched_is_asked_again() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let trading = key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let e = |v: &str| seed::cipher().encrypt(v);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, 1, '2026-01-02', '2026-01-02')",
              rusqlite::params![s.user_id, kucoin, e("read-only"), e("s")]).unwrap();
    let read_only = c.last_insert_rowid();
    identity::Account::by_id(c, first).unwrap().unwrap().add_key(c, read_only).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(options(&asked.body).iter().any(|v| v.starts_with(&format!("a{first}~"))), "{}", asked.body);
    c.execute("UPDATE api_keys SET last_sync_error = 'Access Denied' WHERE id = ?1", [trading]).unwrap();
    let before = ledger(c);
    let again = answer(&app, &mut b, &asked, &format!("a{first}")).await;
    assert!(!imported(&again) && again.body.contains("Something changed since you answered."), "{}", again.body);
    assert_eq!(ledger(c), before);
}

/// Memory review (rule M7): a question offering a new account names the sole existing one as it shows it, numbered
/// ("KuCoin¹"), and its outcome carries that account's id.
#[tokio::test(flavor = "current_thread")]
async fn a_question_offering_a_new_account_carries_the_numbered_accounts_id() {
    use deltabadger::tracker::import::{run, service};
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    let mut b = browser(&app, c, s.user_id);
    assert!(imported(&upload(&app, &mut b, &kucoin_file("444")).await));
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL", []).unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "555");
    let signed_in: service::SignedIn = std::sync::Arc::new(|_| Ok(true));
    let outcome = service::upload(&app, s.user_id, "kucoin", kucoin_file("444").into_bytes(), Some("trading.csv".into()), signed_in, service::permit(&app).await).await.unwrap();
    let service::Outcome::Question { answer, warning_accounts, .. } = outcome else { panic!("{outcome:?}") };
    let run::Answer::Warned(_, choices) = &answer else { panic!("{answer:?}") };
    assert!(choices.iter().any(|c| c.accept == Some(run::Accept::New)), "{answer:?}");
    assert!(warning_accounts.iter().any(|a| a.account_id == first && a.exchange_id == kucoin && a.label.number == Some(1)), "{warning_accounts:?} {answer:?}");
}

/// Memory review r3: a handle-shaped `api_key_id` with a long generation is still a handle: beside a file and no token,
/// the request is no file, and the file is never written.
#[tokio::test(flavor = "current_thread")]
async fn a_long_generation_handle_beside_a_file_is_no_file() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    krakens(c, s.user_id, 1);
    let mut b = browser(&app, c, s.user_id);
    assert_eq!(b.get(&app, "/settings/api_keys").await.status, 200);
    let handle = format!("0123456789abcdef.{}.0.{}", "9".repeat(64), "a".repeat(32));
    let clean = ledger_csv(&[("kraken", "C1", "ETH", 9, 1)]);
    let (content_type, body) = common::web::multipart(&[("format", "deltabadger"), ("api_key_id", handle.as_str())], &[("file", "ledger.csv", clean.as_bytes())]);
    let before = ledger(c);
    let answer = b.send_bytes(&app, "POST", "/tracker/import", Some(body), common::web::Csrf::Header, &[("content-type", content_type.as_str())]).await;
    assert_eq!((answer.status, ledger(c)), (422, before), "{}", answer.body);
    assert!(answer.body.contains("Choose a file first."), "{}", answer.body);
}

/// Memory review r3 (rule M7): an account question's options carry notes naming other accounts (here the rows account 1
/// already holds); its outcome carries those accounts' ids.
#[tokio::test(flavor = "current_thread")]
async fn an_account_questions_notes_carry_their_accounts_ids() {
    use deltabadger::tracker::import::{run, service};
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    key_on(c, s.user_id, kucoin, None);
    let first = identity::Account::first(c, s.user_id, kucoin).unwrap().id.unwrap();
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, &kucoin_file("444")).await;
    assert!(imported(&answer(&app, &mut b, &asked, &format!("a{first}")).await));
    // The same rows under a UID no account is known by: which account, each option noting the rows account 1 holds.
    let signed_in: service::SignedIn = std::sync::Arc::new(|_| Ok(true));
    let outcome = service::upload(&app, s.user_id, "kucoin", kucoin_file("777").into_bytes(), Some("trading.csv".into()), signed_in, service::permit(&app).await).await.unwrap();
    let service::Outcome::Question { answer, warning_accounts, .. } = outcome else { panic!("{outcome:?}") };
    assert!(matches!(answer, run::Answer::Ambiguous(_)), "{answer:?}");
    assert!(warning_accounts.iter().any(|a| a.account_id == first && a.exchange_id == kucoin && a.rows.is_some()), "{warning_accounts:?} {answer:?}");
}

/// PLAN-710-importmem A8/C6: a click queued for an import slot is checked once it holds one, inside its transaction, as
/// any click is. What won meanwhile decides: a Cancel, an expiry swept, a sibling's committed refusal leave no file (the
/// ledger as it was, no generation issued); a sibling's committed re-ask (generation 2) makes the queued generation-1
/// click stale, so it is asked again: generation 3, and the sibling's generation-2 handles resolve no more.
#[tokio::test(flavor = "current_thread")]
async fn a_click_queued_for_a_slot_finds_what_won_meanwhile() {
    use deltabadger::tracker::import::service;
    for case in ["cancel", "expiry", "refusal", "re-ask"] {
        let (_d, o, s, _server, app) = alpaca().await;
        let c = &o.primary;
        let (kraken, _) = krakens(c, s.user_id, 1);
        let mut b = browser(&app, c, s.user_id);
        let asked = upload(&app, &mut b, &rest_csv()).await;
        let token = token_of(&asked);
        let handle = common::import::handle_of(&app, token.clone(), hidden(&asked.body)).await.unwrap();
        let before = ledger(c);
        let (mine, other) = (service::permit(&app).await, service::permit(&app).await);
        let mut queued = b.clone();
        let fields = [("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handle.as_str())];
        let click = queued.send(&app, "POST", "/tracker/import", Some(&fields), common::web::Csrf::Header, &[]);
        let sibling = async {
            // The click is admitted and waiting for a slot by now.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let signed_in: service::SignedIn = std::sync::Arc::new(|_| Ok(true));
            match case {
                "cancel" => assert_eq!(service::cancel(&app, s.user_id, token.clone(), signed_in, mine).await.unwrap(), service::Outcome::Cancelled),
                "expiry" => { c.execute("UPDATE import_parked_uploads SET expires_at = '2000-01-01 00:00:00'", []).unwrap(); service::sweep(&app).await; drop(mine) }
                "refusal" => { c.execute("UPDATE import_parked_uploads SET state = 'refused' WHERE token = ?1", [&token]).unwrap(); drop(mine) }
                _ => {
                    identity::new_account(c, s.user_id, kraken, chrono::Utc::now()).unwrap();
                    let outcome = service::confirm(&app, s.user_id, token.clone(), handle.clone(), signed_in, mine).await.unwrap();
                    assert!(matches!(outcome, service::Outcome::Question { .. }), "{outcome:?}");
                }
            }
        };
        let (answer, ()) = tokio::join!(click, sibling);
        drop(other);
        let gens: Vec<i64> = c.prepare("SELECT DISTINCT gen FROM import_parked_choices WHERE token = ?1").unwrap().query_map([&token], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(ledger(c), before, "{case}: nothing written");
        if case == "re-ask" {
            assert!(answer.body.contains("Something changed since you answered."), "{case}: {}", answer.body);
            assert_eq!((state_of(c, &token).as_deref(), gens), (Some("open"), vec![3]), "{case}: only generation 3 resolves");
        } else {
            assert_eq!(answer.status, 422, "{case}: {}", answer.body);
            assert!(gens.iter().all(|g| *g == 1), "{case}: no generation issued: {gens:?}");
        }
    }
}

/// PLAN-710-importmem B1″: a click answered busy changes nothing: its upload stays open, its handle valid, no worker
/// writes later, and the same click writes once a slot is free. One whose upload was cancelled meanwhile is no file.
#[tokio::test(flavor = "current_thread")]
async fn a_busy_click_keeps_its_upload_and_its_handle() {
    use deltabadger::tracker::import::service;
    for cancelled in [false, true] {
        let (_d, o, s, _server, app) = alpaca().await;
        let c = &o.primary;
        krakens(c, s.user_id, 1);
        let mut b = browser(&app, c, s.user_id);
        let asked = upload(&app, &mut b, &rest_csv()).await;
        let token = token_of(&asked);
        let handle = common::import::handle_of(&app, token.clone(), hidden(&asked.body)).await.unwrap();
        let before = ledger(c);
        let fields = [("token", token.as_str()), ("format", "deltabadger"), ("api_key_id", handle.as_str())];
        let held = (service::permit(&app).await, service::permit(&app).await);
        let busy = b.send(&app, "POST", "/tracker/import", Some(&fields), common::web::Csrf::Header, &[]).await;
        assert_eq!(busy.status, 503, "{}", busy.body);
        drop(held);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!((ledger(c), state_of(c, &token).as_deref()), (before, Some("open")), "nothing ran behind the busy answer");
        if cancelled {
            let gone = post(&app, &mut b, "/tracker/import", &[("token", &token), ("format", "deltabadger"), ("cancel", "Cancel")]).await;
            assert_eq!(gone.status, 200, "{}", gone.body);
        }
        let again = b.send(&app, "POST", "/tracker/import", Some(&fields), common::web::Csrf::Header, &[]).await;
        if cancelled { assert_eq!((again.status, ledger(c)), (422, before), "{}", again.body) } else { assert!(imported(&again), "{}", again.body) }
    }
}
