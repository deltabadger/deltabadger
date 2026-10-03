//! Who an `Authorization: Bearer <token>` header speaks for: app/lib/oauth_bearer_token_resolver.rb
//! on Doorkeeper's `oauth_access_tokens`. The MCP endpoint calls `authenticate` with scope `mcp`.
//!
//! The header is read by hand. Rails' resolver uses `/\ABearer\s+(.+)\z/i`, which CodeQL flags
//! (rb/polynomial-redos): `\s+` and `.+` can both take the same spaces. Here the scheme, the
//! whitespace and the token are taken in one pass each.
use super::oauth::{scopes, AccessToken};
use super::WebError;
use crate::engine::Clock;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

/// Why a header names nobody. The order of the checks is the resolver's: a token both revoked and
/// expired is `Revoked`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No header, another scheme, or no token after `Bearer`.
    Missing,
    /// No such token.
    Invalid,
    Revoked,
    Expired,
    /// The token does not carry the scope the endpoint needs.
    InsufficientScope,
    /// The user the token was issued for no longer exists.
    UserNotFound,
}

impl Refusal {
    /// The resolver's symbol for this refusal.
    pub fn name(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::InsufficientScope => "insufficient_scope",
            Self::UserNotFound => "user_not_found",
        }
    }
}

/// Who a valid token speaks for, and through which client. The caller loads what it needs of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bearer {
    pub user_id: i64,
    /// `oauth_applications.id`: with the user, the key of the client's grant in `connected_clients`.
    pub application_id: i64,
    pub token_id: i64,
}

/// Ruby's `\s`: space, tab, line feed, vertical tab, form feed, carriage return.
fn ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
}

/// The token in an `Authorization` header: `Bearer` in any case, at least one whitespace character,
/// then the rest with the whitespace around it removed. A value with a line break has no token: no
/// HTTP header can carry one.
pub fn bearer_token(header: Option<&str>) -> Option<&str> {
    let header = header?;
    let rest = header.get(..6).filter(|scheme| scheme.eq_ignore_ascii_case("bearer")).map(|_| &header[6..])?;
    if !rest.starts_with(ruby_space) || header.contains(['\n', '\r']) {
        return None;
    }
    // String#strip also removes NUL at the end.
    Some(rest.trim_matches(|c: char| ruby_space(c) || c == '\0')).filter(|token| !token.is_empty())
}

fn find(c: &Connection, header: Option<&str>) -> Result<Result<AccessToken, Refusal>, WebError> {
    let Some(token) = bearer_token(header) else { return Ok(Err(Refusal::Missing)) };
    Ok(AccessToken::find(c, "token", token)?.ok_or(Refusal::Invalid))
}

fn check(c: &Connection, token: &AccessToken, required_scope: &str, now: DateTime<Utc>) -> Result<Result<Bearer, Refusal>, WebError> {
    if token.revoked(now) { return Ok(Err(Refusal::Revoked)); }
    if token.expired(now) { return Ok(Err(Refusal::Expired)); }
    if !scopes(&token.scopes).contains(&required_scope) { return Ok(Err(Refusal::InsufficientScope)); }
    let user: Option<i64> = match token.resource_owner_id {
        Some(id) => c.query_row("SELECT id FROM users WHERE id = ?1", [id], |r| r.get(0)).optional()?,
        None => None,
    };
    Ok(user.map(|user_id| Bearer { user_id, application_id: token.application_id, token_id: token.id }).ok_or(Refusal::UserNotFound))
}

/// OauthBearerTokenResolver.call: a read. The token is looked up by its stored (plain) value; the
/// comparison is the index's, as in Rails. Called inside `App::db`; the time the row is judged by
/// is read here, with the database in hand, not when the request arrived.
pub fn resolve(c: &Connection, header: Option<&str>, required_scope: &str, clock: &dyn Clock) -> Result<Result<Bearer, Refusal>, WebError> {
    match find(c, header)? {
        Ok(token) => check(c, &token, required_scope, clock.now()),
        Err(refusal) => Ok(Err(refusal)),
    }
}

/// How many rows one presentation of an access token walks back at most. A longer chain is
/// finished by the presentations that follow, so no request holds the write lock for long.
pub const RETIRED_AT_ONCE: usize = 100;

/// Retires the refresh tokens the presented token descends from: every row reached by following
/// `previous_refresh_token` back from it, `RETIRED_AT_ONCE` rows per presentation. A row that is
/// not revoked is revoked (one conditional write: a row that is revoked keeps its time); a row that
/// is already revoked is passed through, because what is behind it may not be: two tokens refreshed
/// from the same one share their ancestors, and the second must not stop where the first has been.
///
/// The ancestors' own `previous_refresh_token` values are left as they are. The place reached is
/// kept on the presented row alone: its `previous_refresh_token` becomes the refresh token of the
/// next row to look at, or `''` when the chain has ended, and the next presentation goes on from
/// there. That is a value Doorkeeper itself would follow (Doorkeeper::OAuth::Token.authenticate
/// revokes the row it names and empties it), and nothing else in Doorkeeper or the app reads the
/// column, so a revoked row that still names its predecessor is read by Rails as any revoked row.
///
/// Stored links are data: a row seen before in this walk (a loop, or a row that names itself or
/// the presented one) ends the chain, and the presented row is never revoked by its own walk. A loop
/// longer than one walk is ended across presentations: see the place kept at the limit, below.
/// ponytail: a token that descends from a long chain walks all of it once, 100 rows a presentation,
/// even where every row is revoked already. Deleting retired rows (not in this build) is what ends that.
fn retire_ancestors(c: &Connection, presented: &AccessToken, now: DateTime<Utc>) -> Result<(), WebError> {
    let mut next = presented.previous_refresh_token.clone();
    let mut seen = vec![presented.id];
    for _ in 0..RETIRED_AT_ONCE {
        if next.trim().is_empty() { break; }
        // By the unique index on `refresh_token`.
        let Some(ancestor) = AccessToken::find(c, "refresh_token", &next)?.filter(|ancestor| !seen.contains(&ancestor.id)) else { next.clear(); break };
        seen.push(ancestor.id);
        if ancestor.revoked_at.is_none() { ancestor.revoke(c, now)?; }
        next = ancestor.previous_refresh_token;
    }
    // The walk stopped at the limit. The place kept for the next presentation must name a row older
    // (a smaller id) than every row this walk passed: issuance only ever links a row to an older one,
    // so a chain that was issued always passes, and the oldest row reached falls with every
    // presentation. A malformed chain fails it (a loop longer than one walk, which would otherwise be
    // walked a hundred rows at a time for ever) and is ended here.
    if !next.trim().is_empty() {
        let oldest = seen.iter().copied().fold(presented.id, i64::min);
        if AccessToken::find(c, "refresh_token", &next)?.is_none_or(|row| row.id >= oldest) { next.clear(); }
    }
    if next != presented.previous_refresh_token {
        c.execute("UPDATE oauth_access_tokens SET previous_refresh_token = ?1 WHERE id = ?2", (next, presented.id))?;
    }
    Ok(())
}

/// `resolve` for an endpoint: the same answer, and a token that came from a refresh retires every
/// refresh token before it (`retire_ancestors`) from the first time it is presented. Presenting it
/// shows that the client received the new pair; until then the refresh token it came from still
/// works, so a refresh whose response was lost can be repeated: the newest refresh token is never
/// among the ancestors of a token that was presented.
///
/// Doorkeeper means the same for the one token before (Doorkeeper::OAuth::Token.authenticate), and
/// Rails never makes even that call: its resolver looks the token up directly, so under Rails every
/// refresh token ever issued stays valid until the client is revoked. This goes further than
/// Doorkeeper on purpose: an install that comes from Rails has every old refresh token still live.
///
/// Called inside `App::db`. The time is read from `clock` after the write lock is held.
pub fn authenticate(c: &Connection, header: Option<&str>, required_scope: &str, clock: &dyn Clock) -> Result<Result<Bearer, Refusal>, WebError> {
    let token = match find(c, header)? {
        Ok(token) => token,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let now = if token.previous_refresh_token.trim().is_empty() {
        clock.now()
    } else {
        super::oauth::transaction(c, clock, |c, now| retire_ancestors(c, &token, now).map(|()| now))?
    };
    check(c, &token, required_scope, now)
}
