//! The `(:locale)` route scope: 2.x's routes, ApplicationController#switch_locale and
//! #default_url_options, and LocaleHelper#locale_switch_path.
use std::collections::hash_map::{Entry, HashMap};

/// config.i18n.available_locales, in 2.x's order (pinned by tests/locale.rs).
pub const LOCALES: [&str; 15] = ["en", "pl", "es", "de", "nl", "fr", "pt", "ru", "it", "bg", "el", "sv", "da", "cs", "sk"];
pub const DEFAULT: &str = "en";

/// First path segments that are routed outside the locale scope: `/de/up` is not a route.
const UNSCOPED: [&str; 4] = ["up", "cable", "oauth", ".well-known"];

pub fn known(value: &str) -> Option<&'static str> {
    LOCALES.iter().copied().find(|l| *l == value)
}

/// D-LANGUAGE-STEP: the language step's rows, larger language communities first. The desktop launcher's status page
/// keeps the same table (src-tauri/ui/index.html; tests/language.rs compares them).
pub const PICKER: [&str; 15] = ["en", "es", "de", "fr", "pt", "it", "nl", "pl", "ru", "cs", "sk", "sv", "da", "el", "bg"];

/// A language's name in that language: the language dropdown's label without its flag.
pub fn native_name(code: &str) -> &'static str {
    super::layout::LANGUAGES.iter().find(|(c, _)| *c == code).and_then(|(_, label)| label.split_once(' ')).map_or("English", |(_, name)| name)
}

/// What a first start does with the user's languages (D-LANGUAGE-STEP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The first preference is one of ours and not English: used unasked.
    Use(&'static str),
    /// English or a language we don't have: the picker asks, this one preselected.
    Ask(&'static str),
}

impl Choice {
    pub fn code(self) -> &'static str {
        match self { Choice::Use(code) | Choice::Ask(code) => code }
    }
}

/// A language tag's primary subtag, when it is one of `LOCALES` (`pl-PL`, `pl_PL` and `PL` are `pl`).
fn primary(tag: &str) -> Option<&'static str> {
    known(&tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase())
}

/// The rule (Jan 2026-11-20: "if the user's language is not English and we have it: use it and don't ask; if it is English
/// or a language we don't have: show the picker"), over the user's languages in preference order. The desktop launcher
/// has its own copy (src-tauri/src/lib.rs); both are tested against tests/fixtures/first_choice.json.
pub fn first_choice<S: AsRef<str>>(preferences: &[S]) -> Choice {
    match preferences.first().and_then(|first| primary(first.as_ref())) {
        Some(code) if code != DEFAULT => Choice::Use(code),
        _ => Choice::Ask(preferences.iter().find_map(|tag| primary(tag.as_ref())).unwrap_or(DEFAULT)),
    }
}

/// `Accept-Language`'s languages, most wanted first (by q, ties in header order). Dropped before the rule sees them: `*`,
/// a malformed item (a tag that is not `1*8ALPHA *("-" 1*8alphanum)`, a parameter other than a well-formed q), and an
/// item with q=0, which RFC 9110 §12.4.2 makes "not acceptable".
pub fn accept_language(header: &str) -> Vec<&str> {
    let tag_ok = |tag: &str| {
        let mut parts = tag.split('-');
        let first = parts.next().unwrap_or("");
        (1..=8).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_alphabetic())
            && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
    };
    // A qvalue in thousandths: "0" ["." 0*3DIGIT] / "1" ["." 0*3("0")].
    let qvalue = |text: &str| -> Option<u16> {
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) { return None; }
        let thousandths: u16 = format!("{fraction:0<3}").parse().ok()?;
        match whole { "0" => Some(thousandths), "1" if thousandths == 0 => Some(1000), _ => None }
    };
    let mut items: Vec<(&str, u16)> = header.split(',').filter_map(|item| {
        let mut parts = item.split(';');
        let tag = parts.next()?.trim();
        if !tag_ok(tag) { return None; }
        // At most one parameter, a well-formed q: a second q makes the item malformed, not a reweighting.
        let q = match (parts.next(), parts.next()) {
            (None, _) => 1000,
            (Some(parameter), None) => {
                let (name, value) = parameter.split_once('=')?;
                if !name.trim().eq_ignore_ascii_case("q") { return None; }
                qvalue(value.trim())?
            }
            (Some(_), Some(_)) => return None,
        };
        (q > 0).then_some((tag, q))
    }).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1));
    items.into_iter().map(|(tag, _)| tag).collect()
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

/// A path as 2.x's route helpers generate it under default_url_options: prefixed for every locale
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

/// 2.x's CGI.escape, which `to_query` uses: letters, digits and `_.-~` stay, a space is `+`.
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
