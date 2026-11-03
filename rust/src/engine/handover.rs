//! Handover. Takeover: refuse an ineligible install, claim it, drop Rails' jobs for the bots
//! this engine now runs, unstick `executing`. Handback: every order accounted for, then — in ONE
//! SQLite transaction — every working bot `scheduled` and the handover row set to handed back.
use super::{eligibility, model, placement, Clock, EngineError};
use crate::crypto::Cipher;
use crate::enums::BotStatus;
use crate::lease::{self, Claim, EngineLock};
use crate::store::Opened;
use crate::venue::VenueFactory;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub struct Takeover { pub claim: Claim, pub eligible: Vec<i64>, pub deleted_jobs: usize, pub normalised: usize, pub imported_calls: usize,
    /// Rails queue rows this takeover dropped with nothing left to finish (D-UPGRADE-RAILS-WORK-DROPPED); `log`: its start-log lines.
    pub dropped_jobs: usize, pub log: Vec<String>,
    /// The bots and rules this start set aside (set_aside::apply), in the claim's own transaction.
    pub set_aside: Vec<super::set_aside::Aside>, pub rules_stopped: Vec<i64> }

/// What a takeover does with a Rails queue row of one class (PLAN-B2 r1.3, r2.5, r3.1-r3.3, r4.2-r4.7, r5 3-4, r6 fork A,
/// r7.2). Rails never runs again on a taken-over install, so every class it can leave has exactly one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailsWork {
    /// Deleted for every bot the engine owns, set-aside bots included; Rust's own tick, checks and rebalance decide again
    /// (D-UPGRADE-RAILS-CHECKS-REDONE). A claimed or failed one is ordinary work Rails left ambiguous: one activity line on
    /// its bot (D-UPGRADE-RAILS-AMBIGUITY-INHERITED). RebalanceJob's legs are behind Rails' own barrier (rebalancer.rb).
    Redone,
    /// Bot::StopJob: the pending amount-limit stop happens now, in the claim transaction, with its stop_message_key.
    Stop,
    /// Bot::SignalJob: a waiting call is imported; a claimed or failed one becomes the bot's unresolved marker
    /// (D-UPGRADE-CLAIMED-SIGNAL); an unreadable waiting one is deleted and logged (D-UPGRADE-UNREAD-SIGNAL).
    Signal,
    /// Bot::ResyncIndexCompositionJob: the bot's resync marker.
    Resync,
    /// A liquidation or redeploy command: deleted with one activity line on its bot; a surviving `placing` marker becomes
    /// the ambiguous halt (D-UPGRADE-COMMAND-DROPPED).
    Command,
    /// Bot::FetchAndCreateOrderJob, unfinished: a hard stop naming the bot and the order id (r6 fork A).
    FetchOrder,
    /// Rule::ExecuteJob and Rule::EvaluateAllJob (engine::rules): imported, refused, or (a claimed/failed fan-out)
    /// acknowledged.
    Rule,
    /// Journalled for its scope (the job's first argument: a user id, or an api key id for API_KEY_VALIDATOR) in
    /// RAILS_WAKES_KEY, and woken at every start until the Rust job of that name has run (r3.3, r4.4, r4.5).
    Wake(&'static str),
    /// An argumentless job (Asset::FetchAllAssetsDataFromCoingeckoJob, Setup::SeedAndSyncJob; Exchange::SyncTickersAndAssetsJob,
    /// whose one whole TICKERS run serves any venue's row): journalled unscoped (scope
    /// "") in RAILS_WAKES_KEY and woken at every start until the job's unscoped record shows a run (PLAN-A17 item 12).
    WakeUnscoped(&'static str),
    /// Ibkr::GenerateConnectionKeysJob: the key row's empty material is filled in the claim transaction, as the job does
    /// (two RSA-2048 keys and ffdhe2048, D-IBKR-DHGROUP); material already there is never replaced.
    IbkrKeys,
    /// Nothing to finish without Rails (mail not sent yet, page refreshes, syncs the scheduler runs on its own, a tax
    /// report the user asks for again): deleted and counted in the takeover's log line (D-UPGRADE-RAILS-WORK-DROPPED). So
    /// is a class missing from this table, which only a newer 2.x could have enqueued.
    Dropped,
}

/// Every job class 2.x can leave in its queue (r3.2, r4.5): each `class … < …` under app/jobs by its full name, each
/// perform_later receiver, config/recurring.yml's classes and its `command:` runner, and the mailer's job. Pinned against
/// the repository by tests/handover.rs `every_rails_job_class_has_an_outcome`.
pub const RAILS_WORK: &[(&str, RailsWork)] = {
    use RailsWork::*;
    &[
        ("Bot::ActionJob", Redone), ("Bot::BroadcastAfterScheduledActionJob", Redone), ("Bot::FetchAndUpdateOrderJob", Redone),
        ("Bot::FetchAndUpdateOpenOrdersJob", Redone), ("Bot::PriceLimitCheckJob", Redone), ("Bot::PriceDropLimitCheckJob", Redone),
        ("Bot::MovingAverageLimitCheckJob", Redone), ("Bot::IndicatorLimitCheckJob", Redone), ("Bot::RebalanceJob", Redone),
        ("Bot::StopJob", Stop),
        ("Bot::SignalJob", Signal),
        ("Bot::ResyncIndexCompositionJob", Resync),
        ("Bot::LiquidateExitedJob", Command), ("Bot::RedeployJob", Command), ("Bot::DeclineRedeployJob", Command),
        ("Bot::ResolveLiquidationJob", Command), ("Bot::ResolveRedeployJob", Command),
        ("Bot::FetchAndCreateOrderJob", FetchOrder),
        ("Rule::ExecuteJob", Rule), ("Rule::EvaluateAllJob", Rule),
        ("Tracker::LedgerJob", Wake(crate::tracker::jobs::TRACKER_LEDGER)), ("Tracker::LedgerJob::Retry", Wake(crate::tracker::jobs::TRACKER_LEDGER)),
        ("PortfolioSnapshot::BackfillJob", Wake(crate::tracker::jobs::PORTFOLIO_BACKFILL)),
        ("ApiKeyValidatorJob", Wake(crate::sync::jobs::API_KEY_VALIDATOR)),
        ("Ibkr::GenerateConnectionKeysJob", IbkrKeys),
        // Abstract bases: never enqueued themselves.
        ("ApplicationJob", Dropped), ("BotJob", Dropped), ("Bot::LimitCheckJobBase", Dropped),
        // Mail Rails had not sent yet.
        ("ActionMailer::MailDeliveryJob", Dropped), ("ApplicationMailDeliveryJob", Dropped),
        // Page refreshes and metrics caches: the web renders from the data.
        ("Bot::BroadcastIndicatorLimitInfoUpdateJob", Dropped), ("Bot::BroadcastMetricsUpdateJob", Dropped),
        ("Bot::BroadcastMovingAverageLimitInfoUpdateJob", Dropped), ("Bot::BroadcastPnlUpdateJob", Dropped),
        ("Bot::BroadcastPriceDropLimitInfoUpdateJob", Dropped), ("Bot::BroadcastPriceLimitInfoUpdateJob", Dropped),
        ("User::BroadcastGlobalPnlUpdateJob", Dropped), ("Bot::UpdateMetricsJob", Dropped), ("Bot::ExpireRestatedMetricsJob", Dropped),
        ("Bot::WarmMetricsCachesJob", Dropped),
        // Syncs and maintenance the scheduler runs on its own schedule, or that have no Rust counterpart to finish.
        ("AccountBalance::SyncAllJob", Dropped), ("AccountBalance::SyncJob", Dropped), ("AccountTransaction::ResolveAssetsJob", Dropped),
        ("AccountTransaction::SyncAllJob", Dropped), ("AccountTransaction::SyncJob", Dropped), ("AccountTransaction::SyncTrackerJob", Dropped),
        ("AppUpdate::CheckJob", WakeUnscoped(crate::jobs::app_update::NAME)), ("Asset::FetchAllAssetsDataFromCoingeckoJob", WakeUnscoped(crate::jobs::reference::ASSETS)),
        ("Asset::FetchDataFromCoingeckoJob", Wake(crate::jobs::reference::ASSET_DATA)),
        ("Asset::InferColorFromImageJob", Wake(crate::jobs::asset_color::NAME)), ("Asset::SyncAlpacaCryptoFromDeltabadgerJob", Dropped),
        ("Asset::SyncStocksFromDeltabadgerJob", Dropped), ("BotActivityLog::PruneJob", Dropped), ("Bot::ConvertSingleAssetBotsJob", Dropped),
        ("Bot::EvaluateRebalancersJob", Dropped), ("Bot::RepairOrphanedBotsJob", Dropped), ("Exchange::SyncAllTickersAndAssetsJob", Dropped),
        ("Exchange::SyncAlpacaAssetsJob", Dropped), ("Exchange::SyncTickersAndAssetsJob", WakeUnscoped(crate::jobs::reference::TICKERS)), ("Ibkr::CheckActivationJob", Dropped),
        ("Index::SyncFromCoingeckoJob", Dropped), ("Setup::SeedAndSyncJob", WakeUnscoped(crate::jobs::setup_sync::NAME)), ("SolidQueue::RecurringJob", Dropped),
        // r7.2: the report page shows nothing in progress; the user asks again.
        ("Tax::GenerateReportJob", Dropped),
    ]
};

/// A queue row's outcome by its class_name: a class not in RAILS_WORK is dropped.
pub fn rails_work(class: &str) -> RailsWork {
    RAILS_WORK.iter().find(|(name, _)| *name == class).map_or(RailsWork::Dropped, |(_, work)| *work)
}

fn first_gid(arguments: &str) -> Option<String> {
    let v: Value = serde_json::from_str(arguments).ok()?;
    let first = v.get("arguments")?.get(0)?;
    first.get("_aj_globalid").and_then(Value::as_str).or_else(|| first.as_str()).map(str::to_string)
}

/// Automation::Schedulable#active_job?: a `class` job for the record `gid` in a scheduled, ready, claimed or blocked execution
/// (a failed one is over), its arguments matched as #job_matches_record? matches them (any argument, GlobalID or bare string).
pub fn active_job(queue: &Connection, class: &str, gid: &str) -> rusqlite::Result<bool> {
    let live = ["solid_queue_scheduled_executions", "solid_queue_ready_executions", "solid_queue_claimed_executions", "solid_queue_blocked_executions"]
        .map(|table| format!("EXISTS (SELECT 1 FROM {table} e WHERE e.job_id = j.id)")).join(" OR ");
    let mut s = queue.prepare(&format!("SELECT j.arguments FROM solid_queue_jobs j WHERE j.class_name = ?1 AND j.finished_at IS NULL AND NOT EXISTS (SELECT 1 FROM solid_queue_failed_executions f WHERE f.job_id=j.id) AND ({live})"))?;
    let jobs = s.query_map([class], |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(jobs.iter().flatten().any(|arguments| {
        let Ok(v) = serde_json::from_str::<Value>(arguments) else { return false };
        let args = if v.is_object() { v.get("arguments").cloned().unwrap_or(Value::Null) } else { v };
        args.as_array().is_some_and(|args| args.iter().any(|arg| arg.get("_aj_globalid").and_then(Value::as_str).or_else(|| arg.as_str()) == Some(gid)))
    }))
}

/// An engine-owned install keeps only check_install's refusals on restart. A takeover from Rails adds the flip-rule
/// check and notes stale reference data without refusing it (check_install_at, D-UPGRADE-FRESHNESS-AT-TICK): the scheduler
/// that refreshes it starts only after the claim. The tick refuses a stale bot in both cases.
pub fn startup_report(c: &rusqlite::Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<eligibility::Report, EngineError> {
    if lease::read(c, cipher)?.is_some_and(|v| v["engine"] == "rust") {
        eligibility::check_install(c)
    } else {
        eligibility::check_install_at(c, now)
    }
}

pub fn take_over(lock: &EngineLock, o: &Opened, cipher: &Cipher, version: &str, now: DateTime<Utc>) -> Result<Takeover, EngineError> {
    super::provider::bind_cipher(&o.primary,cipher)?;
    let problems = super::rules::takeover_problems(&o.primary,&o.queue)?;
    if !problems.is_empty() { return Err(EngineError::Ineligible(problems)); }
    // Rails' queue, read and judged before anything changes owner (PLAN-B2 r1.3, r3.1, r4.2-r4.7, r5, r6 fork A). A row a
    // takeover already acted on (QUEUE_DONE_KEY) is only deleted again. What may hold an order nothing records is a hard
    // stop here, before anything is written.
    // Every bot's class, read before anything is written: a row without one fails the takeover here, never after the claim.
    let mut s = o.primary.prepare("SELECT type FROM bots")?;
    for class in s.query_map([], |r| r.get::<_, String>(0))? { class?; }
    drop(s);
    let done_before = queue_done(&o.primary)?;
    let (signal_jobs, unread_signals) = signal_jobs(o, &done_before)?;
    let mut work = queue_work(o, cipher, &done_before)?;
    for (row, line) in unread_signals { work.done.push(row); work.log.push(line); }
    for row in super::rules::unreadable_jobs(&o.primary, &o.queue)? {
        work.hard.push(format!("A withdrawal rule's queued job can't be read: support must remove queue row {row}."));
        work.details.push(format!("solid_queue_jobs {row}: Rule job without an active job id or a readable rule"));
    }
    if !work.hard.is_empty() {
        return Err(EngineError::HardStop(super::set_aside::hard_stop(&super::set_aside::Hard { lines: work.hard, details: work.details })));
    }
    work.rails_ambiguous = rails_ambiguous(&o.primary)?;
    // Ibkr::GenerateConnectionKeysJob's material, made before the claim's write lock is taken (RSA takes a moment).
    let ibkr: Vec<(i64, [String; 3])> = work.ibkr.iter().map(|id| {
        use crate::venue::signers::ibkr as signer;
        (*id, [signer::generate_rsa().0, signer::generate_rsa().0, signer::ffdhe2048_pem()].map(|pem| cipher.encrypt(&pem)))
    }).collect();
    let mut rule_jobs=super::rules::jobs(&o.primary,&o.queue)?;
    // Claim first, with the startup data steps: once committed, Rails refuses, and a failure below is
    // repaired by the next start, which repeats the deletes and the normalisation idempotently.
    let tx = model::immediate(&o.primary)?;
    // Bot::StopJob (r1.3): the amount-limit stop Rails had queued happens now, before the check, as Rails would have run it.
    for (bot, key) in &work.stops { super::tick::stop_for_rails_job(&tx, *bot, key.as_deref(), now)?; }
    // PLAN-B2 R3 (D-UPGRADE-SET-ASIDE): before the claim, every working bot or rule this build refuses is stopped with its
    // reasons, and the check runs again on this transaction. What it still refuses is a hard stop: returning drops `tx`,
    // so nothing here is written. The bots the engine then runs are the second check's.
    let plan = match super::set_aside::apply(&tx, Some(cipher), &|c| startup_report(c, cipher, now), now)? {
        Ok(plan) => plan,
        Err(hard) => return Err(EngineError::HardStop(super::set_aside::hard_stop(&hard))),
    };
    let report = plan.report;
    // D-KUCOIN-HF-RETENTION: before the claim moves the takeover stamp, the open KuCoin rows it adopts are marked.
    super::reconcile::mark_adopted(&tx, cipher, now)?;
    let claim = lease::claim(lock, &tx, cipher, version, now)?;
    // In the claim's own transaction, before the engine ticks or the web serves a request: every unresolved intent
    // carries what it was sent under (placement::backfill_snapshots).
    placement::backfill_snapshots(&tx)?;
    crate::sync::coinbase_repair::run(&tx, now)?;
    // From here on a reference source is as fresh as its job's completed runs; a job this engine has not run yet starts
    // from Rails' baseline, read once, here, in the commit that closes Rails' ownership window. A restart reseeds nothing.
    super::staleness::seed(&tx, now)?;
    let imported_calls = import_signal_jobs(&tx, &signal_jobs, now)?;
    apply_queue_work(&tx, &work, matches!(claim, Claim::FromRails), now)?;
    import_wakes(&tx, &work.wakes, now)?;
    for (id, material) in &ibkr { fill_ibkr_keys(&tx, cipher, *id, material, now)?; }
    super::rules::acknowledge_fanouts(&tx, &mut rule_jobs, now)?;
    super::rules::import(&tx,&rule_jobs,now)?;
    let mut done = done_before;
    done.extend(work.done.iter().filter(|row| !done.contains(row)).copied().collect::<Vec<_>>());
    write_queue_done(&tx, &done, now)?;
    // Before the commit: once it lands, a crash would skip these rows for good, so the line must already be out. A failed
    // commit repeats it next start, which is harmless.
    for line in &work.log { super::log(line); }
    tx.commit()?;
    // A crash here leaves the outcomes committed and Rails' rows queued: the next start deletes them and repeats nothing.
    #[cfg(debug_assertions)]
    crate::store::abort_point("after-queue-commit");
    let deleted_signal_jobs = finish_signal_import(o, now)? + super::rules::finish_cleanup(&o.primary,&o.queue,now)? + finish_queue_work(o, now)?;

    // Bot::ResyncIndexCompositionJob rows Rails has not finished (ready, scheduled, blocked or claimed; a failed one is over),
    // for every index bot neither deleted nor archived that has an exchange, whatever its status: each becomes the bot's
    // resync marker, committed before the rows go, so a crash between the two marks again on the next takeover. A claimed
    // row of a dead Rails process is included: the resync runs at least once.
    let mut s = o.primary.prepare(&format!("SELECT id FROM bots WHERE type = 'Bots::DcaIndex' AND status NOT IN ({}, {}) AND exchange_id IS NOT NULL",
                                           BotStatus::Deleted as i64, BotStatus::Archived as i64))?;
    let index_gids: Vec<(i64, String)> = s.query_map([], |r| r.get::<_, i64>(0))?.map(|id| id.map(|id| (id, format!("gid://deltabadger/Bots::DcaIndex/{id}"))))
        .collect::<Result<_, _>>()?;
    drop(s);
    let mut s = o.queue.prepare("SELECT j.id, j.arguments FROM solid_queue_jobs j WHERE j.finished_at IS NULL AND j.class_name = 'Bot::ResyncIndexCompositionJob' \
                                 AND NOT EXISTS (SELECT 1 FROM solid_queue_failed_executions f WHERE f.job_id = j.id)")?;
    let resyncs: Vec<(i64, Option<String>)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    drop(s);
    let resyncs: Vec<(i64, i64)> = resyncs.into_iter().filter_map(|(job, args)| {
        let gid = args.as_deref().and_then(first_gid)?;
        index_gids.iter().find(|(_, g)| *g == gid).map(|(bot, _)| (job, *bot))
    }).collect();
    let tx = model::immediate(&o.primary)?;
    for (_, bot) in &resyncs { super::index::mark_resync(&tx, *bot, now)?; }
    tx.commit()?;
    for (job, _) in &resyncs { o.queue.execute("DELETE FROM solid_queue_jobs WHERE id = ?1", [job])?; } // executions cascade

    let tx = model::immediate(&o.primary)?;
    let mut normalised = 0;
    for id in &report.eligible {
        normalised += model::unstick(&tx, *id, now)? as usize; // the last run (Rails' or ours) was cut short
    }
    tx.commit()?;
    Ok(Takeover { claim, eligible: report.eligible, deleted_jobs: resyncs.len() + deleted_signal_jobs, dropped_jobs: work.dropped, normalised, imported_calls,
                  set_aside: plan.bots, rules_stopped: plan.rules, log: work.log })
}

// ---- Rails' queue at a takeover (PLAN-B2 B2-6) ----------------------------------------------------------------------------

/// app_configs key (the import-set pattern of SIGNAL_IMPORT_KEY): the queue rows (solid_queue_jobs.id) whose outcome a
/// takeover committed with its claim and that may still be queued (the queue is its own database). A start skips them and
/// deletes them again, then the ids whose rows are gone leave the set, so a crash between the commit and the delete never
/// stops, marks or logs twice. Rails is gone, so nothing enqueues a row in between.
pub const QUEUE_DONE_KEY: &str = "rust_rails_queue_done";

fn queue_done(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let Some(text) = crate::app_config::get_plain(c, QUEUE_DONE_KEY).map_err(EngineError::Data)? else { return Ok(vec![]) };
    serde_json::from_str(&text).map_err(|e| EngineError::Data(format!("{QUEUE_DONE_KEY}: {e}")))
}

fn write_queue_done(c: &Connection, rows: &[i64], now: DateTime<Utc>) -> Result<(), EngineError> {
    if rows.is_empty() { return crate::app_config::delete(c, QUEUE_DONE_KEY).map_err(EngineError::Data); }
    crate::app_config::set_plain(c, QUEUE_DONE_KEY, &serde_json::to_string(rows).expect("ids serialise"), now).map_err(EngineError::Data)
}

/// After the claim committed: the rows in QUEUE_DONE_KEY leave the queue (their executions cascade), then the set keeps only
/// the rows still there. Returns the rows deleted.
fn finish_queue_work(o: &Opened, now: DateTime<Utc>) -> Result<usize, EngineError> {
    let done = queue_done(&o.primary)?;
    if done.is_empty() { return Ok(0); }
    let mut deleted = 0;
    let mut left = vec![];
    for row in done {
        deleted += o.queue.execute("DELETE FROM solid_queue_jobs WHERE id = ?1", [row])?;
        let there: bool = o.queue.query_row("SELECT EXISTS (SELECT 1 FROM solid_queue_jobs WHERE id = ?1)", [row], |r| r.get(0))?;
        if there { left.push(row); }
    }
    let tx = model::immediate(&o.primary)?;
    write_queue_done(&tx, &left, now)?;
    tx.commit()?;
    Ok(deleted)
}

/// A queue row Rails had not finished, and where it stood: waiting (ready, scheduled, blocked or none), claimed by a
/// process that died, or failed (Solid Queue fails a dead process's claims when it prunes it).
struct QueueRow { id: i64, job: Option<String>, class: String, arguments: Option<String>, ambiguous: bool }

fn queue_rows(q: &Connection) -> Result<Vec<QueueRow>, EngineError> {
    let mut s = q.prepare("SELECT j.id, j.active_job_id, j.class_name, j.arguments, \
                           EXISTS (SELECT 1 FROM solid_queue_claimed_executions e WHERE e.job_id = j.id) OR EXISTS (SELECT 1 FROM solid_queue_failed_executions e WHERE e.job_id = j.id) \
                           FROM solid_queue_jobs j WHERE j.finished_at IS NULL ORDER BY j.id")?;
    let rows = s.query_map([], |r| Ok(QueueRow { id: r.get(0)?, job: r.get(1)?, class: r.get(2)?, arguments: r.get(3)?, ambiguous: r.get(4)? }))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// The job's `index`th argument as ActiveJob serialized it.
fn argument(arguments: Option<&str>, index: usize) -> Option<Value> {
    let v: Value = serde_json::from_str(arguments?).ok()?;
    v.get("arguments")?.get(index).cloned()
}

/// The bot a job's first argument names (`gid://deltabadger/Bots::<class>/<id>`), when it is one.
fn bot_of(arguments: Option<&str>, class: Option<&str>) -> Option<i64> {
    let gid = arguments.and_then(first_gid)?;
    let (ty, id) = gid.strip_prefix("gid://deltabadger/")?.rsplit_once('/')?;
    if !ty.starts_with("Bots::") || class.is_some_and(|c| c != ty) { return None; }
    id.parse().ok()
}

/// A bot row that is there and not deleted: Rails' GlobalID finds a deleted bot too, but nothing of it runs again.
fn live_bot(c: &Connection, id: i64) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM bots WHERE id = ?1 AND status <> ?2)", params![id, BotStatus::Deleted as i64], |r| r.get(0))?)
}

fn bot_row(c: &Connection, id: i64) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM bots WHERE id = ?1)", [id], |r| r.get(0))?)
}

/// A claimed or failed Bot::SignalJob on a signal bot with no marker of its own (r4.2, D-UPGRADE-CLAIMED-SIGNAL).
/// `triggered_at`: the call's time, or why it can't be read (kept in the line's details).
struct Unresolved { bot: i64, marker_id: String, side: Option<crate::venue::Side>, triggered_at: Result<DateTime<Utc>, String> }

/// The DCA bots whose newest activity line is Rails' own placement_ambiguous, read before the takeover writes any line.
fn rails_ambiguous(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare("SELECT b.id FROM bots b WHERE b.type <> ?1 AND b.status <> ?2 AND (SELECT l.event FROM bot_activity_logs l WHERE l.bot_id = b.id \
                           ORDER BY l.created_at DESC, l.id DESC LIMIT 1) = 'placement_ambiguous' ORDER BY b.id")?;
    let ids = s.query_map(params![model::SIGNAL_TYPE, BotStatus::Deleted as i64], |r| r.get::<_, i64>(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// What the claim transaction does with Rails' queue, and what it deletes after the commit.
#[derive(Default)]
struct QueueWork {
    /// The rows acted on (journalled in QUEUE_DONE_KEY, deleted after the commit).
    done: Vec<i64>,
    /// Of those, the rows dropped (D-UPGRADE-RAILS-WORK-DROPPED).
    dropped: usize,
    stops: Vec<(i64, Option<String>)>,
    signals: Vec<Unresolved>,
    /// (bot, class, claimed or failed).
    commands: Vec<(i64, String, bool)>,
    /// Bots of a claimed or failed Bot::ActionJob (C12), and (rails_ambiguous) those Rails' last line left ambiguous.
    unrecorded: Vec<i64>,
    rails_ambiguous: Vec<i64>,
    /// (job, scope) of the wake rows, one per pair (RAILS_WAKES_KEY); the IBKR keys whose material is filled.
    wakes: Vec<(&'static str, Option<i64>)>,
    ibkr: Vec<i64>,
    /// Start-log lines.
    log: Vec<String>,
    hard: Vec<String>,
    details: Vec<String>,
}

/// Every unfinished row's outcome by its class (RAILS_WORK), except the waiting signal calls (signal_jobs), rule jobs
/// (engine::rules) and resyncs, which are judged on their own.
fn queue_work(o: &Opened, cipher: &Cipher, skip: &[i64]) -> Result<QueueWork, EngineError> {
    let c = &o.primary;
    let mut w = QueueWork::default();
    for row in queue_rows(&o.queue)?.into_iter().filter(|r| !skip.contains(&r.id)) {
        let args = row.arguments.as_deref();
        match rails_work(&row.class) {
            RailsWork::Redone => {
                w.done.push(row.id);
                // D-UPGRADE-RAILS-AMBIGUITY-INHERITED: an ActionJob a dead process held may have sent an order Rails never
                // recorded, as Rails' own ambiguity does (action_job.rb's KNOWN GAP); its bot carries one line (C12).
                if row.class == "Bot::ActionJob" && row.ambiguous {
                    if let Some(bot) = bot_of(args, None).filter(|b| !w.unrecorded.contains(b)) { if live_bot(c, bot)? { w.unrecorded.push(bot); } }
                }
            }
            RailsWork::Stop => {
                w.done.push(row.id);
                let key = argument(args, 1).and_then(|k| k.get("stop_message_key").and_then(Value::as_str).map(str::to_string));
                match bot_of(args, None) {
                    Some(bot) => w.stops.push((bot, key)),
                    None => w.log.push(format!("dropped Bot::StopJob (queue row {}): it names no bot this build can read", row.id)),
                }
            }
            RailsWork::Signal if row.ambiguous => unresolved_signal(c, &row, &mut w)?,
            RailsWork::Command => {
                w.done.push(row.id);
                match bot_of(args, None) {
                    Some(bot) if live_bot(c, bot)? => w.commands.push((bot, row.class.clone(), row.ambiguous)),
                    Some(_) => {}
                    None => w.log.push(format!("dropped {} (queue row {}): it names no bot this build can read", row.class, row.id)),
                }
            }
            RailsWork::FetchOrder => {
                // r6 fork A: Rails placed this order and never recorded it; the job holds its id. Nothing here records it.
                let order = argument(args, 1).map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).unwrap_or_default();
                match bot_of(args, None) {
                    // A soft-deleted bot's order is still on the exchange: only a bot whose row is gone has nothing to record it in.
                    Some(bot) if !bot_row(c, bot)? => { w.done.push(row.id); w.dropped += 1; }
                    Some(bot) => {
                        w.hard.push(format!("{}: order {order} was placed on your exchange before the update but isn't recorded here yet.", super::set_aside::name(c, bot)?));
                        w.details.push(format!("solid_queue_jobs {}: Bot::FetchAndCreateOrderJob bot {bot} order {order}", row.id));
                    }
                    None => {
                        w.hard.push(format!("An order was placed on your exchange before the update but isn't recorded here yet, and its bot can't be read (queue row {}).", row.id));
                        w.details.push(format!("solid_queue_jobs {}: Bot::FetchAndCreateOrderJob with arguments this build does not read", row.id));
                    }
                }
            }
            RailsWork::Wake(job) => {
                // Waiting, claimed or failed alike: every one of these jobs is safe to run again. Its argument is a plain
                // integer (ActiveJob serializes an Integer as itself); a row naming nothing there is Rails' RecordNotFound.
                w.done.push(row.id);
                let (table, noun) = wake_scope(job);
                // Asset::FetchDataFromCoingeckoJob and Asset::InferColorFromImageJob are enqueued with the Asset itself
                // (sync_alpaca_assets_job.rb:166, market_data.rb:86): a GlobalID.
                let id = if job == crate::jobs::reference::ASSET_DATA || job == crate::jobs::asset_color::NAME { asset_of(args) } else { first_id(args) };
                match id {
                    Some(id) if exists(c, table, id)? => { if !w.wakes.contains(&(job, Some(id))) { w.wakes.push((job, Some(id))); } }
                    _ => {
                        w.dropped += 1;
                        w.log.push(format!("dropped {} (queue row {}): it names no {noun} this install has", row.class, row.id));
                    }
                }
            }
            RailsWork::WakeUnscoped(job) => {
                w.done.push(row.id);
                if !w.wakes.contains(&(job, None)) { w.wakes.push((job, None)); }
            }
            RailsWork::IbkrKeys => {
                // Ibkr::GenerateConnectionKeysJob#perform: nothing for a key that is gone or already has its material.
                w.done.push(row.id);
                match first_id(args) {
                    Some(id) if ibkr_blank(c, cipher, id)? == Some(true) => { if !w.ibkr.contains(&id) { w.ibkr.push(id); } }
                    _ => w.dropped += 1,
                }
            }
            RailsWork::Dropped => { w.done.push(row.id); w.dropped += 1; }
            RailsWork::Signal | RailsWork::Resync | RailsWork::Rule => {}
        }
    }
    Ok(w)
}

/// r1.2, r4.2, r5-4, r6 fork A: a claimed or failed Bot::SignalJob may have placed its order. It never takes the discard
/// path: it becomes its bot's unresolved marker (written by apply_queue_work), unless the bot is gone (deleted: nothing of
/// it runs again), already holds a marker or an unresolved intent of its own, or cannot be read: those two are hard stops.
fn unresolved_signal(c: &Connection, row: &QueueRow, w: &mut QueueWork) -> Result<(), EngineError> {
    let args = row.arguments.as_deref();
    let hard = |w: &mut QueueWork, line: String, detail: &str| { w.hard.push(line); w.details.push(format!("solid_queue_jobs {}: Bot::SignalJob {detail}", row.id)); };
    let unread = format!("A webhook call may have placed an order before the update, and the bot it belongs to can't be read (queue row {}).", row.id);
    let Some(bot) = bot_of(args, Some(model::SIGNAL_TYPE)) else { hard(w, unread, "names no signal bot this build reads"); return Ok(()) };
    if !live_bot(c, bot)? {
        w.done.push(row.id);
        w.log.push(format!("dropped a webhook call of deleted bot {bot} (queue row {})", row.id));
        return Ok(());
    }
    let loaded = match model::load_bot(c, bot) { Ok(b) => b, Err(e) => { hard(w, unread, &format!("bot {bot}: {e:?}")); return Ok(()) } };
    if super::signal::blocked(&loaded) {
        hard(w, format!("{}: a webhook call may have placed an order before the update, and this bot already has another order whose result isn't confirmed.",
                        super::set_aside::name(c, bot)?), &format!("bot {bot} already holds a marker or an unresolved intent"));
        return Ok(());
    }
    let rule = argument(args, 1).and_then(|g| g.get("_aj_globalid").and_then(Value::as_str).and_then(|g| g.strip_prefix("gid://deltabadger/BotSignal/")).and_then(|n| n.parse::<i64>().ok()));
    let side = match rule { Some(id) => model::signal(c, id)?.filter(|r| r.bot_id == bot).map(|r| r.side), None => None };
    let triggered_at = match argument(args, 2).and_then(|t| t.get("value").and_then(Value::as_str).map(str::to_string)) {
        Some(v) => crate::codec::parse_time(&v).map_err(|e| format!("{v}: {e:?}")),
        None => Err("no triggered_at".to_string()),
    };
    let marker_id = format!("rails-job:{}", row.job.clone().unwrap_or_else(|| format!("queue-row-{}", row.id)));
    w.done.push(row.id);
    w.signals.push(Unresolved { bot, marker_id, side, triggered_at });
    Ok(())
}

/// The claim transaction's part of the queue work, after the claim (the stops ran before the check).
fn apply_queue_work(tx: &Connection, w: &QueueWork, from_rails: bool, now: DateTime<Utc>) -> Result<(), EngineError> {
    use super::amount::TxKind;
    use model::Level;
    let mut marked: Vec<i64> = vec![];
    for u in &w.signals {
        // One marker per bot, the first job's (Bots::Signal#mark_placement_unresolved!'s shape); `side` only when its rule is
        // still there. The owner's one confirmation covers every call listed beside it.
        if !marked.contains(&u.bot) {
            let mut marker = json!({ "id": u.marker_id, "at": now.format("%Y-%m-%dT%H:%M:%SZ").to_string(), "error": "Deltabadger stopped while placing it" });
            if let Some(side) = u.side { marker["side"] = json!(side.as_str()); }
            model::merge_transient_compact(tx, u.bot, &[(super::signal::MARKER_KEY, marker)])?;
            marked.push(u.bot);
        }
        let (when, unread) = match &u.triggered_at {
            Ok(t) => (format!("at {}", t.format("%Y-%m-%d %H:%M:%S UTC")), None),
            Err(e) => ("at a time that can't be read".to_string(), Some(e.clone())),
        };
        let call = match u.side { Some(side) => format!("{} {when}", side.as_str()), None => when };
        model::log_activity(tx, u.bot, UPGRADE_SIGNAL_UNRESOLVED, Level::Warning, json!({ "call": call, "marker_id": u.marker_id, "triggered_at_error": unread }), now)?;
    }
    for (bot, class, ambiguous) in &w.commands {
        // r4.3: Rails persisted the command's intent before it sent anything; a surviving `placing` marker becomes the
        // ambiguous halt as the engine promotes one (composition::promote_orphan). Nothing moves money.
        let kind = if class.contains("Liquidat") { TxKind::Liquidation } else { TxKind::Redeploy };
        let marker = super::composition::name(kind).is_some_and(|name| model::load_bot(tx, *bot).is_ok_and(|b| b.transient.get(format!("{name}_pending")).is_some_and(Value::is_object)));
        if *ambiguous && marker {
            super::composition::promote_orphan(tx, *bot, kind, now)?;
        } else {
            // D-UPGRADE-COMMAND-DROPPED (C4).
            model::log_activity(tx, *bot, UPGRADE_COMMAND_DROPPED, Level::Warning, json!({ "action": command_action(class), "job": class }), now)?;
        }
    }
    // C12: a claimed or failed ActionJob, and, at the first takeover from Rails, a DCA bot whose last line is Rails' own
    // placement_ambiguous (the order Rails' KNOWN GAP leaves unrecorded).
    let mut bots = w.unrecorded.clone();
    if from_rails { for id in &w.rails_ambiguous { if !bots.contains(id) { bots.push(*id); } } }
    for bot in bots { model::log_activity(tx, bot, UPGRADE_ORDER_UNRECORDED, Level::Warning, json!({}), now)?; }
    Ok(())
}

// ---- Rails' wakes and IBKR keys (PLAN-B2 r2.5, r3.3, r4.4, r4.5) ------------------------------------------------------

/// app_configs key: the Rust jobs a takeover owes a run, one entry per (job, scope) of a Rails queue row it took (Tracker::
/// LedgerJob and its ::Retry, PortfolioSnapshot::BackfillJob, ApiKeyValidatorJob), each with the takeover's time. Written in
/// the claim transaction, beside the rows' ids in QUEUE_DONE_KEY (so a crash before their delete imports nothing twice). At
/// every start the scheduler wakes each entry (due_rails_wakes) until that job's state for that scope shows a run at or
/// after the entry's time; a run that failed leaves it for the next start.
/// D-UPGRADE-RAILS-WAKES: Rails ran such a job once, with its own retries; here it runs at least once after the takeover,
/// and one whose run keeps failing (a key the validator doesn't cover, a backfill without market data) is woken again at
/// every start, as a manual wake of it would be. Duplicates of one job and scope run once.
pub const RAILS_WAKES_KEY: &str = "rust_rails_wakes";

#[derive(serde::Serialize, serde::Deserialize)]
struct RailsWake { job: String, scope: String, at: String }

/// The table a wake job's scope is a row of, and its name in a log line.
fn wake_scope(job: &str) -> (&'static str, &'static str) {
    if job == crate::sync::jobs::API_KEY_VALIDATOR { ("api_keys", "API key") }
    else if job == crate::jobs::reference::ASSET_DATA || job == crate::jobs::asset_color::NAME { ("assets", "asset") }
    else { ("users", "user") }
}

/// The asset a job's first argument names (`gid://deltabadger/Asset/<id>`), when it is one.
fn asset_of(arguments: Option<&str>) -> Option<i64> {
    let gid = arguments.and_then(first_gid)?;
    gid.strip_prefix("gid://deltabadger/Asset/")?.parse().ok().filter(|id| *id > 0)
}

/// The job's first argument as a positive integer id.
fn first_id(arguments: Option<&str>) -> Option<i64> { argument(arguments, 0)?.as_i64().filter(|id| *id > 0) }

fn exists(c: &Connection, table: &str, id: i64) -> Result<bool, EngineError> {
    Ok(c.query_row(&format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE id = ?1)"), [id], |r| r.get(0))?)
}

/// `api_key.rsa_signature_key.blank?` of key `id` (None: no such key). Material this secret can't open is not blank: it
/// may be what the user uploaded to IBKR, and is never replaced (D-IBKR-RECONNECT keeps it).
fn ibkr_blank(c: &Connection, cipher: &Cipher, id: i64) -> Result<Option<bool>, EngineError> {
    let signing: Option<Option<String>> = c.query_row("SELECT rsa_signature_key FROM api_keys WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    Ok(signing.map(|v| v.is_none_or(|v| crate::ruby::blank(&v) || cipher.decrypt(&v).is_ok_and(|p| crate::ruby::blank(&p)))))
}

/// The job's update!, only while the row is still blank (a key made meanwhile keeps its own).
fn fill_ibkr_keys(tx: &Connection, cipher: &Cipher, id: i64, [signing, encryption, dh]: &[String; 3], now: DateTime<Utc>) -> Result<(), EngineError> {
    if ibkr_blank(tx, cipher, id)? != Some(true) { return Ok(()); }
    tx.execute("UPDATE api_keys SET rsa_signature_key = ?1, rsa_encryption_key = ?2, dh_param = ?3, updated_at = ?4 WHERE id = ?5",
               params![signing, encryption, dh, crate::codec::format_time(now), id])?;
    Ok(())
}

fn rails_wakes(c: &Connection) -> Result<Vec<RailsWake>, EngineError> {
    let Some(text) = crate::app_config::get_plain(c, RAILS_WAKES_KEY).map_err(EngineError::Data)? else { return Ok(vec![]) };
    serde_json::from_str(&text).map_err(|e| EngineError::Data(format!("{RAILS_WAKES_KEY}: {e}")))
}

fn write_rails_wakes(c: &Connection, wakes: &[RailsWake], now: DateTime<Utc>) -> Result<(), EngineError> {
    if wakes.is_empty() { return crate::app_config::delete(c, RAILS_WAKES_KEY).map_err(EngineError::Data); }
    crate::app_config::set_plain(c, RAILS_WAKES_KEY, &serde_json::to_string(wakes).expect("wakes serialise"), now).map_err(EngineError::Data)
}

/// In the claim transaction: each (job, scope) is journalled at `now`. The time is in the job
/// state's own precision (jobs::state), so a run recorded after it never reads as earlier.
fn import_wakes(tx: &Connection, wakes: &[(&'static str, Option<i64>)], now: DateTime<Utc>) -> Result<(), EngineError> {
    if wakes.is_empty() { return Ok(()); }
    let mut all = rails_wakes(tx)?;
    let at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    for (job, id) in wakes {
        let scope = id.map_or_else(String::new, |id| id.to_string());
        // A taken-over SeedAndSyncJob is a setup request too (D-CG-SETUP-DURABLE), so it survives as one.
        if *job == crate::jobs::setup_sync::NAME { crate::jobs::setup_sync::request(tx, now).map_err(EngineError::Data)?; }
        // A row taken now is new work even beside an entry still journalled (Rails queued it after a handback): its time is
        // this takeover's. A crash replay never gets here twice for one row (QUEUE_DONE_KEY skips it).
        match all.iter_mut().find(|w| w.job == *job && w.scope == scope) {
            Some(w) => w.at.clone_from(&at),
            None => all.push(RailsWake { job: job.to_string(), scope, at: at.clone() }),
        }
    }
    write_rails_wakes(tx, &all, now)
}

/// At every start, before the scheduler runs: the journal's (job, scope) pairs to wake once. An entry whose job has run
/// for its scope since the entry was made leaves the journal; so does one whose user or key is gone since (with a
/// start-log line: Rails' job would fail on it). No other precondition is checked: the job's own run decides (r4.4).
pub fn due_rails_wakes(c: &Connection, now: DateTime<Utc>) -> Result<Vec<(String, Option<String>)>, EngineError> {
    let tx = model::immediate(c)?;
    let all = rails_wakes(&tx)?;
    if all.is_empty() { return Ok(vec![]); }
    let before = all.len();
    let mut due = vec![];
    for w in all {
        let at = crate::codec::parse_time(&w.at).map_err(|e| EngineError::Data(format!("{RAILS_WAKES_KEY}: {}: {e:?}", w.at)))?;
        // "" is an unscoped entry (RailsWork::WakeUnscoped); every entry before A17 has a numeric scope.
        let scope = (!w.scope.is_empty()).then_some(w.scope.as_str());
        let ran = crate::jobs::state::read(&tx, &w.job, scope).map_err(EngineError::Data)?.last_run_at;
        if ran.is_some_and(|t| t >= at) { continue; }
        if scope.is_none() { due.push(w); continue; }
        let (table, noun) = wake_scope(&w.job);
        let id: i64 = w.scope.parse().map_err(|e| EngineError::Data(format!("{RAILS_WAKES_KEY}: scope {}: {e}", w.scope)))?;
        if !exists(&tx, table, id)? {
            super::log(&format!("dropped the {} run Deltabadger owed {noun} {id} from before the update: that {noun} is gone", w.job));
            continue;
        }
        due.push(w);
    }
    if due.len() != before { write_rails_wakes(&tx, &due, now)?; }
    tx.commit()?;
    Ok(due.into_iter().map(|w| { let scope = (!w.scope.is_empty()).then_some(w.scope); (w.job, scope) }).collect())
}

/// Activity events a takeover writes (their texts: web::i18n, `bot_activity.events.<event>`): C3, C4, C12.
pub const UPGRADE_SIGNAL_UNRESOLVED: &str = "upgrade_signal_unresolved";
pub const UPGRADE_COMMAND_DROPPED: &str = "upgrade_command_dropped";
pub const UPGRADE_ORDER_UNRECORDED: &str = "upgrade_order_unrecorded";

/// C4's %{action}: what the owner had asked for.
fn command_action(class: &str) -> &'static str {
    match class {
        "Bot::LiquidateExitedJob" => "sell the assets this bot no longer holds",
        "Bot::ResolveLiquidationJob" => "clear the unconfirmed sale",
        "Bot::RedeployJob" => "redeploy",
        "Bot::DeclineRedeployJob" => "decline the redeploy",
        _ => "clear the unconfirmed redeploy",
    }
}

/// Where a piece of Rust-only state lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// A table Rust's migrations create and 2.x's schema lacks.
    Table,
    /// An `app_configs` key; one ending in ':' or '.' is a prefix.
    AppConfig,
    /// A `bots.transient_data` key.
    Bot,
    /// A field inside a Rails row's JSON.
    Field,
    /// A text key with no 2.x translation: a `stop_message_key` or a `bot_activity_logs.event`.
    Text,
}

/// What a handback does with one piece of Rust-only state (PLAN-B2 §7, r1.10, r7.1). Rails is never handed state it
/// can't run safely: an order only Rust records, a rule only Rust runs, a pending decision only Rust settles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handback {
    /// Refused while it exists; the refusal line contains this text. Nothing is written.
    Refuses(&'static str),
    /// Waited for: the handback ends Unresolved (exit 3) or waits out the webhook calls, then settles it.
    Waits,
    /// Finished or removed by the handback, before the lease moves.
    Settled,
    /// Handed back as it is: 2.x never reads it, and a reclaim finds it again.
    Ignored,
    /// Handed back: 2.x's adoption acts on it (config/initializers/engine_lease.rb `adopt_handback!`).
    Adopted,
    /// Handed back: a cache 2.x never reads, refilled on demand.
    Cache,
}

/// Every piece of Rust-only state and its one handback outcome (PLAN-B2 r1.10). Pinned against the code by tests/handover.rs
/// `every_rust_only_state_has_a_handback_outcome` (every table Rust's migrations add, every rust_… key in src/), and
/// each refusing or waiting row is run by `each_refusing_state_refuses_the_handback_and_writes_nothing`.
pub const HANDBACK_STATE: &[(Held, &str, Handback)] = {
    use Handback::*;
    use Held::*;
    &[
        // ---- tables ----
        // The user's imported accounts and rows (tracker): 2.x never reads them.
        (Table, "exchange_accounts", Ignored), (Table, "exchange_account_credentials", Ignored), (Table, "exchange_account_imports", Ignored),
        (Table, "import_rows", Ignored), (Table, "import_cutoffs", Ignored),
        // IBKR: an IBKR bot, an open IBKR order and an unresolved IBKR intent refuse on Rails' own rows (D-IBKR-HANDBACK).
        (Table, "ibkr_conids", Ignored), (Table, "ibkr_bindings", Ignored), (Table, "ibkr_bot_accounts", Ignored),
        (Table, "ibkr_reservations", Refuses("shares held for an order that has not settled")),
        // KuCoin HF (D-KUCOIN-HF-LEDGER): an hf_enabled coverage row refuses; its fills go with it.
        (Table, "kucoin_coverage", Refuses("KuCoin HF history is Rust-only")), (Table, "kucoin_withheld_fills", Ignored),
        // API orders (D-ORDER-QUEUED): one placed through a bot is that bot's rust_placement (a wait). A standalone one is
        // never sent again after a restart (api_orders::sweep_at_start); once placed it is a venue order no bot follows,
        // as every standalone order 2.x places is (it records none, and cancels by exchange id). D-HANDBACK-STANDALONE-ROUTE:
        // its stored route (a KuCoin HF or UTA family) is this engine's; 2.x's cancel sends on its own route.
        (Table, "rust_api_orders", Ignored), (Table, "rust_api_order_standalone", Ignored),
        // Withdrawal rules (rules::handback_problems): an intent or an unconsumed evaluation refuses.
        (Table, "rust_withdrawal_intents", Refuses("withdrawal work is unconsumed or unconfirmed")),
        (Table, "rust_rule_evaluations", Refuses("withdrawal work is unconsumed or unconfirmed")),
        (Table, "rust_withdrawal_candidates", Ignored), (Table, "rust_withdrawal_ids", Ignored), (Table, "rust_rule_log_origins", Ignored),
        (Table, "rust_rails_attempt_acks", Ignored), (Table, "rust_rails_job_acks", Ignored), (Table, "rust_recorder_runs", Ignored),
        // Tax: a queued report runs after a reclaim; the carried costs are the user's (r7.1: intact after handback, 2.x boot
        // and reclaim).
        (Table, "rust_tax_reports", Ignored), (Table, "rust_tax_pl_carry", Ignored),
        (Table, "nbp_rates", Cache), (Table, "nbp_coverage", Cache), (Table, "cnb_rates", Cache), (Table, "cnb_coverage", Cache),
        (Table, "estv_crypto_values", Cache),
        // Rust's migration record: 2.x's db:migrate leaves it alone.
        (Table, "rust_schema_migrations", Ignored),
        // ---- app_configs ----
        (AppConfig, "engine_lease", Adopted),
        (AppConfig, "rust_created_at", Refuses("this install was created new, so there is no earlier app to hand it back to.")),
        (AppConfig, "rust_taken_over_at", Ignored), (AppConfig, "rust_handed_back_at", Ignored),
        // The latest release the update check stored (jobs::app_update): 2.x keeps its own in Rails.cache.
        (AppConfig, "rust_app_update", Ignored),
        (AppConfig, "rust_binance_a12_v1:", Refuses("Binance A12 identity data permanently requires Rust ownership")),
        (AppConfig, "rust_binance_repair_gaps.", Ignored), (AppConfig, "rust_account_identity_key_v1", Ignored),
        (AppConfig, "rust_coinbase_size_in_quote_repaired_at", Ignored),
        // KuCoin (reconcile.rs): an HF or UTA order record refuses; a Classic one is Rails' own route. An obligation or an
        // unproven route sits on Rails' own transaction row, which adopt_handback! polls while it is open.
        (AppConfig, "rust_kucoin_order:", Refuses("KuCoin HF orders are Rust-only")),
        (AppConfig, "rust_kucoin_unresolved:", Ignored), (AppConfig, "rust_kucoin_evidence:", Ignored), (AppConfig, "rust_kucoin_origin:", Ignored),
        (AppConfig, "rust_kucoin_hf_gap.", Ignored),
        // A user cancel's re-read: adopt_handback! polls every open order.
        (AppConfig, "rust_cancel_reread:", Ignored),
        // The takeover's journals: the queue rows it handled are deleted first (finish_queue_work, finish_signal_import);
        // a row that won't go refuses. The wakes are woken again at a reclaim.
        (AppConfig, "rust_rails_queue_done", Settled), (AppConfig, "rust_signal_import", Settled), (AppConfig, "rust_rails_wakes", Ignored),
        (AppConfig, "rust_rule_import", Refuses("withdrawal job cleanup is pending")),
        (AppConfig, "rust_rule_job_cleanup", Refuses("withdrawal job cleanup is pending")),
        (AppConfig, "rust_rules_fired_at", Ignored),
        (AppConfig, "rust_job.", Ignored), (AppConfig, "rust_sync.", Ignored), (AppConfig, "rust_tracker_gaps.", Ignored),
        (AppConfig, "rust_tracker_ledger.", Cache),
        // ---- bots.transient_data ----
        (Bot, "rust_placement", Waits), (Bot, "rust_reversal_cancels", Waits), (Bot, "rust_signal_calls", Waits),
        (Bot, "rust_base_cap_rotation_pending", Refuses("run the engine to settle the base-cap rotation decision before handing back")),
        (Bot, "rust_redeploy_request", Refuses("redeploy request pending; run the engine before handing back")),
        (Bot, "liquidation_selling_since.holdings", Refuses("liquidation request pending; run the engine before handing back")),
        (Bot, "rust_amount_limit_stops_pending", Settled), (Bot, "rust_condition_check_dead", Settled), (Bot, "rust_condition_info", Settled),
        (Bot, "rust_condition_info_wanted", Settled), (Bot, "rust_open_orders_wanted", Settled), (Bot, "rust_defer_until", Settled), (Bot, "rust_continue_start", Settled),
        (Bot, "rust_index_resync", Adopted),
        // D-HANDBACK-SPLIT-HOLD: 2.x does not compare a split against the venue's positions, so it would trade the bot.
        (Bot, "rust_split_hold", Refuses("its holdings disagree with a stock split")),
        // A set-aside bot is stopped; 2.x shows its stop key untranslated (D-HANDBACK-RUST-TEXT).
        (Bot, "rust_set_aside", Ignored),
        // Mail 2.x never sends.
        (Bot, "rust_set_aside_mail_pending", Ignored), (Bot, "rust_error_mail_pending", Ignored), (Bot, "rust_stopped_mail_pending", Ignored),
        (Bot, "rust_limit_mail_pending", Ignored), (Bot, "rust_base_limit_mail_pending", Ignored), (Bot, "rust_funds_mail_pending", Ignored),
        (Bot, "rust_base_cap_stop_mailed", Ignored),
        // The tick's own holds and stamps; a halted rebalance's halt is Rails' own rebalance_pending.
        (Bot, "rust_failure_origin", Ignored), (Bot, "rust_flip_refusal", Ignored),
        (Bot, "rust_history_generation", Ignored), (Bot, "run_generation", Ignored), (Bot, "tick_generation", Ignored), (Bot, "rust_rebalance_halted", Ignored), (Bot, "rust_rebalance_resolution", Ignored),
        (Bot, "rust_rebalance_resolved_orders", Ignored),
        // ---- elsewhere ----
        // In account_transactions.raw_data of rows this engine wrote: 2.x reads neither field.
        (Field, "rust_split_legs", Ignored), (Field, "amounts_inexact", Ignored), (Field, "source_rows", Ignored),
        // D-HANDBACK-RUST-TEXT: 2.x renders these as a missing translation (no raise); never rewritten or cleared.
        (Text, super::set_aside::STOP_KEY, Ignored), (Text, UPGRADE_SIGNAL_UNRESOLVED, Ignored), (Text, UPGRADE_COMMAND_DROPPED, Ignored),
        (Text, UPGRADE_ORDER_UNRECORDED, Ignored),
    ]
};

pub async fn hand_back<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock) -> Result<usize, EngineError> {
    hand_back_since(lock, o, factory, cipher, clock, DateTime::<Utc>::MIN_UTC).await
}

/// `hand_back` by a process that started at `process_start` (placement::recover_since).
pub async fn hand_back_since<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock, process_start: DateTime<Utc>) -> Result<usize, EngineError> {
    // PLAN-B1 r2.1: an install 3.0 created has no Rails to go back to. Refused before anything is written; the marker stays.
    if crate::app_config::get_plain(&o.primary, lease::CREATED_AT).map_err(EngineError::Data)?.is_some() {
        return Err(EngineError::Ineligible(vec!["this install was created new, so there is no earlier app to hand it back to.".into()]));
    }
    if crate::sync::binance_writer::written(&o.primary)? {
        return Err(EngineError::Ineligible(vec!["Binance A12 identity data permanently requires Rust ownership".into()]));
    }
    // The row Rails will read back must open with this secret before anything is written. With a wrong SECRET_KEY_BASE the
    // handback would otherwise overwrite it with a row Rails cannot decrypt and report success. An absent row (never taken
    // over by Rust) holds nothing to protect.
    lease::read(&o.primary, cipher)?;
    refuse_rust_only_conditions(&o.primary)?;
    refuse_ibkr(&o.primary, clock.now())?;
    hand_back_calls(o, clock.now())?;
    // Rails must never be handed a row a takeover already acted on (QUEUE_DONE_KEY): a crash before its delete is finished
    // here, and a row that still won't go refuses.
    finish_queue_work(o, clock.now())?;
    if !queue_done(&o.primary)?.is_empty() {
        return Err(EngineError::Ineligible(vec!["queued jobs the takeover already handled are still in the queue; start the engine once, then hand back".into()]));
    }
    let problems=super::rules::handback_problems(&o.primary)?;
    if !problems.is_empty(){return Err(EngineError::Ineligible(problems));}
    let mut unresolved = vec![];
    let mut s = o.primary.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL ORDER BY id")?;
    let pending: Vec<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for id in pending {
        let bot = model::load_bot(&o.primary, id)?;
        let (venue, _) = super::run::venue_for(&o.primary, factory, cipher, &bot)?;
        if let placement::Recovery::Pending = placement::recover_since(&o.primary, &venue, &bot, clock, process_start).await? { unresolved.push(id); }
    }
    // A reversal's cancel not yet recorded is Rust's alone: wait for it. So is a reserve still waiting for its buy's final
    // state: a Rails without the reader for that list would never credit it, so it is settled here before Rails takes over.
    for id in super::reversal::unrecorded(&o.primary)? { if !unresolved.contains(&id) { unresolved.push(id); } }
    for id in super::reversal::pending_settlements(&o.primary)? { if !unresolved.contains(&id) { unresolved.push(id); } }
    if !unresolved.is_empty() { return Err(EngineError::Unresolved(unresolved)); }
    refuse_pending_rotations(&o.primary)?;
    refuse_split_holds(&o.primary)?;
    refuse_commands(&o.primary)?;
    // Amount-limit stops a swept fill left pending (the engine died before its tick ended) land now, one `stopped` line each,
    // as the engine's next start would have run them (run::step). Rails neither reads the count nor polls the closed row
    // again, so after the handback nothing else would ever stop the bot.
    let mut s = o.primary.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_amount_limit_stops_pending') IS NOT NULL ORDER BY id")?;
    let stops: Vec<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    drop(s);
    for id in stops { super::tick::run_pending_amount_limit_stops(&o.primary, id, clock.now())?; }

    let now = clock.now();
    let tx = model::immediate(&o.primary)?;
    // Again inside the ownership transaction: a rule switched on since the first check is caught before the lease moves.
    if crate::sync::binance_writer::written(&tx)? {
        return Err(EngineError::Ineligible(vec!["Binance A12 identity data permanently requires Rust ownership".into()]));
    }
    refuse_rust_only_conditions(&tx)?;
    refuse_ibkr(&tx, now)?;
    refuse_pending_rotations(&tx)?;
    refuse_split_holds(&tx)?;
    refuse_commands(&tx)?;
    let problems=super::rules::handback_problems(&tx)?;
    if !problems.is_empty(){return Err(EngineError::Ineligible(problems));}
    let (scheduled, working) = (BotStatus::Scheduled as i64, model::working_list());
    // A bot paused on its price rule stays `waiting`: Rails' adoption (Bot::RepairOrphanedBotsJob) restarts its check chain,
    // where `scheduled` would trade it at the next checkpoint whatever the price.
    let mut s = tx.prepare(&format!("SELECT id FROM bots WHERE status IN ({working}) AND status <> ?1 ORDER BY id"))?;
    let ids = s.query_map([scheduled], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for id in ids {
        if model::condition_paused_type(&tx, &model::load_bot(&tx, id)?)?.is_some() { continue; }
        tx.execute("UPDATE bots SET status = ?1, updated_at = ?2 WHERE id = ?3", params![scheduled, crate::codec::format_time(now), id])?;
    }
    // The check's death marker, and the status line's stored info and waiting requests (Rails' own cache starts empty).
    for key in [super::conditions::DEAD, super::conditions::INFO_KEY, super::conditions::INFO_WANTED_KEY, super::polling::REFRESH_KEY] {
        tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{key}') WHERE json_type(transient_data, '$.{key}') IS NOT NULL"), [])?;
    }
    let scheduled: usize = tx.query_row("SELECT count(*) FROM bots WHERE status = ?1", [scheduled], |r| r.get::<_, i64>(0))? as usize;
    // rust_defer_until is the engine's own key, and it goes. Adopting a handback re-arms every scheduled bot at
    // next_interval_checkpoint_at (EngineLease.adopt_handback! → Bot::RepairOrphanedBotsJob#repair_bot), the first checkpoint
    // after the handback. For a checkpoint wait that is never before the deferred checkpoint, so Rails keeps the wait. A
    // run-now wait (placement::run_now, a continue Rails would run at once) that has not ticked yet is dropped: the bot runs
    // one checkpoint late, with the amount carried.
    placement::remove_wait(&tx, None)?;
    // A pending composition resync (index::RESYNC_KEY) stays on its bot: EngineLease.adopt_handback! enqueues
    // Bot::ResyncIndexCompositionJob for it and removes the key, and a reclaim before that finds it here still.
    // A continue start the engine never handled goes the same way: Rails' adoption re-arms the bot at its next checkpoint.
    tx.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_continue_start') \
                WHERE json_type(transient_data, '$.rust_continue_start') IS NOT NULL", [])?;
    lease::hand_back(lock, &tx, cipher, now)?;
    // When this engine left, plain, so `check` reads it without keys: Rails' stamps after it are Rails' own (staleness).
    crate::app_config::set_plain(&tx, lease::HANDED_BACK_AT, &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), now)
        .map_err(EngineError::Data)?;
    tx.commit()?;
    Ok(scheduled)
}

/// `deltabadger handback`: one pass; when it leaves an order unaccounted for, wait (a full window, so a fresh process may
/// trust Alpaca's "not found") and pass once more.
pub async fn hand_back_retrying<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock,
                                                 process_start: DateTime<Utc>, wait: std::time::Duration) -> Result<usize, EngineError> {
    loop {
        match hand_back_since(lock, o, factory, cipher, clock, process_start).await {
            // Each pass reads the calls again, so a restart during the wait waits for the same calls (§R8').
            Err(EngineError::SignalCallsWaiting(until)) => {
                let left = (until - clock.now()).to_std().unwrap_or_default() + std::time::Duration::from_secs(1);
                super::log(&format!("a webhook call is still valid; waiting {} s for it to expire", left.as_secs()));
                tokio::time::sleep(left).await;
            }
            Err(EngineError::Unresolved(_)) => {
                super::log(&format!("an order is not accounted for yet; waiting {} s before looking again", wait.as_secs()));
                tokio::time::sleep(wait).await;
                return hand_back_since(lock, o, factory, cipher, clock, process_start).await;
            }
            other => return other,
        }
    }
}

/// What `deltabadger handback` runs: this process starts now, so an Alpaca "not found" is trusted only a full margin after
/// `clock.now()` (placement::recover_since); when the first pass leaves an order unaccounted for, it waits that margin + 1 s once.
pub async fn hand_back_cli<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock) -> Result<usize, EngineError> {
    let wait = std::time::Duration::from_secs(super::venue_rules::ALPACA.absence_margin_secs as u64 + 1);
    hand_back_retrying(lock, o, factory, cipher, clock, clock.now(), wait).await
}

/// 0 handed back, 3 an order still unaccounted for (use resolve-placement), 1 anything else.
pub fn handback_exit_code(r: &Result<usize, EngineError>) -> i32 {
    match r { Ok(_) => 0, Err(EngineError::Unresolved(_)) => 3, Err(_) => 1 }
}

/// app_configs key (plan 8d T2, r2 §R1): the active_job_ids of the Bot::SignalJob rows a takeover imported whose queue rows
/// may still be there (the queue is its own database, so the import and the delete cannot share a transaction). While it
/// holds any, the engine runs no signal call (run::run_signals), so a crash between the two never runs a call twice.
pub const SIGNAL_IMPORT_KEY: &str = "rust_signal_import";

fn import_set(c: &rusqlite::Connection) -> Result<Vec<String>, EngineError> {
    let Some(text) = crate::app_config::get_plain(c, SIGNAL_IMPORT_KEY).map_err(EngineError::Data)? else { return Ok(vec![]) };
    serde_json::from_str(&text).map_err(|e| EngineError::Data(format!("{SIGNAL_IMPORT_KEY}: {e}")))
}

fn write_import_set(c: &rusqlite::Connection, ids: &[String], now: DateTime<Utc>) -> Result<(), EngineError> {
    if ids.is_empty() { return crate::app_config::delete(c, SIGNAL_IMPORT_KEY).map_err(EngineError::Data); }
    crate::app_config::set_plain(c, SIGNAL_IMPORT_KEY, &serde_json::to_string(ids).expect("ids serialise"), now).map_err(EngineError::Data)
}

/// Whether a takeover's import of Rails' webhook calls is unfinished (its queue rows may still be there).
pub fn signal_import_pending(c: &rusqlite::Connection) -> Result<bool, EngineError> { Ok(!import_set(c)?.is_empty()) }

/// A Bot::SignalJob Rails had not run: its active_job_id, and its bot and the call it becomes, or None for a job Rails
/// would discard (`discard_on ActiveJob::DeserializationError`: its bot or its rule is gone).
type SignalJob = (String, Option<(i64, model::SignalCall)>);

/// Every waiting Bot::SignalJob row (ready, scheduled or blocked; a claimed or failed one is queue_work's), in queue order, and
/// the waiting rows this build cannot read (no job id, arguments it does not read): (queue row, start-log line). Those are
/// deleted, never placed (D-UPGRADE-UNREAD-SIGNAL; Rails' discard_on discards what it cannot deserialize).
/// (queue row, start-log line) of a waiting call this build can't read.
type UnreadSignal = (i64, String);

fn signal_jobs(o: &Opened, skip: &[i64]) -> Result<(Vec<SignalJob>, Vec<UnreadSignal>), EngineError> {
    let mut s = o.queue.prepare("SELECT j.active_job_id, j.arguments, j.id FROM solid_queue_jobs j WHERE j.class_name = 'Bot::SignalJob' AND j.finished_at IS NULL \
                                 AND NOT EXISTS (SELECT 1 FROM solid_queue_claimed_executions c WHERE c.job_id = j.id) \
                                 AND NOT EXISTS (SELECT 1 FROM solid_queue_failed_executions f WHERE f.job_id = j.id) ORDER BY j.id")?;
    let rows: Vec<(Option<String>, Option<String>, i64)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let (mut jobs, mut unread) = (vec![], vec![]);
    for (job, arguments, row) in rows.into_iter().filter(|(_, _, row)| !skip.contains(row)) {
        let bot = arguments.as_deref().and_then(first_gid).unwrap_or_else(|| "no bot".into());
        match (job, arguments.as_deref().ok_or_else(|| "no arguments".to_string()).and_then(signal_call)) {
            (Some(job), Ok((bot_id, signal_id, triggered_at))) => {
                let bot: bool = o.primary.query_row("SELECT EXISTS (SELECT 1 FROM bots WHERE id = ?1 AND type = ?2)", params![bot_id, model::SIGNAL_TYPE], |r| r.get(0))?;
                let rule: bool = o.primary.query_row("SELECT EXISTS (SELECT 1 FROM bot_signals WHERE id = ?1)", [signal_id], |r| r.get(0))?;
                let call = (bot && rule).then(|| (bot_id, model::SignalCall { source: format!("rails:{job}"), signal_id, triggered_at }));
                jobs.push((job, call));
            }
            (None, _) => unread.push((row, format!("deleted a waiting webhook call this build can't read (queue row {row}, {bot}): no job id"))),
            (_, Err(e)) => unread.push((row, format!("deleted a waiting webhook call this build can't read (queue row {row}, {bot}): {e}"))),
        }
    }
    Ok((jobs, unread))
}

/// Bot::SignalJob's arguments as ActiveJob serialized them (`perform_later(bot, signal, triggered_at)`): the bot's and the
/// rule's GlobalIDs and the TimeWithZone (tests/fixtures/signal_jobs.json).
fn signal_call(arguments: &str) -> Result<(i64, i64, DateTime<Utc>), String> {
    let unread = || "arguments this build does not read".to_string();
    let v: Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let a = v.get("arguments").and_then(Value::as_array).filter(|a| a.len() == 3).ok_or_else(unread)?;
    let id = |i: usize, prefix: &str| a[i].get("_aj_globalid").and_then(Value::as_str).and_then(|g| g.strip_prefix(prefix)).and_then(|n| n.parse::<i64>().ok()).ok_or_else(unread);
    let time = a[2].as_object().filter(|t| t.get("_aj_serialized").and_then(Value::as_str) == Some("ActiveJob::Serializers::TimeWithZoneSerializer")).ok_or_else(unread)?;
    let at = crate::codec::parse_time(time.get("value").and_then(Value::as_str).ok_or_else(unread)?).map_err(|e| format!("triggered_at: {e:?}"))?;
    Ok((id(0, "gid://deltabadger/Bots::Signal/")?, id(1, "gid://deltabadger/BotSignal/")?, at))
}

/// Step 1 of §R1, in the claim's transaction: each job neither in the import set nor already a call becomes its bot's call
/// (a discarded one becomes none), and its id joins the set. Returns the calls added.
fn import_signal_jobs(tx: &rusqlite::Connection, jobs: &[SignalJob], now: DateTime<Utc>) -> Result<usize, EngineError> {
    let mut set = import_set(tx)?;
    let mut added = 0;
    for (job, call) in jobs {
        if set.contains(job) { continue; }
        if let Some((bot_id, call)) = call {
            if !model::load_bot(tx, *bot_id)?.signal_calls()?.iter().any(|c| c.source == call.source) {
                model::append_signal_call(tx, *bot_id, call)?;
                added += 1;
            }
        }
        set.push(job.clone());
    }
    write_import_set(tx, &set, now)?;
    Ok(added)
}

/// Steps 2 and 3 of §R1, after the import committed: the imported jobs leave the queue (their executions cascade), then the
/// ids whose rows are gone leave the set. Repeated by every start and by the handback, so a crash between any two steps is
/// finished later. Returns the rows deleted.
fn finish_signal_import(o: &Opened, now: DateTime<Utc>) -> Result<usize, EngineError> {
    let set = import_set(&o.primary)?;
    if set.is_empty() { return Ok(0); }
    let mut deleted = 0;
    let mut left = vec![];
    for job in set {
        deleted += o.queue.execute("DELETE FROM solid_queue_jobs WHERE active_job_id = ?1 AND class_name = 'Bot::SignalJob'", [&job])?;
        let there: bool = o.queue.query_row("SELECT EXISTS (SELECT 1 FROM solid_queue_jobs WHERE active_job_id = ?1 AND class_name = 'Bot::SignalJob')", [&job], |r| r.get(0))?;
        if there { left.push(job); }
    }
    let tx = model::immediate(&o.primary)?;
    write_import_set(&tx, &left, now)?;
    tx.commit()?;
    Ok(deleted)
}

/// Plan 8d T3 (§R8, §R8'): the webhook calls still waiting. Rails cannot be handed a valid call (that would be a Solid Queue
/// row, which is not ported), so while any is still valid the handback refuses with the instant the last one expires
/// (`hand_back_retrying` waits it out; no engine runs during a handback, so none is placed). Then each is removed with
/// the line the engine would write for it, in one transaction: `signal_expired`, or none for a rule that is gone.
fn hand_back_calls(o: &Opened, now: DateTime<Utc>) -> Result<(), EngineError> {
    finish_signal_import(o, now)?;
    let mut calls = vec![];
    for id in super::signal::bots_with_calls(&o.primary)? {
        calls.extend(model::load_bot(&o.primary, id)?.signal_calls()?.into_iter().map(|call| (id, call)));
    }
    let valid = calls.iter().filter(|(_, c)| !super::signal::expired(c.triggered_at, now)).map(|(_, c)| super::signal::expires_at(c)).max();
    if let Some(until) = valid { return Err(EngineError::SignalCallsWaiting(until)); }
    let tx = model::immediate(&o.primary)?;
    for (id, call) in &calls {
        let kept = model::signal(&tx, call.signal_id)?.is_some_and(|r| r.bot_id == *id);
        super::signal::consume(&tx, *id, call, kept.then_some("signal_expired"), now)?;
    }
    tx.commit()?;
    Ok(())
}

/// D-HANDBACK-RUST-CONDITIONS: the index and signal bots (not deleted) with a rule switched on that this engine runs and Rails
/// never reads there (its DcaIndex and Signal lack the concerns): (bot id, the switch). Handing them back would trade
/// them with the rule silently dropped.
pub fn rust_only_conditions(c: &Connection) -> Result<Vec<(i64, String)>, EngineError> {
    use super::conditions::{stored_support, Rule, Support};
    let mut s = c.prepare("SELECT id, type, settings FROM bots WHERE type IN ('Bots::DcaIndex', ?1) AND status <> ?2 AND json_valid(settings) ORDER BY id")?;
    let rows: Vec<(i64, String, String)> = s.query_map(params![model::SIGNAL_TYPE, BotStatus::Deleted as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut found = vec![];
    for (id, class, settings) in rows {
        let settings: Value = serde_json::from_str(&settings).map_err(|e| EngineError::Data(format!("bot {id}: {e}")))?;
        for (key, value) in settings.as_object().into_iter().flatten() {
            let Some((rule, selling)) = Rule::of_flag(key) else { continue };
            if value == &Value::Bool(true) && stored_support(&class, &settings, rule, selling) == Support::Implemented { found.push((id, key.clone())); }
        }
    }
    Ok(found)
}

/// D-IBKR-HANDBACK (`ibkr_reservations::handback_refusals`).
fn refuse_ibkr(c: &Connection, now: DateTime<Utc>) -> Result<(), EngineError> {
    let found = crate::venue::ibkr_reservations::handback_refusals(c, now)?;
    if found.is_empty() { Ok(()) } else { Err(EngineError::Ineligible(found)) }
}

/// Rails cannot finish a deferred base-cap rotation decision, even when its cancels and reserves are settled.
fn refuse_pending_rotations(c: &Connection) -> Result<(), EngineError> {
    let key = super::amount_limit::ROTATION_PENDING;
    let mut s = c.prepare("SELECT id FROM bots WHERE json_type(transient_data, ?1) IS NOT NULL ORDER BY id")?;
    let ids = s.query_map([format!("$.{key}")], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() { return Ok(()); }
    Err(EngineError::Ineligible(ids.into_iter().map(|id| format!("bot {id}: {key} is pending; run the engine to settle the base-cap rotation decision before handing back")).collect()))
}

/// D-HANDBACK-SPLIT-HOLD: a bot this engine holds because a stock split disagrees with the venue's positions (tick.rs,
/// `rust_split_hold`); 2.x makes no such comparison and would trade it. The hold clears at the bot's next tick that agrees.
fn refuse_split_holds(c: &Connection) -> Result<(), EngineError> {
    let mut s = c.prepare("SELECT id FROM bots WHERE json_type(transient_data, '$.rust_split_hold') IS NOT NULL AND status <> ?1 ORDER BY id")?;
    let ids = s.query_map([BotStatus::Deleted as i64], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() { return Ok(()); }
    Err(EngineError::Ineligible(ids.into_iter().map(|id| format!("bot {id}: trading is held because its holdings disagree with a stock split; \
        let this engine run the bot until the hold clears before handing back")).collect()))
}

fn refuse_rust_only_conditions(c: &Connection) -> Result<(), EngineError> {
    // D-KUCOIN-HF-LEDGER: Rails reads neither HF fills nor the one-trade rule; the marker stays through a later ClassicLf.
    if crate::sync::kucoin_hf::any_enabled(c).map_err(|e| EngineError::Data(e.0))? { return Err(EngineError::Ineligible(vec!["KuCoin HF history is Rust-only".into()])); }
    let hf = super::reconcile::handback_refusals(c)?;
    if !hf.is_empty() { return Err(EngineError::Ineligible(hf)); }
    let found = rust_only_conditions(c)?;
    if found.is_empty() { return Ok(()); }
    Err(EngineError::Ineligible(found.into_iter().map(|(id, key)| format!("bot {id}: {key} is on, and only this engine runs it on that kind of bot; switch it off before handing back")).collect()))
}


fn refuse_commands(c: &Connection) -> Result<(),EngineError> {
    let mut s=c.prepare("SELECT id,json_type(transient_data,'$.liquidation_selling_since.holdings') IS NOT NULL FROM bots WHERE json_type(transient_data,'$.liquidation_selling_since.holdings') IS NOT NULL OR json_type(transient_data,'$.rust_redeploy_request')='object' ORDER BY id")?;
    let reasons=s.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,bool>(1)?)))?.map(|r|r.map(|(id,selling)|format!("bot {id}: {} request pending; run the engine before handing back",if selling {"liquidation"} else {"redeploy"}))).collect::<Result<Vec<_>,_>>()?;
    if reasons.is_empty() {Ok(())} else {Err(EngineError::Ineligible(reasons))}
}
