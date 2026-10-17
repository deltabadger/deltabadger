//! The wizard's save of a new Bots::DcaMultiAsset (Bots::Wizard::PickSpendableAssetsController#finalise_and_redirect):
//! `bot_relation.new(session config)`, `set_missed_quote_amount`, `save`. Every callback Rails runs on that save is listed in
//! `CALLBACKS` with how this port handles it; tests/fixtures/bot_create_callbacks.json pins Rails' list
//! (script/rust/record_bot_create_callbacks.rb), so a callback added or changed there fails `bot_create_callbacks_are_all_handled`
//! until this file follows. The insert, the composition rows and the guard run in one write transaction.
use crate::codec::format_time;
use crate::web::{i18n, WebError};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// (chain, kind, callback, its conditions joined by " & ", how this port handles it). Chains as Rails names them:
/// initialize (after_initialize), validation (before/after_validation), validate (validate/validates), save, create, commit.
pub const CALLBACKS: &[(&str, &str, &str, &str, &str)] = &[
    ("initialize", "after", "initialize_indicator_limitable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_moving_average_limitable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_price_drop_limitable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_price_limitable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_quote_amount_limitable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_limit_orderable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_smart_intervalable_settings", "the callback did not return false", "ported: defaults (build, smart_quote)"),
    ("validation", "after", "_ensure_no_duplicate_errors", "", "ported: Errors::add drops a repeated message"),
    ("validation", "before", "normalize_changed_in_place_attributes", "", "no-op: no attribute declares a normalization"),
    ("validation", "before", "set_default_label", "proc app/models/automation/labelable.rb:9", "ported: label (a new bot has none)"),
    ("validation", "before", "seed_smart_interval_base_amount", "proc app/models/bot/smart_intervalable.rb:14", "no-op: Smart Intervals start off and the bot buys"),
    ("validation", "before", "derive_allocations_from_base_asset_ids", "on create", "ported: build (equal weights, base_asset_ids removed)"),
    ("validation", "before", "pin_lone_weight", "", "ported: build (one member weighs 1.0)"),
    ("validate", "before", "cant_modify_encrypted_attributes_when_frozen", "proc activerecord-8.1.4/lib/active_record/encryption/encryptable_record.rb:13", "no-op: a bot has no encrypted attribute"),
    ("validate", "before", "validate_exchange_not_retired", "", "no-op on create: only a start or a working status is refused"),
    ("validate", "before", "validate_exchange_order_placement_available", "", "no-op on create: only a start or a working status is refused"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(label)", "", "passes: set_default_label always names the bot"),
    ("validate", "before", "validate_associated_records_for_bot_activity_logs", "", "no-op: a new bot has none"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(user) {\"message\":\"required\"}", "proc activerecord-8.1.4/lib/active_record/associations/builder/belongs_to.rb:148", "passes: the signed-in user's bot"),
    ("validate", "before", "validate_associated_records_for_transactions", "", "no-op: a new bot has none"),
    ("validate", "before", "not_archived", "on start", "no-op: start only"),
    ("validate", "before", "trading_key_ready", "on start", "no-op: start only"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(quote_amount)", "", "passes: WizardDefaults gives 100"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(quote_amount) {\"greater_than\":0}", "", "passes: WizardDefaults gives 100"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(weighting) {\"allow_nil\":true,\"in\":[\"manual\",\"market_cap\"]}", "", "passes: the wizard sets no weighting"),
    ("validate", "before", "validate_external_ids", "on update", "no-op: update only"),
    ("validate", "before", "validate_unchangeable_assets", "on update", "no-op: update only"),
    ("validate", "before", "validate_unchangeable_interval", "on update", "no-op: update only"),
    ("validate", "before", "validate_unchangeable_exchange", "on update", "no-op: update only"),
    ("validate", "before", "validate_tickers_available", "on start", "no-op: start only"),
    ("validate", "before", "validate_condition_subjects_in_composition", "", "passes: every condition starts off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(smart_intervaled) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(smart_interval_quote_amount)", "proc app/models/bot/smart_intervalable.rb:27", "no-op: Smart Intervals start off"),
    ("validate", "before", "validate_smart_interval_quote_minimum", "proc app/models/bot/smart_intervalable.rb:29", "no-op: Smart Intervals start off"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(smart_interval_base_amount)", "proc app/models/bot/smart_intervalable.rb:31", "no-op: Smart Intervals start off"),
    ("validate", "before", "validate_smart_interval_base_minimum", "proc app/models/bot/smart_intervalable.rb:33", "no-op: Smart Intervals start off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(limit_ordered) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(limit_order_pcnt_distance) {\"greater_than_or_equal_to\":0,\"less_than_or_equal_to\":1}", "limit_ordered", "no-op: limit orders start off"),
    ("validate", "before", "limit_ordered_required_for_exchange", "", "passes: only Hyperliquid locks limit orders, and its key step is refused"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(quote_amount_limited) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(quote_amount_limit) {\"greater_than_or_equal_to\":\"proc\"}", "quote_amount_limited?", "no-op: the spend cap starts off"),
    ("validate", "before", "validate_quote_amount_limit_not_reached", "on start & proc app/models/bot/quote_amount_limitable.rb:21", "no-op: start only"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(base_amount_limited) {\"in\":[true,false]}", "", "passes: unset"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(base_amount_limit) {\"greater_than\":0}", "proc app/models/bot/base_amount_limitable.rb:22", "no-op: the sell cap starts off"),
    ("validate", "before", "validate_base_amount_limit_not_reached", "on start & proc app/models/bot/base_amount_limitable.rb:25", "no-op: start only"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_limited) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(price_limit) {\"greater_than_or_equal_to\":0}", "price_limited?", "no-op: the condition starts off"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(price_limit_range_lower_bound) {\"greater_than_or_equal_to\":0}", "price_limited?", "no-op: the condition starts off"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(price_limit_range_upper_bound) {\"greater_than_or_equal_to\":0}", "price_limited?", "no-op: the condition starts off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_limit_timing_condition) {\"in\":[\"while\",\"after\"]}", "", "passes: the default while"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_limit_value_condition) {\"in\":[\"above\",\"below\",\"between\"]}", "", "passes: the default below"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_limit_action) {\"in\":[\"pause\",\"start_selling\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_price_limit) {\"greater_than_or_equal_to\":0}", "sell_price_limited?", "no-op: the bot buys"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_price_limit_range_lower_bound) {\"greater_than_or_equal_to\":0}", "sell_price_limited?", "no-op: the bot buys"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_price_limit_range_upper_bound) {\"greater_than_or_equal_to\":0}", "sell_price_limited?", "no-op: the bot buys"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_price_limit_action) {\"in\":[\"pause\",\"start_buying\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_drop_limited) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(price_drop_limit) {\"greater_than_or_equal_to\":0,\"less_than_or_equal_to\":1}", "price_drop_limited?", "no-op: the condition starts off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_drop_limit_time_window_condition) {\"in\":[\"ath\",\"twenty_four_hours\"]}", "", "passes: the default ath"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_price_drop_limit_time_window_condition) {\"in\":[\"twenty_four_hours\",\"seven_days\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(price_drop_limit_action) {\"in\":[\"pause\",\"start_selling\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_price_drop_limit) {\"greater_than_or_equal_to\":0,\"less_than_or_equal_to\":1}", "sell_price_drop_limited?", "no-op: the bot buys"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_price_drop_limit_action) {\"in\":[\"pause\",\"start_buying\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(moving_average_limited) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(moving_average_limit_timing_condition) {\"in\":[\"while\",\"after\"]}", "", "passes: the default while"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(moving_average_limit_value_condition) {\"in\":[\"above\",\"below\"]}", "", "passes: the default below"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(moving_average_limit_in_ma_type) {\"in\":[\"sma\",\"ema\"]}", "", "passes: the default sma"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(moving_average_limit_in_timeframe) {\"in\":[\"one_hour\",\"four_hours\",\"one_day\",\"three_days\",\"one_week\",\"one_month\"]}", "", "passes: the default one_day"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(moving_average_limit_in_period) {\"only_integer\":true,\"greater_than\":0}", "moving_average_limited?", "no-op: the condition starts off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(moving_average_limit_action) {\"in\":[\"pause\",\"start_selling\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_moving_average_limited) {\"in\":[true,false]}", "", "passes: the reader's false"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_moving_average_limit_timing_condition) {\"in\":[\"while\",\"after\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_moving_average_limit_value_condition) {\"in\":[\"above\",\"below\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_moving_average_limit_in_ma_type) {\"in\":[\"sma\",\"ema\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_moving_average_limit_in_timeframe) {\"in\":[\"one_hour\",\"four_hours\",\"one_day\",\"three_days\",\"one_week\",\"one_month\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_moving_average_limit_in_period) {\"only_integer\":true,\"greater_than\":0}", "sell_moving_average_limited?", "no-op: the bot buys"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_moving_average_limit_action) {\"in\":[\"pause\",\"start_buying\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(indicator_limited) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(indicator_limit) {\"message\":\"proc\"}", "indicator_limited?", "no-op: the condition starts off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(indicator_limit_timing_condition) {\"in\":[\"while\",\"after\"]}", "", "passes: the default while"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(indicator_limit_value_condition) {\"in\":[\"above\",\"below\"]}", "", "passes: the default below"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(indicator_limit_in_indicator) {\"in\":[\"rsi\"]}", "", "passes: the default rsi"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(indicator_limit_in_timeframe) {\"in\":[\"one_hour\",\"four_hours\",\"one_day\",\"three_days\",\"one_week\",\"one_month\"]}", "", "passes: the default one_day"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(indicator_limit_action) {\"in\":[\"pause\",\"start_selling\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_indicator_limited) {\"in\":[true,false]}", "", "passes: the reader's false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_indicator_limit) {\"message\":\"proc\"}", "sell_indicator_limited?", "no-op: the bot buys"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_indicator_limit_timing_condition) {\"in\":[\"while\",\"after\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_indicator_limit_value_condition) {\"in\":[\"above\",\"below\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_indicator_limit_in_indicator) {\"in\":[\"rsi\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_indicator_limit_in_timeframe) {\"in\":[\"one_hour\",\"four_hours\",\"one_day\",\"three_days\",\"one_week\",\"one_month\"]}", "", "passes: the reader's default"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_indicator_limit_action) {\"in\":[\"pause\",\"start_buying\"]}", "", "passes: the reader's pause"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(interval)", "", "passes: WizardDefaults gives week"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(interval) {\"in\":[\"hour\",\"day\",\"week\",\"month\"]}", "", "passes: WizardDefaults gives week"),
    ("validate", "before", "validate_starting_time_settings", "on start", "no-op: start only"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(missed_quote_amount) {\"greater_than_or_equal_to\":0}", "", "passes: a new bot owes 0"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(rebalance_threshold) {\"greater_than\":0,\"less_than_or_equal_to\":1}", "", "passes: unset"),
    ("validate", "before", "validate_associated_records_for_bot_index_assets", "", "no-op: a new bot has none in memory"),
    ("validate", "before", "validate_associated_records_for_index_assets", "", "no-op: a new bot has none in memory"),
    ("validate", "before", "validate_allocations", "", "ported: validate"),
    ("validate", "before", "validate_member_count", "on start", "no-op: start only"),
    ("validate", "before", "validate_allocations_balanced", "on start", "no-op: start only"),
    ("validate", "before", "validate_composition_pairs", "proc app/models/bots/dca_multi_asset/allocatable.rb:14", "ported: validate"),
    ("validate", "before", "validate_composition_frozen_while_working", "on update", "no-op: update only"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(direction) {\"allow_nil\":true,\"in\":[\"buying\",\"selling\"]}", "", "passes: unset"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_denomination) {\"allow_nil\":true,\"in\":[\"base\",\"quote\"]}", "", "passes: unset"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_amount) {\"allow_nil\":true,\"greater_than\":0}", "", "passes: unset"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(sell_quote_amount) {\"allow_nil\":true,\"greater_than\":0}", "", "passes: unset"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(sell_interval) {\"in\":[\"hour\",\"day\",\"week\",\"month\"]}", "", "passes: unset"),
    ("validate", "before", "validate_unchangeable_sell_interval", "on update", "no-op: update only"),
    ("save", "after", "refresh_composition", "proc app/models/bots/dca_multi_asset.rb:26 & the callback did not return false", "ported: engine::basket::refresh_composition after the insert"),
    ("save", "after", "proc app/models/automation/labelable.rb:11", "the callback did not return false", "no-op: an in-memory flag"),
    ("save", "before", "update_settings_changed_at", "will_save_change_to_settings?", "ported: settings_changed_at is now"),
    ("save", "before", "autosave_associated_records_for_exchange", "", "no-op: the exchange row exists"),
    ("save", "before", "autosave_associated_records_for_user", "", "no-op: the user row exists"),
    ("save", "before", "store_previous_exchange_id", "", "no-op: an in-memory value"),
    ("save", "before", "set_tickers", "will_save_change_to_exchange_id?", "ported: tickers (the ticker defaults read it)"),
    ("save", "before", "set_quote_amount_limit_enabled_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it, so no time is written (recorded)"),
    ("save", "before", "set_base_amount_limit_enabled_at", "will_save_change_to_settings?", "no-op on create: unset before and after"),
    ("save", "before", "set_price_limit_enabled_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_price_limit_condition_met_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_price_limit_in_ticker_id", "will_save_change_to_exchange_id?", "ported: buy and sell ticker defaults"),
    ("save", "before", "reset_price_limit_info_cache", "will_save_change_to_settings?", "no-op: Rails.cache only"),
    ("save", "before", "set_price_limit_value_condition", "will_save_change_to_settings?", "no-op on create: the timing condition is while"),
    ("save", "before", "set_price_drop_limit_enabled_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_price_drop_limit_condition_met_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_price_drop_limit_in_ticker_id", "will_save_change_to_exchange_id?", "ported: buy and sell ticker defaults"),
    ("save", "before", "reset_price_drop_limit_info_cache", "will_save_change_to_settings?", "no-op: Rails.cache only"),
    ("save", "before", "set_moving_average_limit_enabled_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_moving_average_limit_condition_met_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_moving_average_limit_in_ticker_id", "will_save_change_to_exchange_id?", "ported: buy and sell ticker defaults"),
    ("save", "before", "reset_moving_average_limit_info_cache", "will_save_change_to_settings?", "no-op: Rails.cache only"),
    ("save", "before", "set_indicator_limit_enabled_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_indicator_limit_condition_met_at", "will_save_change_to_settings?", "no-op on create: the switch is as after_initialize left it (recorded)"),
    ("save", "before", "set_indicator_limit_in_ticker_id", "will_save_change_to_exchange_id?", "ported: buy and sell ticker defaults"),
    ("save", "before", "reset_indicator_limit_info_cache", "will_save_change_to_settings?", "no-op: Rails.cache only"),
    ("save", "before", "check_missed_quote_amount_was_set", "", "ported: transient_data (owes 0, the capture flag cleared)"),
    ("save", "around", "around_save_collection_association", "", "no-op: no association in memory"),
    ("create", "after", "autosave_associated_records_for_index_assets", "the callback did not return false", "no-op: none in memory"),
    ("create", "after", "autosave_associated_records_for_bot_index_assets", "the callback did not return false", "no-op: none in memory (refresh_composition writes them)"),
    ("create", "after", "autosave_associated_records_for_transactions", "the callback did not return false", "no-op: none in memory"),
    ("create", "after", "autosave_associated_records_for_bot_activity_logs", "the callback did not return false", "no-op: none in memory"),
    ("create", "before", "assign_position", "", "ported: position is the user's last + 1"),
    ("commit", "after", "broadcast_metrics_panel", "on update & on update & proc app/models/bots/dca_multi_asset.rb:28", "no-op: update only"),
    ("commit", "after", "proc app/models/bot.rb:41", "on update & on update", "no-op: update only"),
    ("commit", "after", "broadcast_columns_lock_update", "on update & on update & saved_change_to_status?", "no-op: update only"),
    ("commit", "after", "broadcast_status_button_update", "on update & on update & saved_change_to_status?", "no-op: update only"),
    ("commit", "after", "broadcast_status_bar_update", "on update & on update & proc app/models/bot.rb:38", "no-op: update only"),
];

fn to_i(value: &Value) -> i64 { super::to_i(value) }

/// Ruby truthiness, for `||=`.
fn unset(value: Option<&Value>) -> bool { matches!(value, None | Some(Value::Null | Value::Bool(false))) }

/// `present?` on a decoded value.
fn present(value: Option<&Value>) -> bool {
    match value { None | Some(Value::Null | Value::Bool(false)) => false, Some(Value::String(text)) => !crate::ruby::blank(text), _ => true }
}

/// `x ||= value`, in place when the key is there (nil or false), else appended.
fn default(settings: &mut Map<String, Value>, key: &str, value: Value) {
    if unset(settings.get(key)) { settings.insert(key.into(), value); }
}

/// A ticker as the defaults read it.
struct Ticker { id: i64, base: String, quote_decimals: Option<i64> }

/// `set_tickers`: the venue's available, trading-enabled tickers at the quote, of these assets, in table order.
fn tickers(c: &Connection, exchange: Option<i64>, quote: Option<i64>, ids: &[i64]) -> Result<Vec<Ticker>, WebError> {
    let Some(exchange) = exchange else { return Ok(vec![]) };
    let mut statement = c.prepare("SELECT id, base, quote_decimals FROM tickers WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND quote_asset_id IS ?2 \
                                   AND base_asset_id IN (SELECT value FROM json_each(?3)) ORDER BY id")?;
    let rows = statement.query_map(rusqlite::params![exchange, quote, json!(ids).to_string()], |r| Ok(Ticker { id: r.get(0)?, base: r.get::<_, Option<String>>(1)?.unwrap_or_default(), quote_decimals: r.get(2)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `tickers.min_by { |t| t[:base] }&.id`.
fn first_ticker(tickers: &[Ticker]) -> Value {
    tickers.iter().min_by(|a, b| a.base.as_bytes().cmp(b.base.as_bytes())).map_or(Value::Null, |ticker| json!(ticker.id))
}

/// initialize_smart_intervalable_settings' amount: `[quote_amount / 10, minimum_smart_interval_quote_amount * 10].max
/// .round(least_precise_quote_decimals).to_f`, nil without an amount or a ticker. A new basket has no composition rows, so
/// the venue's own minimum is 0 and the floor is the frequency's (one order every five minutes) or the precision.
fn smart_quote(settings: &Map<String, Value>, tickers: &[Ticker]) -> Value {
    let Some(decimals) = tickers.iter().filter_map(|t| t.quote_decimals).min() else { return Value::Null };
    let amount = settings.get("quote_amount").filter(|v| present(Some(v)));
    let Some(amount) = amount.and_then(|v| v.as_f64().map(|f| (f, v.is_i64()))) else { return Value::Null };
    let seconds = match settings.get("interval").and_then(Value::as_str) { Some("hour") => 3_600.0, Some("day") => 86_400.0, Some("week") => 604_800.0, Some("month") => 2_629_746.0, _ => 0.0 };
    let frequency = if seconds > 0.0 { amount.0 / seconds * 300.0 } else { 0.0 };
    let scale = 10f64.powi(decimals as i32);
    let minimum = crate::engine::accounting::round_up(frequency, scale).max(1.0 / scale);
    let tenth = if amount.1 { (amount.0 as i64).div_euclid(10) as f64 } else { amount.0 / 10.0 };
    let floor = minimum * 10.0;
    let value = if floor > tenth { if decimals == 0 { floor.round() } else { crate::ruby::float_round(floor, decimals as i32) } } else { tenth };
    json!(value)
}

/// Bots::DcaMultiAsset::Allocatable#normalize_allocations over equal weights: thousandths by largest remainder.
fn equal_allocations(ids: &[i64]) -> Map<String, Value> {
    let mut unique: Vec<i64> = vec![];
    for id in ids { if !unique.contains(id) { unique.push(*id); } }
    let mut weights: Map<String, Value> = unique.iter().map(|id| (id.to_string(), json!(1.0))).collect();
    if crate::web::bot::draft::normalize(&mut weights).is_err() { return Map::new(); }
    weights
}

/// The bot `bot_relation.new(config)` builds and `save` writes, after its callbacks.
pub struct Built { pub exchange_id: Option<i64>, pub quote: Option<i64>, pub settings: Map<String, Value>, pub label: String, members: Vec<i64> }

/// after_initialize, set_missed_quote_amount and the before_validation callbacks, in Rails' order.
pub fn build(c: &Connection, config: &Value, locale: &str) -> Result<Built, WebError> {
    let exchange_id = super::exchange_id(Some(config));
    let mut settings = config.get("settings").and_then(Value::as_object).cloned().unwrap_or_default();
    let quote = settings.get("quote_asset_id").filter(|v| present(Some(v))).map(to_i);
    let listed: Vec<i64> = match settings.get("base_asset_ids") { None | Some(Value::Null) => vec![], Some(Value::Array(ids)) => ids.iter().map(to_i).collect(), Some(one) => vec![to_i(one)] };
    let initial = tickers(c, exchange_id, quote, &listed)?;
    // after_initialize, each concern in turn.
    default(&mut settings, "smart_intervaled", json!(false));
    let smart = smart_quote(&settings, &initial);
    default(&mut settings, "smart_interval_quote_amount", smart);
    default(&mut settings, "limit_ordered", json!(false));
    default(&mut settings, "limit_order_pcnt_distance", json!(0.001));
    default(&mut settings, "quote_amount_limited", json!(false));
    default(&mut settings, "quote_amount_limit", json!(1000));
    let first = first_ticker(&initial);
    for (key, value) in [("price_limited", json!(false)), ("price_limit", json!(1_000_000)), ("price_limit_range_lower_bound", json!(0)),
                         ("price_limit_range_upper_bound", json!(1_000_000)), ("price_limit_timing_condition", json!("while")),
                         ("price_limit_value_condition", json!("below")), ("price_limit_in_ticker_id", first.clone()),
                         ("price_drop_limited", json!(false)), ("price_drop_limit", json!(0.2)), ("price_drop_limit_time_window_condition", json!("ath")),
                         ("price_drop_limit_in_ticker_id", first.clone()),
                         ("moving_average_limited", json!(false)), ("moving_average_limit_timing_condition", json!("while")),
                         ("moving_average_limit_value_condition", json!("below")), ("moving_average_limit_in_ticker_id", first.clone()),
                         ("moving_average_limit_in_ma_type", json!("sma")), ("moving_average_limit_in_timeframe", json!("one_day")), ("moving_average_limit_in_period", json!(9)),
                         ("indicator_limited", json!(false)), ("indicator_limit", json!(30)), ("indicator_limit_timing_condition", json!("while")),
                         ("indicator_limit_value_condition", json!("below")), ("indicator_limit_in_ticker_id", first.clone()),
                         ("indicator_limit_in_indicator", json!("rsi")), ("indicator_limit_in_timeframe", json!("one_day"))] {
        default(&mut settings, key, value);
    }
    // set_default_label (Automation::Labelable): one asset by its name, a basket by its first three symbols.
    let label = label(c, &listed, locale)?;
    // derive_allocations_from_base_asset_ids, then pin_lone_weight.
    let nonzero: Vec<i64> = listed.iter().copied().filter(|id| *id != 0).collect();
    let empty = settings.get("allocations").and_then(Value::as_object).is_none_or(Map::is_empty);
    if empty && !nonzero.is_empty() { settings.insert("allocations".into(), Value::Object(equal_allocations(&nonzero))); }
    settings.shift_remove("base_asset_ids");
    if let Some(Value::Object(weights)) = settings.get_mut("allocations") {
        if weights.len() == 1 { for weight in weights.values_mut() { *weight = json!(1.0); } }
    }
    let members = settings.get("allocations").and_then(Value::as_object).map(|weights| weights.keys().map(|key| crate::web::bot::search::to_i(key)).collect()).unwrap_or_default();
    Ok(Built { exchange_id, quote, settings, label, members })
}

fn label(c: &Connection, ids: &[i64], locale: &str) -> Result<String, WebError> {
    let mut find = c.prepare("SELECT name, symbol FROM assets WHERE id = ?1")?;
    let mut rows = vec![];
    for id in ids { rows.push(find.query_row([id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))).optional()?); }
    let named = if ids.len() == 1 { rows[0].clone().and_then(|(name, _)| name).unwrap_or_default() } else {
        let symbols: Vec<String> = rows.into_iter().filter_map(|row| row.and_then(|(_, symbol)| symbol)).collect();
        if symbols.len() > 3 { format!("{} + {}", symbols[..3].join(", "), symbols.len() - 3) } else { symbols.join(", ") }
    };
    Ok(if crate::ruby::blank(&named) { i18n::text(locale, "bot.new", &[]) } else { named })
}

/// ActiveModel's message for an error code on `attribute`, as `errors.add(attribute, code)` looks it up.
fn message(locale: &str, attribute: &str, code: &str) -> String {
    let keys = [format!("activerecord.errors.models.bots/dca_multi_asset.attributes.{attribute}.{code}"), format!("activerecord.errors.models.bots/dca_multi_asset.{code}"),
                format!("activerecord.errors.models.bot.attributes.{attribute}.{code}"), format!("activerecord.errors.models.bot.{code}"),
                format!("activerecord.errors.messages.{code}"), format!("errors.attributes.{attribute}.{code}"), format!("errors.messages.{code}")];
    i18n::text_chain(locale, &keys, &[]).unwrap_or_else(|| format!("translation missing: {locale}.errors.messages.{code}"))
}

/// The create-context validations a wizard-built bot can fail (CALLBACKS names why the others pass): validate_allocations, then
/// validate_composition_pairs. `errors.messages.values.flatten`: by attribute in the order each first failed, repeats dropped.
pub fn validate(c: &Connection, bot: &Built, locale: &str) -> Result<Vec<String>, WebError> {
    let mut errors: Vec<(&str, String)> = vec![];
    let mut add = |attribute: &'static str, text: String| if !errors.iter().any(|(a, t)| *a == attribute && *t == text) { errors.push((attribute, text)); };
    let weights = bot.settings.get("allocations").and_then(Value::as_object).cloned().unwrap_or_default();
    if weights.is_empty() { add("allocations", i18n::text(locale, "errors.bots.multi_asset.min_assets", &[])); }
    if !weights.values().all(|w| w.as_f64().is_some_and(|w| w.is_finite() && (0.0..=1.0).contains(&w))) { add("allocations", message(locale, "allocations", "invalid")); }
    if let Some(quote) = bot.quote { if weights.contains_key(&quote.to_string()) { add("allocations", message(locale, "allocations", "invalid")); } }
    let ids: Vec<String> = weights.keys().cloned().collect();
    let found: i64 = c.query_row("SELECT COUNT(*) FROM assets WHERE id IN (SELECT CAST(value AS INTEGER) FROM json_each(?1))", [json!(ids).to_string()], |r| r.get(0))?;
    if found != ids.len() as i64 { add("allocations", message(locale, "allocations", "invalid")); }
    // `if: exchange_id? && new_record?`.
    if let Some(exchange) = bot.exchange_id.filter(|id| *id != 0) {
        let name: Option<Option<String>> = c.query_row("SELECT name FROM exchanges WHERE id = ?1", [exchange], |r| r.get(0)).optional()?;
        match name {
            None => add("exchange", message(locale, "exchange", "invalid")),
            Some(name) => {
                let mut missing = vec![];
                for id in &bot.members {
                    let listed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM tickers WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND base_asset_id = ?2 AND quote_asset_id IS ?3)",
                                                   rusqlite::params![exchange, id, bot.quote], |r| r.get(0))?;
                    if !listed { missing.push(*id); }
                }
                if !missing.is_empty() {
                    let mut statement = c.prepare("SELECT symbol FROM assets WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
                    let symbols = statement.query_map([json!(missing).to_string()], |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?;
                    let symbols: Vec<String> = symbols.into_iter().map(Option::unwrap_or_default).collect();
                    let assets = crate::web::bot::draft::sentence(&symbols.iter().map(String::as_str).collect::<Vec<_>>(), locale);
                    add("allocations", i18n::text(locale, "errors.bots.multi_asset.pair_missing", &[("assets", i18n::Arg::Text(&assets)), ("exchange_name", i18n::Arg::Text(&name.unwrap_or_default()))]));
                }
            }
        }
    }
    let mut ordered: Vec<String> = vec![];
    for attribute in ["allocations", "exchange"] {
        ordered.extend(errors.iter().filter(|(a, _)| *a == attribute).map(|(_, t)| t.clone()));
    }
    Ok(ordered)
}

/// The before_save and before_create callbacks, the insert, and refresh_composition (after_save), on `c` inside the caller's
/// write transaction. Returns the new bot's id.
pub fn insert(c: &Connection, user: i64, mut bot: Built, now: DateTime<Utc>) -> Result<i64, WebError> {
    // set_tickers, then each condition's set_*_in_ticker_id: buy and sell on the default member.
    let first = first_ticker(&tickers(c, bot.exchange_id, bot.quote, &bot.members)?);
    for rule in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
        bot.settings.insert(format!("{rule}_in_ticker_id"), first.clone());
        bot.settings.insert(format!("sell_{rule}_in_ticker_id"), first.clone());
    }
    let at = format_time(now);
    // assign_position: after the user's last bot.
    let position: i64 = c.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM bots WHERE user_id = ?1", [user], |r| r.get(0))?;
    // set_missed_quote_amount on a bot never started owes 0; check_missed_quote_amount_was_set clears the capture flag.
    let transient = json!({"missed_quote_amount": 0, "missed_quote_amount_was_set": null});
    c.execute("INSERT INTO bots (user_id, exchange_id, type, status, label, position, settings, transient_data, settings_changed_at, created_at, updated_at) \
               VALUES (?1, ?2, 'Bots::DcaMultiAsset', 0, ?3, ?4, ?5, ?6, ?7, ?7, ?7)",
              rusqlite::params![user, bot.exchange_id, bot.label, position, Value::Object(bot.settings).to_string(), transient.to_string(), at])?;
    let id = c.last_insert_rowid();
    let loaded = crate::engine::model::load_bot(c, id)?;
    // A derivation that finds nothing tradeable is Rails' Failure, which the after_save callback does not act on.
    let _ = crate::engine::basket::refresh_composition(c, &loaded, now)?;
    Ok(id)
}
