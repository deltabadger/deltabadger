use super::account::Draft;
use crate::web::{
    auth::User,
    i18n::{self, escape, Arg},
    layout::{Ctx, Page},
    shell::{self, Shell},
    timezone, App, WebError,
};
use askama::Template;
use axum::{http::StatusCode, response::Response};
use chrono::Offset;
use rusqlite::Connection;

#[derive(Template)]
#[template(path = "settings/account.html")]
struct Account<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    user: &'a User,
    name: String,
    version: &'static str,
    locale_label: String,
    locale_options: String,
    zone_label: String,
    zone_options: String,
    zone_id: String,
    local_time: String,
    two_fa: bool,
    two_fa_button: String,
    length_label: String,
    errors: Vec<String>,
    messages: std::collections::BTreeMap<String, String>,
    pending_email: String,
    pending_label: String,
    excluded_registration_settings: String,
    excluded_advanced_bots: String,
    excluded_email_notifications: String,
    wash_widget: String,
}
impl Account<'_> {
    fn error(&self, field: &str) -> String {
        self.messages.get(field).cloned().unwrap_or_default()
    }
    fn invalid(&self, field: &str) -> bool {
        self.errors.iter().any(|error| error == field)
    }
}
fn language_options(selected: &str) -> (String, String) {
    let langs = [
        ("en", "English"),
        ("de", "Deutsch"),
        ("nl", "Nederlands"),
        ("fr", "Français"),
        ("es", "Español"),
        ("pt", "Português"),
        ("it", "Italiano"),
        ("pl", "Polski"),
        ("ru", "Русский"),
        ("cs", "Čeština"),
        ("sk", "Slovenčina"),
        ("da", "Dansk"),
        ("sv", "Svenska"),
        ("el", "Ελληνικά"),
        ("bg", "Български"),
    ];
    let label = langs
        .iter()
        .find(|(code, _)| *code == selected)
        .map_or("English", |(_, label)| label)
        .to_string();
    let options = langs
        .iter()
        .map(|(code, label)| {
            format!(
                "<option{} value=\"{}\">{}</option>",
                if *code == selected {
                    " selected=\"selected\""
                } else {
                    ""
                },
                code,
                label
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    (label, options)
}
fn zone_label(name: &str, offset: i32) -> String {
    format!(
        "(GMT {}{}:{:02}) {}",
        if offset >= 0 { "+" } else { "-" },
        offset.abs() / 3600,
        offset.abs() % 3600 / 60,
        name
    )
}
fn zones(ctx: &Ctx, selected: &str) -> (String, String) {
    let mut zones: Vec<_> = timezone::names()
        .map(|name| {
            let seconds = timezone::local(ctx.now, name)
                .offset()
                .fix()
                .local_minus_utc();
            (seconds, name.as_str())
        })
        .collect();
    zones.sort();
    let options = zones
        .iter()
        .map(|(offset, name)| {
            format!(
                "<option data-offset=\"{}\"{} value=\"{}\">{}</option>",
                offset,
                if *name == selected {
                    " selected=\"selected\""
                } else {
                    ""
                },
                escape(name),
                escape(&zone_label(name, *offset))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    (
        zone_label(
            selected,
            timezone::local(ctx.now, selected)
                .offset()
                .fix()
                .local_minus_utc(),
        ),
        options,
    )
}
#[derive(Template)]
#[template(path = "settings/registration_settings.html")]
struct Registration<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    registration_open: bool,
}
#[derive(Template)]
#[template(path = "settings/advanced_bots.html")]
struct Advanced<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    advanced: bool,
}
#[derive(Template)]
#[template(path = "settings/email_notifications.html")]
struct EmailNotifications<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    mail_configured: bool,
    env_available: bool,
    env_name: String,
    custom: bool,
    env_configured: bool,
    has_credentials: bool,
    host: Option<String>,
    port: Option<String>,
    configured_label: String,
}
#[derive(Template)]
#[template(path = "settings/wash.html")]
struct Wash<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    enabled: Option<bool>,
    question: String,
    locked: String,
}
pub fn wash(c: &Connection, ctx: &Ctx, csrf: &str, user_id: i64) -> Result<String, WebError> {
    let (enabled, jurisdiction): (Option<bool>, Option<String>) = c.query_row(
        "SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1",
        [user_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let code = jurisdiction
        .as_deref()
        .filter(|s| !crate::ruby::blank(s))
        .unwrap_or("US");
    let choices = [("US", "US", 30), ("GB", "UK", 30), ("IE", "IE", 28)];
    let label = |name: &str, days: i64| {
        i18n::t(
            ctx.locale,
            "settings.wash_sale.option",
            &[("days", Arg::Count(days)), ("country", Arg::Text(name))],
        )
    };
    let selected = choices
        .iter()
        .find(|(name, _, _)| *name == code)
        .map(|(_, name, days)| label(name, *days))
        .unwrap_or_default();
    let options = choices
        .iter()
        .map(|(name, country, days)| {
            format!(
                "<option{} value=\"{}\">{}</option>",
                if *name == code {
                    " selected=\"selected\""
                } else {
                    ""
                },
                name,
                escape(&label(country, *days))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let select=format!("\n        <div class=\"sinput sinput--select\" data-controller=\"form--select-display\">\n          <span data-form--select-display-target=\"label\">{}</span>\n          <select name=\"wash_sale[jurisdiction]\" id=\"wash_sale_jurisdiction\" data-action=\"change-&gt;form--select-display#update\">{options}</select>\n        </div>\n",escape(&selected));
    let sentence = i18n::t(
        ctx.locale,
        "settings.wash_sale.sentence_html",
        &[("select_html", Arg::Html(&select))],
    );
    let question = format!(
        r#"<div class="wash-sale-question">
  <p class="text-inactive">{}</p>

  <label class="form__radio">
    <input type="radio" name="wash_sale[enabled]" id="wash_sale_enabled_1" value="1"{} data-form--checkbox-enables-target="checkbox" />
    <span class="conversational conversational--small">
      {sentence}
    </span>
  </label>

  <label class="form__radio">
    <input type="radio" name="wash_sale[enabled]" id="wash_sale_enabled_0" value="0"{} data-form--checkbox-enables-target="checkbox" />
    <span class="conversational conversational--small">{}</span>
  </label>
</div>
"#,
        ctx.t("settings.wash_sale.prompt_body"),
        if enabled == Some(true) {
            " checked=\"checked\""
        } else {
            ""
        },
        if enabled == Some(false) {
            " checked=\"checked\""
        } else {
            ""
        },
        ctx.t("settings.wash_sale.prompt_decline")
    );
    // The locked list is read even with protection off, as Rails does; rendering is conditional.
    let mut statement=c.prepare("SELECT assets.symbol,wash_sale_locks.buy_locked_until FROM wash_sale_locks JOIN assets ON assets.id=wash_sale_locks.asset_id WHERE user_id=?1 AND buy_locked_until>?2 ORDER BY buy_locked_until,wash_sale_locks.id")?;
    let locks = statement
        .query_map((user_id, crate::codec::format_time(ctx.now)), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let locked = if locks.is_empty() {
        format!(
            "\n          <small>{}</small>\n",
            ctx.t("settings.wash_sale.locked_none")
        )
    } else {
        let mut entries = String::new();
        for (symbol, until) in locks {
            let until = crate::codec::parse_time(&until)
                .map_err(|_| WebError::Config("invalid wash-sale deadline".into()))?;
            let days = (until.date_naive() - ctx.now.date_naive()).num_days();
            let text = i18n::t(
                ctx.locale,
                "settings.wash_sale.locked_entry",
                &[("days", Arg::Count(days))],
            );
            entries.push_str(&format!("\n              <li>\n                <b>{}</b>\n                {}\n                <small>{}</small>\n              </li>\n",escape(&symbol),escape(&text),(until-chrono::Duration::days(1)).format("%Y-%m-%d")));
        }
        format!("\n          <ul class=\"settings-locked\">{entries}          </ul>\n")
    };
    Ok(Wash {
        v: ctx,
        csrf,
        enabled,
        question,
        locked,
    }
    .render()?)
}
pub async fn account(
    app: &App,
    ctx: &Ctx,
    status: StatusCode,
    draft: Option<Draft>,
    flash_now: Vec<(&'static str, String)>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(crate::web::auth::unauthenticated(ctx));
    };
    let (v, inner, owner) = (ctx.clone(), app.clone(), user.clone());
    let csrf = ctx.csrf_token();
    let token = csrf.clone();
    let (name, pending_email, wash_widget, shell, registration_open, advanced, email_widget) = app
        .db(move |c| {
            let (name, email): (Option<String>, Option<String>) = c.query_row(
                "SELECT name,unconfirmed_email FROM users WHERE id=?1",
                [owner.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let read = |key: &str| crate::web::auth::app_config(c, &inner.cipher, key);
            let registration_open = read("registration_open")?.as_deref() == Some("true");
            let advanced: bool = c.query_row(
                "SELECT advanced_bots_enabled FROM users WHERE id=?1",
                [owner.id],
                |r| r.get(0),
            )?;
            let provider = read("smtp_provider")?;
            let env_available = inner
                .settings_smtp
                .address
                .as_deref()
                .is_some_and(|s| !crate::ruby::blank(s));
            let host = read("smtp_host")?;
            let port = read("smtp_port")?;
            let custom = !env_available && provider.as_deref() == Some("custom_smtp");
            let has_credentials = read("smtp_username")?.is_some_and(|s| !crate::ruby::blank(&s))
                && read("smtp_password")?.is_some_and(|s| !crate::ruby::blank(&s));
            let configured_label = i18n::t(
                v.locale,
                "settings.email_notifications.smtp_configured_html",
                &[(
                    "host",
                    Arg::Text(
                        host.as_deref()
                            .filter(|s| !crate::ruby::blank(s))
                            .unwrap_or("smtp.gmail.com"),
                    ),
                )],
            );
            let email_widget = EmailNotifications {
                v: &v,
                csrf: &token,
                mail_configured: env_available
                    || provider.as_deref().is_some_and(|s| !crate::ruby::blank(s)),
                env_available,
                env_name: inner.settings_smtp_provider_name.clone(),
                custom,
                env_configured: env_available || provider.as_deref() == Some("env_smtp"),
                has_credentials,
                host,
                port,
                configured_label,
            }
            .render()?;
            Ok((
                name.unwrap_or_default(),
                email.unwrap_or_default(),
                wash(c, &v, &token, owner.id)?,
                Shell::load(c, &inner, &owner)?,
                registration_open,
                advanced,
                email_widget,
            ))
        })
        .await?;
    let (locale_label, locale_options) =
        language_options(user.locale.as_deref().unwrap_or(i18n::DEFAULT));
    let (zone_label, zone_options) = zones(ctx, &user.time_zone);
    let local = timezone::local(ctx.now, &user.time_zone);
    let local_time = local.format("%Y-%m-%d %I:%M %p").to_string();
    let pending_label = i18n::t(
        ctx.locale,
        "devise.registrations.edit.awaiting_confirmation",
        &[("email", Arg::Text(&pending_email))],
    );
    let draft = draft.unwrap_or_default();
    let mut user = user;
    if let Some(email) = draft.email {
        user.email = email;
    }
    let body = Account {
        v: ctx,
        csrf: &csrf,
        user: &user,
        name: draft.name.unwrap_or(name),
        version: include_str!("../../../../src-tauri/Cargo.toml")
            .lines()
            .find_map(|line| {
                line.strip_prefix("version = ")
                    .and_then(|s| s.strip_prefix('"'))
                    .and_then(|s| s.strip_suffix('"'))
            })
            .unwrap_or("0.0.0"),
        locale_label,
        locale_options,
        zone_label,
        zone_options,
        zone_id: timezone::zone(&user.time_zone).map_or("UTC".into(), |z| z.name().into()),
        local_time,
        two_fa: user.otp_enabled,
        two_fa_button: ctx.t(if user.otp_enabled {
            "helpers.label.settings.disable_two_fa"
        } else {
            "helpers.label.settings.enable_two_fa"
        }),
        length_label: i18n::t(
            ctx.locale,
            "devise.passwords.validations.length",
            &[("count", Arg::Count(8))],
        ),
        errors: draft.errors,
        messages: draft.messages,
        pending_email,
        pending_label,
        excluded_registration_settings: Registration {
            v: ctx,
            csrf: &csrf,
            registration_open,
        }
        .render()?,
        excluded_advanced_bots: Advanced {
            v: ctx,
            csrf: &csrf,
            advanced,
        }
        .render()?,
        excluded_email_notifications: email_widget,
        wash_widget,
    }
    .render()?;
    shell::application(
        ctx,
        &csrf,
        &user,
        &shell,
        Page {
            status,
            body,
            flash_now,
        },
    )
}
