//! Sign-in, the second factor and sign-out, as Users::SessionsController, Devise 5 (paranoid mode,
//! :lockable, :confirmable) and Users::VerifyOtp behave, on the same `users` columns.
//! There is no remember-me here: the checkbox is rendered and ignored. Rails' cookie is always
//! `Secure`, so it never worked on a plain-http install, and a session lasts 30 days anyway.
use super::layout::{self, Ctx, Page};
use super::{flash, i18n, i18n::Arg, locale, App, WebError};
use crate::codec::{format_time, parse_time};
use crate::crypto::{hash_password, verify_password, Cipher};
use crate::engine::EngineError;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;

/// Devise.maximum_attempts and Devise.unlock_in (pinned by tests/web.rs).
pub const MAXIMUM_ATTEMPTS: i64 = 5;
pub const UNLOCK_IN_SECONDS: i64 = 15 * 60;

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
fn failure(ctx: &Ctx, message: &str) -> Response {
    if ctx.method == Method::GET {
        ctx.session.lock().return_to = Some(ctx.params.fullpath.clone());
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
}

/// Exactly one bcrypt computation per request, on every path: a hash for an unknown email, one
/// verification for a known one. How long the answer takes then says nothing about the account.
fn password_stage(c: &Connection, email: &str, password: &str, now: DateTime<Utc>) -> Result<PasswordStage, WebError> {
    let Some(mut user) = User::find_by_email(c, email)? else {
        let _ = hash_password(password);
        return Ok(PasswordStage::Invalid);
    };
    let correct = verify_password(password, &user.encrypted_password);
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
    let stage = { let email = email.clone(); app.db(move |c| password_stage(c, &email, &password, now)).await? };
    match stage {
        PasswordStage::SignedIn(user) => Ok(continue_sign_in(&ctx, &user, true)),
        PasswordStage::Unconfirmed => Ok(failure(&ctx, "unconfirmed")),
        PasswordStage::Invalid => {
            // Devise names the authentication keys in the message: the attribute's name, downcased.
            let keys = i18n::text(ctx.locale, "activerecord.attributes.user.email", &[]).to_lowercase();
            let message = i18n::text(ctx.locale, "devise.failure.invalid", &[("authentication_keys", Arg::Text(&keys))]);
            login_page(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, &email, vec![(flash::ALERT, message)]).await
        }
    }
}

