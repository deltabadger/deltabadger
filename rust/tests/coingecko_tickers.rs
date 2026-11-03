//! PLAN-A17 A17-2, the Rust-only half: the venue tickers on a CoinGecko install. Request budget (no new pair asks
//! CoinGecko nothing; a new pair crawls once a day, the claim surviving a reopen and racing claimers getting one),
//! D-CG-429-NO-MORE-CRAWLS, D-CG-RETIRED-NOT-ASKED, setup sync's order and its stop on a closed quota, the takeover wake,
//! a Binance-scale venue in bounded units, an interrupted import's recovery, and the scheduler's deadline keeping the
//! finished venues. Rails' half is script/rust/reference_data.rb's CoinGecko ticker scenarios.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::data_api::DataApi;
use deltabadger::jobs::synchronizer::{self, Catalogues};
use deltabadger::jobs::{coingecko::{Api, Clients, CoinGecko, Throttle}, reference, setup_sync, state, Cx, Db, Job, Outcome, Wakers};
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const NOW: &str = "2026-10-02T00:15:00Z";
const KEY: &str = "CG-test-key";
const KRAKEN: &str = "Exchanges::Kraken";
const COINBASE: &str = "Exchanges::Coinbase";

fn install(venues: &[&str]) -> (tempfile::TempDir, Connection, Vec<i64>) {
    let dir = common::baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let c = o.primary;
    for (k, v) in [("market_data_provider", "coingecko"), ("coingecko_api_key", KEY)] {
        deltabadger::app_config::set(&c, &seed::cipher(), k, v, at(NOW)).unwrap();
    }
    c.execute_batch("PRAGMA foreign_keys = OFF; UPDATE exchanges SET available = 0; DELETE FROM tickers; DELETE FROM exchange_assets;").unwrap();
    let ids = venues.iter().map(|ty| {
        c.execute("INSERT OR IGNORE INTO exchanges (type, name, available, created_at, updated_at) VALUES (?1, ?1, 1, '2026-01-01', '2026-01-01')", [ty]).unwrap();
        c.execute("UPDATE exchanges SET available = 1 WHERE type = ?1", [ty]).unwrap();
        c.query_row("SELECT id FROM exchanges WHERE type = ?1", [ty], |r| r.get(0)).unwrap()
    }).collect();
    (dir, c, ids)
}
fn reopen(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }
fn asset(c: &Connection, ext: &str) -> i64 {
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?1, ?1, 'Cryptocurrency', '2026-01-01', '2026-01-01')", [ext]).unwrap();
    c.last_insert_rowid()
}
fn ticker(c: &Connection, venue: i64, base: i64, quote: i64, name: &str, b: &str, q: &str) {
    for a in [base, quote] { c.execute("INSERT OR IGNORE INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, '2026-01-01', '2026-01-01')", [a, venue]).unwrap(); }
    c.execute("INSERT INTO tickers (exchange_id, base_asset_id, quote_asset_id, ticker, base, quote, base_decimals, quote_decimals, price_decimals, minimum_base_size, minimum_quote_size, available, created_at, updated_at) \
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, 8, 2, 2, 0.0001, 1, 1, '2026-01-01', '2026-01-01')", rusqlite::params![venue, base, quote, name, b, q]).unwrap();
}
/// A catalogue row as get_tickers_info gives it.
fn row(name: &str, base: &str, quote: &str) -> Value {
    json!({ "ticker": name, "base": base, "quote": quote, "minimum_base_size": "0.001", "minimum_quote_size": "5", "maximum_base_size": null,
            "maximum_quote_size": null, "base_decimals": 6, "quote_decimals": 2, "price_decimals": 3, "trading_enabled": true, "available": true })
}
fn crawl_key(id: &str, page: usize) -> String { format!("GET /api/v3/exchanges/{id}/tickers?depth=false&include_exchange_logo=false&order=base_asset&page={page}") }
fn gecko(base: &str, coin: &str, target: &str) -> Value { json!({ "base": base, "coin_id": coin, "target": target, "target_coin_id": null }) }

fn clients(t: &ScriptedTransport) -> Clients {
    let t = t.clone();
    Rc::new(move |key: &str| Box::new(CoinGecko::new(key, t.clone()).with_throttle(Arc::new(Throttle::new(10_000)))) as Box<dyn Api>)
}
fn demo(t: &ScriptedTransport) -> &ScriptedTransport { t.reply("GET /api/v3/ping", 401, json!({})) }
fn cg_requests(t: &ScriptedTransport) -> Vec<String> { t.requests().iter().map(deltabadger::venue::http::request_key).filter(|k| !k.ends_with("/ping")).collect() }

/// Scripted catalogues by venue type, and the venues asked, in order.
type Answers = Rc<RefCell<HashMap<String, Result<Vec<Value>, String>>>>;
#[derive(Clone, Default)]
struct Cats { rows: Answers, asked: Rc<RefCell<Vec<String>>>, proxies: Rc<RefCell<Vec<Option<String>>>> }
impl Cats {
    fn set(&self, ty: &str, rows: Result<Vec<Value>, String>) -> &Self { self.rows.borrow_mut().insert(ty.into(), rows); self }
    fn seam(&self) -> Catalogues {
        let me = self.clone();
        Rc::new(move |ty: &str, proxy: Option<String>| {
            me.asked.borrow_mut().push(ty.into());
            me.proxies.borrow_mut().push(proxy);
            let answer = me.rows.borrow().get(ty).cloned();
            Box::pin(async move { match answer { Some(a) => a, None => std::future::pending().await } })
        })
    }
}

fn cx(db: &Db, now: &str) -> Cx<'static> { Cx { db: db.clone(), clock: Box::leak(Box::new(FixedClock(at(now)))), wakers: Wakers::default() } }
async fn tickers(c: Connection, t: &ScriptedTransport, cats: &Cats, now: &str) -> Outcome {
    let db = Db::new(c, seed::cipher());
    reference::run_once_full::<ScriptedTransport>(reference::TICKERS, None::<DataApi<ScriptedTransport>>, &clients(t), &cats.seam(), cx(&db, now)).await
}
fn count(c: &Connection, q: &str) -> i64 { c.query_row(q, [], |r| r.get(0)).unwrap() }

/// A run with no new pair asks CoinGecko nothing; a new pair crawls once a day, the claim surviving a reopen; two racing
/// claimers get one claim.
#[tokio::test(flavor = "current_thread")]
async fn a_new_pair_crawls_once_a_day_and_nothing_new_asks_nothing() {
    let (d, c, ids) = install(&[KRAKEN]);
    let (a, b) = (asset(&c, "coin-a"), asset(&c, "coin-b"));
    ticker(&c, ids[0], a, b, "AB", "A", "B");
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![row("AB", "A", "B")]));
    let t = ScriptedTransport::default();
    assert_eq!(tickers(c, &t, &cats, NOW).await, Outcome::Done, "the catalogue applied");
    assert!(t.requests().is_empty(), "no new pair: CoinGecko is not asked");

    cats.set(KRAKEN, Ok(vec![row("AB", "A", "B"), row("CB", "C", "B")]));
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 200, json!({ "tickers": [gecko("A", "coin-a", "B"), gecko("C", "coin-c", "B")] }));
    assert_eq!(tickers(reopen(&d), &t, &cats, NOW).await, Outcome::Done);
    assert_eq!(cg_requests(&t), [crawl_key("kraken", 1)]);
    // coin-b is not in the crawl's map: its symbol B resolves only if CoinGecko names it; here B maps to nothing.
    assert_eq!(count(&reopen(&d), "SELECT count(*) FROM tickers WHERE ticker = 'CB'"), 0, "B unresolved: the pair waits");
    assert_eq!(count(&reopen(&d), "SELECT count(*) FROM assets WHERE external_id = 'coin-c'"), 1, "the crawl's coins are minted");

    // The same day, on a reopened database: the claim holds.
    let t = ScriptedTransport::default();
    assert_eq!(tickers(reopen(&d), &t, &cats, "2026-10-02T20:00:00Z").await, Outcome::Done);
    assert!(t.requests().is_empty(), "claimed today: no second crawl");
    // A day on, it crawls again, and the pair resolves.
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 200, json!({ "tickers": [{ "base": "C", "coin_id": "coin-c", "target": "B", "target_coin_id": "coin-b" }] }));
    assert_eq!(tickers(reopen(&d), &t, &cats, "2026-10-03T00:15:00Z").await, Outcome::Done);
    assert_eq!(cg_requests(&t).len(), 1);
    assert_eq!(count(&reopen(&d), "SELECT count(*) FROM tickers WHERE ticker = 'CB' AND available = 1"), 1);

    // Racing claimers, each on its own connection: one claim.
    let key = "rust_job.crawl_claim.race";
    let (x, y) = (reopen(&d), reopen(&d));
    let won = [synchronizer::claim(&x, key, at(NOW)).unwrap(), synchronizer::claim(&y, key, at(NOW)).unwrap()];
    assert_eq!(won.iter().filter(|w| **w).count(), 1);
}

/// D-CG-429-NO-MORE-CRAWLS: the first venue's crawl answers 429; the second venue's new pair is not crawled, and both
/// catalogues still apply. The outcome is RateLimited and names the catalogue failure too.
#[tokio::test(flavor = "current_thread")]
async fn a_429_ends_every_crawl_but_every_catalogue_applies() {
    let (d, c, ids) = install(&[KRAKEN, COINBASE, "Exchanges::Binance"]);
    let (a, b) = (asset(&c, "coin-a"), asset(&c, "coin-b"));
    ticker(&c, ids[0], a, b, "AB", "A", "B");
    ticker(&c, ids[1], a, b, "A-B", "A", "B");
    let mut changed = row("A-B", "A", "B");
    changed["price_decimals"] = json!(7);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![row("AB", "A", "B"), row("NEW1", "N1", "B")]))
        .set(COINBASE, Ok(vec![changed, row("N2-B", "N2", "B")]))
        .set("Exchanges::Binance", Err("Binance: HTTP 503".into()));
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 429, json!({}));
    let out = tickers(c, &t, &cats, NOW).await;
    assert!(matches!(&out, Outcome::RateLimited(m) if m.contains("kraken") && m.contains("binance: Binance: HTTP 503")), "{out:?}");
    assert_eq!(cg_requests(&t), [crawl_key("kraken", 1)], "Coinbase's crawl is not asked");
    let c = reopen(&d);
    assert_eq!(count(&c, &format!("SELECT price_decimals FROM tickers WHERE exchange_id = {}", ids[1])), 7, "Coinbase's catalogue applied");
    assert_eq!(count(&c, "SELECT count(*) FROM app_configs WHERE key = 'rust_job.crawl_claim.gdax'"), 0, "no claim spent without a request");
}

/// D-CG-RETIRED-NOT-ASKED; a stock venue is not asked; an install with no provider asks nothing and is NothingNew; the
/// scheduled job with a blank key fails before any venue (sync_tickers_from_coingecko!).
#[tokio::test(flavor = "current_thread")]
async fn retired_and_stock_venues_are_not_asked_and_guards_ask_nothing() {
    let (d, c, _) = install(&[KRAKEN, "Exchanges::Bitmart", "Exchanges::Alpaca"]);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![]));
    let t = ScriptedTransport::default();
    assert_eq!(tickers(c, &t, &cats, NOW).await, Outcome::NothingNew);
    assert_eq!(*cats.asked.borrow(), [KRAKEN]);

    deltabadger::app_config::set(&reopen(&d), &seed::cipher(), "coingecko_api_key", "", at(NOW)).unwrap();
    let cats = Cats::default();
    assert_eq!(tickers(reopen(&d), &t, &cats, NOW).await, Outcome::Failed("CoinGecko API key not configured".into()));
    assert!(cats.asked.borrow().is_empty() && t.requests().is_empty());

    reopen(&d).execute("DELETE FROM app_configs WHERE key IN ('market_data_provider', 'coingecko_api_key')", []).unwrap();
    assert_eq!(tickers(reopen(&d), &t, &cats, NOW).await, Outcome::NothingNew, "not configured: return");
    assert!(cats.asked.borrow().is_empty());
}

/// A venue minting crypto assets wakes the bulk asset backfill under its once-a-day claim: one wake however many venues
/// mint. Setup sync passes no wake.
#[tokio::test(flavor = "current_thread")]
async fn minted_coins_wake_the_backfill_once_a_day() {
    let (d, c, _) = install(&[KRAKEN, COINBASE]);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![row("XY", "X", "Y")])).set(COINBASE, Ok(vec![row("Z-Y", "Z", "Y")]));
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 200, json!({ "tickers": [gecko("X", "coin-x", "Y"), gecko("Y", "coin-y", "X")] }))
        .reply(&crawl_key("gdax", 1), 200, json!({ "tickers": [gecko("Z", "coin-z", "Y"), gecko("Y", "coin-y", "Z")] }));
    let db = Db::new(c, seed::cipher());
    let cx = cx(&db, NOW);
    let wakers = cx.wakers.clone();
    assert_eq!(reference::run_once_full::<ScriptedTransport>(reference::TICKERS, None, &clients(&t), &cats.seam(), cx).await, Outcome::Done);
    assert_eq!(wakers.pending(), [(reference::ASSETS.to_string(), None)], "one wake for two minting venues");
    assert_eq!(count(&reopen(&d), "SELECT count(*) FROM tickers"), 2);
}

/// Setup sync on CoinGecko: TICKERS (forced: no claim taken), ASSETS, INDICES in that order; a catalogue failure and a
/// crawl's 429 stop it before ASSETS, the setup request kept.
#[tokio::test(flavor = "current_thread")]
async fn setup_sync_runs_tickers_first_and_stops_on_a_closed_quota() {
    let (d, c, _) = install(&[KRAKEN, COINBASE]);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![row("XY", "X", "Y")])).set(COINBASE, Err("Coinbase: HTTP 500".into()));
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 200, json!({ "tickers": [gecko("X", "coin-x", "Y"), gecko("Y", "coin-y", "X")] }))
        .reply("GET /api/v3/coins/markets", 200, json!([]));
    let db = Db::new(c, seed::cipher());
    setup_sync::request(&reopen(&d), at(NOW)).unwrap();
    let job = setup_sync::SeedAndSync::new(&|_| None, DataApi::live).with_coingecko(clients(&t)).with_catalogues(cats.seam());
    assert_eq!(job.spec().deadline, std::time::Duration::from_secs(90 * 60));
    job.run(cx(&db, NOW), vec![]).await;
    let keys = cg_requests(&t);
    assert_eq!(keys[0], crawl_key("kraken", 1), "tickers first: {keys:?}");
    assert!(keys[1..].iter().all(|k| k.starts_with("GET /api/v3/coins/markets")) && keys.len() >= 2, "then assets and indices: {keys:?}");
    assert_eq!(count(&reopen(&d), "SELECT count(*) FROM app_configs WHERE key LIKE 'rust_job.crawl_claim.%'"), 0, "forced: no claim");
    assert_eq!(state::read(&reopen(&d), reference::TICKERS, None).unwrap().last_error.as_deref().map(|e| e.contains("coinbase")), Some(true));

    let (d, c, _) = install(&[KRAKEN, COINBASE]);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![row("XY", "X", "Y")])).set(COINBASE, Err("Coinbase: HTTP 500".into()));
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 429, json!({}));
    setup_sync::request(&reopen(&d), at(NOW)).unwrap();
    let db = Db::new(c, seed::cipher());
    setup_sync::SeedAndSync::new(&|_| None, DataApi::live).with_coingecko(clients(&t)).with_catalogues(cats.seam()).run(cx(&db, NOW), vec![]).await;
    assert_eq!(cg_requests(&t), [crawl_key("kraken", 1)], "nothing after the 429");
    assert!(deltabadger::app_config::get_plain(&reopen(&d), setup_sync::REQUEST).unwrap().is_some(), "the request stays");
}

/// The takeover: a queued per-venue Rails job wakes one whole TICKERS run; the fan-out stays dropped.
#[test]
fn the_takeover_wakes_the_tickers_run() {
    use deltabadger::engine::handover::{rails_work, RailsWork};
    assert_eq!(rails_work("Exchange::SyncTickersAndAssetsJob"), RailsWork::WakeUnscoped(reference::TICKERS));
    assert_eq!(rails_work("Exchange::SyncAllTickersAndAssetsJob"), RailsWork::Dropped);
}

/// A Binance-scale venue: ~2,000 new pairs from one crawl publish in bounded units (assets, rows, sweep), every pair landed.
#[tokio::test(flavor = "current_thread")]
async fn a_binance_scale_venue_publishes_in_bounded_units() {
    let (d, c, ids) = install(&["Exchanges::Binance"]);
    let n: usize = 2000;
    let rows: Vec<Value> = (0..n).map(|i| row(&format!("C{i}USDT"), &format!("C{i}"), "USDT")).collect();
    let crawl: Vec<Value> = (0..n).map(|i| gecko(&format!("C{i}"), &format!("scale-coin-{i}"), "USDT"))
        .chain([json!({ "base": "USDT", "coin_id": "scale-tether", "target": "C0", "target_coin_id": "scale-coin-0" })]).collect();
    let (map, ambiguous) = synchronizer::symbol_map("binance", &crawl);
    assert!(ambiguous.is_empty());
    let plan = synchronizer::plan(&c, ids[0], "binance", &rows, Some(&map)).unwrap();
    assert!(plan.units() >= 2 * n.div_ceil(deltabadger::jobs::CHUNK), "{} units", plan.units());
    let t = ScriptedTransport::default();
    let pages: Vec<Value> = crawl.chunks(100).map(|p| json!({ "tickers": p })).collect();
    demo(&t);
    for (i, p) in pages.iter().enumerate() { t.reply(&crawl_key("binance", i + 1), 200, p.clone()); }
    assert!(!crawl.len().is_multiple_of(100), "the last page is short");
    let cats = Cats::default();
    cats.set("Exchanges::Binance", Ok(rows));
    assert_eq!(tickers(c, &t, &cats, NOW).await, Outcome::Done);
    let c = reopen(&d);
    assert_eq!(count(&c, &format!("SELECT count(*) FROM tickers WHERE exchange_id = {}", ids[0])), n as i64);
    let path = c.path().unwrap().to_string();
    assert!(deltabadger::jobs::import::holds(&path).contains_key("venue tickers"));
}

/// An import interrupted after its assets were published: a fresh, non-forced run the same day re-applies the catalogue
/// and leaves the lost discovery's new pairs waiting for the claim's expiry; a day on, they resolve.
#[tokio::test(flavor = "current_thread")]
async fn an_interrupted_import_recovers_as_rails_does() {
    let (d, c, ids) = install(&[KRAKEN]);
    let (a, b) = (asset(&c, "coin-a"), asset(&c, "coin-b"));
    ticker(&c, ids[0], a, b, "AB", "A", "B");
    let mut changed = row("AB", "A", "B");
    changed["price_decimals"] = json!(5);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![changed, row("NB", "N", "B")]));
    let crawl = json!({ "tickers": [gecko("N", "coin-n", "B"), { "base": "A", "coin_id": "coin-a", "target": "B", "target_coin_id": "coin-b" }] });
    c.execute_batch("CREATE TRIGGER crash BEFORE UPDATE ON tickers BEGIN SELECT RAISE(ABORT, 'interrupted'); END").unwrap();
    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 200, crawl.clone());
    let db = Db::new(c, seed::cipher());
    let run_cx = cx(&db, NOW);
    let wakers = run_cx.wakers.clone();
    let out = reference::run_once_full::<ScriptedTransport>(reference::TICKERS, None, &clients(&t), &cats.seam(), run_cx).await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("interrupted")), "{out:?}");
    let c = reopen(&d);
    assert_eq!(count(&c, "SELECT count(*) FROM assets WHERE external_id = 'coin-n'"), 1, "the assets unit committed");
    assert_eq!(wakers.pending(), [(reference::ASSETS.to_string(), None)], "the backfill is woken once the coins commit, the ticker failure after them notwithstanding");
    assert!(state::read(&c, reference::TICKERS, None).unwrap().incomplete_since.is_some(), "the record is left incomplete");
    c.execute_batch("DROP TRIGGER crash").unwrap();

    let t = ScriptedTransport::default();
    assert_eq!(tickers(reopen(&d), &t, &cats, "2026-10-02T04:15:00Z").await, Outcome::Done);
    assert!(t.requests().is_empty(), "the claim holds");
    let c = reopen(&d);
    assert_eq!(count(&c, "SELECT price_decimals FROM tickers WHERE ticker = 'AB'"), 5, "the catalogue re-applied");
    assert_eq!(count(&c, "SELECT count(*) FROM tickers WHERE ticker = 'NB'"), 0, "the new pair waits");

    let t = ScriptedTransport::default();
    demo(&t).reply(&crawl_key("kraken", 1), 200, crawl);
    assert_eq!(tickers(reopen(&d), &t, &cats, "2026-10-03T00:15:00Z").await, Outcome::Done);
    assert_eq!(count(&reopen(&d), "SELECT count(*) FROM tickers WHERE ticker = 'NB'"), 1, "resolved after the claim's expiry");
}

/// The scheduler's deadline cancelling a run keeps every finished venue, for the scheduled TICKERS (60 minutes on the
/// coingecko provider) and for setup sync: the second venue's catalogue never answers.
#[tokio::test(flavor = "current_thread")]
async fn a_deadline_keeps_the_finished_venues() {
    use deltabadger::jobs::{JobFuture, Scheduler, Spec, Wake};
    struct Short(Box<dyn Job>);
    impl Job for Short {
        fn spec(&self) -> Spec { Spec { deadline: std::time::Duration::from_secs(2), ..self.0.spec() } }
        fn run<'a>(&'a self, cx: Cx<'a>, wakes: Vec<Wake>) -> JobFuture<'a> { self.0.run(cx, wakes) }
    }
    for setup in [false, true] {
        let (d, c, ids) = install(&[KRAKEN, COINBASE]);
        let (a, b) = (asset(&c, "coin-a"), asset(&c, "coin-b"));
        ticker(&c, ids[0], a, b, "AB", "A", "B");
        let mut changed = row("AB", "A", "B");
        changed["price_decimals"] = json!(6);
        let cats = Cats::default();
        cats.set(KRAKEN, Ok(vec![changed])); // Coinbase's never answers
        let t = ScriptedTransport::default();
        let job: Box<dyn Job> = if setup {
            setup_sync::request(&c, at(NOW)).unwrap();
            Box::new(setup_sync::SeedAndSync::new(&|_| None, DataApi::live).with_coingecko(clients(&t)).with_catalogues(cats.seam()))
        } else {
            let mut all = reference::shared_jobs_full::<ScriptedTransport>(Rc::new(None), clients(&t), cats.seam());
            let i = all.iter().position(|j| j.spec().name == reference::TICKERS).unwrap();
            let job = all.remove(i);
            assert_eq!(job.spec().deadline, reference::COINGECKO_TICKERS_DEADLINE);
            assert_eq!(job.spec().deadline, std::time::Duration::from_secs(3600));
            job
        };
        let name = job.spec().name;
        let scheduler = Scheduler::new(c, seed::cipher(), vec![Box::new(Short(job))], None);
        scheduler.wakers().wake(name, None, None);
        let (stop, rx) = tokio::sync::watch::channel(false);
        let file = d.path().join("production.sqlite3");
        let driver = async {
            let started = std::time::Instant::now();
            loop {
                let failing = Connection::open(&file).ok().and_then(|c| state::read(&c, name, None).ok()).is_some_and(|s| s.failing());
                if failing { break; }
                assert!(started.elapsed() < std::time::Duration::from_secs(60), "{setup}: the deadline was never recorded");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            stop.send(true).unwrap();
        };
        let clock = FixedClock(at(NOW));
        let (result, ()) = tokio::join!(scheduler.run(rx, &clock), driver);
        result.unwrap();
        assert_eq!(count(&reopen(&d), "SELECT price_decimals FROM tickers WHERE ticker = 'AB'"), 6, "{setup}: Kraken's units kept");
        assert_eq!(*cats.asked.borrow(), [KRAKEN, COINBASE]);
    }
}

#[test]
fn claims_expire_after_a_day() {
    let (d, _c, _) = install(&[]);
    let c = reopen(&d);
    let now = at(NOW);
    assert!(synchronizer::claim(&c, synchronizer::BACKFILL_CLAIM, now).unwrap());
    assert!(!synchronizer::claim(&reopen(&d), synchronizer::BACKFILL_CLAIM, now + Duration::hours(23)).unwrap(), "survives a reopen");
    assert!(synchronizer::claim(&reopen(&d), synchronizer::BACKFILL_CLAIM, now + Duration::hours(24)).unwrap());
}

/// D-CG-ROW-REFUSED: existing AUSD (A/USD) and BUSD (B/USD); the catalogue names A/USD "BUSD", which the (exchange, ticker)
/// unique index refuses. Rails raises RecordNotUnique out of the venue's sync there: the row confirms nothing and the sweep
/// never runs. Rust skips the row, applies the rows after it, and does not sweep the venue that run either.
#[tokio::test(flavor = "current_thread")]
async fn a_refused_row_confirms_nothing_and_suppresses_the_sweep() {
    let (d, c, ids) = install(&[KRAKEN]);
    let (a, b, u, g) = (asset(&c, "coin-a"), asset(&c, "coin-b"), asset(&c, "coin-usd"), asset(&c, "coin-gone"));
    ticker(&c, ids[0], a, u, "AUSD", "A", "USD");
    ticker(&c, ids[0], b, u, "BUSD", "B", "USD");
    ticker(&c, ids[0], g, u, "GUSD", "G", "USD"); // not in the catalogue: a sweep would take it
    let mut b_row = row("BUSD", "B", "USD");
    b_row["price_decimals"] = json!(7);
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![row("BUSD", "A", "USD"), b_row]));
    let t = ScriptedTransport::default();
    assert_eq!(tickers(c, &t, &cats, NOW).await, Outcome::Done);
    let c = reopen(&d);
    assert_eq!(count(&c, "SELECT count(*) FROM tickers WHERE ticker IN ('AUSD', 'BUSD', 'GUSD') AND available = 1"), 3, "nothing swept");
    assert_eq!(count(&c, "SELECT price_decimals FROM tickers WHERE ticker = 'AUSD'"), 2, "the refused row wrote nothing");
    assert_eq!(count(&c, "SELECT price_decimals FROM tickers WHERE ticker = 'BUSD'"), 7, "the rows after it apply");
    assert_eq!(count(&c, &format!("SELECT count(*) FROM exchange_assets WHERE exchange_id = {} AND available = 0", ids[0])), 0);
}

/// The catalogue goes through the venue's exchange proxy as its client would: AppConfig `proxy_<name>` over ENV `PROXY_<NAME>`.
#[tokio::test(flavor = "current_thread")]
async fn the_catalogue_takes_the_venues_proxy() {
    let (d, c, _) = install(&[KRAKEN, COINBASE]);
    deltabadger::app_config::set(&c, &seed::cipher(), "proxy_kraken", "http://stored-proxy:8080", at(NOW)).unwrap();
    let env = |k: &str| (k == "PROXY_KRAKEN" || k == "PROXY_COINBASE").then(|| "http://env-proxy:1".to_string());
    assert_eq!(synchronizer::catalogue_proxy(&c, &seed::cipher(), KRAKEN, &env).unwrap().as_deref(), Some("http://stored-proxy:8080"), "AppConfig wins");
    assert_eq!(synchronizer::catalogue_proxy(&c, &seed::cipher(), COINBASE, &env).unwrap().as_deref(), Some("http://env-proxy:1"), "else the env");
    assert_eq!(synchronizer::catalogue_proxy(&c, &seed::cipher(), COINBASE, &|_| None).unwrap(), None);
    assert!(synchronizer::PublicTransport::new(Some("not a url")).is_err(), "an unusable proxy sends nothing");
    let cats = Cats::default();
    cats.set(KRAKEN, Ok(vec![])).set(COINBASE, Ok(vec![]));
    tickers(c, &ScriptedTransport::default(), &cats, NOW).await;
    assert_eq!(cats.proxies.borrow()[0].as_deref(), Some("http://stored-proxy:8080"), "the run hands the catalogue its venue's proxy");
    drop(d);
}

/// Fiat.currencies, recorded from Rails (script/rust/record_fiat_currencies.rb), pinned to fiat.rb and the synchronizer.
#[test]
fn the_fiat_currencies_are_rails() {
    use sha2::{Digest, Sha256};
    let v: Value = serde_json::from_str(include_str!("fixtures/fiat_currencies.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in v["sources"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "re-record {path}: script/rust/repin");
    }
    let rust: Vec<Value> = synchronizer::FIAT.iter()
        .map(|(id, sym, name, color)| json!({ "external_id": id, "symbol": sym, "name": name, "category": "Currency", "color": color })).collect();
    assert_eq!(Value::Array(rust), v["currencies"]);
}
