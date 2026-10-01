//! Spec §3 handover. Takeover: refuse an ineligible install, claim it, drop Rails' jobs for the bots
//! this engine now runs, unstick `executing`. Handback: every order accounted for, then — in ONE
//! SQLite transaction — every working bot `scheduled` and the handover row set to handed back.
use super::{eligibility, model, placement, Clock, EngineError};
use crate::crypto::Cipher;
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
    let tx = model::immediate(&o.primary)?; // claim and normalisation land together or not at all
    let claim = lease::claim(lock, &tx, cipher, version, now)?;

    let mut gids: Vec<String> = report.eligible.iter().map(|id| format!("gid://deltabadger/Bots::DcaMultiAsset/{id}")).collect();
    for id in &report.eligible {
        let mut s = tx.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1)")?;
        for tx in s.query_map([id], |r| r.get::<_, i64>(0))? { gids.push(format!("gid://deltabadger/Transaction/{}", tx?)); }
    }
    let placeholders = RAILS_BOT_JOBS.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut s = o.queue.prepare(&format!("SELECT id, arguments FROM solid_queue_jobs WHERE finished_at IS NULL AND class_name IN ({placeholders})"))?;
    let doomed: Vec<i64> = s.query_map(rusqlite::params_from_iter(RAILS_BOT_JOBS), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?
        .filter_map(Result::ok)
        .filter(|(_, args)| args.as_deref().and_then(first_gid).is_some_and(|g| gids.contains(&g)))
        .map(|(id, _)| id).collect();
    for id in &doomed { o.queue.execute("DELETE FROM solid_queue_jobs WHERE id = ?1", [id])?; } // executions cascade

    let mut normalised = 0;
    for id in &report.eligible {
        // `executing`/`waiting` only exist mid-tick: here they mean the last run (Rails' or ours) was cut short.
        normalised += tx.execute("UPDATE bots SET status = 1, updated_at = ?1 WHERE id = ?2 AND status IN (4, 6)",
                                        params![crate::codec::format_time(now), id])?;
    }
    tx.commit()?;
    Ok(Takeover { claim, eligible: report.eligible, deleted_jobs: doomed.len(), normalised })
}

pub async fn hand_back<F: VenueFactory>(lock: &EngineLock, o: &Opened, factory: &F, cipher: &Cipher, clock: &dyn Clock) -> Result<usize, EngineError> {
    let mut unresolved = vec![];
    let mut s = o.primary.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL ORDER BY id")?;
    let pending: Vec<i64> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for id in pending {
        let bot = model::load_bot(&o.primary, id)?;
        let venue = factory.for_key(model::credentials_for(&o.primary, cipher, &bot)?);
        if let placement::Recovery::Pending = placement::recover(&o.primary, &venue, &bot, clock).await? { unresolved.push(id); }
    }
    if !unresolved.is_empty() { return Err(EngineError::Unresolved(unresolved)); }

    let now = clock.now();
    let tx = model::immediate(&o.primary)?;
    tx.execute("UPDATE bots SET status = 1, updated_at = ?1 WHERE status IN (4, 5, 6)", [crate::codec::format_time(now)])?;
    let scheduled: usize = tx.query_row("SELECT count(*) FROM bots WHERE status = 1", [], |r| r.get::<_, i64>(0))? as usize;
    lease::hand_back(lock, &tx, cipher, now)?;
    tx.commit()?;
    Ok(scheduled)
}
