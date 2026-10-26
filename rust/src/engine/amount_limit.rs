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
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3",
              params![format!("$.{}", cap.mail_marker()), notice::limit_marker(now).to_string(), bot_id])?;
    if cap == Cap::Base {
        let key = model::load_bot(c, bot_id)?.base_amount_limit_key()?;
        c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, ?2) WHERE id = ?3", params![format!("$.{STOP_MAILED}"), key, bot_id])?;
    }
    Ok(())
}

/// D-6D-2, in the rotation's own transaction (web::bot::write::reverse), after the direction is set: a bot that now sells its
/// one asset with the base cap on and used up is stopped (`amount_sold`) and owed the base cap's mail. Rails rotates it into
/// selling and leaves it idle, every tick skipping (`sell_skipped reason=cap_reached`), with no word to its owner.
/// A working bot is stopped as tick::stop_owing_mail stops it (its `stopped` line); a stopped one keeps its stop and takes
/// the reason. Once per cap: nothing is written when STOP_MAILED already holds the cap's key. Returns whether it wrote.
pub fn stop_at_rotation(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    if !bot.base_amount_limited() || !accounting::base_amount_limit_reached(c, &bot)? { return Ok(false); }
    let key = bot.base_amount_limit_key()?;
    if bot.transient.get(STOP_MAILED).and_then(Value::as_str) == Some(key.as_str()) { return Ok(false); }
    let marker = notice::limit_marker(now);
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
