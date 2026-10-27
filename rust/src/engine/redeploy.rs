//! Bot::RedeployJob and Bot::Composition::Redeployable: the user's Yes puts a liquidation's proceeds back into the composition.
use super::{amount::{self, OrderPlan, Origin, TxKind}, basket, composition, model::{self, Level, Ticker}, placement, rebalance, tick, Clock, EngineError};
use super::venue_rules::{AmountLogic, Rounding, VenueRules};
use crate::{enums::BotStatus, figures::{dec::Dec, db::Subject, walk::Metrics, FiguresError}, ruby::BigDec, venue::{Attributed, Side, Venue}};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

pub const REQUEST_KEY: &str = "rust_redeploy_request";
pub const PENDING_KEY: &str = "redeploy_pending";
/// Redeployable::BASE_HEADROOM, kept where the order goes out base-denominated at a crossing venue's persisted limit.
const BASE_HEADROOM: &str = "1.01";

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }
fn big(d: &Dec) -> Result<BigDec, EngineError> { BigDec::parse(&d.to_s_f()).map_err(data) }

pub(crate) fn refuse(c: &Connection, id: i64, reason: Value, clock: &dyn Clock) -> Result<(), EngineError> {
    model::log_activity(c, id, "redeploy_not_started", Level::Info, json!({"reason":reason}), clock.now())
}
fn skip(c: &Connection, id: i64, ticker: &Ticker, reason: &str, clock: &dyn Clock) -> Result<(), EngineError> {
    model::log_activity(c, id, "redeploy_skipped", Level::Info, json!({"base":ticker.base_symbol,"reason":reason}), clock.now())
}

/// D-6C-9: Redeployable#redeploy_offer, for the engine, the panel and MCP alike. A negative difference (an offset above the
/// banked total) is clamped to 0 and logged, never rewritten; an offer under the composition's smallest quote minimum (0 when
/// no member states one) is 0.
pub fn offer(c: &Connection, s: &Subject, m: &Metrics) -> Result<Dec, FiguresError> {
    let (banked, spent) = super::accounting::index_redeploy_totals(&s.orders)?;
    let offset = c.query_row("SELECT redeploy_declined_offset FROM bots WHERE id=?1", [s.bot.id], |r| r.get::<_, rusqlite::types::Value>(0))?;
    let offset = Dec::from_sql((&offset).into())?.unwrap_or_else(Dec::zero);
    let offerable = (&(&banked - &spent)? - &offset)?;
    if offerable.is_negative() {
        super::log(&format!("[engine] redeploy offset stranded above banked total bot={} offset={} banked={} spent={}", s.bot.id, offset.to_s_f(), banked.to_s_f(), spent.to_s_f()));
    }
    let cash = m.walked.as_ref().map(|w| w.realised_cash.to_d()).transpose()?.unwrap_or_else(Dec::zero);
    let offer = offerable.max(Dec::zero()).min(cash);
    let minimum = crate::figures::db::composition_minimum(c, s)?.unwrap_or_else(Dec::zero);
    Ok(if offer < minimum { Dec::zero() } else { offer })
}

/// The request's owner clears only the version it took.
pub(crate) fn clear(c: &Connection, id: i64, request: &Value) -> Result<(), EngineError> {
    model::locked(c, |c| {
        if model::load_bot(c, id)?.transient[REQUEST_KEY] == *request { model::merge_transient_compact(c, id, &[(REQUEST_KEY, Value::Null)])?; }
        Ok(())
    })
}

/// D-6C-12: what one share sends, or None below the venue's minimum. A quote-sized market buy where the venue's market
/// logic takes quote and the quote clears its minimum. Otherwise base-denominated: a crossing venue keeps Rails' path (its
/// persisted crossed limit, with the 1% haircut); any other venue sends a GTC limit at ref × 1.01, sized share / limit and
/// floored, checked against the limit minimums at that price and quantity. Spend never exceeds the share there.
/// `headroom`: false when the fold only asks whether a share is placeable (Redeployable#placeable? sizes without it).
pub fn size(ticker: &Ticker, price: &BigDec, spend: &BigDec, rules: &VenueRules, headroom: bool) -> Result<Option<OrderPlan>, EngineError> {
    let (base_decimals, quote_decimals, _) = ticker.scales().map_err(EngineError::Data)?;
    if !price.is_positive() || !spend.is_positive() { return Ok(None); }
    let amount = spend.div(price).ok_or_else(|| data("zero price"))?;
    let rule = rules.minimum_logic.market;
    let quote = spend.floor(quote_decimals);
    let quote_clears = match rule {
        AmountLogic::Quote => !amount::buy_minimum(ticker, price, rule, &quote, &quote.div(price).ok_or_else(|| data("zero price"))?)?.1,
        AmountLogic::BaseOrQuote => quote.is_positive() && quote >= (&ticker.minimum_base_size * price).ceil(quote_decimals).max(ticker.minimum_quote_size.clone()),
        AmountLogic::Base | AmountLogic::BaseAndQuoteInBase => false,
    };
    if quote_clears {
        let (plan, _) = amount::size_order(ticker, Side::Buy, price, &amount, spend, rules.minimum_logic, TxKind::Redeploy)?;
        return Ok(Some(OrderPlan { quote_type: true, volume: quote, ..plan }.with_wire_price(rules, price, true)?));
    }
    if rules.cross.is_some() {
        let (plan, below) = amount::size_order(ticker, Side::Buy, price, &amount, spend, rules.minimum_logic, TxKind::Redeploy)?;
        let (plan, below) = if headroom && !plan.quote_type {
            let cut = amount.div(&BigDec::parse(BASE_HEADROOM).expect("decimal")).expect("non-zero").floor(base_decimals);
            amount::size_order(ticker, Side::Buy, price, &cut, spend, rules.minimum_logic, TxKind::Redeploy)?
        } else { (plan, below) };
        return Ok(if below { None } else { Some(plan.with_wire_price(rules, price, true)?) });
    }
    // r5 rows 1, 6 and 8: never a base market buy on a venue that cannot bound it.
    let limit = rules.adjusted_price(&(price * &BigDec::parse(BASE_HEADROOM).expect("decimal")), ticker, Rounding::Floor).map_err(EngineError::Data)?;
    let Some(qty) = spend.div(&limit).map(|q| q.floor(base_decimals)) else { return Ok(None) };
    let worth = &qty * &limit;
    let below = qty.is_zero() || worth < ticker.minimum_quote_size || qty < ticker.minimum_base_size
        || amount::buy_minimum(ticker, &limit, rules.minimum_logic.limit, &worth.floor(quote_decimals), &qty)?.1;
    Ok((!below).then(|| OrderPlan { ticker: ticker.clone(), side: Side::Buy, limit: true, price: limit, amount: qty.clone(), quote_amount: spend.clone(),
        quote_type: false, volume: qty, kind: TxKind::Redeploy, origin: Origin::QuoteBudget, wire_price: None }))
}

/// Redeployable#redeploy!, after Bot::RedeployJob's own refusals.
pub async fn run<V: Venue>(c: &Connection, venue: &V, id: i64, clock: &dyn Clock, cx: &tick::TickContext<'_>) -> Result<(), EngineError> {
    let handle = crate::venue::Handle::new(venue, cx.credential_version.clone());
    let venue = &handle;
    let bot = model::load_bot(c, id)?;
    if matches!(bot.status, BotStatus::Deleted | BotStatus::Archived) { return refuse(c, id, json!("archived"), clock); }
    if model::key_pending(c, &bot)? { return refuse(c, id, json!("api_key_pending"), clock); }
    // #advance_waiting_redeploys!: this leg's own waiting rows, swept with every other; abandoned ones halt.
    let watched = composition::waiting_rows(c, Some(id), Some(TxKind::Redeploy))?;
    if !watched.is_empty() {
        let before = composition::halted(c, &watched)?;
        if let Err(e) = super::polling::sweep(c, venue, &bot, clock.now()).await { super::log(&format!("[engine] redeploy order refresh failed bot={id}: {e:?}")); }
        composition::halt_abandoned(c, &watched, &before, clock.now())?;
    }
    composition::promote_orphan(c, id, TxKind::Redeploy, clock.now())?;
    let bot = model::load_bot(c, id)?;
    if let Some(stale) = super::staleness::stale(c, &bot, clock.now())? { return refuse(c, id, json!([stale.message]), clock); }
    let mut claim = rebalance::Claim { composition: placement::composition_snapshot(c, &bot)?, bot };
    if !rebalance::before_rebalance(c, venue, &mut claim, clock, cx).await? { return refuse(c, id, json!(["changed"]), clock); }
    // After the refresh, on the members being bought (a refresh can rotate a closed-market member in).
    let composed = composition_tickers(c, &claim.bot)?;
    // Exchanges::Alpaca#all_crypto?: an empty list is not all crypto, so the clock is asked.
    let crypto = !composed.is_empty() && composed.iter().all(|t| t.crypto);
    if venue.rules().market_hours && !crypto
        && !matches!(super::clock::gate(venue.clock_result().await.value, &model::exchange_name(c, &claim.bot)?, clock.now()), super::clock::Gate::Open) {
        return refuse(c, id, json!(["market_closed"]), clock);
    }
    let current = model::load_bot(c, id)?;
    let reason = if current.transient.get(PENDING_KEY).is_some_and(|p| p.as_object().is_some_and(|p| !p.is_empty())) { Some("halted") }
        else if !composition::waiting_rows(c, Some(id), Some(TxKind::Redeploy))?.is_empty() { Some("orders_waiting") }
        else if current.selling() { Some("selling") }
        else if rebalance::is_pending(&current.transient) { Some("rebalance_pending") }
        else if composition::liquidation_in_flight(c, &current, clock.now())? { Some("liquidation_pending") }
        else if current.rust_placement().is_some() { Some("halted") }
        else if super::reversal::barred(&current) { Some("reversal_pending") }
        else { None };
    if let Some(reason) = reason { return refuse(c, id, json!([reason]), clock); }
    let Some(reconciled) = rebalance::split_gate(c, venue, &claim, clock).await? else { return refuse(c, id, json!(["split_hold"]), clock) };
    let bot = &claim.bot;
    let locked = super::wash::locked(c, bot.user_id, clock)?;
    let members = basket::members(c, bot, &locked)?;
    // #redeployable_amount: the offer on a fresh walk, capped by the venue's free quote (the user may have withdrawn it).
    let subject = Subject::load(c, id).map_err(data)?;
    let at = crate::figures::at::At::from_utc(clock.now()).ok_or_else(|| data("invalid time"))?;
    let walked = crate::figures::walk::metrics(c, &subject, at).map_err(data)?;
    let offered = big(&offer(c, &subject, &walked).map_err(data)?)?;
    let Some(any) = members.first().map(|m| m.ticker.clone()).or(composed.first().cloned()) else { return refuse(c, id, json!(["No assets in composition"]), clock) };
    let free = tick::live_free_balance(c, venue, bot, &any, any.quote_asset_id).await?.map_err(data)?;
    let total = offered.min(free);
    if !total.is_positive() { return refuse(c, id, json!(["nothing_to_redeploy"]), clock); }
    if members.is_empty() {
        if composed.is_empty() { return refuse(c, id, json!(["No assets in composition"]), clock); }
        super::log(&format!("[engine] redeploy bot={id} event=all_members_wash_sale_locked"));
        return Ok(());
    }
    // get_orders_data(amount, market: true): holdings with resting buys, every member at its market buy price, the split.
    let Some(holdings) = model::locked(c, |tx| {
        if super::splits::snapshot(tx, bot)? != reconciled { return Ok(None); }
        Ok(Some(basket::walk(tx, bot, clock.now())?.amounts))
    })? else { return refuse(c, id, json!(["changed"]), clock) };
    let reserved = basket::reserved(c, bot)?;
    let mut priced = vec![];
    for member in members {
        let price = match tick::market_price_read(venue, bot, &member.ticker, Side::Buy, clock, cx).await? {
            Ok(p) => p,
            Err(crate::venue::VenueError::Transient(m) | crate::venue::VenueError::Ambiguous(m)) => return Err(EngineError::Data(format!("No price for {}: {m}", member.ticker.base_symbol))),
            Err(e) => return Err(EngineError::Data(format!("No price for {}: {}", member.ticker.base_symbol, crate::ruby::to_sentence(e.rejection().unwrap_or_default())))),
        };
        priced.push(basket::Priced { member, reference: price.clone(), price });
    }
    let legs = match basket::split(&priced, &holdings, &reserved, &total) {
        Ok(legs) => legs,
        Err(decimals) => return refuse(c, id, json!([format!("limit price rounds to zero at {decimals} decimals")]), clock),
    };
    let rules = venue.rules();
    let legs = fold(c, id, legs, rules, clock)?;
    let tickers: Vec<_> = priced.iter().map(|p| &p.member.ticker).collect();
    let (mut remaining, mut placed, mut washed) = (total, 0, 0);
    for leg in legs {
        if !remaining.is_positive() { break; }
        // Shutdown: what was placed stands; nothing else is reported (the batch did not run out of placeable shares).
        if (cx.stopping)() { return Ok(()); }
        let ticker = &leg.ticker;
        if super::wash::locked(c, bot.user_id, clock)?.contains(&ticker.base_asset_id) {
            super::log(&format!("[engine] redeploy bot={id} event=order_wash_sale_locked base={}", ticker.base_code));
            washed += 1;
            continue;
        }
        // Re-priced at placement, as #place_one_redeploy! re-reads it.
        let Some(price) = tick::market_price_for(venue, bot, ticker, Side::Buy, clock, cx).await? else { skip(c, id, ticker, "unpriced", clock)?; continue };
        let spend = leg.quote.clone().min(remaining.clone());
        if !spend.is_positive() { skip(c, id, ticker, "budget_exhausted", clock)?; continue; }
        let Some(plan) = size(ticker, &price, &spend, rules, true)? else { skip(c, id, ticker, "below_minimum", clock)?; continue };
        let hold = venue.placement_hold(ticker, Side::Buy).await.map_err(data)?;
        let begun = placement::begin_operation(c, bot, &plan, &tickers, &claim.composition, &reconciled, &cx.credential_version, venue.placement_identity(),
            placement::Op::Redeploy, false, hold.as_ref(), clock);
        drop(hold);
        let intent = match begun? {
            placement::Begun::Intent(i) => i,
            placement::Begun::WashSaleLocked => { washed += 1; continue; }
            placement::Begun::BelowMinimum(_) => { skip(c, id, ticker, "below_minimum", clock)?; continue; }
            placement::Begun::CredentialsChanged => return Err(EngineError::CredentialsChanged),
            placement::Begun::Refused(m) => return Err(EngineError::Data(m)),
            placement::Begun::LedgerStale(stale) => return refuse(c, id, json!([stale.message]), clock),
            placement::Begun::Changed | placement::Begun::Fenced => if placed == 0 { return refuse(c, id, json!(["changed"]), clock) } else { return Ok(()) },
        };
        match placement::send_recorded(c, venue, bot, &intent, clock).await? {
            placement::Sent::Accepted(_) => { remaining = &remaining - &spend; placed += 1; }
            placement::Sent::NotSent(_) => {}
            placement::Sent::Rejected(errors) if placement::safe_rejection(&errors) => {}
            // Halted: nothing else may trade until the user resolves it.
            placement::Sent::Rejected(_) | placement::Sent::BelowMinimum(_) | placement::Sent::Ambiguous(_) | placement::Sent::Fenced => return Ok(()),
        }
    }
    if placed == 0 && washed > 0 {
        super::log(&format!("[engine] redeploy bot={id} skipped=wash_sale_locked"));
        return Ok(());
    }
    if placed == 0 { return model::log_activity(c, id, "redeploy_below_minimums", Level::Info, json!({"reason":["below_minimums"]}), clock.now()); }
    Ok(())
}

/// Allocatable#composition_tickers: the in-index members' tickers.
fn composition_tickers(c: &Connection, bot: &model::Bot) -> Result<Vec<Ticker>, EngineError> {
    let ids = c.prepare("SELECT ticker_id FROM bot_index_assets WHERE bot_id=?1 AND in_index=1 AND ticker_id IS NOT NULL ORDER BY id")?
        .query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for t in ids { if let Some(t) = model::ticker_by_id(c, bot.exchange_id, t)? { out.push(t); } }
    Ok(out)
}

/// #fold_below_minimums: every share no order can place is pooled into the largest placeable one.
fn fold(c: &Connection, id: i64, legs: Vec<basket::Leg>, rules: &VenueRules, clock: &dyn Clock) -> Result<Vec<basket::Leg>, EngineError> {
    let mut viable = vec![];
    let mut tiny = vec![];
    for leg in legs {
        if size(&leg.ticker, &leg.reference, &leg.quote, rules, false)?.is_some() { viable.push(leg) } else { tiny.push(leg) }
    }
    if tiny.is_empty() || viable.is_empty() { viable.extend(tiny); return Ok(viable); }
    let pooled = tiny.iter().fold(BigDec::zero(), |a, l| &a + &l.quote);
    // Enumerable#max_by keeps the first of equal maxima.
    let receiver = (0..viable.len()).fold(0, |best, i| if viable[i].quote > viable[best].quote { i } else { best });
    model::log_activity(c, id, "redeploy_folded", Level::Info, json!({"into":viable[receiver].ticker.base_code,
        "amount":(pooled.to_f() * 100.0).round() / 100.0,"from":tiny.iter().map(|l|l.ticker.base_code.as_str()).collect::<Vec<_>>()}), clock.now())?;
    viable[receiver].quote = &viable[receiver].quote + &pooled;
    Ok(viable)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Declined { Declined, Refused }

/// Bot::DeclineRedeployJob in the web's transaction: refused while a request waits or a redeploy is in flight (the offset is a
/// snapshot of `banked - spent`, and a growing spend would strand it); otherwise the offset makes the offer read zero now.
pub fn decline(c: &Connection, id: i64, user: i64, now: chrono::DateTime<chrono::Utc>) -> Result<Declined, EngineError> {
    let tx = model::immediate(c)?;
    let bot = model::load_bot(&tx, id)?;
    if bot.user_id != user { return Err(data("redeploy owner changed")); }
    if composition::redeploy_in_flight(&tx, &bot)? {
        model::log_activity(&tx, id, "redeploy_decline_refused", Level::Info, json!({"reason":["redeploy_in_flight"]}), now)?;
        tx.commit()?;
        return Ok(Declined::Refused);
    }
    let (banked, spent) = super::accounting::index_redeploy_totals(&crate::figures::db::orders(&tx, id).map_err(data)?).map_err(data)?;
    tx.execute("UPDATE bots SET redeploy_declined_offset=?1 WHERE id=?2", (((&banked - &spent).map_err(data)?).to_s_f(), id))?;
    model::log_activity(&tx, id, "redeploy_declined", Level::Info, json!({"user_id":user}), now)?;
    tx.commit()?;
    Ok(Declined::Declined)
}

/// T4 / D-6C-2 / D-6C-11: Clear of an ambiguous redeploy, under the lock recovery takes. A placement intent this halt still
/// holds is removed with it, and what it asked to spend joins the declined offset, so a later Yes never offers it again.
/// Returns the waiting venue id while a REDEPLOY row still waits.
pub fn resolve(c: &Connection, id: i64, user: i64, nonce: &str, now: chrono::DateTime<chrono::Utc>) -> Result<Option<String>, EngineError> {
    let tx = model::immediate(c)?;
    let bot = model::load_bot(&tx, id)?;
    if bot.user_id != user { return Err(data("redeploy owner changed")); }
    let waiting: Option<Option<String>> = tx.query_row("SELECT external_id FROM transactions WHERE bot_id=?1 AND transaction_type='REDEPLOY' AND status=0 AND external_status IN (0,1) ORDER BY id LIMIT 1",
        [id], |r| r.get(0)).optional()?;
    if let Some(order) = waiting { return Ok(Some(order.unwrap_or_default())); }
    let pending = &bot.transient[PENDING_KEY];
    if pending["state"] != "ambiguous" || pending["id"].as_str() != Some(nonce) { return Ok(None); }
    if let Some(intent) = placement::load_intent(&tx, &bot, None)? {
        // Recovery committed a different intent meanwhile: it is not this halt's to remove.
        if intent.plan.kind != TxKind::Redeploy || intent.cl_ord_id != nonce { return Ok(None); }
        let stored = tx.query_row("SELECT redeploy_declined_offset FROM bots WHERE id=?1", [id], |r| r.get::<_, rusqlite::types::Value>(0))?;
        let stored = crate::ruby::from_sql((&stored).into()).map_err(data)?.unwrap_or_else(BigDec::zero);
        tx.execute("UPDATE bots SET redeploy_declined_offset=?1 WHERE id=?2", ((&stored + &intent.plan.quote_amount).to_s_f(), id))?;
        model::merge_transient_compact(&tx, id, &[("rust_placement", Value::Null)])?;
    }
    // The abandoned rows this Clear accounted for, kept in state (the activity log is pruned) so the backstop scan skips them.
    let mut resolved: Vec<i64> = bot.transient["redeploy_resolved_orders"].as_array().into_iter().flatten().filter_map(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok())).collect();
    for row in tx.prepare("SELECT id FROM transactions WHERE bot_id=?1 AND transaction_type='REDEPLOY' AND external_status=4 ORDER BY id")?.query_map([id], |r| r.get::<_, i64>(0))? {
        let row = row?;
        if !resolved.contains(&row) { resolved.push(row); }
    }
    model::merge_transient_compact(&tx, id, &[(PENDING_KEY, Value::Null), ("redeploy_resolved_orders", if resolved.is_empty() { Value::Null } else { json!(resolved) })])?;
    model::log_activity(&tx, id, "redeploy_manually_resolved", Level::Info, json!({"user_id":user}), now)?;
    tx.commit()?;
    Ok(None)
}
