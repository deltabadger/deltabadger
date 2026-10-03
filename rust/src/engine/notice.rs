//! Mail the engine owes the owner, kept as markers on the bot's own row.
//!
//! Where Rails calls a `Bot::Notifyable` method, the engine writes a key into `bots.transient_data` in the same
//! statement or transaction that spends the mail budget or stops the bot. That key is the whole queue: nothing is
//! held in memory, so a crash or a restart loses no mail. Whoever sends mail (`mail::sender`) reads `pending`, and
//! `clear`s a marker only after the mail server accepted the message, so a mail is sent at least once.
//!
//! | Key | Value | Mail (BotAlertsMailer) |
//! |---|---|---|
//! | `rust_funds_mail_pending` | `{"quote_asset": 2, "stamped_at": "…"}` | `end_of_funds` |
//! | `rust_error_mail_pending` | `{"<kind>": {"error": "…", "stamped_at": "…"}}`, kind as in `failure_notifications` | `notify_about_error` |
//! | `rust_stopped_mail_pending` | `{"error": "…", "stamped_at": "…"}` | `stopped_by_error` |
//! | `rust_limit_mail_pending` | `{"stamped_at": "…"}` | `stopped_by_amount_limit` |
//!
//! `error` is at most 500 characters (longer text is cut, with an ellipsis): a marker never grows with what a venue sends.
//! `stamped_at` is when the engine raised it, the instant of the stamp or stop it was written with (ISO 8601, milliseconds, UTC). `error` is the raw message (Rails' `e.message`);
//! the mail humanises it in the user's language.
//!
//! Rails leaves these keys alone: every write of `transient_data` there merges into the stored value (store_accessor,
//! Bot#merge_transient_data!), so after a handback they stay on the row, unsent, until this engine runs again.
use super::{eligibility, model, EngineError};
use crate::crypto::Cipher;
use crate::ruby::iso8601_ms;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};

/// The funds marker is the one the scheduler plan fixed (its S-5(b)); the stamp that writes it is tick::stamp_funds_low.
pub const FUNDS: &str = super::tick::FUNDS_MAIL_PENDING;
pub const ERROR: &str = "rust_error_mail_pending";
pub const STOPPED: &str = "rust_stopped_mail_pending";
pub const LIMIT: &str = "rust_limit_mail_pending";
pub const KEYS: [&str; 4] = [FUNDS, ERROR, STOPPED, LIMIT];
/// The most an error text may be, in a marker and in the mail built from it.
pub const ERROR_LIMIT: usize = 500;

/// `text`, or its first `limit` characters and an ellipsis.
pub fn bounded(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Notice {
    /// Bot::Notifyable#notify_end_of_funds: the quote balance is under three days of spend, or a buy was refused for funds.
    EndOfFunds { quote_asset_id: Option<i64> },
    /// #notify_about_error: a tick failed; one a day per bot and failure kind.
    Error { kind: String, error: String },
    /// #notify_stopped_by_error: the second blocking failure in a row stopped the bot.
    StoppedByError { error: String },
    /// #notify_stopped_by_amount_limit: a fill took the bot to its quote cap and stopped it.
    StoppedByAmountLimit,
}

impl Notice {
    /// The mailer action Rails enqueues for it.
    pub fn mail(&self) -> &'static str {
        match self {
            Notice::EndOfFunds { .. } => "end_of_funds",
            Notice::Error { .. } => "notify_about_error",
            Notice::StoppedByError { .. } => "stopped_by_error",
            Notice::StoppedByAmountLimit => "stopped_by_amount_limit",
        }
    }
}

/// One marker found on a bot's row. `stamped_at` is kept as written: it is what `clear` matches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pending { pub bot_id: i64, pub stamped_at: String, pub notice: Notice }

impl Pending {
    pub fn raised_at(&self) -> Option<DateTime<Utc>> { DateTime::parse_from_rfc3339(&self.stamped_at).ok().map(|t| t.with_timezone(&Utc)) }
}

pub fn funds_marker(quote_asset_id: Option<i64>, now: DateTime<Utc>) -> Value { json!({ "quote_asset": quote_asset_id, "stamped_at": iso8601_ms(now) }) }
pub fn stopped_marker(error: &str, now: DateTime<Utc>) -> Value { json!({ "error": bounded(error, ERROR_LIMIT), "stamped_at": iso8601_ms(now) }) }
pub fn limit_marker(now: DateTime<Utc>) -> Value { json!({ "stamped_at": iso8601_ms(now) }) }
/// The error marker with `kind`'s entry set, the other kinds' entries kept.
pub fn error_marker(existing: Option<&Value>, kind: &str, error: &str, now: DateTime<Utc>) -> Value {
    let mut by_kind = existing.and_then(Value::as_object).cloned().unwrap_or_default();
    by_kind.insert(kind.to_string(), json!({ "error": bounded(error, ERROR_LIMIT), "stamped_at": iso8601_ms(now) }));
    Value::Object(by_kind)
}

fn kind_ok(kind: &str) -> bool { !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') }

/// Every readable marker in one bot's `transient_data`. A marker of another shape is ignored here and stays on the row.
pub fn pending_in(bot_id: i64, transient: &Map<String, Value>) -> Vec<Pending> {
    let at = |v: &Value| v.get("stamped_at").and_then(Value::as_str).map(str::to_string);
    let error = |v: &Value| v.get("error").and_then(Value::as_str).map(str::to_string);
    let mut out = vec![];
    if let Some((v, at)) = transient.get(FUNDS).and_then(|v| Some((v, at(v)?))) {
        out.push(Pending { bot_id, stamped_at: at, notice: Notice::EndOfFunds { quote_asset_id: v.get("quote_asset").and_then(Value::as_i64) } });
    }
    for (kind, v) in transient.get(ERROR).and_then(Value::as_object).into_iter().flatten().filter(|(kind, _)| kind_ok(kind)) {
        if let (Some(at), Some(error)) = (at(v), error(v)) { out.push(Pending { bot_id, stamped_at: at, notice: Notice::Error { kind: kind.clone(), error } }); }
    }
    if let Some((at, error)) = transient.get(STOPPED).and_then(|v| Some((at(v)?, error(v)?))) { out.push(Pending { bot_id, stamped_at: at, notice: Notice::StoppedByError { error } }); }
    if let Some(at) = transient.get(LIMIT).and_then(at) { out.push(Pending { bot_id, stamped_at: at, notice: Notice::StoppedByAmountLimit }); }
    out
}

/// The markers of at most `limit` bots whose id is above `after`, by bot id; and the id to pass as `after` next time
/// (0 once the last bot has been read, so that a reader goes round). One primary-key range scan of `bots`, stopping at
/// `limit` rows that carry a marker: there is no index on a JSON key without a schema change, and the table is one row
/// per bot.
pub fn pending(c: &Connection, after: i64, limit: usize) -> Result<(Vec<Pending>, i64), EngineError> {
    let mut s = c.prepare("SELECT id, transient_data FROM bots WHERE id > ?1 AND instr(transient_data, '_mail_pending') > 0 ORDER BY id LIMIT ?2")?;
    let rows = s.query_map(params![after, limit as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let next = if rows.len() < limit { 0 } else { rows.last().map_or(0, |(id, _)| *id) };
    let out = rows.into_iter()
        .filter_map(|(id, raw)| serde_json::from_str::<Value>(&raw).ok().and_then(|v| v.as_object().map(|m| pending_in(id, m))))
        .flatten().collect();
    Ok((out, next))
}

/// Every marker on every bot (tests, the parity harness).
pub fn all_pending(c: &Connection) -> Result<Vec<Pending>, EngineError> { Ok(pending(c, 0, usize::MAX >> 1)?.0) }

/// Removes the marker `p` was read from, and only that one: a marker raised again since (another `stamped_at`) stays.
/// One short write transaction; nothing else in `transient_data` is touched. Returns whether it was still there.
/// It is a write of `bots` from outside the engine, so it passes `eligibility::guard` before it commits, like every
/// such write. The key it removes is one the engine never reads, so the guard can only refuse for a reason that was
/// already there; the marker then stays (Err), and the caller must not send its mail a second time because of that.
pub fn clear(c: &Connection, cipher: &Cipher, p: &Pending) -> Result<bool, EngineError> {
    let path = match &p.notice {
        Notice::EndOfFunds { .. } => format!("$.{FUNDS}"),
        Notice::Error { kind, .. } if kind_ok(kind) => format!("$.{ERROR}.{kind}"),
        Notice::Error { kind, .. } => return Err(EngineError::Data(format!("bot {}: error marker kind {kind:?}", p.bot_id))),
        Notice::StoppedByError { .. } => format!("$.{STOPPED}"),
        Notice::StoppedByAmountLimit => format!("$.{LIMIT}"),
    };
    let tx = model::immediate(c)?;
    let n = tx.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?1) WHERE id = ?2 AND json_extract(transient_data, ?3) = ?4",
                       params![path, p.bot_id, format!("{path}.stamped_at"), p.stamped_at])?;
    // The last kind gone: no empty object is left behind.
    tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{ERROR}') WHERE id = ?1 AND json_extract(transient_data, '$.{ERROR}') = '{{}}'"), [p.bot_id])?;
    if let Err(refusal) = eligibility::guard(&tx, cipher, p.bot_id) { return Err(EngineError::Ineligible(vec![refusal.reason()])); } // rolled back
    tx.commit()?;
    Ok(n == 1)
}
