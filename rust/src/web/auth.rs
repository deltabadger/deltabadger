//! Sign-in, the second factor and sign-out, as Users::SessionsController, Devise 5 (paranoid mode,
//! :lockable, :confirmable) and Users::VerifyOtp behave, on the same `users` columns.
//! There is no remember-me here: the checkbox is rendered and ignored. Rails' cookie is always
//! `Secure`, so it never worked on a plain-http install, and a session lasts 30 days anyway.
use super::layout::{self, Ctx, Page};
use super::session::{Pending, SessionData};
use super::{flash, i18n, i18n::Arg, locale, App, WebError};
use crate::codec::{format_time, parse_time};
use crate::crypto::{totp_at, Cipher};
use crate::engine::EngineError;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Devise.maximum_attempts, Devise.unlock_in, and Users::SessionsController::PENDING_TTL (pinned by tests/web.rs).
pub const MAXIMUM_ATTEMPTS: i64 = 5;
pub const UNLOCK_IN_SECONDS: i64 = 15 * 60;
pub const PENDING_TTL_SECONDS: i64 = 5 * 60;
/// The longest path kept as `return_to`. The session is the cookie, and a browser drops a cookie
/// over 4096 bytes, the CSRF token in it included: the sign-in that follows would then be refused.
/// A path of this length makes a cookie of about 3 KB. (Rails raises CookieOverflow there.)
pub const MAX_RETURN_TO_BYTES: usize = 2048;

#[derive(Clone, Debug)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub encrypted_password: String,
    pub admin: bool,
    pub locale: Option<String>,
    pub time_zone: String,
    pub display_currency: String,
    pub hide_balances: bool,
    pub confirmed: bool,
    pub failed_attempts: i64,
    pub locked_at: Option<DateTime<Utc>>,
    pub otp_enabled: bool,
    /// As stored: an ActiveRecord::Encryption envelope.
    pub otp_secret_key: Option<String>,
    pub last_otp_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, email, encrypted_password, admin, locale, time_zone, display_currency, hide_balances, confirmed_at, \
                       failed_attempts, locked_at, otp_module, otp_secret_key, last_otp_at";

fn time(column: &str, text: Option<String>) -> Result<Option<DateTime<Utc>>, WebError> {
    text.map(|t| parse_time(&t).map_err(|e| WebError::Engine(EngineError::Data(format!("users.{column}: {e:?}"))))).transpose()
}

impl User {
    fn read(c: &Connection, condition: &str, value: &dyn rusqlite::ToSql) -> Result<Option<User>, WebError> {
        let row = c.query_row(&format!("SELECT {COLUMNS} FROM users WHERE {condition} = ?1"), [value], |r| {
            Ok((User {
                id: r.get(0)?, email: r.get(1)?, encrypted_password: r.get(2)?, admin: r.get(3)?, locale: r.get::<_, Option<String>>(4)?.filter(|l| !l.is_empty()),
                time_zone: r.get(5)?, display_currency: r.get(6)?, hide_balances: r.get(7)?, confirmed: r.get::<_, Option<String>>(8)?.is_some(),
                failed_attempts: r.get::<_, Option<i64>>(9)?.unwrap_or(0), locked_at: None, otp_enabled: r.get::<_, Option<i64>>(11)? == Some(1),
                otp_secret_key: r.get(12)?, last_otp_at: None,
            }, r.get::<_, Option<String>>(10)?, r.get::<_, Option<String>>(13)?))
        }).optional()?;
        row.map(|(user, locked_at, last_otp_at)| Ok(User { locked_at: time("locked_at", locked_at)?, last_otp_at: time("last_otp_at", last_otp_at)?, ..user })).transpose()
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<User>, WebError> {
        Self::read(c, "id", &id)
    }

    /// Devise's find_for_authentication: the email stripped and downcased, then matched exactly.
    pub fn find_by_email(c: &Connection, email: &str) -> Result<Option<User>, WebError> {
        Self::read(c, "email", &email.trim().to_lowercase())
    }

    /// Devise's authenticatable_salt: what ties a session to the password it was opened with.
    pub fn salt(&self) -> &str {
        self.encrypted_password.get(..29).unwrap_or(&self.encrypted_password)
    }

    /// Lockable#lock_expired?: strictly more than the unlock time ago.
    fn lock_expired(&self, now: DateTime<Utc>) -> bool {
        self.locked_at.is_some_and(|at| (now - at).num_microseconds().is_none_or(|us| us > UNLOCK_IN_SECONDS * 1_000_000))
    }

    /// Lockable#access_locked?
    pub fn locked(&self, now: DateTime<Utc>) -> bool {
        self.locked_at.is_some() && !self.lock_expired(now)
    }

    /// Lockable#unlock_access!
    fn unlock(&mut self, c: &Connection, now: DateTime<Utc>) -> Result<(), WebError> {
        c.execute("UPDATE users SET locked_at = NULL, failed_attempts = 0, updated_at = ?1 WHERE id = ?2", (format_time(now), self.id))?;
        (self.locked_at, self.failed_attempts) = (None, 0);
        Ok(())
    }

    /// Lockable#increment_failed_attempts: an atomic counter update that does not move updated_at.
    fn increment_failed_attempts(&mut self, c: &Connection) -> Result<(), WebError> {
        c.execute("UPDATE users SET failed_attempts = COALESCE(failed_attempts, 0) + 1 WHERE id = ?1", [self.id])?;
        self.failed_attempts = c.query_row("SELECT failed_attempts FROM users WHERE id = ?1", [self.id], |r| r.get(0))?;
        Ok(())
    }

    /// Lockable#lock_access!
    fn lock(&mut self, c: &Connection, now: DateTime<Utc>) -> Result<(), WebError> {
        c.execute("UPDATE users SET locked_at = ?1, updated_at = ?1 WHERE id = ?2", (format_time(now), self.id))?;
        self.locked_at = Some(now);
        Ok(())
    }

    /// Lockable#reset_failed_attempts!: a write only when there is something to reset.
    fn reset_failed_attempts(&mut self, c: &Connection, now: DateTime<Utc>) -> Result<(), WebError> {
        if self.failed_attempts != 0 {
            c.execute("UPDATE users SET failed_attempts = 0, updated_at = ?1 WHERE id = ?2", (format_time(now), self.id))?;
            self.failed_attempts = 0;
        }
        Ok(())
    }

    /// Lockable#valid_for_authentication? around the password check, whose answer is `correct`: an
    /// expired lock is lifted first; a wrong password counts; so does a right one while the account is locked.
    fn password_attempt(&mut self, c: &Connection, correct: bool, now: DateTime<Utc>) -> Result<bool, WebError> {
        if self.lock_expired(now) {
            self.unlock(c, now)?;
        }
        if correct && !self.locked(now) {
            return Ok(true);
        }
        self.increment_failed_attempts(c)?;
        if self.failed_attempts >= MAXIMUM_ATTEMPTS && !self.locked(now) {
            self.lock(c, now)?;
        }
        Ok(false)
    }
}

/// Who is making this request.
#[derive(Clone)]
pub enum Current {
    SignedOut,
    SignedIn(Arc<User>),
    /// Signed in, but Devise's `active_for_authentication?` now says no: `locked` or `unconfirmed`.
    Inactive(&'static str),
}

impl Current {
    pub fn user(&self) -> Option<&User> {
        match self {
            Current::SignedIn(user) => Some(user),
            _ => None,
        }
    }
}

/// Warden's fetch from the session: the row must exist and its salt must still match. A lock or a
/// lost confirmation ends the session, as Devise's activatable hook does on every request.
pub async fn current_user(app: &App, session: &super::session::Session, now: DateTime<Utc>) -> Result<Current, WebError> {
    let Some((id, salt)) = session.lock().user.clone() else { return Ok(Current::SignedOut) };
    let user = app.db(move |c| User::find(c, id)).await?.filter(|user| user.salt() == salt);
    let current = match user {
        Some(user) if user.locked(now) => Current::Inactive("locked"),
        Some(user) if !user.confirmed => Current::Inactive("unconfirmed"),
        Some(user) => Current::SignedIn(Arc::new(user)),
        None => Current::SignedOut,
    };
    if !matches!(current, Current::SignedIn(_)) {
        session.lock().user = None;
    }
    Ok(current)
}

/// The login path Devise's failure app redirects to: it keeps the locale prefix of the request that
/// failed, and only a prefix; a `?locale=` parameter does not carry over.
fn login_path(ctx: &Ctx) -> String {
    ctx.params.path_locale.map_or_else(|| "/login".to_string(), |prefix| format!("/{prefix}/login"))
}

/// Devise's failure app: remember where a GET was going, say why, and send the browser to the login page.
/// A path too long to keep is forgotten, and the sign-in then lands on the root.
fn failure(ctx: &Ctx, message: &str) -> Response {
    if ctx.method == Method::GET {
        ctx.session.lock().return_to = Some(ctx.params.fullpath.clone()).filter(|path| path.len() <= MAX_RETURN_TO_BYTES);
    }
    flash::set(&ctx.session, flash::ALERT, i18n::text(ctx.locale, &format!("devise.failure.{message}"), &[]));
    let mut response = layout::early_redirect(StatusCode::FOUND, &login_path(ctx));
    response.extensions_mut().insert(super::headers::BelowControllers);
    response
}

/// `authenticate_user!` for a request with nobody signed in: the failure app's response.
pub fn unauthenticated(ctx: &Ctx) -> Response {
    failure(ctx, "unauthenticated")
}

/// A session that was signed in and no longer may be (see `Current::Inactive`).
pub fn inactive(ctx: &Ctx) -> Option<Response> {
    match ctx.current {
        Current::Inactive(message) => Some(failure(ctx, message)),
        _ => None,
    }
}

/// Devise's two prepended filters, which run before the CSRF check and before switch_locale, so
/// their message is English and their redirect has no locale prefix:
/// - require_no_authentication: the login page and its POST, when already signed in;
/// - verify_signed_out_user: sign-out, when nobody is signed in.
pub fn prepended_filters(ctx: &Ctx) -> Option<Response> {
    let signed_in = ctx.user().is_some();
    match (ctx.params.route_path.as_str(), &ctx.method) {
        ("/login", &Method::GET | &Method::HEAD | &Method::POST) if signed_in => {
            flash::set(&ctx.session, flash::ALERT, i18n::text(i18n::DEFAULT, "devise.failure.already_authenticated", &[]));
            let location = ctx.session.lock().return_to.take().unwrap_or_else(|| "/".to_string());
            Some(layout::early_redirect(StatusCode::FOUND, &location))
        }
        ("/logout", &Method::DELETE) if !signed_in && !matches!(ctx.current, Current::Inactive(_)) => {
            flash::set(&ctx.session, flash::NOTICE, i18n::text(i18n::DEFAULT, "devise.sessions.already_signed_out", &[]));
            Some(layout::early_redirect(StatusCode::SEE_OTHER, "/"))
        }
        _ => None,
    }
}

#[derive(Template)]
#[template(path = "sessions/new.html")]
struct NewView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    email: &'a str,
    registration_open: bool,
}

/// An `app_configs` value, decrypted (AppConfig.get).
pub fn app_config(c: &Connection, cipher: &Cipher, key: &str) -> Result<Option<String>, WebError> {
    let stored: Option<Option<String>> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    stored.flatten().map(|value| cipher.decrypt(&value).map_err(|e| WebError::Engine(EngineError::Data(format!("app_configs[{key}]: {e:?}"))))).transpose()
}

async fn login_page(app: &App, ctx: &Ctx, status: StatusCode, email: &str, flash_now: Vec<(&str, String)>) -> Result<Response, WebError> {
    let inner = app.clone();
    let registration_open = app.db(move |c| Ok(app_config(c, &inner.cipher, "registration_open")?.as_deref() == Some("true"))).await?;
    let csrf = ctx.csrf_token();
    let body = NewView { v: ctx, csrf: &csrf, email, registration_open }.render()?;
    layout::devise(ctx, &csrf, Page { status, body, flash_now })
}

/// GET /login.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    login_page(&app, &ctx, StatusCode::OK, ctx.params.query("user[email]").unwrap_or(""), Vec::new()).await
}

enum PasswordStage {
    Invalid,
    Unconfirmed,
    SignedIn(User),
    /// The password is right and the account has two-factor on: the code is still owed.
    SecondFactor(User),
    /// The same, but the account is locked: no second-factor prompt at all.
    SecondFactorLocked,
}

/// What the password stage decides once the one bcrypt computation of the request is done
/// (`App::password_check`, which runs outside the database lock; `correct` is its answer against
/// `verified_against`, the hash the row held when the request arrived). The row is read again here,
/// under the lock: the lock, the counter and the hash are what they are now, not what they were
/// before the computation, so attempts that overlap each count, and the fifth locks. A row that
/// has gone, or whose password changed meanwhile, is a failed sign-in that counts nothing: the
/// answer was computed against a hash that is no longer the account's.
///
/// Exactly one bcrypt computation per request, on every path: a hash for an unknown email, one
/// verification for a known one. How long the answer takes then says nothing about the account,
/// including whether it has two-factor on (Rails verifies a two-factor account's wrong password twice).
fn password_stage(c: &Connection, user_id: i64, verified_against: &str, correct: bool, password: &str, now: DateTime<Utc>) -> Result<PasswordStage, WebError> {
    let Some(mut user) = User::find(c, user_id)?.filter(|user| user.encrypted_password == verified_against) else {
        return Ok(PasswordStage::Invalid);
    };
    if user.otp_enabled && correct {
        return Ok(if user.locked(now) { PasswordStage::SecondFactorLocked } else { PasswordStage::SecondFactor(user) });
    }
    // Devise's database_authenticatable strategy: a blank password never reaches a row.
    if password.trim().is_empty() {
        return Ok(PasswordStage::Invalid);
    }
    if !user.password_attempt(c, correct, now)? {
        return Ok(PasswordStage::Invalid);
    }
    if !user.confirmed {
        return Ok(PasswordStage::Unconfirmed); // Devise's activatable hook fires before lockable's reset
    }
    user.reset_failed_attempts(c, now)?;
    Ok(PasswordStage::SignedIn(user))
}

/// The answer when too many sign-ins are already waiting for their bcrypt computation
/// (`App::password_check`): try again in a second. Made below the controllers, as rack-attack's 429
/// is; no attempt is spent and no row is touched.
fn busy() -> Response {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::RETRY_AFTER, "1")], "Too many sign-ins at once. Try again in a moment.\n").into_response();
    response.extensions_mut().insert(super::headers::BelowControllers);
    response
}

/// continue_sign_in: the session now belongs to `user`; go where they were heading, else to the root.
fn continue_sign_in(ctx: &Ctx, user: &User, new_csrf_token: bool) -> Response {
    let mut session = ctx.session.lock();
    session.user = Some((user.id, user.salt().to_string()));
    session.auto_open_bot_wizard = true;
    if new_csrf_token {
        session.csrf = None; // Devise's clean_up_csrf_token_on_authentication
    }
    let mut location = session.return_to.take().unwrap_or_else(|| ctx.path("/"));
    // The account's saved locale, unless the request chose one.
    if let Some(saved) = user.locale.as_deref().filter(|l| ctx.params.locale().is_none() && *l != locale::DEFAULT) {
        location.push(if location.contains('?') { '&' } else { '?' });
        location.push_str(&format!("locale={saved}"));
    }
    layout::redirect(StatusCode::SEE_OTHER, &location)
}

/// POST /login.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let email = ctx.params.form("user[email]").unwrap_or("").to_string();
    let password = ctx.params.form("user[password]").unwrap_or("").to_string();
    let now = ctx.now;
    // The row under the lock, the bcrypt computation outside it, then the decision under the lock again.
    let found = { let email = email.clone(); app.db(move |c| Ok(User::find_by_email(c, &email)?.map(|user| (user.id, user.encrypted_password)))).await? };
    let Some(correct) = app.password_check(password.clone(), found.as_ref().map(|(_, hash)| hash.clone())).await? else { return Ok(busy()) };
    let stage = match found {
        Some((id, hash)) => app.db(move |c| password_stage(c, id, &hash, correct, &password, now)).await?,
        None => PasswordStage::Invalid,
    };
    match stage {
        PasswordStage::SignedIn(user) => Ok(continue_sign_in(&ctx, &user, true)),
        PasswordStage::SecondFactor(user) => {
            // sign_out clears the whole session; only the pending sign-in survives.
            *ctx.session.lock() = SessionData { pending: Some(Pending { user_id: user.id, started_at: now.timestamp() }), ..SessionData::default() };
            // pending_sign_in_locale: the request's locale, else the account's; a prefix only for a routable, non-default one.
            let wanted = ctx.params.locale().or(user.locale.as_deref()).and_then(locale::known).unwrap_or(locale::DEFAULT);
            Ok(layout::redirect(StatusCode::FOUND, &locale::path(wanted, "/verify_two_factor")))
        }
        PasswordStage::SecondFactorLocked => {
            flash::set(&ctx.session, flash::ALERT, i18n::text(ctx.locale, "devise.failure.locked", &[]));
            Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/login")))
        }
        PasswordStage::Unconfirmed => Ok(failure(&ctx, "unconfirmed")),
        PasswordStage::Invalid => {
            // Devise names the authentication keys in the message: the attribute's name, downcased.
            let keys = i18n::text(ctx.locale, "activerecord.attributes.user.email", &[]).to_lowercase();
            let message = i18n::text(ctx.locale, "devise.failure.invalid", &[("authentication_keys", Arg::Text(&keys))]);
            login_page(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, &email, vec![(flash::ALERT, message)]).await
        }
    }
}


#[derive(Template)]
#[template(path = "sessions/two_factor.html")]
struct TwoFactorView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
}


/// abandon_pending_sign_in: back to the login page, with the same message a wrong code gets.
fn abandon(ctx: &Ctx) -> Response {
    ctx.session.lock().pending = None;
    flash::set(&ctx.session, flash::ALERT, i18n::text(ctx.locale, "errors.messages.bad_2fa_code", &[]));
    layout::redirect(StatusCode::FOUND, &ctx.path("/login"))
}

/// Users::VerifyOtp: TOTP (SHA1, 6 digits, 30 s) for the previous, current and next step, but only
/// steps later than the last one spent. The last match wins and is recorded.
fn verify_otp(c: &Connection, cipher: &Cipher, user: &User, code: &str, now: DateTime<Utc>) -> Result<bool, WebError> {
    let Some(stored) = user.otp_secret_key.as_deref().filter(|s| !s.trim().is_empty()) else { return Ok(false) };
    let seed = cipher.decrypt(stored).map_err(|e| WebError::Engine(EngineError::Data(format!("users.otp_secret_key: {e:?}"))))?;
    let spent = user.last_otp_at.map(|at| at.timestamp().div_euclid(30));
    // The latest matching step is the one recorded, as ROTP's verify returns it.
    let matched = ((now.timestamp() - 30).div_euclid(30)..=(now.timestamp() + 30).div_euclid(30)).rev()
        .filter(|step| *step >= 0 && spent.is_none_or(|s| *step > s))
        .find(|step| totp_at(&seed, (*step * 30) as u64).is_some_and(|expected| bool::from(expected.as_bytes().ct_eq(code.as_bytes()))));
    let Some(step) = matched else { return Ok(false) };
    let spent_at = DateTime::from_timestamp(step * 30, 0).unwrap_or(now);
    c.execute("UPDATE users SET last_otp_at = ?1, updated_at = ?2 WHERE id = ?3", (format_time(spent_at), format_time(now), user.id))?;
    Ok(true)
}

enum CodeStage {
    /// No user, or the account is locked.
    Abandon,
    /// Nothing to verify: show the form.
    Form,
    Wrong,
    /// The wrong code that reached the limit: the account is locked now.
    WrongAndLocked,
    /// The code was right and is spent, but the account is not confirmed: Devise's activatable hook
    /// refuses the sign-in itself, not the request after it.
    Unconfirmed,
    SignedIn(User),
}

fn code_stage(c: &Connection, cipher: &Cipher, user_id: i64, code: Option<String>, now: DateTime<Utc>) -> Result<CodeStage, WebError> {
    let Some(mut user) = User::find(c, user_id)? else { return Ok(CodeStage::Abandon) };
    if user.lock_expired(now) {
        user.unlock(c, now)?; // unlock_access_if_lock_expired!
    }
    if user.locked(now) {
        return Ok(CodeStage::Abandon);
    }
    let Some(code) = code else { return Ok(CodeStage::Form) };
    if verify_otp(c, cipher, &user, &code, now)? {
        user.reset_failed_attempts(c, now)?;
        return Ok(if user.confirmed { CodeStage::SignedIn(user) } else { CodeStage::Unconfirmed });
    }
    // register_failed_otp_attempt: the same counter as password failures.
    user.increment_failed_attempts(c)?;
    if user.failed_attempts >= MAXIMUM_ATTEMPTS {
        user.lock(c, now)?;
        return Ok(CodeStage::WrongAndLocked);
    }
    Ok(CodeStage::Wrong)
}

fn two_factor_page(ctx: &Ctx, status: StatusCode, flash_now: Vec<(&str, String)>) -> Result<Response, WebError> {
    let csrf = ctx.csrf_token();
    let body = TwoFactorView { v: ctx, csrf: &csrf }.render()?;
    layout::devise(ctx, &csrf, Page { status, body, flash_now })
}

/// GET and POST /verify_two_factor. Only a POST with a code spends an attempt.
pub async fn two_factor(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let now = ctx.now;
    let pending = ctx.session.lock().pending.clone();
    let Some(pending) = pending.filter(|p| p.started_at > 0 && now.timestamp() - p.started_at < PENDING_TTL_SECONDS) else {
        return Ok(abandon(&ctx));
    };
    let code = ctx.params.form("user[otp_code_token]").filter(|code| ctx.method == Method::POST && !code.trim().is_empty()).map(str::to_string);
    let inner = app.clone();
    match app.db(move |c| code_stage(c, &inner.cipher, pending.user_id, code, now)).await? {
        CodeStage::Abandon | CodeStage::WrongAndLocked => Ok(abandon(&ctx)),
        CodeStage::Form => two_factor_page(&ctx, StatusCode::OK, Vec::new()),
        CodeStage::Wrong => {
            let message = i18n::text(ctx.locale, "errors.messages.bad_2fa_code", &[]);
            two_factor_page(&ctx, StatusCode::UNPROCESSABLE_ENTITY, vec![(flash::ALERT, message)])
        }
        CodeStage::Unconfirmed => {
            ctx.session.lock().pending = None;
            Ok(failure(&ctx, "unconfirmed"))
        }
        CodeStage::SignedIn(user) => {
            ctx.session.lock().pending = None;
            Ok(continue_sign_in(&ctx, &user, false))
        }
    }
}

/// DELETE /logout (signed in; the signed-out case is a prepended filter). Devise forgets every
/// remember-me cookie Rails may have issued, clears the session, and the flash with it.
pub async fn destroy(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if let Some(user_id) = ctx.user().map(|u| u.id) {
        let now = ctx.now;
        app.db(move |c| {
            c.execute("UPDATE users SET remember_created_at = NULL, updated_at = ?1 WHERE id = ?2 AND remember_created_at IS NOT NULL", (format_time(now), user_id))?;
            Ok(())
        }).await?;
    }
    *ctx.session.lock() = SessionData::default();
    Ok(layout::redirect(StatusCode::SEE_OTHER, &ctx.path("/")))
}
