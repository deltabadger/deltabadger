//! PLAN-A17 (A17-1), the Rust-only half: the one market-data rule its three readers share, the CoinGecko asset refresh's
//! request budget, its rate-limit and degraded answers, the one-asset job's own record, setup sync's CoinGecko branch and
//! its durable request, and staleness on a CoinGecko-fed catalogue. Rails' half is script/rust/reference_data.rb.
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::jobs::{coingecko::{self, Api, Clients, CoinGecko, Throttle}, reference, setup_sync, state, Cx, Db, Job, Outcome, Wakers};
use deltabadger::engine::FixedClock;
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::rc::Rc;
use std::sync::Arc;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const NOW: &str = "2026-10-02T00:20:00Z";
const KEY: &str = "CG-test-key";

fn install() -> (tempfile::TempDir, Connection) {
    let dir = common::baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    for (k, v) in [("market_data_provider", "coingecko"), ("coingecko_api_key", KEY)] {
        deltabadger::app_config::set(&o.primary, &seed::cipher(), k, v, at(NOW)).unwrap();
    }
    (dir, o.primary)
}
fn reopen(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }
fn asset(c: &Connection, ext: &str, category: &str) -> i64 {
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?1, ?1, ?2, '2026-01-01', '2026-01-01')", [ext, category]).unwrap();
    c.last_insert_rowid()
}
fn coin(id: &str) -> Value { json!({ "id": id, "symbol": id, "name": id, "image": format!("https://img/{id}.png"), "market_cap": 1e9, "market_cap_rank": 3, "circulating_supply": 10.5 }) }
/// Scripted clients over one transport (the throttle generous: request counts, not timing, are under test here).
fn clients(t: &ScriptedTransport) -> Clients {
    let t = t.clone();
    Rc::new(move |key: &str| Box::new(CoinGecko::new(key, t.clone()).with_throttle(Arc::new(Throttle::new(10_000)))) as Box<dyn Api>)
}
fn demo(t: &ScriptedTransport) -> &ScriptedTransport { t.reply("GET /api/v3/ping", 401, json!({})) }
fn cx(db: &Db) -> Cx<'static> { Cx { db: db.clone(), clock: Box::leak(Box::new(FixedClock(at(NOW)))), wakers: Wakers::default() } }
fn symbol(c: &Connection, ext: &str) -> String { c.query_row("SELECT symbol FROM assets WHERE external_id = ?1", [ext], |r| r.get(0)).unwrap() }

/// The three readers (web, jobs, engine) give one answer for every row/env combination.
#[test]
fn the_three_readers_of_market_data_settings_agree() {
    use deltabadger::market_data::{decide, Env, Provider, Rows};
    let values = [None, Some(""), Some("deltabadger"), Some("coingecko")];
    let texts = [None, Some(""), Some("v")];
    let mut cases = 0;
    for provider in values { for url in texts { for key in texts { for env_url in [None, Some("http://data-api:3000")] {
        let (dir, c) = install();
        c.execute("DELETE FROM app_configs WHERE key IN ('market_data_provider','coingecko_api_key')", []).unwrap();
        let k = seed::cipher();
        for (name, v) in [("market_data_provider", provider), ("market_data_url", url), ("market_data_token", url), ("coingecko_api_key", key)] {
            if let Some(v) = v { deltabadger::app_config::set(&c, &k, name, v, at(NOW)).unwrap(); }
        }
        let env_map: Vec<(&str, String)> = env_url.map(|u| vec![("MARKET_DATA_URL", u.to_string())]).unwrap_or_default();
        let env = |n: &str| env_map.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone());
        let rows = Rows { provider: provider.map(String::from), url: url.map(String::from), token: url.map(String::from), coingecko_key: key.map(String::from) };
        let expected = decide(&rows, &Env::read(&env));
        assert_eq!(deltabadger::market_data::read(&c, &k, &Env::read(&env)).unwrap(), expected);
        let jobs = deltabadger::jobs::data_api::config(&env, &c, &k).unwrap();
        assert_eq!(jobs, match &expected.provider { Provider::DataApi(cfg) => Some(cfg.clone()), _ => None });
        deltabadger::engine::provider::bind(&c, &k, &env).unwrap();
        assert_eq!(deltabadger::engine::provider::settings(&c).unwrap(), expected, "{provider:?} {url:?} {key:?} {env_url:?}");
        drop(dir);
        cases += 1;
    }}}}
    assert_eq!(cases, 72);
}

/// The budget (PLAN-A17): the seed's 2,629 crypto assets are one plan ping and ⌈2,629/250⌉ = 11 `coins/markets` pages.
#[tokio::test(flavor = "current_thread")]
async fn the_asset_refresh_asks_one_page_per_250_assets() {
    let (d, c) = install();
    let ids: Vec<String> = (0..2629).map(|i| format!("coin-{i:04}")).collect();
    c.execute_batch("BEGIN").unwrap();
    for id in &ids { asset(&c, id, "Cryptocurrency"); }
    c.execute_batch("COMMIT").unwrap();
    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/markets", 200, json!([coin("coin-0000")]));
    let db = Db::new(c, seed::cipher());
    let out = reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await;
    assert_eq!(out, Outcome::Done);
    assert_eq!(t.requests().len(), 12);
    assert_eq!(t.requests().iter().filter(|r| r.path == "/api/v3/coins/markets").count(), 11);
    assert_eq!(symbol(&reopen(&d), "coin-0000"), "COIN-0000");
}

/// A 429 on the second page: nothing refreshed, the run is rate limited, the pages after it are not asked; a ping 429 asks
/// nothing more (D-CG-PLAN-PING); a blank key asks nothing and writes nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_rate_limit_stops_the_refresh_and_keeps_the_last_data() {
    let (d, c) = install();
    for i in 0..600 { asset(&c, &format!("coin-{i:03}"), "Cryptocurrency"); }
    let db = Db::new(c, seed::cipher());
    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/markets", 200, json!([coin("coin-000")])).reply("GET /api/v3/coins/markets", 429, json!({}));
    let out = reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await;
    assert!(matches!(out, Outcome::RateLimited(_)), "{out:?}");
    assert_eq!(t.requests().len(), 3, "ping + 2 pages; the third is not asked");
    assert_eq!(symbol(&reopen(&d), "coin-000"), "coin-000", "nothing refreshed from the first page");

    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 429, json!({}));
    let out = reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await;
    assert!(matches!(out, Outcome::RateLimited(_)), "{out:?}");
    assert_eq!(t.requests().len(), 1);

    deltabadger::app_config::set(&reopen(&d), &seed::cipher(), "coingecko_api_key", "", at(NOW)).unwrap();
    let t = ScriptedTransport::default();
    let out = reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await;
    assert_eq!(out, Outcome::Failed("CoinGecko API key not configured".into()));
    assert!(t.requests().is_empty());
}

/// An answer naming none of the coins asked certifies nothing (Failed, so the job record and CRYPTO_ASSETS do not move);
/// no crypto asset at all is nothing new.
#[tokio::test(flavor = "current_thread")]
async fn an_answer_with_no_coin_asked_certifies_nothing() {
    let (_d, c) = install();
    asset(&c, "bitcoin", "Cryptocurrency");
    let db = Db::new(c, seed::cipher());
    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/markets", 200, json!([]));
    let out = reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await;
    assert!(matches!(out, Outcome::Failed(ref m) if m.contains("answered no coin")), "{out:?}");

    let (_d, c) = install();
    let db = Db::new(c, seed::cipher());
    let t = ScriptedTransport::default();
    assert_eq!(reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await, Outcome::NothingNew);
    assert!(t.requests().is_empty());
}

/// D-CG-ASSET-ROW-SKIPS: a coin without a name is skipped; the assets after it are still refreshed (Rails' dig_or_raise
/// stops the job there).
#[tokio::test(flavor = "current_thread")]
async fn a_coin_without_a_name_is_skipped_and_the_rest_refreshed() {
    let (d, c) = install();
    for id in ["alpha", "broken", "gamma"] { asset(&c, id, "Cryptocurrency"); }
    let db = Db::new(c, seed::cipher());
    let t = ScriptedTransport::default();
    let mut broken = coin("broken");
    broken["name"] = Value::Null;
    demo(&t).reply("GET /api/v3/coins/markets", 200, json!([coin("alpha"), broken, coin("gamma")]));
    assert_eq!(reference::run_once_with::<ScriptedTransport>(reference::ASSETS, None, &clients(&t), cx(&db)).await, Outcome::Done);
    let c = reopen(&d);
    assert_eq!((symbol(&c, "alpha"), symbol(&c, "broken"), symbol(&c, "gamma")), ("ALPHA".into(), "broken".into(), "GAMMA".into()));
}

/// The one-asset job runs under its own scope through the scheduler and never moves the bulk refresh's record.
#[tokio::test(flavor = "current_thread")]
async fn the_one_asset_job_keeps_its_own_record() {
    let (d, c) = install();
    let id = asset(&c, "ethereum", "Cryptocurrency");
    state::record_error(&c, reference::ASSETS, None, at("2026-10-01T00:20:00Z"), "interrupted").unwrap();
    let before = state::read(&c, reference::ASSETS, None).unwrap();
    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/ethereum", 200, json!({ "symbol": "eth", "name": "Ethereum", "image": { "large": "https://img/eth.png" } }));
    let cg = clients(&t);
    // As jobs::resolve builds it in production, with the scripted clients.
    let resolver: deltabadger::jobs::Resolver = Box::new(move |name, scope| {
        assert_eq!(name, reference::ASSET_DATA);
        Ok(Box::new(reference::AssetData { cg: cg.clone(), asset: scope.unwrap().parse().unwrap() }) as Box<dyn Job>)
    });
    // Only the woken job: a scheduled bulk run would race it for the transport.
    let scheduler = deltabadger::jobs::Scheduler::new(c, seed::cipher(), vec![], None).with_resolver(resolver);
    scheduler.wakers().wake(reference::ASSET_DATA, Some(&id.to_string()), None);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let db = || reopen(&d);
    let scope = id.to_string();
    let (ended, ()) = tokio::join!(scheduler.run(stopped, &deltabadger::engine::SystemClock), async {
        for _ in 0..100 {
            if state::read(&db(), reference::ASSET_DATA, Some(&scope)).unwrap().last_run_at.is_some() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let _ = stop.send(true);
    });
    ended.unwrap();
    assert_eq!(symbol(&db(), "ethereum"), "ETH");
    assert!(state::read(&db(), reference::ASSET_DATA, Some(&scope)).unwrap().last_success_at.is_some());
    let after = state::read(&db(), reference::ASSETS, None).unwrap();
    assert_eq!((after.last_success_at, after.last_error_at), (before.last_success_at, before.last_error_at), "the bulk record is untouched");
}

/// Setup sync on CoinGecko: the assets refresh, then the ranking; a 429 in the first step asks nothing more and keeps the
/// request (D-CG-SETUP-DURABLE); a complete run clears the request it served, but not a newer one.
#[tokio::test(flavor = "current_thread")]
async fn setup_sync_runs_assets_then_ranking_and_stops_on_a_rate_limit() {
    let (d, c) = install();
    asset(&c, "bitcoin", "Cryptocurrency");
    setup_sync::request(&c, at(NOW)).unwrap();
    let db = Db::new(c, seed::cipher());
    let job = |t: &ScriptedTransport| setup_sync::SeedAndSync::new(&|_| None, deltabadger::jobs::data_api::DataApi::live).with_coingecko(clients(t));

    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/markets", 429, json!({}));
    job(&t).run(cx(&db), vec![]).await;
    assert_eq!(t.requests().len(), 2, "ping + the assets page; the ranking is not asked");
    assert!(deltabadger::app_config::get_plain(&reopen(&d), setup_sync::REQUEST).unwrap().is_some(), "the request stays");

    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/markets", 200, json!([coin("bitcoin")]))
        .reply("GET /api/v3/coins/markets", 200, json!([{ "id": "bitcoin", "market_cap": 1e12 }]));
    job(&t).run(cx(&db), vec![]).await;
    assert_eq!(t.requests().iter().filter(|r| r.path == "/api/v3/coins/markets").count(), 2, "assets, then the ranking");
    assert!(state::read(&reopen(&d), reference::INDICES, None).unwrap().last_success_at.is_some());
    assert_eq!(deltabadger::app_config::get_plain(&reopen(&d), setup_sync::REQUEST).unwrap(), None, "served: cleared");

    // A newer request arriving during the run (the second save) survives it.
    setup_sync::request(&reopen(&d), at(NOW)).unwrap();
    struct Bump(String, Box<dyn Api>);
    impl Api for Bump {
        fn markets<'a>(&'a self, ids: Option<&'a [String]>, category: Option<&'a str>, per_page: usize, page: usize) -> coingecko::Fut<'a> {
            setup_sync::request(&Connection::open(&self.0).unwrap(), "2026-10-02T00:21:00Z".parse().unwrap()).unwrap();
            self.1.markets(ids, category, per_page, page)
        }
        fn coin<'a>(&'a self, id: &'a str) -> coingecko::Fut<'a> { self.1.coin(id) }
        fn exchange_tickers<'a>(&'a self, id: &'a str, page: usize) -> coingecko::Fut<'a> { self.1.exchange_tickers(id, page) }
    }
    let t = ScriptedTransport::default();
    demo(&t).reply("GET /api/v3/coins/markets", 200, json!([coin("bitcoin")]))
        .reply("GET /api/v3/coins/markets", 200, json!([{ "id": "bitcoin", "market_cap": 1e12 }]));
    let path = d.path().join("production.sqlite3").display().to_string();
    let inner = clients(&t);
    let bumping: Clients = Rc::new(move |k: &str| Box::new(Bump(path.clone(), inner(k))) as Box<dyn Api>);
    setup_sync::SeedAndSync::new(&|_| None, deltabadger::jobs::data_api::DataApi::live).with_coingecko(bumping).run(cx(&db), vec![]).await;
    assert!(deltabadger::app_config::get_plain(&reopen(&d), setup_sync::REQUEST).unwrap().is_some(), "the newer request stays for one more run");
    assert!(setup_sync::unfinished(&reopen(&d), &seed::cipher()).unwrap(), "a start recovers it");
}

/// Staleness on a CoinGecko-fed catalogue: a market-cap basket (which reads CRYPTO_ASSETS; a direct Top Coins bot does not)
/// is fresh after a complete CoinGecko refresh and refused at the tick past the 49 h bound, as on data-api.
#[test]
fn a_coingecko_catalogue_goes_stale_as_a_data_api_one_does() {
    use deltabadger::engine::{model, staleness};
    let (_d, o, s) = common::install();
    let c = o.primary;
    for (k, v) in [("market_data_provider", "coingecko"), ("coingecko_api_key", KEY)] { deltabadger::app_config::set(&c, &seed::cipher(), k, v, at(NOW)).unwrap(); }
    let id = seed::insert_bot(&c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("weighting", json!("market_cap")).weights(&[(s.btc, 1.0)]));
    staleness::seed(&c, at("2026-09-30T00:00:00Z")).unwrap(); // the takeover's record
    state::record_success(&c, reference::ASSETS, None, at(NOW)).unwrap();
    let bot = model::load_bot(&c, id).unwrap();
    assert!(staleness::stale(&c, &bot, at("2026-10-04T00:20:00Z")).unwrap().is_none(), "48 h: fresh");
    let stale = staleness::stale(&c, &bot, at("2026-10-04T01:21:00Z")).unwrap().expect("past 49 h");
    assert_eq!(stale.source, staleness::CRYPTO_ASSETS.name);
}
