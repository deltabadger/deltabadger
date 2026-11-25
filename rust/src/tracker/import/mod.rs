//! The tracker's CSV import (Import::Run and its readers): `csv` the text, rows, zones, times and amounts; `binance`
//! Binance's own export; `kucoin` KuCoin's account history (Rust-only); `deltabadger` the app's own export read back.
pub mod binance;
pub mod csv;
pub mod deltabadger;
pub mod kucoin;
pub mod run;
pub mod service;

/// PLAN-710-importmem B5: values collected once each, in the order first met (as a list shows them), each found by its
/// hash, not by comparing it with every one before it.
#[derive(Debug, Clone)]
pub(crate) struct Distinct<T> { list: Vec<T>, seen: std::collections::HashSet<T> }

impl<T> Default for Distinct<T> { fn default() -> Self { Self { list: vec![], seen: Default::default() } } }

impl<T: Eq + std::hash::Hash + Clone> Distinct<T> {
    pub(crate) fn contains<Q: Eq + std::hash::Hash + ?Sized>(&self, value: &Q) -> bool where T: std::borrow::Borrow<Q> { self.seen.contains(value) }
    /// Kept, unless it was met before.
    pub(crate) fn insert(&mut self, value: T) { if self.seen.insert(value.clone()) { self.list.push(value); } }
    pub(crate) fn into_list(self) -> Vec<T> { self.list }
}

impl Distinct<String> {
    /// `value`, copied only the first time it is met.
    pub(crate) fn push(&mut self, value: &str) { if !self.contains(value) { self.insert(value.to_string()); } }
}

/// AccountTransaction.entry_types: a type's stored id.
pub fn entry_type(name: &str) -> Option<i64> {
    crate::web::tracker::row::TYPES.iter().position(|t| *t == name).map(|i| i as i64)
}

/// #710 7a rule N8: `import_row_origins`, the importer of each ledger row a file wrote.
pub const ORIGINS: &str = "import_row_origins";
/// The importer of a Binance export row the user imported past a warning, its retained id stripped (rules P6/Q3).
pub const BINANCE_ACCEPTED: &str = "binance_accepted";

/// Whether this install records origins (an install the migration has not reached, or a test schema, records none).
pub fn origins_ready(c: &rusqlite::Connection) -> rusqlite::Result<bool> { crate::engine::rules::table_exists(c, ORIGINS) }

/// Ledger row `row` was written by importer `kind`, in the insert's transaction; a row recorded already keeps its first.
pub fn record_origin(c: &rusqlite::Connection, row: i64, kind: &str, now: chrono::DateTime<chrono::Utc>) -> rusqlite::Result<()> {
    c.execute("INSERT INTO import_row_origins (transaction_id, import_kind, created_at) VALUES (?1, ?2, ?3) ON CONFLICT (transaction_id) DO NOTHING",
              rusqlite::params![row, kind, crate::sync::sql_time(now)]).map(|_| ())
}

/// #710 7a rules P9/Q5: before the engine starts after 2.x's window, every ledger row 2.x inserted since the handback
/// `back` whose raw data is one of its importer's shapes (`{}`, or `{operation}` alone; v2.70.21 import/run.rb,
/// deltabadger_csv.rb) is recorded as imported (`v2`). A missing venue id alone proves nothing: 2.x's own syncs write
/// some rows without one, with the venue's payload.
pub fn v2_origins(c: &rusqlite::Connection, back: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> rusqlite::Result<usize> {
    if !origins_ready(c)? { return Ok(0); }
    c.execute("INSERT INTO import_row_origins (transaction_id, import_kind, created_at)
               SELECT a.id, 'v2', ?2 FROM account_transactions a WHERE a.created_at > ?1 AND json_valid(a.raw_data) AND json_type(a.raw_data) = 'object'
                 AND NOT EXISTS (SELECT 1 FROM json_each(a.raw_data) WHERE key <> 'operation')
               ON CONFLICT (transaction_id) DO NOTHING", rusqlite::params![crate::sync::sql_time(back), crate::sync::sql_time(now)])
}

#[cfg(test)]
mod tests {
    /// B5: a collected value is found by its hash, not by comparing it with every one before it; the first-met order stays.
    #[test]
    fn distinct_values_keep_their_order_and_are_not_compared_with_every_other() {
        use std::{cell::Cell, hash::{Hash, Hasher}};
        thread_local!(static EQ: Cell<usize> = const { Cell::new(0) });
        #[derive(Clone, Debug)] struct K(usize);
        impl PartialEq for K { fn eq(&self, o: &Self) -> bool { EQ.with(|n| n.set(n.get() + 1)); self.0 == o.0 } }
        impl Eq for K {}
        impl Hash for K { fn hash<H: Hasher>(&self, h: &mut H) { self.0.hash(h) } }
        let mut d = super::Distinct::default();
        for n in (0..2000).chain(0..2000).rev() { d.insert(K(n)); }
        let list = d.into_list();
        assert_eq!((list.len(), list[0].0, list[1999].0), (2000, 1999, 0), "first-met order");
        assert!(EQ.with(Cell::get) < 10_000, "{} comparisons", EQ.with(Cell::get));
    }
}
