//! BotApi input dialect. No SQL writes: the shared writer owns validation and persistence.
use super::{draft::{Draft, FieldError}, Kind};
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
fn invalid(setting: &str) -> String { format!("{setting} must be a number.") }
fn unsupported(setting: &str, kind: &str) -> String { format!("{setting} applies to {kind} bots only.") }
fn fail(draft: &mut Draft, message: String) { draft.errors.push(FieldError { field: "base".into(), message }); }

/// First service refusal wins, in quote/label/index/weights order. Model errors stay ordered.
pub enum InputOutcome { Parsed, AsciiAliasRefused }

pub fn apply(c: &Connection, draft: &mut Draft, fields: &Value) -> Result<InputOutcome, WebError> {
    if draft.original.working() {
        fail(draft, format!("Bot must be stopped before updating settings. Current status: {}.", draft.original.status.label()));
        return Ok(InputOutcome::Parsed);
    }
    let mut updates = Map::new();
    if present(&fields["quote_amount"]) {
        let Some(amount) = number(&fields["quote_amount"]).filter(|n| n > &BigDec::zero()) else { fail(draft, invalid("quote_amount")); return Ok(InputOutcome::Parsed); };
        updates.insert("quote_amount".into(), json!(amount.to_f()));
    }
    if present(&fields["label"]) { updates.insert("label".into(), fields["label"].clone()); }
    if present(&fields["num_coins"]) || present(&fields["allocation_flattening"]) {
        if draft.original.kind != Kind::Index { fail(draft,unsupported("num_coins / allocation_flattening","index")); return Ok(InputOutcome::Parsed); }
        if present(&fields["num_coins"]) {
            let v=&fields["num_coins"];
            let Some(n)=v.as_f64().filter(|n| n.fract()==0.0).and_then(|_|number(v)) else { fail(draft,invalid("num_coins")); return Ok(InputOutcome::Parsed); };
            let count=n.to_f() as i64;
            updates.insert("num_coins".into(),json!(count));
        }
        if present(&fields["allocation_flattening"]) {
            let Some(n)=in_range(&fields["allocation_flattening"],1) else { fail(draft,invalid("allocation_flattening")); return Ok(InputOutcome::Parsed); };
            updates.insert("allocation_flattening".into(),json!(n.to_f()));
        }
    }
    if present(&fields["allocations"]) {
        if draft.original.kind != Kind::Basket { fail(draft,unsupported("allocations","basket")); return Ok(InputOutcome::Parsed); }
        match weights(draft,&fields["allocations"]) {
            Ok(value) => { updates.insert("allocations".into(),value); },
            Err(message) => {
                // Classify the unsupported alias only; this cannot resolve an asset or
                // enable a write. NBSP and other Rails-invalid names retain Invalid.
                let alias=message.strip_suffix(" is not in this basket; membership cannot be changed here.")
                    .is_some_and(|name| unicode_ascii_alias(draft,name));
                fail(draft,message);
                return Ok(if alias {InputOutcome::AsciiAliasRefused} else {InputOutcome::Parsed});
            }
        }
    }
    if updates.is_empty() { fail(draft,"No settings provided to update.".into()); return Ok(InputOutcome::Parsed); }
    draft.submitted = updates.clone();
    for (key,value) in updates {
        if key=="label" {
            draft.candidate.label=value.as_str().ok_or_else(||super::data("MCP label has invalid shape".into()))?.to_owned();
        } else {
            if key=="allocations" { draft.candidate.settings.insert("weighting".into(),json!("manual")); }
            if key=="num_coins" { draft.candidate.settings.insert("hold_all".into(),json!(value.as_i64().is_some_and(|n|n>=draft.candidate.max_coins()))); }
            draft.candidate.settings.insert(key,value);
        }
    }
    draft.refresh(c)?;
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

fn weights(draft: &Draft, value: &Value) -> Result<Value,String> {
    let invalid = || "allocations must be 'SYMBOL:percent,…' or {symbol: percent}.".to_owned();
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
    let mut total=BigDec::zero();
    for (name,pct) in parsed {
        let mut ids:Vec<i64>=keys.iter().filter_map(|(id,key)|(key==&name).then_some(*id)).collect();
        if !name.is_empty() && name.bytes().all(|b|b.is_ascii_digit()) {
            if let Ok(id)=name.parse::<i64>() { if keys.iter().any(|(member,_)| *member==id) && !ids.contains(&id) {ids.push(id);} }
        }
        if ids.is_empty() { ids=draft.candidate.base_assets.iter().filter(|a|a.symbol.as_deref().is_some_and(|s|s.eq_ignore_ascii_case(&name))).map(|a|a.id).collect(); }
        if ids.is_empty() { return Err(format!("{name} is not in this basket; membership cannot be changed here.")); }
        if ids.len()>1 {
            let names:Vec<_>=ids.iter().filter_map(|id|keys.iter().find(|(member,_)|member==id).map(|(_,key)|key.as_str())).collect();
            return Err(format!("{name} names more than one basket asset: {}. Use one of those.",names.join(", ")));
        }
        let id=ids[0];
        if weights.contains_key(&id.to_string()) {
            let key=keys.iter().find(|(member,_)|*member==id).map(|(_,k)|k.as_str()).ok_or_else(invalid)?;
            return Err(format!("{key} is given twice."));
        }
        total=&total+&pct;
        weights.insert(id.to_string(),json!(pct.div(&BigDec::from_i64(100)).ok_or_else(invalid)?.to_f()));
    }
    let missing:Vec<_>=keys.iter().filter(|(id,_)|!weights.contains_key(&id.to_string())).map(|(_,key)|key.as_str()).collect();
    if !missing.is_empty() { return Err(format!("Give a weight for every basket asset; missing: {}.",missing.join(", "))); }
    let delta=&total-&BigDec::from_i64(100);
    let tolerance=BigDec::parse("0.1").map_err(|_|invalid())?;
    if delta>tolerance || delta< &BigDec::zero()-&tolerance { return Err("Weights must sum to 100.".into()); }
    Ok(Value::Object(weights))
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
