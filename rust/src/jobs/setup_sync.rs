//! Setup::SeedAndSyncJob on the deltabadger provider, which the platform claim starts (web::setup): setup_sync_status
//! pending -> in_progress, the reference syncs its deltabadger branch runs (tickers of every venue, assets, indices,
//! stocks), then completed, and the "syncing" banner (`flash-syncing` on `settings_sync`) is removed.
//!
//! `load_seed` is not run: the install was seeded by db:prepare. The CoinGecko branch is not ported: without the
//! deltabadger provider the job only completes. The syncs run here with the data-api configuration as it is now (the
//! claim has just written it); each records its outcome under its own name, as the scheduler would.
use super::data_api::{self, Config, DataApi};
use super::schedule::Jitter;
use super::{reference, state, Cx, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use crate::app_config;
use crate::venue::http::Transport;

pub const NAME: &str = "setup_seed_and_sync_job";
const STATUS: &str = "setup_sync_status";
/// What SeedAndSyncJob's deltabadger branch syncs, in its order (the stock sync it enqueues runs last).
const SYNCS: [&str; 4] = [reference::TICKERS, reference::ASSETS, reference::INDICES, reference::STOCKS];

/// A committed claim survives process shutdown even if its in-memory wake did not. Only a persisted
/// connection with unfinished work is recovered; completed or never-connected installs stay idle.
pub fn unfinished(c: &rusqlite::Connection, cipher: &crate::crypto::Cipher) -> Result<bool, String> {
    let connected = app_config::get(c, cipher, "platform_connected_at")?.is_some_and(|at| !crate::ruby::blank(&at));
    Ok(connected && matches!(app_config::get(c, cipher, STATUS)?.as_deref(), Some("pending" | "in_progress")))
}

pub struct SeedAndSync<T: Transport> {
    /// MARKET_DATA_URL and MARKET_DATA_TOKEN as the process was started with them.
    env: Vec<(&'static str, String)>,
    api: Box<dyn Fn(Config) -> DataApi<T>>,
}

impl<T: Transport> SeedAndSync<T> {
    pub fn new(env: &dyn Fn(&str) -> Option<String>, api: impl Fn(Config) -> DataApi<T> + 'static) -> Self {
        let env = ["MARKET_DATA_URL", "MARKET_DATA_TOKEN"].into_iter().filter_map(|name| env(name).map(|value| (name, value))).collect();
        Self { env, api: Box::new(api) }
    }
}

impl<T: Transport + 'static> Job for SeedAndSync<T> {
    fn spec(&self) -> Spec {
        Spec { name: NAME, scope: None, schedule: None, jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE * (SYNCS.len() as u32 + 1) }
    }

    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            let (env, started) = (self.env.clone(), cx.clock.now());
            let config = cx.db.run(move |c, cipher| {
                // mark_sync_in_progress
                if app_config::get(c, cipher, STATUS)?.as_deref() == Some("pending") {
                    app_config::set(c, cipher, STATUS, "in_progress", started)?;
                }
                let lookup = |name: &str| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone());
                data_api::config(&lookup, c, cipher)
            }).await;
            let failure = match config {
                Ok(Some(config)) => {
                    for name in SYNCS {
                        let sub = Cx { db: cx.db.clone(), clock: cx.clock, wakers: cx.wakers.clone() };
                        let outcome = reference::run_once(name, Some((self.api)(config.clone())), sub).await;
                        let at = cx.clock.now();
                        // Rails logs a failed sync and goes on; its record is the job's own.
                        let recorded = cx.db.run(move |c, _| match &outcome {
                            Outcome::Done => state::record_success(c, name, None, at),
                            Outcome::NothingNew => state::record_run(c, name, None, at),
                            Outcome::Failed(m) | Outcome::Transient(m) | Outcome::RateLimited(m) => state::record_error(c, name, None, at, m),
                        }).await;
                        if let Err(e) = recorded { crate::engine::log(&format!("[setup] could not record {name}: {e}")); }
                    }
                    None
                }
                Ok(None) => Some("the market-data provider is not deltabadger: the CoinGecko setup sync is not ported".to_string()),
                Err(e) => Some(e),
            };
            if let Some(reason) = &failure { crate::engine::log(&format!("[setup] Sync failed: {reason}")); }
            // mark_sync_completed (only from in_progress), or mark_sync_failed (always), then the broadcast.
            let (failed, ended) = (failure.is_some(), cx.clock.now());
            let completed = cx.db.run(move |c, cipher| {
                let in_progress = app_config::get(c, cipher, STATUS)?.as_deref() == Some("in_progress");
                if failed || in_progress { app_config::set(c, cipher, STATUS, "completed", ended)?; }
                Ok(failed || in_progress)
            }).await;
            match completed {
                Ok(true) => { cx.db.notifications.setup_synced(); Outcome::Done }
                Ok(false) => Outcome::NothingNew,
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}
