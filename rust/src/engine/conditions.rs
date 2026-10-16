//! Bot::PriceLimitable's gate (slice 7a: `price_limit` / `sell_price_limit` with the `pause` action) and the minute check
//! that re-arms a paused bot (Bot::PriceLimitCheckJob on Bot::LimitCheckJobBase).
use super::model::{self, Bot, PriceRule, Ticker};
use super::tick::PriceCache;
use super::{accounting, placement, Clock, EngineError};
use crate::enums::BotStatus;
use crate::ruby::{iso8601_ms, BigDec, Num};
use crate::venue::{Attributed, PriceSide, VenueError};
use chrono::{DateTime, Duration, Timelike, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cmp::Ordering;

/// `price <=> n` as Ruby compares a BigDecimal with an Integer, a Float (Float#to_d) or a BigDecimal.
fn cmp(price: &BigDec, n: &Num) -> Result<Ordering, EngineError> { Ok(price.cmp(&n.to_dec().map_err(EngineError::Arithmetic)?)) }
fn num_cmp(a: &Num, b: &Num) -> Result<Ordering, EngineError> {
    Ok(match (a, b) {
        (Num::Int(x), Num::Int(y)) => x.cmp(y),
        (Num::Int(_) | Num::Float(_), Num::Int(_) | Num::Float(_)) => num_f(a).partial_cmp(&num_f(b)).unwrap_or(Ordering::Equal),
        _ => a.to_dec().map_err(EngineError::Arithmetic)?.cmp(&b.to_dec().map_err(EngineError::Arithmetic)?),
    })
}
fn num_f(n: &Num) -> f64 { match n { Num::Int(i) => *i as f64, Num::Float(f) => *f, Num::Dec(d) => d.to_f() } }

/// price_limit_value_condition_satisfied? (price_limitable.rb:294-308): strict below/above; `between` is inclusive of
/// `[lower, upper].min` and `.max`; any other condition is never met.
pub fn price_met(rule: &PriceRule, price: &BigDec) -> Result<bool, EngineError> {
    Ok(match rule.value_condition.as_deref() {
        Some("below") => cmp(price, &rule.limit)? == Ordering::Less,
        Some("above") => cmp(price, &rule.limit)? == Ordering::Greater,
        Some("between") => {
            let (a, b) = (&rule.lower, &rule.upper);
            let (lo, hi) = if num_cmp(b, a)? == Ordering::Less { (b, a) } else { (a, b) };
            cmp(price, lo)? != Ordering::Less && cmp(price, hi)? != Ordering::Greater
        }
        _ => false,
    })
}

/// What get_price_limit_condition_met? answered (price_limitable.rb:197-222), its writes made.
pub enum Evaluated {
    /// No rule on the active side.
    Off,
    /// Met (`after` with a stored met_at, or the price satisfies it). `refreshed`: the bot as the stamp left it, when this
    /// call stamped met_at, so the tick sizes from it.
    Met { refreshed: Option<Box<Bot>> },
    /// The price was read and does not satisfy the rule (a met_at it had is gone, its carry captured).
    Unmet,
    /// No available ticker for the rule: not met, nothing written (price_limitable.rb:202).
    Unavailable,
    /// The price read failed: nothing written; the caller decides what the failure means.
    Failed(VenueError),
}

/// The rule's ticker: `tickers.available.find_by(id: <prefix>_in_ticker_id)`, Bot#tickers being the venue's available,
/// trading-enabled pairs at the bot's quote for its members and bot_index_assets (dca_multi_asset.rb:364-369); a rule with
/// no stored id reads the one that sorts first by its venue code (`tickers.min_by { |t| t[:base] }`).
pub fn rule_ticker(c: &Connection, bot: &Bot, rule: &PriceRule) -> Result<Option<Ticker>, EngineError> {
    let mut ids = bot.asset_ids();
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
    for a in s.query_map([bot.id], |r| r.get::<_, i64>(0))? { let a = a?; if !ids.contains(&a) { ids.push(a); } }
    drop(s);
    let mut tickers = vec![];
    for a in ids {
        if let Some(t) = model::ticker_for_asset(c, bot, a)? { if t.available && t.trading_enabled && !tickers.iter().any(|x: &Ticker| x.id == t.id) { tickers.push(t); } }
    }
    tickers.sort_by_key(|t| t.id); // the relation's own order
    let id = match rule.ticker_id { Some(id) => Some(id), None => tickers.iter().min_by(|a, b| a.base_code.cmp(&b.base_code)).map(|t| t.id) };
    Ok(id.and_then(|id| tickers.into_iter().find(|t| t.id == id)))
}

/// get_price_limit_condition_met? for the active side, with its two writes, each fenced to the run that read the price:
/// met for the first time stamps `<prefix>_condition_met_at`; unmet after a met_at captures the carry
/// (set_missed_quote_amount, capped as QuoteAmountLimitable caps pending_quote_amount) and clears met_at, in one write.
/// The last price comes through Exchange's 5 s cache, the one the tick's own reference reads. The writes are stamped with
/// the clock read after the price arrives (Rails' `Time.current` there): a read that crosses an interval checkpoint must
/// not stamp, or capture the carry at, the instant before it.
pub async fn evaluate<V: Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Evaluated, EngineError> {
    let now = clock.now();
    let Some(rule) = bot.price_rule(bot.selling()).map_err(EngineError::Data)? else { return Ok(Evaluated::Off) };
    if rule.after && rule.met_at_us.is_some() { return Ok(Evaluated::Met { refreshed: None }); }
    let Some(ticker) = rule_ticker(c, bot, &rule)? else { return Ok(Evaluated::Unavailable) };
    let key = (bot.exchange_id, ticker.id, PriceSide::Last);
    let (price, origin) = match prices.get(key, now, version) {
        Some(hit) => (hit, version.clone()),
        None => {
            let result = venue.price_result(&ticker, PriceSide::Last).await;
            let origin = result.origin().clone();
            match result.value {
                Ok(p) => { prices.put_result(key, now, model::Produced::new(p.clone(), origin.clone())); (p, origin) }
                Err(e) => return Ok(Evaluated::Failed(e)),
            }
        }
    };
    let now = clock.now();
    let key = format!("{}_condition_met_at", rule.prefix);
    if price_met(&rule, &price)? {
        if rule.met_at_us.is_some() { return Ok(Evaluated::Met { refreshed: None }); }
        let refreshed = model::run_credential_write(c, bot, &origin, |tx| {
            model::update_transient(tx, bot.id, &[(key.as_str(), json!(iso8601_ms(now)))], now)?;
            model::load_bot(tx, bot.id)
        })?;
        return Ok(Evaluated::Met { refreshed: Some(Box::new(refreshed)) });
    }
    if rule.met_at_us.is_some() {
        model::run_credential_write(c, bot, &origin, |tx| {
            let current = model::load_bot(tx, bot.id)?;
            let mut owed = accounting::pending_quote_amount_num(tx, &current, now.timestamp_micros())?;
            if let Some(available) = accounting::quote_amount_available_num(tx, &current)? { owed = accounting::minimum(owed, available)?; }
            // The capture lands (the carry window moved: started_at now reads nil), so missed_quote_amount_was_set stays true.
            model::update_transient(tx, bot.id, &[("missed_quote_amount", accounting::serialized(owed)?), ("missed_quote_amount_was_set", json!(true)),
                                                  (key.as_str(), Value::Null)], now)
        })?;
    }
    Ok(Evaluated::Unmet)
}

/// `Time.now.utc.end_of_minute`, to the microsecond.
pub fn end_of_minute(now: DateTime<Utc>) -> DateTime<Utc> {
    now.with_second(59).and_then(|t| t.with_nanosecond(999_999_000)).unwrap_or(now)
}

/// Bot::RepairOrphanedBotsJob (every 15 minutes) restarting a dead check chain at the check's own next time
/// (Bot::LimitCheckable#enqueue_limit_check_job): the end of the minute of the next quarter hour.
pub fn repair_at(now: DateTime<Utc>) -> DateTime<Utc> {
    let minute = now.minute() - now.minute() % 15;
    let quarter = now.with_minute(minute).and_then(|t| t.with_second(0)).and_then(|t| t.with_nanosecond(0)).unwrap_or(now) + Duration::minutes(15);
    end_of_minute(quarter)
}

/// What a check decided.
#[derive(Debug, PartialEq)]
pub enum Checked {
    /// Met: the bot is `scheduled` again, with a run-now obligation committed in the same write (placement::run_now), so it
    /// ticks at once, off its grid, across a restart too.
    Rearmed,
    /// The next check: the end of the minute when not met, a minute on when the read failed.
    Next(DateTime<Utc>),
    /// No longer waiting (stopped, reversed or re-armed meanwhile): no chain.
    Gone,
}

/// A zero price is not a failed Result in Rails: Exchange#get_last_price raises "Wrong last price for …" (kraken.rb:224-236),
/// out of the gate (the tick fails) and out of the check job (the chain dies until Repair). So does an unreadable price:
/// parse_venue_number's ArgumentError (alpaca.rb:611), which the adapter words "unreadable … price for …" or, for a number
/// out of range, "unreadable number …". VenueError carries no raise/Result distinction, so this reads that wording.
pub fn raised(e: &VenueError) -> Option<String> {
    match e { VenueError::Rejected(m) if m.first().is_some_and(|m| m.starts_with("Wrong ") || m.starts_with("invalid value for BigDecimal") || m.starts_with("unreadable ")) => Some(crate::ruby::to_sentence(m)), _ => None }
}

/// Rust-only marker of a check that raised (Bot::LimitCheckJobBase lets the job die): the chain waits for Repair.
pub const DEAD: &str = "rust_condition_check_dead";

/// Bot::LimitCheckJobBase#perform with Bot::PriceLimitCheckJob's condition, on a bot paused by its price rule.
pub async fn check<V: crate::venue::Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Checked, EngineError> {
    if bot.status != BotStatus::Waiting { return Ok(Checked::Gone); }
    let venue = crate::venue::Handle::for_bot(venue, c, bot)?;
    let evaluated = evaluate(c, &venue, bot, clock, prices, version).await?;
    let now = clock.now(); // after the price read: next_check_at and 1.minute.from_now are read there
    let revive = |tx: &Connection| -> Result<(), EngineError> {
        tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{DEAD}') WHERE id = ?1 AND json_type(transient_data, '$.{DEAD}') IS NOT NULL"), [bot.id])?;
        Ok(())
    };
    match evaluated {
        Evaluated::Met { .. } => {
            // rearm!: waiting → scheduled only while still waiting, and the immediate Bot::ActionJob as a durable obligation.
            let rearmed = model::run_write(c, bot, |tx| {
                let n = tx.execute("UPDATE bots SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status = ?4",
                                   rusqlite::params![BotStatus::Scheduled as i64, crate::codec::format_time(now), bot.id, BotStatus::Waiting as i64])?;
                if n == 1 { revive(tx)?; placement::run_now(tx, &model::load_bot(tx, bot.id)?, now)?; }
                Ok(n == 1)
            })?;
            Ok(if rearmed { Checked::Rearmed } else { Checked::Gone })
        }
        Evaluated::Off | Evaluated::Unmet | Evaluated::Unavailable => { model::run_write(c, bot, revive)?; Ok(Checked::Next(end_of_minute(now))) }
        Evaluated::Failed(e) => match raised(&e) {
            Some(m) => Err(EngineError::Data(m)),
            // A failed Result, a rejected key included, or a rescued transport raise: again in a minute (limit_check_job_base.rb:11, :52-55).
            None => { model::run_write(c, bot, revive)?; Ok(Checked::Next(now + Duration::minutes(1))) }
        },
    }
}
