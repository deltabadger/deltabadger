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
        };
        Group { external_id: external_id(s).to_string(), weight: weight.unwrap_or(0.0), index_type: index_type(s).map(str::to_string),
                index_category_id: index_category_id(s).map(str::to_string), index_name: text("index_name"), index_name_prefix: text("index_name_prefix"),
                num_coins: num_coins(s), hold_all: hold_all(s), flattening: flattening(s) }
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
    /// The one group of a bot whose whole content is one index.
    pub fn lone_group(&self) -> Option<&Group> { if self.assets.is_empty() && self.groups.len() == 1 { self.groups.first() } else { None } }
}

impl Bot {
    /// The bot's content, decoded from either spelling.
    pub fn composition(&self) -> Option<Composition> { Composition::of(&self.bot_type, self.settings.as_object()) }
    /// The composition the engine runs as groups.
    fn grouped(&self) -> Option<Composition> {
        // slice 2: grouped MultiAsset activates here. Until then only a `Bots::DcaIndex` row runs as an index.
        if self.bot_type == "Bots::DcaIndex" { self.composition() } else { None }
    }
    pub fn has_groups(&self) -> bool { self.grouped().is_some_and(|c| !c.groups.is_empty()) }
    pub fn lone_group(&self) -> Option<Group> { self.grouped()?.lone_group().cloned() }
    pub fn index_external_ids(&self) -> Vec<String> { self.grouped().map(|c| c.groups.into_iter().map(|g| g.external_id).collect()).unwrap_or_default() }
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
}
