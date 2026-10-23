//! KuCoin's account-history import (plan KI, slices KI1 and KI2): rows stored by their own id, ledger entries derived one UTC
//! second at a time, identity recorded only. Fixtures are KuCoin's own exports, numbers as exported, the UID a constant.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::identity::{self, Standing};
use deltabadger::tracker::import::kucoin::{derive, Catalogue, SourceRow};
use deltabadger::tracker::import::{csv, run};
use rusqlite::Connection;

const KUCOIN: &str = "Exchanges::Kucoin";
const UID: &str = "123456789";
const OTHER: &str = "987654321";
const HEADER: &str = "UID,Account Type,Currency,Side,Amount,Fee,Time(UTC+03:00),Remark,Type";
const TRADING: &str = include_str!("fixtures/kucoin/trading_account.csv");
const FUNDING: &str = include_str!("fixtures/kucoin/funding_account.csv");

fn now() -> DateTime<Utc> { "2026-10-22T10:00:00Z".parse().unwrap() }
/// An export of `rows` ("Currency,Side,Amount,Fee,local time,Type", the account mainAccount, no remark) for `uid`.
fn export(uid: &str, rows: &[&str]) -> String {
    let mut out = vec![HEADER.to_string()];
    for r in rows {
        let f: Vec<&str> = r.split(',').collect();
        out.push(format!("{uid},mainAccount,{},{},{},{},{},,{}", f[0], f[1], f[2], f[3], f[4], f[5]));
    }
    out.join("\n")
}

/// A ledger row: type, currency, amount, quote, quote amount, fee currency, fee, time, tx_id.
type LedgerRow = (i64, String, f64, Option<String>, Option<f64>, Option<String>, Option<f64>, String, String);
/// A derived entry: type, currency, amount, quote amount, fee currency, reason.
type DerivedRow = (i64, String, String, Option<String>, Option<String>, Option<String>);

struct Fx { _dir: tempfile::TempDir, c: Connection, cipher: deltabadger::crypto::Cipher, user: i64, exchange: i64 }

impl Fx {
    fn new() -> Self {
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, 'KuCoin', '0.1', '0.1', '2026-01-01', '2026-01-01')", [KUCOIN]).unwrap();
        let exchange = c.last_insert_rowid();
        let mut assets = vec![];
        for symbol in ["ETH", "BTC", "USDT", "KCS"] {
            c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01', '2026-01-01')",
                      [format!("seed-{symbol}"), symbol.to_string()]).unwrap();
            assets.push(c.last_insert_rowid());
        }
        for (base, quote, b, q, d) in [("ETH", "BTC", 0, 1, 6), ("BTC", "USDT", 1, 2, 1), ("KCS", "USDT", 3, 2, 3)] {
            c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                       minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 8, 8, ?7, '0', '0', 1, 1, '2026-01-01', '2026-01-01')",
                      rusqlite::params![exchange, format!("{base}-{quote}"), base, quote, assets[b], assets[q], d]).unwrap();
        }
        let fx = Self { _dir: dir, c, cipher: common::seed::cipher(), user: 0, exchange };
        let user = fx.user("o@example.com");
        Self { user, ..fx }
    }
    fn user(&self, email: &str) -> i64 {
        self.c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES (?1, 'x', 'Owner', 1, '2026-01-01', '2026-01-01')", [email]).unwrap();
        let user = self.c.last_insert_rowid();
        let e = |v: &str| self.cipher.encrypt(v);
        self.c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, 1, '2026-01-01', '2026-01-01')",
                       rusqlite::params![user, self.exchange, e(email), e("secret"), e("phrase")]).unwrap();
        user
    }
    fn import_as(&self, user: i64, text: &str) -> run::Answer { run::import(&self.c, &self.cipher, user, "kucoin", text.as_bytes(), None, None, now()).unwrap() }
    fn import(&self, text: &str) -> run::Answer { self.import_as(self.user, text) }
    fn imported(&self, text: &str) -> (usize, usize) {
        match self.import(text) { run::Answer::Written { imported, generic, .. } => (imported, generic), other => panic!("{other:?}") }
    }
    /// The user's ledger as (type, currency, amount, quote, quote amount, fee currency, fee, time, tx_id), by tx_id.
    fn ledger_of(&self, user: i64) -> Vec<LedgerRow> {
        let mut s = self.c.prepare("SELECT entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, transacted_at, COALESCE(tx_id, '')
                                    FROM account_transactions WHERE user_id = ?1 ORDER BY tx_id").unwrap();
        s.query_map([user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
    fn ledger(&self) -> Vec<LedgerRow> { self.ledger_of(self.user) }
    fn count(&self, sql: &str) -> i64 { self.c.query_row(sql, [], |r| r.get(0)).unwrap() }
    fn standing(&self) -> Standing { identity::standing(&self.c, self.user, self.exchange).unwrap() }
}

fn lines(text: &str) -> Vec<&str> { text.lines().collect() }
/// An export of the header and `rows` lines of `text`.
fn part(text: &str, rows: std::ops::Range<usize>) -> String { let l = lines(text); std::iter::once(l[0]).chain(l[1..][rows].iter().copied()).collect::<Vec<_>>().join("\n") }

fn rows(text: &str) -> Vec<SourceRow> { deltabadger::tracker::import::kucoin::read(&csv::Table::parse(text).unwrap()).unwrap() }
fn derived(text: &str, catalogue: &Catalogue) -> Vec<DerivedRow> {
    derive(&rows(text), catalogue).into_iter().map(|d| (d.entry.entry_type, d.entry.base_currency.unwrap(), d.entry.base_amount.to_s_f(),
        d.entry.quote_amount.map(|q| q.to_s_f()), d.entry.fee_currency, d.entry.raw.value["reason"].as_str().map(str::to_string))).collect()
}
fn catalogue() -> Catalogue { Catalogue::of(&[("ETH", "BTC", 6), ("BTC", "USDT", 1), ("KCS", "USDT", 3)]) }

// ---- Derivation ----------------------------------------------------------------------------------------------------

#[test]
fn a_buy_and_a_sell_of_one_pair_in_one_second_stay_two_trades() {
    let out = derived(&export(UID, &["BTC,Deposit,0.1,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,2001,1,2026-01-01 12:00:00,Spot",
                                     "BTC,Withdrawal,0.05,0,2026-01-01 12:00:00,Spot", "USDT,Deposit,999.5,0.5,2026-01-01 12:00:00,Spot"]), &catalogue());
    assert_eq!(out, [(0, "BTC".into(), "0.1".into(), Some("2000.0".into()), Some("USDT".into()), None),
                     (1, "BTC".into(), "0.05".into(), Some("1000.0".into()), Some("USDT".into()), None)]);
}

#[test]
fn no_unique_assignment_is_one_aggregated_trade_with_exact_sums() {
    // Two buys that pair either way at valid prices within 1 % of each other: neither pairing is the one.
    let out = derived(&export(UID, &["BTC,Deposit,1,0,2026-01-01 12:00:00,Spot", "BTC,Deposit,1.001,0,2026-01-01 12:00:00,Spot",
                                     "USDT,Withdrawal,20020,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,20040.02,0,2026-01-01 12:00:00,Spot"]),
                      &Catalogue::of(&[("BTC", "USDT", 2)]));
    assert_eq!(out, [(0, "BTC".into(), "2.001".into(), Some("40060.02".into()), Some("USDT".into()), Some("aggregated".into()))]);
}

#[test]
fn unpaired_legs_and_unknown_types_are_signed_adjustments() {
    let out = derived(&export(UID, &["ETH,Deposit,1,0,2026-01-01 12:00:00,Spot", "DOGE,Withdrawal,5,0,2026-01-01 12:00:00,Spot",
                                     "SOL,Deposit,2,0,2026-01-01 12:00:01,Mystery Credit", "SOL,Withdrawal,3,0,2026-01-01 12:00:02,Mystery Debit"]), &catalogue());
    let mut out = out;
    out.sort();
    assert_eq!(out, [(15, "DOGE".into(), "-5.0".into(), None, None, Some("unpaired".into())), (15, "ETH".into(), "1.0".into(), None, None, Some("unpaired".into())),
                     (15, "SOL".into(), "-3.0".into(), None, None, Some("unmapped_type".into())), (15, "SOL".into(), "2.0".into(), None, None, Some("unmapped_type".into()))]);
}

#[test]
fn a_kcs_fee_leg_is_attached_to_its_one_fill_else_a_fee_row() {
    let one = derived(&export(UID, &["BTC,Deposit,0.1,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,2000,0,2026-01-01 12:00:00,Spot",
                                     "KCS,Withdrawal,0.2,0,2026-01-01 12:00:00,Spot"]), &Catalogue::of(&[("BTC", "USDT", 1)]));
    assert_eq!(one, [(0, "BTC".into(), "0.1".into(), Some("2000.0".into()), Some("KCS".into()), None)]);
    let two = derived(&export(UID, &["BTC,Deposit,0.1,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,2000,0,2026-01-01 12:00:00,Spot",
                                     "BTC,Deposit,0.2,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,4020,0,2026-01-01 12:00:00,Spot",
                                     "KCS,Withdrawal,0.2,0,2026-01-01 12:00:00,Spot"]), &Catalogue::of(&[("BTC", "USDT", 1)]));
    assert_eq!(two.iter().filter(|e| e.0 == 10).map(|e| (e.1.as_str(), e.2.as_str())).collect::<Vec<_>>(), [("KCS", "0.2")]);
    assert_eq!(two.iter().filter(|e| e.0 == 0).count(), 2);
}

#[test]
fn convert_is_a_swap_pair_and_internal_transfers_derive_nothing() {
    let out = derive(&rows(&export(UID, &["BTC,Withdrawal,0.1,0,2026-01-01 12:00:00,Convert", "ETH,Deposit,1.5,0,2026-01-01 12:00:00,Convert",
                                          "BTC,Withdrawal,0.3,0,2026-01-01 12:00:05,Transfer", "BTC,Deposit,0.3,0,2026-01-01 12:00:05,Transfer"]))
                     .into_iter().filter(|r| r.kind == "Convert").collect::<Vec<_>>(), &catalogue());
    assert_eq!(out.iter().map(|d| (d.entry.entry_type, d.entry.base_currency.clone().unwrap())).collect::<Vec<_>>(), [(3, "BTC".into()), (2, "ETH".into())]);
    assert!(out[0].entry.group_id.is_some() && out[0].entry.group_id == out[1].entry.group_id);
    let transfers: Vec<SourceRow> = rows(&export(UID, &["BTC,Withdrawal,0.3,0,2026-01-01 12:00:05,Transfer", "BTC,Deposit,0.3,0,2026-01-01 12:00:05,Transfer"]));
    assert!(derive(&transfers, &catalogue()).is_empty());
}

#[test]
fn identical_rows_in_one_second_are_occurrences_and_both_count() {
    let text = export(UID, &["BTC,Deposit,0.1,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,2000,0,2026-01-01 12:00:00,Spot",
                             "BTC,Deposit,0.1,0,2026-01-01 12:00:00,Spot", "USDT,Withdrawal,2000,0,2026-01-01 12:00:00,Spot"]);
    let r = rows(&text);
    assert_eq!(r.iter().map(|r| r.row_id.as_str()).collect::<std::collections::BTreeSet<_>>().len(), 4);
    assert_eq!(derived(&text, &catalogue()).iter().filter(|e| e.0 == 0).count(), 2, "two trades of the same size, not one");
}

#[test]
fn many_legs_on_one_side_aggregate_fast_and_identical_legs_pair() {
    let at = "2026-01-01 12:00:00";
    let mut legs: Vec<String> = (1..=8).map(|i| format!("BTC,Deposit,0.{i},0,{at},Spot")).collect();
    legs.extend((1..=100).map(|i| format!("USDT,Withdrawal,{},0,{at},Spot", 2000 + i)));
    let started = std::time::Instant::now();
    let out = derived(&export(UID, &legs.iter().map(String::as_str).collect::<Vec<_>>()), &catalogue());
    assert!(started.elapsed().as_secs() < 2, "bounded search");
    assert_eq!(out.iter().map(|e| e.5.as_deref()).collect::<Vec<_>>(), [Some("aggregated")]);
    let same: Vec<String> = (0..8).flat_map(|_| [format!("BTC,Deposit,0.1,0,{at},Spot"), format!("USDT,Withdrawal,2000,0,{at},Spot")]).collect();
    let started = std::time::Instant::now();
    let out = derived(&export(UID, &same.iter().map(String::as_str).collect::<Vec<_>>()), &catalogue());
    assert!(started.elapsed().as_secs() < 2, "identical legs are one choice");
    assert_eq!(out.len(), 8);
    assert!(out.iter().all(|e| e.0 == 0 && e.5.is_none()), "{out:?}");
}

// ---- The write path ------------------------------------------------------------------------------------------------

#[test]
fn the_same_file_twice_adds_nothing() {
    let fx = Fx::new();
    assert_eq!(fx.imported(TRADING), (163, 0));
    let before = fx.ledger();
    assert!(matches!(fx.import(TRADING), run::Answer::Written { imported: 0, .. }));
    assert_eq!(fx.ledger(), before);
    assert_eq!(fx.count("SELECT count(*) FROM import_rows"), 327);
}

#[test]
fn subset_and_superset_exports_give_the_same_ledger_in_either_order() {
    let (subset, superset) = (part(TRADING, 0..120), TRADING.to_string());
    let a = Fx::new();
    a.import(&subset);
    a.import(&superset);
    let b = Fx::new();
    b.import(&superset);
    b.import(&subset);
    assert_eq!(a.ledger().len(), 163);
    assert_eq!(a.ledger(), b.ledger());
}

#[test]
fn complementary_exports_give_their_union_and_a_completed_second_is_rebuilt() {
    // The 4-row second split across two files: the first holds its two ETH legs only (unpaired), the second its BTC legs.
    let four: Vec<&str> = lines(TRADING).into_iter().filter(|l| l.contains("08:34:27")).collect();
    let fx = Fx::new();
    let first = [HEADER, four[0], four[1]].join("\n");
    assert_eq!(fx.imported(&first), (2, 2), "two unpaired legs, labelled generically");
    let second = [HEADER, four[2], four[3]].join("\n");
    fx.import(&second);
    let trades: Vec<_> = fx.ledger().into_iter().filter(|r| r.7.starts_with("2022-09-28 05:34:27")).collect();
    assert_eq!(trades.len(), 2);
    assert!(trades.iter().all(|t| t.0 == 0), "{trades:?}");
    fx.import(FUNDING);
    assert_eq!(fx.ledger().iter().filter(|r| r.0 == 4).count(), 1, "the funding account's deposit, its transfer derives nothing");
}

#[test]
fn rebuilding_a_second_keeps_the_users_edits_on_entries_derived_as_before() {
    let fx = Fx::new();
    let at = "2026-01-01 12:00:00";
    fx.import(&export(UID, &[&format!("BTC,Deposit,0.1,0,{at},Deposit"), &format!("USDT,Withdrawal,100,0,{at},Convert")]));
    let id = |t: i64| -> i64 { fx.count(&format!("SELECT id FROM account_transactions WHERE entry_type = {t}")) };
    let (deposit, half) = (id(4), id(15));
    fx.c.execute("UPDATE account_transactions SET manual_values = '{\"base_amount\":\"0.2\"}', transfer_link_rejected = 1 WHERE id = ?1", [deposit]).unwrap();
    fx.c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, linked_transaction_id, created_at, updated_at)
                  VALUES (?1, ?2, 5, 'USDT', 100, '2025-12-31 12:00:00', ?3, '2026-01-01', '2026-01-01')", rusqlite::params![fx.user, fx.exchange, half]).unwrap();
    // The other half of the Convert: the lone half (same source row) becomes a swap leg, the deposit is derived as before.
    fx.import(&export(UID, &[&format!("ETH,Deposit,0.05,0,{at},Convert")]));
    let kept: (i64, String, i64) = fx.c.query_row("SELECT id, manual_values, transfer_link_rejected FROM account_transactions WHERE entry_type = 4", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(kept, (deposit, "{\"base_amount\":\"0.2\"}".into(), 1));
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE entry_type = 15"), 0, "the lone half is replaced");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE entry_type IN (2, 3)"), 2);
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE linked_transaction_id IS NOT NULL"), 0, "the link to the replaced half is cleared");
}

#[test]
fn two_users_importing_one_file_each_get_their_own_rows() {
    let fx = Fx::new();
    let other = fx.user("p@example.com");
    assert_eq!(fx.imported(TRADING).0, 163);
    assert!(matches!(fx.import_as(other, TRADING), run::Answer::Written { imported: 163, .. }));
    assert_eq!(fx.count("SELECT count(*) FROM import_rows"), 654);
    assert_eq!(fx.ledger_of(other).len(), 163);
}

#[test]
fn the_notice_counts_generic_labels() {
    let fx = Fx::new();
    let (imported, generic) = fx.imported(&export(UID, &["SOL,Deposit,2,0,2026-01-01 12:00:01,Mystery Credit", "ETH,Deposit,1,0,2026-01-01 12:00:01,Deposit"]));
    assert_eq!((imported, generic), (2, 1));
}

// ---- Errors --------------------------------------------------------------------------------------------------------

#[test]
fn a_file_without_one_readable_account_zone_or_amount_is_malformed() {
    let fx = Fx::new();
    let good = "BTC,Deposit,0.1,0,2026-01-01 12:00:00,Deposit";
    let mixed = [export(UID, &[good]), export(OTHER, &[good]).lines().nth(1).unwrap().to_string()].join("\n");
    let masked = export("1234****89", &[good]);
    let missing = export("", &[good]);
    let zone = export(UID, &[good]).replace("Time(UTC+03:00)", "Time(local)");
    let amount = export(UID, &["BTC,Deposit,0.1x,0,2026-01-01 12:00:00,Deposit"]);
    for (name, text) in [("mixed", mixed), ("masked", masked), ("missing", missing), ("zone", zone), ("amount", amount)] {
        assert_eq!(fx.import(&text), run::Answer::Error("malformed"), "{name}");
    }
    assert_eq!(fx.count("SELECT count(*) FROM import_rows"), 0);
}

// ---- Identity ------------------------------------------------------------------------------------------------------

fn key_id(fx: &Fx) -> i64 { fx.c.query_row("SELECT id FROM api_keys WHERE user_id = ?1", [fx.user], |r| r.get(0)).unwrap() }
fn observe(fx: &Fx, raw: &str) {
    let key = key_id(fx);
    let credentials = deltabadger::sync::validation_material(&fx.c, &fx.cipher, key).unwrap().0;
    let version = deltabadger::engine::model::credential_version_by_id(&fx.c, key).unwrap().unwrap();
    identity::record(&fx.c, &fx.cipher, key, &credentials, &version, &identity::Observed::Read(identity::AccountId::parse(raw).unwrap()), now()).unwrap();
}

#[test]
fn an_import_records_its_account_and_another_accounts_file_is_a_conflict_in_either_order() {
    for key_present in [false, true] {
        let fx = Fx::new();
        if key_present { identity::key(&fx.c, &fx.cipher, now()).unwrap(); }
        let stored = deltabadger::app_config::get_plain(&fx.c, identity::KEY_CONFIG).unwrap();
        fx.import(TRADING);
        assert_eq!(fx.count("SELECT count(*) FROM exchange_account_imports"), 1);
        assert_eq!(fx.standing(), Standing::Bound);
        let after = deltabadger::app_config::get_plain(&fx.c, identity::KEY_CONFIG).unwrap();
        assert!(after.is_some() && (stored.is_none() || stored == after), "created once, then reused");
        // File then key: the file established the account, the key reads another.
        observe(&fx, OTHER);
        assert_eq!(fx.standing(), Standing::Conflicted);
    }
    // Key then file: the key established the account, the file names another. It imports all the same.
    let fx = Fx::new();
    observe(&fx, OTHER);
    assert_eq!(fx.standing(), Standing::Bound);
    assert_eq!(fx.imported(TRADING).0, 163, "recorded, never refused");
    assert_eq!(fx.standing(), Standing::Conflicted);
}

// ---- Chart effect --------------------------------------------------------------------------------------------------

#[test]
fn the_backfill_holds_the_files_balances_with_no_spike() {
    let fx = Fx::new();
    fx.import(FUNDING);
    fx.import(TRADING);
    // A 2026 API fill and deposit after the file.
    fx.c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, tx_id, transacted_at, raw_data, created_at, updated_at)
                  VALUES (?1, ?2, 0, 'ETH', 0.01, 'BTC', 0.0005, 'ETH-BTC-1', '2026-02-01 00:00:00', '{}', '2026-02-01', '2026-02-01'),
                         (?1, ?2, 4, 'BTC', 0.01, NULL, NULL, 'dep-1', '2026-01-15 00:00:00', '{}', '2026-01-15', '2026-01-15')", [fx.user, fx.exchange]).unwrap();
    // Independently: every row that is no transfer moves its currency by its Amount.
    let mut expected: std::collections::BTreeMap<String, f64> = Default::default();
    for text in [FUNDING, TRADING] {
        for r in rows(text).iter().filter(|r| r.kind != "Transfer") {
            *expected.entry(r.currency.clone()).or_default() += if r.inflow { r.amount.to_f() } else { -r.amount.to_f() };
        }
    }
    *expected.get_mut("ETH").unwrap() += 0.01;
    *expected.get_mut("BTC").unwrap() += 0.01 - 0.0005;
    let stored = deltabadger::tracker::rows::load(&fx.c, fx.user).unwrap();
    let last = deltabadger::tracker::backfill::held(&stored).unwrap();
    let venue = last.values().next().unwrap();
    for (coin, quantity) in &expected { assert!((venue[coin].to_f() - quantity).abs() < 1e-12, "{coin}: {} vs {quantity}", venue[coin].to_f()); }
    // No spike: no prefix of the history ever holds less than nothing, or more of a coin than ever came in.
    let ceiling = 0.05;
    for n in 1..=stored.len() {
        for (coin, q) in deltabadger::tracker::backfill::held(&stored[..n]).unwrap().values().flat_map(|v| v.iter()) {
            assert!(q.to_f() >= -1e-12 && q.to_f() <= ceiling, "{coin} after {n} rows: {}", q.to_f());
        }
    }
}

// ---- Takeover (KI2) ------------------------------------------------------------------------------------------------

use deltabadger::sync::continuation::{Run, Stopwatch};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::rc::Rc;

/// The sync's clock, and the key's watermark: the sync reads from 25 hours before it.
const SYNC_NOW: &str = "2026-09-20T02:00:00Z";
const WATERMARK: &str = "2026-09-15T00:00:00Z";
fn ms(utc: &str) -> i64 { utc.parse::<DateTime<Utc>>().unwrap().timestamp_millis() }

/// KuCoin's three sources as the walk reads them, `per_page` rows a page (transfers newest first, within the bounds asked).
#[derive(Clone)]
struct Api { fills: Rc<Vec<Value>>, deposits: Rc<Vec<Value>>, withdrawals: Rc<Vec<Value>>, per_page: usize }
impl Transport for Api {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        use deltabadger::sync::kucoin_ledger::{DEPOSITS_PATH, FILLS_PATH, WITHDRAWALS_PATH};
        let q = |k: &str| r.query.iter().find(|(name, _)| *name == k).map(|(_, v)| v.parse::<i64>().unwrap());
        let at = |v: &Value| v["createdAt"].as_i64().unwrap();
        let mut rows: Vec<&Value> = match r.path.as_str() { FILLS_PATH => self.fills.iter().collect(), DEPOSITS_PATH => self.deposits.iter().collect(),
                                                            WITHDRAWALS_PATH => self.withdrawals.iter().collect(), other => panic!("unscripted {other}") };
        rows.retain(|v| q("startAt").is_none_or(|s| at(v) >= s) && q("endAt").is_none_or(|e| at(v) < e));
        rows.sort_by_key(|v| std::cmp::Reverse(at(v)));
        let page = q("currentPage").unwrap() as usize;
        let items: Vec<&Value> = rows.iter().skip((page - 1) * self.per_page).take(self.per_page).copied().collect();
        let body = json!({ "code": "200000", "data": { "currentPage": page, "pageSize": 500, "totalNum": rows.len(), "totalPage": rows.len().div_ceil(self.per_page), "items": items } });
        Ok(HttpResponse { status: 200, body: body.to_string() })
    }
}

/// A run with room for every page (`paused` false), or for its first page only (a run the deadline stops after one).
struct Pace { paused: bool }
impl Stopwatch for Pace { fn elapsed(&self) -> std::time::Duration { std::time::Duration::from_secs(if self.paused { 1_000_000 } else { 0 }) } }

fn fill(id: &str, size: &str, funds: &str, fee: &str, utc: &str) -> Value {
    json!({ "symbol": "BTC-USDT", "tradeId": id, "orderId": id, "side": "buy", "price": "1", "size": size, "funds": funds, "fee": fee, "feeCurrency": "USDT", "createdAt": ms(utc) })
}
/// A transfer; a blank `id` is one KuCoin names by neither id nor hash (stored without a tx_id).
fn transfer(id: &str, amount: &str, fee: &str, utc: &str) -> Value {
    let mut v = json!({ "currency": "USDT", "amount": amount, "fee": fee, "status": "SUCCESS", "isInner": false, "createdAt": ms(utc) });
    if !id.is_empty() { v["id"] = json!(id); }
    v
}

impl Fx {
    /// The user's KuCoin key with a watermark it produced (an incremental sync from WATERMARK less the overlap).
    fn watermark(&self, utc: &str) {
        let key = key_id(self);
        self.c.execute("UPDATE api_keys SET last_synced_at = ?1 WHERE id = ?2", rusqlite::params![deltabadger::sync::sql_time(utc.parse().unwrap()), key]).unwrap();
        let producer = deltabadger::engine::model::credential_version_by_id(&self.c, key).unwrap().unwrap();
        let tx = self.c.unchecked_transaction().unwrap();
        deltabadger::sync::cache::record_ledger(&deltabadger::engine::model::check_credential_result(&tx, &Some(producer.clone())).unwrap(), key, &producer,
                                                SYNC_NOW.parse().unwrap()).unwrap();
        tx.commit().unwrap();
    }
    /// One run of the key's ledger sync; whether it completed.
    async fn sync(&self, api: &Api, paused: bool) -> bool {
        let c = Connection::open(self._dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        let db = deltabadger::jobs::Db::new(c, common::seed::cipher());
        let key = key_id(self);
        let credentials = deltabadger::sync::validation_material(&self.c, &self.cipher, key).unwrap().0;
        let pace = Pace { paused };
        let run = Run { deadline: std::time::Duration::from_secs(3600), chain_runs: usize::MAX, stopwatch: &pace };
        let clock = deltabadger::engine::FixedClock(SYNC_NOW.parse().unwrap());
        let out = deltabadger::sync::kucoin_ledger::sync_within(&db, api.clone(), key, &credentials, &clock, std::time::Duration::ZERO, &run).await.unwrap().unwrap();
        db.settled().await;
        out.complete
    }
    /// The file's entries still held, as (class, time).
    fn file_entries(&self) -> Vec<(String, String)> {
        let mut s = self.c.prepare("SELECT json_extract(raw_data, '$.class'), transacted_at FROM account_transactions WHERE user_id = ?1 AND tx_id LIKE 'kucoin-csv-%' \
                                    ORDER BY 2, 1").unwrap();
        s.query_map([self.user], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
    fn file_classes_at(&self, utc_second: &str) -> Vec<String> {
        self.file_entries().into_iter().filter(|(_, at)| at == utc_second).map(|(class, _)| class).collect()
    }
}

/// Day A (2026-09-10, before any API row) and day B (2026-09-16, which the API also returns): a deposit, a buy with a
/// KCS fee leg and an unpaired leg, a withdrawal, and (day B) staking income. Times are UTC+03:00.
fn overlapping_export() -> String {
    export(UID, &["USDT,Deposit,100,0,2026-09-10 13:00:00,Deposit", "BTC,Deposit,0.001,0,2026-09-10 13:00:01,Spot", "USDT,Withdrawal,60.06,0.06,2026-09-10 13:00:01,Spot",
                  "USDT,Withdrawal,5,1,2026-09-10 13:00:02,Withdrawal",
                  "USDT,Deposit,50,0,2026-09-16 13:00:00,Deposit", "BTC,Deposit,0.001,0,2026-09-16 13:00:01,Spot", "USDT,Withdrawal,70.07,0.07,2026-09-16 13:00:01,Spot",
                  "KCS,Withdrawal,0.1,0,2026-09-16 13:00:01,Spot", "XYZ,Deposit,3,0,2026-09-16 13:00:01,Spot",
                  "USDT,Withdrawal,5,1,2026-09-16 13:00:02,Withdrawal", "USDT,Deposit,2,0,2026-09-16 13:00:03,Staking Profits"])
}

/// The API from 2026-09-14: one row of each source before day B, and day B's buy, deposit (named by neither id nor
/// hash, so stored without a tx_id) and withdrawal.
fn overlapping_api(per_page: usize) -> Api {
    Api { fills: Rc::new(vec![fill("f1", "0.002", "130", "0.1", "2026-09-14T12:00:00Z"), fill("f2", "0.001", "70", "0.07", "2026-09-16T10:00:01Z")]),
          deposits: Rc::new(vec![transfer("d1", "10", "0", "2026-09-14T12:00:00Z"), transfer("", "50", "0", "2026-09-16T10:00:00Z")]),
          withdrawals: Rc::new(vec![transfer("w1", "3", "0", "2026-09-14T12:00:00Z"), transfer("w2", "5", "1", "2026-09-16T10:00:02Z")]), per_page }
}

const DAY_A: [(&str, &str); 3] = [("deposits", "2026-09-10 10:00:00"), ("fills", "2026-09-10 10:00:01"), ("withdrawals", "2026-09-10 10:00:02")];

/// What both orders must hold: day A's file entries, day B's staking income only from the file, every API row, and
/// every source row still stored.
fn assert_taken_over(fx: &Fx) {
    let kept: Vec<(String, String)> = DAY_A.iter().map(|(c, t)| (c.to_string(), t.to_string())).chain([("other".to_string(), "2026-09-16 10:00:03".to_string())]).collect();
    assert_eq!(fx.file_entries(), kept);
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE json_extract(raw_data, '$.createdAt') IS NOT NULL"), 6, "every API row");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions"), 10);
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE entry_type = 4 AND transacted_at LIKE '2026-09-16%'"), 1, "day B's deposit once, the API's");
    assert_eq!(fx.count("SELECT count(*) FROM import_rows"), 11, "source rows stay stored");
}

#[tokio::test(flavor = "current_thread")]
async fn import_then_completed_sync_and_completed_sync_then_import_give_one_ledger() {
    let first = Fx::new();
    first.watermark(WATERMARK);
    first.import(&overlapping_export());
    assert_eq!(first.file_entries().len(), 9, "day A's three, day B's deposit, buy, fee, adjustment, withdrawal and income");
    // A link into day B's file deposit, cleared when it is taken over.
    first.c.execute("UPDATE account_transactions SET linked_transaction_id = (SELECT id FROM account_transactions WHERE tx_id LIKE 'kucoin-csv-%' AND entry_type = 4 \
                     AND transacted_at = '2026-09-16 10:00:00') WHERE tx_id LIKE 'kucoin-csv-%' AND entry_type = 5 AND transacted_at = '2026-09-10 10:00:02'", []).unwrap();
    assert!(first.sync(&overlapping_api(500), false).await);
    assert_taken_over(&first);
    assert_eq!(first.count("SELECT count(*) FROM account_transactions WHERE linked_transaction_id IS NOT NULL AND linked_transaction_id NOT IN (SELECT id FROM account_transactions)"), 0);
    assert_eq!(first.count("SELECT count(*) FROM account_transactions WHERE entry_type = 5 AND transacted_at = '2026-09-10 10:00:02' AND linked_transaction_id IS NULL"), 1);

    let second = Fx::new();
    second.watermark(WATERMARK);
    assert!(second.sync(&overlapping_api(500), false).await);
    let (imported, _) = second.imported(&overlapping_export());
    assert_eq!(imported, 4, "day A's three and the income: what the API covers is not derived");
    assert_taken_over(&second);
    assert_eq!(first.ledger(), second.ledger());
    // The same file again, and a second at the cutoff, add nothing.
    assert_eq!(second.imported(&overlapping_export()).0, 0);
    assert_eq!(second.imported(&export(UID, &["USDT,Deposit,9,0,2026-09-14 15:00:00,Deposit"])).0, 0, "at the deposits' cutoff second");
    assert_eq!(second.imported(&export(UID, &["USDT,Deposit,9,0,2026-09-14 14:59:59,Deposit"])).0, 1, "a second before it");
}

#[tokio::test(flavor = "current_thread")]
async fn a_paused_sync_takes_over_only_the_sources_it_has_finished() {
    let fx = Fx::new();
    fx.watermark(WATERMARK);
    fx.import(&overlapping_export());
    let day_b = |s: &str| fx.file_classes_at(&format!("2026-09-16 {s}"));
    let api = overlapping_api(1);
    // Fills, page 1 of 2: nothing finished.
    assert!(!fx.sync(&api, true).await);
    assert_eq!((day_b("10:00:00"), day_b("10:00:01").len(), day_b("10:00:02")), (vec!["deposits".to_string()], 3, vec!["withdrawals".to_string()]));
    // Fills, page 2: the fills finished, their buy, fee and adjustment taken over.
    assert!(!fx.sync(&api, true).await);
    assert!(day_b("10:00:01").is_empty());
    assert_eq!(day_b("10:00:00"), ["deposits"]);
    // Deposits, page 1 of 2: not finished.
    assert!(!fx.sync(&api, true).await);
    assert_eq!(day_b("10:00:00"), ["deposits"]);
    // Deposits, page 2: finished; paused between deposits and withdrawals, the withdrawal stays the file's.
    assert!(!fx.sync(&api, true).await);
    assert!(day_b("10:00:00").is_empty());
    assert_eq!(day_b("10:00:02"), ["withdrawals"]);
    assert!(!fx.sync(&api, true).await);
    assert_eq!(day_b("10:00:02"), ["withdrawals"]);
    // The last withdrawals page completes the import.
    assert!(fx.sync(&api, true).await);
    assert_taken_over(&fx);
}

#[tokio::test(flavor = "current_thread")]
async fn a_rescan_moves_the_cutoff_earlier_and_a_rejected_api_row_moves_none() {
    let fx = Fx::new();
    fx.import(&export(UID, &["USDT,Deposit,50,0,2026-09-16 13:00:00,Deposit"]));
    // An id-less deposit already held (another file) within a second of d1: the API's d1 is its duplicate, never stored.
    fx.c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, raw_data, manual_values, \
                  transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, 4, 'USDT', 10, '2026-09-14 12:00:00.5', '{}', '{}', 0, '2026-01-01', '2026-01-01')",
                 [fx.user, fx.exchange]).unwrap();
    let api = Api { fills: Rc::new(vec![]), deposits: Rc::new(vec![transfer("d1", "10", "0", "2026-09-14T12:00:00Z"), transfer("d3", "1", "0", "2026-09-17T12:00:00Z")]),
                    withdrawals: Rc::new(vec![]), per_page: 500 };
    fx.watermark(WATERMARK);
    assert!(fx.sync(&api, false).await);
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE tx_id = 'd1'"), 0, "rejected as the held row's duplicate");
    assert_eq!(fx.file_classes_at("2026-09-16 10:00:00"), ["deposits"], "the cutoff is d3's, the earliest API row stored");
    // A rescan from further back stores an earlier deposit (no duplicate this time): the cutoff moves before day B.
    fx.c.execute("DELETE FROM account_transactions WHERE tx_id IS NULL AND raw_data = '{}'", []).unwrap();
    fx.watermark("2026-09-14T00:00:00Z");
    assert!(fx.sync(&api, false).await);
    assert!(fx.file_classes_at("2026-09-16 10:00:00").is_empty());
    assert_eq!(fx.count("SELECT count(*) FROM import_rows"), 1);
}

// ---- Review fixes: provenance, one writer per import, cutoffs in Rust's own table ----------------------------------

#[test]
fn a_restored_trade_is_never_replaced_by_a_partial_export() {
    let (btc, usdt) = ("BTC,Deposit,0.001,0,2026-01-01 12:00:01,Spot", "USDT,Withdrawal,60.06,0.06,2026-01-01 12:00:01,Spot");
    // Restored through the Deltabadger CSV: the id kept, no raw data and no source rows; or the raw data kept, the rows not.
    for (name, raw) in [("no raw data", Some("{}")), ("no source rows", None)] {
        let fx = Fx::new();
        assert_eq!(fx.imported(&export(UID, &[btc, usdt])).0, 1);
        fx.c.execute("DELETE FROM import_rows", []).unwrap();
        fx.c.execute("UPDATE account_transactions SET manual_values = '{\"quote_amount\":\"61\"}', raw_data = COALESCE(?1, raw_data)", [raw]).unwrap();
        let restored = || -> (i64, i64, f64, String) {
            fx.c.query_row("SELECT id, entry_type, quote_amount, manual_values FROM account_transactions WHERE entry_type = 0", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
        };
        let before = restored();
        // A later export holding only the BTC leg: stored, but the restored second derives nothing.
        assert_eq!(fx.imported(&export(UID, &[btc])).0, 0, "{name}");
        assert_eq!(restored(), before, "{name}");
        // The whole second again: still nothing, the restored trade covers it.
        assert_eq!(fx.imported(&export(UID, &[btc, usdt])).0, 0, "{name}");
        assert_eq!(restored(), before, "{name}");
        assert_eq!(fx.count("SELECT count(*) FROM account_transactions"), 1, "{name}");
        assert_eq!(fx.count("SELECT count(*) FROM import_rows"), 2, "{name}: the rows are stored");
    }
}

#[test]
fn a_second_without_a_restored_entry_still_derives() {
    let fx = Fx::new();
    assert_eq!(fx.imported(&export(UID, &["BTC,Deposit,0.001,0,2026-01-01 12:00:01,Spot", "USDT,Withdrawal,60.06,0.06,2026-01-01 12:00:01,Spot"])).0, 1);
    fx.c.execute("DELETE FROM import_rows", []).unwrap();
    fx.c.execute("UPDATE account_transactions SET raw_data = '{}'", []).unwrap();
    // The restored second derives nothing; the next second, and an earlier one, derive as always.
    let (imported, generic) = fx.imported(&export(UID, &["BTC,Deposit,0.001,0,2026-01-01 12:00:01,Spot", "BTC,Deposit,0.002,0,2026-01-01 12:00:02,Spot",
                                                         "USDT,Deposit,5,0,2026-01-01 12:00:00,Deposit"]));
    assert_eq!((imported, generic), (2, 1), "the next second's unpaired leg and the earlier deposit");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE transacted_at = '2026-01-01 09:00:01'"), 1, "only the restored trade");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE transacted_at = '2026-01-01 09:00:02' AND entry_type = 15"), 1);
}

/// `n` Spot legs, one per second from 2026-01-01 00:00:00 (UTC+03:00), each an unpaired adjustment.
fn unpaired(n: usize) -> Vec<String> {
    let start: chrono::NaiveDateTime = "2026-01-01T00:00:00".parse().unwrap();
    (0..n).map(|i| format!("XYZ,Deposit,1,0,{},Spot", (start + chrono::Duration::seconds(i as i64)).format("%Y-%m-%d %H:%M:%S"))).collect()
}

#[test]
fn thousands_of_unpaired_rows_import_in_one_pass_as_one_file_per_second_would() {
    let rows = unpaired(4000);
    let fx = Fx::new();
    let started = std::time::Instant::now();
    assert_eq!(fx.imported(&export(UID, &rows.iter().map(String::as_str).collect::<Vec<_>>())), (4000, 4000));
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    // The same as each second written on its own, with its own writer state (one file per second).
    let rows = unpaired(200);
    let (whole, each) = (Fx::new(), Fx::new());
    whole.import(&export(UID, &rows.iter().map(String::as_str).collect::<Vec<_>>()));
    for r in &rows { each.import(&export(UID, &[r])); }
    assert_eq!(whole.ledger().len(), 200);
    assert_eq!(whole.ledger(), each.ledger());
}

#[tokio::test(flavor = "current_thread")]
async fn the_cutoffs_survive_an_app_configs_wipe() {
    let fx = Fx::new();
    fx.watermark(WATERMARK);
    assert!(fx.sync(&overlapping_api(500), false).await);
    assert_eq!(fx.count("SELECT count(*) FROM import_cutoffs"), 3);
    assert_eq!(fx.count("SELECT count(*) FROM app_configs WHERE key LIKE 'rust_kucoin_csv%'"), 0, "never written there");
    // Rails' encryption reset deletes every app_configs row.
    fx.c.execute("DELETE FROM app_configs", []).unwrap();
    assert_eq!(fx.imported(&overlapping_export()).0, 4, "day A's three and the income: what the API covers is still not derived");
    assert_taken_over(&fx);
}
