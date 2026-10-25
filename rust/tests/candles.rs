//! Venue::candles against Exchange#get_indicator_candles as Rails answers the same bodies (script/rust/record_candles.rb):
//! the candles it returns (CandleBuilder's built ones included), its Failure, or its raise, and every request in order.
//! Kraken first (slice 7b-1), then Binance, Binance.US and Alpaca (stocks and crypto), then every other venue that trades.
use deltabadger::engine::candles::{Candle, Timeframe};
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::binance::BinanceVenue;
use deltabadger::venue::http::{self, HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::{bingx, bitget, bitrue, bitvavo, bybit, coinbase, gemini, hyperliquid, kucoin, mexc};
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

const VENUES: &[(&str, usize)] = &[("kraken", 98), ("binance", 80), ("binance_us", 10), ("alpaca", 106), ("kucoin", 79), ("bitget", 74), ("bybit", 76),
    ("mexc", 75), ("bitrue", 76), ("bitvavo", 75), ("gemini", 73), ("hyperliquid", 70), ("coinbase", 74), ("bingx", 69)];

fn fixture(venue: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{venue}_candles.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// D-COND-CANDLE-UNREADABLE: a price Rails' `to_d` reads as 0.0 (null, a word, blank) is unreadable here, and so is a
/// time that is no whole number (a fraction `Time.at` or `to_i` takes, a null or word `to_i` reads as 1970), a row that
/// is no row (Integer#[] reads bits of a number; Array() makes pairs of an object) and Bybit's null `retMsg`; Rails
/// returns a candle (or its Failure, or none once its start filter drops a 1970 candle), Rust refuses the answer. Every
/// other case agrees exactly.
const UNREADABLE: &[(&str, &str)] = &[("kraken", "row_open_null"), ("kraken", "row_open_word"), ("kraken", "row_close_blank"), ("kraken", "row_time_float"),
    ("binance", "row_open_null"), ("binance", "row_open_word"), ("binance", "row_close_blank"), ("binance", "row_time_float"),
    ("binance_us", "row_open_null"), ("alpaca", "crypto_bar_open_null"), ("alpaca", "crypto_bar_open_word"), ("alpaca", "crypto_bar_close_blank"),
    ("kucoin", "row_open_null"), ("kucoin", "row_open_word"), ("kucoin", "row_close_blank"), ("kucoin", "row_time_float"), ("kucoin", "row_time_null"),
    ("kucoin", "row_time_word"),
    ("bitget", "row_open_null"), ("bitget", "row_open_word"), ("bitget", "row_close_blank"), ("bitget", "row_time_float"), ("bitget", "row_time_null"), ("bitget", "row_time_word"),
    ("bybit", "row_open_null"), ("bybit", "row_open_word"), ("bybit", "row_close_blank"), ("bybit", "row_time_float"), ("bybit", "row_time_null"), ("bybit", "row_time_word"), ("bybit", "json_object"),
    ("mexc", "row_open_null"), ("mexc", "row_open_word"), ("mexc", "row_close_blank"), ("mexc", "row_time_float"), ("mexc", "json_array"),
    ("bitrue", "row_open_null"), ("bitrue", "row_open_word"), ("bitrue", "row_close_blank"), ("bitrue", "row_time_float"), ("bitrue", "row_time_null"), ("bitrue", "row_time_word"),
    ("bitvavo", "row_open_null"), ("bitvavo", "row_open_word"), ("bitvavo", "row_close_blank"), ("bitvavo", "row_time_float"), ("bitvavo", "row_time_null"), ("bitvavo", "row_time_word"), ("bitvavo", "json_array"), ("bitvavo", "json_object"),
    ("gemini", "row_open_null"), ("gemini", "row_open_word"), ("gemini", "row_close_blank"), ("gemini", "row_time_float"), ("gemini", "row_time_null"), ("gemini", "row_time_word"), ("gemini", "json_array"),
    ("hyperliquid", "row_open_null"), ("hyperliquid", "row_open_word"), ("hyperliquid", "row_close_blank"), ("hyperliquid", "row_time_float"), ("hyperliquid", "row_time_null"), ("hyperliquid", "row_time_word"),
    ("coinbase", "row_open_null"), ("coinbase", "row_open_word"), ("coinbase", "row_close_blank"), ("coinbase", "row_time_float"), ("coinbase", "row_time_null"), ("coinbase", "row_time_word"),
    ("bingx", "row_open_null"), ("bingx", "row_open_word"), ("bingx", "row_close_blank"), ("bingx", "row_time_float"), ("bingx", "row_time_null"), ("bingx", "row_time_word"), ("bingx", "json_array"),];
/// D-COND-CANDLE-ERROR-ENVELOPE: an answer whose code is not the venue's success is its refusal here; Rails reads the
/// missing rows as no candles.
const ENVELOPE: &[(&str, &str)] = &[("kucoin", "error_envelope"), ("kucoin", "json_object"), ("bitget", "error_envelope"), ("bitget", "json_object"),
    ("bingx", "error_envelope"), ("bingx", "json_object")];
/// Spec E §2.10 (each adapter's `answer`): HTTP 429 (and Binance's 418) on a read is a throttle that names the venue's
/// rate-limit text; Rails fails it with the body.
const THROTTLED: &[(&str, &str, &str)] = &[("kraken", "http_429", "EAPI:Rate limit exceeded"), ("binance", "http_418", "rate limit exceeded"),
    ("binance", "http_429", "rate limit exceeded"), ("binance_us", "http_429", "rate limit exceeded"), ("bitrue", "http_429", "Too Many Requests"),
    ("bingx", "http_429", "Too Many Requests")];

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
    for &(venue, _) in VENUES {
        for (file, recorded) in fixture(venue)["sources"].as_object().unwrap() {
            use sha2::Digest;
            let now = hex::encode(sha2::Sha256::digest(std::fs::read(root.join(file)).unwrap()));
            assert_eq!(recorded.as_str().unwrap(), now, "{file} changed: re-record with script/rust/repin");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn every_recorded_candle_read_is_answered_as_rails_answers_it() {
    let (mut failures, mut unreadable, mut envelope, mut throttled, mut total) = (vec![], 0, 0, 0, 0);
    for &(venue, count) in VENUES {
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
                "kucoin" => kucoin::KucoinVenue::new(wire, kucoin::URL).candles(&ticker("BTC-USDT"), start_at, tf).await,
                "bitget" => bitget::BitgetVenue::new(wire, bitget::URL).candles(&ticker("BTCUSDT"), start_at, tf).await,
                "bybit" => bybit::BybitVenue::new(wire, bybit::URL).candles(&ticker("BTCUSDT"), start_at, tf).await,
                "mexc" => mexc::MexcVenue::new(wire, mexc::URL, "").candles(&ticker("BTCUSDT"), start_at, tf).await,
                "bitrue" => bitrue::BitrueVenue::new(wire, bitrue::URL, "").candles(&ticker("BTCUSDT"), start_at, tf).await,
                "bitvavo" => bitvavo::BitvavoVenue::new(wire, bitvavo::URL).candles(&ticker("BTC-EUR"), start_at, tf).await,
                "gemini" => gemini::GeminiVenue::new(wire, gemini::URL).candles(&ticker("btcusd"), start_at, tf).await,
                "hyperliquid" => hyperliquid::HyperliquidVenue::new(wire, hyperliquid::URL, "").candles(&ticker("@107"), start_at, tf).await,
                "coinbase" => coinbase::CoinbaseVenue::new(wire, coinbase::URL).candles(&ticker("BTC-USD"), start_at, tf).await,
                "bingx" => bingx::BingxVenue::new(wire, bingx::URL, "").candles(&ticker("BTC-USDT"), start_at, tf).await,
                _ => {
                    let t = &case["ticker"];
                    let urls = Urls { trading: deltabadger::venue::alpaca::PAPER_TRADING_URL.into(), data: deltabadger::venue::alpaca::DATA_URL.into() };
                    AlpacaVenue::new(wire, urls).candles(&ticker_of(t["ticker"].as_str().unwrap(), t["base"].as_str().unwrap(), t["crypto"] == true), start_at, tf).await
                }
            };
            // A POST is recorded with its body (Hyperliquid).
            let requests: Vec<String> = transport.requests().iter()
                .map(|r| match &r.body { Some(b) => format!("{} {b}", http::request_key(r)), None => http::request_key(r) }).collect();
            if Value::from(requests.clone()) != case["requests"] { failures.push(format!("{venue} {name}: requested {requests:?}, Rails {}", case["requests"])); }
            let network = case["http"].as_object().unwrap().values().flat_map(|r| r.as_array().unwrap()).any(|r| r.get("network").is_some());
            let listed = |list: &[(&str, &str)]| list.contains(&(venue, name));
            let ok = match (&got, case.get("candles"), case.get("failure"), case.get("raised")) {
                // Rails answers candles (or, where it reads such a row as none, its Failure); Rust refuses.
                (Err(VenueError::Ambiguous(_)), _, _, _) if listed(UNREADABLE) => { unreadable += 1; true }
                (Err(VenueError::Rejected(_)), Some(rails), _, _) if listed(ENVELOPE) => { envelope += 1; rails == &Value::Array(vec![]) }
                (Ok(candles), Some(rails), _, _) => Value::from(candles.iter().map(row).collect::<Vec<_>>()) == *rails,
                // honeymaker returns a transport failure as a Failure; the engine reads it as a transient read failure.
                // honeymaker returns it; Clients::Alpaca raises Client::TransientNetworkError.
                (Err(VenueError::Transient(_)), _, Some(_), _) => network,
                (Err(VenueError::Transient(_)), _, _, Some(raised)) => network && raised == "Client::TransientNetworkError",
                (Err(VenueError::Rejected(e)), _, Some(_), _) if THROTTLED.iter().any(|(v, n, _)| (*v, *n) == (venue, name)) => {
                    throttled += 1;
                    THROTTLED.iter().any(|(v, n, text)| (*v, *n) == (venue, name) && e[0].contains(text))
                }
                (Err(e @ (VenueError::Rejected(_) | VenueError::Unauthorized(_) | VenueError::Coded { .. })), _, Some(rails), _) => !network && Value::from(e.rejection().unwrap().to_vec()) == *rails,
                // Rails' KeyError keeps its text ("Key path error not found"); any other raise is the adapter's unreadable answer.
                (Err(VenueError::Ambiguous(m)), _, _, Some(raised)) => raised != "KeyError" || case["message"] == *m,
                _ => false,
            };
            if !ok { failures.push(format!("{venue} {name}: rust {got:?}\n  rails {}", Value::Object(case.as_object().unwrap().clone()))); }
        }
    }
    assert!(failures.is_empty(), "{} of {total} cases differ:\n{}", failures.len(), failures.join("\n"));
    assert_eq!((unreadable, envelope, throttled), (UNREADABLE.len(), ENVELOPE.len(), THROTTLED.len()), "every listed divergence is still recorded");
}

/// Only the venues whose candles are recorded here say so to eligibility.
#[test]
fn condition_candles_are_on_only_where_recorded() {
    let on: Vec<&str> = deltabadger::venue::registry::ROWS.iter().filter(|r| r.condition_candles).map(|r| r.name).collect();
    assert_eq!(on, ["Alpaca", "Kraken", "Binance", "Binance.US", "KuCoin", "Bitvavo", "Gemini", "Bitget", "MEXC", "Hyperliquid", "Bybit", "Bitrue", "Coinbase", "BingX"]);
}

/// The default for a venue not yet ported: a refusal, never an empty series (which would read as "no candles").
#[tokio::test(flavor = "current_thread")]
async fn a_venue_without_candles_refuses() {
    let v = deltabadger::venue::fake::FakeVenue::new();
    let got = v.candles(&ticker("btceur"), chrono::Utc::now(), Timeframe::Day).await;
    assert!(matches!(got, Err(VenueError::Rejected(_))), "{got:?}");
}

/// D-COND-CANDLE-CAP: Rails walks Binance's, Bitget's, Bybit's, MEXC's and Coinbase's pages, and follows Alpaca's tokens,
/// without end; past MAX_CANDLE_PAGES pages the read is a settled failure. (Rails cannot record it: the same answer every
/// time never ends its walk.) Bitrue's and Bitvavo's backward walks end on a page no older than the last, so a repeated
/// answer ends them; their cap holds only against a venue that keeps answering older pages.
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
    // Bitget and Bybit: a full page of old candles, every time.
    let page: Vec<Value> = (0..1000).map(|i| serde_json::json!([((start.timestamp() + i) * 1000).to_string(), "1", "2", "0.5", "1.5", "1"])).collect();
    let bitget_wire = ScriptedTransport::default();
    bitget_wire.reply("GET /api/v2/spot/market/candles", 200, serde_json::json!({ "code": "00000", "data": page }));
    let got = bitget::BitgetVenue::new(At(bitget_wire.clone(), now), bitget::URL).candles(&ticker("BTCUSDT"), start, Timeframe::Minute).await;
    assert!(matches!(&got, Err(VenueError::Rejected(e)) if e[0] == "Bitget: more than 1000 pages of candles"), "{got:?}");
    assert_eq!(bitget_wire.requests().len(), 1000);
    let bybit_wire = ScriptedTransport::default();
    bybit_wire.reply("GET /v5/market/kline", 200, serde_json::json!({ "retCode": 0, "result": { "list": page } }));
    let got = bybit::BybitVenue::new(At(bybit_wire.clone(), now), bybit::URL).candles(&ticker("BTCUSDT"), start, Timeframe::Minute).await;
    assert!(matches!(&got, Err(VenueError::Rejected(e)) if e[0] == "Bybit: more than 1000 pages of candles"), "{got:?}");
    assert_eq!(bybit_wire.requests().len(), 1000);
    // MEXC's windows are known before any is read: twenty years of minutes is refused at once.
    let mexc_wire = ScriptedTransport::default();
    let got = mexc::MexcVenue::new(At(mexc_wire.clone(), now), mexc::URL, "").candles(&ticker("BTCUSDT"), now - chrono::Duration::days(7300), Timeframe::Minute).await;
    assert!(matches!(&got, Err(VenueError::Rejected(e)) if e[0] == "MEXC: more than 1000 pages of candles"), "{got:?}");
    assert!(mexc_wire.requests().is_empty());
    // Coinbase: each window answers the same old candle, so the walk never reaches now.
    let coinbase_wire = ScriptedTransport::default();
    let old = serde_json::json!({ "candles": [{ "start": "1", "open": "1", "high": "2", "low": "0.5", "close": "1.5", "volume": "1" }] });
    coinbase_wire.reply("GET /api/v3/brokerage/market/products/BTC-USD/candles", 200, old);
    let got = coinbase::CoinbaseVenue::new(At(coinbase_wire.clone(), now), coinbase::URL).candles(&ticker("BTC-USD"), start, Timeframe::Minute).await;
    assert!(matches!(&got, Err(VenueError::Rejected(e)) if e[0] == "Coinbase: more than 1000 pages of candles"), "{got:?}");
    assert_eq!(coinbase_wire.requests().len(), 1000);
}

/// KuCoin's walk is capped in Rails (Exchange::MAX_CANDLE_PAGES): past it Rails returns what it read, and so does Rust.
#[tokio::test(flavor = "current_thread")]
async fn kucoins_capped_walk_returns_what_it_read() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:01Z").unwrap().to_utc();
    let start = now - chrono::Duration::days(3000);
    let wire = ScriptedTransport::default();
    wire.reply("GET /api/v1/market/candles", 200, serde_json::json!({ "code": "200000", "data": [[start.timestamp().to_string(), "1", "1.5", "2", "0.5", "1", "1"]] }));
    let got = kucoin::KucoinVenue::new(At(wire.clone(), now), kucoin::URL).candles(&ticker("BTC-USDT"), start, Timeframe::Minute).await.unwrap();
    assert_eq!((got.len(), wire.requests().len()), (1, 1000));
}

/// D-COND-CANDLE-COINBASE-EMPTY-WINDOW: after a window with candles, an empty one moves on from its end; Rails asks the
/// same window again without end (unrecordable).
#[tokio::test(flavor = "current_thread")]
async fn coinbase_moves_past_an_empty_window() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:01Z").unwrap().to_utc();
    let start = now - chrono::Duration::days(1000);
    let wire = ScriptedTransport::default();
    let candle = serde_json::json!({ "start": (start.timestamp() + 86_400).to_string(), "open": "1", "high": "2", "low": "0.5", "close": "1.5", "volume": "1" });
    wire.reply("GET /api/v3/brokerage/market/products/BTC-USD/candles", 200, serde_json::json!({ "candles": [candle] }))
        .reply("GET /api/v3/brokerage/market/products/BTC-USD/candles", 200, serde_json::json!({ "candles": [] }));
    let got = coinbase::CoinbaseVenue::new(At(wire.clone(), now), coinbase::URL).candles(&ticker("BTC-USD"), start, Timeframe::Day).await.unwrap();
    let starts: Vec<String> = wire.requests().iter().map(|r| r.query.iter().find(|(k, _)| *k == "start").unwrap().1.clone()).collect();
    let day = |d: i64| (start.timestamp() + d * 86_400).to_string();
    assert_eq!(starts, [day(0), (start.timestamp() + 86_401).to_string(), (start.timestamp() + 86_401 + 350 * 86_400).to_string(),
                        (start.timestamp() + 86_401 + 700 * 86_400).to_string()]);
    assert_eq!(got.len(), 1);
}

/// ActiveSupport's `1.month.ago`: a calendar month, the day clamped to the shorter month's last.
#[test]
fn a_month_ago_is_a_calendar_month() {
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().to_utc();
    assert_eq!(Timeframe::Month.ago(at("2026-03-31T10:00:00Z")), at("2026-02-28T10:00:00Z"));
    assert_eq!(Timeframe::Month.ago(at("2028-03-31T10:00:00Z")), at("2028-02-29T10:00:00Z"));
    assert_eq!(Timeframe::Week.ago(at("2026-09-02T10:00:01Z")), at("2026-08-26T10:00:01Z"));
}
