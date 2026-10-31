//! PLAN-A9 O3: an order sent through MCP or the REST API naming a signal bot (D-ORDER-QUEUED). The web validates it, writes
//! one `rust_api_orders` row (`queued`) and waits; the engine takes it at the start of a pass and between bots, places it
//! through the bot (engine::signal::run_api_order) and answers it (`done`, `outcome`). Rails places it in the request
//! under the exchange's lease (BotApi::Orders::BotOrder); here a request never calls a venue, and a crash between send and
//! record is the intent's recovery's, as for a webhook call.
//!
//! The states: `queued` (the web's) → `taken` (this engine's; nothing sent) → `sending` (consumed with the order's intent,
//! placement::begin_signal) → `done`; or `queued` → `withdrawn` (the web gave up first; exactly one of take and withdraw
//! wins). The web deletes a `done` row once it has read it; what nobody read is swept after an hour.
use super::model::{self, CredentialVersion};
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::ruby::BigDec;
use crate::venue::Side;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// BotOrder::LEASE_WAIT: a request no pass took within this is withdrawn by the web (`venue_busy`, nothing placed).
pub const WINDOW_SECONDS: i64 = 10;
/// The last instant after `created_at` an order request may leave (PLAN-A9 r1 §2): past it, `busy`, nothing sent.
pub const SEND_SECONDS: i64 = 60;
/// How long one drain may keep taking requests, so polls, recovery and ticks still run under sustained API traffic.
pub const DRAIN: std::time::Duration = std::time::Duration::from_secs(10);
/// How long an answered or withdrawn row nobody read stays.
const KEEP_HOURS: i64 = 1;

/// What the engine answers a request with (bot_api::orders maps each to Rails' result).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// The venue holds the order; its row (None only if recovery recorded it first) and the venue's id.
    Submitted { transaction_id: Option<i64>, order_id: String },
    /// Below the venue's minimum: a skipped row.
    Skipped,
    /// An earlier order of the bot is unresolved: nothing sized or sent.
    Unresolved,
    /// A wash-sale lock refused the buy: nothing recorded.
    Locked,
    /// The request may have gone out: the marker and a placement_ambiguous line, no row.
    Ambiguous { errors: Vec<String> },
    /// Nothing reached the venue, or it refused: a failed row.
    Failed { errors: Vec<String> },
    /// BotOrder's own refusal on the bot as the engine found it (its code and text), or the market gate's.
    Refused { code: String, message: String },
    /// An API buy the spending cap does not cover: what it leaves, in quote.
    CapReached { left: String },
    /// Nothing was placed: taken too late, or past its send deadline.
    Busy,
    /// A crash left it mid-flight; nobody waits for it.
    Unknown,
}

/// A request as the engine took it.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: i64,
    pub user_id: i64,
    pub bot_id: i64,
    pub side: Side,
    pub amount: BigDec,
    pub in_base: bool,
    /// The caller's exchange_name/base_asset/quote_asset, null where left out (BotOrder#pair_mismatch).
    pub supplied: serde_json::Map<String, Value>,
    pub credential_version: Option<CredentialVersion>,
    pub created_at: DateTime<Utc>,
    pub send_by: DateTime<Utc>,
}

/// What the web writes: the bot, the order as the caller sized it, and what validation saw.
pub struct New<'a> { pub user_id: i64, pub bot: &'a model::Bot, pub side: Side, pub amount: &'a BigDec, pub in_base: bool, pub supplied: &'a serde_json::Map<String, Value> }

/// The request, with the credential version of the bot's key as validation saw it (in the same database job).
pub fn insert(c: &Connection, new: &New<'_>, now: DateTime<Utc>) -> Result<i64, EngineError> {
    let version = model::credential_version(c, new.bot)?;
    let send_by = now.checked_add_signed(Duration::seconds(SEND_SECONDS)).ok_or_else(super::schedule::time_range_error)?;
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, credential_version, state, created_at, send_by) \
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9)",
              params![new.user_id, new.bot.id, new.side.as_str(), new.amount.to_persisted().map_err(|e| EngineError::Data(format!("api order amount {e:?}")))?,
                      if new.in_base { "base" } else { "quote" }, Value::Object(new.supplied.clone()).to_string(),
                      version.map(|v| v.cache_stamp().to_string()), format_time(now), format_time(send_by)])?;
    Ok(c.last_insert_rowid())
}

/// The web's read while it waits: the state, and the outcome once `done`.
pub fn state(c: &Connection, id: i64) -> Result<Option<(String, Option<Outcome>)>, EngineError> {
    let row: Option<(String, Option<String>)> = c.query_row("SELECT state, outcome FROM rust_api_orders WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    row.map(|(state, outcome)| Ok((state, outcome.map(|o| serde_json::from_str(&o).map_err(|e| EngineError::Data(format!("api order outcome: {e}")))).transpose()?))).transpose()
}

/// The web gave up on a request no pass took: true when it was still queued (nothing will be sent).
pub fn withdraw(c: &Connection, id: i64) -> Result<bool, EngineError> {
    Ok(c.execute("UPDATE rust_api_orders SET state = 'withdrawn' WHERE id = ?1 AND state = 'queued'", [id])? == 1)
}

/// The web read the answer.
pub fn forget(c: &Connection, id: i64) -> Result<(), EngineError> {
    c.execute("DELETE FROM rust_api_orders WHERE id = ?1 AND state IN ('done', 'withdrawn')", [id])?;
    Ok(())
}

/// Every queued request, oldest first.
pub fn queued(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare("SELECT id FROM rust_api_orders WHERE state = 'queued' ORDER BY id")?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}

/// Takes a queued request (exactly one of this and `withdraw` wins).
pub fn take(c: &Connection, id: i64, now: DateTime<Utc>) -> Result<Option<Request>, EngineError> {
    if c.execute("UPDATE rust_api_orders SET state = 'taken', taken_at = ?2 WHERE id = ?1 AND state = 'queued'", params![id, format_time(now)])? != 1 { return Ok(None); }
    type Row = (i64, Option<i64>, String, String, String, String, Option<String>, String, String);
    let (user_id, bot_id, side, amount, amount_type, supplied, version, created_at, send_by): Row = c.query_row(
        "SELECT user_id, bot_id, side, amount, amount_type, supplied, credential_version, created_at, send_by FROM rust_api_orders WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?;
    let bad = |what: &str| EngineError::Data(format!("api order {id}: {what}"));
    let time = |t: &str| parse_time(t).map_err(|_| bad("time"));
    Ok(Some(Request {
        id, user_id, bot_id: bot_id.ok_or_else(|| bad("no bot"))?,
        side: match side.as_str() { "buy" => Side::Buy, "sell" => Side::Sell, _ => return Err(bad("side")) },
        amount: BigDec::parse(&amount).map_err(|_| bad("amount"))?, in_base: amount_type == "base",
        supplied: serde_json::from_str(&supplied).map_err(|_| bad("supplied"))?,
        credential_version: version.map(|v| serde_json::from_str(&v).ok().and_then(|v| CredentialVersion::from_stamp(&v)).ok_or_else(|| bad("credential version"))).transpose()?,
        created_at: time(&created_at)?, send_by: time(&send_by)?,
    }))
}

/// Whether this engine still holds the request, nothing sent (placement::begin_signal, in the claim's transaction).
pub fn still_taken(c: &Connection, id: i64) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT state FROM rust_api_orders WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional()?.as_deref() == Some("taken"))
}

/// Consumed with the order's intent, in its transaction.
pub fn sending(c: &Connection, id: i64, cl_ord_id: &str) -> Result<(), EngineError> {
    c.execute("UPDATE rust_api_orders SET state = 'sending', cl_ord_id = ?2 WHERE id = ?1 AND state = 'taken'", params![id, cl_ord_id])?;
    Ok(())
}

/// The answer, in the transaction of whatever the order wrote (its row, its marker), or its own.
pub fn finish(c: &Connection, id: i64, outcome: &Outcome) -> Result<(), EngineError> {
    let text = serde_json::to_string(outcome).map_err(|e| EngineError::Data(format!("api order outcome: {e}")))?;
    c.execute("UPDATE rust_api_orders SET state = 'done', outcome = ?2 WHERE id = ?1 AND state IN ('taken', 'sending')", params![id, text])?;
    Ok(())
}

/// At engine start: the process that waited for these died with the last one. A queued request is withdrawn, a taken one
/// (nothing sent) answered `busy`, a sending one (its intent is recovery's) `unknown`; old answered rows go.
pub fn sweep_at_start(c: &Connection, now: DateTime<Utc>) -> Result<(), EngineError> {
    let busy = serde_json::to_string(&Outcome::Busy).map_err(|e| EngineError::Data(e.to_string()))?;
    let unknown = serde_json::to_string(&Outcome::Unknown).map_err(|e| EngineError::Data(e.to_string()))?;
    model::locked(c, |tx| {
        tx.execute("UPDATE rust_api_orders SET state = 'withdrawn' WHERE state = 'queued'", [])?;
        tx.execute("UPDATE rust_api_orders SET state = 'done', outcome = ?1 WHERE state = 'taken'", [&busy])?;
        let mut s = tx.prepare("SELECT id, bot_id, cl_ord_id FROM rust_api_orders WHERE state = 'sending'")?;
        for row in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?)))? {
            let (id, bot, cl_ord_id) = row?;
            super::log(&format!("[engine] api order {id} (bot {bot:?}, cl_ord_id {cl_ord_id:?}) was mid-flight at a restart; its intent's recovery settles it"));
        }
        drop(s);
        tx.execute("UPDATE rust_api_orders SET state = 'done', outcome = ?1 WHERE state = 'sending'", [&unknown])?;
        let before = now.checked_sub_signed(Duration::hours(KEEP_HOURS)).ok_or_else(super::schedule::time_range_error)?;
        tx.execute("DELETE FROM rust_api_orders WHERE state IN ('done', 'withdrawn') AND created_at < ?1", [format_time(before)])?;
        Ok(())
    })
}
