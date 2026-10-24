//! Venue::candles against Exchange#get_indicator_candles as Rails answers the same bodies (script/rust/record_candles.rb):
//! the candles it returns (CandleBuilder's built ones included), its Failure, or its raise, and every request in order.
//! Kraken first (slice 7b-1); each venue that follows adds its fixture here.
use deltabadger::engine::candles::{Candle, Timeframe};
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::http::{self, ScriptedTransport};
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::{Venue, VenueError};
use serde_json::Value;

fn fixture(venue: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{venue}_candles.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// D-COND-CANDLE-UNREADABLE: a price Rails' `to_d` reads as 0.0 (null, a word, blank) is unreadable here, and so is a
/// float time `Time.at` takes; Rails returns a candle, Rust refuses the answer. Every other case agrees exactly.
const UNREADABLE: [(&str, &str); 4] = [("kraken", "row_open_null"), ("kraken", "row_open_word"), ("kraken", "row_close_blank"), ("kraken", "row_time_float")];
/// Spec E §2.10 (kraken.rs `answer`): HTTP 429 on a read is a throttle that names Kraken's rate-limit text; Rails fails it
/// with the body.
const THROTTLED: [(&str, &str); 1] = [("kraken", "http_429")];

fn timeframe(name: &str) -> Timeframe {
    match name {
        "1m" => Timeframe::Minute, "5m" => Timeframe::FiveMinutes, "15m" => Timeframe::FifteenMinutes, "30m" => Timeframe::ThirtyMinutes,
        "1h" => Timeframe::Hour, "4h" => Timeframe::FourHours, "1d" => Timeframe::Day, "3d" => Timeframe::ThreeDays, "1w" => Timeframe::Week,
        "1M" => Timeframe::Month, other => panic!("timeframe {other}"),
    }
}

fn ticker(pair: &str) -> Ticker {
    let zero = BigDec::zero();
    Ticker { id: 1, ticker: pair.into(), base_code: "XBT".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(),
             exchange_name: "Kraken".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 2, price_decimals: 1,
             minimum_base_size: zero.clone(), minimum_quote_size: zero, trading_enabled: true, available: true, crypto: true }
}

/// A candle as the recorder prints it: ISO8601 with microseconds, then each decimal's to_s('F').
fn row(c: &Candle) -> Value {
    serde_json::json!([c.at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(), c.open.to_s_f(), c.high.to_s_f(), c.low.to_s_f(), c.close.to_s_f(), c.volume.to_s_f()])
}

#[test]
fn the_fixtures_are_recorded_from_the_current_rails_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for venue in ["kraken"] {
        for (file, recorded) in fixture(venue)["sources"].as_object().unwrap() {
            use sha2::Digest;
            let now = hex::encode(sha2::Sha256::digest(std::fs::read(root.join(file)).unwrap()));
            assert_eq!(recorded.as_str().unwrap(), now, "{file} changed: re-record with script/rust/repin");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn every_recorded_candle_read_is_answered_as_rails_answers_it() {
    let (mut failures, mut unreadable, mut throttled, mut total) = (vec![], 0, 0, 0);
    for (venue, count) in [("kraken", 98)] {
        let fixture = fixture(venue);
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), count, "{venue}");
        total += cases.len();
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let start_at = chrono::DateTime::parse_from_rfc3339(case["start_at"].as_str().unwrap()).unwrap().to_utc();
            let transport = ScriptedTransport::from_script(&case["http"]);
            let got = KrakenVenue::new(transport.clone()).candles(&ticker("XXBTZEUR"), start_at, timeframe(case["timeframe"].as_str().unwrap())).await;
            let requests: Vec<String> = transport.requests().iter().map(http::request_key).collect();
            if Value::from(requests.clone()) != case["requests"] { failures.push(format!("{venue} {name}: requested {requests:?}, Rails {}", case["requests"])); }
            let network = case["http"].as_object().unwrap().values().flat_map(|r| r.as_array().unwrap()).any(|r| r.get("network").is_some());
            let listed = |list: &[(&str, &str)]| list.contains(&(venue, name));
            let ok = match (&got, case.get("candles"), case.get("failure"), case.get("raised")) {
                (Err(VenueError::Ambiguous(_)), Some(_), _, _) if listed(&UNREADABLE) => { unreadable += 1; true }
                (Ok(candles), Some(rails), _, _) => Value::from(candles.iter().map(row).collect::<Vec<_>>()) == *rails,
                // honeymaker returns a transport failure as a Failure; the engine reads it as a transient read failure.
                (Err(VenueError::Transient(_)), _, Some(_), _) => network,
                (Err(VenueError::Rejected(e)), _, Some(_), _) if listed(&THROTTLED) => { throttled += 1; e[0].contains("EAPI:Rate limit exceeded") }
                (Err(e @ (VenueError::Rejected(_) | VenueError::Unauthorized(_))), _, Some(rails), _) => !network && Value::from(e.rejection().unwrap().to_vec()) == *rails,
                // Rails' KeyError keeps its text ("Key path error not found"); any other raise is the adapter's unreadable answer.
                (Err(VenueError::Ambiguous(m)), _, _, Some(raised)) => raised != "KeyError" || case["message"] == *m,
                _ => false,
            };
            if !ok { failures.push(format!("{venue} {name}: rust {got:?}\n  rails {}", Value::Object(case.as_object().unwrap().clone()))); }
        }
    }
    assert!(failures.is_empty(), "{} of {total} cases differ:\n{}", failures.len(), failures.join("\n"));
    assert_eq!((unreadable, throttled), (UNREADABLE.len(), THROTTLED.len()), "every listed divergence is still recorded");
}

/// Only the venues whose candles are recorded here say so to eligibility.
#[test]
fn condition_candles_are_on_only_where_recorded() {
    let on: Vec<&str> = deltabadger::venue::registry::ROWS.iter().filter(|r| r.condition_candles).map(|r| r.name).collect();
    assert_eq!(on, ["Kraken"]);
}

/// The default for a venue not yet ported: a refusal, never an empty series (which would read as "no candles").
#[tokio::test(flavor = "current_thread")]
async fn a_venue_without_candles_refuses() {
    let v = deltabadger::venue::gemini::GeminiVenue::new(ScriptedTransport::default(), deltabadger::venue::gemini::URL);
    let got = v.candles(&ticker("btceur"), chrono::Utc::now(), Timeframe::Day).await;
    assert!(matches!(got, Err(VenueError::Rejected(_))), "{got:?}");
}
