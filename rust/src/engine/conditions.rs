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
pub fn rule_ticker(c: &Connection, bot: &Bot, rule: &PriceRule) -> Result<Option<Ticker>, EngineError> { Ok(rule_ticker_quoted(c, bot, rule)?.0) }

/// `rule_ticker`, and `decimals[:quote]` of the same relation (Bots::DcaMultiAsset#decimals: the least quote precision among
/// Bot#tickers), which the status line rounds the price to.
fn rule_ticker_quoted(c: &Connection, bot: &Bot, rule: &PriceRule) -> Result<(Option<Ticker>, Option<i64>), EngineError> {
    let mut ids = bot.asset_ids();
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
    for a in s.query_map([bot.id], |r| r.get::<_, i64>(0))? { let a = a?; if !ids.contains(&a) { ids.push(a); } }
    drop(s);
    let mut tickers = vec![];
    for a in ids {
        if let Some(t) = model::ticker_for_asset(c, bot, a)? { if t.available && t.trading_enabled && !tickers.iter().any(|x: &Ticker| x.id == t.id) { tickers.push(t); } }
    }
    tickers.sort_by_key(|t| t.id); // the relation's own order
    let decimals = tickers.iter().map(|t| t.quote_decimals).min();
    let id = match rule.ticker_id { Some(id) => Some(id), None => tickers.iter().min_by(|a, b| a.base_code.cmp(&b.base_code)).map(|t| t.id) };
    Ok((id.and_then(|id| tickers.into_iter().find(|t| t.id == id)), decimals))
}

/// How a condition's writes are fenced to what read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fence {
    /// The tick's or the check's own run (model::run_credential_write).
    Run,
    /// The status line's info job (Bot::BroadcastPriceLimitInfoUpdateJob), which runs on any status, a stopped or deleted bot
    /// included: the write lands only while the bot is configured as it was read (`config`), under the key that read the price.
    Config,
}

/// What a condition decision was read from: the class, venue, settings (direction included), the carry window's two marks,
/// every rule's met_at and the carry. A write fenced by `Fence::Config` lands only while this is unchanged.
fn config(b: &Bot) -> Result<Value, EngineError> {
    let kept: serde_json::Map<String, Value> = b.transient.as_object().into_iter().flatten()
        .filter(|(k, _)| k.ends_with("_condition_met_at") || k.as_str() == "missed_quote_amount" || k.as_str() == "missed_quote_amount_was_set")
        .map(|(k, v)| (k.clone(), v.clone())).collect();
    Ok(json!([b.bot_type, b.exchange_id, b.settings, b.started_at_us()?, b.settings_changed_at_us()?, kept]))
}

fn fenced_write<T>(c: &Connection, bot: &Bot, origin: &Option<model::CredentialVersion>, fence: Fence,
                   work: impl FnOnce(&model::FencedTransaction<'_>) -> Result<T, EngineError>) -> Result<T, EngineError> {
    match fence {
        Fence::Run => model::run_credential_write(c, bot, origin, work),
        Fence::Config => model::credential_write(c, origin, |tx| {
            if config(&model::load_bot(tx, bot.id)?)? != config(bot)? { return Err(EngineError::RunFenced); }
            work(tx)
        }),
    }
}

/// The decorated `started_at` as Rails' chained trigger-rule decorators read it, on the ACTIVE side (`active_*_limited?`,
/// price_limitable.rb:106-115 and its three siblings): the later of started_at and every enabled rule's met_at, nil while
/// one of them is nil. accounting::condition_started_at reads the buy side only (7b-3 T6 makes it side-aware).
fn active_started_us(bot: &Bot) -> Result<Option<i64>, EngineError> {
    let side = if bot.selling() { "sell_" } else { "" };
    let mut started = bot.started_at_us()?;
    for rule in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
        if bot.settings.get(format!("{side}{rule}ed").as_str()) != Some(&Value::Bool(true)) { continue; }
        let met = crate::codec::optional_time(bot.transient.get(format!("{side}{rule}_condition_met_at").as_str()))
            .map_err(|_| EngineError::Data("unreadable stored timestamp".into()))?;
        started = started.zip(met).map(|(a, b)| a.max(b.timestamp_micros()));
    }
    Ok(started)
}

/// The last price through Exchange's 5 s cache, and the key that produced it.
async fn last_price<V: Attributed>(venue: &V, bot: &Bot, ticker: &Ticker, now: DateTime<Utc>, prices: &PriceCache, version: &Option<model::CredentialVersion>)
    -> Result<(BigDec, Option<model::CredentialVersion>), VenueError> {
    let key = (bot.exchange_id, ticker.id, PriceSide::Last);
    if let Some(hit) = prices.get(key, now, version) { return Ok((hit, version.clone())); }
    let result = venue.price_result(ticker, PriceSide::Last).await;
    let origin = result.origin().clone();
    let p = result.value?;
    prices.put_result(key, now, model::Produced::new(p.clone(), origin.clone()));
    Ok((p, origin))
}

/// get_price_limit_condition_met? for the active side, with its two writes, each fenced to what read the price (`fence`):
/// met for the first time stamps `<prefix>_condition_met_at`; unmet after a met_at captures the carry
/// (set_missed_quote_amount, capped as QuoteAmountLimitable caps pending_quote_amount) and clears met_at, in one write.
/// The last price comes through Exchange's 5 s cache, the one the tick's own reference reads. The writes are stamped with
/// the clock read after the price arrives (Rails' `Time.current` there): a read that crosses an interval checkpoint must
/// not stamp, or capture the carry at, the instant before it. Each write also broadcasts the status line from inside it, as
/// Rails does (broadcast_price_limit_info_update: its Rails.cache.fetch, stored in `INFO_KEY`).
pub async fn evaluate<V: Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>, fence: Fence) -> Result<Evaluated, EngineError> {
    let now = clock.now();
    let Some(rule) = bot.price_rule(bot.selling()).map_err(EngineError::Data)? else { return Ok(Evaluated::Off) };
    if rule.after && rule.met_at_us.is_some() { return Ok(Evaluated::Met { refreshed: None }); }
    let (Some(ticker), decimals) = rule_ticker_quoted(c, bot, &rule)? else { return Ok(Evaluated::Unavailable) };
    let (price, origin) = match last_price(venue, bot, &ticker, now, prices, version).await { Ok(read) => read, Err(e) => return Ok(Evaluated::Failed(e)) };
    let now = clock.now();
    let key = format!("{}_condition_met_at", rule.prefix);
    let side = if bot.selling() { "selling" } else { "buying" };
    if price_met(&rule, &price)? {
        if rule.met_at_us.is_some() { return Ok(Evaluated::Met { refreshed: None }); }
        let refreshed = fenced_write(c, bot, &origin, fence, |tx| {
            model::update_transient(tx, bot.id, &[(key.as_str(), json!(iso8601_ms(now)))], now)?;
            if let Some(info) = info_value(&ticker, decimals, &price, true) { fetch_info(tx, bot.id, PRICE, side, now, info)?; }
            model::load_bot(tx, bot.id)
        })?;
        return Ok(Evaluated::Met { refreshed: Some(Box::new(refreshed)) });
    }
    if rule.met_at_us.is_some() {
        fenced_write(c, bot, &origin, fence, |tx| {
            let current = model::load_bot(tx, bot.id)?;
            if active_started_us(&current)?.is_none() {
                // Accountable#check_missed_quote_amount_was_set (accountable.rb:119-127): the clear leaves the carry window where it
                // was (another enabled rule, or no start, already keeps the decorated started_at nil), so the capture is taken
                // back: the carry as stored before it (nil when absent), and the marker nil.
                let before = current.transient.get("missed_quote_amount").cloned().unwrap_or(Value::Null);
                model::update_transient(tx, bot.id, &[("missed_quote_amount", before), ("missed_quote_amount_was_set", Value::Null), (key.as_str(), Value::Null)], now)?;
            } else {
                let mut owed = accounting::pending_quote_amount_num(tx, &current, now.timestamp_micros())?;
                if let Some(available) = accounting::quote_amount_available_num(tx, &current)? { owed = accounting::minimum(owed, available)?; }
                // The capture lands (the carry window moved: started_at now reads nil), so missed_quote_amount_was_set stays true.
                model::update_transient(tx, bot.id, &[("missed_quote_amount", accounting::serialized(owed)?), ("missed_quote_amount_was_set", json!(true)),
                                                      (key.as_str(), Value::Null)], now)?;
            }
            if let Some(info) = info_value(&ticker, decimals, &price, false) { fetch_info(tx, bot.id, PRICE, side, now, info)?; }
            Ok(())
        })?;
    }
    Ok(Evaluated::Unmet)
}

/// The price rule's name in the status line's keys and routes.
pub const PRICE: &str = "price_limit";
/// D-COND-INFO-STORE: what Rails keeps in Rails.cache under `bot_<id>_<rule>_info_<side>` for 20 s, kept in transient_data
/// (a Rust-only key, outside the parity snapshots): `{rule: {side: {base, quote, price, condition_met, expires_us}}}`.
pub const INFO_KEY: &str = "rust_condition_info";
/// The status line's requests the web left for the engine (BroadcastsController#*_info_update → the info job), Rust-only:
/// `{rule: {"version": n, "locale": "en"}}`. Each request bumps the version; the engine removes only the version it served.
pub const INFO_WANTED_KEY: &str = "rust_condition_info_wanted";
const INFO_TTL_US: i64 = 20_000_000;

/// The cached info of (rule, side) in `stored` (transient_data's INFO_KEY) while it lives (MemoryStore: expired once
/// `expires_at <= now`).
pub fn live_info<'a>(stored: Option<&'a Value>, rule: &str, side: &str, now: DateTime<Utc>) -> Option<&'a Value> {
    stored?.get(rule)?.get(side).filter(|v| v["expires_us"].as_i64().is_some_and(|at| at > now.timestamp_micros()))
}

/// broadcast_price_limit_info_update's hash: the pair's asset symbols, the price rounded to `decimals[:quote]`, the answer.
/// None: no quote precision (`round(nil)` raises in Rails: no info).
fn info_value(ticker: &Ticker, decimals: Option<i64>, price: &BigDec, met: bool) -> Option<Value> {
    let places = crate::ruby::scale(decimals?).ok()?;
    Some(json!({ "base": ticker.base_symbol, "quote": ticker.quote_symbol, "price": price.round(places).to_s_f(), "condition_met": met }))
}

/// `Rails.cache.fetch(key, expires_in: 20.seconds) { info }`: a live entry is kept as it is (its own expiry too); otherwise
/// `info` is stored for 20 s. Returns what the broadcast shows.
fn fetch_info(tx: &Connection, bot_id: i64, rule: &str, side: &str, now: DateTime<Utc>, info: Value) -> Result<Value, EngineError> {
    let transient: String = tx.query_row("SELECT transient_data FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
    let transient: Value = serde_json::from_str(&transient).map_err(|e| EngineError::Data(format!("bot {bot_id}: {e}")))?;
    if let Some(hit) = live_info(transient.get(INFO_KEY), rule, side, now) { return Ok(hit.clone()); }
    let mut all = transient.get(INFO_KEY).and_then(Value::as_object).cloned().unwrap_or_default();
    let mut entry = info;
    entry["expires_us"] = json!(now.timestamp_micros() + INFO_TTL_US);
    let mut sides = all.get(rule).and_then(Value::as_object).cloned().unwrap_or_default();
    sides.insert(side.into(), entry.clone());
    all.insert(rule.into(), Value::Object(sides));
    tx.execute(&format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{INFO_KEY}', json(?2)) WHERE id = ?1"),
               rusqlite::params![bot_id, Value::Object(all).to_string()])?;
    Ok(entry)
}

/// `reset_price_limit_info_cache`'s delete: the entry of `side` goes (a settings save that changed the rule).
pub fn forget_info(tx: &Connection, bot_id: i64, rule: &str, side: &str) -> Result<(), EngineError> {
    tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{INFO_KEY}.{rule}.{side}') WHERE id = ?1"), [bot_id])?;
    Ok(())
}

/// Leaves a status-line request for `rule` (the web's BroadcastsController#*_info_update; `user`: the bot must be theirs). One
/// statement: the version is bumped atomically, so a request the engine claimed earlier never hides this one. False: no such bot.
pub fn want_info(c: &Connection, bot_id: i64, user: Option<i64>, rule: &str, locale: &str) -> Result<bool, EngineError> {
    let k = INFO_WANTED_KEY;
    let n = c.execute(&format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{k}', json_set(coalesce(json_extract(transient_data, '$.{k}'), '{{}}'), \
                                '$.{rule}', json_object('version', coalesce(json_extract(transient_data, '$.{k}.{rule}.version'), 0) + 1, 'locale', ?3))) \
                                WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2) AND json_type(transient_data) = 'object'"), rusqlite::params![bot_id, user, locale])?;
    Ok(n == 1)
}

/// One status-line request as the engine claimed it: the bot as read with the request, and the request's version and locale.
#[derive(Clone, Debug)]
pub struct InfoClaim { pub bot: Bot, pub version: i64, pub locale: Option<String> }

/// The bots with a waiting request for `rule`, each claimed as it is now.
pub fn info_claims(c: &Connection, rule: &str) -> Result<Vec<InfoClaim>, EngineError> {
    let mut s = c.prepare(&format!("SELECT id, json_extract(transient_data, '$.{INFO_WANTED_KEY}.{rule}.version'), \
                                    json_extract(transient_data, '$.{INFO_WANTED_KEY}.{rule}.locale') FROM bots \
                                    WHERE json_type(transient_data, '$.{INFO_WANTED_KEY}.{rule}') IS NOT NULL ORDER BY id"))?;
    let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    let mut out = vec![];
    for (id, version, locale) in rows { out.push(InfoClaim { bot: model::load_bot(c, id)?, version: version.unwrap_or(0), locale }); }
    Ok(out)
}

/// Removes the request of `rule` only if it is still the claimed version: a newer one survives for the next pass.
pub fn ack_info(tx: &Connection, bot_id: i64, rule: &str, version: i64) -> Result<(), EngineError> {
    tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{INFO_WANTED_KEY}.{rule}') WHERE id = ?1 \
                         AND json_extract(transient_data, '$.{INFO_WANTED_KEY}.{rule}.version') IS ?2"), rusqlite::params![bot_id, version])?;
    tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{INFO_WANTED_KEY}') WHERE id = ?1 \
                         AND json_extract(transient_data, '$.{INFO_WANTED_KEY}') = '{{}}'"), [bot_id])?;
    Ok(())
}

/// Bot::BroadcastPriceLimitInfoUpdateJob (broadcast_price_limit_info_update, price_limitable.rb:224-250) for one claimed
/// request, on any status: the rule's ticker, its last price, then get_price_limit_condition_met? itself, its writes
/// included (Q1: a page view stamps or clears met_at and captures the carry, as Rails' does), then the cached info. Any failure
/// leaves no info, as Rails broadcasts nothing then; the request is served either way. Some(info): what the line shows.
/// Err(RunFenced): the bot's configuration changed since the claim; nothing of this run is kept and the request stays.
/// Err(CredentialsChanged): the key changed during the read; the request stays.
pub async fn info<V: crate::venue::Venue>(c: &Connection, venue: &V, claim: &InfoClaim, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Option<Value>, EngineError> {
    let bot = &claim.bot;
    let venue = &crate::venue::Handle::for_bot(venue, c, bot)?;
    let served = |c: &Connection| -> Result<Option<Value>, EngineError> {
        let tx = model::immediate(c)?; ack_info(&tx, bot.id, PRICE, claim.version)?; tx.commit()?; Ok(None)
    };
    // Only Bots::DcaMultiAsset and Bots::DcaSingleAsset include PriceLimitable (an index bot's job raises NoMethodError:
    // no info, no write). model::conditions_apply once #742 lands.
    if !matches!(bot.bot_type.as_str(), "Bots::DcaMultiAsset" | "Bots::DcaSingleAsset") { return served(c); }
    // A rule that is off renders nothing (the partial's own guard), so no info is kept for it.
    let Some(rule) = bot.price_rule(bot.selling()).map_err(EngineError::Data)? else { return served(c) };
    let (Some(ticker), decimals) = rule_ticker_quoted(c, bot, &rule)? else { return served(c) };
    let Ok((price, origin)) = last_price(venue, bot, &ticker, clock.now(), prices, version).await else { return served(c) };
    let met = match evaluate(c, venue, bot, clock, prices, version, Fence::Config).await? {
        Evaluated::Met { .. } => true,
        Evaluated::Failed(_) => return served(c),
        Evaluated::Off | Evaluated::Unmet | Evaluated::Unavailable => false,
    };
    let Some(info) = info_value(&ticker, decimals, &price, met) else { return served(c) };
    let now = clock.now();
    let side = if bot.selling() { "selling" } else { "buying" };
    model::credential_write(c, &origin, |tx| {
        let current = model::load_bot(tx, bot.id)?;
        // Settings or direction changed since the claim: the answer is for a rule that no longer is.
        if (&current.bot_type, current.exchange_id, &current.settings) != (&bot.bot_type, bot.exchange_id, &bot.settings) { return Err(EngineError::RunFenced); }
        let shown = fetch_info(tx, bot.id, PRICE, side, now, info)?;
        ack_info(tx, bot.id, PRICE, claim.version)?;
        Ok(Some(shown))
    })
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
    match e { VenueError::Rejected(m) | VenueError::Unauthorized(m) if m.first().is_some_and(|m| m.starts_with("Wrong ") || m.starts_with("invalid value for BigDecimal") || m.starts_with("unreadable ")) => Some(crate::ruby::to_sentence(m)), _ => None }
}

/// Rust-only marker of a check that raised (Bot::LimitCheckJobBase lets the job die): the chain waits for Repair.
pub const DEAD: &str = "rust_condition_check_dead";

/// Bot::LimitCheckJobBase#perform with Bot::PriceLimitCheckJob's condition, on a bot paused by its price rule.
pub async fn check<V: crate::venue::Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Checked, EngineError> {
    if bot.status != BotStatus::Waiting { return Ok(Checked::Gone); }
    let venue = crate::venue::Handle::for_bot(venue, c, bot)?;
    let evaluated = evaluate(c, &venue, bot, clock, prices, version, Fence::Run).await?;
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
