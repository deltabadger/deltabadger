//! Bingx N-run equivalence, overlapping halves, offset paging, durable restart and cancellation replay.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::jobs::Db;
use deltabadger::sync::bingx_ledger;
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const NOW: &str = "2026-09-20T02:00:00Z";
const DAY: i64 = 86_400_000;

fn now_ms() -> i64 {
    NOW.parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        .timestamp_millis()
}
fn creds() -> Credentials {
    Credentials {
        redaction_values: vec![],
        key: "PKTEST".into(),
        secret: "paper-secret".into(),
        passphrase: Some("paper".into()),
    }
}

/// A Bingx key with no watermark: a whole-history import.
fn install() -> (tempfile::TempDir, Db, i64) {
    let (dir, o, s) = common::install_alpaca();
    o.primary
        .execute(
            "UPDATE exchanges SET type = 'Exchanges::Bingx', name = 'Bingx' WHERE id = ?1",
            [s.exchange_id],
        )
        .unwrap();
    (
        dir,
        Db::new(o.primary, common::seed::cipher()),
        s.api_key_id,
    )
}

/// More than one page of transfers and trades, two held coins, upper/lower-case currency names.
struct Account {
    deposits: Vec<Value>,
    withdrawals: Vec<Value>,
    trades: Vec<Value>,
}
fn account() -> Account {
    let n = now_ms();
    let deposit = |coin: &str, amount: String, at: i64, status: i64| json!({ "amount": amount, "coin": coin, "status": status, "txId": format!("0x{coin}{at}:0"), "insertTime": at });
    let mut deposits: Vec<Value> = (0..1200)
        .map(|i| {
            deposit(
                "USDT",
                format!("1.{i:04}"),
                n - 40 * DAY + 3_600_000 + i * 60_000,
                1,
            )
        })
        .collect();
    deposits.push(deposit("BTC", "0.5".into(), n - 20 * DAY, 1));

    let withdrawals = vec![
        json!({ "id": "w1", "coin": "USDTTRC20", "network": "TRC20", "amount": "3", "transactionFee": "1", "status": 6, "applyTime": n - 5 * DAY }),
        json!({ "id": "w2", "coin": "USDTTRC20", "network": "TRC20", "amount": "4", "transactionFee": "1", "status": 6, "applyTime": n - 4 * DAY }),
    ];
    let trade = |symbol: &str, i: i64| {
        json!({ "symbol": symbol, "id": format!("{symbol}-{i}"), "orderId": format!("{i}"), "quoteQty": "60", "price": format!("{}", 60_000 + i), "qty": "0.001",
                                                "commission": "-0.01", "commissionAsset": "USDT", "time": n - 5 * DAY + i * 60_000, "isBuyer": i % 2 == 0 })
    };
    let mut trades: Vec<Value> = (0..1250).map(|i| trade("BTC-USDT", i)).collect();
    trades.extend((1000..1003).map(|i| trade("ETH-USDT", i)));
    Account {
        deposits,
        withdrawals,
        trades,
    }
}

#[derive(Default)]
struct Watch {
    at: Cell<Duration>,
    cut: tokio::sync::Notify,
}
impl Stopwatch for Watch {
    fn elapsed(&self) -> Duration {
        self.at.get()
    }
}

/// Bingx as the walk reads it: every request `latency` long on the watch; each history the rows within [startTime, endTime],
/// at most `limit`, with offsets for transfers. `cut_at`: the runner's deadline cuts the run at that request (counted from 1).
#[derive(Clone)]
struct Venue {
    account: Rc<Account>,
    watch: Rc<Watch>,
    latency: Duration,
    log: Rc<RefCell<Vec<String>>>,
    cut_at: Rc<Cell<Option<usize>>>,
    listing: Option<Value>,
    /// The venue's and the sync's clock (epoch ms), moved on by `tick` at every request.
    now: Rc<Cell<i64>>,
    tick: i64,
}
struct Moving(Rc<Cell<i64>>);
impl deltabadger::engine::Clock for Moving {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(self.0.get()).unwrap()
    }
}
impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let n = self.log.borrow().len() + 1;
        self.watch.at.set(self.watch.at.get() + self.latency);
        let sent = self.now.get(); // the request's own latency passes after BingX reads its range
        self.now.set(sent + self.tick);
        if self.cut_at.get() == Some(n) {
            self.watch.cut.notify_one();
        }
        self.log
            .borrow_mut()
            .push(deltabadger::venue::http::request_key(r));
        let q = |k: &str| {
            r.query
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.clone())
        };
        let within = |rows: &[Value], time: &str| -> Value {
            let (from, to, limit): (i64, i64, usize) = (
                q("startTime").unwrap().parse().unwrap(),
                q("endTime").unwrap().parse().unwrap(),
                q("limit").unwrap().parse().unwrap(),
            );
            json!(rows
                .iter()
                .filter(|d| (from..=to).contains(&d[time].as_i64().unwrap()))
                .filter(|d| q("symbol").is_none_or(|s| d["symbol"] == s.as_str()))
                .filter(|d| q("coin").is_none_or(|c| d["symbol"]
                    .as_str()
                    .unwrap()
                    .split('_')
                    .next()
                    .unwrap()
                    .eq_ignore_ascii_case(&c)))
                .skip(q("offset").map_or(0, |s| s.parse().unwrap()))
                .take(limit)
                .collect::<Vec<_>>())
        };
        let body = match r.path.as_str() {
            "/openApi/api/v3/capital/deposit/hisrec" => within(&self.account.deposits, "insertTime"),
            "/openApi/api/v3/capital/withdraw/history" => within(&self.account.withdrawals, "applyTime"),
            "/openApi/spot/v1/account/balance" => json!({ "code":0,"data":{"balances": [{ "asset": "ETH", "free": "1", "locked": "0" }] }}),
            "/openApi/spot/v1/common/symbols" => self.listing.clone().unwrap_or_else(|| json!({"code":0,"data": { "symbols": [{ "symbol": "BTC-USDT", "baseAsset": "btc", "quoteAsset": "usdt" }, { "symbol": "ETH-USDT", "baseAsset": "ETH", "quoteAsset": "usdt" },
                                                          { "symbol": "XRP-USDT", "baseAsset": "XRP", "quoteAsset": "usdt" }] }})),
            // BingX answers a startTime past its seven days with the last 24 hours instead: a sync must never ask for one.
            "/openApi/spot/v1/trade/myTrades" if q("startTime").unwrap().parse::<i64>().unwrap() < sent - 7 * DAY => {
                panic!("myTrades asked from {} at {sent}, beyond its seven days", q("startTime").unwrap())
            }
            "/openApi/spot/v1/trade/myTrades" => json!({"code":0,"data":{"fills":within(&self.account.trades, "time")}}),
            other => panic!("unscripted {other}"),
        };
        Ok(HttpResponse {
            status: 200,
            body: body.to_string(),
        })
    }
}

type Ran = Result<
    Result<deltabadger::sync::ledger::Outcome, deltabadger::sync::Failure>,
    deltabadger::sync::SyncError,
>;

async fn run(db: &Db, key: i64, v: &Venue, deadline: Duration) -> Option<Ran> {
    v.watch.at.set(Duration::ZERO);
    let run = Run {
        deadline,
        chain_runs: 1000,
        stopwatch: &*v.watch,
    };
    let (credentials, clock) = (creds(), Moving(v.now.clone()));
    let out = tokio::select! {
        biased;
        _ = v.watch.cut.notified() => None,
        out = bingx_ledger::sync_within(db, v.clone(), key, &credentials, &clock, &run) => Some(out),
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    out
}

/// Runs until the import completes; how many runs it took.
async fn import(db: &Db, key: i64, v: &Venue, deadline: Duration) -> usize {
    for runs in 1..=1000 {
        if let Some(out) = run(db, key, v, deadline).await {
            if out.unwrap().unwrap().complete {
                return runs;
            }
            let watermark = db
                .run(move |c, _| {
                    c.query_row(
                        "SELECT last_synced_at FROM api_keys WHERE id=?1",
                        [key],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .map_err(|e| e.to_string())
                })
                .await
                .unwrap();
            assert!(
                watermark.is_none(),
                "a checkpoint never advances the watermark"
            );
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
    Venue {
        account: account.clone(),
        watch: Rc::default(),
        latency,
        log: Rc::default(),
        cut_at: Rc::default(),
        listing: None,
        now: Rc::new(Cell::new(now_ms())),
        tick: 0,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_over_many_runs_stores_what_one_run_stores() {
    let account = Rc::new(account());
    let (_d1, one_db, one_key) = install();
    let one = venue(&account, Duration::from_millis(1));
    assert_eq!(
        import(&one_db, one_key, &one, Duration::from_secs(3600)).await,
        1,
        "a short import is one run"
    );
    let (rows, watermark) = stored(&one_db, one_key).await;
    // 1201 deposits, two withdrawals, 1250 BTC and three ETH trades.
    assert_eq!(rows.len(), 1201 + 2 + 1253);
    assert!(
        rows.iter().all(|r| r[6] != json!(-0.01)),
        "a commission is stored as its absolute value"
    );
    let log = one.log.borrow().clone();
    assert!(
        log.iter()
            .filter(|k| k.starts_with("GET /openApi/spot/v1/trade/myTrades"))
            .count()
            > 6,
        "two complete histories with a halved window"
    );
    assert!(
        log.iter().any(|k| k.contains("offset=1000")),
        "transfer paging reached the second page"
    );
    // Slow requests force checkpoints in the windows, halves and offsets. The ETH pair is read later
    // from the list frozen at discovery; discovery is never repeated on resumption.
    let (_d2, many_db, many_key) = install();
    let many = venue(&account, Duration::from_secs(60));
    let runs = import(&many_db, many_key, &many, Duration::from_secs(150)).await;
    assert!(runs >= 20, "slow requests force many runs: {runs} runs");
    assert_eq!(
        many.log
            .borrow()
            .iter()
            .filter(|k| k.starts_with("GET /openApi/spot/v1/common/symbols"))
            .count(),
        1,
        "pair discovery is frozen across runs"
    );
    assert_eq!(
        stored(&many_db, many_key).await,
        (rows.clone(), watermark.clone()),
        "N runs store one run's rows and watermark"
    );

    // A run cut mid-walk (the first run at its first request, then each run at its second) replays from the last checkpoint
    // and stores nothing twice.
    let (_d3, cut_db, cut_key) = install();
    let cut = venue(&account, Duration::from_secs(60));
    cut.cut_at.set(Some(1));
    assert!(
        run(&cut_db, cut_key, &cut, Duration::from_secs(150))
            .await
            .is_none(),
        "the first run is cut before it stores its first page"
    );
    cut.cut_at.set(Some(3));
    import(&cut_db, cut_key, &cut, Duration::from_secs(150)).await;
    assert_eq!(
        stored(&cut_db, cut_key).await,
        (rows, watermark),
        "a cut run replays and stores every row once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn transfer_discovery_survives_a_restart_before_the_trade_phase() {
    let account = Rc::new(account());
    let (dir, db, key) = install();
    let v = venue(&account, Duration::from_secs(60));
    // Each run admits just one request. The BTC balance is zero; only a completed deposit discovers it.
    for _ in 0..100 {
        let out = run(&db, key, &v, Duration::from_secs(150))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!out.complete);
        if v.log
            .borrow()
            .iter()
            .any(|r| r.starts_with("GET /openApi/spot/v1/account/balance"))
        {
            break;
        }
    }
    let before = stored(&db, key).await;
    assert!(before.1.is_none());
    assert!(before.0.iter().any(|r| r[1] == "BTC"));
    drop(db);
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let db = Db::new(c, common::seed::cipher());
    import(&db, key, &v, Duration::from_secs(150)).await;
    assert!(v
        .log
        .borrow()
        .iter()
        .any(|r| r.starts_with("GET /openApi/spot/v1/trade/myTrades")
            && r.contains("symbol=BTC-USDT")));
    assert_eq!(stored(&db, key).await.0.len(), 1201 + 2 + 1253);
}

#[tokio::test(flavor = "current_thread")]
async fn the_trade_reach_is_taken_when_the_trades_are_read() {
    let account = Rc::new(account());
    // A whole-history import whose transfer walk takes days of clock time: the trades are read from seven days before
    // the moment they are read, not before the import began.
    let (_d1, db, key) = install();
    let mut slow = venue(&account, Duration::from_millis(1));
    slow.tick = 2 * 3_600_000;
    import(&db, key, &slow, Duration::from_secs(3600)).await;
    assert!(slow.now.get() - now_ms() > 5 * DAY, "the transfer walk took days");
    assert!(slow.log.borrow().iter().any(|r| r.starts_with("GET /openApi/spot/v1/trade/myTrades")));

    // A continuation cut after the first trade page split its range, resumed two days later: the halves it kept are
    // clamped to the venue's seven days before they are sent.
    let (_d2, db, key) = install();
    let resumed = venue(&account, Duration::from_secs(60));
    for _ in 0..1000 {
        run(&db, key, &resumed, Duration::from_secs(150)).await.unwrap().unwrap().unwrap();
        if resumed.log.borrow().iter().any(|r| r.starts_with("GET /openApi/spot/v1/trade/myTrades")) { break; }
    }
    let before = resumed.log.borrow().len();
    resumed.now.set(resumed.now.get() + 2 * DAY);
    import(&db, key, &resumed, Duration::from_secs(150)).await;
    assert!(resumed.log.borrow()[before..].iter().any(|r| r.starts_with("GET /openApi/spot/v1/trade/myTrades")), "the kept halves are read");
}
