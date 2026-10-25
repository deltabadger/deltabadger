//! Venue::candles against Exchange#get_indicator_candles as Rails answers the same bodies (script/rust/record_candles.rb):
//! the candles it returns (CandleBuilder's built ones included), its Failure, or its raise, and every request in order.
//! Kraken first (slice 7b-1), then Binance, Binance.US and Alpaca (stocks and crypto); each venue adds its fixture here.
use deltabadger::engine::candles::{Candle, Timeframe};
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::binance::BinanceVenue;
use deltabadger::venue::http::{self, HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::{Venue, VenueError};
use serde_json::Value;

/// The scripted transport at the case's clock: Binance's walk ends against `timeframe.ago`.
#[derive(Clone)]
struct At(ScriptedTransport, chrono::DateTime<chrono::Utc>);
impl Transport for At {
    fn now(&self) -> chrono::DateTime<chrono::Utc> { self.1 }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.0.send(r).await }
    async fn send_typed(&self, r: &HttpRequest) -> Result<(HttpResponse, Option<String>), TransportError> { self.0.send_typed(r).await }
}

const VENUES: [(&str, usize); 4] = [("kraken", 98), ("binance", 80), ("binance_us", 10), ("alpaca", 106)];

fn fixture(venue: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{venue}_candles.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// D-COND-CANDLE-UNREADABLE: a price Rails' `to_d` reads as 0.0 (null, a word, blank) is unreadable here, and so is a
/// float time `Time.at` takes; Rails returns a candle, Rust refuses the answer. Every other case agrees exactly.
const UNREADABLE: [(&str, &str); 12] = [("kraken", "row_open_null"), ("kraken", "row_open_word"), ("kraken", "row_close_blank"), ("kraken", "row_time_float"),
    ("binance", "row_open_null"), ("binance", "row_open_word"), ("binance", "row_close_blank"), ("binance", "row_time_float"),
    ("binance_us", "row_open_null"), ("alpaca", "crypto_bar_open_null"), ("alpaca", "crypto_bar_open_word"), ("alpaca", "crypto_bar_close_blank")];
/// Spec E §2.10 (each adapter's `answer`): HTTP 429 (and Binance's 418) on a read is a throttle that names the venue's
/// rate-limit text; Rails fails it with the body.
const THROTTLED: [(&str, &str, &str); 4] = [("kraken", "http_429", "EAPI:Rate limit exceeded"), ("binance", "http_418", "rate limit exceeded"),
    ("binance", "http_429", "rate limit exceeded"), ("binance_us", "http_429", "rate limit exceeded")];

fn timeframe(name: &str) -> Timeframe {
    match name {
        "1m" => Timeframe::Minute, "5m" => Timeframe::FiveMinutes, "15m" => Timeframe::FifteenMinutes, "30m" => Timeframe::ThirtyMinutes,
        "1h" => Timeframe::Hour, "4h" => Timeframe::FourHours, "1d" => Timeframe::Day, "3d" => Timeframe::ThreeDays, "1w" => Timeframe::Week,
        "1M" => Timeframe::Month, other => panic!("timeframe {other}"),
    }
}

fn ticker(pair: &str) -> Ticker { ticker_of(pair, "XBT", true) }
fn ticker_of(pair: &str, base: &str, crypto: bool) -> Ticker {
    let zero = BigDec::zero();
    Ticker { id: 1, ticker: pair.into(), base_code: base.into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(),
             exchange_name: "Kraken".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 2, price_decimals: 1,
             minimum_base_size: zero.clone(), minimum_quote_size: zero, trading_enabled: true, available: true, crypto }
}

/// A candle as the recorder prints it: ISO8601 with microseconds, then each decimal's to_s('F').
fn row(c: &Candle) -> Value {
    serde_json::json!([c.at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(), c.open.to_s_f(), c.high.to_s_f(), c.low.to_s_f(), c.close.to_s_f(), c.volume.to_s_f()])
}

#[test]
fn the_fixtures_are_recorded_from_the_current_rails_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (venue, _) in VENUES {
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
    for (venue, count) in VENUES {
        let fixture = fixture(venue);
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), count, "{venue}");
        total += cases.len();
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let start_at = chrono::DateTime::parse_from_rfc3339(case["start_at"].as_str().unwrap()).unwrap().to_utc();
            let now = chrono::DateTime::parse_from_rfc3339(case["now"].as_str().unwrap()).unwrap().to_utc();
            let tf = timeframe(case["timeframe"].as_str().unwrap());
            let transport = ScriptedTransport::from_script(&case["http"]);
            let wire = At(transport.clone(), now);
            let got = match venue {
                "kraken" => KrakenVenue::new(wire).candles(&ticker("XXBTZEUR"), start_at, tf).await,
                "binance" => BinanceVenue::new(wire, deltabadger::venue::binance::URL, "").candles(&ticker("BTCUSDT"), start_at, tf).await,
                "binance_us" => BinanceVenue::new(wire, deltabadger::venue::binance::US_URL, "").with_rules(&deltabadger::engine::venue_rules::BINANCE_US)
                    .candles(&ticker("BTCUSDT"), start_at, tf).await,
                _ => {
                    let t = &case["ticker"];
                    let urls = Urls { trading: deltabadger::venue::alpaca::PAPER_TRADING_URL.into(), data: deltabadger::venue::alpaca::DATA_URL.into() };
                    AlpacaVenue::new(wire, urls).candles(&ticker_of(t["ticker"].as_str().unwrap(), t["base"].as_str().unwrap(), t["crypto"] == true), start_at, tf).await
                }
            };
            let requests: Vec<String> = transport.requests().iter().map(http::request_key).collect();
            if Value::from(requests.clone()) != case["requests"] { failures.push(format!("{venue} {name}: requested {requests:?}, Rails {}", case["requests"])); }
            let network = case["http"].as_object().unwrap().values().flat_map(|r| r.as_array().unwrap()).any(|r| r.get("network").is_some());
            let listed = |list: &[(&str, &str)]| list.contains(&(venue, name));
            let ok = match (&got, case.get("candles"), case.get("failure"), case.get("raised")) {
                (Err(VenueError::Ambiguous(_)), Some(_), _, _) if listed(&UNREADABLE) => { unreadable += 1; true }
                (Ok(candles), Some(rails), _, _) => Value::from(candles.iter().map(row).collect::<Vec<_>>()) == *rails,
                // honeymaker returns a transport failure as a Failure; the engine reads it as a transient read failure.
                // honeymaker returns it; Clients::Alpaca raises Client::TransientNetworkError.
                (Err(VenueError::Transient(_)), _, Some(_), _) => network,
                (Err(VenueError::Transient(_)), _, _, Some(raised)) => network && raised == "Client::TransientNetworkError",
                (Err(VenueError::Rejected(e)), _, Some(_), _) if THROTTLED.iter().any(|(v, n, _)| (*v, *n) == (venue, name)) => {
                    throttled += 1;
                    THROTTLED.iter().any(|(v, n, text)| (*v, *n) == (venue, name) && e[0].contains(text))
                }
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
    assert_eq!(on, ["Alpaca", "Kraken", "Binance", "Binance.US"]);
}

/// The default for a venue not yet ported: a refusal, never an empty series (which would read as "no candles").
#[tokio::test(flavor = "current_thread")]
async fn a_venue_without_candles_refuses() {
    let v = deltabadger::venue::gemini::GeminiVenue::new(ScriptedTransport::default(), deltabadger::venue::gemini::URL);
    let got = v.candles(&ticker("btceur"), chrono::Utc::now(), Timeframe::Day).await;
    assert!(matches!(got, Err(VenueError::Rejected(_))), "{got:?}");
}

/// D-COND-CANDLE-CAP: Rails walks Binance's pages, and follows Alpaca's tokens, without end; past MAX_CANDLE_PAGES pages
/// the read is a settled failure. (Rails cannot record it: the same answer every time never ends its walk.)
#[tokio::test(flavor = "current_thread")]
async fn an_endless_walk_is_a_settled_failure() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:01Z").unwrap().to_utc();
    let start = now - chrono::Duration::days(30);
    // Every page answers one old candle, so every next page starts a second later: each request differs.
    let binance = ScriptedTransport::default();
    binance.reply("GET /api/v3/klines", 200, serde_json::json!([[start.timestamp() * 1000, "1", "2", "0.5", "1.5", "1"]]));
    let got = BinanceVenue::new(At(binance.clone(), now), deltabadger::venue::binance::URL, "").candles(&ticker("BTCUSDT"), start, Timeframe::Day).await;
    assert!(matches!(&got, Err(VenueError::Rejected(e)) if e[0] == "Binance: more than 1000 pages of candles"), "{got:?}");
    assert_eq!(binance.requests().len(), 1000);
    let alpaca = ScriptedTransport::default();
    alpaca.reply("GET /v1beta3/crypto/us/bars", 200, serde_json::json!({ "bars": { "BTC/USD": [] }, "next_page_token": "again" }));
    let urls = Urls { trading: deltabadger::venue::alpaca::PAPER_TRADING_URL.into(), data: deltabadger::venue::alpaca::DATA_URL.into() };
    let got = AlpacaVenue::new(At(alpaca.clone(), now), urls).candles(&ticker_of("BTC/USD", "BTC", true), start, Timeframe::Day).await;
    assert!(matches!(&got, Err(VenueError::Rejected(e)) if e[0] == "Alpaca: more than 1000 pages of candles"), "{got:?}");
    assert_eq!(alpaca.requests().len(), 1000);
}

/// ActiveSupport's `1.month.ago`: a calendar month, the day clamped to the shorter month's last.
#[test]
fn a_month_ago_is_a_calendar_month() {
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().to_utc();
    assert_eq!(Timeframe::Month.ago(at("2026-03-31T10:00:00Z")), at("2026-02-28T10:00:00Z"));
    assert_eq!(Timeframe::Month.ago(at("2028-03-31T10:00:00Z")), at("2028-02-29T10:00:00Z"));
    assert_eq!(Timeframe::Week.ago(at("2026-09-02T10:00:01Z")), at("2026-08-26T10:00:01Z"));
}
