//! Bot::ResyncIndexCompositionJob in the engine (plan CI T3b): the durable `rust_index_resync` marker, consumed at pass start
//! before any tick, for any status. Its derivation is the tick's, proven against Rails in tests/parity.rs (grid-venue-index);
//! these pin what Rails' queue gives for free: it runs once after each trigger, survives a restart, and is never lost to a
//! trigger landing mid-run or to a write that rolls back.
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{index, model, FixedClock};
use deltabadger::store::{self, Paths};
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];
const NOW: &str = "2026-09-03T15:00:00Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }

/// What the transport answers, and a hook run once before the first price read (a write landing while the probes await).
#[derive(Clone)]
struct Hooked { inner: ScriptedTransport, db: String, sql: Rc<RefCell<Option<String>>> }
impl Transport for Hooked {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if r.path.contains("/quotes/") || r.path.contains("/trades/") {
            if let Some(sql) = self.sql.borrow_mut().take() { rusqlite::Connection::open(&self.db).unwrap().execute_batch(&sql).unwrap(); }
        }
        self.inner.send(r).await
    }
}

/// Every venue the engine asks for, by exchange type.
struct Factory { t: Hooked, asked: Rc<RefCell<Vec<String>>> }
impl deltabadger::venue::VenueFactory for Factory {
    type V = AlpacaVenue<Hooked>;
    fn for_bot(&self, exchange_type: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V {
        self.asked.borrow_mut().push(exchange_type.into());
        AlpacaVenue::new(self.t.clone(), Urls::for_passphrase(Some("paper")))
    }
}

struct Rig { dir: tempfile::TempDir, id: i64, s: seed::Seeded, t: ScriptedTransport, sql: Rc<RefCell<Option<String>>>, asked: Rc<RefCell<Vec<String>>> }
impl Rig {
    /// Five Alpaca stocks ranked AAA..EEE by data-api's ND100 row, a STOPPED index bot over it (top 3), a clock `open` or not,
    /// each stock's ask and last at `asks` (0 = unpriced).
    fn new(open: bool, asks: [f64; 5]) -> Self { Self::with(open, asks, json!({})) }
    /// `new`, with some replies replaced (`over`: the script's own shape).
    fn with(open: bool, asks: [f64; 5], over: Value) -> Self {
        let (dir, o, s) = common::install_alpaca();
        for sym in SYMBOLS { seed::add_alpaca_stock(&o.primary, &s, sym); }
        seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                           &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
        let id = seed::index_bot(&o.primary, &s, "nasdaq-100", 3, 0.0, false);
        o.primary.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap();
        let mut m = json!({
            "GET /v2/clock": ok(json!({ "timestamp": "2026-09-03T11:00:00-04:00", "is_open": open, "next_open": "2026-09-04T09:30:00-04:00", "next_close": "2026-09-03T16:00:00-04:00" })),
            "GET /v2/orders:by_client_order_id": [{ "status": 404, "body": { "code": 40410000, "message": "order not found" } }],
        });
        for (sym, ask) in SYMBOLS.iter().zip(asks) {
            m[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": ask, "bp": ask } }));
            m[format!("GET /v2/stocks/{sym}/trades/latest")] = ok(json!({ "trade": { "p": ask } }));
        }
        for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
        Rig { dir, id, s, t: ScriptedTransport::from_script(&m), sql: Rc::default(), asked: Rc::default() }
    }
    fn db(&self) -> String { self.dir.path().join("production.sqlite3").display().to_string() }
    fn c(&self) -> rusqlite::Connection { store::open(&Paths::from_env(&|_| None, self.dir.path())).unwrap().primary }
    fn engine(&self) -> Engine<Factory> {
        let paths = Paths::from_env(&|_| None, self.dir.path());
        let lock = deltabadger::lease::lock(&paths, at(NOW)).unwrap();
        let factory = Factory { t: Hooked { inner: self.t.clone(), db: self.db(), sql: self.sql.clone() }, asked: self.asked.clone() };
        Engine::new(store::open(&paths).unwrap().primary, factory, seed::cipher(), lock)
    }
    fn mark(&self) { index::mark_resync(&self.c(), self.id, at(NOW)).unwrap(); }
    fn marker(&self) -> Option<Value> {
        self.c().query_row("SELECT json_extract(transient_data, '$.rust_index_resync') FROM bots WHERE id = ?1", [self.id], |r| r.get::<_, Option<String>>(0))
            .unwrap().map(|m| serde_json::from_str(&m).unwrap())
    }
    fn members(&self) -> Vec<String> {
        let c = self.c();
        let mut s = c.prepare("SELECT a.symbol FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id WHERE m.bot_id = ?1 AND m.in_index = 1 ORDER BY m.target_allocation DESC").unwrap();
        s.query_map([self.id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }
    fn reads(&self, path: &str) -> usize { self.t.requests().iter().filter(|r| r.path.ends_with(path)).count() }
}
async fn step(e: &mut Engine<Factory>, now: &str) { run::step(e, &FixedClock(at(now))).await.unwrap(); }

#[tokio::test(flavor = "current_thread")]
async fn a_marked_stopped_bot_resyncs_at_the_first_pass_after_a_restart_and_its_marker_goes() {
    let r = Rig::new(true, [100.0, 200.0, 300.0, 400.0, 500.0]);
    r.mark();
    drop(r.engine()); // an engine that stopped before its first pass
    let mut e = r.engine();
    let mut events = e.subscribe_to(|ev| matches!(ev, EngineEvent::OrderUpdated { .. }));
    step(&mut e, NOW).await;
    assert_eq!(r.members(), ["AAA", "BBB", "CCC"]);
    assert_eq!(r.marker(), None);
    assert!(r.t.posted_orders().is_empty(), "a resync places nothing");
    assert_eq!(events.try_recv().ok(), Some(EngineEvent::OrderUpdated { bot_id: r.id }), "the panel is published (broadcast_metrics_panel)");
    let before = r.t.requests().len();
    step(&mut e, "2026-09-03T15:01:00Z").await;
    assert_eq!(r.t.requests().len(), before, "nothing pending: no second run");
}

/// IndexAllocatable#probe_side with the market closed: a stock newcomer is probed on its last (Rails' job has no market gate).
#[tokio::test(flavor = "current_thread")]
async fn with_the_market_closed_a_stock_newcomer_is_probed_on_its_last() {
    let r = Rig::new(false, [100.0, 200.0, 300.0, 400.0, 500.0]);
    r.mark();
    step(&mut r.engine(), NOW).await;
    assert_eq!(r.members(), ["AAA", "BBB", "CCC"]);
    assert_eq!((r.reads("/quotes/latest"), r.reads("/trades/latest"), r.reads("/v2/clock")), (0, 3, 1), "one clock read per derivation");
}

/// The failure kinds: a returned Failure clears the marker and is published; a rejected key clears it and is not; an Alpaca
/// transport failure keeps it and retries at n⁴ + 2 s, three attempts in all, then clears it.
#[tokio::test(flavor = "current_thread")]
async fn each_failure_kind_keeps_or_clears_the_marker_as_rails_job_ends() {
    let r = Rig::new(true, [0.0; 5]);
    r.mark();
    let mut e = r.engine();
    let mut events = e.subscribe_to(|ev| matches!(ev, EngineEvent::OrderUpdated { .. }));
    step(&mut e, NOW).await;
    assert_eq!((r.members().len(), r.marker()), (0, None), "No matching coins: the job completes");
    assert!(events.try_recv().is_ok());

    let html_401 = json!({ "GET /v2/stocks/AAA/quotes/latest": [{ "status": 401, "body": "<html><body>401 Authorization Required</body></html>" }] });
    let r = Rig::with(true, [100.0; 5], html_401);
    r.mark();
    let mut e = r.engine();
    let mut events = e.subscribe_to(|ev| matches!(ev, EngineEvent::OrderUpdated { .. }));
    step(&mut e, NOW).await;
    assert_eq!((r.members().len(), r.marker()), (0, None), "a rejected key raises: the job fails, not retried");
    assert!(events.try_recv().is_err(), "a raise broadcasts nothing");

    let refused = json!({ "GET /v2/stocks/AAA/quotes/latest": [{ "network": "pre_send", "message": "Faraday::ConnectionFailed: refused" }] });
    let r = Rig::with(true, [100.0; 5], refused);
    r.mark();
    let marker = r.marker();
    let mut e = r.engine();
    step(&mut e, NOW).await;
    assert_eq!(r.marker(), marker, "retry_on: kept");
    step(&mut e, "2026-09-03T15:00:02Z").await;
    assert_eq!(r.reads("/AAA/quotes/latest"), 1, "waiting 1⁴ + 2 s");
    step(&mut e, "2026-09-03T15:00:03Z").await;
    assert_eq!((r.reads("/AAA/quotes/latest"), r.marker()), (2, marker.clone()), "the second attempt");
    step(&mut e, "2026-09-03T15:00:21Z").await;
    assert_eq!((r.reads("/AAA/quotes/latest"), r.marker()), (3, None), "the third attempt fails: attempts exhausted, cleared");
    assert!(r.members().is_empty());
}

/// A trigger landing while the probes await is not lost: one that only bumps the marker lets the membership commit and keeps
/// the marker; a settings save or an index pull voids the run (index changed during price probes). Either way a second run
/// follows on the next pass.
#[tokio::test(flavor = "current_thread")]
async fn a_trigger_during_the_probes_keeps_the_marker_for_another_run() {
    for (sql, written) in [("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_index_resync.n', 99)", true),
                           ("UPDATE bots SET settings = json_set(settings, '$.num_coins', 2)", false),
                           ("UPDATE indices SET top_coins = '[\"BBB.US\",\"AAA.US\",\"CCC.US\",\"DDD.US\",\"EEE.US\"]'", false)] {
        let r = Rig::new(true, [100.0, 200.0, 300.0, 400.0, 500.0]);
        r.mark();
        *r.sql.borrow_mut() = Some(sql.into());
        let mut e = r.engine();
        step(&mut e, NOW).await;
        assert_eq!(!r.members().is_empty(), written, "{sql}");
        assert!(r.marker().is_some(), "{sql}: kept");
        step(&mut e, "2026-09-03T15:00:02Z").await;
        assert_eq!(r.marker(), None, "{sql}: the second run clears it");
        assert!(!r.members().is_empty(), "{sql}");
    }
}

/// The membership write and the marker clear are one commit: a refusal after write_members (check_install, made to refuse
/// by a write inside that transaction) rolls both back.
#[tokio::test(flavor = "current_thread")]
async fn a_refusal_after_the_membership_write_rolls_back_the_marker_clear_too() {
    let r = Rig::new(true, [100.0, 200.0, 300.0, 400.0, 500.0]);
    r.mark();
    let marker = r.marker();
    r.c().execute_batch(&format!("CREATE TRIGGER refuse AFTER INSERT ON bot_index_assets BEGIN \
                                   UPDATE bots SET status = 1, settings = json_set(settings, '$.direction', 'selling') WHERE id = {}; END", r.id)).unwrap();
    step(&mut r.engine(), NOW).await;
    assert_eq!((r.members().len(), r.marker()), (0, marker));
    assert_eq!(model::load_bot(&r.c(), r.id).unwrap().status, deltabadger::enums::BotStatus::Stopped);
}

/// A bot with an unresolved placement keeps its membership until the intent settles: the resync waits, marker kept.
#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_placement_defers_the_resync_until_it_settles() {
    use deltabadger::engine::{amount, placement};
    use deltabadger::ruby::BigDec;
    let r = Rig::new(true, [100.0, 200.0, 300.0, 400.0, 500.0]);
    let c = r.c();
    let bot = model::load_bot(&c, r.id).unwrap();
    let ticker_id: i64 = c.query_row("SELECT id FROM tickers WHERE ticker = 'AAA'", [], |r| r.get(0)).unwrap();
    let ticker = model::ticker_by_id(&c, r.s.exchange_id, ticker_id).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(100), &deltabadger::engine::venue_rules::ALPACA).unwrap() else { panic!("plan") };
    placement::begin(&c, &bot, &plan, &FixedClock(at(NOW))).unwrap();
    r.mark();
    let mut e = r.engine();
    for now in [NOW, "2026-09-03T15:01:00Z"] {
        step(&mut e, now).await;
        assert!(r.members().is_empty() && r.marker().is_some(), "{now}: deferred while the intent is unresolved");
    }
    c.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement')", []).unwrap(); // settled (NotPlaced)
    step(&mut e, "2026-09-03T15:02:00Z").await;
    assert_eq!((r.members(), r.marker()), (vec!["AAA".to_string(), "BBB".into(), "CCC".into()], None));
}

/// A marked bot outside this engine's slice (a stopped Bybit index bot), and one with no exchange: the first waits for
/// Rails without a venue ever being built for it (its marker kept for adopt_handback!), the second is never enqueued by
/// Rails and its marker goes. Other marked bots resync meanwhile.
#[tokio::test(flavor = "current_thread")]
async fn a_bot_outside_the_slice_keeps_its_marker_and_no_venue_is_built_for_it() {
    let r = Rig::new(true, [100.0, 200.0, 300.0, 400.0, 500.0]);
    let c = r.c();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Bybit', 'Bybit', '0.1', '0.1', '2026-01-01', '2026-01-01')", []).unwrap();
    let bybit = c.last_insert_rowid();
    let foreign = seed::index_bot(&c, &r.s, "nasdaq-100", 3, 0.0, false);
    let homeless = seed::index_bot(&c, &r.s, "nasdaq-100", 3, 0.0, false);
    c.execute("UPDATE bots SET status = 2, exchange_id = ?1 WHERE id = ?2", [bybit, foreign]).unwrap();
    c.execute("UPDATE bots SET status = 0, exchange_id = NULL WHERE id = ?1", [homeless]).unwrap();
    for id in [r.id, foreign, homeless] { index::mark_resync(&c, id, at(NOW)).unwrap(); }
    let mut e = r.engine();
    for now in [NOW, "2026-09-03T15:01:00Z", "2026-09-03T15:02:00Z"] {
        step(&mut e, now).await;
        index::mark_resync_all(&r.c(), at(now)).unwrap(); // the day's index pull marks every live index bot again
    }
    let marked: Vec<i64> = index::resync_marked(&r.c()).unwrap().into_iter().map(|(id, _)| id).collect();
    assert!(marked.contains(&foreign) && !marked.contains(&homeless), "{marked:?}");
    assert!(r.asked.borrow().iter().all(|t| t == "Exchanges::Alpaca"), "{:?}", r.asked.borrow());
    assert_eq!(r.members(), ["AAA", "BBB", "CCC"]);
}

/// Plan CI T2 (r2): the tokenized refusal is the bot's own tick, never the install. A running engine with a Binance basket
/// holding an open order and a Binance Layer-1 index bot: when the day's index pull ranks pax-gold (tokenized) inside
/// num_coins, the pass stays Ok, the basket's order is still polled, and the index tick fails with the member reason, writing
/// no member and placing nothing. Ranked outside num_coins, the index bot buys.
#[tokio::test(flavor = "current_thread")]
async fn a_tokenized_member_fails_only_its_index_bots_tick() {
    use deltabadger::venue::binance::{BinanceVenue, Pairs, URL};
    struct Binance(ScriptedTransport, Pairs);
    impl deltabadger::venue::VenueFactory for Binance {
        type V = BinanceVenue<ScriptedTransport>;
        fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { BinanceVenue::new(self.0.clone(), URL, "s").with_pairs(self.1.clone()) }
    }
    let (dir, o, s) = common::install();
    let c = &o.primary;
    c.execute("UPDATE exchanges SET type = 'Exchanges::Binance', name = 'Binance'", []).unwrap();
    c.execute("UPDATE assets SET external_id = 'tether', symbol = 'USDT', category = 'Cryptocurrency' WHERE id = ?1", [s.quote]).unwrap();
    c.execute("UPDATE tickers SET ticker = 'BTCUSDT', base = 'BTC', quote = 'USDT', quote_decimals = 8, price_decimals = 2, minimum_quote_size = '5'", []).unwrap();
    c.execute("UPDATE assets SET market_cap = 1.3e12 WHERE id = ?1", [s.btc]).unwrap();
    for (ext, sym, cap, instrument) in [("ethereum", "ETH", 4e11, None), ("pax-gold", "PAXG", 9e11, Some("tokenized"))] {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, instrument_type, market_cap, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', ?3, ?4, '2026-01-01', '2026-01-01')",
                  rusqlite::params![ext, sym, instrument, cap]).unwrap();
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2 || 'USDT', ?2, 'USDT', ?3, ?4, 5, 8, 2, '0.0001', '5', 1, 1, '2026-01-01', '2026-01-01')",
                  rusqlite::params![s.exchange_id, sym, c.last_insert_rowid(), s.quote]).unwrap();
    }
    let basket = seed::insert_bot(c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00"));
    seed::insert_tx(c, &s, basket, &seed::TxSpec { status: 0, external_status: Some(1), external_id: Some("BTCUSDT-28457".into()), order_type: 0, amount: None,
        quote_amount: Some("60"), price: None, quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 14:00:01".into() });
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.last_action_job_at', '2026-09-01T14:00:00.500Z') WHERE id = ?1", [basket]).unwrap();
    seed::insert_index(c, "layer-1", &["bitcoin", "pax-gold", "ethereum"], &json!({}));
    let index = seed::index_bot(c, &s, "layer-1", 2, 0.0, false);
    seed::fresh_stock_jobs(c, at("2026-09-01T14:00:00Z"));
    let exchange_id = s.exchange_id;
    let pairs = Pairs::load(c, exchange_id).unwrap();
    let book = |sym: &str, ask: &str| json!([{ "status": 200, "body": { "symbol": format!("{sym}USDT"), "bidPrice": ask, "bidQty": "1", "askPrice": ask, "askQty": "1" } }]);
    let t = ScriptedTransport::from_script(&json!({
        "GET /api/v3/ticker/bookTicker?symbol=BTCUSDT": book("BTC", "64000.00"), "GET /api/v3/ticker/bookTicker?symbol=ETHUSDT": book("ETH", "2500.00"),
        "GET /api/v3/ticker/bookTicker?symbol=PAXGUSDT": book("PAXG", "2400.00"),
        "GET /api/v3/allOrders": [{ "status": 200, "body": [] }],
        "GET /api/v3/order": [{ "status": 200, "body": { "symbol": "BTCUSDT", "orderId": 28457, "orderListId": -1, "clientOrderId": "web_28457", "price": "0.00000000",
            "origQty": "0.00000000", "executedQty": "0.00000000", "cummulativeQuoteQty": "0.00000000", "status": "NEW", "timeInForce": "GTC", "type": "MARKET",
            "side": "BUY", "origQuoteOrderQty": "60.00000000", "time": 1_788_000_000_000_i64 } }],
        "GET /api/v3/account": [{ "status": 200, "body": { "makerCommission": 10, "canTrade": true, "balances": [{ "asset": "USDT", "free": "100000", "locked": "0" }] } }],
        "POST /api/v3/order": (1..=4).map(|n| json!({ "status": 200, "body": { "symbol": "BTCUSDT", "orderId": 30_000 + n, "orderListId": -1, "clientOrderId": "x", "transactTime": 1_788_000_000_000_i64 } })).collect::<Vec<_>>(),
    }));
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T14:00:00Z")).unwrap();
    let mut e = Engine::new(store::open(&paths).unwrap().primary, Binance(t.clone(), pairs), seed::cipher(), lock);
    let members = || -> i64 { c.query_row("SELECT count(*) FROM bot_index_assets WHERE bot_id = ?1", [index], |r| r.get(0)).unwrap() };
    run::step(&mut e, &FixedClock(at("2026-09-01T14:00:00.5Z"))).await.expect("the pass stays Ok");
    assert!(t.requests().iter().any(|r| r.method == "GET" && (r.path.ends_with("Orders") || r.path == "/api/v3/order")), "the basket's open order is polled");
    assert!(t.requests().iter().all(|r| r.method != "POST"), "nothing placed");
    assert_eq!(members(), 0);
    let failed: String = c.query_row("SELECT details FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'execution_failed'", [index], |r| r.get(0)).unwrap();
    assert!(failed.contains("asset category Cryptocurrency / instrument tokenized"), "{failed}");

    c.execute("UPDATE indices SET top_coins = '[\"bitcoin\",\"ethereum\",\"pax-gold\"]'", []).unwrap(); // the next pull
    seed::fresh_stock_jobs(c, at("2026-09-08T14:00:00Z"));
    run::step(&mut e, &FixedClock(at("2026-09-08T14:00:01Z"))).await.expect("the pass stays Ok");
    assert_eq!(members(), 2, "BTC and ETH");
    assert!(t.requests().iter().filter(|r| r.method == "POST" && r.path == "/api/v3/order").count() >= 2, "the index bot buys");
}

/// Bots::DcaIndex#start enqueues Bot::ActionJob and Bot::ResyncIndexCompositionJob side by side: the first tick does not wait
/// for the resync. Here the start's resync and its first tick share the engine's first pass: no pass is lost between them.
#[tokio::test(flavor = "current_thread")]
async fn a_fresh_start_ticks_in_the_same_pass_as_its_resync() {
    let r = Rig::with(true, [100.0, 200.0, 300.0, 400.0, 500.0], json!({
        "POST /v2/orders": ok(json!({ "id": "OTX-1", "status": "accepted" })),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    }));
    let c = r.c();
    seed::fresh_stock_jobs(&c, at(NOW));
    c.execute("UPDATE bots SET status = 1, started_at = '2026-09-03 14:59:59' WHERE id = ?1", [r.id]).unwrap();
    r.mark();
    step(&mut r.engine(), NOW).await;
    assert_eq!(r.marker(), None, "the start's resync ran");
    let ticked: Option<String> = c.query_row("SELECT json_extract(transient_data, '$.last_action_job_at') FROM bots WHERE id = ?1", [r.id], |r| r.get(0)).unwrap();
    assert_eq!(ticked.as_deref(), Some("2026-09-03T15:00:00.000Z"), "the first tick ran in the resync's pass");
}

/// The page of a freshly started bot hears its first tick as Rails broadcasts it, here one whose order the venue refuses
/// (the key rejected). The engine announces the claim, each row and the end, in that order; each is rendered for the
/// owner's page as Rails renders it:
/// - the claim (`executing`, Bot after_update_commit, app/models/bot.rb:38-40): status and progress bars, both Start/Stop
///   buttons, the columns lock, on `user_<id>:bot_updates`;
/// - the failed row (Transaction after_create_commit → Bot#broadcast_new_order, bot.rb:316-348): the placeholder removed
///   with the bot's first row, the row's sentence prepended on the bot's page stream (no columnar row: it was not
///   accepted), the order filters replaced;
/// - the end (Bot::ActionJob's rescue and Bot::BroadcastAfterScheduledActionJob, app/jobs/bot/action_job.rb): the bars,
///   now naming the failure and the next try.
#[tokio::test(flavor = "current_thread")]
async fn the_page_hears_the_first_tick_as_rails_broadcasts_it() {
    let r = Rig::with(true, [100.0, 200.0, 300.0, 400.0, 500.0], json!({
        "POST /v2/orders": [{ "status": 401, "body": { "code": 40110000, "message": "unauthorized." } }],
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    }));
    let c = r.c();
    seed::fresh_stock_jobs(&c, at(NOW));
    c.execute("UPDATE bots SET status = 1, started_at = '2026-09-03 14:59:59' WHERE id = ?1", [r.id]).unwrap();
    r.mark();
    let mut e = r.engine();
    let mut events = e.subscribe_to(EngineEvent::is_live);
    step(&mut e, NOW).await;
    let mut heard = vec![];
    while let Ok(event) = events.try_recv() { heard.push(event); }
    let failed: Vec<i64> = c.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND status = 1 ORDER BY id").unwrap()
        .query_map([r.id], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert!(!failed.is_empty(), "the refused order is a failed row");
    let mut expected = vec![EngineEvent::StatusChanged { bot_id: r.id }];
    expected.extend(failed.iter().map(|&tx| EngineEvent::OrderRecorded { bot_id: r.id, transaction_id: tx }));
    expected.push(EngineEvent::TickEnded { bot_id: r.id });
    assert_eq!(heard, expected);

    let app = common::web::app(r.dir.path(), common::web::SECRET, common::web::TestClock::at(NOW));
    let render = |event: &EngineEvent| deltabadger::web::bot::live::render(&c, &app, event).unwrap();
    let user = format!("user_{}:bot_updates", r.s.user_id);
    let page = deltabadger::web::bot::page::page_stream(&deltabadger::web::bot::Bot::find(&c, r.s.user_id, r.id, deltabadger::web::bot::For::Page, "en").unwrap().unwrap());
    let shape = |messages: &[(String, String)]| messages.iter().map(|(stream, html)| {
        let action = html.split("action=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or_default().to_string();
        let target = html.split("target=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or_default().to_string();
        (if *stream == user { "user" } else if *stream == page { "page" } else { "?" }, action, target)
    }).collect::<Vec<_>>();
    let dom = |name: &str| format!("{name}_bots_dca_index_{}", r.id);
    let owned = |v: &[(&'static str, &str, String)]| v.iter().map(|(s, a, t)| (*s, a.to_string(), t.clone())).collect::<Vec<_>>();

    assert_eq!(shape(&render(&heard[0])), owned(&[("user", "replace", dom("status_bar")), ("user", "replace", dom("progress_bar")),
        ("user", "replace", dom("status_button")), ("user", "replace", dom("tile_status_button")), ("user", "add_class", dom("columns"))]));

    let first = render(&heard[1]);
    assert_eq!(shape(&first), owned(&[("page", "remove", "orders_list_placeholder".into()), ("page", "prepend", "orders_list".into()), ("user", "replace", dom("order_filters"))]));
    assert!(first[1].1.contains(&format!("id=\"timeline_transaction_{}\"", failed[0])) && first[1].1.contains("rejected the API key"), "{}", first[1].1);
    assert!(!first.iter().any(|(_, html)| html.contains("authenticity_token")), "a broadcast carries no session token");

    let ended = render(heard.last().unwrap());
    assert_eq!(shape(&ended), owned(&[("user", "replace", dom("status_bar")), ("user", "replace", dom("progress_bar"))]));
    assert!(ended[0].1.contains("rejected the API key") && ended[0].1.contains("countdown"), "the retrying bar names the failure: {}", ended[0].1);
}

/// The owner's switch of a Layer-1 index bot from Binance to KuCoin (settings saved while stopped, then a start): its stored
/// members still name Binance's tickers, ZEC among them, and KuCoin lists no ZEC at its quote. Rails' resync re-derives the
/// members from KuCoin's tickers (IndexAllocatable#derive_composition), marks ZEC out of the index (update_bot_index_assets)
/// and the next Bot::ActionJob buys what it seated. A stored member without a ticker here is what that resync replaces, not
/// a reason to leave the bot to Rails: the marker is consumed, the members are KuCoin's, the start is admitted and it ticks.
#[tokio::test(flavor = "current_thread")]
async fn an_index_bot_switched_to_a_venue_without_one_of_its_members_resyncs_and_ticks() {
    use deltabadger::venue::{binance::Pairs, kucoin::{KucoinVenue, URL}};
    struct Kucoin(ScriptedTransport, Pairs);
    impl deltabadger::venue::VenueFactory for Kucoin {
        type V = KucoinVenue<ScriptedTransport>;
        fn for_bot(&self, t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V {
            assert_eq!(t, "Exchanges::Kucoin", "only the bot's current venue is built");
            KucoinVenue::new(self.0.clone(), URL).with_pairs(self.1.clone())
        }
    }
    let (dir, o, s) = common::install();
    let c = &o.primary;
    let kucoin = s.exchange_id;
    c.execute("UPDATE exchanges SET type = 'Exchanges::Kucoin', name = 'KuCoin', maker_fee = '0.1', taker_fee = '0.1'", []).unwrap();
    c.execute("UPDATE assets SET external_id = 'tether', symbol = 'USDT', category = 'Cryptocurrency' WHERE id = ?1", [s.quote]).unwrap();
    c.execute("UPDATE assets SET market_cap = 1.3e12 WHERE id = ?1", [s.btc]).unwrap();
    c.execute("UPDATE tickers SET ticker = 'BTC-USDT', base = 'BTC', quote = 'USDT', base_decimals = 8, quote_decimals = 6, price_decimals = 1, \
               minimum_base_size = '0.00001', minimum_quote_size = '0.1'", []).unwrap();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Binance', 'Binance', '0.1', '0.1', '2026-01-01', '2026-01-01')", []).unwrap();
    let binance = c.last_insert_rowid();
    let ticker = |exchange: i64, pair: &str, base: &str, asset: i64| -> i64 {
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?3, 'USDT', ?4, ?5, 5, 6, 2, '0.0001', '0.1', 1, 1, '2026-01-01', '2026-01-01')",
                  rusqlite::params![exchange, pair, base, asset, s.quote]).unwrap();
        c.last_insert_rowid()
    };
    let asset = |ext: &str, sym: &str, cap: f64| -> i64 {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, market_cap, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', ?3, '2026-01-01', '2026-01-01')",
                  rusqlite::params![ext, sym, cap]).unwrap();
        c.last_insert_rowid()
    };
    let (zec, eth) = (asset("zcash", "ZEC", 9e11), asset("ethereum", "ETH", 4e11));
    ticker(kucoin, "ETH-USDT", "ETH", eth); // KuCoin lists no ZEC-USDT
    let old: Vec<i64> = [("BTCUSDT", "BTC", s.btc), ("ZECUSDT", "ZEC", zec), ("ETHUSDT", "ETH", eth)].iter().map(|(p, b, a)| ticker(binance, p, b, *a)).collect();
    seed::insert_index(c, "layer-1", &["bitcoin", "zcash", "ethereum"], &json!({}));
    let id = seed::index_bot(c, &s, "layer-1", 2, 0.0, false);
    c.execute("UPDATE bots SET status = 2, settings = json_set(settings, '$.quote_asset_id', ?2) WHERE id = ?1", [id, s.quote]).unwrap();
    // What Binance left: BTC and ZEC seated on Binance's tickers, each bought there once.
    seed::insert_member(c, id, s.btc, old[0], 0.59, true, "2026-08-01 00:00:00", None);
    seed::insert_member(c, id, zec, old[1], 0.41, true, "2026-08-01 00:00:00", None);
    for (n, (a, sym, qty, price)) in [(s.btc, "BTC", "0.0005", "64000"), (zec, "ZEC", "0.5", "50")].into_iter().enumerate() {
        c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, amount_exec, \
                   quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, 0, 2, 0, 0, ?4, 32, ?5, ?4, 32, ?6, 'USDT', ?7, ?8, 'week', 60, 'REGULAR', '[]', '2026-08-25 14:00:01', '2026-08-25 14:00:01')",
                  rusqlite::params![id, binance, format!("{}", 28_000 + n), qty, price, sym, a, s.quote]).unwrap();
    }
    // The settings save that moved it (Bot#custom_exchange_id_changed? enqueues the resync).
    c.execute("UPDATE bots SET exchange_id = ?2 WHERE id = ?1", [id, kucoin]).unwrap();
    let now = "2026-09-01T14:00:00.5Z";
    seed::fresh_stock_jobs(c, at(now));
    index::mark_resync(c, id, at(now)).unwrap();
    let pairs = Pairs::load(c, kucoin).unwrap();
    let kc = |body: Value| json!([{ "status": 200, "body": { "code": "200000", "data": body } }]);
    let book = |p: &str| kc(json!({ "time": 1, "sequence": "1", "bids": [[p, "1"]], "asks": [[p, "1"]] }));
    let t = ScriptedTransport::from_script(&json!({
        "GET /api/v1/market/orderbook/level2_20?symbol=BTC-USDT": book("64000.0"), "GET /api/v1/market/orderbook/level2_20?symbol=ETH-USDT": book("2500.00"),
        "GET /api/v1/market/allTickers": kc(json!({ "time": 1, "ticker": [{ "symbol": "BTC-USDT", "last": "64000.0" }, { "symbol": "ETH-USDT", "last": "2500.00" }] })),
        "GET /api/v1/accounts": kc(json!([{ "id": "q", "currency": "USDT", "type": "trade", "balance": "100000", "available": "100000", "holds": "0" }])),
        "GET /api/v1/fills": kc(json!({ "currentPage": 1, "pageSize": 500, "totalNum": 0, "totalPage": 0, "items": [] })),
        "POST /api/v1/orders": (1..=4).map(|n| json!({ "status": 200, "body": { "code": "200000", "data": { "orderId": format!("6716f0a2c1b6d10007a3b1c{n}") } } })).collect::<Vec<_>>(),
    }));
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at(now)).unwrap();
    let mut e = Engine::new(store::open(&paths).unwrap().primary, Kucoin(t.clone(), pairs), seed::cipher(), lock);
    let rows = || -> Vec<(String, bool, i64)> {
        let mut q = c.prepare("SELECT a.symbol, m.in_index, m.ticker_id FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id WHERE m.bot_id = ?1 ORDER BY a.symbol").unwrap();
        q.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    let resync: Option<String> = c.query_row("SELECT json_extract(transient_data, '$.rust_index_resync') FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(resync.is_some());
    run::step(&mut e, &FixedClock(at(now))).await.expect("the pass stays Ok");
    let resync: Option<String> = c.query_row("SELECT json_extract(transient_data, '$.rust_index_resync') FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(resync, None, "the resync ran here, not left to Rails");
    let eth_kc: i64 = c.query_row("SELECT id FROM tickers WHERE exchange_id = ?1 AND ticker = 'ETH-USDT'", [kucoin], |r| r.get(0)).unwrap();
    assert_eq!(rows(), vec![("BTC".into(), true, s.ticker_id), ("ETH".into(), true, eth_kc), ("ZEC".into(), false, old[1])],
               "BTC and ETH seated on KuCoin's tickers; ZEC, untradable there, out of the index (its row a holding only)");
    assert!(t.posted_orders().is_empty(), "a resync places nothing");

    // The start (Bots::DcaIndex#start): admitted by the shared rule, and the next pass ticks it on KuCoin.
    let tx = model::immediate(c).unwrap();
    tx.execute("UPDATE bots SET status = 1, started_at = '2026-09-01 14:00:01' WHERE id = ?1", [id]).unwrap();
    deltabadger::engine::eligibility::guard(&tx, &seed::cipher(), Some(id)).expect("the start is admitted");
    tx.commit().unwrap();
    run::step(&mut e, &FixedClock(at("2026-09-01T14:00:02Z"))).await.expect("the pass stays Ok");
    let pairs: Vec<String> = t.requests().iter().filter(|r| r.method == "POST" && r.path == "/api/v1/orders")
        .map(|r| r.body.clone().unwrap_or_default()["symbol"].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(pairs, ["BTC-USDT", "ETH-USDT"], "the bot buys what the resync seated");
}
