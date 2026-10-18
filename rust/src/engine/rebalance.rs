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

/// #rebalance_sell_order_data's choice: the most overweight tradeable entry and its excess over its share of the total,
/// when that is positive.
pub fn sell_target(entries: Option<&[Entry]>) -> Option<(usize, BigDec)> {
    let (Some(i), total) = extreme_entry(entries, Direction::Max) else { return None };
    let e = &entries?[i];
    let excess = &e.value - &(&total * &e.target);
    excess.is_positive().then_some((i, excess))
}

/// #rebalance_buy_order_data's choice: the most underweight tradeable entry and its shortfall against the total the owed cash
/// will make (`target × (total + owed) − value`), when that is positive.
pub fn buy_target(entries: Option<&[Entry]>, owed: &BigDec) -> Option<(usize, BigDec)> {
    let (Some(i), total) = extreme_entry(entries, Direction::Min) else { return None };
    let e = &entries?[i];
    let shortfall = &(&e.target * &(&total + owed)) - &e.value;
    shortfall.is_positive().then_some((i, shortfall))
}

/// `[a, b, …].min`: the first of the smallest.
pub fn least(xs: impl IntoIterator<Item = BigDec>) -> Option<BigDec> { xs.into_iter().reduce(|a, b| if b < a { b } else { a }) }

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

// ---- valuation for a trade (6b T4/T6) --------------------------------------------------------------------------------

/// A valuation that did not happen: the bulk price read failed in transport (Rails raises it), or the figures raised.
pub enum ValueFail { Transient(String), Raised(String) }

/// metrics(force: true), metrics_with_current_prices(force: true) and composition_targets(locked: []) over them: the walk,
/// and the entries (None when the figures are stale, a member went unpriced, or a figure this build cannot hold). The venue
/// is asked only past the walk's own early returns.
pub async fn value<V: crate::venue::Venue>(c: &rusqlite::Connection, venue: &V, bot: &super::model::Bot, now: chrono::DateTime<chrono::Utc>)
    -> Result<Result<(Metrics, Option<Vec<Entry>>), ValueFail>, EngineError> {
    use crate::figures::{at::At, db::Subject, live, market::Failure, walk, FiguresError};
    use crate::venue::VenueError;
    let fig = |e: FiguresError| EngineError::Data(format!("{e:?}"));
    let at = At::from_utc(now).ok_or_else(|| EngineError::Data("the valuation's time is out of range".into()))?;
    let subject = Subject::load(c, bot.id).map_err(fig)?;
    let walked = walk::metrics(c, &subject, at).map_err(fig)?;
    let symbols: Vec<String> = subject.tickers.iter().map(|t| t.ticker.clone()).collect();
    let prices = if symbols.is_empty() || !live::needs_venue_price(c, &subject, &walked, at).map_err(fig)? { Ok(vec![]) } else {
        match venue.tickers_prices(&symbols).await {
            // `rescue ArgumentError; next`: a price that is no number is left out, not read as zero.
            Ok(raw) => Ok(raw.into_iter().map(|(code, v)| (code, crate::figures::dec::Dec::venue_number(&v)))
                .filter(|(_, p)| !matches!(p, Err(crate::figures::num::NumError::NotANumber))).collect()),
            Err(VenueError::Transient(m)) => return Ok(Err(ValueFail::Transient(m))),
            Err(VenueError::Rejected(e) | VenueError::Unauthorized(e)) => Err(Failure::Failed(crate::ruby::to_sentence(&e))),
            Err(VenueError::Ambiguous(m)) => Err(Failure::Failed(m)),
        }
    };
    let data = match live::live(c, &subject, &walked, &Prefetched(prices), at) {
        Ok(data) => Some(data),
        Err(FiguresError::Raised(m)) => return Ok(Err(ValueFail::Raised(m))),
        // A figure Rails computes and this build cannot (a price beyond dec's limits): no valuation, never a guessed one.
        Err(FiguresError::NotComputed(_)) => None,
        Err(e) => return Err(fig(e)),
    };
    let members = members(c, bot, &subject)?;
    let entries = match &data { Some(data) => composition_targets(data, &members, &[])?, None => None };
    Ok(Ok((walked, entries)))
}

// ---- the state machine (Bot::Rebalancer, Bot::RebalanceJob) -------------------------------------------------------------

use super::amount::{OrderPlan, TxKind};
use super::model::{Bot, Level};
use super::tick::TickContext;
use super::Clock;
use crate::enums::BotStatus;
use crate::venue::{Attributed, Side, Venue};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

/// A Rust-only key beside an ambiguous halt this engine wrote: eligibility admits such a bot (idle until the user resolves
/// it after a handback); an ambiguous phase Rails wrote stays refused.
pub const RUST_HALT_KEY: &str = "rust_rebalance_halted";

/// What one evaluation did: Rails' Result::Success(skipped:), Success(transaction_id:), or Failure (a raise included).
#[derive(Debug, PartialEq)]
pub enum Outcome { Skipped(&'static str), Placed(i64), Failed(String) }

/// What became of a REBALANCE intent, wherever it was settled (send, recovery, the operator, the handback). Applied in the
/// same transaction that writes the row and removes the intent, so the pending state never disagrees with the rows.
pub enum Settlement { Accepted(i64), NotPlaced, Halt(String) }

/// Bot::Rebalancer#place_rebalance_order's outcomes after the venue answered (or provably was never reached):
/// - Accepted: clear_rebalance_below_minimum!, carry_pending_forward, `rebalance_<side>_placed`.
/// - NotPlaced (the TransientNetworkError rescue, and a pre-trade rejection whose failed row the caller wrote): a sell owes
///   nothing (the state goes); a buy keeps what it owes, its attempt mark dropped.
/// - Halt: halt_ambiguous!.
///
/// A swap halted meanwhile stays halted; an accepted order's id is still recorded in it, for the user's resolution.
pub(crate) fn settle(c: &Connection, bot_id: i64, plan: &OrderPlan, settlement: Settlement, now: chrono::DateTime<chrono::Utc>) -> Result<(), EngineError> {
    let bot = super::model::load_bot(c, bot_id)?;
    let current = pending(&bot.transient)?;
    let sell = plan.side == Side::Sell;
    match settlement {
        Settlement::Accepted(id) => {
            let next = match current {
                Some(mut p) if p.phase == PHASE_AMBIGUOUS => { if sell { p.sell_transaction_id = Some(id) } else { p.buy_transaction_id = Some(id) }; p }
                _ if sell => Pending { phase: PHASE_SELLING.into(), sell_transaction_id: Some(id), buy_transaction_id: None, remaining_quote_amount: None, buy_attempted: false },
                p => Pending { phase: PHASE_BUYING.into(), sell_transaction_id: p.as_ref().and_then(|p| p.sell_transaction_id), buy_transaction_id: Some(id),
                               remaining_quote_amount: p.and_then(|p| p.remaining_quote_amount), buy_attempted: true },
            };
            clear_below_minimum(c, bot_id, &bot.transient)?;
            set_pending(c, bot_id, &next)?;
            super::model::log_activity(c, bot_id, &format!("rebalance_{}_placed", plan.side.as_str()), Level::Info, plan.log_details(), now)
        }
        Settlement::NotPlaced => match current {
            None => Ok(()),
            Some(p) if p.phase == PHASE_AMBIGUOUS => Ok(()),
            Some(_) if sell => clear_pending(c, bot_id),
            Some(p) => set_pending(c, bot_id, &Pending { buy_transaction_id: None, buy_attempted: false, ..p }),
        },
        Settlement::Halt(reason) => halt(c, bot_id, &reason, now),
    }
}

/// #halt_ambiguous!: the phase becomes ambiguous, the ids, the owed remainder and the attempt mark kept; one error line.
pub(crate) fn halt(c: &Connection, bot_id: i64, reason: &str, now: chrono::DateTime<chrono::Utc>) -> Result<(), EngineError> {
    super::model::locked(c, |c| {
        let bot = super::model::load_bot(c, bot_id)?;
        let p = pending(&bot.transient)?;
        let halted = Pending { phase: PHASE_AMBIGUOUS.into(), sell_transaction_id: p.as_ref().and_then(|p| p.sell_transaction_id),
                               buy_transaction_id: p.as_ref().and_then(|p| p.buy_transaction_id),
                               remaining_quote_amount: p.as_ref().and_then(|p| p.remaining_quote_amount.clone()), buy_attempted: p.is_some_and(|p| p.buy_attempted) };
        super::model::merge_transient_compact(c, bot_id, &[(PENDING_KEY, halted.to_json()), (RUST_HALT_KEY, Value::Bool(true))])?;
        super::model::log_activity(c, bot_id, "rebalance_ambiguous", Level::Error, json!({ "reason": reason }), now)
    })
}

/// Bot::EvaluateRebalancersJob#candidates for one bot (run::rebalance_candidates' query): a basket or index bot, not deleted
/// or archived, with the rule on or a swap pending.
pub fn candidate(bot: &Bot) -> bool {
    matches!(bot.bot_type.as_str(), "Bots::DcaIndex" | "Bots::DcaMultiAsset") && !matches!(bot.status, BotStatus::Deleted | BotStatus::Archived)
        && (crate::web::bot::cast_boolean(bot.settings.get("rebalance_enabled")) || bot.transient.get(PENDING_KEY).is_some_and(|v| !v.is_null()))
}

/// Bot::RebalanceJob#perform for one bot: #resumable?, then the market, then Bot::Rebalancer#rebalance!. First, an order
/// this bot left unresolved is looked up by its client order id (placement::recover_since); while it stays unknown nothing
/// new is decided, and a REBALANCE one still unknown a full absence window after it was sent halts the swap as Rails
/// halts an unknown placement outright (the sanctioned divergence: Rails halts at once, the engine asks the venue first).
pub async fn run<V: Venue>(c: &Connection, venue: &V, bot_id: i64, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    let handle = crate::venue::Handle::new(venue, cx.credential_version.clone());
    let venue = &handle;
    let bot = super::model::load_bot(c, bot_id)?;
    if matches!(bot.status, BotStatus::Deleted | BotStatus::Archived) { return Ok(Outcome::Skipped("not_resumable")); }
    if !crate::web::bot::cast_boolean(bot.settings.get("rebalance_enabled")) && !is_pending(&bot.transient) { return Ok(Outcome::Skipped("not_resumable")); }
    if let Some(raw) = bot.rust_placement() {
        if let super::placement::Recovery::Pending = super::placement::recover_since(c, venue, &bot, clock, cx.process_start).await? {
            let rebalance = raw["transaction_type"].as_str() == Some(TxKind::Rebalance.as_str());
            let at = crate::codec::optional_time(raw.get("at")).map_err(|_| EngineError::Data("rust_placement.at is unreadable".into()))?;
            let margin = chrono::Duration::seconds(venue.rules().absence_margin_secs.max(super::placement::DEADLINE_SECONDS + super::placement::ABSENCE_AFTER_SECONDS));
            if rebalance && at.is_some_and(|at| clock.now() >= at + margin) && pending(&bot.transient)?.is_none_or(|p| p.phase != PHASE_AMBIGUOUS) {
                let side = raw["side"].as_str().unwrap_or("buy");
                halt(c, bot_id, &format!("{side} placement outcome unknown: the venue has not answered for order {}", raw["cl_ord_id"].as_str().unwrap_or_default()), clock.now())?;
                return Ok(Outcome::Skipped("ambiguous"));
            }
            return Ok(Outcome::Skipped("awaiting_reconciliation"));
        }
    }
    let bot = super::model::load_bot(c, bot_id)?;
    // The pass's one read of what it decides from, before its first await (the clock below).
    let claim = Claim { composition: super::placement::composition_snapshot(c, &bot)?, bot };
    let bot = &claim.bot;
    // The placement barrier: nothing is sized while a reversal's cancels are not recorded (begin refuses it anyway).
    if super::reversal::barred(bot) { return Ok(Outcome::Skipped("reversal_pending")); }
    // As a tick: nothing is decided from reference data past its bound.
    if let Some(stale) = super::staleness::stale(c, bot, clock.now())? {
        super::log(&format!("[engine] bot {bot_id}: rebalance skipped: {}", stale.message));
        return Ok(Outcome::Skipped("stale"));
    }
    // #market_open?: all crypto is open; Exchanges::Alpaca reads the clock. A clock it cannot read is skipped here (Rails
    // reads nil as open): the next evaluation asks again, and an owed buy waits at most one cadence.
    if venue.rules().market_hours && !super::model::all_crypto(c, bot)? {
        let answer = venue.clock_result().await.value;
        if !matches!(super::clock::gate(answer, &super::model::exchange_name(c, bot)?, clock.now()), super::clock::Gate::Open) {
            super::log(&format!("[engine] bot {bot_id}: rebalance skipped: market closed"));
            return Ok(Outcome::Skipped("market_closed"));
        }
    }
    rebalance(c, venue, claim, clock, cx).await
}

/// What one pass decides from, read once before its first await: the bot as loaded (settings and allocations, status,
/// restatement_generation, the pending phase) and its composition (placement::composition_snapshot: type, exchange,
/// settings, member rows). Every write the pass makes compares the stored rows against it under its own write lock: the
/// composition refresh, the split gate and the intent's fence (begin_rebalance). Nothing is re-read to be compared
/// against, so an edit landing while the venue is asked (the clock, positions, prices) refuses the pass; the next
/// evaluation runs it again from the edit, as Rails would. The pass's own composition refresh moves the claim forward
/// with what it wrote, read under that write's lock.
struct Claim { bot: Bot, composition: Value }

/// Bot::Rebalancer#rebalance!.
async fn rebalance<V: Venue + Attributed>(c: &Connection, venue: &V, mut claim: Claim, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    if pending(&claim.bot.transient)?.is_some_and(|p| p.phase == PHASE_AMBIGUOUS) { return Ok(Outcome::Skipped("ambiguous")); }
    if !before_rebalance(c, venue, &mut claim, clock, cx).await? { return Ok(Outcome::Skipped("changed")); }
    let claim = &claim;
    let bot = &claim.bot;
    if let Some(p) = pending(&bot.transient)? {
        return match p.phase.as_str() {
            PHASE_SELLING => resume_selling(c, venue, claim, p, clock, cx).await,
            PHASE_BUYING => resume_buying(c, venue, claim, p, clock, cx).await,
            _ => { halt(c, bot.id, "unrecognised pending phase", clock.now())?; Ok(Outcome::Skipped("ambiguous")) }
        };
    }
    // Bots::DcaMultiAsset#rebalance_targets: sliders off 100 % give no targets, so no drift (a pending swap, resumed above,
    // still has its targets).
    if !super::eligibility::allocations_balanced(bot) { return Ok(Outcome::Skipped("not_due")); }
    let Some(reconciled) = split_gate(c, venue, claim, clock).await? else { return Ok(Outcome::Skipped("split_hold")) };
    // #rebalance_due? reads the drift from rebalance_targets (cached in Rails; always fresh here).
    let entries = match value(c, venue, bot, clock.now()).await? {
        Ok((_, entries)) => entries,
        Err(ValueFail::Transient(m) | ValueFail::Raised(m)) => return Ok(Outcome::Failed(m)),
    };
    if !due(&bot.settings, &bot.transient, false, drift(entries.as_deref()).as_ref())? { return Ok(Outcome::Skipped("not_due")); }
    start(c, venue, claim, entries.unwrap_or_default(), &reconciled, clock, cx).await
}

/// tick::execute's split gate, before either leg is sized: the snapshot first, then the account's restated holdings against
/// the venue's positions (splits::refusal), and the snapshot unchanged across that read. The snapshot is carried to the
/// intent, whose fence refuses a split row that changed while the leg was sized. A generation other than the pass's claim
/// holds too. None: hold, nothing is sized.
async fn split_gate<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &Claim, clock: &dyn Clock) -> Result<Option<super::splits::Snapshot>, EngineError> {
    let bot = &claim.bot;
    let reconciled = super::splits::snapshot(c, bot)?;
    let reason = if reconciled.generation != bot.restatement_generation { "split generation changed since the pass began".into() } else {
        match super::splits::refusal(c, venue, bot, clock.now()).await? {
            Some(reason) => reason,
            None if super::splits::snapshot(c, bot)? != reconciled => "split generation or held assets changed during reconciliation".into(),
            None => return Ok(Some(reconciled)),
        }
    };
    super::log(&format!("[engine] bot {}: rebalance skipped: split reconciliation required: {reason}", bot.id));
    Ok(None)
}

/// Bot::Composition::Rebalancer#before_rebalance: the composition is refreshed (a stopped bot's DCA tick never does it), best
/// effort: a failure is logged and the stored composition stands. Written only while the stored composition is still the
/// claim's (false: it changed, the pass ends); the claim then moves to what was written.
async fn before_rebalance<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &mut Claim, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<bool, EngineError> {
    let bot = &claim.bot;
    let refreshed = if bot.bot_type == "Bots::DcaIndex" {
        match super::index::refresh_for_rebalance(c, venue, bot, &claim.composition, clock, cx.prices, &cx.credential_version).await {
            Ok(Ok(written)) => Ok(written),
            Ok(Err(super::index::Refusal::Failure(m) | super::index::Refusal::Transient(m))) => Err(m),
            Err(e @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(e),
            Err(e) => Err(format!("{e:?}")),
        }
    } else {
        let now = clock.now();
        let claimed = &claim.composition;
        // The bot is read again inside the write lock, and written only while it still is what the pass claimed.
        let written = super::model::locked(c, |tx| {
            let current = super::model::load_bot(tx, bot.id)?;
            if &super::placement::composition_snapshot(tx, &current)? != claimed { return Ok(None); }
            Ok(Some(match super::basket::refresh_composition(tx, &current, now)? {
                Ok(()) => Ok(super::placement::composition_snapshot(tx, &current)?),
                Err(m) => Err(m),
            }))
        });
        match written {
            Ok(None) => {
                super::log(&format!("[engine] bot {}: rebalance skipped: its settings or members changed since the pass began", bot.id));
                return Ok(false);
            }
            Ok(Some(r)) => r,
            Err(e @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(e),
            Err(e) => Err(format!("{e:?}")),
        }
    };
    match refreshed {
        Ok(written) => claim.composition = written,
        Err(m) => super::log(&format!("[engine] rebalance composition refresh failed bot={}: {m}", bot.id)),
    }
    Ok(true)
}

/// #start_rebalance!: sell the most overweight member down to its target (never more than is free on the venue).
async fn start<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &Claim, entries: Vec<Entry>, reconciled: &super::splits::Snapshot, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    let bot = &claim.bot;
    // #rebalance_sell_order_data; nil is a silent below_minimum.
    let Some((i, excess)) = sell_target(Some(&entries)) else { return Ok(Outcome::Skipped("below_minimum")) };
    let ticker = entries[i].ticker.clone().expect("a tradeable entry has a ticker");
    let Some(price) = super::tick::market_price_for(venue, bot, &ticker, Side::Sell, clock, cx).await? else { return Ok(Outcome::Skipped("below_minimum")) };
    let free = match super::tick::live_free_balance(c, venue, bot, &ticker, ticker.base_asset_id).await? { Ok(f) => f, Err(e) => return Ok(Outcome::Failed(format!("{e:?}"))) };
    let amount = least([excess.div(&price).expect("the price is positive"), free]).expect("two values");
    if !amount.is_positive() { return Ok(Outcome::Skipped("below_minimum")); }
    if waiting_buy_blocks_sell(c, venue, bot, &ticker, clock.now()).await? { return Ok(Outcome::Skipped("open_buy")); }
    let quote = &amount * &price;
    let (plan, below) = super::amount::size_rebalance(&ticker, Side::Sell, &price, &amount, &quote, venue.rules().minimum_logic)?;
    if below {
        // #abandon_below_minimum!(selling): silent, but the widget's flag says why a breached band does not trade.
        flag_below_minimum(c, bot.id, &bot.transient, clock.now())?;
        return Ok(Outcome::Skipped("below_minimum"));
    }
    let intent_state = Pending { phase: PHASE_SELLING.into(), sell_transaction_id: None, buy_transaction_id: None, remaining_quote_amount: None, buy_attempted: false };
    place(c, venue, claim, &plan, &intent_state, reconciled, clock, cx).await
}

/// #waiting_buy_blocks_sell?: a waiting buy of this asset on any of the account's bots, by id or by any name a venue lists it
/// under. This bot's own rows are swept first (a stopped bot has no tick to do it); a failed sweep leaves the guard on.
async fn waiting_buy_blocks_sell<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, ticker: &super::model::Ticker, now: chrono::DateTime<chrono::Utc>) -> Result<bool, EngineError> {
    if !account_waiting_buys(c, bot, ticker)? { return Ok(false); }
    if let Err(e) = super::polling::sweep(c, venue, bot, now).await { super::log(&format!("[engine] rebalance waiting-buy refresh failed bot={}: {e:?}", bot.id)); }
    account_waiting_buys(c, bot, ticker)
}

fn account_waiting_buys(c: &Connection, bot: &Bot, ticker: &super::model::Ticker) -> Result<bool, EngineError> {
    let mut s = c.prepare("SELECT base FROM tickers WHERE base_asset_id = ?1")?;
    let mut names: Vec<String> = s.query_map([ticker.base_asset_id], |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?
        .into_iter().flatten().map(|b| crate::figures::db::base_spelling(&b).to_string()).collect();
    let symbol: Option<String> = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [ticker.base_asset_id], |r| r.get(0)).optional()?.flatten();
    names.extend(symbol);
    names.retain(|n| !n.trim().is_empty());
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM transactions WHERE status = 0 AND external_status IN (0, 1) AND side = 0 \
                    AND bot_id IN (SELECT id FROM bots WHERE user_id = ?1 AND status <> ?2) \
                    AND (base_asset_id = ?3 OR base IN (SELECT value FROM json_each(?4))))",
                   params![bot.user_id, BotStatus::Deleted as i64, ticker.base_asset_id, serde_json::to_string(&names).expect("strings")], |r| r.get(0))?)
}

/// A pending row as #pending_transaction finds it (any of the bot's rows), with what #terminal? and #realized_proceeds read.
struct Row { external_status: Option<i64>, price: Option<BigDec>, amount_exec: Option<BigDec>, quote_amount_exec: Option<BigDec> }

fn pending_row(c: &Connection, bot_id: i64, id: Option<i64>) -> Result<Option<Row>, EngineError> {
    let Some(id) = id else { return Ok(None) };
    let dec = |v: rusqlite::types::ValueRef<'_>| crate::ruby::from_sql(v).map_err(|e| EngineError::Data(format!("{e:?}")));
    let mut s = c.prepare("SELECT external_status, price, amount_exec, quote_amount_exec FROM transactions WHERE bot_id = ?1 AND id = ?2")?;
    let mut rows = s.query(params![bot_id, id])?;
    let Some(r) = rows.next()? else { return Ok(None) };
    Ok(Some(Row { external_status: r.get(0)?, price: dec(r.get_ref(1)?)?, amount_exec: dec(r.get_ref(2)?)?, quote_amount_exec: dec(r.get_ref(3)?)? }))
}

impl Row {
    /// #terminal?: closed, cancelled or abandoned.
    fn terminal(&self) -> bool { matches!(self.external_status, Some(2..=4)) }
    fn abandoned(&self) -> bool { self.external_status == Some(4) }
    /// #realized_proceeds: the quote figure, else 0 for nothing executed, else amount × price; None when neither is known.
    fn realized(&self) -> Option<BigDec> {
        if let Some(q) = &self.quote_amount_exec { return Some(q.clone()); }
        if self.amount_exec.as_ref().is_some_and(BigDec::is_zero) { return Some(BigDec::zero()); }
        Some(self.amount_exec.as_ref()? * self.price.as_ref()?)
    }
}

/// A swap whose pending leg is closed with its figures still unknown (#realized_proceeds is nil): a sell's proceeds or a buy's
/// fill. The holdings walk cannot value it yet; Rails re-reads that row first on each fire (#resume_selling!, #resume_buying!
/// refresh before anything else), and so does the engine.
pub(crate) fn awaiting_figures(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    let Some(p) = pending(&bot.transient)? else { return Ok(false) };
    let id = match p.phase.as_str() { PHASE_SELLING => p.sell_transaction_id, PHASE_BUYING => p.buy_transaction_id, _ => return Ok(false) };
    Ok(pending_row(c, bot.id, id)?.is_some_and(|row| row.terminal() && !row.abandoned() && row.realized().is_none()))
}

/// #refresh_pending_order!: Bot::FetchAndUpdateOrderJob for this one row whatever its state (a closed row whose figures are
/// still unknown is asked again); a failure is logged and the row read as it stands.
async fn refresh<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, id: i64, now: chrono::DateTime<chrono::Utc>) {
    if let Err(e) = super::polling::refresh(c, venue, bot.id, id, now).await {
        super::log(&format!("[engine] rebalance refresh failed bot={} transaction={id} error={e:?}", bot.id));
    }
}

/// #resume_selling!.
async fn resume_selling<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &Claim, p: Pending, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    let (bot, now) = (&claim.bot, clock.now());
    if pending_row(c, bot.id, p.sell_transaction_id)?.is_none() { halt(c, bot.id, "sell intent with no persisted order", now)?; return Ok(Outcome::Skipped("ambiguous")); }
    let id = p.sell_transaction_id.expect("found by it");
    refresh(c, venue, bot, id, now).await;
    let row = pending_row(c, bot.id, Some(id))?.ok_or_else(|| EngineError::Data(format!("transaction {id} vanished")))?;
    if !row.terminal() { return Ok(Outcome::Skipped("waiting_for_sell")); }
    if row.abandoned() { halt(c, bot.id, "sell abandoned", now)?; return Ok(Outcome::Skipped("ambiguous")); }
    // nil, not zero: a closed sell whose quote figure is still unknown waits for one it can spend.
    let Some(proceeds) = row.realized() else { return Ok(Outcome::Skipped("proceeds_unknown")) };
    if !proceeds.is_positive() { clear_pending(c, bot.id)?; return Ok(Outcome::Skipped("nothing_sold")); }
    set_pending(c, bot.id, &Pending { phase: PHASE_BUYING.into(), sell_transaction_id: Some(id), buy_transaction_id: None, remaining_quote_amount: Some(proceeds), buy_attempted: false })?;
    place_pending_buy(c, venue, claim, clock, cx).await
}

/// #resume_buying!.
async fn resume_buying<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &Claim, p: Pending, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    let (bot, now) = (&claim.bot, clock.now());
    if pending_row(c, bot.id, p.buy_transaction_id)?.is_none() {
        if p.buy_transaction_id.is_none() {
            // Attempted with no order id in the state: the send may have landed. Only a handoff that never left is replayed.
            if p.buy_attempted { halt(c, bot.id, "buy attempted with no persisted order", now)?; return Ok(Outcome::Skipped("ambiguous")); }
            return place_pending_buy(c, venue, claim, clock, cx).await;
        }
        halt(c, bot.id, "buy intent with no persisted order", now)?;
        return Ok(Outcome::Skipped("ambiguous"));
    }
    let id = p.buy_transaction_id.expect("found by it");
    refresh(c, venue, bot, id, now).await;
    let row = pending_row(c, bot.id, Some(id))?.ok_or_else(|| EngineError::Data(format!("transaction {id} vanished")))?;
    if !row.terminal() { return Ok(Outcome::Skipped("waiting_for_buy")); }
    if row.abandoned() { halt(c, bot.id, "buy abandoned", now)?; return Ok(Outcome::Skipped("ambiguous")); }
    let Some(filled) = row.realized() else { return Ok(Outcome::Skipped("fill_unknown")) };
    let remaining = &p.remaining_quote_amount.unwrap_or_else(BigDec::zero) - &filled;
    if !remaining.is_positive() { clear_pending(c, bot.id)?; return Ok(Outcome::Skipped("completed")); }
    // A partly filled then cancelled buy still owes the remainder: only that is placed.
    set_pending(c, bot.id, &Pending { phase: PHASE_BUYING.into(), sell_transaction_id: p.sell_transaction_id, buy_transaction_id: None, remaining_quote_amount: Some(remaining), buy_attempted: false })?;
    place_pending_buy(c, venue, claim, clock, cx).await
}

/// #place_pending_buy! with #rebalance_buy_order_data: the owed proceeds into the most underweight member, capped by its
/// shortfall against the cash-inclusive total and by the settled quote on the venue.
async fn place_pending_buy<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &Claim, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    let (bot, bot_id) = (&claim.bot, claim.bot.id);
    // The phase is this pass's own write (resume_*); everything the buy is sized from is the claim's.
    let p = pending(&super::model::load_bot(c, bot_id)?.transient)?.ok_or_else(|| EngineError::Data(format!("bot {bot_id}: no pending rebalance to buy for")))?;
    let owed = p.remaining_quote_amount.clone().unwrap_or_else(BigDec::zero);
    if !owed.is_positive() { clear_pending(c, bot_id)?; return Ok(Outcome::Skipped("nothing_owed")); }
    let Some(reconciled) = split_gate(c, venue, claim, clock).await? else { return Ok(Outcome::Skipped("split_hold")) };
    let entries = match value(c, venue, bot, clock.now()).await? {
        Ok((_, entries)) => entries,
        Err(ValueFail::Transient(m) | ValueFail::Raised(m)) => return Ok(Outcome::Failed(m)),
    };
    let unavailable = Ok(Outcome::Skipped("buy_unavailable"));
    let Some((i, shortfall)) = buy_target(entries.as_deref(), &owed) else { return unavailable };
    let ticker = entries.as_ref().expect("an entry was found")[i].ticker.clone().expect("a tradeable entry has a ticker");
    let Some(price) = super::tick::market_price_for(venue, bot, &ticker, Side::Buy, clock, cx).await? else { return unavailable };
    let free = match super::tick::live_free_balance(c, venue, bot, &ticker, ticker.quote_asset_id).await? { Ok(f) => f, Err(e) => return Ok(Outcome::Failed(format!("{e:?}"))) };
    let spend = least([owed.clone(), shortfall, free]).expect("three values");
    if !spend.is_positive() { return unavailable; }
    let amount = spend.div(&price).expect("the price is positive");
    let (plan, below) = super::amount::size_rebalance(&ticker, Side::Buy, &price, &amount, &spend, venue.rules().minimum_logic)?;
    if below {
        // #abandon_below_minimum!(buying): the user's own cash left unconvertible, said once.
        let now = clock.now();
        super::model::locked(c, |tx| { super::model::log_activity(tx, bot_id, "rebalance_dust", Level::Info, plan.log_details(), now)?; clear_pending(tx, bot_id) })?;
        return Ok(Outcome::Skipped("below_minimum"));
    }
    // #mark_buy_attempted!, written with the intent.
    let intent_state = Pending { phase: PHASE_BUYING.into(), sell_transaction_id: p.sell_transaction_id, buy_transaction_id: None, remaining_quote_amount: Some(owed), buy_attempted: true };
    place(c, venue, claim, &plan, &intent_state, &reconciled, clock, cx).await
}

/// #place_rebalance_order: the intent and the pending phase in one transaction (placement::begin_rebalance), one send, and
/// the outcome settled with the row (placement::settle_rebalance through send_recorded). The fence compares the stored bot
/// against the pass's claim: its settings, allocations and members as read before the first await.
#[allow(clippy::too_many_arguments)]
async fn place<V: Venue + Attributed>(c: &Connection, venue: &V, claim: &Claim, plan: &OrderPlan, intent_state: &Pending, reconciled: &super::splits::Snapshot, clock: &dyn Clock, cx: &TickContext<'_>) -> Result<Outcome, EngineError> {
    use super::placement::{Begun, Sent};
    let bot = &claim.bot;
    let intent = match super::placement::begin_rebalance(c, bot, plan, &[&plan.ticker], &claim.composition, reconciled, &cx.credential_version, venue.placement_identity(), intent_state, clock)? {
        Begun::Intent(intent) => intent,
        Begun::Changed | Begun::Fenced => return Ok(Outcome::Skipped("changed")),
        Begun::LedgerStale(_) => return Ok(Outcome::Skipped("ledger_stale")),
        Begun::CredentialsChanged => return Ok(Outcome::Failed("credentials changed".into())),
        Begun::BelowMinimum(_) => return Ok(Outcome::Skipped("below_minimum")),
    };
    Ok(match super::placement::send_recorded(c, venue, bot, &intent, clock).await? {
        Sent::Accepted(txid) => Outcome::Placed(c.query_row("SELECT id FROM transactions WHERE exchange_id = ?1 AND external_id = ?2", params![bot.exchange_id, txid], |r| r.get(0))?),
        Sent::Rejected(errors) if super::placement::safe_rejection(&errors) => Outcome::Failed(crate::ruby::to_sentence(&errors)),
        Sent::Rejected(_) => Outcome::Skipped("ambiguous"),
        Sent::NotSent(m) => Outcome::Failed(m),
        Sent::Ambiguous(_) => Outcome::Skipped("awaiting_reconciliation"),
        Sent::Fenced => Outcome::Skipped("changed"),
    })
}
