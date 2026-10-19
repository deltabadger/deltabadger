//! Placement protocol. The intent is committed (with a deadline read from the clock at that
//! moment) before AddOrder, which is sent at most once. A lost reply is resolved by cl_ord_id, and a
//! found order is recorded, filled and cleared in one transaction. "Not placed" is concluded only from a
//! complete lookup that STARTED after the deadline + 60 s (Kraken), or 20 minutes after both the intent and the
//! process start on a venue without a server-side deadline (Alpaca, VenueRules::absence_margin_secs).
use crate::venue::Attributed;
use super::amount::{write_accepted_order, write_order_row, OrderPlan, RowKind, TxKind};
use super::model::{self, Bot, Level};
use super::schedule::checkpoints;
use super::{polling, Clock, EngineError};
use crate::ruby::BigDec;
use crate::venue::{Side, Venue, VenueError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub const DEADLINE_SECONDS: i64 = 10;
/// How long after `at` an intent may still be sent. Never later: absence is measured from `at`, so a send delayed by a slow
/// commit or a suspended process must not land after "not placed" became provable. Equal to Kraken's deadline, which
/// Kraken enforces server-side anyway.
pub const SEND_WINDOW_SECONDS: i64 = DEADLINE_SECONDS;
pub const ABSENCE_AFTER_SECONDS: i64 = 60;
/// Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS: definitive pre-trade rejections, plus Exchanges::Kucoin::ERRORS[:placement_safe_transient]
/// (400002). Rails scopes the KuCoin string to KuCoin; it is listed for every venue here, a string only KuCoin sends.
pub const PLACEMENT_SAFE_TRANSIENT_ERRORS: [&str; 3] = ["Timestamp for this request is outside of the recvWindow", "Timestamp for this request was", "Invalid KC-API-TIMESTAMP"];

#[derive(Debug, Clone)]
pub struct Intent { pub cl_ord_id: String, pub deadline: DateTime<Utc>, pub at: DateTime<Utc>, pub plan: OrderPlan,
    /// The pair the order was sent for, as the venue names it (recorded: the ticker row's name can change after the send).
    pub symbol: String,
    /// In-memory only; no secret, digest or credential is persisted in an intent.
    credential_version: Option<model::CredentialVersion>,
    bot_id: i64,
    run: Option<model::RunIdentity>,
    /// Who placed an order outside a DCA run or a rebalance: "signal" (a webhook call of a signal bot, engine::signal).
    pub source: Option<String>,
}

impl Intent {
    /// A signal bot's order (`begin_signal`).
    pub fn signal(&self) -> bool { self.source.as_deref() == Some(SIGNAL_SOURCE) }
    pub fn bot_id(&self) -> i64 { self.bot_id }
    /// `Err` when a value would not read back through `from_json` (BigDec::to_persisted): such an intent is never written.
    fn to_json(&self) -> Result<Value, EngineError> {
        let p = &self.plan;
        let d = |name: &str, v: &BigDec| v.to_persisted().map_err(|e| EngineError::Data(format!("the order's {name} {e:?}; not placed")));
        Ok(json!({ "cl_ord_id": self.cl_ord_id, "deadline": self.deadline.to_rfc3339(), "at": self.at.to_rfc3339(), "ticker_id": p.ticker.id, "symbol": self.symbol,
                   "base_asset_id": p.ticker.base_asset_id,
                   "limit": p.limit, "price": d("price", &p.price)?, "amount": d("amount", &p.amount)?, "quote_amount": d("quote_amount", &p.quote_amount)?,
                   "quote_type": p.quote_type, "volume": d("volume", &p.volume)?, "side": p.side.as_str(), "transaction_type": p.kind.as_str(), "run_identity": self.run.as_ref().map(model::RunIdentity::to_json),
                   "source": self.source }))
    }
    fn from_json(c: &Connection, bot: &Bot, v: &Value, producer:Option<model::CredentialVersion>) -> Result<Self, EngineError> {
        let bad = || EngineError::Data(format!("rust_placement {v}"));
        let d = |k: &str| v[k].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(bad);
        let t = |k: &str| crate::codec::optional_time(v.get(k)).map_err(|_|bad())?.ok_or_else(bad);
        let b = |k: &str| v[k].as_bool().ok_or_else(bad);
        // The intent names its own ticker: a basket's leg k is not the bot's first member. base_asset_id is for the logs; an
        // intent written by an earlier build has none.
        let ticker = model::ticker_by_id(c, bot.exchange_id, v["ticker_id"].as_i64().ok_or_else(bad)?)?.ok_or_else(bad)?;
        // An intent written by an earlier build names no symbol: its own ticker's (by id), never the bot's current pair.
        let symbol = v["symbol"].as_str().map_or_else(|| ticker.ticker.clone(), str::to_string);
        // An intent written before selling existed names no side: it was a buy. Anything else unreadable is refused.
        let side = match v.get("side") { None | Some(Value::Null) => Side::Buy, Some(x) if x == "buy" => Side::Buy, Some(x) if x == "sell" => Side::Sell, _ => return Err(bad()) };
        // An intent written before rebalancing existed names no kind: it was a REGULAR order.
        let kind = match v.get("transaction_type") { None | Some(Value::Null) => TxKind::Regular, Some(x) => x.as_str().and_then(TxKind::parse).ok_or_else(bad)? };
        let source = match v.get("source") { None | Some(Value::Null) => None, Some(Value::String(s)) => Some(s.clone()), Some(_) => return Err(bad()) };
        Ok(Self { source, symbol, bot_id: bot.id, run: v.get("run_identity").filter(|v| !v.is_null()).map(|v| model::RunIdentity::from_json(v).ok_or_else(bad)).transpose()?, credential_version: producer, cl_ord_id: v["cl_ord_id"].as_str().ok_or_else(bad)?.to_string(), deadline: t("deadline")?, at: t("at")?,
                  plan: OrderPlan { ticker, side, limit: b("limit")?, price: d("price")?, amount: d("amount")?, quote_amount: d("quote_amount")?,
                                    quote_type: b("quote_type")?, volume: d("volume")?, kind } })
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
                || bot.settings.get("allocations").unwrap_or(&Value::Null) != sent
                || v.get("composition").is_some_and(|sent| composition_snapshot(c, &bot).map_or(true, |current| *sent != current)),
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
pub(crate) fn still_pending(c: &Connection, bot_id: i64, cl_ord_id: &str) -> Result<bool, EngineError> {
    Ok(model::load_bot(c, bot_id)?.rust_placement().is_some_and(|v| v["cl_ord_id"] == cl_ord_id))
}
fn gone(bot_id: i64) -> EngineError { EngineError::Data(format!("bot {bot_id} has no matching unresolved order")) }

// A Rust-only key, written and removed without touching updated_at: a clean tick leaves nothing Rails would not.
pub(crate) fn set_intent(c: &Connection, bot_id: i64, v: Option<&Value>) -> Result<(), EngineError> {
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

/// A failure deferral uses the producing fenced handle, never a post-call lookup.
pub fn defer_failure(c:&model::FencedTransaction<'_>,bot:&Bot,now:DateTime<Utc>)->Result<(),EngineError>{
    defer_to_next_checkpoint(c,bot,now)?;
    let producer=c.producer_version().map(|version|version.cache_stamp());
    c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_defer_until.origin','venue','$.rust_defer_until.producer',json(?1)) WHERE id=?2",params![serde_json::to_string(&producer).map_err(|_|EngineError::Data("failure producer serialization failed".into()))?,bot.id])?;
    Ok(())
}

/// A wait that has already ended (at the bot's last checkpoint): the bot is due at once, across a restart too, until its next
/// tick removes it. What a continue start that Rails runs at once leaves (run::step_bot).
pub fn run_now(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<(), EngineError> {
    wait_until(c, bot, now, |cps| cps.last_us)
}

/// Removes `rust_defer_until`: one bot's, or every bot's (`None`, the handback). The one statement every path uses.
pub fn remove_wait(c: &Connection, bot_id: Option<i64>) -> Result<(), EngineError> {
    c.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_defer_until') \
               WHERE (?1 IS NULL OR id = ?1) AND json_type(transient_data, '$.rust_defer_until') IS NOT NULL", [bot_id])?;
    Ok(())
}

fn wait_until(c: &Connection, bot: &Bot, now: DateTime<Utc>, pick: fn(super::schedule::Checkpoints) -> i64) -> Result<(), EngineError> {
    let (Some(anchor), Some(eff), Some(schedule)) = (bot.started_at_us()?, bot.effective(), bot.schedule_key()?) else { return Ok(()) };
    let at = pick(checkpoints(anchor, now.timestamp_micros(), eff)?);
    let until = DateTime::from_timestamp_micros(at).ok_or_else(super::schedule::time_range_error)?.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_defer_until', json(?1)) WHERE id = ?2",
              params![json!({ "until": until, "schedule": schedule, "origin": "local" }).to_string(), bot.id])?;
    Ok(())
}

/// Tests only: the intent for `plan`, written without the fence (a fixture may write one on a stopped bot). Engine code
/// writes intents only through `begin_unless_changed`.
pub fn begin(c: &Connection, bot: &Bot, plan: &OrderPlan, clock: &dyn Clock) -> Result<Intent, EngineError> {
    match begin_checked(c, bot, plan, clock, None, None, None, None)? {
        Begun::Intent(intent) => Ok(*intent),
        Begun::BelowMinimum(_) => Err(EngineError::Data("order below minimum after exact cap guard".into())),
        Begun::Changed | Begun::Fenced | Begun::CredentialsChanged | Begun::LedgerStale(_) => Err(EngineError::Data("unfenced placement skipped".into())),
    }
}

/// `begin`, fenced: under the same write lock, the bot must still be working and its composition (exchange, quote asset and
/// allocations, compared by value as `stranded` compares them) must still be what `sized_from` holds. Every ticker used
/// to size the basket must also match (names, asset ids, availability, precision and minimums). Otherwise no intent is
/// written, nothing will be sent, and `Begun::Changed` is returned with one log line naming the reason; the next pass sees the new row.
/// A bot no longer on the run that sized it (stopped, or stopped and started again) answers `Begun::Fenced` instead.
///
/// Rails' leg loop fences a stop, or a stop and a restart, the same way (Automation::Statusable#stopped_mid_tick?) but not an
/// edit: it places an order sized before an edit landed, a divergence. The fence only ever removes such
/// an order; what it would have bought stays owed through pending_quote_amount.
pub fn begin_unless_changed(c: &Connection, sized_from: &Bot, plan: &OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &super::splits::Snapshot, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c, sized_from, plan, clock, Some((tickers, composition, reconciled)), None, None, None)
}

/// BelowMinimum: the exact-cap guard (amount::guard_exact_cap) reduced the order below the venue minimum; the
/// leg is reported as skipped, never placed. Fenced: a stop, or a stop and a restart, took the run from the tick that
/// sized the order (Automation::Statusable::FENCED); that tick writes nothing more to the bot.
/// LedgerStale: the account ledger stopped being fresh after the order was sized (a sync started or failed meanwhile);
/// nothing was written or sent, and the tick returns its checkpoint (tick.rs) instead of ending as a success.
pub enum Begun { Intent(Box<Intent>), Changed, Fenced, CredentialsChanged, BelowMinimum(Box<OrderPlan>), LedgerStale(Box<super::staleness::Stale>) }
/// Production placement always passes the version captured with the venue credentials (L).
#[allow(clippy::too_many_arguments)]
/// `identity` is the sending venue's `placement_identity()`: recorded in the intent before the send, so recovery never asks
/// another account or another Alpaca host about this order (`identity_mismatch`).
pub fn begin_with_credentials(c: &Connection, bot: &Bot, plan: &OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &super::splits::Snapshot, version: &Option<model::CredentialVersion>, identity: Option<Value>, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c,bot,plan,clock,Some((tickers,composition,reconciled)),Some(version),identity,None)
}

/// A REBALANCE order's intent (Bot::Rebalancer#place_rebalance_order). It belongs to no DCA run: a stopped bot rebalances.
/// Its claim is what it was sized from: under the intent's write lock the bot must not be deleted or archived, and its
/// composition, member rows, split rows, tickers, eligibility and credentials must be what the sizing read, as for a DCA
/// order (a stop, a reversal or a settings edit changes the snapshot); the send checks the same claim again
/// (`rebalance_claim_holds`). `state` is the pending phase written in the intent's own transaction (set_rebalance_pending!
/// for a sell, mark_buy_attempted! for a buy), so a crash after the send always leaves both the intent and the phase.
#[allow(clippy::too_many_arguments)]
pub fn begin_rebalance(c: &Connection, bot: &Bot, plan: &OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &super::splits::Snapshot, version: &Option<model::CredentialVersion>, identity: Option<Value>, state: &super::rebalance::Pending, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c,bot,plan,clock,Some((tickers,composition,reconciled)),Some(version),identity,Some(state))
}
#[allow(clippy::too_many_arguments)]
fn begin_checked(c: &Connection, bot: &Bot, plan: &OrderPlan, clock: &dyn Clock, fence: Option<(&[&model::Ticker], &Value, &super::splits::Snapshot)>, version: Option<&Option<model::CredentialVersion>>, identity: Option<Value>, rebalance: Option<&super::rebalance::Pending>) -> Result<Begun, EngineError> {
    let tx = model::immediate(c)?; // check-and-set under one write lock
    let current = model::load_bot(&tx, bot.id)?;
    if current.rust_placement().is_some() {
        return Err(EngineError::Data(format!("bot {} already has an unresolved order", bot.id)));
    }
    // The placement barrier (plan r4): no order while a reversal's cancel of an order it captured is not recorded. Checked
    // here, under the intent's own write lock, so no schedule or race lets an order out beside one the flip is cancelling.
    if super::reversal::barred(&current) {
        super::log(&format!("[engine] bot {}: {} order not placed: a reversal's cancels are not recorded yet", bot.id, plan.ticker.ticker));
        return Ok(Begun::Changed);
    }
    if let Some(version) = version {
        let fresh=match version {Some(v)=>model::credential_is_current(&tx,v)?,None=>false};
        if !fresh {
            super::log(&format!("[engine] bot {}: credentials changed; order not placed; retry with fresh credentials", bot.id));
            return Ok(Begun::CredentialsChanged);
        }
    }
    if let Some((tickers, composition, reconciled)) = fence {
        let mut ticker_changed = false;
        for ticker in tickers {
            if model::ticker_by_id(&tx, bot.exchange_id, ticker.id)?.as_ref() != Some(*ticker) {
                ticker_changed = true;
                break;
            }
        }
        // Still the run that sized it: a stop and a start put the row back to working, so it must still be the claim this tick
        // took (Automation::Statusable#stopped_mid_tick?): executing, with the same started_at and last_action_job_at.
        let same_run = if rebalance.is_some() {
            !matches!(current.status, crate::enums::BotStatus::Deleted | crate::enums::BotStatus::Archived)
        } else {
            current.status == crate::enums::BotStatus::Executing && current.run_identity()? == bot.run_identity()?
        };
        if !same_run {
            super::log(&format!("[engine] bot {}: {} order not placed: it was stopped after it was sized", bot.id, plan.ticker.ticker));
            return Ok(Begun::Fenced);
        }
        let reason = if (current.exchange_id, current.quote_asset_id(), current.settings.get("allocations"))
                != (bot.exchange_id, bot.quote_asset_id(), bot.settings.get("allocations")) { Some("its composition changed") }
            else if &composition_snapshot(&tx, &current)? != composition { Some("its member rows or index settings changed") }
            else if current.restatement_generation != reconciled.generation { Some("its split rows changed") }
            else if let Some(stale) = super::staleness::ledger_stale(&tx, &current, clock.now())? {
                // Usually another bot's order on the same key woke the ledger sync after this one was sized. Nothing is
                // sent on that ledger; the tick hands its checkpoint back and sizes again once the sync completes.
                super::log(&format!("[engine] bot {}: {} order not placed: its account ledger needs a complete refresh after it was sized; the tick waits for the refresh and sizes again", bot.id, plan.ticker.ticker));
                return Ok(Begun::LedgerStale(Box::new(stale)));
            }
            else if ticker_changed || tickers.iter().any(|t| !t.available || !t.trading_enabled) { Some("its ticker data changed or is unavailable") }
            else if !(if rebalance.is_some() { super::eligibility::rebalance_reasons(&tx, &current)? } else { super::eligibility::bot_reasons(&tx, &current)? }).is_empty() {
                Some("its members or provider are no longer eligible")
            }
            else if super::provider::stale(&tx, &current)?.is_some() { Some("its index provider configuration changed") }
            else { None };
        if let Some(reason) = reason {
            super::log(&format!("[engine] bot {}: {} order not placed: {reason} after it was sized", bot.id, plan.ticker.ticker));
            return Ok(Begun::Changed);
        }
    }
    let guarded;
    // The exact quote cap bounds what a REGULAR BUY spends; a sell spends nothing (Rails' QuoteAmountLimitable decorates the buy
    // carry), and guard_exact_cap passes a REBALANCE buy through untouched.
    let plan = if plan.side == Side::Buy {
        let safe = match super::amount::guard_exact_cap(&tx, &current, plan)? {
            super::amount::CapGuard::Place(safe) => safe,
            super::amount::CapGuard::BelowMinimum(skipped) => return Ok(Begun::BelowMinimum(Box::new(skipped))),
        };
        guarded = safe;
        &guarded
    } else { plan };
    let now = clock.now(); // the deadline must be in the future when Kraken receives the order
    let intent = Intent { source: None, symbol: plan.ticker.ticker.clone(), bot_id: bot.id, run: if rebalance.is_some() { None } else { Some(bot.run_identity()?) }, credential_version: match version {Some(v)=>v.clone(),None=>model::credential_version(&tx,bot)?}, cl_ord_id: uuid::Uuid::new_v4().to_string(), deadline: now.checked_add_signed(Duration::seconds(DEADLINE_SECONDS)).ok_or_else(super::schedule::time_range_error)?, at: now, plan: plan.clone() };
    // Before anything is committed or sent: an intent recovery or `resolve-placement` could not read back would strand the
    // bot after a real order. Such a plan is refused as Rails fails a StandardError raised inside execute_action
    // (execution_failed, no retry; the next checkpoint sizes afresh). Dropping `tx` rolls back. The read-back resolves the
    // intent's own ticker by its id, as recovery will.
    let mut v = intent.to_json()?;
    Intent::from_json(&tx, bot, &v,intent.credential_version.clone())?;
    // What the order is sent under: `stranded` refuses any change to it until the order settles.
    v["exchange_id"] = json!(current.exchange_id);
    v["quote_asset_id"] = json!(current.quote_asset_id());
    v["allocations"] = current.settings.get("allocations").cloned().unwrap_or(Value::Null);
    v["composition"] = composition_snapshot(&tx, &current)?;
    if let Some(Value::Object(identity)) = identity { for (k, value) in identity { v[k] = value; } }
    set_intent(&tx, bot.id, Some(&v))?;
    if let Some(state) = rebalance { super::rebalance::set_pending(&tx, bot.id, state)?; }
    tx.commit()?; // durable before the send
    Ok(Begun::Intent(Box::new(intent)))
}

/// `Intent::source` of a signal bot's order.
pub const SIGNAL_SOURCE: &str = "signal";

/// What `begin_signal` found under the intent's write lock. Only `Intent` wrote one; `Stopped` and `Disabled` consumed the
/// call with its `signal_ignored` line; the others changed nothing, and the call stays for the next pass.
#[derive(Debug)]
pub enum SignalClaim { Intent(Box<Intent>), Blocked, Stopped, Disabled, Gone, Changed, CredentialsChanged }

/// A signal bot's intent (plan 8a §R4, §R10): `run: None`, `source: "signal"`. In its own immediate transaction:
/// - the barrier (engine::signal::blocked: Rails' unresolved marker or an unresolved intent) stands → `Blocked`;
/// - for a webhook `call`: it is still queued (`Gone`), the bot still works (a stop that committed first: `Stopped`, which
///   logs `signal_ignored` as Rails' job would at its start) and the rule is still on (`Disabled`, the same line);
/// - the exchange and the pair are what the order was sized from (`Changed`), and the credentials too;
/// - the call is removed in the transaction that writes the intent.
///
/// `call` is None for the one more attempt after a placement-safe rejection: Rails places it at once, whatever happened
/// meanwhile, so only the barrier, the pair and the credentials are asked.
#[allow(clippy::too_many_arguments)]
pub fn begin_signal(c: &Connection, sized_from: &Bot, plan: &OrderPlan, call: Option<&model::SignalCall>, version: &Option<model::CredentialVersion>, identity: Option<Value>, clock: &dyn Clock) -> Result<SignalClaim, EngineError> {
    let tx = model::immediate(c)?;
    let current = model::load_bot(&tx, sized_from.id)?;
    if super::signal::blocked(&current) { return Ok(SignalClaim::Blocked); }
    let now = clock.now();
    if let Some(call) = call {
        if !current.signal_calls()?.iter().any(|x| x.source == call.source) { return Ok(SignalClaim::Gone); }
        let refused = if !crate::enums::BOT_WORKING.contains(&current.status) { Some(SignalClaim::Stopped) }
            else if !model::signal(&tx, call.signal_id)?.is_some_and(|s| s.enabled) { Some(SignalClaim::Disabled) }
            else { None };
        if let Some(refused) = refused {
            super::signal::consume(&tx, current.id, call, Some("signal_ignored"), now)?;
            tx.commit()?;
            return Ok(refused);
        }
    }
    if (current.exchange_id, &current.settings) != (sized_from.exchange_id, &sized_from.settings) {
        super::log(&format!("[engine] bot {}: {} order not placed: its exchange or pair changed after it was sized", sized_from.id, plan.ticker.ticker));
        return Ok(SignalClaim::Changed);
    }
    let fresh = match version { Some(v) => model::credential_is_current(&tx, v)?, None => false };
    if !fresh {
        super::log(&format!("[engine] bot {}: credentials changed; order not placed; retry with fresh credentials", sized_from.id));
        return Ok(SignalClaim::CredentialsChanged);
    }
    let intent = Intent { source: Some(SIGNAL_SOURCE.into()), symbol: plan.ticker.ticker.clone(), bot_id: current.id, run: None, credential_version: version.clone(),
                          cl_ord_id: uuid::Uuid::new_v4().to_string(), deadline: now.checked_add_signed(Duration::seconds(DEADLINE_SECONDS)).ok_or_else(super::schedule::time_range_error)?,
                          at: now, plan: plan.clone() };
    let mut v = intent.to_json()?;
    Intent::from_json(&tx, &current, &v, intent.credential_version.clone())?;
    v["exchange_id"] = json!(current.exchange_id);
    v["quote_asset_id"] = json!(current.quote_asset_id());
    v["allocations"] = current.settings.get("allocations").cloned().unwrap_or(Value::Null);
    v["composition"] = composition_snapshot(&tx, &current)?;
    if let Some(Value::Object(identity)) = identity { for (k, value) in identity { v[k] = value; } }
    set_intent(&tx, current.id, Some(&v))?;
    if let Some(call) = call { super::signal::consume(&tx, current.id, call, None, now)?; }
    tx.commit()?; // durable before the send, the call with it
    Ok(SignalClaim::Intent(Box::new(intent)))
}

#[derive(Debug)]
pub enum Sent { Accepted(String), Rejected(Vec<String>), Ambiguous(String), NotSent(String), Fenced }

/// Acquire the dispatch write lock before rechecking the captured generation. Neither production
/// adapter (Alpaca, Kraken) has a blocking preflight or session lock between this check and HTTP send.
/// Any future adapter with such a wait must recheck after it, before transmission. STOP can
/// invalidate this run while the response is in flight; the accepted order still gets recorded.
pub async fn send<V: Venue>(c: &Connection, venue: &V, intent: &Intent, clock: &dyn Clock) -> Result<Sent, EngineError> {
    {
        let tx = model::immediate(c)?;
        if !intent_owned(&tx, intent)? || !still_pending(&tx, intent.bot_id, &intent.cl_ord_id)? {
            // Fencing proves this invocation sent nothing. Only discard its own intent
            // if no earlier dispatch may have reached the venue; keep that evidence for recovery.
            let removed = tx.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.rust_placement') \
                        WHERE id=?1 AND json_extract(transient_data,'$.rust_placement.cl_ord_id')=?2 \
                        AND coalesce(json_extract(transient_data,'$.rust_placement.dispatched'),0)=0",
                       params![intent.bot_id, intent.cl_ord_id])?;
            // Provably never sent: a REBALANCE intent's phase goes with it, as the pre-transmission rescue undoes it.
            if removed == 1 && intent.plan.kind == TxKind::Rebalance { super::rebalance::settle(&tx, intent.bot_id, &intent.plan, super::rebalance::Settlement::NotPlaced, clock.now())?; }
            tx.commit()?;
            return Ok(Sent::Fenced);
        }
        model::check_credential_result(&tx, &intent.credential_version)?;
        tx.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement.dispatched',json('true')) WHERE id=?1", [intent.bot_id])?;
        tx.commit()?;
    }
    Ok(send_to_venue(venue, intent, clock).await)
}

/// An accepted order is an immutable fact, even when STOP/restart changed the run meanwhile.
/// Record and clear its intent in a new short transaction, before any follow-up work.
pub async fn send_recorded<V: Venue>(c: &Connection, venue: &V, bot: &Bot, intent: &Intent, clock: &dyn Clock) -> Result<Sent, EngineError> {
    let sent = send(c, venue, intent, clock).await?;
    let tx = model::immediate(c)?;
    match &sent {
        Sent::Accepted(id) => { record_accepted_in(&tx, bot, intent, id, clock.now())?; }
        Sent::Rejected(errors) => { record_rejected_in(&tx, bot, intent, errors, clock.now())?; }
        Sent::NotSent(_) => {
            set_intent(&tx, bot.id, None)?;
            if intent.plan.kind == TxKind::Rebalance { super::rebalance::settle(&tx, bot.id, &intent.plan, super::rebalance::Settlement::NotPlaced, clock.now())?; }
        }
        Sent::Ambiguous(_) | Sent::Fenced => {}
    }
    tx.commit()?;
    Ok(sent)
}

fn intent_owned(c: &Connection, intent: &Intent) -> Result<bool, EngineError> {
    if intent.plan.kind == TxKind::Rebalance { return rebalance_claim_holds(c, intent.bot_id); }
    if intent.signal() { return signal_claim_holds(c, intent.bot_id); }
    match &intent.run { Some(run) => model::owns_identity(c, intent.bot_id, run), None => Ok(false) }
}

/// A signal bot's intent (`begin_signal`) is owned while the bot is not deleted or archived and no reversal bars it, as a
/// REBALANCE one's without the composition check (its pair cannot change while it is working; `stranded` refuses it anyway).
/// A stop after the claim leaves the order, as Rails' job would.
fn signal_claim_holds(c: &Connection, bot_id: i64) -> Result<bool, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    Ok(!matches!(bot.status, crate::enums::BotStatus::Deleted | crate::enums::BotStatus::Archived) && !super::reversal::barred(&bot))
}

/// A REBALANCE intent's claim (`begin_rebalance`): the bot is not deleted or archived, no reversal bars it, and its
/// composition is still the snapshot the intent recorded.
fn rebalance_claim_holds(c: &Connection, bot_id: i64) -> Result<bool, EngineError> {
    let bot = model::load_bot(c, bot_id)?;
    if matches!(bot.status, crate::enums::BotStatus::Deleted | crate::enums::BotStatus::Archived) || super::reversal::barred(&bot) { return Ok(false); }
    Ok(bot.rust_placement().and_then(|v| v.get("composition").cloned()) == Some(composition_snapshot(c, &bot)?))
}

async fn send_to_venue<V: Venue>(venue: &V, intent: &Intent, clock: &dyn Clock) -> Sent {
    let handle=crate::venue::Handle::new(venue,intent.credential_version.clone());
    let venue=&handle;
    let Some(send_until) = intent.at.checked_add_signed(Duration::seconds(SEND_WINDOW_SECONDS)) else { return Sent::NotSent(format!("{:?}", super::schedule::time_range_error())); };
    if clock.now() > send_until {
        return Sent::NotSent(format!("the order intent from {} is older than {SEND_WINDOW_SECONDS} s; not sent", intent.at.to_rfc3339()));
    }
    let rules = venue.rules();
    match intent.plan.limit_refusal() {
        Ok(Some(refusal)) => return Sent::Rejected(vec![refusal]),
        Ok(None) => {}
        Err(e) => return Sent::NotSent(format!("{e}; not sent")),
    }
    let order = match intent.plan.to_order(intent.cl_ord_id.clone(), intent.deadline, rules.wire) {
        Ok(o) => o,
        Err(e) => return Sent::NotSent(format!("{e}; not sent")),
    };
    match venue.placement_result(&order).await.value {
        Ok(txid) => Sent::Accepted(txid),
        Err(VenueError::Rejected(e) | VenueError::Unauthorized(e)) if rules.add_outcome_unknown(&e) => Sent::Ambiguous(crate::ruby::to_sentence(&e)),
        Err(VenueError::Rejected(e) | VenueError::Unauthorized(e)) => Sent::Rejected(e),
        Err(VenueError::Ambiguous(m)) => Sent::Ambiguous(m),
        Err(VenueError::Transient(m)) => Sent::NotSent(m),
    }
}

pub fn record_accepted(c: &Connection, bot: &Bot, intent: &Intent, txid: &str) -> Result<i64, EngineError> {
    let tx = model::immediate(c)?;
    let id = record_accepted_in(&tx, bot, intent, txid, intent.at)?;
    tx.commit()?;
    Ok(id)
}
fn record_accepted_in(tx: &rusqlite::Transaction<'_>, bot: &Bot, intent: &Intent, txid: &str, now: DateTime<Utc>) -> Result<i64, EngineError> {
    if !still_pending(tx, bot.id, &intent.cl_ord_id)? { return Err(gone(bot.id)); }
    let id = write_accepted_order(tx, bot, &intent.plan, txid, intent.at, intent.run.as_ref())?;
    set_intent(tx, bot.id, None)?;
    if intent.plan.kind == TxKind::Rebalance { super::rebalance::settle(tx, bot.id, &intent.plan, super::rebalance::Settlement::Accepted(id), now)?; }
    Ok(id)
}

pub fn safe_rejection(errors: &[String]) -> bool {
    errors.iter().any(|m| PLACEMENT_SAFE_TRANSIENT_ERRORS.iter().any(|p| m.contains(p)))
}
pub fn record_rejected(c: &Connection, bot: &Bot, intent: &Intent, errors: &[String]) -> Result<bool, EngineError> {
    let tx = model::immediate(c)?;
    let recorded = record_rejected_in(&tx, bot, intent, errors, intent.at)?;
    tx.commit()?;
    Ok(recorded)
}
fn record_rejected_in(tx: &rusqlite::Transaction<'_>, bot: &Bot, intent: &Intent, errors: &[String], now: DateTime<Utc>) -> Result<bool, EngineError> {
    let fenced = model::check_credential_result(tx, &intent.credential_version)?;
    if !still_pending(tx, bot.id, &intent.cl_ord_id)? { return Err(gone(bot.id)); }
    let safe = safe_rejection(errors);
    // Bot::Rebalancer#handle_placement_failure: only placement_transient_error? proves a pre-trade rejection, which writes the
    // failed REBALANCE row and undoes the attempt; any other failure halts the swap as ambiguous, with no row.
    if intent.plan.kind == TxKind::Rebalance {
        if safe { write_order_row(&fenced, bot, &intent.plan, RowKind::Failed { errors: errors.to_vec() }, intent.at)?; }
        set_intent(tx, bot.id, None)?;
        let settlement = if safe { super::rebalance::Settlement::NotPlaced }
            else { super::rebalance::Settlement::Halt(format!("{} placement failed: {}", intent.plan.side.as_str(), crate::ruby::to_sentence(errors))) };
        super::rebalance::settle(tx, bot.id, &intent.plan, settlement, now)?;
        return Ok(safe);
    }
    if !safe {
        let id = write_order_row(&fenced, bot, &intent.plan, RowKind::Failed { errors: errors.to_vec() }, intent.at)?;
        tx.execute("UPDATE transactions SET run_identity=?2 WHERE id=?1", params![id, intent.run.as_ref().map(|r| r.order_json().to_string())])?;
    }
    set_intent(tx, bot.id, None)?;
    Ok(!safe)
}

/// Nothing reached the venue (VenueError::Transient, or the send window refused the send).
pub fn drop_intent(c: &Connection, bot_id: i64) -> Result<(), EngineError> { set_intent(c, bot_id, None) }

#[derive(Debug)]
pub enum Recovery { NoIntent, Recorded(i64), NotPlaced, Pending }

/// Legacy intents cannot identify their producing run. Settling one must conservatively
/// retain Rails' ambiguous-placement checkpoint wait; this does not grant dispatch ownership.
pub fn pending_requires_checkpoint(bot: &Bot) -> Result<bool, EngineError> {
    let Some(raw) = bot.rust_placement() else { return Ok(false) };
    // A REBALANCE order belongs to no DCA run (begin_rebalance).
    if raw["transaction_type"].as_str() == Some(TxKind::Rebalance.as_str()) { return Ok(false); }
    let Some(run) = raw.get("run_identity").filter(|v| !v.is_null()) else { return Ok(true) };
    let run = model::RunIdentity::from_json(run).ok_or_else(||EngineError::Data("unreadable intent run identity".into()))?;
    Ok(run == bot.run_identity()?)
}

/// Why this venue must not be asked about an intent: it would ask another host (an Alpaca key's mode changed) or another
/// account (the key id changed). Either would truthfully answer not-found, and past the absence margin that would clear an
/// order that exists. An intent from before the mode was recorded was sent paper (that build sent every Alpaca order to
/// the paper host); one without a fingerprint or a host is compared on what it has. A venue that states no identity (no
/// key: Rails' unsaved fallback) is not compared.
fn identity_mismatch(intent: &Value, now: Option<Value>) -> Option<&'static str> {
    let now = now?;
    let mode = |v: &Value| v["alpaca_live"].as_bool().unwrap_or(false);
    if mode(intent) != mode(&now) { return Some(if mode(intent) { "in Alpaca live mode and the key is now paper" } else { "in Alpaca paper mode and the key is now live" }); }
    if let (Some(sent), Some(host)) = (intent["host"].as_str(), now["host"].as_str()) {
        if sent != host { return Some("to another host than the venue is now reached at"); }
    }
    match intent["key_fingerprint"].as_str() {
        Some(sent) if Some(sent) != now["key_fingerprint"].as_str() => Some("under another API key id than the key now holds"),
        _ => None,
    }
}

/// One line per unresolved order per process: recovery runs every tick.
fn log_once(cl_ord_id: &str, line: &str) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    if SEEN.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(cl_ord_id.to_string()) { super::log(line); }
}

pub async fn recover<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock) -> Result<Recovery, EngineError> {
    recover_since(c, venue, bot, clock, DateTime::<Utc>::MIN_UTC).await
}

/// `recover` by a process that started at `process_start`.
pub async fn recover_since<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, process_start: DateTime<Utc>) -> Result<Recovery, EngineError> {
    let Some(raw) = bot.rust_placement() else { return Ok(Recovery::NoIntent) };
    let handle=crate::venue::Handle::for_bot(venue,c,bot)?;
    let venue=&handle;
    let intent = Intent::from_json(c, bot, &raw,venue.producer())?;
    if let Some(why) = identity_mismatch(&raw, venue.placement_identity()) {
        log_once(&intent.cl_ord_id, &format!("[engine] bot {}: its unresolved order was sent {why}; it is not looked up anywhere else; settle it with `deltabadger resolve-placement`", bot.id));
        return Ok(Recovery::Pending);
    }
    let rules = venue.rules();
    // Kraken drops an order after the deadline it was sent with, so absence is provable from deadline + 60 s whatever the
    // process start. A venue without one (VenueRules::absence_margin_secs): the lookup must start a full margin after the
    // intent AND after this process started, because a process suspended (or killed and restarted) after its send cannot
    // vouch for when that send left.
    let absent_from = if rules.deadline_sent { intent.deadline.checked_add_signed(Duration::seconds(ABSENCE_AFTER_SECONDS)).ok_or_else(super::schedule::time_range_error)? }
                      else { intent.at.max(process_start).checked_add_signed(Duration::try_seconds(rules.absence_margin_secs).ok_or_else(super::schedule::time_range_error)?).ok_or_else(super::schedule::time_range_error)? };
    let started = clock.now(); // only a scan that starts after the cutoff can prove absence
    match venue.recovery_result(&intent.cl_ord_id, &intent.symbol, intent.at.checked_sub_signed(Duration::hours(1)).ok_or_else(super::schedule::time_range_error)?).await.value {
        // An answer on the other side is not this intent's order: incomplete, so the intent stays.
        Ok(Some(state)) if state.sell != (intent.plan.side == Side::Sell) => {
            log_once(&intent.cl_ord_id, &format!("[engine] bot {}: the venue answered for its unresolved {} order with a {} order; it stays unresolved", bot.id, intent.plan.side.as_str(), if state.sell { "sell" } else { "buy" }));
            Ok(Recovery::Pending)
        }
        Ok(Some(state)) => {
            let now = clock.now();
            let tx = model::immediate(c)?;
            let fenced=model::check_credential_result(&tx, &intent.credential_version)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            let id = write_accepted_order(&tx, bot, &intent.plan, &state.txid, intent.at, intent.run.as_ref())?;
            // The intent goes before the fill is applied: the amount cap counts an unresolved intent as spent, and this one is
            // now its row, which polling's stop trigger must not count twice.
            set_intent(&tx, bot.id, None)?;
            let rebalance = intent.plan.kind == TxKind::Rebalance;
            if rebalance { super::rebalance::settle(&tx, bot.id, &intent.plan, super::rebalance::Settlement::Accepted(id), now)?; }
            // As FetchAndUpdateOrderJob would, after placement; its amount-limit stop lands with the fill (the tick ends here).
            if polling::apply_in(&fenced, bot.id, id, &state, true, now)? {
                polling::owe_limit_mail(&tx, bot.id, now)?;
                super::tick::stop_for_amount_limit(&tx, bot.id, now)?;
            }
            // A REBALANCE order belongs to no DCA run, and a signal bot has no schedule: neither is deferred.
            if intent.signal() { super::signal::clear_marker(&tx, bot.id, &intent.cl_ord_id)?; }
            else if !rebalance && (intent.run.is_none() || intent_owned(&tx, &intent)?) { defer_failure(&fenced, &model::load_bot(&tx, bot.id)?, now)?; }
            tx.commit()?;
            Ok(Recovery::Recorded(id))
        }
        Ok(None) if rules.absence_provable && started >= absent_from => {
            let tx = model::immediate(c)?;
            let fenced=model::check_credential_result(&tx, &intent.credential_version)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            set_intent(&tx, bot.id, None)?;
            if intent.plan.kind == TxKind::Rebalance { super::rebalance::settle(&tx, bot.id, &intent.plan, super::rebalance::Settlement::NotPlaced, started)?; }
            else if intent.signal() { super::signal::clear_marker(&tx, bot.id, &intent.cl_ord_id)?; }
            else if intent.run.is_none() || intent_owned(&tx, &intent)? { defer_failure(&fenced, &model::load_bot(&tx, bot.id)?, started)?; }
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
    let intent = Intent::from_json(&tx, &bot, &raw,None)?;
    if let OperatorResolution::Placed(t) = &resolution {
        let dup: i64 = tx.query_row("SELECT count(*) FROM transactions WHERE exchange_id = ?1 AND external_id = ?2", params![bot.exchange_id, t], |r| r.get(0))?;
        if dup > 0 { return Err(EngineError::Data(format!("order {t} is already recorded"))); }
    }
    let (label, txid, settlement) = match resolution {
        OperatorResolution::Placed(txid) => { let id = write_accepted_order(&tx, &bot, &intent.plan, &txid, intent.at, intent.run.as_ref())?; ("placed", Some(txid), super::rebalance::Settlement::Accepted(id)) }
        OperatorResolution::NotPlaced => ("not_placed", None, super::rebalance::Settlement::NotPlaced),
    };
    set_intent(&tx, bot_id, None)?;
    if intent.plan.kind == TxKind::Rebalance { super::rebalance::settle(&tx, bot_id, &intent.plan, settlement, now)?; }
    // A signal bot has no schedule to defer; its marker is the user's to resolve (the bot page).
    else if !intent.signal() && (intent.run.is_none() || intent_owned(&tx, &intent)?) { defer_to_next_checkpoint(&tx, &bot, now)?; }
    model::log_activity(&tx, bot_id, "placement_ambiguous", Level::Warning,
        json!({ "error": "resolved by the operator", "resolution": label, "source": "operator", "cl_ord_id": intent.cl_ord_id, "order_id": txid }), now)?;
    tx.commit()?;
    Ok(())
}

/// The rows and settings an order was sized from. Read again under the intent's write lock.
pub fn composition_snapshot(c: &Connection, bot: &Bot) -> Result<Value, EngineError> {
    let members: String = c.query_row("SELECT json_group_array(json_array(asset_id,ticker_id,target_allocation,in_index)) FROM (SELECT asset_id,ticker_id,target_allocation,in_index FROM bot_index_assets WHERE bot_id=?1 ORDER BY id)", [bot.id], |r| r.get(0))?;
    let members: Value = serde_json::from_str(&members).map_err(|_| EngineError::Data("unreadable composition".into()))?;
    Ok(json!({"type":bot.bot_type,"exchange":bot.exchange_id,"settings":bot.settings,"members":members}))
}
