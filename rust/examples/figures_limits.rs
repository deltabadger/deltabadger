//! Hostile numbers and long histories against the figures, with no Rails in them.
//!
//! Numbers: compact values that would be a gigabyte written out, digit strings of ten megabytes, integers beyond
//! 64 bits, Infinity. Each must come back as "not computed", within a second, and none may panic or allocate what
//! it describes.
//!
//! Histories: 100,000 orders; a holding recorded twice over, with and without its asset; thousands of sales that
//! each lengthen a cost; sales that double it. Each is held to the figures' own meters (`budget`): the steps it
//! takes and the limbs it holds are counted, not timed, so a pass that is quadratic in the orders fails here on
//! every machine alike. Times and the allocator's peak are measured and printed beside them.
//!
//!   cargo run --release --example figures_limits
//!
//! runs them in the release profile, where a panic aborts the process (`panic = "abort"`): the run ends with exit
//! status 0 and its last line, or it does not end well. rust/tests/figures_limits.rs runs the same checks in
//! `cargo test`, whose harness always unwinds.
use deltabadger::figures::at::At;
use deltabadger::figures::budget::{self, Limits, Used, FIGURE};
use deltabadger::figures::db::Subject;
use deltabadger::figures::dec::{Dec, MAX_SCALE};
use deltabadger::figures::num::{Num, NumError};
use deltabadger::figures::scripted::Scripted;
use deltabadger::figures::totals::{self, Rates};
use deltabadger::figures::{chart, live, splits, walk, FiguresError, NOT_A_NUMBER, OUT_OF_RANGE, OVER_BUDGET};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// The allocator, counting: the bytes allocated now, and the most there have been.
struct Counting;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        PEAK.fetch_max(ALLOCATED.fetch_add(layout.size(), Relaxed) + layout.size(), Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Longer than this and a check has computed something it should have refused.
const QUICKLY: Duration = Duration::from_secs(1);
/// How long any history here may take. No build on any machine should come near it (a debug build on a busy one
/// takes a sixth). It is not what catches a pass that is quadratic in the orders: the steps are.
const AT_MOST: Duration = Duration::from_secs(60);
/// The orders of the long histories, and of the short one beside the first of them.
const LONG: i64 = 100_000;
const SHORT: i64 = LONG / 16;
/// The steps (`budget`) all the figures of a bot may take per order of a history of plain orders: 200 are counted
/// for 100,000 orders and for 6,250 alike, and 172 for the holding recorded twice over. A list of the 100,000
/// instants so far searched once per order would add 50,000 per order.
const STEPS_PER_ORDER: u64 = 220;
const MEGABYTE: usize = 1 << 20;
const NOW: At = At(1_790_000_000_000_000_000);

const SCHEMA: &str = "
    CREATE TABLE users (id integer PRIMARY KEY, time_zone varchar, display_currency varchar, hide_balances boolean);
    CREATE TABLE exchanges (id integer PRIMARY KEY, type varchar);
    CREATE TABLE assets (id integer PRIMARY KEY, symbol varchar, name varchar, category varchar, external_id varchar);
    CREATE TABLE tickers (id integer PRIMARY KEY, exchange_id integer, ticker varchar, base varchar, base_asset_id integer, quote_asset_id integer,
                          quote_decimals integer, available boolean, trading_enabled boolean);
    CREATE TABLE bots (id integer PRIMARY KEY, user_id integer, exchange_id integer, type varchar, settings json, status integer);
    CREATE TABLE bot_index_assets (bot_id integer, asset_id integer, ticker_id integer, in_index boolean);
    CREATE TABLE transactions (id integer PRIMARY KEY, bot_id integer, status integer, created_at datetime(6), exchange_id integer, price decimal, amount decimal,
                               amount_exec decimal, quote_amount_exec decimal, base varchar, base_asset_id integer, side integer, external_status integer,
                               transaction_type varchar);
    CREATE TABLE account_transactions (user_id integer, exchange_id integer, entry_type integer, base_currency varchar, raw_data json, transacted_at datetime(6));
    INSERT INTO users VALUES (1, 'UTC', 'EUR', 0);
    INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca');
    INSERT INTO assets VALUES (1, 'USDT', 'Tether', 'Cryptocurrency', 'tether'), (2, 'AAA', 'AAA Inc.', 'Stock', 'stock-aaa'), (3, 'BBB', 'BBB Inc.', 'Stock', 'stock-bbb');
    INSERT INTO tickers VALUES (1, 1, 'AAA', 'AAA', 2, 1, 2, 1, 1), (2, 1, 'BBB', 'BBB', 3, 1, 2, 1, 1);
    INSERT INTO bots VALUES (1, 1, 1, 'Bots::DcaMultiAsset', '{\"quote_asset_id\": 1, \"allocations\": {\"2\": 0.5, \"3\": 0.5}}', 1);
";

/// One install: a basket of one stock, quoted in USDT, with one buy at `price` (a column value, as SQL).
fn install(price: &str, split_ratio: Option<&str>, quote_decimals: i64) -> Result<Connection, String> {
    let c = Connection::open_in_memory().map_err(|e| e.to_string())?;
    c.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    c.execute(&format!("INSERT INTO transactions VALUES (1, 1, 0, '2026-03-02 14:30:00', 1, {price}, 1, 1, 100, 'AAA', 2, 0, 2, 'REGULAR')"), []).map_err(|e| e.to_string())?;
    c.execute("UPDATE tickers SET quote_decimals = ?1", [quote_decimals]).map_err(|e| e.to_string())?;
    if let Some(ratio) = split_ratio {
        c.execute("INSERT INTO account_transactions VALUES (1, 1, 15, 'AAA', ?1, '2026-03-03 00:00:00')", [json!({ "corporate_action": "split", "split_ratio": ratio }).to_string()]).map_err(|e| e.to_string())?;
    }
    Ok(c)
}

/// One asset bought every minute for 69 days, sold whole, and bought again; beside it a holding the venue neither
/// prices nor has candles for, so every point of the chart keeps its fill mark.
fn long_history(c: &Connection, orders: i64) -> Result<(), String> {
    c.execute_batch(&format!("
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {orders})
        INSERT INTO transactions SELECT i + 10, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + i * 60, 'unixepoch'), 1, 100 + (i % 700) * 0.01, 0.01, 0.01,
                                        (100 + (i % 700) * 0.01) * 0.01, 'AAA', 2, 0, 2, 'REGULAR' FROM n;
        INSERT INTO transactions VALUES (2, 1, 0, '2026-03-02 14:30:01', 1, 55.21, 0.5, 0.5, 27.605, 'BBB', 3, 0, 2, 'REGULAR');
        INSERT INTO transactions VALUES (5, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + ({orders} + 1) * 60, 'unixepoch'), 1, 104, {orders} * 0.01 + 1, {orders} * 0.01 + 1,
                                         104 * ({orders} * 0.01 + 1), 'AAA', 2, 1, 2, 'LIQUIDATION');
        INSERT INTO transactions VALUES (6, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + ({orders} + 2) * 60, 'unixepoch'), 1, 104.5, 0.01, 0.01, 1.045, 'AAA', 2, 0, 2, 'REGULAR');
    ")).map_err(|e| e.to_string())
}


/// A holding recorded twice over: `orders` / 2 buys from before orders stored their asset, then as many sales of the
/// asset itself, of which the ledger holds none (so no sale lengthens a cost). Every sale asks whether the lots
/// recorded without the asset still hold units.
fn shadowed_history(c: &Connection, orders: i64) -> Result<(), String> {
    let half = orders / 2;
    c.execute_batch(&format!("
        UPDATE transactions SET base_asset_id = NULL WHERE id = 1;
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {half})
        INSERT INTO transactions SELECT i + 10, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + i * 60, 'unixepoch'), 1, 100, 0.01, 0.01, 1, 'AAA', NULL, 0, 2, 'REGULAR' FROM n;
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {half})
        INSERT INTO transactions SELECT i + 10 + {half}, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + (i + {half}) * 60, 'unixepoch'), 1, 101, 0.00001, 0.00001, 0.00101, 'AAA', 2, 1, 2, 'REGULAR' FROM n;
    ")).map_err(|e| e.to_string())
}

/// Three units bought for 100; then, `pairs` times, one of them sold and one bought back for 33. Priced: each
/// sale reports 34, and the holding's cost gains 32 digits (a third of it is taken off, and a third is 32 digits).
/// Not priced: a REBALANCE sale with no proceeds reported, whose estimate is the cost itself, which the buy then
/// divides by: the cost's digits double.
fn pairs(c: &Connection, pairs: i64, priced: bool) -> Result<(), String> {
    let sale = if priced { "34, 1, 1, 34, 'AAA', 2, 1, 2, 'REGULAR'" } else { "0, 1, 1, NULL, 'AAA', 2, 1, 2, 'REBALANCE'" };
    c.execute_batch(&format!("
        UPDATE transactions SET amount = 3, amount_exec = 3 WHERE id = 1;
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {pairs})
        INSERT INTO transactions SELECT i * 2, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + i * 120, 'unixepoch'), 1, {sale} FROM n;
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {pairs})
        INSERT INTO transactions SELECT i * 2 + 1, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1772461800 + i * 120 + 60, 'unixepoch'), 1, 33, 1, 1, 33, 'AAA', 2, 0, 2, 'REGULAR' FROM n;
    ")).map_err(|e| e.to_string())
}

/// `buys` plain buys after everything else: each adds to the cost the sales left, and the chart keeps the sum.
fn then_buys(c: &Connection, buys: i64) -> Result<(), String> {
    c.execute_batch(&format!("
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {buys})
        INSERT INTO transactions SELECT 1000000 + i, 1, 0, strftime('%Y-%m-%d %H:%M:%S', 1773000000 + i * 60, 'unixepoch'), 1, 33, 1, 1, 33, 'AAA', 2, 0, 2, 'REGULAR' FROM n;
    ")).map_err(|e| e.to_string())
}

/// Everything the library computes for the bot, in order, through to what the page is sent; the first thing it
/// would not compute. The points of the chart, and the bytes of its attributes.
fn figures(c: &Connection, script: &Value, now: At) -> Result<(usize, usize), FiguresError> {
    let market = Scripted::new(script, Some("deltabadger"));
    let subject = Subject::load(c, 1)?;
    let metrics = walk::metrics(c, &subject, now)?;
    let live = live::live(c, &subject, &metrics, &market, now)?;
    let marked = chart::marked(c, &subject, &live, &market, now)?;
    let page = chart::page(c, &subject, &marked, false)?;
    totals::profit_in_usd(c, &market, &mut Rates::default(), subject.quote.as_deref(), &live)?;
    totals::denomination(c, &market, "EUR")?;
    let written = page.map_or(0, |page| page.attributes(&chrono_tz::Tz::UTC, 3).iter().map(|(_, value)| value.len()).sum());
    Ok((marked.chart.labels.len(), written))
}

fn script(price: Value, open: Value, coin: Value, usd_rate: Value) -> Value {
    let bars = json!({ "status": 200, "body": { "bars": [{ "t": "2026-03-02T05:00:00Z", "o": open }], "symbol": "AAA" } });
    json!({
        "GET data.alpaca.markets/v2/stocks/snapshots": { "status": 200, "body": { "AAA": { "latestTrade": { "p": price } } } },
        "GET data.alpaca.markets/v2/stocks/AAA/bars": bars, "GET data.alpaca.markets/v2/stocks/AAA/bars?adjustment=split": bars,
        "GET data-api:3000/api/v1/prices?coin_ids=tether&vs_currencies=usd": { "status": 200, "body": { "data": { "tether": { "usd": coin } } } },
        "GET data-api:3000/api/v1/exchange_rates": { "status": 200, "body": { "data": { "usd": { "value": usd_rate }, "eur": { "value": 55000.5 } } } },
    })
}

fn fine() -> Value { script(json!(104.52), json!(100.0), json!(0.9993), json!(64000.5)) }

/// What all the figures of the bot of `c` cost, under one figure's limits for the lot of them.
struct Cost { points: Result<(usize, usize), FiguresError>, used: Used, took: Duration, peak: usize }

fn cost(c: &Connection) -> Cost {
    let before = ALLOCATED.load(Relaxed);
    PEAK.store(before, Relaxed);
    let started = Instant::now();
    let (points, used) = budget::scope(FIGURE, || figures(c, &fine(), NOW));
    Cost { points, used, took: started.elapsed(), peak: PEAK.load(Relaxed).saturating_sub(before) }
}

/// A history on top of one install, and what its figures cost. Printed: the numbers the plan states are these.
fn history(what: &str, build: &dyn Fn(&Connection) -> Result<(), String>) -> Result<Cost, String> {
    let c = install("100", None, 2)?;
    build(&c)?;
    let cost = cost(&c);
    let outcome = match &cost.points { Ok((points, written)) => format!("{points} points, {written} bytes written"), Err(error) => format!("{error:?}") };
    eprintln!("{what}: {outcome}; {} steps, {} limbs held, {} MB allocated at most, {:?}", cost.used.steps, cost.used.held, cost.peak / MEGABYTE, cost.took);
    if cost.took > AT_MOST { return Err(format!("{what}: only after {:?}", cost.took)); }
    Ok(cost)
}

fn computed(what: &str, cost: &Cost, points: usize) -> Result<(), String> {
    match &cost.points { Ok((drawn, _)) if *drawn == points => Ok(()), other => Err(format!("{what}: {other:?}, not {points} points")) }
}

fn at_most(what: &str, name: &str, value: u64, limit: u64) -> Result<(), String> {
    if value > limit { Err(format!("{what}: {value} {name}, more than {limit}")) } else { Ok(()) }
}

/// The histories: each computed (or refused) for what it is, within steps and limbs that are counted.
fn histories() -> Result<usize, String> {
    // Plain orders, and many of them: linear, by count. (The sorts make it a little more than linear.)
    for orders in [SHORT, LONG] {
        let what = format!("a history of {orders} orders");
        let cost = history(&what, &|c| long_history(c, orders))?;
        computed(&what, &cost, orders as usize + 5)?;
        at_most(&what, "steps: a pass is not linear", cost.used.steps, STEPS_PER_ORDER * orders as u64)?;
    }
    let what = format!("{LONG} orders of a holding recorded with and without its asset");
    let cost = history(&what, &|c| shadowed_history(c, LONG))?;
    computed(&what, &cost, LONG as usize / 2 * 2 + 2)?;
    at_most(&what, "steps: a pass is not linear", cost.used.steps, STEPS_PER_ORDER * LONG as u64)?;

    // Sales that lengthen a cost by 32 digits each: 700 of them, as in the grid, where Rails' figures are the same.
    let what = "700 priced sales, each followed by a buy";
    let cost = history(what, &|c| pairs(c, 700, true))?;
    computed(what, &cost, 1402)?;
    at_most(what, "steps", cost.used.steps, 26_000_000)?;
    at_most(what, "limbs held", cost.used.held, 2_000_000)?;

    // Sales that double it: eleven are computed (Rails takes 1.5 seconds), the twelfth is beyond a figure's steps.
    let what = "11 sales with no proceeds reported, each followed by a buy";
    let cost = history(what, &|c| pairs(c, 11, false))?;
    computed(what, &cost, 24)?;
    let what = "12 sales with no proceeds reported, each followed by a buy";
    let cost = history(what, &|c| pairs(c, 12, false))?;
    if !matches!(&cost.points, Err(FiguresError::NotComputed(why)) if why == OVER_BUDGET) { return Err(format!("{what}: {:?}", cost.points)); }
    at_most(what, "steps", cost.used.steps, FIGURE.steps)?;

    // A long cost carried into a long chart, through to what the page is sent: 600 such sales leave a cost of
    // 19,200 digits, and each of 20,000 buys after them makes another, which the chart keeps. Steps, limbs,
    // bytes and time, all four.
    let what = "600 priced sales and buys, then 20000 buys";
    let cost = history(what, &|c| pairs(c, 600, true).and_then(|()| then_buys(c, 20_000)))?;
    computed(what, &cost, 21_202)?;
    at_most(what, "steps", cost.used.steps, 120_000_000)?;
    at_most(what, "limbs held", cost.used.held, 48_000_000)?;
    at_most(what, "bytes allocated at one time", cost.peak as u64, 320 * MEGABYTE as u64)?;
    at_most(what, "bytes written for the page", cost.points.as_ref().map_or(0, |(_, written)| *written) as u64, 4 * MEGABYTE as u64)?;

    // The same with 100,000 buys is 213 million limbs to keep: refused when the figure holds what it may.
    let what = "600 priced sales and buys, then 100000 buys";
    let cost = history(what, &|c| pairs(c, 600, true).and_then(|()| then_buys(c, 100_000)))?;
    if !matches!(&cost.points, Err(FiguresError::NotComputed(why)) if why == OVER_BUDGET) { return Err(format!("{what}: {:?}", cost.points)); }
    at_most(what, "limbs held", cost.used.held, FIGURE.held)?;
    at_most(what, "bytes allocated at one time", cost.peak as u64, 320 * MEGABYTE as u64)?;
    Ok(9)
}

pub fn run() -> Result<usize, String> {
    let mut checks = histories()?;
    let mut within = |limit: Duration, what: &str, verdict: &dyn Fn() -> Result<bool, String>| -> Result<(), String> {
        let started = Instant::now();
        let held = verdict()?;
        let took = started.elapsed();
        if !held { return Err(format!("{what}: not so")); }
        if took > limit { return Err(format!("{what}: only after {took:?}")); }
        checks += 1;
        Ok(())
    };
    let mut check = |what: &str, verdict: &dyn Fn() -> Result<bool, String>| within(QUICKLY, what, verdict);
    let ten_megabytes = "9".repeat(10_000_000);
    let small = format!("0.{}1", "0".repeat(10_000_000));

    // A number on its way in, by every door: a venue's or a provider's string, a JSON number, a column.
    for text in ["1e-1000000000", "1e1000000000", "-1e1000000000", "1e99999999999999999999999", "1e-99999999999999999999999", ten_megabytes.as_str(), small.as_str()] {
        let short = &text[..text.len().min(30)];
        check(&format!("the text {short}"), &|| Ok(Dec::strict(text) == Err(NumError::OutOfRange)))?;
        check(&format!("a JSON string {short}"), &|| Ok(Dec::to_d(&json!(text)) == Err(NumError::OutOfRange)))?;
        check(&format!("a text column {short}"), &|| Ok(Dec::from_sql(rusqlite::types::ValueRef::Text(text.as_bytes())) == Err(NumError::OutOfRange)))?;
        check(&format!("a split of {short}:1"), &|| Ok(splits::factor(&json!({ "split_ratio": format!("{text}:1") })).is_err() || !text.bytes().all(|b| b.is_ascii_digit())))?;
    }
    check("the text 1e401, and 1e400 is read", &|| Ok(Dec::strict("1e401") == Err(NumError::OutOfRange) && Dec::strict("1e400").is_ok() && Dec::strict("1e-400").is_ok() && Dec::strict("1e-401").is_err()))?;
    let nines = "9".repeat(250);
    check("600 digits written out, and 500 are read", &|| Ok(Dec::strict(&format!("{nines}e-600")) == Err(NumError::OutOfRange) && Dec::strict(&format!("{nines}e-500")).is_ok()))?;
    check("a first digit at 10^401, and at 10^400 it is read", &|| Ok(Dec::strict(&format!("{nines}e152")) == Err(NumError::OutOfRange) && Dec::strict(&format!("{nines}e151")).is_ok()))?;
    check("a text of 257 characters, and 256 are read", &|| Ok(Dec::strict(&"9".repeat(257)) == Err(NumError::OutOfRange) && Dec::strict(&"9".repeat(256)).is_ok()))?;
    check("every finite Float of a column is read", &|| Ok(Dec::from_sql(rusqlite::types::ValueRef::Real(f64::MAX)).is_ok() && Dec::from_sql(rusqlite::types::ValueRef::Real(5e-324)).is_ok()))?;
    // A venue's or a provider's number has tighter limits: its first digit within 10^±40, 64 significant digits.
    check("a venue's 1e41, and 1e40 is read", &|| Ok(Dec::to_d(&json!("1e41")) == Err(NumError::OutOfRange) && Dec::to_d(&json!("1e40")).is_ok() && Dec::to_d(&json!(1e41)) == Err(NumError::OutOfRange)))?;
    check("a venue's 1e-41, and 1e-40 is read", &|| Ok(Dec::to_d(&json!("1e-41")) == Err(NumError::OutOfRange) && Dec::to_d(&json!("1e-40")).is_ok() && Dec::to_d(&json!(1e-41)) == Err(NumError::OutOfRange)))?;
    check("a venue's 65 digits, and 64 are read", &|| Ok(Dec::to_d(&json!("1".repeat(65))) == Err(NumError::OutOfRange) && Dec::to_d(&json!(format!("0.{}", "1".repeat(64)))).is_ok() && Dec::to_d(&json!("12300000000000000000000000000000000000000")).is_ok()))?;
    check("a provider's rate of 1e41 or 1e-41", &|| Ok(Num::from_json(&json!(1e41)) == Err(NumError::OutOfRange) && Num::from_json(&json!(1e-41)) == Err(NumError::OutOfRange) && Num::from_json(&json!(0.0)).is_ok_and(|n| n.is_some())))?;
    check("a real column of Infinity", &|| Ok(Dec::from_sql(rusqlite::types::ValueRef::Real(f64::INFINITY)) == Err(NumError::NotANumber)))?;
    for integer in [json!(9_223_372_036_854_775_808u64), json!(u64::MAX), json!(1.8446744073709552e19), json!(-1e19)] {
        check(&format!("the JSON number {integer}"), &|| Ok(Num::from_json(&integer) == Err(NumError::OutOfRange)))?;
    }

    // Compact numbers with growing gaps pay for the zeros before they are printed. No per-number cap.
    let (far, near) = (Dec::strict("1e400").map_err(|e| format!("{e:?}"))?, Dec::strict("1e-400").map_err(|e| format!("{e:?}"))?);
    let chain = |next: &dyn Fn(&Dec) -> Result<Dec, NumError>| -> Result<bool, String> {
        let limits = Limits { steps: 200_000, held: 200_000 };
        let (outcome, used) = budget::scope(limits, || {
            let mut x = Dec::one();
            for step in 0..64 {
                match next(&x) {
                    Ok(y) => x = y,
                    Err(error) => return error == NumError::OverBudget && step > 0,
                }
            }
            false
        });
        Ok(outcome && used.steps <= limits.steps)
    };
    check("1e400 multiplied by itself until its zeros exhaust the budget", &|| chain(&|x| x * &far))?;
    check("1e-400 multiplied by itself", &|| chain(&|x| x * &near))?;
    check("a quotient ever further below the point", &|| chain(&|x| x.div(&far)))?;
    check("a sum of two numbers 800 digits apart is kept", &|| Ok((&far + &near).is_ok_and(|sum| sum.to_s_f().len() == 802)))?;
    check("rounding to a billion places", &|| Ok(Dec::one().round(1_000_000_000) == Err(NumError::OutOfRange) && Dec::one().round(MAX_SCALE).is_ok()))?;
    check("rounding to places below zero", &|| Ok(Num::Dec(Dec::one()).round(-1) == Err(NumError::OutOfRange)))?;
    check("an Integer beyond 64 bits", &|| Ok(Num::Int(i64::MAX).add(&Num::Int(1)) == Err(NumError::OutOfRange)))?;

    // By its digits: no chain of operations takes more steps or holds more limbs than its scope may. Small limits
    // here, so that running out takes a moment; a figure's own are met by the histories above.
    let small = Limits { steps: 2_000_000, held: 200_000 };
    check("squaring until the steps run out", &|| {
        let (outcome, used) = budget::scope(small, || {
            let mut x = Dec::strict("1.0000001")?;
            for step in 0..64 { match &x * &x { Ok(square) => x = square, Err(error) => return Ok((error, step)) } }
            Err(NumError::NotANumber)
        });
        Ok(matches!(outcome, Ok((NumError::OverBudget, step)) if step > 8) && used.steps <= small.steps)
    })?;
    check("keeping every sum until the limbs run out", &|| {
        let (outcome, used) = budget::scope(small, || {
            let long = Dec::parse(&"7".repeat(9_000))?;
            let (mut sum, mut kept) = (Dec::zero(), vec![]);
            for step in 0..10_000 { match &sum + &long { Ok(next) => { kept.push(next.clone()); sum = next; } Err(error) => return Ok((error, step)) } }
            Err(NumError::NotANumber)
        });
        Ok(matches!(outcome, Ok((NumError::OverBudget, step)) if step > 100) && used.held <= small.held)
    })?;
    check("what a scope kept is no longer held when it is dropped", &|| Ok(budget::scope(small, || (&Dec::one() + &Dec::one()).is_ok()) == (true, Used { steps: 2, held: 3 })))?;
    let long = Dec::parse(&"7".repeat(360_000)).map_err(|e| format!("{e:?}"))?;
    check("outside any scope, a product beyond a figure's steps is refused before it is made", &|| Ok((&long * &long) == Err(NumError::OverBudget)))?;

    // The same through the library, for one bot: every figure that would need the number is "not computed".
    let refused = |c: &Connection, script: &Value, reason: &str| -> Result<bool, String> {
        match figures(c, script, At(1_773_000_000_000_000_000)) { Err(FiguresError::NotComputed(why)) => Ok(why == reason), other => Err(format!("{other:?}")) }
    };
    let plain = install("100", None, 2)?;
    check("an install with nothing hostile in it is computed", &|| figures(&plain, &fine(), At(1_773_000_000_000_000_000)).map(|(points, _)| points == 2).map_err(|e| format!("{e:?}")))?;
    for hostile in ["1e1000000000", "1e-1000000000", ten_megabytes.as_str()] {
        let short = &hostile[..hostile.len().min(30)];
        check(&format!("a price of {short}"), &|| refused(&plain, &script(json!(hostile), json!(100.0), json!(0.9993), json!(64000.5)), OUT_OF_RANGE))?;
        check(&format!("a candle of {short}"), &|| refused(&plain, &script(json!(104.52), json!(hostile), json!(0.9993), json!(64000.5)), OUT_OF_RANGE))?;
        check(&format!("a coin price of {short}"), &|| refused(&plain, &script(json!(104.52), json!(100.0), json!(hostile), json!(64000.5)), OUT_OF_RANGE))?;
    }
    // A member of an answer that is no number, and that no figure of this bot uses, changes nothing.
    let with = |path: &str, member: &str, value: Value| -> Value {
        let mut script = fine();
        if let Some(members) = script.pointer_mut(path).and_then(Value::as_object_mut) { members.insert(member.to_string(), value); }
        script
    };
    let usual = figures(&plain, &fine(), At(1_773_000_000_000_000_000)).map_err(|e| format!("{e:?}"))?;
    for hostile in ["NaN", "Infinity", "1e1000000000", "12abc"] {
        let price = with("/GET data.alpaca.markets~1v2~1stocks~1snapshots/body", "ZZZ", json!({ "latestTrade": { "p": hostile } }));
        check(&format!("a price of {hostile} for a ticker the bot does not hold"), &|| figures(&plain, &price, At(1_773_000_000_000_000_000)).map(|figures| figures == usual).map_err(|e| format!("{e:?}")))?;
        let rate = with("/GET data-api:3000~1api~1v1~1exchange_rates/body/data", "xau", json!({ "value": hostile }));
        check(&format!("a rate of {hostile} for a currency nobody asked about"), &|| figures(&plain, &rate, At(1_773_000_000_000_000_000)).map(|figures| figures == usual).map_err(|e| format!("{e:?}")))?;
    }
    check("a rate of NaN for the currency that was asked about", &|| refused(&plain, &with("/GET data-api:3000~1api~1v1~1exchange_rates/body/data", "eur", json!({ "value": "NaN" })), NOT_A_NUMBER))?;
    check("a rate beyond 64 bits", &|| refused(&plain, &script(json!(104.52), json!(100.0), json!(0.9993), json!(9_223_372_036_854_775_809u64)), OUT_OF_RANGE))?;
    check("a price of Infinity", &|| refused(&plain, &script(json!("Infinity"), json!(100.0), json!(0.9993), json!(64000.5)), NOT_A_NUMBER))?;
    let split = install("100", Some(&format!("{ten_megabytes}:1")), 2)?;
    check("a split ratio of ten megabytes", &|| refused(&split, &fine(), OUT_OF_RANGE))?;
    let places = install("100", None, 1_000_000_000)?;
    check("a ticker with a billion quote decimals", &|| refused(&places, &fine(), OUT_OF_RANGE))?;
    // A decimal column has NUMERIC affinity: SQLite stores a numeric text as a double, so 1e1000000000 arrives as Infinity.
    let stored = install("'1e1000000000'", None, 2)?;
    check("a price column holding 1e1000000000", &|| refused(&stored, &fine(), NOT_A_NUMBER))?;
    let vanished = install("'1e-1000000000'", None, 2)?; // and this one as the integer 0: a row with no price to speak of
    check("a price column holding 1e-1000000000", &|| figures(&vanished, &fine(), At(1_773_000_000_000_000_000)).map(|_| true).map_err(|e| format!("{e:?}")))?;
    Ok(checks)
}

#[allow(dead_code)] // rust/tests/figures_limits.rs includes this file for `run`
fn main() {
    match run() {
        Ok(checks) => println!("every hostile number was refused and every history kept to its budget ({checks} checks)"),
        Err(what) => { eprintln!("{what}"); std::process::exit(1); }
    }
}
