//! #710 slice 6 (PLAN-710-6 §8): names, settings per account, REST/MCP, duplicate standing and Merge, proven on two
//! accounts of one exchange (tests/common/two_accounts.rs). A second account cannot be made otherwise until slice 7.
mod common;
use common::seed;
use common::two_accounts::Two;
use deltabadger::identity::{self, Label};
use deltabadger::store::{self, Paths};
use rusqlite::Connection;

fn install() -> (tempfile::TempDir, store::Opened) {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o)
}
#[allow(dead_code)]
fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T { c.query_row(sql, [], |r| r.get(0)).unwrap() }
fn remove(c: &Connection, account: i64) { c.execute("UPDATE exchange_accounts SET removed_at = 'now' WHERE id = ?1", [account]).unwrap(); }
fn plain_sql(c: &Connection, account: i64) -> String {
    one(c, &format!("SELECT {} FROM exchange_accounts x JOIN exchanges e ON e.id = x.exchange_id WHERE x.id = {account}", identity::plain_label_sql("e.name", "x.id")))
}

// ---- commit 1: names (§3, T1-T3) ----

/// T1: one live account reads `Kraken`; two read `Kraken 1`/`Kraken 2` and `Kraken<sup>1</sup>`, the name escaped; live 1
/// and 3 read their numbers; a lone live 2 reads `Kraken`; the SQL form is `plain()`.
#[test]
fn t1_the_label_shows_the_number_only_while_the_exchange_has_two_live_accounts() {
    let (_d, o) = install();
    let c = &o.primary;
    let seeded = seed::seed_kraken(c, &seed::cipher());
    let lone = identity::Account::first(c, seeded.user_id, seeded.exchange_id).unwrap().ensure(c, chrono::Utc::now()).unwrap().id.unwrap();
    assert_eq!(identity::label(c, lone).unwrap(), Label { name: "Kraken".into(), number: None });
    assert_eq!(identity::label(c, lone).unwrap().plain(), "Kraken");
    assert_eq!(plain_sql(c, lone), "Kraken");

    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let [a, b] = [two.id(0), two.id(1)];
    assert_eq!((identity::label(c, a).unwrap().plain(), identity::label(c, b).unwrap().plain()), ("Kraken 1".into(), "Kraken 2".into()));
    assert_eq!(identity::label(c, a).unwrap().html(), "Kraken<sup>1</sup>");
    assert_eq!(Label { name: "A&B".into(), number: Some(2) }.html(), "A&amp;B<sup>2</sup>");
    for id in [a, b] { assert_eq!(plain_sql(c, id), identity::label(c, id).unwrap().plain()); }
    let labels = identity::labels(c, two.seeded.user_id).unwrap();
    assert_eq!(labels[&b].plain(), "Kraken 2");

    let third = two.third(c).id.unwrap();
    remove(c, b);
    assert_eq!((identity::label(c, a).unwrap().plain(), identity::label(c, third).unwrap().plain()), ("Kraken 1".into(), "Kraken 3".into()));
    assert_eq!(identity::label(c, b).unwrap().plain(), "Kraken 2", "a removed account always shows its number");
    remove(c, a);
    remove(c, third);
    c.execute("UPDATE exchange_accounts SET removed_at = NULL WHERE id = ?1", [b]).unwrap();
    assert_eq!(identity::label(c, b).unwrap().plain(), "Kraken", "a lone live 2");
    for id in [a, b, third] { assert_eq!(plain_sql(c, id), identity::label(c, id).unwrap().plain()); }
}

/// The app over `dir`'s install, a market-data provider configured (the populated tracker page needs one).
fn web_app(dir: &std::path::Path, c: &Connection) -> deltabadger::web::App {
    c.execute("UPDATE users SET setup_completed = 1, confirmed_at = '2026-01-01'", []).unwrap();
    let app = common::web::app(dir, "engine-test-secret", common::web::TestClock::at("2026-09-20T12:00:00Z"));
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    for (key, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(c, &app.cipher, key, value, app.now()).unwrap();
    }
    app
}
/// A signed-in browser of `owner`.
fn browser(app: &deltabadger::web::App, c: &Connection, owner: i64) -> common::web::Browser {
    use deltabadger::web::session;
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
    let data = session::SessionData { user: Some((owner, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None }
}
async fn get(app: &deltabadger::web::App, c: &Connection, owner: i64, path: &str) -> common::web::Answer {
    browser(app, c, owner).send(app, "GET", path, None, common::web::Csrf::None, &[]).await
}
/// An MCP tool's answer (`tools::call`'s `Done`), called by `owner`'s personal token.
fn mcp(c: &Connection, app: &deltabadger::web::App, owner: i64, name: &str, args: serde_json::Value) -> serde_json::Value {
    use deltabadger::web::{bearer::Bearer, mcp::tools::{self, Called}};
    let on: serde_json::Map<String, serde_json::Value> = tools::NAMES.iter().map(|t| (t.to_string(), serde_json::json!(true))).collect();
    c.execute("UPDATE users SET mcp_settings=?1, rest_settings=?1 WHERE id=?2", rusqlite::params![serde_json::json!({"tool_permissions": on}).to_string(), owner]).unwrap();
    c.execute("INSERT OR IGNORE INTO oauth_applications(name,uid,personal_access_token,personal_owner_id,created_at,updated_at) VALUES('test','test-'||?1,1,?1,'2026-09-10','2026-09-10')", [owner]).unwrap();
    let application: i64 = c.query_row("SELECT id FROM oauth_applications WHERE personal_owner_id = ?1", [owner], |r| r.get(0)).unwrap();
    let who = Bearer { user_id: owner, application_id: application, token_id: 1 };
    match tools::call(c, app, who, name, &args).unwrap() { Called::Done(v) => v, _ => panic!("{name}: not answered at once") }
}
fn cut(body: &str, from: &str, len: usize) -> String { body.find(from).map_or_else(|| format!("<{from} not found>"), |at| body[at..(at + len).min(body.len())].to_string()) }

/// T2: on two accounts every page names each account: the tracker chips and the sync warning, the settings key list, the
/// bot switch's current and other items, the rule tile, the gap line and the gap modal.
#[tokio::test(flavor = "current_thread")]
async fn t2_pages_on_two_accounts_carry_the_number() {
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    let user = two.seeded.user_id;
    two.row(c, 0, 4, "BTC", "1", Some("t-a"), None, "2026-09-10 10:00:00");
    two.row(c, 1, 4, "BTC", "2", Some("t-b"), None, "2026-09-11 10:00:00");
    c.execute("UPDATE api_keys SET last_sync_error = 'Kraken said no' WHERE id = ?1", [two.keys[1]]).unwrap();
    let app = web_app(dir.path(), c);
    let tracker = get(&app, c, user, "/tracker").await;
    assert_eq!(tracker.status, 200, "{}", &tracker.body[..tracker.body.len().min(400)]);
    assert!(tracker.body.contains(">Kraken<sup>1</sup>") && tracker.body.contains(">Kraken<sup>2</sup>"), "chips");
    assert!(cut(&tracker.body, "sync-warnings", 600).contains("Kraken 2"), "{}", cut(&tracker.body, "sync-warnings", 600));

    let settings = get(&app, c, user, "/settings/api_keys").await;
    assert_eq!(settings.status, 200);
    assert!(settings.body.contains("Kraken<sup>1</sup>") && settings.body.contains("Kraken<sup>2</sup>"), "{}", cut(&settings.body, "data-api-key", 800));

    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [two.bots[1]]).unwrap();
    let bot = get(&app, c, user, &format!("/bots/{}", two.bots[1])).await;
    assert_eq!(bot.status, 200, "{}", &bot.body[..bot.body.len().min(400)]);
    let menu = cut(&bot.body, "dropdown__item--current", 12000);
    assert!(menu.contains("Kraken<sup>2</sup>"), "the current item: {menu}");
    assert!(menu.contains("Kraken<sup>1</sup>"), "the other account's item: {menu}");

    two.rule(c, &cipher, 1, two.seeded.btc, 0);
    let rules = get(&app, c, user, "/rules").await;
    assert_eq!(rules.status, 200, "{}", &rules.body[..rules.body.len().min(400)]);
    assert!(rules.body.contains("<span class=\"rule__exchange\">Kraken<sup>2</sup></span>"), "{}", cut(&rules.body, "rule__exchange", 200));

    // The gap line names its account: the reading key's name is the account's label.
    let reading = deltabadger::tracker::import::run::reading(c, user).unwrap();
    assert_eq!(reading.iter().map(|k| k.exchange_name.as_str()).collect::<Vec<_>>(), ["Kraken 1", "Kraken 2"]);
    let modal = get(&app, c, user, &format!("/tracker/history_gap?account_id={}", two.id(1))).await;
    assert_eq!(modal.status, 200, "{}", &modal.body[..modal.body.len().min(400)]);
    assert!(modal.body.contains("Kraken 2"), "the modal: {}", &modal.body[..modal.body.len().min(1500)]);
}

/// T3: plain text says `Kraken 2`: account 2's bot's end-of-funds and failure mails, MCP `list_bots` and
/// `get_bot_details`, and a stored bot error's venue (`model::exchange_name`).
#[tokio::test(flavor = "current_thread")]
async fn t3_plain_text_names_the_account() {
    use deltabadger::engine::notice::Notice;
    use deltabadger::mail::render;
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let urls = render::Urls { root: "http://localhost:3000".into() };
    let mail = |notice: Notice| render::for_notice(c, two.bots[1], &notice, "noreply@example.com", &urls).unwrap().unwrap();
    assert!(mail(Notice::EndOfFunds { quote_asset_id: Some(two.seeded.quote) }).html.contains("Kraken 2"));
    assert!(mail(Notice::StoppedByError { error: "Insufficient funds".into() }).html.contains("KRAKEN 2"), "the mailer upper-cases the venue");
    assert!(!render::for_notice(c, two.bots[0], &Notice::EndOfFunds { quote_asset_id: Some(two.seeded.quote) }, "noreply@example.com", &urls).unwrap().unwrap().html.contains("Kraken 2"));
    let bot = deltabadger::engine::model::load_bot(c, two.bots[1]).unwrap();
    assert_eq!(deltabadger::engine::model::exchange_name(c, &bot).unwrap(), "Kraken 2");
    let app = web_app(dir.path(), c);
    let listed = mcp(c, &app, two.seeded.user_id, "list_bots", serde_json::json!({}));
    assert!(listed["content"][0]["text"].as_str().unwrap().contains("| Kraken 2 |") && listed.to_string().contains("| Kraken 1 |"), "{listed}");
    let details = mcp(c, &app, two.seeded.user_id, "get_bot_details", serde_json::json!({"bot_id": two.bots[1]}));
    assert!(details.to_string().contains("Exchange: Kraken 2"), "{details}");
}

// ---- commit 2: settings per account (§4.1, T4-T5) ----

fn status(c: &Connection, bot: i64) -> i64 { c.query_row("SELECT status FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap() }
/// Bot `n` of `two` settles: an order of it still polled (open on the venue).
fn settle(c: &Connection, two: &Two, n: usize) { two.order(c, n, &format!("open-{n}"), 0, Some(1)); }
fn key_material(c: &Connection, cipher: &deltabadger::crypto::Cipher, key: i64) -> String {
    cipher.decrypt(&c.query_row("SELECT key FROM api_keys WHERE id = ?1", [key], |r| r.get::<_, String>(0)).unwrap()).unwrap()
}
/// A signed-in browser that has read the settings page (its csrf token).
async fn on_settings(app: &deltabadger::web::App, c: &Connection, owner: i64) -> common::web::Browser {
    let mut b = browser(app, c, owner);
    assert_eq!(b.get(app, "/settings/api_keys").await.status, 200);
    b
}

/// T4: deleting account 2's trading key stops only account 2's working bots; account 1 settling does not refuse it,
/// account 2 settling does.
#[tokio::test(flavor = "current_thread")]
async fn t4_a_key_delete_acts_on_its_account_only() {
    for settling in [None, Some(0), Some(1)] {
        let (dir, o) = install();
        let c = &o.primary;
        let two = Two::kraken(c, &seed::cipher());
        if let Some(n) = settling { settle(c, &two, n); }
        let app = web_app(dir.path(), c);
        let mut b = on_settings(&app, c, two.seeded.user_id).await;
        let answer = b.send(&app, "DELETE", &format!("/settings/destroy_api_key/{}", two.keys[1]), Some(&[]), common::web::Csrf::Header, &[]).await;
        let gone = !one::<bool>(c, &format!("SELECT EXISTS(SELECT 1 FROM api_keys WHERE id = {})", two.keys[1]));
        if settling == Some(1) {
            assert_eq!(answer.status, 422, "account 2 settles");
            assert!(!gone && status(c, two.bots[1]) == 1, "nothing changed");
        } else {
            assert_eq!(answer.status, 200, "{settling:?}: {}", answer.body);
            assert!(gone, "{settling:?}");
            assert_eq!((status(c, two.bots[0]), status(c, two.bots[1])), (1, 2), "{settling:?}: only account 2's bot stops");
        }
    }
}

struct Quiet;
impl deltabadger::web::settings::keys::Logger for Quiet { fn warn(&self, _: &str) {} }

/// T4: a bot sheet's key save on account 2 (Alpaca, validated by its host) changes account 2's key only.
#[tokio::test(flavor = "current_thread")]
async fn t4_a_bot_sheet_save_on_account_2_changes_only_its_key() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let kraken = Two::kraken(c, &cipher);
    let alpaca = Two::venue(c, &cipher, &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    c.execute("UPDATE bots SET status = 2 WHERE exchange_id = ?1", [alpaca.seeded.exchange_id]).unwrap();
    let app = web_app(dir.path(), c).with_settings_key_boundary(deltabadger::venue::alpaca::Hosts::all(&server.uri()), std::sync::Arc::new(Quiet)).unwrap();
    let mut b = browser(&app, c, kraken.seeded.user_id);
    assert_eq!(b.get(&app, &format!("/bots/{}", alpaca.bots[1])).await.status, 200);
    let saved = b.send(&app, "POST", &format!("/bots/{}/add_api_key", alpaca.bots[1]), Some(&[("api_key[key]", "new-key"), ("api_key[secret]", "new-secret"), ("api_key[passphrase]", "paper")]),
                       common::web::Csrf::Header, &[]).await;
    assert!(saved.status < 400, "{}: {}", saved.status, saved.body);
    assert_eq!((key_material(c, &cipher, alpaca.keys[0]), key_material(c, &cipher, alpaca.keys[1])), ("account-1-key".into(), "new-key".into()));
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM api_keys WHERE exchange_id = {}", alpaca.seeded.exchange_id)), 2, "no key made");
}

async fn json_save(app: &deltabadger::web::App, b: &mut common::web::Browser, exchange: &str, account: Option<&str>) -> common::web::Answer {
    let mut form = vec![("api_key[exchange_id]", exchange), ("api_key[key_type]", "trading"), ("api_key[key]", "new-key"), ("api_key[secret]", "new-secret")];
    if let Some(a) = account { form.push(("api_key[account_id]", a)); }
    b.send(app, "POST", "/api/api_keys", Some(&form), common::web::Csrf::Header, &[]).await
}

/// T5: the browser's JSON key save naming Kraken with two live accounts is a 422; with account 2's `account_id` the key is
/// saved there; a foreign `account_id` is a 404.
#[tokio::test(flavor = "current_thread")]
async fn t5_the_json_key_save_names_its_account() {
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    let app = web_app(dir.path(), c);
    let mut b = on_settings(&app, c, two.seeded.user_id).await;
    let exchange = two.seeded.exchange_id.to_string();
    let ambiguous = json_save(&app, &mut b, &exchange, None).await;
    assert_eq!(ambiguous.status, 422, "{}", ambiguous.body);
    let message: serde_json::Value = serde_json::from_str(&ambiguous.body).unwrap();
    assert_eq!(message["data"], false);
    assert!(message["message"].as_str().unwrap().contains(&format!("Kraken 2 (account_id {})", two.id(1))), "{message}");
    assert_eq!((key_material(c, &cipher, two.keys[0]), key_material(c, &cipher, two.keys[1])), ("test-key".into(), "account-2-key".into()), "nothing saved");
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1 + 100, ?2, 1, 'now', 'now')", [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    let foreign = c.last_insert_rowid().to_string();
    assert_eq!(json_save(&app, &mut b, &exchange, Some(&foreign)).await.status, 404, "another user's account");
    let second = two.id(1).to_string();
    let saved = json_save(&app, &mut b, &exchange, Some(&second)).await;
    assert_eq!(saved.status, 201, "{}", saved.body);
    assert_eq!((key_material(c, &cipher, two.keys[0]), key_material(c, &cipher, two.keys[1])), ("test-key".into(), "new-key".into()));
}

// ---- commit 3: REST/MCP and switches (§4.2-§4.5, T6-T10) ----

/// T6: `pick` with no account gives the exchange's account with one live account and `Ambiguous` with two (labels and
/// ids, by number); a posted id of another user, another exchange, a removed or an unknown account is `NotFound`.
#[test]
fn t6_pick_resolves_a_posted_account_or_names_the_choice() {
    use identity::Pick;
    let (_d, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    let (user, exchange) = (two.seeded.user_id, two.seeded.exchange_id);
    assert_eq!(identity::pick(c, user, exchange, None).unwrap(),
               Pick::Ambiguous(vec![(Label { name: "Kraken".into(), number: Some(1) }, two.id(0)), (Label { name: "Kraken".into(), number: Some(2) }, two.id(1))]));
    assert_eq!(identity::Pick::required("Kraken", &[(Label { name: "Kraken".into(), number: Some(1) }, 12), (Label { name: "Kraken".into(), number: Some(2) }, 34)]),
               "Kraken has 2 accounts: Kraken 1 (account_id 12), Kraken 2 (account_id 34). Name one with account_id.");
    assert_eq!(identity::pick(c, user, exchange, Some(two.id(1))).unwrap(), Pick::One(two.accounts[1]));
    let other = Two::venue(c, &cipher, &two.seeded, "Exchanges::Binance", "Binance");
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1 + 100, ?2, 1, 'now', 'now')", [user, exchange]).unwrap();
    let foreign = c.last_insert_rowid();
    for id in [foreign, other.id(1), 999_999] { assert_eq!(identity::pick(c, user, exchange, Some(id)).unwrap(), Pick::NotFound, "{id}"); }
    remove(c, two.id(1));
    assert_eq!(identity::pick(c, user, exchange, Some(two.id(1))).unwrap(), Pick::NotFound, "a removed account");
    assert_eq!(identity::pick(c, user, exchange, None).unwrap(), Pick::One(two.accounts[0]), "one live account");
}

/// REST over HTTP with an `api` token of `owner` that has every tool on; MCP through `tools::call` (`mcp`).
async fn rest(app: &deltabadger::web::App, c: &Connection, owner: i64, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
    use axum::{body::{to_bytes, Body}, http::Request};
    use tower::ServiceExt;
    if !one::<bool>(c, "SELECT EXISTS(SELECT 1 FROM oauth_access_tokens WHERE token = 'slice6-api')") {
        c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('s6','s6','http://localhost/cb',0,'mcp api','2026-09-01 12:00:00','2026-09-01 12:00:00')", []).unwrap();
        let application = c.last_insert_rowid();
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'slice6-api','api','2026-09-20 11:00:00',86400)", (application, owner)).unwrap();
        c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-01 12:00:00','2026-09-01 12:00:00')", (owner, application)).unwrap();
        let on: serde_json::Map<String, serde_json::Value> = deltabadger::web::mcp::tools::NAMES.iter().map(|t| (t.to_string(), serde_json::json!(true))).collect();
        c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1", [serde_json::json!({"tool_permissions": on}).to_string()]).unwrap();
    }
    let key = KEY.with(|k| k.borrow().clone()).unwrap_or_else(|| format!("k-{}", rand_key()));
    let mut request = Request::builder().method(method).uri(path).header("host", "localhost:3000").header("authorization", "Bearer slice6-api").header("accept", "application/json")
        .header("idempotency-key", key);
    if body.is_some() { request = request.header("content-type", "application/json"); }
    let response = deltabadger::web::router(app.clone()).oneshot(request.body(Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let text = String::from_utf8(to_bytes(response.into_body(), 64_000_000).await.unwrap().to_vec()).unwrap();
    (status, serde_json::from_str(&text).unwrap_or(serde_json::json!(text)))
}
thread_local! { static KEY: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }
fn rand_key() -> u64 { use std::sync::atomic::{AtomicU64, Ordering}; static N: AtomicU64 = AtomicU64::new(0); N.fetch_add(1, Ordering::Relaxed) }
fn mcp_text(v: &serde_json::Value) -> String { v["content"][0]["text"].as_str().unwrap_or_default().to_string() }
fn counts(c: &Connection) -> (i64, i64, i64) {
    (one(c, "SELECT count(*) FROM rust_api_orders"), one(c, "SELECT count(*) FROM bots"), one(c, "SELECT count(*) FROM exchange_account_rules"))
}

/// T7: every write that names only Kraken (two live accounts), over REST and MCP, is the same 422 naming both accounts and
/// changes nothing; with account 2's `account_id` the order is queued on 2, the bot and the rule are 2's, the cancel goes
/// through 2.
#[tokio::test(flavor = "current_thread")]
async fn t7_writes_naming_only_the_exchange_ask_which_account() {
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    let user = two.seeded.user_id;
    c.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, 'now', 'now')", [two.seeded.btc, two.seeded.exchange_id]).unwrap();
    let app = web_app(dir.path(), c);
    let wanted = format!("Kraken has 2 accounts: Kraken 1 (account_id {}), Kraken 2 (account_id {}). Name one with account_id.", two.id(0), two.id(1));
    let before = counts(c);
    let order = serde_json::json!({"exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR", "amount": 0.001});
    for tool in ["market_buy", "market_sell", "limit_buy", "limit_sell"] {
        let mut args = order.clone();
        if tool.starts_with("limit") { args["price"] = serde_json::json!(50000); }
        let answer = mcp(c, &app, user, tool, args.clone());
        assert!(mcp_text(&answer).contains(&wanted) && answer["isError"] == true, "MCP {tool}: {answer}");
        args["type"] = serde_json::json!(tool);
        let (status, body) = rest(&app, c, user, "POST", "/api/v1/orders", Some(args)).await;
        assert_eq!((status, body["error"]["code"].as_str()), (422, Some("account_required")), "REST {tool}: {body}");
    }
    let cancel = mcp(c, &app, user, "cancel_order", serde_json::json!({"order_id": "abc", "exchange_name": "Kraken"}));
    assert!(mcp_text(&cancel).contains(&wanted), "{cancel}");
    let (status, _) = rest(&app, c, user, "DELETE", "/api/v1/orders/abc?exchange_name=Kraken", None).await;
    assert_eq!(status, 422);
    let bot = serde_json::json!({"exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR", "quote_amount": 10, "interval": "day"});
    for (tool, rest_type, args) in [("create_bot", "dca", bot.clone()), ("create_signal_bot", "signal", serde_json::json!({"exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR"}))] {
        let answer = mcp(c, &app, user, tool, args.clone());
        assert!(mcp_text(&answer).contains(&wanted), "MCP {tool}: {answer}");
        let mut args = args; args["type"] = serde_json::json!(rest_type);
        let (status, body) = rest(&app, c, user, "POST", "/api/v1/bots", Some(args)).await;
        assert_eq!(status, 422, "REST {tool}: {body}");
    }
    let (status, body) = rest(&app, c, user, "POST", "/api/v1/rules", Some(serde_json::json!({"exchange_name": "Kraken", "asset": "BTC", "address": "bc1-x"}))).await;
    assert_eq!((status, body["error"]["code"].as_str()), (422, Some("account_required")), "{body}");
    assert_eq!(counts(c), before, "nothing queued, created or cancelled");

    // With account 2's account_id.
    let caller = deltabadger::web::bot_api::orders::Caller { user, dry_run: false, mcp: true };
    let mut args = order.as_object().unwrap().clone();
    args.insert("account_id".into(), serde_json::json!(two.id(1)));
    let placed = deltabadger::web::bot_api::orders::place(c, &app, caller, deltabadger::web::bot_api::orders::Kind::MarketBuy, &args, None).unwrap();
    let deltabadger::web::bot_api::orders::Placed::Queued(queued) = placed else { panic!("not queued") };
    assert_eq!(one::<i64>(c, &format!("SELECT account_id FROM rust_api_order_standalone WHERE order_id = {}", queued.id)), two.id(1));
    let created = mcp(c, &app, user, "create_signal_bot", serde_json::json!({"exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR", "account_id": two.id(1)}));
    let id = one::<i64>(c, "SELECT max(id) FROM bots");
    assert_eq!(identity::account_of_bot(c, id).unwrap(), Some(two.id(1)), "{created}");
    let (status, body) = rest(&app, c, user, "POST", "/api/v1/rules", Some(serde_json::json!({"exchange_name": "Kraken", "asset": "BTC", "address": "bc1-x", "account_id": two.id(1)}))).await;
    // The rule is account 2's: its withdrawal key is looked up there (account 1 has none either; the refusal names 2).
    assert_eq!((status, body["error"]["code"].as_str()), (403, Some("withdrawal_key_missing")), "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("Kraken 2"), "{body}");
    let job = deltabadger::web::bot_api::orders::cancel(c, caller, &serde_json::json!("abc"), &serde_json::json!("Kraken"), &serde_json::json!(two.id(1))).unwrap();
    let deltabadger::web::bot_api::orders::Cancel::Send(job) = job else { panic!("not sent") };
    assert_eq!(job.account().and_then(|a| a.id), Some(two.id(1)), "the cancel goes through account 2's key");
}

/// T8: bot switches over the shared resolver, for a DCA, an index and a signal bot: a stopped bot switched by exchange
/// only to Kraken (two accounts) is a 422 and nothing changes; an account-only switch moves it to 2; Coinbase with Kraken
/// 2's account_id and a removed account_id are 404s; its own exchange alone keeps its account.
#[tokio::test(flavor = "current_thread")]
async fn t8_bot_switches_resolve_once_before_any_write() {
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    let other = Two::venue(c, &cipher, &two.seeded, "Exchanges::Coinbase", "Coinbase");
    let user = two.seeded.user_id;
    let app = web_app(dir.path(), c);
    let index = seed::insert_bot(c, &other.seeded, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE bots SET type = 'Bots::DcaIndex', settings = json_set(settings, '$.index_type', 'top', '$.num_coins', 3, '$.allocation_flattening', 0) WHERE id = ?1", [index]).unwrap();
    let signal = seed::insert_bot(c, &other.seeded, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE bots SET type = 'Bots::Signal', settings = json_set(settings, '$.base_asset_id', ?2, '$.quote_asset_id', ?3) WHERE id = ?1", [signal, two.seeded.btc, two.seeded.quote]).unwrap();
    for bot in [other.bots[0], index, signal] {
        c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).unwrap();
        identity::place_bot(c, bot, user, other.seeded.exchange_id, None, chrono::Utc::now()).unwrap();
        let switch = |exchange: Option<i64>, account: Option<i64>| identity::pick_switch(c, user, bot, exchange, account).unwrap();
        assert!(matches!(switch(Some(two.seeded.exchange_id), None), identity::Switch::Ambiguous(ref l) if l.len() == 2), "{bot}");
        assert_eq!(switch(Some(other.seeded.exchange_id), None), identity::Switch::Keep);
        assert_eq!(switch(None, Some(two.id(1))), identity::Switch::To(two.accounts[1]));
        assert_eq!(switch(Some(other.seeded.exchange_id), Some(two.id(1))), identity::Switch::NotFound, "a conflicting pair");
        let answer = mcp(c, &app, user, "update_bot_settings", serde_json::json!({"bot_id": bot, "exchange_id": two.seeded.exchange_id}));
        assert!(mcp_text(&answer).contains("Kraken has 2 accounts"), "{bot}: {answer}");
        assert_eq!(one::<i64>(c, &format!("SELECT exchange_id FROM bots WHERE id = {bot}")), other.seeded.exchange_id, "nothing changed");
        let (status, body) = rest(&app, c, user, "PATCH", &format!("/api/v1/bots/{bot}"), Some(serde_json::json!({"exchange_id": other.seeded.exchange_id, "account_id": two.id(1)}))).await;
        assert_eq!(status, 404, "{bot}: {body}");
        let moved = mcp(c, &app, user, "update_bot_settings", serde_json::json!({"bot_id": bot, "account_id": two.id(1)}));
        assert_eq!(identity::account_of_bot(c, bot).unwrap(), Some(two.id(1)), "{bot}: {moved}");
        assert_eq!(one::<i64>(c, &format!("SELECT exchange_id FROM bots WHERE id = {bot}")), two.seeded.exchange_id);
        let kept = mcp(c, &app, user, "update_bot_settings", serde_json::json!({"bot_id": bot, "exchange_id": two.seeded.exchange_id, "label": format!("kept {bot}")}));
        assert_eq!(identity::account_of_bot(c, bot).unwrap(), Some(two.id(1)), "its own exchange keeps its account: {kept}");
    }
    remove(c, two.id(1));
    assert_eq!(identity::pick_switch(c, user, signal, None, Some(two.id(1))).unwrap(), identity::Switch::NotFound, "a removed account");
}

/// T9: reads per account: account transactions by `account_id`, two `list_exchanges` entries, balances naming the account
/// they read, and an open-order id one account's bot holds never hiding the other account's live order.
#[tokio::test(flavor = "current_thread")]
async fn t9_reads_carry_and_take_the_account() {
    use common::two_accounts::SHARED_TX;
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    two.row(c, 0, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-10 10:00:00");
    two.row(c, 1, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-10 10:00:00");
    two.row(c, 1, 4, "BTC", "2", Some("b-only"), None, "2026-09-11 10:00:00");
    let app = web_app(dir.path(), c);
    let second = mcp(c, &app, user, "list_account_transactions", serde_json::json!({"account_id": two.id(1)}));
    let rows = second["structuredContent"]["transactions"].as_array().cloned().unwrap_or_default();
    let text = second.to_string();
    assert!(text.contains("Account transactions (2)") && text.contains("Kraken 2"), "{second}");
    let both = mcp(c, &app, user, "list_account_transactions", serde_json::json!({"exchange_id": two.seeded.exchange_id}));
    assert!(both.to_string().contains("Account transactions (3)"), "{both}");
    let _ = rows;
    let exchanges = mcp(c, &app, user, "list_exchanges", serde_json::json!({}));
    assert!(mcp_text(&exchanges).contains("Connected Exchanges (2)") && mcp_text(&exchanges).contains("Kraken 2"), "{exchanges}");
    let balances = mcp(c, &app, user, "get_exchange_balances", serde_json::json!({"exchange_name": "Kraken"}));
    assert!(mcp_text(&balances).contains("Kraken has 2 accounts"), "{balances}");
}

/// T9 (r2): account 1's bot holds open order `X`, and account 2's venue lists live order `X` (Alpaca, read live):
/// `list_open_orders` lists both, account 1's from the database and account 2's from its venue.
#[tokio::test(flavor = "current_thread")]
async fn t9_an_order_id_one_account_holds_never_hides_the_others_live_order() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
        {"id": "X", "symbol": "BTCEUR", "asset_class": "crypto", "side": "buy", "type": "limit", "qty": "0.001", "filled_qty": "0", "filled_avg_price": null, "limit_price": "50000", "status": "new"}]))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let kraken = Two::kraken(c, &cipher);
    let alpaca = Two::venue(c, &cipher, &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    c.execute("UPDATE api_keys SET passphrase = ?1 WHERE exchange_id = ?2", rusqlite::params![cipher.encrypt("paper"), alpaca.seeded.exchange_id]).unwrap();
    alpaca.order(c, 0, "X", 0, Some(1));
    let app = web_app(dir.path(), c).with_settings_key_boundary(deltabadger::venue::alpaca::Hosts::all(&server.uri()), std::sync::Arc::new(Quiet)).unwrap();
    let user = kraken.seeded.user_id;
    use deltabadger::web::mcp::{reads, tools::Called};
    let Called::Fetch(fetch) = reads::plan(c, &app, user, "list_open_orders", &serde_json::json!({"exchange_name": "Alpaca"})).unwrap() else { panic!("no fetch") };
    let fetched = reads::fetch(&app, fetch).await;
    let read = reads::finish(c, fetched).unwrap();
    let orders = mcp_text(&read);
    assert!(orders.starts_with("Open orders (2):") && orders.contains("| Alpaca 1 | ext: X") && orders.contains("(Limit order) | Alpaca 2 | ext: X"), "account 1's from the db, account 2's from its venue: {read}");
    // `account_id` alone reads that account's exchange only; one not the user's live account is a 404.
    let Called::Fetch(fetch) = reads::plan(c, &app, user, "list_open_orders", &serde_json::json!({"account_id": alpaca.id(1)})).unwrap() else { panic!("no fetch") };
    let read = reads::finish(c, reads::fetch(&app, fetch).await).unwrap();
    let orders = mcp_text(&read);
    assert!(orders.starts_with("Open orders (1):") && orders.contains("| Alpaca 2 | ext: X") && !orders.contains("Kraken"), "{read}");
    remove(c, kraken.id(1));
    let gone = reads::plan(c, &app, user, "list_open_orders", &serde_json::json!({"account_id": kraken.id(1)})).unwrap();
    let Called::Done(gone) = gone else { panic!("a removed account is not read") };
    assert!(gone.to_string().contains("Account not found."), "{gone}");
    c.execute("UPDATE exchange_accounts SET removed_at = NULL WHERE id = ?1", [kraken.id(1)]).unwrap();
    // The unread exchanges' JSON keeps the bare name, the account beside it (§4.4); the text names the account.
    let Called::Fetch(fetch) = reads::plan(c, &app, user, "list_open_orders", &serde_json::json!({})).unwrap() else { panic!("no fetch") };
    let full = reads::finish_read(c, reads::fetch(&app, fetch).await).unwrap();
    let read = serde_json::json!({"data": full.res.data, "text": full.text});
    let unavailable = read["data"]["unavailable"].as_array().cloned().unwrap_or_default();
    let kraken_entries: Vec<_> = unavailable.iter().filter(|u| u["exchange"] == "Kraken").collect();
    assert_eq!(kraken_entries.len(), 2, "{read}");
    assert!(kraken_entries.iter().any(|u| u["account"] == 2 && u["account_id"] == kraken.id(1)), "{read}");
    assert!(!unavailable.iter().any(|u| u["exchange"].as_str().is_some_and(|e| e.ends_with(" 1") || e.ends_with(" 2"))), "{read}");
    assert!(read["text"].as_str().unwrap_or_default().contains("! Kraken 2:"), "{read}");
}

/// T3 (refusals): the REST/MCP order and bot-create refusals, the multi-asset create validation and the tax report's sync
/// banner name account 2 as `Kraken 2`.
#[tokio::test(flavor = "current_thread")]
async fn t3_refusals_name_the_account() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let app = web_app(dir.path(), c);
    let order = |base: &str| serde_json::json!({"exchange_name": "Kraken", "base_asset": base, "quote_asset": "EUR", "amount": 0.001, "account_id": two.id(1)});
    let unknown = mcp(c, &app, user, "market_buy", order("ETH"));
    assert!(mcp_text(&unknown).contains("Trading pair ETH/EUR not found on Kraken 2."), "{unknown}");
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two.keys[1]]).unwrap();
    let keyless = mcp(c, &app, user, "market_buy", order("BTC"));
    assert!(mcp_text(&keyless).contains("No valid API key found for Kraken 2."), "{keyless}");
    let (status, body) = rest(&app, c, user, "POST", "/api/v1/bots", Some(serde_json::json!({"type": "dca", "exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR",
        "quote_amount": 10, "interval": "day", "account_id": two.id(1)}))).await;
    assert_eq!(status, 403, "{body}");
    assert!(body["error"]["message"].as_str().unwrap_or_default().contains("No valid API key found for Kraken 2."), "{body}");
    c.execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [two.keys[1]]).unwrap();

    // validate_composition_pairs: an asset Kraken does not list against EUR.
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('ethereum', 'ETH', 'Ethereum', 'Cryptocurrency', 'now', 'now')", []).unwrap();
    let eth = c.last_insert_rowid();
    let config = serde_json::json!({"exchange_id": two.seeded.exchange_id, "settings": {"quote_asset_id": two.seeded.quote, "allocations": {eth.to_string(): 1.0}}});
    let built = deltabadger::web::wizard::create::build(c, &config, "en").unwrap();
    let errors = deltabadger::web::wizard::create::validate(c, &built, Some(&two.accounts[1]), "en").unwrap();
    assert!(errors.iter().any(|e| e.contains("on Kraken 2.")), "{errors:?}");

    two.row(c, 1, 4, "BTC", "1", Some("t-b"), None, "2026-03-10 10:00:00");
    c.execute("UPDATE api_keys SET last_sync_error = 'Kraken said no' WHERE id = ?1", [two.keys[1]]).unwrap();
    let report = deltabadger::tax::crypto_report(c, user, "DE", 2026, false, chrono::NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()).unwrap();
    let deltabadger::tax::Outcome::Csv(csv) = report else { panic!("refused") };
    let banners: Vec<&str> = csv.lines().filter(|l| l.contains("Kraken")).collect();
    assert!(banners.iter().any(|l| l.contains("Kraken 1 ")) && banners.iter().any(|l| l.contains("Kraken 2 ")), "never synced 1, failed 2: {banners:?}");
}

/// T9 (balances): a balance read naming account 2 reads with account 2's key, over MCP and REST; account 1's key is
/// another (the venue answers only account 2's).
#[tokio::test(flavor = "current_thread")]
async fn t9_a_balance_read_naming_account_2_uses_its_key() {
    use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID", "account-2-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ACTIVE", "cash": "0"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).and(header("APCA-API-KEY-ID", "account-2-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let kraken = Two::kraken(c, &cipher);
    let alpaca = Two::venue(c, &cipher, &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    c.execute("UPDATE api_keys SET passphrase = ?1 WHERE exchange_id = ?2", rusqlite::params![cipher.encrypt("paper"), alpaca.seeded.exchange_id]).unwrap();
    let app = web_app(dir.path(), c).with_settings_key_boundary(deltabadger::venue::alpaca::Hosts::all(&server.uri()), std::sync::Arc::new(Quiet)).unwrap();
    let user = kraken.seeded.user_id;
    use deltabadger::web::mcp::{reads, tools::Called};
    for (n, ok) in [(1, true), (0, false)] {
        let Called::Fetch(fetch) = reads::plan(c, &app, user, "get_exchange_balances", &serde_json::json!({"exchange_name": "Alpaca", "account_id": alpaca.id(n)})).unwrap() else { panic!("no fetch") };
        let read = reads::finish(c, reads::fetch(&app, fetch).await).unwrap();
        assert_eq!(read["isError"] != true, ok, "account {}: {read}", n + 1);
        let (status, body) = rest(&app, c, user, "GET", &format!("/api/v1/exchanges/{}/balances?account_id={}", alpaca.seeded.exchange_id, alpaca.id(n)), None).await;
        assert_eq!(status == 200, ok, "REST account {}: {status} {body}", n + 1);
    }
}

/// T9 (transactions): with two live accounts each bot order of `GET /transactions` and `list_transactions` carries its
/// account; with one, the answer has no account fields (D-API-ACCOUNT-FIELDS).
#[tokio::test(flavor = "current_thread")]
async fn t9_transactions_carry_the_account_only_with_two() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let first = two.order(c, 0, "o-1", 0, Some(1));
    let second = two.order(c, 1, "o-2", 0, Some(1));
    let app = web_app(dir.path(), c);
    let (status, body) = rest(&app, c, user, "GET", "/api/v1/transactions", None).await;
    assert_eq!(status, 200, "{body}");
    let rows = body["data"]["transactions"].as_array().cloned().unwrap_or_default();
    let of = |id: i64| rows.iter().find(|r| r["id"] == id).cloned().unwrap_or_default();
    assert_eq!((of(first)["account_id"].as_i64(), of(first)["account"].as_i64()), (Some(two.id(0)), Some(1)), "{body}");
    assert_eq!((of(second)["account_id"].as_i64(), of(second)["account"].as_i64()), (Some(two.id(1)), Some(2)), "{body}");
    let service = deltabadger::web::bot_api::transactions(c, user, &serde_json::Value::Null, &serde_json::Value::Null).unwrap().unwrap();
    assert_eq!(service.data, body["data"], "MCP's list_transactions reads the same service");

    let (dir, o) = install();
    let c = &o.primary;
    let seeded = seed::seed_kraken(c, &seed::cipher());
    identity::Account::first(c, seeded.user_id, seeded.exchange_id).unwrap().ensure(c, chrono::Utc::now()).unwrap();
    let bot = seed::insert_bot(c, &seeded, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    seed::insert_tx(c, &seeded, bot, &seed::TxSpec { status: 0, external_status: Some(1), external_id: Some("o-1".into()), order_type: 0, amount: Some("0.001"),
        quote_amount: Some("60"), price: Some("60000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-02 10:00:00".into() });
    let app = web_app(dir.path(), c);
    let (_, body) = rest(&app, c, seeded.user_id, "GET", "/api/v1/transactions", None).await;
    let rows = body["data"]["transactions"].as_array().cloned().unwrap_or_default();
    assert!(rows.len() == 1 && rows.iter().all(|r| r.get("account_id").is_none() && r.get("account").is_none()), "{body}");
}

/// Codex 6a r2: an open-order read naming only an account of an exchange this build does not serve is not found, never a
/// read of every account.
#[test]
fn an_open_order_read_naming_a_bitmart_account_is_not_found() {
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let kraken = Two::kraken(c, &cipher);
    let bitmart = Two::venue(c, &cipher, &kraken.seeded, "Exchanges::Bitmart", "Bitmart");
    let app = web_app(dir.path(), c);
    let read = mcp(c, &app, kraken.seeded.user_id, "list_open_orders", serde_json::json!({"account_id": bitmart.id(1)}));
    assert!(mcp_text(&read).contains("Account not found."), "{read}");
}

/// Codex 6a r2: `account_id: null` on a signal bot's settings update is no account, as on the DCA path.
#[tokio::test(flavor = "current_thread")]
async fn a_null_account_id_on_a_signal_update_is_no_account() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let app = web_app(dir.path(), c);
    let (status, body) = rest(&app, c, user, "POST", "/api/v1/bots", Some(serde_json::json!({"type": "signal", "exchange_name": "Kraken", "base_asset": "BTC",
        "quote_asset": "EUR", "account_id": two.id(1)}))).await;
    assert!(status < 300, "{body}");
    let bot = body["data"]["id"].as_i64().unwrap();
    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).unwrap();
    let answer = mcp(c, &app, user, "update_bot_settings", serde_json::json!({"bot_id": bot, "label": "renamed", "account_id": null}));
    assert!(answer["isError"] != true, "{answer}");
    assert_eq!(one::<String>(c, &format!("SELECT label FROM bots WHERE id = {bot}")), "renamed");
    assert_eq!(deltabadger::identity::account_of_bot(c, bot).unwrap(), Some(two.id(1)));
}

/// Codex 6a r3: `account_id` posted strictly (`"12oops"` names no account), and an Idempotency-Key reused with another
/// account in the query is another request, never a replay of the first account's answer.
#[tokio::test(flavor = "current_thread")]
async fn an_account_id_is_whole_and_part_of_the_requests_fingerprint() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let app = web_app(dir.path(), c);
    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [two.bots[0]]).unwrap();
    let moved = mcp(c, &app, user, "update_bot_settings", serde_json::json!({"bot_id": two.bots[0], "account_id": format!("{}oops", two.id(1))}));
    assert!(moved["isError"] == true, "{moved}");
    assert_eq!(deltabadger::identity::account_of_bot(c, two.bots[0]).unwrap(), Some(two.id(0)));
    KEY.with(|k| *k.borrow_mut() = Some("one-key".into()));
    let body = serde_json::json!({"type": "market_buy", "exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR", "amount": 0.001});
    let first = rest(&app, c, user, "POST", &format!("/api/v1/orders?account_id={}", two.id(0)), Some(body.clone())).await;
    let second = rest(&app, c, user, "POST", &format!("/api/v1/orders?account_id={}", two.id(1)), Some(body)).await;
    KEY.with(|k| *k.borrow_mut() = None);
    assert_eq!((second.0, second.1["error"]["code"].as_str()), (409, Some("idempotency_key_reused")), "first {first:?}, second {second:?}");
}

/// Codex 6a r4: a posted `account_id` that names nothing (`false`) is not found, never the default account; a balance
/// read on an account with no correct key names that account; a bot moving accounts with open orders is told to cancel
/// them on the account holding them.
#[tokio::test(flavor = "current_thread")]
async fn a_posted_account_must_name_one_and_refusals_name_the_right_account() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let app = web_app(dir.path(), c);
    // MCP's schema refuses it already; REST has no schema.
    for wrong in [serde_json::json!(false), serde_json::json!([]), serde_json::json!("")] {
        let (status, body) = rest(&app, c, user, "POST", "/api/v1/orders", Some(serde_json::json!({"type": "market_buy", "exchange_name": "Kraken", "base_asset": "BTC",
            "quote_asset": "EUR", "amount": 0.001, "account_id": wrong}))).await;
        assert_eq!((status, body["error"]["code"].as_str()), (404, Some("account_not_found")), "{wrong}: {body}");
        // Codex 6a r5: every entry point holds the same rule.
        let (status, body) = rest(&app, c, user, "POST", "/api/v1/bots", Some(serde_json::json!({"type": "dca", "exchange_name": "Kraken", "base_asset": "BTC", "quote_asset": "EUR",
            "quote_amount": 10, "interval": "day", "account_id": wrong}))).await;
        assert_eq!(status, 404, "bot create {wrong}: {body}");
        let (status, body) = rest(&app, c, user, "POST", "/api/v1/rules", Some(serde_json::json!({"exchange_name": "Kraken", "asset": "BTC", "address": "bc1-x", "account_id": wrong}))).await;
        assert_eq!(status, 404, "rule create {wrong}: {body}");
    }
    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [two.bots[1]]).unwrap();
    let bots_before = counts(c);
    let (status, body) = rest(&app, c, user, "PATCH", &format!("/api/v1/bots/{}", two.bots[1]), Some(serde_json::json!({"label": "x", "account_id": []}))).await;
    assert_eq!(status, 404, "bot update []: {body}");
    assert_eq!(counts(c), bots_before);
    for path in ["/api/v1/orders?account_id=", "/api/v1/transactions/account?account_id="] {
        let (status, body) = rest(&app, c, user, "GET", path, None).await;
        assert_eq!((status, body["error"]["code"].as_str()), (404, Some("account_not_found")), "{path}: {body}");
    }
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two.keys[1]]).unwrap();
    let balances = mcp(c, &app, user, "get_exchange_balances", serde_json::json!({"exchange_name": "Kraken", "account_id": two.id(1)}));
    assert!(mcp_text(&balances).contains("No valid API key found for Kraken 2."), "{balances}");
    c.execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [two.keys[1]]).unwrap();
    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [two.bots[0]]).unwrap();
    two.order(c, 0, "open-0", 0, Some(1));
    let moved = mcp(c, &app, user, "update_bot_settings", serde_json::json!({"bot_id": two.bots[0], "account_id": two.id(1)}));
    assert!(mcp_text(&moved).contains("on Kraken 1 before"), "{moved}");
    assert_eq!(deltabadger::identity::account_of_bot(c, two.bots[0]).unwrap(), Some(two.id(0)));
}

/// Codex 6a r4: an Alpaca open-order read naming the exchange, neither of whose two accounts has a correct key, names both
/// accounts as unchecked.
#[tokio::test(flavor = "current_thread")]
async fn an_alpaca_read_with_no_correct_key_names_both_accounts() {
    let (dir, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let kraken = Two::kraken(c, &cipher);
    let alpaca = Two::venue(c, &cipher, &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    c.execute("UPDATE api_keys SET status = 2 WHERE exchange_id = ?1", [alpaca.seeded.exchange_id]).unwrap();
    let app = web_app(dir.path(), c);
    use deltabadger::web::mcp::{reads, tools::Called};
    let Called::Fetch(fetch) = reads::plan(c, &app, kraken.seeded.user_id, "list_open_orders", &serde_json::json!({"exchange_name": "Alpaca"})).unwrap() else { panic!("no fetch") };
    let read = reads::finish_read(c, reads::fetch(&app, fetch).await).unwrap();
    let unchecked: Vec<_> = read.res.data["unavailable"].as_array().cloned().unwrap_or_default();
    assert_eq!(unchecked.len(), 2, "{:?} {}", unchecked, read.text);
    assert!(read.text.contains("! Alpaca 1:") && read.text.contains("! Alpaca 2:"), "{}", read.text);
}

// ---- commit 4: duplicate standing (§5, T11-T14) ----

fn observe(c: &Connection, two: &Two, n: usize) -> bool {
    identity::observe_overlap(c, &identity::AccountRow::of(c, &two.accounts[n]).unwrap().unwrap(), chrono::Utc::now()).unwrap()
}
/// `n` buys of account `k` under the shared trade ids `trade-0…`: the first `base_amount` given, the rest 1, one a day.
fn trades(c: &Connection, two: &Two, k: usize, n: usize, entry_type: i64, first_amount: &str) {
    for i in 0..n { two.trade(c, k, entry_type, &format!("trade-{i}"), if i == 0 { first_amount } else { "1" }, &format!("2026-09-0{} 10:00:00", i + 1)); }
}

/// T11: equal fingerprints make 2 a duplicate of 1; different or NULL ones none; three accounts on one fingerprint make 2
/// and 3 duplicates of 1; a removed 1 makes 3 a duplicate of 2.
#[test]
fn t11_equal_fingerprints_make_the_higher_number_a_duplicate() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let (a, b) = (two.id(0), two.id(1));
    assert!(identity::duplicates(c, user).unwrap().is_empty(), "NULL fingerprints");
    two.fingerprint(c, 0, Some("fp-1"));
    assert!(identity::duplicates(c, user).unwrap().is_empty(), "one NULL");
    two.fingerprint(c, 1, Some("fp-2"));
    assert!(identity::duplicates(c, user).unwrap().is_empty(), "different");
    two.fingerprint(c, 1, Some("fp-1"));
    assert_eq!(identity::duplicates(c, user).unwrap(), [(b, a)]);
    let third = two.third(c).id.unwrap();
    c.execute("UPDATE exchange_accounts SET fingerprint = 'fp-1' WHERE id = ?1", [third]).unwrap();
    assert_eq!(identity::duplicates(c, user).unwrap(), [(b, a), (third, a)]);
    remove(c, a);
    assert_eq!(identity::duplicates(c, user).unwrap(), [(third, b)], "a removed 1");
}

/// T12: overlap evidence on Kraken: 2 strong matches write nothing, 3 write the observation; a conflicting amount, side or
/// asset vetoes it; deposits and withdrawal/deposit pairs never count; conflicting fingerprints veto; the fixture's shared
/// ids write nothing.
#[test]
fn t12_shared_trade_ids_with_the_same_facts_are_the_evidence() {
    let case = |setup: &dyn Fn(&Connection, &Two)| -> bool {
        let (_d, o) = install();
        let c = &o.primary;
        let two = Two::kraken(c, &seed::cipher());
        setup(c, &two);
        let observed = observe(c, &two, 1);
        assert_eq!(observed, identity::duplicates(c, two.seeded.user_id).unwrap() == [(two.id(1), two.id(0))], "an observation is the standing");
        observed
    };
    assert!(!case(&|c, two| { trades(c, two, 0, 2, 0, "1"); trades(c, two, 1, 2, 0, "1"); }), "two matches");
    assert!(case(&|c, two| { trades(c, two, 0, 3, 0, "1"); trades(c, two, 1, 3, 0, "1"); }), "three matches");
    assert!(!case(&|c, two| { trades(c, two, 0, 3, 0, "1"); trades(c, two, 1, 3, 0, "2"); }), "a different base_amount vetoes");
    assert!(!case(&|c, two| { trades(c, two, 0, 4, 0, "1"); trades(c, two, 1, 4, 1, "1"); }), "the other side");
    assert!(!case(&|c, two| {
        trades(c, two, 0, 4, 0, "1"); trades(c, two, 1, 4, 0, "1");
        c.execute("UPDATE account_transactions SET base_currency = 'ETH' WHERE id = (SELECT max(id) FROM account_transactions)", []).unwrap();
    }), "another asset on one shared id");
    assert!(!case(&|c, two| { trades(c, two, 0, 5, 4, "1"); trades(c, two, 1, 5, 4, "1"); }), "deposits never count");
    assert!(!case(&|c, two| { trades(c, two, 0, 5, 5, "1"); trades(c, two, 1, 5, 4, "1"); }), "withdrawal/deposit pairs");
    assert!(!case(&|c, two| { two.fingerprint(c, 0, Some("a")); two.fingerprint(c, 1, Some("b")); trades(c, two, 0, 5, 0, "1"); trades(c, two, 1, 5, 0, "1"); }),
            "conflicting fingerprints");
    assert!(!case(&|c, two| {
        use common::two_accounts::{SHARED_ORDER, SHARED_TX};
        for k in 0..2 { two.row(c, k, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00"); two.order(c, k, SHARED_ORDER, 0, Some(2)); }
    }), "the fixture's shared ids");
}

/// T12: KuCoin is not a listed venue; an observed account is not sampled again; an install without the table reads no
/// duplicates.
#[test]
fn t12_unlisted_venues_reruns_and_old_installs_observe_nothing() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    trades(c, &kucoin, 0, 5, 0, "1");
    trades(c, &kucoin, 1, 5, 0, "1");
    assert!(!observe(c, &kucoin, 1), "KuCoin");
    trades(c, &kraken, 0, 3, 0, "1");
    trades(c, &kraken, 1, 3, 0, "1");
    assert!(observe(c, &kraken, 1));
    assert!(!observe(c, &kraken, 1), "observed once");
    assert!(!observe(c, &kraken, 0), "a number 1 is never a duplicate");
    assert_eq!(identity::duplicates(c, kraken.seeded.user_id).unwrap(), [(kraken.id(1), kraken.id(0))]);
    kraken.fingerprint(c, 0, Some("fp"));
    kraken.fingerprint(c, 1, Some("fp"));
    c.execute_batch("DROP TABLE exchange_account_overlaps").unwrap();
    assert!(identity::duplicates(c, kraken.seeded.user_id).unwrap().is_empty(), "no table, no duplicates");
}

/// T13: 3 observed against 2 while 2 is 1's duplicate by fingerprint: both chains end at 1.
#[test]
fn t13_chains_resolve_to_the_lowest_live_end() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let third = two.third(c).id.unwrap();
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    c.execute("INSERT INTO exchange_account_overlaps (account_id, other_account_id, matches, observed_at) VALUES (?1, ?2, 3, 'now')", [third, two.id(1)]).unwrap();
    assert_eq!(identity::duplicates(c, two.seeded.user_id).unwrap(), [(two.id(1), two.id(0)), (third, two.id(0))]);
}

/// A balance row of account `n` of `two`: `units` of BTC worth `value`.
fn hold(c: &Connection, two: &Two, n: usize, units: &str, value: &str) {
    c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at, account_id) \
               VALUES (?1, ?2, ?3, ?4, 0, 100, ?5, '2026-09-20 10:00:00', '2026-09-20 10:00:00', 'now', 'now', ?6)",
              rusqlite::params![two.seeded.user_id, two.seeded.exchange_id, two.seeded.btc, units, value, two.id(n)]).unwrap();
}

/// T14: with 2 a duplicate, "All"'s balances, walk input and venue snapshots are account 1's; account 2's own scope reads
/// nothing; its bot keeps its status. The history version gains the standing only while a duplicate exists, so the
/// ledger cache and the sweep see the change.
#[test]
fn t14_a_duplicate_is_left_out_of_the_figures() {
    use deltabadger::tracker::{backfill, figures::{self, page}, rows, snapshot, Scope};
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let [a, b] = [0, 1].map(|n| identity::AccountRow::of(c, &two.accounts[n]).unwrap().unwrap());
    two.row(c, 0, 4, "BTC", "1", Some("dep-a"), None, "2026-09-10 10:00:00");
    two.row(c, 1, 4, "BTC", "1", Some("dep-b"), None, "2026-09-10 10:00:00");
    hold(c, &two, 0, "1", "100");
    hold(c, &two, 1, "1", "100");
    let value = |scope: Scope| page::cold(c, user, &scope, true).unwrap().holdings.iter().map(|h| h.value.as_ref().unwrap().to_s_f()).collect::<Vec<_>>();
    let balances = |scope: Scope| figures::balances(c, user, &scope).unwrap().len();
    let version = backfill::history_version(c, user).unwrap();
    let cache = deltabadger::tracker::cache::version(c, user).unwrap();
    assert!(!version.contains("_d"), "no duplicate: the version is 2.x's");
    assert_eq!((value(Scope::All), balances(Scope::All), rows::load_all(c, user).unwrap().len()), (vec!["200.0".to_string()], 2, 2));

    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(value(Scope::All), value(Scope::Account(a)), "All's balances are account 1's");
    assert_eq!(balances(Scope::All), 1);
    assert!(value(Scope::Account(b)).is_empty() && balances(Scope::Account(b)) == 0, "the duplicate's own scope reads nothing");
    let walked = rows::load_all(c, user).unwrap();
    assert_eq!(walked.iter().map(|r| r.tx_id.clone()).collect::<Vec<_>>(), [Some("dep-a".to_string())], "the walk reads account 1's rows only");
    assert_eq!(snapshot::venues(c, user).unwrap(), [a], "no venue snapshot of the duplicate");
    let changed = backfill::history_version(c, user).unwrap();
    // The duplicate's row no longer counts, and the standing is appended.
    assert!(changed.ends_with(&format!("_d{}:{}", b.id, a.id)) && changed != version, "{changed}");
    assert!(deltabadger::tracker::cache::version(c, user).unwrap() != cache, "the ledger cache re-walks");
    let status: i64 = c.query_row("SELECT status FROM bots WHERE id = ?1", [two.bots[1]], |r| r.get(0)).unwrap();
    assert_eq!(status, 1, "its bot keeps working: nothing in the engine reads the standing");
}

/// T14: "All" says a duplicate is left out; the duplicate's own scope shows the notice in place of its figures.
#[tokio::test(flavor = "current_thread")]
async fn t14_the_page_says_a_duplicate_is_left_out() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    two.row(c, 0, 4, "BTC", "1", Some("t-a"), None, "2026-09-10 10:00:00");
    two.row(c, 1, 4, "BTC", "1", Some("t-b"), None, "2026-09-10 10:00:00");
    let app = web_app(dir.path(), c);
    let all = get(&app, c, user, "/tracker").await;
    assert_eq!(all.status, 200);
    assert!(!all.body.contains("left out of your totals"), "no duplicate, no notice");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let all = get(&app, c, user, "/tracker").await;
    assert!(all.body.contains("Kraken<sup>2</sup> is the same exchange account as Kraken<sup>1</sup>, so it is left out of your totals."), "{}", cut(&all.body, "notice", 600));
    assert!(all.body.contains("data-grid--six"), "All keeps its figures");
    let own = get(&app, c, user, &format!("/tracker?account_id={}", two.id(1))).await;
    assert_eq!(own.status, 200);
    assert!(own.body.contains("left out of your totals") && own.body.contains("/settings/api_keys"), "{}", cut(&own.body, "notice", 600));
    assert!(!own.body.contains("data-grid--six"), "the notice replaces the duplicate's figures");
}

struct NoVenue;
impl deltabadger::sync::jobs::Connect for NoVenue {
    type T = deltabadger::venue::http::ScriptedTransport;
    fn connect(&self, _: &deltabadger::crypto::Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ScriptedTransport::from_script(&serde_json::json!({})), deltabadger::venue::alpaca::Urls::for_passphrase(None))
    }
}

/// T14: the stored history follows the standing. D's USD deposits start in January and S's in June: after D becomes a
/// duplicate the sweep leaves no whole-portfolio day before June. With S holding no row, the no-sweep branch leaves no
/// stored day up to yesterday. A sweep under an unchanged standing deletes nothing earlier.
#[tokio::test(flavor = "current_thread")]
async fn t14_the_stored_history_follows_the_standing() {
    for s_rows in [true, false] {
        let (dir, o) = install();
        let two = Two::kraken(&o.primary, &seed::cipher());
        let user = two.seeded.user_id;
        two.row(&o.primary, 1, 4, "USD", "100", Some("dep-d"), None, "2026-01-10 10:00:00");
        if s_rows { two.row(&o.primary, 0, 4, "USD", "50", Some("dep-s"), None, "2026-06-10 10:00:00"); }
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        let db = deltabadger::jobs::Db::new(o.primary, seed::cipher());
        let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
        let clock = deltabadger::engine::FixedClock(at);
        let sweep = || deltabadger::tracker::jobs::backfill_run(&db, &NoVenue, None::<&deltabadger::jobs::market::Feed<deltabadger::venue::http::ScriptedTransport>>, None, user, &clock);
        let first = |c: &Connection, table: &str| -> Option<String> { c.query_row(&format!("SELECT min(date) FROM {table} WHERE user_id = ?1 AND date <= '2026-09-19'"), [user], |r| r.get(0)).unwrap() };
        assert_eq!(sweep().await, Ok(true));
        assert_eq!(first(&c, "portfolio_snapshots").as_deref(), Some("2026-01-10"));
        assert_eq!(sweep().await, Ok(true), "an unchanged standing");
        assert_eq!(first(&c, "portfolio_snapshots").as_deref(), Some("2026-01-10"), "nothing earlier deleted");
        c.execute_batch("UPDATE exchange_accounts SET fingerprint = 'fp'").unwrap();
        let cipher = seed::cipher();
        assert!(deltabadger::tracker::backfill::wanted(&c, &cipher, user, at.date_naive()).unwrap(), "the standing asks for a sweep");
        if s_rows {
            assert_eq!(sweep().await, Ok(true));
            assert_eq!(first(&c, "portfolio_snapshots").as_deref(), Some("2026-06-10"), "no day before S's first row");
        } else {
            assert_eq!(sweep().await, Ok(false));
            assert_eq!((first(&c, "portfolio_snapshots"), first(&c, "exchange_account_snapshots")), (None, None), "no stored day left");
        }
        assert!(!deltabadger::tracker::backfill::wanted(&c, &cipher, user, at.date_naive()).unwrap(), "stamped");
    }
}

// ---- commit 5: the ownership fence (§6.4, T16) ----

/// Whether a result captured under `version` may commit now (`check_credential_result`, the commit every sync and the
/// engine pass).
fn commits(c: &Connection, version: &deltabadger::engine::model::CredentialVersion) -> bool {
    let tx = c.unchecked_transaction().unwrap();
    let out = match deltabadger::engine::model::check_credential_result(&tx, &Some(version.clone())) {
        Ok(_) => true,
        Err(deltabadger::engine::EngineError::CredentialsChanged) => false,
        Err(e) => panic!("{e:?}"),
    };
    drop(tx);
    out
}
/// A stand-in of Merge's key move: account 2's key `key` joins number 1 (its membership goes).
fn move_key_to_one(c: &Connection, key: i64) { c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [key]).unwrap(); }

/// T16: every capture path fills the account and every one is discarded at its commit once its key moved to another
/// account, or its account was removed: a sync's read by id, the engine's bot key version, a placement's credentials, a
/// withdrawal's and a settings save's prepared version. A decoded stamp (no account) compares as before.
#[test]
fn t16_a_result_whose_key_changed_account_is_discarded() {
    use deltabadger::engine::model;
    let cipher = seed::cipher();
    type Capture = fn(&Connection, &deltabadger::crypto::Cipher, &Two) -> model::CredentialVersion;
    let paths: [(&str, Capture); 5] = [
        ("credential_version_by_id", |c, _, two| model::credential_version_by_id(c, two.keys[1]).unwrap().unwrap()),
        ("credential_version", |c, _, two| model::credential_version(c, &model::load_bot(c, two.bots[1]).unwrap()).unwrap().unwrap()),
        ("credentials_with_redaction", |c, cipher, two| model::credentials_with_redaction(c, cipher, &model::load_bot(c, two.bots[1]).unwrap()).unwrap().1.unwrap()),
        ("withdrawal_credentials", |c, cipher, two| {
            c.execute("UPDATE api_keys SET key_type = 1 WHERE id = ?1", [two.keys[1]]).unwrap();
            model::withdrawal_credentials(c, cipher, &two.accounts[1]).unwrap().unwrap().1
        }),
        ("prepared_credential_version", |_, _, two| model::prepared_credential_version(two.keys[1], &Default::default()).with_account(two.accounts[1].id)),
    ];
    for (name, capture) in paths {
        for removal in [false, true] {
            let (_d, o) = install();
            let c = &o.primary;
            let two = Two::kraken(c, &cipher);
            let version = capture(c, &cipher, &two);
            assert_eq!(version.account(), Some(two.id(1)), "{name} fills the account");
            assert!(commits(c, &version) || name == "prepared_credential_version" && model::account_is_current(c, &version).unwrap(), "{name}: current");
            if removal { remove(c, two.id(1)); } else { move_key_to_one(c, two.keys[1]); }
            assert!(!model::account_is_current(c, &version).unwrap(), "{name}, removed {removal}: discarded");
            if name != "prepared_credential_version" { assert!(!commits(c, &version), "{name}, removed {removal}: the commit refuses"); }
        }
    }
    // A decoded stamp has no account: it compares as on main, whatever its key's account.
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &cipher);
    let b = identity::AccountRow::of(c, &two.accounts[1]).unwrap().unwrap();
    let stamps = deltabadger::sync::cache::capture_read(c, two.seeded.user_id, &deltabadger::tracker::Scope::Account(b)).unwrap().producer_stamps();
    move_key_to_one(c, two.keys[1]);
    let wait = serde_json::json!({"origin": "venue", "producer": stamps[0]});
    assert!(model::wait_is_current(c, &wait).unwrap(), "a stamp's version is the key's ciphertext alone, as on main");
}

/// T16 (r4): a settings save validated on account 2's empty key slot, committed after Merge tombstoned 2, is refused by
/// `Account::ensure`: nothing is inserted and 2 holds no key.
#[test]
fn t16_nothing_is_written_to_a_removed_account() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    remove(c, two.id(1));
    let err = two.accounts[1].ensure(c, chrono::Utc::now()).unwrap_err();
    assert!(err.to_string().contains(identity::REMOVED), "{err}");
    assert!(two.accounts[0].ensure(c, chrono::Utc::now()).is_ok(), "a live account is unchanged");
    // The save's own fence: a pending slot's version (no key yet) names only its account, which is removed.
    let pending = deltabadger::engine::model::prepared_credential_version(0, &Default::default()).with_account(two.accounts[1].id);
    assert!(!deltabadger::engine::model::account_is_current(c, &pending).unwrap());
}

/// T16: the constructor lint: `ciphertext_version(` is called by the five captures only (model.rs's own tests aside).
#[test]
fn t16_every_credential_capture_is_one_of_the_five() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut calls = vec![];
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { stack.push(path); continue; }
            if path.extension().is_none_or(|e| e != "rs") { continue; }
            let text = std::fs::read_to_string(&path).unwrap();
            let live = text.split("#[cfg(test)]").next().unwrap();
            for (n, line) in live.lines().enumerate() {
                if line.contains("ciphertext_version(") && !line.contains("fn ciphertext_version(") { calls.push(format!("{}:{}", path.strip_prefix(&root).unwrap().display(), n + 1)); }
            }
        }
    }
    assert_eq!(calls.len(), 5, "a new credential capture must fill its account (PLAN-710-6 §6.4): {calls:?}");
}

// ---- commit 6: Merge (§6, T15, T17-T27) ----

use deltabadger::merge::{self, Refusal};

/// Every table's rows, as text: what a refused Merge must leave as it was.
fn dump(c: &Connection) -> Vec<String> {
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let mut out = vec![];
    for t in tables {
        let mut s = c.prepare(&format!("SELECT * FROM \"{t}\"")).unwrap();
        let n = s.column_count();
        let rows: Vec<String> = s.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join("|"))).unwrap().map(Result::unwrap).collect();
        out.extend(rows.into_iter().map(|r| format!("{t}: {r}")));
    }
    out
}
/// Kraken with two accounts, 2 a duplicate of 1 by fingerprint.
fn duplicate() -> (tempfile::TempDir, store::Opened, Two) {
    let (d, o) = install();
    let two = Two::kraken(&o.primary, &seed::cipher());
    two.fingerprint(&o.primary, 0, Some("fp"));
    two.fingerprint(&o.primary, 1, Some("fp"));
    (d, o, two)
}
/// `merge::run` in one transaction, committed only when it merged.
fn merge_now(c: &Connection, two: &Two, d: i64) -> Result<Option<merge::Merged>, Refusal> {
    let tx = c.unchecked_transaction().unwrap();
    let out = merge::run(&tx, &seed::cipher(), two.seeded.user_id, d, chrono::Utc::now());
    if matches!(out, Ok(Some(_))) { tx.commit().unwrap(); }
    out
}
fn refused_unchanged(c: &Connection, two: &Two, what: &str) {
    let before = dump(c);
    match merge_now(c, two, two.id(1)) { Err(Refusal::InFlight) => {}, other => panic!("{what}: {other:?}") }
    assert_eq!(dump(c), before, "{what}: nothing changed");
}

/// T15: in flight, a Merge refuses and writes nothing: a placement intent on 2's bot, an open polled order on 1's bot, a
/// withdrawal intent of 2's rule, a queued, taken or sending standalone order, a placed one the pre-flight has not seen
/// end (the fence). A filled one does not block, and keeps its log, on S.
#[test]
fn t15_anything_in_flight_refuses_the_merge() {
    let cipher = seed::cipher();
    type Setup = fn(&Connection, &Two, &deltabadger::crypto::Cipher);
    let setups: [(&str, Setup); 7] = [
        ("placement intent on 2", |c, two, _| { c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json('{\"x\":1}')) WHERE id = ?1", [two.bots[1]]).unwrap(); }),
        ("open polled order on 1", |c, two, _| { two.order(c, 0, "open-1", 0, Some(1)); }),
        ("withdrawal intent of 2's rule", |c, two, cipher| {
            let rule = two.rule(c, cipher, 1, two.seeded.btc, 0);
            c.execute("INSERT INTO rust_withdrawal_intents (owner, state, client_id, created_at, api_key_id, credential_digest, host, account_scope, key_identity, params_enc, fee_basis, budget, free_balance) \
                       VALUES (?1, 'prepared', 'c', 'now', ?2, 'd', 'h', 's', 'k', 'p', 'f', 'b', 'f')", rusqlite::params![format!("rule:{rule}"), two.keys[1]]).unwrap();
        }),
        ("queued standalone", |c, two, _| { two.standalone(c, 1, "queued", None, None); }),
        ("taken standalone", |c, two, _| { two.standalone(c, 0, "taken", None, None); }),
        ("sending standalone", |c, two, _| { two.standalone(c, 1, "sending", None, None); }),
        ("placed, not seen ended (the fence)", |c, two, _| { two.standalone(c, 1, "done", Some(r#"{"kind":"placed","order_id":"L-1"}"#), Some("L-1")); }),
    ];
    for (what, setup) in setups {
        let (_d, o, two) = duplicate();
        setup(&o.primary, &two, &cipher);
        refused_unchanged(&o.primary, &two, what);
    }
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let filled = two.standalone(c, 1, "done", Some(r#"{"kind":"placed","order_id":"F-1"}"#), Some("F-1"));
    assert_eq!(merge::record_terminal(c, filled, "filled").unwrap(), 1);
    // An uncertain outcome the API already read (and forgot) is no barrier either (r3).
    let ambiguous = two.standalone(c, 1, "done", Some(r#"{"kind":"ambiguous","errors":[]}"#), None);
    deltabadger::engine::api_orders::forget(c, ambiguous).unwrap();
    assert!(merge_now(c, &two, two.id(1)).unwrap().is_some());
    let log = deltabadger::engine::api_orders::log_order(c, two.seeded.user_id, two.seeded.exchange_id, "F-1", None).unwrap().unwrap();
    assert_eq!(log.0.id, Some(two.id(0)), "the filled order's cancel log is S's");
}

/// T17/T18: both keys correct: 2's goes, 1's stays; 1's incorrect and 2's correct: 2's moves (no membership), 1's goes and
/// its rows' key is the kept one; a withdrawal key only on 2 moves. No bot is stopped; 2's bots trade on 1, status kept.
#[test]
fn t17_t18_keys_and_bots_move_to_the_survivor() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [two.bots[1]]).unwrap();
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!((merged.deleted_keys.clone(), merged.kept_keys.clone()), (vec![two.keys[1]], vec![two.keys[0]]));
    assert_eq!(identity::account_of_bot(c, two.bots[1]).unwrap(), Some(two.id(0)), "2's bot trades on 1");
    let status: Vec<i64> = two.bots.iter().map(|b| c.query_row("SELECT status FROM bots WHERE id = ?1", [b], |r| r.get(0)).unwrap()).collect();
    assert_eq!(status, [1, 2], "statuses kept, none stopped");
    assert_eq!(deltabadger::engine::model::credential_version(c, &deltabadger::engine::model::load_bot(c, two.bots[1]).unwrap()).unwrap().unwrap().id, two.keys[0], "with 1's kept key");

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two.keys[0]]).unwrap();
    let row = two.row(c, 0, 4, "BTC", "1", Some("on-1"), None, "2026-09-01 10:00:00");
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'w', 'w', 1, 1, 'now', 'now')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    let withdrawal = c.last_insert_rowid();
    c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [withdrawal, two.id(1)]).unwrap();
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(merged.deleted_keys, [two.keys[0]]);
    assert_eq!(identity::account_of_key(c, two.keys[1]).unwrap(), Some(two.id(0)), "2's key is 1's now");
    assert_eq!(identity::account_of_key(c, withdrawal).unwrap(), Some(two.id(0)), "the withdrawal key moves");
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_keys"), 0, "no membership into a number 1");
    assert_eq!(c.query_row("SELECT api_key_id FROM account_transactions WHERE id = ?1", [row], |r| r.get::<_, i64>(0)).unwrap(), two.keys[1], "re-pointed");
}

fn rules_of(c: &Connection) -> Vec<(i64, i64, i64)> {
    c.prepare("SELECT id, account_id, status FROM exchange_account_rules ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
}
fn legacy(c: &Connection, id: i64) -> (Option<i64>, i64) { c.query_row("SELECT exchange_id, status FROM rules WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() }

/// T19: rules. S live, D live: S's stays, D's is deleted; S none: D's moves and is mirrored; S deleted: the staged swap;
/// D deleted: it stays on D. Withdrawal ids move, a twin once, and `id_taken` holds on S.
#[test]
fn t19_rules_move_without_meeting_a_unique_index() {
    let cipher = seed::cipher();
    let (btc, eur) = (|t: &Two| t.seeded.btc, |t: &Two| t.seeded.quote);
    let _ = eur;
    // S live, D live.
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let (s, d) = (two.rule(c, &cipher, 0, btc(&two), 0), two.rule(c, &cipher, 1, btc(&two), 0));
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(rules_of(c), [(s, two.id(0), 0), (d, two.id(1), 3)]);
    // S none, D live.
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let d = two.rule(c, &cipher, 1, btc(&two), 0);
    assert_eq!(legacy(c, d), (None, 3), "a placeholder while on 2");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(rules_of(c), [(d, two.id(0), 0)]);
    assert_eq!(legacy(c, d), (Some(two.seeded.exchange_id), 0), "mirrored");
    // S deleted, D live: the staged swap.
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let (s, d) = (two.rule(c, &cipher, 0, btc(&two), 3), two.rule(c, &cipher, 1, btc(&two), 0));
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(rules_of(c), [(s, two.id(1), 3), (d, two.id(0), 0)]);
    assert_eq!((legacy(c, s), legacy(c, d)), ((None, 3), (Some(two.seeded.exchange_id), 0)));
    // S live / none, D deleted: D's stays on D.
    for s_status in [Some(0), None] {
        let (_d, o, two) = duplicate();
        let c = &o.primary;
        let s = s_status.map(|st| two.rule(c, &cipher, 0, btc(&two), st));
        let d = two.rule(c, &cipher, 1, btc(&two), 3);
        merge_now(c, &two, two.id(1)).unwrap().unwrap();
        let mut want: Vec<(i64, i64, i64)> = s.map(|s| (s, two.id(0), 0)).into_iter().collect();
        want.push((d, two.id(1), 3));
        assert_eq!(rules_of(c), want);
    }
    // Withdrawal ids.
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    two.withdrawal_id(c, 0, "W-both", "rule:1");
    two.withdrawal_id(c, 1, "W-both", "rule:2");
    two.withdrawal_id(c, 1, "W-two", "rule:2");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let ids: Vec<(i64, String, String)> = c.prepare("SELECT account_id, venue_id, owner FROM rust_withdrawal_ids ORDER BY venue_id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(ids, [(two.id(0), "W-both".into(), "rule:1".into()), (two.id(0), "W-two".into(), "rule:2".into())]);
}

/// T20: 2's orders move to 1 (2.x's column takes the id, the membership goes); a shared bot-order id refuses naming both
/// orders, and nothing changes; standalone orders move.
#[test]
fn t20_orders_move_and_a_shared_id_refuses() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let order = two.order(c, 1, "only-2", 0, Some(2));
    let standalone = two.standalone(c, 1, "done", Some(r#"{"kind":"placed","order_id":"S-1","terminal":"filled"}"#), Some("S-1"));
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(c.query_row("SELECT external_id FROM transactions WHERE id = ?1", [order], |r| r.get::<_, String>(0)).unwrap(), "only-2");
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_transactions"), 0);
    assert_eq!(identity::Account::of_order(c, order).unwrap().unwrap().id, Some(two.id(0)));
    assert_eq!(c.query_row("SELECT account_id FROM rust_api_order_standalone WHERE order_id = ?1", [standalone], |r| r.get::<_, i64>(0)).unwrap(), two.id(0));

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    two.order(c, 0, common::two_accounts::SHARED_ORDER, 0, Some(2));
    two.order(c, 1, common::two_accounts::SHARED_ORDER, 0, Some(2));
    let before = dump(c);
    match merge_now(c, &two, two.id(1)) { Err(Refusal::Conflict(rows)) => assert_eq!(rows.len(), 2, "{rows:?}"), other => panic!("{other:?}") }
    assert_eq!(dump(c), before);
}

fn links(c: &Connection) -> Vec<(Option<String>, Option<String>)> {
    let any = deltabadger::venue_ids::Scope::install(c).unwrap();
    c.prepare(&format!("SELECT {}, (SELECT {} FROM account_transactions l WHERE l.id = a.linked_transaction_id) FROM account_transactions a WHERE a.linked_transaction_id IS NOT NULL ORDER BY a.id",
                       any.row_id("a"), any.row_id("l"))).unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
fn link(c: &Connection, from: i64, to: i64) { c.execute("UPDATE account_transactions SET linked_transaction_id = ?2 WHERE id = ?1", [from, to]).unwrap(); }

/// T21: twins are deleted and the rest move (an id-less row too); SHARED_GROUP's legs make one group; metadata merges or
/// refuses; links resolve as one graph, whatever order the rows were made in.
#[test]
fn t21_ledger_rows_dedup_by_venue_id_and_keep_their_metadata() {
    use common::two_accounts::{SHARED_GROUP, SHARED_TX};
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let s_twin = two.row(c, 0, 4, "BTC", "1", Some(SHARED_TX), Some(SHARED_GROUP), "2026-09-01 10:00:00");
    two.row(c, 1, 4, "BTC", "1", Some(SHARED_TX), Some(SHARED_GROUP), "2026-09-01 10:00:00");
    let moved = two.row(c, 1, 4, "BTC", "2", Some("only-2"), Some(SHARED_GROUP), "2026-09-02 10:00:00");
    let idless = two.row(c, 1, 10, "BTC", "0.1", None, None, "2026-09-03 10:00:00");
    c.execute("UPDATE account_transactions SET manual_values = '{\"price\":\"5\"}' WHERE tx_id IS NULL AND id = (SELECT max(id) - 2 FROM account_transactions)", []).unwrap();
    let all_before = deltabadger::tracker::rows::load_all(c, two.seeded.user_id).unwrap().len();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let ids: Vec<i64> = c.prepare("SELECT id FROM account_transactions ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(ids, [s_twin, moved, idless], "the twin went, the others moved");
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM exchange_account_rows"), 0);
    assert_eq!(c.query_row("SELECT tx_id FROM account_transactions WHERE id = ?1", [moved], |r| r.get::<_, String>(0)).unwrap(), "only-2");
    assert_eq!(c.query_row("SELECT manual_values FROM account_transactions WHERE id = ?1", [s_twin], |r| r.get::<_, String>(0)).unwrap(), "{\"price\":\"5\"}", "D's price fills S's empty one");
    let groups: Vec<Option<String>> = deltabadger::tracker::rows::load_all(c, two.seeded.user_id).unwrap().into_iter().map(|r| r.group).collect();
    assert_eq!(groups, [Some(SHARED_GROUP.to_string()), Some(SHARED_GROUP.to_string()), None], "one group");
    assert_eq!(all_before, 1, "before, the duplicate's rows were left out of the walk");

    // Metadata conflicts refuse, naming the row, and change nothing.
    let conflict = |setup: &dyn Fn(&Connection, &Two, i64, i64)| {
        let (_d, o, two) = duplicate();
        let c = &o.primary;
        let s = two.row(c, 0, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
        let d = two.row(c, 1, 4, "BTC", "1", Some(SHARED_TX), None, "2026-09-01 10:00:00");
        setup(c, &two, s, d);
        let before = dump(c);
        let out = merge_now(c, &two, two.id(1));
        let refused = matches!(&out, Err(Refusal::Conflict(rows)) if rows.iter().any(|r| r.contains(" 1 BTC")));
        if refused { assert_eq!(dump(c), before); }
        refused
    };
    let manual = |c: &Connection, id: i64, v: &str| { c.execute("UPDATE account_transactions SET manual_values = ?2 WHERE id = ?1", rusqlite::params![id, v]).unwrap(); };
    assert!(conflict(&|c, _, s, d| { manual(c, s, "{\"price\":\"5\"}"); manual(c, d, "{\"price\":\"6\"}"); }), "different prices");
    assert!(!conflict(&|c, _, s, d| { manual(c, s, "{\"price\":\"5\"}"); manual(c, d, "{\"price\":\"5\"}"); }), "equal prices");
    assert!(conflict(&|c, two, s, d| {
        let (a, b) = (two.order(c, 0, "o-a", 0, Some(2)), two.order(c, 0, "o-b", 0, Some(2)));
        c.execute("UPDATE account_transactions SET transaction_id = CASE id WHEN ?1 THEN ?3 ELSE ?4 END WHERE id IN (?1, ?2)", [s, d, a, b]).unwrap();
    }), "two bot orders");
    assert!(conflict(&|c, two, s, d| {
        let (x, y) = (two.row(c, 0, 5, "BTC", "1", Some("x"), None, "2026-08-01 10:00:00"), two.row(c, 0, 5, "BTC", "1", Some("y"), None, "2026-08-01 10:00:00"));
        link(c, x, s); link(c, y, d);
    }), "two sources onto one target");
    assert!(conflict(&|c, two, s, d| {
        let x = two.row(c, 0, 5, "BTC", "1", Some("x"), None, "2026-08-01 10:00:00");
        c.execute("UPDATE account_transactions SET transfer_link_rejected = 1 WHERE id = ?1", [d]).unwrap();
        link(c, x, s);
    }), "rejected on one side, linked on the other");

    // Kept links: D's outgoing one, one into D, both sides twins; crossed links (r3) in both orders.
    for deposits_first in [true, false] {
        let (_d, o, two) = duplicate();
        let c = &o.primary;
        let pair = |id: &str, kind: i64| -> (i64, i64) {
            (two.row(c, 0, kind, "BTC", "1", Some(id), None, "2026-09-01 10:00:00"), two.row(c, 1, kind, "BTC", "1", Some(id), None, "2026-09-01 10:00:00"))
        };
        let ((sd, dd), (sw, dw)) = if deposits_first { let d = pair("dep", 4); (d, pair("wd", 5)) } else { let w = pair("wd", 5); (pair("dep", 4), w) };
        link(c, sw, dd);
        link(c, dw, sd);
        let out = two.row(c, 1, 5, "BTC", "1", Some("out-2"), None, "2026-09-05 10:00:00");
        let ext = two.row(c, 0, 4, "BTC", "1", Some("in-1"), None, "2026-09-05 10:00:00");
        link(c, out, ext);
        merge_now(c, &two, two.id(1)).unwrap().unwrap();
        assert_eq!(links(c), [(Some("wd".into()), Some("dep".into())), (Some("out-2".into()), Some("in-1".into()))], "deposits first: {deposits_first}");
        let _ = (sd, sw);
    }
}

/// T22/T23/T24: 2's balances go and 1's stay; snapshots and caches are cleared; no app_configs key names 2; KuCoin coverage
/// and its HF marker move, withheld twins stored once; Binance A12 state on 2 refuses.
#[test]
fn t22_t23_t24_balances_kucoin_and_binance() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    hold(c, &two, 0, "1", "100");
    hold(c, &two, 1, "1", "100");
    c.execute("INSERT INTO portfolio_snapshots (user_id, date, value_usd, invested_usd, partial, created_at, updated_at) VALUES (?1, '2026-09-01', 1, 1, 0, 'now', 'now')", [two.seeded.user_id]).unwrap();
    let b = identity::AccountRow::of(c, &two.accounts[1]).unwrap().unwrap();
    deltabadger::app_config::set_plain(c, &deltabadger::sync::cache::balance_origin_key_of(&b), "{}", chrono::Utc::now()).unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM exchange_account_balances WHERE account_id = {}", two.id(1))), 0);
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM exchange_account_balances WHERE account_id = {}", two.id(0))), 1);
    assert_eq!(one::<i64>(c, "SELECT count(*) FROM portfolio_snapshots"), 0, "re-swept");
    let named: i64 = c.query_row("SELECT count(*) FROM app_configs WHERE key LIKE ?1", [format!("%:{}:{}%", two.seeded.exchange_id, two.id(1))], |r| r.get(0)).unwrap();
    assert_eq!(named, 0, "no key names 2");

    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    kucoin.fingerprint(c, 0, Some("fp"));
    kucoin.fingerprint(c, 1, Some("fp"));
    for n in [0, 1] {
        c.execute("INSERT INTO kucoin_coverage (account_id, source, start_at, end_at, updated_at) VALUES (?1, 'hf_enabled', 'a', 'a', 'now')", [kucoin.id(n)]).unwrap();
        c.execute("INSERT INTO kucoin_withheld_fills (account_id, tx_id, transacted_at, entry, created_at) VALUES (?1, 'f-1', 'now', '{}', 'now')", [kucoin.id(n)]).unwrap();
    }
    c.execute("INSERT INTO kucoin_coverage (account_id, source, start_at, end_at, updated_at) VALUES (?1, 'export', 'a', 'b', 'now')", [kucoin.id(1)]).unwrap();
    c.execute("INSERT INTO kucoin_withheld_fills (account_id, tx_id, transacted_at, entry, created_at) VALUES (?1, 'f-2', 'now', '{}', 'now')", [kucoin.id(1)]).unwrap();
    merge_now(c, &kucoin, kucoin.id(1)).unwrap().unwrap();
    let rows = |sql: &str| -> Vec<(i64, String)> { c.prepare(sql).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect() };
    assert_eq!(rows("SELECT account_id, source FROM kucoin_coverage ORDER BY id"), [(kucoin.id(0), "hf_enabled".into()), (kucoin.id(0), "export".into())]);
    assert_eq!(rows("SELECT account_id, tx_id FROM kucoin_withheld_fills ORDER BY tx_id"), [(kucoin.id(0), "f-1".into()), (kucoin.id(0), "f-2".into())]);

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    deltabadger::app_config::set_plain(c, &format!("rust_binance_a12_v1:bucket:{}:{}@{}:x", two.seeded.user_id, two.seeded.exchange_id, two.id(1)), "1", chrono::Utc::now()).unwrap();
    let before = dump(c);
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))));
    assert_eq!(dump(c), before);
}

/// T13/T25: 2 is tombstoned (no fingerprint), 1 reads `Kraken`; `?account_id=<2>`, `pick` and `pick_switch` are 404s and
/// the next number is 3; a third account observed against 2 now points at 1 and merges into it. Handback after a merge into
/// 1 reports nothing for accounts 2+; merging 3 into 2 keeps 2's refusals.
#[tokio::test(flavor = "current_thread")]
async fn t13_t25_the_tombstone_chains_and_the_handback() {
    let (dir, o, two) = duplicate();
    let c = &o.primary;
    let cipher = seed::cipher();
    let third = two.third(c).id.unwrap();
    c.execute("INSERT INTO exchange_account_overlaps (account_id, other_account_id, matches, observed_at) VALUES (?1, ?2, 3, 'now')", [third, two.id(1)]).unwrap();
    two.rule(c, &cipher, 1, two.seeded.btc, 3);
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let (removed, fingerprint): (Option<String>, Option<String>) = c.query_row("SELECT removed_at, fingerprint FROM exchange_accounts WHERE id = ?1", [two.id(1)], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(removed.is_some() && fingerprint.is_none());
    assert_eq!(one::<i64>(c, &format!("SELECT other_account_id FROM exchange_account_overlaps WHERE account_id = {third}")), two.id(0), "retargeted");
    assert_eq!(identity::duplicates(c, two.seeded.user_id).unwrap(), [(third, two.id(0))]);
    assert_eq!(identity::pick(c, two.seeded.user_id, two.seeded.exchange_id, Some(two.id(1))).unwrap(), identity::Pick::NotFound);
    assert_eq!(identity::pick_switch(c, two.seeded.user_id, two.bots[0], None, Some(two.id(1))).unwrap(), identity::Switch::NotFound);
    let app = web_app(dir.path(), c);
    assert_eq!(get(&app, c, two.seeded.user_id, &format!("/tracker?account_id={}", two.id(1))).await.status, 404);
    assert!(deltabadger::engine::rules::handback_problems(c).unwrap().is_empty(), "a deleted rule of a tombstone does not refuse the handback");
    merge_now(c, &two, third).unwrap().unwrap();
    assert_eq!(identity::label(c, two.id(0)).unwrap().plain(), "Kraken", "one live account left");
    let next: i64 = one(c, &format!("SELECT max(number) + 1 FROM exchange_accounts WHERE exchange_id = {}", two.seeded.exchange_id));
    assert_eq!(next, 4, "numbers are never reused");

    // 3 into 2: S is an account 2+, whose memberships remain and keep refusing.
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    two.fingerprint(c, 0, Some("other"));
    let third = two.third(c);
    c.execute("UPDATE exchange_accounts SET fingerprint = 'fp' WHERE id = ?1", [third.id.unwrap()]).unwrap();
    let row = two.row(c, 0, 4, "BTC", "1", Some("n1"), None, "2026-09-01 10:00:00");
    let _ = row;
    let s3 = deltabadger::venue_ids::Scope::of(c, &third).unwrap();
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, transfer_link_rejected, created_at, updated_at) \
               VALUES (?1, ?2, 4, 'BTC', 1, NULL, '2026-09-02 10:00:00', '{}', '{}', 0, '2026-09-02 10:00:00', '2026-09-02 10:00:00')", [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    deltabadger::venue_ids::after_insert(c, &s3, c.last_insert_rowid(), Some("t3")).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    merge::run(&tx, &seed::cipher(), two.seeded.user_id, third.id.unwrap(), chrono::Utc::now()).unwrap().unwrap();
    tx.commit().unwrap();
    assert_eq!(one::<i64>(c, "SELECT account_id FROM exchange_account_rows"), two.id(1), "3's row is 2's");
    // What the handback's account-2 refusals read (handover::refuse_rust_only_conditions).
    let refusing: bool = one(c, "SELECT EXISTS(SELECT 1 FROM exchange_account_rows m JOIN exchange_accounts a ON a.id = m.account_id WHERE a.number <> 1) \
                                 AND EXISTS(SELECT 1 FROM exchange_account_keys m JOIN exchange_accounts a ON a.id = m.account_id WHERE a.number <> 1)");
    assert!(refusing, "2's memberships keep refusing");
}

/// T26: the Merge row shows only while a duplicate exists; the confirm posts; success replaces `api_keys` with the flash,
/// a refusal is a 422 flash naming the rows.
#[tokio::test(flavor = "current_thread")]
async fn t26_settings_offers_merge_while_a_duplicate_exists() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    let app = web_app(dir.path(), c);
    let page = get(&app, c, user, "/settings/api_keys").await;
    assert!(!page.body.contains("merge_account"), "no duplicate, no row");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let page = get(&app, c, user, "/settings/api_keys").await;
    assert!(page.body.contains("Kraken<sup>2</sup> is the same exchange account as Kraken<sup>1</sup>.") && page.body.contains(&format!("/settings/merge_account/{}", two.id(1))),
            "{}", cut(&page.body, "api_keys", 1500));
    // A conflict: a 422 flash naming the rows.
    two.order(c, 0, common::two_accounts::SHARED_ORDER, 0, Some(2));
    two.order(c, 1, common::two_accounts::SHARED_ORDER, 0, Some(2));
    let mut b = on_settings(&app, c, user).await;
    let path = format!("/settings/merge_account/{}", two.id(1));
    let refused = b.send(&app, "POST", &path, Some(&[]), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(refused.status, 422, "{}", &refused.body[..refused.body.len().min(600)]);
    assert!(refused.body.contains("disagree on these records") && refused.body.contains("2026-09-02 buy"), "{}", refused.body);
    c.execute("DELETE FROM transactions", []).unwrap();
    c.execute("DELETE FROM exchange_account_transactions", []).unwrap();
    let merged = b.send(&app, "POST", &path, Some(&[]), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(merged.status, 200, "{}", &merged.body[..merged.body.len().min(600)]);
    assert!(merged.body.contains("action=\"replace\" target=\"api_keys\"") && merged.body.contains("was merged into"), "{}", merged.body);
    assert_eq!(b.send(&app, "POST", &path, Some(&[]), common::web::Csrf::Header, &[]).await.status, 404, "no longer a duplicate");
}

/// T15 (r2): the pre-flight reads a placed standalone order from its venue: resting (`new`) refuses with nothing changed;
/// once the venue says `filled`, the order is recorded terminal and the Merge goes through, its log on S.
#[tokio::test(flavor = "current_thread")]
async fn t15_the_preflight_reads_a_resting_order_from_its_venue() {
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let alpaca = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    alpaca.fingerprint(c, 0, Some("fp"));
    alpaca.fingerprint(c, 1, Some("fp"));
    // The fixture's EUR-quoted bots cannot run on Alpaca (the eligibility guard refuses any write while they are scheduled).
    for bot in alpaca.bots { c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).unwrap(); }
    let order = alpaca.standalone(c, 1, "done", Some(r#"{"kind":"placed","order_id":"L-1"}"#), Some("L-1"));
    let server = MockServer::start().await;
    let answer = |status: &str| ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "L-1", "client_order_id": "c", "status": status, "symbol": "BTC/EUR",
        "side": "buy", "type": "limit", "order_type": "limit", "qty": "0.001", "filled_qty": if status == "filled" { "0.001" } else { "0" }, "filled_avg_price": if status == "filled" { Some("60000") } else { None },
        "limit_price": "60000", "asset_class": "crypto"}));
    Mock::given(matchers::method("GET")).and(matchers::path("/v2/orders/L-1")).respond_with(answer("new")).up_to_n_times(1).mount(&server).await;
    Mock::given(matchers::method("GET")).and(matchers::path("/v2/orders/L-1")).respond_with(answer("filled")).mount(&server).await;
    c.execute("UPDATE users SET setup_completed = 1, confirmed_at = '2026-01-01'", []).unwrap();
    let uri = server.uri();
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at("2026-09-20T12:00:00Z"))
        .with_settings_key_boundary(deltabadger::venue::alpaca::Hosts { paper: uri.clone(), live: uri.clone(), data: uri }, deltabadger::web::settings::keys::logger()).unwrap();
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    let mut b = on_settings(&app, c, alpaca.seeded.user_id).await;
    let path = format!("/settings/merge_account/{}", alpaca.id(1));
    let before = dump(c);
    let resting = b.send(&app, "POST", &path, Some(&[]), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(resting.status, 422, "{}", &resting.body[..resting.body.len().min(600)]);
    assert!(resting.body.contains("still in flight"), "{}", resting.body);
    assert_eq!(dump(c), before, "nothing changed");
    let merged = b.send(&app, "POST", &path, Some(&[]), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(merged.status, 200, "{}", merged.body.rsplit("</button>").next().unwrap_or_default());
    let terminal: String = c.query_row("SELECT json_extract(outcome, '$.terminal') FROM rust_api_orders WHERE id = ?1", [order], |r| r.get(0)).unwrap();
    assert_eq!(terminal, "filled");
    assert_eq!(one::<i64>(c, &format!("SELECT account_id FROM rust_api_order_standalone WHERE order_id = {order}")), alpaca.id(0), "its log is S's");
    common::drop_mock_server(server).await;
}

/// T27: the eight keys are in every locale, each its own, with the arguments it names.
#[test]
fn t27_every_locale_has_the_eight_keys() {
    let keys = [("tracker.duplicate_account_html", &["duplicate", "survivor"][..]), ("tracker.merge_in_settings", &[][..]),
                ("settings.accounts.duplicate_html", &["duplicate", "survivor"][..]), ("settings.accounts.merge", &[][..]),
                ("settings.accounts.confirm_merge_html", &["duplicate", "survivor"][..]), ("settings.accounts.merged_html", &["duplicate", "survivor"][..]),
                ("settings.accounts.merge_in_flight_html", &["duplicate", "survivor"][..]), ("settings.accounts.merge_conflict_html", &["duplicate", "survivor", "rows"][..])];
    let all = deltabadger::web::i18n::all();
    let text = |locale: &str, key: &str| all.iter().find(|(k, _)| *k == format!("{locale}.{key}")).map(|(_, v)| v.to_string()).unwrap_or_else(|| panic!("{locale}.{key} missing"));
    for locale in deltabadger::web::locale::LOCALES {
        for (key, args) in keys {
            let t = text(locale, key);
            for arg in args { assert!(t.contains(&format!("%{{{arg}}}")), "{locale}.{key}: {t}"); }
            if locale != "en" && key != "settings.accounts.merge" { assert_ne!(t, text("en", key), "{locale}.{key} is its own"); }
        }
    }
}

// ---- review round 1 ----

/// R1 (KuCoin): a restored fill (no provenance) of the duplicate at a second the survivor holds an API fill of refuses the
/// Merge, naming it: both would stay visible.
#[test]
fn r1_a_restored_kucoin_fill_beside_the_others_api_copy_refuses() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    kucoin.fingerprint(c, 0, Some("fp"));
    kucoin.fingerprint(c, 1, Some("fp"));
    let api = kucoin.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1}' WHERE id = ?1", [api]).unwrap();
    kucoin.row(c, 1, 0, "BTC", "1", None, None, "2026-09-01 10:00:00");
    let before = dump(c);
    match merge_now(c, &kucoin, kucoin.id(1)) { Err(Refusal::Conflict(rows)) => assert_eq!(rows, ["KuCoin 2: 2026-09-01 buy 1 BTC"]), other => panic!("{other:?}") }
    assert_eq!(dump(c), before);
}

/// R2: an unresolved KuCoin settlement on an order of either account, or a withdrawal intent under either account's key
/// (a recorder's included), is in flight.
#[test]
fn r2_settlements_and_every_withdrawal_intent_are_in_flight() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let order = two.order(c, 1, "k-1", 1, Some(2));
    deltabadger::app_config::set_plain(c, &format!("{}{order}", deltabadger::engine::reconcile::UNRESOLVED), "{\"why\":\"x\"}", chrono::Utc::now()).unwrap();
    refused_unchanged(c, &two, "unresolved settlement");
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("INSERT INTO rust_withdrawal_intents (owner, state, client_id, created_at, api_key_id, credential_digest, host, account_scope, key_identity, params_enc, fee_basis, budget, free_balance) \
               VALUES ('recorder:s1', 'prepared', 'c', 'now', ?1, 'd', 'h', 's', 'k', 'p', 'f', 'b', 'f')", [two.keys[0]]).unwrap();
    refused_unchanged(c, &two, "a recorder's intent on 1");
}

/// R3: overlap evidence counts only rows the venue returned: a restored copy of the other account's trades (a Deltabadger
/// CSV restore stores no venue answer) is no evidence.
#[test]
fn r3_restored_rows_are_never_overlap_evidence() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    c.execute("UPDATE account_transactions SET raw_data = '{}' WHERE id IN (SELECT id FROM account_transactions ORDER BY id LIMIT 3)", []).unwrap();
    assert!(!observe(c, &two, 1), "account 1's copies were restored");
}

/// R4: a chain whose known fingerprints differ along it (#3 B, #2 unknown, #1 A) is no duplicate.
#[test]
fn r4_a_chain_with_conflicting_fingerprints_is_no_duplicate() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let third = two.third(c).id.unwrap();
    two.fingerprint(c, 0, Some("A"));
    c.execute("UPDATE exchange_accounts SET fingerprint = 'B' WHERE id = ?1", [third]).unwrap();
    for (d, s) in [(two.id(1), two.id(0)), (third, two.id(1))] {
        c.execute("INSERT INTO exchange_account_overlaps (account_id, other_account_id, matches, observed_at) VALUES (?1, ?2, 3, 'now')", [d, s]).unwrap();
    }
    assert_eq!(identity::duplicates(c, two.seeded.user_id).unwrap(), [(two.id(1), two.id(0))]);
}

/// R5: a survivor's withdrawal linked to the duplicate's deposit reads unlinked once that deposit leaves the walk.
#[test]
fn r5_a_link_to_a_duplicates_row_reads_unlinked() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let w = two.row(c, 0, 5, "BTC", "1", Some("w"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 4, "BTC", "1", Some("dep"), None, "2026-09-01 10:00:00");
    two.row(c, 0, 4, "BTC", "1", Some("dep"), None, "2026-09-01 10:00:00");
    link(c, w, d);
    let rows = deltabadger::tracker::rows::load_all(c, two.seeded.user_id).unwrap();
    assert_eq!(rows.iter().find(|r| r.id == w).unwrap().linked_to, None);
}

/// R7/R8: a discovery reply and a job's later capture are refused once the key moved to another account.
#[tokio::test(flavor = "current_thread")]
async fn r7_r8_late_captures_on_another_account_are_refused() {
    let (_d, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    let version = deltabadger::engine::model::credential_version_by_id(c, two.keys[1]).unwrap().unwrap();
    let credentials = deltabadger::crypto::Credentials { ibkr: None, redaction_values: vec![], key: "account-2-key".into(), secret: "account-2-secret".into(), passphrase: None };
    let due = identity::Due { api_key_id: two.keys[1], exchange_type: "Exchanges::Kraken".into(), credentials: credentials.clone(), version: version.clone() };
    move_key_to_one(c, two.keys[1]);
    assert!(!identity::publish(c, &cipher, &due, &identity::Observed::Failed("x".into()), chrono::Utc::now()).unwrap(), "discovery");
    let db = deltabadger::jobs::Db::new(Connection::open(_d.path().join("production.sqlite3")).unwrap(), cipher.clone());
    let plain = deltabadger::sync::capture_bound_version(&db, two.keys[1], &credentials).await;
    assert!(plain.is_ok(), "unbound, a capture is as before");
    let bound = deltabadger::sync::bound(Some(two.id(1)), deltabadger::sync::capture_bound_version(&db, two.keys[1], &credentials)).await;
    assert!(bound.is_err(), "bound to account 2, a capture on account 1 is refused");
}

/// R9: a live account 2+ refuses the handback with nothing else of it left; a Merge of 2 into 1 lets the handback through,
/// 2.x reads the moved rule in its own columns, and a takeover keeps it on account 1.
#[tokio::test(flavor = "current_thread")]
async fn r9_t25_merge_handback_and_takeover() {
    use deltabadger::engine::handover;
    let now = chrono::Utc::now();
    let dir = common::v2_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = deltabadger::lease::lock(&p, now).unwrap();
    let o = store::open(&p).unwrap();
    let cipher = seed::cipher();
    let two = Two::kraken(&o.primary, &cipher);
    let c = &o.primary;
    for b in two.bots { c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [b]).unwrap(); }
    handover::take_over(&l, &o, &cipher, "0.2.0", now).unwrap();
    let rule = two.rule(c, &cipher, 1, two.seeded.btc, 0);
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let third = two.third(c).id.unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let refused = handover::hand_back(&l, &o, &deltabadger::venue::fake::FakeFactory::default(), &cipher, &deltabadger::engine::FixedClock(now)).await;
    assert!(matches!(&refused, Err(deltabadger::engine::EngineError::Ineligible(lines)) if lines.iter().any(|l| l.contains(handover::SECOND_ACCOUNT))), "{refused:?}");
    c.execute("UPDATE exchange_accounts SET removed_at = 'now' WHERE id = ?1", [third]).unwrap();
    handover::hand_back(&l, &o, &deltabadger::venue::fake::FakeFactory::default(), &cipher, &deltabadger::engine::FixedClock(now)).await.unwrap();
    assert_eq!(c.query_row("SELECT exchange_id, status FROM rules WHERE id = ?1", [rule], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?))).unwrap(), (Some(two.seeded.exchange_id), 0));
    handover::take_over(&l, &o, &cipher, "0.2.0", now).unwrap();
    assert_eq!(identity::account_of_rule(c, rule).unwrap(), Some(two.id(0)), "the rule is on account 1 again");
}

/// R10: a duplicate's own activity moves no history version.
#[test]
fn r10_a_duplicates_activity_moves_no_history_version() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let before = deltabadger::tracker::backfill::history_version(c, two.seeded.user_id).unwrap();
    two.row(c, 1, 4, "BTC", "1", Some("late"), None, "2026-09-25 10:00:00");
    assert_eq!(deltabadger::tracker::backfill::history_version(c, two.seeded.user_id).unwrap(), before);
}

// ---- review round 2 ----

/// A: twins merge only when every financial fact agrees; another fee, or another venue answer, refuses naming both rows.
#[test]
fn ra_twins_with_different_facts_refuse() {
    for (what, sql) in [("fee", "UPDATE account_transactions SET fee_currency = 'BTC', fee_amount = '0.01' WHERE id = ?1"),
                        ("raw answer", "UPDATE account_transactions SET raw_data = '{\"legs\":2}' WHERE id = ?1")] {
        let (_d, o, two) = duplicate();
        let c = &o.primary;
        two.trade(c, 0, 0, "t-1", "1", "2026-09-01 10:00:00");
        let d = two.trade(c, 1, 0, "t-1", "1", "2026-09-01 10:00:00");
        c.execute(sql, [d]).unwrap();
        let before = dump(c);
        match merge_now(c, &two, two.id(1)) { Err(Refusal::Conflict(rows)) => assert_eq!(rows.len(), 2, "{what}: {rows:?}"), other => panic!("{what}: {other:?}") }
        assert_eq!(dump(c), before, "{what}");
    }
}

/// B: a KuCoin CSV fill (with its source rows) of one account at a second the other holds an API fill of refuses.
#[test]
fn rb_a_kucoin_csv_fill_beside_the_others_api_copy_refuses() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    kucoin.fingerprint(c, 0, Some("fp"));
    kucoin.fingerprint(c, 1, Some("fp"));
    let api = kucoin.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1}' WHERE id = ?1", [api]).unwrap();
    let csv = kucoin.row(c, 1, 0, "BTC", "1", Some("kucoin-csv-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"source\":\"kucoin_csv\",\"class\":\"fills\",\"source_rows\":[\"r1\"]}' WHERE id = ?1", [csv]).unwrap();
    c.execute("INSERT INTO import_rows (user_id, exchange_id, row_id, account_type, currency, side, amount, fee, transacted_at, remark, row_type, created_at) \
               VALUES (?1, ?2, 'r1', 'TRADE', 'BTC', 'in', '1', '0', '2026-09-01 10:00:00', '', 'fills', 'now')", [kucoin.seeded.user_id, kucoin.seeded.exchange_id]).unwrap();
    assert!(matches!(merge_now(c, &kucoin, kucoin.id(1)), Err(Refusal::Conflict(_))));
}

/// C: a recorder's withdrawal on the survivor whose key was deleted is still in flight; another user's unresolved KuCoin
/// settlement on the same exchange is not this Merge's.
#[test]
fn rc_in_flight_by_account_and_by_user() {
    let cipher = seed::cipher();
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let params = serde_json::json!({"user_id": two.seeded.user_id, "exchange_id": two.seeded.exchange_id}).to_string();
    c.execute("INSERT INTO rust_withdrawal_intents (owner, state, client_id, created_at, api_key_id, credential_digest, host, account_scope, key_identity, params_enc, fee_basis, budget, free_balance) \
               VALUES ('recorder:s2', 'unresolved', 'c', 'now', 999999, 'd', 'h', 's', 'k', ?1, 'f', 'b', 'f')", [cipher.encrypt(&params)]).unwrap();
    refused_unchanged(c, &two, "a recorder intent whose key was deleted");

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('other@example.com', 'x', 0, 'now', 'now')", []).unwrap();
    let other = seed::Seeded { user_id: c.last_insert_rowid(), ..two.seeded };
    let bot = seed::insert_bot(c, &other, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let order = seed::insert_tx(c, &other, bot, &seed::TxSpec { status: 1, external_status: Some(2), external_id: Some("theirs".into()), order_type: 0, amount: Some("0.001"),
        quote_amount: Some("60"), price: Some("60000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-02 10:00:00".into() });
    deltabadger::app_config::set_plain(c, &format!("{}{order}", deltabadger::engine::reconcile::UNRESOLVED), "{\"why\":\"x\"}", chrono::Utc::now()).unwrap();
    assert!(merge_now(c, &two, two.id(1)).unwrap().is_some(), "another user's order is not in flight here");
}

/// E: an id built without the venue's own trade id (`SYMBOL--`) is no evidence.
#[test]
fn re_synthetic_ids_are_no_evidence() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    for k in 0..2 { for i in 0..3 { two.trade(c, k, 0, &format!("BTCEUR-{i}-"), "1", &format!("2026-09-0{} 10:00:00", i + 1)); } }
    assert!(!observe(c, &two, 1));
}

/// G: a duplicate's key error marks no snapshot partial; rotating a duplicate's key changes no portfolio provenance; the
/// selected account's own keys decide never-synced (a third account's sync does not hide it).
#[tokio::test(flavor = "current_thread")]
async fn rg_a_duplicates_keys_decide_no_freshness() {
    let (dir, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET last_sync_error = 'Kraken said no' WHERE id = ?1", [two.keys[1]]).unwrap();
    assert!(!deltabadger::tracker::snapshot::partial(c, two.seeded.user_id, &[]).unwrap(), "the duplicate's error");
    let stamps = deltabadger::sync::cache::capture_read(c, two.seeded.user_id, &deltabadger::tracker::Scope::All).unwrap().producer_stamps();
    assert_eq!(stamps.as_array().unwrap().len(), 1, "the duplicate's key is no producer of All");
    // Never synced: account 1 has never synced; the duplicate and a third account have.
    let third = two.third(c);
    c.execute("UPDATE exchange_accounts SET fingerprint = 'other' WHERE id = ?1", [third.id.unwrap()]).unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, balances_synced_at, created_at, updated_at) VALUES (?1, ?2, 'k3', 's3', 1, 0, '2026-09-20 10:00:00', 'now', 'now')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    third.add_key(c, c.last_insert_rowid()).unwrap();
    c.execute("UPDATE api_keys SET balances_synced_at = '2026-09-20 10:00:00', last_sync_error = NULL WHERE id = ?1", [two.keys[1]]).unwrap();
    hold(c, &two, 1, "1", "100");
    let app = web_app(dir.path(), c);
    let page = get(&app, c, two.seeded.user_id, &format!("/tracker?exchange_id={}", two.seeded.exchange_id)).await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Click Sync to load your portfolio."), "{}", cut(&page.body, "tracker-holdings", 600));
}

/// H: when the standing changes and nothing is counted, today's point goes too.
#[tokio::test(flavor = "current_thread")]
async fn rh_todays_point_follows_the_standing() {
    let (dir, o) = install();
    let two = Two::kraken(&o.primary, &seed::cipher());
    let user = two.seeded.user_id;
    two.row(&o.primary, 1, 4, "USD", "100", Some("dep-d"), None, "2026-01-10 10:00:00");
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let db = deltabadger::jobs::Db::new(o.primary, seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    let clock = deltabadger::engine::FixedClock(at);
    let sweep = || deltabadger::tracker::jobs::backfill_run(&db, &NoVenue, None::<&deltabadger::jobs::market::Feed<deltabadger::venue::http::ScriptedTransport>>, None, user, &clock);
    assert_eq!(sweep().await, Ok(true));
    c.execute("INSERT INTO portfolio_snapshots (user_id, date, value_usd, invested_usd, partial, created_at, updated_at) VALUES (?1, '2026-09-20', 100, 100, 0, 'now', 'now') \
               ON CONFLICT (user_id, date) DO NOTHING", [user]).unwrap();
    c.execute_batch("UPDATE exchange_accounts SET fingerprint = 'fp'").unwrap();
    assert_eq!(sweep().await, Ok(false));
    let left: i64 = c.query_row("SELECT count(*) FROM portfolio_snapshots WHERE user_id = ?1", [user], |r| r.get(0)).unwrap();
    assert_eq!(left, 0, "no stale point on All");
}

/// I: a completed Binance history repair of the duplicate refuses the Merge.
#[test]
fn ri_a_binance_repair_checkpoint_refuses() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let key = deltabadger::jobs::state::key("binance_history_repair_account", Some(&format!("3:{}:{}@{}", two.seeded.user_id, two.seeded.exchange_id, two.id(1))));
    deltabadger::app_config::set_plain(c, &key, "{}", chrono::Utc::now()).unwrap();
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))));
}
