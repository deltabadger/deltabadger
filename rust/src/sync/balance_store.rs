//! #710 slice 5 (PLAN-710-5 §4): an account's balances and venue snapshots. 2.x's `account_balances` and
//! `portfolio_venue_snapshots` are unique per (user, exchange, …), so both moved to Rust-owned stores keyed by account
//! (migration 20261121000001): `exchange_account_balances` and `exchange_account_snapshots`, 2.x's columns in 2.x's
//! order with `account_id` last. 2.x's tables stay, unread but here: the takeover's refill and the parity projections.
use rusqlite::Connection;

/// The balance store and its 2.x columns, as 2.x orders them.
pub const BALANCES: &str = "exchange_account_balances";
pub const BALANCE_COLUMNS: &str = "id, asset_id, created_at, exchange_id, free, locked, priced_at, synced_at, updated_at, usd_price, usd_value, user_id";
/// The venue snapshot store and its 2.x columns.
pub const SNAPSHOTS: &str = "exchange_account_snapshots";
pub const SNAPSHOT_COLUMNS: &str = "id, user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at";
/// (2.x table, its store, their shared columns).
const STORES: [(&str, &str, &str); 2] = [(V2_BALANCES, BALANCES, BALANCE_COLUMNS), (V2_SNAPSHOTS, SNAPSHOTS, SNAPSHOT_COLUMNS)];

/// 2.x's names of the two tables, for the parity snapshots keyed by them.
pub const V2_BALANCES: &str = "account_balances";
pub const V2_SNAPSHOTS: &str = "portfolio_venue_snapshots";

/// Whether the install has the stores (an install a `run` has not migrated reads and writes 2.x's tables, as before).
pub fn ready(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [SNAPSHOTS], |r| r.get(0))
}

/// SQL: what 2.x's table `v2` (`account_balances` or `portfolio_venue_snapshots`) would hold, read from its store with 2.x's
/// columns under 2.x's name, for the parity snapshots keyed by 2.x's names. None for any other name, and on an install
/// without the stores (2.x's own table is what it holds).
pub fn v2_projection(c: &Connection, v2: &str) -> rusqlite::Result<Option<String>> {
    if !ready(c)? { return Ok(None); }
    Ok(STORES.iter().find(|(name, _, _)| *name == v2).map(|(name, store, columns)| format!("(SELECT {columns} FROM {store}) AS {name}")))
}

/// Every venue snapshot of `user` goes (a rewritten ledger invalidates them all; the sweep writes them again).
pub fn forget_snapshots(c: &Connection, user: i64) -> rusqlite::Result<()> {
    let table = if ready(c)? { SNAPSHOTS } else { V2_SNAPSHOTS };
    c.execute(&format!("DELETE FROM {table} WHERE user_id = ?1"), [user]).map(drop)
}

/// The takeover after 2.x owned the install (handover's window): 2.x wrote only its own tables, so the number-1 rows of each
/// store are 2.x's rows again. Run after `identity::fill` (every (user, exchange) 2.x named has its number 1), in the claim.
/// Rows of accounts 2+ stay. 2.x counted ids from its own sequence and Rust gave account 2+ rows ids from the store's, so an
/// id can be held on both sides: the store's sequence is raised above both ranges before any insert, a 2.x row whose id an
/// account 2+ row holds gets a fresh id, and every other row keeps 2.x's (with one account per exchange, all of them).
pub fn refill_number_one(c: &Connection) -> rusqlite::Result<()> {
    const NUMBER_1: &str = "(SELECT a.id FROM exchange_accounts a WHERE a.user_id = t.user_id AND a.exchange_id = t.exchange_id AND a.number = 1)";
    // Rows stay eager: an exchange 2.x connected gets its number 1 (identity::FILL's rule, for these two tables).
    c.execute(&format!("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
                        SELECT user_id, exchange_id, 1, datetime('now'), datetime('now') FROM ({}) n
                        WHERE NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = n.user_id AND a.exchange_id = n.exchange_id AND a.number = 1)
                        ORDER BY user_id, exchange_id",
                       STORES.map(|(v2, _, _)| format!("SELECT user_id, exchange_id FROM {v2}")).join(" UNION ")), [])?;
    for (v2, store, columns) in STORES {
        c.execute(&format!("DELETE FROM {store} WHERE account_id IN (SELECT id FROM exchange_accounts WHERE number = 1)"), [])?;
        let seq: i64 = c.query_row(&format!("SELECT max(coalesce((SELECT seq FROM sqlite_sequence WHERE name = '{store}'), 0), coalesce((SELECT max(id) FROM {store}), 0),
                                                        coalesce((SELECT seq FROM sqlite_sequence WHERE name = '{v2}'), 0), coalesce((SELECT max(id) FROM {v2}), 0))"), [], |r| r.get(0))?;
        c.execute("DELETE FROM sqlite_sequence WHERE name = ?1", [store])?;
        c.execute("INSERT INTO sqlite_sequence (name, seq) VALUES (?1, ?2)", rusqlite::params![store, seq])?;
        let rest = columns.strip_prefix("id, ").expect("id first");
        let of = |list: &str| list.split(", ").map(|n| format!("t.{n}")).collect::<Vec<_>>().join(", ");
        // Held by an account 2+ row: a fresh id, above both ranges.
        c.execute(&format!("INSERT INTO {store} ({rest}, account_id) SELECT {}, {NUMBER_1} FROM {v2} t WHERE t.id IN (SELECT id FROM {store}) ORDER BY t.id", of(rest)), [])?;
        c.execute(&format!("INSERT INTO {store} ({columns}, account_id) SELECT {}, {NUMBER_1} FROM {v2} t WHERE t.id NOT IN (SELECT id FROM {store}) ORDER BY t.id", of(columns)), [])?;
    }
    Ok(())
}
