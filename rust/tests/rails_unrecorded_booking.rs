//! A28-2a (PLAN-A28-2 Final shape): booking an order Rails sent but never recorded, on the bot owner's confirmation. The
//! report's live line stores its bookable orders as offers; the web's request is served by the engine's pass
//! (`serve_bookings`), which re-reads the order by the history read's own contract, books it as Rails'
//! `persist_accepted_order!` writes a row, and the ordinary follow-up poll settles it. Real Binance and Bybit venues over
//! `http::ScriptedTransport`, hand-written bodies (and, for parity, the Rails recorder's). No live venue call.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, Seeded};
use deltabadger::engine::accounting;
use deltabadger::engine::model;
use deltabadger::engine::polling;
use deltabadger::engine::rails_unrecorded::{self as ru, Refused, Report};
use deltabadger::jobs::Db;
use deltabadger::ruby::BigDec;
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::{binance, bybit, Venue, VenueFactory};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;

fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn now() -> DateTime<Utc> { t("2026-09-30T12:00:00Z") }
const AT: &str = "2026-09-29 14:03:00";
const PLACED: &str = "2026-09-29T14:03:05Z";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Binance, Bybit }
impl Kind {
    fn exchange(self) -> (&'static str, &'static str) { match self { Kind::Binance => ("Exchanges::Binance", "Binance"), Kind::Bybit => ("Exchanges::Bybit", "Bybit") } }
    fn external_id(self, id: u64) -> String { if self == Kind::Bybit { id.to_string() } else { format!("BTCEUR-{id}") } }
}

/// The scripted venue for every bot, after `hook` (what happens between the read and the write).
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

/// A running weekly bot on `kind` with an ambiguous Rails tick at AT, collected; its ledger synced after the window.
fn fx(kind: Kind) -> Fx { fx_with(kind, BotSpec::weekly(20.0, "2026-09-01 10:00:00"), "BTCEUR", "2026-09-22 14:03:00", AT, now()) }

fn fx_with(kind: Kind, spec: BotSpec, symbol: &str, prior: &str, at: &str, collected: DateTime<Utc>) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.sqlite3");
    let c = open(&path);
    deltabadger::store::create_schema(&c).unwrap();
    deltabadger::store::test_support::migrate(&c).unwrap();
    let s = seed::seed_kraken(&c, &seed::cipher());
    let (ty, name) = kind.exchange();
    c.execute("UPDATE exchanges SET type = ?2, name = ?3 WHERE id = ?1", params![s.exchange_id, ty, name]).unwrap();
    c.execute("UPDATE tickers SET ticker = ?2, base = 'BTC', price_decimals = 2 WHERE id = ?1", params![s.ticker_id, symbol]).unwrap();
    let b = seed::insert_bot(&c, &s, &spec);
    prior_row(&c, &s, b, prior);
    ambiguous_line(&c, b, at);
    ru::collect(&c, &queue(), None, collected).unwrap();
    synced(&c, s.api_key_id, collected);
    let db = Db::new(open(&path), seed::cipher());
    Fx { _dir: dir, path, c, s, b, kind, tr: ScriptedTransport::default(), db }
}
fn prior_row(c: &Connection, s: &Seeded, b: i64, at: &str) {
    let selling = model::load_bot(c, b).unwrap().selling();
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, base_asset_id, quote_asset_id, transaction_type, \
               amount, amount_exec, quote_amount, quote_amount_exec, price, error_messages, created_at, updated_at) \
               VALUES (?1, ?2, ?3, 0, 2, ?6, ?4, ?5, 'REGULAR', 0.0004, 0.0004, 20, 20, 50000, '[]', ?7, ?7)",
              params![b, s.exchange_id, format!("PRIOR-{b}-{at}"), s.btc, s.quote, i64::from(selling), at]).unwrap();
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
fn dump(c: &Connection, sql: &str) -> String {
    let mut s = c.prepare(sql).unwrap();
    let n = s.column_count();
    s.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join(","))).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(";")
}
/// The money guard: no order row, carry, figure input or ledger link moves, and no resolution is written.
fn money(c: &Connection) -> Vec<String> {
    vec![dump(c, "SELECT * FROM transactions ORDER BY id"), dump(c, "SELECT id, settings, transient_data, status, started_at, settings_changed_at FROM bots ORDER BY id"),
         dump(c, "SELECT id, transaction_id, linked_transaction_id FROM account_transactions ORDER BY id"), dump(c, "SELECT * FROM rust_rails_unrecorded_resolutions"),
         dump(c, "SELECT * FROM rust_rails_unrecorded_offers ORDER BY id")]
}
fn lines(c: &Connection, bot: i64, event: &str) -> Vec<Value> {
    let mut s = c.prepare("SELECT details FROM bot_activity_logs WHERE bot_id = ?1 AND event = ?2 ORDER BY id").unwrap();
    s.query_map(params![bot, event], |r| r.get::<_, String>(0)).unwrap().map(|r| serde_json::from_str(&r.unwrap()).unwrap()).collect()
}
fn count(c: &Connection, sql: &str) -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() }

#[derive(Clone, Copy, PartialEq)]
enum St { Open, Partly, Filled, PartlyCancelled, Cancelled }
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
            "orderStatus": status, "orderType": ty, "marketUnit": unit, "avgPrice": "", "cumExecQty": o.exec, "cumExecValue": o.cum, "cumExecFee": "0", "leavesQty": "0",
            "timeInForce": if o.limit.is_some() { "GTC" } else { "IOC" }, "createdTime": ms(o.at).to_string(), "updatedTime": (ms(o.at) + 1000).to_string() })
}
fn page(rows: Vec<Value>) -> Value { json!({ "retCode": 0, "retMsg": "OK", "result": { "category": "spot", "list": rows, "nextPageCursor": "" } }) }
/// One Binance trade of `order`, as myTrades lists it.
fn trade(id: i64, order: u64, qty: &str, quote: &str, commission: &str, asset: &str, at: &str) -> Value {
    json!({ "symbol": "BTCEUR", "id": id, "orderId": order, "orderListId": -1, "price": "50000", "qty": qty, "quoteQty": quote, "commission": commission,
            "commissionAsset": asset, "time": ms(at), "isBuyer": true, "isMaker": false, "isBestMatch": true })
}
/// The same trade stored as the ledger importer stores it.
fn fill(c: &Connection, s: &Seeded, id: i64, order: u64, qty: &str, quote: &str, at: &str) -> i64 {
    let raw = deltabadger::sync::activities::Raw::from_value(trade(id, order, qty, quote, "0", "EUR", at));
    let legs = deltabadger::sync::binance_ledger::normalize_trade(&raw, "BTCEUR", "BTC", "EUR").unwrap();
    let e = &legs[0];
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
               raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'BTC', ?6, 'EUR', ?7, ?8, ?9, ?9, ?9)",
              params![s.user_id, s.exchange_id, s.api_key_id, e.tx_id, e.entry_type, qty, quote, e.raw.value.to_string(),
                      deltabadger::codec::format_time(e.transacted_at.unwrap())]).unwrap();
    c.last_insert_rowid()
}

impl Fx {
    fn binance(&self) -> binance::BinanceVenue<ScriptedTransport> { binance::BinanceVenue::new(self.tr.clone(), binance::URL, "secret") }
    fn bybit(&self) -> bybit::BybitVenue<ScriptedTransport> { bybit::BybitVenue::new(self.tr.clone(), bybit::URL).with_history_db(self.path.clone(), now() + Duration::days(40)) }
    /// The venue lists these orders (both the report's read and the booking's identity read: the last reply repeats).
    fn answer(&self, orders: &[O]) {
        match self.kind {
            Kind::Bybit => {
                let (open, closed): (Vec<&O>, Vec<&O>) = orders.iter().partition(|o| matches!(o.state, St::Open | St::Partly));
                self.tr.reply("GET /v5/order/realtime", 200, page(open.iter().map(|o| bybit_row(o)).collect()));
                self.tr.reply("GET /v5/order/history", 200, page(closed.iter().map(|o| bybit_row(o)).collect()));
            }
            Kind::Binance => { self.tr.reply("GET /api/v3/allOrders", 200, Value::Array(orders.iter().map(binance_row).collect())); }
        }
    }
    /// The report's pass: the line and its offers.
    async fn report(&self) {
        let n = match self.kind {
            Kind::Binance => Report::new(Factory { venue: self.binance(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Bybit => Report::new(Factory { venue: self.bybit(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
        }.unwrap();
        assert_eq!(n, 1);
    }
    /// The line's obligation and its offers (offer id, external id).
    fn offers(&self) -> (i64, Vec<(i64, String)>) {
        let line: Value = self.c.query_row("SELECT details FROM bot_activity_logs WHERE bot_id = ?1 AND event IN ('rails_order_open', 'rails_order_listed') ORDER BY id DESC LIMIT 1",
                                           [self.b], |r| r.get::<_, String>(0)).map(|d| serde_json::from_str(&d).unwrap()).unwrap();
        let obligation = line["obligation"].as_i64().unwrap();
        let mut s = self.c.prepare("SELECT id, external_id FROM rust_rails_unrecorded_offers WHERE obligation_id = ?1 ORDER BY id").unwrap();
        let offers = s.query_map([obligation], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        (obligation, offers)
    }
    fn request(&self, obligation: i64, offer: i64) -> Result<(), Refused> { ru::request_booking(&self.c, self.s.user_id, self.b, obligation, offer, now()).unwrap() }
    /// One engine pass of `serve_bookings` at `at`, `hook` run between the identity read and the write.
    async fn serve_with(&self, at: DateTime<Utc>, hook: Option<Box<dyn FnOnce()>>) -> ru::Served {
        let hook = RefCell::new(hook);
        match self.kind {
            Kind::Binance => ru::serve_bookings(&self.c, &Factory { venue: self.binance(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Bybit => ru::serve_bookings(&self.c, &Factory { venue: self.bybit(), hook }, &seed::cipher(), &|_| false, at).await,
        }.unwrap()
    }
    async fn serve(&self) -> ru::Served { self.serve_with(now(), None).await }
    /// The report, the owner's confirmation of the first offer, the engine's pass: the booked row.
    async fn booked(&self, orders: &[O]) -> i64 {
        self.answer(orders);
        self.report().await;
        let (obligation, offers) = self.offers();
        self.request(obligation, offers[0].0).unwrap();
        let served = self.serve().await;
        assert_eq!(served.booked.len(), 1, "{:?}", lines(&self.c, self.b, ru::NOT_ADDED));
        served.booked[0].1
    }
    async fn follow_up(&self, tx: i64, at: DateTime<Utc>) -> Result<polling::FollowUp, polling::PollFailure> {
        match self.kind {
            Kind::Binance => polling::follow_up(&self.c, &self.binance(), self.b, tx, at).await,
            Kind::Bybit => polling::follow_up(&self.c, &self.bybit(), self.b, tx, at).await,
        }
    }
    fn row(&self, tx: i64) -> Value {
        self.c.query_row("SELECT json_object('status', status, 'external_status', external_status, 'external_id', external_id, 'side', side, 'order_type', order_type, \
                          'amount', amount, 'quote_amount', quote_amount, 'price', price, 'amount_exec', amount_exec, 'quote_amount_exec', quote_amount_exec, \
                          'bot_interval', bot_interval, 'bot_quote_amount', bot_quote_amount, 'created_at', created_at, 'run_identity', run_identity, \
                          'transaction_type', transaction_type, 'base_asset_id', base_asset_id) FROM transactions WHERE id = ?1", [tx],
                         |r| r.get::<_, String>(0)).map(|s| serde_json::from_str(&s).unwrap()).unwrap()
    }
    fn carry(&self, at: &str) -> BigDec {
        accounting::pending_quote_amount(&self.c, &model::load_bot(&self.c, self.b).unwrap(), deltabadger::codec::parse_time(at).unwrap().timestamp_micros()).unwrap()
    }
}

// ---- offers (§1) ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_line_stores_its_bookable_orders_as_offers_and_names_its_obligation() {
    for kind in [Kind::Binance, Kind::Bybit] {
        let f = fx(kind);
        f.answer(&[o(7, St::Filled, "0.0004", "20"), limit(8, St::Open, "0", "0"), limit(9, St::Partly, "0.0001", "5"),
                   o(10, St::PartlyCancelled, "0.0002", "10"), o(11, St::Cancelled, "0", "0")]);
        f.report().await;
        let (obligation, offers) = f.offers();
        let mut ids: Vec<String> = offers.into_iter().map(|(_, e)| e).collect();
        ids.sort_by_key(|e| e.rsplit('-').next().unwrap().parse::<u64>().unwrap());
        assert_eq!(ids, [7, 8, 9, 10].map(|n| kind.external_id(n)), "{kind:?}: cancelled with nothing filled is listed, never offered");
        assert_eq!(obligation, count(&f.c, "SELECT id FROM rust_rails_unrecorded"));
    }
}

#[tokio::test]
async fn a_ledger_only_candidate_or_an_unserved_venue_offers_nothing() {
    let f = fx(Kind::Binance);
    fill(&f.c, &f.s, 50, 5, "0.0004", "20.01", "2026-09-29T14:03:01Z");
    f.answer(&[]);
    f.report().await;
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_offers"), 0);
    let f = fx(Kind::Binance);
    f.c.execute("UPDATE exchanges SET type = 'Exchanges::Kraken', name = 'Kraken' WHERE id = ?1", [f.s.exchange_id]).unwrap();
    f.report().await;
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_offers"), 0);
}

// ---- one booking: Rails' accepted row, then the follow-up poll (r2.1, r2.2) ------------------------------------------------

#[tokio::test]
async fn a_booking_writes_rails_accepted_row_and_the_follow_up_settles_it() {
    let f = fx(Kind::Binance);
    let mine = fill(&f.c, &f.s, 50, 7, "0.0004", "20", "2026-09-29T14:03:06Z");
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    assert_eq!(f.request(obligation, offers[0].0), Ok(()));
    let served = f.serve().await;
    let tx = served.booked[0].1;
    assert_eq!(served.booked, [(f.b, tx)]);
    let row = f.row(tx);
    assert_eq!(row, json!({ "status": 0, "external_status": 0, "external_id": "BTCEUR-7", "side": 0, "order_type": 0, "amount": null, "quote_amount": 20,
                            "price": 50000, "amount_exec": null, "quote_amount_exec": null, "bot_interval": "week", "bot_quote_amount": 20, "created_at": AT,
                            "run_identity": null, "transaction_type": "REGULAR", "base_asset_id": f.s.btc }), "no intent, route or reservation");
    assert_eq!(count(&f.c, "SELECT count(*) FROM ibkr_reservations"), 0);
    assert_eq!(f.c.query_row("SELECT transaction_id FROM account_transactions WHERE id = ?1", [mine], |r| r.get::<_, Option<i64>>(0)).unwrap(), Some(tx), "its fill linked");
    let resolution: (i64, String, i64, i64, String, String) = f.c.query_row("SELECT obligation_id, resolution, offer_id, transaction_id, confirmed_base, confirmed_quote \
        FROM rust_rails_unrecorded_resolutions", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
    assert_eq!(resolution, (obligation, "booked".into(), offers[0].0, tx, "0.0004".into(), "20.0".into()));
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_requests"), 0);
    let recovered = lines(&f.c, f.b, ru::RECOVERED);
    assert_eq!(recovered, [json!({ "obligation": obligation, "transaction_id": tx, "venue_created_at": "2026-09-29 14:03:05",
                                   "order": "bought 0.0004 BTC for 20 EUR (filled), placed 2026-09-29 14:03:05 UTC" })]);
    assert_eq!(model::load_bot(&f.c, f.b).unwrap().transient[model::HISTORY_GENERATION], json!(1));
    // FetchAndUpdateOrderJob: GET order, then its trades with a quote commission.
    f.tr.reply("GET /api/v3/order", 200, binance_row(&o(7, St::Filled, "0.0004", "20")));
    f.tr.reply("GET /api/v3/myTrades", 200, json!([trade(50, 7, "0.0004", "20", "0.02", "EUR", "2026-09-29T14:03:06Z")]));
    assert_eq!(f.follow_up(tx, now()).await, Ok(polling::FollowUp::Done));
    let row = f.row(tx);
    assert_eq!((row["external_status"].clone(), row["amount_exec"].clone()), (json!(2), json!(0.0004)));
    // r3.1(c): the settled execution equals the ledger's on the gross basis.
    assert_eq!(row["amount_exec"], json!(0.0004));
    assert_eq!(f.serve().await, ru::Served::default(), "settled: nothing more to book or watch");
}

#[tokio::test]
async fn each_bookable_state_is_booked_and_an_open_one_is_polled_to_closed() {
    for kind in [Kind::Binance, Kind::Bybit] {
        for (order, value) in [(limit(7, St::Open, "0", "0"), "0.0004"), (limit(7, St::Partly, "0.0001", "5"), "0.0004"),
                               (o(7, St::PartlyCancelled, "0.0002", "10"), "20")] {
            let f = fx(kind);
            let tx = f.booked(std::slice::from_ref(&order)).await;
            let row = f.row(tx);
            assert_eq!(row["external_status"], json!(0), "{kind:?}: unknown until the poll");
            if order.limit.is_some() { assert_eq!((row["amount"].clone(), row["quote_amount"].clone()), (json!(value.parse::<f64>().unwrap()), Value::Null)); }
            else { assert_eq!((row["amount"].clone(), row["quote_amount"].clone(), row["price"].clone()), (Value::Null, json!(20), json!(50000))); }
        }
        // An open order, then filled: the poller settles it.
        let mut f = fx(kind);
        let tx = f.booked(&[limit(7, St::Open, "0", "0")]).await;
        f.tr = ScriptedTransport::default();
        match kind {
            Kind::Binance => {
                f.tr.reply("GET /api/v3/order", 200, binance_row(&limit(7, St::Open, "0", "0")));
                assert_eq!(f.follow_up(tx, now()).await, Ok(polling::FollowUp::StillOpen));
                f.tr = ScriptedTransport::default();
                f.tr.reply("GET /api/v3/order", 200, binance_row(&limit(7, St::Filled, "0.0004", "20")));
                f.tr.reply("GET /api/v3/myTrades", 200, json!([trade(50, 7, "0.0004", "20", "0", "BNB", PLACED)]));
            }
            Kind::Bybit => {
                f.tr.reply("GET /v5/order/realtime", 200, page(vec![bybit_row(&limit(7, St::Open, "0", "0"))]));
                assert_eq!(f.follow_up(tx, now()).await, Ok(polling::FollowUp::StillOpen));
                f.tr = ScriptedTransport::default();
                // Realtime no longer lists it: the history read finds it by the row's own created_at.
                f.tr.reply("GET /v5/order/realtime", 200, page(vec![]));
                f.tr.reply("GET /v5/order/history", 200, page(vec![bybit_row(&limit(7, St::Filled, "0.0004", "20"))]));
            }
        }
        assert_eq!(f.follow_up(tx, now()).await, Ok(polling::FollowUp::Done), "{kind:?}");
        assert_eq!(f.row(tx)["external_status"], json!(2), "{kind:?}");
        if kind == Kind::Bybit {
            let last = f.tr.requests().into_iter().rev().find(|r| r.path == bybit::HISTORY_PATH).unwrap();
            let start = last.query.iter().find(|(k, _)| *k == "startTime").unwrap().1.clone();
            assert_eq!(start, (t("2026-09-29T14:03:00Z") - Duration::hours(1)).timestamp_millis().to_string(), "searched from the row's created_at");
        }
    }
}

// ---- refusals write nothing (§2.3, r2.1) ---------------------------------------------------------------------------------

#[tokio::test]
async fn every_refusal_writes_nothing_but_its_line() {
    type Setup = fn(&mut Fx);
    let cases: [(&str, Setup, &str); 10] = [
        ("nothing filled at confirm", |f| { f.tr = ScriptedTransport::default(); f.answer(&[o(7, St::Cancelled, "0", "0")]); }, "nothing_filled"),
        ("unknown cost", |f| { f.tr = ScriptedTransport::default(); f.answer(&[o(7, St::Filled, "0.0004", "-1.00000000")]); }, "no_cost"),
        ("gone from the venue", |f| { f.tr = ScriptedTransport::default(); f.answer(&[]); }, "not_found"),
        ("held by another bot", |f| {
            let other = seed::insert_bot(&f.c, &f.s, &BotSpec::weekly(20.0, "2026-09-01 10:00:00"));
            f.c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
                         VALUES (?1, ?2, 'BTCEUR-7', 0, 0, 'REGULAR', '[]', ?3, ?3)", params![other, f.s.exchange_id, AT]).unwrap();
        }, "held"),
        ("a fill linked", |f| {
            let x = fill(&f.c, &f.s, 60, 7, "0.0001", "5", "2026-09-29T16:00:00Z");
            f.c.execute("UPDATE account_transactions SET transaction_id = (SELECT min(id) FROM transactions) WHERE id = ?1", [x]).unwrap();
        }, "linked"),
        ("the ledger shows more", |f| { fill(&f.c, &f.s, 61, 7, "0.0005", "25", "2026-09-29T14:03:06Z"); }, "ledger"),
        ("bot deleted", |f| { f.c.execute("UPDATE bots SET status = 3 WHERE id = ?1", [f.b]).unwrap(); }, "deleted"),
        ("bot moved to another exchange", |f| {
            f.c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::BinanceUs', 'Binance.US', '0.1', '0.1', ?1, ?1)", [AT]).unwrap();
            f.c.execute("UPDATE bots SET exchange_id = ?2 WHERE id = ?1", params![f.b, f.c.last_insert_rowid()]).unwrap();
        }, "settings_changed"),
        ("key replaced since", |f| { f.c.execute("UPDATE api_keys SET updated_at = '2026-09-30 09:00:00' WHERE id = ?1", [f.s.api_key_id]).unwrap(); }, "key_changed"),
        ("mid-tick", |f| { f.c.execute("UPDATE bots SET status = ?2 WHERE id = ?1", params![f.b, deltabadger::enums::BotStatus::Executing as i64]).unwrap(); }, "busy"),
    ];
    for (name, setup, reason) in cases {
        let mut f = fx(Kind::Binance);
        f.answer(&[o(7, St::Filled, "0.0004", "20")]);
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        setup(&mut f);
        let before = money(&f.c);
        assert_eq!(f.serve().await.booked, [], "{name}");
        assert_eq!(money(&f.c), before, "{name}: nothing written");
        let refused = lines(&f.c, f.b, ru::NOT_ADDED);
        assert_eq!(refused.len(), 1, "{name}");
        assert_eq!(refused[0]["reason"], reason, "{name}");
        assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_requests"), 0, "{name}: the request goes");
        if reason != "deleted" { assert_eq!(f.request(obligation, offers[0].0), Ok(()), "{name}: the offer can be asked for again"); }
    }
}

#[tokio::test]
async fn kucoin_and_ibkr_are_never_booked() {
    for ty in ["Exchanges::Kucoin", "Exchanges::Ibkr"] {
        let f = fx(Kind::Binance);
        f.answer(&[o(7, St::Filled, "0.0004", "20")]);
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        f.c.execute("UPDATE exchanges SET type = ?2 WHERE id = ?1", params![f.s.exchange_id, ty]).unwrap();
        let before = money(&f.c);
        let requests = f.tr.requests().len();
        assert_eq!(f.serve().await.booked, []);
        assert_eq!((money(&f.c), f.tr.requests().len()), (before, requests), "{ty}: nothing written, nothing read");
        assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "unsupported");
    }
}

#[tokio::test]
async fn a_key_changed_between_the_read_and_the_write_writes_nothing() {
    let f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    let before = money(&f.c);
    let (path, key) = (f.path.clone(), f.s.api_key_id);
    let hook: Box<dyn FnOnce()> = Box::new(move || { open(&path).execute("UPDATE api_keys SET secret = ?2 WHERE id = ?1", params![key, seed::cipher().encrypt("other")]).unwrap(); });
    assert_eq!(f.serve_with(now(), Some(hook)).await.booked, []);
    assert_eq!(money(&f.c), before);
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "key_changed");
}

#[tokio::test]
async fn three_transient_identity_failures_refuse_and_a_retry_waits_a_minute() {
    let mut f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    f.tr = ScriptedTransport::default();
    for _ in 0..3 { f.tr.reply("GET /api/v3/allOrders", 500, json!("<html>busy</html>")); }
    let before = money(&f.c);
    let reads = || f.tr.requests().iter().filter(|r| r.path == "/api/v3/allOrders").count();
    let first = 0;
    f.serve_with(now(), None).await;
    f.serve_with(now() + Duration::seconds(30), None).await;
    assert_eq!(reads(), first + 1, "not again within a minute");
    f.serve_with(now() + Duration::seconds(60), None).await;
    assert!(lines(&f.c, f.b, ru::NOT_ADDED).is_empty());
    f.serve_with(now() + Duration::seconds(120), None).await;
    assert_eq!(reads(), first + 3);
    assert_eq!(money(&f.c), before);
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "unreachable");
}

// ---- once: double requests, the group, "none was mine" (r1.6) -----------------------------------------------------------

#[tokio::test]
async fn a_double_request_or_a_second_pass_books_once() {
    let f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20"), o(8, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    assert_eq!(f.request(obligation, offers[0].0), Ok(()));
    assert_eq!(f.request(obligation, offers[1].0), Err(Refused::Pending));
    // Two engine passes on two connections, the second after the first committed.
    let other = open(&f.path);
    let factory = Factory { venue: f.binance(), hook: RefCell::new(None) };
    let cipher = seed::cipher();
    let (a, b) = tokio::join!(f.serve(), ru::serve_bookings(&other, &factory, &cipher, &|_| false, now()));
    assert_eq!(a.booked.len() + b.unwrap().booked.len(), 1);
    assert_eq!(count(&f.c, "SELECT count(*) FROM transactions WHERE external_id LIKE 'BTCEUR-%'"), 1);
    assert_eq!(lines(&f.c, f.b, ru::RECOVERED).len(), 1);
    assert_eq!(f.request(obligation, offers[1].0), Err(Refused::Decided));
}

#[tokio::test]
async fn one_attempt_books_one_order_whatever_source_named_it() {
    let f = fx(Kind::Binance);
    // The same Rails tick's failed row, five minutes on: a second obligation of one attempt.
    f.c.execute("INSERT INTO transactions (bot_id, exchange_id, status, side, base_asset_id, quote_asset_id, transaction_type, error_messages, bot_interval, \
                 bot_quote_amount, created_at, updated_at) VALUES (?1, ?2, 1, 0, ?3, ?4, 'REGULAR', '[\"Net::ReadTimeout\"]', 'week', 20, '2026-09-29 14:08:00', '2026-09-29 14:08:00')",
                params![f.b, f.s.exchange_id, f.s.btc, f.s.quote]).unwrap();
    ru::collect(&f.c, &queue(), None, now()).unwrap();
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, now()).await.unwrap();
    let mut s = f.c.prepare("SELECT obligation_id, id FROM rust_rails_unrecorded_offers ORDER BY id").unwrap();
    let offers: Vec<(i64, i64)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(offers.len(), 2, "each line offers the order");
    f.request(offers[0].0, offers[0].1).unwrap();
    assert_eq!(f.request(offers[1].0, offers[1].1), Err(Refused::Pending), "one request per attempt");
    f.serve().await;
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_resolutions WHERE resolution = 'booked'"), 2, "the whole group is resolved");
    assert_eq!(f.request(offers[1].0, offers[1].1), Err(Refused::Decided));
    assert_eq!(ru::dismiss(&f.c, f.s.user_id, f.b, offers[1].0, now()).unwrap(), Err(Refused::Decided));
}

#[tokio::test]
async fn none_was_mine_resolves_the_group_with_one_line_and_no_row() {
    let f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    let carry = f.carry("2026-09-30 12:00:00");
    let rows = dump(&f.c, "SELECT * FROM transactions ORDER BY id");
    assert_eq!(ru::dismiss(&f.c, f.s.user_id + 1, f.b, obligation, now()).unwrap(), Err(Refused::NotFound), "another user's bot");
    assert_eq!(ru::dismiss(&f.c, f.s.user_id, f.b, obligation, now()).unwrap(), Ok(()));
    assert_eq!(dump(&f.c, "SELECT * FROM transactions ORDER BY id"), rows);
    assert_eq!(f.carry("2026-09-30 12:00:00"), carry);
    assert_eq!(lines(&f.c, f.b, ru::NOT_MINE), [json!({ "obligation": obligation })]);
    assert_eq!(f.request(obligation, offers[0].0), Err(Refused::Decided));
    assert!(ru::offers_for(&f.c, f.b, &[obligation], now()).unwrap()[0].resolved);
}

// ---- the carry (§4, r1.7) ------------------------------------------------------------------------------------------------

/// PLAN-A28 §5's case: DCA 20/day from day 0 00:00; day 0 bought 20, day 1 Rails' ambiguous tick (20 landed unseen), day 2
/// bought 40; takeover on day 2.
fn carry_fx(failed_row: bool) -> Fx {
    let spec = BotSpec { settings: json!({ "interval": "day", "quote_amount": 20 }), ..BotSpec::weekly(20.0, "2026-09-01 00:00:00") };
    let f = fx_with(Kind::Binance, spec, "BTCEUR", "2026-09-01 00:00:00", "2026-09-02 00:00:00", t("2026-09-03T00:00:01Z"));
    // Day 2's 40.
    prior_row(&f.c, &f.s, f.b, "2026-09-03 00:00:00");
    f.c.execute("UPDATE transactions SET quote_amount = 40, quote_amount_exec = 40, amount = 0.0008, amount_exec = 0.0008 WHERE id = (SELECT max(id) FROM transactions)", []).unwrap();
    if failed_row {
        f.c.execute_batch("DELETE FROM rust_rails_unrecorded; DELETE FROM bot_activity_logs").unwrap();
        f.c.execute("INSERT INTO transactions (bot_id, exchange_id, status, side, base_asset_id, quote_asset_id, transaction_type, error_messages, bot_interval, \
                     bot_quote_amount, created_at, updated_at) VALUES (?1, ?2, 1, 0, ?3, ?4, 'REGULAR', '[\"Net::ReadTimeout\"]', 'week', 15, '2026-09-02 00:00:00', '2026-09-02 00:00:00')",
                    params![f.b, f.s.exchange_id, f.s.btc, f.s.quote]).unwrap();
        ru::collect(&f.c, &queue(), None, t("2026-09-03T00:00:01Z")).unwrap();
    }
    f
}
const DAY1: &str = "2026-09-02T00:00:03Z";

#[tokio::test]
async fn a_booked_order_inside_the_window_shrinks_the_next_orders_floored_at_zero() {
    let f = carry_fx(false);
    // Rails' recorder: readings one second after each tick.
    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rails_unrecorded_vectors.json")).unwrap()).unwrap();
    let carry = vectors["bookings"]["carry"].clone();
    let pinned = |k: &str| carry[format!("{k}_after_tick")].as_str().map(|s| s.parse::<f64>().unwrap());
    assert_eq!(f.carry("2026-09-04 00:00:01").to_f(), pinned("day3_without").unwrap_or(20.0), "without the booking: 20 more on day 3");
    f.answer(&[O { at: DAY1, ..o(7, St::Filled, "0.0004", "20") }]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, t("2026-09-03T00:00:01Z")).await.unwrap();
    let (obligation, offers) = f.offers();
    let view = ru::offers_for(&f.c, f.b, &[obligation], t("2026-09-04T00:00:01Z")).unwrap();
    assert_eq!((view[0].offers[0].carry_before.as_ref().map(BigDec::to_f), view[0].offers[0].carry_after.as_ref().map(BigDec::to_f)), (Some(20.0), Some(0.0)),
               "the preview: what the booking would make it");
    ru::request_booking(&f.c, f.s.user_id, f.b, obligation, offers[0].0, t("2026-09-03T00:00:01Z")).unwrap().unwrap();
    let served = ru::serve_bookings(&f.c, &Factory { venue: f.binance(), hook: RefCell::new(None) }, &seed::cipher(), &|_| false, t("2026-09-03T00:00:01Z")).await.unwrap();
    assert_eq!(served.booked.len(), 1, "{:?}", lines(&f.c, f.b, ru::NOT_ADDED));
    assert_eq!(f.carry("2026-09-04 00:00:01").to_f(), pinned("day3_with").unwrap_or(0.0), "day 3: nothing owed");
    assert_eq!(f.carry("2026-09-05 00:00:01").to_f(), pinned("day4_with").unwrap_or(20.0), "day 4: back to 20");
}

#[tokio::test]
async fn a_booked_failed_row_order_before_the_window_leaves_the_carry_and_keeps_rails_stamp() {
    let f = carry_fx(true);
    // A settings change after Rails' tick moved the window past it.
    f.c.execute("UPDATE bots SET settings_changed_at = '2026-09-02 12:00:00' WHERE id = ?1", [f.b]).unwrap();
    let before = f.carry("2026-09-04 00:00:01");
    f.answer(&[O { at: DAY1, ..o(7, St::Filled, "0.0004", "20") }]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, t("2026-09-03T00:00:01Z")).await.unwrap();
    let (obligation, offers) = f.offers();
    let view = &ru::offers_for(&f.c, f.b, &[obligation], t("2026-09-04T00:00:01Z")).unwrap()[0].offers[0];
    assert_eq!(view.carry_before, view.carry_after, "outside the window: the next order isn't affected");
    ru::request_booking(&f.c, f.s.user_id, f.b, obligation, offers[0].0, t("2026-09-03T00:00:01Z")).unwrap().unwrap();
    let served = ru::serve_bookings(&f.c, &Factory { venue: f.binance(), hook: RefCell::new(None) }, &seed::cipher(), &|_| false, t("2026-09-03T00:00:01Z")).await.unwrap();
    let tx = served.booked[0].1;
    assert_eq!(f.carry("2026-09-04 00:00:01"), before);
    let row = f.row(tx);
    assert_eq!((row["bot_interval"].clone(), row["bot_quote_amount"].clone()), (json!("week"), json!(15)), "D-RAILS-UNRECORDED-FAILED-ROW-STAMP");
}

// ---- the race (§5, r2.4) -------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_booking_waits_for_a_tick_in_hand_and_the_next_tick_sizes_with_it() {
    let f = carry_fx(false);
    f.answer(&[O { at: DAY1, ..o(7, St::Filled, "0.0004", "20") }]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, t("2026-09-03T00:00:01Z")).await.unwrap();
    let (obligation, offers) = f.offers();
    let at = t("2026-09-04T00:00:01Z");
    // A tick that claimed the bot and sized (Executing, its intent persisted) holds it.
    f.c.execute("UPDATE bots SET status = ?2, transient_data = json_set(transient_data, '$.rust_placement', json('{\"cl_ord_id\":\"x\"}')) WHERE id = ?1",
                params![f.b, deltabadger::enums::BotStatus::Executing as i64]).unwrap();
    ru::request_booking(&f.c, f.s.user_id, f.b, obligation, offers[0].0, at).unwrap().unwrap();
    let before = money(&f.c);
    let factory = Factory { venue: f.binance(), hook: RefCell::new(None) };
    assert_eq!(ru::serve_bookings(&f.c, &factory, &seed::cipher(), &|_| false, at).await.unwrap().booked, []);
    assert_eq!(money(&f.c), before, "refused mid-tick, nothing written");
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "busy");
    // The tick dispatched its sized order (the day-3 20) and released the bot; the booking asked again is taken.
    f.c.execute("UPDATE bots SET status = 1, transient_data = json_remove(transient_data, '$.rust_placement') WHERE id = ?1", [f.b]).unwrap();
    prior_row(&f.c, &f.s, f.b, "2026-09-04 00:00:00");
    ru::request_booking(&f.c, f.s.user_id, f.b, obligation, offers[0].0, at).unwrap().unwrap();
    assert_eq!(ru::serve_bookings(&f.c, &factory, &seed::cipher(), &|_| false, at).await.unwrap().booked.len(), 1);
    // The next tick (day 4) sizes with it: 5×20 owed − 120 spent → floored at 0.
    assert_eq!(f.carry("2026-09-05 00:00:01").to_f(), 0.0);
}

// ---- the watch (r3.2), never stale (r4.1), never lower (r5.1) --------------------------------------------------------------

#[tokio::test]
async fn a_booked_row_no_poll_holds_is_enrolled_again_and_after_seven_days_unsettled_gets_one_line() {
    let f = fx(Kind::Binance);
    let tx = f.booked(&[o(7, St::Filled, "0.0004", "20")]).await;
    let factory = Factory { venue: f.binance(), hook: RefCell::new(None) };
    let (yes, no): (&dyn Fn(i64) -> bool, &dyn Fn(i64) -> bool) = (&|_| true, &|_| false);
    let cipher = seed::cipher();
    let pass = |at: DateTime<Utc>, polled: bool| ru::serve_bookings(&f.c, &factory, &cipher, if polled { yes } else { no }, at);
    assert_eq!(pass(now(), true).await.unwrap().watch, []);
    assert_eq!(pass(now(), false).await.unwrap().watch, [(f.b, tx)]);
    assert!(lines(&f.c, f.b, ru::UNSETTLED).is_empty());
    pass(now() + Duration::days(7), true).await.unwrap();
    pass(now() + Duration::days(8), true).await.unwrap();
    assert_eq!(lines(&f.c, f.b, ru::UNSETTLED), [json!({ "transaction_id": tx, "venue": "Binance", "placed": "2026-09-29 14:03:05 UTC" })]);
    assert_eq!(f.row(tx)["external_status"], json!(0), "never abandoned or removed");
    // A deleted bot's row is not watched.
    f.c.execute("UPDATE bots SET status = 3 WHERE id = ?1", [f.b]).unwrap();
    assert_eq!(pass(now(), false).await.unwrap().watch, []);
}

#[tokio::test]
async fn a_failed_settlement_leaves_the_row_waiting_for_the_watch() {
    let f = fx(Kind::Binance);
    let tx = f.booked(&[o(7, St::Filled, "0.0004", "20")]).await;
    // r3.1(b): its trades don't add up to executedQty yet.
    f.tr.reply("GET /api/v3/order", 200, binance_row(&o(7, St::Filled, "0.0004", "20")));
    f.tr.reply("GET /api/v3/myTrades", 200, json!([trade(50, 7, "0.0001", "5", "0", "EUR", PLACED)]));
    assert!(f.follow_up(tx, now()).await.is_err());
    assert_eq!(f.row(tx)["external_status"], json!(0));
    assert_eq!(f.serve().await.watch, [(f.b, tx)]);
}

#[tokio::test]
async fn a_booked_bybit_row_thirty_days_old_and_missing_is_never_abandoned() {
    let mut f = fx(Kind::Bybit);
    let tx = f.booked(&[o(7, St::Filled, "0.0004", "20")]).await;
    f.tr = ScriptedTransport::default();
    let carry = f.carry("2026-10-30 12:00:00");
    f.tr.reply("GET /v5/order/realtime", 200, page(vec![]));
    f.tr.reply("GET /v5/order/history", 200, page(vec![]));
    let bot = model::load_bot(&f.c, f.b).unwrap();
    let swept = polling::sweep(&f.c, &f.bybit(), &bot, t("2026-10-30T12:00:00Z")).await;
    assert_eq!(swept, Ok(()), "a missing booked row neither fails the bot's sweep nor is abandoned");
    assert_eq!(f.row(tx)["external_status"], json!(0), "D-RAILS-UNRECORDED-NEVER-STALE");
    assert_eq!(f.carry("2026-10-30 12:00:00"), carry);
    assert_eq!(f.serve().await.watch, [(f.b, tx)]);
}

#[tokio::test]
async fn an_answer_below_what_the_owner_confirmed_is_held_and_a_consistent_one_applies() {
    let mut f = fx(Kind::Binance);
    let tx = f.booked(&[o(7, St::Filled, "0.0004", "20")]).await;
    let carry = f.carry("2026-09-30 12:00:00");
    let row = f.row(tx);
    // Cancelled 0/0 on the follow-up path, then on the sweep path: not applied.
    f.tr.reply("GET /api/v3/order", 200, binance_row(&o(7, St::Cancelled, "0", "0")));
    assert_eq!(f.follow_up(tx, now()).await, Ok(polling::FollowUp::Done));
    assert_eq!((f.row(tx), f.carry("2026-09-30 12:00:00")), (row.clone(), carry.clone()), "held");
    f.tr = ScriptedTransport::default();
    f.tr.reply("GET /api/v3/order", 200, binance_row(&o(7, St::Cancelled, "0", "0")));
    f.tr.reply("GET /api/v3/allOrders", 200, json!([binance_row(&o(7, St::Cancelled, "0", "0"))]));
    polling::sweep(&f.c, &f.binance(), &model::load_bot(&f.c, f.b).unwrap(), now()).await.unwrap();
    assert_eq!(f.row(tx), row, "the sweep path holds it too");
    // Seven days held: one line.
    f.follow_up(tx, now() + Duration::days(7)).await.unwrap();
    f.follow_up(tx, now() + Duration::days(8)).await.unwrap();
    assert_eq!(lines(&f.c, f.b, ru::SETTLED_LOWER).len(), 1);
    // A consistent answer (0.1 % commission in BTC, within the taker fee) applies and ends the hold.
    f.tr = ScriptedTransport::default();
    f.tr.reply("GET /api/v3/order", 200, binance_row(&o(7, St::Filled, "0.0004", "20")));
    f.tr.reply("GET /api/v3/myTrades", 200, json!([trade(50, 7, "0.0004", "20", "0.0000004", "BTC", PLACED)]));
    assert_eq!(f.follow_up(tx, now() + Duration::days(9)).await, Ok(polling::FollowUp::Done));
    assert_eq!((f.row(tx)["external_status"].clone(), f.row(tx)["amount_exec"].clone()), (json!(2), json!(0.0003996)));
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_notices WHERE kind = 'held'"), 0);
}

#[tokio::test]
async fn an_answer_lower_than_the_fee_allows_on_the_quote_is_held() {
    let f = fx(Kind::Binance);
    let tx = f.booked(&[o(7, St::Filled, "0.0004", "20")]).await;
    // Same base, but 10 EUR of quote: below 20 × (1 − 0.4 %).
    f.tr.reply("GET /api/v3/order", 200, binance_row(&o(7, St::Filled, "0.0004", "10")));
    f.tr.reply("GET /api/v3/myTrades", 200, json!([trade(50, 7, "0.0004", "10", "0", "EUR", PLACED)]));
    f.follow_up(tx, now()).await.unwrap();
    assert_eq!(f.row(tx)["external_status"], json!(0));
}

// ---- the engine pass (run.rs) ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_engine_books_a_request_polls_it_and_announces_it() {
    let f = fx(Kind::Binance);
    f.answer(&[limit(7, St::Open, "0", "0")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    // A stopped bot: only the booking's own poll reads its order.
    f.c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [f.b]).unwrap();
    f.tr.reply("GET /api/v3/order", 200, binance_row(&limit(7, St::Filled, "0.0004", "20")));
    f.tr.reply("GET /api/v3/myTrades", 200, json!([trade(50, 7, "0.0004", "20", "0", "BNB", PLACED)]));
    let lock = deltabadger::lease::lock(&deltabadger::store::Paths::from_env(&|_| None, f._dir.path()), now()).unwrap();
    deltabadger::lease::claim(&lock, &f.c, &seed::cipher(), "test", now()).unwrap();
    let mut e = deltabadger::engine::run::Engine::new(open(&f.path), Factory { venue: f.binance(), hook: RefCell::new(None) }, seed::cipher(), lock);
    let mut events = e.subscribe();
    let clock = deltabadger::engine::FixedClock(now());
    deltabadger::engine::run::step(&mut e, &clock).await.unwrap();
    let tx: i64 = f.c.query_row("SELECT id FROM transactions WHERE external_id = 'BTCEUR-7'", [], |r| r.get(0)).unwrap();
    let mut recorded = false;
    while let Ok(ev) = events.try_recv() { recorded |= ev == deltabadger::engine::events::EngineEvent::OrderRecorded { bot_id: f.b, transaction_id: tx }; }
    assert!(recorded);
    deltabadger::engine::run::step(&mut e, &clock).await.unwrap();
    assert_eq!(f.row(tx)["external_status"], json!(2), "polled to closed by the engine, no page refresh: {:?}", f.tr.requests().iter().map(|r| r.path.clone()).collect::<Vec<_>>());
}

// ---- texts ---------------------------------------------------------------------------------------------------------------

#[test]
fn every_booking_line_renders_in_english_without_a_missing_key() {
    use deltabadger::web::i18n::{self, Arg};
    let args = [("venue", Arg::Text("Binance")), ("placed", Arg::Text("2026-09-29 14:03:05 UTC")), ("order", Arg::Text("bought 0.0004 BTC"))];
    let mut keys: Vec<String> = ["rails_order_recovered", "rails_order_not_mine", "rails_order_unsettled", "rails_order_settled_lower"].map(String::from).to_vec();
    for reason in ["busy", "key_changed", "settings_changed", "held", "linked", "nothing_filled", "no_cost", "ledger", "not_found", "unreachable", "refused",
                   "deleted", "resolved", "unsupported", "unreadable"] { keys.push(format!("rails_order_not_added_{reason}")); }
    for key in keys {
        let text = i18n::text("en", &format!("bot_activity.events.{key}"), &args);
        assert!(!text.contains("%{") && !text.contains("bot_activity") && !text.contains("obligation"), "{key}: {text}");
    }
}

// ---- parity with the Rails recorder (PLAN-A28-2 r2.6) ------------------------------------------------------------------------

/// Book each recorded case from its order, follow it up with its recorded bodies, and compare the row with Rails' own after
/// `persist_accepted_order!` and after `update_with_order_data`, numerically, on every column the recorder lists.
#[tokio::test]
async fn each_booked_row_equals_rails_row_after_persist_and_after_the_poll() {
    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rails_unrecorded_vectors.json")).unwrap()).unwrap();
    let cases = vectors["bookings"]["cases"].as_object().expect("the recorder's bookings key").clone();
    assert_eq!(cases.len(), 5);
    for (name, case) in cases {
        let order = &case["order"];
        let at = DateTime::from_timestamp_millis(order["time"].as_i64().unwrap()).unwrap();
        let sell = order["side"] == "SELL";
        let mut spec = BotSpec { settings: json!({ "interval": "day", "quote_amount": 20 }), ..BotSpec::weekly(20.0, "2026-08-01 00:00:00") };
        if sell { spec = spec.with("direction", json!("selling")); }
        let stamp = deltabadger::codec::format_time(at);
        let f = fx_with(Kind::Binance, spec, "BTCUSDT", "2026-08-20 00:00:00", &stamp, at + Duration::days(1));
        f.c.execute("UPDATE assets SET symbol = 'USDT' WHERE id = ?1", [f.s.quote]).unwrap();
        f.c.execute("UPDATE tickers SET quote = 'USDT' WHERE id = ?1", [f.s.ticker_id]).unwrap();
        f.tr.reply("GET /api/v3/allOrders", 200, json!([order]));
        Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, at + Duration::days(1)).await.unwrap();
        let (obligation, offers) = f.offers();
        ru::request_booking(&f.c, f.s.user_id, f.b, obligation, offers[0].0, at + Duration::days(1)).unwrap().unwrap();
        let served = ru::serve_bookings(&f.c, &Factory { venue: f.binance(), hook: RefCell::new(None) }, &seed::cipher(), &|_| false, at + Duration::days(1)).await.unwrap();
        let [(_, tx)] = served.booked[..] else { panic!("{name}: {:?}", lines(&f.c, f.b, ru::NOT_ADDED)) };
        same(&f.c, tx, &case["persisted"], &format!("{name} persisted"));
        f.tr.reply("GET /api/v3/order", 200, order.clone());
        f.tr.reply("GET /api/v3/myTrades", 200, case["trades"].clone());
        polling::follow_up(&f.c, &f.binance(), f.b, tx, at + Duration::days(1)).await.unwrap();
        same(&f.c, tx, &case["polled"], &format!("{name} polled"));
    }
}

/// The row's columns as Rails names them, numbers compared as numbers, asset ids by their symbol.
fn same(c: &Connection, tx: i64, rails: &Value, what: &str) {
    for (col, want) in rails.as_object().unwrap() {
        let got: rusqlite::types::Value = match col.as_str() {
            "base_asset_id" | "quote_asset_id" => c.query_row(&format!("SELECT a.symbol FROM transactions t JOIN assets a ON a.id = t.{col} WHERE t.id = ?1"), [tx], |r| r.get(0)).unwrap(),
            _ => c.query_row(&format!("SELECT {col} FROM transactions WHERE id = ?1"), [tx], |r| r.get(0)).unwrap(),
        };
        let enum_name = |v: i64| match col.as_str() {
            "status" => ["submitted", "failed", "skipped"][v as usize].to_string(),
            "external_status" => ["unknown", "open", "closed", "cancelled", "abandoned"][v as usize].to_string(),
            "side" => ["buy", "sell"][v as usize].to_string(),
            "order_type" => ["market_order", "limit_order"][v as usize].to_string(),
            _ => v.to_string(),
        };
        let got = match got { rusqlite::types::Value::Null => Value::Null, rusqlite::types::Value::Integer(n) => json!(enum_name(n)), rusqlite::types::Value::Real(x) => json!(x),
                              rusqlite::types::Value::Text(s) => json!(s), other => panic!("{other:?}") };
        let num = |v: &Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()));
        let equal = match (num(&got), num(want)) { (Some(a), Some(b)) => (a - b).abs() < 1e-12, _ => &got == want };
        assert!(equal, "{what}: {col}: Rust {got} vs Rails {want}");
    }
}

/// Codex branch r1: a failed row's stored pair is Rails' at T; one the bot no longer holds is refused at booking.
#[tokio::test]
async fn a_failed_row_offer_for_a_pair_the_bot_no_longer_holds_is_refused() {
    let f = carry_fx(true);
    f.answer(&[O { at: DAY1, ..o(7, St::Filled, "0.0004", "20") }]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, t("2026-09-03T00:00:01Z")).await.unwrap();
    let (obligation, offers) = f.offers();
    ru::request_booking(&f.c, f.s.user_id, f.b, obligation, offers[0].0, t("2026-09-03T00:00:01Z")).unwrap().unwrap();
    f.c.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json('{\"999\": 1.0}')) WHERE id = ?1", [f.b]).unwrap();
    let before = money(&f.c);
    let served = ru::serve_bookings(&f.c, &Factory { venue: f.binance(), hook: RefCell::new(None) }, &seed::cipher(), &|_| false, t("2026-09-03T00:00:01Z")).await.unwrap();
    assert_eq!((served.booked, money(&f.c)), (vec![], before));
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "settings_changed");
}

// ---- the web: the line's buttons and POST /bots/:id/rails_orders (A28-2a, §1, §2, r1.7, r2.1) -----------------------------

/// The web over the fixture's install at `at`, an engine attached (its wake), and the owner signed in.
fn web(f: &Fx, at: &str) -> (deltabadger::web::App, common::web::Browser, std::sync::Arc<tokio::sync::Notify>) {
    use deltabadger::web::{csrf, session};
    let env = |n: &str| (n == "SECRET_KEY_BASE").then(|| "booking-web-secret".to_string());
    let primary = open(&f.path);
    deltabadger::store::configure(&primary).unwrap();
    let app = deltabadger::web::App::new(deltabadger::web::Config::from_env(&env).unwrap(), &env, primary, common::web::TestClock::at(at)).unwrap();
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    f.c.execute("UPDATE users SET confirmed_at = created_at, setup_completed = 1, wash_sale_enabled = 0", []).unwrap();
    let hash: String = f.c.query_row("SELECT encrypted_password FROM users WHERE id = ?1", [f.s.user_id], |r| r.get(0)).unwrap();
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((f.s.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let browser = common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) };
    (app, browser, wake)
}
async fn answer(app: &deltabadger::web::App, b: &mut common::web::Browser, bot: i64, form: &[(&str, &str)], csrf: common::web::Csrf) -> common::web::Answer {
    b.send(app, "POST", &format!("/bots/{bot}/rails_orders"), Some(form), csrf, &[]).await
}
/// The flash the POST left in the session.
fn flash(app: &deltabadger::web::App, b: &common::web::Browser) -> Vec<(String, String)> {
    deltabadger::web::session::open(&app.keys.session, b.cookie.as_deref().unwrap(), app.now()).unwrap().flash
}
/// The bot's feed (the orders frame): its `bot_activity_log_` row for the line.
async fn line_row(app: &deltabadger::web::App, b: &mut common::web::Browser, bot: i64, c: &Connection) -> String {
    let feed = b.send(app, "GET", &format!("/bots/{bot}.turbo_stream"), None, common::web::Csrf::None, &[("Turbo-Frame", "orders_pagination")]).await;
    assert_eq!(feed.status, 200, "{}", feed.body);
    let id: i64 = c.query_row("SELECT id FROM bot_activity_logs WHERE bot_id = ?1 AND event IN ('rails_order_open', 'rails_order_listed') ORDER BY id DESC LIMIT 1", [bot], |r| r.get(0)).unwrap();
    let start = feed.body.find(&format!("<tr id=\"bot_activity_log_{id}\"")).unwrap_or_else(|| panic!("{}", feed.body));
    feed.body[start..].split_once("</tr>").unwrap().0.to_string() + "</tr>"
}
fn waiting(c: &Connection) -> Vec<String> { vec![dump(c, "SELECT * FROM rust_rails_unrecorded_requests"), dump(c, "SELECT id, event, details FROM bot_activity_logs ORDER BY id")] }

#[tokio::test]
async fn the_post_without_csrf_or_on_another_users_bot_writes_nothing() {
    let f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    let (app, mut b, _) = web(&f, "2026-09-30T12:00:00Z");
    let (obligation, offer) = (obligation.to_string(), offers[0].0.to_string());
    let before = (money(&f.c), waiting(&f.c));
    for form in [[("obligation", obligation.as_str()), ("offer", offer.as_str())], [("obligation", obligation.as_str()), ("none", "1")]] {
        let forged = answer(&app, &mut b, f.b, &form, common::web::Csrf::None).await;
        assert_eq!((forged.status, forged.header("location")), (302, Some("/")), "Rails' unverified-request handler");
        assert_eq!((money(&f.c), waiting(&f.c)), before);
    }
    // A forged request signs the session out (Rails' handler): sign in again.
    let (app, mut b, _) = web(&f, "2026-09-30T12:00:00Z");
    f.c.execute("INSERT INTO users (id, email, created_at, updated_at) VALUES (9999, 'foreign@example.com', '2026-01-01', '2026-01-01')", []).unwrap();
    f.c.execute("UPDATE bots SET user_id = 9999 WHERE id = ?1", [f.b]).unwrap();
    for form in [[("obligation", obligation.as_str()), ("offer", offer.as_str())], [("obligation", obligation.as_str()), ("none", "1")]] {
        let foreign = answer(&app, &mut b, f.b, &form, common::web::Csrf::Header).await;
        assert_eq!(foreign.status, 404, "another user's bot: the bot actions' not-found: {:?} {}", foreign.header("location"), foreign.body);
        assert_eq!((money(&f.c), waiting(&f.c)), before);
    }
    let unknown = answer(&app, &mut b, f.b + 100, &[("obligation", obligation.as_str()), ("none", "1")], common::web::Csrf::Header).await;
    assert_eq!(unknown.status, 404);
}

#[tokio::test]
async fn this_was_this_bots_order_leaves_a_request_and_wakes_the_engine() {
    let f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    let (app, mut b, wake) = web(&f, "2026-09-30T12:00:00Z");
    let before = money(&f.c);
    let posted = answer(&app, &mut b, f.b, &[("obligation", &obligation.to_string()), ("offer", &offers[0].0.to_string())], common::web::Csrf::Header).await;
    assert_eq!((posted.status, posted.header("location")), (302, Some(format!("/bots/{}", f.b).as_str())), "{}", posted.body);
    assert_eq!(flash(&app, &b), [("notice".to_string(), "Checking this order with Binance. Reload the page in a moment to see whether it was added.".to_string())]);
    assert_eq!(dump(&f.c, "SELECT obligation_id, offer_id, attempts FROM rust_rails_unrecorded_requests"), format!("Integer({obligation}),Integer({}),Integer(0)", offers[0].0));
    assert_eq!(money(&f.c), before, "the engine books it, not the web");
    tokio::time::timeout(std::time::Duration::from_secs(1), wake.notified()).await.expect("the engine is woken");
    // Pending: refused, nothing written; the line says it is being checked.
    let again = waiting(&f.c);
    for form in [[("obligation", obligation.to_string()), ("offer", offers[0].0.to_string())], [("obligation", obligation.to_string()), ("none", "1".to_string())]] {
        let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(answer(&app, &mut b, f.b, &form, common::web::Csrf::Header).await.status, 302);
        assert_eq!(flash(&app, &b), [("alert".to_string(), Refused::Pending.text().to_string())]);
        assert_eq!((money(&f.c), waiting(&f.c)), (before.clone(), again.clone()));
    }
    let row = line_row(&app, &mut b, f.b, &f.c).await;
    assert!(row.contains("Checking with Binance… Reload the page to see the result.") && !row.contains("<form"), "{row}");
}

#[tokio::test]
async fn none_of_these_resolves_not_mine_and_then_refuses_as_decided() {
    let f = fx(Kind::Binance);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    let (app, mut b, _) = web(&f, "2026-09-30T12:00:00Z");
    let rows = dump(&f.c, "SELECT * FROM transactions ORDER BY id");
    let posted = answer(&app, &mut b, f.b, &[("obligation", &obligation.to_string()), ("none", "1")], common::web::Csrf::Header).await;
    assert_eq!((posted.status, posted.header("location")), (302, Some(format!("/bots/{}", f.b).as_str())));
    assert_eq!(flash(&app, &b), [("notice".to_string(), "Noted: none of these orders was this bot's.".to_string())]);
    let resolution: (i64, String) = f.c.query_row("SELECT obligation_id, resolution FROM rust_rails_unrecorded_resolutions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(resolution, (obligation, "not_mine".to_string()));
    assert_eq!(lines(&f.c, f.b, ru::NOT_MINE), [json!({ "obligation": obligation })]);
    assert_eq!(dump(&f.c, "SELECT * FROM transactions ORDER BY id"), rows);
    let before = (money(&f.c), waiting(&f.c));
    for form in [[("obligation", obligation.to_string()), ("offer", offers[0].0.to_string())], [("obligation", obligation.to_string()), ("none", "1".to_string())]] {
        let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(answer(&app, &mut b, f.b, &form, common::web::Csrf::Header).await.status, 302);
        assert_eq!(flash(&app, &b), [("alert".to_string(), "This was already decided.".to_string())]);
        assert_eq!((money(&f.c), waiting(&f.c)), before);
    }
    // Decided: the line is as it was before A28-2a, its sentence alone.
    let row = line_row(&app, &mut b, f.b, &f.c).await;
    assert!(row.ends_with("</td>\n</tr>") && !row.contains("<form") && !row.contains("Checking"), "{row}");
}

#[tokio::test]
async fn the_line_offers_each_order_with_its_carry_and_a_none_button() {
    let f = carry_fx(false);
    f.answer(&[O { at: DAY1, ..o(7, St::Filled, "0.0004", "20") }]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, t("2026-09-03T00:00:01Z")).await.unwrap();
    let (obligation, offers) = f.offers();
    let (app, mut b, _) = web(&f, "2026-09-04T00:00:01Z");
    let row = line_row(&app, &mut b, f.b, &f.c).await;
    let forms: Vec<&str> = row.split("<form ").skip(1).collect();
    assert_eq!(forms.len(), 2, "one per offer and one none: {row}");
    assert!(forms[0].contains("data-turbo-confirm=\"Add this order to this bot: bought 0.0004 BTC for 20 EUR (filled), placed 2026-09-02 00:00:03 UTC. \
                                This bot now owes 20.00 EUR toward its next order. With this order added, it owes 0.00 EUR.\""), "{row}");
    assert!(forms[0].contains(&format!("action=\"/bots/{}/rails_orders\"", f.b)) && forms[0].contains(">This was this bot&#39;s order</button>"), "{row}");
    assert!(forms[0].contains(&format!("name=\"obligation\" value=\"{obligation}\"")) && forms[0].contains(&format!("name=\"offer\" value=\"{}\"", offers[0].0)), "{row}");
    assert!(forms[1].contains("data-turbo-confirm=\"None of these orders was this bot&#39;s? The bot keeps trading as it does now.\"") &&
            forms[1].contains(">None of these was this bot&#39;s</button>") && forms[1].contains("name=\"none\" value=\"1\""), "{row}");
    for form in &forms { assert_eq!(form.matches("name=\"authenticity_token\"").count(), 1, "{form}"); }
    // The form's own token is accepted, as a browser posts it.
    let field = regex::Regex::new(r#"name="authenticity_token" value="([^"]+)""#).unwrap();
    let mine = field.captures(forms[1]).unwrap()[1].to_string();
    let posted = b.send(&app, "POST", &format!("/bots/{}/rails_orders", f.b), Some(&[("obligation", &obligation.to_string()), ("none", "1"), ("authenticity_token", &mine)]), common::web::Csrf::None, &[]).await;
    assert_eq!(posted.header("location"), Some(format!("/bots/{}", f.b).as_str()));
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_resolutions WHERE resolution = 'not_mine'"), 1);
}

#[tokio::test]
async fn an_offer_that_leaves_the_carry_says_it_goes_into_the_history_only() {
    let f = carry_fx(true);
    // As a_booked_failed_row_order_before_the_window_leaves_the_carry_and_keeps_rails_stamp: outside the window.
    f.c.execute("UPDATE bots SET settings_changed_at = '2026-09-02 12:00:00' WHERE id = ?1", [f.b]).unwrap();
    f.answer(&[O { at: DAY1, ..o(7, St::Filled, "0.0004", "20") }]);
    Report::new(Factory { venue: f.binance(), hook: RefCell::new(None) }).pass(&f.db, t("2026-09-03T00:00:01Z")).await.unwrap();
    let (app, mut b, _) = web(&f, "2026-09-04T00:00:01Z");
    let row = line_row(&app, &mut b, f.b, &f.c).await;
    assert!(!row.contains("owes"), "{row}");
    assert!(row.contains("It goes into the bot&#39;s history only; its next order isn&#39;t affected.\""), "{row}");
}
