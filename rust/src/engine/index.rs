//! Bots::DcaIndex's composition, derived on every tick before anything is sized (Bots::DcaIndex#execute_action,
//! dca_index.rb:126-143; IndexAllocatable#derive_composition, index_allocatable.rb:61-123) from the data-api `indices` row
//! (MarketData.get_top_coins, market_data.rb:117-143), and written as Rails writes it (basket::write_members). The buy leg is
//! the basket's (tick::buy). It never sells: a member that leaves is only marked out of the index.
use crate::venue::Attributed;
use super::model::{self, Bot, Ticker};
use super::tick::PriceCache;
use super::{basket, Clock, EngineError};
use crate::ruby::{float_sum, to_sentence};
use crate::venue::{PriceSide, Venue, VenueError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Why the composition was not refreshed: Rails' Failure (the tick fails, execution_failed, no row), or a transport raise
/// from a newcomer's price probe (retry_on).
#[derive(Debug, PartialEq)]
pub enum Refusal { Failure(String), Transient(String) }

/// Bot::Composition::Weightable.blend: `cap / Σcap × (1 − f) + (1 / n) × f`, in Float, with Ruby's compensated sum; a zero
/// total weighs everything equally.
pub fn blend(caps: &[f64], flattening: f64) -> Vec<f64> {
    if caps.is_empty() { return vec![]; }
    let equal = 1.0 / caps.len() as f64;
    let total = float_sum(caps);
    let f = flattening.clamp(0.0, 1.0);
    caps.iter().map(|cap| {
        let share = if total > 0.0 { cap / total } else { equal };
        (share * (1.0 - f)) + (equal * f)
    }).collect()
}

/// `value.to_f` on a JSON weight: a number, or a numeric string (String#to_f; data-api sends numbers).
fn to_f(v: Option<&Value>) -> f64 {
    match v { Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0), Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0), _ => 0.0 }
}

/// MarketData.get_top_coins' data-api branch: the ranked ids with the asset's market_cap, else the index's published weight;
/// a coin with neither, or with no local asset, is skipped.
fn top_coins(c: &Connection, ids: &[String], weights: &Map<String, Value>) -> Result<Vec<(String, f64)>, EngineError> {
    let mut out = vec![];
    for id in ids {
        let cap: Option<Option<f64>> = c.query_row("SELECT market_cap FROM assets WHERE external_id = ?1", [id], |r| r.get(0)).optional()?;
        let Some(cap) = cap else { continue };
        let mut value = cap.unwrap_or(0.0);
        if value <= 0.0 { value = to_f(weights.get(id)); }
        if value > 0.0 { out.push((id.clone(), value)); }
    }
    Ok(out)
}

/// Ticker#priced?(side) through Rails' 5 s price cache (Exchange#get_*_price fills it), so Step 1's read of a newcomer reuses
/// the probe's answer. A zero or failed price is false; a transport failure raises (retry_on); a rejected key raises
/// Exchange#raise_on_invalid_key!'s error.
async fn priced<V: Venue + Attributed>(venue: &V, bot: &Bot, t: &Ticker, side: PriceSide, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<bool, Refusal> {
    let key = (bot.exchange_id, t.id, side);
    if prices.get(key, clock.now(), version).is_some() { return Ok(true); }
    let result=venue.price_result(t,side).await;
    let origin=result.origin().clone();
    match result.value {
        Ok(p) => { prices.put_result(key,clock.now(),model::Produced::new(p.clone(),origin)); Ok(p.is_positive()) }
        Err(VenueError::Transient(m)) => Err(Refusal::Transient(m)),
        // ponytail: Exchange#invalid_key_error? also reads the HTTP status; Alpaca's own 401 says "unauthorized", and an HTML
        // 401 is "HTTP 401". Another 401 body would read as unpriced here.
        Err(VenueError::Rejected(e)) if e.iter().any(|m| m.contains("unauthorized") || m == "HTTP 401") =>
            Err(Refusal::Failure(format!("{} rejected the API key: {}", t.exchange_name, to_sentence(&e)))),
        Err(_) => Ok(false),
    }
}

/// Bot::Composition::Allocatable#refresh_composition for an index bot: derive_composition, then update_bot_index_assets.
pub async fn refresh_composition<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache) -> Result<Result<(), Refusal>, EngineError> {
    let handle=crate::venue::Handle::for_bot(venue,c,bot)?;
    let version=handle.producer();
    let venue=&handle;
    refresh_composition_captured(c, venue, bot, clock, prices, &version).await
}
pub(crate) async fn refresh_composition_captured<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Result<(), Refusal>, EngineError> {
    let before = super::placement::composition_snapshot(c, bot)?;
    let provider_before = super::provider::fingerprint(c)?;
    if provider_before.is_none() { return Ok(Err(Refusal::Failure("Index provider not configured".into()))); }
    let category = bot.index_category_id().unwrap_or_default();
    let row: Option<(Option<String>, Option<String>, Option<String>)> = c.query_row(
        "SELECT source, top_coins, weights FROM indices WHERE external_id = ?1 ORDER BY id LIMIT 1", [category],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    let source_before = row.clone();
    let Some((source, top, weights)) = row else { return Ok(Err(Refusal::Failure("Index not found".into()))) };
    let top: Vec<String> = top.and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()).unwrap_or_default()
        .into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let weights: Map<String, Value> = weights.and_then(|w| serde_json::from_str::<Value>(&w).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    // #bounded_universe_size: a data-api index publishes its whole membership. #effective_num_coins: all of it when holding the
    // whole universe, else num_coins. derive_composition asks for the bounded universe, else 3 × N capped at 250.
    let bounded = (source.as_deref() == Some("deltabadger") && !top.is_empty()).then_some(top.len() as i64);
    let n = match bounded { Some(b) if bot.hold_all() => b, _ => bot.num_coins().unwrap_or(0) };
    let limit = bounded.unwrap_or(n.saturating_mul(3).min(250)).clamp(0, 1000) as usize;
    if n > 1000 || top.len() > 1000 { return Ok(Err(Refusal::Failure("index exceeds 1000 members".into()))); }
    let ranked = top_coins(c, &top[..top.len().min(limit)], &weights)?;

    // The candidates: available, trading-enabled tickers at the bot's quote, by their base asset's external id (index_by: the
    // last one wins).
    let mut s = c.prepare("SELECT t.id, a.external_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                           WHERE t.exchange_id = ?1 AND t.quote_asset_id = ?2 AND t.available = 1 AND t.trading_enabled = 1 ORDER BY t.id")?;
    let mut by_external: HashMap<String, i64> = HashMap::new();
    for r in s.query_map(params![bot.exchange_id, bot.quote_asset_id()], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (id, ext) = r?;
        if let Some(ext) = ext.filter(|e| !e.is_empty()) { by_external.insert(ext, id); }
    }
    // Incumbents keep their seat without a probe (keyed on ticker_id); everyone else must show a live price on the side the
    // order would use. The tick only gets here with the market open, so #probe_side is the ask, or the last for a limit bot.
    let mut s = c.prepare("SELECT ticker_id FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1")?;
    let incumbents: HashSet<i64> = s.query_map([bot.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let side = if bot.limit_distance().is_some() { PriceSide::Last } else { PriceSide::Ask };
    let mut chosen: Vec<(i64, i64, f64)> = vec![];
    let mut priced_tickers = vec![];
    for (coin, cap) in ranked {
        if chosen.len() as i64 >= n { break; }
        let Some(&ticker_id) = by_external.get(&coin) else { continue };
        let ticker = model::ticker_by_id(c, bot.exchange_id, ticker_id)?
            .ok_or_else(|| EngineError::Data(format!("bot {}: ticker {ticker_id} vanished mid-derivation", bot.id)))?;
        if !incumbents.contains(&ticker_id) {
            match priced(venue, bot, &ticker, side, clock, prices, version).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(r) => return Ok(Err(r)),
            }
        }
        chosen.push((ticker.base_asset_id, ticker_id, cap));
        priced_tickers.push(ticker);
    }
    if chosen.is_empty() {
        return Ok(Err(Refusal::Failure(format!("No matching coins found on {} for the index", model::exchange_name(c, bot)?))));
    }
    // Rails builds an insertion-ordered asset hash BEFORE blending; a repeated key replaces its
    // value without moving its first position. It then assigns that weight to every chosen occurrence.
    let mut caps: Vec<(i64, f64)> = vec![];
    for (asset, _, cap) in &chosen {
        if let Some((_, value)) = caps.iter_mut().find(|(id, _)| id == asset) { *value = *cap; }
        else { caps.push((*asset, *cap)); }
    }
    let weights = blend(&caps.iter().map(|(_, cap)| *cap).collect::<Vec<_>>(), bot.allocation_flattening().unwrap_or(0.0));
    let by_asset: HashMap<i64, f64> = caps.into_iter().map(|(id, _)| id).zip(weights).collect();
    let members: Vec<(i64, i64, f64)> = chosen.iter().map(|(asset, ticker, _)| (*asset, *ticker, by_asset[asset])).collect();
    model::credential_write(c, version, |tx| {
        let current = model::load_bot(tx, bot.id)?;
        let source_now: Option<(Option<String>, Option<String>, Option<String>)> = tx.query_row(
            "SELECT source, top_coins, weights FROM indices WHERE external_id=?1 ORDER BY id LIMIT 1", [category],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let mut changed = current.status != bot.status || super::placement::composition_snapshot(tx, &current)? != before || source_now != source_before || super::provider::fingerprint(tx)? != provider_before;
        for ticker in &priced_tickers { changed |= model::ticker_by_id(tx, bot.exchange_id, ticker.id)?.as_ref() != Some(ticker); }
        if changed { return Ok(Err(Refusal::Transient("index changed during price probes".into()))); }
        let mut reasons = super::eligibility::bot_reasons(tx, &current)?;
        super::eligibility::member_reasons(tx, &current, true, &members.iter().map(|m| m.0).collect::<Vec<_>>(), &mut reasons)?;
        if !reasons.is_empty() { return Ok(Err(Refusal::Failure(reasons.join("; ")))); }
        basket::write_members(tx, bot.id, &members, clock.now())?;
        // Any unexpected ineligibility rolls the transaction back (not a nested Ok(Err)).
        super::eligibility::check_install(tx)?.refusal().map_err(|r| EngineError::Data(r.reason()))?;
        Ok(Ok(()))
    })
}

/// The pairs an index bot's tick may price: every available, trading-enabled ticker at its quote whose asset the index ranks
/// (incumbents at Step 1, newcomers at their probe). For a parity copy (parity::plan_copy).
pub fn candidate_pairs(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let top: Option<Option<String>> = c.query_row("SELECT top_coins FROM indices WHERE external_id = ?1 ORDER BY id LIMIT 1",
        [bot.index_category_id().unwrap_or_default()], |r| r.get(0)).optional()?;
    let top = top.flatten().filter(|t| t.trim_start().starts_with('[')).unwrap_or_else(|| "[]".into());
    let mut s = c.prepare("SELECT t.ticker FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 AND t.quote_asset_id = ?2 \
                           AND t.available = 1 AND t.trading_enabled = 1 AND a.external_id IN (SELECT value FROM json_each(?3)) ORDER BY t.id")?;
    let pairs = s.query_map(params![bot.exchange_id, bot.quote_asset_id(), top], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(pairs)
}
