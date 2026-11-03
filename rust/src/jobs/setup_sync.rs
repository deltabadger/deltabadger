//! Setup::SeedAndSyncJob on the deltabadger provider, which the platform claim starts (web::setup): setup_sync_status
//! pending -> in_progress, the reference syncs its deltabadger branch runs (tickers of every venue, assets, indices,
//! stocks), then completed, and the "syncing" banner (`flash-syncing` on `settings_sync`) is removed.
//!
//! `load_seed` is not run: the install was seeded by db:prepare. On the coingecko provider it runs COINGECKO_SYNCS
//! (PLAN-A17 item 8); with neither provider the job only completes. The syncs run here with the data-api configuration as it is now (the
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

/// D-CG-SETUP-DURABLE: a setup run asked for (a CoinGecko key saved, a resync, a taken-over SeedAndSyncJob), as a plain
/// counter the job reads at start and clears at the end only if no newer request came meanwhile. Rails enqueues the job
/// in the request, and loses it if the process dies first; `setup_sync_status` keeps exactly Rails' writes.
pub const REQUEST: &str = "rust_job.setup_sync.request";

/// Ask for a setup run, in the caller's transaction; the caller wakes NAME after commit.
pub fn request(c: &rusqlite::Connection, now: chrono::DateTime<chrono::Utc>) -> Result<(), String> {
    let n = app_config::get_plain(c, REQUEST)?.and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    app_config::set_plain(c, REQUEST, &(n + 1).to_string(), now)
}

/// A committed claim survives process shutdown even if its in-memory wake did not. Only a persisted
/// connection with unfinished work is recovered; completed or never-connected installs stay idle.
pub fn unfinished(c: &rusqlite::Connection, cipher: &crate::crypto::Cipher) -> Result<bool, String> {
    if app_config::get_plain(c, REQUEST)?.is_some() { return Ok(true); }
    let connected = app_config::get(c, cipher, "platform_connected_at")?.is_some_and(|at| !crate::ruby::blank(&at));
    Ok(connected && matches!(app_config::get(c, cipher, STATUS)?.as_deref(), Some("pending" | "in_progress")))
}

pub struct SeedAndSync<T: Transport> {
    /// MARKET_DATA_URL and MARKET_DATA_TOKEN as the process was started with them.
    env: Vec<(&'static str, String)>,
    api: Box<dyn Fn(Config) -> DataApi<T>>,
    cg: super::coingecko::Clients,
}

impl<T: Transport> SeedAndSync<T> {
    pub fn new(env: &dyn Fn(&str) -> Option<String>, api: impl Fn(Config) -> DataApi<T> + 'static) -> Self {
        let env = ["MARKET_DATA_URL", "MARKET_DATA_TOKEN", "COINGECKO_API_KEY"].into_iter().filter_map(|name| env(name).map(|value| (name, value))).collect();
        Self { env, api: Box::new(api), cg: super::coingecko::live_clients() }
    }
    /// The CoinGecko clients a test hands.
    pub fn with_coingecko(mut self, cg: super::coingecko::Clients) -> Self { self.cg = cg; self }
}

/// What the CoinGecko branch runs, in order (PLAN-A17 item 8): the assets refresh (Rails' sync_assets_from_coingecko!),
/// then the indices job (D-CG-SETUP-RANKING: the stored ranking a Top Coins bot composes from). Venue tickers join in A17-2.
const COINGECKO_SYNCS: [&str; 2] = [reference::ASSETS, reference::INDICES];

enum Branch { DataApi(Config), CoinGecko, Neither }

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
                let request = app_config::get_plain(c, REQUEST)?;
                let branch = match data_api::config(&lookup, c, cipher)? {
                    Some(config) => Branch::DataApi(config),
                    None if crate::market_data::read(c, cipher, &crate::market_data::Env::read(&lookup))?.coingecko() => Branch::CoinGecko,
                    None => Branch::Neither,
                };
                Ok((branch, request))
            }).await;
            let (config, request) = match config { Ok((b, r)) => (Ok(b), r), Err(e) => (Err(e), None) };
            let mut keep_request = false;
            let failure = match config {
                Ok(Branch::CoinGecko) => {
                    for name in COINGECKO_SYNCS {
                        let sub = Cx { db: cx.db.clone(), clock: cx.clock, wakers: cx.wakers.clone() };
                        let outcome = reference::run_once_with(name, None::<DataApi<T>>, &self.cg, sub).await;
                        let limited = matches!(outcome, Outcome::RateLimited(_));
                        record(&cx, name, outcome).await;
                        // A closed quota: the later steps are not asked; their schedules catch up, and the request stays.
                        if limited { keep_request = true; break; }
                    }
                    None
                }
                Ok(Branch::DataApi(config)) => {
                    for name in SYNCS {
                        let sub = Cx { db: cx.db.clone(), clock: cx.clock, wakers: cx.wakers.clone() };
                        let outcome = reference::run_once(name, Some((self.api)(config.clone())), sub).await;
                        // Rails logs a failed sync and goes on; its record is the job's own.
                        record(&cx, name, outcome).await;
                    }
                    None
                }
                Ok(Branch::Neither) => Some("no market-data provider is configured: nothing to sync".to_string()),
                Err(e) => Some(e),
            };
            if let Some(reason) = &failure { crate::engine::log(&format!("[setup] Sync failed: {reason}")); }
            // mark_sync_completed (only from in_progress), or mark_sync_failed (always), then the broadcast.
            let (failed, ended) = (failure.is_some(), cx.clock.now());
            let completed = cx.db.run(move |c, cipher| {
                let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
                // The request this run served, unless a newer one came meanwhile or the run stopped on a closed quota: one
                // conditional delete, so a save between the read and the delete is never lost.
                if let (false, Some(served)) = (keep_request, &request) {
                    tx.execute("DELETE FROM app_configs WHERE key = ?1 AND value = ?2", [REQUEST, served.as_str()]).map_err(|e| e.to_string())?;
                }
                let in_progress = app_config::get(&tx, cipher, STATUS)?.as_deref() == Some("in_progress");
                if failed || in_progress { app_config::set(&tx, cipher, STATUS, "completed", ended)?; }
                tx.commit().map_err(|e| e.to_string())?;
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

/// A step's outcome under the step's own job name, as the scheduler would record it.
async fn record(cx: &Cx<'_>, name: &'static str, outcome: Outcome) {
    let at = cx.clock.now();
    let recorded = cx.db.run(move |c, _| match &outcome {
        Outcome::Done => state::record_success(c, name, None, at),
        Outcome::NothingNew => state::record_run(c, name, None, at),
        Outcome::Failed(m) | Outcome::Transient(m) | Outcome::RateLimited(m) => state::record_error(c, name, None, at, m),
    }).await;
    if let Err(e) = recorded { crate::engine::log(&format!("[setup] could not record {name}: {e}")); }
}
