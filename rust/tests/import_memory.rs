//! PLAN-710-importmem B6′: what an import holds grows with its input, linearly, never with a product of it. This binary
//! counts the bytes the current thread holds (allocated less freed) and the most it held, so each bound below is the peak
//! of one call made on this thread. The bounds are generous multiples of the input, set to fail on quadratic growth.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::sync::{self, activities::{Entry, Raw}, binance_identity::{Context, Event, Source}, binance_writer as writer};
use deltabadger::tracker::import::run;
use deltabadger::{engine::model, lease, store};
use rusqlite::Connection;
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! {
    static HELD: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}
fn grew(by: isize) { HELD.with(|h| { h.set(h.get() + by); PEAK.with(|p| if h.get() > p.get() { p.set(h.get()) }) }) }
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { grew(l.size() as isize); unsafe { System.alloc(l) } }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 { grew(l.size() as isize); unsafe { System.alloc_zeroed(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { grew(-(l.size() as isize)); unsafe { System.dealloc(p, l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 { grew(size as isize - l.size() as isize); unsafe { System.realloc(p, l, size) } }
}
#[global_allocator]
static A: Counting = Counting;

/// What `f` returns, and the most it held at once above what was held when it started.
fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let start = HELD.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let out = f();
    (out, (PEAK.with(Cell::get) - start).max(0) as usize)
}

const MIB: usize = 1 << 20;
fn now() -> DateTime<Utc> { "2026-08-18T00:00:00Z".parse().unwrap() }

/// A header of the format's `columns` and `wide` more empty ones, then `rows` records, each its `record` and `wide` empty
/// cells.
fn wide(columns: &str, record: &str, wide: usize, rows: usize) -> Vec<u8> {
    let pad = ",".repeat(wide);
    let mut out = format!("{columns}{pad}\n");
    for _ in 0..rows { out.push_str(record); out.push_str(&pad); out.push('\n'); }
    out.into_bytes()
}

const BINANCE: (&str, &str) = ("User ID,UTC_Time,Account,Operation,Coin,Change,Remark", "1,2026-08-17 10:00:00,Spot,Deposit,BTC,1,");
const KUCOIN: (&str, &str) = ("UID,Account Type,Currency,Side,Amount,Fee,Time(UTC+03:00),Remark,Type", "123456789,mainAccount,BTC,Deposit,1,0,2026-08-17 13:00:00,,Deposit");
fn deltabadger() -> (String, String) {
    (deltabadger::web::tracker::CSV_HEADERS.join(","), "2026-08-17T10:00:00Z,deposit,BTC,1,,,,,kraken,t1,,,1".to_string())
}

/// B2′: a file of a few MiB that is wide (100k empty columns on every line) is read holding about its own size: the
/// cells no reader names are never kept.
#[test]
fn a_wide_file_is_read_holding_about_its_size() {
    let (dh, dr) = deltabadger();
    for (format, (columns, record)) in [("binance", BINANCE), ("kucoin", KUCOIN), ("deltabadger", (dh.as_str(), dr.as_str()))] {
        let bytes = wide(columns, record, 100_000, 40);
        let offset = (format == "binance").then_some("+00:00");
        let (read, held) = peak(|| run::read(format, &bytes, offset).map(|_| ()));
        assert!(read.is_ok(), "{format}: the file is read");
        assert!(held < 3 * bytes.len() + MIB, "{format}: held {held} bytes reading {} bytes", bytes.len());
    }
}

/// B2′/A7: a tall file (a header, one record, then a million blank lines) is read holding about its own size: a blank line
/// keeps nothing.
#[test]
fn a_tall_file_of_blank_lines_is_read_holding_about_its_size() {
    let (dh, dr) = deltabadger();
    for (format, (columns, record)) in [("binance", BINANCE), ("kucoin", KUCOIN), ("deltabadger", (dh.as_str(), dr.as_str()))] {
        let mut bytes = wide(columns, record, 0, 1);
        bytes.extend(std::iter::repeat_n(b'\n', MIB));
        let offset = (format == "binance").then_some("+00:00");
        let (read, held) = peak(|| run::read(format, &bytes, offset).map(|_| ()));
        assert!(read.is_ok(), "{format}: the file is read");
        assert!(held < 3 * bytes.len() + MIB, "{format}: held {held} bytes reading {} bytes", bytes.len());
    }
}

/// C1: a part's `Content-Disposition` with 100k parameters no reader names keeps none of them.
#[test]
fn ignored_multipart_parameters_are_not_kept() {
    let boundary = "----b0undary";
    let ignored = "; x=1".repeat(100_000);
    let body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"format\"{ignored}\r\n\r\nbinance\r\n--{boundary}--\r\n").into_bytes();
    let (parsed, held) = peak(|| deltabadger::web::multipart::parse(&body, boundary));
    assert_eq!(parsed.unwrap().fields, vec![("format".to_string(), "binance".to_string())]);
    assert!(held < 64 << 10, "held {held} bytes for a {}-byte body", body.len());
}

/// A Kraken install whose venue is Binance, its engine lock held (the Binance writer's precondition), as tests/binance_writer.rs.
struct Binance { dir: tempfile::TempDir, o: store::Opened, key: sync::Key }
fn binance() -> (Binance, lease::EngineLock) {
    let (dir, o, s) = common::install();
    o.primary.execute("UPDATE exchanges SET type='Exchanges::Binance' WHERE id=?1", [s.exchange_id]).unwrap();
    let key = sync::load_key_of(&o.primary, s.api_key_id, &["Exchanges::Binance"]).unwrap();
    let lock = lease::lock(&store::Paths::from_env(&|_| None, dir.path()), now()).unwrap();
    lease::claim(&lock, &o.primary, &common::seed::cipher(), "test", now()).unwrap();
    (Binance { dir, o, key }, lock)
}

/// B3′/A3: a grouped swap (one leg spending a coin with a 64 KiB name, `legs` identical short legs bought) is read and
/// written holding about the file's size: the coin and the swap's group id are kept once, not once per leg, and the
/// duplicate rule reads no stored group id.
#[test]
fn a_long_named_grouped_swap_is_written_holding_about_its_size() {
    let coin = "Z".repeat(64 << 10);
    let legs = 300;
    let mut text = format!("User ID,UTC_Time,Account,Operation,Coin,Change,Remark\n1,2026-08-17 10:00:00,Spot,Transaction Spend,{coin},-{legs},\n");
    for _ in 0..legs { text.push_str("1,2026-08-17 10:00:00,Spot,Transaction Buy,ETH,1,\n"); }
    let (b, _lock) = binance();
    let (answer, held) = peak(|| run::import(&b.o.primary, &common::seed::cipher(), b.key.user_id, "binance", text.as_bytes(), Some("+00:00"), Some(&b.key.id.to_string()), now()).unwrap());
    assert!(matches!(answer, run::Answer::Written { imported, .. } if imported == 2 * legs), "{answer:?}");
    assert!(held < 16 * text.len() + 4 * MIB, "held {held} bytes writing a {}-byte file", text.len());
    // The stored rows keep today's spelling of the group (rule B3: nothing truncated).
    let group: String = b.o.primary.query_row("SELECT group_id FROM account_transactions ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(group, format!("swapcsv_{}_{coin}_ETH", "2026-08-17T10:00:00Z".parse::<DateTime<Utc>>().unwrap().timestamp()));
    drop(b.dir);
}

/// An API deposit of 1 BTC (a retained binding), as tests/binance_writer.rs writes one.
fn deposit(id: &str) -> writer::Input {
    let raw = json!({"id": id, "txId": format!("hash-{id}"), "coin": "BTC", "network": "BTC", "address": "addr", "addressTag": "", "amount": "1",
                     "insertTime": 1786960800000_i64, "applyTime": "2026-08-17 10:00:00", "transactionFee": "0.01", "transId": "detail"});
    writer::Input { event: Event::read(Source::Deposit, &raw, Context { parent: Some(""), occurred_ms: Some(1786960800000), ..Context::default() }).unwrap(), entry: Entry {
        entry_type: 4, base_currency: Some("BTC".into()), base_amount: deltabadger::v2::BigDec::parse("1").unwrap(), quote_currency: None, quote_amount: None,
        fee_currency: None, fee_amount: None, tx_id: Some(format!("hash-{id}")), group_id: None, description: None,
        transacted_at: Some("2026-08-17T10:00:00Z".parse().unwrap()), raw: Raw::from_value(raw) } }
}

/// E3: a unit of `legs` identical file deposits against `bindings` retained deposits that each fit every leg is the
/// existing ambiguity (warned, nothing kept), found holding what grows with the legs and the bindings, not their product.
#[test]
fn identical_legs_against_many_retained_bindings_hold_no_product() {
    let held_for = |legs: usize| {
        let (b, _lock) = binance();
        let c = &b.o.primary;
        let bindings: Vec<writer::Input> = (0..500).map(|n| deposit(&format!("d{n}"))).collect();
        let tx = model::immediate(c).unwrap();
        writer::store(&model::fence_versions(&tx, &[]).unwrap(), &common::seed::cipher(), &b.key, &bindings, now()).unwrap();
        tx.commit().unwrap();
        let entries: Vec<Entry> = (0..legs).map(|_| { let mut e = bindings[0].entry.clone(); e.tx_id = None; e.group_id = Some("unit".into()); e.raw = Raw::from_value(json!({"operation": "Deposit"})); e }).collect();
        let tx = model::immediate(c).unwrap();
        let fenced = model::fence_versions(&tx, &[]).unwrap();
        let (out, held) = peak(|| writer::store_csv(&fenced, &common::seed::cipher(), &b.key, &entries, "k", "binance", false, now()).map(|_| ()));
        let warned = out.expect_err("several retained candidates warn");
        assert!(writer::warned(&warned).is_some(), "{}", warned.0);
        held
    };
    let (one, many) = (held_for(1), held_for(4000));
    assert!(many < one + 4000 * 1024, "one leg held {one} bytes, 4000 legs {many}");
}

/// KuCoin export rows for `seconds` separate seconds: each a lone Spot purchase leg, and (with `paired`) its spent leg too.
fn kucoin_seconds(seconds: usize, paired: bool) -> String {
    let mut out = vec!["UID,Account Type,Currency,Side,Amount,Fee,Time(UTC+03:00),Remark,Type".to_string()];
    for n in 0..seconds {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().and_hms_opt(10, 0, 0).unwrap() + chrono::Duration::seconds(n as i64 * 10);
        let at = at.format("%Y-%m-%d %H:%M:%S");
        out.push(format!("123456789,mainAccount,BTC,Deposit,0.001,0,{at},,Spot"));
        if paired { out.push(format!("123456789,mainAccount,USDT,Withdrawal,60,0,{at},,Spot")); }
    }
    out.join("\n")
}

/// C4/D2/E6: a re-upload that replaces the rows of many separate seconds (each a lone leg the first upload stored as an
/// adjustment, now the purchase its pair makes) holds about its own size: the rows it removes are audited for the links
/// they undo one at a time, never all of their contents at once.
#[test]
fn a_reupload_replacing_many_seconds_holds_no_copy_of_the_rows_it_removes() {
    let dir = common::baseline_install();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    store::configure(&c).unwrap();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kucoin', 'KuCoin', '0.1', '0.1', '2026-01-01', '2026-01-01')", []).unwrap();
    c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('o@example.com', 'x', 1, '2026-01-01', '2026-01-01')", []).unwrap();
    let user = c.last_insert_rowid();
    let exchange: i64 = c.query_row("SELECT id FROM exchanges WHERE type = 'Exchanges::Kucoin'", [], |r| r.get(0)).unwrap();
    let mut assets = vec![];
    for symbol in ["BTC", "USDT"] {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')",
                  [format!("seed-{symbol}"), symbol.to_string()]).unwrap();
        assets.push(c.last_insert_rowid());
    }
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, 'BTC-USDT', 'BTC', 'USDT', ?2, ?3, 8, 8, 1, '0', '0', 1, 1, '2026-01-01', '2026-01-01')",
              rusqlite::params![exchange, assets[0], assets[1]]).unwrap();
    let cipher = common::seed::cipher();
    let seconds = 2000;
    assert!(matches!(run::import(&c, &cipher, user, "kucoin", kucoin_seconds(seconds, false).as_bytes(), None, None, now()).unwrap(), run::Answer::Written { .. }));
    // Each stored row made heavy (as an edited description is): what the audit would copy.
    c.execute("UPDATE account_transactions SET description = ?1", [&"d".repeat(4096)]).unwrap();
    let text = kucoin_seconds(seconds, true);
    let (answer, held) = peak(|| run::import(&c, &cipher, user, "kucoin", text.as_bytes(), None, None, now()).unwrap());
    assert!(matches!(answer, run::Answer::Written { .. }), "{answer:?}");
    let removed: i64 = c.query_row("SELECT count(*) FROM account_transactions WHERE description IS NOT NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(removed, 0, "every second was replaced");
    assert!(held < 8 * text.len() + seconds * 2048, "held {held} bytes replacing {seconds} seconds of a {}-byte file", text.len());
}
