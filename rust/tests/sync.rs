//! The tracker's syncs on a Rails-prepared install: the write rule beside a trading engine, the jobs' contract with the
//! scheduler, the live-key refusal, and the hand-run command's lock. (Row-for-row agreement with Rails: sync_parity.rs.)
mod common;
use common::seed::{self, BotSpec, Seeded};
use deltabadger::crypto::Credentials;
use deltabadger::engine::{Clock, FixedClock};
use deltabadger::sync::balances::{self, NoPrices, ScriptedPrices};
use deltabadger::sync::job_api::{Cx, Db, EngineEvent, Jitter, Job, Outcome, PriceFuture, PriceSource, Retry, Schedule, Wake, DEADLINE};
use deltabadger::sync::{longest_write_hold, WRITE_GAP};
use deltabadger::sync::jobs::{self, BalanceSync, Connect, LedgerSync};
use deltabadger::sync::{self, ledger, SyncError, GUARD_REFUSED, LIVE_REFUSED};
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;

const ACTIVITIES: &str = "GET /v2/account/activities";
const NOW: &str = "2026-09-20T02:00:00Z";

fn clock() -> FixedClock { FixedClock(NOW.parse().unwrap()) }
fn paper() -> Credentials { Credentials { key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn interest(id: &str, date: &str) -> Value { json!({ "id": id, "activity_type": "INT", "net_amount": "0.07", "date": date }) }
fn split(id: &str, qty: &str) -> Value { json!({ "id": id, "activity_type": "SPLIT", "symbol": "AAPL", "qty": qty, "date": "2026-09-15" }) }

/// The seed's ids, copyable into the blocking closures.
#[derive(Clone, Copy)]
struct Ids { user_id: i64, exchange_id: i64, btc: i64, quote: i64, ticker_id: i64, api_key_id: i64 }
impl Ids {
    fn seeded(self) -> Seeded { Seeded { user_id: self.user_id, exchange_id: self.exchange_id, btc: self.btc, quote: self.quote, ticker_id: self.ticker_id, api_key_id: self.api_key_id } }
}

/// The Alpaca seed, plus what the syncs read beyond it: a stock, and the venue's asset list.
fn install() -> (tempfile::TempDir, Db, Ids) {
    let (dir, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('AAPL.US', 'AAPL', 'Apple', 'Stock', '2026-01-01', '2026-01-01')", []).unwrap();
    let aapl = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, \
               minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, 'AAPL', 'AAPL', 'USD', ?2, ?3, 9, 2, 2, '0.000000001', '1', 1, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.exchange_id, aapl, s.quote]).unwrap();
    for asset in [s.quote, s.btc, aapl] {
        c.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, '2026-01-01', '2026-01-01')", [asset, s.exchange_id]).unwrap();
    }
    let ids = Ids { user_id: s.user_id, exchange_id: s.exchange_id, btc: s.btc, quote: s.quote, ticker_id: s.ticker_id, api_key_id: s.api_key_id };
    (dir, Db::new(o.primary, Arc::new(seed::cipher())), ids)
}

fn venue(script: Value) -> (ScriptedTransport, AlpacaVenue<ScriptedTransport>) {
    let t = ScriptedTransport::from_script(&script);
    (t.clone(), AlpacaVenue::new(t, Urls::for_passphrase(None)))
}

async fn one<T: rusqlite::types::FromSql + Send + 'static>(db: &Db, sql: &'static str) -> T {
    db.run(move |c, _| c.query_row(sql, [], |r| r.get(0)).map_err(|e| e.to_string())).await.unwrap()
}

/// A second connection, as the engine's: can it take the write lock right now?
fn write_lock_is_free(dir: &Path) -> bool {
    let c = Connection::open(dir.join("production.sqlite3")).unwrap();
    c.busy_timeout(std::time::Duration::ZERO).unwrap();
    c.execute_batch("BEGIN IMMEDIATE; ROLLBACK").is_ok()
}

/// Asks, at every request, whether another writer could take the lock; answers from the script.
#[derive(Clone)]
struct Probing { inner: ScriptedTransport, dir: PathBuf, free: Rc<RefCell<Vec<bool>>> }
impl Transport for Probing {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.free.borrow_mut().push(write_lock_is_free(&self.dir));
        self.inner.send(r).await
    }
}

struct ProbingPrices { inner: ScriptedPrices, dir: PathBuf, free: Rc<RefCell<Vec<bool>>> }
impl PriceSource for ProbingPrices {
    fn prices<'a>(&'a self, ids: &'a [String], currency: &'a str) -> PriceFuture<'a> {
        self.free.borrow_mut().push(write_lock_is_free(&self.dir));
        self.inner.prices(ids, currency)
    }
}

fn balances_script() -> Value {
    json!({ "GET /v2/account": [ok(json!({ "cash": "100.5" }))],
            "GET /v2/positions": [ok(json!([{ "symbol": "AAPL", "qty": "2" }, { "symbol": "BTCUSD", "qty": "0.5" }]))],
            "GET /v2/stocks/snapshots": [ok(json!({ "AAPL": { "latestTrade": { "p": 227.52 } } }))] })
}

#[tokio::test(flavor = "current_thread")]
async fn no_write_lock_is_held_while_a_request_is_in_flight() {
    let (dir, db, s) = install();
    let free = Rc::new(RefCell::new(vec![]));
    let pages = json!({ ACTIVITIES: [ok(Value::Array((0..100).map(|i| interest(&format!("i{i:03}"), "2026-09-01")).collect())), ok(json!([interest("last", "2026-09-02")]))] });
    let probing = Probing { inner: ScriptedTransport::from_script(&pages), dir: dir.path().into(), free: free.clone() };
    let out = ledger::sync(&db, &AlpacaVenue::new(probing, Urls::for_passphrase(None)), s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!((out.imported, free.borrow().clone()), (101, vec![true, true]), "two pages, the lock free at each request");

    let probing = Probing { inner: ScriptedTransport::from_script(&balances_script()), dir: dir.path().into(), free: free.clone() };
    let prices = ProbingPrices { inner: ScriptedPrices::from_script(&json!({ "GET /api/v1/prices": [ok(json!({ "data": { "bitcoin": { "usd": 64000.5 } } }))] })), dir: dir.path().into(), free: free.clone() };
    let summary = balances::sync(&db, &AlpacaVenue::new(probing, Urls::for_passphrase(None)), &prices, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!((summary.synced, summary.priced_fresh, summary.pricing_error), (3, 3, None));
    assert_eq!(free.borrow().len(), 6, "account, positions, snapshots, prices");
    assert!(free.borrow().iter().all(|f| *f), "the write lock was held across a request: {:?}", free.borrow());
    assert!(write_lock_is_free(dir.path()), "and nothing is left open after a sync");
}

#[tokio::test(flavor = "current_thread")]
async fn another_writer_holding_the_database_delays_a_sync_but_never_fails_it() {
    let (dir, db, s) = install();
    let path = dir.path().join("production.sqlite3");
    let (held_tx, held) = std::sync::mpsc::channel();
    let engine = std::thread::spawn(move || {
        let c = Connection::open(path).unwrap();
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        held_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        c.execute_batch("COMMIT").unwrap();
    });
    held.recv().unwrap();
    let started = std::time::Instant::now();
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([interest("i1", "2026-09-01")]))] }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    engine.join().unwrap();
    assert_eq!(out.imported, 1);
    assert!(started.elapsed() >= std::time::Duration::from_millis(300), "the store waited for the other writer");
}

/// A working bot with a placement intent in hand (as `engine::placement` writes one) and one closed order that names AAPL.
fn bot_that_traded_aapl(c: &Connection, s: Ids) -> i64 {
    let intent = json!({ "cl_ord_id": "c-1", "deadline": "2026-09-19T00:00:10Z", "at": "2026-09-19T00:00:00Z", "ticker_id": s.ticker_id, "limit": false,
                         "price": "64000.0", "amount": "0.0009375", "quote_amount": "60.0", "quote_type": true, "volume": "60.0" });
    let bot = seed::insert_bot(c, &s.seeded(), &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("rust_placement", intent));
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, base, quote, amount, amount_exec, bot_interval, \
               bot_quote_amount, transaction_type, error_messages, created_at, updated_at) VALUES (?1, ?2, 'o-1', 0, 2, 0, 0, 'AAPL', 'USD', 10, 10, 'week', 60, 'REGULAR', '[]', \
               '2026-09-02 14:30:00', '2026-09-02 14:30:00')", [bot, s.exchange_id]).unwrap();
    bot
}

/// What a write that moves a bot's counter passes before it commits. On main a bot whose counter is above 0 is one the
/// engine does not run ("restated prices", until Plan 2d applies splits to a bot's holdings), so a split of a symbol a
/// working bot traded is refused: the unit rolls back, the sync fails, the watermark stays, and the next sync meets
/// the same row.
#[tokio::test(flavor = "current_thread")]
async fn the_engines_guard_refuses_a_split_of_a_working_bots_symbol_and_the_ledger_waits_at_that_row() {
    let (_dir, db, s) = install();
    let bot = db.run(move |c, _| Ok(bot_that_traded_aapl(c, s))).await.unwrap();
    let pages = json!({ ACTIVITIES: [ok(json!([interest("before", "2026-09-10"), split("s-remove", "-10"), split("s-add", "100"), interest("after", "2026-09-16")]))] });
    let state = "SELECT (SELECT count(*) FROM account_transactions) || ' rows, generation ' || (SELECT coalesce(restatement_generation, 0) FROM bots) || ', ' || \
                 (SELECT count(*) FROM bot_activity_logs) || ' lines, watermark ' || (SELECT coalesce(last_synced_at, 'none') || ', error ' || coalesce(last_sync_error, 'none') FROM api_keys)";
    let refused = format!("{GUARD_REFUSED}: bot {bot} (scheduled): restated prices");
    for _ in 0..2 {
        let (_, v) = venue(pages.clone());
        assert_eq!(ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap_err(), SyncError(refused.clone()));
        assert_eq!(one::<String>(&db, state).await, "1 rows, generation 0, 0 lines, watermark none, error none",
                   "the split, a unit of its own, is not stored without its counter, nor anything after it; the unit before it stays and is a duplicate next time");
    }
    let clock = clock();
    let job = LedgerSync::new(Scripted(ScriptedTransport::from_script(&pages)), s.api_key_id);
    assert_eq!(job.run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Failed(refused), "the job record shows it");
    // A balance sync writes nothing the guard looks at: it goes through beside the same working bot.
    let (_, v) = venue(balances_script());
    assert_eq!(balances::sync(&db, &v, &NoPrices, s.api_key_id, &paper(), &clock).await.unwrap().unwrap().synced, 3);
    // Once the bot is one the engine does not run anyway (stopped here), the same activities are stored, split and counter together.
    db.run(|c, _| c.execute("UPDATE bots SET status = 2", []).map_err(|e| e.to_string())).await.unwrap();
    let (_, v) = venue(pages);
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock).await.unwrap().unwrap();
    assert_eq!((out.duplicates, out.imported), (1, 2));
    assert_eq!(one::<String>(&db, state).await, "3 rows, generation 1, 1 lines, watermark 2026-09-16 00:00:00, error none");
}

/// The guard is the engine's say on writes to `bots`, and costs tens of milliseconds a call: a unit that writes no bot
/// does not ask it. Here the install is one the guard refuses outright (a working bot with restated prices), so any
/// unit that asked would fail.
#[tokio::test(flavor = "current_thread")]
async fn only_a_unit_that_writes_bots_asks_the_engines_guard() {
    let (_dir, db, s) = install();
    db.run(move |c, _| { bot_that_traded_aapl(c, s); c.execute("UPDATE bots SET restatement_generation = 1", []).map_err(|e| e.to_string()) }).await.unwrap();
    // Plain ledger units, a failure recorded on the key, the transfer links, the watermark; and a split of a symbol
    // no bot traded, which writes no bot.
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([interest("i-1", "2026-09-01"), { "id": "w-1", "activity_type": "CSW", "net_amount": "-100", "date": "2026-09-02" },
                                                      { "id": "d-1", "activity_type": "CSD", "net_amount": "100", "date": "2026-09-03" },
                                                      { "id": "q-remove", "activity_type": "SPLIT", "symbol": "QQQ", "qty": "-1", "date": "2026-09-04" },
                                                      { "id": "q-add", "activity_type": "SPLIT", "symbol": "QQQ", "qty": "2", "date": "2026-09-04" }]))] }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!((out.imported, out.linked, out.splits.len(), out.splits[0].restated_bots.len()), (4, 1, 1, 0));
    let (_, v) = venue(json!({ ACTIVITIES: [json!({ "status": 500, "body": { "message": "internal server error" } })] }));
    assert_eq!(ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap_err().error, "internal server error");
    // A balance unit.
    let (_, v) = venue(balances_script());
    assert_eq!(balances::sync(&db, &v, &NoPrices, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap().synced, 3);
    // The unit that would move the bot's counter does ask, and is refused.
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([split("s-remove", "-10"), split("s-add", "100")]))] }));
    let SyncError(refused) = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap_err();
    assert!(refused.starts_with(GUARD_REFUSED) && refused.contains("restated prices"), "{refused}");
    assert_eq!(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn a_split_touches_one_counter_on_bots_and_nothing_the_engine_places_with() {
    let (_dir, db, s) = install();
    // A stopped bot: the merged engine refuses a working bot whose prices were restated (the test above).
    let seeded = db.run(move |c, _| {
        let bot = bot_that_traded_aapl(c, s);
        c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).map_err(|e| e.to_string())?;
        Ok(bot)
    }).await.unwrap();
    let frozen = "SELECT group_concat(quote(id) || quote(status) || quote(settings) || quote(transient_data) || quote(updated_at) || quote(started_at), '|') FROM bots";
    let orders = "SELECT group_concat(quote(id) || quote(status) || quote(external_status) || quote(amount_exec) || quote(updated_at), '|') FROM transactions";
    let (bots_before, orders_before): (String, String) = (one(&db, frozen).await, one(&db, orders).await);

    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([split("s-remove", "-10"), split("s-add", "100")]))] }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!(out.splits, vec![ledger::Split { symbol: "AAPL".into(), at: "2026-09-15T00:00:00Z".parse().unwrap(), restated_bots: vec![seeded], effective_later: false }]);
    assert_eq!(one::<i64>(&db, "SELECT restatement_generation FROM bots").await, 1);
    assert_eq!(one::<String>(&db, "SELECT event || ' ' || details || ' ' || created_at FROM bot_activity_logs").await, r#"asset_split {"base":"AAPL","ratio":"10:1"} 2026-09-15 00:00:00"#);
    assert_eq!((one::<String>(&db, frozen).await, one::<String>(&db, orders).await), (bots_before, orders_before), "status, settings, the placement intent and every order are untouched");

    // The row the engine's holdings walk reads (Plan 2d).
    let row: String = one(&db, "SELECT entry_type || ' ' || base_currency || ' ' || base_amount || ' ' || transacted_at || ' ' || json_extract(raw_data, '$.corporate_action') || ' ' \
                                || json_extract(raw_data, '$.split_ratio') FROM account_transactions").await;
    assert_eq!(row, "15 AAPL 90 2026-09-15 00:00:00 split 10:1");

    // A split imported ahead of its date: the caller is told to bump again at that moment, and the bump is one call.
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([{ "id": "later", "activity_type": "SPLIT", "symbol": "AAPL", "qty": "5", "date": "2026-10-05" }]))] }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert!(out.splits[0].effective_later);
    let bumped = ledger::expire_restated(&db, s.user_id, s.exchange_id, "AAPL").await.unwrap();
    assert_eq!((bumped, one::<i64>(&db, "SELECT restatement_generation FROM bots").await), (vec![seeded], 3));
}

/// Every price is held to a venue number's caps, and every value computed from one (fresh or the last stored price)
/// must be finite and within them, before the first balance row is written: past either the sync fails and writes no
/// balance and no clock.
#[tokio::test(flavor = "current_thread")]
async fn a_price_or_a_value_outside_a_venue_numbers_range_fails_the_balance_sync_before_any_write() {
    let (_dir, db, s) = install();
    let state = "SELECT (SELECT count(*) || ' ' || coalesce(group_concat(quote(usd_value)), '') FROM account_balances) || ', synced ' || (SELECT coalesce(balances_synced_at, 'never') FROM api_keys)";
    let script = json!({ "GET /v2/account": [ok(json!({ "cash": "100.5" }))], "GET /v2/positions": [ok(json!([{ "symbol": "BTCUSD", "qty": "1000000000" }]))] });
    let prices = ScriptedPrices::from_script(&json!({ "GET /api/v1/prices": [ok(json!({ "data": { "bitcoin": { "usd": 1e300 } } }))] }));
    let (_, v) = venue(script.clone());
    let failure = balances::sync(&db, &v, &prices, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap_err();
    assert!(failure.error.contains("outside a venue number's range"), "{}", failure.error);
    assert_eq!(one::<String>(&db, state).await, "0 , synced never", "neither the cash row nor the coin's");
    // The last stored price, kept when no fresh one comes: 1e300 times a billion is no number.
    db.run(move |c, _| c.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, priced_at, usd_value, synced_at, created_at, updated_at) \
                                  VALUES (?1, ?2, ?3, 1, 0, 1e300, '2026-09-01', 1e300, '2026-09-01', '2026-09-01', '2026-09-01')", [s.user_id, s.exchange_id, s.btc]).map_err(|e| e.to_string())).await.unwrap();
    let (_, v) = venue(script);
    let failure = balances::sync(&db, &v, &NoPrices, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap_err();
    assert!(failure.error.contains("outside a venue number's range"), "{}", failure.error);
    assert_eq!(one::<String>(&db, state).await, "1 1.0e+300, synced never", "the stored row as it was, and no cash row");
}

/// A venue date is read only within 1970-01-01 ..= 9999-12-31 (UTC); one outside it is an unreadable activity time,
/// which fails the run and stores nothing, as any unreadable time does. Times read back from the database (the
/// watermark, a withdrawal Rails stored) go through checked arithmetic: none of them panics a sync.
#[tokio::test(flavor = "current_thread")]
async fn a_date_at_the_ends_of_the_calendar_is_unreadable_and_nothing_panics() {
    let (_dir, db, s) = install();
    for date in ["-262143-01-01", "+262142-12-31", "262142-12-31", "1969-12-31", "10000-01-01"] {
        let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([interest("fine", "2026-09-01"), interest("edge", date)]))] }));
        let failure = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap_err();
        assert_eq!((failure.error.as_str(), failure.raised), ("unreadable activity time", true), "{date}");
        assert_eq!(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, 0, "{date}: nothing stored");
    }
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([interest("first", "1970-01-01"), interest("last", "9999-12-31")]))] }));
    assert_eq!(ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap().imported, 2, "the ends of the range are read");
    // What Rails may have left: a watermark and a withdrawal at the ends of what the database can hold.
    db.run(move |c, _| {
        c.execute("UPDATE api_keys SET last_synced_at = '-262143-01-01 00:00:00'", []).map_err(|e| e.to_string())?;
        c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, raw_data, manual_values, \
                   transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, 5, 'USD', 10, '+262142-12-31 23:59:59', '{}', '{}', 0, '2026-01-01', '2026-01-01')",
                  [s.user_id, s.api_key_id, s.exchange_id]).map_err(|e| e.to_string())
    }).await.unwrap();
    let (t, v) = venue(json!({ ACTIVITIES: [ok(json!([]))] }));
    ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert!(t.requests()[0].query.iter().all(|(k, _)| *k != "after"), "a watermark with no time 25 h before it reads from the beginning");
}

/// A split imported ahead of its date bumps its bots once at import (as Rails' log_split does) and once more at its
/// date (Rails' Bot::ExpireRestatedMetricsJob): the second bump is kept in the sync's own `app_configs` row, applied by
/// the first ledger sync at or after the date, also after a restart, and then removed.
#[tokio::test(flavor = "current_thread")]
async fn a_split_dated_ahead_bumps_again_at_the_first_sync_on_or_after_its_date_across_a_restart() {
    let (dir, db, s) = install();
    db.run(move |c, _| { let bot = bot_that_traded_aapl(c, s); c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).map_err(|e| e.to_string()) }).await.unwrap();
    let generation = "SELECT restatement_generation FROM bots";
    let pending = "SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%splits%'";
    let at = |t: &str| FixedClock(t.parse().unwrap());
    let empty = || venue(json!({ ACTIVITIES: [ok(json!([]))] })).1;
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([{ "id": "later", "activity_type": "SPLIT", "symbol": "AAPL", "qty": "5", "date": "2026-10-05" }]))] }));
    assert!(ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap().splits[0].effective_later);
    assert_eq!((one::<i64>(&db, generation).await, one::<i64>(&db, pending).await), (1, 1), "the import's own bump, and the one owed at the date");
    ledger::sync(&db, &empty(), s.api_key_id, &paper(), &at("2026-10-04T23:59:59Z")).await.unwrap().unwrap();
    assert_eq!((one::<i64>(&db, generation).await, one::<i64>(&db, pending).await), (1, 1), "nothing before the date");
    // A restart: the process and its connection are gone; what is owed is in the database.
    drop(db);
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    deltabadger::store::configure(&c).unwrap();
    let db = Db::new(c, Arc::new(seed::cipher()));
    ledger::sync(&db, &empty(), s.api_key_id, &paper(), &at("2026-10-05T00:00:00Z")).await.unwrap().unwrap();
    assert_eq!((one::<i64>(&db, generation).await, one::<i64>(&db, pending).await), (2, 0), "bumped at the first sync on its date, and no longer owed");
    ledger::sync(&db, &empty(), s.api_key_id, &paper(), &at("2026-10-06T00:00:00Z")).await.unwrap().unwrap();
    assert_eq!(one::<i64>(&db, generation).await, 2, "once");
}

/// The deferred bump (Rails' Bot::ExpireRestatedMetricsJob) is one guarded write unit through `Db`: a refusal rolls
/// every counter of the unit back, not only the bot the guard named.
#[tokio::test(flavor = "current_thread")]
async fn expire_restated_is_one_guarded_unit_and_a_refusal_rolls_all_of_it_back() {
    let (_dir, db, s) = install();
    let (working, stopped) = db.run(move |c, _| {
        let working = bot_that_traded_aapl(c, s);
        let stopped = seed::insert_bot(c, &s.seeded(), &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [stopped]).map_err(|e| e.to_string())?;
        c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, base, quote, amount, amount_exec, bot_interval, \
                   bot_quote_amount, transaction_type, error_messages, created_at, updated_at) VALUES (?1, ?2, 'o-2', 0, 2, 0, 0, 'AAPL', 'USD', 10, 10, 'week', 60, 'REGULAR', '[]', \
                   '2026-09-02 14:30:00', '2026-09-02 14:30:00')", [stopped, s.exchange_id]).map_err(|e| e.to_string())?;
        Ok((working, stopped))
    }).await.unwrap();
    let generations = "SELECT group_concat(id || ':' || coalesce(restatement_generation, 0), ' ') FROM (SELECT * FROM bots ORDER BY id)";
    let SyncError(refused) = ledger::expire_restated(&db, s.user_id, s.exchange_id, "AAPL").await.unwrap_err();
    assert!(refused.starts_with(GUARD_REFUSED) && refused.contains("restated prices"), "{refused}");
    assert_eq!(one::<String>(&db, generations).await, format!("{working}:0 {stopped}:0"), "the stopped bot's counter rolled back with the working bot's");
    db.run(move |c, _| c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [working]).map_err(|e| e.to_string())).await.unwrap();
    assert_eq!(ledger::expire_restated(&db, s.user_id, s.exchange_id, "AAPL").await.unwrap(), vec![working, stopped]);
    assert_eq!(one::<String>(&db, generations).await, format!("{working}:1 {stopped}:1"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_live_key_is_refused_and_nothing_is_sent() {
    let (_dir, db, s) = install();
    let live = Credentials { passphrase: Some("live".into()), ..paper() };
    let (t, v) = venue(json!({}));
    let failure = ledger::sync(&db, &v, s.api_key_id, &live, &clock()).await.unwrap().unwrap_err();
    assert_eq!((failure.error.as_str(), failure.raised), (LIVE_REFUSED, true));
    let failure = balances::sync(&db, &v, &NoPrices, s.api_key_id, &live, &clock()).await.unwrap().unwrap_err();
    assert_eq!(failure.error, LIVE_REFUSED);
    assert!(t.requests().is_empty(), "no request for a live key");
    assert_eq!(one::<String>(&db, "SELECT last_sync_error || ' ' || status || ' ' || coalesce(last_synced_at, 'never') FROM api_keys").await, format!("{LIVE_REFUSED} 1 never"));
    assert_eq!(one::<i64>(&db, "SELECT (SELECT count(*) FROM account_transactions) + (SELECT count(*) FROM account_balances)").await, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn keys_are_chosen_and_read_as_rails_chooses_and_reads_them() {
    let (_dir, db, s) = install();
    let (reading, credentials) = db.run(move |c, cipher| Ok((sync::reading_keys(c).map_err(|e| e.0)?, sync::credentials(c, cipher, s.api_key_id).map_err(|e| e.0)?))).await.unwrap();
    assert_eq!((reading, credentials), (vec![s.api_key_id], paper()));

    // A read-only key beside the trading key changes nothing; alone (the trading key marked incorrect) it is the one read.
    let (both, alone, not_alpaca) = db.run(move |c, _| {
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k', 's', 1, 2, '2026-01-01', '2026-01-01')",
                  [s.user_id, s.exchange_id]).map_err(|e| e.to_string())?;
        let read_only = c.last_insert_rowid();
        let both = sync::reading_keys(c).map_err(|e| e.0)?;
        c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [s.api_key_id]).map_err(|e| e.to_string())?;
        let alone = sync::reading_keys(c).map_err(|e| e.0)? == vec![read_only];
        c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.25', '0.4', '2026-01-01', '2026-01-01')", []).map_err(|e| e.to_string())?;
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k', 's', 1, 0, '2026-01-01', '2026-01-01')",
                  [s.user_id, c.last_insert_rowid()]).map_err(|e| e.to_string())?;
        Ok((both, alone, sync::load_key(c, c.last_insert_rowid()).unwrap_err().0))
    }).await.unwrap();
    assert_eq!((both, alone), (vec![s.api_key_id], true));
    assert!(not_alpaca.contains("not an Alpaca key"), "{not_alpaca}");
}

#[tokio::test(flavor = "current_thread")]
async fn the_watermark_never_passes_the_start_of_the_sync_and_one_left_in_the_future_is_brought_back() {
    let (_dir, db, s) = install();
    // What Rails leaves behind a split dated ahead: a watermark in the future.
    db.run(|c, _| c.execute("UPDATE api_keys SET last_synced_at = '2026-10-05 00:00:00'", []).map_err(|e| e.to_string())).await.unwrap();
    let (t, v) = venue(json!({ ACTIVITIES: [ok(json!([]))] }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!(t.requests()[0].query.iter().find(|(k, _)| *k == "after").map(|(_, v)| v.as_str()), Some("2026-09-19T01:00:00Z"), "read from now less 25 h, not from the future");
    assert_eq!((out.watermark, one::<String>(&db, "SELECT last_synced_at FROM api_keys").await), (Some(NOW.parse().unwrap()), "2026-09-20 02:00:00".to_string()));
    // A row dated ahead is stored as it is; the watermark stays at the sync's start.
    let (_, v) = venue(json!({ ACTIVITIES: [ok(json!([interest("ahead", "2026-12-01"), interest("behind", "2026-09-10")]))] }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!((out.imported, out.watermark), (2, Some(NOW.parse().unwrap())));
    assert_eq!(one::<String>(&db, "SELECT max(transacted_at) FROM account_transactions").await, "2026-12-01 00:00:00");
}

#[derive(Clone)]
struct Scripted(ScriptedTransport);
impl Connect for Scripted {
    type T = ScriptedTransport;
    fn connect(&self, _: &Credentials) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(self.0.clone(), Urls::for_passphrase(None)) }
}
fn cx<'a>(db: &Db, clock: &'a dyn Clock) -> Cx<'a> { Cx { db: db.clone(), clock } }

#[test]
fn each_reading_key_gets_a_ledger_job_and_a_balance_job_scoped_by_its_id() {
    let (dir, _db, s) = install();
    let venues = Scripted(ScriptedTransport::default());
    let ledger = LedgerSync::new(venues.clone(), 7).spec();
    assert_eq!((ledger.name, ledger.scope.as_deref(), ledger.schedule, ledger.jitter, ledger.retry, ledger.deadline),
               ("ledger_sync", Some("7"), Some(Schedule::Daily { hour: 2, minute: 0 }), Jitter::NONE, Retry::None, std::time::Duration::from_secs(3600)));
    let balance = BalanceSync::new(venues.clone(), Rc::new(NoPrices), 7).spec();
    assert_eq!((balance.name, balance.scope.as_deref(), balance.schedule, balance.jitter, balance.retry, balance.deadline),
               ("balance_sync", Some("7"), Some(Schedule::Daily { hour: 2, minute: 30 }), Jitter::NONE, Retry::None, DEADLINE));
    // The ledger job runs on every order the engine records; nothing else runs on an engine event.
    let order = EngineEvent::OrderRecorded { bot_id: 1, transaction_id: 1 };
    let funds = EngineEvent::FundsLow { bot_id: 1, user_id: 1, quote_asset_id: None };
    assert!(LedgerSync::new(venues.clone(), 7).wants(&order) && !LedgerSync::new(venues.clone(), 7).wants(&funds));
    assert!(!BalanceSync::new(venues.clone(), Rc::new(NoPrices), 7).wants(&order));

    // What the scheduler registers at start (on the connection it is about to hand its `Db`): per reading key, the ledger jobs first.
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let names = |c: &Connection| jobs::register(c, &venues, Rc::new(NoPrices)).unwrap().iter().map(|j| (j.spec().name, j.spec().scope)).collect::<Vec<_>>();
    let key = Some(s.api_key_id.to_string());
    assert_eq!(names(&c), vec![("ledger_sync", key.clone()), ("balance_sync", key)], "the record is rust_job.ledger_sync:<api_key_id>");
    c.execute("UPDATE api_keys SET status = 2", []).unwrap();
    assert!(names(&c).is_empty(), "a key Rails marked incorrect is not a reading key");
}

#[tokio::test(flavor = "current_thread")]
async fn a_run_reports_done_even_with_nothing_to_import_and_failed_with_the_keys_error() {
    let (_dir, db, s) = install();
    let clock = clock();
    // A first success on an empty account: Done, so the job record can tell it from "never ran"; the key itself is untouched.
    let t = ScriptedTransport::from_script(&json!({ ACTIVITIES: [ok(json!([]))] }));
    assert_eq!(LedgerSync::new(Scripted(t.clone()), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Done);
    assert_eq!((t.requests().len(), one::<Option<String>>(&db, "SELECT last_synced_at FROM api_keys").await), (1, None));

    let t = ScriptedTransport::from_script(&json!({ ACTIVITIES: [{ "status": 401, "body": { "code": 40_110_000, "message": "unauthorized." } }] }));
    assert_eq!(LedgerSync::new(Scripted(t), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Failed("unauthorized.".into()));
    assert_eq!(one::<String>(&db, "SELECT last_sync_error || ' ' || status FROM api_keys").await, "unauthorized. 1", "the ledger job never condemns a key");

    let t = ScriptedTransport::from_script(&balances_script());
    assert_eq!(BalanceSync::new(Scripted(t), Rc::new(NoPrices), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Done);
    assert_eq!(one::<String>(&db, "SELECT count(*) || ' ' || sum(usd_value IS NULL) FROM account_balances").await, "3 1", "the coin has no price without a market source");
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/account": [{ "status": 401, "body": { "message": "unauthorized." } }] }));
    assert_eq!(BalanceSync::new(Scripted(t.clone()), Rc::new(NoPrices), s.api_key_id).run(cx(&db, &clock), vec![Wake::Manual(None)]).await, Outcome::Failed("unauthorized.".into()));
    assert_eq!(one::<i64>(&db, "SELECT status FROM api_keys").await, 2, "a balance read Alpaca calls unauthorized marks the key incorrect, as Rails does");
    // The job of a key that stopped being a reading key fails without a request (Rails' job skips it; a job has no "skipped").
    let before = t.requests().len();
    let outcome = BalanceSync::new(Scripted(t.clone()), Rc::new(NoPrices), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await;
    assert_eq!((outcome, t.requests().len()), (Outcome::Failed(format!("api key {} is not a reading key", s.api_key_id)), before));
    // A key this instance cannot decrypt: the error names no secret.
    db.run(|c, _| { let other = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-instance").map_err(|e| format!("{e:?}"))?);
                    c.execute("UPDATE api_keys SET secret = ?1", [other.encrypt("s3cr3t")]).map_err(|e| e.to_string()) }).await.unwrap();
    let Outcome::Failed(text) = LedgerSync::new(Scripted(t), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await else { panic!("an unreadable key must fail the run") };
    assert!(text.contains("unreadable") && !text.contains("s3cr3t"), "{text}");
}

#[tokio::test(flavor = "current_thread")]
async fn any_number_of_orders_wake_one_sync_and_a_replay_stores_nothing_more() {
    let (_dir, db, s) = install();
    let clock = clock();
    let order = |bot_id: i64, transaction_id: i64| Wake::Event(EngineEvent::OrderRecorded { bot_id, transaction_id });
    let t = ScriptedTransport::from_script(&json!({ ACTIVITIES: [ok(json!([interest("i1", "2026-09-01")]))] }));
    let job = LedgerSync::new(Scripted(t.clone()), s.api_key_id);
    assert_eq!(job.run(cx(&db, &clock), vec![order(1, 1), order(1, 2), order(2, 3), Wake::Manual(None)]).await, Outcome::Done);
    assert_eq!((t.requests().len(), one::<i64>(&db, "SELECT count(*) FROM account_transactions").await), (1, 1), "four wakes, one sync");
    // Idempotent: the same wakes replayed (a retry, a run dropped and started again) store nothing more.
    assert_eq!(job.run(cx(&db, &clock), vec![order(1, 1)]).await, Outcome::Done);
    assert_eq!(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, 1);
    assert_eq!(t.requests()[1].query.iter().find(|(k, _)| *k == "after").map(|(_, v)| v.as_str()), Some("2026-08-30T23:00:00Z"), "from the watermark less 25 h");
}

/// A venue that never answers its second request.
#[derive(Clone)]
struct Hanging(ScriptedTransport);
impl Transport for Hanging {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if self.0.requests().len() == 1 { std::future::pending::<()>().await; }
        self.0.send(r).await
    }
}
struct HangingVenues(ScriptedTransport);
impl Connect for HangingVenues {
    type T = Hanging;
    fn connect(&self, _: &Credentials) -> AlpacaVenue<Hanging> { AlpacaVenue::new(Hanging(self.0.clone()), Urls::for_passphrase(None)) }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_run_dropped_at_its_deadline_leaves_nothing_half_written() {
    let (_dir, db, s) = install();
    let clock = clock();
    let pages = json!({ ACTIVITIES: [ok(Value::Array((0..100).map(|i| interest(&format!("i{i:03}"), "2026-09-01")).collect())), ok(json!([interest("last", "2026-09-02")]))] });
    let t = ScriptedTransport::from_script(&pages);
    let job = LedgerSync::new(HangingVenues(t.clone()), s.api_key_id);
    // As the runner does (2f S-2, R1): the run is dropped at its next await once its deadline has passed.
    assert!(tokio::time::timeout(job.spec().deadline, job.run(cx(&db, &clock), vec![Wake::Schedule])).await.is_err(), "the run never ends on its own");
    assert_eq!(one::<String>(&db, "SELECT (SELECT count(*) FROM account_transactions) || ' ' || coalesce(last_synced_at, 'never') || ' ' || coalesce(last_sync_error, 'none') FROM api_keys").await,
               "0 never none", "the first page is not stored on its own, and the key is as it was");
    // The next run starts over and stores everything.
    let t = ScriptedTransport::from_script(&pages);
    assert_eq!(LedgerSync::new(Scripted(t), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Done);
    assert_eq!(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, 101);
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_of_the_ledger_past_any_limit_fails_the_run_and_moves_nothing() {
    let (_dir, db, s) = install();
    let untouched = "SELECT (SELECT count(*) FROM account_transactions) || ' rows, watermark ' || coalesce(last_synced_at, 'none') FROM api_keys";
    let item = |id: String| interest(&id, "2026-09-01");
    let page = |ids: std::ops::Range<usize>, last: &str| { let mut p: Vec<Value> = ids.map(|i| item(format!("i{i}"))).collect(); p.push(item(last.into())); ok(Value::Array(p)) };
    let text = |body: String| json!({ "status": 200, "body": body }); // a body as the venue's own bytes
    let repeat = "the ledger's page tokens repeat: nothing was read".to_string();
    let cases: Vec<(&str, Value, String)> = vec![
        ("a page of more items than were asked for", json!({ ACTIVITIES: [ok(Value::Array((0..101).map(|i| item(format!("i{i}"))).collect()))] }),
         "an activities page with more items than were asked for".into()),
        ("a page over the byte limit", json!({ ACTIVITIES: [ok(json!([{ "id": "big", "activity_type": "INT", "net_amount": "1", "date": "2026-09-01", "note": "x".repeat(ledger::MAX_PAGE_BYTES) }]))] }),
         format!("Client::TransientNetworkError: the response body is over {} bytes", ledger::MAX_PAGE_BYTES)),
        ("a page inside the byte limit with more keys than a page may hold",
         json!({ ACTIVITIES: [text(format!("[{{\"id\":\"wide\",{}}}]", (0..12_000).map(|i| format!("\"k{i}\":0")).collect::<Vec<_>>().join(",")))] }),
         "an activities page with more values than one answer may hold".into()),
        ("a page inside the byte limit with one long list",
         json!({ ACTIVITIES: [text(format!("[{{\"id\":\"long\",\"list\":[{}]}}]", vec!["0"; 100_000].join(",")))] }),
         "an activities page with more values than one answer may hold".into()),
        ("page tokens that come round again", json!({ ACTIVITIES: [page(0..99, "token-a"), page(100..199, "token-b"), page(200..299, "token-a")] }), repeat.clone()),
        ("a page token that does not move (Rails reads this one as the end of the ledger)", json!({ ACTIVITIES: [page(0..99, "token-a"), page(100..199, "token-a")] }), repeat.clone()),
        ("a short last page that ends on a token already used", json!({ ACTIVITIES: [page(0..99, "token-a"), ok(json!([item("token-a".into())]))] }), repeat.clone()),
        ("a full page whose last activity has no id", json!({ ACTIVITIES: [ok(Value::Array((0..100).map(|i| if i == 99 { json!({ "activity_type": "INT", "net_amount": "1", "date": "2026-09-01" }) } else { item(format!("i{i}")) }).collect()))] }),
         "a full page of the ledger ends with an activity that has no id: nothing was read".into()),
        ("a quantity that underflows a double, unquoted",
         json!({ ACTIVITIES: [text(r#"[{"id":"f","activity_type":"FILL","symbol":"AAPL","side":"buy","qty":1e-999,"price":"1","transaction_time":"2026-09-10T14:30:00Z"}]"#.into())] }),
         "unreadable qty: beyond 10^±40".into()),
    ];
    for (what, script, error) in cases {
        let (_, v) = venue(script);
        let failure = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap_err();
        assert_eq!((failure.error.as_str(), failure.raised), (error.as_str(), true), "{what}");
        assert_eq!((one::<String>(&db, untouched).await, one::<String>(&db, "SELECT last_sync_error FROM api_keys").await), ("0 rows, watermark none".to_string(), error), "{what}: nothing stored, and the key says why");
    }
    // The job's outcome is what the runner records.
    let clock = clock();
    let job = LedgerSync::new(Scripted(ScriptedTransport::from_script(&json!({ ACTIVITIES: [page(0..99, "token-a"), page(100..199, "token-a")] }))), s.api_key_id);
    assert_eq!(job.run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Failed(repeat));
}

/// A ledger served as Alpaca serves it: ascending, 100 a page, `page_token` the id after which the page starts.
#[derive(Clone)]
struct Paged { activities: Rc<Vec<Value>>, requests: Rc<RefCell<Vec<Query>>>, fail_next: Rc<Cell<bool>> }
type Query = Vec<(String, String)>;
impl Paged {
    fn new(activities: Vec<Value>) -> Self { Self { activities: Rc::new(activities), requests: Rc::default(), fail_next: Rc::default() } }
    fn venue(&self) -> AlpacaVenue<Paged> { AlpacaVenue::new(self.clone(), Urls::for_passphrase(None)) }
    /// (page_token, after) of every request so far.
    fn asked(&self) -> Vec<(Option<String>, Option<String>)> {
        let of = |q: &Query, k: &str| q.iter().find(|(name, _)| name == k).map(|(_, v)| v.clone());
        self.requests.borrow().iter().map(|q| (of(q, "page_token"), of(q, "after"))).collect()
    }
}
impl Transport for Paged {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.borrow_mut().push(r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        if self.fail_next.replace(false) { return Err(TransportError::NotSent("connection refused".into())); }
        let token = r.query.iter().find(|(k, _)| *k == "page_token").map(|(_, v)| v.as_str());
        let from = token.map_or(0, |t| self.activities.iter().position(|a| a["id"] == t).map_or(self.activities.len(), |i| i + 1));
        Ok(HttpResponse { status: 200, body: Value::Array(self.activities.iter().skip(from).take(100).cloned().collect()).to_string() })
    }
}
#[derive(Clone)]
struct PagedVenues(Paged);
impl Connect for PagedVenues {
    type T = Paged;
    fn connect(&self, _: &Credentials) -> AlpacaVenue<Paged> { self.0.venue() }
}

/// Codex round 2, finding 3: a history longer than one run reads is imported over several runs. A run stops at its
/// page cap, stores what it read and records where it stopped; the watermark and the job's success wait for the end.
#[tokio::test(flavor = "current_thread")]
async fn an_import_longer_than_one_run_continues_where_it_stopped_and_ends_as_one_read_would() {
    // 1,050 activities a minute apart; a three-leg split lies across the end of the first run's third page. Rails merges
    // split legs that are consecutive in one read, so a run does not cut such a group: its last legs wait for the next run.
    let start: chrono::DateTime<chrono::Utc> = "2026-03-01T00:00:00Z".parse().unwrap();
    let leg = |id: &str, qty: &str| json!({ "id": id, "activity_type": "SPLIT", "symbol": "AAPL", "qty": qty, "date": "2026-03-01" });
    let history: Vec<Value> = (0..1_050i64).map(|i| match i {
        298 => leg("cut-remove", "-10"),
        299 => leg("cut-add-1", "15"),
        300 => leg("cut-add-2", "15"),
        _ => json!({ "id": format!("a-{i:04}"), "activity_type": "INT", "net_amount": "0.07", "transaction_time": (start + chrono::Duration::minutes(i)).to_rfc3339() }),
    }).collect();
    let rows = "SELECT count(*) || ' rows; ' || group_concat(tx_id || ' ' || base_amount || ' ' || transacted_at || ' ' || coalesce(description, '-') || ' ' || raw_data, '|') FROM (SELECT * FROM account_transactions ORDER BY id)";
    let key = "SELECT coalesce(last_synced_at, 'none') || ', error ' || coalesce(last_sync_error, 'none') || ', ' || (SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%') || ' import row' FROM api_keys";
    let three_pages = ledger::Limits { pages: 3, runs: 100 };
    let stopped_bot = |c: &Connection, s: Ids| { let bot = bot_that_traded_aapl(c, s); c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).unwrap(); };

    // One read of the whole ledger, for comparison.
    let (whole_rows, whole_key) = {
        let (_dir, db, s) = install();
        db.run(move |c, _| { stopped_bot(c, s); Ok(()) }).await.unwrap();
        let out = ledger::sync(&db, &Paged::new(history.clone()).venue(), s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
        assert_eq!((out.complete, out.imported), (true, 1_048));
        (one::<String>(&db, rows).await, one::<String>(&db, key).await)
    };
    assert!(whole_rows.starts_with("1048 rows; ") && whole_rows.contains(r#""merged_activity_ids":["cut-remove","cut-add-1","cut-add-2"],"split_ratio":"3:1""#));
    assert_eq!(whole_key, "2026-03-01 17:29:00, error none, 0 import row", "the watermark is the newest activity");

    // The same ledger, three pages a run.
    let (dir, db, s) = install();
    db.run(move |c, _| { stopped_bot(c, s); Ok(()) }).await.unwrap();
    let server = Paged::new(history.clone());
    let clock = clock();
    let job = LedgerSync::new(PagedVenues(server.clone()), s.api_key_id).within(three_pages);
    for (run, stored) in [(1, 298), (2, 596), (3, 896)] {
        assert_eq!(job.run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::NothingNew, "run {run}: not yet whole, so no success for Plan 2d to read");
        assert_eq!(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, stored, "run {run}");
        assert_eq!(one::<String>(&db, key).await, "none, error none, 1 import row", "run {run}: the watermark has not moved");
    }
    assert_eq!(one::<String>(&db, "SELECT json_extract(value, '$.cursor') || ' ' || json_extract(value, '$.stored') FROM app_configs WHERE key = 'rust_sync.ledger:1'").await, "a-0897 896",
               "plain JSON under the key's own name: where the import stopped");
    // The run that reaches the end writes the watermark, removes its record, and is the job's first success.
    assert_eq!(job.run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Done);
    assert_eq!((one::<String>(&db, rows).await, one::<String>(&db, key).await), (whole_rows.clone(), whole_key.clone()), "the rows and the watermark one read stores");
    let asked = server.asked();
    assert_eq!(asked.iter().map(|(token, _)| token.clone()).collect::<Vec<_>>(),
               [None, Some("a-0099".into()), Some("a-0199".into()), Some("a-0297".into()), Some("a-0397".into()), Some("a-0497".into()), Some("a-0597".into()), Some("a-0697".into()),
                Some("a-0797".into()), Some("a-0897".into()), Some("a-0997".into())], "each run goes on from the last id the run before it stored: the first from before the split's legs");
    assert!(asked.iter().all(|(_, after)| after.is_none()), "a first import asks for the whole history on every run");
    assert!(write_lock_is_free(dir.path()));
    // And a run after the import is an ordinary incremental one (with the jobs' own cap: this ledger's last 25 hours
    // are more than three pages).
    let before = server.asked().len();
    assert_eq!(LedgerSync::new(PagedVenues(server.clone()), s.api_key_id).run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Done);
    assert_eq!(server.asked()[before], (None, Some("2026-02-28T16:29:00Z".to_string())), "from the watermark less 25 h");
    assert_eq!((one::<String>(&db, rows).await, one::<String>(&db, key).await), (whole_rows.clone(), whole_key.clone()));

    // A run that fails in the middle of an import leaves the record: the next run goes on from the same place.
    let (_dir, db, s) = install();
    let server = Paged::new(history.clone());
    let venue = server.venue();
    assert!(!ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock, three_pages).await.unwrap().unwrap().complete);
    server.fail_next.set(true);
    let failure = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock, three_pages).await.unwrap().unwrap_err();
    assert!(failure.raised && failure.error.starts_with("Client::TransientNetworkError"), "{failure:?}");
    assert_eq!(one::<i64>(&db, "SELECT count(*) FROM app_configs WHERE key = 'rust_sync.ledger:1'").await, 1);
    assert!(!ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock, three_pages).await.unwrap().unwrap().complete);
    assert_eq!(server.asked()[3..5].iter().map(|(token, _)| token.clone()).collect::<Vec<_>>(), [Some("a-0297".to_string()), Some("a-0297".to_string())], "the failed request, then the same one again");
    // A run dropped between two units of a slice (a stop, a crash) has recorded nothing new: the next run reads the
    // slice again, finds its stored rows duplicates, and goes on.
    {
        let (credentials, stored) = (paper(), |dir: &Path| Connection::open(dir.join("production.sqlite3")).unwrap().query_row("SELECT count(*) FROM account_transactions", [], |r| r.get::<_, i64>(0)).unwrap());
        let (dir2, db2, s2) = install();
        let server = Paged::new(history.clone());
        let venue = server.venue();
        {
            let run = ledger::sync_within(&db2, &venue, s2.api_key_id, &credentials, &clock, three_pages);
            let first_unit = async { while stored(dir2.path()) < 100 { tokio::time::sleep(std::time::Duration::from_millis(5)).await; } };
            tokio::select! { _ = run => panic!("the run ended before it was dropped"), () = first_unit => {} }
        }
        assert_eq!(one::<String>(&db2, "SELECT json_extract(value, '$.runs') || ' run, cursor [' || json_extract(value, '$.cursor') || ']' FROM app_configs WHERE key LIKE 'rust_sync.%'").await, "1 run, cursor []",
                   "the run is counted, and no slice is recorded that was not stored whole");
        let mut runs = 0;
        while !ledger::sync_within(&db2, &venue, s2.api_key_id, &credentials, &clock, three_pages).await.unwrap().unwrap().complete { runs += 1; }
        assert_eq!((runs, one::<i64>(&db2, "SELECT count(*) FROM account_transactions").await), (3, 1_048));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_that_ends_exactly_at_the_cap_and_one_rails_overtook_both_end_well() {
    let item = |i: usize| interest(&format!("e-{i:03}"), "2026-09-01");
    let three_pages = ledger::Limits { pages: 3, runs: 100 };
    // Exactly three full pages: the run cannot know the ledger ends there, so it stops at its cap; the next run asks
    // once more, gets nothing, and finishes.
    let (_dir, db, s) = install();
    let server = Paged::new((0..300).map(item).collect());
    let venue = server.venue();
    let first = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), three_pages).await.unwrap().unwrap();
    assert_eq!((first.complete, first.imported, first.watermark), (false, 300, None));
    assert_eq!(one::<String>(&db, "SELECT coalesce(last_synced_at, 'none') FROM api_keys").await, "none");
    let second = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), three_pages).await.unwrap().unwrap();
    assert_eq!((second.complete, second.imported, second.duplicates), (true, 0, 0));
    assert_eq!(server.asked().last(), Some(&(Some("e-299".to_string()), None)), "one request, from the last id");
    assert_eq!(one::<String>(&db, "SELECT last_synced_at || ' ' || (SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%') FROM api_keys").await, "2026-09-01 00:00:00 0");
    assert_eq!((ledger::MAX_PAGES, ledger::MAX_ACTIVITIES), (500, 50_000), "the jobs' own cap: a ledger of exactly 50,000 activities takes two runs");

    // After a handback: Rails syncs from the public watermark (an unfinished import has not moved it), reads what Rust
    // stored as duplicates, and writes its own watermark. The record Rust left is then about another state of the
    // key: the next Rust run ignores it, starts from the watermark as any sync does, and removes it.
    let (_dir, db, s) = install();
    let server = Paged::new((0..450).map(item).collect());
    let venue = server.venue();
    assert!(!ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), three_pages).await.unwrap().unwrap().complete);
    assert_eq!(one::<String>(&db, "SELECT coalesce(last_synced_at, 'none') || ' ' || (SELECT count(*) FROM app_configs WHERE key = 'rust_sync.ledger:1') FROM api_keys").await, "none 1",
               "what Rails finds: no watermark, so it reads the whole ledger");
    db.run(|c, _| c.execute("UPDATE api_keys SET last_synced_at = '2026-09-01 00:00:00'", []).map_err(|e| e.to_string())).await.unwrap(); // Rails' own sync
    let before = server.asked().len();
    let after_rails = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), three_pages).await.unwrap().unwrap();
    assert_eq!(server.asked()[before], (None, Some("2026-08-30T23:00:00Z".to_string())), "no page token; from Rails' watermark less 25 h");
    assert_eq!((after_rails.complete, one::<i64>(&db, "SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%'").await), (false, 1), "450 activities are again more than three pages: a new import, recorded anew");
    let done = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), three_pages).await.unwrap().unwrap();
    assert_eq!((done.complete, one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, one::<i64>(&db, "SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%'").await), (true, 450, 0));

    // Codex round 4, finding 6: an ignored record goes with the first run that completes, also one that has nothing
    // to write: the venue returns nothing new, the watermark stays, and the key has no error to clear.
    let records = "SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%'";
    let (_dir, db, s) = install();
    assert!(!ledger::sync_within(&db, &Paged::new((0..450).map(item).collect()).venue(), s.api_key_id, &paper(), &clock(), three_pages).await.unwrap().unwrap().complete);
    db.run(|c, _| c.execute("UPDATE api_keys SET last_synced_at = '2026-09-01 00:00:00'", []).map_err(|e| e.to_string())).await.unwrap(); // Rails' own sync
    let before = one::<String>(&db, "SELECT last_synced_at || ' ' || updated_at || ' ' || coalesce(last_sync_error, 'none') FROM api_keys").await;
    let (_, empty) = crate::venue(json!({ ACTIVITIES: [ok(json!([]))] }));
    let idle = ledger::sync(&db, &empty, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!((idle.complete, idle.imported, one::<i64>(&db, records).await), (true, 0, 0), "an idle run still removes the record it ignored");
    assert_eq!(one::<String>(&db, "SELECT last_synced_at || ' ' || updated_at || ' ' || coalesce(last_sync_error, 'none') FROM api_keys").await, before, "and writes nothing to the key");
    // A row under the import's name that is not this port's JSON is ignored and removed the same way; a run that
    // fails leaves it for the run that completes.
    db.run(|c, _| c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('rust_sync.ledger:1', 'not json', '2026-09-01', '2026-09-01')", []).map_err(|e| e.to_string())).await.unwrap();
    let (_, failing) = crate::venue(json!({ ACTIVITIES: [json!({ "status": 500, "body": { "message": "internal server error" } })] }));
    assert!(ledger::sync(&db, &failing, s.api_key_id, &paper(), &clock()).await.unwrap().is_err());
    assert_eq!(one::<i64>(&db, records).await, 1);
    assert!(ledger::sync(&db, &empty, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap().complete);
    assert_eq!(one::<i64>(&db, records).await, 0);
}

/// The ledger, the key and the bots' counters: what an import over several runs must leave as one read leaves them.
async fn ledger_state(db: &Db) -> String {
    one::<String>(db, "SELECT (SELECT count(*) || ' rows; ' || coalesce(group_concat(coalesce(tx_id, '-') || ' ' || base_amount || ' ' || transacted_at || ' ' || coalesce(description, '-') || ' ' || raw_data, '|'), '') \
                       FROM (SELECT * FROM account_transactions ORDER BY id)) || ' / watermark ' || coalesce(last_synced_at, 'none') || ', error ' || coalesce(last_sync_error, 'none') || ', ' || \
                       (SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%') || ' import row, counters ' || (SELECT coalesce(group_concat(restatement_generation), '') FROM bots) FROM api_keys").await
}

/// Imports `history` in one read, and again at `pages` pages a run on a second install (a stopped bot that traded AAPL
/// on both). Returns the state one read leaves, the state the runs leave, the rows after the first run, and what each
/// run asked for.
async fn one_read_and_capped(history: &[Value], pages: usize) -> (String, String, i64, Vec<Vec<Option<String>>>) {
    let stopped_bot = |c: &Connection, s: Ids| { let bot = bot_that_traded_aapl(c, s); c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [bot]).unwrap(); };
    let (_dir, db, s) = install();
    db.run(move |c, _| { stopped_bot(c, s); Ok(()) }).await.unwrap();
    assert!(ledger::sync(&db, &Paged::new(history.to_vec()).venue(), s.api_key_id, &paper(), &clock()).await.unwrap().unwrap().complete);
    let whole = ledger_state(&db).await;

    let (_dir, db, s) = install();
    db.run(move |c, _| { stopped_bot(c, s); Ok(()) }).await.unwrap();
    let server = Paged::new(history.to_vec());
    let venue = server.venue();
    let (mut asked, mut first_run_rows) = (vec![], None);
    loop {
        let from = server.asked().len();
        let out = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), ledger::Limits { pages, runs: 100 }).await.unwrap().unwrap();
        asked.push(server.asked()[from..].iter().map(|(token, _)| token.clone()).collect());
        first_run_rows.get_or_insert(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await);
        if out.complete { break; }
        assert!(asked.len() < 20, "the import does not end");
    }
    (whole, ledger_state(&db).await, first_run_rows.unwrap_or_default(), asked)
}

/// Codex round 4, findings 1 and 2: a run that stops at its cap stops where Rails' grouping allows. Rails merges split
/// legs that are neighbours after it has dropped the cancelled activities, so the stop is decided on that sequence;
/// only the group the run ends in waits for the next run; and a run that holds nothing but one group reads on to its end.
#[tokio::test(flavor = "current_thread")]
async fn a_capped_run_never_parts_what_rails_would_merge_and_always_moves_on() {
    let ints = |prefix: &str, n: usize| -> Vec<Value> { (0..n).map(|i| interest(&format!("{prefix}-{i:03}"), "2026-09-01")).collect() };
    let leg = |id: &str, symbol: &str, qty: &str, date: &str| json!({ "id": id, "activity_type": "SPLIT", "symbol": symbol, "qty": qty, "date": date });
    let cancelled = |id: &str, kind: &str| json!({ "id": id, "activity_type": kind, "symbol": "AAPL", "qty": "999", "net_amount": "1", "date": "2026-09-15", "status": "canceled" });
    let cancelled_fill = json!({ "id": "c-fill", "activity_type": "FILL", "symbol": "AAPL", "side": "buy", "qty": "1", "price": "1", "transaction_time": "2026-09-15T14:30:00Z", "status": "canceled" });
    let day = "2026-09-15";
    // The cap falls after the hundredth activity. `before` INTs, then the activities around it, then five more INTs.
    let around = |before: usize, middle: Vec<Value>| -> Vec<Value> { ints("i", before).into_iter().chain(middle).chain(ints("z", 5)).collect() };
    let merged = r#""merged_activity_ids":["s-remove","s-add"],"split_ratio":"2:1""#;

    // Finding 1, as Codex gave it: the run ends `SPLIT -10`, a cancelled INT; the next begins `SPLIT +20`.
    let (whole, capped, first_run, asked) = one_read_and_capped(&around(98, vec![leg("s-remove", "AAPL", "-10", day), cancelled("c-1", "INT"), leg("s-add", "AAPL", "20", day)]), 1).await;
    assert_eq!(capped, whole);
    assert!(whole.starts_with("104 rows; ") && whole.contains("s-remove 10 2026-09-15 00:00:00 Split (AAPL) 2:1 ") && whole.contains(merged) && whole.ends_with("counters 1"), "one row of +10, 2:1, one bump: {whole}");
    assert_eq!((first_run, &asked[1][0]), (98, &Some("i-097".to_string())), "the first run leaves the leg and the cancelled activity after it; the second starts before the leg");

    // Its neighbours. Each must end as one read ends; what one read stores is said beside it.
    let cases: Vec<(&str, usize, Vec<Value>, &str)> = vec![
        ("other dropped kinds between the legs, on both sides of the cap", 97,
         vec![leg("s-remove", "AAPL", "-10", day), cancelled("c-1", "DIV"), cancelled("c-2", "ZZZ"), cancelled("c-3", "CSD"), leg("s-add", "AAPL", "20", day)], merged),
        ("a cancelled split leg between the legs: dropped, so it is no leg", 98,
         vec![leg("s-remove", "AAPL", "-10", day), cancelled("c-1", "SPLIT"), leg("s-add", "AAPL", "20", day)], merged),
        ("the cap between two legs with nothing between them", 99, vec![leg("s-remove", "AAPL", "-10", day), leg("s-add", "AAPL", "20", day)], merged),
        ("three legs, cancelled activities among them, the cap after the second", 96,
         vec![leg("s-remove", "AAPL", "-10", day), cancelled("c-1", "INT"), leg("s-add", "AAPL", "15", day), cancelled("c-2", "FEE"), leg("s-add-2", "AAPL", "15", day)],
         r#""merged_activity_ids":["s-remove","s-add","s-add-2"],"split_ratio":"3:1""#),
        ("a cancelled FILL between the legs: a trade is never dropped, so Rails does not merge them either", 98,
         vec![leg("s-remove", "AAPL", "-10", day), cancelled_fill.clone(), leg("s-add", "AAPL", "20", day)], "s-remove -10 2026-09-15 00:00:00 Split (AAPL) "),
        ("another symbol's leg between them", 98, vec![leg("s-remove", "AAPL", "-10", day), leg("k-remove", "KLAC", "-5", day), leg("s-add", "AAPL", "20", day)], "s-add 20 2026-09-15 00:00:00 Split (AAPL) "),
        ("the same symbol on the next date", 99, vec![leg("s-remove", "AAPL", "-10", day), leg("s-add", "AAPL", "20", "2026-09-16")], "s-add 20 2026-09-16 00:00:00 Split (AAPL) "),
        ("a cancelled activity as the last of the run, after an ordinary one", 99, vec![cancelled("c-1", "INT"), leg("s-remove", "AAPL", "-10", day), leg("s-add", "AAPL", "20", day)], merged),
    ];
    for (what, before, middle, stored) in cases {
        let (whole, capped, _, _) = one_read_and_capped(&around(before, middle), 1).await;
        assert_eq!(capped, whole, "{what}");
        assert!(whole.contains(stored), "{what}: {whole}");
    }

    // Finding 2: a full page of complete splits of different symbols. Only the last pair waits; the run stores the rest.
    let pairs: Vec<Value> = (0..50).flat_map(|i| [leg(&format!("p{i:02}-remove"), &format!("S{i:02}"), "-1", day), leg(&format!("p{i:02}-add"), &format!("S{i:02}"), "2", day)]).collect();
    let (whole, capped, first_run, asked) = one_read_and_capped(&pairs.iter().cloned().chain(ints("z", 30)).collect::<Vec<_>>(), 1).await;
    assert_eq!(capped, whole);
    assert!(whole.starts_with("80 rows; "), "{whole}");
    assert_eq!((first_run, asked.len(), &asked[1][0]), (49, 2, &Some("p48-add".to_string())), "49 splits stored by the first run; the fiftieth read again by the second");
    // The same when the whole ledger is such pages, and it ends on one.
    let (whole, capped, _, asked) = one_read_and_capped(&pairs.iter().cloned().chain(pairs.iter().map(|l| { let mut l = l.clone(); l["id"] = json!(format!("q{}", l["id"].as_str().unwrap())); l["date"] = json!("2026-09-16"); l })).collect::<Vec<_>>(), 1).await;
    assert_eq!(capped, whole);
    assert!(whole.starts_with("100 rows; ") && asked.len() == 3, "{} runs: {whole}", asked.len());

    // One split larger than a run: the run that holds nothing else reads on to its end, and stops after it.
    let long: Vec<Value> = std::iter::once(leg("l-remove", "AAPL", "-100", day)).chain((1..120).map(|i| leg(&format!("l-add-{i:03}"), "AAPL", "1", day))).collect();
    let (whole, capped, first_run, asked) = one_read_and_capped(&ints("i", 50).into_iter().chain(long).chain(ints("z", 80)).collect::<Vec<_>>(), 1).await;
    assert_eq!(capped, whole);
    assert!(whole.starts_with("131 rows; ") && whole.contains("l-remove 19 2026-09-15 00:00:00 Split (AAPL) 25:21 ") && whole.ends_with("counters 1"), "{}", whole.split('|').filter(|row| row.contains("l-remove") || row.contains("counters")).map(|row| row.chars().take(300).collect::<String>()).collect::<Vec<_>>().join(" ... "));
    assert_eq!((first_run, asked.iter().map(Vec::len).collect::<Vec<_>>()), (50, vec![1, 2, 1]), "the second run reads a second page for the sake of the split; the third finds the end");
    assert_eq!(asked[1], [Some("i-049".to_string()), Some("l-add-099".to_string())]);
}

/// Codex rounds 4 and 5: a run reads past its pages only to finish the one split it holds nothing but, and within one
/// bound: five more pages at most, whatever they hold, and the split at most 500 legs, checked on every page it
/// appends. Past it the run fails; every run of an import is counted when it starts, the first too, so an import that
/// keeps failing is ended after its runs, not retried for ever.
#[tokio::test(flavor = "current_thread")]
async fn a_split_too_long_for_a_run_fails_it_and_the_import_is_ended_after_its_runs() {
    let too_long = "a split longer than a run of the ledger import may read on for: nothing was read";
    let legs = |n: usize| (0..n).map(|i| json!({ "id": format!("leg-{i:03}"), "activity_type": "SPLIT", "symbol": "AAPL", "qty": if i == 0 { "-1000" } else { "1" }, "date": "2026-09-15" }));
    let state = "SELECT (SELECT count(*) FROM account_transactions) || ' rows, watermark ' || coalesce(last_synced_at, 'none') || ', runs ' || \
                 coalesce((SELECT json_extract(value, '$.runs') FROM app_configs WHERE key = 'rust_sync.ledger:1'), 'no record') || ', error ' || coalesce(last_sync_error, 'none') FROM api_keys";
    assert_eq!((ledger::MAX_SPLIT_LEGS, ledger::MAX_READ_ON_PAGES), (500, 5));
    let one_page = ledger::Limits { pages: 1, runs: 4 };
    // Codex round 5's three ledgers, at one page a run, and one that never ends: each run fails within its five extra
    // pages, stores nothing, and is counted.
    let cancelled = |i: usize| json!({ "id": format!("c-{i:06}"), "activity_type": "INT", "net_amount": "1", "date": "2026-09-15", "status": "canceled" });
    let cases: Vec<(&str, Vec<Value>, usize)> = vec![
        ("550 legs of one split, and nothing after them (a short last page)", legs(550).collect(), 6),
        ("550 legs, then 50 other activities on the same page", legs(550).chain((0..50).map(|i| interest(&format!("z-{i:03}"), "2026-09-16"))).collect(), 6),
        ("one leg, then 100,000 cancelled activities (Rails drops them, so the split never ends)", legs(1).chain((0..100_000).map(cancelled)).collect(), 6),
        ("700 legs", legs(700).collect(), 6),
    ];
    for (what, ledger, requests) in cases {
        let (_dir, db, s) = install();
        let server = Paged::new(ledger);
        let failure = ledger::sync_within(&db, &server.venue(), s.api_key_id, &paper(), &clock(), one_page).await.unwrap().unwrap_err();
        assert_eq!((failure.error.as_str(), failure.raised, server.asked().len()), (too_long, true, requests), "{what}: the run's page and five more");
        assert_eq!(one::<String>(&db, state).await, format!("0 rows, watermark none, runs 1, error {too_long}"), "{what}: nothing stored, and the run counted");
    }
    // Just inside the bound: 500 legs are read to their end.
    let (_dir, db, s) = install();
    let out = ledger::sync_within(&db, &Paged::new(legs(500).chain((0..30).map(|i| interest(&format!("z-{i:03}"), "2026-09-16"))).collect()).venue(), s.api_key_id, &paper(), &clock(), one_page).await.unwrap().unwrap();
    assert_eq!((out.complete, out.imported), (true, 31));
    assert_eq!(one::<String>(&db, state).await, "31 rows, watermark 2026-09-16 00:00:00, runs no record, error none", "a run that completes removes its count");

    // An import that fails before it ever stored a slice: the first attempt is counted too, and the import is ended at
    // its last run.
    let (_dir, db, s) = install();
    let venue = Paged::new(legs(700).collect()).venue();
    for run in 1..=4 {
        let failure = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), one_page).await.unwrap().unwrap_err();
        assert_eq!(failure.error, too_long, "run {run}");
        assert_eq!(one::<String>(&db, state).await, format!("0 rows, watermark none, runs {run}, error {too_long}"), "run {run}");
    }
    let gave_up = "the ledger import did not end within its runs: it was stopped, and starts again at the next sync";
    assert_eq!(ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), one_page).await.unwrap().unwrap_err().error, gave_up);
    assert_eq!(one::<String>(&db, state).await, format!("0 rows, watermark none, runs no record, error {gave_up}"));

    // The split lies behind fifty other activities: the first run stores those, and every later run fails at the split.
    let (_dir, db, s) = install();
    let server = Paged::new((0..50).map(|i| interest(&format!("i-{i:03}"), "2026-09-01")).chain(legs(700)).collect());
    let venue = server.venue();
    let limits = one_page;
    assert!(!ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), limits).await.unwrap().unwrap().complete);
    assert_eq!(one::<String>(&db, state).await, "50 rows, watermark none, runs 1, error none");
    for run in 2..=4 {
        let failure = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), limits).await.unwrap().unwrap_err();
        assert_eq!(failure.error, too_long, "run {run}");
        assert_eq!(one::<String>(&db, state).await, format!("50 rows, watermark none, runs {run}, error {too_long}"), "run {run}: a failed run is a counted run");
    }
    let gave_up = "the ledger import did not end within its runs: it was stopped, and starts again at the next sync";
    let failure = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), limits).await.unwrap().unwrap_err();
    assert_eq!((failure.error.as_str(), failure.raised), (gave_up, true));
    assert_eq!(one::<String>(&db, state).await, format!("50 rows, watermark none, runs no record, error {gave_up}"), "the import is ended: no record, the watermark where it was");
    // A run dropped before it stored anything is counted too.
    let (_dir, db, s) = install();
    let server = Paged::new((0..250).map(|i| interest(&format!("i-{i:03}"), "2026-09-01")).collect());
    let venue = server.venue();
    assert!(!ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), limits).await.unwrap().unwrap().complete);
    {
        let (credentials, clock) = (paper(), clock());
        let run = ledger::sync_within(&db, &venue, s.api_key_id, &credentials, &clock, limits);
        let counted = async { while one::<String>(&db, state).await != "100 rows, watermark none, runs 2, error none" { tokio::time::sleep(std::time::Duration::from_millis(2)).await; } };
        tokio::select! { _ = run => panic!("the run ended before it was dropped"), () = counted => {} }
    }
    assert!(one::<String>(&db, state).await.contains("runs 2"));
}

/// Codex round 3, finding 7: an import that goes round, over more than one run, is stopped. Each run remembers where
/// every earlier run of the import stopped; and an import has a stated number of runs.
#[tokio::test(flavor = "current_thread")]
async fn an_import_that_goes_round_or_does_not_end_is_stopped_and_says_so() {
    /// A ledger of seven full pages that then starts again from the first: a cycle no single run of three pages sees.
    #[derive(Clone)]
    struct Round { tokens: Rc<RefCell<Vec<Option<String>>>>, endless: bool }
    impl Transport for Round {
        async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
            let token = r.query.iter().find(|(k, _)| *k == "page_token").map(|(_, v)| v.clone());
            self.tokens.borrow_mut().push(token.clone());
            // The page after token "p<n>-99": page n + 1, or with `endless` false page (n + 1) mod 7.
            let after = token.and_then(|t| t.strip_prefix('p')?.split('-').next()?.parse::<usize>().ok()).map_or(0, |n| n + 1);
            let page = if self.endless { after } else { after % 7 };
            Ok(HttpResponse { status: 200, body: Value::Array((0..100).map(|i| interest(&format!("p{page}-{i:02}"), "2026-09-01")).collect()).to_string() })
        }
    }
    let state = "SELECT (SELECT count(*) FROM account_transactions) || ' rows, watermark ' || coalesce(last_synced_at, 'none') || ', ' || \
                 (SELECT count(*) FROM app_configs WHERE key LIKE 'rust_sync.%') || ' import row, error ' || coalesce(last_sync_error, 'none') FROM api_keys";
    let (_dir, db, s) = install();
    let round = Round { tokens: Rc::default(), endless: false };
    let venue = AlpacaVenue::new(round.clone(), Urls::for_passphrase(None));
    let limits = ledger::Limits { pages: 3, runs: 100 };
    // Pages 0-2, 3-5, then 6, 0 and 1 again: no token repeats inside a run, and the third run stops where no run stopped before.
    for (run, rows) in [(1, 300), (2, 600), (3, 700)] {
        let out = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), limits).await.unwrap().unwrap();
        assert!(!out.complete, "run {run}");
        assert_eq!(one::<String>(&db, state).await, format!("{rows} rows, watermark none, 1 import row, error none"), "run {run}");
    }
    assert_eq!(one::<String>(&db, "SELECT json_extract(value, '$.runs') || ' runs, ' || json_extract(value, '$.pages') || ' pages, stops ' || json_extract(value, '$.cursors') FROM app_configs").await,
               r#"3 runs, 9 pages, stops ["p2-99","p5-99","p1-99"]"#);
    // The fourth run reads page 2 again, whose last id is where the first run stopped: the import is going round.
    let stopped = "the ledger import met a page token it had already passed: it was stopped, and starts again at the next sync";
    let failure = ledger::sync_within(&db, &venue, s.api_key_id, &paper(), &clock(), limits).await.unwrap().unwrap_err();
    assert_eq!((failure.error.as_str(), failure.raised), (stopped, true));
    assert_eq!(one::<String>(&db, state).await, format!("700 rows, watermark none, 0 import row, error {stopped}"), "the record is gone, the watermark is where it was, the key says why");
    // And the job's outcome is what the runner records.
    let clock = clock();
    let job = |transport: Round| {
        #[derive(Clone)]
        struct Venues(Round);
        impl Connect for Venues {
            type T = Round;
            fn connect(&self, _: &Credentials) -> AlpacaVenue<Round> { AlpacaVenue::new(self.0.clone(), Urls::for_passphrase(None)) }
        }
        LedgerSync::new(Venues(transport), s.api_key_id)
    };
    // A ledger that never ends and never repeats: the import is given up after its runs.
    let (_dir, db, s2) = install();
    let endless = job(Round { tokens: Rc::default(), endless: true }).within(ledger::Limits { pages: 1, runs: 3 });
    for run in 1..=3 { assert_eq!(endless.run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::NothingNew, "run {run}"); }
    let gave_up = "the ledger import did not end within its runs: it was stopped, and starts again at the next sync";
    assert_eq!(endless.run(cx(&db, &clock), vec![Wake::Schedule]).await, Outcome::Failed(gave_up.into()));
    assert_eq!(one::<String>(&db, state).await, format!("300 rows, watermark none, 0 import row, error {gave_up}"));
    assert_eq!((s2.api_key_id, ledger::MAX_IMPORT_RUNS, ledger::Limits::RUN.runs), (s.api_key_id, 100, 100));
}

#[tokio::test(flavor = "current_thread")]
async fn a_malformed_balance_answer_fails_the_sync_and_removes_nothing() {
    let (_dir, db, s) = install();
    let (_, v) = venue(balances_script());
    assert_eq!(balances::sync(&db, &v, &NoPrices, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap().synced, 3);
    let state = "SELECT (SELECT group_concat(asset_id || ':' || free || ':' || coalesce(usd_value, 'none') || ':' || synced_at, ' ') FROM account_balances) || ' at ' || balances_synced_at FROM api_keys";
    let before: String = one(&db, state).await;
    let later = FixedClock("2026-09-21T02:30:00Z".parse().unwrap());
    let good_account = json!({ "cash": "100.5" });
    let aapl = json!([{ "symbol": "AAPL", "qty": "2" }]);
    let cases: Vec<(&str, Value, Value, Value, String)> = vec![
        ("a position that is no object", good_account.clone(), json!([5]), json!({}), "unreadable position".into()),
        ("a position whose symbol is blank", good_account.clone(), json!([{ "symbol": " ", "qty": "2" }]), json!({}), "a position without a symbol".into()),
        ("a quantity past the caps", good_account.clone(), json!([{ "symbol": "AAPL", "qty": "1e400" }]), json!({}), "unreadable qty: beyond 10^±40".into()),
        ("a quantity that is no number", good_account.clone(), json!([{ "symbol": "AAPL", "qty": "many" }]), json!({}), "unreadable qty: not a plain decimal number".into()),
        ("cash past the caps", json!({ "cash": "9".repeat(70) }), aapl.clone(), json!({}), "unreadable cash: more than 64 significant digits".into()),
        ("cash that is no number", json!({ "cash": true }), aapl.clone(), json!({}), "unreadable cash: not a number".into()),
        ("an account that is a list", json!([{ "cash": "1" }]), aapl.clone(), json!({}), "unreadable account".into()),
        ("a snapshot price past the caps", good_account.clone(), aapl.clone(), json!({ "AAPL": { "latestTrade": { "p": "1e400" } } }), "unreadable snapshot for AAPL".into()),
        ("a snapshot that is no object", good_account.clone(), aapl.clone(), json!({ "AAPL": 5 }), "unreadable snapshot for AAPL".into()),
        ("more positions than one run holds", good_account.clone(), Value::Array(vec![aapl[0].clone(); balances::MAX_POSITIONS + 1]), json!({}), "more than 5000 positions".into()),
        ("a million empty positions inside the byte limit", good_account.clone(), json!(format!("[{}]", vec!["[]"; 1_300_000].join(","))), json!({}), "more than 5000 positions".into()),
        ("one position with more keys than an answer may hold", good_account.clone(),
         json!(format!("[{{\"symbol\":\"AAPL\",\"qty\":\"2\",{}}}]", (0..200_000).map(|i| format!("\"k{i}\":0")).collect::<Vec<_>>().join(","))), json!({}),
         "positions with more values than one answer may hold".into()),
        ("a quantity that underflows a double, unquoted (a double reads it as 0, which would drop the holding)", good_account.clone(),
         json!(r#"[{"symbol":"AAPL","qty":1e-999}]"#), json!({}), "unreadable qty: beyond 10^±40".into()),
        ("cash that underflows a double, unquoted", json!(r#"{"cash":1e-999}"#), aapl.clone(), json!({}), "unreadable cash: beyond 10^±40".into()),
        ("an account answer over the byte limit", json!({ "cash": "1", "note": "x".repeat(balances::MAX_ACCOUNT_BYTES) }), aapl.clone(), json!({}),
         "Client::TransientNetworkError: the response body is over 65536 bytes".into()),
    ];
    for (what, account, positions, snapshots, error) in cases {
        let (_, v) = venue(json!({ "GET /v2/account": [ok(account)], "GET /v2/positions": [ok(positions)], "GET /v2/stocks/snapshots": [ok(snapshots)] }));
        let failure = balances::sync(&db, &v, &NoPrices, s.api_key_id, &paper(), &later).await.unwrap().unwrap_err();
        assert_eq!((failure.error.as_str(), failure.raised), (error.as_str(), false), "{what}");
        assert_eq!(one::<String>(&db, state).await, before, "{what}: every balance is as it was, and the key's clock did not move");
        assert_eq!(one::<i64>(&db, "SELECT status FROM api_keys").await, 1, "{what}: the key is not condemned for an answer this port refuses");
    }
}

/// Run by the test below, as a child process, so that what the sync prints can be read.
#[tokio::test(flavor = "current_thread")]
async fn log_probe_child() {
    if std::env::var("D2A_LOG_PROBE").is_err() { return; }
    let (_dir, db, s) = install();
    // The key's own secret as an activity id (an activity that cannot be saved: no symbol), and the key's own id as the
    // page token an unfinished import stops at.
    let secret_id = format!("{}-0123456789abcdefghij", paper().secret);
    let mut first: Vec<Value> = vec![json!({ "id": secret_id, "activity_type": "FILL", "side": "buy", "qty": "1", "price": "1", "transaction_time": "2026-09-01T10:00:00Z" })];
    first.extend((1..99).map(|i| interest(&format!("i{i}"), "2026-09-01")));
    first.push(interest(&format!("{}-cursor", paper().key), "2026-09-01"));
    let server = Paged::new(first.into_iter().chain([interest("tail", "2026-09-01")]).collect());
    let one_page = ledger::Limits { pages: 1, runs: 100 };
    let out = ledger::sync_within(&db, &server.venue(), s.api_key_id, &paper(), &clock(), one_page).await.unwrap().unwrap();
    assert_eq!((out.complete, out.imported, out.skipped), (false, 99, 1));
    let out = ledger::sync_within(&db, &server.venue(), s.api_key_id, &paper(), &clock(), one_page).await.unwrap().unwrap();
    assert_eq!((out.complete, out.imported), (true, 1));
}

#[test]
fn no_value_of_an_answer_reaches_the_log() {
    let out = Command::new(std::env::current_exe().unwrap()).args(["--exact", "log_probe_child", "--nocapture", "--test-threads=1"]).env("D2A_LOG_PROBE", "1").output().unwrap();
    let printed = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{printed}");
    for line in ["[alpaca] Account transaction sync skipped invalid entry 1 of 100 (entry_type=0)",
                 "[alpaca] ledger import of api key 1 is not complete: 99 activities stored so far, the next run continues"] {
        assert!(printed.contains(line), "{line}\n{printed}");
    }
    for value in [paper().secret, paper().key, "0123456789abcdefghij".to_string(), "-cursor".to_string()] {
        assert!(!printed.contains(&value), "{value:?} of the answer was printed:\n{printed}");
    }
}

/// The write rule over a history of tens of thousands of rows: every write unit is short, and a writer on another
/// connection (the engine's, the web's) gets the lock between two of them.
#[tokio::test(flavor = "current_thread")]
async fn over_a_large_history_every_write_unit_is_short_and_another_writer_gets_in() {
    let (dir, db, s) = install();
    // 30,000 ledger rows of the last week with no asset (every sync looks at them again), 2,000 withdrawals nothing
    // matches and 150 that a deposit does, and 10 stopped bots with 1,000 closed AAPL orders each.
    db.run(move |c, _| {
        let run = |sql: &str| c.execute_batch(sql).map_err(|e| e.to_string());
        let rows = |n: usize, columns: &str| format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {n}) \
             INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, \
             transfer_link_rejected, created_at, updated_at) SELECT {}, {}, {}, {columns}, '{{}}', 0, '2026-09-19 00:00:00', '2026-09-19 00:00:00' FROM n;", s.user_id, s.api_key_id, s.exchange_id);
        run("BEGIN")?;
        run(&rows(30_000, r#"11, 'USD', 0.07, 'h-' || i, datetime('2026-01-01', '+' || i || ' minutes'), '{"activity_type":"INT"}'"#))?;
        run(&rows(2_000, "5, 'EUR', 1000 + i, 'w-' || i, datetime('2025-01-01', '+' || i || ' hours'), '{}'"))?;
        run(&rows(150, "5, 'USD', 500 + i, 'wm-' || i, datetime('2025-06-01', '+' || (i * 100) || ' hours'), '{}'"))?;
        run(&rows(150, "4, 'USD', 499.5 + i, 'dm-' || i, datetime('2025-06-01', '+' || (i * 100 + 1) || ' hours'), '{}'"))?;
        for _ in 0..10 {
            let bot = seed::insert_bot(c, &s.seeded(), &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-01-01 10:00:00") });
            run(&format!(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) \
                 INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, base, quote, amount, amount_exec, bot_interval, bot_quote_amount, \
                 transaction_type, error_messages, created_at, updated_at) SELECT {bot}, {}, 'o-{bot}-' || i, 0, 2, 0, 0, 'AAPL', 'USD', 1, 1, 'week', 60, 'REGULAR', '[]', \
                 datetime('2026-01-01', '+' || i || ' hours'), '2026-01-01 00:00:00' FROM n;", s.exchange_id))?;
        }
        run("COMMIT")
    }).await.unwrap();

    // The other writer: one small write at a time, as the engine makes them, with the engine's busy timeout.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (stop, path) = (stop.clone(), dir.path().join("production.sqlite3"));
        std::thread::spawn(move || {
            let c = Connection::open(path).unwrap();
            c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            let (mut writes, mut longest) = (0u32, std::time::Duration::ZERO);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let asked = std::time::Instant::now();
                c.execute_batch("BEGIN IMMEDIATE").unwrap();
                longest = longest.max(asked.elapsed());
                c.execute("UPDATE bots SET updated_at = updated_at WHERE id = 1", []).unwrap();
                c.execute_batch("COMMIT").unwrap();
                writes += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            (writes, longest)
        })
    };

    // 3,000 activities in 30 pages: 2,898 new rows a minute apart, 100 the history already holds, and one split of two legs.
    let start: chrono::DateTime<chrono::Utc> = "2026-09-10T00:00:00Z".parse().unwrap();
    let pages: Vec<Value> = (0..30).map(|p| ok(Value::Array((0..100).map(|i| match (p, i) {
        (0, _) => interest(&format!("h-{}", i + 1), "2026-09-01"),
        (15, 50) => split("big-remove", "-10"),
        (15, 51) => split("big-add", "100"),
        _ => json!({ "id": format!("n-{p}-{i}"), "activity_type": "INT", "net_amount": "0.07", "transaction_time": (start + chrono::Duration::minutes(p * 100 + i)).to_rfc3339() }),
    }).collect()))).chain([ok(json!([]))]).collect();
    let (_, v) = venue(json!({ ACTIVITIES: pages }));
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (writes, longest_wait) = writer.join().unwrap();

    println!("large history: the longest write unit held the lock for {:?}; the other writer waited at most {longest_wait:?}, over {writes} writes", longest_write_hold());
    assert_eq!((out.imported, out.duplicates, out.linked, out.splits.len()), (2_899, 100, 150, 1));
    assert_eq!(out.splits[0].restated_bots.len(), 10);
    assert_eq!(one::<i64>(&db, "SELECT count(*) FROM bot_activity_logs WHERE event = 'asset_split'").await, 10);
    // Plan 2f's WRITE_LOCK_BOUND, and Plan 2e's bound on what may hold the runtime thread (an engine write waits here).
    assert!(longest_write_hold() < std::time::Duration::from_millis(100), "the longest write unit held the lock for {:?}", longest_write_hold());
    assert!(longest_wait < std::time::Duration::from_millis(250), "the other writer waited {longest_wait:?} for the lock");
    assert!(writes > 30, "the other writer got in between the units: {writes} writes");
    assert_eq!(WRITE_GAP, std::time::Duration::from_millis(110));
}

#[tokio::test(flavor = "current_thread")]
async fn a_run_dropped_between_two_units_leaves_whole_units_and_the_watermark_where_it_was() {
    let (dir, db, s) = install();
    let pages = json!({ ACTIVITIES: (0..5).map(|p| ok(Value::Array((0..if p == 4 { 50 } else { 100 }).map(|i| interest(&format!("c-{p}-{i}"), "2026-09-01")).collect()))).collect::<Vec<_>>() });
    let stored = |dir: &Path| Connection::open(dir.join("production.sqlite3")).unwrap().query_row("SELECT count(*) FROM account_transactions", [], |r| r.get::<_, i64>(0)).unwrap();
    {
        let (_, v) = venue(pages.clone());
        let (clock, credentials) = (clock(), paper());
        let run = ledger::sync(&db, &v, s.api_key_id, &credentials, &clock);
        let first_unit = async { while stored(dir.path()) < 100 { tokio::time::sleep(std::time::Duration::from_millis(5)).await; } };
        tokio::select! {
            _ = run => panic!("the run ended before it was dropped"),
            () = first_unit => {} // dropped here, as a stop or a deadline drops it: in the gap after a unit
        }
    }
    let rows = one::<i64>(&db, "SELECT count(*) FROM account_transactions").await; // waits for a unit still in hand
    assert!(rows % 100 == 0 && (100..450).contains(&rows), "whole units only: {rows} rows");
    assert_eq!(one::<String>(&db, "SELECT coalesce(last_synced_at, 'none') || ' ' || coalesce(last_sync_error, 'none') FROM api_keys").await, "none none", "the watermark is where it was");
    assert!(write_lock_is_free(dir.path()), "no transaction is left open");
    // The next run reads what is stored as duplicates, stores the rest, and only then moves the watermark.
    let (_, v) = venue(pages);
    let out = ledger::sync(&db, &v, s.api_key_id, &paper(), &clock()).await.unwrap().unwrap();
    assert_eq!((out.duplicates as i64, out.imported as i64), (rows, 450 - rows));
    assert_eq!(one::<String>(&db, "SELECT count(*) || ' ' || (SELECT last_synced_at FROM api_keys) FROM account_transactions").await, "450 2026-09-01 00:00:00");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_run_by_hand_is_held_to_the_jobs_deadline() {
    let (_dir, db, s) = install();
    let clock = clock();
    let pages = json!({ ACTIVITIES: [ok(Value::Array((0..100).map(|i| interest(&format!("i{i:03}"), "2026-09-01")).collect())), ok(json!([]))] });
    let job = LedgerSync::new(HangingVenues(ScriptedTransport::from_script(&pages)), s.api_key_id);
    assert_eq!(jobs::run_within_deadline(&job, cx(&db, &clock), vec![Wake::Manual(None)]).await, Outcome::Failed("dropped past its 3600s deadline".into()));
    assert_eq!(one::<i64>(&db, "SELECT count(*) FROM account_transactions").await, 0);
}
// ---- the hand-run command ----

fn cli(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.args(args).env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", "engine-test-secret")
        .env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT");
    c
}
fn stderr(out: &std::process::Output) -> String { String::from_utf8_lossy(&out.stderr).to_string() }

#[test]
fn sync_by_hand_takes_the_engine_lock_and_refuses_while_rails_or_an_engine_holds_it() {
    let (dir, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")]).unwrap();
    drop(o);
    for bad in [vec!["sync"], vec!["sync", "everything"], vec!["sync", "ledger", "one"], vec!["sync", "ledger", "1", "2"]] {
        let out = cli(dir.path(), &bad).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{bad:?}");
        assert!(stderr(&out).contains("usage: deltabadger sync ledger|balances [<api_key_id>]"), "{bad:?}: {}", stderr(&out));
    }
    let out = cli(dir.path(), &["sync", "ledger"]).env_remove("SECRET_KEY_BASE").output().unwrap();
    assert!(out.status.code() == Some(1) && stderr(&out).contains("SECRET_KEY_BASE"), "{}", stderr(&out));

    // What every Rails process holds for its lifetime (config/initializers/00_engine_lock.rb): shared. A `run` or `serve`
    // holds it exclusive. Either way the sync refuses before it opens a database.
    let rails = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.path().join(".engine.lock")).unwrap();
    rails.try_lock_shared().unwrap();
    for kind in ["ledger", "balances"] {
        let out = cli(dir.path(), &["sync", kind]).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{kind}");
        assert!(stderr(&out).contains("another Deltabadger engine is running on this data"), "{kind}: {}", stderr(&out));
    }
    drop(rails);

    // With the lock free it runs. This install's only key is live, so nothing is sent and the key says why (exit 2).
    for kind in ["ledger", "balances"] {
        let out = cli(dir.path(), &["sync", kind]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{kind}: {}", stderr(&out));
        let job = if kind == "ledger" { "ledger_sync:1" } else { "balance_sync:1" };
        assert!(stderr(&out).contains(&format!("{job} failed: {LIVE_REFUSED}")), "{kind}: {}", stderr(&out));
    }
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(c.query_row("SELECT last_sync_error FROM api_keys", [], |r| r.get::<_, String>(0)).unwrap(), LIVE_REFUSED);
    assert!(deltabadger::lease::read(&c, &seed::cipher()).unwrap().is_none(), "a hand-run sync claims nothing");

    // No key to read (the live one marked incorrect): nothing to do, exit 0. Named, a key is synced whatever its status.
    c.execute("UPDATE api_keys SET status = 2", []).unwrap();
    drop(c);
    let out = cli(dir.path(), &["sync", "balances"]).output().unwrap();
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).trim().to_string()), (Some(0), "no Alpaca key to sync".to_string()), "{}", stderr(&out));
    let out = cli(dir.path(), &["sync", "ledger", "1"]).output().unwrap();
    assert!(out.status.code() == Some(2) && stderr(&out).contains(&format!("ledger_sync:1 failed: {LIVE_REFUSED}")), "{}", stderr(&out));

    // On an install the engine refuses (here a working bot with restated prices), the command still writes what is
    // not the engine's business: the guard is asked only by a unit that moves a bot's counter.
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    seed::insert_bot(&c, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE bots SET restatement_generation = 1", []).unwrap();
    c.execute("UPDATE api_keys SET last_sync_error = NULL", []).unwrap();
    drop(c);
    let out = cli(dir.path(), &["sync", "ledger", "1"]).output().unwrap();
    assert!(out.status.code() == Some(2) && stderr(&out).contains(&format!("ledger_sync:1 failed: {LIVE_REFUSED}")) && !stderr(&out).contains(GUARD_REFUSED), "{}", stderr(&out));
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(c.query_row("SELECT last_sync_error FROM api_keys", [], |r| r.get::<_, String>(0)).unwrap(), LIVE_REFUSED, "written, on an install the guard would refuse");
}
