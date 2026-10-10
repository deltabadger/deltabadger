//! A DCA basket's composition, normalized REGULAR history and buy split.
//! Submitted buys and sells share the figures normalizer and RebalanceAccounting books.
//! Inherited merge rows stay in chronological order; their id cutoff only identifies own orders.
//! Settled REBALANCE/LIQUIDATION/REDEPLOY rows move units as Bot::RebalanceAccounting moves them (B2b); eligibility keeps
//! every in-flight one out.
use super::model::{self, Bot, Ticker};
use super::EngineError;
use super::splits::{self, SplitEvent};
use crate::codec::format_time;
use crate::figures::{books::{Books, Fill, Ledger}, db, dec::Dec, fill, num::Num};
use crate::ruby::{decimal_column, float_sum, from_sql, BigDec};
use chrono::{DateTime, Utc};
use rusqlite::types::{Value as Sql, ValueRef};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }
/// The transaction types Rails writes; eligibility names any other.
const KINDS: [&str; 4] = ["REGULAR", "REBALANCE", "LIQUIDATION", "REDEPLOY"];

/// Normalized REGULAR history: holdings, lifetime contribution, uninvested proceeds, and the last
/// split that moved a held position. Amounts and books use checked decimal arithmetic.
#[derive(Debug)]
pub struct Walk { pub amounts: HashMap<i64, BigDec>, pub restated_at_us: Option<i64>, pub contributed: BigDec, pub cash: BigDec }
impl Default for Walk {
    fn default() -> Self { Self { amounts: HashMap::new(), restated_at_us: None, contributed: BigDec::zero(), cash: BigDec::zero() } }
}

/// Base amount held, by asset id: `metrics(force: true)[:asset_breakdown][key_for(asset_id)][:amount]`, splits applied. For
/// callers without a tick clock (2c's ledger vectors); a tick calls `walk` with its own.
pub fn holdings(c: &Connection, bot: &Bot) -> Result<HashMap<i64, BigDec>, EngineError> { Ok(walk(c, bot, Utc::now())?.amounts) }

/// Bot::Composition::Measurable#metrics with its split queue (measurable.rb:51-58, :179-180): the events effective at `now`.
pub fn walk(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Walk, EngineError> {
    crate::figures::budget::within(|| {
        let s = splits::splits(c, bot, now)?;
        walk_with(c, bot, &s.events)
    })
}

/// Split events apply before orders at the same timestamp, and after the final order.
/// Every submitted REGULAR row is normalized, including inherited and partial fills.
pub fn walk_with(c: &Connection, bot: &Bot, events: &[SplitEvent]) -> Result<Walk, EngineError> {
    crate::figures::budget::within(|| walk_bounded(c, bot, events))
        .map_err(|e| EngineError::Data(format!("split walk refused: {e:?}")))
}
fn walk_bounded(c: &Connection, bot: &Bot, events: &[SplitEvent]) -> Result<Walk, EngineError> {
    let orders = db::orders(c, bot.id).map_err(data)?;
    let mut ledger = Ledger::default();
    let mut books = Books::default();
    let mut w = Walk::default();
    let mut pending = events.iter().peekable();
    for order in orders {
        crate::figures::budget::charge(1, 0).map_err(data)?;
        let created_us = order.at.0 / 1000;
        while let Some(e) = pending.next_if(|e| e.at_us <= created_us) {
            apply(&mut ledger, &mut w, e)?;
        }
        let asset = order.asset_id.filter(|_| (order.buy || order.sell) && KINDS.contains(&order.kind.as_str()))
            .ok_or_else(|| data("an unknown transaction type, missing side or missing asset is outside the history walk"))?;
        let key = asset.to_string();
        // A non-REGULAR sell takes Rails' raw-proceeds gates (MQ6); every buy and REGULAR sell the shared normalizer (B2-1).
        let fill = match fill::special_sell_for_engine(&order).map_err(data)? {
            Some(sell) if sell.unpriced() => { books.unpriced_liquidation(&mut ledger, &key, &sell.executed).map_err(data)?; continue; }
            Some(sell) => sell.fill(),
            None => fill::for_engine(&order).map_err(data)?,
        };
        let Some(fill) = fill else { continue };
        books.apply(&mut ledger, Fill::of(order.sell, &order.kind), &key, &Num::Dec(fill.quantity), &Num::Dec(fill.value)).map_err(data)?;
    }
    for e in pending { apply(&mut ledger, &mut w, e)?; }
    for (key, entry) in ledger.0 {
        w.amounts.insert(key.parse().map_err(data)?, BigDec::parse(&entry.amount.to_d().map_err(data)?.to_s_f()).map_err(data)?);
    }
    w.contributed = BigDec::parse(&books.contributed.to_d().map_err(data)?.to_s_f()).map_err(data)?;
    w.cash = BigDec::parse(&books.uninvested_cash().map_err(data)?.to_d().map_err(data)?.to_s_f()).map_err(data)?;
    Ok(w)
}

fn apply(ledger: &mut Ledger, w: &mut Walk, e: &SplitEvent) -> Result<(), EngineError> {
    crate::figures::budget::charge(1, 0).map_err(data)?;
    let key = e.asset_id.to_string();
    let Some(held) = ledger.get(&key).filter(|entry| !entry.amount.is_zero()).cloned() else { return Ok(()) };
    let factor = Num::Dec(Dec::parse(&e.factor.to_s_f()).map_err(data)?);
    ledger.entry(&key).amount = held.amount.mul(&factor).map_err(data)?;
    w.restated_at_us = Some(w.restated_at_us.map_or(e.at_us, |r| r.max(e.at_us)));
    Ok(())
}

/// Bot::Composition::OrderSetter#reserved_waiting_amounts(:buy): the unexecuted remainder (`amount.to_d - amount_exec.to_d`,
/// blank reading 0) of every waiting buy, by asset. A resting order counts as held, so its member is not bought twice.
pub fn reserved(c: &Connection, bot: &Bot) -> Result<HashMap<i64, BigDec>, EngineError> {
    crate::figures::budget::within(|| fill::reserved(c,bot.id)).map_err(data)
}

/// bot_index_assets.target_allocation as ActiveRecord reads it back: ActiveModel::Type::Decimal(precision 10, scale 6). A REAL
/// goes through the Float cast; an INTEGER or TEXT through BigDecimal, then round(6).
pub fn target_from_sql(v: ValueRef<'_>) -> Result<Option<BigDec>, EngineError> {
    match v {
        ValueRef::Real(f) => decimal_column(f, 10, 6).map(Some).map_err(data),
        other => Ok(from_sql(other).map_err(data)?.map(|d| d.round(6))),
    }
}

/// Bots::DcaMultiAsset#refresh_composition, on every tick before anything is sized: derive_composition, then
/// update_bot_index_assets. Err(message) is Rails' Failure, which fails the tick; nothing is written then.
pub fn refresh_composition(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Result<(), String>, EngineError> {
    let allocations = bot.allocations().ok_or_else(|| EngineError::Data(format!("bot {}: allocations this build does not read", bot.id)))?;
    // derive_composition: in settings order, a member counts only with an available, trading-enabled ticker at the quote.
    let mut matched: Vec<(i64, i64, f64)> = vec![];
    for (asset_id, weight) in allocations {
        let ticker: Option<i64> = c.query_row(
            "SELECT id FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2 AND quote_asset_id = ?3 AND available = 1 AND trading_enabled = 1",
            params![bot.exchange_id, asset_id, bot.quote_asset_id()], |r| r.get(0)).optional()?;
        if let Some(ticker_id) = ticker { matched.push((asset_id, ticker_id, weight)); }
    }
    // `matched.sum { weight }` is Ruby's compensated Float sum; each weight is renormalised in Float (`weight / total`).
    let total = float_sum(&matched.iter().map(|(_, _, w)| *w).collect::<Vec<_>>());
    if total <= 0.0 { return Ok(Err(format!("None of the portfolio's weighted assets trade on {}", model::exchange_name(c, bot)?))); }
    let members: Vec<(i64, i64, f64)> = matched.iter().map(|(asset, ticker, weight)| (*asset, *ticker, weight / total)).collect();
    write_members(c, bot.id, &members, now)?;
    Ok(Ok(()))
}

/// A member's row: id, ticker_id, target_allocation (as read back), in_index, entered_at, exited_at.
type MemberRow = (i64, i64, Result<Option<BigDec>, EngineError>, Option<bool>, Option<String>, Option<String>);

/// Allocatable#save_member: find or initialize the member's row and save it. ActiveRecord writes only the attributes that
/// changed, and moves updated_at only then; a new row is inserted with created_at, updated_at and entered_at at now.
fn save_member(c: &Connection, bot_id: i64, asset_id: i64, ticker_id: i64, weight: f64, now: DateTime<Utc>) -> Result<(), EngineError> {
    let target = decimal_column(weight, 10, 6).map_err(data)?;
    let now_text = format_time(now);
    let row: Option<MemberRow> = c.query_row(
        "SELECT id, ticker_id, target_allocation, in_index, entered_at, exited_at FROM bot_index_assets WHERE bot_id = ?1 AND asset_id = ?2",
        params![bot_id, asset_id],
        |r| Ok((r.get(0)?, r.get(1)?, target_from_sql(r.get_ref(2)?), r.get(3)?, r.get(4)?, r.get(5)?))).optional()?;
    let Some((id, old_ticker, old_target, in_index, entered_at, exited_at)) = row else {
        c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5, ?5)", params![bot_id, asset_id, ticker_id, target.to_f(), now_text])?;
        return Ok(());
    };
    let mut sets: Vec<(&str, Sql)> = vec![];
    if old_ticker != ticker_id { sets.push(("ticker_id", Sql::Integer(ticker_id))); }
    if old_target? != Some(target.clone()) { sets.push(("target_allocation", Sql::Real(target.to_f()))); }
    if in_index != Some(true) { sets.push(("in_index", Sql::Integer(1))); }
    if entered_at.is_none() { sets.push(("entered_at", Sql::Text(now_text.clone()))); }
    if exited_at.is_some() { sets.push(("exited_at", Sql::Null)); }
    if sets.is_empty() { return Ok(()); }
    let assignments: Vec<String> = sets.iter().enumerate().map(|(i, (col, _))| format!("{col} = ?{}", i + 1)).collect();
    let mut values: Vec<Sql> = sets.into_iter().map(|(_, v)| v).collect();
    values.push(Sql::Text(now_text));
    values.push(Sql::Integer(id));
    c.execute(&format!("UPDATE bot_index_assets SET {}, updated_at = ?{} WHERE id = ?{}", assignments.join(", "), values.len() - 1, values.len()),
              rusqlite::params_from_iter(values))?;
    Ok(())
}

/// A member as get_orders_data takes it: its ticker (bot_index_assets.ticker_id) and its weight, already `.to_f`.
#[derive(Clone, Debug)]
pub struct Member { pub asset_id: i64, pub ticker: Ticker, pub weight: f64 }

/// Bot::Composition::Allocatable#buyable_allocations with no wash-sale lock (refused by eligibility): the in_index rows by
/// target_allocation DESC, ties by id (Rails' order is undefined there), re-weighted to `t / total` in BigDecimal only
/// when the total is positive and not exactly 1 (thirds stored as 0.333333 are), then `.to_f` (order_setter.rb:406).
pub fn members(c: &Connection, bot: &Bot) -> Result<Vec<Member>, EngineError> {
    let mut s = c.prepare("SELECT asset_id, ticker_id, target_allocation FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1 ORDER BY target_allocation DESC, id")?;
    let rows = s.query_map([bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, target_from_sql(r.get_ref(2)?))))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut weights = vec![];
    for (asset_id, ticker_id, target) in rows { weights.push((asset_id, ticker_id, target?.unwrap_or_else(BigDec::zero))); }
    let total = weights.iter().fold(BigDec::zero(), |acc, (_, _, t)| &acc + t);
    let renormalise = total.is_positive() && total != BigDec::one();
    let mut out = vec![];
    for (asset_id, ticker_id, t) in weights {
        let weight = if renormalise { t.div(&total).expect("the total is positive").to_f() } else { t.to_f() };
        let ticker = model::ticker_by_id(c, bot.exchange_id, ticker_id)?
            .ok_or_else(|| EngineError::Data(format!("bot {}: member ticker {ticker_id} is not on this venue", bot.id)))?;
        out.push(Member { asset_id, ticker, weight });
    }
    Ok(out)
}

/// The pairs of every allocation, in settings order, whatever their availability: what a parity copy must script prices for.
pub fn member_pairs(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let mut out = vec![];
    for (asset_id, _) in bot.allocations().unwrap_or_default() {
        if let Some(t) = model::ticker_for_asset(c, bot, asset_id)? { out.push(t.ticker); }
    }
    Ok(out)
}

/// A member with the price its leg would be sent at: `reference` is what the venue answered (the ask for a market buy, the
/// last trade for a limit buy), `price` is amount::order_price of it.
#[derive(Clone, Debug)]
pub struct Priced { pub member: Member, pub reference: BigDec, pub price: BigDec }

/// One order of the split: its member's ticker, the reference price it was sized from, and its quote amount.
#[derive(Clone, Debug)]
pub struct Leg { pub ticker: Ticker, pub reference: BigDec, pub quote: BigDec }

/// `[a, b, c].min`: the first of equal minima, as Array#min keeps.
fn min3(a: BigDec, b: BigDec, c: BigDec) -> BigDec {
    let m = if b < a { b } else { a };
    if c < m { c } else { m }
}

/// get_orders_data, Steps 2-5 (composition/order_setter.rb:410-482), over members already priced in member order:
/// - current value = (holding + reserved) × price, at the order price (a limit price for a limit bot);
/// - target = (Σ current + x) × weight (BigDecimal × Float, the Float as Float#to_d);
/// - offset = max(0, target − current);
/// - in member order, skipping a zero offset: order = min(offset, x × (offset / Σ offset), what is left). An order that is
///   not positive is skipped; a positive one on a zero price is Err(price decimals) before anything is placed.
pub fn split(priced: &[Priced], holdings: &HashMap<i64, BigDec>, reserved: &HashMap<i64, BigDec>, x: &BigDec) -> Result<Vec<Leg>, i64> {
    let zero = BigDec::zero();
    let current: Vec<BigDec> = priced.iter().map(|p| {
        let held = holdings.get(&p.member.asset_id).unwrap_or(&zero) + reserved.get(&p.member.asset_id).unwrap_or(&zero);
        &held * &p.price
    }).collect();
    let portfolio = &current.iter().fold(BigDec::zero(), |acc, v| &acc + v) + x;
    let offsets: Vec<BigDec> = priced.iter().zip(&current).map(|(p, cur)| {
        let weight = BigDec::from_f64(p.member.weight).expect("a weight is finite");
        (&(&portfolio * &weight) - cur).max(BigDec::zero())
    }).collect();
    let total_offset = offsets.iter().fold(BigDec::zero(), |acc, o| &acc + o);
    let mut remaining = x.clone();
    let mut legs = vec![];
    for (p, offset) in priced.iter().zip(offsets) {
        if offset.is_zero() { continue; }
        // Rails divides first (a BigDecimal division with its own precision), then multiplies by x.
        let share = x * &offset.div(&total_offset).expect("a positive offset makes the total positive");
        let order = min3(offset, share, remaining.clone());
        if !order.is_positive() { continue; }
        // A limit price under the pair's precision floors to zero, and Rails' division would send a volume of Infinity.
        if p.price.is_zero() { return Err(p.member.ticker.price_decimals); }
        remaining = &remaining - &order;
        legs.push(Leg { ticker: p.member.ticker.clone(), reference: p.reference.clone(), quote: order });
    }
    Ok(legs)
}

/// Bot::Composition::Allocatable#update_bot_index_assets (allocatable.rb:108-132), in one transaction: the members not kept
/// are marked out of the index (`update_all`, so updated_at stays), then each kept member is saved (#save_member) with its
/// Float weight, in the order given. An index bot (index.rs) and a basket (refresh_composition) both write through here.
pub fn write_members(c: &Connection, bot_id: i64, members: &[(i64, i64, f64)], now: DateTime<Utc>) -> Result<(), EngineError> {
    model::locked(c, |tx| {
    let kept: Vec<i64> = members.iter().map(|(asset, _, _)| *asset).collect();
    tx.execute("UPDATE bot_index_assets SET in_index = 0, exited_at = ?1 WHERE bot_id = ?2 AND in_index = 1 AND asset_id NOT IN (SELECT value FROM json_each(?3))",
               params![format_time(now), bot_id, serde_json::to_string(&kept).map_err(|_| EngineError::Data("unreadable member ids".into()))?])?;
    for (asset_id, ticker_id, weight) in members { save_member(tx, bot_id, *asset_id, *ticker_id, *weight, now)?; }
    Ok(())
    })
}
