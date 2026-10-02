//! The `(:locale)` route scope: config/routes.rb, ApplicationController#switch_locale and
//! #default_url_options, and LocaleHelper#locale_switch_path.

/// config.i18n.available_locales, in Rails' order (pinned by tests/locale.rs).
pub const LOCALES: [&str; 15] = ["en", "pl", "es", "de", "nl", "fr", "pt", "ru", "it", "bg", "el", "sv", "da", "cs", "sk"];
pub const DEFAULT: &str = "en";

/// First path segments that are routed outside the locale scope: `/de/up` is not a route.
const UNSCOPED: [&str; 2] = ["up", "cable"];

pub fn known(value: &str) -> Option<&'static str> {
    LOCALES.iter().copied().find(|l| *l == value)
}

/// Splits a request path into its locale prefix and the path the routes match:
/// `/de/login` -> (Some("de"), "/login"), `/de` -> (Some("de"), "/"), `/zz/login` -> (None, "/zz/login").
pub fn split(path: &str) -> (Option<&'static str>, &str) {
    let rest = path.strip_prefix('/').unwrap_or(path);
    let (first, tail) = rest.split_once('/').map_or((rest, ""), |(first, tail)| (first, tail));
    match known(first) {
        Some(locale) if !UNSCOPED.contains(&tail.split('/').next().unwrap_or("")) => {
            (Some(locale), if tail.is_empty() { "/" } else { &path[first.len() + 1..] })
        }
        _ => (None, path),
    }
}

/// switch_locale: the request's `locale` parameter, else the signed-in user's, else the default.
/// A value that is not an available locale is ignored, not an error.
pub fn switch(param: Option<&str>, user: Option<&str>) -> &'static str {
    param.filter(|p| !p.is_empty()).or(user).and_then(known).unwrap_or(DEFAULT)
}

/// A path as Rails' route helpers generate it under default_url_options: prefixed for every locale
/// but the default. `path` starts with `/` and may carry a query string.
pub fn path(locale: &str, path: &str) -> String {
    if locale == DEFAULT {
        path.to_string()
    } else if path == "/" {
        format!("/{locale}")
    } else if let Some(query) = path.strip_prefix("/?") {
        format!("/{locale}?{query}")
    } else {
        format!("/{locale}{path}")
    }
}

/// The query keys locale_switch_path drops (LocaleHelper::URL_FOR_RESERVED).
const RESERVED: [&str; 16] = ["host", "protocol", "port", "script_name", "anchor", "only_path", "trailing_slash", "subdomain", "domain",
                              "tld_length", "params", "relative_url_root", "controller", "action", "format", "locale"];

/// Ruby's CGI.escape, which `to_query` uses: letters, digits and `_.-~` stay, a space is `+`.
fn cgi_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// locale_switch_path: the current page under another locale. The locale is always in the path here,
/// English included, and the query keeps its other keys, sorted, as `to_query` writes them.
pub fn switch_path(target: &str, route_path: &str, query: &[(String, String)]) -> String {
    let mut kept: Vec<String> = query.iter()
        .filter(|(key, _)| !RESERVED.contains(&key.split('[').next().unwrap_or(key)))
        .map(|(key, value)| format!("{}={}", cgi_escape(key), cgi_escape(value)))
        .collect();
    kept.sort();
    let base = if route_path == "/" { format!("/{target}") } else { format!("/{target}{route_path}") };
    if kept.is_empty() { base } else { format!("{base}?{}", kept.join("&")) }
}
