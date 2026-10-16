//! The tracker's jobs on a Rails-prepared install, under the real scheduler: a sync wakes the walk, which writes
//! today's rows; the backfill runs at 03:00 when a sweep is wanted, and wakes the walk after it. (Agreement with Rails'
//! figures: tracker_parity.rs.)
mod common;
use common::seed;
use deltabadger::crypto::Credentials;
use deltabadger::engine::Clock;
use deltabadger::jobs::data_api::DataApi;
use deltabadger::jobs::schedule::Schedule;
use deltabadger::jobs::{state, Job, Scheduler};
use deltabadger::sync::balances::NoPrices;
use deltabadger::sync::jobs::{self as sync_jobs, Connect};
use deltabadger::tracker::jobs::{self as tracker_jobs, PORTFOLIO_BACKFILL, TRACKER_LEDGER};
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;
use std::rc::Rc;

fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }

/// Wall time that follows tokio's clock, so a paused test runtime moves it (as in tests/sync.rs).
#[derive(Clone, Copy)]
struct TokioClock { start: chrono::DateTime<chrono::Utc>, origin: tokio::time::Instant }
impl Clock for TokioClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> { self.start + chrono::Duration::from_std(self.origin.elapsed()).unwrap() }
}
fn tokio_clock(at: &str) -> TokioClock { TokioClock { start: at.parse().unwrap(), origin: tokio::time::Instant::now() } }
const MINUTE: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Clone)]
struct Scripted(ScriptedTransport);
impl Connect for Scripted {
    type T = ScriptedTransport;
    fn connect(&self, _: &Credentials) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(self.0.clone(), Urls::for_passphrase(None)) }
}

/// The Alpaca seed with AAPL listed, and a ledger of a $1,000 deposit and two AAPL at $200 on 1 September.
fn install() -> (tempfile::TempDir, i64, i64) {
    let (dir, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('AAPL.US', 'AAPL', 'Apple', 'Stock', '2026-01-01', '2026-01-01')", []).unwrap();
    let aapl = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, \
               minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, 'AAPL', 'AAPL', 'USD', ?2, ?3, 9, 2, 2, '0.000000001', '1', 1, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.exchange_id, aapl, s.quote]).unwrap();
    c.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, '2026-01-01', '2026-01-01')", [aapl, s.exchange_id]).unwrap();
    for (kind, base, amount, quote, quote_amount, at, tx) in [(4, "USD", "1000", None, None, "2026-09-01 14:00:00", "d-1"), (0, "AAPL", "2", Some("USD"), Some("400"), "2026-09-01 14:30:00", "f-1")] {
        c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, \
                   transacted_at, raw_data, manual_values, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '{}', '{}', '2026-09-02 02:00:00', '2026-09-02 02:00:00')",
                  rusqlite::params![s.user_id, s.exchange_id, s.api_key_id, kind, base, amount, quote, quote_amount, tx, at]).unwrap();
    }
    (dir, s.user_id, s.api_key_id)
}

/// The venue's answers: no new activity, $600 and two AAPL at $227.52, and AAPL's closes for September.
fn script() -> Value {
    let closes = closes();
    script_with(vec![ok(json!({ "bars": closes, "symbol": "AAPL", "next_page_token": null }))])
}
fn closes() -> Vec<Value> { (1..=30).map(|d| json!({ "t": format!("2026-09-{d:02}T04:00:00Z"), "o": 220, "h": 220, "l": 220, "c": 220 + d, "v": 1 })).collect() }
fn script_with(bars: Vec<Value>) -> Value {
    json!({ "GET /v2/account/activities": [ok(json!([]))],
            "GET /v2/account": [ok(json!({ "cash": "600" }))],
            "GET /v2/positions": [ok(json!([{ "symbol": "AAPL", "asset_class": "us_equity", "qty": "2", "qty_available": "2" }]))],
            "GET /v2/stocks/snapshots": [ok(json!({ "AAPL": { "latestTrade": { "p": 227.52 } } }))],
            "GET /v2/stocks/AAPL/bars": bars })
}

/// Every job `serve` registers for the install, as `scheduler_service` registers them, today's rows dated by `clock`.
fn jobs(dir: &Path, venue: &Scripted, clock: TokioClock) -> Vec<Box<dyn Job>> {
    let c = Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut jobs = sync_jobs::register(&c, venue, Rc::new(NoPrices)).unwrap();
    jobs.extend(tracker_jobs::register(&c, venue, Rc::new(None::<DataApi<ScriptedTransport>>), std::sync::Arc::new(move || clock.now())).unwrap());
    jobs
}

/// Runs the real scheduler over `jobs` on the install's file for `virtual_time` of the test's clock, then stops it.
async fn schedule(dir: &Path, jobs: Vec<Box<dyn Job>>, clock: &TokioClock, virtual_time: std::time::Duration) {
    let s = Scheduler::new(Connection::open(dir.join("production.sqlite3")).unwrap(), seed::cipher(), jobs, None);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (ended, ()) = tokio::join!(s.run(stopped, clock), async { tokio::time::sleep(virtual_time).await; stop.send(true).unwrap(); });
    ended.unwrap();
}

/// A job's record (last run, last success), minutes.
fn record(dir: &Path, job: &str, scope: i64) -> (String, String) {
    let st = state::read(&Connection::open(dir.join("production.sqlite3")).unwrap(), job, Some(&scope.to_string())).unwrap();
    let t = |t: Option<chrono::DateTime<chrono::Utc>>| t.map_or("never".to_string(), |t| t.format("%Y-%m-%dT%H:%M").to_string());
    (t(st.last_run_at), t(st.last_success_at))
}

fn ran(dir: &Path, job: &str, scope: i64, at: &str) {
    let c=Connection::open(dir.join("production.sqlite3")).unwrap();
    let at=at.parse().unwrap();
    if job==sync_jobs::LEDGER_SYNC {
        // The fixture explicitly declares this known key's seeded ledger complete.
        let producer=deltabadger::engine::model::credential_version_by_id(&c,scope).unwrap().unwrap();
        deltabadger::engine::model::credential_write(&c,&Some(producer.clone()),|fenced|{
            deltabadger::sync::cache::record_ledger(fenced,scope,&producer,at)?;
            state::record_success(fenced,job,Some(&scope.to_string()),at).map_err(deltabadger::engine::EngineError::Data)
        }).unwrap();
    } else {
        state::record_success(&c,job,Some(&scope.to_string()),at).unwrap();
    }
}

/// (date, value, invested, held value, held cost, partial) of every whole-account row, as text.
fn snapshots(dir: &Path) -> Vec<String> {
    let c = Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut s = c.prepare("SELECT date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial FROM portfolio_snapshots ORDER BY date").unwrap();
    s.query_map([], |r| Ok(format!("{} {} {} {} {} {}", r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?, r.get::<_, i64>(5)?)))
        .unwrap().collect::<Result<_, _>>().unwrap()
}

/// Per user with a reading Alpaca key, a walk woken on demand and a backfill at 03:00, after the syncs.
#[test]
fn every_user_with_a_reading_key_gets_a_walk_and_a_backfill() {
    let (dir, user, _) = install();
    let specs: Vec<_> = jobs(dir.path(), &Scripted(ScriptedTransport::default()), tokio_clock("2026-09-20T02:25:00Z")).iter().map(|j| j.spec()).map(|s| (s.name, s.scope, s.schedule)).collect();
    let me = Some(user.to_string());
    assert_eq!(specs[2..], [("credential_scope_factory",None,None), (TRACKER_LEDGER, me.clone(), None), (PORTFOLIO_BACKFILL, me, Some(Schedule::Daily { hour: 3, minute: 0 }))]);
}

/// Rails' AccountBalance::SyncJob ends in PortfolioSnapshot.record!: the balance sync at 02:30 wakes the user's walk,
/// which writes today's row from the balances it just stored.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_nightly_balance_sync_wakes_the_walk_and_today_is_recorded() {
    let (dir, user, key) = install();
    ran(dir.path(), sync_jobs::LEDGER_SYNC, key, "2026-09-20T02:00:05Z");
    ran(dir.path(), sync_jobs::BALANCE_SYNC, key, "2026-09-19T02:30:05Z");
    ran(dir.path(), PORTFOLIO_BACKFILL, user, "2026-09-19T03:00:05Z");
    let venue = Scripted(ScriptedTransport::from_script(&script()));
    let clock = tokio_clock("2026-09-20T02:25:00Z");
    schedule(dir.path(), jobs(dir.path(), &venue, clock), &clock, 10 * MINUTE).await;
    assert_eq!(snapshots(dir.path()), ["2026-09-20 1055.04 1000 455.04 400 0"]);
    assert_eq!(record(dir.path(), TRACKER_LEDGER, user), ("2026-09-20T02:30".into(), "2026-09-20T02:30".into()));
}

/// The backfill runs at 03:00 when the page's test says a sweep is wanted (here: never swept), writes every day from
/// the first transaction to yesterday, and wakes the walk, which writes today. The next night nothing has moved, and
/// it sweeps nothing.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_backfill_runs_at_three_when_wanted_then_the_walk_and_not_again_when_nothing_moved() {
    let (dir, user, key) = install();
    ran(dir.path(), sync_jobs::LEDGER_SYNC, key, "2026-09-20T02:00:05Z");
    ran(dir.path(), sync_jobs::BALANCE_SYNC, key, "2026-09-19T02:30:05Z"); // tonight's missed: it runs at the start
    ran(dir.path(), PORTFOLIO_BACKFILL, user, "2026-09-19T03:00:05Z");
    let transport = ScriptedTransport::from_script(&script());
    let venue = Scripted(transport.clone());
    let clock = tokio_clock("2026-09-20T02:55:00Z");
    schedule(dir.path(), jobs(dir.path(), &venue, clock), &clock, 24 * 60 * MINUTE + 15 * MINUTE).await; // to 03:10 the next day
    let rows = snapshots(dir.path());
    assert_eq!((rows.len(), rows[0].as_str(), rows[18].as_str()), (21, "2026-09-01 1042 1000 442 400 0", "2026-09-19 1078 1000 478 400 0"),
               "1 to 19 September swept, today and tomorrow recorded");
    assert_eq!(rows[19..], ["2026-09-20 1055.04 1000 455.04 400 0", "2026-09-21 1055.04 1000 455.04 400 0"]);
    assert_eq!(record(dir.path(), PORTFOLIO_BACKFILL, user), ("2026-09-21T03:00".into(), "2026-09-20T03:00".into()), "the second night swept nothing");
    assert_eq!(transport.requests().iter().filter(|r| r.path.ends_with("/bars")).count(), 1, "AAPL's closes, once");
}

/// AAPL's closes served in two pages, as Alpaca serves a long range: both are read (Clients::Alpaca#get_bars), so the
/// nights the second page holds are swept as well.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_backfill_reads_every_page_of_closes() {
    let (dir, user, key) = install();
    ran(dir.path(), sync_jobs::LEDGER_SYNC, key, "2026-09-20T02:00:05Z");
    ran(dir.path(), sync_jobs::BALANCE_SYNC, key, "2026-09-19T02:30:05Z");
    ran(dir.path(), PORTFOLIO_BACKFILL, user, "2026-09-19T03:00:05Z");
    let all = closes();
    let (first, rest) = all.split_at(10);
    let transport = ScriptedTransport::from_script(&script_with(vec![ok(json!({ "bars": first, "symbol": "AAPL", "next_page_token": "p2" })),
                                                                     ok(json!({ "bars": rest, "symbol": "AAPL", "next_page_token": null }))]));
    let venue = Scripted(transport.clone());
    let clock = tokio_clock("2026-09-20T02:55:00Z");
    schedule(dir.path(), jobs(dir.path(), &venue, clock), &clock, 24 * 60 * MINUTE + 15 * MINUTE).await;
    let rows = snapshots(dir.path());
    assert_eq!((rows.len(), rows[0].as_str(), rows[18].as_str()), (21, "2026-09-01 1042 1000 442 400 0", "2026-09-19 1078 1000 478 400 0"));
    let pages: Vec<Vec<String>> = transport.requests().iter().filter(|r| r.path.ends_with("/bars")).map(|r| r.query.iter().map(|(k, v)| format!("{k}={v}")).collect()).collect();
    assert_eq!(pages.len(), 2, "both pages");
    assert_eq!(pages[1][1], "page_token=p2", "the token joins the query in its sorted place: {:?}", pages[1]);
}

/// 100,000 distinct held symbols through the job's own run: today's rows are written in linear work, the write lock
/// taken once, for the writes alone; under a small allowance the run is refused before it takes the lock.
#[tokio::test(flavor = "current_thread")]
async fn many_held_symbols_record_in_linear_work_or_are_refused_before_the_write_lock() {
    use deltabadger::tracker::jobs::{ledger_run, Allowance, WALK};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (dir, user, key) = install();
    let path = dir.path().join("production.sqlite3");
    let c = Connection::open(&path).unwrap();
    let exchange: i64 = c.query_row("SELECT exchange_id FROM api_keys WHERE id = ?1", [key], |r| r.get(0)).unwrap();
    c.execute_batch(&format!("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
        INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) SELECT 'ZZ' || i || '.US', 'ZZ' || i, 'ZZ' || i, 'Stock', '2026-01-01', '2026-01-01' FROM n;
        INSERT INTO account_transactions (user_id, exchange_id, api_key_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at,
            raw_data, manual_values, created_at, updated_at)
          SELECT {user}, {exchange}, {key}, 0, symbol, 1, 'USD', 1, 'f-' || symbol, '2026-09-03 14:30:00', '{{}}', '{{}}', '2026-09-03 15:00:00', '2026-09-03 15:00:00'
          FROM assets WHERE symbol LIKE 'ZZ%';
        INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, usd_value, priced_at, synced_at, created_at, updated_at)
          SELECT {user}, {exchange}, id, 1, 0, 1.5, 1.5, '2026-09-20 02:30:00', '2026-09-20 02:30:00', '2026-09-20 02:30:00', '2026-09-20 02:30:00'
          FROM assets WHERE symbol LIKE 'ZZ%';
        UPDATE api_keys SET balances_synced_at = '2026-09-20 02:30:00' WHERE id = {key};")).unwrap();
    let db = deltabadger::jobs::Db::new(Connection::open(&path).unwrap(), seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    // These rows are created for this key by this fixture; provide its actual construction stamp.
    let producer=deltabadger::engine::model::credential_version_by_id(&c,key).unwrap().unwrap();
    deltabadger::engine::model::credential_write(&c,&Some(producer.clone()),|fenced|deltabadger::sync::cache::record(fenced,user,exchange,&producer,true,at)).unwrap();
    let clock = deltabadger::engine::FixedClock(at);
    let walls = std::sync::Arc::new(AtomicUsize::new(0));
    let w = walls.clone();
    let wall: deltabadger::tracker::jobs::Wall = std::sync::Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); at });
    let api = None::<&DataApi<ScriptedTransport>>;
    let out = ledger_run(&db, api, None, user, &clock, wall.clone(), &mut Allowance { steps: 1_000_000, stored: 0 }).await;
    assert_eq!(out.err().as_deref(), Some(deltabadger::figures::OVER_BUDGET));
    assert_eq!(walls.load(Ordering::SeqCst), 0, "refused before the write lock");
    let mut run = Allowance::run();
    let walked = ledger_run(&db, api, None, user, &clock, wall, &mut run).await.unwrap();
    assert_eq!(walked.whole.positions.len(), 100_001, "AAPL and the 100,000");
    assert!(WALK.steps - run.steps < deltabadger::figures::budget::FIGURE.steps / 10, "{} steps", WALK.steps - run.steps);
    assert_eq!(walls.load(Ordering::SeqCst), 1, "the wall clock read once, under the lock");
    assert_eq!(snapshots(dir.path()).len(), 1, "today's row");
}

/// Wash-sale protection on and 3,000 distinct recent loss symbols, beside 20,000 other tickers: the run reads the
/// tickers and the lock rows once each, never once per symbol. Counted in SQLite's own VM steps: a ticker query per
/// symbol would scan the 23,000 tickers 3,000 times (69 million rows); the run stays a small multiple of the data.
#[tokio::test(flavor = "current_thread")]
async fn many_loss_symbols_arm_their_locks_in_one_read_of_the_tickers() {
    use deltabadger::tracker::jobs::{ledger_run, Allowance, WALK};
    use std::sync::atomic::{AtomicU64, Ordering};
    let (dir, user, key) = install();
    let path = dir.path().join("production.sqlite3");
    let c = Connection::open(&path).unwrap();
    let (exchange, quote): (i64, i64) = c.query_row("SELECT k.exchange_id, t.quote_asset_id FROM api_keys k JOIN tickers t ON t.exchange_id = k.exchange_id WHERE k.id = ?1 LIMIT 1",
                                                    [key], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    c.execute_batch(&format!("UPDATE users SET wash_sale_enabled = 1 WHERE id = {user};
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 23000)
        INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at)
          SELECT CASE WHEN i <= 3000 THEN 'LS' ELSE 'UN' END || i || '.US', CASE WHEN i <= 3000 THEN 'LS' ELSE 'UN' END || i, 'x', 'Stock', '2026-01-01', '2026-01-01' FROM n;
        INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size,
            minimum_quote_size, trading_enabled, available, created_at, updated_at)
          SELECT {exchange}, symbol, symbol, 'USD', id, {quote}, 9, 2, 2, '0.000000001', '1', 1, 1, '2026-01-01', '2026-01-01' FROM assets WHERE symbol LIKE 'LS%' OR symbol LIKE 'UN%';
        INSERT INTO account_transactions (user_id, exchange_id, api_key_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at,
            raw_data, manual_values, created_at, updated_at)
          SELECT {user}, {exchange}, {key}, k, symbol, 1, 'USD', CASE k WHEN 0 THEN 10 ELSE 5 END, k || '-' || symbol,
                 CASE k WHEN 0 THEN '2026-09-10 14:30:00' ELSE '2026-09-15 14:30:00' END, '{{}}', '{{}}', '2026-09-16 02:00:00', '2026-09-16 02:00:00'
          FROM assets, (SELECT 0 AS k UNION ALL SELECT 1) WHERE symbol LIKE 'LS%';")).unwrap();
    let counted = Connection::open(&path).unwrap();
    let ops = std::sync::Arc::new(AtomicU64::new(0));
    let o = ops.clone();
    counted.progress_handler(1000, Some(move || { o.fetch_add(1000, Ordering::Relaxed); false }));
    let db = deltabadger::jobs::Db::new(counted, seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    let mut run = Allowance::run();
    let walked = ledger_run(&db, None::<&DataApi<ScriptedTransport>>, None, user, &deltabadger::engine::FixedClock(at), std::sync::Arc::new(move || at), &mut run).await.unwrap();
    assert_eq!(walked.whole.loss_sales.len(), 3000);
    let locks: i64 = c.query_row("SELECT count(*) FROM wash_sale_locks WHERE source = 'ledger'", [], |r| r.get(0)).unwrap();
    assert_eq!(locks, 3000);
    let ops = ops.load(Ordering::Relaxed);
    assert!(ops < VM_BOUND, "{ops} SQLite VM steps");
    assert!(WALK.steps - run.steps < deltabadger::figures::budget::FIGURE.steps / 100, "{} steps", WALK.steps - run.steps);
}
/// Measured: about 1.6 million VM steps for this run. A ticker query per symbol would read 69 million rows.
const VM_BOUND: u64 = 10_000_000;

/// data-api's transport, calling `hook` as each request comes in, before it is answered: the moment a fetch is out.
#[derive(Clone)]
struct Hooked { inner: ScriptedTransport, hook: Rc<dyn Fn(usize)>, seen: Rc<std::cell::Cell<usize>> }
impl deltabadger::venue::http::Transport for Hooked {
    async fn send(&self, r: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
        let n = self.seen.get();
        self.seen.set(n + 1);
        (self.hook)(n);
        self.inner.send(r).await
    }
}

/// The install of `install()` with an airdrop of a coin data-api never prices (each pass asks for it twice: the
/// prefetch's range, then the day's window), and a data-api that calls `hook` with each request's number.
fn hooked_install(hook: impl Fn(&Path, usize) + 'static) -> (tempfile::TempDir, i64, Hooked, DataApi<Hooked>) {
    let (dir, user, key) = install();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute_batch(&format!("INSERT OR IGNORE INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('bitcoin', 'BTC', 'Bitcoin', 'Cryptocurrency', '2026-01-01', '2026-01-01');
        INSERT INTO account_transactions (user_id, exchange_id, api_key_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, created_at, updated_at)
          SELECT {user}, exchange_id, id, 8, 'BTC', 0.001, 'a-1', '2026-09-05 00:00:00', '{{}}', '{{}}', '2026-09-06 02:00:00', '2026-09-06 02:00:00' FROM api_keys WHERE id = {key};")).unwrap();
    let inner = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": [ok(json!({ "prices": [] }))] }));
    let path = dir.path().to_path_buf();
    let hooked = Hooked { inner, hook: Rc::new(move |n| hook(&path, n)), seen: Rc::new(std::cell::Cell::new(0)) };
    let api = DataApi::new(deltabadger::jobs::data_api::Config { url: "http://data-api:3000".into(), token: "t".into() }, hooked.clone(), hooked.clone());
    (dir, user, hooked, api)
}

fn edit_deposit(dir: &Path, amount: &str, stamp: &str) {
    Connection::open(dir.join("production.sqlite3")).unwrap()
        .execute("UPDATE account_transactions SET base_amount = ?1, updated_at = ?2 WHERE tx_id = 'd-1'", [amount, stamp]).unwrap();
}

async fn run_ledger(dir: &Path, user: i64, api: &DataApi<Hooked>) -> Result<deltabadger::tracker::walk::Walked, String> {
    use deltabadger::tracker::jobs::{ledger_run, Allowance};
    let db = deltabadger::jobs::Db::new(Connection::open(dir.join("production.sqlite3")).unwrap(), seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    ledger_run(&db, Some(api), None, user, &deltabadger::engine::FixedClock(at), std::sync::Arc::new(move || at), &mut Allowance::run()).await
}

/// A row edited while a fetch is out never reaches the pass that was walking: every lookup of that pass used the rows
/// it loaded. The ledger moved, so the pass is walked again over the edited rows, as Rails' compute! walks again, and
/// the walk states that pass's $1,500 in, not a mix of the two ledgers.
#[tokio::test(flavor = "current_thread")]
async fn a_row_edited_during_a_fetch_writes_nothing_from_mixed_inputs() {
    let (dir, user, hooked, api) = hooked_install(|path, n| if n == 0 { edit_deposit(path, "1500", "2026-09-20 02:59:00") });
    let walked = run_ledger(dir.path(), user, &api).await.unwrap();
    assert_eq!(walked.whole.total_invested.to_s_f(), "1500.0");
    assert_eq!(hooked.inner.requests().len(), 4, "two passes, each its prefetch and the day's window");
    // Today's row, from that pass (no balances are held here, so its figures say everything left).
    let rows = snapshots(dir.path());
    assert_eq!(rows.len(), 1);
    assert!(rows[0].starts_with("2026-09-20 "), "{rows:?}");
}

/// A ledger edited during every fetch of every pass: after the third pass the rows still moved, and nothing is written,
/// not a snapshot, not a lock, not the page cache; the run says why. (Rails writes its third pass; this build writes
/// nothing from a ledger that is no longer the one it walked.)
#[tokio::test(flavor = "current_thread")]
async fn rows_moving_every_pass_write_nothing() {
    let (dir, user, hooked, api) = hooked_install(|path, n| edit_deposit(path, &format!("{}", 1100 + n), &format!("2026-09-20 02:{:02}:00", 10 + n)));
    let out = run_ledger(dir.path(), user, &api).await;
    assert_eq!(out.err().as_deref(), Some(deltabadger::tracker::jobs::MOVED));
    assert_eq!(hooked.inner.requests().len(), 6, "three passes");
    assert!(snapshots(dir.path()).is_empty());
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let cache: i64 = c.query_row("SELECT count(*) FROM app_configs WHERE key = ?1", [deltabadger::tracker::cache::key(user)], |r| r.get(0)).unwrap();
    assert_eq!(cache, 0);
}

/// While a fetch is out, the walk holds no transaction: another connection takes the write lock at once and commits.
#[tokio::test(flavor = "current_thread")]
async fn no_connection_is_held_while_fetching() {
    let wrote = Rc::new(std::cell::RefCell::new(vec![]));
    let w = wrote.clone();
    let (dir, user, _, api) = hooked_install(move |path, _| {
        let other = Connection::open(path.join("production.sqlite3")).unwrap();
        w.borrow_mut().push(other.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS side_write (n INTEGER); COMMIT;").map_err(|e| e.to_string()));
    });
    run_ledger(dir.path(), user, &api).await.unwrap();
    assert!(!wrote.borrow().is_empty() && wrote.borrow().iter().all(Result::is_ok), "{:?}", wrote.borrow());
}

/// Rows that move after the backfill's walk read them, here during the walk's own fetch: the write sees a ledger that
/// is not the one swept, and writes neither table nor either stamp.
#[tokio::test(flavor = "current_thread")]
async fn rows_moving_under_the_backfills_walk_write_no_history() {
    use deltabadger::tracker::jobs::{backfill_run, SWEEP_MOVED};
    // Request 0 is the backfill's own range for the coin; the walk's prefetch is the next.
    let (dir, user, _, api) = hooked_install(|path, n| if n == 1 { edit_deposit(path, "1200", "2026-09-20 02:59:00") });
    let state = |dir: &Path| {
        let c = Connection::open(dir.join("production.sqlite3")).unwrap();
        let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        (count("SELECT count(*) FROM portfolio_snapshots"), count("SELECT count(*) FROM portfolio_venue_snapshots"),
         count("SELECT count(*) FROM app_configs WHERE key LIKE 'snapshot_history_version_%' OR key LIKE 'snapshot_price_generation_%'"))
    };
    let before = state(dir.path());
    let db = deltabadger::jobs::Db::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(), seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    let out = backfill_run(&db, &Scripted(ScriptedTransport::from_script(&script())), Some(&api), None, user, &deltabadger::engine::FixedClock(at)).await;
    assert_eq!(out.err().as_deref(), Some(SWEEP_MOVED));
    assert_eq!(state(dir.path()), before, "no history row and no stamp written");
}

/// Two venues, rewritten: a transfer from Alpaca lands on Binance at the withdrawal, so Binance's days start there;
/// unlinked and swept again, they start at Binance's own first row, and the days before it are gone. Today's rows,
/// `record!`'s, are left alone. A failure in the write unit leaves both tables and the version as they were.
#[tokio::test(flavor = "current_thread")]
async fn a_two_venue_history_is_rewritten_under_each_venue_and_rolled_back_whole() {
    use deltabadger::tracker::jobs::backfill_run;
    let (dir, user, _) = install();
    let path = dir.path().join("production.sqlite3");
    let c = Connection::open(&path).unwrap();
    c.execute_batch(&format!("
        INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Binance', 'Binance', '0.1', '0.1', '2026-09-01 00:00:00', '2026-09-01 00:00:00');
        INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, created_at, updated_at)
          VALUES ({user}, (SELECT id FROM exchanges WHERE type = 'Exchanges::Binance'), 4, 'USD', 100, 'b-dep', '2026-09-08 10:00:00', '{{}}', '{{}}', '2026-09-08 11:00:00', '2026-09-08 11:00:00');
        INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, linked_transaction_id, created_at, updated_at)
          SELECT {user}, exchange_id, 5, 'USD', 100, 'a-wd', '2026-09-05 10:00:00', '{{}}', '{{}}', (SELECT id FROM account_transactions WHERE tx_id = 'b-dep'), '2026-09-05 11:00:00', '2026-09-05 11:00:00'
          FROM account_transactions WHERE tx_id = 'd-1';
        INSERT INTO portfolio_venue_snapshots (user_id, exchange_id, date, value_usd, invested_usd, partial, created_at, updated_at)
          SELECT {user}, id, '2026-09-20', 7, 7, 0, '2026-09-20 02:00:00', '2026-09-20 02:00:00' FROM exchanges;")).unwrap();
    let db = deltabadger::jobs::Db::new(Connection::open(&path).unwrap(), seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    let sweep = || async { backfill_run(&db, &Scripted(ScriptedTransport::from_script(&script())), None::<&DataApi<ScriptedTransport>>, None, user, &deltabadger::engine::FixedClock(at)).await };
    let binance = |c: &Connection| -> (String, i64, i64) { c.query_row("SELECT min(date), count(*), sum(date = '2026-09-20' AND value_usd = 7) FROM portfolio_venue_snapshots v JOIN exchanges e ON e.id = v.exchange_id WHERE e.type = 'Exchanges::Binance'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap() };
    assert_eq!(sweep().await, Ok(true));
    assert_eq!(binance(&c), ("2026-09-05".to_string(), 16, 1), "5 to 19 September, and today's row kept");
    c.execute("UPDATE account_transactions SET linked_transaction_id = NULL, updated_at = '2026-09-19 12:00:00' WHERE tx_id = 'a-wd'", []).unwrap();
    assert_eq!(sweep().await, Ok(true));
    assert_eq!(binance(&c), ("2026-09-08".to_string(), 13, 1), "from Binance's own first row; the days before it gone");
    // A write that fails rolls the unit back: both tables and the version stamp stay as the last sweep left them.
    let dump = |c: &Connection| -> Vec<String> {
        let mut out = vec![];
        for sql in ["SELECT date || value_usd || invested_usd FROM portfolio_snapshots ORDER BY date", "SELECT exchange_id || date || value_usd FROM portfolio_venue_snapshots ORDER BY exchange_id, date",
                    "SELECT key || value FROM app_configs WHERE key LIKE 'snapshot_%' ORDER BY key"] {
            let mut s = c.prepare(sql).unwrap();
            out.extend(s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap));
        }
        out
    };
    let before = dump(&c);
    c.execute_batch("UPDATE account_transactions SET updated_at = '2026-09-19 13:00:00' WHERE tx_id = 'a-wd';
        CREATE TRIGGER refuse_stamp BEFORE UPDATE ON app_configs WHEN NEW.key LIKE 'snapshot_history_version_%' BEGIN SELECT RAISE(ABORT, 'stamp refused'); END;").unwrap();
    assert!(sweep().await.is_err());
    assert_eq!(dump(&c), before);
}

/// Both tables and both stamps, as text.
fn history_dump(dir: &Path) -> Vec<String> {
    let c = Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut out = vec![];
    for sql in ["SELECT date || '|' || value_usd || '|' || invested_usd || '|' || partial FROM portfolio_snapshots ORDER BY date",
                "SELECT exchange_id || '|' || date || '|' || value_usd || '|' || invested_usd FROM portfolio_venue_snapshots ORDER BY exchange_id, date",
                "SELECT key || '|' || value FROM app_configs WHERE key LIKE 'snapshot_%' ORDER BY key"] {
        let mut s = c.prepare(sql).unwrap();
        out.extend(s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap));
    }
    out
}

/// A populated history, then a change while the backfill's own range fetch is out: the window and the price plan were
/// made from the rows before it, so the write refuses and the history and its stamps stay exactly as they were.
async fn a_change_during_the_range_fetch_writes_nothing(change: &'static str) {
    use deltabadger::tracker::jobs::{backfill_run, SWEEP_MOVED};
    let armed = Rc::new(std::cell::Cell::new(false));
    let a = armed.clone();
    let (dir, user, _, api) = hooked_install(move |path, _| if a.replace(false) { Connection::open(path.join("production.sqlite3")).unwrap().execute_batch(change).unwrap(); });
    let db = deltabadger::jobs::Db::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(), seed::cipher());
    let at: chrono::DateTime<chrono::Utc> = "2026-09-20T03:00:00Z".parse().unwrap();
    let (venues, clock) = (Scripted(ScriptedTransport::from_script(&script())), deltabadger::engine::FixedClock(at));
    let run = || backfill_run(&db, &venues, Some(&api), None, user, &clock);
    assert_eq!(run().await, Ok(true));
    // Moved on, so the next run sweeps: the stamp no longer matches.
    Connection::open(dir.path().join("production.sqlite3")).unwrap().execute("UPDATE account_transactions SET updated_at = '2026-09-19 23:00:00' WHERE tx_id = 'd-1'", []).unwrap();
    let before = history_dump(dir.path());
    assert!(before.len() > 20);
    armed.set(true);
    assert_eq!(run().await.err().as_deref(), Some(SWEEP_MOVED));
    assert_eq!(history_dump(dir.path()), before);
}

#[tokio::test(flavor = "current_thread")]
async fn every_row_deleted_during_the_range_fetch_writes_no_history() {
    a_change_during_the_range_fetch_writes_nothing("DELETE FROM account_transactions").await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_instrument_during_the_range_fetch_writes_no_history() {
    a_change_during_the_range_fetch_writes_nothing("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values, created_at, updated_at)
        SELECT user_id, exchange_id, api_key_id, 8, 'ETH', 0.5, 'a-eth', '2026-09-07 00:00:00', '{}', '{}', '2026-09-20 02:59:00', '2026-09-20 02:59:00' FROM account_transactions WHERE tx_id = 'd-1'").await;
}
