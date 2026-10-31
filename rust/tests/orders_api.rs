//! PLAN-A9 slice O1: what the order routes and tools answer where Rails has no answer to record. The refusals before the
//! venue are compared with Rails on the page grid (api_orders_*, not_ported_api_orders_*) and the MCP control grid
//! (order_*); this file holds the named divergences: D-ORDER-AMOUNT-VALIDATION, D-ORDER-IBKR, and the `not_ported` answer
//! of a standalone request that passes every check, which gives its Idempotency-Key back.
//!
//! Slice O3 (the second half): an order through a signal bot, queued for the engine and waited for (D-ORDER-QUEUED,
//! D-ORDER-WAIT-CAP, D-ORDER-API-CAP), with the web app and the engine on one database and a scripted Kraken.
mod common;
use axum::{body::{to_bytes, Body}, http::Request};
use common::web::{self, TestClock};
use deltabadger::web::App;
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

const NOW: &str = "2026-09-01T12:30:00Z";
const T: &str = "2026-09-01 12:00:00";

/// The seeded Alpaca install (BTC/USD, a correct key), a working signal bot on it, Interactive Brokers with a correct key
/// and a BTC/USD listing, and a client holding an `api` and an `mcp` token with every order tool on.
fn install() -> (tempfile::TempDir, App, i64) {
    let (d, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute("INSERT INTO bots (type,user_id,exchange_id,status,label,settings,transient_data,started_at,created_at,updated_at) VALUES \
               ('Bots::Signal',?1,?2,1,'Signal',?3,'{}',?4,?4,?4)", (s.user_id, s.exchange_id, json!({"base_asset_id": s.btc, "quote_asset_id": s.quote}).to_string(), T)).unwrap();
    let bot = c.last_insert_rowid();
    c.execute("INSERT INTO exchanges (type,name,maker_fee,taker_fee,created_at,updated_at) VALUES ('Exchanges::Ibkr','Interactive Brokers','0.05','0.05',?1,?1)", [T]).unwrap();
    let ibkr = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id,ticker,base,quote,base_asset_id,quote_asset_id,base_decimals,quote_decimals,price_decimals,minimum_base_size,minimum_quote_size,available,trading_enabled,created_at,updated_at) \
               VALUES (?1,'BTCUSD','BTC','USD',?2,?3,8,2,2,'0.0001','1',1,1,?4,?4)", (ibkr, s.btc, s.quote, T)).unwrap();
    c.execute("INSERT INTO api_keys (user_id,exchange_id,key,secret,status,key_type,created_at,updated_at) VALUES (?1,?2,'k','s',1,0,?3,?3)", (s.user_id, ibkr, T)).unwrap();
    credentials(c, s.user_id);
    let app = web::app(d.path(), "orders-test-secret", TestClock::at(NOW));
    (d, app, bot)
}

fn credentials(c: &Connection, user: i64) {
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('orders','orders','http://localhost/cb',0,'mcp api',?1,?1)", [T]).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("orders-mcp", "mcp"), ("orders-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,?5,86400)", (application, user, token, scope, T)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,?3,?3)", (user, application, T)).unwrap();
    let on: serde_json::Map<String, Value> = ["market_buy", "market_sell", "limit_buy", "limit_sell", "cancel_order"].iter().map(|n| (n.to_string(), json!(true))).collect();
    c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1,wash_sale_enabled=0", [json!({"tool_permissions": on}).to_string()]).unwrap();
}

async fn send(app: &App, method: &str, path: &str, token: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (u16, Option<String>, Value) {
    let mut request = Request::builder().method(method).uri(path).header("host", "localhost:3000").header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
    for (k, v) in headers { request = request.header(*k, *v); }
    let response = deltabadger::web::router(app.clone()).oneshot(request.body(Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let session = response.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_string());
    let bytes = to_bytes(response.into_body(), 8_000_000).await.unwrap();
    (status, session, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn order(app: &App, body: Value, key: &str) -> (u16, Value) {
    let (status, _, body) = send(app, "POST", "/api/v1/orders", "orders-api", Some(body), &[("idempotency-key", key)]).await;
    (status, body)
}

fn keys(d: &tempfile::TempDir) -> Vec<(String, String)> {
    let c = Connection::open(d.path().join("production.sqlite3")).unwrap();
    let rows = c.prepare("SELECT key, state FROM idempotency_keys ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    rows
}

const UNPORTED_POST: &str = "POST /api/v1/orders is not served by this build yet.";

#[tokio::test(flavor = "current_thread")]
async fn rest_refuses_what_rails_hands_the_venue_and_answers_not_ported_standalone_past_every_check() {
    let (d, app, bot) = install();
    let pair = json!({"type": "market_buy", "exchange_name": "Alpaca", "base_asset": "BTC", "quote_asset": "USD"});
    let with = |extra: Value| { let mut b = pair.clone(); b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone()); b };
    // D-ORDER-AMOUNT-VALIDATION: BotOrder's refusals, after Rails' own checks.
    for (body, code, message) in [
        (with(json!({"amount": "abc"})), "invalid_number", "amount must be a number greater than 0."),
        (with(json!({"amount": -5})), "invalid_number", "amount must be a number greater than 0."),
        (with(json!({"amount": 10, "amount_type": "QUOTE"})), "invalid_amount_type", "amount_type must be 'quote' or 'base'."),
        (with(json!({"amount": 10, "amount_type": 5})), "invalid_amount_type", "amount_type must be 'quote' or 'base'."),
        (with(json!({"type": "limit_buy", "amount": 10, "price": "0"})), "invalid_number", "price must be a number greater than 0."),
    ] {
        let (status, answer) = order(&app, body.clone(), &uuid::Uuid::new_v4().to_string()).await;
        assert_eq!((status, answer), (422, json!({"data": null, "error": {"code": code, "message": message}})), "{body}");
    }
    let stored = keys(&d).len();
    // Past every check: 501, and the key is given back, so the same request may be sent again once a build places it.
    // D-ORDER-MARKET-PRICE: a market order's `price` is ignored (Rails' market services raise on it).
    for body in [with(json!({"amount": 10})), with(json!({"amount": 10, "price": 100})),
                 // D-ORDER-IBKR: Rails' checks pass on a venue this build does not trade.
                 json!({"type": "limit_buy", "exchange_name": "Interactive Brokers", "base_asset": "BTC", "quote_asset": "USD", "amount": 10, "price": 400})] {
        for _ in 0..2 {
            let (status, answer) = order(&app, body.clone(), "same-key").await;
            assert_eq!((status, answer), (501, json!({"data": null, "error": {"code": "not_ported", "message": UNPORTED_POST}})), "{body}");
        }
    }
    assert_eq!(keys(&d).len(), stored, "a not_ported answer claims no key");
    assert!(keys(&d).iter().all(|(_, state)| state == "completed"));
    // Through a bot with no engine attached: withdrawn at once, nothing queued; the answer is stored with the key.
    let (status, answer) = order(&app, json!({"type": "market_sell", "bot_id": bot, "amount": "0.5"}), "through-bot").await;
    assert_eq!((status, answer), (409, json!({"data": null, "error": {"code": "venue_busy", "message": "Alpaca is busy with another order from your bots. Nothing was placed; try again."}})));
    assert_eq!(keys(&d).last().unwrap(), &("through-bot".to_string(), "completed".to_string()));
    assert_eq!(Connection::open(d.path().join("production.sqlite3")).unwrap().query_row("SELECT count(*) FROM rust_api_orders", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    // A cancel past every check, by exchange id.
    let (status, _, answer) = send(&app, "DELETE", "/api/v1/orders/venue-1?exchange_name=alpaca", "orders-api", None, &[]).await;
    assert_eq!((status, answer["error"]["code"].clone()), (501, json!("not_ported")));
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_answers_venue_busy_with_no_engine_not_ported_standalone_and_on_ibkr_dry_runs() {
    let (d, app, bot) = install();
    let (_, session, _) = send(&app, "POST", "/mcp", "orders-mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "orders", "version": "1"}}})), &[]).await;
    let session = session.unwrap();
    let headers = [("mcp-session-id", session.as_str()), ("mcp-protocol-version", "2025-11-25")];
    assert_eq!(send(&app, "POST", "/mcp", "orders-mcp", Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})), &headers).await.0, 202);
    let call = |name: &'static str, arguments: Value| {
        let (app, headers) = (app.clone(), headers);
        async move {
            let (_, _, body) = send(&app, "POST", "/mcp", "orders-mcp", Some(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": arguments}})), &headers).await;
            body["result"].clone()
        }
    };
    let refused = |text: &str| json!({"isError": true, "content": [{"type": "text", "text": text}]});
    let placing = "This app can't run that yet: orders placed through MCP or the REST API";
    assert_eq!(call("market_buy", json!({"bot_id": bot, "amount": 10})).await, refused("Alpaca is busy with another order from your bots. Nothing was placed; try again."));
    assert_eq!(call("market_sell", json!({"exchange_name": "alpaca", "base_asset": "btc", "quote_asset": "usd", "amount": 0.001})).await, refused(placing));
    assert_eq!(call("cancel_order", json!({"order_id": "venue-1", "exchange_name": "Alpaca"})).await,
               refused("This app can't run that yet: cancels sent through MCP or the REST API"));
    // A wash-sale lock's last day is a calendar day before the deadline in the caller's zone: 2026-10-26 00:00 UTC is the
    // 25th at 23:00 in the Azores once summer time has ended, so the 24th (24 hours before would read the 25th).
    {
        let c = Connection::open(d.path().join("production.sqlite3")).unwrap();
        c.execute("UPDATE users SET wash_sale_enabled=1, wash_sale_jurisdiction='US', time_zone='Azores'", []).unwrap();
        c.execute("INSERT INTO wash_sale_locks (user_id,asset_id,buy_locked_until,source,created_at,updated_at) \
                   SELECT user_id, base_asset_id, '2026-10-26 00:00:00', 'order', ?1, ?1 FROM bots, tickers WHERE bots.id=?2 AND tickers.exchange_id=bots.exchange_id \
                   AND tickers.base_asset_id=json_extract(bots.settings,'$.base_asset_id') LIMIT 1", (T, bot)).unwrap();
    }
    assert_eq!(call("market_buy", json!({"bot_id": bot, "amount": 10})).await,
               refused("BTC is locked until 2026-10-24 after a sale at a loss. Change this in account settings."));
    // Paper trading, the rule off again: D-ORDER-AMOUNT-VALIDATION and D-ORDER-IBKR answer before the dry run would.
    Connection::open(d.path().join("production.sqlite3")).unwrap()
        .execute("UPDATE users SET wash_sale_enabled=0, mcp_settings=json_set(mcp_settings,'$.dry_run',json('true'))", []).unwrap();
    assert_eq!(call("market_buy", json!({"exchange_name": "Alpaca", "base_asset": "BTC", "quote_asset": "USD", "amount": -1})).await,
               refused("[DRY RUN] amount must be a number greater than 0."));
    assert_eq!(call("market_buy", json!({"exchange_name": "Interactive Brokers", "base_asset": "BTC", "quote_asset": "USD", "amount": 10})).await,
               refused("[DRY RUN] This app can't run that yet: orders through MCP or the REST API on this exchange"));
    assert_eq!(call("cancel_order", json!({"order_id": "x", "exchange_name": "Interactive Brokers"})).await,
               refused("[DRY RUN] This app can't run that yet: orders through MCP or the REST API on this exchange"));
}

// ---- O3: orders through a signal bot, placed by the engine ------------------------------------------------------------
use deltabadger::engine::{run::{self, Engine}, FixedClock};
use deltabadger::venue::fake::{AddOutcome, FakeFactory, FakeVenue};

const K0: &str = "2026-09-01T14:00:01Z";
fn kt(secs: i64) -> FixedClock { FixedClock(K0.parse::<chrono::DateTime<chrono::Utc>>().unwrap() + chrono::Duration::seconds(secs)) }

struct Kraken { d: tempfile::TempDir, app: App, e: Engine<FakeFactory>, venue: FakeVenue, bot: i64, user: i64 }
impl Kraken {
    fn c(&self) -> &Connection { &self.e.primary }
    fn count(&self, sql: &str) -> i64 { self.c().query_row(sql, [], |r| r.get(0)).unwrap() }
    fn rows(&self) -> Vec<(i64, Option<String>)> {
        self.c().prepare("SELECT status, external_id FROM transactions ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    }
}

/// A Kraken install (BTC/EUR, a correct key), a working signal bot with a buy rule, the client's tokens, the web app with
/// the engine attached (its clock at K0) and the engine on a fake Kraken priced at 49990.1 / 50000.2.
fn kraken(venue: FakeVenue) -> Kraken {
    let d = common::rails_install();
    let p = deltabadger::store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&p, "2026-09-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = deltabadger::store::open(&p).unwrap();
    let s = common::seed::seed_kraken(&o.primary, &common::seed::cipher());
    o.primary.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
                       VALUES ('Bots::Signal', 1, ?1, ?2, ?3, '{}', '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
                      (s.exchange_id, s.user_id, json!({"base_asset_id": s.btc, "quote_asset_id": s.quote}).to_string())).unwrap();
    let bot = o.primary.last_insert_rowid();
    o.primary.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, created_at, updated_at) \
                       VALUES (?1, 0, 0, '100', 1, 'tok-1', '2026-08-25 09:00:00', '2026-08-25 09:00:00')", [bot]).unwrap();
    o.primary.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1, time_zone='UTC'", []).unwrap();
    credentials(&o.primary, s.user_id);
    let e = Engine::new(o.primary, FakeFactory(venue.clone()), common::seed::cipher(), lock);
    let app = web::app(d.path(), "orders-test-secret", TestClock::at(K0));
    app.attach_engine(e.wake_handle());
    Kraken { d, app, e, venue, bot, user: s.user_id }
}
fn priced() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3").balance_body("ZEUR", "10000", "0") }

/// A request in flight: the HTTP call runs as its own task while the test drives the engine.
fn post(app: &App, body: Value, key: &str) -> tokio::task::JoinHandle<(u16, Value)> {
    let (app, key) = (app.clone(), key.to_string());
    tokio::spawn(async move { order(&app, body, &key).await })
}
/// Until the web has queued `n` requests (each request's db job runs on a blocking thread).
async fn queued(k: &Kraken, n: i64) {
    for _ in 0..2000 {
        if k.count("SELECT count(*) FROM rust_api_orders WHERE state='queued'") >= n { return }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("{n} requests never queued");
}
fn buy(k: &Kraken, amount: Value) -> Value { json!({"type": "market_buy", "bot_id": k.bot, "amount": amount}) }
fn refusal(status: u16, code: &str, message: &str) -> (u16, Value) { (status, json!({"data": null, "error": {"code": code, "message": message}})) }

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_bot_order_waits_for_one_engine_pass_and_answers_rails_bytes_then_replays() {
    let mut k = kraken(priced());
    let request = post(&k.app, buy(&k, json!("100")), "one");
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(0)).await.unwrap();
    let (status, body) = request.await.unwrap();
    let tx: i64 = k.c().query_row("SELECT id FROM transactions", [], |r| r.get(0)).unwrap();
    assert_eq!((status, body.clone()), (201, json!({"data": {"dry_run": false, "bot_id": k.bot, "transaction_id": tx, "order_id": "OFAKE-1", "exchange": "Kraken",
        "pair": "BTC/EUR", "side": "buy", "order_type": "market", "amount": "100", "amount_type": "quote"}, "error": null})));
    assert_eq!(k.venue.sent().len(), 1);
    // Kraken takes a quote buy as sent (#venue_takes?): 100 EUR, not its base at the ask.
    assert_eq!((k.venue.sent()[0].volume.as_str(), k.venue.sent()[0].quote_volume), ("100.0", true));
    assert_eq!(k.rows(), vec![(0, Some("OFAKE-1".into()))]);
    assert_eq!(k.count("SELECT count(*) FROM rust_api_orders"), 0, "answered and forgotten");
    // A replay after completion: the stored bytes, nothing queued, nothing sent.
    let (status, again) = order(&k.app, buy(&k, json!("100")), "one").await;
    assert_eq!((status, again), (201, body));
    assert_eq!((k.venue.sent().len(), k.count("SELECT count(*) FROM rust_api_orders")), (1, 0));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn webhook_calls_and_api_orders_on_one_bot_run_one_after_the_other_and_two_mcp_calls_place_two_orders() {
    let mut k = kraken(priced());
    let rule: i64 = k.c().query_row("SELECT id FROM bot_signals", [], |r| r.get(0)).unwrap();
    deltabadger::engine::model::append_signal_call(k.c(), k.bot, &deltabadger::engine::model::SignalCall { source: "hook:1".into(), signal_id: rule, triggered_at: K0.parse().unwrap() }).unwrap();
    let request = post(&k.app, buy(&k, json!("50")), "mixed");
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(0)).await.unwrap();
    assert_eq!(request.await.unwrap().0, 201);
    assert_eq!(k.venue.sent().len(), 2, "the API order and the webhook call, each placed once");
    assert_eq!(k.rows().len(), 2);
    // Two MCP calls on one bot: two orders, sequential.
    let (_, session, _) = send(&k.app, "POST", "/mcp", "orders-mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "orders", "version": "1"}}})), &[]).await;
    let session = session.unwrap();
    let initialized = [("mcp-session-id", session.as_str()), ("mcp-protocol-version", "2025-11-25")];
    assert_eq!(send(&k.app, "POST", "/mcp", "orders-mcp", Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})), &initialized).await.0, 202);
    let mcp = |id: i64| {
        let (app, session, bot) = (k.app.clone(), session.clone(), k.bot);
        tokio::spawn(async move {
            let headers = [("mcp-session-id", session.as_str()), ("mcp-protocol-version", "2025-11-25")];
            send(&app, "POST", "/mcp", "orders-mcp", Some(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": "market_buy", "arguments": {"bot_id": bot, "amount": 10}}})), &headers).await.2
        })
    };
    let (a, b) = (mcp(2), mcp(3));
    queued(&k, 2).await;
    run::step(&mut k.e, &kt(1)).await.unwrap();
    let texts: Vec<String> = [a.await.unwrap(), b.await.unwrap()].iter().map(|r| r["result"]["content"][0]["text"].as_str().unwrap().to_string()).collect();
    for text in &texts { assert!(text.starts_with(&format!("Market buy order placed by bot {} on Kraken: 10.0 EUR of BTC/EUR. Order OFAKE-", k.bot)), "{text}"); }
    assert_eq!(k.venue.sent().len(), 4);
    assert_eq!(k.rows().len(), 4, "never one order twice");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn one_idempotency_key_sent_twice_at_once_places_one_order() {
    let mut k = kraken(priced());
    let (a, b) = (post(&k.app, buy(&k, json!("100")), "race"), post(&k.app, buy(&k, json!("100")), "race"));
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(0)).await.unwrap();
    let mut answers = vec![a.await.unwrap(), b.await.unwrap()];
    answers.sort_by_key(|(status, _)| *status);
    assert_eq!(answers[0].0, 201);
    assert_eq!(answers[1], refusal(409, "idempotency_in_progress", "A request with this Idempotency-Key is already in progress."));
    assert_eq!(k.venue.sent().len(), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_request_no_pass_takes_within_ten_seconds_is_withdrawn_and_never_sent() {
    let mut k = kraken(priced());
    let (status, body) = order(&k.app, buy(&k, json!("100")), "late").await; // no step at all: the wait runs out
    assert_eq!((status, body), refusal(409, "venue_busy", "Kraken is busy with another order from your bots. Nothing was placed; try again."));
    run::step(&mut k.e, &kt(11)).await.unwrap();
    assert!(k.venue.sent().is_empty());
    assert_eq!(k.count("SELECT count(*) FROM rust_api_orders"), 0);
    assert_eq!(keys(&k.d), vec![("late".to_string(), "completed".to_string())]);
    // Take and withdraw: exactly one wins, in either order.
    use deltabadger::engine::api_orders;
    let bot = deltabadger::engine::model::load_bot(k.c(), k.bot).unwrap();
    let side = deltabadger::venue::Side::Buy;
    let amount = deltabadger::ruby::BigDec::parse("1").unwrap();
    let empty = serde_json::Map::new();
    let new = || api_orders::New { user_id: k.user, bot: &bot, side, amount: &amount, in_base: false, supplied: &empty };
    let first = api_orders::insert(k.c(), &new(), kt(0).0).unwrap();
    assert!(api_orders::take(k.c(), first, kt(0).0).unwrap().is_some());
    assert!(!api_orders::withdraw(k.c(), first).unwrap());
    let second = api_orders::insert(k.c(), &new(), kt(0).0).unwrap();
    assert!(api_orders::withdraw(k.c(), second).unwrap());
    assert!(api_orders::take(k.c(), second, kt(0).0).unwrap().is_none());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_request_taken_past_its_send_deadline_is_busy_and_never_sent() {
    let mut k = kraken(priced());
    let request = post(&k.app, buy(&k, json!("100")), "slow");
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(61)).await.unwrap(); // the engine's clock: 61 s after the request was written
    assert_eq!(request.await.unwrap(), refusal(409, "venue_busy", "Kraken is busy with another order from your bots. Nothing was placed; try again."));
    assert!(k.venue.sent().is_empty() && k.rows().is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_venue_that_never_answers_is_ambiguous_at_the_cap_and_the_engine_still_settles_the_order() {
    let gate = std::rc::Rc::new(tokio::sync::Notify::new());
    let mut k = kraken(priced().hold_add(gate.clone()));
    let started = tokio::time::Instant::now();
    let request = post(&k.app, buy(&k, json!("100")), "held");
    queued(&k, 1).await;
    let clock = kt(0);
    {
        let step = run::step(&mut k.e, &clock);
        tokio::pin!(step);
        let answer = tokio::select! { answer = request => answer.unwrap(), _ = &mut step => panic!("the held send finished") };
        assert_eq!(answer, refusal(502, "placement_ambiguous", "The order may or may not have been placed: the exchange did not answer in time. Check Kraken before sending it again."));
        assert!(started.elapsed() >= deltabadger::web::bot_api::orders::answer_cap(), "not before the last instant a send could start");
        gate.notify_one();
        step.await.unwrap();
    }
    assert_eq!(k.rows(), vec![(0, Some("OFAKE-1".into()))], "the engine records what the venue accepted");
    assert_eq!(k.count("SELECT count(*) FROM rust_api_orders WHERE state='done'"), 1, "answered; nobody reads it now");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_restart_withdraws_queued_requests_answers_taken_ones_busy_and_leaves_sending_ones_to_recovery() {
    let mut k = kraken(priced());
    use deltabadger::engine::api_orders;
    let bot = deltabadger::engine::model::load_bot(k.c(), k.bot).unwrap();
    let amount = deltabadger::ruby::BigDec::parse("100").unwrap();
    let empty = serde_json::Map::new();
    let new = || api_orders::New { user_id: k.user, bot: &bot, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: false, supplied: &empty };
    let (q, t, s) = (api_orders::insert(k.c(), &new(), kt(0).0).unwrap(), api_orders::insert(k.c(), &new(), kt(0).0).unwrap(), api_orders::insert(k.c(), &new(), kt(0).0).unwrap());
    api_orders::take(k.c(), t, kt(0).0).unwrap();
    api_orders::take(k.c(), s, kt(0).0).unwrap();
    api_orders::sending(k.c(), s, "cl-1").unwrap();
    api_orders::sweep_at_start(k.c(), kt(1).0).unwrap(); // what Engine::new does, before the web is attached
    run::step(&mut k.e, &kt(1)).await.unwrap();
    let state = |id: i64| k.c().query_row("SELECT state || ' ' || coalesce(json_extract(outcome, '$.kind'), '') FROM rust_api_orders WHERE id=?1", [id], |r| r.get::<_, String>(0)).unwrap();
    assert_eq!((state(q), state(t), state(s)), ("withdrawn ".into(), "done busy".into(), "done unknown".into()));
    assert!(k.venue.sent().is_empty(), "nothing taken again, nothing sent");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_client_that_hangs_up_mid_wait_still_leaves_its_key_completed() {
    let mut k = kraken(priced());
    let request = post(&k.app, buy(&k, json!("100")), "gone");
    queued(&k, 1).await;
    request.abort();
    run::step(&mut k.e, &kt(0)).await.unwrap();
    for _ in 0..200 {
        if keys(&k.d) == vec![("gone".to_string(), "completed".to_string())] { break }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert_eq!(keys(&k.d), vec![("gone".to_string(), "completed".to_string())]);
    let stored: String = k.c().query_row("SELECT response_body FROM idempotency_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap()["data"]["order_id"], "OFAKE-1");
    assert_eq!(k.venue.sent().len(), 1);
}

/// PLAN-A9 r1 §1, §4, §7 and r2 §1: what changes between validation and the send is asked again in the claim.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_engine_asks_again_what_changed_after_validation() {
    async fn once(k: &mut Kraken, body: Value, key: &str, clock: FixedClock) -> (u16, Value) {
        let request = post(&k.app, body, key);
        queued(k, 1).await;
        run::step(&mut k.e, &clock).await.unwrap();
        request.await.unwrap()
    }
    // Stopped during the price read: refused, nothing sent.
    let k0 = kraken(priced());
    let path = k0.d.path().join("production.sqlite3");
    let bot = k0.bot;
    let mut k = Kraken { venue: k0.venue.clone().on_price(move || { Connection::open(&path).unwrap().execute("UPDATE bots SET status=2 WHERE id=?1", [bot]).unwrap(); }), ..k0 };
    let body = buy(&k, json!("100"));
    assert_eq!(once(&mut k, body, "stop", kt(0)).await, refusal(409, "bot_not_running", &format!("Bot {bot} is not running. Start it to place orders through it.")));
    assert!(k.venue.sent().is_empty() && k.rows().is_empty());

    // A wash-sale lock armed during the price read: locked, nothing recorded.
    let k0 = kraken(priced());
    let path = k0.d.path().join("production.sqlite3");
    let user = k0.user;
    k0.c().execute("UPDATE users SET wash_sale_enabled=1, wash_sale_jurisdiction='US'", []).unwrap();
    let mut k = Kraken { venue: k0.venue.clone().on_price(move || {
        Connection::open(&path).unwrap().execute("INSERT INTO wash_sale_locks (user_id,asset_id,buy_locked_until,source,created_at,updated_at) \
            SELECT ?1, id, '2026-10-01 00:00:00', 'order', ?2, ?2 FROM assets WHERE symbol='BTC'", (user, T)).unwrap();
    }), ..k0 };
    let body = buy(&k, json!("100"));
    assert_eq!(once(&mut k, body, "lock", kt(0)).await, refusal(409, "wash_sale_locked", "BTC is locked after a sale at a loss."));
    assert!(k.venue.sent().is_empty() && k.rows().is_empty());

    // The key replaced in place between validation and the take: nothing placed under another key.
    let mut k = kraken(priced());
    let request = post(&k.app, buy(&k, json!("100")), "key");
    queued(&k, 1).await;
    k.c().execute("UPDATE api_keys SET secret=?1", [common::seed::cipher().encrypt("b3RoZXI=")]).unwrap();
    run::step(&mut k.e, &kt(0)).await.unwrap();
    assert_eq!(request.await.unwrap(), refusal(403, "api_key_missing", "No valid API key found for Kraken. Please add an API key in Settings."));
    assert!(k.venue.sent().is_empty());

    // The pair edited (stop, edit, start) between validation and the take, with the pair named: refused.
    let mut k = kraken(priced());
    let named = json!({"type": "market_buy", "bot_id": k.bot, "amount": "100", "base_asset": "btc"});
    let request = post(&k.app, named, "pair");
    queued(&k, 1).await;
    k.c().execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('ethereum','ETH','Ether','Cryptocurrency',?1,?1)", [T]).unwrap();
    let eth = k.c().last_insert_rowid();
    k.c().execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
                   SELECT exchange_id, 'ETHEUR', 'ETH', 'EUR', ?1, quote_asset_id, 8, 5, 1, '0.0001', '0.5', 1, 1, ?2, ?2 FROM tickers LIMIT 1", (eth, T)).unwrap();
    k.c().execute("UPDATE bots SET settings=json_set(settings,'$.base_asset_id',?1)", [eth]).unwrap();
    run::step(&mut k.e, &kt(0)).await.unwrap();
    assert_eq!(request.await.unwrap(), refusal(422, "bot_pair_mismatch", &format!("Bot {} trades ETH/EUR on Kraken; base_asset say otherwise. Leave them out to use the bot's pair.", k.bot)));
    assert!(k.venue.sent().is_empty());
}

/// D-ORDER-API-CAP (Jan, 2026-10-31): the spending cap applies to an API buy, refused and never clamped.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_spending_cap_refuses_an_api_buy_it_does_not_cover() {
    let mut k = kraken(priced());
    k.c().execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit',60), \
                   transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at',?1)", ["2026-08-25T09:00:00.000000Z"]).unwrap();
    let request = post(&k.app, buy(&k, json!("100")), "over");
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(0)).await.unwrap();
    assert_eq!(request.await.unwrap(), refusal(409, "quote_amount_limit_reached", &format!("Bot {} has 60 EUR left of its spending limit.", k.bot)));
    assert!(k.venue.sent().is_empty() && k.rows().is_empty());
    let request = post(&k.app, buy(&k, json!("60")), "within");
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(1)).await.unwrap();
    assert_eq!(request.await.unwrap().0, 201);
    assert_eq!(k.venue.sent().len(), 1);
}

/// A placement-safe rejection is placed once more (Claim::Replace): two order requests, one row.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_safe_rejection_is_placed_once_more_through_the_api_too() {
    let mut k = kraken(priced().next_add(AddOutcome::Reject(vec!["InvalidNonce".into()])).next_add(AddOutcome::Accept("OTX-2".into())));
    let request = post(&k.app, buy(&k, json!("100")), "nonce");
    queued(&k, 1).await;
    run::step(&mut k.e, &kt(0)).await.unwrap();
    let (status, body) = request.await.unwrap();
    assert_eq!((status, body["data"]["order_id"].clone()), (201, json!("OTX-2")));
    assert_eq!((k.venue.sent().len(), k.rows()), (2, vec![(0, Some("OTX-2".into()))]));
}

/// PLAN-A9 r2 §2: the send fence enforces an API order's `send_by`, whatever the intent's own 10 s window: an intent claimed
/// a second before it and sent after it sends nothing.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_send_fence_enforces_send_by() {
    use deltabadger::engine::{amount, model, placement};
    let k = kraken(priced());
    let bot = model::load_bot(k.c(), k.bot).unwrap();
    let ticker = model::ticker_for(k.c(), &bot).unwrap().unwrap();
    let price = deltabadger::ruby::BigDec::parse("50000.2").unwrap();
    let quote = deltabadger::ruby::BigDec::parse("100").unwrap();
    let (plan, _) = amount::size_order(&ticker, deltabadger::venue::Side::Buy, &price, &quote.div(&price).unwrap(), &quote,
                                       deltabadger::engine::venue_rules::for_exchange("Exchanges::Kraken").unwrap().minimum_logic, amount::TxKind::Regular).unwrap();
    let version = model::credential_version(k.c(), &bot).unwrap();
    let send_by = kt(1).0;
    // Past send_by at the claim itself: nothing written.
    assert!(matches!(placement::begin_signal(k.c(), &bot, &plan, placement::Claim::Replace { send_by: Some(send_by) }, &version, None, &kt(2)).unwrap(), placement::SignalClaim::Expired));
    assert!(model::load_bot(k.c(), k.bot).unwrap().rust_placement().is_none());
    let placement::SignalClaim::Intent(intent) = placement::begin_signal(k.c(), &bot, &plan, placement::Claim::Replace { send_by: Some(send_by) }, &version, None, &kt(0)).unwrap()
        else { panic!("no intent") };
    let sent = placement::send(k.c(), &k.venue, &intent, &kt(2)).await.unwrap();
    assert!(matches!(sent, placement::Sent::NotSent(_)), "{sent:?}");
    assert!(k.venue.sent().is_empty());
}
