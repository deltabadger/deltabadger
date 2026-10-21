//! A Bitget ledger import over several runs (plan VB §1.6 on slice A4's walk): fills paged by idLessThan, the deposit and
//! withdrawal records window by window from 2018-04-01 with a record on the edge two windows share, and two equal fills
//! without a tradeId on different pages (#get_ledger's `uniq`: one row, across runs too). Every multi-run import stores
//! exactly the rows, and ends on exactly the watermark, of one run; a run cut mid-walk replays. Rails-free: the one-run walk
//! against Rails is sync_parity.rs's `bitget-ledger-` scenarios.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::Db;
use deltabadger::sync::bitget_ledger;
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const NOW: &str = "2026-09-20T02:00:00Z";

fn creds() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
fn now_ms() -> i64 { NOW.parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_millis() }
fn from_2018() -> i64 { bitget_ledger::transfers_from().timestamp_millis() }

/// A Bitget key with no watermark: a whole-history import.
fn install() -> (tempfile::TempDir, Db, i64) {
    let (dir, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Bitget', name = 'Bitget' WHERE id = ?1", [s.exchange_id]).unwrap();
    (dir, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}

/// The account, each list newest first as Bitget pages it: 230 fills (the 51st and the 151st equal and without a tradeId);
/// 121 deposits in the first window plus one on the edge it shares with the second, and a pending one; three withdrawals,
/// one failed.
struct Account { fills: Vec<Value>, deposits: Vec<Value>, withdrawals: Vec<Value> }
fn account() -> Account {
    let fill = |i: i64| {
        let mut f = json!({ "symbol": "BTCUSDT", "orderId": format!("o{i}"), "tradeId": (10_000 - i).to_string(), "side": if i % 2 == 0 { "buy" } else { "sell" },
                            "priceAvg": format!("{}", 60_000 + i), "size": "0.001", "feeDetail": { "feeCoin": "USDT", "totalFee": "-0.06" },
                            "cTime": (now_ms() - 1000 - i * 60_000).to_string() });
        if i == 50 || i == 150 { f = json!({ "symbol": "BTCUSDT", "side": "buy", "priceAvg": "61000", "size": "0.002", "fee": "-0.1", "cTime": (now_ms() - 3_600_000).to_string() }); }
        f
    };
    let window = 89 * 86_400_000;
    let deposit = |id: String, at: i64, status: &str| json!({ "orderId": id, "tradeId": "0xabc", "coin": "USDT", "size": "10", "status": status, "cTime": at.to_string() });
    let mut deposits: Vec<Value> = vec![deposit("edge".into(), from_2018() + window, "success")];
    deposits.extend((0..121).map(|i| deposit(format!("{}", 5000 - i), from_2018() + window - 1000 - i * 1000, "success")));
    deposits.push(deposit("pending".into(), from_2018() + 10, "pending"));
    let withdrawals = vec![json!({ "orderId": "w3", "coin": "USDT", "size": "5", "fee": "-1", "status": "success", "cTime": (now_ms() - 10_000).to_string() }),
                           json!({ "orderId": "w2", "coin": "USDT", "size": "6", "fee": "-1", "status": "fail", "cTime": (now_ms() - 20_000).to_string() }),
                           json!({ "orderId": "w1", "coin": "BTC", "size": "0.1", "fee": "-0.0001", "status": "success", "cTime": (from_2018() + 5 * window).to_string() })];
    Account { fills: (0..230).map(fill).collect(), deposits, withdrawals }
}

#[derive(Default)]
struct Watch { at: Cell<Duration>, cut: tokio::sync::Notify }
impl Stopwatch for Watch { fn elapsed(&self) -> Duration { self.at.get() } }

/// Bitget as the walk reads it: every request `latency` long on the watch; each list's rows between startTime and endTime
/// (both included: neighbouring windows share their edge), newest first, 100 a page, after the row idLessThan names.
/// `cut_at`: the runner's deadline cuts the run at that request (counted from 1, in all). `stuck`: every fills page is the
/// first, whatever idLessThan asks.
#[derive(Clone)]
struct Venue { account: Rc<Account>, watch: Rc<Watch>, latency: Duration, log: Rc<RefCell<Vec<String>>>, cut_at: Rc<Cell<Option<usize>>>, stuck: Rc<Cell<bool>> }
impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let n = self.log.borrow().len() + 1;
        self.watch.at.set(self.watch.at.get() + self.latency);
        if self.cut_at.get() == Some(n) { self.watch.cut.notify_one(); }
        self.log.borrow_mut().push(deltabadger::venue::http::request_key(r));
        let q = |k: &str| r.query.iter().find(|(name, _)| *name == k).map(|(_, v)| v.clone());
        let (rows, id) = match r.path.as_str() {
            "/api/v2/spot/trade/fills" => (&self.account.fills, "tradeId"),
            "/api/v2/spot/wallet/deposit-records" => (&self.account.deposits, "orderId"),
            "/api/v2/spot/wallet/withdrawal-records" => (&self.account.withdrawals, "orderId"),
            other => panic!("unscripted {other}"),
        };
        let (start, end): (i64, i64) = (q("startTime").unwrap().parse().unwrap(), q("endTime").unwrap().parse().unwrap());
        let inside: Vec<&Value> = rows.iter().filter(|row| (start..=end).contains(&row["cTime"].as_str().unwrap().parse::<i64>().unwrap())).collect();
        let from = q("idLessThan").filter(|_| !self.stuck.get()).map_or(0, |before| inside.iter().position(|row| row[id] == before.as_str()).unwrap() + 1);
        let page: Vec<&&Value> = inside.iter().skip(from).take(100).collect();
        Ok(HttpResponse { status: 200, body: json!({ "code": "00000", "msg": "success", "data": page }).to_string() })
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
        out = bitget_ledger::sync_within(db, v.clone(), key, &credentials, &clock, &run) => Some(out),
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
    Venue { account: account.clone(), watch: Rc::default(), latency, log: Rc::default(), cut_at: Rc::default(), stuck: Rc::default() }
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_over_many_runs_stores_what_one_run_stores() {
    let account = Rc::new(account());
    let (_d1, one_db, one_key) = install();
    let one = venue(&account, Duration::from_millis(1));
    assert_eq!(import(&one_db, one_key, &one, Duration::from_secs(3600)).await, 1, "a short import is one run");
    let (rows, watermark) = stored(&one_db, one_key).await;
    // 229 fills (the equal pair is one), 122 deposits (the edge record once, the pending one not), 2 withdrawals.
    assert_eq!(rows.len(), 229 + 122 + 2);
    assert_eq!(rows.iter().filter(|r| r[7].is_null()).count(), 1, "the id-less pair is one row");
    let fills: Vec<String> = one.log.borrow().iter().filter(|k| k.contains("/trade/fills")).cloned().collect();
    assert_eq!(fills.len(), 3, "three pages of one window: {fills:?}");
    assert!(fills[1].contains("idLessThan=9901") && fills[2].contains("idLessThan=9801"), "{fills:?}");
    let first_window = one.log.borrow().iter().filter(|k| k.contains("deposit-records") && k.contains(&format!("startTime={}", from_2018()))).count();
    assert_eq!(first_window, 2, "the first window's 122 records are two pages");

    // Each request 60 s of a 200 s deadline: a page or two a run.
    let (_d2, many_db, many_key) = install();
    let many = venue(&account, Duration::from_secs(60));
    let runs = import(&many_db, many_key, &many, Duration::from_secs(200)).await;
    assert!(runs >= 20, "a page or two a run: {runs} runs");
    assert_eq!(stored(&many_db, many_key).await, (rows.clone(), watermark.clone()), "N runs store one run's rows and watermark");

    // A run cut mid-walk (the first run at its first request, then the second at its own) replays from the last checkpoint and
    // stores nothing twice.
    let (_d3, cut_db, cut_key) = install();
    let cut = venue(&account, Duration::from_secs(60));
    cut.cut_at.set(Some(1));
    assert!(run(&cut_db, cut_key, &cut, Duration::from_secs(200)).await.is_none(), "the first run is cut before it stores its first page");
    cut.cut_at.set(Some(3));
    import(&cut_db, cut_key, &cut, Duration::from_secs(200)).await;
    assert_eq!(stored(&cut_db, cut_key).await, (rows, watermark), "a cut run replays and stores every row once");
}

/// D-BITGET-STUCK: a full page that answers whatever idLessThan asks with the first page again. Rails asks for it forever;
/// the walk fails, in one run and when it is read a page a run, never an import that checkpoints the same cursor forever.
#[tokio::test(flavor = "current_thread")]
async fn a_full_page_repeated_fails_instead_of_looping() {
    let account = Rc::new(account());
    for latency in [Duration::from_millis(1), Duration::from_secs(60)] {
        let (_d, db, key) = install();
        let v = venue(&account, latency);
        v.stuck.set(true);
        let mut failed = None;
        for _ in 0..12 {
            match run(&db, key, &v, Duration::from_secs(200)).await.unwrap().unwrap() {
                Ok(out) => assert!(!out.complete, "the import never completes"),
                Err(f) => { failed = Some(f.error); break; }
            }
        }
        let error = failed.unwrap_or_else(|| panic!("no failure at {latency:?} a request"));
        assert!(error.contains("Bitget fills paging stuck at idLessThan 9901"), "{error}");
    }
}
