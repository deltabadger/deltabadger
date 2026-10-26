//! A signal bot's webhook calls (plan 8a): Bot::SignalJob's checks, then Bots::Signal::OrderSetter#execute_signal, one call
//! at a time. The hook (slice 8b) appends each call to `bots.transient_data.rust_signal_calls`; this module consumes it
//! exactly once, in the transaction that commits its intent, or writes its skipped or failed row, or its refusal line. A
//! crash before that point runs the call again, which has touched no venue yet; after it, the intent's recovery is in charge.
//! Nothing here retries a placement: only a definitive pre-trade rejection (placement::PLACEMENT_SAFE_TRANSIENT_ERRORS) is
//! placed once more, under a new intent, after the first settled as not placed. Reads retry in place
//! (Exchange#with_transient_retry).
use super::amount::{self, OrderPlan, RowKind, TxKind};
use super::model::{self, Bot, Level, SignalCall, SignalRule};
use super::placement::{self, Sent, SignalClaim};
use super::tick::TickContext;
use super::{Clock, EngineError};
use crate::enums::BOT_WORKING;
use crate::ruby::{to_sentence, BigDec};
use crate::venue::{Attributed, Side, Venue, VenueError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

/// Bot::SignalJob::MAX_AGE: a call that waited longer is not the trade that was asked for.
pub const MAX_AGE_SECONDS: i64 = 300;
/// Bots::Signal::UNRESOLVED_PLACEMENT_KEY (R8a): an order whose fate is unknown holds the bot until it is resolved.
pub const MARKER_KEY: &str = "signal_placement_unresolved";
/// The notice kind of a signal's failure mail (notice::ERROR): Rails sends it directly, outside any daily budget.
pub const MAIL_KIND: &str = "signal";

/// `triggered_at < MAX_AGE.ago`, strictly (signal_job.rb): the one expiry rule, for the executor, the handback and the
/// takeover import alike. A call exactly MAX_AGE old still runs.
pub fn expired(triggered_at: DateTime<Utc>, now: DateTime<Utc>) -> bool { triggered_at < now - Duration::seconds(MAX_AGE_SECONDS) }

/// When `call` expires: the first instant `expired` holds.
pub fn expires_at(call: &SignalCall) -> DateTime<Utc> { call.triggered_at + Duration::seconds(MAX_AGE_SECONDS) + Duration::microseconds(1) }

/// The one barrier for both producers (§R10): Rails' marker (Bots::Signal#placement_unresolved?: a Hash under the key), or
/// this engine's unresolved intent. While it stands nothing is sized or placed.
pub fn blocked(bot: &Bot) -> bool { bot.transient.get(MARKER_KEY).is_some_and(Value::is_object) || bot.rust_placement().is_some() }

/// Removes `call`, with its Bot::SignalJob refusal line (`{signal_id}`) when there is one. In the caller's transaction.
pub fn consume(c: &Connection, bot_id: i64, call: &SignalCall, line: Option<&str>, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::remove_signal_call(c, bot_id, &call.source)?;
    if let Some(event) = line { model::log_activity(c, bot_id, event, Level::Info, json!({ "signal_id": call.signal_id }), now)?; }
    Ok(())
}

/// The marker this engine writes for an order it sent and could not get a clean answer for (R8a's shape; `id` is the
/// intent's cl_ord_id, never rewritten), as Bots::Signal#mark_placement_unresolved! writes it: merged, updated_at kept.
fn mark_unresolved(c: &Connection, bot_id: i64, cl_ord_id: &str, side: Side, error: &str, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::merge_transient_compact(c, bot_id, &[(MARKER_KEY, json!({ "id": cl_ord_id, "at": now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                                                                     "side": side.as_str(), "error": error }))])
}

/// Recovery settled the intent `cl_ord_id` (recorded or proven absent): the marker it raised goes in that transaction. A
/// marker with another id (Rails', or a newer one) stays: only its own resolution lifts it.
pub fn clear_marker(c: &Connection, bot_id: i64, cl_ord_id: &str) -> Result<(), EngineError> {
    c.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{MARKER_KEY}') WHERE id = ?1 AND json_extract(transient_data, '$.{MARKER_KEY}.id') = ?2"),
              params![bot_id, cl_ord_id])?;
    Ok(())
}

/// Bots::Signal::OrderSetter#signal_failing?: the newest REGULAR row that is not skipped failed. Read before the row that
/// would answer "yes": a skip is not a recovery.
fn failing(c: &Connection, bot_id: i64) -> Result<bool, EngineError> {
    let status: Option<Option<i64>> = c.query_row("SELECT status FROM transactions WHERE bot_id = ?1 AND transaction_type = 'REGULAR' AND status <> 2 \
                                                   ORDER BY created_at DESC, id DESC LIMIT 1", [bot_id], |r| r.get(0)).optional()?;
    Ok(status.flatten() == Some(crate::enums::TxStatus::Failed as i64))
}

/// #notify_signal_failure's mail, owed once the failed row is written: only for the first failure after a row that was not.
fn owe_mail(c: &Connection, bot_id: i64, notify: bool, errors: &[String], now: DateTime<Utc>) -> Result<(), EngineError> {
    if !notify { return Ok(()); }
    let existing = model::load_bot(c, bot_id)?.transient.get(super::notice::ERROR).cloned();
    model::merge_transient_compact(c, bot_id, &[(super::notice::ERROR, super::notice::error_marker(existing.as_ref(), MAIL_KIND, &to_sentence(errors), now))])
}

/// #record_signal_failure for something that failed before anything was sized: the failed row with no amounts, the mail
/// decision, and the call consumed, in one transaction.
/// Written under the credentials the reads were made with: a key replaced meanwhile writes nothing, and the call runs again.
#[allow(clippy::too_many_arguments)]
fn fail_unsized(c: &Connection, bot: &Bot, ticker: &model::Ticker, side: Side, call: &SignalCall, errors: Vec<String>, version: &Option<model::CredentialVersion>, now: DateTime<Utc>) -> Result<Step, EngineError> {
    super::log(&format!("execute_signal bot={} event=order_failed errors={}", bot.id, to_sentence(&errors)));
    model::credential_write(c, version, |tx| {
        let notify = !failing(tx, bot.id)?;
        amount::write_unsized_failure(tx, bot, ticker, side, &errors, now)?;
        owe_mail(tx, bot.id, notify, &errors, now)?;
        consume(tx, bot.id, call, None, now)
    })?;
    Ok(Step::Ran)
}

/// What one pass did with a bot's first waiting call.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// No call waits.
    Idle,
    /// The call was consumed: a refusal line, a row, or an intent (whatever the venue then answered).
    Ran,
    /// The call waits (the barrier stands) until it expires at the time given.
    Waiting(DateTime<Utc>),
    /// Nothing was written; the call runs again on the next pass (credentials replaced, the pair changed meanwhile).
    Again,
}

/// The bot's first waiting call, through Bot::SignalJob#perform's checks in Rails' order and then #execute_signal.
pub async fn run_next<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Step, EngineError> {
    let handle = crate::venue::Handle::new(venue, cx.credential_version.clone());
    let venue = &handle;
    let bot = model::load_bot(c, bot_id)?;
    let Some(call) = bot.signal_calls()?.into_iter().next() else { return Ok(Step::Idle) };
    let now = clock.now();
    let refuse = |line: Option<&str>| -> Result<Step, EngineError> { model::locked(c, |tx| consume(tx, bot_id, &call, line, now))?; Ok(Step::Ran) };
    // `discard_on ActiveJob::DeserializationError`: the rule was removed after the call. No line.
    let Some(rule) = model::signal(c, call.signal_id)?.filter(|r| r.bot_id == bot_id) else { return refuse(None) };
    if expired(call.triggered_at, now) { return refuse(Some("signal_expired")); }
    if !(BOT_WORKING.contains(&bot.status) && rule.enabled) { return refuse(Some("signal_ignored")); }
    // An earlier order may be on the venue: wait for its resolution, until the call expires (R8a's retry_job).
    if blocked(&bot) { return Ok(Step::Waiting(expires_at(&call))); }
    if model::key_pending(c, &bot)? { return refuse(Some("signal_api_key_pending")); }
    let ticker = model::ticker_for(c, &bot)?.ok_or_else(|| EngineError::Data(format!("bot {bot_id}: no ticker")))?;
    // Exchange#market_open?: all crypto is open; Exchanges::Alpaca asks its clock. A clock that cannot be read places
    // nothing (clock.rs fails closed); Rails reads most of those as open.
    if venue.rules().market_hours && !model::all_crypto(c, &bot)? {
        match super::clock::gate(venue.clock_result().await.value, &model::exchange_name(c, &bot)?, now) {
            super::clock::Gate::Open => {}
            super::clock::Gate::Closed { .. } => return refuse(Some("signal_market_closed")),
            super::clock::Gate::Retry(m) | super::clock::Gate::InvalidKey(m) => return fail_unsized(c, &bot, &ticker, rule.side, &call, vec![m], &cx.credential_version, now),
        }
    }
    if let Some(step) = gate(c, venue, &bot, &rule, &ticker, &call, clock, cx).await? { return Ok(step); }
    execute(c, venue, &bot, &rule, &ticker, &call, clock, cx).await
}

/// D-SIG-CONDITIONS / D-SIG-COND-LINE: a call that a rule of its side does not let through, consumed with this line
/// (`{signal_id, limit_type}`): no row, no order, the bot's status unchanged.
pub const CONDITION_UNMET: &str = "signal_condition_unmet";
/// D-SIG-CAP: a buy call the spent spending cap leaves nothing for, consumed with this line (`{signal_id}`). Sells keep working.
pub const CAP_REACHED: &str = "signal_quote_amount_limit_reached";

/// PLAN-conditions-every-bot §2c: the rules of the call's side (a buy call the plain keys, a sell call the `sell_` ones), in
/// the order Indicator, Moving average, Price drop, Price; this build runs the price rule (conditions::support). The first
/// unmet consumes the call with its line. A failed read is unmet; a rejected key is the failed row. Only this gate writes the
/// rule's met_at (stamped when met, cleared when unmet after it); there is no carry, no status and no check job. None: let
/// the call through.
#[allow(clippy::too_many_arguments)]
async fn gate<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, rule: &SignalRule, ticker: &model::Ticker, call: &SignalCall, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Option<Step>, EngineError> {
    use super::conditions::{self, Evaluated};
    let evaluated = match conditions::evaluate_side(c, venue, bot, rule.side == Side::Sell, clock, cx.prices, &cx.credential_version).await {
        Ok(evaluated) => evaluated,
        // Stopped, or its key replaced, while the price was read: nothing was written, and the call runs again.
        Err(EngineError::RunFenced | EngineError::CredentialsChanged) => return Ok(Some(Step::Again)),
        Err(e) => return Err(e),
    };
    let rules = venue.rules();
    let rejected = |e: &[String]| format!("{} rejected the API key: {}", rules.name, to_sentence(e));
    let key = match &evaluated {
        Evaluated::Off | Evaluated::Met { .. } => return Ok(None),
        Evaluated::Failed(VenueError::Unauthorized(e)) => Some(rejected(e)),
        Evaluated::Failed(VenueError::Rejected(e) | VenueError::Coded { errors: e, .. }) if rules.failure_kind(e) == Some("invalid_key") => Some(rejected(e)),
        Evaluated::Unmet | Evaluated::Unavailable | Evaluated::Failed(_) => None,
    };
    if let Some(m) = key { return fail_unsized(c, bot, ticker, rule.side, call, vec![m], &cx.credential_version, clock.now()).map(Some); }
    let now = clock.now();
    // Fenced as the gate's met_at writes are: stopped, or its key replaced, while the price was read keeps the call.
    match model::run_credential_write(c, bot, &cx.credential_version, |tx| {
        model::remove_signal_call(tx, bot.id, &call.source)?;
        model::log_activity(tx, bot.id, CONDITION_UNMET, Level::Info, json!({ "signal_id": call.signal_id, "limit_type": "price" }), now)
    }) {
        Ok(()) => Ok(Some(Step::Ran)),
        Err(EngineError::RunFenced | EngineError::CredentialsChanged) => Ok(Some(Step::Again)),
        Err(e) => Err(e),
    }
}

/// D-SIG-CAP's fast path (§2b step 1): whether the stored amounts alone prove the cap is spent, at any price: nothing left;
/// less than the quote's precision floor (`1/10^quote_decimals`); or, where the venue's market minimum is at least the quote
/// minimum at every price (Quote, BaseOrQuote, BaseAndQuoteInBase), less than `minimum_quote_size`. Under Base logic only
/// nothing left qualifies: the price decides.
pub fn cap_spent_unpriced(left: &BigDec, ticker: &model::Ticker, logic: super::venue_rules::AmountLogic) -> Result<bool, EngineError> {
    use super::venue_rules::AmountLogic;
    if !left.is_positive() { return Ok(true); }
    let (_, quote, _) = ticker.scales().map_err(EngineError::Data)?;
    if left < &BigDec::parse(&format!("1e-{quote}")).map_err(|e| EngineError::Data(format!("{e:?}")))? { return Ok(true); }
    Ok(logic != AmountLogic::Base && ticker.minimum_quote_size.is_positive() && left < &ticker.minimum_quote_size)
}

/// The buy call the spent cap leaves nothing for, consumed with its line.
fn cap_line(c: &Connection, bot: &Bot, call: &SignalCall, now: DateTime<Utc>) -> Result<Step, EngineError> {
    super::log(&format!("execute_signal bot={} event=quote_amount_limit_reached", bot.id));
    model::locked(c, |tx| consume(tx, bot.id, call, Some(CAP_REACHED), now))?;
    Ok(Step::Ran)
}


/// #execute_signal: price, size, the minimum, then placement.
#[allow(clippy::too_many_arguments)]
async fn execute<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, rule: &SignalRule, ticker: &model::Ticker, call: &SignalCall, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Step, EngineError> {
    let side = rule.side;
    let version = &cx.credential_version;
    // #buy_locked?, before anything reads the venue: the call is consumed with no line and nothing recorded.
    if locked_buy(c, bot, ticker, side, clock.now())? {
        let now = clock.now();
        model::locked(c, |tx| consume(tx, bot.id, call, None, now))?;
        return Ok(Step::Ran);
    }
    // D-SIG-CAP (§2b): what the spending cap leaves for a buy, exactly; spent by the stored amounts alone, the call is
    // consumed with the cap line before any venue read. A closed buy that never reported its cost: the failed row.
    let cap = if side == Side::Buy {
        match super::accounting::exact_cap_available(c, bot) {
            Ok(cap) => cap,
            Err(EngineError::Data(m)) if m.contains(crate::figures::fill::VALUE_UNAVAILABLE) => {
                let text = crate::web::i18n::text("en", "activerecord.errors.models.bot.attributes.settings.quote_amount_spent_unknown", &[]);
                return fail_unsized(c, bot, ticker, side, call, vec![text], version, clock.now());
            }
            Err(e) => return Err(e),
        }
    } else { None };
    if let Some(left) = &cap { if cap_spent_unpriced(left, ticker, venue.rules().minimum_logic.market)? { return cap_line(c, bot, call, clock.now()); } }
    // #signal_order_data: every read retried in place; what still fails is a failed row.
    let price = match retrying(venue, || super::tick::market_price_read(venue, bot, ticker, side, clock, cx)).await? {
        Ok(p) => p,
        Err(e) => return fail_unsized(c, bot, ticker, side, call, vec![message(e)], version, clock.now()),
    };
    let held = match side {
        Side::Buy if !rule.percentage => None,
        Side::Buy => Some(retrying(venue, || async { Ok(venue.balance_result(&model::quote_symbol(c, bot)?, model::all_crypto(c, bot)?).await.value) }).await?),
        Side::Sell => Some(retrying(venue, || async {
            let catalog = || crate::sync::balances::catalog_for(c, bot.user_id, bot.exchange_id).map_err(|e| VenueError::Rejected(vec![e.0]));
            Ok(venue.free_base_result(ticker, catalog).await.value)
        }).await?),
    };
    let held = match held.transpose() { Ok(h) => h, Err(e) => return fail_unsized(c, bot, ticker, side, call, vec![message(e)], version, clock.now()) };
    // A price that floored to zero: BigDecimal raises ZeroDivisionError in the sizing, or in #checked_amount_info.
    let sized = if price.is_zero() { None } else { size(rule, &price, held.as_ref()) };
    let Some((mut amount, mut quote)) = sized else { return fail_unsized(c, bot, ticker, side, call, vec!["divided by 0".into()], version, clock.now()) };
    // D-SIG-CAP: a buy is clamped to what the cap leaves, its base at the same price.
    let bound = cap.as_ref().is_some_and(|left| &quote > left);
    if let Some(left) = cap.filter(|_| bound) {
        amount = left.div(&price).ok_or_else(|| EngineError::Data("divided by 0".into()))?;
        quote = left;
    }
    // #checked_amount_info: under the venue's minimum, a skipped row and its line; a clamped buy under it, the cap line.
    let (plan, below) = amount::size_order(ticker, side, &price, &amount, &quote, venue.rules().minimum_logic, TxKind::Regular)?;
    if below && bound { return cap_line(c, bot, call, clock.now()); }
    // Path 2 (plan VB §1.5): Exchange#market_price_for's price, crossed already where the venue crosses; sent as it is.
    let plan = plan.with_wire_price(venue.rules(), &price, true)?;
    if below {
        super::log(&format!("execute_signal bot={} event=order_skipped base={} quote={}", bot.id, ticker.base_symbol, ticker.quote_symbol));
        let now = clock.now();
        model::credential_write(c, version, |tx| {
            model::log_activity(tx, bot.id, "order_skipped", Level::Warning, plan.log_details(), now)?;
            amount::write_order_row(tx, bot, &plan, RowKind::Skipped, now)?;
            consume(tx, bot.id, call, None, now)
        })?;
        return Ok(Step::Ran);
    }
    place(c, venue, bot, &plan, call, clock, cx).await
}

/// Bots::Signal::OrderSetter#buy_locked?: a buy of an asset the account has locked, with its log line.
pub(crate) fn locked_buy(c: &Connection, bot: &Bot, ticker: &model::Ticker, side: Side, now: DateTime<Utc>) -> Result<bool, EngineError> {
    if side != Side::Buy || !super::wash::locked(c, bot.user_id, &super::FixedClock(now))?.contains(&ticker.base_asset_id) { return Ok(false); }
    super::log(&format!("execute_signal bot={} event=order_wash_sale_locked base={}", bot.id, ticker.base_code));
    Ok(true)
}

/// Exchange#with_transient_retry: an idempotent read tried up to 3 times, 0.5 s × n apart, while it fails transiently (a
/// transport failure, or a venue's transient text). READS ONLY.
async fn retrying<T, F, Fut>(venue: &impl Venue, read: F) -> Result<Result<T, VenueError>, EngineError>
where F: Fn() -> Fut, Fut: std::future::Future<Output = Result<Result<T, VenueError>, EngineError>> {
    let mut tries = 0;
    loop {
        tries += 1;
        let result = read().await?;
        let transient = match &result {
            Err(VenueError::Transient(_)) => true,
            Err(VenueError::Rejected(e) | VenueError::Unauthorized(e) | VenueError::Coded { errors: e, .. }) => venue.rules().is_transient(e),
            _ => false,
        };
        if !(transient && tries < 3) { return Ok(result); }
        tokio::time::sleep(std::time::Duration::from_millis(500 * tries)).await;
    }
}

/// A read's failure as Rails raises it into the row: `result.errors.to_sentence`, or the transport's own message.
fn message(e: VenueError) -> String {
    match e { VenueError::Rejected(e) | VenueError::Unauthorized(e) | VenueError::Coded { errors: e, .. } => to_sentence(&e), VenueError::Transient(m) | VenueError::Ambiguous(m) => m }
}

/// #signal_size: (base, quote). A buy spends the rule's quote, or its percentage of the spendable quote (`held`); a sell is
/// its percentage of the free base, or the rule's quote at the bid capped at the free base. None for a division by zero.
pub fn size(rule: &SignalRule, price: &BigDec, held: Option<&BigDec>) -> Option<(BigDec, BigDec)> {
    let hundred = BigDec::from_i64(100);
    let share = |of: &BigDec| (of * &rule.amount).div(&hundred);
    match rule.side {
        Side::Buy => {
            let quote = if rule.percentage { share(held?)? } else { rule.amount.clone() };
            Some((quote.div(price)?, quote))
        }
        Side::Sell => {
            let free = held?;
            let amount = if rule.percentage { share(free)? } else { crate::engine::rebalance::least([rule.amount.div(price)?, free.clone()])? };
            let quote = &amount * price;
            Some((amount, quote))
        }
    }
}

/// #api_order_data: (base, quote) of an API caller's amount at `price` (slice 8f's executor shares it).
pub fn api_order_data(amount: &BigDec, in_base: bool, price: &BigDec) -> Option<(BigDec, BigDec)> {
    if in_base { Some((amount.clone(), amount * price)) } else { Some((amount.div(price)?, amount.clone())) }
}

/// #venue_takes?: whether an API caller's denomination reaches the venue as sent. A sell always goes in base.
pub fn venue_takes(side: Side, in_base: bool, logic: super::venue_rules::MinimumLogic) -> bool {
    use super::venue_rules::AmountLogic;
    if side == Side::Sell { return in_base; }
    match logic.market { AmountLogic::BaseOrQuote => true, AmountLogic::Quote => !in_base, AmountLogic::Base | AmountLogic::BaseAndQuoteInBase => in_base }
}

/// #place_and_track: the intent (and the call with it), one send, and the outcome settled with its row. A placement-safe
/// rejection is placed once more under a new intent, after the first settled as not placed.
async fn place<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, plan: &OrderPlan, call: &SignalCall, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Step, EngineError> {
    let mut call = Some(call);
    loop {
        let intent = match placement::begin_signal(c, bot, plan, call, &cx.credential_version, venue.placement_identity(), clock)? {
            SignalClaim::Intent(intent) => intent,
            SignalClaim::Stopped | SignalClaim::Disabled | SignalClaim::Locked | SignalClaim::BelowMinimum => return Ok(Step::Ran),
            // The barrier rose between the check and the claim (it cannot in one loop, but the claim decides).
            SignalClaim::Blocked => return Ok(match call { Some(call) => Step::Waiting(expires_at(call)), None => Step::Ran }),
            SignalClaim::Gone | SignalClaim::Changed | SignalClaim::CredentialsChanged => return Ok(if call.is_some() { Step::Again } else { Step::Ran }),
        };
        let first = call.is_some();
        call = None; // consumed with the intent
        super::log(&format!("execute_signal bot={} event=order_creating cl_ord_id={}", bot.id, intent.cl_ord_id));
        let sent = placement::send(c, venue, &intent, clock).await?;
        let now = clock.now();
        let tx = model::immediate(c)?;
        match &sent {
            Sent::Accepted(txid) => {
                if placement::still_pending(&tx, bot.id, &intent.cl_ord_id)? {
                    amount::write_accepted_order(&tx, bot, &intent.plan, txid, intent.at, None)?;
                    placement::set_intent(&tx, bot.id, None)?;
                }
                super::log(&format!("execute_signal bot={} event=order_accepted order_id={txid}", bot.id));
            }
            // Signed-timestamp rejections never reached the book: nothing to record; the one more attempt follows.
            Sent::Rejected(errors) if first && placement::safe_rejection(errors) => { placement::set_intent(&tx, bot.id, None)?; }
            // A signal order is outside D-BELOW-MIN-REJECTION (placement::skips_below_minimum): never BelowMinimum, read as Rejected.
            Sent::Rejected(errors) | Sent::BelowMinimum(errors) => { fail_sized(&tx, bot, &intent, errors.clone(), &cx.credential_version, now)?; }
            Sent::NotSent(m) => { fail_sized(&tx, bot, &intent, vec![m.clone()], &cx.credential_version, now)?; }
            Sent::Ambiguous(m) => {
                // The marker first: it stops the next call from placing on top of this one. The intent stays for recovery.
                mark_unresolved(&tx, bot.id, &intent.cl_ord_id, intent.plan.side, m, now)?;
                let mut details = intent.plan.log_details();
                details["error"] = json!(m);
                model::log_activity(&tx, bot.id, "placement_ambiguous", Level::Warning, details, now)?;
                super::log(&format!("execute_signal bot={} event=placement_ambiguous cl_ord_id={} error={m}", bot.id, intent.cl_ord_id));
            }
            // Deleted or archived before the send: nothing left, nothing recorded.
            Sent::Fenced => {}
        }
        tx.commit()?;
        if !matches!(&sent, Sent::Rejected(errors) if first && placement::safe_rejection(errors)) { return Ok(Step::Ran); }
    }
}

/// A definitive rejection, or a send that provably never left: the failed row with the order's amounts, the mail decision,
/// the intent gone. In the caller's transaction.
fn fail_sized(tx: &rusqlite::Transaction<'_>, bot: &Bot, intent: &placement::Intent, errors: Vec<String>, version: &Option<model::CredentialVersion>, now: DateTime<Utc>) -> Result<(), EngineError> {
    let fenced = model::check_credential_result(tx, version)?;
    if !placement::still_pending(tx, bot.id, &intent.cl_ord_id)? { return Ok(()); }
    super::log(&format!("execute_signal bot={} event=order_failed errors={}", bot.id, to_sentence(&errors)));
    let notify = !failing(tx, bot.id)?;
    amount::write_order_row(&fenced, bot, &intent.plan, RowKind::Failed { errors: errors.clone() }, now)?;
    owe_mail(tx, bot.id, notify, &errors, now)?;
    placement::set_intent(tx, bot.id, None)
}

/// Every signal bot with a waiting call, by id.
pub fn bots_with_calls(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare(&format!("SELECT id FROM bots WHERE type = '{}' AND json_array_length(transient_data, '$.{}') > 0 ORDER BY id",
                                   model::SIGNAL_TYPE, model::SIGNAL_CALLS_KEY))?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}
