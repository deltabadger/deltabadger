//! The schemas recorded from actionmcp 0.201.0, and its json_schemer error wording.
use serde_json::{json, Value};
use std::sync::OnceLock;
pub fn metadata() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("metadata.json")).unwrap_or(Value::Null))
}
fn matches_type(value: &Value, kind: &str) -> bool {
    match kind { "object" => value.is_object(), "array" => value.is_array(), "string" => value.is_string(), "number" => value.is_number(), "integer" => value.as_f64().is_some_and(|v| v.fract() == 0.0), "boolean" => value.is_boolean(), "null" => value.is_null(), _ => false }
}
pub fn validate(value: &Value, schema: &Value, path: &str) -> Vec<String> {
    let mut errors = vec![];
    if schema == &Value::Bool(false) { return vec![format!("object property at `{path}` is a disallowed additional property")]; }
    if let Some(kind) = schema.get("type") {
        let types: Vec<&str> = kind.as_str().map(|s| vec![s]).unwrap_or_else(|| kind.as_array().into_iter().flatten().filter_map(Value::as_str).collect());
        if !types.iter().any(|k| matches_type(value,k)) {
            errors.push(format!("value at {} is not {}", if path.is_empty(){"root".to_string()}else{format!("`{path}`")}, if types.len() == 1 { format!("a{} {}", if ["object","array","integer"].contains(&types[0]) {"n"} else {""},types[0]) } else {format!("one of the types: [{}]",types.iter().map(|s|format!("\"{s}\"")).collect::<Vec<_>>().join(", "))}));
        }
    }
    if schema["format"]=="uri" && value.as_str().is_some_and(|v| crate::web::oauth::Uri::parse(v).is_none_or(|u|u.scheme.is_none())) {errors.push(format!("value at `{path}` does not match format: uri"));}
    if let Some(variants) = schema["anyOf"].as_array() {
        let all: Vec<_> = variants.iter().map(|s| validate(value,s,path)).collect();
        if all.iter().all(|e| !e.is_empty()) { errors.extend(all.into_iter().flatten()); }
    }
    if let Some(fixed) = schema.get("const") { if fixed != value { errors.push(format!("value at `{path}` is not: {fixed}")); } }
    if let Some(variants) = schema["enum"].as_array() { if !variants.contains(value) { errors.push(format!("value at `{path}` is not one of: [{}]", variants.iter().map(Value::to_string).collect::<Vec<_>>().join(", "))); } }
    if let Some(items) = value.as_array() { if let Some(s) = schema.get("items") { for (i,v) in items.iter().enumerate() { errors.extend(validate(v,s,&format!("{path}/{i}"))); } } }
    if let Some(object) = value.as_object() {
        for (key,s) in schema["properties"].as_object().into_iter().flatten() {
            if let Some(v)=object.get(key) {let p=format!("{path}/{}",key.replace('~',"~0").replace('/',"~1")); errors.extend(validate(v,s,&p));}
        }
        for (key,v) in object {
            if schema["properties"].get(key).is_none() {
                let p=format!("{path}/{}",key.replace('~',"~0").replace('/',"~1"));
                if let Some(s)=schema.get("additionalProperties") {errors.extend(validate(v,s,&p));}
            }
        }
    }
    if let Some(required) = schema["required"].as_array().filter(|_| value.is_object()) {
        let missing: Vec<_> = required.iter().filter_map(Value::as_str).filter(|k| value.get(k).is_none()).collect();
        if !missing.is_empty() { errors.push(format!("object at {} is missing required properties: {}", if path.is_empty(){"root".to_string()}else{format!("`{path}`")},missing.join(", "))); }
    }
    let mut unique = vec![];
    for error in errors { if !unique.contains(&error) {unique.push(error);} }
    unique
}
pub fn valid_envelope(v: &Value) -> bool {
    if !v.is_object() || v["jsonrpc"] != "2.0" {return false;}
    let valid_id = |id: &Value| id.is_string() || id.as_f64().is_some_and(|v| v.fract() == 0.0);
    if v.get("id").is_some_and(|id| !valid_id(id)) {return false;}
    if v.get("method").is_some() {
        v["method"].is_string() && v.get("params").is_none_or(Value::is_object) && v.get("result").is_none() && v.get("error").is_none()
    } else if v.get("id").is_some() && v.get("params").is_none() {
        (v["result"].is_object() && v.get("error").is_none()) || (v.get("result").is_none() && v["error"]["code"].is_i64() && v["error"]["message"].is_string())
    } else { false }
}
pub fn params_error(v: &Value) -> Option<(i64,String)> {
    let method = v["method"].as_str()?;
    let notification = v.get("id").is_none();
    let family = if notification {"notifications"} else {"requests"};
    let Some(schema) = metadata()[family].get(method) else {
        return notification.then(||(-32601,format!("Unsupported MCP notification method: {method}")));
    };
    let Some(params) = v.get("params") else {
        return metadata()[format!("required_{family}")].as_array().is_some_and(|a| a.contains(&json!(method)))
            .then(||(-32602,format!("Invalid params for {method}: params are required")));
    };
    let errors = validate(params,schema,"");
    (!errors.is_empty()).then(||(-32602,format!("Invalid params for {method}: {}",errors.join(", "))))
}
pub fn error(id: &Value, code: i64, message: &str) -> Value {json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})}
pub fn result(id: &Value, result: Value) -> Value {json!({"jsonrpc":"2.0","id":id,"result":result})}
pub fn tool_text(text: &str, failed: bool) -> Value {
    if failed {json!({"isError":true,"content":[{"type":"text","text":text}]})} else {json!({"content":[{"type":"text","text":text}]})}
}
