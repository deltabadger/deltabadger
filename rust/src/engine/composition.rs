//! What a composition bot holds (PLAN index-as-ingredient §3.1: one `Composition` model and its one codec), shared
//! composition-operation state, and periodic settlement of liquidation and redeploy rows.
use super::{model::{self, Bot, Level}, amount::TxKind, EngineError};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

type Settings<'a> = Option<&'a Map<String, Value>>;

/// A composition bot's content: single assets at their weights and index groups (assets grouped by a rule).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composition { pub assets: Vec<(i64, f64)>, pub groups: Vec<Group>, pub weighting: String }

/// One index ingredient. `external_id` ("top-coins" or the category id) is its key within the bot.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub external_id: String,
    /// The ingredient's allocation; 1.0 for a lone `Bots::DcaIndex`.
    pub weight: f64,
    pub index_type: Option<String>, pub index_category_id: Option<String>, pub index_name: Option<String>, pub index_name_prefix: Option<String>,
    pub num_coins: Option<i64>, pub hold_all: bool, pub flattening: Option<f64>,
}

impl Group {
    /// The index keys of one settings object: a `Bots::DcaIndex` row's top level or one `index_groups` element. `weight`:
    /// a JSON number or a numeric string, else 0.0.
    pub fn from_settings(s: Settings) -> Group {
        let text = |key: &str| s.and_then(|s| s.get(key)?.as_str()).map(str::to_string);
        let weight = match s.and_then(|s| s.get("weight")) {
            Some(Value::Number(n)) => n.as_f64(), Some(Value::String(w)) => w.trim().parse().ok(), _ => None,
        }.filter(|w: &f64| w.is_finite());
        Group { external_id: external_id(s).to_string(), weight: weight.unwrap_or(0.0), index_type: index_type(s).map(str::to_string),
                index_category_id: index_category_id(s).map(str::to_string), index_name: text("index_name"), index_name_prefix: text("index_name_prefix"),
                num_coins: num_coins(s), hold_all: hold_all(s), flattening: flattening(s) }
    }
    /// The one encoder of an `index_groups` element: `from_settings`' keys, an absent field left out.
    pub fn settings(&self) -> Map<String, Value> {
        let mut m = Map::new();
        for (key, value) in [("index_type", self.index_type.clone().map(Value::from)), ("index_category_id", self.index_category_id.clone().map(Value::from)),
                             ("index_name", self.index_name.clone().map(Value::from)), ("index_name_prefix", self.index_name_prefix.clone().map(Value::from)),
                             ("num_coins", self.num_coins.map(Value::from)), ("hold_all", Some(Value::from(self.hold_all))),
                             ("allocation_flattening", self.flattening.map(Value::from)), ("weight", Some(Value::from(self.weight)))] {
            if let Some(value) = value { m.insert(key.into(), value); }
        }
        m
    }
}

impl Composition {
    /// Both stored spellings: a `Bots::DcaIndex` row is one group of weight 1.0 from its top-level keys and no assets; a
    /// `Bots::DcaMultiAsset` row is its `allocations` (none when unreadable: eligibility refuses those) and its
    /// `index_groups` array (an element that is not an object is skipped). Any other class has no composition.
    pub fn of(class: &str, s: Settings) -> Option<Composition> {
        match class {
            "Bots::DcaIndex" => Some(Composition { assets: vec![], groups: vec![Group { weight: 1.0, ..Group::from_settings(s) }], weighting: weighting(s) }),
            "Bots::DcaMultiAsset" => Some(Composition {
                assets: allocations(s).unwrap_or_default(),
                groups: s.and_then(|s| s.get("index_groups")?.as_array()).into_iter().flatten()
                    .filter_map(|g| g.as_object().map(|g| Group::from_settings(Some(g)))).collect(),
                weighting: weighting(s),
            }),
            _ => None,
        }
    }
    /// The one group of a bot whose whole content is one index: no assets, one group at weight 1.0 (the
    /// `allocations_balanced` tolerance).
    pub fn lone_group(&self) -> Option<&Group> {
        match self.groups.as_slice() { [g] if self.assets.is_empty() && (g.weight - 1.0).abs() <= 0.001 => Some(g), _ => None }
    }
}

impl Bot {
    /// The bot's content, decoded from either spelling.
    pub fn composition(&self) -> Option<Composition> { Composition::of(&self.bot_type, self.settings.as_object()) }
    /// The composition the engine runs as groups.
    fn grouped(&self) -> Option<Composition> {
        // PLAN index-as-ingredient slice 2b: a `Bots::DcaIndex` row and a basket stored with `index_groups` run as groups.
        if self.has_groups() { self.composition() } else { None }
    }
    /// Stored with `index_groups` (PLAN index-as-ingredient §3.3): the class and the array as stored, `[]` included, no
    /// decode. What the group table's readers key on; `has_groups` is the activation.
    pub fn stored_groups(&self) -> bool { self.bot_type == "Bots::DcaMultiAsset" && self.settings.get("index_groups").is_some_and(Value::is_array) }
    /// No decode: a `Bots::DcaIndex` row is always one group, a basket stored with `index_groups` its array's (eligibility
    /// refuses an empty or unreadable one).
    pub fn has_groups(&self) -> bool { self.bot_type == "Bots::DcaIndex" || self.stored_groups() }
    pub fn lone_group(&self) -> Option<Group> { self.grouped()?.lone_group().cloned() }
    pub fn index_external_ids(&self) -> Vec<String> { self.grouped().map(|c| c.groups.into_iter().map(|g| g.external_id).collect()).unwrap_or_default() }
}

/// PLAN index-as-ingredient §3.4: what a grouped composition was written from (`bots.transient_data`, Rust-only), set only by
/// the grouped composition write in that write's own transaction. `rev` is fresh per write, so a refusal that read an
/// earlier one never removes a later one (`forget`).
pub const COMPOSED_OF_KEY: &str = "rust_composed_of";

/// The settings a grouped composition is derived from: ingredients in stored order with every group field, the weighting,
/// the venue, the quote and the effective limit mode (it moves the newcomer probe from the ask to the last trade).
pub fn digest(bot: &Bot) -> String { digest_of(&bot.bot_type, bot.settings.as_object(), bot.exchange_id, bot.limit_distance().is_some()) }

/// `digest` of settings not loaded as a bot: the web writer's one composition-change decision (PLAN index-as-ingredient
/// slice 3: it both forgets `rust_composed_of` and marks the resync), `limit` the effective limit mode.
pub fn digest_of(class: &str, s: Settings, exchange_id: i64, limit: bool) -> String {
    use sha2::{Digest, Sha256};
    let c = Composition::of(class, s).unwrap_or_default();
    let groups: Vec<Value> = c.groups.iter().map(|g| json!([g.external_id, g.weight, g.index_type, g.index_category_id, g.index_name,
        g.index_name_prefix, g.num_coins, g.hold_all, g.flattening])).collect();
    let quote = s.and_then(|s| s.get("quote_asset_id")?.as_i64());
    let input = json!([c.assets, groups, c.weighting, exchange_id, quote, limit]);
    format!("{:x}", Sha256::digest(input.to_string()))
}

/// The revision of the bot's stored `rust_composed_of`, if any.
pub fn composed_rev(bot: &Bot) -> Option<String> { bot.transient.get(COMPOSED_OF_KEY)?.get("rev")?.as_str().map(str::to_string) }

/// Records a grouped composition, in the caller's write: the digest, each group's selected ranked row (`[indices.id,
/// ranked_at]`, null for a frozen group) and the effective ingredient shares. Returns the new revision.
pub fn record(c: &Connection, bot: &Bot, ranked: Value, shares: Value) -> Result<String, EngineError> {
    use rusqlite::OptionalExtension;
    let rev = uuid::Uuid::new_v4().to_string();
    let value = json!({ "rev": rev, "digest": digest(bot), "ranked": ranked, "shares": shares });
    c.execute(&format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{COMPOSED_OF_KEY}', json(?2)) WHERE id = ?1 AND json_type(transient_data) = 'object'"),
              rusqlite::params![bot.id, value.to_string()])?;
    // json_set on a non-object leaves it as it was: the members must not commit without their provenance.
    let landed: Option<String> = c.query_row(&format!("SELECT json_extract(transient_data, '$.{COMPOSED_OF_KEY}.rev') FROM bots WHERE id = ?1 AND json_type(transient_data) = 'object'"),
                                             [bot.id], |r| r.get(0)).optional()?.flatten();
    if landed.as_deref() != Some(rev.as_str()) { return Err(EngineError::Data(format!("bot {}: the composition record did not land", bot.id))); }
    Ok(rev)
}

/// Removes `rust_composed_of` only while it is still the revision `read` (None: none was stored when the refresh began).
pub fn forget(c: &Connection, bot_id: i64, read: Option<&str>) -> Result<(), EngineError> {
    c.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{COMPOSED_OF_KEY}') WHERE id = ?1 \
                        AND json_type(transient_data, '$.{COMPOSED_OF_KEY}') IS NOT NULL AND json_extract(transient_data, '$.{COMPOSED_OF_KEY}.rev') IS ?2"),
              rusqlite::params![bot_id, read])?;
    Ok(())
}

/// PLAN index-as-ingredient §3.4: whether the stored members are the composition of the settings as they stand, asked by
/// merge, split and ungroup (which run no refresh) inside their own transaction. A row not stored with `index_groups` keeps
/// today's rules (true). A grouped row: its `rust_composed_of` digest is the settings' own, nothing holds it as stale, and
/// every group's ranked row is the one the composition read (a later ranking, a retirement or a relisting refuses).
pub fn current(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<bool, EngineError> {
    if !bot.stored_groups() { return Ok(true); }
    let Some(of) = bot.transient.get(COMPOSED_OF_KEY) else { return Ok(false) };
    if of.get("digest").and_then(Value::as_str) != Some(digest(bot).as_str()) { return Ok(false); }
    if super::staleness::stale(c, bot, now)?.is_some() { return Ok(false); }
    for id in bot.index_external_ids() {
        if super::index::retired(c, &id)? { return Ok(false); }
        let Some(row) = super::index::ranked_row(c, &id)? else { return Ok(false) };
        if of.get("ranked").and_then(|r| r.get(&id)) != Some(&json!([row.id, row.ranked_at])) { return Ok(false); }
    }
    Ok(true)
}

/// The rules a bot's rule fields name, buy side; `sell_` + each is the sell side's.
pub const RULE_PREFIXES: [&str; 4] = ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"];

/// PLAN index-as-ingredient §3.2 step 3: what a bot whose composition changes shape no longer carries, so nothing dormant
/// wakes on what it now holds ("selling it is a new decision with a new sell sentence"). Every sell-side rule and every
/// flip rule whole (switch, action, value, the latch in `transient_data`); the stored sale (`sell_amount`,
/// `sell_quote_amount`, `smart_interval_base_amount`), the base cap and a base denomination. The one key list of the
/// last-group removal (slice 3) and the index bot's conversion (slice 4). True when anything was on or stored.
pub fn reset_sell_side(settings: &mut Map<String, Value>, transient: &mut Map<String, Value>) -> bool {
    let present = |v: Option<&Value>| v.is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_str().is_none_or(|t| !t.trim().is_empty()));
    let mut changed = false;
    for base in RULE_PREFIXES {
        for prefix in [base.to_string(), format!("sell_{base}")] {
            let action = format!("{prefix}_action");
            let flip = settings.get(&action).and_then(Value::as_str).is_some_and(|a| a != "pause");
            if !prefix.starts_with("sell_") && !flip { continue; }
            let switch = format!("{prefix}ed");
            changed |= settings.get(&switch) == Some(&Value::Bool(true)) || flip;
            if settings.contains_key(&switch) { settings.insert(switch, Value::Bool(false)); }
            if flip { settings.insert(action, json!("pause")); }
            if present(settings.get(&prefix)) { settings.insert(prefix.clone(), Value::Null); }
            transient.remove(&format!("{prefix}_condition_met_at"));
        }
    }
    for key in ["sell_amount", "sell_quote_amount", "smart_interval_base_amount", "base_amount_limit"] {
        if present(settings.get(key)) { changed = true; settings.insert(key.into(), Value::Null); }
    }
    if settings.get("base_amount_limited") == Some(&Value::Bool(true)) { changed = true; settings.insert("base_amount_limited".into(), Value::Bool(false)); }
    if settings.get("sell_denomination").and_then(Value::as_str).is_some_and(|d| d != "quote") { changed = true; settings.insert("sell_denomination".into(), json!("quote")); }
    changed
}

/// Bots::DcaIndex settings: 'top' or 'category'.
pub fn index_type(s: Settings<'_>) -> Option<&str> { s?.get("index_type")?.as_str() }
pub fn index_category_id(s: Settings<'_>) -> Option<&str> { s?.get("index_category_id")?.as_str().filter(|s| !s.trim().is_empty()) }
/// The `indices` row MarketData.get_top_coins reads: a category index with a present
/// index_category_id reads that row, anything else the Top coins row.
pub fn external_id(s: Settings<'_>) -> &str {
    match index_category_id(s) { Some(id) if index_type(s) == Some("category") => id, _ => "top-coins" }
}
/// `num_coins.to_i`: a JSON integer or a string of digits (the slider posts strings). None for anything else: refused.
pub fn num_coins(s: Settings) -> Option<i64> {
    match s?.get("num_coins")? { Value::Number(n) => n.as_i64(), Value::String(s) => s.trim().parse().ok(), _ => None }
}
/// `ActiveModel::Type::Boolean.new.cast(hold_all)` read as `hold_all?`: nil and "" are nil (false); false, 0, "0", "f", "F",
/// "false", "FALSE", "off", "OFF" are false; anything else is true.
pub fn hold_all(s: Settings) -> bool {
    match s.and_then(|s| s.get("hold_all")) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !(s.is_empty() || ["0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&s.as_str())),
        Some(_) => true,
    }
}
/// `allocation_flattening.to_f` (after_initialize `||= 0.0`): a JSON number or a numeric string. None: refused.
pub fn flattening(s: Settings) -> Option<f64> {
    match s.and_then(|s| s.get("allocation_flattening")) {
        None | Some(Value::Null) => Some(0.0),
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        Some(_) => None,
    }
}
/// settings.allocations as Bots::DcaMultiAsset#derive_composition reads it: (asset id, weight) in the stored order (the
/// JSON column keeps it). None when a key is not an id or a weight is not a JSON number (2.x's sliders store Floats).
pub fn allocations(s: Settings) -> Option<Vec<(i64, f64)>> {
    s?.get("allocations")?.as_object()?.iter().map(|(k, v)| Some((k.parse().ok()?, v.as_f64()?))).collect()
}
/// Bots::DcaMultiAsset#weighting: `super.presence || 'manual'`. A non-string is returned as its JSON, never "manual".
pub fn weighting(s: Settings) -> String {
    match s.and_then(|s| s.get("weighting")) {
        None | Some(Value::Null) => "manual".into(),
        Some(Value::String(s)) if s.trim().is_empty() => "manual".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

pub const SELLING_TTL_SECONDS: i64 = 15 * 60;

fn present(bot: &Bot, key: &str) -> bool {
    bot.transient.get(key).is_some_and(|v| match v {
        Value::Null | Value::Bool(false) => false,
        Value::Object(o) => !o.is_empty(), Value::Array(a) => !a.is_empty(),
        Value::String(s) => !s.trim().is_empty(), _ => true,
    })
}

pub fn in_flight(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<bool, EngineError> {
    busy(c, bot, now, true)
}

/// Split excludes redeploy, leaving it to its modal. The other blockers are shared.
pub fn busy(c: &Connection, bot: &Bot, now: DateTime<Utc>, redeploy: bool) -> Result<bool, EngineError> {
    Ok(present(bot, "rebalance_pending") || liquidation_in_flight(c, bot, now)?
        || (redeploy && redeploy_in_flight(c, bot)?))
}

pub fn liquidation_in_flight(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let selling = selling_request(&bot.transient["liquidation_selling_since"],now);
    let abandoned = unresolved_liquidations(c, bot)?;
    Ok(present(bot, "liquidation_pending") || selling || waiting(c, bot.id, "LIQUIDATION")? || abandoned)
}

pub(crate) fn unresolved_liquidations(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    let resolved: Vec<i64> = match bot.transient.get("liquidation_resolved_orders") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())).collect(),
        _ => vec![],
    };
    let mut s = c.prepare("SELECT id FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND external_status=4")?;
    let abandoned = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().any(|id| !resolved.contains(&id));
    Ok(abandoned)
}

/// D-LIQUIDATION-ONE-LINE: what the bot page shows as selling, from durable state only. `assets`: the asset ids of the
/// request's holdings, of waiting LIQUIDATION orders and of a liquidation intent being placed; `blocked`: any operation in
/// flight (`in_flight`), so a Sell stays inert while nothing is named too (an ambiguous marker, an unresolved order).
#[derive(Debug, Default, PartialEq)]
pub struct Selling { pub assets: Vec<i64>, pub blocked: bool, pub redeploying: bool, pub rebalancing: bool }

impl Selling {
    /// Whether the page shows any status row ("Selling …", "Redeploying …", "Rebalancing …").
    pub fn has_rows(&self) -> bool { !self.assets.is_empty() || self.redeploying || self.rebalancing }
}

pub fn selling(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Selling, EngineError> {
    let mut assets: Vec<i64> = vec![];
    let mut add = |id: i64| if !assets.contains(&id) { assets.push(id) };
    // ponytail: a null asset id is skipped, not resolved by the ledger; no Rust request writes one (liquidation::held).
    for holding in bot.transient[super::liquidation::REQUEST_KEY]["holdings"].as_array().into_iter().flatten() {
        if let Some(id) = holding[1].as_i64().or_else(|| holding[1].as_str()?.parse().ok()) { add(id); }
    }
    let mut s = c.prepare("SELECT base_asset_id FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND status=0 AND external_status IN (0,1) AND base_asset_id IS NOT NULL ORDER BY id")?;
    for id in s.query_map([bot.id], |r| r.get::<_, i64>(0))? { add(id?); }
    if bot.transient["liquidation_pending"]["state"] == "placing" {
        if let Some(intent) = super::placement::load_intent(c, bot, None)?.filter(|i| i.plan.kind == TxKind::Liquidation) { add(intent.plan.ticker.base_asset_id); }
    }
    let blocked = !assets.is_empty() || in_flight(c, bot, now)?;
    // D-LIQUIDATION-ONE-LINE: a halted operation shows its own prompt and error line, not a status row.
    let redeploying = bot.transient["redeploy_pending"]["state"] != "ambiguous"
        && (bot.transient[super::redeploy::REQUEST_KEY].is_object() || bot.transient["redeploy_pending"]["state"] == "placing" || waiting(c, bot.id, "REDEPLOY")?);
    let phase = &bot.transient["rebalance_pending"]["phase"];
    let rebalancing = phase == super::rebalance::PHASE_SELLING || phase == super::rebalance::PHASE_BUYING;
    Ok(Selling { assets, blocked, redeploying, rebalancing })
}

pub fn redeploy_in_flight(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    Ok(present(bot, "redeploy_pending") || present(bot, "rust_redeploy_request") || waiting(c, bot.id, "REDEPLOY")?)
}

fn waiting(c: &Connection, id: i64, kind: &str) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND transaction_type=?2 AND status=0 AND external_status IN (0,1))", (id, kind), |r| r.get(0))?)
}

pub(crate) fn name(kind: TxKind) -> Option<&'static str> {
    match kind { TxKind::Regular | TxKind::Rebalance => None, TxKind::Liquidation => Some("liquidation"), TxKind::Redeploy => Some("redeploy") }
}

pub(crate) fn start(c: &Connection, bot_id: i64, kind: TxKind, id: &str, symbol: Option<&str>) -> Result<(), EngineError> {
    let Some(name) = name(kind) else { return Ok(()) };
    let mut pending = json!({"id":id,"state":"placing"});
    if let Some(symbol) = symbol { pending["symbol"] = json!(symbol); }
    model::merge_transient_compact(c, bot_id, &[(&format!("{name}_pending"), pending)])
}

/// Idempotent promotion, preserving the generation nonce and the liquidation's symbol.
pub(crate) fn halt(c: &Connection, bot_id: i64, kind: TxKind, reason: &str, details: Value, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let Some(name) = name(kind) else { return Ok(false) };
    let bot = model::load_bot(c, bot_id)?;
    let key = format!("{name}_pending");
    let Some(mut pending) = bot.transient.get(&key).filter(|p| p.is_object()).cloned() else { return Ok(false) };
    if pending["state"] == "ambiguous" { return Ok(false); }
    pending["state"] = json!("ambiguous");
    model::merge_transient_compact(c, bot_id, &[(&key, pending)])?;
    let mut details = details;
    details["reason"] = json!(reason);
    model::log_activity(c, bot_id, &format!("{name}_ambiguous"), Level::Error, details, now)?;
    Ok(true)
}

/// A 2.x-only placing marker survived its worker. Unlike selling_since, it has no timestamp or TTL
/// (LiquidationState#promote_stale_liquidation_placement!); the engine's exclusive ownership permits promotion.
pub fn promote_orphans(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::locked(c, |c| {
        for kind in [TxKind::Liquidation, TxKind::Redeploy] { promote_orphan(c, bot_id, kind, now)?; }
        Ok(())
    })
}

pub(crate) fn promote_orphan(c: &Connection, bot_id: i64, kind: TxKind, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::locked(c, |c| {
        let bot = model::load_bot(c, bot_id)?;
        if bot.rust_placement().is_some() { return Ok(()); }
        let Some(name) = name(kind) else { return Ok(()) };
        if bot.transient[format!("{name}_pending")]["state"] == "placing" {
            halt(c, bot_id, kind, "placement intent survived its worker", json!({}), now)?;
        }
        Ok(())
    })
}


/// The watched ids belong to this sweep; a terminal row is not announced twice. `kind` None: both kinds.
pub(crate) fn waiting_rows(c: &Connection, bot: Option<i64>, kind: Option<TxKind>) -> Result<Vec<(i64,i64)>, EngineError> {
    let kind=kind.map(TxKind::as_str);
    let mut s=c.prepare("SELECT bot_id,id FROM transactions WHERE transaction_type IN ('LIQUIDATION','REDEPLOY') AND (?2 IS NULL OR transaction_type=?2) AND status=0 AND external_status IN (0,1) AND (?1 IS NULL OR bot_id=?1) ORDER BY id")?;
    let rows=s.query_map(rusqlite::params![bot,kind],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
    Ok(rows)
}
fn nonce() -> String { uuid::Uuid::new_v4().simple().to_string()[..12].to_string() }

/// In the transaction that abandons row `id` (Bot::StaleOrderResolver): an abandoned REDEPLOY raises `redeploy_pending`
/// (Redeployable#halt_abandoned_redeploys!) unless one stands, so no crash between the two lets trading resume.
pub(crate) fn halt_abandoned_redeploy(c: &Connection, bot: i64, id: i64) -> Result<(), EngineError> {
    let kind: String = c.query_row("SELECT transaction_type FROM transactions WHERE id=?1", [id], |r| r.get(0))?;
    if kind != "REDEPLOY" || present(&model::load_bot(c, bot)?, "redeploy_pending") { return Ok(()); }
    model::merge_transient_compact(c, bot, &[("redeploy_pending", json!({"id":nonce(),"state":"ambiguous"}))])
}

/// Bots of `watched` already holding a redeploy halt, read before the pass: an abandonment announces only a halt it raised.
pub(crate) fn halted(c: &Connection, watched: &[(i64,i64)]) -> Result<std::collections::BTreeSet<i64>, EngineError> {
    let mut out = std::collections::BTreeSet::new();
    for (bot, _) in watched { if present(&model::load_bot(c, *bot)?, "redeploy_pending") { out.insert(*bot); } }
    Ok(out)
}

/// Rows of `watched` the venue stopped reporting, announced in one line per bot (D-6C-5: every abandoned base is named).
/// A liquidation's unresolved row is its own halt; a redeploy's halt was raised with the abandonment
/// (halt_abandoned_redeploy) unless one stood before the pass (`before`), when nothing is announced.
pub(crate) fn halt_abandoned(c: &Connection, watched: &[(i64,i64)], before: &std::collections::BTreeSet<i64>, now: DateTime<Utc>) -> Result<(), EngineError> {
    let mut bots: std::collections::BTreeMap<(i64,String),Vec<String>>=Default::default();
    let mut watched=watched.to_vec();
    watched.sort_unstable();
    for (bot,id) in &watched {
        use rusqlite::OptionalExtension;
        let row:Option<(Option<String>,String)>=c.query_row("SELECT base,transaction_type FROM transactions WHERE id=?1 AND bot_id=?2 AND external_status=4",(*id,*bot),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((base,kind))=row {
            let names=bots.entry((*bot,kind)).or_default();
            if let Some(base)=base { if !names.contains(&base) { names.push(base); } }
        }
    }
    for ((bot,kind),names) in bots {
        let details=json!({"base":names.join(", "),"reason":"exchange stopped reporting the order"});
        if kind=="REDEPLOY" {
            if !before.contains(&bot) { model::log_activity(c,bot,"redeploy_ambiguous",Level::Error,details,now)?; }
        } else {
            model::log_activity(c,bot,"liquidation_ambiguous",Level::Error,details,now)?;
        }
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct Scan { pub waiting: Vec<(i64,i64)>, pub late: Vec<(i64,i64)> }
/// T2b: every waiting LIQUIDATION/REDEPLOY row to re-poll, and every terminal one still owing its proceeds.
pub(crate) fn scan(c: &Connection, now: DateTime<Utc>) -> Result<Scan,EngineError> {
    let waiting=waiting_rows(c,None,None)?;
    // ponytail: re-read late proceeds for seven days after the terminal update; widen if a venue reports them later.
    // 2.x has no closed_at column: updated_at records the poll that closed/cancelled the row.
    let mut s=c.prepare("SELECT bot_id,id FROM transactions WHERE transaction_type IN ('LIQUIDATION','REDEPLOY') AND status=0 AND external_status IN (2,3) AND amount_exec>0 AND COALESCE(quote_amount_exec,0)=0 AND updated_at>=?1 ORDER BY id")?;
    let late=s.query_map([crate::codec::format_time(now-chrono::Duration::days(7))],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
    Ok(Scan {waiting,late})
}

pub fn selling_request(request: &Value, now: DateTime<Utc>) -> bool {
    // Rust requests remain owned until compare-cleared, including taken requests after a crash.
    // 2.x markers have no holdings; only their page spinner's 15-minute TTL applies.
    request.is_object() && (request.get("holdings").is_some()
        || request["at"].as_i64().or_else(|| request["at"].as_str()?.parse().ok()).unwrap_or(0) > now.timestamp() - SELLING_TTL_SECONDS)
}

#[cfg(test)]
mod codec_tests {
    use super::*;
    fn of(class: &str, s: Value) -> Option<Composition> { Composition::of(class, s.as_object()) }

    #[test]
    fn a_legacy_index_row_is_one_group_of_weight_one() {
        let c = of("Bots::DcaIndex", json!({"index_type": "category", "index_category_id": "nasdaq-100", "index_name": "Nasdaq 100",
                                            "num_coins": "7", "hold_all": "0", "allocation_flattening": 0.5, "allocations": {"1": 1.0}})).unwrap();
        assert!(c.assets.is_empty(), "a legacy index row's allocations are not ingredients");
        assert_eq!(c.groups, vec![Group { external_id: "nasdaq-100".into(), weight: 1.0, index_type: Some("category".into()),
            index_category_id: Some("nasdaq-100".into()), index_name: Some("Nasdaq 100".into()), index_name_prefix: None,
            num_coins: Some(7), hold_all: false, flattening: Some(0.5) }]);
        assert_eq!(c.weighting, "manual");
        assert_eq!(c.lone_group().map(|g| g.external_id.as_str()), Some("nasdaq-100"));
        // A top index, and settings that are no object at all, follow the Top coins row as today.
        assert_eq!(of("Bots::DcaIndex", json!({"index_type": "top", "index_category_id": "x"})).unwrap().groups[0].external_id, "top-coins");
        let null = Composition::of("Bots::DcaIndex", None).unwrap();
        assert_eq!((null.groups.len(), null.groups[0].external_id.as_str(), null.groups[0].weight), (1, "top-coins", 1.0));
    }

    #[test]
    fn a_basket_reads_its_allocations_and_index_groups() {
        let c = of("Bots::DcaMultiAsset", json!({"allocations": {"5": 0.5}, "weighting": "market_cap", "index_groups": [
            {"index_type": "top", "num_coins": 10, "weight": 0.3},
            "garbage",
            {"index_type": "category", "index_category_id": "layer-1", "hold_all": true, "weight": "0.2"},
            {"index_type": "category", "index_category_id": "defi", "weight": "lots"}]})).unwrap();
        assert_eq!(c.assets, vec![(5, 0.5)]);
        assert_eq!(c.weighting, "market_cap");
        let groups: Vec<(&str, f64, Option<i64>, bool)> = c.groups.iter().map(|g| (g.external_id.as_str(), g.weight, g.num_coins, g.hold_all)).collect();
        assert_eq!(groups, vec![("top-coins", 0.3, Some(10), false), ("layer-1", 0.2, None, true), ("defi", 0.0, None, false)]);
        assert_eq!(c.lone_group(), None, "assets beside groups are no lone index");
        let lone = of("Bots::DcaMultiAsset", json!({"index_groups": [{"index_type": "top", "weight": 1}]})).unwrap();
        assert_eq!(lone.lone_group().map(|g| g.weight), Some(1.0));
    }

    #[test]
    fn missing_or_garbage_keys_give_no_groups() {
        for s in [json!({"allocations": {"1": 1.0}}), json!({"index_groups": {"a": {}}}), json!({"index_groups": "top"}), json!({"index_groups": [1, null]})] {
            assert!(of("Bots::DcaMultiAsset", s.clone()).unwrap().groups.is_empty(), "{s}");
        }
        assert_eq!(of("Bots::DcaMultiAsset", json!({"allocations": {"x": 1.0}})).unwrap().assets, vec![], "unreadable allocations");
        assert_eq!(Composition::of("Bots::DcaMultiAsset", None), Some(Composition { weighting: "manual".into(), ..Default::default() }));
        assert_eq!(of("Bots::Signal", json!({"index_groups": [{}]})), None);
        assert_eq!(of("Bots::DcaSingleAsset", json!({})), None);
    }

    #[test]
    fn a_non_finite_weight_reads_zero_and_a_zero_weight_group_is_no_lone_index() {
        for w in [json!("NaN"), json!("inf"), json!("1e999"), json!("-inf")] {
            let c = of("Bots::DcaMultiAsset", json!({"index_groups": [{"weight": w}]})).unwrap();
            assert_eq!(c.groups[0].weight, 0.0, "{w}");
            assert_eq!(c.lone_group(), None, "{w}");
        }
        assert_eq!(of("Bots::DcaMultiAsset", json!({"index_groups": [{"weight": 0}]})).unwrap().lone_group(), None);
        assert!(of("Bots::DcaMultiAsset", json!({"index_groups": [{"weight": "0.9995"}]})).unwrap().lone_group().is_some());
    }
}
