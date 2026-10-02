//! Rails' I18n as this app uses it: config/locales/*.yml (embedded by build.rs), looked up by full
//! key with a fallback to English, `%{name}` interpolation, and pluralisation with Rails' default
//! one/other rule plus the Russian rule in config/locales/plurals.rb.
//! `t` is the view helper (ActionView's `translate`); `text` is `I18n.t`, which controllers use.

include!(concat!(env!("OUT_DIR"), "/translations.rs"));

pub const DEFAULT: &str = "en";

/// Keys Rails takes from a gem's own locale file, not from config/locales. Pinned by tests/i18n.rs.
const FROM_GEMS: &[(&str, &str)] = &[("en.devise.failure.locked", "Your account is locked.")];

pub enum Arg<'a> {
    /// Plain text: escaped when the key is an HTML key.
    Text(&'a str),
    /// Markup that is already safe (`html_safe` in Rails): never escaped.
    Html(&'a str),
    /// `count:`; it also selects the plural form.
    Count(i64),
}

fn leaf(full_key: &str) -> Option<&'static str> {
    TRANSLATIONS.binary_search_by(|(k, _)| (*k).cmp(full_key)).ok().map(|i| TRANSLATIONS[i].1)
        .or_else(|| FROM_GEMS.iter().find(|(k, _)| *k == full_key).map(|(_, v)| *v))
}

/// config/locales/plurals.rb: only `ru` has a rule; every other locale uses I18n's one/other.
fn category(locale: &str, n: i64) -> &'static str {
    if n == 1 {
        "one"
    } else if locale == "ru" && [2, 3, 4].contains(&(n % 10)) && ![12, 13, 14, 22, 23, 24].contains(&(n % 100)) {
        "few"
    } else {
        "other"
    }
}

fn in_locale(locale: &str, key: &str, count: Option<i64>) -> Option<&'static str> {
    let base = format!("{locale}.{key}");
    if let Some(text) = leaf(&base) {
        return Some(text);
    }
    let n = count?;
    let form = |name: &str| leaf(&format!("{base}.{name}"));
    if n == 0 {
        if let Some(zero) = form("zero") {
            return Some(zero);
        }
    }
    // I18n::Backend::Pluralization: with a rule, the explicit "0"/"1" keys win and a missing form
    // falls back to `other`. Without one Rails raises on a missing form; here it reads as missing.
    if locale == "ru" && (n == 0 || n == 1) {
        if let Some(explicit) = form(if n == 0 { "0" } else { "1" }) {
            return Some(explicit);
        }
    }
    form(category(locale, n)).or_else(|| if locale == "ru" { form("other") } else { None })
}

fn lookup(locale: &str, key: &str, count: Option<i64>) -> Option<&'static str> {
    in_locale(locale, key, count).or_else(|| if locale == DEFAULT { None } else { in_locale(DEFAULT, key, count) })
}

/// ERB::Util.html_escape.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// I18n.interpolate: `%{name}` takes the argument, `%%{name}` is a literal `%{name}`. Rails raises
/// on a name with no argument; here the placeholder stays as written.
fn interpolate(template: &str, args: &[(&str, Arg)], escape_text: bool) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("%{") {
        let Some(length) = rest[start..].find('}') else { break };
        let name = &rest[start + 2..start + length];
        if rest[..start].ends_with('%') {
            out.push_str(&rest[..start - 1]);
            out.push_str(&rest[start..=start + length]);
        } else {
            out.push_str(&rest[..start]);
            match args.iter().find(|(n, _)| *n == name) {
                Some((_, Arg::Text(value))) if escape_text => out.push_str(&escape(value)),
                Some((_, Arg::Text(value))) | Some((_, Arg::Html(value))) => out.push_str(value),
                Some((_, Arg::Count(n))) => out.push_str(&n.to_string()),
                None => out.push_str(&rest[start..=start + length]),
            }
        }
        rest = &rest[start + length + 1..];
    }
    out.push_str(rest);
    out
}

fn count_of(args: &[(&str, Arg)]) -> Option<i64> {
    args.iter().find_map(|(name, arg)| match arg {
        Arg::Count(n) if *name == "count" => Some(*n),
        _ => None,
    })
}

/// ActiveSupport::HtmlSafeTranslation: a key ending in `_html`, or whose last segment is `html`.
fn html_key(key: &str) -> bool {
    key.ends_with("_html") || key == "html" || key.ends_with(".html")
}

/// String#titleize for a key segment: `two_factor_title` -> `Two Factor Title`.
fn titleize(segment: &str) -> String {
    let words = segment.trim_start_matches('_');
    let words = words.strip_suffix("_id").unwrap_or(words).replace('_', " ");
    let mut out = String::with_capacity(words.len());
    let mut word_start = true;
    for c in words.chars() {
        if word_start { out.extend(c.to_uppercase()) } else { out.push(c) }
        word_start = !c.is_alphanumeric();
    }
    out
}

fn value_text(arg: &Arg) -> String {
    match arg {
        Arg::Text(v) | Arg::Html(v) => v.to_string(),
        Arg::Count(n) => n.to_string(),
    }
}

/// The view helper `t`: markup ready to place in a page. A plain key's text is escaped; an HTML
/// key's text is trusted and only its plain arguments are escaped; a missing key is Rails'
/// `translation_missing` span.
pub fn t(locale: &str, key: &str, args: &[(&str, Arg)]) -> String {
    match lookup(locale, key, count_of(args)) {
        Some(template) if html_key(key) => interpolate(template, args, true),
        Some(template) => escape(&interpolate(template, args, false)),
        None => {
            let mut title = format!("translation missing: {locale}.{key}");
            for (name, arg) in args {
                title.push_str(&format!(", {name}: {}", escape(&value_text(arg))));
            }
            let last = key.rsplit('.').next().unwrap_or(key);
            format!("<span class=\"translation_missing\" title=\"{}\">{}</span>", escape(&title), escape(&titleize(last)))
        }
    }
}

/// `I18n.t`: plain text, for flash messages and plain-text responses. A missing key reads
/// "Translation missing: <locale>.<key>", as I18n's default handler returns.
pub fn text(locale: &str, key: &str, args: &[(&str, Arg)]) -> String {
    match lookup(locale, key, count_of(args)) {
        Some(template) => interpolate(template, args, false),
        None => format!("Translation missing: {locale}.{key}"),
    }
}

/// Every embedded (full key, text) pair, for the test that compares the table with Rails' own.
pub fn all() -> &'static [(&'static str, &'static str)] {
    TRANSLATIONS
}
