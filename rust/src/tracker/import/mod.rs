//! The tracker's CSV import (Import::Run and its readers): `csv` the text, rows, zones, times and amounts; `binance`
//! Binance's own export; `deltabadger` the app's own export read back.
pub mod binance;
pub mod csv;
pub mod deltabadger;

/// AccountTransaction.entry_types: a type's stored id.
pub fn entry_type(name: &str) -> Option<i64> {
    crate::web::tracker::row::TYPES.iter().position(|t| *t == name).map(|i| i as i64)
}
