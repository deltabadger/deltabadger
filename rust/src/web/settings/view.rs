use super::{account::Draft, dialog, sections};
use crate::web::{
    auth::User,
    i18n::{self, escape, Arg},
    layout::{Ctx, Page},
    shell::{self, Shell},
    timezone, App, WebError,
};
use axum::{http::StatusCode, response::Response};
use chrono::Offset;
use rusqlite::Connection;

pub(super) fn language_options(selected: &str) -> (String, String) {
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
pub(super) fn zones(ctx: &Ctx, selected: &str) -> (String, String) {
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
/// settings/widgets/_registration.
pub(super) fn registration(ctx: &Ctx, csrf: &str, registration_open: bool) -> Result<String, WebError> {
    Ok(sections::registration(ctx, csrf, registration_open))
}
/// settings/widgets/_advanced_bots.
pub(super) fn advanced(ctx: &Ctx, csrf: &str, advanced: bool) -> Result<String, WebError> {
    Ok(sections::advanced(ctx, csrf, advanced))
}
/// settings/widgets/_email_notifications, as AppConfig and the environment hold it now.
pub(super) fn email(c: &Connection, app: &App, ctx: &Ctx, csrf: &str, show_form: bool) -> Result<String, WebError> {
    let read = |key: &str| crate::web::auth::app_config(c, &app.cipher, key);
    let provider = read("smtp_provider")?;
    let env_available = app.settings_smtp.address.as_deref().is_some_and(|s| !crate::ruby::blank(s));
    let host = read("smtp_host")?;
    let port = read("smtp_port")?;
    let has_credentials = read("smtp_username")?.is_some_and(|s| !crate::ruby::blank(&s))
        && read("smtp_password")?.is_some_and(|s| !crate::ruby::blank(&s));
    let configured_hint = i18n::t(
        ctx.locale,
        "settings.email_notifications.smtp_configured_html",
        &[("host", Arg::Text(host.as_deref().filter(|s| !crate::ruby::blank(s)).unwrap_or("smtp.gmail.com")))],
    );
    Ok(sections::email_notifications(
        ctx,
        csrf,
        &sections::Mail {
            env_available,
            env_name: &app.settings_smtp_provider_name,
            configured: !env_available && provider.as_deref() == Some("custom_smtp"),
            show_form,
            configured_hint: &configured_hint,
            credentials: has_credentials.then_some((host, port)),
        },
    ))
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
    Ok(sections::wash(ctx, csrf, &question, enabled.is_some(), enabled == Some(true), &locked))
}
/// SettingsHelper#settings_connect_visible?: Connect holds container-wide sources (admin) and IBKR.
pub(super) fn connect_visible(c: &Connection, app: &App, user: &User) -> Result<bool, WebError> {
    Ok(user.admin
        || app.config.market_data_url
        || c.query_row(
            "SELECT EXISTS(SELECT 1 FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0 AND e.type='Exchanges::Ibkr')",
            [user.id],
            |r| r.get::<_, bool>(0),
        )?)
}

/// A section of the dialog as a page (or as the frame it opens in).
#[allow(clippy::too_many_arguments)]
pub(super) fn section_page(ctx: &Ctx, csrf: &str, user: &User, shell_data: &Shell, section: &str, connect: bool, pane: &str, status: StatusCode, flash_now: Vec<(&'static str, String)>) -> Result<Response, WebError> {
    let body = dialog::shell(ctx, section, connect, pane);
    shell::application(ctx, csrf, user, shell_data, Page { status, body, flash_now })
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
    let (name, pending_email, wash_widget, shell_data, registration_open, advanced, email_widget, connect) = app
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
            let email_widget = self::email(c, &inner, &v, &token, false)?;
            Ok((
                name.unwrap_or_default(),
                email.unwrap_or_default(),
                wash(c, &v, &token, owner.id)?,
                Shell::load(c, &inner, &owner)?,
                registration_open,
                advanced,
                email_widget,
                connect_visible(c, &inner, &owner)?,
            ))
        })
        .await?;
    let (_, locale_options) = language_options(user.locale.as_deref().unwrap_or(i18n::DEFAULT));
    let draft = draft.unwrap_or_default();
    // A failed email change keeps the address it had: the row shows what is stored, not what was typed.
    let hint = if pending_email.is_empty() {
        user.email.clone()
    } else {
        i18n::text(ctx.locale, "devise.registrations.edit.awaiting_confirmation", &[("email", Arg::Text(&pending_email))])
    };
    // Rails re-renders the whole section on a failed save, and current_user.errors reaches every row: the
    // email and password rows open, and each field shows its own attribute's message.
    let errors = sections::Errors(&draft.messages);
    let registration = registration(ctx, &csrf, registration_open)?;
    let pane = sections::account(
        ctx,
        &sections::Account {
            name: sections::name(ctx, &csrf, &draft.name.clone().unwrap_or(name), &errors),
            email: sections::email(ctx, &csrf, &hint, &errors),
            password: sections::password(ctx, &csrf, draft.email.as_deref().unwrap_or(&user.email), &errors),
            locale: sections::locale(ctx, &csrf, &locale_options),
            time_zone: time_zone_row(ctx, &csrf, &user.time_zone),
            two_fa: sections::two_fa(ctx, user.otp_enabled),
            advanced: self::advanced(ctx, &csrf, advanced)?,
            wash: wash_widget,
            admin: user.admin.then_some((registration.as_str(), email_widget.as_str())),
        },
    );
    section_page(ctx, &csrf, &user, &shell_data, "account", connect, &pane, status, flash_now)
}

/// settings/widgets/_time_zone for `zone`.
pub(super) fn time_zone_row(ctx: &Ctx, csrf: &str, zone: &str) -> String {
    let (_, options) = zones(ctx, zone);
    let local_time = timezone::local(ctx.now, zone).format("%Y-%m-%d %I:%M %p").to_string();
    sections::time_zone(ctx, csrf, &timezone::strict(zone).map_or("UTC".into(), |z| z.name().to_string()), &local_time, &options)
}
