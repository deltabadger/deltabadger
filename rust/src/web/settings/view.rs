//! The settings dialog (app/views/settings/_shell.html.erb, show.html.erb, sections/ and widgets/): a sidebar of
//! sections and the pane they open, each row drawn by SettingsHelper#settings_row.
use super::account::Draft;
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

/// SettingsHelper::SETTINGS_SECTIONS: section, label, icon.
const SIDEBAR: [(&str, &str, &str); 5] = [
    ("account", "links.account", include_str!("../../../templates/svg/_24x24_account.html")),
    ("api_keys", "settings.index.api_keys_section", include_str!("../../../templates/svg/_24x24_lock.html")),
    ("connect", "links.connections", include_str!("../../../templates/svg/_24x24_sync.html")),
    ("mcp", "settings.sections.mcp_server", include_str!("../../../templates/svg/_24x24_chat.html")),
    ("api", "settings.rest.title", include_str!("../../../templates/svg/_24x24_doc.html")),
];
const CLOSE: &str = include_str!("../../../templates/svg/_24x24_close.html");
const BACK: &str = include_str!("../../../templates/svg/_24x24_back.html");
const COPY: &str = include_str!("../../../templates/svg/_16x16_copy.html");

/// SettingsHelper#settings_row: the label and its hint on the left, the control on the right. `label`, `hint`, `icon`
/// and `control` are markup; `attrs` are the row's other attributes, each with its leading space. A row given no block
/// has no control at all; a block that writes nothing leaves the control empty.
pub fn row(label: &str, hint: Option<&str>, icon: Option<&str>, class: &str, attrs: &str, control: Option<&str>) -> String {
    let class = if class.is_empty() { String::new() } else { format!(" {class}") };
    let text = if icon.is_some() { "srow__text srow__text--icon" } else { "srow__text" };
    let icon = icon.map(|icon| format!("<div class=\"srow__icon\">{icon}</div>")).unwrap_or_default();
    let hint = hint.filter(|hint| !crate::ruby::blank(hint)).map(|hint| format!("<div class=\"srow__hint\">{hint}</div>")).unwrap_or_default();
    let control = match control {
        None => String::new(),
        Some("") => "<div class=\"srow__control\"></div>".into(),
        Some(markup) => format!("<div class=\"srow__control\">\n    {markup}</div>"),
    };
    format!("<div class=\"srow{class}\"{attrs}><div class=\"{text}\">{icon}<div class=\"srow__label\">{label}</div>{hint}</div>{control}</div>")
}

/// The opening of a `form_with` that PATCHes (or, with `patch` false, POSTs), with its hidden fields.
fn form(ctx: &Ctx, csrf: &str, attrs: &str, path: &str, patch: bool) -> String {
    format!(
        "<form{attrs} action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">{}<input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />",
        escape(&ctx.path(path)),
        if patch { "<input type=\"hidden\" name=\"_method\" value=\"patch\" />" } else { "" }
    )
}

/// `button_to` with the app's classes; `delete` adds the method override, `params` become hidden fields after the token.
fn button_to(csrf: &str, label: &str, url: &str, class: &str, delete: bool, params: &[(&str, &str)]) -> String {
    let params: String = params.iter().map(|(name, value)| format!("<input type=\"hidden\" name=\"{}\" value=\"{}\" />", escape(name), escape(value))).collect();
    format!(
        "<form class=\"button_to\" method=\"post\" action=\"{}\">{}<button class=\"{class}\" type=\"submit\">{label}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />{params}</form>",
        escape(url),
        if delete { "<input type=\"hidden\" name=\"_method\" value=\"delete\" />" } else { "" }
    )
}

/// settings/_switch: a switch that saves itself. `state` is "on", "partial" or "off"; `enabled` is what a click sends
/// under `name`; `fields` are the other hidden fields (a nil value has no value attribute).
#[allow(clippy::too_many_arguments)]
pub fn switch(ctx: &Ctx, csrf: &str, url: &str, frame: &str, name: &str, state: &str, enabled: &str, title: Option<&str>, fields: &[(&str, Option<&str>)]) -> String {
    // `SecureRandom.hex(4)`: pairs the checkbox with the row-wide label for it.
    let id = format!("switch-{:08x}", rand::random::<u32>());
    let fields: String = fields
        .iter()
        .map(|(field, value)| match value {
            Some(value) => format!("<input value=\"{}\" type=\"hidden\" name=\"{}\" />", escape(value), escape(field)),
            None => format!("<input type=\"hidden\" name=\"{}\" />", escape(field)),
        })
        .collect();
    let title = title.map(|title| format!(" title=\"{title}\" aria-label=\"{title}\"")).unwrap_or_default();
    format!(
        "{}\n  {fields}\n  <input value=\"{}\" type=\"hidden\" name=\"{}\" />\n  <label class=\"toggle toggle--small{}\"{title}>\n    <input type=\"checkbox\" id=\"{id}\"{} data-action=\"change->form--submit#submit\" />\n    <div class=\"toggle__style\"></div>\n</label>  <label class=\"sswitch__hit\" for=\"{id}\" aria-hidden=\"true\"></label>\n</form>\n",
        form(ctx, csrf, &format!(" class=\"sswitch\" data-controller=\"form--submit\" data-turbo-frame=\"{}\"", escape(frame)), url, true),
        escape(enabled),
        escape(name),
        if state == "partial" { " toggle--partial" } else { "" },
        if state == "off" { "" } else { " checked" },
    )
}

/// settings/_confirm: a destructive button that asks first, in place.
pub fn confirm(csrf: &str, label: &str, text: &str, confirm: &str, url: &str, delete: bool, params: &[(&str, &str)]) -> String {
    format!(
        "<details class=\"sconfirm\">\n  <summary class=\"rbutton rbutton--outline\">{label}</summary>\n  <div class=\"sconfirm__panel\">\n    <p>{text}</p>\n    {}\n  </div>\n</details>\n",
        button_to(csrf, confirm, url, "rbutton rbutton--outline rbutton--danger", delete, params)
    )
}

/// settings/widgets/_copy_tag: a pill that copies `value`, showing it without its scheme.
pub fn copy_tag(ctx: &Ctx, value: &str, id: Option<&str>) -> String {
    let text = value.strip_prefix("https://").or_else(|| value.strip_prefix("http://")).unwrap_or(value);
    format!(
        "<span class=\"tag tag--copy\"{} title=\"{}\" data-controller=\"clipboard\" data-clipboard-text-value=\"{}\" data-action=\"click-&gt;clipboard#copy\">{COPY}{}</span>\n",
        id.map(|id| format!(" id=\"{id}\"")).unwrap_or_default(),
        ctx.t("button.copy"),
        escape(value),
        escape(text)
    )
}

/// The page behind a frame: `turbo_frame_tag` with a class.
fn item(id: &str, body: &str) -> String {
    format!("<turbo-frame class=\"sitem\" id=\"{id}\">\n  {body}</turbo-frame>\n")
}

/// SettingsHelper#settings_connect_visible?: Connect holds the container-wide sources (admin) and IBKR.
pub fn connect_visible(c: &Connection, app: &App, user: &User) -> Result<bool, WebError> {
    Ok(user.admin || app.config.market_data_url || ibkr_key(c, user.id)?.is_some())
}

/// The user's IBKR trading key: (status, updated_at).
fn ibkr_key(c: &Connection, user: i64) -> Result<Option<(i64, String)>, WebError> {
    use rusqlite::OptionalExtension;
    Ok(c.query_row(
        "SELECT k.status,k.updated_at FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND e.type='Exchanges::Ibkr' AND k.key_type=0 ORDER BY k.id LIMIT 1",
        [user],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

/// settings/_shell: the dialog, its sidebar with `section` active, and the pane.
fn dialog(ctx: &Ctx, section: &str, connect: bool, pane: &str) -> String {
    let items: Vec<String> = SIDEBAR
        .iter()
        .filter(|(name, _, _)| *name != "connect" || connect)
        .map(|(name, label, icon)| {
            let active = *name == section;
            format!(
                "<a class=\"settings__item{}\"{} href=\"{}\">\n                {icon}<span>{}</span>\n</a>",
                if active { " settings__item--active" } else { "" },
                if active { " aria-current=\"page\"" } else { "" },
                escape(&ctx.path(&format!("/settings/{name}"))),
                ctx.t(label)
            )
        })
        .collect();
    let close = if ctx.turbo_frame.is_some() { String::new() } else { format!("data-modal--base-close-url-value=\"{}\"", escape(&ctx.path("/bots"))) };
    format!(
        "<turbo-frame id=\"modal\">\n  <dialog class=\"dialog sdialog\" data-controller=\"modal--base\" data-action=\"click->modal--base#clickOutside\"\n          {close}>\n    <div class=\"settings\">\n      <button name=\"button\" type=\"button\" class=\"settings__close\" aria-label=\"{}\" data-action=\"modal--base#animateOutCloseAndCleanUp\">\n        {CLOSE}\n</button>      <turbo-frame class=\"settings__frame\" id=\"settings_pane\">\n        <nav class=\"settings__nav\">\n          <div class=\"settings__items\" data-controller=\"settings-nav\">\n              {}          </div>\n          <div class=\"settings__version\">{}</div>\n        </nav>\n        <div class=\"settings__pane\" tabindex=\"-1\" autofocus>\n          {pane}\n        </div>\n</turbo-frame>    </div>\n  </dialog>\n</turbo-frame>\n",
        ctx.t("button.close"),
        items.join("              "),
        escape(crate::web::mcp::app_version())
    )
}

/// The account section's widgets read the user as the request left it: after a failed save, with the values it tried.
pub struct AccountState {
    pub name: Option<String>,
    pub email: String,
    pub email_was: String,
    pub unconfirmed: Option<String>,
    pub draft: Draft,
}

impl AccountState {
    pub fn load(c: &Connection, user: &User, draft: Draft) -> Result<Self, WebError> {
        let (name, email, unconfirmed): (Option<String>, String, Option<String>) =
            c.query_row("SELECT name,email,unconfirmed_email FROM users WHERE id=?1", [user.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(AccountState {
            name: draft.name.clone().or(name),
            email: draft.email.clone().unwrap_or_else(|| email.clone()),
            email_was: email,
            unconfirmed: unconfirmed.filter(|s| !s.is_empty()),
            draft,
        })
    }
    fn any(&self) -> bool { !self.draft.errors.is_empty() }
    /// `errors.full_messages.to_sentence`.
    fn sentence(&self, locale: &str) -> String {
        let mut full = vec![];
        for field in &self.draft.errors {
            let name = crate::web::setup::attribute_name(locale, field);
            for message in self.draft.messages.get(field).into_iter().flatten() {
                full.push(
                    i18n::text_chain(locale, &["errors.format".into()], &[("attribute", Arg::Text(&name)), ("message", Arg::Text(message))])
                        .unwrap_or_else(|| format!("{name} {message}")),
                );
            }
        }
        escape(&crate::web::bot::draft::sentence(&full.iter().map(String::as_str).collect::<Vec<_>>(), locale))
    }
    /// The field and, when it has errors, config/initializers/inline_form_errors.rb around it. `input` is the tag
    /// without its class, `class` its class.
    fn field(&self, field: &str, input: &str, class: &str) -> String {
        let Some(messages) = self.draft.errors.iter().any(|f| f == field).then(|| self.draft.messages.get(field)).flatten() else {
            let class = if class.is_empty() { String::new() } else { format!(" class=\"{class}\"") };
            return input.replacen("<input", &format!("<input{class}"), 1);
        };
        let text = escape(&messages.join(", "));
        let mut chars = text.chars();
        let text = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
        // Nokogiri writes a boolean attribute bare when it rebuilds the tag.
        let input = input.replace("readonly=\"readonly\"", "readonly");
        format!(
            "                   {}\n             <div class=\"form__info form__info--invalid\">{text}</div>\n",
            input.replacen("<input", &format!("<input class=\"{class} is-invalid\""), 1)
        )
    }
}

fn name_widget(ctx: &Ctx, csrf: &str, st: &AccountState) -> String {
    let label = ctx.t("settings.index.name_section");
    let errors: Vec<&str> = if st.draft.errors.iter().any(|f| f == "name") {
        st.draft.messages.get("name").into_iter().flatten().map(String::as_str).collect()
    } else {
        vec![]
    };
    let hint = escape(&crate::web::bot::draft::sentence(&errors, ctx.locale));
    let value = st.name.as_deref().map(|name| format!(" value=\"{}\"", escape(name))).unwrap_or_default();
    let input = format!(
        "<input required=\"required\" pattern=\"^\\p{{L}}+(\\s+\\p{{L}}+)*$\" autocomplete=\"name\" aria-label=\"{label}\" type=\"text\"{value} name=\"user[name]\" id=\"user_name\" />"
    );
    let form = format!(
        "{}\n      {}\n      {}\n</form>",
        form(ctx, csrf, " class=\"sform\" data-turbo-frame=\"update_name_form\"", "/settings/update_name", true),
        st.field("name", &input, "rinput rinput--outline rinput--text"),
        submit(ctx, "button.save")
    );
    item("update_name_form", &row(&label, Some(&hint), None, "", "", Some(&form)))
}

fn submit(ctx: &Ctx, key: &str) -> String {
    let label = ctx.t(key);
    format!("<input type=\"submit\" name=\"commit\" value=\"{label}\" class=\"rbutton rbutton--outline\" data-disable-with=\"{label}\" />")
}

fn error_line(ctx: &Ctx, st: &AccountState) -> String {
    if st.any() { format!("<p class=\"sform__error\">{}</p>", st.sentence(ctx.locale)) } else { String::new() }
}

fn change_summary(ctx: &Ctx, label: &str, hint: Option<&str>) -> String {
    format!(
        "<details {}>\n    <summary>\n      {}    </summary>\n",
        "{open}",
        row(label, hint, None, "", "", Some(&format!("<span class=\"rbutton rbutton--outline\">{}</span>\n", ctx.t("settings.sections.change"))))
    )
}

fn email_widget(ctx: &Ctx, csrf: &str, st: &AccountState) -> String {
    let hint = match &st.unconfirmed {
        Some(email) => i18n::t(ctx.locale, "devise.registrations.edit.awaiting_confirmation", &[("email", Arg::Text(email))]),
        None => escape(&st.email_was),
    };
    let (new_email, current) = (ctx.t("helpers.label.user.new_email"), ctx.t("helpers.label.user.current_password"));
    let email = format!(
        "<input value=\"\" autocomplete=\"username\" required=\"required\" pattern=\"^[a-zA-Z0-9._%+\\-]+@[a-zA-Z0-9.\\-]+\\.[a-zA-Z]{{2,}}$\" placeholder=\"{new_email}\" aria-label=\"{new_email}\" type=\"email\" name=\"user[email]\" id=\"user_email\" />"
    );
    let password = format!(
        "<input autocomplete=\"current-password\" required=\"required\" placeholder=\"{current}\" aria-label=\"{current}\" type=\"password\" name=\"user[current_password]\" id=\"user_current_password\" />"
    );
    let body = format!(
        "{}    {}\n      {}\n      {}\n      {}\n      {}\n</form>  </details>\n",
        change_summary(ctx, &ctx.t("settings.index.email_section"), Some(&hint)).replace("{open}", if st.any() { "open" } else { "" }),
        form(ctx, csrf, " class=\"sform sform--stack\" data-turbo-frame=\"update_email_form\"", "/settings/update_email", true),
        error_line(ctx, st),
        st.field("email", &email, "rinput rinput--outline rinput--text"),
        st.field("current_password", &password, "rinput rinput--outline rinput--text"),
        submit(ctx, "button.save")
    );
    item("update_email_form", &body)
}

fn password_widget(ctx: &Ctx, csrf: &str, st: &AccountState) -> String {
    let (current, new, confirmation) =
        (ctx.t("helpers.label.user.current_password"), ctx.t("helpers.label.user.new_password"), ctx.t("helpers.label.user.new_password_confirmation"));
    let email = format!(
        "<input autocomplete=\"username\" id=\"email_for_password_update\" readonly=\"readonly\" hidden=\"hidden\" type=\"email\" value=\"{}\" name=\"user[email]\" />",
        escape(&st.email)
    );
    let current_input = format!(
        "<input autocomplete=\"current-password\" id=\"current_password_for_password_update\" required=\"required\" placeholder=\"{current}\" aria-label=\"{current}\" type=\"password\" name=\"user[current_password]\" />"
    );
    let password = format!(
        "<input autocomplete=\"new-password\" required=\"required\" pattern=\"(?=.*[a-z])(?=.*[A-Z])(?=.*\\d)(?=.*[\\W_]).{{8,}}\" placeholder=\"{new}\" aria-label=\"{new}\" data-form--password-strength-target=\"input\" data-form--password-match-target=\"input0\" data-action=\"input-&gt;form--password-strength#checkPasswordStrength\" type=\"password\" name=\"user[password]\" id=\"user_password\" />"
    );
    let confirm = format!(
        "<input autocomplete=\"new-password\" required=\"required\" placeholder=\"{confirmation}\" aria-label=\"{confirmation}\" data-form--password-match-target=\"input1\" data-action=\"input-&gt;form--password-match#checkPasswordMatch\" type=\"password\" name=\"user[password_confirmation]\" id=\"user_password_confirmation\" />"
    );
    let rule = |target: &str, key: &str| format!("          <li class=\"form__validation\" data-form--password-{target}>{}</li>\n", ctx.t(key));
    let requirements = format!(
        "<div data-form--password-strength-target=\"requirementsList\" class=\"hidden\">\n        <ul class=\"password-requirements\">\n          <li class=\"form__validation\" data-form--password-strength-target=\"length\">{}</li>\n{}{}{}{}{}        </ul>\n      </div>",
        i18n::t(ctx.locale, "devise.passwords.validations.length", &[("count", Arg::Count(8))]),
        rule("strength-target=\"uppercase\"", "devise.passwords.validations.uppercase"),
        rule("strength-target=\"lowercase\"", "devise.passwords.validations.lowercase"),
        rule("strength-target=\"digit\"", "devise.passwords.validations.digit"),
        rule("strength-target=\"symbol\"", "devise.passwords.validations.symbol"),
        rule("match-target=\"match\"", "devise.passwords.validations.match"),
    );
    let attrs = " class=\"sform sform--stack\" data-controller=\"form--password-strength form--password-match\" data-turbo-frame=\"update_password_form\" data-form--password-match-success-class-value=\"form__validation--success\" data-form--password-strength-success-class-value=\"form__validation--success\" data-form--password-strength-length-pattern-value=\".{8,}\" data-form--password-strength-uppercase-pattern-value=\"(?=.*[A-Z])\" data-form--password-strength-lowercase-pattern-value=\"(?=.*[a-z])\" data-form--password-strength-digit-pattern-value=\"(?=.*\\d)\" data-form--password-strength-symbol-pattern-value=\"(?=.*[\\W_])\"";
    let body = format!(
        "{}    {}\n      {}\n      {}\n      {}\n      {}\n      {}\n      {requirements}\n      {}\n</form>  </details>\n",
        change_summary(ctx, &ctx.t("settings.index.password_section"), None).replace("{open}", if st.any() { "open" } else { "" }),
        form(ctx, csrf, attrs, "/settings/update_password", true),
        error_line(ctx, st),
        st.field("email", &email, ""),
        st.field("current_password", &current_input, "rinput rinput--outline rinput--text"),
        st.field("password", &password, "rinput rinput--outline rinput--text"),
        st.field("password_confirmation", &confirm, "rinput rinput--outline rinput--text"),
        submit(ctx, "button.save")
    );
    item("update_password_form", &body)
}

fn language_options(selected: &str) -> String {
    let langs = [
        ("en", "English"), ("de", "Deutsch"), ("nl", "Nederlands"), ("fr", "Français"), ("es", "Español"), ("pt", "Português"),
        ("it", "Italiano"), ("pl", "Polski"), ("ru", "Русский"), ("cs", "Čeština"), ("sk", "Slovenčina"), ("da", "Dansk"),
        ("sv", "Svenska"), ("el", "Ελληνικά"), ("bg", "Български"),
    ];
    langs
        .iter()
        .map(|(code, label)| format!("<option{} value=\"{code}\">{label}</option>", if *code == selected { " selected=\"selected\"" } else { "" }))
        .collect::<Vec<_>>()
        .join("\n")
}

fn zone_label(name: &str, offset: i32) -> String {
    format!("(GMT {}{}:{:02}) {}", if offset >= 0 { "+" } else { "-" }, offset.abs() / 3600, offset.abs() % 3600 / 60, name)
}

/// SettingsHelper#sorted_time_zone_options, `selected` marked.
fn zone_options(ctx: &Ctx, selected: &str) -> String {
    let mut zones: Vec<_> = timezone::names()
        .map(|name| (timezone::local(ctx.now, name).offset().fix().local_minus_utc(), name.as_str()))
        .collect();
    zones.sort();
    zones
        .iter()
        .map(|(offset, name)| {
            format!(
                "<option data-offset=\"{offset}\"{} value=\"{}\">{}</option>",
                if *name == selected { " selected=\"selected\"" } else { "" },
                escape(name),
                escape(&zone_label(name, *offset))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A select that submits its form on change.
fn select_form(ctx: &Ctx, csrf: &str, frame: &str, path: &str, label: &str, field: &str, options: &str) -> String {
    format!(
        "{}\n      <select class=\"rinput rinput--outline rinput--select\" aria-label=\"{label}\" data-action=\"change-&gt;form--submit#submit\" name=\"user[{field}]\" id=\"user_{field}\">{options}</select>\n</form>",
        form(ctx, csrf, &format!(" data-controller=\"form--submit\" data-turbo-frame=\"{frame}\""), path, true)
    )
}

pub fn locale_widget(ctx: &Ctx, csrf: &str, user: &User) -> String {
    let label = ctx.t("settings.sections.language");
    let options = language_options(user.locale.as_deref().unwrap_or(i18n::DEFAULT));
    let form = select_form(ctx, csrf, "update_locale_form", "/settings/update_locale", &label, "locale", &options);
    item("update_locale_form", &row(&label, None, None, "", "", Some(&form)))
}

pub fn time_zone_widget(ctx: &Ctx, csrf: &str, user: &User) -> String {
    let label = ctx.t("settings.sections.time_zone");
    let hint = format!(
        "<span data-controller=\"clock\" data-clock-time-zone-value=\"{}\">    {}\n    <b data-clock-target=\"timestamp\">{}</b>\n</span>",
        escape(&timezone::strict(&user.time_zone).map_or("UTC".into(), |z| z.name().to_string())),
        ctx.t("settings.language_and_timezone.current_time_is"),
        timezone::local(ctx.now, &user.time_zone).format("%Y-%m-%d %I:%M %p")
    );
    let form = select_form(ctx, csrf, "update_time_zone_form", "/settings/update_time_zone", &label, "time_zone", &zone_options(ctx, &user.time_zone));
    item("update_time_zone_form", &row(&label, Some(&hint), None, "", "", Some(&form)))
}

pub fn two_fa_widget(ctx: &Ctx, enabled: bool) -> String {
    let link = format!(
        "<a class=\"rbutton rbutton--outline\" href=\"{}\">{}</a>\n",
        escape(&ctx.path("/settings/edit_two_fa")),
        ctx.t(if enabled { "helpers.label.settings.disable_two_fa" } else { "helpers.label.settings.enable_two_fa" })
    );
    let hint = ctx.t(if enabled { "helpers.label.settings.enabled" } else { "helpers.label.settings.disabled" });
    item("two_fa", &row(&ctx.t("settings.index.two_factor_section"), Some(&hint), None, "", "", Some(&link)))
}

/// settings/widgets/_registration.
pub(super) fn registration(ctx: &Ctx, csrf: &str, open: bool) -> Result<String, WebError> {
    let title = ctx.t("settings.index.registration_section");
    let switch = switch(ctx, csrf, "/settings/update_registration", "registration_settings", "registration_open", if open { "on" } else { "off" }, if open { "0" } else { "1" }, Some(&title), &[]);
    Ok(item("registration_settings", &row(&title, None, None, "", "", Some(&switch))))
}

/// settings/widgets/_advanced_bots.
pub(super) fn advanced(ctx: &Ctx, csrf: &str, on: bool) -> Result<String, WebError> {
    let hint = ctx.t("settings.advanced_bots.toggle_label");
    let switch = switch(ctx, csrf, "/settings/update_advanced_bots", "advanced_bots", "enabled", if on { "on" } else { "off" }, if on { "0" } else { "1" }, Some(&hint), &[]);
    Ok(item("advanced_bots", &row(&ctx.t("settings.advanced_bots.title"), Some(&hint), None, "", "", Some(&switch))))
}

/// settings/widgets/_email_notifications, as AppConfig and the environment hold it now.
pub(super) fn email(c: &Connection, app: &App, ctx: &Ctx, csrf: &str, show_form: bool) -> Result<String, WebError> {
    let read = |key: &str| crate::web::auth::app_config(c, &app.cipher, key);
    let present = |value: &Option<String>| value.as_deref().is_some_and(|s| !crate::ruby::blank(s));
    let env_available = app.settings_smtp.address.as_deref().is_some_and(|s| !crate::ruby::blank(s));
    let provider = read("smtp_provider")?;
    let (host, port) = (read("smtp_host")?, read("smtp_port")?);
    let configured = !env_available && provider.as_deref() == Some("custom_smtp");
    let has_credentials = present(&read("smtp_username")?) && present(&read("smtp_password")?);
    let title = ctx.t("settings.index.email_notifications_section");
    let send_test = button_to(csrf, &ctx.t("settings.email_notifications.send_test"), &ctx.path("/settings/send_test_email"), "rbutton rbutton--outline", false, &[]);
    let mut body = String::from("\n  ");
    if env_available {
        body += &row(&title, Some(&escape(&app.settings_smtp_provider_name)), None, "", "", Some(&format!("{send_test}\n")));
        body += "\n";
    } else {
        let hint = configured.then(|| {
            i18n::t(ctx.locale, "settings.email_notifications.smtp_configured_html", &[("host", Arg::Text(host.as_deref().filter(|s| !crate::ruby::blank(s)).unwrap_or("smtp.gmail.com")))])
        });
        let mut control = String::new();
        if configured {
            let disconnect = ctx.t("settings.email_notifications.disconnect");
            control += &format!("  {send_test}\n        {}\n      ", confirm(csrf, &disconnect, &ctx.t("settings.index.api_key_confirmation"), &disconnect, &ctx.path("/settings/disconnect_email"), true, &[]));
        }
        let on = configured || show_form;
        let fields: Vec<(&str, Option<&str>)> = if has_credentials {
            vec![("smtp_username", Some(super::preferences::REDACTED)), ("smtp_password", Some(super::preferences::REDACTED)), ("smtp_host", host.as_deref()), ("smtp_port", port.as_deref())]
        } else {
            vec![]
        };
        control += &switch(ctx, csrf, "/settings/update_email_notifications", "email_notifications", "smtp_provider", if on { "on" } else { "off" }, if on { "" } else { "custom_smtp" }, Some(&title), &fields);
        body += &row(&title, hint.as_deref(), None, "", "", Some(&control));
        body += "\n";
        if show_form && !configured {
            let field = |kind: &str, name: &str, value: Option<&str>, key: &str, extra: &str| {
                let label = ctx.t(key);
                format!(
                    "<input type=\"{kind}\" name=\"{name}\" id=\"{name}\"{} class=\"rinput rinput--outline rinput--text\"{extra} aria-label=\"{label}\" placeholder=\"{label}\" />",
                    value.map(|v| format!(" value=\"{v}\"")).unwrap_or_default()
                )
            };
            body += &format!(
                "      {}\n        <input type=\"hidden\" name=\"smtp_provider\" value=\"custom_smtp\" />\n        {}\n        {}\n        {}\n        {}\n        <p class=\"srow__hint\">{}</p>\n        {}\n</form>",
                form(ctx, csrf, " class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"email_notifications\"", "/settings/update_email_notifications", true),
                field("text", "smtp_host", Some("smtp.gmail.com"), "settings.email_notifications.smtp_host", ""),
                field("number", "smtp_port", Some("587"), "settings.email_notifications.smtp_port", ""),
                field("text", "smtp_username", None, "settings.email_notifications.smtp_username", " autocomplete=\"off\""),
                field("password", "smtp_password", None, "settings.email_notifications.smtp_password", " autocomplete=\"off\""),
                ctx.t("settings.email_notifications.smtp_hint_html"),
                submit(ctx, "settings.email_notifications.set")
            );
        }
    }
    Ok(format!("<turbo-frame class=\"sitem\" id=\"email_notifications\">{body}</turbo-frame>\n"))
}

/// settings/widgets/_wash_sale: the account's answer to the wash-sale question, and what the rule locks right now.
pub fn wash(c: &Connection, ctx: &Ctx, csrf: &str, user_id: i64) -> Result<String, WebError> {
    let (enabled, jurisdiction): (Option<bool>, Option<String>) =
        c.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1", [user_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let code = jurisdiction.as_deref().filter(|s| !crate::ruby::blank(s)).unwrap_or("US");
    let choices = [("US", "US", 30), ("GB", "UK", 30), ("IE", "IE", 28)];
    let label = |name: &str, days: i64| i18n::t(ctx.locale, "settings.wash_sale.option", &[("days", Arg::Count(days)), ("country", Arg::Text(name))]);
    let selected = choices.iter().find(|(name, _, _)| *name == code).map(|(_, name, days)| label(name, *days)).unwrap_or_default();
    let options = choices
        .iter()
        .map(|(name, country, days)| format!("<option{} value=\"{}\">{}</option>", if *name == code { " selected=\"selected\"" } else { "" }, name, escape(&label(country, *days))))
        .collect::<Vec<_>>()
        .join("\n");
    let select = format!("\n        <div class=\"sinput sinput--select\" data-controller=\"form--select-display\">\n          <span data-form--select-display-target=\"label\">{}</span>\n          <select name=\"wash_sale[jurisdiction]\" id=\"wash_sale_jurisdiction\" data-action=\"change-&gt;form--select-display#update\">{options}</select>\n        </div>\n", escape(&selected));
    let sentence = i18n::t(ctx.locale, "settings.wash_sale.sentence_html", &[("select_html", Arg::Html(&select))]);
    let checked = |on: bool| if on { " checked=\"checked\"" } else { "" };
    let question = format!(
        "<div class=\"wash-sale-question\">\n  <p class=\"text-inactive\">{}</p>\n\n  <label class=\"form__radio\">\n    <input type=\"radio\" name=\"wash_sale[enabled]\" id=\"wash_sale_enabled_1\" value=\"1\"{} data-form--checkbox-enables-target=\"checkbox\" />\n    <span class=\"conversational conversational--small\">\n      {sentence}\n    </span>\n  </label>\n\n  <label class=\"form__radio\">\n    <input type=\"radio\" name=\"wash_sale[enabled]\" id=\"wash_sale_enabled_0\" value=\"0\"{} data-form--checkbox-enables-target=\"checkbox\" />\n    <span class=\"conversational conversational--small\">{}</span>\n  </label>\n</div>\n",
        ctx.t("settings.wash_sale.prompt_body"),
        checked(enabled == Some(true)),
        checked(enabled == Some(false)),
        ctx.t("settings.wash_sale.prompt_decline")
    );
    // User#locked_assets is read whether or not protection is on, as Rails does; rendering is conditional.
    let mut statement = c.prepare("SELECT assets.symbol,wash_sale_locks.buy_locked_until FROM wash_sale_locks JOIN assets ON assets.id=wash_sale_locks.asset_id WHERE user_id=?1 AND buy_locked_until>?2 ORDER BY buy_locked_until,wash_sale_locks.id")?;
    let locks = statement
        .query_map((user_id, crate::codec::format_time(ctx.now)), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let locked = if enabled != Some(true) {
        String::new()
    } else if locks.is_empty() {
        format!("\n      <div class=\"srow__hint\">\n        <b>{}</b>\n          <small>{}</small>\n      </div>\n", ctx.t("settings.wash_sale.locked_title"), ctx.t("settings.wash_sale.locked_none"))
    } else {
        let mut entries = String::new();
        for (symbol, until) in locks {
            let until = crate::codec::parse_time(&until).map_err(|_| WebError::Config("invalid wash-sale deadline".into()))?;
            let days = (until.date_naive() - ctx.now.date_naive()).num_days();
            let text = i18n::t(ctx.locale, "settings.wash_sale.locked_entry", &[("days", Arg::Count(days))]);
            entries.push_str(&format!("\n              <li>\n                <b>{}</b>\n                {text}\n                <small>{}</small>\n              </li>\n", escape(&symbol), (until - chrono::Duration::days(1)).format("%Y-%m-%d")));
        }
        format!("\n      <div class=\"srow__hint\">\n        <b>{}</b>\n          <ul class=\"settings-locked\">{entries}          </ul>\n      </div>\n", ctx.t("settings.wash_sale.locked_title"))
    };
    Ok(format!(
        "<turbo-frame class=\"sitem\" id=\"wash_sale\">\n  <div class=\"srow srow--block\">\n    <div class=\"srow__text\"><div class=\"srow__label\">{}</div></div>\n\n    {}\n      {question}\n      <button name=\"button\" type=\"submit\" class=\"rbutton rbutton--outline\"{} data-form--checkbox-enables-target=\"submit\">{}</button>\n</form>{locked}  </div>\n</turbo-frame>\n",
        ctx.t("settings.wash_sale.title"),
        form(ctx, csrf, " class=\"sform sform--stack\" data-controller=\"form--checkbox-enables\" data-action=\"change-&gt;form--checkbox-enables#toggleSubmitButton\" data-turbo-frame=\"wash_sale\"", "/settings/update_wash_sale", true),
        if enabled.is_none() { " disabled=\"disabled\"" } else { "" },
        ctx.t("settings.wash_sale.confirm")
    ))
}

/// One `<h3>` of a section.
fn heading(ctx: &Ctx, key: &str, first: bool) -> String {
    format!("<h3 class=\"settings__heading{}\">{}</h3>\n", if first { " settings__heading--first" } else { "" }, ctx.t(key))
}

/// sections/_account.
fn account_pane(c: &Connection, app: &App, ctx: &Ctx, csrf: &str, user: &User, draft: Draft) -> Result<String, WebError> {
    let st = AccountState::load(c, user, draft)?;
    let advanced_on: bool = c.query_row("SELECT advanced_bots_enabled FROM users WHERE id=?1", [user.id], |r| r.get(0))?;
    let mut pane = heading(ctx, "settings.sections.profile", true);
    pane += &name_widget(ctx, csrf, &st);
    pane += &email_widget(ctx, csrf, &st);
    pane += &password_widget(ctx, csrf, &st);
    pane += "\n";
    pane += &heading(ctx, "settings.sections.preferences", false);
    pane += &locale_widget(ctx, csrf, user);
    pane += &time_zone_widget(ctx, csrf, user);
    pane += "\n";
    pane += &heading(ctx, "settings.sections.security", false);
    pane += &two_fa_widget(ctx, user.otp_enabled);
    pane += "\n";
    pane += &heading(ctx, "links.bots", false);
    pane += &advanced(ctx, csrf, advanced_on)?;
    pane += &wash(c, ctx, csrf, user.id)?;
    if user.admin {
        let open = crate::web::auth::app_config(c, &app.cipher, "registration_open")?.as_deref() == Some("true");
        pane += "\n  ";
        pane += &heading(ctx, "settings.sections.instance", false);
        pane += "  ";
        pane += &registration(ctx, csrf, open)?;
        pane += "  ";
        pane += &email(c, app, ctx, csrf, false)?;
        // settings/widgets/_app_update draws nothing until an update check has answered, which this build does not run.
    }
    Ok(pane)
}

/// settings/widgets/_ibkr: hidden without the data API, except for an existing connection.
pub fn ibkr(c: &Connection, app: &App, ctx: &Ctx, user: i64) -> Result<String, WebError> {
    let key = ibkr_key(c, user)?;
    if !app.config.market_data_url && key.is_none() {
        return Ok(String::new());
    }
    let status = match &key {
        Some((1, _)) => "CONNECTED",
        Some((3, updated)) if crate::codec::parse_time(updated).is_ok_and(|at| at <= ctx.now - chrono::Duration::days(14)) => "ACTION NEEDED",
        Some((3, _)) => "PENDING",
        _ => "OFF",
    };
    let link = format!(
        "<a class=\"rbutton rbutton--outline\" data-turbo-frame=\"_top\" href=\"{}\">{}</a>\n",
        escape(&ctx.path("/settings/ibkr_connect")),
        ctx.t(if key.is_some() { "settings.ibkr.manage" } else { "settings.ibkr.connect_cta" })
    );
    Ok(format!("  {}\n", row(&ctx.t("settings.ibkr.widget_title"), Some(status), None, "", " id=\"ibkr_settings\"", Some(&link))))
}

/// sections/_connect: stocks (IBKR for everyone it applies to, Alpaca for the admin) and market data (admin).
fn connect_pane(c: &Connection, app: &App, ctx: &Ctx, csrf: &str, user: &User) -> Result<String, WebError> {
    let ibkr = ibkr(c, app, ctx, user.id)?;
    let mut pane = String::from("\n");
    if !ibkr.is_empty() || user.admin {
        let (stocks, market_data) = super::keys::connection_widgets(c, app, ctx, csrf)?;
        pane += &format!("  {}\n  {ibkr}", heading(ctx, "settings.stocks.title", true));
        if user.admin {
            pane += &format!("  {stocks}\n{market_data}");
        }
    }
    Ok(pane)
}

/// The API base URL a client is given (AppConfig.mcp_url / api_url).
pub fn root(app: &App) -> String {
    app.config.own_origin.clone().unwrap_or_else(|| "http://localhost:3000".into())
}

/// settings/widgets/_mcp.
pub fn mcp_widget(app: &App, ctx: &Ctx, csrf: &str, dry_run: bool) -> String {
    let url = row("URL", None, None, "", "", Some(&copy_tag(ctx, &format!("{}/mcp", root(app)), Some("mcp_url_display"))));
    let switch = switch(ctx, csrf, "/settings/update_mcp_dry_run", "mcp_settings", "enabled", if dry_run { "on" } else { "off" }, if dry_run { "0" } else { "1" }, Some(&ctx.t("settings.mcp.dry_run_label")), &[]);
    let paper = row(&ctx.t("settings.mcp.dry_run_title"), Some(&ctx.t("settings.mcp.dry_run_description")), None, "", "", Some(&switch));
    format!(
        "<turbo-frame id=\"mcp_settings\">\n  {}  <p class=\"settings__intro\">{}</p>\n\n  {url}\n\n  {paper}</turbo-frame>\n",
        heading(ctx, "settings.sections.mcp_server", true),
        ctx.t("settings.mcp.description")
    )
}

/// settings/widgets/_rest: the token shows only in the response to Regenerate.
pub fn rest_widget(app: &App, ctx: &Ctx, csrf: &str, token: &str) -> String {
    let url = row("URL", None, None, "", "", Some(&copy_tag(ctx, &format!("{}/api/v1", root(app)), Some("rest_api_url_display"))));
    let regenerate = ctx.t("settings.rest.regenerate_token");
    let control = format!(
        "{}\n    {}",
        copy_tag(ctx, token, None),
        confirm(csrf, &regenerate, &ctx.t("settings.rest.regenerate_confirm"), &regenerate, &ctx.path("/settings/regenerate_api_token"), false, &[])
    );
    let token = row(&ctx.t("settings.rest.token"), None, None, "srow--token", "", Some(&control));
    let docs = row(
        &ctx.t("settings.rest.docs"),
        None,
        None,
        "",
        "",
        Some(&format!("<a class=\"rbutton rbutton--outline\" title=\"{}\" data-turbo=\"false\" href=\"{}\">deltabadger-api.md</a>\n", ctx.t("settings.rest.download_api_docs"), escape(&ctx.path("/settings/download_api_docs")))),
    );
    format!(
        "<turbo-frame id=\"rest_settings\">\n  {}  <p class=\"settings__intro\">{}</p>\n\n\n  {url}\n  {token}\n  {docs}</turbo-frame>\n",
        heading(ctx, "settings.rest.title", true),
        ctx.t("settings.rest.description")
    )
}

/// settings/widgets/_tool_permissions for one surface: a switch per group, a switch per tool under it.
pub fn tools_widget(ctx: &Ctx, csrf: &str, surface: &str, enabled: &[String]) -> String {
    let on = |tool: &str| enabled.iter().any(|name| name == tool);
    let (tool_url, group_url) = if surface == "mcp" {
        ("/settings/update_mcp_tool_permissions", "/settings/update_mcp_tool_group_permissions")
    } else {
        ("/settings/update_rest_tool_permissions", "/settings/update_rest_tool_group_permissions")
    };
    let mut rows = String::new();
    for (group, tools) in crate::web::oauth::TOOL_GROUPS {
        let count = tools.iter().filter(|tool| on(tool)).count();
        let state = if count == 0 { "off" } else if count == tools.len() { "on" } else { "partial" };
        let group_switch = switch(ctx, csrf, group_url, "tool_permissions", "enabled", state, if count == tools.len() { "0" } else { "1" }, None, &[("group", Some(group))]);
        rows += &format!("\n    {}", row(&ctx.t(&format!("settings.mcp.tools_{group}")), None, None, "srow--group", &format!(" data-group=\"{group}\" data-surface=\"{surface}\""), Some(&group_switch)));
        for tool in tools.iter() {
            let tool_switch = switch(ctx, csrf, tool_url, "tool_permissions", "enabled", if on(tool) { "on" } else { "off" }, if on(tool) { "0" } else { "1" }, None, &[("tool_name", Some(tool))]);
            rows += &format!(
                "      {}",
                row(
                    &ctx.t(&format!("settings.mcp.tools.{tool}.label")),
                    Some(&ctx.t(&format!("settings.mcp.tools.{tool}.description"))),
                    None,
                    "srow--tool",
                    &format!(" data-tool=\"{tool}\" data-surface=\"{surface}\""),
                    Some(&tool_switch)
                )
            );
        }
    }
    format!(
        "<turbo-frame id=\"tool_permissions\">\n  {}  <p class=\"settings__intro\">{}</p>\n{rows}</turbo-frame>\n",
        heading(ctx, "settings.mcp.tools_title", false),
        ctx.t("settings.mcp.tools_description")
    )
}

/// The MCP or REST settings of the user: dry run and the enabled tools of both surfaces.
fn tool_settings(c: &Connection, user: i64) -> Result<(bool, Vec<String>, Vec<String>), WebError> {
    let settings: Option<String> = c.query_row("SELECT mcp_settings FROM users WHERE id=?1", [user], |r| r.get(0))?;
    let settings: serde_json::Value = settings
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|_| WebError::Config("invalid MCP settings".into()))?
        .unwrap_or(serde_json::Value::Null);
    Ok((settings["dry_run"] == true, super::clients::enabled(c, user, "mcp")?, super::clients::enabled(c, user, "rest")?))
}

/// The pane of one section.
fn pane(c: &Connection, app: &App, ctx: &Ctx, csrf: &str, user: &User, section: &str, draft: Draft) -> Result<String, WebError> {
    Ok(match section {
        "account" => account_pane(c, app, ctx, csrf, user, draft)?,
        "api_keys" => format!("\n  {}", super::keys::list(c, ctx, csrf, user.id)?),
        "connect" => connect_pane(c, app, ctx, csrf, user)?,
        "mcp" => {
            let (dry_run, mcp, _) = tool_settings(c, user.id)?;
            format!("\n  {}{}{}", mcp_widget(app, ctx, csrf, dry_run), tools_widget(ctx, csrf, "mcp", &mcp), super::clients::widget(c, ctx, csrf, user.id, "mcp")?)
        }
        _ => {
            let (_, _, rest) = tool_settings(c, user.id)?;
            format!("\n  {}{}{}", rest_widget(app, ctx, csrf, super::preferences::REDACTED), tools_widget(ctx, csrf, "rest", &rest), super::clients::widget(c, ctx, csrf, user.id, "rest")?)
        }
    })
}

/// SettingsController#render_section: the dialog on one section, the whole page on a full visit, the frame on a turbo one.
pub async fn section(app: &App, ctx: &Ctx, section: &'static str, status: StatusCode, draft: Option<Draft>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(crate::web::auth::unauthenticated(ctx));
    };
    let (v, inner, owner, csrf) = (ctx.clone(), app.clone(), user.clone(), ctx.csrf_token());
    let token = csrf.clone();
    let (body, shell) = app
        .db(move |c| {
            let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
            // The REST section mints the personal token on a first visit (User#personal_api_token).
            if section == "api" {
                super::api::ensure_personal(&tx, owner.id, &crate::codec::format_time(inner.now()))?;
            }
            let pane = pane(&tx, &inner, &v, &token, &owner, section, draft.unwrap_or_default())?;
            let body = dialog(&v, section, connect_visible(&tx, &inner, &owner)?, &pane);
            let shell = Shell::load(&tx, &inner, &owner)?;
            tx.commit()?;
            Ok((body, shell))
        })
        .await?;
    shell::application(ctx, &csrf, &user, &shell, Page { status, body, flash_now: vec![] })
}

/// A page of its own inside the dialog (api_key_permissions): `pane` under the sidebar with `section` active.
pub async fn page(app: &App, ctx: &Ctx, section: &'static str, pane: String) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(crate::web::auth::unauthenticated(ctx));
    };
    let (v, inner, owner) = (ctx.clone(), app.clone(), user.clone());
    let csrf = ctx.csrf_token();
    let (body, shell) = app
        .db(move |c| Ok((dialog(&v, section, connect_visible(c, &inner, &owner)?, &pane), Shell::load(c, &inner, &owner)?)))
        .await?;
    shell::application(ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] })
}

/// settings/api_key_permissions: back to the keys, and how to get one.
pub fn permissions_pane(ctx: &Ctx, heading_text: &str, prose: &str) -> String {
    format!(
        "\n  <a class=\"settings__back\" href=\"{}\">\n    {BACK}{}\n</a>  <h3 class=\"settings__heading\">{heading_text}</h3>\n  <div class=\"settings__prose\">{prose}</div>\n",
        escape(&ctx.path("/settings/api_keys")),
        ctx.t("settings.index.api_keys_section")
    )
}

/// SettingsController#render_row: a saved row answers with itself and a flash, never a page refresh.
pub fn saved_row(ctx: &Ctx, frame: &str, widget: &str, notice: String) -> Result<Response, WebError> {
    use axum::response::IntoResponse;
    let flash = crate::web::flash::render(&crate::web::flash::take(&ctx.session, &[(crate::web::flash::NOTICE, notice)]))?;
    let body = crate::web::turbo::stream("replace", frame, widget.trim_end()) + &crate::web::turbo::prepend_flash(flash.trim_end());
    Ok(([(axum::http::header::CONTENT_TYPE, crate::web::turbo::CONTENT_TYPE)], body).into_response())
}

/// The row a write re-renders, with the user as the database now holds them.
pub async fn account_row(app: &App, ctx: &Ctx, frame: &'static str) -> Result<String, WebError> {
    let Some(user) = ctx.user().cloned() else { return Err(WebError::Config("missing settings owner".into())) };
    let (v, csrf) = (ctx.clone(), ctx.csrf_token());
    app.db(move |c| {
        let user = User::find(c, user.id)?.ok_or_else(|| WebError::Config("account no longer exists".into()))?;
        let st = AccountState::load(c, &user, Draft::default())?;
        Ok(match frame {
            "update_email_form" => email_widget(&v, &csrf, &st),
            "update_password_form" => password_widget(&v, &csrf, &st),
            "update_time_zone_form" => time_zone_widget(&v, &csrf, &user),
            _ => two_fa_widget(&v, user.otp_enabled),
        })
    })
    .await
}
