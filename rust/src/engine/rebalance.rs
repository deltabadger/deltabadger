//! Threshold rebalancing's valuation and ranking core, pure: Bot::Composition::Rebalancer#composition_targets (over the
//! figures `figures::live` marks), Bot::Rebalancer's ranking (rebalance_deviation, extreme_rebalance_entry,
//! tradeable_rebalance_entries, get_sell_orders_data's ranking) and Bot::Rebalanceable's drift and due check.
//! Arithmetic is Ruby's BigDecimal (`ruby::BigDec`); tests/rebalance_math.rs replays Rails' own answers.
use super::model::Ticker;
use super::EngineError;
use crate::figures::num::Num;
use crate::figures::walk::Metrics;
use crate::ruby::BigDec;
use serde_json::Value;
use std::cmp::Ordering;

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

/// Bot::Rebalanceable::DEFAULT_THRESHOLD.
pub const DEFAULT_THRESHOLD: &str = "0.05";

/// A current composition member (`bot_index_assets.in_index`, in id order), as composition_targets reads it.
#[derive(Clone, Debug)]
pub struct Member {
    pub asset_id: i64,
    /// `bia.asset.symbol`: the entry's symbol when the walk knows no key for the asset.
    pub symbol: String,
    /// target_allocation as `basket::target_from_sql` reads it; None is "unknown", never zero.
    pub target: Option<BigDec>,
    /// Allocatable#ticker_for_asset: the asset's ticker among the bot's available, trading-enabled ones.
    pub ticker: Option<Ticker>,
}

/// One composition_targets entry: the member's live holding and value and its normalised target.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub ticker: Option<Ticker>, pub symbol: String, pub asset_id: i64, pub amount: BigDec, pub value: BigDec, pub target: BigDec }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { Max, Min }

/// `x.to_d` of a figure.
fn dec(n: &Num) -> Result<BigDec, EngineError> { BigDec::parse(&n.to_d().map_err(data)?.to_s_f()).map_err(data) }

fn sum<'a>(xs: impl Iterator<Item = &'a BigDec>) -> BigDec { xs.fold(BigDec::zero(), |acc, x| &acc + x) }

/// Bot::Composition::Rebalancer#composition_targets over `data` (metrics_with_current_prices, fresh): None when the
/// figures are stale, nobody is a member, every member is locked, a member's holding went unpriced, or no survivor has a
/// positive target.
pub fn composition_targets(data: &Metrics, members: &[Member], locked: &[i64]) -> Result<Option<Vec<Entry>>, EngineError> {
    if data.prices_stale || members.is_empty() { return Ok(None); }
    let unlocked: Vec<&Member> = members.iter().filter(|m| !locked.contains(&m.asset_id)).collect();
    if unlocked.is_empty() || unpriced_holding(data, &unlocked) { return Ok(None); }
    let (mut entries, mut targets) = (vec![], vec![]);
    for m in unlocked {
        let key = data.key_for(m.asset_id);
        let (amount, value) = match key.and_then(|k| data.asset_values.iter().find(|(x, _)| x == k)) {
            Some((_, v)) => (dec(&v.amount)?, dec(&v.current_value)?),
            None => (BigDec::zero(), BigDec::zero()),
        };
        entries.push(Entry { ticker: m.ticker.clone(), symbol: key.map_or_else(|| m.symbol.clone(), str::to_string), asset_id: m.asset_id,
                             amount, value, target: BigDec::zero() });
        targets.push(m.target.clone());
    }
    let values: Vec<BigDec> = entries.iter().map(|e| e.value.clone()).collect();
    for (e, t) in entries.iter_mut().zip(normalise_targets(&values, &targets)) { e.target = t; }
    Ok(entries.iter().any(|e| e.target.is_positive()).then_some(entries))
}

/// #unpriced_holding?: a member with a ticker whose positive holding the live pass could not price.
pub fn unpriced_holding(data: &Metrics, members: &[&Member]) -> bool {
    members.iter().any(|m| {
        let Some(key) = data.key_for(m.asset_id) else { return false };
        data.asset_breakdown.iter().find(|(k, _)| k == key).is_some_and(|(_, b)| b.amount.is_positive())
            && !data.asset_values.iter().any(|(k, _)| k == key) && m.ticker.is_some()
    })
}

/// #normalise_targets: an unknown target is the member's current share of the survivors' value (0 when there is no value);
/// the recorded ones are scaled to fill what mass is left, when they sum to something positive.
pub fn normalise_targets(values: &[BigDec], targets: &[Option<BigDec>]) -> Vec<BigDec> {
    let total = sum(values.iter());
    let out: Vec<(bool, BigDec)> = values.iter().zip(targets).map(|(v, t)| match t {
        Some(t) => (true, t.clone()),
        None => (false, if total.is_positive() { v.div(&total).expect("the total is positive") } else { BigDec::zero() }),
    }).collect();
    let known_mass = (&BigDec::one() - &sum(out.iter().filter(|(k, _)| !k).map(|(_, t)| t))).max(BigDec::zero());
    let known_total = sum(out.iter().filter(|(k, _)| *k).map(|(_, t)| t));
    out.into_iter().map(|(known, t)| if known && known_total.is_positive() {
        (&t * &known_mass).div(&known_total).expect("the total is positive")
    } else { t }).collect()
}

/// #tradeable_rebalance_entries' test: a ticker, available and trading-enabled.
pub fn tradeable(e: &Entry) -> bool { e.ticker.as_ref().is_some_and(|t| t.available && t.trading_enabled) }

/// #rebalance_deviation: how far above its share of `portfolio` the entry sits, then `-target` (a smaller target reads as
/// further over on a tie).
pub fn deviation(e: &Entry, portfolio: &BigDec) -> (BigDec, BigDec) {
    (&e.value - &(portfolio * &e.target), &BigDec::zero() - &e.target)
}

/// #extreme_rebalance_entry: the index of the tradeable entry furthest over (Max) or under (Min) its target, Enumerable's
/// first on a tie, and the total it was measured against (zero when there are no entries, or nothing to sell from).
pub fn extreme_entry(entries: Option<&[Entry]>, direction: Direction) -> (Option<usize>, BigDec) {
    let Some(entries) = entries.filter(|e| !e.is_empty()) else { return (None, BigDec::zero()) };
    let total = sum(entries.iter().map(|e| &e.value));
    if direction == Direction::Max && !total.is_positive() { return (None, BigDec::zero()); }
    let wanted = if direction == Direction::Max { Ordering::Greater } else { Ordering::Less };
    let mut best: Option<(usize, (BigDec, BigDec))> = None;
    for (i, e) in entries.iter().enumerate().filter(|(_, e)| tradeable(e)) {
        let key = deviation(e, &total);
        if best.as_ref().is_none_or(|(_, b)| key.cmp(b) == wanted) { best = Some((i, key)); }
    }
    (best.map(|(i, _)| i), total)
}

/// get_sell_orders_data's ranking: the tradeable entries, most over their share of what is left after `withdrawal` first,
/// then the smaller target, then composition order. Indices into `entries`.
pub fn sell_ranking(entries: &[Entry], withdrawal: &BigDec) -> Vec<usize> {
    let portfolio = (&sum(entries.iter().map(|e| &e.value)) - withdrawal).max(BigDec::zero());
    let mut ranked: Vec<(usize, (BigDec, BigDec))> = entries.iter().enumerate().filter(|(_, e)| tradeable(e)).map(|(i, e)| (i, deviation(e, &portfolio))).collect();
    ranked.sort_by(|(i, a), (j, b)| b.cmp(a).then(i.cmp(j)));
    ranked.into_iter().map(|(i, _)| i).collect()
}

/// Bot::Rebalanceable#rebalance_drift: the largest |value / total − target|; None with no entries or no value.
pub fn drift(entries: Option<&[Entry]>) -> Option<BigDec> {
    let entries = entries.filter(|e| !e.is_empty())?;
    let total = sum(entries.iter().map(|e| &e.value));
    if !total.is_positive() { return None; }
    entries.iter().map(|e| {
        let d = &e.value.div(&total).expect("the total is positive") - &e.target;
        if d < BigDec::zero() { &BigDec::zero() - &d } else { d }
    }).max()
}

/// #rebalance_threshold: `presence&.to_d || DEFAULT_THRESHOLD`. A value Ruby could not `to_d` (a boolean true, an array)
/// is refused rather than guessed.
pub fn threshold(settings: &Value) -> Result<BigDec, EngineError> {
    match settings.get("rebalance_threshold") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(BigDec::parse(DEFAULT_THRESHOLD).expect("a constant")),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(BigDec::parse(DEFAULT_THRESHOLD).expect("a constant")),
        Some(v) => super::schedule::to_d(v).ok_or_else(|| EngineError::Data(format!("rebalance_threshold {v} is not a number"))),
    }
}

/// Bot::Rebalanceable#rebalance_due?: on, no swap pending, buying, not `blocked` (a liquidation or redeploy in flight),
/// and the drift strictly above the threshold.
pub fn due(settings: &Value, transient: &Value, blocked: bool, drift: Option<&BigDec>) -> Result<bool, EngineError> {
    if !crate::web::bot::cast_boolean(settings.get("rebalance_enabled")) { return Ok(false); }
    if transient.get("rebalance_pending").and_then(Value::as_object).is_some_and(|p| !p.is_empty()) { return Ok(false); }
    if settings.get("direction").and_then(Value::as_str) == Some("selling") || blocked { return Ok(false); }
    let Some(drift) = drift else { return Ok(false) };
    Ok(*drift > threshold(settings)?)
}
