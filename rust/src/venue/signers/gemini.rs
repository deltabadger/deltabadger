//! Honeymaker::Clients::Gemini#post_signed: every private call is an empty-body POST whose parameters travel in
//! `X-GEMINI-PAYLOAD` = base64(JSON of the compacted params + `request` (the path) + `nonce` (ms, a string)), signed by
//! `X-GEMINI-SIGNATURE` = hex HMAC-SHA384(secret, that base64 text).
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::Sha384;

/// The nonce every Gemini request takes (`Nonce::next`): one per process, so per key too.
pub static NONCE: super::Nonce = super::Nonce::new();

/// The payload text as Rails encodes it: `Hash#compact` (a null param is left out; an empty array is kept), the params in
/// the caller's order, then `request` and `nonce`. ActiveSupport's `to_json` escapes `<`, `>`, `&`, U+2028 and U+2029,
/// which serde_json writes raw; they occur only inside strings, so rewriting the text is exact.
pub fn payload(path: &str, params: &[(&str, Value)], nonce: u64) -> String {
    let mut m = Map::new();
    for (k, v) in params.iter().filter(|(_, v)| !v.is_null()) { m.insert((*k).to_string(), v.clone()); }
    m.insert("request".into(), Value::from(path));
    m.insert("nonce".into(), Value::from(nonce.to_string()));
    let json = Value::Object(m).to_string();
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// The headers of one signed POST (its body is empty), in honeymaker's order.
pub fn headers(key: &str, secret: &str, path: &str, params: &[(&str, Value)], nonce: u64) -> Vec<(&'static str, String)> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload(path, params, nonce));
    let mut mac = Hmac::<Sha384>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(encoded.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    vec![("X-GEMINI-APIKEY", key.to_string()), ("X-GEMINI-PAYLOAD", encoded), ("X-GEMINI-SIGNATURE", signature),
         ("Accept", "application/json".into()), ("Content-Type", "text/plain".into())]
}
