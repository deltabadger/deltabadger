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
use deltabadger::venue::{alpaca, binance, bingx, bitget, bitrue, bitvavo, bybit, coinbase, gemini, hyperliquid, mexc, Venue, VenueFactory};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;

fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn now() -> DateTime<Utc> { t("2026-09-30T12:00:00Z") }
const AT: &str = "2026-09-29 14:03:00";
const PLACED: &str = "2026-09-29T14:03:05Z";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Binance, Bybit, Alpaca, Hyperliquid, Mexc, Bitrue, Bingx, Bitget, Bitvavo, Gemini, Coinbase }
impl Kind {
    fn exchange(self) -> (&'static str, &'static str) {
        match self { Kind::Binance => ("Exchanges::Binance", "Binance"), Kind::Bybit => ("Exchanges::Bybit", "Bybit"), Kind::Alpaca => ("Exchanges::Alpaca", "Alpaca"),
                     Kind::Hyperliquid => ("Exchanges::Hyperliquid", "Hyperliquid"), Kind::Mexc => ("Exchanges::Mexc", "MEXC"), Kind::Bitrue => ("Exchanges::Bitrue", "Bitrue"),
                     Kind::Bingx => ("Exchanges::Bingx", "BingX"), Kind::Bitget => ("Exchanges::Bitget", "Bitget"), Kind::Bitvavo => ("Exchanges::Bitvavo", "Bitvavo"),
                     Kind::Gemini => ("Exchanges::Gemini", "Gemini"), Kind::Coinbase => ("Exchanges::Coinbase", "Coinbase") }
    }
    fn external_id(self, id: u64) -> String {
        match self { Kind::Bybit | Kind::Gemini | Kind::Coinbase => id.to_string(), Kind::Alpaca => alpaca_id(id), Kind::Hyperliquid => format!("BTC-{id}"),
                     Kind::Bingx | Kind::Bitvavo => format!("BTC-EUR-{id}"), Kind::Binance | Kind::Mexc | Kind::Bitrue | Kind::Bitget => format!("BTCEUR-{id}") }
    }
    /// Alpaca spells a crypto pair "BASE/QUOTE" and trades it against USD only (eligibility's "Alpaca: only USD");
    /// Hyperliquid names a spot pair by its index.
    fn symbol(self) -> &'static str {
        match self { Kind::Alpaca => "BTC/USD", Kind::Hyperliquid => HL_COIN, Kind::Bingx | Kind::Bitvavo | Kind::Coinbase => "BTC-EUR", Kind::Gemini => "btceur", _ => "BTCEUR" }
    }
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
fn fx(kind: Kind) -> Fx { fx_with(kind, BotSpec::weekly(20.0, "2026-09-01 10:00:00"), kind.symbol(), "2026-09-22 14:03:00", AT, now()) }

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
    if kind == Kind::Alpaca {
        c.execute("UPDATE tickers SET quote = 'USD' WHERE id = ?1", [s.ticker_id]).unwrap();
        c.execute("UPDATE assets SET symbol = 'USD' WHERE id = ?1", [s.quote]).unwrap();
    }
    let b = seed::insert_bot(&c, &s, &spec);
    prior_row(&c, &s, b, prior);
    ambiguous_line(&c, b, at);
    ru::collect(&c, &queue(), None, collected).unwrap();
    synced(&c, s.api_key_id, collected);
    let db = Db::new(open(&path), seed::cipher());
    let tr = ScriptedTransport::default();
    // MEXC's reach is 7 days and Bitget's 90 from the venue's clock (D-HISTORY-DOC-RETENTION).
    if matches!(kind, Kind::Mexc | Kind::Bitget) { tr.set_now(now()); }
    Fx { _dir: dir, path, c, s, b, kind, tr, db }
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
fn alpaca_id(id: u64) -> String { format!("6c88c025-e7a0-0992-70eb-{id:012}") }
/// A recorded Alpaca row (tests/fixtures/order_lists/alpaca, a crypto limit, filled) edited into `o`, as the live-read
/// tests build it; `filled_avg_price` = cum / exec (Alpaca states no quote total). The creation time keeps nine digits.
fn alpaca_row(o: &O) -> Value {
    let rows: Vec<Value> = serde_json::from_str(include_str!("fixtures/order_lists/alpaca/orders.json")).unwrap();
    let mut r = rows.into_iter().find(|r| r["asset_class"] == "crypto" && r["status"] == "filled").unwrap();
    let status = match o.state { St::Open => "new", St::Partly => "partially_filled", St::Filled => "filled", St::PartlyCancelled | St::Cancelled => "canceled" };
    let avg = if BigDec::parse(o.exec).unwrap().is_positive() { json!(BigDec::parse(o.cum).unwrap().div(&BigDec::parse(o.exec).unwrap()).unwrap().to_s_f()) } else { Value::Null };
    let at = o.at.replace('Z', ".123456789Z");
    for (k, v) in [("id", json!(alpaca_id(o.id))), ("client_order_id", json!(format!("c-{}", o.id))), ("symbol", json!("BTC/USD")), ("side", json!(if o.sell { "sell" } else { "buy" })),
                   ("type", json!(if o.limit.is_some() { "limit" } else { "market" })), ("order_type", json!(if o.limit.is_some() { "limit" } else { "market" })),
                   ("qty", if o.limit.is_some() { json!(o.size) } else { Value::Null }), ("notional", if o.limit.is_some() { Value::Null } else { json!(o.size) }),
                   ("limit_price", o.limit.map_or(Value::Null, |p| json!(p))), ("status", json!(status)), ("filled_qty", json!(o.exec)), ("filled_avg_price", avg),
                   ("created_at", json!(at)), ("submitted_at", json!(at)), ("updated_at", json!(at)), ("filled_at", Value::Null), ("canceled_at", Value::Null)] {
        r[k] = v;
    }
    r
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
    fn alpaca(&self) -> alpaca::AlpacaVenue<ScriptedTransport> { alpaca::AlpacaVenue::new(self.tr.clone(), alpaca::Urls::for_passphrase(Some("paper"))) }
    fn hl(&self) -> hyperliquid::HyperliquidVenue<ScriptedTransport> {
        let pairs = binance::Pairs { by_symbol: [(HL_COIN.to_string(), binance::Pair { base: "BTC".into(), quote: "EUR".into(), price_decimals: 2 })].into(), assets: Default::default() };
        hyperliquid::HyperliquidVenue::new(self.tr.clone(), hyperliquid::URL, "0x00000000000000000000000000000000000000a1").with_pairs(pairs)
    }
    fn mexc(&self) -> mexc::MexcVenue<ScriptedTransport> { mexc::MexcVenue::new(self.tr.clone(), mexc::URL, "secret") }
    fn bitrue(&self) -> bitrue::BitrueVenue<ScriptedTransport> { bitrue::BitrueVenue::new(self.tr.clone(), bitrue::URL, "secret") }
    fn bingx(&self) -> bingx::BingxVenue<ScriptedTransport> { bingx::BingxVenue::new(self.tr.clone(), bingx::URL, "secret") }
    fn bitget(&self) -> bitget::BitgetVenue<ScriptedTransport> { bitget::BitgetVenue::new(self.tr.clone(), bitget::URL) }
    fn bitvavo(&self) -> bitvavo::BitvavoVenue<ScriptedTransport> { bitvavo::BitvavoVenue::new(self.tr.clone(), bitvavo::URL) }
    fn gemini(&self) -> gemini::GeminiVenue<ScriptedTransport> { gemini::GeminiVenue::new(self.tr.clone(), gemini::URL) }
    fn coinbase(&self) -> coinbase::CoinbaseVenue<ScriptedTransport> { coinbase::CoinbaseVenue::new(self.tr.clone(), coinbase::URL) }
    /// The venue lists these orders (both the report's read and the booking's identity read: the last reply repeats).
    fn answer(&self, orders: &[O]) {
        match self.kind {
            Kind::Bybit => {
                let (open, closed): (Vec<&O>, Vec<&O>) = orders.iter().partition(|o| matches!(o.state, St::Open | St::Partly));
                self.tr.reply("GET /v5/order/realtime", 200, page(open.iter().map(|o| bybit_row(o)).collect()));
                self.tr.reply("GET /v5/order/history", 200, page(closed.iter().map(|o| bybit_row(o)).collect()));
            }
            Kind::Binance => { self.tr.reply("GET /api/v3/allOrders", 200, Value::Array(orders.iter().map(binance_row).collect())); }
            Kind::Alpaca => { self.tr.reply("GET /v2/orders", 200, Value::Array(orders.iter().map(alpaca_row).collect())); }
            Kind::Hyperliquid => {
                self.tr.reply(HL_ORDERS, 200, Value::Array(orders.iter().map(hl_row).collect()));
                self.tr.reply(HL_FILLS, 200, Value::Array(orders.iter().filter(|o| o.exec != "0").map(|o| hl_fill(o.id, o.exec, &dec(o.cum).div(&dec(o.exec)).unwrap().to_s_f(), o.id, ms(o.at) + 500)).collect()));
            }
            Kind::Mexc | Kind::Bitrue => {
                self.tr.reply(doc_list(self.kind), 200, Value::Array(orders.iter().map(|o| doc_row(self.kind, o)).collect()));
                self.lookup(orders);
            }
            Kind::Bingx => {
                let (open, closed): (Vec<&O>, Vec<&O>) = orders.iter().partition(|o| matches!(o.state, St::Open | St::Partly));
                self.tr.reply(BINGX_OPEN, 200, bingx_page(open.iter().map(|o| doc_row(Kind::Bingx, o)).collect()));
                self.tr.reply(BINGX_HISTORY, 200, bingx_page(closed.iter().map(|o| doc_row(Kind::Bingx, o)).collect()));
                self.lookup(orders);
            }
            Kind::Bitget | Kind::Bitvavo | Kind::Gemini => {
                doc2_list(self, orders);
                if let Some(first) = orders.first() { self.lookup_row(row2(self.kind, first)); }
            }
            Kind::Coinbase => {
                self.tr.reply(CB_ORDERS, 200, cb_page(orders.iter().map(cb_row).collect()));
                if let Some(first) = orders.first() { self.lookup_row(cb_row(first)); }
            }
        }
    }
    /// A28-3d-1: the venue's single-order lookup answers with the first order (as `doc_row` writes it).
    fn lookup(&self, orders: &[O]) {
        let Some(first) = orders.first() else { return };
        self.lookup_row(doc_row(self.kind, first));
    }
    fn lookup_row(&self, row: Value) {
        match self.kind {
            Kind::Bingx => { self.tr.reply(BINGX_QUERY, 200, json!({ "code": 0, "msg": "", "data": row })); }
            Kind::Bitget => { self.tr.reply(BITGET_ORDER_INFO, 200, bitget_page(vec![row])); }
            Kind::Bitvavo => { self.tr.reply(BITVAVO_ORDER, 200, row); }
            Kind::Gemini => { self.tr.reply(GEMINI_STATUS, 200, row); }
            Kind::Coinbase => { self.tr.reply(&cb_lookup(row["order_id"].as_str().unwrap_or("7")), 200, json!({ "order": row })); }
            kind => { self.tr.reply(doc_lookup(kind), 200, row); }
        }
    }
    /// The report's pass: the line and its offers.
    async fn report(&self) {
        let n = match self.kind {
            Kind::Binance => Report::new(Factory { venue: self.binance(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Bybit => Report::new(Factory { venue: self.bybit(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Alpaca => Report::new(Factory { venue: self.alpaca(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Hyperliquid => Report::new(Factory { venue: self.hl(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Mexc => Report::new(Factory { venue: self.mexc(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Bitrue => Report::new(Factory { venue: self.bitrue(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Bingx => Report::new(Factory { venue: self.bingx(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Bitget => Report::new(Factory { venue: self.bitget(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Bitvavo => Report::new(Factory { venue: self.bitvavo(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Gemini => Report::new(Factory { venue: self.gemini(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
            Kind::Coinbase => Report::new(Factory { venue: self.coinbase(), hook: RefCell::new(None) }).pass(&self.db, now()).await,
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
            Kind::Alpaca => ru::serve_bookings(&self.c, &Factory { venue: self.alpaca(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Hyperliquid => ru::serve_bookings(&self.c, &Factory { venue: self.hl(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Mexc => ru::serve_bookings(&self.c, &Factory { venue: self.mexc(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Bitrue => ru::serve_bookings(&self.c, &Factory { venue: self.bitrue(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Bingx => ru::serve_bookings(&self.c, &Factory { venue: self.bingx(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Bitget => ru::serve_bookings(&self.c, &Factory { venue: self.bitget(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Bitvavo => ru::serve_bookings(&self.c, &Factory { venue: self.bitvavo(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Gemini => ru::serve_bookings(&self.c, &Factory { venue: self.gemini(), hook }, &seed::cipher(), &|_| false, at).await,
            Kind::Coinbase => ru::serve_bookings(&self.c, &Factory { venue: self.coinbase(), hook }, &seed::cipher(), &|_| false, at).await,
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
            Kind::Alpaca => polling::follow_up(&self.c, &self.alpaca(), self.b, tx, at).await,
            Kind::Hyperliquid => polling::follow_up(&self.c, &self.hl(), self.b, tx, at).await,
            Kind::Mexc => polling::follow_up(&self.c, &self.mexc(), self.b, tx, at).await,
            Kind::Bitrue => polling::follow_up(&self.c, &self.bitrue(), self.b, tx, at).await,
            Kind::Bingx => polling::follow_up(&self.c, &self.bingx(), self.b, tx, at).await,
            Kind::Bitget => polling::follow_up(&self.c, &self.bitget(), self.b, tx, at).await,
            Kind::Bitvavo => polling::follow_up(&self.c, &self.bitvavo(), self.b, tx, at).await,
            Kind::Gemini => polling::follow_up(&self.c, &self.gemini(), self.b, tx, at).await,
            Kind::Coinbase => polling::follow_up(&self.c, &self.coinbase(), self.b, tx, at).await,
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
            _ => unreachable!(),
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
                   "deleted", "resolved", "unsupported", "unreadable", "unconfirmed"] { keys.push(format!("rails_order_not_added_{reason}")); }
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

// ---- MCP and REST: answer_unrecorded_order and POST /api/v1/bots/:id/unrecorded_orders (A28-2b, D-RAILS-UNRECORDED-API) -

/// The web's App over the fixture, with an OAuth client holding an `mcp` and an `api` token that follow the owner's tools
/// (a NULL grant), and the owner's `tools` on for both surfaces.
struct Api { app: deltabadger::web::App, wake: std::sync::Arc<tokio::sync::Notify> }
fn api(f: &Fx, tools: &[&str]) -> Api {
    let (app, _, wake) = web(f, "2026-09-30T12:00:00Z");
    f.c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('A','a','http://localhost/cb',0,'mcp api','2026-09-30 11:00:00','2026-09-30 11:00:00')", []).unwrap();
    let application = f.c.last_insert_rowid();
    for (token, scope) in [("a-mcp", "mcp"), ("a-api", "api")] {
        f.c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-30 11:00:00',86400)",
                    params![application, f.s.user_id, token, scope]).unwrap();
    }
    f.c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-30 11:00:00','2026-09-30 11:00:00')",
                params![f.s.user_id, application]).unwrap();
    permit(f, tools);
    Api { app, wake }
}
fn permit(f: &Fx, tools: &[&str]) {
    let permissions = json!({ "tool_permissions": tools.iter().map(|t| (t.to_string(), json!(true))).collect::<serde_json::Map<_, _>>() }).to_string();
    f.c.execute("UPDATE users SET mcp_settings = ?1, rest_settings = ?1", [&permissions]).unwrap();
}
const BOTH: [&str; 2] = ["answer_unrecorded_order", "get_bot_details"];
impl Api {
    async fn send(&self, request: axum::http::Request<axum::body::Body>) -> (u16, String) {
        use tower::ServiceExt;
        let response = deltabadger::web::router(self.app.clone()).oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        (status, String::from_utf8(axum::body::to_bytes(response.into_body(), 8_000_000).await.unwrap().to_vec()).unwrap())
    }
    /// REST with the `api` token: the status and the envelope.
    async fn rest(&self, method: &str, path: &str, body: Option<(&str, String)>) -> (u16, Value) {
        let mut r = axum::http::Request::builder().method(method).uri(path).header("host", "localhost:3000").header("authorization", "Bearer a-api").header("accept", "application/json");
        if let Some((kind, _)) = &body { r = r.header("content-type", *kind); }
        let (status, text) = self.send(r.body(axum::body::Body::from(body.map(|(_, b)| b).unwrap_or_default())).unwrap()).await;
        (status, serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text })))
    }
    async fn post_json(&self, bot: i64, body: Value) -> (u16, Value) { self.rest("POST", &format!("/api/v1/bots/{bot}/unrecorded_orders"), Some(("application/json", body.to_string()))).await }
    async fn post_form(&self, path: &str, form: &str) -> (u16, Value) { self.rest("POST", path, Some(("application/x-www-form-urlencoded", form.to_string()))).await }
    /// One JSON-RPC call on a fresh MCP session: the answer.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let request = |body: Value, session: Option<&str>| {
            let mut r = axum::http::Request::builder().method("POST").uri("/mcp").header("host", "localhost:3000").header("authorization", "Bearer a-mcp")
                .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
            if let Some(session) = session { r = r.header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25"); }
            r.body(axum::body::Body::from(body.to_string())).unwrap()
        };
        use tower::ServiceExt;
        let response = deltabadger::web::router(self.app.clone()).oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"A","version":"1"}}}), None)).await.unwrap();
        let session = response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
        assert_eq!(self.send(request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Some(&session))).await.0, 202);
        let (_, body) = self.send(request(json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}), Some(&session))).await;
        serde_json::from_str(&body).unwrap()
    }
    /// A tools/call: its text and whether it is an error.
    async fn mcp(&self, name: &str, arguments: Value) -> (String, bool) {
        let answer = self.rpc("tools/call", json!({ "name": name, "arguments": arguments })).await;
        let result = &answer["result"];
        (result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{answer}")).to_string(), result["isError"] == true)
    }
    async fn listed(&self, bot: i64) -> Value {
        let (status, body) = self.rest("GET", &format!("/api/v1/bots/{bot}"), None).await;
        assert_eq!(status, 200, "{body}");
        body["data"]["unrecorded_orders"].clone()
    }
}
/// The fixture's line, reported with one filled order on offer.
async fn offered(kind: Kind) -> (Fx, i64, i64) {
    let f = fx(kind);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    (f, obligation, offers[0].0)
}
async fn woken(a: &Api) { tokio::time::timeout(std::time::Duration::from_secs(1), a.wake.notified()).await.expect("the engine is woken"); }

#[tokio::test]
async fn mcp_and_rest_book_exactly_what_the_web_books() {
    // The web.
    let (web_fx, obligation, offer) = offered(Kind::Binance).await;
    let (app, mut b, _) = web(&web_fx, "2026-09-30T12:00:00Z");
    assert_eq!(answer(&app, &mut b, web_fx.b, &[("obligation", &obligation.to_string()), ("offer", &offer.to_string())], common::web::Csrf::Header).await.status, 302);
    let tx = web_fx.serve().await.booked[0].1;
    let booked = |f: &Fx| (money(&f.c), lines(&f.c, f.b, ru::RECOVERED), f.carry("2026-09-30 12:00:00"));
    let by_web = booked(&web_fx);
    // MCP.
    let (f, obligation2, offer2) = offered(Kind::Binance).await;
    assert_eq!((obligation2, offer2), (obligation, offer));
    let a = api(&f, &BOTH);
    let (text, error) = a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "offer": offer })).await;
    assert_eq!((text.as_str(), error), ("Checking this order with Binance. Reload the page in a moment to see whether it was added. get_bot_details shows the result under unrecorded orders.", false));
    woken(&a).await;
    assert_eq!(f.serve().await.booked, [(f.b, tx)]);
    assert_eq!(booked(&f), by_web, "the same row, resolution, line and carry");
    // REST.
    let (f, _, _) = offered(Kind::Binance).await;
    let a = api(&f, &BOTH);
    let (status, body) = a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await;
    assert_eq!((status, &body), (202, &json!({ "data": { "bot_id": f.b, "obligation": obligation, "offer": offer, "state": "checking" }, "error": null })));
    woken(&a).await;
    assert_eq!(f.serve().await.booked, [(f.b, tx)]);
    assert_eq!(booked(&f), by_web);
    // The read: booked, its row, no projection.
    let listed = a.listed(f.b).await;
    assert_eq!(listed["obligations"][0]["state"], "booked");
    assert_eq!(listed["obligations"][0]["transaction_id"], tx);
    assert_eq!(listed["obligations"][0]["offers"][0]["carry_after"], Value::Null);
    assert_eq!(listed["carry"], json!(f.carry("2026-09-30 12:00:00").to_s_f()));
    let (text, _) = a.mcp("get_bot_details", json!({ "bot_id": f.b })).await;
    assert!(text.contains(&format!("- obligation {obligation}: added to the bot's history as transaction {tx}")), "{text}");
}

#[tokio::test]
async fn none_was_mine_through_mcp_and_a_rest_form() {
    let (f, obligation, _) = offered(Kind::Binance).await;
    let a = api(&f, &BOTH);
    let rows = dump(&f.c, "SELECT * FROM transactions ORDER BY id");
    assert_eq!(a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "none": true })).await, ("Noted: none of these orders was this bot's.".to_string(), false));
    assert_eq!(lines(&f.c, f.b, ru::NOT_MINE), [json!({ "obligation": obligation })]);
    assert_eq!(dump(&f.c, "SELECT * FROM transactions ORDER BY id"), rows);
    assert_eq!(a.listed(f.b).await["obligations"][0]["state"], "not_mine");
    let (f, obligation, _) = offered(Kind::Binance).await;
    let a = api(&f, &BOTH);
    let (status, body) = a.post_form(&format!("/api/v1/bots/{}/unrecorded_orders", f.b), &format!("obligation={obligation}&none=1")).await;
    assert_eq!((status, body), (200, json!({ "data": { "bot_id": f.b, "obligation": obligation, "state": "not_mine" }, "error": null })));
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_resolutions WHERE resolution = 'not_mine'"), 1);
}

#[tokio::test]
async fn a_repeat_says_checking_then_decided_and_books_once() {
    let (f, obligation, offer) = offered(Kind::Binance).await;
    let a = api(&f, &BOTH);
    let form = format!("obligation={obligation}&offer={offer}");
    assert_eq!(a.post_form(&format!("/api/v1/bots/{}/unrecorded_orders", f.b), &form).await.0, 202, "a form body's digits");
    let before = (money(&f.c), waiting(&f.c));
    let (status, body) = a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await;
    assert_eq!((status, &body["error"]), (409, &json!({ "code": "already_checking", "message": Refused::Pending.text(), "details": { "state": "checking" } })));
    assert_eq!(a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "none": true })).await, (Refused::Pending.text().to_string(), true));
    assert_eq!((money(&f.c), waiting(&f.c)), before);
    let listed = a.listed(f.b).await;
    assert_eq!((listed["obligations"][0]["state"].clone(), listed["obligations"][0]["offers"][0]["carry_after"].clone()), (json!("checking"), Value::Null));
    let tx = f.serve().await.booked[0].1;
    let before = (money(&f.c), waiting(&f.c));
    let (status, body) = a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await;
    assert_eq!((status, &body["error"]["code"], &body["error"]["details"]), (409, &json!("already_decided"), &json!({ "state": "booked", "transaction_id": tx })));
    let (text, error) = a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "offer": offer })).await;
    assert_eq!((text, error), (format!("This was already decided. Added to the bot's history as transaction {tx}."), true));
    assert_eq!((money(&f.c), waiting(&f.c)), before);
    assert_eq!(count(&f.c, "SELECT count(*) FROM transactions WHERE external_id = 'BTCEUR-7'"), 1);
}

#[tokio::test]
async fn refusals_and_malformed_answers_write_nothing() {
    let (f, obligation, offer) = offered(Kind::Binance).await;
    let a = api(&f, &BOTH);
    let path = format!("/api/v1/bots/{}/unrecorded_orders", f.b);
    let before = (money(&f.c), waiting(&f.c));
    let unchanged = |why: &str| assert_eq!((money(&f.c), waiting(&f.c)), before, "{why}");
    // Request-time refusals.
    for (body, code) in [(json!({ "obligation": obligation + 1000, "offer": offer }), "order_not_listed"), (json!({ "obligation": obligation, "offer": offer + 1000 }), "order_not_listed"),
                         (json!({ "obligation": obligation + 1000, "none": true }), "order_not_listed")] {
        let (status, answer) = a.post_json(f.b, body.clone()).await;
        assert_eq!((status, answer["error"]["code"].clone()), (404, json!(code)), "{body}");
        unchanged(&body.to_string());
    }
    let (text, error) = a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "offer": offer + 1000 })).await;
    assert_eq!((text.as_str(), error), (Refused::NotFound.text(), true));
    // Malformed: both, neither, none false or null, offer null, fractions, overflow, strings in JSON.
    let malformed = [json!({ "obligation": obligation, "offer": offer, "none": true }), json!({ "obligation": obligation }), json!({ "obligation": obligation, "none": false }),
                     json!({ "obligation": obligation, "none": null }), json!({ "obligation": obligation, "offer": null }), json!({ "obligation": obligation, "offer": 1.5 }),
                     json!({ "obligation": obligation as f64 + 0.5, "none": true }), json!({ "obligation": obligation, "offer": 9_223_372_036_854_775_808u64 }),
                     json!({ "obligation": obligation.to_string(), "none": true }), json!({ "obligation": obligation, "offer": offer.to_string() }), json!({ "offer": offer })];
    for body in &malformed {
        let (status, answer) = a.post_json(f.b, body.clone()).await;
        assert_eq!((status, answer["error"]["code"].clone()), (422, json!("invalid_answer")), "REST {body}");
        let mut args = body.clone();
        args["bot_id"] = json!(f.b);
        let (text, error) = a.mcp("answer_unrecorded_order", args).await;
        assert!(error && text.starts_with("Invalid input"), "MCP {body}: {text}");
        unchanged(&body.to_string());
    }
    for args in [json!({ "bot_id": f.b as f64 + 0.5, "obligation": obligation, "none": true }), json!({ "bot_id": f.b.to_string(), "obligation": obligation, "none": true })] {
        let (text, error) = a.mcp("answer_unrecorded_order", args.clone()).await;
        assert!(error && text.starts_with("Invalid input"), "MCP {args}: {text}");
    }
    for form in [format!("obligation={obligation}&offer=1.5"), format!("obligation={obligation}&none=0"), format!("obligation=-{obligation}&none=1"), format!("obligation={obligation}&offer={offer}&none=1")] {
        assert_eq!(a.post_form(&path, &form).await.0, 422, "{form}");
        unchanged(&form);
    }
    // The query is never read: an offer there is no answer.
    let (status, _) = a.rest("POST", &format!("{path}?obligation={obligation}&offer={offer}"), Some(("application/json", "{}".into()))).await;
    assert_eq!(status, 422);
    // Path ids: only digits name a bot.
    for bad in [format!("{}abc", f.b), "1.5".into(), "9223372036854775808".into()] {
        let (status, answer) = a.rest("POST", &format!("/api/v1/bots/{bad}/unrecorded_orders"), Some(("application/json", json!({ "obligation": obligation, "none": true }).to_string()))).await;
        assert!(matches!(status, 404 | 501), "{bad}: {status} {answer}");
        unchanged(&bad);
    }
    // Another user's bot, a deleted bot.
    f.c.execute("INSERT INTO users (id, email, created_at, updated_at) VALUES (9999, 'foreign@example.com', '2026-01-01', '2026-01-01')", []).unwrap();
    for sql in ["UPDATE bots SET user_id = 9999 WHERE id = ?1", "UPDATE bots SET user_id = (SELECT min(id) FROM users WHERE id <> 9999), status = 3 WHERE id = ?1"] {
        f.c.execute(sql, [f.b]).unwrap();
        let before = (money(&f.c), waiting(&f.c));
        let (status, answer) = a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await;
        assert_eq!((status, answer["error"]["code"].clone()), (404, json!("bot_not_found")), "{sql}");
        assert_eq!(a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "none": true })).await, ("Bot not found.".to_string(), true));
        assert_eq!((money(&f.c), waiting(&f.c)), before, "{sql}");
    }
}

#[tokio::test]
async fn the_tool_is_off_until_granted_and_paper_trading_records_nothing() {
    let (f, obligation, offer) = offered(Kind::Binance).await;
    let a = api(&f, &["get_bot_details"]);
    let before = (money(&f.c), waiting(&f.c));
    let (status, body) = a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await;
    assert_eq!((status, body["error"]["code"].clone()), (403, json!("tool_disabled")));
    let listed = a.rpc("tools/list", json!({})).await;
    assert!(!listed.to_string().contains("answer_unrecorded_order"), "{listed}");
    let called = a.rpc("tools/call", json!({ "name": "answer_unrecorded_order", "arguments": { "bot_id": f.b, "obligation": obligation, "none": true } })).await;
    assert_eq!(called["error"]["code"], -32602);
    assert_eq!((money(&f.c), waiting(&f.c)), before);
    // On, but not granted to this client.
    permit(&f, &BOTH);
    f.c.execute("UPDATE connected_clients SET mcp_tools = '[\"get_bot_details\"]', rest_tools = '[\"get_bot_details\"]'", []).unwrap();
    assert_eq!(a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await.0, 403);
    assert!(!a.rpc("tools/list", json!({})).await.to_string().contains("answer_unrecorded_order"));
    f.c.execute("UPDATE connected_clients SET mcp_tools = NULL, rest_tools = NULL", []).unwrap();
    // Paper trading: MCP records nothing; REST has none.
    f.c.execute("UPDATE users SET mcp_settings = json_set(mcp_settings, '$.dry_run', json('true'))", []).unwrap();
    let (text, error) = a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": obligation, "offer": offer })).await;
    assert!(error && text.starts_with("[DRY RUN] Paper trading is on, so nothing was recorded"), "{text}");
    assert_eq!((money(&f.c), waiting(&f.c)), before);
    assert_eq!(a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await.0, 202, "REST is never a dry run");
}

#[tokio::test]
async fn the_read_shows_the_offers_and_the_engines_refusal() {
    let (f, obligation, offer) = offered(Kind::Binance).await;
    let a = api(&f, &BOTH);
    let listed = a.listed(f.b).await;
    let view = &ru::offers_for(&f.c, f.b, &[obligation], now()).unwrap()[0].offers[0];
    let (from, to): (String, String) = f.c.query_row("SELECT span_from, span_to FROM rust_rails_unrecorded WHERE id = ?1", [obligation], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(listed, json!({ "carry": view.carry_before.as_ref().map(BigDec::to_s_f), "more": false, "obligations": [{ "obligation": obligation, "state": "open", "from": from, "to": to,
        "offers": [{ "offer": offer, "order": view.order, "carry_after": view.carry_after.as_ref().map(BigDec::to_s_f) }], "transaction_id": null, "refused": null }] }));
    let (text, _) = a.mcp("get_bot_details", json!({ "bot_id": f.b })).await;
    assert!(text.contains("Unrecorded orders from before the update (answer with answer_unrecorded_order); carry now")
            && text.contains(&format!("- obligation {obligation} ({from} to {to}): offer {offer} = {}", view.order)) && text.contains("; or none: true"), "{text}");
    // The engine refuses (mid-tick): open again, with the web's line.
    assert_eq!(a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await.0, 202);
    f.c.execute("UPDATE bots SET status = ?2 WHERE id = ?1", params![f.b, deltabadger::enums::BotStatus::Executing as i64]).unwrap();
    f.serve().await;
    f.c.execute("UPDATE bots SET status = 1 WHERE id = ?1", [f.b]).unwrap();
    let refused = &a.listed(f.b).await["obligations"][0];
    let message = "Couldn't add the order from before the update to this bot's history. This bot was placing an order at that moment. Try again in a minute.";
    assert_eq!((refused["state"].clone(), refused["refused"]["reason"].clone(), refused["refused"]["message"].clone()), (json!("open"), json!("busy"), json!(message)));
    assert!(a.mcp("get_bot_details", json!({ "bot_id": f.b })).await.0.contains(&format!("  last try refused: {message}")));
    // The line is pruned before the next pass: still listed, then booked.
    assert_eq!(a.post_json(f.b, json!({ "obligation": obligation, "offer": offer })).await.0, 202);
    f.c.execute("DELETE FROM bot_activity_logs WHERE event IN ('rails_order_open', 'rails_order_listed')", []).unwrap();
    assert_eq!(a.listed(f.b).await["obligations"][0]["state"], "checking");
    let tx = f.serve().await.booked[0].1;
    assert_eq!(a.listed(f.b).await["obligations"][0]["transaction_id"], tx);
}

/// A reported obligation of the bot over [from, to], with `offers` offers (unreadable facts: listed without offers).
fn obligation(f: &Fx, from: &str, to: &str, offers: usize) -> i64 {
    f.c.execute("INSERT INTO rust_rails_unrecorded (bot_id, source, source_ref, span_from, span_to, collected_at, reported_at) VALUES (?1, 'failed_row', \
                 (SELECT coalesce(max(source_ref), 0) + 1000 FROM rust_rails_unrecorded), ?2, ?3, ?4, ?4)", params![f.b, from, to, "2026-09-30 11:00:00"]).unwrap();
    let id = f.c.last_insert_rowid();
    for n in 0..offers { f.c.execute("INSERT INTO rust_rails_unrecorded_offers (obligation_id, external_id, symbol, facts) VALUES (?1, ?2, 'BTCEUR', '{}')", params![id, format!("X-{id}-{n}")]).unwrap(); }
    id
}
fn day(n: usize) -> (String, String) { (format!("2026-08-{:02} 10:00:00", n + 1), format!("2026-08-{:02} 10:01:00", n + 1)) }

#[tokio::test]
async fn open_obligations_come_first_and_a_repeat_reaches_one_past_the_listing() {
    let f = fx(Kind::Binance);
    let a = api(&f, &BOTH);
    let ids: Vec<i64> = (0..21).map(|n| { let (from, to) = day(n); obligation(&f, &from, &to, 1) }).collect();
    let listed = a.listed(f.b).await;
    assert_eq!((listed["obligations"].as_array().unwrap().len(), listed["more"].clone()), (20, json!(true)));
    assert_eq!(listed["obligations"][0]["obligation"], ids[0], "oldest first");
    assert!(a.mcp("get_bot_details", json!({ "bot_id": f.b })).await.0.contains("- and 1 more"));
    assert!(!a.mcp("answer_unrecorded_order", json!({ "bot_id": f.b, "obligation": ids[0], "none": true })).await.1);
    let listed = a.listed(f.b).await;
    let shown: Vec<i64> = listed["obligations"].as_array().unwrap().iter().map(|o| o["obligation"].as_i64().unwrap()).collect();
    assert_eq!(shown, ids[1..21], "the 21st comes into view; the decided one is behind the open ones");
    // The decided one is still asked about through the tool.
    let (status, body) = a.post_json(f.b, json!({ "obligation": ids[0], "none": true })).await;
    assert_eq!((status, body["error"]["details"].clone()), (409, json!({ "state": "not_mine", "transaction_id": null })));
    // An old open one behind 20 decided ones: listed first.
    for id in &ids[1..20] { a.post_json(f.b, json!({ "obligation": id, "none": true })).await; }
    let listed = a.listed(f.b).await;
    assert_eq!((listed["obligations"][0]["obligation"].clone(), listed["obligations"][0]["state"].clone()), (json!(ids[20]), json!("open")));
}

#[tokio::test]
async fn a_group_is_read_from_every_member() {
    let f = fx(Kind::Binance);
    let a = api(&f, &BOTH);
    let low = obligation(&f, "2026-08-01 10:00:00", "2026-08-01 10:01:00", 1);
    let high = obligation(&f, "2026-08-01 10:05:00", "2026-08-01 10:06:00", 2);
    let lone = obligation(&f, "2026-08-01 10:30:00", "2026-08-01 10:31:00", 0);
    model::log_activity(&f.c, f.b, ru::NOT_ADDED, Level::Warning, json!({ "reason": "unreachable", "venue": "Binance", "placed": null, "obligation": high }), now()).unwrap();
    let listed = a.listed(f.b).await;
    let shown: Vec<(i64, usize, Value)> = listed["obligations"].as_array().unwrap().iter().map(|o| (o["obligation"].as_i64().unwrap(), o["offers"].as_array().unwrap().len(), o["refused"]["reason"].clone())).collect();
    assert_eq!(shown, [(low, 0, json!("unreachable")), (high, 0, json!("unreachable"))], "each member listed with its own offers (unreadable here), the group's refusal on both; no offer, not listed ({lone})");
    assert_eq!(f.c.query_row("SELECT count(*) FROM rust_rails_unrecorded_offers WHERE obligation_id = ?1", [high], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(a.post_json(f.b, json!({ "obligation": high, "none": true })).await.0, 200);
    let states: Vec<Value> = a.listed(f.b).await["obligations"].as_array().unwrap().iter().map(|o| o["state"].clone()).collect();
    assert_eq!(states, [json!("not_mine"), json!("not_mine")]);
}
use deltabadger::engine::model::Level;

#[tokio::test]
async fn the_refusal_is_looked_for_in_the_newest_lines_only() {
    let f = fx(Kind::Binance);
    let a = api(&f, &BOTH);
    let id = obligation(&f, "2026-08-01 10:00:00", "2026-08-01 10:01:00", 1);
    let line = |reason: &str, pad: usize, at: &str| f.c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, ?2, 2, ?3, ?4)",
        params![f.b, ru::NOT_ADDED, json!({ "reason": reason, "venue": "Binance", "obligation": id, "pad": "x".repeat(pad) }).to_string(), at]).unwrap();
    line("unreachable", 0, "2026-09-30 10:00:00");
    line("ledger", 20_000, "2026-09-30 10:00:01");
    let reason = |listed: Value| listed["obligations"][0]["refused"]["reason"].clone();
    assert_eq!(reason(a.listed(f.b).await), json!("unreachable"), "an oversized line is skipped, the next older one shown");
    let others = |n: usize| for i in 0..n { f.c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'placement_ambiguous', 1, '{}', ?2)",
        params![f.b, format!("2026-09-30 11:{:02}:{:02}", i / 60, i % 60)]).unwrap(); };
    others(498);
    assert_eq!(reason(a.listed(f.b).await), json!("unreachable"), "500th newest line");
    others(1);
    assert_eq!(reason(a.listed(f.b).await), Value::Null, "past the window: open, no refusal");
    let mut q = f.c.prepare("EXPLAIN QUERY PLAN SELECT event, CASE WHEN octet_length(details) <= 16384 THEN details END, created_at FROM bot_activity_logs \
                             WHERE bot_id = 1 ORDER BY created_at DESC, id DESC LIMIT 500").unwrap();
    let plan = q.query_map([], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join("; ");
    assert!(plan.contains("index_bot_activity_logs_on_bot_id_and_created_at") && !plan.contains("TEMP B-TREE"), "{plan}");
}

#[tokio::test]
async fn the_read_is_refused_past_the_read_limits() {
    let f = fx(Kind::Binance);
    let a = api(&f, &BOTH);
    let id = obligation(&f, "2026-08-01 10:00:00", "2026-08-01 10:01:00", 0);
    f.c.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10001) \
                 INSERT INTO rust_rails_unrecorded_offers (obligation_id, external_id, symbol, facts) SELECT ?1, 'X-' || i, 'BTCEUR', '{}' FROM n", [id]).unwrap();
    let (status, body) = a.rest("GET", &format!("/api/v1/bots/{}", f.b), None).await;
    assert_eq!((status, body["error"]["code"].clone()), (501, json!("not_ported")));
    assert_eq!(a.mcp("get_bot_details", json!({ "bot_id": f.b })).await.0, "Read unavailable: request data exceeds read limits");
}

/// Codex branch r2: the Settings switch names the tool, on both surfaces.
#[test]
fn the_settings_switch_names_the_tool() {
    use deltabadger::web::i18n;
    for locale in ["en", "de", "pl"] {
        assert_eq!(i18n::text(locale, "settings.mcp.tools.answer_unrecorded_order.label", &[]), "Answer unrecorded order");
        assert_eq!(i18n::text(locale, "settings.mcp.tools.answer_unrecorded_order.description", &[]), "Say which order from before the update was a bot's, or that none was");
    }
}

// ---- A28-3b-1: Alpaca, its booking on the recorded answer -------------------------------------------------------------------

#[tokio::test]
async fn an_alpaca_order_is_offered_and_booked_once_with_its_creation_time_to_the_microsecond() {
    let f = fx(Kind::Alpaca);
    f.answer(&[o(7, St::Filled, "0.0004", "20"), limit(8, St::Open, "0", "0"), o(11, St::Cancelled, "0", "0")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    let mut ids: Vec<String> = offers.iter().map(|(_, e)| e.clone()).collect();
    ids.sort();
    assert_eq!(ids, [alpaca_id(7), alpaca_id(8)], "cancelled with nothing filled is never offered");
    let filled = offers.iter().find(|(_, e)| *e == alpaca_id(7)).unwrap().0;
    let before = count(&f.c, "SELECT count(*) FROM transactions");
    f.request(obligation, filled).unwrap();
    // The identity re-read answers the same nine-digit creation time: the stored facts (microseconds) must still match it.
    let served = f.serve().await;
    assert_eq!(served.booked.len(), 1, "{:?}", lines(&f.c, f.b, ru::NOT_ADDED));
    let tx = served.booked[0].1;
    let row = f.row(tx);
    assert_eq!((row["external_id"].clone(), row["quote_amount"].clone(), row["amount"].clone(), row["price"].clone(), row["created_at"].clone()),
               (json!(alpaca_id(7)), json!(20), Value::Null, json!(50000), json!(AT)));
    assert_eq!(count(&f.c, "SELECT count(*) FROM transactions"), before + 1);
    assert_eq!(lines(&f.c, f.b, ru::RECOVERED)[0]["venue_created_at"], "2026-09-29 14:03:05.123456");
    assert_eq!(f.serve().await.booked.len(), 0, "booked once");
    assert_eq!(count(&f.c, "SELECT count(*) FROM transactions"), before + 1);
    let read = f.tr.requests().into_iter().rev().find(|r| r.path == "/v2/orders").unwrap();
    assert_eq!(read.query.iter().find(|(k, _)| *k == "after").unwrap().1, "2026-09-29T13:53:04Z", "the identity read: its creation time - 1 s - the margin");
}

#[tokio::test]
async fn an_alpaca_order_the_re_read_does_not_list_is_refused_with_nothing_written() {
    // D-ALPACA-HISTORY-SUBMISSION-WINDOW: an order submitted past the margin is not in the re-read; the refusal is clean.
    let mut f = fx(Kind::Alpaca);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    f.tr = ScriptedTransport::default();
    f.answer(&[]);
    let before = money(&f.c);
    assert_eq!(f.serve().await.booked.len(), 0);
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "not_found");
    let after = money(&f.c);
    assert_eq!(after[..3], before[..3], "no order row, carry or link");
}

/// An Alpaca FILL activity of `order`, through activities::normalize, stored as the importer stores it.
fn alpaca_fill(c: &Connection, s: &Seeded, id: &str, order: &str, qty: &str, price: &str, at: &str) -> i64 {
    let raw = deltabadger::sync::activities::Raw::from_value(json!({ "id": id, "activity_type": "FILL", "transaction_time": at, "type": "fill", "price": price,
        "qty": qty, "side": "buy", "symbol": "BTC/USD", "leaves_qty": "0", "order_id": order, "cum_qty": qty, "order_status": "filled" }));
    let e = deltabadger::sync::activities::normalize(&raw, &deltabadger::sync::activities::CryptoPairs::default()).unwrap().unwrap();
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
               raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'BTC', ?6, 'USD', ?7, ?8, ?9, ?9, ?9)",
              params![s.user_id, s.exchange_id, s.api_key_id, e.tx_id, e.entry_type, e.base_amount.to_s_f(), e.quote_amount.as_ref().map(BigDec::to_s_f),
                      e.raw.value.to_string(), deltabadger::codec::format_time(e.transacted_at.unwrap())]).unwrap();
    c.last_insert_rowid()
}

#[tokio::test]
async fn an_alpaca_quote_from_a_rounded_average_is_booked_beside_the_ledgers_exact_fills() {
    // Two fills, 0.0002 at 50000 and 0.0001 at 50100: 15.01 USD. Alpaca states the average rounded, so its product is a hair less.
    for (exec, lost) in [("0.0003", false), ("0.0002", true)] {
        let f = fx(Kind::Alpaca);
        let fills = [alpaca_fill(&f.c, &f.s, "a1", &alpaca_id(7), "0.0002", "50000", "2026-09-29T14:03:06Z"),
                     alpaca_fill(&f.c, &f.s, "a2", &alpaca_id(7), "0.0001", "50100", "2026-09-29T14:03:07Z")];
        let mut row = alpaca_row(&o(7, St::Filled, exec, "15.01"));
        row["filled_avg_price"] = json!("50033.333333333");
        f.tr.reply("GET /v2/orders", 200, json!([row]));
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        let served = f.serve().await;
        if lost {
            // The venue reports less base than the ledger proves: refused, whatever the quote.
            assert_eq!((served.booked.len(), lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"].clone()), (0, json!("ledger")));
            continue;
        }
        assert_eq!(served.booked.len(), 1, "{:?}", lines(&f.c, f.b, ru::NOT_ADDED));
        let tx = served.booked[0].1;
        for id in fills {
            assert_eq!(f.c.query_row("SELECT transaction_id FROM account_transactions WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0)).unwrap(), Some(tx), "its fills linked");
        }
    }
}

// ---- A28-3c-3a: Hyperliquid, its booking on the recorded answer -------------------------------------------------------------

const HL_COIN: &str = "@142";
const HL_ORDERS: &str = "POST /info historicalOrders";
const HL_FILLS: &str = "POST /info userFillsByTime";
fn dec(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
/// A recorded Hyperliquid `filled` row (tests/fixtures/order_lists/hyperliquid, scrubbed) edited into `o` as the live-read
/// tests edit it: oid, coin, side, limitPx, origSz, `sz` left, status, both times.
fn hl_row(o: &O) -> Value {
    let rows: Vec<Value> = serde_json::from_str(include_str!("fixtures/order_lists/hyperliquid/historical_orders.json")).unwrap();
    let mut r = rows.into_iter().find(|r| r["status"] == "filled").unwrap();
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
/// A recorded fill edited: coin, oid, a buy, sz, px, tid, time; no fee.
fn hl_fill(oid: u64, sz: &str, px: &str, tid: u64, time: i64) -> Value {
    let fills: Vec<Value> = serde_json::from_str(include_str!("fixtures/order_lists/hyperliquid/user_fills_by_time.json")).unwrap();
    let mut f = fills[0].clone();
    for (k, v) in [("coin", json!(HL_COIN)), ("oid", json!(oid)), ("side", json!("B")), ("sz", json!(sz)), ("px", json!(px)), ("tid", json!(tid)), ("time", json!(time)), ("fee", json!("0.0"))] {
        f[k] = v;
    }
    f
}
/// The fill stored as the Hyperliquid ledger import stores it: a buy of BTC for EUR, keyed by its tid, its raw row kept.
fn hl_ledger_fill(c: &Connection, s: &Seeded, fill: &Value) -> i64 {
    let (sz, px) = (dec(fill["sz"].as_str().unwrap()), dec(fill["px"].as_str().unwrap()));
    let at = deltabadger::codec::format_time(DateTime::<Utc>::from_timestamp_millis(fill["time"].as_i64().unwrap()).unwrap());
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
               raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, 'BTC', ?5, 'EUR', ?6, ?7, ?8, ?8, ?8)",
              params![s.user_id, s.exchange_id, s.api_key_id, fill["tid"].to_string(), sz.to_s_f(), (&px * &sz).to_s_f(), fill.to_string(), at]).unwrap();
    c.last_insert_rowid()
}

#[tokio::test]
async fn a_hyperliquid_order_is_offered_and_booked_once_with_its_fills_quote_and_its_fills_linked() {
    let f = fx(Kind::Hyperliquid);
    // Two fills, 0.0003 at 50000 and 0.0001 at 50100: 20.01 EUR, the quote the venue's own fills state.
    let fills = [hl_fill(7, "0.0003", "50000", 1, ms(PLACED) + 400), hl_fill(7, "0.0001", "50100", 2, ms(PLACED) + 600)];
    let linked: Vec<i64> = fills.iter().map(|x| hl_ledger_fill(&f.c, &f.s, x)).collect();
    f.tr.reply(HL_ORDERS, 200, json!([hl_row(&limit(7, St::Filled, "0.0004", "20.01")), hl_row(&limit(8, St::Cancelled, "0", "0"))]));
    f.tr.reply(HL_FILLS, 200, json!(fills));
    f.report().await;
    let (obligation, offers) = f.offers();
    assert_eq!(offers.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>(), ["BTC-7"], "the id is <base>-<oid>; cancelled with nothing filled is never offered");
    let before = count(&f.c, "SELECT count(*) FROM transactions");
    f.request(obligation, offers[0].0).unwrap();
    let served = f.serve().await;
    assert_eq!(served.booked.len(), 1, "{:?}", lines(&f.c, f.b, ru::NOT_ADDED));
    let tx = served.booked[0].1;
    let row = f.row(tx);
    assert_eq!((row["external_id"].clone(), row["amount"].clone(), row["price"].clone(), row["created_at"].clone()), (json!("BTC-7"), json!(0.0004), json!(50000), json!(AT)));
    for id in linked {
        assert_eq!(f.c.query_row("SELECT transaction_id FROM account_transactions WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0)).unwrap(), Some(tx), "its fills linked");
    }
    assert_eq!(f.serve().await.booked.len(), 0, "booked once");
    assert_eq!(count(&f.c, "SELECT count(*) FROM transactions"), before + 1);
    // The identity re-read: the account's orders, then the fills from its creation time - 1 s.
    let bodies: Vec<Value> = f.tr.requests().iter().rev().take(2).map(|r| r.body.clone().unwrap()).collect();
    assert_eq!((bodies[1]["type"].clone(), bodies[0]["type"].clone(), bodies[0]["startTime"].clone()), (json!("historicalOrders"), json!("userFillsByTime"), json!(ms(PLACED) - 1000)));
}

// ---- A28-3d-1: MEXC, Bitrue, BingX, booked only when the venue's own lookup reads the same order (G4, r1.1) ----------------

const BINGX_OPEN: &str = "GET /openApi/spot/v1/trade/openOrders";
const BINGX_HISTORY: &str = "GET /openApi/spot/v1/trade/historyOrders";
const BINGX_QUERY: &str = "GET /openApi/spot/v1/trade/query";
const DOC_BUILT: [Kind; 3] = [Kind::Mexc, Kind::Bitrue, Kind::Bingx];
fn bingx_page(rows: Vec<Value>) -> Value { json!({ "code": 0, "msg": "", "data": { "orders": rows } }) }
fn doc_list(kind: Kind) -> &'static str { if kind == Kind::Mexc { "GET /api/v3/allOrders" } else { "GET /api/v1/allOrders" } }
fn doc_lookup(kind: Kind) -> &'static str { if kind == Kind::Mexc { "GET /api/v3/order" } else { "GET /api/v1/order" } }
/// A vector's raw LIMIT FILLED row (tests/fixtures/<venue>_vectors.json) edited into `o`, plus `time`, as the live-read tests build it.
fn doc_row(kind: Kind, o: &O) -> Value {
    let name = match kind { Kind::Mexc => "mexc", Kind::Bitrue => "bitrue", _ => "bingx" };
    let v: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/tests/fixtures/{name}_vectors.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let mut r = v[format!("{name}_orders")].as_array().unwrap().iter().map(|e| e["raw"].clone())
        .find(|r| r["type"] == "LIMIT" && r["status"] == "FILLED" && r.get("origQuoteOrderQty").is_some()).unwrap();
    let status = match o.state { St::Open => "NEW", St::Partly => "PARTIALLY_FILLED", St::Filled => "FILLED", St::PartlyCancelled | St::Cancelled => "CANCELED" };
    let (ty, orig, orig_quote, price) = match o.limit { Some(p) => ("LIMIT", o.size, "0", p), None => ("MARKET", o.exec, o.size, "0") };
    for (k, v) in [("symbol", json!(kind.symbol())), ("orderId", json!(o.id.to_string())), ("type", json!(ty)), ("side", json!(if o.sell { "SELL" } else { "BUY" })),
                   ("status", json!(status)), ("price", json!(price)), ("origQty", json!(orig)), ("origQuoteOrderQty", json!(orig_quote)), ("executedQty", json!(o.exec)),
                   ("cummulativeQuoteQty", json!(o.cum)), ("time", json!(ms(o.at)))] {
        r[k] = v;
    }
    r
}
/// The list alone (the lookup scripted by the case).
fn doc_list_only(f: &Fx, orders: &[O]) {
    if f.kind == Kind::Bingx {
        let (open, closed): (Vec<&O>, Vec<&O>) = orders.iter().partition(|o| matches!(o.state, St::Open | St::Partly));
        f.tr.reply(BINGX_OPEN, 200, bingx_page(open.iter().map(|o| doc_row(Kind::Bingx, o)).collect()));
        f.tr.reply(BINGX_HISTORY, 200, bingx_page(closed.iter().map(|o| doc_row(Kind::Bingx, o)).collect()));
    } else {
        f.tr.reply(doc_list(f.kind), 200, Value::Array(orders.iter().map(|o| doc_row(f.kind, o)).collect()));
    }
}
/// One fill of `order` stored as the venue's ledger import stores it: MEXC and Bitrue key it "SYMBOL-orderId-id", BingX by
/// its bare id (fills.rs reads either back to "SYMBOL-orderId").
fn doc_fill(f: &Fx, id: i64, order: &str, qty: &str, quote: &str, at: &str) -> i64 {
    let symbol = f.kind.symbol();
    let raw = json!({ "symbol": symbol, "id": id, "orderId": order, "price": "50000", "qty": qty, "quoteQty": quote, "commission": "0", "commissionAsset": "EUR",
                      "time": ms(at), "isBuyer": true, "isMaker": false });
    let tx_id = if f.kind == Kind::Bingx { id.to_string() } else { format!("{symbol}-{order}-{id}") };
    f.c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
                 raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, 'BTC', ?5, 'EUR', ?6, ?7, ?8, ?8, ?8)",
                params![f.s.user_id, f.s.exchange_id, f.s.api_key_id, tx_id, qty, quote, raw.to_string(), deltabadger::codec::format_time(t(at))]).unwrap();
    f.c.last_insert_rowid()
}
fn linked_to(c: &Connection, fill: i64) -> Option<i64> { c.query_row("SELECT transaction_id FROM account_transactions WHERE id = ?1", [fill], |r| r.get(0)).unwrap() }

#[tokio::test]
async fn a_doc_built_order_in_each_bookable_state_is_booked_when_its_lookup_reads_it_the_same() {
    for kind in DOC_BUILT {
        for order in [limit(7, St::Open, "0", "0"), limit(7, St::Partly, "0.0001", "5"), o(7, St::Filled, "0.0004", "20"), o(7, St::PartlyCancelled, "0.0002", "10")] {
            let f = fx(kind);
            let fill = (order.exec != "0").then(|| doc_fill(&f, 50, "7", order.exec, order.cum, "2026-09-29T14:03:06Z"));
            let tx = f.booked(std::slice::from_ref(&order)).await;
            let row = f.row(tx);
            assert_eq!(row["external_id"], json!(kind.external_id(7)), "{kind:?}");
            if let Some(fill) = fill { assert_eq!(linked_to(&f.c, fill), Some(tx), "{kind:?}: its fills linked"); }
            let lookup = f.tr.requests().into_iter().rev().find(|r| r.path == doc_lookup(kind).trim_start_matches("GET ") || r.path == bingx::QUERY_PATH).unwrap();
            let q = |k: &str| lookup.query.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
            assert_eq!((q("symbol"), q("orderId")), (Some(kind.symbol().to_string()), Some("7".to_string())), "{kind:?}: the Rails-called lookup by symbol and orderId");
            assert_eq!(f.serve().await.booked.len(), 0, "booked once");
        }
        // A string id, as MEXC and Bitrue may answer one: booked under it, its fill linked.
        let f = fx(kind);
        let fill = doc_fill(&f, 51, "C02__7", "0.0004", "20", "2026-09-29T14:03:06Z");
        let mut row = doc_row(kind, &o(7, St::Filled, "0.0004", "20"));
        row["orderId"] = json!("C02__7");
        if kind == Kind::Bingx { f.tr.reply(BINGX_OPEN, 200, bingx_page(vec![])); f.tr.reply(BINGX_HISTORY, 200, bingx_page(vec![row.clone()])); }
        else { f.tr.reply(doc_list(kind), 200, json!([row.clone()])); }
        f.lookup_row(row);
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        let served = f.serve().await;
        assert_eq!(served.booked.len(), 1, "{kind:?}: {:?}", lines(&f.c, f.b, ru::NOT_ADDED));
        assert_eq!(f.row(served.booked[0].1)["external_id"], json!(format!("{}-C02__7", kind.symbol())));
        assert_eq!(linked_to(&f.c, fill), Some(served.booked[0].1));
    }
}

#[tokio::test]
async fn a_doc_built_order_the_lookup_reads_otherwise_is_refused_unconfirmed() {
    type Edit = (&'static str, O, fn(&mut Value));
    let lim = limit(7, St::Filled, "0.0004", "20");
    let mkt = o(7, St::Filled, "0.0004", "20");
    let cases: [Edit; 10] = [
        ("another id", lim.clone(), |r| r["orderId"] = json!("8")),
        ("no id", lim.clone(), |r| { r.as_object_mut().unwrap().remove("orderId"); }),
        ("another symbol", lim.clone(), |r| r["symbol"] = json!(if r["symbol"] == "BTC-EUR" { "ETH-EUR" } else { "ETHEUR" })),
        ("size", lim.clone(), |r| r["origQty"] = json!("0.0005")),
        ("denomination", mkt.clone(), |r| { r["origQuoteOrderQty"] = json!("0"); r["origQty"] = json!("0.0004"); }),
        ("kind", lim.clone(), |r| { r["type"] = json!("MARKET"); r["price"] = json!("0"); }),
        ("price", lim.clone(), |r| r["price"] = json!("50001")),
        ("state", lim.clone(), |r| r["status"] = json!("PARTIALLY_FILLED")),
        ("filled base", lim.clone(), |r| r["executedQty"] = json!("0.0003")),
        ("filled quote", lim.clone(), |r| r["cummulativeQuoteQty"] = json!("19.99")),
    ];
    for kind in DOC_BUILT {
        for (name, order, edit) in cases.iter().cloned() {
            let f = fx(kind);
            doc_list_only(&f, std::slice::from_ref(&order));
            let mut row = doc_row(kind, &order);
            edit(&mut row);
            f.lookup_row(row);
            unconfirmed(&f, &format!("{kind:?} {name}")).await;
        }
        // The venue's "order gone" answer.
        let f = fx(kind);
        doc_list_only(&f, std::slice::from_ref(&lim));
        match kind {
            Kind::Bingx => { f.tr.reply(BINGX_QUERY, 200, json!({ "code": 100404, "msg": "order does not exist" })); }
            _ => { f.tr.reply(doc_lookup(kind), 400, json!({ "code": -2013, "msg": "Order does not exist." })); }
        }
        unconfirmed(&f, &format!("{kind:?} gone")).await;
    }
}
async fn unconfirmed(f: &Fx, name: &str) {
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    let before = money(&f.c);
    assert_eq!(f.serve().await.booked, [], "{name}");
    assert_eq!(money(&f.c), before, "{name}: nothing written");
    let refused = lines(&f.c, f.b, ru::NOT_ADDED);
    assert_eq!((refused.len(), refused[0]["reason"].as_str()), (1, Some("unconfirmed")), "{name}");
    assert_eq!(count(&f.c, "SELECT count(*) FROM rust_rails_unrecorded_requests"), 0, "{name}: the request goes");
}

#[tokio::test]
async fn a_failed_doc_built_lookup_counts_an_attempt_and_a_refused_one_refuses() {
    for kind in DOC_BUILT {
        let lookup = if kind == Kind::Bingx { BINGX_QUERY } else { doc_lookup(kind) };
        let f = fx(kind);
        doc_list_only(&f, &[o(7, St::Filled, "0.0004", "20")]);
        f.tr.reply(lookup, 500, json!("<html>busy</html>"));
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        let before = money(&f.c);
        f.serve_with(now(), None).await;
        assert!(lines(&f.c, f.b, ru::NOT_ADDED).is_empty(), "{kind:?}: a retry, no line");
        assert_eq!(count(&f.c, "SELECT attempts FROM rust_rails_unrecorded_requests"), 1, "{kind:?}: one attempt counted");
        f.serve_with(now() + Duration::seconds(60), None).await;
        f.serve_with(now() + Duration::seconds(120), None).await;
        let lookups = f.tr.requests().iter().filter(|r| format!("GET {}", r.path) == lookup).count();
        assert_eq!((lookups, money(&f.c) == before), (3, true), "{kind:?}");
        assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "unreachable", "{kind:?}");
        // The key refused at the lookup.
        let f = fx(kind);
        doc_list_only(&f, &[o(7, St::Filled, "0.0004", "20")]);
        f.tr.reply(lookup, 401, json!({ "code": 700002, "msg": "Signature for this request is not valid." }));
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        assert_eq!(f.serve().await.booked, []);
        assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "refused", "{kind:?}");
    }
}

// ---- A28-3d-2: Bitget, Bitvavo, Gemini, booked only when the venue's own lookup reads the same order (G4) ------------------

const BITGET_UNFILLED: &str = "GET /api/v2/spot/trade/unfilled-orders";
const BITGET_HISTORY: &str = "GET /api/v2/spot/trade/history-orders";
const BITGET_ORDER_INFO: &str = "GET /api/v2/spot/trade/orderInfo";
const BITVAVO_ORDERS: &str = "GET /v2/orders";
const BITVAVO_OPEN: &str = "GET /v2/ordersOpen";
const BITVAVO_ORDER: &str = "GET /v2/order";
const GEMINI_ACTIVE: &str = "POST /v1/orders";
const GEMINI_HISTORY: &str = "POST /v1/orders/history";
const GEMINI_STATUS: &str = "POST /v1/order/status";
const DOC_BUILT_2: [Kind; 3] = [Kind::Bitget, Kind::Bitvavo, Kind::Gemini];
fn bitget_page(rows: Vec<Value>) -> Value { json!({ "code": "00000", "msg": "success", "requestTime": 1790690585000u64, "data": rows }) }
fn avg(o: &O) -> Option<String> { dec(o.exec).is_positive().then(|| dec(o.cum).div(&dec(o.exec)).unwrap().to_s_f()) }
fn vector_row(name: &str, key: &str, find: impl Fn(&Value) -> bool) -> Value {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/tests/fixtures/{name}_vectors.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    v[key].as_array().unwrap().iter().map(|e| e["raw"].clone()).find(|r| find(r)).unwrap()
}
/// `o` as the venue lists it, as the live-read tests build it: a Bitget or Bitvavo vector's raw limit filled row edited into
/// it plus `cTime` / `created`, or Gemini's order body (tests/gemini.rs).
fn row2(kind: Kind, o: &O) -> Value {
    let side = if o.sell { "sell" } else { "buy" };
    match kind {
        Kind::Bitget => {
            let mut r = vector_row("bitget", "bitget_parsed", |r| r["orderType"] == "limit" && r["status"] == "filled");
            let status = match o.state { St::Open => "live", St::Partly => "partially_filled", St::Filled => "filled", St::PartlyCancelled | St::Cancelled => "cancelled" };
            let (ty, price) = match o.limit { Some(p) => ("limit", p), None => ("market", "0") };
            for (k, v) in [("symbol", json!("BTCEUR")), ("orderId", json!(o.id.to_string())), ("orderType", json!(ty)), ("side", json!(side)), ("status", json!(status)),
                           ("price", json!(price)), ("size", json!(o.size)), ("priceAvg", json!(avg(o))), ("baseVolume", json!(o.exec)), ("quoteVolume", json!(o.cum)),
                           ("cTime", json!(ms(o.at).to_string()))] {
                r[k] = v;
            }
            r
        }
        Kind::Bitvavo => {
            let mut r = vector_row("bitvavo", "bitvavo_orders", |r| r["orderType"] == "limit" && r["status"] == "filled");
            let status = match o.state { St::Open => "new", St::Partly => "partiallyFilled", St::Filled => "filled", St::PartlyCancelled | St::Cancelled => "canceled" };
            let (ty, price, base, quote) = match o.limit { Some(p) => ("limit", json!(p), json!(o.size), Value::Null), None => ("market", Value::Null, Value::Null, json!(o.size)) };
            for (k, v) in [("orderId", json!(o.id.to_string())), ("market", json!("BTC-EUR")), ("orderType", json!(ty)), ("side", json!(side)), ("status", json!(status)),
                           ("price", price), ("amount", base), ("amountQuote", quote), ("filledAmount", json!(o.exec)), ("filledAmountQuote", json!(o.cum)), ("created", json!(ms(o.at)))] {
                r[k] = v;
            }
            r
        }
        _ => {
            let live = matches!(o.state, St::Open | St::Partly);
            let remaining = if live { (&dec(o.size) - &dec(o.exec)).to_s_f() } else { "0".into() };
            json!({ "order_id": o.id.to_string(), "id": o.id.to_string(), "symbol": "btceur", "exchange": "gemini", "avg_execution_price": avg(o).unwrap_or_else(|| "0".into()),
                    "side": side, "type": "exchange limit", "timestamp": (ms(o.at) / 1000).to_string(), "timestampms": ms(o.at), "is_live": live,
                    "is_cancelled": matches!(o.state, St::PartlyCancelled | St::Cancelled), "is_hidden": false, "was_forced": false, "executed_amount": o.exec,
                    "remaining_amount": remaining, "original_amount": o.size, "price": o.limit.unwrap_or("0"), "options": [] })
        }
    }
}
/// The lists alone (the lookup scripted by the case), these rows split into the open ones and the rest.
fn doc2_rows(f: &Fx, open: Vec<Value>, closed: Vec<Value>) {
    match f.kind {
        Kind::Bitget => { f.tr.reply(BITGET_UNFILLED, 200, bitget_page(open)); f.tr.reply(BITGET_HISTORY, 200, bitget_page(closed)); }
        Kind::Bitvavo => { f.tr.reply(BITVAVO_ORDERS, 200, Value::Array(closed)); f.tr.reply(BITVAVO_OPEN, 200, Value::Array(open)); }
        _ => { f.tr.reply(GEMINI_ACTIVE, 200, Value::Array(open)); f.tr.reply(GEMINI_HISTORY, 200, Value::Array(closed)); }
    }
}
fn doc2_list(f: &Fx, orders: &[O]) {
    let (open, closed): (Vec<&O>, Vec<&O>) = orders.iter().partition(|o| matches!(o.state, St::Open | St::Partly));
    doc2_rows(f, open.iter().map(|o| row2(f.kind, o)).collect(), closed.iter().map(|o| row2(f.kind, o)).collect());
}
/// One fill of `order` stored as the venue's ledger import stores it (fills.rs keys Bitget's and Bitvavo's to
/// "<symbol>-<orderId>", Gemini's to the bare order_id).
fn doc2_fill(f: &Fx, id: i64, order: Value, qty: &str, quote: &str, at: &str) -> i64 {
    let price = dec(quote).div(&dec(qty)).unwrap().to_s_f();
    let raw = match f.kind {
        Kind::Bitget => json!({ "symbol": "BTCEUR", "orderId": order, "tradeId": id.to_string(), "side": "buy", "orderType": "limit", "priceAvg": price, "size": qty,
                                "amount": quote, "feeDetail": { "feeCoin": "EUR", "totalFee": "0" }, "cTime": ms(at).to_string() }),
        Kind::Bitvavo => json!({ "market": "BTC-EUR", "id": id.to_string(), "orderId": order, "side": "buy", "amount": qty, "price": price, "fee": "0", "feeCurrency": "EUR",
                                 "timestamp": ms(at) }),
        _ => json!({ "symbol": "btceur", "tid": id, "order_id": order, "type": "Buy", "amount": qty, "price": price, "fee_amount": "0", "fee_currency": "EUR",
                     "timestampms": ms(at) }),
    };
    f.c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
                 raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, 'BTC', ?5, 'EUR', ?6, ?7, ?8, ?8, ?8)",
                params![f.s.user_id, f.s.exchange_id, f.s.api_key_id, id.to_string(), qty, quote, raw.to_string(), deltabadger::codec::format_time(t(at))]).unwrap();
    f.c.last_insert_rowid()
}
fn lookup2(kind: Kind) -> &'static str { match kind { Kind::Bitget => BITGET_ORDER_INFO, Kind::Bitvavo => BITVAVO_ORDER, _ => GEMINI_STATUS } }
fn asked(r: &deltabadger::venue::http::HttpRequest, k: &str) -> Option<String> {
    r.query.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone()).or_else(|| r.body.as_ref().and_then(|b| b[k].as_str()).map(str::to_string))
}

#[tokio::test]
async fn a_bitget_bitvavo_or_gemini_order_in_each_bookable_state_is_booked_when_its_lookup_reads_it_the_same() {
    for kind in DOC_BUILT_2 {
        // Gemini places limits only.
        let filled = if kind == Kind::Gemini { limit(7, St::Filled, "0.0004", "20") } else { o(7, St::Filled, "0.0004", "20") };
        let partly = if kind == Kind::Gemini { limit(7, St::PartlyCancelled, "0.0002", "10") } else { o(7, St::PartlyCancelled, "0.0002", "10") };
        for order in [limit(7, St::Open, "0", "0"), limit(7, St::Partly, "0.0001", "5"), filled, partly] {
            let f = fx(kind);
            let fill = (order.exec != "0").then(|| doc2_fill(&f, 50, json!("7"), order.exec, order.cum, "2026-09-29T14:03:06Z"));
            let tx = f.booked(std::slice::from_ref(&order)).await;
            assert_eq!(f.row(tx)["external_id"], json!(kind.external_id(7)), "{kind:?}");
            if let Some(fill) = fill { assert_eq!(linked_to(&f.c, fill), Some(tx), "{kind:?}: its fills linked"); }
            let lookup = f.tr.requests().into_iter().rev().find(|r| format!("{} {}", r.method, r.path) == lookup2(kind)).unwrap();
            let want = match kind { Kind::Bitget => (None, Some("7")), Kind::Bitvavo => (Some("BTC-EUR"), Some("7")), _ => (None, None) };
            assert_eq!((asked(&lookup, "market"), asked(&lookup, "orderId")), (want.0.map(str::to_string), want.1.map(str::to_string)), "{kind:?}: the Rails-called lookup");
            if kind == Kind::Gemini { assert_eq!(lookup.body, Some(json!({ "order_id": "7" }))); }
            assert_eq!(f.serve().await.booked.len(), 0, "booked once");
        }
    }
    // r1.3: a Gemini id that is a JSON integer, in the list and the lookup: booked under its digits, its fill linked.
    let f = fx(Kind::Gemini);
    let fill = doc2_fill(&f, 51, json!(106817811u64), "0.0004", "20", "2026-09-29T14:03:06Z");
    let mut row = row2(Kind::Gemini, &limit(7, St::Filled, "0.0004", "20"));
    row["order_id"] = json!(106817811u64);
    doc2_rows(&f, vec![], vec![row.clone()]);
    f.lookup_row(row);
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    let served = f.serve().await;
    assert_eq!(served.booked.len(), 1, "{:?}", lines(&f.c, f.b, ru::NOT_ADDED));
    assert_eq!(f.row(served.booked[0].1)["external_id"], json!("106817811"));
    assert_eq!(linked_to(&f.c, fill), Some(served.booked[0].1));
}

#[tokio::test]
async fn a_bitget_bitvavo_or_gemini_order_the_lookup_reads_otherwise_is_refused_unconfirmed() {
    type Edit = (&'static str, fn(&mut Value));
    let lim = limit(7, St::Filled, "0.0004", "20");
    let cases: [(Kind, O, &[Edit]); 4] = [
        (Kind::Bitget, lim.clone(), &[
            ("another id", |r| r["orderId"] = json!("8")), ("no id", |r| { r.as_object_mut().unwrap().remove("orderId"); }), ("another symbol", |r| r["symbol"] = json!("ETHEUR")),
            ("side", |r| r["side"] = json!("sell")), ("size", |r| r["size"] = json!("0.0005")), ("kind", |r| { r["orderType"] = json!("market"); r["price"] = json!("0"); }),
            ("price", |r| r["price"] = json!("50001")), ("state", |r| r["status"] = json!("partially_filled")), ("filled base", |r| r["baseVolume"] = json!("0.0003")),
            ("filled quote", |r| r["quoteVolume"] = json!("19.99")), ("time", |r| r["cTime"] = json!((ms(PLACED) + 1).to_string())),
        ]),
        (Kind::Bitvavo, lim.clone(), &[
            ("another id", |r| r["orderId"] = json!("8")), ("another market", |r| r["market"] = json!("ETH-EUR")), ("size", |r| r["amount"] = json!("0.0005")),
            ("kind", |r| { r["orderType"] = json!("market"); r["price"] = Value::Null; }), ("price", |r| r["price"] = json!("50001")),
            ("state", |r| r["status"] = json!("partiallyFilled")), ("filled base", |r| r["filledAmount"] = json!("0.0003")),
            ("filled quote", |r| r["filledAmountQuote"] = json!("19.99")),
        ]),
        (Kind::Bitvavo, o(7, St::Filled, "0.0004", "20"), &[("denomination", |r| { r["amountQuote"] = Value::Null; r["amount"] = json!("20"); })]),
        (Kind::Gemini, lim.clone(), &[
            ("another id", |r| r["order_id"] = json!("8")), ("another symbol", |r| r["symbol"] = json!("ethusd")), ("size", |r| { r["original_amount"] = json!("0.0005"); r["executed_amount"] = json!("0.0005"); }),
            ("kind", |r| r["type"] = json!("exchange market")), ("price", |r| r["price"] = json!("50001")),
            ("state", |r| { r["is_live"] = json!(true); r["remaining_amount"] = json!("0.0001"); }), ("filled quote", |r| r["avg_execution_price"] = json!("49975")),
            ("r1.2: executed at a zero average", |r| r["avg_execution_price"] = json!("0")), ("time", |r| r["timestampms"] = json!(ms(PLACED) + 1)),
        ]),
    ];
    for (kind, order, edits) in cases {
        for (name, edit) in edits {
            let f = fx(kind);
            doc2_list(&f, std::slice::from_ref(&order));
            let mut row = row2(kind, &order);
            edit(&mut row);
            f.lookup_row(row);
            unconfirmed(&f, &format!("{kind:?} {name}")).await;
        }
    }
    // The venue's "order gone" answer.
    for (kind, status, body) in [(Kind::Bitget, 400, json!({ "code": "40109", "msg": "The data of the order cannot be found" })), (Kind::Bitget, 200, bitget_page(vec![])),
                                 (Kind::Bitvavo, 404, json!({ "errorCode": 240, "error": "No order found." })),
                                 (Kind::Gemini, 404, json!({ "result": "error", "reason": "OrderNotFound", "message": "not found" }))] {
        let f = fx(kind);
        doc2_list(&f, std::slice::from_ref(&lim));
        f.tr.reply(lookup2(kind), status, body);
        unconfirmed(&f, &format!("{kind:?} gone {status}")).await;
    }
}

#[tokio::test]
async fn a_failed_bitget_bitvavo_or_gemini_lookup_counts_an_attempt_and_a_refused_one_refuses() {
    for (kind, refusal) in [(Kind::Bitget, json!({ "code": "40009", "msg": "sign signature error" })), (Kind::Bitvavo, json!({ "errorCode": 309, "error": "Your signature is invalid." })),
                            (Kind::Gemini, json!({ "result": "error", "reason": "InvalidSignature", "message": "x" }))] {
        let order = limit(7, St::Filled, "0.0004", "20");
        let f = fx(kind);
        doc2_list(&f, std::slice::from_ref(&order));
        f.tr.reply(lookup2(kind), 500, json!("<html>busy</html>"));
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        let before = money(&f.c);
        f.serve_with(now(), None).await;
        assert!(lines(&f.c, f.b, ru::NOT_ADDED).is_empty(), "{kind:?}: a retry, no line");
        assert_eq!(count(&f.c, "SELECT attempts FROM rust_rails_unrecorded_requests"), 1, "{kind:?}: one attempt counted");
        f.serve_with(now() + Duration::seconds(60), None).await;
        f.serve_with(now() + Duration::seconds(120), None).await;
        let lookups = f.tr.requests().iter().filter(|r| format!("{} {}", r.method, r.path) == lookup2(kind)).count();
        assert_eq!((lookups, money(&f.c) == before), (3, true), "{kind:?}");
        assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "unreachable", "{kind:?}");
        // The key refused at the lookup, by its documented code on a 400.
        let f = fx(kind);
        doc2_list(&f, std::slice::from_ref(&order));
        f.tr.reply(lookup2(kind), 400, refusal);
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        assert_eq!(f.serve().await.booked, []);
        assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "refused", "{kind:?}");
    }
}

#[tokio::test]
async fn r1_1_a_bitget_order_that_gains_a_bare_id_row_before_its_booking_is_held() {
    // The offer is made; then a legacy row holding the bare `<orderId>` (bitget.rb:719) appears: the booking is refused held.
    let f = fx(Kind::Bitget);
    f.answer(&[o(7, St::Filled, "0.0004", "20")]);
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    let other = seed::insert_bot(&f.c, &f.s, &BotSpec::weekly(20.0, "2026-09-01 10:00:00"));
    f.c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
                 VALUES (?1, ?2, '7', 0, 0, 'REGULAR', '[]', ?3, ?3)", params![other, f.s.exchange_id, AT]).unwrap();
    let before = money(&f.c);
    assert_eq!(f.serve().await.booked, []);
    assert_eq!(money(&f.c), before);
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "held");
}

// ---- A28-3d-3: Coinbase, booked only when its own historical/{id} lookup reads the same order (G4) ----------------------

const CB_ORDERS: &str = "GET /api/v3/brokerage/orders/historical/batch";
fn cb_lookup(id: &str) -> String { format!("GET /api/v3/brokerage/orders/historical/{id}") }
fn cb_page(rows: Vec<Value>) -> Value { json!({ "orders": rows, "sequence": "0", "has_next": false, "cursor": "" }) }
/// `o` as the live-read tests build it: a Coinbase vector's raw MARKET FILLED row (`coinbase_orders`) edited into it, its
/// configuration as `add_order` sends it, plus `created_time`.
fn cb_row(o: &O) -> Value {
    let mut r = vector_row("coinbase", "coinbase_orders", |r| r["order_type"] == "MARKET" && r["status"] == "FILLED");
    let status = match o.state { St::Open | St::Partly => "OPEN", St::Filled => "FILLED", St::PartlyCancelled | St::Cancelled => "CANCELLED" };
    let (ty, config) = match o.limit {
        Some(p) => ("LIMIT", json!({ "limit_limit_gtc": { "base_size": o.size, "limit_price": p } })),
        None => ("MARKET", json!({ "market_market_ioc": { "quote_size": o.size } })),
    };
    for (k, v) in [("order_id", json!(o.id.to_string())), ("product_id", json!("BTC-EUR")), ("order_type", json!(ty)), ("side", json!(if o.sell { "SELL" } else { "BUY" })),
                   ("status", json!(status)), ("average_filled_price", json!(avg(o).unwrap_or_else(|| "0".into()))), ("filled_size", json!(o.exec)),
                   ("total_value_after_fees", json!(o.cum)), ("outstanding_hold_amount", json!("0")), ("order_configuration", config), ("created_time", json!(o.at))] {
        r[k] = v;
    }
    r
}
/// One fill of `order` stored as the Coinbase ledger import stores it (fills.rs keys it to the bare order_id, tx_id its trade_id).
fn cb_fill(f: &Fx, id: i64, order: &str, qty: &str, quote: &str, at: &str) -> i64 {
    let price = dec(quote).div(&dec(qty)).unwrap().to_s_f();
    let raw = json!({ "entry_id": format!("e{id}"), "trade_id": id.to_string(), "order_id": order, "trade_time": at, "trade_type": "FILL", "price": price, "size": qty,
                      "commission": "0", "product_id": "BTC-EUR", "sequence_timestamp": at, "liquidity_indicator": "TAKER", "size_in_quote": false, "user_id": "u",
                      "side": "BUY", "retail_portfolio_id": "p" });
    f.c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
                 raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, 'BTC', ?5, 'EUR', ?6, ?7, ?8, ?8, ?8)",
                params![f.s.user_id, f.s.exchange_id, f.s.api_key_id, id.to_string(), qty, quote, raw.to_string(), deltabadger::codec::format_time(t(at))]).unwrap();
    f.c.last_insert_rowid()
}

#[tokio::test]
async fn a_coinbase_order_in_each_bookable_state_is_booked_when_its_lookup_reads_it_the_same() {
    for order in [limit(7, St::Open, "0", "0"), limit(7, St::Partly, "0.0001", "5"), o(7, St::Filled, "0.0004", "20"), o(7, St::PartlyCancelled, "0.0002", "10")] {
        let f = fx(Kind::Coinbase);
        let fill = (order.exec != "0").then(|| cb_fill(&f, 50, "7", order.exec, order.cum, "2026-09-29T14:03:06Z"));
        let tx = f.booked(std::slice::from_ref(&order)).await;
        assert_eq!(f.row(tx)["external_id"], json!("7"));
        if let Some(fill) = fill { assert_eq!(linked_to(&f.c, fill), Some(tx), "its fills linked"); }
        let lookup = f.tr.requests().into_iter().rev().find(|r| r.path.starts_with(coinbase::ORDER) && r.path != coinbase::ORDERS).unwrap();
        assert_eq!((lookup.path.as_str(), lookup.query.len()), ("/api/v3/brokerage/orders/historical/7", 0), "order_identified's lookup");
        assert_eq!(f.serve().await.booked.len(), 0, "booked once");
    }
}

#[tokio::test]
async fn a_coinbase_order_the_lookup_reads_otherwise_is_refused_unconfirmed() {
    type Edit = (&'static str, fn(&mut Value));
    let cases: [(O, &[Edit]); 2] = [
        (limit(7, St::Filled, "0.0004", "20"), &[
            ("another id", |r| r["order_id"] = json!("8")), ("no id", |r| { r.as_object_mut().unwrap().remove("order_id"); }),
            ("another product", |r| r["product_id"] = json!("ETH-EUR")), ("side", |r| r["side"] = json!("SELL")),
            ("size", |r| r["order_configuration"]["limit_limit_gtc"]["base_size"] = json!("0.0005")),
            ("kind", |r| { r["order_type"] = json!("MARKET"); r["order_configuration"] = json!({ "market_market_ioc": { "base_size": "0.0004" } }); }),
            ("price", |r| r["order_configuration"]["limit_limit_gtc"]["limit_price"] = json!("50001")), ("state", |r| r["status"] = json!("OPEN")),
            ("filled base", |r| r["filled_size"] = json!("0.0003")), ("filled quote", |r| r["total_value_after_fees"] = json!("19.99")),
            ("a hold", |r| r["outstanding_hold_amount"] = json!("0.01")), ("time", |r| r["created_time"] = json!("2026-09-29T14:03:05.000001Z")),
            ("PENDING", |r| r["status"] = json!("PENDING")),
        ]),
        (o(7, St::Filled, "0.0004", "20"), &[("denomination", |r| r["order_configuration"]["market_market_ioc"] = json!({ "base_size": "20" }))]),
    ];
    for (order, edits) in cases {
        for (name, edit) in edits {
            let f = fx(Kind::Coinbase);
            f.tr.reply(CB_ORDERS, 200, cb_page(vec![cb_row(&order)]));
            let mut row = cb_row(&order);
            edit(&mut row);
            f.tr.reply(&cb_lookup("7"), 200, json!({ "order": row }));
            unconfirmed(&f, name).await;
        }
    }
    // The order gone (HTTP 404).
    for (status, body) in [(404, json!({ "error": "NOT_FOUND", "code": 5, "message": "order not found", "details": [] }))] {
        let f = fx(Kind::Coinbase);
        f.tr.reply(CB_ORDERS, 200, cb_page(vec![cb_row(&limit(7, St::Filled, "0.0004", "20"))]));
        f.tr.reply(&cb_lookup("7"), status, body);
        unconfirmed(&f, &format!("gone {status}")).await;
    }
}

#[tokio::test]
async fn a_failed_coinbase_lookup_counts_an_attempt_and_one_refused_by_its_status_refuses() {
    // r1.2: an UNAUTHENTICATED body on a 400 is retried like any failure; a 401 refuses. Branch r1.2: so is a 200 whose body
    // carries no `order` (an error envelope, or the bare row).
    for failure in [(500, json!("<html>busy</html>")), (400, json!({ "error": "UNAUTHENTICATED", "code": 16, "message": "x", "details": [] })),
                    (200, json!({ "error": "INTERNAL", "code": 13, "message": "x", "details": [] })), (200, cb_row(&limit(7, St::Filled, "0.0004", "20")))] {
        let order = limit(7, St::Filled, "0.0004", "20");
        let f = fx(Kind::Coinbase);
        f.tr.reply(CB_ORDERS, 200, cb_page(vec![cb_row(&order)]));
        f.tr.reply(&cb_lookup("7"), failure.0, failure.1.clone());
        f.report().await;
        let (obligation, offers) = f.offers();
        f.request(obligation, offers[0].0).unwrap();
        let before = money(&f.c);
        f.serve_with(now(), None).await;
        assert!(lines(&f.c, f.b, ru::NOT_ADDED).is_empty(), "{failure:?}: a retry, no line");
        assert_eq!(count(&f.c, "SELECT attempts FROM rust_rails_unrecorded_requests"), 1, "one attempt counted");
        f.serve_with(now() + Duration::seconds(60), None).await;
        f.serve_with(now() + Duration::seconds(120), None).await;
        let lookups = f.tr.requests().iter().filter(|r| r.path.starts_with(coinbase::ORDER) && r.path != coinbase::ORDERS).count();
        assert_eq!((lookups, money(&f.c) == before), (3, true));
        assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "unreachable", "{failure:?}");
    }
    let f = fx(Kind::Coinbase);
    f.tr.reply(CB_ORDERS, 200, cb_page(vec![cb_row(&limit(7, St::Filled, "0.0004", "20"))]));
    f.tr.reply(&cb_lookup("7"), 401, json!({ "error": "UNAUTHENTICATED", "code": 16, "message": "Unauthorized", "details": [] }));
    f.report().await;
    let (obligation, offers) = f.offers();
    f.request(obligation, offers[0].0).unwrap();
    assert_eq!(f.serve().await.booked, []);
    assert_eq!(lines(&f.c, f.b, ru::NOT_ADDED)[0]["reason"], "refused");
}

#[tokio::test]
async fn an_id_that_cannot_stand_in_a_coinbase_path_is_never_looked_up() {
    let tr = ScriptedTransport::default();
    let venue = coinbase::CoinbaseVenue::new(tr.clone(), coinbase::URL);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    for id in ["", "7/../x", "7?a=b"] { assert_eq!(venue.order_created(id, "BTC-EUR", deadline).await, Ok(None), "{id}"); }
    assert!(tr.requests().is_empty());
}
