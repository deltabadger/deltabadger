//! A28-3a (PLAN-A28-3 r1-r4, D-RAILS-UNRECORDED-LIVE-HISTORY, D-RAILS-UNRECORDED-OPEN-NAMED): the report job reads a served
//! venue's order history live once A28-1's line is due, and names the orders no bot recorded. Each case runs the real
//! Binance, Binance.US and Bybit venue over `http::ScriptedTransport` with hand-written bodies in the documented row shape
//! (no order-list body is recorded live anywhere, §1). A28-3b-1 adds Alpaca, its rows built on a recorded one
//! (tests/fixtures/order_lists/alpaca, Jan's paper account; each edit named in `alpaca_row`). A28-3c-3a adds Hyperliquid, its
//! rows and fills built on recorded ones (tests/fixtures/order_lists/hyperliquid, a public protocol address; each edit named
//! in `hl_row` / `hl_fill`); it runs the cases below whose orders are limits (`SERVED_ALL`) and its own: a Hyperliquid
//! order is always a limit sized in base. No live venue call. Every pass
//! is checked by the money guard.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, Seeded};
use deltabadger::engine::rails_unrecorded::{self as ru, Report};
use deltabadger::jobs::Db;
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::{alpaca, binance, bybit, hyperliquid, Venue, VenueFactory};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;

fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn now() -> DateTime<Utc> { t("2026-09-30T12:00:00Z") }
const AT: &str = "2026-09-29 14:03:00";
/// When the unrecorded orders were placed, inside the span ± 10 min.
const PLACED: &str = "2026-09-29T14:03:05Z";
const WHEN: &str = "around 2026-09-29 14:03 UTC";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Binance, BinanceUs, Bybit, Alpaca, Hyperliquid }
/// The venues every case runs on (market and limit orders).
const SERVED: [Kind; 4] = [Kind::Binance, Kind::BinanceUs, Kind::Bybit, Kind::Alpaca];
/// The venues the limit-order cases run on: SERVED and Hyperliquid.
const SERVED_ALL: [Kind; 5] = [Kind::Binance, Kind::BinanceUs, Kind::Bybit, Kind::Alpaca, Kind::Hyperliquid];

impl Kind {
    fn exchange(self) -> (&'static str, &'static str) {
        match self { Kind::Binance => ("Exchanges::Binance", "Binance"), Kind::BinanceUs => ("Exchanges::BinanceUs", "Binance.US"), Kind::Bybit => ("Exchanges::Bybit", "Bybit"),
                     Kind::Alpaca => ("Exchanges::Alpaca", "Alpaca"), Kind::Hyperliquid => ("Exchanges::Hyperliquid", "Hyperliquid") }
    }
    /// The order's id as `add_order` writes `transactions.external_id`: Alpaca's bare UUID, Hyperliquid's "<base>-<oid>".
    fn external_id(self, id: u64) -> String {
        match self { Kind::Bybit => id.to_string(), Kind::Alpaca => alpaca_id(id), Kind::Hyperliquid => format!("BTC-{id}"), _ => format!("BTCEUR-{id}") }
    }
    /// The pair's listing: Alpaca spells a crypto pair "BASE/QUOTE"; Hyperliquid names a spot pair by its index.
    fn symbol(self) -> &'static str { match self { Kind::Alpaca => "BTC/EUR", Kind::Hyperliquid => HL_COIN, _ => "BTCEUR" } }
    /// One row of the single list a venue answers with (Binance's allOrders, Alpaca's /v2/orders, Hyperliquid's
    /// historicalOrders; Bybit's either list).
    fn row(self, o: &O) -> Value { match self { Kind::Bybit => bybit_row(o), Kind::Alpaca => alpaca_row(o), Kind::Hyperliquid => hl_row(o), _ => binance_row(o) } }
}

/// A test venue factory: the scripted venue for every bot, after `hook` (what happens between the read and the write).
struct Factory<V> { venue: V, hook: RefCell<Option<Box<dyn FnOnce()>>> }
impl<V: Venue + Clone> VenueFactory for Factory<V> {
    type V = V;
    fn for_bot(&self, _exchange_type: &str, _credentials: Option<deltabadger::crypto::Credentials>) -> V {
        if let Some(hook) = self.hook.borrow_mut().take() { hook(); }
        self.venue.clone()
    }
}

struct Fx { _dir: tempfile::TempDir, path: PathBuf, c: Connection, s: Seeded, b: i64, kind: Kind, tr: ScriptedTransport, db: Db }

fn open(path: &PathBuf) -> Connection { Connection::open(path).unwrap() }

/// A due obligation on `kind`: a running bot's ambiguous Rails tick, collected, its ledger synced after the window.
fn fx(kind: Kind) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.sqlite3");
    let c = open(&path);
    deltabadger::store::create_schema(&c).unwrap();
    deltabadger::store::test_support::migrate(&c).unwrap();
    let s = seed::seed_kraken(&c, &seed::cipher());
    let (ty, name) = kind.exchange();
    c.execute("UPDATE exchanges SET type = ?2, name = ?3 WHERE id = ?1", params![s.exchange_id, ty, name]).unwrap();
    c.execute("UPDATE tickers SET ticker = ?2, base = 'BTC' WHERE id = ?1", params![s.ticker_id, kind.symbol()]).unwrap();
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    synced(&c, s.api_key_id, now());
    let db = Db::new(open(&path), seed::cipher());
    Fx { _dir: dir, path, c, s, b, kind, tr: ScriptedTransport::default(), db }
}
fn bot(c: &Connection, s: &Seeded) -> i64 {
    let b = seed::insert_bot(c, s, &BotSpec::weekly(20.0, "2026-09-01 10:00:00"));
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, base_asset_id, quote_asset_id, transaction_type, \
               error_messages, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 2, 0, ?4, ?5, 'REGULAR', '[]', '2026-09-22 14:03:00', '2026-09-22 14:03:00')",
              params![b, s.exchange_id, format!("PRIOR-{b}"), s.btc, s.quote]).unwrap();
    b
}
fn ambiguous_line(c: &Connection, bot: i64, at: &str) {
    c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'placement_ambiguous', 1, ?2, ?3)",
              params![bot, json!({ "error": "Net::ReadTimeout" }).to_string(), at]).unwrap();
}
fn queue() -> Connection {
    let q = Connection::open_in_memory().unwrap();
    q.execute_batch("CREATE TABLE solid_queue_jobs (id INTEGER PRIMARY KEY, class_name TEXT, arguments TEXT, scheduled_at TEXT, created_at TEXT, finished_at TEXT);
                     CREATE TABLE solid_queue_claimed_executions (id INTEGER PRIMARY KEY, job_id INTEGER, process_id INTEGER, created_at TEXT);
                     CREATE TABLE solid_queue_processes (id INTEGER PRIMARY KEY, last_heartbeat_at TEXT);
                     CREATE TABLE solid_queue_failed_executions (id INTEGER PRIMARY KEY, job_id INTEGER, created_at TEXT);").unwrap();
    q
}
fn synced(c: &Connection, key: i64, at: DateTime<Utc>) { deltabadger::jobs::state::record_success(c, deltabadger::sync::jobs::LEDGER_SYNC, Some(&key.to_string()), at).unwrap(); }
fn lines(c: &Connection, bot: i64) -> Vec<(String, Value)> {
    let mut s = c.prepare("SELECT event, details FROM bot_activity_logs WHERE bot_id = ?1 AND event LIKE 'rails_order_%' ORDER BY id").unwrap();
    s.query_map([bot], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().map(|r| { let (e, d) = r.unwrap(); (e, serde_json::from_str(&d).unwrap()) }).collect()
}
/// The money guard (r2.6): no order row, carry, figure or ledger link moves.
fn money(c: &Connection) -> Vec<String> {
    let dump = |sql: &str| -> String {
        let mut s = c.prepare(sql).unwrap();
        let n = s.column_count();
        s.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join(","))).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(";")
    };
    vec![dump("SELECT * FROM transactions ORDER BY id"), dump("SELECT id, settings, transient_data, status, started_at, settings_changed_at FROM bots ORDER BY id"),
         dump("SELECT id, transaction_id, linked_transaction_id FROM account_transactions ORDER BY id"), dump("SELECT count(*) FROM transactions")]
}

/// The guard beside what a hook changes on purpose (another bot's order marked HOOKED, a bot's settings mark).
fn money_beside_hook(c: &Connection) -> Vec<String> {
    let dump = |sql: &str| -> String {
        let mut s = c.prepare(sql).unwrap();
        let n = s.column_count();
        s.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join(","))).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(";")
    };
    vec![dump("SELECT * FROM transactions WHERE error_messages NOT LIKE '%HOOKED%' ORDER BY id"), dump("SELECT id, settings, transient_data, status, started_at FROM bots ORDER BY id"),
         dump("SELECT id, transaction_id, linked_transaction_id FROM account_transactions ORDER BY id")]
}

impl Fx {
    /// One pass of the job at `at`, `hook` run between the read and its write; the money guard around it.
    async fn pass_with(&self, at: DateTime<Utc>, hook: Option<Box<dyn FnOnce()>>) -> usize {
        let hooked = hook.is_some();
        let guard = |c: &Connection| if hooked { money_beside_hook(c) } else { money(c) };
        let before = guard(&self.c);
        let hook = RefCell::new(hook);
        let n = match self.kind {
            Kind::Binance => Report::new(Factory { venue: binance::BinanceVenue::new(self.tr.clone(), binance::URL, "secret"), hook }).pass(&self.db, at).await,
            Kind::BinanceUs => Report::new(Factory { venue: binance::BinanceVenue::new(self.tr.clone(), binance::US_URL, "secret")
                .with_rules(&deltabadger::engine::venue_rules::BINANCE_US), hook }).pass(&self.db, at).await,
            Kind::Bybit => Report::new(Factory { venue: bybit::BybitVenue::new(self.tr.clone(), bybit::URL), hook }).pass(&self.db, at).await,
            Kind::Alpaca => Report::new(Factory { venue: alpaca_venue(&self.tr), hook }).pass(&self.db, at).await,
            Kind::Hyperliquid => Report::new(Factory { venue: hl_venue(&self.tr), hook }).pass(&self.db, at).await,
        }.unwrap();
        assert_eq!(guard(&self.c), before, "a pass writes no order, carry or link");
        n
    }
    async fn pass(&self, at: DateTime<Utc>) -> usize { self.pass_with(at, None).await }
    fn lines(&self) -> Vec<(String, Value)> { lines(&self.c, self.b) }
    fn one(&self) -> (String, Value) { let mut l = self.lines(); assert_eq!(l.len(), 1, "{l:?}"); l.remove(0) }
    fn requests(&self) -> usize { self.tr.requests().len() }
    /// A28-1's line for the install as it is now: the sync report on a copy (the byte-equal oracle).
    fn a28_1(&self) -> (String, Value) {
        let copy = self.path.with_file_name("a28_1.sqlite3");
        let _ = std::fs::remove_file(&copy);
        self.c.execute("VACUUM INTO ?1", [copy.to_str().unwrap()]).unwrap();
        let c = open(&copy);
        assert_eq!(ru::report(&c, now()).unwrap(), 1);
        let mut l = lines(&c, self.b);
        l.remove(0)
    }
    /// The venue answers with these orders: Binance's allOrders; Bybit's realtime (the open ones) and history (the rest).
    fn answer(&self, orders: &[O]) {
        match self.kind {
            Kind::Bybit => {
                let (open, closed): (Vec<&O>, Vec<&O>) = orders.iter().partition(|o| matches!(o.state, St::Open | St::Partly));
                self.tr.reply("GET /v5/order/realtime", 200, page(open.iter().map(|o| bybit_row(o)).collect(), ""));
                self.tr.reply("GET /v5/order/history", 200, page(closed.iter().map(|o| bybit_row(o)).collect(), ""));
            }
            Kind::Hyperliquid => {
                self.tr.reply(self.first_path(), 200, Value::Array(orders.iter().map(hl_row).collect()));
                self.tr.reply(HL_FILLS, 200, Value::Array(orders.iter().filter(|o| o.exec != "0").map(|o| hl_fill(o, o.id, 500)).collect()));
            }
            kind => { self.tr.reply(self.first_path(), 200, Value::Array(orders.iter().map(|o| kind.row(o)).collect())); }
        }
    }
    fn first_path(&self) -> &'static str {
        match self.kind { Kind::Bybit => "GET /v5/order/realtime", Kind::Alpaca => "GET /v2/orders", Kind::Hyperliquid => HL_ORDERS, _ => "GET /api/v3/allOrders" }
    }
    /// The key's refusal, as each venue answers it.
    fn refuse(&self, http: bool) {
        match (self.kind, http) {
            (Kind::Bybit, false) => self.tr.reply(self.first_path(), 200, json!({ "retCode": 10003, "retMsg": "API key is invalid.", "result": {} })),
            (Kind::Bybit, true) => self.tr.reply(self.first_path(), 401, json!("Unauthorized")),
            (Kind::Alpaca, false) => self.tr.reply(self.first_path(), 403, json!({ "code": 40310000, "message": "forbidden." })),
            (Kind::Alpaca, true) => self.tr.reply(self.first_path(), 401, json!({ "code": 40110000, "message": "request is not authorized" })),
            // Hyperliquid's answer to an address it can't read.
            (Kind::Hyperliquid, false) => self.tr.reply(self.first_path(), 422, json!("Failed to deserialize the JSON body into the target type")),
            (Kind::Hyperliquid, true) => self.tr.reply(self.first_path(), 401, json!("Unauthorized")),
            (_, false) => self.tr.reply(self.first_path(), 400, json!({ "code": -2015, "msg": "Invalid API-key, IP, or permissions for action." })),
            (_, true) => self.tr.reply(self.first_path(), 401, json!({ "code": -2015, "msg": "Invalid API-key, IP, or permissions for action." })),
        };
    }
    fn server_error(&self, status: u16) { self.tr.reply(self.first_path(), status, json!("<html>busy</html>")); }
}

fn page(rows: Vec<Value>, cursor: &str) -> Value { json!({ "retCode": 0, "retMsg": "OK", "result": { "category": "spot", "list": rows, "nextPageCursor": cursor } }) }

#[derive(Clone, Copy, PartialEq)]
enum St { Open, Partly, Filled, PartlyCancelled, Cancelled }
/// One order, venue-neutral: `limit` its price (else a market order of `size` in the quote), `size` the base for a limit.
#[derive(Clone)]
struct O { id: u64, sell: bool, limit: Option<&'static str>, size: &'static str, state: St, exec: &'static str, cum: &'static str, at: &'static str }
fn o(id: u64, state: St, exec: &'static str, cum: &'static str) -> O { O { id, sell: false, limit: None, size: "20", state, exec, cum, at: PLACED } }
fn limit(id: u64, state: St, exec: &'static str, cum: &'static str) -> O { O { id, sell: false, limit: Some("50000"), size: "0.0004", state, exec, cum, at: PLACED } }
fn ms(at: &str) -> i64 { t(at).timestamp_millis() }

fn binance_row(o: &O) -> Value {
    let status = match o.state { St::Open => "NEW", St::Partly => "PARTIALLY_FILLED", St::Filled => "FILLED", St::PartlyCancelled | St::Cancelled => "CANCELED" };
    let (ty, orig, orig_quote, price) = match o.limit { Some(p) => ("LIMIT", o.size, "0.00000000", p), None => ("MARKET", o.exec, o.size, "0.00000000") };
    json!({ "symbol": "BTCEUR", "orderId": o.id, "orderListId": -1, "clientOrderId": format!("web_{}", o.id), "price": price, "origQty": orig, "executedQty": o.exec,
            "cummulativeQuoteQty": o.cum, "status": status, "timeInForce": "GTC", "type": ty, "side": if o.sell { "SELL" } else { "BUY" }, "stopPrice": "0.00000000",
            "icebergQty": "0.00000000", "time": ms(o.at), "updateTime": ms(o.at) + 1000, "isWorking": true, "workingTime": ms(o.at),
            "origQuoteOrderQty": orig_quote, "selfTradePreventionMode": "EXPIRE_MAKER" })
}
fn bybit_row(o: &O) -> Value {
    let status = match o.state { St::Open => "New", St::Partly => "PartiallyFilled", St::Filled => "Filled", St::PartlyCancelled => "PartiallyFilledCanceled", St::Cancelled => "Cancelled" };
    let (ty, unit, price) = match o.limit { Some(p) => ("Limit", "", p), None => ("Market", "quoteCoin", "0") };
    json!({ "orderId": o.id.to_string(), "orderLinkId": "", "symbol": "BTCEUR", "price": price, "qty": o.size, "side": if o.sell { "Sell" } else { "Buy" },
            "orderStatus": status, "orderType": ty, "marketUnit": unit, "avgPrice": "", "cumExecQty": o.exec, "cumExecValue": o.cum, "leavesQty": "0",
            "timeInForce": if o.limit.is_some() { "GTC" } else { "IOC" }, "createdTime": ms(o.at).to_string(), "updatedTime": (ms(o.at) + 1000).to_string() })
}

fn alpaca_venue<T: deltabadger::venue::http::Transport + Clone>(tr: &T) -> alpaca::AlpacaVenue<T> {
    alpaca::AlpacaVenue::new(tr.clone(), alpaca::Urls::for_passphrase(Some("paper")))
}
fn alpaca_id(id: u64) -> String { format!("6c88c025-e7a0-0992-70eb-{id:012}") }
/// The recorded rows (Jan's paper account, scrubbed by the recorder).
fn alpaca_recorded() -> Vec<Value> { serde_json::from_str(include_str!("fixtures/order_lists/alpaca/orders.json")).unwrap() }
/// A recorded crypto limit row (`BTC/USD`, filled), edited into `o`: id, client id, symbol (BTC/EUR, the seeded pair),
/// side, type, `qty` or `notional`, `limit_price`, status, `filled_qty`, `filled_avg_price` (= cum / exec, Alpaca states
/// no quote total), and every time from `o.at`. Everything else is as recorded.
fn alpaca_row(o: &O) -> Value {
    let mut r = alpaca_recorded().into_iter().find(|r| r["asset_class"] == "crypto" && r["status"] == "filled").unwrap();
    let status = match o.state { St::Open => "new", St::Partly => "partially_filled", St::Filled => "filled", St::PartlyCancelled | St::Cancelled => "canceled" };
    let dec = |s: &str| deltabadger::ruby::BigDec::parse(s).unwrap();
    let avg = if dec(o.exec).is_positive() { json!(dec(o.cum).div(&dec(o.exec)).unwrap().to_s_f()) } else { Value::Null };
    for (k, v) in [("id", json!(alpaca_id(o.id))), ("client_order_id", json!(format!("c-{}", o.id))), ("symbol", json!("BTC/EUR")), ("side", json!(if o.sell { "sell" } else { "buy" })),
                   ("type", json!(if o.limit.is_some() { "limit" } else { "market" })), ("order_type", json!(if o.limit.is_some() { "limit" } else { "market" })),
                   ("qty", if o.limit.is_some() { json!(o.size) } else { Value::Null }), ("notional", if o.limit.is_some() { Value::Null } else { json!(o.size) }),
                   ("limit_price", o.limit.map_or(Value::Null, |p| json!(p))), ("status", json!(status)), ("filled_qty", json!(o.exec)), ("filled_avg_price", avg),
                   ("created_at", json!(o.at)), ("submitted_at", json!(o.at)), ("updated_at", json!(o.at)), ("filled_at", Value::Null), ("canceled_at", Value::Null)] {
        r[k] = v;
    }
    r
}

/// One Binance fill of `order` on BTCEUR, stored as the importer stores it.
#[allow(clippy::too_many_arguments)]
fn fill(c: &Connection, s: &Seeded, id: i64, order: u64, qty: &str, quote: &str, at: &str) -> i64 {
    let raw = deltabadger::sync::activities::Raw::from_value(json!({ "symbol": "BTCEUR", "id": id, "orderId": order, "orderListId": -1, "price": "50000",
        "qty": qty, "quoteQty": quote, "commission": "0", "commissionAsset": "EUR", "time": ms(at), "isBuyer": true, "isMaker": false, "isBestMatch": true }));
    let legs = deltabadger::sync::binance_ledger::normalize_trade(&raw, "BTCEUR", "BTC", "EUR").unwrap();
    let e = &legs[0];
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
               raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'BTC', ?6, 'EUR', ?7, ?8, ?9, ?9, ?9)",
              params![s.user_id, s.exchange_id, s.api_key_id, e.tx_id, e.entry_type, qty, quote, e.raw.value.to_string(),
                      deltabadger::codec::format_time(e.transacted_at.unwrap())]).unwrap();
    c.last_insert_rowid()
}

// ---- what a line names (§2, r2.1) ------------------------------------------------------------------------------------

#[tokio::test]
async fn each_order_state_is_named_as_the_venue_lists_it() {
    let placed = "placed 2026-09-29 14:03:05 UTC";
    let cases: [(O, &str, &str); 5] = [
        (o(7, St::Filled, "0.0004", "20.01"), ru::LIVE_LISTED, "bought 0.0004 BTC for 20.01 EUR (filled)"),
        (limit(7, St::Open, "0", "0"), ru::LIVE_OPEN, "buy of 0.0004 BTC at 50000 EUR, still open"),
        (limit(7, St::Partly, "0.0001", "5"), ru::LIVE_OPEN, "buy of 0.0004 BTC at 50000 EUR, 0.0001 BTC filled for 5 EUR, still open"),
        (o(7, St::PartlyCancelled, "0.0002", "10"), ru::LIVE_LISTED, "buy of 20 EUR cancelled, 0.0002 BTC filled for 10 EUR"),
        (o(7, St::Cancelled, "0", "0"), ru::LIVE_LISTED, "buy of 20 EUR cancelled, nothing filled"),
    ];
    for kind in SERVED {
        for (order, event, text) in &cases {
            let f = fx(kind);
            f.answer(std::slice::from_ref(order));
            assert_eq!(f.pass(now()).await, 1);
            let (_, venue) = kind.exchange();
            assert_eq!(f.one(), (event.to_string(), json!({ "when": WHEN, "venue": venue, "orders": format!("{text}, {placed}"), "count": 1, "complete": true, "conflict": false,
                                            "obligation": f.c.query_row("SELECT id FROM rust_rails_unrecorded", [], |r| r.get::<_, i64>(0)).unwrap() })),
                       "{kind:?}");
            assert_eq!(f.pass(now() + Duration::hours(1)).await, 0, "one line, ever");
            assert_eq!(f.requests(), if kind == Kind::Bybit { 2 } else { 1 }, "{kind:?}: read once");
        }
    }
}

#[tokio::test]
async fn the_read_asks_for_the_bots_pair_over_the_window_in_milliseconds() {
    for kind in SERVED {
        let f = fx(kind);
        f.answer(&[]);
        f.pass(now()).await;
        let (from, to) = (ms("2026-09-29T13:53:00Z").to_string(), ms("2026-09-29T14:13:00Z").to_string());
        let q = |i: usize, k: &str| f.tr.requests()[i].query.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        if kind == Kind::Bybit {
            assert_eq!((q(0, "category"), q(0, "symbol"), q(0, "limit"), q(0, "startTime")), (Some("spot".into()), Some("BTCEUR".into()), Some("50".into()), None));
            assert_eq!(f.tr.requests()[1].path, bybit::HISTORY_PATH);
            assert_eq!((q(1, "startTime"), q(1, "endTime")), (Some(from), Some(to)));
        } else if kind == Kind::Alpaca {
            // One request over every symbol: status=all, the window ± 10 min by submission time, the documented page maximum.
            assert_eq!(f.tr.requests()[0].path, "/v2/orders");
            assert_eq!(f.tr.requests()[0].query, vec![("status", "all".to_string()), ("after", "2026-09-29T13:43:00Z".into()), ("until", "2026-09-29T14:23:00Z".into()), ("limit", "500".into())]);
        } else {
            assert_eq!(f.tr.requests()[0].path, "/api/v3/allOrders");
            assert_eq!((q(0, "symbol"), q(0, "startTime"), q(0, "endTime"), q(0, "limit")), (Some("BTCEUR".into()), Some(from), Some(to), Some("1000".into())));
        }
    }
}

#[tokio::test]
async fn an_open_order_is_listed_first_behind_three_closed_ones() {
    for kind in SERVED {
        let f = fx(kind);
        let mut orders: Vec<O> = (1..=3).map(|n| O { at: "2026-09-29T14:01:00Z", ..o(n, St::Filled, "0.0004", "20") }).collect();
        orders.push(limit(9, St::Open, "0", "0"));
        f.answer(&orders);
        f.pass(now()).await;
        let (event, d) = f.one();
        assert_eq!((event.as_str(), d["count"].as_i64()), (ru::LIVE_OPEN, Some(4)));
        let text = d["orders"].as_str().unwrap();
        assert!(text.starts_with("buy of 0.0004 BTC at 50000 EUR, still open"), "{kind:?}: {text}");
        assert!(text.ends_with("; and 1 more"), "{text}");
    }
}

// ---- what is never a positive: the A28-1 line, byte for byte (r1.1, r2.1) ------------------------------------------------

#[tokio::test]
async fn an_order_a_bot_holds_another_side_or_none_leaves_a28_1s_line() {
    for kind in SERVED {
        for case in ["held", "sell", "empty", "outside"] {
            let f = fx(kind);
            let oracle = f.a28_1();
            match case {
                "held" => {
                    f.c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
                                 VALUES (?1, ?2, ?3, 0, 0, 'REGULAR', '[]', ?4, ?4)", params![f.b, f.s.exchange_id, kind.external_id(7), "2026-09-22 14:03:00"]).unwrap();
                    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
                }
                "sell" => f.answer(&[O { sell: true, ..o(7, St::Filled, "0.0004", "20") }]),
                "outside" => f.answer(&[O { at: "2026-09-29T14:20:00Z", ..limit(7, St::Open, "0", "0") }]),
                _ => f.answer(&[]),
            }
            assert_eq!(f.pass(now()).await, 1);
            assert_eq!(f.one(), oracle, "{kind:?} {case}");
            assert_eq!(oracle.0, ru::NONE_SEEN);
        }
    }
}

#[tokio::test]
async fn an_unserved_venues_line_is_a28_1s_with_no_read() {
    for (ty, name) in [("Exchanges::Kraken", "Kraken"), ("Exchanges::Gemini", "Gemini"), ("Exchanges::Mexc", "MEXC")] {
        let f = fx(Kind::Binance);
        f.c.execute("UPDATE exchanges SET type = ?2, name = ?3 WHERE id = ?1", params![f.s.exchange_id, ty, name]).unwrap();
        let oracle = f.a28_1();
        f.pass(now()).await;
        assert_eq!(f.one(), oracle, "{ty}");
        assert_eq!(f.requests(), 0);
    }
}

#[tokio::test]
async fn a_prefix_outcome_never_reads() {
    let f = fx(Kind::Binance);
    f.c.execute("UPDATE bots SET settings_changed_at = '2026-09-30 09:00:00' WHERE id = ?1", [f.b]).unwrap();
    f.pass(now()).await;
    assert_eq!(f.one().1["reason"], "settings_changed");
    assert_eq!(f.requests(), 0);
}

#[tokio::test]
async fn a_line_not_due_on_the_ledger_is_not_read_yet() {
    let f = fx(Kind::Bybit);
    deltabadger::jobs::state::mark_incomplete(&f.c, deltabadger::sync::jobs::LEDGER_SYNC, Some(&f.s.api_key_id.to_string()), now()).unwrap();
    assert_eq!(f.pass(now()).await, 0);
    assert_eq!(f.requests(), 0, "A28-1's timing (r2.4)");
    f.answer(&[limit(7, St::Open, "0", "0")]);
    // Seven days on, the unread line is due: the venue is read then.
    assert_eq!(f.pass(now() + Duration::days(7)).await, 1);
    assert_eq!(f.one().0, ru::LIVE_OPEN);
}

// ---- strict rows and partial reads (r1.2, r2.2) ------------------------------------------------------------------------

#[tokio::test]
async fn an_unreadable_row_alone_leaves_a28_1s_line_and_beside_an_open_order_keeps_it() {
    for kind in SERVED {
        let odd: [fn(&mut Value); 4] = [
            |r| { let k = ["time", "created_at"].into_iter().find(|k| r.get(*k).is_some()).unwrap_or("createdTime"); r.as_object_mut().unwrap().remove(k); },
            |r| r["side"] = json!("BOTH"),
            |r| if r.get("orderStatus").is_some() { r["orderStatus"] = json!("Untriggered") } else { r["status"] = json!("PENDING_NEW") },
            |r| if r.get("marketUnit").is_some() { r["marketUnit"] = json!("") } else { r["type"] = json!("STOP_LOSS_LIMIT") },
        ];
        for (n, spoil) in odd.iter().enumerate() {
            for with_open in [false, true] {
                let f = fx(kind);
                let oracle = f.a28_1();
                let mut bad = kind.row(&o(8, St::Filled, "0.0004", "20"));
                spoil(&mut bad);
                let open = limit(9, St::Open, "0", "0");
                match kind {
                    Kind::Bybit => {
                        let rows = if with_open { vec![bybit_row(&open)] } else { vec![] };
                        f.tr.reply("GET /v5/order/realtime", 200, page(rows, ""));
                        f.tr.reply("GET /v5/order/history", 200, page(vec![bad], ""));
                    }
                    _ => {
                        let mut rows = if with_open { vec![kind.row(&open)] } else { vec![] };
                        rows.push(bad);
                        f.tr.reply(f.first_path(), 200, Value::Array(rows));
                    }
                }
                f.pass(now()).await;
                let line = f.one();
                if with_open {
                    assert_eq!((line.0.as_str(), line.1["count"].as_i64(), line.1["complete"].as_bool()), (ru::LIVE_OPEN, Some(1), Some(false)), "{kind:?} {n}");
                } else {
                    assert_eq!(line, oracle, "{kind:?} {n}");
                }
            }
        }
    }
}

#[tokio::test]
async fn a_capped_read_keeps_what_it_validated() {
    for kind in SERVED_ALL {
        for with_open in [false, true] {
            let f = fx(kind);
            let oracle = f.a28_1();
            let full = match kind { Kind::Bybit => 50, Kind::Alpaca => alpaca::HISTORY_LIMIT, Kind::Hyperliquid => hyperliquid::HISTORY_LIMIT, _ => 1000 };
            let sells: Vec<O> = (100..).take(full).map(|id| O { sell: true, ..limit(id, St::Open, "0", "0") }).collect();
            match kind {
                Kind::Bybit => {
                    // Five full realtime pages, each with a next cursor: the cap.
                    for p in 0..5 {
                        let mut rows: Vec<Value> = sells.iter().map(|o| bybit_row(&O { id: o.id + 1000 * p, ..o.clone() })).collect();
                        if with_open && p == 0 { rows[0] = bybit_row(&limit(9, St::Open, "0", "0")); }
                        f.tr.reply("GET /v5/order/realtime", 200, page(rows, "next"));
                    }
                    f.tr.reply("GET /v5/order/history", 200, page(vec![], ""));
                }
                _ => {
                    let mut rows: Vec<Value> = sells.iter().map(|o| kind.row(o)).collect();
                    if with_open { rows[0] = kind.row(&limit(9, St::Open, "0", "0")); }
                    f.tr.reply(f.first_path(), 200, Value::Array(rows));
                    if kind == Kind::Hyperliquid { f.tr.reply(HL_FILLS, 200, json!([])); }
                }
            }
            f.pass(now()).await;
            let line = f.one();
            if with_open { assert_eq!((line.0.as_str(), line.1["complete"].as_bool()), (ru::LIVE_OPEN, Some(false)), "{kind:?}"); }
            else { assert_eq!(line, oracle, "{kind:?}"); }
            if kind == Kind::Bybit { assert_eq!(f.tr.requests().iter().filter(|r| r.path == bybit::REALTIME_PATH).count(), 5, "five pages at most"); }
        }
    }
}

#[tokio::test]
async fn a_later_page_that_fails_keeps_the_open_order_of_the_first() {
    let f = fx(Kind::Bybit);
    f.tr.reply("GET /v5/order/realtime", 200, page((0..50).map(|n| bybit_row(&if n == 0 { limit(9, St::Open, "0", "0") } else { O { sell: true, ..limit(100 + n, St::Open, "0", "0") } })).collect(), "next"));
    f.tr.reply("GET /v5/order/realtime", 500, json!("<html>busy</html>"));
    f.tr.reply("GET /v5/order/history", 200, page(vec![], ""));
    f.pass(now()).await;
    let (event, d) = f.one();
    assert_eq!((event.as_str(), d["complete"].as_bool()), (ru::LIVE_OPEN, Some(false)));
}

#[tokio::test]
async fn an_order_both_bybit_lists_name_is_listed_once_as_history_has_it() {
    let f = fx(Kind::Bybit);
    f.tr.reply("GET /v5/order/realtime", 200, page(vec![bybit_row(&limit(9, St::Partly, "0.0001", "5"))], ""));
    f.tr.reply("GET /v5/order/history", 200, page(vec![bybit_row(&limit(9, St::Filled, "0.0004", "20"))], ""));
    f.pass(now()).await;
    let (event, d) = f.one();
    assert_eq!((event.as_str(), d["count"].as_i64()), (ru::LIVE_LISTED, Some(1)));
    assert!(d["orders"].as_str().unwrap().starts_with("bought 0.0004 BTC for 20 EUR (filled)"), "{d}");
}

#[tokio::test]
async fn a_symbol_a_replaced_listing_also_carried_is_not_read() {
    let f = fx(Kind::Binance);
    // The coin that listed as BTCEUR before: another asset, its row tombstoned (jobs::import::tombstone).
    f.c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('old-coin', 'BTC', 'Old coin', 'Cryptocurrency', ?1, ?1)", [AT]).unwrap();
    let old = f.c.last_insert_rowid();
    f.c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                 minimum_base_size, minimum_quote_size, available, created_at, updated_at) VALUES (?1, '__stale_99_BTCEUR', '__stale_99_BTC', 'EUR', ?2, ?3, 8, 2, 2, 0, 0, 0, ?4, ?4)",
                params![f.s.exchange_id, old, f.s.quote, AT]).unwrap();
    let oracle = f.a28_1();
    f.pass(now()).await;
    assert_eq!((f.one(), f.requests()), (oracle, 0));
}

// ---- refusals, retries and durability (r1.3, r2.3, r3.2) ---------------------------------------------------------------

#[tokio::test]
async fn a_refusal_writes_a28_1s_line_and_after_a_crash_is_never_asked_again() {
    for kind in SERVED_ALL {
        for http in [false, true] {
            let f = fx(kind);
            let oracle = f.a28_1();
            f.refuse(http);
            assert_eq!(f.pass(now()).await, 1);
            assert_eq!(f.one(), oracle, "{kind:?} {http}");
            assert_eq!(f.requests(), 1);
            // The line's transaction lost (a crash after the refusal was stored), then a restart with the same key.
            f.c.execute_batch("UPDATE rust_rails_unrecorded SET reported_at = NULL, outcome = NULL; DELETE FROM bot_activity_logs WHERE event LIKE 'rails_order_%'").unwrap();
            assert_eq!(f.pass(now() + Duration::hours(1)).await, 1);
            assert_eq!((f.one(), f.requests()), (oracle.clone(), 1), "{kind:?}: one request ever");
        }
    }
}

#[tokio::test]
async fn a_server_error_or_throttle_writes_nothing_and_a_later_pass_reads_again() {
    for kind in SERVED_ALL {
        for status in [500, 429, 503] {
            let f = fx(kind);
            // The scripted replies are served in turn: the failure, then the answer.
            f.server_error(status);
            f.answer(&[limit(9, St::Open, "0", "0")]);
            assert_eq!(f.pass(now()).await, 0);
            assert!(f.lines().is_empty());
            assert_eq!(f.pass(now() + Duration::minutes(30)).await, 0, "not again within the hour");
            assert_eq!(f.requests(), 1);
            assert_eq!(f.pass(now() + Duration::hours(1)).await, 1);
            assert_eq!(f.one().0, ru::LIVE_OPEN, "{kind:?} {status}");
        }
    }
}

#[tokio::test]
async fn bybits_rate_limit_code_is_retried_too() {
    let f = fx(Kind::Bybit);
    f.tr.reply("GET /v5/order/realtime", 200, json!({ "retCode": 10006, "retMsg": "Too many visits!", "result": {} }));
    assert_eq!(f.pass(now()).await, 0);
    assert!(f.lines().is_empty());
}

#[tokio::test]
async fn three_failed_passes_write_a28_1s_line() {
    for kind in SERVED_ALL {
        let f = fx(kind);
        let oracle = f.a28_1();
        f.server_error(500);
        assert_eq!(f.pass(now()).await, 0);
        assert_eq!(f.pass(now() + Duration::hours(1)).await, 0);
        assert_eq!(f.pass(now() + Duration::hours(2)).await, 1);
        assert_eq!((f.one(), f.requests()), (oracle, 3), "{kind:?}");
    }
}

#[tokio::test]
async fn a_key_replaced_since_before_the_window_is_never_read() {
    // Rails stores updated_at to the second: the second before the window's start is no proof of continuity either.
    for (updated, read) in [("2026-09-29 13:52:58", true), ("2026-09-29 13:52:59", false), ("2026-09-29 13:55:00", false), ("2026-09-30 09:00:00", false)] {
        for kind in SERVED_ALL {
            let f = fx(kind);
            f.c.execute("UPDATE api_keys SET updated_at = ?2 WHERE id = ?1", params![f.s.api_key_id, updated]).unwrap();
            let oracle = f.a28_1();
            f.answer(&[limit(9, St::Open, "0", "0")]);
            f.pass(now()).await;
            if read { assert_eq!(f.one().0, ru::LIVE_OPEN, "{kind:?}"); } else { assert_eq!((f.one(), f.requests()), (oracle, 0), "{kind:?} {updated}"); }
        }
    }
}

#[tokio::test]
async fn no_trading_key_is_no_read() {
    let f = fx(Kind::Binance);
    // A read-only key: the ledger reads with it, no bot trades with it.
    f.c.execute("UPDATE api_keys SET key_type = 2 WHERE id = ?1", [f.s.api_key_id]).unwrap();
    let oracle = f.a28_1();
    f.pass(now()).await;
    assert_eq!((f.one(), f.requests()), (oracle, 0));
}

#[tokio::test]
async fn a_key_changed_between_the_read_and_the_write_writes_nothing_then_the_next_pass_reads_again() {
    for kind in SERVED {
        let f = fx(kind);
        f.answer(&[limit(9, St::Open, "0", "0")]);
        let path = f.path.clone();
        let key = f.s.api_key_id;
        let hook: Box<dyn FnOnce()> = Box::new(move || { open(&path).execute("UPDATE api_keys SET secret = ?2 WHERE id = ?1", params![key, seed::cipher().encrypt("other")]).unwrap(); });
        assert_eq!(f.pass_with(now(), Some(hook)).await, 0);
        assert!(f.lines().is_empty());
        assert_eq!(f.pass(now() + Duration::hours(1)).await, 1);
        assert_eq!(f.one().0, ru::LIVE_OPEN, "{kind:?}");
    }
}

#[tokio::test]
async fn a_settings_change_between_the_read_and_the_write_writes_nothing_then_the_prefix_line() {
    let f = fx(Kind::Binance);
    f.answer(&[limit(9, St::Open, "0", "0")]);
    let (path, b) = (f.path.clone(), f.b);
    let hook: Box<dyn FnOnce()> = Box::new(move || { open(&path).execute("UPDATE bots SET settings_changed_at = '2026-09-30 11:59:00' WHERE id = ?1", [b]).unwrap(); });
    assert_eq!(f.pass_with(now(), Some(hook)).await, 0);
    assert!(f.lines().is_empty());
    f.pass(now() + Duration::hours(1)).await;
    assert_eq!(f.one().1["reason"], "settings_changed");
    assert_eq!(f.requests(), 1);
}

#[tokio::test]
async fn an_order_recorded_between_the_read_and_the_write_is_not_named() {
    for kind in SERVED_ALL {
        let f = fx(kind);
        let oracle = f.a28_1();
        f.answer(&[limit(9, St::Open, "0", "0")]);
        let (path, s, id) = (f.path.clone(), f.s.exchange_id, kind.external_id(9));
        let other = bot(&f.c, &f.s);
        let hook: Box<dyn FnOnce()> = Box::new(move || {
            open(&path).execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
                                 VALUES (?1, ?2, ?3, 0, 0, 'REGULAR', '[\"HOOKED\"]', '2026-09-30 11:59:00', '2026-09-30 11:59:00')", params![other, s, id]).unwrap();
        });
        f.pass_with(now(), Some(hook)).await;
        assert_eq!(f.one(), oracle, "{kind:?}");
    }
}

#[tokio::test]
async fn the_reads_of_a_pass_are_bounded_and_taken_round_robin() {
    let f = fx(Kind::Binance);
    // 21 more obligations, each its own Rails tick a day earlier, the last one the one with an open order (r1.8).
    f.c.execute_batch("DELETE FROM rust_rails_unrecorded; DELETE FROM bot_activity_logs WHERE event = 'placement_ambiguous'").unwrap();
    for n in 0..21 { ambiguous_line(&f.c, f.b, &format!("2026-09-28 {:02}:00:00", n + 1)); }
    ambiguous_line(&f.c, f.b, AT);
    ru::collect(&f.c, &queue(), None, now()).unwrap();
    assert_eq!(ru::collect(&f.c, &queue(), None, now()).unwrap(), 0);
    for _ in 0..21 { f.server_error(500); }
    f.answer(&[limit(9, St::Open, "0", "0")]);
    assert_eq!(f.pass(now()).await, 0);
    assert_eq!(f.requests(), 20, "twenty reads a pass");
    f.pass(now() + Duration::hours(1)).await;
    let open: Vec<_> = f.lines().into_iter().filter(|l| l.0 == ru::LIVE_OPEN).collect();
    assert_eq!(open.len(), 1, "the 22nd obligation is read in the second pass");
    assert_eq!(open[0].1["when"], WHEN);
}

// ---- the ledger's evidence, per order (r3.1, r4.1) ---------------------------------------------------------------------

#[tokio::test]
async fn ledger_fills_of_a_listed_order_are_never_contradicted() {
    struct Case { live: O, fills: &'static [(&'static str, &'static str, &'static str)], text: &'static str, conflict: bool }
    let cases = [
        // The venue says cancelled with nothing filled; the ledger has its fill: never "nothing filled".
        Case { live: o(7, St::Cancelled, "0", "0"), fills: &[("0.0002", "10", "2026-09-29T14:03:06Z")], text: "buy of 20 EUR, at least 0.0002 BTC filled for 10 EUR", conflict: true },
        // The venue lower than the ledger proves; an open order keeps its state word.
        Case { live: limit(7, St::Partly, "0.0001", "5"), fills: &[("0.0002", "10", "2026-09-29T14:03:06Z")],
               text: "buy of 0.0004 BTC at 50000 EUR, at least 0.0002 BTC filled for 10 EUR, still open", conflict: true },
        // An unknown quote on the venue (Binance's negative cummulativeQuoteQty), the same base in the ledger: its quote.
        Case { live: o(7, St::Filled, "0.0004", "-1.00000000"), fills: &[("0.0004", "20.01", "2026-09-29T14:03:06Z")], text: "bought 0.0004 BTC for 20.01 EUR (filled)", conflict: false },
        // The venue higher than the ledger: the venue's figures.
        Case { live: o(7, St::PartlyCancelled, "0.0004", "20"), fills: &[("0.0001", "5", "2026-09-29T14:03:06Z")], text: "buy of 20 EUR cancelled, 0.0004 BTC filled for 20 EUR", conflict: false },
        // r4.1: a fill of the order only after the window still counts.
        Case { live: o(7, St::Cancelled, "0", "0"), fills: &[("0.0002", "10", "2026-09-29T18:00:00Z")], text: "buy of 20 EUR, at least 0.0002 BTC filled for 10 EUR", conflict: true },
    ];
    for case in cases {
        let f = fx(Kind::Binance);
        for (n, (qty, quote, at)) in case.fills.iter().enumerate() { fill(&f.c, &f.s, 50 + n as i64, 7, qty, quote, at); }
        f.answer(std::slice::from_ref(&case.live));
        f.pass(now()).await;
        let (_, d) = f.one();
        assert_eq!(d["orders"], format!("{}, placed 2026-09-29 14:03:05 UTC", case.text));
        assert_eq!((d["conflict"].as_bool(), d["count"].as_i64()), (Some(case.conflict), Some(1)), "the ledger's candidate is the listed order, not another");
    }
}

#[tokio::test]
async fn an_unreadable_ledger_row_after_the_order_was_placed_leaves_no_fill_claim() {
    let f = fx(Kind::Binance);
    let r = fill(&f.c, &f.s, 50, 99, "0.0002", "10", "2026-09-29T19:00:00Z");
    f.c.execute("UPDATE account_transactions SET raw_data = json_set(raw_data, '$.qty', 'abc') WHERE id = ?1", [r]).unwrap();
    f.answer(&[o(7, St::Cancelled, "0", "0")]);
    f.pass(now()).await;
    assert_eq!(f.one().1["orders"], "buy of 20 EUR cancelled, placed 2026-09-29 14:03:05 UTC");
}

#[tokio::test]
async fn a_ledger_candidate_the_venue_did_not_list_is_merged_into_the_line() {
    let f = fx(Kind::Binance);
    fill(&f.c, &f.s, 50, 5, "0.0004", "20.01", "2026-09-29T14:03:01Z");
    f.answer(&[limit(9, St::Open, "0", "0")]);
    f.pass(now()).await;
    let (event, d) = f.one();
    assert_eq!((event.as_str(), d["count"].as_i64()), (ru::LIVE_OPEN, Some(2)));
    assert_eq!(d["orders"], "buy of 0.0004 BTC at 50000 EUR, still open, placed 2026-09-29 14:03:05 UTC; bought 0.0004 BTC for 20.01 EUR on 2026-09-29 14:03:01 UTC (filled)");
}

#[tokio::test]
async fn an_order_linked_to_a_bots_order_through_its_fill_is_not_named() {
    let f = fx(Kind::Binance);
    let oracle = f.a28_1();
    let other = bot(&f.c, &f.s);
    f.c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
                 VALUES (?1, ?2, 'X', 0, 0, 'REGULAR', '[]', '2026-09-29 14:03:01', '2026-09-29 14:03:01')", params![other, f.s.exchange_id]).unwrap();
    let held = f.c.last_insert_rowid();
    let late = fill(&f.c, &f.s, 50, 7, "0.0004", "20", "2026-09-29T16:00:00Z");
    f.c.execute("UPDATE account_transactions SET transaction_id = ?2 WHERE id = ?1", params![late, held]).unwrap();
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.pass(now()).await;
    assert_eq!(f.one(), oracle);
}

// ---- texts ---------------------------------------------------------------------------------------------------------------

#[test]
fn every_live_line_renders_in_english_without_a_missing_key() {
    use deltabadger::web::i18n::{self, Arg};
    for key in ["rails_order_open", "rails_order_listed", "rails_order_listed_one"] {
        let text = i18n::text("en", &format!("bot_activity.events.{key}"), &[("when", Arg::Text(WHEN)), ("venue", Arg::Text("Binance")), ("orders", Arg::Text("x"))]);
        assert!(text.starts_with("Before the update, an order from this bot may have gone through without being recorded (around 2026-09-29 14:03 UTC)."), "{key}: {text}");
        assert!(text.ends_with("This may not be every order from that time. Check your Binance order history."), "{key}: {text}");
        assert!(!text.contains("%{") && !text.contains("no money"), "{key}: {text}");
    }
}

// ---- Codex branch r1 ---------------------------------------------------------------------------------------------------

/// The scripted venue, but a request to `stall` never answers.
#[derive(Clone)]
struct Stalling { inner: ScriptedTransport, stall: &'static str }
impl deltabadger::venue::http::Transport for Stalling {
    async fn send(&self, r: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
        if r.path == self.stall { std::future::pending::<()>().await; }
        self.inner.send(r).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_read_past_its_deadline_keeps_the_open_order_it_already_validated() {
    let f = fx(Kind::Bybit);
    f.tr.reply("GET /v5/order/realtime", 200, page(vec![bybit_row(&limit(9, St::Open, "0", "0"))], ""));
    let venue = bybit::BybitVenue::new(Stalling { inner: f.tr.clone(), stall: bybit::HISTORY_PATH }, bybit::URL);
    let before = money(&f.c);
    assert_eq!(Report::new(Factory { venue, hook: RefCell::new(None) }).pass(&f.db, now()).await.unwrap(), 1);
    let (event, d) = f.one();
    assert_eq!((event.as_str(), d["complete"].as_bool()), (ru::LIVE_OPEN, Some(false)));
    assert_eq!(money(&f.c), before);
}

#[tokio::test]
async fn a_short_bybit_page_with_a_cursor_is_followed() {
    let f = fx(Kind::Bybit);
    f.tr.reply("GET /v5/order/realtime", 200, page(vec![bybit_row(&O { sell: true, ..limit(5, St::Open, "0", "0") })], "page2"));
    f.tr.reply("GET /v5/order/realtime", 200, page(vec![bybit_row(&limit(9, St::Open, "0", "0"))], ""));
    f.tr.reply("GET /v5/order/history", 200, page(vec![], ""));
    f.pass(now()).await;
    assert_eq!(f.one().0, ru::LIVE_OPEN);
    let cursors: Vec<_> = f.tr.requests().iter().map(|r| r.query.iter().find(|(k, _)| *k == "cursor").map(|(_, v)| v.clone())).collect();
    assert_eq!(cursors, [None, Some("page2".to_string()), None]);
}

#[tokio::test]
async fn a_failed_read_is_retried_no_sooner_than_an_hour_later() {
    let f = fx(Kind::Binance);
    f.server_error(500);
    f.pass(now()).await;
    f.pass(now() + Duration::minutes(59)).await;
    assert_eq!(f.requests(), 1);
    f.pass(now() + Duration::minutes(60)).await;
    assert_eq!(f.requests(), 2);
}

#[tokio::test]
async fn a_key_changed_during_a_refused_or_last_failed_read_writes_nothing_then_reads_with_the_new_key() {
    for refused in [true, false] {
        let f = fx(Kind::Binance);
        // In turn: the failure(s) — a refusal, or a third 500 after two — then the answer.
        if refused { f.refuse(false); } else { for _ in 0..3 { f.server_error(500); } }
        f.answer(&[limit(9, St::Open, "0", "0")]);
        if !refused {
            f.pass(now()).await;
            f.pass(now() + Duration::hours(1)).await;
        }
        let (path, key) = (f.path.clone(), f.s.api_key_id);
        let hook: Box<dyn FnOnce()> = Box::new(move || { open(&path).execute("UPDATE api_keys SET secret = ?2 WHERE id = ?1", params![key, seed::cipher().encrypt("other")]).unwrap(); });
        assert_eq!(f.pass_with(now() + Duration::hours(2), Some(hook)).await, 0, "refused {refused}");
        assert!(f.lines().is_empty());
        assert_eq!(f.pass(now() + Duration::hours(3)).await, 1);
        assert_eq!(f.one().0, ru::LIVE_OPEN, "refused {refused}: the new key is read afresh");
    }
}

// ---- A28-3b-1: Alpaca, on its recorded answer --------------------------------------------------------------------------

#[test]
fn every_recorded_alpaca_row_reads_strictly_as_the_recording_states_it() {
    use deltabadger::venue::{CreatedKind, CreatedSize, CreatedState};
    let rows = alpaca_recorded();
    let mut states = std::collections::BTreeMap::new();
    for r in &rows {
        let o = alpaca::created_order(r["symbol"].as_str().unwrap(), r).unwrap_or_else(|| panic!("unreadable recorded row: {r}"));
        assert_eq!(o.external_id, r["id"].as_str().unwrap());
        assert_eq!(matches!(o.kind, CreatedKind::Limit { .. }), r["type"] == "limit");
        assert_eq!(matches!(o.size, CreatedSize::Quote(_)), !r["notional"].is_null());
        *states.entry(format!("{:?}", o.state)).or_insert(0) += 1;
        if o.state == CreatedState::PartlyFilledOpen { assert!(o.filled_base.is_positive() && r["status"] == "partially_filled"); }
    }
    assert_eq!(states, [("Cancelled".to_string(), 3), ("Filled".into(), 27), ("Open".into(), 1), ("PartlyFilledOpen".into(), 1)].into_iter().collect());
    let page: Vec<Value> = serde_json::from_str(include_str!("fixtures/order_lists/alpaca/orders.page.json")).unwrap();
    let empty: Vec<Value> = serde_json::from_str(include_str!("fixtures/order_lists/alpaca/orders.empty.json")).unwrap();
    assert_eq!((page.len(), empty.len()), (1, 0));
}

#[test]
fn an_alpaca_creation_time_is_kept_to_the_microsecond() {
    let mut r = alpaca_row(&limit(7, St::Open, "0", "0"));
    r["created_at"] = json!("2026-09-29T14:03:05.123456789Z");
    let o = alpaca::created_order("BTC/EUR", &r).unwrap();
    assert_eq!(o.created_at, t("2026-09-29T14:03:05.123456Z"));
    assert_eq!(deltabadger::codec::format_time(o.created_at), "2026-09-29 14:03:05.123456", "what A28-2's facts store and compare");
}

#[test]
fn an_alpaca_size_is_its_qty_or_its_notional_never_both() {
    use deltabadger::venue::CreatedSize;
    let base = |r: &mut Value| { r["notional"] = Value::Null; r["qty"] = json!("0.0004"); };
    let mut r = alpaca_row(&o(7, St::Filled, "0.0004", "20"));
    assert!(matches!(alpaca::created_order("BTC/EUR", &r).unwrap().size, CreatedSize::Quote(_)));
    base(&mut r);
    assert!(matches!(alpaca::created_order("BTC/EUR", &r).unwrap().size, CreatedSize::Base(_)));
    r["notional"] = json!("20");
    assert!(alpaca::created_order("BTC/EUR", &r).is_none(), "both");
    r["qty"] = Value::Null; r["notional"] = Value::Null;
    assert!(alpaca::created_order("BTC/EUR", &r).is_none(), "neither");
}

#[tokio::test]
async fn an_alpaca_state_that_doesnt_say_whether_the_order_is_open_is_unreadable() {
    for status in ["calculated", "stopped", "suspended", "PENDING_NEW"] {
        for with_open in [false, true] {
            let f = fx(Kind::Alpaca);
            let oracle = f.a28_1();
            let mut bad = alpaca_row(&o(8, St::Filled, "0.0004", "20"));
            bad["status"] = json!(status);
            let mut rows = if with_open { vec![alpaca_row(&limit(9, St::Open, "0", "0"))] } else { vec![] };
            rows.push(bad);
            f.tr.reply("GET /v2/orders", 200, Value::Array(rows));
            f.pass(now()).await;
            let line = f.one();
            if with_open { assert_eq!((line.0.as_str(), line.1["count"].as_i64(), line.1["complete"].as_bool()), (ru::LIVE_OPEN, Some(1), Some(false)), "{status}"); }
            else { assert_eq!(line, oracle, "{status}"); }
        }
    }
}

#[tokio::test]
async fn an_alpaca_row_whose_class_or_spelling_disagrees_is_never_attributed() {
    // (the listing as stored, the row's symbol, its asset_class)
    let cases = [("BTC/EUR", "BTCEUR", "crypto"), ("BTCEUR", "BTCEUR", "crypto"), ("BTC/EUR", "BTC/EUR", "us_equity"), ("BTC/EUR", "BTC/EUR", "us_option")];
    for (listing, symbol, class) in cases {
        for with_open in [false, true] {
            let f = fx(Kind::Alpaca);
            f.c.execute("UPDATE tickers SET ticker = ?2 WHERE id = ?1", params![f.s.ticker_id, listing]).unwrap();
            let oracle = f.a28_1();
            let mut bad = alpaca_row(&o(8, St::Filled, "0.0004", "20"));
            bad["symbol"] = json!(symbol);
            bad["asset_class"] = json!(class);
            let mut open = alpaca_row(&limit(9, St::Open, "0", "0"));
            open["symbol"] = json!(listing);
            open["asset_class"] = json!(if listing.contains('/') { "crypto" } else { "us_equity" });
            let mut rows = if with_open { vec![open] } else { vec![] };
            rows.push(bad);
            f.tr.reply("GET /v2/orders", 200, Value::Array(rows));
            f.pass(now()).await;
            let line = f.one();
            if with_open { assert_eq!((line.0.as_str(), line.1["count"].as_i64(), line.1["complete"].as_bool()), (ru::LIVE_OPEN, Some(1), Some(false)), "{listing} {symbol} {class}"); }
            else { assert_eq!(line, oracle, "{listing} {symbol} {class}: nothing offered, so nothing to book"); }
        }
    }
}

#[tokio::test]
async fn another_pairs_alpaca_row_is_skipped_unread() {
    let f = fx(Kind::Alpaca);
    let mut other = alpaca_row(&o(8, St::Filled, "0.0004", "20"));
    other["symbol"] = json!("ETH/EUR");
    other["status"] = json!("calculated");
    other.as_object_mut().unwrap().remove("created_at");
    f.tr.reply("GET /v2/orders", 200, Value::Array(vec![other, alpaca_row(&limit(9, St::Open, "0", "0"))]));
    f.pass(now()).await;
    let (event, d) = f.one();
    assert_eq!((event.as_str(), d["count"].as_i64(), d["complete"].as_bool()), (ru::LIVE_OPEN, Some(1), Some(true)));
}

// ---- A28-3c-3a: Hyperliquid, on its recorded answer --------------------------------------------------------------------

const HL_COIN: &str = "@142";
const HL_USER: &str = "0x00000000000000000000000000000000000000a1";
const HL_ORDERS: &str = "POST /info historicalOrders";
const HL_FILLS: &str = "POST /info userFillsByTime";

fn hl_venue<T: deltabadger::venue::http::Transport + Clone>(tr: &T) -> hyperliquid::HyperliquidVenue<T> {
    hl_venue_for(tr, HL_COIN, "BTC")
}
fn hl_venue_for<T: deltabadger::venue::http::Transport + Clone>(tr: &T, coin: &str, base: &str) -> hyperliquid::HyperliquidVenue<T> {
    let pairs = binance::Pairs { by_symbol: [(coin.to_string(), binance::Pair { base: base.into(), quote: "EUR".into(), price_decimals: 2 })].into(), assets: Default::default() };
    hyperliquid::HyperliquidVenue::new(tr.clone(), hyperliquid::URL, HL_USER).with_pairs(pairs)
}
/// The recorded answers (the Hyperliquid Assistance Fund's spot orders and fills, scrubbed by the recorder).
fn hl_recorded() -> Vec<Value> { serde_json::from_str(include_str!("fixtures/order_lists/hyperliquid/historical_orders.json")).unwrap() }
fn hl_fills_recorded() -> Vec<Value> { serde_json::from_str(include_str!("fixtures/order_lists/hyperliquid/user_fills_by_time.json")).unwrap() }
fn dec(s: &str) -> deltabadger::ruby::BigDec { deltabadger::ruby::BigDec::parse(s).unwrap() }

/// A recorded `filled` row edited into `o` (a limit: Hyperliquid has no other order): oid, coin (the seeded pair), side,
/// limitPx, origSz, `sz` (what is left; an `open` row is written at placement, so all of it), status, both times (the
/// status a second after placement). Everything else is as recorded.
fn hl_row(o: &O) -> Value {
    let mut r = hl_recorded().into_iter().find(|r| r["status"] == "filled").unwrap();
    let left = match o.state { St::Open | St::Partly | St::Cancelled => o.size.to_string(), St::Filled => "0.0".into(), St::PartlyCancelled => (&dec(o.size) - &dec(o.exec)).to_s_f() };
    let status = match o.state { St::Open | St::Partly => "open", St::Filled => "filled", St::PartlyCancelled | St::Cancelled => "canceled" };
    for (k, v) in [("oid", json!(o.id)), ("coin", json!(HL_COIN)), ("side", json!(if o.sell { "A" } else { "B" })), ("limitPx", json!(o.limit.unwrap_or("50000"))),
                   ("origSz", json!(o.size)), ("sz", json!(left)), ("timestamp", json!(ms(o.at)))] {
        r["order"][k] = v;
    }
    r["status"] = json!(status);
    r["statusTimestamp"] = json!(ms(o.at) + 1000);
    r
}
/// A recorded fill edited into `o`'s whole fill: coin, oid, side, `sz` = exec, `px` = cum / exec, `time` placement + `after`
/// ms, `tid`.
fn hl_fill(o: &O, tid: u64, after: i64) -> Value {
    let mut f = hl_fills_recorded().remove(0);
    for (k, v) in [("coin", json!(HL_COIN)), ("oid", json!(o.id)), ("side", json!(if o.sell { "A" } else { "B" })), ("sz", json!(o.exec)),
                   ("px", json!(dec(o.cum).div(&dec(o.exec)).unwrap().to_s_f())), ("time", json!(ms(o.at) + after)), ("tid", json!(tid))] {
        f[k] = v;
    }
    f
}
/// D-HL-HISTORY-PUBLIC-RECORDED, the named edit: the scrub replaced each amount on its own, so an order's sizes are
/// restated from its recorded fills (origSz = what is left + Σ fill sz; a filled order leaves 0).
fn hl_consistent(rows: &mut [Value], fills: &[Value]) {
    for r in rows.iter_mut() {
        let oid = r["order"]["oid"].clone();
        let filled = fills.iter().filter(|f| f["oid"] == oid).fold(dec("0"), |a, f| &a + &dec(f["sz"].as_str().unwrap()));
        if !filled.is_positive() { continue; }
        let left = if r["status"] == "filled" { dec("0") } else { dec(r["order"]["sz"].as_str().unwrap()) };
        r["order"]["origSz"] = json!((&left + &filled).to_s_f());
    }
}
async fn hl_read(tr: &ScriptedTransport, coin: &str, base: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<deltabadger::venue::Created, deltabadger::venue::HistoryError> {
    hl_venue_for(tr, coin, base).orders_created(&[coin.to_string()], from, to, tokio::time::Instant::now() + std::time::Duration::from_secs(20)).await
}
fn window() -> (DateTime<Utc>, DateTime<Utc>) { (t("2026-09-29T13:53:00Z"), t("2026-09-29T14:13:00Z")) }

#[tokio::test]
async fn every_recorded_hyperliquid_order_reads_strictly_by_its_newest_row_and_its_fills() {
    let fills = hl_fills_recorded();
    let mut rows = hl_recorded();
    hl_consistent(&mut rows, &fills);
    let at = |r: &Value| DateTime::<Utc>::from_timestamp_millis(r["order"]["timestamp"].as_i64().unwrap()).unwrap();
    let (from, to) = (rows.iter().map(at).min().unwrap(), rows.iter().map(at).max().unwrap());
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, Value::Array(rows.clone()));
    tr.reply(HL_FILLS, 200, Value::Array(fills.clone()));
    let read = hl_read(&tr, "@107", "HYPE", from, to).await.unwrap();
    assert!(read.complete, "{} rows, fewer than the cap", rows.len());
    let mut states = std::collections::BTreeMap::new();
    for o in &read.orders {
        *states.entry(format!("{:?}", o.state)).or_insert(0) += 1;
        assert!(o.external_id.starts_with("HYPE-"));
        if o.filled_base.is_positive() {
            let oid: u64 = o.external_id[5..].parse().unwrap();
            let mine: Vec<&Value> = fills.iter().filter(|f| f["oid"] == json!(oid)).collect();
            let quote = mine.iter().fold(dec("0"), |a, f| &a + &(&dec(f["px"].as_str().unwrap()) * &dec(f["sz"].as_str().unwrap())));
            assert_eq!(o.filled_quote, Some(quote), "the quote is the order's own fills");
        } else {
            assert_eq!(o.filled_quote, Some(dec("0")));
        }
    }
    assert_eq!(states, [("Cancelled".to_string(), 1106), ("Filled".into(), 31), ("PartlyFilledCancelled".into(), 9)].into_iter().collect());
    assert_eq!(read.orders.len(), rows.len());
    // The request bodies: the account's orders, then its fills from the window's start.
    let bodies: Vec<Value> = tr.requests().iter().map(|r| r.body.clone().unwrap()).collect();
    assert_eq!(bodies, [json!({ "type": "historicalOrders", "user": HL_USER }), json!({ "type": "userFillsByTime", "user": HL_USER, "startTime": from.timestamp_millis() })]);
    let open: Vec<Value> = serde_json::from_str(include_str!("fixtures/order_lists/hyperliquid/frontend_open_orders.json")).unwrap();
    assert!(open.is_empty(), "the recorded account had nothing resting");
}

#[tokio::test]
async fn each_hyperliquid_order_state_is_named_as_the_venue_lists_it() {
    let placed = "placed 2026-09-29 14:03:05 UTC";
    let cases: [(O, &str, &str, usize); 5] = [
        (limit(7, St::Filled, "0.0004", "20.01"), ru::LIVE_LISTED, "bought 0.0004 BTC for 20.01 EUR (filled)", 2),
        (limit(7, St::Open, "0", "0"), ru::LIVE_OPEN, "buy of 0.0004 BTC at 50000 EUR, still open", 2),
        (limit(7, St::Partly, "0.0001", "5"), ru::LIVE_OPEN, "buy of 0.0004 BTC at 50000 EUR, 0.0001 BTC filled for 5 EUR, still open", 2),
        (limit(7, St::PartlyCancelled, "0.0002", "10"), ru::LIVE_LISTED, "buy of 0.0004 BTC at 50000 EUR cancelled, 0.0002 BTC filled for 10 EUR", 2),
        (limit(7, St::Cancelled, "0", "0"), ru::LIVE_LISTED, "buy of 0.0004 BTC at 50000 EUR cancelled, nothing filled", 1),
    ];
    for (order, event, text, requests) in &cases {
        let f = fx(Kind::Hyperliquid);
        f.answer(std::slice::from_ref(order));
        assert_eq!(f.pass(now()).await, 1);
        assert_eq!(f.one(), (event.to_string(), json!({ "when": WHEN, "venue": "Hyperliquid", "orders": format!("{text}, {placed}"), "count": 1, "complete": true, "conflict": false,
                                        "obligation": f.c.query_row("SELECT id FROM rust_rails_unrecorded", [], |r| r.get::<_, i64>(0)).unwrap() })));
        assert_eq!(f.pass(now() + Duration::hours(1)).await, 0, "one line, ever");
        assert_eq!(f.requests(), *requests, "{text}: fills read only for an open or filled order");
    }
}

#[tokio::test]
async fn a_hyperliquid_order_held_on_another_side_outside_or_none_leaves_a28_1s_reach_line() {
    for case in ["held", "sell", "empty", "outside"] {
        let f = fx(Kind::Hyperliquid);
        let oracle = f.a28_1();
        match case {
            "held" => {
                f.c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
                             VALUES (?1, ?2, 'BTC-7', 0, 0, 'REGULAR', '[]', '2026-09-22 14:03:00', '2026-09-22 14:03:00')", params![f.b, f.s.exchange_id]).unwrap();
                f.answer(&[limit(7, St::Filled, "0.0004", "20")]);
            }
            "sell" => f.answer(&[O { sell: true, ..limit(7, St::Filled, "0.0004", "20") }]),
            "outside" => f.answer(&[O { at: "2026-09-29T14:20:00Z", ..limit(7, St::Open, "0", "0") }]),
            _ => f.answer(&[]),
        }
        assert_eq!(f.pass(now()).await, 1);
        assert_eq!(f.one(), oracle, "{case}");
        // D-HL-HISTORY-REACH-KEPT: the ledger's reach is short; the live read never turns it into "none seen".
        assert_eq!((oracle.0.as_str(), oracle.1["reason"].as_str()), (ru::UNCHECKED, Some("reach")));
    }
}

#[tokio::test]
async fn a_hyperliquid_order_keeps_its_newest_row_and_an_open_rows_fill_is_its_fills() {
    let (from, to) = window();
    let order = limit(7, St::Partly, "0.0001", "5");
    let placement = hl_row(&limit(7, St::Open, "0", "0"));
    for final_first in [true, false] {
        // The placement row and the final one, in either order; the final one is the order's.
        let fin = hl_row(&limit(7, St::Filled, "0.0004", "20"));
        let tr = ScriptedTransport::default();
        tr.reply(HL_ORDERS, 200, if final_first { json!([fin, placement]) } else { json!([placement, fin]) });
        tr.reply(HL_FILLS, 200, json!([hl_fill(&limit(7, St::Filled, "0.0004", "20"), 1, 500)]));
        let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
        assert_eq!(read.orders.iter().map(|o| format!("{:?}", o.state)).collect::<Vec<_>>(), ["Filled"], "final first {final_first}");
    }
    // A placement row and a final row with the same status time: the final one; two final rows that tie: unreadable.
    let mut fin = hl_row(&limit(7, St::Cancelled, "0", "0"));
    fin["statusTimestamp"] = placement["statusTimestamp"].clone();
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([placement, fin]));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    assert_eq!((read.orders.len(), read.orders[0].state, read.complete), (1, deltabadger::venue::CreatedState::Cancelled, true));
    let mut other = hl_row(&limit(7, St::Filled, "0.0004", "20"));
    other["statusTimestamp"] = fin["statusTimestamp"].clone();
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([fin, other]));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    assert_eq!((read.orders.len(), read.complete), (0, false));
    // An open row says nothing of fills: two fills make it partly filled, open.
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([placement]));
    tr.reply(HL_FILLS, 200, json!([hl_fill(&limit(7, St::Partly, "0.00005", "2.5"), 1, 400), hl_fill(&limit(7, St::Partly, "0.00005", "2.5"), 2, 600)]));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    let o = &read.orders[0];
    assert_eq!((o.state, o.filled_base.to_s_f(), o.filled_quote.as_ref().map(|q| q.to_s_f())), (deltabadger::venue::CreatedState::PartlyFilledOpen, order.exec.to_string(), Some("5.0".into())));
}

#[tokio::test]
async fn a_hyperliquid_order_whose_fills_dont_state_its_fill_is_unreadable() {
    let (from, to) = window();
    let cases: [(&str, O, Vec<Value>); 7] = [
        ("final, fills short", limit(7, St::Filled, "0.0004", "20"), vec![hl_fill(&limit(7, St::Partly, "0.0001", "5"), 1, 500)]),
        ("final, no fill", limit(7, St::PartlyCancelled, "0.0002", "10"), vec![]),
        ("open, fills the whole size", limit(7, St::Partly, "0.0001", "5"), vec![hl_fill(&limit(7, St::Filled, "0.0004", "20"), 1, 500)]),
        ("another coin", limit(7, St::Filled, "0.0004", "20"), vec![{ let mut f = hl_fill(&limit(7, St::Filled, "0.0004", "20"), 1, 500); f["coin"] = json!("@7"); f }]),
        ("another side", limit(7, St::Filled, "0.0004", "20"), vec![{ let mut f = hl_fill(&limit(7, St::Filled, "0.0004", "20"), 1, 500); f["side"] = json!("A"); f }]),
        ("no price", limit(7, St::Filled, "0.0004", "20"), vec![{ let mut f = hl_fill(&limit(7, St::Filled, "0.0004", "20"), 1, 500); f["px"] = json!("0"); f }]),
        // An order id that can't be read may be this order's: an open order is never read as unfilled beside it.
        ("unreadable oid", limit(7, St::Open, "0", "0"), vec![{ let mut f = hl_fill(&limit(7, St::Partly, "0.0001", "5"), 1, 500); f["oid"] = json!("7"); f }]),
    ];
    for (name, order, fills) in cases {
        let tr = ScriptedTransport::default();
        tr.reply(HL_ORDERS, 200, json!([hl_row(&order), hl_row(&limit(8, St::Cancelled, "0", "0"))]));
        tr.reply(HL_FILLS, 200, Value::Array(fills));
        let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
        assert_eq!((read.orders.iter().map(|o| o.external_id.as_str()).collect::<Vec<_>>(), read.complete), (vec!["BTC-8"], false), "{name}");
    }
}

#[tokio::test]
async fn hyperliquid_fills_are_one_per_time_coin_and_tid() {
    let (from, to) = window();
    let half = limit(7, St::Partly, "0.0002", "10");
    // Overlapping pages repeat a fill: counted once. The same tid at another time is another fill.
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Filled, "0.0004", "20"))]));
    tr.reply(HL_FILLS, 200, json!([hl_fill(&half, 1, 400), hl_fill(&half, 1, 400), hl_fill(&half, 1, 600)]));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    assert_eq!((read.orders.len(), read.complete), (1, true));
    // One identity with two bodies: no fill read is evidence.
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Filled, "0.0004", "20")), hl_row(&limit(8, St::Cancelled, "0", "0"))]));
    let mut twin = hl_fill(&half, 1, 400);
    twin["px"] = json!("50001");
    tr.reply(HL_FILLS, 200, json!([hl_fill(&half, 1, 400), twin, hl_fill(&half, 2, 600)]));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    assert_eq!((read.orders.iter().map(|o| o.external_id.as_str()).collect::<Vec<_>>(), read.complete), (vec!["BTC-8"], false));
}

#[tokio::test]
async fn hyperliquid_fills_are_paged_to_their_end_and_capped() {
    let (from, to) = window();
    let filler = |n: u64| { let mut f = hl_fill(&O { id: 99, ..limit(99, St::Filled, "0.0004", "20") }, 10_000 + n, 100); f["time"] = json!(ms(PLACED) + n as i64); f };
    let full: Vec<Value> = (0..2000).map(filler).collect();
    // A full page, then the short one after it (from the newest time seen): the open order's fill is on the second page.
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Open, "0", "0"))]));
    tr.reply(HL_FILLS, 200, Value::Array(full.clone()));
    tr.reply(HL_FILLS, 200, json!([hl_fill(&limit(7, St::Partly, "0.0001", "5"), 1, 5000)]));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    assert_eq!((read.orders[0].state, read.complete), (deltabadger::venue::CreatedState::PartlyFilledOpen, true));
    let starts: Vec<Value> = tr.requests().iter().filter(|r| r.body.as_ref().unwrap()["type"] == "userFillsByTime").map(|r| r.body.as_ref().unwrap()["startTime"].clone()).collect();
    assert_eq!(starts, [json!(from.timestamp_millis()), json!(ms(PLACED) + 1999)]);
    // Full pages past the cap: the open order is unreadable (nothing else read: retried); a filled order whose fills
    // already add up is kept (more fills would exceed it).
    for (orders, kept) in [(vec![limit(7, St::Open, "0", "0")], None), (vec![limit(7, St::Open, "0", "0"), limit(8, St::Filled, "0.0004", "20")], Some("BTC-8"))] {
        let tr = ScriptedTransport::default();
        tr.reply(HL_ORDERS, 200, Value::Array(orders.iter().map(hl_row).collect()));
        let mut first = full.clone();
        first[0] = hl_fill(&limit(8, St::Filled, "0.0004", "20"), 2, 300);
        tr.reply(HL_FILLS, 200, Value::Array(first));
        tr.reply(HL_FILLS, 200, Value::Array(full.clone()));
        let read = hl_read(&tr, HL_COIN, "BTC", from, to).await;
        match kept {
            None => assert!(matches!(read, Ok(ref c) if c.orders.is_empty() && !c.complete), "{read:?}"),
            Some(id) => { let c = read.unwrap(); assert_eq!((c.orders.iter().map(|o| o.external_id.as_str()).collect::<Vec<_>>(), c.complete), (vec![id], false)); }
        }
        assert_eq!(tr.requests().len(), 1 + hyperliquid::HISTORY_FILL_PAGES);
    }
}

#[tokio::test]
async fn a_failed_hyperliquid_fills_read_is_retried_unless_an_order_was_read() {
    let (from, to) = window();
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Filled, "0.0004", "20"))]));
    tr.reply(HL_FILLS, 503, json!("busy"));
    assert!(matches!(hl_read(&tr, HL_COIN, "BTC", from, to).await, Err(deltabadger::venue::HistoryError::Retry(_))));
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Filled, "0.0004", "20")), hl_row(&limit(8, St::Cancelled, "0", "0"))]));
    tr.reply(HL_FILLS, 503, json!("busy"));
    let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
    assert_eq!((read.orders.iter().map(|o| o.external_id.as_str()).collect::<Vec<_>>(), read.complete), (vec!["BTC-8"], false));
    // Through the report: a pass writes nothing, the next one reads again.
    let f = fx(Kind::Hyperliquid);
    f.tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Filled, "0.0004", "20"))]));
    f.tr.reply(HL_FILLS, 503, json!("busy"));
    f.tr.reply(HL_FILLS, 200, json!([hl_fill(&limit(7, St::Filled, "0.0004", "20"), 1, 500)]));
    assert_eq!(f.pass(now()).await, 0);
    assert_eq!(f.pass(now() + Duration::hours(1)).await, 1);
    assert_eq!(f.one().0, ru::LIVE_LISTED);
}

#[tokio::test]
async fn a_full_hyperliquid_answer_covers_the_window_when_its_oldest_status_is_before_it() {
    let (from, to) = window();
    for (oldest, complete) in [("2026-09-29T13:52:59Z", true), ("2026-09-29T13:53:00Z", false)] {
        let mut rows: Vec<Value> = (0..hyperliquid::HISTORY_LIMIT as u64).map(|n| hl_row(&O { sell: true, ..limit(100 + n, St::Cancelled, "0", "0") })).collect();
        let last = rows.len() - 1;
        rows[last]["statusTimestamp"] = json!(ms(oldest));
        rows[last]["order"]["timestamp"] = json!(ms(oldest) - 1000);
        rows[0] = hl_row(&limit(7, St::Cancelled, "0", "0"));
        let tr = ScriptedTransport::default();
        tr.reply(HL_ORDERS, 200, Value::Array(rows));
        let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
        assert_eq!(read.complete, complete, "oldest status {oldest}");
    }
}

#[tokio::test]
async fn an_unreadable_hyperliquid_row_of_the_pair_is_incomplete_and_anothers_is_skipped() {
    let (from, to) = window();
    type Spoil = (&'static str, fn(&mut Value));
    let spoils: [Spoil; 7] = [
        ("no creation time", |r| { r["order"].as_object_mut().unwrap().remove("timestamp"); }),
        ("odd side", |r| r["order"]["side"] = json!("X")),
        ("triggered", |r| r["status"] = json!("triggered")),
        ("a market order", |r| r["order"]["orderType"] = json!("Market")),
        ("a trigger", |r| r["order"]["isTrigger"] = json!(true)),
        ("more left than ordered", |r| r["order"]["sz"] = json!("0.0005")),
        ("filled with some left", |r| { r["status"] = json!("filled"); r["order"]["sz"] = json!("0.0001"); }),
    ];
    for (name, spoil) in spoils {
        for (coin, complete) in [(HL_COIN, false), ("@7", true)] {
            let mut bad = hl_row(&limit(8, St::Cancelled, "0", "0"));
            spoil(&mut bad);
            bad["order"]["coin"] = json!(coin);
            let tr = ScriptedTransport::default();
            tr.reply(HL_ORDERS, 200, json!([bad, hl_row(&limit(9, St::Cancelled, "0", "0"))]));
            let read = hl_read(&tr, HL_COIN, "BTC", from, to).await.unwrap();
            assert_eq!((read.orders.iter().map(|o| o.external_id.as_str()).collect::<Vec<_>>(), read.complete), (vec!["BTC-9"], complete), "{name} on {coin}");
        }
    }
    // A pair the venue's tickers don't name has no "<base>-<oid>": unreadable.
    let tr = ScriptedTransport::default();
    tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(9, St::Cancelled, "0", "0"))]));
    let read = hl_venue_for(&tr, "@999", "BTC").orders_created(&[HL_COIN.to_string()], from, to, tokio::time::Instant::now() + std::time::Duration::from_secs(20)).await.unwrap();
    assert_eq!((read.orders.len(), read.complete), (0, false));
}
