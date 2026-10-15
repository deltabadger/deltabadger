//! TrackerController#index's `load_ledgers` / `load_history`: a served tracker page wakes the ledger walk while no warm
//! ledger is cached and the sweep while `backfill::wanted`; a refused page wakes nothing, and neither writes a row.
mod common;
use common::web::{self, Browser, TestClock};
use deltabadger::{jobs::{self, Cx, Db, Outcome}, sync::jobs::Connect, tracker::jobs::{self as tracker_jobs, PORTFOLIO_BACKFILL, TRACKER_LEDGER},
                  venue::{alpaca::{AlpacaVenue, Urls}, http::ScriptedTransport}, web::{session, App}};
use std::{rc::Rc, sync::Arc};
#[path = "support/tracker_index_grid.rs"]
#[allow(dead_code)]
mod grid;

const NOW: &str = "2026-09-20T02:00:00Z";

#[derive(Clone)]
struct Venues;
impl Connect for Venues {
    type T = ScriptedTransport;
    fn connect(&self, _: &deltabadger::crypto::Credentials) -> AlpacaVenue<Self::T> { AlpacaVenue::new(ScriptedTransport::from_script(&serde_json::json!({})), Urls::for_passphrase(Some("paper"))) }
}

fn install() -> (tempfile::TempDir, rusqlite::Connection, i64, App, Browser) {
    let (dir, opened, seeded) = common::install_alpaca();
    let c = opened.primary;
    common::seed::fresh_stock_jobs(&c, NOW.parse().unwrap());
    c.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,4,'USD',100,'2026-09-01 00:00:00','{}','{}','2026-09-01 00:00:00','2026-09-01 00:00:00')", [seeded.user_id, seeded.exchange_id]).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    app.attach_jobs(jobs::Wakers::default()).unwrap();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), ..Default::default() };
    let browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    (dir, c, seeded.user_id, app, browser)
}

/// The jobs one GET asked for, drained from the wakers between requests.
async fn wakes(app: &App, browser: &mut Browser, path: &str) -> (u16, Vec<(String, Option<String>)>) {
    let wakers = app.job_wakers().unwrap();
    let before = wakers.pending().len();
    let status = browser.get(app, path).await.status;
    (status, wakers.pending()[before..].to_vec())
}

async fn run(dir: &std::path::Path, app: &App, name: &str, owner: i64) -> Outcome {
    let now: chrono::DateTime<chrono::Utc> = NOW.parse().unwrap();
    let job = tracker_jobs::resolve::<Venues, ScriptedTransport>(name, owner, &Venues, Rc::new(None), Arc::new(move || now)).unwrap();
    let db = Db::new(rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(), app.cipher.clone());
    let clock = deltabadger::engine::FixedClock(now);
    job.run(Cx { db, clock: &clock, wakers: jobs::Wakers::default() }, vec![]).await
}

#[tokio::test(flavor = "current_thread")]
async fn a_served_page_wakes_what_is_wanted_and_a_refused_page_wakes_nothing() {
    let (dir, c, owner, app, mut b) = install();
    let me = Some(owner.to_string());
    let both = vec![(TRACKER_LEDGER.to_string(), me.clone()), (PORTFOLIO_BACKFILL.to_string(), me.clone())];
    let before = grid::snapshot(&c);

    // Nothing swept, no ledger cached.
    assert_eq!(wakes(&app, &mut b, "/tracker").await, (200, both.clone()));
    for refused in ["/tracker?from=2026-09-01", "/tracker?exchange_id[]=1", "/tracker?exchange_id=999999"] {
        let (status, asked) = wakes(&app, &mut b, refused).await;
        assert_ne!(status, 200, "{refused}");
        assert!(asked.is_empty(), "{refused} woke {asked:?}");
    }
    assert_eq!(grid::snapshot(&c), before, "a GET writes nothing");

    // The history swept, the ledger still cold: the walk alone.
    assert_eq!(run(dir.path(), &app, PORTFOLIO_BACKFILL, owner).await, Outcome::Done);
    assert_eq!(wakes(&app, &mut b, "/tracker").await, (200, vec![(TRACKER_LEDGER.to_string(), me.clone())]));

    // Both current: nothing.
    assert_eq!(run(dir.path(), &app, TRACKER_LEDGER, owner).await, Outcome::Done);
    assert_eq!(wakes(&app, &mut b, "/tracker").await, (200, vec![]));

    // A row the sweep has not read: the history is stale, and so is the ledger.
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,(SELECT exchange_id FROM account_transactions LIMIT 1),4,'USD',5,'2026-09-10 00:00:00','{}','{}','2026-09-19 00:00:00','2026-09-19 00:00:00')", [owner]).unwrap();
    assert_eq!(wakes(&app, &mut b, "/tracker").await, (200, both.clone()));

    // A walk that failed is not a cached ledger: the page asks again.
    assert_eq!(run(dir.path(), &app, PORTFOLIO_BACKFILL, owner).await, Outcome::Done);
    c.execute_batch("UPDATE account_transactions SET raw_data='not json', updated_at='2026-09-19 12:00:00'").unwrap();
    assert!(matches!(run(dir.path(), &app, TRACKER_LEDGER, owner).await, Outcome::Failed(_)));
    let (status, asked) = wakes(&app, &mut b, "/tracker").await;
    assert!(asked.contains(&(TRACKER_LEDGER.to_string(), me.clone())), "{status} {asked:?}");
}

/// An account with nothing in it yet: no ledger cached, no history stamped. Rails' loaders ask for both.
#[tokio::test(flavor = "current_thread")]
async fn an_empty_account_asks_for_both() {
    let (_dir, c, owner, app, mut b) = install();
    c.execute_batch("DELETE FROM account_transactions; DELETE FROM api_keys;").unwrap();
    let me = Some(owner.to_string());
    assert_eq!(wakes(&app, &mut b, "/tracker").await, (200, vec![(TRACKER_LEDGER.to_string(), me.clone()), (PORTFOLIO_BACKFILL.to_string(), me)]));
}

/// The sweep's gate, not `stale_prices?`: a partial row only for today (the sync's to rewrite) with a new price
/// generation asks for no sweep; a partial day inside the window does.
#[tokio::test(flavor = "current_thread")]
async fn only_a_partial_day_a_sweep_can_reach_asks_for_one_when_prices_arrive() {
    let (dir, c, owner, app, mut b) = install();
    assert_eq!(run(dir.path(), &app, PORTFOLIO_BACKFILL, owner).await, Outcome::Done);
    assert_eq!(run(dir.path(), &app, TRACKER_LEDGER, owner).await, Outcome::Done);
    let generation = |c: &rusqlite::Connection| c.execute("INSERT INTO historical_prices(asset,currency,date,price) VALUES('wakes-test','usd','2020-01-01',1)", []).unwrap();
    c.execute("UPDATE portfolio_snapshots SET partial=1 WHERE user_id=?1 AND date='2026-09-20'", [owner]).unwrap();
    assert_eq!(c.query_row("SELECT changes()", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "the ledger wrote today's row");
    generation(&c);
    // New prices make the cached ledger stale too (its version carries the generation): the walk is asked for.
    let sweep = (PORTFOLIO_BACKFILL.to_string(), Some(owner.to_string()));
    let (status, asked) = wakes(&app, &mut b, "/tracker").await;
    assert_eq!(status, 200);
    assert!(!asked.contains(&sweep), "today's partial row asked for a sweep: {asked:?}");
    c.execute("UPDATE portfolio_snapshots SET partial=1 WHERE user_id=?1 AND date='2026-09-10'", [owner]).unwrap();
    assert!(wakes(&app, &mut b, "/tracker").await.1.contains(&sweep), "a partial day in the window asks for a sweep");
}

/// A sweep gate that cannot be read drops its own wake: the page is served, and the cold ledger still walks.
#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_sweep_gate_keeps_the_page_and_the_ledger_wake() {
    let (dir, c, owner, app, mut b) = install();
    assert_eq!(run(dir.path(), &app, PORTFOLIO_BACKFILL, owner).await, Outcome::Done);
    assert_eq!(c.execute(r#"UPDATE app_configs SET value='{"p":"AAAA","h":{"iv":"AAAAAAAAAAAAAAAA","at":"AAAAAAAAAAAAAAAAAAAAAA=="}}' WHERE key=?1"#, [deltabadger::tracker::backfill::history_key(owner)]).unwrap(), 1);
    assert_eq!(wakes(&app, &mut b, "/tracker").await, (200, vec![(TRACKER_LEDGER.to_string(), Some(owner.to_string()))]));
}
