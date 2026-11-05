//! Index::SyncFromCoingeckoJob's coingecko branch (app/jobs/index/sync_from_coingecko_job.rb:31-128), the category half of
//! the INDICES job on a CoinGecko install (PLAN-A17 A17-4, Rust item 3). `reference::coingecko_indices` runs Top Coins
//! first, then `pass`, on one client (one plan ping). Every write is a unit fenced by the run's captured provider
//! fingerprint; each ranked row is attested `coingecko` (engine::index::record_ranking) in the unit that writes it.
//!
//! Named divergences: D-CG-CATEGORY-STORED (every valid member and its cap stored), D-CG-FOLLOWED-DAILY (followed
//! categories asked on every run while not current), D-CG-FOLLOWED-KEPT (the sweep, the < 3 disqualification and the empty
//! availability skip spare a followed category), D-CG-INDEX-PULL-RETRIED (a failed list or followed fetch is Transient),
//! D-CG-INDEX-429-STOPS (a 429 asks nothing more), D-CG-RANK-ONCE-PER-SLOT (only what is not current is asked),
//! D-CG-AVAILABILITY-FROM-RANKING (step g), D-CG-INVALID-CAP-HELD (a negative cap refuses the answer),
//! D-CG-CATEGORIES-UNCACHED (the list asked once per run, never held an hour).
use super::coingecko::Api;
use super::import::present;
use super::{Cx, Outcome};
use crate::codec::format_time;
use crate::engine::{index, model, provider, EngineError};
use chrono::{DateTime, Datelike, Utc};
use md5::{Digest, Md5};
use rusqlite::types::Value as Sql;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashSet};

/// The slice completed for (fingerprint, slot): `<fingerprint>@<slot>` (contract item 5).
pub const SLICE_KEY: &str = "rust_job.indices_slice";
/// Index::EXCLUDED_CATEGORY_IDS (app/models/index.rb:11-16).
const EXCLUDED: [&str; 4] = ["stablecoins", "fiat-backed-stablecoin", "eur-stablecoin", "usd-stablecoin"];
/// Index::ExchangeAvailability::TOP_COINS_COUNT and MINIMUM_SUPPORTED_COINS (exchange_availability.rb:4-5).
const TOP_COINS_COUNT: usize = 10;
const MINIMUM_SUPPORTED_COINS: i64 = 3;
const CHANGED: &str = "index configuration changed during refresh; refresh again";

/// `due_today?` (:139-141) on the slot's UTC date: MD5(id) mod 14 == JDN mod 14. A run after midnight and before 10:30
/// belongs to the previous day's slot.
pub fn due_on(id: &str, slot: DateTime<Utc>) -> bool {
    let digest: [u8; 16] = Md5::digest(id.as_bytes()).into();
    let jdn = i64::from(slot.date_naive().num_days_from_ce()) + 1_721_425;
    u128::from_be_bytes(digest) % 14 == jdn.rem_euclid(14) as u128
}

/// A ranking of `external_id` that needs no refresh in this slot: attested `coingecko` under `fingerprint` at or after `slot`.
pub fn current(c: &Connection, external_id: &str, fingerprint: &str, slot: DateTime<Utc>) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM indices i JOIN rust_index_rankings r ON r.index_id = i.id WHERE i.external_id = ?1 \
                    AND r.provider = 'coingecko' AND r.fingerprint = ?2 AND r.ranked_at >= ?3)",
        params![external_id, fingerprint, index::ranked_at_text(slot)], |r| r.get(0))?)
}

/// Followed (D-CG-FOLLOWED-DAILY, Q-FOLLOWED (a)): the categories of index bots neither deleted nor archived that have an
/// exchange, stopped ones included (Rails' recheck set), sorted; never `top-coins`.
fn followed(c: &Connection) -> Result<BTreeSet<String>, EngineError> {
    use crate::enums::BotStatus::{Archived, Deleted};
    let ids = c.prepare(&format!("SELECT id FROM bots WHERE type = 'Bots::DcaIndex' AND status NOT IN ({}, {}) AND exchange_id IS NOT NULL ORDER BY id",
                                 Deleted as i64, Archived as i64))?
        .query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = BTreeSet::new();
    for id in ids {
        let bot = model::load_bot(c, id)?;
        // Top Coins' own row is ranked by Top Coins, whatever the bot's index type says.
        if bot.index_type() == Some("category") && bot.index_category_id().is_some_and(|id| id != "top-coins") { out.insert(bot.index_external_id().to_string()); }
    }
    Ok(out)
}

/// `strip_brackets` (:160-162): bracketed text removed, whitespace runs collapsed, stripped.
fn strip_brackets(name: &str) -> String {
    static RE: std::sync::OnceLock<(regex::Regex, regex::Regex)> = std::sync::OnceLock::new();
    let (brackets, spaces) = RE.get_or_init(|| (regex::Regex::new(r"[ \t\r\n\x0b\x0c]*\([^)]+\)").unwrap(), regex::Regex::new(r"[ \t\r\n\x0b\x0c]+").unwrap()));
    let once = brackets.replace_all(name, "");
    spaces.replace_all(&once, " ").trim_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\0').to_string()
}

/// `Exchange.available`, in id order.
fn venues(c: &Connection) -> Result<Vec<(i64, String)>, EngineError> {
    Ok(c.prepare_cached("SELECT id, type FROM exchanges WHERE available = 1 ORDER BY id")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// `calculate_top_coins_by_exchange` (:167-182): per available venue (empty lists kept), the first 10 of `coins` it lists
/// as available, trading-enabled bases, in `coins`' order.
fn top_coins_by_exchange(c: &Connection, coins: &[String]) -> Result<Map<String, Value>, EngineError> {
    let ids = json!(coins).to_string();
    let mut out = Map::new();
    for (venue, ty) in venues(c)? {
        let listed: HashSet<String> = c.prepare_cached("SELECT DISTINCT a.external_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
            WHERE t.exchange_id = ?1 AND t.available = 1 AND t.trading_enabled = 1 AND a.external_id IN (SELECT value FROM json_each(?2))")?
            .query_map(params![venue, ids], |r| r.get(0))?.collect::<Result<_, _>>()?;
        out.insert(ty, json!(coins.iter().filter(|id| listed.contains(*id)).take(TOP_COINS_COUNT).collect::<Vec<_>>()));
    }
    Ok(out)
}

/// `Index.calculate_available_exchanges(top_coins:)` (exchange_availability.rb:21-36): per available venue, the most
/// available trading-enabled tickers on one quote whose base is one of `coins`, kept when at least 3.
fn available_exchanges(c: &Connection, coins: &[String]) -> Result<Map<String, Value>, EngineError> {
    let mut out = Map::new();
    if coins.is_empty() { return Ok(out); }
    let ids = json!(coins).to_string();
    let assets: Vec<i64> = c.prepare_cached("SELECT id FROM assets WHERE external_id IN (SELECT value FROM json_each(?1))")?
        .query_map([&ids], |r| r.get(0))?.collect::<Result<_, _>>()?;
    if assets.is_empty() { return Ok(out); }
    let assets = json!(assets).to_string();
    for (venue, ty) in venues(c)? {
        let n: i64 = c.prepare_cached("SELECT coalesce(max(n), 0) FROM (SELECT count(*) AS n FROM tickers WHERE exchange_id = ?1 AND available = 1 \
            AND trading_enabled = 1 AND base_asset_id IN (SELECT value FROM json_each(?2)) GROUP BY quote_asset_id)")?
            .query_row(params![venue, assets], |r| r.get(0))?;
        if n >= MINIMUM_SUPPORTED_COINS { out.insert(ty, json!(n)); }
    }
    Ok(out)
}

/// Availability qualified against each venue's own list (:76-80, exchange_availability.rb:63-72's per-exchange branch).
fn availability_of(c: &Connection, per_exchange: &Map<String, Value>) -> Result<Map<String, Value>, EngineError> {
    let mut out = Map::new();
    for (ty, coins) in per_exchange {
        if let Some(n) = available_exchanges(c, &strings(coins))?.remove(ty) { out.insert(ty.clone(), n); }
    }
    Ok(out)
}

fn strings(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() }
fn parsed(text: Option<String>) -> Value { text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null) }

/// `to_f` of a coin's market_cap, as the engine reads a stored cap (index.rs `to_f`): a number, a numeric string, else 0.
fn cap(v: &Value) -> f64 {
    let f = match v { Value::Number(n) => n.as_f64().unwrap_or(0.0), Value::String(s) => s.trim().parse().unwrap_or(0.0), _ => 0.0 };
    if f.is_finite() { f } else { 0.0 }
}

/// What a category's answer became.
#[derive(Debug, PartialEq)]
enum Ranked { Written, Disqualified, Skipped, Unlisted }

/// One category's good answer (:55-118): the valid coins (answer ids, deduped first-wins, with a local asset, in answer
/// order); Rails' rules for a non-followed one; else the row, its caps and its ranking. In the caller's fenced unit.
fn rank(c: &Connection, id: &str, rows: &[Value], followed: bool, feed: Option<&Value>, fingerprint: &str, now: DateTime<Utc>) -> Result<Ranked, EngineError> {
    let mut seen = HashSet::new();
    let mut valid = vec![];
    let mut weights = Map::new();
    for row in rows {
        let Some(coin) = row["id"].as_str() else { continue };
        if !seen.insert(coin.to_string()) { continue; }
        let known: bool = c.prepare_cached("SELECT EXISTS (SELECT 1 FROM assets WHERE external_id = ?1)")?.query_row([coin], |r| r.get(0))?;
        if !known { continue; }
        valid.push(coin.to_string());
        weights.insert(coin.to_string(), json!(cap(&row["market_cap"])));
    }
    if !followed && (valid.len() as i64) < MINIMUM_SUPPORTED_COINS { return Ok(Ranked::Disqualified); }
    let per_exchange = top_coins_by_exchange(c, &valid)?;
    let available = availability_of(c, &per_exchange)?;
    if !followed && available.is_empty() { return Ok(Ranked::Skipped); }
    type Row = (i64, Option<String>, Option<String>, Option<f64>, [Option<String>; 4]);
    let existing: Option<Row> = c.query_row("SELECT id, name, description, CAST(market_cap AS REAL), top_coins, top_coins_by_exchange, available_exchanges, weights \
        FROM indices WHERE external_id = ?1 AND source = 'coingecko'", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?]))).optional()?;
    let t = format_time(now);
    let json_cols = [json!(valid), Value::Object(per_exchange), Value::Object(available), Value::Object(weights)];
    let [top, by_ex, avail, w] = json_cols.iter().map(Value::to_string).collect::<Vec<_>>().try_into().map_err(|_| EngineError::Data("columns".into()))?;
    // name, description, market_cap.
    type Attrs = (Option<String>, Option<String>, Option<String>);
    let attrs = |f: &Value| -> Result<Attrs, EngineError> {
        Ok((f["name"].as_str().map(strip_brackets), super::import::string(&f["content"]), super::import::decimal(&f["market_cap"]).map_err(EngineError::Data)?))
    };
    let index_id = match (existing, feed) {
        (None, None) => return Ok(Ranked::Unlisted),
        (None, Some(f)) => {
            let (name, description, market_cap) = attrs(f)?;
            // Index::WEIGHTED_CATEGORIES[id] || 0, for a new row only.
            let weight = super::import::WEIGHTED_CATEGORIES.iter().find(|(k, _)| *k == id).map_or(0, |(_, w)| *w);
            c.execute("INSERT INTO indices (external_id, source, name, description, top_coins, top_coins_by_exchange, market_cap, available_exchanges, \
                       weights, weight, created_at, updated_at) VALUES (?1, 'coingecko', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![id, name, description, top, by_ex, market_cap, avail, w, weight, t])?;
            c.last_insert_rowid()
        }
        (Some((row, old_name, old_description, old_cap, old_json)), feed) => {
            // update!: the feed's attributes while CoinGecko lists the category, else the row keeps its own; updated_at moves
            // only on a change (a JSON column compared parsed, as Rails compares the cast values).
            let (name, description, market_cap, cap_changed) = match feed {
                Some(f) => {
                    let (n, d, m) = attrs(f)?;
                    let changed = m.as_deref().and_then(|m| m.parse::<f64>().ok()) != old_cap;
                    (n, d, m.map(Sql::Text).unwrap_or(Sql::Null), changed)
                }
                None => (old_name.clone(), old_description.clone(), Sql::Null, false),
            };
            let changed = name != old_name || description != old_description || cap_changed
                || old_json.into_iter().zip(&json_cols).any(|(old, new)| parsed(old) != *new);
            c.execute("UPDATE indices SET name = ?2, description = ?3, top_coins = ?4, top_coins_by_exchange = ?5, market_cap = CASE WHEN ?11 THEN ?6 ELSE market_cap END, \
                       available_exchanges = ?7, weights = ?8, updated_at = CASE WHEN ?9 THEN ?10 ELSE updated_at END WHERE id = ?1",
                params![row, name, description, top, by_ex, market_cap, avail, w, changed, t, feed.is_some()])?;
            row
        }
    };
    index::record_ranking(c, index_id, "coingecko", fingerprint, now)?;
    Ok(Ranked::Written)
}

/// `refresh_availability` (:132-138) as D-CG-AVAILABILITY-FROM-RANKING reads it: for every `coingecko` row, the new
/// (top_coins_by_exchange, available_exchanges) when they changed. A row this engine ranked under `fingerprint` (of any age)
/// re-derives the per-venue lists from its stored full ranking; any other row, Rails' `refresh_available_exchanges!`
/// (its saved per-venue lists, else its top_coins). Read outside the write lock; one row's failure is logged.
fn refresh_plan(c: &Connection, fingerprint: &str) -> Result<Vec<(i64, Option<String>, String)>, EngineError> {
    type Row = (i64, Option<String>, Option<String>, Option<String>, Option<String>, bool);
    let rows: Vec<Row> = c.prepare("SELECT i.id, i.external_id, i.top_coins, i.top_coins_by_exchange, i.available_exchanges, \
        EXISTS (SELECT 1 FROM rust_index_rankings r WHERE r.index_id = i.id AND r.provider = 'coingecko' AND r.fingerprint = ?1) \
        FROM indices i WHERE i.source = 'coingecko' ORDER BY i.id")?
        .query_map([fingerprint], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<_, _>>()?;
    let mut out = vec![];
    for (id, external_id, top, by_ex, avail, ranked) in rows {
        let one = || -> Result<Option<(Option<String>, String)>, EngineError> {
            let top = strings(&parsed(top.clone()));
            let old_by_ex = parsed(by_ex.clone());
            let (new_by_ex, new_avail) = if ranked {
                let per = top_coins_by_exchange(c, &top)?;
                let avail = availability_of(c, &per)?;
                (Some(Value::Object(per)), avail)
            } else {
                match old_by_ex.as_object().filter(|m| !m.is_empty()) {
                    Some(per) => (None, availability_of(c, per)?),
                    None => (None, available_exchanges(c, &top)?),
                }
            };
            let new_avail = Value::Object(new_avail);
            let by_ex_changed = new_by_ex.as_ref().is_some_and(|n| *n != old_by_ex);
            if !by_ex_changed && new_avail == parsed(avail.clone()) { return Ok(None); }
            Ok(Some((new_by_ex.filter(|_| by_ex_changed).map(|v| v.to_string()), new_avail.to_string())))
        };
        match one() {
            Ok(Some((by_ex, avail))) => out.push((id, by_ex, avail)),
            Ok(None) => {}
            Err(e) => crate::engine::log(&format!("[Index Sync] Failed to refresh availability for {}: {e:?}", external_id.as_deref().unwrap_or("?"))),
        }
    }
    Ok(out)
}

/// What the pass left for the combined Outcome (reference::coingecko_indices).
#[derive(Debug, Default)]
pub struct Tally { pub rate_limited: Option<String>, pub transient: Vec<String>, pub failed: Option<String>, pub done: bool }

impl Tally {
    /// RateLimited > Transient > Failed > Done > NothingNew.
    pub fn outcome(self) -> Outcome {
        if let Some(m) = self.rate_limited { return Outcome::RateLimited(m); }
        if !self.transient.is_empty() { return Outcome::Transient(self.transient.join("; ")); }
        if let Some(m) = self.failed { return Outcome::Failed(m); }
        if self.done { Outcome::Done } else { Outcome::NothingNew }
    }
    /// Folds another part's Outcome in (Top Coins').
    pub fn add(&mut self, o: Outcome) {
        match o {
            Outcome::RateLimited(m) => { self.rate_limited.get_or_insert(m); }
            Outcome::Transient(m) => self.transient.push(m),
            Outcome::Failed(m) => { self.failed = Some(match self.failed.take() { Some(f) => format!("{f}; {m}"), None => m }); }
            Outcome::Done => self.done = true,
            Outcome::NothingNew => {}
        }
    }
}

/// One write unit on the job's connection, fenced by the run's captured fingerprint.
async fn unit<T: Send + 'static>(cx: &Cx<'_>, fingerprint: &str, f: impl FnOnce(&Connection) -> Result<T, EngineError> + Send + 'static) -> Result<T, String> {
    let fingerprint = fingerprint.to_string();
    cx.db.run(move |c, _| model::locked(c, |tx| {
        if provider::fingerprint(tx)?.as_deref() != Some(&fingerprint) { return Err(EngineError::Data(CHANGED.into())); }
        f(tx)
    }).map_err(|e| match e { EngineError::Data(m) if m == CHANGED => m, e => format!("{e:?}") })).await
}

/// One due category: (id, followed, in the slice).
struct Due { id: String, followed: bool, slice: bool }

/// The category pass (Rust item 3, steps b–h) after the guards and Top Coins, on `api` (the run's one client).
pub async fn pass(api: &dyn Api, fingerprint: &str, cx: &Cx<'_>, tally: &mut Tally) {
    if let Err(m) = steps(api, fingerprint, cx, tally).await { tally.failed = Some(m); }
}

async fn steps(api: &dyn Api, fingerprint: &str, cx: &Cx<'_>, tally: &mut Tally) -> Result<(), String> {
    let now = cx.clock.now();
    let slot = index::ranking_slot(now);
    let mark = format!("{fingerprint}@{}", index::ranked_at_text(slot));
    let m = mark.clone();
    let (followed, slice_done) = cx.db.run(move |c, _| {
        let done = crate::app_config::get_plain(c, SLICE_KEY)?.as_deref() == Some(m.as_str());
        Ok((followed(c).map_err(|e| format!("{e:?}"))?, done))
    }).await?;

    // b. The list (D-CG-CATEGORIES-UNCACHED). A 429 asks nothing more; another failure is retried, with no sweep and no slice.
    let feed: Option<Vec<Value>> = match api.categories().await {
        Err(e) if e.rate_limited() => { tally.rate_limited = Some(e.message()); None }
        Err(e) => { tally.transient.push(format!("CoinGecko categories: {}", e.message())); None }
        Ok(Value::Array(rows)) => Some(rows),
        Ok(_) => { tally.transient.push("CoinGecko categories: the answer is not a list".into()); None }
    };
    // c. Eligible: id present, not excluded, content present.
    let eligible: Vec<&Value> = feed.iter().flatten().filter(|c| c["id"].as_str().is_some_and(|id| !id.trim().is_empty() && !EXCLUDED.contains(&id))
                                                              && present(&c["content"])).collect();
    let eligible_ids: Vec<String> = eligible.iter().filter_map(|c| c["id"].as_str().map(str::to_string)).collect();
    if !eligible_ids.is_empty() {
        let keep = json!(eligible_ids).to_string();
        unit(cx, fingerprint, move |tx| {
            // The followed set as this transaction sees it: a follow committed during the list request is spared too.
            let spare = json!(self::followed(tx)?).to_string();
            tx.execute("DELETE FROM indices WHERE source = 'coingecko' AND external_id NOT IN (SELECT value FROM json_each(?1)) \
                        AND external_id NOT IN (SELECT value FROM json_each(?2))", params![keep, spare])?;
            Ok(())
        }).await?;
    }

    // d. Due: the slice (until complete for this fingerprint and slot), then the followed, each only while not current.
    let slice_asked = !eligible_ids.is_empty() && !slice_done;
    let mut due: Vec<Due> = vec![];
    if slice_asked {
        for id in eligible_ids.iter().filter(|id| due_on(id, slot)) {
            if !due.iter().any(|d| d.id == *id) { due.push(Due { id: id.clone(), followed: followed.contains(id), slice: true }); }
        }
    }
    for id in &followed { if !due.iter().any(|d| d.id == *id) { due.push(Due { id: id.clone(), followed: true, slice: false }); } }
    let (fp, ids) = (fingerprint.to_string(), due.iter().map(|d| d.id.clone()).collect::<Vec<_>>());
    let skip: Vec<bool> = cx.db.run(move |c, _| ids.iter().map(|id| current(c, id, &fp, slot)).collect::<Result<Vec<_>, _>>().map_err(|e| format!("{e:?}"))).await?;
    let due: Vec<Due> = due.into_iter().zip(skip).filter_map(|(d, cur)| (!cur).then_some(d)).collect();

    // e. Each due category, one `coins/markets?category=` page of 250 (Rails' get_coins_list_with_market_data(limit: 250)).
    let mut slice_open = 0usize; // slice categories not answered
    let mut disqualified: Vec<String> = vec![];
    for d in &due {
        if tally.rate_limited.is_some() { slice_open += usize::from(d.slice); continue; }
        let entry = feed.iter().flatten().find(|c| c["id"].as_str() == Some(d.id.as_str())).cloned();
        let answer = match api.markets(None, Some(&d.id), 250, 1).await {
            Err(e) if e.rate_limited() => { tally.rate_limited = Some(e.message()); slice_open += usize::from(d.slice); continue; }
            Err(e) => Err(e.message()),
            Ok(Value::Array(rows)) if rows.iter().any(|r| cap(&r["market_cap"]) < 0.0) =>
                Err("the answer holds a negative market cap; previous ranking kept (D-CG-INVALID-CAP-HELD)".into()),
            Ok(Value::Array(rows)) => Ok(rows),
            Ok(_) => Err("the answer is not a list".into()),
        };
        let rows = match answer {
            Ok(rows) => rows,
            Err(m) => {
                crate::engine::log(&format!("[Index Sync] Failed to sync category {}: {m}", d.id));
                if d.followed { tally.transient.push(format!("CoinGecko category {}: {m}", d.id)); }
                slice_open += usize::from(d.slice);
                continue;
            }
        };
        let (id, followed, fp) = (d.id.clone(), d.followed, fingerprint.to_string());
        let ranked = unit(cx, fingerprint, move |tx| {
            let followed = followed || self::followed(tx)?.contains(&id); // followed since the run began
            rank(tx, &id, &rows, followed, entry.as_ref(), &fp, now)
        }).await?;
        match ranked {
            Ranked::Written => tally.done = true,
            Ranked::Disqualified => disqualified.push(d.id.clone()),
            Ranked::Skipped => {}
            Ranked::Unlisted => crate::engine::log(&format!("[Index Sync] {}: followed, but CoinGecko lists no such category and no row holds it; nothing written", d.id)),
        }
    }

    // f. The disqualified (never a followed one).
    if !disqualified.is_empty() {
        let ids = json!(disqualified).to_string();
        unit(cx, fingerprint, move |tx| {
            let spare = json!(self::followed(tx)?).to_string(); // a follow since the fetch keeps its row
            tx.execute("DELETE FROM indices WHERE source = 'coingecko' AND external_id IN (SELECT value FROM json_each(?1)) \
                        AND external_id NOT IN (SELECT value FROM json_each(?2))", params![ids, spare])?;
            Ok(())
        }).await?;
    }

    // g. Availability, for every coingecko row, written only on a change; ranked_at untouched.
    let fp = fingerprint.to_string();
    let changes = cx.db.run(move |c, _| refresh_plan(c, &fp).map_err(|e| format!("{e:?}"))).await?;
    if !changes.is_empty() {
        let t = format_time(now);
        unit(cx, fingerprint, move |tx| {
            for (id, by_ex, avail) in changes {
                tx.execute("UPDATE indices SET top_coins_by_exchange = coalesce(?2, top_coins_by_exchange), available_exchanges = ?3, updated_at = ?4 WHERE id = ?1",
                           params![id, by_ex, avail, t])?;
            }
            Ok(())
        }).await?;
    }

    // h. The last unit: the provider record (the list was read), the slice's completion, Rails' recheck_index_bots; then the
    // engine is woken.
    let (list_read, completed) = (feed.is_some(), slice_asked && slice_open == 0);
    let fp = fingerprint.to_string();
    unit(cx, fingerprint, move |tx| {
        if list_read { provider::record(tx, &fp, now)?; }
        if completed { crate::app_config::set_plain(tx, SLICE_KEY, &mark, now).map_err(EngineError::Data)?; }
        index::mark_resync_all(tx, now).map(|_| ())
    }).await?;
    if completed { tally.done = true; }
    cx.wakers.wake_engine();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_are_stripped_as_rails_strips_them() {
        assert_eq!(strip_brackets("Layer 1 (L1)"), "Layer 1");
        assert_eq!(strip_brackets("YZi Labs (Prev. Binance Labs) Portfolio"), "YZi Labs Portfolio");
        assert_eq!(strip_brackets("  Meme  "), "Meme");
    }

    #[test]
    fn the_slice_is_the_slots_utc_date() {
        // JDN(2026-10-02) = 2461316; 01:00 on the 3rd is still the 2nd's slot.
        let slot: DateTime<Utc> = "2026-10-02T10:30:00Z".parse().unwrap();
        assert_eq!(i64::from(slot.date_naive().num_days_from_ce()) + 1_721_425, 2_461_316);
        let ids: Vec<String> = (0..200).map(|i| format!("cat-{i}")).collect();
        let today = ids.iter().filter(|id| due_on(id, slot)).count();
        assert!((5..30).contains(&today), "about 1 in 14: {today}");
        let next = slot + chrono::Duration::days(1);
        assert!(ids.iter().all(|id| !(due_on(id, slot) && due_on(id, next))), "one bucket a day");
    }
}
