//! The in-process scheduler (rust/src/jobs): durable state, schedules, the runner.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed;
use deltabadger::app_config;
use deltabadger::jobs::state::{self, JobState};
use deltabadger::store::{self, Paths};
use rusqlite::Connection;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn db() -> (tempfile::TempDir, Connection) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o.primary)
}

#[test]
fn job_state_is_one_plain_app_configs_row_per_job_that_reads_without_the_keys() {
    let (_d, c) = db();
    let t = at("2026-10-02T10:15:03Z");
    assert_eq!(state::read(&c, "sync_x", None).unwrap(), JobState::default(), "a job that never ran here");
    state::record_error(&c, "sync_x", None, t, &"e".repeat(300)).unwrap();
    let s = state::read(&c, "sync_x", None).unwrap();
    assert!(s.failing());
    assert_eq!(s.last_error.as_deref().map(str::len), Some(state::ERROR_LIMIT));
    state::record_success(&c, "sync_x", None, t + Duration::minutes(1)).unwrap();
    let s = state::read(&c, "sync_x", None).unwrap();
    assert!(!s.failing(), "a success after the error");
    assert!(s.last_error.is_some(), "the last error stays visible");
    assert!(s.describe().starts_with("last success 2026-10-02T10:16:03Z"), "{}", s.describe());
    let raw: String = c.query_row("SELECT value FROM app_configs WHERE key = 'rust_job.sync_x'", [], |r| r.get(0)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).expect("plain JSON, no envelope");
    assert_eq!(v["last_success_at"], "2026-10-02T10:16:03.000Z");
    assert_eq!(state::all(&c).unwrap().into_iter().map(|(n, _)| n).collect::<Vec<_>>(), vec!["sync_x".to_string()]);
}

#[test]
fn app_config_set_is_a_no_op_for_an_unchanged_value_and_encrypts_a_changed_one() {
    let (_d, c) = db();
    let cipher = seed::cipher();
    let updated = |c: &Connection| -> String { c.query_row("SELECT updated_at FROM app_configs WHERE key = 'k'", [], |r| r.get(0)).unwrap() };
    app_config::set(&c, &cipher, "k", "31", at("2026-10-02T10:15:00Z")).unwrap();
    app_config::set(&c, &cipher, "k", "31", at("2026-10-03T10:15:00Z")).unwrap();
    assert_eq!(updated(&c), "2026-10-02 10:15:00", "AppConfig.set of the same value saves nothing");
    app_config::set(&c, &cipher, "k", "32", at("2026-10-03T10:15:00Z")).unwrap();
    assert_eq!(updated(&c), "2026-10-03 10:15:00");
    let raw: String = c.query_row("SELECT value FROM app_configs WHERE key = 'k'", [], |r| r.get(0)).unwrap();
    assert!(raw.contains("\"p\":"), "stored as Rails' encryption envelope: {raw}");
    assert_eq!(app_config::get(&c, &cipher, "k").unwrap().as_deref(), Some("32"));
    assert!(app_config::exists(&c, "k").unwrap());
    assert!(!app_config::exists(&c, "missing").unwrap());
}

use deltabadger::jobs::schedule::{first_due, Jitter, Schedule};

#[test]
fn schedules_fire_as_rails_cron_does_in_utc() {
    let daily = Schedule::Daily { hour: 10, minute: 15 }; // "15 10 * * *"
    assert_eq!(daily.last_fire(at("2026-10-02T10:14:59Z")), at("2026-10-01T10:15:00Z"));
    assert_eq!(daily.last_fire(at("2026-10-02T10:15:00Z")), at("2026-10-02T10:15:00Z"));
    assert_eq!(daily.next_fire(at("2026-10-02T10:15:00Z")), at("2026-10-03T10:15:00Z"));
    let four = Schedule::EveryHours { every: 4, minute: 15 }; // "15 */4 * * *"
    assert_eq!(four.last_fire(at("2026-10-02T00:10:00Z")), at("2026-10-01T20:15:00Z"), "across midnight");
    assert_eq!(four.last_fire(at("2026-10-02T13:00:00Z")), at("2026-10-02T12:15:00Z"));
    assert_eq!(four.next_fire(at("2026-10-02T23:59:00Z")), at("2026-10-03T00:15:00Z"));
    assert_eq!((daily.period(), four.period()), (Duration::hours(24), Duration::hours(4)));
    assert_eq!((daily.stale_after(), four.stale_after()), (Duration::hours(49), Duration::hours(9)), "2 × period + 1 h");
}

#[test]
fn a_job_is_due_at_start_when_its_latest_fire_has_no_success_since() {
    let s = Schedule::Daily { hour: 10, minute: 0 };
    let draw = Duration::seconds(300);
    let now = at("2026-10-02T12:00:00Z");
    assert_eq!(first_due(s, draw, None, now), at("2026-10-02T10:05:00Z"), "never ran here: the draw is past, so at once");
    assert_eq!(first_due(s, draw, Some(at("2026-10-02T09:59:59Z")), now), at("2026-10-02T10:05:00Z"), "Rails' last run was yesterday's");
    assert_eq!(first_due(s, draw, Some(at("2026-10-02T10:04:00Z")), now), at("2026-10-03T10:05:00Z"), "ran after the fire: tomorrow");
    assert_eq!(first_due(s, draw, None, at("2026-10-02T10:01:00Z")), at("2026-10-02T10:05:00Z"), "inside the window the draw still applies");
}

#[test]
fn jitter_draws_inside_its_window() {
    for _ in 0..500 {
        let d = Jitter { min_secs: 1, max_secs: 900 }.draw().num_seconds();
        assert!((1..=900).contains(&d), "{d}");
    }
    assert_eq!(Jitter::NONE.draw(), Duration::zero());
}

use deltabadger::engine::events::{EngineEvent, EngineEvents};
use deltabadger::engine::{Clock, SystemClock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use deltabadger::jobs::{Cx, Job, JobFuture, Outcome, Retry, Scheduler, Spec, Wake, DEADLINE};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use tokio::sync::watch;

/// Wall time that follows tokio's clock, so a paused test runtime moves it.
struct TokioClock { start: DateTime<Utc>, origin: tokio::time::Instant }
impl Clock for TokioClock {
    fn now(&self) -> DateTime<Utc> { self.start + Duration::from_std(self.origin.elapsed()).unwrap() }
}
fn clock(s: &str) -> TokioClock { TokioClock { start: at(s), origin: tokio::time::Instant::now() } }

type Runs = Rc<RefCell<Vec<(DateTime<Utc>, Vec<Wake>)>>>;
type Hook = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;
/// The names of the jobs that ran, in order: a log several fakes share.
type Order = Rc<RefCell<Vec<&'static str>>>;

struct Fake { spec: Spec, runs: Runs, outcomes: RefCell<VecDeque<Outcome>>, wants_orders: bool, hang: bool, hook: Hook, order: Order }

impl Job for Fake {
    fn spec(&self) -> Spec { self.spec.clone() }
    fn wants(&self, e: &EngineEvent) -> bool { self.wants_orders && matches!(e, EngineEvent::OrderRecorded { .. }) }
    fn run<'a>(&'a self, cx: Cx<'a>, wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            self.order.borrow_mut().push(self.spec.name);
            self.runs.borrow_mut().push((cx.clock.now(), wakes));
            let hook = self.hook.borrow_mut().take();
            if let Some(f) = hook { f(); }
            if self.hang { std::future::pending::<()>().await; }
            self.outcomes.borrow_mut().pop_front().unwrap_or(Outcome::Done)
        })
    }
}

fn fake(name: &'static str, schedule: Option<Schedule>, retry: Retry, outcomes: Vec<Outcome>) -> (Fake, Runs, Hook) {
    let (runs, hook): (Runs, Hook) = Default::default();
    let spec = Spec { name, scope: None, schedule, jitter: Jitter::NONE, retry, deadline: DEADLINE };
    let order = Order::default();
    (Fake { spec, runs: runs.clone(), outcomes: RefCell::new(outcomes.into()), wants_orders: false, hang: false, hook: hook.clone(), order }, runs, hook)
}

/// Runs `s` for `virtual_time`, then stops it and waits for it to return.
async fn drive(s: Scheduler, c: &TokioClock, virtual_time: std::time::Duration, during: impl std::future::Future<Output = ()>) {
    let (tx, rx) = watch::channel(false);
    let (ended, ()) = tokio::join!(s.run(rx, c), async {
        during.await;
        tokio::time::sleep(virtual_time).await;
        tx.send(true).unwrap();
    });
    ended.unwrap();
}

fn times(runs: &Runs) -> Vec<DateTime<Utc>> { runs.borrow().iter().map(|r| r.0).collect() }
fn second_connection(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }
const DAY: std::time::Duration = std::time::Duration::from_secs(86_400);

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_missed_fire_runs_once_at_start_then_the_job_runs_at_each_fire_and_records_its_success() {
    let (d, c) = db();
    let (job, runs, _) = fake("daily", Some(Schedule::Daily { hour: 10, minute: 15 }), Retry::None, vec![]);
    let now = clock("2026-10-02T12:00:00Z");
    drive(Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None), &now, 2 * DAY, async {}).await;
    assert_eq!(times(&runs), vec![at("2026-10-02T12:00:00Z"), at("2026-10-03T10:15:00Z"), at("2026-10-04T10:15:00Z")]);
    assert!(runs.borrow().iter().all(|r| r.1 == vec![Wake::Schedule]));
    assert_eq!(state::read(&second_connection(&d), "daily", None).unwrap().last_success_at, Some(at("2026-10-04T10:15:00Z")));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_success_since_the_latest_fire_survives_a_restart_and_the_job_waits_for_its_next_fire() {
    let (_d, c) = db();
    state::record_success(&c, "daily", None, at("2026-10-02T10:15:02Z")).unwrap();
    let (job, runs, _) = fake("daily", Some(Schedule::Daily { hour: 10, minute: 15 }), Retry::None, vec![]);
    drive(Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None), &clock("2026-10-02T12:00:00Z"), DAY, async {}).await;
    assert_eq!(times(&runs), vec![at("2026-10-03T10:15:00Z")]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn transient_failures_retry_after_polynomial_waits_counted_per_kind_then_the_error_is_recorded() {
    let (d, c) = db();
    let (job, runs, _) = fake("pull", Some(Schedule::Daily { hour: 10, minute: 0 }), Retry::Polynomial { attempts: 2 }, vec![
        Outcome::Transient("t1".into()), Outcome::RateLimited("r1".into()), Outcome::Transient("t2".into())]);
    drive(Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None), &clock("2026-10-02T10:30:00Z"), std::time::Duration::from_secs(3600), async {}).await;
    // 1⁴ + 2 = 3 s after each first failure of a kind; the second transient spends its 2 attempts.
    assert_eq!(times(&runs), vec![at("2026-10-02T10:30:00Z"), at("2026-10-02T10:30:03Z"), at("2026-10-02T10:30:06Z")]);
    assert!(runs.borrow().iter().all(|r| r.1 == vec![Wake::Schedule]), "a retry replays the failed run's wakes");
    let s = state::read(&second_connection(&d), "pull", None).unwrap();
    assert!(s.failing());
    assert_eq!(s.last_error.as_deref(), Some("t2"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_wake_during_the_jobs_own_run_runs_it_again_after_and_events_reach_only_the_jobs_that_want_them() {
    let (_d, c) = db();
    let (mut ledger, ledger_runs, hook) = fake("ledger", None, Retry::None, vec![]);
    ledger.wants_orders = true;
    let (other, other_runs, _) = fake("other", None, Retry::None, vec![]);
    let mut events = EngineEvents::default();
    let rx = events.subscribe();
    let s = Scheduler::new(c, seed::cipher(), vec![Box::new(ledger), Box::new(other)], Some(rx));
    let wakers = s.wakers();
    *hook.borrow_mut() = Some(Box::new(move || wakers.wake("ledger", None, Some(7)))); // during the first run
    let order = EngineEvent::OrderRecorded { bot_id: 1, transaction_id: 10 };
    let sent = order.clone();
    drive(s, &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(5), async move { events.send(sent); }).await;
    let wakes: Vec<Vec<Wake>> = ledger_runs.borrow().iter().map(|r| r.1.clone()).collect();
    assert_eq!(wakes, vec![vec![Wake::Event(order)], vec![Wake::Manual(Some(7))]]);
    assert!(other_runs.borrow().is_empty(), "a job that does not want the event never runs for it");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_stop_drops_the_job_in_hand_and_records_nothing_for_it() {
    let (d, c) = db();
    let (mut job, runs, _) = fake("slow", Some(Schedule::Daily { hour: 0, minute: 0 }), Retry::None, vec![]);
    job.hang = true;
    drive(Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None), &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(5), async {}).await;
    assert_eq!(runs.borrow().len(), 1, "it started, and the stop returned anyway");
    assert_eq!(state::read(&second_connection(&d), "slow", None).unwrap(), JobState::default(), "so it runs again at the next start");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_run_past_its_deadline_is_dropped_recorded_as_an_error_and_the_runner_moves_on() {
    let (d, c) = db();
    assert_eq!(DEADLINE, std::time::Duration::from_secs(600), "ten minutes unless the job declares its own");
    let (mut slow, slow_runs, _) = fake("slow", Some(Schedule::Daily { hour: 0, minute: 0 }), Retry::None, vec![]);
    slow.hang = true;
    let (next, next_runs, _) = fake("next", Some(Schedule::Daily { hour: 1, minute: 0 }), Retry::None, vec![]);
    let s = Scheduler::new(c, seed::cipher(), vec![Box::new(slow), Box::new(next)], None);
    drive(s, &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(3600), async {}).await;
    assert_eq!(times(&slow_runs), vec![at("2026-10-02T12:00:00Z")]);
    assert_eq!(times(&next_runs), vec![at("2026-10-02T12:10:00Z")], "the runner is free the moment the deadline drops the run");
    let st = state::read(&second_connection(&d), "slow", None).unwrap();
    assert_eq!((st.last_error_at, st.last_error.as_deref()), (Some(at("2026-10-02T12:10:00Z")), Some("dropped past its 600s deadline")));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_oldest_due_job_runs_first_whatever_its_registration_order() {
    let (_d, c) = db();
    let order = Order::default();
    let (mut late, _, _) = fake("late", Some(Schedule::Daily { hour: 11, minute: 0 }), Retry::None, vec![]);
    let (mut early, _, _) = fake("early", Some(Schedule::Daily { hour: 6, minute: 0 }), Retry::None, vec![]);
    (late.order, early.order) = (order.clone(), order.clone());
    let s = Scheduler::new(c, seed::cipher(), vec![Box::new(late), Box::new(early)], None);
    drive(s, &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(60), async {}).await;
    assert_eq!(*order.borrow(), vec!["early", "late"], "the missed 06:00 fire is older than the missed 11:00 one");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_woken_job_never_runs_ahead_of_a_job_overdue_past_its_staleness_bound() {
    let (_d, c) = db();
    // "four" succeeded after its 08:15 fire, so it is next due at 12:15; its bound is 2 × 4 h + 1 h = 9 h.
    state::record_success(&c, "four", None, at("2026-10-02T08:20:00Z")).unwrap();
    let order = Order::default();
    let (mut hog, _, _) = fake("hog", Some(Schedule::Daily { hour: 11, minute: 0 }), Retry::None, vec![]);
    hog.hang = true;
    hog.spec.deadline = std::time::Duration::from_secs(11 * 3600); // a job that declared a long deadline: it holds the runner until 23:00
    let (mut four, _, _) = fake("four", Some(Schedule::EveryHours { every: 4, minute: 15 }), Retry::None, vec![]);
    let (mut mail, _, _) = fake("mail", None, Retry::None, vec![]);
    (hog.order, four.order, mail.order) = (order.clone(), order.clone(), order.clone());
    let s = Scheduler::new(c, seed::cipher(), vec![Box::new(hog), Box::new(four), Box::new(mail)], None);
    s.wakers().wake("mail", None, None); // due from 12:00, when the runner first looks: older than four's 12:15
    drive(s, &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(12 * 3600), async {}).await;
    assert_eq!(*order.borrow(), vec!["hog", "four", "mail"], "at 23:00 four is 10 h 45 min overdue, past its 9 h bound: it runs before the older wake");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_job_registered_per_scope_is_scheduled_woken_and_recorded_per_scope() {
    let (d, c) = db();
    // Key 2 synced after today's 02:00 fire; key 1 did not.
    state::record_success(&c, "ledger", Some("2"), at("2026-10-02T02:00:30Z")).unwrap();
    let (mut jobs, mut runs): (Vec<Box<dyn Job>>, Vec<Runs>) = (vec![], vec![]);
    for key in ["1", "2"] {
        let outcomes = if key == "2" { vec![Outcome::Failed("key 2 refused".into())] } else { vec![] };
        let (mut job, r, _) = fake("ledger", Some(Schedule::Daily { hour: 2, minute: 0 }), Retry::None, outcomes);
        job.spec.scope = Some(key.into());
        jobs.push(Box::new(job));
        runs.push(r);
    }
    let s = Scheduler::new(c, seed::cipher(), jobs, None);
    let wakers = s.wakers();
    drive(s, &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(60), async move { wakers.wake("ledger", Some("2"), Some(42)); }).await;
    let wakes = |r: &Runs| r.borrow().iter().map(|run| run.1.clone()).collect::<Vec<_>>();
    assert_eq!(wakes(&runs[0]), vec![vec![Wake::Schedule]], "key 1 missed its fire: it runs at start, for its schedule only");
    assert_eq!(wakes(&runs[1]), vec![vec![Wake::Manual(Some(42))]], "key 2 ran after the fire: only its own wake runs it");
    let c = second_connection(&d);
    assert_eq!(state::read(&c, "ledger", Some("1")).unwrap().last_success_at, Some(at("2026-10-02T12:00:00Z")));
    let two = state::read(&c, "ledger", Some("2")).unwrap();
    assert_eq!((two.last_success_at, two.last_error.as_deref()), (Some(at("2026-10-02T02:00:30Z")), Some("key 2 refused")));
    assert_eq!(state::all(&c).unwrap().into_iter().map(|(n, _)| n).collect::<Vec<_>>(), vec!["ledger:1", "ledger:2"]);
}

/// How long one blocking unit of `Chunky` holds SQLite's write lock: an import's chunk.
const UNIT: std::time::Duration = std::time::Duration::from_millis(50);

/// A job whose run is an endless sequence of blocking units, each one transaction holding the write lock for `UNIT`, as
/// import::publish runs a phase. `done` counts the units that committed.
struct Chunky { spec: Spec, done: Arc<AtomicUsize> }

impl Job for Chunky {
    fn spec(&self) -> Spec { self.spec.clone() }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            loop {
                let done = self.done.clone();
                let unit = cx.db.run(move |c, _| {
                    c.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
                    std::thread::sleep(UNIT);
                    c.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                    done.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }).await;
                if let Err(e) = unit { return Outcome::Failed(e); }
            }
        })
    }
}

fn chunky(deadline: std::time::Duration) -> (Chunky, Arc<AtomicUsize>) {
    let done = Arc::new(AtomicUsize::new(0));
    let spec = Spec { name: "chunky", scope: None, schedule: Some(Schedule::Daily { hour: 0, minute: 0 }), jitter: Jitter::NONE, retry: Retry::None, deadline };
    (Chunky { spec, done: done.clone() }, done)
}

#[tokio::test(flavor = "current_thread")] // real time: the units sleep on the blocking pool
async fn a_deadline_mid_import_ends_the_run_within_one_unit_and_records_it() {
    let (d, c) = db();
    let deadline = std::time::Duration::from_millis(300);
    let (job, done) = chunky(deadline);
    let (tx, rx) = watch::channel(false);
    let file = d.path().join("production.sqlite3");
    let s = Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None); // the cipher's key derivation before the clock starts
    let started = std::time::Instant::now();
    let watch_record = async {
        loop {
            let st = state::read(&Connection::open(&file).unwrap(), "chunky", None).unwrap();
            if st.last_error.is_some() { break (started.elapsed(), done.load(Ordering::SeqCst), st); }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };
    let (ended, (elapsed, units, st)) = tokio::join!(s.run(rx, &SystemClock), async {
        let seen = watch_record.await;
        tx.send(true).unwrap();
        seen
    });
    ended.unwrap();
    assert_eq!(st.last_error.as_deref(), Some("dropped past its 300ms deadline"));
    assert!(elapsed < deadline + 2 * UNIT + std::time::Duration::from_millis(100), "recorded {elapsed:?} after the start: the unit in hand, then the record");
    tokio::time::sleep(4 * UNIT).await;
    assert_eq!(done.load(Ordering::SeqCst), units, "no abandoned work ran past the unit in hand");
}

#[tokio::test(flavor = "current_thread")] // real time
async fn a_stop_mid_import_returns_at_once_and_leaves_at_most_the_unit_in_hand() {
    let (d, c) = db();
    let (job, done) = chunky(DEADLINE);
    let (tx, rx) = watch::channel(false);
    let stopped = std::cell::Cell::new(None);
    let (ended, ()) = tokio::join!(Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None).run(rx, &SystemClock), async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tx.send(true).unwrap();
        stopped.set(Some((std::time::Instant::now(), done.load(Ordering::SeqCst))));
    });
    ended.unwrap();
    let (at, units) = stopped.get().unwrap();
    assert!(at.elapsed() < UNIT, "the runner returned {:?} after the stop", at.elapsed());
    tokio::time::sleep(4 * UNIT).await;
    assert!(done.load(Ordering::SeqCst) <= units + 1, "only the unit in hand finished after the stop");
    assert_eq!(state::read(&second_connection(&d), "chunky", None).unwrap(), JobState::default(), "a stop records nothing");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_run_that_refreshed_nothing_is_recorded_as_a_run_not_a_success() {
    let (d, c) = db();
    let (job, runs, _) = fake("empty", Some(Schedule::Daily { hour: 10, minute: 15 }), Retry::None, vec![Outcome::NothingNew]);
    drive(Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None), &clock("2026-10-02T12:00:00Z"), std::time::Duration::from_secs(60), async {}).await;
    assert_eq!(runs.borrow().len(), 1);
    let st = state::read(&second_connection(&d), "empty", None).unwrap();
    assert_eq!((st.last_run_at, st.last_success_at), (Some(at("2026-10-02T12:00:00Z")), None), "freshness reads only last_success_at");
    // A restart the same day: the run counts for the schedule, so the job waits for its next fire.
    let (job, runs, _) = fake("empty", Some(Schedule::Daily { hour: 10, minute: 15 }), Retry::None, vec![]);
    drive(Scheduler::new(second_connection(&d), seed::cipher(), vec![Box::new(job)], None), &clock("2026-10-02T13:00:00Z"),
          std::time::Duration::from_secs(60), async {}).await;
    assert!(runs.borrow().is_empty());
}

#[tokio::test(flavor = "current_thread")] // real time: another connection holds SQLite's write lock
async fn a_stop_while_the_record_waits_for_the_write_lock_returns_at_once_and_skips_the_record() {
    let (d, c) = db();
    let (job, runs, _) = fake("done", Some(Schedule::Daily { hour: 0, minute: 0 }), Retry::None, vec![]); // due at start, done at once
    // Built before the lock is taken: the cipher's key derivation takes long in a debug build.
    let s = Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None);
    let file = d.path().join("production.sqlite3");
    let (locked, wait_locked) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let other = Connection::open(file).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        locked.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(3)); // past the record's own wait (RECORD_BUSY, 1 s)
        other.execute_batch("COMMIT").unwrap();
    });
    wait_locked.recv().unwrap();
    let (tx, rx) = watch::channel(false);
    let stopped = std::cell::Cell::new(None);
    let (ended, ()) = tokio::join!(s.run(rx, &SystemClock), async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await; // the run is over; its record waits for the lock
        tx.send(true).unwrap();
        stopped.set(Some(std::time::Instant::now()));
    });
    ended.unwrap();
    assert!(stopped.get().unwrap().elapsed() < std::time::Duration::from_millis(100), "the runner returned at the stop, not after the lock");
    assert_eq!(runs.borrow().len(), 1);
    holder.join().unwrap();
    assert_eq!(state::read(&second_connection(&d), "done", None).unwrap(), JobState::default(), "the record was skipped");
}
