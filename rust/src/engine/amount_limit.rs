//! The two amount limits' stops and mails: Bot::QuoteAmountLimitable ("don't spend more than N quote", buying) and
//! Bot::BaseAmountLimitable ("don't sell more than N base", a selling one-asset basket, slice 6d).
use super::{accounting, model, notice, tick, EngineError};
use crate::codec::format_time;
use crate::enums::{BotStatus, BOT_WORKING};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;

/// stop_message_key of the base cap's stop (config/locales/bot.en.yml: "The whole amount has been sold.").
pub const AMOUNT_SOLD: &str = "bot.settings.extra_amount_limit.amount_sold";
/// D-6D-2's dedupe: the base cap's key (model::Bot::base_amount_limit_key) its stop mail was last owed under. Written with the
/// mail marker and never cleared by the mail's delivery, so a rotation of a bot already stopped and mailed for this cap owes
/// nothing more; editing the cap (a new key) mails again.
pub const STOP_MAILED: &str = "rust_base_cap_stop_mailed";
/// D-6D-2 deferred: the open sells a rotation queued for cancel while the cap is used up only with them. The decision waits
/// until each is final (`settle_rotation`, from the fill's write and reversal::scan), then is taken on what was sold.
pub const ROTATION_PENDING: &str = "rust_base_cap_rotation_pending";

/// Which cap fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap { Quote, Base }

impl Cap {
    pub fn stop_message_key(self) -> &'static str { match self { Cap::Quote => tick::AMOUNT_SPENT, Cap::Base => AMOUNT_SOLD } }
    /// The marker of its mail (engine::notice): stopped_by_amount_limit or stopped_by_base_amount_limit.
    pub fn mail_marker(self) -> &'static str { match self { Cap::Quote => notice::LIMIT, Cap::Base => notice::BASE_LIMIT } }
    /// Its `kind` in a counted stop (`rust_amount_limit_stops_pending`); a count without one is the quote cap's.
    pub fn kind(self) -> &'static str { match self { Cap::Quote => "quote", Cap::Base => "base" } }
}

/// #notify_stopped_by_amount_limit / #notify_stopped_by_base_amount_limit, owed in the caller's transaction (the fill's, on
/// every path that applies one). The base cap's also records its key in STOP_MAILED.
pub(crate) fn owe_mail(c: &Connection, bot_id: i64, cap: Cap, now: DateTime<Utc>) -> Result<(), EngineError> {
    let marker = if cap == Cap::Base { base_marker(c, bot_id, now)? } else { notice::limit_marker(now) };
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3",
              params![format!("$.{}", cap.mail_marker()), marker.to_string(), bot_id])?;
    if cap == Cap::Base {
        let key = model::load_bot(c, bot_id)?.base_amount_limit_key()?;
        c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, ?2) WHERE id = ?3", params![format!("$.{STOP_MAILED}"), key, bot_id])?;
    }
    Ok(())
}

/// The base cap's mail marker with what the mail states, as #notify_stopped_by_base_amount_limit captures it at enqueue:
/// the cap (D-6D-1's spelling) and the base's symbol. Without a single base the marker holds the stamp alone.
fn base_marker(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<Value, EngineError> {
    let mut marker = notice::limit_marker(now);
    let settings = model::load_bot(c, bot_id)?.settings;
    if let Some((base, decimals)) = crate::mail::render::base_of(c, bot_id, &settings)? {
        marker["amount"] = crate::mail::render::base_cap_amount(settings.get("base_amount_limit"), decimals).into();
        marker["base"] = base.into();
    }
    Ok(marker)
}

/// D-6D-2, in the rotation's own transaction (web::bot::write::reverse), after the direction is set: a bot that now sells its
/// one asset with the base cap on and used up is stopped (`amount_sold`) and owed the base cap's mail. Rails rotates it into
/// selling and leaves it idle, every tick skipping (`sell_skipped reason=cap_reached`), with no word to its owner.
/// A working bot is stopped as tick::stop_owing_mail stops it (its `stopped` line); a stopped one keeps its stop and takes
/// the reason. Once per cap: nothing is written when STOP_MAILED already holds the cap's key. Returns whether it wrote.
/// Counted is only what is definitely sold: the open sells this rotation cancels are left out, and when the cap is used up
/// only with them the decision waits for their final state (ROTATION_PENDING).
pub fn stop_at_rotation(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<bool, EngineError> {
    c.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?1) WHERE id = ?2", params![format!("$.{ROTATION_PENDING}"), bot_id])?;
    decide(c, bot_id, now, true)
}

fn decide(c: &Connection, bot_id: i64, now: DateTime<Utc>, defer: bool) -> Result<bool, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    if !bot.base_amount_limited() { return Ok(false); }
    let key = bot.base_amount_limit_key()?;
    if bot.transient.get(STOP_MAILED).and_then(Value::as_str) == Some(key.as_str()) { return Ok(false); }
    if !accounting::base_amount_limit_reached_settled(c, &bot)? {
        if defer && accounting::base_amount_limit_reached(c, &bot)? {
            let mut s = c.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND side = 1 AND status = 0 AND external_status IN (0, 1) AND transaction_type = 'REGULAR' ORDER BY id")?;
            let open = s.query_map([bot_id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
            c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3",
                      params![format!("$.{ROTATION_PENDING}"), serde_json::json!(open).to_string(), bot_id])?;
        }
        return Ok(false);
    }
    let marker = base_marker(c, bot_id, now)?;
    let stopped = if BOT_WORKING.contains(&bot.status) {
        tick::stop_owing_mail(c, bot_id, AMOUNT_SOLD, Some((notice::BASE_LIMIT, marker)), now)?
    } else if bot.status == BotStatus::Stopped {
        c.execute("UPDATE bots SET stop_message_key = ?1, updated_at = ?2, transient_data = json_set(transient_data, ?3, json(?4)) WHERE id = ?5 AND status = ?6",
                  params![AMOUNT_SOLD, format_time(now), format!("$.{}", notice::BASE_LIMIT), marker.to_string(), bot_id, BotStatus::Stopped as i64])? == 1
    } else { false };
    if stopped {
        c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, ?2) WHERE id = ?3", params![format!("$.{STOP_MAILED}"), key, bot_id])?;
    }
    Ok(stopped)
}

/// The deferred D-6D-2 decision, in the write that left order `tx_id` final (polling's fill, reversal::scan): the order
/// leaves ROTATION_PENDING, and once none is left the decision is taken on what was sold. `decided`: the fill's own
/// re-check stopped the bot already (handle_base_amount_limit_update); the list goes, nothing more is written.
pub(crate) fn settle_rotation(c: &Connection, bot_id: i64, tx_id: i64, now: DateTime<Utc>, decided: bool) -> Result<(), EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    let mut ids: Vec<i64> = bot.transient.get(ROTATION_PENDING).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    if !ids.contains(&tx_id) { return Ok(()); }
    let open: bool = c.query_row("SELECT status = 0 AND external_status IN (0, 1) FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0)).unwrap_or(false);
    if open && !decided { return Ok(()); }
    ids.retain(|id| *id != tx_id);
    if ids.is_empty() || decided {
        c.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?1) WHERE id = ?2", params![format!("$.{ROTATION_PENDING}"), bot_id])?;
        if !decided { decide(c, bot_id, now, false)?; }
    } else {
        c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3", params![format!("$.{ROTATION_PENDING}"), serde_json::json!(ids).to_string(), bot_id])?;
    }
    Ok(())
}

/// The bots whose deferred decision waits, each with its orders (reversal::scan).
pub(crate) fn rotation_pending(c: &Connection) -> Result<Vec<(i64, Vec<i64>)>, EngineError> {
    let mut s = c.prepare("SELECT id, json_extract(transient_data, ?1) FROM bots WHERE json_array_length(transient_data, ?1) > 0 ORDER BY id")?;
    let rows = s.query_map([format!("$.{ROTATION_PENDING}")], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().map(|(id, raw)| (id, serde_json::from_str::<Vec<i64>>(&raw).unwrap_or_default())).collect())
}
