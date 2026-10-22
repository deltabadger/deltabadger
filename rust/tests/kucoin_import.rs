//! KuCoin's account-history import (plan KI, slice KI1): rows stored by their own id, ledger entries derived one UTC
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
        let mut s = self.c.prepare("SELECT entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, transacted_at, tx_id
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
fn rebuilding_a_second_clears_links_to_the_entries_it_replaces() {
    let fx = Fx::new();
    let at = "2026-01-01 12:00:00";
    fx.import(&export(UID, &[&format!("BTC,Deposit,0.1,0,{at},Deposit")]));
    let deposit: i64 = fx.count("SELECT id FROM account_transactions WHERE entry_type = 4");
    fx.c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, linked_transaction_id, created_at, updated_at)
                  VALUES (?1, ?2, 5, 'BTC', 0.1, '2025-12-31 12:00:00', ?3, '2026-01-01', '2026-01-01')", rusqlite::params![fx.user, fx.exchange, deposit]).unwrap();
    fx.import(&export(UID, &[&format!("BTC,Deposit,0.1,0,{at},Deposit"), &format!("ETH,Deposit,1,0,{at},Deposit")]));
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE linked_transaction_id IS NOT NULL"), 0, "nullified, as dependent: :nullify does");
    assert_eq!(fx.count("SELECT count(*) FROM account_transactions WHERE entry_type = 4"), 2);
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
