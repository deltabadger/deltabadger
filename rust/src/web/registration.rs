//! Signup: Users::RegistrationsController on Devise 5 (registerable, confirmable, validatable, paranoid privacy on a
//! taken address), on the same `users` columns. The validations are setup's (`setup::validate`, User#valid? in Rails'
//! order).
//!
//! Named divergences (PLAN-A16):
//! - D-SIGNUP-NO-THANK-YOU: /thank-you is gone in 3.0; a signup lands on the login page with Devise's own
//!   signed_up_but_unconfirmed notice.
//! - Privacy: a taken address answers exactly as a new one (Rails answers 302 to the one and 303 to the other), with one
//!   bcrypt computation on both, and mails the account owner instead (D-ACCOUNT-MAIL-OFF-REQUEST: never in the request).
//! - D-SIGNUP-RACE: two signups with one address at once: the second meets the unique index and takes the taken path.
//! - D-SIGNUP-REAL-EMAIL: the Gmail account that "already taken" goes to is matched by username exactly, not by LIKE.
//! - D-SIGNUP-THROTTLE (rate_limit.rs) and D-SIGNUP-NAME-ONE-LINE (setup::validate).
use super::layout::{self, Ctx, Page};
use super::settings::mail::{self, AccountMail};
use super::setup::{self, FieldError};
use super::{auth, flash, App, WebError};
use crate::codec::format_time;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior};

/// Which links a Devise page shows under its form (devise/shared/_links).
pub struct Links {
    pub login: bool,
    pub signup: bool,
}

#[derive(Template)]
#[template(path = "registrations/new.html")]
struct NewView<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    links: Links,
    name: Option<&'a str>,
    email: &'a str,
    name_error: Option<String>,
    email_error: Option<String>,
    password_error: Option<String>,
}

pub(super) async fn registration_open(app: &App) -> Result<bool, WebError> {
    let inner = app.clone();
    app.db(move |c| Ok(auth::app_config(c, &inner.cipher, "registration_open")?.as_deref() == Some("true"))).await
}

/// `errors[attribute]` as the inline error shows it: joined, first letter up.
fn field_error(errors: &[FieldError], attribute: &str) -> Option<String> {
    let messages: Vec<&str> = errors.iter().filter(|e| e.attribute == attribute).map(|e| e.message.as_str()).collect();
    (!messages.is_empty()).then(|| setup::upcase_first(&messages.join(", ")))
}

fn page(ctx: &Ctx, status: StatusCode, name: Option<&str>, email: &str, errors: &[FieldError]) -> Result<Response, WebError> {
    let csrf = ctx.csrf_token();
    let body = NewView {
        v: ctx, csrf: &csrf, links: Links { login: true, signup: false }, name, email,
        name_error: field_error(errors, "name"), email_error: field_error(errors, "email"), password_error: field_error(errors, "password"),
    }.render()?;
    layout::devise(ctx, &csrf, Page { status, body, flash_now: Vec::new() })
}

/// check_registration_open.
fn closed(ctx: &Ctx) -> Response {
    layout::redirect(StatusCode::FOUND, &ctx.path("/login"))
}

/// GET /signup.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if !registration_open(&app).await? { return Ok(closed(&ctx)) }
    page(&ctx, StatusCode::OK, None, "", &[])
}

enum Outcome {
    Invalid(Vec<FieldError>),
    /// The address belongs to an account: mail its owner, at this address.
    Taken(String),
    Created { id: i64, token: String },
}

/// POST /signup.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if !registration_open(&app).await? { return Ok(closed(&ctx)) }
    // params.require(:user): no user field at all is ActionController::ParameterMissing.
    if !ctx.params.form.iter().any(|(name, _)| name.starts_with("user[")) { return Ok(layout::bad_request()) }
    let field = |name: &str| ctx.params.form(name).map(str::to_string);
    let (name, email, password) = (field("user[name]"), field("user[email]"), field("user[password]"));
    let email = email.as_deref().map(setup::normalized_email).unwrap_or_default();
    // Devise hashes on assignment, before validation, and only a present password: one computation on every path that
    // has one, the taken path included.
    let hash = match password.as_deref().filter(|p| !crate::ruby::blank(p)) {
        Some(password) => match app.password_hash(password.to_string()).await? {
            None => return Ok(auth::busy()),
            Some(None) => return Err(WebError::Config("no random salt for a new account's password".into())),
            Some(Some(hash)) => Some(hash),
        },
        None => None,
    };
    let (locale, now, inner) = (ctx.locale, ctx.now, app.clone());
    let (submitted_name, address) = (name.clone(), email.clone());
    let outcome = app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        let errors = setup::validate(&tx, locale, name.as_deref(), &address, password.as_deref())?;
        // Rails drops every email error once one is :taken, and answers as for a new account if nothing else is wrong.
        let taken = errors.iter().any(|e| e.attribute == "email" && e.kind == Some("taken"));
        let errors: Vec<FieldError> = errors.into_iter().filter(|e| !(taken && e.attribute == "email")).collect();
        if !errors.is_empty() { return Ok(Outcome::Invalid(errors)) }
        if taken { return Ok(Outcome::Taken(real_email(&tx, &address)?)) }
        let hash = hash.ok_or_else(|| WebError::Config("a valid signup without a password hash".into()))?;
        let (token, at) = (crate::crypto::friendly_token(), format_time(now));
        let inserted = tx.execute(
            "INSERT INTO users (name, email, encrypted_password, confirmation_token, confirmation_sent_at, otp_secret_key, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5, ?5)",
            (&name, &address, &hash, &token, &at, inner.cipher.encrypt_binary(&super::settings::two_fa::new_seed())),
        );
        match inserted {
            Ok(_) => {}
            // D-SIGNUP-RACE: another signup took the address after the validation read it.
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation && email_exists(&tx, &address)? => {
                return Ok(Outcome::Taken(real_email(&tx, &address)?));
            }
            Err(e) => return Err(e.into()),
        }
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Outcome::Created { id, token })
    }).await?;
    let mail = match outcome {
        Outcome::Invalid(errors) => return page(&ctx, StatusCode::UNPROCESSABLE_ENTITY, submitted_name.as_deref(), &email, &errors),
        Outcome::Taken(address) => AccountMail::EmailTaken { address },
        Outcome::Created { id, token } => AccountMail::Confirmation { user_id: id, address: email, token },
    };
    mail::send(&app, mail, ctx.locale).await;
    flash::set(&ctx.session, flash::NOTICE, super::i18n::text(ctx.locale, "devise.registrations.signed_up_but_unconfirmed", &[]));
    Ok(layout::redirect(StatusCode::SEE_OTHER, &ctx.path("/login")))
}

fn email_exists(c: &Connection, email: &str) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE email = ?1)", [email], |r| r.get(0))?)
}

/// User::Email.real_email: for a Gmail address, the account holding the same Google username (exact, then the other
/// domain, then a +tag address on gmail.com, then on googlemail.com); any other address is itself. D-SIGNUP-REAL-EMAIL:
/// usernames compared exactly, where Rails' LIKE read `_` and `%` in them as wildcards.
fn real_email(c: &Connection, email: &str) -> Result<String, WebError> {
    let Some(username) = setup::google_username(email) else { return Ok(email.to_string()) };
    for exact in [format!("{username}@gmail.com"), format!("{username}@googlemail.com")] {
        if let Some(found) = c.query_row("SELECT email FROM users WHERE email = ?1", [&exact], |r| r.get::<_, String>(0)).optional()? { return Ok(found) }
    }
    let mut statement = c.prepare("SELECT email FROM users WHERE email LIKE '%@gmail.com' OR email LIKE '%@googlemail.com' ORDER BY id")?;
    let stored = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let tagged = |domain: &str| stored.iter().find(|e| e.ends_with(domain) && e.contains('+') && setup::google_username(e).as_deref() == Some(username.as_str())).cloned();
    Ok(tagged("@gmail.com").or_else(|| tagged("@googlemail.com")).unwrap_or_else(|| email.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(emails: &[&str]) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT)", []).unwrap();
        for email in emails { c.execute("INSERT INTO users (email) VALUES (?1)", [email]).unwrap(); }
        c
    }

    #[test]
    fn real_email_matches_the_google_username_exactly() {
        // D-SIGNUP-REAL-EMAIL: Rails' LIKE 'j_n+%@gmail.com' also matched jan+x@gmail.com (a lower id).
        let c = db(&["jan+x@gmail.com", "j_n+y@gmail.com"]);
        assert_eq!(real_email(&c, "j_n@gmail.com").unwrap(), "j_n+y@gmail.com");
        assert_eq!(real_email(&c, "jan+new@googlemail.com").unwrap(), "jan+x@gmail.com");
        let c = db(&["ann+q@googlemail.com", "ann@gmail.com"]);
        assert_eq!(real_email(&c, "ann+z@gmail.com").unwrap(), "ann@gmail.com");
        assert_eq!(real_email(&db(&[]), "x@example.com").unwrap(), "x@example.com");
    }
}
