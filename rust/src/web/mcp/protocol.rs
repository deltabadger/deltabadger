//! The schemas recorded from actionmcp 0.201.0, and its json_schemer error wording.
use serde_json::{json, Value};
use std::sync::OnceLock;
pub fn metadata() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| { let mut data = recorded(); rule_kinds(&mut data); split_bots(&mut data); data })
}
/// D-SPLIT-API: the split_bots tool, this build's own (Rails has no such tool), after the recorded ones.
fn split_bots(data: &mut Value) {
    let ids = |description: &str| json!({"type": "array", "items": {"type": "integer"}, "description": description});
    let Some(tools) = data["tools"].as_array_mut() else { return };
    tools.push(json!({
        "name": "split_bots",
        "description": "Split basket or index bots: each becomes one stopped bot per asset it still trades on its exchange, buying that asset with its share of the amount at the same interval and keeping that asset's history. The split bots are deleted. If a bot has money from past sales waiting for an answer, the call is refused with code proceeds_unanswered and each such bot's offer: answer it with answer_redeploy_offer, or pass the bot's id in keep_proceeds_for to keep the money as it is.",
        "inputSchema": {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "bot_ids": ids("The IDs of the bots to split"),
                "keep_proceeds_for": ids("The IDs of the bots whose money from past sales stays as it is (No to the redeploy offer)")
            },
            "required": ["bot_ids"],
            "additionalProperties": false
        }
    }));
}
/// The schemas as recorded from Rails, before `rule_kinds`.
pub fn recorded() -> Value { serde_json::from_str(include_str!("metadata.json")).unwrap_or(Value::Null) }
/// D-MCP-COND-KINDS: update_bot_settings' rule fields say which bot kinds take them (conditions::support), where this build
/// runs a rule on an index or signal bot that Rails' does not have: the only change to the recorded schemas. A signal bot
/// watches its own pair (no subject field).
fn rule_kinds(data: &mut Value) {
    use crate::engine::conditions::{support, Support};
    use crate::web::bot::Kind;
    let Some(tool) = data["tools"].as_array_mut().and_then(|tools| tools.iter_mut().find(|t| t["name"] == "update_bot_settings")) else { return };
    let Some(fields) = tool["inputSchema"]["properties"].as_object_mut() else { return };
    for (name, schema) in fields.iter_mut() {
        let Some((rule, selling)) = crate::web::bot::draft::field_rule(name) else { continue };
        let index = support(Kind::Index, rule, selling, "pause") == Support::Implemented;
        let signal = support(Kind::Signal, rule, selling, "pause") == Support::Implemented && !name.ends_with("_in_ticker_id");
        if !index && !signal { continue; }
        let Some(text) = schema["description"].as_str() else { continue };
        let kinds = match (index, signal) { (true, true) => "Basket, index and signal bots", (true, false) => "Basket and index bots", _ => "Basket and signal bots" };
        let text = if name.ends_with("_in_ticker_id") {
            text.replace("Basket bots: the ticker id of the member it watches (get_bot_details lists them).",
                         "Basket and index bots: the ticker id of the member it watches (get_bot_details lists them); an index bot's may be a former member.")
        } else if name.ends_with("_mode") { format!("{text} {kinds}; flip is for basket bots only.") }
        else { format!("{text} {kinds}.") };
        schema["description"] = json!(text);
    }
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
