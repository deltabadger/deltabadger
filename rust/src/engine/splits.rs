//! The engine reuses the figures port of Rails' split matching. Trust is an additional trading gate.
use crate::venue::Attributed;
use super::{model::{self, Bot}, basket, EngineError};
use crate::figures::{self, at::At, db, splits as rails};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// `cap`: the base cap's split dependencies at `now` (A14b; empty when the cap is off). The account sync bumps the generation
/// only for bots whose rows name a split's symbol, so a wallet seller's split, or one coming into effect, shows only here.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot { pub generation: i64, pub held_assets: Vec<i64>, pub cap: CapSplits }
pub fn snapshot(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Snapshot, EngineError> {
    let generation = c.query_row("SELECT restatement_generation FROM bots WHERE id=?1", [bot.id], |r| r.get(0))?;
    let mut s = c.prepare("SELECT DISTINCT base_asset_id FROM transactions WHERE bot_id=?1 AND status=0 AND base_asset_id IS NOT NULL ORDER BY base_asset_id")?;
    let held_assets = s.query_map([bot.id], |r| r.get(0))?.collect::<Result<_,_>>()?;
    Ok(Snapshot { generation, held_assets, cap: cap_dependencies(c, bot, now)? })
}

/// A14b (D-SPLIT-BASE-CAP): split events per (venue, asset), already in effect, and whether any of them cannot be sized.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CapSplits { pub events: Vec<(i64, SplitEvent)>, pub unresolved: bool }
impl CapSplits {
    /// F_{venue,asset}(t): the product of the factors of that asset's events on that venue after `t_us` (an event at `t` is
    /// already in a count taken at `t`).
    pub fn factor(&self, venue: i64, asset: i64, t_us: i64) -> BigDec {
        self.events.iter().filter(|(v, e)| *v == venue && e.asset_id == asset && t_us < e.at_us).fold(BigDec::one(), |f, (_, e)| &f * &e.factor)
    }
}

/// The base cap's split events for (venue, asset) pairs a bot need not have traded (a seller of a wallet it never bought
/// through the bot). Matched once per venue, with only that venue's holdings and eligibility: `groups` buckets by holding
/// and checks eligibility across every venue it is given, which would apply one venue's report to another's sales. A
/// holding's names are the venue's tickers for the asset and the names the bot's own rows of it were recorded under.
pub fn asset_splits(c: &Connection, user_id: i64, bot_id: i64, pairs: &[(i64, i64)], now: DateTime<Utc>) -> Result<CapSplits, EngineError> {
    let mut venues: BTreeMap<i64, std::collections::BTreeSet<i64>> = BTreeMap::new();
    for (venue, asset) in pairs { venues.entry(*venue).or_default().insert(*asset); }
    let at = At::from_utc(now).ok_or_else(|| data("split time out of range"))?;
    figures::budget::within(|| {
        let mut out = CapSplits::default();
        for (venue, assets) in venues {
            let mut holdings = vec![];
            for asset in &assets {
                let mut s = c.prepare("SELECT base FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2 \
                                       UNION SELECT base FROM transactions WHERE bot_id = ?3 AND exchange_id = ?1 AND base_asset_id = ?2 AND base IS NOT NULL ORDER BY 1")?;
                let strings = s.query_map(params![venue, asset, bot_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                holdings.push(rails::Holding { key: asset.to_string(), asset_id: Some(*asset), strings });
            }
            let strings = holdings.iter().flat_map(|h| h.strings.iter().map(|s| (venue, s.as_str()))).collect();
            let eligible = (strings, assets.iter().map(|a| (venue, *a)).collect(), vec![venue]);
            for e in rails::events_with(c, user_id, &eligible, &holdings, at).map_err(data)? {
                out.events.push((venue, SplitEvent { at_us: e.at.0 / 1000, asset_id: e.key.parse().map_err(data)?, factor: BigDec::parse(&e.factor.to_s_f()).map_err(data)? }));
            }
            out.unresolved |= rails::unresolved_with(c, user_id, &eligible, &holdings, at).map_err(data)?;
        }
        Ok(out)
    })
}

/// The base cap's dependencies: `asset_splits` over the pairs its tally counts (accounting::cap_pairs). The tick's gate, the
/// placement fences and the tally read this one set. Empty while the cap is off.
pub fn cap_dependencies(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<CapSplits, EngineError> {
    match super::accounting::cap_pairs(c, bot)? {
        Some(pairs) => asset_splits(c, bot.user_id, bot.id, &pairs, now),
        None => Ok(CapSplits::default()),
    }
}

pub const QUARANTINE_SECS: i64 = rails::QUARANTINE_SECONDS;
#[derive(Clone, Debug, PartialEq)]
pub struct SplitEvent { pub at_us: i64, pub asset_id: i64, pub factor: BigDec }
#[derive(Default)]
pub struct Splits { pub events: Vec<SplitEvent>, pub unresolved: bool }
fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

pub fn splits(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Splits, EngineError> {
    figures::budget::within(|| {
        let orders = db::orders(c, bot.id).map_err(data)?;
        let mut held: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for o in &orders { if let Some(id) = o.asset_id { held.entry(id).or_default().extend(o.base.clone()); } }
        let holdings: Vec<rails::Holding> = held.into_iter().map(|(id, strings)| rails::Holding { key: id.to_string(), asset_id: Some(id), strings }).collect();
        let now = At::from_utc(now).ok_or_else(|| data("split time out of range"))?;
        let events = rails::events(c, bot.user_id, &orders, &holdings, now).map_err(data)?.into_iter().map(|e| {
            Ok(SplitEvent { at_us: e.at.0 / 1000, asset_id: e.key.parse().map_err(data)?, factor: BigDec::parse(&e.factor.to_s_f()).map_err(data)? })
        }).collect::<Result<Vec<_>, EngineError>>()?;
        Ok(Splits { events, unresolved: rails::unresolved(c, bot.user_id, &orders, &holdings, now).map_err(data)? })
    })
}

pub fn untrusted(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let s = splits(c, bot, now)?;
    if s.unresolved { return Ok(true); }
    // A14b: a base cap that counts an asset whose split cannot be sized cannot be counted; no sale is sized against it.
    if cap_dependencies(c, bot, now)?.unresolved { return Ok(true); }
    let walk = basket::walk_with(c, bot, &s.events)?;
    Ok(walk.restated_at_us.is_some_and(|at| at > now.timestamp_micros() - QUARANTINE_SECS * 1_000_000))
}

/// D2a's split_verdicts contract: row evidence alone never establishes venue agreement.
pub fn row_verdict(rows: &[Value]) -> &'static str {
    let [row] = rows else { return "several rows" };
    let raw = &row["raw_data"];
    if raw["merged_activity_ids"].as_array().is_none_or(|ids| ids.len() < 2) { return "a lone leg"; }
    let terms: Vec<f64> = raw["split_ratio"].as_str().unwrap_or_default().split(':').map(|t| t.parse::<u64>().map_or(0.0, |t| t as f64)).collect();
    let &[new, old] = terms.as_slice() else { return "no ratio" };
    if new < 1.0 || old < 1.0 || new == old { return "no ratio"; }
    let number = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()).filter(|x| x.is_finite());
    let (Some(net), Some(first)) = (number(&row["base_amount"]), number(&raw["qty"])) else { return "not one split" };
    let (before, after) = if first < 0.0 { (-first, net-first) } else { (first-net, first) };
    if before > 0.0 && after > 0.0 && (after/before-new/old).abs() <= 0.001*(after/before)*(1.0+1e-9) { "trusted" } else { "not one split" }
}

/// One share quantum (9 decimals), an absolute rounding allowance, never the ratio's per-mille window.
pub fn position_agrees(expected: &BigDec, actual: &BigDec) -> bool {
    let delta = expected - actual;
    delta.max(actual - expected) <= BigDec::one().div(&BigDec::from_i64(1_000_000_000)).unwrap_or_else(BigDec::zero)
}

/// Stored reports affecting a symbol this bot actually traded. Every applicable row must pass; no rewrite or override.
fn row_refusal(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Option<String>, EngineError> {
    let orders = db::orders(c,bot.id).map_err(data)?;
    let mut held: BTreeMap<i64,Vec<String>> = BTreeMap::new();
    for order in &orders { if let Some(id)=order.asset_id { held.entry(id).or_default().extend(order.base.clone()); } }
    let holdings: Vec<_> = held.into_iter().map(|(id,strings)| rails::Holding {key:id.to_string(),asset_id:Some(id),strings}).collect();
    type ReportKey = (String, String, Option<(String, String)>);
    let mut groups: BTreeMap<ReportKey,Vec<Value>> = BTreeMap::new();
    for group in rails::groups(c,bot.user_id,&orders,&holdings).map_err(data)? {
        for row in group.rows {
            if row.at.utc()>now {continue;}
            let amount=c.query_row("SELECT base_amount FROM account_transactions WHERE id=?1",[row.id],|r| Ok(crate::ruby::from_sql(r.get_ref(0)?)))?.map_err(data)?.ok_or_else(|| data("missing split amount"))?;
            // D-ALPACA-SPLIT-GROUPING: each action must prove its own ratio. Distinct actions may compose;
            // several reports of one action still fail the row-evidence gate.
            let event = crate::sync::activities::split_event(&row.raw_data).map(|(key, id)| (key.to_string(), id.to_string()));
            groups.entry((row.base_currency,row.at.utc().format("%Y-%m-%d").to_string(),event)).or_default().push(json!({"base_amount":amount.to_s_f(),"raw_data":row.raw_data}));
        }
    }
    for ((symbol, day, _), rows) in groups {
        let verdict = row_verdict(&rows);
        if verdict != "trusted" { return Ok(Some(format!("{symbol} {day}: {verdict}"))); }
    }
    Ok(None)
}

/// Account positions must be compared to account holdings, never to one bot's share alone.
/// Shared/manual or unsupported histories fail closed if the account cannot be reconciled.
pub async fn refusal<V: crate::venue::Venue>(c: &Connection, venue: &V, bot: &Bot, now: DateTime<Utc>) -> Result<Option<String>, EngineError> {
    let handle=crate::venue::Handle::for_bot(venue,c,bot)?;
    let venue=&handle;
    // All synchronous history reads, grouping and walks share one cumulative scope. End it before venue I/O.
    let expected = match figures::budget::within(|| expected_positions(c, bot, now)) {
        Ok(Ok(expected)) => expected,
        Ok(Err(reason)) => return Ok(Some(reason)),
        Err(EngineError::Data(reason)) => return Ok(Some(format!("account history cannot be reconciled within its budget: {reason}"))),
        Err(err) => return Err(err),
    };
    if expected.is_empty() { return Ok(None); }
    let actual = match venue.positions_result().await.value { Ok(p) => p, Err(_) => return Ok(Some("venue positions unavailable or unreadable; retry after a complete refresh".into())) };
    for (symbol, held) in expected {
        let actual = actual.get(&symbol).cloned().unwrap_or_else(BigDec::zero);
        if !position_agrees(&held, &actual) {
            return Ok(Some(format!("{symbol}: restated account quantity {}, venue quantity {}; reconciliation required", held.to_s_f(),actual.to_s_f())));
        }
    }
    Ok(None)
}

fn expected_positions(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Result<HashMap<String, BigDec>, String>, EngineError> {
    if let Some(reason) = row_refusal(c, bot, now)? { return Ok(Err(reason)); }
    if untrusted(c, bot, now)? { return Ok(Err("split unresolved or within the two-day price quarantine".into())); }
    let own = basket::walk(c, bot, now)?;
    if own.restated_at_us.is_none() { return Ok(Ok(HashMap::new())); }
    let s = splits(c, bot, now)?;
    let assets: std::collections::BTreeSet<i64> = s.events.iter().map(|e| e.asset_id).collect();
    let mut expected: HashMap<String, BigDec> = HashMap::new();
    let mut stmt = c.prepare("SELECT id FROM bots WHERE user_id=?1 AND exchange_id=?2 ORDER BY id")?;
    let mut ids = stmt.query(params![bot.user_id,bot.exchange_id])?;
    while let Some(row) = ids.next()? {
        figures::budget::charge(1, 0).map_err(data)?;
        let id = row.get(0)?;
        let other = model::load_bot(c,id)?;
        if other.rust_placement().is_some() { return Ok(Err("an account order is still being reconciled".into())); }
        let waiting: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND status=0 AND external_status IN (0,1))", [id], |r| r.get(0))?;
        if waiting { return Ok(Err("an account order can still fill; waiting for settled quantities".into())); }
        if row_refusal(c,&other,now)?.is_some() || untrusted(c,&other,now)? { return Ok(Err("another account holding has an unresolved split".into())); }
        let walk = basket::walk(c,&other,now)?;
        for asset in &assets {
            let Some(t) = model::ticker_for_asset(c,bot,*asset)? else { return Ok(Err("split symbol is no longer uniquely listed".into())); };
            let held = walk.amounts.get(asset).cloned().unwrap_or_else(BigDec::zero);
            let total = expected.entry(t.base_code).or_insert_with(BigDec::zero);
            *total = &*total + &held;
        }
    }
    Ok(Ok(expected))
}
