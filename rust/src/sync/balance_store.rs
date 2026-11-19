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
const STORES: [(&str, &str, &str); 2] = [("account_balances", BALANCES, BALANCE_COLUMNS), ("portfolio_venue_snapshots", SNAPSHOTS, SNAPSHOT_COLUMNS)];

/// SQL: what 2.x's table `v2` (`account_balances` or `portfolio_venue_snapshots`) would hold, read from its store with 2.x's
/// columns under 2.x's name, for the parity snapshots keyed by 2.x's names. None for any other name.
pub fn v2_projection(v2: &str) -> Option<String> {
    STORES.iter().find(|(name, _, _)| *name == v2).map(|(name, store, columns)| format!("(SELECT {columns} FROM {store}) AS {name}"))
}

/// The takeover after 2.x owned the install (handover's window): 2.x wrote only its own tables, so the number-1 rows of each
/// store are 2.x's rows again. Run after `identity::fill` (every (user, exchange) 2.x named has its number 1), in the claim.
/// Rows of accounts 2+ stay. 2.x counted ids from its own sequence and Rust gave account 2+ rows ids from the store's, so an
/// id can be held on both sides: the store's sequence is raised above both ranges before any insert, a 2.x row whose id an
/// account 2+ row holds gets a fresh id, and every other row keeps 2.x's (with one account per exchange, all of them).
pub fn refill_number_one(c: &Connection) -> rusqlite::Result<()> {
    const NUMBER_1: &str = "(SELECT a.id FROM exchange_accounts a WHERE a.user_id = t.user_id AND a.exchange_id = t.exchange_id AND a.number = 1)";
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
