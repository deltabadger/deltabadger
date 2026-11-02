//! Password reset: Users::PasswordsController on Devise 5 (recoverable, paranoid, reset_password_within 6 hours,
//! sign_in_after_reset_password false), on the same `users` columns and the same token digest (crypto::ResetTokens), so
//! a link Rails mailed before the takeover works here.
//!
//! The new password, in Rails' order: the token (found, in its period); on a two-factor account an expired lock is lifted,
//! a locked account is refused with the lock's message, a blank code is refused; the new password (blank, policy,
//! confirmation) is checked before the code is spent; a wrong code counts toward the lock /login shares; a right one
//! resets that counter. A refused attempt never changes the password, the token, its time or the last spent code: the
//! only writes on a refusal are Rails' own (the lifted lock, the counted wrong code). The bcrypt computation runs
//! outside the database lock, and the write is decided again on the row as it is then, so one token wins once.
//!
//! Named divergences (PLAN-A16):
//! - D-RESET-NOTICE-COPY: the request's notice is Devise's reset copy, not the confirmation copy Rails shows.
//! - D-RESET-FORM-KEEPS-FIELDS: a refused password is shown on the whole form (Rails drops the code field, the patterns
//!   and the email when Devise re-renders it).
//! - D-RESET-UPDATE-STALE-TOKEN: an unknown or expired token on submit goes back to the request page with the expiry
//!   message, as the link itself does (Rails re-renders the form with an error it never shows).
//! - D-ACCOUNT-MAIL-OFF-REQUEST (settings::mail): the answer never waits for or depends on the mail.
use super::auth::{self, User};
use super::layout::{self, Ctx, Page};
use super::registration::{registration_open, Links};
use super::settings::mail::{self, AccountMail};
use super::setup;
use super::{flash, App, WebError};
use crate::codec::{format_time, parse_time};
use crate::engine::EngineError;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

/// Devise.reset_password_within.
pub const RESET_PASSWORD_WITHIN_SECONDS: i64 = 6 * 60 * 60;

#[derive(Template)]
#[template(path = "passwords/new.html")]
struct NewView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    links: Links,
}

#[derive(Template)]
#[template(path = "passwords/edit.html")]
struct EditView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    links: Links,
    token: &'a str,
    email: &'a str,
    two_factor: bool,
    password_error: Option<String>,
    confirmation_error: Option<String>,
    code_error: Option<String>,
}

/// GET /password/new.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let csrf = ctx.csrf_token();
    let body = NewView { v: &ctx, csrf: &csrf, links: Links { login: true, signup: registration_open(&app).await? } }.render()?;
    layout::devise(&ctx, &csrf, Page { status: StatusCode::OK, body, flash_now: Vec::new() })
}

/// POST /password: paranoid, the same answer whether the address has an account or not.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let email = setup::normalized_email(ctx.params.form("user[email]").unwrap_or(""));
    let (inner, now) = (app.clone(), ctx.now);
    // The write lock is taken for every address, known or not: under a busy database both wait alike.
    let issued = app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        // D-NO-MAIL-SAYS-SO: no mail can leave this install, so no token is written (one mailed earlier keeps working).
        if !mail::set_up(&inner, &tx)? { return Ok(Err(())) }
        let id = if email.is_empty() { None } else { tx.query_row("SELECT id FROM users WHERE email = ?1", [&email], |r| r.get::<_, i64>(0)).optional()? };
        let issued = match id {
            Some(id) => {
                // set_reset_password_token: a new token each time, which ends the one before.
                let (raw, digest) = inner.reset_tokens.generate();
                tx.execute("UPDATE users SET reset_password_token = ?1, reset_password_sent_at = ?2, updated_at = ?2 WHERE id = ?3", (&digest, format_time(now), id))?;
                Some((id, raw))
            }
            None => None,
        };
        tx.commit()?;
        Ok(Ok(issued))
    }).await?;
    let Ok(issued) = issued else {
        flash::set(&ctx.session, flash::ALERT, super::i18n::text(ctx.locale, "devise.passwords.mail_not_set_up", &[]));
        return Ok(layout::redirect(StatusCode::SEE_OTHER, &ctx.path("/password/new")));
    };
    if let Some((user_id, token)) = issued {
        mail::send(&app, AccountMail::ResetPassword { user_id, token }, ctx.locale).await;
    }
    flash::set(&ctx.session, flash::NOTICE, super::i18n::text(ctx.locale, "devise.passwords.send_paranoid_instructions", &[]));
    Ok(layout::redirect(StatusCode::SEE_OTHER, &ctx.path("/login")))
}

/// The account a raw token opens: found by its digest, and sent within `reset_password_within` (inclusive).
fn holder(app: &App, c: &Connection, raw: &str, now: DateTime<Utc>) -> Result<Option<User>, WebError> {
    let Some(digest) = app.reset_tokens.digest(raw) else { return Ok(None) };
    let Some(user) = User::find_by_reset_digest(c, &digest)? else { return Ok(None) };
    let sent: Option<String> = c.query_row("SELECT reset_password_sent_at FROM users WHERE id = ?1", [user.id], |r| r.get(0))?;
    let sent = sent.map(|t| parse_time(&t).map_err(|e| WebError::Engine(EngineError::Data(format!("users.reset_password_sent_at: {e:?}"))))).transpose()?;
    Ok(sent.filter(|at| (now - *at).num_microseconds().is_some_and(|us| us <= RESET_PASSWORD_WITHIN_SECONDS * 1_000_000)).map(|_| user))
}

/// The link is dead: back to the request page, saying so.
fn expired(ctx: &Ctx, status: StatusCode) -> Response {
    flash::set(&ctx.session, flash::ALERT, super::i18n::text(ctx.locale, "devise.passwords.token_expired", &[]));
    layout::redirect(status, &ctx.path("/password/new"))
}

struct Refusal {
    password: Option<String>,
    confirmation: Option<String>,
    code: Option<String>,
}

async fn form(app: &App, ctx: &Ctx, status: StatusCode, token: &str, user: &User, refusal: Refusal) -> Result<Response, WebError> {
    let csrf = ctx.csrf_token();
    let body = EditView {
        v: ctx, csrf: &csrf, links: Links { login: true, signup: registration_open(app).await? }, token, email: &user.email,
        two_factor: user.otp_enabled, password_error: refusal.password, confirmation_error: refusal.confirmation, code_error: refusal.code,
    }.render()?;
    layout::devise(ctx, &csrf, Page { status, body, flash_now: Vec::new() })
}

/// GET /password/edit.
pub async fn edit(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let token = ctx.params.query("reset_password_token").unwrap_or("").to_string();
    if crate::ruby::blank(&token) { // assert_reset_token_passed
        flash::set(&ctx.session, flash::ALERT, super::i18n::text(ctx.locale, "devise.passwords.no_token", &[]));
        return Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/login")));
    }
    let (inner, raw, now) = (app.clone(), token.clone(), ctx.now);
    match app.db(move |c| holder(&inner, c, &raw, now)).await? {
        Some(user) => form(&app, &ctx, StatusCode::OK, &token, &user, Refusal { password: None, confirmation: None, code: None }).await,
        None => Ok(expired(&ctx, StatusCode::FOUND)), // ensure_valid_token
    }
}

/// Devise's validatable on the new password, as errors[:password] and errors[:password_confirmation] hold it.
fn password_refusal(locale: &str, password: &str, confirmation: Option<&str>) -> Option<Refusal> {
    let (mut on_password, mut on_confirmation) = (vec![], vec![]);
    if crate::ruby::blank(password) {
        on_password.push(setup::kind_message(locale, "password", "blank", None)); // Recoverable#reset_password
    } else {
        if confirmation.is_some_and(|c| c != password) { on_confirmation.push(setup::kind_message(locale, "password_confirmation", "confirmation", None)) }
        let length = password.chars().count() as i64;
        if length < 8 { on_password.push(setup::kind_message(locale, "password", "too_short", Some(8))) }
        if length > 128 { on_password.push(setup::kind_message(locale, "password", "too_long", Some(128))) }
        if !super::user::password_complexity(password) { on_password.push(super::i18n::text(locale, "errors.messages.too_simple_password", &[])) }
    }
    let joined = |messages: Vec<String>| (!messages.is_empty()).then(|| setup::upcase_first(&messages.join(", ")));
    let (password, confirmation) = (joined(on_password), joined(on_confirmation));
    (password.is_some() || confirmation.is_some()).then_some(Refusal { password, confirmation, code: None })
}

enum Decision {
    Stale,
    Refused(Box<User>, Refusal),
    /// Every check that needs no bcrypt passed: hash the password.
    Hash,
    Changed,
}

/// The two-factor gate, steps 2–4 (lift an expired lock; locked; blank code), on the row as it is now.
fn second_factor_gate(c: &Connection, ctx_locale: &str, user: &mut User, code: Option<&str>, now: DateTime<Utc>) -> Result<Option<Refusal>, WebError> {
    if !user.otp_enabled { return Ok(None) }
    if user.lock_expired(now) { user.unlock(c, now)? }
    let refuse = |key: &str| Some(Refusal { password: None, confirmation: None, code: Some(super::i18n::text(ctx_locale, key, &[])) });
    if user.locked(now) { return Ok(refuse("devise.failure.locked")) }
    if code.is_none_or(crate::ruby::blank) { return Ok(refuse("errors.messages.empty_two_fa_token")) }
    Ok(None)
}

/// PATCH or PUT /password.
pub async fn update(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let field = |name: &str| ctx.params.form(name).map(str::to_string);
    let token = field("user[reset_password_token]").unwrap_or_default();
    let password = field("user[password]").unwrap_or_default();
    let confirmation = field("user[password_confirmation]");
    let code = field("user[otp_code_token]");
    let locale = ctx.locale;
    // Everything but the bcrypt computation, under the lock. Writes only Rails' lifted lock.
    let first = {
        let (inner, token, password, confirmation, code) = (app.clone(), token.clone(), password.clone(), confirmation.clone(), code.clone());
        app.db(move |c| super::oauth::transaction(c, &*inner.clock, |c, now| {
            let Some(mut user) = holder(&inner, c, &token, now)? else { return Ok(Decision::Stale) };
            if let Some(refusal) = second_factor_gate(c, locale, &mut user, code.as_deref(), now)? { return Ok(Decision::Refused(Box::new(user), refusal)) }
            if let Some(refusal) = password_refusal(locale, &password, confirmation.as_deref()) { return Ok(Decision::Refused(Box::new(user), refusal)) }
            Ok(Decision::Hash)
        })).await?
    };
    match first {
        Decision::Stale => return Ok(expired(&ctx, StatusCode::SEE_OTHER)),
        Decision::Refused(user, refusal) => return form(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, &token, &user, refusal).await,
        Decision::Hash | Decision::Changed => {}
    }
    let hash = match app.password_hash(password).await? {
        None => return Ok(auth::busy()),
        Some(None) => return Err(WebError::Config("no random salt for a new password".into())),
        Some(Some(hash)) => hash,
    };
    // The decision again, on the row as it is now (a second submit of the token, two-factor switched on meanwhile).
    let inner = app.clone();
    let token_for_write = token.clone();
    let second = app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        let now = inner.now();
        let Some(mut user) = holder(&inner, &tx, &token_for_write, now)? else { return Ok(Decision::Stale) };
        if let Some(refusal) = second_factor_gate(&tx, locale, &mut user, code.as_deref(), now)? {
            tx.commit()?;
            return Ok(Decision::Refused(Box::new(user), refusal));
        }
        if user.otp_enabled {
            if !auth::verify_otp(&tx, &inner.cipher, &user, code.as_deref().unwrap_or(""), now)? {
                // register the failure on the counter /login shares; the fifth locks.
                user.increment_failed_attempts(&tx)?;
                if user.failed_attempts >= auth::MAXIMUM_ATTEMPTS { user.lock(&tx, now)? }
                tx.commit()?;
                let refusal = Refusal { password: None, confirmation: None, code: Some(super::i18n::text(locale, "errors.messages.bad_2fa_code", &[])) };
                return Ok(Decision::Refused(Box::new(user), refusal));
            }
            user.reset_failed_attempts(&tx, now)?;
        }
        tx.execute("UPDATE users SET encrypted_password = ?1, reset_password_token = NULL, reset_password_sent_at = NULL, updated_at = ?2 WHERE id = ?3",
                   (&hash, format_time(now), user.id))?;
        tx.commit()?;
        Ok(Decision::Changed)
    }).await?;
    match second {
        Decision::Stale => Ok(expired(&ctx, StatusCode::SEE_OTHER)),
        Decision::Refused(user, refusal) => form(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, &token, &user, refusal).await,
        Decision::Hash => Err(WebError::Config("reset decided twice".into())),
        Decision::Changed => {
            flash::set(&ctx.session, flash::NOTICE, super::i18n::text(ctx.locale, "devise.passwords.updated_not_active", &[]));
            Ok(layout::redirect(StatusCode::SEE_OTHER, &ctx.path("/login")))
        }
    }
}
