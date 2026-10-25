//! Kraken's ledger sync (plan Kraken sync, slices K2 and K3) against a Kraken that serves its ledger as the docs describe
//! it: `with_cursor` pages of `per` entries, the currency ledger by default and the tokenized one under
//! `aclass=tokenized_asset`, `start` exclusive. Rows are checked as stored, and through the tracker's real walk.
mod common;
use chrono::{DateTime, TimeZone, Utc};
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::rc::Rc;

const KRAKEN: &str = "Exchanges::Kraken";
const SYNC_NOW: &str = "2026-09-20T02:00:00Z";

fn secs(day: u32, hour: u32) -> f64 { Utc.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap().timestamp() as f64 }

/// One Ledgers entry, in Kraken's key order.
fn entry(id: &str, asset: &str, kind: &str, subtype: &str, amount: &str, fee: &str, at: f64, refid: &str) -> (String, Value) {
    let aclass = if asset.ends_with('x') || asset.ends_with("SPV") { "tokenized_asset" } else { "currency" };
    (id.to_string(), json!({ "refid": refid, "time": at, "type": kind, "subtype": subtype, "aclass": aclass, "asset": asset, "amount": amount, "fee": fee, "balance": "0" }))
}

#[derive(Default)]
struct State {
    currency: Vec<(String, Value)>, tokenized: Vec<(String, Value)>, per: usize,
    /// Every request's form fields, sorted.
    sent: Vec<Vec<(String, String)>>,
    /// (request number, status, body): answered instead of the ledger.
    faults: VecDeque<(usize, u16, String)>,
    /// Cursors Kraken refuses once (their lifetime ran out).
    expired: HashSet<String>,
    /// Every page names the cursor it was asked with as the next.
    stuck: bool,
}

#[derive(Clone, Default)]
struct Kraken(Rc<RefCell<State>>);

impl Kraken {
    fn new(per: usize, rows: Vec<(String, Value)>) -> Self {
        let (tokenized, currency) = rows.into_iter().partition(|(_, e)| e["aclass"] == "tokenized_asset");
        Self(Rc::new(RefCell::new(State { currency, tokenized, per, ..Default::default() })))
    }
    fn sent(&self) -> Vec<String> { self.0.borrow().sent.iter().map(|f| f.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")).collect() }
    fn clear(&self) { self.0.borrow_mut().sent.clear(); }
}

impl Transport for Kraken {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        assert_eq!(r.path, deltabadger::venue::kraken::LEDGERS);
        let mut s = self.0.borrow_mut();
        let mut fields: Vec<(String, String)> = r.body.as_ref().and_then(Value::as_object).into_iter().flatten()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        fields.sort();
        s.sent.push(fields.clone());
        let n = s.sent.len();
        if s.faults.front().is_some_and(|f| f.0 == n) {
            let (_, status, body) = s.faults.pop_front().unwrap();
            return Ok(HttpResponse { status, body });
        }
        let get = |k: &str| fields.iter().find(|(name, _)| name == k).map(|(_, v)| v.clone());
        let cursor = get("cursor");
        if let Some(c) = cursor.as_ref().filter(|c| s.expired.contains(*c)) {
            let c = c.clone();
            s.expired.remove(&c);
            return Ok(HttpResponse { status: 200, body: json!({ "error": ["EGeneral:Invalid arguments:cursor"] }).to_string() });
        }
        let start = get("start").map(|v| v.parse::<f64>().unwrap());
        let rows: Vec<&(String, Value)> = if get("aclass").as_deref() == Some("tokenized_asset") { &s.tokenized } else { &s.currency }
            .iter().filter(|(_, e)| start.is_none_or(|st| e["time"].as_f64().unwrap() > st)).collect();
        let from = cursor.as_ref().map_or(0, |c| c[1..].parse::<usize>().unwrap());
        let page: serde_json::Map<String, Value> = rows.iter().skip(from).take(s.per).map(|(id, e)| (id.clone(), e.clone())).collect();
        let next = if s.stuck { Some(cursor.clone().unwrap_or_else(|| "c1".into())) } else if from + s.per < rows.len() { Some(format!("c{}", from + s.per)) } else { None };
        let mut result = json!({ "ledger": page, "count": null });
        if let Some(next) = next { result["cursor"] = json!({ "next": next }); }
        Ok(HttpResponse { status: 200, body: json!({ "error": [], "result": result }).to_string() })
    }
}

/// A run with room for every page, or for its first page only.
struct Pace { paused: bool }
impl Stopwatch for Pace { fn elapsed(&self) -> std::time::Duration { std::time::Duration::from_secs(if self.paused { 1_000_000 } else { 0 }) } }

/// One stored row: tx_id, type, currency, amount, fee currency, fee, asset, the linked row's tx_id.
type Row = (String, i64, String, f64, Option<String>, Option<f64>, Option<i64>, Option<String>);

struct Fx { dir: tempfile::TempDir, c: Connection, cipher: deltabadger::crypto::Cipher, exchange: i64, binance: i64, assets: BTreeMap<&'static str, i64> }

impl Fx {
    fn new() -> Self {
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        let exchange = |kind: &str, name: &str| {
            c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?2, '0.1', '0.1', '2026-01-01', '2026-01-01')", [kind, name]).unwrap();
            c.last_insert_rowid()
        };
        let (kraken, binance) = (exchange(KRAKEN, "Kraken"), exchange("Exchanges::Binance", "Binance"));
        let mut assets = BTreeMap::new();
        for (symbol, external, category, kind) in [("USD", "usd-k", "Fiat", None), ("BTC", "bitcoin-k", "Cryptocurrency", None), ("ETH", "ethereum-k", "Cryptocurrency", None),
                                                   ("DOT", "polkadot-k", "Cryptocurrency", None), ("DOGE", "dogecoin-k", "Cryptocurrency", None), ("LTC", "litecoin-k", "Cryptocurrency", None),
                                                   ("NVDAx", "nvidia-xstock-k", "Cryptocurrency", Some("tokenized")), ("LUNC", "terra-luna-k", "Cryptocurrency", None),
                                                   ("TWIN", "twin-a-k", "Cryptocurrency", None), ("TWIN2", "twin-b-k", "Cryptocurrency", None)] {
            c.execute("INSERT INTO assets (external_id, symbol, name, category, instrument_type, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, ?4, '2026-01-01', '2026-01-01')",
                      params![external, if symbol == "TWIN2" { "TWIN" } else { symbol }, category, kind]).unwrap();
            assets.insert(symbol, c.last_insert_rowid());
        }
        // As Exchanges::Kraken#get_tickers_info stores them: the wsname's base (XBT for BTC, XDG for DOGE, LUNA for LUNC).
        for (base, symbol) in [("XBT", "BTC"), ("ETH", "ETH"), ("DOT", "DOT"), ("XDG", "DOGE"), ("LTC", "LTC"), ("NVDAx", "NVDAx"), ("LUNA", "LUNC"), ("TWINA", "TWIN"), ("TWINB", "TWIN2")] {
            c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                       minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?3, 'USD', ?4, ?5, 8, 5, 1, '0', '0', 1, 1, '2026-01-01', '2026-01-01')",
                      params![kraken, format!("{base}USD"), base, assets[symbol], assets["USD"]]).unwrap();
        }
        Self { dir, c, cipher: common::seed::cipher(), exchange: kraken, binance, assets }
    }
    /// A user with a reading Kraken key: (user, key).
    fn user(&self, email: &str) -> (i64, i64) {
        self.c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES (?1, 'x', 'Owner', 1, '2026-01-01', '2026-01-01')", [email]).unwrap();
        let user = self.c.last_insert_rowid();
        let e = |v: &str| self.cipher.encrypt(v);
        self.c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, 1, '2026-01-01', '2026-01-01')",
                       params![user, self.exchange, e(email), e("c2VjcmV0")]).unwrap();
        (user, self.c.last_insert_rowid())
    }
    /// The key's watermark, as this credential produced it (an incremental sync from `utc` less the overlap).
    fn watermark(&self, key: i64, utc: &str) {
        self.c.execute("UPDATE api_keys SET last_synced_at = ?1 WHERE id = ?2", params![deltabadger::sync::sql_time(utc.parse().unwrap()), key]).unwrap();
        let producer = deltabadger::engine::model::credential_version_by_id(&self.c, key).unwrap().unwrap();
        let tx = self.c.unchecked_transaction().unwrap();
        deltabadger::sync::cache::record_ledger(&deltabadger::engine::model::check_credential_result(&tx, &Some(producer.clone())).unwrap(), key, &producer, SYNC_NOW.parse().unwrap()).unwrap();
        tx.commit().unwrap();
    }
    /// One run of the key's ledger sync: whether it completed, or its failure.
    async fn sync(&self, key: i64, api: &Kraken, paused: bool) -> Result<bool, String> {
        let c = Connection::open(self.dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        let db = deltabadger::jobs::Db::new(c, common::seed::cipher());
        let credentials = deltabadger::sync::validation_material(&self.c, &self.cipher, key).unwrap().0;
        let pace = Pace { paused };
        let run = Run { deadline: std::time::Duration::from_secs(3600), chain_runs: usize::MAX, stopwatch: &pace };
        let clock = deltabadger::engine::FixedClock(SYNC_NOW.parse().unwrap());
        let out = deltabadger::sync::kraken_ledger::sync_within(&db, api.clone(), key, &credentials, &clock, std::time::Duration::ZERO, &run).await.unwrap();
        db.settled().await;
        out.map(|o| o.complete).map_err(|f| f.error)
    }
    /// Runs until the import completes; how many runs it took.
    async fn sync_through(&self, key: i64, api: &Kraken, paused: bool) -> usize {
        for n in 1..100 { if self.sync(key, api, paused).await.unwrap() { return n; } }
        panic!("the import does not complete");
    }
    fn rows(&self, user: i64) -> Vec<Row> {
        let mut s = self.c.prepare("SELECT t.tx_id, t.entry_type, t.base_currency, t.base_amount, t.fee_currency, t.fee_amount, t.base_asset_id, l.tx_id
                                    FROM account_transactions t LEFT JOIN account_transactions l ON l.id = t.linked_transaction_id WHERE t.user_id = ?1 ORDER BY t.tx_id").unwrap();
        s.query_map([user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
    fn row(&self, user: i64, tx: &str) -> Row { self.rows(user).into_iter().find(|r| r.0 == tx).unwrap_or_else(|| panic!("no row {tx}")) }
    /// Every column of the user's rows, as text: what "nothing changed" compares.
    fn dump(&self, user: i64) -> String {
        let mut s = self.c.prepare("SELECT * FROM account_transactions WHERE user_id = ?1 ORDER BY id").unwrap();
        let n = s.column_count();
        let rows: Vec<String> = s.query_map([user], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))).unwrap().map(Result::unwrap).collect();
        rows.join("\n")
    }
    fn deferred(&self, key: i64) -> Option<Value> {
        self.c.query_row("SELECT value FROM app_configs WHERE key = ?1", [format!("rust_sync.kraken_repair_deferred:{key}")], |r| r.get::<_, String>(0)).ok().map(|t| serde_json::from_str(&t).unwrap())
    }
    /// A row as Rails' sync wrote it (Exchanges::Kraken#normalize_kraken_ledger_entry): `|amount|`, the fee beside it.
    #[allow(clippy::too_many_arguments)]
    fn rails_row(&self, user: i64, key: i64, (id, e): &(String, Value), kind: i64, currency: &str, fee: Option<f64>, asset: Option<i64>) -> i64 {
        let amount = e["amount"].as_str().unwrap().trim_start_matches('-').parse::<f64>().unwrap();
        let at = deltabadger::sync::sql_time(DateTime::<Utc>::from_timestamp(e["time"].as_f64().unwrap() as i64, 0).unwrap());
        self.c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, fee_currency, fee_amount, tx_id, group_id,
                        transacted_at, raw_data, manual_values, base_asset_id, transfer_link_rejected, created_at, updated_at)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, '{}', ?13, 0, '2026-09-01', '2026-09-01')",
                       params![user, key, self.exchange, kind, currency, amount, fee.map(|_| currency), fee, id, e["refid"].as_str(), at, e.to_string(), asset]).unwrap();
        self.c.last_insert_rowid()
    }
    fn other_venue_row(&self, user: i64, kind: i64, currency: &str, amount: f64, at: f64) -> i64 {
        let at = deltabadger::sync::sql_time(DateTime::<Utc>::from_timestamp(at as i64, 0).unwrap());
        self.c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, manual_values,
                        transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '{}', '{}', 0, '2026-09-01', '2026-09-01')",
                       params![user, self.binance, kind, currency, amount, format!("bn-{amount}-{kind}"), at]).unwrap();
        self.c.last_insert_rowid()
    }
    fn link(&self, withdrawal: i64, deposit: i64) {
        self.c.execute("UPDATE account_transactions SET linked_transaction_id = ?1 WHERE id = ?2", [deposit, withdrawal]).unwrap();
    }
    async fn link_transfers(&self, user: i64) {
        let c = Connection::open(self.dir.path().join("production.sqlite3")).unwrap();
        let db = deltabadger::jobs::Db::new(c, common::seed::cipher());
        deltabadger::sync::ledger::link_transfers(&db, user, SYNC_NOW.parse().unwrap()).await.unwrap();
        db.settled().await;
    }
}

/// The tracker's real walk of the user's rows (enriched with links and transfer fees, opened where it must be, then walked
/// account-wide and located): each coin's quantity and each cash currency's balance at the end, and the openings it
/// synthesized.
fn walked(fx: &Fx, user: i64) -> (BTreeMap<String, f64>, BTreeMap<String, f64>, usize, deltabadger::tracker::walk::Walk) {
    use deltabadger::tracker::{fx::FxTable, prices::{NoFetch, PriceService, Reference}, walk};
    let stored = deltabadger::tracker::rows::load(&fx.c, user).unwrap();
    let deposits = stored.iter().map(|r| (r.id, r.amount.clone())).collect();
    let (reference, table) = (Reference::default(), FxTable::default());
    let mut none = NoFetch;
    let mut ps = PriceService::new(&reference, &table, chrono::NaiveDate::from_ymd_opt(2026, 9, 20).unwrap(), &mut none);
    let w = walk::walk_rows(&stored, &deposits, &mut ps).unwrap();
    let (mut coins, mut cash) = (BTreeMap::<String, f64>::new(), BTreeMap::<String, f64>::new());
    for r in w.rows.iter().filter(|r| !r.opening) {
        for (s, a) in walk::quantity_moves(r.kind, &r.base, &r.amount, r.fee_currency.as_deref(), r.fee_amount.as_ref(), r.linked, r.transfer_fee.as_ref()).unwrap() {
            if s != "USD" { *coins.entry(s).or_default() += a.to_f(); }
        }
        for (s, a) in walk::cash_moves(r.kind, &r.base, &r.amount, r.quote.as_deref(), r.quote_amount.as_ref(), r.fee_currency.as_deref(), r.fee_amount.as_ref()).unwrap() {
            *cash.entry(s).or_default() += a.to_f();
        }
    }
    let openings = w.rows.iter().filter(|r| r.opening).count();
    (coins, cash, openings, w)
}

/// Σ(amount − fee) of the raw entries, by the symbol each code is written under.
fn deltas(entries: &[(String, Value)], symbol: &dyn Fn(&str) -> String) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::<String, f64>::new();
    for (_, e) in entries {
        let n = |k: &str| e[k].as_str().unwrap().parse::<f64>().unwrap();
        *out.entry(symbol(e["asset"].as_str().unwrap())).or_default() += n("amount") - n("fee");
    }
    out
}

fn close(a: &BTreeMap<String, f64>, b: &BTreeMap<String, f64>) -> bool {
    a.keys().chain(b.keys()).all(|k| (a.get(k).copied().unwrap_or(0.0) - b.get(k).copied().unwrap_or(0.0)).abs() < 1e-9)
}

fn symbol_of(code: &str) -> String {
    match code { "ZUSD" => "USD", "XXBT" | "XBT.M" => "BTC", "XETH" | "ETH.S" | "ETH.F" => "ETH", "DOT.S" => "DOT", "XXDG" => "DOGE", "XLTC" => "LTC", "NVDASPV" => "NVDAx", other => other }.to_string()
}

// ---- the walk (K2) ---------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_throttled_page_is_asked_again_on_the_same_cursor() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let api = Kraken::new(1, vec![entry("L1", "XETH", "deposit", "", "1", "0", secs(16, 1), "D1"), entry("L2", "XETH", "deposit", "", "2", "0", secs(16, 2), "D2")]);
    // The second page is throttled twice: Kraken's error, then an HTTP 429 (read as the same throttle, Spec E §2.10).
    api.0.borrow_mut().faults = VecDeque::from([(2, 200, json!({ "error": ["EAPI:Rate limit exceeded"] }).to_string()), (3, 429, String::new())]);
    assert_eq!(fx.sync(key, &api, false).await, Ok(true));
    assert_eq!(api.sent(), ["with_cursor=true", "cursor=c1&with_cursor=true", "cursor=c1&with_cursor=true", "cursor=c1&with_cursor=true", "aclass=tokenized_asset&with_cursor=true"]);
    assert_eq!(fx.rows(user).len(), 2);
    // Throttled past its retries: the sync fails, nothing of a fresh import is stored.
    let (user, key) = fx.user("p@example.com");
    let api = Kraken::new(1, vec![entry("L1", "XETH", "deposit", "", "1", "0", secs(16, 1), "D1")]);
    api.0.borrow_mut().faults = (1..=7).map(|n| (n, 200, json!({ "error": ["EAPI:Rate limit exceeded"] }).to_string())).collect();
    assert_eq!(fx.sync(key, &api, false).await, Err("EAPI:Rate limit exceeded".into()));
    assert_eq!(api.sent().len(), 7);
    assert!(fx.rows(user).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_cursor_that_does_not_move_fails_the_sync() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let api = Kraken::new(1, vec![entry("L1", "XETH", "deposit", "", "1", "0", secs(16, 1), "D1"), entry("L2", "XETH", "deposit", "", "2", "0", secs(16, 2), "D2")]);
    api.0.borrow_mut().stuck = true;
    assert_eq!(fx.sync(key, &api, false).await, Err("Kraken currency ledger paging is stuck: the cursor did not move".into()));
    assert_eq!(api.sent(), ["with_cursor=true", "cursor=c1&with_cursor=true"]);
    assert!(fx.rows(user).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejected_resumed_cursor_restarts_its_source_once() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let api = Kraken::new(1, (1..=3).map(|i| entry(&format!("L{i}"), "XETH", "deposit", "", "1", "0", secs(16, i), &format!("D{i}"))).collect());
    assert_eq!(fx.sync(key, &api, true).await, Ok(false));
    api.clear();
    api.0.borrow_mut().expired.insert("c1".into());
    assert_eq!(fx.sync(key, &api, false).await, Ok(true));
    assert_eq!(api.sent(), ["cursor=c1&with_cursor=true", "with_cursor=true", "cursor=c1&with_cursor=true", "cursor=c2&with_cursor=true", "aclass=tokenized_asset&with_cursor=true"]);
    assert_eq!(fx.rows(user).iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["L1", "L2", "L3"], "the page read again is a duplicate");
    // A cursor refused after the restart (one this run was given) fails the sync.
    let (_, key) = fx.user("p@example.com");
    assert_eq!(fx.sync(key, &api, true).await, Ok(false));
    api.0.borrow_mut().expired.insert("c1".into());
    api.0.borrow_mut().faults = VecDeque::from([(api.sent().len() + 3, 200, json!({ "error": ["EGeneral:Invalid arguments:cursor"] }).to_string())]);
    assert_eq!(fx.sync(key, &api, false).await, Err("EGeneral:Invalid arguments:cursor".into()));
}

/// D-KR-W: both walks with their exact params; each run of a cut import resumes its source at its own cursor, and the
/// tokenized history, over pages and runs, is stored whole.
#[tokio::test(flavor = "current_thread")]
async fn a_cut_run_resumes_each_source_where_it_stopped() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    fx.watermark(key, "2026-09-15T00:00:00Z");
    let start = Utc.with_ymd_and_hms(2026, 9, 13, 23, 0, 0).unwrap().timestamp();
    let api = Kraken::new(1, vec![entry("L1", "XETH", "deposit", "", "1", "0", secs(16, 1), "D1"), entry("L2", "ZUSD", "deposit", "", "100", "0", secs(16, 2), "D2"),
                                  entry("X1", "NVDAx", "trade", "", "1.5", "0", secs(16, 3), "T1"), entry("X2", "NVDAx", "trade", "", "-0.5", "0.001", secs(17, 3), "T2"),
                                  entry("L0", "XETH", "deposit", "", "9", "0", secs(13, 1), "D0")]);
    let mut runs = vec![];
    loop {
        api.clear();
        let complete = fx.sync(key, &api, true).await.unwrap();
        runs.push(api.sent());
        if complete { break; }
    }
    let s = |t: &str| t.replace("S", &start.to_string());
    assert_eq!(runs, [vec![s("start=S&with_cursor=true")], vec![s("cursor=c1&start=S&with_cursor=true")], vec![s("aclass=tokenized_asset&start=S&with_cursor=true")],
                      vec![s("aclass=tokenized_asset&cursor=c1&start=S&with_cursor=true")]]);
    assert_eq!(fx.rows(user).iter().map(|r| (r.0.as_str(), r.1, r.2.as_str(), r.3, r.6)).collect::<Vec<_>>(),
               [("L1", 4, "ETH", 1.0, Some(fx.assets["ETH"])), ("L2", 4, "USD", 100.0, Some(fx.assets["USD"])), ("X1", 0, "NVDAx", 1.5, Some(fx.assets["NVDAx"])),
                ("X2", 1, "NVDAx", 0.5, Some(fx.assets["NVDAx"]))], "everything after the exclusive start, nothing before it");
    let watermark: String = fx.c.query_row("SELECT last_synced_at FROM api_keys WHERE id = ?1", [key], |r| r.get(0)).unwrap();
    assert_eq!(watermark, "2026-09-17 03:00:00", "the newest row read, once the import completes");
}

// ---- divergences, links and repair (K3) ------------------------------------------------------------------------------

/// D-KR-F with the links of D-KR-4: the real enriched walk holds, per asset, exactly Σ(amount − fee) of Kraken's entries,
/// with no opening synthesized, for coins and for cash; and that is Kraken's own final balance of every code.
#[tokio::test(flavor = "current_thread")]
async fn the_walk_holds_what_kraken_holds_with_no_openings() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let mut entries = vec![
        entry("A1", "ZUSD", "deposit", "", "1000.0000", "2.0000", secs(10, 1), "D1"),
        entry("A2", "ZUSD", "trade", "", "-600.0000", "1.2000", secs(10, 2), "T1"), entry("A3", "XXBT", "trade", "", "0.0100000000", "0.0000100000", secs(10, 2), "T1"),
        entry("A4", "XETH", "deposit", "", "2.0000000000", "0.0010000000", secs(10, 3), "D2"),
        // An internal move with its fee on the withdrawal leg, and one with it on the deposit leg.
        entry("A5", "XETH", "transfer", "spottostaking", "-1.0000000000", "0.0100000000", secs(11, 1), "P1"), entry("A6", "ETH.S", "transfer", "stakingfromspot", "1.0000000000", "0", secs(11, 1), "P1"),
        entry("A7", "ETH.S", "transfer", "stakingtospot", "-0.5000000000", "0", secs(12, 1), "P2"), entry("A8", "XETH", "transfer", "spotfromstaking", "0.5000000000", "0.0050000000", secs(12, 1), "P2"),
        entry("A9", "ETH.S", "staking", "", "0.0100000000", "0.0010000000", secs(13, 1), "S1"),
        entry("B1", "XXBT", "withdrawal", "", "-0.0020000000", "0.0001000000", secs(14, 1), "W1"),
        entry("B2", "ZUSD", "trade", "", "200.0000", "-0.0500", secs(15, 1), "T2"), entry("B3", "XXBT", "trade", "", "-0.0030000000", "0", secs(15, 1), "T2"),
        entry("B4", "ZUSD", "rollover", "", "-1.0000", "0", secs(15, 2), "R1"), entry("B5", "XETH", "earn", "allocation", "-0.2000000000", "0", secs(16, 1), "E1"),
        entry("B6", "ETH.F", "earn", "allocation", "0.2000000000", "0", secs(16, 1), "E1"),
        entry("B7", "NVDAx", "trade", "", "1.0000000000", "0.0010000000", secs(17, 1), "T3"), entry("B8", "ZUSD", "trade", "", "-180.0000", "0.1800", secs(17, 1), "T3"),
        entry("B9", "XXBT", "trade", "", "0", "-0.0000010000", secs(18, 1), "T4"),
    ];
    // Kraken's running balance of each code after each entry.
    let mut balances: BTreeMap<String, f64> = BTreeMap::new();
    for (_, e) in entries.iter_mut() {
        let n = |k: &str| e[k].as_str().unwrap().parse::<f64>().unwrap();
        let b = balances.entry(e["asset"].as_str().unwrap().to_string()).or_default();
        *b += n("amount") - n("fee");
        e["balance"] = json!(format!("{b:.10}"));
    }
    let api = Kraken::new(3, entries.clone());
    fx.sync_through(key, &api, false).await;
    let linked: Vec<(String, Option<String>)> = fx.rows(user).into_iter().filter(|r| r.7.is_some()).map(|r| (r.0, r.7)).collect();
    assert_eq!(linked, [("A5".into(), Some("A6".into())), ("A7".into(), Some("A8".into())), ("B5".into(), Some("B6".into()))], "each internal pair is linked by its refid");
    let (coins, cash, openings, _) = walked(&fx, user);
    assert_eq!(openings, 0, "no opening synthesized");
    let expected = deltas(&entries, &symbol_of);
    let (want_cash, want_coins): (BTreeMap<String, f64>, BTreeMap<String, f64>) = expected.into_iter().partition(|(s, _)| s == "USD");
    assert!(close(&coins, &want_coins), "coins: walk {coins:?}, Kraken {want_coins:?}");
    assert!(close(&cash, &want_cash), "cash: walk {cash:?}, Kraken {want_cash:?}");
    // Kraken's own final balances, summed per asset, are the same.
    let mut kraken = BTreeMap::<String, f64>::new();
    for (code, b) in &balances { *kraken.entry(symbol_of(code)).or_default() += b; }
    let walk: BTreeMap<String, f64> = coins.into_iter().chain(cash).collect();
    assert!(close(&walk, &kraken), "walk {walk:?}, Kraken's balances {kraken:?}");
}

/// r3 row 1: a negative fee (a maker rebate) is folded, so the lot engine meets none: a sell of 0.00499, a buy of the
/// rebate alone, and every lot covered.
#[tokio::test(flavor = "current_thread")]
async fn negative_fees_go_through_the_lot_engine_folded() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let entries = vec![
        entry("N1", "ZUSD", "deposit", "", "1000.0000", "0", secs(10, 1), "D1"),
        entry("N2", "XXBT", "trade", "", "0.0100000000", "-0.0001000000", secs(11, 1), "T1"), entry("N3", "ZUSD", "trade", "", "-600.0000", "-0.1000", secs(11, 1), "T1"),
        entry("N4", "XXBT", "trade", "", "-0.0050000000", "-0.0000100000", secs(12, 1), "T2"), entry("N5", "ZUSD", "trade", "", "300.0000", "-0.0500", secs(12, 1), "T2"),
        entry("N6", "XXBT", "trade", "", "0", "-0.0000200000", secs(13, 1), "T3"),
    ];
    fx.sync_through(key, &Kraken::new(50, entries.clone()), false).await;
    let btc = Some(fx.assets["BTC"]);
    let rows: Vec<(String, i64, f64, Option<f64>)> = fx.rows(user).into_iter().filter(|r| r.6 == btc).map(|r| (r.0, r.1, r.3, r.5)).collect();
    assert_eq!(rows, [("N2".into(), 0, 0.0101, None), ("N4".into(), 1, 0.00499, None), ("N6".into(), 0, 0.00002, None)]);
    let (coins, cash, openings, w) = walked(&fx, user);
    assert_eq!(openings, 0);
    assert!(!w.global.uncovered && !w.engine.uncovered, "every disposal is covered by a lot");
    let sells: Vec<(String, f64, f64)> = w.global.disposals.iter().map(|d| (d.asset.clone(), d.amount.to_f(), d.fee.to_f())).collect();
    assert_eq!(sells.iter().filter(|s| s.0 == "BTC").cloned().collect::<Vec<_>>(), [("BTC".to_string(), 0.00499, 0.0)]);
    assert!((coins["BTC"] - 0.00513).abs() < 1e-12 && (cash["USD"] - 700.15).abs() < 1e-9, "{coins:?} {cash:?}");
}

/// D-KR-5: the first Rust sync re-reads the history Rails stored (no provenance stamp) and rewrites the syncing user's
/// rows, by the conflict rules; another user's rows under the same ledger ids are untouched. An old Deltabadger CSV with
/// the same tx_ids, imported after, changes nothing.
#[tokio::test(flavor = "current_thread")]
async fn the_upgrade_repairs_only_the_syncing_users_rows() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let (other, other_key) = fx.user("p@example.com");
    let e = |id, asset, kind, sub, amount, fee, h, refid| entry(id, asset, kind, sub, amount, fee, secs(16, h), refid);
    let (dot, doge, ltc, btc, eth, linked, rejected, conflict, manual) =
        (e("U1", "DOT.S", "staking", "", "0.2500000000", "0.0100000000", 1, "S1"), e("U2", "XXDG", "deposit", "", "100.00000000", "0", 2, "D2"),
         e("U3", "XLTC", "deposit", "", "2.0000000000", "0", 3, "D3"), e("U4", "XXBT", "withdrawal", "", "-0.0020000000", "0.0001000000", 4, "W4"),
         e("U5", "XETH", "deposit", "", "5.0000000000", "0.1000000000", 5, "D5"), e("U6", "XETH", "withdrawal", "", "-1.0000000000", "0.0100000000", 6, "W6"),
         e("U7", "XETH", "withdrawal", "", "-0.3000000000", "0.0010000000", 7, "W7"), e("U8", "DOT.S", "withdrawal", "", "-1.0000000000", "0", 8, "W8"),
         e("U9", "XXBT", "trade", "", "0.0100000000", "0", 0, "T9"));
    let (dot_id, eth_id) = (fx.assets["DOT"], fx.assets["ETH"]);
    for (u, k) in [(user, key), (other, other_key)] {
        fx.rails_row(u, k, &dot, 6, "DOT.S", Some(0.01), None);
        fx.rails_row(u, k, &doge, 4, "XDG", None, Some(fx.assets["DOGE"]));
        fx.rails_row(u, k, &ltc, 4, "XLTC", None, None);
        fx.rails_row(u, k, &btc, 5, "BTC", Some(0.0001), None);
        // A stored asset that is not the resolver's (DOT for an ETH deposit): the id wins, its currency stays.
        fx.rails_row(u, k, &eth, 4, "ETH", Some(0.1), Some(dot_id));
    }
    // Linked to another venue's deposit, still valid once rewritten (1.01 ≥ 0.99).
    let wd = fx.rails_row(user, key, &linked, 5, "ETH", Some(0.01), Some(eth_id));
    let dep = fx.other_venue_row(user, 4, "ETH", 0.99, secs(16, 7));
    fx.link(wd, dep);
    let gone = fx.rails_row(user, key, &rejected, 5, "ETH", Some(0.001), Some(eth_id));
    fx.c.execute("UPDATE account_transactions SET transfer_link_rejected = 1 WHERE id = ?1", [gone]).unwrap();
    // Linked to a deposit of "DOT.S" on another venue: rewritten to DOT the pair breaks, so the row stays, counted.
    let stuck = fx.rails_row(user, key, &conflict, 5, "DOT.S", None, None);
    let stuck_dep = fx.other_venue_row(user, 4, "DOT.S", 1.0, secs(16, 9));
    fx.link(stuck, stuck_dep);
    let priced = fx.rails_row(user, key, &manual, 0, "BTC", None, None);
    fx.c.execute("UPDATE account_transactions SET manual_values = '{\"price\":\"60000\"}' WHERE id = ?1", [priced]).unwrap();
    let before_other = fx.dump(other);
    let (dot_entry, manual_entry) = (dot.clone(), manual.clone());
    let entries = vec![dot, doge.clone(), ltc.clone(), btc.clone(), eth, linked, rejected, conflict, manual];
    fx.sync_through(key, &Kraken::new(50, entries), false).await;

    let (btc_id, doge_id, ltc_id) = (Some(fx.assets["BTC"]), Some(fx.assets["DOGE"]), Some(fx.assets["LTC"]));
    let r = |tx: &str| { let r = fx.row(user, tx); (r.1, r.2, r.3, r.4, r.5, r.6) };
    assert_eq!(r("U1"), (6, "DOT".into(), 0.24, None, None, Some(dot_id)), "DOT.S: currency and asset named, the fee folded");
    assert_eq!(r("U2"), (4, "DOGE".into(), 100.0, None, None, doge_id));
    assert_eq!(r("U3"), (4, "LTC".into(), 2.0, None, None, ltc_id), "D-KR-2");
    assert_eq!(r("U4"), (5, "BTC".into(), 0.0021, None, None, btc_id), "a net withdrawal with its fee folded, its asset filled");
    assert_eq!(r("U5"), (4, "ETH".into(), 4.9, None, None, Some(dot_id)), "the id wins: currency kept, amounts Kraken's");
    assert_eq!((r("U6").2, fx.row(user, "U6").7), (1.01, Some("bn-0.99-4".into())), "rewritten, the link kept");
    let rejected: i64 = fx.c.query_row("SELECT transfer_link_rejected FROM account_transactions WHERE id = ?1", [gone], |r| r.get(0)).unwrap();
    assert_eq!((r("U7").2, rejected), (0.301, 1), "rewritten, the rejection kept");
    assert_eq!(r("U8"), (5, "DOT.S".into(), 1.0, None, None, None), "a linked conflict is left untouched");
    let kept: String = fx.c.query_row("SELECT manual_values FROM account_transactions WHERE id = ?1", [priced], |r| r.get(0)).unwrap();
    assert_eq!((r("U9").5, kept.as_str()), (btc_id, "{\"price\":\"60000\"}"), "the asset filled, the manual price kept");
    assert_eq!(fx.dump(other), before_other, "the other user's rows under the same ids are untouched");
    assert!(fx.deferred(key).is_none(), "the deferred list goes at completion");
    // The walk: Kraken's own quantities where no link reaches another venue, and an opening only for the linked conflict
    // left as Rails stored it.
    let (coins, _, _, w) = walked(&fx, user);
    let opened: Vec<&str> = w.rows.iter().filter(|r| r.opening).map(|r| r.base.as_str()).collect();
    assert_eq!(opened, ["DOT.S"]);
    let kraken = deltas(&[dot_entry.clone(), doge.clone(), ltc.clone(), btc, manual_entry.clone()], &symbol_of);
    for coin in ["BTC", "DOGE", "LTC", "DOT"] { assert!((coins[coin] - kraken[coin]).abs() < 1e-12, "{coin}: walk {}, Kraken {}", coins[coin], kraken[coin]); }

    // An old Deltabadger CSV of the same rows (Rails' shapes) changes nothing.
    let after = fx.dump(user);
    let csv = format!("{}\n2026-09-16T01:00:00Z,staking_reward,DOT.S,0.25,,,DOT.S,0.01,kraken,U1,S1,\n2026-09-16T04:00:00Z,withdrawal,BTC,0.002,,,BTC,0.0001,kraken,U4,W4,\n\
                       2026-09-16T00:00:00Z,buy,BTC,0.01,,,,,kraken,U9,T9,", deltabadger::web::tracker::CSV_HEADERS.join(","));
    match deltabadger::tracker::import::run::import(&fx.c, &fx.cipher, user, "deltabadger", csv.as_bytes(), None, None, SYNC_NOW.parse().unwrap()).unwrap() {
        deltabadger::tracker::import::run::Answer::Written { imported, duplicates, .. } => assert_eq!((imported, duplicates), (0, 3)),
        other => panic!("{other:?}"),
    }
    assert_eq!(fx.dump(user), after, "the reimport changes no row");
}

/// r3 row 3: a Deltabadger CSV into an empty Kraken account resolves the canonical symbols it carries (BTC, LUNC, a
/// tokenized symbol); a symbol two listed assets share resolves to none.
#[tokio::test(flavor = "current_thread")]
async fn a_csv_into_an_empty_account_resolves_its_canonical_symbols() {
    let fx = Fx::new();
    let (user, _) = fx.user("o@example.com");
    let csv = format!("{}\n2026-09-16T01:00:00Z,deposit,BTC,0.5,,,,,kraken,C1,,\n2026-09-16T02:00:00Z,deposit,LUNC,1000,,,,,kraken,C2,,\n\
                       2026-09-16T03:00:00Z,buy,NVDAx,1.5,USD,270,,,kraken,C3,,\n2026-09-16T04:00:00Z,deposit,TWIN,3,,,,,kraken,C4,,", deltabadger::web::tracker::CSV_HEADERS.join(","));
    match deltabadger::tracker::import::run::import(&fx.c, &fx.cipher, user, "deltabadger", csv.as_bytes(), None, None, SYNC_NOW.parse().unwrap()).unwrap() {
        deltabadger::tracker::import::run::Answer::Written { imported, .. } => assert_eq!(imported, 4),
        other => panic!("{other:?}"),
    }
    assert_eq!(fx.rows(user).iter().map(|r| (r.0.as_str(), r.6)).collect::<Vec<_>>(),
               [("C1", Some(fx.assets["BTC"])), ("C2", Some(fx.assets["LUNC"])), ("C3", Some(fx.assets["NVDAx"])), ("C4", None)]);
}

/// r3 rows 2 and 5 through the real continuation path: a historical linked pair (stored 10/10) whose targets are 10.2/10.1,
/// the deposit met first. Each commit leaves a valid pair; the deposit, deferred, converges at completion; the transfer fee
/// is 0.1; a second sync changes nothing. A linked deposit whose target is a withdrawal is left untouched for good.
#[tokio::test(flavor = "current_thread")]
async fn a_historical_pair_converges_at_completion() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let wd_entry = entry("H1", "XETH", "transfer", "spottostaking", "-10.0000000000", "0.2000000000", secs(1, 1), "P1");
    let dep_entry = entry("H2", "ETH.S", "transfer", "stakingfromspot", "10.1000000000", "0", secs(1, 1), "P1");
    let flip_wd = entry("H3", "XETH", "withdrawal", "", "-1.0000000000", "0", secs(2, 1), "W3");
    let flip_dep = entry("H4", "XETH", "deposit", "", "-1.0000000000", "0", secs(2, 2), "D4");
    let eth = Some(fx.assets["ETH"]);
    let wd = fx.rails_row(user, key, &wd_entry, 5, "ETH", None, eth);
    let dep = fx.rails_row(user, key, &dep_entry, 4, "ETH", None, eth);
    fx.link(wd, dep);
    fx.c.execute("UPDATE account_transactions SET base_amount = 10 WHERE id = ?1", [dep]).unwrap();
    let (a, b) = (fx.rails_row(user, key, &flip_wd, 5, "ETH", None, eth), fx.rails_row(user, key, &flip_dep, 4, "ETH", None, eth));
    fx.link(a, b);
    // The deposit first, then the withdrawal, then a recent row that advances the watermark, a page each.
    let recent = entry("H5", "XXBT", "trade", "", "0.0100000000", "0", secs(19, 1), "T5");
    let api = Kraken::new(1, vec![dep_entry, wd_entry, flip_dep, flip_wd, recent]);
    let pair = |fx: &Fx| (fx.row(user, "H1").3, fx.row(user, "H2").3);
    assert_eq!(fx.sync(key, &api, true).await, Ok(false));
    assert_eq!(pair(&fx), (10.0, 10.0), "the deposit is deferred: 10.1 would outgrow its withdrawal");
    assert_eq!(fx.deferred(key).unwrap()["rows"].as_array().unwrap().len(), 1);
    assert_eq!(fx.sync(key, &api, true).await, Ok(false));
    assert_eq!(pair(&fx), (10.2, 10.0), "the withdrawal is written: valid against the stored deposit");
    fx.sync_through(key, &api, true).await;
    assert_eq!(pair(&fx), (10.2, 10.1), "converged at completion");
    assert_eq!(fx.row(user, "H4").1, 4, "a linked deposit whose target is a withdrawal is left");
    assert_eq!((fx.row(user, "H4").3, fx.row(user, "H3").3), (1.0, 1.0));
    assert!(fx.deferred(key).is_none());
    let watermark: String = fx.c.query_row("SELECT last_synced_at FROM api_keys WHERE id = ?1", [key], |r| r.get(0)).unwrap();
    assert_eq!(watermark, "2026-09-19 01:00:00");
    let (_, _, _, w) = walked(&fx, user);
    let fee = w.rows.iter().find(|r| r.linked && r.amount.to_f() == 10.2).and_then(|r| r.transfer_fee.clone()).unwrap().to_f();
    assert!((fee - 0.1).abs() < 1e-12, "the transfer fee is the fees of both legs: {fee}");
    let before = fx.dump(user);
    fx.sync_through(key, &api, false).await;
    assert_eq!(fx.dump(user), before, "a second sync changes nothing");
}

/// D-KR-4: an internal leg stored by a run that stopped before its other leg is not taken by another venue's matcher in
/// between; the next run links the pair by its refid.
#[tokio::test(flavor = "current_thread")]
async fn an_interrupted_internal_pair_is_linked_by_refid_later() {
    let fx = Fx::new();
    let (user, key) = fx.user("o@example.com");
    let api = Kraken::new(1, vec![entry("I1", "XETH", "transfer", "spottostaking", "-1.0000000000", "0.0050000000", secs(16, 1), "P1"),
                                  entry("I2", "ETH.S", "transfer", "stakingfromspot", "1.0000000000", "0", secs(16, 1), "P1")]);
    assert_eq!(fx.sync(key, &api, true).await, Ok(false));
    // Another venue's sync: a deposit the generic matcher would pair with the withdrawal leg (0 to 2 % below, within 72 h).
    let other = fx.other_venue_row(user, 4, "ETH", 0.995, secs(16, 2));
    fx.link_transfers(user).await;
    assert_eq!(fx.row(user, "I1").7, None, "the internal leg is not the generic matcher's");
    let claimed: i64 = fx.c.query_row("SELECT COUNT(*) FROM account_transactions WHERE linked_transaction_id = ?1", [other], |r| r.get(0)).unwrap();
    assert_eq!(claimed, 0);
    fx.sync_through(key, &api, true).await;
    assert_eq!(fx.row(user, "I1").7, Some("I2".into()), "linked by its refid");
}
