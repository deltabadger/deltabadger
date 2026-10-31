//! PLAN-A9 slice O1: what the order routes and tools answer where Rails has no answer to record. The refusals before the
//! venue are compared with Rails on the page grid (api_orders_*, not_ported_api_orders_*) and the MCP control grid
//! (order_*); this file holds the named divergences: D-ORDER-AMOUNT-VALIDATION, D-ORDER-IBKR, and the `not_ported` answer
//! of an order that passes every check, which gives its Idempotency-Key back. Cancels past the checks are sent
//! (slice O2): tests/cancel_order.rs.
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
async fn rest_refuses_what_rails_hands_the_venue_and_answers_not_ported_past_every_check() {
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
    for body in [with(json!({"amount": 10})), with(json!({"amount": 10, "price": 100})), json!({"type": "market_sell", "bot_id": bot, "amount": "0.5"}),
                 // D-ORDER-IBKR: Rails' checks pass on a venue this build does not trade.
                 json!({"type": "limit_buy", "exchange_name": "Interactive Brokers", "base_asset": "BTC", "quote_asset": "USD", "amount": 10, "price": 400})] {
        for _ in 0..2 {
            let (status, answer) = order(&app, body.clone(), "same-key").await;
            assert_eq!((status, answer), (501, json!({"data": null, "error": {"code": "not_ported", "message": UNPORTED_POST}})), "{body}");
        }
    }
    assert_eq!(keys(&d).len(), stored, "a not_ported answer claims no key");
    assert!(keys(&d).iter().all(|(_, state)| state == "completed"));
    // D-ORDER-IBKR: a cancel past Rails' checks on a venue this build does not trade (a cancel elsewhere is sent: tests/cancel_order.rs).
    let (status, _, answer) = send(&app, "DELETE", "/api/v1/orders/venue-1?exchange_name=interactive%20brokers", "orders-api", None, &[]).await;
    assert_eq!((status, answer["error"]["code"].clone()), (501, json!("not_ported")));
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_answers_not_ported_live_orders_and_on_ibkr_dry_runs() {
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
    assert_eq!(call("market_buy", json!({"bot_id": bot, "amount": 10})).await, refused(placing));
    assert_eq!(call("market_sell", json!({"exchange_name": "alpaca", "base_asset": "btc", "quote_asset": "usd", "amount": 0.001})).await, refused(placing));
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
