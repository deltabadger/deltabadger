//! Venue::tickers_prices on Kraken, Binance and KuCoin against Exchange#get_tickers_prices(symbols:) as Rails answers the
//! same bodies (script/rust/record_tickers_prices.rb): the prices its hash holds, its Failure, or its raise, and every
//! request in order (Kraken's bulk read and its per-pair fallback).
use deltabadger::venue::binance::{BinanceVenue, Pairs};
use deltabadger::venue::http::{self, ScriptedTransport};
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::kucoin::KucoinVenue;
use deltabadger::venue::{Venue, VenueError};
use serde_json::{Map, Value};

fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/tickers_prices_vectors.json")).unwrap() }

/// The sanctioned divergence (plan CB Q6, ruby::json_to_d): a price string that is no finite decimal is left out, so the
/// member reads unpriced and nothing trades, where String#to_d reads "abc" as 0, "12x" as 12 and keeps "Infinity". Each
/// case names the code Rails priced and Rust leaves out; every other case agrees exactly.
const OMITTED: [(&str, &str, &str); 7] = [
    ("binance", "string_abc", "BTCUSDT"), ("binance", "string_12x", "BTCUSDT"), ("binance", "string_blank", "BTCUSDT"),
    ("binance", "string_infinity", "BTCUSDT"), ("binance", "string_nan", "BTCUSDT"),
    ("kucoin", "string_12x", "BTC-USDT"), ("kucoin", "string_infinity", "BTC-USDT"),
];

#[test]
fn the_fixture_is_recorded_from_the_current_rails_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (file, recorded) in fixture()["sources"].as_object().unwrap() {
        use sha2::Digest;
        let now = hex::encode(sha2::Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(recorded.as_str().unwrap(), now, "{file} changed: re-record with script/rust/repin");
    }
}

async fn answer(venue: &str, transport: ScriptedTransport, symbols: &[String]) -> Result<Vec<(String, Value)>, VenueError> {
    match venue {
        "kraken" => KrakenVenue::new(transport).tickers_prices(symbols).await,
        "binance" => BinanceVenue::new(transport, deltabadger::venue::binance::URL, "s").with_pairs(Pairs::default()).tickers_prices(symbols).await,
        _ => KucoinVenue::new(transport, deltabadger::venue::kucoin::URL).tickers_prices(symbols).await,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn every_recorded_bulk_price_read_is_answered_as_rails_answers_it() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 113);
    let mut failures = vec![];
    let mut diverged = vec![];
    for case in cases {
        let (venue, name) = (case["venue"].as_str().unwrap(), case["name"].as_str().unwrap());
        let symbols: Vec<String> = serde_json::from_value(case["symbols"].clone()).unwrap();
        let transport = ScriptedTransport::from_script(&case["http"]);
        let got = answer(venue, transport.clone(), &symbols).await;
        let requests: Vec<String> = transport.requests().iter().map(http::request_key).collect();
        if Value::from(requests.clone()) != case["requests"] { failures.push(format!("{venue} {name}: requested {requests:?}, Rails {}", case["requests"])); }
        let network = case["http"].as_object().unwrap().values().flat_map(|r| r.as_array().unwrap()).any(|r| r.get("network").is_some());
        let ok = match (&got, case.get("prices"), case.get("failure"), case.get("raised")) {
            (Ok(prices), Some(rails), _, _) => {
                let mut want = rails.as_object().unwrap().clone();
                for (_, _, code) in OMITTED.iter().filter(|(v, n, _)| *v == venue && *n == name) {
                    assert!(want.remove(*code).is_some(), "{venue} {name}: Rails no longer prices {code}; drop it from OMITTED");
                    diverged.push(name.to_string());
                }
                prices.iter().cloned().collect::<Map<String, Value>>() == want && prices.len() == want.len()
            }
            // honeymaker returns a transport failure as a Failure: the figures go stale (rebalance::value, transport_raises false).
            (Err(VenueError::Transient(_)), _, Some(_), _) => network,
            (Err(VenueError::Rejected(e)), _, Some(rails), _) => !network && Value::from(e.clone()) == *rails,
            // Rails' KeyError keeps its text ("Key path price not found"); another raise is the adapter's unreadable answer.
            (Err(VenueError::Ambiguous(m)), _, _, Some(_)) => case.get("message").is_none_or(|rails| rails == m),
            _ => false,
        };
        if !ok { failures.push(format!("{venue} {name}: rust {got:?}\n  rails {}", Value::Object(case.as_object().unwrap().clone()))); }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
    assert_eq!(diverged.len(), OMITTED.len(), "every listed divergence is still recorded");
}

/// The divergence spelt out: a Binance member whose price is "abc" or "12x" is unpriced (Rails values it at 0 or 12).
#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_string_price_leaves_the_member_unpriced() {
    for bad in ["abc", "12x"] {
        let t = ScriptedTransport::from_script(&serde_json::json!({ "GET /api/v3/ticker/price": [{ "status": 200, "body": [
            { "symbol": "BTCUSDT", "price": bad }, { "symbol": "ETHUSDT", "price": "2500.47" }] }] }));
        let got = answer("binance", t, &[]).await.unwrap();
        assert_eq!(got, vec![("ETHUSDT".to_string(), Value::from("2500.47"))], "{bad}");
    }
}

/// rebalance::value reads an `Ambiguous` bulk answer as Rails' unrescued raise. Alpaca's reader never answers one: an
/// unreadable or failed answer is `Rejected` (stale figures), a transport failure `Transient` (Clients::Alpaca raises it).
#[tokio::test(flavor = "current_thread")]
async fn alpacas_bulk_prices_never_answer_ambiguous() {
    use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
    let path = "GET /v1beta3/crypto/us/latest/trades";
    for reply in [serde_json::json!({ "status": 200, "body": "<html>" }), serde_json::json!({ "status": 200, "body": "[1,2" }),
                  serde_json::json!({ "status": 500, "body": "" }), serde_json::json!({ "status": 200, "body": "1e999" })] {
        let t = ScriptedTransport::from_script(&serde_json::json!({ path: [reply.clone()] }));
        let got = AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))).tickers_prices(&["BTC/USD".to_string()]).await;
        assert!(matches!(got, Err(VenueError::Rejected(_))), "{reply}: {got:?}");
    }
    let t = ScriptedTransport::from_script(&serde_json::json!({ path: [{ "network": "post_send", "message": "Net::ReadTimeout" }] }));
    let got = AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))).tickers_prices(&["BTC/USD".to_string()]).await;
    assert!(matches!(got, Err(VenueError::Transient(_))), "{got:?}");
}
