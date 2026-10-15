//! The settings dialog: settings/_shell (the sidebar and the pane), SettingsHelper#settings_row,
//! settings/_switch and settings/_confirm. Each writes what its Rails partial writes, whitespace included,
//! because the page comparison keeps whitespace between tags.
use crate::web::{i18n::escape, layout::Ctx};

/// SettingsController::SECTIONS, in the sidebar's order.
pub const SECTIONS: [&str; 5] = ["account", "api_keys", "connect", "mcp", "api"];

const ACCOUNT: &str = "<svg fill=\"none\" viewbox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\">\n<path class=\"fill--icon\" clip-rule=\"evenodd\" d=\"M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16ZM2 12a10 10 0 1 1 20 0 10 10 0 0 1-20 0Z\" fill-rule=\"evenodd\"></path>\n<path class=\"fill--icon\" clip-rule=\"evenodd\" d=\"M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Zm-5 3a5 5 0 1 1 10 0 5 5 0 0 1-10 0Z\" fill-rule=\"evenodd\"></path>\n<path class=\"fill--icon\" clip-rule=\"evenodd\" d=\"M12 17c-2 0-3.8.6-5.4 1.8l-1.2-1.6c2-1.4 4.2-2.2 6.6-2.2 2.4 0 4.7.8 6.6 2.2l-1.2 1.6A9.2 9.2 0 0 0 12 17Z\" fill-rule=\"evenodd\"></path>\n</svg>\n";
const LOCK: &str = "<svg fill=\"none\" viewbox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\">\n<path class=\"fill--icon\" d=\"M12 2C14.7352 2 17 4.05492 17 7V9H19C19.5523 9 20 9.44772 20 10V20C20 20.5523 19.5523 21 19 21H5L4.89746 20.9951C4.42703 20.9472 4.05278 20.573 4.00488 20.1025L4 20V10C4 9.48232 4.39333 9.05621 4.89746 9.00488L5 9H7V7C7 4.05492 9.2648 2 12 2ZM6 19H18V11H6V19ZM12 4C10.3169 4 9 5.21121 9 7V9H15V7C15 5.21121 13.6831 4 12 4Z\"></path>\n</svg>\n";
const SYNC: &str = "<svg class=\"icon-24 icon-24--sync\" fill=\"none\" viewbox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\">\n<path class=\"fill--icon\" d=\"M3.9 14.5a1 1 0 0 0-.9 1V20a1 1 0 0 0 2 0v-2.08a9.2 9.2 0 0 0 9.33 2.77c3.73-1 6.2-4.03 6.66-7.56a1 1 0 0 0-1.98-.26 7 7 0 0 1-5.2 5.9 7.2 7.2 0 0 1-7.4-2.27H8.5a1 1 0 1 0 0-2H3.9M9.67 3.3a9 9 0 0 0-6.66 7.57 1 1 0 0 0 1.98.26 7 7 0 0 1 5.2-5.9 7.2 7.2 0 0 1 7.4 2.27H15.5a1 1 0 0 0 0 2H20a1 1 0 0 0 1-1V4a1 1 0 0 0-2 0v2.08a9.2 9.2 0 0 0-9.33-2.77\"></path>\n</svg>\n";
const CHAT: &str = "<svg fill=\"none\" viewbox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\">\n<path class=\"fill--icon\" d=\"M20 11.7993C19.9998 8.57205 16.638 5.6001 12 5.6001C7.36202 5.6001 4.00021 8.57205 4 11.7993C4 13.5027 4.90148 15.0991 6.46387 16.2642C6.79685 16.5125 6.94207 16.9409 6.82812 17.3403L6.36914 18.9448L8.85254 17.7622L8.93262 17.728C9.12231 17.6572 9.32971 17.6449 9.52734 17.6948C10.3049 17.8916 11.1356 17.9995 12 17.9995C16.638 17.9995 20 15.0268 20 11.7993ZM22 11.7993C22 16.5247 17.3031 19.9995 12 19.9995C11.0991 19.9995 10.2249 19.9016 9.39258 19.7183L5.22949 21.7026C4.87479 21.8715 4.45433 21.8176 4.1543 21.564C3.85425 21.3101 3.73008 20.9038 3.83789 20.5259L4.72266 17.4243C3.06832 15.9855 2 14.0192 2 11.7993C2.00026 7.074 6.69714 3.6001 12 3.6001C17.3029 3.6001 21.9997 7.074 22 11.7993Z\" fill=\"#0039E6\"></path>\n</svg>\n";
const DOC: &str = "<svg class=\"icon-24 icon-24--doc\" fill=\"none\" viewbox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\">\n<path class=\"fill--icon\" d=\"M13.8 2q.3 0 .7.3l5.2 5.4q.3.3.3.7V21q0 1-1 1H5a1 1 0 0 1-1-1V3q0-1 1-1zM6 20h12V10h-5a1 1 0 0 1-1-1V4H6zm8-12h3.2L14 4.7z\"></path>\n</svg>\n";
const CLOSE: &str = "<svg class=\"icon-24\" fill=\"none\" viewbox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\">\n<path d=\"M5.7 4.3a1 1 0 0 0-1.4 1.4l6.3 6.3-6.3 6.3a1 1 0 1 0 1.4 1.4l6.3-6.3 6.3 6.3a1 1 0 0 0 1.4-1.4L13.4 12l6.3-6.3a1 1 0 0 0-1.4-1.4L12 10.6 5.7 4.3Z\" fill=\"#2948A1\"></path>\n</svg>\n";
pub const BACK: &str = "<svg class=\"icon-24\" xmlns=\"http://www.w3.org/2000/svg\" viewbox=\"0 0 24 24\" fill=\"none\">\n<path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-linejoin=\"round\" stroke-width=\"2\" d=\"m10 19-7-7 7-7M3 12h18\"></path>\n</svg>\n";

/// SettingsHelper::SETTINGS_SECTIONS.
const NAV: [(&str, &str, &str); 5] = [
    ("account", "links.account", ACCOUNT),
    ("api_keys", "settings.index.api_keys_section", LOCK),
    ("connect", "links.connections", SYNC),
    ("mcp", "settings.sections.mcp_server", CHAT),
    ("api", "settings.rest.title", DOC),
];

/// The version the sidebar shows: the release, as src-tauri/Cargo.toml carries it.
pub fn version() -> &'static str {
    include_str!("../../../../src-tauri/Cargo.toml")
        .lines()
        .find_map(|line| line.strip_prefix("version = ").and_then(|s| s.strip_prefix('"')).and_then(|s| s.strip_suffix('"')))
        .unwrap_or("0.0.0")
}

/// settings/_shell around `pane`. `connect` is SettingsHelper#settings_connect_visible?: Connect holds
/// only container-wide sources (admin) and IBKR, and is left out where it would be empty.
pub fn shell(ctx: &Ctx, section: &str, connect: bool, pane: &str) -> String {
    let items: String = NAV
        .iter()
        .filter(|(name, _, _)| *name != "connect" || connect)
        .map(|(name, label, icon)| {
            let active = *name == section;
            format!(
                "              <a class=\"settings__item{}\"{} href=\"{}\">\n                {icon}<span>{}</span>\n</a>",
                if active { " settings__item--active" } else { "" },
                if active { " aria-current=\"page\"" } else { "" },
                escape(&ctx.path(&format!("/settings/{name}"))),
                ctx.t(label)
            )
        })
        .collect();
    // Opened from the menu it is a frame over the page behind; visited as a page, closing goes to the bots.
    let close = if ctx.turbo_frame.is_none() {
        format!(" data-modal--base-close-url-value=\"{}\"", escape(&ctx.path("/bots")))
    } else {
        String::new()
    };
    format!(
        "<turbo-frame id=\"modal\">\n  <dialog class=\"dialog sdialog\" data-controller=\"modal--base\" data-action=\"click-&gt;modal--base#clickOutside\"{close}>\n    <div class=\"settings\">\n      <button name=\"button\" type=\"button\" class=\"settings__close\" aria-label=\"{}\" data-action=\"modal--base#animateOutCloseAndCleanUp\">\n        {CLOSE}\n</button>      <turbo-frame class=\"settings__frame\" id=\"settings_pane\">\n        <nav class=\"settings__nav\">\n          <div class=\"settings__items\" data-controller=\"settings-nav\">\n{items}          </div>\n          <div class=\"settings__version\">{}</div>\n        </nav>\n        <div class=\"settings__pane\" tabindex=\"-1\" autofocus>\n          \n{pane}\n\n        </div>\n</turbo-frame>    </div>\n</dialog></turbo-frame>",
        escape(&ctx.t("button.close")),
        version()
    )
}

/// SettingsHelper#settings_row. `label`, `hint` and `icon` are markup; `class` and `attributes` (already
/// rendered, each with its leading space) go on the row; `control` is the block, None without one.
pub fn row(label: &str, hint: Option<&str>, class: &str, attributes: &str, icon: Option<&str>, control: Option<&str>) -> String {
    let icon_markup = icon.map(|icon| format!("<div class=\"srow__icon\">{icon}</div>")).unwrap_or_default();
    let hint = hint.filter(|hint| !hint.is_empty()).map(|hint| format!("<div class=\"srow__hint\">{hint}</div>")).unwrap_or_default();
    let control = control.map(|control| format!("<div class=\"srow__control\">{control}</div>")).unwrap_or_default();
    format!(
        "<div class=\"srow{}\"{attributes}><div class=\"srow__text{}\">{icon_markup}<div class=\"srow__label\">{label}</div>{hint}</div>{control}</div>",
        if class.is_empty() { String::new() } else { format!(" {class}") },
        if icon.is_some() { " srow__text--icon" } else { "" }
    )
}

/// ActiveSupport's String#parameterize for the ASCII it is given here: anything but a letter, a digit,
/// `-` or `_` becomes one `-`, without one at either end, lowercased.
pub fn parameterize(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut squeezed = String::new();
    for ch in out.chars() {
        if ch == '-' && squeezed.ends_with('-') { continue; }
        squeezed.push(ch);
    }
    squeezed.trim_matches('-').to_string()
}

/// One switch of settings/_switch: posts `name`=`enabled` (with `fields`) to `url`, shows `state`
/// ("on", "partial" or "off"). Its checkbox id is the url and the fields, as Rails makes it.
#[allow(clippy::too_many_arguments)]
pub fn switch(csrf: &str, url: &str, frame: &str, name: &str, enabled: &str, state: &str, title: Option<&str>, fields: &[(&str, Option<&str>)]) -> String {
    let mut parts = vec!["switch".to_string(), url.to_string()];
    parts.extend(fields.iter().map(|(_, value)| value.unwrap_or("").to_string()));
    let id = parameterize(&parts.join(" "));
    let hidden: String = fields
        .iter()
        .map(|(field, value)| match value {
            Some(value) => format!("<input value=\"{}\" type=\"hidden\" name=\"{}\" />", escape(value), escape(field)),
            None => format!("<input type=\"hidden\" name=\"{}\" />", escape(field)),
        })
        .collect();
    let titled = title.map(|title| format!(" title=\"{0}\" aria-label=\"{0}\"", escape(title))).unwrap_or_default();
    format!(
        "<form class=\"sswitch\" data-controller=\"form--submit\" data-turbo-frame=\"{}\" action=\"{}\" accept-charset=\"UTF-8\" method=\"post\"><input type=\"hidden\" name=\"_method\" value=\"patch\" /><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />\n  {hidden}\n  <input value=\"{}\" type=\"hidden\" name=\"{}\" />\n  <label class=\"toggle toggle--small{}\"{titled}>\n    <input type=\"checkbox\" id=\"{id}\" {}data-action=\"change->form--submit#submit\" />\n    <div class=\"toggle__style\"></div>\n</label>  <label class=\"sswitch__hit\" for=\"{id}\" aria-hidden=\"true\"></label>\n</form>",
        escape(frame),
        escape(url),
        escape(enabled),
        escape(name),
        if state == "partial" { " toggle--partial" } else { "" },
        if state == "off" { "" } else { "checked " }
    )
}

/// settings/_confirm: a destructive button that opens a panel saying what happens, with the real button.
pub fn confirm(csrf: &str, label: &str, text: &str, confirm: &str, url: &str, method: &str, params: &[(&str, &str)]) -> String {
    let method_field = if method == "post" { String::new() } else { format!("<input type=\"hidden\" name=\"_method\" value=\"{method}\" />") };
    let params: String = params
        .iter()
        .map(|(name, value)| format!("<input type=\"hidden\" name=\"{}\" value=\"{}\" />", escape(name), escape(value)))
        .collect();
    format!(
        "<details class=\"sconfirm\">\n  <summary class=\"rbutton rbutton--outline\">{label}</summary>\n  <div class=\"sconfirm__panel\">\n    <p>{text}</p>\n    <form class=\"button_to\" method=\"post\" action=\"{}\">{method_field}<button class=\"rbutton rbutton--outline rbutton--danger\" type=\"submit\">{confirm}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />{params}</form>\n  </div>\n</details>\n",
        escape(url)
    )
}
