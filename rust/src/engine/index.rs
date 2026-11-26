//! Bots::DcaIndex's composition, derived on every tick before anything is sized (Bots::DcaIndex#execute_action,
//! IndexAllocatable#derive_composition) from the data-api `indices` row
//! (MarketData.get_top_coins) as this engine ranked it (`ranked_row`), and written as 2.x writes it (basket::write_members). The buy leg is
//! the basket's (tick::buy). It never sells: a member that leaves is only marked out of the index.
use crate::venue::Attributed;
use super::model::{self, Bot, Ticker};
use super::tick::PriceCache;
use super::{basket, Clock, EngineError};
use crate::v2::{float_sum, to_sentence};
use crate::venue::{PriceSide, Venue, VenueError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Why the composition was not refreshed: 2.x's Failure (the tick fails, execution_failed, no row), or a transport raise
/// from a newcomer's price probe (retry_on).
#[derive(Debug, PartialEq)]
pub enum Refusal { Failure(String), Transient(String) }

/// What the closing re-check returns when anything the derivation read moved during its price probes (no 2.x equivalent).
pub const CHANGED: &str = "index changed during price probes";

/// Bot::ResyncIndexCompositionJob's pending run, as a durable per-bot marker in `bots.transient_data` (2.x never reads a
/// `rust_*` key and merges transient_data, so it survives every 2.x write and a handback, where
/// EngineLease.adopt_handback! enqueues the job for it). `{"n", "at"}`: every trigger bumps `n`, so two triggers in one
/// millisecond still differ, and a run clears it only when it is still the value that run read.
pub const RESYNC_KEY: &str = "rust_index_resync";

/// PLAN index-as-ingredient §4: the bots that follow an index, of either stored spelling (an SQL condition on `bots`).
pub const FOLLOWS_INDEX_SQL: &str = "(type = 'Bots::DcaIndex' OR (type = 'Bots::DcaMultiAsset' AND json_valid(settings) AND json_type(settings, '$.index_groups') = 'array'))";

/// Marks every index bot `filter` selects (an SQL condition on `bots`; `?1` is `id`) for a resync, in the caller's
/// transaction. Returns how many were marked.
fn mark(c: &Connection, filter: &str, id: Option<i64>, now: chrono::DateTime<chrono::Utc>) -> Result<usize, EngineError> {
    let at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Ok(c.execute(&format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{RESYNC_KEY}', \
                           json_object('n', coalesce(json_extract(transient_data, '$.{RESYNC_KEY}.n'), 0) + 1, 'at', ?2)) \
                           WHERE {FOLLOWS_INDEX_SQL} AND json_type(transient_data) = 'object' AND {filter}"), params![id, at])?)
}

/// Bots::DcaIndex#start and #resync_index_composition (an index-definition or exchange change): this bot.
pub fn mark_resync(c: &Connection, bot_id: i64, now: chrono::DateTime<chrono::Utc>) -> Result<usize, EngineError> { mark(c, "id = ?1", Some(bot_id), now) }

/// Index::SyncFromCoingeckoJob#recheck_index_bots: every index bot neither deleted nor
/// archived that has an exchange.
pub fn mark_resync_all(c: &Connection, now: chrono::DateTime<chrono::Utc>) -> Result<usize, EngineError> {
    use crate::enums::BotStatus::{Archived, Deleted};
    mark(c, &format!("status NOT IN ({}, {}) AND exchange_id IS NOT NULL", Deleted as i64, Archived as i64), None, now)
}

/// Every index bot with a pending resync, whatever its status: (id, the marker's JSON text).
pub fn resync_marked(c: &Connection) -> Result<Vec<(i64, String)>, EngineError> {
    let mut s = c.prepare(&format!("SELECT id, json_extract(transient_data, '$.{RESYNC_KEY}') FROM bots WHERE {FOLLOWS_INDEX_SQL} \
                                    AND json_type(transient_data, '$.{RESYNC_KEY}') IS NOT NULL ORDER BY id"))?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Clears the marker only if it is still `read`: a trigger that landed meanwhile keeps it, for another run.
pub fn clear_resync(c: &Connection, bot_id: i64, read: &str) -> Result<bool, EngineError> {
    Ok(c.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{RESYNC_KEY}') WHERE id = ?1 \
                           AND json_extract(transient_data, '$.{RESYNC_KEY}') = ?2"), params![bot_id, read])? == 1)
}

/// Bot::Composition::Weightable.blend: `cap / Σcap × (1 − f) + (1 / n) × f`, in Float, with 2.x's compensated sum; a zero
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
pub(crate) fn to_f(v: Option<&Value>) -> f64 {
    match v { Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0), Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0), _ => 0.0 }
}

/// The `indices` row an index bot following `external_id` reads (D-INDEX-UNATTESTED-HELD, D-INDEX-NEWEST-RANKING): among
/// the rows with that id ranked in `rust_index_rankings` by the current provider (provider::ranking_provider) under the
/// current configuration (provider::fingerprint), the newest `ranked_at`, then the lowest id. A row with no ranking, or
/// another provider's or configuration's, is never selected.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked { pub id: i64, pub source: Option<String>, pub top_coins: Option<String>, pub weights: Option<String>, pub provider: String, pub ranked_at: String }

pub fn ranked_row(c: &Connection, external_id: &str) -> Result<Option<Ranked>, EngineError> {
    let Some(provider) = super::provider::ranking_provider(c)? else { return Ok(None) };
    let Some(fingerprint) = super::provider::fingerprint(c)? else { return Ok(None) };
    // A copy no Rust migration ran on (a page-grid fixture the web reads) holds no ranking: nothing is selected.
    let table: bool = c.prepare_cached("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rust_index_rankings')")?.query_row([], |r| r.get(0))?;
    if !table { return Ok(None); }
    Ok(c.query_row("SELECT i.id, i.source, i.top_coins, i.weights, r.provider, r.ranked_at FROM indices i JOIN rust_index_rankings r ON r.index_id = i.id \
                    WHERE i.external_id = ?1 AND r.provider = ?2 AND r.fingerprint = ?3 ORDER BY r.ranked_at DESC, i.id LIMIT 1",
        params![external_id, provider, fingerprint],
        |r| Ok(Ranked { id: r.get(0)?, source: r.get(1)?, top_coins: r.get(2)?, weights: r.get(3)?, provider: r.get(4)?, ranked_at: r.get(5)? })).optional()?)
}

/// `ranked_at` as every writer stores it (fixed width, so the text orders as the time).
pub fn ranked_at_text(at: chrono::DateTime<chrono::Utc>) -> String { at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true) }

/// A stored `ranked_at` as a grouped composition records it (`rust_composed_of.ranked`) and `composition::current` compares
/// it: re-serialised from the parsed time in `ranked_at_text`'s one form (27 bytes for years 0..=9999), so the record has a
/// known maximum length whatever the row holds. None: unparsable or outside those years.
pub fn recorded_ranked_at(text: &str) -> Option<String> {
    use chrono::Datelike;
    match crate::codec::parse_time(text) { Ok(at) if (0..=9999).contains(&at.year()) => Some(ranked_at_text(at)), _ => None }
}

/// Attests `index_id` as ranked by `provider` under `fingerprint` at `at`, in the caller's write unit (the same one that
/// writes the row's `top_coins`/`weights`).
pub fn record_ranking(c: &Connection, index_id: i64, provider: &str, fingerprint: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), EngineError> {
    c.execute("INSERT INTO rust_index_rankings (index_id, provider, fingerprint, ranked_at) VALUES (?1, ?2, ?3, ?4) \
               ON CONFLICT(index_id) DO UPDATE SET provider = excluded.provider, fingerprint = excluded.fingerprint, ranked_at = excluded.ranked_at",
        params![index_id, provider, fingerprint, ranked_at_text(at)])?;
    Ok(())
}

/// May compose (D-INDEX-FRESH-PER-ROW): ranked within `staleness::bound_secs(24)` (49 h) of `now`. Unreadable is not usable.
pub fn usable(r: &Ranked, now: chrono::DateTime<chrono::Utc>) -> bool {
    crate::codec::parse_time(&r.ranked_at).is_ok_and(|at| (now - at).num_seconds() <= super::staleness::bound_secs(24))
}

/// The INDICES job's slot at `now`: its schedule's latest fire (daily 10:30 UTC).
pub fn ranking_slot(now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    crate::jobs::reference::specs().into_iter().find(|s| s.name == crate::jobs::reference::INDICES)
        .and_then(|s| s.schedule).map_or(now, |s| s.last_fire(now))
}

/// The bot needs a refresh of a ranking it follows: an index bot with a followed id that has no usable ranking (absent,
/// unattested, another configuration's, or past 49 h).
pub fn ranking_wanted(c: &Connection, bot: &Bot, now: chrono::DateTime<chrono::Utc>) -> Result<bool, EngineError> {
    Ok(!wanted_ids(c, bot, now)?.is_empty())
}

/// The followed ids, in stored order, that `ranking_wanted` wants ranked.
fn wanted_ids(c: &Connection, bot: &Bot, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<String>, EngineError> {
    let mut out = vec![];
    if !bot.has_groups() { return Ok(out); }
    for id in bot.index_external_ids() {
        if retired(c, &id)? || ranked_row(c, &id)?.is_some_and(|r| usable(&r, now)) { continue; }
        if !out.contains(&id) { out.push(id); }
    }
    Ok(out)
}

/// What `EngineEvent::RankingMissing` is deduped on, per process: (external id, fingerprint, slot).
pub type WakeKey = (String, String, i64);

/// The followed ids to wake the INDICES job for, each once per (external id, fingerprint, slot) in `sent`; empty when the
/// bot wants nothing or every id was already announced in this slot under this configuration. Older slots' keys are dropped.
pub fn ranking_wake(c: &Connection, bot: &Bot, now: chrono::DateTime<chrono::Utc>, sent: &mut HashSet<WakeKey>) -> Result<Vec<String>, EngineError> {
    let wanted = wanted_ids(c, bot, now)?;
    if wanted.is_empty() { return Ok(wanted); }
    let slot = ranking_slot(now).timestamp();
    sent.retain(|k| k.2 == slot);
    let fingerprint = super::provider::fingerprint(c)?.unwrap_or_default();
    Ok(wanted.into_iter().filter(|id| sent.insert((id.clone(), fingerprint.clone(), slot))).collect())
}

/// A17-4 r18 (D-INDEX-RETIRED-KEPT): the composition's refusal of a retired index it has no provenance for.
pub const RETIRED: &str = "Index retired";

/// The positive provenance of a bot's membership (`bots.transient_data`, Rust-only): the external id this engine's index
/// composition last wrote it from, set in that write's own transaction. Removed by every other write that changes what the
/// bot follows or holds (the index switch, an index settings write) and by a new ownership's claim; never inferred.
pub const COMPOSED_KEY: &str = "rust_composed_index";

/// Whether `external_id` is retired by the install's current provider (rust_index_retired). False with no provider, and
/// on a copy with no Rust migrations (a page-grid fixture): nothing there was ever retired by this engine.
pub fn retired(c: &Connection, external_id: &str) -> Result<bool, EngineError> {
    let Some(provider) = super::provider::ranking_provider(c)? else { return Ok(false) };
    let table: bool = c.prepare_cached("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rust_index_retired')")?.query_row([], |r| r.get(0))?;
    if !table { return Ok(false); }
    Ok(c.prepare_cached("SELECT EXISTS (SELECT 1 FROM rust_index_retired WHERE external_id = ?1 AND provider = ?2)")?.query_row(params![external_id, provider], |r| r.get(0))?)
}

/// Whether the bot's current membership of the index `external_id` is this engine's composition of it. One reader for
/// both spellings (PLAN index-as-ingredient §3.3): a `Bots::DcaIndex` row by COMPOSED_KEY, a bot stored with
/// `index_groups` by its `rust_bot_group_members` rows written by a composition here.
pub fn composed_here(c: &Connection, bot: &Bot, external_id: &str) -> Result<bool, EngineError> {
    if !bot.stored_groups() { return Ok(bot.transient.get(COMPOSED_KEY).and_then(Value::as_str) == Some(external_id)); }
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM rust_bot_group_members WHERE bot_id = ?1 AND index_external_id = ?2 AND composed = 1)",
                   params![bot.id, external_id], |r| r.get(0))?)
}

/// D-INDEX-RETIRED-KEPT: some followed id is retired and the bot's membership of it is not this engine's composition.
pub fn retired_uncomposed(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    for id in bot.index_external_ids() { if retired(c, &id)? && !composed_here(c, bot, &id)? { return Ok(true); } }
    Ok(false)
}
/// Some followed id is retired.
pub fn any_retired(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    for id in bot.index_external_ids() { if retired(c, &id)? { return Ok(true); } }
    Ok(false)
}
/// Some followed id's membership is not this engine's composition.
pub fn any_uncomposed(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    for id in bot.index_external_ids() { if !composed_here(c, bot, &id)? { return Ok(true); } }
    Ok(false)
}

/// The tickers that keep their seat in the index `external_id` without a probe: a `Bots::DcaIndex` row's `in_index`
/// members; for a bot stored with `index_groups`, that group's rows whose ticker is still the asset's `in_index` one.
pub fn incumbents(c: &Connection, bot: &Bot, external_id: &str) -> Result<HashSet<i64>, EngineError> {
    if !bot.stored_groups() {
        let mut s = c.prepare("SELECT ticker_id FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1")?;
        let rows = s.query_map([bot.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        return Ok(rows);
    }
    let mut s = c.prepare("SELECT g.ticker_id FROM rust_bot_group_members g JOIN bot_index_assets m ON m.bot_id = g.bot_id AND m.asset_id = g.asset_id \
                           AND m.ticker_id = g.ticker_id AND m.in_index = 1 WHERE g.bot_id = ?1 AND g.index_external_id = ?2")?;
    let rows = s.query_map(params![bot.id, external_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// A validated catalogue of `provider` no longer lists `external_id`, in the caller's write unit: the retirement, every
/// ranking of the id dropped (the old ranking never composes after a re-listing), and one `index_retired` line per bot
/// following it (category bots neither deleted nor archived, with an exchange). Nothing when it already is retired.
pub fn retire(c: &Connection, external_id: &str, provider: &str, following: &[i64], now: chrono::DateTime<chrono::Utc>) -> Result<bool, EngineError> {
    if c.execute("INSERT OR IGNORE INTO rust_index_retired (external_id, provider, retired_at) VALUES (?1, ?2, ?3)",
                 params![external_id, provider, ranked_at_text(now)])? == 0 { return Ok(false); }
    c.execute("DELETE FROM rust_index_rankings WHERE index_id IN (SELECT id FROM indices WHERE external_id = ?1)", [external_id])?;
    for bot in following {
        c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, message, details, created_at) VALUES (?1, 'index_retired', 1, NULL, ?2, ?3)",
            params![bot, serde_json::json!({ "index": external_id }).to_string(), crate::codec::format_time(now)])?;
    }
    Ok(true)
}

/// A later validated catalogue of `provider` lists `external_id` again: the retirement goes, in the caller's unit. The id has
/// no ranking (`retire` dropped them), so its bots are held and woken as any unranked index until a new ranking commits.
pub fn unretire(c: &Connection, external_id: &str, provider: &str) -> Result<bool, EngineError> {
    Ok(c.execute("DELETE FROM rust_index_retired WHERE external_id = ?1 AND provider = ?2", params![external_id, provider])? > 0)
}

/// The asset classifier owns taxonomy, including overrides for legacy catalogue rows.
pub use crate::asset_taxonomy::stablecoin;

/// MarketData.get_top_coins' data-api branch: the ranked ids with the asset's market_cap, else the index's published weight;
/// a coin with neither, or with no local asset, is skipped. On a `coingecko` ranking (`own_caps`) the ranking's own caps
/// only, as 2.x's live answer weighs (`coin['market_cap'].to_f`): `weights[id]` with to_f
/// (missing or null: 0.0), never the asset's cap, never skipped for zero. Stablecoins and ids without a local asset are
/// skipped on both. Ok(None): a negative or non-finite cap (D-CG-INVALID-CAP-HELD; its writer never stores one).
fn top_coins(c: &Connection, ids: &[String], weights: &Map<String, Value>, own_caps: bool) -> Result<Option<Vec<(String, f64)>>, EngineError> {
    let mut out = vec![];
    for id in ids {
        let asset: Option<(Option<f64>, Option<String>)> = c.query_row("SELECT market_cap, category FROM assets WHERE external_id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((cap, category)) = asset else { continue };
        if stablecoin(category.as_deref(), id) { continue; }
        if own_caps {
            let value = to_f(weights.get(id));
            if !value.is_finite() || value < 0.0 { return Ok(None); }
            out.push((id.clone(), value));
            continue;
        }
        let mut value = cap.unwrap_or(0.0);
        if value <= 0.0 { value = to_f(weights.get(id)); }
        if value > 0.0 { out.push((id.clone(), value)); }
    }
    Ok(Some(out))
}

/// Ticker#priced?(side) through 2.x's 5 s price cache (Exchange#get_*_price fills it), so Step 1's read of a newcomer reuses
/// the probe's answer, in 2.x's order: a price is priced when positive; a rejected key (HTTP 401, or the venue's own
/// invalid-key text) raises Exchange#raise_on_invalid_key!'s error; a transport failure raises only where the client raises
/// it (Clients::Alpaca, `transport_raises`: retry_on), and honeymaker's Failure reads as unpriced; anything else is unpriced.
async fn priced<V: Venue + Attributed>(venue: &V, bot: &Bot, t: &Ticker, side: PriceSide, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<bool, Refusal> {
    let key = (bot.exchange_id, t.id, side);
    // A grouped bot's second group asks the cache for a pair the first one probed: only a positive answer is priced.
    if let Some(p) = prices.get_for(key, t, clock.now(), version) { return Ok(!bot.stored_groups() || p.is_positive()); }
    let result=venue.price_result(t,side).await;
    let origin=result.origin().clone();
    let rejected = |e: &[String]| Refusal::Failure(format!("{} rejected the API key: {}", t.exchange_name, to_sentence(e)));
    match result.value {
        Ok(p) => { prices.put_result(key,t,clock.now(),model::Produced::new(p.clone(),origin)); Ok(p.is_positive()) }
        Err(VenueError::Unauthorized(e)) => Err(rejected(&e)),
        Err(VenueError::Rejected(e)) if venue.rules().is_invalid_key(&e) => Err(rejected(&e)),
        Err(VenueError::Transient(m)) if venue.rules().transport_raises => Err(Refusal::Transient(m)),
        Err(_) => Ok(false),
    }
}

/// IndexAllocatable#probe_side: the last for a limit bot; else the ask while Exchange#market_open?(tickers: [ticker]) — always
/// on a venue without market hours and for a crypto ticker on Alpaca, otherwise Alpaca's clock, read once per derivation
/// (2.x's one-minute clock cache; a tick or a rebalance passed its own open gate, so `open` starts known): a rejected key raises, a transport failure raises (retry_on), and any other failure
/// reads as nil, which is open.
async fn probe_side<V: Venue>(venue: &V, bot: &Bot, t: &Ticker, open: &mut Option<bool>) -> Result<PriceSide, Refusal> {
    if bot.limit_distance().is_some() { return Ok(PriceSide::Last); }
    if !venue.rules().market_hours || t.crypto { return Ok(PriceSide::Ask); }
    let open = match *open {
        Some(o) => o,
        None => {
            let o = match venue.clock().await {
                Ok(crate::venue::ClockAnswer::Body(b)) => serde_json::from_str::<Value>(&b).map_or(true, |v| v.get("is_open") == Some(&Value::Bool(true))),
                Ok(crate::venue::ClockAnswer::Failed { status, message }) if status == Some(401) || venue.rules().is_invalid_key(std::slice::from_ref(&message)) =>
                    return Err(Refusal::Failure(format!("{} rejected the API key: {message}", t.exchange_name))),
                Ok(crate::venue::ClockAnswer::Failed { .. }) => true,
                Err(VenueError::Transient(m) | VenueError::Ambiguous(m)) => return Err(Refusal::Transient(m)),
                Err(e) if e.rejection().is_some_and(|e| venue.rules().is_invalid_key(e)) =>
                    return Err(Refusal::Failure(format!("{} rejected the API key: {}", t.exchange_name, to_sentence(e.rejection().unwrap_or_default())))),
                Err(_) => true,
            };
            *open = Some(o);
            o
        }
    };
    Ok(if open { PriceSide::Ask } else { PriceSide::Last })
}

/// Bot::Composition::Allocatable#refresh_composition for an index bot: derive_composition, then update_bot_index_assets.
pub async fn refresh_composition<V: Venue>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache) -> Result<Result<(), Refusal>, EngineError> {
    let handle=crate::venue::Handle::for_bot(venue,c,bot)?;
    let version=handle.producer();
    let venue=&handle;
    Ok(refresh_composition_captured(c, venue, bot, clock, prices, &version).await?.map(|_| ()))
}
/// The same, returning the composition as written (a grouped tick sizes from it, PLAN index-as-ingredient §3.4).
pub(crate) async fn refresh_composition_captured<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Result<Value, Refusal>, EngineError> {
    refresh_owned(c, venue, bot, clock, prices, version, None, Some(true)).await
}

/// Bot::Composition::Rebalancer#before_rebalance for an index bot: the same refresh for a bot in any status but deleted or
/// archived (rebalancing runs while the DCA schedule is stopped), so no DCA run is required to own it. `claimed` is the
/// composition the rebalance pass read before its first await: the write lock compares against it, never against a copy
/// read after the venue was asked. Ok carries the composition as written, read under the same lock.
pub(crate) async fn refresh_for_rebalance<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, claimed: &Value, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Result<Value, Refusal>, EngineError> {
    refresh_owned(c, venue, bot, clock, prices, version, Some(claimed), Some(true)).await
}

/// Bot::ResyncIndexCompositionJob#perform's refresh_composition: the rebalance's refresh (any status but deleted or
/// archived, no DCA run owned), with no market gate (the job has none), so a stock newcomer's probe side reads the clock.
/// `marker` is the value read at pass start: the membership write clears it in the same transaction, if unchanged.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn refresh_for_resync<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, claimed: &Value, marker: &str, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Result<Value, Refusal>, EngineError> {
    refresh_owned_marked(c, venue, bot, clock, prices, version, Some(claimed), None, Some(marker)).await
}

#[allow(clippy::too_many_arguments)]
async fn refresh_owned<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>, claimed: Option<&Value>, open: Option<bool>) -> Result<Result<Value, Refusal>, EngineError> {
    refresh_owned_marked(c, venue, bot, clock, prices, version, claimed, open, None).await
}

#[allow(clippy::too_many_arguments)]
async fn refresh_owned_marked<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>, claimed: Option<&Value>, mut open: Option<bool>, marker: Option<&str>) -> Result<Result<Value, Refusal>, EngineError> {
    // A DCA tick owns its run and refreshes from what it reads here; a rebalance pass brings its own earlier claim.
    let run_owned = claimed.is_none();
    let before = match claimed { Some(v) => v.clone(), None => super::placement::composition_snapshot(c, bot)? };
    let provider_before = super::provider::fingerprint(c)?;
    let groups = bot.composition().map(|c| c.groups).unwrap_or_default();
    // A grouped bot's every refusal, the early ones included, forgets the composition it read (revision-checked).
    if bot.stored_groups() {
        let read = super::composition::composed_rev(bot);
        let refreshed = if provider_before.is_none() { Err(Refusal::Failure("Index provider not configured".into())) }
            else if groups.is_empty() { Err(Refusal::Failure("index groups: none readable".into())) }
            else { refresh_grouped(c, venue, bot, clock, prices, version, run_owned, &before, &provider_before, &groups, open, marker).await? };
        if refreshed.is_err() { model::locked(c, |tx| super::composition::forget(tx, bot.id, read.as_deref()))?; }
        return Ok(refreshed);
    }
    if provider_before.is_none() { return Ok(Err(Refusal::Failure("Index provider not configured".into()))); }
    // PLAN index-as-ingredient slice 2a: per group, in stored order. Only a `Bots::DcaIndex` row reaches this (one group).
    if groups.is_empty() { return Ok(Err(Refusal::Failure("index groups: none readable".into()))); }
    // D-INDEX-RETIRED-KEPT: a retired index keeps the membership this engine composed from it, unread and unwritten (a
    // resync's job completes); without that provenance nothing is composed or traded.
    // D-INDEX-RETIRED-PAIR is a grouped row's (refresh_grouped); a `Bots::DcaIndex` row keeps main's frozen path unchanged.
    let mut frozen = false;
    for group in &groups {
        if !retired(c, &group.external_id)? { continue; }
        if !composed_here(c, bot, &group.external_id)? { return Ok(Err(Refusal::Failure(RETIRED.into()))); }
        frozen = true;
    }
    if frozen {
        if let Some(read) = marker { model::locked(c, |tx| clear_resync(tx, bot.id, read).map(|_| ()))?; }
        return Ok(Ok(before));
    }
    let mut derived: Vec<Derived> = vec![];
    for group in &groups {
        match derive_group(c, venue, bot, group, &mut open, clock, prices, version).await? {
            Ok(d) => derived.push(d),
            Err(r) => return Ok(Err(r)),
        }
    }
    // One group (2a): its members as chosen, repeats kept (write_members' upsert collapses them, as 2.x's does).
    let members: Vec<(i64, i64, f64)> = derived.iter().flat_map(|d| d.members.iter().copied()).collect();
    model::credential_write(c, version, |tx| {
        if run_owned { model::require_run(tx, bot)?; }
        else if matches!(model::load_bot(tx, bot.id)?.status, crate::enums::BotStatus::Deleted | crate::enums::BotStatus::Archived) { return Err(EngineError::RunFenced); }
        let current = model::load_bot(tx, bot.id)?;
        // Every group's row is read before anything is compared, so an unreadable one raises whatever else moved.
        let sources_now = derived.iter().map(|d| ranked_row(tx, &d.external_id)).collect::<Result<Vec<_>, _>>()?;
        // A ranking that aged past its bound during the probes composes nothing: the next pass's gate holds the bot.
        let mut changed = current.status != bot.status || super::placement::composition_snapshot(tx, &current)? != before;
        for (d, now) in derived.iter().zip(&sources_now) {
            changed = changed || now.as_ref() != Some(&d.source) || !usable(&d.source, clock.now()) || retired(tx, &d.external_id)?;
        }
        changed = changed || super::provider::fingerprint(tx)? != provider_before || super::provider::stale(tx, &current)?.is_some();
        for ticker in derived.iter().flat_map(|d| &d.priced) { changed |= model::ticker_by_id(tx, bot.exchange_id, ticker.id)?.as_ref() != Some(ticker); }
        if changed { return Ok(Err(Refusal::Transient(CHANGED.into()))); }
        let mut reasons = super::eligibility::bot_reasons_typed(tx, &current)?;
        let alpaca = model::exchange_type(tx, &current)? == "Exchanges::Alpaca";
        super::eligibility::member_reasons(tx, &current, alpaca, &members.iter().map(|m| m.0).collect::<Vec<_>>(), &mut reasons)?;
        if !reasons.is_empty() { return Ok(Err(Refusal::Failure(reasons.join("; ")))); }
        // A taxonomy refresh during price probes cannot seat a newly classified stablecoin.
        for (asset, _, _) in &members {
            let (category, ext): (Option<String>, String) = tx.query_row("SELECT category, external_id FROM assets WHERE id=?1", [asset], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if stablecoin(category.as_deref(), &ext) { return Ok(Err(Refusal::Transient(CHANGED.into()))); }
        }
        basket::write_members(tx, bot.id, &members, clock.now())?;
        for d in &derived {
            tx.execute(&format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{COMPOSED_KEY}', ?2) WHERE id = ?1"), params![bot.id, d.external_id])?;
            if (d.members.len() as i64) < d.n {
                let message = format!("Index composition: {} of {} requested assets are tradable on {} in {}.", d.members.len(), d.n, model::exchange_name(tx, bot)?, model::quote_symbol(tx, bot)?);
                tx.execute("INSERT INTO bot_activity_logs (bot_id,event,level,message,details,created_at) VALUES (?1,'index_composition_shortfall',1,?2,?3,?4)",
                    params![bot.id, message, serde_json::json!({"available":d.members.len(),"requested":d.n}).to_string(), crate::codec::format_time(clock.now())])?;
            }
        }
        // A resync's marker goes with the membership it asked for, in this one commit; a trigger since it was read keeps it.
        if let Some(read) = marker { clear_resync(tx, bot.id, read)?; }
        // Any unexpected ineligibility rolls the transaction back (not a nested Ok(Err)).
        super::eligibility::check_install(tx)?.refusal().map_err(|r| EngineError::Data(r.reason()))?;
        Ok(Ok(super::placement::composition_snapshot(tx, &current)?))
    })
}

/// D-INDEX-RETIRED-PAIR (a grouped bot only): a frozen group's composed table rows, each kept only while its ticker is still
/// on the bot's exchange with that asset as base, the bot's quote, available and trading-enabled; the survivors renormalised
/// (all zero: equal, `blend`'s fallback). `true`: some member dropped.
pub(crate) fn frozen_members(c: &Connection, bot: &Bot, external_id: &str) -> Result<(Vec<basket::Derived>, bool), EngineError> {
    let mut s = c.prepare("SELECT asset_id, ticker_id, weight FROM rust_bot_group_members WHERE bot_id = ?1 AND index_external_id = ?2 AND composed = 1 ORDER BY asset_id")?;
    let rows = s.query_map(params![bot.id, external_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut kept = vec![];
    for (asset, ticker, weight) in &rows {
        let valid: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM tickers WHERE id = ?1 AND exchange_id = ?2 AND base_asset_id = ?3 AND quote_asset_id = ?4 \
                                       AND available = 1 AND trading_enabled = 1)", params![ticker, bot.exchange_id, asset, bot.quote_asset_id()], |r| r.get(0))?;
        if valid { kept.push((*asset, *ticker, *weight)); }
    }
    let dropped = kept.len() != rows.len();
    if !dropped { return Ok((kept, false)); }
    let total = float_sum(&kept.iter().map(|m| m.2).collect::<Vec<_>>());
    let equal = blend(&vec![0.0; kept.len()], 0.0);
    Ok((kept.into_iter().enumerate().map(|(i, (a, t, w))| (a, t, if total > 0.0 { w / total } else { equal[i] })).collect(), true))
}

/// Some frozen group of a grouped bot lost a member's pair since it was composed (placement's fence: the next refresh drops it).
pub fn frozen_pair_lost(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    if !bot.stored_groups() { return Ok(false); }
    for id in bot.index_external_ids() {
        if retired(c, &id)? && composed_here(c, bot, &id)? && frozen_members(c, bot, &id)?.1 { return Ok(true); }
    }
    Ok(false)
}

/// A live group's selection replayed on the closing transaction's data in published order: an incumbent or a pair this pass
/// probed priced is seated, one it probed unpriced is skipped. None (`CHANGED`): a candidate whose probe this pass did not
/// make (it became tradable during the probes), a pair gone, or a seat sequence other than the one derived.
fn replay(c: &Connection, bot: &Bot, group: &super::composition::Group, d: &Derived) -> Result<Option<Vec<basket::Derived>>, EngineError> {
    let Some(ranked) = top_coins(c, &d.top, &d.weights, d.own_caps)? else { return Ok(None) };
    let mut s = c.prepare("SELECT t.id, a.external_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                           WHERE t.exchange_id = ?1 AND t.quote_asset_id = ?2 AND t.available = 1 AND t.trading_enabled = 1 ORDER BY t.id")?;
    let mut by_external: HashMap<String, i64> = HashMap::new();
    for r in s.query_map(params![bot.exchange_id, bot.quote_asset_id()], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (id, ext) = r?;
        if let Some(ext) = ext.filter(|e| !e.is_empty()) { by_external.insert(ext, id); }
    }
    let incumbents = incumbents(c, bot, &d.external_id)?;
    let mut chosen = vec![];
    for (coin, cap) in ranked {
        if seats(&chosen, true) as i64 >= d.n { break; }
        let Some(&ticker_id) = by_external.get(&coin) else { continue };
        let Some(ticker) = model::ticker_by_id(c, bot.exchange_id, ticker_id)? else { return Ok(None) };
        if !incumbents.contains(&ticker_id) {
            match d.probed.get(&ticker_id) { Some(true) => {} Some(false) => continue, None => return Ok(None) }
        }
        chosen.push((ticker.base_asset_id, ticker_id, cap));
    }
    let members = blend_chosen(&chosen, group.flattening.unwrap_or(0.0), true);
    let same = members.iter().map(|m| (m.0, m.1)).eq(d.members.iter().map(|m| (m.0, m.1)));
    Ok(same.then_some(members))
}

/// One ingredient of a grouped composition, as the closing write combines them.
struct Ingredient { group: Option<String>, stored: f64, members: Vec<(i64, i64, f64)>, selected: bool }

/// The grouped composition (PLAN index-as-ingredient §4): every live group derived with probes, then one closing write that
/// replays them, reads the frozen groups, the named assets and the caps under its lock, and writes the flat members, the
/// group rows and `rust_composed_of` together. Any group's refusal is the bot's; nothing is written then.
#[allow(clippy::too_many_arguments)]
async fn refresh_grouped<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>,
                                                run_owned: bool, before: &Value, provider_before: &Option<String>, groups: &[super::composition::Group],
                                                mut open: Option<bool>, marker: Option<&str>) -> Result<Result<Value, Refusal>, EngineError> {
    use super::composition::Group;
    let composition = bot.composition().unwrap_or_default();
    let mut live: Vec<(&Group, Derived)> = vec![];
    let mut frozen: Vec<&Group> = vec![];
    for group in groups {
        if retired(c, &group.external_id)? {
            if !composed_here(c, bot, &group.external_id)? { return Ok(Err(Refusal::Failure(RETIRED.into()))); }
            frozen.push(group);
            continue;
        }
        match derive_group(c, venue, bot, group, &mut open, clock, prices, version).await? {
            Ok(d) => live.push((group, d)),
            Err(r) => return Ok(Err(r)),
        }
    }
    let now = clock.now();
    model::credential_write(c, version, |tx| {
        if run_owned { model::require_run(tx, bot)?; }
        else if matches!(model::load_bot(tx, bot.id)?.status, crate::enums::BotStatus::Deleted | crate::enums::BotStatus::Archived) { return Err(EngineError::RunFenced); }
        let current = model::load_bot(tx, bot.id)?;
        let sources_now = live.iter().map(|(_, d)| ranked_row(tx, &d.external_id)).collect::<Result<Vec<_>, _>>()?;
        let mut changed = current.status != bot.status || super::placement::composition_snapshot(tx, &current)? != *before;
        for ((_, d), source) in live.iter().zip(&sources_now) {
            changed = changed || source.as_ref() != Some(&d.source) || !usable(&d.source, now) || retired(tx, &d.external_id)?;
        }
        for g in &frozen { changed = changed || !retired(tx, &g.external_id)?; }
        changed = changed || super::provider::fingerprint(tx)? != *provider_before || super::provider::stale(tx, &current)?.is_some();
        // A probe's answer, priced or not, holds only for the ticker row it probed.
        for ticker in live.iter().flat_map(|(_, d)| d.priced.iter().chain(&d.unpriced)) { changed |= model::ticker_by_id(tx, bot.exchange_id, ticker.id)?.as_ref() != Some(ticker); }
        if changed { return Ok(Err(Refusal::Transient(CHANGED.into()))); }
        // Every ingredient from the data under this lock: named assets in settings order, then the groups in stored order.
        let mut ingredients = vec![];
        for (asset, weight) in &composition.assets {
            let ticker: Option<i64> = tx.query_row("SELECT id FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2 AND quote_asset_id = ?3 AND available = 1 AND trading_enabled = 1",
                params![bot.exchange_id, asset, bot.quote_asset_id()], |r| r.get(0)).optional()?;
            if let Some(t) = ticker { ingredients.push(Ingredient { group: None, stored: *weight, members: vec![(*asset, t, 1.0)], selected: false }); }
        }
        let mut ranked = Map::new();
        // The assets whose `assets.market_cap` this write reads: a live data-api group's ranked candidates (Codex 2b r2, r3),
        // and under top-level market cap every ingredient's (below).
        let mut cap_readers: Vec<i64> = vec![];
        for group in groups {
            if let Some((_, d)) = live.iter().find(|(g, _)| g.external_id == group.external_id) {
                let Some(members) = replay(tx, &current, group, d)? else { return Ok(Err(Refusal::Transient(CHANGED.into()))) };
                // Every asset the ranking names was examined by its cap (a missing one skipped it), seated or not (Codex 2b r3).
                if !d.own_caps {
                    let mut named = tx.prepare("SELECT id FROM assets WHERE external_id IN (SELECT value FROM json_each(?1))")?;
                    let top = serde_json::to_string(&d.top).map_err(|e| EngineError::Data(e.to_string()))?;
                    for a in named.query_map([top], |r| r.get::<_, i64>(0))? { cap_readers.push(a?); }
                }
                let Some(ranked_at) = recorded_ranked_at(&d.source.ranked_at) else { return Ok(Err(Refusal::Transient(CHANGED.into()))) };
                ranked.insert(group.external_id.clone(), serde_json::json!([d.source.id, ranked_at]));
                ingredients.push(Ingredient { group: Some(group.external_id.clone()), stored: group.weight, members, selected: true });
            } else {
                let (members, _) = frozen_members(tx, &current, &group.external_id)?;
                if members.is_empty() { return Ok(Err(Refusal::Failure(RETIRED.into()))); }
                ranked.insert(group.external_id.clone(), Value::Null);
                ingredients.push(Ingredient { group: Some(group.external_id.clone()), stored: group.weight, members, selected: false });
            }
        }
        // Shares: the stored ingredient weights, renormalised over what is present; under top-level market cap the caps
        // (an asset's own, a group's the sum over its members), each from this transaction, unless one is missing.
        let stored: Vec<f64> = ingredients.iter().map(|i| i.stored).collect();
        let mut shares: Option<Vec<f64>> = None;
        let ids: Vec<i64> = ingredients.iter().flat_map(|i| i.members.iter().map(|m| m.0)).collect();
        if composition.weighting == "market_cap" { cap_readers.extend(&ids); }
        let crypto: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM assets WHERE category = 'Cryptocurrency' AND id IN (SELECT value FROM json_each(?1)))",
            [serde_json::to_string(&cap_readers).map_err(|e| EngineError::Data(e.to_string()))?], |r| r.get(0))?;
        if crypto {
            if let Some(stale) = super::staleness::caps_stale(tx, &current, now)? { return Ok(Err(Refusal::Failure(stale.message))); }
        }
        if composition.weighting == "market_cap" {
            if let Some(caps) = basket::asset_caps(tx, &ids)? {
                let ingredient_caps: Vec<f64> = ingredients.iter().map(|i| float_sum(&i.members.iter().map(|m| caps[&m.0]).collect::<Vec<_>>())).collect();
                shares = Some(blend(&ingredient_caps, 0.0));
            }
        }
        let shares = match shares {
            Some(s) => s,
            None => {
                let total = float_sum(&stored);
                if total <= 0.0 { return Ok(Err(Refusal::Failure(format!("None of the portfolio's weighted assets trade on {}", model::exchange_name(tx, bot)?)))); }
                stored.iter().map(|w| w / total).collect()
            }
        };
        // Members: each ingredient's share × its within-group weight, an asset reached twice holding the sum.
        let mut members: Vec<(i64, i64, f64)> = vec![];
        for (ingredient, share) in ingredients.iter().zip(&shares) {
            for (asset, ticker, weight) in &ingredient.members {
                match members.iter_mut().find(|m| m.0 == *asset) { Some(m) => m.2 += share * weight, None => members.push((*asset, *ticker, share * weight)) }
            }
        }
        let mut reasons = super::eligibility::bot_reasons_typed(tx, &current)?;
        let alpaca = model::exchange_type(tx, &current)? == "Exchanges::Alpaca";
        super::eligibility::member_reasons(tx, &current, alpaca, &members.iter().map(|m| m.0).collect::<Vec<_>>(), &mut reasons)?;
        if !reasons.is_empty() { return Ok(Err(Refusal::Failure(reasons.join("; ")))); }
        // A taxonomy refresh during price probes cannot seat a newly classified stablecoin; a named asset is the user's own.
        let named: Vec<i64> = composition.assets.iter().map(|a| a.0).collect();
        for asset in ingredients.iter().filter(|i| i.selected).flat_map(|i| i.members.iter().map(|m| m.0)).filter(|a| !named.contains(a)) {
            let (category, ext): (Option<String>, String) = tx.query_row("SELECT category, external_id FROM assets WHERE id=?1", [asset], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if stablecoin(category.as_deref(), &ext) { return Ok(Err(Refusal::Transient(CHANGED.into()))); }
        }
        basket::write_members(tx, bot.id, &members, now)?;
        tx.execute("DELETE FROM rust_bot_group_members WHERE bot_id = ?1", [bot.id])?;
        let mut share_of = serde_json::json!({"assets": {}, "groups": {}});
        for (ingredient, share) in ingredients.iter().zip(&shares) {
            match &ingredient.group {
                None => { share_of["assets"][ingredient.members[0].0.to_string()] = serde_json::json!(share); }
                Some(id) => {
                    share_of["groups"][id.as_str()] = serde_json::json!(share);
                    for (asset, ticker, weight) in &ingredient.members {
                        tx.execute("INSERT INTO rust_bot_group_members (bot_id, index_external_id, asset_id, ticker_id, composed, weight) VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                                   params![bot.id, id, asset, ticker, weight])?;
                    }
                }
            }
        }
        super::composition::record(tx, &current, Value::Object(ranked), share_of)?;
        for ((group, d), ingredient) in live.iter().zip(ingredients.iter().filter(|i| i.selected)) {
            let available = ingredient.members.len();
            if (available as i64) < d.n {
                let message = format!("Index composition: {available} of {} requested assets are tradable on {} in {}.", d.n, model::exchange_name(tx, bot)?, model::quote_symbol(tx, bot)?);
                tx.execute("INSERT INTO bot_activity_logs (bot_id,event,level,message,details,created_at) VALUES (?1,'index_composition_shortfall',1,?2,?3,?4)",
                    params![bot.id, message, serde_json::json!({"available":available,"requested":d.n,"index":group.external_id}).to_string(), crate::codec::format_time(now)])?;
            }
        }
        if let Some(read) = marker { clear_resync(tx, bot.id, read)?; }
        super::eligibility::check_install(tx)?.refusal().map_err(|r| EngineError::Data(r.reason()))?;
        Ok(Ok(super::placement::composition_snapshot(tx, &current)?))
    })
}

/// One group's derivation (IndexAllocatable#derive_composition): its ranked row, the top N of the venue's tickers at the
/// quote, incumbents seated without a probe, newcomers probed, the group's flattening blend. `members` keeps every chosen
/// occurrence, as 2.x assigns the blended weight to each.
struct Derived { external_id: String, n: i64, members: Vec<(i64, i64, f64)>, source: Ranked, priced: Vec<Ticker>,
                 /// Grouped path: the tickers probed unpriced, as read for the probe (the closing write fences them too).
                 unpriced: Vec<Ticker>,
                 /// Grouped path: each probed ticker's answer, for the closing replay.
                 probed: HashMap<i64, bool>, top: Vec<String>, weights: Map<String, Value>, own_caps: bool }

/// Seats taken: chosen occurrences, or on the grouped path distinct assets (a ranking naming A twice seats A once).
fn seats(chosen: &[(i64, i64, f64)], distinct: bool) -> usize {
    if !distinct { return chosen.len(); }
    let mut seen = HashSet::new();
    chosen.iter().filter(|(a, _, _)| seen.insert(*a)).count()
}

/// The group's blend over what it chose: 2.x's insertion-ordered asset map (a repeat keeps its first position and takes the
/// later cap), every occurrence weighted; on the grouped path each asset once.
fn blend_chosen(chosen: &[(i64, i64, f64)], flattening: f64, distinct: bool) -> Vec<(i64, i64, f64)> {
    let mut caps: Vec<(i64, f64)> = vec![];
    for (asset, _, cap) in chosen {
        if let Some((_, value)) = caps.iter_mut().find(|(id, _)| id == asset) { *value = *cap; }
        else { caps.push((*asset, *cap)); }
    }
    let weights = blend(&caps.iter().map(|(_, cap)| *cap).collect::<Vec<_>>(), flattening);
    let by_asset: HashMap<i64, f64> = caps.into_iter().map(|(id, _)| id).zip(weights).collect();
    let mut seen = HashSet::new();
    chosen.iter().filter(|(a, _, _)| !distinct || seen.insert(*a)).map(|(asset, ticker, _)| (*asset, *ticker, by_asset[asset])).collect()
}

#[allow(clippy::too_many_arguments)]
async fn derive_group<V: Venue + Attributed>(c: &Connection, venue: &V, bot: &Bot, group: &super::composition::Group, open: &mut Option<bool>, clock: &dyn Clock, prices: &PriceCache, version: &Option<model::CredentialVersion>) -> Result<Result<Derived, Refusal>, EngineError> {
    let category = group.external_id.as_str();
    let Some(source_before) = ranked_row(c, category)? else { return Ok(Err(Refusal::Failure("Index not found".into()))) };
    let own_caps = source_before.provider == "coingecko";
    let Ranked { source, top_coins: top, weights, .. } = source_before.clone();
    let top: Vec<String> = top.and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()).unwrap_or_default()
        .into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let weights: Map<String, Value> = weights.and_then(|w| serde_json::from_str::<Value>(&w).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    // D-INDEX-TOP-N: filter the full ranked list for this venue and quote, then take N.
    // A 3 × N prefix can run out before the venue's first tradable member.
    let bounded = (source.as_deref() == Some("deltabadger") && !top.is_empty()).then_some(top.len() as i64);
    let n = match bounded { Some(b) if group.hold_all => b, _ => group.num_coins.unwrap_or(0) };
    if n > 1000 || top.len() > 1000 { return Ok(Err(Refusal::Failure("index exceeds 1000 members".into()))); }
    let Some(ranked) = top_coins(c, &top, &weights, own_caps)? else { return Ok(Err(Refusal::Failure("index ranking holds an invalid market cap".into()))) };

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
    // order would use (#probe_side, per ticker).
    let incumbents = incumbents(c, bot, category)?;
    let distinct = bot.stored_groups();
    let mut chosen: Vec<(i64, i64, f64)> = vec![];
    let mut priced_tickers = vec![];
    let mut probed = HashMap::new();
    let mut unpriced = vec![];
    for (coin, cap) in ranked {
        if seats(&chosen, distinct) as i64 >= n { break; }
        let Some(&ticker_id) = by_external.get(&coin) else { continue };
        let ticker = model::ticker_by_id(c, bot.exchange_id, ticker_id)?
            .ok_or_else(|| EngineError::Data(format!("bot {}: ticker {ticker_id} vanished mid-derivation", bot.id)))?;
        if !incumbents.contains(&ticker_id) {
            let side = match probe_side(venue, bot, &ticker, open).await { Ok(side) => side, Err(r) => return Ok(Err(r)) };
            match priced(venue, bot, &ticker, side, clock, prices, version).await {
                Ok(true) => { probed.insert(ticker_id, true); }
                Ok(false) => { probed.insert(ticker_id, false); unpriced.push(ticker); continue }
                Err(r) => return Ok(Err(r)),
            }
        }
        chosen.push((ticker.base_asset_id, ticker_id, cap));
        priced_tickers.push(ticker);
    }
    if chosen.is_empty() {
        return Ok(Err(Refusal::Failure(format!("No matching coins found on {} for the index", model::exchange_name(c, bot)?))));
    }
    // 2.x builds an insertion-ordered asset hash BEFORE blending; a repeated key replaces its
    // value without moving its first position. It then assigns that weight to every chosen occurrence.
    let members = blend_chosen(&chosen, group.flattening.unwrap_or(0.0), distinct);
    Ok(Ok(Derived { external_id: category.to_string(), n, members, source: source_before, priced: priced_tickers, unpriced, probed, top, weights, own_caps }))
}

/// The pairs an index bot's tick may price: every available, trading-enabled ticker at its quote whose asset its ranking
/// (`ranked_row`) names (incumbents at Step 1, newcomers at their probe).
/// Over every followed id, one list (ticker order, each pair once); each row's `top_coins` text is read by `json_each`
/// as it is stored.
pub fn candidate_pairs(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let mut tops = vec![];
    for id in bot.index_external_ids() {
        if let Some(t) = ranked_row(c, &id)?.and_then(|r| r.top_coins).filter(|t| t.trim_start().starts_with('[')) { tops.push(t); }
    }
    let tops = serde_json::to_string(&tops).map_err(|e| EngineError::Data(e.to_string()))?;
    let mut s = c.prepare("SELECT t.ticker FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 AND t.quote_asset_id = ?2 \
                           AND t.available = 1 AND t.trading_enabled = 1 AND a.external_id IN (SELECT j.value FROM json_each(?3) r, json_each(r.value) j) ORDER BY t.id")?;
    let pairs = s.query_map(params![bot.exchange_id, bot.quote_asset_id(), tops], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(pairs)
}

/// The same over the row 2.x reads (`Index.find_by(external_id:)`, the lowest id), on a 2.x install that holds no
/// rankings: for a parity copy (parity::plan_copy), which scripts the prices the recorded tick asks.
pub fn v2_candidate_pairs(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let top: Option<Option<String>> = c.query_row("SELECT top_coins FROM indices WHERE external_id = ?1 ORDER BY id LIMIT 1",
        [bot.index_external_id()], |r| r.get(0)).optional()?;
    pairs_of(c, bot, top.flatten())
}

fn pairs_of(c: &Connection, bot: &Bot, top: Option<String>) -> Result<Vec<String>, EngineError> {
    let top = top.filter(|t| t.trim_start().starts_with('[')).unwrap_or_else(|| "[]".into());
    let mut s = c.prepare("SELECT t.ticker FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 AND t.quote_asset_id = ?2 \
                           AND t.available = 1 AND t.trading_enabled = 1 AND a.external_id IN (SELECT value FROM json_each(?3)) ORDER BY t.id")?;
    let pairs = s.query_map(params![bot.exchange_id, bot.quote_asset_id(), top], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(pairs)
}
