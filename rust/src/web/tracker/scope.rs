//! #710 slice 5 (PLAN-710-5 §5.6): the tracker's scope parameter, read once for every page and download that takes it.
//! - `account_id=<id>` names an account; a foreign or unknown one is a 404 (as a foreign bot id).
//! - `exchange_id=<id>` alone names that exchange's number 1, so old links keep working; an unknown exchange is a 404,
//!   and a known one where the user has no account reads nothing (`Scope::EmptyLegacyExchange`), never "All".
//! - neither: the whole portfolio.
//!
//! A number 1 keeps emitting `exchange_id=<exchange>` (with one account per exchange every link is as before); an
//! account 2+ emits `account_id=<id>`.
use super::{layout, Ctx, WebError};
use crate::identity::AccountRow;
use crate::tracker::Scope;
use axum::response::Response;
use rusqlite::Connection;

/// The scope `ctx`'s query names for `owner`, or the 404 it answers.
pub fn read(c: &Connection, ctx: &Ctx, owner: i64) -> Result<Result<Scope, Response>, WebError> {
    let present = |key: &str| ctx.params.query(key).filter(|s| !s.trim().is_empty());
    if let Some(raw) = present("account_id") {
        let account = crate::web::bot::id_from_path(raw).map(|id| AccountRow::by_id(c, id)).transpose()?.flatten();
        return Ok(match account { Some(a) if a.user_id == owner => Ok(Scope::Account(a)), _ => Err(layout::missing()) });
    }
    let Some(raw) = present("exchange_id") else { return Ok(Ok(Scope::All)) };
    let Some(id) = crate::web::bot::id_from_path(raw) else { return Ok(Err(layout::missing())) };
    if !c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)", [id], |r| r.get::<_, bool>(0))? { return Ok(Err(layout::missing())); }
    Ok(Ok(Scope::of_exchange(c, owner, Some(id))?))
}

/// The query pair a link to account `a`'s scope carries.
pub fn pair(a: &AccountRow) -> (&'static str, String) {
    if a.number == 1 { ("exchange_id", a.exchange_id.to_string()) } else { ("account_id", a.id.to_string()) }
}

/// The scope's hidden field of a page's date form, as the request named it (`account_id` before `exchange_id`, as `read`).
pub fn hidden(ctx: &Ctx) -> Option<(&'static str, String)> {
    ["account_id", "exchange_id"].into_iter().find_map(|key| ctx.params.query(key).filter(|s| !s.trim().is_empty()).map(|v| (key, crate::web::i18n::escape(v))))
}

/// The query pairs a page passes on (its scope and window), as the request named them.
pub fn passed(ctx: &Ctx) -> Vec<(String, String)> {
    ctx.params.query.iter().filter(|(key, _)| matches!(key.as_str(), "exchange_id" | "account_id" | "from" | "to")).cloned().collect()
}
