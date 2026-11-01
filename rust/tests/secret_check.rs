//! PLAN-B2 r2.4: whether a SECRET_KEY_BASE reads an install (crypto::reads_install), before any start writes. One sample
//! per encrypted class; refused only when samples exist and none opens. On an unmigrated 2.x install: no Rust table is read.
mod common;
use deltabadger::crypto::{reads_install, Cipher, EncryptionKeys};
use rusqlite::Connection;

const T: &str = "2026-01-01 00:00:00";

fn right() -> Cipher { common::seed::cipher() }
fn wrong() -> Cipher { Cipher::new(&EncryptionKeys::resolve(&|_| None, "another-instance").unwrap()) }
fn install() -> (tempfile::TempDir, Connection) {
    let dir = common::rails_install_unmigrated();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    (dir, c)
}
fn config(c: &Connection, key: &str, value: &str) {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", (key, value, T)).unwrap();
}
fn user(c: &Connection, otp: Option<&str>) {
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, otp_secret_key, created_at, updated_at) VALUES ('o@example.com', 'x', 'Owner', 1, ?1, ?2, ?2)", (otp, T)).unwrap();
}
fn api_key(c: &Connection, key: &str, secret: &str) {
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.25', '0.4', ?1, ?1)", [T]).unwrap();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (1, ?1, ?2, ?3, 1, 0, ?4, ?4)",
              (c.last_insert_rowid(), key, secret, T)).unwrap();
}

#[test]
fn a_wrong_secret_on_an_install_with_an_identity_key_and_credentials_is_refused() {
    let (_d, c) = install();
    user(&c, None);
    api_key(&c, &right().encrypt("k"), &right().encrypt("s"));
    config(&c, deltabadger::identity::KEY_CONFIG, &right().encrypt(&"ab".repeat(32)));
    assert!(!reads_install(&c, &wrong()).unwrap());
    assert!(reads_install(&c, &right()).unwrap());
}

#[test]
fn only_2fa_seeds_stored_and_a_wrong_secret_is_refused() {
    let (_d, c) = install();
    user(&c, Some(&right().encrypt_binary("JBSWY3DPEHPK3PXP")));
    assert!(!reads_install(&c, &wrong()).unwrap());
    assert!(reads_install(&c, &right()).unwrap());
}

#[test]
fn a_lease_alone_and_a_wrong_secret_is_refused() {
    let (_d, c) = install();
    config(&c, deltabadger::lease::KEY, &right().encrypt(r#"{"engine":"rust"}"#));
    assert!(!reads_install(&c, &wrong()).unwrap());
    assert!(reads_install(&c, &right()).unwrap());
}

/// An encryption reset (identity.rs): the identity key unreadable, the credentials readable. Not a wrong secret: it passes,
/// and identity::initialize clears the identities as before.
#[test]
fn the_right_secret_with_an_unreadable_identity_key_and_readable_credentials_passes() {
    let (_d, c) = install();
    user(&c, None);
    api_key(&c, &right().encrypt("k"), &right().encrypt("s"));
    config(&c, deltabadger::identity::KEY_CONFIG, &wrong().encrypt(&"ab".repeat(32)));
    assert!(reads_install(&c, &right()).unwrap());
}

/// Plain values in encrypted columns read as themselves under any key (support_unencrypted_data): they are no sample.
#[test]
fn an_install_with_nothing_encrypted_passes_any_secret() {
    let (_d, c) = install();
    assert!(reads_install(&c, &wrong()).unwrap(), "nothing stored");
    user(&c, Some(""));
    api_key(&c, "plain-key", "plain-secret");
    config(&c, "alpaca_crypto_listings_last_good_count", "1");
    config(&c, "rust_taken_over_at", "2026-01-01T00:00:00Z");
    assert!(reads_install(&c, &wrong()).unwrap());
}

/// Each class counts on its own: with every other class absent, one unreadable sample of it refuses.
#[test]
fn each_encrypted_app_config_row_class_is_sampled() {
    let (_d, c) = install();
    config(&c, "plain_first", "1");
    config(&c, "market_data_token", &right().encrypt("t"));
    assert!(!reads_install(&c, &wrong()).unwrap(), "the first encrypted row, past a plain one");
    assert!(reads_install(&c, &right()).unwrap());
}
