//! What wakes the figures' warming, all of it through the queue (`Service::enqueue`, consumed by `loading::follow`):
//! the boot, a scheduler job that rewrote what the figures read, and the five-minute tick for the accounts signed in
//! within Rails' activity window (Bot::WarmMetricsCachesJob).
use super::{loading, service::Work};
use crate::web::App;

/// Bot::WarmMetricsCachesJob::ACTIVITY_WINDOW.
pub const ACTIVITY_WINDOW: i64 = 30 * 60;
/// A job run's figures work waits this long, so that a burst of runs (a key's ledger and balances) fills once. A later
/// run never pushes it back (`Work` keeps the earlier due time).
pub const DEBOUNCE: i64 = 5;

/// The accounts with figures: a non-deleted bot and a paper Alpaca key.
async fn accounts(app: &App) -> Vec<(i64, (String, String))> {
    let inner = app.clone();
    let found = app.db(move |c| {
        let users: Vec<i64> = c.prepare(&format!("SELECT DISTINCT user_id FROM bots WHERE status != {} AND user_id IS NOT NULL ORDER BY user_id", crate::enums::BotStatus::Deleted as i64))?
            .query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut out = vec![];
        for user in users {
            if loading::read_info(c, &inner, user)?.is_some() { out.push((user, loading::own_key(c, user)?)); }
        }
        Ok(out)
    }).await;
    match found { Ok(found) => found, Err(e) => { crate::engine::log(&format!("figures: the accounts to warm could not be read: {e:?}")); vec![] } }
}

/// At boot, whoever reads the figures: each account's own figures are made and published, before any page asks.
pub async fn boot(app: &App) {
    let now = app.now().timestamp();
    for (user, key) in accounts(app).await { app.figure_service.enqueue(user, Work { publish: true, locales: [key].into(), not_before: now }); }
}

/// Two seconds past each five-minute cut: the accounts signed in within the activity window are filled again, as
/// Bot::WarmMetricsCachesJob keeps their caches warm.
pub async fn tick(app: &App) {
    let now = app.now().timestamp();
    let active = app.figure_service.active_since(now - ACTIVITY_WINDOW);
    for (user, key) in accounts(app).await.into_iter().filter(|(user, _)| active.contains(user)) {
        app.figure_service.enqueue(user, Work { publish: false, locales: [key].into(), not_before: now });
    }
}
/// When the tick after `now` falls.
pub fn next_tick(now: i64) -> i64 { let at = now - now.rem_euclid(300) + 2; if at > now { at } else { at + 300 } }

/// The scheduler's hook (`Scheduler::with_ran`): a job that rewrites what the figures read queues its accounts'
/// figures, `DEBOUNCE` seconds on. A key's ledger and balances: its owner. The tracker's ledger: its user. The reference
/// data (listings, assets, indices, the setup's seed): every account with figures.
pub fn job_ran(app: App) -> crate::jobs::Ran {
    Box::new(move |name, scope| {
        use crate::jobs::reference;
        let scope = scope.and_then(|s| s.parse::<i64>().ok());
        let whom = match name {
            crate::sync::jobs::LEDGER_SYNC | crate::sync::jobs::BALANCE_SYNC => match scope { Some(key) => Whom::Key(key), None => return },
            crate::tracker::jobs::TRACKER_LEDGER => match scope { Some(user) => Whom::User(user), None => return },
            reference::STOCKS | reference::ALPACA_CRYPTO | reference::INDICES | reference::TICKERS | reference::ASSETS | crate::jobs::setup_sync::NAME => Whom::All,
            _ => return,
        };
        let app = app.clone();
        tokio::spawn(async move {
            let not_before = app.now().timestamp() + DEBOUNCE;
            let users = match whom {
                Whom::User(user) => vec![user],
                Whom::Key(key) => app.db(move |c| { use rusqlite::OptionalExtension; Ok(c.query_row("SELECT user_id FROM api_keys WHERE id=?1", [key], |r| r.get::<_, i64>(0)).optional()?) }).await.ok().flatten().into_iter().collect(),
                Whom::All => accounts(&app).await.into_iter().map(|(user, _)| user).collect(),
            };
            for user in users { app.figure_service.enqueue(user, Work { publish: false, locales: Default::default(), not_before }); }
        });
    })
}
enum Whom { Key(i64), User(i64), All }
