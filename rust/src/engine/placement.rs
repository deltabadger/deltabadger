//! Placement protocol. The intent is committed (with a deadline read from the clock at that
//! moment) before AddOrder, which is sent at most once. A lost reply is resolved by cl_ord_id, and a
//! found order is recorded, filled and cleared in one transaction. "Not placed" is concluded only from a
//! complete lookup that STARTED after the deadline + 60 s (Kraken), or 20 minutes after both the intent and the
//! process start on a venue without a server-side deadline (Alpaca, VenueRules::absence_margin_secs).
use crate::venue::Attributed;
use super::amount::{write_accepted_order, write_order_row, OrderPlan, Origin, RowKind, TxKind};
use super::model::{self, Bot, Level};
use super::schedule::checkpoints;
use super::rebalance::Settlement;
use super::wash::Reason;
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
/// (400002), Exchanges::Bitvavo's (errorCode 304, outside the access window), Exchanges::Gemini::ERRORS[:placement_safe_transient]
/// (InvalidNonce), Exchanges::Bitget's (40008, "Request timestamp expired") and Exchanges::Bitrue's timestamp errors.
/// Rails scopes each venue's string to its venue;
/// they are listed for every venue here, strings only that venue sends (Kraken's nonce error is "EAPI:Invalid nonce").
pub const PLACEMENT_SAFE_TRANSIENT_ERRORS: [&str; 8] = ["Timestamp for this request is outside of the recvWindow", "Timestamp for this request was", "Invalid KC-API-TIMESTAMP",
    "\"errorCode\":304", "InvalidNonce", "Request timestamp expired", "recvWindow IllegalArgumentException", "timestamp IllegalArgumentException"];

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
    /// A composition sale at a loss: the wash-sale claim taken with this intent (`rust_placement.wash_claim`). A rebalance
    /// sell's lives in the bot's own keys (wash::claim_rebalance_sell).
    pub wash_claim: Option<super::wash::Held>,
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
                   "source": self.source, "origin": p.origin.as_str(), "wire_price": p.wire_price.as_ref().map(|w| d("wire_price", w)).transpose()? }))
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
        // D-679: an intent written before origins existed was sized as Rails sizes it, and is read as a base quantity: its
        // volume is never re-derived. One with no wire price was never crossed (every venue before Gemini).
        let origin = match v.get("origin") { None | Some(Value::Null) => Origin::Base, Some(x) => x.as_str().and_then(Origin::parse).ok_or_else(bad)? };
        let wire_price = match v.get("wire_price") { None | Some(Value::Null) => None, Some(_) => Some(d("wire_price")?) };
        let wash_claim = super::wash::Held::from_json(v.get("wash_claim"))?;
        Ok(Self { wash_claim, source, symbol, bot_id: bot.id, run: v.get("run_identity").filter(|v| !v.is_null()).map(|v| model::RunIdentity::from_json(v).ok_or_else(bad)).transpose()?, credential_version: producer, cl_ord_id: v["cl_ord_id"].as_str().ok_or_else(bad)?.to_string(), deadline: t("deadline")?, at: t("at")?,
                  plan: OrderPlan { ticker, side, limit: b("limit")?, price: d("price")?, amount: d("amount")?, quote_amount: d("quote_amount")?,
                                    quote_type: b("quote_type")?, volume: d("volume")?, kind, origin, wire_price } })
    }
}

/// Context committed with a composition operation's intent.
#[derive(Clone, Copy)]
pub enum Op<'a> { Rebalance(&'a super::rebalance::Pending), Liquidation { symbol: &'a str }, Redeploy }

/// Tests only: rehydrate a committed intent to exercise the commit/send boundary. Production recovery never resubmits it.
pub fn load_intent(c: &Connection, bot: &Bot, producer: Option<model::CredentialVersion>) -> Result<Option<Intent>, EngineError> {
    bot.rust_placement().map(|v| Intent::from_json(c, bot, &v, producer)).transpose()
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
    begin_unfenced(c, bot, plan, None, false, clock)
}

/// Tests only: an operation context and a sale's loss verdict, driven without the sizing fence.
pub fn begin_unfenced(c: &Connection, bot: &Bot, plan: &OrderPlan, op: Option<Op<'_>>, loss: bool, clock: &dyn Clock) -> Result<Intent, EngineError> {
    match begin_checked(c, bot, plan, clock, None, None, None, op, loss, None)? {
        Begun::Intent(intent) => Ok(*intent),
        Begun::BelowMinimum(_) => Err(EngineError::Data("order below minimum after exact cap guard".into())),
        Begun::Refused(why) => Err(EngineError::Data(why)),
        Begun::Changed | Begun::Fenced | Begun::CredentialsChanged | Begun::LedgerStale(_) | Begun::WashSaleLocked => Err(EngineError::Data("unfenced placement skipped".into())),
    }
}

/// Tests only: `begin` with the sending venue's `identity` and `hold` (an IBKR order's, `Venue::placement_hold`), unfenced.
pub fn begin_held(c: &Connection, bot: &Bot, plan: &OrderPlan, identity: Option<Value>, hold: Option<&crate::venue::PlacementHold>, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c, bot, plan, clock, None, None, identity, None, false, hold)
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
    begin_checked(c, sized_from, plan, clock, Some((tickers, composition, reconciled)), None, None, None, false, None)
}

/// BelowMinimum: the exact-cap guard (amount::guard_exact_cap) reduced the order below the venue minimum; the
/// leg is reported as skipped, never placed. Fenced: a stop, or a stop and a restart, took the run from the tick that
/// sized the order (Automation::Statusable::FENCED); that tick writes nothing more to the bot.
/// LedgerStale: the account ledger stopped being fresh after the order was sized (a sync started or failed meanwhile);
/// nothing was written or sent, and the tick returns its checkpoint (tick.rs) instead of ending as a success.
/// WashSaleLocked: a buy of an asset the account locked after it was sized (order_setter.rb:56, rebalancer.rb:212); nothing
/// was written.
/// Refused: the venue's own check in the intent's transaction refused the order (an IBKR order: its account, its contract
/// or its sell cap, `ibkr_reservations::reserve`); nothing was written, the reason is the user's.
pub enum Begun { Intent(Box<Intent>), Changed, Fenced, CredentialsChanged, BelowMinimum(Box<OrderPlan>), LedgerStale(Box<super::staleness::Stale>), WashSaleLocked, Refused(String) }
/// Production placement always passes the version captured with the venue credentials (L).
#[allow(clippy::too_many_arguments)]
/// `identity` is the sending venue's `placement_identity()`: recorded in the intent before the send, so recovery never asks
/// another account or another Alpaca host about this order (`identity_mismatch`).
/// `loss`: a sell the caller judged a sale at a loss (wash::sell_at_loss over the sized order): its wash-sale claim is taken
/// in the intent's own transaction, after every fence (D-WASH-CLAIM-WITH-INTENT).
/// `hold`: what the venue read for this order under its lock (`Venue::placement_hold`), checked in the intent's transaction.
pub fn begin_with_credentials(c: &Connection, bot: &Bot, plan: &OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &super::splits::Snapshot, version: &Option<model::CredentialVersion>, identity: Option<Value>, loss: bool, hold: Option<&crate::venue::PlacementHold>, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c,bot,plan,clock,Some((tickers,composition,reconciled)),Some(version),identity,None,loss,hold)
}

/// A REBALANCE order's intent (Bot::Rebalancer#place_rebalance_order). It belongs to no DCA run: a stopped bot rebalances.
/// Its claim is what it was sized from: under the intent's write lock the bot must not be deleted or archived, and its
/// composition, member rows, split rows, tickers, eligibility and credentials must be what the sizing read, as for a DCA
/// order (a stop, a reversal or a settings edit changes the snapshot); the send checks the same claim again
/// (`rebalance_claim_holds`). `state` is the pending phase written in the intent's own transaction (set_rebalance_pending!
/// for a sell, mark_buy_attempted! for a buy), so a crash after the send always leaves both the intent and the phase.
#[allow(clippy::too_many_arguments)]
/// A sell's `loss` claims the lock and rewrites Rails' three keys in the same transaction (rebalancer.rb:59-69).
pub fn begin_rebalance(c: &Connection, bot: &Bot, plan: &OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &super::splits::Snapshot, version: &Option<model::CredentialVersion>, identity: Option<Value>, state: &super::rebalance::Pending, loss: bool, hold: Option<&crate::venue::PlacementHold>, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c,bot,plan,clock,Some((tickers,composition,reconciled)),Some(version),identity,Some(Op::Rebalance(state)),loss,hold)
}
/// A composition operation's intent, with the same snapshot, credential and reversal fences as rebalance.
#[allow(clippy::too_many_arguments)]
pub fn begin_operation(c: &Connection, bot: &Bot, plan: &OrderPlan, tickers: &[&model::Ticker], composition: &Value, reconciled: &super::splits::Snapshot, version: &Option<model::CredentialVersion>, identity: Option<Value>, op: Op<'_>, loss: bool, hold: Option<&crate::venue::PlacementHold>, clock: &dyn Clock) -> Result<Begun, EngineError> {
    begin_checked(c, bot, plan, clock, Some((tickers, composition, reconciled)), Some(version), identity, Some(op), loss, hold)
}
#[allow(clippy::too_many_arguments)]
fn begin_checked(c: &Connection, bot: &Bot, plan: &OrderPlan, clock: &dyn Clock, fence: Option<(&[&model::Ticker], &Value, &super::splits::Snapshot)>, version: Option<&Option<model::CredentialVersion>>, identity: Option<Value>, op: Option<Op<'_>>, loss: bool, hold: Option<&crate::venue::PlacementHold>) -> Result<Begun, EngineError> {
    let context_matches = matches!((plan.kind, op),
        (TxKind::Regular, None) | (TxKind::Rebalance, Some(Op::Rebalance(_))) |
        (TxKind::Liquidation, Some(Op::Liquidation { .. })) | (TxKind::Redeploy, Some(Op::Redeploy)));
    if !context_matches { return Err(EngineError::Data("order kind does not match its operation context".into())); }
    let tx = model::immediate(c)?; // check-and-set under one write lock
    let current = model::load_bot(&tx, bot.id)?;
    if plan.kind == TxKind::Rebalance && (current.transient["rebalance_pending"]["phase"] == "ambiguous"
        || current.transient["rust_rebalance_resolution"] != bot.transient["rust_rebalance_resolution"]) { return Ok(Begun::Changed); }
    if current.rust_placement().is_some() {
        return Err(EngineError::Data(format!("bot {} already has an unresolved order", bot.id)));
    }
    // The wash-sale re-read: a buy sized before another venue's sale locked its asset is not placed.
    if plan.side == Side::Buy && super::wash::locked(&tx, current.user_id, clock)?.contains(&plan.ticker.base_asset_id) { return Ok(Begun::WashSaleLocked); }
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
        let same_run = match plan.kind {
            TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => !matches!(current.status, crate::enums::BotStatus::Deleted | crate::enums::BotStatus::Archived),
            TxKind::Regular => current.status == crate::enums::BotStatus::Executing && current.run_identity()? == bot.run_identity()?,
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
            else if !(match plan.kind { TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => super::eligibility::rebalance_reasons(&tx, &current)?, TxKind::Regular => super::eligibility::bot_reasons(&tx, &current)? }).is_empty() {
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
    let mut intent = Intent { wash_claim: None, source: None, symbol: plan.ticker.ticker.clone(), bot_id: bot.id, run: match plan.kind { TxKind::Regular => Some(bot.run_identity()?), TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => None }, credential_version: match version {Some(v)=>v.clone(),None=>model::credential_version(&tx,bot)?}, cl_ord_id: uuid::Uuid::new_v4().to_string(), deadline: now.checked_add_signed(Duration::seconds(DEADLINE_SECONDS)).ok_or_else(super::schedule::time_range_error)?, at: now, plan: plan.clone() };
    // Before anything is committed or sent: an intent recovery or `resolve-placement` could not read back would strand the
    // bot after a real order. Such a plan is refused as Rails fails a StandardError raised inside execute_action
    // (execution_failed, no retry; the next checkpoint sizes afresh). Dropping `tx` rolls back. The read-back resolves the
    // intent's own ticker by its id, as recovery will.
    let mut v = intent.to_json()?;
    Intent::from_json(&tx, bot, &v,intent.credential_version.clone())?;
    // The venue's own check, in this transaction (IBKR's account, contract and sell reservation; nothing elsewhere).
    if let Some(why) = crate::venue::ibkr_reservations::reserve(&tx, &current, plan, &intent.cl_ord_id, hold, now)? {
        super::log(&format!("[engine] bot {}: {} order not placed: {why}", bot.id, plan.ticker.ticker));
        return Ok(Begun::Refused(why));
    }
    // What the order is sent under: `stranded` refuses any change to it until the order settles.
    v["exchange_id"] = json!(current.exchange_id);
    v["quote_asset_id"] = json!(current.quote_asset_id());
    v["allocations"] = current.settings.get("allocations").cloned().unwrap_or(Value::Null);
    v["composition"] = composition_snapshot(&tx, &current)?;
    if let Some(Value::Object(identity)) = identity { for (k, value) in identity { v[k] = value; } }
    // The sale's wash-sale claim, after every fence (as Rails claims after #stopped_mid_tick?), with the intent.
    let asset = plan.ticker.base_asset_id;
    if plan.side == Side::Sell {
        match plan.kind {
            TxKind::Rebalance => super::wash::claim_rebalance_sell(&tx, &current, asset, loss, now)?,
            TxKind::Regular | TxKind::Liquidation => {
                if loss {
                    if let Some(claim) = super::wash::claim(&tx, current.user_id, asset, now.date_naive(), now)? {
                        let held = super::wash::Held { asset_id: asset, claim };
                        v["wash_claim"] = held.to_json();
                        intent.wash_claim = Some(held);
                    }
                }
            }
            TxKind::Redeploy => {}
        }
    }
    set_intent(&tx, bot.id, Some(&v))?;
    match op {
        Some(Op::Rebalance(state)) => super::rebalance::set_pending(&tx, bot.id, state)?,
        Some(Op::Liquidation { symbol }) => super::composition::start(&tx, bot.id, plan.kind, &intent.cl_ord_id, Some(symbol))?,
        Some(Op::Redeploy) => super::composition::start(&tx, bot.id, plan.kind, &intent.cl_ord_id, None)?,
        None => {}
    }
    tx.commit()?; // durable before the send
    Ok(Begun::Intent(Box::new(intent)))
}

/// `Intent::source` of a signal bot's order.
pub const SIGNAL_SOURCE: &str = "signal";

/// What `begin_signal` found under the intent's write lock. Only `Intent` wrote one; `Stopped` and `Disabled` consumed the
/// call with its `signal_ignored` line, `Locked` (a buy of an asset the account locked since the call was sized) with
/// none; the others changed nothing, and the call stays for the next pass.
#[derive(Debug)]
/// `BelowMinimum` (D-SIG-CAP): the exact cap guard reduced a buy below the venue's minimum; its skipped row, its line and the
/// consumed call were written in the claim's transaction.
pub enum SignalClaim { Intent(Box<Intent>), Blocked, Stopped, Disabled, Gone, Changed, CredentialsChanged, Locked, BelowMinimum }

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
    // An IBKR order needs its venue's hold (begin_checked); a signal bot on IBKR is refused (eligibility::signal_admission).
    if model::exchange_type(&tx, &current)? == crate::venue::ibkr::IBKR {
        super::log(&format!("[engine] bot {}: {} order not placed: IBKR takes no signal orders", sized_from.id, plan.ticker.ticker));
        return Ok(SignalClaim::Changed);
    }
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
        // #checked_amount_info's re-read (the one more attempt after a safe rejection places at once, without it).
        if super::signal::locked_buy(&tx, &current, &plan.ticker, plan.side, now)? {
            super::signal::consume(&tx, current.id, call, None, now)?;
            tx.commit()?;
            return Ok(SignalClaim::Locked);
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
    // D-SIG-CAP: the exact cap bounds a buy under the intent lock, on the call and on the one more attempt alike. A reduction
    // under the venue's minimum is the skipped row Rails writes for an order under it, with the call consumed.
    let guarded;
    let plan = if plan.side == Side::Buy {
        match super::amount::guard_exact_cap(&tx, &current, plan)? {
            super::amount::CapGuard::Place(safe) => { guarded = safe; &guarded }
            super::amount::CapGuard::BelowMinimum(skipped) => {
                super::log(&format!("execute_signal bot={} event=order_skipped base={} quote={}", current.id, skipped.ticker.base_symbol, skipped.ticker.quote_symbol));
                let fenced = model::check_credential_result(&tx, version)?;
                model::log_activity(&tx, current.id, "order_skipped", Level::Warning, skipped.log_details(), now)?;
                write_order_row(&fenced, &current, &skipped, RowKind::Skipped, now)?;
                drop(fenced);
                if let Some(call) = call { super::signal::consume(&tx, current.id, call, None, now)?; }
                tx.commit()?;
                return Ok(SignalClaim::BelowMinimum);
            }
        }
    } else { plan };
    let intent = Intent { wash_claim: None, source: Some(SIGNAL_SOURCE.into()), symbol: plan.ticker.ticker.clone(), bot_id: current.id, run: None, credential_version: version.clone(),
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

/// BelowMinimum (D-BELOW-MIN-REJECTION): the venue definitively refused a scheduled DCA buy leg as under its minimum
/// order size (`below_minimum`). `send_recorded` writes no failed row and clears the intent; the tick reports the leg as
/// the pre-check reports one it skips (tick::record_skipped). Rails records a failed order.
#[derive(Debug)]
pub enum Sent { Accepted(String), Rejected(Vec<String>), Ambiguous(String), NotSent(String), Fenced, BelowMinimum(Vec<String>) }

/// D-BELOW-MIN-REJECTION's table: (exchanges.type, structured code, message prefix). An entry with a code matches a
/// `VenueError::Coded` refusal with that code whose text starts with the prefix ("" for any); one without matches the text's
/// prefix alone. Only definitive refusals are asked (after `add_outcome_unknown` and the adapters' own ambiguity), and every
/// message of a refusal must match: a mixed refusal stays a failure. Upper limits and precision failures never match.
/// Venues not listed (KuCoin, Bitget, BingX, Bitrue, Coinbase, Gemini) have no cited too-small answer yet: theirs stay
/// failed rows.
const BELOW_MINIMUM: &[(&str, Option<&str>, &str)] = &[
    // Binance spot API docs, Errors → Filter failures (-1013): "MIN_NOTIONAL: price * quantity is too low"; Rails' own
    // fixture of the answer (test/models/exchanges/binance_test.rb, action_job_test.rb). Not "Filter failure: NOTIONAL",
    // whose range also has a maximum, nor LOT_SIZE (too high or too low, or off the step).
    ("Exchanges::Binance", Some("-1013"), "Filter failure: MIN_NOTIONAL"),
    ("Exchanges::BinanceUs", Some("-1013"), "Filter failure: MIN_NOTIONAL"),
    // MEXC spot v3 docs, Error Code: 30002 "The minimum transaction volume cannot be less than :" (30003 is the maximum).
    ("Exchanges::Mexc", Some("30002"), ""),
    // Bybit v5 docs, Error codes (spot): 170140 "Order value exceeded lower limit", 170136 "Order quantity lower than the
    // minimum." (170137/170148 are precision).
    ("Exchanges::Bybit", Some("170140"), ""),
    ("Exchanges::Bybit", Some("170136"), ""),
    // Bitvavo docs, Errors: 217 "`amountQuote` is below the minimum for this market", 212 "`amount` is below the minimum for
    // this market" (210 is the maximum).
    ("Exchanges::Bitvavo", Some("217"), ""),
    ("Exchanges::Bitvavo", Some("212"), ""),
    // Kraken support, "API error messages": "EOrder:Order minimum not met (volume too low)".
    ("Exchanges::Kraken", None, "EOrder:Order minimum not met"),
    // Hyperliquid docs, Error responses: MinTradeNtl "Order must have minimum value of $10.", MinTradeSpotNtl "… of 10
    // {quote_token}." (Rails test/models/bot/order_setter_test.rb records the latter as a hard rejection); as the status
    // error hyperliquid::add_order words it.
    ("Exchanges::Hyperliquid", None, "Hyperliquid order failed: Order must have minimum value of "),
    // Alpaca's 403 to a crypto buy under its $10 floor: "cost basis must be >= minimal amount of order 10". Its numeric code
    // (40310000) is shared with other refusals (insufficient buying power), so the text decides.
    ("Exchanges::Alpaca", None, "cost basis must be >= minimal amount of order"),
];

/// Is this definitive refusal the venue's "order too small" answer (BELOW_MINIMUM)?
pub fn below_minimum(exchange_type: &str, code: Option<&str>, errors: &[String]) -> bool {
    !errors.is_empty() && errors.iter().all(|m| BELOW_MINIMUM.iter().any(|(venue, want, prefix)| *venue == exchange_type
        && want.is_none_or(|w| code == Some(w)) && m.starts_with(prefix)))
}

/// OM3b's scope: a scheduled DCA, basket or index BUY leg. Out of scope, as before: sells (selling freezes the buy carry),
/// signal orders (their own consumed-call lifecycle, engine::signal) and REBALANCE legs (a below-minimum rebalance buy
/// abandons its dust, rebalance.rs).
fn skips_below_minimum(intent: &Intent) -> bool {
    match intent.plan.kind {
        TxKind::Regular => intent.plan.side == Side::Buy && intent.run.is_some() && !intent.signal(),
        TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => false,
    }
}

/// Acquire the dispatch write lock before rechecking the captured generation. The crypto adapters and Alpaca have no
/// blocking preflight or session lock between this check and HTTP send. IBKR's has: its add_order reads the contract rules
/// and waits for its login's lock (up to ibkr::LOCK_WAIT) before the POST, and a stop landing in that wait does not
/// withdraw the order, as a stop landing during Clients::Ibkr's IbkrLock wait does not in Rails. STOP can
/// invalidate this run while the response is in flight; the accepted order still gets recorded.
pub async fn send<V: Venue>(c: &Connection, venue: &V, intent: &Intent, clock: &dyn Clock) -> Result<Sent, EngineError> {
    // Preparation may perform HTTP. Refresh before the existing dispatch fence, so a rotation during
    // detection cannot authorize a send. The committed family, not the latest account mode, is authoritative.
    let bot = model::load_bot(c, intent.bot_id)?;
    if let Err(error) = venue.validate_preparation(bot.rust_placement().as_ref()).await {
        let dispatched = model::credential_write(c, &intent.credential_version, |tx| {
            let raw = model::load_bot(tx, intent.bot_id)?.rust_placement();
            Ok(raw.is_some_and(|v| v["cl_ord_id"] == intent.cl_ord_id && v["dispatched"] == true))
        })?;
        // A failed read proves nothing about an earlier dispatch. Only this unsent invocation
        // is refused; an existing ambiguous placement remains an obligation.
        if dispatched { return Ok(Sent::Ambiguous(format!("{}; previous dispatch remains unresolved", error.message()))); }
        return Ok(Sent::NotSent(error.message()));
    }
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
            if removed == 1 { settle(&tx, intent.bot_id, intent, Settlement::NotPlaced(Reason::Fenced), clock.now())?; }
            tx.commit()?;
            return Ok(Sent::Fenced);
        }
        model::check_credential_result(&tx, &intent.credential_version)?;
        tx.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement.dispatched',json('true')) WHERE id=?1", [intent.bot_id])?;
        tx.commit()?;
    }
    Ok(send_to_venue(venue, intent, clock).await)
}

/// Preparation carries its original producer across HTTP, just like balances and prices. A no-op
/// preparation does not introduce a credential requirement for other venues or the classic replay.
pub(crate) async fn prepare<V: Venue>(c: &Connection, venue: &V) -> Result<Result<(), VenueError>, EngineError> {
    let origin = venue.producer();
    let result = venue.prepare().await;
    let version = match &result { Ok(version) => version.clone(), Err(_) => origin };
    if version.is_some() { model::credential_write(c, &version, |_| Ok(()))?; }
    Ok(result.map(|_| ()))
}

/// An accepted order is an immutable fact, even when STOP/restart changed the run meanwhile.
/// Record and clear its intent in a new short transaction, before any follow-up work.
pub async fn send_recorded<V: Venue>(c: &Connection, venue: &V, bot: &Bot, intent: &Intent, clock: &dyn Clock) -> Result<Sent, EngineError> {
    let sent = send(c, venue, intent, clock).await?;
    let tx = model::immediate(c)?;
    match &sent {
        Sent::Accepted(id) => { record_accepted_in(&tx, bot, intent, id, clock.now())?; }
        Sent::Rejected(errors) => { record_rejected_in(&tx, bot, intent, errors, clock.now())?; }
        Sent::NotSent(message) => {
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Err(gone(bot.id)); }
            set_intent(&tx, bot.id, None)?;
            settle(&tx, bot.id, intent, Settlement::NotPlaced(Reason::PreTransmission), clock.now())?;
            match intent.plan.kind {
                TxKind::Liquidation | TxKind::Redeploy => {
                    let name = super::composition::name(intent.plan.kind).expect("composition kind");
                    model::log_activity(&tx, bot.id, &format!("{name}_skipped"), Level::Info,
                        json!({"base":match intent.plan.kind { TxKind::Liquidation => &intent.plan.ticker.base_code, TxKind::Redeploy => &intent.plan.ticker.base_symbol, TxKind::Regular | TxKind::Rebalance => unreachable!() },"reason":format!("transient: {message}")}), clock.now())?;
                }
                TxKind::Regular | TxKind::Rebalance => {}
            }
        }
        // No failed row: the leg is the tick's to report as skipped. The intent goes in the same checks a rejection's row does.
        Sent::BelowMinimum(_) => {
            model::check_credential_result(&tx, &intent.credential_version)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Err(gone(bot.id)); }
            set_intent(&tx, bot.id, None)?;
        }
        Sent::Ambiguous(message) => match intent.plan.kind {
            TxKind::Liquidation | TxKind::Redeploy => {
                if still_pending(&tx, bot.id, &intent.cl_ord_id)? {
                    settle(&tx, bot.id, intent, Settlement::Halt(format!("placement outcome unknown: {message}")), clock.now())?;
                }
            }
            TxKind::Regular | TxKind::Rebalance => {}
        },
        Sent::Fenced => {}
    }
    tx.commit()?;
    Ok(sent)
}

fn intent_owned(c: &Connection, intent: &Intent) -> Result<bool, EngineError> {
    match intent.plan.kind {
        TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => rebalance_claim_holds(c, intent.bot_id),
        TxKind::Regular => {
            if intent.signal() { return signal_claim_holds(c, intent.bot_id); }
            match &intent.run { Some(run) => model::owns_identity(c, intent.bot_id, run), None => Ok(false) }
        }
    }
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
    // D-679: a crossing venue's market order goes out at its persisted crossed price, or not at all (never uncrossed).
    if rules.cross.is_some() && !intent.plan.limit && intent.plan.wire_price.is_none() {
        return Sent::NotSent("no crossed price prepared; not sent".into());
    }
    let order = match intent.plan.to_order(intent.cl_ord_id.clone(), intent.deadline, rules.wire) {
        Ok(o) => o,
        Err(e) => return Sent::NotSent(format!("{e}; not sent")),
    };
    match venue.placement_result(&order).await.value {
        Ok(txid) => Sent::Accepted(txid),
        Err(VenueError::Rejected(e) | VenueError::Unauthorized(e) | VenueError::Coded { errors: e, .. }) if rules.add_outcome_unknown(&e) => Sent::Ambiguous(crate::ruby::to_sentence(&e)),
        Err(VenueError::Coded { code, errors }) if skips_below_minimum(intent) && below_minimum(rules.exchange_type, Some(&code), &errors) => Sent::BelowMinimum(errors),
        Err(VenueError::Rejected(e)) if skips_below_minimum(intent) && below_minimum(rules.exchange_type, None, &e) => Sent::BelowMinimum(e),
        Err(VenueError::Rejected(e) | VenueError::Unauthorized(e) | VenueError::Coded { errors: e, .. }) => Sent::Rejected(e),
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
    settle(tx, bot.id, intent, Settlement::Accepted(id), now)?;
    Ok(id)
}

/// What the intent settles beside its row, in the transaction that removes it: a REBALANCE intent's pending phase (with its
/// sell's wash-sale claim, rebalance::settle), else a composition sale's claim (wash::settle_claim, the one owner).
fn settle(tx: &Connection, bot_id: i64, intent: &Intent, settlement: Settlement, now: DateTime<Utc>) -> Result<(), EngineError> {
    let reason = match &settlement { Settlement::Accepted(_) => Reason::Accepted, Settlement::NotPlaced(reason) => *reason, Settlement::Halt(_) => Reason::Ambiguous };
    match intent.plan.kind {
        TxKind::Regular => super::wash::settle_claim(tx, bot_id, super::wash::Holder::Composition(intent.wash_claim.as_ref()), reason, now),
        TxKind::Rebalance => super::rebalance::settle(tx, bot_id, &intent.plan, settlement, now),
        TxKind::Liquidation | TxKind::Redeploy => {
            let name = super::composition::name(intent.plan.kind).expect("composition kind");
            match settlement {
                Settlement::Accepted(_) => {
                    model::merge_transient_compact(tx, bot_id, &[(&format!("{name}_pending"), Value::Null)])?;
                    model::log_activity(tx, bot_id, &format!("{name}_placed"), Level::Info, intent.plan.log_details(), now)?;
                }
                Settlement::NotPlaced(Reason::Rejected { safe: true, .. } | Reason::PreTransmission) => {
                    model::merge_transient_compact(tx, bot_id, &[(&format!("{name}_pending"), Value::Null)])?;
                }
                Settlement::Halt(message) => { super::composition::halt(tx, bot_id, intent.plan.kind, &message, intent.plan.log_details(), now)?; }
                Settlement::NotPlaced(_) => { super::composition::halt(tx, bot_id, intent.plan.kind, "placement intent survived its worker", intent.plan.log_details(), now)?; }
            }
            let holder = match intent.plan.kind {
                TxKind::Liquidation => super::wash::Holder::Liquidation(intent.wash_claim.as_ref()),
                TxKind::Redeploy => super::wash::Holder::None,
                TxKind::Regular | TxKind::Rebalance => unreachable!(),
            };
            super::wash::settle_claim(tx, bot_id, holder, reason, now)
        }
    }
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
    match intent.plan.kind {
      TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => {
        if safe { write_order_row(&fenced, bot, &intent.plan, RowKind::Failed { errors: errors.to_vec() }, intent.at)?; }
        match intent.plan.kind {
            TxKind::Rebalance => set_intent(tx, bot.id, None)?,
            TxKind::Liquidation | TxKind::Redeploy => { if safe { set_intent(tx, bot.id, None)?; } }
            TxKind::Regular => unreachable!(),
        }
        let settlement = if safe { Settlement::NotPlaced(Reason::Rejected { safe, unknown: false }) }
            else { Settlement::Halt(match intent.plan.kind {
                TxKind::Rebalance => format!("{} placement failed: {}", intent.plan.side.as_str(), crate::ruby::to_sentence(errors)),
                TxKind::Liquidation | TxKind::Redeploy => format!("placement failed: {}", crate::ruby::to_sentence(errors)),
                TxKind::Regular => unreachable!(),
            }) };
        settle(tx, bot.id, intent, settlement, now)?;
        return Ok(safe);
      }
      TxKind::Regular => {}
    }
    // Rails restores a composition sale's claim unless Exchange#ambiguous_placement_error? (order_setter.rb:83); its
    // HTTP/acknowledgement half is already Sent::Ambiguous here (add_outcome_unknown), its text half is #transient_error?.
    let unknown = !safe && super::venue_rules::for_exchange(&model::exchange_type(tx, bot)?).is_some_and(|rules| rules.is_transient(errors));
    // Before the failed row, as Rails rolls the claim back before create_failed_order!.
    settle(tx, bot.id, intent, Settlement::NotPlaced(Reason::Rejected { safe, unknown }), now)?;
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
    match raw.get("transaction_type").and_then(Value::as_str).and_then(TxKind::parse) {
        Some(TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy) => return Ok(false),
        Some(TxKind::Regular) | None => {}
    }
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
    if let Some(sent) = intent["kucoin_account_mode"].as_str() {
        if Some(sent) != now["kucoin_account_mode"].as_str() { return Some("under another or unprepared KuCoin account mode"); }
    }
    let mode = |v: &Value| v["alpaca_live"].as_bool().unwrap_or(false);
    if mode(intent) != mode(&now) { return Some(if mode(intent) { "in Alpaca live mode and the key is now paper" } else { "in Alpaca paper mode and the key is now live" }); }
    if let (Some(sent), Some(host)) = (intent["host"].as_str(), now["host"].as_str()) {
        if sent != host { return Some("to another host than the venue is now reached at"); }
    }
    // D-IBKR-BOT-ACCOUNT: an IBKR order is looked up only in the account it went to.
    if let Some(sent) = intent["ibkr_account_ref"].as_str() {
        if Some(sent) != now["ibkr_account_ref"].as_str() { return Some("to another IBKR account than the connection now names"); }
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
    if raw.get("kucoin_account_mode").is_some() {
        if let Err(error) = prepare(c, venue).await? {
            log_once(&intent.cl_ord_id, &format!("[engine] bot {}: recovery account preparation failed: {error:?}", bot.id));
            return Ok(Recovery::Pending);
        }
    }
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
            // D-KUCOIN-ZERO-FILL-FUNDS: an old zero-fill answer is Closed; a young one leaves the row waiting for its polls.
            let state = state.settled(now);
            let tx = model::immediate(c)?;
            let fenced=model::check_credential_result(&tx, &intent.credential_version)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            let id = write_accepted_order(&tx, bot, &intent.plan, &state.txid, intent.at, intent.run.as_ref())?;
            // The intent goes before the fill is applied: the amount cap counts an unresolved intent as spent, and this one is
            // now its row, which polling's stop trigger must not count twice.
            set_intent(&tx, bot.id, None)?;
            settle(&tx, bot.id, &intent, Settlement::Accepted(id), now)?;
            // As FetchAndUpdateOrderJob would, after placement; its amount-limit stop lands with the fill (the tick ends here).
            // The base cap's too: a recovered sell fill that uses it up stops the bot (amount_sold) and owes its mail.
            if let Some(cap) = polling::apply_in(&fenced, bot.id, id, &state, true, now)? {
                super::amount_limit::owe_mail(&tx, bot.id, cap, now)?;
                super::tick::stop_for_amount_limit(&tx, bot.id, cap, now)?;
            }
            // A REBALANCE order belongs to no DCA run, and a signal bot has no schedule: neither is deferred.
            defer_recovered(&fenced, &intent, now)?;
            tx.commit()?;
            Ok(Recovery::Recorded(id))
        }
        Ok(None) if rules.absence_provable && started >= absent_from => {
            let tx = model::immediate(c)?;
            let fenced=model::check_credential_result(&tx, &intent.credential_version)?;
            if !still_pending(&tx, bot.id, &intent.cl_ord_id)? { return Ok(Recovery::NoIntent); }
            set_intent(&tx, bot.id, None)?;
            // Absence proven: the claim stays, as a Rails worker crash leaves it.
            settle(&tx, bot.id, &intent, Settlement::NotPlaced(Reason::Absent), started)?;
            defer_recovered(&fenced, &intent, started)?;
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
        OperatorResolution::Placed(txid) => { let id = write_accepted_order(&tx, &bot, &intent.plan, &txid, intent.at, intent.run.as_ref())?; ("placed", Some(txid), Settlement::Accepted(id)) }
        OperatorResolution::NotPlaced => ("not_placed", None, Settlement::NotPlaced(Reason::Absent)),
    };
    set_intent(&tx, bot_id, None)?;
    settle(&tx, bot_id, &intent, settlement, now)?;
    // A signal bot has no schedule to defer; its marker is the user's to resolve (the bot page).
    match intent.plan.kind {
        TxKind::Regular => { if !intent.signal() && (intent.run.is_none() || intent_owned(&tx, &intent)?) { defer_to_next_checkpoint(&tx, &bot, now)?; } }
        TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => {}
    }
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

fn defer_recovered(c: &model::FencedTransaction<'_>, intent: &Intent, now: DateTime<Utc>) -> Result<(), EngineError> {
    match intent.plan.kind {
        TxKind::Regular => {
            if intent.signal() { super::signal::clear_marker(c, intent.bot_id, &intent.cl_ord_id)?; }
            else if intent.run.is_none() || intent_owned(c, intent)? { defer_failure(c, &model::load_bot(c, intent.bot_id)?, now)?; }
        }
        TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => {}
    }
    Ok(())
}

/// A still-unanswered composition order becomes clearable after the venue's absence margin.
/// The comparison and state transition share a write lock; a recovered or cleared intent wins exclusively.
pub fn halt_unknown(c: &Connection, bot_id: i64, kind: TxKind, rules: &super::venue_rules::VenueRules, now: DateTime<Utc>) -> Result<bool, EngineError> {
    match kind {
        TxKind::Regular => return Ok(false),
        TxKind::Rebalance | TxKind::Liquidation | TxKind::Redeploy => {}
    }
    model::locked(c, |c| {
        let bot = model::load_bot(c, bot_id)?;
        let Some(raw) = bot.rust_placement() else { return Ok(false) };
        if raw["transaction_type"].as_str() != Some(kind.as_str()) { return Ok(false); }
        let at = crate::codec::optional_time(raw.get("at")).map_err(|_| EngineError::Data("rust_placement.at is unreadable".into()))?;
        let margin = Duration::seconds(rules.absence_margin_secs.max(DEADLINE_SECONDS + ABSENCE_AFTER_SECONDS));
        if at.is_none_or(|at| now < at + margin) { return Ok(false); }
        let reason = format!("{} placement outcome unknown: the venue has not answered for order {}", raw["side"].as_str().unwrap_or("buy"), raw["cl_ord_id"].as_str().unwrap_or_default());
        match kind {
            TxKind::Regular => Ok(false),
            TxKind::Rebalance => {
                if super::rebalance::pending(&bot.transient)?.is_some_and(|p| p.phase == super::rebalance::PHASE_AMBIGUOUS) { return Ok(false); }
                super::rebalance::halt(c, bot_id, &reason, now)?;
                Ok(true)
            }
            TxKind::Liquidation | TxKind::Redeploy => super::composition::halt(c, bot_id, kind, &reason, json!({}), now),
        }
    })
}
