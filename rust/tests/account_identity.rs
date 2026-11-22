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
        let dir = common::baseline_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        deltabadger::store::configure(&c).unwrap();
        c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('o@example.com', 'x', 1, '2026-01-01', '2026-01-01')", []).unwrap();
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
    /// The same key material in a fresh envelope: 2.x's or Rust's re-save of an unchanged key.
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
    let dir = common::v2_install_unmigrated();
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

/// The actual 2.70.21 recovery (EncryptedCredentials::Reset, as `deltabadger:encryption:reset` runs it, in the published
/// image: common::v2) on the install Rust made, then Rust's start.
#[test]
fn rust_handles_the_reset_v2_actually_makes() {
    if !common::v2::enabled() { return; }
    let (fx, _) = established();
    drop(fx.c);
    let scratch = tempfile::tempdir().unwrap();
    let out = common::v2::v2(fx.dir.path(), scratch.path(), "engine-test-secret", &["runner", "EncryptedCredentials::Reset.new.call"]);
    assert!(out.status.success(), "2.70.21's reset: {}", String::from_utf8_lossy(&out.stderr));
    let c = Connection::open(fx.dir.path().join("production.sqlite3")).unwrap();
    assert!(!deltabadger::app_config::exists(&c, identity::KEY_CONFIG).unwrap(), "2.x deleted every app_configs row");
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
    let account = identity::Account::of_key(&fx.c, key).unwrap().unwrap();
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
    identity::forget(&fx.c, key, &account, chrono::Utc::now()).unwrap();
    assert_eq!(account.exchange_id, kucoin);
    assert_eq!((fx.count("exchange_account_credentials"), fx.account(KUCOIN).is_some()), (0, true), "history keeps the account");
    let again = fx.key(KUCOIN, 0, "kc-a");
    fx.observe(again, read(A));
    assert_eq!(fx.standing(KUCOIN), Standing::Bound, "re-added: the same account");

    let binance_key = fx.key(BINANCE, 0, "bn-a");
    fx.observe(binance_key, read(B));
    let account = identity::Account::of_key(&fx.c, binance_key).unwrap().unwrap();
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [binance_key]).unwrap();
    identity::forget(&fx.c, binance_key, &account, chrono::Utc::now()).unwrap();
    // #710: numbers are never reused, so the row stays, its identity cleared (what delete-then-recreate left before).
    let row: (i64, Option<String>, Option<Vec<u8>>, Option<String>) = fx.c.query_row("SELECT number, fingerprint, account_id_enc, removed_at FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2",
        [fx.user, fx.exchange(BINANCE)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!(row, (1, None, None, None), "no key and no history: the account stays, unknown and not tombstoned");
    let again = fx.key(BINANCE, 0, "bn-b");
    fx.observe(again, read(A));
    assert_eq!((fx.account(BINANCE), fx.count("exchange_accounts")), (Some(Some(fx.fingerprint_of(BINANCE, A))), 2), "a reconnect fills it: still one account");
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
    // #710 7a (Codex branch r1): the fence stays, unconfirmed, until a read names the account.
    assert_eq!((identity::credential_standing(&fx.c, stray).unwrap(), fx.standing(KUCOIN)), (CredentialStanding::Conflict, Standing::Conflicted),
               "a replacement's failed read leaves the key unconfirmed, still fenced");
    fx.observe(stray, read(A));
    assert_eq!(fx.standing(KUCOIN), Standing::Bound, "a read naming the account lifts it");
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
    let lock=deltabadger::lease::lock(&deltabadger::store::Paths::from_env(&|_|None,fx.dir.path()),now()).unwrap();
    deltabadger::lease::claim(&lock,&fx.c,&fx.cipher,"test",now()).unwrap();
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

fn kucoin_credentials() -> Credentials { Credentials { ibkr: None, key: "kc-identity-key".into(), secret: "kc-secret".into(), passphrase: Some("kc-phrase".into()), redaction_values: vec![] } }

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
    let binance = Credentials { ibkr: None, key: "bn-key".into(), secret: "bn-secret".into(), passphrase: None, redaction_values: vec![] };
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

// ---- #710 slice 1: numbered accounts, eager rows, the resolver --------------------------------------------------------

const V710: &str = "20261119000001";
fn paths(dir: &std::path::Path) -> deltabadger::store::Paths { deltabadger::store::Paths::from_env(&|_| None, dir) }
/// The registry of the build before #710 slice 1.
fn before_710() -> deltabadger::store::Registry<'static> {
    use deltabadger::store::{Registry, REGISTRY, RUST_MIGRATIONS};
    let m = RUST_MIGRATIONS.iter().position(|m| m.version >= V710).unwrap();
    let t = REGISTRY.tables.iter().position(|(v, _)| *v >= V710).unwrap();
    Registry { migrations: &RUST_MIGRATIONS[..m], tables: &REGISTRY.tables[..t], retired: REGISTRY.retired }
}
type AccountRow = (i64, i64, i64, i64, Option<String>, Option<String>);
fn accounts(c: &Connection) -> Vec<AccountRow> {
    c.prepare("SELECT id, user_id, exchange_id, number, fingerprint, removed_at FROM exchange_accounts ORDER BY user_id, exchange_id, number").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

/// A 2.x install the build before #710 slice 1 migrated, with a row in every table that names a (user, exchange), then
/// this build's start: each existing account keeps its id and fingerprint as number 1, every other (user, exchange)
/// named anywhere gets number 1, once.
#[test]
fn the_upgrade_numbers_every_account_1_and_adds_number_1_wherever_an_exchange_is_named() {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let lock = deltabadger::lease::lock(&p, Utc::now()).unwrap();
    deltabadger::store::migrate_with(&p, &lock, &before_710()).unwrap();
    let c = Connection::open(&p.primary).unwrap();
    let t = "'2026-01-01'";
    // Foreign keys off: only the (user, exchange) pairs matter here.
    c.execute_batch(&format!("PRAGMA foreign_keys = OFF;
        INSERT INTO exchange_accounts (id, user_id, exchange_id, fingerprint, account_id_enc, created_at, updated_at) VALUES (7, 1, 1, 'fp-1', x'01', {t}, {t});
        INSERT INTO api_keys (user_id, exchange_id, created_at, updated_at) VALUES (1, 1, {t}, {t}), (1, 2, {t}, {t});
        INSERT INTO bots (id, user_id, exchange_id, created_at, updated_at) VALUES (1, 1, 3, {t}, {t}), (2, 1, NULL, {t}, {t}), (3, NULL, 9, {t}, {t});
        INSERT INTO transactions (bot_id, exchange_id, created_at, updated_at) VALUES (1, 4, {t}, {t}), (1, 3, {t}, {t});
        INSERT INTO rules (user_id, exchange_id, type, created_at, updated_at) VALUES (1, 5, 'Rules::Withdrawal', {t}, {t}), (1, NULL, 'Rules::Other', {t}, {t});
        INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, created_at, updated_at) VALUES (2, 1, 0, 'BTC', 1, {t}, {t}, {t}), (1, 2, 0, 'BTC', 1, {t}, {t}, {t});
        INSERT INTO account_balances (user_id, exchange_id, asset_id, synced_at, created_at, updated_at) VALUES (2, 2, 1, {t}, {t}, {t});
        INSERT INTO portfolio_venue_snapshots (user_id, exchange_id, date, created_at, updated_at) VALUES (2, 3, {t}, {t}, {t});
        INSERT INTO exchange_account_imports (user_id, exchange_id, fingerprint, account_id_enc, observed_at) VALUES (2, 4, 'fp-i', x'02', {t});
        INSERT INTO import_rows (user_id, exchange_id, row_id, account_type, currency, side, amount, fee, transacted_at, remark, row_type, created_at)
          VALUES (2, 5, 'r', 'a', 'BTC', 'in', '1', '0', {t}, '', 't', {t});
        INSERT INTO import_cutoffs (user_id, exchange_id, class, cutoff_at, updated_at) VALUES (2, 6, 'c', {t}, {t});
        INSERT INTO kucoin_coverage (user_id, exchange_id, source, start_at, end_at, updated_at) VALUES (3, 1, 's', {t}, {t}, {t});
        INSERT INTO kucoin_withheld_fills (user_id, exchange_id, tx_id, transacted_at, entry, created_at) VALUES (3, 2, 'x', {t}, '{{}}', {t});
        INSERT INTO rust_api_orders (id, user_id, side, amount, amount_type, supplied, state, created_at, send_by) VALUES (5, 3, 'buy', '1', 'quote', '{{}}', 'queued', {t}, {t});
        INSERT INTO rust_api_order_standalone (order_id, exchange_id, ticker_id, order_type, surface) VALUES (5, 3, 1, 'market', 'rest');")).unwrap();
    assert_eq!(deltabadger::store::migrate(&p, &lock).unwrap()[0], V710);
    let rows = accounts(&c);
    assert_eq!(rows[0], (7, 1, 1, 1, Some("fp-1".into()), None), "the existing account keeps its id and fingerprint");
    let named: Vec<(i64, i64, i64, Option<String>)> = rows.iter().map(|(_, u, e, n, f, _)| (*u, *e, *n, f.clone())).collect();
    let want: Vec<(i64, i64, i64, Option<String>)> = [(1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (2, 1), (2, 2), (2, 3), (2, 4), (2, 5), (2, 6), (3, 1), (3, 2), (3, 3)]
        .into_iter().map(|(u, e)| (u, e, 1, ((u, e) == (1, 1)).then(|| "fp-1".to_string()))).collect();
    assert_eq!(named, want, "number 1 for each named (user, exchange), once; a NULL user or exchange names none");
    assert_eq!((count(&c, "exchange_account_keys"), count(&c, "exchange_account_bots")), (0, 0), "no membership yet: every subject is its number 1's");
    assert_eq!(deltabadger::store::check(&p).unwrap().pending_rust, Vec::<String>::new());
    assert_eq!(deltabadger::store::migrate(&p, &lock).unwrap(), Vec::<String>::new(), "a restart applies nothing");
    assert_eq!(identity::fill(&c, now()).unwrap(), 0, "FILL agrees with the migration");
    assert_eq!(accounts(&c), rows);
}
fn count(c: &Connection, table: &str) -> i64 { c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap() }

/// 2.x's rows after a handback: FILL gives each newly named (user, exchange) its number 1 and leaves the rest.
#[test]
fn fill_adds_number_1_for_what_2x_added() {
    let fx = Fx::new();
    let kucoin = fx.key(KUCOIN, 0, "kc-a");
    let before = identity::ensure_account(&fx.c, fx.user, fx.exchange(KUCOIN), now()).unwrap();
    fx.key(BINANCE, 0, "bn-a"); // as 2.x saves it: no account
    assert_eq!(identity::fill(&fx.c, now()).unwrap(), 1);
    assert_eq!(identity::account_of_key(&fx.c, kucoin).unwrap(), Some(before));
    assert_eq!(identity::accounts_of(&fx.c, fx.user, fx.exchange(BINANCE)).unwrap().iter().map(|(_, n)| *n).collect::<Vec<_>>(), [1]);
    assert_eq!(identity::fill(&fx.c, now()).unwrap(), 0);
}

/// Two accounts on one exchange, inserted directly (no path makes a second yet): number 1 with a key and a bot of its
/// own (no membership row), number 2 with a key and a bot by membership, number 3 a tombstone, and another user's account.
struct Two { fx: Fx, exchange: i64, first: i64, second: i64, third: i64, key1: i64, key2: i64, bot1: i64, bot2: i64, foreign: i64 }
fn two() -> Two {
    let fx = Fx::new();
    let exchange = fx.exchange(KUCOIN);
    let other = fx.exchange(BINANCE);
    let account = |user: i64, exchange: i64, number: i64, removed: Option<&str>| {
        fx.c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, removed_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, '2026-01-01', '2026-01-01')",
                     rusqlite::params![user, exchange, number, removed]).unwrap();
        fx.c.last_insert_rowid()
    };
    let (first, second, third) = (account(fx.user, exchange, 1, None), account(fx.user, exchange, 2, None), account(fx.user, exchange, 3, Some("2026-02-01")));
    fx.c.execute("INSERT INTO users (email, encrypted_password, admin, created_at, updated_at) VALUES ('x@example.com', 'x', 0, '2026-01-01', '2026-01-01')", []).unwrap();
    let foreign = account(fx.c.last_insert_rowid(), exchange, 1, None);
    account(fx.user, other, 1, None);
    let (key1, key2) = (fx.key(KUCOIN, 0, "kc-1"), fx.key(KUCOIN, 0, "kc-2"));
    let bot = |label: &str| {
        fx.c.execute("INSERT INTO bots (user_id, exchange_id, type, label, created_at, updated_at) VALUES (?1, ?2, 'Bots::DcaSingleAsset', ?3, '2026-01-01', '2026-01-01')",
                     rusqlite::params![fx.user, exchange, label]).unwrap();
        fx.c.last_insert_rowid()
    };
    let (bot1, bot2) = (bot("one"), bot("two"));
    fx.c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [key2, second]).unwrap();
    fx.c.execute("INSERT INTO exchange_account_bots (bot_id, account_id) VALUES (?1, ?2)", [bot2, second]).unwrap();
    Two { fx, exchange, first, second, third, key1, key2, bot1, bot2, foreign }
}

#[test]
fn the_resolver_takes_the_membership_row_and_falls_back_to_number_1() {
    let t = two();
    let c = &t.fx.c;
    assert_eq!((identity::account_of_key(c, t.key1).unwrap(), identity::account_of_key(c, t.key2).unwrap()), (Some(t.first), Some(t.second)));
    assert_eq!((identity::account_of_bot(c, t.bot1).unwrap(), identity::account_of_bot(c, t.bot2).unwrap()), (Some(t.first), Some(t.second)));
    assert_eq!((identity::key_of(c, t.first, 0).unwrap(), identity::key_of(c, t.second, 0).unwrap()), (Some(t.key1), Some(t.key2)), "each account its own key");
    assert_eq!((identity::key_of(c, t.first, 1).unwrap(), identity::key_of(c, t.third, 0).unwrap()), (None, None), "no withdrawal key; nothing on the tombstone");
    assert_eq!(identity::key_of(c, t.foreign, 0).unwrap(), None, "another user's account holds none of this user's keys");
    assert_eq!(identity::accounts_of(c, t.fx.user, t.exchange).unwrap(), [(t.first, 1), (t.second, 2)], "live ones, by number: the tombstone is left out");
    assert_eq!((identity::account_of_key(c, 999_999).unwrap(), identity::account_of_bot(c, 999_999).unwrap()), (None, None));
}

#[test]
fn a_membership_row_naming_another_users_or_exchanges_account_resolves_to_nothing() {
    let t = two();
    let c = &t.fx.c;
    let binance: i64 = c.query_row("SELECT id FROM exchange_accounts WHERE exchange_id = ?1", [t.fx.exchange(BINANCE)], |r| r.get(0)).unwrap();
    for account in [t.foreign, binance] {
        c.execute("UPDATE exchange_account_keys SET account_id = ?1 WHERE api_key_id = ?2", [account, t.key2]).unwrap();
        c.execute("UPDATE exchange_account_bots SET account_id = ?1 WHERE bot_id = ?2", [account, t.bot2]).unwrap();
        assert_eq!((identity::account_of_key(c, t.key2).unwrap(), identity::account_of_bot(c, t.bot2).unwrap()), (None, None), "{account}");
        assert_eq!(identity::key_of(c, account, 0).unwrap(), None, "{account}");
    }
}

/// With 2+ accounts the readers of today keep the lowest live number; a tombstoned number 1 is skipped, and an exchange
/// whose every account is a tombstone gets the next number, never a reused one.
#[test]
fn ensure_account_and_standing_read_the_lowest_live_number_and_never_reuse_one() {
    let t = two();
    let c = &t.fx.c;
    assert_eq!(identity::ensure_account(c, t.fx.user, t.exchange, now()).unwrap(), t.first);
    c.execute("UPDATE exchange_accounts SET removed_at = '2026-03-01' WHERE id = ?1", [t.first]).unwrap();
    assert_eq!(identity::ensure_account(c, t.fx.user, t.exchange, now()).unwrap(), t.second);
    c.execute("UPDATE exchange_accounts SET removed_at = '2026-03-01' WHERE id = ?1", [t.second]).unwrap();
    let made = identity::ensure_account(c, t.fx.user, t.exchange, now()).unwrap();
    let number: i64 = c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [made], |r| r.get(0)).unwrap();
    assert_eq!(number, 4, "after 1, 2 and the tombstone 3");
    assert_eq!(identity::standing(c, t.fx.user, t.exchange).unwrap(), Standing::Unknown);
}

/// A deleted key's membership goes with it.
#[test]
fn forget_drops_the_keys_membership_and_keeps_every_account() {
    let t = two();
    let before = accounts(&t.fx.c);
    let account = identity::Account::of_key(&t.fx.c, t.key2).unwrap().unwrap();
    t.fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [t.key2]).unwrap();
    identity::forget(&t.fx.c, t.key2, &account, chrono::Utc::now()).unwrap();
    assert_eq!((count(&t.fx.c, "exchange_account_keys"), accounts(&t.fx.c)), (0, before));
}

// ---- #710 slice 2: keys by account, on the two-account fixture ---------------------------------------------------------

fn fingerprints(t: &Two) -> (Option<String>, Option<String>) {
    let fp = |id: i64| t.fx.c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    (fp(t.first), fp(t.second))
}

/// An account scopes keys and bots by membership; a (user, exchange) with no row yet is its number 1 to come.
#[test]
fn an_account_scopes_its_keys_and_bots_and_a_number_1_without_a_row_takes_what_has_no_membership() {
    let t = two();
    let c = &t.fx.c;
    let (first, second) = (identity::Account::of_key(c, t.key1).unwrap().unwrap(), identity::Account::of_bot(c, t.bot2).unwrap().unwrap());
    assert_eq!((first.id, second.id), (Some(t.first), Some(t.second)));
    assert_eq!(identity::Account::first(c, t.fx.user, t.exchange).unwrap(), first, "the exchange reads as its number 1");
    let ids = |table: &str, account: &identity::Account| -> Vec<i64> {
        let condition = if table == "api_keys" { account.keys("s") } else { account.bots("s") };
        let mut s = c.prepare(&format!("SELECT s.id FROM {table} s WHERE {condition} ORDER BY s.id")).unwrap();
        s.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!((ids("api_keys", &first), ids("api_keys", &second)), (vec![t.key1], vec![t.key2]));
    assert_eq!((ids("bots", &first), ids("bots", &second)), (vec![t.bot1], vec![t.bot2]));
    assert_eq!((first.number(c).unwrap(), second.number(c).unwrap()), (1, 2));

    let fx = Fx::new();
    let key = fx.key(BINANCE, 0, "bn-a"); // as 2.x saves it: no account row
    let unknown = identity::Account::of_key(&fx.c, key).unwrap().unwrap();
    assert_eq!((unknown, unknown.key(&fx.c, 0).unwrap()), (identity::Account::first(&fx.c, fx.user, fx.exchange(BINANCE)).unwrap(), Some(key)));
    assert_eq!((unknown.id, unknown.number(&fx.c).unwrap()), (None, 1));
    let made = unknown.ensure(&fx.c, now()).unwrap();
    made.add_key(&fx.c, key).unwrap();
    assert_eq!((made.key(&fx.c, 0).unwrap(), fx.count("exchange_account_keys")), (Some(key), 0), "number 1's keys need no membership row");
}

/// A deleted key clears the identity of its own account only, and only when that account has no other key.
#[test]
fn forgetting_a_key_clears_its_own_accounts_identity_only() {
    let t = two();
    let c = &t.fx.c;
    c.execute("UPDATE exchange_accounts SET fingerprint = 'fp-' || number WHERE id IN (?1, ?2)", [t.first, t.second]).unwrap();
    // No bot keeps either account (a bot that does keeps its fingerprint: s7a_removing_the_last_key_keeps_...).
    c.execute_batch("DELETE FROM exchange_account_bots; DELETE FROM bots;").unwrap();
    let delete = |key: i64| {
        let account = identity::Account::of_key(c, key).unwrap().unwrap();
        c.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
        identity::forget(c, key, &account, chrono::Utc::now()).unwrap();
    };
    delete(t.key2);
    assert_eq!(fingerprints(&t), (Some("fp-1".into()), None), "account 2 had no other key; account 1 keeps its own");
    c.execute("UPDATE exchange_accounts SET fingerprint = 'fp-2' WHERE id = ?1", [t.second]).unwrap();
    let other = t.fx.key(KUCOIN, 2, "kc-1-read");
    delete(t.key1);
    assert_eq!(fingerprints(&t), (Some("fp-1".into()), Some("fp-2".into())), "account 1 still has its read-only key");
    delete(other);
    assert_eq!(fingerprints(&t), (None, Some("fp-2".into())));
}

/// Each account has its reading key, and only that key establishes it.
#[test]
fn each_account_is_read_with_its_own_key_and_establishes_from_it() {
    let t = two();
    let c = &t.fx.c;
    let reading = run::reading(c, t.fx.user).unwrap();
    assert_eq!(reading.iter().map(|k| (k.id, k.account_id)).collect::<Vec<_>>(), [(t.key1, Some(t.first)), (t.key2, Some(t.second))]);
    for keys in [deltabadger::sync::ledger_reading_keys(c).unwrap(), deltabadger::sync::balance_reading_keys(c).unwrap()] {
        assert_eq!(keys.iter().filter(|id| [t.key1, t.key2].contains(id)).count(), 2, "{keys:?}");
    }
    t.fx.observe(t.key2, read(B));
    assert_eq!(fingerprints(&t), (None, Some(t.fx.fingerprint_of(KUCOIN, B))), "account 2's key fills account 2");
    assert_eq!(identity::credential_standing(c, t.key2).unwrap(), CredentialStanding::Bound);
    t.fx.observe(t.key1, read(A));
    assert_eq!(fingerprints(&t), (Some(t.fx.fingerprint_of(KUCOIN, A)), Some(t.fx.fingerprint_of(KUCOIN, B))));
}

/// Eager rows know no identity: a start without an identity key writes nothing for them (no reset, no key).
#[test]
fn unknown_eager_accounts_are_no_reset() {
    let mut fx = Fx::new();
    fx.key(KUCOIN, 0, "kc-a");
    identity::fill(&fx.c, now()).unwrap();
    fx.restart();
    assert!(deltabadger::app_config::get_plain(&fx.c, identity::KEY_CONFIG).unwrap().is_none(), "nothing to clear, no key stored");
}

/// A write on a 2.x install Rust has not migrated (no identity tables) goes on: touching its account does nothing.
#[test]
fn touching_an_account_on_an_install_without_identity_tables_writes_nothing() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    identity::touch(&c, 1, 1, now()).unwrap();
    let tables: i64 = c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0);
}

// ---- #710 slice 3: the gate (PLAN-710 §3) ---------------------------------------------------------------------------

/// Every statement in src/ that deletes a membership, or a subject that has one, and why it is allowed: the subject itself
/// goes (or moves, as a whole), never another's membership. A new one must be added here, with its reason.
const MEMBERSHIP_DELETES: &[(&str, &str, &str)] = &[
    ("src/identity.rs", "DELETE FROM exchange_account_keys", "forget: the deleted key's own row"),
    ("src/identity.rs", "DELETE FROM exchange_account_bots", "place_bot: the moved bot's own row, written again for its new account"),
    ("src/rotation.rs", "DELETE FROM exchange_account_keys", "clear: every key is deleted with it"),
    ("src/rotation.rs", "DELETE FROM api_keys", "clear: every key"),
    ("src/web/settings/keys.rs", "DELETE FROM api_keys", "delete: one key, then forget"),
    ("src/engine/rules.rs", "DELETE FROM exchange_account_rules", "refill: number-1 rules 2.x deleted; accounts 2+ are never touched"),
    ("src/engine/api_orders.rs", "DELETE FROM rust_api_order_standalone", "forget, orphans: the order's own row, its account inside it"),
    // #710 Merge (PLAN-710-6 §6.2): the duplicate's subjects move to its survivor or go, in one transaction.
    ("src/merge.rs", "DELETE FROM api_keys", "remove_key: a key of the duplicate the survivor's of its type replaces"),
    ("src/merge.rs", "DELETE FROM exchange_account_keys", "move_key, remove_key: the moved key's row (written again for a survivor 2+), the removed key's"),
    ("src/merge.rs", "DELETE FROM exchange_account_rules", "the staged swap: the survivor's deleted rule, inserted again under the duplicate with its id"),
    ("src/venue_ids.rs", "DELETE FROM exchange_account_transactions", "move_orders: the duplicate's orders into a number 1, which takes their ids in 2.x's column"),
];

#[test]
fn only_the_listed_statements_delete_a_membership_or_its_subject() {
    let pattern = regex::Regex::new(r"DELETE FROM (exchange_account_(?:keys|bots|transactions|rules)|api_keys|bots|transactions|rules|rust_api_order_standalone)\b").unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = std::collections::BTreeSet::new();
    let mut stack = vec![root.join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { stack.push(path); continue; }
            if path.extension().is_none_or(|e| e != "rs") { continue; }
            let text = std::fs::read_to_string(&path).unwrap();
            let file = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            for m in pattern.find_iter(&text) { found.insert((file.clone(), m.as_str().to_string())); }
        }
    }
    let allowed: std::collections::BTreeSet<(String, String)> = MEMBERSHIP_DELETES.iter().map(|(f, s, _)| (f.to_string(), s.to_string())).collect();
    assert_eq!(found, allowed);
}

/// The gate: account 1 and account 2 of one exchange each hold a key, a bot, a bot's order, a withdrawal rule and a standalone
/// order. Every way account 1's can go (a key deleted, the bot deleted and its order gone, the rule deleted in Rust and then
/// in 2.x and refilled, the standalone order forgotten, an encryption reset) leaves account 2's resolving to account 2,
/// never to number 1.
#[test]
fn deleting_one_accounts_subjects_leaves_the_other_accounts_memberships() {
    use deltabadger::engine::{api_orders, rules};
    use deltabadger::identity::Account;
    let dir = common::baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let c = &o.primary;
    let cipher = common::seed::cipher();
    let s = common::seed::seed_kraken(c, &cipher);
    let first = Account::first(c, s.user_id, s.exchange_id).unwrap().ensure(c, now()).unwrap();
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [s.user_id, s.exchange_id]).unwrap();
    let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, 'k', 's', 1, 0, 'now', 'now')", [s.user_id, s.exchange_id]).unwrap();
    let key = c.last_insert_rowid();
    second.add_key(c, key).unwrap();
    let (mut bots, mut orders, mut rule_ids, mut standalone) = (vec![], vec![], vec![], vec![]);
    let (amount, pair) = (deltabadger::v2::BigDec::parse("1").unwrap(), serde_json::Map::new());
    for (n, account) in [first, second].into_iter().enumerate() {
        let bot = common::seed::insert_bot(c, &s, &common::seed::BotSpec { status: 2, ..common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
        identity::place_bot(c, bot, s.user_id, s.exchange_id, Some(account), now()).unwrap();
        let order = common::seed::insert_row(c, &s, bot, s.btc, &json!({"external_id": format!("O-{n}"), "external_status": 2, "created_at": "2026-09-02 10:00:00"}));
        if n == 1 {
            // Relocated as a placement writes it (slice 4): the venue id in the membership, 2.x's column NULL.
            c.execute("UPDATE transactions SET external_id = NULL WHERE id = ?1", [order]).unwrap();
            c.execute("INSERT INTO exchange_account_transactions (transaction_id, account_id, venue_external_id) VALUES (?1, ?2, 'O-1')", [order, second.id.unwrap()]).unwrap();
        }
        bots.push(bot);
        orders.push(order);
        rule_ids.push(rules::create(c, &account, s.btc, "address", "{}", "2026-09-02 10:00:00").unwrap());
        let id = api_orders::insert(c, &api_orders::New::Standalone { user_id: s.user_id, side: deltabadger::venue::Side::Buy, amount: &amount, in_base: false, supplied: &pair,
            standalone: api_orders::Standalone { account, exchange_id: s.exchange_id, ticker_id: s.ticker_id, limit: false, price: None, mcp: false } }, now()).unwrap();
        standalone.push(id);
    }
    let check = |why: &str| {
        assert_eq!(Account::of_bot(c, bots[1]).unwrap(), Some(second), "{why}: the bot");
        assert_eq!(Account::of_order(c, orders[1]).unwrap(), Some(second), "{why}: the order");
        assert_eq!(Account::of_rule(c, rule_ids[1]).unwrap(), Some(second), "{why}: the rule");
        let account: i64 = c.query_row("SELECT account_id FROM rust_api_order_standalone WHERE order_id = ?1", [standalone[1]], |r| r.get(0)).unwrap();
        assert_eq!(Some(account), second.id, "{why}: the standalone order");
        let siblings: Vec<i64> = c.prepare(&format!("SELECT b.id FROM bots b WHERE {} ORDER BY b.id", identity::sibling_bots(c, bots[1]).unwrap())).unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(siblings, [bots[1]], "{why}: account 2's bots are its own");
    };
    check("as made");
    assert_eq!(Account::of_key(c, key).unwrap(), Some(second));
    assert_eq!(Account::of_order(c, orders[0]).unwrap(), Some(first), "no membership row: number 1");
    // Account 1's key deleted, as the settings delete does it.
    c.execute("DELETE FROM api_keys WHERE id = ?1", [s.api_key_id]).unwrap();
    identity::forget(c, s.api_key_id, &first, chrono::Utc::now()).unwrap();
    check("account 1's key deleted");
    assert_eq!(Account::of_key(c, key).unwrap(), Some(second), "account 2's key");
    // Account 1's bot deleted (bots are kept as deleted) and its order gone (an order's membership cascades with it).
    c.execute("UPDATE bots SET status = 3 WHERE id = ?1", [bots[0]]).unwrap();
    c.execute("DELETE FROM transactions WHERE id = ?1", [orders[0]]).unwrap();
    check("account 1's bot and order gone");
    // Account 1's rule deleted in Rust, then by 2.x and refilled at a takeover.
    c.execute("UPDATE exchange_account_rules SET status = 3 WHERE id = ?1", [rule_ids[0]]).unwrap();
    rules::mirror(c, Some(rule_ids[0])).unwrap();
    c.execute("DELETE FROM rules WHERE id = ?1", [rule_ids[0]]).unwrap();
    rules::refill(c).unwrap();
    assert_eq!(Account::of_rule(c, rule_ids[0]).unwrap(), None, "2.x's delete reaches the number-1 rule");
    check("account 1's rule deleted");
    // Account 1's standalone order answered and forgotten.
    c.execute("UPDATE rust_api_orders SET state = 'withdrawn' WHERE id = ?1", [standalone[0]]).unwrap();
    api_orders::forget(c, standalone[0]).unwrap();
    check("account 1's standalone order forgotten");
    // An encryption reset: every key goes, with its membership; nothing else moves.
    deltabadger::rotation::clear(c, now()).unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM exchange_account_keys", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    check("a reset");
}

// ---- #710 slice 3: the ownership migration (20261119000002) ------------------------------------------------------------

const V710_3: &str = "20261119000002";
fn before_710_3() -> deltabadger::store::Registry<'static> {
    use deltabadger::store::{Registry, REGISTRY, RUST_MIGRATIONS};
    let m = RUST_MIGRATIONS.iter().position(|m| m.version >= V710_3).unwrap();
    let t = REGISTRY.tables.iter().position(|(v, _)| *v >= V710_3).unwrap();
    Registry { migrations: &RUST_MIGRATIONS[..m], tables: &REGISTRY.tables[..t], retired: REGISTRY.retired }
}

/// The ownership migration over what the build before it left:
/// - every withdrawal rule naming an exchange reaches the store, one whose (user, exchange) had no account getting its
///   number 1 first; one with no exchange stays 2.x's only, its row untouched;
/// - withdrawal ids: a rule's on its rule's account; recorder ids, which name no user, stay exchange-scoped (NULL), so one
///   venue id on two exchanges with no account at all migrates instead of colliding.
#[test]
fn the_ownership_migration_keeps_every_rule_and_scopes_ids_it_cannot_place_by_exchange() {
    let dir = common::v2_install_unmigrated();
    let p = paths(dir.path());
    let lock = deltabadger::lease::lock(&p, Utc::now()).unwrap();
    deltabadger::store::migrate_with(&p, &lock, &before_710_3()).unwrap();
    let c = Connection::open(&p.primary).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("PRAGMA foreign_keys = OFF;
        INSERT INTO rules (id, user_id, exchange_id, asset_id, type, status, settings, created_at, updated_at) VALUES
          (1, 1, 5, 1, 'Rules::Withdrawal', 1, '{{}}', {t}, {t}), (2, 1, NULL, 1, 'Rules::Withdrawal', 1, '{{}}', {t}, {t}), (3, 1, 6, NULL, 'Rules::Other', 1, '{{}}', {t}, {t});
        DELETE FROM exchange_accounts;
        INSERT INTO rust_withdrawal_ids (exchange_id, venue_id, owner, settled_at, source) VALUES
          (5, 'R', 'rule:1', {t}, 'rust'), (7, 'V', 'recorder:a', {t}, 'rust'), (8, 'V', 'recorder:b', {t}, 'rust');")).unwrap();
    assert_eq!(deltabadger::store::migrate(&p, &lock).unwrap().first().map(String::as_str), Some(V710_3), "the ownership migration, then any later one");
    let number_1: i64 = c.query_row("SELECT id FROM exchange_accounts WHERE user_id = 1 AND exchange_id = 5 AND number = 1", [], |r| r.get(0)).unwrap();
    let store: Vec<(i64, i64)> = c.prepare("SELECT id, account_id FROM exchange_account_rules ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(store, [(1, number_1)], "the withdrawal rule with an exchange, on its new number 1");
    assert_eq!(c.query_row("SELECT count(*) FROM rules", [], |r| r.get::<_, i64>(0)).unwrap(), 3, "2.x's rules untouched");
    let ids: Vec<(Option<i64>, i64, String)> = c.prepare("SELECT account_id, exchange_id, venue_id FROM rust_withdrawal_ids ORDER BY rowid").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(ids, [(Some(number_1), 5, "R".into()), (None, 7, "V".into()), (None, 8, "V".into())]);
    // Still unique per exchange where the account is unknown.
    assert!(c.execute("INSERT INTO rust_withdrawal_ids (account_id, exchange_id, venue_id, owner, settled_at, source) VALUES (NULL, 7, 'V', 'x', 'now', 'rust')", []).is_err());
}

/// #710 review r2 (D): a venue's own export routed to account 2's key fingerprints account 2, never the lowest live account.
#[test]
fn a_native_export_fingerprints_the_account_its_key_belongs_to() {
    let fx = Fx::new();
    let first = fx.key(BINANCE, 0, "bn-a");
    let exchange = fx.exchange(BINANCE);
    let lock = deltabadger::lease::lock(&deltabadger::store::Paths::from_env(&|_| None, fx.dir.path()), now()).unwrap();
    deltabadger::lease::claim(&lock, &fx.c, &fx.cipher, "test", now()).unwrap();
    deltabadger::identity::Account::of_key(&fx.c, first).unwrap().unwrap().ensure(&fx.c, now()).unwrap();
    fx.c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')", [fx.user, exchange]).unwrap();
    let second_account = fx.c.last_insert_rowid();
    let second = fx.key(BINANCE, 0, "bn-b");
    fx.c.execute("INSERT INTO exchange_account_keys (api_key_id, account_id) VALUES (?1, ?2)", [second, second_account]).unwrap();
    let text = fixture()["binance_export"].as_str().unwrap().to_string();
    // 7a rule H3: asked which account (a key id answers only the venue question of a one-account venue), account 2 answered.
    let import = |answer: Option<&str>| run::import(&fx.c, &fx.cipher, fx.user, "binance", text.as_bytes(), Some("+00:00"), answer, now()).unwrap();
    let run::Answer::Ambiguous(choices) = import(None) else { panic!("asked") };
    let answer = import(Some(&choices.iter().find(|c| c.value.starts_with(&format!("a{second_account}~"))).expect("account 2 offered").value));
    assert!(matches!(answer, run::Answer::Written { .. }), "{answer:?}");
    let print = |id: i64| -> Option<String> { fx.c.query_row("SELECT fingerprint FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    let first_account = deltabadger::identity::account_of_key(&fx.c, first).unwrap().unwrap();
    assert_eq!((print(first_account).is_some(), print(second_account).is_some()), (false, true), "the file's account is its key's");
}

/// 7a §0.10, the fence's exit: the identity discovery job still reads a fenced key (identity only: `due` takes every key of
/// an identity venue, fenced or not, and `publish` commits the observation alone). While its read names another account
/// it syncs nothing; the next read naming its account's again lifts the fence. Its material changing makes it due at once;
/// otherwise it is due again after `observe_every`.
#[test]
fn s7_0_10_the_identity_read_still_runs_for_a_fenced_key_and_lifts_the_fence() {
    let (fx, key) = established();
    let reads = |c: &Connection| deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&key);
    assert!(reads(&fx.c));
    fx.replace(key, "kc-b");
    assert!(identity::due(&fx.c, &fx.cipher, now()).unwrap().iter().any(|d| d.api_key_id == key), "a material change is due at once");
    fx.observe(key, read(B));
    assert!(!reads(&fx.c), "fenced: no sync");
    assert!(!identity::due(&fx.c, &fx.cipher, now()).unwrap().iter().any(|d| d.api_key_id == key), "just read");
    let later = now() + identity::observe_every() + Duration::seconds(1);
    let due = identity::due(&fx.c, &fx.cipher, later).unwrap().into_iter().find(|d| d.api_key_id == key).expect("a fenced key is read again");
    let rows = fx.count("account_transactions");
    assert!(identity::publish(&fx.c, &fx.cipher, &due, &read(A), later).unwrap());
    assert!(reads(&fx.c), "a read naming its account's lifts the fence");
    assert_eq!(fx.count("account_transactions"), rows, "the identity read writes no rows");
}

/// #710 7a §9.1 (Codex branch r1: signal.rs:433, api_orders.rs:361, withdrawal.rs:577, sync/jobs.rs:313/389): a version
/// captured before discovery found its key opens another account fails the one final check every claim, send and venue
/// result commits under (`check_credential_result`, `key_is_current`), as a changed credential does, its material
/// unchanged. What the key records about itself (its validation status, its identity read, a job's run) still commits.
#[test]
fn s7a_a_key_found_to_open_another_account_fails_every_final_check() {
    use deltabadger::engine::{model, EngineError};
    let (fx, key) = established();
    let captured = fx.version(key);
    assert!(model::key_is_current(&fx.c, &captured).unwrap());
    fx.observe(key, read(B));
    assert_eq!(fx.version(key), captured, "the material is unchanged");
    assert!(!deltabadger::sync::ledger_reading_keys(&fx.c).unwrap().contains(&key), "fenced");
    assert!(!model::key_is_current(&fx.c, &captured).unwrap(), "a fenced key is no current key");
    assert!(matches!(model::credential_write(&fx.c, &Some(captured.clone()), |_| Ok(())), Err(EngineError::CredentialsChanged)), "the final check refuses");
    let tx = fx.c.unchecked_transaction().unwrap();
    assert!(model::check_credential_record(&tx, &Some(captured)).is_ok(), "its own record still commits");
}

/// Codex branch r1 (identity.rs:1398): 2.x replaced account A's key in place; the takeover fences it with its sentinel and
/// drops every digest. A failed or timed-out first read (the material unknown, so "changed") never lifts the fence; only
/// a read naming A does. A key fenced by a read of another account keeps that fence through a failed read too.
#[test]
fn s7a_a_failed_read_never_lifts_a_fence() {
    let (fx, key) = established();
    let reads = |c: &Connection| deltabadger::sync::ledger_reading_keys(c).unwrap().contains(&key);
    fx.replace(key, "kc-replaced-by-2x");
    identity::after_v2(&fx.c, &fx.cipher).unwrap();
    assert!(!reads(&fx.c), "fenced at takeover");
    fx.observe(key, Observed::Failed("transport".into()));
    assert!(!reads(&fx.c), "a timed-out read keeps the takeover's fence");
    fx.observe(key, Observed::Failed("http 401".into()));
    assert!(!reads(&fx.c), "a definite failure keeps it too");
    fx.observe(key, read(A));
    assert!(reads(&fx.c), "a read naming A lifts it");
    fx.observe(key, read(B));
    identity::after_v2(&fx.c, &fx.cipher).unwrap();
    fx.observe(key, Observed::Failed("transport".into()));
    assert!(!reads(&fx.c), "a read of another account stays the fence after a failed read");
}

/// Codex branch r1 (identity.rs:1422): removing the last key of an account that a stopped bot or a withdrawal rule still
/// keeps never clears its fingerprint, so another wallet connected later is a new account, never the heir of that bot.
/// With nothing keeping it, the fingerprint goes as before.
#[test]
fn s7a_removing_the_last_key_keeps_the_fingerprint_of_an_account_a_bot_or_rule_keeps() {
    let print = |fx: &Fx| fx.account(KUCOIN).flatten();
    let remove = |fx: &Fx, key: i64| {
        let account = identity::Account::of_key(&fx.c, key).unwrap().unwrap();
        fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
        identity::forget(&fx.c, key, &account, now()).unwrap();
    };
    for keeper in ["bot", "rule", "nothing"] {
        let fx = Fx::new();
        let key = fx.key(KUCOIN, 0, "kc-a");
        fx.observe(key, read(A));
        assert!(print(&fx).is_some(), "{keeper}: fingerprinted");
        let (exchange, account) = (fx.exchange(KUCOIN), identity::account_of_key(&fx.c, key).unwrap().unwrap());
        match keeper {
            "bot" => fx.c.execute("INSERT INTO bots (user_id, exchange_id, status, type, settings, transient_data, created_at, updated_at) VALUES (?1, ?2, 2, 'Bots::DcaSingleAsset', '{}', '{}', 'now', 'now')",
                                  [fx.user, exchange]).map(|_| ()).unwrap(),
            "rule" => fx.c.execute("INSERT INTO exchange_account_rules (account_id, user_id, type, status, exchange_id, settings, created_at, updated_at) VALUES (?1, ?2, 'Rules::Withdrawal', 0, ?3, '{}', 'now', 'now')",
                                   [account, fx.user, exchange]).map(|_| ()).unwrap(),
            _ => {}
        }
        remove(&fx, key);
        assert_eq!(print(&fx).is_some(), keeper != "nothing", "{keeper}");
    }
}

/// Rule F1 (Codex branch r2, identity.rs:1140/1398): a key is fenced whatever its status. A revalidation that times out
/// sets the key pending; a queued signal order or poll captured before must still fail its final check, and a failed read
/// in that state keeps the takeover's sentinel, so the key is still fenced once it is correct again.
#[test]
fn s7a_a_pending_or_incorrect_key_stays_fenced() {
    use deltabadger::engine::model;
    let fenced = |fx: &Fx, key: i64| -> bool { fx.c.query_row(&format!("SELECT {} FROM api_keys k WHERE k.id = ?1", identity::fenced_sql(&fx.c, "k").unwrap()), [key], |r| r.get(0)).unwrap() };
    let status = |fx: &Fx, key: i64, s: i64| { fx.c.execute("UPDATE api_keys SET status = ?2 WHERE id = ?1", [key, s]).unwrap(); };
    let (fx, key) = established();
    let captured = fx.version(key);
    fx.observe(key, read(B));
    for s in [0, 2] {
        status(&fx, key, s);
        assert!(fenced(&fx, key), "status {s}: fenced");
        assert!(!model::key_is_current(&fx.c, &captured).unwrap(), "status {s}: the final check refuses");
    }
    // The takeover's sentinel, then a timed-out read while the key is pending.
    let (fx, key) = established();
    fx.replace(key, "kc-replaced-by-2x");
    identity::after_v2(&fx.c, &fx.cipher).unwrap();
    status(&fx, key, 0);
    fx.observe(key, Observed::Failed("transport".into()));
    status(&fx, key, 1);
    assert!(fenced(&fx, key), "a failed read on a pending key keeps the sentinel");
    assert!(!deltabadger::sync::ledger_reading_keys(&fx.c).unwrap().contains(&key));
}

/// Rule F6 (Codex branch r2, identity.rs:1434, venue_ids.rs:184): removing an account's last key never clears its
/// fingerprint while anything references it, by one predicate (`identity::named`). Number 1's bot orders have no
/// membership row: its bot moved to account 2 after a fill, the order still names number 1, so number 1 keeps its
/// fingerprint; so does a deleted bot of number 1. An account 2 an order of a moved bot still names is kept, not
/// tombstoned. With nothing left, number 1's fingerprint goes.
#[test]
fn s7a_removing_the_last_key_keeps_the_fingerprint_while_an_order_or_any_bot_names_the_account() {
    let remove = |c: &Connection, key: i64| {
        let account = identity::Account::of_key(c, key).unwrap().unwrap();
        c.execute("DELETE FROM api_keys WHERE id = ?1", [key]).unwrap();
        identity::forget(c, key, &account, now()).unwrap();
    };
    let order = |c: &Connection, bot: i64, exchange: i64| {
        c.execute("INSERT INTO transactions (bot_id, exchange_id, status, created_at, updated_at) VALUES (?1, ?2, 1, 'now', 'now')", [bot, exchange]).unwrap();
        c.last_insert_rowid()
    };
    for keeper in ["moved bot's order", "deleted bot", "nothing"] {
        let t = two();
        let c = &t.fx.c;
        c.execute("UPDATE exchange_accounts SET fingerprint = 'fp-' || number WHERE id IN (?1, ?2)", [t.first, t.second]).unwrap();
        match keeper {
            "moved bot's order" => {
                order(c, t.bot1, t.exchange);
                c.execute("INSERT INTO exchange_account_bots (bot_id, account_id) VALUES (?1, ?2)", [t.bot1, t.second]).unwrap();
            }
            "deleted bot" => { c.execute("UPDATE bots SET status = ?2 WHERE id = ?1", [t.bot1, deltabadger::enums::BotStatus::Deleted as i64]).unwrap(); }
            _ => { c.execute("DELETE FROM bots WHERE id = ?1", [t.bot1]).unwrap(); }
        }
        remove(c, t.key1);
        assert_eq!(fingerprints(&t).0.is_some(), keeper != "nothing", "{keeper}");
    }
    // Account 2: its bot moved back to number 1 after an order on 2 (a membership row names 2): kept, not tombstoned.
    let t = two();
    let c = &t.fx.c;
    let placed = order(c, t.bot2, t.exchange);
    c.execute("INSERT INTO exchange_account_transactions (transaction_id, account_id) VALUES (?1, ?2)", [placed, t.second]).unwrap();
    c.execute("DELETE FROM exchange_account_bots WHERE bot_id = ?1", [t.bot2]).unwrap();
    remove(c, t.key2);
    assert_eq!(one_removed(c, t.second), None, "account 2 stays live");
}
fn one_removed(c: &Connection, id: i64) -> Option<String> { c.query_row("SELECT removed_at FROM exchange_accounts WHERE id = ?1", [id], |r| r.get(0)).unwrap() }

/// Codex branch r2 (sync/cache.rs:116): the tracker's ledger watermark asks the key that can sync. An account whose
/// trading key was fenced mid-import (its run left incomplete) and whose read-only key syncs is current once the read-only
/// key's import is; the tracker's publication still reads the fenced key's stored rows (`fence_read`).
#[test]
fn s7a_the_tracker_watermark_reads_the_key_that_can_sync() {
    use deltabadger::sync::cache;
    let (fx, trading) = established();
    let read_only = fx.key(KUCOIN, 2, "kc-a-read");
    fx.observe(read_only, read(A));
    common::two_accounts::stamp_ledgers(&fx.c, fx.user);
    deltabadger::jobs::state::mark_incomplete(&fx.c, "ledger_sync", Some(&trading.to_string()), now()).unwrap();
    let current = |fx: &Fx| cache::capture_read(&fx.c, fx.user, &deltabadger::tracker::Scope::All).unwrap().ledger_is_current(&fx.c).unwrap();
    assert!(!current(&fx), "the trading key syncs, and its import is incomplete");
    fx.observe(trading, read(B));
    assert!(deltabadger::sync::ledger_reading_keys(&fx.c).unwrap().contains(&read_only), "the read-only key syncs");
    assert!(current(&fx), "the read-only key's complete import is the watermark");
    let origin = cache::capture_read(&fx.c, fx.user, &deltabadger::tracker::Scope::All).unwrap();
    let tx = fx.c.unchecked_transaction().unwrap();
    assert!(cache::fence_read(&tx, &origin).is_ok(), "the figures from stored rows still publish");
}

/// Rule R2 (Codex branch r3, identity.rs:1661): discovery's read of a key fences every copy of that material on its
/// account. Account A holds kc-a as its trading and its read-only key; discovery reads the read-only one as B's: the
/// trading copy, never read itself, is fenced at once too.
#[test]
fn s7a_r2_discovery_fences_every_copy_of_the_key_it_read() {
    let (fx, trading) = established();
    let read_only = fx.key(KUCOIN, 2, "kc-a");
    let fenced = |k: i64| fx.c.query_row(&format!("SELECT {} FROM api_keys k WHERE k.id = ?1", identity::fenced_sql(&fx.c, "k").unwrap()), [k], |r| r.get::<_, bool>(0)).unwrap();
    assert!(!fenced(trading) && !fenced(read_only));
    let due = identity::due(&fx.c, &fx.cipher, now()).unwrap().into_iter().find(|d| d.api_key_id == read_only).expect("the read-only key is due");
    assert!(identity::publish(&fx.c, &fx.cipher, &due, &read(B), now()).unwrap());
    assert_eq!((fenced(read_only), fenced(trading)), (true, true), "both copies fenced");
    assert!(!deltabadger::sync::ledger_reading_keys(&fx.c).unwrap().contains(&trading));
}

/// Rule R3 (Codex branch r3, identity.rs:1457, kucoin_hf_ledger.rs:250): a row of ANY Rust-owned table with an
/// `account_id` column keeps its account named, so forgetting the account's last key keeps its fingerprint. The tables are
/// read from the migrated schema: a table added later with the column and not counted fails here.
#[test]
fn s7a_r3_every_rust_owned_table_naming_an_account_keeps_it() {
    let fx = Fx::new();
    let owned = deltabadger::store::REGISTRY.live_tables();
    let mut s = fx.c.prepare("SELECT m.name FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type = 'table' AND p.name = 'account_id' ORDER BY m.name").unwrap();
    let schema: Vec<String> = s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).filter(|t: &String| owned.contains(&t.as_str())).collect();
    drop(s);
    for t in ["kucoin_coverage", "kucoin_withheld_fills", "exchange_account_rules", "rust_api_order_standalone", "import_rows"] {
        assert!(schema.iter().any(|s| s == t), "{t} names an account: {schema:?}");
    }
    let exchange = fx.exchange(KUCOIN);
    // Placeholder rows: no foreign key or trigger of this throwaway install checks them.
    fx.c.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON").unwrap();
    let triggers: Vec<String> = fx.c.prepare("SELECT name FROM sqlite_master WHERE type = 'trigger'").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    for t in triggers { fx.c.execute(&format!("DROP TRIGGER \"{t}\""), []).unwrap(); }
    for table in &schema {
        fx.c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) SELECT ?1, ?2, coalesce(max(number), 0) + 1, 'now', 'now' FROM exchange_accounts WHERE user_id = ?1 AND exchange_id = ?2",
                     [fx.user, exchange]).unwrap();
        let account = fx.c.last_insert_rowid();
        assert!(!identity::named(&fx.c, account).unwrap(), "{table}: a fresh account names nothing");
        // One row naming the account, every other required column filled with a placeholder of its type.
        let mut s = fx.c.prepare("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1)").unwrap();
        let columns: Vec<(String, String, bool, Option<String>, i64)> = s.query_map([table], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap().map(Result::unwrap).collect();
        drop(s);
        let (mut names, mut values) = (vec![], vec![]);
        for (name, kind, notnull, default, pk) in columns {
            let value = if name == "account_id" { account.to_string() }
                        else if pk > 0 { format!("{}", account * 1000 + names.len() as i64) }
                        else if notnull && default.is_none() { if kind.to_uppercase().contains("INT") || kind.to_uppercase().contains("REAL") || kind.to_uppercase().contains("NUM") { "1".into() } else { format!("'{table}-{account}'") } }
                        else { continue };
            names.push(format!("\"{name}\""));
            values.push(value);
        }
        fx.c.execute(&format!("INSERT INTO \"{table}\" ({}) VALUES ({})", names.join(", "), values.join(", ")), []).unwrap_or_else(|e| panic!("{table}: {e}"));
        assert!(identity::named(&fx.c, account).unwrap(), "{table}: its row names the account");
    }
}

/// Rule R4 (Codex branch r3, cache.rs:39, shell.rs:62): balance freshness asks the key that can sync, as the ledger
/// watermark does (one selector, `cache::reading_key`). The trading key is found to open another account; the read-only key
/// reads the balances completely: they are fresh, and the shell keeps the portfolio ring.
#[test]
fn s7a_r4_balance_freshness_reads_the_key_that_can_sync() {
    use deltabadger::{engine::model, sync::cache, tracker::Scope};
    let (fx, trading) = established();
    let read_only = fx.key(KUCOIN, 2, "kc-a-read");
    fx.observe(read_only, read(A));
    fx.observe(trading, read(B));
    assert!(deltabadger::sync::balance_reading_keys(&fx.c).unwrap().contains(&read_only), "the read-only key syncs");
    let version = fx.version(read_only);
    let tx = fx.c.unchecked_transaction().unwrap();
    cache::record(&model::fence_versions(&tx, &[version]).unwrap(), fx.user, fx.exchange(KUCOIN), &fx.version(read_only), true, now()).unwrap();
    tx.commit().unwrap();
    assert!(!cache::stale(&fx.c, fx.user, &Scope::All).unwrap(), "the read-only key's complete read is current");
}

