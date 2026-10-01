//! Replaced in Task 8.
use super::EngineError;
pub fn apply_in(_c: &rusqlite::Connection, _bot_id: i64, _tx_id: i64, _s: &crate::venue::OrderState, _m: bool, _n: chrono::DateTime<chrono::Utc>) -> Result<(), EngineError> { Ok(()) }
