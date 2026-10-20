//! JSON field rules the venue adapters share (plan VB §1.3), each answering with the venue's own unreadable error
//! (`binance::unreadable`, `kucoin::unreadable`). A helper moves here only when a second venue needs it as it is.
use super::VenueError;
use crate::ruby::BigDec;
use serde_json::Value;

/// Utilities::Hash.dig_or_raise: absent or null is unreadable.
pub(crate) fn field<'a>(v: &'a Value, key: &str, unreadable: fn() -> VenueError) -> Result<&'a Value, VenueError> {
    v.get(key).filter(|x| !x.is_null()).ok_or_else(unreadable)
}

/// A decimal string or number: present and readable, or the whole answer is unreadable (never a zero). DIVERGES from Ruby
/// on purpose (ruby::json_to_d): Rails' `.to_d` reads "garbage" as 0.
pub(crate) fn required(v: &Value, unreadable: fn() -> VenueError) -> Result<BigDec, VenueError> {
    crate::ruby::json_to_d(v).ok().flatten().ok_or_else(unreadable)
}
