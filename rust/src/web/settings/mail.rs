//! Account mail crosses the same SMTP boundary as the existing mail service.
use crate::{
    mail::{self, render, smtp, Message},
    web::{auth, App, WebError},
};
use std::{future::Future, pin::Pin, sync::Arc};
pub type Delivery = Pin<Box<dyn Future<Output = Result<(), WebError>> + Send>>;
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
                .ok_or_else(|| WebError::Config("account mail has no SMTP configuration".into()))?;
            let from = mail::mailbox(&message.from)
                .ok_or_else(|| WebError::Config("account mail has an invalid sender".into()))?;
            let to = mail::mailbox(&message.to)
                .ok_or_else(|| WebError::Config("account mail has an invalid recipient".into()))?;
            let wire = message
                .encode(now, &format!("{}@deltabadger", uuid::Uuid::new_v4()))
                .map_err(|_| WebError::Config("cannot encode account mail".into()))?;
            smtp::deliver(&settings, &from.address, &to.address, &wire)
                .await
                .map_err(|_| WebError::Config("account mail delivery failed".into()))
        })
    }
}
pub fn live() -> Arc<dyn Mailer> {
    Arc::new(Live)
}
pub async fn confirmation(
    app: &App,
    user_id: i64,
    address: String,
    token: String,
    locale: &'static str,
) -> Result<(), WebError> {
    let inner = app.clone();
    let (message, settings) = app
        .db(move |c| {
            let keys = [
                "smtp_provider",
                "smtp_username",
                "smtp_password",
                "smtp_host",
                "smtp_port",
            ];
            let mut saved = std::collections::HashMap::new();
            for key in keys {
                saved.insert(key.to_string(), auth::app_config(c, &inner.cipher, key)?);
            }
            let read = |key: &str| saved.get(key).cloned().flatten();
            let settings = match smtp::Settings::current(&inner.settings_smtp, &read) {
                Ok(s) => Some(s),
                Err(reason) if reason == "no SMTP_ADDRESS, and no SMTP settings saved" => None,
                Err(_) => {
                    return Err(WebError::Config(
                        "invalid account SMTP configuration".into(),
                    ))
                }
            };
            let name: String = c.query_row(
                "SELECT COALESCE(name,'') FROM users WHERE id=?1",
                [user_id],
                |r| r.get(0),
            )?;
            let recipient = render::Recipient {
                email: address,
                name,
                locale,
            };
            let urls = render::Urls {
                root: inner
                    .config
                    .own_origin
                    .clone()
                    .unwrap_or_else(|| "http://localhost:3000".into())
                    + "/",
            };
            let message = render::confirmation_instructions(
                &smtp::notifications_sender(&inner.settings_smtp, &read),
                &urls,
                &recipient,
                &token,
            );
            Ok((message, settings))
        })
        .await?;
    let delivery = app.settings_mailer.deliver(message, settings, app.now());
    // Delivery continues after a browser disconnects, as Rails' after-commit callback does.
    tokio::spawn(delivery)
        .await
        .map_err(|_| WebError::Config("account mail task failed".into()))?
}
