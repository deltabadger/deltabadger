//! Figures are made ahead of any page: the queue of work and its consumer, the fill that keeps what it rendered, expiry
//! at consumption, the bounded market fetch, the builder's own connection, and what wakes the warming. A page only reads
//! what was kept, or shows Rails' loading state and leaves the work to the queue.
//! Wall-clock timing tests here are `#[ignore = "timing: nightly"]`, out of the PR gate; the nightly workflow runs them.
//! By hand: `cargo nextest run --profile timing --run-ignored only` (rust/.config/nextest.toml lists them).
mod common;
use deltabadger::figures::budget;
use deltabadger::web::figure::loading::{self, Source};
use deltabadger::web::figure::service::Work;
use deltabadger::web::{App, Config};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const AT: &str = "2026-03-18T12:01:00Z";

// Wall-clock bounds hold for a release build. A debug build on a busy CI runner served a page in 66 ms beside a CPU-heavy
// fill, so debug gets four times the room: still far below the multi-second work the pages must not wait on, which each
// test also measures (`FILL_OVER_PAGE`).
const ROOM: u32 = if cfg!(debug_assertions) { 4 } else { 1 };
/// A page's p95 beside background figures work, and a commit beside a builder statement.
const PAGE_BOUND: Duration = Duration::from_millis(50 * ROOM as u64);
/// How many times longer than the pages' p95 the background work (at least the window the pages were measured in) must
/// take, for the test to say anything.
const FILL_OVER_PAGE: u32 = 10;
/// How long pages are measured beside a fill, unless it ends first: twice what a page within `PAGE_BOUND` needs the
/// fill to outlast, so a fill that is still running clears `FILL_OVER_PAGE` whatever the runner's speed.
const FILL_WINDOW: Duration = Duration::from_millis(2 * FILL_OVER_PAGE as u64 * 50 * ROOM as u64);

/// Daily bars of AAA up to the day before `AT`.
fn bars() -> Value {
    let start = common::web::at("2026-03-02T00:00:00Z");
    Value::Array((0..16).map(|i| {
        let t = (start + chrono::Duration::days(i)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        json!({"t": t, "o": 100 + i, "h": 100 + i, "l": 100 + i, "c": 100 + i, "v": 1000})
    }).collect())
}

/// The market every account here reads: AAA's price and its bars, each answer `delay_ms` away.
fn script(delay_ms: u64) -> Value {
    let bars = json!({"delay_ms": delay_ms, "body": {"bars": bars()}});
    json!({
        "GET data.alpaca.markets/v2/stocks/snapshots": {"delay_ms": delay_ms, "body": {"AAA": {"latestTrade": {"p": 110.0}}}},
        "GET data.alpaca.markets/v2/stocks/AAA/bars": bars.clone(),
        "GET data.alpaca.markets/v2/stocks/AAA/bars?adjustment=split": bars,
    })
}

/// An install with `users` accounts, each with a paper Alpaca key and a basket of AAA bought once.
struct Install { _dir: tempfile::TempDir, file: PathBuf, clock: Arc<common::web::TestClock>, app: App }
impl Install {
    fn new(users: i64, delay_ms: u64) -> Install { Install::with_script(users, script(delay_ms)) }
    fn with_script(users: i64, script: Value) -> Install {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("production.sqlite3");
        let c = rusqlite::Connection::open(&file).unwrap();
        c.execute_batch(include_str!("fixtures/settings_primary_schema.sql")).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        let t = "'2026-01-01'";
        c.execute_batch(&format!("
            INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (1,'Exchanges::Alpaca','Alpaca',{t},{t});
            INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (1,'Fiat','usd','USD','US Dollar',{t},{t}),(2,'Stock','aaa','AAA','AAA Inc',{t},{t});
            INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at)
                VALUES (1,'AAA',2,9,1,0,1,2,'USD',1,2,'AAA',{t},{t});
        ")).unwrap();
        let clock = common::web::TestClock::at(AT);
        let env = common::web::env(common::web::SECRET);
        let app = App::new(Config::from_env(&env).unwrap(), &env, rusqlite::Connection::open(&file).unwrap(), clock.clone()).unwrap()
            .with_figure_source(Source::Script(script)).unwrap();
        for user in 1..=users { add_account(&c, &app, user); }
        Install { _dir: dir, file, clock, app }
    }
    fn connection(&self) -> rusqlite::Connection { rusqlite::Connection::open(&self.file).unwrap() }
    /// The `figures` service of `serve`, with no engine: what consumes the queue.
    fn consume(&self) -> tokio::sync::watch::Sender<bool> {
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        let (stop, stopped) = tokio::sync::watch::channel(false);
        std::mem::forget(events);
        tokio::spawn(loading::follow(self.app.clone(), rx, stopped));
        stop
    }
    fn now(&self) -> i64 { self.clock_now().timestamp() }
    fn clock_now(&self) -> chrono::DateTime<chrono::Utc> { deltabadger::engine::Clock::now(&*self.clock) }
}

fn add_account(c: &rusqlite::Connection, app: &App, user: i64) {
    let t = "'2026-01-01'";
    c.execute(&format!("INSERT INTO users(id,email,encrypted_password,setup_completed,admin,confirmed_at,created_at,updated_at) VALUES (?1,'u'||?1||'@example.com','x',1,?1=1,{t},{t},{t})"), [user]).unwrap();
    c.execute(&format!("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,passphrase,created_at,updated_at) VALUES (?1,1,0,1,?2,?3,?4,{t},{t})"),
              rusqlite::params![user, app.cipher.encrypt("key"), app.cipher.encrypt("secret"), app.cipher.encrypt("paper")]).unwrap();
    c.execute(&format!("INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) \
                        VALUES (?1,?1,1,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":1,\"allocations\":{{\"2\":1.0}}}}','{{}}',{t},{t})"), [user]).unwrap();
    c.execute(&format!("INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
                        VALUES (?1,1,'AAA',2,'USD',1,1,1,100,100,0,0,2,'o'||?1,'REGULAR','2026-03-02 14:30:00',{t})"), [user]).unwrap();
}

/// A user's own key: what a publication renders.
fn own() -> BTreeSet<(String, String)> { BTreeSet::from([("en".to_string(), String::new())]) }

/// Until `done` holds, or panics after `secs`.
async fn until(secs: u64, what: &str, mut done: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done().await {
        assert!(Instant::now() < deadline, "{what}: not within {secs} s");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether a publication of the user's figures went out (the account's latest, as a reconnecting page is sent it).
async fn published(app: &App, user: i64) -> bool { loading::latest(app, user, &format!("user_{user}:bot_updates")).await.is_some_and(|kept| !kept.is_empty()) }

/// What a page in `locale` under `prefix` would show now, and the figure arithmetic it took (budget steps).
async fn page_figures(app: &App, user: i64, locale: &'static str, prefix: &'static str) -> (Option<Value>, u64) {
    let snapshot = loading::prepare(app, user).await.unwrap();
    let clock = app.clock.clone();
    app.db(move |c| { let (out, used) = budget::scope(budget::FIGURE, || loading::render(c, user, &snapshot, locale, "token", prefix, &*clock)); Ok((out?, used.steps)) }).await.unwrap()
}

/// How many fills this account began (the serial of its last).
fn fills(app: &App, user: i64) -> u64 { app.figure_service.settled(user) }

// ---- Task 2: the queue, its consumer, and work that arrives during a fill -------------------------------------------

#[tokio::test]
async fn boot_publishes_without_any_page_or_cable_demand() {
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the boot publication", async || published(&install.app, 1).await).await;
}

#[tokio::test]
async fn idle_locale_miss_on_a_ready_entry_renders_without_a_fill() {
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the English figures", async || published(&install.app, 1).await).await;
    let before = fills(&install.app, 1);
    install.app.figure_service.enqueue(1, Work { publish: false, locales: BTreeSet::from([("fr".to_string(), "/fr".to_string())]), not_before: install.now() });
    until(10, "the French figures, kept", async || matches!(page_figures(&install.app, 1, "fr", "/fr").await, (Some(_), 0))).await;
    assert_eq!(fills(&install.app, 1), before, "a locale of a ready entry is rendered from its cache: no fill, no market request");
}

#[tokio::test]
async fn second_locale_arriving_after_the_fill_captured_its_work_is_rendered() {
    let install = Install::new(1, 300);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (shown, _) = page_figures(&install.app, 1, "fr", "/fr").await;
    assert!(shown.is_none(), "mid-fill, the French page shows the loading state");
    let pages = install.app.clone();
    loading::enqueue_page(&pages, 1, "fr", "/fr");
    until(10, "the English publication", async || published(&install.app, 1).await).await;
    until(10, "the French figures, kept", async || matches!(page_figures(&install.app, 1, "fr", "/fr").await, (Some(_), 0))).await;
    assert_eq!(fills(&install.app, 1), 1, "one fill: the French work waited for it and was rendered from its cache");
}

#[tokio::test]
async fn publication_requested_after_capture_is_delivered() {
    let install = Install::new(1, 300);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: false, locales: own(), not_before: install.now() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A cable subscribes mid-fill and asks for the publication.
    assert!(loading::resubscribed(&install.app, 1, "user_1:bot_updates").await.is_empty());
    until(10, "the publication the cable asked for", async || published(&install.app, 1).await).await;
    assert_eq!(fills(&install.app, 1), 1);
}

#[tokio::test]
async fn three_users_with_slow_fills_all_publish() {
    let install = Install::new(3, 300);
    let _stop = install.consume();
    // The clock moves: work turned away while both fills run is tried again a second later (`Load::Busy`).
    let clock = install.clock.clone();
    tokio::spawn(async move { loop { tokio::time::sleep(Duration::from_millis(200)).await; clock.set(deltabadger::engine::Clock::now(&*clock) + chrono::Duration::seconds(1)); } });
    for user in 1..=3 { install.app.figure_service.enqueue(user, Work { publish: true, locales: own(), not_before: install.now() }); }
    for user in 1..=3 { until(15, &format!("user {user}'s publication"), async || published(&install.app, user).await).await; }
}

#[tokio::test]
async fn debounce_coalesces_job_wakes() {
    let install = Install::new(1, 0);
    let _stop = install.consume();
    let due = install.now() + 5;
    for _ in 0..3 { install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: due }); }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(fills(&install.app, 1), 0, "nothing runs before its debounce");
    install.clock.set(install.clock_now() + chrono::Duration::seconds(5));
    until(10, "the debounced publication", async || published(&install.app, 1).await).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fills(&install.app, 1), 1, "three wakes, one fill");
}

// ---- Task 3: a page never computes figures; the fill keeps what it rendered ------------------------------------------

/// Reads of the tables only a figure computation reads, on the connection that renders a page.
fn figure_reads(c: &rusqlite::Connection, reads: Arc<std::sync::atomic::AtomicUsize>) {
    use rusqlite::hooks::{AuthAction, Authorization};
    c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
        if let AuthAction::Read { table_name, .. } = context.action {
            if ["transactions", "tickers", "bot_index_assets", "account_transactions", "wash_sale_locks", "assets"].contains(&table_name) { reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
        }
        Authorization::Allow
    }));
}

#[tokio::test]
async fn render_runs_no_figure_computation() {
    use deltabadger::web::figure::loading::Snapshot;
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the figures", async || published(&install.app, 1).await).await;
    let ready = loading::prepare(&install.app, 1).await.unwrap();
    let Snapshot::Ready(cache, at, revision, mark, until_at) = ready.clone() else { panic!("not ready") };
    let mut unkept = cache.clone();
    unkept.set_stamp(at);
    // (case, snapshot, locale, prefix, whether the page's connection has seen no write since `prepare`)
    let mut cases = vec![
        ("ready, kept", ready.clone(), "en", "", true),
        ("ready, nothing kept", Snapshot::Ready(unkept, at, revision, mark, until_at), "en", "", true),
        ("ready, another locale", ready.clone(), "de", "/de", true),
        ("failed", Snapshot::Failed(None), "en", "", true),
        // Written since `prepare`: the fingerprint is read again, and only it.
        ("stale", Snapshot::Ready(cache.clone(), at, revision.wrapping_add(1), None, until_at), "en", "", false),
    ];
    install.clock.set(install.clock_now() + chrono::Duration::minutes(5));
    cases.push(("expired", loading::prepare(&install.app, 1).await.unwrap(), "en", "", true));
    for (case, snapshot, locale, prefix, unchanged) in cases {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = reads.clone();
        let clock = install.app.clock.clone();
        let (shown, steps) = install.app.db(move |c| {
            figure_reads(c, counted);
            let (out, used) = budget::scope(budget::FIGURE, || loading::render(c, 1, &snapshot, locale, "token", prefix, &*clock));
            c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
            Ok((out?, used.steps))
        }).await.unwrap();
        // Written since `prepare`, the fingerprint is read again: whatever it costs, and nothing more.
        let fingerprint = if unchanged { 0 } else { install.app.db(|c| Ok(budget::scope(budget::FIGURE, || loading::revision(c, 1)).1.steps)).await.unwrap() };
        assert_eq!(steps, fingerprint, "{case}: a page computed figures");
        if unchanged { assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 0, "{case}: a page read what only a computation reads"); }
        if case.starts_with("ready, n") || case == "ready, another locale" || case == "expired" { assert!(shown.is_none(), "{case}: Rails' loading state"); }
    }
}

// ---- Task 4: kept figures expire where they are consumed --------------------------------------------------------------

/// Ready figures made at `made`, then the snapshot a page prepared at `prepared` rendered at `rendered`.
async fn shown_after(install: &Install, made: &str, prepared: &str, rendered: &str) -> Option<Value> {
    install.clock.set(common::web::at(made));
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the figures", async || published(&install.app, 1).await).await;
    install.clock.set(common::web::at(prepared));
    let snapshot = loading::prepare(&install.app, 1).await.unwrap();
    assert!(matches!(snapshot, loading::Snapshot::Ready(..)), "ready at {prepared}");
    install.clock.set(common::web::at(rendered));
    let app = install.app.clone();
    install.app.db(move |c| loading::render(c, 1, &snapshot, "en", "token", "", &*app.clock)).await.unwrap()
}

#[tokio::test]
async fn prepare_then_render_after_the_deadline_is_cold() {
    let at_cut = Install::new(1, 0);
    assert!(shown_after(&at_cut, "2026-03-18T12:04:59Z", "2026-03-18T12:04:59Z", "2026-03-18T12:04:59.999Z").await.is_some(), "before the cut");
    let at_cut = Install::new(1, 0);
    assert!(shown_after(&at_cut, "2026-03-18T12:04:59Z", "2026-03-18T12:04:59Z", "2026-03-18T12:05:00Z").await.is_none(), "the five-minute cut");
    let at_midnight = Install::new(1, 0);
    assert!(shown_after(&at_midnight, "2026-03-18T23:59:59Z", "2026-03-18T23:59:59Z", "2026-03-19T00:00:00Z").await.is_none(), "midnight, when the day counts change");
    let locked = Install::new(1, 0);
    let c = locked.connection();
    c.execute("UPDATE users SET wash_sale_enabled=1, wash_sale_jurisdiction='US' WHERE id=1", []).unwrap();
    c.execute("INSERT INTO wash_sale_locks(user_id,asset_id,buy_locked_until,source,created_at,updated_at) VALUES (1,2,'2026-03-18 12:02:00','ledger','2026-01-01','2026-01-01')", []).unwrap();
    assert!(shown_after(&locked, "2026-03-18T12:01:59Z", "2026-03-18T12:01:59Z", "2026-03-18T12:02:00Z").await.is_none(), "a wash-sale lock ends");
}

/// Kept figures are served while `t_r` < `until` (the next cut, or a lock's end before it), with the figures of the fill
/// (`t_c`). From `until` on a page shows the loading state and the queue fills again: the publication carries the new
/// `t_c`, never the old one past its lifetime.
#[tokio::test]
async fn kept_output_lifetime_follows_the_cut_and_lock_deadline() {
    use deltabadger::web::figure::loading::Snapshot;
    for (lock, until_at) in [(None, "2026-03-18T12:05:00Z"), (Some("2026-03-18 12:03:00"), "2026-03-18T12:03:00Z")] {
        let install = Install::new(1, 0);
        if let Some(lock) = lock {
            let c = install.connection();
            c.execute("UPDATE users SET wash_sale_enabled=1, wash_sale_jurisdiction='GB' WHERE id=1", []).unwrap();
            c.execute("INSERT INTO wash_sale_locks(user_id,asset_id,buy_locked_until,source,created_at,updated_at) VALUES (1,2,?1,'ledger','2026-01-01','2026-01-01')", [lock]).unwrap();
        }
        let _stop = install.consume();
        install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
        until(10, "the figures", async || published(&install.app, 1).await).await;
        let t_c = common::web::at(AT);
        let until_at = common::web::at(until_at);
        for t_r in [t_c + chrono::Duration::seconds(30), until_at - chrono::Duration::milliseconds(1)] {
            install.clock.set(t_r);
            let snapshot = loading::prepare(&install.app, 1).await.unwrap();
            let Snapshot::Ready(_, at, ..) = &snapshot else { panic!("{t_r}: kept figures are served") };
            assert_eq!(at.utc(), t_c, "{t_r}: the figures of the fill, read later");
            assert!(page_figures(&install.app, 1, "en", "").await.0.is_some(), "{t_r}");
        }
        install.clock.set(until_at);
        let before = fills(&install.app, 1);
        let snapshot = loading::prepare(&install.app, 1).await.unwrap();
        let app = install.app.clone();
        let shown = install.app.db(move |c| loading::render_page(&app, c, 1, &snapshot, "en", "token", "")).await.unwrap();
        assert!(shown.is_none(), "{until_at}: Rails' loading state, never the old figures");
        until(10, "the next fill's figures", async || {
            matches!(loading::prepare(&install.app, 1).await.unwrap(), Snapshot::Ready(_, at, ..) if at.utc() == until_at)
        }).await;
        assert!(fills(&install.app, 1) > before, "{until_at}: a fill ran");
        until(10, "the republication", async || published(&install.app, 1).await).await;
    }
}

// ---- Task 5: a bounded concurrent fetch, CPU work off the runtime's thread, and a timed-out fill's bound ------------

/// An install with one account whose basket holds `members` assets (S1, S2, ...), each bought `buys` times, with `bars`
/// daily bars each (from `start`, `step` seconds apart), every market answer `delay_ms` away.
fn members_install(members: usize, buys: usize, bars: usize, start: &str, step: i64, delay_ms: u64) -> Install {
    let start = common::web::at(start);
    let mut script = serde_json::Map::new();
    let mut prices = serde_json::Map::new();
    for i in 1..=members {
        let symbol = format!("S{i}");
        let series: Vec<Value> = (0..bars).map(|b| {
            let t = (start + chrono::Duration::seconds(step * b as i64)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
            json!({"t": t, "o": 50 + (b % 7) as i64, "h": 51, "l": 49, "c": 50, "v": 1000})
        }).collect();
        // As a venue sends it: text, which the scripted transport hands over as it is.
        let reply = json!({"delay_ms": delay_ms, "body": json!({"bars": series}).to_string()});
        script.insert(format!("GET data.alpaca.markets/v2/stocks/{symbol}/bars"), reply.clone());
        script.insert(format!("GET data.alpaca.markets/v2/stocks/{symbol}/bars?adjustment=split"), reply);
        prices.insert(symbol, json!({"latestTrade": {"p": 55.0}}));
    }
    script.insert("GET data.alpaca.markets/v2/stocks/snapshots".into(), json!({"delay_ms": delay_ms, "body": prices}));
    let install = Install::with_script(0, Value::Object(script));
    let c = install.connection();
    let t = "'2026-01-01'";
    let mut allocations = serde_json::Map::new();
    for i in 1..=members {
        let id = 100 + i as i64;
        c.execute(&format!("INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (?1,'Stock','s'||?1,'S{i}','S{i} Inc',{t},{t})"), [id]).unwrap();
        c.execute(&format!("INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at) \
                            VALUES (?1,'S{i}',?1,9,1,0,1,2,'USD',1,2,'S{i}',{t},{t})"), [id]).unwrap();
        allocations.insert(id.to_string(), json!(1.0 / members as f64));
    }
    c.execute(&format!("INSERT INTO users(id,email,encrypted_password,setup_completed,admin,confirmed_at,created_at,updated_at) VALUES (1,'u1@example.com','x',1,1,{t},{t},{t})"), []).unwrap();
    c.execute(&format!("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,passphrase,created_at,updated_at) VALUES (1,1,0,1,?1,?2,?3,{t},{t})"),
              rusqlite::params![install.app.cipher.encrypt("key"), install.app.cipher.encrypt("secret"), install.app.cipher.encrypt("paper")]).unwrap();
    c.execute(&format!("INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES (1,1,1,'Bots::DcaMultiAsset',1,?1,'{{}}',{t},{t})"),
              [json!({"quote_asset_id": 1, "allocations": allocations}).to_string()]).unwrap();
    let first = common::web::at("2026-03-02T14:30:00Z");
    for b in 0..buys {
        let id = 101 + (b % members) as i64;
        let at = (first + chrono::Duration::seconds(3600 * b as i64)).format("%Y-%m-%d %H:%M:%S").to_string();
        c.execute(&format!("INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
                            VALUES (1,1,'S'||(?1-100),?1,'USD',1,0.1,0.1,50,5,0,0,2,'o'||?2,'REGULAR',?3,{t})"), rusqlite::params![id, b as i64, at]).unwrap();
    }
    install
}

/// A signed-in browser for account 1.
fn browser(install: &Install) -> common::web::Browser {
    use deltabadger::web::session::{self, SessionData};
    let signed = SessionData { user: Some((1, "x".into())), ..SessionData::default() };
    common::web::Browser { cookie: Some(session::seal(&install.app.keys.session, &signed, install.clock_now())), page: None }
}

/// `n` page loads of `path`, one due every `gap`, each one's latency from when it was due: as a request that arrives on
/// a socket waits for the runtime's thread, whatever holds it.
async fn latencies(install: &Install, path: &str, n: usize, gap: Duration) -> Vec<Duration> {
    let mut browser = browser(install);
    let mut out = vec![];
    let start = tokio::time::Instant::now();
    for i in 0..n {
        let due = start + gap * i as u32;
        tokio::time::sleep_until(due).await;
        let page = browser.get(&install.app, path).await;
        out.push(due.elapsed());
        assert_eq!(page.status, 200, "{path}: {:?} {}", page.headers, page.body);
    }
    out
}
fn p95(mut times: Vec<Duration>) -> Duration { times.sort(); times[((times.len() as f64 - 1.0) * 0.95).round() as usize] }

/// Page loads of `path`, one due every `gap`, while account 1's fill runs, for up to `FILL_WINDOW`; and how long the fill
/// has run since `started`, to within one `gap`. Every page measured ran beside the fill.
async fn latencies_while_filling(install: &Install, path: &str, gap: Duration, started: Instant) -> (Vec<Duration>, Duration) {
    until(5, "the fill to start", async || install.app.figure_service.filling(1)).await;
    let mut browser = browser(install);
    let mut out = vec![];
    let mut due = tokio::time::Instant::now();
    while install.app.figure_service.filling(1) && started.elapsed() < FILL_WINDOW {
        let page = browser.get(&install.app, path).await;
        out.push(due.elapsed());
        assert_eq!(page.status, 200, "{path}: {:?} {}", page.headers, page.body);
        due += gap;
        tokio::time::sleep_until(due).await;
    }
    (out, started.elapsed())
}

/// A transport that answers every request `delay` later and counts how many are in flight at once.
struct Counting { delay: Duration, now: std::sync::atomic::AtomicUsize, most: std::sync::atomic::AtomicUsize }
impl deltabadger::venue::http::Transport for Counting {
    async fn send(&self, _: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
        use std::sync::atomic::Ordering::SeqCst;
        let now = self.now.fetch_add(1, SeqCst) + 1;
        self.most.fetch_max(now, SeqCst);
        tokio::time::sleep(self.delay).await;
        self.now.fetch_sub(1, SeqCst);
        Ok(deltabadger::venue::http::HttpResponse { status: 200, body: json!({"bars": bars()}).to_string() })
    }
}

#[tokio::test]
async fn boot_fetch_is_bounded_to_eight_in_flight() {
    use deltabadger::venue::http::HttpRequest;
    // The cache's fetch: never more than eight requests at once, and eight while there are that many.
    let counting = Counting { delay: Duration::from_millis(100), now: Default::default(), most: Default::default() };
    let demands: Vec<HttpRequest> = (1..=67).map(|i| HttpRequest { method: "GET", base: "https://data.alpaca.markets".into(), path: format!("/v2/stocks/S{i}/bars"),
        query: vec![("limit", "10000".into()), ("start", "2026-03-02T00:00:00Z".into()), ("timeframe", "1Day".into())], body: None, not_after: None }).collect();
    let mut cache = deltabadger::figures::page_market::Cache::default();
    let t = Instant::now();
    cache.fill_bounded(&counting, demands, common::web::at(AT).timestamp(), &Arc::new(tokio::sync::Semaphore::new(1))).await;
    assert_eq!(counting.most.load(std::sync::atomic::Ordering::SeqCst), 8, "requests in flight at once");
    assert!(t.elapsed() < Duration::from_millis(67 * 100 / 2), "67 answers 100 ms away took {:?}", t.elapsed());
    // An account of 65 holdings after a restart: its first publication within six seconds, at 100 ms an answer.
    let install = members_install(65, 65, 16, "2026-03-02T00:00:00Z", 86_400, 100);
    let _stop = install.consume();
    let t = Instant::now();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(20, "the first publication", async || published(&install.app, 1).await).await;
    // Six seconds for a release build; the fetch's own bound is the eight in flight above.
    assert!(t.elapsed() <= Duration::from_secs(6) * ROOM, "first publication after {:?}", t.elapsed());
}

#[tokio::test]
#[ignore = "timing: nightly"]
async fn pages_stay_responsive_during_large_market_responses() {
    // 32 holdings, each answered with about a megabyte of bars, 100 ms away.
    let install = members_install(32, 32, 13_000, "2026-03-12T00:00:00Z", 30, 100);
    let idle = p95(latencies(&install, "/settings/connect", 10, Duration::from_millis(5)).await);
    let _stop = install.consume();
    let started = Instant::now();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    let (during, fill) = latencies_while_filling(&install, "/settings/connect", Duration::from_millis(25), started).await;
    assert!(during.len() >= 20, "{} pages measured while the fill ran ({fill:?})", during.len());
    let during = p95(during);
    assert!(during < PAGE_BOUND, "p95 {during:?} during the fill ({idle:?} idle)");
    assert!(fill > during * FILL_OVER_PAGE, "the fill ({fill:?}) was no longer than the pages beside it (p95 {during:?})");
}

#[tokio::test]
#[ignore = "timing: nightly"]
async fn pages_stay_responsive_during_a_cpu_heavy_fill() {
    // 60 holdings, 3,000 orders, two years of daily bars each: a computation of seconds.
    let install = members_install(60, 3_000, 730, "2024-03-18T00:00:00Z", 86_400, 0);
    let _stop = install.consume();
    let started = Instant::now();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    let (during, fill) = latencies_while_filling(&install, "/settings/connect", Duration::from_millis(25), started).await;
    assert!(during.len() >= 20, "{} pages measured while the fill ran ({fill:?})", during.len());
    let during = p95(during);
    assert!(during < PAGE_BOUND, "p95 {during:?} during the fill");
    assert!(fill > during * FILL_OVER_PAGE, "the fill ({fill:?}) was no longer than the pages beside it (p95 {during:?})");
}

#[tokio::test]
async fn a_timed_out_fill_keeps_the_compute_bound() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let install = Install::new(1, 0);
    install.app.figure_service.set_fill_timeout(Duration::from_secs(1));
    // Each figures computation on the builder's connection is a worker: the first one blocks for three seconds.
    let (inside, most, entered) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(std::sync::Mutex::new(Vec::<Instant>::new())));
    {
        let (inside, most, entered) = (inside.clone(), most.clone(), entered.clone());
        install.app.figure_service.on_builder(move |c| {
            c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                if matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "transactions", .. }) {
                    let first = { let mut entered = entered.lock().unwrap(); entered.push(Instant::now()); entered.len() == 1 };
                    let now = inside.fetch_add(1, SeqCst) + 1;
                    most.fetch_max(now, SeqCst);
                    if first { std::thread::sleep(Duration::from_secs(3)); }
                    inside.fetch_sub(1, SeqCst);
                }
                rusqlite::hooks::Authorization::Allow
            }));
        });
    }
    let _stop = install.consume();
    let started = Instant::now();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(5, "the first fill's time-out", async || matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Failed(_))).await;
    assert!(started.elapsed() < Duration::from_secs(3), "the fill timed out at one second, not when its worker returned");
    // Its failure has lapsed: the retry starts while the first worker still blocks, and waits for it (with time to).
    install.app.figure_service.set_fill_timeout(Duration::from_secs(10));
    install.clock.set(install.clock_now() + chrono::Duration::seconds(61));
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(15, "the retry's publication", async || published(&install.app, 1).await).await;
    let entered = entered.lock().unwrap().clone();
    assert!(entered.len() >= 2, "the retry computed");
    assert!(entered[1].duration_since(entered[0]) >= Duration::from_secs(3), "the retry's worker started only once the first returned");
    assert_eq!(most.load(SeqCst), 1, "one worker at a time");
}

// ---- Task 6: the builder under WAL, and the read lock of its order reads -----------------------------------------------

#[tokio::test]
#[ignore = "timing: nightly"]
async fn wal_slow_builder_statement_blocks_neither_writer_nor_web() {
    let install = Install::new(1, 0);
    // A builder statement slowed by a function of its own: a millisecond a row, two seconds in all.
    install.app.figure_service.on_builder(|c| {
        c.create_scalar_function("slowly", 1, rusqlite::functions::FunctionFlags::SQLITE_UTF8, |ctx| { std::thread::sleep(Duration::from_millis(1)); ctx.get::<i64>(0) }).unwrap();
    });
    install.app.figure_service.set_builder_path(install.file.to_str().unwrap().to_string());
    let service = install.app.figure_service.clone();
    let slow = tokio::task::spawn_blocking(move || service.with_builder(|c| Ok(c.query_row("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<2000) SELECT sum(slowly(i)) FROM n, transactions", [], |r| r.get::<_, i64>(0))?)));
    tokio::time::sleep(Duration::from_millis(200)).await;
    // A writer commits while it runs ...
    let file = install.file.clone();
    let commit = tokio::task::spawn_blocking(move || {
        let writer = rusqlite::Connection::open(&file).unwrap();
        let t = Instant::now();
        writer.execute("INSERT INTO app_configs(key,value,created_at,updated_at) VALUES ('wal-check','{}','2026-01-01','2026-01-01')", []).unwrap();
        t.elapsed()
    }).await.unwrap();
    // ... and pages are served.
    let pages = p95(latencies(&install, "/settings/connect", 20, Duration::from_millis(25)).await);
    assert!(!slow.is_finished(), "the slow statement ran throughout");
    assert_eq!(slow.await.unwrap().unwrap(), 2000 * 2001 / 2);
    assert!(commit < PAGE_BOUND, "a commit took {commit:?} beside the builder's statement");
    assert!(pages < PAGE_BOUND, "pages p95 {pages:?} beside the builder's statement");
    assert!(Duration::from_secs(2) > pages * FILL_OVER_PAGE && Duration::from_secs(2) > commit * FILL_OVER_PAGE, "the builder's statement (2 s) was no longer than what ran beside it");
}

#[tokio::test]
async fn non_wal_logs_once() {
    let install = Install::new(1, 0);
    // The same install in a rollback journal, which no other connection holds open.
    let rollback = install.file.with_file_name("rollback.sqlite3");
    install.connection().execute("VACUUM INTO ?1", [rollback.to_str().unwrap()]).unwrap();
    assert_eq!(rusqlite::Connection::open(&rollback).unwrap().query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0)).unwrap(), "delete");
    install.app.figure_service.set_builder_path(rollback.to_str().unwrap().to_string());
    for _ in 0..3 {
        let service = install.app.figure_service.clone();
        tokio::task::spawn_blocking(move || service.with_builder(|c| Ok(c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?))).await.unwrap().unwrap();
    }
    assert_eq!(install.app.figure_service.logged(), vec!["figures: database not in WAL; page latency during background work is not guaranteed".to_string()]);
    let wal = Install::new(1, 0);
    wal.app.figure_service.set_builder_path(wal.file.to_str().unwrap().to_string());
    let service = wal.app.figure_service.clone();
    tokio::task::spawn_blocking(move || service.with_builder(|c| Ok(c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?))).await.unwrap().unwrap();
    assert!(wal.app.figure_service.logged().is_empty(), "WAL: nothing to say");
}

#[tokio::test]
async fn builder_order_read_parses_after_the_cursor_closes() {
    // The VM steps of reading a bot's orders when the unreadable row is the first, and when it is the last: the cursor
    // runs to its end before any row is parsed, so its read lock lasts only for the copy, either way.
    let steps = |unreadable_first: bool| -> u64 {
        let install = Install::new(1, 0);
        let c = install.connection();
        c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<3000) \
            INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
            SELECT 1,1,'AAA',2,'USD',1,1,1,100,100,0,0,2,'n'||i,'REGULAR',datetime('2026-03-03','+'||i||' minutes'),'2026-01-01' FROM n").unwrap();
        let at = if unreadable_first { "2026-03-01 00:00:00" } else { "2026-03-20 00:00:00" };
        c.execute("INSERT INTO transactions(bot_id,exchange_id,base,quote,amount,side,status,external_status,external_id,created_at,updated_at) VALUES (1,1,'AAA','USD',1,0,9,2,'bad',?1,'2026-01-01')", [at]).unwrap();
        let counted = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = counted.clone();
        c.progress_handler(1, Some(move || { counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed); false }));
        assert!(deltabadger::figures::db::orders(&c, 1).is_err(), "the unreadable row refuses the history");
        counted.load(std::sync::atomic::Ordering::Relaxed)
    };
    let (first, last) = (steps(true), steps(false));
    assert!(first * 10 >= last * 9, "parsed while the cursor was open: {first} VM steps with the unreadable row first, {last} with it last");
}

// ---- Task 7: what wakes the warming: the boot, a sync job's run, the five-minute tick for active accounts ----------

/// Every account that waits for work now, and what for.
fn queued(app: &App) -> std::collections::BTreeMap<i64, Work> { app.figure_service.take_due(i64::MAX).into_iter().collect() }

#[tokio::test]
async fn boot_enqueues_every_user_with_bots() {
    let install = Install::new(2, 0);
    let c = install.connection();
    let t = "'2026-01-01'";
    // 3: no bots. 4: its only bot deleted. 5: a live Alpaca key (warmed: market data reads with either mode's key). 6: no Alpaca key.
    for user in 3..=6 { add_account(&c, &install.app, user); }
    c.execute("DELETE FROM transactions WHERE bot_id=3", []).unwrap();
    c.execute("DELETE FROM bots WHERE user_id=3", []).unwrap();
    c.execute("UPDATE bots SET status=?1 WHERE user_id=4", [deltabadger::enums::BotStatus::Deleted as i64]).unwrap();
    c.execute("UPDATE api_keys SET passphrase=?1 WHERE user_id=5", [install.app.cipher.encrypt("live")]).unwrap();
    c.execute("DELETE FROM api_keys WHERE user_id=6", []).unwrap();
    c.execute(&format!("UPDATE users SET locale='fr',updated_at={t} WHERE id=2"), []).unwrap();
    deltabadger::web::figure::warm::boot(&install.app).await;
    let queued = queued(&install.app);
    assert_eq!(queued.keys().copied().collect::<Vec<_>>(), vec![1, 2, 5], "the accounts with bots and an Alpaca key");
    assert_eq!(queued[&1], Work { publish: true, locales: own(), not_before: install.now() });
    assert_eq!(queued[&2].locales, BTreeSet::from([("fr".to_string(), "/fr".to_string())]), "each in its own locale");
}

#[tokio::test]
async fn warm_tick_fills_only_users_active_in_30_minutes() {
    let install = Install::new(2, 0);
    install.clock.set(common::web::at("2026-03-18T11:31:00Z"));
    // Account 2 signs in 31 minutes before the tick, account 1 a minute before it.
    use deltabadger::web::session::{self, SessionData};
    let page = |user: i64| { let signed = SessionData { user: Some((user, "x".into())), ..SessionData::default() }; common::web::Browser { cookie: Some(session::seal(&install.app.keys.session, &signed, install.clock_now())), page: None } };
    assert_eq!(page(2).get(&install.app, "/settings/connect").await.status, 200);
    install.clock.set(common::web::at("2026-03-18T12:01:00Z"));
    assert_eq!(page(1).get(&install.app, "/settings/connect").await.status, 200);
    let _stop = install.consume();
    tokio::time::sleep(Duration::from_millis(100)).await; // the service is running, and knows when its next tick is
    // The tick: two seconds past the next five-minute cut.
    install.clock.set(common::web::at("2026-03-18T12:05:02Z"));
    until(10, "account 1's warm fill", async || fills(&install.app, 1) > 0 && matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Ready(..))).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(fills(&install.app, 2), 0, "account 2 was last active 34 minutes ago");
}

/// A fake job of `name` and `scope`, which only counts its runs.
struct Ran { name: &'static str, scope: Option<String> }
impl deltabadger::jobs::Job for Ran {
    fn spec(&self) -> deltabadger::jobs::Spec { deltabadger::jobs::Spec { name: self.name, scope: self.scope.clone(), schedule: None, jitter: deltabadger::jobs::schedule::Jitter::NONE, retry: deltabadger::jobs::Retry::None, deadline: deltabadger::jobs::DEADLINE } }
    fn run<'a>(&'a self, _: deltabadger::jobs::Cx<'a>, _: Vec<deltabadger::jobs::Wake>) -> deltabadger::jobs::JobFuture<'a> { Box::pin(async { deltabadger::jobs::Outcome::Done }) }
}

#[tokio::test]
async fn a_sync_job_run_enqueues_its_user_once_per_debounce() {
    let install = Install::new(2, 0);
    let key: i64 = install.connection().query_row("SELECT id FROM api_keys WHERE user_id=2", [], |r| r.get(0)).unwrap();
    let jobs: Vec<Box<dyn deltabadger::jobs::Job>> = vec![Box::new(Ran { name: deltabadger::sync::jobs::LEDGER_SYNC, scope: Some(key.to_string()) })];
    let scheduler = deltabadger::jobs::Scheduler::new(install.connection(), install.app.cipher.clone(), jobs, None)
        .with_ran(deltabadger::web::figure::warm::job_ran(install.app.clone()));
    let wakers = scheduler.wakers();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let due = install.now() + 5;
    let (ended, ()) = tokio::join!(scheduler.run(stopped, &*install.clock), async {
        wakers.wake(deltabadger::sync::jobs::LEDGER_SYNC, Some(&key.to_string()), None);
        until(5, "the first run's work", async || install.app.figure_service.next_due().is_some()).await;
        install.clock.set(install.clock_now() + chrono::Duration::seconds(3));
        wakers.wake(deltabadger::sync::jobs::LEDGER_SYNC, Some(&key.to_string()), None);
        tokio::time::sleep(Duration::from_millis(500)).await;
        stop.send(true).unwrap();
    });
    ended.unwrap();
    let queued = queued(&install.app);
    assert_eq!(queued.keys().copied().collect::<Vec<_>>(), vec![2], "the key's owner, and no one else");
    assert_eq!(queued[&2].not_before, due, "a later run never pushes the debounce back");
}

// ---- Review: queued work survives a failed render-only pass; a page reads a precomputed reason ---------------------

#[tokio::test]
async fn a_render_only_pass_overtaken_by_a_write_keeps_its_work() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the English figures", async || published(&install.app, 1).await).await;
    // Armed now: the builder's next read of the orders (the French render-only pass) first sees a bot's settings change
    // committed by another connection, after `consume` read the fingerprint and before `keep` checks it.
    let (armed, written) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicBool::new(false)));
    let (trigger, done, file) = (armed.clone(), written.clone(), install.file.clone());
    install.app.figure_service.on_builder(move |c| {
        c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "transactions", .. }) && trigger.swap(false, SeqCst) {
                rusqlite::Connection::open(&file).unwrap().execute("UPDATE bots SET label='renamed' WHERE id=1", []).unwrap();
                done.store(true, SeqCst);
            }
            rusqlite::hooks::Authorization::Allow
        }));
    });
    let before = fills(&install.app, 1);
    let pages = install.app.clone();
    loading::enqueue_page(&pages, 1, "fr", "/fr");
    until(10, "the write during the French pass, committed", async || written.load(SeqCst)).await;
    // Nothing but the queue asks: the work it queued again finds the new revision, starts one fill that carries it, and
    // that fill ends in the publication.
    until(10, "the publication after the write", async || published(&install.app, 1).await).await;
    assert_eq!(fills(&install.app, 1), before + 1, "the retry is one full fill");
    assert!(matches!(page_figures(&install.app, 1, "fr", "/fr").await, (Some(_), 0)), "the French figures, kept by that fill");
}

#[tokio::test]
async fn repeated_overtakes_never_lose_the_request() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the English figures", async || published(&install.app, 1).await).await;
    // Eight writes, one as each computation on the builder begins (it reads the account's settings first): the French
    // pass, then every pass of every fill its retries start, is overtaken until they stop.
    let (writes, committed) = (Arc::new(AtomicUsize::new(8)), Arc::new(AtomicUsize::new(0)));
    let (left, done, file) = (writes.clone(), committed.clone(), install.file.clone());
    install.app.figure_service.on_builder(move |c| {
        c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "users", column_name: "hide_balances" }) {
                if let Ok(n) = left.fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1)) {
                    rusqlite::Connection::open(&file).unwrap().execute("UPDATE bots SET label=?1 WHERE id=1", [format!("renamed {n}")]).unwrap();
                    done.fetch_add(1, SeqCst);
                }
            }
            rusqlite::hooks::Authorization::Allow
        }));
    });
    let pages = install.app.clone();
    loading::enqueue_page(&pages, 1, "fr", "/fr");
    until(20, "every write, committed", async || committed.load(SeqCst) == 8).await;
    until(20, "the publication after the last write", async || published(&install.app, 1).await).await;
    assert!(matches!(page_figures(&install.app, 1, "fr", "/fr").await, (Some(_), 0)), "the French figures, after eight overtakes");
}

#[tokio::test]
async fn an_account_without_an_alpaca_key_reads_a_precomputed_reason() {
    let install = Install::new(1, 0);
    let c = install.connection();
    c.execute("DELETE FROM api_keys WHERE user_id=1", []).unwrap();
    // A long history, one fill of it executed with no value: the reason Rails' row_readings case shows.
    c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<3000) \
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
        SELECT 1,1,'AAA',2,'USD',1,1,1,100,100,0,0,2,'n'||i,'REGULAR',datetime('2026-03-03','+'||i||' minutes'),'2026-01-01' FROM n; \
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
        VALUES (1,1,'AAA',2,'USD',1,1,1,0,0,2,'unpriced','REGULAR','2026-03-10 00:00:00','2026-01-01');").unwrap();
    let _stop = install.consume();
    let absent = "<span class=\"no-value\">Figures unavailable: executed fill value unavailable</span>";
    let shown = |snapshot: loading::Snapshot| { let app = install.app.clone(); async move { app.clone().db(move |c| loading::render_page(&app, c, 1, &snapshot, "en", "token", "")).await.unwrap() } };
    // The first view may show the loading state while the queue reads the reason; every view after it reads it.
    let first = loading::prepare(&install.app, 1).await.unwrap();
    shown(first).await;
    until(10, "the reason, read by the queue", async || matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Failed(Some(_)))).await;
    // A view runs no order walk on the page's connection (the fingerprint, which hashes whole rows, is no walk).
    let walks = order_walks(&install.app).await;
    let snapshot = loading::prepare(&install.app, 1).await.unwrap();
    let page = shown(snapshot).await.expect("the account's no-value figures");
    assert_eq!(walks.finish(&install.app).await, 0, "a page view walked the orders");
    assert_eq!(page, json!({"bots": {"1": {"tile": format!("<div id=\"pnl_bots_dca_multi_asset_1\">{absent}</div>"), "metrics": format!("<div id=\"metrics\">{absent}</div>"),
        "chart": format!("<div id=\"chart\">{absent}</div>")}}, "account": format!("<div id=\"global-pnl\">{absent}</div>")}));
}

/// Statements on the page's connection that read the orders' values the way a walk does (`db::orders`: price and amounts,
/// not the whole row the fingerprint hashes), counted from when it is installed until `finish`.
struct Walks(Arc<std::sync::Mutex<(bool, bool, usize)>>);
async fn order_walks(app: &App) -> Walks {
    let seen = Arc::new(std::sync::Mutex::new((false, false, 0)));
    let statement = seen.clone();
    app.db(move |c| {
        c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            let mut s = statement.lock().unwrap();
            match context.action {
                rusqlite::hooks::AuthAction::Select => { if s.0 && !s.1 { s.2 += 1; } s.0 = false; s.1 = false; }
                rusqlite::hooks::AuthAction::Read { table_name: "transactions", column_name: "price" } => s.0 = true,
                rusqlite::hooks::AuthAction::Read { table_name: "transactions", column_name: "external_id" } => s.1 = true,
                _ => {}
            }
            rusqlite::hooks::Authorization::Allow
        }));
        Ok(())
    }).await.unwrap();
    Walks(seen)
}
impl Walks {
    async fn finish(self, app: &App) -> usize {
        app.db(|c| { c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>); Ok(()) }).await.unwrap();
        let s = self.0.lock().unwrap();
        s.2 + usize::from(s.0 && !s.1)
    }
}

/// An account with no Alpaca key whose one executed fill has no value.
fn unkeyed_install() -> Install {
    let install = Install::new(1, 0);
    let c = install.connection();
    c.execute("DELETE FROM api_keys WHERE user_id=1", []).unwrap();
    c.execute_batch("INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
        VALUES (1,1,'AAA',2,'USD',1,1,1,0,0,2,'unpriced','REGULAR','2026-03-10 00:00:00','2026-03-10 00:00:00');").unwrap();
    install
}

#[tokio::test]
async fn a_fill_repaired_in_the_same_second_changes_the_no_key_reason() {
    let install = unkeyed_install();
    let _stop = install.consume();
    let first = loading::prepare(&install.app, 1).await.unwrap();
    let app = install.app.clone();
    install.app.db(move |c| loading::render_page(&app, c, 1, &first, "en", "", "")).await.unwrap();
    until(10, "the unpriced reason", async || matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Failed(Some("executed fill value unavailable")))).await;
    // The venue's price arrives; the row keeps its timestamp, to the second.
    install.connection().execute("UPDATE transactions SET price=100, quote_amount_exec=100 WHERE external_id='unpriced'", []).unwrap();
    let stale = loading::prepare(&install.app, 1).await.unwrap();
    assert!(!matches!(stale, loading::Snapshot::Failed(Some(_))), "the old reason, after the fill was repaired");
    let app = install.app.clone();
    install.app.db(move |c| loading::render_page(&app, c, 1, &stale, "en", "", "")).await.unwrap();
    until(10, "the repaired account's reason", async || matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Failed(None))).await;
    assert_eq!(fills(&install.app, 1), 0, "no key: the reason comes from the stored fills, never from a market read");
}

/// A live key reads market data as a paper key does (Rails' market_data_client): its account gets figures, and every
/// request it made was to the data host (the script answers nothing else).
#[tokio::test]
async fn a_live_key_account_gets_its_figures() {
    let install = Install::new(1, 0);
    install.connection().execute("UPDATE api_keys SET passphrase=?1 WHERE user_id=1", [install.app.cipher.encrypt("live")]).unwrap();
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the live key's figures", async || published(&install.app, 1).await).await;
    assert!(matches!(page_figures(&install.app, 1, "en", "").await, (Some(_), _)));
}

/// The account's figures streams on a `/cable` connection of its own, as a page subscribes to them.
async fn cable(install: &Install, stream: &str) -> (tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio::task::JoinHandle<()>) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    use deltabadger::web::session::{self, SessionData};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let served = install.app.clone();
    let server = tokio::spawn(async move { let _ = deltabadger::web::server::serve_on(listener, served, Default::default()).await; });
    let cookie = session::seal(&install.app.keys.session, &SessionData { user: Some((1, "x".into())), ..SessionData::default() }, install.clock_now());
    let mut request = format!("ws://{address}/cable").into_client_request().unwrap();
    request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
    request.headers_mut().insert("sec-websocket-protocol", "actioncable-v1-json".parse().unwrap());
    request.headers_mut().insert("cookie", format!("_deltabadger_rust_session={cookie}").parse().unwrap());
    let mut socket = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(request)).await.unwrap().unwrap().0;
    let identifier = json!({"channel": "Turbo::StreamsChannel", "signed_stream_name": deltabadger::web::cable::signed_stream_name(&install.app.keys.streams, stream)}).to_string();
    socket.send(Message::Text(json!({"command": "subscribe", "identifier": identifier}).to_string().into())).await.unwrap();
    loop {
        let message = socket.next().await.unwrap().unwrap();
        if serde_json::from_str::<Value>(message.to_text().unwrap()).unwrap()["type"] == "confirm_subscription" { break; }
    }
    (socket, server)
}

#[tokio::test]
async fn a_no_key_publication_overtaken_by_the_cut_is_still_delivered() {
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let install = unkeyed_install();
    let (mut socket, server) = cable(&install, "user_1:bot_updates").await;
    // The queue's reading of the reason runs across the five-minute cut: what it read has expired by the time its
    // publication renders.
    let armed = Arc::new(AtomicBool::new(true));
    let (trigger, clock) = (armed.clone(), install.clock.clone());
    install.app.figure_service.on_builder(move |c| {
        c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "transactions", .. }) && trigger.swap(false, SeqCst) {
                clock.set(common::web::at("2026-03-18T12:05:00Z"));
            }
            rusqlite::hooks::Authorization::Allow
        }));
    });
    let _stop = install.consume();
    let pages = install.app.clone();
    loading::enqueue_page(&pages, 1, "en", "");
    let delivered = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if message.to_text().unwrap().contains("Figures unavailable: executed fill value unavailable") { break; }
        }
    }).await;
    server.abort();
    assert!(!armed.load(SeqCst), "the reading crossed the cut");
    assert!(delivered.is_ok(), "the page never received its publication");
}

// ---- Review: a render-only pass that fails with nothing written makes no loop -----------------------------------------

#[tokio::test]
async fn a_failing_render_only_pass_with_no_write_is_not_retried_in_a_loop() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the English figures", async || published(&install.app, 1).await).await;
    // From now on every figures computation on the builder fails (as one over its budget would), and nothing is written:
    // the revision and the ready entry stay as they are. Each computation reads the account's settings first.
    let (armed, passes) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)));
    let (failing, counted) = (armed.clone(), passes.clone());
    install.app.figure_service.on_builder(move |c| {
        c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if failing.load(SeqCst) && matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "users", column_name: "hide_balances" }) {
                counted.fetch_add(1, SeqCst);
                return rusqlite::hooks::Authorization::Deny;
            }
            rusqlite::hooks::Authorization::Allow
        }));
    });
    install.app.figure_service.enqueue(1, Work { publish: true, locales: BTreeSet::from([("fr".to_string(), "/fr".to_string()), ("de".to_string(), "/de".to_string())]), not_before: install.now() });
    until(10, "the French and German passes", async || passes.load(SeqCst) > 0).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let made = passes.load(SeqCst);
    assert!(made <= 4, "{made} computations in a second and a half: a pass that fails with nothing written is retried in a loop");
    let failures: Vec<String> = install.app.figure_service.logged().into_iter().filter(|line| line.contains("not rendered")).collect();
    assert_eq!(failures.len(), 1, "the failure is said once: {failures:?}");
    assert_eq!(fills(&install.app, 1), 1, "no fill: nothing was written");
}

#[tokio::test]
async fn a_failed_render_only_pass_keeps_its_publication_for_the_next_fill() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let install = Install::new(1, 0);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the English figures", async || published(&install.app, 1).await).await;
    // The page's French pass fails against the unchanged revision (every computation on the builder fails while armed).
    let armed = Arc::new(AtomicBool::new(true));
    let failing = armed.clone();
    install.app.figure_service.on_builder(move |c| {
        c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if failing.load(SeqCst) && matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "users", column_name: "hide_balances" }) {
                return rusqlite::hooks::Authorization::Deny;
            }
            rusqlite::hooks::Authorization::Allow
        }));
    });
    loading::enqueue_page(&install.app, 1, "fr", "/fr");
    until(10, "the failed French pass", async || install.app.figure_service.logged().iter().any(|line| line.contains("not rendered"))).await;
    armed.store(false, SeqCst);
    // A write, then a warm that asks no publication (as the tick and the scheduler do): its fill ends in the page's.
    install.connection().execute("UPDATE bots SET label='renamed' WHERE id=1", []).unwrap();
    install.app.figure_service.enqueue(1, Work { publish: false, locales: Default::default(), not_before: install.now() });
    until(10, "the publication the failed pass owed", async || published(&install.app, 1).await).await;
}

// ---- Stale while refilling: Rails' per-bot cache outlives another bot's writes ------------------------------------------

/// Bot `bot` of account 1, started (scheduled), on AAA like the account's first.
fn start_bot(c: &rusqlite::Connection, bot: i64) {
    c.execute("INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) \
               VALUES (?1,1,1,'Bots::DcaMultiAsset',1,'{\"quote_asset_id\":1,\"allocations\":{\"2\":1.0}}','{}','2026-03-18','2026-03-18')", [bot]).unwrap();
}
/// An accepted buy of AAA by `bot`.
fn order(c: &rusqlite::Connection, bot: i64, external: &str) {
    c.execute("INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
               VALUES (?1,1,'AAA',2,'USD',1,1,1,100,100,0,0,2,?2,'REGULAR','2026-03-17 14:30:00','2026-03-18')", rusqlite::params![bot, external]).unwrap();
}

#[tokio::test]
async fn another_bots_start_and_order_keep_a_bots_figures_until_the_refill_swaps_them() {
    let install = Install::new(1, 150);
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the first figures", async || published(&install.app, 1).await).await;
    let (before, _) = page_figures(&install.app, 1, "en", "").await;
    let before = before.expect("ready figures");
    let tile = before["bots"]["1"]["tile"].clone();
    assert!(tile.is_string() && before["account"].is_string());
    // Bot B is started, then gets an order: A's tile, page metrics and chart are A's throughout, never the loading state.
    let c = install.connection();
    start_bot(&c, 100);
    let (shown, _) = page_figures(&install.app, 1, "en", "").await;
    let shown = shown.expect("A's figures while B's start is refilled, not the loading state");
    for panel in ["tile", "metrics", "chart"] { assert_eq!(shown["bots"]["1"][panel], before["bots"]["1"][panel], "A's {panel}"); }
    assert!(shown["bots"]["100"].is_null(), "B has no figures yet: its panels show the loading state");
    assert!(shown["account"].is_string(), "B has no accepted order: the headline sums A, as User#global_pnl_snapshot does");
    order(&c, 100, "b1");
    until(10, "both bots' figures", async || {
        let (shown, _) = page_figures(&install.app, 1, "en", "").await;
        let shown = shown.expect("never the loading state for A");
        assert_eq!(shown["bots"]["1"]["tile"], tile, "A's tile");
        if shown["bots"]["100"].is_null() { assert!(shown["account"].is_null(), "B's order has no figures yet: the headline waits"); }
        shown["bots"]["100"]["tile"].is_string() && shown["account"].is_string()
    }).await;
    // B's figures read before its order may show a moment, as its Rails cache would; the refill after the order swaps in.
    until(10, "the refill for the current revision", async || {
        let (shown, _) = page_figures(&install.app, 1, "en", "").await;
        assert_eq!(shown.expect("never the loading state for A")["bots"]["1"]["tile"], tile, "A's tile");
        matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Ready(..))
    }).await;
}

#[tokio::test]
async fn writes_during_every_refill_still_end_in_saved_figures() {
    let install = Install::new(1, 50);
    let c = install.connection();
    assert!(matches!(loading::prepare(&install.app, 1).await.unwrap(), loading::Snapshot::Cold), "nothing saved yet");
    // A write lands every couple of milliseconds, before and during every fill, until figures are shown.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = { let (stop, file) = (stop.clone(), install.file.clone()); std::thread::spawn(move || {
        let c = rusqlite::Connection::open(file).unwrap();
        c.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut n = 0u32;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) { n += 1; order(&c, 1, &format!("w{n}")); std::thread::sleep(Duration::from_millis(2)); }
        n
    }) };
    until(10, "figures saved under churn", async || {
        let (shown, _) = page_figures(&install.app, 1, "en", "").await;
        shown.is_some_and(|shown| shown["bots"]["1"]["tile"].is_string())
    }).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(writer.join().unwrap() > 1, "the writes kept coming");
    drop(c);
}

/// Ready figures of account 1, published and shown, with bots 1 and `extra` (none if 0) of AAA.
async fn warm_with(install: &Install, extra: i64) -> Value {
    if extra != 0 { start_bot(&install.connection(), extra); }
    let _stop = install.consume();
    install.app.figure_service.enqueue(1, Work { publish: true, locales: own(), not_before: install.now() });
    until(10, "the first figures", async || published(&install.app, 1).await).await;
    page_figures(&install.app, 1, "en", "").await.0.expect("ready figures")
}

#[tokio::test]
async fn a_cacheless_bots_first_order_holds_back_the_stale_headline() {
    let install = Install::new(1, 150);
    let before = warm_with(&install, 100).await;
    assert_eq!(before["bots"]["100"]["cached"], false, "no order: Rails caches nothing for it");
    order(&install.connection(), 100, "b1");
    let snapshot = loading::prepare(&install.app, 1).await.unwrap();
    assert!(matches!(snapshot, loading::Snapshot::Stale(..)), "the refill runs");
    let clock = install.app.clock.clone();
    let shown = install.app.db(move |c| loading::render(c, 1, &snapshot, "en", "token", "", &*clock)).await.unwrap().expect("A's figures");
    assert_eq!(shown["bots"]["1"]["tile"], before["bots"]["1"]["tile"], "A's tile");
    assert!(shown["account"].is_null(), "a bot with a submitted order and no cache: User#global_pnl_snapshot is loading");
}


#[tokio::test]
async fn a_sell_control_a_live_minimum_removes_is_not_shown_stale() {
    let install = Install::new(1, 150);
    let before = warm_with(&install, 0).await;
    assert!(before["bots"]["1"]["metrics"].as_str().unwrap().contains("/liquidation/new?"), "AAA is sellable");
    install.connection().execute("UPDATE tickers SET minimum_base_size=1000 WHERE id=1", []).unwrap();
    let snapshot = loading::prepare(&install.app, 1).await.unwrap();
    let clock = install.app.clock.clone();
    let shown = install.app.db(move |c| loading::render(c, 1, &snapshot, "en", "token", "", &*clock)).await.unwrap();
    let metrics = shown.as_ref().and_then(|v| v["bots"]["1"]["metrics"].as_str().map(str::to_string));
    assert!(metrics.is_none_or(|m| !m.contains("/liquidation/new?")), "sellable_holdings drops a holding under the minimum: no Sell link");
}
