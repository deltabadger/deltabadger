//! PLAN-A29: `deltabadger encryption-report | encryption-reset | encryption-derived-keys`, Rails' deltabadger:encryption
//! tasks, run as the operator runs them, on an install holding every kind of stored credential.
mod common;
use common::seed::{self, BotSpec};
use deltabadger::crypto::{Cipher, EncryptionKeys};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

const SECRET: &str = "engine-test-secret";
const OTHER: &str = "a-later-secret-that-is-longer-than-thirty-two-characters";
const PUBLISHED: &str = "dev-secret-key-not-for-production";
const T: &str = "2026-01-01 00:00:00";

fn cipher_for(secret: &str) -> Cipher { Cipher::new(&EncryptionKeys::resolve(&|_| None, secret).unwrap()) }

fn cmd(dir: &Path, secret: &str, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.args(args).env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", secret).env("IBKR_CATALOGUE_URL", "off")
        .env_remove("DATABASE_URL").env_remove("DELTABADGER_PLATFORM")
        .env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT");
    c
}
fn run(dir: &Path, secret: &str, args: &[&str]) -> Output { cmd(dir, secret, args).output().unwrap() }
fn out(o: &Output) -> String { String::from_utf8_lossy(&o.stdout).into_owned() }
fn err(o: &Output) -> String { String::from_utf8_lossy(&o.stderr).into_owned() }
fn db(dir: &Path) -> Connection { Connection::open(dir.join("production.sqlite3")).unwrap() }

/// Each database file's sha256. A read-only open in WAL mode may leave an empty -wal (and its -shm index) behind: a
/// -wal holding anything is a write, and fails here.
fn files(dir: &Path) -> BTreeMap<String, String> {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.file_name().to_string_lossy().ends_with("-wal") { assert_eq!(e.metadata().unwrap().len(), 0, "{:?} holds a write", e.path()); }
    }
    std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".sqlite3"))
        .map(|e| { use sha2::Digest; (e.file_name().to_string_lossy().into_owned(), hex::encode(sha2::Sha256::digest(std::fs::read(e.path()).unwrap()))) }).collect()
}

/// Every table's rows, as text.
fn tables(c: &Connection) -> BTreeMap<String, Vec<String>> {
    let names: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    names.into_iter().map(|t| {
        let mut s = c.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid")).unwrap();
        let n = s.column_count();
        let rows = s.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join("|")))
            .unwrap().map(Result::unwrap).collect();
        (t, rows)
    }).collect()
}

struct Fixture { dir: tempfile::TempDir, bot: i64, rule: i64, seeded: seed::Seeded }

/// An install under `secret` holding every kind of credential Rails' report lists, Rust's own encrypted rows, a plain
/// Rust row, a working bot with rebalancing on and a working withdrawal rule. Taken over by Rust (its lease encrypted).
fn install(secret: &str) -> Fixture {
    let dir = common::rails_install();
    let c = db(dir.path());
    let cipher = cipher_for(secret);
    let s = seed::seed_kraken(&c, &cipher);
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, 1, ?6, ?6)",
              (s.user_id, s.exchange_id, cipher.encrypt("w-key"), cipher.encrypt("w-secret"), cipher.encrypt("w-pass"), T)).unwrap();
    c.execute("INSERT INTO fee_api_keys (exchange_id, key, secret) VALUES (?1, ?2, ?3)", (s.exchange_id, cipher.encrypt("fee-key"), cipher.encrypt("fee-secret"))).unwrap();
    let now = chrono::Utc::now();
    for (k, v) in [("smtp_password", "smtp-pass"), ("coingecko_api_key", "cg-key"), ("market_data_token", "md-token")] {
        deltabadger::app_config::set(&c, &cipher, k, v, now).unwrap();
    }
    deltabadger::app_config::set_plain(&c, "registration_open", "true", now).unwrap();
    deltabadger::app_config::set_plain(&c, "rust_created_at", "2026-01-01T00:00:00.000Z", now).unwrap();
    deltabadger::app_config::set_plain(&c, "rust_taken_over_at", "2026-01-02T00:00:00.000Z", now).unwrap();
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('engine_lease', ?1, ?2, ?2)",
              (cipher.encrypt(r#"{"engine":"rust","version":"0.1.0","since":"2026-01-02T00:00:00Z"}"#), T)).unwrap();
    c.execute("UPDATE users SET otp_secret_key = ?1, otp_module = 1", [cipher.encrypt_binary("JBSWY3DPEHPK3PXP")]).unwrap();
    c.execute("INSERT INTO rules (type, user_id, exchange_id, asset_id, address, status, settings, created_at, updated_at) VALUES ('Rules::Withdrawal', ?1, ?2, ?3, ?4, 1, '{}', ?5, ?5)",
              (s.user_id, s.exchange_id, s.btc, cipher.encrypt("bc1qexampleaddress"), T)).unwrap();
    let rule = c.last_insert_rowid();
    c.execute("INSERT INTO oauth_applications (name, uid, redirect_uri, created_at, updated_at) VALUES ('MCP', 'uid-1', 'http://localhost/cb', ?1, ?1)", [T]).unwrap();
    let app = c.last_insert_rowid();
    c.execute("INSERT INTO oauth_access_tokens (application_id, resource_owner_id, token, created_at) VALUES (?1, ?2, 'bearer-1', ?3)", (app, s.user_id, T)).unwrap();
    c.execute("INSERT INTO oauth_access_grants (application_id, resource_owner_id, token, expires_in, redirect_uri, created_at) VALUES (?1, ?2, 'grant-1', 600, 'http://localhost/cb', ?3)", (app, s.user_id, T)).unwrap();
    c.execute("INSERT INTO connected_clients (user_id, oauth_application_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", (s.user_id, app, T)).unwrap();
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, entry_type, base_amount, base_currency, transacted_at, created_at, updated_at) \
               VALUES (?1, ?2, ?3, 0, '1', 'BTC', ?4, ?4, ?4)", (s.user_id, s.exchange_id, s.api_key_id, T)).unwrap();
    let bot = seed::insert_bot(&c, &s, &BotSpec::weekly(10.0, "2026-01-01 00:00:00").with("rebalance_enabled", serde_json::json!(true)));
    Fixture { dir, bot, rule, seeded: s }
}

// ---- report ---------------------------------------------------------------------------------------------------------

const REPORT: &str = "
API keys stored here — these will be cleared because the encryption key is
changing. Every stored value still decrypts under the current secret, and that
secret is not one published in this repository, so nothing below is known to be
exposed. If you know otherwise, re-run with --published.
  Kraken (trading) — o@example.com
  Kraken (withdrawal) — o@example.com

Fee API keys — will also be cleared: Kraken
Two-factor seed present for: o@example.com
Settings that will reset:    coingecko_api_key, market_data_token, registration_open, smtp_password
Those settings hold credentials too — they will be cleared and need re-entering,
not rotated: SMTP password, Alpaca key and secret, CoinGecko key, market-data token.

Withdrawal addresses — COPY THESE NOW, they cannot be recovered afterwards:
  Kraken / BTC: bc1qexampleaddress

";

#[test]
fn the_report_lists_every_credential_in_rails_words() {
    let f = install(SECRET);
    let o = run(f.dir.path(), SECRET, &["encryption-report"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o), REPORT);
}

#[test]
fn published_says_revoke_whether_flagged_secret_or_unreadable() {
    let f = install(SECRET);
    let revoke = "API keys — REVOKE each of these at the exchange.";
    assert!(out(&run(f.dir.path(), SECRET, &["encryption-report", "--published"])).contains(revoke));
    // A secret that reads nothing stored: the data was written under another key.
    let o = run(f.dir.path(), OTHER, &["encryption-report"]);
    assert!(out(&o).contains(revoke), "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("Kraken / BTC: <unreadable>"));
    let p = install(PUBLISHED);
    assert!(out(&run(p.dir.path(), PUBLISHED, &["encryption-report"])).contains(revoke));
}

#[test]
fn the_report_lists_owed_work() {
    let f = install(SECRET);
    db(f.dir.path()).execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json('{\"sell_transaction_id\":1}')) WHERE id = ?1", [f.bot]).unwrap();
    let o = out(&run(f.dir.path(), SECRET, &["encryption-report"]));
    assert!(o.contains("Owed — the reset refuses until each of these is settled:"), "{o}");
    assert!(o.contains(&format!("Bot {}: a rebalance is part-way through.", f.bot)), "{o}");
}

// ---- derived keys ---------------------------------------------------------------------------------------------------

#[test]
fn derived_keys_prints_the_pair_that_reads_the_install() {
    let f = install(SECRET);
    let o = out(&run(f.dir.path(), SECRET, &["encryption-derived-keys"]));
    let line = |var: &str| o.lines().find_map(|l| l.trim().strip_prefix(&format!("{var}="))).unwrap_or_else(|| panic!("{var} in {o}")).to_string();
    let (primary, salt) = (line("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY"), line("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT"));
    // Set as the instance's own pair under a different SECRET_KEY_BASE, they read the install.
    let c = cmd(f.dir.path(), OTHER, &["check"]).env("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", &primary)
        .env("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", &salt).output().unwrap();
    assert!(!err(&c).contains("can't read this install"), "{}{}", out(&c), err(&c));
    let cipher = Cipher::new(&EncryptionKeys { primary_key: primary, key_derivation_salt: salt });
    assert!(!deltabadger::rotation::written_under_another_key(&db(f.dir.path()), &cipher).unwrap());
}

#[test]
fn derived_keys_refuses_an_unreadable_install_and_says_nothing_to_do_with_its_own_pair() {
    let f = install(SECRET);
    let o = out(&run(f.dir.path(), OTHER, &["encryption-derived-keys"]));
    assert!(o.contains("could not be confirmed") && !o.contains("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY="), "{o}");
    let d = EncryptionKeys::resolve(&|_| None, SECRET).unwrap();
    let o = cmd(f.dir.path(), OTHER, &["encryption-derived-keys"]).env("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", &d.primary_key)
        .env("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", &d.key_derivation_salt).output().unwrap();
    assert!(out(&o).contains("already keeps its encryption keys separate"), "{}{}", out(&o), err(&o));
}

// ---- reset ----------------------------------------------------------------------------------------------------------

#[test]
fn without_the_confirmation_nothing_changes() {
    let f = install(SECRET);
    let before = files(f.dir.path());
    for args in [&["encryption-reset"][..], &["encryption-reset", "--confirm", "yes"], &["encryption-reset", "--confirm=clear"]] {
        let o = run(f.dir.path(), SECRET, args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(err(&o).contains("Re-run with:  deltabadger encryption-reset --confirm clear-credentials"), "{}", err(&o));
        assert_eq!(files(f.dir.path()), before, "{args:?}");
    }
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--bogus"]);
    assert!(err(&o).contains("usage: deltabadger encryption-reset"), "{}", err(&o));
}

#[test]
fn the_reset_clears_exactly_the_inventory_and_leaves_every_other_row() {
    let f = install(SECRET);
    let before = tables(&db(f.dir.path()));
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let text = out(&o);
    for line in ["API keys deleted:            2", "Fee API keys deleted:        1", "Settings cleared:            4", "Withdrawal addresses:        1",
                 "Two-factor seed cleared for: 1 user(s)", "REST API and MCP tokens:     2", "Bots stopped:                1", "Rules stopped:               1",
                 "Nothing here was known to be exposed", &format!("Rebalancing was turned off on: Bot {}.", f.bot)] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    let c = db(f.dir.path());
    let after = tables(&c);
    let changed = ["api_keys", "fee_api_keys", "app_configs", "users", "rules", "bots", "bot_activity_logs", "oauth_access_tokens", "oauth_access_grants",
                   "connected_clients", "account_transactions", "sqlite_sequence"];
    for (t, rows) in &before { if !changed.contains(&t.as_str()) { assert_eq!(after.get(t), Some(rows), "{t} changed"); } }
    let n = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(n("SELECT count(*) FROM api_keys") + n("SELECT count(*) FROM fee_api_keys") + n("SELECT count(*) FROM oauth_access_tokens")
               + n("SELECT count(*) FROM oauth_access_grants") + n("SELECT count(*) FROM connected_clients"), 0);
    assert_eq!(n("SELECT count(*) FROM oauth_applications"), 1, "client registrations are kept");
    assert_eq!(n("SELECT count(*) FROM account_transactions WHERE api_key_id IS NULL"), 1, "ledger rows kept, unlinked");
    assert_eq!(n("SELECT count(*) FROM users WHERE otp_secret_key IS NULL AND otp_module = 0"), 1);
    assert_eq!(n(&format!("SELECT count(*) FROM rules WHERE id = {} AND address IS NULL AND status = 2", f.rule)), 1);
    assert_eq!(n(&format!("SELECT count(*) FROM bots WHERE id = {} AND status = 2 AND json_extract(settings, '$.rebalance_enabled') = 0", f.bot)), 1);
    assert_eq!(n(&format!("SELECT count(*) FROM bot_activity_logs WHERE bot_id = {} AND event = 'stopped'", f.bot)), 1);
    let keys: Vec<String> = c.prepare("SELECT key FROM app_configs ORDER BY key").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(keys, ["engine_lease", "rust_created_at", "rust_taken_over_at"], "the plain Rust rows stay, the lease is rewritten");
    let lease: String = c.query_row("SELECT value FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get(0)).unwrap();
    assert!(!deltabadger::crypto::is_envelope(&lease) && lease.contains("\"engine\":\"rust\""), "{lease}");
    // Nothing encrypted is left for any key to fail on.
    assert!(!deltabadger::rotation::written_under_another_key(&c, &cipher_for("any-new-secret")).unwrap());
}

#[test]
fn after_the_reset_the_install_starts_on_a_new_secret() {
    let f = install(SECRET);
    assert!(run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials"]).status.success());
    let o = run(f.dir.path(), OTHER, &["check"]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("ready:"), "{}", out(&o));
}

#[test]
fn the_reset_works_where_the_secret_already_changed_and_says_revoke() {
    let f = install(SECRET);
    let o = run(f.dir.path(), OTHER, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("Those credentials should ALREADY have been REVOKED"), "{}", out(&o));
    assert_eq!(db(f.dir.path()).query_row("SELECT count(*) FROM api_keys", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn compromised_is_judged_before_the_clear() {
    // A published secret's install: after the clear nothing is left to judge, the text still says revoke.
    let p = install(PUBLISHED);
    let o = run(p.dir.path(), PUBLISHED, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert!(out(&o).contains("Those credentials should ALREADY have been REVOKED"), "{}{}", out(&o), err(&o));
    // Readable and not published: only the flag forces it.
    let f = install(SECRET);
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials", "--published"]);
    assert!(out(&o).contains("Those credentials should ALREADY have been REVOKED"), "{}{}", out(&o), err(&o));
}

#[test]
fn interactive_brokers_needs_its_own_confirmation() {
    let f = install(SECRET);
    let c = db(f.dir.path());
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Ibkr', 'IBKR', '0', '0', ?1, ?1)", [T]).unwrap();
    let ibkr = c.last_insert_rowid();
    c.execute("INSERT INTO api_keys (user_id, exchange_id, key, rsa_signature_key, status, key_type, created_at, updated_at) VALUES (1, ?1, 'consumer', ?2, 1, 0, ?3, ?3)",
              (ibkr, cipher_for(SECRET).encrypt("-----BEGIN PRIVATE KEY-----"), T)).unwrap();
    drop(c);
    let before = files(f.dir.path());
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("re-run with --confirm-ibkr as well."), "{}", err(&o));
    assert_eq!(files(f.dir.path()), before);
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials", "--confirm-ibkr"]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("Interactive Brokers: once the app is back up"));
}

#[test]
fn the_reset_refuses_while_the_engine_holds_the_install() {
    let f = install(SECRET);
    let paths = deltabadger::store::Paths::from_env(&|_| None, f.dir.path());
    let _held = deltabadger::lease::lock(&paths, chrono::Utc::now()).unwrap();
    let before = files(f.dir.path());
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("another Deltabadger engine is running on this data"), "{}", err(&o));
    assert_eq!(files(f.dir.path()), before);
    // The report still runs beside the app, and says the owed work wasn't checked.
    let o = run(f.dir.path(), SECRET, &["encryption-report"]);
    assert!(out(&o).contains(deltabadger::rotation::OWED_NOT_CHECKED), "{}{}", out(&o), err(&o));
}

/// Each kind of owed work refuses the reset with nothing written, with this secret and with one that reads nothing.
#[test]
fn owed_work_refuses_the_reset() {
    type Add<'a> = &'a dyn Fn(&Connection, &Fixture);
    let late = |kind: &'static str| move |c: &Connection, f: &Fixture| {
        let now = deltabadger::codec::format_time(chrono::Utc::now());
        let tx = seed::insert_tx(c, &f.seeded, f.bot, &seed::TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("{kind}-1")), order_type: 0,
                                 amount: Some("0.001"), quote_amount: None, price: None, quote_amount_exec: None, amount_exec: Some("0.001"), created_at: now });
        c.execute("UPDATE transactions SET transaction_type = ?2 WHERE id = ?1", (tx, kind)).unwrap();
    };
    let (late_liquidation, late_redeploy) = (late("LIQUIDATION"), late("REDEPLOY"));
    let cases: [(&str, Add, &str); 9] = [
        ("liquidation proceeds", &late_liquidation, "its proceeds aren't recorded yet"),
        ("redeploy spend", &late_redeploy, "its proceeds aren't recorded yet"),
        ("liquidation marker", &|c, f| { c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.liquidation_pending', json('{\"id\":\"x\",\"state\":\"ambiguous\"}')) WHERE id = ?1", [f.bot]).unwrap(); },
         "a sale of its holdings hasn't finished"),
        ("abandoned liquidation", &|c, f| { let tx = seed::insert_tx(c, &f.seeded, f.bot, &seed::TxSpec { status: 1, external_status: Some(4), external_id: Some("L-1".into()), order_type: 0,
                                     amount: Some("0.001"), quote_amount: None, price: None, quote_amount_exec: None, amount_exec: None, created_at: T.into() });
                                     c.execute("UPDATE transactions SET transaction_type = 'LIQUIDATION' WHERE id = ?1", [tx]).unwrap(); }, "a sale of its holdings hasn't finished"),
        ("open order", &|c, f| { seed::insert_tx(c, &f.seeded, f.bot, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("OPEN-1".into()), order_type: 0,
                                     amount: Some("0.001"), quote_amount: Some("10"), price: Some("10000"), quote_amount_exec: None, amount_exec: None, created_at: T.into() }); }, "open orders"),
        ("swap", &|c, f| { c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json('{\"sell_transaction_id\":1}')) WHERE id = ?1", [f.bot]).unwrap(); },
         "a rebalance is part-way through"),
        ("liquidation", &|c, f| { c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.liquidation_selling_since', json('{\"holdings\":{}}')) WHERE id = ?1", [f.bot]).unwrap(); },
         "a sale of its holdings hasn't finished"),
        ("api order", &|c, _| { c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, created_at, send_by) \
                                          VALUES (1, NULL, 'buy', '1', 'quote', '{}', 'sending', ?1, ?1)", [T]).unwrap(); }, "(request 1) hasn't finished"),
        ("withdrawal", &|c, f| { c.execute("INSERT INTO rust_withdrawal_intents (owner, state, client_id, created_at, api_key_id, credential_digest, host, account_scope, key_identity, params_enc, fee_basis, budget, free_balance) \
                                           VALUES (?1, 'dispatched', 'c1', ?2, 2, 'd', 'h', 's', 'k', 'p', '{}', '1', '1')", (format!("rule:{}", f.rule), T)).unwrap(); },
         "a withdrawal may have been sent"),
    ];
    for (kind, add, said) in cases {
        for secret in [SECRET, OTHER] {
            let f = install(SECRET);
            add(&db(f.dir.path()), &f);
            let before = files(f.dir.path());
            let o = run(f.dir.path(), secret, &["encryption-reset", "--confirm", "clear-credentials"]);
            assert_eq!(o.status.code(), Some(1), "{kind}: {}{}", out(&o), err(&o));
            assert!(err(&o).contains(said), "{kind}: {said:?} in {}", err(&o));
            assert!(err(&o).contains(if secret == SECRET { "Settle each of these in the app first" } else { "Contact support before clearing anything" }), "{kind}: {}", err(&o));
            assert_eq!(files(f.dir.path()), before, "{kind}: nothing written");
        }
    }
}

#[test]
fn a_withdrawal_never_sent_is_deleted_not_owed() {
    let f = install(SECRET);
    db(f.dir.path()).execute("INSERT INTO rust_withdrawal_intents (owner, state, client_id, created_at, api_key_id, credential_digest, host, account_scope, key_identity, params_enc, fee_basis, budget, free_balance) \
                              VALUES (?1, 'prepared', 'c1', ?2, 2, 'd', 'h', 's', 'k', ?3, '{}', '1', '1')", (format!("rule:{}", f.rule), T, cipher_for(SECRET).encrypt("{}"))).unwrap();
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert_eq!(db(f.dir.path()).query_row("SELECT count(*) FROM rust_withdrawal_intents", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

// ---- start --------------------------------------------------------------------------------------------------------

#[test]
fn check_and_the_commands_refuse_without_a_secret() {
    let f = install(SECRET);
    for c in ["encryption-report", "encryption-reset", "encryption-derived-keys"] {
        let o = cmd(f.dir.path(), SECRET, &[c]).env_remove("SECRET_KEY_BASE").output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{c}");
        assert!(err(&o).contains("SECRET_KEY_BASE is not set"), "{c}: {}", err(&o));
    }
}

/// SecretKeyBaseGuard at start: said on stderr before anything else, never refusing (here the held lock refuses).
#[test]
fn a_published_or_short_secret_is_said_at_start() {
    let f = install(SECRET);
    let paths = deltabadger::store::Paths::from_env(&|_| None, f.dir.path());
    let _held = deltabadger::lease::lock(&paths, chrono::Utc::now()).unwrap();
    let o = run(f.dir.path(), PUBLISHED, &["run"]);
    assert!(err(&o).contains("SECRET_KEY_BASE is a value published in the public repository."), "{}", err(&o));
    assert!(err(&o).contains("encryption-reset --confirm clear-credentials"), "{}", err(&o));
    assert!(err(&o).contains("another Deltabadger engine is running"), "the warning refuses nothing: {}", err(&o));
    let o = run(f.dir.path(), &"x".repeat(64), &["run"]);
    assert!(!err(&o).contains("SECURITY"), "{}", err(&o));
}

// ---- Rails cross-check (PLAN-A29 r2.7) ------------------------------------------------------------------------------

/// The listed text rewrites (PLAN-A29 §5): Rails' words → 3.0's. Nothing else may differ.
const REWRITES: &[(&str, &str)] = &[
    ("re-run with PUBLISHED=yes.", "re-run with --published."), // R1
    ("  1. Stop the app. A job a worker has already picked up cannot be cancelled and\n     would keep trading while this runs.",
     "  1. Stop the app. This refuses to run while the app is running: the app holds\n     the install while it runs."), // S1
    ("Run `deltabadger:encryption:report`.", "Run `deltabadger encryption-report`."), // S3
    ("re-run with PUBLISHED=yes as well", "re-run with --published as well"), // S4
    ("Re-run with:  CONFIRM=clear-credentials bin/rails deltabadger:encryption:reset", "Re-run with:  deltabadger encryption-reset --confirm clear-credentials"), // S5
    ("rather than blanking it:\nthat file runs two services off one volume, and each would generate its own.", "rather than blanking it."), // S9
];
const VECTOR_TABLES: &[&str] = &["users", "exchanges", "assets", "api_keys", "fee_api_keys", "app_configs", "rules", "oauth_applications",
                                 "oauth_access_tokens", "oauth_access_grants", "connected_clients", "bots", "account_transactions"];

fn rewritten(rails: &str) -> String { REWRITES.iter().fold(rails.to_string(), |t, (from, to)| t.replace(from, to)) }

fn cell(v: &serde_json::Value) -> String {
    match v { serde_json::Value::Null => "NULL".into(), serde_json::Value::String(s) => s.clone(), serde_json::Value::Bool(b) => (*b as i64).to_string(), other => other.to_string() }
}

/// Each row of `table` as column → text, the columns D-RESET-STOP-LOGGED's stop writes masked (Rails' update_all writes
/// no updated_at; both write a stopped_at of their own `now`).
fn rows(c: &Connection, table: &str) -> Vec<BTreeMap<String, String>> {
    let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY id")).unwrap();
    let names: Vec<String> = s.column_names().into_iter().map(str::to_string).collect();
    s.query_map([], |r| Ok(names.iter().enumerate().map(|(i, n)| (n.clone(), match r.get::<_, rusqlite::types::Value>(i).unwrap() {
        rusqlite::types::Value::Null => "NULL".into(), rusqlite::types::Value::Integer(i) => i.to_string(), rusqlite::types::Value::Real(f) => serde_json::json!(f).to_string(),
        rusqlite::types::Value::Text(t) => t, rusqlite::types::Value::Blob(b) => format!("{b:?}") })).collect())).unwrap().map(Result::unwrap).map(mask).collect()
}
fn mask(mut row: BTreeMap<String, String>) -> BTreeMap<String, String> {
    for k in ["stopped_at", "updated_at"] { if let Some(v) = row.get_mut(k) { *v = "~".into(); } }
    row
}

#[test]
fn rails_and_rust_clear_the_same_rows_and_differ_only_by_the_listed_rewrites() {
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/encryption_vectors.json")).unwrap();
    let secret = v["secret_key_base"].as_str().unwrap();
    assert_eq!(v["independent"], false, "the vectors are of an install deriving its keys");
    let dir = common::rails_install();
    let c = db(dir.path());
    for table in VECTOR_TABLES {
        for row in v["before"][table].as_array().unwrap() {
            let row = row.as_object().unwrap();
            let cols: Vec<&String> = row.keys().collect();
            let sql = format!("INSERT INTO {table} ({}) VALUES ({})", cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
                              (1..=cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", "));
            let values: Vec<rusqlite::types::Value> = cols.iter().map(|k| match &row[*k] {
                serde_json::Value::Null => rusqlite::types::Value::Null,
                serde_json::Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
                serde_json::Value::Number(n) => n.as_i64().map(rusqlite::types::Value::Integer).unwrap_or_else(|| rusqlite::types::Value::Real(n.as_f64().unwrap())),
                serde_json::Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                other => rusqlite::types::Value::Text(other.to_string()),
            }).collect();
            c.execute(&sql, rusqlite::params_from_iter(values)).unwrap();
        }
    }
    drop(c);
    let report = run(dir.path(), secret, &["encryption-report"]);
    assert_eq!(out(&report), rewritten(v["report"]["stdout"].as_str().unwrap()), "{}", err(&report));
    let unconfirmed = run(dir.path(), secret, &["encryption-reset"]);
    assert_eq!(unconfirmed.status.code(), Some(v["unconfirmed"]["status"].as_i64().unwrap() as i32));
    assert_eq!(err(&unconfirmed).trim_end(), rewritten(v["unconfirmed"]["stderr"].as_str().unwrap()).trim_end());
    let reset = run(dir.path(), secret, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert_eq!(out(&reset), rewritten(v["reset"]["stdout"].as_str().unwrap()), "{}", err(&reset));
    let c = db(dir.path());
    for table in VECTOR_TABLES {
        let rails: Vec<BTreeMap<String, String>> = v["after"][table].as_array().unwrap().iter()
            .map(|r| mask(r.as_object().unwrap().iter().map(|(k, v)| (k.clone(), cell(v))).collect())).collect();
        assert_eq!(rows(&c, table), rails, "{table} after the reset");
    }
}

/// An order Rails placed and never recorded (its Bot::FetchAndCreateOrderJob still queued): the takeover's own hard
/// stop, reached through the reset's judgement of the next start.
#[test]
fn an_order_rails_never_recorded_refuses_the_reset() {
    let f = install(SECRET);
    let q = Connection::open(f.dir.path().join("production_queue.sqlite3")).unwrap();
    let args = serde_json::json!({ "job_class": "Bot::FetchAndCreateOrderJob",
        "arguments": [{ "_aj_globalid": format!("gid://deltabadger/Bots::DcaMultiAsset/{}", f.bot) }, "OQCLML-BW3P3-BUCMWZ"] });
    q.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, active_job_id, priority, created_at, updated_at) \
               VALUES ('default', 'Bot::FetchAndCreateOrderJob', ?1, 'job-1', 0, ?2, ?2)", (args.to_string(), T)).unwrap();
    drop(q);
    let before = files(f.dir.path());
    let o = run(f.dir.path(), SECRET, &["encryption-reset", "--confirm", "clear-credentials"]);
    assert_eq!(o.status.code(), Some(1), "{}{}", out(&o), err(&o));
    assert!(err(&o).contains("order OQCLML-BW3P3-BUCMWZ was placed on your exchange before the update but isn't recorded here yet"), "{}", err(&o));
    assert_eq!(files(f.dir.path()), before);
}
