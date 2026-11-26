//! The bot menu's index entries (Bot::IndexSwitch): a portfolio follows an index ("Follow index"), an index bot changes its
//! index ("Change the index") or keeps its members at their weights as a portfolio ("Custom allocation"). The row changes class in place, inside one write transaction that calls no venue and no
//! market-data service.
use super::{composition, draft, write, Bot, For};
use crate::enums::BotStatus;
use crate::engine::{conditions::{self, Rule, Support, TRIGGERS}, eligibility, index, model};
use crate::web::{bots, i18n, layout::Ctx, wizard, WebError};
use crate::codec;
use crate::engine::accounting::{web_effective_amount as effective_amount, web_minimum as minimum, web_serialized as serialized};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// `Bots::DcaMultiAsset.stored_attributes[:settings]`, in 2.x's order (pinned by tests/pages.rs against 2.x).
pub const STORED_DCA_MULTI_ASSET: &[&str] = & ["quote_asset_id", "quote_amount", "interval", "allocations", "base_asset_ids", "weighting",
    "smart_intervaled", "smart_interval_quote_amount", "smart_interval_base_amount", "limit_ordered", "limit_order_pcnt_distance",
    "quote_amount_limited", "quote_amount_limit", "base_amount_limited", "base_amount_limit", "price_limited", "price_limit",
    "price_limit_range_lower_bound", "price_limit_range_upper_bound", "price_limit_timing_condition", "price_limit_value_condition",
    "price_limit_in_ticker_id", "price_limit_action", "sell_price_limited", "sell_price_limit", "sell_price_limit_range_lower_bound",
    "sell_price_limit_range_upper_bound", "sell_price_limit_timing_condition", "sell_price_limit_value_condition", "sell_price_limit_in_ticker_id",
    "sell_price_limit_action", "price_drop_limited", "price_drop_limit", "price_drop_limit_time_window_condition", "price_drop_limit_in_ticker_id",
    "price_drop_limit_action", "sell_price_drop_limited", "sell_price_drop_limit", "sell_price_drop_limit_time_window_condition",
    "sell_price_drop_limit_in_ticker_id", "sell_price_drop_limit_action", "moving_average_limited", "moving_average_limit_timing_condition",
    "moving_average_limit_value_condition", "moving_average_limit_in_ticker_id", "moving_average_limit_in_ma_type", "moving_average_limit_in_timeframe",
    "moving_average_limit_in_period", "moving_average_limit_action", "sell_moving_average_limited", "sell_moving_average_limit_timing_condition",
    "sell_moving_average_limit_value_condition", "sell_moving_average_limit_in_ticker_id", "sell_moving_average_limit_in_ma_type",
    "sell_moving_average_limit_in_timeframe", "sell_moving_average_limit_in_period", "sell_moving_average_limit_action", "indicator_limited",
    "indicator_limit", "indicator_limit_timing_condition", "indicator_limit_value_condition", "indicator_limit_in_ticker_id",
    "indicator_limit_in_indicator", "indicator_limit_in_timeframe", "indicator_limit_action", "sell_indicator_limited", "sell_indicator_limit",
    "sell_indicator_limit_timing_condition", "sell_indicator_limit_value_condition", "sell_indicator_limit_in_ticker_id",
    "sell_indicator_limit_in_indicator", "sell_indicator_limit_in_timeframe", "sell_indicator_limit_action", "start_time_enabled",
    "start_time_mode", "start_time_of_day", "start_at", "rebalance_enabled", "rebalance_threshold", "direction", "sell_amount", "sell_interval",
    "sell_denomination", "sell_quote_amount"];

/// `Bots::DcaIndex.stored_attributes[:settings]` (the Follow/Change switch's target), pinned with the list above.
pub const STORED_DCA_INDEX: &[&str] = & ["quote_asset_id", "quote_amount", "interval", "num_coins", "allocation_flattening", "index_type",
    "index_category_id", "index_name", "index_name_prefix", "hold_all", "smart_intervaled", "smart_interval_quote_amount",
    "smart_interval_base_amount", "limit_ordered", "limit_order_pcnt_distance", "start_time_enabled", "start_time_mode", "start_time_of_day",
    "start_at", "rebalance_enabled", "rebalance_threshold"];

pub enum Switched {
    /// Not this user's bot, or deleted (Bots::Botable#set_bot).
    Missing,
    /// Not an index bot: `head :not_found`.
    NotIndex,
    /// A bot whose page this build does not serve.
    Unported(&'static str),
    /// Bot::IndexSwitch::Refused, or the install's guard: nothing written.
    Refused(String),
    Done,
}

fn t(locale: &str, key: &str) -> String { i18n::text(locale, &format!("errors.bots.index_switch.{key}"), &[]) }

/// Bot::IndexSwitch.refusal, plus the engine's own fence (write::engine_holds). 2.x's `poll_in_flight?` has no analogue: a
/// stopped bot has no condition check in this engine, and the switch bumps the run generation.
fn refusal(c: &Connection, bot: &model::Bot, now: chrono::DateTime<chrono::Utc>) -> Result<Option<&'static str>, WebError> {
    if !matches!(bot.status, BotStatus::Stopped | BotStatus::Created) { return Ok(Some("stop_first")); }
    if in_flight(c, bot, now)? || write::engine_holds(c, bot)? { return Ok(Some("in_flight")); }
    Ok(None)
}

/// `rebalance_pending? || liquidation_in_flight? || redeploy_in_flight?` (Bot::Merge.mergeable? and Bot::IndexSwitch.refusal).
pub(crate) fn in_flight(c: &Connection, bot: &model::Bot, now: chrono::DateTime<chrono::Utc>) -> Result<bool, WebError> {
    Ok(crate::engine::composition::in_flight(c, bot, now)?)
}

/// Split leaves redeploy to its modal.
pub(crate) fn busy(c: &Connection, bot: &model::Bot, redeploy: bool, now: chrono::DateTime<chrono::Utc>) -> Result<bool, WebError> {
    Ok(crate::engine::composition::busy(c, bot, now, redeploy)?)
}

/// POST /bots/:id/custom_allocation (Bots::CustomAllocationsController#create, Bot::IndexSwitch.customize!): the index bot becomes
/// a portfolio of its in-index members that still trade at its venue and quote, at their weights rounded to the slider's 0.1%.
///
/// D-IDX-CUSTOM-STORED: 2.x re-derives the index in the request first (market data and venue probes); this build reads the
/// stored members, which the engine's resync keeps, and refuses `in_flight` while a resync it was asked for is still due.
pub fn customize(c: &Connection, ctx: &Ctx, owner: i64, id: i64) -> Result<Switched, WebError> {
    // PLAN index-as-ingredient §6.2: a grouped bot's Custom allocation ungroups every group.
    if grouped_row(c, owner, id)? { return grouped_route(c, ctx, owner, id, "ungroup_index_id", "*"); }
    match customize_inner(c, ctx, owner, id) {
        Err(e) if super::unreadable(&e) => Ok(Switched::Unported("a bot beyond the supported history, member or numeric bounds")),
        other => other,
    }
}

fn customize_inner(c: &Connection, ctx: &Ctx, owner: i64, id: i64) -> Result<Switched, WebError> {
    const FROM: &str = "Bots::DcaIndex";
    const TO: &str = "Bots::DcaMultiAsset";
    let tx = model::immediate(c)?;
    let now = ctx.app.now();
    let class: Option<Option<String>> = tx.query_row("SELECT type FROM bots WHERE id=?1 AND user_id=?2 AND status<>3", (id, owner), |r| r.get(0)).optional()?;
    match class { None => return Ok(Switched::Missing), Some(class) if class.as_deref() != Some(FROM) => return Ok(Switched::NotIndex), _ => {} }
    let engine_bot = model::load_bot(&tx, id)?;
    if let Some(key) = refusal(&tx, &engine_bot, ctx.app.now())? { return Ok(Switched::Refused(t(ctx.locale, key))); }
    let (provider, _) = bots::market_data(&tx, &ctx.app)?;
    if let Some(reason) = super::refusal(&tx, id, provider, For::Settings)? { return Ok(Switched::Unported(reason)); }
    if engine_bot.transient.get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()) { return Ok(Switched::Refused(t(ctx.locale, "in_flight"))); }
    let Some(draft) = draft::Draft::load(&tx, owner, id, ctx.locale)? else { return Ok(Switched::Missing) };
    let source = draft.original.clone();
    if let Some(reason) = source.unrendered() { return Ok(Switched::Unported(reason)); }
    // `source.bot_index_assets.in_index.where(asset_id: tradeable).order(target_allocation: :desc)`, each weight `to_f`.
    let quote = source.settings.get("quote_asset_id").and_then(Value::as_i64);
    let mut s = tx.prepare("SELECT asset_id, CAST(target_allocation AS REAL) FROM bot_index_assets WHERE bot_id=?1 AND in_index=1 AND asset_id IN \
        (SELECT base_asset_id FROM tickers WHERE available=1 AND trading_enabled=1 AND exchange_id=?2 AND quote_asset_id IS ?3) ORDER BY target_allocation DESC")?;
    let mut weights: Map<String, Value> = s.query_map((id, source.exchange.id, quote), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?)))?
        .map(|row| row.map(|(asset, weight)| (asset.to_string(), Value::from(weight.unwrap_or(0.0)))))
        .collect::<Result<_, _>>()?;
    drop(s);
    if weights.is_empty() { return Ok(Switched::Refused(t(ctx.locale, "no_members"))); }
    draft::normalize(&mut weights)?;
    let basket = switch(&tx, ctx, owner, id, &draft, source, (FROM, TO, super::Kind::Basket, STORED_DCA_MULTI_ASSET), |_, settings| {
        settings.insert("allocations".into(), Value::Object(weights));
        Ok(())
    })?;
    let basket = match basket { Ok(bot) => bot, Err(messages) => return Ok(Switched::Refused(messages)) };
    // The portfolio's after-save composition: members it left out become exited.
    composition::reconcile(&tx, &basket, now)?;
    commit(tx, ctx, id)
}

/// The guard over the switched bot, then the commit; a refusal rolls everything back.
fn commit(tx: rusqlite::Transaction<'_>, ctx: &Ctx, id: i64) -> Result<Switched, WebError> {
    if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) {
        tx.rollback()?;
        return Ok(Switched::Refused(write_refused(ctx.locale, &refusal.reason())));
    }
    tx.commit()?;
    Ok(Switched::Done)
}

fn write_refused(locale: &str, reason: &str) -> String { i18n::text(locale, "engine.write_refused", &[("reason", i18n::Arg::Text(reason))]) }

/// Bot::IndexSwitch.switch! once its refusals have passed, in the caller's write transaction: the carry the source class
/// computes (set_missed_quote_amount), `becomes!` (the target class's after_initialize defaults over the same settings, a
/// default the source computed kept), only the target's own settings, `assign` (the switch's yield), the label nulled unless
/// the user gave one, and the save with its settings clock, carry capture and generated name. The run generation moves, so
/// an engine write already in flight is fenced. Returns the switched bot as it now loads, or the validation errors that
/// refuse it.
#[allow(clippy::too_many_arguments)]
fn switch(tx: &Connection, ctx: &Ctx, owner: i64, id: i64, draft: &draft::Draft, mut source: Bot, (from, to, kind, stored): (&str, &str, super::Kind, &[&str]),
          assign: impl FnOnce(&Connection, &mut Map<String, Value>) -> Result<(), WebError>) -> Result<Result<Bot, String>, WebError> {
    let now = ctx.app.now();
    // A name the user never changed follows what the bot now holds.
    let generated = source.clone().default_label(ctx.locale);
    let renamed = !source.label_unsaved && source.label != generated;
    // The carry the source computes (set_missed_quote_amount), capped as on any settings edit (Bot::Accountable).
    let carry = write::pending(tx, &draft.original, now)?;
    let at = codec::format_time(now);
    // becomes!: the row is loaded as the target over the source's initialised settings (`becomes!` shares the attributes), so
    // a default the source computed (a Smart Intervals amount off its venue minimums) is kept, not recomputed for the target.
    one(tx.execute("UPDATE bots SET type=?1, settings=?2 WHERE id=?3 AND user_id=?4 AND type=?5 AND status<>3",
        (to, Value::Object(draft.original.settings.clone()).to_string(), id, owner, from))?)?;
    let target = Bot::find(tx, owner, id, For::Page, ctx.locale)?.ok_or_else(|| super::data("switched bot disappeared".into()))?;
    // The target's own settings, and those of each rule switched on that this build runs on the target kind (an index bot's
    // cap and price rule); a rule that is off is not carried (2.x carries none to an index bot).
    let source_settings = Value::Object(target.settings.clone());
    let runs = |key: &str| STORED_DCA_MULTI_ASSET.contains(&key) && rule_of(key).is_some_and(|(rule, selling, prefix)| {
        let flag = prefix.as_ref().map_or_else(|| "quote_amount_limited".to_string(), |p| format!("{p}ed"));
        source_settings.get(&flag) == Some(&Value::Bool(true))
            && conditions::support(kind, rule, selling, &prefix.map_or_else(|| "pause".into(), |p| conditions::action(&source_settings, &p))) == Support::Implemented
    });
    let mut settings: Map<String, Value> = stored.iter().filter_map(|key| target.settings.get(*key).map(|v| (key.to_string(), v.clone()))).collect();
    settings.extend(target.settings.iter().filter(|(key, _)| !stored.contains(&key.as_str()) && runs(key)).map(|(key, v)| (key.clone(), v.clone())));
    assign(tx, &mut settings)?;
    source.settings = settings.clone();
    source.kind = kind;
    let carry = write::stored_carry(&draft.raw_transient, serialized(minimum(carry, effective_amount(&source)?)?)?);
    let label = if renamed { Some(source.label.clone()) } else { None };
    // The settings column as ActiveRecord's JSON coder writes it (an index's name may hold <, > or &).
    one(tx.execute("UPDATE bots SET settings=?1, label=?2, settings_changed_at=?3, updated_at=?3, \
        transient_data=json_remove(json_set(transient_data,'$.run_generation',coalesce(json_extract(transient_data,'$.run_generation'),0)+1),'$.rust_composed_index') WHERE id=?4 AND type=?5",
        (bots::json(&Value::Object(settings)), &label, &at, id, to))?)?;
    for (key, value) in [("missed_quote_amount", carry), ("missed_quote_amount_was_set", Value::Null)] {
        if draft.raw_transient.get(key) != Some(&value) {
            one(tx.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,json(?2)) WHERE id=?3", (format!("$.{key}"), value.to_string(), id))?)?;
        }
    }
    let switched = Bot::find(tx, owner, id, For::Page, ctx.locale)?.ok_or_else(|| super::data("switched bot disappeared".into()))?;
    // `target.save!`: the target class's validations over the switched row, as its settings save runs them; RecordInvalid
    // is Refused with the full messages, and the caller's transaction is dropped unwritten.
    let mut saved = draft::Draft::from_bot(switched.clone());
    saved.as_unsaved_of(&draft.original);
    saved.validate(tx, draft::ValidationContext::Update, now, bots::market_data(tx, &ctx.app)?.1, ctx.locale)?;
    if !saved.errors.is_empty() {
        let messages: Vec<String> = saved.errors.iter().map(|e| super::mcp_input::full_messages(std::slice::from_ref(e))).collect();
        return Ok(Err(draft::sentence(&messages.iter().map(String::as_str).collect::<Vec<_>>(), ctx.locale)));
    }
    // Automation::Labelable#set_default_label: the generated name is stored.
    if !renamed { one(tx.execute("UPDATE bots SET label=?1 WHERE id=?2", (&switched.label, id))?)?; }
    Ok(Ok(switched))
}

/// What the bot menu's index picker offers bot `id` (Bots::IndicesController#set_indices): Index.followable_on its venue and
/// quote, an index bot's own index left out.
pub(crate) fn offered(c: &Connection, ctx: &Ctx, id: i64) -> Result<Vec<wizard::index::IndexTile>, WebError> { offered_except(c, ctx, id, &[]) }

/// `offered`, with `held` (a grouped basket's indexes, for Add index and a group's Change) left out too.
pub(crate) fn offered_except(c: &Connection, ctx: &Ctx, id: i64, held: &[String]) -> Result<Vec<wizard::index::IndexTile>, WebError> {
    let (class, settings, exchange_id, venue): (Option<String>, String, i64, Option<String>) = c.query_row(
        "SELECT b.type, b.settings, e.id, e.type FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE b.id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    let settings: Value = serde_json::from_str(&settings).map_err(|e| super::data(format!("bot {id} settings: {e}")))?;
    // Bots::DcaIndex#current_index: the category's row (any source), else the internal Top Coins. A lone grouped basket's
    // group is its current index too (PLAN index-as-ingredient slice 4a): re-picking it would rebuild the group.
    let lone = settings.as_object().and_then(super::lone_group_of).filter(|_| class.as_deref() == Some("Bots::DcaMultiAsset"));
    let lone_external = lone.as_ref().map(id_of);
    let followed = if class.as_deref() == Some("Bots::DcaIndex") { settings.as_object().cloned() } else { lone };
    let current: Option<i64> = match &followed { None => None, Some(settings) => {
        match (settings.get("index_type").and_then(Value::as_str), settings.get("index_category_id").and_then(Value::as_str).filter(|id| !crate::v2::blank(id))) {
            (Some("category"), Some(category)) => c.query_row("SELECT id FROM indices WHERE external_id=?1 ORDER BY id LIMIT 1", [category], |r| r.get(0)).optional()?,
            _ => c.query_row("SELECT id FROM indices WHERE external_id='top-coins' AND source='internal' ORDER BY id LIMIT 1", [], |r| r.get(0)).optional()?,
        }
    } };
    let quote = settings.get("quote_asset_id").and_then(Value::as_i64);
    // A lone group's index is left out by its external id, whichever provider's row a tile is (an index bot keeps 2.x's row rule).
    let exclude: Vec<String> = lone_external.into_iter().chain(held.iter().cloned()).collect();
    wizard::index::tiles(c, &ctx.app, Some(&wizard::index::Followable { exchange_id, class: venue.as_deref().unwrap_or_default(), quote, current, exclude: &exclude }))
}

/// POST /bots/:id/index (Bots::IndicesController#create, Bot::IndexSwitch.follow!): a portfolio starts following the index it
/// picked ("Follow index"), or an index bot changes its index ("Change the index"), with a new bot's size for it: a bounded
/// index held whole, any other ten members. `category` is the tile's `index_category_id`.
///
/// D-IDX-FOLLOW-LATE: 2.x derives the new members in the request (market data and venue probes for newcomers); this build
/// leaves the engine a resync marker, and the engine derives them once woken (engine::run::resync_bot).
/// D-IDX-COINGECKO: where this engine cannot derive (or run) the index bot (an install with no provider: neither data-api
/// nor a CoinGecko key), 2.x switches and this build refuses, writing nothing. On a CoinGecko key it switches, and the
/// engine holds the bot until CoinGecko ranks the new index (staleness::ranking_stale).
pub fn follow(c: &Connection, ctx: &Ctx, owner: i64, id: i64, category: Option<&str>) -> Result<Switched, WebError> {
    // PLAN index-as-ingredient §6.2: on a grouped bot, every ingredient replaced by the picked index.
    if grouped_row(c, owner, id)? { return grouped_route(c, ctx, owner, id, "follow_index", category.unwrap_or_default()); }
    match follow_inner(c, ctx, owner, id, category) {
        Err(e) if super::unreadable(&e) => Ok(Switched::Unported("a bot beyond the supported history, member or numeric bounds")),
        other => other,
    }
}

fn follow_inner(c: &Connection, ctx: &Ctx, owner: i64, id: i64, category: Option<&str>) -> Result<Switched, WebError> {
    const TO: &str = "Bots::DcaIndex";
    let tx = model::immediate(c)?;
    let now = ctx.app.now();
    let class: Option<Option<String>> = tx.query_row("SELECT type FROM bots WHERE id=?1 AND user_id=?2 AND status<>3", (id, owner), |r| r.get(0)).optional()?;
    let Some(class) = class else { return Ok(Switched::Missing) };
    let from = match class.as_deref() { Some(from @ ("Bots::DcaIndex" | "Bots::DcaMultiAsset")) => from.to_string(), _ => return Ok(Switched::NotIndex) };
    let (provider, configured) = bots::market_data(&tx, &ctx.app)?;
    if !configured { return Ok(Switched::NotIndex) }
    let Some(index) = offered(&tx, ctx, id)?.into_iter().find(|tile| category.is_some_and(|category| tile.external_id == category)) else {
        return Ok(Switched::Refused(t(ctx.locale, "not_offered")));
    };
    let engine_bot = model::load_bot(&tx, id)?;
    // `bot.try(:selling?)`: an index bot cannot sell, and a selling portfolio would silently start buying.
    let refused = match refusal(&tx, &engine_bot, ctx.app.now())? { None if from == "Bots::DcaMultiAsset" && engine_bot.selling() => Some("selling"), other => other };
    if let Some(key) = refused { return Ok(Switched::Refused(t(ctx.locale, key))); }
    if let Some(reason) = super::refusal(&tx, id, provider, For::Settings)? { return Ok(Switched::Unported(reason)); }
    let Some(draft) = draft::Draft::load(&tx, owner, id, ctx.locale)? else { return Ok(Switched::Missing) };
    let source = draft.original.clone();
    if let Some(reason) = source.unrendered() { return Ok(Switched::Unported(reason)); }
    if let Some(reason) = wizard::index::refusal(&tx, &ctx.app, &source.exchange.class)? { return Ok(Switched::Refused(write_refused(ctx.locale, reason))); }
    let switched = switch(&tx, ctx, owner, id, &draft, source, (&from, TO, super::Kind::Index, STORED_DCA_INDEX), |c, settings| {
        // Index#bot_settings, then the size: `num_coins = bounded_universe_size || 10`, `hold_all = size.present?`.
        let top = index.external_id == "top-coins";
        let prefix = wizard::index_create::count_named(Some(&index.external_id)).map(|(prefix, _)| prefix);
        for (key, value) in [("index_type", json!(if top { "top" } else { "category" })), ("index_category_id", if top { Value::Null } else { json!(index.external_id) }),
                             ("index_name", if top { Value::Null } else { json!(index.stored_name) }), ("index_name_prefix", if top { Value::Null } else { json!(prefix) })] {
            store(settings, key, value);
        }
        let bounded = wizard::index_create::bounded(wizard::index_create::index_row(c, settings)?.as_ref());
        store(settings, "num_coins", json!(bounded.unwrap_or(10)));
        store(settings, "hold_all", json!(bounded.is_some()));
        if matches!(settings.get("allocation_flattening"), None | Some(Value::Null | Value::Bool(false))) { settings.insert("allocation_flattening".into(), json!(0.0)); }
        Ok(())
    })?;
    if let Err(messages) = switched { return Ok(Switched::Refused(messages)); }
    // after_update_commit :resync_index_composition (the index definition changed), and the request's own derivation.
    index::mark_resync(&tx, id, now).map_err(|e| super::data(format!("{e:?}")))?;
    commit(tx, ctx, id)
}

/// The rule a stored rule setting belongs to, its side, and the side's trigger prefix (None for the cap).
fn rule_of(key: &str) -> Option<(Rule, bool, Option<String>)> {
    if matches!(key, "quote_amount_limited" | "quote_amount_limit") { return Some((Rule::Cap, false, None)); }
    let (selling, rest) = match key.strip_prefix("sell_") { Some(rest) => (true, rest), None => (false, key) };
    let (rule, prefix) = TRIGGERS.iter().filter(|(_, prefix)| rest.starts_with(prefix)).max_by_key(|(_, prefix)| prefix.len())?;
    Some((*rule, selling, Some(format!("{}{prefix}", if selling { "sell_" } else { "" }))))
}

/// A store accessor's write: ActiveRecord::Store assigns only a value that differs, by 2.x's ==, from what the store reads
/// (so a nil never adds an absent key).
fn store(settings: &mut Map<String, Value>, key: &str, value: Value) {
    let old = settings.get(key).unwrap_or(&Value::Null);
    let same = match (old.as_f64(), value.as_f64()) { (Some(a), Some(b)) => a == b, _ => *old == value };
    if !same { settings.insert(key.into(), value); }
}

fn one(rows: usize) -> Result<(), WebError> {
    if rows == 1 { Ok(()) } else { Err(super::data("index switch lost its bot".into())) }
}

#[derive(askama::Template)]
#[template(path = "bots/indices/new.html")]
struct Picker<'a> { v: &'a Ctx, title: String, none: String, action: String, csrf: &'a str, tiles: Vec<wizard::index::IndexTile>, error: Option<String> }

/// bots/indices/new's content: "Change the index" or "Follow index" over the offered indices' tiles, which post to the switch,
/// or the sentence saying there are none at this venue and quote.
pub fn picker(c: &Connection, ctx: &Ctx, bot: &Bot, csrf: &str) -> Result<String, WebError> {
    use askama::Template;
    let title = ctx.t(if bot.kind == super::Kind::Index || bot.lone_group().is_some() { "bot.index_switch.change_index" } else { "bot.index_switch.follow_index" });
    let quote = bot.quote_asset.as_ref().and_then(|asset| asset.symbol.clone()).unwrap_or_default();
    let none = i18n::text(ctx.locale, "bot.index_switch.none", &[("exchange", i18n::Arg::Text(&bot.exchange.name)), ("quote", i18n::Arg::Text(&quote))]);
    Ok(Picker { v: ctx, title, none, action: ctx.path(&format!("/bots/{}/index", bot.id)), csrf, tiles: offered(c, ctx, bot.id)?, error: None }.render()?)
}

/// PLAN index-as-ingredient slice 5: the picker of a grouped basket's Add index (`change`: None) or of one group's Change the
/// index, every held index left out, posting to `action`; `error`: a refusal, shown above the tiles.
pub fn group_picker(c: &Connection, ctx: &Ctx, bot: &Bot, csrf: &str, change: Option<&str>, error: Option<String>) -> Result<String, WebError> {
    use askama::Template;
    let title = ctx.t(if change.is_some() { "bot.index_switch.change_index" } else { "bot.dca_multi_asset.add_index" });
    let quote = bot.quote_asset.as_ref().and_then(|asset| asset.symbol.clone()).unwrap_or_default();
    let none = i18n::text(ctx.locale, "bot.index_switch.none", &[("exchange", i18n::Arg::Text(&bot.exchange.name)), ("quote", i18n::Arg::Text(&quote))]);
    let action = ctx.path(&match change { Some(id) => format!("/bots/{}/index_groups/{id}", bot.id), None => format!("/bots/{}/index_groups", bot.id) });
    Ok(Picker { v: ctx, title, none, action, csrf, tiles: offered_except(c, ctx, bot.id, &bot.group_ids())?, error }.render()?)
}

/// Bots::Botable#set_bot and Bots::IndicesController#require_composition_bot: None for a bot that is not this user's or is
/// deleted; false for one that is neither a portfolio nor an index bot, or on an install without market data (`head :not_found`).
pub fn composition_bot(c: &Connection, ctx: &Ctx, owner: i64, id: i64) -> Result<Option<bool>, WebError> {
    let class: Option<Option<String>> = c.query_row("SELECT type FROM bots WHERE id=?1 AND user_id=?2 AND status<>3", (id, owner), |r| r.get(0)).optional()?;
    let Some(class) = class else { return Ok(None) };
    Ok(Some(matches!(class.as_deref(), Some("Bots::DcaIndex" | "Bots::DcaMultiAsset")) && bots::market_data(c, &ctx.app)?.1))
}

/// What an ingredient operation did, for the writer's tail (PLAN index-as-ingredient §6.2, slice 3).
pub(super) struct Ran { pub last_gone: bool }

fn field(fields: &Map<String, Value>, key: &str) -> Option<String> {
    fields.get(key).filter(|v| super::mcp_input::present(v)).map(|v| match v { Value::String(s) => s.trim().to_string(), other => other.to_string() })
}

/// The groups of the candidate, as stored objects.
fn groups_of(settings: &Map<String, Value>) -> Vec<Map<String, Value>> {
    settings.get("index_groups").and_then(Value::as_array).into_iter().flatten().map(|g| g.as_object().cloned().unwrap_or_default()).collect()
}
fn id_of(group: &Map<String, Value>) -> String { crate::engine::composition::external_id(Some(group)).to_string() }

/// The tile `external_id` as the bot menu's picker offers it on the **candidate's** venue and quote, with `follow_inner`'s
/// provider and venue checks (Codex slice 3 r1 #6); Err: the refusal.
fn offered_here(c: &Connection, ctx: &Ctx, draft: &draft::Draft, external_id: &str) -> Result<Result<wizard::index::IndexTile, String>, WebError> {
    if !bots::market_data(c, &ctx.app)?.1 { return Ok(Err(t(ctx.locale, "not_offered"))); }
    if let Some(reason) = wizard::index::refusal(c, &ctx.app, &draft.candidate.exchange.class)? { return Ok(Err(write_refused(ctx.locale, reason))); }
    let quote = draft.candidate.settings.get("quote_asset_id").and_then(Value::as_i64);
    let here = wizard::index::Followable { exchange_id: draft.candidate.exchange.id, class: &draft.candidate.exchange.class, quote, current: None, exclude: &[] };
    let Some(tile) = wizard::index::tiles(c, &ctx.app, Some(&here))?.into_iter().find(|tile| tile.external_id == external_id) else { return Ok(Err(t(ctx.locale, "not_offered"))) };
    // The row the engine composes from, when one is ranked, must be offered here itself (Codex slice 3 r1 #4).
    if let Some(ranked) = crate::engine::index::ranked_row(c, external_id)? {
        if !wizard::index::row_offered(c, ranked.id, &here)? { return Ok(Err(t(ctx.locale, "not_offered"))); }
    }
    Ok(Ok(tile))
}

/// A new group for `tile` at `weight`, as Follow index sets an index bot's keys (`follow_inner`): a bounded index held
/// whole, any other ten members, no flattening; written by the one encoder (`Group::settings`).
fn group_for(c: &Connection, tile: &wizard::index::IndexTile, weight: f64) -> Result<Map<String, Value>, WebError> {
    let top = tile.external_id == "top-coins";
    let mut group = crate::engine::composition::Group { external_id: tile.external_id.clone(), weight, index_type: Some(if top { "top" } else { "category" }.into()),
        index_category_id: (!top).then(|| tile.external_id.clone()), index_name: if top { None } else { tile.stored_name.clone() },
        index_name_prefix: if top { None } else { wizard::index_create::count_named(Some(&tile.external_id)).map(|(prefix, _)| prefix.to_string()) },
        num_coins: None, hold_all: false, flattening: Some(0.0) };
    let bounded = draft::group_bound(c, &group.settings())?;
    group.num_coins = Some(bounded.unwrap_or(10));
    group.hold_all = bounded.is_some();
    Ok(group.settings())
}

/// PLAN index-as-ingredient §6.2: the one ingredient operation a settings post carries, applied to the draft's candidate
/// before validation, inside the writer's transaction. Ok(Err): the refusal (nothing is written). A plain basket takes only
/// `add_index` (its first group, slice 4c); every other op needs a row already stored with groups. Every operation takes the index switch's full
/// refusal, an open order and a redeploy 2.x still holds (or a queue that cannot be read). Losing the last group is refused
/// while the bot sells; a buying bot's dormant base sale and base cap are cleared in the same write (rule B).
pub(super) fn ingredients(c: &Connection, ctx: &Ctx, draft: &mut draft::Draft, fields: &Value, now: chrono::DateTime<chrono::Utc>) -> Result<Result<Option<Ran>, String>, WebError> {
    use crate::web::bot::action_params::INGREDIENT_OPS;
    let Some(fields) = fields.as_object() else { return Ok(Ok(None)) };
    let posted: Vec<&str> = INGREDIENT_OPS.iter().copied().filter(|key| field(fields, key).is_some()).collect();
    if posted.is_empty() { return Ok(Ok(None)); }
    let refuse = |key: &str| Ok(Err(t(ctx.locale, key)));
    let op = match posted.as_slice() { [op] if *op != "change_index_to" => *op, ["change_index_id", "change_index_to"] => "change_index_id", _ => return refuse("not_offered") };
    // PLAN slice 4c: a plain basket gains its first group by `add_index` on the web form (no 2.x recording; Follow index on
    // a plain basket keeps 2.x's switch to an index bot, which the next start converts). Every other op needs a group.
    let first = !super::stored_groups(&draft.raw_settings);
    if first && op != "add_index" { return refuse("not_offered"); }
    let id = draft.original.id;
    let engine = model::load_bot(c, id)?;
    if let Some(key) = refusal(c, &engine, now)? { return refuse(key); }
    if write::orders_may_execute(c, id)? || super::split::redeploying(c, &ctx.app, &engine, "Bots::DcaMultiAsset")?.is_some() { return refuse("in_flight"); }
    let mut groups = groups_of(&draft.candidate.settings);
    let position = |groups: &[Map<String, Value>], external_id: &str| groups.iter().position(|g| id_of(g) == external_id);
    let named = field(fields, op).unwrap_or_default();
    match op {
        "add_index" | "follow_index" => {
            if op == "add_index" && position(&groups, &named).is_some() { return refuse("not_offered"); }
            // The current index of a lone group is not offered (`offered`): re-following it would reset its count and flattening.
            if op == "follow_index" && super::lone_group_of(&draft.raw_settings).is_some_and(|g| id_of(&g) == named) { return refuse("not_offered"); }
            let tile = match offered_here(c, ctx, draft, &named)? { Ok(tile) => tile, Err(refused) => return Ok(Err(refused)) };
            if op == "add_index" { groups.push(group_for(c, &tile, 0.0)?); }
            else {
                // Follow index on a grouped bot: every ingredient replaced by the one group.
                groups = vec![group_for(c, &tile, 1.0)?];
                draft.candidate.settings.insert("allocations".into(), json!({}));
            }
        }
        "remove_index_id" => {
            let Some(at) = position(&groups, &named) else { return refuse("not_offered") };
            groups.remove(at);
            // Rule A: the last group goes only from a basket already valid without it.
            if groups.is_empty() {
                let weights = crate::engine::composition::allocations(Some(&draft.candidate.settings)).unwrap_or_default();
                if weights.is_empty() || (super::float_sum(weights.iter().map(|(_, w)| *w)) - 1.0).abs() > 0.001 { return refuse("weights_first"); }
            }
        }
        "change_index_id" => {
            let Some(at) = position(&groups, &named) else { return refuse("not_offered") };
            let to = field(fields, "change_index_to").unwrap_or_default();
            if to == named || position(&groups, &to).is_some() { return refuse("not_offered"); }
            let tile = match offered_here(c, ctx, draft, &to)? { Ok(tile) => tile, Err(refused) => return Ok(Err(refused)) };
            let weight = crate::engine::composition::Group::from_settings(Some(&groups[at])).weight;
            groups[at] = group_for(c, &tile, weight)?;
        }
        _ => match ungroup(c, ctx, draft, fields, &engine, groups, &named, now)? { Ok(kept) => groups = kept, Err(refused) => return Ok(Err(refused)) },
    }
    let last_gone = groups.is_empty();
    // A first group only on a clean buyer, as stored and as posted (a combined post can't slip past): refused whole, nothing
    // cleared (Codex 4c r1).
    if first && (crate::engine::composition::sells_or_may(&draft.raw_settings) || crate::engine::composition::sells_or_may(&draft.candidate.settings)) {
        return refuse("sell_off_first");
    }
    if last_gone {
        if draft.candidate.settings.get("direction").and_then(Value::as_str) == Some("selling") { return refuse("buy_first"); }
        // Codex slice 3 r1 #1, #2: the sell side starts anew for what remains (§3.2 step 3), and the save says so.
        if crate::engine::composition::reset_sell_side(&mut draft.candidate.settings, &mut draft.candidate.transient) {
            draft.notice = Some(i18n::text(ctx.locale, "errors.bots.index_switch.selling_reset", &[]));
        }
        draft.candidate.settings.shift_remove("index_groups");
    } else {
        draft.candidate.settings.insert("index_groups".into(), Value::Array(groups.into_iter().map(Value::Object).collect()));
    }
    draft.refresh(c)?;
    // A discarded buy-side flip reads as a rule that was never set: its fields take the defaults every load gives them.
    if last_gone { draft.candidate.fill_defaults(); }
    Ok(Ok(Some(Ran { last_gone })))
}

/// Custom allocation as one operation (Codex slice 3 r1 #5): the stored composition must be its settings' own
/// (`composition::current`), checked once; every requested group (`*`: all) expands from that one snapshot, each member
/// adding `group weight × its weight` to its asset; under market cap every ingredient first takes its composed share and
/// the bot turns manual. Fractions are kept exactly. Refused beside any other composition edit, or past MAX_ASSETS (assets
/// plus surviving groups). Returns the surviving groups.
#[allow(clippy::too_many_arguments)]
fn ungroup(c: &Connection, ctx: &Ctx, draft: &mut draft::Draft, fields: &Map<String, Value>, engine: &model::Bot, groups: Vec<Map<String, Value>>, named: &str,
           now: chrono::DateTime<chrono::Utc>) -> Result<Result<Vec<Map<String, Value>>, String>, WebError> {
    // A lone group's flat count and flattening are its group's fields (slice 4a), so an edit too.
    const EDITS: [&str; 12] = ["allocations", "index_groups", "weighting", "normalize_allocations", "add_asset_id", "remove_asset_id", "exchange_id", "account_id", "quote_asset_id", "limit_ordered",
                               "num_coins", "allocation_flattening"];
    if EDITS.iter().any(|key| fields.contains_key(*key)) { return Ok(Err(t(ctx.locale, "gone"))); }
    if !crate::engine::composition::current(c, engine, now)? { return Ok(Err(t(ctx.locale, "in_flight"))); }
    let expand: Vec<String> = if named == "*" { groups.iter().map(id_of).collect() } else { vec![named.to_string()] };
    if expand.is_empty() || expand.iter().any(|g| !groups.iter().any(|x| id_of(x) == *g)) { return Ok(Err(t(ctx.locale, "not_offered"))); }
    let shares = engine.transient.get(crate::engine::composition::COMPOSED_OF_KEY).and_then(|of| of.get("shares")).cloned().unwrap_or(Value::Null);
    let market_cap = crate::engine::composition::weighting(Some(&draft.candidate.settings)) == "market_cap";
    let mut weights: Map<String, Value> = draft.candidate.settings.get("allocations").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut groups = groups;
    if market_cap {
        for (asset, w) in weights.iter_mut() { *w = shares["assets"].get(asset).cloned().unwrap_or(json!(0.0)); }
        for g in groups.iter_mut() { let share = shares["groups"].get(id_of(g)).cloned().unwrap_or(json!(0.0)); g.insert("weight".into(), share); }
        draft.candidate.settings.insert("weighting".into(), json!("manual"));
    }
    let mut s = c.prepare("SELECT asset_id, weight FROM rust_bot_group_members WHERE bot_id = ?1 AND index_external_id = ?2 ORDER BY asset_id")?;
    for external_id in &expand {
        let weight = groups.iter().find(|g| id_of(g) == *external_id).map_or(0.0, |g| crate::engine::composition::Group::from_settings(Some(g)).weight);
        for row in s.query_map(rusqlite::params![engine.id, external_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))? {
            let (asset, member) = row?;
            let entry = weights.entry(asset.to_string()).or_insert(json!(0.0));
            *entry = json!(entry.as_f64().unwrap_or(0.0) + weight * member);
        }
    }
    drop(s);
    groups.retain(|g| !expand.contains(&id_of(g)));
    if weights.len() + groups.len() > super::MAX_ASSETS {
        return Ok(Err(i18n::text(ctx.locale, "errors.bots.multi_asset.max_assets", &[("max", i18n::Arg::Text(&super::MAX_ASSETS.to_string()))])));
    }
    draft.candidate.settings.insert("allocations".into(), Value::Object(weights));
    Ok(Ok(groups))
}

/// The bot menu's index routes on a basket stored with groups: the operation posted through the one settings writer.
fn grouped_route(c: &Connection, ctx: &Ctx, owner: i64, id: i64, op: &str, value: &str) -> Result<Switched, WebError> { grouped_ops(c, ctx, owner, id, &[(op, value)]) }

/// The ingredient operation `fields` posted through the one settings writer (the bot menu's index routes, and the index
/// sheet's Change the index, Custom allocation and Remove).
pub fn grouped_ops(c: &Connection, ctx: &Ctx, owner: i64, id: i64, fields: &[(&str, &str)]) -> Result<Switched, WebError> {
    if fields.iter().any(|(_, value)| value.trim().is_empty()) { return Ok(Switched::Refused(t(ctx.locale, "not_offered"))); }
    let root: Map<String, Value> = fields.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    let params = super::action_params::ActionParams::from_form(json!({ "bots_dca_multi_asset": root }));
    let built = |_: &Connection, _: &Ctx, d: &draft::Draft| Ok(write::Prepared { response: (d.errors.clone(), d.notice.clone()), broadcasts: vec![], after_scheduling: None });
    Ok(match write::settings(c, ctx, owner, id, &params, built)? {
        write::Outcome::Missing => Switched::Missing,
        write::Outcome::Unported(reason) => Switched::Unported(reason),
        write::Outcome::Invalid((errors, _), _) | write::Outcome::GuardRefused((errors, _)) => {
            let messages: Vec<String> = errors.iter().map(|e| super::mcp_input::full_messages(std::slice::from_ref(e))).collect();
            Switched::Refused(draft::sentence(&messages.iter().map(String::as_str).collect::<Vec<_>>(), ctx.locale))
        }
        write::Outcome::Committed((_, Some(notice))) => { crate::web::flash::set(&ctx.session, crate::web::flash::NOTICE, notice); Switched::Done }
        write::Outcome::NoChange(_) | write::Outcome::Committed(_) => Switched::Done,
    })
}

/// Whether bot `id` of `owner` is a basket stored with index groups.
fn grouped_row(c: &Connection, owner: i64, id: i64) -> Result<bool, WebError> {
    let settings: Option<String> = c.query_row("SELECT settings FROM bots WHERE id=?1 AND user_id=?2 AND status<>3 AND type='Bots::DcaMultiAsset'", (id, owner), |r| r.get(0)).optional()?;
    Ok(settings.is_some_and(|s| serde_json::from_str::<Value>(&s).ok().and_then(|v| v.get("index_groups").map(Value::is_array)).unwrap_or(false)))
}
