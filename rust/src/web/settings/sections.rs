//! The settings sections' rows, as app/views/settings/widgets/* render them inside the dialog
//! (rust/tests/pages.rs holds each to 2.x's markup).
use super::dialog::{row, switch};
use crate::web::{i18n::escape, layout::Ctx};
use std::collections::BTreeMap;

/// A form's opening tag as `form_with` writes it, the method override and the token after it.
pub fn form_open(attributes: &str, action: &str, method: &str, csrf: &str) -> String {
    let method_field = if method == "post" { String::new() } else { format!("<input type=\"hidden\" name=\"_method\" value=\"{method}\" />") };
    format!("<form{attributes} action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">{method_field}<input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />", escape(action))
}

/// ActionView's field_error_proc as 2.x sets it: Nokogiri re-writes the
/// field with ` is-invalid` appended to its class (minimising `readonly="readonly"` as it does), and the
/// message goes under it. `input` is the field's tag.
pub fn field(input: &str, error: Option<&String>) -> String {
    let Some(message) = error else { return input.to_string() };
    let mut tag = input.replacen(" />", ">", 1).replace("readonly=\"readonly\"", "readonly");
    match tag.find("class=\"") {
        Some(at) => {
            let end = at + 7 + tag[at + 7..].find('"').unwrap_or(0);
            tag.insert_str(end, " is-invalid");
        }
        None => {
            let end = tag.len() - 1;
            tag.insert_str(end, " class=\" is-invalid\"");
        }
    }
    format!("                   {tag}\n             <div class=\"form__info form__info--invalid\">{}</div>\n", escape(message))
}

pub struct Errors<'a>(pub &'a BTreeMap<String, String>);
impl Errors<'_> {
    fn get(&self, field: &str) -> Option<&String> { self.0.get(field) }
    fn any(&self) -> bool { !self.0.is_empty() }
}

fn submit(label: &str, extra: &str) -> String {
    format!("<input type=\"submit\" name=\"commit\" value=\"{label}\" class=\"rbutton rbutton--secondary\"{extra} data-disable-with=\"{label}\" />")
}

/// A row that opens into a form: the summary is the row with its Change button.
fn details(frame: &str, label: &str, hint: Option<&str>, open: bool, ctx: &Ctx, form: &str) -> String {
    let control = format!("\n        <span class=\"rbutton rbutton--secondary\">{}</span>\n", ctx.t("settings.sections.change"));
    format!(
        "<turbo-frame class=\"sitem\" id=\"{frame}\">\n  <details {}>\n    <summary>\n      {}    </summary>\n    {form}  </details>\n</turbo-frame>",
        if open { "open" } else { "" },
        row(label, hint, "", "", None, Some(&control))
    )
}

/// settings/widgets/_email. `hint` is the address, or the pending reconfirmation.
pub fn email(ctx: &Ctx, csrf: &str, hint: &str, errors: &Errors) -> String {
    let address = format!(
        "<input value=\"\" autocomplete=\"username\" required=\"required\" class=\"rinput rinput--outline rinput--text\" pattern=\"^[a-zA-Z0-9._%+\\-]+@[a-zA-Z0-9.\\-]+\\.[a-zA-Z]{{2,}}$\" placeholder=\"{0}\" aria-label=\"{0}\" type=\"email\" name=\"user[email]\" id=\"user_email\" />",
        ctx.t("helpers.label.user.new_email")
    );
    let password = format!(
        "<input autocomplete=\"current-password\" required=\"required\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{0}\" aria-label=\"{0}\" type=\"password\" name=\"user[current_password]\" id=\"user_current_password\" />",
        ctx.t("helpers.label.user.current_password")
    );
    let form = format!(
        "{}\n      {}\n      {}\n      {}\n</form>",
        form_open(" class=\"sform sform--stack\" data-turbo-frame=\"update_email_form\"", &ctx.path("/settings/update_email"), "patch", csrf),
        field(&address, errors.get("email")),
        field(&password, errors.get("current_password")),
        submit(&ctx.t("button.save"), "")
    );
    details("update_email_form", &ctx.t("settings.index.email_section"), Some(&escape(hint)), errors.any(), ctx, &form)
}

/// settings/widgets/_password.
pub fn password(ctx: &Ctx, csrf: &str, email: &str, errors: &Errors) -> String {
    let current = format!(
        "<input autocomplete=\"current-password\" id=\"current_password_for_password_update\" required=\"required\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{0}\" aria-label=\"{0}\" type=\"password\" name=\"user[current_password]\" />",
        ctx.t("helpers.label.user.current_password")
    );
    let new = format!(
        "<input autocomplete=\"new-password\" required=\"required\" class=\"rinput rinput--outline rinput--text\" pattern=\"(?=.*[a-z])(?=.*[A-Z])(?=.*\\d)(?=.*[\\W_]).{{8,}}\" placeholder=\"{0}\" aria-label=\"{0}\" data-form--password-strength-target=\"input\" data-form--password-match-target=\"input0\" data-action=\"input-&gt;form--password-strength#checkPasswordStrength\" type=\"password\" name=\"user[password]\" id=\"user_password\" />",
        ctx.t("helpers.label.user.new_password")
    );
    let confirmation = format!(
        "<input autocomplete=\"new-password\" required=\"required\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{0}\" aria-label=\"{0}\" data-form--password-match-target=\"input1\" data-action=\"input-&gt;form--password-match#checkPasswordMatch\" type=\"password\" name=\"user[password_confirmation]\" id=\"user_password_confirmation\" />",
        ctx.t("helpers.label.user.new_password_confirmation")
    );
    let requirement = |target: &str, key: &str| format!("\n          <li class=\"form__validation\" data-form--password-strength-target=\"{target}\">{}</li>", ctx.t(key));
    let length = crate::web::i18n::t(ctx.locale, "devise.passwords.validations.length", &[("count", crate::web::i18n::Arg::Count(8))]);
    let hidden = format!("<input autocomplete=\"username\" id=\"email_for_password_update\" readonly=\"readonly\" hidden=\"hidden\" type=\"email\" value=\"{}\" name=\"user[email]\" />", escape(email));
    let form = format!(
        "{}\n      {}\n      {}\n      {}\n      {}\n      <div data-form--password-strength-target=\"requirementsList\" class=\"hidden\">\n        <ul class=\"password-requirements\">\n          <li class=\"form__validation\" data-form--password-strength-target=\"length\">{length}</li>{}{}{}{}\n          <li class=\"form__validation\" data-form--password-match-target=\"match\">{}</li>\n        </ul>\n      </div>\n      {}\n</form>",
        form_open(
            " class=\"sform sform--stack\" data-controller=\"form--password-strength form--password-match\" data-turbo-frame=\"update_password_form\" data-form--password-match-success-class-value=\"form__validation--success\" data-form--password-strength-success-class-value=\"form__validation--success\" data-form--password-strength-length-pattern-value=\".{8,}\" data-form--password-strength-uppercase-pattern-value=\"(?=.*[A-Z])\" data-form--password-strength-lowercase-pattern-value=\"(?=.*[a-z])\" data-form--password-strength-digit-pattern-value=\"(?=.*\\d)\" data-form--password-strength-symbol-pattern-value=\"(?=.*[\\W_])\"",
            &ctx.path("/settings/update_password"),
            "patch",
            csrf
        ),
        field(&hidden, errors.get("email")),
        field(&current, errors.get("current_password")),
        field(&new, errors.get("password")),
        field(&confirmation, errors.get("password_confirmation")),
        requirement("uppercase", "devise.passwords.validations.uppercase"),
        requirement("lowercase", "devise.passwords.validations.lowercase"),
        requirement("digit", "devise.passwords.validations.digit"),
        requirement("symbol", "devise.passwords.validations.symbol"),
        ctx.t("devise.passwords.validations.match"),
        submit(&ctx.t("button.save"), "")
    );
    details("update_password_form", &ctx.t("settings.index.password_section"), None, errors.any(), ctx, &form)
}

/// A select that submits its row on change: settings/widgets/_locale and _time_zone.
#[allow(clippy::too_many_arguments)]
fn select_row(ctx: &Ctx, csrf: &str, frame: &str, action: &str, label: &str, hint: Option<&str>, field: &str, options: &str) -> String {
    let form = format!(
        "\n    {}\n      <select class=\"rinput rinput--outline rinput--select\" aria-label=\"{label}\" data-action=\"change-&gt;form--submit#submit\" name=\"user[{field}]\" id=\"user_{field}\">{options}</select>\n</form>",
        form_open(&format!(" data-controller=\"form--submit\" data-turbo-frame=\"{frame}\""), &ctx.path(action), "patch", csrf)
    );
    format!("<turbo-frame class=\"sitem\" id=\"{frame}\">\n  {}</turbo-frame>", row(label, hint, "", "", None, Some(&form)))
}

pub fn locale(ctx: &Ctx, csrf: &str, options: &str) -> String {
    select_row(ctx, csrf, "update_locale_form", "/settings/update_locale", &ctx.t("settings.sections.language"), None, "locale", options)
}

pub fn time_zone(ctx: &Ctx, csrf: &str, zone_id: &str, local_time: &str, options: &str) -> String {
    let hint = format!(
        "<span data-controller=\"clock\" data-clock-time-zone-value=\"{}\">    {}\n    <b data-clock-target=\"timestamp\">{}</b>\n</span>",
        escape(zone_id),
        ctx.t("settings.language_and_timezone.current_time_is"),
        escape(local_time)
    );
    select_row(ctx, csrf, "update_time_zone_form", "/settings/update_time_zone", &ctx.t("settings.sections.time_zone"), Some(&hint), "time_zone", options)
}

/// settings/widgets/_two_fa: the row, closed.
pub fn two_fa(ctx: &Ctx, enabled: bool) -> String {
    let control = format!(
        "\n    <a class=\"rbutton rbutton--secondary\" href=\"{}\">{}</a>\n",
        escape(&ctx.path("/settings/edit_two_fa")),
        ctx.t(if enabled { "helpers.label.settings.disable_two_fa" } else { "helpers.label.settings.enable_two_fa" })
    );
    let hint = ctx.t(if enabled { "helpers.label.settings.enabled" } else { "helpers.label.settings.disabled" });
    format!("<turbo-frame class=\"sitem\" id=\"two_fa\">\n  {}</turbo-frame>", row(&ctx.t("settings.index.two_factor_section"), Some(&hint), "", "", None, Some(&control)))
}

/// settings/edit_two_fa: the 2FA row opened, the QR code (when enabling) and the code that confirms it.
pub fn two_fa_open(ctx: &Ctx, csrf: &str, enabled: bool, qr: &str, error: Option<&String>) -> String {
    let control = format!("\n      <a class=\"rbutton rbutton--secondary\" href=\"{}\">{}</a>\n", escape(&ctx.path("/settings/account")), ctx.t("button.cancel"));
    let label = ctx.t("helpers.label.settings.enter_two_fa_code");
    let input = format!(
        "<input class=\"rinput rinput--outline rinput--text\" required=\"required\" pattern=\"\\d{{6}}\" autocomplete=\"one-time-code\" inputmode=\"numeric\" placeholder=\"{label}\" aria-label=\"{label}\" type=\"text\" name=\"user[otp_code_token]\" id=\"user_otp_code_token\" />"
    );
    let button = ctx.t(if enabled { "helpers.label.settings.disable_two_fa" } else { "helpers.label.settings.enable_two_fa" });
    let qr = if qr.is_empty() { String::new() } else { format!("{qr}\n") };
    format!(
        "<turbo-frame class=\"sitem\" id=\"two_fa\">\n  <div>\n    {}    {}\n      {qr}\n      {}\n      <input type=\"submit\" name=\"commit\" value=\"{button}\" class=\"rbutton rbutton--secondary {}\" data-disable-with=\"{button}\" />\n</form>  </div>\n</turbo-frame>",
        row(&ctx.t("settings.index.two_factor_section"), None, "", "", None, Some(&control)),
        form_open(" class=\"sform sform--stack\"", &ctx.path("/settings/update_two_fa"), "patch", csrf),
        field(&input, error),
        if enabled { "rbutton--danger" } else { "" }
    )
}

/// D-DESKTOP-NOTIFY: the desktop's notifications row, one line (no toggle: none exists for which alerts are raised).
pub fn desktop_notifications(ctx: &Ctx) -> String {
    format!("<div class=\"sitem\" id=\"desktop_notifications\">\n  {}</div>", row(&ctx.t("desktop_notice.settings"), None, "", "", None, None))
}

/// D-DESKTOP-BROWSER-CODE: the desktop's "Sign in a browser" row; with `code`, the code it just made.
pub fn browser_sign_in(ctx: &Ctx, csrf: &str, code: Option<&str>) -> String {
    let label = ctx.t("desktop_sign_in.settings_title");
    let button = ctx.t(if code.is_some() { "desktop_sign_in.settings_new_code" } else { "desktop_sign_in.settings_show_code" });
    let control = format!(
        "\n    {}<input type=\"submit\" name=\"commit\" value=\"{button}\" class=\"rbutton rbutton--secondary\" data-disable-with=\"{button}\" />\n</form>\n",
        form_open(" class=\"button_to\"", &ctx.path("/settings/browser_sign_in_code"), "post", csrf)
    );
    let hint = match code {
        Some(code) => format!(
            "<strong class=\"desktop-code\" data-desktop-code=\"{}\">{}-{}</strong> {}",
            escape(code), escape(&code[..4]), escape(&code[4..]),
            crate::web::i18n::t(ctx.locale, "desktop_sign_in.settings_code_hint", &[("count", crate::web::i18n::Arg::Count(crate::web::desktop::code_minutes()))])
        ),
        None => ctx.t("desktop_sign_in.settings_hint"),
    };
    format!("<turbo-frame class=\"sitem\" id=\"desktop_browser_sign_in\">\n  {}</turbo-frame>", row(&label, Some(&hint), "", "", None, Some(&control)))
}

/// settings/widgets/_advanced_bots.
pub fn advanced(ctx: &Ctx, csrf: &str, on: bool) -> String {
    let label = ctx.t("settings.advanced_bots.toggle_label");
    let control = format!(
        "\n    {}\n",
        switch(csrf, &ctx.path("/settings/update_advanced_bots"), "advanced_bots", "enabled", if on { "0" } else { "1" }, if on { "on" } else { "off" }, Some(&label), &[])
    );
    format!("<turbo-frame class=\"sitem\" id=\"advanced_bots\">\n  {}</turbo-frame>", row(&ctx.t("settings.advanced_bots.title"), Some(&label), "", "", None, Some(&control)))
}

/// settings/widgets/_registration. D-NO-MAIL-SAYS-SO: open with no way to send mail, the row says new accounts cannot confirm.
pub fn registration(ctx: &Ctx, csrf: &str, open: bool, mail_set_up: bool) -> String {
    let label = ctx.t("settings.index.registration_section");
    let control = format!(
        "\n    {}\n",
        switch(csrf, &ctx.path("/settings/update_registration"), "registration_settings", "registration_open", if open { "0" } else { "1" }, if open { "on" } else { "off" }, Some(&label), &[])
    );
    let hint = (open && !mail_set_up).then(|| ctx.t("settings.registration.mail_not_set_up"));
    format!("<turbo-frame class=\"sitem\" id=\"registration_settings\">\n  {}</turbo-frame>", row(&label, hint.as_deref(), "", "", None, Some(&control)))
}

/// settings/widgets/_wash_sale: the shared question, Confirm, and what is locked right now.
pub fn wash(ctx: &Ctx, csrf: &str, question: &str, decided: bool, enabled: bool, locked: &str) -> String {
    let locked = if enabled {
        format!("\n      <div class=\"srow__hint\">\n        <b>{}</b>{locked}      </div>\n", ctx.t("settings.wash_sale.locked_title"))
    } else {
        String::new()
    };
    format!(
        "<turbo-frame class=\"sitem\" id=\"wash_sale\">\n  <div class=\"srow srow--block\">\n    <div class=\"srow__text\"><div class=\"srow__label\">{}</div></div>\n\n    {}\n      {question}\n      <button name=\"button\" type=\"submit\"{} class=\"rbutton rbutton--secondary\" data-form--checkbox-enables-target=\"submit\">{}</button>\n</form>{locked}\n  </div>\n</turbo-frame>",
        ctx.t("settings.wash_sale.title"),
        form_open(
            " class=\"sform sform--stack\" data-controller=\"form--checkbox-enables\" data-action=\"change-&gt;form--checkbox-enables#toggleSubmitButton\" data-turbo-frame=\"wash_sale\"",
            &ctx.path("/settings/update_wash_sale"),
            "patch",
            csrf
        ),
        if decided { "" } else { " disabled=\"disabled\"" },
        ctx.t("settings.wash_sale.confirm")
    )
}

/// The Account section's pane (settings/sections/_account), from its rows.
pub struct Account<'a> {
    pub email: String,
    pub password: String,
    pub locale: String,
    pub time_zone: String,
    pub two_fa: String,
    pub advanced: String,
    pub wash: String,
    /// Registration, email notifications and the update row (settings/widgets/_app_update), for an admin.
    pub admin: Option<(&'a str, &'a str, &'a str)>,
}

pub fn account(ctx: &Ctx, a: &Account) -> String {
    let instance = a
        .admin
        .map(|(registration, email, update)| format!("\n  <h3 class=\"sheet__heading\">{}</h3>\n  {registration}\n  {email}\n  \n{update}{}", ctx.t("settings.sections.instance"), jobs(ctx)))
        .unwrap_or_default();
    // D-DESKTOP-NO-ACCOUNT-PAGES: a section with no rows (the desktop's Profile and Security) has no heading either.
    let sections = [
        ("settings.sections.profile", format!("{}\n{}", a.email, a.password)),
        ("settings.sections.preferences", format!("{}\n{}", a.locale, a.time_zone)),
        ("settings.sections.security", a.two_fa.clone()),
        ("links.bots", format!("{}\n{}", a.advanced, a.wash)),
    ];
    let mut out = String::new();
    for (heading, rows) in sections.iter().filter(|(_, rows)| !rows.trim().is_empty()) {
        let first = if out.is_empty() { "  <h3 class=\"sheet__heading sheet__heading--first\">" } else { "\n\n<h3 class=\"sheet__heading\">" };
        out.push_str(&format!("{first}{}</h3>\n{rows}", ctx.t(heading)));
    }
    format!("{out}\n{instance}")
}

/// A25 (D-JOBS-SETTINGS-LINK): the admin's way to /jobs, last in the Instance section, on every platform (the desktop
/// window has no address bar). 2.x has no link to its dashboard. Never locale-prefixed: `/de/jobs` is not the page.
pub fn jobs(ctx: &Ctx) -> String {
    let open = format!("\n        <a class=\"rbutton rbutton--secondary\" data-turbo-frame=\"_top\" href=\"/jobs\">{}</a>\n", ctx.t("settings.jobs.open"));
    format!("  <div id=\"jobs\" class=\"sitem\">\n    {}\n  </div>\n", row(&ctx.t("settings.jobs.title"), Some(&ctx.t("settings.jobs.hint")), "", "", None, Some(&open)))
}

/// settings/widgets/_app_update: nothing is claimed without an answer. The desktop always gets its button (its own updater
/// answers); elsewhere a stored release newer than `current` is "available", an older or equal one "up to date", and no
/// answer (none yet, expired, or never asked) renders nothing. Whether the check is switched off is not read here: an
/// answer still stored shows until it expires, as 2.x's cache entry does.
pub fn app_update(ctx: &Ctx, platform: crate::jobs::app_update::Platform, latest: Option<&crate::jobs::app_update::Release>, current: &str) -> String {
    use crate::jobs::app_update::{self as update, Platform};
    let desktop = platform == Platform::Desktop;
    let available = !desktop && latest.is_some_and(|l| update::available(current, &l.version));
    let (hint, control) = match latest {
        _ if desktop => (
            format!("<span data-desktop-update-status=\"true\">{}</span>", ctx.t("desktop_update.about")),
            format!(
                "\n        <button name=\"button\" type=\"button\" class=\"rbutton rbutton--secondary\" data-desktop-update=\"true\">{}</button>\n",
                ctx.t("settings.app_update.check_now")
            ),
        ),
        Some(l) if available => (
            crate::web::i18n::t(ctx.locale, "settings.app_update.available", &[("version", crate::web::i18n::Arg::Text(&l.version))]),
            format!("\n        <a target=\"_blank\" rel=\"noopener\" class=\"rbutton rbutton--secondary\" href=\"{}\">{}</a>\n", escape(&l.url), ctx.t("settings.app_update.release_notes")),
        ),
        Some(_) => (ctx.t("settings.app_update.up_to_date"), String::new()),
        None => return String::new(),
    };
    let more = match platform {
        _ if !available => String::new(),
        Platform::Umbrel => format!("      <div class=\"srow__more\">\n          <p>{}</p>\n      </div>\n", ctx.t("settings.app_update.umbrel")),
        Platform::Docker => format!(
            "      <div class=\"srow__more\">\n          <p>{}</p>\n          <span class=\"tag tag--copy\" data-controller=\"clipboard\" data-clipboard-text-value=\"{cmd}\" data-action=\"click->clipboard#copy\" title=\"{}\">{COPY}{cmd}</span>\n      </div>\n",
            ctx.t("settings.app_update.docker"), ctx.t("button.copy"), cmd = escape(update::COMPOSE_COMMAND)
        ),
        _ => format!("      <div class=\"srow__more\">\n          <p><a target=\"_blank\" rel=\"noopener\" href=\"{}/07-updating.md\">{}</a></p>\n      </div>\n", update::MANUAL_URL, ctx.t("settings.app_update.manual")),
    };
    format!(
        "  <div id=\"app_update\" class=\"sitem\">\n    {}\n{more}  </div>\n",
        row(&ctx.t("settings.app_update.title"), Some(&hint), "", "", None, Some(&control))
    )
}

/// settings/widgets/_email_notifications. Platform SMTP, when the container has it, is the only provider;
/// otherwise one switch for custom SMTP, which keeps the stored credentials when switched off.
pub struct Mail<'a> {
    pub env_available: bool,
    pub env_name: &'a str,
    pub configured: bool,
    pub show_form: bool,
    pub configured_hint: &'a str,
    /// (host, port) of stored credentials, sent back as placeholders when the switch turns it on again.
    pub credentials: Option<(Option<String>, Option<String>)>,
}

fn send_test(ctx: &Ctx, csrf: &str) -> String {
    format!(
        "<form class=\"button_to\" method=\"post\" action=\"{}\"><button class=\"rbutton rbutton--secondary\" type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" /></form>",
        escape(&ctx.path("/settings/send_test_email")),
        ctx.t("settings.email_notifications.send_test")
    )
}

pub fn email_notifications(ctx: &Ctx, csrf: &str, m: &Mail) -> String {
    let label = ctx.t("settings.index.email_notifications_section");
    let inner = if m.env_available {
        let control = format!("\n      {}\n", send_test(ctx, csrf));
        format!("\n\n    {}", row(&label, Some(&escape(m.env_name)), "", "", None, Some(&control)))
    } else {
        let on = m.configured || m.show_form;
        let fields: Vec<(&str, Option<&str>)> = match &m.credentials {
            Some((host, port)) => vec![("smtp_username", Some(super::preferences::REDACTED)), ("smtp_password", Some(super::preferences::REDACTED)), ("smtp_host", host.as_deref()), ("smtp_port", port.as_deref())],
            None => vec![],
        };
        let configured = if m.configured {
            format!(
                "\n        {}\n        {}",
                send_test(ctx, csrf),
                super::dialog::confirm(
                    csrf,
                    &ctx.t("settings.email_notifications.disconnect"),
                    &ctx.t("settings.index.api_key_confirmation"),
                    &ctx.t("settings.email_notifications.disconnect"),
                    &ctx.path("/settings/disconnect_email"),
                    "delete",
                    &[]
                )
            )
        } else {
            String::new()
        };
        let control = format!(
            "{configured}\n      {}\n",
            switch(csrf, &ctx.path("/settings/update_email_notifications"), "email_notifications", "smtp_provider", if on { "" } else { "custom_smtp" }, if on { "on" } else { "off" }, Some(&label), &fields)
        );
        let hint = m.configured.then_some(m.configured_hint);
        let mut out = format!("\n\n    {}\n", row(&label, hint, "", "", None, Some(&control)));
        if m.show_form && !m.configured {
            let input = |kind: &str, name: &str, value: Option<&str>, key: &str, autocomplete: bool| {
                let placeholder = ctx.t(key);
                format!(
                    "<input type=\"{kind}\" name=\"{name}\" id=\"{name}\"{}{} class=\"rinput rinput--outline rinput--text\" aria-label=\"{placeholder}\" placeholder=\"{placeholder}\" />",
                    value.map(|value| format!(" value=\"{value}\"")).unwrap_or_default(),
                    if autocomplete { " autocomplete=\"off\"" } else { "" }
                )
            };
            out.push_str(&format!(
                "\n      {}\n        <input type=\"hidden\" name=\"smtp_provider\" value=\"custom_smtp\" />\n        {}\n        {}\n        {}\n        {}\n        <p class=\"srow__hint\">{}</p>\n        <input type=\"submit\" name=\"commit\" value=\"{set}\" class=\"rbutton rbutton--secondary\" data-disable-with=\"{set}\" />\n</form>",
                form_open(" class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"email_notifications\"", &ctx.path("/settings/update_email_notifications"), "patch", csrf),
                input("text", "smtp_host", Some("smtp.gmail.com"), "settings.email_notifications.smtp_host", false),
                input("number", "smtp_port", Some("587"), "settings.email_notifications.smtp_port", false),
                input("text", "smtp_username", None, "settings.email_notifications.smtp_username", true),
                input("password", "smtp_password", None, "settings.email_notifications.smtp_password", true),
                ctx.t("settings.email_notifications.smtp_hint_html"),
                set = ctx.t("settings.email_notifications.set")
            ));
        }
        out
    };
    format!("<turbo-frame class=\"sitem\" id=\"email_notifications\">{inner}</turbo-frame>")
}

/// settings/widgets/_market_data.
pub struct Market<'a> {
    /// The platform's own feed, the only one there is: its note.
    pub hosted: Option<&'a str>,
    /// "coingecko", "deltabadger", or "" when off.
    pub selected: &'a str,
    pub coingecko_configured: bool,
    pub platform_connected: bool,
    pub platform_note: &'a str,
    pub coingecko_instructions: &'a str,
    pub platform_errors: Option<&'a str>,
    /// The provider whose connect card Settings draws: a tile picked, or a write that needs its form again.
    pub card: Option<&'a str>,
}

/// The providers as the wizard's exchange tiles (CoinGecko's mark is its wordmark, deltabadger's the logo beside the
/// lowercase name), each opening its connect card at `href(provider)` in `frame`.
pub fn market_data_providers(ctx: &Ctx, frame: &str, href: impl Fn(&str) -> String) -> String {
    let tiles: String = [("coingecko", "CoinGecko", false, include_str!("../../../templates/svg/_coingecko_wordmark.html")),
                         ("deltabadger", "deltabadger", true, include_str!("../../../templates/svg/_40x30_logo.html"))]
        .iter()
        .map(|(key, name, shown, mark)| format!(
            "
        <div class=\"exchange-grid__option\">\n          <a class=\"exchange-grid__item exchange-grid__item--provider\" data-turbo-frame=\"{frame}\" href=\"{}\" aria-label=\"{name}\">{mark}{}</a>\n        </div>",
            escape(&href(key)), if *shown { format!("<span>{name}</span>") } else { String::new() }))
        .collect();
    format!("<section class=\"exchange-group\">\n      <h3 class=\"label\">{}</h3>\n      <div class=\"exchange-grid\">{tiles}\n      </div>\n    </section>", ctx.t("bot.setup.market_data.providers"))
}

/// D-MARKET-DATA-PICK: Connect's market data, as the index wizard connects it: with no provider, the intro and the
/// providers' tiles; a tile opens its connect card (Back returns to the tiles). A connected provider is its row: CoinGecko
/// with resync and disconnect and, under it, the upgrade to the Deltabadger Data API; the claimed platform with its disconnect.
pub fn market_data(ctx: &Ctx, csrf: &str, m: &Market) -> String {
    let heading = format!("\n  <h3 class=\"sheet__heading sheet__heading--first\">{}</h3>", ctx.t("settings.index.resync_assets_section"));
    if let Some(note) = m.hosted {
        return format!("<turbo-frame id=\"market_data_settings\">{heading}\n  <p class=\"sheet__intro\">{note}</p>\n</turbo-frame>");
    }
    if let Some(provider) = m.card {
        return market_data_card(ctx, csrf, provider, m, false);
    }
    let mut body = heading;
    if m.coingecko_configured {
        let control = format!(
            "\n              <form class=\"button_to\" method=\"post\" action=\"{}\"><button class=\"rbutton rbutton--secondary\" type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" /></form>\n              {}\n",
            escape(&ctx.path("/settings/resync_assets")),
            ctx.t("settings.resync_assets.button"),
            super::dialog::confirm(csrf, &ctx.t("settings.market_data.disconnect"), &ctx.t("settings.index.api_key_confirmation"), &ctx.t("settings.market_data.disconnect"), &ctx.path("/settings/disconnect_market_data"), "delete", &[])
        );
        body.push_str(&format!("\n  {}", row("CoinGecko", Some(&ctx.t("settings.market_data.coingecko_configured")), "", "", None, Some(&control))));
        // On CoinGecko the Deltabadger Data API stays on offer: stocks too, and a stable IP to whitelist. Its card claims a code,
        // or for a claimed platform is one Connect button that makes it the provider.
        let upgrade = format!(
            "\n          <a class=\"rbutton rbutton--secondary\" data-turbo-frame=\"settings_pane\" href=\"{}\">{}</a>\n",
            escape(&ctx.path("/settings/connect?provider=deltabadger")),
            ctx.t("settings.market_data.upgrade_button")
        );
        body.push_str(&format!("\n  {}", row(&ctx.t("settings.market_data.upgrade_title"), Some(&ctx.t("settings.market_data.upgrade")), "", "", None, Some(&upgrade))));
    } else if m.selected != "deltabadger" {
        let tiles = market_data_providers(ctx, "settings_pane", |key| ctx.path(&format!("/settings/connect?provider={key}")));
        body.push_str(&format!("\n  <p class=\"sheet__intro\">{}</p>\n  {tiles}", ctx.t("settings.market_data.intro")));
    }
    if m.platform_connected {
        let control = format!(
            "\n          {}\n",
            super::dialog::confirm(csrf, &ctx.t("settings.platform.disconnect"), &ctx.t("settings.index.api_key_confirmation"), &ctx.t("settings.platform.disconnect"), &ctx.path("/settings/platform_connection"), "delete", &[])
        );
        body.push_str(&format!("\n  {}", row(&ctx.t("settings.platform.option"), Some(m.platform_note), "", "", None, Some(&control))));
    }
    format!("<turbo-frame id=\"market_data_settings\">{body}\n</turbo-frame>")
}

/// D-INDEX-CONNECT-IN-VIEW: a market-data provider's connect card in the index wizard's Index view, drawn as the wizard's
/// exchange key card (`.set-api--connect`): the provider's mark and name with Connect on that line, its field under them, the
/// walkthrough (CoinGecko's) or the subscription note (Deltabadger's) under that. The same frame, fields and writes as Settings'
/// widget, each form carrying `wizard=1`. Settings' Connect draws it too (`wizard` false): Back to its tiles, no `wizard=1`.
pub fn market_data_card(ctx: &Ctx, csrf: &str, provider: &str, m: &Market, wizard: bool) -> String {
    let deltabadger = provider == "deltabadger";
    let (action, method) = if deltabadger && !m.platform_connected { ("/settings/platform_connection", "post") } else { ("/settings/update_market_data", "patch") };
    let method_field = if method == "post" { String::new() } else { format!("<input type=\"hidden\" name=\"_method\" value=\"{method}\" />") };
    let (name, mark) = if deltabadger { ("deltabadger".to_string(), include_str!("../../../templates/svg/_40x30_logo.html")) }
                       else { ("CoinGecko".to_string(), include_str!("../../../templates/svg/_coingecko.html")) };
    let field = |name: &str, label: &str, error: Option<&str>| format!(
        "\n    <input autofocus=\"autofocus\" required=\"required\" autocomplete=\"off\" data-controller=\"autofocus\" class=\"rinput rinput--outline rinput--text{}\" placeholder=\"{label}\" aria-label=\"{label}\" type=\"text\" name=\"{name}\" id=\"{name}\" />{}",
        if error.is_some() { " rinput--invalid" } else { "" },
        error.map(|e| format!("\n    <p class=\"sform__error\" role=\"alert\">{}</p>", escape(e))).unwrap_or_default());
    let fields = match (deltabadger, m.platform_connected) {
        (false, _) => format!("<input type=\"hidden\" name=\"market_data_provider\" value=\"coingecko\" />{}", field("coingecko_api_key", &ctx.t("setup.coingecko_api_key"), None)),
        // Claimed but not the provider: Connect alone.
        (true, true) => "<input type=\"hidden\" name=\"market_data_provider\" value=\"deltabadger\" />".to_string(),
        (true, false) => field("claim_code", &ctx.t("settings.platform.code"), m.platform_errors),
    };
    let help = if deltabadger {
        let t = |key: &str| ctx.t(&format!("bot.setup.market_data.subscription.{key}"));
        let perks: String = ["indexes", "history", "ip"].iter().map(|k| format!("<p><b>{}</b><br>{}</p>", t(&format!("{k}_title")), t(k))).collect();
        format!("<div class=\"sheet__prose sheet__prose--perks\"><p>{}</p>{perks}</div>", t("lead_html"))
    } else {
        format!("<h3 class=\"set-api__eyebrow\">{}</h3>\n      {}", crate::web::i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", crate::web::i18n::Arg::Text("CoinGecko"))]), m.coingecko_instructions)
    };
    let connect = ctx.t("button.connect");
    // The logo is Back: the Index view's providers, or Connect's.
    let (back, frame, mark_wizard) = if wizard { (crate::web::wizard::INDEX_VIEW, "modal_content", "<input type=\"hidden\" name=\"wizard\" value=\"1\" />") }
                                     else { ("/settings/connect", "settings_pane", "") };
    let (back, back_label) = (escape(&ctx.path(back)), ctx.t("button.back"));
    // In Settings the wizard's key-step layout goes around it, as the Index view's does (the card's styles are scoped to it).
    let (layout, layout_end) = if wizard { ("", "") } else { ("<div class=\"bot-creation-layout bot-creation-layout--api\"><div class=\"vt-step-body\">\n", "\n</div></div>") };
    format!(
        "<turbo-frame id=\"market_data_settings\">\n{layout}<div class=\"set-api set-api--connect\">\n  <header class=\"set-api__brand\">\n  <a class=\"set-api__back\" href=\"{back}\" data-turbo-frame=\"{frame}\" data-wizard-dir=\"back\" title=\"{back_label}\">\n  <span class=\"set-api__mark{}\" aria-hidden=\"true\">{mark}</span>\n  <h2 class=\"set-api__title\">{}</h2>\n  </a>\n</header>\n  <form class=\"set-api__form form\" data-turbo-frame=\"market_data_settings\" action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">{method_field}<input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />{mark_wizard}{fields}\n    <div class=\"set-api__actions\">\n      <input type=\"submit\" name=\"commit\" value=\"{connect}\" class=\"rbutton rbutton--secondary\" data-turbo-stream=\"true\" data-turbo-submits-with=\"{}\" data-disable-with=\"{connect}\" />\n    </div>\n</form>\n    <section class=\"set-api__instructions\">\n      {help}\n    </section>\n</div>{layout_end}\n</turbo-frame>",
        if deltabadger { " set-api__mark--deltabadger" } else { "" }, escape(&name), escape(&ctx.path(action)), ctx.t("bot.setup.validating"))
}

/// D-TRACKER-COINGECKO-SHEET: the tracker's CoinGecko card (its "Connect to CoinGecko" links, a report with no market data), as
/// the wizard draws `market_data_card` but with no Back (no step before it): for an admin the key field posting to
/// `/tracker/setup_coingecko`, `error` under it; for anyone else who to ask.
pub fn tracker_coingecko_card(ctx: &Ctx, csrf: &str, admin: bool, error: Option<&str>) -> String {
    let (name, mark) = ("CoinGecko", include_str!("../../../templates/svg/_coingecko.html"));
    let header = format!("  <header class=\"set-api__brand\">\n  <span class=\"set-api__mark\" aria-hidden=\"true\">{mark}</span>\n  <h2 class=\"set-api__title\">{name}</h2>\n</header>");
    if !admin { return format!("<div class=\"set-api set-api--connect\">\n{header}\n  <p class=\"set-api__note\">{}</p>\n</div>", ctx.t("setup.coingecko_admin_only")) }
    let label = ctx.t("setup.coingecko_api_key");
    let invalid = if error.is_some() { " rinput--invalid\" aria-invalid=\"true\" aria-describedby=\"api_key_error" } else { "" };
    let error = error.map(|e| format!("\n    <p class=\"sform__error\" id=\"api_key_error\" role=\"alert\">{}</p>", escape(e))).unwrap_or_default();
    let connect = ctx.t("button.connect");
    let how = crate::web::i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", crate::web::i18n::Arg::Text(name))]);
    format!(
        "<div class=\"set-api set-api--connect\">\n{header}\n  <p class=\"set-api__note\">{}</p>\n  <form class=\"set-api__form form\" {} action=\"{}\" accept-charset=\"UTF-8\" method=\"post\"><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />\n    <input required=\"required\" autofocus=\"autofocus\" autocomplete=\"off\" data-controller=\"autofocus\" class=\"rinput rinput--outline rinput--text{invalid}\" placeholder=\"{label}\" aria-label=\"{label}\" type=\"text\" name=\"api_key\" id=\"api_key\" />{error}\n    <div class=\"set-api__actions\">\n      <input type=\"submit\" name=\"commit\" value=\"{connect}\" class=\"rbutton rbutton--secondary\" data-turbo-stream=\"true\" data-turbo-submits-with=\"{}\" data-disable-with=\"{connect}\" />\n    </div>\n</form>\n    <section class=\"set-api__instructions\">\n      <h3 class=\"set-api__eyebrow\">{how}</h3>\n      {}\n    </section>\n</div>",
        ctx.t("tracker.setup_coingecko_html"), super::keys::SHEET_FORM, escape(&ctx.path("/tracker/setup_coingecko")), ctx.t("bot.setup.validating"), ctx.t("setup.coingecko_api_key_hint_html"))
}

/// One exchange key of settings/widgets/_api_keys.
pub struct Key {
    pub id: i64,
    pub name: String,
    pub name_id: String,
    pub read_only: bool,
    pub withdrawal: bool,
    pub instructions: bool,
}

/// settings/widgets/_api_keys: the keys, a row each, under Trading keys and (when there are any) Withdrawal keys.
pub fn api_keys(ctx: &Ctx, csrf: &str, keys: &[Key]) -> String {
    let mut out = String::from("<turbo-frame id=\"api_keys\">");
    for (index, (heading, withdrawal)) in [("settings.index.trading_keys_section", false), ("settings.index.withdrawal_keys_section", true)].into_iter().enumerate() {
        let group: Vec<&Key> = keys.iter().filter(|key| key.withdrawal == withdrawal).collect();
        if index > 0 && group.is_empty() { continue; }
        out.push_str(&format!("\n    <h3 class=\"sheet__heading{}\">{}</h3>", if index == 0 { " sheet__heading--first" } else { "" }, ctx.t(heading)));
        if group.is_empty() {
            out.push_str(&format!("\n      <p class=\"sheet__empty\">{}</p>\n", ctx.t("settings.sections.no_keys")));
        }
        for key in group {
            let permissions = if key.instructions {
                format!(
                    "\n          <a class=\"rbutton rbutton--secondary\" data-turbo-frame=\"settings_pane\" href=\"{}\">{}</a>",
                    escape(&ctx.path(&format!("/settings/api_key_permissions/{}", key.id))),
                    ctx.t("settings.sections.permissions")
                )
            } else {
                String::new()
            };
            let control = format!(
                "{permissions}\n        {}\n",
                super::dialog::confirm(csrf, &ctx.t("button.remove"), &ctx.t("settings.api_keys.confirm_delete"), &ctx.t("button.delete"), &ctx.path(&format!("/settings/destroy_api_key/{}", key.id)), "delete", &[])
            );
            let label = format!(
                "{}{}",
                escape(&key.name),
                if key.read_only { format!("<span class=\"srow__aside\">{}</span>", ctx.t("links.tracker")) } else { String::new() }
            );
            out.push_str(&format!(
                "\n      {}",
                row(&label, None, "srow--key", &format!(" data-api-key=\"{}\"", key.id), Some(crate::web::bot::exchange_svg(&key.name_id)), Some(&control))
            ));
        }
    }
    out.push_str("</turbo-frame>");
    out
}

const COPY: &str = "<svg class=\"icon-16\" fill=\"none\" viewbox=\"0 0 16 16\" xmlns=\"http://www.w3.org/2000/svg\">\n<path class=\"fill--icon\" clip-rule=\"evenodd\" d=\"M14 7q0-1-1-1H7a1 1 0 0 0-1 1v6q0 1 1 1h6q1 0 1-1zM7 4a3 3 0 0 0-3 3v6a3 3 0 0 0 3 3h6a3 3 0 0 0 3-3V7a3 3 0 0 0-3-3z\" fill-rule=\"evenodd\"></path>\n<path class=\"fill--icon\" d=\"M10 3q0-1-1-1H3a1 1 0 0 0-1 1v6q0 1 1 1h2v2H3a3 3 0 0 1-3-3V3a3 3 0 0 1 3-3h6a3 3 0 0 1 3 3v2h-2z\"></path>\n</svg>\n";

/// settings/widgets/_copy_tag: a pill that copies `value`, showing it without its scheme.
fn copy_tag(ctx: &Ctx, value: &str, id: Option<&str>) -> String {
    let shown = value.strip_prefix("https://").or_else(|| value.strip_prefix("http://")).unwrap_or(value);
    format!(
        "<span class=\"tag tag--copy\"{} title=\"{}\" data-controller=\"clipboard\" data-clipboard-text-value=\"{}\" data-action=\"click-&gt;clipboard#copy\">{COPY}{}</span>",
        id.map(|id| format!(" id=\"{id}\"")).unwrap_or_default(),
        ctx.t("button.copy"),
        escape(value),
        escape(shown)
    )
}

/// settings/widgets/_mcp: the URL and paper trading.
pub fn mcp(ctx: &Ctx, csrf: &str, root: &str, dry_run: bool) -> String {
    let url = format!("\n    {}\n\n", copy_tag(ctx, &format!("{root}/mcp"), Some("mcp_url_display")));
    let paper = format!(
        "\n    {}\n",
        switch(csrf, &ctx.path("/settings/update_mcp_dry_run"), "mcp_settings", "enabled", if dry_run { "0" } else { "1" }, if dry_run { "on" } else { "off" }, Some(&ctx.t("settings.mcp.dry_run_label")), &[])
    );
    format!(
        "<turbo-frame id=\"mcp_settings\">\n  <h3 class=\"sheet__heading sheet__heading--first\">{}</h3>\n  <p class=\"sheet__intro\">{}</p>\n\n  {}\n  {}</turbo-frame>",
        ctx.t("settings.sections.mcp_server"),
        ctx.t("settings.mcp.description"),
        row("URL", None, "", "", None, Some(&url)),
        row(&ctx.t("settings.mcp.dry_run_title"), Some(&ctx.t("settings.mcp.dry_run_description")), "", "", None, Some(&paper))
    )
}

/// settings/widgets/_rest: the URL, the token (shown in full only in Regenerate's answer) and the docs.
pub fn rest(ctx: &Ctx, csrf: &str, root: &str, token: &str) -> String {
    let url = format!("\n    {}\n\n", copy_tag(ctx, &format!("{root}/api/v1"), Some("rest_api_url_display")));
    let token = format!(
        "\n    {}\n\n    {}\n",
        copy_tag(ctx, token, None),
        super::dialog::confirm(csrf, &ctx.t("settings.rest.regenerate_token"), &ctx.t("settings.rest.regenerate_confirm"), &ctx.t("settings.rest.regenerate_token"), &ctx.path("/settings/regenerate_api_token"), "post", &[])
    );
    let docs = format!(
        "\n    <a class=\"rbutton rbutton--secondary\" title=\"{}\" data-turbo=\"false\" href=\"{}\">deltabadger-api.md</a>\n",
        ctx.t("settings.rest.download_api_docs"),
        escape(&ctx.path("/settings/download_api_docs"))
    );
    format!(
        "<turbo-frame id=\"rest_settings\">\n  <h3 class=\"sheet__heading sheet__heading--first\">{}</h3>\n  <p class=\"sheet__intro\">{}</p>\n\n\n  {}\n  {}\n  {}</turbo-frame>",
        ctx.t("settings.rest.title"),
        ctx.t("settings.rest.description"),
        row("URL", None, "", "", None, Some(&url)),
        row(&ctx.t("settings.rest.token"), None, "srow--token", "", None, Some(&token)),
        row(&ctx.t("settings.rest.docs"), None, "", "", None, Some(&docs))
    )
}

/// settings/widgets/_tool_permissions for one surface: a switch per group, a switch per tool under it.
pub fn tool_permissions(ctx: &Ctx, csrf: &str, surface: &str, enabled: &[String]) -> String {
    let (tool_url, group_url) = (ctx.path(&format!("/settings/update_{surface}_tool_permissions")), ctx.path(&format!("/settings/update_{surface}_tool_group_permissions")));
    let mut out = format!(
        "<turbo-frame id=\"tool_permissions\">\n  <h3 class=\"sheet__heading\">{}</h3>\n  <p class=\"sheet__intro\">{}</p>\n",
        ctx.t("settings.mcp.tools_title"),
        ctx.t("settings.mcp.tools_description")
    );
    for (group, tools) in crate::web::oauth::TOOL_GROUPS {
        let on = tools.iter().filter(|tool| enabled.iter().any(|name| name == *tool)).count();
        let state = if on == 0 { "off" } else if on == tools.len() { "on" } else { "partial" };
        let control = format!("\n      {}\n", switch(csrf, &group_url, "tool_permissions", "enabled", if on == tools.len() { "0" } else { "1" }, state, None, &[("group", Some(group))]));
        out.push_str(&format!(
            "\n    {}",
            row(&ctx.t(&format!("settings.mcp.tools_{group}")), None, "srow--group", &format!(" data-group=\"{group}\" data-surface=\"{surface}\""), None, Some(&control))
        ));
        for tool in tools.iter() {
            let is_on = enabled.iter().any(|name| name == tool);
            let control = format!("\n        {}\n", switch(csrf, &tool_url, "tool_permissions", "enabled", if is_on { "0" } else { "1" }, if is_on { "on" } else { "off" }, None, &[("tool_name", Some(tool))]));
            out.push_str(&format!(
                "      {}",
                row(
                    &ctx.t(&format!("settings.mcp.tools.{tool}.label")),
                    Some(&ctx.t(&format!("settings.mcp.tools.{tool}.description"))),
                    "srow--tool",
                    &format!(" data-tool=\"{tool}\" data-surface=\"{surface}\""),
                    None,
                    Some(&control)
                )
            ));
        }
    }
    out.push_str("</turbo-frame>");
    out
}

/// One connected client of a surface, as settings/widgets/_connected_client draws it.
pub struct Client {
    pub id: i64,
    pub name: String,
    pub date: String,
    /// Whether the client holds this surface's scope: switches only then.
    pub scoped: bool,
    /// (group, state, enabled) per group.
    pub groups: Vec<(&'static str, &'static str, &'static str)>,
}

/// settings/widgets/_connected_clients for one surface.
pub fn connected_clients(ctx: &Ctx, csrf: &str, surface: &str, clients: &[Client]) -> String {
    let mut out = format!("<turbo-frame id=\"connected_clients\">\n  <h3 class=\"sheet__heading\">{}</h3>", ctx.t("settings.mcp.connected_clients"));
    if clients.is_empty() {
        out.push_str(&format!("\n    <p class=\"sheet__empty\">{}</p>", ctx.t("settings.mcp.no_clients")));
    } else {
        out.push_str(&format!("\n    <p class=\"sheet__intro\">{}</p>", ctx.t("settings.mcp.client_permissions_hint")));
        for client in clients {
            let revoke = format!(
                "\n    {}\n",
                super::dialog::confirm(csrf, &ctx.t("settings.mcp.revoke_client"), &ctx.t("settings.mcp.revoke_client_confirm"), &ctx.t("settings.mcp.revoke_client"), &ctx.path(&format!("/settings/revoke_mcp_client/{}", client.id)), "delete", &[("surface", surface)])
            );
            out.push_str(&format!(
                "\n    <div class=\"sclient sitem\" data-client=\"{}\">\n  {}",
                client.id,
                row(&escape(&client.name), Some(&escape(&client.date)), "srow--client", "", None, Some(&revoke))
            ));
            if client.scoped {
                let url = ctx.path(&format!("/settings/update_client_tool_permissions/{}", client.id));
                for (group, state, enabled) in &client.groups {
                    let control = format!("\n      {}\n", switch(csrf, &url, "connected_clients", "enabled", enabled, state, None, &[("surface", Some(surface)), ("group", Some(group))]));
                    out.push_str(&format!(
                        "    {}",
                        row(&ctx.t(&format!("settings.mcp.tools_{group}")), None, "srow--tool", &format!(" data-group=\"{group}\" data-surface=\"{surface}\""), None, Some(&control))
                    ));
                }
            }
            out.push_str("</div>");
        }
    }
    out.push_str("\n</turbo-frame>");
    out
}
