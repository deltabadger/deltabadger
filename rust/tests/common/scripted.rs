//! Alpaca over ScriptedTransport for engine tests: the seeded pairs BTC/USD, ETH/USD and SOL/USD.
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

pub const PRE_SEND: &str = "Faraday::ConnectionFailed: Connection refused - connect(2) for \"paper-api.alpaca.markets\" port 443";
pub const POST_SEND: &str = "Faraday::TimeoutError: Net::ReadTimeout";

pub fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
/// Alpaca's own not-found envelope: the only 404 that proves an order absent.
pub fn not_found() -> Value { json!({ "status": 404, "body": { "code": 40410000, "message": "order not found for 9b1d2c3e-0000-4000-8000-000000000001" } }) }
pub fn accepted(id: &str) -> Value { ok(json!({ "id": id, "status": "pending_new" })) }

/// Every pair's price in one body, as Alpaca answers `symbols=…`: each leg reads its own pair from it. Six POSTs accepted as
/// OTX-1..OTX-6 (the last repeats), a funded account, no positions. `over` replaces whole keys.
pub fn script(over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v1beta3/crypto/us/latest/quotes": [ok(json!({ "quotes": { "BTC/USD": { "ap": 64000 }, "ETH/USD": { "ap": 2500 }, "SOL/USD": { "ap": 150 } } }))],
        "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 }, "ETH/USD": { "p": 2500 }, "SOL/USD": { "p": 150 } } }))],
        "POST /v2/orders": [accepted("OTX-1"), accepted("OTX-2"), accepted("OTX-3"), accepted("OTX-4"), accepted("OTX-5"), accepted("OTX-6")],
        "GET /v2/account": [ok(json!({ "cash": "100000", "non_marginable_buying_power": "100000" }))],
        "GET /v2/positions": [ok(json!([]))],
    });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}

pub fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
