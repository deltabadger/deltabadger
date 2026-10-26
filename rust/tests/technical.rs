//! engine::technical against Ticker::TechnicallyAnalyzable as recorded (script/rust/record_indicator_vectors.rb): RSI and
//! the moving averages to the last digit and Ruby class, the high and low of a window with the timeframe it reads, and the
//! all-time high with the `tickers` row it writes, the caches between calls.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::candles::{Candle, Timeframe};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::technical::{self, Cache, MaType, Window};
use deltabadger::ruby::{BigDec, Num};
use deltabadger::venue::fake::FakeVenue;
use deltabadger::venue::VenueError;
use rusqlite::{params, Connection};
use serde_json::Value;

fn fixture() -> Value {
    use sha2::{Digest, Sha256};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/indicator_vectors.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in fixture["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}: script/rust/repin");
    }
    fixture
}

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn iso(t: DateTime<Utc>) -> String { t.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string() }

fn tf(name: &str) -> Timeframe {
    match name {
        "1m" => Timeframe::Minute, "5m" => Timeframe::FiveMinutes, "15m" => Timeframe::FifteenMinutes, "30m" => Timeframe::ThirtyMinutes,
        "1h" => Timeframe::Hour, "4h" => Timeframe::FourHours, "1d" => Timeframe::Day, "3d" => Timeframe::ThreeDays, "1w" => Timeframe::Week,
        "1M" => Timeframe::Month, other => panic!("timeframe {other}"),
    }
}
fn tf_name(t: Timeframe) -> &'static str {
    ["1m", "5m", "15m", "30m", "1h", "4h", "1d", "3d", "1w", "1M"].into_iter().find(|n| tf(n) == t).unwrap()
}

fn candles(rows: &Value) -> Vec<Candle> {
    rows.as_array().unwrap().iter().map(|r| {
        let d = |i: usize| BigDec::parse(r[i].as_str().unwrap()).unwrap();
        Candle { at: at(r[0].as_str().unwrap()), open: d(1), high: d(2), low: d(3), close: d(4), volume: d(5) }
    }).collect()
}

fn answer(a: &Value) -> Result<Vec<Candle>, VenueError> {
    match a.get("failure") { Some(f) => Err(VenueError::Rejected(serde_json::from_value(f.clone()).unwrap())), None => Ok(candles(&a["candles"])) }
}

/// A value as the recorder encodes it.
fn encode(n: &Num) -> Value {
    match n {
        Num::Int(i) => serde_json::json!({ "int": i.to_string() }),
        Num::Float(f) => serde_json::json!({ "float": format!("{:016x}", f.to_bits()), "s": deltabadger::ruby::float_to_s(*f) }),
        Num::Dec(d) => serde_json::json!({ "dec": d.to_s_f() }),
    }
}

fn reads(v: &FakeVenue) -> Value {
    v.candle_reads().into_iter().map(|(start, t)| serde_json::json!({ "start_at": iso(start), "timeframe": tf_name(t) })).collect()
}

fn outcome<T>(r: Result<T, VenueError>, enc: impl Fn(T) -> Value) -> Value {
    match r {
        Ok(v) => serde_json::json!({ "value": enc(v) }),
        Err(VenueError::Rejected(e)) => serde_json::json!({ "failure": e }),
        Err(e) => panic!("{e:?}"),
    }
}

fn ticker_named(id: i64, pd: i64) -> Ticker {
    let zero = BigDec::zero();
    Ticker { id, ticker: "XBTEUR".into(), base_code: "XBT".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(),
             exchange_name: "Kraken".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 5, price_decimals: pd,
             minimum_base_size: zero.clone(), minimum_quote_size: zero, trading_enabled: true, available: true, crypto: true }
}

/// RSI, SMA and EMA over every recorded series: the candle read's start, then the value and its class, or the Failure.
/// D-COND-RSI-DEGENERATE: where Rails' RSI is Float NaN (a flat series) Rust answers Rails' too-few-closes Failure; the
/// recording keeps Rails' NaN and this test names each such case.
#[tokio::test(flavor = "current_thread")]
async fn indicators_match_rails_to_the_last_digit() {
    let f = fixture();
    let now = at(f["now"].as_str().unwrap());
    let series = f["series"].as_array().unwrap();
    let cases = f["indicators"].as_array().unwrap();
    assert!((1300..=2500).contains(&cases.len()), "{}", cases.len());
    let mut degenerate = vec![];
    for c in cases {
        let s = &series[c["series"].as_u64().unwrap() as usize];
        let timeframe = tf(s["timeframe"].as_str().unwrap());
        let period = c["period"].as_u64().unwrap() as usize;
        let ticker = ticker_named(1, c["price_decimals"].as_i64().unwrap());
        let venue = FakeVenue::new().candles_answer(Ok(candles(&s["candles"])));
        let got = match c["kind"].as_str().unwrap() {
            "rsi" => technical::rsi(&venue, &Cache::default(), &ticker, timeframe, period, now).await,
            kind => technical::moving_average(&venue, &Cache::default(), &ticker, timeframe, period, if kind == "sma" { MaType::Sma } else { MaType::Ema }, now).await,
        };
        let name = format!("{} {} {period} pd {}", s["name"], c["kind"], c["price_decimals"]);
        assert_eq!(reads(&venue), c["reads"], "{name}");
        let mut want = serde_json::json!({});
        for k in ["value", "failure"] { if let Some(v) = c.get(k) { want[k] = v.clone(); } }
        if c["value"]["s"] == "NaN" || c["value"]["dec"] == "NaN" {
            degenerate.push(s["name"].as_str().unwrap().to_string());
            let message = format!("Failed to calculate {} RSI for XBTEUR (period: {period})", timeframe.inspect());
            assert_eq!(outcome(got, |v| encode(&v)), serde_json::json!({ "failure": [message] }), "{name}");
            continue;
        }
        assert_eq!(outcome(got, |v| encode(&v)), want, "{name}");
    }
    // Rails' NaN comes only from RSI (the guard above reads `value`, which a moving average never has as NaN) and covers
    // the flat series at every period.
    assert_eq!(degenerate.iter().filter(|n| *n == "flat_30").count(), 5);
    assert_eq!(degenerate.len(), 50, "D-COND-RSI-DEGENERATE cases");
}

fn install() -> (tempfile::TempDir, deltabadger::store::Opened, common::seed::Seeded, Ticker, Ticker) {
    let (dir, o, s) = common::install();
    let c = &o.primary;
    let kraken = model::ticker_by_id(c, s.exchange_id, s.ticker_id).unwrap().unwrap();
    let t = "2026-01-01 00:00:00";
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Alpaca', 'Alpaca', '0.15', '0.25', ?1, ?1)", [t]).unwrap();
    let alpaca = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('AAPL.US', 'AAPL', 'Apple', 'Stock', ?1, ?1)", [t]).unwrap();
    let aapl = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('usd', 'USD', 'US Dollar', 'Currency', ?1, ?1)", [t]).unwrap();
    let usd = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
               VALUES (?1, 'AAPL', 'AAPL', 'USD', ?2, ?3, 9, 2, 2, '0.000000001', '1', 1, 1, ?4, ?4)", params![alpaca, aapl, usd, t]).unwrap();
    let stock = model::ticker_by_id(c, alpaca, c.last_insert_rowid()).unwrap().unwrap();
    assert!(!stock.crypto && kraken.crypto);
    (dir, o, s, kraken, stock)
}

fn row(c: &Connection, id: i64) -> Value {
    c.query_row("SELECT typeof(ath), ath, ath_updated_at, updated_at FROM tickers WHERE id = ?1", [id], |r| {
        let ath = match r.get_ref(1)? { rusqlite::types::ValueRef::Real(f) => Some(f), rusqlite::types::ValueRef::Integer(i) => Some(i as f64), _ => None };
        Ok(serde_json::json!({ "ath_type": r.get::<_, String>(0)?, "ath": ath, "ath_updated_at": r.get::<_, Option<String>>(2)?, "updated_at": r.get::<_, String>(3)? }))
    }).unwrap()
}

/// Calls in order on one cache, each at its own time: high and low per window and timeframe step, the ATH's incremental,
/// deep and fallback walks, the restated (Alpaca stock) replacement, the known-empty seed, the 20 s and until-close caches.
#[tokio::test(flavor = "current_thread")]
async fn high_low_and_ath_sequences_match_rails() {
    let f = fixture();
    let (_dir, o, _s, kraken, stock) = install();
    let c = &o.primary;
    let sequences = f["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 46);
    for seq in sequences {
        let ticker = if seq["ticker"] == "kraken" { &kraken } else { &stock };
        let before = &seq["before"];
        let ath: Option<f64> = before["ath"].as_str().map(|s| s.parse().unwrap());
        let updated = before["ath_updated_at"].as_str().map(|s| deltabadger::codec::format_time(at(s)));
        c.execute("UPDATE tickers SET ath = ?1, ath_updated_at = ?2, updated_at = '2026-01-01 00:00:00' WHERE id = ?3", params![ath, updated, ticker.id]).unwrap();
        let cache = Cache::default();
        for step in seq["steps"].as_array().unwrap() {
            let name = format!("{} at {}", seq["name"], step["at"]);
            let now = at(step["at"].as_str().unwrap());
            let mut venue = FakeVenue::new();
            for a in step["answers"].as_array().unwrap() { venue = venue.candles_answer(answer(a)); }
            let window = || Window::of(step["window"].as_str().unwrap()).unwrap();
            let period = || step["period"].as_u64().unwrap() as usize;
            let timeframe = || tf(step["timeframe"].as_str().unwrap());
            let dec = |v: Option<BigDec>| v.map_or(Value::Null, |d| serde_json::json!({ "dec": d.to_s_f() }));
            let got = match step["call"].as_str().unwrap() {
                "high" => outcome(technical::high_of_last(c, &venue, &cache, ticker, window(), now).await.unwrap(), dec),
                "low" => outcome(technical::low_of_last(&venue, &cache, ticker, window(), now).await, dec),
                "rsi" => outcome(technical::rsi(&venue, &cache, ticker, timeframe(), period(), now).await, |v| encode(&v)),
                kind => outcome(technical::moving_average(&venue, &cache, ticker, timeframe(), period(), if kind == "sma" { MaType::Sma } else { MaType::Ema }, now).await, |v| encode(&v)),
            };
            let mut want = serde_json::json!({});
            if let Some(v) = step.get("failure") { want["failure"] = v.clone(); } else { want["value"] = step["value"].clone(); }
            assert_eq!(got, want, "{name}");
            assert_eq!(reads(&venue), step["reads"], "{name}");
            let r = &step["row"];
            let rails_ath = r["ath"].as_str().map(|s| s.parse::<f64>().unwrap());
            assert_eq!(row(c, ticker.id), serde_json::json!({ "ath_type": r["ath_type"], "ath": rails_ath, "ath_updated_at": r["ath_updated_at"], "updated_at": r["updated_at"] }), "{name}");
            // The row read back as Rails reads it (Float#to_d of the REAL).
            let read = c.query_row("SELECT ath FROM tickers WHERE id = ?1", [ticker.id], |x| Ok(deltabadger::ruby::from_sql(x.get_ref(0)?).unwrap())).unwrap();
            assert_eq!(dec(read), r["ath_read"], "{name}");
        }
    }
}
