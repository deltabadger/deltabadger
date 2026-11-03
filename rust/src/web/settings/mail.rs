//! Account mail (confirmation, password reset, "email already taken") crosses the same SMTP boundary as the mail service.
//!
//! D-ACCOUNT-MAIL-OFF-REQUEST: Rails sends these inside the request (Devise's deliver_now), so a known address waited
//! for SMTP and an unknown one did not, and an SMTP error was a 500 for known addresses only. Here the answer is decided
//! first and `send` can neither fail nor wait: what it cannot prepare or deliver is logged by kind (never an address
//! or a token) and dropped, as Rails' interceptor drops mail when SMTP is not configured. The raw token lives only in
//! the delivery task's memory, never at rest. At most `ACCOUNT_MAILS_AT_ONCE` deliveries run at once; one more is
//! dropped, not queued.
use crate::{
    mail::{self, render, smtp, Message},
    web::{auth, App, WebError},
};
use std::{future::Future, pin::Pin, sync::Arc};
pub type Delivery = Pin<Box<dyn Future<Output = Result<(), WebError>> + Send>>;
pub const ACCOUNT_MAILS_AT_ONCE: usize = 4;
/// Called while the request runs (a test mailer records the message here); the delivery is the future it returns.
pub trait Mailer: Send + Sync {
    fn deliver(
        &self,
        message: Message,
        settings: Option<smtp::Settings>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Delivery;
}
struct Live;
impl Mailer for Live {
    fn deliver(
        &self,
        message: Message,
        settings: Option<smtp::Settings>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Delivery {
        Box::pin(async move {
            let settings = settings
                .ok_or_else(|| WebError::Config("SMTP is not configured".into()))?;
            let from = mail::mailbox(&message.from)
                .ok_or_else(|| WebError::Config("the sender address is invalid".into()))?;
            let to = mail::mailbox(&message.to)
                .ok_or_else(|| WebError::Config("the recipient address is invalid".into()))?;
            let wire = message
                .encode(now, &format!("{}@deltabadger", uuid::Uuid::new_v4()))
                .map_err(|_| WebError::Config("the message cannot be encoded".into()))?;
            smtp::deliver(&settings, &from.address, &to.address, &wire)
                .await
                // Display, not Debug: the stage and the reply's codes, never the server's text (D-TEST-EMAIL-ERROR-SAFE).
                .map_err(|failure| WebError::Config(format!("delivery failed ({failure})")))
        })
    }
}
pub fn live() -> Arc<dyn Mailer> {
    Arc::new(Live)
}

/// Which account mail, and what it carries.
pub enum AccountMail {
    /// Devise's confirmation_instructions to `address` (the account's, or the unconfirmed one).
    Confirmation { user_id: i64, address: String, token: String },
    /// Devise's reset_password_instructions to the account's address, with the raw token.
    ResetPassword { user_id: i64, token: String },
    /// CustomDeviseMailer#email_already_taken to `address` (User::Email.real_email).
    EmailTaken { address: String },
}

impl AccountMail {
    fn kind(&self) -> &'static str {
        match self {
            AccountMail::Confirmation { .. } => "confirmation_instructions",
            AccountMail::ResetPassword { .. } => "reset_password_instructions",
            AccountMail::EmailTaken { .. } => "email_already_taken",
        }
    }
}

/// Renders `mail` in `locale` from the rows as they are, and delivers it off the request. Never fails, never waits.
pub async fn send(app: &App, mail: AccountMail, locale: &'static str) {
    let kind = mail.kind();
    let inner = app.clone();
    let prepared = app.db(move |c| prepare(&inner, c, mail, locale)).await;
    let (message, settings) = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return, // the row is gone: nobody to write to
        Err(error) => return log(kind, &format!("cannot be prepared ({})", reason(&error))),
    };
    let Ok(permit) = app.account_mail_slots.clone().try_acquire_owned() else {
        return log(kind, "dropped: too many account mails are being sent");
    };
    let delivery = app.settings_mailer.deliver(message, settings, app.now());
    // Delivery continues after a browser disconnects, as Rails' after-commit callback does.
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(error) = delivery.await { log(kind, &format!("not sent: {}", reason(&error))) }
    });
}

fn log(kind: &str, what: &str) {
    eprintln!("deltabadger: account mail {kind} {what}");
}

/// A reason fit for the log: this module's own messages, otherwise only the error's kind.
fn reason(error: &WebError) -> String {
    match error {
        WebError::Config(message) => message.clone(),
        WebError::Engine(_) => "database error".into(),
        WebError::Template(_) => "template error".into(),
        WebError::Task(_) => "task error".into(),
        WebError::RecordInvalid => "invalid record".into(),
    }
}

type Prepared = Option<(Message, Option<smtp::Settings>)>;

const NOT_SET_UP: &str = "no SMTP_ADDRESS, and no SMTP settings saved";

fn saved_smtp(app: &App, c: &rusqlite::Connection) -> Result<std::collections::HashMap<&'static str, Option<String>>, WebError> {
    let keys = ["smtp_provider", "smtp_username", "smtp_password", "smtp_host", "smtp_port"];
    keys.into_iter().map(|key| Ok((key, auth::app_config(c, &app.cipher, key)?))).collect()
}

/// D-NO-MAIL-SAYS-SO: whether this install has any way to send mail (SMTP_ADDRESS, or SMTP saved in Settings). A property
/// of the install, never of an account, so the pages that say "no mail can be sent" reveal nothing about who has one.
/// A configuration that is there but invalid counts as set up: its failure is logged, as any delivery failure is.
pub fn set_up(app: &App, c: &rusqlite::Connection) -> Result<bool, WebError> {
    if let Some(set_up) = app.mail_set_up { return Ok(set_up) }
    let saved = saved_smtp(app, c)?;
    let read = |key: &str| saved.get(key).cloned().flatten();
    Ok(!matches!(smtp::Settings::current(&app.settings_smtp, &read), Err(reason) if reason == NOT_SET_UP))
}

fn prepare(app: &App, c: &rusqlite::Connection, mail: AccountMail, locale: &'static str) -> Result<Prepared, WebError> {
    use rusqlite::OptionalExtension;
    let saved = saved_smtp(app, c)?;
    let read = |key: &str| saved.get(key).cloned().flatten();
    let settings = match smtp::Settings::current(&app.settings_smtp, &read) {
        Ok(s) => Some(s),
        Err(reason) if reason == NOT_SET_UP => None,
        Err(_) => return Err(WebError::Config("the SMTP configuration is invalid".into())),
    };
    let user = |condition: &str, value: &dyn rusqlite::ToSql| -> Result<Option<(String, String)>, WebError> {
        Ok(c.query_row(&format!("SELECT COALESCE(name,''), email FROM users WHERE {condition} = ?1"), [value], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    };
    let urls = render::Urls { root: app.config.own_origin.clone().unwrap_or_else(|| "http://localhost:3000".into()) + "/" };
    let from = smtp::notifications_sender(&app.settings_smtp, &read);
    let recipient = |name: String, email: String| render::Recipient { email, name, locale };
    let message = match mail {
        AccountMail::Confirmation { user_id, address, token } => {
            let Some((name, _)) = user("id", &user_id)? else { return Ok(None) };
            render::confirmation_instructions(&from, &urls, &recipient(name, address), &token)
        }
        AccountMail::ResetPassword { user_id, token } => {
            let Some((name, email)) = user("id", &user_id)? else { return Ok(None) };
            render::reset_password_instructions(&from, &urls, &recipient(name, email), &token)
        }
        AccountMail::EmailTaken { address } => {
            // `@resource = User.find_by(email:)`: the greeting names that account.
            let name = user("email", &address)?.map(|(name, _)| name).unwrap_or_default();
            render::email_already_taken(&from, &urls, &recipient(name, address))
        }
    };
    Ok(Some((message, settings)))
}

/// The confirmation mail of a resend or of an email change (settings::confirmation, settings::account).
pub async fn confirmation(app: &App, user_id: i64, address: String, token: String, locale: &'static str) {
    send(app, AccountMail::Confirmation { user_id, address, token }, locale).await;
}

/// What a test email needs, or why there is none.
enum TestMail { NotConfigured, Refused(String), Ready(Box<(Message, smtp::Settings, String)>) }

/// SettingsController#send_test_email (admin only): TestMailer.test_email(current_user).deliver_now, inside the request.
/// The wait is the SMTP client's own bound: 5 s to connect, 5 s for each reply, 30 s for the whole delivery; a server that
/// does not answer in time is an alert naming the stage ("greeting: no answer in time").
/// D-TEST-EMAIL-ERROR-SAFE: Rails' alert carries `e.message`, the server's reply text, which can name the SMTP account. This
/// alert carries smtp::Failure's stage and codes only, never server text or a credential.
pub async fn send_test(app: App, ctx: crate::web::layout::Ctx) -> Result<axum::response::Response, WebError> {
    use crate::web::{flash, i18n, turbo};
    use axum::{http::{header, StatusCode}, response::IntoResponse};
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    if !user.admin { return Ok((StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "text/html")]).into_response()) }
    // ApplicationMailer#set_locale(user): the user's language, whatever the request's.
    let locale = crate::web::locale::switch(None, user.locale.as_deref());
    let inner = app.clone();
    let prepared = app.db(move |c| {
        let saved = saved_smtp(&inner, c)?;
        let read = |key: &str| saved.get(key).cloned().flatten();
        let settings = match smtp::Settings::current(&inner.settings_smtp, &read) {
            Ok(settings) => settings,
            Err(reason) if reason == NOT_SET_UP => return Ok(TestMail::NotConfigured),
            // SmtpSettings.configured? is true for what this build refuses to send with: Rails' delivery fails there.
            Err(reason) => return Ok(TestMail::Refused(reason)),
        };
        let name: String = c.query_row("SELECT COALESCE(name,'') FROM users WHERE id = ?1", [user.id], |r| r.get(0))?;
        let urls = render::Urls { root: inner.config.own_origin.clone().unwrap_or_else(|| "http://localhost:3000".into()) + "/" };
        let message = render::test_email(&smtp::notifications_sender(&inner.settings_smtp, &read), &urls,
                                         &render::Recipient { email: user.email.clone(), name, locale });
        Ok(TestMail::Ready(Box::new((message, settings, user.email))))
    }).await?;
    let failed = |error: &str| (flash::ALERT, i18n::t(ctx.locale, "settings.email_notifications.test_email_failed", &[("error", i18n::Arg::Text(error))]));
    let now = match prepared {
        TestMail::NotConfigured => (flash::ALERT, ctx.t("settings.email_notifications.not_configured")),
        TestMail::Refused(reason) => failed(&reason),
        TestMail::Ready(ready) => { let (message, settings, email) = *ready; match app.settings_mailer.deliver(message, Some(settings), app.now()).await {
            Ok(()) => (flash::NOTICE, i18n::t(ctx.locale, "settings.email_notifications.test_email_sent", &[("email", i18n::Arg::Text(&email))])),
            // The alert already says the mail failed: the stage and codes alone (`delivery failed (auth: 535 5.7.8)` → `auth: 535 5.7.8`).
            Err(error) => { let why = reason(&error); failed(why.strip_prefix("delivery failed (").and_then(|w| w.strip_suffix(')')).unwrap_or(&why)) }
        } }
    };
    let markup = flash::render(&flash::take(&ctx.session, &[now]))?;
    Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], turbo::prepend_flash(markup.trim_end())).into_response())
}
