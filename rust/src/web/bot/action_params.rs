//! Bounded Rack parameters and the BotsController allowlist. The existing serde_json
//! preserve_order feature supplies the small scalar/object/array tree without another dependency.
use super::Kind;
use crate::web::{Params, FORM_FIELDS};
use axum::http::{HeaderMap, Method, StatusCode};
use serde_json::{Map, Value};

pub type Param = Value;
pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamError { Shape, Limit, Root }

impl ParamError {
    pub fn status(self) -> StatusCode { StatusCode::BAD_REQUEST }
}

#[derive(Clone, Debug)]
pub struct ActionParams(Param, bool);

impl ActionParams {
    pub fn parse(params: &Params) -> Result<Self, ParamError> {
        let mut query = form(&params.query)?;
        let body = match &params.json {
            Some(value) => { validate(value)?; value.clone() }
            None => form(&params.form)?,
        };
        merge(&mut query, body);
        validate(&query)?;
        Ok(Self(query, false))
    }

    pub fn from_mcp(value: Value) -> Result<Self, ParamError> {
        validate(&value)?;
        Ok(Self(value, true))
    }

    pub fn mcp(&self) -> bool { self.1 }

    pub fn value(&self) -> &Param { &self.0 }

    /// Rails' controller parameter merge: query keys replace body keys shallowly.
    /// Existing bot actions retain their separately approved body-winning merge.
    pub fn parse_rails(params: &Params) -> Result<Self, ParamError> {
        let mut parsed = Self::parse(params)?;
        let query = form(&params.query)?;
        let object = parsed.0.as_object_mut().ok_or(ParamError::Shape)?;
        if let Value::Object(query) = query { object.extend(query); }
        Ok(parsed)
    }

    pub fn permitted(&self, kind: Kind) -> Result<Param, ParamError> {
        if self.mcp() { return Ok(self.0.clone()); }
        let (root, allowed) = match kind {
            Kind::Basket => ("bots_dca_multi_asset", BASKET_SCALARS),
            Kind::Index => ("bots_dca_index", INDEX_SCALARS),
        };
        let fields = self.0.get(root).and_then(Value::as_object).filter(|v| !v.is_empty()).ok_or(ParamError::Root)?;
        let mut out = Map::new();
        for key in allowed {
            if let Some(value) = fields.get(*key).filter(|v| !v.is_array() && !v.is_object()) {
                out.insert((*key).into(), value.clone());
            }
        }
        if kind == Kind::Basket {
            if let Some(value) = fields.get("allocations").filter(|v| v.is_object()) {
                out.insert("allocations".into(), value.clone());
            }
        }
        Ok(Value::Object(out))
    }

    /// Only Start uses Ruby's scalar to_s conversion; typed settings remain typed.
    pub fn start_fresh(&self) -> Result<bool, StartFlagError> {
        match self.0.get("start_fresh") {
            None => start_fresh(None),
            Some(Value::String(s)) => start_fresh(Some(s)),
            Some(Value::Bool(true)) => start_fresh(Some("true")),
            Some(Value::Bool(false)) => start_fresh(Some("false")),
            Some(Value::Number(n)) => start_fresh(Some(&n.to_string())),
            Some(_) => Err(StartFlagError::Invalid),
        }
    }
}

// The plan explicitly requires a deep, body-winning merge. Rails 8.1's request merger
// is shallow and query-winning; action_transport pins that named divergence to real requests.
fn merge(query: &mut Param, body: Param) {
    match (query, body) {
        (Value::Object(q), Value::Object(b)) => {
            for (key, value) in b {
                if let Some(old) = q.get_mut(&key) { merge(old, value); }
                else { q.insert(key, value); }
            }
        }
        (q, b) => *q = b,
    }
}

/// Empty containers count too; a thousand empty objects cannot evade the work budget.
pub fn validate(value: &Param) -> Result<(), ParamError> {
    fn visit(value: &Param, depth: usize, leaves: &mut usize) -> Result<(), ParamError> {
        if depth > MAX_DEPTH { return Err(ParamError::Limit); }
        match value {
            Value::Object(v) if !v.is_empty() => for child in v.values() { visit(child, depth + 1, leaves)?; },
            Value::Array(v) if !v.is_empty() => for child in v { visit(child, depth + 1, leaves)?; },
            _ => { *leaves += 1; if *leaves > FORM_FIELDS { return Err(ParamError::Limit); } }
        }
        Ok(())
    }
    if !value.is_object() { return Err(ParamError::Shape); }
    visit(value, 0, &mut 0)
}

pub fn json(bytes: &[u8]) -> Result<Param, ParamError> {
    let value = serde_json::from_slice(bytes).map_err(|_| ParamError::Shape)?;
    validate(&value)?;
    Ok(value)
}

fn form(pairs: &[(String, String)]) -> Result<Param, ParamError> {
    if pairs.len() > FORM_FIELDS { return Err(ParamError::Limit); }
    let mut out = Value::Object(Map::new());
    for (name, value) in pairs { normalize(&mut out, name, value, 0)?; }
    validate(&out)?;
    Ok(out)
}

/// Rack::QueryParser#_normalize_params: a later scalar replaces any shape, while
/// extending an existing scalar or the wrong container raises ParameterTypeError.
fn normalize(params: &mut Param, name: &str, value: &str, depth: usize) -> Result<(), ParamError> {
    if depth >= MAX_DEPTH { return Err(ParamError::Limit); }
    let (key, after) = if depth == 0 {
        match name.char_indices().find(|(i, c)| *i > 0 && *c == '[') {
            Some((at, _)) => name.split_at(at), None => (name, ""),
        }
    } else if let Some(rest) = name.strip_prefix("[]") { ("[]", rest) }
    else if let Some((key, rest)) = name.strip_prefix('[').and_then(|s| s.split_once(']')) { (key, rest) }
    else { (name, "") };
    if key.is_empty() { return Ok(()); }
    if after.is_empty() && key == "[]" && depth != 0 {
        *params = Value::Array(vec![Value::String(value.into())]);
        return Ok(());
    }
    let object = params.as_object_mut().ok_or(ParamError::Shape)?;
    if after.is_empty() || after == "[" {
        object.insert(if after == "[" { name } else { key }.into(), Value::String(value.into()));
    } else if let Some(rest) = after.strip_prefix("[]") {
        let array = object.entry(key.to_string()).or_insert_with(|| Value::Array(Vec::new())).as_array_mut().ok_or(ParamError::Shape)?;
        if rest.is_empty() { array.push(Value::String(value.into())); }
        else {
            let child_key = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
                .filter(|s| !s.is_empty() && !s.contains(['[', ']'])).unwrap_or(rest);
            if let Some(last) = array.last_mut().filter(|v| v.is_object() && !has_path(v, child_key)) {
                normalize(last, child_key, value, depth + 1)?;
            } else {
                let mut child = Value::Object(Map::new());
                normalize(&mut child, child_key, value, depth + 1)?;
                array.push(child);
            }
        }
    } else {
        let child = object.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() { return Err(ParamError::Shape); }
        normalize(child, after, value, depth + 1)?;
    }
    Ok(())
}

fn has_path(mut value: &Param, key: &str) -> bool {
    if key.contains("[]") { return false; }
    for part in key.split(['[', ']']).filter(|s| !s.is_empty()) {
        let Some(next) = value.as_object().and_then(|v| v.get(part)) else { return false };
        value = next;
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Update, Start, Stop, Delete, Archive, Unarchive }

/// Locale has already been removed. Classification does not install mutation routes.
pub fn action(path: &str, method: &Method) -> Option<Action> {
    let path = path.strip_suffix(".turbo_stream").unwrap_or(path);
    let rest = path.strip_prefix("/bots/")?;
    let (id, suffix) = rest.split_once('/').unwrap_or((rest, ""));
    super::id_from_path(id)?;
    match (method.as_str(), suffix) {
        ("PATCH" | "PUT", "") => Some(Action::Update),
        ("PATCH" | "PUT", "start") => Some(Action::Start),
        ("PATCH" | "PUT", "stop") => Some(Action::Stop),
        ("DELETE", "delete") => Some(Action::Delete),
        ("POST", "archive") => Some(Action::Archive),
        ("DELETE", "archive") => Some(Action::Unarchive),
        _ => None,
    }
}

/// Resolve explicit HTML-only requests before any future mutation handler writes.
/// Delete/unarchive have Rails' explicit stream responses even for HTML Accept.
pub fn format_allowed(action: Action, path: &str, headers: &HeaderMap) -> bool {
    if matches!(action, Action::Delete | Action::Unarchive) || path.ends_with(".turbo_stream") { return true; }
    let accepted: Vec<&str> = headers.get_all("accept").iter().filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(',')).filter_map(|item| {
            let mut parts = item.trim().split(';');
            let media = parts.next()?.trim();
            (!parts.any(|p| p.trim().strip_prefix("q=").is_some_and(|q| matches!(q, "0" | "0.0" | "0.00" | "0.000")))).then_some(media)
        }).collect();
    !accepted.iter().any(|m| m.eq_ignore_ascii_case("text/html"))
        || accepted.iter().any(|m| m.eq_ignore_ascii_case("text/vnd.turbo-stream.html") || *m == "*/*" || *m == "text/*")
}

/// BotsController scalar allowlist, pinned to the generated Rails vector.
pub const BASKET_SCALARS: &[&str] = &[
    "quote_asset_id",
    "quote_amount",
    "interval",
    "weighting",
    "smart_intervaled",
    "smart_interval_quote_amount",
    "smart_interval_base_amount",
    "limit_ordered",
    "limit_order_pcnt_distance",
    "quote_amount_limited",
    "quote_amount_limit",
    "base_amount_limited",
    "base_amount_limit",
    "price_limited",
    "price_limit",
    "price_limit_range_lower_bound",
    "price_limit_range_upper_bound",
    "price_limit_timing_condition",
    "price_limit_value_condition",
    "price_limit_in_ticker_id",
    "price_limit_action",
    "sell_price_limited",
    "sell_price_limit",
    "sell_price_limit_range_lower_bound",
    "sell_price_limit_range_upper_bound",
    "sell_price_limit_timing_condition",
    "sell_price_limit_value_condition",
    "sell_price_limit_in_ticker_id",
    "sell_price_limit_action",
    "price_drop_limited",
    "price_drop_limit",
    "price_drop_limit_time_window_condition",
    "price_drop_limit_in_ticker_id",
    "price_drop_limit_action",
    "sell_price_drop_limited",
    "sell_price_drop_limit",
    "sell_price_drop_limit_time_window_condition",
    "sell_price_drop_limit_in_ticker_id",
    "sell_price_drop_limit_action",
    "moving_average_limited",
    "moving_average_limit_timing_condition",
    "moving_average_limit_value_condition",
    "moving_average_limit_in_ticker_id",
    "moving_average_limit_in_ma_type",
    "moving_average_limit_in_timeframe",
    "moving_average_limit_in_period",
    "moving_average_limit_action",
    "sell_moving_average_limited",
    "sell_moving_average_limit_timing_condition",
    "sell_moving_average_limit_value_condition",
    "sell_moving_average_limit_in_ticker_id",
    "sell_moving_average_limit_in_ma_type",
    "sell_moving_average_limit_in_timeframe",
    "sell_moving_average_limit_in_period",
    "sell_moving_average_limit_action",
    "indicator_limited",
    "indicator_limit",
    "indicator_limit_timing_condition",
    "indicator_limit_value_condition",
    "indicator_limit_in_ticker_id",
    "indicator_limit_in_indicator",
    "indicator_limit_in_timeframe",
    "indicator_limit_action",
    "sell_indicator_limited",
    "sell_indicator_limit",
    "sell_indicator_limit_timing_condition",
    "sell_indicator_limit_value_condition",
    "sell_indicator_limit_in_ticker_id",
    "sell_indicator_limit_in_indicator",
    "sell_indicator_limit_in_timeframe",
    "sell_indicator_limit_action",
    "start_time_enabled",
    "start_time_mode",
    "start_time_of_day",
    "start_at",
    "rebalance_enabled",
    "rebalance_threshold",
    "direction",
    "sell_amount",
    "sell_interval",
    "sell_denomination",
    "sell_quote_amount",
    "label",
    "exchange_id",
    "add_asset_id",
    "remove_asset_id",
    "normalize_allocations",
    "price_limit_mode",
    "price_drop_limit_mode",
    "moving_average_limit_mode",
    "indicator_limit_mode",
    "sell_price_limit_mode",
    "sell_price_drop_limit_mode",
    "sell_moving_average_limit_mode",
    "sell_indicator_limit_mode",
];

/// BotsController scalar allowlist, pinned to the generated Rails vector.
pub const INDEX_SCALARS: &[&str] = &[
    "quote_asset_id",
    "quote_amount",
    "interval",
    "num_coins",
    "allocation_flattening",
    "index_type",
    "index_category_id",
    "index_name",
    "index_name_prefix",
    "hold_all",
    "smart_intervaled",
    "smart_interval_quote_amount",
    "smart_interval_base_amount",
    "limit_ordered",
    "limit_order_pcnt_distance",
    "start_time_enabled",
    "start_time_mode",
    "start_time_of_day",
    "start_at",
    "rebalance_enabled",
    "rebalance_threshold",
    "label",
    "exchange_id",
    "num_coins_ceiling",
    "num_coins_rendered",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartFlagError { Invalid }

pub fn start_fresh(value: Option<&str>) -> Result<bool, StartFlagError> {
    match value {
        None => Ok(true),
        Some(s) if s.eq_ignore_ascii_case("true") || s == "1" => Ok(true),
        Some(s) if s.eq_ignore_ascii_case("false") || s == "0" => Ok(false),
        Some(_) => Err(StartFlagError::Invalid),
    }
}

#[cfg(test)]
mod flag_tests {
    use super::{start_fresh, StartFlagError};
    #[test]
    fn action_transport_start_flag() {
        for value in [None, Some("true"), Some("TRUE"), Some("1")] {
            assert_eq!(start_fresh(value), Ok(true));
        }
        for value in [Some("false"), Some("FALSE"), Some("0")] {
            assert_eq!(start_fresh(value), Ok(false));
        }
        for value in [Some(""), Some("yes"), Some(" true"), Some("true ")] {
            assert_eq!(start_fresh(value), Err(StartFlagError::Invalid));
        }
    }
}

/// Submitted numeric failures are 422 field errors, unlike malformed Rack transport (400).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericError { Shape, Bound }
impl NumericError {
    pub fn status(self) -> StatusCode { StatusCode::UNPROCESSABLE_ENTITY }
}
/// MRI String#to_f / #to_i consume a prefix, including digit-separated underscores. All
/// arithmetic stays finite and bounded; overflow is an explicit submitted-value error, never 0.
pub fn numeric(value: &Value, integer: bool) -> Result<Value, NumericError> {
    if value.is_boolean() || value.is_array() || value.is_object() { return Err(NumericError::Shape); }
    let input = value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
    if input.len() > crate::ruby::MAX_INPUT_LEN { return Err(NumericError::Bound); }
    let s = input.trim_start_matches([' ', '\t', '\n', '\r', '\u{000b}', '\u{000c}']);
    let mut chars = s.chars().peekable();
    let mut prefix = String::new();
    if let Some(sign @ ('+' | '-')) = chars.peek().copied() { prefix.push(sign); chars.next(); }
    let mut digits = 0;
    let mut previous_digit = false;
    let mut dot = false;
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() { prefix.push(ch); chars.next(); digits += 1; previous_digit = true; }
        else if ch == '_' && previous_digit {
            chars.next();
            if !chars.peek().is_some_and(|c| c.is_ascii_digit()) { break; }
            previous_digit = false;
        } else if ch == '.' && !integer && !dot {
            prefix.push(ch); chars.next(); dot = true; previous_digit = false;
        } else { break; }
    }
    if digits == 0 { return Ok(if integer { serde_json::json!(0) } else { serde_json::json!(0.0) }); }
    if !integer && chars.peek().is_some_and(|c| matches!(c, 'e' | 'E')) {
        chars.next();
        let mut exponent = String::from("e");
        if let Some(sign @ ('+' | '-')) = chars.peek().copied() { exponent.push(sign); chars.next(); }
        let mut count = 0;
        while let Some(ch) = chars.next() {
            if ch.is_ascii_digit() { exponent.push(ch); count += 1; }
            else if ch != '_' || count == 0 || !chars.peek().is_some_and(|c| c.is_ascii_digit()) { break; }
        }
        if count > 0 { prefix.push_str(&exponent); }
    }
    if integer {
        prefix.parse::<i64>().map(|n| serde_json::json!(n)).map_err(|_| NumericError::Bound)
    } else {
        let n = prefix.parse::<f64>().map_err(|_| NumericError::Shape)?;
        serde_json::Number::from_f64(n).map(Value::Number).ok_or(NumericError::Bound)
    }
}
