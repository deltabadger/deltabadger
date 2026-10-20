//! A KuCoin ledger import over several runs (plan VB §1.6, slice A1): admission against the deadline, checkpoints,
//! replay after a cut, the stall and chain rules, and a new credential. Every multi-run import must store exactly the
//! rows, and end on exactly the watermark, of the same account imported in one run. Rails-free: one-run KuCoin imports
//! against Rails are sync_parity.rs's `kucoin-ledger-` scenarios.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::{Clock, FixedClock};
use deltabadger::jobs::Db;
use deltabadger::sync::continuation::{Run, Stopwatch, CHAIN_RUNS};
use deltabadger::sync::kucoin_ledger::{self, DEPOSITS_PATH, FILLS_PATH, WITHDRAWALS_PATH};
use deltabadger::sync::ledger::Outcome;
use deltabadger::sync::Failure;
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const NOW: &str = "2026-09-20T02:00:00Z";
/// The key's watermark: the import starts 25 hours before it, so the fills are five weekly windows.
const WATERMARK: &str = "2026-08-20T00:00:00Z";
const DAY_MS: i64 = 86_400_000;

fn clock() -> FixedClock { FixedClock(NOW.parse().unwrap()) }
fn creds() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
fn start_ms() -> i64 { WATERMARK.parse::<chrono::DateTime<chrono::Utc>>().unwrap().timestamp_millis() - 25 * 3_600_000 }

/// A KuCoin key with a watermark this credential produced.
fn install() -> (tempfile::TempDir, Db, i64) {
    let (dir, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute("UPDATE exchanges SET type = 'Exchanges::Kucoin', name = 'KuCoin' WHERE id = ?1", [s.exchange_id]).unwrap();
    c.execute("UPDATE api_keys SET last_synced_at = ?1 WHERE id = ?2", rusqlite::params![deltabadger::sync::sql_time(WATERMARK.parse().unwrap()), s.api_key_id]).unwrap();
    let producer = deltabadger::engine::model::credential_version_by_id(c, s.api_key_id).unwrap().unwrap();
    let tx = c.unchecked_transaction().unwrap();
    deltabadger::sync::cache::record_ledger(&deltabadger::engine::model::check_credential_result(&tx, &Some(producer.clone())).unwrap(), s.api_key_id, &producer, clock().now()).unwrap();
    tx.commit().unwrap();
    (dir, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}

/// The account: fills spread over the five windows (several pages each), deposits and withdrawals.
fn account() -> Account {
    let t0 = start_ms();
    let fill = |i: i64| json!({ "symbol": "BTC-USDT", "tradeId": format!("t{i}"), "orderId": format!("o{i}"), "side": if i % 3 == 0 { "sell" } else { "buy" },
                                "price": "100", "size": format!("0.{:02}", i % 90 + 1), "funds": format!("{}", i % 90 + 1), "fee": "0.01", "feeCurrency": "USDT",
                                "createdAt": t0 + i * DAY_MS / 3 + 1 });
    let transfer = |i: i64, kind: &str| json!({ "id": format!("{kind}{i}"), "currency": "USDT", "amount": format!("{}", 10 + i), "fee": "0", "createdAt": t0 + i * DAY_MS });
    Account { fills: (0..90).map(fill).collect(), deposits: (0..7).map(|i| transfer(i, "d")).collect(), withdrawals: (0..5).map(|i| transfer(i, "w")).collect() }
}

struct Account { fills: Vec<Value>, deposits: Vec<Value>, withdrawals: Vec<Value> }

type Delay = Box<dyn Fn(usize) -> Duration>;

/// The scripted run clock: requests and units advance it as the test says.
#[derive(Default)]
struct Watch { at: Cell<Duration>, units: Cell<usize>, unit_delay: RefCell<Option<Delay>>, cut_at_unit: Cell<Option<usize>>, cut: tokio::sync::Notify }
impl Stopwatch for Watch {
    fn elapsed(&self) -> Duration { self.at.get() }
    /// A cut as the unit starts lands at its first await: the unit (already on the blocking pool) commits, the run stops.
    fn unit_begun(&self) { if self.cut_at_unit.get() == Some(self.units.get() + 1) { self.cut.notify_one(); } }
    fn unit_done(&self) {
        let n = self.units.get() + 1;
        self.units.set(n);
        if let Some(delay) = self.unit_delay.borrow().as_ref() { self.at.set(self.at.get() + delay(n)); }
    }
}

/// KuCoin as the walk reads it: `per_page` items a page, each request `latency` long on the watch, the first `throttled`
/// requests answered 429000.
#[derive(Clone)]
struct Venue { account: Rc<Account>, per_page: usize, watch: Rc<Watch>, latency: Rc<dyn Fn(usize) -> Duration>, throttled: Rc<Cell<usize>>, log: Rc<RefCell<Vec<String>>>,
               /// Cut the run (as the runner's deadline would) once it has sent this many requests in all.
               cut_at_request: Rc<Cell<Option<usize>>> }

impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let n = self.log.borrow().len();
        self.watch.at.set(self.watch.at.get() + (self.latency)(n));
        if self.cut_at_request.get() == Some(n + 1) { self.watch.cut.notify_one(); }
        let q = |k: &str| r.query.iter().find(|(name, _)| *name == k).map(|(_, v)| v.parse::<i64>().unwrap());
        self.log.borrow_mut().push(format!("{} {:?} {:?} {:?}", r.path, q("startAt"), q("endAt"), q("currentPage")));
        if self.throttled.get() > 0 {
            self.throttled.set(self.throttled.get() - 1);
            return Ok(HttpResponse { status: 429, body: json!({ "code": "429000", "msg": "Too Many Requests" }).to_string() });
        }
        let at = |v: &Value| v["createdAt"].as_i64().unwrap();
        let rows: Vec<&Value> = match r.path.as_str() {
            FILLS_PATH => self.account.fills.iter().filter(|f| at(f) >= q("startAt").unwrap() && at(f) < q("endAt").unwrap()).collect(),
            DEPOSITS_PATH => self.account.deposits.iter().filter(|d| q("startAt").is_none_or(|s| at(d) >= s)).collect(),
            WITHDRAWALS_PATH => self.account.withdrawals.iter().filter(|d| q("startAt").is_none_or(|s| at(d) >= s)).collect(),
            other => panic!("unscripted {other}"),
        };
        let page = q("currentPage").unwrap() as usize;
        let items: Vec<&Value> = rows.iter().skip((page - 1) * self.per_page).take(self.per_page).copied().collect();
        let body = json!({ "code": "200000", "data": { "currentPage": page, "pageSize": 500, "totalNum": rows.len(), "totalPage": rows.len().div_ceil(self.per_page), "items": items } });
        Ok(HttpResponse { status: 200, body: body.to_string() })
    }
}

fn venue(account: &Rc<Account>, per_page: usize, watch: &Rc<Watch>, latency: impl Fn(usize) -> Duration + 'static) -> Venue {
    Venue { account: account.clone(), per_page, watch: watch.clone(), latency: Rc::new(latency), throttled: Rc::new(Cell::new(0)), log: Rc::default(), cut_at_request: Rc::default() }
}

type Ran = Result<Result<Outcome, Failure>, deltabadger::sync::SyncError>;

/// One run, on its own fresh watch (as each job run starts its own), unless the test cuts it after a number of units.
async fn run(db: &Db, v: &Venue, deadline: Duration, chain: usize) -> Option<Ran> {
    let watch = v.watch.clone();
    watch.at.set(Duration::ZERO);
    watch.units.set(0);
    let run = Run { deadline, chain_runs: chain, stopwatch: &*watch };
    let (key, credentials, clock) = (key_id(db).await, creds(), clock());
    let out = tokio::select! {
        biased;
        _ = watch.cut.notified() => None,
        out = kucoin_ledger::sync_within(db, v.clone(), key, &credentials, &clock, Duration::ZERO, &run) => Some(out),
    };
    // A cut run may leave a unit on the blocking pool: it finishes, as it would in production.
    db.settled().await;
    out
}

async fn key_id(db: &Db) -> i64 { db.run(|c, _| c.query_row("SELECT id FROM api_keys", [], |r| r.get(0)).map_err(|e| e.to_string())).await.unwrap() }

/// What an import leaves: every row as the tracker reads it, and the key's watermark and error.
async fn result(db: &Db) -> (Vec<String>, Option<String>, Option<String>, bool) {
    db.run(|c, _| {
        let mut s = c.prepare("SELECT entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, tx_id, transacted_at, raw_data \
                               FROM account_transactions ORDER BY transacted_at, tx_id").map_err(|e| e.to_string())?;
        let rows = s.query_map([], |r| Ok((0..10).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))).and_then(|r| r.collect::<Result<Vec<_>, _>>()).map_err(|e| e.to_string())?;
        let (w, e): (Option<String>, Option<String>) = c.query_row("SELECT last_synced_at, last_sync_error FROM api_keys", [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        let record: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE key LIKE 'rust_sync.venue_ledger:%')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        Ok((rows, w, e, record))
    }).await.unwrap()
}

/// 385.55 s of reserves (seven tries of 45 s, a 500-row page at the 2.11 s floor, the 60 s finish) leave room for `pages`
/// requests of 10 s each before the next is refused.
fn deadline_for(pages: u64) -> Duration { Duration::from_millis(385_550 + 10_000 * (pages - 1) + 5_000) }
const LONG: Duration = Duration::from_secs(1_000_000);

/// The one-run import every multi-run import must equal.
async fn one_run(per_page: usize) -> (Vec<String>, Option<String>, Option<String>, bool, usize) {
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, per_page, &watch, |_| Duration::from_secs(10));
    let out = run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap();
    assert!(out.complete && out.imported == 102, "{out:?}");
    let (rows, w, e, record) = result(&db).await;
    let requests = v.log.borrow().len();
    (rows, w, e, record, requests)
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_stopped_at_every_page_boundary_stores_what_one_run_stores() {
    let (rows, watermark, error, record, requests) = one_run(4).await;
    assert!(!record && error.is_none() && watermark.is_some());
    // Each run reads `pages` pages, so over the sizes every source boundary and every point inside a pager is a stop.
    for pages in [1u64, 2, 3, 5, 8] {
        let (_dir, db, _) = install();
        let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
        let v = venue(&account, 4, &watch, |_| Duration::from_secs(10));
        let mut runs = 0;
        loop {
            runs += 1;
            let out = run(&db, &v, deadline_for(pages), usize::MAX).await.unwrap().unwrap().unwrap();
            let (_, w, _, rec) = result(&db).await;
            if out.complete { break; }
            assert!(rec, "an unfinished import keeps its record ({pages} pages a run)");
            assert_eq!(w, Some(deltabadger::sync::sql_time(WATERMARK.parse().unwrap())), "the watermark waits for the end");
            assert!(runs < 100, "the import must end");
        }
        assert_eq!(v.log.borrow().len(), requests, "{pages} pages a run: every page read once, none twice");
        assert_eq!(runs as usize, requests.div_ceil(pages as usize), "{pages} pages a run");
        assert_eq!(result(&db).await, (rows.clone(), watermark.clone(), None, false), "{pages} pages a run: Rails' single-run rows and watermark");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_slow_answer_and_a_throttle_end_the_run_by_checkpoint() {
    let (rows, watermark, ..) = one_run(4).await;
    // Answers near the transport's whole budget: the first page is always read; the second would not fit.
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |_| Duration::from_secs(44));
    let out = run(&db, &v, Duration::from_secs(420), CHAIN_RUNS).await.unwrap().unwrap().unwrap();
    assert!(!out.complete);
    let sent = v.log.borrow().len();
    assert_eq!((sent, result(&db).await.3), (1, true), "one page, then the checkpoint");
    // A throttled page asked again (each try 44 s): the retries count against the run, and the run ends with its cursor.
    v.throttled.set(3);
    let before = v.log.borrow().len();
    let out = run(&db, &v, deadline_for(2), CHAIN_RUNS).await.unwrap().unwrap().unwrap();
    assert!(!out.complete);
    assert_eq!(v.log.borrow().len() - before, 4, "three throttled tries and the answer; the next page did not fit");
    while !run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete {}
    assert_eq!(result(&db).await, (rows, watermark, None, false));
}

#[tokio::test(flavor = "current_thread")]
async fn slow_and_growing_commits_stop_the_store_in_time_and_the_next_run_goes_on() {
    let (rows, watermark, ..) = one_run(40).await;
    for (name, delay) in [("slow", Box::new(|_: usize| Duration::from_secs(150)) as Delay),
                          ("growing", Box::new(|n: usize| Duration::from_secs(5 * n as u64 * n as u64)))] {
        let (_dir, db, _) = install();
        let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
        let v = venue(&account, 40, &watch, |_| Duration::from_secs(1));
        *watch.unit_delay.borrow_mut() = Some(delay);
        let first = run(&db, &v, Duration::from_secs(700), CHAIN_RUNS).await.unwrap().unwrap().unwrap();
        assert!(!first.complete, "{name}: the store stopped before the deadline");
        assert!(watch.at.get() < Duration::from_secs(700), "{name}: {:?} past the deadline", watch.at.get());
        let mut runs = 1;
        while !run(&db, &v, Duration::from_secs(700), CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete { runs += 1; assert!(runs < 200); }
        assert_eq!(result(&db).await, (rows.clone(), watermark.clone(), None, false), "{name}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_expensive_first_page_is_read_and_stored_and_the_second_is_refused() {
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |n| if n == 0 { Duration::from_secs(5_000) } else { Duration::from_secs(1) });
    let out = run(&db, &v, Duration::from_secs(3_600), CHAIN_RUNS).await.unwrap().unwrap().unwrap();
    assert!(!out.complete && out.imported == 4, "{out:?}");
    let sent = v.log.borrow().len();
    assert_eq!((sent, result(&db).await.3), (1, true), "admitted, checkpointed; the second refused");
}

/// A KuCoin account of deposits only, read whole in one run on a fresh install: the rows every multi-run import of it
/// must equal.
async fn one_run_of(account: &Rc<Account>, per_page: usize) -> Vec<String> {
    let (_dir, db, _) = install();
    let v = venue(account, per_page, &Rc::new(Watch::default()), |_| Duration::from_secs(1));
    assert!(run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    result(&db).await.0
}

/// `n` deposits with ids, and at each of `twins` an identical deposit without an id or a walletTxId.
fn deposits(n: i64, twins: &[usize]) -> Account {
    let t = start_ms() + 2 * DAY_MS;
    let mut deposits: Vec<Value> = (0..n).map(|i| json!({ "id": format!("d{i}"), "currency": "USDT", "amount": format!("{}", 1 + i), "fee": "0", "createdAt": t + i })).collect();
    for at in twins { deposits.insert(*at, json!({ "currency": "USDT", "amount": "777", "fee": "0", "createdAt": t + 500 })); }
    Account { fills: vec![], deposits, withdrawals: vec![] }
}

/// The runner cuts a run at an await while a page's write is on the blocking pool: the page lands whole with its
/// checkpoint (a page is one transaction), the run stops, and the next run goes on from there, storing every row once.
#[tokio::test(flavor = "current_thread")]
async fn a_cut_at_a_pages_write_leaves_it_whole_and_the_next_run_stores_nothing_twice() {
    let account = Rc::new(deposits(150, &[99, 100]));
    let rows = one_run_of(&account, 500).await;
    assert_eq!((rows.len(), rows.iter().filter(|r| r.contains("777")).count()), (152, 2), "every deposit once, the identical pair twice");
    let (_dir, db, _) = install();
    let watch = Rc::new(Watch::default());
    let v = venue(&account, 500, &watch, |_| Duration::from_secs(1));
    watch.cut_at_unit.set(Some(1));
    assert!(run(&db, &v, LONG, CHAIN_RUNS).await.is_none(), "cut at the page's write");
    let (stored, w, _, record) = result(&db).await;
    assert_eq!((stored.len(), w, record), (152, Some(deltabadger::sync::sql_time(WATERMARK.parse().unwrap())), true),
               "the page and its checkpoint landed; the watermark did not move");
    watch.cut_at_unit.set(None);
    assert!(run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    assert_eq!(result(&db).await.0, rows);
}

/// Review repro: an id-less deposit checkpointed on one page, an identical one on the next page written in one go with
/// the rest, and a cut at any write: 301 rows, never 302.
#[tokio::test(flavor = "current_thread")]
async fn identical_idless_rows_on_two_pages_survive_a_cut_at_any_write() {
    let account = Rc::new(deposits(299, &[50, 220]));
    let rows = one_run_of(&account, 200).await;
    assert_eq!(rows.len(), 301);
    for cut in 1..=4 {
        let (_dir, db, _) = install();
        let watch = Rc::new(Watch::default());
        let v = venue(&account, 200, &watch, |_| Duration::from_secs(1));
        watch.cut_at_unit.set(Some(cut));
        let _ = run(&db, &v, LONG, CHAIN_RUNS).await;
        watch.cut_at_unit.set(None);
        while !run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete {}
        assert_eq!(result(&db).await.0, rows, "cut at write {cut}");
    }
    // And over runs of one page each, no cut.
    let (_dir, db, _) = install();
    let v = venue(&account, 200, &Rc::new(Watch::default()), |_| Duration::from_secs(10));
    while !run(&db, &v, deadline_for(1), usize::MAX).await.unwrap().unwrap().unwrap().complete {}
    assert_eq!(result(&db).await.0, rows);
}

/// A trade imported from a file (no id) at `at`: 0.5 BTC bought.
async fn file_row(db: &Db, at: i64) {
    let at = deltabadger::sync::sql_time(chrono::DateTime::from_timestamp_millis(at).unwrap());
    db.run(move |c, _| c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, raw_data, \
                                  manual_values, transfer_link_rejected, created_at, updated_at) SELECT user_id, exchange_id, 0, 'BTC', 0.5, ?1, '{}', '{}', 0, ?1, ?1 \
                                  FROM api_keys", [at]).map(|_| ()).map_err(|e| e.to_string())).await.unwrap();
}

/// Review repro: a trade imported from a file before the import (no id) matches two fills with distinct ids read in
/// different runs. One store marks the first a duplicate of the file row and stores the second; so must two runs.
#[tokio::test(flavor = "current_thread")]
async fn a_file_row_matching_two_fills_in_different_runs_absorbs_only_the_first() {
    let at = start_ms() + DAY_MS;
    let fill = |id: &str| json!({ "symbol": "BTC-USDT", "tradeId": id, "orderId": "o1", "side": "buy", "price": "100", "size": "0.5", "funds": "50",
                                 "fee": "0.01", "feeCurrency": "USDT", "createdAt": at });
    let account = Rc::new(Account { fills: vec![fill("t1"), fill("t2")], deposits: vec![], withdrawals: vec![] });
    let (_dir, db, _) = install();
    file_row(&db, at).await;
    let v = venue(&account, 1, &Rc::new(Watch::default()), |_| Duration::from_secs(1));
    assert!(run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    let rows = result(&db).await.0;
    assert_eq!(rows.len(), 2, "the file row and the second fill");
    let (_dir, db, _) = install();
    file_row(&db, at).await;
    let v = venue(&account, 1, &Rc::new(Watch::default()), |_| Duration::from_secs(10));
    let mut runs = 0;
    while !run(&db, &v, deadline_for(1), usize::MAX).await.unwrap().unwrap().unwrap().complete { runs += 1; }
    assert!(runs >= 2, "the two fills were read in different runs");
    assert_eq!(result(&db).await.0, rows);
}

/// Review repro: a run that stores one page, slowly, keeps that page's measured cost for the next run's estimate.
#[tokio::test(flavor = "current_thread")]
async fn a_slow_single_write_is_kept_for_the_next_runs_estimate() {
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |_| Duration::from_secs(10));
    *watch.unit_delay.borrow_mut() = Some(Box::new(|_| Duration::from_secs(150)));
    assert!(!run(&db, &v, deadline_for(1), CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    let units: Value = db.run(|c, _| c.query_row("SELECT value FROM app_configs WHERE key LIKE 'rust_sync.venue_ledger:%'", [], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())).await.map(|t| serde_json::from_str::<Value>(&t).unwrap()["units"].clone()).unwrap();
    assert_eq!(units, json!([150_000]));
}

/// Runs cut before their first checkpoint make no progress: after STALLS of them the next run fails at once with a sync
/// error (and wakes nothing), and the run after that reads again from the cursor.
#[tokio::test(flavor = "current_thread")]
async fn runs_that_never_checkpoint_stop_the_chain_and_the_cursor_survives() {
    let (rows, watermark, ..) = one_run(4).await;
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |_| Duration::from_secs(10));
    assert!(!run(&db, &v, deadline_for(2), CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    // Two runs cut by the deadline before they store anything (the record counts them as they start).
    for _ in 0..2 {
        v.cut_at_request.set(Some(v.log.borrow().len() + 1));
        assert!(run(&db, &v, deadline_for(2), CHAIN_RUNS).await.is_none());
    }
    v.cut_at_request.set(None);
    let before = v.log.borrow().len();
    let Some(Ok(Err(failure))) = run(&db, &v, deadline_for(2), CHAIN_RUNS).await else { panic!("a stalled import fails") };
    assert!(failure.error.contains("completed no page") && !failure.raised, "{}", failure.error);
    assert_eq!(v.log.borrow().len(), before, "nothing read");
    let (_, w, e, record) = result(&db).await;
    assert!(record && e.is_some() && w == Some(deltabadger::sync::sql_time(WATERMARK.parse().unwrap())));
    while !run(&db, &v, deadline_for(2), CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete {}
    assert_eq!(result(&db).await, (rows, watermark, None, false), "resumed from the cursor, and the error cleared");
}

/// CHAIN_RUNS forced to 2: the second run of a chain records that the import is still running and wakes nothing; the
/// next scheduled sync resumes from the cursor; the import ends with the one-run rows.
#[tokio::test(flavor = "current_thread")]
async fn an_import_over_two_capped_chains_ends_with_the_one_run_rows() {
    let (rows, watermark, ..) = one_run(4).await;
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |_| Duration::from_secs(10));
    let mut capped = 0;
    loop {
        match run(&db, &v, deadline_for(3), 2).await.unwrap().unwrap() {
            Ok(out) if out.complete => break,
            Ok(_) => {}
            Err(failure) => { assert!(failure.error.contains("still running after"), "{}", failure.error); capped += 1; assert!(result(&db).await.3, "the record stays"); }
        }
        assert!(capped < 50);
    }
    assert!(capped >= 2, "the import spans more than two chains");
    let sent = v.log.borrow().len();
    assert_eq!(sent, one_run(4).await.4, "no page read twice");
    assert_eq!(result(&db).await, (rows, watermark, None, false));
}

/// A new credential mid-import discards the record: the next run walks from the start again (#511).
#[tokio::test(flavor = "current_thread")]
async fn a_credential_replacement_mid_import_starts_the_walk_again() {
    let (rows, ..) = one_run(4).await;
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |_| Duration::from_secs(10));
    assert!(!run(&db, &v, deadline_for(3), CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    let first = v.log.borrow()[0].clone();
    // The same key text, encrypted again: a new ciphertext, a new credential version.
    db.run(|c, cipher| c.execute("UPDATE api_keys SET secret = ?1", [cipher.encrypt("paper-secret")]).map(|_| ()).map_err(|e| e.to_string())).await.unwrap();
    v.log.borrow_mut().clear();
    assert!(run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    // The old watermark was another credential's: the new import reads the whole history, from KuCoin's first day.
    let from = kucoin_ledger::HISTORY_FROM_MS;
    assert_eq!(v.log.borrow()[0], format!("{FILLS_PATH} Some({from}) Some({}) Some(1)", from + 7 * DAY_MS), "not the old cursor ({first})");
    assert_eq!(result(&db).await.0, rows, "the rows of one import, none twice");
}

/// Review repro: a trade imported from a file between two runs (id > the record's floor, not this import's) matches a
/// fill the next run reads. One store over the file row marks the fill a duplicate; the run after the file import must
/// too: the rows this import stored are the ones it wrote, not every newer row.
#[tokio::test(flavor = "current_thread")]
async fn a_file_row_imported_between_runs_absorbs_the_fill_it_matches() {
    let t0 = start_ms() + DAY_MS;
    let later = t0 + 21 * DAY_MS;
    let fill = |id: &str, at: i64| json!({ "symbol": "BTC-USDT", "tradeId": id, "orderId": id, "side": "buy", "price": "100", "size": "0.5", "funds": "50",
                                           "fee": "0.01", "feeCurrency": "USDT", "createdAt": at });
    let account = Rc::new(Account { fills: vec![fill("t1", t0), fill("t2", later)], deposits: vec![], withdrawals: vec![] });
    let (_dir, db, _) = install();
    let v = venue(&account, 1, &Rc::new(Watch::default()), |_| Duration::from_secs(10));
    assert!(!run(&db, &v, deadline_for(1), usize::MAX).await.unwrap().unwrap().unwrap().complete);
    assert_eq!(result(&db).await.0.len(), 1, "the first fill stored");
    file_row(&db, later).await;
    while !run(&db, &v, deadline_for(1), usize::MAX).await.unwrap().unwrap().unwrap().complete {}
    assert_eq!(result(&db).await.0.len(), 2, "the first fill and the file row; the second fill is its duplicate");
}

/// Review repro: a new credential rereads history already stored. Every entry is known by its id, so no identity's
/// count decides anything and the record carries none.
#[tokio::test(flavor = "current_thread")]
async fn a_reread_of_known_history_carries_no_repeat_counts() {
    let (_dir, db, _) = install();
    let (account, watch) = (Rc::new(account()), Rc::new(Watch::default()));
    let v = venue(&account, 4, &watch, |_| Duration::from_secs(10));
    assert!(run(&db, &v, LONG, CHAIN_RUNS).await.unwrap().unwrap().unwrap().complete);
    db.run(|c, cipher| c.execute("UPDATE api_keys SET secret = ?1", [cipher.encrypt("paper-secret")]).map(|_| ()).map_err(|e| e.to_string())).await.unwrap();
    // The new import reads from KuCoin's first day: empty weeks first, then the known rows. Stop part-way through them.
    let mut checked = false;
    while !run(&db, &v, deadline_for(5), usize::MAX).await.unwrap().unwrap().unwrap().complete {
        let record: Value = db.run(|c, _| c.query_row("SELECT value FROM app_configs WHERE key LIKE 'rust_sync.venue_ledger:%'", [], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())).await.map(|t| serde_json::from_str::<Value>(&t).unwrap()).unwrap();
        assert_eq!(record["repeats"], json!([]), "no zero counts recorded");
        checked |= !record["max_seen"].is_null();
    }
    assert!(checked, "a run stopped after rereading known rows");
}
