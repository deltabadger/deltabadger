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
    import_tickers(&Db::new(c, common::seed::cipher()), kraken, vec![row], "2026-10-02T10:15:00Z").await.unwrap();
    let c = reopen(&d);
    assert_eq!(one::<f64>(&c, "SELECT minimum_base_size FROM tickers"), 1.0);
    assert_eq!(one::<i64>(&c, "SELECT minimum_quote_size <= 0.3 FROM tickers"), 1,
               "an order of 0.30 EUR meets the minimum, as in Rails; the float's own digits (0.30000000000000004) would refuse it");
    assert_eq!(one::<i64>(&c, "SELECT maximum_quote_size FROM tickers"), 1_000_000);
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
    let rotation = swap(n, deltabadger::jobs::CHUNK + 1);
    let before = tickers(&reopen(&d));
    let err = db.run(move |c, _| import::plan_tickers(c, kraken, &rotation, None)).await.err().expect("refused");
    assert!(err.contains("larger than one write unit"), "{err}");
    assert_eq!(tickers(&reopen(&d)), before, "nothing written");
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
    let rows = vec![json!({ "external_id": "ethereum", "symbol": "ETH", "name": "ETH", "category": "Cryptocurrency" })];
    import::import_assets(&db, import::plan_assets(&rows, "https://data.example").unwrap(), at("2026-10-02T10:20:00Z")).await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    probe.join().unwrap();
    assert!(import::holds(&file).contains_key("tickers"), "the rolled-back unit was released and recorded: {:?}", import::holds(&file));
    let gap = import::shortest_gap(&file).expect("several units");
    assert!(gap >= import::CHUNK_GAP, "a unit started {gap:?} after the previous one released the lock");
    let wait = std::time::Duration::from_micros(longest.load(std::sync::atomic::Ordering::SeqCst));
    assert!(wait < std::time::Duration::from_millis(100) + import::CHUNK_GAP, "another writer waited {wait:?}");
}
