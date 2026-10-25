//! IBKR accounts (Ibkr plan I2, D-IBKR-ACCOUNT). One IBKR login can hold several brokerage accounts, and the account
//! behind a login can change without its credentials changing. Rails reads whichever account the login lists first; here:
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

/// A correct check of key `api_key_id` named `account`: the key is bound to it when it has no binding yet. Err: the key
/// is bound to another account (or its bound account cannot be read); nothing is written. Call inside the write
/// transaction that stores the check's verdict.
pub fn bind(c: &Connection, cipher: &Cipher, api_key_id: i64, account: &str, now: DateTime<Utc>) -> Result<Result<(), String>, String> {
    let db = |e: rusqlite::Error| format!("ibkr binding: {e}");
    if let Some(bound) = binding(c, api_key_id).map_err(db)? {
        return Ok(match cipher.decrypt(&bound.account_id_enc) {
            Ok(id) if id == account => Ok(()),
            Ok(id) => Err(format!("this IBKR connection belongs to account {id}, and the login now names another account; delete the connection and connect again to use it")),
            Err(_) => Err("the account this IBKR connection belongs to cannot be read; delete the connection and connect again".into()),
        });
    }
    let fingerprint = crate::identity::key(c, cipher, now)?.fingerprint_text(super::ibkr::IBKR, account);
    c.execute("INSERT INTO ibkr_bindings (api_key_id, account_ref, account_fingerprint, account_id_enc, bound_at) VALUES (?1, ?2, ?3, ?4, ?5)",
              params![api_key_id, &fingerprint[..16], fingerprint, cipher.encrypt(account), format_time(now)]).map_err(db)?;
    Ok(Ok(()))
}

/// The deleted key's binding goes with it: a reconnect binds whatever account it names.
pub fn forget(c: &Connection, api_key_id: i64) -> rusqlite::Result<()> {
    // An install `run`/`serve` has not migrated has no bindings (as identity::ready).
    let migrated: bool = c.query_row("SELECT count(*) = 1 FROM sqlite_master WHERE type = 'table' AND name = 'ibkr_bindings'", [], |r| r.get(0))?;
    if !migrated { return Ok(()); }
    c.execute("DELETE FROM ibkr_bindings WHERE api_key_id = ?1", [api_key_id]).map(|_| ())
}

/// The trading key of IBKR bot `bot_id`; None for a bot on any other venue.
pub(crate) fn ibkr_key(c: &Connection, bot_id: i64) -> rusqlite::Result<Option<Option<i64>>> {
    c.query_row("SELECT e.type = ?2, (SELECT k.id FROM api_keys k WHERE k.user_id = b.user_id AND k.exchange_id = b.exchange_id AND k.key_type = 0 ORDER BY k.id LIMIT 1)
                 FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", params![bot_id, super::ibkr::IBKR],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<i64>>(1)?))).optional()
        .map(|row| row.and_then(|(ibkr, key)| ibkr.then_some(key)))
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

/// Why IBKR bot `bot_id` may not place an order as `account_ref` (the account its venue reads): its key is not bound to
/// it, or the bot belongs to another account. Compared by account_ref, inside the intent's transaction (plan I4b).
pub fn placement_refusal(c: &Connection, bot_id: i64, account_ref: &str) -> rusqlite::Result<Option<String>> {
    let bound = match ibkr_key(c, bot_id)? { Some(Some(key)) => binding(c, key)?, _ => None };
    let Some(bound) = bound else { return Ok(Some("The IBKR connection has not named its account yet. Check the connection again, then start the bot.".into())) };
    let owned: Option<String> = c.query_row("SELECT account_ref FROM ibkr_bot_accounts WHERE bot_id = ?1", [bot_id], |r| r.get(0)).optional()?;
    Ok(if bound.account_ref != account_ref { Some("The IBKR connection now names another account than the one this order was read for.".into()) }
       else if owned.is_some_and(|owned| owned != account_ref) { Some("This bot belongs to another IBKR account than the connection's. Create a new bot for this account.".into()) }
       else { None })
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
