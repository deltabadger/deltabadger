//! The index wizard's save of a new Bots::DcaIndex (Bots::DcaIndexes::PickSpendableAssetsController, by way of
//! Bots::Wizard::PickSpendableAssetsController#finalise_and_redirect): `bot_relation.new(session config)`,
//! `set_missed_quote_amount`, `save`. Every callback Rails runs on that save is listed in `CALLBACKS` with how this port
//! handles it; tests/fixtures/index_create_callbacks.json pins Rails' list (script/rust/record_bot_create_callbacks.rb), so a
//! callback added or changed there fails `index_create_callbacks_are_all_handled` until this file follows. Rails derives no
//! composition on create (no bot_index_assets row): the first buy does.
use crate::codec::format_time;
use crate::web::{i18n, WebError};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// (chain, kind, callback, its conditions joined by " & ", how this port handles it), as in wizard::create::CALLBACKS.
pub const CALLBACKS: &[(&str, &str, &str, &str, &str)] = &[
    ("initialize", "after", "initialize_index_allocatable_settings", "the callback did not return false", "ported: defaults (build: num_coins, allocation_flattening)"),
    ("initialize", "after", "initialize_limit_orderable_settings", "the callback did not return false", "ported: defaults (build)"),
    ("initialize", "after", "initialize_smart_intervalable_settings", "the callback did not return false", "ported: defaults (build, smart_quote with the venue minimum)"),
    ("validation", "after", "_ensure_no_duplicate_errors", "", "ported: one message per failure"),
    ("validation", "before", "normalize_changed_in_place_attributes", "", "no-op: no attribute declares a normalization"),
    ("validation", "before", "set_default_label", "proc app/models/automation/labelable.rb:9", "ported: label (default_label; a new bot has none)"),
    ("validation", "before", "clamp_num_coins_to_bounded_index", "", "ported: build (clamp)"),
    ("validation", "before", "seed_smart_interval_base_amount", "proc app/models/bot/smart_intervalable.rb:14", "no-op: Smart Intervals start off and the bot buys"),
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
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(num_coins)", "", "passes: after_initialize gives the universe or 10"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(num_coins) {\"greater_than_or_equal_to\":2,\"less_than_or_equal_to\":\"validation_max_coins\"}", "", "ported: validate"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(allocation_flattening)", "", "passes: WizardDefaults gives 0.0"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(allocation_flattening) {\"greater_than_or_equal_to\":0,\"less_than_or_equal_to\":1}", "", "passes: WizardDefaults gives 0.0"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(index_type)", "", "passes: the index step stores top or category"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(index_type) {\"in\":[\"top\",\"category\"]}", "", "passes: the index step stores top or category"),
    ("validate", "before", "validate_bot_exchange", "on start,update & exchange_id?", "no-op: start and update only"),
    ("validate", "before", "validate_external_ids", "on update", "no-op: update only"),
    ("validate", "before", "validate_unchangeable_assets", "on update", "no-op: update only"),
    ("validate", "before", "validate_unchangeable_interval", "on update", "no-op: update only"),
    ("validate", "before", "validate_unchangeable_exchange", "on update", "no-op: update only"),
    ("validate", "before", "validate_market_data_configured", "on start", "no-op: start only"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(smart_intervaled) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(smart_interval_quote_amount)", "proc app/models/bot/smart_intervalable.rb:27", "no-op: Smart Intervals start off"),
    ("validate", "before", "validate_smart_interval_quote_minimum", "proc app/models/bot/smart_intervalable.rb:29", "no-op: Smart Intervals start off"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(smart_interval_base_amount)", "proc app/models/bot/smart_intervalable.rb:31", "no-op: Smart Intervals start off"),
    ("validate", "before", "validate_smart_interval_base_minimum", "proc app/models/bot/smart_intervalable.rb:33", "no-op: Smart Intervals start off"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(limit_ordered) {\"in\":[true,false]}", "", "passes: the default false"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(limit_order_pcnt_distance) {\"greater_than_or_equal_to\":0,\"less_than_or_equal_to\":1}", "limit_ordered", "no-op: limit orders start off"),
    ("validate", "before", "limit_ordered_required_for_exchange", "", "passes: only Hyperliquid locks limit orders, and its quote step sets them on (create::lock_limit_orders)"),
    ("validate", "before", "ActiveRecord::Validations::PresenceValidator(interval)", "", "passes: WizardDefaults gives week"),
    ("validate", "before", "ActiveModel::Validations::InclusionValidator(interval) {\"in\":[\"hour\",\"day\",\"week\",\"month\"]}", "", "passes: WizardDefaults gives week"),
    ("validate", "before", "validate_starting_time_settings", "on start", "no-op: start only"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(missed_quote_amount) {\"greater_than_or_equal_to\":0}", "", "passes: a new bot owes 0"),
    ("validate", "before", "ActiveRecord::Validations::NumericalityValidator(rebalance_threshold) {\"greater_than\":0,\"less_than_or_equal_to\":1}", "", "passes: unset"),
    ("validate", "before", "validate_associated_records_for_bot_index_assets", "", "no-op: a new bot has none in memory"),
    ("validate", "before", "validate_associated_records_for_index_assets", "", "no-op: a new bot has none in memory"),
    ("save", "after", "proc app/models/automation/labelable.rb:11", "the callback did not return false", "no-op: an in-memory flag"),
    ("save", "before", "update_settings_changed_at", "will_save_change_to_settings?", "ported: settings_changed_at is now"),
    ("save", "before", "autosave_associated_records_for_exchange", "", "no-op: the exchange row exists"),
    ("save", "before", "autosave_associated_records_for_user", "", "no-op: the user row exists"),
    ("save", "before", "store_previous_exchange_id", "", "no-op: an in-memory value"),
    ("save", "before", "set_tickers", "will_save_change_to_exchange_id?", "no-op: memoises the relation; the index bot writes no ticker setting"),
    ("save", "before", "check_missed_quote_amount_was_set", "", "ported: transient_data (owes 0, the capture flag cleared)"),
    ("save", "around", "around_save_collection_association", "", "no-op: no association in memory"),
    ("create", "after", "autosave_associated_records_for_index_assets", "the callback did not return false", "no-op: none in memory (the composition is derived at the first buy)"),
    ("create", "after", "autosave_associated_records_for_bot_index_assets", "the callback did not return false", "no-op: none in memory (the composition is derived at the first buy)"),
    ("create", "after", "autosave_associated_records_for_transactions", "the callback did not return false", "no-op: none in memory"),
    ("create", "after", "autosave_associated_records_for_bot_activity_logs", "the callback did not return false", "no-op: none in memory"),
    ("create", "before", "assign_position", "", "ported: position is the user's last + 1"),
    ("commit", "after", "resync_index_composition", "on update & on update & proc app/models/bots/dca_index.rb:54", "no-op: update only"),
    ("commit", "after", "proc app/models/bot.rb:41", "on update & on update", "no-op: update only"),
    ("commit", "after", "broadcast_columns_lock_update", "on update & on update & saved_change_to_status?", "no-op: update only"),
    ("commit", "after", "broadcast_status_button_update", "on update & on update & saved_change_to_status?", "no-op: update only"),
    ("commit", "after", "broadcast_status_bar_update", "on update & on update & proc app/models/bot.rb:38", "no-op: update only"),
];

/// Bots::DcaIndex::COUNT_NAMED_INDICES: (index_category_id, prefix, name).
pub const COUNT_NAMED: &[(&str, &str, &str)] = &[("nasdaq-100", "ND", "ND100")];

pub fn count_named(category: Option<&str>) -> Option<(&'static str, &'static str)> {
    COUNT_NAMED.iter().find(|(id, ..)| Some(*id) == category).map(|(_, prefix, name)| (*prefix, *name))
}

/// Bots::DcaIndex::MIN_COINS and MAX_COINS.
const MIN_COINS: i64 = 2;
const MAX_COINS: i64 = 100;

fn text<'a>(settings: &'a Map<String, Value>, key: &str) -> Option<&'a str> { settings.get(key).and_then(Value::as_str) }

fn present(value: Option<&Value>) -> bool {
    match value { None | Some(Value::Null | Value::Bool(false)) => false, Some(Value::String(text)) => !crate::ruby::blank(text), _ => true }
}

/// Bots::DcaIndex#current_index: (source, top_coins, top_coins_by_exchange) of the category's row (any source), else the
/// internal Top Coins row.
pub struct IndexRow { pub source: Option<String>, pub top_coins: Vec<String>, pub by_exchange: Map<String, Value>, pub available_exchanges: Map<String, Value> }

impl IndexRow {
    /// Index#top_coins_for_exchange.
    pub fn top_coins_for_exchange(&self, exchange_type: &str) -> Vec<String> {
        match self.by_exchange.get(exchange_type) {
            Some(Value::Array(ids)) => ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect(),
            Some(Value::Null) | None => self.top_coins.clone(),
            // A value that is not a list is what Rails would `||` past only when nil.
            Some(_) => vec![],
        }
    }
}

fn json_column(text: Option<String>) -> Value { text.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null) }

pub fn index_row(c: &Connection, settings: &Map<String, Value>) -> Result<Option<IndexRow>, WebError> {
    let category = text(settings, "index_category_id").filter(|id| !crate::ruby::blank(id));
    let read = |r: &rusqlite::Row<'_>| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?));
    let columns = "SELECT source, top_coins, top_coins_by_exchange, available_exchanges FROM indices";
    let row = match (text(settings, "index_type"), category) {
        (Some("category"), Some(id)) => c.query_row(&format!("{columns} WHERE external_id = ?1 ORDER BY id LIMIT 1"), [id], read).optional()?,
        _ => c.query_row(&format!("{columns} WHERE external_id = 'top-coins' AND source = 'internal' ORDER BY id LIMIT 1"), [], read).optional()?,
    };
    Ok(row.map(|(source, top_coins, by_exchange, available)| IndexRow {
        source,
        top_coins: match json_column(top_coins) { Value::Array(ids) => ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect(), _ => vec![] },
        by_exchange: json_column(by_exchange).as_object().cloned().unwrap_or_default(),
        available_exchanges: json_column(available).as_object().cloned().unwrap_or_default(),
    }))
}

/// Bots::DcaIndex#bounded_universe_size: a data-api index publishes its whole membership.
pub fn bounded(index: Option<&IndexRow>) -> Option<i64> {
    index.filter(|index| index.source.as_deref() == Some("deltabadger") && !index.top_coins.is_empty()).map(|index| index.top_coins.len() as i64)
}

fn holds_whole_universe(settings: &Map<String, Value>, bounded: Option<i64>) -> bool {
    crate::web::bot::cast_boolean(settings.get("hold_all")) && bounded.unwrap_or(0) > 0
}

fn num_coins(settings: &Map<String, Value>) -> Option<i64> {
    match settings.get("num_coins") { Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)), Some(Value::String(s)) => Some(super::to_i(&json!(s))), _ => None }
}

/// "Layer-1" as ActiveSupport's `titleize` writes "layer-1": words split on `-`, `_` and spaces, each capitalised.
// ponytail: the words a category id holds; Rails' inflector also knows acronyms and keeps apostrophes, which no id uses.
fn titleize(id: &str) -> String {
    id.split(['-', '_', ' ']).filter(|word| !word.is_empty()).map(|word| {
        let mut chars = word.chars();
        chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect())
    }).collect::<Vec<_>>().join(" ")
}

/// Bots::DcaIndex#display_index_name of a bot built from these settings (after_initialize's defaults included).
pub fn display_index_name(settings: &Map<String, Value>, bounded: Option<i64>, locale: &str) -> String {
    let category = text(settings, "index_category_id");
    let holds = holds_whole_universe(settings, bounded);
    let effective = if holds { bounded } else { num_coins(settings) };
    if let (Some((prefix, name)), Some(count)) = (count_named(category), effective) {
        return if holds { name.to_string() } else { format!("{prefix}{count}") };
    }
    if let Some(name) = text(settings, "index_name").filter(|name| !crate::ruby::blank(name)) { return name.to_string() }
    match text(settings, "index_type") {
        Some("top") | None => num_coins(settings).map_or_else(|| i18n::text(locale, "bot.dca_index.setup.pick_index.top_coins", &[]), |count| format!("Top {count}")),
        Some(kind) if crate::ruby::blank(kind) => num_coins(settings).map_or_else(|| i18n::text(locale, "bot.dca_index.setup.pick_index.top_coins", &[]), |count| format!("Top {count}")),
        _ => category.map_or_else(|| "Index".to_string(), titleize),
    }
}

/// The index bot `bot_relation.new(config)` builds and `save` writes, after its callbacks.
pub struct Built { pub exchange_id: Option<i64>, pub settings: Map<String, Value>, pub label: String, bounded: Option<i64> }

/// `x ||= value`.
fn default(settings: &mut Map<String, Value>, key: &str, value: Value) {
    if matches!(settings.get(key), None | Some(Value::Null | Value::Bool(false))) { settings.insert(key.into(), value); }
}

/// initialize_smart_intervalable_settings' amount: `[quote_amount / 10, minimum_smart_interval_quote_amount * 10].max
/// .round(least_precise_quote_decimals).to_f`, nil without an amount or a ticker. An index bot's venue floor is its quote's
/// largest minimum_quote_size (Bots::DcaIndex#minimum_for_exchange).
fn smart_quote(settings: &Map<String, Value>, decimals: Option<i64>, exchange: f64, listed: bool) -> Value {
    let (true, Some(decimals)) = (listed, decimals) else { return Value::Null };
    let Some(amount) = settings.get("quote_amount").filter(|v| present(Some(v))) else { return Value::Null };
    let Some(value) = amount.as_f64() else { return Value::Null };
    let seconds = match text(settings, "interval") { Some("hour") => 3_600.0, Some("day") => 86_400.0, Some("week") => 604_800.0, Some("month") => 2_629_746.0, _ => 0.0 };
    let frequency = if seconds > 0.0 { value / seconds * 300.0 } else { 0.0 };
    let scale = 10f64.powi(decimals as i32);
    let minimum = crate::engine::accounting::round_up(frequency, scale).max(1.0 / scale).max(exchange);
    let floor = minimum * 10.0;
    // Array#max keeps the first of two equal values; Integer#round(n > 0) is the integer itself.
    let tenth = if amount.is_i64() { (value as i64).div_euclid(10) as f64 } else { value / 10.0 };
    let round = |x: f64| if decimals == 0 { x.round() } else { crate::ruby::float_round(x, decimals as i32) };
    json!(if floor > tenth { round(floor) } else if amount.is_i64() { tenth } else { round(tenth) })
}

/// after_initialize (in the order Rails runs it: Smart Intervals, limit orders, the index's own), set_missed_quote_amount
/// and the before_validation callbacks (set_default_label, then clamp_num_coins_to_bounded_index, which default_label runs
/// first itself).
pub fn build(c: &Connection, config: &Value, locale: &str) -> Result<Built, WebError> {
    let exchange_id = super::exchange_id(Some(config));
    let mut settings = config.get("settings").and_then(Value::as_object).cloned().unwrap_or_default();
    let quote = settings.get("quote_asset_id").filter(|v| present(Some(v))).map(super::to_i);
    // `set_tickers`: the venue's available, trading-enabled tickers at the quote.
    let (listed, decimals, minimum): (i64, Option<i64>, Option<f64>) = match exchange_id {
        Some(exchange) => c.query_row("SELECT COUNT(*), MIN(quote_decimals), MAX(CAST(minimum_quote_size AS REAL)) FROM tickers \
                                       WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND quote_asset_id IS ?2",
                                      rusqlite::params![exchange, quote], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?,
        None => (0, None, None),
    };
    default(&mut settings, "smart_intervaled", json!(false));
    let smart = smart_quote(&settings, decimals, minimum.unwrap_or(0.0), listed > 0);
    default(&mut settings, "smart_interval_quote_amount", smart);
    default(&mut settings, "limit_ordered", json!(false));
    default(&mut settings, "limit_order_pcnt_distance", json!(0.001));
    let bounded = bounded(index_row(c, &settings)?.as_ref());
    default(&mut settings, "num_coins", json!(bounded.map_or(10, |size| size.min(10))));
    default(&mut settings, "allocation_flattening", json!(0.0));
    // clamp_num_coins_to_bounded_index.
    if let (Some(count), Some(size)) = (num_coins(&settings), bounded) {
        if !holds_whole_universe(&settings, bounded) && count > size { settings.insert("num_coins".into(), json!(size)); }
    }
    let category = text(&settings, "index_category_id");
    let name = display_index_name(&settings, bounded, locale);
    let label = if count_named(category).is_some() || text(&settings, "index_type") != Some("category") { name } else {
        [Some(name), settings.get("num_coins").filter(|v| !v.is_null()).map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    };
    let label = if crate::ruby::blank(&label) { i18n::text(locale, "bot.new", &[]) } else { label };
    Ok(Built { exchange_id, settings, label, bounded })
}

/// ActiveModel's message for an error code on `attribute`, as `errors.add(attribute, code, count:)` looks it up.
fn message(locale: &str, attribute: &str, code: &str, count: i64) -> String {
    let keys = [format!("activerecord.errors.models.bots/dca_index.attributes.{attribute}.{code}"), format!("activerecord.errors.models.bots/dca_index.{code}"),
                format!("activerecord.errors.models.bot.attributes.{attribute}.{code}"), format!("activerecord.errors.models.bot.{code}"),
                format!("activerecord.errors.messages.{code}"), format!("errors.attributes.{attribute}.{code}"), format!("errors.messages.{code}")];
    i18n::text_chain(locale, &keys, &[("count", i18n::Arg::Count(count))]).unwrap_or_else(|| format!("translation missing: {locale}.errors.messages.{code}"))
}

/// The create-context validation a wizard-built index bot can fail (CALLBACKS names why the others pass): the coin count,
/// between MIN_COINS and the universe (a bot holding the whole of it is judged against the larger of the two).
pub fn validate(bot: &Built, locale: &str) -> Vec<String> {
    let Some(count) = num_coins(&bot.settings) else { return vec![] };
    let max = bot.bounded.unwrap_or(MAX_COINS);
    let max = if holds_whole_universe(&bot.settings, bot.bounded) { max.max(count) } else { max };
    if count < MIN_COINS { return vec![message(locale, "num_coins", "greater_than_or_equal_to", MIN_COINS)] }
    if count > max { return vec![message(locale, "num_coins", "less_than_or_equal_to", max)] }
    vec![]
}

/// The before_save and before_create callbacks and the insert, on `c` inside the caller's write transaction. Returns the
/// new bot's id.
pub fn insert(c: &Connection, user: i64, bot: Built, now: DateTime<Utc>) -> Result<i64, WebError> {
    let at = format_time(now);
    let position: i64 = c.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM bots WHERE user_id = ?1", [user], |r| r.get(0))?;
    let transient = json!({"missed_quote_amount": 0, "missed_quote_amount_was_set": null});
    c.execute("INSERT INTO bots (user_id, exchange_id, type, status, label, position, settings, transient_data, settings_changed_at, created_at, updated_at) \
               VALUES (?1, ?2, 'Bots::DcaIndex', 0, ?3, ?4, ?5, ?6, ?7, ?7, ?7)",
              // The settings column as ActiveRecord's JSON coder writes it (ActiveSupport escapes <, > and &: an index's name may hold them).
              rusqlite::params![user, bot.exchange_id, bot.label, position, crate::web::bots::json(&Value::Object(bot.settings)), transient.to_string(), at])?;
    Ok(c.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_default_label() {
        let s = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(display_index_name(&s(json!({"index_type": "category", "index_category_id": "nasdaq-100", "num_coins": 12, "hold_all": true})), Some(12), "en"), "ND100");
        assert_eq!(display_index_name(&s(json!({"index_type": "category", "index_category_id": "nasdaq-100", "num_coins": 7})), Some(12), "en"), "ND7");
        assert_eq!(display_index_name(&s(json!({"index_type": "top", "num_coins": 10})), None, "en"), "Top 10");
        assert_eq!(display_index_name(&s(json!({"index_type": "category", "index_category_id": "layer-1", "num_coins": 10})), None, "en"), "Layer 1");
        assert_eq!(titleize("decentralized-finance-defi"), "Decentralized Finance Defi");
    }

    #[test]
    fn smart_amount_takes_the_venue_floor() {
        let s = json!({"quote_amount": 100, "interval": "week"});
        assert_eq!(smart_quote(s.as_object().unwrap(), Some(2), 1.0, true), json!(10.0));
        assert_eq!(smart_quote(s.as_object().unwrap(), Some(2), 5.0, true), json!(50.0));
        assert_eq!(smart_quote(s.as_object().unwrap(), Some(2), 0.0, false), Value::Null);
    }
}
