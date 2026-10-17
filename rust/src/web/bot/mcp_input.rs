//! BotApi input dialect. No SQL writes: the shared writer owns validation and persistence.
use super::{draft::{Draft, FieldError}, write::Failure, Kind};
use crate::{ruby::BigDec, web::WebError};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

/// Ruby String#strip removes NUL, TAB..CR and ASCII space, preserving Unicode spaces.
pub fn strip(text:&str)->&str {
    text.trim_matches(|c:char| matches!(c,'\0'|'\t'|'\n'|'\u{000b}'|'\u{000c}'|'\r'|' '))
}
pub fn present(v: &Value) -> bool {
    match v { Value::Null | Value::Bool(false) => false, Value::String(s) => !s.trim_matches(char::is_whitespace).is_empty(), Value::Object(o) => !o.is_empty(), Value::Array(a) => !a.is_empty(), _ => true }
}
pub fn number(v: &Value) -> Option<BigDec> {
    let text = match v {
        Value::Number(n) => {
            let value=n.as_f64()?;
            // Float#to_d retains negative zero, which the plain-number FORMAT rejects.
            if value.is_sign_negative() {return None;}
            match BigDec::from_f64(value).map(|d| d.to_s_f()) {Ok(s)=>s,Err(_)=>return None}
        },
        Value::String(s) => strip(s).to_owned(),
        _ => return None,
    };
    let mut parts = text.split('.');
    let whole = parts.next()?;
    if whole.is_empty() || whole.len() > 15 || !whole.bytes().all(|b| b.is_ascii_digit()) { return None; }
    if let Some(fraction) = parts.next() {
        if fraction.is_empty() || fraction.len() > 18 || !fraction.bytes().all(|b| b.is_ascii_digit()) { return None; }
    }
    if parts.next().is_some() { return None; }
    // A strict input refusal is a value, matching BotApi::Number.parse returning nil.
    BigDec::parse(&text).ok()
}
fn in_range(v: &Value, max: i64) -> Option<BigDec> {
    number(v).filter(|n| n >= &BigDec::zero() && n <= &BigDec::from_i64(max))
}
fn invalid(setting: &str) -> (&'static str, String) { ("invalid_number", format!("{setting} must be a number.")) }
fn unsupported(setting: &str, kind: &str) -> (&'static str, String) { ("unsupported_setting", format!("{setting} applies to {kind} bots only.")) }
/// A service refusal: the page's base error, and BotApi's failure (a validation one unless it says otherwise).
fn fail(draft: &mut Draft, (code, message): (&'static str, String)) -> InputOutcome {
    draft.errors.push(FieldError { field: "base".into(), message: message.clone() });
    let failure = if code == "bot_running" { Failure::conflict(code, message) } else { Failure::invalid(code, message) };
    InputOutcome::Refused(failure)
}

/// First service refusal wins, in catalogue order. Model errors stay ordered.
pub enum InputOutcome { Parsed, Refused(Failure), AsciiAliasRefused }

/// The settings catalogue in order (BotApi::Bots::SettingsFields, recorded in the tool's schema): the order
/// refusals are checked in and the order the answer lists what changed.
fn catalogue() -> Vec<(&'static str, &'static Value)> {
    super::super::mcp::protocol::metadata()["tools"].as_array().into_iter().flatten()
        .find(|t| t["name"] == "update_bot_settings")
        .and_then(|t| t["inputSchema"]["properties"].as_object())
        .map_or_else(Vec::new, |p| p.iter().filter(|(k, _)| k.as_str() != "bot_id").map(|(k, v)| (k.as_str(), v)).collect())
}

/// BotApi::Bots::SettingsFields::NAMES.
pub fn setting_names() -> Vec<&'static str> { catalogue().into_iter().map(|(name, _)| name).collect() }

fn blank(v: &Value) -> bool { v.is_null() || v.as_str().is_some_and(|s| strip(s).is_empty()) }

/// The automation dialect, turned into exactly what the settings page would post; the page's own parser
/// (`Draft::parse`) then reads it. No setting is written here.
pub fn apply(c: &Connection, draft: &mut Draft, fields: &Value, zone: &str, now: chrono::DateTime<chrono::Utc>) -> Result<InputOutcome, WebError> {
    if draft.original.working() {
        return Ok(fail(draft, ("bot_running", format!("Bot must be stopped before updating settings. Current status: {}.", draft.original.status.label()))));
    }
    let kind = draft.original.kind;
    let mut form = Map::new();
    for (name, schema) in catalogue() {
        let Some(value) = fields.get(name) else { continue };
        if blank(value) {
            // The page posts a blank for a field it cleared; other fields ignore one.
            if schema["type"].as_array().is_some_and(|t| t.contains(&json!("null"))) { form.insert(name.into(), Value::Null); }
            continue;
        }
        let index_only = matches!(name, "num_coins" | "allocation_flattening");
        if index_only && kind != Kind::Index { return Ok(fail(draft, unsupported("num_coins / allocation_flattening", "index"))); }
        let shared = index_only || super::draft::form_field(Kind::Index, name) || matches!(name, "label" | "exchange_id");
        if !shared && kind != Kind::Basket { return Ok(fail(draft, unsupported(name, "basket"))); }
        let posted = match name {
            "label" => value.clone(),
            "allocations" => match weights(draft, value) {
                Ok(posted) => posted,
                Err((code, message)) => {
                    // Classify the unsupported alias only; this cannot resolve an asset or
                    // enable a write. NBSP and other Rails-invalid names retain Invalid.
                    let alias = message.strip_suffix(" is not in this basket; add it with add_asset_id first.")
                        .is_some_and(|name| unicode_ascii_alias(draft, name));
                    let refused = fail(draft, (code, message));
                    return Ok(if alias { InputOutcome::AsciiAliasRefused } else { refused });
                }
            },
            "quote_amount" => match number(value).filter(|n| n > &BigDec::zero()) {
                Some(n) => json!(n.to_s_f()),
                None => return Ok(fail(draft, invalid(name))),
            },
            "allocation_flattening" => match in_range(value, 1) {
                Some(n) => json!(n.to_s_f()),
                None => return Ok(fail(draft, invalid(name))),
            },
            _ => match schema["type"].as_str().or_else(|| schema["type"][0].as_str()) {
                // A page switch posts "1" or "0"; a bare false would be blank to the page's parser. Anything else (REST's
                // text) is posted as its `to_s`.
                Some("boolean") => json!(match value { Value::Bool(true) => "1".into(), Value::Bool(false) => "0".into(), other => ruby_s(other) }),
                Some("number") if name.ends_with("_id") || name.ends_with("_period") || name == "num_coins" => {
                    // BotApi::Number.integer: a whole number however it arrived; the Float itself is judged first.
                    let whole = value.as_f64().is_none_or(|n| n.fract() == 0.0);
                    match number(value).filter(|n| whole && n.to_s_f().ends_with(".0")) {
                        Some(n) => json!(n.to_s_f().trim_end_matches(".0")),
                        None => return Ok(fail(draft, invalid(name))),
                    }
                },
                Some("number") => match number(value) {
                    Some(n) => json!(n.to_s_f()),
                    None => return Ok(fail(draft, invalid(name))),
                },
                // `value.to_s`.
                _ => json!(ruby_s(value)),
            },
        };
        form.insert(name.into(), posted);
    }
    if form.is_empty() { return Ok(fail(draft, ("no_updates_provided", "No settings provided to update.".into()))); }
    let accepted: Map<String, Value> = form.keys().map(|k| (k.clone(), Value::Null)).collect();
    draft.callback_zone = Some(zone.to_owned());
    match draft.parse(c, &Value::Object(form), zone, now) {
        Ok(()) | Err(super::draft::ParseError::Invalid(_)) => {},
        Err(super::draft::ParseError::Database(e)) => return Err(e),
    }
    // The answer lists the accepted settings, in catalogue order, never a transport argument.
    draft.submitted = accepted;
    Ok(InputOutcome::Parsed)
}

/// Unicode aliases of an ASCII symbol affect refusal classification, never lookup.
/// Default case folding has no ASCII alias for dotless i. Lower/upper handles the
/// long s, Kelvin sign and expanded ligatures without allowing an order or edit.
fn unicode_ascii_alias(draft:&Draft,name:&str)->bool {
    if name.is_ascii() || name.contains('\u{0131}') {return false;}
    let alias=name.to_lowercase().to_uppercase();
    draft.candidate.base_assets.iter().any(|asset|asset.symbol.as_deref()
        .is_some_and(|symbol|symbol.is_ascii() && symbol.eq_ignore_ascii_case(&alias)))
}

/// `value.to_s` for a scalar parameter.
fn ruby_s(value: &Value) -> String { crate::web::mcp::tools::str_value(value) }

fn weights(draft: &Draft, value: &Value) -> Result<Value,(&'static str,String)> {
    let invalid = || ("invalid_allocations", "allocations must be 'SYMBOL:percent,…' or {symbol: percent}.".to_owned());
    let mut given=Vec::new();
    match value {
        Value::String(s) => {
            // Ruby split drops trailing empty entries.
            for entry in s.trim_end_matches(',').split(',') {
                let Some((name,pct))=strip(entry).split_once(':') else { return Err(invalid()); };
                given.push((name.to_owned(),json!(pct)));
            }
        },
        Value::Object(o) => for (key,value) in o { given.push((key.clone(),value.clone())); },
        _ => return Err(invalid()),
    }
    if given.is_empty() || given.len()>1000 { return Err(invalid()); }
    let mut identifiers=std::collections::HashSet::new();
    let mut parsed=Vec::new();
    for (name,pct) in given {
        if name.trim_matches(char::is_whitespace).is_empty() { return Err(invalid()); }
        let Some(pct)=in_range(&pct,100) else { return Err(invalid()); };
        let name=strip(&name).to_owned();
        if !identifiers.insert(name.to_uppercase()) { return Err(invalid()); }
        parsed.push((name,pct));
    }
    let candidates:Vec<_>=draft.candidate.base_assets.iter().map(|a|(crate::figures::keys::Identity::Asset(a.id),crate::figures::keys::candidate(a.id,a.symbol.as_deref(),a.name.as_deref()))).collect();
    let keys:Vec<_>=crate::figures::keys::call(&candidates).map_err(|_|invalid())?.into_iter().filter_map(|(id,key)|match id{crate::figures::keys::Identity::Asset(id)=>Some((id,key)),_=>None}).collect();
    let mut weights=Map::new();
    for (name,pct) in parsed {
        let mut ids:Vec<i64>=keys.iter().filter_map(|(id,key)|(key==&name).then_some(*id)).collect();
        if !name.is_empty() && name.bytes().all(|b|b.is_ascii_digit()) {
            if let Ok(id)=name.parse::<i64>() { if keys.iter().any(|(member,_)| *member==id) && !ids.contains(&id) {ids.push(id);} }
        }
        if ids.is_empty() { ids=draft.candidate.base_assets.iter().filter(|a|a.symbol.as_deref().is_some_and(|s|s.eq_ignore_ascii_case(&name))).map(|a|a.id).collect(); }
        if ids.is_empty() { return Err(("asset_not_in_basket", format!("{name} is not in this basket; add it with add_asset_id first."))); }
        if ids.len()>1 {
            let names:Vec<_>=ids.iter().filter_map(|id|keys.iter().find(|(member,_)|member==id).map(|(_,key)|key.as_str())).collect();
            return Err(("ambiguous_basket_asset", format!("{name} names more than one basket asset: {}. Use one of those.",names.join(", "))));
        }
        let id=ids[0];
        if weights.contains_key(&id.to_string()) {
            let key=keys.iter().find(|(member,_)|*member==id).map(|(_,k)|k.as_str()).ok_or_else(invalid)?;
            return Err(("invalid_allocations", format!("{key} is given twice.")));
        }
        weights.insert(id.to_string(),json!(pct.to_s_f()));
    }
    // The page posts every member's slider: the named ones at their new weight, the rest at the weight the
    // slider shows (rounded to 0.1). Balance is judged at start, as on the page.
    let stored=match draft.raw_settings.get("allocations") {Some(Value::Object(stored))=>stored.clone(),_=>Map::new()};
    let mut posted=Map::new();
    for (id,value) in stored {
        let slider=match weights.get(&id) {
            Some(pct)=>pct.clone(),
            None=>{
                let weight=match &value {Value::String(s)=>strip(s).parse::<f64>().unwrap_or(0.0),other=>other.as_f64().unwrap_or(0.0)};
                json!(crate::web::format::Num::Float(weight*100.0).round(1).to_f())
            },
        };
        posted.insert(id,slider);
    }
    Ok(Value::Object(posted))
}

pub fn full_messages(errors: &[FieldError]) -> String {
    errors.iter().map(|e| {
        if e.field=="base" { return e.message.clone(); }
        let name=e.field.replace('_'," ");
        let mut chars=name.chars();
        let title=match chars.next() {Some(c)=>c.to_uppercase().collect::<String>()+chars.as_str(),None=>String::new()};
        format!("{title} {}",e.message)
    }).collect::<Vec<_>>().join(", ")
}

/// Reuse the existing Rails/Oj encoder for persisted MCP settings and JSON-RPC history.
pub fn encode(value: &Value) -> String {
    match value {
        Value::Null=>"null".into(), Value::Bool(b)=>b.to_string(),
        Value::String(s)=>crate::figures::json::J::Str(s.clone()).write(),
        Value::Number(n)=>{
            // Preserve both signed and unsigned JSON integers exactly. With this crate's
            // features a Number is an integer or a finite f64; the lexical branch also
            // remains lossless if serde adds another representation.
            if n.is_i64() || n.is_u64() {n.to_string()}
            else {match n.as_f64() {Some(f)=>crate::figures::num::oj_float(f),None=>n.to_string()}}
        },
        Value::Array(a)=>format!("[{}]",a.iter().map(encode).collect::<Vec<_>>().join(",")),
        Value::Object(o)=>format!("{{{}}}",o.iter().map(|(k,v)|format!("{}:{}",crate::figures::json::J::Str(k.clone()).write(),encode(v))).collect::<Vec<_>>().join(",")),
    }
}
