//! Plan 2e: the engine and the web UI in one process (`supervisor::serve`), on one runtime, under one lock.
mod common;
use common::seed::{self, BotSpec, TxSpec};
use common::web::{self as w, TestClock};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{model, EngineError, SystemClock};
use deltabadger::enums::BotStatus;
use deltabadger::supervisor::{self, Ended, Service};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use deltabadger::venue::fake::{FakeFactory, FakeVenue};
use deltabadger::web::App;
use deltabadger::{codec, lease, ruby};
use deltabadger::store::{self, Paths};
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

fn priced() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0") }
fn ago(seconds: i64) -> String { codec::format_time(chrono::Utc::now() - chrono::Duration::seconds(seconds)) }

struct Rig { _dir: tempfile::TempDir, engine: Engine<FakeFactory>, app: App, listener: tokio::net::TcpListener, port: u16, bot: i64, db: PathBuf, s: seed::Seeded }

/// I-1: the longest any stretch of a full pass may hold the runtime thread (a debug build). /cable pings every 3 s and
/// a client calls the connection stale after 6 s; a page request waits for the thread too.
const RUNTIME_THREAD_BOUND: Duration = Duration::from_millis(250);

/// A task on this test's runtime thread that wakes every millisecond and records the longest gap between two wakes:
/// how long something else held the thread.
struct Meter { max_us: Arc<AtomicU64>, samples: Arc<AtomicU64>, task: tokio::task::JoinHandle<()> }

impl Meter {
    fn start() -> Self {
        let (max_us, samples) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (m, n) = (max_us.clone(), samples.clone());
        let task = tokio::spawn(async move {
            let mut last = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let now = Instant::now();
                m.fetch_max((now - last).as_micros() as u64, Ordering::SeqCst);
                n.fetch_add(1, Ordering::SeqCst);
                last = now;
            }
        });
        Self { max_us, samples, task }
    }

    /// The longest hold, once the meter has taken one more sample: a stretch that ended just before this call (the
    /// last synchronous segment of what was measured) is in it too.
    async fn stop(self) -> Duration {
        let seen = self.samples.load(Ordering::SeqCst);
        while self.samples.load(Ordering::SeqCst) == seen {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        self.task.abort();
        Duration::from_micros(self.max_us.load(Ordering::SeqCst))
    }
}

/// An install of ordinary size around the rig's bot: 500 settled orders on the rig's (due) bot inside its current
/// schedule, which its tick sums (`amount.rs`'s invested amount; each executed 0.0001, so it still owes and buys), and 49
/// more bots, stopped, each with 20 settled orders.
fn ordinary_install(r: &Rig) {
    let c = rusqlite::Connection::open(&r.db).unwrap();
    for n in 0..500 {
        seed::insert_tx(&c, &r.s, r.bot, &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("ODUE-{n}")), order_type: 0,
            amount: Some("0.000000002"), quote_amount: Some("0.0001"), price: Some("50000"), quote_amount_exec: Some("0.0001"),
            amount_exec: Some("0.000000002"), created_at: ago(3600) });
    }
    for _ in 0..49 {
        let id = seed::insert_bot(&c, &r.s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
        for n in 0..20 {
            seed::insert_tx(&c, &r.s, id, &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("OSETTLED-{id}-{n}")), order_type: 0,
                amount: Some("0.0012"), quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: Some("60"), amount_exec: Some("0.0012"),
                created_at: "2026-09-01 10:00:02".into() });
        }
    }
}

/// A Rails-prepared Kraken install with one bot, locked as `serve` locks it: the engine on one connection, the web on
/// its own, a listener bound but not yet serving.
async fn rig(v: FakeVenue, spec: BotSpec) -> Rig {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, chrono::Utc::now()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let bot = seed::insert_bot(&o.primary, &s, &spec);
    let app = w::app(dir.path(), w::SECRET, TestClock::at("2026-09-10T12:00:30Z"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let db = dir.path().join("production.sqlite3");
    Rig { engine: Engine::new(o.primary, FakeFactory(v), seed::cipher(), lock), app, listener, port, bot, db, s, _dir: dir }
}

/// GET /up from a plain socket, off the runtime thread; `None` when nothing listens.
async fn up(port: u16) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(stream, "GET /up HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer).ok()?;
        Some(answer)
    }).await.unwrap()
}

async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn status(db: &Path, bot: i64) -> i64 {
    rusqlite::Connection::open(db).unwrap().query_row("SELECT status FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap()
}

fn outstanding(db: &Path, external_id: &str) -> i64 {
    rusqlite::Connection::open(db).unwrap()
        .query_row("SELECT count(*) FROM transactions WHERE external_id = ?1 AND status = 0 AND external_status IN (0, 1)", [external_id], |r| r.get(0)).unwrap()
}

/// The web's start (`Lifecycle#start` with start_fresh=true) as Plan 3's handler writes it (I-6): scheduled, a new
/// anchor, no last action, and only from created or stopped. `?1` now, `?2` the bot.
const WEB_START: &str = "UPDATE bots SET status = 1, stop_message_key = NULL, started_at = ?1, \
    transient_data = json_remove(transient_data, '$.last_action_job_at') WHERE id = ?2 AND status IN (0, 2)";

/// The web's stop (`Lifecycle#stop`) as Plan 3's handler writes it (I-6): unconditional but for a terminal bot, as
/// Rails returns early for an archived or deleted one (`lifecycle.rb:91-98`). `?1` now, `?2` the bot.
const WEB_STOP: &str = "UPDATE bots SET status = 2, stopped_at = ?1, stop_message_key = NULL, updated_at = ?1 WHERE id = ?2 AND status NOT IN (3, 7)";

/// The bot's `last_action_job_at`, as stored: a tick ran when it moves.
fn last_action(db: &Path, bot: i64) -> Option<String> {
    rusqlite::Connection::open(db).unwrap()
        .query_row("SELECT json_extract(transient_data, '$.last_action_job_at') FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap()
}

/// Every column of the bot's row, as stored.
fn bot_row(db: &Path, bot: i64) -> Vec<rusqlite::types::Value> {
    rusqlite::Connection::open(db).unwrap()
        .query_row("SELECT * FROM bots WHERE id = ?1", [bot], |r| (0..r.as_ref().column_count()).map(|i| r.get(i)).collect()).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn sigterm_lets_the_tick_in_hand_finish_while_the_web_still_answers_then_stops_both() {
    let gate = Rc::new(Notify::new());
    let v = priced().hold_add(gate.clone());
    let r = rig(v.clone(), BotSpec::weekly(60.0, &ago(1))).await; // due now
    let (stop, port) = (r.engine.stop_handle(), r.port);
    let driver = async {
        until("the tick to send its order", || !v.sent().is_empty()).await; // AddOrder's reply is held: the tick is in hand
        stop.request(); // what the SIGTERM handler does
        let during = up(port).await;
        gate.notify_one();
        during
    };
    let (ended, during) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![]), driver);
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    assert!(during.as_deref().is_some_and(|a| a.starts_with("HTTP/1.1 200")), "the web answers while the engine drains: {during:?}");
    assert!(up(port).await.is_none(), "once the engine has returned, nothing listens");
    assert_eq!(v.sent().len(), 1);
    assert_eq!(status(&r.db, r.bot), BotStatus::Scheduled as i64, "the tick in hand finished");
    assert_eq!(outstanding(&r.db, "OFAKE-1"), 1, "its order is recorded, for the next start or the handback to poll");
}

#[tokio::test(flavor = "current_thread")]
async fn a_web_stop_during_an_in_flight_tick_wins_and_nothing_more_is_placed() {
    let gate = Rc::new(Notify::new());
    let v = priced().hold_add(gate.clone());
    let r = rig(v.clone(), BotSpec::weekly(60.0, &ago(1))).await;
    let (stop, app, bot) = (r.engine.stop_handle(), r.app.clone(), r.bot);
    let driver = async {
        until("the tick to send its order", || !v.sent().is_empty()).await;
        // Lifecycle#stop as the web writes it (interface I-6).
        let now = codec::format_time(chrono::Utc::now());
        app.db(move |c| {
            c.execute(WEB_STOP, rusqlite::params![now, bot])?;
            Ok(())
        }).await.unwrap();
        app.wake_engine();
        gate.notify_one();
        tokio::time::sleep(Duration::from_millis(300)).await; // the woken pass runs
        stop.request();
    };
    let (ended, ()) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![]), driver);
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    assert_eq!(status(&r.db, r.bot), BotStatus::Stopped as i64, "no write of the tick undid the stop");
    assert_eq!(v.sent().len(), 1, "the order already sent stands; nothing more is placed");
    assert_eq!(outstanding(&r.db, "OFAKE-1"), 1, "recorded and still owed its poll");
}

#[tokio::test(flavor = "current_thread")]
async fn a_web_start_or_settings_write_followed_by_wake_engine_ticks_without_waiting_for_the_idle_cap() {
    let two_hours = 2 * 3600;
    let cases: [(&str, BotSpec, &'static str); 2] = [
        // Start with start_fresh=true on a stopped bot (Lifecycle#start): scheduled, a new anchor, no last action.
        ("start", BotSpec { status: 2, ..BotSpec::weekly(60.0, &ago(1)) }, WEB_START),
        // A settings write: weekly → hourly makes the last hour's checkpoint due.
        ("settings", BotSpec::weekly(60.0, &ago(two_hours + 30))
             .transient("last_action_job_at", json!(ruby::iso8601_ms(chrono::Utc::now() - chrono::Duration::seconds(two_hours)))),
         "UPDATE bots SET settings = json_set(settings, '$.interval', 'hour'), settings_changed_at = ?1 WHERE id = ?2"),
    ];
    for (case, spec, write) in cases {
        let v = priced();
        let r = rig(v.clone(), spec).await;
        let (stop, app, bot, db) = (r.engine.stop_handle(), r.app.clone(), r.bot, r.db.clone());
        let driver = async {
            tokio::time::sleep(Duration::from_millis(300)).await; // the engine is asleep on its 60 s idle cap
            assert!(v.sent().is_empty(), "{case}: nothing is due before the write");
            let before = last_action(&db, bot);
            let now = codec::format_time(chrono::Utc::now());
            app.db(move |c| { c.execute(write, rusqlite::params![now, bot])?; Ok(()) }).await.unwrap();
            app.wake_engine();
            // The tick ran. (A settings write restarts the amount's window at settings_changed_at, so that tick owes
            // nothing and places nothing; the start owes one interval and buys.)
            until(case, || last_action(&db, bot).is_some_and(|t| Some(&t) != before.as_ref())).await;
            stop.request();
        };
        let t0 = Instant::now();
        let (ended, ()) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![]), driver);
        assert!(matches!(ended, Ended::Stopped), "{case}: {ended:?}");
        assert!(t0.elapsed() < Duration::from_secs(30), "{case}: well inside the idle cap: {:?}", t0.elapsed());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_engine_failure_ends_the_web_with_it() {
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await; // not due for decades
    let (app, port) = (r.app.clone(), r.port);
    let driver = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(up(port).await.is_some_and(|a| a.starts_with("HTTP/1.1 200")), "serving");
        // The engine's next pass cannot read the install (an engine-level failure; an unguarded ineligible write is the
        // debug assertion's case, below).
        app.db(move |c| { c.execute_batch("ALTER TABLE rules RENAME TO rules_gone")?; Ok(()) }).await.unwrap();
        app.wake_engine();
    };
    let both = async { tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![]), driver) };
    let (ended, ()) = tokio::time::timeout(Duration::from_secs(20), both).await.expect("the engine ended on the wake");
    assert!(matches!(ended, Ended::Engine(EngineError::Sqlite(_))), "{ended:?}");
    assert!(up(port).await.is_none(), "no half-alive process: the web stopped with the engine");
}

/// R2(a): in one process every writer of `bots` runs `eligibility::guard`; a write that skips it is caught by the
/// engine's next pass, naming the bot. (Release builds end the process instead: Ineligible, exit 2.)
#[cfg(debug_assertions)]
#[tokio::test(flavor = "current_thread")]
async fn a_web_write_that_skips_the_guard_trips_the_debug_assertion_naming_the_bot() {
    let Rig { _dir: _keep, engine, app, listener, bot, .. } = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let web = app.clone();
    let local = tokio::task::LocalSet::new();
    let supervised = local.spawn_local(async move { supervisor::serve(engine, Some((app, listener)), &SystemClock, vec![]).await });
    local.run_until(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        // What the guard refuses (Task 2a), committed without it.
        web.db(move |c| {
            c.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true')) WHERE id = ?1", [bot])?;
            Ok(())
        }).await.unwrap();
        web.wake_engine();
    }).await;
    let joined = tokio::time::timeout(Duration::from_secs(20), local.run_until(supervised)).await.expect("the woken pass ran");
    let panic = joined.expect_err("the pass must panic").into_panic();
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("skipped eligibility::guard") && message.contains(&format!("bot {bot} (scheduled): quote_amount_limited")), "{message}");
}

/// R2(a), the guard's second refusal: a write that skips it and moves a bot with an unresolved order onto another asset
/// strands that order (the install stays eligible). The engine's next pass names the bot all the same.
#[cfg(debug_assertions)]
#[tokio::test(flavor = "current_thread")]
async fn a_web_write_that_strands_an_unresolved_order_trips_the_debug_assertion_naming_the_bot() {
    use deltabadger::engine::{amount, placement, venue_rules, FixedClock};
    use deltabadger::ruby::BigDec;
    // Stopped, so nothing ticks it; its order was sent and the reply lost (Kraken's deadline is ahead: still pending).
    let Rig { _dir: _keep, engine, app, listener, bot, db, s, .. } = rig(priced(), BotSpec { status: 2, ..BotSpec::weekly(60.0, "2099-01-01 00:00:00") }).await;
    let c = rusqlite::Connection::open(&db).unwrap();
    let b = model::load_bot(&c, bot).unwrap();
    let ticker = model::ticker_for(&c, &b).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&b, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), venue_rules::KRAKEN.minimum_logic)
        else { panic!("sized") };
    placement::begin(&c, &b, &plan, &FixedClock(chrono::Utc::now())).unwrap();
    // Another plain cryptocurrency on the same venue.
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) \
               VALUES ('ethereum', 'ETH', 'Ethereum', 'Cryptocurrency', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", []).unwrap();
    let eth = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
               VALUES (?1, 'ETHEUR', 'ETH', 'EUR', ?2, ?3, 8, 5, 2, '0.002', '0.5', 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
              rusqlite::params![s.exchange_id, eth, s.quote]).unwrap();
    drop(c);
    let web = app.clone();
    let local = tokio::task::LocalSet::new();
    let supervised = local.spawn_local(async move { supervisor::serve(engine, Some((app, listener)), &SystemClock, vec![]).await });
    local.run_until(async {
        tokio::time::sleep(Duration::from_millis(300)).await; // the first pass ran and found the order pending
        // What the guard refuses as Reconciling, committed without it.
        let allocations = json!({ eth.to_string(): 1.0 }).to_string();
        web.db(move |c| {
            c.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json(?1)) WHERE id = ?2", rusqlite::params![allocations, bot])?;
            Ok(())
        }).await.unwrap();
        web.wake_engine();
    }).await;
    let joined = tokio::time::timeout(Duration::from_secs(20), local.run_until(supervised)).await.expect("the woken pass ran");
    let panic = joined.expect_err("the pass must panic").into_panic();
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("skipped eligibility::guard") && message.contains(&format!("bot {bot}: stranded")), "{message}");
}

/// I-1, measured: with a venue that answers slowly (each call awaits 50 ms of timer, so the thread is free meanwhile)
/// and an install of ordinary size, a full pass with a full tick never holds the runtime thread longer than the bound.
#[tokio::test(flavor = "current_thread")]
async fn a_full_tick_never_holds_the_runtime_thread_longer_than_the_bound() {
    let v = priced().latency(Duration::from_millis(50));
    // Due now, started eight days ago: its schedule holds the 500 orders `ordinary_install` gives it.
    let mut r = rig(v.clone(), BotSpec::weekly(60.0, &ago(8 * 86_400))).await;
    ordinary_install(&r);
    let meter = Meter::start();
    tokio::time::sleep(Duration::from_millis(20)).await; // the meter is running
    let t0 = Instant::now();
    run::step(&mut r.engine, &SystemClock).await.unwrap();
    let took = t0.elapsed();
    let held = meter.stop().await; // after one more sample: the pass's last stretch is measured too
    println!("I-1 measurement: full pass {took:?}; longest hold of the runtime thread {held:?}");
    assert_eq!(v.sent().len(), 1, "a full tick: price, AddOrder, balance");
    assert!(took >= Duration::from_millis(150), "the venue's three awaits yielded the thread: {took:?}");
    assert!(held < RUNTIME_THREAD_BOUND, "held the runtime thread {held:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_service_that_ends_unasked_ends_the_process_after_the_engine_drains() {
    for with_web in [true, false] { // `serve`, then `run`: one supervisor, one rule
        let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
        let port = r.port;
        let failing = Service { name: "scheduler", run: Box::pin(async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            Err::<(), String>("reference data unreachable".into())
        }) };
        let web = with_web.then_some((r.app, r.listener));
        let ended = tokio::time::timeout(Duration::from_secs(20), supervisor::serve(r.engine, web, &SystemClock, vec![failing]))
            .await.expect("ended");
        assert!(matches!(&ended, Ended::Service { name: "scheduler", error } if error == "reference data unreachable"), "web {with_web}: {ended:?}");
        assert!(up(port).await.is_none(), "web {with_web}: nothing listens once it has ended");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_waits_for_every_service_to_finish_its_unit() {
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let (stop, port) = (r.engine.stop_handle(), r.port);
    let finished = Arc::new(AtomicBool::new(false));
    let (svc_stop, svc_done) = (stop.clone(), finished.clone());
    let mailer = Service { name: "mail", run: Box::pin(async move {
        svc_stop.requested().await; // the one stop signal
        tokio::time::sleep(Duration::from_millis(300)).await; // the unit in hand
        svc_done.store(true, Ordering::SeqCst);
        Ok(())
    }) };
    let driver = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.request(); // what the SIGTERM handler does
    };
    let (ended, ()) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![mailer]), driver);
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    assert!(finished.load(Ordering::SeqCst), "serve returned only after the service finished its unit");
    assert!(up(port).await.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_web_write_holding_the_database_delays_the_engine_but_never_fails_it() {
    let v = priced();
    let r = rig(v.clone(), BotSpec::weekly(60.0, &ago(1))).await; // due now
    let mode: String = rusqlite::Connection::open(&r.db).unwrap().query_row("PRAGMA journal_mode", [], |x| x.get(0)).unwrap();
    assert_eq!(mode, "wal", "Rails' adapter leaves its databases in WAL; Rust never changes it");
    // The web's own connection takes SQLite's write lock and holds it 1.5 s (inside both sides' 5 s busy timeout),
    // from before the engine's first write.
    let (held_tx, held) = tokio::sync::oneshot::channel();
    let app = r.app.clone();
    let web_write = tokio::spawn(async move {
        app.db(move |c| {
            let tx = model::immediate(c)?;
            tx.execute("UPDATE users SET updated_at = updated_at", [])?;
            let _ = held_tx.send(());
            std::thread::sleep(Duration::from_millis(1500));
            tx.commit()?;
            Ok(())
        }).await
    });
    let meter = Meter::start();
    held.await.unwrap();
    let stop = r.engine.stop_handle();
    let t0 = Instant::now();
    let driver = async {
        until("the tick to send its order", || !v.sent().is_empty()).await;
        stop.request(); // the tick in hand finishes, then the engine stops
    };
    let (ended, ()) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![]), driver);
    web_write.await.unwrap().unwrap();
    let longest = meter.stop().await;
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    assert!(t0.elapsed() >= Duration::from_millis(1200), "the engine waited for the web's write: {:?}", t0.elapsed());
    // I-1: the busy wait runs on the runtime thread; it is the one way past the bound.
    assert!(longest > RUNTIME_THREAD_BOUND, "the engine's busy wait held the thread {longest:?}");
    assert_eq!(status(&r.db, r.bot), BotStatus::Scheduled as i64, "and then ticked normally");
    assert_eq!(outstanding(&r.db, "OFAKE-1"), 1);
}

/// R2(c), ruled: a web start on a bot that is already working (a stale tab) is refused, as Rails' API refuses it
/// (`bot_already_running`, 409), not restarted as Rails' web controller does (a listed divergence). The start write
/// moves only a created or stopped bot, so it changes nothing; the handler then commits nothing, queues no tick, sends
/// no wake and re-renders with `engine.already_running`. Had it restarted the bot (a new anchor, no last action), this
/// pass would buy at once.
#[tokio::test(flavor = "current_thread")]
async fn a_web_start_on_a_working_bot_is_refused_and_changes_nothing() {
    let two_hours = 2 * 3600;
    let v = priced();
    // Working (scheduled), bought two hours ago: not due again for most of a week.
    let mut r = rig(v.clone(), BotSpec::weekly(60.0, &ago(two_hours + 30))
        .transient("last_action_job_at", json!(ruby::iso8601_ms(chrono::Utc::now() - chrono::Duration::seconds(two_hours))))).await;
    let before = bot_row(&r.db, r.bot);
    let (bot, now) = (r.bot, codec::format_time(chrono::Utc::now()));
    let moved = r.app.db(move |c| Ok(c.execute(WEB_START, rusqlite::params![now, bot])?)).await.unwrap();
    assert_eq!(moved, 0, "refused: the start write moves no working bot");
    assert_eq!(bot_row(&r.db, r.bot), before, "no column changed: status, started_at, transient_data, updated_at");
    run::step(&mut r.engine, &SystemClock).await.unwrap(); // a pass, as if anything had woken the engine
    assert!(v.sent().is_empty(), "no order was placed: the schedule was not re-anchored");
    assert_eq!(bot_row(&r.db, r.bot), before, "and the pass left the row as it was");
}

/// Codex round 1 (P2): the stop never resurrects a terminal bot. Rails' `Lifecycle#stop` returns early for an archived or
/// a deleted bot (`lifecycle.rb:91-98`), so a stale tab's stop changes no column of either.
#[tokio::test(flavor = "current_thread")]
async fn a_web_stop_leaves_an_archived_or_deleted_bot_as_it_is() {
    for terminal in [BotStatus::Archived, BotStatus::Deleted] {
        let r = rig(priced(), BotSpec { status: terminal as i64, ..BotSpec::weekly(60.0, "2099-01-01 00:00:00") }).await;
        let before = bot_row(&r.db, r.bot);
        let (bot, now) = (r.bot, codec::format_time(chrono::Utc::now()));
        let moved = r.app.db(move |c| Ok(c.execute(WEB_STOP, rusqlite::params![now, bot])?)).await.unwrap();
        assert_eq!(moved, 0, "{terminal:?}: the stop moves no terminal bot");
        assert_eq!(bot_row(&r.db, r.bot), before, "{terminal:?}: no column changed");
    }
}

/// A service that, once a stop is requested, holds its drain until `gate` is notified.
fn draining(stop: run::Shutdown, gate: Rc<Notify>) -> Service<'static> {
    Service { name: "mail", run: Box::pin(async move {
        stop.requested().await;
        gate.notified().await;
        Ok(())
    }) }
}

/// Codex round 1 (P1): the engine returns first and drops its own lock handle, but the install stays locked until every
/// service has drained (and, in `main.rs`, through the runtime's shutdown): no other process takes it over meanwhile.
#[tokio::test(flavor = "current_thread")]
async fn the_lock_is_held_until_every_service_has_drained() {
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let p = Paths::from_env(&|_| None, r._dir.path());
    let (stop, port, gate) = (r.engine.stop_handle(), r.port, Rc::new(Notify::new()));
    let service = draining(stop.clone(), gate.clone());
    let driver = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.request();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let engine_returned = up(port).await.is_none(); // the web stops when the engine returns
        let taken = lease::lock(&p, chrono::Utc::now()); // what a second process's `handback` would try
        let refused = matches!(taken, Err(lease::LeaseError::Locked));
        drop(taken);
        gate.notify_one();
        (engine_returned, refused)
    };
    let (ended, (engine_returned, refused)) =
        tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![service]), driver);
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    assert!(engine_returned, "the engine had returned; only the service was draining");
    assert!(refused, "another process could take the install while a service was still draining");
    assert!(lease::lock(&p, chrono::Utc::now()).is_ok(), "released once serve returned");
}

/// Codex rounds 1–2 (P1): once the engine has returned, no request reaches the app, even while a service still drains: a
/// socket the server accepted before then and that sends its first request now is answered 503 or closed, never by the
/// app. Deterministic: the server accepts in order, so once B (connected after A) is answered, A has been accepted; the
/// engine has returned once a fresh connection is refused.
#[tokio::test(flavor = "current_thread")]
async fn a_socket_accepted_before_the_engine_returned_gets_no_answer_from_the_app_after() {
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let (stop, port, gate) = (r.engine.stop_handle(), r.port, Rc::new(Notify::new()));
    let service = draining(stop.clone(), gate.clone());
    let driver = async {
        let (a, b) = tokio::task::spawn_blocking(move || {
            let a = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap(); // accepted, silent
            a.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut b = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            (a, w::keep_alive_get(&mut b, "/up"))
        }).await.unwrap();
        stop.request();
        while up(port).await.is_some() { tokio::task::yield_now().await; } // until the engine has returned
        let late = tokio::task::spawn_blocking(move || { let mut a = a; w::keep_alive_get(&mut a, "/up") }).await.unwrap();
        gate.notify_one(); // the service was draining all along
        (b, late)
    };
    let (ended, (b, late)) =
        tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![service]), driver);
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    assert!(b.as_deref().is_some_and(|x| x.starts_with("HTTP/1.1 200")), "{b:?}");
    assert!(late.as_deref().is_none_or(|x| x.starts_with("HTTP/1.1 503")), "the app answered after the engine returned: {late:?}");
}

/// Codex round 1 (P2): a service that fails while a requested stop drains still decides how the process ends (exit 2),
/// whether it fails while the engine finishes its tick in hand or after the engine has returned.
#[tokio::test(flavor = "current_thread")]
async fn a_service_failing_during_a_requested_stop_still_ends_the_process_non_zero() {
    // While the engine finishes its tick in hand (AddOrder's reply held).
    let gate = Rc::new(Notify::new());
    let v = priced().hold_add(gate.clone());
    let r = rig(v.clone(), BotSpec::weekly(60.0, &ago(1))).await;
    let stop = r.engine.stop_handle();
    let s = stop.clone();
    let failing = Service { name: "mail", run: Box::pin(async move {
        s.requested().await;
        Err::<(), String>("outbox unwritable".into())
    }) };
    let driver = async {
        until("the tick to send its order", || !v.sent().is_empty()).await;
        stop.request();
        tokio::time::sleep(Duration::from_millis(300)).await; // the service has failed; the tick is still in hand
        gate.notify_one();
    };
    let (ended, ()) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![failing]), driver);
    assert!(matches!(&ended, Ended::Service { name: "mail", error } if error == "outbox unwritable"), "before the engine returned: {ended:?}");

    // After the engine has returned.
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let stop = r.engine.stop_handle();
    let s = stop.clone();
    let failing = Service { name: "mail", run: Box::pin(async move {
        s.requested().await;
        tokio::time::sleep(Duration::from_millis(300)).await; // the engine returns meanwhile
        Err::<(), String>("outbox unwritable".into())
    }) };
    let driver = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.request();
    };
    let (ended, ()) = tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![failing]), driver);
    assert!(matches!(&ended, Ended::Service { name: "mail", error } if error == "outbox unwritable"), "after the engine returned: {ended:?}");
}

/// Codex round 2 (P2): the first failure decides. The engine fails first; a service that then fails while it drains is
/// logged, and `Ended` stays the engine's.
#[tokio::test(flavor = "current_thread")]
async fn an_engine_failure_stays_the_first_failure_when_a_service_then_fails_while_draining() {
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let (app, stop) = (r.app.clone(), r.engine.stop_handle());
    let failing = Service { name: "mail", run: Box::pin(async move {
        stop.requested().await; // requested by the supervisor only once the engine has failed
        Err::<(), String>("outbox unwritable".into())
    }) };
    let driver = async {
        // The engine's next pass cannot read the install.
        app.db(move |c| { c.execute_batch("ALTER TABLE rules RENAME TO rules_gone")?; Ok(()) }).await.unwrap();
        app.wake_engine();
    };
    let both = async { tokio::join!(supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![failing]), driver) };
    let (ended, ()) = tokio::time::timeout(Duration::from_secs(20), both).await.expect("the engine ended");
    assert!(matches!(ended, Ended::Engine(EngineError::Sqlite(_))), "{ended:?}");
}

/// Codex round 1 (P2): a service that returns `Ok(())` before any stop was requested ends the process too.
#[tokio::test(flavor = "current_thread")]
async fn a_service_returning_unasked_ends_the_process() {
    let r = rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")).await;
    let port = r.port;
    let quitter = Service { name: "sync", run: Box::pin(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok::<(), String>(())
    }) };
    let ended = tokio::time::timeout(Duration::from_secs(20), supervisor::serve(r.engine, Some((r.app, r.listener)), &SystemClock, vec![quitter]))
        .await.expect("ended");
    assert!(matches!(&ended, Ended::Service { name: "sync", error } if error == "returned before a stop was requested"), "{ended:?}");
    assert!(up(port).await.is_none(), "the web stopped with it");
}

/// Codex round 1 (P2): the meter catches a hold at the very end of what it measures, with nothing awaited after it; so
/// a pass whose last synchronous stretch exceeds the bound fails `a_full_tick_never_holds_the_runtime_thread_longer_than_the_bound`.
#[tokio::test(flavor = "current_thread")]
async fn the_meter_catches_a_hold_at_the_very_end_of_what_it_measures() {
    let stall = RUNTIME_THREAD_BOUND + Duration::from_millis(50);
    let meter = Meter::start();
    tokio::time::sleep(Duration::from_millis(20)).await; // the meter is running
    std::thread::sleep(stall); // the last stretch, as at the end of a pass
    let held = meter.stop().await;
    assert!(held >= stall, "an injected {stall:?} final stall measured as {held:?}");
    assert!(held >= RUNTIME_THREAD_BOUND, "so the bound test fails on it");
}

/// What `run` and `serve` do once the supervisor returns (`main.rs`): `rt.shutdown_timeout(5 s)`. A blocking unit still
/// running then (a service's database call abandoned at the stop, a request's) delays the exit by at most those 5 s;
/// an ordinary drop of the runtime would wait for it to finish.
#[test]
fn a_blocking_unit_left_running_cannot_hold_the_process_past_the_runtime_shutdown() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let r = rt.block_on(rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")));
    let stop = r.engine.stop_handle();
    let s = stop.clone();
    let service = Service { name: "sync", run: Box::pin(async move {
        let unit = tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_secs(30)));
        tokio::select! { _ = unit => {}, _ = s.requested() => {} } // abandoned at the stop; its thread runs on
        Ok(())
    }) };
    let driver = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.request();
    };
    let (ended, ()) = rt.block_on(async { tokio::join!(supervisor::serve(r.engine, None, &SystemClock, vec![service]), driver) });
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    let t0 = Instant::now();
    rt.shutdown_timeout(Duration::from_secs(5));
    assert!(t0.elapsed() < Duration::from_secs(7), "the 30 s unit held the exit {:?}", t0.elapsed());
}

/// An upgraded /cable WebSocket has left its hyper connection, so the server's close does not reach it. Once the engine
/// has returned, `main.rs` no longer drives the runtime: the socket hears nothing more (not even a ping), and the
/// runtime's shutdown drops it at once, without waiting out the 5 s.
#[test]
fn a_websocket_hears_nothing_once_the_engine_has_returned_and_is_dropped_at_the_runtime_shutdown() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    const HASH: &str = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let Rig { _dir: _keep, engine, app, listener, port, db, s, .. } = rt.block_on(rig(priced(), BotSpec::weekly(60.0, "2099-01-01 00:00:00")));
    rusqlite::Connection::open(&db).unwrap()
        .execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", rusqlite::params![HASH, s.user_id]).unwrap();
    let app = app.with_cable_timing(Duration::from_millis(50), Duration::from_secs(60)).unwrap(); // a ping every 50 ms
    let session = deltabadger::web::session::SessionData { user: Some((s.user_id, HASH[..29].to_string())), ..Default::default() };
    let cookie = deltabadger::web::session::seal(&app.keys.session, &session, app.now());
    let (live_tx, live) = tokio::sync::oneshot::channel();
    let browser = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let mut request = format!("ws://127.0.0.1:{port}/cable").into_client_request().unwrap();
            request.headers_mut().insert("sec-websocket-protocol", "actioncable-v1-json".parse().unwrap());
            request.headers_mut().insert("origin", format!("http://127.0.0.1:{port}").parse().unwrap());
            request.headers_mut().insert("cookie", format!("_deltabadger_rust_session={cookie}").parse().unwrap());
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            for _ in 0..2 { socket.next().await.unwrap().unwrap(); } // the welcome and a ping: live
            live_tx.send(()).unwrap();
            let mut heard = Vec::new();
            let listening = async {
                loop {
                    match socket.next().await {
                        Some(Ok(Message::Text(_))) => heard.push(Instant::now()),
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => return Instant::now(),
                    }
                }
            };
            let gone = tokio::time::timeout(Duration::from_secs(20), listening).await.expect("the socket ended within 20 s");
            (heard, gone)
        })
    });
    let stop = engine.stop_handle();
    let driver = async {
        live.await.unwrap();
        stop.request(); // what the SIGTERM handler does; this future ends here, so block_on returns with the engine
    };
    let (ended, ()) = rt.block_on(async { tokio::join!(supervisor::serve(engine, Some((app, listener)), &SystemClock, vec![]), driver) });
    let returned = Instant::now();
    assert!(matches!(ended, Ended::Stopped), "{ended:?}");
    std::thread::sleep(Duration::from_millis(500)); // ten pings' time with nothing driving the runtime
    let shutdown = Instant::now();
    rt.shutdown_timeout(Duration::from_secs(5));
    let (heard, gone) = browser.join().unwrap();
    let late: Vec<_> = heard.iter().filter(|t| **t > returned + Duration::from_millis(100)).collect();
    assert!(late.is_empty(), "{} message(s) after the engine returned", late.len());
    assert!(gone >= shutdown, "open until the runtime's shutdown");
    assert!(gone < shutdown + Duration::from_secs(1), "dropped at the shutdown, not lingering: {:?}", gone - shutdown);
}
