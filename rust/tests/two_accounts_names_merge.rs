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

