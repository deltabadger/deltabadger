//! The settings sections' rows, as app/views/settings/widgets/* render them inside the dialog
//! (rust/tests/pages.rs holds each to Rails' markup).
use super::dialog::{row, switch};
use crate::web::{i18n::escape, layout::Ctx};
use std::collections::BTreeMap;

/// A form's opening tag as `form_with` writes it, the method override and the token after it.
pub fn form_open(attributes: &str, action: &str, method: &str, csrf: &str) -> String {
    let method_field = if method == "post" { String::new() } else { format!("<input type=\"hidden\" name=\"_method\" value=\"{method}\" />") };
    format!("<form{attributes} action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">{method_field}<input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />", escape(action))
}

/// ActionView's field_error_proc as config/initializers/inline_form_errors.rb sets it: Nokogiri re-writes the
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

/// settings/widgets/_name.
pub fn name(ctx: &Ctx, csrf: &str, value: &str, errors: &Errors) -> String {
    let input = format!(
        "<input class=\"rinput rinput--outline rinput--text\" required=\"required\" pattern=\"^\\p{{L}}+(\\s+\\p{{L}}+)*$\" autocomplete=\"name\" aria-label=\"{}\" type=\"text\" value=\"{}\" name=\"user[name]\" id=\"user_name\" />",
        ctx.t("settings.index.name_section"),
        escape(value)
    );
    let form = format!(
        "\n    {}\n      {}\n      {}\n</form>",
        form_open(" class=\"sform\" data-turbo-frame=\"update_name_form\"", &ctx.path("/settings/update_name"), "patch", csrf),
        field(&input, errors.get("name")),
        submit(&ctx.t("button.save"), "")
    );
    format!("<turbo-frame class=\"sitem\" id=\"update_name_form\">\n  {}</turbo-frame>", row(&ctx.t("settings.index.name_section"), None, "", "", None, Some(&form)))
}

fn submit(label: &str, extra: &str) -> String {
    format!("<input type=\"submit\" name=\"commit\" value=\"{label}\" class=\"rbutton rbutton--outline\"{extra} data-disable-with=\"{label}\" />")
}

/// A row that opens into a form: the summary is the row with its Change button.
fn details(frame: &str, label: &str, hint: Option<&str>, open: bool, ctx: &Ctx, form: &str) -> String {
    let control = format!("\n        <span class=\"rbutton rbutton--outline\">{}</span>\n", ctx.t("settings.sections.change"));
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
        "\n    <a class=\"rbutton rbutton--outline\" href=\"{}\">{}</a>\n",
        escape(&ctx.path("/settings/edit_two_fa")),
        ctx.t(if enabled { "helpers.label.settings.disable_two_fa" } else { "helpers.label.settings.enable_two_fa" })
    );
    let hint = ctx.t(if enabled { "helpers.label.settings.enabled" } else { "helpers.label.settings.disabled" });
    format!("<turbo-frame class=\"sitem\" id=\"two_fa\">\n  {}</turbo-frame>", row(&ctx.t("settings.index.two_factor_section"), Some(&hint), "", "", None, Some(&control)))
}

/// settings/edit_two_fa: the 2FA row opened, the QR code (when enabling) and the code that confirms it.
pub fn two_fa_open(ctx: &Ctx, csrf: &str, enabled: bool, qr: &str, error: Option<&String>) -> String {
    let control = format!("\n      <a class=\"rbutton rbutton--outline\" href=\"{}\">{}</a>\n", escape(&ctx.path("/settings/account")), ctx.t("button.cancel"));
    let label = ctx.t("helpers.label.settings.enter_two_fa_code");
    let input = format!(
        "<input class=\"rinput rinput--outline rinput--text\" required=\"required\" pattern=\"\\d{{6}}\" autocomplete=\"one-time-code\" inputmode=\"numeric\" placeholder=\"{label}\" aria-label=\"{label}\" type=\"text\" name=\"user[otp_code_token]\" id=\"user_otp_code_token\" />"
    );
    let button = ctx.t(if enabled { "helpers.label.settings.disable_two_fa" } else { "helpers.label.settings.enable_two_fa" });
    let qr = if qr.is_empty() { String::new() } else { format!("{qr}\n") };
    format!(
        "<turbo-frame class=\"sitem\" id=\"two_fa\">\n  <div>\n    {}    {}\n      {qr}\n      {}\n      <input type=\"submit\" name=\"commit\" value=\"{button}\" class=\"rbutton rbutton--outline {}\" data-disable-with=\"{button}\" />\n</form>  </div>\n</turbo-frame>",
        row(&ctx.t("settings.index.two_factor_section"), None, "", "", None, Some(&control)),
        form_open(" class=\"sform sform--stack\"", &ctx.path("/settings/update_two_fa"), "patch", csrf),
        field(&input, error),
        if enabled { "rbutton--danger" } else { "" }
    )
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

/// settings/widgets/_registration.
pub fn registration(ctx: &Ctx, csrf: &str, open: bool) -> String {
    let label = ctx.t("settings.index.registration_section");
    let control = format!(
        "\n    {}\n",
        switch(csrf, &ctx.path("/settings/update_registration"), "registration_settings", "registration_open", if open { "0" } else { "1" }, if open { "on" } else { "off" }, Some(&label), &[])
    );
    format!("<turbo-frame class=\"sitem\" id=\"registration_settings\">\n  {}</turbo-frame>", row(&label, None, "", "", None, Some(&control)))
}

/// settings/widgets/_wash_sale: the shared question, Confirm, and what is locked right now.
pub fn wash(ctx: &Ctx, csrf: &str, question: &str, decided: bool, enabled: bool, locked: &str) -> String {
    let locked = if enabled {
        format!("\n      <div class=\"srow__hint\">\n        <b>{}</b>{locked}      </div>\n", ctx.t("settings.wash_sale.locked_title"))
    } else {
        String::new()
    };
    format!(
        "<turbo-frame class=\"sitem\" id=\"wash_sale\">\n  <div class=\"srow srow--block\">\n    <div class=\"srow__text\"><div class=\"srow__label\">{}</div></div>\n\n    {}\n      {question}\n      <button name=\"button\" type=\"submit\"{} class=\"rbutton rbutton--outline\" data-form--checkbox-enables-target=\"submit\">{}</button>\n</form>{locked}\n  </div>\n</turbo-frame>",
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
    pub name: String,
    pub email: String,
    pub password: String,
    pub locale: String,
    pub time_zone: String,
    pub two_fa: String,
    pub advanced: String,
    pub wash: String,
    pub admin: Option<(&'a str, &'a str)>,
}

pub fn account(ctx: &Ctx, a: &Account) -> String {
    let instance = a
        .admin
        .map(|(registration, email)| format!("\n  <h3 class=\"settings__heading\">{}</h3>\n  {registration}\n  {email}\n  \n", ctx.t("settings.sections.instance")))
        .unwrap_or_default();
    format!(
        "  <h3 class=\"settings__heading settings__heading--first\">{}</h3>\n{}\n{}\n{}\n\n<h3 class=\"settings__heading\">{}</h3>\n{}\n{}\n\n<h3 class=\"settings__heading\">{}</h3>\n{}\n\n<h3 class=\"settings__heading\">{}</h3>\n{}\n{}\n{instance}",
        ctx.t("settings.sections.profile"),
        a.name,
        a.email,
        a.password,
        ctx.t("settings.sections.preferences"),
        a.locale,
        a.time_zone,
        ctx.t("settings.sections.security"),
        a.two_fa,
        ctx.t("links.bots"),
        a.advanced,
        a.wash
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
        "<form class=\"button_to\" method=\"post\" action=\"{}\"><button class=\"rbutton rbutton--outline\" type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" /></form>",
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
                "\n      {}\n        <input type=\"hidden\" name=\"smtp_provider\" value=\"custom_smtp\" />\n        {}\n        {}\n        {}\n        {}\n        <p class=\"srow__hint\">{}</p>\n        <input type=\"submit\" name=\"commit\" value=\"{set}\" class=\"rbutton rbutton--outline\" data-disable-with=\"{set}\" />\n</form>",
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

/// shared/_segmented as a radio group, where these widgets place it.
pub fn segmented(label: &str, fluid: bool, options: &[(&str, String, bool)]) -> String {
    let buttons: String = options
        .iter()
        .map(|(value, text, on)| {
            format!(
                "\n            <button type=\"button\" class=\"segmented__option{}\" role=\"radio\" aria-checked=\"{on}\" tabindex=\"{}\" data-value=\"{value}\" data-segmented-target=\"option\" data-action=\"segmented#select keydown-&gt;segmented#keys\">{text}</button>",
                if *on { " is-on" } else { "" },
                if *on { "0" } else { "-1" }
            )
        })
        .collect();
    format!(
        "\n<div class=\"segmented{}\" role=\"radiogroup\" aria-label=\"{}\" data-controller=\"segmented\" data-action=\"click-&gt;segmented#toggle\">\n  <span class=\"segmented__thumb\" data-segmented-target=\"thumb\"></span>\n  <div class=\"segmented__dropdown\" data-segmented-target=\"dropdown\">\n    <div class=\"segmented__panes\">\n      <div class=\"segmented__pane segmented__menu\">{buttons}\n      </div>\n    </div>\n  </div>\n  <span class=\"segmented__caret\" aria-hidden=\"true\">\n    <svg class=\"icon-16\" fill=\"none\" viewbox=\"0 0 16 16\" xmlns=\"http://www.w3.org/2000/svg\">\n<path clip-rule=\"evenodd\" d=\"M4.3 6.3a1 1 0 0 1 1.4 0L8 8.6l2.3-2.3a1 1 0 1 1 1.4 1.4l-3 3a1 1 0 0 1-1.4 0l-3-3a1 1 0 0 1 0-1.4Z\" fill-rule=\"evenodd\"></path>\n</svg>\n\n</span></div>\n",
        if fluid { " segmented--fluid" } else { "" },
        escape(label)
    )
}

/// settings/widgets/_stocks: Alpaca as the container's catalog source.
pub struct Stocks<'a> {
    pub hosted: Option<&'a str>,
    pub configured: bool,
    pub hint: Option<&'a str>,
    pub paper: bool,
    pub instructions: &'a str,
}

pub fn stocks(ctx: &Ctx, csrf: &str, s: &Stocks) -> String {
    if let Some(note) = s.hosted {
        return format!("<turbo-frame class=\"sitem\" id=\"stocks_settings\">\n    {}\n</turbo-frame>", row("Alpaca", Some(note), "", "", None, None));
    }
    let disconnect = if s.configured {
        format!(
            "\n        {}\n",
            super::dialog::confirm(csrf, &ctx.t("settings.stocks.disconnect"), &ctx.t("settings.index.api_key_confirmation"), &ctx.t("settings.stocks.disconnect"), &ctx.path("/settings/disconnect_stocks"), "delete", &[])
        )
    } else {
        String::new()
    };
    let mode = format!(
        "\n        {}",
        segmented("Mode", false, &[("paper", ctx.t("settings.stocks.mode_paper"), s.paper), ("live", ctx.t("settings.stocks.mode_live"), !s.paper)])
    );
    let connect = ctx.t("button.connect");
    format!(
        "<turbo-frame class=\"sitem\" id=\"stocks_settings\">\n    {}\n    {}\n      {}      <input value=\"{}\" data-form--segmented-field-target=\"field\" type=\"hidden\" name=\"alpaca_mode\" id=\"alpaca_mode\" />\n      <input autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"API Key\" aria-label=\"API Key\" type=\"text\" name=\"alpaca_api_key\" id=\"alpaca_api_key\" />\n      <input autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"API Secret\" aria-label=\"API Secret\" type=\"text\" name=\"alpaca_api_secret\" id=\"alpaca_api_secret\" />\n      {}\n      <details class=\"shelp\">\n        <summary>{}</summary>\n        <div class=\"settings__prose\">{}</div>\n      </details>\n</form></turbo-frame>",
        row("Alpaca", s.hint, "", "", None, Some(&disconnect)),
        form_open(
            " class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"stocks_settings\" data-controller=\"form--segmented-field\" data-action=\"segmented:change-&gt;form--segmented-field#set\"",
            &ctx.path("/settings/update_stocks"),
            "patch",
            csrf
        ),
        row("Mode", None, "srow--flush", "", None, Some(&mode)),
        if s.paper { "paper" } else { "live" },
        submit(&connect, &format!(" data-turbo-stream=\"true\" data-turbo-submits-with=\"{}\"", ctx.t("bot.setup.validating"))),
        crate::web::i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", crate::web::i18n::Arg::Text("Alpaca"))]),
        s.instructions
    )
}

/// settings/widgets/_market_data.
pub struct Market<'a> {
    /// The platform's own feed, the only one there is: its note.
    pub hosted: Option<&'a str>,
    /// "coingecko", "deltabadger", or "" when off.
    pub selected: &'a str,
    pub coingecko_configured: bool,
    pub coingecko_key_blank: bool,
    pub platform_connected: bool,
    pub platform_note: &'a str,
    pub coingecko_instructions: &'a str,
    pub platform_errors: Option<&'a str>,
}

pub fn market_data(ctx: &Ctx, csrf: &str, m: &Market) -> String {
    let label = ctx.t("settings.index.resync_assets_section");
    if let Some(note) = m.hosted {
        return format!("<turbo-frame id=\"market_data_settings\">\n\n    {}\n</turbo-frame>", row(&label, Some(note), "srow--heading", "", None, None));
    }
    let on = !m.selected.is_empty();
    let turn_on = if on { "" } else if m.platform_connected { "deltabadger" } else { "coingecko" };
    let switch = format!(
        "\n        {}\n",
        switch(csrf, &ctx.path("/settings/update_market_data"), "market_data_settings", "market_data_provider", turn_on, if on { "on" } else { "off" }, Some(&label), &[])
    );
    let mut body = format!(
        "\n\n    <div data-controller=\"form--market-data\" data-action=\"segmented:change->form--market-data#select\"\n         data-form--market-data-platform-connected-value=\"{}\">\n      {}\n",
        m.platform_connected,
        row(&label, None, "srow--heading", "", None, Some(&switch))
    );
    if on {
        let provider = ctx.t("settings.sections.provider");
        let control = format!(
            "\n          {}\n            {}            <input value=\"{}\" data-form--market-data-target=\"provider\" type=\"hidden\" name=\"market_data_provider\" id=\"market_data_provider\" />\n</form>",
            form_open(" data-form--market-data-target=\"providerForm\" data-turbo-frame=\"market_data_settings\"", &ctx.path("/settings/update_market_data"), "patch", csrf),
            segmented(&provider, true, &[("coingecko", "CoinGecko".into(), m.selected == "coingecko"), ("deltabadger", ctx.t("settings.platform.option"), m.selected == "deltabadger")]),
            m.selected
        );
        body.push_str(&format!("        {}\n", row(&provider, None, "", "", None, Some(&control))));
        let connect = ctx.t("button.connect");
        let validating = format!(" data-turbo-stream=\"true\" data-turbo-submits-with=\"{}\"", ctx.t("bot.setup.validating"));
        let coingecko = if m.coingecko_configured {
            let control = format!(
                "\n              <form class=\"button_to\" method=\"post\" action=\"{}\"><button class=\"rbutton rbutton--outline\" type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" /></form>\n              {}\n",
                escape(&ctx.path("/settings/resync_assets")),
                ctx.t("settings.resync_assets.button"),
                super::dialog::confirm(csrf, &ctx.t("settings.market_data.disconnect"), &ctx.t("settings.index.api_key_confirmation"), &ctx.t("settings.market_data.disconnect"), &ctx.path("/settings/disconnect_market_data"), "delete", &[])
            );
            format!("\n            {}", row("CoinGecko", Some(&ctx.t("settings.market_data.coingecko_configured")), "", "", None, Some(&control)))
        } else if m.coingecko_key_blank {
            let key = ctx.t("setup.coingecko_api_key");
            format!(
                "\n            {}\n              <input value=\"coingecko\" type=\"hidden\" name=\"market_data_provider\" id=\"market_data_provider\" />\n              <input required=\"required\" autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{key}\" aria-label=\"{key}\" type=\"text\" name=\"coingecko_api_key\" id=\"coingecko_api_key\" />\n              {}\n              <details class=\"shelp\">\n                <summary>{}</summary>\n                <div class=\"settings__prose\">{}</div>\n              </details>\n</form>",
                form_open(" class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"market_data_settings\"", &ctx.path("/settings/update_market_data"), "patch", csrf),
                submit(&connect, &validating),
                crate::web::i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", crate::web::i18n::Arg::Text("CoinGecko"))]),
                m.coingecko_instructions
            )
        } else {
            String::new()
        };
        body.push_str(&format!(
            "        <div class=\"sitem\" data-form--market-data-target=\"coingeckoFields\" {}>{coingecko}\n        </div>\n\n",
            if m.selected == "coingecko" { "" } else { "hidden" }
        ));
        let claim = if m.platform_connected {
            String::new()
        } else {
            let code = ctx.t("settings.platform.code");
            format!(
                "\n            {}\n{}              <p class=\"srow__hint\">{}</p>\n              <input required=\"required\" autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{code}\" aria-label=\"{code}\" type=\"text\" name=\"claim_code\" id=\"claim_code\" />\n              {}\n</form>",
                form_open(" class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"market_data_settings\"", &ctx.path("/settings/platform_connection"), "post", csrf),
                m.platform_errors.map(|errors| format!("                <p class=\"sform__error\" role=\"alert\">{}</p>\n", escape(errors))).unwrap_or_default(),
                ctx.t("settings.platform.subscription_html"),
                submit(&connect, &validating)
            )
        };
        body.push_str(&format!(
            "        <div class=\"sitem\" data-form--market-data-target=\"deltabadgerButtons\" {}>{claim}        </div>\n",
            if m.selected == "deltabadger" { "" } else { "hidden" }
        ));
    }
    if m.platform_connected {
        let control = format!(
            "\n          {}\n",
            super::dialog::confirm(csrf, &ctx.t("settings.platform.disconnect"), &ctx.t("settings.index.api_key_confirmation"), &ctx.t("settings.platform.disconnect"), &ctx.path("/settings/platform_connection"), "delete", &[])
        );
        body.push_str(&format!("\n        {}\n", row(&ctx.t("settings.platform.option"), Some(m.platform_note), "", "", None, Some(&control))));
    }
    format!("<turbo-frame id=\"market_data_settings\">{body}\n    </div>\n</turbo-frame>")
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
        out.push_str(&format!("\n    <h3 class=\"settings__heading{}\">{}</h3>", if index == 0 { " settings__heading--first" } else { "" }, ctx.t(heading)));
        if group.is_empty() {
            out.push_str(&format!("\n      <p class=\"settings__empty\">{}</p>\n", ctx.t("settings.sections.no_keys")));
        }
        for key in group {
            let permissions = if key.instructions {
                format!(
                    "\n          <a class=\"rbutton rbutton--outline\" data-turbo-frame=\"settings_pane\" href=\"{}\">{}</a>",
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
        "<turbo-frame id=\"mcp_settings\">\n  <h3 class=\"settings__heading settings__heading--first\">{}</h3>\n  <p class=\"settings__intro\">{}</p>\n\n  {}\n  {}</turbo-frame>",
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
        "\n    <a class=\"rbutton rbutton--outline\" title=\"{}\" data-turbo=\"false\" href=\"{}\">deltabadger-api.md</a>\n",
        ctx.t("settings.rest.download_api_docs"),
        escape(&ctx.path("/settings/download_api_docs"))
    );
    format!(
        "<turbo-frame id=\"rest_settings\">\n  <h3 class=\"settings__heading settings__heading--first\">{}</h3>\n  <p class=\"settings__intro\">{}</p>\n\n\n  {}\n  {}\n  {}</turbo-frame>",
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
        "<turbo-frame id=\"tool_permissions\">\n  <h3 class=\"settings__heading\">{}</h3>\n  <p class=\"settings__intro\">{}</p>\n",
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
    let mut out = format!("<turbo-frame id=\"connected_clients\">\n  <h3 class=\"settings__heading\">{}</h3>", ctx.t("settings.mcp.connected_clients"));
    if clients.is_empty() {
        out.push_str(&format!("\n    <p class=\"settings__empty\">{}</p>", ctx.t("settings.mcp.no_clients")));
    } else {
        out.push_str(&format!("\n    <p class=\"settings__intro\">{}</p>", ctx.t("settings.mcp.client_permissions_hint")));
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
