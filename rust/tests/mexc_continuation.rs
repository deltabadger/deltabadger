//! A MEXC ledger import over several runs (plan VB §1.6 on slice A5's walk): 7-day transfer ranges, a full page halved with
//! its halves' rows kept only when the range ends, a coin seen only on a deposit in one run whose pair's trades are read in
//! a later one, the frozen pair list, a trades read halved, and a run cut mid-range replayed. Every multi-run import stores
//! exactly the rows, and ends on exactly the watermark, of one run. Rails-free: the one-run walk against Rails is
//! sync_parity.rs's `mexc-ledger-` scenarios.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::Db;
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::sync::mexc_ledger;
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const NOW: &str = "2026-09-20T02:00:00Z";
const DAY: i64 = 86_400_000;

fn now_ms() -> i64 { NOW.parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_millis() }
fn creds() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }

/// A MEXC key with no watermark: a whole-history import.
fn install() -> (tempfile::TempDir, Db, i64) {
    let (dir, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Mexc', name = 'MEXC' WHERE id = ?1", [s.exchange_id]).unwrap();
    (dir, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}

/// The account: 1200 USDT deposits inside one 7-day range (a full page, halved), an ETH deposit in another, deposits in a
/// pending status, two withdrawals; BTC held; 250 BTCUSDT trades over the month (halved twice) and 3 ETHUSDT ones.
struct Account { deposits: Vec<Value>, withdrawals: Vec<Value>, trades: Vec<Value> }
fn account() -> Account {
    let n = now_ms();
    let deposit = |coin: &str, amount: String, at: i64, status: i64| json!({ "amount": amount, "coin": coin, "status": status, "txId": format!("0x{coin}{at}:0"), "insertTime": at });
    let mut deposits: Vec<Value> = (0..1200).map(|i| deposit("USDT", format!("1.{i:04}"), n - 40 * DAY + 3_600_000 + i * 60_000, 5)).collect();
    deposits.push(deposit("ETH", "0.5".into(), n - 20 * DAY, 12));
    deposits.push(deposit("XRP", "9".into(), n - 10 * DAY, 1));
    let withdrawals = vec![json!({ "id": "w1", "coin": "USDT", "amount": "3", "transactionFee": "1", "status": 7, "applyTime": n - 5 * DAY }),
                           json!({ "id": "w2", "coin": "USDT", "amount": "4", "transactionFee": "1", "status": 6, "applyTime": n - 4 * DAY })];
    let trade = |symbol: &str, i: i64| json!({ "symbol": symbol, "id": i, "orderId": format!("C02__{i}"), "price": format!("{}", 60_000 + i), "qty": "0.001",
                                                "quoteQty": "60", "commission": "-0.01", "commissionAsset": "USDT", "time": n - 25 * DAY + (i % 1000) * 3_600_000, "isBuyer": i % 2 == 0 });
    let mut trades: Vec<Value> = (0..250).map(|i| trade("BTCUSDT", i)).collect();
    trades.extend((1000..1003).map(|i| trade("ETHUSDT", i)));
    Account { deposits, withdrawals, trades }
}

#[derive(Default)]
struct Watch { at: Cell<Duration>, cut: tokio::sync::Notify }
impl Stopwatch for Watch { fn elapsed(&self) -> Duration { self.at.get() } }

/// MEXC as the walk reads it: every request `latency` long on the watch; each history the rows within [startTime, endTime],
/// at most `limit`, as MEXC pages nothing. `cut_at`: the runner's deadline cuts the run at that request (counted from 1).
#[derive(Clone)]
struct Venue { account: Rc<Account>, watch: Rc<Watch>, latency: Duration, log: Rc<RefCell<Vec<String>>>, cut_at: Rc<Cell<Option<usize>>> }
impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let n = self.log.borrow().len() + 1;
        self.watch.at.set(self.watch.at.get() + self.latency);
        if self.cut_at.get() == Some(n) { self.watch.cut.notify_one(); }
        self.log.borrow_mut().push(deltabadger::venue::http::request_key(r));
        let q = |k: &str| r.query.iter().find(|(name, _)| *name == k).map(|(_, v)| v.clone());
        let within = |rows: &[Value], time: &str| -> Value {
            let (from, to, limit): (i64, i64, usize) = (q("startTime").unwrap().parse().unwrap(), q("endTime").unwrap().parse().unwrap(), q("limit").unwrap().parse().unwrap());
            json!(rows.iter().filter(|d| (from..=to).contains(&d[time].as_i64().unwrap())).filter(|d| q("symbol").is_none_or(|s| d["symbol"] == s.as_str()))
                .take(limit).collect::<Vec<_>>())
        };
        let body = match r.path.as_str() {
            "/api/v3/capital/deposit/hisrec" => within(&self.account.deposits, "insertTime"),
            "/api/v3/capital/withdraw/history" => within(&self.account.withdrawals, "applyTime"),
            "/api/v3/account" => json!({ "canTrade": true, "balances": [{ "asset": "BTC", "free": "1", "locked": "0" }] }),
            "/api/v3/exchangeInfo" => json!({ "symbols": [{ "symbol": "BTCUSDT", "baseAsset": "BTC", "quoteAsset": "USDT" }, { "symbol": "ETHUSDT", "baseAsset": "ETH", "quoteAsset": "USDT" },
                                                          { "symbol": "XRPUSDT", "baseAsset": "XRP", "quoteAsset": "USDT" }] }),
            "/api/v3/myTrades" => within(&self.account.trades, "time"),
            other => panic!("unscripted {other}"),
        };
        Ok(HttpResponse { status: 200, body: body.to_string() })
    }
}

type Ran = Result<Result<deltabadger::sync::ledger::Outcome, deltabadger::sync::Failure>, deltabadger::sync::SyncError>;

async fn run(db: &Db, key: i64, v: &Venue, deadline: Duration) -> Option<Ran> {
    v.watch.at.set(Duration::ZERO);
    let run = Run { deadline, chain_runs: 1000, stopwatch: &*v.watch };
    let (credentials, clock) = (creds(), FixedClock(NOW.parse().unwrap()));
    let out = tokio::select! {
        biased;
        _ = v.watch.cut.notified() => None,
        out = mexc_ledger::sync_within(db, v.clone(), key, &credentials, &clock, &run) => Some(out),
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    out
}

/// Runs until the import completes; how many runs it took.
async fn import(db: &Db, key: i64, v: &Venue, deadline: Duration) -> usize {
    for runs in 1..=300 {
        if let Some(out) = run(db, key, v, deadline).await {
            if out.unwrap().unwrap().complete { return runs; }
        }
    }
    panic!("the import never completed");
}

async fn stored(db: &Db, key: i64) -> (Vec<Value>, Option<String>) {
    db.run(move |c, _| {
        let mut s = c.prepare("SELECT entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, tx_id, transacted_at, raw_data \
                               FROM account_transactions ORDER BY id").unwrap();
        let rows = s.query_map([], |r| Ok(json!([r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, f64>(2)?, r.get::<_, Option<String>>(3)?,
                                                 r.get::<_, Option<f64>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<f64>>(6)?, r.get::<_, Option<String>>(7)?,
                                                 r.get::<_, String>(8)?, r.get::<_, String>(9)?]))).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        let watermark: Option<String> = c.query_row("SELECT last_synced_at FROM api_keys WHERE id = ?1", [key], |r| r.get(0)).unwrap();
        Ok((rows, watermark))
    }).await.unwrap()
}

fn venue(account: &Rc<Account>, latency: Duration) -> Venue {
    Venue { account: account.clone(), watch: Rc::default(), latency, log: Rc::default(), cut_at: Rc::default() }
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_over_many_runs_stores_what_one_run_stores() {
    let account = Rc::new(account());
    let (_d1, one_db, one_key) = install();
    let one = venue(&account, Duration::from_millis(1));
    assert_eq!(import(&one_db, one_key, &one, Duration::from_secs(3600)).await, 1, "a short import is one run");
    let (rows, watermark) = stored(&one_db, one_key).await;
    // 1200 USDT and the ETH deposit (the pending XRP one is no entry), one completed withdrawal, 250 + 3 trades.
    assert_eq!(rows.len(), 1201 + 1 + 253);
    assert!(rows.iter().all(|r| r[6] != json!(-0.01)), "a commission is stored as its absolute value");
    let log = one.log.borrow().clone();
    let pairs: Vec<String> = log.iter().filter(|k| k.starts_with("GET /api/v3/myTrades")).map(|k| k.split("symbol=").nth(1).unwrap().split('&').next().unwrap().to_string()).collect();
    assert_eq!(pairs.iter().filter(|s| *s == "ETHUSDT").count(), 1, "ETH only deposited: its pair read once, unhalved");
    assert!(pairs.iter().filter(|s| *s == "BTCUSDT").count() >= 5 && !pairs.contains(&"XRPUSDT".to_string()), "BTCUSDT halved; XRP's deposit was not credited: {pairs:?}");
    assert!(log.iter().filter(|k| k.starts_with("GET /api/v3/capital/deposit/hisrec")).count() > 13, "13 ranges over 89 days, one halved");

    // Each request 60 s of a 200 s deadline: a request a run. The halved range's rows are kept by the run that ends it, the
    // ETH pair is read runs after the deposit that named it, from the list frozen when the walk entered its trade phase.
    let (_d2, many_db, many_key) = install();
    let many = venue(&account, Duration::from_secs(60));
    let runs = import(&many_db, many_key, &many, Duration::from_secs(200)).await;
    assert!(runs >= 20, "a request a run: {runs} runs");
    assert_eq!(stored(&many_db, many_key).await, (rows.clone(), watermark.clone()), "N runs store one run's rows and watermark");

    // A run cut mid-walk (the first run at its first request, then each run at its second) replays from the last checkpoint
    // and stores nothing twice.
    let (_d3, cut_db, cut_key) = install();
    let cut = venue(&account, Duration::from_secs(60));
    cut.cut_at.set(Some(1));
    assert!(run(&cut_db, cut_key, &cut, Duration::from_secs(200)).await.is_none(), "the first run is cut before it stores its first page");
    cut.cut_at.set(Some(3));
    import(&cut_db, cut_key, &cut, Duration::from_secs(200)).await;
    assert_eq!(stored(&cut_db, cut_key).await, (rows, watermark), "a cut run replays and stores every row once");
}
