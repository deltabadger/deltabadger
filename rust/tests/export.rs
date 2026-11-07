//! PLAN-B4b: `deltabadger export`, the download of a stopped copy. Its files against Rails' own export of the same input
//! (tests/fixtures/portable_export.json, script/rust/record_portable_export.rb), the scrub's guards, the canonical
//! projection, the refusals, and the restores it promises (§5).
#![cfg(unix)]
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::crypto::{Cipher, EncryptionKeys};
use deltabadger::export::{self, ExportError, Scratch};
use rusqlite::types::Value as Sql;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const VECTOR_SECRET: &str = "b4b-shared-vector-secret-key-base";
const VECTOR_PRIMARY: &str = "43bf7769d880b9201f00ac0bdef2af078cadfdbf737177375615e1767e72563f";
const VECTOR_SALT: &str = "948c8649965f6f0b7a94b7033f0be2ffb0b8b5e06205899336a07fab863a3736";

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| serde_json::from_str(include_str!("fixtures/portable_export.json")).unwrap())
}
fn now() -> DateTime<Utc> { "2026-11-01T12:00:00Z".parse().unwrap() }

/// A scratch container: `<root>/storage`, `<root>/export` (absent), `<root>/.dockerenv`. Paths are real (no /var link).
struct Box { _dir: tempfile::TempDir, s: Scratch }
impl Box {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("storage")).unwrap();
        std::fs::write(root.join(".dockerenv"), "").unwrap();
        Box { s: Scratch { storage: root.join("storage"), output: root.join("export"), marker: root.join(".dockerenv") }, _dir: dir }
    }
    fn primary(&self) -> PathBuf { self.s.storage.join("production.sqlite3") }
    /// The source, for the test's own writes (this SQLite enforces foreign keys by default; Rails' seed order doesn't).
    fn db(&self) -> Connection { let c = Connection::open(self.primary()).unwrap(); c.pragma_update(None, "foreign_keys", "OFF").unwrap(); c }
    fn out(&self, rel: &str) -> PathBuf { self.s.output.join(rel) }
    fn export(&self) -> Result<Vec<String>, ExportError> { export::run(&self.s, &scratch_env, now()) }
    fn exported(&self) -> Vec<String> { self.export().unwrap_or_else(|e| panic!("export failed ({}): {}", e.exit_code(), e.message())) }
}
fn scratch_env(k: &str) -> Option<String> { (k == "EXPORT_SCRATCH").then(|| "1".into()) }

/// Copies an install's two files into the box's storage.
fn install_into(b: &Box, from: &Path) {
    for f in common::INSTALL_FILES { std::fs::copy(from.join(f), b.s.storage.join(f)).unwrap(); }
}

fn to_sql(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Number(n) => n.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(n.as_f64().unwrap())),
        Value::String(s) => Sql::Text(s.clone()),
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Object(o) if o.contains_key("real") => Sql::Real(o["real"].as_str().unwrap().parse().unwrap()),
        Value::Object(o) if o.contains_key("blob") => Sql::Blob(hex::decode(o["blob"].as_str().unwrap()).unwrap()),
        other => panic!("unexpected cell {other}"),
    }
}

/// The recorder's seed (the Rails tables' rows), written into a Rails-built install with this build's migrations.
fn rails_input(b: &Box) {
    let template = common::rails_install();
    install_into(b, template.path());
    let c = b.db();
    for (table, t) in fixture()["source"].as_object().unwrap() {
        if matches!(table.as_str(), "schema_migrations" | "ar_internal_metadata") { continue; }
        let cols: Vec<&str> = t["columns"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
        let sql = format!("INSERT INTO {table} ({}) VALUES ({})", cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(","),
                          (1..=cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(","));
        for row in t["rows"].as_array().unwrap() {
            c.execute(&sql, rusqlite::params_from_iter(row.as_array().unwrap().iter().map(to_sql))).unwrap();
        }
    }
}

fn rows(c: &Connection, table: &str) -> (Vec<String>, Vec<Vec<Sql>>) {
    let mut s = c.prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid")).unwrap();
    let cols = s.column_names().iter().map(|c| c.to_string()).collect();
    let n = s.column_count();
    let rows = s.query_map([], |r| (0..n).map(|i| r.get::<_, Sql>(i)).collect()).unwrap().map(Result::unwrap).collect();
    (cols, rows)
}

/// Every file under `dir`: relative path → (size, mtime, sha256), the directory listing included.
fn listing(dir: &Path) -> BTreeMap<String, (u64, i64, String)> {
    use sha2::Digest;
    use std::os::unix::fs::MetadataExt;
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let m = std::fs::symlink_metadata(e.path()).unwrap();
        let bytes = if m.is_file() { std::fs::read(e.path()).unwrap() } else { vec![] };
        out.insert(e.file_name().to_string_lossy().into_owned(), (m.len(), m.mtime_nsec() + m.mtime() * 1_000_000_000, hex::encode(sha2::Sha256::digest(bytes))));
    }
    out
}

fn output_files(b: &Box) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { walk(root, &p, out) } else { out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned()) }
        }
    }
    let mut v = vec![];
    if b.s.output.exists() { walk(&b.s.output, &b.s.output, &mut v); }
    v.sort();
    v
}

/// Every output byte, the files concatenated, and the printed lines.
fn all_bytes(b: &Box, printed: &[String]) -> Vec<u8> {
    let mut all = printed.join("\n").into_bytes();
    for f in output_files(b) { all.extend(std::fs::read(b.out(&f)).unwrap()); }
    all
}
fn contains(hay: &[u8], needle: &str) -> bool { hay.windows(needle.len()).any(|w| w == needle.as_bytes()) }

const CACHE_CABLE: [&str; 2] = ["portable/production_cable.sqlite3", "portable/production_cache.sqlite3"];

// ---- 1, 7, 8: the files against Rails' ------------------------------------------------------------------------------

#[test]
fn the_files_are_rails_files_of_the_same_install() {
    let b = Box::new();
    rails_input(&b);
    let printed = b.exported();
    let f = fixture();
    assert_eq!(printed.last().unwrap(), f["printed"].as_str().unwrap().trim_end());
    let rails: Vec<&str> = f["files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).filter(|p| !CACHE_CABLE.contains(p)).collect();
    assert_eq!(output_files(&b), rails, "D-EXPORT-NO-CACHE-CABLE: Rails' files less cache and cable");
    assert_eq!(std::fs::read_to_string(b.out("csv/tracker.csv")).unwrap(), f["tracker_csv"].as_str().unwrap());
    for (name, body) in f["bots"].as_object().unwrap() {
        assert_eq!(std::fs::read_to_string(b.out(&format!("csv/bots/{name}"))).unwrap_or_else(|_| panic!("no {name}")), body.as_str().unwrap(), "{name}");
    }
    let version = |t: &str| t.replace("{{app_version}}", deltabadger::VERSION).replace("{{schema_version}}", &export::schema_version().to_string());
    assert_eq!(std::fs::read_to_string(b.out("csv/README_CSV.txt")).unwrap(), version(f["readme_csv"].as_str().unwrap()));
    let manifest: String = version(f["manifest"].as_str().unwrap()).lines().filter(|l| !CACHE_CABLE.iter().any(|c| l.contains(c)))
        .map(|l| format!("{l}\n")).collect();
    assert_eq!(std::fs::read_to_string(b.out("manifest.json")).unwrap(), manifest);
}

#[test]
fn the_transliteration_table_and_labels_are_rubys() {
    let f = fixture();
    let ruby: BTreeMap<String, String> = serde_json::from_value(f["transliteration"].clone()).unwrap();
    let ours: BTreeMap<String, String> = export::TRANSLITERATION.iter().map(|(c, s)| (c.to_string(), s.to_string())).collect();
    assert_eq!(ours, ruby);
    for (label, expected) in f["labels"].as_object().unwrap() {
        if label.trim().is_empty() { continue; } // a blank label is the bot's generated name (the files test)
        assert_eq!(export::file_label(label), expected.as_str().unwrap(), "{label:?}");
    }
    // D-EXPORT-LABEL-NFC: a decomposed accent is not folded into its letter.
    assert_eq!(export::file_label("Cafe\u{301}"), "cafe");
}

#[test]
fn the_manifest_names_this_build_and_every_file_written() {
    let b = Box::new();
    rails_input(&b);
    b.exported();
    let m: Value = serde_json::from_str(&std::fs::read_to_string(b.out("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["app_version"], deltabadger::VERSION);
    let newest = deltabadger::store::REGISTRY.migrations.iter().map(|m| m.version).max().unwrap();
    assert_eq!(m["schema_version"], json!(newest.parse::<i64>().unwrap()));
    assert_eq!(m["schema_version"].to_string().len(), 14);
    let files: Vec<String> = output_files(&b).into_iter().filter(|f| f != "manifest.json").collect();
    assert_eq!(m["files"], json!(files));
}

#[test]
fn csv_names_fit_launchpads_rule_and_rails_sizes() {
    let b = Box::new();
    rails_input(&b);
    b.exported();
    let rule = regex::Regex::new(r"\Acsv/bots/\d+-[a-zA-Z0-9_-]+\.csv\z").unwrap();
    for f in output_files(&b).iter().filter(|f| f.starts_with("csv/bots/")) {
        assert!(rule.is_match(f), "{f}");
        assert!(f.len() <= 100, "{f}");
        let rails = &fixture()["bots"][f.trim_start_matches("csv/bots/")];
        assert_eq!(std::fs::metadata(b.out(f)).unwrap().len() as usize, rails.as_str().unwrap().len(), "{f}");
    }
    let longest = format!("csv/bots/{}-{}.csv", i64::MAX, export::file_label(&"Ab".repeat(100)));
    assert!(longest.len() <= 100, "{longest}");
}

// ---- 2: the scrub on Rails' tables ----------------------------------------------------------------------------------

#[test]
fn the_scrubbed_rails_tables_are_rails_scrubbed_rows() {
    let b = Box::new();
    rails_input(&b);
    b.exported();
    let out = Connection::open(b.out("portable/production.sqlite3")).unwrap();
    let f = fixture();
    for (table, t) in f["scrubbed"].as_object().unwrap() {
        if table == "ar_internal_metadata" { continue; } // the schema's own stamps
        let (cols, ours) = rows(&out, table);
        let theirs: Vec<Vec<Sql>> = t["rows"].as_array().unwrap().iter().map(|r| r.as_array().unwrap().iter().map(to_sql).collect()).collect();
        assert_eq!(cols, t["columns"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect::<Vec<_>>(), "{table}");
        assert_eq!(ours.len(), theirs.len(), "{table}");
        let token = cols.iter().position(|c| c == "token");
        for (i, (a, e)) in ours.iter().zip(&theirs).enumerate() {
            if table == "bot_signals" {
                // Regenerated: changed from the source's, 64 hex.
                let t = token.unwrap();
                let Sql::Text(new) = &a[t] else { panic!("token") };
                assert!(new.len() == 64 && new.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{new}");
                assert_ne!(Some(new.as_str()), f["source"]["bot_signals"]["rows"][i][t].as_str());
                let strip = |r: &Vec<Sql>| r.iter().enumerate().filter(|(j, _)| *j != t).map(|(_, v)| v.clone()).collect::<Vec<_>>();
                assert_eq!(strip(a), strip(e), "{table} row {i}");
            } else {
                assert_eq!(a, e, "{table} row {i}");
            }
        }
    }
}

// ---- 4: the guards against a new secret column ----------------------------------------------------------------------

#[test]
fn policy_covers_coverage() {
    let policy: Vec<(&str, &str)> = export::POLICY.iter().map(|(k, _)| *k).collect();
    assert_eq!(policy, deltabadger::rotation::COVERAGE.to_vec());
    let deleted: Vec<String> = export::APP_CONFIGS_DELETED.iter().filter_map(|k| match k { export::Key::Exact(k) => Some(k.to_string()), _ => None }).collect();
    let rails: Vec<String> = serde_json::from_value(fixture()["app_config_keys"].clone()).unwrap();
    assert_eq!(&deleted[..rails.len()], &rails[..], "Rails' 13 keys");
    let prefixes: Vec<String> = export::APP_CONFIGS_DELETED.iter().filter_map(|k| match k { export::Key::Prefix(p) => Some(p.to_string()), _ => None }).collect();
    assert_eq!(prefixes, serde_json::from_value::<Vec<String>>(fixture()["app_config_prefixes"].clone()).unwrap());
    assert_eq!(export::SESSION_TABLES.to_vec(), serde_json::from_value::<Vec<String>>(fixture()["session_tables"].clone()).unwrap());
    let cleared: Vec<&str> = export::POLICY.iter().filter(|(_, p)| *p == export::Policy::Clear).map(|((_, c), _)| *c).collect();
    assert_eq!(cleared, serde_json::from_value::<Vec<String>>(fixture()["api_key_columns"].clone()).unwrap());
}

/// Tables whose every row the scrub deletes.
const DELETED_TABLES: [&str; 6] = ["fee_api_keys", "exchange_account_credentials", "exchange_account_imports", "ibkr_bindings", "ibkr_bot_accounts", "ibkr_reservations"];

#[test]
fn every_secret_shaped_column_is_reviewed() {
    let dir = common::baseline_install();
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let shape = regex::Regex::new(r"_enc$|token|secret|password|passphrase|_digest$|^key$|otp|signature|private").unwrap();
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let mut unreviewed = vec![];
    for t in &tables {
        if export::SESSION_TABLES.contains(&t.as_str()) || DELETED_TABLES.contains(&t.as_str()) { continue; }
        let cols: Vec<String> = c.prepare("SELECT name FROM pragma_table_info(?1)").unwrap().query_map([t], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for col in cols.iter().filter(|c| shape.is_match(c)) {
            let k = (t.as_str(), col.as_str());
            if deltabadger::rotation::COVERAGE.contains(&k) || export::CLEARED.contains(&k) || export::REVIEWED_PLAIN.iter().any(|(r, _)| *r == k) { continue; }
            unreviewed.push(format!("{t}.{col}"));
        }
    }
    assert!(unreviewed.is_empty(), "secret-shaped columns with no export decision: {unreviewed:?}");
}

/// An install holding every credential this build stores, each under `cipher` where it is stored encrypted, plus the
/// tokens and sessions. Returns (every scrubbed value, plaintext and ciphertext alike; the kept ciphertexts).
fn credentials(c: &Connection, cipher: &Cipher, s: &seed::Seeded) -> (Vec<String>, Vec<String>) {
    let t = "2026-01-01 00:00:00";
    let mut gone: Vec<String> = vec![];
    let enc = |plain: &str, gone: &mut Vec<String>| { let e = cipher.encrypt(plain); gone.push(plain.into()); gone.push(e.clone()); e };
    let cols = ["key", "secret", "passphrase", "access_token", "rsa_signature_key", "rsa_encryption_key", "dh_param"];
    let vals: Vec<String> = cols.iter().map(|col| enc(&format!("PLANTED-api-{col}"), &mut gone)).collect();
    c.execute(&format!("UPDATE api_keys SET {} WHERE id = ?1", cols.iter().enumerate().map(|(i, col)| format!("{col} = ?{}", i + 2)).collect::<Vec<_>>().join(", ")),
              rusqlite::params_from_iter(std::iter::once(Sql::Integer(s.api_key_id)).chain(vals.into_iter().map(Sql::Text)))).unwrap();
    let (fk, fs) = (enc("PLANTED-fee-key", &mut gone), enc("PLANTED-fee-secret", &mut gone));
    c.execute("INSERT INTO fee_api_keys (exchange_id, key, secret) VALUES (?1, ?2, ?3)", (s.exchange_id, fk, fs)).unwrap();
    for (k, v) in [("smtp_password", "PLANTED-smtp-password"), ("coingecko_api_key", "PLANTED-coingecko"), ("market_data_token", "PLANTED-market-token"),
                   ("proxy_kraken", "PLANTED-proxy"), ("rust_account_identity_key_v1", "PLANTED-identity-key")] {
        let e = enc(v, &mut gone);
        c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", (k, e, t)).unwrap();
    }
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('mystery_future_secret', 'PLANTED-mystery', ?1, ?1)", [t]).unwrap();
    gone.push("PLANTED-mystery".into());
    let otp = cipher.encrypt_binary("KEPTOTPSEED");
    let address = cipher.encrypt("bc1qkeptaddress");
    c.execute("UPDATE users SET otp_secret_key = ?1, otp_module = 1, reset_password_token = 'PLANTED-reset', confirmation_token = 'PLANTED-confirm', remember_created_at = ?2", (&otp, t)).unwrap();
    gone.extend(["PLANTED-reset".to_string(), "PLANTED-confirm".to_string()]);
    c.execute("INSERT INTO rules (type, user_id, exchange_id, asset_id, address, status, settings, created_at, updated_at) VALUES ('Rules::Withdrawal', ?1, ?2, ?3, ?4, 1, '{}', ?5, ?5)",
              (s.user_id, s.exchange_id, s.btc, &address, t)).unwrap();
    let rule = c.last_insert_rowid();
    c.execute("INSERT INTO oauth_applications (name, uid, secret, redirect_uri, created_at, updated_at) VALUES ('MCP', 'uid-1', 'PLANTED-client-secret', 'http://localhost/cb', ?1, ?1)", [t]).unwrap();
    let app = c.last_insert_rowid();
    c.execute("INSERT INTO oauth_access_tokens (application_id, resource_owner_id, token, refresh_token, created_at) VALUES (?1, ?2, 'PLANTED-bearer', 'PLANTED-refresh', ?3)", (app, s.user_id, t)).unwrap();
    c.execute("INSERT INTO oauth_access_grants (application_id, resource_owner_id, token, expires_in, redirect_uri, created_at) VALUES (?1, ?2, 'PLANTED-grant', 600, 'http://localhost/cb', ?3)", (app, s.user_id, t)).unwrap();
    c.execute("INSERT INTO connected_clients (user_id, oauth_application_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", (s.user_id, app, t)).unwrap();
    gone.extend(["PLANTED-client-secret", "PLANTED-bearer", "PLANTED-refresh", "PLANTED-grant"].map(String::from));
    let bot = seed::insert_bot(c, s, &BotSpec::weekly(10.0, t));
    c.execute("INSERT INTO bot_signals (bot_id, amount, token, enabled, created_at, updated_at) VALUES (?1, 10, 'PLANTED-signal-token', 1, ?2, ?2)", (bot, t)).unwrap();
    gone.push("PLANTED-signal-token".into());
    // Rust's own: an account identity, its credential digest, an IBKR binding, a prepared withdrawal; an owed one is kept.
    let id_enc = enc("PLANTED-account-id", &mut gone);
    c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, fingerprint, account_id_enc, created_at, updated_at) VALUES (?1, ?2, 'PLANTED-fingerprint', ?3, ?4, ?4)",
              (s.user_id, s.exchange_id, &id_enc, t)).unwrap();
    c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, material_digest, account_id_enc) VALUES (?1, ?2, 'PLANTED-material-digest', ?3)",
              (s.api_key_id, s.exchange_id, &id_enc)).unwrap();
    gone.extend(["PLANTED-fingerprint", "PLANTED-material-digest"].map(String::from));
    let bind_enc = enc("PLANTED-ibkr-account", &mut gone);
    c.execute("INSERT INTO ibkr_bindings (api_key_id, account_ref, account_fingerprint, account_id_enc, bound_at) VALUES (?1, 'ref', 'PLANTED-ibkr-fp', ?2, ?3)", (s.api_key_id, &bind_enc, t)).unwrap();
    gone.push("PLANTED-ibkr-fp".into());
    let prepared = enc("PLANTED-prepared-params", &mut gone);
    let owed = cipher.encrypt(&json!({ "rule_id": rule, "user_id": s.user_id, "exchange_id": s.exchange_id, "asset_id": s.btc, "exchange": "Kraken",
        "symbol": "BTC", "coin": "XBT", "client_id": "cid", "destination": { "kind": "Kraken", "coin": "XBT", "key": "my-wallet", "address": "bc1qowed", "method": "Bitcoin" },
        "amount": "0.01", "fee_cap": "0.001", "free_balance": "1", "percentage": "100", "created_at": t, "wallet_type": null }).to_string());
    for (owner, state, params) in [("rule:900".to_string(), "prepared", &prepared), (format!("rule:{rule}"), "dispatched", &owed)] {
        c.execute("INSERT INTO rust_withdrawal_intents (owner, state, client_id, created_at, api_key_id, credential_digest, host, account_scope, key_identity, params_enc, fee_basis, budget, free_balance) \
                   VALUES (?1, ?2, 'cid', ?3, ?4, 'abababababababababababababababababababababababababababababababab', 'host', 'spot', 'kid', ?5, 'flat', '1', '1')", (owner, state, t, s.api_key_id, params)).unwrap();
    }
    // A job's error text echoing a platform token (data-api's non-HTML reply), and the index provider's bare fingerprint.
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('rust_job.data_api_sync', ?1, ?2, ?2)",
              (json!({ "last_run_at": "2026-01-01T00:00:00Z", "last_error": "401 {\"error\":\"PLANTED-data-api-token\"}" }).to_string(), t)).unwrap();
    gone.push("PLANTED-data-api-token".into());
    (gone, vec![otp, address, owed])
}

fn credential_install(b: &Box, cipher: &Cipher) -> (seed::Seeded, Vec<String>, Vec<String>) {
    let template = common::rails_install();
    install_into(b, template.path());
    let c = b.db();
    let s = seed::seed_kraken(&c, cipher);
    let (gone, kept) = credentials(&c, cipher, &s);
    (s, gone, kept)
}

// ---- 5: no planted secret anywhere ----------------------------------------------------------------------------------

#[test]
fn no_scrubbed_secret_is_in_any_output_byte_and_the_kept_values_are() {
    let b = Box::new();
    let cipher = seed::cipher();
    let (_, gone, kept) = credential_install(&b, &cipher);
    let printed = b.exported();
    let all = all_bytes(&b, &printed);
    for g in &gone { assert!(!contains(&all, g), "{g} is in the download"); }
    for k in &kept { assert!(contains(&std::fs::read(b.out("portable/production.sqlite3")).unwrap(), k), "a kept value is missing"); }
    let out = Connection::open(b.out("portable/production.sqlite3")).unwrap();
    let job: String = out.query_row("SELECT value FROM app_configs WHERE key = 'rust_job.data_api_sync'", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&job).unwrap(), json!({ "last_run_at": "2026-01-01T00:00:00Z" }));
    let intents: Vec<String> = out.prepare("SELECT state FROM rust_withdrawal_intents").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(intents, ["dispatched"]);
}

#[test]
fn encrypted_values_only_where_kept() {
    let b = Box::new();
    let cipher = seed::cipher();
    credential_install(&b, &cipher);
    b.exported();
    let out = Connection::open(b.out("portable/production.sqlite3")).unwrap();
    let tables: Vec<String> = out.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let mut found = vec![];
    for t in tables {
        let (cols, rs) = rows(&out, &t);
        for r in rs {
            for (col, v) in cols.iter().zip(&r) {
                let text = match v { Sql::Text(s) => s.clone(), Sql::Blob(b) => String::from_utf8_lossy(b).into_owned(), _ => continue };
                if deltabadger::crypto::is_envelope(&text) {
                    let key = if t == "app_configs" { out.query_row("SELECT key FROM app_configs WHERE value = ?1", [&text], |r| r.get::<_, String>(0)).unwrap() } else { String::new() };
                    found.push((t.clone(), col.clone(), key));
                }
            }
        }
    }
    for (t, col, key) in &found {
        let policy = export::POLICY.iter().find(|((pt, pc), _)| pt == t && pc == col).map(|(_, p)| *p);
        let ok = match policy { Some(export::Policy::Keep(_)) => true, Some(export::Policy::FilterAppConfigs) => export::app_config_kept(key), _ => false };
        assert!(ok, "an encrypted value is kept in {t}.{col} {key}");
    }
    assert!(found.iter().any(|(t, ..)| t == "users") && found.iter().any(|(t, ..)| t == "rules"), "{found:?}");
}

// ---- 5b: the canonical projection -----------------------------------------------------------------------------------

/// `sqlite_master` of a fresh install as `bootstrap` builds its schema.
fn bootstrap_schema() -> Vec<(String, String, String, Option<String>)> {
    let c = Connection::open_in_memory().unwrap();
    deltabadger::store::create_schema(&c).unwrap();
    deltabadger::store::apply_pending(&c).unwrap();
    schema(&c)
}
fn schema(c: &Connection) -> Vec<(String, String, String, Option<String>)> {
    c.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}

#[test]
fn input_ddl_never_reaches_the_download() {
    let b = Box::new();
    rails_input(&b);
    let c = b.db();
    // The stray-conditions migration (it updates bots) and the import-cutoff one (it deletes app_configs rows) still pending.
    c.execute_batch("DELETE FROM rust_schema_migrations WHERE version IN ('20261025000001', '20261023000001'); DROP TABLE import_cutoffs").unwrap();
    c.execute_batch("
        UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true')) WHERE type = 'Bots::DcaIndex';
        ALTER TABLE bots ADD COLUMN sneaky TEXT DEFAULT 'SENTINEL-column';
        UPDATE bots SET sneaky = 'SENTINEL-column-value';
        CREATE TABLE sneaky_table (id INTEGER PRIMARY KEY AUTOINCREMENT, x TEXT);
        INSERT INTO sneaky_table (x) VALUES ('SENTINEL-table');
        CREATE VIEW sneaky_view AS SELECT 'SENTINEL-view' AS x;
        CREATE INDEX sneaky_index ON bots(label) WHERE label <> 'SENTINEL-index';
        UPDATE bots SET label = 'cut' WHERE id = 2;
        CREATE TRIGGER sneaky_trigger AFTER UPDATE ON bots BEGIN UPDATE bots SET label = (SELECT secret FROM api_keys ORDER BY id LIMIT 1) WHERE id = NEW.id; END;
        UPDATE sqlite_sequence SET seq = 'SENTINEL-seq' WHERE name = 'users';
        UPDATE sqlite_sequence SET seq = 9000 WHERE name = 'bots';
        INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('rust_kucoin_csv.cutoff:1:1', '{}', '2026-01-01', '2026-01-01');
        ALTER TABLE app_configs ADD COLUMN extra TEXT;
        UPDATE app_configs SET extra = 'cut' WHERE key = 'rust_kucoin_csv.cutoff:1:1';
        CREATE UNIQUE INDEX app_configs_extra ON app_configs(extra);
        PRAGMA writable_schema = ON;
        UPDATE sqlite_master SET sql = replace(sql, '\"time_zone\" varchar DEFAULT ''UTC''', '\"time_zone\" varchar DEFAULT ''SENTINEL-zone''') WHERE name = 'users';
        UPDATE sqlite_master SET sql = replace(sql, '\"label\" varchar', '\"label\" varchar DEFAULT ''SENTINEL-fk'' REFERENCES app_configs(extra) ON DELETE SET DEFAULT') WHERE name = 'bots';
        PRAGMA writable_schema = OFF;
    ").unwrap();
    drop(c);
    let printed = b.exported();
    let all = all_bytes(&b, &[]);
    assert!(!contains(&all, "SENTINEL"), "an input sentinel is in the download");
    assert!(!contains(&all, "planted-export-credential-secret"), "the trigger's copy is in the download");
    for name in ["table sneaky_table", "view sneaky_view", "trigger sneaky_trigger", "index sneaky_index", "column bots.sneaky", "column app_configs.extra"] {
        assert!(printed.iter().any(|l| l.ends_with(name)), "{name} not named: {printed:?}");
    }
    let out = Connection::open(b.out("portable/production.sqlite3")).unwrap();
    assert_eq!(schema(&out), bootstrap_schema());
    assert_eq!(out.query_row("SELECT seq FROM sqlite_sequence WHERE name = 'bots'", [], |r| r.get::<_, i64>(0)).unwrap(), 9000);
    assert_eq!(out.query_row("SELECT typeof(seq) FROM sqlite_sequence WHERE name = 'users'", [], |r| r.get::<_, String>(0)).unwrap(), "integer");
    assert_eq!(out.query_row("SELECT label FROM bots WHERE id = 2", [], |r| r.get::<_, String>(0)).unwrap(), "cut");
    assert!(!contains(&printed.join("\n").into_bytes(), "SENTINEL"));
}

#[test]
fn a_generated_column_refuses_before_anything_is_written() {
    let b = Box::new();
    rails_input(&b);
    b.db().execute_batch("ALTER TABLE api_keys ADD COLUMN leak TEXT GENERATED ALWAYS AS (hex(secret)) VIRTUAL").unwrap();
    let e = b.export().unwrap_err();
    assert_eq!(e.exit_code(), 65, "{}", e.message());
    assert!(e.message().contains("api_keys.leak"), "{}", e.message());
    assert!(output_files(&b).is_empty());
}

#[test]
fn a_missing_canonical_column_refuses() {
    let b = Box::new();
    rails_input(&b);
    b.db().execute_batch("ALTER TABLE action_mcp_sessions DROP COLUMN consents").unwrap();
    let e = b.export().unwrap_err();
    assert_eq!(e.exit_code(), 65, "{}", e.message());
    assert!(output_files(&b).is_empty());
}

#[test]
fn migration_inserts_only_into_rust_tables() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let sql: String = std::fs::read_dir(&dir).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect::<Vec<_>>().join("\n");
    let created = regex::Regex::new(r#"(?i)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?"?(\w+)"?"#).unwrap();
    let created: Vec<String> = created.captures_iter(&sql).map(|c| c[1].to_lowercase()).collect();
    let inserts = regex::Regex::new(r#"(?i)INSERT\s+(?:OR\s+\w+\s+)?INTO\s+"?(\w+)"?"#).unwrap();
    for c in inserts.captures_iter(&sql) {
        assert!(created.contains(&c[1].to_lowercase()), "a Rust migration inserts into {}, which no Rust migration creates", &c[1]);
    }
}

// ---- app_configs: the allowlist and the source scan -----------------------------------------------------------------

/// The key a writer call's argument names: a literal, a `format!` prefix, a constant, or a key-building function, as far as
/// the source says; None when it only relays a key from elsewhere.
fn resolve(expr: &str, file: &str, consts: &BTreeMap<String, Vec<(String, String)>>, fns: &BTreeMap<String, Vec<(String, String)>>) -> Option<String> {
    let e = expr.trim().trim_start_matches('&').trim();
    let lookup = |m: &BTreeMap<String, Vec<(String, String)>>, name: &str| -> Option<String> {
        let all = m.get(name)?;
        all.iter().find(|(f, _)| f == file).or(all.first()).map(|(_, v)| v.clone())
    };
    if let Some(lit) = e.strip_prefix('"').and_then(|s| s.strip_suffix('"')) { return Some(lit.into()); }
    if let Some(body) = e.strip_prefix("format!(\"") {
        let fmt = body.split('"').next()?;
        let head = fmt.split('{').next().unwrap_or("");
        if !head.is_empty() { return Some(format!("{head}*")); }
        let name = fmt.trim_start_matches('{').split('}').next()?;
        if name.is_empty() { let arg = body.split(',').nth(1)?.trim().trim_end_matches(')'); return resolve(arg, file, consts, fns).map(|v| format!("{}*", v.trim_end_matches('*'))); }
        return lookup(consts, name).map(|v| format!("{v}*"));
    }
    let name = e.split('(').next()?.rsplit("::").next()?.trim();
    if e.contains('(') { return lookup(fns, name).map(|v| format!("{v}*")); }
    lookup(consts, name)
}

/// (file, key or `prefix*`, or `?expr` when unresolved) for every app_configs writer in `sources`.
fn written_keys(sources: &[(String, String)]) -> Vec<(String, String)> {
    let konst = regex::Regex::new(r#"const ([A-Z_]+): &(?:'static )?str = "([^"]*)""#).unwrap();
    let func = regex::Regex::new(r#"fn ([a-z_]+)\([^)]*\)\s*->\s*String\s*\{\s*(?:match [^{]+\{\s*Some\(\w+\) => )?format!\("([^"{]*)(?:\{(\w*)\})?"#).unwrap();
    let mut consts: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut fns: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (file, text) in sources {
        for c in konst.captures_iter(text) { consts.entry(c[1].into()).or_default().push((file.clone(), c[2].into())); }
    }
    for (file, text) in sources {
        for c in func.captures_iter(text) {
            let prefix = match c.get(3).map(|m| m.as_str()).filter(|n| !n.is_empty()) {
                Some(name) => format!("{}{}", &c[2], consts.get(name).and_then(|v| v.iter().find(|(f, _)| f == file).or(v.first())).map(|(_, v)| v.as_str()).unwrap_or("?")),
                None => c[2].to_string(),
            };
            fns.entry(c[1].into()).or_default().push((file.clone(), prefix));
        }
    }
    let call = regex::Regex::new(r"app_config::(set_plain|set_nullable|set)\(").unwrap();
    let raw = regex::Regex::new(r#"INSERT INTO app_configs[^"]*"[^,]*,\s*(?:rusqlite::)?(?:params!\[|\()"#).unwrap();
    let lease_upsert = regex::Regex::new(r"\bupsert\(primary, ").unwrap();
    let mut out = vec![];
    // The argument at `index` (0-based) of the call whose `(` is at `open`, split at depth-0 commas.
    let arg = |text: &str, open: usize, index: usize| -> String {
        let (mut depth, mut n, mut start, mut in_str) = (0i32, 0usize, open + 1, false);
        let bytes = text.as_bytes();
        for (i, &ch) in bytes.iter().enumerate().skip(open + 1) {
            match ch {
                b'"' if bytes[i - 1] != b'\\' => in_str = !in_str,
                b'(' | b'[' | b'{' if !in_str => depth += 1,
                b')' | b']' | b'}' if !in_str => { if depth == 0 { return if n == index { text[start..i].to_string() } else { String::new() } } depth -= 1 }
                b',' if !in_str && depth == 0 => { if n == index { return text[start..i].to_string() } n += 1; start = i + 1 }
                _ => {}
            }
        }
        String::new()
    };
    for (file, text) in sources {
        if file.ends_with("src/app_config.rs") || file.ends_with("src/rotation.rs") || file.ends_with("src/export.rs") { continue; }
        let mut found = |expr: String| {
            let key = resolve(&expr, file, &consts, &fns).unwrap_or_else(|| format!("?{}", expr.trim()));
            out.push((file.clone(), key));
        };
        for m in call.find_iter(text) {
            let index = if m.as_str().contains("set_plain") { 1 } else { 2 };
            found(arg(text, m.end() - 1, index));
        }
        for m in raw.find_iter(text) { found(arg(text, m.end() - 1, 0)); }
        if file.ends_with("src/lease.rs") { for m in lease_upsert.find_iter(text) { found(arg(text, m.start() + "upsert".len(), 1)); } }
    }
    out
}

/// Writers whose key is relayed from a caller the scan already resolves (or from a test's fixture), by file and expression.
const RELAYED: &[(&str, &str, &str)] = &[
    ("src/web/settings/platform.rs", "key", "a platform setting relayed from its caller's list, each a Rails setting"),
    ("src/web/settings/mail.rs", "key", "the SMTP settings: smtp_ keys, deleted"),
    ("src/web/settings/market_data.rs", "key", "the market-data settings: deleted"),
    ("src/web/tracker/first_sync.rs", "key", "tests: a fixture row"),
    ("src/store.rs", "key", "the seed's smtp_provider / market_data_provider defaults: deleted"),
    ("src/sync/balances.rs", "key", "rust_sync.kraken_unresolved, the venue's record key"),
    ("src/sync/binance_repair.rs", "&name", "bucket_name: rust_binance_a12_v1:bucket:<…>"),
    ("src/sync/binance_writer.rs", "&name", "scope: rust_binance_a12_v1:bucket:<…>"),
    ("src/sync/kucoin_hf.rs", "&format!(\"{record}{id}\")", "record = venue::kucoin::ORDER_RECORD (rust_kucoin_order:)"),
    ("src/sync/kucoin_hf_ledger.rs", "route.progress_key(key_id)", "Route::progress: rust_sync.kucoin_hf / rust_sync.kucoin_uta"),
    ("src/sync/ledger.rs", "key", "put_config: import_key / splits_key, rust_sync.ledger*"),
    ("src/tracker/snapshot.rs", "&key", "jobs::state::key(\"snapshot_origin\", …): rust_job.snapshot_origin:<…>"),
    ("src/web/settings/preferences.rs", "key", "the SMTP form's smtp_ keys: deleted"),
    ("src/web/setup.rs", "key", "the platform claim: market_data_*, proxy_*, platform_connected_at (deleted), setup_sync_status (kept)"),
    ("src/jobs/synchronizer.rs", "key", "claim: rust_job.crawl_claim.<…> and rust_job.asset_backfill_claim, named by its callers"),
    ("src/lease.rs", "key", "upsert: engine_lease and the rust_*_at markers, named by its callers"),
    ("src/engine/rules.rs", "key", "write_journal: rust_rule_import / rust_rule_job_cleanup, named by its callers"),
    ("src/engine/reconcile.rs", "&key", "rust_kucoin_origin:<tx>, built on the line before"),
    ("src/web/tracker/first_sync.rs", "crate::tracker::cache::key(8)", "rust_tracker_ledger.<owner>"),
    ("src/web/tracker/first_sync.rs", "crate::tracker::cache::key(7)", "rust_tracker_ledger.<owner>"),
];

fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "rs") {
                let rel = p.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap().to_string_lossy().into_owned();
                out.push((rel, std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    let mut v = vec![];
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut v);
    v.sort();
    v
}

fn unreviewed(sources: &[(String, String)]) -> Vec<String> {
    let reviewed = |key: &str| {
        let probe = key.trim_end_matches('*');
        let prefix = key.ends_with('*');
        let matches = |k: &export::Key| match k {
            export::Key::Exact(e) => !prefix && *e == probe, // a family of keys is reviewed only by a prefix
            export::Key::Prefix(p) => probe.starts_with(p),
        };
        export::APP_CONFIGS_DELETED.iter().any(matches) || export::APP_CONFIGS.iter().any(|(k, _)| matches(k))
    };
    written_keys(sources).into_iter().filter(|(file, key)| match key.strip_prefix('?') {
        Some(expr) => !RELAYED.iter().any(|(f, e, _)| file == f && expr == *e),
        None => !reviewed(key),
    }).map(|(f, k)| format!("{f}: {k}")).collect()
}

#[test]
fn every_app_config_key_in_source_is_reviewed() {
    let src = sources();
    let keys = written_keys(&src);
    assert!(keys.len() > 60, "the scan found only {} writers", keys.len());
    let left = unreviewed(&src);
    assert!(left.is_empty(), "app_configs keys the export has no decision for (add to export::APP_CONFIGS or delete them):\n{}", left.join("\n"));
    // A planted new key fails.
    let mut planted = src.clone();
    planted.push(("src/planted.rs".into(), "fn f(c: &C) { app_config::set_plain(c, \"rust_new_secret_token\", v, now); }".into()));
    assert_eq!(unreviewed(&planted), ["src/planted.rs: rust_new_secret_token"]);
}

#[test]
fn an_unknown_app_config_key_is_not_in_the_download() {
    let b = Box::new();
    rails_input(&b);
    b.db().execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('brand_new_setting', 'PLANTED-unknown', '2026-01-01', '2026-01-01')", []).unwrap();
    b.exported();
    assert!(!contains(&all_bytes(&b, &[]), "PLANTED-unknown"));
}

// ---- 5c, 5d, 6: the source, its sidecars, the refusals --------------------------------------------------------------

#[test]
fn the_source_queue_absent_empty_or_garbage_is_no_obstacle() {
    for queue in [None, Some(&b""[..]), Some(&b"not a database at all"[..])] {
        let b = Box::new();
        rails_input(&b);
        let q = b.s.storage.join("production_queue.sqlite3");
        match queue { None => std::fs::remove_file(&q).unwrap(), Some(bytes) => std::fs::write(&q, bytes).unwrap() }
        let before = listing(&b.s.storage);
        b.exported();
        assert_eq!(listing(&b.s.storage), before);
        let out = Connection::open(b.out("portable/production_queue.sqlite3")).unwrap();
        assert_eq!(out.query_row("SELECT count(*) FROM solid_queue_jobs", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
}

#[test]
fn a_wal_source_is_read_with_its_wal_and_left_byte_for_byte() {
    for stale_shm in [false, true] {
        let b = Box::new();
        let built = Box::new();
        rails_input(&built);
        // A writer with checkpoints off: the newest rows live only in its -wal; copy the files while it is open.
        let w = built.db();
        w.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0").unwrap();
        w.execute("INSERT INTO users (email, encrypted_password, created_at, updated_at) VALUES ('only-in-wal@example.test', 'x', '2026-01-01', '2026-01-01')", []).unwrap();
        for f in ["production.sqlite3", "production.sqlite3-wal", "production_queue.sqlite3"] { std::fs::copy(built.s.storage.join(f), b.s.storage.join(f)).unwrap(); }
        drop(w);
        assert!(std::fs::metadata(b.s.storage.join("production.sqlite3-wal")).unwrap().len() > 0);
        if stale_shm { std::fs::write(b.s.storage.join("production.sqlite3-shm"), vec![7u8; 32768]).unwrap(); }
        let before = listing(&b.s.storage);
        b.exported();
        assert_eq!(listing(&b.s.storage), before, "stale_shm={stale_shm}");
        let out = Connection::open(b.out("portable/production.sqlite3")).unwrap();
        assert_eq!(out.query_row("SELECT count(*) FROM users WHERE email = 'only-in-wal@example.test'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }
}

fn refused(b: &Box, env: &dyn Fn(&str) -> Option<String>, why: &str) {
    let before = listing(&b.s.storage);
    let e = export::run(&b.s, env, now()).unwrap_err();
    assert_eq!(e.exit_code(), 1, "{why}: {}", e.message());
    assert_eq!(listing(&b.s.storage), before, "{why}");
    assert!(output_files(b).is_empty(), "{why}");
}

#[test]
fn an_unsafe_environment_is_refused_and_nothing_changes() {
    let b = Box::new();
    rails_input(&b);
    refused(&b, &|_| None, "no EXPORT_SCRATCH");
    refused(&b, &|k| (k == "EXPORT_SCRATCH").then(|| "true".into()), "EXPORT_SCRATCH not 1");
    for var in ["DATABASE_PATH", "QUEUE_DATABASE_PATH", "DATABASE_URL", "PRIMARY_DATABASE_URL"] {
        refused(&b, &|k| match k { "EXPORT_SCRATCH" => Some("1".into()), k if k == var => Some("/elsewhere".into()), _ => None }, var);
    }
    std::fs::remove_file(&b.s.marker).unwrap();
    refused(&b, &scratch_env, "no container marker");
    std::fs::write(&b.s.marker, "").unwrap();
    // A non-empty output.
    std::fs::create_dir(&b.s.output).unwrap();
    std::fs::write(b.s.output.join("left-over"), "x").unwrap();
    let e = b.export().unwrap_err();
    assert_eq!(e.exit_code(), 1);
    std::fs::remove_dir_all(&b.s.output).unwrap();
    // A linked primary, a hard-linked one, a hot journal.
    let elsewhere = b.s.storage.parent().unwrap().join("elsewhere.sqlite3");
    std::fs::rename(b.primary(), &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, b.primary()).unwrap();
    refused(&b, &scratch_env, "symlinked primary");
    std::fs::remove_file(b.primary()).unwrap();
    std::fs::hard_link(&elsewhere, b.primary()).unwrap();
    refused(&b, &scratch_env, "hard-linked primary");
    std::fs::remove_file(&elsewhere).unwrap();
    std::fs::write(b.s.storage.join("production.sqlite3-journal"), "x").unwrap();
    refused(&b, &scratch_env, "hot journal");
    std::fs::remove_file(b.s.storage.join("production.sqlite3-journal")).unwrap();
    // Another engine holds the flock.
    std::fs::write(b.s.storage.join(".engine.lock"), "").unwrap();
    let paths = deltabadger::store::Paths::from_env(&|_| None, &b.s.storage);
    let held = deltabadger::lease::lock(&paths, now()).unwrap();
    refused(&b, &scratch_env, "flock held");
    drop(held);
    // Free again: the lock file is used, never created or removed.
    let before = listing(&b.s.storage);
    b.exported();
    assert_eq!(listing(&b.s.storage), before);
}

#[test]
fn an_absent_lock_file_is_never_created() {
    let b = Box::new();
    rails_input(&b);
    assert!(!b.s.storage.join(".engine.lock").exists());
    let before = listing(&b.s.storage);
    b.exported();
    assert_eq!(listing(&b.s.storage), before);
}

#[test]
fn a_copy_from_a_newer_build_is_incompatible() {
    let b = Box::new();
    rails_input(&b);
    b.db().execute("INSERT INTO rust_schema_migrations (version, applied_at) VALUES ('20990101000001', '2099-01-01')", []).unwrap();
    let before = listing(&b.s.storage);
    let e = b.export().unwrap_err();
    assert_eq!(e.exit_code(), 65, "{}", e.message());
    assert_eq!(listing(&b.s.storage), before);
    assert!(output_files(&b).is_empty());
}

#[test]
fn the_cli_outside_a_container_refuses_with_1() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("export").env_remove("EXPORT_SCRATCH").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("EXPORT_SCRATCH=1"));
}

// ---- 3, 9: restores -------------------------------------------------------------------------------------------------

/// launchpad's 3.0 `.secrets` (PLAN-B4b §6): a fresh SECRET_KEY_BASE and the explicit pair.
fn secrets_file(keys: &EncryptionKeys) -> String {
    format!("SECRET_KEY_BASE={}\nACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY={}\nACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT={}\n",
            hex::encode([rand::random::<[u8; 32]>(), rand::random::<[u8; 32]>()].concat()), keys.primary_key, keys.key_derivation_salt)
}

/// `portable/.` with launchpad's `.secrets` in a new storage dir; the command run there with that file's values.
fn restore(b: &Box, keys: &EncryptionKeys) -> (tempfile::TempDir, Vec<(String, String)>) {
    let to = tempfile::tempdir().unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(b.out(&format!("portable/{f}")), to.path().join(f)).unwrap(); }
    let secrets = secrets_file(keys);
    std::fs::write(to.path().join(".secrets"), &secrets).unwrap();
    let env = secrets.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    (to, env)
}
fn cli(dir: &Path, env: &[(String, String)], args: &[&str]) -> std::process::Output {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.args(args).env("STORAGE_DIR", dir).env("IBKR_CATALOGUE_URL", "off")
        .env_remove("DATABASE_URL").env_remove("DATABASE_PATH").env_remove("QUEUE_DATABASE_PATH").env_remove("DELTABADGER_PLATFORM")
        .env_remove("SECRET_KEY_BASE").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT");
    for (k, v) in env { c.env(k, v); }
    c.output().unwrap()
}
fn ok(o: &std::process::Output) -> String {
    assert!(o.status.success(), "{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn the_shared_vector_is_rusts_own_derivation() {
    let keys = EncryptionKeys::resolve(&|_| None, VECTOR_SECRET).unwrap();
    assert_eq!((keys.primary_key.as_str(), keys.key_derivation_salt.as_str()), (VECTOR_PRIMARY, VECTOR_SALT));
}

#[test]
fn a_3_0_export_restores_on_3_0_signed_in_with_the_same_password_and_2fa() {
    let keys = EncryptionKeys::resolve(&|_| None, VECTOR_SECRET).unwrap();
    let cipher = Cipher::new(&keys);
    let b = Box::new();
    let (_s, _, _) = credential_install(&b, &cipher);
    let c = b.db();
    let hash = deltabadger::crypto::hash_password("kept-password-123").unwrap();
    c.execute("UPDATE users SET encrypted_password = ?1", [&hash]).unwrap();
    // A Rust-owned install (its lease encrypted under the pair), taken over earlier.
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('engine_lease', ?1, '2026-01-02', '2026-01-02')",
              [cipher.encrypt(r#"{"engine":"rust","version":"0.1.0","since":"2026-01-02T00:00:00Z"}"#)]).unwrap();
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('rust_taken_over_at', '2026-01-02T00:00:00.000Z', '2026-01-02', '2026-01-02')", []).unwrap();
    let lease: String = c.query_row("SELECT value FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get(0)).unwrap();
    drop(c);
    b.exported();
    let (to, env) = restore(&b, &keys);
    ok(&cli(to.path(), &env, &["check"]));
    let r = Connection::open(to.path().join("production.sqlite3")).unwrap();
    assert_eq!(r.query_row("SELECT value FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get::<_, String>(0)).unwrap(), lease, "D-EXPORT-LEASE-KEPT");
    let (stored, otp): (String, String) = r.query_row("SELECT encrypted_password, otp_secret_key FROM users LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(deltabadger::crypto::verify_password("kept-password-123", &stored));
    let restored = Cipher::new(&EncryptionKeys { primary_key: env[1].1.clone(), key_derivation_salt: env[2].1.clone() });
    assert_eq!(restored.decrypt(&otp).unwrap(), "KEPTOTPSEED");
    let address: String = r.query_row("SELECT address FROM rules WHERE type = 'Rules::Withdrawal'", [], |r| r.get(0)).unwrap();
    assert_eq!(restored.decrypt(&address).unwrap(), "bc1qkeptaddress");
    let statuses: Vec<i64> = r.prepare("SELECT status FROM bots UNION SELECT status FROM rules").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert!(statuses.iter().all(|s| [2, 3, 7].contains(s)), "{statuses:?}");
    // No key material: the api_keys rows are kept, their columns empty (Rails), no fee key, no setting to re-enter.
    let report = ok(&cli(to.path(), &env, &["encryption-report"]));
    assert!(report.contains("Every stored value still decrypts under the current secret"), "{report}");
    assert!(report.contains("Fee API keys — will also be cleared: (none)") && report.contains("Settings that will reset:    (none)"), "{report}");
    assert_eq!(r.query_row("SELECT count(*) FROM api_keys WHERE coalesce(key, secret, passphrase, access_token, rsa_signature_key, rsa_encryption_key, dh_param) IS NOT NULL OR status <> 2", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    // A session cookie sealed under the old SECRET_KEY_BASE is no session under the download's.
    let derive = |secret: &str| -> [u8; 32] {
        use hmac::Mac;
        let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"deltabadger rust session v1");
        mac.finalize().into_bytes().into()
    };
    let data = deltabadger::web::session::SessionData { user: Some((1, "$2a$04$salt".into())), ..Default::default() };
    let cookie = deltabadger::web::session::seal(&derive(VECTOR_SECRET), &data, now());
    assert!(deltabadger::web::session::open(&derive(VECTOR_SECRET), &cookie, now()).is_some());
    assert!(deltabadger::web::session::open(&derive(&env[0].1), &cookie, now()).is_none());
}

#[test]
fn an_owed_withdrawal_survives_is_never_sent_again_and_resolves_by_hand() {
    let keys = EncryptionKeys::resolve(&|_| None, VECTOR_SECRET).unwrap();
    let cipher = Cipher::new(&keys);
    let b = Box::new();
    credential_install(&b, &cipher);
    let rule: i64 = b.db().query_row("SELECT id FROM rules WHERE type = 'Rules::Withdrawal'", [], |r| r.get(0)).unwrap();
    b.exported();
    let (to, env) = restore(&b, &keys);
    let out = ok(&cli(to.path(), &env, &["resolve-withdrawal", &rule.to_string(), "--not-sent"]));
    assert!(out.contains("resolved"), "{out}");
    let r = Connection::open(to.path().join("production.sqlite3")).unwrap();
    assert_eq!(r.query_row("SELECT count(*) FROM rust_withdrawal_intents", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn a_2x_export_is_taken_over_by_3_0_and_exports_again() {
    use std::io::Read;
    let b = Box::new();
    let mut primary = vec![];
    flate2::read::GzDecoder::new(&include_bytes!("fixtures/portable_export_2x.sqlite3.gz")[..]).read_to_end(&mut primary).unwrap();
    std::fs::write(b.primary(), primary).unwrap();
    deltabadger::store::create_queue_schema(&Connection::open(b.s.storage.join("production_queue.sqlite3")).unwrap()).unwrap();
    // Restore 5.2: Rails' download, as Rails' recorder left it, passes `check` and is taken over (no lease: from Rails).
    // Rails' seed holds an open order with no key to look after it, which the takeover rightly refuses: it is settled first.
    b.db().execute("UPDATE transactions SET external_status = 3 WHERE external_status = 1", []).unwrap();
    let paths = deltabadger::store::Paths::from_env(&|_| None, &b.s.storage);
    deltabadger::store::check(&paths).unwrap();
    {
        let lock = deltabadger::lease::lock(&paths, now()).unwrap();
        deltabadger::store::migrate(&paths, &lock).unwrap();
        let o = deltabadger::store::open(&paths).unwrap();
        let cipher = Cipher::new(&EncryptionKeys::resolve(&|_| None, "portable-export-recorder-secret-portable-export-recorder-secret-portable-export-recorder-secret-portable-export-recorder-secret-").unwrap());
        deltabadger::engine::handover::take_over(&lock, &o, &cipher, deltabadger::VERSION, now()).unwrap();
    }
    let lease: String = b.db().query_row("SELECT value FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get(0)).unwrap();
    std::fs::remove_file(b.s.storage.join(".engine.lock")).ok();
    // 5.3: then exported, and that download passes `check` again, its lease the takeover's (D-EXPORT-LEASE-KEPT).
    b.exported();
    let to = tempfile::tempdir().unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(b.out(&format!("portable/{f}")), to.path().join(f)).unwrap(); }
    deltabadger::store::check(&deltabadger::store::Paths::from_env(&|_| None, to.path())).unwrap();
    let r = Connection::open(to.path().join("production.sqlite3")).unwrap();
    assert_eq!(schema(&r), bootstrap_schema());
    assert_eq!(r.query_row("SELECT value FROM app_configs WHERE key = 'engine_lease'", [], |r| r.get::<_, String>(0)).unwrap(), lease);
}

// ---- 5.1: no order before the owner acts ----------------------------------------------------------------------------

struct Factory(deltabadger::venue::http::ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { common::scripted::venue(&self.0) }
}

#[tokio::test(flavor = "current_thread")]
async fn a_restored_install_places_no_order_before_the_owner_acts() {
    use deltabadger::engine::{basket, liquidation, model, redeploy, run, FixedClock};
    let cipher = seed::cipher();
    let b = Box::new();
    let template = common::baseline_install();
    install_into(&b, template.path());
    let c = b.db();
    let s = seed::seed_alpaca(&c, &cipher);
    let (eth, _) = seed::add_eth_sol(&c, &s);
    let t0: DateTime<Utc> = "2026-09-01T12:30:00Z".parse().unwrap();
    let basket_bot = |c: &Connection, transient: Value| {
        let spec = BotSpec { status: 1, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }.with("rebalance_enabled", json!(true)).weights(&[(s.btc, 0.5), (eth, 0.5)]);
        let id = seed::insert_bot(c, &s, &spec);
        basket::refresh_composition(c, &model::load_bot(c, id).unwrap(), "2026-09-01T09:00:00Z".parse().unwrap()).unwrap().unwrap();
        for (asset, qty, price, quote, n) in [(s.btc, "0.01", "64000", "640", 1), (eth, "0.04", "2500", "100", 2)] {
            seed::insert_row(c, &s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{id}-{n}"), "amount": qty, "amount_exec": qty,
                "quote_amount": quote, "quote_amount_exec": quote, "price": price, "created_at": "2026-09-01 10:00:01" }));
        }
        let pairs: Vec<(String, Value)> = transient.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let refs: Vec<(&str, Value)> = pairs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        model::merge_transient_compact(c, id, &refs).unwrap();
        id
    };
    // A swap with an owed buy after a filled sell, an unattempted buy, a requested sale, a requested redeploy.
    let sold = seed::insert_row(&c, &s, 0, s.btc, &json!({ "external_status": 2, "external_id": "OSOLD", "side": 1, "amount": "0.001", "amount_exec": "0.001",
        "quote_amount_exec": "64", "price": "64000", "created_at": "2026-09-01 11:00:00" }));
    let swap = basket_bot(&c, json!({ "rebalance_pending": { "phase": "buying", "sell_transaction_id": sold, "remaining_quote_amount": "64", "buy_attempted": false } }));
    let fresh = basket_bot(&c, json!({ "rebalance_pending": { "phase": "buying", "remaining_quote_amount": "50", "buy_attempted": false } }));
    let selling = basket_bot(&c, json!({ liquidation::REQUEST_KEY: { "id": "sell", "at": 1, "holdings": [["BTC", s.btc]] } }));
    let redeploying = basket_bot(&c, json!({ redeploy::REQUEST_KEY: { "id": "redeploy", "at": 1 } }));
    c.execute("UPDATE transactions SET bot_id = ?1 WHERE id = ?2", (swap, sold)).unwrap();
    // An API order queued under the old key.
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, credential_version, state, created_at, send_by) \
               VALUES (?1, ?2, 'buy', '10', 'quote', '{}', 'old', 'queued', ?3, ?4)", (s.user_id, swap, "2026-09-01 12:29:00", "2026-09-01 12:40:00")).unwrap();
    // A working withdrawal rule.
    c.execute("INSERT INTO rules (type, user_id, exchange_id, asset_id, address, status, settings, created_at, updated_at) VALUES ('Rules::Withdrawal', ?1, ?2, ?3, ?4, 1, '{}', ?5, ?5)",
              (s.user_id, s.exchange_id, s.btc, cipher.encrypt("bc1qaddr"), "2026-09-01 10:00:00")).unwrap();
    drop(c);
    b.exported();
    let to = tempfile::tempdir().unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(b.out(&format!("portable/{f}")), to.path().join(f)).unwrap(); }
    let paths = deltabadger::store::Paths::from_env(&|_| None, to.path());
    let o = deltabadger::store::open(&paths).unwrap();
    for id in [swap, fresh] { assert_eq!(model::load_bot(&o.primary, id).unwrap().transient["rebalance_pending"]["phase"], "ambiguous", "bot {id}"); }
    for (id, key) in [(selling, liquidation::REQUEST_KEY), (redeploying, redeploy::REQUEST_KEY)] { assert!(model::load_bot(&o.primary, id).unwrap().transient.get(key).is_none_or(Value::is_null), "bot {id}"); }
    // The owner re-enters the venue's key; one full scheduler pass.
    o.primary.execute("UPDATE api_keys SET key = ?1, secret = ?2, status = 1", (cipher.encrypt("new-key"), cipher.encrypt("new-secret"))).unwrap();
    // Bid and ask at one level (a sale is priced at the bid), the holdings at the venue, orders accepted.
    let t = common::scripted::script(json!({
        "GET /v1beta3/crypto/us/latest/quotes": [common::scripted::ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 }, "ETH/USD": { "ap": 2500, "bp": 2500 }, "SOL/USD": { "ap": 150, "bp": 150 } } }))],
        "GET /v2/positions": [common::scripted::ok(json!([{ "symbol": "BTCUSD", "asset_class": "crypto", "qty": "0.04", "qty_available": "0.04", "side": "long" },
                                                         { "symbol": "ETHUSD", "asset_class": "crypto", "qty": "0.16", "qty_available": "0.16", "side": "long" }]))],
    }));
    for n in 1..=6 { t.reply(&format!("GET /v2/orders/OTX-{n}"), 200, json!({ "id": format!("OTX-{n}"), "status": "new", "filled_qty": "0" })); }
    let lock = deltabadger::lease::lock(&paths, t0).unwrap();
    let mut e = run::Engine::new(o.primary, Factory(t.clone()), cipher.clone(), lock);
    e.inject_rebalance_at(t0); // the rebalance pass is due in this one
    run::step(&mut e, &FixedClock(t0)).await.unwrap();
    run::step(&mut e, &FixedClock(t0 + chrono::Duration::minutes(5))).await.unwrap();
    // Not even a read: a swap's owed buy and a requested sale are what would go out first (checked by hand with each
    // hold turned off: the sale is placed, then both owed buys).
    assert!(t.posted_orders().is_empty(), "orders placed after a restore: {:?}", t.posted_orders());
    assert!(t.requests().is_empty(), "venue requests after a restore: {:?}", t.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect::<Vec<_>>());
}

/// Codex branch review: a bot with no venue (Automation::ExchangeConnectable's optional association) gets its file, header
/// only, instead of failing the whole export.
#[test]
fn a_bot_without_a_venue_gets_its_file() {
    let b = Box::new();
    rails_input(&b);
    b.db().execute("INSERT INTO bots (id, user_id, type, label, status, settings, transient_data, created_at, updated_at) \
                    VALUES (99, 1, 'Bots::DcaSingleAsset', 'No venue', 2, '{}', '{}', '2026-01-01', '2026-01-01')", []).unwrap();
    b.exported();
    assert_eq!(std::fs::read_to_string(b.out("csv/bots/99-no-venue.csv")).unwrap(), "Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status\n");
}
