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

// ---- no identity venue (Alpaca): material decides (§1.1 rule 4) ----

async fn alpaca() -> (tempfile::TempDir, store::Opened, seed::Seeded, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
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
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let before = keys(c, &cipher, s.exchange_id);
    let mut b = browser(&app, c, s.user_id);
    let first = save(&app, &mut b, s.exchange_id, None, &[("api_key[key]", "second-key"), ("api_key[secret]", "second-secret"), ("api_key[passphrase]", "paper")]).await;
    assert!(first.status < 400, "{}: {}", first.status, first.body);
    assert_eq!(numbers(c, s.exchange_id), [1, 2]);
    let again = save(&app, &mut b, s.exchange_id, None, &[("api_key[key]", "second-key"), ("api_key[secret]", "second-secret")]).await;
    assert!(again.status < 400, "{}: {}", again.status, again.body);
    assert_eq!(numbers(c, s.exchange_id), [1, 2], "the same material is the same account");
    let account1 = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    let reconnect = save(&app, &mut b, s.exchange_id, Some(account1), &[("api_key[key]", "second-key"), ("api_key[secret]", "second-secret"), ("api_key[passphrase]", "paper")]).await;
    assert!(reconnect.status < 400, "{}: {}", reconnect.status, reconnect.body);
    let mut want = before.clone();
    want.push((2, "second-key".into()));
    assert_eq!(keys(c, &cipher, s.exchange_id), want, "account 1 keeps its key");
    let notice = b.get(&app, "/tracker").await;
    assert!(notice.body.contains("This key belongs to Alpaca") && notice.body.contains("was not changed"), "{}", &notice.body[..notice.body.len().min(2000)]);
}

/// A 2.x form (no `account_id`, no `untargeted`) still saves into number 1, and again replaces that key, as before.
#[tokio::test(flavor = "current_thread")]
async fn a_legacy_form_replaces_number_1s_key() {
    let (_d, o, s, _server, app) = alpaca().await;
    let (c, cipher) = (&o.primary, seed::cipher());
    let mut want = keys(c, &cipher, s.exchange_id);
    want.push((1, "replaced".into()));
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

/// A KuCoin exchange of the Alpaca fixture's user, its key server answering each key's account: `key-a` reads A, `key-b`
/// reads B, any other key's read fails (a 503).
async fn kucoin() -> (tempfile::TempDir, store::Opened, i64, i64, MockServer, deltabadger::web::App) {
    let server = MockServer::start().await;
    for (key, uid) in [("key-a", UID_A), ("key-b", UID_B), ("key-b2", UID_B)] {
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
    let made = b.send(&app, "POST", "/bots/dca_multi_assets/pick_spendable_asset", Some(&[("bots_dca_multi_asset[quote_asset_id]", quote.as_str())]), common::web::Csrf::Header, &[]).await;
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
    let saved = b.send(&app, "POST", "/bots/dca_single_assets/add_api_key", Some(&[("api_key[key]", "second-key"), ("api_key[secret]", "second-secret"), ("api_key[passphrase]", "paper")]),
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
    assert_eq!(b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await.status, 302);
    let key_step = b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    assert_eq!((key_step.status, location(&key_step)), (302, "/rules/withdrawals/confirm_settings/new"), "account 2's withdrawal key is correct: {}", key_step.body);
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert_eq!(confirm.status, 200, "{}", confirm.body);
    assert!(confirm.body.contains("bc1qaccounttw") && !confirm.body.contains("bc1qaccounton"), "account 2's address: {}", confirm.body);
    let made = b.send(&app, "POST", "/rules/withdrawals/confirm_settings", Some(&[("network", "bitcoin"), ("min_amount", "0.1"), ("threshold_type", "min_amount")]),
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
    let mut want = before.clone();
    want.push((2, "reader-2".into()));
    assert_eq!(keys(c, &cipher, s.exchange_id), want, "account 2's reading key; account 1 untouched");
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
