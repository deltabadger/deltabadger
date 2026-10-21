//! A Bitvavo ledger import over several runs (plan VB §1.6 on slice A3's walk): the transfer pages counted by content across
//! runs (a boundary row read again by the next run is not stored twice, an equal pair stays two), a currency seen only on a
//! transfer page in one run whose market's trades are read in a later one, the frozen market list, and a run cut mid-page
//! replayed. Every multi-run import stores exactly the rows, and ends on exactly the watermark, of one run. Rails-free: the
//! one-run walk against Rails is sync_parity.rs's `bitvavo-ledger-` scenarios.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::Db;
use deltabadger::sync::bitvavo_ledger;
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const NOW: &str = "2026-09-20T02:00:00Z";
const BASE: i64 = 1_780_000_000_000;

fn creds() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }

/// A Bitvavo key with no watermark: a whole-history import.
fn install() -> (tempfile::TempDir, Db, i64) {
    let (dir, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Bitvavo', name = 'Bitvavo' WHERE id = ?1", [s.exchange_id]).unwrap();
    (dir, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}

/// The account: 1003 deposits (two pages, the second re-reading the first's oldest millisecond, which holds an equal pair;
/// ETH only on the second page), two withdrawals, BTC held, and 1005 BTC-EUR and 2 ETH-EUR trades.
struct Account { deposits: Vec<Value>, withdrawals: Vec<Value>, trades: Vec<(String, Vec<Value>)> }
fn account() -> Account {
    let deposit = |symbol: &str, amount: String, at: i64| json!({ "timestamp": at, "symbol": symbol, "amount": amount, "fee": "0", "status": "completed" });
    let mut deposits: Vec<Value> = (0..998).map(|i| deposit("EUR", format!("1.{i:03}"), BASE + (1000 - i) * 60_000)).collect();
    deposits.extend([deposit("EUR", "5".into(), BASE + 2 * 60_000), deposit("EUR", "5".into(), BASE + 2 * 60_000), deposit("ETH", "0.5".into(), BASE + 60_000)]);
    let withdrawals = vec![json!({ "timestamp": BASE + 5_000, "symbol": "EUR", "amount": "3", "fee": "0.1", "status": "completed" }),
                           json!({ "timestamp": BASE + 6_000, "symbol": "EUR", "amount": "4", "fee": "0.1", "status": "awaiting_processing" })];
    let trade = |market: &str, i: i64| json!({ "id": format!("{market}-t{i}"), "orderId": "o", "timestamp": BASE + 100_000_000 - i * 1000, "market": market,
                                              "side": if i % 2 == 0 { "buy" } else { "sell" }, "amount": "0.001", "price": format!("{}", 60_000 + i), "fee": "0.01", "feeCurrency": "EUR" });
    Account { deposits, withdrawals, trades: vec![("BTC-EUR".into(), (0..1005).map(|i| trade("BTC-EUR", i)).collect()), ("ETH-EUR".into(), (0..2).map(|i| trade("ETH-EUR", i)).collect())] }
}

#[derive(Default)]
struct Watch { at: Cell<Duration>, cut: tokio::sync::Notify }
impl Stopwatch for Watch { fn elapsed(&self) -> Duration { self.at.get() } }

/// Bitvavo as the walk reads it: every request `latency` long on the watch; the histories newest first before `end`, 1000 a
/// page; trades before `tradeIdTo`. `cut_at`: the runner's deadline cuts the run at that request (counted from 1, in all).
#[derive(Clone)]
struct Venue { account: Rc<Account>, watch: Rc<Watch>, latency: Duration, log: Rc<RefCell<Vec<String>>>, cut_at: Rc<Cell<Option<usize>>> }
impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let n = self.log.borrow().len() + 1;
        self.watch.at.set(self.watch.at.get() + self.latency);
        if self.cut_at.get() == Some(n) { self.watch.cut.notify_one(); }
        self.log.borrow_mut().push(deltabadger::venue::http::request_key(r));
        let q = |k: &str| r.query.iter().find(|(name, _)| *name == k).map(|(_, v)| v.clone());
        let page = |rows: &[Value]| {
            let end: i64 = q("end").unwrap().parse().unwrap();
            let mut rows: Vec<&Value> = rows.iter().filter(|d| d["timestamp"].as_i64().unwrap() < end).collect();
            rows.sort_by_key(|d| std::cmp::Reverse(d["timestamp"].as_i64().unwrap()));
            json!(rows.into_iter().take(1000).collect::<Vec<_>>())
        };
        let body = match r.path.as_str() {
            "/v2/depositHistory" => page(&self.account.deposits),
            "/v2/withdrawalHistory" => page(&self.account.withdrawals),
            "/v2/balance" => json!([{ "symbol": "BTC", "available": "1", "inOrder": "0" }]),
            "/v2/markets" => json!([{ "market": "BTC-EUR" }, { "market": "ETH-EUR" }, { "market": "ADA-EUR" }]),
            "/v2/trades" => {
                let all = &self.account.trades.iter().find(|(m, _)| Some(m.clone()) == q("market")).unwrap().1;
                let from = q("tradeIdTo").map_or(0, |id| all.iter().position(|t| t["id"] == id.as_str()).unwrap() + 1);
                json!(all.iter().skip(from).take(1000).collect::<Vec<_>>())
            }
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
        out = bitvavo_ledger::sync_within(db, v.clone(), key, &credentials, &clock, Duration::ZERO, &run) => Some(out),
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    out
}

/// Runs until the import completes; how many runs it took.
async fn import(db: &Db, key: i64, v: &Venue, deadline: Duration) -> usize {
    for runs in 1..=200 {
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
    // 998 + the pair + ETH, one completed withdrawal, 1005 + 2 trades; the boundary pair is two rows, never four.
    assert_eq!(rows.len(), 1001 + 1 + 1007);
    assert_eq!(rows.iter().filter(|r| r[2] == json!(5.0)).count(), 2);
    let markets: Vec<String> = one.log.borrow().iter().filter(|k| k.starts_with("GET /v2/trades")).map(|k| k.split("market=").nth(1).unwrap().split('&').next().unwrap().to_string()).collect();
    assert_eq!(markets, ["BTC-EUR", "BTC-EUR", "ETH-EUR"], "BTC held, ETH only deposited (on the second page), ADA neither");

    // Each request 60 s of a 200 s deadline: a page a run. ETH is seen in the run that reads the second deposit page; its
    // trades are read runs later, from the list frozen when the walk entered its trade phase.
    let (_d2, many_db, many_key) = install();
    let many = venue(&account, Duration::from_secs(60));
    let runs = import(&many_db, many_key, &many, Duration::from_secs(200)).await;
    assert!(runs >= 7, "a page a run: {runs} runs");
    assert_eq!(stored(&many_db, many_key).await, (rows.clone(), watermark.clone()), "N runs store one run's rows and watermark");
    let second_page = many.log.borrow().iter().position(|k| k.starts_with("GET /v2/depositHistory") && !k.contains(&format!("end={}", NOW.parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_millis() + 1))).unwrap();
    let eth = many.log.borrow().iter().position(|k| k.contains("market=ETH-EUR")).unwrap();
    assert!(second_page + 3 < eth, "ETH's trades are read runs after the page that named it");

    // A run cut mid-walk (the first run at its first request, then the second run at its own) replays from the
    // last checkpoint and stores nothing twice.
    let (_d3, cut_db, cut_key) = install();
    let cut = venue(&account, Duration::from_secs(60));
    cut.cut_at.set(Some(1));
    assert!(run(&cut_db, cut_key, &cut, Duration::from_secs(200)).await.is_none(), "the first run is cut before it stores its first page");
    cut.cut_at.set(Some(3));
    import(&cut_db, cut_key, &cut, Duration::from_secs(200)).await;
    assert_eq!(stored(&cut_db, cut_key).await, (rows, watermark), "a cut run replays and stores every row once");
}
