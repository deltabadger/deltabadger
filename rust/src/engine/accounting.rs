//! R6: the single monetary accounting boundary for engine, settings, start and preview.
//! Fill reads go through figures::fill. Adapters select a window and preserve Ruby numeric
//! classes; spent, pending, compensated cap buckets and the exact-cost guard live only here.
use super::{model::Bot, amount::OrderPlan, EngineError};
use super::schedule::{checkpoints, effective, interval_count, Effective};
use crate::{codec::{self, format_time}, enums::BotStatus, ruby::{ruby_sum, BigDec, Num, round6_micros}};
/// Opaque normalized money: only accounting can read these fields. The normalizer
/// constructs values; adapters may pass them around but cannot sum or inspect money.
/// No Debug/serialization/amount accessors expose the private monetary fields.
pub struct Commitment { status: i64, value: BigDec, cap_kind: CapKind, zero_without_report: bool }
pub(crate) enum CapKind { Integer(i64), RealBits(u64), Decimal }
impl Commitment {
    pub(crate) fn normalized(status:i64,value:BigDec,cap_kind:CapKind,zero_without_report:bool)->Self {
        Self{status,value,cap_kind,zero_without_report}
    }
}
pub struct SoldCommitment { quantity: BigDec }
impl SoldCommitment {
    pub(crate) fn normalized(quantity:BigDec)->Self { Self{quantity} }
}
use crate::web::{bot::{self as web_bot, start, Bot as WebBot, Kind}, format::Num as WebNum, WebError};
use chrono::{DateTime, Datelike, Months, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
fn at(us: i64) -> Result<DateTime<Utc>, EngineError> { DateTime::from_timestamp_micros(us).ok_or_else(super::schedule::time_range_error) }
fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }
fn error(message: &str) -> WebError { WebError::Engine(EngineError::Data(message.into())) }
fn core(value: WebNum) -> Num { match value { WebNum::Int(n)=>Num::Int(n), WebNum::Float(n)=>Num::Float(n), WebNum::Dec(n)=>Num::Dec(n) } }
fn view(value: Num) -> WebNum { match value { Num::Int(n)=>WebNum::Int(n), Num::Float(n)=>WebNum::Float(n), Num::Dec(n)=>WebNum::Dec(n) } }

/// RULING-B2A-R8: reject inputs beyond the exact Integer/Float interchange range.
/// Validate the stored decimal spelling BEFORE any conversion, including unsigned JSON integers.
pub fn bounded_decimal(value: &BigDec) -> Result<(), EngineError> {
    let bound = BigDec::from_i64(9_007_199_254_740_992);
    if value > &bound || value < &(&BigDec::zero() - &bound) {
        return Err(EngineError::Data("accounting magnitude exceeds 2^53".into()));
    }
    Ok(())
}
/// Figures already hold exact decimals. Compare their magnitude directly; serializing every
/// fill here would spend the figures walk's operation budget without changing its value.
pub fn bounded_fill_amount(value: &crate::figures::dec::Dec) -> Result<(), EngineError> {
    let magnitude = if value.is_negative() { value.neg() } else { value.clone() };
    if magnitude > crate::figures::dec::Dec::from_i64(9_007_199_254_740_992) {
        return Err(EngineError::Data("accounting magnitude exceeds 2^53".into()));
    }
    Ok(())
}
fn bounded_num(value: &Num) -> Result<(), EngineError> {
    if let Num::Float(n) = value {
        if !n.is_finite() || n.abs() > 9_007_199_254_740_992.0 { return Err(EngineError::Data("accounting magnitude exceeds 2^53".into())); }
    }
    bounded_decimal(&value.to_dec().map_err(EngineError::Arithmetic)?)
}
pub fn setting_num(value: &Value) -> Result<Num, EngineError> {
    let Value::Number(n) = value else { return Err(data("accounting amount is not a number")); };
    bounded_decimal(&BigDec::parse(&n.to_string()).map_err(EngineError::Arithmetic)?)?;
    match n.as_i64() {
        Some(n) => Ok(Num::Int(n)),
        None => n.as_f64().filter(|n| n.is_finite()).map(Num::Float).ok_or_else(||data("accounting amount is not finite")),
    }
}
/// Accountable explicitly calls to_d for present carry; absent/blank carry remains Integer zero.
pub fn carry_num(value: Option<&Value>) -> Result<Num, EngineError> {
    let value = match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => return Ok(Num::Int(0)),
        Some(Value::String(s)) if s.trim().is_empty() => return Ok(Num::Int(0)),
        Some(Value::String(s)) => Num::Dec(BigDec::parse(s).map_err(EngineError::Arithmetic)?),
        Some(v) => Num::Dec(setting_num(v)?.to_dec().map_err(EngineError::Arithmetic)?),
    };
    bounded_num(&value)?;
    Ok(value)
}
pub fn validate_stored_amounts(settings: &Value, transient: &Value) -> Result<(), EngineError> {
    for key in ["quote_amount", "smart_interval_quote_amount", "quote_amount_limit", "base_amount_limit"] {
        if let Some(value) = settings.get(key).filter(|v| v.is_number()) { setting_num(value)?; }
    }
    carry_num(transient.get("missed_quote_amount"))?;
    Ok(())
}
fn validate_web_amounts(bot: &WebBot) -> Result<(), EngineError> {
    validate_stored_amounts(&Value::Object(bot.settings.clone()), &Value::Object(bot.transient.clone()))
}

/// Exact normalized spend; preserve Integer zero for unreported cancelled rows.
fn spent_in_window(rows: &[Commitment]) -> Result<Num, EngineError> {
    let mut total = BigDec::zero();
    let mut decimal = false;
    for row in rows {
        total = total.checked_add(&row.value).map_err(data)?;
        decimal |= !row.zero_without_report;
    }
    Ok(if decimal { Num::Dec(total) } else { Num::Int(0) })
}

/// The only pending arithmetic. Window/checkpoint adapters supply the count, never money totals.
fn pending_window(amount: Num, count: i64, carry: Num, rows: &[Commitment]) -> Result<Num, EngineError> {
    bounded_num(&amount)?; bounded_num(&carry)?;
    let owed = match amount {
        Num::Int(n) => Num::Int(n.checked_mul(count).ok_or(EngineError::Arithmetic(codec::CodecError::IntegerOverflow("pending multiply")))?),
        Num::Float(n) => { let n = n * count as f64; if !n.is_finite() { return Err(data("carry overflow")); } Num::Float(n) },
        Num::Dec(n) => Num::Dec(n.checked_mul(&BigDec::from_i64(count)).map_err(EngineError::Arithmetic)?),
    };
    let pending = owed.add(&carry).map_err(EngineError::Arithmetic)?.sub(&spent_in_window(rows)?).map_err(EngineError::Arithmetic)?;
    Ok(if pending.is_negative() { Num::Int(0) } else { pending })
}

/// Ruby's min keeps the first operand and its class on ties.
fn minimum(a: Num, b: Num) -> Result<Num, EngineError> {
    if b.sub(&a).map_err(EngineError::Arithmetic)?.is_negative() { Ok(b) } else { Ok(a) }
}
/// Bot::Accountable#pending_quote_amount as Rails has it since its fill-credit fix (#448): what the bot owes since the
/// window opened, minus every submitted REGULAR buy in the window, read in ONE statement, so a row that changes state
/// between reads is counted once. Settled: its accepted normalized value. Open or unknown: its submitted quote, or amount × price
/// (#invested_quote). Cancelled or abandoned: normalized execution before it stopped; no execution credits zero.
/// Nothing in polling moves missed_quote_amount any more. The window is max(started_at, settings_changed_at), inclusive
/// for every row.
pub fn pending_quote_amount(c: &Connection, bot: &Bot, now_us: i64) -> Result<BigDec, EngineError> {
    validate_stored_amounts(&bot.settings, &bot.transient)?;
    if bot.status == BotStatus::Deleted { return Ok(BigDec::zero()); }
    let Some(started) = bot.started_at_us()? else { return Ok(BigDec::zero()) };
    let since = bot.calc_since_us()?.ok_or_else(super::schedule::time_range_error)?;
    // Rails binds a Time as its quoted_date text and SQLite compares text: bind the same text.
    let since_text = format_time(at(since)?);

    let rows = crate::figures::fill::commitments(c,bot.id,&since_text).map_err(data)?;

    let interval = bot.interval().ok_or_else(|| EngineError::Data("interval".into()))?;
    let quote_amount = bot.quote_amount().ok_or_else(|| EngineError::Data("quote_amount".into()))?;
    let smart = bot.smart_quote_amount();
    let eff = effective(interval, quote_amount, smart);
    let intervals = interval_count(checkpoints(started, now_us, eff)?.last_us, since, eff)?;
    // Keep Integer/Float settings kinds; Accountable alone determines carry coercion.
    pending_window(bot.effective_quote_amount_num()?, intervals, carry_num(bot.transient.get("missed_quote_amount"))?, &rows)?.to_dec().map_err(data)
}

/// Bot::Lifecycle#start(start_fresh: false)'s `set_orders_now`, for a stopped bot the web continued. `use_delayed_first` is
/// always false there (only a fresh start computes a start time), so the bot runs at once unless #restarting_within_interval?:
/// it had ticked (last_action_job_at is set) and what it owes, capped as QuoteAmountLimitable caps it, is under one
/// `effective_quote_amount` (a smart split's amount when the split is on). Only the buy side: a selling bot is refused.
pub fn continue_runs_now(c: &Connection, bot: &Bot, now_us: i64) -> Result<bool, EngineError> {
    if bot.last_action_job_at_us()?.is_none() { return Ok(true); }
    let mut owed = pending_quote_amount(c, bot, now_us)?;
    if let Some(available) = quote_amount_available(c, bot)? { if available < owed { owed = available; } }
    Ok(owed >= bot.effective_quote_amount_num()?.to_dec().map_err(data)?)
}

/// Bot::QuoteAmountLimitable#quote_amount_available_before_limit_reached (quote_amount_limitable.rb:50-79), with one stricter
/// term: an unresolved placement intent counts as spent at its full quote until it is settled.
/// None = no cap (the limit is off). Since the stamp (`created_at >= quote_amount_limit_enabled_at`; an unset stamp matches
/// nothing), submitted REGULAR buys count: closed at their executed quote, waiting at their submitted quote (or
/// amount × price), cancelled or abandoned at what they executed. Failed and skipped rows, sells and other types do not.
///
/// Computed with Ruby's own numerics: the closed and waiting buckets are BigDecimal (decimal columns); the
/// stopped bucket is `pluck(Arel.sql('COALESCE(quote_amount_exec, 0)'))`, SQLite's own INTEGER or REAL; each bucket is an
/// Array#sum in row order, the buckets are added closed + waiting + stopped, and the limit is the settings' Integer or Float.
/// So a Float leaks in where Rails' does: cap 60.03 with one cancelled fill of 60.02 leaves 0.00999999999999801.
pub fn quote_amount_available_num(c: &Connection, bot: &Bot) -> Result<Option<Num>, EngineError> {
    Ok(engine_cap(c, bot, None)?.0)
}

/// RULING-B2A-R1b/R1c: the ONLY cap Float arithmetic boundary. Preserve SQL numeric kinds,
/// Ruby Array#sum, operand coercion and Float comparison. Sizing receives Float#to_d via to_dec.
/// Exact normalized availability is checked separately under the placement write lock.
fn rails_cap_availability(limit: Num, rows: &[Commitment], intent: Option<BigDec>, precision: Option<i64>) -> Result<(Num, bool), EngineError> {
        bounded_num(&limit)?;
        if let Some(ref amount) = intent { bounded_decimal(amount)?; }
        let mut closed = vec![];
        let mut waiting = vec![];
        let mut stopped = vec![];
        for row in rows {
            let value = match row.cap_kind {
                CapKind::Integer(n) => Num::Int(n),
                CapKind::RealBits(bits) => Num::Float(f64::from_bits(bits)),
                CapKind::Decimal => Num::Dec(row.value.clone()),
            };
            match row.status {
                2 => closed.push(value),
                0 | 1 => waiting.push(value),
                _ => stopped.push(value),
            }
        }
        let mut spent = ruby_sum(&closed).map_err(EngineError::Arithmetic)?.add(&ruby_sum(&waiting).map_err(EngineError::Arithmetic)?).map_err(EngineError::Arithmetic)?
            .add(&ruby_sum(&stopped).map_err(EngineError::Arithmetic)?).map_err(EngineError::Arithmetic)?;
    if let Some(quote) = intent { spent = spent.add(&Num::Dec(quote)).map_err(EngineError::Arithmetic)?; }
    let left = limit.sub(&spent).map_err(EngineError::Arithmetic)?;
    let left = if left.is_negative() { Num::Int(0) } else { left };
    let reached = match precision {
        Some(d) => left.lt_f64(1.0 / 10f64.powi(d as i32)).map_err(EngineError::Arithmetic)?,
        None => false,
    };
    Ok((left, reached))
}

fn engine_cap(c: &Connection, bot: &Bot, precision: Option<i64>) -> Result<(Option<Num>, bool), EngineError> {
    validate_stored_amounts(&bot.settings, &bot.transient)?;
    let Some(limit) = bot.quote_amount_limit().map_err(EngineError::Data)? else { return Ok((None, false)) };
    let rows = match bot.quote_amount_limit_enabled_at_us().map_err(EngineError::Data)? {
        Some(since) => crate::figures::fill::commitments(c, bot.id, &format_time(at(since)?)).map_err(data)?,
        None => vec![],
    };
    let intent = bot.rust_placement().map(|intent| {
        let quote = intent["quote_amount"].as_str().ok_or_else(||data("unreadable intent quote"))?;
        BigDec::parse(quote).map_err(data)
    }).transpose()?;
    let (left, reached) = rails_cap_availability(limit, &rows, intent, precision)?;
    Ok((Some(left), reached))
}

/// `quote_amount_available_num` as the BigDecimal the engine sizes with (a Float as Float#to_d, which is what Rails' next
/// BigDecimal operation makes of it).
pub fn quote_amount_available(c: &Connection, bot: &Bot) -> Result<Option<BigDec>, EngineError> {
    quote_amount_available_num(c, bot)?.map(|n| n.to_dec().map_err(data)).transpose()
}

/// Bot::QuoteAmountLimitable#quote_amount_limit_reached?: what is left is under the pair's precision floor,
/// `1.0 / 10**min(quote_decimals)` over Bot#tickers (members and former members, available and trading-enabled; with none
/// the floor is 0 and nothing is under it). Not the venue minimum: a remainder between the two keeps the bot running.
pub fn quote_amount_limit_reached(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    if !bot.quote_amount_limited() { return Ok(false); }
    let mut assets = bot.asset_ids();
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
    for a in s.query_map([bot.id], |r| r.get::<_, i64>(0))? { let a = a?; if !assets.contains(&a) { assets.push(a); } }
    let decimals: Option<i64> = c.query_row(
        "SELECT min(quote_decimals) FROM tickers WHERE exchange_id = ?1 AND quote_asset_id = ?2 AND available = 1 AND trading_enabled = 1 \
         AND base_asset_id IN (SELECT value FROM json_each(?3))",
        params![bot.exchange_id, bot.quote_asset_id(), serde_json::to_string(&assets).map_err(data)?], |r| r.get(0))?;
    let Some(d) = decimals else { return Ok(false) };
    // `1.0 / (10**d)`: a Float, compared as Ruby compares the remainder's own class with it.
    Ok(engine_cap(c, bot, Some(d))?.1)
}

/// R1c order safety: exact cap minus exact normalized commitments, including unresolved intent.
/// The JSON decimal spelling is used here, never the Rails Float-to-decimal approximation of the cap.
pub fn exact_cap_available(c: &Connection, bot: &Bot) -> Result<Option<BigDec>, EngineError> {
    validate_stored_amounts(&bot.settings, &bot.transient)?;
    if !bot.quote_amount_limited() { return Ok(None); }
    let limit = match bot.settings.get("quote_amount_limit") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => BigDec::from_i64(1000),
        Some(Value::Number(n)) => BigDec::parse(&n.to_string()).map_err(data)?,
        _ => return Err(EngineError::Data("invalid quote cap".into())),
    };
    let mut spent = BigDec::zero();
    if let Some(since) = bot.quote_amount_limit_enabled_at_us().map_err(EngineError::Data)? {
        let rows = crate::figures::fill::commitments(c, bot.id, &format_time(at(since)?)).map_err(data)?;
        spent = spent_in_window(&rows)?.to_dec().map_err(data)?;
    }
    if let Some(intent) = bot.rust_placement() {
        let quote = intent["quote_amount"].as_str().ok_or_else(||EngineError::Data("unreadable intent quote".into()))?;
        spent = spent.checked_add(&BigDec::parse(quote).map_err(data)?).map_err(data)?;
    }
    Ok(Some(limit.checked_add(&(&BigDec::zero() - &spent)).map_err(data)?.max(BigDec::zero())))
}

/// R2 distinguishes a reduced below-minimum leg from a placement fence that changed.
pub enum CapGuard { Place(OrderPlan), BelowMinimum(OrderPlan) }

/// RULING-B2A-R2: compare the EXACT cost after venue quantization, never rounded division or cents.
/// A safe order retains Rails' entire sizing/minimum decision. Only an actual reduction checks a minimum.
/// At most four venue increments may be removed; a larger change refuses with a reason, under the intent lock.
pub fn guard_exact_cap(c: &Connection, bot: &Bot, plan: &OrderPlan) -> Result<CapGuard, EngineError> {
    let Some(exact) = exact_cap_available(c, bot)? else { return Ok(CapGuard::Place(plan.clone())); };
    let (base, quote, price_scale) = plan.ticker.scales().map_err(EngineError::Data)?;
    let price = if plan.limit { plan.price.floor(price_scale) } else { plan.price.clone() };
    let quantity_wire = plan.limit || !plan.quote_type;
    let scale = if quantity_wire { base } else { quote };
    let mut units = if plan.limit && plan.quote_type {
        plan.volume.div(&price).ok_or_else(||EngineError::Data("zero limit price".into()))?.floor(base)
    } else { plan.volume.floor(scale) };
    let exact_cost = |units: &BigDec| -> Result<BigDec, EngineError> {
        if quantity_wire { units.checked_mul(&price).map_err(data) } else { Ok(units.clone()) }
    };
    let mut cost = exact_cost(&units)?;
    if cost <= exact { return Ok(CapGuard::Place(plan.clone())); }
    let step = BigDec::parse(&format!("1e-{scale}")).map_err(data)?;
    for _ in 0..4 {
        units = units.checked_add(&(&BigDec::zero() - &step)).map_err(data)?.max(BigDec::zero());
        cost = exact_cost(&units)?;
        if cost <= exact { break; }
    }
    if cost > exact { return Err(EngineError::Data("exact cap guard did not converge within 4 venue increments; order not placed".into())); }
    let mut guarded = plan.clone();
    guarded.quote_amount = cost.clone();
    guarded.amount = if quantity_wire { units.clone() } else {
        cost.div(&price).ok_or_else(||EngineError::Data("zero order price".into()))?
    };
    // A reduced Alpaca limit intent stores base units. Recovery must not divide a rounded cost again.
    guarded.quote_type = !quantity_wire;
    guarded.volume = units;
    if guarded.volume.is_zero() || cost < plan.ticker.minimum_quote_size {
        return Ok(CapGuard::BelowMinimum(guarded));
    }
    Ok(CapGuard::Place(guarded))
}
pub fn web_effective_amount(bot: &WebBot) -> Result<WebNum, WebError> {
    validate_web_amounts(bot)?;
    let key = if bot.on("smart_intervaled") && bot.number("smart_interval_quote_amount").is_some() { "smart_interval_quote_amount" } else { "quote_amount" };
    bot.number(key).ok_or_else(|| error("effective amount is missing"))
}
pub fn web_serialized(value: WebNum) -> Result<Value, WebError> {
    bounded_num(&core(value.clone()))?;
    match value { WebNum::Int(n) => Ok(json!(n)), WebNum::Float(n) if n.is_finite() => Ok(json!(n)), WebNum::Dec(n) => Ok(json!(n.to_s_f())), _ => Err(error("carry is not finite")) }
}
pub fn web_minimum(a: WebNum, b: WebNum) -> Result<WebNum, WebError> {
    Ok(view(minimum(core(a), core(b))?))
}

/// Fallible Accountable read through the shared normalizer, bounded to 100000 rows.
pub fn web_pending(c: &Connection, bot: &WebBot, now: DateTime<Utc>) -> Result<WebNum, WebError> {
    validate_web_amounts(bot)?;
    let mut started = bot.started_at;
    for prefix in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
        if bot.on(&format!("{prefix}ed")) {
            let met = codec::optional_time(bot.transient.get(&format!("{prefix}_condition_met_at"))).map_err(|_| error("unreadable stored timestamp"))?;
            started = started.zip(met).map(|(a,b)| a.max(b));
        }
    }
    let Some(started) = started else { return Ok(WebNum::Int(0)) };
    let changed: Option<String> = c.query_row("SELECT settings_changed_at FROM bots WHERE id=?1", [bot.id], |r| r.get(0))?;
    let since = changed.as_deref().map(codec::parse_time).transpose().map_err(|_| error("invalid settings window"))?.map_or(started, |at| at.max(started));
    let eff = bot.effective().ok_or_else(|| error("invalid carry interval"))?;
    let duration = eff.seconds();
    if !duration.is_finite() || !(web_bot::MIN_SPAN_SECONDS..=web_bot::MAX_SPAN_SECONDS).contains(&duration) { return Err(start::history_error()); }
    // The page/engine helper uses infallible calendar operations. Use the same arithmetic
    // with checked, bounded month stepping here, so a crafted draft cannot reach those panics.
    let anchor = bot.anchor()?.unwrap_or(now);
    let last = checkpoint(anchor, now, eff)?;
    let delta = last.checked_sub(since.timestamp_micros()).ok_or_else(|| error("carry time overflow"))?;
    let count = ((delta as f64 / 1_000_000.0) / duration).floor() + 1.0;
    if !count.is_finite() || count.abs() > i64::MAX as f64 { return Err(start::history_error()); }
    let amount = core(web_effective_amount(bot)?);
    let carry = carry_num(bot.transient.get("missed_quote_amount"))?;
    let rows = crate::figures::fill::action_commitments(c, bot.id, &codec::format_time(since)).map_err(web_bot::fill_error)?;
    let pending = pending_window(amount, count as i64, carry, &rows)?;
    if let Some(cap) = web_amount_limit(c, bot)? { Ok(view(minimum(pending, core(cap.left.ok_or_else(||error("unknown cap spend"))?))?)) } else { Ok(view(pending)) }
}

fn checkpoint(anchor: DateTime<Utc>, now: DateTime<Utc>, eff: Effective) -> Result<i64, WebError> {
    if !(1..=9999).contains(&anchor.year()) || !(1..=9999).contains(&now.year()) { return Err(start::history_error()); }
    let us = anchor.timestamp_micros();
    if let Effective::Month = eff {
        let mut next = anchor;
        for _ in 0..120_000 {
            if next > now { return next.checked_sub_months(Months::new(1)).map(|t| t.timestamp_micros()).ok_or_else(start::history_error); }
            next = next.checked_add_months(Months::new(1)).ok_or_else(start::history_error)?;
        }
        return Err(start::history_error());
    }
    let d = eff.seconds();
    if anchor > now { return Ok(round6_micros(us, &[(d,-1)])); }
    let elapsed = now.timestamp_micros().checked_sub(us).ok_or_else(start::history_error)? as f64 / 1_000_000.0;
    let terms = match eff {
        Effective::Seconds(_) => vec![((elapsed/d).ceil()*d,1),(d,-1)],
        Effective::MonthSeconds(_) => {
            let mut k = (elapsed/d).floor() as i64;
            for _ in 0..4 {
                if crate::ruby::exceeds(us,(d,k),now.timestamp_micros()) { k = k.checked_sub(1).ok_or_else(start::history_error)?; } else { break; }
            }
            for _ in 0..4 {
                if !crate::ruby::exceeds(us,(d,k),now.timestamp_micros()) || k < 1 { k = k.checked_add(1).ok_or_else(start::history_error)?; } else { break; }
            }
            vec![(d,k.checked_sub(1).ok_or_else(start::history_error)?)]
        }
        Effective::Month => return Err(start::history_error()),
    };
    Ok(round6_micros(us,&terms))
}

pub fn web_amount_limit(c: &Connection, bot: &WebBot) -> Result<Option<start::Limit>, WebError> {
    validate_web_amounts(bot)?;
    if !bot.on("quote_amount_limited") { return Ok(None); }
    let Some(limit) = bot.number("quote_amount_limit") else { return Ok(None) };
    let since = codec::optional_time(bot.transient.get("quote_amount_limit_enabled_at")).map_err(|_|error("unreadable stored timestamp"))?.map(format_time);
    let rows = match since {
        Some(since) => crate::figures::fill::action_commitments(c, bot.id, &since).map_err(web_bot::fill_error)?,
        None => vec![],
    };
    let intent = bot.transient.get("rust_placement").filter(|v| !v.is_null()).map(|v| {
        BigDec::parse(v["quote_amount"].as_str().ok_or_else(||error("unreadable intent quote"))?).map_err(|_|error("unreadable intent quote"))
    }).transpose()?;
    let precision = bot.tickers.iter().map(|t| i64::from(t.quote_decimals)).min();
    let (left, reached) = rails_cap_availability(core(limit), &rows, intent, precision)?;
    Ok(Some(start::Limit { left: Some(view(left)), reached }))
}
/// Automation::Schedulable::INTERVALS, in seconds as `interval_duration.to_f` gives them.
fn interval_seconds(interval: super::schedule::Interval) -> f64 {
    match interval { super::schedule::Interval::Hour => 3_600.0, super::schedule::Interval::Day => 86_400.0, super::schedule::Interval::Week => 604_800.0, super::schedule::Interval::Month => 2_629_746.0 }
}

/// Bot::SmartIntervalable#smart_interval_minimum(:quote).
pub fn smart_interval_minimum(bot: &WebBot) -> start::Minimum {
    let Some(decimals) = bot.tickers.iter().map(|ticker| ticker.quote_decimals).min() else { return start::Minimum { value: WebNum::Int(0), reason: start::Reason::None, decimals: 0 } };
    // At most one order every five minutes.
    let frequency = match (bot.number("quote_amount"), bot.interval()) {
        (Some(amount), Some(interval)) => amount.to_f() / interval_seconds(interval) * 300.0,
        _ => 0.0,
    };
    let scale = 10f64.powi(i32::from(decimals));
    let precision = 1.0 / scale;
    // A one-asset basket keeps the pair bot's floor, which never included the venue's minimum.
    let largest = |tickers: Vec<&web_bot::Ticker>| tickers.iter().map(|ticker| ticker.minimum_quote_size.clone()).max().map_or(0.0, |size| size.to_f());
    let exchange = match bot.kind {
        Kind::Basket if bot.one_asset() => 0.0,
        Kind::Basket => largest(bot.composition_tickers()),
        Kind::Index => largest(bot.tickers.iter().collect()),
    };
    let reason = if exchange >= frequency && exchange >= precision { start::Reason::Exchange } else if frequency >= precision { start::Reason::Frequency } else { start::Reason::Precision };
    let rounded_up = round_up(frequency, scale); // Utilities::Number.round_up
    start::Minimum { value: WebNum::Float(rounded_up.max(precision).max(exchange)), reason, decimals }
}

/// Bot::SmartIntervalable#initialize_smart_intervalable_settings: the amount a row without one is
/// given on load. `[quote_amount / 10, minimum * 10].max.round(decimals).to_f`, in Ruby's classes:
/// an Integer amount divides as an Integer (25 / 10 is 2). `None` without an amount or a ticker.
pub fn default_smart_interval_quote_amount(bot: &WebBot) -> Option<f64> {
    let decimals = bot.tickers.iter().map(|ticker| ticker.quote_decimals).min()?;
    let tenth = match bot.number("quote_amount")? {
        WebNum::Int(amount) => WebNum::Int(amount.checked_div_euclid(10)?),
        amount => WebNum::Float(amount.to_f() / 10.0),
    };
    let floor = smart_interval_minimum(bot).value.to_f() * 10.0;
    let larger = if floor > tenth.to_f() { WebNum::Float(floor) } else { tenth };
    Some(larger.round(decimals).to_f())
}


pub fn minimum_quote_amount_limit(bot: &WebBot) -> f64 {
    bot.tickers.iter().map(|ticker| ticker.quote_decimals).min().map_or(0.0, |decimals| 1.0 / 10f64.powi(i32::from(decimals)))
}

pub(crate) fn round_up(value: f64, scale: f64) -> f64 {
    if value > f64::MAX / scale { value } else { (value * scale).ceil() / scale }
}

/// Preview's sell-cap projection, from validated normalizer rows only.
pub fn base_cap_reached(c: &Connection, id: i64, limit: &BigDec, since: Option<&str>, decimals: Option<u8>) -> Result<bool, WebError> {
    let mut total = BigDec::zero();
    for amount in crate::figures::fill::sold_commitments(c, id, since).map_err(web_bot::fill_error)? {
        total = total.checked_add(&amount.quantity).map_err(data)?;
    }
    let floor = decimals.map_or(0.0, |d| 10f64.powi(-i32::from(d)));
    let floor = BigDec::from_f64(floor).map_err(data)?;
    Ok((limit - &total).max(BigDec::zero()) < floor)
}

pub fn balance_buffer(bot: &Bot, interval_seconds: f64, days: f64) -> Result<BigDec, EngineError> {
    BigDec::from_f64(bot.quote_amount().unwrap_or_default() / interval_seconds * days).map_err(data)
}
pub fn sell_minimum(amount: f64, seconds: f64, decimals: u8) -> start::Minimum {
    let frequency = if amount > 0.0 && seconds > 0.0 { amount / seconds * 300.0 } else { 0.0 };
    let scale = 10f64.powi(i32::from(decimals));
    let precision = 1.0 / scale;
    start::Minimum { value: WebNum::Float(round_up(frequency, scale).max(precision)),
        reason: if frequency >= precision { start::Reason::Frequency } else { start::Reason::Precision }, decimals }
}
pub fn default_sell_split(amount: f64, minimum: &start::Minimum) -> Result<Value, WebError> {
    let split = BigDec::from_f64(amount).map_err(data)?.div(&BigDec::from_i64(10)).ok_or_else(||error("invalid sell split"))?;
    let floor = WebNum::Float(minimum.value.to_f() * 10.0);
    let amount = WebNum::Dec(split);
    let amount = if amount.to_f() >= floor.to_f() { amount } else { floor };
    Ok(json!(amount.round(minimum.decimals).to_f()))
}

/// Exact aggregate for normalized figure/lots consumers; no Float conversion.
pub fn sum_figure_decimals<'a>(mut values: impl Iterator<Item=&'a crate::figures::dec::Dec>) -> Result<crate::figures::dec::Dec, crate::figures::num::NumError> {
    values.try_fold(crate::figures::dec::Dec::zero(), |sum, value| &sum + value)
}

pub fn effective_interval(interval: super::schedule::Interval, quote_amount: f64, smart_quote_amount: Option<f64>) -> Effective {
    let seconds = match smart_quote_amount {
        Some(s) => interval.seconds() / (quote_amount / s),
        None => return if interval == super::schedule::Interval::Month { Effective::Month } else { Effective::Seconds(interval.seconds()) },
    };
    if seconds == super::schedule::MONTH_SECONDS { Effective::MonthSeconds(seconds) } else { Effective::Seconds(seconds) }
}

/// Redeployable#redeploy_banked and #redeploy_spent. Banked is only proceeds a LIQUIDATION reported (MQ6: an unpriced
/// sale banks nothing, so no offer exceeds Rails'); spent is the normalized REDEPLOY buy value (B2-1/MQ7).
pub fn index_redeploy_totals(orders: &[crate::figures::db::Order]) -> Result<(crate::figures::dec::Dec,crate::figures::dec::Dec),crate::figures::FiguresError> {
    use crate::figures::dec::Dec;
    let (mut banked,mut spent)=(Dec::zero(),Dec::zero());
    for order in orders {
        crate::figures::budget::charge(1,0)?;
        match order.kind.as_str(){
            "LIQUIDATION"=>if let Some(value)=crate::figures::fill::reported(order)? { banked=(&banked+&value)?; },
            "REDEPLOY"=>if let Some(fill)=crate::figures::fill::parse(order)? { spent=(&spent+&fill.value)?; },
            _=>{},
        }
    }
    Ok((banked,spent))
}


#[cfg(test)]
mod r7_tests {
    use super::*;
    #[test]
    fn r7_integer_pending_multiply_overflow_refuses() {
        for (amount,count) in [(9_007_199_254_740_992,1024),(-9_007_199_254_740_992,1025)] {
            let result=pending_window(Num::Int(amount),count,Num::Int(0),&[]);
            assert!(result.is_err(), "R7 multiplication refuses");
            assert!(format!("{result:?}").contains("IntegerOverflow"), "R7 typed multiplication error: {result:?}");
        }
        assert!(matches!(pending_window(Num::Int(2),3,Num::Int(0),&[]),Ok(Num::Int(6))));
    }
}
