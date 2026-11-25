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
