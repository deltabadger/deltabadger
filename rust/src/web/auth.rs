//! Sign-in, the second factor and sign-out, as Users::SessionsController, Devise 5 (paranoid mode,
//! :lockable, :confirmable) and Users::VerifyOtp behave, on the same `users` columns.
//! There is no remember-me here: the checkbox is rendered and ignored. Rails' cookie is always
//! `Secure`, so it never worked on a plain-http install, and a session lasts 30 days anyway.
use super::layout::{self, Ctx, Page};
use super::{App, WebError};
use crate::crypto::Cipher;
use crate::engine::EngineError;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;

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
