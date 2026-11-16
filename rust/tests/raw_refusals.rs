//! D-RAW-REFUSALS-COPY (part 2 of D-ORDER-TOO-LARGE-COPY): venue refusals Rails shows raw read as a plain sentence on the
//! bot page, in the orders list, in the mail and in the desktop notice. Four groups: too small outside the scheduled DCA
//! skip, our own transport errors (`is_transient`), Binance-style size-filter failures, and JSON bodies (their message
//! field). Display only: what the engine stores and does is unchanged, pinned here per group.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{model, notice, placement, FixedClock};
use deltabadger::mail::render::{self, humanize_error, Urls};
use deltabadger::store::{self, Paths};
use deltabadger::venue::binance::{BinanceVenue, Pairs};
use deltabadger::venue::bitget::BitgetVenue;
use deltabadger::venue::gemini::GeminiVenue;
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::Venue;
use deltabadger::web::i18n;
use serde_json::{json, Value};

const T0: &str = "2026-09-01T10:00:00.5Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn en(venue: &str, name: &str, message: &str) -> String { humanize_error(venue, name, "en", message) }
fn de(venue: &str, name: &str, message: &str) -> String { humanize_error(venue, name, "de", message) }

const SMALL_EN: &str = "The order is below the minimum {} accepts. Raise the amount.";
const SMALL_DE: &str = "Die Order liegt unter dem Minimum, das {} annimmt. Erhöhe den Betrag.";
const RANGE_EN: &str = "The order is outside the size range {} allows. Check the amount.";
const RANGE_DE: &str = "Die Order liegt außerhalb der Größe, die {} erlaubt. Prüfe den Betrag.";
const DOWN_EN: &str = "{} is temporarily unavailable. The bot will retry automatically.";
const DOWN_DE: &str = "{} ist vorübergehend nicht verfügbar. Der Bot versucht es automatisch erneut.";
fn fill(template: &str, name: &str) -> String { template.replace("{}", name) }

// --- 1. too small ----------------------------------------------------------------------------------------------------

#[test]
fn every_sourced_too_small_text_reads_as_the_sentence() {
    for (venue, name, message) in [
        ("Exchanges::Binance", "Binance", "Filter failure: MIN_NOTIONAL"),
        ("Exchanges::BinanceUs", "Binance.US", "Filter failure: MIN_NOTIONAL"),
        ("Exchanges::Bybit", "Bybit", "Order value exceeded lower limit"),
        ("Exchanges::Bybit", "Bybit", "Order quantity lower than the minimum."),
        ("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":217,"error":"Minimum order size in quote currency is 5 EUR."}"#),
        ("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":212,"error":"`amount` is below the minimum for this market."}"#),
        ("Exchanges::Kraken", "Kraken", "EOrder:Order minimum not met"),
        ("Exchanges::Kraken", "Kraken", "EOrder:Order minimum not met (volume too low)"),
        ("Exchanges::Bitget", "Bitget", r#"{"code":"45110","msg":"less than the minimum order quantity","requestTime":1788000000000,"data":null}"#),
        ("Exchanges::Bitget", "Bitget", "Bitget API error 45110: Less than the minimum order amount"),
        ("Exchanges::Kucoin", "KuCoin", "KuCoin API error 102435: Below the minimum order size for the currency"),
    ] {
        assert_eq!(en(venue, name, message), fill(SMALL_EN, name), "{venue} {message}");
        assert_eq!(de(venue, name, message), fill(SMALL_DE, name), "{venue} {message}");
    }
}

#[test]
fn the_minimum_is_quoted_only_when_the_text_ends_with_a_plain_amount() {
    let limit = |name: &str, minimum: &str| format!("{name} accepts orders of at least {minimum}. Raise the amount.");
    for (venue, name, message, minimum) in [
        ("Exchanges::Mexc", "MEXC", "The minimum transaction volume cannot be less than :1USDT", "1USDT"),
        ("Exchanges::Mexc", "MEXC", "The minimum transaction volume cannot be less than :5 USDT", "5 USDT"),
        ("Exchanges::Hyperliquid", "Hyperliquid", "Hyperliquid order failed: Order must have minimum value of $10.", "$10"),
        ("Exchanges::Hyperliquid", "Hyperliquid", "Hyperliquid order failed: Order must have minimum value of $10. asset=10142", "$10"),
        ("Exchanges::Hyperliquid", "Hyperliquid", "Hyperliquid order failed: Order must have minimum value of 10 USDC.", "10 USDC"),
        ("Exchanges::Alpaca", "Alpaca", "cost basis must be >= minimal amount of order 10", "10"),
        ("Exchanges::Alpaca", "Alpaca", "cost basis must be >= minimal amount of order 1.5", "1.5"),
    ] {
        assert_eq!(en(venue, name, message), limit(name, minimum), "{message}");
    }
    assert_eq!(de("Exchanges::Mexc", "MEXC", "The minimum transaction volume cannot be less than :1USDT"), "MEXC nimmt Orders ab 1USDT an. Erhöhe den Betrag.");
    for ending in ["", "$", "10abc1", "10, max 20", ".5", "1..5", "1e5", "10 USDTUSDTUSDT", "10  USD", "-10", "10 USD extra"] {
        let message = format!("cost basis must be >= minimal amount of order {ending}");
        assert_eq!(en("Exchanges::Alpaca", "Alpaca", &message), fill(SMALL_EN, "Alpaca"), "{message:?}");
    }
}

#[test]
fn too_small_texts_outside_the_table_keep_what_they_read_today() {
    for (venue, message) in [
        // No source: not in Kraken's docs, only in our scripted mail grid.
        ("Exchanges::Kraken", "EGeneral:Invalid arguments:volume minimum not met"),
        // KuCoin's 400100 is a generic parameter error; its only source is a scripted recording.
        ("Exchanges::Kucoin", r#"{"code":"400100","msg":"Order size below the minimum requirement."}"#),
        ("Exchanges::Alpaca", r#"order rejected: <qty> must be > 0 & "notional" too small"#),
        // A table text under another venue.
        ("Exchanges::Coinbase", "Filter failure: MIN_NOTIONAL"),
        ("Exchanges::Kraken", "Order value exceeded lower limit"),
        // Bitget by its code, never a neighbour of it.
        ("Exchanges::Bitget", r#"{"code":"4511","msg":"x"}"#),
        ("Exchanges::Bitget", r#"{"code":"451100","msg":"x"}"#),
        ("Exchanges::Bitget", "Bitget API error 451100: x"),
        ("Exchanges::Bitvavo", r#"{"errorCode":2170,"error":"x"}"#),
    ] {
        let out = en(venue, "X", message);
        assert!(out != fill(SMALL_EN, "X") && !out.contains("accepts orders of at least"), "{venue} {message:?} -> {out:?}");
    }
}

#[test]
fn the_engine_reads_the_same_skip_rows_as_before() {
    let m = |s: &str| vec![s.to_string()];
    // The rows it skipped before still skip...
    assert!(placement::below_minimum("Exchanges::Binance", Some("-1013"), &m("Filter failure: MIN_NOTIONAL")));
    assert!(placement::below_minimum("Exchanges::Mexc", Some("30002"), &m("anything")));
    assert!(placement::below_minimum("Exchanges::Bybit", Some("170140"), &m("Order value exceeded lower limit")));
    assert!(placement::below_minimum("Exchanges::Bitvavo", Some("217"), &m("x")));
    assert!(placement::below_minimum("Exchanges::Kraken", None, &m("EOrder:Order minimum not met")));
    // ...and the display-only rows never do.
    assert!(!placement::below_minimum("Exchanges::Bitget", Some("45110"), &m("less than the minimum order quantity")));
    assert!(!placement::below_minimum("Exchanges::Bitget", None, &m("Bitget API error 45110: Less than the minimum order amount")));
    assert!(!placement::below_minimum("Exchanges::Kucoin", Some("102435"), &m("KuCoin API error 102435: Below the minimum order size for the currency")));
    assert!(!placement::below_minimum("Exchanges::Kucoin", None, &m("KuCoin API error 102435: Below the minimum order size for the currency")));
}

// --- 2. our own transport errors -------------------------------------------------------------------------------------

#[test]
fn whatever_is_transient_matches_reads_as_temporarily_unavailable() {
    for (venue, name, message) in [
        ("Exchanges::Binance", "Binance", "Net::ReadTimeout"),
        ("Exchanges::Gemini", "Gemini", r#"Connection refused - connect(2) for "api.gemini.com" port 443"#),
        ("Exchanges::Alpaca", "Alpaca", r#"Faraday::ConnectionFailed: Connection refused - connect(2) for "data.alpaca.markets" port 443"#),
        ("Exchanges::Hyperliquid", "Hyperliquid", "Hyperliquid::Error: Network error: Net::ReadTimeout"),
        ("Exchanges::Hyperliquid", "Hyperliquid", r#"Hyperliquid::Error: Network error: Connection refused - connect(2) for "api.hyperliquid.xyz" port 443"#),
        ("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":101,"error":"Unknown error."}"#),
        ("Exchanges::Kucoin", "KuCoin", "The system is busy"),
    ] {
        assert_eq!(en(venue, name, message), fill(DOWN_EN, name), "{venue} {message}");
        assert_eq!(de(venue, name, message), fill(DOWN_DE, name), "{venue} {message}");
    }
    // What `is_transient` does not match keeps its text: a 5xx without a body, Hyperliquid's unparsed gateway page, and
    // Kraken's network errors (its list names only its own codes).
    for (venue, message) in [("Exchanges::Kraken", "Net::ReadTimeout"), ("Exchanges::Binance", "the server responded with status 502 for POST https://api.binance.com/api/v3/order"),
                             ("Exchanges::Hyperliquid", "Hyperliquid::Error: Network error: Empty input (after ) at line 1, column 1 [parse.c:1270] in 'Bad Gateway")] {
        assert_eq!(en(venue, "X", message), message);
    }
}

// --- 3. size filters -------------------------------------------------------------------------------------------------

#[test]
fn binance_style_size_filters_read_as_the_neutral_sentence() {
    for (venue, name) in [("Exchanges::Binance", "Binance"), ("Exchanges::BinanceUs", "Binance.US"), ("Exchanges::Bitrue", "Bitrue")] {
        for message in ["Filter failure: NOTIONAL", "Filter failure: LOT_SIZE"] {
            assert_eq!(en(venue, name, message), fill(RANGE_EN, name), "{venue} {message}");
            assert_eq!(de(venue, name, message), fill(RANGE_DE, name), "{venue} {message}");
        }
    }
    for (venue, message) in [("Exchanges::Mexc", "Filter failure: NOTIONAL"), ("Exchanges::Binance", "Filter failure: PRICE_FILTER"),
                             ("Exchanges::Binance", "Filter failure: NOTIONALS")] {
        assert_eq!(en(venue, "X", message), message);
    }
}

// --- 4. JSON bodies --------------------------------------------------------------------------------------------------

#[test]
fn a_json_body_shows_its_message_field() {
    for (venue, body, shown) in [
        ("Exchanges::Gemini", r#"{"result":"error","reason":"InvalidPrice","message":"Invalid price for symbol BTCUSD"}"#, "Invalid price for symbol BTCUSD"),
        ("Exchanges::Gemini", r#"{"result":"error","reason":"InvalidSymbol","message":"Invalid symbol"}"#, "Invalid symbol"),
        ("Exchanges::Bitvavo", r#"{"errorCode":205,"error":"market parameter is invalid."}"#, "market parameter is invalid."),
        ("Exchanges::Bitget", r#"{"code":"40034","msg":"Parameter symbol does not exist","requestTime":1788000000000,"data":null}"#, "Parameter symbol does not exist"),
        ("Exchanges::Coinbase", r#"{"error":"INVALID_ARGUMENT","error_details":"valid product_id is required","message":"valid product_id is required"}"#, "valid product_id is required"),
    ] {
        assert_eq!(en(venue, "X", body), shown, "{venue}");
        assert_eq!(de(venue, "X", body), shown, "{venue}: the venue's own words, in every locale");
    }
}

#[test]
fn a_json_body_without_its_field_and_other_venues_bodies_keep_what_they_read_today() {
    for (venue, body) in [
        ("Exchanges::Gemini", r#"{"result":"error","reason":"InvalidPrice"}"#),
        ("Exchanges::Gemini", r#"{"result":"error","message":""}"#),
        ("Exchanges::Gemini", r#"{"result":"error","message":"   "}"#),
        ("Exchanges::Gemini", r#"{"result":"error","message":7}"#),
        ("Exchanges::Gemini", r#"["message"]"#),
        // Another venue's field name.
        ("Exchanges::Bitvavo", r#"{"errorCode":205,"message":"x"}"#),
        ("Exchanges::Bitget", r#"{"code":"40034","message":"x"}"#),
        // Venues out of this group.
        ("Exchanges::Kraken", r#"{"code":"400007","msg":"Access Denied"}"#),
        ("Exchanges::Kucoin", r#"{"code":"400100","msg":"x"}"#),
        // Cut at the notice bound, and two errors joined: neither parses.
        ("Exchanges::Gemini", r#"{"result":"error","reason":"InvalidPrice","message":"Invalid price for symb…"#),
        ("Exchanges::Gemini", r#"{"result":"error","message":"a"}, {"result":"error","message":"b"}"#),
        ("Exchanges::Gemini", "plain text"),
    ] {
        assert_eq!(en(venue, "X", body), body, "{venue} {body}");
    }
    // A long body: what the notice keeps is cut, so it stays raw; the full body (the orders list) shows its message.
    let long = format!(r#"{{"result":"error","reason":"InvalidPrice","message":"Invalid price","pad":"{}"}}"#, "x".repeat(600));
    assert_eq!(en("Exchanges::Gemini", "Gemini", &long), "Invalid price");
    let cut: String = long.chars().take(notice::ERROR_LIMIT).collect();
    assert_eq!(en("Exchanges::Gemini", "Gemini", &cut), cut);
}

#[test]
fn a_kind_the_venue_already_names_keeps_its_sentence() {
    assert_eq!(en("Exchanges::Gemini", "Gemini", r#"{"result":"error","reason":"InsufficientFunds","message":"Insufficient funds"}"#),
               "Not enough funds on your Gemini account.");
    assert_eq!(en("Exchanges::Bybit", "Bybit", "Order amount too large."), "The order is larger than Bybit allows. Lower the amount, or split it across more bots.");
}

#[test]
fn every_locale_has_the_three_sentences_of_its_own() {
    for locale in deltabadger::web::locale::LOCALES {
        for (key, placeholder) in [("order_too_small", None), ("order_too_small_limit", Some("%{minimum}")), ("order_size_out_of_range", None)] {
            let full = format!("{locale}.errors.exchange.{key}");
            let text = i18n::all().iter().find(|(k, _)| *k == full).unwrap_or_else(|| panic!("{full} missing")).1;
            assert!(text.contains("%{exchange}"), "{full}: {text}");
            assert_eq!(text.contains("%{minimum}"), placeholder.is_some(), "{full}: {text}");
        }
        for (venue, message) in [("Exchanges::Mexc", "The minimum transaction volume cannot be less than :1USDT"), ("Exchanges::Kraken", "EOrder:Order minimum not met"),
                                 ("Exchanges::Binance", "Filter failure: LOT_SIZE")] {
            let out = humanize_error(venue, "Venue", locale, message);
            assert!(out.contains("Venue") && !out.contains("%{") && out != message, "{locale}: {out}");
        }
        let out = humanize_error("Exchanges::Mexc", "Venue", locale, "The minimum transaction volume cannot be less than :1USDT");
        assert!(out.contains("1USDT"), "{locale}: {out}");
    }
    if let Some(en) = deltabadger::web::locale::LOCALES.iter().find(|l| **l == "en") {
        for other in deltabadger::web::locale::LOCALES.iter().filter(|l| *l != en) {
            let a = humanize_error("Exchanges::Kraken", "Kraken", en, "EOrder:Order minimum not met");
            assert_ne!(a, humanize_error("Exchanges::Kraken", "Kraken", other, "EOrder:Order minimum not met"), "{other} is a copy of en");
        }
    }
}

/// Every recorded Rails mail this divergence changes reads one of its sentences (the parity harness compares them with the
/// reading before it, `humanize_error_before_raw_refusals`).
#[test]
fn the_recorded_parity_texts_this_changes_read_a_sentence() {
    for (venue, name, message, want) in [
        ("Exchanges::Bybit", "Bybit", "Net::ReadTimeout", fill(DOWN_EN, "Bybit")),
        ("Exchanges::Binance", "Binance", r#"Connection refused - connect(2) for "api.binance.com" port 443"#, fill(DOWN_EN, "Binance")),
        ("Exchanges::BinanceUs", "Binance.US", "Filter failure: NOTIONAL", fill(RANGE_EN, "Binance.US")),
        ("Exchanges::Bitrue", "Bitrue", "Filter failure: LOT_SIZE", fill(RANGE_EN, "Bitrue")),
        ("Exchanges::Kraken", "Kraken", "EOrder:Order minimum not met", fill(SMALL_EN, "Kraken")),
        ("Exchanges::Bitget", "Bitget", r#"{"code":"45110","msg":"less than the minimum order quantity","requestTime":1788000000000,"data":null}"#, fill(SMALL_EN, "Bitget")),
        ("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":217,"error":"Minimum order size in quote currency is 5 EUR."}"#, fill(SMALL_EN, "Bitvavo")),
        ("Exchanges::Hyperliquid", "Hyperliquid", "Hyperliquid order failed: Order must have minimum value of $10. asset=10142", "Hyperliquid accepts orders of at least $10. Raise the amount.".into()),
        ("Exchanges::Gemini", "Gemini", r#"{"result":"error","reason":"InvalidPrice","message":"Invalid price for symbol BTCUSD"}"#, "Invalid price for symbol BTCUSD".into()),
    ] {
        assert_eq!(en(venue, name, message), want, "{venue} {message}");
        assert_eq!(render::humanize_error_before_raw_refusals(venue, name, "en", message), message, "Rails' reading: raw");
    }
}

// --- end to end ------------------------------------------------------------------------------------------------------

struct Install { dir: tempfile::TempDir, o: store::Opened, s: seed::Seeded, bot: i64 }

/// The seeded Kraken install renamed to `exchange_type`, its pair BTC/EUR (`btceur` on Gemini, else `BTCEUR`).
fn install_as(exchange_type: &str, name: &str) -> Install {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute("UPDATE exchanges SET type = ?1, name = ?2", [exchange_type, name]).unwrap();
    let symbol = if exchange_type == "Exchanges::Gemini" { "btceur" } else { "BTCEUR" };
    o.primary.execute("UPDATE tickers SET ticker = ?1", [symbol]).unwrap();
    o.primary.execute_batch("UPDATE tickers SET base = 'BTC', quote = 'EUR', base_decimals = 8, quote_decimals = 2, price_decimals = 2, \
                             minimum_base_size = '0.00001', minimum_quote_size = '1'").unwrap();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("interval", json!("hour")));
    Install { dir, o, s, bot }
}
fn exchange_id(i: &Install) -> i64 { i.o.primary.query_row("SELECT id FROM exchanges", [], |r| r.get(0)).unwrap() }

fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }

/// A Bitget buy refused with `body` (HTTP 400).
fn bitget(i: &Install, body: Value) -> BitgetVenue<ScriptedTransport> {
    let env = |data: Value| json!({ "code": "00000", "msg": "success", "requestTime": 1, "data": data });
    let t = ScriptedTransport::from_script(&json!({
        "GET /api/v2/spot/market/orderbook": ok(env(json!({ "asks": [["64001.37", "0.2"]], "bids": [["64000", "1.5"]], "ts": "1" }))),
        "GET /api/v2/spot/market/tickers": ok(env(json!([{ "symbol": "BTCEUR", "lastPr": "64000.5" }]))),
        "GET /api/v2/spot/account/assets": ok(env(json!([{ "coin": "BTC", "available": "0.5", "frozen": "0", "locked": "0" }, { "coin": "EUR", "available": "100000", "frozen": "0", "locked": "0" }]))),
        "POST /api/v2/spot/trade/place-order": [{ "status": 400, "body": body }],
    }));
    BitgetVenue::new(t, deltabadger::venue::bitget::URL).with_pairs(Pairs::load(&i.o.primary, exchange_id(i)).unwrap())
}
/// A Gemini buy refused with `body` (HTTP 400).
fn gemini(i: &Install, body: Value) -> GeminiVenue<ScriptedTransport> {
    let t = ScriptedTransport::from_script(&json!({
        "GET /v1/pubticker/btceur": ok(json!({ "bid": "64000.01", "ask": "64000.11", "last": "64000.05", "volume": { "BTC": "1", "EUR": "1", "timestamp": 1 } })),
        "POST /v1/balances": ok(json!([{ "type": "exchange", "currency": "BTC", "amount": "0.5", "available": "0.5", "availableForWithdrawal": "0.5" },
                                       { "type": "exchange", "currency": "EUR", "amount": "100000", "available": "100000", "availableForWithdrawal": "100000" }])),
        "POST /v1/order/new": [{ "status": 400, "body": body }],
    }));
    let creds = deltabadger::crypto::Credentials { ibkr: None, key: "KEY-A".into(), secret: "s".into(), passphrase: None, redaction_values: vec![] };
    GeminiVenue::new(t, deltabadger::venue::gemini::URL).with_pairs(Pairs::load(&i.o.primary, exchange_id(i)).unwrap()).with_identity(&creds)
}
/// A Binance buy refused with `body` (HTTP 400).
fn binance(i: &Install, body: Value) -> BinanceVenue<ScriptedTransport> {
    let t = ScriptedTransport::from_script(&json!({
        "GET /api/v3/ticker/bookTicker": ok(json!({ "symbol": "BTCEUR", "bidPrice": "64000.01", "bidQty": "1", "askPrice": "64000.11", "askQty": "1" })),
        "GET /api/v3/ticker/price": ok(json!({ "symbol": "BTCEUR", "price": "64000.05" })),
        "GET /api/v3/account": ok(json!({ "balances": [{ "asset": "BTC", "free": "0.5", "locked": "0" }, { "asset": "EUR", "free": "100000", "locked": "0" }] })),
        "POST /api/v3/order": [{ "status": 400, "body": body }],
    }));
    BinanceVenue::new(t, "https://api.binance.com", "s3cr3t").with_pairs(Pairs::load(&i.o.primary, exchange_id(i)).unwrap())
}
/// A Gemini buy that never leaves: the connection is refused before the order is sent (the decisions grid's
/// `add_network_pre_send`).
fn gemini_unreachable(i: &Install, message: &str) -> GeminiVenue<ScriptedTransport> {
    let t = ScriptedTransport::from_script(&json!({
        "GET /v1/pubticker/btceur": ok(json!({ "bid": "64000.01", "ask": "64000.11", "last": "64000.05", "volume": { "BTC": "1", "EUR": "1", "timestamp": 1 } })),
        "POST /v1/balances": ok(json!([{ "type": "exchange", "currency": "BTC", "amount": "0.5", "available": "0.5", "availableForWithdrawal": "0.5" },
                                       { "type": "exchange", "currency": "EUR", "amount": "100000", "available": "100000", "availableForWithdrawal": "100000" }])),
        "POST /v1/order/new": [{ "network": "pre_send", "message": message }],
    }));
    let creds = deltabadger::crypto::Credentials { ibkr: None, key: "KEY-A".into(), secret: "s".into(), passphrase: None, redaction_values: vec![] };
    GeminiVenue::new(t, deltabadger::venue::gemini::URL).with_pairs(Pairs::load(&i.o.primary, exchange_id(i)).unwrap()).with_identity(&creds)
}

/// What the engine kept: the bot's status and transient data, its rows and its log, the raw text and the install's own
/// key digest replaced so that two twins compare.
fn state(i: &Install, raw: &str) -> String {
    let esc = serde_json::to_string(raw).unwrap();
    let hide = |v: String| v.replace(raw, "RAW").replace(&esc[1..esc.len() - 1], "RAW");
    let q = |sql: &str| -> Vec<String> {
        let mut st = i.o.primary.prepare(sql).unwrap();
        st.query_map([i.bot], |r| r.get::<_, Option<String>>(0)).unwrap().map(|v| hide(v.unwrap().unwrap_or_default())).collect()
    };
    let bot = model::load_bot(&i.o.primary, i.bot).unwrap();
    let mut out = format!("status {:?}\ntransient {}\nrows {:?}\nlogs {:?}", bot.status, hide(serde_json::to_string(&bot.transient).unwrap()),
                          q("SELECT status || ' ' || coalesce(error_messages, '') FROM transactions WHERE bot_id = ?1 ORDER BY id"),
                          q("SELECT event || ' ' || level || ' ' || details FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id"));
    if let Some(digest) = bot.transient["rust_failure_origin"]["ciphertext_digest"].as_str() { out = out.replace(digest, "DIGEST"); }
    out
}

/// Two installs ticked at +0, +1 and +26 h, one refused with `classified`, its twin with `unclassified` (same shape):
/// the engine keeps exactly the same, the raw text aside.
async fn twins<V: Venue>(exchange_type: &str, name: &str, venue: impl Fn(&Install, &str) -> V, classified: &str, unclassified: &str) -> Install {
    let (big, other) = (install_as(exchange_type, name), install_as(exchange_type, name));
    for hours in [0, 1, 26] {
        let now = at(T0) + Duration::hours(hours);
        for (i, raw) in [(&big, classified), (&other, unclassified)] {
            tick::tick(&i.o.primary, &venue(i, raw), i.bot, &FixedClock(now), &mut Attempts::default()).await.unwrap();
            let (stored, esc) = (state(i, "\u{0}"), serde_json::to_string(raw).unwrap());
            assert!(stored.contains(raw) || stored.contains(&esc[1..esc.len() - 1]), "+{hours} h: the raw text is what is stored: {stored}");
        }
        assert_eq!(state(&big, classified), state(&other, unclassified), "+{hours} h");
    }
    big
}

/// The status bar, the bot page, its orders list, the mail and the desktop notice read `sentence` and never `raw`.
async fn every_surface_reads(i: &Install, sentence: &str, raw: &str) {
    let app = common::web::app(i.dir.path(), common::web::SECRET, common::web::TestClock::at(T0));
    let bar = deltabadger::web::bot::live::render(&i.o.primary, &app, &EngineEvent::TickEnded { bot_id: i.bot }).unwrap()
        .into_iter().map(|(_, html)| html).find(|html| html.contains("target=\"status_bar_")).expect("a status bar");
    assert!(bar.contains(sentence) && !bar.contains(raw), "{bar}");

    i.o.primary.execute("UPDATE users SET confirmed_at = '2026-01-01 00:00:00'", []).unwrap();
    let hash: String = i.o.primary.query_row("SELECT encrypted_password FROM users WHERE id = ?1", [i.s.user_id], |r| r.get(0)).unwrap();
    use deltabadger::web::session;
    let data = session::SessionData { user: Some((i.s.user_id, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    let browser = || common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    let page = browser().send(&app, "GET", &format!("/bots/{}", i.bot), None, common::web::Csrf::None, &[]).await;
    assert_eq!(page.status, 200, "{:?}", page.header("location"));
    assert!(page.body.contains(sentence) && !page.body.contains(raw), "{}", page.body);
    let feed = browser().send(&app, "GET", &format!("/bots/{}.turbo_stream", i.bot), None, common::web::Csrf::None, &[("Turbo-Frame", "orders_pagination")]).await;
    assert!(feed.body.contains(sentence) && !feed.body.contains(raw), "{}", feed.body);

    let bot = model::load_bot(&i.o.primary, i.bot).unwrap();
    let pending = notice::pending_in(i.bot, bot.transient.as_object().unwrap());
    let owed = pending.iter().find(|p| matches!(p.notice, notice::Notice::Error { .. })).unwrap_or_else(|| panic!("an error notice: {:?}", bot.transient));
    let mail = render::for_notice(&i.o.primary, i.bot, &owed.notice, "bot@example.com", &Urls::from_env(&|_| None)).unwrap().expect("a mail");
    assert!(mail.html.contains(sentence) && !mail.html.contains(raw), "{}", mail.html);
    let facts = render::facts(&i.o.primary, i.bot, &owed.notice).unwrap().expect("facts");
    let (title, body) = render::desktop_text(&facts, &owed.notice);
    assert!(body.contains(sentence) && !body.contains(raw), "{title} / {body}");
}

#[tokio::test(flavor = "current_thread")]
async fn group_1_a_bitget_buy_refused_as_too_small_is_handled_as_before_and_reads_the_sentence() {
    let refuse = |code: &str, msg: &str| json!({ "code": code, "msg": msg, "requestTime": 1788000000000u64, "data": null });
    let venue = |i: &Install, raw: &str| bitget(i, serde_json::from_str(raw).unwrap());
    let small = refuse("45110", "less than the minimum order quantity").to_string();
    let other = refuse("43999", "an answer no table names").to_string();
    let i = twins("Exchanges::Bitget", "Bitget", venue, &small, &other).await;
    every_surface_reads(&i, &fill(SMALL_EN, "Bitget"), "minimum order quantity").await;
}

#[tokio::test(flavor = "current_thread")]
async fn group_3_a_binance_size_filter_failure_is_handled_as_before_and_reads_the_sentence() {
    let venue = |i: &Install, raw: &str| binance(i, json!({ "code": -1013, "msg": raw }));
    let i = twins("Exchanges::Binance", "Binance", venue, "Filter failure: NOTIONAL", "Filter failure: NO_TABLE_NAMES_THIS").await;
    every_surface_reads(&i, &fill(RANGE_EN, "Binance"), "Filter failure").await;
}

#[tokio::test(flavor = "current_thread")]
async fn group_4_a_gemini_json_refusal_is_handled_as_before_and_reads_its_message() {
    let venue = |i: &Install, raw: &str| gemini(i, serde_json::from_str(raw).unwrap());
    let body = json!({ "result": "error", "reason": "InvalidPrice", "message": "Invalid price for symbol BTCEUR" }).to_string();
    let other = json!({ "result": "error", "reason": "NoTableNamesThis" }).to_string();
    let i = twins("Exchanges::Gemini", "Gemini", venue, &body, &other).await;
    every_surface_reads(&i, "Invalid price for symbol BTCEUR", "InvalidPrice").await;
}

/// Group 2: an unclassified twin differs by design (the engine retries a transient text), so the proof is the state the
/// base commit (bfec11581, before D-RAW-REFUSALS-COPY) stored for the same run, pinned as a literal.
#[tokio::test(flavor = "current_thread")]
async fn group_2_a_refused_connection_is_handled_as_before_and_reads_temporarily_unavailable() {
    const RAW: &str = r#"Connection refused - connect(2) for "api.gemini.com" port 443"#;
    let i = install_as("Exchanges::Gemini", "Gemini");
    let mut attempts = Attempts::default();
    let mut outcomes = vec![];
    for hours in [0, 1, 26] {
        let mut now = at(T0) + Duration::hours(hours);
        loop {
            let outcome = tick::tick(&i.o.primary, &gemini_unreachable(&i, RAW), i.bot, &FixedClock(now), &mut attempts).await.unwrap();
            outcomes.push(format!("{outcome:?}"));
            match outcome { TickOutcome::RetryAfter(wait) => now += Duration::from_std(wait).unwrap(), _ => break }
        }
    }
    let got = format!("outcomes {outcomes:?}\n{}", state(&i, RAW));
    assert_eq!(got, GROUP_2_GOLDEN, "the engine's handling changed");
    every_surface_reads(&i, &fill(DOWN_EN, "Gemini"), "Connection refused").await;
}

const GROUP_2_GOLDEN: &str = r##"outcomes ["Rescheduled", "Rescheduled", "Rescheduled"]
status Retrying
transient {"last_action_job_at":"2026-09-02T12:00:00.500Z","tick_generation":3,"rust_failure_origin":{"key_id":1,"ciphertext_digest":"DIGEST"},"failure_notifications":{"unknown":"2026-09-02T12:00:00Z"},"rust_error_mail_pending":{"unknown":{"error":"RAW","stamped_at":"2026-09-02T12:00:00.500Z"}},"rust_defer_until":{"until":"2026-09-02T13:00:00.000000Z","schedule":"1788256800000000/Seconds(3600.0)","origin":"venue","producer":{"key_id":1,"ciphertext_digest":"DIGEST"}}}
rows ["1 [\"RAW\"]", "1 [\"RAW\"]", "1 [\"RAW\"]"]
logs []"##;
