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
    assert_eq!(b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await.status, 302);
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
        assert!(fixes.iter().any(|l| l.contains("key_type=trading") && l.ends_with(&format!("account_id={account}"))), "a fix for account {account}: {fixes:?}");
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
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert!(confirm.status == 200 && !confirm.body.contains("name=\"account_id\""), "a 2.x form: {}", confirm.body);
    // Tab B picks account 2, BTC again.
    let pick = format!("/rules/withdrawals/pick_exchange?account_id={second}");
    assert_eq!(b.send(&app, "POST", &pick, Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
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
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    let confirm = b.get(&app, "/rules/withdrawals/confirm_settings/new").await;
    assert!(confirm.body.contains(&format!("<input type=\"hidden\" name=\"account_id\" value=\"{second}\" />")), "the form names account 2: {}", confirm.body);
    // Tab B picks account 1, BTC again and its address.
    assert_eq!(b.send(&app, "POST", &pick(first), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
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
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
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
async fn upload(app: &deltabadger::web::App, b: &mut common::web::Browser, format: &str, text: &str) -> common::web::Answer {
    assert_eq!(b.get(app, "/settings/api_keys").await.status, 200);
    let (content_type, body) = common::web::multipart(&[("format", format)], &[("file", BINANCE_NAME, text.as_bytes())]);
    b.send_bytes(app, "POST", "/tracker/import", Some(body), common::web::Csrf::Header, &[("content-type", content_type.as_str())]).await
}
/// The question's options (`value`s) and the answer `value` posted back with the parked file.
fn options(body: &str) -> Vec<String> {
    regex::Regex::new(r#"data-value="([^"]+)""#).unwrap().captures_iter(body).map(|m| m[1].to_string()).collect()
}
async fn answer(app: &deltabadger::web::App, b: &mut common::web::Browser, asked: &common::web::Answer, format: &str, value: &str) -> common::web::Answer {
    let token = regex::Regex::new(r#"name="token" id="token" value="([0-9a-f]{16})""#).unwrap().captures(&asked.body).unwrap_or_else(|| panic!("parked: {}", asked.body))[1].to_string();
    b.send(app, "POST", "/tracker/import", Some(&[("token", token.as_str()), ("format", format), ("api_key_id", value)]), common::web::Csrf::Header, &[]).await
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
    let done = upload(&app, &mut b, "kucoin", &kucoin_file("111")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    let accounts = identity::accounts_of(c, s.user_id, kucoin).unwrap();
    assert_eq!(accounts.len(), 1, "one keyless account");
    assert!(rows_on(c, accounts[0].0) > 0);
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM account_transactions WHERE exchange_id = {kucoin} AND api_key_id IS NOT NULL")), 0, "no key named");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, "binance", &binance_file("222")).await;
    assert_eq!(asked.status, 200);
    assert_eq!(options(&asked.body).len(), 2, "Binance or Binance.US: {}", asked.body);
    assert_eq!((one::<i64>(c, "SELECT count(*) FROM account_transactions"), identity::accounts_of(c, s.user_id, us).unwrap().len()), (ledger, 0), "nothing written yet");
    let done = answer(&app, &mut b, &asked, "binance", &format!("v{us}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    let accounts = identity::accounts_of(c, s.user_id, us).unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(rows_on(c, accounts[0].0), 1);
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
    let done = upload(&app, &mut b, "binance", &binance_file("222")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!((rows_on(c, first), rows_on(c, second)), (0, 1), "account 2's file");
    let done = upload(&app, &mut b, "binance", &binance_file("333")).await;
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
    let asked = upload(&app, &mut b, "kucoin", &kucoin_file("444")).await;
    let set = format!("{first}-{second}");
    assert_eq!(options(&asked.body), [format!("a{first}~{set}"), format!("a{second}~{set}"), format!("n{kucoin}~{set}")], "{}", asked.body);
    assert!(asked.body.contains("This file could be from more than one account here. Which one?"), "{}", asked.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written");
    // Account 2 is read as another account meanwhile: the answer naming it is asked again.
    fingerprint(c, second, "Exchanges::Kucoin", "555");
    let again = answer(&app, &mut b, &asked, "kucoin", &format!("a{second}~{set}")).await;
    assert_eq!(options(&again.body), [format!("a{first}~{first}"), format!("n{kucoin}~{first}")], "asked again, account 2 no longer offered: {}", again.body);
    assert!(rows_on(c, first) == 0 && rows_on(c, second) == 0, "never routed to another account");
    // The new-account answer.
    let done = answer(&app, &mut b, &again, "kucoin", &format!("n{kucoin}~{first}")).await;
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
    let asked = upload(&app, &mut b, "binance", &binance_file("222")).await;
    assert_eq!(options(&asked.body), [format!("v{binance}"), format!("v{us}")], "{}", asked.body);
    assert_eq!((rows_on(c, on_binance), rows_on(c, on_us)), (0, 0), "nothing written yet");
    let done = answer(&app, &mut b, &asked, "binance", &format!("v{us}")).await;
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
    let asked = upload(&app, &mut b, "binance", &binance_file("777")).await;
    assert_eq!(options(&asked.body), [format!("v{binance}"), format!("v{us}")], "{}", asked.body);
    let which = answer(&app, &mut b, &asked, "binance", &format!("v{binance}")).await;
    let set = format!("{first}-{keyless}");
    assert_eq!(options(&which.body), [format!("a{first}~{set}"), format!("a{keyless}~{set}"), format!("n{binance}~{set}")], "{}", which.body);
    assert_eq!((rows_on(c, first), rows_on(c, keyless)), (0, 0), "nothing written yet");
}

/// §3: a parked answer is checked against the file's account under the import's lock. Account 2 bound to the file's UID in
/// another tab wins over a parked "account 1"; two parked "a new account" answers for one UID make one account.
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
    let asked = upload(&app, &mut b, "kucoin", &kucoin_file("444")).await;
    assert_eq!(options(&asked.body).len(), 3, "{}", asked.body);
    fingerprint(c, second, "Exchanges::Kucoin", "444");
    let done = answer(&app, &mut b, &asked, "kucoin", &format!("a{first}~{first}-{second}")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    assert_eq!(rows_on(c, first), 0, "never on account 1");
    assert!(rows_on(c, second) > 0, "on the file's account");
    // Two tabs, each asked (accounts 1 and 3 unfingerprinted), each answering "a new account".
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let (one, two) = (upload(&app, &mut b, "kucoin", &kucoin_file("888")).await, upload(&app, &mut b, "kucoin", &kucoin_file("888")).await);
    for asked in [&one, &two] {
        let done = answer(&app, &mut b, asked, "kucoin", &options(&asked.body)[2]).await;
        assert_eq!(done.status, 200, "{}", done.body);
    }
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
    let asked = upload(&app, &mut b, "binance", &binance_file("222")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), ku.to_string()], "{}", asked.body);
    let done = answer(&app, &mut b, &asked, "binance", &kb.to_string()).await;
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
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, "binance", &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, "binance", &kb.to_string()).await;
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
    let asked = upload(&app, &mut b, "binance", &binance_file("777")).await;
    let which = answer(&app, &mut b, &asked, "binance", &format!("v{binance}")).await;
    let first_value = options(&which.body).into_iter().find(|v| v.starts_with(&format!("a{first}"))).unwrap_or_else(|| panic!("{}", which.body));
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "777");
    let done = answer(&app, &mut b, &which, "binance", &first_value).await;
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
    let asked = upload(&app, &mut b, "kucoin", &kucoin_file("444")).await;
    let first_value = options(&asked.body).into_iter().find(|v| v.starts_with(&format!("a{first}"))).unwrap_or_else(|| panic!("{}", asked.body));
    identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, "kucoin", &first_value).await;
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
    let asked = upload(&app, &mut b, "binance", &binance_file("123")).await;
    let done = answer(&app, &mut b, &asked, "binance", &options(&asked.body)[0]).await;
    assert!(imported(&done) && rows_on(c, first) == 1, "{}: {}", done.status, done.body);
    // As before 7a: Binance's account 1 then read another account; its copy of 123's file is a conflict there.
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1", [first]).unwrap();
    fingerprint(c, first, "Exchanges::Binance", "111");
    fingerprint(c, on_us, "Exchanges::BinanceUs", "123");
    // R10: the file is held as a conflict on Binance, so the venue is asked first; Binance.US answered, it lands there.
    let asked = upload(&app, &mut b, "binance", &binance_file("123")).await;
    let done = answer(&app, &mut b, &asked, "binance", &format!("v{us}")).await;
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
    let done = upload(&app, &mut b, "deltabadger", &file).await;
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
    let done = upload(&app, &mut b, "deltabadger", &file).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    let first = identity::Account::first(c, s.user_id, s.exchange_id).unwrap().id.unwrap();
    assert_eq!(rows_on(c, first), last as i64, "account 1's rows imported");
    assert!(done.body.contains(&format!("No account connected for: {name} 2. Those rows were left out.")), "{}", done.body);
}

/// §3, D-TRACKER-EMPTY-IMPORT: the empty tracker's welcome screen offers Import beside Connect, so a user with no key can
/// start from a venue's export (the import makes the keyless account).
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
    assert_eq!(links(&page.body, "/tracker/import/new").len(), 1, "Import: {}", page.body);
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
    for uid in ["111", "222"] { let done = upload(&app, &mut b, "kucoin", &kucoin_file(uid)).await; assert!(imported(&done), "{uid}: {}: {}", done.status, done.body); }
    let accounts: Vec<i64> = identity::accounts_of(c, s.user_id, kucoin).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(accounts.len(), 2, "one account per file");
    let before: Vec<_> = accounts.iter().map(|a| totals(c, *a)).collect();
    assert!(before.iter().all(|t| !t.is_empty()));
    deltabadger::rotation::clear(c, chrono::Utc::now()).unwrap();
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let asked = upload(&app, &mut b, "kucoin", &kucoin_file("111")).await;
    let set = format!("{}-{}", accounts[0], accounts[1]);
    assert_eq!(options(&asked.body), [format!("a{}~{set}", accounts[0]), format!("a{}~{set}", accounts[1]), format!("n{kucoin}~{set}")], "{}", asked.body);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger, "nothing written before the answer");
    let done = answer(&app, &mut b, &asked, "kucoin", &format!("a{}~{set}", accounts[0])).await;
    assert!(done.status == 200 && done.body.contains("Every transaction in this file is already in your history."), "{}: {}", done.status, done.body);
    let done = upload(&app, &mut b, "kucoin", &kucoin_file("222")).await;
    assert!(done.status == 200 && done.body.contains("Every transaction in this file is already in your history."), "the one unread account: {}: {}", done.status, done.body);
    assert_eq!(identity::accounts_of(c, s.user_id, kucoin).unwrap().len(), 2, "no account made");
    assert_eq!(accounts.iter().map(|a| totals(c, *a)).collect::<Vec<_>>(), before, "the totals hold");
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM account_transactions"), ledger);
}

/// §9.7: an account whose export another account holds as a conflict (imported there before 7a) is named on the tracker:
/// its history stays where it is and is not counted twice.
#[tokio::test(flavor = "current_thread")]
async fn the_tracker_says_when_an_account_s_export_is_held_on_another() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let mut b = browser(&app, c, s.user_id);
    let done = upload(&app, &mut b, "kucoin", &kucoin_file("222")).await;
    assert!(imported(&done), "{}: {}", done.status, done.body);
    // As before 7a: account 1's own key then read another account.
    let first = identity::accounts_of(c, s.user_id, kucoin).unwrap()[0].0;
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1", [first]).unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    let page = b.get(&app, "/tracker").await;
    assert!(page.status == 200 && !page.body.contains("history was imported on"), "no account of that export yet: {}", page.body);
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Kucoin", "222");
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    assert_eq!(page.body.matches("history was imported on KuCoin<sup class=").count() + page.body.matches("history was imported on KuCoin<sup>").count(), 1, "{}", page.body);
}

/// §9.7 rule H1: when the account's sync could not match some entries with the export another account holds, the notice
/// says so instead of promising nothing is counted twice.
#[tokio::test(flavor = "current_thread")]
async fn the_held_notice_says_when_the_export_could_not_be_matched() {
    let (_d, o, s, _server, app) = alpaca().await;
    let c = &o.primary;
    let _lock = own(_d.path(), c, &app);
    let kucoin = venue(c, "Exchanges::Kucoin", "KuCoin");
    let mut b = browser(&app, c, s.user_id);
    assert!(imported(&upload(&app, &mut b, "kucoin", &kucoin_file("222")).await));
    let first = identity::accounts_of(c, s.user_id, kucoin).unwrap()[0].0;
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1", [first]).unwrap();
    fingerprint(c, first, "Exchanges::Kucoin", "111");
    let second = identity::new_account(c, s.user_id, kucoin, chrono::Utc::now()).unwrap();
    fingerprint(c, second, "Exchanges::Kucoin", "222");
    identity::mark_held_unmatched(c, second, first, chrono::Utc::now()).unwrap();
    let page = b.get(&app, "/tracker").await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("could not be matched") && !page.body.contains("is not counted twice"), "{}", page.body);
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
    let done = upload(&app, &mut b, "deltabadger", &file).await;
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
    assert!(imported(&upload(&app, &mut b, "deltabadger", &file).await));
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
    let done = upload(&app2, &mut b2, "deltabadger", &export.body).await;
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

/// Answers Alpaca's order read (filled) and, as it answers, saves account 1's read-only key from another request.
struct SaveWhileRead(std::path::PathBuf, i64, i64);
impl wiremock::Respond for SaveWhileRead {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let c = Connection::open(&self.0).unwrap();
        let cipher = seed::cipher();
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, ?5, 1, 'now', 'now')",
                  rusqlite::params![self.1, self.2, cipher.encrypt("other-request-key"), cipher.encrypt("other-secret"), cipher.encrypt("paper")]).unwrap();
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
    let saved: Vec<String> = c.prepare("SELECT key FROM api_keys WHERE key_type = 2").unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap().map(|k| cipher.decrypt(&k.unwrap()).unwrap()).collect();
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
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, "binance", &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    identity::new_account(c, s.user_id, binance, chrono::Utc::now()).unwrap();
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "777");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, "binance", &kb.to_string()).await;
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
    let asked = upload(&app, &mut b, "binance", &binance_file("123")).await;
    let done = answer(&app, &mut b, &asked, "binance", &options(&asked.body)[0]).await;
    assert!(imported(&done) && rows_on(c, first) == 1, "{}: {}", done.status, done.body);
    c.execute("UPDATE exchange_accounts SET fingerprint = NULL, account_id_enc = NULL WHERE id = ?1", [first]).unwrap();
    fingerprint(c, first, "Exchanges::Binance", "111");
    fingerprint(c, on_us, "Exchanges::BinanceUs", "123");
    let fresh = upload(&app, &mut b, "binance", &binance_file("123")).await;
    assert_eq!(options(&fresh.body), [kb.to_string(), format!("v{us}")], "the venue is asked: {}", fresh.body);
    assert_eq!(rows_on(c, on_us), 0, "nothing on Binance.US yet");
    let done = answer(&app, &mut b, &fresh, "binance", &format!("v{us}")).await;
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
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
    b.get(&app, "/rules/withdrawals/add_api_key/new").await;
    assert_eq!(b.get(&app, "/rules/withdrawals/confirm_settings/new").await.status, 200);
    // Tab B picks account 1.
    assert_eq!(b.send(&app, "POST", &pick(first), Some(&[("bots_dca_single_asset[exchange_id]", exchange.as_str()), ("untargeted", "1")]), common::web::Csrf::Header, &[]).await.status, 302);
    b.send(&app, "POST", "/rules/withdrawals/pick_asset", Some(&[("bots_dca_single_asset[asset_id]", btc.as_str())]), common::web::Csrf::Header, &[]).await;
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
    let mut b = browser(&app, c, s.user_id);
    let asked = upload(&app, &mut b, "binance", &binance_file("777")).await;
    assert_eq!(options(&asked.body), [kb.to_string(), format!("v{us}")], "{}", asked.body);
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [kb]).unwrap();
    c.execute("DELETE FROM api_keys WHERE id = ?1", [kb]).unwrap();
    let on_us = identity::new_account(c, s.user_id, us, chrono::Utc::now()).unwrap();
    fingerprint(c, on_us, "Exchanges::BinanceUs", "777");
    let ledger: i64 = one(c, "SELECT count(*) FROM account_transactions");
    let again = answer(&app, &mut b, &asked, "binance", &kb.to_string()).await;
    let values = options(&again.body);
    assert!(!values.is_empty() && values.iter().all(|v| v.starts_with('a') || v.starts_with(&format!("n{binance}~"))), "Binance's accounts asked: {}", again.body);
    assert_eq!((rows_on(c, on_us), one::<i64>(c, "SELECT count(*) FROM account_transactions")), (0, ledger), "nothing written, never on Binance.US");
}
