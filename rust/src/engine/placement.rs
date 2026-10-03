//! Placement protocol. The intent is committed (with a deadline read from the clock at that
//! moment) before AddOrder, which is sent at most once. A lost reply is resolved by cl_ord_id, and a
//! found order is recorded, filled and cleared in one transaction. "Not placed" is concluded only from a
//! complete lookup that STARTED after the deadline + 60 s (Kraken), or 20 minutes after both the intent and the
//! process start on a venue without a server-side deadline (Alpaca, VenueRules::absence_margin_secs).
use super::amount::{write_order_row, OrderPlan, RowKind};
use super::model::{self, Bot, Level};
use super::schedule::{checkpoints, effective};
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
                "base_asset_id": p.ticker.base_asset_id,
                "limit": p.limit, "price": p.price.to_s_f(), "amount": p.amount.to_s_f(), "quote_amount": p.quote_amount.to_s_f(),
                "quote_type": p.quote_type, "volume": p.volume.to_s_f() })
    }
    fn from_json(c: &Connection, bot: &Bot, v: &Value) -> Result<Self, EngineError> {
        let bad = || EngineError::Data(format!("rust_placement {v}"));
        let d = |k: &str| v[k].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(bad);
        let t = |k: &str| v[k].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc)).ok_or_else(bad);
        let b = |k: &str| v[k].as_bool().ok_or_else(bad);
        // The intent names its own ticker: a basket's leg k is not the bot's first member. base_asset_id is for the logs; an
        // intent written by an earlier build has none.
        let ticker = model::ticker_by_id(c, bot.exchange_id, v["ticker_id"].as_i64().ok_or_else(bad)?)?.ok_or_else(bad)?;
        Ok(Self { cl_ord_id: v["cl_ord_id"].as_str().ok_or_else(bad)?.to_string(), deadline: t("deadline")?, at: t("at")?,
                  plan: OrderPlan { ticker, limit: b("limit")?, price: d("price")?, amount: d("amount")?, quote_amount: d("quote_amount")?,
                                    quote_type: b("quote_type")?, volume: d("volume")? } })
    }
}

/// Bots whose unresolved order (`rust_placement`) was sent under another composition, asset, exchange or quote than the
/// row holds now. `eligibility::guard` refuses ANY such change while the order is unresolved. Recovery would cope, since
/// `Intent::from_json` finds the order's ticker by its id whatever the bot's settings, but Rails freezes a working bot's
/// composition too, and one rule is simpler and safe. The intent records what it was sent under (`exchange_id`,
/// `quote_asset_id`, `allocations`, written by `begin`; one written by an earlier build gets them at takeover,
/// `backfill_snapshots`). An intent without them counts as changed: fail closed.
pub fn stranded(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL ORDER BY id")?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    let mut out = vec![];
    for id in ids {
        let bot = model::load_bot(c, id)?;
        let Some(v) = bot.rust_placement() else { continue };
        let changed = match v.get("allocations") {
            Some(sent) => v["exchange_id"].as_i64() != Some(bot.exchange_id) || v["quote_asset_id"].as_i64() != bot.quote_asset_id()
                || bot.settings.get("allocations") != Some(sent),
            None => true,
        };
        if changed { out.push(id); }
    }
    Ok(out)
}

/// Gives every unresolved intent written by an earlier build the snapshot `begin` now records, from the bot's row. Exact:
/// such intents come only from the earlier engine, which had no web UI and let nothing else write, so the row is what the
/// order was sent under. `handover::take_over` calls it before the engine ticks or the web serves a request. Each write is
/// one key-scoped statement. Returns how many it filled.
pub fn backfill_snapshots(c: &Connection) -> Result<usize, EngineError> {
    let mut s = c.prepare("SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement') IS NOT NULL \
                           AND json_type(transient_data, '$.rust_placement.allocations') IS NULL ORDER BY id")?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    for id in &ids {
        let bot = model::load_bot(c, *id)?;
        let allocations = bot.settings.get("allocations").cloned().unwrap_or(Value::Null);
        c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.exchange_id', ?1, \
                   '$.rust_placement.quote_asset_id', ?2, '$.rust_placement.allocations', json(?3)) WHERE id = ?4",
                  params![bot.exchange_id, bot.quote_asset_id(), allocations.to_string(), id])?;
    }
    Ok(ids.len())
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

/// After a run is rescheduled (an intent settled, placed or not, or a failure that reschedules), nothing is placed before
/// the bot's next checkpoint, as Bot::ActionJob's next run of a failed run is at next_interval_checkpoint_at
/// (action_job.rb:325-328). Written with `json_set` (in the transaction that settles an intent), so a restart honours it
/// (run::step_bot), together with the schedule it was computed under: a fresh start or an interval edit voids it. The
/// deferred tick removes it (tick::tick_recovering). A Rust-only key, outside the parity snapshots, like `rust_placement`.
pub fn defer_to_next_checkpoint(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<(), EngineError> {
    wait_until(c, bot, now, |cps| cps.next_us)
}

/// A wait that has already ended (at the bot's last checkpoint): the bot is due at once, across a restart too, until its next
/// tick removes it. What a continue start that Rails runs at once leaves (run::step_bot).
pub fn run_now(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<(), EngineError> {
    wait_until(c, bot, now, |cps| cps.last_us)
}

fn wait_until(c: &Connection, bot: &Bot, now: DateTime<Utc>, pick: fn(super::schedule::Checkpoints) -> i64) -> Result<(), EngineError> {
    let (Some(anchor), Some(interval), Some(quote), Some(schedule)) = (bot.started_at_us, bot.interval(), bot.quote_amount(), bot.schedule_key()) else { return Ok(()) };
    let at = pick(checkpoints(anchor, now.timestamp_micros(), effective(interval, quote, bot.smart_quote_amount())));
    let until = DateTime::from_timestamp_micros(at).expect("time in range").to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_defer_until', json(?1)) WHERE id = ?2",
              params![json!({ "until": until, "schedule": schedule }).to_string(), bot.id])?;
    Ok(())
}

/// The intent for `plan`, written without the fence: for a caller that already holds the row it sized from (tests, recovery
/// fixtures). The tick uses `begin_unless_changed`.
pub fn begin(c: &Connection, bot: &Bot, plan: &OrderPlan, clock: &dyn Clock) -> Result<Intent, EngineError> {
    Ok(begin_checked(c, bot, plan, clock, false)?.expect("unfenced"))
}

/// `begin`, fenced: under the same write lock, the bot must still be working and its composition (exchange, quote asset and
/// allocations, compared by value as `stranded` compares them) must still be what `sized_from` holds. Otherwise no intent is
/// written, nothing will be sent, and `None` is returned with one log line naming the reason; the next pass sees the new row.
///
/// A divergence from Rails, whose leg loop places an order sized before a stop or an edit landed. It only ever removes such
/// an order; what it would have bought stays owed through pending_quote_amount.
pub fn begin_unless_changed(c: &Connection, sized_from: &Bot, plan: &OrderPlan, clock: &dyn Clock) -> Result<Option<Intent>, EngineError> {
    begin_checked(c, sized_from, plan, clock, true)
}

fn begin_checked(c: &Connection, bot: &Bot, plan: &OrderPlan, clock: &dyn Clock, fence: bool) -> Result<Option<Intent>, EngineError> {
    let tx = model::immediate(c)?; // check-and-set under one write lock
    let current = model::load_bot(&tx, bot.id)?;
    if current.rust_placement().is_some() {
        return Err(EngineError::Data(format!("bot {} already has an unresolved order", bot.id)));
    }
    if fence {
        let reason = if !crate::enums::BOT_WORKING.contains(&current.status) { Some("it was stopped") }
            else if (current.exchange_id, current.quote_asset_id(), current.settings.get("allocations"))
                != (bot.exchange_id, bot.quote_asset_id(), bot.settings.get("allocations")) { Some("its composition changed") }
            else { None };
        if let Some(reason) = reason {
            super::log(&format!("[engine] bot {}: {} order not placed: {reason} after it was sized", bot.id, plan.ticker.ticker));
            return Ok(None);
        }
    }
    let now = clock.now(); // the deadline must be in the future when Kraken receives the order
    let intent = Intent { cl_ord_id: uuid::Uuid::new_v4().to_string(), deadline: now + Duration::seconds(DEADLINE_SECONDS), at: now, plan: plan.clone() };
    // What the order is sent under: `stranded` refuses any change to it until the order settles.
    let mut v = intent.to_json();
    v["exchange_id"] = json!(current.exchange_id);
    v["quote_asset_id"] = json!(current.quote_asset_id());
    v["allocations"] = current.settings.get("allocations").cloned().unwrap_or(Value::Null);
    set_intent(&tx, bot.id, Some(&v))?;
    tx.commit()?; // durable before the send
    Ok(Some(intent))
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

/// Nothing reached the venue (VenueError::Transient, or the send window refused the send).
pub fn drop_intent(c: &Connection, bot_id: i64) -> Result<(), EngineError> { set_intent(c, bot_id, None) }

#[derive(Debug)]
pub enum Recovery { NoIntent, Recorded(i64), NotPlaced, Pending }

pub async fn recover<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock) -> Result<Recovery, EngineError> {
    recover_since(c, venue, bot, clock, DateTime::<Utc>::MIN_UTC).await
}

/// `recover` by a process that started at `process_start`.
pub async fn recover_since<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, process_start: DateTime<Utc>) -> Result<Recovery, EngineError> {
    let Some(raw) = bot.rust_placement() else { return Ok(Recovery::NoIntent) };
    let intent = Intent::from_json(c, bot, &raw)?;
    let rules = venue.rules();
    // Kraken drops an order after the deadline it was sent with, so absence is provable from deadline + 60 s whatever the
    // process start. A venue without one (VenueRules::absence_margin_secs): the lookup must start a full margin after the
    // intent AND after this process started, because a process suspended (or killed and restarted) after its send cannot
    // vouch for when that send left.
    let absent_from = if rules.deadline_sent { intent.deadline + Duration::seconds(ABSENCE_AFTER_SECONDS) }
                      else { intent.at.max(process_start) + Duration::seconds(rules.absence_margin_secs) };
    let started = clock.now(); // only a scan that starts after the cutoff can prove absence
    match venue.order_by_client_id(&intent.cl_ord_id, intent.at - Duration::hours(1)).await {
        Ok(Some(state)) => {
            let now = clock.now();
            let tx = model::immediate(c)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            let id = write_order_row(&tx, bot, &intent.plan, RowKind::Submitted { external_id: state.txid.clone() }, intent.at)?;
            // The intent goes before the fill is applied: the amount cap counts an unresolved intent as spent, and this one is
            // now its row, which polling's stop trigger must not count twice.
            set_intent(&tx, bot.id, None)?;
            // As FetchAndUpdateOrderJob would, after placement; its amount-limit stop lands with the fill (the tick ends here).
            if polling::apply_in(&tx, bot.id, id, &state, true, now)? { super::tick::stop_for_amount_limit(&tx, bot.id, now)?; }
            defer_to_next_checkpoint(&tx, bot, now)?;
            tx.commit()?;
            Ok(Recovery::Recorded(id))
        }
        Ok(None) if started >= absent_from => {
            let tx = model::immediate(c)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            set_intent(&tx, bot.id, None)?;
            defer_to_next_checkpoint(&tx, bot, started)?;
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

/// `deltabadger resolve-placement`: a human checked the venue (Kraken's or Alpaca's own site) because its API could not answer.
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
    defer_to_next_checkpoint(&tx, &bot, now)?;
    model::log_activity(&tx, bot_id, "placement_ambiguous", Level::Warning,
        json!({ "error": "resolved by the operator", "resolution": label, "source": "operator", "cl_ord_id": intent.cl_ord_id, "order_id": txid }), now)?;
    tx.commit()?;
    Ok(())
}
