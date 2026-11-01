//! IBKR placement (Ibkr plan I4b): the order through `Session::submit` with `cOID` and the reply whitelist, the sell
//! reservations (D-IBKR-SELLCAP), the bot-account checks, recovery by cOID, and the handback fence.
//!
//! Every IBKR answer here is provisional (tests/fixtures/ibkr_orders.json): the shapes Rails' parser expects, not recorded
//! from IBKR. The registry row cannot trade until a paper login has recorded them (`a_trading_ibkr_row_needs_paper_order_fixtures`).
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, Seeded};
use deltabadger::crypto::Cipher;
use deltabadger::engine::amount::{OrderPlan, Origin, TxKind};
use deltabadger::engine::model::{self, credential_version_by_id};
use deltabadger::engine::placement::{self, Begun, Recovery, Sent};
use deltabadger::engine::{polling, FixedClock};
use deltabadger::ruby::BigDec;
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::ibkr::{self, IBKR};
use deltabadger::venue::ibkr_account;
use deltabadger::venue::ibkr_reservations;
use deltabadger::venue::ibkr_venue::IbkrVenue;
use deltabadger::venue::{NewOrder, OrderKind, Side, Venue, VenueError};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;

const CONID: i64 = 265598;
const ACCOUNT_A: &str = "U1234567";
const ACCOUNT_B: &str = "U7654321";
const ORDER_ID: &str = "1799796559";
const ACCOUNTS: &str = "GET /v1/api/iserver/accounts";

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()).unwrap()
}

/// The fixture's answer `name`, each `"$key"` replaced by its value.
fn answer(name: &str, subs: &[(&str, Value)]) -> Value {
    let mut text = fixture("ibkr_orders.json")["answers"][name].to_string();
    assert_ne!(text, "null", "no answer {name}");
    for (k, v) in subs { text = text.replace(&format!("\"${k}\""), &v.to_string()); }
    serde_json::from_str(&text).unwrap()
}

/// Rails' recorded mint and brokerage-session answers, under the DH random they validate with.
fn mint_and_init() -> (Value, Value) {
    let v = fixture("ibkr_key_verdicts.json");
    ibkr::fix_dh_random(Some(v["ibkr"]["dh_random"].as_str().unwrap().into()));
    let one = v["ibkr"]["cases"].as_array().unwrap().iter().find(|c| c["case"] == "one_account").unwrap().clone();
    let reply = |a: &Value| json!({ "status": a["status"], "body": a["body"], "content_type": a["content_type"] });
    (reply(&one["responses"][0]), reply(&one["responses"][1]))
}

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const NOW: &str = "2026-10-26T14:30:00Z";

/// An install whose venue is IBKR: AAPL (conid 265598) in USD, a correct key bound to account A, `ibkr_conids` mapping it.
struct Fx { dir: tempfile::TempDir, c: Connection, cipher: Cipher, s: Seeded }

impl Fx {
    fn new() -> Self {
        let (dir, o, s) = common::install();
        let (c, cipher) = (o.primary, seed::cipher());
        c.execute("UPDATE exchanges SET type = ?1, name = 'Interactive Brokers' WHERE id = ?2", rusqlite::params![IBKR, s.exchange_id]).unwrap();
        let k = fixture("ibkr_test_keys.json");
        let e = |n: &str| cipher.encrypt(k[n].as_str().unwrap());
        let dh = fixture("ibkr_key_verdicts.json")["ibkr"]["dh_param"].as_str().unwrap().to_string();
        c.execute("UPDATE api_keys SET key=?1, secret=?2, access_token=?3, rsa_signature_key=?4, rsa_encryption_key=?5, dh_param=?6, ibkr_realm=?7, status=1 WHERE id=?8",
                  rusqlite::params![e("consumer_key"), e("access_token_secret"), e("access_token"), e("signature_key"), e("encryption_key"), cipher.encrypt(&dh),
                                    k["realm"].as_str().unwrap(), s.api_key_id]).unwrap();
        let asset = |ext: &str, symbol: &str, category: &str| {
            c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, '2026-01-01', '2026-01-01')", [ext, symbol, category]).unwrap();
            c.last_insert_rowid()
        };
        let (aapl, usd) = (asset("AAPL.US", "AAPL", "Stock"), asset("usd", "USD", "Currency"));
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
                   VALUES (?1, ?2, 'AAPL', 'USD', ?3, ?4, 0, 2, 2, '1', '1', 1, 1, '2026-01-01', '2099-01-01')", rusqlite::params![s.exchange_id, CONID.to_string(), aapl, usd]).unwrap();
        let ticker_id = c.last_insert_rowid();
        c.execute("INSERT INTO ibkr_conids (conid, asset_id, created_at) VALUES (?1, ?2, '2026-01-01')", rusqlite::params![CONID, aapl]).unwrap();
        ibkr_account::bind(&c, &cipher, s.api_key_id, ACCOUNT_A, at(NOW)).unwrap().unwrap();
        let s = Seeded { btc: aapl, quote: usd, ticker_id, ..s };
        Self { dir, c, cipher, s }
    }
    /// A bot selling and buying AAPL, belonging to the key's account.
    fn bot(&self) -> i64 {
        let id = seed::insert_bot(&self.c, &self.s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        ibkr_account::assign_bot_account(&self.c, id, at(NOW)).unwrap();
        id
    }
    fn account_ref(&self) -> String { ibkr_account::binding(&self.c, self.s.api_key_id).unwrap().unwrap().account_ref }
    /// IBKR's venue for `account` (as `account_ref`) over `t`, through the key's process-wide login, at `now`.
    fn venue_as(&self, t: &ScriptedTransport, account: &str, account_ref: &str, now: DateTime<Utc>) -> IbkrVenue<ScriptedTransport> {
        let credentials = deltabadger::sync::credentials(&self.c, &self.cipher, self.s.api_key_id).unwrap();
        let version = credential_version_by_id(&self.c, self.s.api_key_id).unwrap().unwrap();
        let login = ibkr::login(&version, credentials.ibkr.as_deref().unwrap()).unwrap();
        IbkrVenue::new(t.clone(), login, Arc::new(FixedClock(now)), account.into(), account_ref.into())
    }
    fn venue(&self, t: &ScriptedTransport) -> IbkrVenue<ScriptedTransport> { self.venue_as(t, ACCOUNT_A, &self.account_ref(), at(NOW)) }
    fn plan(&self, side: Side, quantity: i64) -> OrderPlan {
        let ticker = model::ticker_by_id(&self.c, self.s.exchange_id, self.s.ticker_id).unwrap().unwrap();
        let q = BigDec::from_i64(quantity);
        OrderPlan { ticker, side, limit: false, price: BigDec::from_i64(190), quote_amount: &q * &BigDec::from_i64(190), amount: q.clone(), quote_type: false, volume: q,
                    kind: TxKind::Regular, origin: if side == Side::Sell { Origin::Base } else { Origin::QuoteBudget }, wire_price: None }
    }
    fn one<T: rusqlite::types::FromSql>(&self, sql: &str) -> T { self.c.query_row(sql, [], |r| r.get(0)).unwrap() }
    /// (quantity, transaction_id, released) of every reservation, in order.
    fn reservations(&self) -> Vec<(String, Option<i64>, bool)> {
        let mut s = self.c.prepare("SELECT quantity, transaction_id, released_at IS NOT NULL FROM ibkr_reservations ORDER BY id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    }
}

/// IBKR's answers with `position` shares of AAPL held, the order `post` answers (each POST once, in order), each key of
/// `over` replacing its default answers.
fn script_over(position: i64, posts: &[Value], over: Value) -> ScriptedTransport {
    let (mint, init) = mint_and_init();
    let t = ScriptedTransport::default();
    let page = answer("positions_page", &[("conid", json!(CONID)), ("position", json!(position))]);
    let mut s = json!({
        "POST /v1/api/oauth/live_session_token": [mint], "POST /v1/api/iserver/auth/ssodh/init": [init],
        ACCOUNTS: [ok(answer("accounts", &[]))],
        "POST /v1/api/iserver/contract/rules": [ok(answer("rules", &[]))],
        "GET /v1/api/portfolio/U1234567/positions/0": [ok(page)],
        "GET /v1/api/portfolio/U1234567/positions/1": [ok(json!([]))],
    });
    for (k, v) in over.as_object().unwrap() { s[k] = v.clone(); }
    for (k, v) in s.as_object().unwrap() { for r in v.as_array().unwrap() { push(&t, k, r.clone()); } }
    for p in posts { push(&t, "POST /v1/api/iserver/account/U1234567/orders", p.clone()); }
    t
}
fn script(position: i64, posts: &[Value]) -> ScriptedTransport { script_over(position, posts, json!({})) }
fn push(t: &ScriptedTransport, key: &str, reply: Value) {
    match reply.get("status") {
        Some(_) if reply.get("content_type").is_some_and(|c| !c.is_null()) => { t.reply_typed(key, reply["status"].as_u64().unwrap() as u16, reply["content_type"].as_str().unwrap(), reply["body"].as_str().unwrap()); }
        Some(_) => { t.reply(key, reply["status"].as_u64().unwrap() as u16, reply["body"].clone()); }
        None => { t.network(key, reply["network"].as_str().unwrap(), reply["message"].as_str().unwrap()); }
    }
}
fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn ack(order_id: &str) -> Value { ok(answer("order_ack", &[("order_id", json!(order_id))])) }
fn status(order_id: &str, total: i64, filled: i64, state: &str) -> Value {
    answer("status", &[("order_id", json!(order_id)), ("conid", json!(CONID)), ("total", json!(total.to_string())), ("filled", json!(filled.to_string())), ("status", json!(state))])
}
fn posts(t: &ScriptedTransport, path: &str) -> Vec<Value> { t.requests().into_iter().filter(|r| r.method == "POST" && r.path == path).map(|r| r.body.unwrap_or(Value::Null)).collect() }
const ORDERS_PATH: &str = "/v1/api/iserver/account/U1234567/orders";

/// What placing `side` `quantity` for `bot` did: the hold, the intent (with the venue's identity), the send.
#[derive(Debug)]
enum Placed { Sent(Sent), Refused(String) }

async fn place(fx: &Fx, v: &IbkrVenue<ScriptedTransport>, bot: i64, side: Side, quantity: i64, now: DateTime<Utc>) -> Placed {
    let bot = model::load_bot(&fx.c, bot).unwrap();
    let plan = fx.plan(side, quantity);
    let clock = FixedClock(now);
    let hold = v.placement_hold(&plan.ticker, side).await.unwrap();
    let begun = placement::begin_held(&fx.c, &bot, &plan, v.placement_identity(), hold.as_ref(), &clock).unwrap();
    drop(hold);
    match begun {
        Begun::Intent(intent) => Placed::Sent(placement::send_recorded(&fx.c, v, &bot, &intent, &clock).await.unwrap()),
        Begun::Refused(why) => Placed::Refused(why),
        _ => panic!("not placed"),
    }
}
fn accepted(p: &Placed) -> bool { matches!(p, Placed::Sent(Sent::Accepted(id)) if id == ORDER_ID) }
fn refused(p: &Placed, why: &str) { match p { Placed::Refused(r) => assert!(r.contains(why), "{r}"), other => panic!("{other:?}") } }

/// The order's status as the sweep reads it.
async fn poll(fx: &Fx, bot: i64, answer: Value, now: DateTime<Utc>) {
    let t = script(0, &[]);
    t.reply(&format!("GET /v1/api/iserver/account/order/status/{ORDER_ID}"), 200, answer);
    polling::sweep(&fx.c, &fx.venue(&t), &model::load_bot(&fx.c, bot).unwrap(), now).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_sell_is_a_whole_share_market_day_order_carrying_its_intent_id_and_reserves_its_shares() {
    let fx = Fx::new();
    let a = fx.bot();
    let t = script(10, &[ack(ORDER_ID)]);
    assert!(accepted(&place(&fx, &fx.venue(&t), a, Side::Sell, 10, at(NOW)).await));
    let sent = posts(&t, ORDERS_PATH);
    assert_eq!(sent.len(), 1);
    let order = &sent[0]["orders"][0];
    let tx: i64 = fx.one("SELECT id FROM transactions WHERE external_id = '1799796559'");
    let c_oid = order["cOID"].as_str().unwrap().to_string();
    assert_eq!(c_oid.len(), 36, "the intent's id");
    assert_eq!(order, &json!({ "conid": CONID, "orderType": "MKT", "side": "SELL", "quantity": 10, "tif": "DAY", "cOID": c_oid }));
    assert_eq!(fx.reservations(), [("10".to_string(), Some(tx), false)], "written with the intent, linked to the row");
    let names: Vec<String> = t.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect();
    let post_at = names.iter().position(|n| n == &format!("POST {ORDERS_PATH}")).unwrap();
    assert_eq!(names[post_at - 1], ACCOUNTS, "the account list is read just before the POST");
    // A buy reserves nothing and reads no position.
    let t = script(10, &[ack("1799796560")]);
    let b = fx.bot();
    assert!(matches!(place(&fx, &fx.venue(&t), b, Side::Buy, 3, at(NOW)).await, Placed::Sent(Sent::Accepted(_))));
    assert!(!t.requests().iter().any(|r| r.path.contains("/positions/")));
    assert_eq!(posts(&t, ORDERS_PATH)[0]["orders"][0]["side"], "BUY");
    assert_eq!(fx.reservations().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_quantity_is_floored_to_the_contracts_size_increment() {
    let fx = Fx::new();
    let t = script_over(0, &[ack(ORDER_ID)], json!({ "POST /v1/api/iserver/contract/rules":
        [ok(json!({ "rules": { "sizeIncrement": 5, "incrementRules": [{ "lowerEdge": 0, "increment": 0.01 }], "priceMagnifier": 1 }, "source": "provisional" }))] }));
    let v = fx.venue(&t);
    let order = |volume: &str| NewOrder { pair: "8314".into(), side: Side::Buy, kind: OrderKind::Market, volume: volume.into(), quote_volume: false, cl_ord_id: "c-1".into(), deadline: at(NOW), day: false };
    assert_eq!(v.add_order(&order("7")).await, Ok(ORDER_ID.into()));
    assert_eq!(posts(&t, ORDERS_PATH)[0]["orders"][0]["quantity"], 5);
    assert_eq!(v.add_order(&order("4")).await, Err(VenueError::Rejected(vec!["Order below minimum (0 shares) for conid 8314".into()])));
    assert_eq!(posts(&t, ORDERS_PATH).len(), 1, "nothing sent under one increment");
}

#[tokio::test(flavor = "current_thread")]
async fn two_bots_on_one_conid_share_its_position() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), a, Side::Sell, 10, at(NOW)).await));
    let t = script(10, &[ack("1799796560")]);
    refused(&place(&fx, &fx.venue(&t), b, Side::Sell, 5, at(NOW)).await, "selling 5 shares of IBKR contract 265598 would exceed the 0 held and not already being sold");
    assert!(posts(&t, ORDERS_PATH).is_empty() && model::load_bot(&fx.c, b).unwrap().rust_placement().is_none(), "no intent, nothing sent");
    // A fills: released; the position now holds nothing.
    poll(&fx, a, status(ORDER_ID, 10, 10, "Filled"), at(NOW)).await;
    assert!(fx.reservations()[0].2, "released at Closed");
    refused(&place(&fx, &fx.venue(&script(0, &[])), b, Side::Sell, 1, at(NOW)).await, "exceed the 0 held");
}

#[tokio::test(flavor = "current_thread")]
async fn a_partial_fill_holds_only_what_is_unfilled() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), a, Side::Sell, 10, at(NOW)).await));
    poll(&fx, a, status(ORDER_ID, 10, 4, "Submitted"), at(NOW)).await;
    assert_eq!(fx.one::<f64>("SELECT amount_exec FROM transactions"), 4.0);
    // 6 held, 6 still to go.
    refused(&place(&fx, &fx.venue(&script(6, &[])), b, Side::Sell, 1, at(NOW)).await, "exceed the 0 held");
    // Cancelled after 4: the 6 are free again.
    poll(&fx, a, status(ORDER_ID, 10, 4, "Cancelled"), at(NOW)).await;
    assert!(accepted(&place(&fx, &fx.venue(&script(6, &[ack(ORDER_ID)])), b, Side::Sell, 6, at(NOW)).await));
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_racing_a_fill_releases_nothing_until_ibkr_settles_it() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), a, Side::Sell, 10, at(NOW)).await));
    let t = script(10, &[]);
    t.reply(&format!("DELETE /v1/api/iserver/account/U1234567/order/{ORDER_ID}"), 200, answer("cancel", &[("order_id", json!(ORDER_ID))]));
    assert_eq!(fx.venue(&t).cancel(ORDER_ID).await, Ok(()));
    // D-IBKR-PENDING-CANCEL: the cancel is not confirmed, the order can still fill: still held.
    poll(&fx, a, status(ORDER_ID, 10, 0, "PendingCancel"), at(NOW)).await;
    assert_eq!(fx.one::<i64>("SELECT external_status FROM transactions"), 1, "open");
    refused(&place(&fx, &fx.venue(&script(10, &[])), b, Side::Sell, 10, at(NOW)).await, "exceed the 0 held");
    // It filled before the cancel landed: closed, released, and the position is gone.
    poll(&fx, a, status(ORDER_ID, 10, 10, "Filled"), at(NOW)).await;
    refused(&place(&fx, &fx.venue(&script(0, &[])), b, Side::Sell, 10, at(NOW)).await, "exceed the 0 held");
}

#[tokio::test(flavor = "current_thread")]
async fn an_acknowledged_sell_still_holds_after_a_restart_before_its_first_poll() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), a, Side::Sell, 10, at(NOW)).await));
    // A new process: a fresh connection, a fresh login, nothing in memory.
    let restarted = Fx { c: Connection::open(fx.dir.path().join("production.sqlite3")).unwrap(), ..fx };
    ibkr::forget(restarted.s.api_key_id);
    assert_eq!(restarted.one::<i64>("SELECT external_status FROM transactions"), 0, "never polled");
    refused(&place(&restarted, &restarted.venue(&script(10, &[])), b, Side::Sell, 1, at(NOW) + Duration::minutes(5)).await, "exceed the 0 held");
}

#[tokio::test(flavor = "current_thread")]
async fn a_replaced_conid_keeps_the_outstanding_sells_reservation() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), a, Side::Sell, 10, at(NOW)).await));
    // The catalogue moves AAPL's listing to a new conid; the position moves with it.
    fx.c.execute("INSERT INTO ibkr_conids (conid, asset_id, created_at) VALUES (999001, ?1, '2026-10-26')", [fx.s.btc]).unwrap();
    fx.c.execute("UPDATE tickers SET ticker = '999001' WHERE id = ?1", [fx.s.ticker_id]).unwrap();
    let moved = |position: i64| script_over(0, &[ack("1799796561")],
        json!({ "GET /v1/api/portfolio/U1234567/positions/0": [ok(answer("positions_page", &[("conid", json!(999001)), ("position", json!(position))]))] }));
    refused(&place(&fx, &fx.venue(&moved(10)), b, Side::Sell, 10, at(NOW)).await, "shares of IBKR contract 999001 would exceed the 0 held");
    // The outstanding order is still polled by its id and settles; then the new conid's shares sell.
    poll(&fx, a, status(ORDER_ID, 10, 0, "Cancelled"), at(NOW)).await;
    let t = moved(10);
    assert!(matches!(place(&fx, &fx.venue(&t), b, Side::Sell, 10, at(NOW)).await, Placed::Sent(Sent::Accepted(_))));
    assert_eq!(posts(&t, ORDERS_PATH)[0]["orders"][0]["conid"], 999001);
    assert_eq!(fx.one::<String>("SELECT group_concat(conid) FROM (SELECT conid FROM ibkr_reservations ORDER BY id)"), "265598,999001");
}

#[tokio::test(flavor = "current_thread")]
async fn an_abandoned_order_keeps_its_reservation() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), a, Side::Sell, 10, at(NOW)).await));
    fx.c.execute("UPDATE transactions SET external_status = 4", []).unwrap();
    refused(&place(&fx, &fx.venue(&script(10, &[])), b, Side::Sell, 1, at(NOW)).await, "exceed the 0 held");
    assert!(!fx.reservations()[0].2);
    fx.c.execute("UPDATE bots SET status = 3", []).unwrap();
    assert_eq!(ibkr_reservations::handback_refusals(&fx.c, at(NOW)).unwrap(), ["IBKR reservation 1: shares held for an order that has not settled"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_declined_prompt_is_not_placed_and_frees_its_reservation() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    let t = script(10, &[ok(answer("prompt", &[]))]);
    let reply = "POST /v1/api/iserver/reply/07a13a5a-4a48-44a5-bb25-5ab37b79186c";
    t.reply(reply, 200, answer("declined", &[]));
    let placed = place(&fx, &fx.venue(&t), a, Side::Sell, 10, at(NOW)).await;
    assert!(matches!(&placed, Placed::Sent(Sent::Rejected(e)) if e == &["Order not confirmed: You are about to submit a market order. Are you sure you want to submit this order?".to_string()]), "{placed:?}");
    assert_eq!(posts(&t, "/v1/api/iserver/reply/07a13a5a-4a48-44a5-bb25-5ab37b79186c"), [json!({ "confirmed": false })], "D-IBKR-REPLY: no prompt is whitelisted yet");
    assert_eq!(fx.one::<i64>("SELECT status FROM transactions"), 1, "a failed row");
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), b, Side::Sell, 10, at(NOW)).await), "nothing was placed: the shares are free");
}

#[tokio::test(flavor = "current_thread")]
async fn a_401_on_the_reply_is_not_resent_and_recovers_by_the_intent_id() {
    let fx = Fx::new();
    let a = fx.bot();
    let t = script(10, &[ok(answer("prompt", &[]))]);
    let reply = "POST /v1/api/iserver/reply/07a13a5a-4a48-44a5-bb25-5ab37b79186c";
    t.reply(reply, 401, json!({ "error": "not authenticated" })).reply(reply, 200, answer("order_ack", &[("order_id", json!(ORDER_ID))]));
    let placed = place(&fx, &fx.venue(&t), a, Side::Sell, 10, at(NOW)).await;
    assert!(matches!(placed, Placed::Sent(Sent::Ambiguous(_))), "{placed:?}");
    assert_eq!((posts(&t, ORDERS_PATH).len(), posts(&t, "/v1/api/iserver/reply/07a13a5a-4a48-44a5-bb25-5ab37b79186c").len()), (1, 1), "never resent");
    let intent = model::load_bot(&fx.c, a).unwrap().rust_placement().unwrap();
    let c_oid = intent["cl_ord_id"].as_str().unwrap().to_string();
    assert_eq!(fx.reservations(), [("10".to_string(), None, false)], "held while the outcome is open");
    // Not in the live orders (asked twice) nor in /trades: it waits, however late.
    let lookup = |live: &Value, trades: Value| {
        let t = script(10, &[]);
        t.reply("GET /v1/api/iserver/account/orders", 200, live.clone());
        t.reply("GET /v1/api/iserver/account/trades", 200, trades);
        t
    };
    let empty = json!({ "orders": [], "snapshot": true, "source": "provisional" });
    let t = lookup(&empty, json!([]));
    let r = placement::recover_since(&fx.c, &fx.venue_as(&t, ACCOUNT_A, &fx.account_ref(), at(NOW) + Duration::days(3)), &model::load_bot(&fx.c, a).unwrap(), &FixedClock(at(NOW) + Duration::days(3)), at(NOW)).await.unwrap();
    assert!(matches!(r, Recovery::Pending), "absence is never provable");
    assert_eq!(t.requests().iter().filter(|r| r.path == "/v1/api/iserver/account/orders").count(), 2);
    // Its executions in /trades name it.
    let trades = answer("trades", &[("cOID", json!(c_oid)), ("order_id", json!(ORDER_ID)), ("conid", json!(CONID))]);
    let t = lookup(&empty, trades);
    let r = placement::recover_since(&fx.c, &fx.venue(&t), &model::load_bot(&fx.c, a).unwrap(), &FixedClock(at(NOW) + Duration::hours(1)), at(NOW)).await.unwrap();
    assert!(matches!(r, Recovery::Recorded(_)), "{r:?}");
    let tx: i64 = fx.one("SELECT id FROM transactions WHERE external_id = '1799796559'");
    assert_eq!(fx.reservations(), [("10".to_string(), Some(tx), false)]);
    assert!(model::load_bot(&fx.c, a).unwrap().rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_finds_the_order_in_the_second_live_orders_answer() {
    let fx = Fx::new();
    let a = fx.bot();
    let t = script(10, &[json!({ "network": "post_send", "message": "Net::ReadTimeout" })]);
    assert!(matches!(place(&fx, &fx.venue(&t), a, Side::Sell, 10, at(NOW)).await, Placed::Sent(Sent::Ambiguous(_))));
    let c_oid = model::load_bot(&fx.c, a).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    let t = script(10, &[]);
    t.reply("GET /v1/api/iserver/account/orders", 200, json!({ "orders": [], "snapshot": false, "source": "provisional" }))
     .reply("GET /v1/api/iserver/account/orders", 200, answer("live_orders", &[("cOID", json!(c_oid)), ("order_id", json!(ORDER_ID)), ("conid", json!(CONID))]));
    let r = placement::recover_since(&fx.c, &fx.venue(&t), &model::load_bot(&fx.c, a).unwrap(), &FixedClock(at(NOW)), at(NOW)).await.unwrap();
    assert!(matches!(r, Recovery::Recorded(_)), "{r:?}");
    assert!(!t.requests().iter().any(|r| r.path == "/v1/api/iserver/account/trades"));
    assert_eq!(fx.one::<i64>("SELECT external_status FROM transactions WHERE external_id = '1799796559'"), 1, "open, as listed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_limit_order_is_refused_by_name_and_never_sent() {
    let fx = Fx::new();
    let id = seed::insert_bot(&fx.c, &fx.s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.01)));
    let reasons = deltabadger::engine::eligibility::bot_reasons(&fx.c, &model::load_bot(&fx.c, id).unwrap()).unwrap();
    assert!(reasons.iter().any(|r| r == deltabadger::engine::eligibility::IBKR_NO_LIMIT), "{reasons:?}");
    let t = script(10, &[ack(ORDER_ID)]);
    let limit = NewOrder { pair: CONID.to_string(), side: Side::Buy, kind: OrderKind::Limit { price: "189.00".into() }, volume: "1".into(), quote_volume: false, cl_ord_id: "c-1".into(), deadline: at(NOW), day: false };
    assert_eq!(fx.venue(&t).add_order(&limit).await, Err(VenueError::Rejected(vec!["IBKR takes market orders only; a limit order is not sent".into()])));
    assert!(t.requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_of_another_account_is_refused_at_placement_and_its_order_is_not_looked_up_elsewhere() {
    let fx = Fx::new();
    let a = fx.bot();
    // An order left unresolved under account A.
    assert!(matches!(place(&fx, &fx.venue(&script(10, &[json!({ "network": "post_send", "message": "Net::ReadTimeout" })])), a, Side::Sell, 10, at(NOW)).await, Placed::Sent(Sent::Ambiguous(_))));
    // The connection is deleted and connected again to account B (same key row here: the binding goes with the delete).
    ibkr_account::forget(&fx.c, fx.s.api_key_id).unwrap();
    ibkr_account::bind(&fx.c, &fx.cipher, fx.s.api_key_id, ACCOUNT_B, at(NOW)).unwrap().unwrap();
    let b_ref = fx.account_ref();
    assert!(ibkr_account::bot_binding(&fx.c, &fx.cipher, a).unwrap().is_err(), "the engine builds it a venue that refuses every call");
    // Recovery: the intent went to A; B's venue is never asked.
    let t = ScriptedTransport::default();
    let r = placement::recover_since(&fx.c, &fx.venue_as(&t, ACCOUNT_B, &b_ref, at(NOW)), &model::load_bot(&fx.c, a).unwrap(), &FixedClock(at(NOW) + Duration::days(1)), at(NOW)).await.unwrap();
    assert!(matches!(r, Recovery::Pending));
    assert!(t.requests().is_empty());
    // Placement: a second bot of A, its intent read through B's venue, is refused in the intent's transaction.
    let other = seed::insert_bot(&fx.c, &fx.s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    fx.c.execute("INSERT INTO ibkr_bot_accounts (bot_id, account_ref, account_id_enc, created_at) SELECT ?1, account_ref, account_id_enc, created_at FROM ibkr_bot_accounts WHERE bot_id = ?2",
                 [other, a]).unwrap();
    let t = ScriptedTransport::default();
    t.reply(ACCOUNTS, 200, json!({ "accounts": [ACCOUNT_B], "source": "provisional" }));
    t.reply("GET /v1/api/portfolio/U7654321/positions/0", 200, answer("positions_page", &[("conid", json!(CONID)), ("position", json!(10))]))
     .reply("GET /v1/api/portfolio/U7654321/positions/1", 200, json!([]));
    let (mint, init) = mint_and_init();
    push(&t, "POST /v1/api/oauth/live_session_token", mint);
    push(&t, "POST /v1/api/iserver/auth/ssodh/init", init);
    refused(&place(&fx, &fx.venue_as(&t, ACCOUNT_B, &b_ref, at(NOW)), other, Side::Sell, 1, at(NOW)).await, "This bot belongs to another IBKR account");
    assert!(!t.requests().iter().any(|r| r.method == "POST" && r.path.ends_with("/orders")));
    assert!(model::load_bot(&fx.c, other).unwrap().rust_placement().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_venue_built_for_one_account_never_reads_a_position_of_another() {
    let fx = Fx::new();
    let t = script_over(10, &[], json!({ ACCOUNTS: [ok(json!({ "accounts": [ACCOUNT_B], "source": "provisional" }))] }));
    let ticker = fx.plan(Side::Sell, 1).ticker;
    let r = fx.venue(&t).placement_hold(&ticker, Side::Sell).await;
    assert_eq!(r.err(), Some(VenueError::Rejected(vec![ibkr::ACCOUNT_CHANGED.into()])));
    assert!(!t.requests().iter().any(|r| r.path.contains("/positions/")));
}

#[test]
fn the_handback_is_refused_while_ibkr_holds_anything() {
    let fx = Fx::new();
    let now = at(NOW);
    let refusals = || ibkr_reservations::handback_refusals(&fx.c, now).unwrap();
    assert!(refusals().is_empty(), "no IBKR state: nothing refused");
    let bot = fx.bot();
    assert_eq!(refusals(), [format!("bot {bot}: an IBKR bot; only this engine trades IBKR, so delete it before handing back")]);
    for status in [1, 2, 7] {
        fx.c.execute("UPDATE bots SET status = ?1", [status]).unwrap();
        assert_eq!(refusals().len(), 1, "status {status}");
    }
    fx.c.execute("UPDATE bots SET status = 3", []).unwrap();
    assert!(refusals().is_empty(), "a deleted bot holding nothing");
    let tx = seed::insert_tx(&fx.c, &fx.s, bot, &seed::TxSpec { status: 0, external_status: Some(1), external_id: Some("1".into()), order_type: 0, amount: Some("1"),
        quote_amount: None, price: Some("190"), quote_amount_exec: None, amount_exec: Some("0"), created_at: "2026-10-26 14:00:00".into() });
    assert_eq!(refusals(), [format!("transaction {tx}: an open IBKR order")]);
    fx.c.execute("UPDATE transactions SET external_status = 2", []).unwrap();
    fx.c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json('{\"cl_ord_id\":\"x\"}'))", []).unwrap();
    assert_eq!(refusals(), [format!("bot {bot}: an unresolved IBKR order")]);
    fx.c.execute("INSERT INTO ibkr_reservations (api_key_id, account_ref, conid, bot_id, cl_ord_id, transaction_id, quantity, created_at) VALUES (?1, 'r', ?2, ?3, 'x', NULL, '1', '2026-10-26')",
                 rusqlite::params![fx.s.api_key_id, CONID, bot]).unwrap();
    assert_eq!(refusals().len(), 2, "the intent and its reservation");
    // The intent goes without a row: nothing was placed, the reservation is released.
    fx.c.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement')", []).unwrap();
    assert!(refusals().is_empty());
    assert_eq!(fx.one::<i64>("SELECT count(*) FROM ibkr_reservations WHERE released_at IS NOT NULL"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn hand_back_stops_at_an_ibkr_bot() {
    use deltabadger::store::{self, Paths};
    let dir = common::baseline_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let l = deltabadger::lease::lock(&p, at(NOW)).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    deltabadger::engine::handover::take_over(&l, &o, &seed::cipher(), "0.2.0", at(NOW)).unwrap();
    o.primary.execute("UPDATE exchanges SET type = ?1 WHERE id = ?2", rusqlite::params![IBKR, s.exchange_id]).unwrap();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let factory = deltabadger::venue::fake::FakeFactory::default();
    let r = deltabadger::engine::handover::hand_back(&l, &o, &factory, &seed::cipher(), &FixedClock(at(NOW))).await;
    assert!(matches!(&r, Err(deltabadger::engine::EngineError::Ineligible(v)) if v == &[format!("bot {bot}: an IBKR bot; only this engine trades IBKR, so delete it before handing back")]), "{r:?}");
    o.primary.execute("UPDATE bots SET status = 3", []).unwrap();
    assert!(deltabadger::engine::handover::hand_back(&l, &o, &factory, &seed::cipher(), &FixedClock(at(NOW))).await.is_ok());
}

/// Gate G3: IBKR trades only on order answers recorded from a paper login. Flipping `trades` with provisional fixtures fails here.
#[test]
fn a_trading_ibkr_row_needs_paper_order_fixtures() {
    let answers = fixture("ibkr_orders.json")["answers"].as_object().unwrap().clone();
    assert!(!answers.is_empty());
    let mut sources = vec![];
    for (name, a) in &answers {
        let tagged: Vec<&Value> = match a { Value::Array(rows) => rows.iter().collect(), Value::Object(o) if !o.contains_key("source") => o.values().collect(), other => vec![other] };
        for t in tagged { sources.push((name.clone(), t["source"].as_str().unwrap_or_else(|| panic!("{name}: an answer without its source")).to_string())); }
    }
    let row = deltabadger::venue::registry::row(IBKR).unwrap();
    let paper = sources.iter().all(|(_, s)| s == "paper");
    assert!(!row.trades || paper, "IBKR trades only once every order answer is recorded on a paper login: {:?}", sources.iter().filter(|(_, s)| s != "paper").collect::<Vec<_>>());
    assert!(!row.trades, "while the fixtures are provisional (no paper login yet), IBKR does not trade");
    assert!(deltabadger::engine::venue_rules::for_exchange(IBKR).is_none() && deltabadger::engine::eligibility::admission(Some(IBKR), false, false).is_some(), "so the engine admits no IBKR bot");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_preflight_read_is_not_sent_and_frees_its_reservation() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    let t = script_over(10, &[ack(ORDER_ID)], json!({ "POST /v1/api/iserver/contract/rules": [{ "status": 200, "content_type": "application/json", "body": "{not json" }] }));
    let placed = place(&fx, &fx.venue(&t), a, Side::Sell, 10, at(NOW)).await;
    assert!(matches!(placed, Placed::Sent(Sent::NotSent(_))), "{placed:?}");
    assert!(posts(&t, ORDERS_PATH).is_empty(), "the order POST never left");
    assert!(model::load_bot(&fx.c, a).unwrap().rust_placement().is_none(), "nothing kept for recovery");
    assert!(accepted(&place(&fx, &fx.venue(&script(10, &[ack(ORDER_ID)])), b, Side::Sell, 10, at(NOW)).await), "the shares are free");
}

#[tokio::test(flavor = "current_thread")]
async fn an_order_recovered_from_trades_alone_records_its_fills_and_keeps_polling() {
    let fx = Fx::new();
    let (a, b) = (fx.bot(), fx.bot());
    let t = script(10, &[json!({ "network": "post_send", "message": "Net::ReadTimeout" })]);
    assert!(matches!(place(&fx, &fx.venue(&t), a, Side::Sell, 10, at(NOW)).await, Placed::Sent(Sent::Ambiguous(_))));
    let c_oid = model::load_bot(&fx.c, a).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    let t = script(10, &[]);
    let empty = json!({ "orders": [], "snapshot": true, "source": "provisional" });
    t.reply("GET /v1/api/iserver/account/orders", 200, empty.clone()).reply("GET /v1/api/iserver/account/orders", 200, empty);
    t.reply("GET /v1/api/iserver/account/trades", 200, answer("trades", &[("cOID", json!(c_oid)), ("order_id", json!(ORDER_ID)), ("conid", json!(CONID))]));
    let r = placement::recover_since(&fx.c, &fx.venue(&t), &model::load_bot(&fx.c, a).unwrap(), &FixedClock(at(NOW)), at(NOW)).await.unwrap();
    assert!(matches!(r, Recovery::Recorded(_)), "{r:?}");
    // The 4 shares /trades shows are credited; the row stays open for its polls to read the status.
    assert_eq!(fx.one::<f64>("SELECT amount_exec FROM transactions WHERE external_id = '1799796559'"), 4.0);
    assert_eq!(fx.one::<f64>("SELECT quote_amount_exec FROM transactions WHERE external_id = '1799796559'"), 760.4);
    assert_eq!(fx.one::<i64>("SELECT external_status FROM transactions WHERE external_id = '1799796559'"), 1, "open");
    // The reservation holds only the 6 unfilled: with 7 held, one share is free.
    refused(&place(&fx, &fx.venue(&script(7, &[])), b, Side::Sell, 2, at(NOW)).await, "exceed the 1 held");
    assert!(accepted(&place(&fx, &fx.venue(&script(7, &[ack(ORDER_ID)])), b, Side::Sell, 1, at(NOW)).await));
}
