//! 3.0 plan A4 (PLAN-A4-cancel r4.2): cancel an open order from the bot page, inside the request, as
//! Bots::CancelOrdersController#destroy does. The request makes one cancel call and writes no order state; the engine's
//! re-read settles the row. 2.x's answers to the same venue replies are pinned by the page grid (cancel_order_*).
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
/// A running engine (the imported row goes first: the engine refuses an install with one left open). Its start-up poll
/// of every open order reads OPEN-1 still open, so that poll backs off: only the cancel's hand-off can bring it forward.
async fn running_engine(i: &Install, t: &ScriptedTransport) -> deltabadger::engine::run::Engine<Factory> {
    i.o.primary.execute("DELETE FROM transactions WHERE id = ?1", [i.imported]).unwrap();
    let paths = store::Paths::from_env(&|_| None, i.d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    let mut engine = deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, Factory(t.clone()), seed::cipher(), lock);
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
    engine
}
async fn engine_step(i: &Install, t: &ScriptedTransport) { running_engine(i, t).await; }
fn reads(t: &ScriptedTransport) -> usize { t.requests().iter().filter(|r| r.method == "GET" && r.path == "/v2/orders/OPEN-1").count() }
fn open_order() -> Value {
    ok(json!({ "id": "OPEN-1", "status": "new", "symbol": "BTC/USD", "type": "limit", "side": "buy", "qty": "0.001", "notional": null,
               "filled_qty": "0", "filled_avg_price": null, "limit_price": "60000" }))
}
fn cancelled_order() -> Value {
    ok(json!({ "id": "OPEN-1", "status": "canceled", "symbol": "BTC/USD", "type": "limit", "side": "buy", "qty": "0.001", "notional": null,
               "filled_qty": "0", "filled_avg_price": null, "limit_price": "60000" }))
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_answers_v2_notice_writes_no_order_state_and_the_engine_settles_the_row() {
    let i = install();
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OPEN-1": [open_order(), cancelled_order()] }));
    let mut engine = running_engine(&i, &t).await;
    assert_eq!((reads(&t), row_state(&i.o, i.open).0), (1, Some(1)), "the start-up poll read it open and backs off");
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(reads(&t), 1, "without a cancel the backing-off poll is not due");
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
    // The hand-off brings the read forward to now, with a fresh budget: the same instant's pass reads and settles it.
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(reads(&t), 2);
    assert_eq!(row_state(&i.o, i.open).0, Some(deltabadger::enums::TxExternalStatus::Cancelled as i64));
    assert!(reread_keys(&i.o).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_refusal_and_an_invalid_key_answer_v2_failures_and_are_still_re_read() {
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
async fn an_unreachable_venue_is_v2_failure_flash_not_a_500() {
    // D-CANCEL-TRANSPORT-FAILURE: nothing listens on this port; 2.x's controller would raise.
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
async fn a_pending_re_read_is_drained_by_the_next_pass() {
    let i = install();
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OPEN-1": [open_order(), cancelled_order()] }));
    let mut engine = running_engine(&i, &t).await;
    // A hand-off committed by a request whose wake was lost (or before a restart): the next pass drains it.
    deltabadger::app_config::set_plain(&i.o.primary, &format!("rust_cancel_reread:{}", i.open), &json!({ "bot": i.bot }).to_string(), at(T0)).unwrap();
    deltabadger::app_config::set_plain(&i.o.primary, "rust_cancel_reread:999", &json!({ "bot": i.bot }).to_string(), at(T0)).unwrap();
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(reads(&t), 2);
    assert_eq!(row_state(&i.o, i.open).0, Some(deltabadger::enums::TxExternalStatus::Cancelled as i64));
    assert!(reread_keys(&i.o).is_empty(), "a key for a row that is gone goes too");
}

/// r4.2: a reversal whose own cancel of the same buy comes second is refused by the venue, and its reserve is dropped,
/// as 2.x's failed flip cancel registers none.
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


// ---- PLAN-A9 slice O2: cancel_order (MCP) and DELETE /api/v1/orders/:id (REST) send the cancel through the same core ----

/// A client of the install's owner with an `api` and an `mcp` token and cancel_order on.
fn api_client(c: &rusqlite::Connection) {
    const T: &str = "2026-09-01 12:00:00";
    let user: i64 = c.query_row("SELECT id FROM users WHERE email = 'o@example.com'", [], |r| r.get(0)).unwrap();
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('c','c','http://localhost/cb',0,'mcp api',?1,?1)", [T]).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("cancel-mcp", "mcp"), ("cancel-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,?5,86400)", (application, user, token, scope, T)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,?3,?3)", (user, application, T)).unwrap();
    c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1", [json!({"tool_permissions": {"cancel_order": true}}).to_string()]).unwrap();
}

async fn call(app: &deltabadger::web::App, method: &str, path: &str, token: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (u16, Option<String>, Value) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder().method(method).uri(path).header("host", "localhost:3000").header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
    for (k, v) in headers { request = request.header(*k, *v); }
    let response = deltabadger::web::router(app.clone()).oneshot(request.body(axum::body::Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let session = response.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), 8_000_000).await.unwrap();
    (status, session, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
async fn rest_cancel(app: &deltabadger::web::App, id: &str) -> (u16, Value) {
    let (status, _, body) = call(app, "DELETE", &format!("/api/v1/orders/{id}"), "cancel-api", None, &[]).await;
    (status, body)
}
async fn mcp_cancel(app: &deltabadger::web::App, arguments: Value) -> Value {
    let (_, session, _) = call(app, "POST", "/mcp", "cancel-mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}})), &[]).await;
    let session = session.unwrap();
    let headers = [("mcp-session-id", session.as_str()), ("mcp-protocol-version", "2025-11-25")];
    call(app, "POST", "/mcp", "cancel-mcp", Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})), &headers).await;
    let (_, _, body) = call(app, "POST", "/mcp", "cancel-mcp", Some(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "cancel_order", "arguments": arguments}})), &headers).await;
    body["result"].clone()
}
fn failed(message: &str) -> Value { json!({"data": null, "error": {"code": "cancel_failed", "message": message}}) }

#[tokio::test(flavor = "current_thread")]
async fn an_api_cancel_by_db_id_sends_one_cancel_leaves_the_re_read_and_the_engine_settles_the_row() {
    let i = install();
    api_client(&i.o.primary);
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OPEN-1": [open_order(), cancelled_order()] }));
    let mut engine = running_engine(&i, &t).await;
    let server = venue(204, None).await;
    let (app, _) = signed_in(&i, &server.uri()).await;
    let before = (row_state(&i.o, i.open), activity(&i.o));
    let (status, body) = rest_cancel(&app, &i.open.to_string()).await;
    assert_eq!((status, body), (200, json!({"data": {"dry_run": false, "id": i.open, "exchange": "Alpaca", "pair": "BTC/USD", "side": "buy", "cancelled": true}, "error": null})));
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.iter().map(|r| (r.method.to_string(), r.url.path().to_string())).collect::<Vec<_>>(), vec![("DELETE".into(), "/v2/orders/OPEN-1".into())]);
    assert_eq!((row_state(&i.o, i.open), activity(&i.o)), before, "the request writes no order state");
    assert_eq!(reread_keys(&i.o), vec![format!("rust_cancel_reread:{}", i.open)]);
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(row_state(&i.o, i.open).0, Some(deltabadger::enums::TxExternalStatus::Cancelled as i64));
    assert!(reread_keys(&i.o).is_empty());
    // MCP: the same path, 2.x's tool text.
    i.o.primary.execute("UPDATE transactions SET external_status = 1 WHERE id = ?1", [i.open]).unwrap();
    assert_eq!(mcp_cancel(&app, json!({"order_id": i.open.to_string()})).await,
               json!({"content": [{"type": "text", "text": format!("Order #{} (BUY BTC/USD on Alpaca) cancellation submitted.", i.open)}]}));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn an_api_cancel_by_exchange_id_uses_a_bot_rows_stored_order_and_sends_a_foreign_id_bare() {
    let i = install();
    api_client(&i.o.primary);
    let server = venue(204, None).await;
    Mock::given(matchers::method("DELETE")).and(matchers::path("/v2/orders/VENUE-9")).respond_with(ResponseTemplate::new(204)).mount(&server).await;
    let (app, _) = signed_in(&i, &server.uri()).await;
    // r3: a bot row's order id finds the row; it is cancelled and re-read (D-CANCEL-REREAD-ALWAYS), in 2.x's exchange shape.
    assert_eq!(rest_cancel(&app, "OPEN-1?exchange_name=alpaca").await,
               (200, json!({"data": {"dry_run": false, "external_id": "OPEN-1", "exchange": "Alpaca", "cancelled": true}, "error": null})));
    assert_eq!(reread_keys(&i.o), vec![format!("rust_cancel_reread:{}", i.open)]);
    i.o.primary.execute("DELETE FROM app_configs WHERE key LIKE 'rust_cancel_reread:%'", []).unwrap();
    // An id no row holds: the bare id, and no re-read.
    assert_eq!(mcp_cancel(&app, json!({"order_id": "VENUE-9", "exchange_name": "Alpaca"})).await,
               json!({"content": [{"type": "text", "text": "Order VENUE-9 cancellation submitted on Alpaca."}]}));
    assert!(reread_keys(&i.o).is_empty());
    // A numeric id that is a DB id keeps 2.x's precedence, even with an exchange named and another row holding it as an order id.
    i.o.primary.execute("UPDATE transactions SET external_id = ?1 WHERE id = ?2", (i.open.to_string(), i.imported)).unwrap();
    let (status, body) = rest_cancel(&app, &format!("{}?exchange_name=alpaca", i.open)).await;
    assert_eq!((status, &body["data"]["id"]), (200, &json!(i.open)), "{body}");
    // Another user's row is no DB id of this user's: the exchange path, which needs the exchange.
    i.o.primary.execute("INSERT INTO users (email, encrypted_password, created_at, updated_at) VALUES ('x@example.com', 'x', ?1, ?1)", [T0]).unwrap();
    i.o.primary.execute("UPDATE bots SET user_id = ?1 WHERE id = ?2", (i.o.primary.last_insert_rowid(), i.bot)).unwrap();
    assert_eq!(rest_cancel(&app, &i.open.to_string()).await.1["error"]["code"], "exchange_name_required");
    // D-ORDER-ID-SAFE: an id that would leave /v2/orders/ (here: a position's liquidation) is refused, nothing sent.
    // (REST: an encoded '/' in the path is the router's 404 before any service.)
    assert_eq!(rest_cancel(&app, "..%2Fpositions%2FQQQM?exchange_name=alpaca").await.0, 404);
    for id in ["../positions/QQQM", "a.b", "a?b", &"x".repeat(129)] {
        assert_eq!(mcp_cancel(&app, json!({"order_id": id, "exchange_name": "Alpaca"})).await["content"][0]["text"],
                   "order_id must be an exchange order id: letters, digits, '-' and '_'.", "{id}");
    }
    let paths: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| format!("{} {}", r.method, r.url.path())).collect();
    assert_eq!(paths, vec!["DELETE /v2/orders/OPEN-1", "DELETE /v2/orders/VENUE-9", "DELETE /v2/orders/OPEN-1"]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_api_cancel_the_venue_refuses_or_never_answers_is_cancel_failed() {
    let i = install();
    api_client(&i.o.primary);
    let server = venue(422, Some(json!({"code": 42210000, "message": "order is not cancelable"}))).await;
    let (app, _) = signed_in(&i, &server.uri()).await;
    let (status, body) = rest_cancel(&app, &i.open.to_string()).await;
    assert_eq!((status, &body["error"]["code"]), (502, &json!("cancel_failed")), "{body}");
    assert!(body["error"]["message"].as_str().unwrap().starts_with("Cancel failed: ") && body["error"]["message"].as_str().unwrap().contains("not cancelable"), "{body}");
    assert_eq!(reread_keys(&i.o).len(), 1, "D-CANCEL-REREAD-ALWAYS");
    // D-ORDER-CANCEL-TRANSPORT: nothing listens here; 2.x raises (a 500).
    let (app, _) = signed_in(&i, "http://127.0.0.1:9").await;
    for id in [i.open.to_string(), "VENUE-9?exchange_name=alpaca".to_string()] {
        let (status, body) = rest_cancel(&app, &id).await;
        assert_eq!((status, &body["error"]["code"]), (502, &json!("cancel_failed")), "{id}: {body}");
        assert!(body["error"]["message"].as_str().unwrap().len() > "Cancel failed: ".len(), "{body}");
    }
    // The imported row: Transaction#cancel's refusal, nothing sent.
    let (status, body) = rest_cancel(&app, &i.imported.to_string()).await;
    assert_eq!((status, body), (502, failed("Cancel failed: Cannot cancel imported orders")));
}

// ---- KuCoin (r2 §3, r3): a stored route beats a bare id; a bare id goes classic, or is refused on HF and UTA ----

const KUCOIN_MODE: &str = "/api/ua/v2/account/mode";

/// A KuCoin install (the Kraken seed renamed), a stopped bot with three open rows: HF-1 (an HF record), UTA-1 (a UTA
/// record) and CL-1 (classic, no record); the owner's API client. The records name `host` and the seeded key.
fn kucoin_install(host: &str) -> (tempfile::TempDir, store::Opened, i64, [i64; 3]) {
    let dir = common::baseline_install();
    let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute_batch("UPDATE exchanges SET type='Exchanges::Kucoin',name='KuCoin'; UPDATE tickers SET ticker='BTC-EUR',base='BTC'").unwrap();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let fingerprint = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"test-key"));
    let rows = ["HF-1", "UTA-1", "CL-1"].map(|ext| seed::insert_row(&o.primary, &s, bot, s.btc, &json!({ "external_status": 1, "external_id": ext, "order_type": 1,
        "amount": "0.001", "price": "60000", "amount_exec": "0", "quote_amount_exec": "0", "created_at": "2026-09-01 10:00:01" })));
    for (row, family) in [(rows[0], "ClassicHf"), (rows[1], "Uta")] {
        let record = json!({"family": family, "symbol": "BTC-EUR", "sell": false, "limit": true, "host": host, "key_fingerprint": fingerprint, "deal_size": "0", "deal_funds": "0"});
        deltabadger::app_config::set_plain(&o.primary, &format!("rust_kucoin_order:{row}"), &record.to_string(), at(T0)).unwrap();
    }
    api_client(&o.primary);
    (dir, o, bot, rows)
}
/// The account mode KuCoin answers from now on (classic, classic with HF opened, or unified), read afresh.
async fn kucoin_mode(server: &MockServer, o: &store::Opened, mode: &str) {
    server.reset().await;
    let (self_mode, opened) = match mode { "hf" => ("CLASSIC", true), "uta" => ("UNIFIED", false), _ => ("CLASSIC", false) };
    Mock::given(matchers::method("GET")).and(matchers::path(KUCOIN_MODE)).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": {"selfAccountMode": self_mode}}))).mount(server).await;
    Mock::given(matchers::method("GET")).and(matchers::path("/api/v1/hf/accounts/opened")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": opened}))).mount(server).await;
    for (method, path, id) in [("DELETE", "/api/v1/hf/orders/HF-1", "HF-1"), ("POST", "/api/ua/v2/unified/order/cancel", "UTA-1")] {
        Mock::given(matchers::method(method)).and(matchers::path(path)).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": {"orderId": id}}))).mount(server).await;
    }
    Mock::given(matchers::method("DELETE")).and(matchers::path_regex("^/api/v1/orders/")).respond_with(ResponseTemplate::new(200)
        .set_body_json(json!({"code": "200000", "data": {"cancelledOrderIds": ["x"]}}))).mount(server).await;
    let key: i64 = o.primary.query_row("SELECT id FROM api_keys", [], |r| r.get(0)).unwrap();
    deltabadger::venue::kucoin_account::Resolver::shared().invalidate(&server.uri(), key);
}
fn kucoin_app(d: &tempfile::TempDir, server: &MockServer) -> deltabadger::web::App {
    web::app(d.path(), "engine-test-secret", TestClock::at(T0)).with_venue_key_url(deltabadger::venue::kucoin::KUCOIN, &server.uri()).unwrap()
}
async fn sends(server: &MockServer) -> Vec<String> {
    server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}{}", r.method, r.url.path(), r.url.query().map(|q| format!("?{q}")).unwrap_or_default())).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn kucoin_bot_orders_by_exchange_id_go_on_their_stored_route_whatever_the_account_mode_now() {
    let server = MockServer::start().await;
    let (d, o, _bot, rows) = kucoin_install(&server.uri());
    for mode in ["hf", "uta", "classic"] {
        kucoin_mode(&server, &o, mode).await;
        // A fresh App each time: nothing in the process carries the route, the records do.
        let app = kucoin_app(&d, &server);
        for ext in ["HF-1", "UTA-1"] {
            let (status, body) = rest_cancel(&app, &format!("{ext}?exchange_name=kucoin")).await;
            assert_eq!((status, &body["data"]["cancelled"]), (200, &json!(true)), "{mode} {ext}: {body}");
        }
        assert_eq!(sends(&server).await, vec!["DELETE /api/v1/hf/orders/HF-1?symbol=BTC-EUR", "POST /api/ua/v2/unified/order/cancel"], "{mode}");
        assert_eq!(reread_keys(&o), vec![format!("rust_cancel_reread:{}", rows[0]), format!("rust_cancel_reread:{}", rows[1])], "{mode}");
        o.primary.execute("DELETE FROM app_configs WHERE key LIKE 'rust_cancel_reread:%'", []).unwrap();
    }
    // By DB id too (A4's OrderRef::load).
    kucoin_mode(&server, &o, "classic").await;
    assert_eq!(rest_cancel(&kucoin_app(&d, &server), &rows[0].to_string()).await.0, 200);
    assert_eq!(sends(&server).await, vec!["DELETE /api/v1/hf/orders/HF-1?symbol=BTC-EUR"]);
}

#[tokio::test(flavor = "current_thread")]
async fn kucoin_a_foreign_id_is_the_classic_cancel_on_a_classic_account_and_refused_unsent_on_hf_and_uta() {
    let server = MockServer::start().await;
    let (d, o, _bot, _rows) = kucoin_install(&server.uri());
    kucoin_mode(&server, &o, "classic").await;
    let app = kucoin_app(&d, &server);
    assert_eq!(rest_cancel(&app, "FOREIGN-1?exchange_name=KuCoin").await,
               (200, json!({"data": {"dry_run": false, "external_id": "FOREIGN-1", "exchange": "KuCoin", "cancelled": true}, "error": null})));
    // A classic bot row (no record) is the classic cancel too, re-read.
    assert_eq!(rest_cancel(&app, "CL-1?exchange_name=KuCoin").await.0, 200);
    assert_eq!(sends(&server).await, vec!["DELETE /api/v1/orders/FOREIGN-1", "DELETE /api/v1/orders/CL-1"]);
    for mode in ["hf", "uta"] {
        kucoin_mode(&server, &o, mode).await;
        // D-ORDER-KUCOIN-FOREIGN
        assert_eq!(rest_cancel(&kucoin_app(&d, &server), "FOREIGN-1?exchange_name=KuCoin").await,
                   (502, failed("Cancel failed: Order FOREIGN-1 was not placed through Deltabadger; cancel it on KuCoin.")), "{mode}");
        assert!(sends(&server).await.is_empty(), "{mode}: nothing sent");
    }
    // A mode KuCoin will not tell: refused, nothing sent.
    server.reset().await;
    Mock::given(matchers::method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "500000", "msg": "busy"}))).mount(&server).await;
    let (key, user, exchange): (i64, i64, i64) = o.primary.query_row("SELECT id, user_id, exchange_id FROM api_keys", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    deltabadger::identity::Account::first(&o.primary, user, exchange).unwrap().ensure(&o.primary, at(T0)).unwrap(); // a fingerprinted account has its row
    deltabadger::venue::kucoin_account::Resolver::shared().invalidate(&server.uri(), key);
    let (status, body) = rest_cancel(&kucoin_app(&d, &server), "FOREIGN-1?exchange_name=KuCoin").await;
    assert_eq!((status, &body["error"]["code"]), (502, &json!("cancel_failed")), "{body}");
    assert!(sends(&server).await.is_empty());
}

/// PLAN-A9 O4 r2 §3, r1 §4 (D-ORDER-STANDALONE-LOG): a standalone order this build placed keeps its route; a cancel by its
/// exchange id goes on that route in every account mode, from a fresh process, after the start-up sweep, and after its
/// ticker row is gone (the route is what was stored at the claim). No re-read is left: there is no row.
#[tokio::test(flavor = "current_thread")]
async fn kucoin_standalone_orders_by_exchange_id_go_on_their_logged_route_whatever_the_account_mode_now() {
    use deltabadger::engine::api_orders;
    let server = MockServer::start().await;
    let (d, o, _bot, _rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange, ticker): (i64, i64, i64) = c.query_row("SELECT k.user_id, t.exchange_id, t.id FROM api_keys k JOIN tickers t ON t.exchange_id=k.exchange_id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    let fingerprint = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"test-key"));
    let amount = deltabadger::v2::BigDec::parse("0.001").unwrap();
    let pair = serde_json::Map::new();
    for (ext, mode) in [("SHF-1", "ClassicHf"), ("SUTA-1", "Uta"), ("SCL-1", "ClassicLf")] {
        let id = api_orders::insert(c, &api_orders::New::Standalone { user_id: user, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: true, supplied: &pair,
            standalone: api_orders::Standalone { account: deltabadger::identity::Account::first(c, user, exchange).unwrap(), exchange_id: exchange, ticker_id: ticker, limit: true, price: Some(deltabadger::v2::BigDec::parse("60000").unwrap()), mcp: false } }, at(T0)).unwrap();
        api_orders::take(c, id, at(T0)).unwrap().unwrap();
        let identity = json!({"host": server.uri(), "key_fingerprint": fingerprint, "kucoin_account_mode": mode});
        assert!(api_orders::claim(c, id, "cl", Some(&identity), &json!({"symbol": "BTC-EUR", "side": "buy", "limit": true})).unwrap());
        api_orders::finish(c, id, &api_orders::Outcome::Placed { order_id: ext.into() }).unwrap();
        api_orders::forget(c, id).unwrap();
    }
    api_orders::sweep_at_start(c, at(T0) + chrono::Duration::days(2)).unwrap();
    c.execute("DELETE FROM tickers", []).unwrap();
    for mode in ["hf", "uta", "classic"] {
        kucoin_mode(&server, &o, mode).await;
        Mock::given(matchers::method("DELETE")).and(matchers::path("/api/v1/hf/orders/SHF-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": {"orderId": "SHF-1"}}))).mount(&server).await;
        Mock::given(matchers::method("POST")).and(matchers::path("/api/ua/v2/unified/order/cancel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": {"orderId": "SUTA-1"}}))).with_priority(1).mount(&server).await;
        let app = kucoin_app(&d, &server);
        for ext in ["SHF-1", "SUTA-1", "SCL-1"] {
            let (status, body) = rest_cancel(&app, &format!("{ext}?exchange_name=kucoin")).await;
            assert_eq!((status, &body["data"]["cancelled"]), (200, &json!(true)), "{mode} {ext}: {body}");
        }
        assert_eq!(sends(&server).await, vec!["DELETE /api/v1/hf/orders/SHF-1?symbol=BTC-EUR", "POST /api/ua/v2/unified/order/cancel", "DELETE /api/v1/orders/SCL-1"], "{mode}");
        assert!(reread_keys(&o).is_empty(), "{mode}: no row, no re-read");
    }
}

/// #710: a standalone order placed on account 2 is queued, taken and logged on account 2, and its cancel by exchange id goes
/// out with account 2's key; with account 2's key incorrect it is refused, account 1's correct key notwithstanding.
#[tokio::test(flavor = "current_thread")]
async fn a_second_accounts_standalone_order_is_cancelled_with_that_accounts_key() {
    use deltabadger::{engine::api_orders, identity::Account};
    let server = MockServer::start().await;
    let (d, o, _bot, _rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange, ticker): (i64, i64, i64) = c.query_row("SELECT k.user_id, t.exchange_id, t.id FROM api_keys k JOIN tickers t ON t.exchange_id=k.exchange_id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [user, exchange]).unwrap();
    let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("account-2-key"), cipher.encrypt("dGVzdC1zZWNyZXQ="), cipher.encrypt("pass")]).unwrap();
    let key = c.last_insert_rowid();
    second.add_key(c, key).unwrap();
    let (amount, pair) = (deltabadger::v2::BigDec::parse("0.001").unwrap(), serde_json::Map::new());
    let id = api_orders::insert(c, &api_orders::New::Standalone { user_id: user, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: true, supplied: &pair,
        standalone: api_orders::Standalone { account: second, exchange_id: exchange, ticker_id: ticker, limit: true, price: Some(deltabadger::v2::BigDec::parse("60000").unwrap()), mcp: false } }, at(T0)).unwrap();
    assert_eq!(api_orders::take(c, id, at(T0)).unwrap().unwrap().standalone.unwrap().account, second);
    assert!(api_orders::claim(c, id, "cl", None, &json!({"symbol": "BTC-EUR", "side": "buy", "limit": true})).unwrap());
    api_orders::finish(c, id, &api_orders::Outcome::Placed { order_id: "SCL-2".into() }).unwrap();
    assert_eq!(api_orders::log_order(c, user, exchange, "SCL-2", None).unwrap().unwrap().0, second);
    kucoin_mode(&server, &o, "classic").await;
    let app = kucoin_app(&d, &server);
    // #710 slice 6 (§4.3): with two live accounts a venue id names its account, even when only one holds it.
    assert_eq!(rest_cancel(&app, "SCL-2?exchange_name=kucoin").await.0, 422);
    let named = format!("SCL-2?exchange_name=kucoin&account_id={}", second.id.unwrap());
    let (status, body) = rest_cancel(&app, &named).await;
    assert_eq!((status, &body["data"]["cancelled"]), (200, &json!(true)), "{body}");
    let keys: Vec<String> = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "DELETE")
        .map(|r| r.headers.get("KC-API-KEY").unwrap().to_str().unwrap().to_string()).collect();
    assert_eq!(keys, ["account-2-key"]);
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [key]).unwrap();
    let (status, body) = rest_cancel(&app, &named).await;
    assert_eq!((status, body["error"]["code"].as_str()), (403, Some("api_key_missing")), "{body}");
}

/// #710 (Codex 6a): two accounts each logged standalone order `SCL`, account 2's the newer: the log read for account 1 finds
/// account 1's, never the newer one of account 2 that would hide it.
#[tokio::test(flavor = "current_thread")]
async fn a_standalone_order_id_both_accounts_logged_reads_the_named_accounts() {
    use deltabadger::{engine::api_orders, identity::Account};
    let server = MockServer::start().await;
    let (_d, o, _bot, _rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange, ticker): (i64, i64, i64) = c.query_row("SELECT k.user_id, t.exchange_id, t.id FROM api_keys k JOIN tickers t ON t.exchange_id=k.exchange_id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    let first = Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [user, exchange]).unwrap();
    let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    let (amount, pair) = (deltabadger::v2::BigDec::parse("0.001").unwrap(), serde_json::Map::new());
    for (account, symbol) in [(first, "BTC-EUR"), (second, "ETH-EUR")] {
        let id = api_orders::insert(c, &api_orders::New::Standalone { user_id: user, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: true, supplied: &pair,
            standalone: api_orders::Standalone { account, exchange_id: exchange, ticker_id: ticker, limit: true, price: Some(deltabadger::v2::BigDec::parse("60000").unwrap()), mcp: false } }, at(T0)).unwrap();
        api_orders::take(c, id, at(T0)).unwrap().unwrap();
        assert!(api_orders::claim(c, id, &format!("cl-{symbol}"), None, &json!({"symbol": symbol, "side": "buy", "limit": true})).unwrap());
        api_orders::finish(c, id, &api_orders::Outcome::Placed { order_id: "SCL".into() }).unwrap();
    }
    let read = |account: Option<i64>| api_orders::log_order(c, user, exchange, "SCL", account).unwrap().unwrap();
    assert_eq!(read(first.id).0, first);
    assert_eq!(read(first.id).2["symbol"], "BTC-EUR");
    assert_eq!(read(second.id).0, second);
    assert_eq!(read(None).0, second, "no account: the newest, as before");
}

/// #710: a bot's order is cancelled with the key of the account it was placed on, by database id or by exchange id, even
/// after the bot itself moved to another account: never that account's key, which could reach another order of that id.
#[tokio::test(flavor = "current_thread")]
async fn a_bots_order_is_cancelled_on_the_account_it_was_placed_on() {
    use deltabadger::identity::Account;
    let server = MockServer::start().await;
    let (d, o, bot, rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange): (i64, i64) = c.query_row("SELECT user_id, exchange_id FROM bots WHERE id = ?1", [bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let first = Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [user, exchange]).unwrap();
    let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("account-2-key"), cipher.encrypt("dGVzdC1zZWNyZXQ="), cipher.encrypt("pass")]).unwrap();
    second.add_key(c, c.last_insert_rowid()).unwrap();
    // The bot moved after placing its orders (account 1's: no membership row).
    deltabadger::identity::place_bot(c, bot, user, exchange, Some(second), at(T0)).unwrap();
    kucoin_mode(&server, &o, "classic").await;
    let app = kucoin_app(&d, &server);
    // #710 slice 6 (§4.3): a venue id names its account with two live accounts (the order's, account 1).
    for path in [rows[2].to_string(), format!("CL-1?exchange_name=kucoin&account_id={}", first.id.unwrap())] {
        let (status, body) = rest_cancel(&app, &path).await;
        assert_eq!(status, 200, "{path}: {body}");
    }
    let keys: Vec<String> = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "DELETE")
        .map(|r| r.headers.get("KC-API-KEY").unwrap().to_str().unwrap().to_string()).collect();
    assert_eq!(keys, ["test-key", "test-key"], "account 1's key, the orders'");
}

/// #710: a standalone order logged on account 2 is cancelled when account 2's key is correct, account 1's broken: the
/// logged order's account is found before any key is checked.
#[tokio::test(flavor = "current_thread")]
async fn a_second_accounts_standalone_order_needs_only_its_own_accounts_key() {
    use deltabadger::{engine::api_orders, identity::Account};
    let server = MockServer::start().await;
    let (d, o, _bot, _rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange, ticker, first_key): (i64, i64, i64, i64) = c.query_row("SELECT k.user_id, t.exchange_id, t.id, k.id FROM api_keys k JOIN tickers t ON t.exchange_id=k.exchange_id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [user, exchange]).unwrap();
    let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("account-2-key"), cipher.encrypt("dGVzdC1zZWNyZXQ="), cipher.encrypt("pass")]).unwrap();
    second.add_key(c, c.last_insert_rowid()).unwrap();
    let (amount, pair) = (deltabadger::v2::BigDec::parse("0.001").unwrap(), serde_json::Map::new());
    let id = api_orders::insert(c, &api_orders::New::Standalone { user_id: user, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: true, supplied: &pair,
        standalone: api_orders::Standalone { account: second, exchange_id: exchange, ticker_id: ticker, limit: true, price: Some(deltabadger::v2::BigDec::parse("60000").unwrap()), mcp: false } }, at(T0)).unwrap();
    api_orders::take(c, id, at(T0)).unwrap().unwrap();
    assert!(api_orders::claim(c, id, "cl", None, &json!({"symbol": "BTC-EUR", "side": "buy", "limit": true})).unwrap());
    api_orders::finish(c, id, &api_orders::Outcome::Placed { order_id: "SCL-3".into() }).unwrap();
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [first_key]).unwrap();
    kucoin_mode(&server, &o, "classic").await;
    let (status, body) = rest_cancel(&kucoin_app(&d, &server), &format!("SCL-3?exchange_name=kucoin&account_id={}", second.id.unwrap())).await;
    assert_eq!((status, &body["data"]["cancelled"]), (200, &json!(true)), "{body}");
}

/// #710 7a §9.3: a numeric order id with an explicit `account_id` looks the local order up only within that account. Account
/// 1's bot row id, named for account 2, is not account 1's order: it is a bare id cancelled through account 2's key.
#[tokio::test(flavor = "current_thread")]
async fn a_numeric_order_id_named_for_another_account_is_not_that_accounts_row() {
    use deltabadger::identity::Account;
    let server = MockServer::start().await;
    let (d, o, bot, rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange): (i64, i64) = c.query_row("SELECT user_id, exchange_id FROM bots WHERE id = ?1", [bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [user, exchange]).unwrap();
    let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    let cipher = seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, 'now', 'now')",
              rusqlite::params![user, exchange, cipher.encrypt("account-2-key"), cipher.encrypt("dGVzdC1zZWNyZXQ="), cipher.encrypt("pass")]).unwrap();
    second.add_key(c, c.last_insert_rowid()).unwrap();
    kucoin_mode(&server, &o, "classic").await;
    let app = kucoin_app(&d, &server);
    let (status, body) = rest_cancel(&app, &format!("{}?exchange_name=kucoin&account_id={}", rows[2], second.id.unwrap())).await;
    assert_eq!(status, 200, "{body}");
    let sent: Vec<(String, String)> = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "DELETE")
        .map(|r| (r.headers.get("KC-API-KEY").unwrap().to_str().unwrap().to_string(), r.url.path().to_string())).collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "account-2-key", "account 2's key, never account 1's row");
    assert!(sent[0].1.ends_with(&format!("/{}", rows[2])), "the id as a bare venue id: {sent:?}");
}

/// #710 7a §9.4: a standalone order the pre-flight recorded terminal never sends a cancel, after a key replacement too: the
/// cancel ends locally, saying so.
#[tokio::test(flavor = "current_thread")]
async fn a_standalone_order_recorded_terminal_sends_no_cancel() {
    use deltabadger::{engine::api_orders, identity::Account};
    let server = MockServer::start().await;
    let (d, o, _bot, _rows) = kucoin_install(&server.uri());
    let c = &o.primary;
    let (user, exchange, ticker): (i64, i64, i64) = c.query_row("SELECT k.user_id, t.exchange_id, t.id FROM api_keys k JOIN tickers t ON t.exchange_id=k.exchange_id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    let first = Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    let (amount, pair) = (deltabadger::v2::BigDec::parse("0.001").unwrap(), serde_json::Map::new());
    let id = api_orders::insert(c, &api_orders::New::Standalone { user_id: user, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: true, supplied: &pair,
        standalone: api_orders::Standalone { account: first, exchange_id: exchange, ticker_id: ticker, limit: true, price: Some(deltabadger::v2::BigDec::parse("60000").unwrap()), mcp: false } }, at(T0)).unwrap();
    api_orders::take(c, id, at(T0)).unwrap().unwrap();
    assert!(api_orders::claim(c, id, "cl", None, &json!({"symbol": "BTC-EUR", "side": "buy", "limit": true})).unwrap());
    api_orders::finish(c, id, &api_orders::Outcome::Placed { order_id: "SCL-9".into() }).unwrap();
    deltabadger::merge::record_terminal(c, id, "filled").unwrap();
    kucoin_mode(&server, &o, "classic").await;
    let (status, body) = rest_cancel(&kucoin_app(&d, &server), "SCL-9?exchange_name=kucoin").await;
    assert_ne!(status, 200, "{body}");
    assert!(body.to_string().contains("already filled"), "{body}");
    assert!(server.received_requests().await.unwrap().iter().all(|r| r.method.as_str() != "DELETE"), "nothing sent");
}

/// #710 7a: fence the install's one trading key: its account knows fingerprint `fa`, the key's latest identity read names
/// `fb` (`identity::fenced_sql`).
fn fence_the_key(c: &rusqlite::Connection) {
    let (key, user, exchange): (i64, i64, i64) = c.query_row("SELECT id, user_id, exchange_id FROM api_keys WHERE key_type = 0", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    let account = deltabadger::identity::Account::first(c, user, exchange).unwrap().ensure(c, at(T0)).unwrap();
    c.execute("UPDATE exchange_accounts SET fingerprint = 'fa' WHERE id = ?1", [account.id.unwrap()]).unwrap();
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, fingerprint, observed_at) VALUES (?1, ?2, 'fb', ?3)", rusqlite::params![key, exchange, T0]).unwrap();
    let fenced = deltabadger::identity::fenced_sql(c, "k").unwrap();
    assert!(c.query_row(&format!("SELECT {fenced} FROM api_keys k WHERE k.id = ?1"), [key], |r| r.get::<_, bool>(0)).unwrap(), "fenced");
}

/// #710 7a §9.1 (Codex branch r1, run.rs:876 and bot_api/orders.rs:540): a key whose identity read names another account
/// polls nothing and cancels nothing, whether from the bot page or the API by row or by exchange id: the venue could
/// hold another wallet's order with the same id. The order waits; nothing is sent, nothing is re-read.
#[tokio::test(flavor = "current_thread")]
async fn a_fenced_key_polls_and_cancels_nothing() {
    let i = install();
    api_client(&i.o.primary);
    fence_the_key(&i.o.primary);
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OPEN-1": [open_order(), cancelled_order()] }));
    let mut engine = running_engine(&i, &t).await;
    deltabadger::engine::run::step(&mut engine, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(reads(&t), 0, "no poll through the fenced key");
    let server = venue(204, None).await;
    let (app, mut browser) = signed_in(&i, &server.uri()).await;
    let name: String = i.o.primary.query_row("SELECT name FROM exchanges WHERE id = ?1", [i.s.exchange_id], |r| r.get(0)).unwrap();
    let answer = cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.open)).await;
    assert_eq!((answer.status, answer.body), (422, flash("danger", &format!("Failed to cancel order: Invalid API key. Please check your API key settings on {name}."))));
    let (status, body) = rest_cancel(&app, &i.open.to_string()).await;
    assert_eq!((status, body), (502, failed("Cancel failed: the API key reads another account")));
    let (status, body) = rest_cancel(&app, "OPEN-1?exchange_name=alpaca").await;
    assert_eq!((status, &body["error"]["code"]), (403, &json!("api_key_missing")), "{body}");
    assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
    assert!(reread_keys(&i.o).is_empty(), "nothing to re-read");
    assert_eq!(row_state(&i.o, i.open).0, Some(1), "the order waits, open");
}

/// Answers KuCoin's account mode (classic) and, as it answers, fences the install's trading key: discovery finding the key
/// opens another account while the cancel waits on that lookup.
struct FenceOnMode(std::path::PathBuf);
impl wiremock::Respond for FenceOnMode {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let c = rusqlite::Connection::open(&self.0).unwrap();
        fence_the_key(&c);
        ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": {"selfAccountMode": "CLASSIC"}}))
    }
}

/// Rule F3 (Codex branch r2, bot_api/orders.rs:555): a cancel re-checks the fence after its last await, right before it
/// sends. A bare KuCoin id waits on the account-mode lookup; a key fenced meanwhile sends nothing (A's order id never goes
/// through B's wallet), by exchange id and by a bot row's id alike.
#[tokio::test(flavor = "current_thread")]
async fn s7a_a_key_fenced_while_a_cancel_waits_sends_no_cancel() {
    let server = MockServer::start().await;
    let (d, o, _bot, _rows) = kucoin_install(&server.uri());
    let path = std::path::PathBuf::from(o.primary.path().unwrap());
    Mock::given(matchers::method("GET")).and(matchers::path(KUCOIN_MODE)).respond_with(FenceOnMode(path)).mount(&server).await;
    Mock::given(matchers::method("GET")).and(matchers::path("/api/v1/hf/accounts/opened")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": false}))).mount(&server).await;
    Mock::given(matchers::method("DELETE")).and(matchers::path_regex("^/api/v1/orders/")).respond_with(ResponseTemplate::new(200)
        .set_body_json(json!({"code": "200000", "data": {"cancelledOrderIds": ["x"]}}))).mount(&server).await;
    let (key, user, exchange): (i64, i64, i64) = o.primary.query_row("SELECT id, user_id, exchange_id FROM api_keys", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    deltabadger::identity::Account::first(&o.primary, user, exchange).unwrap().ensure(&o.primary, at(T0)).unwrap(); // a fingerprinted account has its row
    deltabadger::venue::kucoin_account::Resolver::shared().invalidate(&server.uri(), key);
    let (status, body) = rest_cancel(&kucoin_app(&d, &server), "FOREIGN-1?exchange_name=KuCoin").await;
    assert_eq!((status, body), (502, failed("Cancel failed: the API key reads another account")));
    assert!(sends(&server).await.is_empty(), "nothing sent");
}

/// Answers Alpaca's cancel yes and, as it answers, fences the install's trading key: discovery finding the key opens
/// another account while the cancel is out.
struct FenceOnCancel(std::path::PathBuf);
impl wiremock::Respond for FenceOnCancel {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        fence_the_key(&rusqlite::Connection::open(&self.0).unwrap());
        ResponseTemplate::new(204)
    }
}

/// Rule R1 (Codex branch r3, cancel.rs:36): a cancel's yes read after the key was found to open another account while the
/// cancel was out confirms nothing for this account: no success flash, no `cancelled: true`.
#[tokio::test(flavor = "current_thread")]
async fn s7a_r1_a_cancel_whose_key_is_fenced_while_it_is_out_is_not_confirmed() {
    let i = install();
    api_client(&i.o.primary);
    let (user, exchange): (i64, i64) = i.o.primary.query_row("SELECT user_id, exchange_id FROM api_keys WHERE key_type = 0", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    deltabadger::identity::Account::first(&i.o.primary, user, exchange).unwrap().ensure(&i.o.primary, at(T0)).unwrap(); // a fingerprinted account has its row
    let path = std::path::PathBuf::from(i.o.primary.path().unwrap());
    let server = MockServer::start().await;
    Mock::given(matchers::method("DELETE")).and(matchers::path("/v2/orders/OPEN-1")).respond_with(FenceOnCancel(path)).mount(&server).await;
    let (app, mut browser) = signed_in(&i, &server.uri()).await;
    let answer = cancel(&app, &mut browser, &format!("/bots/{}/transactions/{}", i.bot, i.open)).await;
    assert_eq!((answer.status, answer.body), (422, flash("danger", "Failed to cancel order")));
    // The REST cancel alike, the fence lifted before it and raised again while it is out.
    i.o.primary.execute("DELETE FROM exchange_account_credentials", []).unwrap();
    let (status, body) = rest_cancel(&app, &i.open.to_string()).await;
    assert_eq!((status, body), (502, failed("Cancel failed: the cancel could not be confirmed: the API key was found to open another account while it was sent")));
}
