//! Bot::PriceLimitable's gate (slice 7a: `price_limit` / `sell_price_limit` with the `pause` action) and the minute check
//! that re-arms a paused bot (Bot::PriceLimitCheckJob on Bot::LimitCheckJobBase).
use super::model::{self, Bot, PriceRule, Ticker};
use super::tick::PriceCache;
use super::{accounting, placement, Clock, EngineError};
use crate::enums::BotStatus;
use crate::ruby::{iso8601_ms, BigDec, Num};
use crate::venue::{Attributed, PriceSide, VenueError};
use crate::web::bot::Kind;
use chrono::{DateTime, Duration, Timelike, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cmp::Ordering;

/// A rule a bot can carry: the spending cap and the four trigger rules (each with a buy and a sell side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule { Cap, Price, PriceDrop, MovingAverage, Indicator }

/// The trigger rules and their keys' prefix on the buy side (`sell_` + prefix on the sell side).
pub const TRIGGERS: [(Rule, &str); 4] = [(Rule::Price, "price_limit"), (Rule::PriceDrop, "price_drop_limit"),
                                          (Rule::MovingAverage, "moving_average_limit"), (Rule::Indicator, "indicator_limit")];

impl Rule {
    /// The rule a switch names, and whether it is the sell side's: `quote_amount_limited`, `base_amount_limited` (the cap's
    /// sell side, slice 6d), `[sell_]<prefix>ed`.
    pub fn of_flag(key: &str) -> Option<(Rule, bool)> {
        if key == "quote_amount_limited" { return Some((Rule::Cap, false)); }
        if key == "base_amount_limited" { return Some((Rule::Cap, true)); }
        let (selling, rest) = match key.strip_prefix("sell_") { Some(rest) => (true, rest), None => (false, key) };
        let prefix = rest.strip_suffix("ed")?;
        TRIGGERS.iter().find(|(_, p)| *p == prefix).map(|(rule, _)| (*rule, selling))
    }
}

/// What this build does with a rule on a bot kind (PLAN-conditions-every-bot §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// The engine runs it (with the `pause` action), the pages and the API take it.
    Implemented,
    /// It has nothing to act on there (an index bot never sells; an index or signal bot has no direction to flip): the
    /// switch is inert, eligibility admits it.
    Inapplicable,
    /// Not built yet: eligibility refuses a bot that has it on, as it always did.
    NotImplemented,
}

/// The (kind, rule) pairs the engine runs, with the `pause` action. Each slice adds its own with that rule's engine.
pub const IMPLEMENTED: &[(Kind, Rule)] = &[(Kind::Basket, Rule::Cap), (Kind::Basket, Rule::Price), (Kind::Index, Rule::Cap), (Kind::Index, Rule::Price)];

/// D-IDX-CONDITIONS / D-IDX-CAP: one rule set for every bot kind, Rails' class gap fixed (only DcaMultiAsset includes the
/// concerns in Rails). `selling`: the rule's sell side; `action`: its `*_action` (the cap has none: "pause").
pub fn support(kind: Kind, rule: Rule, selling: bool, action: &str) -> Support {
    let flip = matches!(action, "start_selling" | "start_buying");
    // The cap's sell side (base_amount_limited) has nothing to sell on an index bot.
    if rule == Rule::Cap && selling && kind == Kind::Index { return Support::Inapplicable; }
    if rule != Rule::Cap && ((kind == Kind::Index && (selling || flip)) || (kind == Kind::Signal && flip)) { return Support::Inapplicable; }
    if action == "pause" && IMPLEMENTED.contains(&(kind, rule)) { Support::Implemented } else { Support::NotImplemented }
}

/// A rule's `*_action` as Bot::PriceLimitable reads it (and its siblings): absent, false or blank is `pause`; any other value
/// as its text, or its JSON when not a string. `prefix`: the side's prefix (`sell_price_limit`).
pub fn action(settings: &Value, prefix: &str) -> String {
    match settings.get(format!("{prefix}_action").as_str()).filter(|v| !v.is_null()) {
        None | Some(Value::Bool(false)) => "pause".to_string(),
        Some(Value::String(s)) if s.trim().is_empty() => "pause".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Whether a rule that is on decorates the carry window's `started_at`: where Rails reads it (a basket, whatever this build
/// runs: eligibility refuses what it does not) or where this engine runs it (Implemented). An index or signal bot's rule this
/// build does not run stays inert there, as in Rails.
pub fn decorates(kind: Kind, support: Support) -> bool {
    support == Support::Implemented || (kind == Kind::Basket && support == Support::NotImplemented)
}

/// `support` for the rule as `bot_type`'s stored settings hold it.
pub fn stored_support(bot_type: &str, settings: &Value, rule: Rule, selling: bool) -> Support {
    let prefix = TRIGGERS.iter().find(|(r, _)| *r == rule).map(|(_, p)| *p);
    let action = prefix.map_or_else(|| "pause".to_string(), |p| action(settings, &format!("{}{p}", if selling { "sell_" } else { "" })));
    support(Kind::of(bot_type), rule, selling, &action)
}

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
/// D-IDX-SUBJECT: an index bot's rule watches the ticker its settings name (`<prefix>_in_ticker_id`, written when the rule is
/// switched on), if it is one of the venue's available, trading-enabled pairs at the bot's quote: a member, a former member
/// or none. No stored id, or one that is not such a pair, is no ticker (the gate pauses).
fn rule_ticker_quoted(c: &Connection, bot: &Bot, rule: &PriceRule) -> Result<(Option<Ticker>, Option<i64>), EngineError> {
    if bot.bot_type == "Bots::DcaIndex" {
        let Some(id) = rule.ticker_id else { return Ok((None, None)) };
        let ticker = model::ticker_by_id(c, bot.exchange_id, id)?
            .filter(|t| t.available && t.trading_enabled && Some(t.quote_asset_id) == bot.quote_asset_id());
        let decimals = ticker.as_ref().map(|t| t.quote_decimals);
        return Ok((ticker, decimals));
    }
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

/// get_price_limit_condition_met? for the active side, with its two writes, each fenced to the run that read the price:
/// met for the first time stamps `<prefix>_condition_met_at`; unmet after a met_at captures the carry
/// (set_missed_quote_amount, capped as QuoteAmountLimitable caps pending_quote_amount) and clears met_at, in one write.
/// The last price comes through Exchange's 5 s cache, the one the tick's own reference reads. The writes are stamped with
/// the clock read after the price arrives (Rails' `Time.current` there): a read that crosses an interval checkpoint must
/// not stamp, or capture the carry at, the instant before it. Each write also broadcasts the status line from inside it, as
/// Rails does (broadcast_price_limit_info_update: its Rails.cache.fetch, stored in `INFO_KEY`).
pub async fn evaluate<V: Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Evaluated, EngineError> {
    let now = clock.now();
    let Some(rule) = bot.price_rule(bot.selling()).map_err(EngineError::Data)? else { return Ok(Evaluated::Off) };
    if rule.after && rule.met_at_us.is_some() { return Ok(Evaluated::Met { refreshed: None }); }
    let (Some(ticker), decimals) = rule_ticker_quoted(c, bot, &rule)? else { return Ok(Evaluated::Unavailable) };
    let (price, origin) = match last_price(venue, bot, &ticker, now, prices, version).await { Ok(read) => read, Err(e) => return Ok(Evaluated::Failed(e)) };
    let now = clock.now();
    let key = format!("{}_condition_met_at", rule.prefix);
    let side = side_of(bot.selling());
    if price_met(&rule, &price)? {
        if rule.met_at_us.is_some() { return Ok(Evaluated::Met { refreshed: None }); }
        let refreshed = model::run_credential_write(c, bot, &origin, |tx| {
            model::update_transient(tx, bot.id, &[(key.as_str(), json!(iso8601_ms(now)))], now)?;
            if let Some(info) = info_value(&ticker, decimals, &price, true) { fetch_info(tx, bot.id, PRICE, side, now, info)?; }
            model::load_bot(tx, bot.id)
        })?;
        return Ok(Evaluated::Met { refreshed: Some(Box::new(refreshed)) });
    }
    if rule.met_at_us.is_some() {
        model::run_credential_write(c, bot, &origin, |tx| {
            let current = model::load_bot(tx, bot.id)?;
            if current.condition_started_us()?.is_none() {
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
/// A side as the status line's keys name it.
pub const BUYING: &str = "buying";
pub const SELLING: &str = "selling";
/// The side's name: `selling` for the sell side's rule (`sell_*`), else `buying`.
pub fn side_of(selling: bool) -> &'static str { if selling { SELLING } else { BUYING } }
/// A side named by a request (`buying`/`selling`); anything else is no side (the active one is meant).
pub fn side_named(side: &str) -> Option<&'static str> { [BUYING, SELLING].into_iter().find(|s| *s == side) }
/// D-COND-INFO-STORE: what Rails keeps in Rails.cache under `bot_<id>_<rule>_info_<side>` for 20 s, kept in transient_data
/// (a Rust-only key, outside the parity snapshots): `{rule: {side: {base, quote, price, condition_met, expires_us}}}`.
pub const INFO_KEY: &str = "rust_condition_info";
/// The status line's requests the web left for the engine (BroadcastsController#*_info_update → the info job), Rust-only:
/// `{rule: {side: {"version": n, "locale": "en"}}}` (D-INFO-SIDE). Each request bumps its side's version; the engine removes
/// only the version it served.
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

/// Leaves a status-line request for `rule` on `side` (the web's BroadcastsController#*_info_update; `user`: the bot must be
/// theirs). No side: the bot's active side as it is stored now (Rails' job reads the bot's direction). One statement: the
/// version is bumped atomically, so a request the engine claimed earlier never hides this one. False: no such bot.
pub fn want_info(c: &Connection, bot_id: i64, user: Option<i64>, rule: &str, side: Option<&str>, locale: &str) -> Result<bool, EngineError> {
    let k = INFO_WANTED_KEY;
    let named = side.and_then(side_named);
    let side = "coalesce(?4, CASE WHEN json_extract(settings, '$.direction') = 'selling' THEN 'selling' ELSE 'buying' END)";
    let n = c.execute(&format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{k}', json_set(coalesce(json_extract(transient_data, '$.{k}'), '{{}}'), \
                                '$.{rule}.' || {side}, json_object('version', coalesce(json_extract(transient_data, '$.{k}.{rule}.' || {side} || '.version'), 0) + 1, 'locale', ?3))) \
                                WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2) AND json_type(transient_data) = 'object'"),
                       rusqlite::params![bot_id, user, locale, named])?;
    Ok(n == 1)
}

/// One status-line request as the engine claimed it: the bot as read with the request, the side asked for, and the request's
/// version and locale.
#[derive(Clone, Debug)]
pub struct InfoClaim { pub bot: Bot, pub side: &'static str, pub version: i64, pub locale: Option<String> }

/// The bots with a waiting request for `rule`, each side claimed as the bot is now. A bot that cannot be loaded (eligibility
/// lists it as unreadable), or whose request is not `{version: integer}`, is skipped and logged: a status line never stops
/// the engine.
pub fn info_claims(c: &Connection, rule: &str) -> Result<Vec<InfoClaim>, EngineError> {
    let mut rows = vec![];
    for side in [BUYING, SELLING] {
        let path = format!("$.{INFO_WANTED_KEY}.{rule}.{side}");
        let mut s = c.prepare(&format!("SELECT id, json_extract(transient_data, '{path}.version'), json_extract(transient_data, '{path}.locale') FROM bots \
                                        WHERE json_valid(transient_data) AND json_type(transient_data, '{path}') IS NOT NULL"))?;
        for r in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, side, r.get::<_, rusqlite::types::Value>(1)?, r.get::<_, rusqlite::types::Value>(2)?)))? { rows.push(r?); }
    }
    rows.sort_by_key(|(id, side, ..)| (*id, *side == SELLING));
    let mut out = vec![];
    for (id, side, version, locale) in rows {
        let rusqlite::types::Value::Integer(version) = version else { super::log(&format!("[engine] bot {id}: unreadable status-line request; skipped")); continue };
        let locale = match locale { rusqlite::types::Value::Text(l) => Some(l), _ => None };
        match model::load_bot(c, id) {
            Ok(bot) => out.push(InfoClaim { bot, side, version, locale }),
            Err(err @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(err),
            Err(err) => super::log(&format!("[engine] bot {id}: its status-line request waits, the bot is unreadable: {err:?}")),
        }
    }
    Ok(out)
}

/// Removes the request of (`rule`, `side`) only if it is still the claimed version: a newer one survives for the next pass.
pub fn ack_info(tx: &Connection, bot_id: i64, rule: &str, side: &str, version: i64) -> Result<(), EngineError> {
    let k = INFO_WANTED_KEY;
    tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{k}.{rule}.{side}') WHERE id = ?1 \
                         AND json_extract(transient_data, '$.{k}.{rule}.{side}.version') IS ?2"), rusqlite::params![bot_id, version])?;
    for path in [format!("$.{k}.{rule}"), format!("$.{k}")] {
        tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '{path}') WHERE id = ?1 AND json_extract(transient_data, '{path}') = '{{}}'"), [bot_id])?;
    }
    Ok(())
}

/// What the status line answers for a price rule at `price`, without writing anything: an `after` rule with a stored met_at
/// is met, otherwise the price decides (get_price_limit_condition_met?'s answer, its writes left out).
pub fn judge(rule: &PriceRule, price: &BigDec) -> Result<bool, EngineError> {
    if rule.after && rule.met_at_us.is_some() { return Ok(true); }
    price_met(rule, price)
}

/// Bot::BroadcastPriceLimitInfoUpdateJob (broadcast_price_limit_info_update, price_limitable.rb:224-250) for one claimed
/// request, on any status, for the claimed side's rule: its ticker, its last price, the answer (`judge`), then the cached
/// info. Any failure leaves no info, as Rails broadcasts nothing then; the request is served either way. Some(info): what
/// the line shows.
/// D-INFO-READONLY: Rails' job runs get_price_limit_condition_met? with its writes, so a page view stamps or clears met_at and
/// captures the carry, a stopped bot's included, and that changes later trades. Here the status line never changes trading
/// state: the only writes are the info cache (`fetch_info`) and the served request (`ack_info`).
/// Err(RunFenced): the bot's configuration changed since the claim; nothing is kept and the request stays.
/// Err(CredentialsChanged): the key changed during the read; the request stays.
pub async fn info<V: crate::venue::Venue>(c: &Connection, venue: &V, claim: &InfoClaim, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Option<Value>, EngineError> {
    let (bot, side) = (&claim.bot, claim.side);
    let venue = &crate::venue::Handle::for_bot(venue, c, bot)?;
    let served = |c: &Connection| -> Result<Option<Value>, EngineError> {
        let tx = model::immediate(c)?; ack_info(&tx, bot.id, PRICE, side, claim.version)?; tx.commit()?; Ok(None)
    };
    // A rule that is off, or one this build does not run on the bot's kind (support), renders nothing (the partial's own
    // guard), so no info is kept for it.
    let Some(rule) = bot.price_rule(side == SELLING).map_err(EngineError::Data)? else { return served(c) };
    let (Some(ticker), decimals) = rule_ticker_quoted(c, bot, &rule)? else { return served(c) };
    let Ok((price, origin)) = last_price(venue, bot, &ticker, clock.now(), prices, version).await else { return served(c) };
    let Some(info) = info_value(&ticker, decimals, &price, judge(&rule, &price)?) else { return served(c) };
    let now = clock.now();
    model::credential_write(c, &origin, |tx| {
        let current = model::load_bot(tx, bot.id)?;
        // Settings, direction or the rule's met_at changed since the claim: the answer is for a rule that no longer is.
        let met_at = |b: &Bot| b.transient.get(format!("{}_condition_met_at", rule.prefix).as_str()).cloned();
        if (&current.bot_type, current.exchange_id, &current.settings, met_at(&current)) != (&bot.bot_type, bot.exchange_id, &bot.settings, met_at(bot)) { return Err(EngineError::RunFenced); }
        let shown = fetch_info(tx, bot.id, PRICE, side, now, info)?;
        ack_info(tx, bot.id, PRICE, side, claim.version)?;
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
    match e { VenueError::Rejected(m) | VenueError::Unauthorized(m) | VenueError::Coded { errors: m, .. } if m.first().is_some_and(|m| m.starts_with("Wrong ") || m.starts_with("invalid value for BigDecimal") || m.starts_with("unreadable ")) => Some(crate::ruby::to_sentence(m)), _ => None }
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
