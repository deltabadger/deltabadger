//! The Binance trading adapter (venue::binance) over recorded bodies, its signer against honeymaker's own output, and its
//! answers mapped as Exchanges::Binance and Exchange#ambiguous_placement_error? map them. Rails-free: no install, no network.
use deltabadger::venue::binance::BinanceVenue;
use deltabadger::venue::http::ScriptedTransport;
use serde_json::Value;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/binance_vectors.json")).unwrap() }
fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for ch in snake.chars() { if ch == '_' { up = true } else if up { out.extend(ch.to_uppercase()); up = false } else { out.push(ch) } }
    out
}

// ---- signing: byte-identical to Honeymaker::Clients::Binance#sign_params under a frozen #timestamp_ms

#[test]
fn every_recorded_honeymaker_signed_query_is_reproduced() {
    let cases = vectors()["signing"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 32);
    for c in cases {
        let v = BinanceVenue::new(ScriptedTransport::default(), "https://api.binance.com", c["api_secret"].as_str().unwrap());
        let mut params: Vec<(&'static str, String)> = c["args"].as_object().unwrap().iter()
            .map(|(k, v)| (&*Box::leak(camel(k).into_boxed_str()), v.as_str().unwrap().to_string())).collect();
        if !params.iter().any(|(k, _)| *k == "recvWindow") { params.push(("recvWindow", "5000".into())); } // the gem's default
        let ms = c["timestamp"].as_i64().unwrap();
        let now = chrono::DateTime::from_timestamp_millis(ms).unwrap();
        let method = if c["method"] == "POST" { "POST" } else { "GET" };
        let r = v.signed(method, c["path"].as_str().unwrap(), params, now);
        assert_eq!((r.method, r.path.as_str()), (method, c["path"].as_str().unwrap()), "{c}");
        assert_eq!(r.url().query(), c["query"].as_str(), "{c}");
        assert!(r.body.is_none() && c["body"].is_null(), "honeymaker sends every param in the query, a POST too: {c}");
    }
}
