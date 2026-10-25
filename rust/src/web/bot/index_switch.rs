//! The bot menu's index entries (Bot::IndexSwitch): a portfolio follows an index ("Follow index"), an index bot changes its
//! index ("Change the index") or keeps its members at their weights as a portfolio ("Custom allocation"). The row changes class in place, inside one write transaction that calls no venue and no
//! market-data service.
use super::{composition, draft, write, Bot, For};
use crate::enums::BotStatus;
use crate::engine::{eligibility, index, model};
use crate::web::{bots, i18n, layout::Ctx, wizard, WebError};
use crate::codec;
use crate::engine::accounting::{web_effective_amount as effective_amount, web_minimum as minimum, web_serialized as serialized};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// `Bots::DcaMultiAsset.stored_attributes[:settings]`, in Rails' order (pinned by tests/pages.rs against Rails).
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

/// Bot::IndexSwitch.refusal, plus the engine's own fence (write::engine_holds). Rails' `poll_in_flight?` has no analogue: a
/// stopped bot has no condition check in this engine, and the switch bumps the run generation.
fn refusal(c: &Connection, bot: &model::Bot) -> Result<Option<&'static str>, WebError> {
    if !matches!(bot.status, BotStatus::Stopped | BotStatus::Created) { return Ok(Some("stop_first")); }
    if in_flight(c, bot)? || write::engine_holds(c, bot)? { return Ok(Some("in_flight")); }
    Ok(None)
}

/// `rebalance_pending? || liquidation_in_flight? || redeploy_in_flight?` (Bot::Merge.mergeable? and Bot::IndexSwitch.refusal).
pub(crate) fn in_flight(c: &Connection, bot: &model::Bot) -> Result<bool, WebError> {
    let pending = |key: &str| bot.transient.get(key).is_some_and(|v| match v { Value::Null => false, Value::Object(o) => !o.is_empty(), Value::String(s) => !s.trim().is_empty(), _ => true });
    // rebalance_pending?, liquidation_in_flight? (pending, a waiting leg, an abandoned leg not attested), redeploy_in_flight?.
    let resolved: Vec<i64> = match bot.transient.get("liquidation_resolved_orders") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())).collect(),
        _ => vec![],
    };
    let waiting: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND transaction_type IN ('LIQUIDATION','REDEPLOY') AND status=0 AND external_status IN (0,1))",
        [bot.id], |r| r.get(0))?;
    let mut s = c.prepare("SELECT id FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND external_status=4")?;
    let abandoned = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().any(|id| !resolved.contains(&id));
    Ok(["rebalance_pending", "liquidation_pending", "liquidation_selling_since", "redeploy_pending"].iter().any(|k| pending(k)) || waiting || abandoned)
}

/// POST /bots/:id/custom_allocation (Bots::CustomAllocationsController#create, Bot::IndexSwitch.customize!): the index bot becomes
/// a portfolio of its in-index members that still trade at its venue and quote, at their weights rounded to the slider's 0.1%.
///
/// D-IDX-CUSTOM-STORED: Rails re-derives the index in the request first (market data and venue probes); this build reads the
/// stored members, which the engine's resync keeps, and refuses `in_flight` while a resync it was asked for is still due.
pub fn customize(c: &Connection, ctx: &Ctx, owner: i64, id: i64) -> Result<Switched, WebError> {
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
    if let Some(key) = refusal(&tx, &engine_bot)? { return Ok(Switched::Refused(t(ctx.locale, key))); }
    let wash: Option<bool> = tx.query_row("SELECT wash_sale_enabled FROM users WHERE id=?1", [owner], |r| r.get(0))?;
    let (provider, _) = bots::market_data(&tx, &ctx.app)?;
    if let Some(reason) = super::refusal(&tx, id, wash, provider, For::Settings)? { return Ok(Switched::Unported(reason)); }
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
    let mut settings: Map<String, Value> = stored.iter().filter_map(|key| target.settings.get(*key).map(|v| (key.to_string(), v.clone()))).collect();
    assign(tx, &mut settings)?;
    source.settings = settings.clone();
    source.kind = kind;
    let carry = write::stored_carry(&draft.raw_transient, serialized(minimum(carry, effective_amount(&source)?)?)?);
    let label = if renamed { Some(source.label.clone()) } else { None };
    // The settings column as ActiveRecord's JSON coder writes it (an index's name may hold <, > or &).
    one(tx.execute("UPDATE bots SET settings=?1, label=?2, settings_changed_at=?3, updated_at=?3, \
        transient_data=json_set(transient_data,'$.run_generation',coalesce(json_extract(transient_data,'$.run_generation'),0)+1) WHERE id=?4 AND type=?5",
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
pub(crate) fn offered(c: &Connection, ctx: &Ctx, id: i64) -> Result<Vec<wizard::index::IndexTile>, WebError> {
    let (class, settings, exchange_id, venue): (Option<String>, String, i64, Option<String>) = c.query_row(
        "SELECT b.type, b.settings, e.id, e.type FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE b.id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    let settings: Value = serde_json::from_str(&settings).map_err(|e| super::data(format!("bot {id} settings: {e}")))?;
    // Bots::DcaIndex#current_index: the category's row (any source), else the internal Top Coins.
    let current: Option<i64> = if class.as_deref() != Some("Bots::DcaIndex") { None } else {
        match (settings.get("index_type").and_then(Value::as_str), settings.get("index_category_id").and_then(Value::as_str).filter(|id| !crate::ruby::blank(id))) {
            (Some("category"), Some(category)) => c.query_row("SELECT id FROM indices WHERE external_id=?1 ORDER BY id LIMIT 1", [category], |r| r.get(0)).optional()?,
            _ => c.query_row("SELECT id FROM indices WHERE external_id='top-coins' AND source='internal' ORDER BY id LIMIT 1", [], |r| r.get(0)).optional()?,
        }
    };
    let quote = settings.get("quote_asset_id").and_then(Value::as_i64);
    wizard::index::tiles(c, &ctx.app, Some(&wizard::index::Followable { exchange_id, class: venue.as_deref().unwrap_or_default(), quote, current }))
}

/// POST /bots/:id/index (Bots::IndicesController#create, Bot::IndexSwitch.follow!): a portfolio starts following the index it
/// picked ("Follow index"), or an index bot changes its index ("Change the index"), with a new bot's size for it: a bounded
/// index held whole, any other ten members. `category` is the tile's `index_category_id`.
///
/// D-IDX-FOLLOW-LATE: Rails derives the new members in the request (market data and venue probes for newcomers); this build
/// leaves the engine a resync marker, and the engine derives them once woken (engine::run::resync_bot).
/// D-IDX-COINGECKO: where this engine cannot derive (or run) the index bot, Rails switches and this build refuses, writing nothing.
pub fn follow(c: &Connection, ctx: &Ctx, owner: i64, id: i64, category: Option<&str>) -> Result<Switched, WebError> {
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
    let refused = match refusal(&tx, &engine_bot)? { None if from == "Bots::DcaMultiAsset" && engine_bot.selling() => Some("selling"), other => other };
    if let Some(key) = refused { return Ok(Switched::Refused(t(ctx.locale, key))); }
    let wash: Option<bool> = tx.query_row("SELECT wash_sale_enabled FROM users WHERE id=?1", [owner], |r| r.get(0))?;
    if let Some(reason) = super::refusal(&tx, id, wash, provider, For::Settings)? { return Ok(Switched::Unported(reason)); }
    let Some(draft) = draft::Draft::load(&tx, owner, id, ctx.locale)? else { return Ok(Switched::Missing) };
    let source = draft.original.clone();
    if let Some(reason) = source.unrendered() { return Ok(Switched::Unported(reason)); }
    if let Some(reason) = wizard::index::refusal(&tx, &ctx.app, None, &source.exchange.class)? { return Ok(Switched::Refused(write_refused(ctx.locale, reason))); }
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

/// A store accessor's write: ActiveRecord::Store assigns only a value that differs, by Ruby's ==, from what the store reads
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
struct Picker<'a> { v: &'a Ctx, title: String, none: String, action: String, csrf: &'a str, tiles: Vec<wizard::index::IndexTile> }

/// bots/indices/new's content: "Change the index" or "Follow index" over the offered indices' tiles, which post to the switch,
/// or the sentence saying there are none at this venue and quote.
pub fn picker(c: &Connection, ctx: &Ctx, bot: &Bot, csrf: &str) -> Result<String, WebError> {
    use askama::Template;
    let title = ctx.t(if bot.kind == super::Kind::Index { "bot.index_switch.change_index" } else { "bot.index_switch.follow_index" });
    let quote = bot.quote_asset.as_ref().and_then(|asset| asset.symbol.clone()).unwrap_or_default();
    let none = i18n::text(ctx.locale, "bot.index_switch.none", &[("exchange", i18n::Arg::Text(&bot.exchange.name)), ("quote", i18n::Arg::Text(&quote))]);
    Ok(Picker { v: ctx, title, none, action: ctx.path(&format!("/bots/{}/index", bot.id)), csrf, tiles: offered(c, ctx, bot.id)? }.render()?)
}

/// Bots::Botable#set_bot and Bots::IndicesController#require_composition_bot: None for a bot that is not this user's or is
/// deleted; false for one that is neither a portfolio nor an index bot, or on an install without market data (`head :not_found`).
pub fn composition_bot(c: &Connection, ctx: &Ctx, owner: i64, id: i64) -> Result<Option<bool>, WebError> {
    let class: Option<Option<String>> = c.query_row("SELECT type FROM bots WHERE id=?1 AND user_id=?2 AND status<>3", (id, owner), |r| r.get(0)).optional()?;
    let Some(class) = class else { return Ok(None) };
    Ok(Some(matches!(class.as_deref(), Some("Bots::DcaIndex" | "Bots::DcaMultiAsset")) && bots::market_data(c, &ctx.app)?.1))
}
