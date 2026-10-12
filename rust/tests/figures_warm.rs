//! Figures are made ahead of any page: the queue of work and its consumer, the fill that keeps what it rendered, expiry
//! at consumption, the bounded market fetch, the builder's own connection, and what wakes the warming. A page only reads
//! what was kept, or shows Rails' loading state and leaves the work to the queue.
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
#[allow(dead_code)]
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
    #[allow(dead_code)]
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
    c.execute(&format!("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES (?1,'u'||?1||'@example.com','x',{t},{t})"), [user]).unwrap();
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
