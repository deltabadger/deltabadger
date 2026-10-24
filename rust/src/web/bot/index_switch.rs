//! The bot menu's index entries (Bot::IndexSwitch): an index bot keeps its members at their weights as a portfolio
//! ("Custom allocation"). The row changes class in place, inside one write transaction that calls no venue and no
//! market-data service.
use super::{composition, draft, write, Bot, For};
use crate::enums::BotStatus;
use crate::engine::{eligibility, index, model};
use crate::web::{bots, i18n, layout::Ctx, WebError};
use crate::codec;
use crate::engine::accounting::{web_effective_amount as effective_amount, web_minimum as minimum, web_serialized as serialized};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

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
    if ["rebalance_pending", "liquidation_pending", "liquidation_selling_since", "redeploy_pending"].iter().any(|k| pending(k)) || waiting || abandoned
        || write::engine_holds(c, bot)? {
        return Ok(Some("in_flight"));
    }
    Ok(None)
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
    let mut source = draft.original.clone();
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
    // A name the user never changed follows what the bot now holds.
    let generated = source.clone().default_label(ctx.locale);
    let renamed = !source.label_unsaved && source.label != generated;
    // The carry the index computes (set_missed_quote_amount), capped as on any settings edit (Bot::Accountable).
    let carry = write::pending(&tx, &draft.original, now)?;
    let at = codec::format_time(now);
    // becomes!: the portfolio's after_initialize defaults over the same settings, then only its own keys, then the weights.
    // The row is loaded as the portfolio over the index's initialised settings (`becomes!` shares the attributes), so a
    // default the index computed (a Smart Intervals amount off its venue minimums) is kept, not recomputed for the portfolio.
    one(tx.execute("UPDATE bots SET type=?1, settings=?2 WHERE id=?3 AND user_id=?4 AND type=?5 AND status<>3",
        (TO, Value::Object(draft.original.settings.clone()).to_string(), id, owner, FROM))?)?;
    let target = Bot::find(&tx, owner, id, For::Page, ctx.locale)?.ok_or_else(|| super::data("switched bot disappeared".into()))?;
    let mut settings: Map<String, Value> = STORED_DCA_MULTI_ASSET.iter().filter_map(|key| target.settings.get(*key).map(|v| (key.to_string(), v.clone()))).collect();
    settings.insert("allocations".into(), Value::Object(weights));
    source.settings = settings.clone();
    source.kind = super::Kind::Basket;
    let carry = write::stored_carry(&draft.raw_transient, serialized(minimum(carry, effective_amount(&source)?)?)?);
    let label = if renamed { Some(source.label.clone()) } else { None };
    one(tx.execute("UPDATE bots SET settings=?1, label=?2, settings_changed_at=?3, updated_at=?3, \
        transient_data=json_set(transient_data,'$.run_generation',coalesce(json_extract(transient_data,'$.run_generation'),0)+1) WHERE id=?4 AND type=?5",
        (Value::Object(settings).to_string(), &label, &at, id, TO))?)?;
    for (key, value) in [("missed_quote_amount", carry), ("missed_quote_amount_was_set", Value::Null)] {
        if draft.raw_transient.get(key) != Some(&value) {
            one(tx.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,json(?2)) WHERE id=?3", (format!("$.{key}"), value.to_string(), id))?)?;
        }
    }
    let basket = Bot::find(&tx, owner, id, For::Page, ctx.locale)?.ok_or_else(|| super::data("switched bot disappeared".into()))?;
    // Automation::Labelable#set_default_label: the generated name is stored.
    if !renamed { one(tx.execute("UPDATE bots SET label=?1 WHERE id=?2", (&basket.label, id))?)?; }
    // The portfolio's after-save composition: members it left out become exited.
    composition::reconcile(&tx, &basket, now)?;
    if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) {
        tx.rollback()?;
        return Ok(Switched::Refused(i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&refusal.reason()))])));
    }
    tx.commit()?;
    Ok(Switched::Done)
}

fn one(rows: usize) -> Result<(), WebError> {
    if rows == 1 { Ok(()) } else { Err(super::data("index switch lost its bot".into())) }
}
