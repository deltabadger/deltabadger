//! PLAN index-as-ingredient slice 1: the group table's storage. A bot stored with `index_groups` reads
//! `rust_bot_group_members`; a `Bots::DcaIndex` row reads exactly what it read before; a grouped row never trades as a
//! plain basket (slice 2 runs it).
mod common;
use common::seed::{self, BotSpec};
use chrono::{DateTime, Utc};
use common::scripted::{accepted, ok};
use deltabadger::engine::rebalance::{self, Outcome};
use deltabadger::engine::tick::{self, Attempts, PriceCache, TickContext, TickOutcome};
use deltabadger::engine::{basket, index, model, placement, redeploy, FixedClock};
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::store;
use serde_json::json;
use std::collections::HashSet;

const T0: &str = "2026-09-01T12:30:00Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn script() -> ScriptedTransport {
    ScriptedTransport::from_script(&json!({
        "GET /v1beta3/crypto/us/latest/quotes": [ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 }, "ETH/USD": { "ap": 2500, "bp": 2500 }, "SOL/USD": { "ap": 100, "bp": 100 } } }))],
        "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 }, "ETH/USD": { "p": 2500 }, "SOL/USD": { "p": 100 } } }))],
        "POST /v2/orders": [accepted("OTX-1"), accepted("OTX-2"), accepted("OTX-3")],
        "GET /v2/account": [ok(json!({ "cash": "100000", "non_marginable_buying_power": "100000" }))],
        "GET /v2/positions": [ok(json!([]))],
    }))
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
fn exec(o: &store::Opened, sql: &str) { o.primary.execute_batch(sql).unwrap(); }
fn members(o: &store::Opened, id: i64) -> Vec<(i64, i64, String, Option<bool>)> {
    o.primary.prepare("SELECT asset_id, ticker_id, CAST(target_allocation AS TEXT), in_index FROM bot_index_assets WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}
fn group_settings(o: &store::Opened, id: i64) {
    exec(o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups', json('[{{\"index_type\":\"top\",\"num_coins\":3,\"weight\":0.5}}]')) WHERE id = {id}"));
}
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
    group_settings(&o, id);
    exec(&o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, target_allocation, created_at, updated_at) VALUES
          ({id}, {btc}, {bt}, 1, 0.5, '2026-01-01', '2026-01-01'), ({id}, {eth}, {et}, 1, 0.5, '2026-01-01', '2026-01-01');
        INSERT INTO rust_bot_group_members VALUES ({id}, 'top-coins', {btc}, {bt}, 1, 0.6), ({id}, 'top-coins', {eth}, 777, 1, 0.4),
                                                  ({id}, 'layer-1', {eth}, {et}, 0, 1.0);", btc = s.btc, bt = s.ticker_id, et = eth_ticker));
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(bot.stored_groups() && !bot.has_groups());
    assert!(index::composed_here(&o.primary, &bot, "top-coins").unwrap());
    assert!(!index::composed_here(&o.primary, &bot, "layer-1").unwrap(), "composed = 0");
    assert!(!index::composed_here(&o.primary, &bot, "defi").unwrap(), "no rows");
    assert_eq!(index::incumbents(&o.primary, &bot, "top-coins").unwrap(), HashSet::from([s.ticker_id]), "ETH's stored ticker is no longer its in_index one");
    assert_eq!(index::incumbents(&o.primary, &bot, "layer-1").unwrap(), HashSet::from([eth_ticker]), "each group its own rows");
    // The snapshot fences the group rows too, ordered by group then asset.
    let snapshot = placement::composition_snapshot(&o.primary, &bot).unwrap();
    assert_eq!(snapshot["groups"], json!([["layer-1", eth, eth_ticker, 0, 1.0], ["top-coins", s.btc, s.ticker_id, 1, 0.6], ["top-coins", eth, 777, 1, 0.4]]));
    exec(&o, &format!("UPDATE rust_bot_group_members SET weight = 0.5 WHERE bot_id = {id} AND asset_id = {}", s.btc));
    assert_ne!(placement::composition_snapshot(&o.primary, &bot).unwrap(), snapshot, "a group row's change moves the snapshot");
    // Read as f64: a weight one ulp off is a change (SQLite's json_array would print both as 0.5).
    exec(&o, &format!("UPDATE rust_bot_group_members SET weight = 0.5 WHERE bot_id = {id} AND asset_id = {}", s.btc));
    let half = placement::composition_snapshot(&o.primary, &bot).unwrap();
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

/// A row stored with `index_groups` (a later build's) never trades as a plain basket, on any path that bypasses the install
/// check: a stopped one's redeploy (Codex: it passes the check with rebalancing off), a rebalance pass and a tick each
/// refuse in the shared refresh (basket::GROUPED) before anything is written. Its membership, a group-only SOL included,
/// stays as it was, and nothing is sent.
#[tokio::test(flavor = "current_thread")]
async fn a_grouped_row_neither_redeploys_rebalances_nor_ticks_as_a_plain_basket() {
    let (d, o, s) = common::install_alpaca();
    let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
        .with("rebalance_enabled", json!(false)).weights(&[(s.btc, 0.5), (eth, 0.5)]));
    basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    let sol_ticker: i64 = o.primary.query_row("SELECT id FROM tickers WHERE base_asset_id = ?1", [sol], |r| r.get(0)).unwrap();
    exec(&o, &format!("UPDATE bot_index_assets SET target_allocation = 0.25 WHERE bot_id = {id};
        INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, in_index, target_allocation, created_at, updated_at) VALUES ({id}, {sol}, {sol_ticker}, 1, 0.5, '2026-01-01', '2026-01-01');"));
    // Held: 640 USD of BTC, 100 of ETH, and 100 USD banked from a sold SOL holding (the redeploy's offer).
    for (asset, qty, value, price, n) in [(s.btc, "0.01", "640", "64000", 1), (eth, "0.04", "100", "2500", 2), (sol, "1", "100", "100", 3)] {
        seed::insert_row(&o.primary, &s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty, "amount_exec": qty,
            "quote_amount": value, "quote_amount_exec": value, "price": price, "created_at": "2026-09-01 10:00:01" }));
    }
    let sold = seed::insert_row(&o.primary, &s, id, sol, &json!({ "external_status": 2, "external_id": "OSOLD", "side": 1, "amount": "1", "amount_exec": "1",
        "quote_amount": "100", "quote_amount_exec": "100", "price": "100", "created_at": "2026-09-01 10:00:02" }));
    exec(&o, &format!("UPDATE transactions SET transaction_type = 'LIQUIDATION' WHERE id = {sold}"));
    group_settings(&o, id);
    let before = members(&o, id);
    assert_eq!(before.len(), 3);
    let orders = || o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap();
    let rows = orders();
    let t = script();
    let prices = PriceCache::default();
    let cx = |bot: &model::Bot| TickContext { credential_version: model::credential_version(&o.primary, bot).unwrap(), prices: &prices,
                                              process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    // A queued redeploy of the stopped bot.
    model::merge_transient_compact(&o.primary, id, &[(redeploy::REQUEST_KEY, json!({"id": "yes-1", "at": at(T0).timestamp()}))]).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let _ = redeploy::run(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &cx(&bot)).await;
    assert_eq!((t.posted_orders().len(), members(&o, id), orders()), (0, before.clone(), rows), "redeploy");
    // A rebalance pass, the rule on.
    exec(&o, &format!("UPDATE bots SET settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01),
        transient_data = json_remove(transient_data, '$.{}') WHERE id = {id}", redeploy::REQUEST_KEY));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let rebalanced = rebalance::run(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &cx(&bot)).await.unwrap();
    assert!(!matches!(rebalanced, Outcome::Placed(_)), "{rebalanced:?}");
    assert_eq!((t.posted_orders().len(), members(&o, id), orders()), (0, before.clone(), rows), "rebalance");
    // A due tick of the bot, scheduled.
    exec(&o, &format!("UPDATE bots SET status = 1 WHERE id = {id}"));
    let ticked = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at("2026-09-08T10:00:01Z")), &mut Attempts::default()).await.unwrap();
    assert!(!matches!(ticked, TickOutcome::Done { placed: true }), "{ticked:?}");
    assert_eq!((t.posted_orders().len(), members(&o, id), orders()), (0, before, rows), "tick");
    drop(d);
}

/// The install check `run::step` reads before any tick or rebalance names a grouped row, so the loop never picks it up.
#[test]
fn a_grouped_row_is_refused_by_the_install_check() {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("rebalance_enabled", json!(true))
        .with("rebalance_threshold", json!(0.01)).weights(&[(s.btc, 0.5), (eth, 0.5)]));
    let plain = deltabadger::engine::eligibility::check_install(&o.primary).unwrap();
    assert!(plain.eligible.contains(&id) && plain.problems.is_empty(), "the same basket without groups runs: {:?}", plain.problems);
    group_settings(&o, id);
    let report = deltabadger::engine::eligibility::check_install(&o.primary).unwrap();
    assert!(!report.eligible.contains(&id), "never ticked");
    assert!(report.problems.iter().any(|p| p.starts_with(&format!("bot {id} ")) && p.contains("index groups")), "{:?}", report.problems);
}
