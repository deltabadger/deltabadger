//! IBKR accounts (Ibkr plan I2, D-IBKR-ACCOUNT). One IBKR login can hold several brokerage accounts, and the account
//! behind a login can change without its credentials changing. 2.x reads whichever account the login lists first; here:
//! - a login that lists more than one account is refused (incorrect);
//! - a key's first correct check binds it to the one account it names (`ibkr_bindings`), never updated; a later check
//!   naming another account is refused and writes nothing; deleting the key deletes its binding;
//! - an IBKR bot belongs to one account (`ibkr_bot_accounts`), set once: at creation when its key is bound, else at its
//!   first successful Start. `check_bot_account` refuses a bot whose key is now bound to another account; Start calls it,
//!   and the placement, recovery, cancel and liquidation paths of IBKR's trading slice call it before they reach IBKR.
//!
//! The account id is kept encrypted; `account_ref` (a prefix of its HMAC under the install's identity key) is what names
//! the account wherever per-account rows are namespaced.
use crate::codec::format_time;
use crate::crypto::Cipher;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// A login that lists more than one account (D-IBKR-ACCOUNT).
pub const MULTI_ACCOUNT: &str = "this IBKR login holds more than one account; Deltabadger works with a login that holds one account";

/// An IBKR account id as `/iserver/accounts` names it: a live (U…), paper (DU…) or demo (DF…) account.
pub fn parse(raw: &str) -> Option<&str> {
    let digits = raw.strip_prefix("DU").or_else(|| raw.strip_prefix("DF")).or_else(|| raw.strip_prefix('U'))?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(raw)
}

/// Exchanges::Ibkr#extract_accounts: `selectedAccount` first, then `accounts` (or the answer itself when it is not an
/// object), nulls dropped, each value once.
pub fn listed(data: &Value) -> Vec<Value> {
    let (selected, accounts) = match data {
        Value::Null => return vec![],
        Value::Object(o) => (o.get("selectedAccount").cloned().unwrap_or(Value::Null), match o.get("accounts") {
            None | Some(Value::Null) => vec![], Some(Value::Array(a)) => a.clone(), Some(v) => vec![v.clone()] }),
        Value::Array(a) => (Value::Null, a.clone()),
        v => (Value::Null, vec![v.clone()]),
    };
    let mut out: Vec<Value> = vec![];
    for v in std::iter::once(selected).chain(accounts) { if !v.is_null() && !out.contains(&v) { out.push(v); } }
    out
}

/// The account a key is bound to.
pub struct Binding { pub account_ref: String, account_id_enc: String }
impl Binding {
    /// The bound account id, when it can be read.
    pub fn account_id(&self, cipher: &Cipher) -> Option<String> { cipher.decrypt(&self.account_id_enc).ok() }
}

pub fn binding(c: &Connection, api_key_id: i64) -> rusqlite::Result<Option<Binding>> {
    c.query_row("SELECT account_ref, account_id_enc FROM ibkr_bindings WHERE api_key_id = ?1", [api_key_id],
                |r| Ok(Binding { account_ref: r.get(0)?, account_id_enc: r.get(1)? })).optional()
}

/// #710 7b rule 8: `bind`'s refusal of a first bind whose account another account holds (a key of an account the login
/// does not open, or one that holds that account's history): the key stays unbound and inert.
pub const OTHER_ACCOUNT: &str = "this IBKR login names another account than the one this connection belongs to";

/// A correct check of key `api_key_id` named `account`, inside the write transaction that stores the check's verdict.
/// - The key is bound already: Ok when it is bound to `account` (its exchange account's IBKR identity is established and
///   proven, `identity::establish_read`); Err when it is bound to another (nothing written here: the caller records the
///   read, which fences the key, #710 7b D-IBKR-FENCE).
/// - Not bound yet: it binds when the read routes to its own account (`identity::route_key` with that account the target,
///   rule 8) and no other account holds history of `account` (its evidence, the takeover's sentinel included); else Err
///   (`OTHER_ACCOUNT`), nothing bound.
pub fn bind(c: &Connection, cipher: &Cipher, api_key_id: i64, account: &str, now: DateTime<Utc>) -> Result<Result<(), String>, String> {
    bind_as(c, cipher, api_key_id, account, true, now)
}
/// `bind` for a key save, whose read routed the key already (`identity::route_key`, the user's answer included): no
/// second routing.
pub fn bind_routed(c: &Connection, cipher: &Cipher, api_key_id: i64, account: &str, now: DateTime<Utc>) -> Result<Result<(), String>, String> {
    bind_as(c, cipher, api_key_id, account, false, now)
}
fn bind_as(c: &Connection, cipher: &Cipher, api_key_id: i64, account: &str, scheduled: bool, now: DateTime<Utc>) -> Result<Result<(), String>, String> {
    let db = |e: rusqlite::Error| format!("ibkr binding: {e}");
    let id = crate::identity::AccountId::ibkr(account).ok_or_else(|| "ibkr binding: an account id that is no IBKR id".to_string())?;
    // The key's account row first (a number 1 2.x left has none yet): a bind always establishes its account's identity.
    let mine = match crate::identity::Account::of_key(c, api_key_id).map_err(db)? {
        Some(a) if crate::identity::ready(c).map_err(db)? => a.ensure(c, now).map_err(db)?.id,
        _ => None,
    };
    if let Some(bound) = binding(c, api_key_id).map_err(db)? {
        return Ok(match cipher.decrypt(&bound.account_id_enc) {
            Ok(b) if b == account => { if let Some(a) = mine { established(c, cipher, a, &id, now)?; } Ok(()) }
            Ok(b) => Err(format!("this IBKR connection belongs to account {b}, and the login now names another account")),
            Err(_) => Err("the account this IBKR connection belongs to cannot be read; delete the connection and connect again".into()),
        });
    }
    let fingerprint = crate::identity::key(c, cipher, now)?.fingerprint_text(super::ibkr::IBKR, account);
    if let Some(a) = mine.filter(|_| scheduled) {
        let (user, exchange): (i64, i64) = c.query_row("SELECT user_id, exchange_id FROM exchange_accounts WHERE id = ?1", [a], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db)?;
        let material = crate::sync::credentials(c, cipher, api_key_id).map_err(|e| e.0)?;
        let route = crate::identity::route_key(c, cipher, user, exchange, super::ibkr::IBKR, 0, Some(a), &crate::identity::Reading::Read(&id), &material, Some(api_key_id), now)?;
        let held_elsewhere: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM ibkr_account_evidence v JOIN exchange_accounts x ON x.id = v.account_id
                                                WHERE x.user_id = ?1 AND x.exchange_id = ?2 AND x.removed_at IS NULL AND v.account_id <> ?3 AND v.fingerprint IN (?4, ?5))",
                                               params![user, exchange, a, fingerprint, crate::identity::UNCONFIRMED_AFTER_V2], |r| r.get(0)).map_err(db)?;
        if route != crate::identity::Route::To(a) || held_elsewhere { return Ok(Err(OTHER_ACCOUNT.into())); }
    }
    c.execute("INSERT INTO ibkr_bindings (api_key_id, account_ref, account_fingerprint, account_id_enc, bound_at) VALUES (?1, ?2, ?3, ?4, ?5)",
              params![api_key_id, &fingerprint[..16], fingerprint, cipher.encrypt(account), format_time(now)]).map_err(db)?;
    if let Some(a) = mine { established(c, cipher, a, &id, now)?; }
    crate::sync::settle_readers_of_key(c, api_key_id).map_err(|e| e.0)?;
    Ok(Ok(()))
}

/// A bind's read of `id` on exchange account `account`: its fingerprint established and proven (`identity::establish_read`),
/// and, once it is that account's own, the takeover's sentinel gone (the account is confirmed again, #710 7b).
fn established(c: &Connection, cipher: &Cipher, account: i64, id: &crate::identity::AccountId, now: DateTime<Utc>) -> Result<(), String> {
    crate::identity::establish_read(c, cipher, account, super::ibkr::IBKR, id, now)?;
    let fingerprint = crate::identity::key(c, cipher, now)?.fingerprint(super::ibkr::IBKR, id);
    c.execute("DELETE FROM ibkr_account_evidence WHERE account_id = ?1 AND fingerprint = ?2
               AND (SELECT fingerprint FROM exchange_accounts WHERE id = ?1) = ?3", params![account, crate::identity::UNCONFIRMED_AFTER_V2, fingerprint])
        .map(|_| ()).map_err(|e| format!("ibkr binding: {e}"))
}

/// The user's IBKR keys of `exchange` that have not named their account (no binding), by id.
pub fn unbound_keys(c: &Connection, user: i64, exchange: i64) -> rusqlite::Result<Vec<i64>> {
    c.prepare("SELECT k.id FROM api_keys k WHERE k.user_id = ?1 AND k.exchange_id = ?2 AND NOT EXISTS (SELECT 1 FROM ibkr_bindings b WHERE b.api_key_id = k.id) ORDER BY k.id")?
        .query_map([user, exchange], |r| r.get(0))?.collect()
}

/// The IBKR accounts exchange account `account` holds history of (`ibkr_account_evidence`), the sentinel left out.
pub fn evidence(c: &Connection, account: i64) -> rusqlite::Result<Vec<String>> {
    if !crate::engine::rules::table_exists(c, "ibkr_account_evidence")? { return Ok(vec![]); }
    c.prepare("SELECT fingerprint FROM ibkr_account_evidence WHERE account_id = ?1 AND fingerprint <> ?2 ORDER BY fingerprint")?
        .query_map(params![account, crate::identity::UNCONFIRMED_AFTER_V2], |r| r.get(0))?.collect()
}
/// Whether 2.x may have synced history of an unknown IBKR account onto `account` (the takeover's sentinel).
pub fn takeover_held(c: &Connection, account: i64) -> rusqlite::Result<bool> {
    if !crate::engine::rules::table_exists(c, "ibkr_account_evidence")? { return Ok(false); }
    c.query_row("SELECT EXISTS(SELECT 1 FROM ibkr_account_evidence WHERE account_id = ?1 AND fingerprint = ?2)", params![account, crate::identity::UNCONFIRMED_AFTER_V2], |r| r.get(0))
}
/// #710 7b rule 2: whether `account` can take a read of `fingerprint` without changing its proven identity (so "Connect
/// to it instead" is offered): unproven, and no key of it bound to another IBKR account.
pub fn offerable(c: &Connection, account: i64, fingerprint: &str) -> rusqlite::Result<bool> {
    let fingerprinted: bool = c.query_row("SELECT fingerprint IS NOT NULL FROM exchange_accounts WHERE id = ?1", [account], |r| r.get(0)).optional()?.unwrap_or(false);
    if fingerprinted && crate::identity::proven(c, account)? { return Ok(false); }
    let Some(a) = crate::identity::Account::by_id(c, account)? else { return Ok(false) };
    let other: bool = c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM api_keys k JOIN ibkr_bindings b ON b.api_key_id = k.id WHERE {} AND b.account_fingerprint <> ?1)", a.keys("k")),
                                  [fingerprint], |r| r.get(0))?;
    Ok(!other)
}

/// #710 7b §2, the backfill (at startup after the migration, and at every takeover before `identity::after_v2`): each live
/// IBKR account's evidence E is its live keys' bindings and its bots' IBKR accounts (any status, the key present or gone).
/// Every id in E, and a fingerprint the account has, is evidence (idempotent). An account with no fingerprint takes the one
/// id of E as its proven fingerprint; with two or more it stays unproven (ambiguous); with none it is unchanged. A
/// fingerprint it has stays, and is proven.
pub fn backfill(c: &Connection, cipher: &Cipher, now: DateTime<Utc>) -> Result<(), String> {
    let db = |e: rusqlite::Error| format!("ibkr backfill: {e}");
    if !crate::engine::rules::table_exists(c, "ibkr_account_evidence").map_err(db)? || !crate::identity::ready(c).map_err(db)? { return Ok(()); }
    let accounts: Vec<(i64, Option<String>)> = c.prepare("SELECT a.id, a.fingerprint FROM exchange_accounts a JOIN exchanges e ON e.id = a.exchange_id WHERE e.type = ?1 AND a.removed_at IS NULL ORDER BY a.id")
        .map_err(db)?.query_map([super::ibkr::IBKR], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
    if accounts.is_empty() { return Ok(()); }
    let key = crate::identity::key(c, cipher, now)?;
    for (id, own) in accounts {
        let Some(account) = crate::identity::Account::by_id(c, id).map_err(db)? else { continue };
        let mut seen: Vec<(String, String)> = c.prepare(&format!("SELECT b.account_fingerprint, b.account_id_enc FROM ibkr_bindings b JOIN api_keys k ON k.id = b.api_key_id WHERE {}", account.keys("k")))
            .map_err(db)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
        let bots: Vec<String> = c.prepare(&format!("SELECT o.account_id_enc FROM ibkr_bot_accounts o JOIN bots b ON b.id = o.bot_id WHERE {}", account.bots("b")))
            .map_err(db)?.query_map([], |r| r.get(0)).map_err(db)?.collect::<Result<_, _>>().map_err(db)?;
        for enc in bots {
            let Ok(plain) = cipher.decrypt(&enc) else { continue };
            seen.push((key.fingerprint_text(super::ibkr::IBKR, &plain), enc));
        }
        let mut fingerprints: Vec<&str> = seen.iter().map(|(f, _)| f.as_str()).collect();
        fingerprints.sort();
        fingerprints.dedup();
        for f in fingerprints.iter().copied().chain(own.as_deref()) {
            c.execute("INSERT OR IGNORE INTO ibkr_account_evidence (account_id, fingerprint, source) VALUES (?1, ?2, 'backfill')", params![id, f]).map_err(db)?;
        }
        match (own, fingerprints.as_slice()) {
            (Some(_), _) => prove(c, id, now).map_err(db)?,
            (None, [one]) => {
                let enc = seen.iter().find(|(f, _)| f == one).and_then(|(_, e)| cipher.decrypt(e).ok());
                if let Some(plain) = enc {
                    c.execute("UPDATE exchange_accounts SET fingerprint = ?2, account_id_enc = ?3, updated_at = ?4 WHERE id = ?1 AND fingerprint IS NULL",
                              params![id, one, cipher.encrypt(&plain).into_bytes(), format_time(now)]).map_err(db)?;
                    prove(c, id, now).map_err(db)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn prove(c: &Connection, id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    c.execute("INSERT INTO account_fingerprint_proofs (account_id, created_at) VALUES (?1, ?2) ON CONFLICT (account_id) DO NOTHING", params![id, format_time(now)]).map(|_| ())
}

/// #710 7b, a takeover after 2.x's window: 2.x binds whatever account a login lists first, so every live IBKR account with a
/// proven fingerprint may hold history of an unknown account (the sentinel evidence), and each of its keys is read again
/// before it is trusted (the `unconfirmed-after-2.x` observation fences it until a read names the account).
pub fn after_v2(c: &Connection) -> rusqlite::Result<()> {
    if !crate::engine::rules::table_exists(c, "ibkr_account_evidence")? || !crate::identity::ready(c)? { return Ok(()); }
    let accounts: Vec<i64> = c.prepare("SELECT a.id FROM exchange_accounts a JOIN exchanges e ON e.id = a.exchange_id WHERE e.type = ?1 AND a.removed_at IS NULL
                                        AND a.fingerprint IS NOT NULL AND EXISTS (SELECT 1 FROM account_fingerprint_proofs p WHERE p.account_id = a.id) ORDER BY a.id")?
        .query_map([super::ibkr::IBKR], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for id in accounts {
        c.execute("INSERT OR IGNORE INTO ibkr_account_evidence (account_id, fingerprint, source) VALUES (?1, ?2, 'takeover')", params![id, crate::identity::UNCONFIRMED_AFTER_V2])?;
        let Some(account) = crate::identity::Account::by_id(c, id)? else { continue };
        let keys: Vec<(i64, i64)> = c.prepare(&format!("SELECT k.id, k.exchange_id FROM api_keys k WHERE {}", account.keys("k")))?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        for (key, exchange) in keys {
            c.execute("INSERT INTO exchange_account_credentials (api_key_id, exchange_id, fingerprint) VALUES (?1, ?2, ?3)
                       ON CONFLICT (api_key_id) DO UPDATE SET fingerprint = excluded.fingerprint, account_id_enc = NULL, observed_credential_version = NULL",
                      params![key, exchange, crate::identity::UNCONFIRMED_AFTER_V2])?;
        }
        crate::sync::settle_reader(c, id).map_err(|e| rusqlite::Error::InvalidParameterName(e.0))?;
    }
    Ok(())
}

/// The deleted key's binding goes with it. Its exchange account keeps its IBKR identity while it holds anything
/// (`identity::forget`, #710 7b rule 6), so a reconnect binds only that account.
pub fn forget(c: &Connection, api_key_id: i64) -> rusqlite::Result<()> {
    // An install `run`/`serve` has not migrated has no bindings (as identity::ready).
    let migrated: bool = c.query_row("SELECT count(*) = 1 FROM sqlite_master WHERE type = 'table' AND name = 'ibkr_bindings'", [], |r| r.get(0))?;
    if !migrated { return Ok(()); }
    c.execute("DELETE FROM ibkr_bindings WHERE api_key_id = ?1", [api_key_id]).map(|_| ())
}

/// The trading key of IBKR bot `bot_id` (its account's, #710); None for a bot on any other venue.
pub(crate) fn ibkr_key(c: &Connection, bot_id: i64) -> rusqlite::Result<Option<Option<i64>>> {
    let ibkr: Option<bool> = c.query_row("SELECT e.type = ?2 FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", params![bot_id, super::ibkr::IBKR],
                                         |r| r.get(0)).optional()?;
    if ibkr != Some(true) { return Ok(None); }
    Ok(Some(match crate::identity::Account::of_bot(c, bot_id)? { Some(account) => account.key(c, 0)?, None => None }))
}

/// Bot `bot_id` belongs to the account its key is bound to, unless it already belongs to one. Nothing for a bot on another
/// venue, or whose key is not bound yet.
pub fn assign_bot_account(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let Some(Some(key)) = ibkr_key(c, bot_id)? else { return Ok(()) };
    let Some(bound) = binding(c, key)? else { return Ok(()) };
    c.execute("INSERT INTO ibkr_bot_accounts (bot_id, account_ref, account_id_enc, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (bot_id) DO NOTHING",
              params![bot_id, bound.account_ref, bound.account_id_enc, format_time(now)]).map(|_| ())
}

/// Whether bot `bot_id` may reach IBKR through its key: Ok for a bot on another venue; for an IBKR bot, its key is bound,
/// and to the account the bot belongs to when it belongs to one. Err: why not, in words for the user.
pub fn check_bot_account(c: &Connection, cipher: &Cipher, bot_id: i64) -> rusqlite::Result<Result<(), String>> {
    let Some(key) = ibkr_key(c, bot_id)? else { return Ok(Ok(())) };
    let Some(bound) = key.map(|key| binding(c, key)).transpose()?.flatten() else {
        return Ok(Err("The IBKR connection has not named its account yet. Check the connection again, then start the bot.".into()));
    };
    let owned: Option<String> = c.query_row("SELECT account_id_enc FROM ibkr_bot_accounts WHERE bot_id = ?1", [bot_id], |r| r.get(0)).optional()?;
    let Some(owned) = owned else { return Ok(Ok(())) };
    Ok(match (cipher.decrypt(&owned), cipher.decrypt(&bound.account_id_enc)) {
        (Ok(mine), Ok(now)) if mine == now => Ok(()),
        (Ok(mine), _) => Err(format!("This bot belongs to IBKR account {mine}, and the IBKR connection is now another account. Create a new bot for this account.")),
        _ => Err("This bot belongs to an IBKR account that cannot be read. Create a new bot for the connected account.".into()),
    })
}

/// The account IBKR bot `bot_id` reaches IBKR as: its key's bound account, once `check_bot_account` passes. Err: why not.
pub fn bot_account(c: &Connection, cipher: &Cipher, bot_id: i64) -> rusqlite::Result<Result<String, String>> {
    if let Err(why) = check_bot_account(c, cipher, bot_id)? { return Ok(Err(why)) }
    let Some(Some(key)) = ibkr_key(c, bot_id)? else { return Ok(Err("This bot is not an IBKR bot.".into())) };
    Ok(match binding(c, key)?.map(|b| b.account_id(cipher)) {
        Some(Some(account)) => Ok(account),
        _ => Err("The account this IBKR connection belongs to cannot be read. Delete the connection and connect again.".into()),
    })
}

/// `bot_account` with the account's `account_ref`: what an IBKR bot's venue is built with (`VenueFactory::for_ibkr`).
pub fn bot_binding(c: &Connection, cipher: &Cipher, bot_id: i64) -> rusqlite::Result<Result<(String, String), String>> {
    let account = match bot_account(c, cipher, bot_id)? { Ok(a) => a, Err(why) => return Ok(Err(why)) };
    let Some(Some(key)) = ibkr_key(c, bot_id)? else { return Ok(Err("This bot is not an IBKR bot.".into())) };
    Ok(match binding(c, key)? { Some(b) => Ok((account, b.account_ref)), None => Err("The IBKR connection has not named its account yet.".into()) })
}

/// #710 7b rule 9: the refusal of a placement through a fenced key.
pub const FENCED: &str = "The IBKR login now opens another account than this connection's. Nothing is placed until it opens this account again.";

/// Why IBKR bot `bot_id` may not place an order as `account_ref` (the account its venue reads): its key is not bound to
/// it, or the bot belongs to another account. Compared by account_ref, inside the intent's transaction (plan I4b).
pub fn placement_refusal(c: &Connection, bot_id: i64, account_ref: &str) -> rusqlite::Result<Option<String>> {
    let bound = match ibkr_key(c, bot_id)? { Some(Some(key)) => binding(c, key)?, _ => None };
    let Some(bound) = bound else { return Ok(Some("The IBKR connection has not named its account yet. Check the connection again, then start the bot.".into())) };
    let owned: Option<String> = c.query_row("SELECT account_ref FROM ibkr_bot_accounts WHERE bot_id = ?1", [bot_id], |r| r.get(0)).optional()?;
    // #710 7b rule 9 (D-IBKR-FENCE): a key whose latest read names another account places nothing.
    let key = ibkr_key(c, bot_id)?.flatten();
    let fenced: bool = match key { Some(k) => c.query_row(&format!("SELECT {} FROM api_keys k WHERE k.id = ?1", crate::identity::fenced_sql(c, "k")?), [k], |r| r.get(0)).optional()?.unwrap_or(false), None => false };
    if fenced { return Ok(Some(FENCED.into())); }
    Ok(if bound.account_ref != account_ref { Some("The IBKR connection now names another account than the one this order was read for.".into()) }
       else if owned.is_some_and(|owned| owned != account_ref) { Some("This bot belongs to another IBKR account than the connection's. Create a new bot for this account.".into()) }
       else { None })
}

/// D-IBKR-ACCOUNT-ROWS (#710 7b): what a portfolio read answers when a row names another account than the bound one.
pub const FOREIGN_ROWS: &str = "an IBKR portfolio row names another account than this connection's";
/// Whether portfolio row `row`'s `field` (`acctId` on positions, `acctcode` on the ledger) is present and names another
/// account than `account`. Absent, the row says nothing (2.x books rows by conid or currency only).
pub fn names_other(row: &Value, field: &str, account: &str) -> bool {
    match row.get(field) { None | Some(Value::Null) => false, Some(Value::String(s)) => s.trim() != account, Some(_) => true }
}

/// Whether `/iserver/accounts`' answer names `account` and no other.
pub fn names_only(data: &Value, account: &str) -> bool { listed(data) == [Value::String(account.into())] }

/// #710 7b rule 3: a pending IBKR connection (`ibkr_registrations`) as a key check and a finish read it: its columns in
/// `api_keys` order (key, secret, passphrase, access_token, rsa_signature_key, rsa_encryption_key, dh_param), encrypted,
/// and its whole material decrypted.
pub struct Registration { pub id: i64, pub revision: i64, pub status: i64, pub user_id: i64, pub exchange_id: i64, pub target: Option<i64>, pub updated_at: String,
                          pub columns: [Option<String>; 7], pub realm: Option<String>, pub credentials: crate::crypto::Credentials,
                          /// Its material's version as a save validates it (id 0: no key row yet).
                          pub producer: crate::engine::model::CredentialVersion }

/// Registration `id` (None: gone).
pub fn registration(c: &Connection, cipher: &Cipher, id: i64) -> Result<Option<Registration>, String> {
    let db = |e: rusqlite::Error| format!("ibkr registration: {e}");
    let row = c.query_row("SELECT revision, status, user_id, exchange_id, target_account_id, updated_at, key, secret, access_token, rsa_signature_key, rsa_encryption_key, dh_param, ibkr_realm
                           FROM ibkr_registrations WHERE id = ?1", [id],
                          |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, String>(5)?,
                                  [r.get::<_, Option<String>>(6)?, r.get(7)?, None, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?], r.get::<_, Option<String>>(12)?))).optional().map_err(db)?;
    let Some((revision, status, user_id, exchange_id, target, updated_at, columns, realm)) = row else { return Ok(None) };
    let open = |v: &Option<String>| v.as_ref().map(|v| cipher.decrypt(v).map_err(|_| format!("ibkr registration {id} is unreadable"))).transpose().map(Option::unwrap_or_default);
    let (key, secret, access_token, signature_key, encryption_key, dh_param) = (open(&columns[0])?, open(&columns[1])?, open(&columns[3])?, open(&columns[4])?, open(&columns[5])?, open(&columns[6])?);
    let material = super::signers::ibkr::Material { consumer_key: key.clone(), access_token: access_token.clone(), access_token_secret: secret.clone(), signature_key: signature_key.clone(),
                                                   encryption_key: encryption_key.clone(), dh_param: dh_param.clone(), realm: realm.clone().unwrap_or_default() };
    let credentials = crate::crypto::Credentials { redaction_values: vec![key.clone(), secret.clone(), access_token, signature_key, encryption_key, dh_param], key, secret, passphrase: None,
                                                   ibkr: Some(Box::new(material)) };
    let producer = crate::engine::model::prepared_credential_version(0, &columns).with_realm(realm.as_deref());
    Ok(Some(Registration { id, revision, status, user_id, exchange_id, target, updated_at, columns, realm, credentials, producer }))
}
/// The registrations awaiting IBKR's activation (status 3), by id: the activation job's to finish (rule 5).
pub fn awaiting(c: &Connection) -> rusqlite::Result<Vec<i64>> {
    if !crate::engine::rules::table_exists(c, "ibkr_registrations")? { return Ok(vec![]); }
    c.prepare("SELECT id FROM ibkr_registrations WHERE status = 3 ORDER BY id")?.query_map([], |r| r.get(0))?.collect()
}

// ---- #710 7b §2: the per-login read sequence (`ibkr_login_reads`, D-IBKR-FENCE "per login, the latest read wins") ----

/// Where a login's `/iserver/accounts` reads are numbered: the database file, the login's digest (`login_digest`) and the
/// cipher an in-lock read records what it saw with. `closed()` (its state could not be loaded): every read and gate
/// through it refuses, so nothing is signed (branch r1 #5).
#[derive(Clone)]
pub struct Reads { pub path: String, pub digest: String, cipher: Option<Cipher> }

/// One accounts read's place in its login's order: `seq` the number it took, `prev` the latest before it (0: none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Taken { pub seq: i64, pub prev: i64 }

/// The reads of `credentials`' login in database `c`. None: an in-memory database or one not migrated (no key is bound
/// there either): the reads go unnumbered. Numbered from the first read on, an identity key or not (branch r1 #4); a
/// schema that cannot be read is `Reads::closed()`.
pub fn reads(c: &Connection, cipher: &Cipher, credentials: &crate::crypto::Credentials) -> Option<Reads> {
    let path = c.path().filter(|p| !p.is_empty())?.to_string();
    match crate::engine::rules::table_exists(c, "ibkr_login_reads") {
        Ok(true) => Some(Reads { path, digest: login_digest(cipher, credentials), cipher: Some(cipher.clone()) }),
        Ok(false) => None,
        Err(_) => Some(Reads::closed()),
    }
}
/// A login's digest: its full material (`identity::ibkr_material`, the effective realm included) under the install's
/// encryption key, so it exists before any identity key (a first connection's reads are numbered when they are made).
pub fn login_digest(cipher: &Cipher, credentials: &crate::crypto::Credentials) -> String {
    cipher.digest("deltabadger-ibkr-login-v1", &[&credentials.key, &credentials.secret, credentials.passphrase.as_deref().unwrap_or(""), &crate::identity::ibkr_material(credentials)])
}

/// The next number of `digest`'s reads (taken while the read holds its login: reads are numbered in the order they are made).
pub fn take_seq(c: &Connection, digest: &str) -> rusqlite::Result<Taken> {
    let prev: i64 = c.query_row("SELECT seq FROM ibkr_login_reads WHERE login_digest = ?1", [digest], |r| r.get(0)).optional()?.unwrap_or(0);
    c.execute("INSERT INTO ibkr_login_reads (login_digest, seq, committed_seq) VALUES (?1, ?2, 0) ON CONFLICT (login_digest) DO UPDATE SET seq = excluded.seq",
              params![digest, prev + 1])?;
    Ok(Taken { seq: prev + 1, prev })
}
/// Whether read `seq` is still `digest`'s latest: a verdict is acted on only then.
pub fn seq_current(c: &Connection, digest: &str, seq: i64) -> rusqlite::Result<bool> {
    Ok(c.query_row("SELECT seq = ?2 FROM ibkr_login_reads WHERE login_digest = ?1", params![digest, seq], |r| r.get(0)).optional()?.unwrap_or(false))
}
/// Read `seq`'s verdict acted on, naming `fingerprint` (None: it named no account). False, nothing written, when a later
/// read of the login was taken meanwhile (that one's verdict decides).
pub fn commit_seq(c: &Connection, digest: &str, seq: i64, fingerprint: Option<&str>) -> rusqlite::Result<bool> {
    Ok(c.execute("UPDATE ibkr_login_reads SET committed_seq = seq, committed_fingerprint = ?3 WHERE login_digest = ?1 AND seq = ?2", params![digest, seq, fingerprint])? == 1)
}
/// Rule 12(b): whether key `api_key_id` may send without an accounts read of its own: its login's latest read was acted
/// on and named the account the key is bound to. No row (a key bound before 7b) or an unfinished read is "check first".
pub fn authorized(c: &Connection, digest: &str, api_key_id: i64) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM ibkr_login_reads r JOIN ibkr_bindings b ON b.api_key_id = ?2
                 WHERE r.login_digest = ?1 AND r.committed_seq = r.seq AND r.committed_fingerprint = b.account_fingerprint)", params![digest, api_key_id], |r| r.get(0))
}

/// A verdict read's number as its acting transaction holds it: the login digest and the read (None: unnumbered).
pub type Verdict = Option<(String, Taken)>;
/// Whether verdict read `taken` may still be acted on (no later read of its login taken). An unnumbered read may.
pub fn verdict_current(c: &Connection, taken: &Verdict) -> rusqlite::Result<bool> {
    match taken { Some((digest, t)) => seq_current(c, digest, t.seq), None => Ok(true) }
}
/// Verdict read `taken`, naming `read` (None: no account), acted on in the caller's transaction: committed (its fingerprint
/// under the identity key, which a bind before it made). False, nothing written, when a later read of the login was taken
/// meanwhile. An unnumbered read (an in-memory database) commits nothing.
pub fn commit_verdict(c: &Connection, cipher: &Cipher, taken: &Verdict, read: Option<&crate::identity::AccountId>) -> rusqlite::Result<bool> {
    let Some((digest, t)) = taken else { return Ok(true) };
    let fingerprint = read.and_then(|id| crate::identity::existing_key(c, cipher).map(|k| k.fingerprint(super::ibkr::IBKR, id)));
    commit_seq(c, digest, t.seq, fingerprint.as_deref())
}

/// The read of `answer` an accounts list comes to: the one IBKR account it names (None: none, several, or no IBKR id).
pub fn one_listed(answer: &Value) -> Option<String> {
    match listed(answer).as_slice() { [one] => one.as_str().and_then(parse).map(str::to_string), _ => None }
}

/// The process-wide order of each login's reads: a key check has a login of its own, so two reads of one login are
/// numbered and sent one at a time here. ponytail: never pruned (one small entry per login read since start).
static ORDER: LazyLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);

impl Reads {
    /// A login whose read state could not be loaded: nothing is numbered, authorized or sent through it (fails closed).
    pub fn closed() -> Self { Self { path: String::new(), digest: String::new(), cipher: None } }
    /// Whether this is `closed()`.
    pub fn is_closed(&self) -> bool { self.path.is_empty() }
    fn write<T>(&self, work: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, String>) -> Result<T, String> {
        if self.is_closed() { return Err("ibkr login reads: the login's read state could not be loaded".into()); }
        let db = |e: rusqlite::Error| format!("ibkr login reads: {e}");
        let mut c = Connection::open_with_flags(&self.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(db)?;
        c.busy_timeout(std::time::Duration::from_secs(5)).map_err(db)?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(db)?;
        let out = work(&tx)?;
        tx.commit().map_err(db)?;
        Ok(out)
    }
    fn read<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        if self.is_closed() { return Err(rusqlite::Error::InvalidPath("closed".into())); }
        let c = Connection::open_with_flags(&self.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        work(&c)
    }
    /// This login's place in the read order, held while one read is numbered and sent.
    pub async fn ordered(&self) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = ORDER.lock().unwrap_or_else(|p| p.into_inner()).entry(self.digest.clone()).or_default().clone();
        lock.lock_owned().await
    }
    pub fn take(&self) -> Result<Taken, String> { self.write(|tx| take_seq(tx, &self.digest).map_err(|e| e.to_string())) }
    /// Rule 12(b) for key `api_key_id` (`authorized`); a database it cannot read is no authority.
    pub fn authorized(&self, api_key_id: i64) -> bool { self.read(|c| authorized(c, &self.digest, api_key_id)).unwrap_or(false) }
    /// r6, an in-lock read (the balance read, `submit`, `IbkrVenue::checked`) of key `version.id`, bound to `bound`, committed
    /// before its lock is released: a read naming `bound` only commits; one naming one other account is recorded as the
    /// key's observation (which fences it, rule 7) and commits. Several or none is left for a key check (an unfinished
    /// read: rule 7 checks it, rule 12 sends nothing meanwhile). A later read taken meanwhile decides instead.
    /// Branch r1 #7: a read naming `bound` is judged against the key's account too: when that account is proven as another
    /// IBKR account (a 7a upgrade that retained it), the read is recorded (the key is fenced there) as one naming another is.
    pub fn commit_in_lock(&self, version: &crate::engine::model::CredentialVersion, taken: Taken, bound: &str, answer: &Value, now: DateTime<Utc>) -> Result<(), String> {
        let Some(read) = one_listed(answer) else { return Ok(()) };
        let cipher = self.cipher.as_ref().ok_or("ibkr login reads: closed")?;
        self.write(|tx| {
            let db = |e: rusqlite::Error| format!("ibkr login reads: {e}");
            if !seq_current(tx, &self.digest, taken.seq).map_err(db)? { return Ok(()); }
            let Some(key) = crate::identity::existing_key(tx, cipher) else { return Ok(()) };
            let fingerprint = key.fingerprint_text(super::ibkr::IBKR, &read);
            let account = crate::identity::account_of_key(tx, version.id).map_err(db)?;
            let proven_other = match account {
                Some(a) => tx.query_row("SELECT fingerprint IS NOT NULL AND fingerprint <> ?2 FROM exchange_accounts WHERE id = ?1", params![a, fingerprint], |r| r.get::<_, bool>(0))
                    .optional().map_err(db)?.unwrap_or(false) && crate::identity::proven(tx, a).map_err(db)?,
                None => false,
            };
            if (read != bound || proven_other) && crate::engine::model::credential_is_current(tx, version).map_err(|e| format!("{e:?}"))? {
                let id = crate::identity::AccountId::ibkr(&read).ok_or("ibkr login reads: an account id that is no IBKR id")?;
                let credentials = crate::sync::credentials(tx, cipher, version.id).map_err(|e| e.0)?;
                crate::identity::record(tx, cipher, version.id, &credentials, version, &crate::identity::Observed::Read(id), now)?;
            }
            commit_seq(tx, &self.digest, taken.seq, Some(&fingerprint)).map_err(db)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn account_ids_are_live_paper_or_demo() {
        for good in ["U1234567", "DU1", "DF42"] { assert_eq!(parse(good), Some(good)); }
        for bad in ["", "U", "DU", "u123", "X123", "U12a", " U1", "All", "F1234"] { assert_eq!(parse(bad), None, "{bad:?}"); }
    }

    #[test]
    fn listed_is_extract_accounts() {
        assert_eq!(listed(&json!({"accounts": ["U1"], "selectedAccount": "U1"})), [json!("U1")]);
        assert_eq!(listed(&json!({"accounts": ["U2", "U1"], "selectedAccount": "U1"})), [json!("U1"), json!("U2")]);
        assert_eq!(listed(&json!(["U1", null, "U1"])), [json!("U1")]);
        assert_eq!(listed(&json!("OK")), [json!("OK")]);
        assert!(listed(&json!(null)).is_empty() && listed(&json!({})).is_empty() && listed(&json!({"accounts": []})).is_empty());
    }
}
