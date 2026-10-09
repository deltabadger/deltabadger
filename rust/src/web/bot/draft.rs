use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationContext { Update, Start }

#[derive(Clone, Debug, Default)]
pub struct JsonChanges {
    pub set: Map<String, Value>,
    pub remove: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SaveEffects {
    pub settings: JsonChanges,
    pub transient: JsonChanges,
    pub settings_changed: bool,
    pub composition_changed: bool,
}

use super::{action_params, start, Asset, Bot, Exchange, For, Kind, Ticker};
use crate::enums::BotStatus;
use crate::ruby::BigDec;
use crate::web::{format, i18n, timezone, WebError};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::json;

/// Keep the lock-time row and the load-time defaults separate. No method in this module writes
/// SQL, runs a job, calls a venue, or makes a renderability decision about a rejected form.
#[derive(Clone, Debug)]
pub struct Draft {
    pub original: Bot,
    pub raw_settings: Map<String, Value>,
    pub raw_transient: Map<String, Value>,
    pub baseline: Map<String, Value>,
    pub candidate: Bot,
    pub submitted_label: Option<Value>,
    pub submitted_exchange_id: Option<Value>,
    pub parsed: Map<String, Value>,
    pub errors: Vec<FieldError>,
    pub sliders: Option<Value>,
    pub submitted: Map<String, Value>,
    parse_errors: Vec<FieldError>,
    exchange_missing: bool,
    error_codes: Vec<(String, String, String)>,
}

/// A handler must distinguish submitted invalidity (422) from a database failure (500).
#[derive(Debug)]
pub enum ParseError { Invalid(String), Database(WebError) }
impl ParseError {
    pub fn status(&self) -> axum::http::StatusCode {
        match self { Self::Invalid(_) => axum::http::StatusCode::UNPROCESSABLE_ENTITY, Self::Database(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR }
    }
}

fn failure(message: &str) -> WebError { super::data(message.to_owned()) }
fn present(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}
fn text(value: &Value) -> String {
    match value { Value::Null => String::new(), Value::String(s) => s.clone(), other => other.to_string() }
}

fn numeric(value: &Value, integer: bool) -> Result<Value, WebError> {
    action_params::numeric(value, integer).map_err(|e| failure(match e {
        action_params::NumericError::Shape => "numeric parameter has the wrong shape",
        action_params::NumericError::Bound => "numeric parameter exceeds its bound",
    }))
}

fn percentage(value: &Value) -> Result<Value, WebError> {
    let n = numeric(value, false)?.as_f64().ok_or_else(|| failure("percentage is not numeric"))?;
    Ok(json!(format::Num::Float(n / 100.0).round(4).to_f()))
}

impl Draft {
    /// Call inside the writer's BEGIN IMMEDIATE. Ownership/type/deletion are checked in SQL
    /// before loading associations, defaults, or any guard reason.
    pub fn load(c: &Connection, owner: i64, id: i64, locale: &str) -> Result<Option<Self>, WebError> {
        let row = c.query_row("SELECT settings, transient_data FROM bots WHERE id = ?1 AND user_id = ?2 AND status <> 3 AND type IN ('Bots::DcaMultiAsset', 'Bots::DcaIndex')",
            (id, owner), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional()?;
        let Some((raw, transient)) = row else { return Ok(None) };
        // Bound stored input before loading associations or cloning a renderable model.
        if raw.len() > 65_536 || transient.len() > 65_536 { return Err(start::history_error()); }
        let raw_settings = super::object(&raw, "bots.settings").map_err(|_| start::history_error())?;
        let raw_transient = super::object(&transient, "bots.transient_data").map_err(|_| start::history_error())?;
        for values in [&raw_settings, &raw_transient] {
            action_params::validate(&Value::Object(values.clone())).map_err(|_| start::history_error())?;
        }
        let members: i64 = c.query_row("SELECT count(*) FROM (SELECT 1 FROM bot_index_assets WHERE bot_id=?1 LIMIT 1001)", [id], |r| r.get(0))?;
        if members > 1000 { return Err(start::history_error()); }
        let Some(bot) = Bot::find(c, owner, id, For::Page, locale)? else { return Ok(None) };
        let mut draft = Self::from_bot(bot);
        draft.raw_settings = raw_settings;
        draft.raw_transient = raw_transient;
        Ok(Some(draft))
    }

    pub fn from_bot(bot: Bot) -> Self {
        Self { raw_settings: bot.settings.clone(), raw_transient: bot.transient.clone(), baseline: bot.settings.clone(), candidate: bot.clone(), original: bot,
            submitted_label: None, submitted_exchange_id: None, parsed: Map::new(), errors: vec![], sliders: None, submitted: Map::new(), parse_errors: vec![], exchange_missing: false, error_codes: vec![] }
    }

    pub fn parse(&mut self, c: &Connection, params: &Value, zone: &str, now: DateTime<Utc>) -> Result<(), ParseError> {
        self.submitted = params.as_object().cloned().unwrap_or_default();
        match self.parse_inner(c, params, zone, now) {
            Ok(()) if self.parse_errors.is_empty() => Ok(()),
            Ok(()) => { self.errors = self.parse_errors.clone(); Err(ParseError::Invalid("invalid numeric settings".into())) }
            Err(WebError::Engine(crate::engine::EngineError::Data(message))) => {
                if self.parse_errors.is_empty() { self.parse_errors.push(FieldError { field: "settings".into(), message: message.clone() }); }
                self.errors = self.parse_errors.clone();
                Err(ParseError::Invalid(message))
            }
            Err(error) => Err(ParseError::Database(error)),
        }
    }

    fn parse_inner(&mut self, c: &Connection, params: &Value, zone: &str, now: DateTime<Utc>) -> Result<(), WebError> {
        action_params::validate(params).map_err(|_| failure("invalid settings parameter tree"))?;
        let fields = params.as_object().ok_or_else(|| failure("settings parameters are not an object"))?;
        let basket = self.candidate.kind == Kind::Basket;
        self.parsed.clear();
        // Actual parse_params output, deliberately narrower than stored_attributes / strong params.
        for (key, value) in fields {
            let common = matches!(key.as_str(), "quote_asset_id" | "quote_amount" | "interval" | "smart_intervaled" | "smart_interval_quote_amount" | "smart_interval_base_amount" |
                "limit_ordered" | "limit_order_pcnt_distance" | "start_time_enabled" | "start_time_mode" | "start_time_of_day" | "start_at" | "rebalance_enabled" | "rebalance_threshold");
            let basket_field = matches!(key.as_str(), "weighting" | "sell_interval" | "sell_amount" | "sell_quote_amount" | "quote_amount_limited" | "quote_amount_limit" | "base_amount_limited" | "base_amount_limit")
                || trigger_field(key);
            if !(common || (basket && basket_field) || (self.candidate.kind == Kind::Index && key == "allocation_flattening")) { continue; }
            let explicit_null = matches!(key.as_str(), "start_at" | "rebalance_threshold" | "sell_amount" | "sell_quote_amount");
            if key == "rebalance_enabled" { self.parsed.insert(key.clone(), json!(value.as_str().is_some_and(|s| matches!(s, "1" | "true")))); continue; }
            if key == "start_time_enabled" {
                if !value.is_null() && value.as_str() != Some("") { self.parsed.insert(key.clone(), json!(super::cast_boolean(Some(value)))); }
                continue;
            }
            if key == "start_at" { self.parsed.insert(key.clone(), date(value, zone, now)?); continue; }
            if !present(value) {
                if explicit_null { self.parsed.insert(key.clone(), Value::Null); }
                continue;
            }
            let result = if key.ends_with("_mode") && trigger_field(key) {
                let Some(prefix) = key.strip_suffix("_mode") else { continue };
                let action = if value == "flip" { if prefix.starts_with("sell_") { "start_buying" } else { "start_selling" } } else { "pause" };
                self.parsed.insert(format!("{prefix}_action"), json!(action));
                if !prefix.ends_with("price_drop_limit") { self.parsed.insert(format!("{prefix}_timing_condition"), json!(if value == "restrict" { "while" } else { "after" })); }
                continue;
            } else if key.ends_with("ed") {
                Ok(json!(value.as_str().is_some_and(|s| matches!(s, "1" | "true"))))
            } else if key.ends_with("_id") || key.ends_with("_period") { numeric(value, true) }
            else if matches!(key.as_str(), "limit_order_pcnt_distance" | "price_drop_limit" | "sell_price_drop_limit" | "rebalance_threshold") { percentage(value) }
            else if matches!(key.as_str(), "quote_amount" | "smart_interval_quote_amount" | "smart_interval_base_amount" | "sell_amount" | "sell_quote_amount" | "quote_amount_limit" | "base_amount_limit" | "allocation_flattening")
                || key.ends_with("_range_lower_bound") || key.ends_with("_range_upper_bound") || matches!(key.as_str(), "price_limit" | "sell_price_limit" | "indicator_limit" | "sell_indicator_limit") { numeric(value, false) }
            else { Ok(value.clone()) };
            match result {
                Ok(value) => { self.parsed.insert(key.clone(), value); }
                Err(e) => {
                    self.parse_errors.push(FieldError { field: key.clone(), message: "is outside the supported numeric range or shape".into() });
                    let _ = e;
                    self.parsed.insert(key.clone(), value.clone());
                }
            }
        }
        if basket { self.parse_allocations(fields)?; }
        else if let Some(coins) = fields.get("num_coins").filter(|v| present(v)) {
            let count = numeric(coins, true)?;
            let rendered = fields.get("num_coins_rendered").filter(|v| present(v)).map(|v| numeric(v, true)).transpose()?;
            if rendered.as_ref() != Some(&count) {
                let ceiling = fields.get("num_coins_ceiling").filter(|v| present(v)).map(|v| numeric(v, true)).transpose()?.and_then(|v| v.as_i64()).unwrap_or(self.candidate.max_coins());
                self.parsed.insert("hold_all".into(), json!(count.as_i64().is_some_and(|n| n >= ceiling)));
                self.parsed.insert("num_coins".into(), count);
            }
        }
        if self.candidate.exchange.class == "Exchanges::Hyperliquid" { self.parsed.insert("limit_ordered".into(), json!(true)); }
        self.candidate.settings.extend(self.parsed.clone());
        if let Some(label) = fields.get("label").filter(|v| present(v)) {
            self.submitted_label = Some(label.clone()); self.candidate.label = text(label);
        }
        if let Some(exchange) = fields.get("exchange_id").filter(|v| !v.is_null()) {
            self.submitted_exchange_id = Some(exchange.clone());
            let id = match exchange { Value::Bool(b) => i64::from(*b), _ => numeric(exchange, true)?.as_i64().unwrap_or(0) };
            let found = c.query_row("SELECT id, name, type, maker_fee FROM exchanges WHERE id = ?1", [id], |r| Ok(Exchange {
                id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), class: r.get::<_, Option<String>>(2)?.unwrap_or_default(), maker_fee: r.get(3)?,
            })).optional()?;
            self.exchange_missing = found.is_none();
            self.candidate.exchange = found.unwrap_or(Exchange { id, name: String::new(), class: String::new(), maker_fee: None });
        }
        self.refresh(c)?;
        Ok(())
    }

    fn parse_allocations(&mut self, fields: &Map<String, Value>) -> Result<(), WebError> {
        let mut allocations = self.candidate.settings.get("allocations").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut changed = false;
        if let Some(value) = fields.get("allocations").filter(|v| present(v)) {
            let weights = value.as_object().ok_or_else(|| failure("allocations must be an object"))?;
            if weights.len() > 1000 { return Err(failure("allocation work budget exceeded")); }
            self.sliders = Some(value.clone());
            allocations.clear();
            for (key, value) in weights {
                let n = numeric(&json!(text(value).replace(',', ".")), false)?.as_f64().ok_or_else(|| failure("allocation is not numeric"))?;
                allocations.insert(key.clone(), if n < 0.0 { json!(0) } else if n > 100.0 { json!(1) } else { json!(n / 100.0) });
            }
            changed = true;
        }
        for key in ["add_asset_id", "remove_asset_id"] {
            if let Some(value) = fields.get(key).filter(|v| present(v)) {
                let id = numeric(value, true)?.to_string();
                if key == "add_asset_id" { allocations.entry(id).or_insert(json!(0.0)); }
                else { allocations.shift_remove(&id); }
                changed = true;
            }
        }
        if fields.get("normalize_allocations").is_some_and(|v| matches!(text(v).as_str(), "1" | "true")) {
            normalize(&mut allocations)?; changed = true;
        }
        if changed { self.parsed.insert("allocations".into(), Value::Object(allocations)); }
        Ok(())
    }

    pub(super) fn refresh(&mut self, c: &Connection) -> Result<(), WebError> {
        let quote = self.candidate.settings.get("quote_asset_id").and_then(Value::as_i64);
        self.candidate.quote_asset = quote.map(|id| Asset::find(c, id)).transpose()?.flatten();
        if self.candidate.kind == Kind::Basket {
            self.candidate.base_assets.clear();
            for (id, _) in self.candidate.allocations() {
                if let Some(asset) = Asset::find(c, id)? { self.candidate.base_assets.push(asset); }
            }
        }
        let ids: Vec<i64> = self.candidate.allocations().iter().map(|(id, _)| *id).chain(self.candidate.memberships.iter().map(|m| m.asset.id)).collect();
        self.candidate.tickers = Ticker::all(c, "t.exchange_id = ?1 AND t.quote_asset_id IS ?2 AND t.available = 1 AND t.trading_enabled = 1", &[&self.candidate.exchange.id, &quote])?
            .into_iter().filter(|t| self.candidate.kind == Kind::Index || ids.contains(&t.base_asset_id)).collect();
        Ok(())
    }

    pub fn error_sentence(&self, locale: &str) -> String {
        let messages: Vec<&str> = self.errors.iter().map(|e| e.message.as_str()).collect();
        sentence(&messages, locale)
    }
}

fn trigger_field(key: &str) -> bool {
    let key = key.strip_prefix("sell_").unwrap_or(key);
    for prefix in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
        if key == format!("{prefix}ed") || key == prefix && prefix != "moving_average_limit" { return true; }
        if let Some(suffix) = key.strip_prefix(prefix) {
            return match prefix {
                "price_limit" => matches!(suffix, "_range_lower_bound" | "_range_upper_bound" | "_value_condition" | "_in_ticker_id" | "_mode"),
                "price_drop_limit" => matches!(suffix, "_time_window_condition" | "_in_ticker_id" | "_mode"),
                "moving_average_limit" => matches!(suffix, "_value_condition" | "_in_ticker_id" | "_in_ma_type" | "_in_timeframe" | "_in_period" | "_mode"),
                _ => matches!(suffix, "_value_condition" | "_in_ticker_id" | "_in_indicator" | "_in_timeframe" | "_mode"),
            };
        }
    }
    false
}

fn normalize(weights: &mut Map<String, Value>) -> Result<(), WebError> {
    if weights.is_empty() { return Ok(()); }
    let mut values = weights.values().map(|v| numeric(v, false).and_then(|n| n.as_f64().ok_or_else(|| failure("allocation"))).map(|n| n.max(0.0))).collect::<Result<Vec<_>, _>>()?;
    values.iter().try_fold(0.0, |sum, value| {
        if *value > f64::MAX - sum { Err(failure("allocation total overflow")) } else { Ok(sum + value) }
    })?;
    let mut total = super::float_sum(values.iter().copied());
    if total <= 0.0 { values.fill(1.0); total = values.len() as f64; }
    let exact: Vec<f64> = values.iter().map(|n| n / total * 1000.0).collect();
    let mut units: Vec<u32> = exact.iter().map(|n| n.floor() as u32).collect();
    let remainder = 1000u32.checked_sub(units.iter().sum()).ok_or_else(|| failure("allocation rounding overflow"))?;
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|a, b| {
        let fraction = |i: &usize| exact.get(*i).zip(units.get(*i)).map_or(0.0, |(e, u)| e - f64::from(*u));
        fraction(b).total_cmp(&fraction(a)).then(a.cmp(b))
    });
    for index in order.into_iter().take(remainder as usize) { if let Some(unit) = units.get_mut(index) { *unit += 1; } }
    for (value, unit) in weights.values_mut().zip(units) { *value = json!(f64::from(unit) / 1000.0); }
    Ok(())
}

/// Bounded forms accepted by datetime-local and ISO8601 callers, plus the short numeric
/// fragments exercised by Rails' Date._parse vectors. No unbounded calendar search.
fn date(value: &Value, name: &str, now: DateTime<Utc>) -> Result<Value, WebError> {
    let Some(input) = value.as_str() else { return Ok(Value::Null) };
    if input.len() > 128 { return Err(failure("start date exceeds its bound")); }
    let s = input.trim();
    if s.is_empty() { return Ok(Value::Null); }
    if let Ok(at) = DateTime::parse_from_rfc3339(s) { return Ok(json!(at.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))); }
    let zone = timezone::zone(name).unwrap_or(chrono_tz::UTC);
    let naive = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter().find_map(|pattern| NaiveDateTime::parse_from_str(s, pattern).ok())
        .or_else(|| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0))
        .or_else(|| {
            // Date._parse's compact numeric fragments: decimal-separated digits are a
            // right-aligned HHMMSS, while a three-digit run supplies only yday. Time.zone.parse
            // ignores yday when it supplies the missing calendar fields from today's date.
            let day = now.with_timezone(&zone).date_naive();
            let runs: Vec<&str> = s.split(|ch: char| !ch.is_ascii_digit()).filter(|part| !part.is_empty()).collect();
            if let Some((whole, fraction)) = s.split_once(['.', ',']) {
                if !fraction.starts_with(|ch: char| ch.is_ascii_digit()) || whole.is_empty() || whole.len() > 6 || !whole.bytes().all(|b| b.is_ascii_digit()) { return None; }
                let n = whole.parse::<u32>().ok()?;
                return day.and_hms_opt(n / 10000, n / 100 % 100, n % 100);
            }
            if runs.last().is_some_and(|run| run.len() == 3) { return day.and_hms_opt(0, 0, 0); }
            if runs.len() == 1 && s.bytes().all(|b| b.is_ascii_digit()) && s.len() <= 2 {
                return day.with_day(s.parse().ok()?)?.and_hms_opt(0, 0, 0);
            }
            None
        });
    let Some(naive) = naive else { return Ok(Value::Null) };
    if !(1..=9999).contains(&naive.year()) { return Err(failure("start date exceeds its bound")); }
    // ActiveSupport chooses DST on overlap, and moves a nonexistent local time forward an hour.
    let at = match zone.from_local_datetime(&naive) {
        LocalResult::Single(at) => Some(at), LocalResult::Ambiguous(a, b) => Some(a.min(b)),
        LocalResult::None => naive.checked_add_signed(Duration::hours(1)).and_then(|next| zone.from_local_datetime(&next).earliest()),
    };
    Ok(at.map_or(Value::Null, |at| json!(at.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))))
}

fn sentence(messages: &[&str], locale: &str) -> String {
    let connector = |key: &str, fallback: &str| {
        let value = i18n::text(locale, key, &[]);
        if value.starts_with("Translation missing:") { fallback.to_owned() } else { value }
    };
    match messages {
        [] => String::new(), [one] => (*one).to_owned(), [a, b] => format!("{a}{}{b}", connector("support.array.two_words_connector", " and ")),
        many => match many.split_last() {
            Some((last, rest)) => format!("{}{}{last}", rest.join(&connector("support.array.words_connector", ", ")), connector("support.array.last_word_connector", ", and ")),
            None => String::new(),
        },
    }
}

impl Draft {
    fn add(&mut self, field: &str, message: String) {
        let error = FieldError { field: field.into(), message };
        if !self.errors.contains(&error) { self.errors.push(error); }
    }
    fn error(&mut self, locale: &str, field: &str, code: &str, count: Option<f64>) {
        let count = count.map(|n| if n.fract() == 0.0 { format!("{n:.0}") } else { format::float_to_s(n) });
        let code_key = (field.to_owned(), code.to_owned(), count.clone().unwrap_or_default());
        if self.error_codes.contains(&code_key) { return; }
        self.error_codes.push(code_key);
        let args = count.as_deref().map(|c| vec![("count", i18n::Arg::Text(c))]).unwrap_or_default();
        let kind = if self.candidate.kind == Kind::Basket { "bots/dca_multi_asset" } else { "bots/dca_index" };
        let keys = vec![format!("activerecord.errors.models.{kind}.attributes.{field}.{code}"), format!("activerecord.errors.models.{kind}.{code}"), format!("activerecord.errors.models.bot.attributes.{field}.{code}"), format!("activerecord.errors.models.bot.{code}"),
            format!("activerecord.errors.messages.{code}"), format!("errors.attributes.{field}.{code}"), format!("errors.messages.{code}")];
        let mut message = None;
        for key in &keys {
            let found = i18n::text(locale, key, &args);
            if !found.starts_with("Translation missing:") { message = Some(found); break; }
        }
        self.errors.push(FieldError { field: field.into(), message: message.unwrap_or_else(|| format!("Translation missing. Options considered were:\n{}", keys.iter().map(|key| format!("- {locale}.{key}")).collect::<Vec<_>>().join("\n"))) });
    }
    fn read(&self, field: &str) -> Value {
        let value = self.candidate.settings.get(field);
        if field.ends_with("_action") { return value.filter(|v| present(v)).cloned().unwrap_or(json!("pause")); }
        if field == "direction" { return value.filter(|v| present(v)).cloned().unwrap_or(json!("buying")); }
        if field == "weighting" { return value.filter(|v| present(v)).cloned().unwrap_or(json!("manual")); }
        if field == "sell_interval" { return value.filter(|v| present(v)).cloned().unwrap_or_else(|| self.read("interval")); }
        if field == "sell_denomination" {
            return if self.candidate.one_asset() { value.filter(|v| present(v)).cloned().unwrap_or(json!("quote")) } else { json!("quote") };
        }
        if field == "rebalance_threshold" { return value.filter(|v| present(v)).map(|v| numeric(v, false).unwrap_or(Value::Null)).unwrap_or(json!(0.05)); }
        if let Some(value) = value.filter(|v| !v.is_null()) { return value.clone(); }
        if field == "base_amount_limited" || field.starts_with("sell_") && field.ends_with("ed") { return json!(false); }
        if let Some(field) = field.strip_prefix("sell_") {
            return match field {
                "price_limit" => json!(1_000_000), "price_limit_range_lower_bound" => json!(0), "price_limit_range_upper_bound" => json!(1_000_000),
                "price_drop_limit" => json!(0.2), "price_drop_limit_time_window_condition" => json!("twenty_four_hours"),
                "indicator_limit" => json!(70), "indicator_limit_in_indicator" => json!("rsi"),
                "moving_average_limit_in_period" => json!(9), "moving_average_limit_in_ma_type" => json!("sma"),
                key if key.ends_with("_value_condition") => json!("above"),
                key if key.ends_with("_timing_condition") => json!("while"),
                key if key.ends_with("_in_timeframe") => json!("one_day"), _ => Value::Null,
            };
        }
        Value::Null
    }
    fn inclusion(&mut self, locale: &str, field: &str, allowed: &[&str]) {
        if !self.read(field).as_str().is_some_and(|s| allowed.contains(&s)) { self.error(locale, field, "inclusion", None); }
    }
    fn boolean(&mut self, locale: &str, field: &str) {
        if !self.read(field).is_boolean() { self.error(locale, field, "inclusion", None); }
    }
    fn number(&mut self, locale: &str, field: &str, lower: Option<(f64, bool)>, upper: Option<f64>, integer: bool) -> Option<f64> {
        let value = self.read(field);
        // Numericality is a strict numeric read. The request parser has already applied Ruby's
        // prefix coercion; a malformed stored value must not become a successful zero here.
        let n = match &value { Value::Number(n) => n.as_f64(), Value::String(s) => s.trim().parse::<f64>().ok(), _ => None }.filter(|n| n.is_finite());
        let Some(n) = n else { self.error(locale, field, "not_a_number", None); return None };
        if integer && n.fract() != 0.0 { self.error(locale, field, "not_an_integer", None); return Some(n); }
        if let Some((bound, inclusive)) = lower {
            if if inclusive { n < bound } else { n <= bound } { self.error(locale, field, if inclusive { "greater_than_or_equal_to" } else { "greater_than" }, Some(bound)); }
        }
        if let Some(bound) = upper { if n > bound { self.error(locale, field, "less_than_or_equal_to", Some(bound)); } }
        Some(n)
    }
    fn presence(&mut self, locale: &str, field: &str) {
        if !present(&self.read(field)) { self.error(locale, field, "blank", None); }
    }
    fn composition_changed(&self) -> bool {
        let keys: &[&str] = if self.candidate.kind == Kind::Basket { &["allocations", "quote_asset_id", "weighting"] }
            else { &["quote_asset_id", "num_coins", "hold_all", "allocation_flattening", "index_type", "index_category_id"] };
        keys.iter().any(|key| !equal_option(self.raw_settings.get(*key), self.candidate.settings.get(*key)))
    }

    /// Shared writer boundary, after Rails validation so Rails refusals keep their order/text.
    /// Returns true only for an additional Rust schedule-range refusal.
    pub fn validate_schedule_bounds(&mut self, locale: &str, now: DateTime<Utc>) -> bool {
        if !self.errors.is_empty() { return false; }
        let bot = &self.candidate;
        let outside = bot.schedule_bounds().is_some() || bot.checkpoints(now).is_none_or(|cp| {
            DateTime::from_timestamp_micros(cp.next_us).is_none() || DateTime::from_timestamp_micros(cp.last_us).is_none()
        });
        if !outside { return false; }
        let field = if bot.effective().is_some_and(|eff| eff.seconds() < super::MIN_SPAN_SECONDS) { "smart_interval_quote_amount" } else { "quote_amount" };
        self.error(locale, field, "greater_than", Some(0.0));
        true
    }

    pub fn validate(&mut self, c: &Connection, context: ValidationContext, now: DateTime<Utc>, provider: bool, locale: &str) -> Result<(), WebError> {
        self.errors = self.parse_errors.clone();
        self.error_codes.clear();
        let starting = context == ValidationContext::Start;
        if self.candidate.kind == Kind::Basket {
            if let Some(weights) = self.candidate.settings.get_mut("allocations").and_then(Value::as_object_mut) {
                if weights.len() == 1 { for value in weights.values_mut() { *value = json!(1.0); } }
            }
        } else if let Some(size) = self.candidate.bounded_universe_size() {
            if !self.candidate.holds_whole_universe() {
                if let Some(coins) = self.candidate.settings.get("num_coins").and_then(Value::as_i64) {
                    if coins > size { self.candidate.settings.insert("num_coins".into(), json!(size)); }
                }
            }
        }
        let sells_base = self.candidate.kind == Kind::Basket && self.read("direction") == "selling" && self.read("sell_denomination") == "base";
        if sells_base && self.read("smart_intervaled") == json!(true) && !present(&self.read("smart_interval_base_amount")) && present(&self.read("sell_amount")) {
            let minimum = self.base_minimum();
            let amount = numeric(&self.read("sell_amount"), false)?.as_f64().unwrap_or(0.0);
            let split = BigDec::from_f64(amount).ok().and_then(|n| n.div(&BigDec::from_i64(10)));
            let floor = format::Num::Float(minimum.value.to_f() * 10.0);
            let amount = split.map(format::Num::Dec).filter(|n| n.to_f() >= floor.to_f()).unwrap_or(floor);
            self.candidate.settings.insert("smart_interval_base_amount".into(), json!(amount.round(minimum.decimals).to_f()));
        }
        // Inheritance and concern order is observable in errors.messages. Group fields only after
        // every callback, retaining each field's first position and all duplicate messages.
        if starting && self.candidate.exchange.retired() { self.add("base", i18n::text(locale, "errors.exchange_retired", &[])); }
        if self.candidate.label.trim().is_empty() { self.error(locale, "label", "blank", None); }
        if starting && self.original.status == BotStatus::Archived { self.add("status", i18n::text(locale, "errors.bots.archived", &[])); }
        self.presence(locale, "quote_amount");
        self.number(locale, "quote_amount", Some((0.0, false)), None, false);
        if self.candidate.kind == Kind::Basket { self.inclusion(locale, "weighting", &["manual", "market_cap"]); }
        else {
            self.presence(locale, "num_coins");
            let max = if self.candidate.holds_whole_universe() {
                self.candidate.max_coins().max(self.candidate.settings.get("num_coins").and_then(Value::as_i64).unwrap_or(0))
            } else { self.candidate.bounded_universe_size().unwrap_or(100) };
            self.number(locale, "num_coins", Some((2.0, true)), Some(max as f64), false);
            self.presence(locale, "allocation_flattening");
            self.number(locale, "allocation_flattening", Some((0.0, true)), Some(1.0), false);
            self.presence(locale, "index_type"); self.inclusion(locale, "index_type", &["top", "category"]);
            if self.candidate.exchange.id != 0 && !matches!(self.candidate.status, BotStatus::Stopped | BotStatus::Deleted | BotStatus::Archived)
                && (starting || self.candidate.exchange.id != self.original.exchange.id || self.candidate.settings.get("quote_asset_id") != self.raw_settings.get("quote_asset_id"))
                && self.candidate.tickers.is_empty() {
                self.add("exchange", i18n::text(locale, "errors.bots.exchange_asset_mismatch", &[("exchange_name", i18n::Arg::Text(&self.candidate.exchange.name))]));
            }
        }
        if context == ValidationContext::Update {
            if self.candidate.quote_asset.is_none() { self.error(locale, "quote_asset_id", "invalid", None); }
            if self.original.has_orders && self.raw_settings.get("quote_asset_id") != self.candidate.settings.get("quote_asset_id") { self.error(locale, "quote_asset_id", "unchangeable", None); }
            if self.original.working() && self.raw_settings.get("interval") != self.candidate.settings.get("interval") { self.add("settings", "Interval cannot be changed while the bot is running".into()); }
            if self.original.has_waiting_orders && self.original.exchange.id != self.candidate.exchange.id {
                self.add("exchange", i18n::text(locale, "errors.bots.exchange_change_while_open_orders", &[("exchange_name", i18n::Arg::Text(&self.candidate.exchange.name))]));
            }
        }
        if self.candidate.kind == Kind::Basket {
            if starting {
                let quote = self.candidate.settings.get("quote_asset_id").and_then(Value::as_i64);
                for (id, _) in self.candidate.allocations() {
                    let existing = self.candidate.memberships.iter().filter(|m| m.in_index).filter_map(|m| m.ticker.as_ref()).find(|t| t.base_asset_id == id).cloned();
                    let ticker = match existing { Some(t) => Some(t), None => Ticker::all(c, "t.exchange_id = ?1 AND t.base_asset_id = ?2 AND t.quote_asset_id IS ?3", &[&self.candidate.exchange.id, &id, &quote])?.into_iter().next() };
                    if !ticker.is_some_and(|t| t.tradable()) { self.error(locale, "allocations", "invalid", None); break; }
                }
            }
            let ids: Vec<i64> = self.candidate.allocations().iter().map(|(id, _)| *id).collect();
            let quote = self.candidate.settings.get("quote_asset_id").and_then(Value::as_i64);
            let allowed: Vec<i64> = Ticker::all(c, "t.exchange_id = ?1 AND t.quote_asset_id IS ?2", &[&self.candidate.exchange.id, &quote])?.into_iter().filter(|t| ids.contains(&t.base_asset_id)).map(|t| t.id).collect();
            if !allowed.is_empty() {
                for prefix in trigger_prefixes() {
                    if self.read(&format!("{prefix}ed")) != json!(true) { continue; }
                    if let Some(watched) = self.candidate.settings.get(&format!("{prefix}_in_ticker_id")).filter(|v| present(v)) {
                        let id = numeric(watched, true)?.as_i64();
                        if !id.is_some_and(|id| allowed.contains(&id)) { self.add("settings", i18n::text(locale, "errors.bots.multi_asset.condition_subject", &[])); }
                    }
                }
            }
        } else if starting && !provider { self.add("base", i18n::text(locale, "errors.bots.market_data_required", &[])); }
        self.boolean(locale, "smart_intervaled");
        if self.read("smart_intervaled") == json!(true) && self.read("direction") != "selling" {
            if let Some(amount) = self.number(locale, "smart_interval_quote_amount", None, None, false) {
                let minimum = start::smart_interval_minimum(&self.candidate);
                if amount < minimum.value.to_f() {
                    if let Some(message) = start::smart_interval_minimum_message(&self.candidate, &minimum, locale) { self.add("smart_interval_quote_amount", message); }
                    else { self.error(locale, "smart_interval_quote_amount", "greater_than_or_equal_to", Some(minimum.value.to_f())); }
                }
            }
        }
        if sells_base && self.read("smart_intervaled") == json!(true) && present(&self.read("sell_amount")) {
            if let Some(amount) = self.number(locale, "smart_interval_base_amount", None, None, false) {
                let minimum = self.base_minimum();
                if amount < minimum.value.to_f() {
                    let mut base = self.candidate.clone();
                    base.quote_asset = base.base_assets.first().cloned();
                    if let Some(message) = start::smart_interval_minimum_message(&base, &minimum, locale) { self.add("smart_interval_base_amount", message); }
                    else { self.error(locale, "smart_interval_base_amount", "greater_than_or_equal_to", Some(minimum.value.to_f())); }
                }
            }
        }
        self.boolean(locale, "limit_ordered");
        if !matches!(self.read("limit_ordered"), Value::Null | Value::Bool(false)) { self.number(locale, "limit_order_pcnt_distance", Some((0.0, true)), Some(1.0), false); }
        if self.candidate.kind == Kind::Basket {
            self.boolean(locale, "quote_amount_limited");
            if self.read("quote_amount_limited") == json!(true) {
                self.number(locale, "quote_amount_limit", Some((start::minimum_quote_amount_limit(&self.candidate), true)), None, false);
                if starting && self.read("direction") != "selling" && start::amount_limit(c, &self.candidate)?.is_some_and(|limit| limit.reached) { self.error(locale, "settings", "quote_amount_limit_reached", None); }
            }
            self.boolean(locale, "base_amount_limited");
            if self.candidate.one_asset() && self.read("base_amount_limited") == json!(true) && self.read("direction") == "selling" {
                self.number(locale, "base_amount_limit", Some((0.0, false)), None, false);
                if starting && self.base_cap_reached(c)? { self.error(locale, "settings", "base_amount_limit_reached", None); }
            }
            self.conditions(locale);
        }
        self.presence(locale, "interval"); self.inclusion(locale, "interval", &["hour", "day", "week", "month"]);
        if starting && self.candidate.start_time_enabled() {
            match self.candidate.text("start_time_mode") {
                Some("date") => {
                    let at = self.candidate.text("start_at").and_then(|s| DateTime::parse_from_rfc3339(s).ok());
                    if at.is_none() { self.error(locale, "start_at", "blank", None); }
                    else if at.is_some_and(|at| at <= now) { self.error(locale, "start_at", "must_be_future", None); }
                }
                Some(mode) if start::MODES.contains(&mode) => {
                    if !start::hhmm(self.candidate.text("start_time_of_day")) { self.error(locale, "start_time_of_day", "invalid", None); }
                }
                _ => self.error(locale, "start_time_mode", "inclusion", None),
            }
        }
        let carry = self.candidate.transient.get("missed_quote_amount");
        if let Some(value) = carry.filter(|v| present(v)) {
            if numeric(value, false)?.as_f64().is_some_and(|n| n < 0.0) { self.error(locale, "missed_quote_amount", "greater_than_or_equal_to", Some(0.0)); }
        }
        self.number(locale, "rebalance_threshold", Some((0.0, false)), Some(1.0), false);
        if self.candidate.kind == Kind::Basket {
            self.allocations_validation(c, context, locale)?;
            self.inclusion(locale, "direction", &["buying", "selling"]);
            self.inclusion(locale, "sell_denomination", &["base", "quote"]);
            for field in ["sell_amount", "sell_quote_amount"] {
                if present(&self.read(field)) {
                    // Rails readers call to_d (an invalid stored string becomes zero).
                    if numeric(&self.read(field), false)?.as_f64().is_some_and(|v| v <= 0.0) { self.error(locale, field, "greater_than", Some(0.0)); }
                }
            }
            self.inclusion(locale, "sell_interval", &["hour", "day", "week", "month"]);
            if context == ValidationContext::Update && self.original.working() && self.read("direction") == "selling" && self.original.settings.get("sell_interval") != self.candidate.settings.get("sell_interval") {
                self.add("settings", "Sell interval cannot be changed while the bot is selling".into());
            }
        }
        // The action contract also freezes index composition while running or pending.
        // Rails currently applies this lock only to baskets; retain its recorded message.
        if self.candidate.kind == Kind::Index && context == ValidationContext::Update
            && (self.original.working() || self.candidate.transient.get("rebalance_pending").is_some_and(present))
            && (self.composition_changed() || self.original.exchange.id != self.candidate.exchange.id) {
            self.add("allocations", i18n::text(locale, "errors.bots.multi_asset.locked_while_running", &[]));
        }
        let mut fields = Vec::<String>::new();
        for e in &self.errors { if !fields.contains(&e.field) { fields.push(e.field.clone()); } }
        self.errors.sort_by_key(|e| fields.iter().position(|f| f == &e.field));
        Ok(())
    }

    fn base_minimum(&self) -> start::Minimum {
        let Some(decimals) = self.candidate.tickers.iter().map(|t| t.base_decimals).min() else {
            return start::Minimum { value: format::Num::Int(0), reason: start::Reason::None, decimals: 0 };
        };
        let interval = self.read("sell_interval");
        let seconds = match interval.as_str() { Some("hour") => 3600.0, Some("day") => 86400.0, Some("week") => 604800.0, Some("month") => 2629746.0, _ => 0.0 };
        let amount = numeric(&self.read("sell_amount"), false).ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let frequency = if amount > 0.0 && seconds > 0.0 { amount / seconds * 300.0 } else { 0.0 };
        let scale = 10f64.powi(i32::from(decimals));
        let precision = 1.0 / scale;
        start::Minimum { value: format::Num::Float(start::round_up(frequency, scale).max(precision)),
            reason: if frequency >= precision { start::Reason::Frequency } else { start::Reason::Precision }, decimals }
    }

    fn base_cap_reached(&self, c: &Connection) -> Result<bool, WebError> {
        let limit = numeric(&self.read("base_amount_limit"), false)?;
        let limit = BigDec::parse(&limit.to_string()).map_err(|_| failure("base cap exceeds numeric bounds"))?;
        let since = self.candidate.transient.get("base_amount_limit_enabled_at").and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|at| crate::codec::format_time(at.with_timezone(&Utc)));
        let mut statement = c.prepare("SELECT CASE WHEN external_status = 2 THEN COALESCE(amount_exec, amount) WHEN external_status IN (0, 1) THEN amount ELSE COALESCE(amount_exec, 0) END FROM transactions WHERE bot_id = ?1 AND side = 1 AND status = 0 AND transaction_type = 'REGULAR' AND created_at >= ?2 AND external_status IN (0, 1, 2, 3, 4) LIMIT 100001")?;
        let mut rows = statement.query((self.candidate.id, since))?;
        let mut total = BigDec::zero();
        let mut count = 0;
        while let Some(row) = rows.next()? {
            count += 1;
            if count > start::HISTORY_WORK_BUDGET { return Err(start::history_error()); }
            if let Some(amount) = row.get::<_, super::Stored>(0)?.0 { total = &total + &amount; }
        }
        let floor = self.candidate.tickers.iter().map(|t| t.base_decimals).min().map_or(0.0, |d| 10f64.powi(-i32::from(d)));
        let floor = BigDec::from_f64(floor).map_err(|_| failure("base cap floor exceeds numeric bounds"))?;
        Ok((&limit - &total).max(BigDec::zero()) < floor)
    }

    fn conditions(&mut self, locale: &str) {
        // Follow each concern's declaration order, including the intentionally asymmetric
        // price/sell-price validations in Rails.
        self.boolean(locale, "price_limited");
        for sell in [false, true] {
            let prefix = if sell { "sell_price_limit" } else { "price_limit" };
            if self.read(&format!("{prefix}ed")) == json!(true) {
                for suffix in ["", "_range_lower_bound", "_range_upper_bound"] { self.number(locale, &format!("{prefix}{suffix}"), Some((0.0, true)), None, false); }
            }
            if !sell {
                self.inclusion(locale, "price_limit_timing_condition", &["while", "after"]);
                self.inclusion(locale, "price_limit_value_condition", &["above", "below", "between"]);
            }
            self.inclusion(locale, &format!("{prefix}_action"), if sell { &["pause", "start_buying"] } else { &["pause", "start_selling"] });
        }
        self.boolean(locale, "price_drop_limited");
        if self.read("price_drop_limited") == json!(true) { self.number(locale, "price_drop_limit", Some((0.0, true)), Some(1.0), false); }
        self.inclusion(locale, "price_drop_limit_time_window_condition", &["ath", "twenty_four_hours"]);
        self.inclusion(locale, "sell_price_drop_limit_time_window_condition", &["twenty_four_hours", "seven_days"]);
        self.inclusion(locale, "price_drop_limit_action", &["pause", "start_selling"]);
        if self.read("sell_price_drop_limited") == json!(true) { self.number(locale, "sell_price_drop_limit", Some((0.0, true)), Some(1.0), false); }
        self.inclusion(locale, "sell_price_drop_limit_action", &["pause", "start_buying"]);
        for base in ["moving_average_limit", "indicator_limit"] {
            for sell in [false, true] {
                let prefix = if sell { format!("sell_{base}") } else { base.to_owned() };
                self.boolean(locale, &format!("{prefix}ed"));
                if base == "indicator_limit" && self.read(&format!("{prefix}ed")) == json!(true) { self.number(locale, &prefix, None, None, false); }
                self.inclusion(locale, &format!("{prefix}_timing_condition"), &["while", "after"]);
                self.inclusion(locale, &format!("{prefix}_value_condition"), &["above", "below"]);
                self.inclusion(locale, &format!("{prefix}_in_{}", if base == "moving_average_limit" { "ma_type" } else { "indicator" }), if base == "moving_average_limit" { &["sma", "ema"] } else { &["rsi"] });
                self.inclusion(locale, &format!("{prefix}_in_timeframe"), super::TIMEFRAMES);
                if base == "moving_average_limit" && self.read(&format!("{prefix}ed")) == json!(true) { self.number(locale, &format!("{prefix}_in_period"), Some((0.0, false)), None, true); }
                self.inclusion(locale, &format!("{prefix}_action"), if sell { &["pause", "start_buying"] } else { &["pause", "start_selling"] });
            }
        }
    }

    fn allocations_validation(&mut self, c: &Connection, context: ValidationContext, locale: &str) -> Result<(), WebError> {
        let weights = self.candidate.settings.get("allocations").and_then(Value::as_object).cloned().unwrap_or_default();
        if weights.is_empty() { self.add("allocations", i18n::text(locale, "errors.bots.multi_asset.min_assets", &[])); }
        if !weights.values().all(|v| v.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n))) { self.error(locale, "allocations", "invalid", None); }
        let quote = self.candidate.settings.get("quote_asset_id").and_then(Value::as_i64);
        if quote.is_some_and(|id| weights.contains_key(&id.to_string())) { self.error(locale, "allocations", "invalid", None); }
        let mut known = std::collections::HashSet::new();
        let mut ids = vec![];
        for key in weights.keys() {
            let id = numeric(&json!(key), true)?.as_i64().unwrap_or(0);
            ids.push(id);
            if Asset::find(c, id)?.is_some() { known.insert(id); }
        }
        if known.len() != weights.len() { self.error(locale, "allocations", "invalid", None); }
        if context == ValidationContext::Start {
            if weights.len() > super::MAX_ASSETS { self.add("base", i18n::text(locale, "bot.dca_multi_asset.too_many_assets", &[("count", i18n::Arg::Count((weights.len() - super::MAX_ASSETS) as i64))])); }
            if self.read("weighting") != "market_cap" && (super::float_sum(weights.values().filter_map(Value::as_f64)) - 1.0).abs() > 0.001 { self.add("allocations", i18n::text(locale, "bot.dca_multi_asset.normalize_first", &[])); }
        }
        let exchange_changed = self.candidate.exchange.id != self.original.exchange.id;
        if self.candidate.exchange.id != 0 && (self.composition_changed() || exchange_changed) {
            if self.exchange_missing { self.error(locale, "exchange", "invalid", None); }
            else {
                let mut missing = vec![];
                for id in ids {
                    if !self.candidate.tickers.iter().any(|t| t.base_asset_id == id) {
                        if let Some(asset) = Asset::find(c, id)? { missing.push((id, asset.symbol().to_owned())); }
                        else { missing.push((id, String::new())); }
                    }
                }
                if !missing.is_empty() {
                    missing.sort_by_key(|(id, _)| *id);
                    let symbols: Vec<&str> = missing.iter().filter(|(_, symbol)| !symbol.is_empty()).map(|(_, symbol)| symbol.as_str()).collect();
                    let symbols = sentence(&symbols, locale);
                    self.add("allocations", i18n::text(locale, "errors.bots.multi_asset.pair_missing", &[("assets", i18n::Arg::Text(&symbols)), ("exchange_name", i18n::Arg::Text(&self.candidate.exchange.name))]));
                }
            }
        }
        let pending = self.candidate.transient.get("rebalance_pending").is_some_and(present);
        if context == ValidationContext::Update {
            if pending && exchange_changed { self.add("exchange", i18n::text(locale, "errors.bots.multi_asset.locked_while_running", &[])); }
            if (self.original.working() || pending) && (self.composition_changed() || exchange_changed) { self.add("allocations", i18n::text(locale, "errors.bots.multi_asset.locked_while_running", &[])); }
        }
        Ok(())
    }

    pub fn check(&self) -> start::Check {
        let mut check = start::Check { invalid: !self.errors.is_empty(), errors: self.errors.clone(),
            rejected: self.parse_errors.iter().filter_map(|e| self.submitted.get(&e.field).map(|v| (e.field.clone(), v.clone()))).collect(), ..Default::default() };
        for error in &self.errors {
            match error.field.as_str() {
                "smart_interval_quote_amount" if check.smart_interval_quote_amount.is_none() => check.smart_interval_quote_amount = Some(error.message.clone()),
                "start_at" => check.start_at = Some(if self.candidate.text("start_at").and_then(|s| DateTime::parse_from_rfc3339(s).ok()).is_some() { "must_be_future" } else { "blank" }),
                "start_time_mode" => check.start_time_mode = true, "start_time_of_day" => check.start_time_of_day = true, _ => {},
            }
        }
        check
    }

    pub fn save_effects(&self, c: &Connection, now: DateTime<Utc>) -> Result<SaveEffects, WebError> {
        let mut settings = self.candidate.settings.clone();
        let mut transient = self.candidate.transient.clone();
        if self.candidate.kind == Kind::Basket && !equal(&json!(settings), &json!(self.raw_settings)) {
            for prefix in ["quote_amount_limit", "base_amount_limit"].into_iter().chain(trigger_prefixes()) {
                let key = format!("{prefix}ed");
                let current = self.read(&key);
                if self.raw_settings.get(&key).unwrap_or(&Value::Null) != &current {
                    callback_set(&mut transient, format!("{prefix}_enabled_at"), if current == json!(true) { json!(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)) } else { Value::Null });
                    if prefix != "quote_amount_limit" && prefix != "base_amount_limit" { callback_set(&mut transient, format!("{prefix}_condition_met_at"), Value::Null); }
                }
            }
            for prefix in ["price_limit", "sell_price_limit"] {
                let timing = format!("{prefix}_timing_condition");
                let condition = format!("{prefix}_value_condition");
                if self.raw_settings.get(&timing) != Some(&self.read(&timing)) && self.read(&timing) != "while" && self.read(&condition) == "between" { callback_set(&mut settings, condition, json!("above")); }
            }
        }
        if self.candidate.kind == Kind::Basket && self.candidate.exchange.id != self.original.exchange.id {
            let first = self.candidate.tickers.iter().min_by(|a, b| a.base.as_bytes().cmp(b.base.as_bytes())).map(|t| t.id);
            for prefix in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
                let key = format!("{prefix}_in_ticker_id");
                let sell_key = format!("sell_{key}");
                let old_subject = |key: &str| -> Result<Option<Ticker>, WebError> {
                    let Some(id) = self.raw_settings.get(key).filter(|v| present(v)) else { return Ok(None) };
                    let id = numeric(id, true)?.as_i64().unwrap_or(0);
                    Ok(Ticker::all(c, "t.id = ?1", &[&id])?.into_iter().next())
                };
                let mapped = |ticker: &Ticker| self.candidate.tickers.iter().find(|t| t.base_asset_id == ticker.base_asset_id && t.quote_asset_id == ticker.quote_asset_id).map(|t| t.id);
                let (buy, sell) = if let Some(old) = old_subject(&key)? {
                    (mapped(&old), old_subject(&sell_key)?.as_ref().map_or(first, mapped))
                } else { (first, first) };
                callback_set(&mut settings, key, json!(buy));
                callback_set(&mut settings, sell_key, json!(sell));
            }
        }
        Ok(SaveEffects { settings: changes(&self.raw_settings, &settings), transient: changes(&self.raw_transient, &transient),
            settings_changed: !equal(&Value::Object(self.candidate.settings.clone()), &Value::Object(self.baseline.clone())), composition_changed: self.composition_changed() })
    }
}

fn trigger_prefixes() -> [&'static str; 8] {
    ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit", "sell_price_limit", "sell_price_drop_limit", "sell_moving_average_limit", "sell_indicator_limit"]
}
fn changes(before: &Map<String, Value>, after: &Map<String, Value>) -> JsonChanges {
    if equal(&json!(before), &json!(after)) { return JsonChanges::default(); }
    JsonChanges { set: after.iter().filter(|(key, value)| before.get(*key) != Some(*value)).map(|(key, value)| (key.clone(), value.clone())).collect(),
        remove: before.keys().filter(|key| !after.contains_key(*key)).cloned().collect() }
}

// Ruby Hash equality compares Numbers by value, but distinguishes an absent key from JSON null.
fn equal_option(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) { (Some(a), Some(b)) => equal(a, b), (None, None) => true, _ => false }
}
pub(super) fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => a.len() == b.len() && a.iter().all(|(key, value)| equal_option(Some(value), b.get(key))),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b)),
        (Value::Number(a), Value::Number(b)) if a.is_i64() != b.is_i64() => {
            let pair = a.as_i64().zip(b.as_f64()).or_else(|| b.as_i64().zip(a.as_f64()));
            pair.is_some_and(|(i, f)| f.fract() == 0.0 && f >= i64::MIN as f64 && f < -(i64::MIN as f64) && f as i64 == i)
        }
        _ => a == b,
    }
}

// ActiveRecord store_accessor writers do not materialize an absent key when assigned nil.
// Direct settings.merge from parse_params does: keep those two operations distinct.
fn callback_set(values: &mut Map<String, Value>, key: String, value: Value) {
    if !equal(values.get(&key).unwrap_or(&Value::Null), &value) { values.insert(key, value); }
}

impl Draft {
    /// Startable#initial_start_at. Day arithmetic is bounded and follows local calendar days;
    /// Rails chooses the DST occurrence on overlaps and advances gaps by one hour.
    pub fn initial_start_at(&self, now: DateTime<Utc>, name: &str) -> Result<Option<DateTime<Utc>>, WebError> {
        if !self.candidate.start_time_enabled() { return Ok(None); }
        let mode=self.candidate.text("start_time_mode").ok_or_else(||failure("missing start mode"))?;
        if mode=="date" {
            return self.candidate.text("start_at").and_then(|s|DateTime::parse_from_rfc3339(s).ok()).map(|t|Some(t.with_timezone(&Utc))).ok_or_else(||failure("invalid start date"));
        }
        let zone=timezone::zone(name).unwrap_or(chrono_tz::UTC);
        let local=now.with_timezone(&zone);
        let (hour,minute)=self.candidate.text("start_time_of_day").and_then(|s|s.split_once(':')).ok_or_else(||failure("invalid start hour"))?;
        let hour=hour.parse::<u32>().map_err(|_|failure("invalid start hour"))?;
        let minute=minute.parse::<u32>().map_err(|_|failure("invalid start minute"))?;
        let mut day=local.date_naive();
        let step=if mode=="hour" {1} else {
            let weekday=start::MODES.iter().position(|s|*s==mode).filter(|n|*n<7).ok_or_else(||failure("invalid start weekday"))?;
            let days=(weekday as i64-i64::from(local.weekday().num_days_from_monday())).rem_euclid(7);
            day=day.checked_add_signed(Duration::days(days)).ok_or_else(||failure("start date overflow"))?;
            7
        };
        let resolve=|day:NaiveDate| -> Result<DateTime<Utc>,WebError> {
            let naive=day.and_hms_opt(hour,minute,0).ok_or_else(||failure("invalid start time"))?;
            let at=match zone.from_local_datetime(&naive) {
                LocalResult::Single(at)=>Some(at), LocalResult::Ambiguous(a,b)=>Some(a.min(b)),
                LocalResult::None=>naive.checked_add_signed(Duration::hours(1)).and_then(|n|zone.from_local_datetime(&n).earliest()),
            }.ok_or_else(||failure("unresolvable start time"))?;
            Ok(at.with_timezone(&Utc))
        };
        let first=resolve(day)?;
        let future=if first<=now {
            // Rails adds fixed seconds after converting the candidate to UTC.
            first.checked_add_signed(Duration::days(step)).ok_or_else(||failure("start date overflow"))?
        } else {first};
        Ok(Some(future))
    }
}
