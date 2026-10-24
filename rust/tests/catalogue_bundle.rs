//! Self-hosted catalogue (Ibkr plan R2, D-BROKER-SEED, D-BROKER-BUNDLE-BOUND): the bundled snapshot once per start and the
//! public snapshot daily, each source through R1's import protocol, every run through the real Scheduler.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, Seeded};
use deltabadger::engine::{model, staleness, FixedClock};
use deltabadger::jobs::data_api::ApiError;
use deltabadger::jobs::reference::{self, Fetch, FetchFuture};
use deltabadger::jobs::state::{self, JobState};
use deltabadger::jobs::Scheduler;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const T1: &str = "2026-10-24T14:00:00Z";
const T2: &str = "2026-10-25T14:00:00Z";
const RUN: &str = "2026-10-25T16:00:00Z";

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn later(s: &str, minutes: i64) -> String { (at(s) + Duration::minutes(minutes)).to_rfc3339() }
fn primary(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }

fn fixture(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/catalogue_e2e/{name}.json"))).unwrap()).unwrap()
}

/// One source of the snapshot: `(metadata, rows)` as a feed carries them.
fn source(revision: i64, generated_at: &str, rows: Vec<Value>) -> Value {
    json!({ "metadata": { "revision": revision, "generated_at": generated_at, "complete": true, "count": rows.len() }, "data": rows })
}

/// data-api's PublishCatalogueSnapshotJob shape: `metadata.sources.<source>` and `data.<source>`, gzipped.
fn snapshot(sources: &[(&str, Value)]) -> Vec<u8> {
    let (mut meta, mut data) = (json!({}), json!({}));
    for (name, feed) in sources {
        meta[*name] = feed["metadata"].clone();
        data[*name] = feed["data"].clone();
    }
    gzip(json!({ "metadata": { "built_at": RUN, "sources": meta }, "data": data }).to_string().as_bytes())
}
fn gzip(b: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(b).unwrap();
    z.finish().unwrap()
}

/// 1,000 stock assets (Alpaca's stock guard needs that many without a baseline) and their Alpaca listings; asset 0 is AAA0.
fn stocks(revision: i64, generated_at: &str) -> [(&'static str, Value); 2] {
    let assets = (0..1000).map(|i| json!({ "asset_id": format!("stock:F{i}"), "external_id": format!("AAA{i}.US"), "type": "stock",
        "symbol": format!("AAA{i}"), "name": format!("Company {i}"),
        "identifiers": [{ "scheme": "alpaca", "value": format!("us_equity:AAA{i}") }, { "scheme": "eodhd", "value": format!("AAA{i}.US") }] })).collect();
    let listings = (0..1000).map(|i| json!({ "listing_id": format!("NASDAQ:AAA{i}"), "base": format!("AAA{i}"), "quote": "USD",
        "ticker": format!("AAA{i}"), "base_external_id": format!("AAA{i}.US"), "quote_external_id": "USD.FOREX", "fractionable": true, "trading_enabled": true })).collect();
    [("stock_assets", source(revision, generated_at, assets)), ("alpaca_listings", source(revision, generated_at, listings))]
}

/// Alpaca crypto listings for seed_alpaca's bitcoin and the 29 assets `crypto_assets` adds (the guard's minimum is 30).
fn crypto(revision: i64, generated_at: &str) -> (&'static str, Value) {
    let row = |ext: &str, sym: &str| json!({ "symbol": format!("{sym}/USD"), "listing_id": format!("alpaca_crypto:{sym}/USD"), "venue": "alpaca_crypto",
        "venue_scheme": "exchange_slug", "native_symbol": format!("{sym}/USD"), "base_asset_id": format!("crypto:{ext}"), "quote_asset_id": "fiat:USD",
        "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2, "minimum_base_size": "0.000027", "minimum_quote_size": "1",
        "maximum_base_size": "100000", "maximum_quote_size": "200000", "fractionable": true, "trading_enabled": true });
    let mut rows = vec![row("bitcoin", "BTC")];
    rows.extend((0..29).map(|i| row(&format!("coin{i}"), &format!("C{i}"))));
    ("alpaca_crypto", source(revision, generated_at, rows))
}
fn crypto_assets(c: &Connection) {
    for i in 0..29 {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')",
                  [format!("coin{i}"), format!("C{i}")]).unwrap();
    }
}

/// An install with Alpaca (seed_alpaca) and IBKR, and the IBKR fixtures' assets. The IBKR seed is Seeded on the IBKR venue.
fn install() -> (tempfile::TempDir, Seeded, Seeded) {
    let (d, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute("INSERT INTO exchanges (type, name, available, maker_fee, taker_fee, created_at, updated_at) \
               VALUES ('Exchanges::Ibkr', 'Interactive Brokers', 1, '0.0', '0.0', '2026-01-01', '2026-01-01')", []).unwrap();
    let ibkr = Seeded { exchange_id: c.last_insert_rowid(), user_id: s.user_id, btc: s.btc, quote: s.quote, ticker_id: s.ticker_id, api_key_id: s.api_key_id };
    for (ext, symbol, category) in [("EUNL.XETRA", "EUNL", "Stock"), ("VOO.US", "VOO", "Stock"), ("EUR.FOREX", "EUR", "Currency")] {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, '2026-01-01', '2026-01-01')",
                  [ext, symbol, category]).unwrap();
    }
    crypto_assets(c);
    (d, s, ibkr)
}

/// The public snapshot as this test serves it: its body (None: 404) and every URL asked for.
#[derive(Clone, Default)]
struct Remote { body: Rc<RefCell<Option<Vec<u8>>>>, calls: Rc<RefCell<Vec<String>>> }
impl Remote {
    fn serving(body: Vec<u8>) -> Self { let r = Self::default(); *r.body.borrow_mut() = Some(body); r }
}
impl Fetch for Remote {
    fn fetch<'a>(&'a self, url: &'a str) -> FetchFuture<'a> {
        self.calls.borrow_mut().push(url.to_string());
        let body = self.body.borrow().clone();
        Box::pin(async move { body.ok_or(ApiError::Failed { status: Some(404), message: "not found".into() }) })
    }
}

/// The bundle written where this test's catalogue jobs read it.
fn bundle(d: &tempfile::TempDir, bytes: &[u8]) -> PathBuf {
    let path = d.path().join("catalogue.json.gz");
    std::fs::write(&path, bytes).unwrap();
    path
}

/// One run of the catalogue job `name` by the real Scheduler at `now`, with this environment; its state once recorded.
async fn run(d: &tempfile::TempDir, name: &str, env: &[(&str, &str)], remote: &Remote, now: &str) -> JobState {
    let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let lookup = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let jobs = reference::catalogue_jobs(&lookup, d.path().join("catalogue.json.gz"), remote.clone()).into_iter().filter(|j| j.spec().name == name).collect();
    let scheduler = Scheduler::new(primary(d), seed::cipher(), jobs, None);
    scheduler.wakers().wake(name, None, None);
    let (stop, rx) = tokio::sync::watch::channel(false);
    let (clock, now) = (FixedClock(at(now)), at(now));
    let recorded = async {
        loop {
            let st = state::read(&primary(d), name, None).unwrap();
            if st.last_run_at == Some(now) || st.last_error_at == Some(now) { stop.send(true).unwrap(); return st; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    let (ran, st) = tokio::time::timeout(std::time::Duration::from_secs(60), async { tokio::join!(scheduler.run(rx, &clock), recorded) })
        .await.expect("the run is recorded");
    ran.unwrap();
    st
}

fn rec(d: &tempfile::TempDir, job: &str) -> (Option<i64>, Option<i64>, Option<DateTime<Utc>>, bool) {
    let st = state::read(&primary(d), job, None).unwrap();
    (st.revision, st.importing_revision, st.source_at, st.catalogue)
}
/// IBKR conid → (available, trading_enabled).
fn ibkr_flags(d: &tempfile::TempDir, ibkr: i64) -> Vec<(String, bool, bool)> {
    let c = primary(d);
    let mut s = c.prepare("SELECT ticker, available, trading_enabled FROM tickers WHERE exchange_id = ?1 ORDER BY ticker").unwrap();
    s.query_map([ibkr], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
fn count(d: &tempfile::TempDir, sql: &str) -> i64 { primary(d).query_row(sql, [], |r| r.get(0)).unwrap() }
/// Every row of `table`, every column, as SQLite stores it.
fn dump(d: &tempfile::TempDir, table: &str) -> Vec<Vec<String>> {
    let c = primary(d);
    let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY 1")).unwrap();
    let n = s.column_count();
    s.query_map([], |r| (0..n).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect()).unwrap().collect::<Result<_, _>>().unwrap()
}

const OFF: [(&str, &str); 1] = [("IBKR_CATALOGUE_URL", "off")];

#[tokio::test(flavor = "current_thread")]
async fn an_old_bundle_beside_a_newer_remote_snapshot_the_remote_wins() {
    let (d, _, ibkr) = install();
    let [assets, listings] = stocks(1, T1);
    bundle(&d, &snapshot(&[assets, listings, crypto(1, T1), ("ibkr_listings", fixture("feed"))]));
    let st = run(&d, reference::BOOTSTRAP, &[], &Remote::default(), RUN).await;
    assert_eq!(st.last_error, None);
    assert_eq!(rec(&d, reference::IBKR), (Some(1), None, Some(at(T1)), true));
    assert_eq!(ibkr_flags(&d, ibkr.exchange_id), vec![("100001".into(), true, true), ("100002".into(), true, true)]);
    // The remote snapshot is newer: revision 2 of IBKR (VOO revoked) and of the Alpaca sources (one listing gone).
    let [assets, mut listings] = stocks(2, T2);
    listings.1["data"].as_array_mut().unwrap().pop();
    listings.1["metadata"]["count"] = json!(999);
    let remote = Remote::serving(snapshot(&[assets, listings, crypto(2, T2), ("ibkr_listings", fixture("feed_revoked"))]));
    let st = run(&d, reference::REFRESH, &[], &remote, &later(RUN, 1)).await;
    assert_eq!(st.last_error, None);
    assert_eq!(*remote.calls.borrow(), vec![reference::SNAPSHOT_URL.to_string()], "the default URL is the public snapshot");
    let won = |d: &tempfile::TempDir| {
        assert_eq!(rec(d, reference::IBKR), (Some(2), None, Some(at(T2)), true));
        assert_eq!(rec(d, reference::STOCKS), (Some(2), None, Some(at(T2)), true));
        assert_eq!(rec(d, reference::STOCK_ASSETS_FEED), (Some(2), None, Some(at(T2)), true));
        assert_eq!(rec(d, reference::ALPACA_CRYPTO), (Some(2), None, Some(at(T2)), true));
        assert_eq!(ibkr_flags(d, ibkr.exchange_id), vec![("100001".into(), true, true), ("100002".into(), true, false)]);
        assert_eq!(count(d, "SELECT count(*) FROM tickers WHERE ticker = 'AAA999' AND available = 0"), 1, "the complete revision swept it");
    };
    won(&d);
    // A restart reads the old bundle again: an older revision writes nothing.
    let (tickers, assets) = (dump(&d, "tickers"), dump(&d, "assets"));
    let st = run(&d, reference::BOOTSTRAP, &[], &Remote::default(), &later(RUN, 2)).await;
    assert_eq!(st.last_error, None);
    won(&d);
    assert_eq!((dump(&d, "tickers"), dump(&d, "assets")), (tickers, assets));
}

#[tokio::test(flavor = "current_thread")]
async fn fingerprint_invalidation_then_an_upstream_failure_then_refresh_disables_the_listing_on_old_evidence() {
    let (d, _, ibkr) = install();
    bundle(&d, &snapshot(&[("ibkr_listings", fixture("feed"))]));
    run(&d, reference::BOOTSTRAP, &[], &Remote::default(), RUN).await;
    let bot = model::load_bot(&primary(&d), seed::insert_bot(&primary(&d), &ibkr, &BotSpec::weekly(60.0, "2026-10-01 10:00:00"))).unwrap();
    // data-api: VOO's fingerprint changed (disabled at once, revision 2) and its re-resolution failed (no new evidence).
    let remote = Remote::serving(snapshot(&[("ibkr_listings", fixture("feed_revoked_no_evidence"))]));
    let st = run(&d, reference::REFRESH, &[], &remote, &later(RUN, 1)).await;
    assert_eq!(st.last_error, None);
    assert_eq!(rec(&d, reference::IBKR), (Some(2), None, Some(at(T1)), true), "new revision, old evidence");
    assert_eq!(ibkr_flags(&d, ibkr.exchange_id), vec![("100001".into(), true, true), ("100002".into(), true, false)]);
    // Snapshot-fed: the bound is 30 days from that evidence, not 49 h.
    let c = primary(&d);
    assert!(staleness::stale(&c, &bot, at(T1) + Duration::days(29)).unwrap().is_none());
    let stale = staleness::stale(&c, &bot, at(T1) + Duration::days(31)).unwrap().expect("31 days after the evidence");
    assert!(stale.message.contains("IBKR tickers") && stale.message.contains("720h bound"), "{}", stale.message);
}

#[tokio::test(flavor = "current_thread")]
async fn with_refresh_off_a_31_day_old_bundle_leaves_both_brokers_stale() {
    for (days, stale) in [(29, false), (31, true)] {
        let (d, alpaca, ibkr) = install();
        let old = (at(RUN) - Duration::days(days)).to_rfc3339();
        let [assets, listings] = stocks(1, &old);
        let mut feed = fixture("feed");
        feed["metadata"]["generated_at"] = json!(old);
        bundle(&d, &snapshot(&[assets, listings, crypto(1, &old), ("ibkr_listings", feed)]));
        let remote = Remote::default();
        assert_eq!(run(&d, reference::BOOTSTRAP, &OFF, &remote, RUN).await.last_error, None);
        let st = run(&d, reference::REFRESH, &OFF, &remote, &later(RUN, 1)).await;
        assert_eq!((st.last_error, st.last_success_at), (None, None), "off: a run that refreshed nothing");
        assert!(remote.calls.borrow().is_empty());
        let c = primary(&d);
        let crypto_bot = model::load_bot(&c, seed::insert_bot(&c, &alpaca, &BotSpec::weekly(60.0, "2026-10-01 10:00:00"))).unwrap();
        let ibkr_bot = model::load_bot(&c, seed::insert_bot(&c, &ibkr, &BotSpec::weekly(60.0, "2026-10-01 10:00:00"))).unwrap();
        let now = at(RUN) + Duration::minutes(2);
        for bot in [&crypto_bot, &ibkr_bot] {
            assert_eq!(staleness::stale(&c, bot, now).unwrap().is_some(), stale, "{days} days, bot {}", bot.id);
            assert_eq!(matches!(staleness::verdict(&c, bot, now).unwrap(), staleness::Verdict::Stale(_)), stale, "{days} days at the door");
        }
        let report = staleness::report(&c, now).unwrap();
        for name in ["Alpaca crypto tickers", "Alpaca stock tickers", "IBKR tickers"] {
            let line = report.iter().find(|l| l.starts_with(name)).unwrap();
            assert_eq!(line.contains("STALE"), stale, "{line}");
            assert!(line.contains("720h bound"), "{line}");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_corrupt_gzip_writes_nothing() {
    let good = snapshot(&[("ibkr_listings", fixture("feed"))]);
    for broken in [good[..good.len() / 2].to_vec(), b"not gzip at all".to_vec(), gzip(b"{\"metadata\": "), gzip(b"[]")] {
        let (d, _, _) = install();
        let before = (dump(&d, "tickers"), dump(&d, "assets"), dump(&d, "app_configs"));
        bundle(&d, &broken);
        let st = run(&d, reference::BOOTSTRAP, &[], &Remote::default(), RUN).await;
        assert!(st.last_error.as_deref().is_some_and(|m| m.contains("nothing imported")), "{st:?}");
        let remote = Remote::serving(broken.clone());
        let st = run(&d, reference::REFRESH, &[], &remote, &later(RUN, 1)).await;
        assert!(st.last_error.as_deref().is_some_and(|m| m.contains("nothing imported")), "{st:?}");
        let c = primary(&d);
        c.execute("DELETE FROM app_configs WHERE key IN (?1, ?2)", [state::key(reference::BOOTSTRAP, None), state::key(reference::REFRESH, None)]).unwrap();
        assert_eq!((dump(&d, "tickers"), dump(&d, "assets"), dump(&d, "app_configs")), before);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn no_requests_when_refresh_is_off_or_data_api_is_configured() {
    let (d, _, ibkr) = install();
    let remote = Remote::serving(snapshot(&[("ibkr_listings", fixture("feed"))]));
    for env in [&OFF[..], &[("IBKR_CATALOGUE_URL", " OFF ")], &[("MARKET_DATA_URL", "http://data-api:3000"), ("MARKET_DATA_TOKEN", "tok")]] {
        let st = run(&d, reference::REFRESH, env, &remote, RUN).await;
        assert_eq!((st.last_error, st.last_success_at, st.last_run_at), (None, None, Some(at(RUN))), "{env:?}");
    }
    assert!(remote.calls.borrow().is_empty());
    // Hosted: the bundle is not read either, and nothing is imported.
    bundle(&d, &snapshot(&[("ibkr_listings", fixture("feed"))]));
    let st = run(&d, reference::BOOTSTRAP, &[("MARKET_DATA_URL", "http://data-api:3000")], &remote, &later(RUN, 1)).await;
    assert_eq!((st.last_error, st.last_success_at), (None, None));
    assert!(ibkr_flags(&d, ibkr.exchange_id).is_empty());
    // Another URL is fetched as given.
    run(&d, reference::REFRESH, &[("IBKR_CATALOGUE_URL", "https://mirror.example/c.json.gz")], &remote, &later(RUN, 2)).await;
    assert_eq!(*remote.calls.borrow(), vec!["https://mirror.example/c.json.gz".to_string()]);
    assert_eq!(ibkr_flags(&d, ibkr.exchange_id).len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn the_alpaca_legacy_backfill_runs_from_the_bundle() {
    let (d, _, _) = install();
    let c = primary(&d);
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('alpaca_0b5c', 'AAA0', 'Company 0', 'Stock', '2026-01-01', '2026-01-01')", []).unwrap();
    let legacy = c.last_insert_rowid();
    let [assets, listings] = stocks(1, T1);
    bundle(&d, &snapshot(&[assets, listings]));
    let st = run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), RUN).await;
    assert_eq!(st.last_error, None);
    let c = primary(&d);
    let (id, ext): (i64, String) = c.query_row("SELECT id, external_id FROM assets WHERE symbol = 'AAA0'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((id, ext.as_str()), (legacy, "AAA0.US"), "the legacy row takes its canonical id and keeps its own");
    assert!(deltabadger::app_config::get(&c, &seed::cipher(), "stock_canonical_backfill_completed_at").unwrap().is_some());
    assert_eq!(count(&d, "SELECT count(*) FROM tickers t JOIN exchanges e ON e.id = t.exchange_id WHERE e.type = 'Exchanges::Alpaca' AND t.ticker LIKE 'AAA%'"), 1000);
    assert_eq!(rec(&d, reference::STOCKS), (Some(1), None, Some(at(T1)), true));
    // A snapshot source with no evidence yet (data-api's IBKR feed before its first sync) is skipped, not an error.
    bundle(&d, &snapshot(&[("ibkr_listings", json!({ "metadata": { "revision": 1, "generated_at": null, "complete": false, "count": 0 }, "data": [] }))]));
    let st = run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), &later(RUN, 1)).await;
    assert_eq!(st.last_error, None);
    assert_eq!(rec(&d, reference::IBKR), (None, None, None, false));
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_sources_with_null_evidence_and_rows_are_refused() {
    let (d, _, ibkr) = install();
    let mut feed = fixture("feed");
    feed["metadata"]["generated_at"] = Value::Null;
    let [assets, mut listings] = stocks(1, T1);
    listings.1["metadata"]["generated_at"] = Value::Null;
    bundle(&d, &snapshot(&[assets, listings, ("ibkr_listings", feed)]));
    let st = run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), RUN).await;
    let m = st.last_error.unwrap_or_default();
    assert!(m.contains("IBKR feed: rows without evidence") && m.contains(&format!("{}: rows without evidence", reference::STOCKS)), "{m}");
    assert!(ibkr_flags(&d, ibkr.exchange_id).is_empty());
    assert_eq!(rec(&d, reference::STOCKS), (None, None, None, false));
    assert_eq!(rec(&d, reference::STOCK_ASSETS_FEED), (Some(1), None, Some(at(T1)), true), "the source with evidence still imports");
}

/// The legacy-row fixture: an `alpaca_<uuid>` stock whose canonical the bundle's stock assets name.
fn legacy(d: &tempfile::TempDir) -> i64 {
    let c = primary(d);
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('alpaca_0b5c', 'AAA0', 'Company 0', 'Stock', '2026-01-01', '2026-01-01')", []).unwrap();
    c.last_insert_rowid()
}
fn legacy_state(d: &tempfile::TempDir, id: i64) -> (String, bool) {
    let c = primary(d);
    (c.query_row("SELECT external_id FROM assets WHERE id = ?1", [id], |r| r.get(0)).unwrap(),
     deltabadger::app_config::get(&c, &seed::cipher(), "stock_canonical_backfill_completed_at").unwrap().is_some())
}

#[tokio::test(flavor = "current_thread")]
async fn the_legacy_backfill_waits_for_a_stock_assets_payload_that_takes_the_fence() {
    // No evidence (null generated_at): nothing is rewritten and no completion flag is set.
    let (d, _, _) = install();
    let id = legacy(&d);
    let [mut assets, listings] = stocks(1, T1);
    assets.1["metadata"]["generated_at"] = Value::Null; // rows without evidence: refused by validation
    bundle(&d, &snapshot(&[assets, listings]));
    let st = run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), RUN).await;
    assert!(st.last_error.as_deref().is_some_and(|m| m.contains("rows without evidence")), "{st:?}");
    assert_eq!(legacy_state(&d, id), ("alpaca_0b5c".into(), false));
    // An older revision than the one imported: nothing either.
    let (d, _, _) = install();
    let [assets, listings] = stocks(2, T2);
    bundle(&d, &snapshot(&[assets, listings]));
    run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), RUN).await;
    assert_eq!(rec(&d, reference::STOCK_ASSETS_FEED).0, Some(2));
    primary(&d).execute("DELETE FROM app_configs WHERE key = 'stock_canonical_backfill_completed_at'", []).unwrap();
    let older = legacy(&d);
    let [assets, listings] = stocks(1, T1);
    bundle(&d, &snapshot(&[assets, listings]));
    run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), &later(RUN, 1)).await;
    assert_eq!(legacy_state(&d, older), ("alpaca_0b5c".into(), false));
}

#[tokio::test(flavor = "current_thread")]
async fn an_alpaca_listing_whose_asset_is_missing_leaves_a_partial_revision_that_the_same_revision_completes() {
    let (d, alpaca, _) = install();
    let (name, mut feed) = crypto(1, T1);
    let rows = feed["data"].as_array_mut().unwrap();
    let mut late = rows[1].clone();
    for k in ["symbol", "native_symbol"] { late[k] = json!("LATE/USD"); }
    late["listing_id"] = json!("alpaca_crypto:LATE/USD");
    late["base_asset_id"] = json!("crypto:late");
    rows.push(late);
    feed["metadata"]["count"] = json!(31);
    let gz = snapshot(&[(name, feed)]);
    bundle(&d, &gz);
    assert_eq!(run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), RUN).await.last_error, None);
    let st = state::read(&primary(&d), reference::ALPACA_CRYPTO, None).unwrap();
    assert_eq!((st.revision, st.partial), (Some(1), true));
    let pairs = |d: &tempfile::TempDir| count(d, &format!("SELECT count(*) FROM tickers WHERE exchange_id = {} AND available = 1", alpaca.exchange_id));
    assert_eq!(pairs(&d), 30);
    // A ticker the payload lacks is kept: no sweep from a partial import.
    primary(&d).execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('late', 'LATE', 'Late', 'Cryptocurrency', '2026-01-01', '2026-01-01')", []).unwrap();
    assert_eq!(run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), &later(RUN, 1)).await.last_error, None);
    let st = state::read(&primary(&d), reference::ALPACA_CRYPTO, None).unwrap();
    assert_eq!((st.revision, st.partial), (Some(1), false), "the same revision imports again once the asset exists");
    assert_eq!(pairs(&d), 31);
}

#[tokio::test(flavor = "current_thread")]
async fn after_a_handback_a_rails_refresh_counts_for_a_catalogue_fed_source() {
    let (d, alpaca, _) = install();
    let old = (at(RUN) - Duration::days(40)).to_rfc3339();
    bundle(&d, &snapshot(&[crypto(1, &old)]));
    run(&d, reference::BOOTSTRAP, &OFF, &Remote::default(), RUN).await;
    let c = primary(&d);
    let bot = model::load_bot(&c, seed::insert_bot(&c, &alpaca, &BotSpec::weekly(60.0, "2026-10-01 10:00:00"))).unwrap();
    let now = at(RUN) + Duration::days(2);
    assert!(matches!(staleness::verdict(&c, &bot, now).unwrap(), staleness::Verdict::Stale(_)), "40-day-old evidence");
    // Handed back to Rails a day later; Rails' crypto sync refreshes the catalogue; the engine takes the install over again.
    let (back, refreshed, taken) = (at(RUN) + Duration::days(1), at(RUN) + Duration::days(1) + Duration::hours(1), at(RUN) + Duration::days(2));
    for (k, t) in [(staleness::HANDED_BACK_AT, back), (staleness::TAKEN_OVER_AT, taken)] {
        deltabadger::app_config::set_plain(&c, k, &t.to_rfc3339(), t).unwrap();
    }
    c.execute("UPDATE exchange_assets SET updated_at = ?1 WHERE exchange_id = ?2", rusqlite::params![deltabadger::codec::format_time(refreshed), alpaca.exchange_id]).unwrap();
    assert!(matches!(staleness::verdict(&c, &bot, now).unwrap(), staleness::Verdict::Fresh), "Rails' refresh in its window counts");
    let line = staleness::report(&c, now).unwrap().into_iter().find(|l| l.starts_with("Alpaca crypto tickers")).unwrap();
    assert!(!line.contains("STALE") && line.contains("49h bound"), "{line}");
}
