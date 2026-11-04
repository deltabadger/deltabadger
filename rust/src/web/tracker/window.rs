//! `filtered_transactions`' date window (TrackerController, PLAN-A19): one reader for the page's transactions table, the
//! first-sync page and the CSV export. Named divergences:
//! - D-TRACKER-FILTER-USER-DAYS: `from`/`to` are days in the user's zone, the zone the rows are printed in (Rails cuts
//!   UTC days, so a row printed on the 4th could sit inside `to=…03`).
//! - D-TRACKER-FILTER-ISO-ONLY: `YYYY-MM-DD` only, what the date control and the app's own links send (Rails'
//!   `Date.parse` also reads `Nov 3`, `20261103`, and `11/03/2026` as 11 March); every other text is a 400.
//! - D-TRACKER-FILTER-BAD-INPUT: an unparseable or non-existent day, or a structured key (`from[]`, `exchange_id[k]`, …),
//!   where Rails raises (500), is a 400.
use super::{first_sync, Ctx};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use rusqlite::types::Value as Sql;

/// The request asked for a window this build does not read: answered with a 400.
pub struct Bad;

/// The SQL bounds, each absent when its parameter is absent or blank: a default the controls display is never a bound.
pub struct Window { from: Option<String>, to: Option<String> }

/// The zone the user's days are read in: the one their rows are printed in (`timezone::local`).
pub fn zone(ctx: &Ctx) -> Tz {
    ctx.user().and_then(|u| crate::web::timezone::strict(&u.time_zone)).unwrap_or(Tz::UTC)
}

/// A day the window may name: an ISO day this build reads (`first_sync::iso_day`), nothing else.
fn day(raw: &str) -> Result<NaiveDate, Bad> {
    match first_sync::iso_day(raw) { Ok(Some(day)) => Ok(day), _ => Err(Bad) }
}

/// Rails' `params[:from].present?` (the last value of a repeated key, as `Params::query` reads it).
pub fn present<'a>(ctx: &'a Ctx, key: &str) -> Option<&'a str> {
    ctx.params.query(key).filter(|v| !crate::ruby::blank(v))
}

/// `Exchange.find(params[:exchange_id])` on a structured value, which runs before any date is read: a hash finds nothing
/// (RecordNotFound, 404, as Rails); an array finds its ids (404 when one is missing), and then Rails fails on
/// `for_exchange(array)` (500; D-TRACKER-FILTER-BAD-INPUT: 400). `None` when the venue is a plain value or absent.
pub fn structured_exchange(c: &rusqlite::Connection, ctx: &Ctx) -> Result<Option<axum::response::Response>, crate::web::WebError> {
    let keys: Vec<&(String, String)> = ctx.params.query.iter().filter(|(k, _)| k.starts_with("exchange_id[")).collect();
    if keys.is_empty() { return Ok(None); }
    if keys.iter().any(|(k, _)| k != "exchange_id[]") { return Ok(Some(crate::web::layout::missing())); }
    for (_, raw) in keys {
        let Some(id) = crate::web::bot::id_from_path(raw) else { return Ok(Some(crate::web::layout::missing())) };
        if !c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)", [id], |r| r.get::<_, bool>(0))? { return Ok(Some(crate::web::layout::missing())); }
    }
    Ok(Some(crate::web::layout::bad_request()))
}

/// The structured keys Rails crashes on, and each present day.
pub fn read(ctx: &Ctx, zone: Tz) -> Result<Window, Bad> {
    if ctx.params.query.iter().any(|(k, _)| k.starts_with("exchange_id[") || k.starts_with("from[") || k.starts_with("to[")) { return Err(Bad); }
    let bound = |key: &str, clock: (u32, u32, u32, u32)| -> Result<Option<String>, Bad> {
        let Some(raw) = present(ctx, key) else { return Ok(None) };
        let naive = day(raw)?.and_hms_micro_opt(clock.0, clock.1, clock.2, clock.3).ok_or(Bad)?;
        // `quoted_date`: no fraction when it is zero, so a row stored at the midnight compares equal to it.
        Ok(Some(crate::codec::format_time(crate::web::timezone::wall(zone, naive).ok_or(Bad)?)))
    };
    Ok(Window { from: bound("from", (0, 0, 0, 0))?, to: bound("to", (23, 59, 59, 999_999))? })
}

impl Window {
    /// `in_date_range`: the clauses after a WHERE, their binds numbered after `binds`'.
    pub fn push(&self, sql: &mut String, binds: &mut Vec<Sql>) {
        for (bound, op) in [(&self.from, ">="), (&self.to, "<=")] {
            let Some(bound) = bound else { continue };
            binds.push(Sql::Text(bound.clone()));
            sql.push_str(&format!(" AND transacted_at{op}?{}", binds.len()));
        }
    }
}

/// The date controls' values: the parameter as sent, else the first row's day and today, both in `zone`.
pub fn defaults(ctx: &Ctx, zone: Tz, first: Option<DateTime<Utc>>) -> (String, String) {
    let from = present(ctx, "from").map_or_else(|| first.map(|t| t.with_timezone(&zone).date_naive().to_string()).unwrap_or_default(), str::to_string);
    let to = present(ctx, "to").map_or_else(|| ctx.app.now().with_timezone(&zone).date_naive().to_string(), str::to_string);
    (from, to)
}
