//! The reference-data jobs (rust/src/jobs/{import,reference}.rs). Row parity with Rails is
//! rust/tests/reference_parity.rs; these pin the Rust-side mechanics parity cannot see.
//! Wall-clock timing tests here are `#[ignore = "timing: nightly"]`, out of the PR gate; the nightly workflow runs them.
//! By hand: `cargo nextest run --profile timing --run-ignored only` (rust/.config/nextest.toml lists them).
mod common;
use chrono::{DateTime, Utc};
use deltabadger::jobs::{import, Db};
use deltabadger::store::{self, Paths};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn db() -> (tempfile::TempDir, Connection) {
    let dir = common::baseline_install();
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
    import::import_indices(&db, import::plan_indices(&[index("L1"), index("Layer 1")]).unwrap(), at("2026-10-02T10:30:00Z"), None).await.unwrap();
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
    // D-TOK-XSTOCKS (plan A15): the catalogue files the SpaceX xStock as `spacex-xstocks`, which Rails' `%-xstock` misses.
    asset(&c, "spacex-xstocks", "SPCXX", "Cryptocurrency");
    asset(&c, "xstockscoin", "XSC", "Cryptocurrency");
    import::mark_tokenized(&db).await.unwrap();
    assert_eq!(one::<String>(&c, "SELECT instrument_type FROM assets WHERE external_id = 'nvda-xstock'"), "tokenized");
    assert_eq!(one::<String>(&c, "SELECT instrument_type FROM assets WHERE external_id = 'spacex-xstocks'"), "tokenized");
    assert_eq!(one::<Option<String>>(&c, "SELECT instrument_type FROM assets WHERE external_id = 'xstockscoin'"), None, "the issuer suffix only");
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
#[ignore = "timing: nightly"]
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
    assert!(wait < WRITE_LOCK_BOUND + import::CHUNK_GAP, "another writer waited {wait:?} at a phase boundary");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "timing: nightly"]
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
    assert!(wait < WRITE_LOCK_BOUND + import::CHUNK_GAP, "another writer waited {wait:?}");
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
    if name == reference::INDICES && api.is_some() {
        deltabadger::engine::provider::bind(&c,&seed::cipher(),&|key| match key {
            "MARKET_DATA_URL"=>Some("http://data-api:3000".into()),"MARKET_DATA_TOKEN"=>Some("tok".into()),_=>None
        }).unwrap();
    }
    reference::run_once(name, api, Cx { db: Db::new(c, seed::cipher()), clock: &FixedClock(at(now)), wakers: Default::default() }).await
}

#[tokio::test(flavor = "current_thread")]
async fn without_the_deltabadger_provider_every_data_api_job_fails_and_prune_still_runs() {
    for spec in reference::specs() {
        let (_d, c) = db();
        let out = run(spec.name, None, c, "2026-10-02T10:30:00Z").await;
        if spec.name == reference::PRUNE { assert_eq!(out, Outcome::Done); }
        // PLAN-A17: with no provider at all the asset refresh returns, as Rails' `return unless MarketData.configured?`.
        // A17-2: so do the venue tickers (SyncAllTickersAndAssetsJob's own guard).
        // A17-4: and the indices (Index::SyncFromCoingeckoJob's own guard).
        else if [reference::ASSETS, reference::TICKERS, reference::INDICES].contains(&spec.name) { assert_eq!(out, Outcome::NothingNew); }
        else { assert!(matches!(&out, Outcome::Failed(m) if m.contains("not deltabadger")), "{}: {out:?}", spec.name); }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_stock_jobs_switch_stops_it_and_a_legacy_row_without_a_backfill_stops_it_after_one_request() {
    let (d, c) = db();
    deltabadger::app_config::set(&c, &seed::cipher(), "stock_sync_enabled", "false", at("2026-10-01T00:00:00Z")).unwrap();
    let t = ScriptedTransport::default(); // any request would panic: nothing is scripted
    assert!(matches!(run(reference::STOCKS, scripted(&t), c, "2026-10-02T10:05:00Z").await, Outcome::Failed(m) if m.contains("switched off")));
    assert!(t.requests().is_empty());
    let c = reopen(&d);
    c.execute("DELETE FROM app_configs", []).unwrap();
    asset(&c, "alpaca_0b5c", "IBIT", "Stock");
    // A payload with no alpaca identifier: the backfill leaves its flag unset, which gates the sync.
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/assets", 200, json!({ "data": [{ "external_id": "IBIT.US", "symbol": "IBIT", "type": "etf" }] }));
    assert!(matches!(run(reference::STOCKS, scripted(&t), c, "2026-10-02T10:05:00Z").await, Outcome::Failed(m) if m.contains("canonical backfill")));
    assert_eq!(t.requests().len(), 1);
    assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM app_configs"), 0, "not even the backfill flag");
    assert_eq!(one::<String>(&reopen(&d), "SELECT external_id FROM assets WHERE symbol = 'IBIT'"), "alpaca_0b5c");
}

/// A floating-point canonical id is refused before any write: its spelling would not be Rails' Float#to_s, so the
/// existing-canonical check could miss ("1e+20" against "1.0e+20").
#[tokio::test(flavor = "current_thread")]
async fn a_float_canonical_id_is_refused_before_any_write() {
    let (d, c) = db();
    asset(&c, "1.0e+20", "BIG", "Stock");
    asset(&c, "alpaca_0b5d", "AAPL", "Stock");
    let before: Vec<(i64, String)> = { let mut s = c.prepare("SELECT id, external_id FROM assets ORDER BY id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect() };
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/assets", 200, json!({ "data": [{ "external_id": 1e20, "symbol": "AAPL", "type": "stock",
        "identifiers": [{ "scheme": "alpaca", "value": "us_equity:AAPL" }] }] }));
    let out = run(reference::STOCKS, scripted(&t), c, "2026-10-02T10:05:00Z").await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("not an integer")), "{out:?}");
    let c = reopen(&d);
    let after: Vec<(i64, String)> = { let mut s = c.prepare("SELECT id, external_id FROM assets ORDER BY id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect() };
    assert_eq!(after, before, "no asset changes");
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM app_configs"), 0, "no flag");
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

// Wall-clock bounds hold for a release build. A debug build on a busy machine held a prune unit for 137 ms, so debug
// gets five times the room. What keeps a unit short is its size (`jobs::CHUNK` rows), not the clock.
const ROOM: u32 = if cfg!(debug_assertions) { 5 } else { 1 };
const RUNTIME_THREAD_BOUND: Duration = Duration::from_millis(250 * ROOM as u64); // the engine ticks on this thread
const WRITE_LOCK_BOUND: Duration = Duration::from_millis(100 * ROOM as u64);     // one write unit

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
#[ignore = "timing: nightly"]
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
    let stocks = reference::run_once(reference::STOCKS, Some(api), Cx { db: db.clone(), clock: &FixedClock(at("2026-10-02T10:05:00Z")), wakers: Default::default() }).await;
    let prune = reference::run_once(reference::PRUNE, None::<DataApi<ScriptedTransport>>, Cx { db, clock: &FixedClock(at("2026-10-02T10:05:00Z")), wakers: Default::default() }).await;
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
        "category": "Tokenized Stock", "instrument_type": "tokenized" }] }));
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
    // A bot with no key makes any full guard refuse. An unrelated unit must still commit: this proves the skip path.
    c.execute("DELETE FROM api_keys", [])?;
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
        "category": "Tokenized Stock", "instrument_type": "tokenized" }] }));
    let out = run(reference::ASSETS, scripted(&t), o.primary, "2026-10-02T00:20:00Z").await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("rolled back") && m.contains(&format!("bot {bot}"))), "{out:?}");
    let c = Connection::open(d.path().join("production.sqlite3"))?;
    assert_eq!(c.query_row("SELECT instrument_type FROM assets WHERE external_id = 'bitcoin'", [], |r| r.get::<_, Option<String>>(0))?, None);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn crypto_freshness_requires_written_tickers_meeting_the_baseline() {
    for (count, usable, baseline) in [(31, 0, 0), (31, 29, 0), (40, 31, 40), (31, 31, 0)] {
        let (d, c) = db();
        exchange(&c, "Exchanges::Alpaca", true);
        let old = at("2026-09-01T10:15:00Z");
        let now = at("2026-10-02T10:15:00Z");
        deltabadger::jobs::state::record_success(&c, reference::ALPACA_CRYPTO, None, old).unwrap();
        deltabadger::jobs::state::mark_incomplete(&c, reference::ALPACA_CRYPTO, None, old).unwrap();
        if baseline > 0 { deltabadger::app_config::set(&c, &seed::cipher(), "alpaca_crypto_listings_last_good_count", &baseline.to_string(), old).unwrap(); }
        let rows: Vec<Value> = (0..count).map(|i| {
            asset(&c, &format!("coin-{i}"), &format!("C{i}"), "Cryptocurrency");
            json!({ "base_asset_id": format!("crypto:coin-{i}"), "symbol": format!("C{i}/USD"), "base_decimals": 8,
                    "quote_decimals": 2, "price_decimals": if i < usable { Some(2) } else { None } })
        }).collect();
        let t = ScriptedTransport::default();
        t.reply("GET /api/v2/listings?venue=alpaca_crypto", 200, json!({ "data": rows }));
        let out = run(reference::ALPACA_CRYPTO, scripted(&t), c, "2026-10-02T10:15:00Z").await;
        let c = reopen(&d);
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM exchange_assets"), count + 1);
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM tickers"), usable);
        if usable == count {
            assert_eq!(out, Outcome::Done);
        } else {
            assert!(matches!(out, Outcome::Failed(_)), "{count}/{usable}/{baseline}: {out:?}");
            let state = deltabadger::jobs::state::read(&c, reference::ALPACA_CRYPTO, None).unwrap();
            assert_eq!(state.last_success_at, Some(old));
            assert_eq!(state.incomplete_since, Some(old));
            assert_ne!(state.last_success_at, Some(now));
        }
    }
}

#[tokio::test(flavor="current_thread")]
async fn index_refresh_attests_only_the_configuration_it_requested() {
    use deltabadger::{app_config,engine::provider,venue::http::{Transport,HttpRequest,HttpResponse,TransportError}};
    struct Change { scripted:ScriptedTransport, c:Connection, change:bool }
    impl Transport for Change {
        async fn send(&self, req:&HttpRequest)->Result<HttpResponse,TransportError> {
            if self.change {
                self.c.execute("UPDATE app_configs SET value='https://changed.invalid' WHERE key='market_data_url'",[]).unwrap();
            }
            self.scripted.send(req).await
        }
    }
    for change in [false,true] {
        let (dir,c)=db();
        let now=at("2026-10-02T10:30:00Z");
        for (key,value) in [("market_data_provider","deltabadger"),("market_data_url","http://data-api:3000"),("market_data_token","tok")] {
            app_config::set(&c,&seed::cipher(),key,value,now).unwrap();
        }
        provider::bind(&c,&seed::cipher(),&|_|None).unwrap();
        let captured=provider::fingerprint(&c).unwrap().unwrap();
        let t=ScriptedTransport::default();
        t.reply("GET /api/v2/indices",200,json!({"data":[{"external_id":"nasdaq-100","name":"Nasdaq 100","top_coins":["AAA.US"]}]}));
        let make=||Change{scripted:t.clone(),c:reopen(&dir),change};
        let api=DataApi::new(Config{url:"http://data-api:3000".into(),token:"tok".into()},make(),make());
        let out=reference::run_once(reference::INDICES,Some(api),Cx{db:Db::new(c,seed::cipher()),clock:&FixedClock(now),wakers:Default::default()}).await;
        let after=reopen(&dir);
        let attested=app_config::get_plain(&after,"rust_job.index_provider").unwrap();
        if change {
            assert!(matches!(out,Outcome::Failed(_)),"{out:?}");
            assert!(attested.is_none(),"an old request cannot vouch for new configuration");
        } else {
            assert_eq!(out,Outcome::Done);
            assert_eq!(attested.as_deref(),Some(captured.as_str()));
        }
    }
}

fn collision_pair(ext: &str, ticker: &str, base: &str) -> Value {
    let mut row = pair(ext, ticker, base);
    row["quote"] = json!("USD");
    row["quote_external_id"] = json!("usd");
    row
}

#[tokio::test(flavor = "current_thread")]
async fn collision_import_orders_and_batch_preserve_both_classes() {
    for reverse in [false, true] {
        let (dir, c) = db();
        let venue = exchange(&c, "Exchanges::Alpaca", true);
        asset(&c, "usd", "USD", "Fiat");
        let coin = asset(&c, "bitcoin", "BTC", "Cryptocurrency");
        let stock = asset(&c, "BTC.US", "BTC", "Stock");
        let mut rows = vec![collision_pair("bitcoin", "BTC/USD", "BTC"), collision_pair("BTC.US", "BTC", "BTC")];
        if reverse { rows.reverse(); }
        let db = Db::new(c, common::seed::cipher());
        for row in &rows { import_tickers(&db, venue, vec![row.clone()], "2026-10-06T10:00:00Z").await.unwrap(); }
        let before = tickers(&reopen(&dir));
        let mut written = import_tickers(&db, venue, rows, "2026-10-06T10:00:00Z").await.unwrap();
        written.sort_unstable();
        assert_eq!(written, vec![coin, stock], "the batch keep-set contains both asset classes");
        assert_eq!(before, tickers(&reopen(&dir)), "neither class is tombstoned or replaced");
        let c = reopen(&dir);
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM tickers WHERE base = 'BTC' AND available = 1 AND trading_enabled = 1"), 2);
        for id in [coin, stock] {
            assert_eq!(one::<i64>(&c, &format!("SELECT count(*) FROM tickers WHERE base_asset_id = {id}")), 1);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn collision_sweeps_and_wrong_class_records_stay_in_the_importing_class() {
    let (dir, c) = db();
    let venue = exchange(&c, "Exchanges::Alpaca", true);
    asset(&c, "usd", "USD", "Fiat");
    asset(&c, "bitcoin", "BTC", "Cryptocurrency");
    asset(&c, "BTC.US", "BTC", "Stock");
    asset(&c, "ethereum", "ETH", "Cryptocurrency");
    asset(&c, "AAPL.US", "AAPL", "Stock");
    let db = Db::new(c, common::seed::cipher());
    let rows = vec![collision_pair("bitcoin", "BTC/USD", "BTC"), collision_pair("BTC.US", "BTC", "BTC"),
                    collision_pair("ethereum", "ETH/USD", "ETH"), collision_pair("AAPL.US", "AAPL", "AAPL")];
    import_tickers(&db, venue, rows.clone(), "2026-10-06T10:00:00Z").await.unwrap();
    for (category, own, other, stale, untouched) in [("Stock", 1, 0, "AAPL", "ETH/USD"), ("Cryptocurrency", 0, 1, "ETH/USD", "BTC")] {
        let mut wrong = rows[other].clone();
        wrong["trading_enabled"] = json!(false);
        let feed = vec![rows[own].clone(), wrong];
        let plan = db.run(move |c, _| import::plan_tickers(c, venue, &feed, Some(category))).await.unwrap();
        import::publish_tickers(&db, venue, plan, at("2026-10-06T10:00:00Z")).await.unwrap();
        let c = reopen(&dir);
        assert_eq!(one::<i64>(&c, &format!("SELECT available FROM tickers WHERE ticker = '{stale}'")), 0);
        assert_eq!(one::<i64>(&c, &format!("SELECT available AND trading_enabled FROM tickers WHERE ticker = '{untouched}'")), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn collision_stock_import_leaves_crypto_tick_placeable_and_failed_contribution_owed() {
    use common::{scripted, seed::{self, BotSpec}};
    use deltabadger::engine::{model, amount, tick::{self, Attempts, TickOutcome}, FixedClock};
    let (dir, o, s) = common::install_alpaca();
    let stock = asset(&o.primary, "BTC.US", "BTC", "Stock");
    o.primary.execute("UPDATE assets SET instrument_type = 'stock' WHERE id = ?1", [stock]).unwrap();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-10-06 10:00:00"));
    let stock_bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-10-06 10:00:00").weights(&[(stock, 1.0)]));
    let db = Db::new(reopen(&dir), seed::cipher());
    let row = json!({ "ticker": "BTC", "base": "BTC", "quote": "USD", "base_external_id": "BTC.US", "quote_external_id": "usd",
                      "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2, "minimum_quote_size": "1" });
    let plan = import::plan_tickers(&o.primary, s.exchange_id, &[row], Some("Stock")).unwrap();
    import::publish_tickers(&db, s.exchange_id, plan, at("2026-10-06T10:00:00Z")).await.unwrap();
    for (bot_id, base, symbol) in [(id, s.btc, "BTC/USD"), (stock_bot, stock, "BTC")] {
        let bot = model::load_bot(&o.primary, bot_id).unwrap();
        let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
        assert_eq!((ticker.base_asset_id, ticker.ticker.as_str()), (base, symbol));
        let external = if base == s.btc { "bitcoin" } else { "BTC.US" };
        let name = format!("collision-{base}");
        seed::insert_index(&o.primary, &name, &[external], &json!({ external: 100.0 }));
        let index_id = seed::index_bot(&o.primary, &s, &name, 1, 0.0, false);
        let index_bot = model::load_bot(&o.primary, index_id).unwrap();
        let transport = scripted::script(json!({ "GET /v2/stocks/BTC/quotes/latest": [scripted::ok(json!({ "quote": { "ap": 100, "bp": 99 } }))] }));
        deltabadger::engine::index::refresh_composition(&o.primary, &scripted::venue(&transport), &index_bot,
            &FixedClock(at("2026-10-06T10:05:00Z")), &tick::PriceCache::default()).await.unwrap().unwrap();
        assert_eq!(one::<i64>(&o.primary, &format!("SELECT asset_id FROM bot_index_assets WHERE bot_id = {index_id} AND in_index = 1")), base);
    }
    let transport = scripted::script(json!({}));
    let out = tick::tick(&o.primary, &scripted::venue(&transport), id, &FixedClock(at("2026-10-06T10:05:00Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(transport.posted_orders()[0]["symbol"], "BTC/USD");
    assert_eq!(transport.posted_orders()[0]["notional"], "60.00");
    let failed = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-10-06 10:00:00"));
    o.primary.execute("UPDATE tickers SET available = 0 WHERE id = ?1", [s.ticker_id]).unwrap();
    let out = tick::tick(&o.primary, &scripted::venue(&transport), failed, &FixedClock(at("2026-10-06T10:06:00Z")), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    let bot = model::load_bot(&o.primary, failed).unwrap();
    assert_eq!(amount::pending_quote_amount(&o.primary, &bot, at("2026-10-06T10:06:00Z").timestamp_micros()).unwrap().to_s_f(), "60.0");
    assert_eq!(transport.posted_orders().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn collision_native_ticker_conflicts_never_steal_another_class() {
    let (dir, c) = db();
    let venue = exchange(&c, "Exchanges::Alpaca", true);
    asset(&c, "usd", "USD", "Fiat");
    asset(&c, "bitcoin", "BTC", "Cryptocurrency");
    asset(&c, "BTC.US", "BTC", "Stock");
    let db = Db::new(c, common::seed::cipher());
    let coin = collision_pair("bitcoin", "BTC/USD", "BTC");
    let bad = collision_pair("BTC.US", "BTC/USD", "BTC");
    import_tickers(&db, venue, vec![coin.clone()], "2026-10-06T10:00:00Z").await.unwrap();
    let before = tickers(&reopen(&dir));
    assert!(import_tickers(&db, venue, vec![bad.clone()], "2026-10-06T10:00:00Z").await.is_err());
    assert_eq!(before, tickers(&reopen(&dir)));
    assert!(import_tickers(&db, venue, vec![coin, bad], "2026-10-06T10:00:00Z").await.is_err());
    assert_eq!(before, tickers(&reopen(&dir)));
}

#[tokio::test(flavor = "current_thread")]
async fn collision_stock_health_count_excludes_crypto_listings() {
    let (d, c) = db();
    exchange(&c, "Exchanges::Alpaca", true);
    asset(&c, "bitcoin", "BTC", "Cryptocurrency");
    asset(&c, "BTC.US", "BTC", "Stock");
    let key = "alpaca_listings_last_good_count";
    deltabadger::app_config::set(&c, &seed::cipher(), key, "1", at("2026-10-05T10:00:00Z")).unwrap();
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/assets", 200, json!({ "data": [] }));
    t.reply("GET /api/v2/listings", 200, json!({ "data": [
        collision_pair("BTC.US", "BTC", "BTC"), collision_pair("bitcoin", "BTC/USD", "BTC")
    ] }));
    let out = run(reference::STOCKS, scripted(&t), c, "2026-10-06T10:00:00Z").await;
    assert_eq!(out, Outcome::Done);
    let c = reopen(&d);
    assert_eq!(deltabadger::app_config::get(&c, &seed::cipher(), key).unwrap().as_deref(), Some("1"));
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM tickers"), 1);
    assert_eq!(one::<String>(&c, "SELECT ticker FROM tickers"), "BTC");
}

#[tokio::test(flavor = "current_thread")]
async fn collision_same_class_display_pairs_are_deduped_and_reconciled() {
    let (d, c) = db();
    let ex = exchange(&c, "Exchanges::Alpaca", true);
    let first = asset(&c, "FIRST.US", "BTC", "Stock");
    asset(&c, "OTHER.US", "BTC", "Stock");
    asset(&c, "EUR.FOREX", "EUR", "Currency");
    let db = Db::new(c, common::seed::cipher());
    let rows = vec![pair("FIRST.US", "FIRST", "BTC"), pair("OTHER.US", "OTHER", "BTC")];
    assert_eq!(import_tickers(&db, ex, rows.clone(), "2026-10-02T10:15:00Z").await.unwrap(), vec![first]);
    assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM tickers WHERE base='BTC'"), 1);
    import_tickers(&db, ex, vec![rows[1].clone()], "2026-10-02T10:16:00Z").await.unwrap();
    let c = reopen(&d);
    assert_eq!(one::<String>(&c, "SELECT ticker FROM tickers WHERE base='BTC'"), "OTHER");
    assert_eq!(one::<i64>(&c, &format!("SELECT available FROM tickers WHERE base_asset_id={first}")), 0);
}

/// Index::SyncFromCoingeckoJob#recheck_index_bots after a pull that did not fail, an empty one included: every index bot that
/// is neither deleted nor archived and has an exchange gets its composition resync marker (engine::index::RESYNC_KEY), in any
/// status. A failed pull marks nothing.
#[tokio::test(flavor = "current_thread")]
async fn an_index_pull_that_did_not_fail_marks_every_live_index_bot_for_a_resync() {
    let bots = |c: &Connection| {
        let ex = exchange(c, "Exchanges::Kraken", true);
        c.execute("INSERT INTO users (id, email, encrypted_password, name, admin, created_at, updated_at) VALUES (1, 'o@example.com', 'x', 'Owner', 1, '2026-01-01', '2026-01-01')", []).unwrap();
        // created, scheduled, stopped, deleted, archived; a created one with no exchange; a basket.
        for (ty, status, exchange) in [("Bots::DcaIndex", 0, Some(ex)), ("Bots::DcaIndex", 1, Some(ex)), ("Bots::DcaIndex", 2, Some(ex)), ("Bots::DcaIndex", 3, Some(ex)),
                                       ("Bots::DcaIndex", 7, Some(ex)), ("Bots::DcaIndex", 0, None), ("Bots::DcaMultiAsset", 1, Some(ex))] {
            c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, created_at, updated_at) VALUES (?1, ?2, ?3, 1, '{}', '{}', '2026-01-01', '2026-01-01')",
                      rusqlite::params![ty, status, exchange]).unwrap();
        }
    };
    let marked = |c: &Connection| -> Vec<i64> {
        let mut s = c.prepare("SELECT status FROM bots WHERE json_extract(transient_data, '$.rust_index_resync.n') = 1 ORDER BY id").unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let index = json!({ "external_id": "layer-1", "source": "coingecko", "name": "Layer 1", "top_coins": ["bitcoin"] });
    for (body, expected) in [(json!({ "data": [index] }), Outcome::Done), (json!({ "data": [] }), Outcome::NothingNew)] {
        let (d, c) = db();
        bots(&c);
        let t = ScriptedTransport::default();
        t.reply("GET /api/v2/indices", 200, body);
        assert_eq!(run(reference::INDICES, scripted(&t), c, "2026-10-02T10:30:00Z").await, expected);
        assert_eq!(marked(&reopen(&d)), vec![0, 1, 2], "{expected:?}: created, scheduled and stopped index bots with an exchange");
    }
    let (d, c) = db();
    bots(&c);
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/indices", 500, json!({ "error": "boom" }));
    assert!(matches!(run(reference::INDICES, scripted(&t), c, "2026-10-02T10:30:00Z").await, Outcome::Transient(_)));
    assert_eq!(marked(&reopen(&d)), Vec::<i64>::new(), "PullFailed: no recheck");
}

// ---- The INDICES job on a CoinGecko install: Top Coins (D-INDEX-TOP-N), then the category pass (PLAN-A17 A17-4) ----

use deltabadger::jobs::coingecko::{self, Api, Clients, CoinGecko as CgClient, Throttle};
use std::rc::Rc;

const CG_KEY: &str = "cg-test-key";
const CG_NOW: &str = "2026-10-02T10:31:00Z";
/// `coins/markets` as Top Coins asks it, and as a category is asked (the client's sorted params).
const TOP_KEY: &str = "GET /api/v3/coins/markets?locale=en&order=market_cap_desc&page=1&per_page=250&precision=full&sparkline=false&vs_currency=usd";
const CATS_KEY: &str = "GET /api/v3/coins/categories?order=market_cap_desc";
fn cat_key(id: &str) -> String { format!("GET /api/v3/coins/markets?category={id}&locale=en&order=market_cap_desc&page=1&per_page=250&precision=full&sparkline=false&vs_currency=usd") }

/// Scripted clients over one transport (the throttle generous: request counts are under test, not timing).
fn cg_clients(t: &ScriptedTransport) -> Clients {
    let t = t.clone();
    Rc::new(move |key: &str| { assert!(key.starts_with("cg-"), "{key}"); Box::new(CgClient::new(key, t.clone()).with_throttle(Arc::new(Throttle::new(10_000)))) as Box<dyn Api> })
}
/// The transport with the demo ping (a 401 picks the demo host) and Top Coins' answer.
fn cg_transport() -> ScriptedTransport {
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, 200, json!([{ "id": "bitcoin", "market_cap": 1e12 }]));
    t
}
/// What a run asked, by name: ping, top-coins, categories, or the category id.
fn asked(t: &ScriptedTransport) -> Vec<String> {
    t.requests().iter().map(|r| match r.path.as_str() {
        "/api/v3/ping" => "ping".to_string(),
        "/api/v3/coins/categories" => "categories".to_string(),
        "/api/v3/coins/markets" => r.query.iter().find(|(k, _)| *k == "category").map_or("top-coins".to_string(), |(_, v)| v.clone()),
        p => p.to_string(),
    }).collect()
}
fn set_coingecko(c: &Connection, key: &str) {
    deltabadger::engine::provider::bind(c, &seed::cipher(), &|_| None).unwrap();
    for (k, v) in [("market_data_provider", "coingecko"), ("coingecko_api_key", key)] { deltabadger::app_config::set(c, &seed::cipher(), k, v, at("2026-01-01T00:00:00Z")).unwrap(); }
}
/// A CoinGecko install over the seeded Kraken (BTC/EUR), with `coins` listed at EUR there.
fn cg_install(coins: &[&str]) -> (tempfile::TempDir, Connection, seed::Seeded) {
    let (d, o, s) = common::install();
    let c = o.primary;
    set_coingecko(&c, CG_KEY);
    for coin in coins { cg_listed(&c, &s, coin); }
    (d, c, s)
}
/// An asset with an available, trading-enabled EUR ticker on the seeded venue.
fn cg_listed(c: &Connection, s: &seed::Seeded, coin: &str) -> i64 {
    let id = asset(c, coin, &coin.to_uppercase(), "Cryptocurrency");
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?3, 'EUR', ?4, ?5, 8, 2, 2, 0.001, 0.01, 1, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![s.exchange_id, format!("{}EUR", coin.to_uppercase()), coin.to_uppercase(), id, s.quote]).unwrap();
    id
}
/// A category index bot on the seeded venue, `status` (1 scheduled, 2 stopped).
fn cg_bot(c: &Connection, s: &seed::Seeded, category: &str, status: i64) -> i64 {
    let settings = json!({ "quote_asset_id": s.quote, "quote_amount": 60.0, "interval": "week", "num_coins": 3, "allocation_flattening": 0.0,
                           "index_type": "category", "index_category_id": category, "hold_all": false });
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
               VALUES ('Bots::DcaIndex', ?1, ?2, ?3, ?4, '{}', '2026-09-01 14:00:00', '2026-01-01', '2026-01-01')",
              rusqlite::params![status, s.exchange_id, s.user_id, settings.to_string()]).unwrap();
    c.last_insert_rowid()
}
/// A `coins/categories` row.
fn category(id: &str, content: &str) -> Value { json!({ "id": id, "name": format!("{id} (X)"), "content": content, "market_cap": 1.5e9 }) }
/// A `coins/markets?category=` answer: the coins with their caps.
fn members(coins: &[(&str, f64)]) -> Value { json!(coins.iter().map(|(id, cap)| json!({ "id": id, "symbol": id, "market_cap": cap })).collect::<Vec<_>>()) }
async fn cg_run(d: &tempfile::TempDir, t: &ScriptedTransport, now: &str) -> Outcome {
    let c = reopen(d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    reference::run_once_with(reference::INDICES, None::<DataApi<ScriptedTransport>>, &cg_clients(t),
                             Cx { db: Db::new(c, seed::cipher()), clock: &FixedClock(at(now)), wakers: Default::default() }).await
}
/// The ranking of the `coingecko` row of `id`: (provider, ranked_at).
fn ranking(c: &Connection, id: &str) -> Option<(String, String)> {
    use rusqlite::OptionalExtension;
    c.query_row("SELECT r.provider, r.ranked_at FROM indices i JOIN rust_index_rankings r ON r.index_id = i.id WHERE i.external_id = ?1 AND i.source = 'coingecko'",
                [id], |r| Ok((r.get(0)?, r.get(1)?))).optional().unwrap()
}
fn row_text(c: &Connection, id: &str, col: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    c.query_row(&format!("SELECT {col} FROM indices WHERE external_id = ?1 AND source = 'coingecko'"), [id], |r| r.get(0)).optional().unwrap().flatten()
}
fn top_of(c: &Connection, id: &str) -> Vec<String> { serde_json::from_str(&row_text(c, id, "top_coins").unwrap()).unwrap() }
/// An existing `coingecko` row as Rails' pass leaves it (10 display coins, no weights), unattested.
fn rails_category_row(c: &Connection, id: &str, top: &[&str]) -> i64 {
    c.execute("INSERT INTO indices (external_id, source, name, description, top_coins, top_coins_by_exchange, available_exchanges, weights, weight, created_at, updated_at) \
               VALUES (?1, 'coingecko', ?1, 'Rails', ?2, '{}', '{}', '{}', 0, '2026-09-01 10:30:00', '2026-09-01 10:30:00')",
              rusqlite::params![id, json!(top).to_string()]).unwrap();
    c.last_insert_rowid()
}

/// Exercise the production job outcome and the real scheduler, including its 15-minute delay.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn direct_index_503_is_retried_then_succeeds_on_attempt_two() {
    use deltabadger::jobs::{Job, JobFuture, Scheduler, Spec, Wake, Retry};
    struct Direct(Clients);
    impl Job for Direct {
        fn spec(&self) -> Spec { reference::specs().into_iter().find(|s| s.name == reference::INDICES).unwrap() }
        fn run<'a>(&'a self, cx: Cx<'a>, _: Vec<Wake>) -> JobFuture<'a> { Box::pin(async move { reference::coingecko_indices(&self.0, &cx).await }) }
    }
    let (d,c) = db();
    let now = at("2026-10-02T10:30:00Z");
    seed::insert_index(&c,"top-coins",&["previous"],&json!({})); // before the provider: unranked
    set_coingecko(&c, CG_KEY);
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, 503, json!({})).reply(TOP_KEY, 200, json!([{ "id":"bitcoin", "market_cap":1000000 }]))
        .reply(CATS_KEY, 200, json!([]));
    let calls = || t.requests().iter().filter(|r| r.path == "/api/v3/coins/markets").count();
    let job = Direct(cg_clients(&t));
    assert_eq!(job.spec().retry, Retry::Fixed { wait_secs: 900, attempts: 4 });
    let scheduler = Scheduler::new(c, seed::cipher(), vec![Box::new(job)], None);
    struct Clock { start: DateTime<Utc>, origin: tokio::time::Instant }
    impl deltabadger::engine::Clock for Clock {
        fn now(&self) -> DateTime<Utc> { self.start + chrono::Duration::from_std(self.origin.elapsed()).unwrap() }
    }
    let clock = Clock { start: now, origin: tokio::time::Instant::now() };
    let (stop, rx) = tokio::sync::watch::channel(false);
    let (result, ()) = tokio::join!(scheduler.run(rx, &clock), async {
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        assert_eq!(calls(), 1, "no retry before 15 minutes");
        assert_eq!(one::<String>(&reopen(&d), "SELECT top_coins FROM indices WHERE external_id='top-coins'"), "[\"previous\"]");
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        stop.send(true).unwrap();
    });
    result.unwrap();
    assert_eq!(calls(), 2, "503 must schedule the second attempt");
    assert_eq!(one::<String>(&reopen(&d), "SELECT top_coins FROM indices WHERE external_id='top-coins'"), "[\"bitcoin\"]");
}

#[tokio::test(flavor = "current_thread")]
async fn direct_index_refresh_preserves_bundled_stablecoin_classification() {
    let (d,c) = db();
    let now = at("2026-10-02T10:30:00Z");
    set_coingecko(&c, CG_KEY);
    let catalogue: Value = serde_json::from_str(include_str!("../../db/seed_data/assets.json")).unwrap();
    let ids = ["usds", "ethena-usde", "usd1-wlfi"];
    let rows: Vec<_> = catalogue["data"].as_array().unwrap().iter().filter(|a| ids.contains(&a["external_id"].as_str().unwrap())).cloned().collect();
    let db = Db::new(c,seed::cipher());
    import::import_assets(&db, import::plan_assets(&rows, "").unwrap(), now).await.unwrap();
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, 200, json!(ids.map(|id| json!({"id":id,"market_cap":999})))).reply(CATS_KEY, 200, json!([]));
    assert_eq!(reference::coingecko_indices(&cg_clients(&t),&Cx { db, clock:&FixedClock(now), wakers:Default::default() }).await, Outcome::Done);
    let c = reopen(&d);
    for id in ids {
        let (category, cap): (String, i64) = c.query_row("SELECT category,market_cap FROM assets WHERE external_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(category, "Cryptocurrency", "sync routing stays intact");
        assert_eq!(cap,999);
        assert!(deltabadger::engine::index::stablecoin(Some(&category),id), "{id}: daily refresh must preserve the corrected asset class");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn direct_index_fetch_errors_use_the_data_api_retry_classification() {
    let (d,c) = db();
    let now = at("2026-10-02T10:30:00Z");
    seed::insert_index(&c,"top-coins",&["previous"],&json!({})); // before the provider: unranked
    set_coingecko(&c, CG_KEY);
    let db = Db::new(c,seed::cipher());
    for status in [Some(500),Some(502),Some(503),None] {
        let t = ScriptedTransport::default();
        t.reply("GET /api/v3/ping", 401, json!({})).reply(CATS_KEY, 200, json!([]));
        match status { Some(s) => { t.reply(TOP_KEY, s, json!({})); } None => { t.network(TOP_KEY, "pre_send", "down"); } }
        let out = reference::coingecko_indices(&cg_clients(&t), &Cx { db:db.clone(), clock:&FixedClock(now), wakers:Default::default() }).await;
        assert!(matches!(out,Outcome::Transient(_)), "{status:?}: {out:?}");
        assert_eq!(one::<String>(&reopen(&d), "SELECT top_coins FROM indices WHERE external_id='top-coins'"), "[\"previous\"]");
    }
    // PLAN-A17 item 11: a 429 (the plan ping's or the ranking's) stays a rate limit; the same Retry retries it. Nothing more
    // is asked (D-CG-INDEX-429-STOPS): the categories are not scripted.
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, 429, json!({}));
    let out = reference::coingecko_indices(&cg_clients(&t), &Cx { db:db.clone(), clock:&FixedClock(now), wakers:Default::default() }).await;
    assert!(matches!(out,Outcome::RateLimited(_)), "{out:?}");
    assert_eq!(asked(&t), ["ping", "top-coins"]);
    assert_eq!(one::<String>(&reopen(&d), "SELECT top_coins FROM indices WHERE external_id='top-coins'"), "[\"previous\"]");
}

#[tokio::test(flavor = "current_thread")]
async fn self_hosted_top_coins_refreshes_and_expires_without_forcing_trades() {
    use deltabadger::engine::{index,model,provider,staleness};
    let (d,o,s) = common::install();
    let c = o.primary;
    seed::insert_index(&c,"top-coins",&["old"],&json!({}));
    let id = seed::index_bot(&c,&s,"top-coins",20,0.0,false);
    let now = at("2026-10-02T10:30:00Z");
    for (k,v) in [("market_data_provider","coingecko"),("market_data_url",""),("market_data_token",""),("coingecko_api_key",CG_KEY)] {
        deltabadger::app_config::set(&c,&seed::cipher(),k,v,now).unwrap();
    }
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([]));
    let db = Db::new(c,seed::cipher());
    // The same daily job used for hosted indices dispatches to the direct provider.
    let spec = reference::specs().into_iter().find(|s|s.name==reference::INDICES).unwrap();
    assert!(spec.schedule.is_some());
    assert_eq!(reference::coingecko_indices(&cg_clients(&t),&Cx{db:db.clone(),clock:&FixedClock(now),wakers:Default::default()}).await,Outcome::Done);
    let c = reopen(&d);
    assert_eq!(one::<String>(&c,"SELECT top_coins FROM indices WHERE external_id='top-coins'"),"[\"bitcoin\"]");
    assert_eq!(one::<i64>(&c,"SELECT count(*) FROM transactions"),0);
    assert!(index::resync_marked(&c).unwrap().iter().any(|(bot,_)|*bot==id));
    provider::bind(&c,&seed::cipher(),&|_|None).unwrap();
    let bot = model::load_bot(&c,id).unwrap();
    assert!(deltabadger::engine::eligibility::bot_reasons(&c,&bot).unwrap().is_empty(),"the refreshed direct-provider bot is admitted");
    assert!(provider::stale(&c,&bot).unwrap().is_none());
    assert_eq!(index::ranked_row(&c,"top-coins").unwrap().map(|r|r.provider).as_deref(),Some("coingecko"),"the writer attests its own ranking");
    assert!(staleness::stale(&c,&bot,now).unwrap().is_none());
    seed::insert_index(&c,"unrelated-category",&["bitcoin"],&json!({}));
    assert!(staleness::stale(&c,&bot,now+chrono::Duration::hours(50)).unwrap().is_some(),"another index cannot hide an old Top Coins ranking");
    assert!(staleness::report(&c,now+chrono::Duration::hours(50)).unwrap().iter().any(|line|line.contains("Top Coins")&&line.contains("STALE")));
    // The next slot asks again; an empty answer keeps the previous ranking.
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, 200, json!([])).reply(CATS_KEY, 200, json!([]));
    let next = now + chrono::Duration::days(1);
    assert!(matches!(reference::coingecko_indices(&cg_clients(&t),&Cx{db,clock:&FixedClock(next),wakers:Default::default()}).await,Outcome::Failed(_)));
    assert_eq!(one::<String>(&c,"SELECT top_coins FROM indices WHERE external_id='top-coins'"),"[\"bitcoin\"]");
}

/// A client whose Top Coins request switches the install's key mid-run.
struct Changing(std::path::PathBuf, Box<dyn Api>);
impl Api for Changing {
    fn markets<'a>(&'a self, ids: Option<&'a [String]>, category: Option<&'a str>, per_page: usize, page: usize) -> coingecko::Fut<'a> {
        let c = Connection::open(&self.0).unwrap();
        deltabadger::app_config::set(&c,&seed::cipher(),"coingecko_api_key","replacement",at("2026-10-02T10:30:00Z")).unwrap();
        self.1.markets(ids, category, per_page, page)
    }
    fn coin<'a>(&'a self, id: &'a str) -> coingecko::Fut<'a> { self.1.coin(id) }
    fn exchange_tickers<'a>(&'a self, id: &'a str, page: usize) -> coingecko::Fut<'a> { self.1.exchange_tickers(id, page) }
    fn categories(&self) -> coingecko::Fut<'_> { self.1.categories() }
}

#[tokio::test(flavor = "current_thread")]
async fn direct_index_refresh_rejects_a_provider_change_during_the_request() {
    let (d,c)=db();
    seed::insert_index(&c,"top-coins",&["previous"],&json!({})); // before the provider: unranked
    let now=at("2026-10-02T10:30:00Z");
    set_coingecko(&c, CG_KEY);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("payments", "P")]));
    let (path, inner) = (d.path().join("production.sqlite3"), cg_clients(&t));
    let changing: Clients = Rc::new(move |k: &str| Box::new(Changing(path.clone(), inner(k))) as Box<dyn Api>);
    let out=reference::coingecko_indices(&changing,&Cx{db:Db::new(c,seed::cipher()),clock:&FixedClock(now),wakers:Default::default()}).await;
    assert!(matches!(&out,Outcome::Failed(message) if message.contains("configuration changed")), "{out:?}");
    assert_eq!(one::<String>(&reopen(&d),"SELECT top_coins FROM indices WHERE external_id='top-coins'"),"[\"previous\"]");
    assert_eq!(one::<i64>(&reopen(&d),"SELECT count(*) FROM indices WHERE external_id='payments'"),0, "the category pass's units are fenced too");
}

#[test]
fn direct_index_provider_uses_the_environment_key_only_without_a_database_override() {
    use deltabadger::{app_config,engine::provider};
    let (_d,c)=db();
    let now=at("2026-10-02T10:30:00Z");
    app_config::set(&c,&seed::cipher(),"market_data_provider","coingecko",now).unwrap();
    provider::bind(&c,&seed::cipher(),&|key|(key=="COINGECKO_API_KEY").then(||"environment-key".into())).unwrap();
    assert_eq!(provider::coingecko_key(&c).unwrap().as_deref(),Some("environment-key"));
    let fingerprint=provider::fingerprint(&c).unwrap();
    app_config::set(&c,&seed::cipher(),"coingecko_api_key","database-key",now).unwrap();
    assert_eq!(provider::coingecko_key(&c).unwrap().as_deref(),Some("database-key"));
    assert_ne!(provider::fingerprint(&c).unwrap(),fingerprint);
    app_config::set(&c,&seed::cipher(),"coingecko_api_key","",now).unwrap();
    assert_eq!(provider::coingecko_key(&c).unwrap(),None,"explicitly clearing the key does not restore the environment key");
}

// ---- A17-4: the category pass (jobs::categories). On 2026-10-02 (JDN mod 14 = 4) the slice is MD5(id) mod 14 = 4:
// payments, music, pump-fun, launchpad, frog-themed-coins; layer-1, dex, nft are 2026-10-01's (3); oracle, storage, meme-token
// are never due here. ----

fn twelve() -> Vec<String> { (b'a'..=b'l').map(|ch| format!("coin-{}", ch as char)).collect() }

/// Rails' rules and the named divergences on one run: the slice asked in feed order, then the followed (sorted); Top Coins
/// first. D-CG-CATEGORY-STORED (every valid member and its cap stored), D-CG-FOLLOWED-KEPT (a followed category kept
/// through the sweep, written with < 3 valid members), the sweep's cascade (a swept category's ranking goes with it).
#[tokio::test(flavor = "current_thread")]
async fn the_category_pass_ranks_the_slice_sweeps_and_keeps_what_is_followed() {
    let coins = twelve();
    let (d, c, s) = cg_install(&coins.iter().map(String::as_str).collect::<Vec<_>>());
    asset(&c, "unlisted-1", "U1", "Cryptocurrency");
    asset(&c, "unlisted-2", "U2", "Cryptocurrency");
    asset(&c, "unlisted-3", "U3", "Cryptocurrency");
    let fp = deltabadger::engine::provider::fingerprint(&c).unwrap().unwrap();
    // A row this engine ranked, now gone from the feed: swept, its ranking cascaded.
    let old = rails_category_row(&c, "old-category", &["coin-a"]);
    deltabadger::engine::index::record_ranking(&c, old, "coingecko", &fp, at("2026-09-30T10:31:00Z")).unwrap();
    rails_category_row(&c, "launchpad", &["coin-a"]); // listed without a description: swept
    rails_category_row(&c, "oracle", &["coin-a"]);    // followed, gone from the feed: retired (r18), its row kept, not asked
    rails_category_row(&c, "meme-token", &["coin-b"]); // eligible, not due: untouched
    cg_bot(&c, &s, "oracle", 2);
    cg_bot(&c, &s, "frog-themed-coins", 1);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("stablecoins", "S"), category("payments", "Payments"), category("music", "M"), category("pump-fun", "P"),
                                   category("launchpad", ""), category("frog-themed-coins", "F"), category("meme-token", "Memes")]))
        .reply(&cat_key("payments"), 200, members(&coins.iter().enumerate().map(|(i, id)| (id.as_str(), 1e9 - i as f64)).chain([("no-asset", 5.0), ("coin-a", 1.0)]).collect::<Vec<_>>()))
        .reply(&cat_key("music"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0), ("no-asset", 1.0)]))
        .reply(&cat_key("pump-fun"), 200, members(&[("unlisted-1", 3.0), ("unlisted-2", 2.0), ("unlisted-3", 1.0)]))
        .reply(&cat_key("frog-themed-coins"), 200, members(&[("coin-c", 7.0), ("no-asset", 1.0)]))
        .reply(&cat_key("oracle"), 200, members(&[("coin-d", 5.0), ("coin-e", 0.0), ("coin-f", 4.0)]));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "payments", "music", "pump-fun", "frog-themed-coins"]);
    let c = reopen(&d);
    let ids = |sql: &str| -> Vec<String> { c.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
    assert_eq!(ids("SELECT external_id FROM indices WHERE source = 'coingecko' ORDER BY external_id"), ["frog-themed-coins", "meme-token", "oracle", "payments"],
               "old-category and launchpad swept; music disqualified; pump-fun skipped (no venue, no row)");
    assert_eq!(one::<i64>(&c, &format!("SELECT count(*) FROM rust_index_rankings WHERE index_id = {old}")), 0, "the sweep cascades its ranking");
    assert_eq!(top_of(&c, "payments"), coins, "D-CG-CATEGORY-STORED: every valid member, deduped, answer order (Rails: the first 10)");
    let weights: Value = serde_json::from_str(&row_text(&c, "payments", "weights").unwrap()).unwrap();
    assert_eq!((weights["coin-a"].as_f64(), weights["coin-l"].as_f64(), weights.as_object().unwrap().len()), (Some(1e9), Some(1e9 - 11.0), 12), "the first answer's cap");
    assert_eq!(row_text(&c, "payments", "name").as_deref(), Some("payments"), "brackets stripped");
    assert_eq!(one::<i64>(&c, "SELECT weight FROM indices WHERE external_id = 'payments'"), 0);
    let by_ex: Value = serde_json::from_str(&row_text(&c, "payments", "top_coins_by_exchange").unwrap()).unwrap();
    assert_eq!(by_ex["Exchanges::Kraken"].as_array().unwrap().len(), 10, "per venue: Rails' first 10");
    assert_eq!(row_text(&c, "payments", "available_exchanges").as_deref(), Some(r#"{"Exchanges::Kraken":10}"#));
    assert_eq!(ranking(&c, "payments").map(|r| r.0).as_deref(), Some("coingecko"));
    assert_eq!(top_of(&c, "frog-themed-coins"), ["coin-c"], "D-CG-FOLLOWED-KEPT: written with one valid member");
    assert_eq!(row_text(&c, "frog-themed-coins", "available_exchanges").as_deref(), Some("{}"), "and no venue with 3");
    assert_eq!(top_of(&c, "oracle"), ["coin-a"], "retired: its row as it was");
    assert_eq!(one::<String>(&c, "SELECT provider FROM rust_index_retired WHERE external_id = 'oracle'"), "coingecko");
    assert_eq!(top_of(&c, "meme-token"), ["coin-b"], "not due, not followed: not asked");
    assert_eq!(ranking(&c, "meme-token"), None);
    assert_eq!(deltabadger::app_config::get_plain(&c, deltabadger::jobs::categories::SLICE_KEY).unwrap(),
               Some(format!("{fp}@{}", deltabadger::engine::index::ranked_at_text(at("2026-10-02T10:30:00Z")))), "the slice completed for this slot");
    assert!(deltabadger::engine::index::resync_marked(&c).unwrap().len() == 2, "every index bot marked (recheck_index_bots)");
}

/// D-CG-RANK-ONCE-PER-SLOT: a second run in the slot asks the list only; a key change asks everything due again.
#[tokio::test(flavor = "current_thread")]
async fn a_second_run_in_the_slot_asks_the_list_only_and_a_key_change_asks_everything_again() {
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    cg_bot(&c, &s, "oracle", 1);
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    let script = |t: &ScriptedTransport| { t.reply(CATS_KEY, 200, json!([category("payments", "P"), category("oracle", "O")]))
        .reply(&cat_key("payments"), 200, three.clone()).reply(&cat_key("oracle"), 200, three.clone()); };
    let t = cg_transport();
    script(&t);
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "payments", "oracle"]);
    let t = cg_transport();
    script(&t);
    assert_eq!(cg_run(&d, &t, "2026-10-02T12:00:00Z").await, Outcome::NothingNew);
    assert_eq!(asked(&t), ["ping", "categories"], "Top Coins, the slice and the followed are current");
    deltabadger::app_config::set(&reopen(&d), &seed::cipher(), "coingecko_api_key", "cg-other-key", at("2026-10-02T12:30:00Z")).unwrap(); // a new fingerprint
    let t = cg_transport();
    script(&t);
    assert_eq!(cg_run(&d, &t, "2026-10-02T13:00:00Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "payments", "oracle"]);
}

/// D-CG-INDEX-429-STOPS: a 429 on the list, on Top Coins or mid-slice asks nothing more, keeps every committed unit, and ends
/// RateLimited; the retry asks only what is not current.
#[tokio::test(flavor = "current_thread")]
async fn a_429_anywhere_stops_the_run_and_the_retry_asks_only_the_rest() {
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    let feed = json!([category("payments", "P"), category("music", "M"), category("pump-fun", "F")]);
    // On the list: Top Coins committed.
    let (d, _c, _s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let t = cg_transport();
    t.reply(CATS_KEY, 429, json!({}));
    assert!(matches!(cg_run(&d, &t, CG_NOW).await, Outcome::RateLimited(_)));
    assert_eq!(asked(&t), ["ping", "top-coins", "categories"]);
    assert_eq!(index_ranked(&reopen(&d), "top-coins"), Some("coingecko".into()));
    // On Top Coins: the categories are not asked.
    let (d, _c, _s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let t = ScriptedTransport::default();
    t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, 429, json!({}));
    assert!(matches!(cg_run(&d, &t, CG_NOW).await, Outcome::RateLimited(_)));
    assert_eq!(asked(&t), ["ping", "top-coins"]);
    // Mid-slice: payments committed, music's 429 stops it, pump-fun not asked; the retry asks music and pump-fun only.
    let (d, _c, _s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed.clone()).reply(&cat_key("payments"), 200, three.clone()).reply(&cat_key("music"), 429, json!({}));
    assert!(matches!(cg_run(&d, &t, CG_NOW).await, Outcome::RateLimited(_)));
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "payments", "music"]);
    assert_eq!(ranking(&reopen(&d), "payments").map(|r| r.0).as_deref(), Some("coingecko"), "the committed unit is kept");
    assert_eq!(deltabadger::app_config::get_plain(&reopen(&d), deltabadger::jobs::categories::SLICE_KEY).unwrap(), None, "the slice is not complete");
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed).reply(&cat_key("music"), 200, three.clone()).reply(&cat_key("pump-fun"), 200, three);
    assert_eq!(cg_run(&d, &t, "2026-10-02T10:46:00Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "categories", "music", "pump-fun"], "the retry: Top Coins and payments are current");
    assert!(deltabadger::app_config::get_plain(&reopen(&d), deltabadger::jobs::categories::SLICE_KEY).unwrap().is_some());
}

fn index_ranked(c: &Connection, id: &str) -> Option<String> {
    deltabadger::engine::provider::bind(c, &seed::cipher(), &|_| None).unwrap();
    deltabadger::engine::index::ranked_row(c, id).unwrap().map(|r| r.provider)
}

/// Top Coins first, never blocking the categories (r4 P2): a 500 is Transient with the categories committed, and the retry asks
/// Top Coins only; a malformed answer is Failed with the categories committed.
#[tokio::test(flavor = "current_thread")]
async fn a_top_coins_failure_never_blocks_the_categories() {
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    for (status, body) in [(500, json!({ "error": "boom" })), (200, json!([]))] {
        let (d, _c, _s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
        let t = ScriptedTransport::default();
        t.reply("GET /api/v3/ping", 401, json!({})).reply(TOP_KEY, status, body.clone())
            .reply(CATS_KEY, 200, json!([category("payments", "P")])).reply(&cat_key("payments"), 200, three.clone());
        let out = cg_run(&d, &t, CG_NOW).await;
        if status == 500 { assert!(matches!(out, Outcome::Transient(_)), "{out:?}"); } else { assert!(matches!(out, Outcome::Failed(_)), "{out:?}"); }
        assert_eq!(asked(&t), ["ping", "top-coins", "categories", "payments"]);
        assert_eq!(ranking(&reopen(&d), "payments").map(|r| r.0).as_deref(), Some("coingecko"), "{status}: the categories are committed");
        if status == 500 {
            let t = cg_transport();
            t.reply(CATS_KEY, 200, json!([category("payments", "P")]));
            assert_eq!(cg_run(&d, &t, "2026-10-02T10:46:00Z").await, Outcome::Done);
            assert_eq!(asked(&t), ["ping", "top-coins", "categories"], "the retry asks Top Coins only");
        }
    }
}

/// The slice is the slot's UTC date: a run at 01:00 belongs to the previous day's 10:30 slot; and a wake after a successful
/// slot asks no slice category.
#[tokio::test(flavor = "current_thread")]
async fn the_slice_crosses_midnight_with_its_slot_and_a_later_wake_asks_no_slice() {
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    let feed = json!([category("payments", "P"), category("layer-1", "L1"), category("storage", "S")]);
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed.clone()).reply(&cat_key("layer-1"), 200, three.clone());
    assert_eq!(cg_run(&d, &t, "2026-10-02T01:00:00Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "layer-1"], "01:00 on the 2nd is the 1st's slot (JDN mod 14 = 3)");
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed.clone()).reply(&cat_key("payments"), 200, three.clone());
    assert_eq!(cg_run(&d, &t, "2026-10-03T01:00:00Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "payments"], "01:00 on the 3rd is the 2nd's slot");
    // Followed after the slot succeeded: the woken run asks the list and that category only.
    cg_bot(&c, &s, "storage", 2);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed).reply(&cat_key("storage"), 200, three);
    assert_eq!(cg_run(&d, &t, "2026-10-03T02:00:00Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "categories", "storage"]);
}

/// The slice's completion is its own row per (fingerprint, slot), whatever the Outcome: a non-followed fetch that fails leaves
/// it open (logged, Rails' skip); the retry that writes nothing (a disqualified answer) completes it.
#[tokio::test(flavor = "current_thread")]
async fn the_slice_completes_per_fingerprint_and_slot_even_on_a_run_that_writes_nothing() {
    use deltabadger::jobs::categories::SLICE_KEY;
    let (d, _c, _s) = cg_install(&["coin-a"]);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("payments", "P")])).reply(&cat_key("payments"), 500, json!({}));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done, "Top Coins was written; a non-followed failure is Rails' logged skip");
    assert_eq!(deltabadger::app_config::get_plain(&reopen(&d), SLICE_KEY).unwrap(), None);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("payments", "P")])).reply(&cat_key("payments"), 200, members(&[("coin-a", 1.0)]));
    assert_eq!(cg_run(&d, &t, "2026-10-02T10:46:00Z").await, Outcome::Done, "the slice completed");
    assert_eq!(asked(&t), ["ping", "categories", "payments"]);
    assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM indices WHERE external_id = 'payments'"), 0, "disqualified: nothing written");
    assert!(deltabadger::app_config::get_plain(&reopen(&d), SLICE_KEY).unwrap().is_some());
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("payments", "P")]));
    assert_eq!(cg_run(&d, &t, "2026-10-02T11:00:00Z").await, Outcome::NothingNew);
    assert_eq!(asked(&t), ["ping", "categories"], "a complete slot asks no slice category again, the disqualified one included");
}

/// The wake (contract item 3): the INDICES job, and only it, wants RankingMissing; a held bot's category is followed, so the
/// woken run asks the list and that category only, and the bot's ranking is then usable.
#[tokio::test(flavor = "current_thread")]
async fn the_ranking_wake_runs_indices_and_asks_the_list_and_the_missing_category_only() {
    use deltabadger::engine::events::EngineEvent;
    let wake = EngineEvent::RankingMissing { external_id: "storage".into() };
    let jobs = reference::jobs(None::<DataApi<ScriptedTransport>>);
    let wanting: Vec<&str> = jobs.iter().filter(|j| j.wants(&wake)).map(|j| j.spec().name).collect();
    assert_eq!(wanting, [reference::INDICES]);
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("storage", "S")]));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    let bot = cg_bot(&c, &s, "storage", 1);
    let held = deltabadger::engine::model::load_bot(&c, bot).unwrap();
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    assert!(deltabadger::engine::index::ranking_wanted(&c, &held, at("2026-10-02T11:00:00Z")).unwrap());
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("storage", "S")])).reply(&cat_key("storage"), 200, three);
    assert_eq!(cg_run(&d, &t, "2026-10-02T11:00:01Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "categories", "storage"], "ping + list + the category: 3 requests");
    let c = reopen(&d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    assert!(!deltabadger::engine::index::ranking_wanted(&c, &held, at("2026-10-02T11:00:02Z")).unwrap());
    assert!(deltabadger::engine::staleness::ranking_stale(&c, &held, at("2026-10-02T11:00:02Z")).unwrap().is_none());
}

/// D-CG-INVALID-CAP-HELD: a negative cap refuses the whole answer, as a failed fetch (previous ranking kept, a followed one
/// Transient and retried); D-CG-INDEX-PULL-RETRIED: a failed list is Transient too, with no sweep.
#[tokio::test(flavor = "current_thread")]
async fn a_negative_cap_refuses_the_answer_and_a_failed_list_is_retried() {
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    cg_bot(&c, &s, "oracle", 1);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("oracle", "O")])).reply(&cat_key("oracle"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    let before = (ranking(&reopen(&d), "oracle"), top_of(&reopen(&d), "oracle"));
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("oracle", "O")])).reply(&cat_key("oracle"), 200, members(&[("coin-a", 100.0), ("coin-b", -50.0)]));
    let out = cg_run(&d, &t, "2026-10-03T10:31:00Z").await;
    assert!(matches!(&out, Outcome::Transient(m) if m.contains("negative")), "{out:?}");
    assert_eq!((ranking(&reopen(&d), "oracle"), top_of(&reopen(&d), "oracle")), before, "the previous ranking kept");
    let t = cg_transport();
    t.reply(CATS_KEY, 500, json!({})).reply(&cat_key("oracle"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]));
    let out = cg_run(&d, &t, "2026-10-04T10:31:00Z").await;
    assert!(matches!(&out, Outcome::Transient(m) if m.contains("categories")), "{out:?}");
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "oracle"], "the followed category is still asked, from its row");
    assert_eq!(one::<i64>(&reopen(&d), "SELECT count(*) FROM indices WHERE external_id = 'oracle'"), 1, "no sweep");
}

/// D-CG-AVAILABILITY-FROM-RANKING: a 72-hour-old, unfollowed, not-due category whose venue lists its members after the fetch
/// is available after the next pass with no fetch of it, ranked_at unchanged. A failed fetch of a followed category with an
/// availability change moves updated_at only.
#[tokio::test(flavor = "current_thread")]
async fn availability_is_recomputed_from_the_stored_ranking_without_a_fetch() {
    let (d, c, s) = cg_install(&[]);
    for id in ["coin-a", "coin-b", "coin-c"] { asset(&c, id, &id.to_uppercase(), "Cryptocurrency"); }
    cg_bot(&c, &s, "oracle", 1);
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("storage", "S"), category("oracle", "O")])).reply(&cat_key("oracle"), 200, three.clone());
    assert_eq!(cg_run(&d, &t, "2026-09-29T10:31:00Z").await, Outcome::Done); // the 29th's slice (bucket 1) holds neither
    // storage, ranked 72 h ago by this engine with no venue listing its members.
    let c = reopen(&d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    let fp = deltabadger::engine::provider::fingerprint(&c).unwrap().unwrap();
    let storage = rails_category_row(&c, "storage", &["coin-a", "coin-b", "coin-c"]);
    deltabadger::engine::index::record_ranking(&c, storage, "coingecko", &fp, at("2026-09-29T10:31:00Z")).unwrap();
    let ranked_before = ranking(&c, "storage");
    let oracle_before = (ranking(&c, "oracle"), row_text(&c, "oracle", "updated_at"));
    assert_eq!(row_text(&c, "oracle", "available_exchanges").as_deref(), Some("{}"));
    for id in ["coin-a", "coin-b", "coin-c"] {
        let a: i64 = one(&c, &format!("SELECT id FROM assets WHERE external_id = '{id}'"));
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?2, 'EUR', ?3, ?4, 8, 2, 2, 0.001, 0.01, 1, 1, '2026-01-01', '2026-01-01')",
                  rusqlite::params![s.exchange_id, format!("{id}EUR"), a, s.quote]).unwrap();
    }
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("storage", "S"), category("oracle", "O")])).reply(&cat_key("oracle"), 500, json!({}));
    assert!(matches!(cg_run(&d, &t, "2026-10-02T10:31:00Z").await, Outcome::Transient(_)));
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "oracle"], "storage is not fetched");
    let c = reopen(&d);
    assert_eq!(row_text(&c, "storage", "available_exchanges").as_deref(), Some(r#"{"Exchanges::Kraken":3}"#));
    assert_eq!(row_text(&c, "storage", "top_coins_by_exchange").as_deref(), Some(r#"{"Exchanges::Kraken":["coin-a","coin-b","coin-c"]}"#),
               "re-derived from the stored ranking (Rails re-counts its saved per-venue lists)");
    assert_eq!(ranking(&c, "storage"), ranked_before, "ranked_at untouched");
    assert_eq!(ranking(&c, "oracle"), oracle_before.0, "the failed fetch keeps the ranking");
    assert_ne!(row_text(&c, "oracle", "updated_at"), oracle_before.1, "the availability change moved updated_at");
    assert_eq!(row_text(&c, "oracle", "available_exchanges").as_deref(), Some(r#"{"Exchanges::Kraken":3}"#));
}

/// Takeover of a 2.x CoinGecko install (D-INDEX-UNATTESTED-HELD): Rails' 10-coin row holds the bot stale and is never composed;
/// the woken run ranks it, and the bot is fresh.
#[tokio::test(flavor = "current_thread")]
async fn a_rails_coingecko_row_is_held_until_the_job_ranks_it() {
    use deltabadger::engine::{index, model, staleness};
    let coins = twelve();
    let (d, c, s) = cg_install(&coins.iter().map(String::as_str).collect::<Vec<_>>());
    rails_category_row(&c, "layer-1", &coins[..10].iter().map(String::as_str).collect::<Vec<_>>());
    let bot = model::load_bot(&c, cg_bot(&c, &s, "layer-1", 1)).unwrap();
    let now = at(CG_NOW);
    // As Rails' last pull left the fence (a takeover keeps it): only the ranking holds the bot.
    deltabadger::engine::provider::record(&c, &deltabadger::engine::provider::fingerprint(&c).unwrap().unwrap(), now).unwrap();
    assert!(index::ranked_row(&c, "layer-1").unwrap().is_none(), "Rails' row is never selected");
    assert!(staleness::reference_stale(&c, &bot, now).unwrap().is_some(), "held stale");
    assert!(index::ranking_wanted(&c, &bot, now).unwrap(), "and wanted: the tick wakes the job");
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("layer-1", "L1")]))
        .reply(&cat_key("layer-1"), 200, members(&coins.iter().map(|id| (id.as_str(), 2.0)).collect::<Vec<_>>()));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "top-coins", "categories", "layer-1"], "not in today's slice: asked as followed");
    let c = reopen(&d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    let r = index::ranked_row(&c, "layer-1").unwrap().expect("ranked");
    assert_eq!((r.provider.as_str(), serde_json::from_str::<Vec<String>>(r.top_coins.as_deref().unwrap()).unwrap().len()), ("coingecko", 12));
    assert!(staleness::reference_stale(&c, &bot, now).unwrap().is_none(), "fresh once ranked");
}

/// The Rust half of the composition check (decisions.rb cannot script CoinGecko beneath a venue tick): the pass's stored
/// ranking of an answer composes the members and weights Rails' live composition takes from the same answer (its own caps,
/// to_f; the top N by cap, weighed by cap: index_allocatable.rb:56-123), never the assets' caps.
#[tokio::test(flavor = "current_thread")]
async fn a_category_bot_composes_from_the_stored_answer_as_rails_composes_it_live() {
    use deltabadger::engine::{index, model};
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c", "coin-d"]);
    c.execute("UPDATE assets SET market_cap = 1 WHERE external_id LIKE 'coin-%'", []).unwrap(); // conflicting local caps: never read
    let id = cg_bot(&c, &s, "layer-1", 1);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("layer-1", "L1")]))
        .reply(&cat_key("layer-1"), 200, members(&[("coin-a", 300.0), ("no-asset", 250.0), ("coin-b", 200.0), ("coin-c", 100.0), ("coin-d", 50.0)]));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    let c = reopen(&d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    let venue = deltabadger::venue::fake::FakeVenue::new().ticker("COIN-AEUR", "10", "10", "10").ticker("COIN-BEUR", "10", "10", "10")
        .ticker("COIN-CEUR", "10", "10", "10").ticker("COIN-DEUR", "10", "10", "10");
    let bot = model::load_bot(&c, id).unwrap();
    index::refresh_composition(&c, &venue, &bot, &FixedClock(at(CG_NOW)), &Default::default()).await.unwrap().unwrap();
    let mut s = c.prepare("SELECT a.external_id, m.target_allocation FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id WHERE m.bot_id = ?1 AND m.in_index = 1 ORDER BY m.target_allocation DESC").unwrap();
    let got: Vec<(String, f64)> = s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["coin-a", "coin-b", "coin-c"], "the top 3 by the answer's caps");
    // Rails' blend rounds each target to 6 places.
    for ((_, w), cap) in got.iter().zip([300.0, 200.0, 100.0]) { assert!((w - cap / 600.0).abs() < 1e-6, "{got:?}"); }
}

/// The data-api pull stamps every row it upserts `data-api` under its captured fingerprint (the recorded `indices-import`
/// payload: source 'coingecko' rows included), so they compose on data-api; a Rails row it does not ship stays unattested.
#[tokio::test(flavor = "current_thread")]
async fn the_data_api_pull_ranks_what_it_imports_and_nothing_else() {
    use deltabadger::engine::{index, provider};
    let (d, o, s) = common::install();
    let c = o.primary;
    let bot = seed::index_bot(&c, &s, "layer-1", 3, 0.0, false); // a data-api install
    c.execute("DELETE FROM rust_index_rankings", []).unwrap();
    rails_category_row(&c, "rails-only", &["bitcoin"]);
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/indices", 200, json!({ "data": [
        { "external_id": "nasdaq-100", "source": "deltabadger", "name": "Nasdaq 100", "top_coins": ["AAPL.US"], "weights": { "AAPL.US": 0.08 } },
        { "external_id": "layer-1", "source": "coingecko", "name": "Layer 1", "top_coins": ["bitcoin"], "weights": { "bitcoin": 1.0 } } ] }));
    let api = DataApi::new(Config { url: "https://data.invalid".into(), token: "scripted-token".into() }, t.clone(), t.clone());
    assert_eq!(reference::run_once(reference::INDICES, Some(api), Cx { db: Db::new(c, seed::cipher()), clock: &FixedClock(at(CG_NOW)), wakers: Default::default() }).await, Outcome::Done);
    let c = reopen(&d);
    provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    let fp = provider::fingerprint(&c).unwrap().unwrap();
    let ranked: Vec<(String, String, String)> = c.prepare("SELECT i.external_id, r.provider, r.fingerprint FROM rust_index_rankings r JOIN indices i ON i.id = r.index_id ORDER BY i.external_id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(ranked, [("layer-1".to_string(), "data-api".to_string(), fp.clone()), ("nasdaq-100".into(), "data-api".into(), fp)]);
    assert_eq!(index::ranked_row(&c, "layer-1").unwrap().map(|r| r.source), Some(Some("coingecko".into())));
    assert!(index::ranked_row(&c, "rails-only").unwrap().is_none());
    let bot = deltabadger::engine::model::load_bot(&c, bot).unwrap();
    assert!(deltabadger::engine::staleness::ranking_stale(&c, &bot, at(CG_NOW)).unwrap().is_none(), "the imported row composes");
}

/// Per-ranking fingerprint: a same-slot key change with one followed category fetched and one failing: the failed one stays
/// unselected (its ranking is the old configuration's), the retry asks only it.
#[tokio::test(flavor = "current_thread")]
async fn after_a_key_change_a_failed_category_stays_unselected_and_the_retry_asks_only_it() {
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    cg_bot(&c, &s, "oracle", 1);
    cg_bot(&c, &s, "storage", 1);
    let three = members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]);
    let feed = json!([category("oracle", "O"), category("storage", "S")]);
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed.clone()).reply(&cat_key("oracle"), 200, three.clone()).reply(&cat_key("storage"), 200, three.clone());
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    deltabadger::app_config::set(&reopen(&d), &seed::cipher(), "coingecko_api_key", "cg-other-key", at("2026-10-02T12:00:00Z")).unwrap();
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed.clone()).reply(&cat_key("oracle"), 200, three.clone()).reply(&cat_key("storage"), 500, json!({}));
    assert!(matches!(cg_run(&d, &t, "2026-10-02T12:01:00Z").await, Outcome::Transient(_)));
    assert_eq!(index_ranked(&reopen(&d), "oracle"), Some("coingecko".into()));
    assert_eq!(index_ranked(&reopen(&d), "storage"), None, "the old key's ranking is never selected under the new one");
    let t = cg_transport();
    t.reply(CATS_KEY, 200, feed).reply(&cat_key("storage"), 200, three);
    assert_eq!(cg_run(&d, &t, "2026-10-02T12:16:00Z").await, Outcome::Done);
    assert_eq!(asked(&t), ["ping", "categories", "storage"]);
    assert_eq!(index_ranked(&reopen(&d), "storage"), Some("coingecko".into()));
}

/// A client whose categories request lands while the owner follows two categories (the bots committed mid-request).
struct FollowsDuring(std::path::PathBuf, seed::Seeded, Box<dyn Api>);
impl Api for FollowsDuring {
    fn markets<'a>(&'a self, ids: Option<&'a [String]>, category: Option<&'a str>, per_page: usize, page: usize) -> coingecko::Fut<'a> { self.2.markets(ids, category, per_page, page) }
    fn coin<'a>(&'a self, id: &'a str) -> coingecko::Fut<'a> { self.2.coin(id) }
    fn exchange_tickers<'a>(&'a self, id: &'a str, page: usize) -> coingecko::Fut<'a> { self.2.exchange_tickers(id, page) }
    fn categories(&self) -> coingecko::Fut<'_> {
        let c = Connection::open(&self.0).unwrap();
        cg_bot(&c, &self.1, "old-category", 2);
        cg_bot(&c, &self.1, "music", 1);
        self.2.categories()
    }
}

/// D-CG-FOLLOWED-KEPT against a follow committed during the categories request: the sweep and the disqualification read the
/// followed set in their own transaction, so the swept-to-be row keeps its row and ranking, and the < 3 category is written.
#[tokio::test(flavor = "current_thread")]
async fn a_follow_during_the_categories_request_keeps_its_row_and_ranking() {
    let (d, c, s) = cg_install(&["coin-a", "coin-b"]);
    let fp = deltabadger::engine::provider::fingerprint(&c).unwrap().unwrap();
    let old = rails_category_row(&c, "old-category", &["coin-a"]);
    deltabadger::engine::index::record_ranking(&c, old, "coingecko", &fp, at("2026-09-30T10:31:00Z")).unwrap();
    let before = ranking(&c, "old-category");
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("music", "M")])).reply(&cat_key("music"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0)]));
    let (path, inner) = (d.path().join("production.sqlite3"), cg_clients(&t));
    let seeded = seed::Seeded { ..s };
    let clients: Clients = Rc::new(move |k: &str| Box::new(FollowsDuring(path.clone(), seed::Seeded { ..seeded }, inner(k))) as Box<dyn Api>);
    let c = reopen(&d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    assert_eq!(reference::coingecko_indices(&clients, &Cx { db: Db::new(c, seed::cipher()), clock: &FixedClock(at(CG_NOW)), wakers: Default::default() }).await, Outcome::Done);
    let c = reopen(&d);
    let _ = before;
    // A follow committed during the request is read in the unit's own transaction: the list no longer names the category, so
    // it is retired (r18): its row kept, its ranking dropped.
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM indices WHERE external_id = 'old-category'"), 1, "not swept: its row kept");
    assert_eq!(ranking(&c, "old-category"), None, "retired: its ranking dropped");
    assert_eq!(one::<String>(&c, "SELECT provider FROM rust_index_retired WHERE external_id = 'old-category'"), "coingecko");
    assert_eq!(top_of(&c, "music"), ["coin-a", "coin-b"], "followed since the run began: written, not disqualified");
}

// ---- A17-4 r18: retired categories (D-INDEX-RETIRED-KEPT) ----

fn retirements(c: &Connection) -> Vec<(String, String)> {
    c.prepare("SELECT external_id, provider FROM rust_index_retired ORDER BY external_id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
fn retired_lines(c: &Connection) -> Vec<i64> {
    c.prepare("SELECT bot_id FROM bot_activity_logs WHERE event = 'index_retired' ORDER BY bot_id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// A validated list that no longer names a followed category retires it: the table row, its ranking dropped, one line per
/// following bot; the next run asks nothing of it and writes no second line; its row is kept by the sweep.
#[tokio::test(flavor = "current_thread")]
async fn a_validated_coingecko_list_retires_a_followed_category_once() {
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let (one_bot, two_bot) = (cg_bot(&c, &s, "oracle", 1), cg_bot(&c, &s, "oracle", 2));
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("oracle", "O"), category("meme-token", "M")])).reply(&cat_key("oracle"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]));
    assert_eq!(cg_run(&d, &t, CG_NOW).await, Outcome::Done);
    assert!(ranking(&reopen(&d), "oracle").is_some());
    for (n, now) in ["2026-10-03T10:31:00Z", "2026-10-04T10:31:00Z"].into_iter().enumerate() {
        let t = cg_transport();
        t.reply(CATS_KEY, 200, json!([category("meme-token", "M")]));
        cg_run(&d, &t, now).await;
        assert!(!asked(&t).contains(&"oracle".to_string()), "run {n}: a retired category is not asked: {:?}", asked(&t));
        let c = reopen(&d);
        assert_eq!(retirements(&c), [("oracle".to_string(), "coingecko".to_string())]);
        assert!(ranking(&c, "oracle").is_none(), "its ranking dropped");
        assert_eq!(one::<i64>(&c, "SELECT count(*) FROM indices WHERE external_id = 'oracle'"), 1, "its row kept");
        assert_eq!(retired_lines(&c), [one_bot, two_bot], "one line per following bot, once");
    }
}

/// Only a validated list retires (r18): a list entry without an id, `[{}]`, a list with no eligible category, or a failed
/// read retires nothing and un-retires nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_malformed_coingecko_list_retires_and_clears_nothing() {
    for (body, status) in [(json!([category("meme-token", "M"), { "name": "no id" }]), 200), (json!([{}]), 200), (json!([category("stablecoins", "S")]), 200),
                           (json!([category("meme-token", "M"), { "id": "" }]), 200), (json!({}), 500)] {
        let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
        cg_bot(&c, &s, "oracle", 1);
        c.execute("INSERT INTO rust_index_retired VALUES ('meme-token', 'coingecko', '2026-10-01T00:00:00Z')", []).unwrap();
        let t = cg_transport();
        t.reply(CATS_KEY, status, body.clone()).reply(&cat_key("oracle"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]));
        cg_run(&d, &t, CG_NOW).await;
        let c = reopen(&d);
        assert_eq!(retirements(&c), [("meme-token".to_string(), "coingecko".to_string())], "{body}: nothing retired or cleared");
        assert!(retired_lines(&c).is_empty(), "{body}");
    }
}

/// Re-listing: a validated list naming a retired category again drops the retirement in the same unit and asks it in the
/// same slot; until a new ranking commits its bots are held (a failed fetch leaves them held).
#[tokio::test(flavor = "current_thread")]
async fn a_relisted_category_is_unretired_and_fetched_and_held_until_ranked() {
    use deltabadger::engine::{model, staleness};
    let (d, c, s) = cg_install(&["coin-a", "coin-b", "coin-c"]);
    let bot = cg_bot(&c, &s, "oracle", 1);
    rails_category_row(&c, "oracle", &["coin-a"]);
    c.execute("INSERT INTO rust_index_retired VALUES ('oracle', 'coingecko', '2026-10-01T00:00:00Z')", []).unwrap();
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("oracle", "O")])).reply(&cat_key("oracle"), 500, json!({}));
    let out = cg_run(&d, &t, CG_NOW).await;
    assert!(matches!(&out, Outcome::Transient(m) if m.contains("oracle")), "{out:?}");
    assert!(asked(&t).contains(&"oracle".to_string()), "fetched in the same slot");
    let c = reopen(&d);
    deltabadger::engine::provider::bind(&c, &seed::cipher(), &|_| None).unwrap();
    assert!(retirements(&c).is_empty(), "the retirement dropped");
    let held = || staleness::ranking_stale(&c, &model::load_bot(&c, bot).unwrap(), at(CG_NOW)).unwrap().is_some();
    assert!(held(), "held after the failed fetch");
    let t = cg_transport();
    t.reply(CATS_KEY, 200, json!([category("oracle", "O")])).reply(&cat_key("oracle"), 200, members(&[("coin-a", 3.0), ("coin-b", 2.0), ("coin-c", 1.0)]));
    assert_eq!(cg_run(&d, &t, "2026-10-02T10:46:00Z").await, Outcome::Done);
    assert!(!held(), "ranked: composes again");
}

/// The data-api pull retires too, on its own provider: a successful, non-empty payload in which every row names an
/// external_id; a malformed one retires nothing; a later payload naming the id again un-retires it.
#[tokio::test(flavor = "current_thread")]
async fn the_data_api_pull_retires_what_a_validated_payload_no_longer_lists() {
    let (d, o, s) = common::install();
    let c = o.primary;
    let bot = seed::index_bot(&c, &s, "layer-1", 3, 0.0, false);
    let pull = |body: Value| {
        let t = ScriptedTransport::default();
        t.reply("GET /api/v2/indices", 200, body);
        DataApi::new(Config { url: "https://data.invalid".into(), token: "scripted-token".into() }, t.clone(), t)
    };
    let run = |api, now: &'static str| { let c = reopen(&d); async move { reference::run_once(reference::INDICES, Some(api), Cx { db: Db::new(c, seed::cipher()), clock: &FixedClock(at(now)), wakers: Default::default() }).await } };
    let other = json!({ "external_id": "meme", "source": "coingecko", "name": "Meme", "top_coins": ["bitcoin"] });
    let layer = json!({ "external_id": "layer-1", "source": "coingecko", "name": "Layer 1", "top_coins": ["bitcoin"] });
    drop(c);
    // Malformed: a row with no external_id retires nothing (whatever the import then makes of it).
    run(pull(json!({ "data": [other.clone(), { "source": "coingecko", "name": "nameless" }] })), CG_NOW).await;
    assert!(retirements(&reopen(&d)).is_empty());
    assert_eq!(run(pull(json!({ "data": [other.clone()] })), CG_NOW).await, Outcome::Done);
    let c = reopen(&d);
    assert_eq!(retirements(&c), [("layer-1".to_string(), "data-api".to_string())]);
    assert_eq!(retired_lines(&c), [bot]);
    assert_eq!(one::<i64>(&c, "SELECT count(*) FROM rust_index_rankings r JOIN indices i ON i.id = r.index_id WHERE i.external_id = 'layer-1'"), 0, "its rankings dropped");
    assert_eq!(run(pull(json!({ "data": [other, layer] })), "2026-10-03T10:31:00Z").await, Outcome::Done);
    let c = reopen(&d);
    assert!(retirements(&c).is_empty(), "listed again: un-retired");
    assert_eq!(retired_lines(&c), [bot], "no second line");
}
