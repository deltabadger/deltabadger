//! A synthetic tracker install: an Alpaca account with a dollar deposit and a bitcoin buy, its ledger import current,
//! and the venues and balances a test adds to it (shared by tracker_gaps and tracker_scope_frames).
#![allow(dead_code, unused_imports)]
use crate::common;
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

pub const AT: &str = "2026-10-01T02:00:00Z";
pub fn at() -> DateTime<Utc> { AT.parse().unwrap() }
pub fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
pub fn paper() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
#[derive(Clone)]
pub struct Venues;
impl Connect for Venues { type T = ScriptedTransport; fn connect(&self, _: &Credentials) -> AlpacaVenue<Self::T> { AlpacaVenue::new(ScriptedTransport::from_script(&json!({})), Urls::for_passphrase(Some("paper"))) } }
pub type Api = Rc<Option<DataApi<ScriptedTransport>>>;

/// data-api answering every historical-price request with `price` on each of `days`.
pub fn prices_api(price: f64, days: &[&str]) -> Api {
    let ms = |d: &str| d.parse::<chrono::NaiveDate>().unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
    let answer = ok(json!({ "prices": days.iter().map(|d| json!([ms(d), price])).collect::<Vec<_>>() }));
    let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": vec![answer; 60] }));
    Rc::new(Some(DataApi::new(Config { url: "http://data-api:3000".into(), token: "t".into() }, t.clone(), t)))
}

/// An Alpaca account (a dollar deposit, a bitcoin buy of 0.01) whose ledger import is current.
pub struct Fx { pub _dir: tempfile::TempDir, pub s: common::seed::Seeded, pub file: PathBuf, pub sent: Arc<Mutex<Vec<String>>> }
impl Fx {
    pub fn c(&self) -> Connection { let c = Connection::open(&self.file).unwrap(); c.busy_timeout(std::time::Duration::from_secs(5)).unwrap(); c }
    pub fn db(&self) -> Db {
        let sent = self.sent.clone();
        Db::new(self.c(), common::seed::cipher()).with_notifications(Notifications::new(move |stream, payload| { sent.lock().unwrap().push(format!("{stream} {payload}")); Ok(()) }))
    }
    pub fn refreshes(&self) -> usize { self.sent.lock().unwrap().iter().filter(|s| s.contains("action=\"refresh\"")).count() }
    pub fn owner(&self) -> i64 { self.s.user_id }
    pub fn row(&self) -> Option<Value> { self.c().query_row("SELECT value FROM app_configs WHERE key=?1", [gaps::key(self.owner())], |r| r.get::<_, String>(0)).optional().unwrap().map(|v| serde_json::from_str(&v).unwrap()) }
    pub fn cache(&self) -> (String, String) { self.c().query_row("SELECT value, updated_at FROM app_configs WHERE key=?1", [cache::key(self.owner())], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() }
    pub fn exec(&self, sql: &str) { self.c().execute_batch(sql).unwrap(); }
    /// A copy of the install as it is now.
    pub fn copy(&self) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("production.sqlite3");
        self.c().execute("VACUUM INTO ?1", [file.to_str().unwrap()]).unwrap();
        (dir, file)
    }
    /// A balance sync by the real writer: `cash` dollars and `btc` bitcoin at `price`, at `when`.
    pub async fn sync(&self, cash: &str, btc: Option<&str>, price: f64, when: DateTime<Utc>) {
        let positions: Vec<Value> = btc.map(|q| json!({ "symbol": "BTCUSD", "asset_class": "crypto", "qty": q, "qty_available": q })).into_iter().collect();
        let venue = AlpacaVenue::new(ScriptedTransport::from_script(&json!({ "GET /v2/account": [ok(json!({ "cash": cash }))], "GET /v2/positions": [ok(json!(positions))] })), Urls::for_passphrase(Some("paper")));
        let prices = ScriptedPrices::from_script(&json!({ "GET /api/v1/prices": [ok(json!({ "data": { "bitcoin": { "usd": price } } }))] }));
        balances::sync(&self.db(), &venue, &prices, self.s.api_key_id, &paper(), &FixedClock(when)).await.unwrap().unwrap();
    }
}
pub fn stamp_ledger(c: &Connection, key: i64) {
    let v = deltabadger::engine::model::credential_version_by_id(c, key).unwrap().unwrap();
    deltabadger::engine::model::credential_write(c, &Some(v.clone()), |f| deltabadger::sync::cache::record_ledger(f, key, &v, at())).unwrap();
}
#[allow(clippy::too_many_arguments)] // a row's columns
pub fn insert_row(c: &Connection, user: i64, exchange: i64, kind: i64, base: &str, amount: &str, quote: Option<(&str, &str)>, when: &str) {
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'{}','{}',?8,?8)",
              rusqlite::params![user, exchange, kind, base, amount, quote.map(|q| q.0), quote.map(|q| q.1), when]).unwrap();
}
pub fn alpaca() -> Fx {
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
pub async fn alpaca_synced(btc: &str) -> Fx { let fx = alpaca(); fx.sync("400", Some(btc), 60000.0, at()).await; fx }
pub fn job(owner: i64, api: Api) -> Box<dyn Job> { tracker::jobs::resolve::<Venues, ScriptedTransport>(tracker::jobs::TRACKER_LEDGER, owner, &Venues, api, Arc::new(at)).unwrap() }
pub async fn run(fx: &Fx) -> Outcome { run_with(fx, Rc::new(None)).await }
pub async fn run_with(fx: &Fx, api: Api) -> Outcome { job(fx.owner(), api).run(Cx { db: fx.db(), clock: &FixedClock(at()), wakers: jobs::Wakers::default() }, vec![]).await }
/// The ledger producer alone, as before this feature (no step): what the cache holds without it.
pub async fn producer_alone(file: &PathBuf, owner: i64, api: Api) {
    let db = Db::new(Connection::open(file).unwrap(), common::seed::cipher());
    tracker::jobs::ledger_run(&db, (*api).as_ref(), None, owner, &FixedClock(at()), Arc::new(at), &mut tracker::jobs::Allowance::run()).await.unwrap();
}
pub fn cache_of(file: &PathBuf, owner: i64) -> (String, String) { Connection::open(file).unwrap().query_row("SELECT value, updated_at FROM app_configs WHERE key=?1", [cache::key(owner)], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() }
pub fn gaps_of(row: &Value) -> &Vec<Value> { row["gaps"].as_array().unwrap() }

/// The step's seams are process-wide; every test that sets one runs in a process of its own (nextest).
pub fn reset_seams() {
    gaps::seam::STEPS.store(gaps::GAPS.steps, SeqCst);
    gaps::seam::SQL.store(gaps::SQL_INSTRUCTIONS, SeqCst);
    *gaps::seam::AFTER_READ.lock().unwrap() = None;
    gaps::seam::SKIP_READ.store(false, SeqCst);
}

pub fn add_kucoin(fx: &Fx) -> (i64, i64) { add_venue(fx, "Exchanges::Kucoin", "KuCoin", 0) }
pub fn add_venue(fx: &Fx, kind: &str, name: &str, key_type: i64) -> (i64, i64) {
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
pub fn kucoin_balances(fx: &Fx, exchange: i64, key: i64, held: &[(&str, &str, &str)]) {
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

pub fn seeded(s: &common::seed::Seeded) -> common::seed::Seeded { common::seed::Seeded { user_id: s.user_id, exchange_id: s.exchange_id, btc: s.btc, quote: s.quote, ticker_id: s.ticker_id, api_key_id: s.api_key_id } }
pub fn kucoin_key(fx: &Fx) -> i64 { fx.c().query_row("SELECT k.id FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE e.type='Exchanges::Kucoin' ORDER BY k.id LIMIT 1", [], |r| r.get(0)).unwrap() }

pub fn app(fx: &Fx) -> deltabadger::web::App {
    let app = common::web::app(fx._dir.path(), common::web::SECRET, TestClock::at(AT));
    // A venue other than Alpaca needs a market-data provider for the populated page (as the setup leaves one).
    for (key, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(&fx.c(), &app.cipher, key, value, app.now()).unwrap();
    }
    app
}
pub fn browser(fx: &Fx, app: &deltabadger::web::App) -> Browser {
    use deltabadger::web::session;
    let hash: String = fx.c().query_row("SELECT encrypted_password FROM users WHERE id=?1", [fx.owner()], |r| r.get(0)).unwrap();
    let data = session::SessionData { user: Some((fx.owner(), hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None }
}
pub async fn get(fx: &Fx, app: &deltabadger::web::App, path: &str) -> common::web::Answer {
    let answer = browser(fx, app).send(app, "GET", path, None, Csrf::None, &[]).await;
    assert_eq!(answer.status, 200, "{path}: {}", &answer.body[..answer.body.len().min(400)]);
    answer
}
