//! 3.0 plan A4 (PLAN-A4-cancel r4.2): cancel an open order from the bot page, inside the request, as
//! Bots::CancelOrdersController#destroy does. The request makes one cancel call and writes no order state; the engine's
//! re-read settles the row. Rails' answers to the same venue replies are pinned by the page grid (cancel_order_*).
mod common;
use common::scripted::ok;
use common::seed::{self, BotSpec};
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::engine::{model, FixedClock};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Hosts, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};
use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

const T0: &str = "2026-09-01T12:30:00Z";
fn at(s: &str) -> chrono::DateTime<chrono::Utc> { s.parse().unwrap() }
const TURBO: (&str, &str) = ("Accept", "text/vnd.turbo-stream.html, text/html, application/xhtml+xml");

struct Install { d: tempfile::TempDir, o: store::Opened, s: seed::Seeded, bot: i64, open: i64, imported: i64 }

/// A stopped Alpaca bot with one open limit buy (OPEN-1) and one open imported row; the owner can sign in.
fn install() -> Install {
    let (d, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let row = |ext: &str| seed::insert_row(&o.primary, &s, bot, s.btc, &json!({ "external_status": 1, "external_id": ext, "order_type": 1,
        "amount": "0.001", "price": "60000", "amount_exec": "0", "quote_amount_exec": "0", "created_at": "2026-09-01 10:00:01" }));
    let (open, imported) = (row("OPEN-1"), row("imported_1"));
    o.primary.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01',wash_sale_enabled=0", [deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    Install { d, o, s, bot, open, imported }
}

fn hosts(uri: &str) -> Hosts { Hosts { paper: uri.into(), live: uri.into(), data: uri.into() } }

async fn signed_in(i: &Install, uri: &str) -> (deltabadger::web::App, Browser) {
    let app = web::app(i.d.path(), "engine-test-secret", TestClock::at(T0))
        .with_settings_key_boundary(hosts(uri), deltabadger::web::settings::keys::logger()).unwrap();
    let browser = login(&app, i.bot).await;
    (app, browser)
}

async fn login(app: &deltabadger::web::App, bot: i64) -> Browser {
    let mut browser = Browser::default();
    browser.get(app, "/login").await;
    assert_eq!(browser.post(app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(browser.get(app, &format!("/bots/{bot}")).await.status, 200);
    browser
}

async fn venue(status: u16, body: Option<Value>) -> MockServer {
    let server = MockServer::start().await;
    let answer = body.map_or_else(|| ResponseTemplate::new(status), |b| ResponseTemplate::new(status).set_body_json(b));
    Mock::given(matchers::method("DELETE")).and(matchers::path("/v2/orders/OPEN-1")).respond_with(answer).mount(&server).await;
    server
}

async fn cancel(app: &deltabadger::web::App, browser: &mut Browser, path: &str) -> web::Answer {
    browser.send(app, "DELETE", path, Some(&[]), Csrf::Header, &[TURBO]).await
}

fn flash(style: &str, text: &str) -> String {
    format!("<turbo-stream action=\"prepend\" target=\"flash\"><template>{}</template></turbo-stream>",
        deltabadger::web::flash::render(&[deltabadger::web::flash::Message { style: if style == "primary" { "primary" } else { "danger" }, text: text.into() }]).unwrap().trim_end())
}

fn reread_keys(o: &store::Opened) -> Vec<String> {
    o.primary.prepare("SELECT key FROM app_configs WHERE key LIKE 'rust_cancel_reread:%' ORDER BY key").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
fn row_state(o: &store::Opened, id: i64) -> (Option<i64>, String) {
    o.primary.query_row("SELECT external_status, updated_at FROM transactions WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
}
fn activity(o: &store::Opened) -> i64 { o.primary.query_row("SELECT count(*) FROM bot_activity_logs", [], |r| r.get(0)).unwrap() }

struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _: &str, _: Option<deltabadger::crypto::Credentials>) -> Self::V { AlpacaVenue::new(self.0.clone(), Urls::for_passphrase(Some("paper"))) }
}
/// The engine refuses an install with an imported row left open (outside its slice), so that row goes first.
async fn engine_step(i: &Install, t: &ScriptedTransport) {
    i.o.primary.execute("DELETE FROM transactions WHERE id = ?1", [i.imported]).unwrap();
    let paths = store::Paths::from_env(&|_| None, i.d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    let mut engine = deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, Factory(t.clone()), seed::cipher(), lock);
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
}
fn cancelled_order() -> Value {
    ok(json!({ "id": "OPEN-1", "status": "canceled", "symbol": "BTC/USD", "type": "limit", "side": "buy", "qty": "0.001", "notional": null,
               "filled_qty": "0", "filled_avg_price": null, "limit_price": "60000" }))
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_answers_rails_notice_writes_no_order_state_and_the_engine_settles_the_row() {
    let i = install();
    let server = venue(204, None).await;
    let (app, mut browser) = signed_in(&i, &server.uri()).await;
    let before = (row_state(&i.o, i.open), activity(&i.o), model::load_bot(&i.o.primary, i.bot).unwrap().transient);
    let answer = cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.open)).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.body, flash("primary", "Order cancelled"));
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.len(), 1, "one cancel, nothing else");
    assert_eq!((sent[0].method.as_str(), sent[0].url.path()), ("DELETE", "/v2/orders/OPEN-1"));
    assert_eq!((row_state(&i.o, i.open), activity(&i.o), model::load_bot(&i.o.primary, i.bot).unwrap().transient), before, "the request writes no order state");
    assert_eq!(reread_keys(&i.o), vec![format!("rust_cancel_reread:{}", i.open)]);
    // The engine's re-read (FetchAndUpdateOrderJob) settles it, and the hand-off is gone.
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OPEN-1": [cancelled_order()] }));
    engine_step(&i, &t).await;
    assert!(t.requests().iter().any(|r| r.method == "GET" && r.path == "/v2/orders/OPEN-1"), "{:?}", t.requests());
    assert_eq!(row_state(&i.o, i.open).0, Some(deltabadger::enums::TxExternalStatus::Cancelled as i64));
    assert!(reread_keys(&i.o).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_refusal_and_an_invalid_key_answer_rails_failures_and_are_still_re_read() {
    let i = install();
    let name: String = i.o.primary.query_row("SELECT name FROM exchanges WHERE id = ?1", [i.s.exchange_id], |r| r.get(0)).unwrap();
    for (status, body, text) in [(422, json!({"code": 42210000, "message": "order is not cancelable"}), "Failed to cancel order".to_string()),
                                 (401, json!({"message": "unauthorized"}), format!("Failed to cancel order: Invalid API key. Please check your API key settings on {name}."))] {
        let server = venue(status, Some(body)).await;
        let (app, mut browser) = signed_in(&i, &server.uri()).await;
        let answer = cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.open)).await;
        assert_eq!((answer.status, answer.body), (422, flash("danger", &text)));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        // D-CANCEL-REREAD-ALWAYS
        assert_eq!(reread_keys(&i.o), vec![format!("rust_cancel_reread:{}", i.open)]);
    }
    assert_eq!(row_state(&i.o, i.open).0, Some(1));
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreachable_venue_is_rails_failure_flash_not_a_500() {
    // D-CANCEL-TRANSPORT-FAILURE: nothing listens on this port; Rails' controller would raise.
    let i = install();
    let (app, mut browser) = signed_in(&i, "http://127.0.0.1:9").await;
    let answer = cancel(&app, &mut browser, &format!("/de/bots/{}/transactions/{}", i.bot, i.open)).await;
    assert_eq!((answer.status, answer.body), (422, flash("danger", "Stornierung der Bestellung fehlgeschlagen")), "the owner's locale");
    assert_eq!(reread_keys(&i.o).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_guards_answer_as_botable_and_set_transaction_and_send_nothing() {
    let i = install();
    let server = venue(204, None).await;
    let (app, mut browser) = signed_in(&i, &server.uri()).await;
    // An imported row: Transaction#cancel refuses it; the controller shows the generic failure.
    let answer = cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.imported)).await;
    assert_eq!((answer.status, answer.body), (422, flash("danger", "Failed to cancel order")));
    // Another bot's row, and a row that is not there: @bot.transactions.find.
    let other = seed::insert_bot(&i.o.primary, &i.s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    assert_eq!(cancel(&app, &mut browser, &format!("/bots/{other}/transactions/{}", i.open)).await.status, 404);
    assert_eq!(cancel(&app, &mut browser, &format!("/bots/{}/transactions/999", i.bot)).await.status, 404);
    // No such bot, and a deleted one: Botable redirects to the list.
    for path in [format!("/bots/999/transactions/{}", i.open), "/bots/x/transactions/1".to_string()] {
        let answer = cancel(&app, &mut browser, &path).await;
        assert_eq!(answer.status, 302, "{path}");
    }
    i.o.primary.execute("UPDATE bots SET status = 3 WHERE id = ?1", [i.bot]).unwrap();
    let answer = cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.open)).await;
    assert_eq!(answer.status, 302);
    assert!(answer.headers.iter().any(|(k, v)| k == "location" && v.ends_with("/bots")), "{:?}", answer.headers);
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(reread_keys(&i.o).is_empty());
    // Signed out: authenticate_user!.
    let answer = Browser::default().send(&app, "DELETE", &format!("/bots/{}/transactions/{}", i.bot, i.open), Some(&[]), Csrf::None, &[TURBO]).await;
    assert_eq!(answer.status, 302, "Devise redirects to the sign-in page");
}

#[tokio::test(flavor = "current_thread")]
async fn a_slow_venue_holds_no_connection_another_request_needs() {
    let i = install();
    let server = MockServer::start().await;
    Mock::given(matchers::method("DELETE")).respond_with(ResponseTemplate::new(204).set_delay(std::time::Duration::from_secs(3))).mount(&server).await;
    let (app, mut browser) = signed_in(&i, &server.uri()).await;
    let mut other = login(&app, i.bot).await;
    let path = format!("/bots/{}/transactions/{}", i.bot, i.open);
    let slow = cancel(&app, &mut browser, &path);
    let page = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), other.get(&app, &format!("/bots/{}", i.bot))).await
    };
    let (slow, page): (web::Answer, Result<web::Answer, _>) = tokio::join!(slow, page);
    assert_eq!(slow.status, 200);
    assert_eq!(page.expect("the list must answer while the venue is slow").status, 200);
}

#[tokio::test(flavor = "current_thread")]
async fn a_re_read_left_by_a_crash_is_drained_at_the_next_start() {
    let i = install();
    deltabadger::app_config::set_plain(&i.o.primary, &format!("rust_cancel_reread:{}", i.open), &json!({ "bot": i.bot }).to_string(), at(T0)).unwrap();
    deltabadger::app_config::set_plain(&i.o.primary, "rust_cancel_reread:999", &json!({ "bot": i.bot }).to_string(), at(T0)).unwrap();
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OPEN-1": [cancelled_order()] }));
    engine_step(&i, &t).await;
    assert_eq!(row_state(&i.o, i.open).0, Some(deltabadger::enums::TxExternalStatus::Cancelled as i64));
    assert!(reread_keys(&i.o).is_empty(), "a key for a row that is gone goes too");
}

/// r4.2: a reversal whose own cancel of the same buy comes second is refused by the venue, and its reserve is dropped,
/// as Rails' failed flip cancel registers none.
#[tokio::test(flavor = "current_thread")]
async fn a_reversal_cancelling_after_the_user_is_refused_and_registers_no_reserve() {
    let i = install();
    let server = venue(204, None).await;
    let (app, mut browser) = signed_in(&i, &server.uri()).await;
    assert_eq!(cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.open)).await.status, 200);
    deltabadger::engine::reversal::record(&i.o.primary, i.bot, &[(i.open, true)], true).unwrap();
    let t = ScriptedTransport::from_script(&json!({
        "DELETE /v2/orders/OPEN-1": [{ "status": 422, "body": { "code": 42210000, "message": "order is not cancelable" } }],
        "GET /v2/orders/OPEN-1": [cancelled_order(), cancelled_order()] }));
    engine_step(&i, &t).await;
    let bot = model::load_bot(&i.o.primary, i.bot).unwrap();
    assert!(bot.transient.get("flip_reserve_pending").is_none(), "{}", bot.transient);
    let failed: i64 = i.o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event = 'order_cancel_failed'", [], |r| r.get(0)).unwrap();
    assert_eq!(failed, 1);
}
