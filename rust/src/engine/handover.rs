//! Spec §3 handover. Takeover: refuse an ineligible install, claim it, drop Rails' jobs for the bots
//! this engine now runs, unstick `executing`. Handback: every order accounted for, then — in ONE
//! SQLite transaction — every working bot `scheduled` and the handover row set to handed back.
use super::{eligibility, model, placement, Clock, EngineError};
use crate::crypto::Cipher;
use crate::enums::BotStatus;
use crate::lease::{self, Claim, EngineLock};
use crate::store::Opened;
use crate::venue::VenueFactory;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::Value;

pub struct Takeover { pub claim: Claim, pub eligible: Vec<i64>, pub deleted_jobs: usize, pub normalised: usize }

const RAILS_BOT_JOBS: [&str; 4] = ["Bot::ActionJob", "Bot::BroadcastAfterScheduledActionJob", "Bot::FetchAndUpdateOrderJob", "Bot::FetchAndUpdateOpenOrdersJob"];

fn first_gid(arguments: &str) -> Option<String> {
    let v: Value = serde_json::from_str(arguments).ok()?;
    let first = v.get("arguments")?.get(0)?;
    first.get("_aj_globalid").and_then(Value::as_str).or_else(|| first.as_str()).map(str::to_string)
}

pub fn take_over(lock: &EngineLock, o: &Opened, cipher: &Cipher, version: &str, now: DateTime<Utc>) -> Result<Takeover, EngineError> {
    let report = eligibility::check_install(&o.primary)?;
    let mut problems = report.problems.clone();
    problems.extend(report.unreadable.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})")));
    if !problems.is_empty() { return Err(EngineError::Ineligible(problems)); }
    // Claim first and on its own: once committed, Rails refuses, and a failure below is repaired by the next
    // start, which repeats the deletes and the normalisation idempotently.
    let claim = lease::claim(lock, &o.primary, cipher, version, now)?;

    let mut gids: Vec<String> = report.eligible.iter().map(|id| format!("gid://deltabadger/Bots::DcaMultiAsset/{id}")).collect();
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
    for id in &doomed { o.queue.execute("DELETE FROM solid_queue_jobs WHERE id = ?1", [id])?; } // executions cascade

    let tx = model::immediate(&o.primary)?;
    let mut normalised = 0;
    for id in &report.eligible {
        normalised += model::unstick(&tx, *id, now)? as usize; // the last run (Rails' or ours) was cut short
    }
    tx.commit()?;
    Ok(Takeover { claim, eligible: report.eligible, deleted_jobs: doomed.len(), normalised })
}

pub async fn hand_back<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock) -> Result<usize, EngineError> {
    hand_back_since(lock, o, factory, cipher, clock, DateTime::<Utc>::MIN_UTC).await
}

/// `hand_back` by a process that started at `process_start` (placement::recover_since).
pub async fn hand_back_since<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock, process_start: DateTime<Utc>) -> Result<usize, EngineError> {
    // The row Rails will read back must open with this secret before anything is written. With a wrong SECRET_KEY_BASE the
    // handback would otherwise overwrite it with a row Rails cannot decrypt and report success. An absent row (never taken
    // over by Rust) holds nothing to protect.
    lease::read(&o.primary, cipher)?;
    let mut unresolved = vec![];
    let mut s = o.primary.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL ORDER BY id")?;
    let pending: Vec<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for id in pending {
        let bot = model::load_bot(&o.primary, id)?;
        let venue = factory.for_bot(&model::exchange_type(&o.primary, &bot)?, model::credentials_for(&o.primary, cipher, &bot)?);
        if let placement::Recovery::Pending = placement::recover_since(&o.primary, &venue, &bot, clock, process_start).await? { unresolved.push(id); }
    }
    if !unresolved.is_empty() { return Err(EngineError::Unresolved(unresolved)); }

    let now = clock.now();
    let tx = model::immediate(&o.primary)?;
    let (scheduled, working) = (BotStatus::Scheduled as i64, model::working_list());
    tx.execute(&format!("UPDATE bots SET status = ?1, updated_at = ?2 WHERE status IN ({working}) AND status <> ?1"), params![scheduled, crate::codec::format_time(now)])?;
    let scheduled: usize = tx.query_row("SELECT count(*) FROM bots WHERE status = ?1", [scheduled], |r| r.get::<_, i64>(0))? as usize;
    lease::hand_back(lock, &tx, cipher, now)?;
    tx.commit()?;
    Ok(scheduled)
}

/// `deltabadger handback`: one pass; when it leaves an order unaccounted for, wait (a full window, so a fresh process may
/// trust Alpaca's "not found") and pass once more.
pub async fn hand_back_retrying<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock,
                                                 process_start: DateTime<Utc>, wait: std::time::Duration) -> Result<usize, EngineError> {
    match hand_back_since(lock, o, factory, cipher, clock, process_start).await {
        Err(EngineError::Unresolved(_)) => {
            super::log(&format!("an order is not accounted for yet; waiting {} s before looking again", wait.as_secs()));
            tokio::time::sleep(wait).await;
            hand_back_since(lock, o, factory, cipher, clock, process_start).await
        }
        other => other,
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
