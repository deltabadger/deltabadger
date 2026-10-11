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

/// User::Password::PATTERN as Ruby's `=~` reads it: lookaheads from some position for a lowercase letter, an
/// uppercase letter and a digit (ASCII classes), a symbol (`[\W_]`: anything but an ASCII letter or digit), and 8
/// characters. `.` stops at a line feed, so all of it must be on one line, but `[\W_]` also matches the line feed
/// itself: a line that a line feed ends has its symbol.
pub(crate) fn password_complexity(password: &str) -> bool {
    let lines: Vec<&str> = password.split('\n').collect();
    lines.iter().enumerate().any(|(index, line)| {
        line.chars().count() >= 8 && line.chars().any(|c| c.is_ascii_lowercase()) && line.chars().any(|c| c.is_ascii_uppercase())
            && line.chars().any(|c| c.is_ascii_digit()) && (index + 1 < lines.len() || line.chars().any(|c| !c.is_ascii_alphanumeric()))
    })
}
