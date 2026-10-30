//! AUDIT #15, D-BINANCE-ORDER-LINK: Binance fills link to their bot orders, as they are stored and, for stored
//! history, by the pass woken at every start. D-LEDGER-ORDER-OWNER: only the row owner's orders.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::{engine::model, lease, store, sync::{self, activities::{Entry, Raw}, binance_order_links as links, binance_writer as writer}};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn now() -> DateTime<Utc> { "2026-08-18T00:00:00Z".parse().unwrap() }
fn claim(c: &Connection, path: &std::path::Path) -> lease::EngineLock {
    let lock = lease::lock(&store::Paths::from_env(&|_| None, path), now()).unwrap();
    lease::claim(&lock, c, &common::seed::cipher(), "test", now()).unwrap(); lock
}
fn binance(c: &Connection, s: &common::seed::Seeded) -> sync::Key {
    c.execute("UPDATE exchanges SET type='Exchanges::Binance' WHERE id=?1", [s.exchange_id]).unwrap();
    sync::load_key_of(c, s.api_key_id, &["Exchanges::Binance"]).unwrap()
}
fn bot(c: &Connection, s: &common::seed::Seeded) -> i64 { common::seed::insert_bot(c, s, &common::seed::BotSpec::weekly(10.0, "2026-01-01 00:00:00")) }
fn order(c: &Connection, s: &common::seed::Seeded, bot: i64, external: &str, status: i64) -> i64 {
    common::seed::insert_row(c, s, bot, s.btc, &json!({"external_id": external, "status": status, "amount": "0.001", "created_at": "2026-08-17 10:00:00"}))
}
/// A stored ledger row as any writer left it, link NULL.
fn row(c: &Connection, s: &common::seed::Seeded, kind: i64, tx_id: Option<&str>, raw: &Value) -> i64 {
    c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, tx_id, transacted_at, raw_data, \
               manual_values, transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'BTC', 0.001, ?5, '2026-08-17 10:00:00', ?6, '{}', 0, \
               '2026-08-17 10:00:00', '2026-08-17 10:00:00')", params![s.user_id, s.api_key_id, s.exchange_id, kind, tx_id, raw.to_string()]).unwrap();
    c.last_insert_rowid()
}
fn link(c: &Connection, id: i64) -> Option<i64> { c.query_row("SELECT transaction_id FROM account_transactions WHERE id=?1", [id], |r| r.get(0)).unwrap() }
fn fill(order: Value, id: Value) -> Value { json!({"symbol": "BTCUSDT", "orderId": order, "id": id, "isBuyer": true}) }
/// Every column of every ledger row except the two the pass may write.
fn rest(c: &Connection) -> Vec<Vec<String>> {
    let mut s = c.prepare("SELECT * FROM account_transactions ORDER BY id").unwrap();
    let names: Vec<String> = s.column_names().into_iter().map(str::to_string).collect();
    s.query_map([], |r| Ok(names.iter().enumerate().filter(|(_, n)| !["transaction_id", "updated_at"].contains(&n.as_str()))
        .map(|(i, _)| format!("{:?}", r.get_ref(i).unwrap())).collect())).unwrap().collect::<Result<_, _>>().unwrap()
}

#[test]
fn a_fill_names_its_order_only_by_its_whole_id_and_raw_evidence() {
    let ok = |tx: &str, raw: Value| links::fill_order(Some(tx), &raw);
    assert_eq!(ok("BTCUSDT-12-101", fill(json!(12), json!(101))), Some(Some("BTCUSDT-12".into())));
    assert_eq!(ok("BTCUSDT-12-101", fill(json!("12"), json!("101"))), Some(Some("BTCUSDT-12".into())), "number and string agree");
    assert_eq!(ok("ETHBTC-3-7-in", fill(json!(3), json!(7))), Some(Some("ETHBTC-3".into())));
    assert_eq!(ok("ETHBTC-3-7-out", fill(json!(3), json!(7))), Some(Some("ETHBTC-3".into())));
    for (tx, order, id) in [("BTCUSDT-12-101.5", json!(12), json!(101)), ("BTCUSDT-12", json!(12), json!(101)), ("BTCUSDT--101", json!(12), json!(101)),
                            ("BTCUSDT-12-101", json!(13), json!(101)), ("BTCUSDT-12-101", json!(12), json!(102)), ("BTCUSDT-012-101", json!("012"), json!(101)),
                            ("BTCUSDT-12-101", json!(12.0), json!(101)), ("BTCUSDT-12-101", json!(-12), json!(101)), ("BTCUSDT-12-101", json!("+12"), json!(101)),
                            ("btcusdt-12-101", json!(12), json!(101)), ("BTCUSDT-12-101-x", json!(12), json!(101)), ("dust-12-101", json!(12), json!(101))] {
        assert_eq!(ok(tx, fill(order.clone(), id.clone())), Some(None), "{tx} {order} {id}: a fill, no order");
    }
    assert_eq!(links::fill_order(None, &fill(json!(12), json!(101))), Some(None));
    assert_eq!(links::fill_order(Some("BTCUSDT-12"), &json!({"order_id": "BTCUSDT-12"})), None, "no orderId: the generic matcher's row");
}

#[test]
fn new_fills_link_as_stored_and_never_through_the_generic_lookups() {
    let (dir, o, s) = common::install(); let c = &o.primary; let key = binance(c, &s); let _lock = claim(c, dir.path());
    let b = bot(c, &s); let id = order(c, &s, b, "BTCUSDT-12", 0); let csv_order = order(c, &s, b, "BTCUSDT-99", 0);
    let entry = |kind: i64, tx: &str, raw: Value| Entry { entry_type: kind, base_currency: Some("BTC".into()), base_amount: deltabadger::ruby::BigDec::parse("0.001").unwrap(),
        quote_currency: Some("USDT".into()), quote_amount: Some(deltabadger::ruby::BigDec::parse("64").unwrap()), fee_currency: None, fee_amount: None,
        tx_id: Some(tx.into()), group_id: None, description: None, transacted_at: Some("2026-08-17T10:00:00Z".parse().unwrap()), raw: Raw::from_value(raw) };
    let rows = [entry(0, "BTCUSDT-12-101", fill(json!(12), json!(101))), entry(0, "BTCUSDT-12-102", fill(json!("12"), json!("102"))),
                // Malformed fills whose generic lookups would match an order: no link.
                entry(0, "BTCUSDT-12", json!({"orderId": 12, "id": 103})), entry(0, "BTCUSDT-12-103.5", json!({"orderId": 12, "id": 103, "order_id": "BTCUSDT-12"})),
                // A row naming the order directly, without orderId, keeps the generic match.
                entry(1, "BTCUSDT-99", json!({"operation": "Transaction Sold"}))];
    let tx = model::immediate(c).unwrap();
    writer::store_csv(&model::fence_versions(&tx, &[]).unwrap(), &common::seed::cipher(), &key, &rows, now()).unwrap();
    tx.commit().unwrap();
    let by_tx = |t: &str| c.query_row("SELECT transaction_id FROM account_transactions WHERE tx_id=?1", [t], |r| r.get::<_, Option<i64>>(0)).unwrap();
    assert_eq!((by_tx("BTCUSDT-12-101"), by_tx("BTCUSDT-12-102")), (Some(id), Some(id)), "both fills, one order");
    assert_eq!((by_tx("BTCUSDT-12"), by_tx("BTCUSDT-12-103.5")), (None, None));
    assert_eq!(by_tx("BTCUSDT-99"), Some(csv_order));
}

#[test]
fn stored_fills_link_once_and_nothing_else_changes() {
    let (dir, o, s) = common::install(); let c = &o.primary; binance(c, &s); let cipher = common::seed::cipher();
    let b = bot(c, &s); let id = order(c, &s, b, "BTCUSDT-12", 0); let swap = order(c, &s, b, "ETHBTC-3", 0);
    order(c, &s, b, "BTCUSDT-40", 1); // not submitted
    // Another user's order: (exchange_id, external_id) is unique, so the id a fill names may be theirs alone.
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('x@example.com','x','X',0,'2026-01-01','2026-01-01')", []).unwrap();
    let other = common::seed::Seeded { user_id: c.last_insert_rowid(), ..s };
    let theirs = bot(c, &other); order(c, &other, theirs, "BTCUSDT-60", 0);
    let plain = row(c, &s, 0, Some("BTCUSDT-12-101"), &fill(json!(12), json!(101)));
    let (inn, out) = (row(c, &s, 2, Some("ETHBTC-3-7-in"), &json!({"orderId": 3, "id": 7})), row(c, &s, 3, Some("ETHBTC-3-7-out"), &json!({"orderId": 3, "id": 7})));
    let refused = [row(c, &s, 0, Some("BTCUSDT-40-1"), &fill(json!(40), json!(1))), row(c, &s, 0, Some("BTCUSDT-60-1"), &fill(json!(60), json!(1))),
                   row(c, &s, 2, Some("dust-9-in"), &json!({"orderId": 12, "id": 9})), row(c, &s, 2, Some("USDT-in"), &json!({"orderId": 12, "id": 9})),
                   row(c, &s, 4, Some("BTCUSDT-12-104"), &fill(json!(12), json!(104)))]; // a deposit: not a trade entry type
    let kept = row(c, &s, 0, Some("BTCUSDT-12-105"), &fill(json!(12), json!(105)));
    c.execute("UPDATE account_transactions SET transaction_id=?1, manual_values='{\"price\":\"1\"}' WHERE id=?2", params![swap, kept]).unwrap();
    let before = rest(c);
    assert_eq!(links::link_stored(c, &cipher, now()).unwrap(), 0, "without Rust ownership nothing runs");
    let _lock = claim(c, dir.path());
    assert_eq!(links::link_stored(c, &cipher, now()).unwrap(), 3);
    assert_eq!((link(c, plain), link(c, inn), link(c, out)), (Some(id), Some(swap), Some(swap)));
    for r in refused { assert_eq!(link(c, r), None, "row {r}"); }
    assert_eq!(link(c, kept), Some(swap), "an existing link, even a wrong one, is kept");
    assert_eq!(rest(c), before, "only transaction_id and updated_at are written");
    assert_eq!(links::link_stored(c, &cipher, now()).unwrap(), 0, "a second start writes nothing");
    let late = row(c, &s, 1, Some("BTCUSDT-70-1"), &fill(json!(70), json!(1)));
    assert_eq!(links::link_stored(c, &cipher, now()).unwrap(), 0);
    let order70 = order(c, &s, b, "BTCUSDT-70", 0);
    assert_eq!(links::link_stored(c, &cipher, now()).unwrap(), 1, "an order recorded after its fill links at the next start");
    assert_eq!(link(c, late), Some(order70));
}

#[test]
fn rows_that_never_link_do_not_hold_the_pass() {
    let (dir, o, s) = common::install(); let c = &o.primary; binance(c, &s); let _lock = claim(c, dir.path());
    let b = bot(c, &s); let id = order(c, &s, b, "BTCUSDT-12", 0);
    for n in 0..600 { row(c, &s, 0, Some(&format!("BTCUSDT-999-{n}")), &fill(json!(999), json!(n))); }
    let tail: Vec<i64> = (1..=3).map(|n| row(c, &s, 0, Some(&format!("BTCUSDT-12-{n}")), &fill(json!(12), json!(n)))).collect();
    assert_eq!(links::link_stored(c, &common::seed::cipher(), now()).unwrap(), 3);
    for r in tail { assert_eq!(link(c, r), Some(id)); }
    assert_eq!(links::link_stored(c, &common::seed::cipher(), now()).unwrap(), 0);
}

#[test]
fn another_users_order_never_links_on_any_venue() {
    let (dir, o, s) = common::install(); let c = &o.primary; let _lock = claim(c, dir.path()); // Kraken: the generic matcher
    let key = sync::load_key_of(c, s.api_key_id, &["Exchanges::Kraken"]).unwrap();
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('x@example.com','x','X',0,'2026-01-01','2026-01-01')", []).unwrap();
    let other = common::seed::Seeded { user_id: c.last_insert_rowid(), ..s };
    let theirs = bot(c, &other); order(c, &other, theirs, "OXYZ-1", 0);
    let e = Entry { entry_type: 0, base_currency: Some("BTC".into()), base_amount: deltabadger::ruby::BigDec::parse("0.001").unwrap(), quote_currency: None, quote_amount: None,
                    fee_currency: None, fee_amount: None, tx_id: Some("OXYZ-1".into()), group_id: None, description: None,
                    transacted_at: Some("2026-08-17T10:00:00Z".parse().unwrap()), raw: Raw::from_value(json!({})) };
    let tx = model::immediate(c).unwrap();
    sync::ledger::store_file(&model::fence_versions(&tx, &[]).unwrap(), &key, vec![e], now()).unwrap();
    tx.commit().unwrap();
    assert_eq!(c.query_row("SELECT transaction_id FROM account_transactions WHERE tx_id='OXYZ-1'", [], |r| r.get::<_, Option<i64>>(0)).unwrap(), None);
}
