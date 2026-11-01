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
use rusqlite::{params, Connection};
use serde_json::Value;

pub struct Takeover { pub claim: Claim, pub eligible: Vec<i64>, pub deleted_jobs: usize, pub normalised: usize, pub imported_calls: usize }

const RAILS_BOT_JOBS: [&str; 5] = ["Bot::ActionJob", "Bot::BroadcastAfterScheduledActionJob", "Bot::FetchAndUpdateOrderJob", "Bot::FetchAndUpdateOpenOrdersJob", "Bot::PriceLimitCheckJob"];

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
    let report = startup_report(&o.primary, cipher, now)?;
    let mut problems = report.problems.clone();
    problems.extend(report.unreadable.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})")));
    problems.extend(super::rules::takeover_problems(&o.primary,&o.queue)?);
    if !problems.is_empty() { return Err(EngineError::Ineligible(problems)); }
    // Rails' webhook calls, read and judged before anything changes owner: a claimed or unreadable one refuses here.
    refuse_composition_jobs(o)?;
    let signal_jobs = signal_jobs(o)?;
    let rule_jobs=super::rules::jobs(&o.primary,&o.queue)?;
    // Claim first, with the startup data steps: once committed, Rails refuses, and a failure below is
    // repaired by the next start, which repeats the deletes and the normalisation idempotently.
    let tx = model::immediate(&o.primary)?;
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
    super::rules::import(&tx,&rule_jobs,now)?;
    tx.commit()?;
    let deleted_signal_jobs = finish_signal_import(o, now)? + super::rules::finish_cleanup(&o.primary,&o.queue,now)?;

    // GlobalID names the STI class: Bots::DcaMultiAsset or Bots::DcaIndex.
    let mut gids: Vec<String> = vec![];
    for id in &report.eligible {
        let ty: String = o.primary.query_row("SELECT type FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
        gids.push(format!("gid://deltabadger/{ty}/{id}"));
    }
    for id in &report.eligible {
        let mut s = o.primary.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1)")?;
        for t in s.query_map([id], |r| r.get::<_, i64>(0))? { gids.push(format!("gid://deltabadger/Transaction/{}", t?)); }
    }
    let placeholders = RAILS_BOT_JOBS.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut s = o.queue.prepare(&format!("SELECT id, arguments FROM solid_queue_jobs WHERE finished_at IS NULL AND class_name IN ({placeholders})"))?;
    let jobs: Vec<(i64, Option<String>)> = s.query_map(rusqlite::params_from_iter(RAILS_BOT_JOBS), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?
        .collect::<Result<_, _>>()?;
    let doomed: Vec<i64> = jobs.into_iter()
        .filter(|(_, args)| args.as_deref().and_then(first_gid).is_some_and(|g| gids.contains(&g)))
        .map(|(id, _)| id).collect();
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
    for id in doomed.iter().chain(resyncs.iter().map(|(job, _)| job)) { o.queue.execute("DELETE FROM solid_queue_jobs WHERE id = ?1", [id])?; } // executions cascade

    let tx = model::immediate(&o.primary)?;
    let mut normalised = 0;
    for id in &report.eligible {
        normalised += model::unstick(&tx, *id, now)? as usize; // the last run (Rails' or ours) was cut short
    }
    tx.commit()?;
    Ok(Takeover { claim, eligible: report.eligible, deleted_jobs: doomed.len() + resyncs.len() + deleted_signal_jobs, normalised, imported_calls })
}

pub async fn hand_back<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock) -> Result<usize, EngineError> {
    hand_back_since(lock, o, factory, cipher, clock, DateTime::<Utc>::MIN_UTC).await
}

/// `hand_back` by a process that started at `process_start` (placement::recover_since).
pub async fn hand_back_since<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock, process_start: DateTime<Utc>) -> Result<usize, EngineError> {
    // PLAN-B1 r2.1: an install 3.0 created has no Rails to go back to. Refused before anything is written; the marker stays.
    if crate::app_config::get_plain(&o.primary, lease::CREATED_AT).map_err(EngineError::Data)?.is_some() {
        return Err(EngineError::Ineligible(vec!["this install was created by Deltabadger 3.0, so there is no earlier version to hand it back to.".into()]));
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
    for key in [super::conditions::DEAD, super::conditions::INFO_KEY, super::conditions::INFO_WANTED_KEY] {
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

/// Every Bot::SignalJob row Rails has not finished (ready, scheduled or blocked; a failed one is over), in queue order. A
/// claimed one (a Rails process died mid-run, so its order's fate is unknown) or one this build cannot read refuses the
/// takeover, naming the job and its bot.
fn signal_jobs(o: &Opened) -> Result<Vec<SignalJob>, EngineError> {
    let mut s = o.queue.prepare("SELECT j.active_job_id, j.arguments, EXISTS (SELECT 1 FROM solid_queue_claimed_executions c WHERE c.job_id = j.id) \
                                 FROM solid_queue_jobs j WHERE j.class_name = 'Bot::SignalJob' AND j.finished_at IS NULL \
                                 AND NOT EXISTS (SELECT 1 FROM solid_queue_failed_executions f WHERE f.job_id = j.id) ORDER BY j.id")?;
    let rows: Vec<(Option<String>, Option<String>, bool)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let (mut jobs, mut problems) = (vec![], vec![]);
    for (job, arguments, claimed) in rows {
        let name = format!("Bot::SignalJob {}", job.as_deref().unwrap_or("(no job id)"));
        let bot = arguments.as_deref().and_then(first_gid).unwrap_or_default();
        if claimed {
            problems.push(format!("{name} for {bot} was claimed by a Rails process that stopped while running it: whether it placed an order is unknown; \
                                   check the venue, then delete the job from the queue"));
            continue;
        }
        match (job, arguments.as_deref().ok_or_else(|| "no arguments".to_string()).and_then(signal_call)) {
            (Some(job), Ok((bot_id, signal_id, triggered_at))) => {
                let bot: bool = o.primary.query_row("SELECT EXISTS (SELECT 1 FROM bots WHERE id = ?1 AND type = ?2)", params![bot_id, model::SIGNAL_TYPE], |r| r.get(0))?;
                let rule: bool = o.primary.query_row("SELECT EXISTS (SELECT 1 FROM bot_signals WHERE id = ?1)", [signal_id], |r| r.get(0))?;
                let call = (bot && rule).then(|| (bot_id, model::SignalCall { source: format!("rails:{job}"), signal_id, triggered_at }));
                jobs.push((job, call));
            }
            (None, _) => problems.push(format!("{name} for {bot}: no job id")),
            (_, Err(e)) => problems.push(format!("{name} for {bot}: {e}")),
        }
    }
    if !problems.is_empty() { return Err(EngineError::Ineligible(problems)); }
    Ok(jobs)
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

fn refuse_rust_only_conditions(c: &Connection) -> Result<(), EngineError> {
    // D-KUCOIN-HF-LEDGER: Rails reads neither HF fills nor the one-trade rule; the marker stays through a later ClassicLf.
    if crate::sync::kucoin_hf::any_enabled(c).map_err(|e| EngineError::Data(e.0))? { return Err(EngineError::Ineligible(vec!["KuCoin HF history is Rust-only".into()])); }
    let hf = super::reconcile::handback_refusals(c)?;
    if !hf.is_empty() { return Err(EngineError::Ineligible(hf)); }
    let found = rust_only_conditions(c)?;
    if found.is_empty() { return Ok(()); }
    Err(EngineError::Ineligible(found.into_iter().map(|(id, key)| format!("bot {id}: {key} is on, and only this engine runs it on that kind of bot; switch it off before handing back")).collect()))
}


// D-6C-10: a queued manual command belongs to Rails, including stopped and archived bots.
fn refuse_composition_jobs(o: &Opened) -> Result<(),EngineError> {
    let mut s=o.primary.prepare("SELECT id,type FROM bots ORDER BY id")?;
    let bots=s.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut reasons=vec![];
    for (id,class) in bots {
        let gid=format!("gid://deltabadger/{class}/{id}");
        for job in ["Bot::LiquidateExitedJob","Bot::RedeployJob","Bot::DeclineRedeployJob","Bot::ResolveLiquidationJob","Bot::ResolveRedeployJob"] {
            if active_job(&o.queue,job,&gid)? { reasons.push(format!("bot {id}: unfinished {job}; let Rails finish it before takeover")); }
        }
    }
    if reasons.is_empty() { Ok(()) } else { Err(EngineError::Ineligible(reasons)) }
}
fn refuse_commands(c: &Connection) -> Result<(),EngineError> {
    let mut s=c.prepare("SELECT id,json_type(transient_data,'$.liquidation_selling_since.holdings') IS NOT NULL FROM bots WHERE json_type(transient_data,'$.liquidation_selling_since.holdings') IS NOT NULL OR json_type(transient_data,'$.rust_redeploy_request')='object' ORDER BY id")?;
    let reasons=s.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,bool>(1)?)))?.map(|r|r.map(|(id,selling)|format!("bot {id}: {} request pending; run the engine before handing back",if selling {"liquidation"} else {"redeploy"}))).collect::<Result<Vec<_>,_>>()?;
    if reasons.is_empty() {Ok(())} else {Err(EngineError::Ineligible(reasons))}
}
