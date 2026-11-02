//! D-KRAKEN-TOKENIZED (plan A15): a Kraken xStock pair (`NVDAxUSD`, base code `NVDAx`) traded by Rust. Rails never sends
//! one (AddOrder without `asset_class`, the pair listed untradable), so nothing here is a recorded Rails vector. Each venue
//! answer is scripted from Kraken's REST docs (AddOrder `asset_class=tokenized_asset` for xStocks, read 2026-11-02) and
//! reviewed by hand. Every case runs the real Kraken adapter over ScriptedTransport, through the real tick.
//! Kraken's live behaviour is confirmed after merge on an own monitored account, not here.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::model;
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::FixedClock;
use deltabadger::enums::BotStatus;
use deltabadger::store::{self, Paths};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::{NewOrder, OrderKind, Side, Venue};
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }

/// The Kraken seed turned into an xStock bot: NVIDIA xStock (a tokenized wrapper, as the catalogue files it) over USD on
/// Kraken's own pair and code, with Kraken's xStock minimums (ordermin 1e-8, costmin 5 USD, lot 8, cost 5, pair 2).
fn xstock_bot(quote: f64) -> (tempfile::TempDir, store::Opened, i64) {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute_batch(&format!(
        "UPDATE assets SET external_id = 'nvidia-xstock', symbol = 'NVDAX', name = 'NVIDIA xStock', instrument_type = 'tokenized' WHERE id = {b};
         UPDATE assets SET external_id = 'usd', symbol = 'USD', name = 'US Dollar', category = 'Fiat' WHERE id = {q};
         UPDATE tickers SET ticker = 'NVDAxUSD', base = 'NVDAx', quote = 'USD', base_decimals = 8, quote_decimals = 5, price_decimals = 2,
                            minimum_base_size = '0.00000001', minimum_quote_size = '5' WHERE id = {t};",
        b = s.btc, q = s.quote, t = s.ticker_id)).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(quote, "2026-09-01 10:00:00"));
    (dir, o, id)
}

/// Kraken's Ticker for the pair, a funded USD balance, and `adds` as AddOrder's answers in turn.
fn kraken(adds: Vec<Value>) -> (ScriptedTransport, KrakenVenue<ScriptedTransport>) {
    let ticker = json!({ "error": [], "result": { "NVDAxUSD": { "a": ["180.00", "1", "1.000"], "b": ["179.90", "1", "1.000"], "c": ["179.95", "0.1"],
        "v": ["1", "1"], "p": ["1", "1"], "t": [1, 1], "l": ["1", "1"], "h": ["1", "1"], "o": "1" } } });
    let t = ScriptedTransport::from_script(&json!({
        "GET /0/public/Ticker": [ok(ticker)],
        "POST /0/private/BalanceEx": [ok(json!({ "error": [], "result": { "ZUSD": { "balance": "1000", "hold_trade": "0" } } }))],
        "POST /0/private/AddOrder": adds,
    }));
    (t.clone(), KrakenVenue::new(t))
}

async fn run(o: &store::Opened, v: &KrakenVenue<ScriptedTransport>, id: i64, when: &str) -> TickOutcome {
    tick::tick(&o.primary, v, id, &FixedClock(at(when)), &mut Attempts::default()).await.unwrap()
}
fn add_orders(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.path == "/0/private/AddOrder").map(|r| r.body.unwrap()).collect()
}
fn rows(o: &store::Opened) -> Vec<(i64, Option<String>)> {
    o.primary.prepare("SELECT status, external_id FROM transactions ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
fn events(o: &store::Opened) -> Vec<String> {
    o.primary.prepare("SELECT event FROM bot_activity_logs ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

fn order(pair: &str, kind: OrderKind, quote: bool) -> NewOrder {
    NewOrder { pair: pair.into(), side: Side::Buy, kind, volume: "10.0".into(), quote_volume: quote, cl_ord_id: "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01".into(),
               deadline: at("2026-09-01T10:00:10.123456Z"), day: false }
}

// ---- the wire

#[tokio::test(flavor = "current_thread")]
async fn an_xstock_order_carries_asset_class_on_market_and_limit_and_no_other_pair_does() {
    let txid = || ok(json!({ "error": [], "result": { "txid": ["OX-1"] } }));
    let send = |o: NewOrder| async move {
        let t = ScriptedTransport::from_script(&json!({ "POST /0/private/AddOrder": [txid()] }));
        KrakenVenue::new(t.clone()).add_order(&o).await.unwrap();
        t.requests()[0].body.as_ref().unwrap().to_string()
    };
    assert_eq!(send(order("NVDAxUSD", OrderKind::Market, true)).await,
        r#"{"ordertype":"market","type":"buy","volume":"10.0","pair":"NVDAxUSD","asset_class":"tokenized_asset","cl_ord_id":"5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01","oflags":"viqc","deadline":"2026-09-01T10:00:10.123Z"}"#);
    assert_eq!(send(order("NVDAxUSD", OrderKind::Limit { price: "179.95".into() }, false)).await,
        r#"{"ordertype":"limit","type":"buy","volume":"10.0","pair":"NVDAxUSD","asset_class":"tokenized_asset","cl_ord_id":"5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01","price":"179.95","deadline":"2026-09-01T10:00:10.123Z"}"#);
    // A currency pair, and a gold token the catalogue marks tokenized but Kraken lists as currency: no asset_class.
    for pair in ["XBTEUR", "PAXGUSD", "XAUTUSDT"] {
        assert!(!send(order(pair, OrderKind::Market, true)).await.contains("asset_class"), "{pair}");
    }
}

// ---- the tick, through the real adapter

#[tokio::test(flavor = "current_thread")]
async fn an_xstock_bot_places_its_buy_with_asset_class_and_stores_the_txid() {
    let (_d, o, id) = xstock_bot(60.0);
    let (t, v) = kraken(vec![ok(json!({ "error": [], "result": { "txid": ["OX-NVDA-1"] } }))]);
    let out = run(&o, &v, id, "2026-09-01T10:00:00.5Z").await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    let sent = add_orders(&t);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!((sent[0]["pair"].as_str(), sent[0]["asset_class"].as_str()), (Some("NVDAxUSD"), Some("tokenized_asset")), "{sent:?}");
    assert_eq!(rows(&o), vec![(0, Some("OX-NVDA-1".into()))]);
    // The price the buy was sized at was read with the class too.
    let priced: Vec<_> = t.requests().into_iter().filter(|r| r.path == "/0/public/Ticker").map(|r| r.query).collect();
    assert!(!priced.is_empty() && priced.iter().all(|q| q.contains(&("asset_class", "tokenized_asset".to_string()))), "{priced:?}");
}

/// The region case: Kraken refuses an account that may not trade xStocks. The existing `restricted` outcome, never a silent
/// skip or a retry loop: the first refusal is a failed row with Kraken's words and a failure kind; the second in a row
/// stops the bot with the region copy (bot.status.stopped_by_error.restricted).
#[tokio::test(flavor = "current_thread")]
async fn a_region_refusal_is_restricted_and_the_second_in_a_row_stops_the_bot() {
    let (_d, o, id) = xstock_bot(60.0);
    let refused = || ok(json!({ "error": ["EAccount:Invalid permissions"] }));
    let (_t, v) = kraken(vec![refused(), refused()]);
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:00.5Z").await, TickOutcome::Rescheduled));
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.last_failure_kind().as_deref(), Some("restricted"));
    assert_ne!(b.status, BotStatus::Stopped);
    let errors: String = one(&o, "SELECT error_messages FROM transactions ORDER BY id DESC LIMIT 1");
    assert!(errors.contains("EAccount:Invalid permissions"), "{errors}");
    assert!(matches!(run(&o, &v, id, "2026-09-08T10:00:00.5Z").await, TickOutcome::Stopped));
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
    assert_eq!(one::<String>(&o, &format!("SELECT stop_message_key FROM bots WHERE id = {id}")), "bot.status.stopped_by_error.restricted");
}

#[tokio::test(flavor = "current_thread")]
async fn insufficient_funds_on_an_xstock_is_the_funds_failure_and_the_bot_keeps_running() {
    let (_d, o, id) = xstock_bot(60.0);
    let (_t, v) = kraken(vec![ok(json!({ "error": ["EOrder:Insufficient funds"] }))]);
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:00.5Z").await, TickOutcome::Rescheduled));
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.last_failure_kind().as_deref(), Some("insufficient_funds"));
    assert_ne!(b.status, BotStatus::Stopped);
    assert_eq!(rows(&o).len(), 1, "one failed row");
}

#[tokio::test(flavor = "current_thread")]
async fn an_xstock_minimum_is_a_skip_whether_kraken_or_the_pre_check_says_so() {
    // Kraken's answer (D-BELOW-MIN-REJECTION): a skipped row, never a failed one.
    let (_d, o, id) = xstock_bot(60.0);
    let (_t, v) = kraken(vec![ok(json!({ "error": ["EOrder:Order minimum not met"] }))]);
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:00.5Z").await, TickOutcome::Done { placed: false }));
    assert_eq!(rows(&o), vec![(2, None)], "one skipped row");
    assert!(events(&o).iter().any(|e| e == "order_skipped") && !events(&o).iter().any(|e| e == "execution_failed"), "{:?}", events(&o));
    // Under costmin (5 USD): nothing is sent.
    let (_d, o, id) = xstock_bot(3.0);
    let (t, v) = kraken(vec![]);
    run(&o, &v, id, "2026-09-01T10:00:00.5Z").await;
    assert!(add_orders(&t).is_empty());
    assert_eq!(rows(&o), vec![(2, None)], "skipped before the send");
}

// ---- prices and candles: the reads a holding's value and a chart need

#[tokio::test(flavor = "current_thread")]
async fn an_xstock_price_and_its_candles_are_asked_for_with_asset_class() {
    let ticker_body = |pair: &str, last: &str| ok(json!({ "error": [], "result": { pair: { "a": [last, "1", "1.000"], "b": [last, "1", "1.000"], "c": [last, "0.1"] } } }));
    // Bulk prices: the all-pairs read carries the currency class only; the xStock is then asked for alone, with its class.
    let t = ScriptedTransport::from_script(&json!({ "GET /0/public/Ticker": [ticker_body("XXBTZEUR", "50000.0"), ticker_body("NVDAxUSD", "180.00")] }));
    let prices = KrakenVenue::new(t.clone()).tickers_prices(&["XXBTZEUR".into(), "NVDAxUSD".into()]).await.unwrap();
    assert_eq!(prices, vec![("XXBTZEUR".to_string(), json!("50000.0")), ("NVDAxUSD".to_string(), json!("180.0"))]);
    let queries: Vec<Vec<(&str, String)>> = t.requests().into_iter().map(|r| r.query).collect();
    assert_eq!(queries, vec![vec![], vec![("pair", "NVDAxUSD".to_string()), ("asset_class", "tokenized_asset".to_string())]]);
    // The page reader's per-pair fallback (figures/page_market.rs) builds the same query.
    assert_eq!(deltabadger::venue::kraken::ticker_query("NVDAxUSD"), vec![("pair", "NVDAxUSD".to_string()), ("asset_class", "tokenized_asset".to_string())]);
    assert_eq!(deltabadger::venue::kraken::ticker_query("XBTUSD"), vec![("pair", "XBTUSD".to_string())]);
    // Candles, for the bot's conditions and the page charts (one request builder).
    let r = deltabadger::venue::kraken::ohlc_request("https://api.kraken.com", "NVDAxUSD", "1440", 1_756_000_000);
    assert!(r.query.contains(&("asset_class", "tokenized_asset".to_string())), "{:?}", r.query);
    let r = deltabadger::venue::kraken::ohlc_request("https://api.kraken.com", "XBTEUR", "1440", 1_756_000_000);
    assert!(!r.query.iter().any(|(k, _)| *k == "asset_class"), "{:?}", r.query);
}
