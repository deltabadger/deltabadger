//! The `(:locale)` route scope: config/routes.rb, ApplicationController#switch_locale and
//! #default_url_options, and LocaleHelper#locale_switch_path.
use std::collections::hash_map::{Entry, HashMap};

/// config.i18n.available_locales, in Rails' order (pinned by tests/locale.rs).
pub const LOCALES: [&str; 15] = ["en", "pl", "es", "de", "nl", "fr", "pt", "ru", "it", "bg", "el", "sv", "da", "cs", "sk"];
pub const DEFAULT: &str = "en";

/// First path segments that are routed outside the locale scope: `/de/up` is not a route.
const UNSCOPED: [&str; 4] = ["up", "cable", "oauth", ".well-known"];

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

/// The credential parameters locale_switch_path drops at any nesting depth
/// (LocaleHelper::SENSITIVE_QUERY; keep the two lists identical).
const SENSITIVE: [&str; 27] = ["confirmation_token", "password", "password_confirmation", "current_password", "otp_secret_key",
                               "otp_secret", "otp_code_token", "key", "secret", "passphrase", "access_token", "refresh_token",
                               "rsa_signature_key", "rsa_encryption_key", "dh_param", "api_token", "smtp_username", "smtp_password",
                               "coingecko_api_key", "alpaca_api_key", "alpaca_api_secret", "market_data_token", "reset_password_token",
                               "unlock_token", "token", "claim_code", "code"];

/// A query key naming a credential: any part of it (`user[password]`, `rows[][secret]`), in any letter case.
pub fn sensitive_query(key: &str) -> bool {
    key.split(['[', ']']).any(|part| SENSITIVE.iter().any(|secret| part.eq_ignore_ascii_case(secret)))
}

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

/// The query locale_switch_path gives every language link: the page's own, without the reserved
/// keys and the credentials (`sensitive_query`), as `Hash#to_query` writes it. Each key is one entry: a repeated key keeps its last value,
/// as Rack parsed it, and a list (`a[]`) keeps all its values in the order they came. The entries
/// are then sorted as whole strings, so keys are in order and a list's values are not reordered.
/// One pass over the query, made once per page (`Ctx::languages`), whatever the number of links.
/// ponytail: Rack's nesting beyond that (`a[][b]`, `a` beside `a[b]`) is not modelled; no link of this app writes such a query.
pub fn switch_query(query: &[(String, String)]) -> String {
    let mut by_key: HashMap<&str, String> = HashMap::new();
    for (key, value) in query.iter().filter(|(key, _)| !RESERVED.contains(&key.split('[').next().unwrap_or(key)) && !sensitive_query(key)) {
        let pair = format!("{}={}", cgi_escape(key), cgi_escape(value));
        match by_key.entry(key.as_str()) {
            Entry::Occupied(mut entry) if key.ends_with("[]") => { entry.get_mut().push('&'); entry.get_mut().push_str(&pair); }
            Entry::Occupied(mut entry) => { entry.insert(pair); }
            Entry::Vacant(entry) => { entry.insert(pair); }
        }
    }
    let mut kept: Vec<String> = by_key.into_values().collect();
    kept.sort();
    kept.join("&")
}

/// locale_switch_path: the current page under another locale, with the query `switch_query` made
/// of the page's. The locale is always in the path here, English included.
pub fn switch_path(target: &str, route_path: &str, query: &str) -> String {
    let base = if route_path == "/" { format!("/{target}") } else { format!("/{target}{route_path}") };
    if query.is_empty() { base } else { format!("{base}?{query}") }
}

pub fn without_secret_query(path:&str)->std::borrow::Cow<'_,str>{
    use std::borrow::Cow;
    let Some((base,query))=path.split_once('?') else{return Cow::Borrowed(path)};
    let pairs:Vec<_>=form_urlencoded::parse(query.as_bytes()).collect();
    if !pairs.iter().any(|(key,_)|sensitive_query(key)){return Cow::Borrowed(path)}
    let mut safe=form_urlencoded::Serializer::new(String::new());
    for (key,value) in pairs{if !sensitive_query(&key){safe.append_pair(&key,&value);}}
    let safe=safe.finish();Cow::Owned(if safe.is_empty(){base.to_string()}else{format!("{base}?{safe}")})
}
