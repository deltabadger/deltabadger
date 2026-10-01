//! Spec §3 placement protocol. The intent is committed (with a deadline read from the clock at that
//! moment) before AddOrder, which is sent at most once. A lost reply is resolved by cl_ord_id, and a
//! found order is recorded, filled and cleared in one transaction. "Not placed" is concluded only from a
//! complete lookup that STARTED after the deadline + 60 s.
use super::amount::{write_order_row, OrderPlan, RowKind};
use super::model::{self, Bot, Level};
use super::{polling, Clock, EngineError};
use crate::ruby::BigDec;
use crate::venue::{Venue, VenueError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub const DEADLINE_SECONDS: i64 = 10;
/// How long after `at` an intent may still be sent. Never later: absence is measured from `at`, so a send delayed by a slow
/// commit or a suspended process must not land after "not placed" became provable. Equal to Kraken's deadline, which
/// Kraken enforces server-side anyway.
pub const SEND_WINDOW_SECONDS: i64 = DEADLINE_SECONDS;
pub const ABSENCE_AFTER_SECONDS: i64 = 60;
/// Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS: definitive pre-trade rejections.
pub const PLACEMENT_SAFE_TRANSIENT_ERRORS: [&str; 2] = ["Timestamp for this request is outside of the recvWindow", "Timestamp for this request was"];

#[derive(Debug, Clone)]
pub struct Intent { pub cl_ord_id: String, pub deadline: DateTime<Utc>, pub at: DateTime<Utc>, pub plan: OrderPlan }

impl Intent {
    fn to_json(&self) -> Value {
        let p = &self.plan;
        json!({ "cl_ord_id": self.cl_ord_id, "deadline": self.deadline.to_rfc3339(), "at": self.at.to_rfc3339(), "ticker_id": p.ticker.id,
                "limit": p.limit, "price": p.price.to_s_f(), "amount": p.amount.to_s_f(), "quote_amount": p.quote_amount.to_s_f(),
                "quote_type": p.quote_type, "volume": p.volume.to_s_f() })
    }
    fn from_json(c: &Connection, bot: &Bot, v: &Value) -> Result<Self, EngineError> {
        let bad = || EngineError::Data(format!("rust_placement {v}"));
        let d = |k: &str| v[k].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(bad);
        let t = |k: &str| v[k].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc)).ok_or_else(bad);
        let b = |k: &str| v[k].as_bool().ok_or_else(bad);
        let ticker = model::ticker_for(c, bot)?.filter(|t| Some(t.id) == v["ticker_id"].as_i64()).ok_or_else(bad)?;
        Ok(Self { cl_ord_id: v["cl_ord_id"].as_str().ok_or_else(bad)?.to_string(), deadline: t("deadline")?, at: t("at")?,
                  plan: OrderPlan { ticker, limit: b("limit")?, price: d("price")?, amount: d("amount")?, quote_amount: d("quote_amount")?,
                                    quote_type: b("quote_type")?, volume: d("volume")? } })
    }
}

/// Re-read under the write lock: is the intent with this cl_ord_id still the bot's unresolved one?
fn still_pending(c: &Connection, bot_id: i64, cl_ord_id: &str) -> Result<bool, EngineError> {
    Ok(model::load_bot(c, bot_id)?.rust_placement().is_some_and(|v| v["cl_ord_id"] == cl_ord_id))
}
fn gone(bot_id: i64) -> EngineError { EngineError::Data(format!("bot {bot_id} has no matching unresolved order")) }

// A Rust-only key, written and removed without touching updated_at: a clean tick leaves nothing Rails would not.
fn set_intent(c: &Connection, bot_id: i64, v: Option<&Value>) -> Result<(), EngineError> {
    match v {
        Some(v) => c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json(?1)) WHERE id = ?2", params![v.to_string(), bot_id])?,
        None => c.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement') WHERE id = ?1", [bot_id])?,
    };
    Ok(())
}

pub fn begin(c: &Connection, bot: &Bot, plan: &OrderPlan, clock: &dyn Clock) -> Result<Intent, EngineError> {
    let tx = model::immediate(c)?; // check-and-set under one write lock
    if model::load_bot(&tx, bot.id)?.rust_placement().is_some() {
        return Err(EngineError::Data(format!("bot {} already has an unresolved order", bot.id)));
    }
    let now = clock.now(); // the deadline must be in the future when Kraken receives the order
    let intent = Intent { cl_ord_id: uuid::Uuid::new_v4().to_string(), deadline: now + Duration::seconds(DEADLINE_SECONDS), at: now, plan: plan.clone() };
    set_intent(&tx, bot.id, Some(&intent.to_json()))?;
    tx.commit()?; // durable before the send
    Ok(intent)
}

#[derive(Debug)]
pub enum Sent { Accepted(String), Rejected(Vec<String>), Ambiguous(String), NotSent(String) }

pub async fn send<V: Venue>(venue: &V, intent: &Intent, clock: &dyn Clock) -> Sent {
    if clock.now() > intent.at + Duration::seconds(SEND_WINDOW_SECONDS) {
        return Sent::NotSent(format!("the order intent from {} is older than {SEND_WINDOW_SECONDS} s; not sent", intent.at.to_rfc3339()));
    }
    let rules = venue.rules();
    match venue.add_order(&intent.plan.to_order(intent.cl_ord_id.clone(), intent.deadline, rules.wire)).await {
        Ok(txid) => Sent::Accepted(txid),
        Err(VenueError::Rejected(e)) if rules.add_outcome_unknown(&e) => Sent::Ambiguous(crate::ruby::to_sentence(&e)),
        Err(VenueError::Rejected(e)) => Sent::Rejected(e),
        Err(VenueError::Ambiguous(m)) => Sent::Ambiguous(m),
        Err(VenueError::Transient(m)) => Sent::NotSent(m),
    }
}

pub fn record_accepted(c: &Connection, bot: &Bot, intent: &Intent, txid: &str) -> Result<i64, EngineError> {
    let tx = model::immediate(c)?;
    if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Err(gone(bot.id)); }
    let id = write_order_row(&tx, bot, &intent.plan, RowKind::Submitted { external_id: txid.to_string() }, intent.at)?;
    set_intent(&tx, bot.id, None)?;
    tx.commit()?;
    Ok(id)
}

pub fn record_rejected(c: &Connection, bot: &Bot, intent: &Intent, errors: &[String]) -> Result<bool, EngineError> {
    let tx = model::immediate(c)?;
    if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Err(gone(bot.id)); }
    let safe = errors.iter().any(|m| PLACEMENT_SAFE_TRANSIENT_ERRORS.iter().any(|p| m.contains(p)));
    if !safe { write_order_row(&tx, bot, &intent.plan, RowKind::Failed { errors: errors.to_vec() }, intent.at)?; }
    set_intent(&tx, bot.id, None)?;
    tx.commit()?;
    Ok(!safe)
}

/// Nothing reached the venue (VenueError::Transient).
pub fn drop_intent(c: &Connection, bot_id: i64) -> Result<(), EngineError> { set_intent(c, bot_id, None) }

#[derive(Debug)]
pub enum Recovery { NoIntent, Recorded(i64), NotPlaced, Pending }

pub async fn recover<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock) -> Result<Recovery, EngineError> {
    let Some(raw) = bot.rust_placement() else { return Ok(Recovery::NoIntent) };
    let intent = Intent::from_json(c, bot, &raw)?;
    let rules = venue.rules();
    // The last moment the order could still reach the venue: Kraken drops it after the deadline it was sent with; a
    // venue without one may receive it until the client gives up (VenueRules::reach_within_secs after `at`).
    let reach_by = if rules.deadline_sent { intent.deadline } else { intent.at + Duration::seconds(rules.reach_within_secs) };
    let started = clock.now(); // only a scan that starts after the cutoff can prove absence
    match venue.order_by_client_id(&intent.cl_ord_id, intent.at - Duration::hours(1)).await {
        Ok(Some(state)) => {
            let now = clock.now();
            let tx = model::immediate(c)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            let id = write_order_row(&tx, bot, &intent.plan, RowKind::Submitted { external_id: state.txid.clone() }, intent.at)?;
            polling::apply_in(&tx, bot.id, id, &state, true, now)?; // as FetchAndUpdateOrderJob would, after placement
            set_intent(&tx, bot.id, None)?;
            tx.commit()?;
            Ok(Recovery::Recorded(id))
        }
        Ok(None) if started >= reach_by + Duration::seconds(ABSENCE_AFTER_SECONDS) => {
            let tx = model::immediate(c)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            set_intent(&tx, bot.id, None)?;
            model::log_activity(&tx, bot.id, "placement_ambiguous", Level::Warning,
                json!({ "error": format!("the order never reached {}", rules.name), "resolution": "not_placed", "source": "rust", "cl_ord_id": intent.cl_ord_id }), started)?;
            tx.commit()?;
            Ok(Recovery::NotPlaced)
        }
        Ok(None) | Err(_) => Ok(Recovery::Pending),
    }
}

#[derive(Debug)]
pub enum OperatorResolution { Placed(String), NotPlaced }

/// `deltabadger resolve-placement`: a human checked Kraken's own site because Kraken's API could not answer.
pub fn resolve_by_operator(c: &Connection, bot_id: i64, resolution: OperatorResolution, now: DateTime<Utc>) -> Result<(), EngineError> {
    if let OperatorResolution::Placed(t) = &resolution {
        if t.trim().is_empty() { return Err(EngineError::Data("an order id is required".into())); }
    }
    let tx = model::immediate(c)?;
    let bot = model::load_bot(&tx, bot_id)?; // under the write lock, so two resolvers cannot both pass
    let raw = bot.rust_placement().ok_or_else(|| EngineError::Data(format!("bot {bot_id} has no unresolved order")))?;
    let intent = Intent::from_json(&tx, &bot, &raw)?;
    if let OperatorResolution::Placed(t) = &resolution {
        let dup: i64 = tx.query_row("SELECT count(*) FROM transactions WHERE exchange_id = ?1 AND external_id = ?2", params![bot.exchange_id, t], |r| r.get(0))?;
        if dup > 0 { return Err(EngineError::Data(format!("order {t} is already recorded"))); }
    }
    let (label, txid) = match resolution {
        OperatorResolution::Placed(txid) => { write_order_row(&tx, &bot, &intent.plan, RowKind::Submitted { external_id: txid.clone() }, intent.at)?; ("placed", Some(txid)) }
        OperatorResolution::NotPlaced => ("not_placed", None),
    };
    set_intent(&tx, bot_id, None)?;
    model::log_activity(&tx, bot_id, "placement_ambiguous", Level::Warning,
        json!({ "error": "resolved by the operator", "resolution": label, "source": "operator", "cl_ord_id": intent.cl_ord_id, "order_id": txid }), now)?;
    tx.commit()?;
    Ok(())
}
