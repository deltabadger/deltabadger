//! Exchange-account identity, slice AF1 (plan AF §6): the identity key and its reset, the migration's tables, the record-only
//! rules, import account ids, the identity reads of KuCoin, Binance and Binance.US, and the discovery of idle credentials.
//! Nothing here may change how a key, a sync, a bot or an import behaves: every assertion on a key's own row says so.
mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::crypto::{Cipher, Credentials};
use deltabadger::engine::model::{credential_version_by_id, CredentialVersion};
use deltabadger::identity::{self, AccountId, CredentialStanding, Observed, Standing};
use deltabadger::tracker::import::{csv, run};
use rusqlite::Connection;
use serde_json::{json, Value};

const KUCOIN: &str = "Exchanges::Kucoin";
const BINANCE: &str = "Exchanges::Binance";
const BINANCE_US: &str = "Exchanges::BinanceUs";
const A: &str = "165111215";
const B: &str = "999000111";

fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/account_identity.json")).unwrap() }
fn now() -> DateTime<Utc> { "2026-10-22T10:00:00Z".parse().unwrap() }
fn id(raw: &str) -> AccountId { AccountId::parse(raw).unwrap() }
fn read(raw: &str) -> Observed { Observed::Read(id(raw)) }

struct Fx { dir: tempfile::TempDir, c: Connection, cipher: Cipher, user: i64 }

impl Fx {
    fn new() -> Self {
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('o@example.com', 'x', 'Owner', 1, '2026-01-01', '2026-01-01')", []).unwrap();
        let user = c.last_insert_rowid();
        Self { dir, c, cipher: common::seed::cipher(), user }
    }
    /// The same install through a new connection: what a restart sees.
    fn restart(&mut self) {
        self.c = Connection::open(self.dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&self.c).unwrap();
        identity::initialize(&self.c, &self.cipher, now()).unwrap();
    }
    fn exchange(&self, kind: &str) -> i64 {
        if let Ok(id) = self.c.query_row("SELECT id FROM exchanges WHERE type = ?1", [kind], |r| r.get(0)) { return id; }
        self.c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?1, '0.1', '0.1', '2026-01-01', '2026-01-01')", [kind]).unwrap();
        self.c.last_insert_rowid()
    }
    /// A correct key of `kind` (0 trading, 1 withdrawal, 2 read-only) on `venue`.
    fn key(&self, venue: &str, kind: i64, key: &str) -> i64 {
        let e = |v: &str| self.cipher.encrypt(v);
        self.c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, key, secret, passphrase, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, '2026-01-01', '2026-01-01')",
                       rusqlite::params![self.user, self.exchange(venue), kind, e(key), e(&format!("{key}-secret")), e("phrase")]).unwrap();
        self.c.last_insert_rowid()
    }
    fn credentials(&self, id: i64) -> Credentials { deltabadger::sync::validation_material(&self.c, &self.cipher, id).unwrap().0 }
    fn version(&self, id: i64) -> CredentialVersion { credential_version_by_id(&self.c, id).unwrap().unwrap() }
    /// What a check of key `id` publishes, in one transaction as the key flows do.
    fn observe(&self, id: i64, observed: Observed) {
        let (credentials, version) = (self.credentials(id), self.version(id));
        let tx = self.c.unchecked_transaction().unwrap();
        identity::record(&tx, &self.cipher, id, &credentials, &version, &observed, now()).unwrap();
        tx.commit().unwrap();
    }
    fn import(&self, venue: &str, account: Option<&str>) {
        let tx = self.c.unchecked_transaction().unwrap();
        identity::record_import(&tx, &self.cipher, self.user, self.exchange(venue), account.map(id).as_ref(), now()).unwrap();
        tx.commit().unwrap();
    }
    /// The same key material in a fresh envelope: Rails' or Rust's re-save of an unchanged key.
    fn reencrypt(&self, id: i64) {
        let creds = self.credentials(id);
        self.c.execute("UPDATE api_keys SET key = ?1, secret = ?2, passphrase = ?3 WHERE id = ?4",
                       rusqlite::params![self.cipher.encrypt(&creds.key), self.cipher.encrypt(&creds.secret), self.cipher.encrypt("phrase"), id]).unwrap();
    }
    /// New material for key `id`.
    fn replace(&self, id: i64, key: &str) {
        self.c.execute("UPDATE api_keys SET key = ?1, secret = ?2 WHERE id = ?3",
                       rusqlite::params![self.cipher.encrypt(key), self.cipher.encrypt(&format!("{key}-secret")), id]).unwrap();
    }
    fn history(&self, venue: &str) {
        self.c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, created_at, updated_at)
                        VALUES (?1, ?2, 0, 'BTC', 1, '2026-01-01', '2026-01-01', '2026-01-01')", [self.user, self.exchange(venue)]).unwrap();
    }
    fn standing(&self, venue: &str) -> Standing { identity::standing(&self.c, self.user, self.exchange(venue)).unwrap() }
    fn account(&self, venue: &str) -> Option<Option<String>> {
        rusqlite::OptionalExtension::optional(self.c.query_row("SELECT fingerprint FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2",
                                                               [self.user, self.exchange(venue)], |r| r.get(0))).unwrap()
    }
    fn count(&self, table: &str) -> i64 { self.c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap() }
    fn fingerprint_of(&self, venue: &str, raw: &str) -> String { identity::existing_key(&self.c, &self.cipher).unwrap().fingerprint(venue, &id(raw)) }
    /// Every key's own row, as the key flows read it: identity recording must never move it.
    fn keys(&self) -> Vec<(i64, i64, Option<String>)> {
        let mut s = self.c.prepare("SELECT id, status, last_sync_error FROM api_keys ORDER BY id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
}

// ---- Schema, against the actual migration -------------------------------------------------------------------------

#[test]
fn the_migration_creates_the_tables_its_contracts_name_and_the_install_checks() {
    let fx = Fx::new();
    let paths = deltabadger::store::Paths::from_env(&|_| None, fx.dir.path());
    assert_eq!(deltabadger::store::check(&paths).unwrap().pending_rust, Vec::<String>::new());
    for table in identity::TABLES { assert!(deltabadger::store::REGISTRY.owned_tables().contains(&table), "{table}"); }
    assert_eq!(fx.count("exchange_accounts"), 0);
}

#[test]
fn an_id_less_import_creates_an_unknown_account_and_the_first_identity_fills_it_once() {
    let fx = Fx::new();
    fx.import(BINANCE, None);
    assert_eq!(fx.account(BINANCE), Some(None), "an account known only from id-less files");
    assert_eq!(fx.standing(BINANCE), Standing::Unknown);
    fx.import(BINANCE, Some(A));
    assert_eq!(fx.account(BINANCE), Some(Some(fx.fingerprint_of(BINANCE, A))), "the first identity fills it");
    fx.import(BINANCE, Some(B));
    assert_eq!(fx.account(BINANCE), Some(Some(fx.fingerprint_of(BINANCE, A))), "a known fingerprint is never overwritten");
    assert_eq!(fx.count("exchange_account_imports"), 2, "both identity-bearing imports are recorded");
}

#[test]
fn an_unmigrated_install_records_nothing_and_fails_nothing() {
    let dir = common::rails_install_unmigrated();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    identity::record_import(&c, &common::seed::cipher(), 1, 1, Some(&id(A)), now()).unwrap();
    identity::initialize(&c, &common::seed::cipher(), now()).unwrap();
    assert!(!identity::ready(&c).unwrap());
    assert!(deltabadger::app_config::get_plain(&c, identity::KEY_CONFIG).unwrap().is_none(), "no key stored either");
}

// ---- The identity key ------------------------------------------------------------------------------------------------

/// A known account A, a credential observation and an import, and history on the exchange.
fn established() -> (Fx, i64) {
    let fx = Fx::new();
    let key = fx.key(KUCOIN, 0, "kc-a");
    fx.history(KUCOIN);
    fx.observe(key, read(A));
    fx.import(KUCOIN, Some(A));
    assert_eq!(fx.standing(KUCOIN), Standing::Bound);
    (fx, key)
}

fn assert_reset(fx: &Fx) {
    assert_eq!(fx.account(KUCOIN), Some(None), "the account stays, its fingerprint unknown");
    assert_eq!((fx.count("exchange_account_credentials"), fx.count("exchange_account_imports")), (0, 0), "every observation cleared");
    assert_eq!(fx.count("account_transactions"), 1, "history untouched");
    assert!(identity::existing_key(&fx.c, &fx.cipher).is_some(), "a new key is stored");
}

#[test]
fn a_reset_clears_fingerprints_and_observations_keeps_history_and_the_next_check_reestablishes() {
    let (mut fx, key) = established();
    let before = fx.fingerprint_of(KUCOIN, A);
    deltabadger::app_config::delete(&fx.c, identity::KEY_CONFIG).unwrap(); // what EncryptedCredentials::Reset does to it
    fx.restart();
    assert_reset(&fx);
    assert_ne!(fx.fingerprint_of(KUCOIN, A), before, "a new key, new fingerprints");
    fx.observe(key, read(A));
    assert_eq!(fx.account(KUCOIN), Some(Some(fx.fingerprint_of(KUCOIN, A))), "re-established by the next check of the reading key");
    assert_eq!(fx.standing(KUCOIN), Standing::Bound);
}

#[test]
fn an_identity_key_that_cannot_be_unwrapped_is_a_reset() {
    let (mut fx, _) = established();
    let other = Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-secret").unwrap());
    fx.c.execute("UPDATE app_configs SET value = ?1 WHERE key = ?2", [other.encrypt(&"ab".repeat(32)), identity::KEY_CONFIG.into()]).unwrap();
    fx.restart();
    assert_reset(&fx);
}

#[test]
fn an_import_after_a_reset_reestablishes_the_account() {
    let (mut fx, _) = established();
    deltabadger::app_config::delete(&fx.c, identity::KEY_CONFIG).unwrap();
    fx.restart();
    fx.import(KUCOIN, Some(A));
    assert_eq!(fx.account(KUCOIN), Some(Some(fx.fingerprint_of(KUCOIN, A))));
}

#[test]
fn reencryption_keeps_every_fingerprint_and_observation() {
    let (mut fx, key) = established();
    let row = |fx: &Fx| fx.c.query_row("SELECT material_digest, fingerprint FROM exchange_account_credentials WHERE api_key_id = ?1", [key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap();
    let before = (row(&fx), fx.account(KUCOIN));
    fx.reencrypt(key);
    fx.observe(key, Observed::Failed("http 500".into()));
    fx.restart();
    assert_eq!((row(&fx), fx.account(KUCOIN)), before);
}

#[test]
fn no_raw_account_id_reaches_the_database() {
    let (fx, _) = established();
    let mut tables = fx.c.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap();
    let tables: Vec<String> = tables.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    for table in tables {
        let mut s = fx.c.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let n = s.column_count();
        let mut rows = s.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..n {
                let text = match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => String::from_utf8_lossy(t).into_owned(),
                    rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                    _ => String::new(),
                };
                assert!(!text.contains(A), "{table} holds the raw account id");
            }
        }
    }
    let encrypted: Vec<u8> = fx.c.query_row("SELECT account_id_enc FROM exchange_accounts", [], |r| r.get(0)).unwrap();
    assert_eq!(fx.cipher.decrypt(&String::from_utf8(encrypted).unwrap()).unwrap(), A, "the id is kept, in the credential envelope");
}

/// The actual Rails recovery (EncryptedCredentials::Reset, as `deltabadger:encryption:reset` runs it), then Rust's start.
#[test]
fn rust_handles_the_reset_rails_actually_makes() {
    let (fx, _) = established();
    let scratch = tempfile::tempdir().unwrap();
    drop(fx.c);
    std::fs::copy(fx.dir.path().join("production.sqlite3"), scratch.path().join("primary.sqlite3")).unwrap();
    std::fs::copy(fx.dir.path().join("production_queue.sqlite3"), scratch.path().join("queue.sqlite3")).unwrap();
    common::rails(scratch.path(), "test", &["runner", "EncryptedCredentials::Reset.new.call"]);
    let c = Connection::open(scratch.path().join("primary.sqlite3")).unwrap();
    assert!(!deltabadger::app_config::exists(&c, identity::KEY_CONFIG).unwrap(), "Rails deleted every app_configs row");
    identity::initialize(&c, &fx.cipher, now()).unwrap();
    let fx = Fx { dir: fx.dir, c, cipher: fx.cipher, user: fx.user };
    assert_eq!(fx.account(KUCOIN), Some(None));
    assert_eq!((fx.count("exchange_account_credentials"), fx.count("exchange_account_imports"), fx.count("api_keys")), (0, 0, 0));
    assert_eq!(fx.count("account_transactions"), 1);
    assert!(identity::existing_key(&fx.c, &fx.cipher).is_some());
}

// ---- Bindings (record only) ----------------------------------------------------------------------------------------

#[test]
fn concurrent_first_bindings_bind_exactly_one() {
    let fx = Fx::new();
    let exchange = fx.exchange(BINANCE);
    let (path, user) = (fx.dir.path().join("production.sqlite3"), fx.user);
    let threads: Vec<_> = [A, B].into_iter().map(|raw| {
        let path = path.clone();
        std::thread::spawn(move || {
            let c = Connection::open(&path).unwrap();
            deltabadger::store::configure(&c).unwrap();
            let tx = deltabadger::engine::model::immediate(&c).unwrap();
            identity::record_import(&tx, &common::seed::cipher(), user, exchange, Some(&id(raw)), now()).unwrap();
            tx.commit().unwrap();
        })
    }).collect();
    for t in threads { t.join().unwrap(); }
    let fingerprint = fx.account(BINANCE).unwrap().unwrap();
    assert!([fx.fingerprint_of(BINANCE, A), fx.fingerprint_of(BINANCE, B)].contains(&fingerprint));
    assert_eq!((fx.count("exchange_accounts"), fx.count("exchange_account_imports")), (1, 2), "one account, both imports recorded");
}

#[test]
fn a_stale_lookup_after_replacement_or_deletion_binds_nothing() {
    let fx = Fx::new();
    let key = fx.key(BINANCE, 0, "bn-a");
    let due = identity::due(&fx.c, &fx.cipher, now()).unwrap().into_iter().next().unwrap();
    fx.replace(key, "bn-b");
    assert!(!identity::publish(&fx.c, &fx.cipher, &due, &read(A), now()).unwrap(), "the key changed since it was read");
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
    assert!(!identity::publish(&fx.c, &fx.cipher, &due, &read(A), now()).unwrap(), "the key is gone");
    assert_eq!((fx.count("exchange_account_credentials"), fx.count("exchange_accounts")), (0, 0));
}

#[test]
fn a_sole_key_with_history_replaced_by_another_accounts_key_is_accepted_and_observed_as_a_conflict() {
    let (fx, key) = established();
    let keys = fx.keys();
    fx.replace(key, "kc-b");
    fx.observe(key, read(B));
    assert_eq!(identity::credential_standing(&fx.c, key).unwrap(), CredentialStanding::Conflict);
    assert_eq!(fx.standing(KUCOIN), Standing::Conflicted);
    assert_eq!(fx.account(KUCOIN), Some(Some(fx.fingerprint_of(KUCOIN, A))), "the account stays A's");
    assert_eq!((fx.keys(), fx.count("account_transactions")), (keys, 1), "the key works as before; history untouched");
}

#[test]
fn a_deleted_key_leaves_its_account_while_history_exists_and_takes_it_otherwise() {
    let (fx, key) = established();
    let kucoin = fx.exchange(KUCOIN);
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
    identity::forget(&fx.c, key, fx.user, kucoin).unwrap();
    assert_eq!((fx.count("exchange_account_credentials"), fx.account(KUCOIN).is_some()), (0, true), "history keeps the account");
    let again = fx.key(KUCOIN, 0, "kc-a");
    fx.observe(again, read(A));
    assert_eq!(fx.standing(KUCOIN), Standing::Bound, "re-added: the same account");

    let binance_key = fx.key(BINANCE, 0, "bn-a");
    fx.observe(binance_key, read(B));
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [binance_key]).unwrap();
    identity::forget(&fx.c, binance_key, fx.user, fx.exchange(BINANCE)).unwrap();
    assert_eq!(fx.account(BINANCE), None, "no key and no history: the account goes");
}

#[test]
fn legacy_conflicting_key_types_are_marked_conflicted_and_keep_working() {
    let fx = Fx::new();
    let trading = fx.key(BINANCE, 0, "bn-trade");
    let withdrawal = fx.key(BINANCE, 1, "bn-withdraw");
    let keys = fx.keys();
    fx.observe(trading, read(A));
    fx.observe(withdrawal, read(B));
    assert_eq!((fx.standing(BINANCE), identity::credential_standing(&fx.c, withdrawal).unwrap()), (Standing::Conflicted, CredentialStanding::Conflict));
    assert_eq!(identity::credential_standing(&fx.c, trading).unwrap(), CredentialStanding::Bound);
    assert_eq!(fx.keys(), keys);
}

#[test]
fn only_the_reading_key_establishes_and_an_unknown_identity_becomes_known() {
    let fx = Fx::new();
    let withdrawal = fx.key(KUCOIN, 1, "kc-withdraw");
    fx.observe(withdrawal, read(B));
    assert_eq!((fx.account(KUCOIN), fx.standing(KUCOIN)), (Some(None), Standing::Unknown), "a withdrawal key never establishes");
    assert_eq!(identity::credential_standing(&fx.c, withdrawal).unwrap(), CredentialStanding::Seen);
    let trading = fx.key(KUCOIN, 0, "kc-trade");
    fx.observe(trading, read(B));
    assert_eq!(fx.standing(KUCOIN), Standing::Bound, "the reading key establishes it");
}

/// Order does not matter: B's withdrawal key observed first, then the reading key establishes A.
#[test]
fn an_earlier_observation_of_another_account_is_a_conflict_once_the_account_is_established() {
    let mut fx = Fx::new();
    let withdrawal = fx.key(BINANCE, 1, "bn-withdraw");
    fx.observe(withdrawal, read(B));
    let trading = fx.key(BINANCE, 0, "bn-trade");
    fx.observe(trading, read(A));
    assert_eq!(fx.standing(BINANCE), Standing::Conflicted);
    fx.restart();
    assert_eq!((fx.standing(BINANCE), identity::credential_standing(&fx.c, withdrawal).unwrap()), (Standing::Conflicted, CredentialStanding::Conflict));
}

#[test]
fn an_unknown_account_with_two_identities_seen_is_conflicted() {
    let fx = Fx::new();
    let one = fx.key(KUCOIN, 1, "kc-w");
    let two = fx.key(KUCOIN, 2, "kc-r");
    fx.c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [two]).unwrap(); // an incorrect key reads nothing; no reading key
    fx.observe(one, read(A));
    fx.observe(two, read(B));
    assert_eq!((fx.account(KUCOIN), fx.standing(KUCOIN)), (Some(None), Standing::Conflicted));
}

// ---- Conflict persistence ------------------------------------------------------------------------------------------

fn conflicted() -> (Fx, i64, i64) {
    let fx = Fx::new();
    let trading = fx.key(KUCOIN, 0, "kc-a");
    let stray = fx.key(KUCOIN, 2, "kc-b");
    fx.observe(trading, read(A));
    fx.observe(stray, read(B));
    assert_eq!(fx.standing(KUCOIN), Standing::Conflicted);
    (fx, trading, stray)
}

#[test]
fn a_conflict_survives_a_failed_retry_and_a_restart() {
    let (mut fx, _, stray) = conflicted();
    fx.observe(stray, Observed::Failed("http 429; code 429000".into()));
    fx.restart();
    assert_eq!(identity::credential_standing(&fx.c, stray).unwrap(), CredentialStanding::Conflict);
    let code: String = fx.c.query_row("SELECT last_failure_code FROM exchange_account_credentials WHERE api_key_id = ?1", [stray], |r| r.get(0)).unwrap();
    assert_eq!(code, "http 429; code 429000");
}

#[test]
fn a_conflict_survives_a_resave_or_reencryption_a_failed_read_and_a_restart_and_discovery_retries_it() {
    let (mut fx, _, stray) = conflicted();
    fx.reencrypt(stray);
    let later = now() + Duration::hours(2);
    let due: Vec<i64> = identity::due(&fx.c, &fx.cipher, later).unwrap().iter().map(|d| d.api_key_id).collect();
    assert_eq!(due, vec![stray], "the stored version differs: discovery reads it again, well before the weekly interval");
    fx.observe(stray, Observed::Failed("transport".into()));
    fx.restart();
    assert_eq!(identity::credential_standing(&fx.c, stray).unwrap(), CredentialStanding::Conflict);
    assert_eq!(fx.standing(KUCOIN), Standing::Conflicted);
    assert!(identity::due(&fx.c, &fx.cipher, now() + Duration::minutes(30)).unwrap().is_empty(), "a failure waits before the next try");
    assert_eq!(identity::due(&fx.c, &fx.cipher, now() + Duration::hours(2)).unwrap().len(), 1, "then retries");
}

#[test]
fn replacing_the_stray_key_with_new_material_clears_the_conflict() {
    let (fx, _, stray) = conflicted();
    fx.replace(stray, "kc-a-again");
    fx.observe(stray, read(A));
    assert_eq!(fx.standing(KUCOIN), Standing::Bound);
    let (fx, _, stray) = conflicted();
    fx.replace(stray, "kc-c");
    fx.observe(stray, Observed::Failed("http 500".into()));
    assert_eq!((identity::credential_standing(&fx.c, stray).unwrap(), fx.standing(KUCOIN)), (CredentialStanding::Unobserved, Standing::Bound),
               "a replacement's failed read drops the old observation");
}

#[test]
fn deleting_the_stray_key_clears_the_conflict() {
    let (fx, _, stray) = conflicted();
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [stray]).unwrap();
    assert_eq!(fx.standing(KUCOIN), Standing::Bound, "a key that is gone is no current credential");
}

// ---- Imports ---------------------------------------------------------------------------------------------------------

fn import_file(text: &str) -> Result<(), &'static str> {
    match run::read("binance", text.as_bytes(), Some("+00:00")) { Ok(_) => Ok(()), Err(run::Answer::Error(e)) => Err(e), Err(_) => Err("other") }
}

#[test]
fn both_binance_header_spellings_are_read_and_bad_ids_refuse_the_file_even_on_skipped_rows() {
    let rows = "1,2026-08-01 10:00:00,Spot,Deposit,USDT,1,\n1,2026-08-01 11:00:00,Spot,transfer_in,USDT,1,\n";
    for header in ["User ID", "User_ID"] { assert_eq!(import_file(&format!("{header},Time,Account,Operation,Coin,Change,Remark\n{rows}")), Ok(()), "{header}"); }
    let h = "User ID,Time,Account,Operation,Coin,Change,Remark\n";
    let deposit = "1,2026-08-01 10:00:00,Spot,Deposit,USDT,1,\n";
    for (name, file) in [
        ("masked on a deliberately skipped row", format!("{h}{deposit}1****9,2026-08-01 11:00:00,Spot,transfer_in,USDT,1,\n")),
        ("mixed on a row with no operation", format!("{h}{deposit}2,2026-08-01 11:00:00,Spot,,USDT,1,\n")),
        ("mixed on an unknown operation", format!("{h}{deposit}2,2026-08-01 11:00:00,Spot,Teleport,USDT,1,\n")),
        ("blank", format!("{h}{deposit},2026-08-01 11:00:00,Spot,Deposit,USDT,1,\n")),
        ("no id column", "Time,Account,Operation,Coin,Change,Remark\n2026-08-01 10:00:00,Spot,Deposit,USDT,1,\n".to_string()),
    ] {
        assert_eq!(import_file(&file), Err("malformed"), "{name}");
    }
    assert_eq!(import_file(h), Err("unreadable"), "a file with no record answers as before");
}

#[test]
fn an_import_establishes_the_account_and_later_keys_are_observed_against_it() {
    let fx = Fx::new();
    let trading = fx.key(BINANCE, 0, "bn-a");
    let text = fixture()["binance_export"].as_str().unwrap().to_string();
    let answer = run::import(&fx.c, &fx.cipher, fx.user, "binance", text.as_bytes(), Some("+00:00"), None, now()).unwrap();
    assert!(matches!(answer, run::Answer::Written { imported: 1, .. }), "{answer:?}");
    assert_eq!(fx.account(BINANCE), Some(Some(fx.fingerprint_of(BINANCE, "354937868"))), "the import established it");
    let keys = fx.keys();
    fx.observe(trading, read("354937868"));
    assert_eq!(identity::credential_standing(&fx.c, trading).unwrap(), CredentialStanding::Bound);
    let other = fx.key(BINANCE, 2, "bn-other");
    fx.observe(other, read(B));
    assert_eq!((identity::credential_standing(&fx.c, other).unwrap(), fx.standing(BINANCE)), (CredentialStanding::Conflict, Standing::Conflicted));
    assert_eq!(fx.keys()[..1], keys[..], "accepted: the keys work as before");
}

#[test]
fn another_accounts_import_and_credential_are_recorded_and_survive_a_restart() {
    let (mut fx, _) = established();
    let stray = fx.key(KUCOIN, 1, "kc-b");
    fx.import(KUCOIN, Some(B));
    fx.observe(stray, read(B));
    fx.restart();
    let imported: Vec<String> = fx.c.prepare("SELECT fingerprint FROM exchange_account_imports ORDER BY id").unwrap()
        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(imported, [fx.fingerprint_of(KUCOIN, A), fx.fingerprint_of(KUCOIN, B)]);
    assert_eq!((identity::credential_standing(&fx.c, stray).unwrap(), fx.account(KUCOIN)), (CredentialStanding::Conflict, Some(Some(fx.fingerprint_of(KUCOIN, A)))));
}

// ---- The AF1 venues' reads, against recorded-shape answers ------------------------------------------------------------

#[test]
fn kucoins_uid_is_its_exports_uid_and_binances_is_its_exports_user_id() {
    let f = fixture();
    let export = |name: &str, columns: &[&str]| identity::export_account_id(&csv::Table::parse(f[name].as_str().unwrap()).unwrap(), columns).unwrap().unwrap();
    assert_eq!(identity::kucoin_answer(200, &f["kucoin_api_key"].to_string()), Observed::Read(export("kucoin_export", &identity::KUCOIN_EXPORT_COLUMNS)));
    assert_eq!(identity::binance_answer(200, &f["binance_account"].to_string()), Observed::Read(export("binance_export", &identity::BINANCE_EXPORT_COLUMNS)));
    let masked = f["kucoin_export"].as_str().unwrap().replace("165111215,mainAccount,6700000000000000000a0002", "1651****15,mainAccount,6700000000000000000a0002");
    assert!(identity::export_account_id(&csv::Table::parse(&masked).unwrap(), &identity::KUCOIN_EXPORT_COLUMNS).is_err(), "a masked UID is a malformed export");
}

fn kucoin_credentials() -> Credentials { Credentials { key: "kc-identity-key".into(), secret: "kc-secret".into(), passphrase: Some("kc-phrase".into()), redaction_values: vec![] } }

/// The production transport: KuCoin's answer echoes the key, which the transport scrubs from the whole body, and the id is
/// still read; error and malformed answers come to a failure that holds no id, and the logged line holds none either.
#[tokio::test(flavor = "current_thread")]
async fn the_production_transport_reads_the_id_before_it_scrubs_and_failures_carry_no_id() {
    use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "kc-identity-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture()["kucoin_api_key"].clone())).mount(&server).await;
    let creds = kucoin_credentials();
    assert_eq!(identity::read_live(KUCOIN, &server.uri(), deltabadger::venue::http::client(), &creds).await, Some(read(A)));
    let transport = deltabadger::venue::http::ReqwestTransport::kucoin(deltabadger::venue::http::client(), creds.key.clone(), creds.secret.clone(), "kc-phrase".into());
    let request = identity::request(KUCOIN, &transport, &server.uri(), &creds.secret).unwrap();
    use deltabadger::venue::http::Transport;
    let (settled, observed) = transport.send_observed(&request, &identity::kucoin_answer).await.unwrap();
    assert_eq!((observed, settled.body.as_str()), (read(A), deltabadger::crypto::VENUE_TEXT_REDACTED), "the body that left the boundary was scrubbed whole");

    for (status, body) in [(401, json!({"code": "400003", "msg": format!("uid {A} has no key")}).to_string()), (200, format!("{{\"uid\": {A}")), (502, format!("<p>{A}</p>"))] {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).respond_with(ResponseTemplate::new(status).set_body_string(body.clone())).mount(&server).await;
        let observed = identity::read_live(KUCOIN, &server.uri(), deltabadger::venue::http::client(), &creds).await.unwrap();
        assert!(matches!(&observed, Observed::Failed(code) if !code.contains(A)), "{status}: {observed:?}");
        let line = transport.failure_line(&deltabadger::venue::http::HttpResponse { status, body });
        assert!(!line.contains(A), "{line}");
    }
}

/// D-ACCOUNT-ID-READ: KuCoin's and Binance's key checks make one more signed read, only for a key they judged correct;
/// Binance.US's identity is its check's own account read.
#[tokio::test(flavor = "current_thread")]
async fn key_checks_read_the_identity_of_a_correct_key_only() {
    use deltabadger::web::settings::key_check;
    use deltabadger::web::settings::validator::Validity;
    use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("DELETE")).and(path("/api/v1/orders/000000000000000000000000")).and(header("KC-API-KEY", "kc-identity-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": "200000", "data": {"cancelledOrderIds": []}}))).mount(&server).await;
    Mock::given(method("DELETE")).and(path("/api/v1/orders/000000000000000000000000")).and(header("KC-API-KEY", "kc-wrong"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": "400003", "msg": "KC-API-KEY not exists"}))).mount(&server).await;
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).respond_with(ResponseTemplate::new(200).set_body_json(fixture()["kucoin_api_key"].clone())).mount(&server).await;
    let flags = json!({"ipRestrict": true, "enableFixApiTrade": false, "enableFixReadOnly": false, "enableFutures": false, "enableInternalTransfer": false,
        "enableMargin": false, "enablePortfolioMarginTrading": false, "enableReading": true, "enableSpotAndMarginTrading": true, "enableVanillaOptions": false,
        "enableWithdrawals": false, "permitsUniversalTransfer": false});
    Mock::given(method("GET")).and(path("/sapi/v1/account/apiRestrictions")).respond_with(ResponseTemplate::new(200).set_body_json(flags)).mount(&server).await;
    Mock::given(method("GET")).and(path(identity::BINANCE_PATH)).respond_with(ResponseTemplate::new(200).set_body_json(fixture()["binance_account"].clone())).mount(&server).await;
    Mock::given(method("DELETE")).and(path("/api/v3/order")).respond_with(ResponseTemplate::new(400).set_body_json(json!({"code": -2011, "msg": "Unknown order sent."}))).mount(&server).await;
    let base = server.uri();
    let check = |venue: &'static str, creds: Credentials| { let base = base.clone(); async move { key_check::check_identified(venue, &base, deltabadger::venue::http::client(), &creds, 0).await } };

    let (validity, observed) = check(KUCOIN, kucoin_credentials()).await;
    assert!(matches!(validity, Validity::Correct) && observed == Some(read(A)));
    let (validity, observed) = check(KUCOIN, Credentials { key: "kc-wrong".into(), ..kucoin_credentials() }).await;
    assert!(matches!(validity, Validity::Incorrect) && observed.is_none(), "an incorrect key is not read");
    let binance = Credentials { key: "bn-key".into(), secret: "bn-secret".into(), passphrase: None, redaction_values: vec![] };
    let (validity, observed) = check(BINANCE, binance.clone()).await;
    assert!(matches!(validity, Validity::Correct) && observed == Some(read("354937868")));
    let (validity, observed) = check(BINANCE_US, binance).await;
    assert!(matches!(validity, Validity::Correct) && observed == Some(read("354937868")));
    let sent: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| format!("{} {}", r.method, r.url.path())).collect();
    assert_eq!(sent, ["DELETE /api/v1/orders/000000000000000000000000", "GET /api/v1/user/api-key", "DELETE /api/v1/orders/000000000000000000000000",
                      "GET /sapi/v1/account/apiRestrictions", "GET /api/v3/account", "GET /api/v3/account", "DELETE /api/v3/order"],
               "one identity read each for KuCoin and Binance; none more for Binance.US");
}

// ---- Discovery -----------------------------------------------------------------------------------------------------

/// After an upgrade, with no user action and across a restart, an idle withdrawal key of another account is observed
/// (all key types, through the job's own reads) and classified as a conflict.
#[tokio::test(flavor = "current_thread")]
async fn discovery_observes_idle_keys_of_every_type_and_survives_a_restart() {
    use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let answer = |uid: &str| { let mut a = fixture()["kucoin_api_key"].clone(); a["data"]["uid"] = json!(uid.parse::<u64>().unwrap()); a };
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "kc-a")).respond_with(ResponseTemplate::new(200).set_body_json(answer(A))).mount(&server).await;
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "kc-b")).respond_with(ResponseTemplate::new(200).set_body_json(answer(B))).mount(&server).await;
    let mut fx = Fx::new();
    let trading = fx.key(KUCOIN, 0, "kc-a");
    let idle = fx.key(KUCOIN, 1, "kc-b");
    fx.key(deltabadger::sync::ALPACA, 0, "alpaca"); // a venue without a confirmed identity is never read
    assert_eq!(identity::JOB, "account_identity");
    let mut passes = 0;
    loop {
        let due = identity::due(&fx.c, &fx.cipher, now()).unwrap();
        if due.is_empty() { break; }
        passes += 1;
        for d in due {
            let observed = identity::read_live(&d.exchange_type, &server.uri(), deltabadger::venue::http::client(), &d.credentials).await.unwrap();
            assert!(identity::publish(&fx.c, &fx.cipher, &d, &observed, now()).unwrap());
        }
        fx.restart();
    }
    assert_eq!(passes, 1, "every credential once, then nothing is due until the weekly interval");
    assert_eq!((fx.standing(KUCOIN), identity::credential_standing(&fx.c, idle).unwrap(), identity::credential_standing(&fx.c, trading).unwrap()),
               (Standing::Conflicted, CredentialStanding::Conflict, CredentialStanding::Bound));
    assert_eq!(identity::due(&fx.c, &fx.cipher, now() + Duration::days(8)).unwrap().len(), 2, "weekly");
}

/// On an upgraded install (no identity key yet), credentials whose reads fail back off like any other failure, so the
/// next run reaches the credentials behind them and the first healthy one bootstraps the key.
#[tokio::test(flavor = "current_thread")]
async fn failing_reads_before_the_identity_key_exists_back_off_so_discovery_reaches_later_credentials() {
    use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let mut ok = fixture()["kucoin_api_key"].clone();
    ok["data"]["uid"] = json!(A.parse::<u64>().unwrap());
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).and(header("KC-API-KEY", "kc-ok")).respond_with(ResponseTemplate::new(200).set_body_json(ok)).with_priority(1).mount(&server).await;
    Mock::given(method("GET")).and(path(identity::KUCOIN_PATH)).respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": "400003", "msg": "KC-API-KEY not exists"}))).mount(&server).await;
    let fx = Fx::new();
    for n in 0..10 { fx.key(KUCOIN, 1, &format!("kc-fail-{n}")); }
    let healthy = fx.key(KUCOIN, 0, "kc-ok");
    for run in 0..2 {
        for d in identity::due(&fx.c, &fx.cipher, now() + Duration::minutes(run)).unwrap() {
            let observed = identity::read_live(&d.exchange_type, &server.uri(), deltabadger::venue::http::client(), &d.credentials).await.unwrap();
            assert!(identity::publish(&fx.c, &fx.cipher, &d, &observed, now() + Duration::minutes(run)).unwrap());
        }
    }
    assert_ne!(identity::credential_standing(&fx.c, healthy).unwrap(), CredentialStanding::Unobserved, "the healthy credential is observed within two runs");
    assert!(identity::existing_key(&fx.c, &fx.cipher).is_some(), "and bootstraps the identity key");
}

struct Egress(String);
impl deltabadger::sync::jobs::Connect for Egress {
    type T = deltabadger::venue::http::ReqwestTransport;
    fn connect(&self, c: &Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(), c.key.clone(), c.secret.clone()),
                                                     deltabadger::venue::alpaca::Urls { trading: self.0.clone(), data: self.0.clone() })
    }
    fn key_check_egress(&self, _: &str, _: Option<String>) -> Result<(String, reqwest::Client), String> { Ok((self.0.clone(), deltabadger::venue::http::client())) }
}

/// A stored proxy setting that cannot be read is never taken for "no proxy": the credential's read is not sent this run,
/// and the failure is recorded with its backoff.
#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_proxy_setting_sends_no_identity_read_and_records_a_failure() {
    use deltabadger::jobs::{Cx, Db, Job};
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
    let fx = Fx::new();
    let key = fx.key(KUCOIN, 0, "kc-a");
    // Encrypted under another instance's keys: stored, and unreadable here.
    let other = Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-instance").unwrap());
    fx.c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('proxy_kucoin', ?1, '2026-01-01', '2026-01-01')", [other.encrypt("http://proxy.invalid:3128")]).unwrap();
    assert!(deltabadger::app_config::get(&fx.c, &fx.cipher, "proxy_kucoin").is_err());
    let clock = deltabadger::engine::FixedClock(now());
    let cx = Cx { db: Db::new(Connection::open(fx.dir.path().join("production.sqlite3")).unwrap(), fx.cipher.clone()), clock: &clock, wakers: Default::default() };
    identity::Discovery { venues: Egress(server.uri()) }.run(cx, vec![]).await;
    server.verify().await;
    let failed: Option<String> = fx.c.query_row("SELECT last_failure_code FROM exchange_account_credentials WHERE api_key_id = ?1", [key], |r| r.get(0)).unwrap();
    assert_eq!(failed.as_deref(), Some("proxy unreadable"));
    assert!(identity::due(&fx.c, &fx.cipher, now()).unwrap().is_empty(), "the failure backs off");
}
