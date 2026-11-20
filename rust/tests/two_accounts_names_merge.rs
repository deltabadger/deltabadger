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
