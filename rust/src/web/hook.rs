//! HooksController: `POST /hook/:token`, the address a signal bot's rule is called at (plan 8b). Outside the
//! session, CSRF and locale pipeline, as ActionController::API is. The token is the whole authentication, so this is the
//! one unauthenticated path that moves money and it does as little as Rails does: one lookup, one claim, one call. The
//! body is never the trade; `entry` reads it (bounded, so a stalled body spends no claim) only as Rack would, for
//! Rack::MethodOverride and for the parse ActionDispatch refuses with a 400.
//!
//! Where Rails enqueues Bot::SignalJob, this build appends the call to `bots.transient_data.rust_signal_calls` in the
//! same SQLite transaction as the claim (engine::signal consumes it), through `eligibility::guard`. Rails' window where
//! the claim and the enqueue diverge does not exist here: a call that is not kept rolls its claim back, and the answer
//! is the 503 Rails gives when its enqueue fails.
//!
//! The token is a bearer secret: it is in no log line this module writes (`mask_hook_path` is the shared mask).
use super::rest::{self, Answer};
use super::{layout, App, Params, WebError};
use crate::engine::{eligibility, model};
use crate::enums::{BotStatus, BOT_WORKING};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

/// BotSignal::TRIGGER_COOLDOWN.
pub const COOLDOWN_SECONDS: i64 = 30;

pub fn router() -> Router<App> {
    Router::new().route("/hook", any(handle)).route("/hook/{*rest}", any(handle))
}

/// Set by `entry` on a POST to /hook whose body or query Rack cannot parse (an invalid %-escape, invalid UTF-8, a
/// parameter too deep or of two shapes, a broken multipart body). ActionDispatch refuses such a request with a 400 once a
/// controller is reached, so the hook answers it before the claim.
#[derive(Clone, Copy)]
pub struct Unparseable;

/// HookTokenFilter (config/initializers/filter_parameter_logging.rb): a path as a log line may print it.
pub fn mask_hook_path(path: &str) -> std::borrow::Cow<'_, str> {
    let Some(rest) = path.strip_prefix("/hook/") else { return path.into() };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    if end == 0 { return path.into(); }
    format!("/hook/[FILTERED]{}", &rest[end..]).into()
}

/// `post '/hook/:token'`, matched as Journey matches it on the path past /hook/: one segment, a token with no `.`, and
/// an optional `.<format>` with none either. The token is the segment unescaped; `Some(None)` is a token that is no
/// UTF-8 once unescaped (the router's 400).
fn token(route_path: &str) -> Option<Option<String>> {
    let segment = route_path.strip_prefix("/hook/")?;
    if segment.contains('/') { return None; }
    let token = match segment.split_once('.') {
        Some((_, format)) if format.is_empty() || format.contains('.') => return None,
        Some((token, _)) => token,
        None => segment,
    };
    if token.is_empty() { return None; }
    Some(rest::unescape(token))
}

/// What the hook answers, as HooksController#create decides it.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// No rule has the token, or its bot is deleted: 404.
    NotFound,
    /// `{"status":"ignored","reason":…}`: the claim was lost (cooldown), or won on a bot that is not working or a rule
    /// that is off (the claim is then spent, as in Rails).
    Ignored(&'static str),
    /// The claim and the call committed together: 202.
    Accepted,
    /// The call cannot be kept here (the engine guard refused it): nothing was written, 503.
    Refused(String),
}

/// Active Record's SQLite datetime text. BotSignal#claim_trigger! compares it as text, so the claim must write and
/// compare the same text across the cutover.
fn db_time(at: DateTime<Utc>) -> String { at.format("%Y-%m-%d %H:%M:%S%.6f").to_string() }

/// One call, in one immediate transaction: the lookup, BotSignal#claim_trigger!'s conditional UPDATE, #ignore_reason, and
/// the call appended for the engine. `keep` judges the write before it commits (the web passes `eligibility::guard`); a
/// refusal rolls the claim back with it.
/// `now` is read once the write lock is held, as BotSignal#claim_trigger! reads Time.current after the lookup: a call that
/// waited for the lock is judged, and stamped, at the time it is claimed.
pub fn receive(c: &Connection, token: &str, now: impl FnOnce() -> DateTime<Utc>, keep: impl FnOnce(&Connection, i64) -> Result<(), String>) -> Result<Outcome, WebError> {
    let tx = model::immediate(c).map_err(|e| WebError::Config(format!("{e:?}")))?;
    let now = now();
    let found: Option<(i64, i64, i64, Option<bool>)> = tx.query_row(
        "SELECT s.id, s.bot_id, b.status, s.enabled FROM bot_signals s JOIN bots b ON b.id = s.bot_id WHERE s.token = ?1",
        [token], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let Some((signal_id, bot_id, status, enabled)) = found.filter(|f| f.2 != BotStatus::Deleted as i64) else { return Ok(Outcome::NotFound) };
    let claimed = tx.execute("UPDATE bot_signals SET last_triggered_at = ?1 WHERE id = ?2 AND (last_triggered_at IS NULL OR last_triggered_at <= ?3)",
                             params![db_time(now), signal_id, db_time(now - Duration::seconds(COOLDOWN_SECONDS))])? == 1;
    let outcome = if !claimed { Outcome::Ignored("cooldown") }
        else if !BOT_WORKING.iter().any(|s| *s as i64 == status) { Outcome::Ignored("bot_not_running") }
        else if enabled != Some(true) { Outcome::Ignored("signal_disabled") }
        else {
            let call = model::SignalCall { source: format!("hook:{}", uuid::Uuid::new_v4()), signal_id, triggered_at: now };
            model::append_signal_call(&tx, bot_id, &call).map_err(|e| WebError::Config(format!("{e:?}")))?;
            if let Err(reason) = keep(&tx, bot_id) { return Ok(Outcome::Refused(reason)); }
            Outcome::Accepted
        };
    tx.commit()?;
    Ok(outcome)
}

pub async fn handle(State(app): State<App>, request: Request) -> Response {
    let Some(params) = request.extensions().get::<Arc<Params>>().cloned() else {
        return WebError::Config("a request reached the routes without passing web::router's entry".into()).into_response();
    };
    let head = request.method() == Method::HEAD;
    // Only POST is routed, and nothing under a locale prefix: anything else is the router's 404.
    let found = (request.method() == Method::POST && params.path_locale.is_none()).then(|| token(&params.route_path)).flatten();
    let Some(token) = found else { return rest::finish(&app, Answer::Below(Box::new(layout::missing())), head, None) };
    let Some(token) = token else { return rest::router_bad_request(head) };
    if request.extensions().get::<Unparseable>().is_some() { return rest::router_bad_request(head); }
    let worker = app.clone();
    let outcome = app.db(move |c| {
        let outcome = receive(c, &token, || worker.now(), |tx, bot_id| eligibility::guard(tx, &worker.cipher, Some(bot_id)).map_err(|r| r.reason()))?;
        // After the commit, inside the job: a dropped request cannot lose the wake.
        if outcome == Outcome::Accepted { worker.wake_engine(); }
        Ok(outcome)
    }).await;
    let answer = match outcome {
        Ok(Outcome::NotFound) => Answer::Head(StatusCode::NOT_FOUND),
        Ok(Outcome::Ignored(reason)) => Answer::Json(StatusCode::OK, format!(r#"{{"status":"ignored","reason":"{reason}"}}"#)),
        Ok(Outcome::Accepted) => Answer::Json(StatusCode::ACCEPTED, r#"{"status":"accepted"}"#.into()),
        Ok(Outcome::Refused(reason)) => {
            eprintln!("deltabadger: a signal call was not kept: {reason}");
            Answer::Head(StatusCode::SERVICE_UNAVAILABLE)
        }
        Err(error) => {
            eprintln!("deltabadger: a signal call failed: {error:?}");
            Answer::Head(StatusCode::SERVICE_UNAVAILABLE)
        }
    };
    rest::finish(&app, answer, head, None)
}

/// Rack::QueryParser's reading of a form or query string, as far as it can fail: each `&`-separated pair unescaped
/// (URI.decode_www_form_component: a `%` must be two hex digits) to UTF-8 (ActionDispatch's encoding check).
pub fn rack_form_ok(encoded: &[u8]) -> bool {
    encoded.split(|b| *b == b'&').all(|pair| {
        let mut out = Vec::with_capacity(pair.len());
        let mut i = 0;
        while i < pair.len() {
            match pair[i] {
                b'%' => {
                    let hex = |at: usize| pair.get(at).and_then(|b| (*b as char).to_digit(16));
                    let (Some(high), Some(low)) = (hex(i + 1), hex(i + 2)) else { return false };
                    out.push((high * 16 + low) as u8);
                    i += 3;
                }
                b'+' => { out.push(b' '); i += 1; }
                b => { out.push(b); i += 1; }
            }
        }
        std::str::from_utf8(&out).is_ok()
    })
}

/// What Rack makes of a POST body to /hook: the fields Rack::MethodOverride reads (a form: urlencoded, or no type at all;
/// or multipart/form-data), and whether it parses. Any other type is not read by Rack.
pub fn body(bytes: &[u8], content_type: &str) -> (Vec<(String, String)>, bool) {
    let media = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match media.as_str() {
        "" | "application/x-www-form-urlencoded" => {
            let pairs: Vec<(String, String)> = form_urlencoded::parse(bytes).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            let ok = rack_form_ok(bytes) && super::bot::action_params::form(&pairs).is_ok();
            (pairs, ok)
        }
        "multipart/form-data" => match super::multipart::boundary(content_type) {
            Some(boundary) => match super::multipart::parse(bytes, &boundary) {
                Ok(parsed) => { let ok = super::bot::action_params::form(&parsed.fields).is_ok(); (parsed.fields, ok) }
                Err(_) => (vec![], false),
            },
            None => (vec![], true),
        },
        _ => (vec![], true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_matched_as_journey_matches_them() {
        assert_eq!(token("/hook/abc"), Some(Some("abc".into())));
        assert_eq!(token("/hook/abc.json"), Some(Some("abc".into())));
        assert_eq!(token("/hook/a%2Db"), Some(Some("a-b".into())));
        assert_eq!(token("/hook/%FF"), Some(None));
        for path in ["/hook", "/hook/", "/hook/a.b.c", "/hook/a.", "/hook/.json", "/hook/a/b", "/hooks/a"] { assert_eq!(token(path), None, "{path}"); }
    }

    #[test]
    fn the_token_is_masked() {
        assert_eq!(mask_hook_path("/hook/secret"), "/hook/[FILTERED]");
        assert_eq!(mask_hook_path("/hook/secret?x=1"), "/hook/[FILTERED]?x=1");
        assert_eq!(mask_hook_path("/hook/secret/x"), "/hook/[FILTERED]/x");
        assert_eq!(mask_hook_path("/login"), "/login");
    }

    #[test]
    fn forms_fail_where_rack_fails() {
        for ok in [&b""[..], b"a=1&b=two+words", b"a=%41", b"_method=delete"] { assert!(rack_form_ok(ok), "{ok:?}"); }
        for bad in [&b"a=%zz"[..], b"a=%4", b"%", b"a=%FF"] { assert!(!rack_form_ok(bad), "{bad:?}"); }
        assert!(!body(b"a=1&a[b]=2", "application/x-www-form-urlencoded").1);
        assert!(!body(format!("a{}=1", "[b]".repeat(40)).as_bytes(), "").1);
        assert_eq!(body(b"_method=delete", "text/plain"), (vec![], true));
        assert_eq!(body(b"_method=delete", "").0, vec![("_method".to_string(), "delete".to_string())]);
    }

    #[test]
    fn the_claim_compares_active_records_text() {
        let at: DateTime<Utc> = "2026-09-10T12:00:30.123456789Z".parse().unwrap();
        assert_eq!(db_time(at), "2026-09-10 12:00:30.123456");
        assert_eq!(db_time("2026-09-10T11:00:00.5Z".parse().unwrap()), "2026-09-10 11:00:00.500000");
    }
}
