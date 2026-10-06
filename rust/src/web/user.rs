//! User model validation for saves of existing users' preferences.
use super::{locale, timezone, WebError};
use rusqlite::Connection;

/// Match User#valid? for a persisted user whose identity/password are unchanged.
/// Name and email format/uniqueness run only when changed; password validators
/// need an assigned password. Devise email presence and preference inclusions
/// run on every save, even if the assigned preferences themselves are unchanged.
/// Stored values are not assignment-normalized by Rails when loading a record.
pub(super) fn validate_save(c: &Connection, owner: i64) -> Result<bool, WebError> {
    let (email, zone, language, currency, jurisdiction) = c.query_row(
        "SELECT email,time_zone,locale,display_currency,wash_sale_jurisdiction FROM users WHERE id=?1",
        [owner], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?,
            r.get::<_,Option<String>>(2)?, r.get::<_,String>(3)?, r.get::<_,Option<String>>(4)?)),
    )?;
    Ok(!email.trim().is_empty()
        && timezone::zone(&zone).is_some()
        && language.as_deref().is_none_or(|s| locale::known(s).is_some())
        && matches!(currency.as_str(), "USD" | "EUR" | "GBP" | "CHF" | "PLN")
        // User#wash_sale_jurisdiction reads blank as the first option, US.
        && jurisdiction.as_deref().is_none_or(|s| s.trim().is_empty() || matches!(s, "US" | "GB" | "IE")))
}
