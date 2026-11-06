//! Secret rotation (PLAN-A29): Rails' `deltabadger:encryption:report`, `:reset` and `:derived_keys`
//! (lib/tasks/encryption.rake) as `deltabadger encryption-report | encryption-reset | encryption-derived-keys`, and the
//! boot warnings of config/initializers/secret_key_base_guard.rb and encryption_key_check.rb. One list of published
//! secrets and one exposure rule, used by the commands and by `run`/`serve`'s start.
//!
//! `clear` never decrypts (EncryptedCredentials::Reset's contract): it takes no Cipher and writes raw columns only, so it
//! runs on an install whose secret has already changed. `owed` judges, on an in-memory copy, what the next start would
//! still owe once every key is gone: the reset refuses while anything is (D-RESET-REFUSES-OWED).
use crate::crypto::{Cipher, EncryptionKeys, PRIMARY_KEY_VAR, SALT_VAR};
use crate::enums::BOT_WORKING;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

// ---- SecretKeyBaseGuard ---------------------------------------------------------------------------------------------

/// SecretKeyBaseGuard::KNOWN_PLACEHOLDERS: matched exactly.
pub const KNOWN_PLACEHOLDERS: [&str; 6] = ["dev-secret-key-not-for-production", "your_secret_key_base_here", "placeholder", "-secret-key-base", "changeme", "secret"];
pub const MINIMUM_LENGTH: usize = 32;

pub fn published(secret: &str) -> bool { KNOWN_PLACEHOLDERS.contains(&secret) }
/// Ruby's String#length counts characters.
pub fn short(secret: &str) -> bool { secret.chars().count() < MINIMUM_LENGTH }

fn present(env: Env, name: &str) -> Option<String> { env(name).filter(|v| !v.trim().is_empty()) }

/// EncryptionKeys.independent?: both variables set.
pub fn independent(env: Env) -> bool { present(env, PRIMARY_KEY_VAR).is_some() && present(env, SALT_VAR).is_some() }

/// EncryptionKeys.derived_from_secret?: the configured pair is exactly the one this secret derives.
pub fn derived_from_secret(env: Env, secret: &str) -> bool {
    independent(env) && EncryptionKeys::resolve(&|_| None, secret).is_ok_and(|d| env(PRIMARY_KEY_VAR).as_deref() == Some(d.primary_key.as_str())
        && env(SALT_VAR).as_deref() == Some(d.key_derivation_salt.as_str()))
}

/// SecretKeyBaseGuard.data_exposed_by?
pub fn data_exposed_by(secret: &str, env: Env) -> bool { !independent(env) || derived_from_secret(env, secret) }

/// SecretKeyBaseGuard.warning_for: what `run` and `serve` print at start, never refusing.
pub fn warning_for(secret: &str, env: Env) -> Option<&'static str> {
    let exposed = data_exposed_by(secret, env);
    if published(secret) { return Some(if exposed { PUBLISHED_WARNING } else { PUBLISHED_SESSIONS_WARNING }) }
    if short(secret) { return Some(if exposed { SHORT_WARNING } else { SHORT_SESSIONS_WARNING }) }
    None
}

// Rails' four texts; the rake/compose/README lines rewritten for 3.0 (PLAN-A29 §7): the commands are `deltabadger
// encryption-*`, the compose file has one service, and the README section is the manual page.
pub const PUBLISHED_WARNING: &str = "\
================================ SECURITY WARNING ================================
SECRET_KEY_BASE is a value published in the public repository.
It is the encryption key for EVERYTHING encrypted in this instance: exchange API
keys, your two-factor secret, withdrawal addresses, and market-data configuration.
Anyone with a copy of this database can already read every credential in it. The
same value also signs and encrypts session cookies, so a reachable instance can be
signed into without ever copying the database. Deleting a credential here does NOT
invalidate it — anything held by an exchange or another service must be revoked
there. The exception is this instance's own REST API and MCP tokens: they exist
only in this database, so step 5 does invalidate those.

In this order:
  1. Stop the app.  docker compose stop
  2. Back up your data, now that nothing is writing to it. The default deployment
     uses a named volume, not a host directory:
       docker compose cp deltabadger:/app/storage ./storage-backup
     If you changed to a bind mount, copy that host directory instead. On a
     default install this copy includes /app/storage/.secrets — the key
     itself — so it decrypts itself. Keep it as carefully as the database.
     If your key comes from .env.docker or a compose file it is NOT in this
     copy, and step 6 overwrites it: save that value too, or this backup
     cannot be restored.
  3. List what is stored:
       docker compose run --rm --no-deps deltabadger encryption-report
     Copy your withdrawal addresses — they are not recoverable afterwards.
  4. REVOKE everything it listed, at its source: exchange API keys, fee keys, SMTP
     password, Alpaca key and secret, CoinGecko key, market-data token. Revoke
     only — replacements have nowhere to live until step 7.
  5. Clear what is stored:
       docker compose run --rm --no-deps deltabadger \\
         encryption-reset --confirm clear-credentials
     This also deletes your REST API token and every MCP client's token, which is
     what actually invalidates them: they are bearer tokens stored in the clear
     and derived from nothing, so changing the secret alone leaves them trading.
  6. Point this install at a new key. Blank the SECRET_KEY_BASE line in
     .env.docker, then delete the generated key, a separate file in the volume:
       docker compose run --rm --no-deps deltabadger \\
         rm -f /app/storage/.secrets
     On a default install .env.docker is already blank and .secrets is where
     your key actually is, so this deletion is what rotates anything at all.
     A new one is written only when that file is absent; an existing one is
     never overwritten.
     If you set ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY and
     ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT — anywhere: that file,
     .env.docker, or a compose file — remove both lines as well. They override
     the new key, so leaving them in place means the credentials you add at
     step 8 are encrypted under the same one you are replacing. No pair is
     written for you here, since that happens only for an install with no
     database, so once you are back up run
     `deltabadger encryption-derived-keys` and set what it prints BEFORE
     step 8. Skip that and this install goes back to deriving them from
     SECRET_KEY_BASE, with the same exposure the next time it changes.
     If your value comes from a compose file instead, put a freshly generated
     value there rather than blanking it:  openssl rand -hex 64
  7. Recreate the container so it picks up the edited value:
       docker compose up -d --force-recreate
     Starting the stopped container instead would bring it back with the old
     value still baked in: compose reads the environment when a container is
     created, not when it starts. A strong per-install secret is generated
     for you.
  8. Sign in, issue fresh credentials, add them, re-enable two-factor, re-issue
     your REST API token, reconnect your MCP clients, and restart your bots and
     rules — the reset cleared and stopped all of them.

Interactive Brokers is the slow one: its key pair is generated by this app and stored
here, so it can only be replaced after step 7 — run the connect wizard again, register
the new key in IBKR's portal, and wait for them to activate it.

Do NOT just change the value on its own — every encrypted field derives from it, so
that alone makes your two-factor secret unreadable and locks you out.
==================================================================================
";

pub const SHORT_WARNING: &str = "\
================================ SECURITY NOTICE =================================
SECRET_KEY_BASE is shorter than 32 characters.
It is the encryption key for everything encrypted in this instance — exchange API
keys, your two-factor secret, withdrawal addresses — and it also signs and encrypts
session cookies. A short key is cheap to brute-force offline from a single encrypted
value or one captured cookie, and recovering it yields both the stored credentials
and the ability to sign in as you.

Whether that is a live incident depends on where the value came from, which cannot
be told from the value itself. Only you know:
  * If a person chose it, or it has ever appeared anywhere public — a repository,
    an issue, a paste, a screenshot, a support thread — treat it as compromised.
    Follow the full procedure in the manual under \"Secrets and encryption keys\",
    including revoking every credential at its source, and do it now.
  * If it was randomly generated and has never left this machine, nothing here is
    known to be exposed. Move to a stronger secret at your convenience, using that
    same procedure.

Either way you cannot simply change the value: every encrypted field derives from it,
so changing it alone makes your two-factor secret unreadable and locks you out.
==================================================================================
";

pub const PUBLISHED_SESSIONS_WARNING: &str = "\
================================ SECURITY WARNING ================================
SECRET_KEY_BASE is a value published in the public repository.
This instance keeps its encryption keys separately, so what is stored here does not
derive from this value and is not readable from it. What this value does do is sign
and encrypt session cookies: anyone who can reach this instance can sign into it as
you, without needing the database.

Replace it. Because the encryption keys are kept separately, stored data is
unaffected:
  1. Revoke this instance's own REST API token and every connected MCP client, under
     Settings. They are bearer tokens that do not derive from this value, so
     replacing it does not end them, and anyone who signed in as you could have
     issued one.
  2. Put a fresh value in place of the published one, wherever this install takes it
     from — /app/storage/.secrets, .env.docker, or your compose file:
       openssl rand -hex 64
     Leave the two encryption key variables exactly as they are.
  3. Recreate the container:
       docker compose up -d --force-recreate
  4. Sign in again. Every session ends, which is the point. Any password reset or
     confirmation email already sent stops working — request a new one.
==================================================================================
";

pub const SHORT_SESSIONS_WARNING: &str = "\
================================ SECURITY NOTICE =================================
SECRET_KEY_BASE is shorter than 32 characters.
This instance keeps its encryption keys separately, so stored credentials do not
derive from this value. It still signs and encrypts session cookies, and a short one
is cheap to brute-force offline from a single captured cookie, which yields the
ability to sign in as you.

Replacing it leaves stored data untouched. Put a fresh value wherever this install
takes it from, leaving the encryption key variables as they are:
  openssl rand -hex 64
then `docker compose up -d --force-recreate` and sign in again. Any password reset
or confirmation email already sent stops working.

If this value has ever been anywhere public, also revoke this instance's REST API
token and its connected MCP clients: those do not derive from it and outlive the
replacement.
==================================================================================
";

/// EncryptionKeyCheck::WARNING, at start when some stored value was written under other keys (never refusing).
pub const KEY_NOTICE: &str = "\
============================== ENCRYPTION KEY NOTICE =============================
Some stored values cannot be read with the encryption keys this instance is
configured with. They were written under different ones.

Anything not yet saved over is intact, and a correct key still reads it. This
cannot tell which of these values have already been written since — saving over one
replaces it for good — so restore the right key before changing any credential here,
and before leaving this instance running.

The usual causes:
  * ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY or
    ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT set to different values
    than the values that wrote this data. Restore them and start again.
  * SECRET_KEY_BASE replaced on an instance that derives its encryption keys from
    it. Restore the previous value and start again.
  * More than one container on this volume running with different values. They all
    need the same ones.

To see what is stored:
  docker compose run --rm --no-deps deltabadger encryption-report

If the previous values are gone for good, the credentials cannot be recovered and
must be replaced at their source. See the manual page \"Secrets and encryption keys\".
==================================================================================
";

// ---- what is stored -------------------------------------------------------------------------------------------------

/// Every encrypted column (EncryptedCredentials::COVERAGE's 13, then Rust's own, PLAN-A29 §4 and r2.1). A Rust table an
/// install has not migrated to is skipped.
pub const COVERAGE: &[(&str, &str)] = &[
    ("api_keys", "key"), ("api_keys", "secret"), ("api_keys", "passphrase"), ("api_keys", "access_token"),
    ("api_keys", "rsa_signature_key"), ("api_keys", "rsa_encryption_key"), ("api_keys", "dh_param"),
    ("fee_api_keys", "key"), ("fee_api_keys", "secret"), ("fee_api_keys", "passphrase"),
    ("app_configs", "value"), ("users", "otp_secret_key"), ("rules", "address"),
    ("exchange_accounts", "account_id_enc"), ("exchange_account_credentials", "account_id_enc"),
    ("exchange_account_imports", "account_id_enc"), ("ibkr_bindings", "account_id_enc"),
    ("ibkr_bot_accounts", "account_id_enc"), ("rust_withdrawal_intents", "params_enc"),
];

fn table(c: &Connection, name: &str) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [name], |r| r.get(0))
}

/// Report#data_written_under_another_key?: some stored value shaped as an encrypted message that this cipher does not
/// open (unreadable or malformed alike: over-warning is the right way round). Short-circuits.
pub fn written_under_another_key(c: &Connection, cipher: &Cipher) -> rusqlite::Result<bool> {
    for (t, col) in COVERAGE {
        if !table(c, t)? { continue; }
        let mut s = c.prepare(&format!("SELECT CAST({col} AS TEXT) FROM {t} WHERE {col} IS NOT NULL"))?;
        let mut rows = s.query([])?;
        while let Some(row) = rows.next()? {
            let v: String = row.get(0)?;
            if crate::crypto::is_envelope(&v) && cipher.decrypt(&v).is_err() { return Ok(true); }
        }
    }
    Ok(false)
}

/// The keys app/models/app_config.rb names (its setting constants; `proxy_` is a prefix): a plain legacy value under one
/// of these is a setting the reset clears too.
pub const RAILS_SETTINGS: &[&str] = &["coingecko_api_key", "setup_sync_status", "market_data_provider", "market_data_url", "market_data_token",
    "platform_connected_at", "registration_open", "alpaca_api_key", "alpaca_api_secret", "alpaca_mode", "smtp_provider", "smtp_username",
    "smtp_password", "smtp_host", "smtp_port"];
const PROXY_PREFIX: &str = "proxy_";

/// The app_configs rows the reset deletes (D-RESET-KEEPS-ENGINE-STATE): every encrypted row, and every Rails setting.
/// The plain rows Rust keeps its own state in stay; the lease is rewritten instead (D-RESET-LEASE-PLAIN). Sorted.
fn settings_reset(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut s = c.prepare("SELECT key, value FROM app_configs ORDER BY key")?;
    let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter(|(k, v)| k != crate::lease::KEY
        && (v.as_deref().is_some_and(crate::crypto::is_envelope) || RAILS_SETTINGS.contains(&k.as_str()) || k.starts_with(PROXY_PREFIX)))
        .map(|(k, _)| k).collect())
}

const KEY_TYPES: [&str; 3] = ["trading", "withdrawal", "read_only"];
const UNREADABLE: &str = "<unreadable>";

/// EncryptedCredentials::Report#call.
pub struct Inventory {
    /// (exchange, key type, user email)
    pub api_keys: Vec<(String, String, String)>,
    pub fee_api_keys: Vec<String>,
    pub two_factor_users: Vec<String>,
    pub app_config_keys: Vec<String>,
    /// (exchange, asset, address)
    pub withdrawal_addresses: Vec<(String, String, String)>,
}

pub fn inventory(c: &Connection, cipher: &Cipher) -> rusqlite::Result<Inventory> {
    let mut s = c.prepare("SELECT e.name, k.key_type, u.email FROM api_keys k LEFT JOIN exchanges e ON e.id = k.exchange_id LEFT JOIN users u ON u.id = k.user_id")?;
    let mut api_keys = s.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(),
        r.get::<_, Option<i64>>(1)?.and_then(|t| KEY_TYPES.get(t as usize)).map(|t| t.to_string()).unwrap_or_default(),
        r.get::<_, Option<String>>(2)?.unwrap_or_default())))?.collect::<Result<Vec<_>, _>>()?;
    api_keys.sort();
    let strings = |sql: &str| -> rusqlite::Result<Vec<String>> {
        let mut s = c.prepare(sql)?;
        let mut v = s.query_map([], |r| Ok(r.get::<_, Option<String>>(0)?.unwrap_or_default()))?.collect::<Result<Vec<_>, _>>()?;
        v.sort();
        Ok(v)
    };
    let mut s = c.prepare("SELECT e.name, a.symbol, r.address FROM rules r LEFT JOIN exchanges e ON e.id = r.exchange_id LEFT JOIN assets a ON a.id = r.asset_id \
                           WHERE r.type = 'Rules::Withdrawal' ORDER BY r.id")?;
    let withdrawal_addresses = s.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        r.get::<_, Option<String>>(2)?.map(|a| cipher.decrypt(&a).unwrap_or_else(|_| UNREADABLE.into())).unwrap_or_default())))?.collect::<Result<Vec<_>, _>>()?;
    Ok(Inventory {
        api_keys,
        fee_api_keys: strings("SELECT e.name FROM fee_api_keys f LEFT JOIN exchanges e ON e.id = f.exchange_id")?,
        two_factor_users: strings(TWO_FACTOR_USERS)?,
        app_config_keys: settings_reset(c)?,
        withdrawal_addresses,
    })
}

/// User.with_two_factor_material (otp_module 0 = disabled).
const TWO_FACTOR_USERS: &str = "SELECT email FROM users WHERE otp_secret_key IS NOT NULL OR otp_module <> 0";

fn list(v: &[String]) -> String { if v.is_empty() { "(none)".into() } else { v.join(", ") } }

/// The `assume_compromised` lambda: the flag, a published current secret, or anything stored under another key.
pub fn compromised(flag: bool, secret: &str, c: &Connection, cipher: &Cipher) -> rusqlite::Result<bool> {
    Ok(flag || published(secret) || written_under_another_key(c, cipher)?)
}

/// encryption-report's text (Rails' report task; R1: `PUBLISHED=yes` → `--published`; R3: the owed list, when given).
pub fn report_text(inv: &Inventory, compromised: bool, owed: Option<&[String]>) -> String {
    let mut o = String::new();
    let mut p = |s: &str| { o.push_str(s); o.push('\n'); };
    if compromised {
        p("\nAPI keys — REVOKE each of these at the exchange. Deleting them here does not");
        p("invalidate them; anyone holding a copy of this database can still trade with them.");
    } else {
        p("\nAPI keys stored here — these will be cleared because the encryption key is");
        p("changing. Every stored value still decrypts under the current secret, and that");
        p("secret is not one published in this repository, so nothing below is known to be");
        p("exposed. If you know otherwise, re-run with --published.");
    }
    if inv.api_keys.is_empty() { p("  (none)"); }
    for (exchange, kind, user) in &inv.api_keys { p(&format!("  {exchange} ({kind}) — {user}")); }
    p(&format!("\nFee API keys — {}: {}", if compromised { "revoke these too" } else { "will also be cleared" }, list(&inv.fee_api_keys)));
    p(&format!("Two-factor seed present for: {}", list(&inv.two_factor_users)));
    p(&format!("Settings that will reset:    {}", list(&inv.app_config_keys)));
    if compromised {
        p("Every credential stored in those settings must be rotated at its source as well —");
        p("SMTP password, Alpaca key and secret, CoinGecko key, market-data token.");
    } else {
        p("Those settings hold credentials too — they will be cleared and need re-entering,");
        p("not rotated: SMTP password, Alpaca key and secret, CoinGecko key, market-data token.");
    }
    p("\nWithdrawal addresses — COPY THESE NOW, they cannot be recovered afterwards:");
    if inv.withdrawal_addresses.is_empty() { p("  (none)"); }
    for (exchange, asset, address) in &inv.withdrawal_addresses { p(&format!("  {exchange} / {asset}: {address}")); }
    match owed {
        Some([]) => {}
        Some(lines) => {
            p("\nOwed — the reset refuses until each of these is settled:");
            for l in lines { p(&format!("  {l}")); }
        }
        None => p(&format!("\n{OWED_NOT_CHECKED}")),
    }
    o.push('\n');
    o
}

/// R3, when the app holds the install: the owed work is judged on a copy only the stopped app's lock makes consistent.
pub const OWED_NOT_CHECKED: &str = "Owed work wasn't checked because the app is running. The reset checks it, with the app stopped.";

// ---- the reset ------------------------------------------------------------------------------------------------------

pub const CONFIRMATION: &str = "clear-credentials";

/// Step 4 of the refusal without confirmation (Rails' revoke_step).
fn revoke_step(compromised: bool) -> &'static str {
    if compromised {
        "REVOKE every credential it listed, at its source. Do not create\n     the replacements yet — there is nowhere to store them until\n     the app is running again."
    } else {
        "Nothing here is known to be exposed — this clears credentials\n     because the encryption key is changing, not because anything\n     leaked."
    }
}

/// Where a platform's data is (S2).
fn storage(platform: Platform) -> &'static str {
    match platform { Platform::Docker => "/app/storage", Platform::Umbrel => "the app's data/storage folder", Platform::Desktop => "the app's data folder" }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform { Docker, Umbrel, Desktop }

/// DELTABADGER_PLATFORM; unset or anything else is Docker, as Rails' text is.
pub fn platform(env: Env) -> Platform {
    match crate::jobs::app_update::platform(env) {
        crate::jobs::app_update::Platform::Umbrel => Platform::Umbrel,
        crate::jobs::app_update::Platform::Desktop => Platform::Desktop,
        _ => Platform::Docker,
    }
}

/// The refusal without `--confirm clear-credentials` (Rails' first abort; S1–S5 rewritten).
pub fn unconfirmed_text(compromised: bool, platform: Platform) -> String {
    format!("
This deletes every stored API key, every REST API and MCP token, disables
two-factor authentication, clears withdrawal addresses, resets settings, and stops
every working bot and rule. It cannot be undone.

Do these first, in order:
  1. Stop the app. This refuses to run while the app is running: the app holds
     the install while it runs.
  2. Back up {} — with the app stopped, so the copy is consistent.
  3. Run `deltabadger encryption-report`. Copy your withdrawal addresses.
  4. {}

Exposure is judged from two things: whether the secret you are running now is one
published in this repository, and whether anything stored still decrypts under it.
Neither can see a key you have already replaced and discarded. If you know the key
this data was written under was published, re-run with --published as well and
every message here will say so.

Re-run with:  deltabadger encryption-reset --confirm {CONFIRMATION}

", storage(platform), revoke_step(compromised))
}

/// Rails' IBKR abort (S6: the flag, and the compose -e sentence gone with the env switch).
pub const IBKR_TEXT: &str = "
This instance holds Interactive Brokers credentials. Unlike an exchange API key,
IBKR's RSA material is generated by this app and stored only here, so it cannot
be re-entered and cannot be replaced in advance. After this you must run the
connect wizard again to generate a fresh key pair, register it in IBKR's portal,
and wait for IBKR to activate it — which has taken days. Everything else here
takes minutes to restore.

If you accept that, re-run with --confirm-ibkr as well.

";

/// The owed refusal (S7, D-RESET-REFUSES-OWED).
pub fn owed_text(lines: &[String], readable: bool) -> String {
    let mut o = String::from("\nNothing was cleared. These still have to be settled first:\n");
    for l in lines { o.push_str(&format!("  {l}\n")); }
    o.push('\n');
    o.push_str(if readable {
        "Settle each of these in the app first, while this install's secret still reads its\n\
         data: cancel the open orders or let them fill, resolve the placements and withdrawals\n\
         with the commands named, let the swaps finish, and answer what the bots' pages ask.\n\
         Then stop the app and run this again.\n\n"
    } else {
        "This install's data can't be read with its current secret, so these can't be settled\n\
         here. Contact support before clearing anything.\n\n"
    });
    o
}

/// What the reset did (Rails' Summary, plus the bots whose rebalancing was turned off: D-RESET-REBALANCE-OFF).
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub bots_stopped: usize, pub rules_stopped: usize, pub withdrawal_addresses_cleared: usize, pub api_keys_deleted: usize,
    pub fee_api_keys_deleted: usize, pub app_configs_deleted: usize, pub two_factor_disabled: usize, pub oauth_tokens_deleted: usize,
    pub ibkr_credentials_destroyed: bool, pub rebalancing_off: Vec<String>,
}

const IBKR_KEYS: &str = "SELECT EXISTS (SELECT 1 FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE e.type = 'Exchanges::Ibkr')";

pub fn holds_ibkr(c: &Connection) -> rusqlite::Result<bool> { c.query_row(IBKR_KEYS, [], |r| r.get(0)) }

/// EncryptedCredentials::Reset#call, inside the caller's transaction. Takes no Cipher: nothing is decrypted, every write
/// is to a raw column. Rust's own rows go with it (D-RESET-RUST-ROWS, D-RESET-IBKR-BOT-ACCOUNT): the account identities
/// and IBKR bindings, readable only under the key that goes, and the withdrawals never sent. A Rust table the install has
/// not migrated to is skipped.
pub fn clear(tx: &Connection, now: DateTime<Utc>) -> rusqlite::Result<Summary> {
    let at = crate::codec::format_time(now);
    let ibkr = holds_ibkr(tx)?;
    // Bots: the web key-delete's stop with its activity line (D-RESET-STOP-LOGGED); stop_message_key untouched (Rails).
    let working = BOT_WORKING.iter().map(|s| (*s as i64).to_string()).collect::<Vec<_>>().join(",");
    let mut s = tx.prepare(&format!("SELECT id FROM bots WHERE status IN ({working}) ORDER BY id"))?;
    let bots = s.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for id in &bots {
        tx.execute("UPDATE bots SET status = 2, stopped_at = ?1, updated_at = ?1 WHERE id = ?2", params![at, id])?;
        tx.execute("INSERT INTO bot_activity_logs (bot_id, event, level, message, details, created_at) VALUES (?1, 'stopped', 0, NULL, '{}', ?2)", params![id, at])?;
    }
    // Rebalancing runs on stopped bots too (rebalance::candidate): off, so a replacement key resumes nothing by itself.
    let mut rebalancing_off = vec![];
    let mut s = tx.prepare("SELECT id, settings FROM bots ORDER BY id")?;
    let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (id, settings) in rows {
        let on = settings.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok())
            .is_some_and(|v| crate::web::bot::cast_boolean(v.get("rebalance_enabled")));
        if on {
            tx.execute("UPDATE bots SET settings = json_set(settings, '$.rebalance_enabled', json('false')) WHERE id = ?1", [id])?;
            rebalancing_off.push(name(tx, id)?);
        }
    }
    let rules_stopped = tx.execute(&format!("UPDATE rules SET status = 2, updated_at = ?1 WHERE status IN ({working})"), [&at])?;
    let withdrawal_addresses_cleared = tx.execute("UPDATE rules SET address = NULL WHERE type = 'Rules::Withdrawal' AND address IS NOT NULL", [])?;
    tx.execute("UPDATE account_transactions SET api_key_id = NULL WHERE api_key_id IS NOT NULL", [])?;
    if table(tx, "ibkr_reservations")? { tx.execute("DELETE FROM ibkr_reservations", [])?; }
    let api_keys_deleted = tx.execute("DELETE FROM api_keys", [])?;
    let fee_api_keys_deleted = tx.execute("DELETE FROM fee_api_keys", [])?;
    let mut app_configs_deleted = 0;
    for key in settings_reset(tx)? { app_configs_deleted += tx.execute("DELETE FROM app_configs WHERE key = ?1", [key])?; }
    rewrite_lease(tx, now)?;
    let two_factor_disabled = tx.execute("UPDATE users SET otp_secret_key = NULL, otp_module = 0 WHERE otp_secret_key IS NOT NULL OR otp_module <> 0", [])?;
    let oauth_tokens_deleted = tx.execute("DELETE FROM oauth_access_tokens", [])? + tx.execute("DELETE FROM oauth_access_grants", [])?;
    tx.execute("DELETE FROM connected_clients", [])?;
    if table(tx, "exchange_accounts")? {
        tx.execute("UPDATE exchange_accounts SET account_id_enc = NULL, fingerprint = NULL WHERE account_id_enc IS NOT NULL OR fingerprint IS NOT NULL", [])?;
        tx.execute("DELETE FROM exchange_account_credentials", [])?;
        tx.execute("DELETE FROM exchange_account_imports", [])?;
    }
    if table(tx, "ibkr_bindings")? { tx.execute("DELETE FROM ibkr_bindings", [])?; tx.execute("DELETE FROM ibkr_bot_accounts", [])?; }
    // Never sent (dispatched is written before the request): the engine drops these itself. The rest are owed.
    if table(tx, "rust_withdrawal_intents")? { tx.execute("DELETE FROM rust_withdrawal_intents WHERE state = 'prepared'", [])?; }
    Ok(Summary { bots_stopped: bots.len(), rules_stopped, withdrawal_addresses_cleared, api_keys_deleted, fee_api_keys_deleted, app_configs_deleted,
                 two_factor_disabled, oauth_tokens_deleted, ibkr_credentials_destroyed: ibkr, rebalancing_off })
}

/// D-RESET-LEASE-PLAIN: the encrypted lease is unreadable under the new key, so it is replaced by the same state in
/// plain JSON (a plain value reads as itself), chosen from the plain markers. Absent, nothing is written.
fn rewrite_lease(tx: &Connection, now: DateTime<Utc>) -> rusqlite::Result<()> {
    use crate::lease::{plain_at, HANDED_BACK_AT, KEY, TAKEN_OVER_AT};
    let exists: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM app_configs WHERE key = ?1)", [KEY], |r| r.get(0))?;
    if !exists { return Ok(()); }
    let at = |k| plain_at(tx, k).ok().flatten();
    let value = match (at(TAKEN_OVER_AT), at(HANDED_BACK_AT)) {
        (Some(taken), back) if back.is_none_or(|b| b < taken) => json!({ "engine": "rust", "version": crate::VERSION, "since": taken.to_rfc3339() }),
        (_, Some(back)) => json!({ "engine": "none", "released_by": "rust", "handed_back": true, "at": back.to_rfc3339() }),
        _ => { tx.execute("DELETE FROM app_configs WHERE key = ?1", [KEY])?; return Ok(()); }
    };
    tx.execute("UPDATE app_configs SET value = ?2, updated_at = ?3 WHERE key = ?1", params![KEY, value.to_string(), crate::codec::format_time(now)])?;
    Ok(())
}

fn name(c: &Connection, id: i64) -> rusqlite::Result<String> {
    let label: Option<String> = c.query_row("SELECT label FROM bots WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
    Ok(match label.filter(|l| !l.trim().is_empty() && l.trim() != format!("Bot {id}")) { Some(l) => format!("Bot {id} (“{}”)", l.trim()), None => format!("Bot {id}") })
}

/// The reset's text after the clear (Rails' summary and Next block; S8/S9 and the platform blocks, PLAN-A29 §6).
pub fn cleared_text(s: &Summary, compromised: bool, independent: bool, platform: Platform) -> String {
    let mut o = String::new();
    let mut p = |s: &str| { o.push_str(s); o.push('\n'); };
    p("\nCleared:");
    p(&format!("  API keys deleted:            {}", s.api_keys_deleted));
    p(&format!("  Fee API keys deleted:        {}", s.fee_api_keys_deleted));
    p(&format!("  Settings cleared:            {}", s.app_configs_deleted));
    p(&format!("  Withdrawal addresses:        {}", s.withdrawal_addresses_cleared));
    p(&format!("  Two-factor seed cleared for: {} user(s)", s.two_factor_disabled));
    p(&format!("  REST API and MCP tokens:     {}", s.oauth_tokens_deleted));
    p(&format!("  Bots stopped:                {}", s.bots_stopped));
    p(&format!("  Rules stopped:               {}", s.rules_stopped));
    if compromised {
        p("\nThose credentials should ALREADY have been REVOKED at their source before this");
        p("ran — deleting them here never invalidated them, and a copy of the old database");
        p("still contains them. If you have not done that yet, do it now.");
    } else {
        p("\nNothing here was known to be exposed, so nothing needed revoking first — this");
        p("only cleared credentials because the encryption key is changing.");
    }
    if !s.rebalancing_off.is_empty() {
        p(&format!("\nRebalancing was turned off on: {}.", s.rebalancing_off.join(", ")));
        p("Turn it back on in each bot's settings when you want it.");
    }
    match platform {
        Platform::Docker => {
            p("\nNext: point this install at a new key. Blank the SECRET_KEY_BASE value in");
            p(".env.docker and delete the generated key, which is a separate file:");
            p("  docker compose run --rm --no-deps deltabadger rm -f /app/storage/.secrets");
            p("On a default install that file is where your key actually is, and a new one is");
            p("written only when it is absent. If your value comes from a compose file instead,");
            p("put a freshly generated one there (openssl rand -hex 64) rather than blanking it.");
            p("Then recreate the container, which is what picks up the change:");
            p("  docker compose up -d --force-recreate");
            p("Starting the stopped container instead brings the old value back with it.");
            if independent { independent_block(&mut p); }
        }
        Platform::Umbrel => {
            p("\nNext: give this install encryption keys of its own. On Umbrel, SECRET_KEY_BASE");
            p("comes from the device's app seed and can't be changed from the app, so the stored");
            p("data moves to a pair of its own instead. Generate two values:");
            p("  openssl rand -hex 32");
            p("  openssl rand -hex 32");
            p("and put them in the app's data/storage/.secrets as these two lines, replacing any");
            p("that are already there:");
            p(&format!("  {PRIMARY_KEY_VAR}=<the first>"));
            p(&format!("  {SALT_VAR}=<the second>"));
            p("Then start the app again from the Umbrel dashboard. Nothing encrypted is left, so a");
            p("new pair loses nothing.");
            p("\nThe app seed still signs sign-in sessions. If it may have been exposed, anyone");
            p("holding it can sign in as you until it changes, and only the Umbrel device can");
            p("change it. Every credential and every REST API and MCP token stored here was");
            p("deleted all the same.");
        }
        Platform::Desktop => {
            p("\nNext: point this install at a new key. Quit the app and make sure its engine has");
            p("closed, then delete .secrets from the app's data folder. The app writes a new");
            p("SECRET_KEY_BASE when that file is absent, and with it no encryption keys of its");
            p("own: that happens only for an install with no database. So once it is back up on");
            p("the new secret, quit it again, run");
            p("  deltabadger encryption-derived-keys");
            p("the same way you ran this, add the two lines it prints to .secrets, and start the");
            p("app again, BEFORE entering the replacement credentials. Otherwise this install");
            p("goes back to deriving its encryption keys from SECRET_KEY_BASE.");
        }
    }
    p("\nIssue the replacement credentials, re-enable two-factor, re-issue your REST");
    p("API token, reconnect your MCP clients, and restart your bots and rules — until");
    p("the app is running there is nowhere to store the new credentials.\n");
    if s.ibkr_credentials_destroyed {
        p("Interactive Brokers: once the app is back up, run the connect wizard again to");
        p("generate a fresh key pair, register it in their portal, and wait for IBKR to");
        p("activate it.\n");
    }
    o
}

fn independent_block(p: &mut impl FnMut(&str)) {
    p("\nThis install also sets its own encryption keys. Remove both lines wherever");
    p(&format!("they are set — {PRIMARY_KEY_VAR} and"));
    p(&format!("{SALT_VAR} — or the credentials you enter next"));
    p("are encrypted under the same key you are replacing.");
    p("No replacement pair is created for you: that happens only for an install with no");
    p("database, and yours has one. Once it is back up on the new secret, run");
    p("  deltabadger encryption-derived-keys");
    p("and set what it prints, BEFORE entering the replacement credentials. Otherwise");
    p("this install goes back to deriving its encryption keys from SECRET_KEY_BASE,");
    p("and the next time that changes you lose the new credentials the same way.");
}

// ---- owed work (D-RESET-REFUSES-OWED, PLAN-A29 r4–r6) ---------------------------------------------------------------

/// What the next start would still owe once every key is gone, judged on `copy` (store::copy_in_memory, which it
/// changes): this build's migrations applied, the clear run, then the takeover itself under a throwaway key, with its
/// lines muted. Its refusals are owed, and so is whatever it leaves that trades or waits on a venue once a key is back.
/// Never decrypts anything stored: the throwaway key reads nothing but the plain lease `clear` wrote.
pub fn owed(lock: &crate::lease::EngineLock, copy: &crate::store::Opened, now: DateTime<Utc>) -> Result<Vec<String>, String> {
    let e = |e: rusqlite::Error| e.to_string();
    crate::store::migrate_copy(copy).map_err(|e| format!("{e:?}"))?;
    let tx = rusqlite::Transaction::new_unchecked(&copy.primary, rusqlite::TransactionBehavior::Immediate).map_err(e)?;
    clear(&tx, now).map_err(e)?;
    tx.commit().map_err(e)?;
    let throwaway = Cipher::new(&EncryptionKeys { primary_key: hex::encode(rand::random::<[u8; 32]>()), key_derivation_salt: hex::encode(rand::random::<[u8; 32]>()) });
    let mut lines = vec![];
    match crate::logger::muted(|| crate::engine::handover::take_over(lock, copy, &throwaway, crate::VERSION, now)) {
        Ok(_) => {}
        Err(crate::engine::EngineError::HardStop(m)) => lines.extend(m.lines().filter(|l| !l.trim().is_empty()).map(str::to_string)),
        Err(crate::engine::EngineError::Ineligible(p)) => lines.extend(p),
        Err(other) => lines.push(format!("This install couldn't be checked: {other:?}")),
    }
    lines.extend(left_over(&copy.primary, now).map_err(e)?);
    let mut seen = std::collections::HashSet::new();
    lines.retain(|l| seen.insert(l.clone()));
    Ok(lines)
}

/// What the takeover lets through but a venue key is still needed for (Codex plan r3–r5).
fn left_over(c: &Connection, now: DateTime<Utc>) -> rusqlite::Result<Vec<String>> {
    use crate::engine::{liquidation, redeploy, signal};
    let mut lines = vec![];
    let mut s = c.prepare("SELECT id, transient_data FROM bots ORDER BY id")?;
    let bots = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (id, transient) in bots {
        let t: Value = transient.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
        let n = name(c, id)?;
        if crate::engine::rebalance::is_pending(&t) { lines.push(format!("{n}: a rebalance is part-way through.")); }
        if t.get(signal::MARKER_KEY).is_some_and(Value::is_object) || t.get("rust_placement").is_some_and(|v| !v.is_null()) {
            lines.push(format!("{n}: it has an order whose result isn't known yet."));
        }
        // The request run_commands takes, and what composition counts as in flight (a marker, a waiting or an abandoned
        // unresolved order): takeover admits these on a stopped bot (Codex branch review r1).
        let bot = crate::engine::model::load_bot(c, id).ok();
        let in_flight = |f: &dyn Fn(&crate::engine::model::Bot) -> Result<bool, crate::engine::EngineError>| bot.as_ref().is_none_or(|b| f(b).unwrap_or(true));
        let liquidating: bool = c.query_row(&format!("SELECT coalesce({}, 0) FROM bots WHERE id = ?1", liquidation::PENDING_SQL), [id], |r| r.get(0))?
            || in_flight(&|b| crate::engine::composition::liquidation_in_flight(c, b, now));
        if liquidating { lines.push(format!("{n}: a sale of its holdings hasn't finished.")); }
        let redeploying: bool = c.query_row(&format!("SELECT coalesce({}, 0) FROM bots WHERE id = ?1", redeploy::PENDING_SQL), [id], |r| r.get(0))?
            || in_flight(&|b| crate::engine::composition::redeploy_in_flight(c, b));
        if redeploying { lines.push(format!("{n}: a redeploy hasn't finished.")); }
    }
    let mut s = c.prepare("SELECT DISTINCT bot_id FROM transactions WHERE status = 0 AND external_status IN (0, 1) AND bot_id IS NOT NULL ORDER BY 1")?;
    for id in s.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()? { lines.push(format!("{}: it has open orders.", name(c, id)?)); }
    drop(s);
    if table(c, "rust_api_orders")? {
        let mut s = c.prepare("SELECT id FROM rust_api_orders WHERE state IN ('queued', 'taken', 'sending') ORDER BY id")?;
        for id in s.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()? {
            lines.push(format!("An order sent through the REST API or MCP (request {id}) hasn't finished: start the app once so it settles it."));
        }
    }
    if table(c, "rust_withdrawal_intents")? {
        let mut s = c.prepare("SELECT owner FROM rust_withdrawal_intents WHERE state IN ('dispatched', 'unresolved') ORDER BY owner")?;
        for owner in s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()? {
            let rule = owner.strip_prefix("rule:").unwrap_or(&owner).to_string();
            lines.push(format!("Rule {rule}: a withdrawal may have been sent and its result isn't recorded; resolve it with resolve-withdrawal {rule}."));
        }
    }
    if table(c, "rust_rails_unrecorded")? {
        // Owed until reported, or while an answer is asked for (offers, no resolution in its group); reported with
        // nothing to answer is done (A28-1's report-only outcome).
        let mut s = c.prepare("SELECT o.id, o.bot_id, o.reported_at IS NULL, EXISTS (SELECT 1 FROM rust_rails_unrecorded_offers f WHERE f.obligation_id = o.id) \
                               FROM rust_rails_unrecorded o ORDER BY o.id")?;
        let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, bool>(2)?, r.get::<_, bool>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
        drop(s);
        for (id, bot, unreported, offered) in rows {
            let n = name(c, bot)?;
            if unreported { lines.push(format!("{n}: an order your previous version may have placed hasn't been checked yet: start the app once so it checks.")); }
            else if offered && crate::engine::rails_unrecorded::decision(c, id).map_err(|e| rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(format!("{e:?}")))))?.is_none() {
                lines.push(format!("{n}: its page asks whether an order your previous version may have placed is yours."));
            }
        }
    }
    Ok(lines)
}

// ---- derived keys ---------------------------------------------------------------------------------------------------

/// The derived-keys task's three answers (D1: the report command; D2: where to put them, per platform).
pub fn derived_keys_text(readable: Option<bool>, env: Env, secret: &str, platform: Platform) -> String {
    if readable != Some(true) {
        return "
Whether this instance can read what it has stored could not be confirmed, so the
values this command derives may not be the ones that encrypted your data. They are
not printed: setting the wrong pair would replace a recoverable state with an
unrecoverable one.

Restore the SECRET_KEY_BASE, or the encryption keys, that this data was written
under, then run this again.

To see what is stored:  deltabadger encryption-report
".into();
    }
    if independent(env) {
        return "
This install already keeps its encryption keys separate from SECRET_KEY_BASE. There
is nothing to do: SECRET_KEY_BASE can be replaced without affecting stored data.

The keys in use are not printed here, and the values this command would otherwise
derive are NOT the ones encrypting your data — putting those in would make all of it
unreadable.
".into();
    }
    let d = EncryptionKeys::resolve(&|_| None, secret).expect("no variables: derived");
    let put = match platform {
        Platform::Docker => "Put them where this install's environment comes from — /app/storage/.secrets, or
.env.docker, or the compose file.

Then recreate the container so the values are picked up:
  docker compose up -d --force-recreate",
        Platform::Umbrel => "Add both lines to the app's data/storage/.secrets, then restart the app from the
Umbrel dashboard.",
        Platform::Desktop => "Add both lines to .secrets in the app's data folder, then quit the app and start it
again.",
    };
    format!("
These are the ActiveRecord Encryption keys this install currently derives from its
SECRET_KEY_BASE. Setting them explicitly changes nothing on its own — they are the
values already in use — but once they are set, SECRET_KEY_BASE no longer determines
them and can be replaced without making stored data unreadable.

  {PRIMARY_KEY_VAR}={}
  {SALT_VAR}={}

Set BOTH. A key used against a different salt reads nothing this install has stored,
and the app refuses to start with only one of them.

{put}

Nothing has been written. Copy the two lines above yourself.
", d.primary_key, d.key_derivation_salt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_reads_as_rails_does() {
        let none = |_: &str| None;
        assert!(published("changeme") && !published("changeme "));
        assert!(short(&"x".repeat(31)) && !short(&"x".repeat(32)));
        assert_eq!(warning_for("dev-secret-key-not-for-production", &none), Some(PUBLISHED_WARNING));
        assert_eq!(warning_for("short", &none), Some(SHORT_WARNING));
        assert_eq!(warning_for(&"x".repeat(64), &none), None);
        let own = |k: &str| (k == PRIMARY_KEY_VAR || k == SALT_VAR).then(|| "own".to_string());
        assert_eq!(warning_for("secret", &own), Some(PUBLISHED_SESSIONS_WARNING));
        assert_eq!(warning_for("short", &own), Some(SHORT_SESSIONS_WARNING));
        let d = EncryptionKeys::resolve(&none, "secret").unwrap();
        let derived = |k: &str| match k { PRIMARY_KEY_VAR => Some(d.primary_key.clone()), SALT_VAR => Some(d.key_derivation_salt.clone()), _ => None };
        assert_eq!(warning_for("secret", &derived), Some(PUBLISHED_WARNING), "a pair derived from the secret is as exposed as it");
    }
}
