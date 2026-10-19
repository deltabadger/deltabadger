//! The "import history" line (Rust only, 3.0): the step after a ledger publish, the row it writes, and what the page
//! and the modal read from it. The page with no gap stays the page it was (tracker_populated proves the grid).
mod common;
use chrono::{DateTime, Utc};
use common::web::{Browser, Csrf, TestClock};
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::{self, data_api::{Config, DataApi}, notifications::Notifications, Cx, Db, Job, Outcome};
use deltabadger::sync::{balances::{self, ScriptedPrices}, jobs::Connect};
use deltabadger::tracker::{self, cache, gaps};
use deltabadger::venue::{alpaca::{AlpacaVenue, Urls}, http::ScriptedTransport};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{atomic::Ordering::SeqCst, Arc, Mutex};

const AT: &str = "2026-10-01T02:00:00Z";
fn at() -> DateTime<Utc> { AT.parse().unwrap() }
fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn paper() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
#[derive(Clone)]
struct Venues;
impl Connect for Venues { type T = ScriptedTransport; fn connect(&self, _: &Credentials) -> AlpacaVenue<Self::T> { AlpacaVenue::new(ScriptedTransport::from_script(&json!({})), Urls::for_passphrase(Some("paper"))) } }
type Api = Rc<Option<DataApi<ScriptedTransport>>>;

/// data-api answering every historical-price request with `price` on each of `days`.
fn prices_api(price: f64, days: &[&str]) -> Api {
    let ms = |d: &str| d.parse::<chrono::NaiveDate>().unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
    let answer = ok(json!({ "prices": days.iter().map(|d| json!([ms(d), price])).collect::<Vec<_>>() }));
    let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": vec![answer; 60] }));
    Rc::new(Some(DataApi::new(Config { url: "http://data-api:3000".into(), token: "t".into() }, t.clone(), t)))
}

/// An Alpaca account (a dollar deposit, a bitcoin buy of 0.01) whose ledger import is current.
struct Fx { _dir: tempfile::TempDir, s: common::seed::Seeded, file: PathBuf, sent: Arc<Mutex<Vec<String>>> }
impl Fx {
    fn c(&self) -> Connection { let c = Connection::open(&self.file).unwrap(); c.busy_timeout(std::time::Duration::from_secs(5)).unwrap(); c }
    fn db(&self) -> Db {
        let sent = self.sent.clone();
        Db::new(self.c(), common::seed::cipher()).with_notifications(Notifications::new(move |stream, payload| { sent.lock().unwrap().push(format!("{stream} {payload}")); Ok(()) }))
    }
    fn refreshes(&self) -> usize { self.sent.lock().unwrap().iter().filter(|s| s.contains("action=\"refresh\"")).count() }
    fn owner(&self) -> i64 { self.s.user_id }
    fn row(&self) -> Option<Value> { self.c().query_row("SELECT value FROM app_configs WHERE key=?1", [gaps::key(self.owner())], |r| r.get::<_, String>(0)).optional().unwrap().map(|v| serde_json::from_str(&v).unwrap()) }
    fn cache(&self) -> (String, String) { self.c().query_row("SELECT value, updated_at FROM app_configs WHERE key=?1", [cache::key(self.owner())], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() }
    fn exec(&self, sql: &str) { self.c().execute_batch(sql).unwrap(); }
    /// A copy of the install as it is now.
    fn copy(&self) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("production.sqlite3");
        self.c().execute("VACUUM INTO ?1", [file.to_str().unwrap()]).unwrap();
        (dir, file)
    }
    /// A balance sync by the real writer: `cash` dollars and `btc` bitcoin at `price`, at `when`.
    async fn sync(&self, cash: &str, btc: Option<&str>, price: f64, when: DateTime<Utc>) {
        let positions: Vec<Value> = btc.map(|q| json!({ "symbol": "BTCUSD", "asset_class": "crypto", "qty": q, "qty_available": q })).into_iter().collect();
        let venue = AlpacaVenue::new(ScriptedTransport::from_script(&json!({ "GET /v2/account": [ok(json!({ "cash": cash }))], "GET /v2/positions": [ok(json!(positions))] })), Urls::for_passphrase(Some("paper")));
        let prices = ScriptedPrices::from_script(&json!({ "GET /api/v1/prices": [ok(json!({ "data": { "bitcoin": { "usd": price } } }))] }));
        balances::sync(&self.db(), &venue, &prices, self.s.api_key_id, &paper(), &FixedClock(when)).await.unwrap().unwrap();
    }
}
fn stamp_ledger(c: &Connection, key: i64) {
    let v = deltabadger::engine::model::credential_version_by_id(c, key).unwrap().unwrap();
    deltabadger::engine::model::credential_write(c, &Some(v.clone()), |f| deltabadger::sync::cache::record_ledger(f, key, &v, at())).unwrap();
}
#[allow(clippy::too_many_arguments)] // a row's columns
fn insert_row(c: &Connection, user: i64, exchange: i64, kind: i64, base: &str, amount: &str, quote: Option<(&str, &str)>, when: &str) {
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'{}','{}',?8,?8)",
              rusqlite::params![user, exchange, kind, base, amount, quote.map(|q| q.0), quote.map(|q| q.1), when]).unwrap();
}
fn alpaca() -> Fx {
    let (dir, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    common::seed::fresh_stock_jobs(c, at());
    insert_row(c, s.user_id, s.exchange_id, 4, "USD", "1000", None, "2026-09-01 00:00:00");
    insert_row(c, s.user_id, s.exchange_id, 0, "BTC", "0.01", Some(("USD", "600")), "2026-09-02 00:00:00");
    stamp_ledger(c, s.api_key_id);
    let file = dir.path().join("production.sqlite3");
    Fx { _dir: dir, s, file, sent: Arc::default() }
}
/// `alpaca()` with its balances synced: $400 and `btc` bitcoin at $60,000 (0.02: 0.01 arrived, $600).
async fn alpaca_synced(btc: &str) -> Fx { let fx = alpaca(); fx.sync("400", Some(btc), 60000.0, at()).await; fx }
fn job(owner: i64, api: Api) -> Box<dyn Job> { tracker::jobs::resolve::<Venues, ScriptedTransport>(tracker::jobs::TRACKER_LEDGER, owner, &Venues, api, Arc::new(at)).unwrap() }
async fn run(fx: &Fx) -> Outcome { run_with(fx, Rc::new(None)).await }
async fn run_with(fx: &Fx, api: Api) -> Outcome { job(fx.owner(), api).run(Cx { db: fx.db(), clock: &FixedClock(at()), wakers: jobs::Wakers::default() }, vec![]).await }
/// The ledger producer alone, as before this feature (no step): what the cache holds without it.
async fn producer_alone(file: &PathBuf, owner: i64, api: Api) {
    let db = Db::new(Connection::open(file).unwrap(), common::seed::cipher());
    tracker::jobs::ledger_run(&db, (*api).as_ref(), None, owner, &FixedClock(at()), Arc::new(at), &mut tracker::jobs::Allowance::run()).await.unwrap();
}
fn cache_of(file: &PathBuf, owner: i64) -> (String, String) { Connection::open(file).unwrap().query_row("SELECT value, updated_at FROM app_configs WHERE key=?1", [cache::key(owner)], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() }
fn gaps_of(row: &Value) -> &Vec<Value> { row["gaps"].as_array().unwrap() }

/// The step's seams are process-wide; every test that sets one runs in a process of its own (nextest).
fn reset_seams() {
    gaps::seam::STEPS.store(gaps::GAPS.steps, SeqCst);
    gaps::seam::SQL.store(gaps::SQL_INSTRUCTIONS, SeqCst);
    *gaps::seam::AFTER_READ.lock().unwrap() = None;
    gaps::seam::SKIP_READ.store(false, SeqCst);
}

// ---- the step ----------------------------------------------------------------------------------------------------

/// Today's refresh first, then the step, then a second refresh because it wrote: one gap, BTC arrived at the venue's
/// price, judged with the key the scheduler reads with, against the balances the sync wrote.
#[tokio::test(flavor = "current_thread")]
async fn a_venue_holding_more_than_its_history_gets_its_gap_after_todays_refresh() {
    let fx = alpaca_synced("0.02").await;
    assert_eq!(run(&fx).await, Outcome::Done);
    assert_eq!(fx.refreshes(), 2, "today's refresh, then the step's: {:?}", fx.sent.lock().unwrap());
    let row = fx.row().expect("the row");
    assert_eq!(row["v"], 1);
    let published: Value = serde_json::from_str(&fx.cache().0).unwrap();
    for field in ["history", "prices", "expires", "producers"] { assert_eq!(row["from"][field], published[field], "{field} copied verbatim"); }
    let gap = &gaps_of(&row)[0];
    assert_eq!((gap["exchange"].as_i64(), gap["key"].as_i64(), gap["arrived_usd"].as_str()), (Some(fx.s.exchange_id), Some(fx.s.api_key_id), Some("600.0")));
    assert_eq!(gap["notes"], json!([{"kind":"arrived","symbol":"BTC","exchange":"Alpaca","history":"0.01","held":"0.02","amount":"600.0"}]));
    let c = fx.c();
    let origin: String = c.query_row("SELECT updated_at FROM app_configs WHERE key=?1", [deltabadger::sync::cache::balance_origin_key(fx.owner(), fx.s.exchange_id)], |r| r.get(0)).unwrap();
    assert_eq!(gap["balances"]["origin_updated_at"].as_str(), Some(origin.as_str()));
    assert_eq!(deltabadger::sync::cache::balance_origin_key(fx.owner(), fx.s.exchange_id), format!("rust_job.balance_origin:{}:{}", fx.owner(), fx.s.exchange_id));
    assert_eq!(gap["balances"]["rows"].as_i64(), Some(2));
    // Run again over the same publication: `[]`-or-gap is written again, idempotently, and refreshes again.
    assert_eq!(run(&fx).await, Outcome::Done);
    assert_eq!(fx.refreshes(), 4);
    assert_eq!(fx.row().unwrap()["gaps"], row["gaps"]);
}

/// The owner's KuCoin shape: a 2022 bitcoin deposit the venue no longer holds, and ether it holds with no history.
#[tokio::test(flavor = "current_thread")]
async fn kucoin_holding_ether_with_no_history_and_no_bitcoin_has_one_gap_with_both_notes() {
    let fx = alpaca_synced("0.01").await;
    let (kucoin, key) = add_kucoin(&fx);
    insert_row(&fx.c(), fx.owner(), kucoin, 4, "BTC", "0.5", None, "2022-01-10 09:00:00");
    kucoin_balances(&fx, kucoin, key, &[("ETH", "7.4", "2000")]);
    assert_eq!(run_with(&fx, prices_api(40000.0, &["2022-01-09", "2022-01-10"])).await, Outcome::Done);
    let row = fx.row().unwrap();
    assert_eq!(gaps_of(&row).len(), 1, "Alpaca holds what its history says: {row}");
    let gap = &gaps_of(&row)[0];
    assert_eq!((gap["exchange"].as_i64(), gap["key"].as_i64(), gap["arrived_usd"].as_str()), (Some(kucoin), Some(key), Some("14800.0")));
    assert_eq!(gap["notes"], json!([{"kind":"arrived","symbol":"ETH","exchange":"KuCoin","history":"0.0","held":"7.4","amount":"14800.0"},
                                     {"kind":"left","symbol":"BTC","exchange":"KuCoin","history":"0.5","held":"0.0","amount":"20000.0"}]));
}

/// A KuCoin venue beside the Alpaca one, with a correct key; returns (exchange, key).
fn add_kucoin(fx: &Fx) -> (i64, i64) { add_venue(fx, "Exchanges::Kucoin", "KuCoin", 0) }
fn add_venue(fx: &Fx, kind: &str, name: &str, key_type: i64) -> (i64, i64) {
    let c = fx.c();
    let exchange: i64 = c.query_row("SELECT id FROM exchanges WHERE type=?1", [kind], |r| r.get(0)).optional().unwrap().unwrap_or_else(|| {
        c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?2, '0.1', '0.1', '2026-01-01', '2026-01-01')", [kind, name]).unwrap();
        c.last_insert_rowid()
    });
    let cipher = common::seed::cipher();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, '2026-01-01', '2026-01-01')",
              rusqlite::params![fx.owner(), exchange, cipher.encrypt(&format!("{name}-{key_type}-key")), cipher.encrypt("s"), cipher.encrypt("p"), key_type]).unwrap();
    let key = c.last_insert_rowid();
    stamp_ledger(&c, key);
    for (symbol, external) in [("ETH", "ethereum"), ("USDT", "tether"), ("SOL", "solana")] {
        let known: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE symbol=?1)", [symbol], |r| r.get(0)).unwrap();
        if !known { c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')", [external, symbol]).unwrap(); }
    }
    (exchange, key)
}
/// Balances a completed sync of `key` left on `exchange`: (symbol, units, price), synced and priced at AT.
fn kucoin_balances(fx: &Fx, exchange: i64, key: i64, held: &[(&str, &str, &str)]) {
    let c = fx.c();
    for (symbol, units, price) in held {
        let asset: i64 = c.query_row("SELECT id FROM assets WHERE symbol=?1 ORDER BY id LIMIT 1", [symbol], |r| r.get(0)).unwrap();
        c.execute("INSERT INTO account_balances(user_id,exchange_id,asset_id,free,locked,usd_price,usd_value,priced_at,synced_at,created_at,updated_at) VALUES(?1,?2,?3,?4,0,?5,?4*?5,?6,?6,?6,?6)",
                  rusqlite::params![fx.owner(), exchange, asset, units, price, "2026-10-01 02:00:00"]).unwrap();
    }
    c.execute("UPDATE api_keys SET balances_synced_at='2026-10-01 02:00:00' WHERE id=?1", [key]).unwrap();
    let v = deltabadger::engine::model::credential_version_by_id(&c, key).unwrap().unwrap();
    deltabadger::engine::model::credential_write(&c, &Some(v.clone()), |f| deltabadger::sync::cache::record(f, fx.owner(), exchange, &v, true, at())).unwrap();
}

/// No gap unless every rule passes: each case writes `[]`, or (a cold cache) no row and no step at all.
#[tokio::test(flavor = "current_thread")]
async fn no_gap_unless_the_venue_passes_every_rule() {
    struct Case { name: &'static str, cold: bool, setup: fn(&Fx, i64, i64) }
    let cases = [
        Case { name: "ledger stamp of another credential", cold: true, setup: |fx, _, key| { fx.c().execute("UPDATE app_configs SET value='{}' WHERE key=?1", [format!("rust_job.ledger_origin:{key}")]).unwrap(); } },
        Case { name: "ledger run left incomplete", cold: true, setup: |fx, _, key| { deltabadger::jobs::state::mark_incomplete(&fx.c(), deltabadger::sync::jobs::LEDGER_SYNC, Some(&key.to_string()), at()).unwrap(); } },
        Case { name: "balance stamp incomplete", cold: false, setup: |fx, exchange, key| { let c = fx.c(); let v = deltabadger::engine::model::credential_version_by_id(&c, key).unwrap().unwrap(); deltabadger::engine::model::credential_write(&c, &Some(v.clone()), |f| deltabadger::sync::cache::record(f, fx.owner(), exchange, &v, false, at())).unwrap(); } },
        Case { name: "unpriced holding", cold: false, setup: |fx, exchange, _| { fx.c().execute("UPDATE account_balances SET usd_value=NULL, usd_price=NULL WHERE exchange_id=?1", [exchange]).unwrap(); } },
        Case { name: "stale price", cold: false, setup: |fx, exchange, _| { fx.c().execute("UPDATE account_balances SET priced_at='2026-09-30 00:00:00' WHERE exchange_id=?1", [exchange]).unwrap(); } },
        Case { name: "a row since the watermark", cold: false, setup: |fx, exchange, _| { insert_row(&fx.c(), fx.owner(), exchange, 0, "SOL", "1", Some(("USD", "100")), "2026-10-01 02:00:00"); } },
        Case { name: "cash only", cold: false, setup: |fx, exchange, _| { fx.c().execute("DELETE FROM account_balances WHERE exchange_id=?1", [exchange]).unwrap(); kucoin_balances(fx, exchange, kucoin_key(fx), &[("USDT", "5000", "1")]); } },
        Case { name: "a $6 gap", cold: false, setup: |fx, exchange, _| { fx.c().execute("UPDATE account_balances SET free=0.003, usd_value=6 WHERE exchange_id=?1", [exchange]).unwrap(); } },
        Case { name: "left only", cold: false, setup: |fx, exchange, _| { fx.c().execute("DELETE FROM account_balances WHERE exchange_id=?1", [exchange]).unwrap(); kucoin_balances(fx, exchange, kucoin_key(fx), &[("SOL", "0.001", "100")]); insert_row(&fx.c(), fx.owner(), exchange, 0, "BTC", "0.5", Some(("USDT", "20000")), "2022-01-10 09:00:00"); } },
    ];
    for case in cases {
        let fx = alpaca_synced("0.01").await;
        let (kucoin, key) = add_kucoin(&fx);
        kucoin_balances(&fx, kucoin, key, &[("ETH", "0.005", "2000")]);
        // The control: $10 of ether arrived is a gap.
        if case.name == "a $6 gap" {
            let (dir, file) = fx.copy();
            let control = Fx { _dir: dir, s: seeded(&fx.s), file, sent: Arc::default() };
            assert_eq!(run(&control).await, Outcome::Done);
            assert_eq!(gaps_of(&control.row().unwrap()).len(), 1, "$10 is a gap");
        }
        (case.setup)(&fx, kucoin, key);
        assert_eq!(run(&fx).await, Outcome::Done, "{}", case.name);
        match fx.row() {
            Some(row) => { assert!(!case.cold, "{}", case.name); assert_eq!(gaps_of(&row), &Vec::<Value>::new(), "{}: {row}", case.name); assert_eq!(fx.refreshes(), 1, "{}: `[]` shows nothing new: today's refresh only", case.name) }
            None => { assert!(case.cold, "{}", case.name); assert_eq!(fx.refreshes(), 0, "{}: no row, so the cache was cold and no step ran", case.name) }
        }
    }
}
fn seeded(s: &common::seed::Seeded) -> common::seed::Seeded { common::seed::Seeded { user_id: s.user_id, exchange_id: s.exchange_id, btc: s.btc, quote: s.quote, ticker_id: s.ticker_id, api_key_id: s.api_key_id } }
fn kucoin_key(fx: &Fx) -> i64 { fx.c().query_row("SELECT k.id FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE e.type='Exchanges::Kucoin' ORDER BY k.id LIMIT 1", [], |r| r.get(0)).unwrap() }

/// Binance.US stamps a ledger it never read: it is never judged, however much it holds.
#[tokio::test(flavor = "current_thread")]
async fn binance_us_with_a_current_stamp_is_never_judged() {
    let fx = alpaca_synced("0.01").await;
    let (us, key) = add_venue(&fx, "Exchanges::BinanceUs", "Binance.US", 0);
    kucoin_balances(&fx, us, key, &[("ETH", "7.4", "2000")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    assert_eq!(gaps_of(&fx.row().unwrap()), &Vec::<Value>::new());
}

// ---- keys ----------------------------------------------------------------------------------------------------------

/// Two correct keys on one venue: the gap is judged with the one the scheduler reads with, the trading key. A trading
/// key missing a permission beside a read-only key: the scheduler reads with the read-only one, so the trading key's
/// import is never current and the cache stays cold (an existing mismatch, see sync/cache.rs): no step, no line.
#[tokio::test(flavor = "current_thread")]
async fn the_gap_is_judged_with_the_key_the_scheduler_reads_with() {
    let fx = alpaca_synced("0.01").await;
    let (kucoin, trading) = add_venue(&fx, "Exchanges::Kucoin", "KuCoin", 0);
    let (_, read_only) = add_venue(&fx, "Exchanges::Kucoin", "KuCoin", 2);
    kucoin_balances(&fx, kucoin, trading, &[("ETH", "1", "2000")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    let row = fx.row().unwrap();
    assert_eq!((gaps_of(&row).len(), gaps_of(&row)[0]["key"].as_i64()), (1, Some(trading)), "{row}");

    let fx = alpaca_synced("0.01").await;
    let (kucoin, trading) = add_venue(&fx, "Exchanges::Kucoin", "KuCoin", 0);
    let (_, read_only_2) = add_venue(&fx, "Exchanges::Kucoin", "KuCoin", 2);
    let c = fx.c();
    c.execute("UPDATE api_keys SET last_sync_error='{\"code\":\"400007\",\"msg\":\"Access Denied\"}' WHERE id=?1", [trading]).unwrap();
    c.execute("DELETE FROM app_configs WHERE key=?1", [format!("rust_job.ledger_origin:{trading}")]).unwrap();
    kucoin_balances(&fx, kucoin, read_only_2, &[("ETH", "1", "2000")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    assert!(fx.row().is_none(), "the cache is cold: no step ran");
    assert_eq!(fx.refreshes(), 0);
    let _ = read_only;
}

// ---- after an import -------------------------------------------------------------------------------------------------

/// The modal's CSV examples land on the selected venue and its reading key through the real importer; imported, the
/// history explains the balances and the step writes `[]`.
#[tokio::test(flavor = "current_thread")]
async fn the_examples_import_onto_the_venue_and_close_its_gap() {
    let fx = alpaca_synced("0.01").await;
    let (kucoin, key) = add_kucoin(&fx);
    insert_row(&fx.c(), fx.owner(), kucoin, 4, "BTC", "0.5", None, "2022-01-10 09:00:00");
    kucoin_balances(&fx, kucoin, key, &[("ETH", "7.4", "2000"), ("SOL", "3.1", "150"), ("BTC", "0.25", "60000")]);
    let days = ["2022-01-09", "2022-01-10", "2022-05-09", "2022-05-10", "2023-01-04", "2023-01-05", "2026-09-19", "2026-09-20"];
    assert_eq!(run_with(&fx, prices_api(30000.0, &days)).await, Outcome::Done);
    let row = fx.row().unwrap();
    let notes = &gaps_of(&row)[0]["notes"];
    assert_eq!(notes.as_array().unwrap().iter().map(|n| format!("{} {}", n["kind"].as_str().unwrap(), n["symbol"].as_str().unwrap())).collect::<Vec<_>>(), ["arrived ETH", "arrived SOL", "left BTC"]);
    for (venue, name_id) in [(kucoin, "kucoin"), (fx.s.exchange_id, "alpaca")] {
        let file = format!("{}\n{}\n", deltabadger::web::tracker::gap::CSV_HEADER, deltabadger::web::tracker::gap::examples(name_id).join("\n"));
        let c = fx.c();
        let reading = tracker::import::run::reading(&c, fx.owner()).unwrap().into_iter().find(|k| k.exchange_id == venue).unwrap();
        let answer = tracker::import::run::import(&c, fx.owner(), "deltabadger", file.as_bytes(), None, None, at()).unwrap();
        assert!(matches!(answer, tracker::import::run::Answer::Written { imported: 5, .. }), "{name_id}: {answer:?}");
        let landed: Vec<(i64, i64)> = c.prepare("SELECT exchange_id, api_key_id FROM account_transactions WHERE transacted_at IN ('2022-05-10 12:00:00','2026-09-20 08:00:00','2023-01-05 10:00:00') AND exchange_id=?1").unwrap()
            .query_map([venue], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(landed, vec![(venue, reading.id); 5], "{name_id}");
        if venue != kucoin { c.execute("DELETE FROM account_transactions WHERE exchange_id=?1 AND transacted_at < '2026-09-01'", [venue]).unwrap(); c.execute("DELETE FROM account_transactions WHERE exchange_id=?1 AND transacted_at='2026-09-20 08:00:00'", [venue]).unwrap(); }
    }
    assert_eq!(run_with(&fx, prices_api(30000.0, &days)).await, Outcome::Done);
    assert_eq!(gaps_of(&fx.row().unwrap()), &Vec::<Value>::new(), "{}", fx.row().unwrap());
}

// ---- isolation from the producer ---------------------------------------------------------------------------------------

/// Every failure of the step writes nothing, leaves the ledger cache exactly as the producer alone leaves it, keeps
/// today's one refresh and the run Done; a row from an earlier publication stays, unreadable.
#[tokio::test(flavor = "current_thread")]
async fn a_failing_step_writes_nothing_and_loses_nothing() {
    for failure in ["sql", "budget", "sql cap", "row over 64 KiB"] {
        reset_seams();
        let fx = alpaca_synced("0.02").await;
        assert_eq!(run(&fx).await, Outcome::Done);
        let earlier = fx.row().unwrap();
        // A new publication: a later bitcoin buy, so the earlier row's `from` no longer matches.
        insert_row(&fx.c(), fx.owner(), fx.s.exchange_id, 0, "BTC", "0.001", Some(("USD", "60")), "2026-09-03 00:00:00");
        match failure {
            "sql" => fx.c().execute_batch("CREATE TRIGGER refuse_gaps BEFORE UPDATE ON app_configs WHEN NEW.key LIKE 'rust_tracker_gaps.%' BEGIN SELECT RAISE(ABORT,'gap row refused'); END;").unwrap(),
            "budget" => gaps::seam::STEPS.store(1, SeqCst),
            "sql cap" => {
                // Another user's rows since the watermark, which the probe's time range reads, over a lowered cap.
                let c = fx.c();
                c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('other@example.com', 'x', 'Other', 0, '2026-01-01', '2026-01-01')", []).unwrap();
                history(&c, c.last_insert_rowid(), fx.s.exchange_id, 3_000, "2026-10-01 03:00:00");
                gaps::seam::SQL.store(5_000, SeqCst);
            }
            _ => {
                let (kucoin, key) = add_kucoin(&fx);
                let c = fx.c();
                let mut held = vec![];
                for i in 0..20 {
                    let symbol = format!("{}{i}", "X".repeat(4000));
                    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')", [format!("x{i}"), symbol.clone()]).unwrap();
                    held.push(symbol);
                }
                kucoin_balances(&fx, kucoin, key, &held.iter().map(|s| (s.as_str(), "1", "2")).collect::<Vec<_>>());
            }
        }
        let (_twin_dir, twin) = fx.copy();
        let before = fx.refreshes();
        assert_eq!(run(&fx).await, Outcome::Done, "{failure}");
        assert_eq!(fx.refreshes() - before, 1, "{failure}: today's refresh only");
        assert_eq!(fx.row().unwrap(), earlier, "{failure}: the earlier row is left as it was");
        producer_alone(&twin, fx.owner(), Rc::new(None)).await;
        assert_eq!(fx.cache(), cache_of(&twin, fx.owner()), "{failure}: the cache as the producer alone leaves it");
        let c = fx.c();
        let cache::Published::Warm { from, .. } = cache::read_published(&c, fx.owner(), at()).unwrap() else { panic!("{failure}: warm") };
        assert_eq!(gaps::read(&c, fx.owner(), None, Some(&from), None).unwrap(), vec![], "{failure}: the earlier row is not this publication's");
    }
}

/// The step charges nothing to the run's allowance: with exactly the allowance today's producer needs (one step less
/// refuses it) and a step that can afford one step, the cache is what the producer alone leaves, the snapshot rows are
/// written, the allowance is untouched and no row is written. The job itself, so starved, is Done with one refresh;
/// with its budget back, it republishes and writes the gap.
#[tokio::test(flavor = "current_thread")]
async fn the_step_runs_on_its_own_budget_after_the_tightest_publish() {
    reset_seams();
    let fx = alpaca_synced("0.02").await;
    let copies: Vec<(tempfile::TempDir, PathBuf)> = (0..4).map(|_| fx.copy()).collect();
    let ledger = |file: &PathBuf, steps: u64| {
        let db = Db::new(Connection::open(file).unwrap(), common::seed::cipher());
        async move { let mut allowance = tracker::jobs::Allowance { steps, stored: 0 }; let out = tracker::jobs::ledger_run(&db, None::<&DataApi<ScriptedTransport>>, None, fx_owner(&db).await, &FixedClock(at()), Arc::new(at), &mut allowance).await.map(|_| ()); (out, allowance, db) }
    };
    let (out, left, _) = ledger(&copies[0].1, tracker::jobs::WALK.steps).await;
    out.unwrap();
    // The smallest allowance that publishes: one step less refuses it.
    let (mut refused, mut needed) = (0, tracker::jobs::WALK.steps - left.steps);
    while needed - refused > 1 {
        let mid = refused + (needed - refused) / 2;
        let (_dir, file) = fx.copy();
        if ledger(&file, mid).await.0.is_ok() { needed = mid } else { refused = mid }
    }
    assert!(ledger(&copies[1].1, needed - 1).await.0.is_err(), "one step less refuses the publish");
    eprintln!("the tightest allowance that publishes: {needed} steps");
    let (out, left, db) = ledger(&copies[2].1, needed).await;
    out.unwrap();
    assert_eq!(left.steps, 0);
    gaps::seam::STEPS.store(1, SeqCst);
    assert!(!gaps::derive_logged(&db, fx.owner(), Arc::new(at)).await, "the starved step writes nothing");
    assert_eq!(left.steps, 0);
    assert_eq!(cache_of(&copies[2].1, fx.owner()), cache_of(&copies[0].1, fx.owner()), "the cache as the producer alone left it");
    let snapshots = |file: &PathBuf| Connection::open(file).unwrap().query_row("SELECT (SELECT count(*) FROM portfolio_snapshots) || '/' || (SELECT count(*) FROM portfolio_venue_snapshots)", [], |r| r.get::<_, String>(0)).unwrap();
    assert_eq!(snapshots(&copies[2].1), snapshots(&copies[0].1));
    assert_ne!(snapshots(&copies[2].1), "0/0");
    assert!(Connection::open(&copies[2].1).unwrap().query_row("SELECT 1 FROM app_configs WHERE key=?1", [gaps::key(fx.owner())], |_| Ok(())).optional().unwrap().is_none());
    // The job, starved the same way, then with its budget back.
    let jobbed = Fx { _dir: tempfile::tempdir().unwrap(), s: seeded(&fx.s), file: copies[3].1.clone(), sent: Arc::default() };
    assert_eq!(run(&jobbed).await, Outcome::Done);
    assert_eq!((jobbed.refreshes(), jobbed.row()), (1, None));
    assert_eq!(jobbed.cache(), cache_of(&copies[0].1, fx.owner()));
    gaps::seam::STEPS.store(gaps::GAPS.steps, SeqCst);
    jobbed.exec("DELETE FROM portfolio_venue_snapshots");
    assert_eq!(run(&jobbed).await, Outcome::Done);
    assert_eq!(jobbed.refreshes(), 3);
    assert_eq!(gaps_of(&jobbed.row().unwrap()).len(), 1);
    assert_ne!(snapshots(&jobbed.file).split('/').nth(1), Some("0"), "the venue snapshot written again");
}
async fn fx_owner(db: &Db) -> i64 { db.run(|c, _| c.query_row("SELECT id FROM users ORDER BY id LIMIT 1", [], |r| r.get(0)).map_err(|e| e.to_string())).await.unwrap() }

// ---- the SQL bound ------------------------------------------------------------------------------------------------------

fn instructions<T>(c: &Connection, work: impl FnOnce(&Connection) -> T) -> (T, u64) {
    let counted = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = counted.clone();
    c.progress_handler(1, Some(move || { counter.fetch_add(1, SeqCst); false }));
    let out = work(c);
    c.progress_handler(0, None::<fn() -> bool>);
    (out, counted.load(SeqCst))
}
fn history(c: &Connection, user: i64, exchange: i64, rows: i64, when: &str) { history_of(c, user, exchange, rows, when, "0, 'BTC', 0.00001, 'USD', 0.6") }
/// `rows` rows of `what` (entry type, base, amount, quote, quote amount) for `user` on `exchange` at `when`.
fn history_of(c: &Connection, user: i64, exchange: i64, rows: i64, when: &str, what: &str) {
    c.execute_batch(&format!("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {rows})
        INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, transacted_at, raw_data, manual_values, created_at, updated_at)
        SELECT {user}, {exchange}, {what}, '{when}', '{{}}', '{{}}', '{when}', '{when}' FROM n")).unwrap();
}

/// The pending probe searches the time index from the watermark on (never the user's index, which reads their whole
/// history): 50,000 rows before the watermark cost it nothing; a watermark at the epoch over 600,000 rows of another
/// user's trips the step's cap, which writes nothing and loses nothing.
#[tokio::test(flavor = "current_thread")]
async fn the_pending_probe_is_bounded_by_the_time_index_and_the_cap() {
    reset_seams();
    let fx = alpaca_synced("0.02").await;
    let c = fx.c();
    let plan: Vec<String> = c.prepare(&format!("EXPLAIN QUERY PLAN {}", gaps::pending_sql())).unwrap().query_map(rusqlite::params!["2026-10-01 02:00:00", 1, 1], |r| r.get(3)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(plan, ["SEARCH account_transactions USING INDEX index_account_transactions_on_transacted_at (transacted_at>?)"]);
    history(&c, fx.owner(), fx.s.exchange_id, 50_000, "2026-09-05 00:00:00");
    let (found, cost) = instructions(&c, |c| c.query_row(&gaps::pending_sql(), rusqlite::params!["2026-10-01 02:00:00", fx.owner(), fx.s.exchange_id], |_| Ok(())).optional().unwrap());
    assert!(found.is_none() && cost < 1000, "the probe took {cost} instructions past 50,000 earlier rows");
    eprintln!("probe past 50,000 earlier rows: {cost} VM instructions");
    c.execute("DELETE FROM account_transactions WHERE transacted_at='2026-09-05 00:00:00'", []).unwrap();
    // Another user's history, and this venue's watermark at the epoch.
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('other@example.com', 'x', 'Other', 0, '2026-01-01', '2026-01-01')", []).unwrap();
    // Dated before this user's own rows, which the probe would otherwise find first.
    history(&c, c.last_insert_rowid(), fx.s.exchange_id, 600_000, "2026-08-01 00:00:00");
    let (_, cost) = instructions(&c, |c| c.query_row(&gaps::pending_sql(), rusqlite::params!["1970-01-01 00:00:00", fx.owner(), 999], |_| Ok(())).optional().unwrap());
    eprintln!("probe from the epoch over 600,000 rows: {cost} VM instructions");
    assert!(cost > gaps::SQL_INSTRUCTIONS);
    c.execute("UPDATE api_keys SET balances_synced_at='1970-01-01 00:00:00' WHERE id=?1", [fx.s.api_key_id]).unwrap();
    assert_eq!(run(&fx).await, Outcome::Done);
    assert_eq!((fx.refreshes(), fx.row()), (1, None), "the cap tripped: nothing written, today's refresh kept");
}

/// The step reuses the cache's validation, which counts the owner's history (`history_version`), and that count is
/// inside the cap: a history large enough ends the step before it judges a venue, with no row.
#[tokio::test(flavor = "current_thread")]
async fn the_cache_validation_the_step_reuses_is_inside_its_cap() {
    reset_seams();
    let fx = alpaca_synced("0.02").await;
    assert_eq!(run(&fx).await, Outcome::Done);
    let c = fx.c();
    c.execute("DELETE FROM app_configs WHERE key=?1", [gaps::key(fx.owner())]).unwrap();
    let mut per_row = 0;
    for rows in [10_000, 50_000] {
        history(&c, fx.owner(), fx.s.exchange_id, rows - c.query_row("SELECT count(*) FROM account_transactions WHERE user_id=?1", [fx.owner()], |r| r.get::<_, i64>(0)).unwrap(), "2026-09-05 00:00:00");
        let (_, cost) = instructions(&c, |c| deltabadger::tracker::backfill::history_version(c, fx.owner()).unwrap());
        per_row = cost / rows as u64;
        eprintln!("history_version over {rows} rows: {cost} VM instructions");
    }
    let enough = (gaps::SQL_INSTRUCTIONS / per_row.max(1)) as i64 + 20_000;
    let have: i64 = c.query_row("SELECT count(*) FROM account_transactions WHERE user_id=?1", [fx.owner()], |r| r.get(0)).unwrap();
    history(&c, fx.owner(), fx.s.exchange_id, enough - have, "2026-09-05 00:00:00");
    eprintln!("a history of {enough} rows exhausts the step's cap in the cache's validation");
    let out = gaps::derive(&c, fx.owner(), at());
    assert!(out.is_err(), "{out:?}");
    assert!(fx.row().is_none());
}

// ---- a deadline during the step -------------------------------------------------------------------------------------

/// The job under the scheduler, its deadline passing while the step is held after its reads: today's refresh was sent
/// before the step began and is the only one; the figures stay as published; once released, the blocking unit runs on
/// and writes the row when its publication is still current, and nothing when another publication landed meanwhile
/// (published on a connection of its own: the held unit keeps the job's). The deadline is recorded after the release
/// (the scheduler's completion waits for the unit to settle).
#[tokio::test(flavor = "current_thread")]
async fn a_deadline_during_the_step_loses_only_its_second_refresh() {
    for superseded in [false, true] {
        reset_seams();
        let fx = alpaca_synced("0.02").await;
        let (entered_tx, entered) = std::sync::mpsc::channel::<()>();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = Mutex::new(released);
        *gaps::seam::AFTER_READ.lock().unwrap() = Some(Box::new(move || { entered_tx.send(()).unwrap(); released.lock().unwrap().recv().unwrap(); }));
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let short = Short { inner: job(fx.owner(), Rc::new(None)), dropped: dropped.clone() };
        let sent = fx.sent.clone();
        let notifications = Notifications::new(move |stream, payload| { sent.lock().unwrap().push(format!("{stream} {payload}")); Ok(()) });
        let scheduler = jobs::Scheduler::new(fx.c(), common::seed::cipher(), vec![Box::new(short)], None).with_notifications(notifications);
        scheduler.wakers().wake(tracker::jobs::TRACKER_LEDGER, Some(&fx.owner().to_string()), None);
        let (stop, rx) = tokio::sync::watch::channel(false);
        let owner = fx.owner();
        let file = fx.file.clone();
        let driver = async {
            let waited = std::time::Instant::now();
            while entered.try_recv().is_err() { assert!(waited.elapsed() < std::time::Duration::from_secs(60), "the step never began"); tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
            assert_eq!(fx.sent.lock().unwrap().clone(), vec![format!("user_{owner}:sync <turbo-stream action=\"refresh\"></turbo-stream>")], "today's refresh, before the step");
            let held_cache = fx.cache();
            while !dropped.load(SeqCst) { assert!(waited.elapsed() < std::time::Duration::from_secs(60)); tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
            if superseded {
                // Another publication on a connection of its own: a later wall clock, so a new `expires`.
                let db = Db::new(Connection::open(&file).unwrap(), common::seed::cipher());
                let later = at() + chrono::Duration::minutes(5);
                tracker::jobs::ledger_run(&db, None::<&DataApi<ScriptedTransport>>, None, owner, &FixedClock(later), Arc::new(move || later), &mut tracker::jobs::Allowance::run()).await.unwrap();
            }
            release.send(()).unwrap();
            loop {
                let failing = Connection::open(&file).ok().and_then(|c| jobs::state::read(&c, tracker::jobs::TRACKER_LEDGER, Some(&owner.to_string())).ok()).is_some_and(|s| s.failing());
                if failing { break; }
                assert!(waited.elapsed() < std::time::Duration::from_secs(60), "the deadline was never recorded");
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            stop.send(true).unwrap();
            held_cache
        };
        let clock = FixedClock(at());
        let (result, held_cache) = tokio::join!(scheduler.run(rx, &clock), driver);
        result.unwrap();
        assert_eq!(fx.refreshes(), 1, "{superseded}: {:?}", fx.sent.lock().unwrap());
        assert!(matches!(cache::read(&fx.c(), owner, None, at()).unwrap(), cache::State::Warm(_)), "the figures stay readable");
        assert_ne!(fx.c().query_row("SELECT count(*) FROM portfolio_venue_snapshots WHERE user_id=?1", [owner], |r| r.get::<_, i64>(0)).unwrap(), 0);
        if superseded {
            assert!(fx.row().is_none(), "the late write found another publication and wrote nothing");
        } else {
            assert_eq!(fx.cache(), held_cache, "the figures as published");
            assert_eq!(gaps_of(&fx.row().expect("the late write, its publication current")).len(), 1);
        }
    }
}
/// The ledger job with a short deadline; its future's drop (the deadline) is observed.
struct Short { inner: Box<dyn Job>, dropped: Arc<std::sync::atomic::AtomicBool> }
struct OnDrop(Arc<std::sync::atomic::AtomicBool>);
impl Drop for OnDrop { fn drop(&mut self) { self.0.store(true, SeqCst); } }
impl Job for Short {
    fn spec(&self) -> jobs::Spec { jobs::Spec { deadline: std::time::Duration::from_secs(5), ..self.inner.spec() } }
    fn run<'a>(&'a self, cx: Cx<'a>, wakes: Vec<jobs::Wake>) -> jobs::JobFuture<'a> {
        let guard = OnDrop(self.dropped.clone());
        Box::pin(async move { let _guard = guard; self.inner.run(cx, wakes).await })
    }
}

// ---- the page and the modal ---------------------------------------------------------------------------------------------

fn app(fx: &Fx) -> deltabadger::web::App {
    let app = common::web::app(fx._dir.path(), common::web::SECRET, TestClock::at(AT));
    // A venue other than Alpaca needs a market-data provider for the populated page (as the setup leaves one).
    for (key, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(&fx.c(), &app.cipher, key, value, app.now()).unwrap();
    }
    app
}
fn browser(fx: &Fx, app: &deltabadger::web::App) -> Browser {
    use deltabadger::web::session;
    let hash: String = fx.c().query_row("SELECT encrypted_password FROM users WHERE id=?1", [fx.owner()], |r| r.get(0)).unwrap();
    let data = session::SessionData { user: Some((fx.owner(), hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None }
}
async fn get(fx: &Fx, app: &deltabadger::web::App, path: &str) -> common::web::Answer {
    let answer = browser(fx, app).send(app, "GET", path, None, Csrf::None, &[]).await;
    assert_eq!(answer.status, 200, "{path}: {}", &answer.body[..answer.body.len().min(400)]);
    answer
}
/// The gap line of `exchange` on a page, if any.
fn line(body: &str, exchange: i64) -> Option<String> {
    let start = body.find(&format!("<tr id=\"history_gap_{exchange}\""))?;
    Some(body[start..start + body[start..].find("</tr>").unwrap() + 5].to_string())
}
/// A page with what differs between two renders of it masked (its CSRF tokens).
fn stable(body: &str) -> String {
    let re = regex::Regex::new(r#"(name="csrf-token" content="|name="authenticity_token" value="|name="csp-nonce" content="|nonce=")[^"]*"#).unwrap();
    re.replace_all(body, "$1•").into_owned()
}

/// The line: at the top of the transactions table, under All and in the venue's own view, its button the modal's; the
/// columns the header has (fewer with balances hidden); localized, with the locale kept in its link.
#[tokio::test(flavor = "current_thread")]
async fn the_line_sits_atop_the_table_in_the_headers_columns() {
    let fx = alpaca_synced("0.02").await;
    assert_eq!(run(&fx).await, Outcome::Done);
    let app = app(&fx);
    let id = fx.s.exchange_id;
    let all = get(&fx, &app, "/tracker").await.body;
    let shown = line(&all, id).expect("the line under All");
    assert!(shown.starts_with(&format!("<tr id=\"history_gap_{id}\" class=\"tracker-row\" data-order-filter-target=\"row\" data-order-type=\"all\">\n  <td class=\"text-left tracker-row__when table__when\">\n    <a class=\"rbutton rbutton--small\" data-turbo-frame=\"modal\" href=\"/tracker/history_gap?exchange_id={id}\">Import history</a>\n  </td>\n  <td class=\"tracker-exchange-icon\">")), "{shown}");
    assert_eq!(shown.matches("<td").count(), 9, "date, venue, type, asset, amount, price, value, fee, bot");
    let tbody = all.find("<tbody>\n").unwrap();
    assert!(all[tbody..].find("history_gap_").unwrap() < all[tbody..].find("account_transaction_").unwrap(), "above the newest row");
    assert!(line(&get(&fx, &app, &format!("/tracker?exchange_id={id}")).await.body, id).is_some(), "the venue's own view");
    fx.exec("UPDATE users SET hide_balances=1");
    let hidden = line(&get(&fx, &app, "/tracker").await.body, id).unwrap();
    assert_eq!(hidden.matches("<td").count(), 6, "no amount, value or fee with balances hidden: {hidden}");
    assert!(!hidden.contains("tracker-row__value") && !hidden.contains("tracker-row__fee"));
    fx.exec("UPDATE users SET hide_balances=0");
    let de = line(&get(&fx, &app, "/de/tracker").await.body, id).unwrap();
    assert!(de.contains(&format!("href=\"/de/tracker/history_gap?exchange_id={id}\">Historie importieren</a>")), "{de}");
}

/// A venue with a gap and no transaction of its own: its view draws the table with the line, not "no transactions".
#[tokio::test(flavor = "current_thread")]
async fn a_venue_with_no_rows_shows_the_table_with_its_line() {
    let fx = alpaca_synced("0.01").await;
    let (kucoin, key) = add_kucoin(&fx);
    kucoin_balances(&fx, kucoin, key, &[("ETH", "1", "2000")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    let app = app(&fx);
    let body = get(&fx, &app, &format!("/tracker?exchange_id={kucoin}")).await.body;
    assert!(line(&body, kucoin).is_some() && body.contains("<thead>") && !body.contains("tracker-empty"), "{body}");
    assert!(line(&get(&fx, &app, &format!("/tracker?exchange_id={}", fx.s.exchange_id)).await.body, kucoin).is_none(), "another venue's view has no line of KuCoin's");
}

/// The balances the gap was judged on, re-synced by the real writer without the producer running: the same balances
/// (only the stamps' times move), a changed quantity, an asset gone. Each time, no line on the page and no evidence
/// in the modal. A missing revision source is no line either.
#[tokio::test(flavor = "current_thread")]
async fn a_balance_sync_since_the_step_hides_the_line_until_it_runs_again() {
    let later = at() + chrono::Duration::seconds(60);
    for change in ["same balances", "changed quantity", "asset gone", "origin row deleted", "stored revision absent"] {
        let fx = alpaca_synced("0.02").await;
        assert_eq!(run(&fx).await, Outcome::Done);
        let app = app(&fx);
        let id = fx.s.exchange_id;
        assert!(line(&get(&fx, &app, "/tracker").await.body, id).is_some(), "{change}: the line before");
        let evidence = |body: &str| body.contains("What the tracker found");
        assert!(evidence(&get(&fx, &app, &format!("/tracker/history_gap?exchange_id={id}")).await.body));
        match change {
            "same balances" => fx.sync("400", Some("0.02"), 60000.0, later).await,
            "changed quantity" => fx.sync("400", Some("0.03"), 60000.0, later).await,
            "asset gone" => fx.sync("400", None, 60000.0, later).await,
            "origin row deleted" => fx.exec(&format!("DELETE FROM app_configs WHERE key='{}'", deltabadger::sync::cache::balance_origin_key(fx.owner(), id))),
            _ => {
                let mut row = fx.row().unwrap();
                row["gaps"][0].as_object_mut().unwrap().remove("balances");
                fx.c().execute("UPDATE app_configs SET value=?1 WHERE key=?2", [row.to_string(), gaps::key(fx.owner())]).unwrap();
            }
        }
        assert!(matches!(cache::read(&fx.c(), fx.owner(), None, at()).unwrap(), cache::State::Warm(_)), "{change}: the producer has not run");
        assert!(line(&get(&fx, &app, "/tracker").await.body, id).is_none(), "{change}: no line");
        assert!(!evidence(&get(&fx, &app, &format!("/tracker/history_gap?exchange_id={id}")).await.body), "{change}: no evidence");
    }
}

/// Anything wrong with the row is no line, the page exactly the page with no row, and the figures still warm.
#[tokio::test(flavor = "current_thread")]
async fn a_row_that_cannot_be_trusted_leaves_the_page_as_it_was() {
    let fx = alpaca_synced("0.02").await;
    assert_eq!(run(&fx).await, Outcome::Done);
    let app = app(&fx);
    let good = fx.row().unwrap();
    fx.exec(&format!("DELETE FROM app_configs WHERE key='{}'", gaps::key(fx.owner())));
    let without = stable(&get(&fx, &app, "/tracker").await.body);
    assert!(without.contains("tracker-positions") && !without.contains("history_gap_"));
    let bad = |edit: &dyn Fn(&mut Value)| { let mut row = good.clone(); edit(&mut row); row.to_string() };
    let cases: Vec<(&str, String)> = vec![
        ("not JSON", "{".into()),
        ("another version", bad(&|r| r["v"] = json!(2))),
        ("another publication", bad(&|r| r["from"]["expires"] = json!("2026-10-31T02:00:01+00:00"))),
        ("a decimal over the bound", bad(&|r| r["gaps"][0]["arrived_usd"] = json!(format!("1.{}", "0".repeat(70))))),
        ("an unknown exchange", bad(&|r| r["gaps"][0]["exchange"] = json!(9999))),
        ("another key", bad(&|r| r["gaps"][0]["key"] = json!(9999))),
        ("an unknown note", bad(&|r| r["gaps"][0]["notes"][0]["kind"] = json!("since_sync"))),
    ];
    for (name, raw) in cases {
        deltabadger::app_config::set_plain(&fx.c(), &gaps::key(fx.owner()), &raw, at()).unwrap();
        assert_eq!(stable(&get(&fx, &app, "/tracker").await.body), without, "{name}");
    }
    // An error from the read itself (its revision query refused): logged, and the page as it was.
    deltabadger::app_config::set_plain(&fx.c(), &gaps::key(fx.owner()), &good.to_string(), at()).unwrap();
    assert!(line(&get(&fx, &app, "/tracker").await.body, fx.s.exchange_id).is_some());
    app.db(|c| { c.authorizer(Some(|a: rusqlite::hooks::AuthContext<'_>| match a.action {
        rusqlite::hooks::AuthAction::Read { table_name: "account_balances", column_name: "updated_at" } => rusqlite::hooks::Authorization::Deny,
        _ => rusqlite::hooks::Authorization::Allow,
    })); Ok(()) }).await.unwrap();
    assert_eq!(stable(&get(&fx, &app, "/tracker").await.body), without, "an Err from gaps::read");
    app.db(|c| { c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>); Ok(()) }).await.unwrap();
}

static TRACE: Mutex<Vec<String>> = Mutex::new(vec![]);
fn traced(event: rusqlite::trace::TraceEvent<'_>) {
    if let rusqlite::trace::TraceEvent::Stmt(statement, sql) = event { TRACE.lock().unwrap().push(statement.expanded_sql().unwrap_or_else(|| sql.to_string())); }
}
/// The statements one GET runs on the app's connection, with or without the gap read (`SKIP_READ`: the page as it was).
async fn statements(fx: &Fx, app: &deltabadger::web::App, path: &str, skip: bool) -> Vec<String> {
    gaps::seam::SKIP_READ.store(skip, SeqCst);
    app.db(|c| { c.trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, Some(traced)); Ok(()) }).await.unwrap();
    TRACE.lock().unwrap().clear();
    get(fx, app, path).await;
    app.db(|c| { c.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None); Ok(()) }).await.unwrap();
    gaps::seam::SKIP_READ.store(false, SeqCst);
    std::mem::take(&mut *TRACE.lock().unwrap())
}
/// What `actual` runs beyond `baseline`, in order: `baseline` must be `actual` with those statements taken out.
fn added(baseline: &[String], actual: &[String]) -> Vec<String> {
    let (mut extra, mut i) = (vec![], 0);
    for s in actual { if i < baseline.len() && *s == baseline[i] { i += 1 } else { extra.push(s.clone()) } }
    assert_eq!(i, baseline.len(), "every statement of the page as it was still runs, in order");
    extra
}

/// What the line costs a page: one app_configs read on a clean account; with a gap shown, the reading keys once and
/// the venue's two revision reads. Nothing added reads the history or the credentials. The modal reads the reading
/// keys once. Measured over a 50,000-row history.
#[tokio::test(flavor = "current_thread")]
async fn the_line_costs_a_page_one_read_and_a_shown_gap_three_more() {
    reset_seams();
    let fx = alpaca_synced("0.02").await;
    // Dollars in, which leave the bitcoin gap as it is.
    history_of(&fx.c(), fx.owner(), fx.s.exchange_id, 50_000, "2026-08-01 00:00:00", "4, 'USD', 1, NULL, NULL");
    assert_eq!(run(&fx).await, Outcome::Done);
    let (owner, id) = (fx.owner(), fx.s.exchange_id);
    let app = app(&fx);
    let gaps_read = format!("SELECT value FROM app_configs WHERE key = 'rust_tracker_gaps.{owner}'");
    let reading = |s: &String| s.starts_with("SELECT k.id, k.exchange_id, k.key_type, k.last_sync_error, e.type, e.name FROM api_keys k");
    let origin = format!("SELECT updated_at FROM app_configs WHERE key = 'rust_job.balance_origin:{owner}:{id}'");
    let revision = format!("SELECT COUNT(*), MAX(updated_at) FROM account_balances WHERE user_id = {owner} AND exchange_id = {id}");
    for path in ["/tracker".to_string(), format!("/tracker?exchange_id={id}")] {
        let extra = added(&statements(&fx, &app, &path, true).await, &statements(&fx, &app, &path, false).await);
        assert_eq!(extra.len(), 4, "{path}: {extra:#?}");
        assert_eq!((extra[0].as_str(), reading(&extra[1]), extra[2].as_str(), extra[3].as_str()), (gaps_read.as_str(), true, origin.as_str(), revision.as_str()), "{path}");
        eprintln!("{path}, one gap shown: +{} statements: {extra:#?}", extra.len());
    }
    // A clean account: the step looked and found nothing.
    let mut row = fx.row().unwrap();
    row["gaps"] = json!([]);
    deltabadger::app_config::set_plain(&fx.c(), &gaps::key(owner), &row.to_string(), at()).unwrap();
    for path in ["/tracker".to_string(), format!("/tracker?exchange_id={id}")] {
        let extra = added(&statements(&fx, &app, &path, true).await, &statements(&fx, &app, &path, false).await);
        assert_eq!(extra, std::slice::from_ref(&gaps_read), "{path}");
    }
    // The modal, with no gap stored and with one: the reading keys once either way.
    for stored in [false, true] {
        if stored { run(&fx).await; }
        let modal = statements(&fx, &app, &format!("/tracker/history_gap?exchange_id={id}"), false).await;
        assert_eq!(modal.iter().filter(|s| reading(s)).count(), 1, "{stored}: {modal:#?}");
        assert_eq!(modal.iter().filter(|s| **s == gaps_read).count(), 1);
        assert_eq!((modal.contains(&origin), modal.contains(&revision)), (stored, stored));
        eprintln!("modal, gap stored {stored}: {} statements", modal.len());
    }
    // The page never runs the step: only the job does.
    let before = gaps::seam::DERIVED.load(SeqCst);
    get(&fx, &app, "/tracker").await;
    get(&fx, &app, &format!("/tracker/history_gap?exchange_id={id}")).await;
    assert_eq!(gaps::seam::DERIVED.load(SeqCst), before);
    run(&fx).await;
    assert_eq!(gaps::seam::DERIVED.load(SeqCst), before + 1);
}

/// `gaps::derive` has one caller, the ledger job, after today's refresh.
#[test]
fn the_step_has_one_caller() {
    fn sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { sources(&path, out) } else if path.extension().is_some_and(|e| e == "rs") { out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap())); }
        }
    }
    let mut all = vec![];
    sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut all);
    let callers: Vec<&String> = all.iter().filter(|(path, body)| !path.ends_with("gaps.rs") && (body.contains("gaps::derive") || body.contains("derive_logged("))).map(|(path, _)| path).collect();
    assert_eq!(callers.len(), 1, "{callers:?}");
    assert!(callers[0].ends_with("tracker/jobs.rs"));
    let jobs = &all.iter().find(|(p, _)| p.ends_with("tracker/jobs.rs")).unwrap().1;
    let refresh = jobs.find("if published { cx.db.notifications.ledger_done(self.user_id); }").unwrap();
    assert!(refresh < jobs.find("derive_logged(").unwrap(), "today's refresh first");
}

/// The modal per venue: KuCoin's three causes, its steps and examples; Binance's export steps and no examples; Alpaca's
/// Deltabadger CSV steps. The evidence is the stored notes in words, masked with balances hidden.
#[tokio::test(flavor = "current_thread")]
async fn the_modal_says_why_shows_the_evidence_and_the_steps_per_venue() {
    let fx = alpaca_synced("0.02").await;
    let (kucoin, key) = add_kucoin(&fx);
    kucoin_balances(&fx, kucoin, key, &[("ETH", "7.4", "2000")]);
    let (binance, _) = add_venue(&fx, "Exchanges::Binance", "Binance", 0);
    assert_eq!(run(&fx).await, Outcome::Done);
    let app = app(&fx);
    let modal = |id: i64| { let (fx, app) = (&fx, &app); async move { get(fx, app, &format!("/tracker/history_gap?exchange_id={id}")).await.body } };
    let k = modal(kucoin).await;
    assert!(k.starts_with("<turbo-frame complete=\"\" id=\"modal\">") && k.contains("<h1 class=\"modal__title\">Complete your KuCoin history</h1>"), "{k}");
    for key in ["why_kucoin_reach", "why_kucoin_hf_convert", "why_kucoin_transfers", "steps_kucoin_1", "steps_kucoin_2", "steps_kucoin_3", "csv_example_caption.swap", "template_hint"] {
        assert!(k.contains(&deltabadger::web::i18n::t("en", &format!("tracker.history_gap.{key}"), &[])), "{key}");
    }
    assert!(k.contains("<li>KuCoin reports 7.40 ETH; the history accounts for 0. The difference is taken as having arrived at its price ($14,800.00).</li>"), "{k}");
    assert!(k.contains("2022-05-10T12:00:00Z,swap_out,BTC,0.5,,,,,kucoin,,kucoin-2022-05-10,"));
    assert!(k.contains("<a class=\"rbutton\" data-turbo-frame=\"modal\" href=\"/tracker/import/new\">Import a file</a>"));
    let b = modal(binance).await;
    assert!(b.contains(&deltabadger::web::i18n::t("en", "tracker.history_gap.why_binance", &[])) && b.contains(&deltabadger::web::i18n::t("en", "tracker.history_gap.steps_binance_2", &[])));
    assert!(!b.contains("<pre") && !b.contains("What the tracker found"), "no examples and no gap on Binance");
    let a = modal(fx.s.exchange_id).await;
    assert!(a.contains("Alpaca holds more than the history Deltabadger could read from it.") && a.contains(",alpaca,,alpaca-convert-1,") && a.contains("What the tracker found"));
    assert!(a.contains("Alpaca reports 0.02 BTC; the history accounts for 0.01. The difference is taken as having arrived at its price ($600.00)."), "{a}");
    fx.exec("UPDATE users SET hide_balances=1");
    let hidden = modal(fx.s.exchange_id).await;
    assert!(hidden.contains("taken as having arrived at its price (•••).") && !hidden.contains("$600.00"), "{hidden}");
}

/// No reading key on the venue (marked incorrect since the page drew the line): the key-fix links, never an import.
#[tokio::test(flavor = "current_thread")]
async fn a_venue_with_no_reading_key_offers_the_key_fix_never_an_import() {
    let fx = alpaca_synced("0.02").await;
    let (kucoin, key) = add_kucoin(&fx);
    let app = app(&fx);
    fx.c().execute("UPDATE api_keys SET status=2 WHERE id=?1", [key]).unwrap();
    let body = get(&fx, &app, &format!("/tracker/history_gap?exchange_id={kucoin}")).await.body;
    assert!(body.contains("KuCoin has no working key to import into.") && !body.contains("/tracker/import/new"), "{body}");
    assert!(body.contains(&format!("href=\"/tracker/add_api_key/new?exchange_id={kucoin}&amp;key_type=trading\">Replace KuCoin trading key</a>")), "{body}");
    assert!(body.contains(&format!("href=\"/tracker/add_api_key/new?exchange_id={kucoin}&amp;key_type=read_only\">Add KuCoin tracker key</a>")));
    fx.c().execute("UPDATE api_keys SET key_type=2 WHERE id=?1", [key]).unwrap();
    let body = get(&fx, &app, &format!("/tracker/history_gap?exchange_id={kucoin}")).await.body;
    assert!(body.contains("Replace KuCoin tracker key") && !body.contains("key_type=trading"), "{body}");
}

/// Signed out, the sign-in; an exchange the user has neither a key nor rows on, a malformed id or none: 404.
#[tokio::test(flavor = "current_thread")]
async fn the_modal_is_the_users_own() {
    let fx = alpaca_synced("0.02").await;
    let app = app(&fx);
    let signed_out = Browser::default().send(&app, "GET", &format!("/tracker/history_gap?exchange_id={}", fx.s.exchange_id), None, Csrf::None, &[]).await;
    assert!(matches!(signed_out.status, 302 | 401), "{}", signed_out.status);
    let c = fx.c();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.1', '0.1', '2026-01-01', '2026-01-01')", []).unwrap();
    let foreign = c.last_insert_rowid();
    for query in [format!("?exchange_id={foreign}"), "?exchange_id=abc".into(), String::new(), "?exchange_id=99999".into()] {
        let answer = browser(&fx, &app).send(&app, "GET", &format!("/tracker/history_gap{query}"), None, Csrf::None, &[]).await;
        assert_eq!(answer.status, 404, "{query}");
    }
}

/// Stored text is escaped wherever the line and the modal place it: the venue's name and a note's symbol.
#[tokio::test(flavor = "current_thread")]
async fn hostile_text_is_escaped() {
    let fx = alpaca_synced("0.01").await;
    let (kucoin, key) = add_kucoin(&fx);
    fx.c().execute("UPDATE exchanges SET name='Ku<script>\"Coin' WHERE id=?1", [kucoin]).unwrap();
    fx.c().execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('evil', 'E<b>\"T', 'E', 'Cryptocurrency', '2026-01-01', '2026-01-01')", []).unwrap();
    kucoin_balances(&fx, kucoin, key, &[("E<b>\"T", "7.4", "2000")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    let app = app(&fx);
    let page = get(&fx, &app, "/tracker").await.body;
    assert!(line(&page, kucoin).is_some());
    let modal = get(&fx, &app, &format!("/tracker/history_gap?exchange_id={kucoin}")).await.body;
    for body in [&page, &modal] { assert!(!body.contains("<script>\"") && !body.contains("E<b>\"T"), "unescaped"); }
    assert!(modal.contains("Ku&lt;script&gt;&quot;Coin") && modal.contains("E&lt;b&gt;&quot;T"), "{modal}");
}

/// In Polish: the line, its modal, and the import it opens, the locale kept throughout.
#[tokio::test(flavor = "current_thread")]
async fn the_polish_flow_from_line_to_import() {
    let fx = alpaca_synced("0.02").await;
    assert_eq!(run(&fx).await, Outcome::Done);
    let app = app(&fx);
    let id = fx.s.exchange_id;
    let page = line(&get(&fx, &app, "/pl/tracker").await.body, id).unwrap();
    let href = format!("/pl/tracker/history_gap?exchange_id={id}");
    assert!(page.contains(&format!("href=\"{href}\">Importuj historię</a>")), "{page}");
    let modal = get(&fx, &app, &href).await.body;
    assert!(modal.contains("Uzupełnij historię z Alpaca") && modal.contains("href=\"/pl/tracker/import/new\">Importuj plik</a>"), "{modal}");
    let import = get(&fx, &app, "/pl/tracker/import/new").await.body;
    assert!(import.contains("<turbo-frame complete=\"\" id=\"modal\">") && import.contains("action=\"/pl/tracker/import\""));
}

/// Every key in every locale, none falling back to English.
#[test]
fn every_locale_has_every_key() {
    let all = deltabadger::web::i18n::all();
    let keys: Vec<&str> = all.iter().filter_map(|(k, _)| k.strip_prefix("en.tracker.history_gap.")).collect();
    assert_eq!(keys.len(), 22);
    for locale in deltabadger::web::locale::LOCALES {
        for key in &keys {
            let full = format!("{locale}.tracker.history_gap.{key}");
            let text = all.iter().find(|(k, _)| *k == full).unwrap_or_else(|| panic!("{full} missing")).1;
            assert!(!text.trim().is_empty(), "{full}");
            if locale != "en" { assert_ne!(Some(text), all.iter().find(|(k, _)| *k == format!("en.tracker.history_gap.{key}")).map(|(_, t)| *t), "{full} is English"); }
        }
    }
}

/// In headless Chrome over the served app (synthetic install, never a real one), at 1280 px and at 375 px in the
/// longest locales: the line shows in the transactions pane, its button opens the modal, and neither the page nor the
/// modal scrolls sideways. Screenshots land in $GAP_SHOTS (default: a temporary directory, printed).
/// Needs Chrome and bun: `cargo nextest run --test tracker_gaps --run-ignored only`.
#[tokio::test(flavor = "current_thread")]
#[ignore = "needs Chrome and bun"]
async fn a_real_browser_lays_out_the_line_and_the_modal() {
    let fx = alpaca_synced("0.02").await;
    let (kucoin, key) = add_kucoin(&fx);
    kucoin_balances(&fx, kucoin, key, &[("ETH", "7.4", "2000")]);
    assert_eq!(run(&fx).await, Outcome::Done);
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    fx.c().execute("UPDATE users SET encrypted_password=?1, wash_sale_enabled=0", [hash]).unwrap();
    let app = common::web::app(fx._dir.path(), common::web::SECRET, TestClock::starting(Utc::now()));
    for (k, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(&fx.c(), &app.cipher, k, value, app.now()).unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { let _ = deltabadger::web::server::serve_on(listener, app, Default::default()).await; });
    let shots = std::env::var("GAP_SHOTS").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("history-gap-shots"));
    std::fs::create_dir_all(&shots).unwrap();
    let script = tempfile::tempdir().unwrap();
    std::fs::write(script.path().join("check.mjs"), BROWSER_CHECK).unwrap();
    let mut check = std::process::Command::new("bun");
    check.arg(script.path().join("check.mjs")).env("BASE_URL", format!("http://{address}")).env("EMAIL", "o@example.com").env("PASSWORD", "Correct-horse-9")
        .env("GAP", fx.s.exchange_id.to_string()).env("SHOTS", &shots);
    let output = tokio::task::spawn_blocking(move || check.output()).await.unwrap().unwrap();
    server.abort();
    let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    eprintln!("{log}\nscreenshots: {}", shots.display());
    assert!(output.status.success(), "{log}");
}
const BROWSER_CHECK: &str = r#####"
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const { BASE_URL, EMAIL, PASSWORD, GAP, SHOTS } = process.env;
const chrome = [process.env.CHROME, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/usr/bin/google-chrome", "/usr/bin/chromium"].filter(Boolean).find(existsSync);
const profile = mkdtempSync(join(tmpdir(), "gap-check-"));
const browser = spawn(chrome, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--window-size=1280,900", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, check, ms = 20000) { const end = Date.now() + ms; for (;;) { const v = await check(); if (v) return v; if (Date.now() > end) throw new Error(`timed out waiting for ${what}`); await sleep(100); } }
const errors = [];
let failed = false;
try {
  const portFile = join(profile, "DevToolsActivePort");
  const port = await until("Chrome", () => existsSync(portFile) && readFileSync(portFile, "utf8").split("\n")[0]);
  const target = await until("a page", async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page"));
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0; const waiting = new Map();
  socket.onmessage = (e) => { const m = JSON.parse(e.data); if (m.method === "Log.entryAdded" && m.params.entry.level === "error") errors.push(m.params.entry.text); if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text); if (waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } };
  const send = (method, params = {}) => new Promise((resolve) => { const n = ++id; waiting.set(n, resolve); socket.send(JSON.stringify({ id: n, method, params })); });
  const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result?.result?.value;
  await send("Log.enable"); await send("Runtime.enable");
  await send("Page.navigate", { url: `${BASE_URL}/login` });
  await until("the login form", () => evaluate(`!!document.querySelector("#user_password")`));
  await evaluate(`(() => { document.querySelector("#user_email").value = ${JSON.stringify(EMAIL)}; document.querySelector("#user_password").value = ${JSON.stringify(PASSWORD)}; document.querySelector("#user_password").form.requestSubmit(); })()`);
  await until("signed in", () => evaluate(`location.pathname !== "/login" && !!document.querySelector("main.main")`));
  const shot = async (name) => writeFileSync(join(SHOTS, `${name}.png`), Buffer.from((await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false })).result.data, "base64"));
  const sideways = `document.documentElement.scrollWidth > document.documentElement.clientWidth`;
  for (const [locale, width] of [["", 1280], ["", 375], ["de", 375], ["el", 375], ["fr", 375], ["pt", 375], ["bg", 375]]) {
    await send("Emulation.setDeviceMetricsOverride", { width, height: 800, deviceScaleFactor: 1, mobile: width < 600 });
    const prefix = locale ? `/${locale}` : "";
    await send("Page.navigate", { url: `${BASE_URL}${prefix}/tracker` });
    await until("the record", () => evaluate(`!!document.querySelector(".tracker-record__switch button[data-value='tx']")`));
    // Clicked until the controllers have connected and the pane switches.
    await until("the transactions pane", async () => { await evaluate(`document.querySelector(".tracker-record__switch button[data-value='tx']").click()`); await sleep(200); return evaluate(`!document.querySelector("[data-pane='tx']").classList.contains("tracker-record__pane--off")`); });
    const row = await until("the line, shown", () => evaluate(`(() => { const r = document.querySelector("#history_gap_${GAP}"); const b = r && r.querySelector("a.rbutton"); if (!r || !r.getClientRects().length) return null; const rect = b.getBoundingClientRect(); return { button: b.textContent, height: Math.round(rect.height), width: Math.round(rect.width), rowHeight: Math.round(r.getBoundingClientRect().height) }; })()`));
    const page = await evaluate(sideways);
    // The same page with the line taken out: whatever scrolls sideways then is not the line's.
    const without = await evaluate(`(() => { const r = document.querySelector("#history_gap_${GAP}"); const next = r.nextSibling, parent = r.parentNode; r.remove(); const out = ${sideways}; parent.insertBefore(r, next); return out; })()`);
    await evaluate(`document.querySelector("#history_gap_${GAP}").scrollIntoView({ block: "center" })`);
    await sleep(200);
    await shot(`${locale || "en"}-${width}-line`);
    await evaluate(`document.querySelector("#history_gap_${GAP} a.rbutton").click()`);
    const modal = await until("the modal", () => evaluate(`(() => { const d = document.querySelector("turbo-frame#modal dialog[open]"); if (!d) return null; const m = d.querySelector(".modal").getBoundingClientRect(); const pres = [...d.querySelectorAll("pre")]; return { title: d.querySelector(".modal__title").textContent, left: Math.round(m.left), right: Math.round(m.right), steps: d.querySelectorAll("ol li").length, pres: pres.length, preInside: pres.every((p) => p.getBoundingClientRect().right <= m.right + 1), import: !!d.querySelector("a[href$='/tracker/import/new']") }; })()`));
    await sleep(400);
    const modalSideways = await evaluate(sideways);
    await shot(`${locale || "en"}-${width}-modal`);
    console.log(JSON.stringify({ locale: locale || "en", width, row, pageScrollsSideways: page, withoutTheLine: without, modal, modalScrollsSideways: modalSideways }));
    if ((page && !without) || (modalSideways && !page) || modal.right > width || modal.left < 0 || !modal.preInside || !modal.import || row.height > 40) { failed = true; console.error(`layout broken at ${locale || "en"} ${width}`); }
  }
  if (errors.length) { failed = true; console.error(`console errors: ${errors.join(" | ")}`); }
} catch (e) { failed = true; console.error(e.stack || String(e)); }
browser.kill();
process.exit(failed ? 1 : 0);
"#####;
