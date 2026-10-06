//! MCP response adapter; settings parsing is deliberately unavailable before Task 3.
use super::{draft::{Draft, FieldError}};
use crate::web::WebError;
use rusqlite::Connection;
use serde_json::Value;
pub enum InputOutcome { Parsed, AsciiAliasRefused }
pub fn apply(_: &Connection, _: &mut Draft, _: &Value) -> Result<InputOutcome,WebError> {
    Err(WebError::Config("MCP settings adapter is not registered".into()))
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
