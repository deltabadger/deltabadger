//! The reference-data jobs (rust/src/jobs/{import,reference}.rs). Row parity with Rails is
//! rust/tests/reference_parity.rs; these pin the Rust-side mechanics parity cannot see.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::jobs::{import, Db};
use deltabadger::store::{self, Paths};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn db() -> (tempfile::TempDir, Connection) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o.primary)
}
fn reopen(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }
fn exchange(c: &Connection, ty: &str, available: bool) -> i64 {
    c.execute("INSERT INTO exchanges (type, name, available, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?1, ?2, '0.1', '0.1', '2026-01-01', '2026-01-01')",
              rusqlite::params![ty, available]).unwrap();
    c.last_insert_rowid()
}
fn asset(c: &Connection, ext: &str, symbol: &str, category: &str) -> i64 {
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, '2026-01-01', '2026-01-01')",
              [ext, symbol, category]).unwrap();
    c.last_insert_rowid()
}
fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T { c.query_row(sql, [], |r| r.get(0)).unwrap() }
/// A Kraken EUR pair as data-api sends it.
fn pair(ext: &str, ticker: &str, base: &str) -> Value {
    json!({ "ticker": ticker, "base": base, "quote": "EUR", "base_external_id": ext, "quote_external_id": "EUR.FOREX", "base_decimals": 8,
            "quote_decimals": 2, "price_decimals": 1, "minimum_base_size": "0.0001", "minimum_quote_size": "0.5", "maximum_base_size": null,
            "maximum_quote_size": "1000000" })
}
/// Plans `rows` for the venue and publishes the plan, as the jobs do.
async fn import_tickers(db: &Db, exchange_id: i64, rows: Vec<Value>, now: &str) -> Result<Vec<i64>, String> {
    let plan = db.run(move |c, _| import::plan_tickers(c, exchange_id, &rows, None)).await?;
    import::publish_tickers(db, exchange_id, plan, at(now)).await
}
/// Every ticker row, every column, by id.
fn tickers(c: &Connection) -> HashMap<i64, Vec<String>> {
    let mut s = c.prepare("SELECT id, ticker, base, quote, minimum_base_size, minimum_quote_size, maximum_quote_size, price_decimals, available, \
                           trading_enabled, updated_at FROM tickers").unwrap();
    s.query_map([], |r| Ok((r.get::<_, i64>(0)?, (1..11).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect::<Result<Vec<_>, _>>()?)))
        .unwrap().collect::<Result<_, _>>().unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn import_tickers_dedups_skips_and_realigns_as_marketdata_does() {
    let (d, c) = db();
    let kraken = exchange(&c, "Exchanges::Kraken", true);
    let (btc, eth, sol, eur) = (asset(&c, "bitcoin", "BTC", "Cryptocurrency"), asset(&c, "ethereum", "ETH", "Cryptocurrency"),
                                asset(&c, "solana", "SOL", "Cryptocurrency"), asset(&c, "EUR.FOREX", "EUR", "Currency"));
    // A row of another asset pair holds the symbol XBTEUR; the ethereum pair is stored under old names.
    c.execute_batch(&format!("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
        minimum_base_size, minimum_quote_size, created_at, updated_at) VALUES \
        ({kraken}, 'XBTEUR', 'XBT', 'EUR', {sol}, {eur}, 8, 2, 2, 1, 1, '2026-01-01', '2026-01-01'), \
        ({kraken}, 'ETHEUR-OLD', 'ETH-OLD', 'EUR', {eth}, {eur}, 8, 2, 2, 1, 1, '2026-01-01', '2026-01-01');")).unwrap();
    let mut no_price = pair("solana", "SOLEUR", "SOL");
    no_price["price_decimals"] = json!(null);
    let rows = vec![pair("bitcoin", "XBTEUR", "XBT"), pair("ethereum", "ETHEUR", "ETH"), pair("ethereum", "ETHEUR2", "ETH2"), no_price,
                    pair("dogecoin", "XDGEUR", "XDG")];
    let written = import_tickers(&Db::new(c, common::seed::cipher()), kraken, rows, "2026-10-02T10:15:00Z").await.unwrap();
    let c = reopen(&d);
    assert_eq!(written, vec![btc, eth], "deduped by asset pair (first wins); no price decimals and an unknown asset are skipped");
    assert_eq!(one::<String>(&c, &format!("SELECT ticker FROM tickers WHERE base_asset_id = {sol}")), "__stale_1_XBTEUR",
               "the other pair's holder is tombstoned");
    assert_eq!(one::<i64>(&c, &format!("SELECT available FROM tickers WHERE base_asset_id = {sol}")), 0);
    assert_eq!(one::<String>(&c, &format!("SELECT ticker || ' ' || base FROM tickers WHERE base_asset_id = {eth}")), "ETHEUR ETH",
               "the same pair is realigned onto the incoming names, keeping its id");
    assert_eq!(one::<String>(&c, &format!("SELECT created_at FROM tickers WHERE base_asset_id = {eth}")), "2026-10-02 10:15:00",
               "upsert_all overwrites created_at: the timestamps are given columns");
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM exchange_assets"), 4, "every resolved asset of the raw payload, solana included");
}

#[tokio::test(flavor = "current_thread")]
async fn floating_sizes_are_stored_as_rubys_bigdecimal_of_the_float() {
    let (d, c) = db();
    let kraken = exchange(&c, "Exchanges::Kraken", true);
    asset(&c, "bitcoin", "BTC", "Cryptocurrency");
    asset(&c, "EUR.FOREX", "EUR", "Currency");
    let mut row = pair("bitcoin", "XBTEUR", "XBT");
    row["minimum_base_size"] = json!(1.0000000000000002);   // Ruby: BigDecimal(1.0000000000000002) => 1.0
    row["minimum_quote_size"] = json!(0.30000000000000004); // 0.1 + 0.2; Ruby: BigDecimal(0.30000000000000004) => 0.3
    row["maximum_quote_size"] = json!(1000000);             // an Integer: exact
    row["maximum_base_size"] = json!(0.12345678901234568);  // 17th digit 8; Ruby truncates: BigDecimal => 0.1234567890123456
    import_tickers(&Db::new(c, common::seed::cipher()), kraken, vec![row], "2026-10-02T10:15:00Z").await.unwrap();
    let c = reopen(&d);
    assert_eq!(one::<f64>(&c, "SELECT minimum_base_size FROM tickers"), 1.0);
    assert_eq!(one::<i64>(&c, "SELECT minimum_quote_size <= 0.3 FROM tickers"), 1,
               "an order of 0.30 EUR meets the minimum, as in Rails; the float's own digits (0.30000000000000004) would refuse it");
    assert_eq!(one::<i64>(&c, "SELECT maximum_quote_size FROM tickers"), 1_000_000);
    assert_eq!(one::<f64>(&c, "SELECT maximum_base_size FROM tickers"), 0.1234567890123456,
               "Float#to_d truncates to 16 digits; rounding would give 0.1234567890123457");
}

#[tokio::test(flavor = "current_thread")]
async fn a_conflict_key_met_twice_is_applied_in_order_and_the_last_row_wins() {
    let (d, c) = db();
    let db = Db::new(c, common::seed::cipher());
    let coin = |symbol: &str| json!({ "external_id": "dup", "symbol": symbol, "name": symbol, "category": "Cryptocurrency" });
    import::import_assets(&db, import::plan_assets(&[coin("D"), coin("D2")], "https://data.example").unwrap(), at("2026-10-02T00:20:00Z")).await.unwrap();
    let index = |name: &str| json!({ "external_id": "layer-1", "source": "coingecko", "name": name });
    import::import_indices(&db, import::plan_indices(&[index("L1"), index("Layer 1")]).unwrap(), at("2026-10-02T10:30:00Z")).await.unwrap();
    let c = reopen(&d);
    assert_eq!(one::<String>(&c, "SELECT symbol FROM assets WHERE external_id = 'dup'"), "D2", "as Rails 8.1.4's upsert_all on SQLite");
    assert_eq!(one::<String>(&c, "SELECT name FROM indices"), "Layer 1");
}

#[tokio::test(flavor = "current_thread")]
async fn assets_publish_in_chunks_and_the_tokenized_registry_is_null_safe() {
    let (d, c) = db();
    let rows: Vec<Value> = (0..1_201).map(|i| json!({ "external_id": format!("coin-{i}"), "symbol": "C", "name": "C",
        "category": "Cryptocurrency", "market_cap": 1.5e9, "circulating_supply": 120456789.12345678 })).collect();
    let plan = import::plan_assets(&rows, "https://data.example").unwrap();
    assert_eq!(import::chunks(&plan.rows).len(), 3, "500 + 500 + 201 rows");
    let db = Db::new(c, common::seed::cipher());
    import::import_assets(&db, plan, at("2026-10-02T00:20:00Z")).await.unwrap();
    let c = reopen(&d);
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM assets"), 1_201);
    assert_eq!(one::<f64>(&c, "SELECT circulating_supply FROM assets LIMIT 1"), 120456789.1234568, "BigDecimal(f.round(8), 16).round(8)");
    asset(&c, "nvda-xstock", "NVDAX", "Cryptocurrency");
    import::mark_tokenized(&db).await.unwrap();
    assert_eq!(one::<String>(&c, "SELECT instrument_type FROM assets WHERE external_id = 'nvda-xstock'"), "tokenized");
}

/// `n` Kraken EUR pairs stored under old names with old parameters; returns the venue and the base asset ids.
fn renamed_venue(c: &Connection, n: usize) -> (i64, Vec<i64>) {
    let kraken = exchange(c, "Exchanges::Kraken", true);
    let eur = asset(c, "EUR.FOREX", "EUR", "Currency");
    let bases: Vec<i64> = (0..n).map(|i| asset(c, &format!("coin-{i}"), &format!("C{i}"), "Cryptocurrency")).collect();
    for (i, b) in bases.iter().enumerate() {
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, created_at, updated_at) VALUES (?1, ?2, ?3, 'EUR', ?4, ?5, 8, 2, 2, 1, 1, '2026-01-01', '2026-01-01')",
                  rusqlite::params![kraken, format!("OLD{i}EUR"), format!("OLD{i}"), b, eur]).unwrap();
    }
    (kraken, bases)
}

/// The payload for `renamed_venue`: every pair renamed (`C<i>EUR`) with new parameters, except the last pair of the first
/// unit and the first of the second, which swap their old names: a rename chain across what would be a unit boundary.
fn renaming_payload(n: usize) -> Vec<Value> {
    let (a, b) = (deltabadger::jobs::CHUNK - 1, deltabadger::jobs::CHUNK);
    (0..n).map(|i| {
        let name = if i == a { format!("OLD{b}") } else if i == b { format!("OLD{a}") } else { format!("C{i}") };
        let mut p = pair(&format!("coin-{i}"), &format!("{name}EUR"), &name);
        p["minimum_base_size"] = json!("0.5");
        p
    }).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_reader_between_two_units_sees_every_ticker_either_old_or_new_never_half_written() {
    let n = deltabadger::jobs::CHUNK + 100;
    let (d, c) = db();
    let (kraken, _) = renamed_venue(&c, n);
    let old = tickers(&c);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (stop, file) = (stop.clone(), d.path().join("production.sqlite3"));
        std::thread::spawn(move || {
            let c = Connection::open(file).unwrap();
            let mut seen = vec![];
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                seen.push(tickers(&c));
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            seen
        })
    };
    import_tickers(&Db::new(c, common::seed::cipher()), kraken, renaming_payload(n), "2026-10-02T10:15:00Z").await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = reader.join().unwrap();
    let new = tickers(&reopen(&d));
    assert!(old.keys().all(|id| old[id] != new[id]), "every row changed: names, parameters and stamps");
    let mut mixed = 0;
    for snapshot in &seen {
        for (id, row) in snapshot {
            assert!(row == &old[id] || row == &new[id], "ticker {id} was seen half-written: {row:?}");
        }
        if snapshot.iter().any(|(id, r)| r == &old[id]) && snapshot.iter().any(|(id, r)| r == &new[id]) { mixed += 1; }
    }
    assert!(mixed > 0, "the reader saw a state between two units ({} snapshots)", seen.len());
    let (a, b) = (deltabadger::jobs::CHUNK - 1, deltabadger::jobs::CHUNK);
    let names: Vec<String> = reopen(&d).prepare("SELECT ticker FROM tickers ORDER BY id").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!((names[a].clone(), names[b].clone()), (format!("OLD{b}EUR"), format!("OLD{a}EUR")), "the swap across the boundary landed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failure_publishes_no_ticker_half_written() {
    let n = deltabadger::jobs::CHUNK + 100;
    // Before any write: an unreadable size fails the plan, and nothing is written, not even an exchange asset.
    let (d, c) = db();
    let (kraken, _) = renamed_venue(&c, n);
    let old = tickers(&c);
    let mut payload = renaming_payload(n);
    payload[n - 2]["minimum_quote_size"] = json!("abc");
    let db = Db::new(c, common::seed::cipher());
    assert!(import_tickers(&db, kraken, payload, "2026-10-02T10:15:00Z").await.unwrap_err().contains("invalid value for BigDecimal()"));
    let c = reopen(&d);
    assert_eq!(tickers(&c), old);
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM exchange_assets"), 0);
    // During the writes: the last unit fails. The first unit's tickers are wholly new, the last unit's wholly old.
    c.execute_batch(&format!("CREATE TRIGGER late_failure BEFORE UPDATE OF ticker ON tickers WHEN NEW.ticker = 'C{}EUR' \
                              BEGIN SELECT RAISE(ABORT, 'late failure'); END;", n - 2)).unwrap();
    assert!(import_tickers(&db, kraken, renaming_payload(n), "2026-10-02T10:15:00Z").await.unwrap_err().contains("late failure"));
    let after = tickers(&c);
    let (changed, kept) = after.iter().fold((0, 0), |(ch, k), (id, row)| {
        assert!(row == &old[id] || (row[0] != old[id][0] && row[3] != old[id][3]), "ticker {id} was left half-written: {row:?}");
        if row == &old[id] { (ch, k + 1) } else { (ch + 1, k) }
    });
    assert!(changed > 0 && kept > 0, "the first unit committed ({changed} new), the failed one did not ({kept} old)");
}

#[tokio::test(flavor = "current_thread")]
async fn a_rename_group_is_only_its_colliding_rows_and_the_bound_is_enforced() {
    let n = 6_800;
    let (d, c) = db();
    let (kraken, _) = renamed_venue(&c, n);
    // The first and the last pair swap their old names, 6,799 rows apart; every other pair keeps its old name.
    let swap = |n: usize, chain: usize| -> Vec<Value> {
        (0..n).map(|i| {
            let name = if i < chain { format!("OLD{}", (i + 1) % chain) } else { format!("OLD{i}") };
            pair(&format!("coin-{i}"), &format!("{name}EUR"), &name)
        }).collect()
    };
    let mut payload = swap(n, 0);
    (payload[0], payload[n - 1]) = (pair("coin-0", &format!("OLD{}EUR", n - 1), &format!("OLD{}", n - 1)), pair(&format!("coin-{}", n - 1), "OLD0EUR", "OLD0"));
    let db = Db::new(c, common::seed::cipher());
    let plan = db.run(move |c, _| import::plan_tickers(c, kraken, &payload, None)).await.unwrap();
    let sizes: Vec<usize> = plan.units.iter().map(|u| u.writes.len()).collect();
    assert!(sizes.iter().all(|&k| k <= deltabadger::jobs::CHUNK), "{sizes:?}");
    assert_eq!(sizes.len(), n.div_ceil(deltabadger::jobs::CHUNK), "the swap does not merge the rows between: {sizes:?}");
    assert_eq!(plan.units.iter().map(|u| u.moving.len()).sum::<usize>(), 2, "only the two swapped rows move");
    let moving_unit = plan.units.iter().find(|u| !u.moving.is_empty()).unwrap();
    assert_eq!(moving_unit.moving.len(), 2, "both halves of the swap share one unit");
    import::publish_tickers(&db, kraken, plan, at("2026-10-02T10:15:00Z")).await.unwrap();
    let names: Vec<String> = reopen(&d).prepare("SELECT ticker FROM tickers ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!((names[0].clone(), names[n - 1].clone()), (format!("OLD{}EUR", n - 1), "OLD0EUR".to_string()));
    // A rotation through CHUNK + 1 pairs is one group larger than a unit: the plan fails before any write.
    // Through the tickers job, as data-api would send it: the refusal comes before the exchange-assets phase.
    let rotation = swap(n, deltabadger::jobs::CHUNK + 1);
    let (before, assets_before) = (tickers(&reopen(&d)), one::<String>(&reopen(&d), "SELECT group_concat(updated_at) FROM exchange_assets"));
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/tickers/kraken", 200, json!({ "data": rotation }));
    let out = run(reference::TICKERS, scripted(&t), reopen(&d), "2026-10-02T11:15:00Z").await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("larger than one write unit")), "{out:?}");
    assert_eq!(tickers(&reopen(&d)), before, "no ticker written");
    assert_eq!(one::<String>(&reopen(&d), "SELECT group_concat(updated_at) FROM exchange_assets"), assets_before, "no exchange asset touched");
}

#[tokio::test(flavor = "current_thread")]
async fn every_write_unit_keeps_the_gap_across_phase_boundaries() {
    let (d, c) = db();
    let file = c.path().unwrap().to_string();
    // Three phases of one unit each: the upsert, the payload's instrument types, the tokenized registry.
    let rows = vec![json!({ "external_id": "pax-gold", "symbol": "PAXG", "name": "PAXG", "category": "Cryptocurrency", "instrument_type": null })];
    let (stop, longest) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)));
    let probe = {
        let (stop, longest, path) = (stop.clone(), longest.clone(), d.path().join("production.sqlite3"));
        std::thread::spawn(move || {
            let p = Connection::open(path).unwrap();
            p.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let t0 = std::time::Instant::now();
                p.execute_batch("BEGIN IMMEDIATE; COMMIT;").unwrap();
                longest.fetch_max(t0.elapsed().as_micros() as u64, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    };
    import::import_assets(&Db::new(c, common::seed::cipher()), import::plan_assets(&rows, "https://data.example").unwrap(), at("2026-10-02T00:20:00Z")).await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    probe.join().unwrap();
    assert_eq!(import::holds(&file).len(), 3, "three phases ran: {:?}", import::holds(&file));
    let gap = import::shortest_gap(&file).expect("two units or more");
    assert!(gap >= import::CHUNK_GAP, "a unit followed the previous one {gap:?} after it, across a phase boundary");
    let wait = std::time::Duration::from_micros(longest.load(std::sync::atomic::Ordering::SeqCst));
    assert!(wait < std::time::Duration::from_millis(100) + import::CHUNK_GAP, "another writer waited {wait:?} at a phase boundary");
}

#[tokio::test(flavor = "current_thread")]
async fn a_unit_that_rolls_back_still_keeps_the_gap_before_the_next_phase() {
    let (d, c) = db();
    let file = c.path().unwrap().to_string();
    let kraken = exchange(&c, "Exchanges::Kraken", true);
    asset(&c, "bitcoin", "BTC", "Cryptocurrency");
    asset(&c, "EUR.FOREX", "EUR", "Currency");
    // The tickers unit fails and rolls back; the assets import that follows is another phase on the same file.
    c.execute_batch("CREATE TRIGGER failing_unit BEFORE INSERT ON tickers BEGIN SELECT RAISE(ABORT, 'unit failed'); END;").unwrap();
    // When the assets unit wrote, by the wall clock (a rolled-back unit cannot leave a mark of its own).
    c.execute_batch("CREATE TABLE unit_marks (at TEXT); CREATE TRIGGER mark_unit AFTER INSERT ON assets \
                     BEGIN INSERT INTO unit_marks VALUES (strftime('%Y-%m-%d %H:%M:%f', 'now')); END;").unwrap();
    let (stop, longest) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)));
    let probe = {
        let (stop, longest, path) = (stop.clone(), longest.clone(), d.path().join("production.sqlite3"));
        std::thread::spawn(move || {
            let p = Connection::open(path).unwrap();
            p.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let t0 = std::time::Instant::now();
                p.execute_batch("BEGIN IMMEDIATE; COMMIT;").unwrap();
                longest.fetch_max(t0.elapsed().as_micros() as u64, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    };
    let db = Db::new(c, common::seed::cipher());
    assert!(import_tickers(&db, kraken, vec![pair("bitcoin", "XBTEUR", "XBT")], "2026-10-02T10:15:00Z").await.unwrap_err().contains("unit failed"));
    let rolled_back = Utc::now(); // the failed unit has released the lock by now
    let rows = vec![json!({ "external_id": "ethereum", "symbol": "ETH", "name": "ETH", "category": "Cryptocurrency" })];
    import::import_assets(&db, import::plan_assets(&rows, "https://data.example").unwrap(), at("2026-10-02T10:20:00Z")).await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    probe.join().unwrap();
    assert!(import::holds(&file).contains_key("tickers"), "the rolled-back unit was released and recorded: {:?}", import::holds(&file));
    // The gap itself: the assets unit waited CHUNK_GAP from the rollback's release (a few µs before `rolled_back`), not from
    // the exchange-assets unit before it, which ended more than CHUNK_GAP earlier. Half the gap tells the two apart.
    let wrote: String = one(&reopen(&d), "SELECT min(at) FROM unit_marks");
    let wrote = chrono::NaiveDateTime::parse_from_str(&wrote, "%Y-%m-%d %H:%M:%S%.f").unwrap().and_utc();
    let after = (wrote - rolled_back).to_std().unwrap_or_default();
    assert!(after >= import::CHUNK_GAP / 2, "the assets unit wrote {after:?} after the rolled-back unit: its release was not recorded");
    let gap = import::shortest_gap(&file).expect("several units");
    assert!(gap >= import::CHUNK_GAP, "a unit started {gap:?} after the previous one released the lock");
    let wait = std::time::Duration::from_micros(longest.load(std::sync::atomic::Ordering::SeqCst));
    assert!(wait < std::time::Duration::from_millis(100) + import::CHUNK_GAP, "another writer waited {wait:?}");
}

use common::seed::{self, BotSpec};
use deltabadger::engine::FixedClock;
use deltabadger::jobs::data_api::{Config, DataApi};
use deltabadger::jobs::{reference, Cx, Outcome};
use deltabadger::venue::http::ScriptedTransport;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn scripted(t: &ScriptedTransport) -> Option<DataApi<ScriptedTransport>> {
    Some(DataApi::new(Config { url: "http://data-api:3000".into(), token: "tok".into() }, t.clone(), t.clone()))
}
async fn run(name: &str, api: Option<DataApi<ScriptedTransport>>, c: Connection, now: &str) -> Outcome {
    reference::run_once(name, api, Cx { db: Db::new(c, seed::cipher()), clock: &FixedClock(at(now)) }).await
}

#[tokio::test(flavor = "current_thread")]
async fn without_the_deltabadger_provider_every_data_api_job_fails_and_prune_still_runs() {
    for spec in reference::specs() {
        let (_d, c) = db();
        let out = run(spec.name, None, c, "2026-10-02T10:30:00Z").await;
        if spec.name == reference::PRUNE { assert_eq!(out, Outcome::Done); }
        else { assert!(matches!(&out, Outcome::Failed(m) if m.contains("not deltabadger")), "{}: {out:?}", spec.name); }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_stock_jobs_switch_and_a_legacy_row_stop_it_before_any_request() {
    let (d, c) = db();
    deltabadger::app_config::set(&c, &seed::cipher(), "stock_sync_enabled", "false", at("2026-10-01T00:00:00Z")).unwrap();
    let t = ScriptedTransport::default(); // any request would panic: nothing is scripted
    assert!(matches!(run(reference::STOCKS, scripted(&t), c, "2026-10-02T10:05:00Z").await, Outcome::Failed(m) if m.contains("switched off")));
    let c = reopen(&d);
    c.execute("DELETE FROM app_configs", []).unwrap();
    asset(&c, "alpaca_0b5c", "IBIT", "Stock");
    assert!(matches!(run(reference::STOCKS, scripted(&t), c, "2026-10-02T10:05:00Z").await, Outcome::Failed(m) if m.contains("legacy")));
    assert!(t.requests().is_empty());
    assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM app_configs"), 0, "not even the backfill flag");
}

#[tokio::test(flavor = "current_thread")]
async fn rate_limits_and_network_failures_retry_and_other_failures_do_not() {
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/listings", 429, json!({ "error": "slow down" }))
        .network("GET /api/v2/listings", "post_send", "Faraday::TimeoutError: Net::ReadTimeout")
        .reply("GET /api/v2/listings", 500, json!({ "error": "boom" }));
    for expected in ["RateLimited", "Transient", "Failed"] {
        let (_d, c) = db();
        let out = run(reference::ALPACA_CRYPTO, scripted(&t), c, "2026-10-02T10:15:00Z").await;
        assert!(format!("{out:?}").starts_with(expected), "{expected}: {out:?}");
    }
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/indices", 500, json!({ "error": "boom" })).network("GET /api/v1/assets", "post_send", "Faraday::TimeoutError: Net::ReadTimeout");
    let (_d, c) = db();
    assert!(matches!(run(reference::INDICES, scripted(&t), c, "2026-10-02T10:30:00Z").await, Outcome::Transient(_)), "PullFailed is retried");
    let (_d, c) = db();
    assert!(matches!(run(reference::ASSETS, scripted(&t), c, "2026-10-02T00:20:00Z").await, Outcome::Failed(_)), "the asset sync rescues everything");
}

#[tokio::test(flavor = "current_thread")]
async fn a_degraded_crypto_payload_creates_usd_imports_nothing_and_says_why() {
    let (d, c) = db();
    exchange(&c, "Exchanges::Alpaca", true);
    let rows: Vec<serde_json::Value> = (1..=29).map(|i| {
        asset(&c, &format!("coin-{i:02}"), &format!("C{i:02}"), "Cryptocurrency");
        json!({ "base_asset_id": format!("crypto:coin-{i:02}"), "symbol": format!("C{i:02}/USD"), "base_decimals": 8, "quote_decimals": 2, "price_decimals": 2 })
    }).collect();
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/listings?venue=alpaca_crypto", 200, json!({ "data": rows }));
    let out = run(reference::ALPACA_CRYPTO, scripted(&t), c, "2026-10-02T10:15:00Z").await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("degraded") && m.contains("29 resolved")), "{out:?}");
    let c = reopen(&d);
    assert_eq!(one::<String>(&c, "SELECT category || ' ' || color FROM assets WHERE external_id = 'usd'"), "Fiat #355E3B", "created before the guard, as Rails does");
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM tickers"), 0);
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM app_configs"), 0, "no baseline ratchet on a bailed run");
}

#[tokio::test(flavor = "current_thread")]
async fn one_venue_failing_does_not_stop_the_others() {
    let (d, c) = db();
    exchange(&c, "Exchanges::Kraken", true);
    exchange(&c, "Exchanges::Binance", true);
    exchange(&c, "Exchanges::Gemini", false); // unavailable: never asked
    exchange(&c, "Exchanges::Alpaca", true);  // a stock venue: never asked here
    asset(&c, "bitcoin", "BTC", "Cryptocurrency");
    asset(&c, "usd", "USD", "Fiat");
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/tickers/kraken", 404, json!({ "error": "Invalid exchange: kraken" }))
        .reply("GET /api/v1/tickers/binance", 200, json!({ "data": [{ "ticker": "BTCUSD", "base": "BTC", "quote": "USD", "base_external_id": "bitcoin",
            "quote_external_id": "usd", "base_decimals": 5, "quote_decimals": 2, "price_decimals": 2, "minimum_base_size": "0.00001", "minimum_quote_size": "5" }] }));
    let out = run(reference::TICKERS, scripted(&t), c, "2026-10-02T12:15:30Z").await;
    assert_eq!(out, Outcome::Failed(r#"kraken: {"error":"Invalid exchange: kraken"}"#.into()));
    assert_eq!(one::<String>(&reopen(&d), "SELECT ticker FROM tickers"), "BTCUSD");
    assert_eq!(reference::name_id("Exchanges::BinanceUs"), "binance_us");
}

#[tokio::test(flavor = "current_thread")]
async fn prune_deletes_only_rows_older_than_90_days() {
    let (d, c) = db();
    let s = seed::seed_kraken(&c, &seed::cipher());
    let bot = seed::insert_bot(&c, &s, &BotSpec::weekly(60.0, "2026-06-01 00:00:00"));
    for at in ["2026-07-03 03:59:59", "2026-07-04 04:00:00", "2026-07-05 04:00:00"] {
        c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'x', 0, '{}', ?2)", rusqlite::params![bot, at]).unwrap();
    }
    assert_eq!(run(reference::PRUNE, None, c, "2026-10-02T04:00:00Z").await, Outcome::Done);
    assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM bot_activity_logs"), 2, "exactly 90 days old stays: `created_at < 90.days.ago`");
}

const RUNTIME_THREAD_BOUND: Duration = Duration::from_millis(250); // the engine ticks on this thread
const WRITE_LOCK_BOUND: Duration = Duration::from_millis(100);     // one write unit

/// A task on the same current-thread runtime that wakes every millisecond and records its longest gap.
fn thread_gap_meter() -> (Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
    let max_us = Arc::new(AtomicU64::new(0));
    let m = max_us.clone();
    let task = tokio::spawn(async move {
        let mut last = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
            let now = Instant::now();
            m.fetch_max((now - last).as_micros() as u64, Ordering::Relaxed);
            last = now;
        }
    });
    (max_us, task)
}

#[tokio::test(flavor = "current_thread")]
async fn full_size_reference_writes_hold_neither_the_runtime_thread_nor_the_write_lock_past_their_bounds() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    // The owner's kind of install: an Alpaca paper venue with 50 bots that pass eligibility (a guarded write re-checks them
    // in every unit), a full stock catalogue imported once before under older names, and a 90-day pruning backlog.
    let (d, o, s) = common::install_alpaca();
    let c = o.primary;
    for _ in 0..50 { seed::insert_bot(&c, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")); }
    let bot: i64 = one(&c, "SELECT min(id) FROM bots");
    let alpaca = s.exchange_id;
    c.execute_batch("BEGIN").unwrap();
    for i in 0..11_600 {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Stock', '2026-01-01', '2026-01-01')",
                  [format!("S{i:05}.US"), format!("S{i:05}")]).unwrap();
        // 6,800 listed before: 6,700 still listed (every tenth renamed now), 100 delisted, whose names 100 new listings take.
        if i < 6_800 {
            c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                       minimum_base_size, minimum_quote_size, created_at, updated_at) VALUES (?1, ?2, ?2, 'USD', last_insert_rowid(), ?3, 9, 2, 2, 1, 1, '2026-01-01', '2026-01-01')",
                      rusqlite::params![alpaca, format!("S{i:05}"), s.quote]).unwrap();
        }
    }
    for n in 0..20_000 {
        c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'x', 0, '{}', ?2)",
                  rusqlite::params![bot, format!("2026-05-01 00:{:02}:{:02}", n / 60 % 60, n % 60)]).unwrap();
    }
    c.execute_batch("COMMIT").unwrap();
    let assets: Vec<Value> = (0..11_600).map(|i| json!({ "asset_id": format!("stock:S{i:05}"), "external_id": format!("S{i:05}.US"),
        "type": if i % 10 == 0 { "etf" } else { "stock" }, "symbol": format!("S{i:05}"), "name": format!("Company {i}"), "market_cap_rank": i,
        "image_url": null, "color": "#123456", "logo_url": format!("/logos/s/{i}.png") })).collect();
    let listing = |i: usize, ticker: String| json!({ "listing_id": i, "base": ticker, "quote": "USD", "ticker": ticker,
        "base_external_id": format!("S{i:05}.US"), "quote_external_id": "USD.FOREX", "fractionable": true });
    let listings: Vec<Value> = (0..6_700).map(|i| listing(i, if i % 10 == 0 { format!("S{i:05}N") } else { format!("S{i:05}") }))
        .chain((6_800..6_900).map(|i| listing(i, format!("S{:05}", i - 100)))).collect();
    // wiremock serves from its own background runtime, so building and sending these bodies is not on the measured thread.
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/api/v2/assets")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": assets }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/v2/listings")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": listings }))).mount(&server).await;
    let api = DataApi::live(Config { url: server.uri(), token: "tok".into() });

    // A writer on another thread, as the web is: its longest wait for SQLite's write lock.
    let (stop, longest_wait) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
    let probe = {
        let (stop, longest, file) = (stop.clone(), longest_wait.clone(), d.path().join("production.sqlite3"));
        std::thread::spawn(move || {
            let p = Connection::open(file).unwrap();
            p.busy_timeout(Duration::from_secs(5)).unwrap();
            while !stop.load(Ordering::SeqCst) {
                let t0 = Instant::now();
                p.execute_batch("BEGIN IMMEDIATE; COMMIT;").unwrap();
                longest.fetch_max(t0.elapsed().as_micros() as u64, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let file = c.path().expect("a file database").to_string(); // the key import::holds keeps
    let db = Db::new(c, seed::cipher());
    let (gap, meter) = thread_gap_meter();
    let stocks = reference::run_once(reference::STOCKS, Some(api), Cx { db: db.clone(), clock: &FixedClock(at("2026-10-02T10:05:00Z")) }).await;
    let prune = reference::run_once(reference::PRUNE, None::<DataApi<ScriptedTransport>>, Cx { db, clock: &FixedClock(at("2026-10-02T10:05:00Z")) }).await;
    meter.abort();
    stop.store(true, Ordering::SeqCst);
    probe.join().unwrap();
    assert_eq!((stocks, prune), (Outcome::Done, Outcome::Done));
    let c = reopen(&d);
    assert_eq!(one::<i64>(&c, &format!("SELECT count(*) FROM tickers WHERE exchange_id = {alpaca} AND base LIKE '__stale_%'")), 100, "the delisted names were taken");
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM bot_activity_logs"), 0, "the backlog is gone");
    let holds = import::holds(&file);
    let (gap, wait) = (Duration::from_micros(gap.load(Ordering::Relaxed)), Duration::from_micros(longest_wait.load(Ordering::SeqCst)));
    eprintln!("measured: longest hold per phase {holds:?}; longest runtime-thread gap {gap:?}; longest wait of another writer {wait:?} (CHUNK = {})",
              deltabadger::jobs::CHUNK);
    for phase in ["stock assets", "usd", "exchange assets", "tickers", "sweep", "prune"] {
        let held = holds.get(phase).unwrap_or_else(|| panic!("no {phase} phase ran"));
        assert!(*held < WRITE_LOCK_BOUND, "{phase} held the write lock {held:?}");
    }
    assert!(gap < RUNTIME_THREAD_BOUND, "the runtime thread was held {gap:?}");
    assert!(wait < WRITE_LOCK_BOUND + import::CHUNK_GAP, "another writer waited {wait:?}: more than the unit in hand and one gap");
}

#[tokio::test(flavor = "current_thread")]
async fn empty_assets_and_indices_payloads_refresh_nothing() {
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/assets", 200, json!({ "data": [] })).reply("GET /api/v2/indices", 200, json!({ "data": [] }));
    for (job, now) in [(reference::ASSETS, "2026-10-02T00:20:00Z"), (reference::INDICES, "2026-10-02T10:30:00Z")] {
        let (d, c) = db();
        assert_eq!(run(job, scripted(&t), c, now).await, Outcome::NothingNew, "{job}: Rails writes nothing, and the source keeps its age");
        assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM app_configs"), 0, "{job}: not even an incomplete mark");
    }
}


#[tokio::test(flavor = "current_thread")]
async fn a_reference_write_that_would_stop_a_running_bot_is_rolled_back_and_names_the_bot() -> Result<(), Box<dyn std::error::Error>> {
    let (d, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/assets", 200, json!({ "data": [{ "external_id": "bitcoin", "symbol": "BTC", "name": "Bitcoin",
        "category": "Cryptocurrency", "instrument_type": "tokenized" }] }));
    let out = run(reference::ASSETS, scripted(&t), o.primary, "2026-10-02T00:20:00Z").await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("rolled back") && m.contains(&format!("bot {bot}"))), "{out:?}");
    let c = Connection::open(d.path().join("production.sqlite3"))?;
    assert_eq!(c.query_row("SELECT instrument_type FROM assets WHERE external_id = 'bitcoin'", [], |r| r.get::<_, Option<String>>(0))?, None);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reference_ticker_units_guard_members_but_skip_unrelated_rows_without_caching() -> Result<(), Box<dyn std::error::Error>> {
    let (d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_alpaca_crypto(&o.primary, &s, "ETH", &json!({ "base_decimals": 8, "quote_decimals": 2,
        "price_decimals": 2, "minimum_base_size": "0.001", "minimum_quote_size": "1" }));
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").weights(&[(s.btc, 0.5), (eth, 0.5)]));
    let row = |ext: &str, symbol: &str, decimals: i64| json!({ "base_external_id": ext, "quote_external_id": "usd",
        "ticker": format!("{symbol}/USD"), "base": symbol, "quote": "USD", "base_decimals": decimals, "quote_decimals": 2,
        "price_decimals": 2, "minimum_base_size": "0.001", "minimum_quote_size": "1" });
    let db = Db::new(o.primary, seed::cipher());
    import_tickers(&db, s.exchange_id, vec![row("seed-eth", "ETH", 8)], "2026-10-02T10:15:00Z").await?;
    let out = import_tickers(&db, s.exchange_id, vec![row("seed-eth", "ETH", -1)], "2026-10-02T10:15:00Z").await;
    assert!(matches!(&out, Err(m) if m.contains("rolled back") && m.contains(&format!("bot {bot}"))), "{out:?}");
    let c = Connection::open(d.path().join("production.sqlite3"))?;
    assert_eq!(c.query_row("SELECT base_decimals FROM tickers WHERE base_asset_id = ?1", [eth], |r| r.get::<_, i64>(0))?, 8);
    // Live trading credentials make any full guard refuse. An unrelated unit must still commit: this proves the skip path.
    c.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")])?;
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('other', 'OTHER', 'Other', 'Cryptocurrency', '2026-01-01', '2026-01-01')", [])?;
    import_tickers(&db, s.exchange_id, vec![row("other", "OTHER", 8)], "2026-10-02T10:15:00Z").await?;
    let out = import_tickers(&db, s.exchange_id, vec![row("seed-eth", "ETH", 8)], "2026-10-02T10:15:00Z").await;
    assert!(matches!(&out, Err(m) if m.contains("rolled back")), "a prior accepted unit cannot cache its verdict: {out:?}");
    // Eligibility also reads every venue spelling of a member when checking its split history, even at another quote.
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('eur', 'EUR', 'Euro', 'Fiat', '2026-01-01', '2026-01-01')", [])?;
    let mut other_quote = row("seed-eth", "ETH", 8);
    other_quote["quote_external_id"] = json!("eur");
    other_quote["quote"] = json!("EUR");
    other_quote["ticker"] = json!("ETH/EUR");
    let out = import_tickers(&db, s.exchange_id, vec![other_quote], "2026-10-02T10:15:00Z").await;
    assert!(matches!(&out, Err(m) if m.contains("rolled back")), "a member's other venue spelling is eligibility-relevant: {out:?}");
    assert!(import::shortest_gap(c.path().ok_or("missing database path")?).is_some_and(|g| g >= import::CHUNK_GAP));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn reference_assets_guard_a_stopped_bot_with_an_outstanding_order() -> Result<(), Box<dyn std::error::Error>> {
    let (d, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    seed::insert_tx(&o.primary, &s, bot, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("open-order".into()),
        order_type: 1, amount: Some("0.001"), quote_amount: Some("60"), price: Some("60000"), quote_amount_exec: None,
        amount_exec: None, created_at: "2026-09-01 10:00:00".into() });
    let t = ScriptedTransport::default();
    t.reply("GET /api/v1/assets", 200, json!({ "data": [{ "external_id": "bitcoin", "symbol": "BTC", "name": "Bitcoin",
        "category": "Cryptocurrency", "instrument_type": "tokenized" }] }));
    let out = run(reference::ASSETS, scripted(&t), o.primary, "2026-10-02T00:20:00Z").await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("rolled back") && m.contains(&format!("bot {bot}"))), "{out:?}");
    let c = Connection::open(d.path().join("production.sqlite3"))?;
    assert_eq!(c.query_row("SELECT instrument_type FROM assets WHERE external_id = 'bitcoin'", [], |r| r.get::<_, Option<String>>(0))?, None);
    Ok(())
}
