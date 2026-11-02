//! A14 (PLAN-A14-splits-restated.md): Rust-only engine tests for splits. Q1 case 2 (an all-crypto bot whose history holds a
//! stock), the Q4 races (a split stored mid-tick, a reverse split leaving a fraction), and the rebalance and redeploy
//! quarantine of the Q2 table.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::rebalance::{self, Outcome};
use deltabadger::engine::tick::{self, Attempts, PriceCache, TickContext};
use deltabadger::engine::{eligibility, model, redeploy, FixedClock};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use rusqlite::params;
use serde_json::{json, Value};

/// Tuesday 2026-09-08 14:00:00.5 UTC, a week after the bots start: one weekly buy is due.
const NOW: &str = "2026-09-08T14:00:00.5Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn venue<T: Transport>(t: T) -> AlpacaVenue<T> { AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))) }

/// A filled buy of `qty` at 100, on 2026-09-01 14:00:01.
fn filled(id: &str, qty: &'static str, quote: &'static str) -> TxSpec {
    TxSpec { status: 0, external_status: Some(2), external_id: Some(id.into()), order_type: 0, amount: Some(qty), quote_amount: Some(quote),
             price: Some("100"), amount_exec: Some(qty), quote_amount_exec: Some(quote), created_at: "2026-09-01 14:00:01".into() }
}
/// A split row that passes `row_verdict`: `removed` shares out, `net` shares in, at `ratio`.
fn trusted_split(c: &rusqlite::Connection, s: &seed::Seeded, sym: &str, when: &str, ratio: &str, removed: &str, net: &str) -> i64 {
    let row = seed::insert_split(c, s, sym, when, Some(ratio));
    let raw = json!({ "corporate_action": "split", "qty": format!("-{removed}"), "split_ratio": ratio, "merged_activity_ids": ["remove", "add"] });
    c.execute("UPDATE account_transactions SET base_amount = ?1, raw_data = ?2 WHERE id = ?3", params![net, raw.to_string(), row]).unwrap();
    row
}
/// An open clock, AAPL and BTC/USD quoted, a funded account, orders accepted, and these positions.
fn script(positions: Value) -> ScriptedTransport {
    ScriptedTransport::from_script(&json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-08T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "GET /v2/stocks/AAPL/quotes/latest": ok(json!({ "quote": { "ap": 187.43, "bp": 187.4 } })),
        "GET /v2/stocks/AAPL/trades/latest": ok(json!({ "trade": { "p": 187.41 } })),
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 } } })),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": { "BTC/USD": { "p": 64000 } } })),
        "POST /v2/orders": (1..=3).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(positions),
    }))
}
fn position(sym: &str, qty: &str) -> Value { json!({ "symbol": sym, "asset_class": "us_equity", "qty": qty, "qty_available": qty }) }
async fn tick_at<T: Transport>(o: &store::Opened, v: &AlpacaVenue<T>, id: i64, now: DateTime<Utc>) -> tick::TickOutcome {
    seed::fresh_stock_jobs(&o.primary, now);
    tick::tick(&o.primary, v, id, &FixedClock(now), &mut Attempts::default()).await.unwrap()
}
fn events(o: &store::Opened, id: i64) -> Vec<String> {
    o.primary.prepare("SELECT event FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
fn hold(o: &store::Opened, id: i64) -> Option<Value> {
    model::load_bot(&o.primary, id).unwrap().transient.get("rust_split_hold").filter(|v| !v.is_null()).cloned()
}

// --- Q1 case 2: an all-crypto bot whose history holds a stock ---

/// A BTC-only basket (allocations and member rows) on Alpaca whose history keeps 0.2 AAPL bought for 20 USD on 09-01 (less
/// than the week's 60, so a buy is still owed), restated once, and an AAPL 4:1 split dated `split_at` (0.2 out, 0.6 net in:
/// 0.8 shares).
fn crypto_bot_with_stock_history(split_at: &str) -> (tempfile::TempDir, store::Opened, i64) {
    let (d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(s.btc, 1.0)]));
    seed::insert_member(&o.primary, id, s.btc, s.ticker_id, 1.0, true, "2026-09-01 14:00:00", None);
    seed::insert_stock_tx(&o.primary, &s, id, aapl, "AAPL", &filled("held-aapl", "0.2", "20"));
    trusted_split(&o.primary, &s, "AAPL", split_at, "4:1", "0.2", "0.6");
    o.primary.execute("UPDATE bots SET restatement_generation = 1 WHERE id = ?1", [id]).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(model::all_crypto(&o.primary, &bot).unwrap(), "no composition row names AAPL");
    assert_eq!(eligibility::bot_reasons(&o.primary, &bot).unwrap(), Vec::<String>::new(), "neither the generation nor the split refuses it");
    (d, o, id)
}

#[tokio::test(flavor = "current_thread")]
async fn an_all_crypto_bot_with_a_recent_split_in_its_stock_history_stands_down() {
    let (_d, o, id) = crypto_bot_with_stock_history("2026-09-07 14:00:00");
    let t = script(json!([position("AAPL", "0.8")]));
    let out = tick_at(&o, &venue(t.clone()), id, at(NOW)).await;
    assert!(matches!(out, tick::TickOutcome::Done { placed: false }), "{out:?}");
    assert!(events(&o, id).contains(&"dca_skipped_restatement".to_string()), "{:?}", events(&o, id));
    assert!(t.posted_orders().is_empty());
    assert_eq!(hold(&o, id), None, "the quarantine is a stand-down, not a hold");
}

#[tokio::test(flavor = "current_thread")]
async fn an_all_crypto_bot_with_a_settled_split_the_venue_agrees_with_buys_its_coin() {
    let (_d, o, id) = crypto_bot_with_stock_history("2026-09-05 00:00:00");
    let t = script(json!([position("AAPL", "0.8")]));
    tick_at(&o, &venue(t.clone()), id, at(NOW)).await;
    assert_eq!(hold(&o, id), None);
    assert!(!events(&o, id).contains(&"dca_skipped_restatement".to_string()), "{:?}", events(&o, id));
    let posted = t.posted_orders();
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(posted[0]["symbol"], "BTC/USD");
}

#[tokio::test(flavor = "current_thread")]
async fn an_all_crypto_bot_with_a_settled_split_the_venue_disagrees_with_holds() {
    let (_d, o, id) = crypto_bot_with_stock_history("2026-09-05 00:00:00");
    let t = script(json!([position("AAPL", "0.2")]));
    tick_at(&o, &venue(t.clone()), id, at(NOW)).await;
    let reason = hold(&o, id).expect("rust_split_hold")["reason"].as_str().unwrap().to_string();
    assert!(reason.contains("AAPL") && reason.contains("restated account quantity 0.8") && reason.contains("venue quantity 0.2"), "{reason}");
    assert!(events(&o, id).contains(&"dca_skipped_restatement".to_string()));
    assert!(t.posted_orders().is_empty());
}

// --- Q4: a split stored mid-tick ---

/// Bumps the bot's restatement generation, once, when the tick reads the coin's price to size its buy: after the split
/// reconciliation took its snapshot and before the placement begins.
struct BumpOnPrice { inner: ScriptedTransport, db: std::path::PathBuf, bot: i64, armed: std::cell::Cell<bool> }
impl Transport for BumpOnPrice {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if self.armed.get() && r.path.starts_with("/v1beta3/crypto/us/latest/") {
            self.armed.set(false);
            rusqlite::Connection::open(&self.db).unwrap()
                .execute("UPDATE bots SET restatement_generation = restatement_generation + 1 WHERE id = ?1", [self.bot]).unwrap();
        }
        self.inner.send(r).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_split_stored_mid_tick_fences_the_placement_and_the_next_tick_buys_once() {
    let (_d, o, s) = common::install_alpaca();
    let c = &o.primary;
    // The newly admitted class: a BTC/USD basket holding the coin, beside Alpaca's BTC security, whose split is recorded.
    c.execute("INSERT INTO assets (external_id, symbol, name, category, instrument_type, created_at, updated_at) VALUES ('BTC.US', 'BTC', 'BTC', 'Stock', 'stock', ?1, ?1)",
              ["2026-01-01 00:00:00"]).unwrap();
    let security = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
               VALUES (?1, 'BTC', 'BTC', 'USD', ?2, ?3, 9, 2, 2, '0.000000001', '1', 1, 1, '2026-01-01 00:00:00', ?4)",
              params![s.exchange_id, security, s.quote, seed::SYNCED]).unwrap();
    let id = seed::insert_bot(c, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00"));
    seed::insert_tx(c, &s, id, &filled("held-btc", "0.001", "64"));
    let row = trusted_split(c, &s, "BTC", "2026-09-07 14:00:00", "2:1", "1", "1");
    c.execute("UPDATE account_transactions SET base_asset_id = ?1 WHERE id = ?2", [security, row]).unwrap();
    c.execute("UPDATE bots SET restatement_generation = 1 WHERE id = ?1", [id]).unwrap();
    let bot = model::load_bot(c, id).unwrap();
    assert!(model::all_crypto(c, &bot).unwrap());
    assert_eq!(eligibility::bot_reasons(c, &bot).unwrap(), Vec::<String>::new());

    let t = script(json!([]));
    let racing = BumpOnPrice { inner: t.clone(), db: c.path().unwrap().into(), bot: id, armed: true.into() };
    tick_at(&o, &venue(racing), id, at(NOW)).await;
    let generation: i64 = c.query_row("SELECT restatement_generation FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(generation, 2, "the bump ran mid-tick");
    assert!(t.posted_orders().is_empty(), "sized before the split was stored: never sent");
    assert!(!events(&o, id).contains(&"dca_skipped_restatement".to_string()), "refused at the placement fence, not by the reconciliation");
    assert!(model::load_bot(c, id).unwrap().rust_placement().is_none());

    tick_at(&o, &venue(t.clone()), id, at(NOW)).await;
    let posted = t.posted_orders();
    assert_eq!(posted.len(), 1, "re-sized and placed once: {posted:?}");
    assert_eq!(posted[0]["symbol"], "BTC/USD");
    assert_eq!(hold(&o, id), None, "a security's split never restates the coin");
}

// --- Q4: a reverse split leaving a fraction ---

/// An AAPL bot holding 0.5 shares, with a settled 1:3 reverse split (0.5 out, 0.166666667 in).
fn reverse_split_bot() -> (tempfile::TempDir, store::Opened, i64) {
    let (d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(aapl, 1.0)]));
    seed::insert_stock_tx(&o.primary, &s, id, aapl, "AAPL", &filled("held-aapl", "0.5", "50"));
    trusted_split(&o.primary, &s, "AAPL", "2026-09-05 00:00:00", "1:3", "0.5", "-0.333333333");
    (d, o, id)
}

#[tokio::test(flavor = "current_thread")]
async fn a_reverse_split_within_one_share_quantum_of_the_venue_trades() {
    let (_d, o, id) = reverse_split_bot();
    let t = script(json!([position("AAPL", "0.166666666")]));
    tick_at(&o, &venue(t.clone()), id, at(NOW)).await;
    assert_eq!(hold(&o, id), None);
    let posted = t.posted_orders();
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(posted[0]["symbol"], "AAPL");
}

#[tokio::test(flavor = "current_thread")]
async fn a_reverse_split_whose_fraction_the_venue_cashed_out_holds() {
    let (_d, o, id) = reverse_split_bot();
    let t = script(json!([]));
    tick_at(&o, &venue(t.clone()), id, at(NOW)).await;
    let reason = hold(&o, id).expect("rust_split_hold")["reason"].as_str().unwrap().to_string();
    assert!(reason.contains("AAPL: restated account quantity 0.1666") && reason.contains("venue quantity 0.0"), "{reason}");
    assert!(t.posted_orders().is_empty());
}

// --- Q2: rebalance and redeploy inside the quarantine ---

const REBALANCE_AT: &str = "2026-09-01T16:30:00Z";
/// A stopped AAA/BBB 50/50 basket, rebalancing on, holding 10 AAA and 3 BBB (bought at 100 on 08-01), with AAA's 3:1 split
/// dated `split_at` (30 AAA, far over target), and the venue holding the restated 30 AAA and 3 BBB.
fn rebalancing_basket(split_at: &str) -> (tempfile::TempDir, store::Opened, i64, ScriptedTransport) {
    let (d, o, s) = common::install_alpaca();
    let c = &o.primary;
    let (aaa, _) = seed::add_alpaca_stock(c, &s, "AAA");
    let (bbb, _) = seed::add_alpaca_stock(c, &s, "BBB");
    let spec = BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-08-01 14:00:00") }.with("rebalance_enabled", json!(true));
    let id = seed::insert_bot(c, &s, &spec.weights(&[(aaa, 0.5), (bbb, 0.5)]));
    c.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) \
               SELECT exchange_id, base_asset_id, 1, created_at, updated_at FROM tickers WHERE quote = 'USD' AND ticker NOT LIKE '%/%'", []).unwrap();
    deltabadger::engine::basket::refresh_composition(c, &model::load_bot(c, id).unwrap(), at("2026-08-01T13:00:00Z")).unwrap().unwrap();
    for (asset, sym, qty, quote) in [(aaa, "AAA", "10", "1000"), (bbb, "BBB", "3", "300")] {
        seed::insert_stock_tx(c, &s, id, asset, sym, &TxSpec { created_at: "2026-08-01 14:00:00".into(), ..filled(&format!("held-{sym}"), qty, quote) });
    }
    trusted_split(c, &s, "AAA", split_at, "3:1", "10", "20");
    seed::fresh_stock_jobs(c, at(REBALANCE_AT));
    let snapshots = json!({ "AAA": { "latestTrade": { "p": 100 } }, "BBB": { "latestTrade": { "p": 100 } } });
    let mut m = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T12:30:00-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": ok(json!({ "id": "OTX-1", "status": "accepted" })),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([position("AAA", "30"), position("BBB", "3")])),
        "GET /v2/stocks/snapshots": ok(snapshots),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": {} })),
    });
    for sym in ["AAA", "BBB"] { m[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": 100, "bp": 100 } })); }
    (d, o, id, ScriptedTransport::from_script(&m))
}
fn context<'a>(o: &store::Opened, id: i64, prices: &'a PriceCache) -> TickContext<'a> {
    let bot = model::load_bot(&o.primary, id).unwrap();
    TickContext { credential_version: model::credential_version(&o.primary, &bot).unwrap(), prices, process_start: DateTime::<Utc>::MIN_UTC,
                  stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} }
}
async fn rebalance_pass(o: &store::Opened, t: &ScriptedTransport, id: i64) -> Outcome {
    let prices = PriceCache::default();
    rebalance::run(&o.primary, &venue(t.clone()), id, &FixedClock(at(REBALANCE_AT)), &context(o, id, &prices)).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_rebalance_inside_a_members_split_quarantine_places_nothing() {
    // Control: the same basket with the split settled swaps.
    let (_d, o, id, t) = rebalancing_basket("2026-08-05 00:00:00");
    let out = rebalance_pass(&o, &t, id).await;
    assert!(matches!(out, Outcome::Placed(_)), "{out:?}");
    assert_eq!(t.posted_orders()[0]["symbol"], "AAA");

    let (_d, o, id, t) = rebalancing_basket("2026-08-31 16:30:00");
    assert_eq!(rebalance_pass(&o, &t, id).await, Outcome::Skipped("split_hold"));
    assert!(t.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary, id).unwrap().transient.get("rebalance_pending").is_none_or(Value::is_null));
}

fn redeploy_reasons(o: &store::Opened, id: i64) -> Vec<Value> {
    o.primary.prepare("SELECT json_extract(details, '$.reason') FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'redeploy_not_started' ORDER BY id").unwrap()
        .query_map([id], |r| r.get::<_, String>(0)).unwrap().map(|v| serde_json::from_str(&v.unwrap()).unwrap()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_redeploy_inside_a_held_stocks_split_quarantine_holds() {
    let (_d, o, id, t) = rebalancing_basket("2026-08-31 16:30:00");
    let prices = PriceCache::default();
    redeploy::run(&o.primary, &venue(t.clone()), id, &FixedClock(at(REBALANCE_AT)), &context(&o, id, &prices)).await.unwrap();
    assert_eq!(redeploy_reasons(&o, id), vec![json!(["split_hold"])]);
    assert!(t.posted_orders().is_empty());

    // Control: settled and reconciled, the split gate lets the redeploy through to its own refusals.
    let (_d, o, id, t) = rebalancing_basket("2026-08-05 00:00:00");
    redeploy::run(&o.primary, &venue(t.clone()), id, &FixedClock(at(REBALANCE_AT)), &context(&o, id, &prices)).await.unwrap();
    assert_eq!(redeploy_reasons(&o, id), vec![json!(["nothing_to_redeploy"])], "past the split gate, refused only for having no offer");
}
