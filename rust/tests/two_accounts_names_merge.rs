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

/// A trading key of account `account` (a third one has none).
fn add_key(c: &Connection, two: &Two, account: i64) -> i64 {
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, 's', 1, 0, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
              rusqlite::params![two.seeded.user_id, two.seeded.exchange_id, format!("key-of-{account}")]).unwrap();
    let key = c.last_insert_rowid();
    identity::Account::by_id(c, account).unwrap().unwrap().add_key(c, key).unwrap();
    key
}
/// Account `n`'s and account 1's balances read at one time, equal (1 BTC each): whether 2 now stands as 1's duplicate.
fn overlaps(c: &Connection, two: &Two, n: usize) -> bool {
    for k in [0, n] { common::two_accounts::read_balances(c, two.seeded.user_id, two.id(k), two.keys[k], &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    standing(c, two).contains(&(two.id(n), two.id(0)))
}
/// The stored standing after `identity::reconsider` ran for the exchange, as a sync's completion runs it.
fn standing(c: &Connection, two: &Two) -> Vec<(i64, i64)> {
    identity::reconsider(c, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).unwrap();
    identity::duplicates(c, two.seeded.user_id).unwrap()
}
/// `n` buys of account `account` (any number) imported by its key `key`, under the shared trade ids `trade-0…`.
fn trades_on(c: &Connection, two: &Two, account: i64, key: i64, n: usize) {
    let scope = deltabadger::venue_ids::Scope::of(c, &identity::Account::by_id(c, account).unwrap().unwrap()).unwrap();
    for i in 0..n {
        let (id, at) = (format!("trade-{i}"), format!("2026-09-0{} 10:00:00", i + 1));
        c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at, \
                   raw_data, manual_values, transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 'BTC', '1', 'EUR', '100', ?4, ?5, json_object('refid', ?6), '{}', 0, ?5, ?5)",
                  rusqlite::params![two.seeded.user_id, key, two.seeded.exchange_id, scope.stored(Some(&id)), at, id]).unwrap();
        deltabadger::venue_ids::after_insert(c, &scope, c.last_insert_rowid(), Some(&id)).unwrap();
    }
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
    assert!(standing(c, &two).is_empty(), "NULL fingerprints");
    two.fingerprint(c, 0, Some("fp-1"));
    assert!(standing(c, &two).is_empty(), "one NULL");
    two.fingerprint(c, 1, Some("fp-2"));
    assert!(standing(c, &two).is_empty(), "different");
    two.fingerprint(c, 1, Some("fp-1"));
    assert_eq!(standing(c, &two), [(b, a)]);
    let third = two.third(c).id.unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k3', 's3', 1, 0, 'now', 'now')", [user, two.seeded.exchange_id]).unwrap();
    identity::Account::by_id(c, third).unwrap().unwrap().add_key(c, c.last_insert_rowid()).unwrap();
    common::two_accounts::confirm(c, third, Some("fp-1"));
    assert_eq!(standing(c, &two), [(b, a), (third, a)]);
    remove(c, a);
    assert_eq!(standing(c, &two), [(third, b)], "a removed 1");
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
        overlaps(c, &two, 1)
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
    assert!(!overlaps(c, &kucoin, 1), "KuCoin");
    trades(c, &kraken, 0, 3, 0, "1");
    trades(c, &kraken, 1, 3, 0, "1");
    assert!(overlaps(c, &kraken, 1));
    assert_eq!(standing(c, &kraken), [(kraken.id(1), kraken.id(0))], "read again, the same");
}

/// T13: 3 observed against 2 while 2 is 1's duplicate by fingerprint: both chains end at 1.
#[test]
fn t13_chains_resolve_to_the_lowest_live_end() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let third = two.third(c).id.unwrap();
    let key = add_key(c, &two, third);
    trades(c, &two, 1, 3, 0, "1");
    trades_on(c, &two, third, key, 3);
    for (a, k) in [(two.id(1), two.keys[1]), (third, key)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0)), (third, two.id(0))]);
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
        for n in 0..2 { common::two_accounts::confirm(&c, two.id(n), Some("fp")); }
        let cipher = seed::cipher();
        assert!(deltabadger::tracker::backfill::wanted(&c, &cipher, user, at.date_naive()).unwrap(), "the standing asks for a sweep");
        if s_rows {
            assert_eq!(sweep().await, Ok(true));
            assert_eq!(first(&c, "portfolio_snapshots").as_deref(), Some("2026-06-10"), "no day before S's first row");
        } else {
            assert_eq!(sweep().await, Ok(true), "the history changed: the ledger walk records today again");
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
        // What the syncs a Merge wakes write (their times, their stamps) is not what a refusal must leave alone.
        let sql = match t.as_str() {
            "app_configs" => "SELECT * FROM app_configs WHERE key NOT LIKE 'rust_job.ledger_sync:%' AND key NOT LIKE 'rust_job.ledger_origin:%' AND key NOT LIKE 'rust_job.balance_origin:%'".to_string(),
            "api_keys" => "SELECT id, key, secret, status, key_type, exchange_id, user_id FROM api_keys".to_string(),
            _ => format!("SELECT * FROM \"{t}\""),
        };
        let mut s = c.prepare(&sql).unwrap();
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
    common::two_accounts::stamp_ledgers(&o.primary, two.seeded.user_id);
    two.fingerprint(&o.primary, 0, Some("fp"));
    two.fingerprint(&o.primary, 1, Some("fp"));
    identity::reconsider(&o.primary, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).unwrap();
    (d, o, two)
}
/// `merge::run` in one transaction, committed only when it merged.
/// `merge::run` after the syncs a Merge wakes have just run.
fn merge_now(c: &Connection, two: &Two, d: i64) -> Result<Option<merge::Merged>, Refusal> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now);
    identity::reconsider(c, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).unwrap();
    merge_stale(c, two, d)
}
/// `merge::run` on the data as it is.
fn merge_stale(c: &Connection, two: &Two, d: i64) -> Result<Option<merge::Merged>, Refusal> {
    let tx = c.unchecked_transaction().unwrap();
    let survivor = identity::duplicates(&tx, two.seeded.user_id).unwrap().into_iter().find(|(x, _)| *x == d).map_or(0, |(_, s)| s);
    let out = merge::run(&tx, &seed::cipher(), two.seeded.user_id, d, survivor, chrono::Utc::now());
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
    // 1 still reads with a correct read-only key (Merge's live check reads both accounts).
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'r', 'r', 1, 2, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    let row = two.row(c, 0, 4, "BTC", "1", Some("on-1"), None, "2026-09-01 10:00:00");
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'w', 'w', 1, 1, 'now', 'now')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    let withdrawal = c.last_insert_rowid();
    c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [withdrawal, two.id(1)]).unwrap();
    // Every live key observes the fingerprint (R7 #4).
    for n in 0..2 { two.fingerprint(c, n, Some("fp")); }
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
    // Rows a venue sync wrote (a row no sync wrote, unique to D, refuses: r4_a).
    for row in [s_twin, moved, idless] { synced(c, row); }
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
        for row in [out, ext] { synced(c, row); }
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
    let key = add_key(c, &two, third);
    trades(c, &two, 1, 3, 0, "1");
    trades_on(c, &two, third, key, 3);
    for (a, k) in [(two.id(1), two.keys[1]), (third, key)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert!(standing(c, &two).contains(&(third, two.id(0))), "3 duplicates 2, which duplicates 1");
    two.rule(c, &cipher, 1, two.seeded.btc, 3);
    // 1 holds what 2 holds (one venue account): the fresh reads agree.
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(0), two.keys[0], &[(two.seeded.btc, "1")], "2026-09-20 10:00:00");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    // Read fresh: 3 against 1, whose rows are now 2's, read by 1's key, once 1's balances are read too.
    for (a, k) in [(two.id(0), two.keys[0]), (third, key)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-21 10:00:00"); }
    let (removed, fingerprint): (Option<String>, Option<String>) = c.query_row("SELECT removed_at, fingerprint FROM exchange_accounts WHERE id = ?1", [two.id(1)], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(removed.is_some() && fingerprint.is_none());
    assert_eq!(standing(c, &two), [(third, two.id(0))]);
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
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k3', 's3', 1, 0, 'now', 'now')", [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    third.add_key(c, c.last_insert_rowid()).unwrap();
    common::two_accounts::confirm(c, third.id.unwrap(), Some("fp"));
    let row = two.row(c, 0, 4, "BTC", "1", Some("n1"), None, "2026-09-01 10:00:00");
    let _ = row;
    let s3 = deltabadger::venue_ids::Scope::of(c, &third).unwrap();
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, transfer_link_rejected, created_at, updated_at) \
               VALUES (?1, ?2, 4, 'BTC', 1, NULL, '2026-09-02 10:00:00', '{\"venue\":\"answer\"}', '{}', 0, '2026-09-02 10:00:00', '2026-09-02 10:00:00')", [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    deltabadger::venue_ids::after_insert(c, &s3, c.last_insert_rowid(), Some("t3")).unwrap();
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    let tx = c.unchecked_transaction().unwrap();
    merge::run(&tx, &seed::cipher(), two.seeded.user_id, third.id.unwrap(), two.id(1), chrono::Utc::now()).unwrap().unwrap();
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
    let survivor = two.id(0).to_string();
    let form: &[(&str, &str)] = &[("survivor_id", &survivor)];
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, "2026-09-20 11:59:00");
    let refused = b.send(&app, "POST", &path, Some(form), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(refused.status, 422, "{}", &refused.body[..refused.body.len().min(600)]);
    assert!(refused.body.contains("disagree on these records") && refused.body.contains("2026-09-02 buy"), "{}", refused.body);
    c.execute("DELETE FROM transactions", []).unwrap();
    c.execute("DELETE FROM exchange_account_transactions", []).unwrap();
    let merged = b.send(&app, "POST", &path, Some(form), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(merged.status, 200, "{}", &merged.body[..merged.body.len().min(600)]);
    assert!(merged.body.contains("action=\"replace\" target=\"api_keys\"") && merged.body.contains("was merged into"), "{}", merged.body);
    assert_eq!(b.send(&app, "POST", &path, Some(form), common::web::Csrf::Header, &[]).await.status, 404, "no longer a duplicate");
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
    let survivor = alpaca.id(0).to_string();
    let form: &[(&str, &str)] = &[("survivor_id", &survivor)];
    common::two_accounts::fresh(c, alpaca.seeded.user_id, alpaca.seeded.exchange_id, "2026-09-20 11:59:00");
    let before = dump(c);
    let resting = b.send(&app, "POST", &path, Some(form), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(resting.status, 422, "{}", &resting.body[..resting.body.len().min(600)]);
    assert!(resting.body.contains("still in flight"), "{}", resting.body);
    assert_eq!(dump(c), before, "nothing changed");
    let merged = b.send(&app, "POST", &path, Some(form), common::web::Csrf::Header, &[("Accept", "text/vnd.turbo-stream.html")]).await;
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
                ("settings.accounts.merge_in_flight_html", &["duplicate", "survivor"][..]), ("settings.accounts.merge_conflict_html", &["duplicate", "survivor", "rows"][..]),
                ("settings.accounts.merge_withdrawal_key_html", &["duplicate", "survivor", "rules"][..]), ("settings.accounts.merge_foreign_order", &[][..]),
                ("settings.accounts.merge_order_unchecked_html", &["account", "rows"][..])];
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
    assert!(!overlaps(c, &two, 1), "account 1's copies were restored");
}

/// R4, R8: a component whose known fingerprints differ (#3 B, #2 unknown, #1 A) holds no duplicate.
#[test]
fn r4_a_chain_with_conflicting_fingerprints_is_no_duplicate() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let third = two.third(c).id.unwrap();
    two.fingerprint(c, 0, Some("A"));
    let key = add_key(c, &two, third);
    common::two_accounts::confirm(c, third, Some("B"));
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    trades_on(c, &two, third, key, 3);
    common::two_accounts::read_balances(c, two.seeded.user_id, third, key, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00");
    // R8: 1, 2 and 3 are one component, and its known fingerprints differ: none of them is a duplicate.
    assert!(!overlaps(c, &two, 1));
    assert!(standing(c, &two).is_empty(), "3 (B) and 1 (A) are linked through 2");
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
    c.execute("INSERT INTO import_rows (user_id, exchange_id, row_id, account_type, currency, side, amount, fee, transacted_at, remark, row_type, created_at, account_id) \
               VALUES (?1, ?2, 'r1', 'TRADE', 'BTC', 'in', '1', '0', '2026-09-01 10:00:00', '', 'fills', 'now', ?3)", [kucoin.seeded.user_id, kucoin.seeded.exchange_id, kucoin.id(1)]).unwrap();
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
    assert!(!overlaps(c, &two, 1));
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
    for n in 0..2 { common::two_accounts::confirm(&c, two.id(n), Some("fp")); }
    assert_eq!(sweep().await, Ok(true), "the history changed: the ledger walk records today again");
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

// ---- review round 3 ----

/// 1: a survivor whose current key reads another venue account than its recorded fingerprint is no survivor: no duplicate,
/// and Merge is a 404.
#[test]
fn r3_1_a_stale_fingerprint_makes_no_duplicate() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE exchange_account_credentials SET fingerprint = 'another-uid' WHERE api_key_id = ?1", [two.keys[0]]).unwrap();
    assert!(standing(c, &two).is_empty());
    assert!(merge_now(c, &two, two.id(1)).unwrap().is_none());
}

/// 2: of twins one restored (no venue answer) and one from the API, the row kept carries the API answer (its signed
/// `net_amount`).
#[test]
fn r3_2_the_kept_twin_carries_the_venue_answer() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let alpaca = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    alpaca.fingerprint(c, 0, Some("fp"));
    alpaca.fingerprint(c, 1, Some("fp"));
    let kept = alpaca.row(c, 0, 11, "USD", "10", Some("act-1"), None, "2026-09-01 10:00:00");
    let api = alpaca.row(c, 1, 11, "USD", "10", Some("act-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"activity_type\":\"INT\",\"net_amount\":\"-10\"}' WHERE id = ?1", [api]).unwrap();
    merge_now(c, &alpaca, alpaca.id(1)).unwrap().unwrap();
    let raw: String = c.query_row("SELECT raw_data FROM account_transactions WHERE id = ?1", [kept], |r| r.get(0)).unwrap();
    assert!(raw.contains("\"net_amount\":\"-10\""), "{raw}");
}

/// 3: a duplicate row with no exact-id twin (an id-less one too) refuses when the survivor holds a row of its entry type and
/// base currency within ten minutes: an Alpaca split leg beside an aggregate, a KuCoin CSV fill a second from its API
/// copy, an id-less Binance CSV deposit beside its API deposit.
#[test]
fn r3_3_a_row_the_others_reconciliation_would_match_refuses() {
    type Setup = fn(&Connection, &Two);
    let cases: [(&str, &str, Setup); 3] = [
        ("Exchanges::Alpaca", "Alpaca", |c, two| { two.row(c, 0, 15, "AAPL", "2", Some("leg-a"), None, "2026-09-01 00:00:00"); two.row(c, 1, 15, "AAPL", "1", Some("leg-b"), None, "2026-09-01 00:00:00"); }),
        (deltabadger::venue::kucoin::KUCOIN, "KuCoin", |c, two| {
            let api = two.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:01");
            c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1}' WHERE id = ?1", [api]).unwrap();
            two.row(c, 1, 0, "BTC", "1", Some("kucoin-csv-1"), None, "2026-09-01 10:00:00");
        }),
        ("Exchanges::Binance", "Binance", |c, two| { two.row(c, 0, 4, "BTC", "1", Some("dep-api"), None, "2026-09-01 10:05:00"); two.row(c, 1, 4, "BTC", "1", None, None, "2026-09-01 10:00:00"); }),
    ];
    for (kind, name, setup) in cases {
        let (_d, o) = install();
        let c = &o.primary;
        let kraken = Two::kraken(c, &seed::cipher());
        let two = Two::venue(c, &seed::cipher(), &kraken.seeded, kind, name);
        two.fingerprint(c, 0, Some("fp"));
        two.fingerprint(c, 1, Some("fp"));
        setup(c, &two);
        let before = dump(c);
        assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))), "{name}");
        assert_eq!(dump(c), before, "{name}");
    }
}

/// 5: a withheld KuCoin fill both accounts hold under one id with other facts refuses.
#[test]
fn r3_5_withheld_fills_that_differ_refuse() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    kucoin.fingerprint(c, 0, Some("fp"));
    kucoin.fingerprint(c, 1, Some("fp"));
    for (n, entry) in [(0, "{\"size\":\"1\"}"), (1, "{\"size\":\"2\"}")] {
        c.execute("INSERT INTO kucoin_withheld_fills (account_id, tx_id, transacted_at, entry, created_at) VALUES (?1, 'f-1', 'now', ?2, 'now')", rusqlite::params![kucoin.id(n), entry]).unwrap();
    }
    assert!(matches!(merge_now(c, &kucoin, kucoin.id(1)), Err(Refusal::Conflict(_))));
}

/// 7: a key the survivor takes over is re-read: its balance time is cleared, so a survivor movement after the survivor's
/// own last balance read (10:00 < 10:05 < the moved key's 10:10) is brought forward.
#[test]
fn r3_7_a_moved_keys_balance_time_is_read_again() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET key_type = 2, balances_synced_at = '2026-09-20 10:10:00' WHERE id = ?1", [two.keys[1]]).unwrap();
    c.execute("UPDATE api_keys SET balances_synced_at = '2026-09-20 10:00:00' WHERE id = ?1", [two.keys[0]]).unwrap();
    two.row(c, 0, 4, "BTC", "0.5", Some("dep-s"), None, "2026-09-20 10:05:00");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    // The survivor's own read as it was before the merge's fresh syncs.
    c.execute("UPDATE api_keys SET balances_synced_at = '2026-09-20 10:00:00' WHERE id = ?1", [two.keys[0]]).unwrap();
    let a = identity::AccountRow::of(c, &two.accounts[0]).unwrap().unwrap();
    let moved = deltabadger::tracker::figures::pending(c, two.seeded.user_id, &deltabadger::tracker::Scope::Account(a)).unwrap();
    assert_eq!(moved.into_iter().map(|(s, d)| (s, d.to_s_f())).collect::<Vec<_>>(), [("BTC".to_string(), "0.5".to_string())]);
}

/// 8: a venue read finished after its key moved to another account answers as for a missing account.
#[test]
fn r3_8_a_late_read_on_a_moved_key_is_account_not_found() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let version = deltabadger::engine::model::credential_version_by_id(c, two.keys[1]).unwrap().unwrap();
    move_key_to_one(c, two.keys[1]);
    use deltabadger::web::mcp::reads::{finish_read, Fetched};
    let done = deltabadger::web::bot_api::Read { res: deltabadger::web::bot_api::Res { status: deltabadger::web::bot_api::Domain::Success, data: serde_json::json!({}), code: None, message: String::new() }, text: "orders".into(), is_error: false };
    let read = finish_read(c, Fetched::Bound(Box::new(Fetched::Done(done)), vec![version])).unwrap();
    assert!(read.is_error && read.text.contains("Account not found"), "{}", read.text);
}

/// 9: a duplicate key's later balance time does not make All's prices stale.
#[test]
fn r3_9_a_duplicate_keys_sync_time_makes_no_staleness() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    hold(c, &two, 0, "1", "100");
    c.execute("UPDATE api_keys SET balances_synced_at = '2026-09-20 10:30:00' WHERE id = ?1", [two.keys[1]]).unwrap();
    assert!(!deltabadger::tracker::figures::page::cold(c, two.seeded.user_id, &deltabadger::tracker::Scope::All, true).unwrap().stale);
}

/// 10: a standing change that leaves nothing to sweep answers that the history changed, so the job wakes today's writer.
#[tokio::test(flavor = "current_thread")]
async fn r3_10_a_standing_change_with_nothing_to_sweep_wakes_todays_writer() {
    let (dir, o) = install();
    let two = Two::kraken(&o.primary, &seed::cipher());
    let user = two.seeded.user_id;
    two.row(&o.primary, 1, 4, "USD", "100", Some("dep-d"), None, "2026-01-10 10:00:00");
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let db = deltabadger::jobs::Db::new(o.primary, seed::cipher());
    let clock = deltabadger::engine::FixedClock("2026-09-20T03:00:00Z".parse().unwrap());
    let sweep = || deltabadger::tracker::jobs::backfill_run(&db, &NoVenue, None::<&deltabadger::jobs::market::Feed<deltabadger::venue::http::ScriptedTransport>>, None, user, &clock);
    assert_eq!(sweep().await, Ok(true));
    for n in 0..2 { common::two_accounts::confirm(&c, two.id(n), Some("fp")); }
    assert_eq!(sweep().await, Ok(true), "the history changed: the job wakes the ledger walk that records today");
}

// ---- review round 4 ----

/// A venue's answer, as a sync stores it with the row.
fn synced(c: &Connection, row: i64) { c.execute("UPDATE account_transactions SET raw_data = json_object('id', ?1, 'venue', 'answer') WHERE id = ?1", [row]).unwrap(); }
/// A venue's own CSV export's row (Binance's), as its import stores it.
fn from_csv(c: &Connection, row: i64, operation: &str) { c.execute("UPDATE account_transactions SET raw_data = json_object('operation', ?2) WHERE id = ?1", rusqlite::params![row, operation]).unwrap(); }
/// A duplicate on Binance.
fn binance() -> (tempfile::TempDir, store::Opened, Two) {
    let (d, o) = install();
    let kraken = Two::kraken(&o.primary, &seed::cipher());
    let two = Two::venue(&o.primary, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    two.fingerprint(&o.primary, 0, Some("fp"));
    two.fingerprint(&o.primary, 1, Some("fp"));
    identity::reconsider(&o.primary, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).unwrap();
    (d, o, two)
}
fn refuses(c: &Connection, two: &Two, what: &str) {
    let before = dump(c);
    assert!(matches!(merge_now(c, two, two.id(1)), Err(Refusal::Conflict(_))), "{what}");
    assert_eq!(dump(c), before, "{what}");
}

/// (a) A row of the duplicate no venue sync wrote, with no exact-id twin, refuses.
#[test]
fn r4_a_a_csv_row_unique_to_the_duplicate_refuses() {
    let (_d, o, two) = binance();
    let c = &o.primary;
    let row = two.row(c, 1, 4, "BTC", "1", None, None, "2026-09-01 10:00:00");
    from_csv(c, row, "Deposit");
    refuses(c, &two, "a CSV deposit only the duplicate holds");
}

/// (a) A row of the duplicate a venue sync wrote, unique to it and with no non-API neighbour, moves: the main case, the
/// duplicate synced after the survivor lost its key.
#[test]
fn r4_a_an_api_row_unique_to_the_duplicate_moves() {
    let (_d, o, two) = binance();
    let c = &o.primary;
    let row = two.row(c, 1, 0, "BTC", "1", Some("trade-only-2"), None, "2026-09-01 10:00:00");
    synced(c, row);
    let s = two.row(c, 0, 0, "BTC", "1", Some("trade-on-1"), None, "2026-09-01 11:00:00");
    synced(c, s);
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert!(c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE id = ?1 AND tx_id = 'trade-only-2')", [row], |r| r.get::<_, bool>(0)).unwrap(), "moved to account 1");
}

/// (b) An API row of the duplicate beside a survivor row no venue sync wrote, of its asset within 24 hours, of any type,
/// refuses: a Binance withdrawal requested at 13:08:10 and completed at 13:20:44; an Earn staking reward against the CSV's
/// lending interest.
#[test]
fn r4_b_a_survivor_csv_row_of_the_asset_within_a_day_refuses() {
    let (_d, o, two) = binance();
    let c = &o.primary;
    let api = two.row(c, 1, 5, "BTC", "1", Some("wd-api"), None, "2026-09-01 13:08:10");
    synced(c, api);
    let csv = two.row(c, 0, 5, "BTC", "1", None, None, "2026-09-01 13:20:44");
    from_csv(c, csv, "Withdraw");
    refuses(c, &two, "the withdrawal at request and at completion");
    let (_d, o, two) = binance();
    let c = &o.primary;
    let api = two.row(c, 1, 11, "USDT", "0.5", Some("earn-1"), None, "2026-09-01 00:00:00");
    synced(c, api);
    let csv = two.row(c, 0, 8, "USDT", "0.5", None, None, "2026-09-01 02:00:00");
    from_csv(c, csv, "Simple Earn Flexible Interest");
    refuses(c, &two, "a staking reward against the CSV's lending interest");
}

/// (c) Two API rows of one type and asset are compared in whole seconds: exactly ten minutes apart refuses.
#[test]
fn r4_c_the_api_window_is_exact_in_seconds() {
    let (_d, o, two) = binance();
    let c = &o.primary;
    let d = two.row(c, 1, 4, "BTC", "1", Some("dep-1"), None, "2026-09-01 00:00:00");
    let s = two.row(c, 0, 4, "BTC", "1", Some("dep-0"), None, "2026-09-01 00:10:00");
    synced(c, d);
    synced(c, s);
    refuses(c, &two, "600 seconds apart");
}

/// 1: overlap evidence holds only while the keys that produced it are the accounts' keys at the same versions: account 1's
/// trading key replaced, the pair is no duplicate and Merge is a 404.
#[test]
fn r4_1_overlap_evidence_dies_with_its_keys() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    let cipher = seed::cipher();
    identity::write_credential(c, &cipher, two.keys[0], Some(&cipher.encrypt("another-kraken-account")), Some(&cipher.encrypt("s")), None, chrono::Utc::now()).unwrap();
    assert!(standing(c, &two).is_empty());
    assert!(merge_now(c, &two, two.id(1)).unwrap().is_none());
}

/// 4: the kept twin takes the duplicate's asset when it has none, and its group when it has none and no survivor row uses
/// it; a group a survivor row already uses refuses.
#[test]
fn r4_4_the_kept_twin_takes_the_duplicates_asset_and_group() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let kept = two.row(c, 0, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 0, "BTC", "1", Some("t-1"), Some("usdt-leg"), "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET base_asset_id = ?2 WHERE id = ?1", [d, two.seeded.btc]).unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let (asset, group): (Option<i64>, Option<String>) = c.query_row("SELECT base_asset_id, group_id FROM account_transactions WHERE id = ?1", [kept], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((asset, group.as_deref()), (Some(two.seeded.btc), Some("usdt-leg")));

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    two.row(c, 0, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    two.row(c, 0, 1, "ETH", "1", Some("other"), Some("usdt-leg"), "2026-08-01 10:00:00");
    two.row(c, 1, 0, "BTC", "1", Some("t-1"), Some("usdt-leg"), "2026-09-01 10:00:00");
    refuses(c, &two, "a group a survivor row already uses");
}

/// 5, R18: classification reads a duplicate's rows too: its BTC equity beside the survivor's BTC coin is ambiguous.
#[test]
fn r4_5_a_duplicates_rows_decide_no_asset_class() {
    // R18: classification reads every row, a duplicate's included (the same venue account): its BTC equity beside the
    // survivor's BTC coin makes the name ambiguous, as on main.
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('BTC.US', 'BTC', 'BTC equity', 'Stock', 'now', 'now')", []).unwrap();
    let equity = c.last_insert_rowid();
    let s = two.row(c, 0, 4, "BTC", "1", Some("s-1"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 4, "BTC", "1", Some("d-1"), None, "2026-09-02 10:00:00");
    c.execute("UPDATE account_transactions SET base_asset_id = CASE id WHEN ?1 THEN ?3 ELSE ?4 END WHERE id IN (?1, ?2)", [s, d, two.seeded.btc, equity]).unwrap();
    let classes = deltabadger::figures::db::account_classes(c, two.seeded.user_id, &["BTC".to_string()]).unwrap();
    assert!(classes.contains(&("BTC".to_string(), Some("Stock".to_string()))), "{classes:?}");
}

// ---- review round 5: the standing read fresh from several signals ----

/// 1+2+3: shared trades, each account's balances read by its reading key within 15 minutes and equal, every key taking part:
/// a duplicate. Balances that differ at comparable reads prove two accounts; reads too far apart wait.
#[test]
fn s5_the_standing_needs_trades_balances_and_every_key() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let user = two.seeded.user_id;
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    let read = |n: usize, units: &str, at: &str| common::two_accounts::read_balances(c, user, two.id(n), two.keys[n], &[(two.seeded.btc, units)], at);
    read(0, "1", "2026-09-20 10:00:00");
    read(1, "1", "2026-09-20 10:14:00");
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "14 minutes apart, equal");
    read(1, "1.5", "2026-09-20 10:05:00");
    assert!(standing(c, &two).is_empty(), "different at comparable reads");
    read(1, "1", "2026-09-20 10:16:00");
    assert!(standing(c, &two).is_empty(), "16 minutes apart: not yet");
}

/// The wallet behind account 2's key switched before its first complete sync: its rows were imported by an older version.
#[test]
fn s5_a_wallet_switched_before_the_duplicates_sync_is_no_duplicate() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    let cipher = seed::cipher();
    identity::write_credential(c, &cipher, two.keys[1], Some(&cipher.encrypt("another-wallet")), Some(&cipher.encrypt("s")), None, chrono::Utc::now()).unwrap();
    assert!(standing(c, &two).is_empty());
}

/// A trading key for another wallet added to account 1 takes no part in the shared trades: no duplicate.
#[test]
fn s5_a_key_that_took_no_part_ends_the_standing() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    add_key(c, &two, two.id(0));
    common::two_accounts::stamp_ledgers(c, two.seeded.user_id);
    assert!(standing(c, &two).is_empty());
}

/// Identical credentials saved again (re-encrypted) are no key event: the stored standing stays.
#[test]
fn s5_a_resaved_key_waits_for_its_next_sync() {
    let (_d, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    let secret: String = c.query_row("SELECT secret FROM api_keys WHERE id = ?1", [two.keys[0]], |r| r.get(0)).unwrap();
    assert!(!identity::write_credential(c, &cipher, two.keys[0], Some(&cipher.encrypt("test-key")), Some(&secret), None, chrono::Utc::now()).unwrap());
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "not comparable until the key syncs again: it stays");
    common::two_accounts::restamp(c, two.seeded.user_id, two.keys[0]);
    assert!(overlaps(c, &two, 1), "and on the next matching sync");
}

/// An account id the venue reports (equal fingerprints) is one signal; balances that differ at comparable reads contradict it.
#[test]
fn s5_fingerprints_need_balances_that_do_not_disagree() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(0), two.keys[0], &[(two.seeded.btc, "1")], "2026-09-20 10:00:00");
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "2")], "2026-09-20 10:01:00");
    assert!(standing(c, &two).is_empty());
}

/// A Deltabadger CSV restore on KuCoin (its amounts kept with it) is no venue sync's row: an id-less restored withdrawal an
/// hour from the survivor's API copy refuses.
#[test]
fn s5_a_kucoin_restore_with_amounts_is_not_synced() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let kucoin = Two::venue(c, &seed::cipher(), &kraken.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    kucoin.fingerprint(c, 0, Some("fp"));
    kucoin.fingerprint(c, 1, Some("fp"));
    let api = kucoin.row(c, 0, 5, "BTC", "1", Some("wd-api"), None, "2026-09-01 11:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1}' WHERE id = ?1", [api]).unwrap();
    let restored = kucoin.row(c, 1, 5, "BTC", "1", None, None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"amounts\":{\"base\":\"1\"}}' WHERE id = ?1", [restored]).unwrap();
    refuses(c, &kucoin, "a restored withdrawal");
}

/// Exact-id twins with two different assets, or two different descriptions, refuse; a description only the duplicate's twin
/// has is kept.
#[test]
fn s5_twins_keep_their_identity() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('BTC.US', 'BTC', 'BTC equity', 'Stock', 'now', 'now')", []).unwrap();
    let equity = c.last_insert_rowid();
    let s = two.row(c, 0, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET base_asset_id = CASE id WHEN ?1 THEN ?3 ELSE ?4 END WHERE id IN (?1, ?2)", [s, d, two.seeded.btc, equity]).unwrap();
    refuses(c, &two, "two assets");

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let s = two.row(c, 0, 8, "ETHW", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 8, "ETHW", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET description = CASE id WHEN ?1 THEN 'Airdrop' ELSE 'Fork (ETHW)' END WHERE id IN (?1, ?2)", [s, d]).unwrap();
    refuses(c, &two, "two descriptions");

    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let s = two.row(c, 0, 8, "ETHW", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 8, "ETHW", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET description = 'Fork (ETHW)' WHERE id = ?1", [d]).unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let kept: Option<String> = c.query_row("SELECT description FROM account_transactions WHERE id = ?1", [s], |r| r.get(0)).unwrap();
    assert_eq!(kept.as_deref(), Some("Fork (ETHW)"));
}

// ---- review round 6 ----

fn now_text(offset_secs: i64) -> String { (chrono::Utc::now() + chrono::Duration::seconds(offset_secs)).format("%Y-%m-%d %H:%M:%S").to_string() }
/// An unresolved Kraken code a balance read of key `key` recorded (`rust_sync.kraken_unresolved:<key>`).
fn unresolved(c: &Connection, key: i64, codes: &[&str]) {
    deltabadger::app_config::set_plain(c, &format!("rust_sync.kraken_unresolved:{key}"), &serde_json::json!({"codes": codes, "at": "now"}).to_string(), chrono::Utc::now()).unwrap();
}

/// Live check: data older than fifteen minutes is not enough to merge on; the refusal asks to try again (and wakes the syncs).
#[test]
fn l6_stale_data_refuses_to_try_again() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-3600));
    let before = dump(c);
    assert!(!matches!(merge_stale(c, &two, two.id(1)), Ok(Some(_))), "an hour-old sync");
    assert_eq!(dump(c), before);
}

/// Live check: fresh agreeing data merges.
#[test]
fn l6_fresh_agreeing_data_merges() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    assert!(merge_stale(c, &two, two.id(1)).unwrap().is_some());
}

/// Live check and standing: fresh balances that differ only in an unresolved asset are two accounts.
#[test]
fn l6_an_unresolved_asset_difference_is_no_duplicate() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    unresolved(c, two.keys[1], &["XTOKEN"]);
    assert!(standing(c, &two).is_empty());
    assert!(!matches!(merge_stale(c, &two, two.id(1)), Ok(Some(_))));
}

/// Live check: a key replaced with no fresh sync since is not enough to merge on.
#[test]
fn l6_a_replaced_key_with_no_fresh_sync_refuses() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    c.execute("UPDATE api_keys SET key = ?2 WHERE id = ?1", rusqlite::params![two.keys[0], seed::cipher().encrypt("test-key")]).unwrap();
    assert!(!matches!(merge_stale(c, &two, two.id(1)), Ok(Some(_))));
}

/// A withdrawal key of account `n`, observed as the account's fingerprint like its other keys.
fn withdrawal_key(c: &Connection, two: &Two, n: usize) -> i64 {
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, 'w', 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
              rusqlite::params![two.seeded.user_id, two.seeded.exchange_id, format!("w-{n}")]).unwrap();
    let key = c.last_insert_rowid();
    two.accounts[n].add_key(c, key).unwrap();
    two.fingerprint(c, n, Some("fp"));
    key
}
/// R7 #1: Merge refuses when a live rule of either account would withdraw with another key afterwards (none to some
/// included), naming the rule; with no rule a key would move under, it merges.
#[test]
fn l6_withdrawal_keys_that_would_change_hands_refuse() {
    let refused = |c: &Connection, two: &Two, what: &str| {
        let before = dump(c);
        match merge_now(c, two, two.id(1)) { Err(Refusal::WithdrawalKeys(rules)) => assert!(rules.iter().any(|r| r.contains("BTC")), "{what}: {rules:?}"), other => panic!("{what}: {other:?}") }
        assert_eq!(dump(c), before, "{what}");
    };
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    withdrawal_key(c, &two, 0);
    two.rule(c, &seed::cipher(), 1, two.seeded.btc, 0);
    refused(c, &two, "the duplicate's rule would take the survivor's key");
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    withdrawal_key(c, &two, 1);
    two.rule(c, &seed::cipher(), 0, two.seeded.btc, 0);
    refused(c, &two, "the survivor's rule would take the duplicate's key");
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    withdrawal_key(c, &two, 0);
    withdrawal_key(c, &two, 1);
    two.rule(c, &seed::cipher(), 1, two.seeded.btc, 0);
    refused(c, &two, "the duplicate's rule would switch keys");
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let key = withdrawal_key(c, &two, 1);
    two.rule(c, &seed::cipher(), 1, two.seeded.btc, 0);
    assert!(merge_now(c, &two, two.id(1)).unwrap().is_some(), "the duplicate's rule keeps its key");
    assert_eq!(identity::account_of_key(c, key).unwrap(), identity::Account::first(c, two.seeded.user_id, two.seeded.exchange_id).unwrap().id);
}

/// #4: a pair the direct comparison rejects is never produced through a chain.
#[test]
fn r6_a_chain_never_overrides_a_direct_veto() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let third = two.third(c).id.unwrap();
    let key = add_key(c, &two, third);
    common::two_accounts::confirm(c, third, Some("fp"));
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(0), two.keys[0], &[(two.seeded.btc, "1")], "2026-09-20 10:00:00");
    common::two_accounts::read_balances(c, two.seeded.user_id, third, key, &[(two.seeded.btc, "2")], "2026-09-20 10:01:00");
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "1")], "2026-09-19 10:00:00");
    // 3 confirmed the fingerprint and entered beside 2 as 1's duplicate; the 3-1 contradiction falls inside that stored
    // group, which leaves whole.
    assert!(standing(c, &two).is_empty());
}

/// #7: a conflicting shared id anywhere in the history vetoes, not only in the sample; Coinbase's `-in`/`-out` legs of one
/// trade are one match.
#[test]
fn r6_the_whole_history_vetoes_and_legs_count_once() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    for i in 0..200 { two.trade(c, 1, 0, &format!("only-{i}"), "1", "2026-09-10 10:00:00"); }
    two.trade(c, 0, 0, "late", "1", "2026-09-11 10:00:00");
    two.trade(c, 1, 0, "late", "2", "2026-09-11 10:00:00");
    assert!(!overlaps(c, &two, 1), "a conflict at row 204");

    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let coinbase = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Coinbase", "Coinbase");
    for k in 0..2 { for t in ["s1", "s2"] { for leg in ["in", "out"] { coinbase.trade(c, k, 0, &format!("{t}-{leg}"), "1", "2026-09-01 10:00:00"); } } }
    assert!(!overlaps(c, &coinbase, 1), "two swaps are two matches, not four");
}

/// #6: refreshing one account a day later keeps the standing its last comparable pair of reads gave, and asks for the
/// partner's read.
#[test]
fn r6_one_accounts_refresh_keeps_the_last_comparable_verdict() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    // The balance job's completion keeps the comparable verdict.
    let now = chrono::Utc::now();
    assert!(identity::after_balance_read(c, two.keys[1], now).unwrap().is_empty());
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "1")], "2026-09-21 10:00:00");
    assert_eq!(identity::after_balance_read(c, two.keys[1], now).unwrap(), [two.keys[0]], "the partner is read now");
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "the day-old pair still stands");
}

/// R7 #3: a replaced key's rows from before its replacement never count, whatever its new credential reads: equal
/// balances, empty or not, with no trade newer than the change are no duplicate.
#[test]
fn r7_a_replaced_keys_old_rows_never_count() {
    let (_d, o) = install();
    let c = &o.primary;
    let cipher = seed::cipher();
    let two = Two::kraken(c, &cipher);
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    identity::write_credential(c, &cipher, two.keys[0], Some(&cipher.encrypt("another-wallet")), Some(&cipher.encrypt("s")), None, chrono::Utc::now()).unwrap();
    deltabadger::app_config::set_plain(c, &format!("rust_key_credential_changed:{}", two.keys[0]), "2026-09-15 00:00:00", chrono::Utc::now()).unwrap();
    common::two_accounts::restamp(c, two.seeded.user_id, two.keys[0]);
    for k in 0..2 { common::two_accounts::read_balances(c, two.seeded.user_id, two.id(k), two.keys[k], &[], "2026-09-20 10:00:00"); }
    assert!(standing(c, &two).is_empty(), "equal empty reads, no newer trade");
    assert!(!overlaps(c, &two, 1), "equal non-empty reads, no newer trade");
}

/// #2: a key whose identity read is masked beside a key that confirmed the fingerprint is no confirmation.
#[test]
fn r6_a_masked_key_beside_a_confirming_one_is_no_confirmation() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let masked = add_key(c, &two, two.id(0));
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, fingerprint, last_failure_code) VALUES (?1, ?2, NULL, 'http 200')", [masked, two.seeded.exchange_id]).unwrap();
    assert!(standing(c, &two).is_empty());
}

/// #10: a KuCoin restore (`amounts`, `amounts_inexact`) is not synced; an exact-id twin differing from its API copy only
/// in those restore fields is no conflict.
#[test]
fn r6_the_kucoin_restore_shape() {
    let kucoin = |c: &Connection| { let k = Two::kraken(c, &seed::cipher()); let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
                                     two.fingerprint(c, 0, Some("fp")); two.fingerprint(c, 1, Some("fp")); two };
    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin(c);
    let row = two.row(c, 1, 5, "BTC", "1", None, None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"amounts\":{\"base\":\"1\"},\"amounts_inexact\":false}' WHERE id = ?1", [row]).unwrap();
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))), "a restore only the duplicate holds");
    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin(c);
    let api = two.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1}' WHERE id = ?1", [api]).unwrap();
    let restore = two.row(c, 1, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"amounts\":{\"base\":\"1\"}}' WHERE id = ?1", [restore]).unwrap();
    assert!(merge_now(c, &two, two.id(1)).unwrap().is_some(), "the twins are one fill");
}

/// #9: the confirm posts the survivor; one that is no longer the derived survivor is a 404, nothing changed.
#[tokio::test(flavor = "current_thread")]
async fn r6_the_posted_survivor_must_be_the_derived_one() {
    let (dir, o, two) = duplicate();
    let c = &o.primary;
    let third = two.third(c).id.unwrap();
    let app = web_app(dir.path(), c);
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, "2026-09-20 11:59:00");
    let mut b = on_settings(&app, c, two.seeded.user_id).await;
    let before = dump(c);
    let answer = b.send(&app, "POST", &format!("/settings/merge_account/{}", two.id(1)), Some(&[("survivor_id", &third.to_string())]), common::web::Csrf::Header,
                        &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(answer.status, 404, "{}", &answer.body[..answer.body.len().min(400)]);
    assert_eq!(dump(c), before);
}

/// An account no key can read the balances of: no sync can make it fresh, so Merge refuses naming it, never "try again".
#[test]
fn l6_an_account_no_key_can_read_refuses_naming_it() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two.keys[0]]).unwrap();
    match merge_stale(c, &two, two.id(1)) { Err(Refusal::Unreadable(a)) => assert_eq!(a, two.id(0)), other => panic!("{other:?}") }
}

// ---- review round 7 ----

/// R7 #2: every credential write goes through `identity::write_credential`: the same material re-encrypted records no
/// change; other material records it, resets the key's ledger watermark and sync state, and clears its identity
/// observation (of the old material).
#[test]
fn r7_the_one_credential_write_records_a_real_change() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let cipher = seed::cipher();
    let key = two.keys[0];
    let changed = format!("rust_key_credential_changed:{key}");
    let material: (String, String) = c.query_row("SELECT key, secret FROM api_keys WHERE id = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let (k, s) = (cipher.decrypt(&material.0).unwrap(), cipher.decrypt(&material.1).unwrap());
    c.execute("UPDATE api_keys SET last_synced_at = '2026-09-01 00:00:00' WHERE id = ?1", [key]).unwrap();
    assert!(!identity::write_credential(c, &cipher, key, Some(&cipher.encrypt(&k)), Some(&cipher.encrypt(&s)), None, chrono::Utc::now()).unwrap(), "the same material");
    assert!(!deltabadger::app_config::exists(c, &changed).unwrap());
    assert!(one::<Option<String>>(c, &format!("SELECT last_synced_at FROM api_keys WHERE id = {key}")).is_some());
    assert!(identity::write_credential(c, &cipher, key, Some(&cipher.encrypt("another-wallet")), Some(&cipher.encrypt(&s)), None, chrono::Utc::now()).unwrap());
    assert!(deltabadger::app_config::exists(c, &changed).unwrap());
    assert!(one::<Option<String>>(c, &format!("SELECT last_synced_at FROM api_keys WHERE id = {key}")).is_none());
    assert!(one::<Option<String>>(c, &format!("SELECT fingerprint FROM exchange_account_credentials WHERE api_key_id = {key}")).is_none(), "the old material's observation");
    assert_eq!(key_material(c, &cipher, key), "another-wallet");
}

/// R7 #2: the JSON key save (POST /api/api_keys) replacing a key's material records the change as the Settings form does:
/// its rows from before never count.
#[tokio::test(flavor = "current_thread")]
async fn r7_the_json_key_save_records_a_replacement() {
    let (dir, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    let app = web_app(dir.path(), c);
    let mut b = on_settings(&app, c, two.seeded.user_id).await;
    let saved = json_save(&app, &mut b, &two.seeded.exchange_id.to_string(), Some(&two.id(0).to_string())).await;
    assert_eq!(saved.status, 201, "{}", saved.body);
    assert!(deltabadger::app_config::exists(c, &format!("rust_key_credential_changed:{}", two.keys[0])).unwrap());
    c.execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [two.keys[0]]).unwrap();
    common::two_accounts::restamp(c, two.seeded.user_id, two.keys[0]);
    assert!(!overlaps(c, &two, 1), "the old wallet's trades are no evidence of the new one");
}

/// R7 #4: fingerprint standing needs every live key of both accounts to confirm it: a key with no observation gives none.
#[test]
fn r7_an_unobserved_live_key_gives_no_fingerprint_standing() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))]);
    add_key(c, &two, two.id(0));
    assert!(standing(c, &two).is_empty());
}

/// R7 #5: the kept verdict survives a refresh under the same credential and key set (its first batch incomplete); a key
/// added to either account drops it.
#[test]
fn r7_a_refresh_keeps_the_verdict_until_the_keys_change() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(overlaps(c, &two, 1));
    assert!(identity::after_balance_read(c, two.keys[1], chrono::Utc::now()).unwrap().is_empty());
    // The refresh's first batch: incomplete, a day later, the same version.
    let a = identity::AccountRow::by_id(c, two.id(1)).unwrap().unwrap();
    let origin = deltabadger::sync::cache::balance_origin_key_of(&a);
    let mut stamp: serde_json::Value = serde_json::from_str(&deltabadger::app_config::get_plain(c, &origin).unwrap().unwrap()).unwrap();
    stamp["complete"] = serde_json::Value::Bool(false);
    deltabadger::app_config::set_plain(c, &origin, &stamp.to_string(), chrono::Utc::now()).unwrap();
    c.execute("UPDATE api_keys SET balances_synced_at = '2026-09-21 10:00:00' WHERE id = ?1", [two.keys[1]]).unwrap();
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "mid-refresh");
    withdrawal_key(c, &two, 0);
    assert!(standing(c, &two).is_empty(), "another key set");
}

/// A buy of account `account` imported by its key `key` under venue id `id`: `amount` BTC for 100 EUR at `at`.
fn trade_on(c: &Connection, two: &Two, account: i64, key: i64, id: &str, amount: &str, at: &str) {
    let scope = deltabadger::venue_ids::Scope::of(c, &identity::Account::by_id(c, account).unwrap().unwrap()).unwrap();
    c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at, \
               raw_data, manual_values, transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 'BTC', ?7, 'EUR', '100', ?4, ?5, json_object('refid', ?6), '{}', 0, ?5, ?5)",
              rusqlite::params![two.seeded.user_id, key, two.seeded.exchange_id, scope.stored(Some(id)), at, id, amount]).unwrap();
    deltabadger::venue_ids::after_insert(c, &scope, c.last_insert_rowid(), Some(id)).unwrap();
}

/// R7 #6, R8: a shared trade id with other facts between 3 and 1 vetoes the component 3-2-1 whole.
#[test]
fn r7_a_direct_trade_conflict_vetoes_every_chain() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let third = two.third(c).id.unwrap();
    let key = add_key(c, &two, third);
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    trades_on(c, &two, third, key, 3);
    trade_on(c, &two, two.id(0), two.keys[0], "x", "1", "2026-09-05 10:00:00");
    trade_on(c, &two, third, key, "x", "2", "2026-09-05 10:00:00");
    for (a, k) in [(two.id(0), two.keys[0]), (two.id(1), two.keys[1]), (third, key)] {
        common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00");
    }
    assert!(standing(c, &two).is_empty());
}

/// R7 #7: Kraken stores only the codes it could not name: while either account holds one, Merge refuses naming it.
#[test]
fn r7_an_unresolved_code_refuses_the_merge_naming_it() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    for k in two.keys { unresolved(c, k, &["XTOKEN"]); }
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "the same codes");
    let before = dump(c);
    match merge_stale(c, &two, two.id(1)) { Err(Refusal::Conflict(rows)) => assert!(rows.iter().any(|r| r.contains("XTOKEN")), "{rows:?}"), other => panic!("{other:?}") }
    assert_eq!(dump(c), before);
}

/// R7 #8: a venue answer beats a restore-shaped raw: the survivor's restored twin takes the duplicate's API answer.
#[test]
fn r7_the_kept_twin_takes_the_venue_answer_over_a_restore() {
    let (_d, o) = install();
    let c = &o.primary;
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let kept = two.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"amounts\":{\"base\":\"1\"}}' WHERE id = ?1", [kept]).unwrap();
    let api = two.row(c, 1, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1}' WHERE id = ?1", [api]).unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(one::<String>(c, &format!("SELECT raw_data FROM account_transactions WHERE id = {kept}")), "{\"createdAt\":1}");
}

/// R7 #10: an order of another user under the same venue id is named generically: no date, side, amount or asset.
#[test]
fn r7_another_users_order_is_named_generically() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    two.order(c, 0, common::two_accounts::SHARED_ORDER, 0, Some(2));
    two.order(c, 1, common::two_accounts::SHARED_ORDER, 0, Some(2));
    c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('other@example.com', 'x', 0, '2026-01-01', '2026-01-01')", []).unwrap();
    c.execute("UPDATE bots SET user_id = ?2 WHERE id = ?1", [two.bots[0], c.last_insert_rowid()]).unwrap();
    match merge_now(c, &two, two.id(1)) {
        Err(Refusal::Conflict(rows)) => {
            assert_eq!(rows.len(), 2, "{rows:?}");
            assert!(rows.contains(&merge::FOREIGN_ORDER.to_string()), "{rows:?}");
        }
        other => panic!("{other:?}"),
    }
}

/// R7 #9: on a single-account install, the read-only key's balance job giving way to the trading key ends with an outcome
/// the scheduler records: it runs once, never woken again at once.
#[tokio::test(flavor = "current_thread")]
async fn r7_a_balance_job_giving_way_runs_once() {
    let (dir, o, s) = common::install();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET status = 1 WHERE id = ?1", [s.api_key_id]).unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 2, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, s.exchange_id, seed::cipher().encrypt("read-key"), seed::cipher().encrypt("read-secret")]).unwrap();
    let reader = c.last_insert_rowid();
    common::number_one(c, s.user_id, s.exchange_id);
    assert!(!deltabadger::sync::balance_reading_keys(c).unwrap().contains(&reader), "the trading key reads");
    let runs = std::rc::Rc::new(std::cell::Cell::new(0));
    let counted = runs.clone();
    let scheduler = deltabadger::jobs::Scheduler::new(store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary, seed::cipher(),
        vec![Box::new(deltabadger::sync::jobs::BalanceSync::new(NoVenue, std::rc::Rc::new(deltabadger::sync::balances::NoPrices), reader))], None)
        .with_resolver(Box::new(|name, scope| Ok(Box::new(Ran(Box::leak(name.to_string().into_boxed_str()), scope.map(str::to_string))) as Box<dyn deltabadger::jobs::Job>)))
        .with_ran(Box::new(move |name, scope| if name == deltabadger::sync::jobs::BALANCE_SYNC && scope == Some(reader.to_string().as_str()) { counted.set(counted.get() + 1) }));
    scheduler.wakers().wake(deltabadger::sync::jobs::BALANCE_SYNC, Some(&reader.to_string()), None);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let clock = deltabadger::engine::FixedClock(chrono::Utc::now());
    let (ended, ()) = tokio::join!(scheduler.run(stopped, &clock), async {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let _ = stop.send(true);
    });
    ended.unwrap();
    assert_eq!(runs.get(), 1);
    let state = deltabadger::jobs::state::read(c, deltabadger::sync::jobs::BALANCE_SYNC, Some(&reader.to_string())).unwrap();
    assert!(state.last_run_at.is_some() && state.last_error.is_none(), "{state:?}");
}

// ---- review round 8 ----

fn set_config(c: &Connection, key: &str, value: &str) { deltabadger::app_config::set_plain(c, key, value, chrono::Utc::now()).unwrap(); }

/// Account number `number` of the exchange with a trading key of its own.
fn account_numbered(c: &Connection, two: &Two, number: i64) -> (i64, i64) {
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, ?3, 'now', 'now')",
              [two.seeded.user_id, two.seeded.exchange_id, number]).unwrap();
    let account = c.last_insert_rowid();
    (account, add_key(c, two, account))
}

/// R8 #4: duplicates are whole components of accepted pairs, each member a duplicate of the lowest number: 4-2, 4-3 and
/// 3-1 make 2, 3 and 4 duplicates of 1.
#[test]
fn r8_a_component_resolves_to_its_lowest_number() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (a1, k1, a2, k2) = (two.id(0), two.keys[0], two.id(1), two.keys[1]);
    let (a3, k3) = account_numbered(c, &two, 3);
    let (a4, k4) = account_numbered(c, &two, 4);
    for (set, pairs) in [("a", [(a4, k4), (a2, k2)]), ("b", [(a4, k4), (a3, k3)]), ("c", [(a3, k3), (a1, k1)])] {
        for i in 0..3 { for (a, k) in pairs { trade_on(c, &two, a, k, &format!("{set}{i}"), "1", &format!("2026-09-0{} 1{i}:00:00", 1 + set.len())); } }
    }
    for (a, k) in [(a1, k1), (a2, k2), (a3, k3), (a4, k4)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert_eq!(standing(c, &two), [(a2, a1), (a3, a1), (a4, a1)]);
}

/// R8 #4: 2-1 and 3-1 accepted, 2 and 3 contradicting each other (a shared id with other facts): no duplicate at all.
#[test]
fn r8_a_veto_inside_a_component_clears_it() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (a1, k1, a2, k2) = (two.id(0), two.keys[0], two.id(1), two.keys[1]);
    let (a3, k3) = account_numbered(c, &two, 3);
    for i in 0..3 {
        for (a, k) in [(a2, k2), (a1, k1)] { trade_on(c, &two, a, k, &format!("c{i}"), "1", &format!("2026-09-02 1{i}:00:00")); }
        for (a, k) in [(a3, k3), (a1, k1)] { trade_on(c, &two, a, k, &format!("d{i}"), "1", &format!("2026-09-03 1{i}:00:00")); }
    }
    trade_on(c, &two, a2, k2, "x", "1", "2026-09-04 10:00:00");
    trade_on(c, &two, a3, k3, "x", "2", "2026-09-04 10:00:00");
    for (a, k) in [(a1, k1), (a2, k2), (a3, k3)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert!(standing(c, &two).is_empty());
}

/// R8 #5: a balance read that skipped a position (Alpaca: unmapped or ambiguous) is not complete: Merge refuses naming it.
#[test]
fn r8_a_read_that_skipped_a_position_refuses_naming_it() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &now_text(-60));
    set_config(c, &format!("rust_sync.skipped:{}", two.keys[1]), r#"{"codes":["XYZ"],"at":"now"}"#);
    let before = dump(c);
    match merge_stale(c, &two, two.id(1)) { Err(Refusal::Conflict(rows)) => assert!(rows.iter().any(|r| r.contains("XYZ")), "{rows:?}"), other => panic!("{other:?}") }
    assert_eq!(dump(c), before);
}

/// A job a wake resolves that only records that it ran. It ends failed: a venue job's success must carry the credential
/// version it read under, which a stand-in has none of.
struct Ran(&'static str, Option<String>);
impl deltabadger::jobs::Job for Ran {
    fn spec(&self) -> deltabadger::jobs::Spec {
        deltabadger::jobs::Spec { name: self.0, scope: self.1.clone(), schedule: None, jitter: deltabadger::jobs::schedule::Jitter::NONE, retry: deltabadger::jobs::Retry::None,
                                  deadline: deltabadger::jobs::DEADLINE }
    }
    fn run<'a>(&'a self, _: deltabadger::jobs::Cx<'a>, _: Vec<deltabadger::jobs::Wake>) -> deltabadger::jobs::JobFuture<'a> {
        Box::pin(async { deltabadger::jobs::Outcome::Failed("stand-in".into()) })
    }
}

/// R8 #7: on a single-account install, Kraken's trading key lacking a permission hands reading to the read-only key; the
/// trading key's balance job gives way and wakes the reader's (registered by the wake), so balances still refresh.
#[tokio::test(flavor = "current_thread")]
async fn r8_a_balance_job_giving_way_wakes_the_reader() {
    let (dir, o, s) = common::install();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET status = 1, last_sync_error = 'EGeneral:Permission denied' WHERE id = ?1", [s.api_key_id]).unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 2, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.user_id, s.exchange_id, seed::cipher().encrypt("read-key"), seed::cipher().encrypt("read-secret")]).unwrap();
    let reader = c.last_insert_rowid();
    common::number_one(c, s.user_id, s.exchange_id);
    assert!(deltabadger::sync::balance_reading_keys(c).unwrap().contains(&reader), "the read-only key reads");
    type Runs = std::cell::RefCell<Vec<(String, Option<String>)>>;
    let runs: std::rc::Rc<Runs> = Default::default();
    let seen = runs.clone();
    let scheduler = deltabadger::jobs::Scheduler::new(store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary, seed::cipher(),
        vec![Box::new(deltabadger::sync::jobs::BalanceSync::new(NoVenue, std::rc::Rc::new(deltabadger::sync::balances::NoPrices), s.api_key_id))], None)
        .with_resolver(Box::new(|name, scope| Ok(Box::new(Ran(Box::leak(name.to_string().into_boxed_str()), scope.map(str::to_string))) as Box<dyn deltabadger::jobs::Job>)))
        .with_ran(Box::new(move |name, scope| seen.borrow_mut().push((name.to_string(), scope.map(str::to_string)))));
    scheduler.wakers().wake(deltabadger::sync::jobs::BALANCE_SYNC, Some(&s.api_key_id.to_string()), None);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let clock = deltabadger::engine::FixedClock(chrono::Utc::now());
    let (ended, ()) = tokio::join!(scheduler.run(stopped, &clock), async {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let _ = stop.send(true);
    });
    ended.unwrap();
    let runs = runs.borrow();
    let balance = |key: i64| runs.iter().filter(|(n, s)| n == deltabadger::sync::jobs::BALANCE_SYNC && s.as_deref() == Some(key.to_string().as_str())).count();
    assert_eq!((balance(s.api_key_id), balance(reader)), (1, 1), "{runs:?}");
}

/// The Alpaca pair of `kraken`'s user, a duplicate by fingerprint, its EUR bots paused (the eligibility guard refuses
/// any write while they are scheduled) unless `scheduled`.
fn alpaca(c: &Connection, kraken: &Two, scheduled: bool) -> Two {
    let alpaca = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    alpaca.fingerprint(c, 0, Some("fp"));
    alpaca.fingerprint(c, 1, Some("fp"));
    if !scheduled { for bot in alpaca.bots { c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).unwrap(); } }
    alpaca
}

/// R8 #3: a standalone order placed under another credential than the account's current trading key is never read
/// with that key: Merge refuses as in flight, naming it, and the order is not marked terminal.
#[tokio::test(flavor = "current_thread")]
async fn r8_an_order_placed_under_another_credential_is_not_checked() {
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let alpaca = alpaca(c, &kraken, false);
    let order = alpaca.standalone(c, 1, "done", Some(r#"{"kind":"placed","order_id":"L-1"}"#), Some("L-1"));
    let cipher = seed::cipher();
    identity::write_credential(c, &cipher, alpaca.keys[1], Some(&cipher.encrypt("another-wallet")), Some(&cipher.encrypt("s")), None, chrono::Utc::now()).unwrap();
    // The new key confirms the account again: its standing enters again (a key event ended it).
    alpaca.fingerprint(c, 1, Some("fp"));
    let server = MockServer::start().await;
    Mock::given(matchers::method("GET")).and(matchers::path("/v2/orders/L-1")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "L-1",
        "client_order_id": "c", "status": "filled", "symbol": "BTC/EUR", "side": "buy", "type": "limit", "order_type": "limit", "qty": "0.001", "filled_qty": "0.001",
        "filled_avg_price": "60000", "limit_price": "60000", "asset_class": "crypto"}))).mount(&server).await;
    c.execute("UPDATE users SET setup_completed = 1, confirmed_at = '2026-01-01'", []).unwrap();
    let uri = server.uri();
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at("2026-09-20T12:00:00Z"))
        .with_settings_key_boundary(deltabadger::venue::alpaca::Hosts { paper: uri.clone(), live: uri.clone(), data: uri }, deltabadger::web::settings::keys::logger()).unwrap();
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    let mut b = on_settings(&app, c, alpaca.seeded.user_id).await;
    let survivor = alpaca.id(0).to_string();
    let answer = b.send(&app, "POST", &format!("/settings/merge_account/{}", alpaca.id(1)), Some(&[("survivor_id", &survivor)]), common::web::Csrf::Header,
                        &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(answer.status, 422, "{}", &answer.body[..answer.body.len().min(600)]);
    assert!(answer.body.contains("can&#39;t check it with its current key") || answer.body.contains("can't check it with its current key"), "{}", answer.body);
    assert!(answer.body.contains("Alpaca 2"), "the account as text");
    assert!(one::<Option<String>>(c, &format!("SELECT json_extract(outcome, '$.terminal') FROM rust_api_orders WHERE id = {order}")).is_none());
    common::drop_mock_server(server).await;
}

/// R8 #8: the eligibility guard a Merge runs is scoped to the merging user's bots: another user's bot that this build
/// cannot run neither blocks the Merge nor reaches its answer.
#[tokio::test(flavor = "current_thread")]
async fn r8_another_users_bot_never_blocks_or_reaches_a_merge() {
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let alpaca = alpaca(c, &kraken, false);
    c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('other@example.com', 'x', 0, '2026-01-01', '2026-01-01')", []).unwrap();
    let other = c.last_insert_rowid();
    // The fixture's other EUR bot on Alpaca, scheduled, now another user's: this build cannot run it.
    let foreign = seed::insert_bot(c, &alpaca.seeded, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE bots SET user_id = ?2 WHERE id = ?1", [foreign, other]).unwrap();
    let app = web_app(dir.path(), c);
    common::two_accounts::fresh(c, alpaca.seeded.user_id, alpaca.seeded.exchange_id, "2026-09-20 11:59:00");
    let mut b = on_settings(&app, c, alpaca.seeded.user_id).await;
    let survivor = alpaca.id(0).to_string();
    let answer = b.send(&app, "POST", &format!("/settings/merge_account/{}", alpaca.id(1)), Some(&[("survivor_id", &survivor)]), common::web::Csrf::Header,
                        &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert_eq!(answer.status, 200, "{}", answer.body.rsplit("</button>").next().unwrap_or_default());
    assert!(!answer.body.contains(&format!("bot {foreign}")), "{}", answer.body);
    assert!(answer.body.contains("Alpaca 2"), "the names as text in the flash");
    assert!(deltabadger::engine::eligibility::guard(c, &seed::cipher(), None).is_err(), "the install-wide guard still refuses");
}

// ---- the stored standing (PLAN-710-6 §5.1 r9) ----

fn standings(c: &Connection) -> Vec<(i64, i64)> {
    c.prepare("SELECT duplicate_id, survivor_id FROM exchange_account_standings ORDER BY duplicate_id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
/// An overlap pair (Kraken, three shared trades) whose balances were read equal: entered.
fn entered() -> (tempfile::TempDir, store::Opened, Two) {
    let (d, o) = install();
    let two = Two::kraken(&o.primary, &seed::cipher());
    trades(&o.primary, &two, 0, 3, 0, "1");
    trades(&o.primary, &two, 1, 3, 0, "1");
    assert!(overlaps(&o.primary, &two, 1));
    (d, o, two)
}

/// The standing is stored: signals alone make no duplicate until `reconsider` enters it, and entry needs every one of
/// them (shared trades without comparable balances, or fingerprints a live key has not confirmed, are not enough).
#[test]
fn st_entry_needs_every_signal_and_is_stored() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    assert!(standing(c, &two).is_empty(), "no balances read");
    for k in [0, 1] { common::two_accounts::read_balances(c, two.seeded.user_id, two.id(k), two.keys[k], &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert!(identity::duplicates(c, two.seeded.user_id).unwrap().is_empty(), "read from the table: nothing entered yet");
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))]);
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);

    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    two.fingerprint(c, 0, Some("fp"));
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'n', 'n', 1, 2, '2026-01-01', '2026-01-01')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [c.last_insert_rowid(), two.id(1)]).unwrap();
    // 2's first key confirms the fingerprint; the second has no observation.
    c.execute("UPDATE exchange_accounts SET fingerprint = 'fp' WHERE id = ?1", [two.id(1)]).unwrap();
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, fingerprint, observed_at) VALUES (?1, ?2, 'fp', 'now')", [two.keys[1], two.seeded.exchange_id]).unwrap();
    assert!(standing(c, &two).is_empty(), "a live key of 2 has not confirmed the fingerprint");
}

/// A refresh that leaves the reads not comparable (one account read a day later) keeps the row: figures do not flip.
#[test]
fn st_a_non_comparable_refresh_keeps_the_row() {
    let (_d, o, two) = entered();
    let c = &o.primary;
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "1")], "2026-09-21 10:00:00");
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))]);
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "5")], "2026-09-22 10:00:00");
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "different, but not comparable");
}

/// A positive contradiction clears it: balances that differ at comparable reads, a shared id with other facts.
#[test]
fn st_a_contradiction_clears_it() {
    let (_d, o, two) = entered();
    let c = &o.primary;
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "2")], "2026-09-20 10:05:00");
    assert!(standing(c, &two).is_empty(), "balances differ");

    let (_d, o, two) = entered();
    let c = &o.primary;
    two.trade(c, 0, 0, "late", "1", "2026-09-11 10:00:00");
    two.trade(c, 1, 0, "late", "2", "2026-09-11 10:00:00");
    assert!(standing(c, &two).is_empty(), "a conflicting shared id");
}

/// Every key event of either account deletes the row: a real credential change, a key added, a key removed, and the
/// duplicate's removal. The same material saved again is no event.
#[test]
fn st_each_key_event_clears_it() {
    let cipher = seed::cipher();
    let now = chrono::Utc::now();
    let (_d, o, two) = entered();
    let c = &o.primary;
    identity::write_credential(c, &cipher, two.keys[0], Some(&cipher.encrypt("test-key")), Some(&c.query_row("SELECT secret FROM api_keys WHERE id = ?1", [two.keys[0]], |r| r.get::<_, String>(0)).unwrap()), None, now).unwrap();
    assert_eq!(standings(c), [(two.id(1), two.id(0))], "the same material");
    identity::write_credential(c, &cipher, two.keys[0], Some(&cipher.encrypt("another-wallet")), None, None, now).unwrap();
    assert!(standings(c).is_empty(), "a real change");

    let (_d, o, two) = entered();
    let c = &o.primary;
    add_key(c, &two, two.id(1));
    assert!(standings(c).is_empty(), "a key added");

    let (_d, o, two) = entered();
    let c = &o.primary;
    identity::forget(c, two.keys[1], &two.accounts[1]).unwrap();
    assert!(standings(c).is_empty(), "a key removed");

    let (_d, o, two) = entered();
    let c = &o.primary;
    identity::tombstone(c, two.id(1), now).unwrap();
    assert!(standings(c).is_empty(), "the account removed");
}

/// Merge refuses when the keys or credentials changed since the row was entered and nothing entered it again (here a
/// live key the standing never saw, written past the key-event path, that has not confirmed the fingerprint).
#[test]
fn st_merge_refuses_when_the_basis_changed() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'w', 'w', 1, 1, '2026-01-01', '2026-01-01')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [c.last_insert_rowid(), two.id(1)]).unwrap();
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    let before = dump(c);
    assert!(matches!(merge_now(c, &two, two.id(1)), Ok(None)));
    assert_eq!(dump(c), before);
}

/// A single-account install writes no row and changes no byte.
#[test]
fn st_a_single_account_install_writes_nothing() {
    let (_d, o, s) = common::install();
    let c = &o.primary;
    common::number_one(c, s.user_id, s.exchange_id);
    let before = dump(c);
    identity::reconsider(c, s.user_id, s.exchange_id, chrono::Utc::now()).unwrap();
    assert!(identity::after_balance_read(c, s.api_key_id, chrono::Utc::now()).unwrap().is_empty());
    assert_eq!(dump(c), before);
}

// ---- review round 9 ----

/// R9 #1: `reconsider` reads its evidence and writes its rows in one immediate write transaction: it takes the write lock
/// first, even when it ends up writing nothing.
#[test]
fn r9_reconsider_takes_the_write_lock() {
    let (dir, o, two) = entered();
    let c = &o.primary;
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "1")], "2026-09-21 10:00:00");
    let other = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    c.busy_timeout(std::time::Duration::from_millis(50)).unwrap();
    assert!(identity::reconsider(c, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).is_err(), "another writer holds the lock");
    other.execute_batch("ROLLBACK").unwrap();
    assert!(identity::reconsider(c, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).is_ok());
}

/// R9 #2, R11 #1: at Merge every kept key's change marker becomes the merge time: no row imported before the Merge is
/// evidence for the merged account again.
#[test]
fn r9_the_kept_key_takes_the_deleted_keys_change_marker() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    deltabadger::app_config::set_plain(c, &identity::credential_changed_key(two.keys[1]), "2026-10-01 00:00:00", chrono::Utc::now()).unwrap();
    let before = now_text(-1);
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(merged.deleted_keys, [two.keys[1]]);
    let marker = deltabadger::app_config::get_plain(c, &identity::credential_changed_key(two.keys[0])).unwrap().unwrap();
    assert!(marker >= before, "{marker} is the merge time");
}

/// A KuCoin pair (a venue whose keys report their account id), both accounts on fingerprint `fp`, entered.
fn kucoin_pair(c: &Connection) -> Two {
    identity::key(c, &seed::cipher(), chrono::Utc::now()).unwrap();
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    two
}
fn observe(c: &Connection, key: i64, observed: &identity::Observed) {
    let cipher = seed::cipher();
    let version = deltabadger::engine::model::credential_version_by_id(c, key).unwrap().unwrap();
    let credentials = deltabadger::crypto::Credentials { key: "k".into(), secret: "s".into(), passphrase: None, redaction_values: vec![], ibkr: None };
    identity::record(c, &cipher, key, &credentials, &version, observed, chrono::Utc::now()).unwrap();
}

/// R9 #3: an identity read naming another account than the standing's, or a definite failure on the key's current
/// material (auth refused: 401 or 403; a masked or error answer under a success status), deletes the standing at once
/// and keeps it out while it is the key's latest answer; a transient one (5xx, 429, the transport, no egress) changes
/// nothing. Merge refuses on the same split since the standing was entered.
#[test]
fn r9_identity_reads_that_contradict_end_the_standing() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin_pair(c);
    observe(c, two.keys[1], &identity::Observed::Read(identity::AccountId::parse("424242").unwrap()));
    assert!(standings(c).is_empty(), "another account's id");

    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin_pair(c);
    for transient in ["http 500", "http 429", "transport", "no egress"] {
        observe(c, two.keys[0], &identity::Observed::Failed(transient.into()));
        assert_eq!(standings(c), [(two.id(1), two.id(0))], "{transient}: nothing comparable changes nothing");
        assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "{transient}: nor does the next reconsideration");
    }
    observe(c, two.keys[0], &identity::Observed::Failed("http 401; code 400003".into()));
    assert!(standings(c).is_empty(), "auth refused on its current material");
    assert!(standing(c, &two).is_empty(), "and it stays out while that is the key's latest answer");

    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin_pair(c);
    observe(c, two.keys[1], &identity::Observed::Failed("http 200".into()));
    assert!(standings(c).is_empty(), "a masked answer");

    let fail = |code: &str| {
        let (d, o, two) = duplicate();
        o.primary.execute("UPDATE exchange_account_credentials SET last_failure_at = ?2, last_failure_code = ?3 WHERE api_key_id = ?1",
                          rusqlite::params![two.keys[0], now_text(1), code]).unwrap();
        (d, o, two)
    };
    let (_d, o, two) = fail("http 503");
    assert!(merge_now(&o.primary, &two, two.id(1)).unwrap().is_some(), "a transient failure since entry");
    let (_d, o, two) = fail("http 401");
    assert!(matches!(merge_now(&o.primary, &two, two.id(1)), Ok(None)), "auth refused since entry");
}

/// R9 #4: the veto runs over the final component: a retained 3 → 2 whose reads are no longer comparable and a new 2 → 1
/// flatten into one component, which a 3-1 conflict voids whole.
#[test]
fn r9_the_veto_runs_over_the_final_component() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (a1, k1, a2, k2) = (two.id(0), two.keys[0], two.id(1), two.keys[1]);
    let (a3, k3) = account_numbered(c, &two, 3);
    for i in 0..3 { for (a, k) in [(a3, k3), (a2, k2)] { trade_on(c, &two, a, k, &format!("b{i}"), "1", &format!("2026-09-02 1{i}:00:00")); } }
    for (a, k) in [(a2, k2), (a3, k3)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert_eq!(standing(c, &two), [(a3, a2)]);
    common::two_accounts::read_balances(c, two.seeded.user_id, a3, k3, &[(two.seeded.btc, "1")], "2026-09-25 10:00:00");
    for i in 0..3 { for (a, k) in [(a2, k2), (a1, k1)] { trade_on(c, &two, a, k, &format!("c{i}"), "1", &format!("2026-09-03 1{i}:00:00")); } }
    trade_on(c, &two, a3, k3, "x", "1", "2026-09-04 10:00:00");
    trade_on(c, &two, a1, k1, "x", "2", "2026-09-04 10:00:00");
    for (a, k) in [(a1, k1), (a2, k2)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert!(standing(c, &two).is_empty(), "{:?}", standings(c));
}

/// R9 #5: the rotation cutoff is compared at full precision, strictly: rows imported 0.8 s before the change are out.
#[test]
fn r9_the_rotation_cutoff_is_exact() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    c.execute("UPDATE account_transactions SET created_at = '2026-09-15 10:00:00.100'", []).unwrap();
    deltabadger::app_config::set_plain(c, &identity::credential_changed_key(two.keys[0]), "2026-09-15 10:00:00.900", chrono::Utc::now()).unwrap();
    assert!(!overlaps(c, &two, 1));
}

/// R9 #7: a live key's status transition and a passphrase-only change are key events; a status write that keeps a key
/// live is none.
#[test]
fn r9_status_transitions_and_passphrases_are_key_events() {
    let cipher = seed::cipher();
    let (_d, o, two) = entered();
    let c = &o.primary;
    identity::write_status(c, two.keys[1], 1, "2026-09-21 10:00:00", true, false).unwrap();
    assert_eq!(standings(c), [(two.id(1), two.id(0))], "still live");
    identity::write_status(c, two.keys[1], 2, "2026-09-21 10:00:00", true, false).unwrap();
    assert!(standings(c).is_empty(), "marked incorrect");

    let (_d, o, two) = entered();
    let c = &o.primary;
    let (key, secret): (String, String) = c.query_row("SELECT key, secret FROM api_keys WHERE id = ?1", [two.keys[0]], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(identity::write_credential(c, &cipher, two.keys[0], Some(&key), Some(&secret), Some(&cipher.encrypt("another-passphrase")), chrono::Utc::now()).unwrap());
    assert!(standings(c).is_empty(), "a passphrase-only change");
}

/// R9 #8: the pre-flight reads a standalone order placed before an identical re-save (a fresh envelope, the same
/// material): its key's material has not changed since.
#[tokio::test(flavor = "current_thread")]
async fn r9_identical_material_keeps_its_ciphertext_and_its_orders_checkable() {
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};
    let cipher = seed::cipher();
    let (dir, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &cipher);
    let alpaca = alpaca(c, &kraken, false);
    let stored = |c: &Connection| c.query_row("SELECT key FROM api_keys WHERE id = ?1", [alpaca.keys[1]], |r| r.get::<_, String>(0)).unwrap();
    let before = stored(c);
    let order = alpaca.standalone(c, 1, "done", Some(r#"{"kind":"placed","order_id":"L-1"}"#), Some("L-1"));
    // An identical save after the placement: the same material under a fresh envelope (R4 versions it), no change recorded.
    let (secret, passphrase): (String, Option<String>) = c.query_row("SELECT secret, passphrase FROM api_keys WHERE id = ?1", [alpaca.keys[1]], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(!identity::write_credential(c, &cipher, alpaca.keys[1], Some(&cipher.encrypt(&cipher.decrypt(&before).unwrap())), Some(&secret), passphrase.as_deref(), chrono::Utc::now()).unwrap());
    assert_ne!(stored(c), before, "a fresh envelope");
    let server = MockServer::start().await;
    Mock::given(matchers::method("GET")).and(matchers::path("/v2/orders/L-1")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "L-1",
        "client_order_id": "c", "status": "filled", "symbol": "BTC/EUR", "side": "buy", "type": "limit", "order_type": "limit", "qty": "0.001", "filled_qty": "0.001",
        "filled_avg_price": "60000", "limit_price": "60000", "asset_class": "crypto"}))).mount(&server).await;
    c.execute("UPDATE users SET setup_completed = 1, confirmed_at = '2026-01-01'", []).unwrap();
    let uri = server.uri();
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at("2026-09-20T12:00:00Z"))
        .with_settings_key_boundary(deltabadger::venue::alpaca::Hosts { paper: uri.clone(), live: uri.clone(), data: uri }, deltabadger::web::settings::keys::logger()).unwrap();
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    let mut b = on_settings(&app, c, alpaca.seeded.user_id).await;
    let survivor = alpaca.id(0).to_string();
    let answer = b.send(&app, "POST", &format!("/settings/merge_account/{}", alpaca.id(1)), Some(&[("survivor_id", &survivor)]), common::web::Csrf::Header,
                        &[("Accept", "text/vnd.turbo-stream.html")]).await;
    assert!(!answer.body.contains("current key"), "{}", answer.body);
    assert_eq!(one::<Option<String>>(c, &format!("SELECT json_extract(outcome, '$.terminal') FROM rust_api_orders WHERE id = {order}")).as_deref(), Some("filled"));
    common::drop_mock_server(server).await;
}

/// R9 #9: a balance read wakes a candidate partner only when the partner's read is more than fifteen minutes older and it
/// was not woken for this pair within the hour: a persistently skipped position (never comparable) loops nothing.
#[test]
fn r9_partner_wakes_are_throttled() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    let now = chrono::Utc::now();
    let at = |offset: i64| (now + chrono::Duration::seconds(offset)).format("%Y-%m-%d %H:%M:%S").to_string();
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(0), two.keys[0], &[(two.seeded.btc, "1")], &at(0));
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "1")], &at(-60));
    deltabadger::app_config::set_plain(c, &format!("rust_sync.skipped:{}", two.keys[1]), r#"{"codes":["ZZZZ"],"at":"now"}"#, now).unwrap();
    assert!(identity::after_balance_read(c, two.keys[0], now).unwrap().is_empty(), "its read is a minute old");
    common::two_accounts::read_balances(c, two.seeded.user_id, two.id(1), two.keys[1], &[(two.seeded.btc, "1")], &at(-7200));
    assert_eq!(identity::after_balance_read(c, two.keys[0], now).unwrap(), [two.keys[1]], "two hours old: read it now");
    assert!(identity::after_balance_read(c, two.keys[0], now).unwrap().is_empty(), "woken within the hour");
}

/// R9 #10: the portable export clears the stored standings, as it clears credentials and identity observations.
#[test]
fn r9_the_portable_export_clears_the_standings() {
    let (_d, o, _two) = entered();
    let c = &o.primary;
    let tx = c.unchecked_transaction().unwrap();
    deltabadger::export::scrub(&tx, chrono::Utc::now()).unwrap();
    assert!(standings(&tx).is_empty());
}

/// R9 #11: the missing-price notice leaves a duplicate's balances out of All.
#[test]
fn r9_the_unpriced_notice_leaves_a_duplicate_out() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, synced_at, created_at, updated_at, account_id) \
               VALUES (?1, ?2, ?3, 1, 0, 'now', 'now', 'now', ?4)", rusqlite::params![two.seeded.user_id, two.seeded.exchange_id, two.seeded.btc, two.id(1)]).unwrap();
    let gaps = deltabadger::tracker::unpriced::read(c, two.seeded.user_id, &deltabadger::tracker::Scope::All, &[]).unwrap();
    assert!(gaps.iter().all(|g| g.now.is_empty()), "{gaps:?}");
}

/// R9 #11: the ledger listings that read every account (the MCP/REST list and CSV export of account transactions) leave
/// a duplicate's rows out; naming the duplicate's account still reads them.
#[tokio::test(flavor = "current_thread")]
async fn r9_listings_leave_a_duplicates_rows_out() {
    let (dir, o, two) = duplicate();
    let c = &o.primary;
    let user = two.seeded.user_id;
    two.row(c, 0, 4, "BTC", "1", Some("on-1"), None, "2026-09-10 10:00:00");
    two.row(c, 1, 4, "BTC", "1", Some("on-2"), None, "2026-09-10 10:00:00");
    let app = web_app(dir.path(), c);
    let all = mcp(c, &app, user, "list_account_transactions", serde_json::json!({"exchange_id": two.seeded.exchange_id}));
    assert!(all.to_string().contains("Account transactions (1)"), "{all}");
    let named = mcp(c, &app, user, "list_account_transactions", serde_json::json!({"account_id": two.id(1)}));
    assert!(named.to_string().contains("Account transactions (1)"), "{named}");
    let exported = mcp(c, &app, user, "export_transactions_csv", serde_json::json!({}));
    assert!(!exported.to_string().contains("on-2"), "{exported}");
}

// ---- review round 10 ----

/// R10 #1: entering a new pair never overwrites a retained edge: a retained 3 → 1 and a freshly accepted 3 - 2 (1's read
/// too old to compare) make one component, every member a duplicate of 1.
#[test]
fn r10_new_pairs_join_retained_edges() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (a1, k1, a2, k2) = (two.id(0), two.keys[0], two.id(1), two.keys[1]);
    let (a3, k3) = account_numbered(c, &two, 3);
    for i in 0..3 { for (a, k) in [(a3, k3), (a1, k1)] { trade_on(c, &two, a, k, &format!("a{i}"), "1", &format!("2026-09-02 1{i}:00:00")); } }
    for (a, k) in [(a1, k1), (a3, k3)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-20 10:00:00"); }
    assert_eq!(standing(c, &two), [(a3, a1)]);
    for i in 0..3 { for (a, k) in [(a3, k3), (a2, k2)] { trade_on(c, &two, a, k, &format!("b{i}"), "1", &format!("2026-09-03 1{i}:00:00")); } }
    for (a, k) in [(a2, k2), (a3, k3)] { common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, "1")], "2026-09-25 10:00:00"); }
    assert_eq!(standing(c, &two), [(a2, a1), (a3, a1)]);
}

/// A KuCoin pair whose keys' identity reads succeeded (their material's digest stored) and named one account, entered.
fn kucoin_observed(c: &Connection) -> Two {
    identity::key(c, &seed::cipher(), chrono::Utc::now()).unwrap();
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    for key in two.keys { observe(c, key, &identity::Observed::Read(identity::AccountId::parse("777").unwrap())); }
    c.execute("UPDATE exchange_accounts SET fingerprint = (SELECT fingerprint FROM exchange_account_credentials WHERE api_key_id = ?1) WHERE id IN (?2, ?3)",
              [two.keys[0], two.id(0), two.id(1)]).unwrap();
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))]);
    two
}

/// R10 #2: a definite failure is sticky: a later transient read does not lift it, only a successful read does.
#[test]
fn r10_a_definite_failure_stays_until_a_successful_read() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin_observed(c);
    observe(c, two.keys[0], &identity::Observed::Failed("http 401; code 400003".into()));
    assert!(standings(c).is_empty());
    observe(c, two.keys[0], &identity::Observed::Failed("http 503".into()));
    assert!(standing(c, &two).is_empty(), "a transient read does not lift the 401");
    observe(c, two.keys[0], &identity::Observed::Read(identity::AccountId::parse("777").unwrap()));
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))], "a successful read does");
}

/// R10 #3: a key that is not live gives no confirmation, whatever its old fingerprint: an account whose only key turned
/// incorrect does not enter again.
#[test]
fn r10_a_key_that_is_not_live_confirms_nothing() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    identity::write_status(c, two.keys[0], 2, "2026-09-21 10:00:00", true, false).unwrap();
    assert!(standing(c, &two).is_empty());
}

/// R10 #8: Merge hands a deleted key's pending split restatements to the kept key, merged with its own.
#[test]
fn r10_merge_keeps_the_deleted_keys_pending_splits() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let set = |key: i64, symbol: &str| deltabadger::app_config::set_plain(c, &format!("rust_sync.ledger_splits:{key}"),
        &serde_json::json!({"pending": [{"symbol": symbol, "exchange_id": two.seeded.exchange_id, "at": 1_900_000_000_000_000i64}]}).to_string(), chrono::Utc::now()).unwrap();
    set(two.keys[0], "AAPL");
    set(two.keys[1], "MSFT");
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(merged.deleted_keys, [two.keys[1]]);
    let kept: serde_json::Value = serde_json::from_str(&deltabadger::app_config::get_plain(c, &format!("rust_sync.ledger_splits:{}", two.keys[0])).unwrap().unwrap()).unwrap();
    let symbols: Vec<&str> = kept["pending"].as_array().unwrap().iter().map(|o| o["symbol"].as_str().unwrap()).collect();
    assert_eq!(symbols, ["AAPL", "MSFT"]);
}

// ---- review round 11 ----

/// R11 #3: a key that is not live cannot end a standing: its definite failure, or a read naming another account, is no
/// key event and no contradiction.
#[test]
fn r11_a_key_that_is_not_live_cannot_end_a_standing() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = kucoin_observed(c);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'r', 'r', 2, 2, '2026-01-01', '2026-01-01')",
              [two.seeded.user_id, two.seeded.exchange_id]).unwrap();
    let spare = c.last_insert_rowid();
    c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [spare, two.id(1)]).unwrap();
    observe(c, spare, &identity::Observed::Failed("http 401".into()));
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))]);
    observe(c, spare, &identity::Observed::Read(identity::AccountId::parse("999").unwrap()));
    assert_eq!(standing(c, &two), [(two.id(1), two.id(0))]);
}

/// R11 #4: split reconciliation reads the bot's own account's ledger rows only: two accounts each importing the split
/// (two real accounts, or a duplicate) leave each bot one report.
#[test]
fn r11_split_rows_are_the_bots_own_accounts() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    for n in 0..2 {
        let row = two.row(c, n, 15, "BTC", "2", Some(&format!("split-{n}")), None, "2026-09-01 10:00:00");
        c.execute("UPDATE account_transactions SET raw_data = '{\"corporate_action\":\"split\"}' WHERE id = ?1", [row]).unwrap();
    }
    let rows = |c: &Connection, bot: i64| deltabadger::figures::db::split_rows(c, two.seeded.user_id, &deltabadger::figures::splits::bot_rows(c, bot).unwrap(), &[two.seeded.exchange_id]).unwrap().len();
    assert_eq!((rows(c, two.bots[0]), rows(c, two.bots[1])), (1, 1), "two real accounts");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    assert_eq!((rows(c, two.bots[0]), rows(c, two.bots[1])), (1, 1), "a duplicate");
}

/// R11 #6: Merge hands pending split restatements to the key that runs the merged account's ledger sync: the duplicate's
/// read-only key held them, the survivor's trading key reads.
#[test]
fn r11_pending_splits_go_to_the_ledger_reading_key() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET key_type = 2 WHERE id = ?1", [two.keys[1]]).unwrap();
    deltabadger::app_config::set_plain(c, &format!("rust_sync.ledger_splits:{}", two.keys[1]),
        &serde_json::json!({"pending": [{"symbol": "MSFT", "exchange_id": two.seeded.exchange_id, "at": 1_900_000_000_000_000i64}]}).to_string(), chrono::Utc::now()).unwrap();
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert!(merged.kept_keys.contains(&two.keys[1]), "the read-only key moves");
    let of = |key: i64| deltabadger::app_config::get_plain(c, &format!("rust_sync.ledger_splits:{key}")).unwrap();
    assert!(of(two.keys[0]).is_some_and(|v| v.contains("MSFT")), "the trading key owes it");
    assert!(of(two.keys[1]).is_none());
}

// ---- review round 12 ----

/// R12 #1: the guard reads the withdrawal key a rule will use after Merge with the withdrawal engine's own selection, on
/// the merged key set: the survivor's 10 (incorrect) and 11 (live), the duplicate's 12 (live) and its rule: Merge deletes
/// 10 and keeps 11 and 12, the engine picks 11, so the duplicate's rule would switch keys: refused.
#[test]
fn r12_the_guard_uses_the_engines_withdrawal_key() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let key = |n: usize, status: i64| {
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'w', 'w', ?3, 1, '2026-01-01', '2026-01-01')",
                  [two.seeded.user_id, two.seeded.exchange_id, status]).unwrap();
        let id = c.last_insert_rowid();
        two.accounts[n].add_key(c, id).unwrap();
        id
    };
    let (ten, eleven, twelve) = (key(0, 2), key(0, 1), key(1, 1));
    assert!(ten < eleven && eleven < twelve);
    for n in 0..2 { two.fingerprint(c, n, Some("fp")); }
    two.rule(c, &seed::cipher(), 1, two.seeded.btc, 0);
    let before = dump(c);
    match merge_now(c, &two, two.id(1)) { Err(Refusal::WithdrawalKeys(rules)) => assert!(!rules.is_empty()), other => panic!("{other:?}") }
    assert_eq!(dump(c), before);
}

/// R12 #2: a bot's split rows are those of every account its orders were placed on: switched to another account after a
/// split on the first, it keeps that split.
#[test]
fn r12_a_switched_bot_keeps_its_accounts_splits() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    two.order(c, 0, "bought-on-1", 1, Some(2));
    let row = two.row(c, 0, 15, "BTC", "10", Some("split-1"), None, "2026-09-05 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"corporate_action\":\"split\"}' WHERE id = ?1", [row]).unwrap();
    identity::place_bot(c, two.bots[0], two.seeded.user_id, two.seeded.exchange_id, Some(two.accounts[1]), chrono::Utc::now()).unwrap();
    let rows = deltabadger::figures::db::split_rows(c, two.seeded.user_id, &deltabadger::figures::splits::bot_rows(c, two.bots[0]).unwrap(), &[two.seeded.exchange_id]).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [row]);
}

/// R12 #5, R18: the split's asset-class lookup reads every account's rows, the duplicate's included.
#[test]
fn r12_the_class_lookup_reads_the_bots_accounts() {
    // R18: classification reads every account's rows, a duplicate's included.
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    for (n, asset) in [(0, two.seeded.btc), (1, two.seeded.quote)] {
        let row = two.row(c, n, 0, "BTC", "1", Some(&format!("t-{n}")), None, "2026-09-01 10:00:00");
        c.execute("UPDATE account_transactions SET base_asset_id = ?2 WHERE id = ?1", [row, asset]).unwrap();
    }
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let classes = deltabadger::figures::db::account_classes(c, two.seeded.user_id, &["BTC".to_string()]).unwrap();
    assert_eq!(classes, [("BTC".to_string(), Some("Cryptocurrency".to_string())), ("BTC".to_string(), Some("Currency".to_string()))]);
}

/// R12 #6: a blank group is no group, planning and writing alike: the kept twin's '' takes the duplicate's group.
#[test]
fn r12_a_blank_group_takes_the_duplicates() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let kept = two.row(c, 0, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET group_id = '' WHERE id = ?1", [kept]).unwrap();
    two.row(c, 1, 0, "BTC", "1", Some("t-1"), Some("usdt-leg"), "2026-09-01 10:00:00");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(one::<Option<String>>(c, &format!("SELECT group_id FROM account_transactions WHERE id = {kept}")).as_deref(), Some("usdt-leg"));
}

// ---- review round 13 ----

/// A key of account `n` with `key_type` and `status`, observed as the account's fingerprint.
fn typed_key(c: &Connection, two: &Two, n: usize, key_type: i64, status: i64) -> i64 {
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k', 's', ?3, ?4, '2026-01-01', '2026-01-01')",
              [two.seeded.user_id, two.seeded.exchange_id, status, key_type]).unwrap();
    let id = c.last_insert_rowid();
    two.accounts[n].add_key(c, id).unwrap();
    id
}

/// R13 #1: per key type Merge keeps the survivor's correct key, else a correct key of the duplicate's: never an incorrect
/// one ahead of a correct one. The survivor reads with a read-only key; the duplicate's older trading key is incorrect,
/// its newer one correct: the correct one moves.
#[test]
fn r13_merge_keeps_a_correct_key_first() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET key_type = 2 WHERE id = ?1", [two.keys[0]]).unwrap();
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two.keys[1]]).unwrap();
    let correct = typed_key(c, &two, 1, 0, 1);
    for n in 0..2 { two.fingerprint(c, n, Some("fp")); }
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert!(merged.kept_keys.contains(&correct) && merged.deleted_keys.contains(&two.keys[1]), "{merged:?}");
    assert_eq!(identity::account_of_key(c, correct).unwrap(), identity::Account::first(c, two.seeded.user_id, two.seeded.exchange_id).unwrap().id);
}

/// R13 #2: restore-shaped twins compare their exact amounts: KuCoin's float columns can hide a difference.
#[test]
fn r13_restored_twins_compare_their_exact_amounts() {
    let (_d, o) = install();
    let c = &o.primary;
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    for (n, exact) in [(0, "9007199254740992"), (1, "9007199254740993")] {
        let row = two.row(c, n, 0, "BTC", "9007199254740992", Some("fill-1"), None, "2026-09-01 10:00:00");
        c.execute("UPDATE account_transactions SET raw_data = json_object('amounts', json_object('base', ?2), 'amounts_inexact', json('false')) WHERE id = ?1", rusqlite::params![row, exact]).unwrap();
    }
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))));
}

/// R13 #5, R14 #1: after Merge each key type has exactly one key on the merged account, and pending splits go to the
/// scheduler's own ledger reader: the survivor's 10 (incorrect) and 11 (correct), the duplicate's 12 (correct, with the
/// pending split): only 11 remains, owes the split, and the moved bot trades with it.
#[test]
fn r13_pending_splits_go_to_the_schedulers_reader() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two.keys[0]]).unwrap();
    let eleven = typed_key(c, &two, 0, 0, 1);
    let twelve = typed_key(c, &two, 1, 0, 1);
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [two.keys[1]]).unwrap();
    c.execute("DELETE FROM api_keys WHERE id = ?1", [two.keys[1]]).unwrap();
    for n in 0..2 { two.fingerprint(c, n, Some("fp")); }
    deltabadger::app_config::set_plain(c, &format!("rust_sync.ledger_splits:{twelve}"),
        &serde_json::json!({"pending": [{"symbol": "MSFT", "exchange_id": two.seeded.exchange_id, "at": 1_900_000_000_000_000i64}]}).to_string(), chrono::Utc::now()).unwrap();
    let merged = merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(merged.kept_keys, [eleven], "{merged:?}");
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM api_keys WHERE user_id = {} AND exchange_id = {} AND key_type = 0", two.seeded.user_id, two.seeded.exchange_id)), 1);
    assert!(deltabadger::app_config::get_plain(c, &format!("rust_sync.ledger_splits:{eleven}")).unwrap().is_some_and(|v| v.contains("MSFT")));
    let bot = deltabadger::engine::model::load_bot(c, two.bots[1]).unwrap();
    assert_eq!(deltabadger::engine::model::credential_version(c, &bot).unwrap().unwrap().id, eleven, "the moved bot trades with 11");
}

/// R13 #7: blanks are judged alike in planning and writing: a kept twin's whitespace group takes the duplicate's.
#[test]
fn r13_a_whitespace_group_takes_the_duplicates() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let kept = two.row(c, 0, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET group_id = ' ' WHERE id = ?1", [kept]).unwrap();
    two.row(c, 1, 0, "BTC", "1", Some("t-1"), Some("usdt-leg"), "2026-09-01 10:00:00");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(one::<Option<String>>(c, &format!("SELECT group_id FROM account_transactions WHERE id = {kept}")).as_deref(), Some("usdt-leg"));
}

/// R13 #3: the split's asset-class lookup reads the user's whole ledger and balances: an
/// asset-less Alpaca name that a Kraken row names as a coin stays ambiguous, as on main.
#[test]
fn r13_the_class_lookup_reads_every_account() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let row = kraken.row(c, 0, 0, "XYZ", "1", Some("k-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET base_asset_id = ?2 WHERE id = ?1", [row, kraken.seeded.btc]).unwrap();
    let classes = deltabadger::figures::db::account_classes(c, kraken.seeded.user_id, &["XYZ".to_string()]).unwrap();
    assert_eq!(classes, [("XYZ".to_string(), Some("Cryptocurrency".to_string()))]);
}

/// R13 #4: a bot with no current exchange (a basket or index bot) reads the split rows of its orders' accounts, and
/// never fails for want of one.
#[test]
fn r13_a_bot_without_an_exchange_reads_its_orders_accounts() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    two.order(c, 0, "bought-on-1", 1, Some(2));
    let row = two.row(c, 0, 15, "BTC", "10", Some("split-1"), None, "2026-09-05 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"corporate_action\":\"split\"}' WHERE id = ?1", [row]).unwrap();
    c.execute("DELETE FROM exchange_account_bots WHERE bot_id = ?1", [two.bots[0]]).unwrap();
    c.execute("UPDATE bots SET exchange_id = NULL WHERE id = ?1", [two.bots[0]]).unwrap();
    let scope = deltabadger::figures::splits::bot_rows(c, two.bots[0]).unwrap();
    let rows = deltabadger::figures::db::split_rows(c, two.seeded.user_id, &scope, &[two.seeded.exchange_id]).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [row]);
}

/// R13 #6: the figure cache's revision moves with what the calculation reads: the duplicate standing, and the account a
/// split row belongs to.
#[test]
fn r13_the_figure_cache_moves_with_the_standing_and_the_scope() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    let rev = |c: &Connection| deltabadger::web::figure::loading::revision(c, two.seeded.user_id, chrono::Utc::now()).unwrap().0;
    let row = two.row(c, 0, 15, "BTC", "10", Some("split-1"), None, "2026-09-05 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"corporate_action\":\"split\"}' WHERE id = ?1", [row]).unwrap();
    let before = rev(c);
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    let entered = rev(c);
    assert_ne!(entered, before, "the standing");
    c.execute("UPDATE account_transactions SET tx_id = NULL WHERE id = ?1", [row]).unwrap();
    let moved = rev(c);
    c.execute("INSERT INTO exchange_account_rows (account_transaction_id, account_id, venue_tx_id) VALUES (?1, ?2, 'split-1')", [row, two.id(1)]).unwrap();
    assert_ne!(rev(c), moved, "the split row's account");
}

// ---- review round 14 ----

/// R14 #2: twins' amounts are compared as stored, exactly: 100000000.0000001 and 100000000.0000002 differ.
#[test]
fn r14_twin_amounts_compare_exactly() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    for (n, amount) in [(0, 100000000.0000001f64), (1, 100000000.0000002f64)] {
        let row = two.row(c, n, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
        c.execute("UPDATE account_transactions SET base_amount = ?2 WHERE id = ?1", rusqlite::params![row, amount]).unwrap();
    }
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))));
}

/// R14 #3: a restore twin's exact amounts against an API twin's fields, at full precision: 1.000000000000000001 is not
/// 1.000000000000000002.
#[test]
fn r14_a_restore_and_an_api_twin_compare_at_full_precision() {
    let (_d, o) = install();
    let c = &o.primary;
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let api = two.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1,\"size\":\"1.000000000000000001\"}' WHERE id = ?1", [api]).unwrap();
    let restore = two.row(c, 1, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"amounts\":{\"base\":\"1.000000000000000002\"}}' WHERE id = ?1", [restore]).unwrap();
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))));
}

/// R14 #4: TRIM only decides blank: a survivor's " swap " legs keep their group, paired, when one of them has a twin.
#[test]
fn r14_a_padded_group_stays_as_it_is() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let leg = two.row(c, 0, 0, "BTC", "1", Some("t-1"), Some(" swap "), "2026-09-01 10:00:00");
    let other = two.row(c, 0, 1, "USDT", "100", Some("t-2"), Some(" swap "), "2026-09-01 10:00:00");
    two.row(c, 1, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    for row in [leg, other] { assert_eq!(one::<Option<String>>(c, &format!("SELECT group_id FROM account_transactions WHERE id = {row}")).as_deref(), Some(" swap ")); }
}

/// R14 #5: with no stored history version, a history read under a duplicate standing is a standing change (the first
/// sweep clears the stale whole-portfolio points); with no duplicate it is none.
#[test]
fn r14_the_first_sweep_under_a_standing_is_a_change() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let cipher = seed::cipher();
    let version = deltabadger::tracker::backfill::history_version(c, two.seeded.user_id).unwrap();
    assert!(deltabadger::tracker::backfill::standing_changed(c, &cipher, two.seeded.user_id, &version).unwrap());
    let (_d, o) = install();
    let c = &o.primary;
    let single = Two::kraken(c, &cipher);
    let version = deltabadger::tracker::backfill::history_version(c, single.seeded.user_id).unwrap();
    assert!(!deltabadger::tracker::backfill::standing_changed(c, &cipher, single.seeded.user_id, &version).unwrap());
}

// ---- review round 15 ----

/// R15 A: shared-id facts compare as stored, exactly: 100000000.0000001 against 100000000.0000002 is a conflict.
#[test]
fn r15_shared_trades_compare_exactly() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    trades(c, &two, 0, 3, 0, "1");
    trades(c, &two, 1, 3, 0, "1");
    for (n, amount) in [(0, 100000000.0000001f64), (1, 100000000.0000002f64)] {
        let row = two.trade(c, n, 0, "near", "1", "2026-09-05 10:00:00");
        c.execute("UPDATE account_transactions SET base_amount = ?2 WHERE id = ?1", rusqlite::params![row, amount]).unwrap();
    }
    assert!(!overlaps(c, &two, 1), "a conflicting shared trade");
}

/// R15 B: one predicate decides a venue-native trade id: a synthetic MEXC id (`BTCUSDT--`) neither matches nor vetoes, and
/// an id-less Coinbase swap (`-in`/`-out`) is no trade.
#[test]
fn r15_only_venue_native_ids_count() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let mexc = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Mexc", "MEXC");
    for k in 0..2 { for i in 0..3 { mexc.trade(c, k, 0, &format!("BTCUSDT-{i}-{i}"), "1", &format!("2026-09-0{} 10:00:00", i + 1)); } }
    mexc.trade(c, 0, 0, "BTCUSDT--", "1", "2026-09-05 10:00:00");
    mexc.trade(c, 1, 0, "BTCUSDT--", "2", "2026-09-06 10:00:00");
    assert!(overlaps(c, &mexc, 1), "the synthetic collision does not veto");

    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let coinbase = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Coinbase", "Coinbase");
    for k in 0..2 {
        for t in ["s1", "s2"] { coinbase.trade(c, k, 0, t, "1", "2026-09-01 10:00:00"); }
        for leg in ["-in", "-out"] { coinbase.trade(c, k, 0, leg, "1", "2026-09-02 10:00:00"); }
    }
    assert!(!overlaps(c, &coinbase, 1), "two matches and an id-less swap are not three");
}

/// R15 C: one definition of blank, Unicode whitespace included: a tab-only group takes the duplicate's.
#[test]
fn r15_a_tab_only_group_is_blank() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let kept = two.row(c, 0, 0, "BTC", "1", Some("t-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET group_id = char(9) WHERE id = ?1", [kept]).unwrap();
    two.row(c, 1, 0, "BTC", "1", Some("t-1"), Some("usdt-leg"), "2026-09-01 10:00:00");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(one::<Option<String>>(c, &format!("SELECT group_id FROM account_transactions WHERE id = {kept}")).as_deref(), Some("usdt-leg"));
}

/// R15 E: a KuCoin fill withheld on one account and visible on the other is one fill: another fee is a conflict, an
/// identical copy is kept once.
#[test]
fn r15_withheld_and_visible_kucoin_fills_are_twins() {
    let pair = |c: &Connection| {
        let k = Two::kraken(c, &seed::cipher());
        let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
        two.fingerprint(c, 0, Some("fp"));
        two.fingerprint(c, 1, Some("fp"));
        c.execute("INSERT INTO kucoin_withheld_fills (account_id, tx_id, transacted_at, entry, created_at) VALUES (?1, 'BTC-USDT-1', '2026-09-01 10:00:00', '{\"createdAt\":1,\"fee\":\"0.1\"}', 'now')",
                  [two.id(0)]).unwrap();
        two
    };
    let (_d, o) = install();
    let c = &o.primary;
    let two = pair(c);
    let row = two.row(c, 1, 0, "BTC", "1", Some("BTC-USDT-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1,\"fee\":\"0.2\"}' WHERE id = ?1", [row]).unwrap();
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))), "another fee");

    let (_d, o) = install();
    let c = &o.primary;
    let two = pair(c);
    let row = two.row(c, 1, 0, "BTC", "1", Some("BTC-USDT-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1,\"fee\":\"0.1\"}' WHERE id = ?1", [row]).unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let copies = one::<i64>(c, "SELECT (SELECT count(*) FROM kucoin_withheld_fills WHERE tx_id = 'BTC-USDT-1') + (SELECT count(*) FROM account_transactions WHERE tx_id = 'BTC-USDT-1' OR id IN (SELECT account_transaction_id FROM exchange_account_rows WHERE venue_tx_id = 'BTC-USDT-1'))");
    assert_eq!(copies, 1, "kept once");
}

// ---- review round 16 ----

/// R16 #1: the duplicate's visible KuCoin copy of a fill the survivor holds withheld keeps its metadata: a manual price
/// and its bot order link survive the Merge.
#[test]
fn r16_a_visible_copy_of_a_withheld_fill_keeps_its_metadata() {
    let (_d, o) = install();
    let c = &o.primary;
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    c.execute("INSERT INTO kucoin_withheld_fills (account_id, tx_id, transacted_at, entry, created_at) VALUES (?1, 'BTC-USDT-1', '2026-09-01 10:00:00', '{\"createdAt\":1,\"fee\":\"0.1\"}', 'now')",
              [two.id(0)]).unwrap();
    let order = two.order(c, 1, "o-1", 1, Some(2));
    let row = two.row(c, 1, 0, "BTC", "1", Some("BTC-USDT-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1,\"fee\":\"0.1\"}', manual_values = '{\"price\":\"123\"}', transaction_id = ?2 WHERE id = ?1", [row, order]).unwrap();
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    let kept: Vec<(String, Option<i64>)> = c.prepare("SELECT manual_values, transaction_id FROM account_transactions WHERE manual_values LIKE '%123%'").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(kept.len(), 1, "the manual price survives");
    assert_eq!(kept[0].1, Some(order), "and the order link");
}

/// R16 #2: Merge re-checks the whole stored component: 2 and 3 are duplicates of 1; account 2's new balance contradicts
/// 1 (no reconsideration yet): merging 3 into 1 is refused.
#[test]
fn r16_merge_rechecks_the_whole_component() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let third = two.third(c).id.unwrap();
    let key = add_key(c, &two, third);
    common::two_accounts::confirm(c, third, Some("fp"));
    assert_eq!(standings(c), [(two.id(1), two.id(0)), (third, two.id(0))]);
    let at = now_text(-60);
    common::two_accounts::fresh(c, two.seeded.user_id, two.seeded.exchange_id, &at);
    for (a, k, units) in [(two.id(0), two.keys[0], "1"), (third, key, "1"), (two.id(1), two.keys[1], "7")] {
        common::two_accounts::read_balances(c, two.seeded.user_id, a, k, &[(two.seeded.btc, units)], &at);
    }
    let before = dump(c);
    assert!(matches!(merge_stale(c, &two, third), Ok(None)));
    assert_eq!(dump(c), before);
}

/// R16 #4: a restore's exact amount against an API twin's unquoted number, never through a float: 100000000000000000001
/// is not 100000000000000000000.
#[test]
fn r16_api_numbers_compare_from_their_text() {
    let (_d, o) = install();
    let c = &o.primary;
    let k = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &k.seeded, deltabadger::venue::kucoin::KUCOIN, "KuCoin");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    let api = two.row(c, 0, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"createdAt\":1,\"size\":100000000000000000001}' WHERE id = ?1", [api]).unwrap();
    let restore = two.row(c, 1, 0, "BTC", "1", Some("fill-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET raw_data = '{\"amounts\":{\"base\":\"100000000000000000000\"}}' WHERE id = ?1", [restore]).unwrap();
    assert!(matches!(merge_now(c, &two, two.id(1)), Err(Refusal::Conflict(_))));
}

// ---- review round 17 ----

/// R17 #4: a standing change removes the user's whole-portfolio points in its own transaction (the backfill rebuilds them
/// under the new standing): entering, and a key event that ends it.
#[test]
fn r17_a_standing_change_clears_the_all_chart() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let point = |c: &Connection| c.execute("INSERT INTO portfolio_snapshots (user_id, date, value_usd, invested_usd, partial, created_at, updated_at) VALUES (?1, '2026-09-19', 200, 200, 0, 'now', 'now')",
                                           [two.seeded.user_id]).unwrap();
    let points = |c: &Connection| one::<i64>(c, &format!("SELECT count(*) FROM portfolio_snapshots WHERE user_id = {}", two.seeded.user_id));
    point(c);
    two.fingerprint(c, 0, Some("fp"));
    assert_eq!(points(c), 1, "no standing yet");
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    assert_eq!(points(c), 0, "entering the standing");
    point(c);
    identity::write_status(c, two.keys[1], 2, "2026-09-21 10:00:00", true, false).unwrap();
    assert!(standings(c).is_empty());
    assert_eq!(points(c), 0, "the key event that ended it");
}

// ---- review round 18 ----

/// R18 #2: classification reads every row, a duplicate's included: a stock's price key is the same with and without the
/// standing, though the duplicate holds the only row recording a coin under its ticker.
#[test]
fn r18_classification_ignores_the_standing() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('ABC.US', 'ABC', 'ABC Inc', 'Stock', 'now', 'now')", []).unwrap();
    let stock = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('abc-coin', 'ABC', 'ABC coin', 'Cryptocurrency', 'now', 'now')", []).unwrap();
    let coin = c.last_insert_rowid();
    for (n, asset) in [(0, stock), (1, coin)] {
        let row = two.row(c, n, 0, "ABC", "1", Some(&format!("abc-{n}")), None, "2026-09-01 10:00:00");
        c.execute("UPDATE account_transactions SET base_asset_id = ?2 WHERE id = ?1", [row, asset]).unwrap();
    }
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    let key = |c: &Connection| deltabadger::tracker::prices::row_price_key(c, two.seeded.user_id, two.seeded.exchange_id, "Exchanges::Alpaca", "ABC", None, day).unwrap();
    let before = key(c);
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
    assert_eq!(key(c), before);
}

/// R18 #1: creating the install's identity key the first time deletes no standing: a Kraken overlap standing survives.
#[test]
fn r18_the_first_identity_key_keeps_the_standings() {
    let (_d, o, two) = entered();
    let c = &o.primary;
    identity::key(c, &seed::cipher(), chrono::Utc::now()).unwrap();
    assert_eq!(standings(c), [(two.id(1), two.id(0))]);
}

/// R18 #3: a moved row whose group the survivor already uses for another group is a conflict naming both: a restored swap
/// on 1 and a later synced swap on 2 under one group id.
#[test]
fn r18_a_moved_rows_group_must_be_free() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    two.row(c, 0, 2, "BTC", "1", Some("r-1"), Some("g1"), "2026-09-01 10:00:00");
    two.row(c, 0, 3, "USDT", "100", Some("r-2"), Some("g1"), "2026-09-01 10:00:00");
    for (id, kind, cur, amount) in [("s-1", 2, "ETH", "1"), ("s-2", 3, "USDT", "50")] {
        let row = two.row(c, 1, kind, cur, amount, Some(id), Some("g1"), "2026-09-10 10:00:00");
        synced(c, row);
    }
    match merge_now(c, &two, two.id(1)) { Err(Refusal::Conflict(rows)) => assert!(rows.len() >= 2, "{rows:?}"), other => panic!("{other:?}") }
}

// ---- review round 19 ----

/// R19 #3: the group collision check compares the walk's own composed group keys (`tracker::rows::account_group`) as they
/// will be after the move: account 3's `swap` moving into account 2 reads `swap@<2>`, which account 1's restored group
/// already is.
#[test]
fn r19_the_group_check_compares_composed_keys() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (a3, k3) = account_numbered(c, &two, 3);
    for (n, fp) in [(0, "other"), (1, "fp")] { two.fingerprint(c, n, Some(fp)); }
    common::two_accounts::confirm(c, a3, Some("fp"));
    assert_eq!(standings(c), [(a3, two.id(1))]);
    two.row(c, 0, 2, "BTC", "1", Some("r-1"), Some(&format!("swap@{}", two.id(1))), "2026-09-01 10:00:00");
    let scope = deltabadger::venue_ids::Scope::of(c, &identity::Account::by_id(c, a3).unwrap().unwrap()).unwrap();
    c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, transfer_link_rejected, group_id, created_at, updated_at) \
               VALUES (?1, ?2, ?3, 2, 'ETH', '1', ?4, '2026-09-10 10:00:00', json_object('refid', 's-1'), '{}', 0, 'swap', 'now', 'now')",
              rusqlite::params![two.seeded.user_id, k3, two.seeded.exchange_id, scope.stored(Some("s-1"))]).unwrap();
    deltabadger::venue_ids::after_insert(c, &scope, c.last_insert_rowid(), Some("s-1")).unwrap();
    match merge_now(c, &two, a3) { Err(Refusal::Conflict(rows)) => assert!(rows.len() >= 2, "{rows:?}"), other => panic!("{other:?}") }
}

/// R19 #4: the history version covers classification evidence, a duplicate's rows included: its new row moves it.
#[test]
fn r19_a_duplicates_row_moves_the_history_version() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let before = deltabadger::tracker::backfill::history_version(c, two.seeded.user_id).unwrap();
    let row = two.row(c, 1, 0, "ABC", "1", Some("abc-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET updated_at = '2026-09-30 10:00:00' WHERE id = ?1", [row]).unwrap();
    assert_ne!(deltabadger::tracker::backfill::history_version(c, two.seeded.user_id).unwrap(), before);
}

/// R19 #5: the live figures' class set reads every balance, a duplicate's included: the survivor's stock ABC beside the
/// duplicate's crypto ABC is ambiguous.
#[test]
fn r19_the_class_set_reads_a_duplicates_balances() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let asset = |category: &str, ext: &str| {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, 'ABC', 'ABC', ?2, 'now', 'now')", [ext, category]).unwrap();
        c.last_insert_rowid()
    };
    let (stock, coin) = (asset("Stock", "ABC.US"), asset("Cryptocurrency", "abc-coin"));
    for (n, id) in [(0, stock), (1, coin)] {
        c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at, account_id) \
                   VALUES (?1, ?2, ?3, 1, 0, 1, 1, '2026-09-20 10:00:00', '2026-09-20 10:00:00', '2026-09-20 10:00:00', '2026-09-20 10:00:00', ?4)", rusqlite::params![two.seeded.user_id, two.seeded.exchange_id, id, two.id(n)]).unwrap();
    }
    let balances = deltabadger::tracker::figures::balances(c, two.seeded.user_id, &deltabadger::tracker::Scope::All).unwrap();
    let fx = deltabadger::tracker::fx::FxTable::load(c).unwrap();
    let out = deltabadger::tracker::figures::compute_on(c, two.seeded.user_id, &deltabadger::tracker::Scope::All, &deltabadger::tracker::walk::Summary::empty(), &balances, &[], &fx, chrono::NaiveDate::from_ymd_opt(2026, 9, 20).unwrap());
    assert!(out.is_err(), "ambiguous");
}

// ---- slice 7a §0: 6b follow-ups ----

/// 7a §0.1: a kept twin that INHERITS the duplicate's group is checked by the same group key as a moved row: account 3's
/// `swap` on the twin of account 2's row reads as 2's `swap`, which account 1's restored group already is.
#[test]
fn s7_0_1_an_inherited_group_is_checked_like_a_moved_one() {
    let (_d, o) = install();
    let c = &o.primary;
    let two = Two::kraken(c, &seed::cipher());
    let (a3, k3) = account_numbered(c, &two, 3);
    for (n, fp) in [(0, "other"), (1, "fp")] { two.fingerprint(c, n, Some(fp)); }
    common::two_accounts::confirm(c, a3, Some("fp"));
    assert_eq!(standings(c), [(a3, two.id(1))]);
    two.row(c, 0, 2, "BTC", "1", Some("r-1"), Some(&format!("swap@{}", two.id(1))), "2026-09-01 10:00:00");
    // The survivor's (2's) row, no group; its twin on 3 carries `swap`.
    let kept = two.row(c, 1, 2, "ETH", "1", Some("s-1"), None, "2026-09-10 10:00:00");
    synced(c, kept);
    c.execute("UPDATE account_transactions SET raw_data = json_object('refid', 's-1') WHERE id = ?1", [kept]).unwrap();
    let scope = deltabadger::venue_ids::Scope::of(c, &identity::Account::by_id(c, a3).unwrap().unwrap()).unwrap();
    c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, transfer_link_rejected, group_id, created_at, updated_at) \
               VALUES (?1, ?2, ?3, 2, 'ETH', '1', ?4, '2026-09-10 10:00:00', json_object('refid', 's-1'), '{}', 0, 'swap', '2026-09-10 10:00:00', '2026-09-10 10:00:00')",
              rusqlite::params![two.seeded.user_id, k3, two.seeded.exchange_id, scope.stored(Some("s-1"))]).unwrap();
    deltabadger::venue_ids::after_insert(c, &scope, c.last_insert_rowid(), Some("s-1")).unwrap();
    match merge_now(c, &two, a3) { Err(Refusal::Conflict(rows)) => assert!(rows.len() >= 2, "{rows:?}"), other => panic!("{other:?}") }
}

/// 7a §0.2: the pricing walk's per-venue asset map is classification: a duplicate's row recording ABC as a stock decides
/// the survivor's unrecorded ABC on Alpaca (the catalogue alone holds a stock and a coin ABC), as the table's key does.
#[test]
fn s7_0_2_the_walks_asset_map_reads_a_duplicates_rows() {
    use deltabadger::tracker::prices::{NoFetch, PriceService, Reference, Venue};
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    two.fingerprint(c, 0, Some("fp"));
    two.fingerprint(c, 1, Some("fp"));
    identity::reconsider(c, two.seeded.user_id, two.seeded.exchange_id, chrono::Utc::now()).unwrap();
    assert_eq!(identity::duplicates(c, two.seeded.user_id).unwrap(), [(two.id(1), two.id(0))]);
    let asset = |category: &str, ext: &str| {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, 'ABC', 'ABC', ?2, 'now', 'now')", [ext, category]).unwrap();
        c.last_insert_rowid()
    };
    let stock = asset("Stock", "ABC.US");
    asset("Cryptocurrency", "abc-coin");
    two.row(c, 0, 0, "ABC", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    let d = two.row(c, 1, 0, "ABC", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET base_asset_id = ?2 WHERE id = ?1", [d, stock]).unwrap();
    c.execute("INSERT INTO historical_prices (asset, currency, date, price) VALUES ('stock:ABC', 'USD', '2026-09-01', '12')", []).unwrap();
    let user = two.seeded.user_id;
    let all = deltabadger::tracker::rows::load_all(c, user).unwrap();
    assert_eq!(all.iter().filter(|r| r.base == "ABC").count(), 1, "the duplicate's row is left out of the walk");
    let table = deltabadger::tracker::prices::row_price_key(c, user, two.seeded.exchange_id, "Exchanges::Alpaca", "ABC", None, chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()).unwrap();
    assert_eq!(table.as_deref(), Some("stock:ABC"), "the table's key reads the duplicate's row");
    let mut r = Reference::load(c, &Venue::alpaca(c).unwrap(), &deltabadger::tracker::prices::symbols(&all)).unwrap();
    r.record_coins(c, user).unwrap();
    let fx = deltabadger::tracker::fx::FxTable::default();
    let mut none = NoFetch;
    let mut ps = PriceService::new(&r, &fx, chrono::NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(), &mut none);
    ps.record(&all);
    let at = deltabadger::figures::at::At::from_sql("2026-09-01 12:00:00").unwrap();
    assert_eq!(ps.price_at("ABC", "USD", at, Some("alpaca")).unwrap().to_s_f(), "12.0", "{:?}", ps.warnings);
}

/// An asset `ABC` of `category` (external id `ext`).
fn abc(c: &Connection, category: &str, ext: &str) -> i64 {
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, 'ABC', 'ABC', ?2, 'now', 'now')", [ext, category]).unwrap();
    c.last_insert_rowid()
}
/// Account `account`'s balance of one `asset`.
fn holds(c: &Connection, user: i64, exchange: i64, account: i64, asset: i64) {
    c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at, account_id) \
               VALUES (?1, ?2, ?3, 1, 0, 1, 1, '2026-09-20 10:00:00', '2026-09-20 10:00:00', '2026-09-20 10:00:00', '2026-09-20 10:00:00', ?4)", rusqlite::params![user, exchange, asset, account]).unwrap();
}
fn figures_of(c: &Connection, user: i64, scope: &deltabadger::tracker::Scope, pending: &[(String, deltabadger::figures::dec::Dec)]) -> Result<deltabadger::tracker::figures::Figures, deltabadger::figures::FiguresError> {
    let balances = deltabadger::tracker::figures::balances(c, user, scope).unwrap();
    let fx = deltabadger::tracker::fx::FxTable::load(c).unwrap();
    deltabadger::tracker::figures::compute_on(c, user, scope, &deltabadger::tracker::walk::Summary::empty(), &balances, pending, &fx, chrono::NaiveDate::from_ymd_opt(2026, 9, 20).unwrap())
}

/// 7a §0.4: an account chip's class evidence reads duplicates of its own exchange only: Kraken's duplicate's crypto ABC
/// makes Binance's stock ABC no ambiguity on Binance's chip (it still does on All).
#[test]
fn s7_0_4_a_chips_class_evidence_reads_its_own_exchanges_duplicates() {
    let (_d, o, kraken) = duplicate();
    let c = &o.primary;
    let user = kraken.seeded.user_id;
    let binance = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Binance", "Binance");
    let (stock, coin) = (abc(c, "Stock", "ABC.US"), abc(c, "Cryptocurrency", "abc-coin"));
    holds(c, user, binance.seeded.exchange_id, binance.id(0), stock);
    holds(c, user, kraken.seeded.exchange_id, kraken.id(1), coin);
    let chip = deltabadger::tracker::Scope::Account(identity::AccountRow::by_id(c, binance.id(0)).unwrap().unwrap());
    assert!(figures_of(c, user, &chip, &[]).is_ok(), "{:?}", figures_of(c, user, &chip, &[]).err());
    assert!(figures_of(c, user, &deltabadger::tracker::Scope::All, &[]).is_err(), "All reads every duplicate");
}

/// 7a §0.5: on All a duplicate's balance class counts for a symbol only pending movements name: the survivor's rows
/// record ABC as a stock, the duplicate holds the coin ABC.
#[test]
fn s7_0_5_a_duplicates_class_counts_for_a_pending_only_symbol() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let user = two.seeded.user_id;
    let (stock, coin) = (abc(c, "Stock", "ABC.US"), abc(c, "Cryptocurrency", "abc-coin"));
    let row = two.row(c, 0, 0, "ABC", "1", Some("a-1"), None, "2026-09-01 10:00:00");
    c.execute("UPDATE account_transactions SET base_asset_id = ?2 WHERE id = ?1", [row, stock]).unwrap();
    holds(c, user, two.seeded.exchange_id, two.id(1), coin);
    let pending = [("ABC".to_string(), deltabadger::figures::dec::Dec::from_i64(1))];
    assert!(figures_of(c, user, &deltabadger::tracker::Scope::All, &pending).is_err(), "ambiguous");
}

/// 7a §0.6: an import's identity observation is its account's, and Merge moves the duplicate's to the survivor.
#[test]
fn s7_0_6_merge_moves_the_duplicates_import_observations() {
    let (_d, o, two) = duplicate();
    let c = &o.primary;
    let observed = c.execute("INSERT INTO exchange_account_imports (user_id, exchange_id, fingerprint, account_id_enc, observed_at, account_id) VALUES (?1, ?2, 'fp', x'00', 'now', ?3)",
              rusqlite::params![two.seeded.user_id, two.seeded.exchange_id, two.id(1)]);
    assert!(observed.is_ok(), "an observation names its account: {observed:?}");
    merge_now(c, &two, two.id(1)).unwrap().unwrap();
    assert_eq!(one::<i64>(c, &format!("SELECT count(*) FROM exchange_account_imports WHERE account_id = {}", two.id(0))), 1);
}

/// 7a §0.9: the Alpaca broker report's FIFO is per account (per depot, BMF Rn. 97), whatever the rows' keys: with both
/// keys deleted, account 2's sale consumes account 2's own lot (cost 200, gain 50), never account 1's earlier one (100).
#[test]
fn s7_0_9_the_broker_report_walks_each_account_on_its_own() {
    let (_d, o) = install();
    let c = &o.primary;
    let kraken = Two::kraken(c, &seed::cipher());
    let two = Two::venue(c, &seed::cipher(), &kraken.seeded, "Exchanges::Alpaca", "Alpaca");
    c.execute("INSERT INTO assets (external_id, symbol, name, category, instrument_type, created_at, updated_at) VALUES ('ACME.US', 'ACME', 'ACME', 'Stock', 'stock', 'now', 'now')", []).unwrap();
    let mut day = chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
    while chrono::Datelike::year(&day) == 2025 { c.execute("INSERT INTO fx_rates (currency, date, rate) VALUES ('USD', ?1, '1')", [day.to_string()]).unwrap(); day = day.succ_opt().unwrap(); }
    for (n, kind, usd, at) in [(0, 0, "100", "2025-01-10 15:00:00"), (1, 0, "200", "2025-02-10 15:00:00"), (1, 1, "250", "2025-03-10 15:00:00")] {
        let row = two.row(c, n, kind, "ACME", "1", Some(&format!("t-{at}")), None, at);
        c.execute("UPDATE account_transactions SET quote_currency = 'USD', quote_amount = ?2, api_key_id = NULL WHERE id = ?1", rusqlite::params![row, usd]).unwrap();
    }
    let csv = deltabadger::tax::broker::report(c, two.seeded.user_id, 2025, &std::collections::HashSet::new()).unwrap();
    let sale = csv.lines().find(|l| l.contains("2025-03-10")).unwrap_or_else(|| panic!("{csv}"));
    assert!(sale.contains("200") && sale.contains("50") && !sale.contains("150"), "{sale}\n{csv}");
}

