//! KuCoin HF fills into the ledger and the tracker (plan HF-3, D-KUCOIN-HF-LEDGER): the HF reader, the one-trade rule
//! between the export, the API and restored entries, the allocation of restored rows, the gap line, and the handback.
//! Rows L01–L10 are the plan's MATRIX rows.
mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::sync::kucoin_hf_ledger::{FILLS_PATH as HF_FILLS, SYMBOLS_PATH};
use deltabadger::sync::kucoin_ledger::{DEPOSITS_PATH, FILLS_PATH, WITHDRAWALS_PATH};
use deltabadger::tracker::import::run;
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use deltabadger::venue::kucoin_account::{MODE, OPENED};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

const KUCOIN: &str = "Exchanges::Kucoin";
const NOW: &str = "2026-10-30T12:00:00Z";
const HEADER: &str = "UID,Account Type,Currency,Side,Amount,Fee,Time(UTC),Remark,Type";
const DB_HEADER: &str = "date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description";

fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ms(s: &str) -> i64 { t(s).timestamp_millis() }

// ---- the venue ------------------------------------------------------------------------------------------------------

type Fault = Box<dyn Fn(&HttpRequest, usize) -> Option<HttpResponse>>;
#[derive(Default)]
struct State {
    mode: Cell<&'static str>, symbols: RefCell<Vec<String>>, hf: RefCell<Vec<Value>>, classic: RefCell<Vec<Value>>, fault: RefCell<Option<Fault>>,
    log: RefCell<Vec<HttpRequest>>, hf_pages: Cell<usize>, replay: RefCell<Option<Vec<Value>>>,
}
/// KuCoin as the reader meets it: account mode, symbols, HF fills (newest id first, `lastId` paging), classic sources.
#[derive(Clone, Default)]
struct Api(Rc<State>);

fn ok(data: Value) -> HttpResponse { HttpResponse { status: 200, body: json!({"code": "200000", "data": data}).to_string() } }

impl Api {
    fn new(mode: &'static str, symbols: &[&str]) -> Self {
        let a = Api::default();
        a.0.mode.set(mode);
        *a.0.symbols.borrow_mut() = symbols.iter().map(|s| s.to_string()).collect();
        a
    }
    fn hf(&self, fills: Vec<Value>) -> &Self { self.0.hf.borrow_mut().extend(fills); self }
    fn classic(&self, fills: Vec<Value>) -> &Self { self.0.classic.borrow_mut().extend(fills); self }
    fn fault(&self, f: impl Fn(&HttpRequest, usize) -> Option<HttpResponse> + 'static) { *self.0.fault.borrow_mut() = Some(Box::new(f)); }
    fn requests(&self, path: &str) -> Vec<HttpRequest> { self.0.log.borrow().iter().filter(|r| r.path == path).cloned().collect() }
}

impl Transport for Api {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.0.log.borrow_mut().push(r.clone());
        let q = |k: &str| r.query.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        match r.path.as_str() {
            MODE => return Ok(ok(json!({"selfAccountMode": if self.0.mode.get() == "uta" { "UNIFIED" } else { "CLASSIC" }}))),
            OPENED => return Ok(ok(json!(self.0.mode.get() == "hf"))),
            SYMBOLS_PATH => return Ok(ok(Value::Array(self.0.symbols.borrow().iter().map(|s| json!({"symbol": s})).collect()))),
            "/api/v1/accounts" => return Ok(ok(json!([{"currency": "BTC", "available": "2", "holds": "0"}]))),
            _ => {}
        }
        if r.path == HF_FILLS {
            let n = self.0.hf_pages.get();
            self.0.hf_pages.set(n + 1);
            if let Some(answer) = self.0.fault.borrow().as_ref().and_then(|f| f(r, n)) { return Ok(answer); }
            if let Some(pages) = self.0.replay.borrow().as_ref() { return Ok(HttpResponse { status: 200, body: pages[n]["response"]["body"].to_string() }); }
            let num = |k: &str| q(k).map(|v| v.parse::<i64>().unwrap());
            let (symbol, from, to, last) = (q("symbol").unwrap(), num("startAt").unwrap(), num("endAt").unwrap(), q("lastId").map(|v| v.parse::<u64>().unwrap()));
            let mut rows: Vec<Value> = self.0.hf.borrow().iter().filter(|f| f["symbol"] == symbol.as_str() && (from..=to).contains(&f["createdAt"].as_i64().unwrap())
                                                                         && last.is_none_or(|l| f["id"].as_u64().unwrap() < l)).cloned().collect();
            rows.sort_by_key(|f| std::cmp::Reverse(f["id"].as_u64().unwrap()));
            rows.truncate(q("limit").unwrap().parse().unwrap());
            let last = rows.last().map(|f| f["id"].clone()).unwrap_or(Value::Null);
            return Ok(ok(json!({"items": rows, "lastId": last})));
        }
        let rows: Vec<Value> = match r.path.as_str() { FILLS_PATH => self.0.classic.borrow().clone(), DEPOSITS_PATH | WITHDRAWALS_PATH => vec![], other => panic!("unscripted {other}") };
        let num = |k: &str| q(k).map(|v| v.parse::<i64>().unwrap());
        let rows: Vec<Value> = rows.into_iter().filter(|v| num("startAt").is_none_or(|s| v["createdAt"].as_i64().unwrap() >= s) && num("endAt").is_none_or(|e| v["createdAt"].as_i64().unwrap() < e)).collect();
        Ok(ok(json!({"currentPage": 1, "pageSize": 500, "totalNum": rows.len(), "totalPage": 1, "items": rows})))
    }
}

static IDS: AtomicUsize = AtomicUsize::new(1);
/// An HF fill of `symbol` (a buy of `size` for `funds`, fee in USDT), its HF `id` and `tradeId` unique.
fn hf_fill(symbol: &str, side: &str, size: &str, funds: &str, fee: &str, at: &str) -> Value {
    let id = IDS.fetch_add(1, Ordering::SeqCst) as u64;
    json!({"id": 9_007_199_254_740_000u64 + id, "tradeId": 11_029_373_945_659_000u64 + id, "orderId": format!("o{id}"), "symbol": symbol, "side": side, "price": "1",
           "size": size, "funds": funds, "fee": fee, "feeCurrency": "USDT", "createdAt": ms(at)})
}
fn classic_fill(id: &str, size: &str, funds: &str, fee: &str, at: &str) -> Value {
    json!({"symbol": "BTC-USDT", "tradeId": id, "orderId": id, "side": "buy", "price": "1", "size": size, "funds": funds, "fee": fee, "feeCurrency": "USDT", "createdAt": ms(at)})
}

/// A clock that moves `step` on every reading.
struct Clock { at: Cell<DateTime<Utc>>, step: Duration }
impl deltabadger::engine::Clock for Clock { fn now(&self) -> DateTime<Utc> { let a = self.at.get(); self.at.set(a + self.step); a } }
fn fixed(s: &str) -> Clock { Clock { at: Cell::new(t(s)), step: Duration::zero() } }

/// Room for every page, or for one page a run.
struct Pace(bool);
impl Stopwatch for Pace { fn elapsed(&self) -> std::time::Duration { std::time::Duration::from_secs(if self.0 { 1_000_000 } else { 0 }) } }

// ---- the install ----------------------------------------------------------------------------------------------------

static USERS: AtomicUsize = AtomicUsize::new(1);
struct Fx { dir: tempfile::TempDir, c: Connection, cipher: deltabadger::crypto::Cipher, user: i64, exchange: i64, key: i64 }

impl Fx {
    fn new() -> Self {
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, 'KuCoin', '0.1', '0.1', '2026-01-01', '2026-01-01')", [KUCOIN]).unwrap();
        let exchange = c.last_insert_rowid();
        let mut assets = vec![];
        for symbol in ["ETH", "BTC", "USDT", "KCS"] {
            c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')",
                      [format!("seed-{symbol}"), symbol.to_string()]).unwrap();
            assets.push(c.last_insert_rowid());
        }
        for (base, quote, b, q, d) in [("ETH", "BTC", 0, 1, 6), ("BTC", "USDT", 1, 2, 1), ("KCS", "USDT", 3, 2, 3), ("ETH", "USDT", 0, 2, 2)] {
            c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                       minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 8, 8, ?7, '0', '0', 1, 1, '2026-01-01', '2026-01-01')",
                      rusqlite::params![exchange, format!("{base}-{quote}"), base, quote, assets[b], assets[q], d]).unwrap();
        }
        let cipher = common::seed::cipher();
        let email = format!("hf{}@example.com", USERS.fetch_add(1, Ordering::SeqCst));
        c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES (?1, 'x', 'Owner', 1, '2026-01-01', '2026-01-01')", [&email]).unwrap();
        let user = c.last_insert_rowid();
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 2, ?3, ?4, ?5, 1, '2026-01-01', '2026-01-01')",
                  rusqlite::params![user, exchange, cipher.encrypt(&email), cipher.encrypt("secret"), cipher.encrypt("phrase")]).unwrap();
        let key = c.last_insert_rowid();
        Fx { dir, c, cipher, user, exchange, key }
    }
    fn db(&self) -> deltabadger::jobs::Db {
        let c = Connection::open(self.dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        deltabadger::jobs::Db::new(c, common::seed::cipher())
    }
    /// One ledger run; Ok(complete) or Err(the key's error).
    async fn sync_with(&self, api: &Api, clock: &dyn deltabadger::engine::Clock, paced: bool) -> Result<bool, String> {
        // Each run detects the mode afresh: a test switches it between runs, inside the resolver's five minutes.
        deltabadger::venue::kucoin_account::Resolver::shared().invalidate(deltabadger::venue::kucoin::URL, self.key);
        let db = self.db();
        let credentials = deltabadger::sync::validation_material(&self.c, &self.cipher, self.key).unwrap().0;
        let pace = Pace(paced);
        let run = Run { deadline: std::time::Duration::from_secs(3600), chain_runs: usize::MAX, stopwatch: &pace };
        let out = deltabadger::sync::kucoin_ledger::sync_within(&db, api.clone(), self.key, &credentials, clock, std::time::Duration::ZERO, &run).await.map_err(|e| e.0)?;
        db.settled().await;
        out.map(|o| o.complete).map_err(|f| f.error)
    }
    async fn sync(&self, api: &Api) -> Result<bool, String> { self.sync_with(api, &fixed(NOW), false).await }
    /// A watermark this key produced a day ago: the classic walk is a few pages, not the history since 2019.
    fn watermark(&self) {
        self.c.execute("UPDATE api_keys SET last_synced_at = '2026-10-29 12:00:00' WHERE id = ?1", [self.key]).unwrap();
        let producer = deltabadger::engine::model::credential_version_by_id(&self.c, self.key).unwrap().unwrap();
        let tx = self.c.unchecked_transaction().unwrap();
        deltabadger::sync::cache::record_ledger(&deltabadger::engine::model::check_credential_result(&tx, &Some(producer.clone())).unwrap(), self.key, &producer, t(NOW)).unwrap();
        tx.commit().unwrap();
    }
    fn import(&self, text: &str) -> run::Answer { run::import(&self.c, &self.cipher, self.user, "kucoin", text.as_bytes(), None, None, t(NOW)).unwrap() }
    fn restore(&self, rows: &[&str]) -> usize {
        let text = std::iter::once(DB_HEADER.to_string()).chain(rows.iter().map(|r| r.to_string())).collect::<Vec<_>>().join("\n");
        match run::import(&self.c, &self.cipher, self.user, "deltabadger", text.as_bytes(), None, None, t(NOW)).unwrap() { run::Answer::Written { imported, .. } => imported, other => panic!("{other:?}") }
    }
    fn count(&self, sql: &str) -> i64 { self.c.query_row(sql, [], |r| r.get(0)).unwrap() }
    /// Each trade as (type, base, amount, quote amount, fee, second, source), sorted.
    fn trades(&self) -> Vec<(i64, String, String, String, String, String)> {
        let mut s = self.c.prepare("SELECT entry_type, base_currency, printf('%.10g', base_amount), printf('%.10g', COALESCE(quote_amount, 0)), printf('%.10g', COALESCE(fee_amount, 0)), \
                                    substr(transacted_at, 1, 19) FROM account_transactions WHERE user_id = ?1 AND entry_type IN (0, 1, 10, 15) ORDER BY 6, 1, 3, 4, 5").unwrap();
        s.query_map([self.user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
    fn coverage(&self, source: &str) -> Vec<(String, String)> {
        let mut s = self.c.prepare("SELECT start_at, end_at FROM kucoin_coverage WHERE user_id = ?1 AND source = ?2 ORDER BY 1").unwrap();
        s.query_map(rusqlite::params![self.user, source], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
    fn ambiguous(&self) -> Vec<String> { self.coverage("ambiguous").into_iter().map(|(a, _)| a).collect() }
    fn uncovered(&self) -> Value {
        let raw: Option<String> = self.c.query_row("SELECT value FROM app_configs WHERE key = ?1", [deltabadger::sync::kucoin_hf::gap_key(self.user)], |r| r.get(0)).ok();
        raw.map(|r| serde_json::from_str::<Value>(&r).unwrap()[self.exchange.to_string()]["uncovered"].clone()).unwrap_or(Value::Null)
    }
    fn line(&self) -> bool { !deltabadger::tracker::gaps::read(&self.c, self.user, None, None, None).unwrap().is_empty() }
    fn error(&self) -> Option<String> { self.c.query_row("SELECT last_sync_error FROM api_keys WHERE id = ?1", [self.key], |r| r.get(0)).unwrap() }
}

/// A KuCoin export of rows "Currency,Side,Amount,Fee,UTC time,Type".
fn export(rows: &[&str]) -> String {
    let mut out = vec![HEADER.to_string()];
    for r in rows {
        let f: Vec<&str> = r.split(',').collect();
        out.push(format!("123456789,mainAccount,{},{},{},{},{},,{}", f[0], f[1], f[2], f[3], f[4], f[5]));
    }
    out.join("\n")
}
/// The export legs of a BTC-USDT buy at `at`: `size` BTC in, `funds` + `fee` USDT out.
fn buy_legs(size: &str, funds: &str, fee: &str, at: &str) -> Vec<String> {
    let total = deltabadger::ruby::BigDec::parse(funds).unwrap();
    let total = (&total + &deltabadger::ruby::BigDec::parse(fee).unwrap()).to_s_f();
    vec![format!("BTC,Deposit,{size},0,{at},Spot"), format!("USDT,Withdrawal,{total},{fee},{at},Spot")]
}
fn file(legs: &[Vec<String>]) -> String { export(&legs.iter().flatten().map(String::as_str).collect::<Vec<_>>()) }

// ---- L01: pages ---------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn l01_pages_boundary_and_large_ids_replayed() {
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/kucoin_hf/fills.json")).unwrap()).unwrap();
    let case = fixture["cases"].as_array().unwrap().iter().find(|c| c["id"] == "pages_boundary_and_large_ids").unwrap();
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    *api.0.replay.borrow_mut() = Some(case["pages"].as_array().unwrap().clone());
    let now = "2024-10-22T07:00:00Z";
    assert_eq!(fx.sync_with(&api, &fixed(now), false).await, Ok(true));
    let sent = api.requests(HF_FILLS);
    assert_eq!(sent.len(), 3);
    let (end, start) = (ms(now), ms(now) - 7 * 86_400_000 + 6 * 3_600_000);
    for (i, r) in sent.iter().enumerate() {
        let names: Vec<&str> = r.query.iter().map(|(k, _)| *k).collect();
        assert_eq!(&names[..4], ["symbol", "startAt", "endAt", "limit"], "in order");
        assert_eq!(r.query[1].1, start.to_string());
        assert_eq!(r.query[2].1, (end - 1).to_string());
        assert_eq!(r.query[3].1, "100");
        if i > 0 {
            let cursor = case["pages"][i - 1]["response"]["body"]["data"]["lastId"].as_u64().unwrap();
            assert!(cursor > 1 << 53);
            assert_eq!(r.query[4], ("lastId", cursor.to_string()), "the returned lastId exactly, never f64 or tradeId");
        } else { assert_eq!(names.len(), 4); }
    }
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE json_extract(raw_data, '$.family') = 'ClassicHf'"), 102);
    let mut s = fx.c.prepare("SELECT raw_data FROM account_transactions WHERE json_extract(raw_data, '$.family') = 'ClassicHf'").unwrap();
    let raws: Vec<Value> = s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| serde_json::from_str(&r.unwrap()).unwrap()).collect();
    let total = |k: &str| raws.iter().fold(deltabadger::ruby::BigDec::zero(), |a, r| &a + &deltabadger::ruby::BigDec::parse(r[k].as_str().unwrap()).unwrap()).to_s_f();
    assert_eq!([total("size"), total("funds"), total("fee")], ["0.00102", "69.071952", "0.069071952"]);
    assert!(raws.iter().all(|r| r["orderId"].as_str().is_some_and(|o| o.starts_with("hf-history-"))));
    assert_eq!(fx.coverage("hf_api").len(), 1, "a complete unit certifies");
}

#[tokio::test(flavor = "current_thread")]
async fn l01_symbol_scoped_trade_identity() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT", "ETH-USDT"]);
    let (mut a, mut b) = (hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-29T10:00:00Z"), hf_fill("ETH-USDT", "buy", "1", "100", "0.1", "2026-10-29T10:00:00Z"));
    a["tradeId"] = json!(7u64);
    b["tradeId"] = json!(7u64);
    api.hf(vec![a, b]);
    assert_eq!(fx.sync(&api).await, Ok(true));
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE tx_id IN ('BTC-USDT-7', 'ETH-USDT-7')"), 2);
}

// ---- L02: faults ----------------------------------------------------------------------------------------------------

fn page_of(items: Vec<Value>, last: Value) -> HttpResponse { ok(json!({"items": items, "lastId": last})) }

#[tokio::test(flavor = "current_thread")]
async fn l02_a_failed_page_keeps_earlier_pages_certifies_nothing_and_fails_the_job_after_classic() {
    type Make = fn(&HttpRequest) -> HttpResponse;
    let faults: Vec<(&str, Make)> = vec![
        ("missing lastId", |_| page_of(vec![hf_fill("BTC-USDT", "buy", "1", "1", "0", "2026-10-29T09:00:00Z")], Value::Null)),
        ("repeated lastId", |r| page_of(vec![hf_fill("BTC-USDT", "buy", "1", "1", "0", "2026-10-29T09:00:00Z")], json!(r.query[4].1.parse::<u64>().unwrap()))),
        ("non-array items", |_| ok(json!({"items": {}, "lastId": 1}))),
        ("malformed item", |_| { let mut f = hf_fill("BTC-USDT", "buy", "1", "1", "0", "2026-10-29T09:00:00Z"); f["size"] = json!("-1"); page_of(vec![f], json!(1)) }),
        ("another symbol", |_| page_of(vec![hf_fill("ETH-USDT", "buy", "1", "1", "0", "2026-10-29T09:00:00Z")], json!(1))),
        ("outside the unit", |_| page_of(vec![hf_fill("BTC-USDT", "buy", "1", "1", "0", "2026-10-30T12:00:01Z")], json!(1))),
        ("envelope", |_| HttpResponse { status: 200, body: json!({"code": "900001", "msg": "no"}).to_string() }),
        ("http", |_| HttpResponse { status: 500, body: "boom".into() }),
        ("throttle exhausted", |_| HttpResponse { status: 429, body: json!({"code": "429000"}).to_string() }),
        ("in-page conflict", |_| {
            let a = hf_fill("BTC-USDT", "buy", "1", "1", "0", "2026-10-29T09:00:00Z");
            let mut b = a.clone();
            b["createdAt"] = json!(a["createdAt"].as_i64().unwrap() + 1000);
            page_of(vec![a, b], json!(1))
        }),
    ];
    for (name, make) in faults {
        let fx = Fx::new();
        let api = Api::new("hf", &["BTC-USDT"]);
        api.hf((0..100).map(|i| hf_fill("BTC-USDT", "buy", "1", "100", "0.1", &format!("2026-10-29T10:{:02}:{:02}Z", i / 60, i % 60))).collect());
        api.fault(move |r, n| (n >= 1).then(|| make(r)));
        let out = fx.sync(&api).await;
        assert!(out.is_err(), "{name}: the job fails");
        assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE json_extract(raw_data, '$.family') = 'ClassicHf'"), 100, "{name}: page 1 stays published");
        assert!(fx.coverage("hf_api").is_empty() && fx.coverage("hf_attempt").len() == 1 && fx.coverage("hf_attempt_open").is_empty(), "{name}");
        assert!(fx.error().is_some(), "{name}");
        assert!(!api.requests(FILLS_PATH).is_empty(), "{name}: classic still runs");
        assert!(fx.line(), "{name}: the line shows");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn l02_a_conflict_with_a_stored_fill_fails_the_page_in_both_family_orders() {
    for hf_first in [true, false] {
        let fx = Fx::new();
        let at = "2026-10-30T10:00:00Z";
        let mut hf = hf_fill("BTC-USDT", "buy", "1", "100", "0.1", at);
        hf["tradeId"] = json!("55");
        let classic = classic_fill("55", "2", "200", "0.2", at);
        let api = Api::new("hf", &["BTC-USDT"]);
        if hf_first { api.hf(vec![hf.clone()]); } else { api.classic(vec![classic.clone()]); }
        let _ = fx.sync(&api).await;
        let before = fx.trades();
        let again = Api::new("hf", &["BTC-USDT"]);
        again.hf(vec![hf]).classic(vec![classic]);
        let out = fx.sync_with(&again, &fixed("2026-10-30T13:00:00Z"), false).await;
        assert!(out.is_err(), "hf_first={hf_first}");
        assert_eq!(fx.trades(), before, "nothing written by the conflicting page");
    }
}

// ---- L03: bounds and retention -------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn l03_bounds_are_within_retention_and_advance_only_on_certified_units() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    assert_eq!(fx.sync(&api).await, Ok(true));
    let first = &api.requests(HF_FILLS)[0];
    assert_eq!(first.query[1].1, (ms(NOW) - 7 * 86_400_000 + 6 * 3_600_000).to_string(), "7 d less the margin, never 8 d");
    assert_eq!(first.query[2].1, (ms(NOW) - 1).to_string());
    // Two hours later: from the certified end less 25 h, bounded by retention.
    let later = Api::new("hf", &["BTC-USDT"]);
    assert_eq!(fx.sync_with(&later, &fixed("2026-10-30T14:00:00Z"), false).await, Ok(true));
    assert_eq!(later.requests(HF_FILLS)[0].query[1].1, (ms(NOW) - 25 * 3_600_000).to_string());
    // An outage of ten days leaves a hole the newer unit cannot close; the newer unit certifies.
    let after = Api::new("hf", &["BTC-USDT"]);
    assert_eq!(fx.sync_with(&after, &fixed("2026-11-09T14:00:00Z"), false).await, Ok(true));
    assert_eq!(fx.coverage("hf_api").len(), 3);
    assert_eq!(fx.uncovered(), json!([["2026-10-30 14:00:00", "2026-11-02 20:00:00"]]));
}

#[tokio::test(flavor = "current_thread")]
async fn l03_retention_expired_on_resume_abandons_and_the_line_shows() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf((0..150).map(|i| hf_fill("BTC-USDT", "buy", "1", "100", "0.1", &format!("2026-10-29T10:{:02}:{:02}Z", i / 60, i % 60))).collect());
    // One page, then the run pauses; the next run starts eight days later.
    assert_eq!(fx.sync_with(&api, &fixed(NOW), true).await, Ok(false));
    assert_eq!(fx.coverage("hf_attempt_open").len(), 1);
    assert!(fx.line(), "an open unit shows its unread remainder");
    let out = fx.sync_with(&api, &fixed("2026-11-07T12:00:00Z"), true).await;
    assert!(out.is_ok() || out.is_err());
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE json_extract(raw_data, '$.family') = 'ClassicHf'"), 100, "what it published stays");
    assert!(!fx.coverage("hf_attempt").is_empty());
    assert!(fx.line());
    // Within one run: the clock passes retention between pages.
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf((0..150).map(|i| hf_fill("BTC-USDT", "buy", "1", "100", "0.1", &format!("2026-10-29T10:{:02}:{:02}Z", i / 60, i % 60))).collect());
    let clock = Clock { at: Cell::new(t(NOW)), step: Duration::days(2) };
    assert!(fx.sync_with(&api, &clock, false).await.is_err(), "no per-page retention check would read on");
    assert!(fx.coverage("hf_api").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn l03_a_first_page_failure_then_recovery_rereads_five_day_old_fills() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf(vec![hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-25T12:00:00Z")]);
    api.fault(|_, n| (n == 0).then(|| HttpResponse { status: 500, body: "boom".into() }));
    assert!(fx.sync(&api).await.is_err());
    assert_eq!(fx.trades().len(), 0);
    assert!(fx.line(), "an empty history whose first page fails shows the line");
    assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T12:05:00Z"), false).await, Ok(true));
    assert_eq!(fx.trades().len(), 1);
}

// ---- L04: symbols ---------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn l04_listed_symbols_are_walked_and_absent_pairs_keep_units_uncertified() {
    let fx = Fx::new();
    // DOGE-USDT has no ticker row here: the live list decides.
    let api = Api::new("hf", &["BTC-USDT", "DOGE-USDT"]);
    api.hf(vec![hf_fill("DOGE-USDT", "buy", "10", "1", "0.001", "2026-10-29T10:00:00Z")]);
    assert_eq!(fx.sync(&api).await, Ok(true));
    assert!(api.requests(HF_FILLS).iter().any(|r| r.query[0].1 == "DOGE-USDT"));
    assert_eq!(fx.coverage("hf_api").len(), 1);
    // DOGE-USDT leaves the list: missing, observed after the next unit's start, so that unit is uncertified.
    let gone = Api::new("hf", &["BTC-USDT"]);
    assert_eq!(fx.sync_with(&gone, &fixed("2026-10-30T14:00:00Z"), false).await, Ok(true));
    assert_eq!(fx.coverage("missing:DOGE-USDT").len(), 1);
    assert_eq!(fx.coverage("hf_api").len(), 1);
    // Listed again: the observation goes; missing again later: a new one.
    assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T16:00:00Z"), false).await, Ok(true));
    assert!(fx.coverage("missing:DOGE-USDT").is_empty());
    assert_eq!(fx.sync_with(&gone, &fixed("2026-10-30T18:00:00Z"), false).await, Ok(true));
    assert_eq!(fx.coverage("missing:DOGE-USDT"), vec![("2026-10-30 18:00:00".into(), "2026-10-30 18:00:00".into())]);
    assert_eq!(fx.coverage("hf_api").len(), 2);
    // A week on, the observation is before the unit's start: certified again.
    assert_eq!(fx.sync_with(&gone, &fixed("2026-11-07T18:00:00Z"), false).await, Ok(true));
    assert_eq!(fx.coverage("hf_api").len(), 3);
}

// ---- L05–L07: the rule ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn l05_one_export_over_hf_trades_counts_each_trade_once() {
    let fx = Fx::new();
    let at = |s: &str| format!("2026-10-29 {s}");
    // An aggregate (two fills one second, one export leg pair) and a shared KCS fee; a delisted pair's trade with no API row.
    let mut legs = vec![buy_legs("0.3", "30", "0.03", &at("10:00:00"))];
    legs.push(vec![format!("BTC,Deposit,0.2,0,{},Spot", at("10:00:05")), format!("USDT,Withdrawal,20,0,{},Spot", at("10:00:05")), format!("KCS,Withdrawal,0.01,0,{},Spot", at("10:00:05"))]);
    legs.push(vec![format!("ETH,Deposit,1,0,{},Spot", at("09:00:00")), format!("BTC,Withdrawal,0.05,0,{},Spot", at("09:00:00"))]);
    fx.import(&file(&legs));
    let api = Api::new("hf", &["BTC-USDT"]);
    let mut kcs = hf_fill("BTC-USDT", "buy", "0.2", "20", "0.01", "2026-10-29T10:00:05Z");
    kcs["feeCurrency"] = json!("KCS");
    api.hf(vec![hf_fill("BTC-USDT", "buy", "0.1", "10", "0.01", "2026-10-29T10:00:00Z"), hf_fill("BTC-USDT", "buy", "0.2", "20", "0.02", "2026-10-29T10:00:00Z"), kcs]);
    assert_eq!(fx.sync(&api).await, Ok(true));
    let trades = fx.trades();
    assert_eq!(trades.len(), 4, "{trades:?}");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE tx_id LIKE 'kucoin-csv-%' AND entry_type IN (0, 1)"), 1, "only the delisted pair's trade from the file");
    assert!(fx.ambiguous().is_empty());
}

/// The rule's result, as (trades, ambiguous seconds).
type Ledger = (Vec<(i64, String, String, String, String, String)>, Vec<String>);
fn ledger(fx: &Fx) -> Ledger { (fx.trades(), fx.ambiguous()) }

#[tokio::test(flavor = "current_thread")]
async fn l06_import_then_api_and_api_then_import_give_one_ledger_and_repeats_are_no_ops() {
    let legs = file(&[buy_legs("1", "100", "0.1", "2026-10-29 10:00:00"), buy_legs("2", "50", "0.05", "2026-10-29 11:00:00")]);
    let fills = vec![hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-29T10:00:00Z"), hf_fill("BTC-USDT", "buy", "3", "70", "0.07", "2026-10-29T12:00:00Z")];
    let first = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf(fills.clone());
    first.import(&legs);
    assert_eq!(first.sync(&api).await, Ok(true));
    let second = Fx::new();
    assert_eq!(second.sync(&api).await, Ok(true));
    second.import(&legs);
    assert_eq!(ledger(&first), ledger(&second));
    assert_eq!(first.trades().len(), 3);
    first.import(&legs);
    assert_eq!(first.sync_with(&api, &fixed("2026-10-30T12:30:00Z"), false).await, Ok(true));
    assert_eq!(ledger(&first), ledger(&second), "a repeat adds nothing");
}

#[tokio::test(flavor = "current_thread")]
async fn l06_a_page_split_inside_a_second_is_flagged_then_cleared_never_double_counted() {
    let fx = Fx::new();
    // Two fills in one second at prices the export cannot pair (one aggregated trade), the second on page 2.
    fx.watermark();
    fx.import(&export(&["BTC,Deposit,3,0,2026-10-29 10:00:00,Spot", "USDT,Withdrawal,330,0,2026-10-29 10:00:00,Spot"]));
    let api = Api::new("hf", &["BTC-USDT"]);
    // Pages go newest id first: the pair's later id is on page 1 with 99 others, its earlier id alone on page 2.
    let mut fills = vec![hf_fill("BTC-USDT", "buy", "1", "100", "0", "2026-10-29T10:00:00Z")];
    fills.extend((0..99).map(|i| hf_fill("BTC-USDT", "buy", "1", "1", "0", &format!("2026-10-29T11:{:02}:{:02}Z", i / 60, i % 60))));
    fills.push(hf_fill("BTC-USDT", "buy", "2", "230", "0", "2026-10-29T10:00:00Z"));
    api.hf(fills);
    assert_eq!(fx.sync_with(&api, &fixed(NOW), true).await, Ok(false));
    let at_second = |fx: &Fx| fx.trades().into_iter().filter(|t| t.5 == "2026-10-29 10:00:00").count();
    assert_eq!(at_second(&fx), 1, "the API row only, the aggregate withheld");
    assert_eq!(fx.ambiguous(), vec!["2026-10-29 10:00:00".to_string()]);
    while fx.coverage("hf_api").is_empty() { assert!(fx.sync_with(&api, &fixed(NOW), true).await.is_ok()); }
    assert_eq!(at_second(&fx), 2, "both API rows, the export's aggregate represented");
    assert!(fx.ambiguous().is_empty(), "cleared by the next page");
}

#[tokio::test(flavor = "current_thread")]
async fn l06_a_rotation_before_a_commit_publishes_nothing() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf(vec![hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-29T10:00:00Z")]);
    let path = fx.dir.path().join("production.sqlite3");
    api.fault(move |_, _| { Connection::open(&path).unwrap().execute("UPDATE api_keys SET passphrase = 'rotated'", []).unwrap(); None });
    let db = fx.db();
    let credentials = deltabadger::sync::validation_material(&fx.c, &fx.cipher, fx.key).unwrap().0;
    let run = Run { deadline: std::time::Duration::from_secs(3600), chain_runs: usize::MAX, stopwatch: &Pace(false) };
    assert!(deltabadger::sync::kucoin_ledger::sync_within(&db, api.clone(), fx.key, &credentials, &fixed(NOW), std::time::Duration::ZERO, &run).await.is_err());
    db.settled().await;
    assert_eq!(fx.trades().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn l07_an_unpaired_api_trade_within_a_second_makes_a_leftover_ambiguous() {
    let fx = Fx::new();
    fx.import(&file(&[buy_legs("1", "100", "0.1", "2026-10-29 10:00:00"), buy_legs("5", "50", "0", "2026-10-29 12:00:00")]));
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf(vec![hf_fill("BTC-USDT", "buy", "2", "200", "0.2", "2026-10-29T10:00:01Z")]);
    assert_eq!(fx.sync(&api).await, Ok(true));
    assert_eq!(fx.ambiguous(), vec!["2026-10-29 10:00:00".to_string()], "the API wins; the leftover is flagged");
    assert!(fx.uncovered().as_array().unwrap().contains(&json!(["2026-10-29 10:00:00", "2026-10-29 10:00:01"])), "the line shows the ambiguous second: {}", fx.uncovered());
    let trades = fx.trades();
    assert!(!trades.iter().any(|t| t.5 == "2026-10-29 10:00:00"), "{trades:?}");
    assert!(trades.iter().any(|t| t.5 == "2026-10-29 12:00:00"), "the same kind of leftover with no API trade near it stays visible");
}

#[tokio::test(flavor = "current_thread")]
async fn l07_identical_trades_in_adjacent_seconds_with_one_api_row_are_both_counted() {
    let fx = Fx::new();
    fx.import(&file(&[buy_legs("1", "100", "0.1", "2026-10-29 10:00:00"), buy_legs("1", "100", "0.1", "2026-10-29 10:00:01")]));
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf(vec![hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-29T10:00:00Z")]);
    assert_eq!(fx.sync(&api).await, Ok(true));
    assert_eq!(fx.trades().len(), 2);
    assert!(fx.ambiguous().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn l07_neighbours_are_judged_again_whatever_the_upload_order() {
    let mut ledgers = vec![];
    for match_first in [false, true] {
        let fx = Fx::new();
        let api = Api::new("hf", &["BTC-USDT"]);
        api.hf(vec![hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-29T10:00:00Z")]);
        assert_eq!(fx.sync(&api).await, Ok(true));
        let (a, b) = (file(&[buy_legs("1", "100", "0.1", "2026-10-29 10:00:00")]), file(&[buy_legs("7", "70", "0", "2026-10-29 10:00:01")]));
        if match_first { fx.import(&a); fx.import(&b); } else { fx.import(&b); fx.import(&a); }
        assert!(fx.ambiguous().is_empty(), "match_first={match_first}");
        assert_eq!(fx.trades().len(), 2, "B becomes visible");
        ledgers.push(ledger(&fx));
    }
    assert_eq!(ledgers[0], ledgers[1]);
}

// ---- L08–L09: migration and restores --------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn l08_the_migration_rebuilds_what_the_classic_cutoff_hid_once() {
    let fx = Fx::new();
    // Classic era: an export over a classic API trade, and HF-era trades the classic API never returns.
    fx.import(&file(&[buy_legs("1", "100", "0.1", "2026-10-20 10:00:00"), buy_legs("2", "50", "0.05", "2026-10-26 10:00:00"),
                      vec!["USDT,Deposit,500,0,2026-10-26 11:00:00,Deposit".to_string()]]));
    let lf = Api::new("lf", &[]);
    lf.classic(vec![classic_fill("c1", "1", "100", "0.1", "2026-10-20T10:00:00Z")]);
    assert_eq!(fx.sync_with(&lf, &fixed("2026-10-27T00:00:00Z"), false).await, Ok(true));
    assert_eq!(fx.trades().len(), 1, "the classic takeover hid the HF-era trade");
    let deposits = fx.count("SELECT count(*) FROM account_transactions WHERE entry_type = 4");
    let hf = Api::new("hf", &["BTC-USDT"]);
    hf.classic(vec![classic_fill("c1", "1", "100", "0.1", "2026-10-20T10:00:00Z")]);
    assert_eq!(fx.sync(&hf).await, Ok(true));
    let trades = fx.trades();
    assert_eq!(trades.len(), 2, "rebuilt once, and the classic second pairs against the API: {trades:?}");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE entry_type = 4"), deposits, "deposits unchanged");
    assert_eq!(fx.count("SELECT count(*) FROM kucoin_coverage WHERE source = 'hf_enabled'"), 1);
    fx.import(&file(&[buy_legs("1", "100", "0.1", "2026-10-20 10:00:00"), buy_legs("2", "50", "0.05", "2026-10-26 10:00:00"),
                      vec!["USDT,Deposit,500,0,2026-10-26 11:00:00,Deposit".to_string()]]));
    assert_eq!(fx.trades(), trades, "an unchanged re-upload adds nothing");
    // ClassicLf afterwards keeps the rule, and the migration runs once.
    assert_eq!(fx.sync_with(&lf, &fixed("2026-10-30T13:00:00Z"), false).await, Ok(true));
    assert_eq!(fx.trades(), trades);
    assert_eq!(fx.count("SELECT count(*) FROM kucoin_coverage WHERE source = 'hf_enabled'"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn l09_restore_and_api_in_either_order_count_once_and_survive_an_app_configs_wipe() {
    let mut ledgers = vec![];
    for restore_first in [true, false] {
        let fx = Fx::new();
        let api = Api::new("hf", &["BTC-USDT"]);
        let fill = hf_fill("BTC-USDT", "buy", "1", "100", "0.1", "2026-10-30T10:00:00Z");
        api.hf(vec![fill]);
        let restored = ["2026-10-30T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,,,"];
        assert_eq!(fx.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true), "enable HF first");
        if restore_first { fx.restore(&restored); assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T12:10:00Z"), false).await, Ok(true)); }
        else { assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T12:10:00Z"), false).await, Ok(true)); fx.restore(&restored); }
        assert_eq!(fx.trades().len(), 1, "restore_first={restore_first}");
        assert_eq!(fx.count("SELECT count(*) FROM kucoin_withheld_fills"), 1, "the API fill is withheld behind the restore");
        assert!(fx.ambiguous().is_empty(), "same effect");
        // An API replay of it is still once.
        assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T12:20:00Z"), false).await, Ok(true));
        assert_eq!(fx.trades().len(), 1);
        ledgers.push(ledger(&fx));
        // A Rails reset of app_configs keeps the marker, flags and withheld rows; the gap row is rebuilt.
        fx.c.execute("DELETE FROM app_configs", []).unwrap();
        assert_eq!(fx.count("SELECT count(*) FROM kucoin_coverage WHERE source = 'hf_enabled'"), 1);
        assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T12:30:00Z"), false).await, Ok(true));
        assert!(!fx.uncovered().is_null());
    }
    assert_eq!(ledgers[0], ledgers[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn l09_restored_a_and_an_export_of_a_and_b_show_b_once() {
    let fx = Fx::new();
    assert_eq!(fx.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    fx.restore(&["2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,,,"]);
    fx.import(&file(&[buy_legs("1", "100", "0.1", "2026-10-29 10:00:00"), buy_legs("2", "50", "0", "2026-10-29 10:00:00")]));
    let trades = fx.trades();
    assert!(trades.len() == 2 || !fx.ambiguous().is_empty(), "B once, or the second flagged; never A twice: {trades:?}");
    // A different export trade at a second the restore holds alone, without exact amounts: ambiguous.
    let legacy = Fx::new();
    legacy.restore(&["2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,,,"]);
    legacy.c.execute("UPDATE account_transactions SET raw_data = '{}'", []).unwrap();
    assert_eq!(legacy.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    legacy.import(&file(&[buy_legs("3", "30", "0", "2026-10-29 10:00:00")]));
    assert_eq!(legacy.ambiguous(), vec!["2026-10-29 10:00:00".to_string()]);
    assert_eq!(legacy.trades().len(), 1);
}

// ---- allocation: every permutation gives one ledger (r9–r13) ---------------------------------------------------------

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 { return vec![items.to_vec()]; }
    let mut out = vec![];
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut p in permutations(&rest) { p.insert(0, head.clone()); out.push(p); }
    }
    out
}

/// `stored` restored first (one file), then `upload` in every order, twice; every result equal.
async fn every_order(stored: &[&str], upload: &[&str], trades: usize) {
    let mut seen: Option<Ledger> = None;
    for order in permutations(upload) {
        let fx = Fx::new();
        assert_eq!(fx.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
        if !stored.is_empty() { fx.restore(stored); }
        fx.restore(&order);
        let mut reversed = order.clone();
        reversed.reverse();
        fx.restore(&reversed);
        let got = ledger(&fx);
        assert_eq!(got.0.len(), trades, "{order:?}: {got:?}");
        match &seen { Some(s) => assert_eq!(s, &got, "{order:?}"), None => seen = Some(got) }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn allocation_is_order_independent() {
    let a = "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,A,,";
    let b = "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,,,";
    let x = "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,X,,";
    // Codex's case: stored X (id'd) and Y (id-less) with one key, then [id-less B, new id'd A].
    every_order(&[x, b], &[b, a], 2).await;
    // A restored id'd A, then A and an identical id-less B: two trades, as the two-row file into an empty history.
    every_order(&[a], &[a, b], 2).await;
    every_order(&[], &[a, b], 2).await;
    // Rows that differ only in quote, or only in fee, stay distinct; a zero fee with a currency is no fee, so that row and
    // the fee-less one are two identical occurrences of one file: two.
    every_order(&[], &["2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,,,kucoin,,,", "2026-10-29T10:00:00Z,buy,BTC,1,USDT,101,,,kucoin,,,",
                       "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.2,kucoin,,,", "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0,kucoin,,,"], 4).await;
    // id-less Y, then restores of A and B with distinct ids and Y's key: two trades whichever comes first.
    every_order(&[b], &[a, "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,B,,"], 2).await;
}

#[tokio::test(flavor = "current_thread")]
async fn allocation_split_uploads_equal_the_single_upload_and_one_file_keeps_two_occurrences() {
    let b = "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,,,";
    let a = "2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,0.1,kucoin,A,,";
    let single = Fx::new();
    assert_eq!(single.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    single.restore(&[a, b]);
    let split = Fx::new();
    assert_eq!(split.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    // The file uploaded in part first and then whole equals the whole file (r7.2); an id-less row alone in a later file
    // is judged by its content (r10.2, below).
    split.restore(&[a]);
    split.restore(&[b, a]);
    assert_eq!(ledger(&single), ledger(&split));
    assert_eq!(single.trades().len(), 2);
    // r10.2: two identical id-less rows in one file are two; as two one-row files, one.
    let one_file = Fx::new();
    assert_eq!(one_file.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    assert_eq!(one_file.restore(&[b, b]), 2);
    assert_eq!(one_file.restore(&[b, b]), 0);
    let two_files = Fx::new();
    assert_eq!(two_files.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    assert_eq!(two_files.restore(&[b]) + two_files.restore(&[b]), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn allocation_matches_exact_before_legacy_and_flags_a_precision_merge() {
    // Legacy L (no amounts, fee 0.1) and exact E (fee 0.10000000000000001); then fees 0.10000000000000001 and
    // 0.100000000000000005: E takes its exact twin, L the other (flagged): two trades whatever the order.
    let row = |fee: &str| format!("2026-10-29T10:00:00Z,buy,BTC,1,USDT,100,USDT,{fee},kucoin,,,");
    let upload = [row("0.10000000000000001"), row("0.100000000000000005")];
    let mut seen = None;
    for order in permutations(&upload) {
        for legacy_first in [true, false] {
            let fx = Fx::new();
            assert_eq!(fx.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
            let (l, e) = (row("0.1"), row("0.10000000000000001"));
            let strip = |fx: &Fx| fx.c.execute("UPDATE account_transactions SET raw_data = '{}' WHERE id = (SELECT max(id) FROM account_transactions)", []).unwrap();
            if legacy_first { fx.restore(&[&l]); strip(&fx); fx.restore(&[&e]); } else { fx.restore(&[&e]); fx.restore(&[&l]); strip(&fx); }
            let order: Vec<&str> = order.iter().map(String::as_str).collect();
            fx.restore(&order);
            let got = ledger(&fx);
            assert_eq!(got.0.len(), 2, "{order:?} legacy_first={legacy_first}: {got:?}");
            assert_eq!(got.1, vec!["2026-10-29 10:00:00".to_string()], "the precision merge is flagged");
            fx.restore(&order);
            assert_eq!(ledger(&fx), got, "a re-upload changes nothing");
            match &seen { Some(s) => assert_eq!(s, &got.0), None => seen = Some(got.0) }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn api_and_an_idless_restore_with_another_quote_are_both_kept_and_flagged() {
    for api_first in [true, false] {
        let fx = Fx::new();
        let api = Api::new("hf", &["BTC-USDT"]);
        api.hf(vec![hf_fill("BTC-USDT", "buy", "1", "100", "0", "2026-10-30T10:00:00Z")]);
        let restore = ["2026-10-30T10:00:00Z,buy,BTC,1,USDT,200,,,kucoin,,,"];
        if api_first { assert_eq!(fx.sync(&api).await, Ok(true)); fx.restore(&restore); } else {
            assert_eq!(fx.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true)); fx.restore(&restore);
            assert_eq!(fx.sync_with(&api, &fixed("2026-10-30T12:10:00Z"), false).await, Ok(true));
        }
        assert_eq!(fx.trades().len(), 1, "the restore is the record at its second; the API fill is withheld");
        assert_eq!(fx.ambiguous(), vec!["2026-10-30 10:00:00".to_string()], "and they differ: flagged");
    }
}

// ---- L10: the gap line ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn l10_exports_close_the_gap_only_when_they_cover_trades() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    fx.c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, raw_data, manual_values, transfer_link_rejected, created_at, updated_at) \
                  VALUES (?1, ?2, ?3, 4, 'USDT', 10, '2026-09-01 00:00:00', '{}', '{}', 0, '2026-09-01', '2026-09-01')", rusqlite::params![fx.user, fx.key, fx.exchange]).unwrap();
    assert_eq!(fx.sync(&api).await, Ok(true));
    let unit_start = "2026-10-23 18:00:00";
    assert_eq!(fx.uncovered(), json!([["2026-09-01 00:00:00", unit_start]]));
    assert!(fx.line());
    // Two disjoint exports leave a hole; a currency-filtered or a funding-only file closes nothing; a bridging one does.
    fx.import(&file(&[buy_legs("1", "1", "0", "2026-09-01 00:00:00"), buy_legs("1", "1", "0", "2026-09-10 00:00:00")]));
    fx.import(&file(&[buy_legs("1", "1", "0", "2026-09-20 00:00:00"), buy_legs("1", "1", "0", "2026-10-24 00:00:00")]));
    assert_eq!(fx.uncovered(), json!([["2026-09-10 00:00:01", "2026-09-20 00:00:00"]]));
    fx.import(&export(&["BTC,Deposit,1,0,2026-09-05 00:00:00,Spot", "BTC,Deposit,1,0,2026-09-25 00:00:00,Spot"]));
    fx.import(&export(&["USDT,Deposit,1,0,2026-09-05 00:00:00,Deposit", "USDT,Deposit,1,0,2026-09-25 00:00:00,Deposit"]));
    assert_eq!(fx.uncovered(), json!([["2026-09-10 00:00:01", "2026-09-20 00:00:00"]]));
    fx.import(&file(&[buy_legs("1", "1", "0", "2026-09-09 00:00:00"), buy_legs("1", "1", "0", "2026-09-21 00:00:00")]));
    assert_eq!(fx.uncovered(), json!([]));
    assert!(!fx.line());
}

// ---- balances and budget ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn balances_read_the_trade_hf_account_and_uta_refuses_keeping_rows() {
    use deltabadger::sync::balances;
    for (mode, kind) in [("hf", Some("trade_hf")), ("lf", Some("trade")), ("uta", None)] {
        let fx = Fx::new();
        let api = Api::new(mode, &[]);
        let credentials = deltabadger::sync::validation_material(&fx.c, &fx.cipher, fx.key).unwrap().0;
        let out = balances::sync_kucoin(&fx.db(), api.clone(), &balances::NoPrices, fx.key, &credentials, &fixed(NOW)).await.unwrap();
        let sent = api.requests("/api/v1/accounts");
        match kind {
            Some(kind) => { assert!(out.is_ok(), "{mode}"); assert_eq!(sent[0].query, vec![("type", kind.to_string())]); }
            None => { assert!(out.is_err() && sent.is_empty(), "{mode}"); }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn budget_one_symbol_crossing_three_deadlines_resumes_at_its_cursor_and_a_rotation_restarts_the_window() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.hf((0..250).map(|i| hf_fill("BTC-USDT", "buy", "1", "1", "0", &format!("2026-10-29T10:{:02}:{:02}Z", i / 60, i % 60))).collect());
    fx.watermark();
    let mut runs = 0;
    while fx.coverage("hf_api").is_empty() { assert!(fx.sync_with(&api, &fixed(NOW), true).await.is_ok()); runs += 1; assert!(runs < 10); }
    assert_eq!(runs, 4, "a page a run: 100, 100, 50, then the empty page");
    assert_eq!(fx.trades().len(), 250);
    assert_eq!(fx.coverage("hf_api").len(), 1);
    let cursors: Vec<String> = api.requests(HF_FILLS).iter().filter_map(|r| r.query.get(4).map(|q| q.1.clone())).collect();
    assert_eq!(cursors.len(), 3, "each run resumes at the returned lastId");
    // A credential rotation discards the progress: the same window from its first symbol, re-reads duplicates.
    let fx = Fx::new();
    fx.watermark();
    assert_eq!(fx.sync_with(&api, &fixed(NOW), true).await, Ok(false));
    fx.c.execute("UPDATE api_keys SET passphrase = ?1", [fx.cipher.encrypt("rotated")]).unwrap();
    let before = api.requests(HF_FILLS).len();
    assert!(fx.sync_with(&api, &fixed(NOW), true).await.is_ok());
    assert_eq!(api.requests(HF_FILLS)[before].query.len(), 4, "the same window from its first page");
    while fx.coverage("hf_api").is_empty() { assert!(fx.sync_with(&api, &fixed(NOW), true).await.is_ok()); }
    assert_eq!(fx.trades().len(), 250);
}


#[tokio::test(flavor = "current_thread")]
async fn a_classic_page_reading_one_id_two_ways_fails_with_nothing_written() {
    let fx = Fx::new();
    let api = Api::new("hf", &["BTC-USDT"]);
    api.classic(vec![classic_fill("9", "1", "100", "0.1", "2026-10-29T10:00:00Z"), classic_fill("9", "2", "200", "0.2", "2026-10-29T10:00:00Z")]);
    assert!(fx.sync(&api).await.is_err());
    assert!(fx.trades().is_empty());
    assert!(fx.error().is_some_and(|e| e.contains("read two ways")));
}

#[tokio::test(flavor = "current_thread")]
async fn an_export_of_rows_all_held_still_bridges_the_gap() {
    let fx = Fx::new();
    assert_eq!(fx.sync(&Api::new("hf", &["BTC-USDT"])).await, Ok(true));
    let (a, b) = (buy_legs("1", "1", "0", "2026-09-01 00:00:00"), buy_legs("1", "1", "0", "2026-10-24 00:00:00"));
    fx.import(&file(std::slice::from_ref(&a)));
    fx.import(&file(std::slice::from_ref(&b)));
    assert!(fx.uncovered().as_array().is_some_and(|u| !u.is_empty()));
    fx.import(&file(&[a, b]));
    assert_eq!(fx.uncovered(), json!([]));
}
