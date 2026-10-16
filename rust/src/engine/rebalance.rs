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
    if is_pending(transient) { return Ok(false); }
    if settings.get("direction").and_then(Value::as_str) == Some("selling") || blocked { return Ok(false); }
    let Some(drift) = drift else { return Ok(false) };
    Ok(*drift > threshold(settings)?)
}

/// The members composition_targets reads: `bot_index_assets.in_index` in id order with each asset's symbol and target, and
/// Allocatable#ticker_for_asset among `subject`'s tickers (the bot's available, trading-enabled ones; the last of two for
/// one asset, as index_by keeps it).
pub fn members(c: &rusqlite::Connection, bot: &super::model::Bot, subject: &crate::figures::db::Subject) -> Result<Vec<Member>, EngineError> {
    let mut s = c.prepare("SELECT bia.asset_id, a.symbol, bia.target_allocation FROM bot_index_assets bia LEFT JOIN assets a ON a.id = bia.asset_id \
                           WHERE bia.bot_id = ?1 AND bia.in_index = 1 ORDER BY bia.id")?;
    let rows = s.query_map([bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, super::basket::target_from_sql(r.get_ref(2)?))))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for (asset_id, symbol, target) in rows {
        let ticker = match subject.tickers.iter().rev().find(|t| t.base_asset_id == asset_id) {
            Some(t) => super::model::ticker_by_id(c, bot.exchange_id, t.id)?,
            None => None,
        };
        out.push(Member { asset_id, symbol: symbol.unwrap_or_default(), target: target?, ticker });
    }
    Ok(out)
}

/// Exchange#get_tickers_prices' answer, read before the figures are marked (they ask synchronously).
pub struct Prefetched(pub crate::figures::market::Fetch<Vec<(String, crate::figures::market::Member<crate::figures::dec::Dec>)>>);
impl crate::figures::market::MarketData for Prefetched {
    fn prices(&self, _: &crate::figures::market::Venue, _: &[String]) -> crate::figures::market::Fetch<Vec<(String, crate::figures::market::Member<crate::figures::dec::Dec>)>> { self.0.clone() }
    fn candles(&self, _: &crate::figures::market::Venue, _: &crate::figures::db::Ticker, _: crate::figures::at::At, _: i64, _: bool) -> crate::figures::market::Fetch<Vec<(crate::figures::at::At, crate::figures::dec::Dec)>> { Err(unasked()) }
    fn exchange_rates(&self) -> crate::figures::market::Fetch<Vec<(String, crate::figures::market::Member<crate::figures::num::Num>)>> { Err(unasked()) }
    fn coin_price(&self, _: &str, _: &str) -> crate::figures::market::Fetch<crate::figures::market::Quoted> { Err(unasked()) }
}
fn unasked() -> crate::figures::market::Failure { crate::figures::market::Failure::Failed("not read for a trade".into()) }

/// Bot::Rebalanceable::PENDING_KEY and its phases.
pub const PENDING_KEY: &str = "rebalance_pending";
pub const PHASE_SELLING: &str = "selling";
pub const PHASE_BUYING: &str = "buying";
pub const PHASE_AMBIGUOUS: &str = "ambiguous";
/// Bot::Rebalanceable::BELOW_MINIMUM_KEY.
pub const BELOW_MINIMUM_KEY: &str = "rebalance_below_minimum_at";

/// transient_data.rebalance_pending as Bot::Rebalanceable#set_rebalance_pending! writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending { pub phase: String, pub sell_transaction_id: Option<i64>, pub buy_transaction_id: Option<i64>, pub remaining_quote_amount: Option<BigDec>, pub buy_attempted: bool }

impl Pending {
    /// The payload, in Rails' shape: the remaining quote as `to_d.to_s('F')` (a String, never a number), buy_attempted a bool.
    pub fn to_json(&self) -> Value {
        serde_json::json!({ "phase": self.phase, "sell_transaction_id": self.sell_transaction_id, "buy_transaction_id": self.buy_transaction_id,
                            "remaining_quote_amount": self.remaining_quote_amount.as_ref().map(BigDec::to_s_f), "buy_attempted": self.buy_attempted })
    }
}

/// Bot::Rebalanceable#rebalance_pending?: a Hash with anything in it.
pub fn is_pending(transient: &Value) -> bool { transient.get(PENDING_KEY).and_then(Value::as_object).is_some_and(|p| !p.is_empty()) }

/// Bot::Rebalanceable#rebalance_pending, read as Rails reads it: an id as the integer (or its digits) `find_by(id:)` takes,
/// the remaining quote by `to_d`, buy_attempted by Ruby truthiness. None when nothing is pending; a value Rails could not
/// use is refused, never guessed.
pub fn pending(transient: &Value) -> Result<Option<Pending>, EngineError> {
    if !is_pending(transient) { return Ok(None); }
    let p = &transient[PENDING_KEY];
    let bad = |k: &str| EngineError::Data(format!("rebalance_pending.{k} {} is not readable", p[k]));
    let id = |k: &str| match &p[k] {
        Value::Null => Ok(None),
        Value::Number(n) => n.as_i64().map(Some).ok_or_else(|| bad(k)),
        Value::String(s) => s.parse().map(Some).map_err(|_| bad(k)),
        _ => Err(bad(k)),
    };
    let remaining = match &p["remaining_quote_amount"] { Value::Null => None, v => Some(super::schedule::to_d(v).ok_or_else(|| bad("remaining_quote_amount"))?) };
    Ok(Some(Pending { phase: p["phase"].as_str().unwrap_or_default().to_string(), sell_transaction_id: id("sell_transaction_id")?,
                      buy_transaction_id: id("buy_transaction_id")?, remaining_quote_amount: remaining,
                      buy_attempted: !matches!(p["buy_attempted"], Value::Null | Value::Bool(false)) }))
}

/// Bot::Rebalanceable#set_rebalance_pending! (merge_transient_data!: updated_at stays).
pub fn set_pending(c: &rusqlite::Connection, bot_id: i64, p: &Pending) -> Result<(), EngineError> {
    super::model::merge_transient_compact(c, bot_id, &[(PENDING_KEY, p.to_json())])
}

/// Bot::Rebalanceable#clear_rebalance_pending!: the key is removed.
pub fn clear_pending(c: &rusqlite::Connection, bot_id: i64) -> Result<(), EngineError> {
    super::model::merge_transient_compact(c, bot_id, &[(PENDING_KEY, Value::Null)])
}

/// Bot::Rebalanceable#rebalance_below_minimum?: the flag holds something that is not blank.
pub fn below_minimum(transient: &Value) -> bool {
    match transient.get(BELOW_MINIMUM_KEY) { None | Some(Value::Null | Value::Bool(false)) => false, Some(Value::String(s)) => !s.trim().is_empty(),
                                            Some(Value::Array(a)) => !a.is_empty(), Some(Value::Object(o)) => !o.is_empty(), Some(_) => true }
}

/// #flag_rebalance_below_minimum!: stamped once (`Time.current.iso8601`), left alone while it stands.
pub fn flag_below_minimum(c: &rusqlite::Connection, bot_id: i64, transient: &Value, now: chrono::DateTime<chrono::Utc>) -> Result<(), EngineError> {
    if below_minimum(transient) { return Ok(()); }
    super::model::merge_transient_compact(c, bot_id, &[(BELOW_MINIMUM_KEY, Value::String(now.format("%Y-%m-%dT%H:%M:%SZ").to_string()))])
}

/// #clear_rebalance_below_minimum!: only when it stands.
pub fn clear_below_minimum(c: &rusqlite::Connection, bot_id: i64, transient: &Value) -> Result<(), EngineError> {
    if !below_minimum(transient) { return Ok(()); }
    super::model::merge_transient_compact(c, bot_id, &[(BELOW_MINIMUM_KEY, Value::Null)])
}
