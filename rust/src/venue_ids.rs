//! #710 slice 4 (PLAN-710-4 §2): a ledger row's venue id (`account_transactions.tx_id`) and a bot order's
//! (`transactions.external_id`), wherever their account keeps them. 2.x's unique indexes name the exchange, so a row of an
//! account 2+ stores NULL in the 2.x column and its id in its membership row (`exchange_account_rows`,
//! `exchange_account_transactions`); a number-1 row has no membership and keeps its id where 2.x reads it. The insert
//! triggers (20261120000001) keep the two forms apart: a member is an account 2+ of its row's exchange and its 2.x id is
//! NULL. Nothing but this module names those columns or tables in SQL.
use crate::identity::Account;
use rusqlite::{params, Connection, OptionalExtension};

/// Whose rows a query reads and a write writes: (user, exchange) and, for an account 2+, its id (`member`). None is the
/// exchange's number 1, whose rows are the ones without a membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope { pub user_id: i64, pub exchange_id: i64, pub member: Option<i64>, ready: bool }

/// Whether the install has the membership tables (one `run`/`serve` has not migrated reads as before #710).
pub fn ready(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'exchange_account_rows')", [], |r| r.get(0))
}

impl Scope {
    /// Account `account`'s.
    pub fn of(c: &Connection, account: &Account) -> rusqlite::Result<Self> {
        let ready = ready(c)?;
        let member = match account.id { Some(id) if ready && account.number(c)? != 1 => Some(id), _ => None };
        Ok(Self { user_id: account.user_id, exchange_id: account.exchange_id, member, ready })
    }
    /// The number 1 of (user, exchange).
    pub fn first(c: &Connection, user_id: i64, exchange_id: i64) -> rusqlite::Result<Self> {
        Ok(Self { user_id, exchange_id, member: None, ready: ready(c)? })
    }
    /// Api key `api_key_id`'s account's. None: no such key, or its account resolves to none.
    pub fn of_key(c: &Connection, api_key_id: i64) -> rusqlite::Result<Option<Self>> {
        Account::of_key(c, api_key_id)?.map(|a| Self::of(c, &a)).transpose()
    }
    /// Bot `bot_id`'s current account's.
    pub fn of_bot(c: &Connection, bot_id: i64) -> rusqlite::Result<Option<Self>> {
        Account::of_bot(c, bot_id)?.map(|a| Self::of(c, &a)).transpose()
    }
    /// The account bot order `transaction_id` was placed on.
    pub fn of_order(c: &Connection, transaction_id: i64) -> rusqlite::Result<Option<Self>> {
        Account::of_order(c, transaction_id)?.map(|a| Self::of(c, &a)).transpose()
    }
    /// SQL predicate: ledger row `a` is this account's, beside the caller's `user_id`/`exchange_id` conditions.
    pub fn rows(&self, a: &str) -> String { self.members(a, "exchange_account_rows", "account_transaction_id") }
    /// SQL predicate: bot order `t` was placed on this account, beside the caller's `exchange_id` (and owner) conditions.
    pub fn orders(&self, t: &str) -> String { self.members(t, "exchange_account_transactions", "transaction_id") }
    fn members(&self, a: &str, table: &str, column: &str) -> String {
        if !self.ready { return "1".into(); }
        match self.member {
            None => format!("NOT EXISTS (SELECT 1 FROM {table} m WHERE m.{column} = {a}.id)"),
            Some(n) => format!("{a}.id IN (SELECT {column} FROM {table} WHERE account_id = {n})"),
        }
    }
    /// What the 2.x column gets on INSERT: the id for number 1, NULL for an account 2+ (its id goes to `after_insert`).
    pub fn stored<'a>(&self, id: Option<&'a str>) -> Option<&'a str> { if self.member.is_some() { None } else { id } }
}

/// SQL: ledger row `a`'s venue id. Exact: a number-1 row has no membership, a member's 2.x column is NULL.
pub fn row_id(a: &str) -> String {
    format!("COALESCE({a}.tx_id, (SELECT m.venue_tx_id FROM exchange_account_rows m WHERE m.account_transaction_id = {a}.id))")
}
/// SQL: ledger row `a`'s account id: its membership's, else its (user, exchange)'s number 1 (NULL while that has no row).
pub fn row_account(a: &str) -> String {
    format!("COALESCE((SELECT m.account_id FROM exchange_account_rows m WHERE m.account_transaction_id = {a}.id), \
             (SELECT n.id FROM exchange_accounts n WHERE n.user_id = {a}.user_id AND n.exchange_id = {a}.exchange_id AND n.number = 1))")
}
/// SQL: ledger row `a` has no venue id.
pub fn id_less(a: &str) -> String { format!("{} IS NULL", row_id(a)) }
/// SQL: ledger row `a` has a venue id.
pub fn has_id(a: &str) -> String { format!("{} IS NOT NULL", row_id(a)) }

/// Ledger row `row`, just inserted in this transaction with `scope.stored(id)`: an account 2+'s membership and venue id.
pub fn after_insert(c: &Connection, scope: &Scope, row: i64, id: Option<&str>) -> rusqlite::Result<()> {
    if let Some(n) = scope.member {
        c.execute("INSERT INTO exchange_account_rows (account_transaction_id, account_id, venue_tx_id) VALUES (?1, ?2, ?3)", params![row, n, id])?;
    }
    Ok(())
}

/// Whether the account holds a ledger row with venue id `id`.
pub fn known(c: &Connection, scope: &Scope, id: &str) -> rusqlite::Result<bool> {
    match scope.member {
        // A member's 2.x column is NULL: the 2.x index alone is number 1's.
        None => c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3)",
                            params![scope.user_id, scope.exchange_id, id], |r| r.get(0)),
        Some(n) => c.query_row("SELECT EXISTS(SELECT 1 FROM exchange_account_rows WHERE account_id = ?1 AND venue_tx_id = ?2)", params![n, id], |r| r.get(0)),
    }
}

/// How `set_id` writes, today's three statements exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// `SET tx_id = ?`, NULL included.
    Replace,
    /// `SET tx_id = COALESCE(?, tx_id)`: a given id replaces a stored one, none keeps it.
    IfGiven,
    /// `SET tx_id = ? … WHERE tx_id IS NULL`: an id-less row takes the id.
    Adopt,
}

/// Ledger row `row` of `scope`'s account gets venue id `new` as `write` says; the rows changed (0 or 1). A unique clash is
/// SQLite's ConstraintViolation, as from 2.x's index. A row of another account is never written.
pub fn set_id(c: &Connection, scope: &Scope, row: i64, new: Option<&str>, write: Write) -> rusqlite::Result<usize> {
    let (table, column, key, owned) = match scope.member {
        None => ("account_transactions", "tx_id", "id", format!("user_id = {} AND exchange_id = {} AND {}", scope.user_id, scope.exchange_id, scope.rows("account_transactions"))),
        Some(n) => ("exchange_account_rows", "venue_tx_id", "account_transaction_id", format!("account_id = {n}")),
    };
    let (value, guard) = match write {
        Write::Replace => ("?1".to_string(), String::new()),
        Write::IfGiven => (format!("COALESCE(?1, {column})"), String::new()),
        Write::Adopt => ("?1".to_string(), format!(" AND {column} IS NULL")),
    };
    c.execute(&format!("UPDATE {table} SET {column} = {value} WHERE {key} = ?2 AND {owned}{guard}"), params![new, row])
}

/// The dust adoption: two id-less legs of `scope`'s account take their ids, both or neither. Number 1 keeps today's one
/// statement; an account 2+ runs both `Adopt` writes in a savepoint, rolled back when either changed no row (0) or clashed
/// (the ConstraintViolation, as the one statement's).
pub fn adopt_pair(c: &Connection, scope: &Scope, legs: [(i64, &str); 2]) -> rusqlite::Result<usize> {
    let [(a, a_id), (b, b_id)] = legs;
    if scope.member.is_none() {
        return c.execute(&format!("UPDATE account_transactions SET tx_id = CASE id WHEN ?1 THEN ?2 ELSE ?3 END WHERE id IN (?1, ?4) AND tx_id IS NULL AND {}",
                                  scope.rows("account_transactions")), params![a, a_id, b_id, b]);
    }
    c.execute_batch("SAVEPOINT adopt_pair")?;
    let both = set_id(c, scope, a, Some(a_id), Write::Adopt).and_then(|n| Ok(n + set_id(c, scope, b, Some(b_id), Write::Adopt)?));
    match both {
        Ok(2) => { c.execute_batch("RELEASE adopt_pair")?; Ok(2) }
        other => { c.execute_batch("ROLLBACK TO adopt_pair; RELEASE adopt_pair")?; other.map(|_| 0) }
    }
}

/// SQL: bot order `t`'s venue id.
pub fn order_id(t: &str) -> String {
    format!("COALESCE({t}.external_id, (SELECT m.venue_external_id FROM exchange_account_transactions m WHERE m.transaction_id = {t}.id))")
}
/// SQL: the account bot order `t` was placed on: its membership's, else the number 1 of its bot's user and its exchange.
pub fn order_account(t: &str) -> String {
    format!("COALESCE((SELECT m.account_id FROM exchange_account_transactions m WHERE m.transaction_id = {t}.id), \
             (SELECT n.id FROM exchange_accounts n JOIN bots b ON b.id = {t}.bot_id WHERE n.user_id = b.user_id AND n.exchange_id = {t}.exchange_id AND n.number = 1))")
}

/// Bot order `row`, just inserted in this transaction with `scope.stored(external)` (`scope`: the account it was placed
/// on): an account 2+'s membership and venue id.
pub fn after_order_insert(c: &Connection, scope: &Scope, row: i64, external: Option<&str>) -> rusqlite::Result<()> {
    if let Some(n) = scope.member {
        c.execute("INSERT INTO exchange_account_transactions (transaction_id, account_id, venue_external_id) VALUES (?1, ?2, ?3)", params![row, n, external])?;
    }
    Ok(())
}

/// The bot order of `scope`'s account with venue id `external`, of one of its user's bots. Number 1: 2.x's unique
/// (exchange_id, external_id); an account 2+: its UNIQUE (account_id, venue_external_id).
pub fn order_by_id(c: &Connection, scope: &Scope, external: &str) -> rusqlite::Result<Option<i64>> {
    match scope.member {
        None => c.query_row("SELECT id FROM transactions WHERE exchange_id = ?1 AND external_id = ?2 AND bot_id IN (SELECT id FROM bots WHERE user_id = ?3)",
                            params![scope.exchange_id, external, scope.user_id], |r| r.get(0)).optional(),
        Some(n) => c.query_row("SELECT transaction_id FROM exchange_account_transactions WHERE account_id = ?1 AND venue_external_id = ?2",
                               params![n, external], |r| r.get(0)).optional(),
    }
}
