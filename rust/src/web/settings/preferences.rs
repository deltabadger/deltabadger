//! The preference writes of SettingsController (app/controllers/settings_controller.rb): the menu's display currency
//! and balance switch, the tracker's cash switch, the account page's advanced-bots switch, and the admin's registration
//! and email-notification settings.
use super::view;
use crate::{
    codec::format_time,
    sync::wire,
    web::{
        auth::{self, User},
        flash,
        i18n::escape,
        layout::{self, Ctx},
        turbo, App, WebError,
    },
};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, Transaction, TransactionBehavior};

/// The PATCH actions this module answers.
pub const ACTIONS: [&str; 6] = [
    "update_display_currency",
    "update_hide_balances",
    "update_show_cash",
    "update_advanced_bots",
    "update_registration",
    "update_email_notifications",
];

/// User::DISPLAY_CURRENCIES.
const CURRENCIES: [&str; 5] = ["USD", "EUR", "GBP", "CHF", "PLN"];

/// SettingsController::REDACTED: a stored secret as the page shows it; submitted back, it keeps what is stored.
const REDACTED: &str = "[redacted]";

fn stream(body: String) -> Response {
    ([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response()
}

/// `redirect_back fallback_location: …, status: :see_other`.
fn back(app: &App, ctx: &Ctx, headers: &HeaderMap, fallback: &str) -> Response {
    let destination = layout::back(&app.config, headers).unwrap_or_else(|| ctx.path(fallback));
    layout::redirect(StatusCode::SEE_OTHER, &destination)
}

/// A user's boolean column set to `on`, written only when it changes (an unchanged attribute is not saved).
fn switch(c: &Connection, app: &App, user: i64, column: &str, on: bool) -> Result<(), WebError> {
    c.execute(
        &format!("UPDATE users SET {column}=?1,updated_at=?2 WHERE id=?3 AND {column} IS NOT ?1"),
        (on, format_time(app.now()), user),
    )?;
    Ok(())
}

pub async fn write(app: App, ctx: Ctx, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let action = ctx.params.route_path.rsplit('/').next().unwrap_or("").to_string();
    // AdminOnly#require_admin!: `head :forbidden`.
    if matches!(action.as_str(), "update_registration" | "update_email_notifications") && !user.admin {
        return Ok((StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "text/html")]).into_response());
    }
    match action.as_str() {
        "update_display_currency" => display_currency(app, ctx, headers, user).await,
        "update_hide_balances" => hide_balances(app, ctx, headers, user).await,
        "update_show_cash" => show_cash(app, ctx, headers, user).await,
        "update_advanced_bots" => advanced_bots(app, ctx, user).await,
        "update_registration" => registration(app, ctx).await,
        _ => email_notifications(app, ctx).await,
    }
}

/// `current_user.update(params.require(:user).permit(:display_currency))`: blank is USD, the code is upcased, and it
/// must be one of the five; then a notice and back to the page the menu was opened on.
async fn display_currency(app: App, ctx: Ctx, headers: HeaderMap, user: User) -> Result<Response, WebError> {
    if !ctx.params.form.iter().any(|(name, _)| name.starts_with("user[")) {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    if let Some(value) = ctx.params.form("user[display_currency]") {
        let code = if crate::ruby::blank(value) { "USD".to_string() } else { value.to_uppercase() };
        if !CURRENCIES.contains(&code.as_str()) {
            // render_preference_error.
            let markup = flash::render(&flash::take(&ctx.session, &[(flash::ALERT, ctx.t("errors.messages.inclusion"))]))?;
            let mut response = stream(turbo::prepend_flash(markup.trim_end()));
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return Ok(response);
        }
        let inner = app.clone();
        app.db(move |c| {
            c.execute(
                "UPDATE users SET display_currency=?1,updated_at=?2 WHERE id=?3 AND display_currency IS NOT ?1",
                (&code, format_time(inner.now()), user.id),
            )?;
            Ok(())
        })
        .await?;
    }
    flash::set(&ctx.session, flash::NOTICE, ctx.t("settings.language_and_timezone.currency_updated"));
    Ok(back(&app, &ctx, &headers, "/bots"))
}

/// Stores the submitted state, then `broadcast_refresh_to("user_<id>", :preferences)` — with the submitting page's
/// Turbo request id, so that page does not refresh twice — and back.
async fn hide_balances(app: App, ctx: Ctx, headers: HeaderMap, user: User) -> Result<Response, WebError> {
    let on = ctx.params.form("hide_balances") == Some("1");
    let inner = app.clone();
    app.db(move |c| switch(c, &inner, user.id, "hide_balances", on)).await?;
    // turbo_stream_refresh_tag(request_id: Turbo.current_request_id): the X-Turbo-Request-Id header, when present.
    let request_id = headers
        .get("x-turbo-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|id| !crate::ruby::blank(id))
        .map_or_else(String::new, |id| format!(" request-id=\"{}\"", escape(id)));
    app.hub.broadcast(
        &format!("user_{}:preferences", user.id),
        &format!("<turbo-stream{request_id} action=\"refresh\"></turbo-stream>"),
    );
    Ok(back(&app, &ctx, &headers, "/bots"))
}

/// `tracker_settings` merged with `show_cash` as submitted; back to the tracker.
async fn show_cash(app: App, ctx: Ctx, headers: HeaderMap, user: User) -> Result<Response, WebError> {
    let on = if ctx.params.form("show_cash") == Some("1") { "true" } else { "false" };
    let inner = app.clone();
    app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        let stored: Option<String> = tx.query_row("SELECT tracker_settings FROM users WHERE id=?1", [user.id], |r| r.get(0))?;
        let parsed = match stored.as_deref() {
            None => wire::Node::Object(vec![]),
            Some(text) => wire::read(text, &mut wire::Budget(usize::MAX), None)
                .map_err(|_| WebError::Config("users.tracker_settings is not JSON".into()))?,
        };
        let mut members = match parsed {
            wire::Node::Object(members) => members,
            wire::Node::Scalar(s) if s == "null" => vec![],
            // `Array#merge` and friends: Rails raises.
            _ => return Err(WebError::Config("users.tracker_settings is not an object".into())),
        };
        let value = wire::Node::Scalar(on.into());
        match members.iter_mut().find(|(key, _)| key == "show_cash") {
            Some((_, old)) if *old == value => return Ok(()),
            Some((_, old)) => *old = value,
            None => members.push(("show_cash".into(), value)),
        }
        tx.execute(
            "UPDATE users SET tracker_settings=?1,updated_at=?2 WHERE id=?3",
            (wire::Node::Object(members).stored(), format_time(inner.now()), user.id),
        )?;
        tx.commit()?;
        Ok(())
    })
    .await?;
    Ok(back(&app, &ctx, &headers, "/tracker"))
}

async fn advanced_bots(app: App, ctx: Ctx, user: User) -> Result<Response, WebError> {
    let on = ctx.params.form("enabled") == Some("1");
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let widget = app
        .db(move |c| {
            switch(c, &inner, user.id, "advanced_bots_enabled", on)?;
            view::advanced(&v, &csrf, on)
        })
        .await?;
    Ok(stream(turbo::stream("replace", "advanced_bots", widget.trim_end())))
}

/// A widget replaced in its frame, and the flash prepended (`flash.now`, when there is a notice).
fn widget_and_flash(ctx: &Ctx, target: &str, widget: &str, notice: Option<&str>) -> Result<Response, WebError> {
    let now: Vec<(&str, String)> = notice.map(|key| (flash::NOTICE, ctx.t(key))).into_iter().collect();
    let markup = flash::render(&flash::take(&ctx.session, &now))?;
    Ok(stream(turbo::stream("replace", target, widget.trim_end()) + &turbo::prepend_flash(markup.trim_end())))
}

async fn registration(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let on = ctx.params.form("registration_open") == Some("1");
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let widget = app
        .db(move |c| {
            crate::app_config::set(c, &inner.cipher, "registration_open", if on { "true" } else { "false" }, inner.now())
                .map_err(WebError::Config)?;
            view::registration(&v, &csrf, on)
        })
        .await?;
    widget_and_flash(&ctx, "registration_settings", &widget, Some("settings.registration.updated"))
}

/// A blank provider switches mail off and keeps the credentials; Custom SMTP without a user name and a password opens
/// its form instead; the platform's server needs nothing more. Anything else changes nothing.
async fn email_notifications(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let (widget, notice) = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            let (cipher, now) = (&inner.cipher, inner.now());
            let form = |name: &str| v.params.form(name).map(str::to_string);
            let config = |result: Result<(), String>| result.map_err(WebError::Config);
            let mut show_form = false;
            let provider = form("smtp_provider");
            let blank = provider.as_deref().is_none_or(crate::ruby::blank);
            let notice = match provider.as_deref() {
                _ if blank => {
                    config(crate::app_config::delete(&tx, "smtp_provider"))?;
                    Some("settings.email_notifications.disabled")
                }
                Some("custom_smtp") => {
                    let kept = |name: &str| -> Result<Option<String>, WebError> {
                        match form(name) {
                            Some(submitted) if submitted == REDACTED => auth::app_config(&tx, cipher, name),
                            submitted => Ok(submitted),
                        }
                    };
                    let (username, password) = (kept("smtp_username")?, kept("smtp_password")?);
                    let present = |value: &Option<String>| value.as_deref().is_some_and(|s| !crate::ruby::blank(s));
                    if present(&username) && present(&password) {
                        config(crate::app_config::set(&tx, cipher, "smtp_provider", "custom_smtp", now))?;
                        for (key, value) in [
                            ("smtp_host", form("smtp_host")),
                            ("smtp_port", form("smtp_port")),
                            ("smtp_username", username),
                            ("smtp_password", password),
                        ] {
                            config(crate::app_config::set_nullable(&tx, cipher, key, value.as_deref(), now))?;
                        }
                        Some("settings.email_notifications.updated")
                    } else {
                        config(crate::app_config::delete(&tx, "smtp_provider"))?;
                        show_form = true;
                        None
                    }
                }
                Some("env_smtp") => {
                    config(crate::app_config::set(&tx, cipher, "smtp_provider", "env_smtp", now))?;
                    Some("settings.email_notifications.updated")
                }
                _ => None,
            };
            let widget = view::email(&tx, &inner, &v, &csrf, show_form)?;
            tx.commit()?;
            Ok((widget, notice))
        })
        .await?;
    widget_and_flash(&ctx, "email_notifications", &widget, notice)
}
