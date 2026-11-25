//! PLAN index-as-ingredient slices 1 and 2b: a basket stored with `index_groups` reads `rust_bot_group_members`, runs as
//! groups (one derivation per group, one closing write), and a `Bots::DcaIndex` row reads exactly what it read before.
mod common;
use common::seed::{self, BotSpec};
use chrono::{DateTime, Utc};
use deltabadger::engine::rebalance::{self, Outcome};
use deltabadger::engine::tick::{self, Attempts, PriceCache, TickContext, TickOutcome};
use deltabadger::engine::{composition, eligibility, index, model, placement, redeploy, staleness, FixedClock};
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use deltabadger::store;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

const T0: &str = "2026-09-01T14:00:00.5Z";
const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn exec(o: &store::Opened, sql: &str) { o.primary.execute_batch(sql).unwrap(); }

/// Five Alpaca stocks AAA..EEE in data-api's ND100 row (caps 5, 3, 2, 1, 0.5 T), BTC/USD beside them, a configured
/// provider (its index bot deleted, out of the way) and fresh catalogues.
struct U { _d: tempfile::TempDir, o: store::Opened, s: seed::Seeded, m: Vec<(i64, i64)> }
fn universe() -> U {
    let (d, o, s) = common::install_alpaca();
    let m: Vec<(i64, i64)> = SYMBOLS.iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                       &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
    let legacy = seed::index_bot(&o.primary, &s, "nasdaq-100", 3, 0.0, false);
    exec(&o, &format!("UPDATE bots SET status = 3 WHERE id = {legacy}"));
    seed::fresh_stock_jobs(&o.primary, at(T0));
    U { _d: d, o, s, m }
}
/// A group over `id`: top `n`, flattening 1.0 (equal weights) unless given, at ingredient weight `w`.
fn group(id: &str, n: i64, w: f64) -> Value { json!({ "index_type": "category", "index_category_id": id, "num_coins": n, "allocation_flattening": 1.0, "weight": w }) }
fn grouped_bot(u: &U, allocations: Value, groups: Value) -> i64 {
    seed::insert_bot(&u.o.primary, &u.s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", allocations).with("index_groups", groups))
}
/// Every stock quoted at `asks[i]` (0 = unpriced), BTC at 64000, an open clock, accepted orders, a funded account.
fn script(asks: [f64; 5]) -> ScriptedTransport { script_with(asks, json!({})) }
fn script_with(asks: [f64; 5], over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": (1..=8).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 } } })),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": { "BTC/USD": { "p": 64000 } } })),
        "GET /v2/stocks/snapshots": ok(json!({ "AAA": { "latestTrade": { "p": 100.0 } }, "BBB": { "latestTrade": { "p": 200.0 } } })),
    });
    for (sym, ask) in SYMBOLS.iter().zip(asks) { m[format!("GET /v2/stocks/{sym}/quotes/latest")] = Value::Array(vec![json!({ "status": 200, "body": { "quote": { "ap": ask, "bp": ask } } }); 4]); }
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue<T: Transport>(t: T) -> AlpacaVenue<T> { AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))) }
async fn refresh<T: Transport>(u: &U, t: T, id: i64) -> Result<(), index::Refusal> {
    index::refresh_composition(&u.o.primary, &venue(t), &model::load_bot(&u.o.primary, id).unwrap(), &FixedClock(at(T0)), &PriceCache::default()).await.unwrap()
}
/// in_index members: (symbol, target_allocation), in row order.
fn members(o: &store::Opened, id: i64) -> Vec<(String, f64)> {
    o.primary.prepare("SELECT a.symbol, m.target_allocation FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id WHERE m.bot_id = ?1 AND m.in_index = 1 ORDER BY m.id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
fn group_rows(o: &store::Opened, id: i64) -> Vec<(String, String, f64)> {
    o.primary.prepare("SELECT g.index_external_id, a.symbol, g.weight FROM rust_bot_group_members g JOIN assets a ON a.id = g.asset_id WHERE g.bot_id = ?1 ORDER BY g.index_external_id, g.asset_id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
}
fn composed_of(o: &store::Opened, id: i64) -> Value { model::load_bot(&o.primary, id).unwrap().transient.get(composition::COMPOSED_OF_KEY).cloned().unwrap_or(Value::Null) }
fn close(got: &[(String, f64)], want: &[(&str, f64)]) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for ((s, g), (ws, w)) in got.iter().zip(want) { assert!(s == ws && (g - w).abs() < 2e-6, "{got:?} vs {want:?}"); }
}

/// A transport that runs `hook` once, just before the first request to `path` is answered (a write landing mid-probe).
type Hook = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;
#[derive(Clone)]
struct Hooked { inner: ScriptedTransport, path: String, hook: Hook }
impl Transport for Hooked {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if r.path == self.path { if let Some(h) = self.hook.borrow_mut().take() { h(); } }
        self.inner.send(r).await
    }
}
fn hooked(t: &ScriptedTransport, path: &str, hook: impl FnOnce() + 'static) -> Hooked {
    Hooked { inner: t.clone(), path: path.into(), hook: Rc::new(RefCell::new(Some(Box::new(hook)))) }
}
fn side_conn(u: &U) -> rusqlite::Connection { rusqlite::Connection::open(u._d.path().join("production.sqlite3")).unwrap() }

// ---------------------------------------------------------------- slice 1: storage

#[test]
fn a_legacy_index_reads_its_key_and_in_index_rows_and_ignores_the_table() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("index_type", json!("category")).with("index_category_id", json!("layer-1")).transient("rust_composed_index", json!("layer-1")));
    exec(&o, &format!("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = {id};
        INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, target_allocation, created_at, updated_at) VALUES ({id}, {}, {}, 1, 1.0, '2026-01-01', '2026-01-01');
        INSERT INTO rust_bot_group_members VALUES ({id}, 'layer-1', 999, 998, 0, 1.0);", s.btc, s.ticker_id));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(!bot.stored_groups());
    assert!(index::composed_here(&o.primary, &bot, "layer-1").unwrap(), "the legacy key, not the table's composed = 0");
    assert!(!index::composed_here(&o.primary, &bot, "top-coins").unwrap());
    assert_eq!(index::incumbents(&o.primary, &bot, "layer-1").unwrap(), HashSet::from([s.ticker_id]), "the in_index rows, not the table's");
}

#[test]
fn a_grouped_bot_reads_the_table_by_group() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let eth_ticker: i64 = o.primary.query_row("SELECT id FROM tickers WHERE base_asset_id = ?1", [eth], |r| r.get(0)).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("rust_composed_index", json!("top-coins")));
    exec(&o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups', json('[{{\"index_type\":\"top\",\"num_coins\":3,\"weight\":0.5}}]')) WHERE id = {id}"));
    exec(&o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, target_allocation, created_at, updated_at) VALUES
          ({id}, {btc}, {bt}, 1, 0.5, '2026-01-01', '2026-01-01'), ({id}, {eth}, {et}, 1, 0.5, '2026-01-01', '2026-01-01');
        INSERT INTO rust_bot_group_members VALUES ({id}, 'top-coins', {btc}, {bt}, 1, 0.6), ({id}, 'top-coins', {eth}, 777, 1, 0.4),
                                                  ({id}, 'layer-1', {eth}, {et}, 0, 1.0);", btc = s.btc, bt = s.ticker_id, et = eth_ticker));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(bot.stored_groups() && bot.has_groups());
    assert!(index::composed_here(&o.primary, &bot, "top-coins").unwrap());
    assert!(!index::composed_here(&o.primary, &bot, "layer-1").unwrap(), "composed = 0");
    assert!(!index::composed_here(&o.primary, &bot, "defi").unwrap(), "no rows");
    assert_eq!(index::incumbents(&o.primary, &bot, "top-coins").unwrap(), HashSet::from([s.ticker_id]), "ETH's stored ticker is no longer its in_index one");
    assert_eq!(index::incumbents(&o.primary, &bot, "layer-1").unwrap(), HashSet::from([eth_ticker]), "each group its own rows");
    let snapshot = placement::composition_snapshot(&o.primary, &bot).unwrap();
    assert_eq!(snapshot["groups"], json!([["layer-1", eth, eth_ticker, 0, 1.0], ["top-coins", s.btc, s.ticker_id, 1, 0.6], ["top-coins", eth, 777, 1, 0.4]]));
    exec(&o, &format!("UPDATE rust_bot_group_members SET weight = 0.5 WHERE bot_id = {id} AND asset_id = {}", s.btc));
    let half = placement::composition_snapshot(&o.primary, &bot).unwrap();
    assert_ne!(half, snapshot, "a group row's change moves the snapshot");
    exec(&o, &format!("UPDATE rust_bot_group_members SET weight = 0.5000000000000001 WHERE bot_id = {id} AND asset_id = {}", s.btc));
    assert_ne!(placement::composition_snapshot(&o.primary, &bot).unwrap(), half, "0.5 and 0.5000000000000001 differ");
}

/// The snapshot of every bot not stored with `index_groups` is the same bytes as before the table, on a store that has
/// none (a 2.x copy no Rust migration ran on).
#[test]
fn a_legacy_snapshot_keeps_its_bytes_without_the_table() {
    let dir = common::v2_install_unmigrated();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert!(!c.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'rust_bot_group_members')", [], |r| r.get::<_, bool>(0)).unwrap());
    c.execute_batch("PRAGMA foreign_keys = OFF; INSERT INTO users (id, email, encrypted_password, created_at, updated_at) VALUES (1, 'o@example.com', 'x', '2026-01-01', '2026-01-01');
        INSERT INTO bots (id, type, status, user_id, exchange_id, settings, transient_data, created_at, updated_at) VALUES
          (1, 'Bots::DcaIndex', 2, 1, 7, '{\"index_type\":\"top\",\"num_coins\":3}', '{}', '2026-01-01', '2026-01-01'),
          (2, 'Bots::DcaMultiAsset', 2, 1, 7, '{\"allocations\":{\"5\":1.0}}', '{}', '2026-01-01', '2026-01-01');
        INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, target_allocation, created_at, updated_at) VALUES
          (1, 5, 6, 1, 0.25, '2026-01-01', '2026-01-01'), (2, 5, 6, 0, 1.0, '2026-01-01', '2026-01-01');").unwrap();
    let bytes = |id: i64| placement::composition_snapshot(&c, &model::load_bot(&c, id).unwrap()).unwrap().to_string();
    assert_eq!(bytes(1), r#"{"type":"Bots::DcaIndex","exchange":7,"settings":{"index_type":"top","num_coins":3},"members":[[5,6,0.25,1]]}"#);
    assert_eq!(bytes(2), r#"{"type":"Bots::DcaMultiAsset","exchange":7,"settings":{"allocations":{"5":1.0}},"members":[[5,6,1,0]]}"#);
}

// ---------------------------------------------------------------- 2b: composition

#[tokio::test(flavor = "current_thread")]
async fn btc_half_and_a_five_member_equal_index_split_fifty_and_ten_each() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 5, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("BTC", 0.5), ("AAA", 0.1), ("BBB", 0.1), ("CCC", 0.1), ("DDD", 0.1), ("EEE", 0.1)]);
    assert_eq!(group_rows(&u.o, id).len(), 5, "the group's own rows, weights within the group");
    assert!(group_rows(&u.o, id).iter().all(|(g, _, w)| g == "nasdaq-100" && (w - 0.2).abs() < 1e-12));
    let of = composed_of(&u.o, id);
    assert_eq!(of["digest"], json!(composition::digest(&model::load_bot(&u.o.primary, id).unwrap())));
    assert_eq!(of["shares"], json!({ "assets": { u.s.btc.to_string(): 0.5 }, "groups": { "nasdaq-100": 0.5 } }));
    // A tick buys BTC and the five stocks from those rows.
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick::tick(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(t.posted_orders().len(), 6);
}

#[tokio::test(flavor = "current_thread")]
async fn an_asset_named_and_in_the_index_holds_the_sum() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.m[0].0.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 0.75), ("BBB", 0.25)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_ranking_naming_an_asset_twice_counts_it_once_and_seats_n_distinct() {
    let u = universe();
    exec(&u.o, "UPDATE indices SET top_coins = '[\"AAA.US\",\"AAA.US\",\"BBB.US\",\"CCC.US\"]' WHERE external_id = 'nasdaq-100'");
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 0.5), ("BBB", 0.5)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_disabled_named_asset_drops_and_the_rest_renormalise() {
    let u = universe();
    exec(&u.o, &format!("UPDATE tickers SET trading_enabled = 0 WHERE id = {}", u.s.ticker_id));
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 0.5), ("BBB", 0.5)]);
    assert_eq!(composed_of(&u.o, id)["shares"]["groups"]["nasdaq-100"], json!(1.0));
}

#[tokio::test(flavor = "current_thread")]
async fn one_groups_refusal_writes_nothing_and_forgets_the_digest() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let (before, rows) = (members(&u.o, id), group_rows(&u.o, id));
    assert!(composed_of(&u.o, id).is_object());
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups', json('[{}, {}]')) WHERE id = {id}", group("nasdaq-100", 2, 0.5), group("gone", 2, 0.5)));
    let refused = refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await;
    assert_eq!(refused, Err(index::Refusal::Failure("Index not found".into())));
    assert_eq!((members(&u.o, id), group_rows(&u.o, id)), (before, rows), "nothing written");
    assert_eq!(composed_of(&u.o, id), Value::Null, "the refused refresh forgot the composition");
}

#[tokio::test(flavor = "current_thread")]
async fn a_cached_unpriced_newcomer_is_not_seated_by_a_second_group() {
    let u = universe();
    seed::insert_index(&u.o.primary, "tech", &["DDD.US", "EEE.US"], &json!({ "DDD.US": 1e12, "EEE.US": 5e11 }));
    // The first group probes DDD (unpriced) before seating EEE; the second asks the cache for DDD.
    exec(&u.o, "UPDATE indices SET top_coins = '[\"DDD.US\",\"EEE.US\"]' WHERE external_id = 'nasdaq-100'");
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 1, 0.5), group("tech", 1, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 0.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("EEE", 1.0)]);
}

// ---------------------------------------------------------------- 2b: the closing write

/// Codex r3 #3: ranking [A, B], one seat, A untradable; A becomes tradable while B is probed. The closing replay reaches A
/// with no probe of it this pass: CHANGED, nothing written; the retry probes A and seats it.
#[tokio::test(flavor = "current_thread")]
async fn a_newly_tradable_leader_during_a_probe_retries() {
    let u = universe();
    exec(&u.o, &format!("UPDATE indices SET top_coins = '[\"AAA.US\",\"BBB.US\"]' WHERE external_id = 'nasdaq-100'; UPDATE tickers SET trading_enabled = 0 WHERE id = {}", u.m[0].1));
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 1, 1.0)]));
    let side = side_conn(&u);
    let aaa = u.m[0].1;
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let first = refresh(&u, hooked(&t, "/v2/stocks/BBB/quotes/latest", move || { side.execute("UPDATE tickers SET trading_enabled = 1 WHERE id = ?1", [aaa]).unwrap(); }), id).await;
    assert_eq!(first, Err(index::Refusal::Transient(index::CHANGED.into())));
    assert!(members(&u.o, id).is_empty() && group_rows(&u.o, id).is_empty(), "nothing written");
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 1.0)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_cap_change_during_a_probe_is_blended_from_the_closing_caps() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([{ "index_type": "category", "index_category_id": "nasdaq-100", "num_coins": 2, "allocation_flattening": 0.0, "weight": 1.0 }]));
    let side = side_conn(&u);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    // AAA's own catalogue cap moves from none (the ranking's 5 T) to 1 T while BBB is probed.
    refresh(&u, hooked(&t, "/v2/stocks/BBB/quotes/latest", move || { side.execute("UPDATE assets SET market_cap = 1000000000000 WHERE external_id = 'AAA.US'", []).unwrap(); }), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 0.25), ("BBB", 0.75)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_group_row_insert_failure_rolls_back_members_and_digest() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    exec(&u.o, "CREATE TRIGGER fail_group BEFORE INSERT ON rust_bot_group_members BEGIN SELECT RAISE(ABORT, 'forced'); END;");
    let r = index::refresh_composition(&u.o.primary, &venue(script([100.0, 200.0, 300.0, 400.0, 500.0])), &model::load_bot(&u.o.primary, id).unwrap(),
                                       &FixedClock(at(T0)), &PriceCache::default()).await;
    assert!(r.is_err(), "{r:?}");
    assert!(members(&u.o, id).is_empty(), "the flat members rolled back with the group rows");
    assert_eq!(composed_of(&u.o, id), Value::Null);
}

// ---------------------------------------------------------------- 2b: frozen groups (D-INDEX-RETIRED-PAIR)

fn retire(u: &U, id: &str) { exec(&u.o, &format!("INSERT INTO rust_index_retired VALUES ('{id}', 'data-api', '2026-09-01T00:00:00Z')")); }

#[tokio::test(flavor = "current_thread")]
async fn a_retired_group_with_provenance_keeps_its_rows_beside_a_live_one() {
    let u = universe();
    seed::insert_index(&u.o.primary, "tech", &["DDD.US", "EEE.US"], &json!({ "DDD.US": 1e12, "EEE.US": 5e11 }));
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 0.5), group("tech", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    retire(&u, "tech");
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    refresh(&u, t.clone(), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 0.25), ("BBB", 0.25), ("DDD", 0.25), ("EEE", 0.25)]);
    assert_eq!(composed_of(&u.o, id)["ranked"]["tech"], Value::Null, "a frozen group has no ranked row");
    assert!(t.requests().iter().all(|r| !r.path.contains("/DDD/") && !r.path.contains("/EEE/")), "its members are not probed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_frozen_member_whose_pair_left_the_quote_drops_and_the_rest_renormalise() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    retire(&u, "nasdaq-100");
    exec(&u.o, &format!("UPDATE tickers SET available = 0 WHERE id = {}", u.m[1].1));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("AAA", 0.5), ("CCC", 0.5)]);
    // Every pair gone: the group has no valid member and is retired without provenance.
    exec(&u.o, &format!("UPDATE tickers SET available = 0 WHERE id IN ({}, {})", u.m[0].1, u.m[2].1));
    assert_eq!(refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await, Err(index::Refusal::Failure(index::RETIRED.into())));
}

/// Codex r3 #5: surviving frozen weights summing to 0 split equally (a grouped row; a legacy one never drops a member).
#[tokio::test(flavor = "current_thread")]
async fn zero_sum_surviving_frozen_weights_split_equally() {
    let u = universe();
    let grouped = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    exec(&u.o, &format!("INSERT INTO rust_bot_group_members VALUES ({grouped}, 'nasdaq-100', {a}, {at}, 1, 1.0), ({grouped}, 'nasdaq-100', {b}, {bt}, 1, 0.0), ({grouped}, 'nasdaq-100', {c}, {ct}, 1, 0.0)",
        a = u.m[0].0, at = u.m[0].1, b = u.m[1].0, bt = u.m[1].1, c = u.m[2].0, ct = u.m[2].1));
    retire(&u, "nasdaq-100");
    exec(&u.o, &format!("UPDATE tickers SET available = 0 WHERE id = {}", u.m[0].1));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), grouped).await.unwrap();
    close(&members(&u.o, grouped), &[("BBB", 0.5), ("CCC", 0.5)]);
}

/// D-INDEX-RETIRED-PAIR is a grouped row's only (lead, Codex 2b r1): a lone retired `Bots::DcaIndex` keeps the membership it
/// composed exactly as main does, its pairs unchecked: nothing dropped, nothing rewritten.
#[tokio::test(flavor = "current_thread")]
async fn a_lone_retired_index_keeps_its_members_whatever_its_pairs() {
    let u = universe();
    let id = seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", 3, 0.0, false);
    exec(&u.o, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_index', 'nasdaq-100') WHERE id = {id}"));
    for (i, w) in [0.5, 0.3, 0.2].into_iter().enumerate() { seed::insert_member(&u.o.primary, id, u.m[i].0, u.m[i].1, w, true, "2026-08-01 00:00:00", None); }
    retire(&u, "nasdaq-100");
    let updated = || u.o.primary.query_row("SELECT group_concat(updated_at) FROM bot_index_assets WHERE bot_id = ?1", [id], |r| r.get::<_, String>(0)).unwrap();
    let stamps = updated();
    exec(&u.o, &format!("UPDATE tickers SET trading_enabled = 0 WHERE id = {}", u.m[0].1));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    assert_eq!(updated(), stamps, "nothing rewritten");
    close(&members(&u.o, id), &[("AAA", 0.5), ("BBB", 0.3), ("CCC", 0.2)]);
    assert_eq!(composed_of(&u.o, id), Value::Null, "a legacy row never gets the grouped key");
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert!(!index::frozen_pair_lost(&u.o.primary, &bot).unwrap(), "no placement hold for a legacy row either");
}

// ---------------------------------------------------------------- 2b: money gates

fn cx<'a>(o: &store::Opened, bot: &model::Bot, prices: &'a PriceCache) -> TickContext<'a> {
    TickContext { credential_version: model::credential_version(&o.primary, bot).unwrap(), prices, process_start: DateTime::<Utc>::MIN_UTC,
                  stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} }
}

/// A funded stopped bot whose group's refresh fails places nothing on a rebalance or a redeploy; its rows stay.
#[tokio::test(flavor = "current_thread")]
async fn a_funded_stopped_bot_whose_group_fails_places_nothing_on_rebalance_or_redeploy() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    for (asset, sym, qty, value, price, n) in [(u.s.btc, "BTC", "0.01", "640", "64000", 1), (u.m[0].0, "AAA", "1", "100", "100", 2)] {
        seed::insert_row(&u.o.primary, &u.s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty, "amount_exec": qty, "base": sym,
            "quote_amount": value, "quote_amount_exec": value, "price": price, "created_at": "2026-09-01 10:00:01" }));
    }
    exec(&u.o, &format!("UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01) WHERE id = {id}"));
    let before = members(&u.o, id);
    let rows = || u.o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap();
    let n = rows();
    let prices = PriceCache::default();
    // Every newcomer unpriced: the refresh of the group refuses ("No matching coins").
    exec(&u.o, &format!("DELETE FROM rust_bot_group_members WHERE bot_id = {id}"));
    let t = script([0.0, 0.0, 0.0, 0.0, 0.0]);
    t.reply("GET /v2/orders/OSWAP-1", 200, json!({ "id": "OSWAP-1", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "sell", "notional": null,
        "qty": "0.005", "filled_qty": "0.005", "filled_avg_price": "64000", "limit_price": null }));
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(!matches!(out, Outcome::Placed(_)), "{out:?}");
    model::merge_transient_compact(&u.o.primary, id, &[(redeploy::REQUEST_KEY, json!({"id": "yes-1", "at": at(T0).timestamp()}))]).unwrap();
    let _ = redeploy::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await;
    assert_eq!((t.posted_orders().len(), members(&u.o, id), rows()), (0, before, n));
}

// ---------------------------------------------------------------- 2b: freshness

#[test]
fn two_groups_one_ranking_expired_holds_the_bot() {
    let u = universe();
    let tech = seed::insert_index(&u.o.primary, "tech", &["DDD.US"], &json!({ "DDD.US": 1e12 }));
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 0.5), group("tech", 1, 0.5)]));
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert!(staleness::ranking_stale(&u.o.primary, &bot, at(T0)).unwrap().is_none());
    exec(&u.o, &format!("UPDATE rust_index_rankings SET ranked_at = '2026-08-29T00:00:00.000000Z' WHERE index_id = {tech}"));
    let stale = staleness::ranking_stale(&u.o.primary, &bot, at(T0)).unwrap().expect("one group past 49 h holds the bot");
    assert!(stale.message.contains("the index tech"), "{}", stale.message);
}

#[test]
fn each_unranked_group_wakes_its_own_ranking() {
    let u = universe();
    for id in ["tech", "defi"] { exec(&u.o, &format!("INSERT INTO indices (external_id, source, created_at, updated_at) VALUES ('{id}', 'deltabadger', '2026-01-01', '2026-01-01')")); }
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 0.4), group("tech", 1, 0.3), group("defi", 1, 0.3)]));
    let mut sent = HashSet::new();
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert_eq!(index::ranking_wake(&u.o.primary, &bot, at(T0), &mut sent).unwrap(), vec!["tech".to_string(), "defi".to_string()]);
    assert!(index::ranking_wake(&u.o.primary, &bot, at(T0), &mut sent).unwrap().is_empty(), "once per slot");
}

/// BTC named beside a stock index on Alpaca: both catalogues judge the bot, the crypto one through the named asset.
#[test]
fn btc_beside_a_stock_index_on_alpaca_waits_for_both_catalogues() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    let bot = || model::load_bot(&u.o.primary, id).unwrap();
    assert!(staleness::stale(&u.o.primary, &bot(), at(T0)).unwrap().is_none());
    // The crypto listing sync last ran long ago.
    deltabadger::jobs::state::record_success(&u.o.primary, deltabadger::jobs::reference::ALPACA_CRYPTO, None, at("2026-08-01T00:00:00Z")).unwrap();
    let stale = staleness::stale(&u.o.primary, &bot(), at(T0)).unwrap();
    let plain = seed::insert_bot(&u.o.primary, &u.s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", json!({ u.m[0].0.to_string(): 1.0 })));
    let stock_only = staleness::stale(&u.o.primary, &model::load_bot(&u.o.primary, plain).unwrap(), at(T0)).unwrap();
    assert!(stale.is_some_and(|s| s.source == "Alpaca crypto tickers"), "the grouped bot waits for the crypto catalogue");
    assert!(stock_only.is_none(), "a stock-only basket does not");
}

// ---------------------------------------------------------------- 2b: the digest

#[tokio::test(flavor = "current_thread")]
async fn the_digest_names_each_groups_ranked_row_and_time() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    // A second selectable row of the same id with the same ranked_at: the lowest id is selected, and named.
    let second = u.o.primary.query_row("SELECT id FROM indices WHERE external_id = 'nasdaq-100'", [], |r| r.get::<_, i64>(0)).unwrap();
    exec(&u.o, "INSERT INTO indices (external_id, source, name, top_coins, weights, weight, created_at, updated_at) SELECT external_id, 'other', name, top_coins, weights, 0, created_at, updated_at FROM indices WHERE external_id = 'nasdaq-100'");
    seed::rank_indices(&u.o.primary, at("2099-01-01T00:00:00Z"));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let ranked = index::ranked_row(&u.o.primary, "nasdaq-100").unwrap().unwrap();
    assert_eq!(ranked.id, second);
    assert_eq!(composed_of(&u.o, id)["ranked"]["nasdaq-100"], json!([ranked.id, ranked.ranked_at]));
}

#[tokio::test(flavor = "current_thread")]
async fn a_late_refusal_keeps_a_newer_composition() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let first = composed_of(&u.o, id);
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let second = composed_of(&u.o, id);
    assert_eq!((first["digest"].clone(), first["shares"].clone()), (second["digest"].clone(), second["shares"].clone()), "identical payloads");
    assert_ne!(first["rev"], second["rev"], "two revisions");
    // A refusal that read the first revision removes nothing; one that read the current one removes it.
    composition::forget(&u.o.primary, id, first["rev"].as_str()).unwrap();
    assert_eq!(composed_of(&u.o, id), second);
    composition::forget(&u.o.primary, id, second["rev"].as_str()).unwrap();
    assert_eq!(composed_of(&u.o, id), Value::Null);
}

#[tokio::test(flavor = "current_thread")]
async fn a_legacy_index_tick_writes_no_composed_of() {
    let u = universe();
    let id = seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", 3, 0.0, false);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert_eq!(members(&u.o, id).len(), 3);
    assert_eq!(composed_of(&u.o, id), Value::Null);
    assert!(group_rows(&u.o, id).is_empty());
}

// ---------------------------------------------------------------- 2b: market cap between ingredients

#[tokio::test(flavor = "current_thread")]
async fn market_cap_weighs_an_index_by_its_members_caps_sum() {
    let u = universe();
    for (sym, cap) in [("AAA.US", 4e12), ("BBB.US", 3e12), ("CCC.US", 2e12), ("DDD.US", 1e12)] { exec(&u.o, &format!("UPDATE assets SET market_cap = {cap} WHERE external_id = '{sym}'")); }
    exec(&u.o, &format!("UPDATE assets SET market_cap = 10000000000000 WHERE id = {}", u.s.btc));
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.9 }), json!([group("nasdaq-100", 4, 0.1)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    // BTC 10 T against the four members' 10 T: half each; the index splits its half equally (flattening 1.0).
    close(&members(&u.o, id), &[("BTC", 0.5), ("AAA", 0.125), ("BBB", 0.125), ("CCC", 0.125), ("DDD", 0.125)]);
    assert_eq!(composed_of(&u.o, id)["shares"]["groups"]["nasdaq-100"], json!(0.5));
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_cap_falls_back_to_the_stored_ingredient_weights() {
    let u = universe();
    for sym in ["AAA.US", "BBB.US", "CCC.US"] { exec(&u.o, &format!("UPDATE assets SET market_cap = 1000000000000 WHERE external_id = '{sym}'")); }
    exec(&u.o, &format!("UPDATE assets SET market_cap = 10000000000000 WHERE id = {}", u.s.btc));
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.9 }), json!([group("nasdaq-100", 4, 0.1)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("BTC", 0.9), ("AAA", 0.025), ("BBB", 0.025), ("CCC", 0.025), ("DDD", 0.025)]);
}

// ---------------------------------------------------------------- 2b: rules, one-asset, eligibility, resync

#[test]
fn a_grouped_basket_gets_a_baskets_rule_support_and_an_index_row_keeps_its_own() {
    use deltabadger::engine::conditions::{Rule, Support};
    let u = universe();
    let grouped = model::load_bot(&u.o.primary, grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]))).unwrap();
    let legacy = model::load_bot(&u.o.primary, seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", 3, 0.0, false)).unwrap();
    assert_eq!(grouped.support(Rule::Price, true), Support::Implemented, "a basket sells");
    assert_eq!(legacy.support(Rule::Price, true), Support::Inapplicable, "a Bots::DcaIndex row never sells");
    // "Sell N base" stays one asset and no group.
    let one = json!({ "allocations": { "1": 1.0 } });
    assert!(deltabadger::engine::schedule::one_asset(one.as_object().unwrap()));
    let mixed = json!({ "allocations": { "1": 0.5 }, "index_groups": [group("nasdaq-100", 2, 0.5)] });
    assert!(!deltabadger::engine::schedule::one_asset(mixed.as_object().unwrap()));
}

fn reasons(u: &U, id: i64) -> Vec<String> { eligibility::bot_reasons(&u.o.primary, &model::load_bot(&u.o.primary, id).unwrap()).unwrap() }

#[test]
fn a_grouped_row_is_eligible_and_malformed_groups_are_refused() {
    let u = universe();
    let lone = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    assert_eq!(reasons(&u, lone), Vec::<String>::new(), "a lone group with allocations {{}}");
    let mixed = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    assert_eq!(reasons(&u, mixed), Vec::<String>::new());
    let mut g = group("nasdaq-100", 2, 1.0);
    for (case, groups) in [("empty", json!([])), ("null", json!([null])), ("duplicate", json!([group("nasdaq-100", 2, 0.5), group("nasdaq-100", 2, 0.5)])),
                           ("absent weight", { g.as_object_mut().unwrap().remove("weight"); json!([g.clone()]) }),
                           ("garbage weight", { g["weight"] = json!("x"); json!([g.clone()]) }),
                           ("over one", { g["weight"] = json!(1.5); json!([g.clone()]) })] {
        let id = grouped_bot(&u, json!({}), groups);
        assert!(!reasons(&u, id).is_empty(), "{case}: refused");
    }
    // Codex 2b r1: an explicit `allocations: null` beside a valid group is refused; absent and {} are not.
    let null = grouped_bot(&u, Value::Null, json!([group("nasdaq-100", 2, 1.0)]));
    assert!(reasons(&u, null).iter().any(|r| r.contains("allocations: an asset id or a weight")), "{:?}", reasons(&u, null));
    let absent = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_remove(settings, '$.allocations') WHERE id = {absent}"));
    assert_eq!(reasons(&u, absent), Vec::<String>::new(), "absent allocations");
    // A named asset with no ticker on this venue is refused (a basket's rule), not dropped.
    exec(&u.o, "INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('nowhere', 'NOW', 'Nowhere', 'Cryptocurrency', '2026-01-01', '2026-01-01')");
    let nowhere: i64 = u.o.primary.last_insert_rowid();
    let id = grouped_bot(&u, json!({ nowhere.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    assert!(reasons(&u, id).iter().any(|r| r.contains(&format!("no ticker for the asset {nowhere}"))), "{:?}", reasons(&u, id));
}

#[test]
fn a_lone_manual_group_at_point_four_is_refused() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 0.4)]));
    assert!(reasons(&u, id).iter().any(|r| r.starts_with(eligibility::UNBALANCED)), "{:?}", reasons(&u, id));
}

#[test]
fn a_valid_group_beside_an_unreadable_named_weight_is_refused() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): "half" }), json!([group("nasdaq-100", 2, 0.5)]));
    assert!(reasons(&u, id).iter().any(|r| r.contains("allocations: an asset id or a weight")), "{:?}", reasons(&u, id));
}

#[test]
fn a_ranking_write_marks_a_grouped_follower_for_resync() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    let marked = index::mark_resync_all(&u.o.primary, at(T0)).unwrap();
    assert!(marked >= 1);
    assert!(index::resync_marked(&u.o.primary).unwrap().iter().any(|(m, _)| *m == id), "a grouped follower is marked and selected");
}

/// PLAN index-as-ingredient §3.4: a named sale of a held asset whose pair is valid sells, whatever any group's refresh says
/// (here every newcomer unpriced: the group refuses).
#[tokio::test(flavor = "current_thread")]
async fn a_sell_action_sells_while_an_unrelated_group_fails_its_probe() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OHELD-1", "amount": "0.01", "amount_exec": "0.01",
        "quote_amount": "640", "quote_amount_exec": "640", "price": "64000", "created_at": "2026-09-01 10:00:01" }));
    exec(&u.o, &format!("UPDATE bots SET status = 2 WHERE id = {id}; DELETE FROM rust_bot_group_members WHERE bot_id = {id}"));
    let t = script_with([0.0, 0.0, 0.0, 0.0, 0.0], json!({ "GET /v2/positions": ok(json!([{ "symbol": "BTCUSD", "asset_class": "crypto", "qty": "0.01", "qty_available": "0.01", "side": "long" }])) }));
    let prices = PriceCache::default();
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    deltabadger::engine::liquidation::run(&u.o.primary, &venue(t.clone()), id, &json!([["BTC", u.s.btc]]), &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    let sells: Vec<Value> = t.posted_orders().into_iter().filter(|b| b["side"] == "sell").collect();
    let logs: Vec<(String, String)> = u.o.primary.prepare("SELECT event, coalesce(details, '') FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(sells.len(), 1, "{:?} {logs:?} {}", t.posted_orders(), model::load_bot(&u.o.primary, id).unwrap().transient);
    assert_eq!(sells[0]["symbol"], "BTC/USD");
}

/// Codex r1 #1: a filled rebalance sell settles and books its proceeds while the group's refresh refuses; the owed buy
/// waits unattempted (`buy_unavailable`) and is placed once, by the first pass whose refresh succeeds.
#[tokio::test(flavor = "current_thread")]
async fn a_filled_sell_with_a_failed_probe_settles_and_its_proceeds_are_spent_once() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OHELD-1", "amount": "0.02", "amount_exec": "0.02",
        "quote_amount": "1280", "quote_amount_exec": "1280", "price": "64000", "created_at": "2026-09-01 10:00:01" }));
    let sell = seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OSWAP-1", "side": 1, "amount": "0.005", "amount_exec": "0.005",
        "quote_amount": "320", "quote_amount_exec": "320", "price": "64000", "created_at": "2026-09-01 11:00:01" }));
    let pending = rebalance::Pending { phase: rebalance::PHASE_SELLING.into(), sell_transaction_id: Some(sell), buy_transaction_id: None, remaining_quote_amount: None, buy_attempted: false };
    exec(&u.o, &format!("UPDATE transactions SET transaction_type = 'REBALANCE' WHERE id = {sell};
        UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01),
        transient_data = json_set(transient_data, '$.rebalance_pending', json('{}')) WHERE id = {id};", pending.to_json()));
    let phase = || rebalance::pending(&model::load_bot(&u.o.primary, id).unwrap().transient).unwrap();
    let prices = PriceCache::default();
    // The refresh refuses (its rows gone, every newcomer unpriced): the sell settles, nothing is bought.
    exec(&u.o, &format!("DELETE FROM rust_bot_group_members WHERE bot_id = {id}"));
    let t = script([0.0, 0.0, 0.0, 0.0, 0.0]);
    t.reply("GET /v2/orders/OSWAP-1", 200, json!({ "id": "OSWAP-1", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "sell", "notional": null,
        "qty": "0.005", "filled_qty": "0.005", "filled_avg_price": "64000", "limit_price": null }));
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Skipped("buy_unavailable")), "{out:?}");
    let p = phase().unwrap();
    assert_eq!((p.phase.as_str(), p.buy_attempted, p.remaining_quote_amount.map(|a| a.to_s_f())), (rebalance::PHASE_BUYING, false, Some("320.0".into())), "proceeds booked once, owed");
    assert!(t.posted_orders().is_empty());
    // A second refused pass changes nothing.
    rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(t.posted_orders().is_empty() && phase().unwrap().remaining_quote_amount.map(|a| a.to_s_f()) == Some("320.0".into()));
    // The refresh succeeds: one buy.
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Placed(_)), "{out:?}");
    assert_eq!(t.posted_orders().iter().filter(|b| b["side"] == "buy").count(), 1);
    assert!(phase().unwrap().buy_attempted, "attempted once, with its intent");
}

/// PLAN index-as-ingredient §4 (Codex r8 6): under top-level market cap a stale crypto asset catalogue refuses the closing
/// write itself, so the background resync stores no shares from stale caps either.
#[tokio::test(flavor = "current_thread")]
async fn stale_caps_resync_writes_nothing() {
    let u = universe();
    exec(&u.o, &format!("UPDATE assets SET market_cap = 10000000000000 WHERE id = {}", u.s.btc));
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    deltabadger::jobs::state::record_success(&u.o.primary, deltabadger::jobs::reference::ASSETS, None, at("2026-08-01T00:00:00Z")).unwrap();
    let refused = refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await;
    assert!(matches!(&refused, Err(index::Refusal::Failure(m)) if m.contains("Crypto assets")), "{refused:?}");
    assert!(members(&u.o, id).is_empty() && composed_of(&u.o, id).is_null());
    // The same bot with manual ingredient weights needs no caps.
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'manual') WHERE id = {id}"));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    close(&members(&u.o, id), &[("BTC", 0.5), ("AAA", 0.25), ("BBB", 0.25)]);
}

/// Incumbency is a ticker's (§3.3): a member whose stored pair is not its current one is probed again.
#[tokio::test(flavor = "current_thread")]
async fn a_member_whose_stored_pair_moved_is_reprobed() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    refresh(&u, t.clone(), id).await.unwrap();
    let probes = |t: &ScriptedTransport| t.requests().iter().filter(|r| r.path.starts_with("/v2/stocks/") && r.path.ends_with("/quotes/latest")).count();
    assert_eq!(probes(&t), 2);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    refresh(&u, t.clone(), id).await.unwrap();
    assert_eq!(probes(&t), 0, "incumbents keep their seat without a probe");
    // A relisting gives AAA a new ticker id: its stored group row no longer names its in_index ticker.
    // AAA's stored row names another pair (as after a venue or quote move): it is no incumbent of that pair any more.
    exec(&u.o, &format!("UPDATE rust_bot_group_members SET ticker_id = {} WHERE bot_id = {id} AND asset_id = {}", u.s.ticker_id, u.m[0].0));
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    refresh(&u, t.clone(), id).await.unwrap();
    assert_eq!(probes(&t), 1, "the moved pair is probed again");
}

// ---------------------------------------------------------------- 2b: the missing money-path tests

fn tx_count(o: &store::Opened, id: i64) -> i64 { o.primary.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap() }
fn events(o: &store::Opened, id: i64, event: &str) -> i64 {
    o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = ?2", rusqlite::params![id, event], |r| r.get(0)).unwrap()
}
/// A filled buy of `qty` units of `asset` worth `value` at `price`.
fn hold(u: &U, id: i64, asset: i64, qty: &str, value: &str, price: &str, n: i64) {
    seed::insert_row(&u.o.primary, &u.s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty, "amount_exec": qty,
        "quote_amount": value, "quote_amount_exec": value, "price": price, "created_at": "2026-09-01 10:00:01" }));
}
/// The account catalogue lists the stocks (exchange_assets), so their Alpaca positions map to them.
fn listed(u: &U) {
    for (asset, _) in &u.m { exec(&u.o, &format!("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES ({asset}, {}, 1, '2026-01-01', '2026-01-01')", u.s.exchange_id)); }
}
/// A composition write landing while `path` is first requested: AAA's group-row weight moves (the snapshot changes).
fn moves_aaa(u: &U, t: &ScriptedTransport, path: &str, id: i64) -> Hooked {
    let (side, aaa) = (side_conn(u), u.m[0].0);
    hooked(t, path, move || { side.execute("UPDATE rust_bot_group_members SET weight = 0.4 WHERE bot_id = ?1 AND asset_id = ?2", [id, aaa]).unwrap(); })
}
async fn tick_at<T: Transport>(u: &U, t: T, id: i64) -> TickOutcome {
    tick::tick(&u.o.primary, &venue(t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap()
}

/// D-INDEX-RETIRED-PAIR at placement, over every frozen member and not only the leg's: BTC named beside a frozen group
/// [AAA, BBB]; an owed rebalance buy targets AAA. The pass's refresh keeps both members; BBB's pair is disabled after it
/// (while the settled cash is read). AAA's buy is held: no order, no row, the owed buy stays unattempted. (A tick's legs
/// carry every member's ticker into the fence; a rebalance leg carries only its own, so only this check sees BBB.)
#[tokio::test(flavor = "current_thread")]
async fn a_frozen_member_disabled_after_the_refresh_holds_the_placement() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    retire(&u, "nasdaq-100");
    hold(&u, id, u.s.btc, "0.02", "1280", "64000", 1);
    let sell = seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OSWAP-1", "side": 1, "amount": "0.005", "amount_exec": "0.005",
        "quote_amount": "320", "quote_amount_exec": "320", "price": "64000", "created_at": "2026-09-01 11:00:01" }));
    let owed = rebalance::Pending { phase: rebalance::PHASE_BUYING.into(), sell_transaction_id: Some(sell), buy_transaction_id: None,
                                    remaining_quote_amount: Some(deltabadger::v2::BigDec::parse("320").unwrap()), buy_attempted: false };
    exec(&u.o, &format!("UPDATE transactions SET transaction_type = 'REBALANCE' WHERE id = {sell};
        UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01),
        transient_data = json_set(transient_data, '$.rebalance_pending', json('{}')) WHERE id = {id};", owed.to_json()));
    let rows = tx_count(&u.o, id);
    let prices = PriceCache::default();
    let (side, bbb) = (side_conn(&u), u.m[1].1);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(hooked(&t, "/v2/account", move || { side.execute("UPDATE tickers SET trading_enabled = 0 WHERE id = ?1", [bbb]).unwrap(); })),
                             id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Skipped("changed")), "{out:?}");
    assert!(t.posted_orders().is_empty(), "{:?}", t.posted_orders());
    assert_eq!(tx_count(&u.o, id), rows, "no order row");
    assert_eq!(rebalance::pending(&model::load_bot(&u.o.primary, id).unwrap().transient).unwrap(), Some(owed), "the owed buy stays, unattempted");
    close(&members(&u.o, id), &[("BTC", 0.5), ("AAA", 0.25), ("BBB", 0.25)]);
    // The next pass's refresh drops BBB and buys AAA, once.
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Placed(_)), "{out:?}");
    close(&members(&u.o, id), &[("BTC", 0.5), ("AAA", 0.5)]);
    let buys: Vec<Value> = t.posted_orders();
    assert_eq!((buys.len(), buys[0]["symbol"].as_str()), (1, Some("AAA")), "{buys:?}");
}

/// PLAN §3.4 sizing claim, every no-intent write of a grouped sizing: the composition moves after the refresh (while the
/// legs are priced, or the owed buy's balance is read). Each write refuses: (a) the below-minimum skip lines and rows,
/// (b) the cap-spent stop, (c) an owed rebalance buy's dust clear.
#[tokio::test(flavor = "current_thread")]
async fn a_grouped_sizing_that_ends_without_an_intent_checks_the_claim() {
    // (a) 1 USD a week over AAA and BBB: two 0.50 legs under the 1 USD minimum.
    let u = universe();
    let id = seed::insert_bot(&u.o.primary, &u.s, &BotSpec::weekly(1.0, "2026-09-01 14:00:00").with("allocations", json!({})).with("index_groups", json!([group("nasdaq-100", 2, 1.0)])));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick_at(&u, moves_aaa(&u, &t, "/v2/stocks/BBB/quotes/latest", id), id).await;
    assert!(t.posted_orders().is_empty());
    assert_eq!((tx_count(&u.o, id), events(&u.o, id, "order_skipped"), events(&u.o, id, "orders_below_minimum")), (0, 0, 0), "(a) no skip row or line");

    // (b) 60 USD a week, a 1 USD cap: the whole remainder sizes to two 0.50 legs, none placeable.
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', 1), \
                         transient_data = json_set(transient_data, '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = {id}"));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick_at(&u, moves_aaa(&u, &t, "/v2/stocks/BBB/quotes/latest", id), id).await;
    let after = model::load_bot(&u.o.primary, id).unwrap();
    let key: Option<String> = u.o.primary.query_row("SELECT stop_message_key FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(t.posted_orders().is_empty());
    assert_ne!(after.status, deltabadger::enums::BotStatus::Stopped, "(b) not stopped");
    assert_eq!((key, events(&u.o, id, "stopped"), tx_count(&u.o, id)), (None, 0, 0));
    assert!(after.transient.get(deltabadger::engine::notice::LIMIT).is_none(), "no amount-limit mail owed");

    // (c) An owed rebalance buy of 0.50 USD (under every member's minimum) after a filled sell.
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    hold(&u, id, u.s.btc, "0.02", "1280", "64000", 1);
    let sell = seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OSWAP-1", "side": 1, "amount": "0.0000078125", "amount_exec": "0.0000078125",
        "quote_amount": "0.5", "quote_amount_exec": "0.5", "price": "64000", "created_at": "2026-09-01 11:00:01" }));
    let owed = rebalance::Pending { phase: rebalance::PHASE_BUYING.into(), sell_transaction_id: Some(sell), buy_transaction_id: None,
                                    remaining_quote_amount: Some(deltabadger::v2::BigDec::parse("0.5").unwrap()), buy_attempted: false };
    exec(&u.o, &format!("UPDATE transactions SET transaction_type = 'REBALANCE' WHERE id = {sell};
        UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01),
        transient_data = json_set(transient_data, '$.rebalance_pending', json('{}')) WHERE id = {id};", owed.to_json()));
    let prices = PriceCache::default();
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(moves_aaa(&u, &t, "/v2/account", id)), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Skipped("changed")), "{out:?}");
    assert!(t.posted_orders().is_empty());
    assert_eq!(rebalance::pending(&model::load_bot(&u.o.primary, id).unwrap().transient).unwrap(), Some(owed), "(c) the owed buy stands");
    assert_eq!(events(&u.o, id, "rebalance_dust"), 0);
}

/// Codex 2a r1: a composed retired group beside an uncomposed live one whose refresh fails. Retirement and provenance are
/// one id's: the pass is not skipped as a retired index's, its filled sell settles and books the proceeds; the next pass
/// whose refresh succeeds buys.
#[tokio::test(flavor = "current_thread")]
async fn a_composed_retired_group_beside_an_uncomposed_live_one_still_rebalances() {
    let u = universe();
    seed::insert_index(&u.o.primary, "tech", &["DDD.US", "EEE.US"], &json!({ "DDD.US": 1e12, "EEE.US": 5e11 }));
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.25), group("tech", 2, 0.25)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    retire(&u, "tech");
    // nasdaq-100 has no rows of this engine's (uncomposed); tech's stand (composed, retired).
    exec(&u.o, &format!("DELETE FROM rust_bot_group_members WHERE bot_id = {id} AND index_external_id = 'nasdaq-100'"));
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert!(index::any_retired(&u.o.primary, &bot).unwrap() && index::any_uncomposed(&u.o.primary, &bot).unwrap() && !index::retired_uncomposed(&u.o.primary, &bot).unwrap());
    hold(&u, id, u.s.btc, "0.02", "1280", "64000", 1);
    let sell = seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OSWAP-1", "side": 1, "amount": "0.005", "amount_exec": "0.005",
        "quote_amount": "320", "quote_amount_exec": "320", "price": "64000", "created_at": "2026-09-01 11:00:01" }));
    let pending = rebalance::Pending { phase: rebalance::PHASE_SELLING.into(), sell_transaction_id: Some(sell), buy_transaction_id: None, remaining_quote_amount: None, buy_attempted: false };
    exec(&u.o, &format!("UPDATE transactions SET transaction_type = 'REBALANCE' WHERE id = {sell};
        UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01),
        transient_data = json_set(transient_data, '$.rebalance_pending', json('{}')) WHERE id = {id};", pending.to_json()));
    let phase = || rebalance::pending(&model::load_bot(&u.o.primary, id).unwrap().transient).unwrap().unwrap();
    let prices = PriceCache::default();
    // nasdaq-100's newcomers all unpriced: its refresh refuses.
    let t = script([0.0, 0.0, 0.0, 0.0, 0.0]);
    t.reply("GET /v2/orders/OSWAP-1", 200, json!({ "id": "OSWAP-1", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "sell", "notional": null,
        "qty": "0.005", "filled_qty": "0.005", "filled_avg_price": "64000", "limit_price": null }));
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Skipped("buy_unavailable")), "{out:?}");
    let p = phase();
    assert_eq!((p.phase.as_str(), p.buy_attempted, p.remaining_quote_amount.map(|a| a.to_s_f())), (rebalance::PHASE_BUYING, false, Some("320.0".into())), "the sell settled");
    assert!(t.posted_orders().is_empty());
    // The refresh succeeds: nasdaq-100 is composed again beside the frozen tech, and the owed buy is placed once.
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Placed(_)), "{out:?}");
    let buys: Vec<Value> = t.posted_orders().into_iter().filter(|b| b["side"] == "buy").collect();
    assert_eq!(buys.len(), 1, "{buys:?}");
    assert!(phase().buy_attempted);
    close(&members(&u.o, id), &[("BTC", 0.5), ("AAA", 0.125), ("BBB", 0.125), ("DDD", 0.125), ("EEE", 0.125)]);
}

/// PLAN §3.4 + §5: BTC is named, AAA is held through the group. With BTC unpriced, the liquidation of both sells AAA (the
/// lone-named-holding staleness rule is a one-asset basket's, not a grouped one's) and skips BTC, which has no value to sell.
#[tokio::test(flavor = "current_thread")]
async fn liquidation_sells_a_priced_group_holding_while_the_named_holding_is_unpriced() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    hold(&u, id, u.s.btc, "0.01", "640", "64000", 1);
    hold(&u, id, u.m[0].0, "2", "200", "100", 2);
    listed(&u);
    exec(&u.o, &format!("UPDATE bots SET status = 2 WHERE id = {id}"));
    let t = script_with([100.0, 200.0, 300.0, 400.0, 500.0], json!({
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": {} })),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": {} })),
        "GET /v2/positions": ok(json!([{ "symbol": "BTCUSD", "asset_class": "crypto", "qty": "0.01", "qty_available": "0.01", "side": "long" },
                                       { "symbol": "AAA", "asset_class": "us_equity", "qty": "2", "qty_available": "2", "side": "long" }])) }));
    let prices = PriceCache::default();
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    deltabadger::engine::liquidation::run(&u.o.primary, &venue(t.clone()), id, &json!([["BTC", u.s.btc], ["AAA", u.m[0].0]]), &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    let logs: Vec<(String, String)> = u.o.primary.prepare("SELECT event, coalesce(details, '') FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    let sells: Vec<Value> = t.posted_orders().into_iter().filter(|b| b["side"] == "sell").collect();
    assert_eq!(sells.len(), 1, "{:?} {logs:?}", t.posted_orders());
    assert_eq!((sells[0]["symbol"].as_str(), sells[0]["qty"].as_str()), (Some("AAA"), Some("2.000000000")), "{sells:?}");
    let rows: Vec<(String, i64, String)> = u.o.primary.prepare("SELECT base, side, transaction_type FROM transactions WHERE bot_id = ?1 AND external_id NOT LIKE 'OHELD-%' ORDER BY id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, vec![("AAA".to_string(), 1, "LIQUIDATION".to_string())]);
    assert!(logs.iter().any(|(e, d)| e == "liquidation_skipped" && d.contains("\"BTC\"")), "BTC (no value) skipped: {logs:?}");
    assert!(!logs.iter().any(|(e, _)| e == "liquidation_not_started"), "{logs:?}");
}

/// PLAN §4: under top-level market cap, a fresh ranking with a stale crypto asset catalogue places nothing and writes no
/// composition: the tick-start check holds it, and the closing write refuses on its own.
#[tokio::test(flavor = "current_thread")]
async fn fresh_ranking_stale_asset_caps_place_no_order() {
    let u = universe();
    exec(&u.o, &format!("UPDATE assets SET market_cap = 10000000000000 WHERE id = {}", u.s.btc));
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert!(staleness::ranking_stale(&u.o.primary, &bot, at(T0)).unwrap().is_none(), "the ranking is fresh");
    deltabadger::jobs::state::record_success(&u.o.primary, deltabadger::jobs::reference::ASSETS, None, at("2026-08-01T00:00:00Z")).unwrap();
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick_at(&u, t.clone(), id).await;
    assert!(matches!(&out, TickOutcome::Stale { message, .. } if message.contains("Crypto assets")), "{out:?}");
    assert!(t.posted_orders().is_empty(), "{:?}", t.posted_orders());
    assert_eq!(tx_count(&u.o, id), 0);
    // The closing write refuses on its own too (a background resync, no tick-start check).
    let refused = refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await;
    assert!(matches!(&refused, Err(index::Refusal::Failure(m)) if m.contains("Crypto assets")), "{refused:?}");
    assert!(members(&u.o, id).is_empty() && group_rows(&u.o, id).is_empty() && composed_of(&u.o, id).is_null(), "no composition from stale caps");
}

/// PLAN §6.2: a grouped bot is never one asset. Reversed (selling for 900 USD), it sells across its current members (BTC,
/// then the group's AAA), not only its one named asset.
#[tokio::test(flavor = "current_thread")]
async fn a_grouped_bot_reversed_sells_its_current_members() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    hold(&u, id, u.s.btc, "0.01", "640", "64000", 1);
    hold(&u, id, u.m[0].0, "2", "200", "100", 2);
    hold(&u, id, u.m[1].0, "1", "200", "200", 3);
    listed(&u);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling', '$.sell_denomination', 'quote', '$.sell_quote_amount', 900) WHERE id = {id}"));
    let t = script_with([100.0, 200.0, 300.0, 400.0, 500.0], json!({
        "GET /v2/positions": ok(json!([{ "symbol": "BTCUSD", "asset_class": "crypto", "qty": "0.01", "qty_available": "0.01", "side": "long" },
                                       { "symbol": "AAA", "asset_class": "us_equity", "qty": "2", "qty_available": "2", "side": "long" },
                                       { "symbol": "BBB", "asset_class": "us_equity", "qty": "1", "qty_available": "1", "side": "long" }])) }));
    let out = tick_at(&u, t.clone(), id).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    let sells: Vec<(String, String)> = t.posted_orders().into_iter().map(|b| { assert_eq!(b["side"], "sell", "{b}"); (b["symbol"].as_str().unwrap().to_string(), b["qty"].as_str().unwrap_or("").to_string()) }).collect();
    // BTC furthest over its share goes first, whole; then AAA whole (2 x 100), then BBB for the last 60 (0.3 x 200).
    assert_eq!(sells, [("BTC/USD", "0.010000000"), ("AAA", "2.000000000"), ("BBB", "0.300000000")].map(|(s, q)| (s.to_string(), q.to_string())));
    let sold: Vec<i64> = u.o.primary.prepare("SELECT side FROM transactions WHERE bot_id = ?1 AND external_id NOT LIKE 'OHELD-%'").unwrap()
        .query_map([id], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(sold.len(), sells.len());
    assert!(sold.iter().all(|s| *s == 1), "every row a sell");
}

/// PLAN §6.2 on the web reverse path (write::reverse): a grouped bot naming one asset toggles as a wider basket does,
/// buying → selling → buying, never "selling N base".
#[test]
fn two_reversals_of_a_grouped_bot_return_it_to_buying() {
    use deltabadger::web::bot::write::{self, Reversal};
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    let reverse = |now: &str| assert!(matches!(write::reverse(&u.o.primary, &seed::cipher(), u.s.user_id, id, at(now)).unwrap(), Reversal::Reversed));
    let side = || { let b = model::load_bot(&u.o.primary, id).unwrap(); (b.settings["direction"].as_str().map(str::to_string), b.settings["sell_denomination"].as_str().map(str::to_string)) };
    reverse("2026-09-01T14:00:01Z");
    let first = side();
    assert_eq!(first.0.as_deref(), Some("selling"));
    assert_ne!(first.1.as_deref(), Some("base"), "never sells N base");
    reverse("2026-09-01T14:00:02Z");
    assert_eq!(side().0.as_deref(), Some("buying"), "two reversals: buying again");
    assert_eq!(model::load_bot(&u.o.primary, id).unwrap().settings["index_groups"], json!([group("nasdaq-100", 2, 0.5)]), "the groups stand");
}

/// PLAN §5 Smart Interval flip clamp: a start-selling trigger flips a grouped bot, and its split is clamped to the largest
/// minimum of the grouped universe (every tradable pair at the quote): EEE's 50 USD, a pair no member trades.
#[tokio::test(flavor = "current_thread")]
async fn a_start_selling_trigger_flips_a_grouped_bot_with_the_universe_quote_minimum() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    exec(&u.o, &format!("UPDATE tickers SET minimum_quote_size = '50' WHERE id = {}", u.m[4].1));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.smart_intervaled', json('true'), '$.smart_interval_quote_amount', 2.0,
        '$.sell_denomination', 'quote', '$.sell_quote_amount', 60, '$.price_limited', json('true'), '$.price_limit', 70000, '$.price_limit_value_condition', 'below',
        '$.price_limit_action', 'start_selling', '$.price_limit_in_ticker_id', {}) WHERE id = {id}", u.s.ticker_id));
    assert!(reasons(&u, id).is_empty(), "{:?}", reasons(&u, id));
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick_at(&u, t.clone(), id).await;
    let b = model::load_bot(&u.o.primary, id).unwrap();
    assert!(b.selling(), "flipped: {out:?} {}", b.transient);
    assert_eq!(b.settings["smart_interval_quote_amount"].as_f64(), Some(50.0), "the universe's largest minimum");
    assert!(t.posted_orders().iter().all(|o| o["side"] != "buy"), "no buy after the flip: {:?}", t.posted_orders());
}

// ---------------------------------------------------------------- 2b: Codex branch review r1

/// Lead r1 #1: a grouped retired group that lost every member's pair refuses new orders, but a pending rebalance's filled
/// sell still settles and books its proceeds (the owed buy waits, unattempted).
#[tokio::test(flavor = "current_thread")]
async fn a_retired_group_losing_every_pair_still_settles_a_pending_rebalance() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    retire(&u, "nasdaq-100");
    exec(&u.o, &format!("UPDATE tickers SET available = 0 WHERE id IN ({}, {})", u.m[0].1, u.m[1].1));
    hold(&u, id, u.s.btc, "0.02", "1280", "64000", 1);
    let sell = seed::insert_row(&u.o.primary, &u.s, id, u.s.btc, &json!({ "external_status": 2, "external_id": "OSWAP-1", "side": 1, "amount": "0.005", "amount_exec": "0.005",
        "quote_amount": "320", "quote_amount_exec": "320", "price": "64000", "created_at": "2026-09-01 11:00:01" }));
    let pending = rebalance::Pending { phase: rebalance::PHASE_SELLING.into(), sell_transaction_id: Some(sell), buy_transaction_id: None, remaining_quote_amount: None, buy_attempted: false };
    exec(&u.o, &format!("UPDATE transactions SET transaction_type = 'REBALANCE' WHERE id = {sell};
        UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01),
        transient_data = json_set(transient_data, '$.rebalance_pending', json('{}')) WHERE id = {id};", pending.to_json()));
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    t.reply("GET /v2/orders/OSWAP-1", 200, json!({ "id": "OSWAP-1", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "sell", "notional": null,
        "qty": "0.005", "filled_qty": "0.005", "filled_avg_price": "64000", "limit_price": null }));
    let prices = PriceCache::default();
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(t.clone()), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Skipped("buy_unavailable")), "{out:?}");
    let p = rebalance::pending(&model::load_bot(&u.o.primary, id).unwrap().transient).unwrap().unwrap();
    assert_eq!((p.phase.as_str(), p.buy_attempted, p.remaining_quote_amount.map(|a| a.to_s_f())), (rebalance::PHASE_BUYING, false, Some("320.0".into())));
    assert!(t.posted_orders().is_empty());
}

/// Lead r1 #1: the grouped closing write re-checks ownership under its lock: a bot archived while a newcomer is probed
/// writes nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_grouped_refresh_whose_bot_is_archived_mid_probe_writes_nothing() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    let side = side_conn(&u);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let r = index::refresh_composition(&u.o.primary, &venue(hooked(&t, "/v2/stocks/BBB/quotes/latest", move || { side.execute("UPDATE bots SET status = 7 WHERE id = ?1", [id]).unwrap(); })),
                                       &model::load_bot(&u.o.primary, id).unwrap(), &FixedClock(at(T0)), &PriceCache::default()).await;
    assert!(!matches!(r, Ok(Ok(()))), "{r:?}");
    assert!(members(&u.o, id).is_empty() && group_rows(&u.o, id).is_empty() && composed_of(&u.o, id).is_null());
}

/// Lead r1 #2 (Codex P1): placement fences the composition's revision, not only its rows. A second refresh that wrote the
/// same rows after this tick's (here: its new `rev`, landing while the legs are priced) holds every leg.
#[tokio::test(flavor = "current_thread")]
async fn a_new_composition_revision_after_sizing_holds_the_placement() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    let side = side_conn(&u);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let rows = group_rows(&u.o, id);
    tick_at(&u, hooked(&t, "/v2/stocks/BBB/quotes/latest", move || {
        side.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_of.rev', 'another-refresh') WHERE id = ?1", [id]).unwrap();
    }), id).await;
    assert!(t.posted_orders().is_empty(), "{:?}", t.posted_orders());
    assert_eq!((tx_count(&u.o, id), group_rows(&u.o, id)), (0, rows));
}

async fn liquidate<T: Transport>(u: &U, t: T, id: i64, holdings: Value) -> Vec<(String, String)> {
    let prices = PriceCache::default();
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    deltabadger::engine::liquidation::run(&u.o.primary, &venue(t), id, &holdings, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    u.o.primary.prepare("SELECT event, coalesce(details, '') FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
/// BTC named beside the group, BTC 0.01 and AAA 2 held, both positions on the account.
fn held_basket(u: &U) -> i64 {
    let id = grouped_bot(u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    hold(u, id, u.s.btc, "0.01", "640", "64000", 1);
    hold(u, id, u.m[0].0, "2", "200", "100", 2);
    listed(u);
    id
}
fn held_positions() -> Value {
    json!({ "GET /v2/positions": ok(json!([{ "symbol": "BTCUSD", "asset_class": "crypto", "qty": "0.01", "qty_available": "0.01", "side": "long" },
                                           { "symbol": "AAA", "asset_class": "us_equity", "qty": "2", "qty_available": "2", "side": "long" }])) })
}
fn sells(t: &ScriptedTransport) -> Vec<String> { t.posted_orders().into_iter().filter(|b| b["side"] == "sell").map(|b| b["symbol"].as_str().unwrap().to_string()).collect() }

/// Lead r1 #3 (Codex P1): the leg's catalogue freshness is judged in the intent's own transaction. The stock catalogue
/// expires while AAA's price is read: AAA is not sold (its leg skipped, `stale`).
#[tokio::test(flavor = "current_thread")]
async fn a_legs_catalogue_expiring_during_sizing_holds_that_sale() {
    let u = universe();
    let id = held_basket(&u);
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    exec(&u.o, &format!("UPDATE bots SET status = 2 WHERE id = {id}"));
    let side = side_conn(&u);
    let t = script_with([100.0, 200.0, 300.0, 400.0, 500.0], held_positions());
    let logs = liquidate(&u, hooked(&t, "/v2/stocks/AAA/quotes/latest", move || {
        deltabadger::jobs::state::record_success(&side, deltabadger::jobs::reference::STOCKS, None, at("2026-08-01T00:00:00Z")).unwrap();
    }), id, json!([["AAA", u.m[0].0]])).await;
    assert!(sells(&t).is_empty(), "{:?} {logs:?}", t.posted_orders());
    assert_eq!(tx_count(&u.o, id), 2, "no order row");
    assert!(logs.iter().any(|(e, d)| e == "liquidation_skipped" && d.contains("\"AAA\"") && d.contains("stale")), "{logs:?}");
}

/// Lead r1 #4 (Codex P2): a stale stock catalogue skips the stock's leg only; BTC (its catalogue fresh) sells in either order.
#[tokio::test(flavor = "current_thread")]
async fn a_stale_stock_catalogue_skips_its_leg_and_btc_still_sells_in_either_order() {
    for stock_first in [true, false] {
        let u = universe();
        let id = held_basket(&u);
        refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
        exec(&u.o, &format!("UPDATE bots SET status = 2 WHERE id = {id}"));
        deltabadger::jobs::state::record_success(&u.o.primary, deltabadger::jobs::reference::STOCKS, None, at("2026-08-01T00:00:00Z")).unwrap();
        let (btc, aaa) = (json!(["BTC", u.s.btc]), json!(["AAA", u.m[0].0]));
        let t = script_with([100.0, 200.0, 300.0, 400.0, 500.0], held_positions());
        let logs = liquidate(&u, t.clone(), id, if stock_first { json!([aaa, btc]) } else { json!([btc, aaa]) }).await;
        assert_eq!(sells(&t), vec!["BTC/USD".to_string()], "stock first {stock_first}: {logs:?}");
        assert!(logs.iter().any(|(e, d)| e == "liquidation_skipped" && d.contains("\"AAA\"") && d.contains("stale")), "{logs:?}");
        assert!(!logs.iter().any(|(e, _)| e == "liquidation_not_started"), "{logs:?}");
    }
}

/// Lead r1 #5 (Codex P2): a probe's negative answer is fenced by the ticker it probed. Ranking [AAA, BBB], one seat, AAA
/// unpriced; AAA's venue symbol is corrected while BBB is probed: CHANGED, nothing written.
#[tokio::test(flavor = "current_thread")]
async fn an_unpriced_probe_whose_ticker_changed_retries() {
    let u = universe();
    exec(&u.o, "UPDATE indices SET top_coins = '[\"AAA.US\",\"BBB.US\"]' WHERE external_id = 'nasdaq-100'");
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 1, 1.0)]));
    let (side, aaa) = (side_conn(&u), u.m[0].1);
    let t = script([0.0, 200.0, 300.0, 400.0, 500.0]);
    let r = refresh(&u, hooked(&t, "/v2/stocks/BBB/quotes/latest", move || { side.execute("UPDATE tickers SET ticker = 'AAA.X', base = 'AAA.X' WHERE id = ?1", [aaa]).unwrap(); }), id).await;
    assert_eq!(r, Err(index::Refusal::Transient(index::CHANGED.into())));
    assert!(members(&u.o, id).is_empty() && group_rows(&u.o, id).is_empty(), "nothing written");
}

/// Lead r1 #6 (Codex P2): the below-minimum rebalance sell's write checks the grouped claim. AAA is 0.25 USD over its
/// share (under the 1 USD minimum); the composition moves while AAA's price is read: no below-minimum flag, no claim.
#[tokio::test(flavor = "current_thread")]
async fn a_below_minimum_rebalance_sell_checks_the_grouped_claim() {
    let u = universe();
    let id = grouped_bot(&u, json!({ u.s.btc.to_string(): 0.5 }), json!([group("nasdaq-100", 2, 0.5)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    listed(&u);
    hold(&u, id, u.s.btc, "0.000003906", "0.25", "64000", 1);
    hold(&u, id, u.m[0].0, "0.005", "0.5", "100", 2);
    hold(&u, id, u.m[1].0, "0.00125", "0.25", "200", 3);
    exec(&u.o, &format!("UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01) WHERE id = {id}"));
    let t = script_with([100.0, 200.0, 300.0, 400.0, 500.0], json!({ "GET /v2/positions": ok(json!([
        { "symbol": "BTCUSD", "asset_class": "crypto", "qty": "0.000003906", "qty_available": "0.000003906", "side": "long" },
        { "symbol": "AAA", "asset_class": "us_equity", "qty": "0.005", "qty_available": "0.005", "side": "long" },
        { "symbol": "BBB", "asset_class": "us_equity", "qty": "0.00125", "qty_available": "0.00125", "side": "long" }])) }));
    let prices = PriceCache::default();
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let out = rebalance::run(&u.o.primary, &venue(moves_aaa(&u, &t, "/v2/stocks/AAA/quotes/latest", id)), id, &FixedClock(at(T0)), &cx(&u.o, &bot, &prices)).await.unwrap();
    assert!(matches!(out, Outcome::Skipped("changed")), "{out:?}");
    assert!(t.posted_orders().is_empty());
    let after = model::load_bot(&u.o.primary, id).unwrap();
    assert!(after.transient.get(rebalance::BELOW_MINIMUM_KEY).is_none_or(Value::is_null), "no below-minimum flag: {}", after.transient);
    let claims: i64 = u.o.primary.query_row("SELECT count(*) FROM rust_wash_sale_claims", [], |r| r.get(0)).unwrap_or(0);
    assert_eq!(claims, 0, "no wash-sale claim row");
}

/// Lead r1 #7 (Codex P2): every early refusal of a grouped refresh forgets the composition (revision-checked): a missing
/// provider, an empty group list.
#[tokio::test(flavor = "current_thread")]
async fn an_early_grouped_refusal_forgets_the_composition() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    assert!(composed_of(&u.o, id).is_object());
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups', json('[]')) WHERE id = {id}"));
    assert!(refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.is_err());
    assert_eq!(composed_of(&u.o, id), Value::Null, "an empty group list forgets it");
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    exec(&u.o, "DELETE FROM app_configs WHERE key = 'market_data_provider'");
    assert_eq!(refresh(&u, script([100.0, 200.0, 300.0, 400.0, 500.0]), id).await, Err(index::Refusal::Failure("Index provider not configured".into())));
    assert_eq!(composed_of(&u.o, id), Value::Null, "a missing provider forgets it");
}

/// Lead r1 #7 (Codex P2): `record` fails when its update lands nowhere (a transient that is not a JSON object): the members
/// roll back with it.
#[tokio::test(flavor = "current_thread")]
async fn a_composition_record_that_lands_nowhere_fails_the_write() {
    let u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 2, 1.0)]));
    exec(&u.o, &format!("UPDATE bots SET transient_data = '[]' WHERE id = {id}"));
    let r = index::refresh_composition(&u.o.primary, &venue(script([100.0, 200.0, 300.0, 400.0, 500.0])), &model::load_bot(&u.o.primary, id).unwrap(),
                                       &FixedClock(at(T0)), &PriceCache::default()).await;
    assert!(!matches!(r, Ok(Ok(()))), "{r:?}");
    assert!(members(&u.o, id).is_empty() && group_rows(&u.o, id).is_empty(), "nothing committed without its provenance");
}
