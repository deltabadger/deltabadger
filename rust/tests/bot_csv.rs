//! 3.0 plan A8 (PLAN-A8-bot-csv r4): what 2.x's recording cannot show. The pages grid (bot_csv_*) holds the export's bytes,
//! the import's rows and its flashes to 2.x; here: a lossless round trip, the import's lock and refusals, and one test per
//! named divergence and restriction.
mod common;
use common::seed::{self, BotSpec};
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::App;
use serde_json::json;

const T0: &str = "2026-09-01T12:30:00Z";
const HEADER: &str = "Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status";

struct Install { d: tempfile::TempDir, o: deltabadger::store::Opened, s: seed::Seeded, a: i64, b: i64 }

/// Two stopped one-asset (BTC/USD) Alpaca baskets of one owner, who signs in.
fn install() -> Install {
    let (d, o, s) = common::install_alpaca();
    let bot = || seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    let (a, b) = (bot(), bot());
    o.primary.execute("UPDATE users SET encrypted_password=?1, confirmed_at='2026-01-01', wash_sale_enabled=0, time_zone='Warsaw'",
                      [deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    Install { d, o, s, a, b }
}

async fn signed_in(i: &Install) -> (App, Browser) {
    let app = web::app(i.d.path(), "engine-test-secret", TestClock::at(T0));
    app.attach_engine(Default::default());
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(browser.get(&app, "/settings/account").await.status, 200);
    (app, browser)
}

async fn export(app: &App, browser: &mut Browser, bot: i64) -> String {
    let answer = browser.send(app, "POST", &format!("/bots/{bot}/export"), Some(&[]), Csrf::Header, &[]).await;
    assert_eq!((answer.status, answer.header("content-type")), (200, Some("text/csv")));
    answer.body
}

/// Posts `bytes` as the import form's file and returns the flash the next page shows.
async fn import(app: &App, browser: &mut Browser, bot: i64, bytes: &[u8]) -> String {
    let (content_type, body) = web::multipart(&[], &[("file", "orders.csv", bytes)]);
    let answer = browser.send_bytes(app, "POST", &format!("/bots/{bot}/import"), Some(body), Csrf::Header, &[("content-type", content_type.as_str())]).await;
    assert_eq!((answer.status, answer.header("location")), (302, Some(format!("/bots/{bot}").as_str())), "{}", answer.body);
    let page = browser.get(app, "/settings/account").await;
    let flash = page.body.split("id=\"flash\"").nth(1).unwrap_or_default();
    let text: String = flash.split("</div>").next().unwrap_or_default().split('<').map(|part| part.split_once('>').map_or(part, |(_, t)| t)).collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ").replace("&#39;", "'");
    assert_eq!(browser.get(app, "/settings/account").await.status, 200);
    text
}

fn csv(rows: &[&str]) -> Vec<u8> { format!("{HEADER}\n{}\n", rows.join("\n")).into_bytes() }
fn ids(i: &Install, bot: i64) -> Vec<String> {
    i.o.primary.prepare("SELECT coalesce(external_id, '') FROM transactions WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([bot], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
fn count(i: &Install) -> i64 { i.o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap() }
/// (side, amount, value, price) of a closed BTC order.
fn closed(i: &Install, bot: i64, ext: &str, at: &str, (side, amount, value, price): (i64, &str, &str, &str)) {
    seed::insert_row(&i.o.primary, &i.s, bot, i.s.btc, &json!({ "external_status": 2, "external_id": ext, "side": side, "amount": amount, "amount_exec": amount,
        "quote_amount": value, "quote_amount_exec": value, "price": price, "created_at": at }));
}

#[tokio::test(flavor = "current_thread")]
async fn an_export_imports_into_another_bot_and_exports_again_cell_for_cell() {
    let i = install();
    closed(&i, i.a, "=1+1", "2026-08-01 10:00:00", (0, "0.001", "60", "60000"));
    closed(&i, i.a, "plain", "2026-08-02 10:00:00.250000", (1, "0.0005", "31.5", "63000"));
    closed(&i, i.a, "with, comma", "2026-12-24 23:30:00", (0, "0.002", "100", "50000"));
    // A merged row placed on another venue keeps its venue in the id.
    i.o.primary.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '2026-01-01', '2026-01-01')", []).unwrap();
    let kraken = i.o.primary.last_insert_rowid();
    closed(&i, i.a, "K-1", "2026-08-03 10:00:00", (0, "0.001", "61", "61000"));
    i.o.primary.execute("UPDATE transactions SET exchange_id = ?1 WHERE external_id = 'K-1'", [kraken]).unwrap();
    let (app, mut browser) = signed_in(&i).await;
    let first = export(&app, &mut browser, i.a).await;
    assert_eq!(import(&app, &mut browser, i.b, first.as_bytes()).await, "Successfully imported 4 order(s).");
    let second = export(&app, &mut browser, i.b).await;
    use deltabadger::tracker::import::csv::Table;
    let (a, b) = (Table::parse(&first, |_| true).unwrap(), Table::parse(&second, |_| true).unwrap());
    let headers: Vec<String> = a.headers().map(str::to_string).collect();
    assert_eq!((headers.clone(), a.records().count()), (b.headers().map(str::to_string).collect::<Vec<_>>(), 4));
    for (x, y) in a.records().zip(b.records()) {
        for (k, (p, q)) in headers.iter().map(|h| (a.get(&x, h), b.get(&y, h))).enumerate() {
            if k == 1 {
                // The Order ID is the destination's import of the source's id, escaped as any text cell.
                let source = p.unwrap().strip_prefix('\'').filter(|r| r.starts_with('=')).unwrap_or(p.unwrap());
                assert_eq!(q, Some(format!("imported_{}_{source}", i.b).as_str()));
            } else { assert_eq!(p, q, "column {k}"); }
        }
    }
    assert!(first.contains("2026-08-01 12:00:00 +0200,'=1+1,") && first.contains("2026-12-25 00:30:00 +0100,\"with, comma\","), "{first}");
    assert!(first.contains(",Kraken:K-1,"), "{first}");
    // D-IMPORT-REIMPORT: B's own export back into B adds nothing.
    let before = count(&i);
    assert_eq!(import(&app, &mut browser, i.b, second.as_bytes()).await, "No new orders to import (all orders already exist or currencies do not match).");
    assert_eq!(count(&i), before);
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_is_refused_while_the_engine_holds_the_bot_or_an_operation_runs() {
    let file = csv(&["2026-08-01 10:00:00 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    for (column, value) in [("status", json!(deltabadger::enums::BotStatus::Executing as i64)),
                            ("transient_data", json!({"rust_placement": {"cl_ord_id": "c-1", "deadline": "2026-09-01T12:31:00Z"}}).to_string().into()),
                            ("transient_data", json!({"rebalance_pending": {"phase": "selling"}}).to_string().into())] {
        let i = install();
        i.o.primary.execute(&format!("UPDATE bots SET {column} = ?1 WHERE id = ?2"), rusqlite::params![value.as_i64().map_or_else(|| rusqlite::types::Value::Text(value.as_str().unwrap().into()), rusqlite::types::Value::Integer), i.a]).unwrap();
        let (app, mut browser) = signed_in(&i).await;
        let text = import(&app, &mut browser, i.a, &file).await;
        assert!(text.contains("is placing an order or running an operation"), "{column}: {text}");
        assert_eq!((count(&i), app.figure_service.marked()), (0, 0), "{column}: nothing written, nothing published");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_waits_for_an_engine_write_and_both_land() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let engine = rusqlite::Connection::open(i.d.path().join("production.sqlite3")).unwrap();
    engine.execute_batch("BEGIN IMMEDIATE").unwrap();
    seed::insert_row(&engine, &i.s, i.a, i.s.btc, &json!({ "external_status": 2, "external_id": "ENGINE-1", "amount": "0.001", "amount_exec": "0.001",
        "quote_amount": "60", "quote_amount_exec": "60", "price": "60000", "created_at": "2026-09-01 12:00:00" }));
    let (bot, inner) = (i.a, app.clone());
    let file = csv(&["2026-08-01 10:00:00 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    let task = tokio::spawn(async move { import(&inner, &mut browser, bot, &file).await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!task.is_finished(), "the import waits for the engine's transaction");
    engine.execute_batch("COMMIT").unwrap();
    assert_eq!(task.await.unwrap(), "Successfully imported 1 order(s).");
    assert_eq!(ids(&i, i.a), vec!["ENGINE-1".to_string(), format!("imported_{}_x", i.a)]);
    assert_eq!(app.figure_service.marked(), 1, "the figures hear the committed import once");
    // D-IMPORT-BUSY's fence: an operation leg sized before this history changed is not sent (tests/rebalance.rs).
    assert_eq!(deltabadger::engine::model::load_bot(&i.o.primary, i.a).unwrap().history_generation(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn reimported_ids_never_collapse_two_orders() {
    for rows in [["2026-08-01 10:00:00 UTC,abc,Market,Buy,0.001,60,60000,BTC,USD,Closed", "2026-08-02 10:00:00 UTC,imported_1_abc,Market,Buy,0.002,120,60000,BTC,USD,Closed"],
                 ["2026-08-02 10:00:00 UTC,imported_1_abc,Market,Buy,0.002,120,60000,BTC,USD,Closed", "2026-08-01 10:00:00 UTC,abc,Market,Buy,0.001,60,60000,BTC,USD,Closed"]] {
        let i = install();
        assert_eq!(i.a, 1);
        let (app, mut browser) = signed_in(&i).await;
        assert_eq!(import(&app, &mut browser, 1, &csv(&rows)).await, "Successfully imported 2 order(s).");
        let mut stored = ids(&i, 1);
        stored.sort();
        assert_eq!(stored, ["imported_1_abc", "imported_1_imported_1_abc"]);
        // Held already: each of the two is that row.
        assert_eq!(import(&app, &mut browser, 1, &csv(&rows)).await, "No new orders to import (all orders already exist or currencies do not match).");
        assert_eq!(count(&i), 2);
    }
    // Held before this file: `abc` maps onto the held id and `imported_1_abc` is that row.
    let i = install();
    closed(&i, 1, "imported_1_abc", "2026-08-01 10:00:00", (0, "0.001", "60", "60000"));
    let (app, mut browser) = signed_in(&i).await;
    let rows = ["2026-08-01 10:00:00 UTC,abc,Market,Buy,0.001,60,60000,BTC,USD,Closed", "2026-08-01 10:00:00 UTC,imported_1_abc,Market,Buy,0.001,60,60000,BTC,USD,Closed"];
    assert_eq!(import(&app, &mut browser, 1, &csv(&rows)).await, "No new orders to import (all orders already exist or currencies do not match).");
    assert_eq!(count(&i), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_id_another_bot_holds_on_the_venue_is_skipped_as_existing() {
    let i = install();
    closed(&i, i.b, &format!("imported_{}_q", i.a), "2026-08-01 10:00:00", (0, "0.001", "60", "60000"));
    let (app, mut browser) = signed_in(&i).await;
    let file = csv(&["2026-08-01 10:00:00 UTC,q,Market,Buy,0.001,60,60000,BTC,USD,Closed", "2026-08-01 10:00:00 UTC,r,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    assert_eq!(import(&app, &mut browser, i.a, &file).await, "Successfully imported 1 order(s).");
    assert_eq!(ids(&i, i.a), [format!("imported_{}_r", i.a)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_row_v2_would_insert_but_no_reader_can_read_refuses_the_whole_file() {
    let good = "2026-08-01 10:00:00 UTC,ok,Market,Buy,0.001,60,60000,BTC,USD,Closed";
    for bad in ["yesterday,x,Market,Buy,0.001,60,60000,BTC,USD,Closed", "2026-08-01,x,Stop,Buy,0.001,60,60000,BTC,USD,Closed",
                "2026-08-01,x,Market,Hold,0.001,60,60000,BTC,USD,Closed", "2026-08-01,x,Market,Buy,12abc,60,60000,BTC,USD,Closed",
                "2026-08-01,x,Market,Buy,-1,60,60000,BTC,USD,Closed", "2026-08-01,x,Market,Buy,0,5,60000,BTC,USD,Closed",
                "2026-08-01,x,Market,Buy,,60,60000,BTC,USD,Closed",
                "1969-12-31 23:59:59 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed", "2200-01-01 00:00:00 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed"] {
        let i = install();
        let (app, mut browser) = signed_in(&i).await;
        assert_eq!(import(&app, &mut browser, i.a, &csv(&[good, bad])).await, "The CSV file is malformed and cannot be parsed.", "{bad}");
        assert_eq!((count(&i), app.figure_service.marked()), (0, 0), "{bad}");
    }
    // The same junk in a row 2.x passes over (not closed) is passed over.
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let ignored = "yesterday,x,Stop,Hold,12abc,60,60000,BTC,USD,Open";
    assert!(import(&app, &mut browser, i.a, &csv(&[good, ignored])).await.starts_with("Successfully imported 1 order(s). Skipped 1 row(s)"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_first_accepted_second_imports_and_every_reader_computes() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let file = csv(&["1970-01-01 00:00:00 UTC,epoch,Market,Buy,0.001,60,60000,BTC,USD,Closed", "2026-08-01 10:00:00 +0200,later,Limit,Sell,0.0005,31,62000,btc,usd,closed"]);
    assert_eq!(import(&app, &mut browser, i.a, &file).await, "Successfully imported 2 order(s).");
    let c = &i.o.primary;
    assert_eq!(c.query_row("SELECT created_at FROM transactions WHERE external_id LIKE '%epoch'", [], |r| r.get::<_, String>(0)).unwrap(), "1970-01-01 00:00:00");
    assert_eq!(c.query_row("SELECT created_at FROM transactions WHERE external_id LIKE '%later'", [], |r| r.get::<_, String>(0)).unwrap(), "2026-08-01 08:00:00");
    let orders = deltabadger::figures::db::orders(c, i.a).unwrap();
    assert!(orders.iter().all(|o| deltabadger::figures::fill::for_engine(o).unwrap().is_some()));
    // The chart's first bar is the epoch's own day: a timeframe back from it is still inside the figures' range.
    let bars = json!({"bars": {"BTC/USD": [{"t": "1970-01-01T00:00:00Z", "o": 60000, "h": 60000, "l": 60000, "c": 60000, "v": 1},
                                           {"t": "2026-09-01T00:00:00Z", "o": 61000, "h": 61000, "l": 61000, "c": 61000, "v": 1}]}});
    let market = deltabadger::figures::scripted::Scripted::new(&json!({
        "GET data.alpaca.markets/v1beta3/crypto/us/latest/trades": {"body": {"trades": {"BTC/USD": {"p": 61000.0}}}},
        "GET data.alpaca.markets/v1beta3/crypto/us/bars?symbols=BTC/USD": {"body": bars}}), None);
    let now = deltabadger::figures::at::At::from_sql("2026-09-01 12:30:00").unwrap();
    deltabadger::web::figure::metrics_panel(c, i.a, &market, now, "en", "").map_err(|e| format!("{e:?}")).unwrap();
    let bot = deltabadger::engine::model::load_bot(c, i.a).unwrap();
    deltabadger::engine::eligibility::bot_reasons(c, &bot).map(|r| assert!(r.is_empty(), "{r:?}")).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn the_mismatch_names_a_mismatched_quote_and_an_unknown_base_is_counted() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    // 2.x names the first row's raw cell (" usd ", a row that only named another asset).
    let file = csv(&["2026-08-01,x,Market,Buy,0.001,60,60000,ETH, usd ,Closed", "2026-08-01,y,Market,Buy,0.001,60,60000,BTC, eur ,Closed"]);
    assert_eq!(import(&app, &mut browser, i.a, &file).await, "Currency mismatch: CSV has 'EUR' but bot uses 'USD' Skipped 2 row(s): not closed, not this bot's assets or quote, or an asset name that fits more than one.");
    let file = csv(&["2026-08-01,x,Market,Buy,0.001,60,60000,ETH,USD,Closed"]);
    assert!(import(&app, &mut browser, i.a, &file).await.starts_with("No new orders to import (all orders already exist or currencies do not match). Skipped 1 row(s)"));
    let file = csv(&["2026-08-01,x,Market,Buy,0.001,60,60000,ETH,USD,Closed", "2026-08-01,z,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    assert!(import(&app, &mut browser, i.a, &file).await.starts_with("Successfully imported 1 order(s). Skipped 1 row(s)"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_file_is_read_as_v2_reads_it_and_bounded() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend(csv(&["2026-08-01,x,Market,Buy,0.001,60,60000,BTC,USD,Closed"]));
    assert_eq!(import(&app, &mut browser, i.a, &bom).await, "Invalid CSV format. Please use a file exported from this application.");
    let mut invalid = csv(&["2026-08-01,x,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    invalid.extend(b"\xff");
    assert_eq!(import(&app, &mut browser, i.a, &invalid).await, "The CSV file is malformed and cannot be parsed.");
    // D-IMPORT-SIZE: past the tracker's bound the body is refused before any handler.
    let huge = vec![b'a'; deltabadger::web::IMPORT_LIMIT + 1];
    let (content_type, body) = web::multipart(&[], &[("file", "orders.csv", &huge)]);
    let answer = browser.send_bytes(&app, "POST", &format!("/bots/{}/import", i.a), Some(body), Csrf::Header, &[("content-type", content_type.as_str())]).await;
    assert_eq!(answer.status, 413);
    assert_eq!(count(&i), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_export_reads_the_rows_as_they_stand() {
    let i = install();
    closed(&i, i.a, "done", "2026-08-01 10:00:00", (0, "0.001", "60", "60000"));
    // D-EXPORT-NO-REFRESH: an open order stays out until the engine's poll closes it; nothing asks the venue here.
    seed::insert_row(&i.o.primary, &i.s, i.a, i.s.btc, &json!({ "external_status": 1, "external_id": "OPEN-1", "order_type": 1, "amount": "0.001",
        "price": "60000", "amount_exec": "0", "quote_amount_exec": "0", "created_at": "2026-08-02 10:00:00" }));
    // D-EXPORT-NIL-CELL: a closed row without a type or side (2.x's import of an unknown Type writes one) is empty cells.
    closed(&i, i.a, "", "2026-08-04 10:00:00", (0, "0.001", "60", "60000"));
    i.o.primary.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '2026-01-01', '2026-01-01')", []).unwrap();
    i.o.primary.execute("UPDATE transactions SET external_id = NULL, exchange_id = (SELECT max(id) FROM exchanges) WHERE external_id = ''", []).unwrap();
    closed(&i, i.a, "untyped", "2026-08-03 10:00:00", (0, "0.001", "60", "60000"));
    i.o.primary.execute("UPDATE transactions SET order_type = NULL, side = NULL WHERE external_id = 'untyped'", []).unwrap();
    let (app, mut browser) = signed_in(&i).await;
    let before: Vec<(i64, String)> = i.o.primary.prepare("SELECT external_status, updated_at FROM transactions ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(export(&app, &mut browser, i.a).await, format!("{HEADER}\n2026-08-01 12:00:00 +0200,done,Market,Buy,0.001,60.0,60000.0,BTC,USD,Closed\n\
                                                               2026-08-03 12:00:00 +0200,untyped,,,0.001,60.0,60000.0,BTC,USD,Closed\n\
                                                               2026-08-04 12:00:00 +0200,Kraken:,Market,Buy,0.001,60.0,60000.0,BTC,USD,Closed\n"));
    let after: Vec<(i64, String)> = i.o.primary.prepare("SELECT external_status, updated_at FROM transactions ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(before, after);
}

/// #710 (Codex r1 P2): an import reads what the bot already holds on its current account only. Order X imported on
/// account 1, the bot moved to account 2, account 2's own X imports as its own order; again, it is a duplicate.
#[tokio::test(flavor = "current_thread")]
async fn an_import_dedups_against_the_bots_current_account_only() {
    let i = install();
    let c = &i.o.primary;
    let now: chrono::DateTime<chrono::Utc> = T0.parse().unwrap();
    deltabadger::identity::Account::first(c, i.s.user_id, i.s.exchange_id).unwrap().ensure(c, now).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [i.s.user_id, i.s.exchange_id]).unwrap();
    let second = deltabadger::identity::Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    let (app, mut browser) = signed_in(&i).await;
    let file = |amount: &str| csv(&[&format!("2026-08-01 10:00:00 UTC,X,Market,Buy,{amount},60,60000,BTC,USD,Closed")]);
    assert_eq!(import(&app, &mut browser, i.a, &file("0.001")).await, "Successfully imported 1 order(s).");
    deltabadger::identity::place_bot(c, i.a, i.s.user_id, i.s.exchange_id, Some(second), now).unwrap();
    assert_eq!(import(&app, &mut browser, i.a, &file("0.002")).await, "Successfully imported 1 order(s).", "account 2's X is its own order");
    assert_eq!(import(&app, &mut browser, i.a, &file("0.002")).await, "No new orders to import (all orders already exist or currencies do not match).");
    let accounts: Vec<Option<i64>> = c.prepare("SELECT (SELECT m.account_id FROM exchange_account_transactions m WHERE m.transaction_id = t.id) FROM transactions t WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([i.a], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(accounts, [None, second.id]);
}

/// An order the bot holds on the exchange it traded on before is another venue order: the same id on its current exchange
/// imports.
#[tokio::test]
async fn an_import_dedups_against_the_bots_current_exchange_only() {
    let i = install();
    let c = &i.o.primary;
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Other710', 'Other', '0.1', '0.1', 'now', 'now')", []).unwrap();
    let other = c.last_insert_rowid();
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, created_at, updated_at) SELECT id, ?2, 'imported_' || id || '_X', 1, '2026-07-01 10:00:00', '2026-07-01 10:00:00' FROM bots WHERE id = ?1",
              [i.a, other]).unwrap();
    let (app, mut browser) = signed_in(&i).await;
    let file = csv(&["2026-08-01 10:00:00 UTC,X,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    assert_eq!(import(&app, &mut browser, i.a, &file).await, "Successfully imported 1 order(s).", "the other exchange's X is another order");
}

/// PLAN-710-importmem A7/B2′: the streamed reader answers as the whole-table one did. A syntax error anywhere is malformed,
/// before the header is judged (an extra column then a broken quote); an extra or reordered header is the invalid format;
/// the first of two same-named headers is the column; a blank record after valid headers counts as skipped.
#[tokio::test(flavor = "current_thread")]
async fn the_streamed_reader_answers_as_the_whole_table_did() {
    let i = install();
    let (app, mut browser) = signed_in(&i).await;
    let row = "2026-08-01 10:00:00 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed";
    let extra = format!("{HEADER},Note\n{row},n\n{row},\"broken\n").into_bytes();
    assert_eq!(import(&app, &mut browser, i.a, &extra).await, "The CSV file is malformed and cannot be parsed.");
    let discarded = format!("{HEADER}\n{row}\n{row},x\"y\n").into_bytes();
    assert_eq!(import(&app, &mut browser, i.a, &discarded).await, "The CSV file is malformed and cannot be parsed.", "a broken quote in a column no reader keeps");
    assert_eq!(import(&app, &mut browser, i.a, format!("{HEADER},Note\n{row},n\n").as_bytes()).await, "Invalid CSV format. Please use a file exported from this application.");
    let reordered = HEADER.replace("Type,Side", "Side,Type");
    assert_eq!(import(&app, &mut browser, i.a, format!("{reordered}\n{row}\n").as_bytes()).await, "Invalid CSV format. Please use a file exported from this application.");
    assert_eq!(count(&i), 0);
    let blank = import(&app, &mut browser, i.a, format!("{HEADER}\n{row}\n\n").as_bytes()).await;
    assert!(blank.starts_with("Successfully imported 1 order(s). Skipped 1 row(s)"), "{blank}");
    assert_eq!(count(&i), 1);
}

/// PLAN index-as-ingredient slice 4a: a basket stored as one index group reads any of the venue's assets by name, as the index
/// bot it was converted from (`members` None), not an empty allowlist: an exited holding's row imports.
#[tokio::test(flavor = "current_thread")]
async fn a_grouped_bot_imports_an_exited_holding_by_symbol() {
    let i = install();
    let c = &i.o.primary;
    c.execute("UPDATE bots SET settings = json_set(settings, '$.allocations', json('{}'), '$.index_groups', json('[{\"index_type\":\"top\",\"num_coins\":3,\"weight\":1.0}]')) WHERE id = ?1", [i.a]).unwrap();
    c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, exited_at, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 0, '2026-08-20 00:00:00', '2026-08-01', '2026-08-20')",
              (i.a, i.s.btc, i.s.ticker_id)).unwrap();
    let (app, mut browser) = signed_in(&i).await;
    let file = csv(&["2026-08-01,held,Market,Buy,0.001,60,60000,BTC,USD,Closed"]);
    assert_eq!(import(&app, &mut browser, i.a, &file).await, "Successfully imported 1 order(s).");
}
