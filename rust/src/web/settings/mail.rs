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
                .map_err(|failure| WebError::Config(format!("delivery failed ({failure:?})")))
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
    let delivery = app.settings_mailer.deliver(message, settings, app.now());
    let Ok(permit) = app.account_mail_slots.clone().try_acquire_owned() else {
        return log(kind, "dropped: too many account mails are being sent");
    };
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

fn prepare(app: &App, c: &rusqlite::Connection, mail: AccountMail, locale: &'static str) -> Result<Prepared, WebError> {
    use rusqlite::OptionalExtension;
    let keys = ["smtp_provider", "smtp_username", "smtp_password", "smtp_host", "smtp_port"];
    let mut saved = std::collections::HashMap::new();
    for key in keys {
        saved.insert(key.to_string(), auth::app_config(c, &app.cipher, key)?);
    }
    let read = |key: &str| saved.get(key).cloned().flatten();
    let settings = match smtp::Settings::current(&app.settings_smtp, &read) {
        Ok(s) => Some(s),
        Err(reason) if reason == "no SMTP_ADDRESS, and no SMTP settings saved" => None,
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
