//! The synchronous write units of jobs/import.rs (write_assets, write_tokenized, write_indices, write_tickers), which the
//! bootstrap will run inside its one transaction (PLAN-B1 §4, r1.5, r2.6). Each unit writes nothing of its own: rolled
//! back, the database is as before. And each writes exactly what its async publisher writes, ids included.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::jobs::{import, Db};
use deltabadger::store::{self, Paths};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::{json, Value};

type Snapshot = Vec<(String, Vec<Vec<String>>)>;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const NOW: &str = "2026-10-02T10:15:00Z";

/// Every row of every table, sqlite_sequence included, in rowid order, each value with its storage class.
fn snapshot(c: &Connection) -> Snapshot {
    let names: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    names.into_iter().map(|t| {
        let order = if t == "sqlite_sequence" { "name" } else { "rowid" };
        let mut s = c.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY {order}")).unwrap();
        let n = s.column_count();
        let rows = s.query_map([], |r| (0..n).map(|i| r.get_ref(i).map(value)).collect::<Result<Vec<_>, _>>())
            .unwrap().collect::<Result<_, _>>().unwrap();
        (t, rows)
    }).collect()
}

fn value(v: rusqlite::types::ValueRef) -> String {
    use rusqlite::types::ValueRef::*;
    match v { Null => "NULL".into(), Integer(i) => format!("int {i}"), Real(f) => format!("real {f:?}"),
              Text(t) => format!("text {}", String::from_utf8_lossy(t)), Blob(b) => format!("blob {b:?}") }
}

/// Two installs hold the same rows. The stamps of ar_internal_metadata and rust_schema_migrations are each install's
/// creation time, not import output.
fn assert_same(a: &Connection, b: &Connection) {
    let strip = |s: Snapshot| s.into_iter().filter(|(t, _)| t != "ar_internal_metadata" && t != "rust_schema_migrations").collect::<Snapshot>();
    let (a, b) = (strip(snapshot(a)), strip(snapshot(b)));
    for ((ta, ra), (tb, rb)) in a.iter().zip(&b) {
        assert_eq!(ta, tb);
        assert_eq!(ra.len(), rb.len(), "{ta}: row count");
        for (x, y) in ra.iter().zip(rb) { assert_eq!(x, y, "{ta}"); }
    }
    assert_eq!(a.len(), b.len());
}

/// An install with two venues, assets and tickers already in place, some rows deleted so ids and sequences have gaps.
/// Kraken holds XBTEUR under the solana pair (tombstoned by the import) and ethereum under old names (moved).
fn install() -> (tempfile::TempDir, Connection) {
    let dir = common::baseline_install();
    let c = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap().primary;
    c.execute_batch("
        INSERT INTO exchanges (type, name, available, maker_fee, taker_fee, created_at, updated_at) VALUES
          ('Exchanges::Kraken', 'Kraken', 1, '0.1', '0.1', '2026-01-01', '2026-01-01'),
          ('Exchanges::Binance', 'Binance', 1, '0.1', '0.1', '2026-01-01', '2026-01-01');
        INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES
          ('gone', 'G', 'G', 'Cryptocurrency', '2026-01-01', '2026-01-01'),
          ('bitcoin', 'BTC', 'Bitcoin', 'Cryptocurrency', '2026-01-01', '2026-01-01'),
          ('ethereum', 'ETH', 'Ethereum', 'Cryptocurrency', '2026-01-01', '2026-01-01'),
          ('solana', 'SOL', 'Solana', 'Cryptocurrency', '2026-01-01', '2026-01-01'),
          ('EUR.FOREX', 'EUR', 'Euro', 'Currency', '2026-01-01', '2026-01-01'),
          ('tether', 'USDT', 'Tether', 'Cryptocurrency', '2026-01-01', '2026-01-01'),
          ('pax-gold', 'PAXG', 'PAX Gold', 'Cryptocurrency', '2026-01-01', '2026-01-01'),
          ('nvda-xstock', 'NVDAX', 'NVIDIA xStock', 'Cryptocurrency', '2026-01-01', '2026-01-01');
        INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals,
                             minimum_base_size, minimum_quote_size, created_at, updated_at) VALUES
          (1, 'GONEEUR', 'G', 'EUR', 1, 5, 8, 2, 2, 1, 1, '2026-01-01', '2026-01-01'),
          (1, 'XBTEUR', 'XBT', 'EUR', 4, 5, 8, 2, 2, 1, 1, '2026-01-01', '2026-01-01'),
          (1, 'ETHEUR-OLD', 'ETH-OLD', 'EUR', 3, 5, 8, 2, 2, 1, 1, '2026-01-01', '2026-01-01');
        DELETE FROM tickers WHERE ticker = 'GONEEUR';
        DELETE FROM assets WHERE external_id = 'gone';
    ").unwrap();
    (dir, c)
}
fn reopen(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }

fn assets_payload() -> Vec<Value> {
    let mut rows: Vec<Value> = (0..1_201).map(|i| json!({ "external_id": format!("coin-{i}"), "symbol": "C", "name": "C",
        "category": "Cryptocurrency", "market_cap": 1.5e9, "circulating_supply": 120456789.12345678 })).collect();
    rows.push(json!({ "external_id": "bitcoin", "symbol": "BTC", "name": "Bitcoin", "category": "Cryptocurrency", "instrument_type": "coin" }));
    rows.push(json!({ "external_id": "tether", "symbol": "USDT", "name": "Tether", "category": "Cryptocurrency", "instrument_type": null }));
    rows.push(json!({ "external_id": "ondo-x-ondo-tokenized", "symbol": "OX", "name": "OX", "category": "Cryptocurrency", "instrument_type": "coin" }));
    rows
}
fn indices_payload() -> Vec<Value> {
    vec![json!({ "external_id": "layer-1", "source": "coingecko", "name": "L1", "top_coins": ["bitcoin"], "market_cap": "123.45" }),
         json!({ "external_id": "meme-token", "source": "coingecko", "name": "Meme", "weights": {"a": 1} }),
         json!({ "external_id": "layer-1", "source": "coingecko", "name": "Layer 1" })]
}
fn pair(ext: &str, quote_ext: &str, ticker: &str, base: &str, quote: &str) -> Value {
    json!({ "ticker": ticker, "base": base, "quote": quote, "base_external_id": ext, "quote_external_id": quote_ext, "base_decimals": 8,
            "quote_decimals": 2, "price_decimals": 1, "minimum_base_size": "0.0001", "minimum_quote_size": "0.5", "maximum_base_size": null,
            "maximum_quote_size": "1000000" })
}
/// Two venue files, as the seeds hold them: Kraken (a rename, a tombstone, a new row) then Binance (only new rows).
fn files() -> [(i64, Vec<Value>); 2] {
    [(1, vec![pair("bitcoin", "EUR.FOREX", "XBTEUR", "XBT", "EUR"), pair("ethereum", "EUR.FOREX", "ETHEUR", "ETH", "EUR")]),
     (2, vec![pair("bitcoin", "tether", "BTCUSDT", "BTC", "USDT"), pair("ethereum", "tether", "ETHUSDT", "ETH", "USDT")])]
}

/// Runs `unit` in a transaction, checks it wrote, rolls back, and checks the database is as before.
fn rolls_back(c: &Connection, unit: impl FnOnce(&Connection)) {
    let before = snapshot(c);
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate).unwrap();
    unit(&tx);
    assert_ne!(snapshot(&tx), before, "the unit wrote inside the transaction");
    tx.rollback().unwrap();
    assert_eq!(snapshot(c), before, "rolled back, every row and sqlite_sequence are as before");
}

/// Runs `unit` in one transaction and commits.
fn committed(c: &Connection, unit: impl FnOnce(&Connection)) {
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate).unwrap();
    unit(&tx);
    tx.commit().unwrap();
}

#[test]
fn each_unit_rolled_back_leaves_the_database_as_it_was() {
    let (_d, c) = install();
    rolls_back(&c, |tx| import::write_assets(tx, import::plan_assets(&assets_payload(), "").unwrap(), at(NOW)).unwrap());
    rolls_back(&c, |tx| import::write_tokenized(tx).unwrap());
    rolls_back(&c, |tx| import::write_indices(tx, import::plan_indices(&indices_payload()).unwrap(), at(NOW)).unwrap());
    rolls_back(&c, |tx| for (venue, rows) in files() {
        let plan = import::plan_tickers_with(tx, venue, &rows, None, false).unwrap();
        import::write_tickers(tx, venue, plan, at(NOW)).unwrap();
    });
}

#[tokio::test(flavor = "current_thread")]
async fn write_assets_and_write_tokenized_write_what_import_assets_writes() {
    let (da, a) = install();
    import::import_assets(&Db::new(a, common::seed::cipher()), import::plan_assets(&assets_payload(), "").unwrap(), at(NOW)).await.unwrap();
    let (_db, b) = install();
    committed(&b, |tx| {
        import::write_assets(tx, import::plan_assets(&assets_payload(), "").unwrap(), at(NOW)).unwrap();
        import::write_tokenized(tx).unwrap();
    });
    let a = reopen(&da);
    assert_same(&a, &b);
    let marked: i64 = b.query_row("SELECT count(*) FROM assets WHERE instrument_type = 'tokenized'", [], |r| r.get(0)).unwrap();
    assert_eq!(marked, 3, "pax-gold, nvda-xstock and the ondo row: the payload's type is overwritten by the registry");
}

#[tokio::test(flavor = "current_thread")]
async fn write_tokenized_writes_what_mark_tokenized_writes() {
    let (da, a) = install();
    import::mark_tokenized(&Db::new(a, common::seed::cipher())).await.unwrap();
    let (_db, b) = install();
    committed(&b, |tx| import::write_tokenized(tx).unwrap());
    assert_same(&reopen(&da), &b);
}

#[tokio::test(flavor = "current_thread")]
async fn write_indices_writes_what_import_indices_writes() {
    let (da, a) = install();
    import::import_indices(&Db::new(a, common::seed::cipher()), import::plan_indices(&indices_payload()).unwrap(), at(NOW)).await.unwrap();
    let (_db, b) = install();
    committed(&b, |tx| import::write_indices(tx, import::plan_indices(&indices_payload()).unwrap(), at(NOW)).unwrap());
    assert_same(&reopen(&da), &b);
}

/// Two files one after another: the second file's plan is made after the first file's write, on the same connection, and
/// so gives the ids the async path gives. A plan made before the first write would collide with it.
#[tokio::test(flavor = "current_thread")]
async fn write_tickers_file_after_file_writes_what_the_async_path_writes() {
    let (da, a) = install();
    let db = Db::new(a, common::seed::cipher());
    let mut async_written = vec![];
    for (venue, rows) in files() {
        let plan = db.run(move |c, _| import::plan_tickers_with(c, venue, &rows, None, false)).await.unwrap();
        async_written.push(import::publish_tickers_fenced(&db, venue, plan, at(NOW), None).await.unwrap());
    }
    drop(db);

    let (_db, b) = install();
    let [(kraken, kraken_rows), (binance, binance_rows)] = files();
    let early = import::plan_tickers_with(&b, binance, &binance_rows, None, false).unwrap();
    let mut written = vec![];
    committed(&b, |tx| {
        let plan = import::plan_tickers_with(tx, kraken, &kraken_rows, None, false).unwrap();
        written.push(import::write_tickers(tx, kraken, plan, at(NOW)).unwrap());
        let plan = import::plan_tickers_with(tx, binance, &binance_rows, None, false).unwrap();
        let new_ids = |p: &import::TickerPlan| p.units.iter().flat_map(|u| &u.writes)
            .filter_map(|w| match w { import::TickerWrite::Upsert { existing: None, id, .. } => Some(*id), _ => None }).collect::<Vec<_>>();
        assert_ne!(new_ids(&plan), new_ids(&early), "the second file's ids depend on what the first file wrote");
        written.push(import::write_tickers(tx, binance, plan, at(NOW)).unwrap());
    });
    assert_eq!(written, async_written);
    assert_same(&reopen(&da), &b);
    let stale: String = b.query_row("SELECT ticker FROM tickers WHERE base_asset_id = 4 AND exchange_id = 1 AND available = 0", [], |r| r.get(0)).unwrap();
    assert_eq!(stale, "__stale_2_XBTEUR", "the fixture exercises a tombstone");
}
