//! A Hyperliquid ledger import over several runs (plan VB §1.6 on slice B1's walk): userFillsByTime and then
//! userNonFundingLedgerUpdates, each page from the newest `time` of the one before (its millisecond read again), identical
//! rows counted by the page that holds them most: pairs of equal zero-hash staking rows, one pair on a page's last
//! millisecond. Every multi-run import stores exactly the rows, and ends on exactly the watermark, of one run; a run cut
//! mid-walk replays. Rails-free: the one-run walk against Rails is sync_parity.rs's `hyperliquid-ledger-` scenarios.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::Db;
use deltabadger::sync::hyperliquid_ledger;
use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const NOW: &str = "2026-09-20T02:00:00Z";
const BASE: i64 = 1_788_000_000_000;

fn creds() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }

/// A Hyperliquid key with no watermark: a whole-history import.
fn install() -> (tempfile::TempDir, Db, i64) {
    let (dir, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Hyperliquid', name = 'Hyperliquid' WHERE id = ?1", [s.exchange_id]).unwrap();
    (dir, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}

/// The account, oldest first: 4500 spot fills, three to a millisecond; 1200 updates, two to a millisecond, where every
/// hundredth pair (the 99th and 100th, one of them the first page's last millisecond) is two equal zero-hash staking rows.
struct Account { fills: Vec<Value>, updates: Vec<Value> }
fn account() -> Account {
    let fills = (0..4500).map(|i: i64| json!({ "coin": "@142", "px": format!("{}", 60_000 + i), "sz": "0.001", "side": if i % 2 == 0 { "B" } else { "A" },
                                                "time": BASE + i / 3, "hash": format!("0x{i:x}"), "oid": 7000 + i, "fee": "0.01", "tid": 1_000_000 + i, "feeToken": "USDC" })).collect();
    let updates = (0..1200).map(|i: i64| {
        let at = BASE + 10_000 + i / 2;
        if i % 100 >= 98 { json!({ "time": at, "hash": format!("0x{}", "0".repeat(64)), "delta": { "type": "cStakingTransfer", "token": "HYPE", "amount": "1.0", "isDeposit": false } }) }
        else { json!({ "time": at, "hash": format!("0xu{i}"), "delta": { "type": "accountClassTransfer", "usdc": format!("{}.5", i), "toPerp": false } }) }
    }).collect();
    Account { fills, updates }
}

#[derive(Default)]
struct Watch { at: Cell<Duration>, cut: tokio::sync::Notify }
impl Stopwatch for Watch { fn elapsed(&self) -> Duration { self.at.get() } }

/// Hyperliquid as the walk reads it: every request `latency` long on the watch; each read's rows from startTime on (included),
/// oldest first, at most its limit. `cut_at`: the runner's deadline cuts the run at that request (counted from 1, in all).
#[derive(Clone)]
struct Venue { account: Rc<Account>, watch: Rc<Watch>, latency: Duration, log: Rc<RefCell<Vec<(String, i64)>>>, cut_at: Rc<Cell<Option<usize>>> }
impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let n = self.log.borrow().len() + 1;
        self.watch.at.set(self.watch.at.get() + self.latency);
        if self.cut_at.get() == Some(n) { self.watch.cut.notify_one(); }
        let body = r.body.clone().unwrap();
        let (kind, start) = (body["type"].as_str().unwrap().to_string(), body["startTime"].as_i64().unwrap());
        assert_eq!(body["user"], "PKTEST", "the key is the account's address");
        self.log.borrow_mut().push((kind.clone(), start));
        let (rows, limit) = match kind.as_str() { "userFillsByTime" => (&self.account.fills, 2000), "userNonFundingLedgerUpdates" => (&self.account.updates, 500), k => panic!("{k}") };
        let page: Vec<&Value> = rows.iter().filter(|row| row["time"].as_i64().unwrap() >= start).take(limit).collect();
        Ok(HttpResponse { status: 200, body: json!(page).to_string() })
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
        out = hyperliquid_ledger::sync_within(db, v.clone(), key, &credentials, &clock, &run) => Some(out),
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
    // Every fill, and every update: the equal staking pairs twice each, never a third time at a page boundary.
    assert_eq!(rows.len(), 4500 + 1200);
    assert_eq!(rows.iter().filter(|r| r[7].is_null()).count(), 24, "twelve equal zero-hash pairs, both of each kept");
    let starts: Vec<(String, i64)> = one.log.borrow().clone();
    // Fills: 2000 a page from 0, then from the newest millisecond of each full page (its rows read again).
    assert_eq!(starts.iter().filter(|(k, _)| k == "userFillsByTime").map(|(_, s)| *s).collect::<Vec<_>>(), [0, BASE + 666, BASE + 1332]);
    assert_eq!(starts.iter().filter(|(k, _)| k == "userNonFundingLedgerUpdates").map(|(_, s)| *s).collect::<Vec<_>>(),
               [0, BASE + 10_249, BASE + 10_498], "the first page ends on a staking pair's millisecond");

    // Each request 60 s of a 200 s deadline: a page or two a run.
    let (_d2, many_db, many_key) = install();
    let many = venue(&account, Duration::from_secs(60));
    let runs = import(&many_db, many_key, &many, Duration::from_secs(200)).await;
    assert!(runs >= 4, "a page or two a run: {runs} runs");
    assert_eq!(stored(&many_db, many_key).await, (rows.clone(), watermark.clone()), "N runs store one run's rows and watermark");

    // A run cut mid-walk (the first run at its first request, then the next at its own) replays from the last checkpoint and
    // stores nothing twice: the counts at the boundary millisecond travel in the checkpoint.
    let (_d3, cut_db, cut_key) = install();
    let cut = venue(&account, Duration::from_secs(60));
    cut.cut_at.set(Some(1));
    assert!(run(&cut_db, cut_key, &cut, Duration::from_secs(200)).await.is_none(), "the first run is cut before it stores its first page");
    cut.cut_at.set(Some(3));
    import(&cut_db, cut_key, &cut, Duration::from_secs(200)).await;
    assert_eq!(stored(&cut_db, cut_key).await, (rows, watermark), "a cut run replays and stores every row once");
}

/// A full page whose rows all share one millisecond: the next page is the same page, which brings nothing new. Rails fails
/// ("returned a full page with nothing new"); so does the walk, in one run and when it is read a page a run.
#[tokio::test(flavor = "current_thread")]
async fn a_full_page_with_nothing_new_fails_instead_of_looping() {
    let stuck = Account { fills: (0..2000).map(|i| json!({ "coin": "@142", "px": "1", "sz": "1", "side": "B", "time": BASE, "tid": i, "fee": "0" })).collect(), updates: vec![] };
    let account = Rc::new(stuck);
    for latency in [Duration::from_millis(1), Duration::from_secs(60)] {
        let (_d, db, key) = install();
        let v = venue(&account, latency);
        let mut failed = None;
        for _ in 0..12 {
            match run(&db, key, &v, Duration::from_secs(100)).await.unwrap().unwrap() {
                Ok(out) => assert!(!out.complete, "the import never completes"),
                Err(f) => { failed = Some(f.error); break; }
            }
        }
        let error = failed.unwrap_or_else(|| panic!("no failure at {latency:?} a request"));
        assert!(error.contains("Hyperliquid user_fills_by_time returned a full page with nothing new"), "{error}");
    }
}
